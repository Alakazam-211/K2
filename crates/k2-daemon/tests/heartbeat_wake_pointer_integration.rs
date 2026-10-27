//! 0.45.6 — a heartbeat wake prompt never silently loses its head.
//!
//! Field reports (appa, 2026-08-31 and 2026-10-13): a long WAKEUP.md
//! reached the agent with only its TAIL (an 8,540-byte file arrived
//! starting mid-sentence near the footer). Long bodies are now sent as
//! a one-line pointer to the file; short bodies are pasted whole.
//!
//! These tests fire real heartbeats through `smart_launch` with the
//! agent replaced by `cat` (via `K2SO_WAKE_HEADLESS_TEST_COMMAND` + a
//! shim dir, never a real LLM CLI). `cat` echoes what the fire typed
//! into the PTY, so the grid shows exactly what the agent would have
//! received: either the whole body or the pointer, never a tail.

use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use k2_core::db::init_for_tests;
use k2_core::db::schema::{AgentHeartbeat, HeartbeatFire};
use k2_core::workspace::wake_prompts::{wake_pointer_prompt, WAKE_PROMPT_INLINE_MAX_BYTES};

use k2_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn setup_project(workspace_id: &str) -> PathBuf {
    let project_path = std::env::temp_dir().join(format!(
        "k2-hb-ptr-{}-{}-{}",
        workspace_id,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&project_path);
    std::fs::create_dir_all(&project_path).unwrap();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name, agent_mode, agent_enabled) \
         VALUES (?1, ?2, ?3, 'manager', 1)",
        rusqlite::params![workspace_id, project_path.to_string_lossy().as_ref(), "hb-ptr"],
    )
    .unwrap();
    project_path
}

fn write_primary_agent(project: &Path, name: &str) {
    let dir = project.join(".k2so/agents").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let body = format!("---\nname: {name}\ntype: manager\n---\n# {name}\n");
    std::fs::write(dir.join("AGENT.md"), body).unwrap();
}

/// Writes the wake file; returns (relative path for the row, absolute path).
fn write_wakeup(project: &Path, hb_name: &str, body: &str) -> (String, PathBuf) {
    let rel = format!(".k2/heartbeats/{hb_name}/WAKEUP.md");
    let abs = project.join(&rel);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(&abs, body).unwrap();
    (rel, abs)
}

/// A `size`-byte body with a distinct head and tail, short lines.
fn head_tail_body(size: usize) -> String {
    let head = "# STEP 0 HEADMARKER run the gate first\n";
    let tail = "\nFOOTER TAILMARKER report and stop.";
    let filler = size - head.len() - tail.len();
    let mut s = String::with_capacity(size);
    s.push_str(head);
    for i in 0..filler {
        s.push(if i % 60 == 59 { '\n' } else { 'x' });
    }
    s.push_str(tail);
    assert_eq!(s.len(), size);
    s
}

fn seed_heartbeat(workspace_id: &str, name: &str, rel: &str, use_workspace_session: bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    AgentHeartbeat::insert(
        &conn,
        &format!("{name}-id"),
        workspace_id,
        name,
        "hourly",
        "{}",
        rel,
        true,
    )
    .expect("seed heartbeat");
    let n = AgentHeartbeat::set_use_workspace_session(&conn, workspace_id, name, use_workspace_session)
        .expect("set use_workspace_session");
    assert_eq!(n, 1, "exactly one heartbeat row");
}

fn latest_fire_reason(workspace_id: &str, name: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let fires = HeartbeatFire::list_by_schedule_name(&conn, workspace_id, name, 5)
        .expect("query heartbeat_fires");
    let latest = fires.first().expect("an audit row must be written");
    assert_eq!(latest.decision, "fired", "fire must succeed: {latest:?}");
    latest.reason.clone().expect("fired row carries a reason")
}

fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Polls the session grid until `want` (whitespace-insensitive, so
/// terminal wrapping can't hide it) shows up. Panics with the grid on
/// timeout. Returns the squashed grid.
fn wait_for_grid(key: &str, want: &str) -> String {
    let session = v2_session_map::lookup_by_agent_name(key)
        .unwrap_or_else(|| panic!("no v2 session under key {key}"));
    let want = squash(want);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let grid = squash(&session.visible_text_rows().join(""));
        if grid.contains(&want) {
            return grid;
        }
        if Instant::now() > deadline {
            panic!("grid under {key} never showed the expected text.\nwant: {want}\ngrid: {grid}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct TestCommandGuard {
    prior: Option<String>,
    prior_shim: Option<String>,
    shim_dir: PathBuf,
}

impl TestCommandGuard {
    /// `<shim_dir>/cat` execs `cat`; `K2_TEST_AGENT_SHIM_DIR` points at
    /// it so no spawn can ever reach a real agent CLI.
    fn set(cmd: &str) -> Self {
        Self::set_script(cmd, "#!/bin/sh\nexec cat\n")
    }

    /// Same guard with a custom shim body.
    fn set_script(cmd: &str, script: &str) -> Self {
        let prior = std::env::var("K2SO_WAKE_HEADLESS_TEST_COMMAND").ok();
        let prior_shim = std::env::var("K2_TEST_AGENT_SHIM_DIR").ok();
        let shim_dir = std::env::temp_dir().join(format!(
            "k2-hb-ptr-shim-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&shim_dir).unwrap();
        let shim = shim_dir.join(cmd);
        k2_core::test_env::write_executable(&shim, script);
        std::env::set_var("K2SO_WAKE_HEADLESS_TEST_COMMAND", cmd);
        std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
        Self { prior, prior_shim, shim_dir }
    }
}

impl Drop for TestCommandGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(v) => std::env::set_var("K2SO_WAKE_HEADLESS_TEST_COMMAND", v),
            None => std::env::remove_var("K2SO_WAKE_HEADLESS_TEST_COMMAND"),
        }
        match &self.prior_shim {
            Some(v) => std::env::set_var("K2_TEST_AGENT_SHIM_DIR", v),
            None => std::env::remove_var("K2_TEST_AGENT_SHIM_DIR"),
        }
        let _ = std::fs::remove_dir_all(&self.shim_dir);
    }
}

/// Fresh fire (the heartbeat's own lane) with a 20 KB wake file: the
/// agent receives the pointer, exactly, and none of the body's tail.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_fire_20kb_wake_file_delivers_pointer_not_tail() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _cmd = TestCommandGuard::set("cat");

    let ws = "hb-ptr-ws-fresh";
    let project = setup_project(ws);
    write_primary_agent(&project, "manager");
    let project_path = project.to_string_lossy().into_owned();
    let body = head_tail_body(20 * 1024);
    let (rel, abs) = write_wakeup(&project, "big-hb", &body);
    seed_heartbeat(ws, "big-hb", &rel, false);

    let result = k2_daemon::heartbeat_launch::smart_launch(&project_path, "big-hb");
    assert_eq!(result["success"], true, "fire must succeed: {result}");
    assert_eq!(result["branch"], "fresh_fire", "{result}");
    assert_eq!(result["wakePrompt"]["mode"], "pointer", "{result}");
    assert_eq!(result["wakePrompt"]["bodyBytes"], 20 * 1024, "{result}");

    let reason = latest_fire_reason(ws, "big-hb");
    assert!(reason.contains("pointer"), "run log must record the pointer choice: {reason}");

    let pointer = wake_pointer_prompt(&abs, "big-hb", 20 * 1024);
    let grid = wait_for_grid(&format!("{ws}:hb:big-hb"), &pointer);
    assert!(!grid.contains("TAILMARKER"), "the body's tail must never be typed: {grid}");
    assert!(!grid.contains("HEADMARKER"), "the body is not pasted at all: {grid}");

    v2_session_map::clear_for_tests();
}

/// Pinned-chat lane (`use_workspace_session`, the default for new
/// heartbeats) with a 20 KB wake file: same pointer rule.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_session_20kb_wake_file_delivers_pointer_not_tail() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _cmd = TestCommandGuard::set("cat");

    let ws = "hb-ptr-ws-pinned";
    let project = setup_project(ws);
    write_primary_agent(&project, "manager");
    let project_path = project.to_string_lossy().into_owned();
    let body = head_tail_body(20 * 1024);
    let (rel, abs) = write_wakeup(&project, "pinned-big", &body);
    seed_heartbeat(ws, "pinned-big", &rel, true);

    let result = k2_daemon::heartbeat_launch::smart_launch(&project_path, "pinned-big");
    assert_eq!(result["success"], true, "fire must succeed: {result}");
    let branch = result["branch"].as_str().expect("branch string");
    assert!(branch.starts_with("workspace_session:"), "{result}");
    assert_eq!(result["wakePrompt"]["mode"], "pointer", "{result}");

    let reason = latest_fire_reason(ws, "pinned-big");
    assert!(reason.contains("pointer"), "run log must record the pointer choice: {reason}");

    let pointer = wake_pointer_prompt(&abs, "pinned-big", 20 * 1024);
    let grid = wait_for_grid(ws, &pointer);
    assert!(!grid.contains("TAILMARKER"), "the body's tail must never be typed: {grid}");

    v2_session_map::clear_for_tests();
}

