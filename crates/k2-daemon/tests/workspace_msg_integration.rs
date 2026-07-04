//! 0.37.0 simplified messaging — `k2so msg <workspace> "text" [--wake]`.
//!
//! Pins the workspace-token resolver against the schema after
//! unification. The smart-cascade `deliver_live` path requires
//! spawning `claude` (or a substitute) and is exercised end-to-end
//! through `cli/k2so` against a live daemon — those checks live in
//! CI's smoke harness, not here.
//!
//! What we cover here:
//! - `resolve_workspace` accepts name | absolute path | UUID and
//!   only returns a hit when a `projects` row matches.
//!
//! 0.39.0f Phase 2.1 wrap-up: the pre-0.38.6 `deliver_to_inbox` tests
//! moved to history alongside the function itself. New inbox-delivery
//! callers should hit `k2_core::inbox::compose` directly (covered by
//! that crate's own test suite + the `inbox_shape_tauri_parity.sh`
//! CLI test).

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Mutex as StdMutex;

use k2_core::db::init_for_tests;

use k2_daemon::workspace_msg;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn setup_project(workspace_id: &str, name: &str) -> PathBuf {
    let project_path = std::env::temp_dir().join(format!(
        "k2so-ws-msg-test-{}-{}-{}",
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
            name,
        ],
    )
    .unwrap();
    project_path
}

// ─────────────────────────────────────────────────────────────────────
// resolve_workspace
// ─────────────────────────────────────────────────────────────────────

#[test]
fn resolve_workspace_by_name_returns_path() {
    let _g = lock();
    init_for_tests();
    let workspace_id = "ws-msg-resolve-name";
    let project = setup_project(workspace_id, "ResolveTest");

    let resolved = workspace_msg::resolve_workspace("ResolveTest");
    assert_eq!(
        resolved.as_deref(),
        Some(project.to_string_lossy().as_ref()),
        "name lookup should return the project's canonical path"
    );
}

#[test]
fn resolve_workspace_by_absolute_path_returns_path() {
    let _g = lock();
    init_for_tests();
    let workspace_id = "ws-msg-resolve-path";
    let project = setup_project(workspace_id, "PathLookup");
    let path_str = project.to_string_lossy().to_string();

    let resolved = workspace_msg::resolve_workspace(&path_str);
    assert_eq!(
        resolved.as_deref(),
        Some(path_str.as_str()),
        "absolute path lookup should round-trip"
    );
}

#[test]
fn resolve_workspace_by_uuid_returns_path() {
    let _g = lock();
    init_for_tests();
    // Real UUID format (36 chars, 4 dashes) so the resolver's UUID
    // detection branch fires, not the name fallback.
    let workspace_id = "11112222-3333-4444-5555-666677778888";
    let project = setup_project(workspace_id, "UuidLookup");

    let resolved = workspace_msg::resolve_workspace(workspace_id);
    assert_eq!(
        resolved.as_deref(),
        Some(project.to_string_lossy().as_ref()),
        "UUID lookup should return the project's canonical path"
    );
}

#[test]
fn resolve_workspace_unknown_token_returns_none() {
    let _g = lock();
    init_for_tests();
    // Don't set up any project — the resolver must miss cleanly,
    // not panic or return a stale match.
    let resolved = workspace_msg::resolve_workspace("definitely-not-a-real-workspace-name");
    assert!(resolved.is_none(), "missing token should return None");
}

#[test]
fn resolve_workspace_empty_token_returns_none() {
    let _g = lock();
    init_for_tests();
    let resolved = workspace_msg::resolve_workspace("");
    assert!(resolved.is_none(), "empty token must short-circuit, not match every row");
}

// ─────────────────────────────────────────────────────────────────────
// Slice 3b — `--wake` spawns the CANONICAL SESSION'S agent (the
// wake-wrong-binary fix, de-generalization research dragon #3).
//
// Pre-3b, `deliver_live`'s wake branches hardcoded
// `command: Some("claude")` + Claude flag grammar, so waking a dormant
// grok/pi/codex workspace spawned the WRONG binary. These tests drive
// deliver_live end-to-end (real v2 PTY spawn against shim binaries that
// exec `cat`) and inspect the spawned session's program + argv.
// ─────────────────────────────────────────────────────────────────────

