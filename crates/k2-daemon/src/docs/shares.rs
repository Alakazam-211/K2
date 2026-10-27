//! Cross-workspace doc shares (MI25, PRD K-PERM-4, K-PERM-9).
//!
//! An agent may open a doc in **another** workspace only when both hold:
//! the two workspaces are connected (comms), **and** a person (owner token,
//! or an Admin/Owner login) shared that file, or a folder above it, with
//! the agent's workspace. Shares are off by default, read-only by default;
//! `write` and `history` are separate grants. Revoking deletes the row, and
//! open connections lose the right at the next upkeep pass (2 s). Every
//! share and unshare is an Access Audit line (metadata only).
//!
//! Stored in the kit's host table (`DocStore::host_*`, amendment 1), so a
//! share lives in `docs.redb` beside the docs it covers and moves with the
//! store across a hand-off. The kit never reads or erases these rows; K2
//! erases a workspace's shares (as owner or sharee) once the workspace is
//! gone ([`sweep_orphans`], from the upkeep).
//!
//! Key: `<owner project id>\0<sharee project id>\0<path>`; `path` is
//! workspace-relative and `""` is the whole workspace (folder shares only).

use serde::{Deserialize, Serialize};

use k2_multiplayer_base::store::{DocStore, HostOp, StoreError};

/// Host-table namespace.
pub const NS: &str = "k2.docs.shares";

/// One share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    /// The workspace whose file it is (project id).
    pub owner: String,
    /// The workspace it is shared with (project id).
    pub sharee: String,
    /// Workspace-relative; `""` = the whole workspace (a folder share).
    pub path: String,
    pub folder: bool,
    pub write: bool,
    pub history: bool,
    /// Who shared it (`person:<key>`).
    pub by: String,
    pub at_ms: u64,
}

impl Share {
    /// Whether this share covers `rel` (a file share: exactly it; a folder
    /// share: anything under it).
    pub fn covers(&self, rel: &str) -> bool {
        if self.folder {
            self.path.is_empty() || rel == self.path || rel.strip_prefix(self.path.as_str()).is_some_and(|r| r.starts_with('/'))
        } else {
            rel == self.path
        }
    }
}

fn key(owner: &str, sharee: &str, path: &str) -> String {
    format!("{owner}\u{0}{sharee}\u{0}{path}")
}

fn decode(v: &[u8]) -> Option<Share> {
    serde_json::from_slice(v).ok()
}

/// The union of every share from `owner` to `sharee` that covers `rel`:
/// `(write, history)`, or `None` when nothing covers it.
pub fn grant(store: &DocStore, owner: &str, sharee: &str, rel: &str) -> Result<Option<(bool, bool)>, StoreError> {
    let rows = store.host_scan(NS, &format!("{owner}\u{0}{sharee}\u{0}"))?;
    let mut out: Option<(bool, bool)> = None;
    for (_, v) in rows {
        let Some(s) = decode(&v) else { continue };
        if s.owner == owner && s.sharee == sharee && s.covers(rel) {
            let (w, h) = out.unwrap_or((false, false));
            out = Some((w || s.write, h || s.history));
        }
    }
    Ok(out)
}

/// Create or replace a share. Returns the previous one at the same key.
pub fn put(store: &DocStore, s: &Share) -> Result<Option<Share>, StoreError> {
    let k = key(&s.owner, &s.sharee, &s.path);
    let prev = store.host_get(NS, &k)?.and_then(|v| decode(&v));
    let v = serde_json::to_vec(s).expect("share serializes");
    store.host_put(NS, &k, &v)?;
    Ok(prev)
}

/// Remove one share; returns it when it existed.
pub fn remove(store: &DocStore, owner: &str, sharee: &str, path: &str) -> Result<Option<Share>, StoreError> {
    let k = key(owner, sharee, path);
    let prev = store.host_get(NS, &k)?.and_then(|v| decode(&v));
    if prev.is_some() {
        store.host_delete(NS, &k)?;
    }
    Ok(prev)
}

/// Every share, optionally only those of `owner` and/or with `sharee`.
pub fn list(store: &DocStore, owner: Option<&str>, sharee: Option<&str>) -> Result<Vec<Share>, StoreError> {
    let prefix = match (owner, sharee) {
        (Some(o), Some(s)) => format!("{o}\u{0}{s}\u{0}"),
        (Some(o), None) => format!("{o}\u{0}"),
        _ => String::new(),
    };
    Ok(store
        .host_scan(NS, &prefix)?
        .into_iter()
        .filter_map(|(_, v)| decode(&v))
        .filter(|s| sharee.is_none_or(|w| s.sharee == w))
        .collect())
}

