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

use super::blob::{BLOBS, BLOB_SPACES};
use super::{DocRows, DocStore, StoreError, DOC_TABLES, HOST, META, PATHS, SHARED_TABLES};

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

/// Whether the kit knows table `name`: [`SHARED_TABLES`] and [`DOC_TABLES`] (v0.9.7: the
/// same list a doc delete erases). A table found in the file but in neither stops a
/// rewrite: it would otherwise be dropped silently.
fn known(name: &str) -> bool {
    SHARED_TABLES.contains(&name) || DOC_TABLES.iter().any(|(t, _)| t.name() == name)
}

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

/// The first table this kit doesn't know (a multimap table, or a table in neither
/// [`SHARED_TABLES`] nor [`DOC_TABLES`]), if any: data a newer kit wrote without a format
/// bump.
fn unknown_table(r: &ReadTransaction) -> Result<Option<String>, StoreError> {
    if let Some(t) = r.list_multimap_tables()?.next() {
        return Ok(Some(t.name().to_owned()));
    }
    for t in r.list_tables()? {
        if !known(t.name()) {
            return Ok(Some(t.name().to_owned()));
        }
    }
    Ok(None)
}

fn copy_all(old: &Database, new: &Database) -> Result<u64, StoreError> {
    let r = old.begin_read()?;
    // a table this kit doesn't know can't be copied: refused as a newer kit's store (v0.9.5)
    if let Some(table) = unknown_table(&r)? {
        return Err(StoreError::NewerData { table });
    }
    let w = new.begin_write()?;
    let mut n = 0;
    // the shared tables (SHARED_TABLES), then every per-doc one
    n += copy(&r, &w, META)?;
    n += copy(&r, &w, PATHS)?;
    n += copy(&r, &w, HOST)?;
    n += copy(&r, &w, BLOB_SPACES)?;
    n += copy(&r, &w, BLOBS)?;
    for (t, _) in DOC_TABLES {
        n += match *t {
            DocRows::One(d) => copy(&r, &w, d)?,
            DocRows::Pair(d) => copy(&r, &w, d)?,
            DocRows::Owner(d) => copy(&r, &w, d)?,
            DocRows::Slot(d) => copy(&r, &w, d)?,
            DocRows::Clock(d) => copy(&r, &w, d)?,
        };
    }
    // a known table this didn't copy (a name in SHARED_TABLES without its copy above)
    // stops the rewrite instead of dropping its rows
    let copied: Vec<String> = w.list_tables()?.map(|t| t.name().to_owned()).collect();
    for t in r.list_tables()? {
        if !copied.iter().any(|c| c == t.name()) {
            return Err(StoreError::Db(format!(
                "rewrite: table {:?} not copied",
                t.name()
            )));
        }
    }
    w.commit()?;
    Ok(n)
}

fn io(what: &str, e: std::io::Error) -> StoreError {
    StoreError::Db(format!("rewrite: {what}: {e}"))
}

impl DocStore {
    /// Whether [`DocStore::rewrite_file`] can copy this store: [`StoreError::NewerData`]
    /// when it holds a table this kit doesn't know (a newer kit's data). Read-only. A purge
    /// checks it before changing anything (v0.9.5), so it refuses cleanly instead of purging
    /// rows and then failing to rewrite the file.
    pub fn check_rewritable(&self) -> Result<(), StoreError> {
        let g = self.db.read().unwrap_or_else(|p| p.into_inner());
        let r = g.begin_read()?;
        match unknown_table(&r)? {
            Some(table) => Err(StoreError::NewerData { table }),
            None => Ok(()),
        }
    }

    /// Test-only: add a table this kit doesn't know (as a newer kit would), with one row, through redb's normal API; a multimap table when
    /// `multimap`.
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn add_table_for_tests(&self, name: &str, multimap: bool) -> Result<(), StoreError> {
        let g = self.db.write().unwrap_or_else(|p| p.into_inner());
        let w = g.begin_write()?;
        if multimap {
            let mut t =
                w.open_multimap_table(redb::MultimapTableDefinition::<u64, u64>::new(name))?;
            t.insert(1, 2)?;
        } else {
            let mut t = w.open_table(TableDefinition::<u64, &[u8]>::new(name))?;
            t.insert(1, b"a newer kit's row".as_slice())?;
        }
        w.commit()?;
        Ok(())
    }

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
