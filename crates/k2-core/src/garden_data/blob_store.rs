//! Garden blobs over `gardens.sqlite` (prd-garden-board-needs-v1 GB5–GB11).
//!
//! - `blobs`: one row per file on disk (hash, size, sniffed mime).
//! - `space_blobs`: which space holds which blob, who uploaded it, its name,
//!   and how many live keys name it (`refs`). A blob counts toward its
//!   space's quota while held or pending (never named yet); one that was
//!   held and let go stops counting at once and is kept 35 days (GB11).
//! - `blob_refs`: (space, key, hash), kept inside the same transaction as
//!   every write (`SqliteStore::write`, GB7).
//!
//! Files are written and removed while the store's one connection is held,
//! so a put that finds a file already there (dedup) never races a GC that
//! removes it.

use std::collections::{BTreeSet, HashSet};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

use super::backend::Filter;
use super::blobs::{
    self, BlobGcReport, BlobInfo, BlobRow, BLOB_PENDING_KEEP_SECS, BLOB_UNREF_KEEP_SECS, MAX_BLOB_BYTES,
    MAX_MISSING_LISTED, STRAY_FILE_KEEP_SECS,
};
use super::sqlite::SqliteStore;
use super::{keys, Actor, DataError, Space};

/// One `blobs.list` page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobPage {
    pub items: Vec<BlobRow>,
    /// The last hash returned when more follow (an exclusive cursor).
    pub next: Option<String>,
    /// What the space is charged for now (held + pending), and its quota.
    pub bytes: i64,
    pub quota: i64,
}

fn meta_key(space: &Space) -> String {
    format!("aux_rev:{}", space.as_str())
}

// ── inside a write (sqlite.rs) ────────────────────────────────────────

