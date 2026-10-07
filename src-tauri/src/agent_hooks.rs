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
//!   - A small set of DB-direct helpers (`cli_*`) flagged for migration
//!     to daemon API calls in a follow-up release (see deletion
//!     manifest Family 3).
//!
//! See `src-tauri/src/lib.rs` for the broader Tauri-vs-daemon split.

// Port + token moved to k2_core::hook_config so the terminal backend
// (also in k2so-core) can read them without a circular dep.
use k2_core::hook_config;

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

/// Force-fire a specific heartbeat by name, bypassing its schedule. The
/// frontend uses this for per-row Launch buttons in the workspace drawer
/// so the user can manually kick off a scheduled workflow without waiting
/// for the cron window. Path is identical to a scheduled fire: resolve
/// the row → read its wakeup.md → spawn_wake_pty → stamp last_fired →
/// write a heartbeat_fires audit row (decision='fired', reason='forced').
///
/// Force-fire a specific heartbeat by name. Thin proxy to the daemon's
/// `/cli/heartbeat/launch` route which runs the smart-launch decision
/// tree (fresh-fire / inject-into-live-session / resume-and-fire) —
/// the same path the cron tick and `k2so heartbeat launch <name>`
/// CLI verb take. All real logic lives in
/// `crates/k2so-daemon/src/heartbeat_launch.rs`; this Tauri command
/// exists only so the React Launch button can trigger it via invoke
/// without a separate HTTP client. Kept under the legacy
/// `k2so_heartbeat_force_fire` name since the renderer still
/// references it from the WorkspacePanel header path; the new
/// `k2so_heartbeat_smart_launch` is the canonical name going forward.
#[tauri::command]
pub fn k2so_heartbeat_force_fire(
    project_path: String,
    name: String,
) -> Result<String, String> {
    let client = crate::daemon_client::DaemonClient::try_connect()?;
    client.cli_post_query(
        "/cli/heartbeat/launch",
        &[("project", &project_path), ("name", &name)],
    )
}

/// Smart-launch a heartbeat — invoked by the React Launch button.
/// Identical impl to `k2so_heartbeat_force_fire`; the rename is a
/// signal that this is the daemon-first path going forward and that
/// callers shouldn't expect Tauri-side spawn behaviour.
#[tauri::command]
pub fn k2so_heartbeat_smart_launch(
    project_path: String,
    name: String,
) -> Result<String, String> {
    let client = crate::daemon_client::DaemonClient::try_connect()?;
    client.cli_post_query(
        "/cli/heartbeat/launch",
        &[("project", &project_path), ("name", &name)],
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
pub fn k2so_heartbeat_active_session(
    project_path: String,
    name: String,
) -> Result<String, String> {
    let client = crate::daemon_client::DaemonClient::try_connect()?;
    client.cli_get(
        "/cli/heartbeat/active-session",
        &[("project", &project_path), ("name", &name)],
    )
}

/// Resolve a live PTY by agent_name across both legacy session_map
/// and v2_session_map. Used by AgentChatPane on mount to decide
/// whether to attach to an existing daemon-owned session vs spawn
/// fresh. Returns JSON:
///   `{ agentName, sessionId, sessionAlive, isV2 }`.
/// `sessionId` is non-null only when a live session exists.
#[tauri::command]
pub fn k2so_session_lookup_by_agent(agent: String) -> Result<String, String> {
    let client = crate::daemon_client::DaemonClient::try_connect()?;
    client.cli_get("/cli/sessions/lookup-by-agent", &[("agent", &agent)])
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
pub fn k2so_sessions_list_for_workspace(path: String) -> Result<String, String> {
    let client = crate::daemon_client::DaemonClient::try_connect()?;
    client.cli_get("/cli/sessions/list-for-workspace", &[("path", &path)])
}

// The helpers below (`get_port`, `get_token`, `generate_token`,
// `shell_escape`) were callers' entry points into the deleted Tauri
// HTTP dispatcher (`start_server`). The dispatcher itself moved to
// the daemon in 0.39.0 (see deletion manifest "Family 6"), so these
// helpers no longer have intra-crate callers — but they remain on
// the kept-list because external code paths may need to read the
// daemon's hook config or escape shell args for future hook scripts.
// `#[allow(dead_code)]` keeps the release build warning-clean
// without obscuring the orphan status for future readers.

/// Get the port the notification server is listening on. Thin
/// re-export kept for existing callers (`crate::agent_hooks::get_port`);
/// the real static lives in `k2_core::hook_config`.
#[allow(dead_code)]
pub fn get_port() -> u16 {
    hook_config::get_port()
}

/// Get the auth token for hook requests.
#[allow(dead_code)]
pub fn get_token() -> &'static str {
    hook_config::get_token()
}

/// Generate a cryptographically secure random hex token.
#[allow(dead_code)]
fn generate_token() -> String {
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf).expect("failed to generate random token");
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Canonical agent lifecycle event types.
// AgentLifecycleEvent / map_event_type / parse_query_params / urldecode
// all moved to k2_core::agent_hooks (imported at the top of this
// file). The daemon's future HTTP handlers share the same helpers so
// the canonical event bucket list can't drift between host and daemon.

/// Shell-escape a string for safe interpolation into shell commands.
/// Uses single-quote wrapping with escaped internal single quotes.
#[allow(dead_code)]
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}


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
}
