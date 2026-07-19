//! Remote Session Stage 2 — grant mint / list / revoke / token auth.
//!
//! Contract under test (real harness, process-isolated HOME):
//!
//!   1. Mint grant while Layer 0 OFF is allowed; shell/spawn with that
//!      token still 403 REMOTE_SESSIONS_DISABLED (Layer 0 first).
//!   2. Enable + shell/spawn with k2rs_ token → 200 ready:false.
//!   3. Spawn with wrong token → NO_GRANT (or invalid token at gate).
//!   4. Revoke → spawn → GRANT_REVOKED.
//!   5. Expired grant (force_expire) → GRANT_EXPIRED.
//!   6. List never contains token/hash; status.activeGrants reflects count.
//!   7. Layer 0 still first: disable + valid grant → REMOTE_SESSIONS_DISABLED.
//!
//! Isolation mirrors `remote_session_layer0_integration.rs`: TEST_LOCK + temp HOME.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-remote-session-grants";

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream =
        StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!(
            "{method} {path_and_query} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\r\n"
        ),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");

    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some((status, body, complete)) = try_parse(&raw) {
            if complete {
                return Resp { status, body };
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                break
            }
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));
    let body = match text.split_once("\r\n\r\n") {
        Some((_h, b)) => b.to_string(),
        None => String::new(),
    };
    Resp { status, body }
}

fn try_parse(raw: &[u8]) -> Option<(u16, String, bool)> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())?;
    let content_len = headers.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let complete = match content_len {
        Some(clen) => body.len() >= clen,
        None => true,
    };
    Some((status, body.to_string(), complete))
}

/// Redirect `$HOME` to a fresh tempdir; restore on drop.
struct HomeEnv {
    prev_home: Option<std::ffi::OsString>,
    tmp_home: std::path::PathBuf,
}

