//! Design mode notes (prd-browser-design-mode-v1 D-A9, D-A10, D-A13), end to
//! end through the real dispatcher:
//!
//! - GET → 405; no token / a bad token → 403; an app pass → 403.
//! - A Connect login without the workspace's remote-instruct opt-in → 403
//!   (`remote_instruction_disabled`), nothing posted; with the opt-in → 200.
//! - The owner's Send: exactly ONE Thread item (design origin, via compose),
//!   delivered to the pinned Chat (the `exec cat` shim); Retry
//!   (`/redeliver`) adds no second item.
//! - An agent passport posts on its own workspace and is refused on another.
//!
//! ISOLATION: temp HOME, an `exec cat` shim as `claude` (no agent CLI ever
//! starts).

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_core::connect_users::{self, Role};
use k2_core::db::schema::WorkspaceSession;
use k2_core::session::SessionId;
use k2_daemon::session_token::{self, HookPrincipal};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::Value as J;

const OWNER_TOKEN: &str = "owner-token-design-notes";

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> J {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", self.body))
    }
}

fn raw_request(port: u16, head: &str, body: &[u8]) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    s.write_all(head.as_bytes()).expect("write head");
    s.write_all(body).expect("write body");
    let mut raw = Vec::new();
    match s.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("read: {e:?}"),
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_else(|| panic!("no body: {text:?}"));
    Resp { status, body }
}

