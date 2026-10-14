//! rusqlite repo for the 0163 source tables (B1). C0 ships [`SourceRow`]
//! (one `sources` row), the row readers and the change counter every
//! writer bumps, so B2 can code against them on day 1.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::de::DeserializeOwned;

use super::spec::{AgentAccess, Keep, OnTargetError, PauseReason, SimulateState, SourceKind, SourceStatus, Viewers};

/// One `sources` row, typed.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceRow {
    pub id: String,
    pub name: String,
    pub label: String,
    pub kind: SourceKind,
    pub preset: Option<String>,
    /// `{fetch, headers, extract, rules}`, canonical JSON.
    pub spec_json: String,
    /// sha256 hex of the spec (`SourceSpec::hash`).
    pub spec_hash: String,
    /// Sorted unique `scheme://host:port`.
    pub origins: Vec<String>,
    /// An exact IP or `http://` target; Owner only.
    pub lan: bool,
    pub secret_name: Option<String>,
    pub url_secret_name: Option<String>,
    /// A vault grant exists now.
    pub secret_bound: bool,
    pub every_secs: u32,
    pub timeout_secs: u32,
    pub max_bytes: u32,
    pub fail_after: u8,
    pub on_target_error: OnTargetError,
    pub link: Option<String>,
    pub viewers: Viewers,
    pub agent_access: AgentAccess,
    pub keep: Keep,
    pub pinned: bool,
    pub pinned_by: Option<String>,
    pub pinned_at: Option<i64>,
    pub status: SourceStatus,
    pub pause_reason: Option<PauseReason>,
    /// `owner` | `login:<user>` | `agent:<projectId>`.
    pub made_by: String,
    pub made_by_project: Option<String>,
    pub approved_by: Option<String>,
    pub approved_at: Option<i64>,
    pub approved_spec_hash: Option<String>,
    pub simulate_state: Option<SimulateState>,
    pub simulate_until: Option<i64>,
    pub simulate_by: Option<String>,
    pub mute_until: Option<i64>,
    pub mute_by: Option<String>,
    pub mute_reason: Option<String>,
    pub rev: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub removed_at: Option<i64>,
}

const COLUMNS: &str = "id, name, label, kind, preset, spec_json, spec_hash, origins_json, lan, secret_name, \
     url_secret_name, secret_bound, every_secs, timeout_secs, max_bytes, fail_after, on_target_error, link, \
     viewers, agent_access, keep, pinned, pinned_by, pinned_at, status, pause_reason, made_by, made_by_project, \
     approved_by, approved_at, approved_spec_hash, simulate_state, simulate_until, simulate_by, mute_until, \
     mute_by, mute_reason, rev, created_at, updated_at, removed_at";

fn conv_err(idx: usize, msg: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, msg.into())
}

/// A stored enum code (`'active'`, `'http-json'`, …) as its serde type.
fn code<T: DeserializeOwned>(r: &Row, idx: usize) -> rusqlite::Result<T> {
    let s: String = r.get(idx)?;
    serde_json::from_value(serde_json::Value::String(s.clone())).map_err(|_| conv_err(idx, format!("unknown code {s:?}")))
}

fn opt_code<T: DeserializeOwned>(r: &Row, idx: usize) -> rusqlite::Result<Option<T>> {
    let s: Option<String> = r.get(idx)?;
    match s {
        None => Ok(None),
        Some(s) => serde_json::from_value(serde_json::Value::String(s.clone()))
            .map(Some)
            .map_err(|_| conv_err(idx, format!("unknown code {s:?}"))),
    }
}

