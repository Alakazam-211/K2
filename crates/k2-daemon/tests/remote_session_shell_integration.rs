//! Remote Session Stage 3 — real shell PTY spawn + grant-bound I/O gate.
//!
//! Contract under test (real harness, process-isolated HOME):
//!
//!   1. enable + mint + spawn with k2rs_ → 200 ready:true sessionId UUID
//!   2. write with k2rs_ token → success (not 403 auth)
//!   3. read with k2rs_ → 200 lines array
//!   4. write with wrong grant token → 403
//!   5. revoke → write → 403 GRANT_REVOKED (or session gone)
//!   6. disable → kills; spawn/write blocked REMOTE_SESSIONS_DISABLED
//!   7. Normal (non-remote) terminal path still works with owner token (smoke)
//!
//! Isolation mirrors grants/layer0 tests: TEST_LOCK + temp HOME.

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

const OWNER_TOKEN: &str = "owner-token-deadbeef-remote-session-shell";

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream =
        StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
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
            "k2-remote-session-shell-{}-{nanos}",
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

fn urlenc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn shell_spawn_write_read_gate_revoke_disable() {
    let _g = lock();
    let _home = HomeEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;

    // ── enable + mint ───────────────────────────────────────────────
    let en = http(d.port, "POST", &q_owner("/cli/remote-session/enable"), Some("{}"));
    assert_eq!(en.status, 200, "enable: {}", en.body);

    let mint = http(
        d.port,
        "POST",
        &q_owner("/cli/remote-session/grant"),
        Some(r#"{"scope":"shell","ttlSeconds":1800,"label":"shell-ci"}"#),
    );
    assert_eq!(mint.status, 200, "mint: {}", mint.body);
    let mint_v = json(&mint.body);
    let token = mint_v["token"].as_str().unwrap().to_string();
    let grant_id = mint_v["grant"]["id"].as_str().unwrap().to_string();

    // ── 1) spawn → ready:true + sessionId UUID ──────────────────────
    let spawn = http(
        d.port,
        "POST",
        &q_token("/cli/remote-session/shell/spawn", &token),
        Some("{}"),
    );
    assert_eq!(spawn.status, 200, "spawn must 200; body={}", spawn.body);
    let sv = json(&spawn.body);
    assert_eq!(sv["ok"], true, "body={}", spawn.body);
    assert_eq!(sv["ready"], true, "real PTY; body={}", spawn.body);
    assert_eq!(sv["grantId"], grant_id, "body={}", spawn.body);
    let session_id = sv["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("sessionId required; body={}", spawn.body))
        .to_string();
    assert!(
        uuid::Uuid::parse_str(&session_id).is_ok(),
        "sessionId must be UUID; got {session_id}"
    );

    // status lists the live session
    let status = http(d.port, "GET", &q_owner("/cli/remote-session/status"), None);
    assert_eq!(status.status, 200, "body={}", status.body);
    let st = json(&status.body);
    let sessions = st["activeSessions"]
        .as_array()
        .unwrap_or_else(|| panic!("activeSessions; body={}", status.body));
    assert!(
        sessions
            .iter()
            .any(|s| s["sessionId"] == session_id && s["grantId"] == grant_id),
        "activeSessions must include bound shell; body={}",
        status.body
    );

    // Give the login shell a moment to settle before write/read.
    std::thread::sleep(Duration::from_millis(400));

    // ── 2) write with k2rs_ → not 403 auth ──────────────────────────
    let msg = urlenc("echo remote-session-stage3-ok");
    let write_path = format!(
        "/cli/terminal/write?token={}&id={}&message={}&no_submit=true",
        urlenc(&token),
        urlenc(&session_id),
        msg
    );
    let wr = http(d.port, "GET", &write_path, None);
    assert_ne!(
        wr.status, 403,
        "matching grant must not auth-deny write; body={}",
        wr.body
    );
    assert_eq!(
        wr.status, 200,
        "write should succeed; body={}",
        wr.body
    );
    let wr_v = json(&wr.body);
    assert_eq!(wr_v["success"], true, "body={}", wr.body);

    // ── 3) read with k2rs_ → 200 lines array ────────────────────────
    let read_path = format!(
        "/cli/terminal/read?token={}&id={}&lines=50",
        urlenc(&token),
        urlenc(&session_id)
    );
    let rd = http(d.port, "GET", &read_path, None);
    assert_eq!(rd.status, 200, "read must 200; body={}", rd.body);
    let rd_v = json(&rd.body);
    assert!(
        rd_v["lines"].is_array(),
        "lines array required; body={}",
        rd.body
    );

    // ── 4) wrong grant token → 403 ──────────────────────────────────
    let mint2 = http(
        d.port,
        "POST",
        &q_owner("/cli/remote-session/grant"),
        Some(r#"{"scope":"shell","ttlSeconds":1800,"label":"other"}"#),
    );
    assert_eq!(mint2.status, 200, "body={}", mint2.body);
    let token2 = json(&mint2.body)["token"].as_str().unwrap().to_string();
    let write_wrong = format!(
        "/cli/terminal/write?token={}&id={}&message=nope&no_submit=true",
        urlenc(&token2),
        urlenc(&session_id)
    );
    let ww = http(d.port, "GET", &write_wrong, None);
    assert_eq!(ww.status, 403, "wrong grant must 403; body={}", ww.body);
    let ww_v = json(&ww.body);
    assert_eq!(
        ww_v["error"]["code"], "NO_GRANT",
        "body={}",
        ww.body
    );

    // Owner ops break-glass may write remote shells.
    let write_owner = format!(
        "/cli/terminal/write?token={}&id={}&message=owner-ops&no_submit=true",
        urlenc(OWNER_TOKEN),
        urlenc(&session_id)
    );
    let wo = http(d.port, "GET", &write_owner, None);
    assert_eq!(
        wo.status, 200,
        "owner must write remote shell; body={}",
        wo.body
    );

    // ── 5) revoke → write → GRANT_REVOKED (or session gone) ─────────
    let rev = http(
        d.port,
        "POST",
        &q_owner("/cli/remote-session/revoke"),
        Some(&format!(r#"{{"id":"{grant_id}"}}"#)),
    );
    assert_eq!(rev.status, 200, "body={}", rev.body);
    let rev_v = json(&rev.body);
    assert!(
        rev_v["killedSessions"].as_u64().unwrap_or(0) >= 1,
        "revoke must kill bound sessions; body={}",
        rev.body
    );

    let write_rev = format!(
        "/cli/terminal/write?token={}&id={}&message=after-revoke&no_submit=true",
        urlenc(&token),
        urlenc(&session_id)
    );
    let wrv = http(d.port, "GET", &write_rev, None);
    assert!(
        wrv.status == 403 || wrv.status == 400,
        "post-revoke write must fail; status={} body={}",
        wrv.status,
        wrv.body
    );
    if wrv.status == 403 {
        let body_v = json(&wrv.body);
        let code = body_v["error"]["code"].as_str().unwrap_or("");
        assert!(
            code == "GRANT_REVOKED" || code == "NO_GRANT" || code == "REMOTE_SESSIONS_DISABLED",
            "expected grant denial code; body={}",
            wrv.body
        );
    }

    // ── 6) disable blocks spawn + write with REMOTE_SESSIONS_DISABLED ─
    let mint3 = http(
        d.port,
        "POST",
        &q_owner("/cli/remote-session/grant"),
        Some(r#"{"scope":"shell","ttlSeconds":1800}"#),
    );
    assert_eq!(mint3.status, 200, "body={}", mint3.body);
    let token3 = json(&mint3.body)["token"].as_str().unwrap().to_string();

    // Re-enable (revoke above left Layer 0 ON), spawn, then disable.
    let en2 = http(d.port, "POST", &q_owner("/cli/remote-session/enable"), Some("{}"));
    assert_eq!(en2.status, 200, "body={}", en2.body);
    let spawn3 = http(
        d.port,
        "POST",
        &q_token("/cli/remote-session/shell/spawn", &token3),
        Some("{}"),
    );
    assert_eq!(spawn3.status, 200, "body={}", spawn3.body);
    let sid3 = json(&spawn3.body)["sessionId"].as_str().unwrap().to_string();

    let dis = http(
        d.port,
        "POST",
        &q_owner("/cli/remote-session/disable"),
        Some("{}"),
    );
    assert_eq!(dis.status, 200, "body={}", dis.body);
    let dis_v = json(&dis.body);
    assert_eq!(dis_v["enabled"], false, "body={}", dis.body);
    assert!(
        dis_v["killedSessions"].as_u64().unwrap_or(0) >= 1,
        "disable must kill; body={}",
        dis.body
    );

    let spawn_dis = http(
        d.port,
        "POST",
        &q_token("/cli/remote-session/shell/spawn", &token3),
        Some("{}"),
    );
    assert_eq!(spawn_dis.status, 403, "body={}", spawn_dis.body);
    assert_eq!(
        json(&spawn_dis.body)["error"]["code"],
        "REMOTE_SESSIONS_DISABLED",
        "body={}",
        spawn_dis.body
    );

    let write_dis = format!(
        "/cli/terminal/write?token={}&id={}&message=x&no_submit=true",
        urlenc(&token3),
        urlenc(&sid3)
    );
    let wd = http(d.port, "GET", &write_dis, None);
    assert_eq!(wd.status, 403, "post-disable write; body={}", wd.body);
    assert_eq!(
        json(&wd.body)["error"]["code"],
        "REMOTE_SESSIONS_DISABLED",
        "body={}",
        wd.body
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_non_remote_terminal_path_still_works() {
    // Smoke: normal terminal write path with owner token is unchanged
    // when the session is not remote-bound (missing session → 400, not
    // grant-gate 403).
    let _g = lock();
    let _home = HomeEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;

    let fake_id = uuid::Uuid::new_v4().to_string();
    let path = format!(
        "/cli/terminal/write?token={}&id={}&message=hi&no_submit=true",
        urlenc(OWNER_TOKEN),
        urlenc(&fake_id)
    );
    let r = http(d.port, "GET", &path, None);
    // Not a remote session → gate passthrough → session not found 400.
    assert_eq!(
        r.status, 400,
        "non-remote missing session should 400 not grant-403; body={}",
        r.body
    );
    assert!(
        !r.body.contains("REMOTE_SESSIONS") && !r.body.contains("NO_GRANT"),
        "must not be remote-session denial; body={}",
        r.body
    );
}
