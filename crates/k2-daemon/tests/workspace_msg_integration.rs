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

// 0.39.0f Phase 2.1 wrap-up: the two `deliver_to_inbox_*` tests that
// previously lived here were removed when the function was deleted
// from `workspace_msg.rs`. See the module docstring above.
