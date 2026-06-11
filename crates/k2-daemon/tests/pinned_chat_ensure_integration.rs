//! K2SO 0.39.39 — daemon-owned pinned-chat session lifecycle (Phase 1).
//!
//! Pins the `POST /cli/workspace/ensure-pinned-chat` find-or-spawn
//! contract (`crate::pinned_chat::ensure_pinned_chat`):
//!
//!   - A never-chatted workspace spawns FRESH: resumedExisting=false,
//!     argv carries `--session-id <new>`, the session registers under
//!     the canonical `<project_id>` key, and active_terminal_id is
//!     stamped.
//!   - A second ensure WITHOUT forceRespawn returns reused=true with
//!     the SAME session id — no duplicate spawn (the atomic
//!     allocate+register that closes the #682 dup-`--session-id` race).
//!   - An ensure WITH forceRespawn unregisters the existing session
//!     (emitting SessionRemoved) and respawns (emitting SessionAdded),
//!     yielding a NEW session id and re-stamping active_terminal_id.
//!
//! Self-contained: shadows `claude` with a temp-dir shim that `exec`s
//! `cat` (a long-lived stdin reader) so no real claude binary / API
//! call is involved. The daemon PTY spawn enriches the child PATH from
//! the test process's PATH (`daemon_pty.rs` PATH-enrichment), which we
//! prepend the shim dir to.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Mutex as StdMutex;

