//! A workspace's compute node list (`k2 compute allow-nodes`, 0.46.8):
//! when a workspace has one, its jobs go only to the nodes on it. The
//! daemon checks it at submit (a named node, `--same-node-as`), in
//! auto-placement (`--any`, no node named: nodes off the list aren't
//! candidates), on every move and retry, and once more before a queued job
//! is handed to its node. No list = every granted node, as before.
//!
//! Why it exists: the whole source tree (and every `--with` repo) syncs to
//! the node a job runs on, so a workspace whose tree carries code that must
//! not leave certain machines needs the daemon, not a brief, to keep its
//! jobs there.
//!
//! Stored by node **id** in `compute_admin_settings` (key
//! `allow_nodes:<workspace id>`, a JSON array), so no migration: a renamed
//! node stays on the list; a node removed and enrolled again under the same
//! name is a new node and is NOT on it (fail closed). Any read error counts
//! as an empty list (nothing allowed), never as "no list".
//!
//! Who changes it is the route's business (`k2-daemon` `compute_allow`):
//! the owner or an Admin in person any way; an agent only narrows its own
//! workspace's ([`change`] says which a request is).

use std::collections::BTreeSet;

use rusqlite::{params, Connection, OptionalExtension};

/// The `compute_admin_settings` key prefix.
pub const KEY_PREFIX: &str = "allow_nodes:";

/// The settings key for one workspace's list.
pub fn key(workspace_id: &str) -> String {
    format!("{KEY_PREFIX}{workspace_id}")
}

/// One workspace's list, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Node ids, sorted, no duplicates, never empty.
    pub nodes: Vec<String>,
    pub updated_by: String,
    pub updated_at: i64,
}

/// The workspace's list: `Ok(None)` when it has none. `Err` on a database
/// error or a value that isn't a non-empty JSON array of ids; callers
/// treat that as "nothing allowed" ([`permit`]).
pub fn get(conn: &Connection, workspace_id: &str) -> Result<Option<Entry>, String> {
    let row: Option<(String, String, i64)> = conn
        .query_row(
            "SELECT value, updated_by, updated_at FROM compute_admin_settings WHERE key = ?1",
            params![key(workspace_id)],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((value, updated_by, updated_at)) = row else { return Ok(None) };
    let nodes: Vec<String> = serde_json::from_str(&value).map_err(|e| format!("allow_nodes for {workspace_id}: {e}"))?;
    if nodes.is_empty() || nodes.iter().any(|n| n.trim().is_empty()) {
        return Err(format!("allow_nodes for {workspace_id}: an empty list or id"));
    }
    Ok(Some(Entry { nodes, updated_by, updated_at }))
}

/// Set (`Some(ids)`, at least one) or clear (`None`) the list.
pub fn set(conn: &Connection, workspace_id: &str, nodes: Option<&[String]>, by: &str, at: i64) -> Result<(), String> {
    match nodes {
        Some(ids) => {
            let ids = normalize(ids);
            if ids.is_empty() {
                return Err("a node list names at least one node".into());
            }
            let value = serde_json::to_string(&ids).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO compute_admin_settings (key, value, updated_by, updated_at) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
                params![key(workspace_id), value, by, at],
            )
        }
        None => conn.execute("DELETE FROM compute_admin_settings WHERE key = ?1", params![key(workspace_id)]),
    }
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Sorted, trimmed, no duplicates or empties.
pub fn normalize(ids: &[String]) -> Vec<String> {
    ids.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<BTreeSet<_>>().into_iter().collect()
}

/// What a workspace's jobs may use, read once (placement checks many nodes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Permit {
    /// No list: every node (grants still decide).
    Any,
    /// Only these node ids (empty after a read error: none).
    Only(Vec<String>),
}

impl Permit {
    pub fn allows(&self, node_id: &str) -> bool {
        match self {
            Permit::Any => true,
            Permit::Only(ids) => ids.iter().any(|i| i == node_id),
        }
    }

    pub fn is_any(&self) -> bool {
        matches!(self, Permit::Any)
    }
}

/// The workspace's [`Permit`]. Fails closed: a read error allows nothing.
pub fn permit(conn: &Connection, workspace_id: &str) -> Permit {
    match get(conn, workspace_id) {
        Ok(None) => Permit::Any,
        Ok(Some(e)) => Permit::Only(e.nodes),
        Err(_) => Permit::Only(Vec::new()),
    }
}

