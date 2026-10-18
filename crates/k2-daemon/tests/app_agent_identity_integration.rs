//! App agent identity surface (0.45.5) through a real in-process daemon:
//! `GET|POST /cli/agent/identity`, `POST /cli/agent/handle`, the two caps
//! (`agent:identity`, `agent:handle`), the room jail, daemon-side checks,
//! the owner paths (unchanged), the `agent_identity_changed` event, and
//! the audit lines.
//!
//! 0.46.1: an agent's own passport (`k2 agent identity`) sets its own
//! color and picture, and nothing else (`agent_passport_*` below).
//!
//! Headless, temp HOME (the audit file and every AGENT.md land there or in
//! temp room folders). No agent is spawned.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_events::SessionEvent;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-agent-identity";

/// A real 1×1 PNG.
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

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

/// Temp HOME for the whole test; restored (and removed) on panic too.
fn with_temp_home<F: FnOnce()>(f: F) {
    struct Restore {
        prev: Option<std::ffi::OsString>,
        tmp: PathBuf,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
    let tmp = std::env::temp_dir().join(format!(
        "k2-agent-identity-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&tmp).expect("temp home");
    let _restore = Restore {
        prev: std::env::var_os("HOME"),
        tmp: tmp.clone(),
    };
    std::env::set_var("HOME", &tmp);
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

fn seed_room(tag: &str) -> Room {
    let id = uuid::Uuid::new_v4().to_string();
    let handle = format!("{tag}{}", &id[..8]);
    let dir = std::env::temp_dir().join(format!("k2-agent-identity-room-{handle}"));
    std::fs::create_dir_all(&dir).expect("room dir");
    let path = dir.canonicalize().expect("canon").to_string_lossy().into_owned();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle, agent_enabled) VALUES (?1, ?2, ?3, ?2, 1)",
        params![id, handle, path],
    )
    .expect("project");
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

fn assert_status(r: &Resp, status: u16, needle: &str, label: &str) {
    assert_eq!(r.status, status, "{label}; {}", r.body);
    assert!(r.body.contains(needle), "{label}: body must contain {needle:?}: {}", r.body);
}

fn project_row(room: &Room) -> (String, String, String, Option<String>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT name, handle, color, icon_url FROM projects WHERE id = ?1",
        params![room.id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .expect("project row")
}

/// Every `agent_identity_changed` on the bus right now, as
/// `(projectId, fields)`. A lagged receiver fails loudly.
fn drain_identity_events(
    rx: &mut tokio::sync::broadcast::Receiver<SessionEvent>,
) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(SessionEvent::AgentIdentityChanged { project_id, fields, .. }) => out.push((project_id, fields)),
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(e) => panic!("session bus recv error: {e:?}"),
        }
    }
    out
}

fn one_event(id: &str, fields: &[&str]) -> Vec<(String, Vec<String>)> {
    vec![(id.to_string(), fields.iter().map(|f| f.to_string()).collect())]
}

fn audit_lines() -> Vec<serde_json::Value> {
    k2_core::auth_audit::tail(1000).expect("audit tail")
}

fn mint_passport(room: &Room) -> String {
    mint_passport_as(room, "identity-agent")
}

fn mint_passport_as(room: &Room, agent_address: &str) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal {
            workspace_uuid: room.id.clone(),
            agent_address: agent_address.to_string(),
        },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

/// The session id a passport (`<sid>.<secret>`) is bound to.
fn passport_session(passport: &str) -> &str {
    passport.split_once('.').expect("passport shape <sid>.<secret>").0
}

fn agent_post(port: u16, passport: &str, body: serde_json::Value) -> Resp {
    http(port, "POST", &format!("/cli/agent/identity?token={passport}"), Some(&body.to_string()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_agent_identity_caps_jail_checks_handle_events_and_audit() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let a = seed_room("penny");
        let b = seed_room("other");

        let plain = guest(port, "plainuser", &[(&a, &["thread:read"])]);
        let ida = guest(port, "idauser", &[(&a, &["thread:read", "agent:identity"]), (&b, &["thread:read"])]);
        let hda = guest(port, "hduser", &[(&a, &["agent:handle"])]);
        let mut rx = k2_daemon::session_events::subscribe();

        // ── Read: the room is enough; caps are reported, never assumed.
        let r = app_get(port, &plain, "/cli/agent/identity", &format!("workspace={}", a.handle));
        assert_eq!(r.status, 200, "plain read; {}", r.body);
        let v = r.json();
        assert_eq!(v["handle"], a.handle.as_str());
        assert_eq!(v["workspace"], a.handle.as_str());
        assert_eq!(v["displayName"], a.handle.as_str());
        assert_eq!(v["color"], "#3b82f6", "the default color");
        assert!(v["image"].is_null(), "{v}");
        assert_eq!(v["canEditIdentity"], false);
        assert_eq!(v["canEditHandle"], false);
        let mut keys: Vec<&str> = v.as_object().expect("object").keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["canEditHandle", "canEditIdentity", "color", "displayName", "handle", "image", "workspace"],
            "no path or id on the wire"
        );
        assert!(!r.body.contains(&a.path), "no path: {}", r.body);
        let r = app_get(port, &ida, "/cli/agent/identity", &format!("workspace={}", a.id));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.json()["canEditIdentity"], true);
        assert_eq!(r.json()["canEditHandle"], false);

        // ── Jail: another room is skin_room, for reads and writes.
        let r = app_get(port, &plain, "/cli/agent/identity", &format!("workspace={}", b.handle));
        assert_status(&r, 403, "skin_room", "plain on room B");
        let r = app_post(port, &ida, "/cli/agent/identity", serde_json::json!({"workspace": b.handle, "displayName": "Nope"}));
        assert_status(&r, 403, "missing capability agent:identity", "ida holds B without the cap");
        let r = app_get(port, &plain, "/cli/agent/identity", "workspace=no-such-room");
        assert_status(&r, 403, "skin_room", "unknown room is skin_room (no oracle)");
        let r = app_get(port, &plain, "/cli/agent/identity", "");
        assert_eq!(r.status, 400, "missing workspace; {}", r.body);

        // ── Caps: thread-only and handle-only can't set identity; identity can't set the handle.
        let set_name = serde_json::json!({"workspace": a.handle, "displayName": "Penny"});
        let r = app_post(port, &plain, "/cli/agent/identity", set_name.clone());
        assert_status(&r, 403, "missing capability agent:identity", "thread-only");
        let r = app_post(port, &hda, "/cli/agent/identity", set_name.clone());
        assert_status(&r, 403, "missing capability agent:identity", "handle does not imply identity");
        let r = app_post(port, &ida, "/cli/agent/handle", serde_json::json!({"workspace": a.handle, "handle": "x"}));
        assert_status(&r, 403, "missing capability agent:handle", "identity does not imply handle");
        assert_eq!(project_row(&a).0, a.handle, "nothing written by refusals");
        assert!(drain_identity_events(&mut rx).is_empty(), "refusals emit nothing");

        // ── Invalid inputs: checked whole, before any write.
        for (body, code, label) in [
            (serde_json::json!({"workspace": a.handle}), "nothing_to_change", "empty"),
            (serde_json::json!({"workspace": a.handle, "displayName": "a/b"}), "bad_name", "slash"),
            (serde_json::json!({"workspace": a.handle, "displayName": "x".repeat(65)}), "bad_name", "too long"),
            (serde_json::json!({"workspace": a.handle, "displayName": "AGENT.md"}), "bad_name", "persona file"),
            (serde_json::json!({"workspace": a.handle, "displayName": 7}), "usage", "not a string"),
            (serde_json::json!({"workspace": a.handle, "color": "red"}), "bad_color", "named color"),
            (serde_json::json!({"workspace": a.handle, "displayName": "Valid", "color": "#12345"}), "bad_color", "mixed body"),
            (
                serde_json::json!({"workspace": a.handle, "image": "data:image/svg+xml;base64,PHN2Zz48c2NyaXB0PmFsZXJ0KDEpPC9zY3JpcHQ+PC9zdmc+"}),
                "type_refused",
                "svg",
            ),
            (
                serde_json::json!({"workspace": a.handle, "image": "data:image/png;base64,PGh0bWw+PHNjcmlwdD54PC9zY3JpcHQ+PC9odG1sPg=="}),
                "type_mismatch",
                "html as png",
            ),
            (serde_json::json!({"workspace": a.handle, "image": "https://example.com/a.png"}), "not_data_url", "url"),
        ] {
            let r = app_post(port, &ida, "/cli/agent/identity", body);
            assert_eq!(r.status, 400, "{label}; {}", r.body);
            assert_eq!(r.json()["error"], code, "{label}; {}", r.body);
        }
        let r = http(port, "POST", &format!("/cli/agent/identity?token={ida}"), Some("not json"));
        assert_status(&r, 400, "usage", "bad JSON");
        let (name, handle, color, icon) = project_row(&a);
        assert_eq!((name.as_str(), handle.as_str(), color.as_str(), icon.as_deref()), (a.handle.as_str(), a.handle.as_str(), "#3b82f6", None), "no partial write");
        assert!(drain_identity_events(&mut rx).is_empty(), "refusals emit nothing");

        // ── A good write: name + color + picture.
        let r = app_post(
            port,
            &ida,
            "/cli/agent/identity",
            serde_json::json!({
                "workspace": a.handle,
                "displayName": "Penny",
                "color": "#ABC",
                "image": format!("data:image/png;base64,{PNG_1X1}"),
            }),
        );
        assert_eq!(r.status, 200, "good write; {}", r.body);
        let v = r.json();
        assert_eq!(v["ok"], true);
        assert_eq!(v["changed"], serde_json::json!(["displayName", "color", "image"]));
        assert_eq!(v["displayName"], "Penny");
        assert_eq!(v["color"], "#aabbcc");
        let img = v["image"].as_str().expect("image").to_string();
        assert!(img.starts_with("data:image/png;base64,"), "{img}");
        let (name, _, color, icon) = project_row(&a);
        assert_eq!(name, "Penny");
        assert_eq!(color, "#aabbcc");
        assert_eq!(icon.as_deref(), Some(img.as_str()), "the daemon's re-encoded copy is stored");
        let persona = std::fs::read_to_string(k2_core::workspace::agent_identity::workspace_agent_md_path(&a.path)).expect("persona file");
        assert!(persona.contains("display_name: Penny"), "{persona}");
        assert_eq!(
            drain_identity_events(&mut rx),
            one_event(&a.id, &["displayName", "name", "color", "image"]),
            "one agent_identity_changed for the whole request"
        );

        // Same values again: nothing changes, nothing is emitted.
        let r = app_post(port, &ida, "/cli/agent/identity", serde_json::json!({"workspace": a.handle, "displayName": "Penny", "color": "#aabbcc"}));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.json()["changed"], serde_json::json!([]));
        assert!(drain_identity_events(&mut rx).is_empty(), "a no-op emits nothing");

        // skin/agents carries the look for apps.
        let r = app_get(port, &ida, "/cli/skin/agents", "");
        assert_eq!(r.status, 200, "{}", r.body);
        let rows = r.json()["agents"].as_array().expect("agents").clone();
        let row = rows.iter().find(|x| x["projectId"] == a.id.as_str()).expect("room A row");
        assert_eq!(row["displayName"], "Penny");
        assert_eq!(row["color"], "#aabbcc");
        assert_eq!(row["image"], img.as_str());
        let row_b = rows.iter().find(|x| x["projectId"] == b.id.as_str()).expect("room B row");
        assert_eq!(row_b["color"], "#3b82f6");
        assert!(row_b["image"].is_null(), "{row_b}");

        // An SVG the owner set never reaches an app.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE projects SET icon_url = 'data:image/svg+xml;base64,PHN2Zy8+' WHERE id = ?1",
                params![b.id],
            )
            .expect("owner svg");
        }
        let r = app_get(port, &ida, "/cli/skin/agents", "");
        let rows = r.json()["agents"].as_array().expect("agents").clone();
        let row_b = rows.iter().find(|x| x["projectId"] == b.id.as_str()).expect("room B row");
        assert!(row_b["image"].is_null(), "svg hidden from apps: {row_b}");
        let r = app_get(port, &ida, "/cli/agent/identity", &format!("workspace={}", b.handle));
        assert!(r.json()["image"].is_null(), "svg hidden: {}", r.body);

        // Clear the picture with null.
        let r = app_post(port, &ida, "/cli/agent/identity", serde_json::json!({"workspace": a.handle, "image": null}));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.json()["changed"], serde_json::json!(["image"]));
        assert!(r.json()["image"].is_null());
        assert_eq!(project_row(&a).3, None);
        assert_eq!(drain_identity_events(&mut rx), one_event(&a.id, &["image"]), "a look-only write emits once");

        // ── Handle: its own cap, the owner's rules, old handle kept as an alias.
        let old_handle = a.handle.clone();
        let r = http(port, "GET", &format!("/cli/agent/handle?token={hda}&workspace={old_handle}"), None);
        assert_eq!(r.status, 405, "handle is POST only; {}", r.body);
        let r = app_post(port, &hda, "/cli/agent/handle", serde_json::json!({"workspace": old_handle, "handle": "a/b"}));
        assert_status(&r, 400, "bad_handle", "slash");
        let r = app_post(port, &hda, "/cli/agent/handle", serde_json::json!({"workspace": old_handle, "handle": "x".repeat(65)}));
        assert_status(&r, 400, "bad_handle", "too long");
        let r = app_post(port, &hda, "/cli/agent/handle", serde_json::json!({"workspace": old_handle, "handle": b.handle}));
        assert_status(&r, 409, "handle_taken", "collision");
        assert!(!r.body.contains("workspace '"), "never names the other agent: {}", r.body);
        // A removed agent's handle is never reused (0159), and the refusal
        // never names whose it was.
        let gone = seed_room("gone");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("DELETE FROM projects WHERE id = ?1", params![gone.id]).expect("remove workspace");
        }
        let r = app_post(port, &hda, "/cli/agent/handle", serde_json::json!({"workspace": old_handle, "handle": gone.handle}));
        assert_status(&r, 409, "handle_retired", "retired handle");
        assert!(!r.body.contains(&gone.id), "{}", r.body);
        assert_eq!(project_row(&a).1, old_handle, "refusals write nothing");
        assert!(drain_identity_events(&mut rx).is_empty(), "refusals emit nothing");

        let r = app_post(port, &hda, "/cli/agent/handle", serde_json::json!({"workspace": old_handle, "handle": "Pennys Desk"}));
        assert_eq!(r.status, 200, "handle write; {}", r.body);
        let v = r.json();
        assert_eq!(v["handle"], "pennys-desk");
        assert_eq!(v["workspace"], "pennys-desk");
        assert_eq!(v["previousHandle"], old_handle.as_str());
        assert_eq!(v["changed"], serde_json::json!(["handle"]));
        assert_eq!(project_row(&a).1, "pennys-desk");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let aliases = k2_core::workspace::handle::aliases_for(&conn, &a.id);
            assert!(aliases.contains(&old_handle), "old handle kept as an alias: {aliases:?}");
        }
        assert_eq!(drain_identity_events(&mut rx), one_event(&a.id, &["handle"]));
        // The pass still reaches its room under the new handle.
        let r = app_get(port, &hda, "/cli/agent/identity", "workspace=pennys-desk");
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.json()["canEditHandle"], true);

        // ── Owner paths are unchanged, and now nudge apps too.
        let r = http(
            port,
            "POST",
            &format!("/cli/workspace/set-agent-display-name?token={OWNER_TOKEN}&project={}&name=Owner%20Named", a.path),
            Some(""),
        );
        assert_eq!(r.status, 200, "owner rename; {}", r.body);
        assert_eq!(r.json(), serde_json::json!({"success": true, "display_name": "Owner Named"}));
        assert_eq!(project_row(&a).0, "Owner Named");
        assert_eq!(drain_identity_events(&mut rx), one_event(&a.id, &["displayName", "name"]), "owner rename: one event");
        let r = http(
            port,
            "POST",
            &format!("/cli/workspace/set-handle?token={OWNER_TOKEN}"),
            Some(&serde_json::json!({"project": a.path, "handle": "owner-desk"}).to_string()),
        );
        assert_eq!(r.status, 200, "owner set-handle; {}", r.body);
        assert_eq!(r.json()["handle"], "owner-desk");
        assert_eq!(drain_identity_events(&mut rx), one_event(&a.id, &["handle"]), "owner set-handle: one event");
        let r = http(
            port,
            "POST",
            &format!("/cli/projects/update?token={OWNER_TOKEN}"),
            Some(&serde_json::json!({"id": a.id, "color": "#010203"}).to_string()),
        );
        assert_eq!(r.status, 200, "owner color; {}", r.body);
        assert_eq!(drain_identity_events(&mut rx), one_event(&a.id, &["color"]));

        // The owner may use the new route with a path, and gets the raw picture.
        let r = http(
            port,
            "POST",
            &format!("/cli/agent/identity?token={OWNER_TOKEN}"),
            Some(&serde_json::json!({"workspace": a.path, "color": "#112233"}).to_string()),
        );
        assert_eq!(r.status, 200, "owner on the new route; {}", r.body);
        assert_eq!(r.json()["canEditHandle"], true);
        assert_eq!(r.json()["color"], "#112233");
        let r = http(port, "GET", &format!("/cli/agent/identity?token={OWNER_TOKEN}&workspace={}", b.id), None);
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.json()["image"], "data:image/svg+xml;base64,PHN2Zy8+", "owner sees the stored icon");

        // ── Other callers: none, a revoked-looking pass, an agent passport.
        let r = http(port, "GET", &format!("/cli/agent/identity?workspace={}", b.handle), None);
        assert_eq!(r.status, 403, "no token; {}", r.body);
        let r = http(port, "GET", &format!("/cli/agent/identity?token=k2skn_not-a-pass&workspace={}", b.handle), None);
        assert_eq!(r.status, 401, "unknown pass; {}", r.body);
        let passport = mint_passport(&a);
        let r = http(
            port,
            "POST",
            &format!("/cli/agent/identity?token={passport}"),
            Some(&serde_json::json!({"workspace": a.id, "displayName": "Self Named"}).to_string()),
        );
        assert_status(&r, 403, "agent_cannot_rename", "an agent passport never renames (0.46.1)");
        assert_ne!(project_row(&a).0, "Self Named");

        // ── Audit: who, which agent, old → new; never the picture bytes.
        let lines = audit_lines();
        let ident: Vec<&serde_json::Value> = lines.iter().filter(|l| l["event"] == "agent.identity.set").collect();
        let by_app = ident
            .iter()
            .find(|l| l["user"] == "app:idauser" && l["outcome"].as_str().is_some_and(|o| o.contains("Penny")))
            .unwrap_or_else(|| panic!("app identity audit line: {ident:?}"));
        let outcome = by_app["outcome"].as_str().expect("outcome");
        assert!(outcome.contains(&format!("agent={}", a.id)), "{outcome}");
        assert!(outcome.contains(&format!("displayName={:?}->\"Penny\"", a.handle)), "{outcome}");
        assert!(outcome.contains("color=#3b82f6->#aabbcc"), "{outcome}");
        assert!(outcome.contains("image=none->sha256:"), "{outcome}");
        assert_eq!(by_app["client"], "app");
        assert!(ident.iter().any(|l| l["user"] == "owner-token"), "owner route audited too: {ident:?}");
        let handle_line = lines
            .iter()
            .find(|l| l["event"] == "agent.handle.set" && l["user"] == "app:hduser")
            .unwrap_or_else(|| panic!("handle audit line: {lines:?}"));
        assert!(
            handle_line["outcome"].as_str().is_some_and(|o| o.contains(&format!("handle={old_handle}->pennys-desk"))),
            "{handle_line}"
        );
        for l in &lines {
            let o = l["outcome"].as_str().unwrap_or("");
            assert!(!o.contains("iVBOR") && !o.contains("base64"), "no picture bytes in audit: {o}");
        }
    });
}

