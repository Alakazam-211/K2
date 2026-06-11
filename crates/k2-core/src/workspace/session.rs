//! Agent session lifecycle: lock / unlock + session-id persistence.
//!
//! Small DB-backed helpers the host (daemon OR src-tauri) calls around
//! a PTY spawn to make it visible to the scheduler:
//!
//! - [`k2so_agents_lock`] — record that an agent has an active session.
//!   Writes both a DB row (`agent_sessions`) and a legacy `.lock` file.
//!   Callers: forced-fire + heartbeat-fired + interactive launches.
//!   The scheduler reads this via
//!   [`crate::workspace::scheduler::is_agent_locked`]
//!   to skip agents that are already running.
//! - [`k2so_agents_unlock`] — mark the session `sleeping` + remove the
//!   `.lock` file. Called at session-end by the TUI exit listener (and
//!   by the scheduler when it observes a stop event).
//! - [`k2so_agents_save_session_id`] — persist the CLI session ID
//!   (Claude Code's `--resume <id>` target) so the next wake rejoins
//!   the same chat instead of spawning fresh.
//! - [`k2so_agents_clear_session_id`] — null out the saved ID after a
//!   no-op (next wake will be a fresh session).

use std::fs;

use crate::workspace::agent_identity::{agent_dir, resolve_project_id};
use crate::workspace::scheduler::agent_work_dir;
use crate::db::schema::WorkspaceSession;

/// Best-effort upsert of an `agent_sessions` row + create the legacy
/// `.lock` file. The row is the source of truth; the file is for
/// pre-migration workspaces whose .k2so layout predates the DB.
pub fn k2so_agents_lock(
    project_path: String,
    agent_name: String,
    terminal_id: Option<String>,
    owner: Option<String>,
) -> Result<(), String> {
    {
        let db = crate::db::shared();
        let conn = db.lock();
        if let Some(project_id) = resolve_project_id(&conn, &project_path) {
            let session_uuid = uuid::Uuid::new_v4().to_string();
            let owner_val = owner.as_deref().unwrap_or("system");
            let _ = WorkspaceSession::upsert(
                &conn,
                &session_uuid,
                &project_id,
                terminal_id.as_deref(),
                None,
                "claude",
                owner_val,
                "running",
            );
        }
    }

    let lock_path = agent_work_dir(&project_path, &agent_name, "").join(".lock");
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(&lock_path, simple_date()).map_err(|e| e.to_string())
}

