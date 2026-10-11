//! The grant automation behind the two compute toggles (Rosson
//! 2026-10-11): the Agent tab's "Allow compute nodes" (Settings →
//! Workspaces → <agent> → Agent) and Settings → Compute "Machines you can
//! use".
//!
//! - An agent's toggle ON = the workspace switch (`agents_can_use_compute`)
//!   plus a grant on **every** active node, with default limits.
//! - A node that becomes active is granted to every workspace whose switch
//!   is on ([`grant_new_node`]).
//! - An agent's toggle OFF = switch off and every grant of that workspace
//!   removed.
//! - A machine's toggle OFF = retire: queued jobs are cancelled, running
//!   ones finish (the node is `draining` meanwhile), then the node is
//!   removed. The retiring set is a small file in `~/.k2/compute/` (no
//!   migration); the daemon's sweep finishes the removal.
//!
//! Pure SQLite + file parts only; the routes and the sweep live in
//! `k2-daemon` (`compute_routes`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::store::{self, GrantLimits, GrantRow, NodeRow};

/// Default disk budget when the node hasn't reported its free space.
pub const FALLBACK_DISK_GB: i64 = super::DEFAULT_MAX_DISK_GB;

/// Default per-job disk for a node: the smaller of 25 % of its free disk
/// and 60 GB (at least 1 GB); 60 GB when the node hasn't said.
pub fn default_disk_gb(disk_free_bytes: Option<u64>) -> i64 {
    match disk_free_bytes {
        Some(free) => {
            let quarter_gb = (free / 4 / (1u64 << 30)) as i64;
            quarter_gb.clamp(1, FALLBACK_DISK_GB)
        }
        None => FALLBACK_DISK_GB,
    }
}

/// The node's last reported free disk (offer), if any.
pub fn node_disk_free(n: &NodeRow) -> Option<u64> {
    n.last_offer.as_ref().map(|o| o.free.disk_free_bytes).filter(|b| *b > 0)
}

/// Fill every unset limit with the default for this node.
pub fn limits_for(n: &NodeRow, asked: &GrantLimits) -> GrantLimits {
    GrantLimits {
        max_job_secs: Some(asked.max_job_secs.unwrap_or(super::DEFAULT_MAX_JOB_SECS)),
        max_disk_gb: Some(asked.max_disk_gb.unwrap_or_else(|| default_disk_gb(node_disk_free(n)))),
        max_parallel: Some(asked.max_parallel.unwrap_or(super::DEFAULT_MAX_PARALLEL)),
        max_queued: Some(asked.max_queued.unwrap_or(super::DEFAULT_MAX_QUEUED)),
    }
}

fn any_set(l: &GrantLimits) -> bool {
    l.max_job_secs.is_some() || l.max_disk_gb.is_some() || l.max_parallel.is_some() || l.max_queued.is_some()
}

/// Active nodes (the ones a grant can name).
pub fn active_nodes(conn: &Connection) -> Vec<NodeRow> {
    store::list_nodes(conn, false).into_iter().filter(|n| n.state == "active").collect()
}

/// Workspaces (project ids) whose "Allow compute nodes" switch is on.
pub fn workspaces_switched_on(conn: &Connection) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare("SELECT id FROM projects WHERE agents_can_use_compute = 1 ORDER BY id") else {
        return Vec::new();
    };
    stmt.query_map([], |r| r.get::<_, String>(0))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// What turning an agent on did.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnableOutcome {
    /// Node ids granted (new or updated) by this call.
    pub granted: Vec<String>,
    /// Node ids whose existing grant was kept as is.
    pub kept: Vec<String>,
}

/// Agent toggle ON: switch on, then a grant on every active node. With no
/// limits asked, an existing grant is kept (its limits are the owner's);
/// with limits, every grant of the workspace gets them (unset ones take
/// the per-node default).
pub fn enable_workspace(
    conn: &Connection,
    workspace_id: &str,
    asked: &GrantLimits,
    by: &str,
    at: i64,
) -> Result<EnableOutcome, String> {
    store::set_agents_can_use_compute(conn, workspace_id, true)?;
    let mut out = EnableOutcome::default();
    for n in active_nodes(conn) {
        if !any_set(asked) && store::grant(conn, &n.id, workspace_id).is_some() {
            out.kept.push(n.id);
            continue;
        }
        store::set_grant(conn, &n.id, workspace_id, &limits_for(&n, asked), by, at)?;
        out.granted.push(n.id);
    }
    Ok(out)
}

