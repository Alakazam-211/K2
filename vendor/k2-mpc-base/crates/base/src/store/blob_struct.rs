//! The structure index in the store (`docs/SESSIONS.md`, "The structure index").
//!
//! - [`BLOB_STRUCT`]: `(space no, blob id, pack, format)` → one row, content-keyed, so two
//!   versions or docs holding the same bytes share it. A row exists only while its blob row
//!   does: it goes in the same transaction that removes the blob ([`remove_rows`],
//!   [`remove_space_rows`]), and a commit re-checks the blob in its own transaction.
//! - [`BLOB_STRUCT_KEYS`]: `(doc, version)` → the minted keys of one version of one doc, an
//!   overlay on the row (per doc, never content-keyed).
//!
//! Both are added without a format bump and **created lazily**, by the first index a host
//! commits: a store none reached keeps no such table, and a host that hasn't adopted the index
//! can still purge after rolling this kit back. Every reader takes a missing table as empty.

use redb::{ReadTransaction, ReadableTable, TableDefinition, TableHandle, WriteTransaction};

use super::blob::{dec_version, BLOBS, BLOB_VERSIONS};
use super::{history, DocId, DocStore, StoreError, VersionId};
use crate::blob::BlobId;
use crate::index::{keys_row_is_for, peek_row, RowStatus};

/// (space number, blob id, pack, format) → one index row.
pub(super) const BLOB_STRUCT: TableDefinition<(u64, &[u8], &str, u32), &[u8]> =
    TableDefinition::new("blob_struct");
/// (doc, version) → the minted keys of that version.
pub(super) const BLOB_STRUCT_KEYS: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("blob_struct_keys");

/// A neighbour version's raw row and raw keys ([`DocStore::blob_struct_neighbour`]).
type NeighbourRows = (Vec<u8>, Vec<u8>);

/// One row's key in `blob_struct`, as the orphan sweep carries it.
type StructKey = (u64, Vec<u8>, String, u32);

/// How many orphan rows one sweep transaction removes.
const SWEEP_BATCH: usize = 1024;

/// How many versions the key overlay looks past in each direction for a neighbour.
const NEIGHBOUR_LOOKBACK: u64 = 64;

/// A row to commit.
pub(crate) struct StructPut<'a> {
    pub doc: DocId,
    pub version: VersionId,
    pub no: u64,
    pub blob: &'a BlobId,
    pub pack: &'a str,
    pub format: u32,
    pub row: &'a [u8],
    pub keys: Option<&'a [u8]>,
}

/// How [`DocStore::blob_struct_commit`] went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StructCommitted {
    Done,
    /// A row for these bytes was already there; it stays, and nothing was written.
    Existing,
    /// The version or its blob moved under the job; nothing was written.
    Stale(&'static str),
}

/// The empty id: everything of a space's rows sort after it.
const ID_MIN: &[u8] = b"";

/// Remove every row of blob `(no, id)` in `w`. A missing table has none.
pub(super) fn remove_rows(w: &WriteTransaction, no: u64, id: &BlobId) -> Result<u64, StoreError> {
    if !history::has_table(w, BLOB_STRUCT.name())? {
        return Ok(0);
    }
    #[cfg(any(test, feature = "differential"))]
    if matches!(
        crate::differential::index_mutant(),
        Some(
            crate::differential::IndexMutant::PurgeKeepsRow
                | crate::differential::IndexMutant::NoRowRemoval
        )
    ) {
        return Ok(0);
    }
    let idb = id.as_bytes().as_slice();
    let mut t = w.open_table(BLOB_STRUCT)?;
    let mut found: Vec<(String, u32)> = Vec::new();
    for kv in t.range((no, idb, "", 0u32)..)? {
        let (k, _) = kv?;
        let (n, i, pack, format) = k.value();
        if n != no || i != idb {
            break;
        }
        found.push((pack.to_owned(), format));
    }
    let n = found.len() as u64;
    for (pack, format) in found {
        t.remove((no, idb, pack.as_str(), format))?;
    }
    Ok(n)
}

