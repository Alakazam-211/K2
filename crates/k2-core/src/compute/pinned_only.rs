//! A compute node that takes only jobs that name it
//! (`k2 compute node pinned-only <node> on|off`).
//!
//! Automatic placement never chooses such a node: not for a job with no
//! node named, not for `--any`, not on a move or a retry. A job that names
//! the node runs there as on any other node. Jobs already queued or
//! running on it are left alone (unlike a pause).
//!
//! Why it exists: a node set up for one kind of job (the Mac node with no
//! job wall, for the tests that apply a sandbox themselves) must never
//! pick up whatever else is waiting.
//!
//! Stored by node **id** in `compute_admin_settings` (key
//! `pinned_only:<node id>`), so no migration, as the workspace node lists
//! are (`super::allow`). The row being there is the flag; its value is
//! not read. A node removed and enrolled again under the same name is a
//! new node and does not carry the flag over.
//!
//! Fails closed: when the table can't be read, every node counts as
//! pinned-only, so automatic placement chooses none of them rather than
//! one it shouldn't.
//!
//! Who changes it is the route's business (`k2-daemon`): the owner or an
//! Admin, never an agent. An agent asks one of them on a permission card
//! (`compute.node.pinned-only`, 0.46.18), and their Allow changes it.

use std::collections::BTreeSet;

use rusqlite::{params, Connection, OptionalExtension};

/// The `compute_admin_settings` key prefix.
pub const KEY_PREFIX: &str = "pinned_only:";

/// The settings key for one node.
pub fn key(node_id: &str) -> String {
    format!("{KEY_PREFIX}{node_id}")
}

/// Who turned it on for a node, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub updated_by: String,
    pub updated_at: i64,
}

/// The node's flag: `Ok(None)` when it is off.
pub fn get(conn: &Connection, node_id: &str) -> Result<Option<Entry>, String> {
    conn.query_row(
        "SELECT updated_by, updated_at FROM compute_admin_settings WHERE key = ?1",
        params![key(node_id)],
        |r| Ok(Entry { updated_by: r.get(0)?, updated_at: r.get(1)? }),
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Turn it on or off for a node. Returns whether that changed anything.
pub fn set(conn: &Connection, node_id: &str, on: bool, by: &str, at: i64) -> Result<bool, String> {
    let node_id = node_id.trim();
    if node_id.is_empty() {
        return Err("pinned-only names a node".into());
    }
    let was = get(conn, node_id)?.is_some();
    if on {
        conn.execute(
            "INSERT INTO compute_admin_settings (key, value, updated_by, updated_at) VALUES (?1, '1', ?2, ?3) \
             ON CONFLICT(key) DO NOTHING",
            params![key(node_id), by, at],
        )
    } else {
        conn.execute("DELETE FROM compute_admin_settings WHERE key = ?1", params![key(node_id)])
    }
    .map_err(|e| e.to_string())?;
    Ok(was != on)
}

/// The pinned-only nodes, read once (placement checks many nodes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pinned {
    /// These node ids.
    Only(BTreeSet<String>),
    /// The table couldn't be read: every node counts as pinned-only.
    Unreadable,
}

impl Pinned {
    /// Is `node_id` closed to automatic placement?
    pub fn holds(&self, node_id: &str) -> bool {
        match self {
            Pinned::Only(ids) => ids.contains(node_id),
            Pinned::Unreadable => true,
        }
    }
}

/// Every pinned-only node. Fails closed (see the module doc).
pub fn all(conn: &Connection) -> Pinned {
    let read = || -> rusqlite::Result<BTreeSet<String>> {
        // `substr`, not LIKE: `_` in the prefix is a LIKE wildcard.
        let mut stmt = conn.prepare("SELECT key FROM compute_admin_settings WHERE substr(key, 1, ?1) = ?2")?;
        let rows = stmt.query_map(params![KEY_PREFIX.len() as i64, KEY_PREFIX], |r| r.get::<_, String>(0))?;
        rows.map(|k| k.map(|k| k[KEY_PREFIX.len()..].to_string())).collect()
    };
    match read() {
        Ok(ids) => Pinned::Only(ids),
        Err(_) => Pinned::Unreadable,
    }
}

/// Is this one node pinned-only? A read error counts as yes.
pub fn is_on(conn: &Connection, node_id: &str) -> bool {
    !matches!(get(conn, node_id), Ok(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        crate::db::isolated_test_connection()
    }

    #[test]
    fn off_by_default_and_on_then_off_round_trips() {
        let c = conn();
        assert_eq!(get(&c, "rig2").expect("read"), None);
        assert!(!is_on(&c, "rig2"));
        assert_eq!(all(&c), Pinned::Only(BTreeSet::new()));

        assert!(set(&c, " rig2 ", true, "owner", 10).expect("on"), "it changed");
        assert_eq!(get(&c, "rig2").expect("read"), Some(Entry { updated_by: "owner".into(), updated_at: 10 }));
        assert!(is_on(&c, "rig2") && all(&c).holds("rig2"));
        // On again changes nothing, and keeps who first turned it on.
        assert!(!set(&c, "rig2", true, "admin", 11).expect("on again"));
        assert_eq!(get(&c, "rig2").expect("read").expect("on").updated_by, "owner");
        // Another node is untouched.
        assert!(!is_on(&c, "rig3") && !all(&c).holds("rig3"));

        assert!(set(&c, "rig2", false, "owner", 12).expect("off"), "it changed");
        assert_eq!(get(&c, "rig2").expect("read"), None);
        assert!(!set(&c, "rig2", false, "owner", 13).expect("off again"));
        assert!(set(&c, "", true, "owner", 14).expect_err("no node").contains("names a node"));
    }

    #[test]
    fn only_its_own_keys_count_and_the_node_id_is_read_whole() {
        let c = conn();
        // A workspace node list, and a key that only looks alike under
        // LIKE (`_` matches any character there).
        crate::compute::allow::set(&c, "ws1", Some(&["rig9".to_string()]), "owner", 1).expect("list");
        c.execute("INSERT INTO compute_admin_settings (key, value, updated_by, updated_at) VALUES ('pinnedXonly:rig7', '1', 'x', 1)", [])
            .expect("insert");
        assert_eq!(all(&c), Pinned::Only(BTreeSet::new()));
        set(&c, "node:with:colons", true, "owner", 2).expect("on");
        set(&c, "arch-box", true, "owner", 3).expect("on");
        assert_eq!(all(&c), Pinned::Only(BTreeSet::from(["arch-box".to_string(), "node:with:colons".to_string()])));
        assert!(!all(&c).holds("rig7") && !all(&c).holds("rig9"));
    }

    #[test]
    fn an_unreadable_table_closes_every_node() {
        let c = conn();
        set(&c, "rig2", true, "owner", 1).expect("on");
        c.execute_batch("ALTER TABLE compute_admin_settings RENAME TO gone;").expect("rename");
        assert_eq!(all(&c), Pinned::Unreadable);
        assert!(all(&c).holds("rig2") && all(&c).holds("any-node"));
        assert!(is_on(&c, "any-node"));
        assert!(get(&c, "rig2").is_err());
    }
}
