//! Real sockets against a real daemon, for the live access tests: open, watch what
//! arrives, cause frames.

#![allow(dead_code)]

use std::time::Duration;

use super::app_fixture::*;
use futures_util::StreamExt;
use k2_core::db::schema::WorkspaceSession;
use k2_daemon::session_events::{self, SessionEvent};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

pub const PW: &str = "s3cret-horse-staple";
pub const LOGIN_PW: &str = "correct-horse-battery-staple-9";
/// A socket's idle close is a 5 s tick away; a little room for a busy node.
pub const CLOSE_WITHIN_MS: u64 = 7_000;

pub type Sock = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, Default)]
pub struct Seen {
    pub frames: Vec<String>,
    pub close: Option<u16>,
    pub ended: bool,
}

impl Seen {
    pub fn has(&self, needle: &str) -> bool {
        self.frames.iter().any(|f| f.contains(needle))
    }
}

/// What arrives within `ms`, or until the socket ends.
pub async fn watch(sock: &mut Sock, ms: u64) -> Seen {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    let mut out = Seen::default();
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, sock.next()).await {
            Err(_) => return out,
            Ok(None) | Ok(Some(Err(_))) => {
                out.ended = true;
                return out;
            }
            Ok(Some(Ok(Message::Text(t)))) => out.frames.push(t.to_string()),
            Ok(Some(Ok(Message::Close(c)))) => {
                out.close = c.map(|c| u16::from(c.code));
                out.ended = true;
                return out;
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

/// The next text frame within `ms`. `Ok(None)`: nothing came. `Err`: the socket
/// ended, with its close code if it sent one.
pub async fn next_frame(sock: &mut Sock, ms: u64) -> Result<Option<String>, Option<u16>> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, sock.next()).await {
            Err(_) => return Ok(None),
            Ok(None) | Ok(Some(Err(_))) => return Err(None),
            Ok(Some(Ok(Message::Text(t)))) => return Ok(Some(t.to_string())),
            Ok(Some(Ok(Message::Close(c)))) => return Err(c.map(|c| u16::from(c.code))),
            Ok(Some(Ok(_))) => {}
        }
    }
}

pub async fn open(port: u16, pq: &str) -> Sock {
    let url = format!("ws://127.0.0.1:{port}{pq}");
    let (s, _) = tokio_tungstenite::connect_async(&url).await.unwrap_or_else(|e| panic!("open {pq}: {e}"));
    s
}

pub fn pin_chat(ws: &Ws) -> String {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    conv
}

pub async fn say(port: u16, ws: &Ws, text: &str) {
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": ws.handle, "from": "k2", "text": text })).await;
    assert_eq!(r.status, 200, "post: {}", r.body);
}

pub fn ticket(room: &str, id: &str) {
    session_events::emit(SessionEvent::TicketChanged {
        project_id: room.to_string(),
        id: id.to_string(),
        change: "answered".into(),
        status: "answered".into(),
        via: None,
        has_brief: false,
    })
    .expect("emit");
}

pub fn file_changed(ws: &Ws, rel: &str) {
    let root = ws.path.to_string_lossy().to_string();
    session_events::emit(SessionEvent::FsChanged { workspace_path: root.clone(), paths: vec![format!("{root}/{rel}")] }).expect("emit");
}

pub async fn owner_ok(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

pub const ALL_CAPS: &[&str] = &["thread:read", "thread:post", "activity:read", "tickets:read", "files:read", "room:members", "heartbeats:read"];

/// One guest on its own role in `ws`. Returns `(role, pass)`.
pub async fn guest_in(port: u16, ws: &Ws, username: &str, caps: &[&str]) -> (String, String) {
    let role = format!("role-{username}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    owner_ok(port, "/cli/skin/users", json!({ "username": username })).await;
    k2_daemon::test_harness::guest_with_own_password(username, PW);
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.expect("join");
    assert_eq!(r.status, 200, "guest login: {}", r.body);
    (role, r.json()["token"].as_str().expect("token").to_string())
}


pub async fn project(port: u16, name: &str, members: &[&Ws]) -> String {
    let g = owner_ok(port, "/cli/project-group/create", json!({ "name": name })).await;
    let id = g["id"].as_str().expect("group id").to_string();
    for m in members {
        owner_ok(port, "/cli/project-group/add-member", json!({ "group": id, "workspace": m.id })).await;
    }
    id
}

