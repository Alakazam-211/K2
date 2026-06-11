//! Agent lifecycle hook glue (Tauri side).
//!
//! The hook notification server (HTTP listener + `/cli/*` dispatcher) was
//! retired in 0.39.0; all hook traffic now terminates in `k2so-daemon`.
//! What remains in this module:
//!
//!   - Tauri command shims (`k2so_heartbeat_*`, `k2so_session_lookup_*`,
//!     `k2so_sessions_list_for_workspace`) that the renderer invokes
//!     and which proxy through to the daemon's `/cli/*` routes.
//!   - Hook-script generation/installation (`generate_hook_script`,
//!     `write_hook_script`, `register_{claude,cursor,gemini}_hooks`,
//!     `register_all_hooks`, `check_hook_injections`) — these still
//!     need to run inside Tauri because they touch the user's per-CLI
//!     config files under `~/.claude`, `~/.cursor`, `~/.config/gemini`.
//!   - A small set of DB-direct helpers (`cli_*`) flagged for migration
//!     to daemon API calls in a follow-up release (see deletion
//!     manifest Family 3).
//!
//! See `src-tauri/src/lib.rs` for the broader Tauri-vs-daemon split.

use tauri::AppHandle;

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
    get_recent_events, map_event_type, record_recent_event, urldecode,
    AgentLifecycleEvent, RecentEvent,
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
    client.cli_get(
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
    client.cli_get(
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

/// Generate the notify hook bash script content.
///
/// The script reads the port and (optionally) the auth token from
/// `~/.k2so/heartbeat.port` / `~/.k2so/heartbeat.token` at exec time
/// rather than baking them in. This means a K2SO restart (which picks a
/// new random port + may rotate the token) doesn't silently break
/// hooks in already-running Claude/Cursor/Gemini sessions.
///
/// The `_port` parameter is kept for API compatibility but no longer
/// used — the caller can regenerate the script whenever they want.
pub fn generate_hook_script(_port: u16) -> String {
    r#"#!/bin/bash
# K2SO Agent Lifecycle Hook — DO NOT EDIT (managed by K2SO)
# This script is called by agent CLIs to notify K2SO of lifecycle events.

# Port and token are read at exec time so a K2SO restart (new random
# port / rotated token) doesn't break hooks in long-running LLM sessions.
K2SO_PORT_FILE="$HOME/.k2so/heartbeat.port"
K2SO_TOKEN_FILE="$HOME/.k2so/heartbeat.token"

if [ ! -r "$K2SO_PORT_FILE" ]; then
    # K2SO isn't running — exit silently so we don't block the agent
    exit 0
fi
K2SO_PORT=$(cat "$K2SO_PORT_FILE" 2>/dev/null)
[ -z "$K2SO_PORT" ] && exit 0

# Token from disk takes precedence over the PTY-injected env var so that
# a K2SO restart (which rotates the token) immediately picks up the new
# value instead of sending 403-rejected requests for the rest of the
# session's life.
if [ -r "$K2SO_TOKEN_FILE" ]; then
    K2SO_HOOK_TOKEN=$(cat "$K2SO_TOKEN_FILE" 2>/dev/null)
fi

# Read JSON from argument or stdin
INPUT="${1:-}"
if [ -z "$INPUT" ]; then
    INPUT=$(cat 2>/dev/null || true)
fi

# Extract event type — try multiple JSON key patterns
EVENT_TYPE=""
for key in hook_event_name type event eventType; do
    val=$(echo "$INPUT" | grep -o "\"$key\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" | head -1 | sed 's/.*"'"$key"'"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')
    if [ -n "$val" ]; then
        EVENT_TYPE="$val"
        break
    fi
done

# If event type was passed as first arg (Cursor style: script.sh Start)
if [ -z "$EVENT_TYPE" ] && [ -n "$1" ] && ! echo "$1" | grep -q '{'; then
    EVENT_TYPE="$1"
fi

[ -z "$EVENT_TYPE" ] && exit 0
[ -z "$K2SO_TAB_ID" ] && exit 0

curl -sG "http://127.0.0.1:$K2SO_PORT/hook/complete" \
    --connect-timeout 1 --max-time 2 \
    --data-urlencode "paneId=$K2SO_PANE_ID" \
    --data-urlencode "tabId=$K2SO_TAB_ID" \
    --data-urlencode "eventType=$EVENT_TYPE" \
    --data-urlencode "token=$K2SO_HOOK_TOKEN" \
    >/dev/null 2>&1 || true

exit 0
"#.to_string()
}

/// Write the hook script to ~/.k2so/hooks/notify.sh
pub fn write_hook_script(port: u16) -> Result<String, String> {
    let home = dirs::home_dir().ok_or("No home directory")?;
    let hooks_dir = home.join(".k2").join("hooks");
    std::fs::create_dir_all(&hooks_dir).map_err(|e| e.to_string())?;

    let script_path = hooks_dir.join("notify.sh");
    let content = generate_hook_script(port);
    std::fs::write(&script_path, &content).map_err(|e| e.to_string())?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&script_path, perms).map_err(|e| e.to_string())?;
    }

    Ok(script_path.to_string_lossy().to_string())
}

