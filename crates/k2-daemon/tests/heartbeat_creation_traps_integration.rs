//! S5 — creation traps (`.k2/prds/prd-heartbeat-firing-v1.md` HB32–HB35,
//! Rosson decisions D4 and D6), through the daemon's real route
//! dispatch and scheduler-fire handler.
//!
//! - `/cli/heartbeat/add` writes `instructions` on the daemon's disk
//!   (HB32) and answers 400, naming the allowed values, for an unknown
//!   frequency or a sub-minute interval (HB34).
//! - An empty WAKEUP.md is a wait, not a failure: the scheduler skips
//!   it, a manual Launch answers `wakeup_empty`, and neither disables
//!   the row or counts a failure (HB33). `heartbeat/list` names it.
//!
//! Lives in its own file (not `triage_integration.rs`) so it cannot
//! collide with the S2 builder's edits there.
//!
//! k2-core is linked without `cfg(test)` here, so the add path's
//! scheduler install is real code. `K2_HEARTBEAT_NO_SELF_HEAL=1` makes
//! the S1 guard refuse every launchctl/crontab write before it runs.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;

use k2_core::db::init_for_tests;
use k2_core::db::schema::AgentHeartbeat;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    let g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("K2_HEARTBEAT_NO_SELF_HEAL", "1");
    init_for_tests();
    g
}

struct Ws {
    dir: PathBuf,
    path: String,
    project_id: String,
}