/// Erase every share whose owner or sharee workspace `exists` says is gone
/// (a removed workspace's rows are K2's to erase, not the kit's). Returns
/// how many went. Rows that don't decode go too.
pub fn sweep_orphans(store: &DocStore, exists: impl Fn(&str) -> bool) -> Result<usize, StoreError> {
    let rows = store.host_scan(NS, "")?;
    let doomed: Vec<String> = rows
        .into_iter()
        .filter(|(_, v)| decode(v).is_none_or(|s| !exists(&s.owner) || !exists(&s.sharee)))
        .map(|(k, _)| k)
        .collect();
    if doomed.is_empty() {
        return Ok(0);
    }
    let ops: Vec<HostOp<'_>> = doomed.iter().map(|k| HostOp::Delete { ns: NS, key: k }).collect();
    store.host_batch(&ops)?;
    Ok(doomed.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (DocStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("k2-docs-shares-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        (DocStore::open(dir.join("docs.redb")).expect("open store"), dir)
    }

    fn share(owner: &str, sharee: &str, path: &str, folder: bool, write: bool, history: bool) -> Share {
        Share {
            owner: owner.into(),
            sharee: sharee.into(),
            path: path.into(),
            folder,
            write,
            history,
            by: "person:owner".into(),
            at_ms: 1,
        }
    }

    #[test]
    fn a_file_share_covers_only_the_file_and_a_folder_share_its_tree() {
        let f = share("a", "b", "notes/x.md", false, false, false);
        assert!(f.covers("notes/x.md"));
        assert!(!f.covers("notes/x.md.bak"));
        assert!(!f.covers("notes/y.md"));
        let d = share("a", "b", "notes", true, false, false);
        assert!(d.covers("notes/x.md"));
        assert!(d.covers("notes/deep/y.md"));
        assert!(!d.covers("notesx/y.md"), "a sibling with the same prefix is not inside");
        assert!(!d.covers("other.md"));
        assert!(share("a", "b", "", true, false, false).covers("anything/at/all.md"));
    }

    #[test]
    fn grants_are_per_pair_read_only_by_default_and_unioned() {
        let (st, dir) = store();
        assert_eq!(grant(&st, "a", "b", "notes/x.md").expect("scan"), None, "off by default");
        put(&st, &share("a", "b", "notes", true, false, false)).expect("put");
        assert_eq!(grant(&st, "a", "b", "notes/x.md").expect("scan"), Some((false, false)), "read only");
        assert_eq!(grant(&st, "a", "c", "notes/x.md").expect("scan"), None, "another sharee gets nothing");
        assert_eq!(grant(&st, "b", "a", "notes/x.md").expect("scan"), None, "shares are one-way");
        put(&st, &share("a", "b", "notes/x.md", false, true, false)).expect("put write");
        put(&st, &share("a", "b", "", true, false, true)).expect("put history");
        assert_eq!(grant(&st, "a", "b", "notes/x.md").expect("scan"), Some((true, true)));
        assert_eq!(grant(&st, "a", "b", "notes/y.md").expect("scan"), Some((false, true)));
        assert!(remove(&st, "a", "b", "").expect("rm").is_some());
        assert!(remove(&st, "a", "b", "").expect("rm again").is_none());
        assert_eq!(grant(&st, "a", "b", "notes/y.md").expect("scan"), Some((false, false)));
        assert_eq!(list(&st, Some("a"), None).expect("list").len(), 2);
        assert_eq!(list(&st, None, Some("b")).expect("list").len(), 2);
        assert_eq!(list(&st, None, Some("z")).expect("list").len(), 0);
        drop(st);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn a_gone_workspace_loses_its_shares_both_ways() {
        let (st, dir) = store();
        put(&st, &share("a", "b", "x.md", false, false, false)).expect("put");
        put(&st, &share("b", "c", "y.md", false, false, false)).expect("put");
        put(&st, &share("c", "a", "z.md", false, false, false)).expect("put");
        let n = sweep_orphans(&st, |id| id != "b").expect("sweep");
        assert_eq!(n, 2);
        let left = list(&st, None, None).expect("list");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].path, "z.md");
        drop(st);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