/// GB3: every hash must be one this space holds; otherwise `blob_missing`
/// naming the first [`MAX_MISSING_LISTED`] refs.
pub(super) fn check_held(tx: &Transaction, space: &Space, hashes: &BTreeSet<&str>) -> Result<(), DataError> {
    if hashes.is_empty() {
        return Ok(());
    }
    let mut q = tx.prepare_cached("SELECT COUNT(*) > 0 FROM space_blobs WHERE space = ?1 AND hash = ?2")?;
    let mut missing = Vec::new();
    for h in hashes {
        let held: bool = q.query_row(params![space.as_str(), h], |r| r.get(0))?;
        if !held && missing.len() < MAX_MISSING_LISTED {
            missing.push(blobs::ref_of(h));
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(DataError::BlobMissing { refs: missing })
    }
}

/// Replace `key`'s refs with `new` and move the held counts (GB7).
pub(super) fn set_key_refs(
    tx: &Transaction,
    space: &Space,
    key: &str,
    new: &BTreeSet<String>,
    now: i64,
) -> Result<(), DataError> {
    let old: BTreeSet<String> = {
        let mut q = tx.prepare_cached("SELECT hash FROM blob_refs WHERE space = ?1 AND key = ?2")?;
        let r = q.query_map(params![space.as_str(), key], |r| r.get(0))?;
        r.collect::<Result<_, _>>()?
    };
    if old.is_empty() && new.is_empty() {
        return Ok(());
    }
    let mut del = tx.prepare_cached("DELETE FROM blob_refs WHERE space = ?1 AND key = ?2 AND hash = ?3")?;
    let mut down = tx.prepare_cached(
        "UPDATE space_blobs SET refs = MAX(refs - 1, 0),
           unref_at = CASE WHEN refs - 1 <= 0 THEN ?3 ELSE unref_at END
         WHERE space = ?1 AND hash = ?2",
    )?;
    for h in old.difference(new) {
        del.execute(params![space.as_str(), key, h])?;
        down.execute(params![space.as_str(), h, now])?;
    }
    let mut ins = tx.prepare_cached("INSERT OR IGNORE INTO blob_refs (space, key, hash) VALUES (?1, ?2, ?3)")?;
    let mut up = tx.prepare_cached(
        "UPDATE space_blobs SET refs = refs + 1, unref_at = NULL, first_ref_at = COALESCE(first_ref_at, ?3)
         WHERE space = ?1 AND hash = ?2",
    )?;
    for h in new.difference(&old) {
        ins.execute(params![space.as_str(), key, h])?;
        up.execute(params![space.as_str(), h, now])?;
    }
    Ok(())
}

/// The space rev the aux tables last saw (GB9).
pub(super) fn set_aux_rev(tx: &Transaction, space: &Space, rev: i64) -> Result<(), DataError> {
    tx.execute(
        "INSERT INTO _k2_meta (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![meta_key(space), rev.to_string()],
    )?;
    Ok(())
}

pub(super) fn forget_aux_rev(tx: &Transaction, space: &Space) -> Result<(), DataError> {
    tx.execute("DELETE FROM _k2_meta WHERE k = ?1", [meta_key(space)])?;
    Ok(())
}

pub(super) fn move_aux_rev(tx: &Transaction, from: &Space, to: &Space) -> Result<(), DataError> {
    tx.execute("DELETE FROM _k2_meta WHERE k = ?1", [meta_key(to)])?;
    tx.execute("UPDATE _k2_meta SET k = ?2 WHERE k = ?1", params![meta_key(from), meta_key(to)])?;
    Ok(())
}

/// Rebuild one space's refs from its live values (GB9): drop its
/// `blob_refs`, scan `entries`, keep refs to blobs the space holds (a ref
/// to one it doesn't hold is dropped and logged), recount.
fn rebuild_space(tx: &Transaction, space: &str, rev: i64, now: i64) -> Result<(), DataError> {
    tx.execute("DELETE FROM blob_refs WHERE space = ?1", [space])?;
    let held: HashSet<String> = {
        let mut q = tx.prepare("SELECT hash FROM space_blobs WHERE space = ?1")?;
        let r = q.query_map([space], |r| r.get(0))?;
        r.collect::<Result<_, _>>()?
    };
    if !held.is_empty() {
        let rows: Vec<(String, String)> = {
            let mut q = tx.prepare("SELECT key, value FROM entries WHERE space = ?1 AND deleted = 0")?;
            let r = q.query_map([space], |r| Ok((r.get(0)?, r.get(1)?)))?;
            r.collect::<Result<_, _>>()?
        };
        let mut ins = tx.prepare("INSERT OR IGNORE INTO blob_refs (space, key, hash) VALUES (?1, ?2, ?3)")?;
        for (key, text) in rows {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
            // Over 64 refs can't have been written by this K2; keep none.
            let Ok(found) = blobs::scan_refs(&v) else { continue };
            for h in found {
                if held.contains(&h) {
                    ins.execute(params![space, key, h])?;
                } else {
                    crate::log_debug!("[garden-data] reconcile: {space} {key} names a blob it doesn't hold; dropped");
                }
            }
        }
    }
    tx.execute(
        "UPDATE space_blobs SET refs = (SELECT COUNT(*) FROM blob_refs b WHERE b.space = space_blobs.space AND b.hash = space_blobs.hash)
         WHERE space = ?1",
        [space],
    )?;
    tx.execute(
        "UPDATE space_blobs SET unref_at = COALESCE(unref_at, ?2) WHERE space = ?1 AND refs = 0 AND first_ref_at IS NOT NULL",
        params![space, now],
    )?;
    tx.execute(
        "UPDATE space_blobs SET unref_at = NULL, first_ref_at = COALESCE(first_ref_at, ?2) WHERE space = ?1 AND refs > 0",
        params![space, now],
    )?;
    tx.execute(
        "INSERT INTO _k2_meta (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![format!("aux_rev:{space}"), rev.to_string()],
    )?;
    Ok(())
}

/// On open (GB9): any `data:` space whose rev differs from the rev the aux
/// tables last saw was written by an older K2 (or predates them): rebuild
/// its refs. Refs of spaces an older K2 deleted go too.
pub(super) fn reconcile_aux(conn: &mut Connection, now: i64) -> Result<(), DataError> {
    let stale: Vec<(String, i64)> = {
        let mut q = conn.prepare(
            "SELECT s.space, s.rev FROM spaces s LEFT JOIN _k2_meta m ON m.k = 'aux_rev:' || s.space
             WHERE s.space LIKE 'data:%' AND (m.v IS NULL OR m.v <> CAST(s.rev AS TEXT))",
        )?;
        let r = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        r.collect::<Result<_, _>>()?
    };
    let gone: i64 =
        conn.query_row("SELECT COUNT(*) FROM blob_refs WHERE space NOT IN (SELECT space FROM spaces)", [], |r| r.get(0))?;
    if stale.is_empty() && gone == 0 {
        return Ok(());
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (space, rev) in &stale {
        rebuild_space(&tx, space, *rev, now)?;
    }
    if gone > 0 {
        let spaces: Vec<String> = {
            let mut q = tx.prepare("SELECT DISTINCT space FROM blob_refs WHERE space NOT IN (SELECT space FROM spaces)")?;
            let r = q.query_map([], |r| r.get(0))?;
            r.collect::<Result<_, _>>()?
        };
        for s in spaces {
            rebuild_space(&tx, &s, 0, now)?;
            tx.execute("DELETE FROM _k2_meta WHERE k = ?1", [format!("aux_rev:{s}")])?;
        }
    }
    tx.commit()?;
    Ok(())
}

// ── reads ────────────────────────────────────────────────────────────

fn row_of(r: &rusqlite::Row) -> rusqlite::Result<BlobRow> {
    let first: Option<i64> = r.get(7)?;
    Ok(BlobRow {
        hash: r.get(0)?,
        size: r.get(1)?,
        mime: r.get(2)?,
        name: r.get(3)?,
        refs: r.get(4)?,
        added_at: r.get(5)?,
        added_by: r.get(6)?,
        pending: first.is_none(),
    })
}

const ROW_COLS: &str = "sb.hash, sb.size, b.mime, sb.name, sb.refs, sb.added_at, sb.added_by, sb.first_ref_at";

fn charged(conn: &Connection, space: &Space) -> Result<i64, DataError> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(size), 0) FROM space_blobs WHERE space = ?1 AND (refs > 0 OR first_ref_at IS NULL)",
        [space.as_str()],
        |r| r.get(0),
    )?)
}

