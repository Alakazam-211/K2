//! Observability / Agent-Ops (`/cli/ops/*`) — read-only route integration
//! tests. They drive REAL HTTP GETs through the REAL
//! `routes::dispatcher::dispatch` (started in-process by
//! `k2_daemon::test_harness::start`) and assert the full dispatch +
//! token-gate + handler stack for Phase A (`/cli/ops/activity`) and Phase B
//! (`/cli/ops/overview`).
//!
//! Harness pattern mirrors `connect_gap_routes_integration.rs`: serialize on
//! a process-wide lock, point `$HOME` at a fresh tempdir, init the in-memory
//! DB, talk to the daemon over a raw loopback TCP socket.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};

use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession};
use k2_daemon::{session_events, test_harness, v2_session_map};

/// Serialize: `$HOME` + the shared in-memory DB + the v2_session_map + the
/// session_events cache are all process-wide globals.
static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-ops-2a";

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("set read timeout");
    let req = format!(
        "{method} {path_and_query} HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");

    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some((status, body, complete)) = try_parse(&raw) {
            if complete {
                return Resp { status, body };
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                break
            }
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));
    let body = match text.split_once("\r\n\r\n") {
        Some((_h, b)) => b.to_string(),
        None => String::new(),
    };
    Resp { status, body }
}

fn try_parse(raw: &[u8]) -> Option<(u16, String, bool)> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())?;
    let content_len = headers.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let complete = match content_len {
        Some(clen) => body.len() >= clen,
        None => true,
    };
    Some((status, body.to_string(), complete))
}

/// Redirect `$HOME` to a fresh tempdir (so `app_settings::load()` reads
/// defaults, not the dev box's real settings), init the in-memory DB, run
/// `f`, restore `$HOME`. Caller holds `TEST_LOCK`.
fn with_temp_home<T, F: FnOnce(&Path) -> T>(f: F) -> T {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("k2so-ops-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();
    v2_session_map::clear_for_tests();

    let out = f(&tmp);

    v2_session_map::clear_for_tests();
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
    out
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Seed a `projects` row with an explicit path + last_interaction.
fn seed_project(id: &str, path: &str, last_interaction: Option<i64>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects \
         (id, path, name, color, agent_mode, pinned, tab_order, manually_active, last_interaction_at) \
         VALUES (?1, ?2, ?3, '#123456', 'off', 0, 0, 0, ?4)",
        rusqlite::params![id, path, "ops-test", last_interaction],
    )
    .expect("seed project");
}

// ─────────────────────────────────────────────────────────────────────
// Phase A — /cli/ops/activity over the persistent activity_feed
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_returns_rows_with_since_limit_actor_honored() {
    let _g = lock();
    with_temp_home(|_home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let pid = "ops-activity-pid";
        seed_project(pid, "/tmp/ops-activity", None);

        // Seed three rows with increasing created_at (insert uses
        // unixepoch(); overwrite created_at explicitly for determinism).
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            for (i, (actor, ts)) in [("alice", 100), ("bob", 200), ("alice", 300)]
                .iter()
                .enumerate()
            {
                let id = k2_core::db::schema::ActivityFeedEntry::insert(
                    &conn,
                    pid,
                    Some(actor),
                    "message",
                    None,
                    None,
                    None,
                    Some(&format!("evt-{i}")),
                    None,
                )
                .expect("insert activity row");
                conn.execute(
                    "UPDATE activity_feed SET created_at = ?1 WHERE id = ?2",
                    rusqlite::params![ts, id],
                )
                .expect("set created_at");
            }
        }

        // Full project read → 3 rows, newest first.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/ops/activity?project={pid}&token={OWNER_TOKEN}"),
        );
        assert_eq!(r.status, 200, "owner GET activity → 200; body={}", r.body);
        let rows: serde_json::Value = serde_json::from_str(&r.body).expect("parse json array");
        let arr = rows.as_array().expect("array");
        assert_eq!(arr.len(), 3, "all three rows; body={}", r.body);
        assert_eq!(
            arr[0]["createdAt"], 300,
            "rows must be newest-first; body={}",
            r.body
        );

        // limit=1 → only the newest row.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/ops/activity?project={pid}&limit=1&token={OWNER_TOKEN}"),
        );
        let arr = serde_json::from_str::<serde_json::Value>(&r.body).unwrap();
        let arr = arr.as_array().unwrap();
        assert_eq!(arr.len(), 1, "limit=1 honored; body={}", r.body);
        assert_eq!(arr[0]["createdAt"], 300);

        // since=250 → only the row at 300.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/ops/activity?project={pid}&since=250&token={OWNER_TOKEN}"),
        );
        let arr = serde_json::from_str::<serde_json::Value>(&r.body).unwrap();
        let arr = arr.as_array().unwrap();
        assert_eq!(arr.len(), 1, "since=250 drops older rows; body={}", r.body);
        assert_eq!(arr[0]["createdAt"], 300);

        // actor=alice → the two alice rows (300, 100).
        let r = http(
            d.port,
            "GET",
            &format!("/cli/ops/activity?project={pid}&actor=alice&token={OWNER_TOKEN}"),
        );
        let arr = serde_json::from_str::<serde_json::Value>(&r.body).unwrap();
        let arr = arr.as_array().unwrap();
        assert_eq!(arr.len(), 2, "actor=alice → 2 rows; body={}", r.body);
        assert!(
            arr.iter().all(|row| row["actor"] == "alice"),
            "every actor-filtered row is alice; body={}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_rejects_unauthenticated_and_missing_project() {
    let _g = lock();
    with_temp_home(|_home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // No token → 403 (token gate, applied by the /cli/ catchall).
        let r = http(d.port, "GET", "/cli/ops/activity?project=p");
        assert_eq!(r.status, 403, "no token must 403; body={}", r.body);

        // Garbage token → 403.
        let r = http(d.port, "GET", "/cli/ops/activity?project=p&token=garbage");
        assert_eq!(r.status, 403, "garbage token must 403; body={}", r.body);

        // Authed but missing required project param → 400.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/ops/activity?token={OWNER_TOKEN}"),
        );
        assert_eq!(r.status, 400, "missing project → 400; body={}", r.body);
    });
}

