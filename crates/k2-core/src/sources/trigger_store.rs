//! rusqlite repo for `source_triggers` and `source_trigger_fires`. Owner:
//! B2. C0 ships the fire-log row and the two calls the source routes (B1)
//! make: count a source's triggers, and delete them with the source.

use rusqlite::{params, Connection};
use serde::Serialize;

/// One `source_trigger_fires` row as `triggers/show` and `triggers/log`
/// return it (never note text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FireRow {
    pub id: String,
    pub at: i64,
    pub incident_id: String,
    pub item_key: Option<String>,
    /// `pending` | `delivered` | `inbox` | `undeliverable` | `suppressed` |
    /// `coalesced` | `test`.
    pub outcome: String,
    pub suppress_reason: Option<String>,
    pub delivered_at: Option<i64>,
    pub woke: Option<bool>,
    pub note_bytes: Option<i64>,
    /// A fixed code (MsgReason / inbox error kind).
    pub detail: Option<String>,
}

/// How many triggers a source has (any status).
pub fn count_for_source(conn: &Connection, source_id: &str) -> rusqlite::Result<usize> {
    conn.query_row("SELECT COUNT(*) FROM source_triggers WHERE source_id = ?1", params![source_id], |r| {
        r.get::<_, i64>(0)
    })
    .map(|n| n as usize)
}

/// Delete a source's triggers (removal; the fire log ages out on its own).
/// Returns how many went.
pub fn delete_for_source(conn: &Connection, source_id: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM source_triggers WHERE source_id = ?1", params![source_id])
}
