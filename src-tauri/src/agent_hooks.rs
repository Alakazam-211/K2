//! Agent lifecycle hook glue (Tauri side).
//!
//! The hook notification server (HTTP listener + `/cli/*` dispatcher) was
//! retired in 0.39.0; all hook traffic now terminates in `k2so-daemon`.
//! What remains in this module:
//!
//!   - Tauri command shims (`k2so_heartbeat_*`, `k2so_session_lookup_*`,
//!     `k2so_sessions_list_for_workspace`) that the renderer invokes
//!     and which proxy through to the daemon's `/cli/*` routes.
//!   - Nothing about hook installation: the daemon writes `notify.sh` and
//!     the per-CLI hook configs itself (prd-daemon-activity-and-thread-
//!     working-v1 DA7, `k2_core::agent_hooks::install`), so a headless
//!     daemon has hooks and an older app bundle can't fight it.
//!
//! See `src-tauri/src/lib.rs` for the broader Tauri-vs-daemon split.

// Ring buffer + canonical event mapping + URL query parsing now live
// in k2so-core so the daemon can share them. The Tauri-side HTTP
// dispatcher that consumed these helpers at runtime was retired in
// 0.39.0 (see the deletion manifest under "Family 6"); the remaining
// references are exclusively in the unit-test module below, so the
// imports are gated behind `#[cfg(test)]` to keep release builds
// warning-clean.
#[cfg(test)]
use k2_core::agent_hooks::{
    get_recent_events, map_event_type, record_recent_event, RecentEvent,
};
#[cfg(test)]
const RECENT_EVENTS_CAP: usize = 50;

use crate::daemon_client::{off_main, DaemonClient};

// Every command below is `async` and runs its daemon call on the blocking
// pool (`off_main`): a sync Tauri command runs ON the main thread, so a
// slow or wedged daemon (10 s reqwest timeout) froze the window. The
// request each one sends is unchanged; the `*_with` fns hold it so the
// tests can aim them at a fake daemon.

/// Force-fire a specific heartbeat by name, bypassing its schedule. The
/// frontend uses this for per-row Launch buttons in the workspace drawer
/// so the user can manually kick off a scheduled workflow without waiting
/// for the cron window. Thin proxy to the daemon's `/cli/heartbeat/launch`
/// route, which runs the smart-launch decision tree (fresh-fire /
/// inject-into-live-session / resume-and-fire) — the same path the cron
/// tick and `k2 heartbeat launch <name>` take. All real logic lives in
/// `crates/k2-daemon/src/heartbeat_launch.rs`. Kept under the legacy
/// `k2so_heartbeat_force_fire` name since the renderer still references
/// it from the WorkspacePanel header path; `k2so_heartbeat_smart_launch`
/// is the canonical name going forward.
#[tauri::command]
pub async fn k2so_heartbeat_force_fire(
    project_path: String,
    name: String,
) -> Result<String, String> {
    off_main(move || heartbeat_launch_with(&DaemonClient::try_connect()?, &project_path, &name)).await
}

/// Smart-launch a heartbeat — invoked by the React Launch button.
/// Identical impl to `k2so_heartbeat_force_fire`; the rename is a
/// signal that this is the daemon-first path going forward and that
/// callers shouldn't expect Tauri-side spawn behaviour.
#[tauri::command]
pub async fn k2so_heartbeat_smart_launch(
    project_path: String,
    name: String,
) -> Result<String, String> {
    off_main(move || heartbeat_launch_with(&DaemonClient::try_connect()?, &project_path, &name)).await
}

fn heartbeat_launch_with(client: &DaemonClient, project_path: &str, name: &str) -> Result<String, String> {
    client.cli_post_query(
        "/cli/heartbeat/launch",
        &[("project", project_path), ("name", name)],
    )
}

/// Resolve a heartbeat's currently-live PTY (if any) so the renderer
/// can decide whether to attach a tab to an existing daemon-owned
/// session vs spawn a fresh resume. Returns JSON:
///   `{ name, claudeSessionId, activeTerminalId, sessionAlive }`.
/// `activeTerminalId` is non-null only when the daemon's
/// `v2_session_map` has the corresponding session — the daemon
/// performs lazy-cleanup of stale columns inside the same call.
/// See `.k2so/prds/heartbeat-active-session-tracking.md`.
#[tauri::command]
pub async fn k2so_heartbeat_active_session(
    project_path: String,
    name: String,
) -> Result<String, String> {
    off_main(move || heartbeat_active_session_with(&DaemonClient::try_connect()?, &project_path, &name)).await
}

fn heartbeat_active_session_with(client: &DaemonClient, project_path: &str, name: &str) -> Result<String, String> {
    client.cli_get(
        "/cli/heartbeat/active-session",
        &[("project", project_path), ("name", name)],
    )
}