fn total_bytes(conn: &Connection) -> Result<i64, DataError> {
    Ok(conn.query_row("SELECT COALESCE(SUM(size), 0) FROM blobs", [], |r| r.get(0))?)
}

fn data_only(space: &Space) -> Result<(), DataError> {
    if space.is_data() {
        Ok(())
    } else {
        Err(DataError::BadRequest("a widget's private save space keeps no files; use its data".into()))
    }
}

impl SqliteStore {
    /// Upload `bytes` into `space` (GB5, GB11, GB14). The hash is computed
    /// here; the mime is sniffed. Already held → `new: false`, nothing
    /// charged again.
    pub fn blob_put(
        &self,
        space: &Space,
        bytes: &[u8],
        declared_mime: Option<&str>,
        name: Option<&str>,
        by: &Actor,
    ) -> Result<BlobInfo, DataError> {
        data_only(space)?;
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(DataError::TooLarge(format!(
                "a file is at most {} MiB; this one is {} bytes",
                MAX_BLOB_BYTES / (1024 * 1024),
                bytes.len()
            )));
        }
        let hash = blobs::sha256_hex(bytes);
        let mime = blobs::sniff(bytes, declared_mime);
        let name = name.and_then(blobs::clean_name);
        let size = bytes.len() as i64;
        let now = self.now();
        let who = by.stored();
        let dir = self.blob_dir();
        let (space_cap, store_cap) = (self.space_blob_cap, self.blob_store_cap);
        self.with(|conn| {
            if let Some(row) = blob_row(conn, space, &hash)? {
                return Ok(row.info(false));
            }
            if charged(conn, space)? + size > space_cap {
                return Err(DataError::SpaceFull { what: "blobs" });
            }
            let on_disk: bool = conn.query_row("SELECT COUNT(*) > 0 FROM blobs WHERE hash = ?1", [&hash], |r| r.get(0))?;
            if !on_disk {
                let over = total_bytes(conn)? + size - store_cap;
                if over > 0 {
                    // GB10: store_full pressure runs the GC early, oldest
                    // let-go blobs first.
                    gc_pressure(conn, &dir, now, over)?;
                    if total_bytes(conn)? + size > store_cap {
                        return Err(DataError::StoreFull);
                    }
                }
            }
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            blobs::write_file(&dir, &hash, bytes)?;
            tx.execute(
                "INSERT OR IGNORE INTO blobs (hash, size, mime, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![hash, size, mime, now],
            )?;
            tx.execute(
                "INSERT INTO space_blobs (space, hash, size, added_at, added_by, name) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![space.as_str(), hash, size, now, who, name],
            )?;
            tx.commit()?;
            let row = blob_row(conn, space, &hash)?.ok_or_else(|| DataError::Unavailable("a blob row vanished".into()))?;
            Ok(row.info(true))
        })
    }

    /// The space's row for a blob, or `None` when it doesn't hold it.
    pub fn blob_row(&self, space: &Space, hash: &str) -> Result<Option<BlobRow>, DataError> {
        self.with(|conn| blob_row(conn, space, hash))
    }

    /// `len` bytes at `offset` of a blob this space holds (else
    /// `blob_missing`). Answers the row, the bytes and `eof`.
    pub fn blob_read(&self, space: &Space, hash: &str, offset: u64, len: usize) -> Result<(BlobRow, Vec<u8>, bool), DataError> {
        let dir = self.blob_dir();
        self.with(|conn| {
            let row = blob_row(conn, space, hash)?.ok_or_else(|| DataError::BlobMissing { refs: vec![blobs::ref_of(hash)] })?;
            let (bytes, eof) = blobs::read_range(&dir, hash, offset, len)?;
            Ok((row, bytes, eof))
        })
    }

    /// The live keys in `space` that name the blob (at most `limit`).
    pub fn blob_holders(&self, space: &Space, hash: &str, limit: usize) -> Result<Vec<String>, DataError> {
        self.with(|conn| {
            let mut q = conn.prepare_cached("SELECT key FROM blob_refs WHERE space = ?1 AND hash = ?2 ORDER BY key LIMIT ?3")?;
            let r = q.query_map(params![space.as_str(), hash, limit as i64], |r| r.get(0))?;
            r.collect::<Result<Vec<String>, _>>().map_err(DataError::from)
        })
    }

    /// What the space is charged for now and its quota (GB11).
    pub fn blob_usage(&self, space: &Space) -> Result<(i64, i64), DataError> {
        let cap = self.space_blob_cap;
        self.with(|conn| Ok((charged(conn, space)?, cap)))
    }

    /// A page of the space's blobs in hash order. With a filter (an agent's
    /// read prefixes) only blobs a key under one of them holds.
    pub fn blob_list(&self, space: &Space, after: Option<&str>, limit: usize, filter: Filter) -> Result<BlobPage, DataError> {
        let limit = limit.clamp(1, super::MAX_LIST_LIMIT);
        let cap = self.space_blob_cap;
        self.with(|conn| {
            let mut q = conn.prepare_cached(&format!(
                "SELECT {ROW_COLS} FROM space_blobs sb JOIN blobs b ON b.hash = sb.hash
                 WHERE sb.space = ?1 AND sb.hash > ?2 ORDER BY sb.hash"
            ))?;
            let mut keys_q = conn.prepare_cached("SELECT key FROM blob_refs WHERE space = ?1 AND hash = ?2")?;
            let mut rows = q.query(params![space.as_str(), after.unwrap_or("")])?;
            let mut items = Vec::new();
            let mut more = false;
            while let Some(r) = rows.next()? {
                let row = row_of(r)?;
                if let Some(ps) = filter {
                    let holders: Vec<String> = keys_q
                        .query_map(params![space.as_str(), row.hash], |r| r.get(0))?
                        .collect::<Result<_, _>>()?;
                    if !holders.iter().any(|k| ps.iter().any(|p| keys::under(p, k))) {
                        continue;
                    }
                }
                if items.len() >= limit {
                    more = true;
                    break;
                }
                items.push(row);
            }
            let next = if more { items.last().map(|r: &BlobRow| r.hash.clone()) } else { None };
            Ok(BlobPage { items, next, bytes: charged(conn, space)?, quota: cap })
        })
    }

    /// One GC pass (GB10): drop pending uploads past 24 h and let-go blobs
    /// past 35 days, then every file no space holds, then stray files a
    /// crash left (older than an hour).
    pub fn blob_gc(&self, now: i64) -> Result<BlobGcReport, DataError> {
        let dir = self.blob_dir();
        self.with(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let rows = tx.execute(
                "DELETE FROM space_blobs WHERE refs <= 0 AND (
                   (first_ref_at IS NULL AND added_at < ?1) OR (unref_at IS NOT NULL AND unref_at < ?2))",
                params![now - BLOB_PENDING_KEEP_SECS, now - BLOB_UNREF_KEEP_SECS],
            )?;
            let orphans = orphan_blobs(&tx)?;
            for (h, _) in &orphans {
                tx.execute("DELETE FROM blobs WHERE hash = ?1", [h])?;
            }
            tx.commit()?;
            let mut report = BlobGcReport { rows, ..Default::default() };
            for (h, size) in orphans {
                if blobs::remove_file(&dir, &h)? {
                    report.files += 1;
                }
                report.bytes += size;
            }
            // Strays: files with no `blobs` row, and `.part` leftovers.
            let known: HashSet<String> = {
                let mut q = conn.prepare("SELECT hash FROM blobs")?;
                let r = q.query_map([], |r| r.get(0))?;
                r.collect::<Result<_, _>>()?
            };
            let (files, parts) = blobs::walk_files(&dir)?;
            for (h, mtime) in files {
                if !known.contains(&h) && mtime < now - STRAY_FILE_KEEP_SECS && blobs::remove_file(&dir, &h)? {
                    report.files += 1;
                }
            }
            for (p, mtime) in parts {
                if mtime < now - STRAY_FILE_KEEP_SECS {
                    let _ = std::fs::remove_file(p);
                }
            }
            Ok(report)
        })
    }

    /// The whole blob folder's bytes (rows in `blobs`).
    pub fn blob_store_bytes(&self) -> Result<i64, DataError> {
        self.with(|conn| total_bytes(conn))
    }
}