/// Register hooks with Claude Code (~/.claude/settings.json)
fn register_claude_hooks(hook_script: &str) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("No home directory")?;
    let settings_path = home.join(".claude").join("settings.json");

    // Read existing settings or create new
    let mut settings: serde_json::Value = if settings_path.exists() {
        let raw = std::fs::read_to_string(&settings_path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).unwrap_or(serde_json::json!({}))
    } else {
        std::fs::create_dir_all(settings_path.parent().unwrap()).map_err(|e| e.to_string())?;
        serde_json::json!({})
    };

    let escaped = shell_escape(hook_script);
    let hook_cmd = format!("[ -x {} ] && {} \"$@\" || true", escaped, escaped);

    let hook_entry = serde_json::json!([{
        "hooks": [{
            "type": "command",
            "command": hook_cmd
        }]
    }]);

    // Events we want to hook into
    let events = ["UserPromptSubmit", "Stop", "PostToolUse", "PostToolUseFailure", "PermissionRequest"];

    let hooks = settings.as_object_mut()
        .ok_or("Invalid settings format")?
        .entry("hooks")
        .or_insert(serde_json::json!({}));

    let hooks_obj = hooks.as_object_mut().ok_or("Invalid hooks format")?;

    for event in &events {
        // Check if we already have a K2SO hook registered
        let existing = hooks_obj.get(*event);
        let already_registered = existing.map_or(false, |v| {
            v.to_string().contains(".k2so/hooks/notify.sh")
        });

        if !already_registered {
            // Merge: keep existing hooks, append ours
            if let Some(existing_arr) = existing.and_then(|v| v.as_array()) {
                let mut merged = existing_arr.clone();
                merged.push(hook_entry[0].clone());
                hooks_obj.insert(event.to_string(), serde_json::Value::Array(merged));
            } else {
                hooks_obj.insert(event.to_string(), hook_entry.clone());
            }
        }
    }

    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&settings_path, json).map_err(|e| e.to_string())?;

    log_debug!("[agent-hooks] Registered Claude Code hooks");
    Ok(())
}

