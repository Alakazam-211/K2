//! Rewriting the store into a fresh file: what makes a purge leave no bytes behind
//! (K-PURGE-2, E-ERASE-4: "pages rewritten, not marked free").
//!
//! **Why not redb's compaction.** `Database::compact` relocates live pages towards the
//! start of the file and truncates the tail. It does not zero freed pages: a page freed by
//! a delete, or left behind by copy-on-write (redb never updates a page in place), keeps its
//! old bytes until a later write happens to reuse it, and compaction only moves *live*
//! pages, so stale copies below the new end of the file survive it. The tests show a deleted
//! row's bytes are still in the file after it is deleted.
//!
//! **What this does instead.** [`DocStore::rewrite_file`] copies every row of every kit
//! table into a new redb file beside the store (`<file>.rewrite`), commits it (durable),
//! renames it over the store file (atomic: the path names the old store or the new one,
//! never a partial one) and syncs the folder; then the old file's handle is closed. The new
//! file is written only from rows that exist at that moment, so no byte of a row deleted or
//! redacted before it can be in it. A crash before the rename leaves the old store whole
//! and a stray temp file, which [`DocStore::open`] removes.
//!
//! **What it doesn't reach.** The old file's disk blocks are released to the file system,
//! not overwritten: they stay readable to raw-device forensics until reused, and in APFS
//! snapshots, Time Machine and other backups taken before (the "not reached" list). The
//! kit's process memory is not scrubbed.
//!
//! It holds the store's write guard for its whole length: every other store call waits.
//! Its cost is a full copy of the store, so it is for purges (rare), not routine use.

use std::path::{Path, PathBuf};

use redb::{Database, MultimapTableHandle, ReadTransaction, TableDefinition, TableHandle};

use super::blob::{BLOBS, BLOB_SPACES, BLOB_SYNC, BLOB_VERSIONS};
use super::history::{CHANGES, CHANGE_CLOCKS, REVERTS, REVERT_CARRY, REVERT_INDEX, REVERT_STATS};
use super::{
    DocStore, StoreError, APPROVALS, CLIENTS, DOCS, DOC_KEYS, HOST, META, PATHS, SNAPSHOTS, SYNC,
    UPDATES, VERSIONS,
};

/// Returned by [`DocStore::rewrite_file`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RewriteReport {
    /// The store file's size before and after.
    pub bytes_before: u64,
    pub bytes_after: u64,
    /// Rows copied.
    pub rows: u64,
}

/// Where a rewrite builds the new file.
pub(super) fn temp_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".rewrite");
    PathBuf::from(p)
}

/// Every table the kit knows. A table found in the file but not here stops a rewrite: it
/// would otherwise be dropped silently.
const TABLES: &[&str] = &[
    "meta",
    "docs",
    "paths",
    "snapshots",
    "updates",
    "clients",
    "versions",
    "approvals",
    "host",
    "sync",
    "doc_keys",
    "changes",
    "reverts",
    "revert_index",
    "revert_carry",
    "revert_stats",
    "change_clocks",
    "blob_spaces",
    "blobs",
    "blob_versions",
    "blob_sync",
];

fn copy<K: redb::Key + 'static, V: redb::Value + 'static>(
    r: &ReadTransaction,
    w: &redb::WriteTransaction,
    def: TableDefinition<K, V>,
) -> Result<u64, StoreError> {
    let src = match r.open_table(def) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let mut dst = w.open_table(def)?;
    let mut n = 0;
    for kv in redb::ReadableTable::iter(&src)? {
        let (k, v) = kv?;
        dst.insert(k.value(), v.value())?;
        n += 1;
    }
    Ok(n)
}

fn copy_all(old: &Database, new: &Database) -> Result<u64, StoreError> {
    let r = old.begin_read()?;
    if let Some(t) = r.list_multimap_tables()?.next() {
        return Err(StoreError::Db(format!(
            "rewrite: unknown multimap table {:?}",
            t.name()
        )));
    }
    for t in r.list_tables()? {
        if !TABLES.contains(&t.name()) {
            return Err(StoreError::Db(format!(
                "rewrite: unknown table {:?}",
                t.name()
            )));
        }
    }
    let w = new.begin_write()?;
    let mut n = 0;
    n += copy(&r, &w, META)?;
    n += copy(&r, &w, DOCS)?;
    n += copy(&r, &w, PATHS)?;
    n += copy(&r, &w, SNAPSHOTS)?;
    n += copy(&r, &w, UPDATES)?;
    n += copy(&r, &w, CLIENTS)?;
    n += copy(&r, &w, VERSIONS)?;
    n += copy(&r, &w, APPROVALS)?;
    n += copy(&r, &w, HOST)?;
    n += copy(&r, &w, SYNC)?;
    n += copy(&r, &w, DOC_KEYS)?;
    n += copy(&r, &w, CHANGES)?;
    n += copy(&r, &w, REVERTS)?;
    n += copy(&r, &w, REVERT_INDEX)?;
    n += copy(&r, &w, REVERT_CARRY)?;
    n += copy(&r, &w, REVERT_STATS)?;
    n += copy(&r, &w, CHANGE_CLOCKS)?;
    n += copy(&r, &w, BLOB_SPACES)?;
    n += copy(&r, &w, BLOBS)?;
    n += copy(&r, &w, BLOB_VERSIONS)?;
    n += copy(&r, &w, BLOB_SYNC)?;
    w.commit()?;
    Ok(n)
}

fn io(what: &str, e: std::io::Error) -> StoreError {
    StoreError::Db(format!("rewrite: {what}: {e}"))
}

impl DocStore {
    /// Rewrite the store into a fresh file and atomically put it in place of the old one
    /// (module docs): after it, the file holds no byte of anything deleted or redacted
    /// before it. Every other store call waits while it runs (a full copy). A purge calls
    /// it; a host may too (it also returns free space to the file system).
    pub fn rewrite_file(&self) -> Result<RewriteReport, StoreError> {
        let mut g = self.db.write().unwrap_or_else(|p| p.into_inner());
        let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let bytes_before = size(&self.path);
        let tmp = temp_path(&self.path);
        match std::fs::remove_file(&tmp) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io("removing an old temp file", e)),
        }
        let fresh = Database::create(&tmp)?;
        let rows = match copy_all(&g, &fresh) {
            Ok(n) => n,
            Err(e) => {
                drop(fresh);
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        };
        if let Err(e) = std::fs::rename(&tmp, &self.path) {
            drop(fresh);
            let _ = std::fs::remove_file(&tmp);
            return Err(io("replacing the store file", e));
        }
        if let Some(dir) = self.path.parent() {
            // the rename is in place; a failed folder sync only weakens its durability
            let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
        }
        // the old file is unlinked; closing it releases its blocks
        let old = std::mem::replace(&mut *g, fresh);
        drop(old);
        Ok(RewriteReport {
            bytes_before,
            bytes_after: size(&self.path),
            rows,
        })
    }

    /// Scan the store file's raw bytes for `needles`, whole or by any 8-byte window
    /// ([`super::scan_bytes`]'s rule), with the write guard held so no commit lands
    /// mid-scan. After a purge (which rewrites the file) a scan for the purged content
    /// should find only what is still live.
    pub fn scan_for(&self, needles: &[&[u8]]) -> Result<Vec<super::Finding>, StoreError> {
        let _g = self.db.write().unwrap_or_else(|p| p.into_inner());
        super::scan_file(&self.path, needles).map_err(|e| StoreError::Db(format!("scan: {e}")))
    }
}