/// Remove every row of space `no` in `w`. A missing table has none.
pub(super) fn remove_space_rows(w: &WriteTransaction, no: u64) -> Result<u64, StoreError> {
    if !history::has_table(w, BLOB_STRUCT.name())? {
        return Ok(0);
    }
    #[cfg(any(test, feature = "differential"))]
    if matches!(
        crate::differential::index_mutant(),
        Some(
            crate::differential::IndexMutant::PurgeKeepsRow
                | crate::differential::IndexMutant::NoRowRemoval
        )
    ) {
        return Ok(0);
    }
    let mut t = w.open_table(BLOB_STRUCT)?;
    let mut found: Vec<(Vec<u8>, String, u32)> = Vec::new();
    for kv in t.range((no, ID_MIN, "", 0u32)..)? {
        let (k, _) = kv?;
        let (n, id, pack, format) = k.value();
        if n != no {
            break;
        }
        found.push((id.to_vec(), pack.to_owned(), format));
    }
    let n = found.len() as u64;
    for (id, pack, format) in found {
        t.remove((no, id.as_slice(), pack.as_str(), format))?;
    }
    Ok(n)
}

/// Remove doc `doc`'s key overlay row of version `vid` in `w` (a purged version).
pub(super) fn remove_keys(
    w: &WriteTransaction,
    doc: DocId,
    vid: VersionId,
) -> Result<(), StoreError> {
    if history::has_table(w, BLOB_STRUCT_KEYS.name())? {
        w.open_table(BLOB_STRUCT_KEYS)?.remove((doc, vid))?;
    }
    Ok(())
}

/// Rewrite: copy `blob_struct`'s rows that still have a blob row (a row without one, left by
/// a kit that doesn't know this table, must not reach the new file). A missing table stays
/// missing. Returns the rows copied.
pub(super) fn copy_live_rows(r: &ReadTransaction, w: &WriteTransaction) -> Result<u64, StoreError> {
    let src = match r.open_table(BLOB_STRUCT) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let blobs = r.open_table(BLOBS)?;
    #[cfg(any(test, feature = "differential"))]
    let copy_orphans = matches!(
        crate::differential::index_mutant(),
        Some(
            crate::differential::IndexMutant::OrphanSurvivesRewrite
                | crate::differential::IndexMutant::NoRowRemoval
        )
    );
    #[cfg(not(any(test, feature = "differential")))]
    let copy_orphans = false;
    let mut dst = w.open_table(BLOB_STRUCT)?;
    let mut n = 0;
    for kv in src.iter()? {
        let (k, v) = kv?;
        let (no, id, _, _) = k.value();
        if !copy_orphans && blobs.get((no, id))?.is_none() {
            continue;
        }
        dst.insert(k.value(), v.value())?;
        n += 1;
    }
    Ok(n)
}