/// Agent toggle OFF: switch off and every grant of the workspace removed.
/// Returns how many grants went. Queued and running jobs keep going (the
/// same rule as a single revoke); new runs are refused.
pub fn disable_workspace(conn: &Connection, workspace_id: &str) -> Result<usize, String> {
    store::set_agents_can_use_compute(conn, workspace_id, false)?;
    conn.execute("DELETE FROM compute_node_grants WHERE workspace_id = ?1", params![workspace_id])
        .map_err(|e| e.to_string())
}

/// A node just became active: grant it to every workspace whose switch is
/// on (default limits; an existing grant is left alone). Returns the
/// workspace ids granted.
pub fn grant_new_node(conn: &Connection, node_id: &str, by: &str, at: i64) -> Result<Vec<String>, String> {
    let n = store::node_by_id(conn, node_id).ok_or_else(|| format!("no node {node_id}"))?;
    if n.state != "active" {
        return Ok(Vec::new());
    }
    let limits = limits_for(&n, &GrantLimits::default());
    let mut granted = Vec::new();
    for ws in workspaces_switched_on(conn) {
        if store::grant(conn, &n.id, &ws).is_some() {
            continue;
        }
        store::set_grant(conn, &n.id, &ws, &limits, by, at)?;
        granted.push(ws);
    }
    Ok(granted)
}

/// One workspace's row for the agents list.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRow {
    pub workspace_id: String,
    pub name: String,
    pub agents_can_use_compute: bool,
    pub grants: Vec<GrantRow>,
}

/// Every workspace with its switch and grants (owner/admin view).
pub fn agents(conn: &Connection) -> Vec<AgentRow> {
    let Ok(mut stmt) = conn.prepare("SELECT id, name, agents_can_use_compute FROM projects ORDER BY name COLLATE NOCASE, id") else {
        return Vec::new();
    };
    let rows: Vec<(String, String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get::<_, Option<i64>>(2)?.unwrap_or(0))))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default();
    rows.into_iter()
        .map(|(id, name, on)| AgentRow {
            grants: store::grants(conn, None, Some(&id)),
            workspace_id: id,
            name,
            agents_can_use_compute: on == 1,
        })
        .collect()
}

// ── retiring machines ────────────────────────────────────────────────

/// One node on its way out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retiring {
    pub by: String,
    pub at: i64,
}

/// `~/.k2/compute/retiring.json`.
pub fn retiring_path() -> PathBuf {
    super::compute_dir().join("retiring.json")
}

/// The retiring set (a missing or bad file is empty).
pub fn retiring_at(path: &Path) -> BTreeMap<String, Retiring> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_retiring_at(path: &Path, set: &BTreeMap<String, Retiring>) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(set).map_err(|e| e.to_string())?)
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("replace {}: {e}", path.display()))
}

/// Add a node to the retiring set.
pub fn mark_retiring_at(path: &Path, node_id: &str, by: &str, at: i64) -> Result<(), String> {
    let mut set = retiring_at(path);
    set.insert(node_id.to_string(), Retiring { by: by.to_string(), at });
    write_retiring_at(path, &set)
}

/// Drop a node from the retiring set (it was resumed or is gone). Returns
/// whether it was there.
pub fn unmark_retiring_at(path: &Path, node_id: &str) -> Result<bool, String> {
    let mut set = retiring_at(path);
    let was = set.remove(node_id).is_some();
    if was {
        write_retiring_at(path, &set)?;
    }
    Ok(was)
}

/// What the sweep should do with one retiring node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetireStep {
    /// Gone or already removed: forget it.
    Forget,
    /// Still has work: wait.
    Wait { active: usize },
    /// Idle: remove it now.
    Remove,
}

