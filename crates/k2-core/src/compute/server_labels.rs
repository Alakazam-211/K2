//! This server's own labels on compute nodes
//! (`k2 compute node label <node> add|remove <label>`, 0.46.18).
//!
//! A label is a word this server puts on a node it may use (`release`,
//! `for:0.46.18`), so its agents can send jobs only there
//! (`k2 compute run --label release`). Labels are **this server's**: they
//! live in this server's database and are read only by its own placement.
//! They are never sent to the node, and another server that uses the same
//! node sees none of them (it keeps its own).
//!
//! They are not the node's own labels (`os`, `arch` and the `k=v` labels
//! its owner sets on the machine, matched by `--label k=v`): a label here
//! has no `=`, which is how `--label` tells the two apart.
//!
//! Stored by node **id** in `compute_admin_settings` (key
//! `labels:<node id>`, the value a JSON list), so no migration, as
//! pinned-only and the workspace node lists are. A node removed and
//! enrolled again is a new node with no labels.
//!
//! Fails closed for placement: when the table can't be read, no node
//! carries any label, so a job that asks for one is placed nowhere rather
//! than somewhere it shouldn't go.
//!
//! Who changes them is the route's business (`k2-daemon`
//! `compute_node_labels`): the owner, an Admin, or an agent under "Allow
//! managing compute nodes". Every agent may read them.

use std::collections::{BTreeSet, HashMap};

use rusqlite::{params, Connection, OptionalExtension};

/// The `compute_admin_settings` key prefix.
pub const KEY_PREFIX: &str = "labels:";

/// The most labels one node carries for this server.
pub const MAX_PER_NODE: usize = 32;

/// The longest label.
pub const MAX_LEN: usize = 64;

/// The settings key for one node.
pub fn key(node_id: &str) -> String {
    format!("{KEY_PREFIX}{node_id}")
}

/// A label as stored: trimmed and lowercased; letters, digits and
/// `. _ : / -`, starting with a letter or digit, at most [`MAX_LEN`]. Never
/// `=` (that is the node's own `k=v` label).
pub fn normalize(raw: &str) -> Result<String, String> {
    let l = raw.trim().to_ascii_lowercase();
    if l.is_empty() {
        return Err("a label is a word, like release or for:0.46.18".into());
    }
    if l.contains('=') {
        return Err(format!(
            "'{raw}' has '=': key=value is the node's own label (set on the machine, matched by --label k=v); this server's labels are single words, like release or for:0.46.18"
        ));
    }
    if l.len() > MAX_LEN {
        return Err(format!("'{raw}' is longer than {MAX_LEN} characters"));
    }
    let ok_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-');
    if !l.starts_with(|c: char| c.is_ascii_alphanumeric()) || !l.chars().all(ok_char) {
        return Err(format!("'{raw}' isn't a label: use letters, digits and . _ : / - (starting with a letter or digit)"));
    }
    Ok(l)
}

/// One node's labels for this server (empty when it has none).
pub fn get(conn: &Connection, node_id: &str) -> Result<BTreeSet<String>, String> {
    let v: Option<String> = conn
        .query_row("SELECT value FROM compute_admin_settings WHERE key = ?1", params![key(node_id)], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    match v {
        None => Ok(BTreeSet::new()),
        Some(text) => serde_json::from_str::<BTreeSet<String>>(&text).map_err(|e| format!("labels for {node_id}: {e}")),
    }
}

/// Add and remove labels on a node. Returns its labels after, and whether
/// that changed anything. Every label is checked ([`normalize`]) before
/// anything is written; removing a label it doesn't carry is not an error.
pub fn change(conn: &Connection, node_id: &str, add: &[String], remove: &[String], by: &str, at: i64) -> Result<(BTreeSet<String>, bool), String> {
    let node_id = node_id.trim();
    if node_id.is_empty() {
        return Err("a label goes on a node: name one".into());
    }
    let add: Vec<String> = add.iter().map(|l| normalize(l)).collect::<Result<_, _>>()?;
    let remove: Vec<String> = remove.iter().map(|l| normalize(l)).collect::<Result<_, _>>()?;
    let before = get(conn, node_id)?;
    let mut after = before.clone();
    for l in &remove {
        after.remove(l);
    }
    after.extend(add);
    if after.len() > MAX_PER_NODE {
        return Err(format!("a node carries at most {MAX_PER_NODE} labels for this server"));
    }
    if after == before {
        return Ok((after, false));
    }
    if after.is_empty() {
        conn.execute("DELETE FROM compute_admin_settings WHERE key = ?1", params![key(node_id)])
    } else {
        let value = serde_json::to_string(&after).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO compute_admin_settings (key, value, updated_by, updated_at) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
            params![key(node_id), value, by, at],
        )
    }
    .map_err(|e| e.to_string())?;
    Ok((after, true))
}

/// Every node's labels, read once (placement checks many nodes).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Labels(pub HashMap<String, BTreeSet<String>>);

impl Labels {
    /// The node's labels (empty when none).
    pub fn of(&self, node_id: &str) -> BTreeSet<String> {
        self.0.get(node_id).cloned().unwrap_or_default()
    }

    /// Does the node carry every label in `want`? (`want` empty: yes.)
    pub fn has_all(&self, node_id: &str, want: &[String]) -> bool {
        want.is_empty() || self.0.get(node_id).is_some_and(|have| want.iter().all(|w| have.contains(w)))
    }
}

