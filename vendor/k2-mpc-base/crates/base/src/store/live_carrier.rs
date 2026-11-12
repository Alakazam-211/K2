//! A live doc's **carrier** (K1, `docs/SESSIONS.md`, "The carrier (K1)"): for a pack that opts
//! in (`Pack::wants_carrier`), the bytes the kit last read from the doc's file or wrote to it,
//! kept as a blob in the doc's space, and **each version's carrier**, the file's bytes as of
//! that version, so a version's file is that version spliced into them.
//!
//! Two additive tables, **without a format bump** (the terms of `client_kinds`,
//! `doc_blob_refs` and `live_copies`), created lazily:
//! - `live_carriers`: `doc` → the live carrier's blob (space number, id, size);
//! - `version_carriers`: `(doc, version)` → a version's carrier's blob, and the version's seq.
//!
//! Each row holds **one count** on its blob's `blobs.refs`, taken and given back in the
//! transaction that changes the row, so blob GC (this kit's and every older one's) keeps a
//! carrier while a row names it. Many rows may name one blob.
//!
//! - **A version takes its carrier when it is cut** ([`carry_new_versions`], in the transaction
//!   that stores it): the live carrier then. When the live carrier becomes the file's bytes at
//!   a sync point (`seq`), every version at that seq takes them ([`DocStore::set_live_carrier`]):
//!   the file then is exactly that version's file. The doc's first carrier also goes to the
//!   versions at its seq that have none (the import's).
//! - **A version that goes gives its count back:** retention, a purge (the session drops the
//!   carriers of the versions it erased, and removes their blobs at once), a doc delete.
//!   Rows whose version is gone (an older kit's retention) are swept at the next load.
//!
//! An older kit reads and writes a store with these tables as before: its blob GC keeps every
//! carrier; its own writes leave the live row naming bytes the file no longer holds (this kit
//! checks the live carrier against the sync point when it loads it and drops a stale one); its
//! retention leaves version rows, swept here; it refuses a rewrite and so a purge naming a table
//! (`StoreError::NewerData`, `MPC-E502`); its doc delete leaves the rows and their counts, which
//! every open here gives back (the orphan sweep). A version it cuts has no carrier.

use std::collections::BTreeSet;

use redb::{ReadableTable, TableDefinition, TableHandle, WriteTransaction};

use super::blob::{adjust_ref, dec_row as dec_blob_row, space_in, BLOBS};
use super::enc::{R, W};
use super::history::{dec_version_head, has_table};
use super::{DocId, DocStore, Seq, StoreError, VersionId, VERSIONS};
use crate::blob::BlobId;

/// `doc` → [`CarrierRow`] (record version 1).
pub(super) const LIVE_CARRIERS: TableDefinition<u64, &[u8]> = TableDefinition::new("live_carriers");

/// `(doc, version)` → [`CarrierRow`] and the version's seq (record version 1).
pub(super) const VERSION_CARRIERS: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("version_carriers");

/// A carrier's blob, by its space's key number (a doc can move to another space; the count stays
/// where it was taken).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CarrierRow {
    pub no: u64,
    pub id: BlobId,
    pub size: u64,
}

/// A blob with no reference left, to remove at once: `(space no, id, size)`.
pub(crate) type Released = (u64, BlobId, u64);

fn enc_row(r: &CarrierRow) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1).u64(r.no).bytes(r.id.as_bytes()).u64(r.size);
    w.0
}

/// `None` for a record this kit can't read (another version): read as no carrier.
fn dec_row(b: &[u8]) -> Option<CarrierRow> {
    let mut r = R::new(b, "live carrier");
    if r.u8().ok()? != 1 {
        return None;
    }
    let row = CarrierRow {
        no: r.u64().ok()?,
        id: BlobId::from_slice(&r.bytes().ok()?)?,
        size: r.u64().ok()?,
    };
    r.end().ok()?;
    Some(row)
}

fn enc_vrow(r: &CarrierRow, seq: Seq) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1)
        .u64(r.no)
        .bytes(r.id.as_bytes())
        .u64(r.size)
        .u64(seq);
    w.0
}