/// Override `$HOME` so the resolver's on-disk probes read a controlled
/// tree. Same pattern as pinned_chat_ensure_integration's HomeGuard;
/// TEST_LOCK serializes every test in this binary so the mutation is
/// race-free here.
struct HomeGuard {
    original: Option<std::ffi::OsString>,
    home: PathBuf,
}

impl HomeGuard {
    fn new(label: &str) -> Self {
        let home = std::env::temp_dir().join(format!(
            "k2so-ws-msg-home-{}-{}-{}",
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

/// Shim an agent binary (execs `cat` → long-lived PTY child) and
/// prepend its dir to PATH so the daemon spawn finds it.
fn install_agent_shim(binary: &str) -> PathBuf {
    let shim_dir = std::env::temp_dir().join(format!(
        "k2so-ws-msg-shim-{}-{}-{}",
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

/// Seed a saved canonical session (`workspace_sessions.session_id` +
/// `harness`) — the dormant-but-wakeable shape.
fn set_saved_session(workspace_id: &str, session_id: &str, harness: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
         VALUES (?1, ?2, ?3, ?4, 'user', 'running', unixepoch()) \
         ON CONFLICT(project_id) DO UPDATE SET session_id = ?3, harness = ?4, active_terminal_id = NULL",
        rusqlite::params![row_id, workspace_id, session_id, harness],
    )
    .unwrap();
}

/// Grok on-disk session fixture per the storage study
/// (`~/.grok/sessions/<enc-cwd>/<uuid>/summary.json`).
fn write_grok_session(project_path: &str, session_id: &str) {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
    let dir = home
        .join(".grok")
        .join("sessions")
        .join("%2Ffixture")
        .join(session_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("summary.json"),
        serde_json::json!({
            "info": { "id": session_id, "cwd": project_path },
            "last_active_at": "2026-07-03T10:00:00Z",
        })
        .to_string(),
    )
    .unwrap();
}

/// Claude on-disk session fixture
/// (`~/.claude/projects/<hash>/<sid>.jsonl`, non-empty).
fn write_claude_session(project_path: &str, session_id: &str) {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
    let root = k2_core::chat_history::resolve_root_project_path(project_path);
    let hash = k2_core::chat_history::claude_project_hash(root);
    let dir = home.join(".claude").join("projects").join(&hash);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{session_id}.jsonl")),
        b"{\"cwd\":\"/x\",\"sessionId\":\"seed\"}\n",
    )
    .unwrap();
}

fn kill_canonical_session(workspace_id: &str) {
    if let Some(s) = k2_daemon::v2_session_map::unregister(workspace_id) {
        s.kill();
    }
}

/// THE wake-wrong-binary demonstration: a dormant workspace whose
/// canonical session is GROK (harness='grok', grok conversation on
/// disk) is woken with `--wake` — the spawned PTY must run `grok` with
/// grok's resume grammar, NOT `claude` (the pre-3b hardcode).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wake_of_grok_harness_workspace_spawns_grok_not_claude() {
    let _g = lock();
    init_for_tests();
    let _home = HomeGuard::new("grok-wake");
    let _shim = install_agent_shim("grok");

    let workspace_id = "ws-msg-grok-wake";
    let project = setup_project(workspace_id, "GrokWake");
    let project_path = project.to_string_lossy().into_owned();
    let sid = "01920000-eeee-7000-8000-000000000abc";
    write_grok_session(&project_path, sid);
    set_saved_session(workspace_id, sid, "grok");

    let r = workspace_msg::deliver_live(
        &project_path,
        "wake up",
        "tester",
        "",
        true,
        std::time::Duration::from_millis(300),
    );
    assert!(
        r.success,
        "grok wake must deliver, got reason={:?} hint={:?}",
        r.reason, r.hint
    );
    assert!(r.woke, "a dormant peer wake must report woke=true");

    let live = k2_daemon::v2_session_map::lookup_by_agent_name(workspace_id)
        .expect("woken canonical session must be registered");
    assert_eq!(
        live.program.as_deref(),
        Some("grok"),
        "THE 3b FIX: waking a grok-harness workspace must spawn grok, not claude; argv={:?}",
        live.args
    );
    let resume_idx = live
        .args
        .iter()
        .position(|a| a == "--resume")
        .unwrap_or_else(|| panic!("grok resumes flag-style (--resume <id>): {:?}", live.args));
    assert_eq!(
        live.args.get(resume_idx + 1).map(String::as_str),
        Some(sid),
        "grok resume argv must target the saved grok session: {:?}",
        live.args
    );
    assert!(
        !live
            .args
            .iter()
            .any(|a| a == "--dangerously-skip-permissions"),
        "the skip-permissions flag is CLAUDE-ONLY and must never leak to grok: {:?}",
        live.args
    );

    kill_canonical_session(workspace_id);
}

/// CLAUDE ARGV PIN (resume): waking a dormant claude workspace with a
/// saved, on-disk session must produce the byte-identical pre-3b argv
/// `--dangerously-skip-permissions --resume <sid>`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wake_claude_resume_argv_is_byte_identical() {
    let _g = lock();
    init_for_tests();
    let _home = HomeGuard::new("claude-wake-resume");
    let _shim = install_agent_shim("claude");

    let workspace_id = "ws-msg-claude-wake-resume";
    let project = setup_project(workspace_id, "ClaudeWakeResume");
    let project_path = project.to_string_lossy().into_owned();
    let sid = "aaaaaaaa-bbbb-cccc-dddd-000000000abc";
    write_claude_session(&project_path, sid);
    set_saved_session(workspace_id, sid, "claude");

    let r = workspace_msg::deliver_live(
        &project_path,
        "wake up",
        "tester",
        "",
        true,
        std::time::Duration::from_millis(300),
    );
    assert!(r.success, "claude wake must deliver, got {:?}", r.reason);

    let live = k2_daemon::v2_session_map::lookup_by_agent_name(workspace_id)
        .expect("woken canonical session must be registered");
    assert_eq!(live.program.as_deref(), Some("claude"));
    assert_eq!(
        live.args,
        vec![
            "--dangerously-skip-permissions".to_string(),
            "--resume".to_string(),
            sid.to_string(),
        ],
        "claude wake argv must be byte-identical to the pre-3b hardcode"
    );

    kill_canonical_session(workspace_id);
}

/// CLAUDE ARGV PIN (fresh): waking a workspace with no session anywhere
/// premints — byte-identical `--dangerously-skip-permissions
/// --session-id <new>` shape, with the minted id persisted to the SSOT.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wake_claude_fresh_argv_is_byte_identical_and_premints() {
    let _g = lock();
    init_for_tests();
    let _home = HomeGuard::new("claude-wake-fresh");
    let _shim = install_agent_shim("claude");

    let workspace_id = "ws-msg-claude-wake-fresh";
    let project = setup_project(workspace_id, "ClaudeWakeFresh");
    let project_path = project.to_string_lossy().into_owned();
    // Wakeable via agent_enabled (no saved session at all).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE projects SET agent_enabled = 1 WHERE id = ?1",
            rusqlite::params![workspace_id],
        )
        .unwrap();
    }

