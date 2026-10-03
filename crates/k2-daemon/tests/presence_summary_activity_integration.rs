//! Home 0.43.2 (prd-home-seamless-0432 Z23 / Z31, T4.5) — busy agents on
//! `GET /cli/presence/summary`.
//!
//! A Home row on another server reads this summary. It now carries
//! `agentActivity` (registered workspaces with a working or waiting agent)
//! and `activeProjectIds` (the canonical Active set). The test drives the
//! REAL sources through the REAL dispatcher:
//!   - a real `cat` PTY registered in the v2 map; a braille title written
//!     through it reaches the session-activity observer (`working`);
//!   - a real `/hook/complete` call with that session's id as `paneId`
//!     (the v2 session id, vs-live Z30) through the daemon's broadcast sink
//!     (`permission`, newer, wins);
//!   - unregistering the session drops it.
//! `online` and `workspaces` keep their shape for older clients.
//!
//! ISOLATION: `$HOME`, the shared DB, the v2 map and the hook sink are
//! process-wide. One test in this binary.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession};
use k2_daemon::{events, test_harness, v2_session_map};

const OWNER_TOKEN: &str = "owner-token-summary-activity-0432";

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    let req = format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");
    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(r) = try_parse(&raw) {
            return r;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    try_parse(&raw).unwrap_or_else(|| panic!("incomplete response: {:?}", String::from_utf8_lossy(&raw)))
}

fn try_parse(raw: &[u8]) -> Option<Resp> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())?;
    let clen = headers.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    })?;
    if body.len() < clen {
        return None;
    }
    Some(Resp { status, body: body.to_string() })
}

fn summary(port: u16) -> serde_json::Value {
    let r = http(port, "GET", &format!("/cli/presence/summary?token={OWNER_TOKEN}"));
    assert_eq!(r.status, 200, "summary GET must be 200; body={}", r.body);
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("summary is JSON ({e}): {}", r.body))
}

/// Poll the summary until `agentActivity` equals `want`, or fail with the
/// last body.
fn await_activity(port: u16, want: serde_json::Value, context: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let v = summary(port);
        if v["agentActivity"] == want {
            return v;
        }
        assert!(Instant::now() < deadline, "{context}: agentActivity never became {want}; last={v}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs() as i64
}

fn seed_project(id: &str, path: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects \
         (id, path, name, color, agent_mode, pinned, tab_order, manually_active, last_interaction_at) \
         VALUES (?1, ?2, ?3, '#123456', 'off', 0, 0, 0, ?4)",
        rusqlite::params![id, path, "summary-activity", now_secs()],
    )
    .expect("seed project");
}

/// Temp `HOME` (so `app_settings::load()` reads defaults), in-memory DB,
/// empty v2 map. Restored by a Drop guard so a failed assert can't leak.
fn with_temp_home<F: FnOnce(&Path)>(f: F) {
    struct Restore {
        prev: Option<std::ffi::OsString>,
        tmp: std::path::PathBuf,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            v2_session_map::clear_for_tests();
            match self.prev.take() {
                Some(p) => std::env::set_var("HOME", p),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!("k2-sum-activity-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    let _restore = Restore { prev: std::env::var_os("HOME"), tmp: tmp.clone() };
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();
    v2_session_map::clear_for_tests();
    f(&tmp);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn summary_lists_busy_workspaces_from_activity_and_hooks() {
    with_temp_home(|home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;

        // The daemon's own hook sink (main.rs installs it at boot; the
        // in-process harness does not), so `/hook/complete` reaches the bus.
        let (tx, _rx) = tokio::sync::broadcast::channel::<events::WireEvent>(64);
        k2_core::agent_hooks::set_sink(Box::new(events::DaemonBroadcastSink::new(tx)));

        // A live session in a registered workspace, plus an idle workspace.
        let cwd = home.join("ws-busy");
        std::fs::create_dir_all(&cwd).expect("create ws dir");
        let cfg = DaemonPtyConfig {
            cols: 80,
            rows: 24,
            cwd: Some(cwd.clone()),
            program: Some("cat".to_string()),
            ..DaemonPtyConfig::default()
        };
        let session: Arc<DaemonPtySession> = DaemonPtySession::spawn(cfg).expect("spawn cat PTY");
        let agent = "tab-summary-busy";
        v2_session_map::register(agent.to_string(), Arc::clone(&session));
        let sid = session.session_id.to_string();
        let session_cwd = session
            .cwd
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .expect("session has a cwd");
        seed_project("ws-busy-id", &session_cwd);
        let idle_dir = home.join("ws-idle");
        std::fs::create_dir_all(&idle_dir).expect("create idle dir");
        seed_project("ws-idle-id", &idle_dir.to_string_lossy());

        // Nothing has said anything yet: no busy workspace. Both projects
        // were just interacted with, so both are Active.
        let v = summary(port);
        assert_eq!(v["agentActivity"], serde_json::json!([]), "{v}");
        let active: Vec<&str> = v["activeProjectIds"]
            .as_array()
            .unwrap_or_else(|| panic!("activeProjectIds is an array: {v}"))
            .iter()
            .map(|x| x.as_str().expect("id string"))
            .collect();
        assert!(active.contains(&"ws-busy-id") && active.contains(&"ws-idle-id"), "{v}");
        // Older-shape fields are still there.
        assert_eq!(v["online"], 0, "{v}");
        assert_eq!(v["workspaces"], serde_json::json!([]), "{v}");

        // 1. A braille spinner title through the real PTY → the activity
        //    observer says working.
        session.write(b"\x1b]0;\xe2\xa0\x8b Working\x07\n".to_vec());
        await_activity(
            port,
            serde_json::json!([{ "workspaceId": "ws-busy-id", "status": "working" }]),
            "title activity",
        );

        // 2. A newer permission hook for that session (paneId = the v2
        //    session id) wins over the title.
        let r = http(
            port,
            "GET",
            &format!("/hook/complete?token={OWNER_TOKEN}&paneId={sid}&tabId=t&eventType=PermissionRequest"),
        );
        assert_eq!(r.status, 200, "hook: {}", r.body);
        await_activity(
            port,
            serde_json::json!([{ "workspaceId": "ws-busy-id", "status": "permission" }]),
            "permission hook",
        );

        // 3. A stop hook clears it.
        let r = http(
            port,
            "GET",
            &format!("/hook/complete?token={OWNER_TOKEN}&paneId={sid}&tabId=t&eventType=Stop"),
        );
        assert_eq!(r.status, 200, "hook: {}", r.body);
        await_activity(port, serde_json::json!([]), "stop hook");

        // 4. Busy again, then the session goes away: it drops out.
        let r = http(
            port,
            "GET",
            &format!("/hook/complete?token={OWNER_TOKEN}&paneId={sid}&tabId=t&eventType=UserPromptSubmit"),
        );
        assert_eq!(r.status, 200, "hook: {}", r.body);
        await_activity(
            port,
            serde_json::json!([{ "workspaceId": "ws-busy-id", "status": "working" }]),
            "start hook",
        );
        let removed = v2_session_map::unregister(agent).expect("session was registered");
        drop(removed);
        await_activity(port, serde_json::json!([]), "session removed");
        drop(session);
    });
}