impl HomeEnv {
    fn set() -> Self {
        let prev_home = std::env::var_os("HOME");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_home = std::env::temp_dir().join(format!(
            "k2-remote-session-grants-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp_home).expect("create temp HOME");
        std::env::set_var("HOME", &tmp_home);
        Self { prev_home, tmp_home }
    }
}

impl Drop for HomeEnv {
    fn drop(&mut self) {
        match &self.prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.tmp_home);
    }
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

fn q_owner(path: &str) -> String {
    format!("{path}?token={OWNER_TOKEN}")
}

fn q_token(path: &str, token: &str) -> String {
    format!("{path}?token={token}")
}

#[tokio::test(flavor = "multi_thread")]
async fn grant_lifecycle_mint_use_revoke_expire_list() {
    let _g = lock();
    let _home = HomeEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;

    // ── 1) Mint while OFF is allowed ────────────────────────────────
    let mint = http(
        d.port,
        "POST",
        &q_owner("/cli/remote-session/grant"),
        Some(r#"{"scope":"shell","ttlSeconds":1800,"label":"ci-lab"}"#),
    );
    assert_eq!(mint.status, 200, "mint while OFF must work; body={}", mint.body);
    let mint_v = json(&mint.body);
    assert_eq!(mint_v["ok"], true, "body={}", mint.body);
    let token = mint_v["token"]
        .as_str()
        .unwrap_or_else(|| panic!("token once: {}", mint.body))
        .to_string();
    assert!(
        token.starts_with("k2rs_"),
        "token must use k2rs_ prefix; token={token}"
    );
    let grant_id = mint_v["grant"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("grant.id: {}", mint.body))
        .to_string();
    assert!(grant_id.starts_with("rs_"), "id={grant_id}");
    assert_eq!(mint_v["grant"]["scope"], "shell");
    assert_eq!(mint_v["grant"]["principalKind"], "grant_token");
    assert_eq!(mint_v["grant"]["label"], "ci-lab");
    assert!(mint_v["grant"]["expiresAt"].as_i64().unwrap_or(0) > 0);

    // status.activeGrants should be 1 even while OFF (mint counts).
    let status0 = http(d.port, "GET", &q_owner("/cli/remote-session/status"), None);
    assert_eq!(status0.status, 200, "body={}", status0.body);
    let s0 = json(&status0.body);
    assert_eq!(s0["enabled"], false, "still OFF; body={}", status0.body);
    assert_eq!(
        s0["activeGrants"], 1,
        "minted grant is active; body={}",
        status0.body
    );

    // Use while OFF → Layer 0 first.
    let spawn_off = http(
        d.port,
        "POST",
        &q_token("/cli/remote-session/shell/spawn", &token),
        Some("{}"),
    );
    assert_eq!(
        spawn_off.status, 403,
        "valid grant while OFF must 403; body={}",
        spawn_off.body
    );
    let off_v = json(&spawn_off.body);
    assert_eq!(
        off_v["error"]["code"], "REMOTE_SESSIONS_DISABLED",
        "Layer 0 must fire before grant check; body={}",
        spawn_off.body
    );

    // ── 2) Enable + spawn with k2rs_ → 200 ready:false ──────────────
    let en = http(d.port, "POST", &q_owner("/cli/remote-session/enable"), Some("{}"));
    assert_eq!(en.status, 200, "body={}", en.body);

    let spawn_ok = http(
        d.port,
        "POST",
        &q_token("/cli/remote-session/shell/spawn", &token),
        Some("{}"),
    );
    assert_eq!(
        spawn_ok.status, 200,
        "valid grant while ON must 200; body={}",
        spawn_ok.body
    );
    let ok_v = json(&spawn_ok.body);
    assert_eq!(ok_v["ok"], true, "body={}", spawn_ok.body);
    assert_eq!(ok_v["ready"], false, "no PTY yet; body={}", spawn_ok.body);
    assert_eq!(ok_v["grantId"], grant_id, "body={}", spawn_ok.body);
    let hint = ok_v["hint"].as_str().unwrap_or("");
    assert!(
        hint.to_ascii_lowercase().contains("stage 3"),
        "hint must mention Stage 3; hint={hint}"
    );

    // ── 3) Wrong token → NO_GRANT (owner token while ON) ────────────
    let spawn_owner = http(
        d.port,
        "POST",
        &q_owner("/cli/remote-session/shell/spawn"),
        Some("{}"),
    );
    assert_eq!(
        spawn_owner.status, 403,
        "owner without grant → 403; body={}",
        spawn_owner.body
    );
    let owner_v = json(&spawn_owner.body);
    assert_eq!(
        owner_v["error"]["code"], "NO_GRANT",
        "body={}",
        spawn_owner.body
    );

    // Completely unknown k2rs_ shape that is well-formed but not in DB.
    let fake = "k2rs_doesnotexist00000000000000000000000000";
    let spawn_fake = http(
        d.port,
        "POST",
        &q_token("/cli/remote-session/shell/spawn", fake),
        Some("{}"),
    );
    assert_eq!(spawn_fake.status, 403, "body={}", spawn_fake.body);
    let fake_v = json(&spawn_fake.body);
    assert_eq!(
        fake_v["error"]["code"], "NO_GRANT",
        "unknown grant hash → NO_GRANT; body={}",
        spawn_fake.body
    );

    // ── 6) List never contains token/hash ───────────────────────────
    let list = http(d.port, "GET", &q_owner("/cli/remote-session/grants"), None);
    assert_eq!(list.status, 200, "body={}", list.body);
    assert!(
        !list.body.contains(&token),
        "list must not leak plaintext token; body={}",
        list.body
    );
    assert!(
        !list.body.contains("credential"),
        "list must not expose credential_hash key; body={}",
        list.body
    );
    // Body of the token (after prefix) also must not appear.
    let body_part = token.strip_prefix("k2rs_").unwrap_or(&token);
    assert!(
        !list.body.contains(body_part),
        "list must not leak token body; body={}",
        list.body
    );
    let list_v = json(&list.body);
    assert_eq!(list_v["ok"], true);
    let grants = list_v["grants"]
        .as_array()
        .unwrap_or_else(|| panic!("grants array; body={}", list.body));
    assert!(
        grants.iter().any(|g| g["id"] == grant_id),
        "minted grant listed; body={}",
        list.body
    );

    // ── 4) Revoke → spawn → GRANT_REVOKED ───────────────────────────
    let rev = http(
        d.port,
        "POST",
        &q_owner("/cli/remote-session/revoke"),
        Some(&format!(r#"{{"id":"{grant_id}"}}"#)),
    );
    assert_eq!(rev.status, 200, "body={}", rev.body);
    let rev_v = json(&rev.body);
    assert_eq!(rev_v["ok"], true);
    assert!(
        rev_v["grant"]["revokedAt"].as_i64().is_some(),
        "revokedAt set; body={}",
        rev.body
    );

    let status_rev = http(d.port, "GET", &q_owner("/cli/remote-session/status"), None);
    let sr = json(&status_rev.body);
    assert_eq!(
        sr["activeGrants"], 0,
        "revoked grant not active; body={}",
        status_rev.body
    );

    let spawn_rev = http(
        d.port,
        "POST",
        &q_token("/cli/remote-session/shell/spawn", &token),
        Some("{}"),
    );
    assert_eq!(spawn_rev.status, 403, "body={}", spawn_rev.body);
    let rev_spawn = json(&spawn_rev.body);
    assert_eq!(
        rev_spawn["error"]["code"], "GRANT_REVOKED",
        "body={}",
        spawn_rev.body
    );

    // ── 5) Expired grant → GRANT_EXPIRED ────────────────────────────
    let mint2 = http(
        d.port,
        "POST",
        &q_owner("/cli/remote-session/grant"),
        Some(r#"{"scope":"shell","ttlSeconds":60,"label":"expire-me"}"#),
    );
    assert_eq!(mint2.status, 200, "body={}", mint2.body);
    let m2 = json(&mint2.body);
    let token2 = m2["token"].as_str().unwrap().to_string();
    let id2 = m2["grant"]["id"].as_str().unwrap().to_string();

    // Force expiry via core helper (same process / same DB as harness).
    k2_core::remote_sessions::force_expire_grant(&id2).expect("force expire");

    let spawn_exp = http(
        d.port,
        "POST",
        &q_token("/cli/remote-session/shell/spawn", &token2),
        Some("{}"),
    );
    assert_eq!(spawn_exp.status, 403, "body={}", spawn_exp.body);
    let exp_v = json(&spawn_exp.body);
    assert_eq!(
        exp_v["error"]["code"], "GRANT_EXPIRED",
        "body={}",
        spawn_exp.body
    );

    // ── 7) Layer 0 still first with a fresh valid grant ─────────────
    let mint3 = http(
        d.port,
        "POST",
        &q_owner("/cli/remote-session/grant"),
        Some(r#"{"scope":"shell","ttlSeconds":1800}"#),
    );
    assert_eq!(mint3.status, 200, "body={}", mint3.body);
    let token3 = json(&mint3.body)["token"].as_str().unwrap().to_string();

    let dis = http(
        d.port,
        "POST",
        &q_owner("/cli/remote-session/disable"),
        Some("{}"),
    );
    assert_eq!(dis.status, 200, "body={}", dis.body);

    let spawn_dis = http(
        d.port,
        "POST",
        &q_token("/cli/remote-session/shell/spawn", &token3),
        Some("{}"),
    );
    assert_eq!(spawn_dis.status, 403, "body={}", spawn_dis.body);
    let dis_v = json(&spawn_dis.body);
    assert_eq!(
        dis_v["error"]["code"], "REMOTE_SESSIONS_DISABLED",
        "disable + valid grant → Layer 0 first; body={}",
        spawn_dis.body
    );

    // Grant tokens must NOT enable the switch.
    let en_grant = http(
        d.port,
        "POST",
        &q_token("/cli/remote-session/enable", &token3),
        Some("{}"),
    );
    assert_eq!(
        en_grant.status, 403,
        "grant token must not enable; body={}",
        en_grant.body
    );

    // runbook scope rejected.
    let runbook = http(
        d.port,
        "POST",
        &q_owner("/cli/remote-session/grant"),
        Some(r#"{"scope":"runbook","ttlSeconds":60}"#),
    );
    assert_eq!(runbook.status, 400, "body={}", runbook.body);
    assert!(
        runbook.body.contains("not_implemented") || runbook.body.contains("runbook"),
        "body={}",
        runbook.body
    );
}