fn dec_vrow(b: &[u8]) -> Option<(CarrierRow, Seq)> {
    let mut r = R::new(b, "version carrier");
    if r.u8().ok()? != 1 {
        return None;
    }
    let row = CarrierRow {
        no: r.u64().ok()?,
        id: BlobId::from_slice(&r.bytes().ok()?)?,
        size: r.u64().ok()?,
    };
    let seq = r.u64().ok()?;
    r.end().ok()?;
    Some((row, seq))
}

/// Give back one count `row` holds, in `w`. A blob gone from the index (its space's key was
/// destroyed) or with no count left has nothing to give back.
fn release(w: &WriteTransaction, row: &CarrierRow, now: Option<u64>) -> Result<(), StoreError> {
    let blob = {
        let t = w.open_table(BLOBS)?;
        let v = t.get((row.no, row.id.as_bytes().as_slice()))?;
        v.map(|v| dec_blob_row(v.value())).transpose()?
    };
    match blob {
        Some(b) if b.refs > 0 => adjust_ref(w, row.no, &row.id, -1, now.unwrap_or(b.since_ms)),
        _ => Ok(()),
    }
}

/// Of `rows`, the blobs nothing references any more (in `w`, before its commit).
fn unreferenced(
    w: &WriteTransaction,
    rows: impl IntoIterator<Item = CarrierRow>,
) -> Result<Vec<Released>, StoreError> {
    let t = w.open_table(BLOBS)?;
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for r in rows {
        if !seen.insert((r.no, r.id)) {
            continue;
        }
        if let Some(b) = t.get((r.no, r.id.as_bytes().as_slice()))? {
            let b = dec_blob_row(b.value())?;
            if b.refs == 0 {
                out.push((r.no, r.id, b.size));
            }
        }
    }
    Ok(out)
}

fn row_in(w: &WriteTransaction, id: DocId) -> Result<Option<CarrierRow>, StoreError> {
    if !has_table(w, LIVE_CARRIERS.name())? {
        return Ok(None);
    }
    let t = w.open_table(LIVE_CARRIERS)?;
    let v = t.get(id)?;
    Ok(v.and_then(|v| dec_row(v.value())))
}

/// Doc `id`'s version rows (readable ones), in `w`.
fn vrows_in(
    w: &WriteTransaction,
    id: DocId,
) -> Result<Vec<(VersionId, CarrierRow, Seq)>, StoreError> {
    if !has_table(w, VERSION_CARRIERS.name())? {
        return Ok(Vec::new());
    }
    let t = w.open_table(VERSION_CARRIERS)?;
    let mut out = Vec::new();
    for kv in t.range((id, 0)..=(id, u64::MAX))? {
        let (k, v) = kv?;
        if let Some((row, seq)) = dec_vrow(v.value()) {
            out.push((k.value().1, row, seq));
        }
    }
    Ok(out)
}

/// The versions just stored (`(id, seq)`, in `w`) take the live carrier, if the doc has one. Run
/// by every transaction that stores a doc's versions.
pub(super) fn carry_new_versions(
    w: &WriteTransaction,
    id: DocId,
    new: &[(VersionId, Seq)],
) -> Result<(), StoreError> {
    if new.is_empty() {
        return Ok(());
    }
    let Some(live) = row_in(w, id)? else {
        return Ok(());
    };
    let mut t = w.open_table(VERSION_CARRIERS)?;
    for (vid, seq) in new {
        adjust_ref(w, live.no, &live.id, 1, 0)?;
        t.insert((id, *vid), enc_vrow(&live, *seq).as_slice())?;
    }
    Ok(())
}

/// The versions `vids` of doc `id` go (retention, a purge, a sweep), in `w`: their carriers give
/// their counts back at `now` (`None`: keeping the blob's time). Returns the rows dropped.
pub(super) fn drop_versions_in(
    w: &WriteTransaction,
    id: DocId,
    vids: &[VersionId],
    now: Option<u64>,
) -> Result<Vec<CarrierRow>, StoreError> {
    if vids.is_empty() || !has_table(w, VERSION_CARRIERS.name())? {
        return Ok(Vec::new());
    }
    let mut gone = Vec::new();
    for vid in vids {
        let row = {
            let t = w.open_table(VERSION_CARRIERS)?;
            let v = t.get((id, *vid))?;
            v.map(|v| v.value().to_vec())
        };
        let Some(b) = row else { continue };
        if let Some((row, _)) = dec_vrow(&b) {
            release(w, &row, now)?;
            gone.push(row);
        }
        w.open_table(VERSION_CARRIERS)?.remove((id, *vid))?;
    }
    Ok(gone)
}