/// Register hooks with Cursor (~/.cursor/hooks.json)
fn register_cursor_hooks(hook_script: &str) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("No home directory")?;
    let hooks_path = home.join(".cursor").join("hooks.json");

    let mut settings: serde_json::Value = if hooks_path.exists() {
        let raw = std::fs::read_to_string(&hooks_path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).unwrap_or(serde_json::json!({}))
    } else {
        std::fs::create_dir_all(hooks_path.parent().unwrap()).map_err(|e| e.to_string())?;
        serde_json::json!({})
    };

    let events = [
        ("beforeSubmitPrompt", "Start"),
        ("stop", "Stop"),
        ("beforeShellExecution", "PermissionRequest"),
        ("beforeMCPExecution", "PermissionRequest"),
    ];

    let hooks_obj = settings.as_object_mut().ok_or("Invalid hooks format")?;

    for (event, mapped_type) in &events {
        let escaped = shell_escape(hook_script);
        let hook_cmd = format!("[ -x {} ] && {} {} || true", escaped, escaped, mapped_type);

        let already_registered = hooks_obj.get(*event).map_or(false, |v| {
            v.to_string().contains(".k2so/hooks/notify.sh")
        });

        if !already_registered {
            let hook_entry = serde_json::json!({
                "command": hook_cmd
            });

            if let Some(existing_arr) = hooks_obj.get(*event).and_then(|v| v.as_array()) {
                let mut merged = existing_arr.clone();
                merged.push(hook_entry);
                hooks_obj.insert(event.to_string(), serde_json::Value::Array(merged));
            } else {
                hooks_obj.insert(event.to_string(), serde_json::json!([hook_entry]));
            }
        }
    }

    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&hooks_path, json).map_err(|e| e.to_string())?;

    log_debug!("[agent-hooks] Registered Cursor hooks");
    Ok(())
}

/// Register hooks with Gemini CLI (~/.gemini/settings.json)
fn register_gemini_hooks(hook_script: &str) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("No home directory")?;
    let settings_path = home.join(".gemini").join("settings.json");

    let mut settings: serde_json::Value = if settings_path.exists() {
        let raw = std::fs::read_to_string(&settings_path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).unwrap_or(serde_json::json!({}))
    } else {
        std::fs::create_dir_all(settings_path.parent().unwrap()).map_err(|e| e.to_string())?;
        serde_json::json!({})
    };

    let escaped = shell_escape(hook_script);
    let hook_cmd = format!("[ -x {} ] && {} \"$@\" || true", escaped, escaped);

    let hook_entry = serde_json::json!([{
        "hooks": [{
            "type": "command",
            "command": hook_cmd
        }]
    }]);

    let events = ["BeforeAgent", "AfterAgent", "AfterTool"];

    let hooks = settings.as_object_mut()
        .ok_or("Invalid settings format")?
        .entry("hooks")
        .or_insert(serde_json::json!({}));

    let hooks_obj = hooks.as_object_mut().ok_or("Invalid hooks format")?;

    for event in &events {
        let already_registered = hooks_obj.get(*event).map_or(false, |v| {
            v.to_string().contains(".k2so/hooks/notify.sh")
        });

        if !already_registered {
            if let Some(existing_arr) = hooks_obj.get(*event).and_then(|v| v.as_array()) {
                let mut merged = existing_arr.clone();
                merged.push(hook_entry[0].clone());
                hooks_obj.insert(event.to_string(), serde_json::Value::Array(merged));
            } else {
                hooks_obj.insert(event.to_string(), hook_entry.clone());
            }
        }
    }

    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&settings_path, json).map_err(|e| e.to_string())?;

    log_debug!("[agent-hooks] Registered Gemini CLI hooks");
    Ok(())
}

/// Check each supported CLI's config file for our notify.sh entries.
/// Used by `/cli/hooks/status` to let users verify injection end-to-end.
///
/// An entry is "injected" when the config file exists and contains at least
/// one command pointing at our notify.sh script. We don't validate the full
/// hook list — partial injection is still reported as injected=true so users
/// see events flow in `recent_events` while also noticing a mismatch.
#[allow(dead_code)]
pub fn check_hook_injections() -> serde_json::Value {
    let home = dirs::home_dir();
    let notify_fragment = ".k2so/hooks/notify.sh";

    let check = |relative: &str| -> serde_json::Value {
        let path = match &home {
            Some(h) => h.join(relative),
            None => return serde_json::json!({ "path": null, "exists": false, "injected": false }),
        };
        let path_str = path.to_string_lossy().to_string();
        if !path.exists() {
            return serde_json::json!({
                "path": path_str,
                "exists": false,
                "injected": false,
            });
        }
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let injected = content.contains(notify_fragment);
        serde_json::json!({
            "path": path_str,
            "exists": true,
            "injected": injected,
        })
    };

    let script_path = home
        .as_ref()
        .map(|h| h.join(notify_fragment))
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    serde_json::json!({
        "notify_script": {
            "path": script_path,
            "exists": home.as_ref().map(|h| h.join(notify_fragment).exists()).unwrap_or(false),
        },
        "claude": check(".claude/settings.json"),
        "cursor": check(".cursor/hooks.json"),
        "gemini": check(".config/gemini/hooks.json"),
    })
}

