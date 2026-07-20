//! Hosted web client Layer 0 — `web_client_enabled` owner wall (PRD §6.7 / §9.4).
//!
//! Contract under test (real harness, process-isolated HOME):
//!
//!   1. Fresh daemon: `/boot-status` → 200 with `webClient.enabled: true`
//!      (product default ON).
//!   2. Query-token `/cli/*` without web signals still works when ON.
//!   3. Owner sets `webClientEnabled: false` via settings update.
//!   4. Web-header request (`X-K2-Client: web`) to `/cli/*` → 403
//!      `WEB_CLIENT_DISABLED` (distinct from `REMOTE_SESSIONS_DISABLED`).
//!   5. Unauthenticated `/boot-status` still 200 and reports
//!      `webClient.enabled: false` (loader must not look dead).
//!   6. Query-token `/cli/*` without web signals still works when OFF
//!      (CLI/desktop path unchanged).
//!
//! Isolation mirrors `remote_session_layer0_integration.rs`: TEST_LOCK +
//! temp HOME.

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

const OWNER_TOKEN: &str = "owner-token-deadbeef-web-client-l0";

struct Resp {
    status: u16,
    body: String,
}

/// Raw HTTP with optional extra headers (one `Name: value` per entry).
fn http(
    port: u16,
    method: &str,
    path_and_query: &str,
    body: Option<&str>,
    extra_headers: &[&str],
) -> Resp {
    let mut stream =
        StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    let mut extra = String::new();
    for h in extra_headers {
        extra.push_str(h);
        extra.push_str("\r\n");
    }
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             {extra}\r\n{b}",
            b.len()
        ),
        None => format!(
            "{method} {path_and_query} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             {extra}\r\n"
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
            "k2-web-client-l0-{}-{nanos}",
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
async fn web_client_layer0_default_on_off_wall_boot_status() {
    let _g = lock();
    let _home = HomeEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;

    // 1) Default: boot-status advertises webClient.enabled = true.
    let bs = http(d.port, "GET", "/boot-status", None, &[]);
    assert_eq!(
        bs.status, 200,
        "boot-status must be 200 unauthenticated; body={}",
        bs.body
    );
    let bv = json(&bs.body);
    assert_eq!(
        bv["webClient"]["enabled"], true,
        "webClient must default ON; body={}",
        bs.body
    );

    // 2) Query-token CLI path works while ON (no web header).
    let who = http(d.port, "GET", &q("/cli/settings/get"), None, &[]);
    assert_eq!(
        who.status, 200,
        "settings/get with owner token must work; body={}",
        who.body
    );
    let sv = json(&who.body);
    assert_eq!(
        sv["webClientEnabled"], true,
        "settings must report webClientEnabled true by default; body={}",
        who.body
    );

    // 3) Owner flips OFF via settings update (query-token, not web).
    let off = http(
        d.port,
        "POST",
        &q("/cli/settings/update"),
        Some(r#"{"webClientEnabled":false}"#),
        &[],
    );
    assert_eq!(
        off.status, 200,
        "owner must be able to disable web client; body={}",
        off.body
    );
    let ov = json(&off.body);
    assert_eq!(
        ov["webClientEnabled"], false,
        "update must persist OFF; body={}",
        off.body
    );

    // 4) Web-header request → 403 WEB_CLIENT_DISABLED.
    let web_denied = http(
        d.port,
        "GET",
        &q("/cli/settings/get"),
        None,
        &["X-K2-Client: web"],
    );
    assert_eq!(
        web_denied.status, 403,
        "web-header request while OFF must 403; body={}",
        web_denied.body
    );
    let denied = json(&web_denied.body);
    assert_eq!(denied["ok"], false, "body={}", web_denied.body);
    assert_eq!(
        denied["error"]["code"], "WEB_CLIENT_DISABLED",
        "distinct Layer 0 code required; body={}",
        web_denied.body
    );
    assert_ne!(
        denied["error"]["code"], "REMOTE_SESSIONS_DISABLED",
        "must not reuse remote-session code; body={}",
        web_denied.body
    );
    let hint = denied["error"]["hint"]
        .as_str()
        .unwrap_or_else(|| panic!("hint missing: {}", web_denied.body));
    assert!(
        hint.to_ascii_lowercase().contains("web")
            || hint.contains("webClientEnabled"),
        "hint should teach re-enable: {hint}"
    );

    // 5) /boot-status still 200 and reports enabled:false.
    let bs2 = http(d.port, "GET", "/boot-status", None, &[]);
    assert_eq!(
        bs2.status, 200,
        "boot-status must stay up when wall is OFF; body={}",
        bs2.body
    );
    let bv2 = json(&bs2.body);
    assert_eq!(
        bv2["webClient"]["enabled"], false,
        "boot-status must advertise wall OFF; body={}",
        bs2.body
    );
    assert_eq!(
        bv2["phase"], "ready",
        "phase must stay ready so loader is not dead; body={}",
        bs2.body
    );

    // 6) CLI query-token without web signals still works when OFF.
    let cli_ok = http(d.port, "GET", &q("/cli/settings/get"), None, &[]);
    assert_eq!(
        cli_ok.status, 200,
        "CLI path must remain open when web wall is OFF; body={}",
        cli_ok.body
    );
    let cv = json(&cli_ok.body);
    assert_eq!(
        cv["webClientEnabled"], false,
        "settings still readable via CLI; body={}",
        cli_ok.body
    );

    // 7) Re-enable via CLI; web-header request succeeds again.
    let on = http(
        d.port,
        "POST",
        &q("/cli/settings/update"),
        Some(r#"{"webClientEnabled":true}"#),
        &[],
    );
    assert_eq!(on.status, 200, "re-enable; body={}", on.body);
    let web_ok = http(
        d.port,
        "GET",
        &q("/cli/settings/get"),
        None,
        &["X-K2-Client: web"],
    );
    assert_eq!(
        web_ok.status, 200,
        "web-header request after re-enable must pass; body={}",
        web_ok.body
    );
}