/// Resolve a live PTY by agent_name across both legacy session_map
/// and v2_session_map. Used by AgentChatPane on mount to decide
/// whether to attach to an existing daemon-owned session vs spawn
/// fresh. Returns JSON:
///   `{ agentName, sessionId, sessionAlive, isV2 }`.
/// `sessionId` is non-null only when a live session exists.
#[tauri::command]
pub async fn k2so_session_lookup_by_agent(agent: String) -> Result<String, String> {
    off_main(move || session_lookup_by_agent_with(&DaemonClient::try_connect()?, &agent)).await
}

fn session_lookup_by_agent_with(client: &DaemonClient, agent: &str) -> Result<String, String> {
    client.cli_get("/cli/sessions/lookup-by-agent", &[("agent", agent)])
}

/// 0.37.11 A9 phase 4a — list live daemon sessions for a workspace.
/// `path` is the workspace path (project path or worktree path);
/// returns JSON array of `{ sessionId, agentName, command, args, cwd, isV2 }`
/// for every session whose cwd is under `path`.
///
/// Renderer calls this in `tabsStore.loadLayoutForWorkspace` before
/// falling back to `launchDefaultAgent`, so opening a workspace in a
/// second window adopts existing PTYs rather than spawning duplicates.
#[tauri::command]
pub async fn k2so_sessions_list_for_workspace(path: String) -> Result<String, String> {
    off_main(move || sessions_list_for_workspace_with(&DaemonClient::try_connect()?, &path)).await
}

fn sessions_list_for_workspace_with(client: &DaemonClient, path: &str) -> Result<String, String> {
    client.cli_get("/cli/sessions/list-for-workspace", &[("path", path)])
}

// 0.46.4: the orphaned `get_port` / `get_token` (k2-core `hook_config`),
// `generate_token` and `shell_escape` helpers were removed with the
// k2-core link; nothing called them since the 0.39.0 dispatcher move.


// ── CLI DB Helpers (retired 0.39.0e) ────────────────────────────────────
// `cli_update_project_setting`, `cli_remove_workspace`, and
// `cli_get_project_settings` were the in-process SQLite-backed
// helpers the old Tauri /cli/* dispatcher routed through. The
// dispatcher was deleted in 0.39.0a-d (8df44a05) and the three
// helpers were orphaned behind `#[allow(dead_code)]`. They are now
// removed: the equivalent surface lives on the daemon as
// `/cli/agents/mode`, `/cli/agents/settings/*`, and
// `/cli/workspace/remove` (see crates/k2so-daemon/src/cli.rs), and
// the daemon delegates into `k2_core::workspace::*` for the
// actual DB work — the canonical single source of truth.
//
// The workspace-harness teardown side of `cli_remove_workspace`
// (`teardown_workspace_harness_files`) stays in Tauri for now under
// `commands::k2so_agents`; callers that need both teardown and DB
// removal should invoke the daemon route for the SQL delete and
// then drive teardown directly. Folding teardown into the daemon
// route is its own follow-up (move `teardown_workspace_harness_files`
// into core first).

#[cfg(test)]
mod tests {
    use super::*;
    // 0.39.0c: parking_lot::Mutex was pruned from the module's
    // top-level use-list when the start_server dispatcher (which
    // owned the only non-test uses) was deleted. Re-import here so
    // the test serialization lock keeps working without polluting
    // the production module with an unused import.
    use parking_lot::Mutex;