/// A short wake file is still pasted whole — head and tail both arrive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_fire_small_wake_file_delivers_whole_body() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _cmd = TestCommandGuard::set("cat");

    let ws = "hb-ptr-ws-small";
    let project = setup_project(ws);
    write_primary_agent(&project, "manager");
    let project_path = project.to_string_lossy().into_owned();
    let body = head_tail_body(400);
    assert!(body.len() <= WAKE_PROMPT_INLINE_MAX_BYTES);
    let (rel, _abs) = write_wakeup(&project, "small-hb", &body);
    seed_heartbeat(ws, "small-hb", &rel, false);

    let result = k2_daemon::heartbeat_launch::smart_launch(&project_path, "small-hb");
    assert_eq!(result["success"], true, "fire must succeed: {result}");
    assert_eq!(result["wakePrompt"]["mode"], "inline", "{result}");
    assert_eq!(result["wakePrompt"]["bodyBytes"], 400, "{result}");

    let reason = latest_fire_reason(ws, "small-hb");
    assert!(reason.contains("inline"), "run log records the inline choice: {reason}");

    let grid = wait_for_grid(&format!("{ws}:hb:small-hb"), "TAILMARKER report and stop.");
    assert!(
        grid.contains(&squash("# STEP 0 HEADMARKER run the gate first")),
        "inline delivery must carry the head: {grid}"
    );

    v2_session_map::clear_for_tests();
}

/// GH #61 — a fire whose wake prompt never reaches a live agent is a
/// recorded failure, not `fired`: `last_fired` does not advance, the
/// failure counter does, and the run log says why. The "agent" here is a
/// shim that exits at once, so the prompt has nowhere to land.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_fire_into_agent_that_exits_records_failure_not_fired() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _cmd = TestCommandGuard::set_script("exiter", "#!/bin/sh\nexit 0\n");

    let ws = "hb-ptr-ws-undelivered";
    let project = setup_project(ws);
    write_primary_agent(&project, "manager");
    let project_path = project.to_string_lossy().into_owned();
    let (rel, _abs) = write_wakeup(&project, "lost-hb", "do the daily check\n");
    seed_heartbeat(ws, "lost-hb", &rel, false);

    let result = k2_daemon::heartbeat_launch::smart_launch(&project_path, "lost-hb");
    assert_eq!(result["success"], false, "an undelivered wake must not succeed: {result}");

    let db = k2_core::db::shared();
    let conn = db.lock();
    let fires = HeartbeatFire::list_by_schedule_name(&conn, ws, "lost-hb", 5)
        .expect("query heartbeat_fires");
    let latest = fires.first().expect("an audit row must be written");
    assert_eq!(latest.decision, "error", "undelivered fire is a failure: {latest:?}");
    let reason = latest.reason.clone().expect("failure row carries a reason");
    assert!(reason.contains("not delivered"), "reason must say the prompt never landed: {reason}");
    assert!(
        !fires.iter().any(|f| f.decision == "fired"),
        "no `fired` row for a wake that never landed: {fires:?}"
    );
    let hb = AgentHeartbeat::get_by_name(&conn, ws, "lost-hb")
        .expect("query heartbeat")
        .expect("heartbeat row");
    assert!(hb.last_fired.is_none(), "last_fired must not advance: {:?}", hb.last_fired);
    assert_eq!(hb.consecutive_failures, 1, "the failure is counted");
    drop(conn);

    v2_session_map::clear_for_tests();
}