/// Decide the next step for a retiring node.
pub fn retire_step(conn: &Connection, node_id: &str) -> RetireStep {
    match store::node_by_id(conn, node_id) {
        None => RetireStep::Forget,
        Some(n) if n.state == "revoked" => RetireStep::Forget,
        Some(n) => {
            let active = store::jobs_in_states(conn, &n.id, store::ACTIVE_STATES).len()
                + store::jobs_in_states(conn, &n.id, &["queued"]).len();
            if active == 0 {
                RetireStep::Remove
            } else {
                RetireStep::Wait { active }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::proto::frames::{Availability, Caps, Control, Free, Offer};
    use std::collections::BTreeMap as Map;

    fn conn() -> Connection {
        crate::db::isolated_test_connection()
    }

    fn ws(c: &Connection, id: &str) {
        c.execute("INSERT INTO projects (id, path, name) VALUES (?1, ?2, ?1)", params![id, format!("/tmp/{id}")]).unwrap();
    }

    fn active_node(c: &Connection, id: &str, name: &str) {
        let labels = Map::from([("os".to_string(), "linux".to_string())]);
        store::insert_pending_node(c, id, name, &format!("fp-{id}"), "pem", &labels, "owner", "123456", 1).unwrap();
        store::confirm_node(c, id, "owner", 2).unwrap();
    }

    fn offer(free_disk: u64) -> Offer {
        Offer {
            revision: 1,
            control: Control::Active,
            control_by: None,
            control_at: None,
            availability: Availability { ok: true, reasons: vec![] },
            caps: Caps { cpu_millis: 8000, mem_bytes: 32 << 30, disk_budget_bytes: 100 << 30, hard: true },
            free: Free { cpu_millis: 8000, mem_bytes: 16 << 30, disk_free_bytes: free_disk },
            slots: vec![],
            running: 0,
            parallel_max: 2,
            refusal: None,
            labels: Map::new(),
            tools: Map::new(),
            protocol: 1,
            boot_id: "b".into(),
            ledger_id: "l".into(),
            node_version: "0".into(),
            features: vec![],
        }
    }

    #[test]
    fn disk_default_is_a_quarter_of_free_capped_at_60() {
        assert_eq!(default_disk_gb(None), 60);
        assert_eq!(default_disk_gb(Some(100 << 30)), 25);
        assert_eq!(default_disk_gb(Some(2000 << 30)), 60);
        assert_eq!(default_disk_gb(Some(1 << 30)), 1);
        assert_eq!(default_disk_gb(Some(0)), 1);
    }

    #[test]
    fn toggle_on_grants_every_active_node_with_defaults() {
        let c = conn();
        ws(&c, "w1");
        active_node(&c, "n1", "mini-1");
        active_node(&c, "n2", "mini-2");
        store::store_offer(&c, "n2", &offer(40 << 30), 3);
        // A pending node is not granted.
        store::insert_pending_node(&c, "n3", "mini-3", "fp-n3", "pem", &Map::new(), "owner", "1", 1).unwrap();

        let out = enable_workspace(&c, "w1", &GrantLimits::default(), "owner", 10).unwrap();
        assert_eq!(out.granted, vec!["n1".to_string(), "n2".to_string()]);
        assert!(store::agents_can_use_compute(&c, "w1"));
        let g1 = store::grant(&c, "n1", "w1").expect("grant on n1");
        assert_eq!((g1.max_job_secs, g1.max_parallel, g1.max_queued, g1.max_disk_gb), (7200, 1, 10, 60));
        let g2 = store::grant(&c, "n2", "w1").expect("grant on n2");
        assert_eq!(g2.max_disk_gb, 10, "25% of 40 GB free");
        assert!(store::grant(&c, "n3", "w1").is_none());
    }

    #[test]
    fn toggle_on_keeps_owner_limits_unless_new_limits_are_asked() {
        let c = conn();
        ws(&c, "w1");
        active_node(&c, "n1", "mini-1");
        store::set_grant(&c, "n1", "w1", &GrantLimits { max_parallel: Some(4), ..Default::default() }, "owner", 1).unwrap();
        let out = enable_workspace(&c, "w1", &GrantLimits::default(), "owner", 2).unwrap();
        assert_eq!(out.kept, vec!["n1".to_string()]);
        assert_eq!(store::grant(&c, "n1", "w1").unwrap().max_parallel, 4);

        let out = enable_workspace(&c, "w1", &GrantLimits { max_job_secs: Some(600), ..Default::default() }, "owner", 3).unwrap();
        assert_eq!(out.granted, vec!["n1".to_string()]);
        let g = store::grant(&c, "n1", "w1").unwrap();
        assert_eq!((g.max_job_secs, g.max_parallel), (600, 1));
    }

    #[test]
    fn new_node_is_granted_to_switched_on_workspaces_only() {
        let c = conn();
        ws(&c, "on");
        ws(&c, "off");
        active_node(&c, "n0", "mini-0");
        enable_workspace(&c, "on", &GrantLimits::default(), "owner", 1).unwrap();

        store::insert_pending_node(&c, "n1", "mini-1", "fp-n1", "pem", &Map::new(), "owner", "1", 1).unwrap();
        assert_eq!(grant_new_node(&c, "n1", "owner", 2).unwrap(), Vec::<String>::new(), "pending nodes get nothing");
        store::confirm_node(&c, "n1", "owner", 3).unwrap();
        assert_eq!(grant_new_node(&c, "n1", "owner", 4).unwrap(), vec!["on".to_string()]);
        assert!(store::grant(&c, "n1", "on").is_some());
        assert!(store::grant(&c, "n1", "off").is_none());
        // Idempotent.
        assert_eq!(grant_new_node(&c, "n1", "owner", 5).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn toggle_off_revokes_every_grant_of_the_workspace() {
        let c = conn();
        ws(&c, "w1");
        ws(&c, "w2");
        active_node(&c, "n1", "mini-1");
        active_node(&c, "n2", "mini-2");
        enable_workspace(&c, "w1", &GrantLimits::default(), "owner", 1).unwrap();
        enable_workspace(&c, "w2", &GrantLimits::default(), "owner", 1).unwrap();
        assert_eq!(disable_workspace(&c, "w1").unwrap(), 2);
        assert!(!store::agents_can_use_compute(&c, "w1"));
        assert!(store::grants(&c, None, Some("w1")).is_empty());
        assert_eq!(store::grants(&c, None, Some("w2")).len(), 2, "other workspaces keep theirs");
        assert!(disable_workspace(&c, "missing").is_err());
    }

    #[test]
    fn agents_lists_every_workspace_with_switch_and_grants() {
        let c = conn();
        ws(&c, "a");
        ws(&c, "b");
        active_node(&c, "n1", "mini-1");
        enable_workspace(&c, "b", &GrantLimits::default(), "owner", 1).unwrap();
        // (The fresh DB may carry seeded rows of its own: check ours.)
        let rows: Vec<AgentRow> = agents(&c).into_iter().filter(|r| r.workspace_id == "a" || r.workspace_id == "b").collect();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].workspace_id.as_str(), rows[0].agents_can_use_compute, rows[0].grants.len()), ("a", false, 0));
        assert_eq!((rows[1].workspace_id.as_str(), rows[1].agents_can_use_compute, rows[1].grants.len()), ("b", true, 1));
    }

    #[test]
    fn retiring_set_round_trips_and_steps() {
        let dir = std::env::temp_dir().join(format!("k2-retire-{}", std::process::id()));
        let path = dir.join("retiring.json");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(retiring_at(&path).is_empty());
        mark_retiring_at(&path, "n1", "owner", 7).unwrap();
        assert_eq!(retiring_at(&path).get("n1"), Some(&Retiring { by: "owner".into(), at: 7 }));
        assert!(unmark_retiring_at(&path, "n1").unwrap());
        assert!(!unmark_retiring_at(&path, "n1").unwrap());
        std::fs::write(&path, "not json").unwrap();
        assert!(retiring_at(&path).is_empty(), "a bad file reads as empty");
        std::fs::remove_dir_all(&dir).unwrap();

        let c = conn();
        assert_eq!(retire_step(&c, "nope"), RetireStep::Forget);
        active_node(&c, "n1", "mini-1");
        assert_eq!(retire_step(&c, "n1"), RetireStep::Remove);
        store::revoke_node(&c, "n1", "owner", 9).unwrap();
        assert_eq!(retire_step(&c, "n1"), RetireStep::Forget);
    }
}