fn http(port: u16, method: &str, path: &str, bearer: Option<&str>, body: Option<&str>) -> Resp {
    let b = body.unwrap_or("");
    let auth = bearer.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        b.len()
    );
    raw_request(port, &head, b.as_bytes())
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn with_temp_home<F: FnOnce(&Path)>(f: F) {
    struct Restore {
        _g: std::sync::MutexGuard<'static, ()>,
        prev_home: Option<std::ffi::OsString>,
        prev_shim: Option<std::ffi::OsString>,
        tmp: PathBuf,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.prev_home.take() {
                Some(p) => std::env::set_var("HOME", p),
                None => std::env::remove_var("HOME"),
            }
            match self.prev_shim.take() {
                Some(p) => std::env::set_var("K2_TEST_AGENT_SHIM_DIR", p),
                None => std::env::remove_var("K2_TEST_AGENT_SHIM_DIR"),
            }
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
    let g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    let tmp = std::env::temp_dir().join(format!("k2-dm-notes-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    let shim_dir = tmp.join("shim");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    let shim = shim_dir.join("claude");
    std::fs::write(&shim, "#!/bin/sh\nexec cat\n").expect("shim");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let tmp = dunce::canonicalize(&tmp).expect("canonical tmp");
    let _restore = Restore {
        _g: g,
        prev_home: std::env::var_os("HOME"),
        prev_shim: std::env::var_os("K2_TEST_AGENT_SHIM_DIR"),
        tmp: tmp.clone(),
    };
    std::env::set_var("HOME", &tmp);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", tmp.join("shim"));
    k2_core::test_isolation::assert_isolated_from_prod();
    f(&tmp);
}

struct Room {
    handle: String,
    project_id: String,
    path: PathBuf,
    conv: String,
}

fn seed_room(home: &Path, handle: &str) -> Room {
    let path = home.join("ws").join(handle);
    std::fs::create_dir_all(&path).expect("workspace dir");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path.to_string_lossy()],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running")
        .expect("pin");
    Room { handle: handle.to_string(), project_id: id, path, conv }
}

fn thread_len(conv: &str) -> usize {
    k2_core::overlay::read_thread(conv, 0).expect("read thread").len()
}

/// Every Thread item of the room: the first post's wake starts a fresh
/// session and may re-pin the Chat to a new conversation, so later posts
/// land there while the first stays where it was posted.
fn room_items(port: u16, room: &Room) -> usize {
    let r = http(port, "GET", &format!("/cli/thread?token={OWNER_TOKEN}&addr={}&limit=1", room.handle), None, None);
    assert_eq!(r.status, 200, "{}", r.body);
    let now = r.json()["conversation_id"].as_str().expect("conversation_id").to_string();
    if now == room.conv {
        thread_len(&room.conv)
    } else {
        thread_len(&room.conv) + thread_len(&now)
    }
}

fn notes(workspace: &str) -> String {
    serde_json::json!({
        "workspace": workspace,
        "itemId": "design-1",
        "page": {"url": "http://localhost:5173/pricing?token=z#a", "title": "Pricing"},
        "notes": [{
            "comment": "make it the primary colour",
            "intent": "change",
            "payload": {"selector": "button.cta", "tag": "button", "text": "Buy", "html": "<button onclick=\"x()\">Buy</button>"},
        }],
    })
    .to_string()
}

fn mint_guest(port: u16, name: &str, room: &str) -> String {
    let r = http(port, "POST", &format!("/cli/skin/users?token={OWNER_TOKEN}"), None, Some(&format!(r#"{{"username":"{name}"}}"#)));
    assert_eq!(r.status, 200, "skin user add: {}", r.body);
    let r = http(
        port,
        "POST",
        &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
        None,
        Some(&format!(r#"{{"name":"{name}","caps":["thread:read","thread:post"],"rooms":["{room}"]}}"#)),
    );
    assert_eq!(r.status, 200, "mint: {}", r.body);
    let tok = r.json()["token"].as_str().expect("token").to_string();
    assert!(tok.starts_with("k2skn_"), "{tok}");
    tok
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn design_notes_gates_single_item_and_retry() {
    with_temp_home(|home| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
        let room = seed_room(home, &format!("dmroom{tag}"));
        let other = seed_room(home, &format!("dmother{tag}"));
        let body = notes(&room.handle);

        // GET is 405, never a mutation.
        let r = http(port, "GET", &format!("/cli/browser/notes?token={OWNER_TOKEN}"), None, None);
        assert_eq!(r.status, 405, "GET: {}", r.body);
        let r = http(port, "GET", &format!("/cli/browser/notes/redeliver?token={OWNER_TOKEN}"), None, None);
        assert_eq!(r.status, 405, "GET redeliver: {}", r.body);
        // No token / a bad token.
        let r = http(port, "POST", "/cli/browser/notes", None, Some(&body));
        assert_eq!(r.status, 403, "no token: {}", r.body);
        let r = http(port, "POST", "/cli/browser/notes?token=not-a-token", None, Some(&body));
        assert_eq!(r.status, 403, "bad token: {}", r.body);
        // An app pass, even one with thread:post on this room.
        let guest = mint_guest(port, &format!("dmguest{tag}"), &room.handle);
        let r = http(port, "POST", &format!("/cli/browser/notes?token={guest}"), None, Some(&body));
        assert_eq!(r.status, 403, "app pass: {}", r.body);
        assert!(r.body.contains("app passes"), "{}", r.body);
        assert_eq!(thread_len(&room.conv), 0, "no refused call posts");

        // A Connect login without the remote-instruct opt-in.
        connect_users::add_user("dmmember", "password123").expect("add_user");
        connect_users::set_role("dmmember", Role::Member).expect("set_role");
        let member = connect_users::create_session("dmmember");
        let r = http(port, "POST", &format!("/cli/browser/notes?token={member}"), None, Some(&body));
        assert_eq!(r.status, 403, "member without opt-in: {}", r.body);
        assert_eq!(r.json()["error"], "remote_instruction_disabled", "{}", r.body);
        assert_eq!(thread_len(&room.conv), 0);

        // The owner's Send: one Thread item, delivered to the pinned Chat.
        let r = http(port, "POST", &format!("/cli/browser/notes?token={OWNER_TOKEN}"), None, Some(&body));
        assert_eq!(r.status, 200, "owner: {}", r.body);
        let v = r.json();
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["delivered"], true, "the pinned Chat (shim) took the inject: {v}");
        assert!(v["targetSessionId"].as_str().is_some_and(|s| !s.is_empty()), "{v}");
        let items = k2_core::overlay::read_thread(&room.conv, 0).expect("thread");
        assert_eq!(items.len(), 1, "exactly one Thread item: {items:?}");
        let doc = &items[0].doc;
        assert_eq!(doc.via.as_deref(), Some("compose"));
        assert_eq!(doc.design.as_ref().map(|d| d.url.as_str()), Some("http://localhost:5173/pricing"));
        let text = doc.body.as_deref().expect("body");
        assert!(!text.contains("onclick") && !text.contains("token=z"), "{text}");
        // The post is the notes plus a pointer; the details are a file in
        // the room's own workspace, under .k2/design/.
        let rel = text
            .rsplit_once("Details: `")
            .and_then(|(_, r)| r.strip_suffix('`'))
            .unwrap_or_else(|| panic!("no Details pointer: {text}"));
        assert!(rel.starts_with(".k2/design/") && rel.ends_with(".md"), "{rel}");
        assert!(!text.contains("button.cta"), "no selector in the Thread: {text}");
        let details = std::fs::read_to_string(room.path.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert!(details.contains("- selector: `button.cta`"), "{details}");
        assert!(!details.contains("onclick"), "{details}");
        let hist = k2_core::workspace_compose_history::list_compose_send_history(&room.project_id).expect("history");
        assert!(hist.is_empty(), "no compose history: {hist:?}");

        // Retry delivers the same item again; no second item.
        let item_id = v["threadItemId"].as_str().expect("id").to_string();
        let retry = serde_json::json!({"workspace": room.handle, "threadItemId": item_id}).to_string();
        let r = http(port, "POST", &format!("/cli/browser/notes/redeliver?token={OWNER_TOKEN}"), None, Some(&retry));
        assert_eq!(r.status, 200, "redeliver: {}", r.body);
        assert_eq!(r.json()["threadItemId"], item_id.as_str());
        assert_eq!(room_items(port, &room), 1, "Retry never posts a second item");

        // The member, once the workspace opts in.
        k2_core::workspace::settings::update_project_setting(&room.path.to_string_lossy(), "allow_remote_instruct", "1")
            .expect("opt in");
        let r = http(port, "POST", &format!("/cli/browser/notes?token={member}"), None, Some(&body));
        assert_eq!(r.status, 200, "member with opt-in: {}", r.body);
        assert_eq!(r.json()["from"], "dmmember", "{}", r.body);
        assert_eq!(room_items(port, &room), 2);

        // An agent passport: its own workspace yes, another no.
        let principal = HookPrincipal {
            workspace_uuid: room.project_id.clone(),
            agent_address: room.project_id.clone(),
        };
        let agent = session_token::mint_session_token(
            &SessionId::new(),
            "pane-dm",
            principal,
            session_token::CredMode::ApiKey,
            session_token::Provider::Anthropic,
        );
        let r = http(port, "POST", "/cli/browser/notes", Some(&agent), Some(&body));
        assert_eq!(r.status, 200, "agent, own workspace: {}", r.body);
        assert_eq!(room_items(port, &room), 3);
        let r = http(port, "POST", "/cli/browser/notes", Some(&agent), Some(&notes(&other.handle)));
        assert_eq!(r.status, 403, "agent, another workspace: {}", r.body);
        assert_eq!(thread_len(&other.conv), 0);
        // A query-string stamp is never trusted as a passport.
        let r = http(
            port,
            "POST",
            &format!("/cli/browser/notes?token=not-a-token&principal_bound=1&project_id={}", other.project_id),
            None,
            Some(&body),
        );
        assert_eq!(r.status, 403, "{}", r.body);
    });
}