// ─────────────────────────────────────────────────────────────────────
// Phase B — /cli/ops/overview over the live v2 session map
// ─────────────────────────────────────────────────────────────────────

/// Spawn a real `cat` PTY in `cwd`, register it under `agent_address`, and
/// return the session Arc so the test holds a strong reference.
fn spawn_session(agent_address: &str, cwd: &Path) -> Arc<DaemonPtySession> {
    let cfg = DaemonPtyConfig {
        cols: 80,
        rows: 24,
        cwd: Some(cwd.to_path_buf()),
        program: Some("cat".to_string()),
        ..DaemonPtyConfig::default()
    };
    let session = DaemonPtySession::spawn(cfg).expect("spawn cat PTY");
    v2_session_map::register(agent_address.to_string(), Arc::clone(&session));
    session
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_lists_live_session_with_status_matching_event_source() {
    let _g = lock();
    with_temp_home(|home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // A live session whose cwd matches a project that is Active (just
        // interacted), so `active` derives true from the SAME function the
        // ActiveChanged broadcast uses (compute_active_project_ids).
        let cwd = home.join("ws-live");
        std::fs::create_dir_all(&cwd).unwrap();
        let agent_address = "tab-ops-live";
        let session = spawn_session(agent_address, &cwd);
        let sid = session.session_id.to_string();

        // The session's stored cwd is the canonical key for resolve_project_id.
        let session_cwd = session
            .cwd
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .expect("session has cwd");
        seed_project("ops-live-pid", &session_cwd, Some(now_secs()));

        // Drive an AgentStatusChanged through the SAME emit `/cli/sessions/
        // events` uses; capture what a bus subscriber receives so we can
        // prove the overview reports the identical truth.
        let mut rx = session_events::subscribe();
        let _ = session_events::emit(session_events::SessionEvent::AgentStatusChanged {
            pane_id: sid.clone(),
            tab_id: agent_address.to_string(),
            status: "start".into(),
            workspace_path: None,
        });
        // Drain to our probe (global bus may carry other events).
        let stream_status = futures_block(async {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                assert!(!remaining.is_zero(), "probe AgentStatusChanged not received");
                match tokio::time::timeout(remaining, rx.recv()).await {
                    Ok(Ok(session_events::SessionEvent::AgentStatusChanged {
                        pane_id,
                        status,
                        ..
                    })) if pane_id == sid => break status,
                    Ok(Ok(_)) => continue,
                    Ok(Err(_)) => panic!("receiver closed"),
                    Err(_) => panic!("timed out waiting for probe event"),
                }
            }
        });
        assert_eq!(stream_status, "start", "the events-stream truth is 'start'");

        let r = http(
            d.port,
            "GET",
            &format!("/cli/ops/overview?token={OWNER_TOKEN}"),
        );
        assert_eq!(r.status, 200, "owner GET overview → 200; body={}", r.body);
        let arr: serde_json::Value = serde_json::from_str(&r.body).expect("parse overview array");
        let arr = arr.as_array().expect("array");

        let entry = arr
            .iter()
            .find(|e| e["sessionId"] == serde_json::json!(sid))
            .unwrap_or_else(|| panic!("overview must list the live session; body={}", r.body));

        assert_eq!(entry["agentAddress"], agent_address);
        assert_eq!(entry["workspacePath"], session_cwd);
        // agent_status is the normalized form of the SAME status the events
        // stream delivered (start → working): no divergent source.
        assert_eq!(
            entry["agentStatus"], "working",
            "overview status must derive from the same AgentStatusChanged the \
             events stream carried ({stream_status}); body={}",
            r.body
        );
        assert_eq!(
            entry["active"], true,
            "session's project is Active per compute_active_project_ids; body={}",
            r.body
        );
        // Not a heartbeat's active terminal → null.
        assert!(
            entry["heartbeatState"].is_null(),
            "non-heartbeat session has null heartbeat_state; body={}",
            r.body
        );
        assert!(
            entry["lastActivityAt"].as_i64().unwrap_or(0) > 0,
            "last_activity_at stamped from the status emit; body={}",
            r.body
        );

        drop(session);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_rejects_unauthenticated() {
    let _g = lock();
    with_temp_home(|_home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let r = http(d.port, "GET", "/cli/ops/overview");
        assert_eq!(r.status, 403, "no token must 403; body={}", r.body);
        let r = http(d.port, "GET", "/cli/ops/overview?token=garbage");
        assert_eq!(r.status, 403, "garbage token must 403; body={}", r.body);
    });
}
