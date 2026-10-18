//! homes-sync-v1: an agent's own cell socket never reaches a person's
//! Homes. `/cli/me/homes*` is not on the scoped-token capability allowlist
//! (`session_token::require_hook`), so every method on every Homes path is
//! 403 before any handler runs, and nothing is written.

use std::path::PathBuf;
use std::sync::Mutex;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use k2_core::session::SessionId;
use k2_daemon::cell_server;
use k2_daemon::cell_uds;
use k2_daemon::session_token::{self, HookPrincipal};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A SHORT `$HOME` under `/tmp` (a Unix socket path is capped near 104
/// bytes on macOS).
fn set_short_home() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = PathBuf::from(format!("/tmp/k2hs{}{}", std::process::id(), nanos % 1_000_000));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();
    tmp
}

async fn uds(sock: &std::path::Path, raw: &str) -> (u16, String) {
    let mut stream = UnixStream::connect(sock).await.expect("connect cell socket");
    stream.write_all(raw.as_bytes()).await.expect("write request");
    stream.flush().await.ok();
    let mut out = Vec::new();
    let _ = stream.read_to_end(&mut out).await;
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status line: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agent_cell_socket_refuses_every_homes_route() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    let home = set_short_home();
    let sid = SessionId::new();
    let token = session_token::mint_session_token(
        &sid,
        "hs-pane",
        HookPrincipal { workspace_uuid: String::new(), agent_address: "hs-agent".to_string() },
        session_token::CredMode::ApiKey,
        session_token::Provider::Anthropic,
    );
    let listener = cell_uds::bind_cell_socket(&sid).expect("bind cell socket");
    let sock = cell_uds::cell_socket_path(&sid);
    cell_server::serve_cell(sid, listener, None);
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    // The same token IS good on this socket for an agent verb (so the 403s
    // below are about the Homes paths, not a broken token).
    let raw = format!("GET /cli/thread?addr=nobody HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\r\n");
    let (status, body) = uds(&sock, &raw).await;
    assert_ne!(status, 403, "the agent token works on its own socket: {body}");

    for (method, path, body) in [
        ("GET", "/cli/me/homes?deviceId=x", ""),
        ("GET", "/cli/me/homes/devices", ""),
        ("POST", "/cli/me/homes/push", r#"{"deviceId":"x","changes":[]}"#),
        ("POST", "/cli/me/homes/devices/register", r#"{"deviceId":"x","name":"x","platform":"ios"}"#),
        ("POST", "/cli/me/homes/devices/remove", r#"{"deviceId":"x"}"#),
        ("GET", "/cli/me/homes/push", ""),
    ] {
        let raw = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let (status, resp) = uds(&sock, &raw).await;
        assert_eq!(status, 403, "{method} {path}: {resp}");
        assert!(!resp.contains("changes") && !resp.contains("devices"), "{method} {path} leaked: {resp}");
    }
    // Nothing was registered for anyone.
    let db = k2_core::db::shared();
    let n: i64 = db.lock().query_row("SELECT COUNT(*) FROM homes_devices", [], |r| r.get(0)).expect("count");
    assert_eq!(n, 0);
    let _ = std::fs::remove_dir_all(&home);
}
