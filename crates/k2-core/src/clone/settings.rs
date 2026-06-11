//! Source workspace K2 settings capture.
//!
//! When an agent is cloned to a remote host the migrated workspace must
//! auto-register on the remote *fully configured* — same agent mode,
//! heartbeat toggle, name, color, worktree mode. Those live in the source
//! machine's `projects` DB row. This module reads the USER-meaningful
//! subset of that row, deliberately EXCLUDING machine-specific fields
//! (local `id`, absolute `path`, `focus_group_id`) which must not travel:
//! the remote mints a fresh id, owns its own path, and has its own
//! focus-group layout.
//!
//! Capture is OPTIONAL/graceful: cloning a folder that isn't yet a
//! registered project yields `None`, and the bundle is still built (the
//! remote registers it with defaults).

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// The portable, user-meaningful K2 settings carried in the clone
/// manifest. All machine-specific identity is intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSettings {
    /// Agent mode: "off" / "agent" / "manager" / "coordinator" / "pod".
    pub agent_mode: String,
    /// Whether the agent is enabled (kept for backward-compat; the daemon
    /// keeps it in sync with `agent_mode != "off"`).
    pub agent_enabled: bool,
    /// Whether the heartbeat scheduler is on.
    pub heartbeat_enabled: bool,
    /// Display name of the workspace.
    pub name: String,
    /// Tab/nav color (hex).
    pub color: String,
    /// Worktree mode (i64 enum mirrored from the projects row).
    pub worktree_mode: i64,
}

/// Read the source project's USER-meaningful settings from the `projects`
/// table, matching on the absolute `path`. Returns `Ok(None)` when no row
/// matches (the path isn't a registered project) — capture is graceful so
/// the bundle still builds. Returns `Err` only on a genuine DB error.
///
/// The lookup matches both the exact path and a trailing-slash-normalized
/// variant so a caller's canonicalized path still finds a row stored
/// without the trailing separator (mirrors `projects_add_from_path`'s
/// own path-normalization).
pub fn capture_settings(
    conn: &Connection,
    project_path: &str,
) -> Result<Option<WorkspaceSettings>, String> {
    let trimmed = project_path.trim_end_matches('/');
    let with_sep = format!("{trimmed}/");

    // d410883: `heartbeat_enabled` is a LIVE aggregate ("does this
    // workspace have ≥1 enabled, non-archived heartbeat?"), NOT the
    // legacy `projects.heartbeat_enabled` column — that column is stale
    // by design now. Reading the column here used to bundle a stale
    // flag that disagreed with what every Project read reports.
    let mut stmt = conn
        .prepare(
            "SELECT agent_mode, agent_enabled, \
             (EXISTS(SELECT 1 FROM workspace_heartbeats wh \
                     WHERE wh.project_id = projects.id \
                     AND wh.enabled = 1 AND wh.archived_at IS NULL)) \
                 AS heartbeat_enabled, \
             name, color, worktree_mode \
             FROM projects \
             WHERE path = ?1 OR path = ?2 OR path = ?3 \
             LIMIT 1",
        )
        .map_err(|e| format!("prepare settings query: {e}"))?;

    let row = stmt.query_row(
        rusqlite::params![project_path, trimmed, with_sep],
        |row| {
            Ok(WorkspaceSettings {
                agent_mode: row
                    .get::<_, String>(0)
                    .unwrap_or_else(|_| "off".to_string()),
                agent_enabled: row.get::<_, i64>(1).unwrap_or(0) != 0,
                heartbeat_enabled: row.get::<_, i64>(2).unwrap_or(0) != 0,
                name: row.get::<_, String>(3)?,
                color: row.get::<_, String>(4).unwrap_or_else(|_| "#3b82f6".to_string()),
                worktree_mode: row.get::<_, i64>(5).unwrap_or(0),
            })
        },
    );

    match row {
        Ok(s) => Ok(Some(s)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(format!("read project settings row: {e}")),
    }
}