/// May `workspace_id`'s jobs go to `node_id`?
pub fn allows(conn: &Connection, workspace_id: &str, node_id: &str) -> bool {
    permit(conn, workspace_id).allows(node_id)
}

/// What a change does to who may receive the workspace's code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Nothing changes.
    Same,
    /// Fewer nodes (or a first list, where there was none): an agent may.
    Narrow,
    /// More nodes, or clearing a list: a person only.
    Widen,
}

/// Classify `old` → `new` (`None` = no list = every node).
pub fn change(old: Option<&[String]>, new: Option<&[String]>) -> Change {
    match (old, new) {
        (None, None) => Change::Same,
        (Some(_), None) => Change::Widen,
        (None, Some(_)) => Change::Narrow,
        (Some(o), Some(n)) => {
            let o: BTreeSet<&str> = o.iter().map(String::as_str).collect();
            let n: BTreeSet<&str> = n.iter().map(String::as_str).collect();
            if o == n {
                Change::Same
            } else if n.is_subset(&o) {
                Change::Narrow
            } else {
                Change::Widen
            }
        }
    }
}

/// The ids `new` adds to `old` (empty when `old` is no list).
pub fn added(old: Option<&[String]>, new: &[String]) -> Vec<String> {
    match old {
        None => Vec::new(),
        Some(o) => normalize(new).into_iter().filter(|n| !o.contains(n)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        crate::db::isolated_test_connection()
    }

    fn v(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_list_is_any_and_a_list_round_trips_sorted() {
        let c = conn();
        assert_eq!(get(&c, "ws1").expect("read"), None);
        assert_eq!(permit(&c, "ws1"), Permit::Any);
        assert!(allows(&c, "ws1", "k2n2"));
        set(&c, "ws1", Some(&v(&["k2n9", " k2-arch ", "k2n9"])), "owner", 10).expect("set");
        let e = get(&c, "ws1").expect("read").expect("a list");
        assert_eq!(e.nodes, v(&["k2-arch", "k2n9"]));
        assert_eq!((e.updated_by.as_str(), e.updated_at), ("owner", 10));
        assert!(allows(&c, "ws1", "k2n9") && allows(&c, "ws1", "k2-arch"));
        assert!(!allows(&c, "ws1", "k2n2"));
        // Another workspace is untouched.
        assert_eq!(permit(&c, "ws2"), Permit::Any);
        set(&c, "ws1", None, "owner", 11).expect("clear");
        assert_eq!(get(&c, "ws1").expect("read"), None);
        assert!(allows(&c, "ws1", "k2n2"));
    }

    #[test]
    fn an_empty_list_is_refused_and_a_bad_value_allows_nothing() {
        let c = conn();
        let e = set(&c, "ws1", Some(&v(&["", "  "])), "owner", 1).expect_err("empty");
        assert!(e.contains("at least one"), "{e}");
        assert_eq!(get(&c, "ws1").expect("read"), None, "nothing stored");
        c.execute(
            "INSERT INTO compute_admin_settings (key, value, updated_by, updated_at) VALUES (?1, 'not json', 'x', 1)",
            params![key("ws1")],
        )
        .expect("bad row");
        assert!(get(&c, "ws1").is_err());
        assert_eq!(permit(&c, "ws1"), Permit::Only(Vec::new()), "fail closed");
        assert!(!allows(&c, "ws1", "k2n9"));
        c.execute("UPDATE compute_admin_settings SET value = '[]' WHERE key = ?1", params![key("ws1")]).expect("empty row");
        assert!(!allows(&c, "ws1", "k2n9"), "an empty stored list allows nothing");
    }

    #[test]
    fn changes_are_classified() {
        let ab = v(&["a", "b"]);
        let a = v(&["a"]);
        let ac = v(&["a", "c"]);
        let ba = v(&["b", "a"]);
        assert_eq!(change(None, None), Change::Same);
        assert_eq!(change(None, Some(&ab)), Change::Narrow, "a first list narrows");
        assert_eq!(change(Some(&ab), None), Change::Widen, "clearing widens");
        assert_eq!(change(Some(&ab), Some(&ba)), Change::Same);
        assert_eq!(change(Some(&ab), Some(&a)), Change::Narrow);
        assert_eq!(change(Some(&a), Some(&ab)), Change::Widen);
        assert_eq!(change(Some(&ab), Some(&ac)), Change::Widen, "a swap adds a node");
        assert_eq!(added(Some(&ab), &ac), v(&["c"]));
        assert_eq!(added(None, &ac), Vec::<String>::new());
    }
}