/// A doc delete (in its transaction `w`): every carrier's count goes back at `now`, and the rows.
pub(super) fn delete_rows(w: &WriteTransaction, id: DocId, now: u64) -> Result<(), StoreError> {
    if let Some(row) = row_in(w, id)? {
        release(w, &row, Some(now))?;
    }
    if has_table(w, LIVE_CARRIERS.name())? {
        w.open_table(LIVE_CARRIERS)?.remove(id)?;
    }
    for (_, row, _) in vrows_in(w, id)? {
        release(w, &row, Some(now))?;
    }
    if has_table(w, VERSION_CARRIERS.name())? {
        w.open_table(VERSION_CARRIERS)?
            .retain_in((id, 0)..=(id, u64::MAX), |_, _| false)?;
    }
    Ok(())
}

/// The orphan sweep's hook for `live_carriers`: a doc an older kit deleted left its row and its
/// count. The count goes back keeping the blob's time (an open has no host clock).
pub(super) fn release_orphan(w: &WriteTransaction, id: DocId) -> Result<(), StoreError> {
    if let Some(row) = row_in(w, id)? {
        release(w, &row, None)?;
    }
    Ok(())
}

/// The orphan sweep's hook for `version_carriers`, as [`release_orphan`].
pub(super) fn release_orphan_versions(w: &WriteTransaction, id: DocId) -> Result<(), StoreError> {
    for (_, row, _) in vrows_in(w, id)? {
        release(w, &row, None)?;
    }
    Ok(())
}

