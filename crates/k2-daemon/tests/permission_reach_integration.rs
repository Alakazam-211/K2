//! Permission cards lane A2 (prd-agent-settings-cards-v1 rev 3.1 §2.4,
//! §4.3, §4.4, §17.5, §17.8): the two person routes on a card
//! (`/cli/thread/permission-note`, `/cli/thread/permission-undo`) and
//! "Waiting on you" (`GET /cli/permission/pending`) on a real daemon, over
//! TCP and an agent's cell socket, with the global switch off and on.
//!
//! Its own binary: it flips the process-wide switch (lane B's rule). The
//! release catalog may be empty on this branch (the E-lanes register
//! actions), so the card here is seeded straight into the store; the
//! request → card → push → note → undo path on real actions is in the lib's
//! `permission::notes::reach_tests`.
// The suite runs over the per-cell Unix socket (cell_server::serve_cell); on
// Windows cells reach the same gate over the TCP arm (cell_uds is a stub there).
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_core::db::schema::WorkspaceSession;
use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const OWNER: &str = "owner-token-permission-reach";
const NOTE: &str = "/cli/thread/permission-note";
const UNDO: &str = "/cli/thread/permission-undo";
const PENDING: &str = "/cli/permission/pending";

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

fn code(r: &Resp) -> String {
    json(r)["error"]["code"].as_str().unwrap_or_else(|| panic!("no error code: {} {}", r.status, r.body)).to_string()
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

fn note_rows(request_id: &str) -> Vec<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut st = conn.prepare("SELECT author, text FROM permission_notes WHERE request_id = ?1 ORDER BY author").unwrap();
    let rows = st.query_map(rusqlite::params![request_id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    rows.map(Result::unwrap).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn note_undo_and_waiting_on_you_by_switch_state_and_caller() {
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2pri{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let _home = Home { prev: std::env::var_os("HOME"), home: home.clone() };
    std::env::set_var("HOME", &home);

    let d = test_harness::start(OWNER).await;
    let port = d.port;

    // A workspace with its pinned Chat, an agent cell, three logins.
    let ws_path = home.join("ws-pri-billing");
    std::fs::create_dir_all(&ws_path).unwrap();
    let ws_id = uuid::Uuid::new_v4().to_string();
    let conv = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'pri-billing', ?2, 'pri-billing')",
            rusqlite::params![ws_id, ws_path.to_string_lossy()],
        )
        .unwrap();
        WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws_id, None, Some(&conv), "claude", "system", "running").unwrap();
    }
    let sid = SessionId::new();
    let passport = k2_daemon::session_token::mint_session_token(
        &sid,
        "pane-pri",
        HookPrincipal { workspace_uuid: ws_id.clone(), agent_address: "pri-billing".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    );
    let listener = k2_daemon::cell_uds::bind_cell_socket(&sid).expect("bind cell socket");
    let sock = k2_daemon::cell_uds::cell_socket_path(&sid);
    k2_daemon::cell_server::serve_cell(sid, listener, None);
    tokio::time::sleep(Duration::from_millis(80)).await;
    let mut sessions = std::collections::BTreeMap::new();
    for (user, role) in [
        ("pri-owner", k2_core::connect_users::Role::Owner),
        ("pri-admin", k2_core::connect_users::Role::Admin),
        ("pri-member", k2_core::connect_users::Role::Member),
    ] {
        k2_core::connect_users::add_user(user, "correct-horse-battery-staple-1").expect("add user");
        k2_core::connect_users::set_role(user, role).expect("set role");
        sessions.insert(user, k2_core::connect_users::create_session(user));
    }

    // A waiting Admin-floor permission card, seeded into the store.
    let request_id = format!("pr_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let now = k2_core::overlay::now_secs();
    let card_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let (item, _) = k2_core::overlay::post_choice_with_action(
            &conn,
            &conv,
            &ws_id,
            "pri-billing",
            "pri-billing",
            "pri-billing asks: Let pri-billing use compute nodes (Allow compute nodes).",
            vec!["Allow".into(), "Deny".into()],
            false,
            Some(now + 3600),
            Some(k2_core::overlay::CardAction {
                id: "permission".into(),
                params: json!({ "request": request_id }),
                dedupe: "seed".into(),
                result: None,
                view: None,
            }),
        )
        .expect("post the card");
        conn.execute(
            "INSERT INTO permission_requests (id, workspace_id, session_id, conversation_id, addr, card_id, action, params_json, \
             params_hash, text_hash, why, state, floor, created_at, expires_at) \
             VALUES (?1, ?2, 's', ?3, 'pri-billing', ?4, 'seed.switch', '{}', 'h', 't', 'Run the suite', 'pending', 'admin', ?5, ?6)",
            rusqlite::params![request_id, ws_id, conv, item.id, now, now + 3600],
        )
        .expect("seed the request");
        item.id
    };
    let note_body = |text: &str| json!({ "addr": "pri-billing", "id": card_id, "text": text }).to_string();
    let undo_body = json!({ "addr": "pri-billing", "id": card_id }).to_string();

    // ── switch off: today's answers, byte for byte ────────────────────
    // (This in-process daemon hasn't turned Garden access on yet: no boot,
    // no settings write. A real 0.46.3 daemon has it on from boot; see the
    // last block.)
    assert!(!k2_core::permission::gate::server_on());
    let today_post = tcp(port, "POST", "/cli/thread/not-a-card-route", OWNER, Some(&note_body("hi"))).await;
    for route in [NOTE, UNDO] {
        for token in [OWNER, sessions["pri-member"].as_str(), passport.as_str()] {
            let r = tcp(port, "POST", route, token, Some(&note_body("hi"))).await;
            let t = tcp(port, "POST", "/cli/thread/not-a-card-route", token, Some(&note_body("hi"))).await;
            assert_eq!((r.status, r.body.as_str()), (t.status, t.body.as_str()), "{route} off = an unknown thread route");
        }
    }
    assert_eq!(today_post.status, 405, "{}", today_post.body);
    let r = tcp(port, "GET", PENDING, OWNER, None).await;
    let t = tcp(port, "GET", "/cli/permission/not-a-route", OWNER, None).await;
    assert_eq!((r.status, r.body.as_str()), (t.status, t.body.as_str()), "pending off = unknown");
    assert!(note_rows(&request_id).is_empty(), "nothing written while off");

    // ── switch on ─────────────────────────────────────────────────────
    let r = tcp(port, "POST", "/cli/settings/update", OWNER, Some(r#"{"permissionCards":true}"#)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(k2_core::permission::gate::server_on());

    // A Member (below the Admin floor) leaves a note.
    let r = tcp(port, "POST", NOTE, &sessions["pri-member"], Some(&note_body("I need this for payroll"))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(json(&r)["note"]["by"], "pri-member");
    assert_eq!(json(&r)["requestId"], request_id.as_str());
    assert_eq!(note_rows(&request_id), vec![("pri-member".to_string(), "I need this for payroll".to_string())]);
    let r = tcp(port, "POST", NOTE, &sessions["pri-member"], Some(&note_body("see https://x.example"))).await;
    assert_eq!((r.status, json(&r)["error"]["reason"].as_str()), (400, Some("link")), "{}", r.body);
    // An approver answers instead.
    let r = tcp(port, "POST", NOTE, &sessions["pri-admin"], Some(&note_body("hi"))).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "note_not_allowed"), "{}", r.body);
    let r = tcp(port, "POST", NOTE, OWNER, Some(&note_body("hi"))).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "note_not_allowed"), "{}", r.body);
    // No agent writes a note: the passport over TCP and over its cell.
    let r = tcp(port, "POST", NOTE, &passport, Some(&note_body("from the agent"))).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = cell(&sock, "POST", NOTE, &passport, Some(&note_body("from the agent"))).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(note_rows(&request_id).len(), 1, "no agent note was written");
    // A GET is refused by the table (POST only).
    let r = tcp(port, "GET", NOTE, &sessions["pri-member"], None).await;
    assert_eq!(r.status, 405, "{}", r.body);

    // Turn off again on a waiting card: nothing to undo.
    let r = tcp(port, "POST", UNDO, &sessions["pri-admin"], Some(&undo_body)).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "undo_expired"), "{}", r.body);
    // A Member is below the route's floor.
    let r = tcp(port, "POST", UNDO, &sessions["pri-member"], Some(&undo_body)).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = tcp(port, "POST", UNDO, &passport, Some(&undo_body)).await;
    assert_eq!(r.status, 403, "{}", r.body);

    // Waiting on you: the people who can answer.
    for token in [OWNER, sessions["pri-owner"].as_str(), sessions["pri-admin"].as_str()] {
        let r = tcp(port, "GET", PENDING, token, None).await;
        assert_eq!(r.status, 200, "{}", r.body);
        let rows = json(&r)["requests"].as_array().expect("requests").clone();
        let mine: Vec<&Value> = rows.iter().filter(|x| x["requestId"] == request_id.as_str()).collect();
        assert_eq!(mine.len(), 1, "{}", r.body);
        assert_eq!((mine[0]["cardId"].as_str(), mine[0]["addr"].as_str()), (Some(card_id.as_str()), Some("pri-billing")));
        assert!(!r.body.contains("Run the suite"), "never the agent's --why");
        assert!(!r.body.contains("payroll"), "never a note");
    }
    let r = tcp(port, "GET", PENDING, &sessions["pri-member"], None).await;
    assert_eq!((r.status, json(&r)["error"].as_str()), (403, Some("role_required")), "the table's Admin floor: {}", r.body);
    // An agent whose own switch is off: today's body, both doors.
    let r = tcp(port, "GET", PENDING, &passport, None).await;
    assert_eq!((r.status, r.body.as_str()), (403, r#"{"error":"Invalid or missing auth token"}"#));
    let r = cell(&sock, "GET", PENDING, &passport, None).await;
    assert_eq!((r.status, r.body.as_str()), (403, r#"{"error":"Invalid or missing auth token"}"#));

    // ── switch off again: today's answers ─────────────────────────────
    // (The seeded request is withdrawn first: the switch-off close would
    // otherwise send its `[permission:]` turn to a real agent address.)
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE permission_requests SET state = 'cancelled', answered_at = ?2 WHERE id = ?1",
            rusqlite::params![request_id, k2_core::overlay::now_secs()],
        )
        .unwrap();
    }
    let r = tcp(port, "POST", "/cli/settings/update", OWNER, Some(r#"{"permissionCards":false}"#)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    // Garden access is always on from 0.46.3 (Rosson 2026-10-23; a real
    // daemon turns it on at boot, this in-process one at its first settings
    // write). K2's own Garden cards take the note and "Turn off again", so
    // with the Beta switch off the note route still answers for itself.
    assert!(k2_core::zen::garden_access::enabled(), "Garden access is on");
    let r = tcp(port, "POST", NOTE, &sessions["pri-member"], Some(&note_body("again"))).await;
    assert_eq!(r.status, 409, "{}", r.body);
    assert!(r.body.contains("request_not_pending"), "{}", r.body);
    drop(d);
}
