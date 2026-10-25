//! Which workspace's agent added a guest (security review, 0.46.4). The
//! owner's guests carry no stamp. An agent with the skin-manage switch may
//! act on a guest with no rooms, role or App yet only when its own
//! workspace's agent added it; an unstamped guest stays the owner's. Copy
//! of box logins onto an App leaves out guests another workspace's agent
//! added. The table is additive in skin.db (created at open, no numbered
//! step), and a guest's row goes with it.

use rusqlite::{params, Connection, OptionalExtension};

use super::{now_secs, with_conn};

/// skin.db DDL (run by the idempotent open block).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS principal_agent_creators (
            principal_id TEXT PRIMARY KEY REFERENCES principals(id) ON DELETE CASCADE,
            workspace_id TEXT NOT NULL,
            created_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db principal_agent_creators: {e}"))
}

/// Stamp `principal_id` as added by the agent of `workspace_id`.
pub fn stamp(principal_id: &str, workspace_id: &str) -> Result<(), String> {
    let (pid, ws) = (principal_id.trim().to_string(), workspace_id.trim().to_string());
    if pid.is_empty() || ws.is_empty() {
        return Err("a creator stamp needs a guest and a workspace".into());
    }
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO principal_agent_creators (principal_id, workspace_id, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(principal_id) DO UPDATE SET workspace_id = excluded.workspace_id",
            params![pid, ws, now_secs()],
        )
        .map_err(|e| format!("creator stamp: {e}"))?;
        Ok(())
    })
}

/// The workspace whose agent added `principal_id` (`None`: the owner's).
pub(super) fn workspace_of_conn(conn: &Connection, principal_id: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT workspace_id FROM principal_agent_creators WHERE principal_id = ?1",
        params![principal_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| format!("creator stamp lookup: {e}"))
}

/// [`workspace_of_conn`] on its own connection.
pub fn workspace_of(principal_id: &str) -> Result<Option<String>, String> {
    with_conn(|conn| workspace_of_conn(conn, principal_id))
}