/// Flip the session to `sleeping` + remove the `.lock` file. Silent
/// no-op if neither the DB row nor the file is present (common on
/// fresh workspaces).
pub fn k2so_agents_unlock(project_path: String, agent_name: String) -> Result<(), String> {
    {
        let db = crate::db::shared();
        let conn = db.lock();
        if let Some(project_id) = resolve_project_id(&conn, &project_path) {
            let _ = WorkspaceSession::update_status(&conn, &project_id, "sleeping");
        }
    }

    let lock_path = agent_work_dir(&project_path, &agent_name, "").join(".lock");
    if lock_path.exists() {
        fs::remove_file(&lock_path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Persist the CLI session ID on the `agent_sessions` row so the next
/// wake can `--resume` into the same chat. Errors if the agent
/// directory doesn't exist (bogus name / already archived).
pub fn k2so_agents_save_session_id(
    project_path: String,
    agent_name: String,
    session_id: String,
) -> Result<(), String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }

    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    WorkspaceSession::update_session_id(&conn, &project_id, &session_id)
        .map(|_| ())
        .map_err(|e| format!("Failed to save session ID: {}", e))
}

/// Null out the saved session ID. Called on no-op so the next wake is
/// a fresh session — no point resuming a session that was just "I have
/// nothing to do."
pub fn k2so_agents_clear_session_id(
    project_path: String,
    agent_name: String,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    if let Some(project_id) = resolve_project_id(&conn, &project_path) {
        let _ = WorkspaceSession::clear_session_id(&conn, &project_id);
    }
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════
// Surfaced flag + chat refresh broadcast — Phase 2 Unit 7d
// ══════════════════════════════════════════════════════════════════════
//
// Moved from `src-tauri/src/commands/k2so_agents.rs`. These functions
// flip per-workspace-session state and broadcast a hook event so every
// renderer window (or future remote companion) reacts in sync. Bodies
// are pure data + agent_hooks::emit calls — no Tauri AppHandle needed.

/// Toggle the per-session `surfaced` flag. When transitioning 0 → 1,
/// emits `HookEvent::SessionSurfaced` so the renderer creates a tab
/// that ATTACHES to the existing PTY (no fresh spawn). When
/// transitioning 1 → 0, the renderer is expected to remove the tab
/// without killing the PTY (the heartbeat session keeps running in
/// the background). See `.k2so/prds/heartbeat-active-session-tracking.md`.
///
/// `terminal_id`, `command`, `args`, `heartbeat_name` are forwarded
/// in the surfaced-event payload so the renderer can construct a tab
/// without re-querying — kept minimal because the event listener is
/// a hot path. Pass empty strings / empty Vec / None when not
/// applicable.
#[allow(clippy::too_many_arguments)]
pub fn k2so_session_set_surfaced(
    project_path: String,
    agent_name: String,
    surfaced: bool,
    terminal_id: Option<String>,
    command: Option<String>,
    args: Option<Vec<String>>,
    heartbeat_name: Option<String>,
    attach_agent_name: Option<String>,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    WorkspaceSession::set_surfaced(&conn, &project_id, surfaced)
        .map_err(|e| format!("set surfaced flag: {}", e))?;
    drop(conn);

    if surfaced {
        // Emit on every `surfaced=true` call (not just 0→1 transitions)
        // so the user can re-summon a tab even when the DB flag was
        // left as `1` by a prior surface that the renderer subsequently
        // dropped (e.g. close-minimize that skipped the surfaced=false
        // flip). The renderer's listener already checks whether a tab
        // exists before creating one, so re-emit is idempotent.
        crate::agent_hooks::emit(
            crate::agent_hooks::HookEvent::SessionSurfaced,
            serde_json::json!({
                "projectPath": project_path,
                "agentName": agent_name,
                "terminalId": terminal_id,
                "command": command,
                "args": args,
                "heartbeatName": heartbeat_name,
                "attachAgentName": attach_agent_name,
            }),
        );
    } else {
        // 0.38.0 commit 6 — symmetric counterpart so every viewer
        // drops the tab from its UI when one window minimizes. The
        // PTY stays alive in the daemon (close-as-minimize); only the
        // surface state propagates. Renderer's `session:unsurfaced`
        // listener is idempotent (no-op if the tab isn't present).
        crate::agent_hooks::emit(
            crate::agent_hooks::HookEvent::SessionUnsurfaced,
            serde_json::json!({
                "projectPath": project_path,
                "agentName": agent_name,
                "terminalId": terminal_id,
                "heartbeatName": heartbeat_name,
                "attachAgentName": attach_agent_name,
            }),
        );
    }
    Ok(())
}

/// 0.38.0 commit 7 — broadcast cross-window when the pinned-chat
/// refresh button is clicked. The originating window already kills
/// the daemon PTY (via `/cli/sessions/v2/close`) and bumps its
/// `refreshNonce` to remount TerminalPane. Other windows can't learn
/// this from Commit 4's `session_removed` push (those filter for
/// `tab-` agent names; pinned chat is the bare project_id), so we
/// emit a Tauri-broadcast `chat:refreshed` event. Every window's
/// AgentChatPane listener bumps its own `refreshNonce` when the
/// payload's `projectPath` matches its workspace, keeping pinned
/// chat sessions in sync across viewers.
pub fn k2so_chat_refresh_broadcast(project_path: String) -> Result<(), String> {
    crate::agent_hooks::emit(
        crate::agent_hooks::HookEvent::ChatRefreshed,
        serde_json::json!({ "projectPath": project_path }),
    );
    Ok(())
}

// ── Date helpers used only by the lockfile body ────────────────────────
//
// `simple_date` writes an ISO-8601 day stamp into the legacy .lock file
// so you can cat it and know roughly when the session started. Uses
// std::time rather than chrono so pre-migration tooling that reads the
// file doesn't have to match a Rust formatting crate.

pub fn simple_date() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = now / 86400;
    let mut y = 1970i64;
    let mut remaining = days as i64;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let months = [
        31,
        if is_leap(y) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 1;
    for &dim in &months {
        if remaining < dim {
            break;
        }
        remaining -= dim;
        m += 1;
    }
    format!("{:04}-{:02}-{:02}", y, m, remaining + 1)
}

pub(crate) fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_leap_handles_century_edge_cases() {
        assert!(is_leap(2000)); // divisible by 400
        assert!(!is_leap(1900)); // divisible by 100, not 400
        assert!(is_leap(2024));
        assert!(!is_leap(2023));
    }

    #[test]
    fn simple_date_shape_is_iso_like() {
        let s = simple_date();
        assert_eq!(s.len(), 10, "expected YYYY-MM-DD, got {s}");
        assert_eq!(s.chars().nth(4), Some('-'));
        assert_eq!(s.chars().nth(7), Some('-'));
    }
}
