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
//!
//! **New nodes join (0.46.18, Rosson 2026-11-23).** A list follows new
//! nodes by default: a node newly added to this server joins every list
//! that exists ([`follow_new_node`]), so a new machine never waits on a
//! person per list. A person may fix a list (`k2 compute allow-nodes
//! --fixed on`): then only a person changes it. The flag is its own key
//! (`allow_nodes_fixed:<workspace id>`, value `1`), so no migration: a list
//! without it follows, which is what every list made before 0.46.18 does.
//! Clearing the list drops the flag with it.

use std::collections::BTreeSet;

use rusqlite::{params, Connection, OptionalExtension};

/// The `compute_admin_settings` key prefix.
pub const KEY_PREFIX: &str = "allow_nodes:";

/// The settings key for one workspace's list.
pub fn key(workspace_id: &str) -> String {
    format!("{KEY_PREFIX}{workspace_id}")
}

/// The `compute_admin_settings` key prefix of the "fixed" flag (0.46.18).
pub const FIXED_PREFIX: &str = "allow_nodes_fixed:";

/// The settings key for one workspace's "fixed" flag.
pub fn fixed_key(workspace_id: &str) -> String {
    format!("{FIXED_PREFIX}{workspace_id}")
}

/// Is the workspace's list fixed (new nodes don't join it)? A read error
/// counts as fixed: never widen a list K2 can't read the flag of.
pub fn is_fixed(conn: &Connection, workspace_id: &str) -> bool {
    match conn
        .query_row("SELECT value FROM compute_admin_settings WHERE key = ?1", params![fixed_key(workspace_id)], |r| r.get::<_, String>(0))
        .optional()
    {
        Ok(None) => false,
        Ok(Some(v)) => v.trim() != "0",
        Err(_) => true,
    }
}

/// Fix the list (`true`) or let it follow new nodes again (`false`).
pub fn set_fixed(conn: &Connection, workspace_id: &str, fixed: bool, by: &str, at: i64) -> Result<(), String> {
    if fixed {
        conn.execute(
            "INSERT INTO compute_admin_settings (key, value, updated_by, updated_at) VALUES (?1, '1', ?2, ?3) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
            params![fixed_key(workspace_id), by, at],
        )
    } else {
        conn.execute("DELETE FROM compute_admin_settings WHERE key = ?1", params![fixed_key(workspace_id)])
    }
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Every workspace id that has a list, sorted.
pub fn workspaces_with_lists(conn: &Connection) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT substr(key, ?2) FROM compute_admin_settings WHERE substr(key, 1, ?3) = ?1 ORDER BY key",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![KEY_PREFIX, KEY_PREFIX.len() as i64 + 1, KEY_PREFIX.len() as i64], |r| r.get::<_, String>(0))
        .map(|rows| rows.filter_map(Result::ok).filter(|w| !w.is_empty()).collect())
        .unwrap_or_default()
}

/// A node was just added to this server: put it on every list that
/// follows new nodes (not fixed, readable, not on it yet). Returns the
/// workspace ids whose list it joined. A list K2 can't read is left as it
/// is (it allows nothing until a person sets it again).
pub fn follow_new_node(conn: &Connection, node_id: &str, by: &str, at: i64) -> Result<Vec<String>, String> {
    let node_id = node_id.trim();
    if node_id.is_empty() {
        return Err("no node id".into());
    }
    let mut joined = Vec::new();
    for ws in workspaces_with_lists(conn) {
        if is_fixed(conn, &ws) {
            continue;
        }
        let Ok(Some(e)) = get(conn, &ws) else { continue };
        if e.nodes.iter().any(|n| n == node_id) {
            continue;
        }
        let mut nodes = e.nodes;
        nodes.push(node_id.to_string());
        set(conn, &ws, Some(&nodes), by, at)?;
        joined.push(ws);
    }
    Ok(joined)
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
        // No list, no flag: a list set again later follows new nodes.
        None => conn.execute("DELETE FROM compute_admin_settings WHERE key IN (?1, ?2)", params![key(workspace_id), fixed_key(workspace_id)]),
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

    /// 0.46.18: a new node joins every list that follows new nodes (the
    /// default, so every list made before 0.46.18 too), never a fixed one,
    /// never a workspace with no list, and never a list it is on already.
    #[test]
    fn a_new_node_joins_following_lists_but_not_fixed_ones() {
        let c = conn();
        set(&c, "follows", Some(&v(&["n1"])), "owner", 1).expect("set");
        set(&c, "fixed", Some(&v(&["n1"])), "owner", 1).expect("set");
        set(&c, "has-it", Some(&v(&["n1", "n9"])), "owner", 1).expect("set");
        set_fixed(&c, "fixed", true, "owner", 2).expect("fix");
        assert!(is_fixed(&c, "fixed") && !is_fixed(&c, "follows") && !is_fixed(&c, "none"));
        assert_eq!(workspaces_with_lists(&c), v(&["fixed", "follows", "has-it"]), "the flag is not a list");

        let joined = follow_new_node(&c, "n9", "new-node:owner", 5).expect("follow");
        assert_eq!(joined, v(&["follows"]));
        let e = get(&c, "follows").expect("read").expect("a list");
        assert_eq!(e.nodes, v(&["n1", "n9"]));
        assert_eq!((e.updated_by.as_str(), e.updated_at), ("new-node:owner", 5));
        assert_eq!(get(&c, "fixed").expect("read").expect("a list").nodes, v(&["n1"]), "a fixed list stays");
        assert_eq!(get(&c, "has-it").expect("read").expect("a list").updated_at, 1, "already on it: untouched");
        assert_eq!(get(&c, "none").expect("read"), None, "no list stays no list");
        assert_eq!(follow_new_node(&c, "n9", "x", 6).expect("again"), Vec::<String>::new(), "once only");

        // Unfixed, the next node joins it too; a cleared list drops its flag.
        set_fixed(&c, "fixed", false, "owner", 7).expect("unfix");
        assert_eq!(follow_new_node(&c, "n10", "x", 8).expect("follow"), v(&["fixed", "follows", "has-it"]));
        set_fixed(&c, "fixed", true, "owner", 9).expect("fix");
        set(&c, "fixed", None, "owner", 10).expect("clear");
        assert!(!is_fixed(&c, "fixed"), "clearing the list drops the flag");

        // A list K2 can't read is left alone (it allows nothing).
        c.execute("UPDATE compute_admin_settings SET value = 'not json' WHERE key = ?1", params![key("follows")]).expect("bad row");
        assert_eq!(follow_new_node(&c, "n11", "x", 11).expect("follow"), v(&["has-it"]));
        assert!(follow_new_node(&c, " ", "x", 12).is_err());
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