fn row(r: &Row) -> rusqlite::Result<SourceRow> {
    let origins_json: String = r.get(7)?;
    let origins: Vec<String> =
        serde_json::from_str(&origins_json).map_err(|e| conv_err(7, format!("origins_json: {e}")))?;
    Ok(SourceRow {
        id: r.get(0)?,
        name: r.get(1)?,
        label: r.get(2)?,
        kind: code(r, 3)?,
        preset: r.get(4)?,
        spec_json: r.get(5)?,
        spec_hash: r.get(6)?,
        origins,
        lan: r.get::<_, i64>(8)? != 0,
        secret_name: r.get(9)?,
        url_secret_name: r.get(10)?,
        secret_bound: r.get::<_, i64>(11)? != 0,
        every_secs: r.get(12)?,
        timeout_secs: r.get(13)?,
        max_bytes: r.get(14)?,
        fail_after: r.get(15)?,
        on_target_error: code(r, 16)?,
        link: r.get(17)?,
        viewers: code(r, 18)?,
        agent_access: code(r, 19)?,
        keep: code(r, 20)?,
        pinned: r.get::<_, i64>(21)? != 0,
        pinned_by: r.get(22)?,
        pinned_at: r.get(23)?,
        status: code(r, 24)?,
        pause_reason: opt_code(r, 25)?,
        made_by: r.get(26)?,
        made_by_project: r.get(27)?,
        approved_by: r.get(28)?,
        approved_at: r.get(29)?,
        approved_spec_hash: r.get(30)?,
        simulate_state: opt_code(r, 31)?,
        simulate_until: r.get(32)?,
        simulate_by: r.get(33)?,
        mute_until: r.get(34)?,
        mute_by: r.get(35)?,
        mute_reason: r.get(36)?,
        rev: r.get(37)?,
        created_at: r.get(38)?,
        updated_at: r.get(39)?,
        removed_at: r.get(40)?,
    })
}

/// The live (not removed) source with this name.
pub fn get_by_name(conn: &Connection, name: &str) -> rusqlite::Result<Option<SourceRow>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM sources WHERE name = ?1 AND status != 'removed'"),
        params![name],
        row,
    )
    .optional()
}

/// Any source row by id, tombstones included.
pub fn get_by_id(conn: &Connection, id: &str) -> rusqlite::Result<Option<SourceRow>> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM sources WHERE id = ?1"), params![id], row).optional()
}

/// Every live source, by name.
pub fn list_live(conn: &Connection) -> rusqlite::Result<Vec<SourceRow>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM sources WHERE status != 'removed' ORDER BY name"))?;
    let rows = stmt.query_map([], row)?;
    rows.collect()
}

/// Bump the one global change counter and return the new value. Call it
/// in the same transaction as the change; the daemon then emits
/// `sources_changed {rev}`.
pub fn bump_rev(conn: &Connection) -> rusqlite::Result<i64> {
    let n = conn.execute("UPDATE source_rev SET rev = rev + 1 WHERE id = 1", [])?;
    if n == 0 {
        conn.execute("INSERT INTO source_rev (id, rev) VALUES (1, 1)", [])?;
    }
    conn.query_row("SELECT rev FROM source_rev WHERE id = 1", [], |r| r.get(0))
}

/// The current change counter.
pub fn current_rev(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT rev FROM source_rev WHERE id = 1", [], |r| r.get(0)).optional().map(|v| v.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn rows_read_back_typed_and_rev_counts_up() {
        let conn = db();
        assert_eq!(current_rev(&conn).unwrap(), 0);
        conn.execute(
            "INSERT INTO sources (id, name, label, spec_json, spec_hash, origins_json, every_secs, viewers, \
             agent_access, keep, status, pause_reason, made_by, made_by_project, simulate_state, simulate_until, \
             rev, created_at, updated_at) VALUES ('s1', 'fleet', 'Fleet', '{}', ?1, \
             '[\"https://acme.k2.dev:443\"]', 60, 'admin', 'shape', 'states', 'paused', 'paused_by_person', \
             'agent:p1', 'p1', 'crit', 1760000300, 1, 1760000000, 1760000000)",
            params!["b".repeat(64)],
        )
        .unwrap();
        let r = get_by_name(&conn, "fleet").unwrap().expect("live row");
        assert_eq!(r.kind, SourceKind::HttpJson);
        assert_eq!(r.origins, vec!["https://acme.k2.dev:443".to_string()]);
        assert_eq!((r.viewers, r.agent_access, r.keep), (Viewers::Admin, AgentAccess::Shape, Keep::States));
        assert_eq!((r.status, r.pause_reason), (SourceStatus::Paused, Some(PauseReason::PausedByPerson)));
        assert_eq!(r.simulate_state, Some(SimulateState::Crit));
        assert_eq!(r.made_by_project.as_deref(), Some("p1"));
        assert_eq!(get_by_id(&conn, "s1").unwrap().map(|r| r.name), Some("fleet".into()));
        assert_eq!(list_live(&conn).unwrap().len(), 1);
        assert_eq!(bump_rev(&conn).unwrap(), 1);
        assert_eq!(bump_rev(&conn).unwrap(), 2);
        assert_eq!(current_rev(&conn).unwrap(), 2);
    }
}
