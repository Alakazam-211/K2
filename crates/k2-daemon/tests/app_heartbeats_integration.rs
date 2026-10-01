//! App heartbeats surface (`.k2/prds/prd-app-heartbeats-surface-v1.md`
//! AH1–AH25, vs-live AH26–AH34, Rosson R1–R3) through a real in-process
//! daemon: dispatcher arms, route policy, caps, room jail, projection,
//! rate limits, the fire-now lease, R2 WAKEUP.md file access, and the
//! actor-stamped history (AH18) for owner, Connect user, agent and app.
//!
//! Headless. Agent spawns resolve only to an `exec cat` shim
//! (`K2_TEST_AGENT_SHIM_DIR`), and `K2_HEARTBEAT_NO_SELF_HEAL=1` keeps the
//! add path from touching launchd / crontab.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::db::schema::{AgentHeartbeat, HeartbeatFire, LeaseCheck, LeaseOutcome, WorkspaceSession};
use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-app-heartbeats";

struct Resp {
    status: u16,
    body: String,
    headers: String,
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
        headers: head.to_string(),
    }
}

fn header_value(headers: &str, name: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
    })
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

/// Temp HOME, agent shim, no launchd/crontab writes. Restored on panic.
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
        "k2-app-hb-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let shim_dir = tmp.join("shim");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    let shim = shim_dir.join("claude");
    std::fs::write(&shim, "#!/bin/sh\nexec cat\n").expect("shim");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
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