use k2_core::db::init_for_tests;
use k2_daemon::pinned_chat::ensure_pinned_chat;
use k2_daemon::session_events::{self, SessionEvent};
use k2_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Create a `claude` shim that execs `cat` and prepend its dir to the
/// test process's PATH so the daemon PTY spawn (which enriches the
/// child PATH from `std::env::var("PATH")`) finds it first. Returns the
/// shim dir so the caller can keep it alive for the test's duration.
fn install_claude_shim() -> PathBuf {
    let shim_dir = std::env::temp_dir().join(format!(
        "k2so-pinned-chat-shim-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&shim_dir).unwrap();
    let shim = shim_dir.join("claude");
    // `cat` with no args reads stdin until EOF → long-lived PTY child.
    std::fs::write(&shim, "#!/bin/sh\nexec cat\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();

    let prev = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{}", shim_dir.display(), prev));
    shim_dir
}

fn setup_project(workspace_id: &str, name: &str) -> PathBuf {
    let project_path = std::env::temp_dir().join(format!(
        "k2so-pinned-chat-test-{}-{}-{}",
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
         VALUES (?1, ?2, ?3, 'custom')",
        rusqlite::params![
            workspace_id,
            project_path.to_string_lossy().as_ref(),
            name,
        ],
    )
    .unwrap();
    project_path
}

fn active_terminal_id(workspace_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceSession::get(&conn, workspace_id)
        .unwrap()
        .and_then(|row| row.active_terminal_id)
}

/// Read `workspace_sessions.session_id` (the pinned-chat identity SSOT)
/// for a workspace.
fn saved_session_id(workspace_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceSession::get(&conn, workspace_id)
        .unwrap()
        .and_then(|row| row.session_id)
}

/// Override `$HOME` for the duration of a test so the resolver's
/// on-disk probes (`claude_session_file_exists`,
/// `newest_claude_session_on_disk`) read a controlled
/// `~/.claude/projects` tree instead of the developer's real one.
/// Restores the prior `$HOME` on drop. Mirrors the `U6HomeGuard`
/// pattern in `chat_history.rs`'s unit tests.
struct HomeGuard {
    original: Option<std::ffi::OsString>,
    home: PathBuf,
}

impl HomeGuard {
    fn new(label: &str) -> Self {
        let home = std::env::temp_dir().join(format!(
            "k2so-pinned-chat-home-{}-{}-{}",
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

/// Write a non-empty Claude session `.jsonl` on disk under
/// `$HOME/.claude/projects/<hash>/<session_id>.jsonl` so the resolver's
/// `claude_session_file_exists` happy path (and the
/// `newest_claude_session_on_disk` converge fallback) find a real prior
/// conversation. The shim `claude` (which just execs `cat`) never writes
/// a `.jsonl`, so the test must seed disk truth itself.
fn write_on_disk_session(project_path: &str, session_id: &str) {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
    let root = k2_core::chat_history::resolve_root_project_path(project_path);
    let hash = k2_core::chat_history::claude_project_hash(root);
    let dir = home.join(".claude").join("projects").join(&hash);
    std::fs::create_dir_all(&dir).unwrap();
    // Non-empty: `newest_claude_session_on_disk` skips 0-byte files
    // (never-run pre-allocations) and `--resume` rejects them.
    std::fs::write(
        dir.join(format!("{session_id}.jsonl")),
        b"{\"cwd\":\"/x\",\"sessionId\":\"seed\"}\n",
    )
    .unwrap();
}

/// Force `workspace_sessions.session_id` (the SSOT) to a known value so
/// a test can assert restart-recovery / resolve reads it.
fn set_saved_session_id(workspace_id: &str, session_id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    // The row may not exist yet (cold workspace); INSERT-or-UPDATE on the
    // project_id-UNIQUE key, mirroring the resolver's own upsert shape.
    let row_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
         VALUES (?1, ?2, ?3, 'claude', 'user', 'running', unixepoch()) \
         ON CONFLICT(project_id) DO UPDATE SET session_id = ?3",
        rusqlite::params![row_id, workspace_id, session_id],
    )
    .unwrap();
}

// ─────────────────────────────────────────────────────────────────────
// Fresh spawn on a never-chatted workspace.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_pinned_chat_fresh_spawns_and_registers() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _shim = install_claude_shim();

    let workspace_id = "pinned-fresh-ws";
    let project = setup_project(workspace_id, "fresh");
    let project_path = project.to_string_lossy().into_owned();

    let out = ensure_pinned_chat(&project_path, false, false)
        .expect("ensure should succeed on a never-chatted workspace");

    assert!(!out.reused, "cold workspace must spawn fresh");
    assert!(
        !out.resumed_existing,
        "never-chatted workspace must pre-allocate a fresh session (--session-id), not --resume"
    );
    assert_eq!(out.command, "claude");
    assert!(
        out.args.iter().any(|a| a == "--session-id"),
        "fresh spawn argv must carry --session-id, got: {:?}",
        out.args
    );
    assert!(
        !out.args.iter().any(|a| a == "--resume"),
        "fresh spawn must NOT carry --resume, got: {:?}",
        out.args
    );
    assert!(!out.session_id.is_empty());
    assert!(!out.claude_session_id.is_empty());

    // Registered under the canonical bare-`<project_id>` key.
    let live = v2_session_map::lookup_by_agent_name(workspace_id);
    assert!(
        live.is_some(),
        "v2_session_map missing canonical key {workspace_id} after ensure"
    );
    assert_eq!(live.unwrap().session_id.to_string(), out.session_id);

    // active_terminal_id stamped.
    assert_eq!(
        active_terminal_id(workspace_id).as_deref(),
        Some(out.session_id.as_str()),
        "workspace_sessions.active_terminal_id must equal the fresh session id"
    );

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// Idempotency: second ensure without forceRespawn reuses.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_pinned_chat_second_call_reuses_no_duplicate() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _shim = install_claude_shim();

    let workspace_id = "pinned-reuse-ws";
    let project = setup_project(workspace_id, "reuse");
    let project_path = project.to_string_lossy().into_owned();

    let first = ensure_pinned_chat(&project_path, false, false).expect("first ensure");
    assert!(!first.reused, "first call must be a fresh spawn");

    let second = ensure_pinned_chat(&project_path, false, false).expect("second ensure");
    assert!(
        second.reused,
        "second ensure without forceRespawn must reuse the live session"
    );
    assert_eq!(
        second.session_id, first.session_id,
        "reused session id must match the original — no duplicate spawn"
    );

    // Exactly ONE map entry for the canonical key.
    let count = v2_session_map::snapshot()
        .into_iter()
        .filter(|(k, _)| k == workspace_id)
        .count();
    assert_eq!(count, 1, "exactly one canonical entry, got {count}");

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// forceRespawn: unregister (Removed) + respawn (Added), new id, re-stamp.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_pinned_chat_force_respawn_replaces_and_emits_removed_then_added() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _shim = install_claude_shim();

    let workspace_id = "pinned-force-ws";
    let project = setup_project(workspace_id, "force");
    let project_path = project.to_string_lossy().into_owned();

    let first = ensure_pinned_chat(&project_path, false, false).expect("first ensure");
    assert!(!first.reused);
    let original_session = first.session_id.clone();
    assert_eq!(
        active_terminal_id(workspace_id).as_deref(),
        Some(original_session.as_str())
    );

    // Subscribe BEFORE the force-respawn so we capture both events.
    let mut rx = session_events::subscribe();

    let respawned =
        ensure_pinned_chat(&project_path, true, false).expect("force_respawn ensure");
    assert!(
        !respawned.reused,
        "force_respawn must spawn fresh, not reuse"
    );
    assert_ne!(
        respawned.session_id, original_session,
        "force_respawn must yield a NEW daemon session id"
    );

    // Drain the bus for ~1s; assert we saw SessionRemoved for the
    // original session id THEN SessionAdded for the new one, in order.
    let mut saw_removed_first = false;
    let mut saw_added_after = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(SessionEvent::SessionRemoved { agent_name, .. }))
                if agent_name == workspace_id =>
            {
                if !saw_added_after {
                    saw_removed_first = true;
                }
            }
            Ok(Ok(SessionEvent::SessionAdded {
                agent_name,
                session_id,
                ..
            })) if agent_name == workspace_id => {
                if saw_removed_first && session_id == respawned.session_id {
                    saw_added_after = true;
                    break;
                }
            }
            Ok(Ok(_)) => continue, // contamination from another test
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }
    assert!(
        saw_removed_first,
        "force_respawn must emit SessionRemoved for the canonical key"
    );
    assert!(
        saw_added_after,
        "force_respawn must emit SessionAdded (after Removed) for the new session"
    );

    // active_terminal_id re-stamped to the new session.
    assert_eq!(
        active_terminal_id(workspace_id).as_deref(),
        Some(respawned.session_id.as_str()),
        "active_terminal_id must be re-stamped to the respawned session id"
    );

    // Exactly one canonical entry remains, pointing at the new session.
    let live = v2_session_map::lookup_by_agent_name(workspace_id)
        .expect("canonical entry must exist after respawn");
    assert_eq!(live.session_id.to_string(), respawned.session_id);

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// Error: unregistered workspace.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_pinned_chat_errors_when_workspace_unregistered() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let result = ensure_pinned_chat("/tmp/k2so-pinned-chat-not-registered", false, false);
    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("project not registered"),
        "error must explain the cause"
    );
}

// ─────────────────────────────────────────────────────────────────────
// GH#24 regression lock (pinned-chat-identity-ssot PRD §8): a reused PTY
// `ensure` returns a STABLE claudeSessionId across N calls when a real
// session exists on disk. The old defect re-minted an unconfirmed id on
// every resolve (a reused PTY never ran it → JSONL absent forever →
// re-mint loop on remote/companion clients). With a non-empty `.jsonl`
// seeded on disk + the SSOT pointing at it, every resolve must converge
// to the SAME id.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_pinned_chat_reused_returns_stable_session_id_across_calls() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _home = HomeGuard::new("stable");
    let _shim = install_claude_shim();

    let workspace_id = "pinned-stable-ws";
    let project = setup_project(workspace_id, "stable");
    let project_path = project.to_string_lossy().into_owned();

    // Seed a REAL prior conversation: an on-disk `.jsonl` + the SSOT
    // pointing at it. This is the Detached state the pinned chat resumes.
    let seeded = "11111111-2222-3333-4444-555555555555";
    write_on_disk_session(&project_path, seeded);
    set_saved_session_id(workspace_id, seeded);

    // First ensure: resolver hits the happy path (saved id's JSONL is on
    // disk) → --resume <seeded>, fresh spawn under the canonical key.
    let first = ensure_pinned_chat(&project_path, false, false).expect("first ensure");
    assert!(!first.reused, "first call spawns fresh");
    assert!(
        first.resumed_existing,
        "a workspace with an on-disk session must --resume, not mint"
    );
    assert_eq!(
        first.claude_session_id, seeded,
        "first resolve must resume the seeded on-disk session"
    );
    assert!(
        first.args.iter().any(|a| a == seeded),
        "argv must carry the seeded id, got: {:?}",
        first.args
    );

    // N more ensures REUSE the live PTY. The id must NOT drift — the
    // GH#24 regression lock. Each call re-resolves; with the JSONL still
    // on disk the resolver converges to the same id every time.
    for n in 0..3 {
        let again = ensure_pinned_chat(&project_path, false, false).expect("reused ensure");
        assert!(again.reused, "call #{n} must reuse the live PTY");
        assert_eq!(
            again.session_id, first.session_id,
            "reused daemon session id must be stable (call #{n})"
        );
        assert_eq!(
            again.claude_session_id, seeded,
            "claudeSessionId must be STABLE across reused calls (call #{n}) — GH#24 regression lock"
        );
    }

    // The SSOT was never overwritten with an unconfirmed mint.
    assert_eq!(
        saved_session_id(workspace_id).as_deref(),
        Some(seeded),
        "workspace_sessions.session_id (SSOT) must still hold the seeded id"
    );

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// #681 lock (kept green): a brand-new workspace with NO on-disk session
// still MINTS a fresh `--session-id` and reports resumedExisting=false,
// even with a HomeGuard pointing at an empty `~/.claude/projects`.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_pinned_chat_brand_new_mints_with_no_on_disk_session() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _home = HomeGuard::new("mint");
    let _shim = install_claude_shim();

    let workspace_id = "pinned-mint-ws";
    let project = setup_project(workspace_id, "mint");
    let project_path = project.to_string_lossy().into_owned();

    // No write_on_disk_session: the workspace is genuinely Absent.
    let out = ensure_pinned_chat(&project_path, false, false).expect("ensure on brand-new ws");
    assert!(
        !out.resumed_existing,
        "Absent workspace must MINT (--session-id), not --resume (#681)"
    );
    assert!(
        out.args.iter().any(|a| a == "--session-id"),
        "argv must carry --session-id, got: {:?}",
        out.args
    );
    assert!(
        !out.args.iter().any(|a| a == "--resume"),
        "argv must NOT carry --resume on a brand-new mint, got: {:?}",
        out.args
    );

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// Phase 2 lock (pinned-chat-identity-ssot PRD §4.3.1): restart-recovery
// for the pinned chat resumes from `workspace_sessions.session_id` (the
// SSOT), NOT `workspace_tab_sessions`. We simulate a daemon restart:
// the v2 map is empty, the SSOT holds the prior conversation id, and
// `handle_v2_spawn` lands with an empty command (the renderer's
// canonical re-spawn shape). The recovered PTY must run
// `claude ... --resume <ssot-id>`.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_recovery_resumes_pinned_chat_from_ssot() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _home = HomeGuard::new("recover");
    let _shim = install_claude_shim();

    let workspace_id = "pinned-recover-ws";
    let project = setup_project(workspace_id, "recover");
    let project_path = project.to_string_lossy().into_owned();

    // Prior conversation: on-disk JSONL + SSOT. NO workspace_tab_sessions
    // row is written (Phase 3 stops booking it for the pinned chat), so
    // recovery MUST source the resume id from the SSOT alone.
    let prior = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    write_on_disk_session(&project_path, prior);
    set_saved_session_id(workspace_id, prior);

    // Daemon "restart": the in-memory map is empty (clear above). The
    // renderer re-spawns the canonical pinned chat with an EMPTY command
    // (the v2 default for a restored canonical session) keyed on the bare
    // project_id. The restart-recovery branch must splice
    // `--resume <prior>` from workspace_sessions.session_id.
    let body = serde_json::json!({
        "agent_name": workspace_id,
        "cwd": project_path,
        // command intentionally absent → recovery branch fires.
    })
    .to_string();
    let resp = k2_daemon::v2_spawn::handle_v2_spawn(body.as_bytes());
    assert_eq!(
        resp.status, "200 OK",
        "recovery spawn must succeed, got: {} / {}",
        resp.status, resp.body
    );

    // Inspect the live session's argv: it must carry `--resume <prior>`
    // sourced from the SSOT, and `claude` as the command.
    let live = v2_session_map::lookup_by_agent_name(workspace_id)
        .expect("recovered canonical session must be registered");
    assert_eq!(
        live.program.as_deref(),
        Some("claude"),
        "recovered pinned chat must run `claude`"
    );
    let resume_idx = live
        .args
        .iter()
        .position(|a| a == "--resume")
        .expect("recovered argv must carry --resume");
    assert_eq!(
        live.args.get(resume_idx + 1).map(String::as_str),
        Some(prior),
        "restart-recovery must resume the SSOT id, got argv: {:?}",
        live.args
    );

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// Issue B (daemon-multi-client-arbitration §6) — EXPLICIT SELECTION WINS.
//
// The dropdown switch persists the pick (set-chat-session) then ensures
// with explicit_selection=true. The resolver must honor the saved id
// directly and SKIP the GH#24 converge fallback — an explicit pick can
// never be silently reverted to the newest on-disk session.
//
// This is the core regression lock for the no-op bug: B is picked but its
// `.jsonl` is NOT on disk (path/slug divergence) while a NEWER session A
// exists. On the AUTO path the converge fallback would clobber B → A
// (that's the reproduced no-op). On the EXPLICIT path it must NOT.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_selection_of_on_disk_session_resumes_it_no_converge() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _home = HomeGuard::new("explicit-ok");
    let _shim = install_claude_shim();

    let workspace_id = "pinned-explicit-ok-ws";
    let project = setup_project(workspace_id, "explicit-ok");
    let project_path = project.to_string_lossy().into_owned();

    // Two real on-disk sessions: B (older, the user's pick) and A (newest).
    let session_b = "bbbbbbbb-0000-0000-0000-000000000000";
    let session_a = "aaaaaaaa-1111-1111-1111-111111111111";
    write_on_disk_session(&project_path, session_b);
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_on_disk_session(&project_path, session_a);

    // Currently pinned to A (newest); the dropdown switch persists B.
    set_saved_session_id(workspace_id, session_a);
    set_saved_session_id(workspace_id, session_b); // = the set-chat-session call

    // Explicit ensure (force_respawn + explicit_selection).
    let out = ensure_pinned_chat(&project_path, true, true).expect("explicit ensure");

    assert_eq!(
        out.claude_session_id, session_b,
        "explicit pick of B must resume B, not the newest A (converge skipped)"
    );
    assert!(
        out.resumed_existing,
        "B is on disk → --resume, resumed_existing=true"
    );
    assert!(
        out.args.iter().any(|a| a == "--resume") && out.args.iter().any(|a| a == session_b),
        "argv must carry --resume <B>, got: {:?}",
        out.args
    );
    assert!(
        !out.args.iter().any(|a| a == session_a),
        "argv must NOT carry A, got: {:?}",
        out.args
    );

    // The SSOT was NOT clobbered back to A by a converge fallback.
    assert_eq!(
        saved_session_id(workspace_id).as_deref(),
        Some(session_b),
        "workspace_sessions.session_id must stay B after an explicit switch"
    );

    v2_session_map::clear_for_tests();
}

// AUTO path keeps the GH#24 converge fallback: a stale saved id whose
// JSONL is gone, NO explicit gesture → resume the newest on-disk session.
// This is the regression lock that explicit-selection must not break.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_path_still_converges_when_saved_id_missing() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _home = HomeGuard::new("auto-converge");
    let _shim = install_claude_shim();

    let workspace_id = "pinned-auto-converge-ws";
    let project = setup_project(workspace_id, "auto-converge");
    let project_path = project.to_string_lossy().into_owned();

    // Saved id B is STALE (not on disk); a real session A is on disk.
    let stale_b = "bbbbbbbb-0000-0000-0000-000000000000";
    let real_a = "aaaaaaaa-1111-1111-1111-111111111111";
    write_on_disk_session(&project_path, real_a);
    set_saved_session_id(workspace_id, stale_b);

    // AUTO path: explicit_selection=false (a cold mount / refresh).
    let out = ensure_pinned_chat(&project_path, false, false).expect("auto ensure");

    assert_eq!(
        out.claude_session_id, real_a,
        "auto path must converge to the newest on-disk session (GH#24)"
    );
    assert_eq!(
        saved_session_id(workspace_id).as_deref(),
        Some(real_a),
        "auto path persists the converged id (GH#24 convergence)"
    );

    v2_session_map::clear_for_tests();
}

// Explicit pick of a session whose `.jsonl` is GENUINELY gone → the
// resolver returns an Err (surfaced as a toast), and must NOT silently
// swap to a different session OR clobber the saved id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_selection_of_missing_session_errors_no_silent_swap() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _home = HomeGuard::new("explicit-missing");
    let _shim = install_claude_shim();

    let workspace_id = "pinned-explicit-missing-ws";
    let project = setup_project(workspace_id, "explicit-missing");
    let project_path = project.to_string_lossy().into_owned();

    // Picked B is NOT on disk; a newer A IS — exactly the converge trap.
    let missing_b = "bbbbbbbb-0000-0000-0000-000000000000";
    let real_a = "aaaaaaaa-1111-1111-1111-111111111111";
    write_on_disk_session(&project_path, real_a);
    set_saved_session_id(workspace_id, missing_b); // the explicit pick

    let result = ensure_pinned_chat(&project_path, true, true);
    assert!(
        result.is_err(),
        "explicit pick of a gone session must error, not silently swap"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains(missing_b) && err.contains("no conversation on disk"),
        "error must name the missing session, got: {err}"
    );

    // The saved id must NOT have been clobbered to A.
    assert_eq!(
        saved_session_id(workspace_id).as_deref(),
        Some(missing_b),
        "explicit-missing must NOT rewrite the SSOT to a different session"
    );

    v2_session_map::clear_for_tests();
}
