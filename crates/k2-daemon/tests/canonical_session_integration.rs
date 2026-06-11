//! 0.37.2 canonical-session ensurance — proactive PTY spawn + DB row
//! registration when a workspace transitions to a bot mode.
//!
//! Pins the contract that solves the SMS-bridge race documented in
//! the nsi-checkin issue: a fresh workspace with mode set + AGENT.md
//! written must have a `workspace_sessions` row + a v2_session_map
//! entry under the canonical key BEFORE any consumer (webhook
//! `k2so msg --wake`, renderer pinned-tab attach, etc.) can race.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;

use k2_core::db::init_for_tests;
use k2_daemon::canonical_session::{
    boot_sweep_ensure_canonical_sessions, ensure_canonical_session,
};
use k2_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn setup_project(workspace_id: &str, name: &str, agent_mode: &str) -> PathBuf {
    let project_path = std::env::temp_dir().join(format!(
        "k2so-canonical-test-{}-{}-{}",
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
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            workspace_id,
            project_path.to_string_lossy().as_ref(),
            name,
            agent_mode,
        ],
    )
    .unwrap();
    project_path
}

/// Write an AGENT.md whose `launch:` profile spawns `cat` instead of
/// claude — keeps the test self-contained, no claude binary required,
/// no API calls. `cat` reads from stdin until EOF, perfect for a
/// long-lived PTY child the test can register + then drop.
fn write_test_agent_md(project: &Path, agent_name: &str, agent_type: &str) {
    let dir = project.join(".k2so/agent");
    std::fs::create_dir_all(&dir).unwrap();
    let body = format!(
        "---\n\
         name: {agent_name}\n\
         type: {agent_type}\n\
         launch:\n  \
           command: cat\n\
         ---\n\
         # {agent_name}\n"
    );
    std::fs::write(dir.join("AGENT.md"), body).unwrap();
}

// ─────────────────────────────────────────────────────────────────────
// Primary contract: fresh ensure spawns + registers + persists
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_canonical_session_fresh_spawns_and_registers() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let workspace_id = "canon-test-ws-fresh";
    let project = setup_project(workspace_id, "fresh-test", "custom");
    write_test_agent_md(&project, "scout", "custom");

    let project_path = project.to_string_lossy().into_owned();

    let outcome = ensure_canonical_session(&project_path)
        .expect("ensure should succeed on a fresh workspace with AGENT.md");

    assert!(
        !outcome.reused,
        "first ensure on a cold workspace must spawn fresh, not reuse"
    );
    assert_eq!(outcome.agent_name, "scout");
    assert_eq!(outcome.project_id, workspace_id);
    assert!(
        !outcome.session_id.is_empty(),
        "session_id must be set on fresh spawn"
    );

    // v2_session_map must contain the canonical key.
    //
    // **0.37.5:** canonical key is bare workspace_id (no
    // `:<agent_name>` suffix; see canonical_session::canonical_key_for).
    // Reverting that helper to the pre-0.37.5 prefix shape MUST
    // flip this assertion to "FAIL".
    let canonical_key = workspace_id.to_string();
    let live = v2_session_map::lookup_by_agent_name(&canonical_key);
    assert!(
        live.is_some(),
        "v2_session_map missing canonical_key={canonical_key} after ensure"
    );
    // Regression guard: legacy `<pid>:<agent>` MUST NOT resolve.
    let legacy_key = format!("{workspace_id}:scout");
    assert!(
        v2_session_map::lookup_by_agent_name(&legacy_key).is_none(),
        "0.37.5 regression: legacy {legacy_key} must NOT be registered"
    );
    let live = live.unwrap();
    assert_eq!(
        live.session_id.to_string(),
        outcome.session_id,
        "v2_session_map session must match the EnsureOutcome session_id"
    );

    // workspace_sessions row must be persisted with terminal_id set.
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = k2_core::db::schema::WorkspaceSession::get(&conn, workspace_id)
        .unwrap()
        .expect("workspace_sessions row should exist after ensure");
    assert_eq!(
        row.terminal_id.as_deref(),
        Some(outcome.session_id.as_str()),
        "workspace_sessions.terminal_id must equal the canonical session id"
    );
    assert_eq!(
        row.status.as_str(),
        "running",
        "workspace_sessions.status must be 'running' after ensure"
    );

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// Idempotency: second call on a live workspace returns reused=true
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_canonical_session_is_idempotent_when_session_alive() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let workspace_id = "canon-test-ws-idempotent";
    let project = setup_project(workspace_id, "idem-test", "custom");
    write_test_agent_md(&project, "scout", "custom");
    let project_path = project.to_string_lossy().into_owned();

    let first = ensure_canonical_session(&project_path)
        .expect("first ensure should succeed");
    assert!(!first.reused, "first call must be fresh spawn");

    let second = ensure_canonical_session(&project_path)
        .expect("second ensure should succeed");
    assert!(
        second.reused,
        "second call against same live session must report reused=true"
    );
    assert_eq!(
        second.session_id, first.session_id,
        "reused session_id must match the original"
    );

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// Error paths: missing workspace registration, missing primary agent
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_canonical_session_errors_when_workspace_unregistered() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    // Path not in `projects` table → resolver returns None.
    let unregistered = "/tmp/k2so-canonical-test-not-registered";
    let result = ensure_canonical_session(unregistered);
    assert!(result.is_err(), "ensure must error on unregistered workspace");
    let err = result.unwrap_err();
    assert!(
        err.contains("project not registered"),
        "error must explain the cause, got: {err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_canonical_session_errors_when_no_agent_md() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let workspace_id = "canon-test-ws-no-agent";
    let project = setup_project(workspace_id, "no-agent", "custom");
    // Deliberately don't write AGENT.md.
    let project_path = project.to_string_lossy().into_owned();

    let result = ensure_canonical_session(&project_path);
    assert!(
        result.is_err(),
        "ensure must error when no primary agent is defined"
    );
}