/// A registered workspace with an agent and a pinned chat.
fn seed_room(tag: &str) -> Room {
    let id = uuid::Uuid::new_v4().to_string();
    let handle = format!("{tag}{}", &id[..8]);
    let dir = std::env::temp_dir().join(format!("k2-app-hb-room-{handle}"));
    std::fs::create_dir_all(&dir).expect("room dir");
    let path = dir.canonicalize().expect("canon").to_string_lossy().into_owned();
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

fn owner_post(port: u16, path: &str, body: &str) {
    let r = http(port, "POST", &format!("{path}?token={OWNER_TOKEN}"), Some(body));
    assert_eq!(r.status, 200, "{path}; {}", r.body);
}

/// An app login whose role has `caps` on each room; returns its pass.
fn guest(port: u16, username: &str, rooms: &[(&Room, &[&str])]) -> String {
    owner_post(port, "/cli/skin/users", &format!(r#"{{"username":"{username}"}}"#));
    owner_post(
        port,
        "/cli/skin/users/password",
        &format!(r#"{{"username":"{username}","password":"s3cret-horse"}}"#),
    );
    let role = format!("r-{username}");
    owner_post(port, "/cli/skin/roles", &format!(r#"{{"name":"{role}"}}"#));
    for (room, caps) in rooms {
        let caps = serde_json::to_string(caps).expect("caps");
        owner_post(
            port,
            "/cli/skin/roles/room",
            &format!(r#"{{"name":"{role}","handle":"{}","caps":{caps}}}"#, room.handle),
        );
    }
    owner_post(
        port,
        "/cli/skin/roles/assign",
        &format!(r#"{{"username":"{username}","role":"{role}"}}"#),
    );
    let login = http(
        port,
        "POST",
        "/cli/skin/login",
        Some(&format!(r#"{{"username":"{username}","password":"s3cret-horse"}}"#)),
    );
    assert_eq!(login.status, 200, "skin login {username}; {}", login.body);
    let tok = login.json()["token"].as_str().expect("token").to_string();
    assert!(tok.starts_with("k2skn_"), "{tok}");
    tok
}

fn app_get(port: u16, tok: &str, path: &str, query: &str) -> Resp {
    http(port, "GET", &format!("{path}?token={tok}&{query}"), None)
}

fn app_post(port: u16, tok: &str, path: &str, body: serde_json::Value) -> Resp {
    http(port, "POST", &format!("{path}?token={tok}"), Some(&body.to_string()))
}

fn owner_get(port: u16, path: &str, room: &Room, query: &str) -> Resp {
    http(
        port,
        "GET",
        &format!("{path}?token={OWNER_TOKEN}&project={}&{query}", room.path),
        None,
    )
}

fn row(room: &Room, name: &str) -> Option<AgentHeartbeat> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    AgentHeartbeat::get_by_name(&conn, &room.id, name).expect("query row")
}

fn history(room: &Room, name: &str) -> Vec<HeartbeatFire> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    HeartbeatFire::list_by_schedule_name(&conn, &room.id, name, 200).expect("history")
}

/// The newest `changed` row's (reason, actor).
fn last_change(room: &Room, name: &str) -> (String, Option<String>) {
    let h = history(room, name);
    let c = h
        .iter()
        .find(|f| f.decision == "changed")
        .unwrap_or_else(|| panic!("no changed row for {name}: {h:?}"));
    (c.reason.clone().expect("changed reason"), c.actor.clone())
}

fn assert_status(r: &Resp, status: u16, needle: &str, label: &str) {
    assert_eq!(r.status, status, "{label}; {}", r.body);
    assert!(r.body.contains(needle), "{label}: body must contain {needle:?}: {}", r.body);
}

const GUEST_ROW_KEYS: [&str; 14] = [
    "createdAt",
    "delivery",
    "disabledReason",
    "enabled",
    "firing",
    "frequency",
    "lastFired",
    "name",
    "nextFireAt",
    "scheduleError",
    "spec",
    "waitDetail",
    "waitReason",
    "waitSince",
];

fn keys(v: &serde_json::Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().expect("object").keys().cloned().collect();
    k.sort();
    k
}

fn mint_passport(room: &Room) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal {
            workspace_uuid: room.id.clone(),
            agent_address: "app-hb-agent".to_string(),
        },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

/// T3–T8, T10, AH26, AH32/33, R1, plus actor rows for every caller class.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_heartbeats_jail_caps_projection_writes_and_actors() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let a = seed_room("sales");
        let b = seed_room("mktg");

        // Room B has an owner heartbeat the app must never see.
        let r = owner_get(
            port,
            "/cli/heartbeat/add",
            &b,
            "name=b-only&frequency=hourly&spec=%7B%22every_seconds%22%3A900%7D&instructions=secret",
        );
        assert_eq!(r.status, 200, "owner add in B; {}", r.body);

        let bob = guest(
            port,
            "bob",
            &[(
                &a,
                &["thread:read", "heartbeats:read", "heartbeats:write", "files:read", "files:write"],
            )],
        );
        let carl = guest(port, "carl", &[(&a, &["thread:read", "heartbeats:write"])]);
        let dora = guest(port, "dora", &[(&a, &["thread:read", "thread:post"])]);
        let erin = guest(port, "erin", &[(&a, &["heartbeats:read"])]);

        // ── T3: room jail. Guest path keys are dropped. ─────────────────
        let added = app_post(
            port,
            &bob,
            "/cli/heartbeat/add",
            serde_json::json!({
                "workspace": a.handle,
                "project": b.path,
                "project_path": b.path,
                "project_id": b.id,
                "name": "inbox-sweep",
                "frequency": "hourly",
                "spec": {"every_seconds": 900},
                "instructions": "Check the sales inbox.",
            }),
        );
        assert_eq!(added.status, 200, "app add; {}", added.body);
        let added_v = added.json();
        assert_eq!(keys(&added_v), GUEST_ROW_KEYS, "AH8 reply: {added_v}");
        assert!(added_v.get("wakeupAbs").is_none(), "{added_v}");
        assert_eq!(added_v["delivery"], "pinned", "AH15: {added_v}");
        assert!(row(&a, "inbox-sweep").is_some(), "landed in room A");
        assert!(row(&b, "inbox-sweep").is_none(), "never in room B");

        let list = app_get(
            port,
            &bob,
            "/cli/heartbeat/list",
            &format!("workspace={}&project={}&project_path={}", a.handle, b.path, b.path),
        );
        assert_eq!(list.status, 200, "list; {}", list.body);
        let names: Vec<String> = list
            .json()
            .as_array()
            .expect("array")
            .iter()
            .map(|r| r["name"].as_str().expect("name").to_string())
            .collect();
        assert_eq!(names, vec!["inbox-sweep".to_string()], "room A rows only");
        // T5: no forbidden key.
        for r in list.json().as_array().expect("array") {
            assert_eq!(keys(r), GUEST_ROW_KEYS, "AH8 row: {r}");
        }
        assert!(!list.body.contains(&a.path), "no room path: {}", list.body);

        let other = app_get(port, &bob, "/cli/heartbeat/list", &format!("workspace={}", b.handle));
        assert_status(&other, 403, "skin_room", "room B is not on the pass");

        // ── T4: cap order ────────────────────────────────────────────────
        let r = app_get(port, &dora, "/cli/heartbeat/list", &format!("workspace={}", a.handle));
        assert_status(&r, 403, "missing capability heartbeats:read", "thread-only list");
        let r = app_post(
            port,
            &dora,
            "/cli/heartbeat/add",
            serde_json::json!({"workspace": a.handle, "name": "x", "frequency": "daily",
                               "spec": {"time": "07:00"}, "instructions": "x"}),
        );
        assert_status(&r, 403, "missing capability heartbeats:write", "thread-only add");
        let r = app_get(port, &carl, "/cli/heartbeat/list", &format!("workspace={}", a.handle));
        assert_status(&r, 403, "missing capability heartbeats:read", "write does not imply read");
        let r = app_post(
            port,
            &erin,
            "/cli/heartbeat/enable",
            serde_json::json!({"workspace": a.handle, "name": "inbox-sweep", "enabled": false}),
        );
        assert_status(&r, 403, "missing capability heartbeats:write", "read does not imply write");
        // AH6: a GET on a write path is not an app door.
        let r = app_get(port, &bob, "/cli/heartbeat/add", &format!("workspace={}", a.handle));
        assert_eq!(r.status, 405, "GET on a write path; {}", r.body);
        // AH7 / R1: never-allowlisted paths stay closed to app passes.
        for path in [
            "/cli/heartbeat/remove",
            "/cli/heartbeat/unarchive",
            "/cli/heartbeat/list-archived",
            "/cli/heartbeat/scheduler-status",
            "/cli/heartbeat/set-session",
        ] {
            let r = app_get(port, &bob, path, &format!("workspace={}&name=inbox-sweep", a.handle));
            assert_eq!(r.status, 403, "{path} for an app pass; {}", r.body);
        }
        assert!(row(&a, "inbox-sweep").is_some(), "app remove must not delete");

        // ── T6: show and the WAKEUP.md body (AH9 / R2) ───────────────────
        let show = app_get(
            port,
            &bob,
            "/cli/heartbeat/show",
            &format!("workspace={}&name=inbox-sweep", a.handle),
        );
        assert_eq!(show.status, 200, "show; {}", show.body);
        let sv = show.json();
        assert_eq!(sv["instructions"], "Check the sales inbox.", "{sv}");
        assert_eq!(sv["wakeupPath"], ".k2/heartbeats/inbox-sweep/WAKEUP.md", "{sv}");
        assert!(sv.get("instructionsHidden").is_none(), "{sv}");
        let show = app_get(
            port,
            &erin,
            "/cli/heartbeat/show",
            &format!("workspace={}&name=inbox-sweep", a.handle),
        );
        assert_eq!(show.status, 200, "show without files:read; {}", show.body);
        let sv = show.json();
        assert_eq!(sv["instructionsHidden"], true, "{sv}");
        assert!(sv.get("instructions").is_none(), "{sv}");
        assert!(sv.get("wakeupPath").is_none(), "{sv}");
        let r = app_get(port, &bob, "/cli/heartbeat/show", &format!("workspace={}&name=ghost", a.handle));
        assert_status(&r, 404, "no_such_heartbeat", "show unknown");

        // ── T7: add validation (AH11, HB34, R3 floor) ────────────────────
        let add = |body: serde_json::Value| app_post(port, &bob, "/cli/heartbeat/add", body);
        let r = add(serde_json::json!({"workspace": a.handle, "name": "no-body", "frequency": "daily", "spec": {"time": "07:00"}}));
        assert_status(&r, 400, "instructions_required", "missing instructions");
        let r = add(serde_json::json!({"workspace": a.handle, "name": "blank", "frequency": "daily", "spec": {"time": "07:00"}, "instructions": "   "}));
        assert_status(&r, 400, "instructions_required", "blank instructions");
        let r = add(serde_json::json!({"workspace": a.handle, "name": "bad-freq", "frequency": "list", "spec": {}, "instructions": "x"}));
        assert_status(&r, 400, "unknown frequency", "HB34 frequency");
        let r = add(serde_json::json!({"workspace": a.handle, "name": "fast", "frequency": "hourly", "spec": {"every_seconds": 15}, "instructions": "x"}));
        assert_status(&r, 400, "interval_too_short", "15 s");
        let r = add(serde_json::json!({"workspace": a.handle, "name": "fast", "frequency": "hourly", "spec": {"every_seconds": 120}, "instructions": "x"}));
        assert_status(&r, 400, "interval_too_short", "R3: 2 min is under the app floor");
        let big = "x".repeat(16 * 1024 + 1);
        let r = add(serde_json::json!({"workspace": a.handle, "name": "big", "frequency": "daily", "spec": {"time": "07:00"}, "instructions": big}));
        assert_status(&r, 400, "instructions_too_large", "16 KiB cap");
        for n in ["no-body", "blank", "bad-freq", "fast", "big"] {
            assert!(row(&a, n).is_none(), "{n} must not be created");
        }

        // ── T8: unknown names are 404 and announce nothing ───────────────
        let mut rx = k2_daemon::session_events::subscribe();
        for (path, body) in [
            ("/cli/heartbeat/enable", serde_json::json!({"workspace": a.handle, "name": "ghost", "enabled": true})),
            ("/cli/heartbeat/edit", serde_json::json!({"workspace": a.handle, "name": "ghost", "instructions": "x"})),
            ("/cli/heartbeat/archive", serde_json::json!({"workspace": a.handle, "name": "ghost"})),
            ("/cli/heartbeat/rename", serde_json::json!({"workspace": a.handle, "from": "ghost", "to": "spirit"})),
            ("/cli/heartbeat/fire", serde_json::json!({"workspace": a.handle, "name": "ghost"})),
        ] {
            let r = app_post(port, &bob, path, body);
            assert_status(&r, 404, "no_such_heartbeat", path);
        }
        // Owner routes too (AH13): never a success.
        let r = owner_get(port, "/cli/heartbeat/enable", &a, "name=ghost&enabled=false");
        assert_status(&r, 404, "no_such_heartbeat", "owner enable unknown");
        let r = owner_get(port, "/cli/heartbeat/archive", &a, "name=ghost");
        assert_status(&r, 404, "no_such_heartbeat", "owner archive unknown");
        loop {
            match rx.try_recv() {
                Ok(k2_daemon::session_events::SessionEvent::HeartbeatRosterChanged { project_id, .. }) => {
                    assert_ne!(project_id, a.id, "a refused write must not announce a change");
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }

        // ── AH26: enable needs an explicit boolean ───────────────────────
        let r = app_post(port, &bob, "/cli/heartbeat/enable", serde_json::json!({"workspace": a.handle, "name": "inbox-sweep"}));
        assert_status(&r, 400, "enabled must be true or false", "dropped enabled");
        assert!(row(&a, "inbox-sweep").expect("row").enabled, "a dropped field never disables");

        // ── Writes + actor rows (AH12, AH18, AH32) ───────────────────────
        assert_eq!(
            last_change(&a, "inbox-sweep"),
            ("added".to_string(), Some("app:bob".to_string()))
        );
        let r = app_post(port, &bob, "/cli/heartbeat/enable", serde_json::json!({"workspace": a.handle, "name": "inbox-sweep", "enabled": false}));
        assert_eq!(r.status, 200, "app disable; {}", r.body);
        assert_eq!(keys(&r.json()), GUEST_ROW_KEYS, "{}", r.body);
        assert_eq!(r.json()["enabled"], false);
        assert_eq!(
            last_change(&a, "inbox-sweep"),
            ("disabled".to_string(), Some("app:bob".to_string()))
        );
        // Instructions alone (AH32), frontmatter kept.
        let r = app_post(port, &carl, "/cli/heartbeat/edit", serde_json::json!({"workspace": a.handle, "name": "inbox-sweep", "instructions": "Sweep twice."}));
        assert_eq!(r.status, 200, "edit instructions only; {}", r.body);
        let raw = std::fs::read_to_string(format!("{}/.k2/heartbeats/inbox-sweep/WAKEUP.md", a.path)).expect("WAKEUP.md");
        assert!(raw.starts_with("---\ndescription:"), "frontmatter kept: {raw:?}");
        assert!(raw.ends_with("Sweep twice.\n"), "{raw:?}");
        assert_eq!(
            last_change(&a, "inbox-sweep"),
            ("instructions edited".to_string(), Some("app:carl".to_string()))
        );
        let r = app_post(port, &carl, "/cli/heartbeat/edit", serde_json::json!({"workspace": a.handle, "name": "inbox-sweep", "instructions": " "}));
        assert_status(&r, 400, "instructions_required", "blank edit");
        let r = app_post(port, &carl, "/cli/heartbeat/edit", serde_json::json!({"workspace": a.handle, "name": "inbox-sweep", "frequency": "hourly", "spec": {"every_seconds": 60}}));
        assert_status(&r, 400, "interval_too_short", "edit under the app floor");
        let r = app_post(port, &carl, "/cli/heartbeat/edit", serde_json::json!({"workspace": a.handle, "name": "inbox-sweep", "frequency": "hourly", "spec": {"every_seconds": 600}}));
        assert_eq!(r.status, 200, "edit schedule; {}", r.body);
        assert_eq!(row(&a, "inbox-sweep").expect("row").spec_json, r#"{"every_seconds":600}"#);
        assert_eq!(
            last_change(&a, "inbox-sweep"),
            ("schedule edited".to_string(), Some("app:carl".to_string()))
        );

        // Owner, Connect user, agent: same history, their own actor.
        let r = owner_get(port, "/cli/heartbeat/enable", &a, "name=inbox-sweep&enabled=true");
        assert_eq!(r.status, 200, "owner enable; {}", r.body);
        assert_eq!(
            last_change(&a, "inbox-sweep"),
            ("enabled".to_string(), Some("owner-token".to_string()))
        );
        owner_post(port, "/cli/users/add", r#"{"username":"connie","password":"connie-pass-123"}"#);
        let login = http(port, "POST", "/cli/auth/login", Some(r#"{"username":"connie","password":"connie-pass-123"}"#));
        assert_eq!(login.status, 200, "connect login; {}", login.body);
        let connie = login.json()["token"].as_str().expect("connect token").to_string();
        let r = http(
            port,
            "POST",
            &format!("/cli/heartbeat/enable?token={connie}"),
            Some(&serde_json::json!({"project": a.path, "name": "inbox-sweep", "enabled": false}).to_string()),
        );
        assert_eq!(r.status, 200, "Connect user POST enable (AH6 both); {}", r.body);
        assert_eq!(
            last_change(&a, "inbox-sweep"),
            ("disabled".to_string(), Some("user:connie".to_string()))
        );
        let passport = mint_passport(&a);
        let r = http(
            port,
            "GET",
            &format!("/cli/heartbeat/enable?token={passport}&name=inbox-sweep&enabled=true"),
            None,
        );
        assert_eq!(r.status, 200, "agent enable; {}", r.body);
        assert_eq!(
            last_change(&a, "inbox-sweep"),
            ("enabled".to_string(), Some(format!("agent:{}", a.handle)))
        );

        // ── R1: archive vs hard delete ───────────────────────────────────
        let r = app_post(port, &bob, "/cli/heartbeat/add", serde_json::json!({"workspace": a.handle, "name": "to-archive", "frequency": "daily", "spec": {"time": "07:00"}, "instructions": "x"}));
        assert_eq!(r.status, 200, "{}", r.body);
        let r = app_post(port, &bob, "/cli/heartbeat/archive", serde_json::json!({"workspace": a.handle, "name": "to-archive"}));
        assert_eq!(r.status, 200, "app archive; {}", r.body);
        assert_eq!(r.json(), serde_json::json!({"success": true, "name": "to-archive", "archived": true}));
        let archived = row(&a, "to-archive").expect("archive keeps the row");
        assert!(archived.archived_at.is_some(), "soft archive");
        assert!(
            std::path::Path::new(&format!("{}/.k2/heartbeats/to-archive/WAKEUP.md", a.path)).exists(),
            "archive keeps WAKEUP.md"
        );
        assert_eq!(last_change(&a, "to-archive"), ("archived".to_string(), Some("app:bob".to_string())));
        let list = app_get(port, &bob, "/cli/heartbeat/list", &format!("workspace={}", a.handle));
        assert!(!list.body.contains("to-archive"), "archived rows are not listed: {}", list.body);
        let r = app_post(port, &bob, "/cli/heartbeat/archive", serde_json::json!({"workspace": a.handle, "name": "to-archive"}));
        assert_status(&r, 404, "no_such_heartbeat", "archived looks unknown to an app");
        // The agent cannot hard delete (R1); the owner can.
        let r = http(port, "GET", &format!("/cli/heartbeat/remove?token={passport}&name=to-archive"), None);
        assert_status(&r, 403, "owner_only", "agent remove");
        assert!(row(&a, "to-archive").is_some(), "agent remove refused");
        let r = owner_get(port, "/cli/heartbeat/remove", &a, "name=to-archive");
        assert_eq!(r.status, 200, "owner remove; {}", r.body);
        assert!(row(&a, "to-archive").is_none(), "owner hard delete");
        assert_eq!(last_change(&a, "to-archive"), ("removed".to_string(), Some("owner-token".to_string())));

        // Rename keeps its history pointer on the new name.
        let r = app_post(port, &bob, "/cli/heartbeat/rename", serde_json::json!({"workspace": a.handle, "from": "inbox-sweep", "to": "inbox-check"}));
        assert_eq!(r.status, 200, "rename; {}", r.body);
        assert_eq!(r.json()["name"], "inbox-check");
        assert_eq!(
            last_change(&a, "inbox-check"),
            ("renamed from inbox-sweep".to_string(), Some("app:bob".to_string()))
        );

        // ── T10 / AH10 / AH33: history rows, trimmed and sanitized ───────
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            HeartbeatFire::insert_with_schedule(
                &conn, &a.id, Some("agent"), Some("inbox-check"), "hourly", "error",
                Some(&format!("spawn failed in {}/.k2/heartbeats/inbox-check", a.path)),
                None, None, Some(12),
            )
            .expect("scheduler-style row");
        }
        let st = app_get(port, &bob, "/cli/heartbeat/status", &format!("workspace={}&name=inbox-check&limit=500", a.handle));
        assert_eq!(st.status, 200, "status; {}", st.body);
        let rows = st.json();
        let rows = rows.as_array().expect("array");
        assert!(!rows.is_empty());
        for r in rows {
            assert_eq!(
                keys(r),
                vec!["actor", "decision", "durationMs", "firedAt", "name", "reason"],
                "{r}"
            );
        }
        let err_row = rows.iter().find(|r| r["decision"] == "error").expect("error row");
        assert_eq!(err_row["reason"], "spawn failed in ./.k2/heartbeats/inbox-check", "{err_row}");
        assert!(err_row["actor"].is_null(), "scheduler rows keep a NULL actor: {err_row}");
        assert!(!st.body.contains(&a.path), "{}", st.body);
        let fl = app_get(port, &bob, "/cli/heartbeat/fires-list", &format!("workspace={}&limit=9999", a.handle));
        assert_eq!(fl.status, 200, "fires-list; {}", fl.body);
        let fl = fl.json();
        let fl = fl.as_array().expect("array");
        assert!(fl.len() <= 200, "clamped to 200");
        assert!(fl.iter().all(|r| r["name"] != "b-only"), "room A history only");
        assert!(
            fl.iter().any(|r| r["actor"] == "user:connie"),
            "Connect actor in History"
        );

        // Owner `show` (AH9) and the owner's full status row keep `actor`.
        let r = owner_get(port, "/cli/heartbeat/show", &a, "name=inbox-check");
        assert_eq!(r.status, 200, "owner show; {}", r.body);
        assert_eq!(r.json()["instructions"], "Sweep twice.");
        assert_eq!(r.json()["wakeupPath"], ".k2/heartbeats/inbox-check/WAKEUP.md");
        let r = owner_get(port, "/cli/heartbeat/show", &a, "name=ghost");
        assert_status(&r, 404, "no_such_heartbeat", "owner show unknown");
        // AH32 owner: instructions alone through the owner route.
        let r = owner_get(port, "/cli/heartbeat/edit", &a, "name=inbox-check&instructions=Owner%20text");
        assert_eq!(r.status, 200, "owner edit instructions; {}", r.body);
        assert_eq!(
            last_change(&a, "inbox-check"),
            ("instructions edited".to_string(), Some("owner-token".to_string()))
        );
    });
}

/// T9 + AH27 + R3: fire now's gates, the in-flight refusal inside the
/// lease transaction, rate limits, and the actor on fire rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_fire_now_gates_lease_and_rate_limits() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        k2_daemon::heartbeat_app_routes::reset_rate_limits_for_tests();
        let a = seed_room("fire");
        let bob = guest(port, "fbob", &[(&a, &["heartbeats:read", "heartbeats:write"])]);
        let add = |name: &str| {
            let r = app_post(
                port,
                &bob,
                "/cli/heartbeat/add",
                serde_json::json!({"workspace": a.handle, "name": name, "frequency": "hourly",
                                   "spec": {"every_seconds": 900}, "instructions": "Ping."}),
            );
            assert_eq!(r.status, 200, "add {name}; {}", r.body);
        };
        let fire = |name: &str, extra: serde_json::Value| {
            let mut body = serde_json::json!({"workspace": a.handle, "name": name});
            if let (Some(o), Some(e)) = (body.as_object_mut(), extra.as_object()) {
                for (k, v) in e {
                    o.insert(k.clone(), v.clone());
                }
            }
            app_post(port, &bob, "/cli/heartbeat/fire", body)
        };
        add("hb");

        // Disabled: 409, and `force` is never read.
        let r = app_post(port, &bob, "/cli/heartbeat/enable", serde_json::json!({"workspace": a.handle, "name": "hb", "enabled": false}));
        assert_eq!(r.status, 200, "{}", r.body);
        let r = fire("hb", serde_json::json!({"force": true}));
        assert_status(&r, 409, "disabled", "disabled + force");
        let r = app_post(port, &bob, "/cli/heartbeat/enable", serde_json::json!({"workspace": a.handle, "name": "hb", "enabled": true}));
        assert_eq!(r.status, 200, "{}", r.body);

        // Lease held: 409 in_flight (pre-check) …
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE workspace_heartbeats SET in_flight_started_at = ?1, concurrency_policy = 'allow' \
                 WHERE project_id = ?2 AND name = 'hb'",
                params![chrono::Utc::now().to_rfc3339(), a.id],
            )
            .expect("hold lease");
            // … and AH27 inside the lease transaction: under `allow` the
            // owner's manual claim still goes through, the app's never.
            assert_eq!(
                AgentHeartbeat::try_acquire_heartbeat_checked(&conn, &a.id, "hb", LeaseCheck::App)
                    .expect("app claim"),
                LeaseOutcome::InFlight
            );
            assert_eq!(
                AgentHeartbeat::try_acquire_heartbeat_checked(&conn, &a.id, "hb", LeaseCheck::Manual)
                    .expect("manual claim"),
                LeaseOutcome::Acquired
            );
        }
        let r = fire("hb", serde_json::json!({}));
        assert_status(&r, 409, "in_flight", "lease held");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE workspace_heartbeats SET in_flight_started_at = NULL, concurrency_policy = 'forbid' \
                 WHERE project_id = ?1 AND name = 'hb'",
                params![a.id],
            )
            .expect("release lease");
        }

        // Fire now: 200, trimmed reply, actor on every fire row.
        let r = fire("hb", serde_json::json!({}));
        assert_eq!(r.status, 200, "fire; {}", r.body);
        let v = r.json();
        assert_eq!(keys(&v), vec!["decision", "name", "reason", "success"], "AH33: {v}");
        let decision = v["decision"].as_str().expect("decision");
        assert_ne!(decision, "skipped_locked", "{v}");
        assert_eq!(v["success"], true, "the pinned-chat delivery ran: {v}");
        assert!(!r.body.contains(&a.path), "{}", r.body);
        let fire_rows: Vec<HeartbeatFire> = history(&a, "hb")
            .into_iter()
            .filter(|f| f.decision != "changed")
            .collect();
        assert!(!fire_rows.is_empty(), "the fire wrote an audit row");
        for f in &fire_rows {
            assert_eq!(f.actor.as_deref(), Some("app:fbob"), "fire row actor: {f:?}");
        }

        // Second fire inside 300 s: 429 + Retry-After.
        let r = fire("hb", serde_json::json!({}));
        assert_eq!(r.status, 429, "{}", r.body);
        let v = r.json();
        assert_eq!(v["error"], "rate_limited", "{v}");
        let secs = v["retryAfterSecs"].as_u64().expect("retryAfterSecs");
        assert!(secs > 0 && secs <= 300, "{secs}");
        let header = header_value(&r.headers, "retry-after").expect("Retry-After header");
        assert_eq!(header, secs.to_string());

        // The owner is never rate limited, and its fire rows say so.
        for _ in 0..2 {
            let r = owner_get(port, "/cli/heartbeat/fire", &a, "name=hb");
            assert_eq!(r.status, 200, "owner fire; {}", r.body);
        }
        assert!(
            history(&a, "hb").iter().any(|f| f.decision != "changed" && f.actor.as_deref() == Some("owner-token")),
            "owner fire rows carry owner-token"
        );

        // Backoff and empty WAKEUP.md refuse before launching.
        add("backoff-hb");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE workspace_heartbeats SET wait_reason = 'backoff' WHERE project_id = ?1 AND name = 'backoff-hb'",
                params![a.id],
            )
            .expect("backoff");
        }
        let r = fire("backoff-hb", serde_json::json!({}));
        assert_status(&r, 409, "backoff", "backoff");
        assert!(r.json().get("nextFireAt").is_some(), "{}", r.body);
        let r = owner_get(port, "/cli/heartbeat/add", &a, "name=empty-hb&frequency=daily&spec=%7B%22time%22%3A%2207%3A00%22%7D");
        assert_eq!(r.status, 200, "owner add without a body; {}", r.body);
        let r = fire("empty-hb", serde_json::json!({}));
        assert_status(&r, 409, "wakeup_empty", "empty WAKEUP.md");

        // A scheduler-origin launch on a request thread keeps a NULL actor.
        add("sched-hb");
        let before = history(&a, "sched-hb").len();
        let out = k2_daemon::heartbeat_routes::with_request_actor(Some("owner-token".into()), || {
            k2_daemon::heartbeat_launch::smart_launch_scheduled(&a.path, "sched-hb", None, None)
        });
        assert!(out.get("decision").is_some(), "{out}");
        let rows = history(&a, "sched-hb");
        assert!(rows.len() > before, "scheduled launch wrote a row: {out}");
        for f in rows.iter().filter(|f| f.decision != "changed") {
            assert_eq!(f.actor, None, "scheduler rows keep actor NULL: {f:?}");
        }
    });
}

/// R2: `heartbeats:write` edits exactly each heartbeat's WAKEUP.md through
/// `/cli/fs/write-file`; `files:write` cannot touch `.k2/heartbeats/**`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wakeup_md_file_access_follows_r2() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let a = seed_room("files");
        let r = owner_get(
            port,
            "/cli/heartbeat/add",
            &a,
            "name=digest&frequency=daily&spec=%7B%22time%22%3A%2207%3A00%22%7D&instructions=Old",
        );
        assert_eq!(r.status, 200, "{}", r.body);
        let both = guest(port, "gboth", &[(&a, &["files:read", "files:write", "heartbeats:write"])]);
        let files_only = guest(port, "gfiles", &[(&a, &["files:read", "files:write"])]);
        let hb_only = guest(port, "ghb", &[(&a, &["heartbeats:write"])]);
        let write = |tok: &str, path: &str, content: &str| {
            app_post(
                port,
                tok,
                "/cli/fs/write-file",
                serde_json::json!({"workspace": a.handle, "path": path, "content": content}),
            )
        };
        let wakeup = format!("{}/.k2/heartbeats/digest/WAKEUP.md", a.path);
        std::fs::create_dir_all(format!("{}/notes", a.path)).expect("notes dir");

        // files:write alone: no WAKEUP.md, no other heartbeat file.
        let r = write(&files_only, ".k2/heartbeats/digest/WAKEUP.md", "hijack");
        assert_status(&r, 403, "missing capability heartbeats:write", "files:write on WAKEUP.md");
        let r = write(&files_only, ".k2/heartbeats/digest/notes.md", "x");
        assert_status(&r, 403, "heartbeats_area", "files:write elsewhere in heartbeats");
        let r = write(&files_only, ".K2/Heartbeats/digest/notes.md", "x");
        assert_status(&r, 403, "heartbeats_area", "case variant");
        assert!(std::fs::read_to_string(&wakeup).expect("wakeup").contains("Old"));
        // Ordinary files still work with files:write.
        let r = write(&files_only, "notes/today.md", "ok");
        assert_eq!(r.status, 200, "ordinary write; {}", r.body);

        // heartbeats:write alone edits exactly the WAKEUP.md.
        let r = write(&hb_only, ".k2/heartbeats/digest/WAKEUP.md", "---\ndescription:\n---\n\nNew body\n");
        assert_eq!(r.status, 200, "heartbeats:write WAKEUP.md; {}", r.body);
        assert!(std::fs::read_to_string(&wakeup).expect("wakeup").contains("New body"));
        let h = history(&a, "digest");
        let c = h.iter().find(|f| f.decision == "changed").expect("changed row");
        assert_eq!(c.reason.as_deref(), Some("instructions edited"));
        assert_eq!(c.actor.as_deref(), Some("app:ghb"));
        let r = write(&hb_only, ".k2/heartbeats/digest/notes.md", "x");
        assert_status(&r, 403, "heartbeats_area", "not the WAKEUP.md");
        let r = write(&hb_only, ".k2/heartbeats/ghost/WAKEUP.md", "x");
        assert_status(&r, 404, "no_such_heartbeat", "no such heartbeat");
        assert!(!std::path::Path::new(&format!("{}/.k2/heartbeats/ghost", a.path)).exists());
        let r = write(&hb_only, "notes/other.md", "x");
        assert_status(&r, 403, "missing capability files:write", "heartbeats:write is not files:write");

        // Every other mutating fs route refuses heartbeat ground, even
        // with both caps.
        let r = app_post(port, &both, "/cli/fs/create", serde_json::json!({"workspace": a.handle, "path": ".k2/heartbeats/new-hb", "is_directory": true}));
        assert_status(&r, 403, "heartbeats_area", "create");
        let r = app_post(port, &both, "/cli/fs/rename", serde_json::json!({"workspace": a.handle, "path": ".k2", "new_name": "k2-moved"}));
        assert_status(&r, 403, "heartbeats_area", "rename .k2");
        let r = app_post(port, &both, "/cli/fs/rename", serde_json::json!({"workspace": a.handle, "path": "notes/today.md", "new_name": "x"}));
        assert_eq!(r.status, 200, "ordinary rename; {}", r.body);
        let r = app_post(port, &both, "/cli/fs/move", serde_json::json!({"workspace": a.handle, "sources": [".k2/heartbeats/digest"], "destination": "notes"}));
        assert_status(&r, 403, "heartbeats_area", "move a heartbeat folder out");
        let r = app_post(port, &both, "/cli/fs/copy", serde_json::json!({"workspace": a.handle, "sources": ["notes"], "destination": ".k2/heartbeats"}));
        assert_status(&r, 403, "heartbeats_area", "copy into heartbeats");
        let r = app_post(port, &both, "/cli/fs/upload-binary", serde_json::json!({"workspace": a.handle, "dir": ".k2/heartbeats/digest", "filename": "x.bin", "base64": "AAAA"}));
        assert_status(&r, 403, "heartbeats_area", "upload into heartbeats");
        // A symlink inside the room cannot reach the folder.
        std::os::unix::fs::symlink(format!("{}/.k2/heartbeats", a.path), format!("{}/hb-link", a.path))
            .expect("symlink");
        let r = write(&both, "hb-link/digest/notes.md", "x");
        assert_status(&r, 403, "heartbeats_area", "symlinked heartbeats folder");
        assert!(!std::path::Path::new(&format!("{}/.k2/heartbeats/digest/notes.md", a.path)).exists());
        // Reading the body stays on files:read.
        let r = app_get(port, &files_only, "/cli/fs/read-file", &format!("workspace={}&path=.k2/heartbeats/digest/WAKEUP.md", a.handle));
        assert_eq!(r.status, 200, "files:read reads WAKEUP.md; {}", r.body);
        assert!(r.body.contains("New body"), "{}", r.body);
    });
}