impl DocStore {
    /// Doc `id`'s carrier, if it has one this kit can read.
    pub(crate) fn live_carrier(&self, id: DocId) -> Result<Option<CarrierRow>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(LIVE_CARRIERS) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let v = t.get(id)?;
        Ok(v.and_then(|v| dec_row(v.value())))
    }

    /// Version `vid`'s carrier, if it has one this kit can read.
    pub(crate) fn version_carrier(
        &self,
        id: DocId,
        vid: VersionId,
    ) -> Result<Option<CarrierRow>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(VERSION_CARRIERS) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let v = t.get((id, vid))?;
        Ok(v.and_then(|v| dec_vrow(v.value())).map(|(r, _)| r))
    }

    /// Every version carrier of doc `id`, by version.
    pub(crate) fn version_carriers(
        &self,
        id: DocId,
    ) -> Result<Vec<(VersionId, CarrierRow)>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(VERSION_CARRIERS) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for kv in t.range((id, 0)..=(id, u64::MAX))? {
            let (k, v) = kv?;
            if let Some((row, _)) = dec_vrow(v.value()) {
                out.push((k.value().1, row));
            }
        }
        Ok(out)
    }

    /// Make the blob `id` holds in `space` doc `doc`'s carrier, the file's bytes at sync point
    /// `seq` (`None`: it has none), in one durable transaction: the live row takes a count on the
    /// new blob and gives the old one's back at `now`; every version at `seq` takes the new blob
    /// too (the file then is that version's file), and on the doc's first carrier so do the
    /// versions at `seq` that have none. Returns the blobs that now have no reference, for the
    /// caller to remove at once ([`crate::blob::BlobStore`]'s removal).
    pub(crate) fn set_live_carrier(
        &self,
        doc: DocId,
        new: Option<(&str, BlobId, u64)>,
        seq: Seq,
        now: u64,
    ) -> Result<Vec<Released>, StoreError> {
        let w = self.begin_write()?;
        let old = row_in(&w, doc)?;
        let new = match new {
            Some((space, id, size)) => Some(CarrierRow {
                no: space_in(&w, space)?.no,
                id,
                size,
            }),
            None => None,
        };
        let mut released = Vec::new();
        if old != new {
            if let Some(n) = &new {
                adjust_ref(&w, n.no, &n.id, 1, now)?;
            }
            if let Some(o) = &old {
                release(&w, o, Some(now))?;
                released.push(*o);
            }
            match &new {
                Some(n) => {
                    w.open_table(LIVE_CARRIERS)?
                        .insert(doc, enc_row(n).as_slice())?;
                }
                None => {
                    if has_table(&w, LIVE_CARRIERS.name())? {
                        w.open_table(LIVE_CARRIERS)?.remove(doc)?;
                    }
                }
            }
        }
        if let Some(n) = &new {
            let mut have = BTreeSet::new();
            for (vid, row, at) in vrows_in(&w, doc)? {
                have.insert(vid);
                if at != seq || row == *n {
                    continue;
                }
                adjust_ref(&w, n.no, &n.id, 1, now)?;
                release(&w, &row, Some(now))?;
                released.push(row);
                w.open_table(VERSION_CARRIERS)?
                    .insert((doc, vid), enc_vrow(n, seq).as_slice())?;
            }
            if old.is_none() {
                let heads: Vec<(VersionId, Seq)> = {
                    let t = w.open_table(VERSIONS)?;
                    let mut out = Vec::new();
                    for kv in t.range((doc, 0)..=(doc, u64::MAX))? {
                        let (k, v) = kv?;
                        let h = dec_version_head(k.value().1, v.value())?;
                        if h.seq == seq && !have.contains(&h.id) {
                            out.push((h.id, h.seq));
                        }
                    }
                    out
                };
                let mut t = w.open_table(VERSION_CARRIERS)?;
                for (vid, at) in heads {
                    adjust_ref(&w, n.no, &n.id, 1, now)?;
                    t.insert((doc, vid), enc_vrow(n, at).as_slice())?;
                }
            }
        }
        let gone = unreferenced(&w, released)?;
        w.commit()?;
        Ok(gone)
    }

    /// Drop the carriers of doc `doc`'s versions `vids` (a purge erased them), in one durable
    /// transaction: their counts go back at `now`. Returns the blobs that now have no reference.
    pub(crate) fn drop_version_carriers(
        &self,
        doc: DocId,
        vids: &[VersionId],
        now: u64,
    ) -> Result<Vec<Released>, StoreError> {
        if vids.is_empty() {
            return Ok(Vec::new());
        }
        let w = self.begin_write()?;
        let gone = drop_versions_in(&w, doc, vids, Some(now))?;
        let gone = unreferenced(&w, gone)?;
        w.commit()?;
        Ok(gone)
    }

    /// Give back the counts of version carriers whose version is gone (an older kit's retention
    /// left them), at `now`. Writes nothing when there are none.
    pub(crate) fn sweep_version_carriers(&self, doc: DocId, now: u64) -> Result<(), StoreError> {
        let orphans: Vec<VersionId> = {
            let r = self.begin_read()?;
            let t = match r.open_table(VERSION_CARRIERS) {
                Ok(t) => t,
                Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
                Err(e) => return Err(e.into()),
            };
            let versions = r.open_table(VERSIONS)?;
            let mut out = Vec::new();
            for kv in t.range((doc, 0)..=(doc, u64::MAX))? {
                let vid = kv?.0.value().1;
                if versions.get((doc, vid))?.is_none() {
                    out.push(vid);
                }
            }
            out
        };
        if orphans.is_empty() {
            return Ok(());
        }
        let w = self.begin_write()?;
        drop_versions_in(&w, doc, &orphans, Some(now))?;
        w.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_round_trip_and_another_version_is_unread() {
        let row = CarrierRow {
            no: 3,
            id: BlobId::from_slice(&[7u8; 32]).unwrap(),
            size: 1234,
        };
        assert_eq!(dec_row(&enc_row(&row)), Some(row));
        assert_eq!(dec_vrow(&enc_vrow(&row, 9)), Some((row, 9)));
        for mut other in [enc_row(&row), enc_vrow(&row, 9)] {
            other[0] = 2;
            assert_eq!(dec_row(&other), None);
            assert_eq!(dec_vrow(&other), None);
        }
        let mut long = enc_row(&row);
        long.push(0);
        assert_eq!(dec_row(&long), None, "a longer record is another layout");
        let mut long = enc_vrow(&row, 9);
        long.push(0);
        assert_eq!(dec_vrow(&long), None);
    }
}