/// Register hooks with all supported agents. Called on app startup.
///
/// Per-CLI failures are collected and emitted as a `hook-injection-failed`
/// Tauri event so the frontend can surface a toast — previously these
/// failures only hit debug logs and users never knew their spinner was
/// broken because of a malformed `~/.claude/settings.json` or similar.
pub fn register_all_hooks(_app_handle: &AppHandle, hook_script: &str) {
    let mut failures: Vec<serde_json::Value> = Vec::new();

    if let Err(e) = register_claude_hooks(hook_script) {
        log_debug!("[agent-hooks] Failed to register Claude hooks: {}", e);
        failures.push(serde_json::json!({ "cli": "claude", "error": e }));
    }
    if let Err(e) = register_cursor_hooks(hook_script) {
        log_debug!("[agent-hooks] Failed to register Cursor hooks: {}", e);
        failures.push(serde_json::json!({ "cli": "cursor", "error": e }));
    }
    if let Err(e) = register_gemini_hooks(hook_script) {
        log_debug!("[agent-hooks] Failed to register Gemini hooks: {}", e);
        failures.push(serde_json::json!({ "cli": "gemini", "error": e }));
    }

    if !failures.is_empty() {
        k2_core::agent_hooks::emit(
            k2_core::agent_hooks::HookEvent::HookInjectionFailed,
            serde_json::json!({ "failures": failures }),
        );
    }
}

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

    #[test]
    fn hook_script_reads_port_and_token_dynamically() {
        // The script must read port + token from disk at exec time so a
        // K2SO restart (new random port / rotated token) doesn't silently
        // break hooks in long-running LLM sessions.
        let script = generate_hook_script(12345);
        assert!(
            !script.contains("K2SO_PORT=\"12345\""),
            "port must not be baked in as a literal"
        );
        assert!(
            script.contains("K2SO_PORT_FILE=\"$HOME/.k2so/heartbeat.port\""),
            "script must read port from heartbeat.port file"
        );
        assert!(
            script.contains("K2SO_TOKEN_FILE=\"$HOME/.k2so/heartbeat.token\""),
            "script must prefer token from heartbeat.token file"
        );
        assert!(
            script.contains("K2SO_PORT=$(cat \"$K2SO_PORT_FILE\""),
            "script must cat the port file at exec time"
        );
    }

    #[test]
    fn check_hook_injections_returns_expected_shape() {
        let val = check_hook_injections();
        // Top-level keys present
        assert!(val.get("notify_script").is_some());
        assert!(val.get("claude").is_some());
        assert!(val.get("cursor").is_some());
        assert!(val.get("gemini").is_some());
        // Each CLI entry has path/exists/injected booleans
        for key in &["claude", "cursor", "gemini"] {
            let entry = val.get(*key).expect("cli entry");
            assert!(entry.get("path").is_some(), "{} missing path", key);
            assert!(entry.get("exists").is_some(), "{} missing exists", key);
            assert!(entry.get("injected").is_some(), "{} missing injected", key);
            assert!(
                entry.get("exists").unwrap().is_boolean(),
                "{}.exists must be bool",
                key
            );
            assert!(
                entry.get("injected").unwrap().is_boolean(),
                "{}.injected must be bool",
                key
            );
        }
    }
}