/// Every labelled node. Fails closed: an unreadable table (or a row that
/// doesn't parse) gives no labels, so a job asking for one finds no node.
pub fn all(conn: &Connection) -> Labels {
    let read = || -> rusqlite::Result<Vec<(String, String)>> {
        // `substr`, not LIKE (`_` is a LIKE wildcard).
        let mut stmt = conn.prepare("SELECT key, value FROM compute_admin_settings WHERE substr(key, 1, ?1) = ?2")?;
        let rows = stmt.query_map(params![KEY_PREFIX.len() as i64, KEY_PREFIX], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect()
    };
    let Ok(rows) = read() else { return Labels::default() };
    Labels(
        rows.into_iter()
            .filter_map(|(k, v)| Some((k[KEY_PREFIX.len()..].to_string(), serde_json::from_str::<BTreeSet<String>>(&v).ok()?)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        crate::db::isolated_test_connection()
    }

    fn set(ls: &[&str]) -> BTreeSet<String> {
        ls.iter().map(|s| s.to_string()).collect()
    }

    fn v(ls: &[&str]) -> Vec<String> {
        ls.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn add_remove_round_trips_and_says_when_nothing_changed() {
        let c = conn();
        assert_eq!(get(&c, "node-a").expect("read"), BTreeSet::new());
        let (now, changed) = change(&c, " node-a ", &v(&["Release", "for:0.46.18"]), &[], "owner", 10).expect("add");
        assert!(changed);
        assert_eq!(now, set(&["for:0.46.18", "release"]), "trimmed and lowercased");
        assert_eq!(get(&c, "node-a").expect("read"), now);
        // Adding again changes nothing.
        assert!(!change(&c, "node-a", &v(&["release"]), &[], "owner", 11).expect("again").1);
        // Remove one, and one it never had.
        let (now, changed) = change(&c, "node-a", &[], &v(&["release", "gpu"]), "owner", 12).expect("remove");
        assert!(changed);
        assert_eq!(now, set(&["for:0.46.18"]));
        // Removing the last label drops the row.
        change(&c, "node-a", &[], &v(&["for:0.46.18"]), "owner", 13).expect("remove last");
        let rows: i64 = c.query_row("SELECT count(*) FROM compute_admin_settings WHERE key = 'labels:node-a'", [], |r| r.get(0)).expect("count");
        assert_eq!(rows, 0);
        assert!(change(&c, "", &v(&["x"]), &[], "owner", 14).expect_err("no node").contains("name one"));
    }

    #[test]
    fn a_bad_label_writes_nothing() {
        let c = conn();
        let long = "x".repeat(MAX_LEN + 1);
        for bad in ["os=linux", "", " ", "-lead", "has space", long.as_str()] {
            let err = change(&c, "node-a", &v(&["good", bad]), &[], "owner", 1).expect_err(bad);
            assert!(!err.is_empty(), "{bad}");
        }
        assert!(normalize("os=linux").expect_err("=").contains("node's own label"));
        assert_eq!(get(&c, "node-a").expect("read"), BTreeSet::new(), "nothing written");
    }

    #[test]
    fn at_most_max_per_node() {
        let c = conn();
        let many: Vec<String> = (0..MAX_PER_NODE).map(|i| format!("l{i}")).collect();
        change(&c, "node-a", &many, &[], "owner", 1).expect("max fits");
        assert!(change(&c, "node-a", &v(&["one-more"]), &[], "owner", 2).expect_err("over").contains("at most"));
    }

    /// The point of the feature: a label lives in the server's own
    /// database. Two servers that both use node-a each see only their own.
    #[test]
    fn labels_are_per_server() {
        let server_a = conn();
        let server_b = conn();
        change(&server_a, "node-a", &v(&["release"]), &[], "owner", 1).expect("a labels it");
        assert_eq!(all(&server_a).of("node-a"), set(&["release"]));
        assert_eq!(get(&server_b, "node-a").expect("read"), BTreeSet::new(), "server-b sees none of server-a's labels");
        assert!(!all(&server_b).has_all("node-a", &v(&["release"])));
        change(&server_b, "node-a", &v(&["nightly"]), &[], "owner", 2).expect("b labels it");
        assert_eq!(all(&server_a).of("node-a"), set(&["release"]), "and server-a sees none of server-b's");
    }

    #[test]
    fn all_reads_only_its_keys_and_has_all_needs_every_label() {
        let c = conn();
        crate::compute::pinned_only::set(&c, "node-b", true, "owner", 1).expect("pinned");
        c.execute("INSERT INTO compute_admin_settings (key, value, updated_by, updated_at) VALUES ('labelsX:node-c', '[\"x\"]', 'x', 1)", [])
            .expect("insert");
        change(&c, "node:a", &v(&["release", "linux-big"]), &[], "owner", 2).expect("add");
        let all = all(&c);
        assert_eq!(all.0.len(), 1, "{all:?}");
        assert!(all.has_all("node:a", &v(&["release"])));
        assert!(all.has_all("node:a", &v(&["release", "linux-big"])));
        assert!(!all.has_all("node:a", &v(&["release", "gpu"])));
        assert!(!all.has_all("node-b", &v(&["release"])));
        assert!(all.has_all("node-b", &[]), "no labels asked: every node");
    }

    #[test]
    fn an_unreadable_table_gives_no_labels() {
        let c = conn();
        change(&c, "node-a", &v(&["release"]), &[], "owner", 1).expect("add");
        c.execute_batch("ALTER TABLE compute_admin_settings RENAME TO gone;").expect("rename");
        assert_eq!(all(&c), Labels::default());
        assert!(!all(&c).has_all("node-a", &v(&["release"])));
        assert!(get(&c, "node-a").is_err());
    }
}
