//! CI on K2 Compute as a permission card (0.46.17, `ci.preview`): the end
//! to end an agent and a person walk, on a real in-process daemon (temp
//! `$HOME`, loopback).
//!
//! With "Permission cards (Beta)" on for the agent and compute on:
//! - every `k2 ci` route answers 409 `ci_preview_off`, and the refusal
//!   carries the exact `ci.preview` ask (the owner's);
//! - the agent's own `POST /cli/settings/update {"ciPreview":true}` is
//!   refused and carries the same ask; `{"ciPreview":false}` can't be
//!   requested;
//! - a Member and an Admin can't allow the card; the owner can, and then
//!   `ciPreview` is on and `k2 ci` no longer answers `ci_preview_off`;
//! - the `ci.preview-off` twin turns it off again, for the owner;
//! - the settings route keeps its POST-only guard (405 on GET).
//!
//! Both doors an agent's `k2` uses: TCP and its cell socket.
//!
//! Its own binary: it flips the process-wide switch. No real `~/.k2`,
//! keychain or running daemon is touched.
// The cell door runs over the per-cell Unix socket (cell_server::serve_cell).
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_core::connect_users::Role;
use k2_core::db::schema::WorkspaceSession;
use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const OWNER: &str = "owner-token-permission-ci-preview";
const ASK: &str = "k2 permission request ci.preview --why \"<what you need it for>\"";

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {} {}", self.status, self.body))
    }
    fn code(&self) -> String {
        let v = self.json();
        match &v["error"] {
            Value::Object(e) => e.get("code").and_then(Value::as_str).unwrap_or_else(|| panic!("no error.code: {} {}", self.status, self.body)).to_string(),
            Value::String(s) => s.clone(),
            _ => panic!("no error: {} {}", self.status, self.body),
        }
    }
}

