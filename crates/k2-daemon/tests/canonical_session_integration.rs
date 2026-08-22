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

/// Register a projects row with `agent_enabled = 1` — the state every
/// supported mode-setting path (`update_project_setting`, `k2 agent
/// hire/set`) leaves a bot-mode workspace in. The 0.40.24 boot-sweep
/// resurrect gate skips `agent_enabled = 0` rows, and the column's
/// schema default is 0, so tests must set it explicitly like the real
/// write paths do. Use [`set_agent_enabled`] to pause an agent.
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
        "INSERT OR REPLACE INTO projects (id, path, name, agent_mode, agent_enabled) \
         VALUES (?1, ?2, ?3, ?4, 1)",
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

/// Flip `projects.agent_enabled` — what `k2 agent set <name>
/// --enabled false/true` stores (mode untouched).
fn set_agent_enabled(workspace_id: &str, enabled: bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE projects SET agent_enabled = ?1 WHERE id = ?2",
        rusqlite::params![i64::from(enabled), workspace_id],
    )
    .unwrap();
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
// Agent-degeneralization S2: with NO `launch:` block in AGENT.md, the
// fresh spawn honors `projects.default_agent` (resolved against
// agent_presets) instead of the old hardcoded claude default.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_canonical_session_honors_projects_default_agent() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let workspace_id = "canon-test-ws-default-agent";
    let project = setup_project(workspace_id, "default-agent-test", "custom");

    // AGENT.md WITHOUT a `launch:` block — level 1 of the resolver is
    // absent, so the spawn must fall to level 2 (projects.default_agent).
    let dir = project.join(".k2so/agent");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("AGENT.md"),
        "---\nname: scout\ntype: custom\n---\n# scout\n",
    )
    .unwrap();

    // Custom enabled preset whose command is `cat` (self-contained: no
    // claude binary, no API), pointed at by the workspace default.
    let preset_id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
             VALUES (?1, 'canon-cat-agent', 'cat', '', 1, 990, 0)",
            rusqlite::params![preset_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE projects SET default_agent = ?1 WHERE id = ?2",
            rusqlite::params![preset_id, workspace_id],
        )
        .unwrap();
    }

    let project_path = project.to_string_lossy().into_owned();
    let outcome = ensure_canonical_session(&project_path)
        .expect("ensure must succeed via the workspace default agent");
    assert!(!outcome.reused, "cold workspace must spawn fresh");

    let live = v2_session_map::lookup_by_agent_name(workspace_id)
        .expect("canonical session must be registered");
    assert_eq!(
        live.program.as_deref(),
        Some("cat"),
        "fresh canonical spawn must run the workspace's default agent \
         (projects.default_agent → preset command), NOT hardcoded claude"
    );

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// Slice 3b — command ownership: the stored HARNESS wins for the
// canonical key when it disagrees with the launch profile.
// ─────────────────────────────────────────────────────────────────────

/// Scratch-HOME guard (controlled on-disk provider stores). TEST_LOCK
/// serializes every test in this binary, so the env mutation is safe.
struct HomeGuard {
    original: Option<std::ffi::OsString>,
    home: PathBuf,
}

impl HomeGuard {
    fn new(label: &str) -> Self {
        let home = std::env::temp_dir().join(format!(
            "k2so-canonical-home-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let original = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        Self { original, home }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// Shim an agent binary (execs `cat`) onto PATH.
fn install_agent_shim(binary: &str) -> PathBuf {
    let shim_dir = std::env::temp_dir().join(format!(
        "k2so-canonical-shim-{}-{}-{}",
        binary,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&shim_dir).unwrap();
    let shim = shim_dir.join(binary);
    std::fs::write(&shim, "#!/bin/sh\nexec cat\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let prev = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{}", shim_dir.display(), prev));
    shim_dir
}

/// A workspace whose launch profile resolves to CLAUDE (no `launch:`
/// block, no workspace default → level-4 fallback) but whose CANONICAL
/// SESSION is GROK (harness='grok' + grok conversation on disk): the
/// ensure respawn must run GROK resuming that conversation — the
/// harness wins over the profile for the canonical key (Slice 3b; 3a
/// spawned the profile bare here, losing the session).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_canonical_session_harness_wins_over_profile_command() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _home = HomeGuard::new("harness-wins");
    let _shim = install_agent_shim("grok");

    let workspace_id = "canon-test-ws-harness-wins";
    let project = setup_project(workspace_id, "harness-wins", "custom");
    // AGENT.md WITHOUT a `launch:` block → profile = default agent
    // resolution → literal claude (scratch HOME has no settings.json,
    // no projects.default_agent set).
    let dir = project.join(".k2so/agent");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("AGENT.md"),
        "---\nname: scout\ntype: custom\n---\n# scout\n",
    )
    .unwrap();
    let project_path = project.to_string_lossy().into_owned();

    // The canonical session is grok: harness + on-disk conversation.
    let sid = "01920000-bbbb-7000-8000-000000000abc";
    {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
        let grok_dir = home
            .join(".grok")
            .join("sessions")
            .join("%2Ffixture")
            .join(sid);
        std::fs::create_dir_all(&grok_dir).unwrap();
        std::fs::write(
            grok_dir.join("summary.json"),
            serde_json::json!({
                "info": { "id": sid, "cwd": project_path },
                "last_active_at": "2026-07-03T10:00:00Z",
            })
            .to_string(),
        )
        .unwrap();
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, 'grok', 'user', 'sleeping', unixepoch()) \
             ON CONFLICT(project_id) DO UPDATE SET session_id = ?3, harness = 'grok'",
            rusqlite::params![row_id, workspace_id, sid],
        )
        .unwrap();
    }