/// A body over the cap is refused before it is read in full.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_agent_identity_body_cap_is_413() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let a = seed_room("cap");
        let ida = guest(port, "capuser", &[(&a, &["agent:identity"])]);
        let big = "A".repeat(k2_daemon::agent_identity_routes::MAX_BODY + 1024);
        let body = format!(r#"{{"workspace":"{}","image":"data:image/png;base64,{big}"}}"#, a.handle);
        let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(20))).expect("timeout");
        let head = format!(
            "POST /cli/agent/identity?token={ida} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).expect("head");
        // The daemon answers from the head alone; the rest may be refused.
        let _ = stream.write_all(body.as_bytes());
        let mut raw = Vec::new();
        let _ = stream.read_to_end(&mut raw);
        let text = String::from_utf8_lossy(&raw);
        assert!(text.starts_with("HTTP/1.1 413"), "{text}");
        assert_eq!(project_row(&a).3, None, "nothing stored");
    });
}

/// 0.46.1 `k2 agent identity`: an agent's passport sets its OWN color and
/// picture through the same checks, writes, event and audit; it is refused
/// for another workspace (one answer, no oracle), for a name or handle, for
/// SVG and oversize pictures, and past the per-workspace budget. Passports
/// this daemon did not mint and API cells are refused. Owner unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_passport_sets_own_color_and_picture_only() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let a = seed_room("selfie");
        let b = seed_room("neighbor");
        let pa = mint_passport(&a);
        let mut rx = k2_daemon::session_events::subscribe();

        // ── Read: no workspace named means itself.
        let r = http(port, "GET", &format!("/cli/agent/identity?token={pa}"), None);
        assert_eq!(r.status, 200, "own read; {}", r.body);
        let v = r.json();
        assert_eq!(v["handle"], a.handle.as_str(), "{v}");
        assert_eq!(v["canEditIdentity"], true, "{v}");
        assert_eq!(v["canEditHandle"], false, "{v}");
        assert!(!r.body.contains(&a.path), "no path: {}", r.body);
        // Naming itself works too.
        let r = http(port, "GET", &format!("/cli/agent/identity?token={pa}&workspace={}", a.handle), None);
        assert_eq!(r.status, 200, "own read by handle; {}", r.body);

        // ── Another workspace, real or not: one 403, the same body.
        let other = http(port, "GET", &format!("/cli/agent/identity?token={pa}&workspace={}", b.handle), None);
        assert_status(&other, 403, "not_your_agent", "read another agent");
        let missing = http(port, "GET", &format!("/cli/agent/identity?token={pa}&workspace=no-such-agent"), None);
        assert_status(&missing, 403, "not_your_agent", "read a missing agent");
        assert_eq!(other.body, missing.body, "no oracle between another agent and none");
        let before_b = project_row(&b);
        let r = agent_post(port, &pa, serde_json::json!({"workspace": b.handle, "color": "#654321"}));
        assert_status(&r, 403, "not_your_agent", "write another agent");
        let r = agent_post(port, &pa, serde_json::json!({"workspace": b.path, "color": "#654321"}));
        assert_status(&r, 403, "not_your_agent", "write another agent by path");
        assert_eq!(project_row(&b), before_b, "nothing written to another agent");

        // ── Name and handle are refused, with the hint, before any write.
        for body in [
            serde_json::json!({"displayName": "Self Named"}),
            serde_json::json!({"display_name": "Self Named", "color": "#111111"}),
            serde_json::json!({"handle": "self-named"}),
        ] {
            let r = agent_post(port, &pa, body.clone());
            assert_status(&r, 403, "agent_cannot_rename", &format!("rename {body}"));
            assert!(r.body.contains("ask your owner"), "{}", r.body);
        }
        let r = http(
            port,
            "POST",
            &format!("/cli/agent/handle?token={pa}"),
            Some(&serde_json::json!({"handle": "self-named"}).to_string()),
        );
        assert_status(&r, 403, "agent_cannot_rename", "handle door");

        // ── Pictures: SVG and oversize are refused by the same checks.
        let r = agent_post(
            port,
            &pa,
            serde_json::json!({"image": "data:image/svg+xml;base64,PHN2Zz48c2NyaXB0PmFsZXJ0KDEpPC9zY3JpcHQ+PC9zdmc+"}),
        );
        assert_eq!(r.status, 400, "svg; {}", r.body);
        assert_eq!(r.json()["error"], "type_refused", "{}", r.body);
        let huge = "A".repeat(k2_core::workspace::agent_look::MAX_INPUT_IMAGE_BYTES / 3 * 4 + 400);
        let r = agent_post(port, &pa, serde_json::json!({"image": format!("data:image/png;base64,{huge}")}));
        assert_eq!(r.status, 400, "oversize; {}", r.body);
        assert_eq!(r.json()["error"], "too_large", "{}", r.body);
        let r = agent_post(port, &pa, serde_json::json!({"color": "teal"}));
        assert_eq!(r.status, 400, "bad color; {}", r.body);
        assert_eq!(r.json()["error"], "bad_color", "{}", r.body);
        let (name, handle, color, icon) = project_row(&a);
        assert_eq!(
            (name.as_str(), handle.as_str(), color.as_str(), icon.as_deref()),
            (a.handle.as_str(), a.handle.as_str(), "#3b82f6", None),
            "refusals write nothing"
        );
        assert!(drain_identity_events(&mut rx).is_empty(), "refusals emit nothing");

        // ── A good write: its own color and picture, one event.
        let r = agent_post(
            port,
            &pa,
            serde_json::json!({"color": "#ABC", "image": format!("data:image/png;base64,{PNG_1X1}")}),
        );
        assert_eq!(r.status, 200, "own write; {}", r.body);
        let v = r.json();
        assert_eq!(v["changed"], serde_json::json!(["color", "image"]), "{v}");
        assert_eq!(v["color"], "#aabbcc");
        let img = v["image"].as_str().expect("image").to_string();
        assert!(img.starts_with("data:image/png;base64,"), "{img}");
        let (name, _, color, icon) = project_row(&a);
        assert_eq!(name, a.handle, "the name is untouched");
        assert_eq!(color, "#aabbcc");
        assert_eq!(icon.as_deref(), Some(img.as_str()));
        assert_eq!(drain_identity_events(&mut rx), one_event(&a.id, &["color", "image"]));
        // Clear the picture, naming itself.
        let r = agent_post(port, &pa, serde_json::json!({"workspace": a.handle, "image": null}));
        assert_eq!(r.status, 200, "clear; {}", r.body);
        assert_eq!(r.json()["changed"], serde_json::json!(["image"]));
        assert_eq!(project_row(&a).3, None);
        assert_eq!(drain_identity_events(&mut rx), one_event(&a.id, &["image"]));

        // ── Audit: actor agent:<workspace>/<session>, client agent.
        let actor = format!("agent:{}/{}", a.handle, passport_session(&pa));
        let lines = audit_lines();
        let mine: Vec<&serde_json::Value> = lines
            .iter()
            .filter(|l| l["event"] == "agent.identity.set" && l["user"] == actor.as_str())
            .collect();
        assert_eq!(mine.len(), 2, "one line per change; {lines:?}");
        let first = mine
            .iter()
            .find(|l| l["outcome"].as_str().is_some_and(|o| o.contains("color=")))
            .unwrap_or_else(|| panic!("the color+picture line: {mine:?}"));
        let outcome = first["outcome"].as_str().expect("outcome");
        assert!(outcome.contains(&format!("agent={}", a.id)), "{outcome}");
        assert!(outcome.contains("color=#3b82f6->#aabbcc"), "{outcome}");
        assert!(outcome.contains("image=none->sha256:"), "{outcome}");
        assert!(!outcome.contains("base64"), "no picture bytes: {outcome}");
        assert!(mine.iter().all(|l| l["client"] == "agent"), "{mine:?}");

        // ── Passports this daemon did not mint, and API cells, are refused.
        let r = agent_post(
            port,
            "0f0f0f0f-0000-4000-8000-000000000000.not-a-secret",
            serde_json::json!({"color": "#222222"}),
        );
        assert_eq!(r.status, 403, "foreign passport; {}", r.body);
        let api = mint_passport_as(&a, "api-cell-7");
        let r = agent_post(port, &api, serde_json::json!({"color": "#222222"}));
        assert_status(&r, 403, "wrong_credential", "api cell");
        assert_eq!(project_row(&a).2, "#aabbcc", "nothing written by refused passports");

        // ── Rate limit: one budget per workspace, shared by its sessions.
        // Two writes are spent; the rest of the budget, then 429.
        let max = k2_daemon::agent_identity_routes::WRITES_PER_PASS_MAX;
        for i in 2..max {
            let r = agent_post(port, &pa, serde_json::json!({"color": format!("#0000{:02x}", i)}));
            assert_eq!(r.status, 200, "write {i}; {}", r.body);
        }
        let r = agent_post(port, &pa, serde_json::json!({"color": "#ff0000"}));
        assert_status(&r, 429, "rate_limited", "over the budget");
        let pa2 = mint_passport(&a);
        let r = agent_post(port, &pa2, serde_json::json!({"color": "#ff0000"}));
        assert_status(&r, 429, "rate_limited", "a second session of the same workspace shares it");
        assert_ne!(project_row(&a).2, "#ff0000", "a 429 writes nothing");
        let pb = mint_passport(&b);
        let r = agent_post(port, &pb, serde_json::json!({"color": "#00ff00"}));
        assert_eq!(r.status, 200, "another workspace has its own budget; {}", r.body);
        assert_eq!(project_row(&b).2, "#00ff00");

        // ── Owner behaviour unchanged: any agent, the name too, no budget.
        let r = http(
            port,
            "POST",
            &format!("/cli/agent/identity?token={OWNER_TOKEN}"),
            Some(
                &serde_json::json!({"workspace": a.handle, "displayName": "Owner Picked", "color": "#ff0000"})
                    .to_string(),
            ),
        );
        assert_eq!(r.status, 200, "owner after the agent's budget; {}", r.body);
        assert_eq!(r.json()["canEditHandle"], true);
        let (name, _, color, _) = project_row(&a);
        assert_eq!((name.as_str(), color.as_str()), ("Owner Picked", "#ff0000"));
        assert!(
            audit_lines().iter().any(|l| l["event"] == "agent.identity.set"
                && l["user"] == "owner-token"
                && l["outcome"].as_str().is_some_and(|o| o.contains("Owner Picked"))),
            "owner audited as before"
        );
    });
}