    let r = workspace_msg::deliver_live(
        &project_path,
        "wake up",
        "tester",
        "",
        true,
        std::time::Duration::from_millis(300),
    );
    assert!(r.success, "fresh claude wake must deliver, got {:?}", r.reason);

    let live = k2_daemon::v2_session_map::lookup_by_agent_name(workspace_id)
        .expect("woken canonical session must be registered");
    assert_eq!(live.program.as_deref(), Some("claude"));
    assert_eq!(live.args.len(), 3, "flag + premint pair: {:?}", live.args);
    assert_eq!(live.args[0], "--dangerously-skip-permissions");
    assert_eq!(live.args[1], "--session-id");
    let minted = live.args[2].clone();
    assert_eq!(minted.len(), 36, "v4 uuid: {minted}");

    // The premint is persisted to the SSOT with a truthful harness.
    let (saved, harness) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row = k2_core::db::schema::WorkspaceSession::get(&conn, workspace_id)
            .unwrap()
            .expect("workspace_sessions row exists after wake");
        (row.session_id, row.harness)
    };
    assert_eq!(saved.as_deref(), Some(minted.as_str()));
    assert_eq!(harness, "claude");

    kill_canonical_session(workspace_id);
}

// 0.39.0f Phase 2.1 wrap-up: the two `deliver_to_inbox_*` tests that
// previously lived here were removed when the function was deleted
// from `workspace_msg.rs`. See the module docstring above.