impl DocStore {
    /// A row's status, from a store with or without the table.
    pub(crate) fn blob_struct_status(
        &self,
        no: u64,
        id: &BlobId,
        pack: &str,
        format: u32,
    ) -> Result<Option<RowStatus>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(BLOB_STRUCT) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let v = t.get((no, id.as_bytes().as_slice(), pack, format))?;
        v.map(|v| peek_row(v.value())).transpose()
    }

    /// A row's stored bytes.
    pub(crate) fn blob_struct_row(
        &self,
        no: u64,
        id: &BlobId,
        pack: &str,
        format: u32,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(BLOB_STRUCT) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let v = t.get((no, id.as_bytes().as_slice(), pack, format))?;
        Ok(v.map(|v| v.value().to_vec()))
    }

    /// The minted keys of version `vid`.
    pub(crate) fn blob_struct_keys(
        &self,
        doc: DocId,
        vid: VersionId,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(BLOB_STRUCT_KEYS) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let v = t.get((doc, vid))?;
        Ok(v.map(|v| v.value().to_vec()))
    }

    /// The nearest version of `doc` around `vid` (earlier first, then later) whose index for
    /// `(no, pack, format)` is ready and has minted keys: `(its row, its keys)`, for carrying
    /// keys forward. At most [`NEIGHBOUR_LOOKBACK`] versions each way.
    pub(crate) fn blob_struct_neighbour(
        &self,
        doc: DocId,
        vid: VersionId,
        no: u64,
        pack: &str,
        format: u32,
    ) -> Result<Option<NeighbourRows>, StoreError> {
        // an overlay of another pack or format is not a neighbour: its positions mean other
        // nodes
        let r = self.begin_read()?;
        let keys = match r.open_table(BLOB_STRUCT_KEYS) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let rows = match r.open_table(BLOB_STRUCT) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let versions = r.open_table(BLOB_VERSIONS)?;
        let earlier = (vid.saturating_sub(NEIGHBOUR_LOOKBACK)..vid).rev();
        let later = (vid + 1)..=vid.saturating_add(NEIGHBOUR_LOOKBACK);
        for v in earlier.chain(later) {
            let Some(k) = keys.get((doc, v))? else {
                continue;
            };
            if !keys_row_is_for(k.value(), pack, format)? {
                continue;
            }
            let Some(ver) = versions.get((doc, v))? else {
                continue;
            };
            let ver = dec_version(ver.value())?;
            if ver.redacted {
                continue;
            }
            let row = rows.get((no, ver.blob.as_bytes().as_slice(), pack, format))?;
            if let Some(row) = row {
                return Ok(Some((row.value().to_vec(), k.value().to_vec())));
            }
        }
        Ok(None)
    }

    /// The minted keys of the newest other version of `doc` that holds blob `id`, if any: a
    /// restore's keys are the keys that content had.
    pub(crate) fn blob_struct_keys_of_blob(
        &self,
        doc: DocId,
        vid: VersionId,
        id: &BlobId,
        pack: &str,
        format: u32,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let r = self.begin_read()?;
        let keys = match r.open_table(BLOB_STRUCT_KEYS) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let versions = r.open_table(BLOB_VERSIONS)?;
        let range = versions.range((doc, 0)..=(doc, u64::MAX))?.rev();
        for kv in range {
            let (k, v) = kv?;
            let at = k.value().1;
            if at == vid {
                continue;
            }
            let v = dec_version(v.value())?;
            if v.redacted || v.blob != *id {
                continue;
            }
            if let Some(found) = keys.get((doc, at))? {
                if keys_row_is_for(found.value(), pack, format)? {
                    return Ok(Some(found.value().to_vec()));
                }
            }
        }
        Ok(None)
    }

    /// Commit an index row (and the version's minted keys) in one transaction, re-checking
    /// that the version still exists, still holds this blob and isn't purged, and that the
    /// blob row is still there: a job racing a purge can't put a row back.
    pub(crate) fn blob_struct_commit(
        &self,
        p: &StructPut<'_>,
    ) -> Result<StructCommitted, StoreError> {
        let w = self.begin_write()?;
        #[cfg(any(test, feature = "differential"))]
        let skip_check = crate::differential::index_mutant()
            == Some(crate::differential::IndexMutant::StaleCommitAccepted);
        #[cfg(not(any(test, feature = "differential")))]
        let skip_check = false;
        if !skip_check {
            {
                let versions = w.open_table(BLOB_VERSIONS)?;
                let Some(v) = versions.get((p.doc, p.version))? else {
                    return Ok(StructCommitted::Stale("the version is gone"));
                };
                let v = dec_version(v.value())?;
                if v.redacted {
                    return Ok(StructCommitted::Stale("the version was purged"));
                }
                if v.blob != *p.blob {
                    return Ok(StructCommitted::Stale("the version holds other bytes"));
                }
            }
            let blobs = w.open_table(BLOBS)?;
            if blobs.get((p.no, p.blob.as_bytes().as_slice()))?.is_none() {
                return Ok(StructCommitted::Stale("the blob was removed"));
            }
        }
        #[cfg(any(test, feature = "differential"))]
        let overwrite = crate::differential::index_mutant()
            == Some(crate::differential::IndexMutant::KeysOverwritten);
        #[cfg(not(any(test, feature = "differential")))]
        let overwrite = false;
        // the first job to record these bytes wins: a job that outlived a session unload must
        // not change what readers were already shown
        if !overwrite
            && history::has_table(&w, BLOB_STRUCT.name())?
            && w.open_table(BLOB_STRUCT)?
                .get((p.no, p.blob.as_bytes().as_slice(), p.pack, p.format))?
                .is_some()
        {
            return Ok(StructCommitted::Existing);
        }
        {
            let mut t = w.open_table(BLOB_STRUCT)?;
            let idb = p.blob.as_bytes().as_slice();
            // older formats of this pack go when a newer one lands
            t.retain_in(
                (p.no, idb, p.pack, 0u32)..=(p.no, idb, p.pack, u32::MAX),
                |k, _| k.3 == p.format,
            )?;
            t.insert((p.no, idb, p.pack, p.format), p.row)?;
        }
        if let Some(keys) = p.keys {
            let mut t = w.open_table(BLOB_STRUCT_KEYS)?;
            // keys minted once stay, under this format; an older format's is replaced
            let kept = match t.get((p.doc, p.version))? {
                Some(v) => keys_row_is_for(v.value(), p.pack, p.format)?,
                None => false,
            };
            if overwrite || !kept {
                t.insert((p.doc, p.version), keys)?;
            }
        }
        w.commit()?;
        Ok(StructCommitted::Done)
    }

    /// Store minted keys for a version that has none under this pack and format (a reused
    /// row: a restore or equal bytes, read for the first time here), in one transaction that
    /// re-checks the version (it still exists, holds `blob`, and isn't purged). Returns what
    /// is stored, the first writer's if there is one; `None` when the version moved.
    pub(crate) fn blob_struct_put_keys(
        &self,
        doc: DocId,
        vid: VersionId,
        blob: &BlobId,
        pack: &str,
        format: u32,
        keys: &[u8],
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let w = self.begin_write()?;
        #[cfg(any(test, feature = "differential"))]
        let skip_check = crate::differential::index_mutant()
            == Some(crate::differential::IndexMutant::PutKeysNoCheck);
        #[cfg(not(any(test, feature = "differential")))]
        let skip_check = false;
        if !skip_check {
            let versions = w.open_table(BLOB_VERSIONS)?;
            let Some(v) = versions.get((doc, vid))? else {
                return Ok(None);
            };
            let v = dec_version(v.value())?;
            if v.redacted || v.blob != *blob {
                return Ok(None);
            }
        }
        let out = {
            let mut t = w.open_table(BLOB_STRUCT_KEYS)?;
            let found = match t.get((doc, vid))? {
                Some(v) if keys_row_is_for(v.value(), pack, format)? => Some(v.value().to_vec()),
                _ => None,
            };
            match found {
                Some(v) => v,
                None => {
                    t.insert((doc, vid), keys)?;
                    keys.to_vec()
                }
            }
        };
        w.commit()?;
        Ok(Some(out))
    }

    /// Remove rows whose blob row is gone (left by a kit that doesn't know this table), from
    /// [`crate::blob::BlobStore::gc`]: the rows are found in a read transaction, and only
    /// those found are deleted, in short write transactions. Returns the rows removed.
    pub(crate) fn blob_struct_sweep_orphans(&self) -> Result<u64, StoreError> {
        #[cfg(any(test, feature = "differential"))]
        if crate::differential::index_mutant()
            == Some(crate::differential::IndexMutant::NoRowRemoval)
        {
            return Ok(0);
        }
        let found = self.blob_struct_orphans()?;
        let mut n = 0;
        for batch in found.chunks(SWEEP_BATCH) {
            n += self.blob_struct_remove_orphans(batch)?;
        }
        Ok(n)
    }

    /// The rows whose blob row is gone, from a read transaction.
    fn blob_struct_orphans(&self) -> Result<Vec<StructKey>, StoreError> {
        #[cfg(any(test, feature = "differential"))]
        let _writer = if crate::differential::index_mutant()
            == Some(crate::differential::IndexMutant::SweepHoldsWriter)
        {
            Some(self.begin_write()?)
        } else {
            None
        };
        let r = self.begin_read()?;
        let t = match r.open_table(BLOB_STRUCT) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let blobs = r.open_table(BLOBS)?;
        let mut gone = Vec::new();
        for kv in t.iter()? {
            let (k, _) = kv?;
            let (no, id, pack, format) = k.value();
            if blobs.get((no, id))?.is_none() {
                gone.push((no, id.to_vec(), pack.to_owned(), format));
            }
        }
        Ok(gone)
    }

    /// Delete `rows`, re-checking in the transaction that each blob is still gone (a blob
    /// stored again since the scan keeps its row).
    fn blob_struct_remove_orphans(&self, rows: &[StructKey]) -> Result<u64, StoreError> {
        #[cfg(any(test, feature = "differential"))]
        let skip_check = crate::differential::index_mutant()
            == Some(crate::differential::IndexMutant::SweepSkipsRecheck);
        #[cfg(not(any(test, feature = "differential")))]
        let skip_check = false;
        let w = self.begin_write()?;
        let mut n = 0;
        {
            let mut t = w.open_table(BLOB_STRUCT)?;
            let blobs = w.open_table(BLOBS)?;
            for (no, id, pack, format) in rows {
                if !skip_check && blobs.get((*no, id.as_slice()))?.is_some() {
                    continue;
                }
                t.remove((*no, id.as_slice(), pack.as_str(), *format))?;
                n += 1;
            }
        }
        w.commit()?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::blob::BlobId;

    fn store(tag: &str) -> (DocStore, std::path::PathBuf) {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2mb-struct-unit-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (DocStore::open(dir.join("docs.redb")).unwrap(), dir)
    }

    fn id(b: u8) -> BlobId {
        BlobId::from_slice(&[b; 32]).unwrap()
    }

    /// A row in the table, with or without a blob row beside it.
    fn put_row(s: &DocStore, blob: u8, row: &[u8], with_blob: bool) {
        let w = s.begin_write().unwrap();
        {
            let mut t = w.open_table(BLOB_STRUCT).unwrap();
            let idb = id(blob);
            t.insert((1u64, idb.as_bytes().as_slice(), "idx", 1u32), row)
                .unwrap();
        }
        w.commit().unwrap();
        if with_blob {
            s.blob_index(1, &id(blob), 1, 0).unwrap();
        }
    }

    fn under_mutant<R>(f: impl FnOnce() -> R) -> R {
        crate::differential::under_env_index_mutant(f)
    }

    /// A blob doc in space `ws`: version 1 holds blob 1, version 2 (current) blob 2.
    fn blob_doc(s: &DocStore) -> (DocId, u64, BlobId, BlobId) {
        use crate::store::{BlobReason, BlobVersion, DocKey, DocMeta};
        let k = s.blob_space_or_create("ws", [7; 32]).unwrap();
        let (a, b) = (id(1), id(2));
        s.blob_index(k.no, &a, 3, 1).unwrap();
        s.blob_index(k.no, &b, 3, 1).unwrap();
        let meta = DocMeta {
            key: DocKey {
                space: "ws".into(),
                path: "p".into(),
            },
            pack: crate::store::BLOB_DOC_PACK.into(),
            engine: crate::engine::NO_ENGINE.to_owned(),
            engine_format: 0,
            codec: Vec::new(),
            created_ms: 1,
            synced_hash: None,
            synced_seq: 0,
            declined: None,
        };
        let v = BlobVersion::new(a, 3, 1, "kit:import", "import", "disk", BlobReason::Import);
        let (doc, _) = s.create_blob_doc(&meta, &v, None).unwrap();
        let v2 = BlobVersion::new(b, 3, 2, "a", "a", "cli", BlobReason::Write);
        s.commit_blob_versions(doc, "ws", 1, &[v2], None).unwrap();
        (doc, k.no, a, b)
    }

    /// An overlay row, written directly.
    fn put_keys_row(s: &DocStore, doc: DocId, vid: VersionId, bytes: &[u8]) {
        let w = s.begin_write().unwrap();
        w.open_table(BLOB_STRUCT_KEYS)
            .unwrap()
            .insert((doc, vid), bytes)
            .unwrap();
        w.commit().unwrap();
    }

    /// Storing a read's minted keys re-checks the version: a purge that won the race leaves
    /// no overlay behind, and keys already there stay.
    #[test]
    fn storing_keys_rechecks_the_version_and_keeps_the_first() {
        under_mutant(|| {
            let (s, dir) = store("putkeys");
            let (doc, _, a, b) = blob_doc(&s);
            s.blob_purge_versions(doc, &[1], 3).unwrap();
            let got = s
                .blob_struct_put_keys(doc, 1, &a, "idx", 1, b"late")
                .unwrap();
            assert_eq!(got, None, "a purged version takes no overlay");
            assert!(s.blob_struct_keys(doc, 1).unwrap().is_none());
            // version 2 stays: the first write wins, under this format
            let first = enc_keys_row_fixture("idx", 1, "first");
            let second = enc_keys_row_fixture("idx", 1, "second");
            let got = s
                .blob_struct_put_keys(doc, 2, &b, "idx", 1, &first)
                .unwrap();
            assert_eq!(got.as_deref(), Some(first.as_slice()));
            let got = s
                .blob_struct_put_keys(doc, 2, &b, "idx", 1, &second)
                .unwrap();
            assert_eq!(
                got.as_deref(),
                Some(first.as_slice()),
                "keys minted once stay"
            );
            let newer = enc_keys_row_fixture("idx", 2, "newer-format");
            let got = s
                .blob_struct_put_keys(doc, 2, &b, "idx", 2, &newer)
                .unwrap();
            assert_eq!(
                got.as_deref(),
                Some(newer.as_slice()),
                "another format replaces"
            );
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    fn enc_keys_row_fixture(pack: &str, format: u32, key: &str) -> Vec<u8> {
        let mut k = crate::index::KeysRow::empty(pack, format);
        k.minted.insert(0, (key.to_owned(), false));
        crate::index::enc_keys_row(&k)
    }

    /// A blob stored again between the sweep's scan and its delete keeps its row.
    #[test]
    fn a_sweep_keeps_a_row_whose_blob_came_back() {
        under_mutant(|| {
            let (s, dir) = store("cameback");
            put_row(&s, 1, b"row", false);
            let found = s.blob_struct_orphans().unwrap();
            assert_eq!(found.len(), 1);
            s.blob_index(1, &id(1), 1, 0).unwrap();
            assert_eq!(
                s.blob_struct_remove_orphans(&found).unwrap(),
                0,
                "a row whose blob came back stays"
            );
            let r = s.begin_read().unwrap();
            let t = r.open_table(BLOB_STRUCT).unwrap();
            assert!(
                t.get((1u64, id(1).as_bytes().as_slice(), "idx", 1u32))
                    .unwrap()
                    .is_some(),
                "a row whose blob came back stays"
            );
            drop((t, r));
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// `delete_blob_version` erases a version: its overlay goes with it.
    #[test]
    fn deleting_a_version_removes_its_overlay() {
        under_mutant(|| {
            let (s, dir) = store("delete");
            let (doc, _, _, _) = blob_doc(&s);
            put_keys_row(&s, doc, 1, b"an-overlay-row");
            s.delete_blob_version(doc, 1, 3).unwrap();
            assert!(
                s.blob_struct_keys(doc, 1).unwrap().is_none(),
                "a deleted version's overlay is gone"
            );
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// The sweep with nothing to remove reads only: it runs while a writer holds the store.
    #[test]
    fn a_sweep_with_nothing_to_remove_waits_on_no_writer() {
        under_mutant(|| {
            let (s, dir) = store("nowriter");
            let s = std::sync::Arc::new(s);
            put_row(&s, 2, b"live", true);
            let w = s.begin_write().unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let (s2, planted) = (s.clone(), crate::differential::index_mutant());
            std::thread::spawn(move || {
                let run = || s2.blob_struct_sweep_orphans();
                let got = match planted {
                    Some(m) => crate::differential::with_index_mutant(m, run),
                    None => run(),
                };
                let _ = tx.send(got);
            });
            let got = rx.recv_timeout(std::time::Duration::from_secs(5));
            assert!(
                matches!(got, Ok(Ok(0))),
                "the sweep waited on a writer: {got:?}"
            );
            drop(w);
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// A row a kit that doesn't know the table left behind doesn't survive a rewrite, and a
    /// row with its blob does.
    #[test]
    fn a_rewrite_drops_a_row_whose_blob_is_gone() {
        under_mutant(|| {
            let (s, dir) = store("orphan");
            put_row(&s, 1, b"orphan-row-bytes-1", false);
            put_row(&s, 2, b"keep-this-other-content", true);
            s.rewrite_file().unwrap();
            let r = s.begin_read().unwrap();
            let t = r.open_table(BLOB_STRUCT).unwrap();
            assert!(
                t.get((1u64, id(1).as_bytes().as_slice(), "idx", 1u32))
                    .unwrap()
                    .is_none(),
                "an orphan row is not copied into the new file"
            );
            assert!(t
                .get((1u64, id(2).as_bytes().as_slice(), "idx", 1u32))
                .unwrap()
                .is_some());
            drop((t, r));
            assert!(
                s.scan_for(&[b"orphan-row-bytes-1".as_slice()])
                    .unwrap()
                    .is_empty(),
                "its bytes left the file"
            );
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// Removing a blob (a purge's or GC's `blob_remove_unreferenced`) takes its rows in the
    /// same transaction: no gap for a reader, no rewrite or sweep needed.
    #[test]
    fn removing_a_blob_takes_its_rows_in_its_transaction() {
        under_mutant(|| {
            let (s, dir) = store("hook");
            put_row(&s, 1, b"row-to-remove", true);
            let gone = s.blob_remove_unreferenced(&[(1, id(1), 1)]).unwrap();
            assert_eq!(gone.len(), 1, "the blob row was removed");
            let r = s.begin_read().unwrap();
            let t = r.open_table(BLOB_STRUCT).unwrap();
            assert!(
                t.get((1u64, id(1).as_bytes().as_slice(), "idx", 1u32))
                    .unwrap()
                    .is_none(),
                "the row goes in the blob's transaction"
            );
            drop((t, r));
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    #[test]
    fn gc_sweeps_rows_with_no_blob() {
        let (s, dir) = store("sweep");
        put_row(&s, 1, b"orphan", false);
        put_row(&s, 2, b"live", true);
        assert_eq!(s.blob_struct_sweep_orphans().unwrap(), 1);
        assert_eq!(s.blob_struct_sweep_orphans().unwrap(), 0);
        let r = s.begin_read().unwrap();
        let t = r.open_table(BLOB_STRUCT).unwrap();
        assert!(t
            .get((1u64, id(2).as_bytes().as_slice(), "idx", 1u32))
            .unwrap()
            .is_some());
        drop((t, r));
        let _ = std::fs::remove_dir_all(dir);
    }
}