// ─────────────────────────────────────────────────────────────────────
// Boot sweep: walks bot-mode workspaces and ensures each
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_sweep_ensures_bot_mode_workspaces_with_agent_md() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    // Three workspaces in three different states — sweep must
    // ensure exactly the one that's both bot-mode AND has AGENT.md.
    let bot_with_agent = setup_project("sweep-bot-agent", "bot+agent", "custom");
    write_test_agent_md(&bot_with_agent, "scout", "custom");

    let _bot_without_agent = setup_project("sweep-bot-no-agent", "bot-only", "custom");
    // No AGENT.md — sweep should skip.

    let _off_workspace = setup_project("sweep-off", "off", "off");
    write_test_agent_md(&_off_workspace, "scout", "custom");
    // Mode 'off' — sweep should skip even though AGENT.md exists.

    boot_sweep_ensure_canonical_sessions();

    // Only the bot+agent workspace should have a canonical session.
    // **0.37.5:** canonical key is bare workspace_id.
    assert!(
        v2_session_map::lookup_by_agent_name("sweep-bot-agent").is_some(),
        "boot sweep must ensure a session for bot-mode + AGENT.md workspaces"
    );
    assert!(
        v2_session_map::lookup_by_agent_name("sweep-bot-no-agent").is_none(),
        "boot sweep must skip bot-mode workspaces without AGENT.md"
    );
    // Regression guard: legacy `<pid>:<agent>` shape MUST NOT exist.
    assert!(
        v2_session_map::lookup_by_agent_name("sweep-bot-agent:scout").is_none(),
        "0.37.5 regression: legacy `sweep-bot-agent:scout` must NOT be registered"
    );
    assert!(
        v2_session_map::lookup_by_agent_name("sweep-off:scout").is_none(),
        "boot sweep must skip mode='off' workspaces"
    );

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// Race contract: SMS bridge's specific scenario
// ─────────────────────────────────────────────────────────────────────

/// The exact race described in the nsi-checkin issue:
///
/// 1. Fresh workspace registered
/// 2. mode=custom set + AGENT.md written
/// 3. Webhook fires `k2so msg --wake` ~150ms later
///
/// After the fix, the canonical session must already exist by the
/// time step 3 happens. The downstream `--wake` cascade should hit
/// Branch 1 (active_terminal_id alive) and inject into THE session,
/// not spawn a duplicate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_then_wake_lands_in_same_session_no_duplicate_spawn() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let workspace_id = "race-test-ws";
    let project = setup_project(workspace_id, "race-test", "custom");
    write_test_agent_md(&project, "scout", "custom");
    let project_path = project.to_string_lossy().into_owned();

    // Step 1: ensure runs first (mode-set or boot sweep).
    let ensured = ensure_canonical_session(&project_path)
        .expect("initial ensure must succeed");
    assert!(!ensured.reused);
    let canonical_session = ensured.session_id.clone();

    // Step 2: a follow-up call (simulating what `k2so msg --wake`
    // does internally — checking for the canonical session before
    // spawning) must report the SAME session, not spawn fresh.
    let post_wake = ensure_canonical_session(&project_path)
        .expect("follow-up ensure must succeed");
    assert!(
        post_wake.reused,
        "follow-up ensure must reuse — race-window spawn would be a regression"
    );
    assert_eq!(
        post_wake.session_id, canonical_session,
        "follow-up call must observe the same canonical session"
    );

    // Step 3: only ONE entry should exist in v2_session_map for
    // this workspace's canonical key.
    // **0.37.5:** canonical key is bare workspace_id.
    let canonical_key = workspace_id.to_string();
    let count = v2_session_map::snapshot()
        .into_iter()
        .filter(|(name, _)| name == &canonical_key)
        .count();
    assert_eq!(
        count, 1,
        "exactly one v2_session_map entry per workspace, got {count}"
    );
    // Regression guard: no legacy `<pid>:<agent>` entry crept in.
    let legacy_key = format!("{workspace_id}:scout");
    assert!(
        v2_session_map::snapshot().iter().all(|(n, _)| n != &legacy_key),
        "0.37.5 regression: legacy {legacy_key} must NOT exist"
    );

    v2_session_map::clear_for_tests();
}