fn tcp_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"),
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
    let text = String::from_utf8_lossy(&out).to_string();
    let line = text.lines().next().unwrap_or_else(|| panic!("no status line: {text:?}"));
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("bad status: {line}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

async fn call(port: u16, method: &str, path: &str, token: &str, body: Option<Value>) -> Resp {
    let sep = if path.contains('?') { '&' } else { '?' };
    let pq = format!("{path}{sep}token={}", enc(token));
    let method = method.to_string();
    let body = body.map(|b| b.to_string());
    tokio::task::spawn_blocking(move || tcp_blocking(port, &method, &pq, body)).await.unwrap()
}

async fn post(port: u16, path: &str, token: &str, body: Value) -> Resp {
    call(port, "POST", path, token, Some(body)).await
}

struct Home {
    home: PathBuf,
    prev: Option<std::ffi::OsString>,
}
impl Drop for Home {
    fn drop(&mut self) {
        k2_core::permission::gate::set_server_switch(false);
        k2_core::compute::set_ci_enabled(false);
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        std::env::remove_var("K2_COMPUTE");
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

struct Ws {
    id: String,
    handle: String,
}

fn workspace(home: &Path, handle: &str) -> Ws {
    let path = home.join("ws").join(handle);
    std::fs::create_dir_all(&path).unwrap();
    let path = path.to_string_lossy().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", rusqlite::params![id, handle, path]).unwrap();
    conn.execute("UPDATE projects SET agents_can_use_compute = 1 WHERE id = ?1", rusqlite::params![id]).unwrap();
    // A pinned chat: the card goes on its Thread.
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").unwrap();
    Ws { id, handle: handle.to_string() }
}

fn login(user: &str, role: Role) -> String {
    k2_core::connect_users::add_user(user, "password-for-pcip-int-9").expect("add_user");
    if role != Role::Member {
        k2_core::connect_users::set_role(user, role).expect("set_role");
    }
    k2_core::connect_users::create_session(user)
}

/// The agent's passport and its cell socket (served).
fn passport(ws: &Ws) -> (String, PathBuf) {
    let sid = SessionId::new();
    let token = k2_daemon::session_token::mint_session_token(
        &sid,
        "pane-pcip",
        HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: ws.handle.clone() },
        CredMode::ApiKey,
        Provider::Anthropic,
    );
    let listener = k2_daemon::cell_uds::bind_cell_socket(&sid).expect("bind cell socket");
    let sock = k2_daemon::cell_uds::cell_socket_path(&sid);
    k2_daemon::cell_server::serve_cell(sid, listener, None);
    (token, sock)
}

/// One request on the agent's cell socket, as its `k2` sends it.
async fn cell(sock: &Path, method: &str, path: &str, bearer: &str, body: Option<Value>) -> Resp {
    let raw = match body.map(|b| b.to_string()) {
        Some(b) => format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {bearer}\r\n\r\n"),
    };
    let mut stream = UnixStream::connect(sock).await.expect("connect cell socket");
    stream.write_all(raw.as_bytes()).await.expect("write");
    stream.flush().await.expect("flush");
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("{method} {path} on the cell: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let line = text.lines().next().unwrap_or_else(|| panic!("no status line: {text:?}"));
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("bad status: {line}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

async fn ci_preview_now(port: u16) -> bool {
    let r = call(port, "GET", "/cli/settings/get", OWNER, None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["ciPreview"].as_bool().unwrap_or_else(|| panic!("no ciPreview in settings: {}", r.body))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ci_preview_card_turns_ci_on_for_the_owner_only() {
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2pcip{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let _home = Home { prev: std::env::var_os("HOME"), home: home.clone() };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");
    let d = test_harness::start(OWNER).await;
    let port = d.port;

    let admin = login("pcipadmin", Role::Admin);
    let member = login("pcipmember", Role::Member);
    let ws = workspace(&home, "pcip-builder");
    let (agent, sock) = passport(&ws);
    tokio::time::sleep(Duration::from_millis(80)).await;
    let r = post(port, "/cli/settings/update", OWNER, json!({ "permissionCards": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/agent-access/set", OWNER, json!({ "toggle": "permission_cards", "workspace": ws.id, "value": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!ci_preview_now(port).await, "off by default");

    // ── off: every CI door names the owner's card ───────────────────────
    for (method, path, body) in [("POST", "/cli/ci/run", Some(json!({}))), ("GET", "/cli/ci/status", None)] {
        let r = call(port, method, path, &agent, body).await;
        assert_eq!((r.status, r.code().as_str()), (409, "ci_preview_off"), "{path}: {}", r.body);
        let v = r.json();
        assert_eq!(v["permission"]["command"], ASK, "{path}: {v}");
        assert_eq!(v["permission"]["approvers"], "owner", "{path}: {v}");
        assert!(v["error"]["hint"].as_str().unwrap_or("").contains("Settings → Compute → CI on K2 Compute"), "{v}");
    }
    // The owner sees the plain refusal: no card line for people.
    let r = post(port, "/cli/ci/run", OWNER, json!({})).await;
    assert_eq!((r.status, r.code().as_str()), (409, "ci_preview_off"), "{}", r.body);
    assert!(r.json().get("permission").is_none(), "{}", r.body);

    // The agent's own `k2 settings set ciPreview true`: refused, with the ask.
    let r = post(port, "/cli/settings/update", &agent, json!({ "ciPreview": true })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.json()["permission"]["command"], ASK, "{}", r.body);
    let r = post(port, "/cli/settings/update", &agent, json!({ "ciPreview": false })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.json()["permission"]["requestable"], false, "{}", r.body);
    // The cell socket (what an agent's `k2` uses first): the same lines.
    let r = cell(&sock, "POST", "/cli/settings/update", &agent, Some(json!({ "ciPreview": true }))).await;
    assert_eq!((r.status, r.code().as_str()), (403, "owner_only"), "{}", r.body);
    assert_eq!(r.json()["permission"]["command"], ASK, "{}", r.body);
    let r = cell(&sock, "POST", "/cli/settings/update", &agent, Some(json!({ "computePreview": true }))).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.json()["permission"]["requestable"], false, "{}", r.body);
    let r = cell(&sock, "POST", "/cli/ci/run", &agent, Some(json!({}))).await;
    assert_eq!((r.status, r.code().as_str()), (409, "ci_preview_off"), "{}", r.body);
    assert_eq!(r.json()["permission"]["command"], ASK, "{}", r.body);
    assert!(!ci_preview_now(port).await, "the agent wrote nothing");

    // ── the card, as the printed line sends it ──────────────────────────
    let r = post(port, "/cli/permission/request", &agent, json!({ "action": "ci.preview", "params": {}, "why": "check my pushes" })).await;
    assert_eq!((r.status, r.json()["status"].as_str()), (200, Some("pending")), "{}", r.body);
    let card = r.json()["card"].as_str().unwrap_or_else(|| panic!("no card: {}", r.body)).to_string();
    let answer = |who: String, card: String| {
        let body = json!({ "addr": ws.handle, "id": card, "answer": "Allow" });
        async move { post(port, "/cli/thread/answer", &who, body).await }
    };
    for who in [member.clone(), admin.clone()] {
        let r = answer(who, card.clone()).await;
        assert_eq!((r.status, r.code().as_str()), (403, "action_card_person_only"), "{}", r.body);
    }
    let r = answer(agent.clone(), card.clone()).await;
    assert_eq!(r.status, 403, "the agent answers its own card: {}", r.body);
    assert!(!ci_preview_now(port).await, "nobody below the owner turned it on");
    let r = answer(OWNER.to_string(), card.clone()).await;
    assert_eq!((r.status, r.json()["action"]["result"]["outcome"].as_str()), (200, Some("approved")), "{}", r.body);
    assert!(ci_preview_now(port).await, "the owner's Allow turned ciPreview on");

    // On: `k2 ci` answers something else now (no repo here), never the card.
    let r = post(port, "/cli/ci/run", &agent, json!({})).await;
    assert!(!r.body.contains("ci_preview_off") && !r.body.contains("ci.preview"), "{} {}", r.status, r.body);

    // ── the off twin, for the owner ─────────────────────────────────────
    let r = post(port, "/cli/permission/request", &agent, json!({ "action": "ci.preview-off", "params": {}, "why": "done with CI" })).await;
    assert_eq!((r.status, r.json()["status"].as_str()), (200, Some("pending")), "{}", r.body);
    let off_card = r.json()["card"].as_str().expect("card").to_string();
    let r = answer(admin.clone(), off_card.clone()).await;
    assert_eq!((r.status, r.code().as_str()), (403, "action_card_person_only"), "{}", r.body);
    let r = answer(OWNER.to_string(), off_card).await;
    assert_eq!((r.status, r.json()["action"]["result"]["outcome"].as_str()), (200, Some("approved")), "{}", r.body);
    assert!(!ci_preview_now(port).await, "the twin turned it off");
    let r = post(port, "/cli/ci/run", &agent, json!({})).await;
    assert_eq!((r.status, r.code().as_str()), (409, "ci_preview_off"), "{}", r.body);

    // The settings route stays POST-only.
    let r = call(port, "GET", "/cli/settings/update", OWNER, None).await;
    assert_eq!(r.status, 405, "{}", r.body);
    drop(d);
}
