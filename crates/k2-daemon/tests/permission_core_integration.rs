//! Permission cards lane A1 (prd-agent-settings-cards-v1 rev 3.1 §2.4,
//! §3.2): the `/cli/permission/*` arms on a real daemon, over TCP and an
//! agent's cell socket, in every switch state.
//!
//! | server | agent | a passport gets |
//! |---|---|---|
//! | off | – | today's unknown-route answer (TCP arm skipped, cell 403) |
//! | on | off | today's body for its transport and method |
//! | on | on | served |
//!
//! Its own binary: it flips the process-wide switch (lane B's rule). The
//! release catalog may be empty on this branch (E-lanes register actions),
//! so "served" is shown by the family's own answers (`enabled`, `bad_action`,
//! `list`); the request → card → answer path is in the lib's core tests.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const OWNER: &str = "owner-token-permission-core";
const TODAY_403: &str = r#"{"error":"Invalid or missing auth token"}"#;
const TODAY_405: &str = r#"{"error":"method not allowed for this route"}"#;

struct Resp {
    status: u16,
    body: String,
}

fn parse(text: &str) -> Resp {
    let line = text.lines().next().unwrap_or_else(|| panic!("no status line: {text:?}"));
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("bad status: {line}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn tcp_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("{method} {pq}: {e}"),
        }
    }
    parse(&String::from_utf8_lossy(&out))
}

async fn tcp(port: u16, method: &str, path: &str, token: &str, body: Option<&str>) -> Resp {
    let pq = format!("{path}?token={token}");
    let (method, body) = (method.to_string(), body.map(str::to_string));
    tokio::task::spawn_blocking(move || tcp_blocking(port, &method, &pq, body)).await.unwrap()
}

async fn cell(sock: &Path, method: &str, path: &str, bearer: &str, body: Option<&str>) -> Resp {
    let raw = match body {
        Some(b) => format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {bearer}\r\n\r\n"),
    };
    let mut stream = UnixStream::connect(sock).await.expect("connect cell socket");
    stream.write_all(raw.as_bytes()).await.expect("write");
    stream.flush().await.ok();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    parse(&String::from_utf8_lossy(&buf))
}

fn json(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {} {}", r.status, r.body))
}

