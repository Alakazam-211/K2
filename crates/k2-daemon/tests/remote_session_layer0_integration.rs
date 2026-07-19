//! Remote Session Layer 0 — hard wall (default OFF) + denial audit.
//!
//! Contract under test (real harness, process-isolated HOME):
//!
//!   1. Fresh daemon: `GET /cli/remote-session/status` → enabled:false
//!   2. `POST /cli/remote-session/shell/spawn` while OFF → 403
//!      `REMOTE_SESSIONS_DISABLED` + recentDenials non-empty on status
//!   3. enable → status enabled true
//!   4. shell/spawn while ON → NOT REMOTE_SESSIONS_DISABLED (NO_GRANT OK)
//!   5. disable works
//!
//! Isolation mirrors `api_gate_integration.rs`: TEST_LOCK + temp HOME.

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

const OWNER_TOKEN: &str = "owner-token-deadbeef-remote-session-l0";

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
            "k2-remote-session-l0-{}-{nanos}",
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

fn q(path: &str) -> String {
    format!("{path}?token={OWNER_TOKEN}")
}

#[tokio::test(flavor = "multi_thread")]
async fn layer0_default_off_denial_enable_spawn_disable() {
    let _g = lock();
    let _home = HomeEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;

    // 1) Default status: enabled false, empty denials.
    let status = http(d.port, "GET", &q("/cli/remote-session/status"), None);
    assert_eq!(
        status.status, 200,
        "status must be 200 for owner; body={}",
        status.body
    );
    let v = json(&status.body);
    assert_eq!(v["ok"], true, "body={}", status.body);
    assert_eq!(
        v["enabled"], false,
        "remote sessions must default OFF; body={}",
        status.body
    );
    assert_eq!(
        v["activeGrants"], 0,
        "no grants in stage 1; body={}",
        status.body
    );
    assert!(
        v["activeSessions"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "activeSessions must be []; body={}",
        status.body
    );
    assert!(
        v["recentDenials"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "recentDenials must start empty; body={}",
        status.body
    );

    // 2) Spawn while OFF → 403 REMOTE_SESSIONS_DISABLED.
    let spawn_off = http(
        d.port,
        "POST",
        &q("/cli/remote-session/shell/spawn"),
        Some("{}"),
    );
    assert_eq!(
        spawn_off.status, 403,
        "spawn while OFF must 403; body={}",
        spawn_off.body
    );
    let denied = json(&spawn_off.body);
    assert_eq!(denied["ok"], false, "body={}", spawn_off.body);
    assert_eq!(
        denied["error"]["code"], "REMOTE_SESSIONS_DISABLED",
        "distinct Layer 0 code required; body={}",
        spawn_off.body
    );
    let hint = denied["error"]["hint"]
        .as_str()
        .unwrap_or_else(|| panic!("hint missing: {}", spawn_off.body));
    assert!(
        hint.contains("Remote Sessions are OFF") || hint.contains("remote-session enable"),
        "hint must teach enable path; hint={hint}"
    );

    // Status after denial: recentDenials non-empty with the same code.
    let status2 = http(d.port, "GET", &q("/cli/remote-session/status"), None);
    assert_eq!(status2.status, 200, "body={}", status2.body);
    let v2 = json(&status2.body);
    assert_eq!(v2["enabled"], false, "still OFF; body={}", status2.body);
    let denials = v2["recentDenials"]
        .as_array()
        .unwrap_or_else(|| panic!("recentDenials must be array; body={}", status2.body));
    assert!(
        !denials.is_empty(),
        "denial must be persisted; body={}",
        status2.body
    );
    assert_eq!(
        denials[0]["code"], "REMOTE_SESSIONS_DISABLED",
        "first denial code; body={}",
        status2.body
    );
    assert_eq!(
        denials[0]["kind"], "denial",
        "kind must be denial; body={}",
        status2.body
    );
    assert_eq!(
        denials[0]["principalLabel"], "owner",
        "owner token label; body={}",
        status2.body
    );

    // 3) enable → status enabled true.
    let en = http(d.port, "POST", &q("/cli/remote-session/enable"), Some("{}"));
    assert_eq!(en.status, 200, "enable must 200; body={}", en.body);
    let en_v = json(&en.body);
    assert_eq!(en_v["ok"], true, "body={}", en.body);
    assert_eq!(en_v["enabled"], true, "body={}", en.body);

    let status3 = http(d.port, "GET", &q("/cli/remote-session/status"), None);
    assert_eq!(status3.status, 200, "body={}", status3.body);
    let v3 = json(&status3.body);
    assert_eq!(
        v3["enabled"], true,
        "status must reflect enable; body={}",
        status3.body
    );

    // 4) Spawn while ON → NOT REMOTE_SESSIONS_DISABLED (NO_GRANT is OK).
    let spawn_on = http(
        d.port,
        "POST",
        &q("/cli/remote-session/shell/spawn"),
        Some("{}"),
    );
    assert_eq!(
        spawn_on.status, 403,
        "stage-1 spawn while ON still 403 (no grant); body={}",
        spawn_on.body
    );
    let on_v = json(&spawn_on.body);
    assert_eq!(on_v["ok"], false, "body={}", spawn_on.body);
    let code = on_v["error"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("code missing: {}", spawn_on.body));
    assert_ne!(
        code, "REMOTE_SESSIONS_DISABLED",
        "Layer 0 must not fire when enabled; body={}",
        spawn_on.body
    );
    assert_eq!(
        code, "NO_GRANT",
        "stage-1 expected NO_GRANT when ON without grant; body={}",
        spawn_on.body
    );

    // 5) disable works.
    let dis = http(d.port, "POST", &q("/cli/remote-session/disable"), Some("{}"));
    assert_eq!(dis.status, 200, "disable must 200; body={}", dis.body);
    let dis_v = json(&dis.body);
    assert_eq!(dis_v["ok"], true, "body={}", dis.body);
    assert_eq!(dis_v["enabled"], false, "body={}", dis.body);
    assert_eq!(
        dis_v["killedSessions"], 0,
        "stage-1 no live sessions; body={}",
        dis.body
    );

    let status4 = http(d.port, "GET", &q("/cli/remote-session/status"), None);
    assert_eq!(status4.status, 200, "body={}", status4.body);
    let v4 = json(&status4.body);
    assert_eq!(
        v4["enabled"], false,
        "status must reflect disable; body={}",
        status4.body
    );

    // Spawn after disable is REMOTE_SESSIONS_DISABLED again.
    let spawn_again = http(
        d.port,
        "POST",
        &q("/cli/remote-session/shell/spawn"),
        Some("{}"),
    );
    assert_eq!(spawn_again.status, 403, "body={}", spawn_again.body);
    let again = json(&spawn_again.body);
    assert_eq!(
        again["error"]["code"], "REMOTE_SESSIONS_DISABLED",
        "disable restores Layer 0 wall; body={}",
        spawn_again.body
    );
}