fn blob_row(conn: &Connection, space: &Space, hash: &str) -> Result<Option<BlobRow>, DataError> {
    if !blobs::valid_hash(hash) {
        return Err(DataError::BadRequest("a blob ref looks like k2blob:sha256-<64 lowercase hex>".into()));
    }
    Ok(conn
        .query_row(
            &format!(
                "SELECT {ROW_COLS} FROM space_blobs sb JOIN blobs b ON b.hash = sb.hash WHERE sb.space = ?1 AND sb.hash = ?2"
            ),
            params![space.as_str(), hash],
            row_of,
        )
        .optional()?)
}

/// `blobs` rows no space holds: `(hash, size)`.
fn orphan_blobs(tx: &Transaction) -> Result<Vec<(String, i64)>, DataError> {
    let mut q = tx.prepare("SELECT hash, size FROM blobs WHERE hash NOT IN (SELECT hash FROM space_blobs)")?;
    let r = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(r.collect::<Result<_, _>>()?)
}

/// `store_full` pressure (GB10): drop let-go rows, oldest `unref_at`
/// first, until `need` bytes of files are freed (or none are left).
/// Pending uploads and held blobs are never touched.
fn gc_pressure(conn: &mut Connection, dir: &std::path::Path, _now: i64, need: i64) -> Result<i64, DataError> {
    let candidates: Vec<(String, String)> = {
        let mut q = conn.prepare(
            "SELECT space, hash FROM space_blobs WHERE refs <= 0 AND unref_at IS NOT NULL ORDER BY unref_at, hash",
        )?;
        let r = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        r.collect::<Result<_, _>>()?
    };
    let mut freed = 0i64;
    for (space, hash) in candidates {
        if freed >= need {
            break;
        }
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM space_blobs WHERE space = ?1 AND hash = ?2", params![space, hash])?;
        let still: bool = tx.query_row("SELECT COUNT(*) > 0 FROM space_blobs WHERE hash = ?1", [&hash], |r| r.get(0))?;
        let size: Option<i64> = if still {
            None
        } else {
            let s = tx.query_row("SELECT size FROM blobs WHERE hash = ?1", [&hash], |r| r.get(0)).optional()?;
            tx.execute("DELETE FROM blobs WHERE hash = ?1", [&hash])?;
            s
        };
        tx.commit()?;
        if let Some(s) = size {
            blobs::remove_file(dir, &hash)?;
            freed += s;
        }
    }
    Ok(freed)
}

#[cfg(test)]
#[path = "blob_store_tests.rs"]
mod tests;
