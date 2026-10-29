//! GH#72 (Rosson 2026-10-14): app guests can't write the agent's config
//! folder (`.k2/`) by default. Through a real in-process daemon: a role
//! with `files:write` is refused in `.k2/` with `agent_config_protected`,
//! the owner-set `files:agentconfig` opt-in lifts it for that room, reads
//! and ordinary files are unchanged, the owner token is unaffected, and an
//! agent with "manage Apps" can never set or spread the opt-in.
//!
//! Headless. Agent spawns resolve only to an `exec cat` shim.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::db::schema::WorkspaceSession;
use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-agent-config";

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("body must be JSON ({e}): {:?}", self.body))
    }
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("read timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
            b.len()
        ),
        None => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        ),
    };
    stream.write_all(req.as_bytes()).expect("write");
    stream.flush().expect("flush");
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let text = String::from_utf8_lossy(&raw).to_string();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let clen = head.lines().find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            });
            if let Some(c) = clen {
                if body.len() >= c {
                    break;
                }
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) => panic!("read response: {e:?} raw={}", String::from_utf8_lossy(&raw)),
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, body) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("no header terminator: {text:?}"));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    Resp {
        status,
        body: body.to_string(),
    }
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

/// Temp HOME and agent shim. Restored on panic.
fn with_temp_home<F: FnOnce()>(f: F) {
    struct Restore {
        prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
        tmp: PathBuf,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            for (name, val) in self.prev.drain(..) {
                match val {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
    const NAMES: [&str; 4] = [
        "HOME",
        "K2_TEST_AGENT_SHIM_DIR",
        "K2SO_WAKE_HEADLESS_TEST_COMMAND",
        "K2_HEARTBEAT_NO_SELF_HEAL",
    ];
    let tmp = std::env::temp_dir().join(format!(
        "k2-app-cfg-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let shim_dir = tmp.join("shim");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    let shim = shim_dir.join("claude");
    k2_core::test_env::write_executable(&shim, "#!/bin/sh\nexec cat\n");
    let _restore = Restore {
        prev: NAMES.iter().map(|n| (*n, std::env::var_os(n))).collect(),
        tmp: tmp.clone(),
    };
    std::env::set_var("HOME", &tmp);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
    std::env::set_var("K2SO_WAKE_HEADLESS_TEST_COMMAND", "claude");
    std::env::set_var("K2_HEARTBEAT_NO_SELF_HEAL", "1");
    f();
}

struct Room {
    id: String,
    handle: String,
    path: String,
}

impl Drop for Room {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

impl Room {
    fn role_md(&self) -> String {
        std::fs::read_to_string(format!("{}/.k2/agent/ROLE.md", self.path)).expect("ROLE.md")
    }
}

/// A registered workspace with a pinned chat, `.k2/agent/ROLE.md`,
/// `.k2/inbox/` and `notes/`.
fn seed_room(tag: &str) -> Room {
    let id = uuid::Uuid::new_v4().to_string();
    let handle = format!("{tag}{}", &id[..8]);
    let dir = std::env::temp_dir().join(format!("k2-app-cfg-room-{handle}"));
    std::fs::create_dir_all(dir.join(".k2/agent")).expect("agent dir");
    std::fs::create_dir_all(dir.join(".k2/inbox")).expect("inbox dir");
    std::fs::create_dir_all(dir.join("notes")).expect("notes dir");
    std::fs::write(dir.join(".k2/agent/ROLE.md"), "original role").expect("role");
    let path = dunce::canonicalize(&dir).expect("canon").to_string_lossy().into_owned();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle, agent_enabled) VALUES (?1, ?2, ?3, ?2, 1)",
        params![id, handle, path],
    )
    .expect("project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(
        &conn,
        &format!("ws-{conv}"),
        &id,
        None,
        Some(&conv),
        "claude",
        "system",
        "running",
    )
    .expect("pin");
    Room { id, handle, path }
}

fn post(port: u16, tok: &str, path: &str, body: serde_json::Value) -> Resp {
    http(port, "POST", &format!("{path}?token={tok}"), Some(&body.to_string()))
}

fn owner_ok(port: u16, path: &str, body: serde_json::Value) -> Resp {
    let r = post(port, OWNER_TOKEN, path, body);
    assert_eq!(r.status, 200, "{path}; {}", r.body);
    r
}

/// An app login whose role `r-<username>` has `caps` on `room`; returns its pass.
fn guest(port: u16, username: &str, room: &Room, caps: &[&str]) -> String {
    owner_ok(port, "/cli/skin/users", serde_json::json!({"username": username}));
    k2_daemon::test_harness::guest_with_own_password(username, "s3cret-horse");
    let role = format!("r-{username}");
    owner_ok(port, "/cli/skin/roles", serde_json::json!({"name": role}));
    owner_ok(
        port,
        "/cli/skin/roles/room",
        serde_json::json!({"name": role, "handle": room.handle, "caps": caps}),
    );
    owner_ok(port, "/cli/skin/roles/assign", serde_json::json!({"username": username, "role": role}));
    let login = http(
        port,
        "POST",
        "/cli/skin/login",
        Some(&serde_json::json!({"username": username, "password": "s3cret-horse"}).to_string()),
    );
    assert_eq!(login.status, 200, "skin login {username}; {}", login.body);
    let tok = login.json()["token"].as_str().expect("token").to_string();
    assert!(tok.starts_with("k2skn_"), "{tok}");
    tok
}

fn mint_passport(room: &Room) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal {
            workspace_uuid: room.id.clone(),
            agent_address: "app-cfg-agent".to_string(),
        },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

fn assert_status(r: &Resp, status: u16, needle: &str, label: &str) {
    assert_eq!(r.status, status, "{label}; {}", r.body);
    assert!(r.body.contains(needle), "{label}: body must contain {needle:?}: {}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guests_cannot_write_agent_config_unless_the_owner_opts_in() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let a = seed_room("cfg");

        // files:write alone: every door refuses .k2/, reads and ordinary
        // files keep working.
        let plain = guest(port, "gplain", &a, &["files:read", "files:write"]);
        let write = |tok: &str, path: &str| {
            post(
                port,
                tok,
                "/cli/fs/write-file",
                serde_json::json!({"workspace": a.handle, "path": path, "content": "guest text"}),
            )
        };
        for path in [".k2/agent/ROLE.md", ".k2/inbox/task.md", ".K2/agent/ROLE.md"] {
            assert_status(&write(&plain, path), 403, "agent_config_protected", path);
        }
        let r = post(port, &plain, "/cli/fs/upload-binary", serde_json::json!({"workspace": a.handle, "dir": ".k2/inbox", "filename": "x.bin", "base64": "aGk="}));
        assert_status(&r, 403, "agent_config_protected", "upload-binary");
        let r = post(port, &plain, "/cli/fs/upload-chunk", serde_json::json!({"workspace": a.handle, "dir": ".k2/inbox", "filename": "x.bin", "upload_id": "u1", "offset": 0, "base64": "aGk=", "is_last": true}));
        assert_status(&r, 403, "agent_config_protected", "upload-chunk");
        let r = post(port, &plain, "/cli/fs/create", serde_json::json!({"workspace": a.handle, "path": ".k2/inbox/new", "is_directory": true}));
        assert_status(&r, 403, "agent_config_protected", "mkdir");
        let r = post(port, &plain, "/cli/fs/rename", serde_json::json!({"workspace": a.handle, "path": ".k2/agent/ROLE.md", "new_name": "old.md"}));
        assert_status(&r, 403, "agent_config_protected", "rename out");
        let r = post(port, &plain, "/cli/fs/move", serde_json::json!({"workspace": a.handle, "sources": [".k2/agent/ROLE.md"], "destination": "notes"}));
        assert_status(&r, 403, "agent_config_protected", "move out");
        assert_eq!(write(&plain, "notes/today.md").status, 200, "ordinary write");
        let r = post(port, &plain, "/cli/fs/copy", serde_json::json!({"workspace": a.handle, "sources": ["notes/today.md"], "destination": ".k2/inbox"}));
        assert_status(&r, 403, "agent_config_protected", "copy into");
        assert_eq!(a.role_md(), "original role");
        assert_eq!(std::fs::read_dir(format!("{}/.k2/inbox", a.path)).expect("inbox").count(), 0);
        let r = http(port, "GET", &format!("/cli/fs/read-file?token={plain}&workspace={}&path=.k2/agent/ROLE.md", a.handle), None);
        assert_eq!(r.status, 200, "reads under .k2 stay open; {}", r.body);
        assert!(r.body.contains("original role"), "{}", r.body);

        // The opt-in means nothing alone.
        let r = post(port, OWNER_TOKEN, "/cli/skin/roles", serde_json::json!({"name": "lonely", "roomAccess": [{"handle": a.handle, "caps": ["files:agentconfig"]}]}));
        assert_status(&r, 400, "files:agentconfig only widens files:write", "opt-in without files:write");

        // Owner opt-in on the role: the guest may write .k2/.
        let opted = guest(port, "gopt", &a, &["files:read", "files:write", "files:agentconfig"]);
        assert_eq!(write(&opted, ".k2/agent/ROLE.md").status, 200, "opted-in write");
        assert_eq!(a.role_md(), "guest text");
        assert_eq!(write(&opted, ".k2/inbox/task.md").status, 200, "opted-in inbox");

        // The owner token is unaffected.
        let r = post(port, OWNER_TOKEN, "/cli/fs/write-file", serde_json::json!({"path": format!("{}/.k2/agent/ROLE.md", a.path), "content": "owner text"}));
        assert_eq!(r.status, 200, "owner write; {}", r.body);
        assert_eq!(a.role_md(), "owner text");

        // An agent with "manage Apps" can't set or spread the opt-in.
        k2_core::workspace::settings::set_agents_can_manage_skin(&a.path, true).expect("manage on");
        let hook = mint_passport(&a);
        let owner_only = |r: &Resp, label: &str| {
            assert_eq!(r.status, 403, "{label}; {}", r.body);
            assert_eq!(r.json()["error"]["code"], "owner_only", "{label}; {}", r.body);
            assert!(r.body.contains("files:agentconfig"), "{label}; {}", r.body);
        };
        let flagged = serde_json::json!(["files:read", "files:write", "files:agentconfig"]);
        let r = post(port, &hook, "/cli/skin/roles", serde_json::json!({"name": "agent-r", "roomAccess": [{"handle": a.handle, "caps": flagged}]}));
        owner_only(&r, "agent creates a flagged role");
        // Sanity: the same agent may manage roles without the opt-in.
        let r = post(port, &hook, "/cli/skin/roles", serde_json::json!({"name": "agent-r", "roomAccess": [{"handle": a.handle, "caps": ["files:read", "files:write"]}]}));
        assert_eq!(r.status, 200, "agent role without opt-in; {}", r.body);
        let r = post(port, &hook, "/cli/skin/roles/room", serde_json::json!({"name": "agent-r", "handle": a.handle, "caps": flagged}));
        owner_only(&r, "agent adds the opt-in to a room");
        let r = post(port, &hook, "/cli/skin/roles/update", serde_json::json!({"name": "agent-r", "roomAccess": [{"handle": a.handle, "caps": flagged}]}));
        owner_only(&r, "agent updates a role to carry the opt-in");
        // agent-app-users S2: an agent adds App users only (one of its Apps).
        let app_dir = std::env::temp_dir().join(format!("cfg-app-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&app_dir).expect("app dir");
        let app = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::published_services::insert(&conn, &a.id, "cfgapp", "node s.mjs", &app_dir.to_string_lossy(), 47_401, "local", "stopped", "cmd", "").expect("service").id
        };
        // 0.46.4: its App users are their own switch, "Let this agent manage
        // its Apps' users": off, the add is refused and nothing is made.
        let r = post(port, &hook, "/cli/skin/users", serde_json::json!({"username": "agentguest", "app": app}));
        assert_eq!((r.status, r.json()["error"]["code"].clone()), (403, serde_json::json!("app_users_off")), "{}", r.body);
        assert!(k2_core::skin::list_principals().unwrap().iter().all(|p| p.username != "agentguest"), "nothing made");
        k2_core::workspace::settings::set_agents_can_manage_app_users(&a.id, true).expect("app users on");
        let r = post(port, &hook, "/cli/skin/users", serde_json::json!({"username": "agentguest", "app": app}));
        assert_eq!(r.status, 200, "agent adds an App user; {}", r.body);
        let r = post(port, &hook, "/cli/skin/roles/assign", serde_json::json!({"username": "agentguest", "role": "r-gopt"}));
        owner_only(&r, "agent hands out the owner's opted-in role");
        let r = post(port, &hook, "/cli/skin-tokens", serde_json::json!({"name": "agent-tok", "caps": flagged, "rooms": [a.handle]}));
        owner_only(&r, "agent mints a flagged platform token");
        let tok = owner_ok(port, "/cli/skin-tokens", serde_json::json!({"name": "owner-tok", "caps": flagged, "rooms": [a.handle]}));
        let tok_id = tok.json()["id"].as_str().expect("token id").to_string();
        let b = seed_room("cfgb");
        let r = post(port, &hook, "/cli/skin-tokens/rooms", serde_json::json!({"id": tok_id, "rooms": [a.handle, b.handle]}));
        owner_only(&r, "agent widens a flagged token's rooms");
        // Nothing the agent tried stuck.
        let roles = http(port, "GET", &format!("/cli/skin/roles?token={OWNER_TOKEN}"), None).json();
        let agent_role = roles["roles"]
            .as_array()
            .expect("roles")
            .iter()
            .find(|r| r["name"] == "agent-r")
            .expect("agent-r");
        assert!(
            !agent_role["caps"].as_array().expect("caps").iter().any(|c| c == "files:agentconfig"),
            "{agent_role}"
        );
        // Tightening stays open: the agent may remove the opt-in.
        let r = post(port, &hook, "/cli/skin/roles/room", serde_json::json!({"name": "r-gopt", "handle": a.handle, "caps": ["files:read", "files:write"]}));
        assert_eq!(r.status, 200, "agent removes the opt-in; {}", r.body);
        // The owner can still set it on any role.
        let r = post(port, OWNER_TOKEN, "/cli/skin/roles/room", serde_json::json!({"name": "agent-r", "handle": a.handle, "caps": flagged}));
        assert_eq!(r.status, 200, "owner sets the opt-in; {}", r.body);
    });
}