    /// Ring-buffer tests share global state — serialize them.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset_recent_events() {
        k2_core::agent_hooks::clear_recent_events();
    }

    fn snapshot_recent_events() -> Vec<RecentEvent> {
        get_recent_events()
    }

    #[test]
    fn ring_buffer_records_matched_events() {
        let _g = TEST_LOCK.lock();
        reset_recent_events();
        record_recent_event("UserPromptSubmit", Some("start"), "pane-1", "tab-1");
        let events = snapshot_recent_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].raw_event, "UserPromptSubmit");
        assert_eq!(events[0].canonical.as_deref(), Some("start"));
        assert_eq!(events[0].pane_id, "pane-1");
        assert_eq!(events[0].tab_id, "tab-1");
        assert!(events[0].matched);
    }

    #[test]
    fn ring_buffer_records_unmatched_events() {
        let _g = TEST_LOCK.lock();
        reset_recent_events();
        record_recent_event("NoSuchEvent", None, "pane-2", "tab-2");
        let events = snapshot_recent_events();
        assert_eq!(events.len(), 1);
        assert!(!events[0].matched);
        assert!(events[0].canonical.is_none());
    }

    #[test]
    fn ring_buffer_caps_at_limit() {
        let _g = TEST_LOCK.lock();
        reset_recent_events();
        for i in 0..RECENT_EVENTS_CAP + 10 {
            record_recent_event(
                &format!("Event{}", i),
                Some("start"),
                "pane-cap",
                "tab-cap",
            );
        }
        let events = snapshot_recent_events();
        assert_eq!(events.len(), RECENT_EVENTS_CAP);
        // Oldest should have been dropped — first recorded was Event0
        assert_eq!(events[0].raw_event, format!("Event{}", 10));
        assert_eq!(
            events.last().unwrap().raw_event,
            format!("Event{}", RECENT_EVENTS_CAP + 9)
        );
    }

    #[test]
    fn map_event_type_covers_primary_lifecycle() {
        // Claude Code
        assert_eq!(map_event_type("UserPromptSubmit"), Some("start"));
        assert_eq!(map_event_type("PostToolUse"), Some("start"));
        assert_eq!(map_event_type("Stop"), Some("stop"));
        assert_eq!(map_event_type("Notification"), Some("permission"));
        // Codex
        assert_eq!(map_event_type("agent-turn-complete"), Some("stop"));
        // Cursor
        assert_eq!(map_event_type("beforeSubmitPrompt"), Some("start"));
        assert_eq!(map_event_type("beforeShellExecution"), Some("permission"));
        // Unknown events must return None
        assert_eq!(map_event_type("NoSuchEvent"), None);
    }

    // ── The forwarders (off the main thread, same request as before) ──

    use crate::daemon_client::test_support::{pairs, FakeDaemon};

    #[test]
    fn heartbeat_launch_posts_project_and_name() {
        let fake = FakeDaemon::serve(vec![(200, r#"{"launched":true}"#)]);
        let client = DaemonClient::for_test(&fake.base, "tok");
        let out = tauri::async_runtime::block_on(off_main(move || {
            heartbeat_launch_with(&client, "/w/my proj", "daily & more")
        }));
        assert_eq!(out, Ok(r#"{"launched":true}"#.to_string()));
        let seen = fake.next();
        assert_eq!(seen.method, "POST");
        assert_eq!(seen.path, "/cli/heartbeat/launch");
        assert_eq!(seen.query, pairs(&[("token", "tok"), ("project", "/w/my proj"), ("name", "daily & more")]));
        assert_eq!(seen.body, "");
    }

    #[test]
    fn heartbeat_launch_surfaces_a_daemon_error() {
        let fake = FakeDaemon::serve(vec![(404, "no such heartbeat")]);
        let client = DaemonClient::for_test(&fake.base, "tok");
        let out = tauri::async_runtime::block_on(off_main(move || heartbeat_launch_with(&client, "/w", "x")));
        assert_eq!(out, Err("daemon /cli/heartbeat/launch 404: no such heartbeat".to_string()));
    }

    #[test]
    fn heartbeat_active_session_gets_project_and_name() {
        let fake = FakeDaemon::serve(vec![(200, r#"{"sessionAlive":false}"#)]);
        let client = DaemonClient::for_test(&fake.base, "tok");
        let out = tauri::async_runtime::block_on(off_main(move || heartbeat_active_session_with(&client, "/w", "hb")));
        assert_eq!(out, Ok(r#"{"sessionAlive":false}"#.to_string()));
        let seen = fake.next();
        assert_eq!(seen.method, "GET");
        assert_eq!(seen.path, "/cli/heartbeat/active-session");
        assert_eq!(seen.query, pairs(&[("token", "tok"), ("project", "/w"), ("name", "hb")]));
    }

    #[test]
    fn session_lookups_get_their_routes() {
        let fake = FakeDaemon::serve(vec![(200, r#"{"sessionId":null}"#), (200, "[]")]);
        let client = DaemonClient::for_test(&fake.base, "tok");
        let (a, b) = tauri::async_runtime::block_on(off_main(move || {
            Ok((
                session_lookup_by_agent_with(&client, "sales/reviewer"),
                sessions_list_for_workspace_with(&client, "/w/tree"),
            ))
        }))
        .expect("ran");
        assert_eq!(a, Ok(r#"{"sessionId":null}"#.to_string()));
        assert_eq!(b, Ok("[]".to_string()));
        let first = fake.next();
        assert_eq!((first.method.as_str(), first.path.as_str()), ("GET", "/cli/sessions/lookup-by-agent"));
        assert_eq!(first.query, pairs(&[("token", "tok"), ("agent", "sales/reviewer")]));
        let second = fake.next();
        assert_eq!((second.method.as_str(), second.path.as_str()), ("GET", "/cli/sessions/list-for-workspace"));
        assert_eq!(second.query, pairs(&[("token", "tok"), ("path", "/w/tree")]));
    }

    #[test]
    fn commands_report_a_missing_daemon_without_blocking() {
        // A `.k2` dir with no daemon.port: the same Err the sync command
        // returned, now from the blocking pool.
        let dir = tempfile::tempdir().expect("tempdir");
        let k2 = dir.path().to_path_buf();
        let out = tauri::async_runtime::block_on(off_main(move || {
            session_lookup_by_agent_with(&DaemonClient::try_connect_in(&k2)?, "a")
        }));
        let err = out.expect_err("no daemon.port must be an error");
        assert!(err.starts_with("read ") && err.contains("daemon.port"), "got: {err}");
    }
}