struct Home {
    home: PathBuf,
    prev: Option<std::ffi::OsString>,
}
impl Drop for Home {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

const ASK: &str = r#"{"action":"no.such-action","why":"Run the suite on the minis"}"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_family_answers_by_switch_state_on_tcp_and_the_cell() {
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = PathBuf::from(format!("/tmp/k2pci{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = std::fs::canonicalize(&home).unwrap();
    let _home = Home { prev: std::env::var_os("HOME"), home: home.clone() };
    std::env::set_var("HOME", &home);

    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let ws_path = home.join("ws-pci-billing");
    std::fs::create_dir_all(&ws_path).unwrap();
    let ws_id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'pci-billing', ?2, 'pci-billing')",
            rusqlite::params![ws_id, ws_path.to_string_lossy()],
        )
        .unwrap();
    }
    let sid = SessionId::new();
    let passport = k2_daemon::session_token::mint_session_token(
        &sid,
        "pane-pci",
        HookPrincipal { workspace_uuid: ws_id.clone(), agent_address: "pci-billing".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    );
    let listener = k2_daemon::cell_uds::bind_cell_socket(&sid).expect("bind cell socket");
    let sock = k2_daemon::cell_uds::cell_socket_path(&sid);
    k2_daemon::cell_server::serve_cell(sid, listener, None);
    tokio::time::sleep(Duration::from_millis(80)).await;

    // ── server off: today's unknown-route answers ─────────────────────
    assert!(!k2_core::permission::gate::server_on());
    let r = cell(&sock, "POST", "/cli/permission/request", &passport, Some(ASK)).await;
    assert_eq!((r.status, r.body.as_str()), (403, TODAY_403));
    let r = tcp(port, "POST", "/cli/permission/request", &passport, Some(ASK)).await;
    assert_eq!((r.status, r.body.as_str()), (405, TODAY_405));
    let r = tcp(port, "GET", "/cli/permission/enabled", OWNER, None).await;
    assert_eq!((r.status, r.body.as_str()), (404, r#"{"error":"route not found"}"#), "owner token, server off");

    // ── server on, agent off: today's body for the agent; people served ─
    let r = tcp(port, "POST", "/cli/settings/update", OWNER, Some(r#"{"permissionCards":true}"#)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(k2_core::permission::gate::server_on());
    for (method, body) in [("POST", Some(ASK)), ("GET", None)] {
        let path = if method == "POST" { "/cli/permission/request" } else { "/cli/permission/enabled" };
        let r = cell(&sock, method, path, &passport, body).await;
        assert_eq!((r.status, r.body.as_str()), (403, TODAY_403), "cell {method} {path}");
    }
    let r = tcp(port, "POST", "/cli/permission/request", &passport, Some(ASK)).await;
    assert_eq!((r.status, r.body.as_str()), (405, TODAY_405), "TCP POST, agent off");
    let r = tcp(port, "GET", "/cli/permission/enabled", &passport, None).await;
    assert_eq!((r.status, r.body.as_str()), (403, TODAY_403), "TCP GET, agent off");
    let r = tcp(port, "GET", "/cli/permission/enabled", OWNER, None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(json(&r)["enabled"], true);
    let r = tcp(port, "POST", "/cli/permission/request", OWNER, Some(ASK)).await;
    assert_eq!((r.status, json(&r)["error"]["code"].as_str()), (403, Some("forbidden")), "an agent verb: {}", r.body);

    // ── both on: served on both doors ─────────────────────────────────
    let r = tcp(
        port,
        "POST",
        "/cli/agent-access/set",
        OWNER,
        Some(r#"{"toggle":"permission_cards","workspace":"pci-billing","value":true}"#),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(k2_core::permission::gate::enabled_for(&ws_id));
    let r = cell(&sock, "GET", "/cli/permission/enabled", &passport, None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(json(&r)["agent"], "pci-billing");
    let r = cell(&sock, "POST", "/cli/permission/request", &passport, Some(ASK)).await;
    assert_eq!((r.status, json(&r)["error"]["code"].as_str()), (400, Some("bad_action")), "{}", r.body);
    let r = tcp(port, "POST", "/cli/permission/request", &passport, Some(ASK)).await;
    assert_eq!((r.status, json(&r)["error"]["code"].as_str()), (400, Some("bad_action")), "{}", r.body);
    let r = cell(&sock, "POST", "/cli/permission/request", &passport, Some(r#"{"action":"no.such-action"}"#)).await;
    assert_eq!(r.status, 400, "{}", r.body);
    let r = cell(&sock, "GET", "/cli/permission/request", &passport, None).await;
    assert_eq!((r.status, json(&r)["error"]["code"].as_str()), (405, Some("method_not_allowed")), "{}", r.body);
    let r = cell(&sock, "GET", "/cli/permission/list", &passport, None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(json(&r)["agent"], "pci-billing");
    assert!(json(&r)["actions"].is_array());
    let r = cell(&sock, "GET", "/cli/permission/status?id=pr_abcdef", &passport, None).await;
    assert_eq!(r.status, 404, "{}", r.body);

    // ── server off again (agent on): today's answers ──────────────────
    let r = tcp(port, "POST", "/cli/settings/update", OWNER, Some(r#"{"permissionCards":false}"#)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = cell(&sock, "POST", "/cli/permission/request", &passport, Some(ASK)).await;
    assert_eq!((r.status, r.body.as_str()), (403, TODAY_403));
    let r = tcp(port, "POST", "/cli/permission/request", &passport, Some(ASK)).await;
    assert_eq!((r.status, r.body.as_str()), (405, TODAY_405));
    drop(d);
}
