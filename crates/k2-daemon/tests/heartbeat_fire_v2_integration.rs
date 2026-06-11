//! 0.37.0 v2 retirement — heartbeat fresh-fires register in
//! `v2_session_map` (not the legacy `terminal::shared()` /
//! `TerminalManager`), write a `workspace_sessions` row with
//! `active_terminal_id` populated, and stamp the heartbeat row's
//! `last_session_id` + `active_terminal_id` synchronously.
//!
//! Pre-0.37.0 the fresh-fire path went through the in-process
//! Alacritty Legacy backend via `wake::spawn_wake_headless`. The
//! v2 retirement migrated it to `wake_headless::spawn_wake_headless`
//! → `spawn_agent_session_v2_blocking` → `DaemonPtySession`. This
//! test pins the new contract so any future regression to the
//! legacy backend surfaces in CI before shipping.
//!
//! The test uses `K2SO_WAKE_HEADLESS_TEST_COMMAND` to substitute
//! `claude` with `cat` so the test doesn't require claude on PATH
//! and doesn't burn API calls.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::db::init_for_tests;
use k2_core::db::schema::AgentHeartbeat;

use k2_daemon::v2_session_map;

/// Serialize tests — they all touch globals (DB, v2_session_map,
/// the K2SO_WAKE_HEADLESS_TEST_COMMAND env var).
static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn setup_project(workspace_id: &str) -> PathBuf {
    let project_path = std::env::temp_dir().join(format!(
        "k2so-hb-fire-v2-test-{}-{}-{}",
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
        "INSERT OR REPLACE INTO projects (id, path, name, agent_mode) \
         VALUES (?1, ?2, ?3, 'manager')",
        rusqlite::params![
            workspace_id,
            project_path.to_string_lossy().as_ref(),
            "hb-fire-v2-test",
        ],
    )
    .unwrap();
    project_path
}

/// Write a primary AGENT.md so `find_primary_agent` resolves. Type
/// must match `agent_mode` from `setup_project` — manager.
fn write_primary_agent(project: &Path, name: &str) {
    let dir = project.join(".k2so/agents").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let body = format!("---\nname: {name}\ntype: manager\n---\n# {name}\n");
    std::fs::write(dir.join("AGENT.md"), body).unwrap();
}

/// Set the test-command override so wake_headless spawns `cat`
/// (benign, available everywhere, exits cleanly when stdin closes)
/// instead of `claude`. Returned guard restores the prior value
/// on drop so subsequent tests aren't affected.
struct TestCommandGuard {
    prior: Option<String>,
}

impl TestCommandGuard {
    fn set(cmd: &str) -> Self {
        let prior = std::env::var("K2SO_WAKE_HEADLESS_TEST_COMMAND").ok();
        std::env::set_var("K2SO_WAKE_HEADLESS_TEST_COMMAND", cmd);
        Self { prior }
    }
}

impl Drop for TestCommandGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(v) => std::env::set_var("K2SO_WAKE_HEADLESS_TEST_COMMAND", v),
            None => std::env::remove_var("K2SO_WAKE_HEADLESS_TEST_COMMAND"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// wake_headless registers in v2_session_map (not legacy)
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wake_headless_v2_registers_in_v2_session_map() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _cmd_guard = TestCommandGuard::set("cat");

    let workspace_id = "hb-fire-v2-ws-1";
    let project = setup_project(workspace_id);
    write_primary_agent(&project, "manager");

    let project_path = project.to_string_lossy().into_owned();
    let agent_name = "manager";

    let terminal_id = k2_daemon::wake_headless::spawn_wake_headless(
        agent_name,
        &project_path,
        "test wake prompt",
        None, // no heartbeat — exercises the chat-tab branch
    )
    .expect("spawn_wake_headless should succeed with cat substitution");

    // The returned terminal_id IS the v2 SessionId stringified.
    // Look it up in v2_session_map under the canonical key.
    // **0.37.5:** canonical key is bare workspace_id (no
    // `:<agent_name>` suffix). Reverting `spawn.rs:228` to the
    // pre-0.37.5 prefix shape MUST flip this assertion to "FAIL".
    let _ = agent_name; // referenced via AGENT.md fixture only
    let canonical_key = workspace_id.to_string();
    let session = v2_session_map::lookup_by_agent_name(&canonical_key);
    assert!(
        session.is_some(),
        "v2_session_map missing canonical_key={canonical_key} after fresh fire"
    );
    let session = session.unwrap();
    assert_eq!(
        session.session_id.to_string(),
        terminal_id,
        "v2_session_map session_id must match the returned terminal_id"
    );

    // Cleanup
    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// wake_headless writes the workspace_sessions row + heartbeat fields
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wake_headless_v2_writes_heartbeat_fields_and_does_not_touch_workspace_sessions() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _cmd_guard = TestCommandGuard::set("cat");

    let workspace_id = "hb-fire-v2-ws-2";
    let project = setup_project(workspace_id);
    write_primary_agent(&project, "manager");
    let project_path = project.to_string_lossy().into_owned();

    // Seed a workspace_heartbeats row so the synchronous stamp path
    // has a target.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        AgentHeartbeat::insert(
            &conn,
            "hb-test-id",
            workspace_id,
            "test-hb",
            "hourly",
            "{}",
            ".k2so/heartbeats/test-hb/WAKEUP.md",
            true,
        )
        .expect("seed heartbeat");
    }

    let terminal_id = k2_daemon::wake_headless::spawn_wake_headless(
        "manager",
        &project_path,
        "test wake prompt",
        Some("test-hb"),
    )
    .expect("spawn_wake_headless with heartbeat name");

    // Allow the synchronous DB writes inside spawn_wake_headless to
    // settle.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 0.37.8: heartbeat fires register under their own per-heartbeat
    // canonical key (`<workspace_id>:hb:<name>`), NOT bare
    // `<workspace_id>` — the chat tab owns the bare slot. Reverting
    // either the canonical_key override in `wake_headless` or the
    // override field in `SpawnWorkspaceSessionRequest` MUST flip this
    // assertion to "FAIL".
    let heartbeat_key = format!("{workspace_id}:hb:test-hb");
    let session = v2_session_map::lookup_by_agent_name(&heartbeat_key)
        .expect("v2_session_map missing per-heartbeat canonical key after fire");
    assert_eq!(
        session.session_id.to_string(),
        terminal_id,
        "v2_session_map session_id under {heartbeat_key} must match returned terminal_id"
    );
    assert!(
        v2_session_map::lookup_by_agent_name(workspace_id).is_none(),
        "heartbeat fire must NOT register under bare workspace_id={workspace_id} \
         (that slot is reserved for the chat tab)"
    );

    // 0.37.8: heartbeat fires don't touch workspace_sessions; that
    // row is the chat tab's lane. Pre-fix, `k2so_agents_lock` ran
    // unconditionally and clobbered the chat tab's terminal_id with
    // the heartbeat's PTY id.
    let db = k2_core::db::shared();
    let conn = db.lock();
    let ws_session = k2_core::db::schema::WorkspaceSession::get(&conn, workspace_id)
        .expect("query workspace_sessions");
    assert!(
        ws_session.is_none()
            || ws_session
                .as_ref()
                .and_then(|r| r.terminal_id.as_deref())
                .map(|t| t != terminal_id)
                .unwrap_or(true),
        "workspace_sessions.terminal_id must NOT be stamped to the heartbeat PTY id; \
         got {ws_session:?} terminal_id matching {terminal_id}"
    );

    // The heartbeat row got its synchronous stamps:
    //  - last_session_id = pinned uuid
    //  - active_terminal_id = the v2 session id
    let hb = AgentHeartbeat::get_by_name(&conn, workspace_id, "test-hb")
        .expect("query heartbeat row")
        .expect("heartbeat row should exist");
    assert!(
        hb.last_session_id.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
        "heartbeat.last_session_id must be stamped (pinned UUID)"
    );
    assert_eq!(
        hb.active_terminal_id.as_deref(),
        Some(terminal_id.as_str()),
        "heartbeat.active_terminal_id must point at the heartbeat's own PTY"
    );

    drop(conn);
    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// Idempotency — second call against same canonical key reuses
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wake_headless_v2_idempotent_under_canonical_key() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _cmd_guard = TestCommandGuard::set("cat");

    let workspace_id = "hb-fire-v2-ws-3";
    let project = setup_project(workspace_id);
    write_primary_agent(&project, "manager");
    let project_path = project.to_string_lossy().into_owned();

    let first = k2_daemon::wake_headless::spawn_wake_headless(
        "manager",
        &project_path,
        "first prompt",
        None,
    )
    .expect("first spawn");

    let second = k2_daemon::wake_headless::spawn_wake_headless(
        "manager",
        &project_path,
        "second prompt",
        None,
    )
    .expect("second spawn");

    // Same canonical key → second call should return the same
    // terminal_id (reuse via spawn_agent_session_v2_blocking's
    // idempotency check). This is the cross-instance "agents launch
    // is idempotent" behavior Baden's webhook flow depends on.
    assert_eq!(
        first, second,
        "two spawns under the same canonical key must return the SAME terminal_id (idempotency)"
    );

    v2_session_map::clear_for_tests();
}
