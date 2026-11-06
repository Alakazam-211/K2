//! k2-node rollouts, the store (migration 0270;
//! prd-compute-update-continuity-v1 §5.10, rev 3). The controller pushes
//! its own k2-node build to every node it owns and records each node's
//! place, so a rollout survives the controller's own restarts. The logic
//! (who is eligible, sending, limits) is the daemon's `compute_rollout`.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// Rollout states.
pub const RUNNING: &str = "running";
pub const PAUSED: &str = "paused";
pub const STOPPED: &str = "stopped";
pub const DONE: &str = "done";

/// Node states in a rollout.
pub const N_PENDING: &str = "pending";
pub const N_OFFERED: &str = "offered";
pub const N_HEALTHY: &str = "healthy";
pub const N_ROLLED_BACK: &str = "rolled_back";
pub const N_REFUSED: &str = "refused";
pub const N_FAILED: &str = "failed";
pub const N_TIMED_OUT: &str = "timed_out";
pub const N_SKIPPED: &str = "skipped";

/// A node's place is settled (nothing more happens to it in this rollout).
pub fn node_done(state: &str) -> bool {
    !matches!(state, N_PENDING | N_OFFERED)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rollout {
    pub id: String,
    pub to_version: String,
    pub trigger: String,
    pub state: String,
    pub batch_size: i64,
    pub now: bool,
    pub by: String,
    pub started_at: i64,
    pub updated_at: i64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RolloutNode {
    pub rollout_id: String,
    pub node_id: String,
    pub state: String,
    pub code: Option<String>,
    pub detail: Option<String>,
    pub at: i64,
    pub offered_at: Option<i64>,
}

fn rollout_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<Rollout> {
    Ok(Rollout {
        id: r.get(0)?,
        to_version: r.get(1)?,
        trigger: r.get(2)?,
        state: r.get(3)?,
        batch_size: r.get(4)?,
        now: r.get::<_, i64>(5)? != 0,
        by: r.get(6)?,
        started_at: r.get(7)?,
        updated_at: r.get(8)?,
        note: r.get(9)?,
    })
}

const R_COLS: &str = "id, to_version, trigger, state, batch_size, now_flag, by, started_at, updated_at, note";

fn node_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<RolloutNode> {
    Ok(RolloutNode { rollout_id: r.get(0)?, node_id: r.get(1)?, state: r.get(2)?, code: r.get(3)?, detail: r.get(4)?, at: r.get(5)?, offered_at: r.get(6)? })
}

const N_COLS: &str = "rollout_id, node_id, state, code, detail, at, offered_at";

/// Start a rollout with its nodes' first states (`pending` or `skipped`).
#[allow(clippy::too_many_arguments)]
pub fn create(
    conn: &Connection,
    id: &str,
    to_version: &str,
    trigger: &str,
    batch_size: i64,
    now_flag: bool,
    by: &str,
    at: i64,
    nodes: &[(String, &str, Option<String>, Option<String>)],
) -> Result<Rollout, String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO compute_node_rollouts (id, to_version, trigger, state, batch_size, now_flag, by, started_at, updated_at) VALUES (?1, ?2, ?3, 'running', ?4, ?5, ?6, ?7, ?7)",
        params![id, to_version, trigger, batch_size, now_flag as i64, by, at],
    )
    .map_err(|e| e.to_string())?;
    for (node, state, code, detail) in nodes {
        tx.execute(
            "INSERT INTO compute_node_rollout_nodes (rollout_id, node_id, state, code, detail, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, node, state, code, detail, at],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    get(conn, id).ok_or_else(|| "the rollout vanished".into())
}

pub fn get(conn: &Connection, id: &str) -> Option<Rollout> {
    conn.query_row(&format!("SELECT {R_COLS} FROM compute_node_rollouts WHERE id = ?1"), params![id], rollout_of).optional().ok().flatten()
}

/// The newest rollout.
pub fn latest(conn: &Connection) -> Option<Rollout> {
    conn.query_row(&format!("SELECT {R_COLS} FROM compute_node_rollouts ORDER BY started_at DESC, rowid DESC LIMIT 1"), [], rollout_of).optional().ok().flatten()
}

/// The newest rollout to `version` (any state).
pub fn latest_to(conn: &Connection, version: &str) -> Option<Rollout> {
    conn.query_row(
        &format!("SELECT {R_COLS} FROM compute_node_rollouts WHERE to_version = ?1 ORDER BY started_at DESC, rowid DESC LIMIT 1"),
        params![version],
        rollout_of,
    )
    .optional()
    .ok()
    .flatten()
}

/// Rollouts still moving (running) or waiting for a person (paused).
pub fn open(conn: &Connection) -> Vec<Rollout> {
    let Ok(mut st) = conn.prepare(&format!("SELECT {R_COLS} FROM compute_node_rollouts WHERE state IN ('running','paused') ORDER BY started_at")) else {
        return Vec::new();
    };
    st.query_map([], rollout_of).map(|r| r.filter_map(Result::ok).collect()).unwrap_or_default()
}

pub fn nodes(conn: &Connection, id: &str) -> Vec<RolloutNode> {
    let Ok(mut st) = conn.prepare(&format!("SELECT {N_COLS} FROM compute_node_rollout_nodes WHERE rollout_id = ?1 ORDER BY node_id")) else {
        return Vec::new();
    };
    st.query_map(params![id], node_of).map(|r| r.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// A node's place in the newest open rollout that has it.
pub fn node_in_open(conn: &Connection, node_id: &str) -> Option<(Rollout, RolloutNode)> {
    let (rid,): (String,) = conn
        .query_row(
            "SELECT n.rollout_id FROM compute_node_rollout_nodes n JOIN compute_node_rollouts r ON r.id = n.rollout_id \
             WHERE n.node_id = ?1 AND r.state IN ('running','paused') ORDER BY r.started_at DESC LIMIT 1",
            params![node_id],
            |r| Ok((r.get(0)?,)),
        )
        .optional()
        .ok()
        .flatten()?;
    let n = conn
        .query_row(&format!("SELECT {N_COLS} FROM compute_node_rollout_nodes WHERE rollout_id = ?1 AND node_id = ?2"), params![rid, node_id], node_of)
        .optional()
        .ok()
        .flatten()?;
    Some((get(conn, &rid)?, n))
}

/// A node's last place in any rollout (newest first): the status line.
pub fn node_latest(conn: &Connection, node_id: &str) -> Option<(Rollout, RolloutNode)> {
    let rid: String = conn
        .query_row(
            "SELECT n.rollout_id FROM compute_node_rollout_nodes n JOIN compute_node_rollouts r ON r.id = n.rollout_id \
             WHERE n.node_id = ?1 ORDER BY r.started_at DESC, r.rowid DESC LIMIT 1",
            params![node_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()?;
    let n = conn
        .query_row(&format!("SELECT {N_COLS} FROM compute_node_rollout_nodes WHERE rollout_id = ?1 AND node_id = ?2"), params![rid, node_id], node_of)
        .optional()
        .ok()
        .flatten()?;
    Some((get(conn, &rid)?, n))
}

/// Move a node. A settled node never moves again (a late `healthy` after
/// a timeout still lands: `timed_out` may become what the node said).
pub fn set_node(conn: &Connection, id: &str, node_id: &str, state: &str, code: Option<&str>, detail: Option<&str>, at: i64) -> bool {
    let offered = (state == N_OFFERED).then_some(at);
    conn.execute(
        "UPDATE compute_node_rollout_nodes SET state = ?3, code = ?4, detail = ?5, at = ?6, offered_at = COALESCE(?7, offered_at) \
         WHERE rollout_id = ?1 AND node_id = ?2 AND state IN ('pending','offered','timed_out')",
        params![id, node_id, state, code, detail, at, offered],
    )
    .map(|n| n == 1)
    .unwrap_or(false)
}

pub fn set_state(conn: &Connection, id: &str, state: &str, note: Option<&str>, at: i64) -> bool {
    conn.execute(
        "UPDATE compute_node_rollouts SET state = ?2, note = COALESCE(?3, note), updated_at = ?4 WHERE id = ?1",
        params![id, state, note, at],
    )
    .map(|n| n == 1)
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&c).unwrap();
        c
    }

    #[test]
    fn a_rollout_keeps_each_nodes_place_and_settled_nodes_never_move_back() {
        let c = conn();
        let r = create(
            &c,
            "r1",
            "0.46.9",
            "explicit",
            0,
            false,
            "owner",
            100,
            &[("n1".into(), N_PENDING, None, None), ("n2".into(), N_SKIPPED, Some("node_needs_manual_install".into()), Some("0.46.3".into()))],
        )
        .unwrap();
        assert_eq!((r.state.as_str(), r.batch_size, r.now), (RUNNING, 0, false));
        assert_eq!(open(&c).len(), 1);
        assert!(set_node(&c, "r1", "n1", N_OFFERED, None, None, 110));
        let (ro, n) = node_in_open(&c, "n1").unwrap();
        assert_eq!((ro.id.as_str(), n.state.as_str(), n.offered_at), ("r1", N_OFFERED, Some(110)));
        assert!(set_node(&c, "r1", "n1", N_TIMED_OUT, None, None, 800));
        assert!(set_node(&c, "r1", "n1", N_HEALTHY, None, None, 900), "a late healthy after the limit still lands");
        assert!(!set_node(&c, "r1", "n1", N_OFFERED, None, None, 901), "settled: never offered again");
        assert!(!set_node(&c, "r1", "n2", N_OFFERED, None, None, 901), "a skipped node stays skipped");
        let ns = nodes(&c, "r1");
        assert_eq!(ns.iter().map(|n| (n.node_id.as_str(), n.state.as_str())).collect::<Vec<_>>(), vec![("n1", N_HEALTHY), ("n2", N_SKIPPED)]);
        assert!(set_state(&c, "r1", DONE, Some("all settled"), 950));
        assert!(open(&c).is_empty());
        assert_eq!(latest_to(&c, "0.46.9").unwrap().note.as_deref(), Some("all settled"));
        assert_eq!(node_latest(&c, "n2").unwrap().1.code.as_deref(), Some("node_needs_manual_install"));
        assert!(node_in_open(&c, "n1").is_none(), "a done rollout holds no node");
    }
}