    let outcome = ensure_canonical_session(&project_path)
        .expect("ensure must succeed for a grok-harness canonical session");
    assert!(!outcome.reused, "cold map must spawn fresh");

    let live = v2_session_map::lookup_by_agent_name(workspace_id)
        .expect("canonical session must be registered");
    assert_eq!(
        live.program.as_deref(),
        Some("grok"),
        "the HARNESS (canonical session's agent) must win over the \
         claude profile command; argv={:?}",
        live.args
    );
    let resume_idx = live
        .args
        .iter()
        .position(|a| a == "--resume")
        .unwrap_or_else(|| panic!("grok respawn must resume the saved session: {:?}", live.args));
    assert_eq!(
        live.args.get(resume_idx + 1).map(String::as_str),
        Some(sid),
        "respawn must resume the stored grok conversation: {:?}",
        live.args
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
    // Deliberately don't write AGENT.md — and clear the enabled flag,
    // since `projects.agent_enabled = 1` ALONE makes an agent
    // resolvable (agent_identity contract b). This test is the
    // genuinely-no-agent case.
    set_agent_enabled(workspace_id, false);
    let project_path = project.to_string_lossy().into_owned();

    let result = ensure_canonical_session(&project_path);
    assert!(
        result.is_err(),
        "ensure must error when no primary agent is defined"
    );
}

// ─────────────────────────────────────────────────────────────────────
// Boot sweep: daemon start does not spawn the canonical fleet
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_sweep_does_not_spawn_canonical_chats() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    // Bot-mode + AGENT.md is exactly the fleet the old sweep spawned.
    let bot_with_agent = setup_project("sweep-bot-agent", "bot+agent", "custom");
    write_test_agent_md(&bot_with_agent, "scout", "custom");

    let _bot_without_agent = setup_project("sweep-bot-no-agent", "bot-only", "custom");
    let _off_workspace = setup_project("sweep-off", "off", "off");
    write_test_agent_md(&_off_workspace, "scout", "custom");

    boot_sweep_ensure_canonical_sessions();

    assert!(
        v2_session_map::lookup_by_agent_name("sweep-bot-agent").is_none(),
        "boot sweep must not spawn canonical chats — daemon start is zero until a need"
    );
    assert!(
        v2_session_map::lookup_by_agent_name("sweep-bot-no-agent").is_none(),
        "boot sweep must not spawn for bot-mode without AGENT.md"
    );
    assert!(
        v2_session_map::lookup_by_agent_name("sweep-bot-agent:scout").is_none(),
        "0.37.5 regression: legacy `sweep-bot-agent:scout` must NOT be registered"
    );
    assert!(
        v2_session_map::lookup_by_agent_name("sweep-off:scout").is_none(),
        "boot sweep must not spawn mode='off' workspaces"
    );

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// Resurrect gate (0.40.24 S5): a paused agent stays paused across a
// daemon restart
// ─────────────────────────────────────────────────────────────────────

/// `k2 agent set <name> --enabled false` pauses an agent WITHOUT
/// changing its mode. Before the S5 gate, the boot sweep resurrected
/// every bot-mode workspace regardless of `agent_enabled`, so the
/// pause silently didn't survive a daemon restart. The sweep must now
/// skip disabled agents — and still resurrect enabled ones.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_sweep_does_not_resurrect_enabled_or_disabled_agents() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let enabled_ws = setup_project("sweep-gate-enabled", "gate-enabled", "custom");
    write_test_agent_md(&enabled_ws, "scout", "custom");

    let disabled_ws = setup_project("sweep-gate-disabled", "gate-disabled", "custom");
    write_test_agent_md(&disabled_ws, "scout", "custom");
    set_agent_enabled("sweep-gate-disabled", false);

    boot_sweep_ensure_canonical_sessions();

    assert!(
        v2_session_map::lookup_by_agent_name("sweep-gate-enabled").is_none(),
        "boot sweep must not resurrect even an ENABLED bot-mode agent"
    );
    assert!(
        v2_session_map::lookup_by_agent_name("sweep-gate-disabled").is_none(),
        "boot sweep must not resurrect a paused agent"
    );

    set_agent_enabled("sweep-gate-disabled", true);
    boot_sweep_ensure_canonical_sessions();
    assert!(
        v2_session_map::lookup_by_agent_name("sweep-gate-disabled").is_none(),
        "re-enabling + boot sweep still must not spawn — need-driven ensure does"
    );

    let path = disabled_ws.to_string_lossy().into_owned();
    let ensured = ensure_canonical_session(&path).expect("explicit ensure is a need");
    assert!(!ensured.reused, "explicit ensure must spawn the dead canonical chat");
    assert!(
        v2_session_map::lookup_by_agent_name("sweep-gate-disabled").is_some(),
        "explicit ensure_canonical_session must spawn after boot left it dead"
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