impl Drop for Ws {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A registered workspace with a configured agent (`agent_enabled = 1`),
/// so the tick and the launcher resolve an agent name.
fn workspace(tag: &str) -> Ws {
    let dir = std::env::temp_dir().join(format!(
        "k2-hb-s5-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.to_string_lossy().into_owned();
    let project_id = format!("proj-s5-{tag}-{}", std::process::id());
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name, agent_enabled) VALUES (?1, ?2, ?3, 1)",
        rusqlite::params![project_id, path, format!("s5-{tag}")],
    )
    .unwrap();
    Ws { dir, path, project_id }
}

fn params(ws: &Ws, pairs: &[(&str, &str)]) -> HashMap<String, String> {
    let mut p: HashMap<String, String> =
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    p.insert("project".into(), ws.path.clone());
    p
}

fn dispatch(route: &str, ws: &Ws, pairs: &[(&str, &str)]) -> (String, serde_json::Value) {
    let resp = k2_daemon::cli::dispatch(route, &params(ws, pairs));
    let body: serde_json::Value = serde_json::from_str(&resp.body)
        .unwrap_or_else(|e| panic!("{route}: body is not JSON ({e}): {}", resp.body));
    (resp.status.to_string(), body)
}

fn row(ws: &Ws, name: &str) -> AgentHeartbeat {
    let db = k2_core::db::shared();
    let conn = db.lock();
    AgentHeartbeat::get_by_name(&conn, &ws.project_id, name)
        .unwrap()
        .expect("heartbeat row exists")
}

fn listed(ws: &Ws, name: &str) -> serde_json::Value {
    let (status, body) = dispatch("/cli/heartbeat/list", ws, &[]);
    assert_eq!(status, "200 OK", "list: {body}");
    body.as_array()
        .expect("list is an array")
        .iter()
        .find(|r| r["name"] == name)
        .unwrap_or_else(|| panic!("{name} not in list: {body}"))
        .clone()
}

fn body_of(abs: &str) -> String {
    let raw = std::fs::read_to_string(abs).expect("WAKEUP.md exists");
    k2_core::workspace::wake_prompts::strip_frontmatter(&raw)
}

fn assert_waiting_enabled(ws: &Ws, name: &str) {
    let hb = row(ws, name);
    assert!(hb.enabled, "{name}: empty WAKEUP.md must not disable the row");
    assert_eq!(hb.disabled_reason, None, "{name}");
    assert_eq!(hb.consecutive_failures, 0, "{name}: empty is not a failure");
    assert_eq!(hb.next_retry_at, None, "{name}: no backoff");
    assert_eq!(hb.in_flight_started_at, None, "{name}: lease released");
}

#[test]
fn add_route_writes_instructions_on_the_daemon_disk() {
    let _g = lock();
    let ws = workspace("add");
    let (status, out) = dispatch(
        "/cli/heartbeat/add",
        &ws,
        &[
            ("name", "with-body"),
            ("frequency", "hourly"),
            ("spec", r#"{"every_seconds":900}"#),
            ("instructions", "check inbox"),
        ],
    );
    assert_eq!(status, "200 OK", "add: {out}");
    let abs = out["wakeupAbs"].as_str().expect("wakeupAbs").to_string();
    assert_eq!(body_of(&abs), "check inbox");
    assert_eq!(out["instructionsWritten"], serde_json::json!(true));
    // S3: the stored reason is the single source — a row with a body in
    // a workspace with an agent is simply `scheduled`, with a next fire.
    let with_body = listed(&ws, "with-body");
    assert_eq!(with_body["waitReason"], serde_json::json!("scheduled"), "{with_body}");
    assert!(with_body["nextFireAt"].is_string(), "{with_body}");

    let (status, out) = dispatch(
        "/cli/heartbeat/add",
        &ws,
        &[("name", "no-body"), ("frequency", "daily"), ("spec", r#"{"time":"07:00"}"#)],
    );
    assert_eq!(status, "200 OK", "add without instructions still creates: {out}");
    assert_eq!(out["waitReason"], serde_json::json!("wakeup_empty"));
    let row = listed(&ws, "no-body");
    assert_eq!(row["enabled"], serde_json::json!(true));
    assert_eq!(row["waitReason"], serde_json::json!("wakeup_empty"));
}

#[test]
fn add_and_edit_routes_answer_400_for_bad_frequencies() {
    let _g = lock();
    let ws = workspace("bad-freq");
    let allowed = "hourly|daily|weekly|monthly|yearly|scheduled";

    let (status, out) = dispatch(
        "/cli/heartbeat/add",
        &ws,
        &[("name", "x"), ("frequency", "list"), ("instructions", "hi")],
    );
    assert_eq!(status, "400 Bad Request", "{out}");
    let err = out["error"].as_str().expect("error string");
    assert!(err.contains("unknown frequency 'list'"), "err={err}");
    assert!(err.contains(allowed), "err={err}");

    let (status, out) = dispatch(
        "/cli/heartbeat/add",
        &ws,
        &[("name", "fast"), ("frequency", "hourly"), ("spec", r#"{"every_seconds":30}"#)],
    );
    assert_eq!(status, "400 Bad Request", "{out}");
    assert!(out["error"].as_str().expect("error").contains("at least 60"), "{out}");

    let db = k2_core::db::shared();
    let conn = db.lock();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workspace_heartbeats WHERE project_id = ?1",
            rusqlite::params![ws.project_id],
            |r| r.get(0),
        )
        .unwrap();
    drop(conn);
    assert_eq!(count, 0, "a refused add writes no row");

    let (status, out) = dispatch(
        "/cli/heartbeat/add",
        &ws,
        &[("name", "ok"), ("frequency", "daily"), ("spec", "{}"), ("instructions", "hi")],
    );
    assert_eq!(status, "200 OK", "{out}");
    let (status, out) = dispatch(
        "/cli/heartbeat/edit",
        &ws,
        &[("name", "ok"), ("frequency", "list"), ("spec", "{}")],
    );
    assert_eq!(status, "400 Bad Request", "{out}");
    assert!(out["error"].as_str().expect("error").contains(allowed), "{out}");
    assert_eq!(row(&ws, "ok").frequency, "daily", "a refused edit leaves the row alone");
}

/// T8 through the daemon: a due empty row is skipped by the real
/// scheduler-fire handler and stays enabled; a manual Launch answers
/// `wakeup_empty` without disabling or counting a failure.
#[tokio::test(flavor = "current_thread")]
async fn empty_wakeup_is_a_wait_for_the_scheduler_and_for_launch() {
    let _g = lock();
    let ws = workspace("empty");
    let (status, out) = dispatch(
        "/cli/heartbeat/add",
        &ws,
        &[("name", "settings-style"), ("frequency", "hourly"), ("spec", r#"{"every_seconds":60}"#)],
    );
    assert_eq!(status, "200 OK", "{out}");
    {
        // Make the first slot long past.
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE workspace_heartbeats SET created_at = unixepoch() - 630 WHERE project_id = ?1",
            rusqlite::params![ws.project_id],
        )
        .unwrap();
    }

    for _ in 0..2 {
        let fired: serde_json::Value =
            serde_json::from_str(&k2_daemon::triage::handle_scheduler_fire(&ws.path)).unwrap();
        assert_eq!(fired["count"].as_u64(), Some(0), "empty body must not launch: {fired}");
    }
    assert_waiting_enabled(&ws, "settings-style");
    let r = listed(&ws, "settings-style");
    assert_eq!(r["waitReason"], serde_json::json!("wakeup_empty"));
    assert!(
        r["waitDetail"].as_str().expect("waitDetail").contains("needs instructions"),
        "{r}"
    );

    let (status, out) = dispatch("/cli/heartbeat/launch", &ws, &[("name", "settings-style")]);
    assert_eq!(status, "200 OK", "{out}");
    assert_eq!(out["success"], serde_json::json!(false), "{out}");
    assert_eq!(out["decision"], serde_json::json!("wakeup_empty"), "{out}");
    assert_waiting_enabled(&ws, "settings-style");
}
