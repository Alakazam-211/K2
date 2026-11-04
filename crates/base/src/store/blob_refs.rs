//! The blobs a doc's values name (`docs/SESSIONS.md`, "Blob references"): a pack declares the
//! roots ([`crate::session::Pack::blob_ref_roots`]), and a value there that is exactly a
//! reference ([`BlobId::parse_ref`]) gets a row here and **one count on the blob's `blobs.refs`,
//! per doc and blob**, in the transaction of the change that names it.
//!
//! - **Why the count.** [`crate::blob::BlobStore::gc`] collects on `refs == 0`, in this kit and
//!   every older one, so a reference taken here holds against an older kit's GC too. The table
//!   only says which docs hold which counts.
//! - **Held while any record names it.** The live doc (hidden elements included), a version or a
//!   revert record. Nothing is released as a value is overwritten or a record leaves retention:
//!   only a purge ([`recompute_in`], from what's left) and a doc delete release, so a replaced
//!   blob stays until one of them runs (a known growth).
//! - **A name with nothing behind it.** A connection's edit is refused
//!   ([`StoreError::BlobRefUnknown`]); an import, an outside save, an undo or a restore isn't.
//!   Its row has space number 0 (space numbers start at 1), which means no count was taken, and
//!   reads report it [`BlobRefState::Missing`]. If the blob arrives, the next change, load or
//!   purge that finds it there takes the count, and so does the removal that would otherwise
//!   take the blob ([`claim_named`]): a name on record is never collected from under the doc.
//! - **Space.** The row keeps the number of the space the count was taken in, since a doc can be
//!   moved to another space and a space's key can be destroyed.
//!
//! **Created lazily**, by the first name (no format bump). An older kit reads and writes a store
//! holding the table as before and takes no references for its own writes; it refuses a rewrite,
//! and so a purge ([`StoreError::NewerData`]), and its doc delete leaves the rows and their
//! counts, which every open here releases ([`release_orphan`], the store's orphan sweep).

use std::collections::{BTreeSet, HashSet};

use redb::{ReadableTable, TableDefinition, TableHandle, WriteTransaction};

use super::blob::{adjust_ref, dec_row, space_in, PurgedBlob, BLOBS};
use super::enc::{R, W};
use super::history::{dec_record, has_table, NewChange, REVERTS};
use super::structured::{ElemOpKind, ElemRoot};
use super::{dec_meta, dec_version_full, DocId, DocStore, StoreError, VersionId, DOCS, VERSIONS};
use crate::blob::{BlobId, BlobRef, BlobRefState, BlobRefs};
use crate::doc::Neutral;
use crate::elem::Value;

/// `(doc, blob hex)` → the space number its count was taken in (0: none taken).
pub(super) const DOC_BLOB_REFS: TableDefinition<(u64, &str), &[u8]> =
    TableDefinition::new("doc_blob_refs");

/// Which of a change's values can name a blob, and whether a name with nothing behind it refuses
/// the change.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BlobRefScan {
    /// [`crate::session::Pack::blob_ref_roots`].
    pub roots: &'static [&'static str],
    /// A connection's edit: a value it writes must name a blob the doc's space holds, checked
    /// here too because blob GC runs outside the session's lock.
    pub strict: bool,
}

impl BlobRefScan {
    pub(crate) fn of(roots: &'static [&'static str]) -> Self {
        Self {
            roots,
            strict: false,
        }
    }
}

pub(super) fn enc_ref(no: u64) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1).u64(no);
    w.0
}

pub(super) fn dec_ref(b: &[u8]) -> Result<u64, StoreError> {
    let mut r = R::new(b, "doc blob ref");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("doc blob ref version"));
    }
    let no = r.u64()?;
    r.end()?;
    Ok(no)
}

fn named_by(v: &Value) -> Option<BlobId> {
    match v {
        Value::Str(s) => BlobId::parse_ref(s),
        Value::Null => None,
    }
}

/// The blobs `content`'s declared roots name.
fn names_in_content(content: &[(String, Neutral)], roots: &[&str], out: &mut BTreeSet<BlobId>) {
    for (root, n) in content {
        if !roots.contains(&root.as_str()) {
            continue;
        }
        match n {
            Neutral::Text(_) => {}
            Neutral::List(items) => out.extend(items.iter().filter_map(|(_, v)| named_by(v))),
            Neutral::Map(m) => out.extend(m.values().filter_map(named_by)),
        }
    }
}

/// The blobs a change's records name in the declared roots (`all`), and those it writes as new
/// values (`written`): a connection's edit may overwrite or delete a name with nothing behind
/// it, but not write one.
fn names_in_elems(elems: &[ElemRoot], roots: &[&str]) -> (BTreeSet<BlobId>, BTreeSet<BlobId>) {
    let (mut all, mut written) = (BTreeSet::new(), BTreeSet::new());
    for root in elems.iter().filter(|r| roots.contains(&r.root.as_str())) {
        for o in &root.ops {
            all.extend(o.value_before.as_ref().and_then(named_by));
            let after = o.value_after.as_ref().and_then(named_by);
            all.extend(after);
            if matches!(
                o.kind,
                ElemOpKind::Insert | ElemOpKind::Set | ElemOpKind::MapSet
            ) {
                written.extend(after);
            }
        }
    }
    (all, written)
}

/// Whether the index holds `(no, id)`.
fn indexed(w: &WriteTransaction, no: u64, id: &BlobId) -> Result<bool, StoreError> {
    Ok(w.open_table(BLOBS)?
        .get((no, id.as_bytes().as_slice()))?
        .is_some())
}

/// Take the doc's reference on each of `names` it doesn't hold yet, in `w`. A name in `written`
/// with nothing behind it refuses the transaction; any other gets a row with no count.
fn take_in(
    w: &WriteTransaction,
    id: DocId,
    names: &BTreeSet<BlobId>,
    written: &BTreeSet<BlobId>,
    now: u64,
) -> Result<(), StoreError> {
    if names.is_empty() {
        return Ok(());
    }
    let space = {
        let docs = w.open_table(DOCS)?;
        let m = docs.get(id)?.ok_or(StoreError::NoDoc(id))?;
        dec_meta(m.value())?.key.space
    };
    let space = match space_in(w, &space) {
        Ok(k) => Some(k.no),
        Err(StoreError::NoBlobSpace) => None,
        Err(e) => return Err(e),
    };
    let mut t = w.open_table(DOC_BLOB_REFS)?;
    for b in names {
        let hex = b.to_hex();
        let found = match space {
            Some(no) if indexed(w, no, b)? => Some(no),
            _ => None,
        };
        if found.is_none() && written.contains(b) {
            return Err(StoreError::BlobRefUnknown(hex));
        }
        let row = t.get((id, hex.as_str()))?.map(|v| dec_ref(v.value()));
        match (row.transpose()?, found) {
            // the count is taken, or there is still nothing to count on
            (Some(no), _) if no != 0 => {}
            (Some(_), None) => {}
            (None, None) => {
                t.insert((id, hex.as_str()), enc_ref(0).as_slice())?;
            }
            (_, Some(no)) => {
                adjust_ref(w, no, b, 1, now)?;
                t.insert((id, hex.as_str()), enc_ref(no).as_slice())?;
            }
        }
    }
    Ok(())
}

/// Blobs about to be removed as unreferenced (`rows`, `(space number, id, size)`), in the removing
/// transaction `w`: any of them a row names with no count, in that row's doc's own space, takes
/// the count now, so the removal leaves it. One read of the table per removal with something to
/// remove, and a doc lookup per match.
pub(super) fn claim_named(
    w: &WriteTransaction,
    rows: &[(u64, BlobId, u64)],
) -> Result<(), StoreError> {
    if rows.is_empty() || planted_gc_ignores_names() || !has_table(w, DOC_BLOB_REFS.name())? {
        return Ok(());
    }
    let due: HashSet<(u64, BlobId)> = rows.iter().map(|(no, id, _)| (*no, *id)).collect();
    let ids: HashSet<BlobId> = rows.iter().map(|r| r.1).collect();
    let mut waiting = Vec::new();
    for kv in w.open_table(DOC_BLOB_REFS)?.iter()? {
        let (k, v) = kv?;
        if dec_ref(v.value())? != 0 {
            continue;
        }
        let (doc, hex) = k.value();
        let b = BlobId::from_hex(hex).ok_or(StoreError::Corrupt("doc blob ref id"))?;
        if ids.contains(&b) {
            waiting.push((doc, b));
        }
    }
    for (doc, b) in waiting {
        let space = {
            let docs = w.open_table(DOCS)?;
            let m = docs.get(doc)?;
            // a doc an older kit deleted: its rows wait for the next open's sweep
            m.map(|m| dec_meta(m.value())).transpose()?
        };
        let Some(meta) = space else { continue };
        let no = match space_in(w, &meta.key.space) {
            Ok(k) => k.no,
            Err(StoreError::NoBlobSpace) => continue,
            Err(e) => return Err(e),
        };
        if due.contains(&(no, b)) {
            adjust_ref(w, no, &b, 1, 0)?;
            w.open_table(DOC_BLOB_REFS)?
                .insert((doc, b.to_hex().as_str()), enc_ref(no).as_slice())?;
        }
    }
    Ok(())
}

/// The planted fault [`crate::differential::BlobRefMutant::GcIgnoresNames`].
#[cfg(any(test, feature = "differential"))]
fn planted_gc_ignores_names() -> bool {
    crate::differential::blob_ref_mutant()
        == Some(crate::differential::BlobRefMutant::GcIgnoresNames)
}

#[cfg(not(any(test, feature = "differential")))]
fn planted_gc_ignores_names() -> bool {
    false
}

/// The references change `c` takes, in `w`: the change's own transaction.
pub(super) fn take_for_change(
    w: &WriteTransaction,
    id: DocId,
    c: &NewChange,
) -> Result<(), StoreError> {
    if c.blob_refs.roots.is_empty() {
        return Ok(());
    }
    let (all, written) = names_in_elems(&c.record.elems, c.blob_refs.roots);
    let refused = if c.blob_refs.strict {
        written
    } else {
        BTreeSet::new()
    };
    take_in(w, id, &all, &refused, c.info.at_ms)
}

/// Give one count back. A row of a blob gone from the index (its space was purged), or one
/// with no count left to give, has nothing to release and stops nothing. `now` is when it lost
/// its last reference, `None` keeping the time it has.
fn release(w: &WriteTransaction, no: u64, id: &BlobId, now: Option<u64>) -> Result<(), StoreError> {
    if no == 0 {
        return Ok(());
    }
    let row = {
        let t = w.open_table(BLOBS)?;
        let v = t.get((no, id.as_bytes().as_slice()))?;
        v.map(|v| dec_row(v.value())).transpose()?
    };
    match row {
        Some(r) if r.refs > 0 => adjust_ref(w, no, id, -1, now.unwrap_or(r.since_ms)),
        _ => Ok(()),
    }
}

/// Every row of doc `id`, as `(blob, space number)`.
fn rows_of(w: &WriteTransaction, id: DocId) -> Result<Vec<(BlobId, u64)>, StoreError> {
    let t = w.open_table(DOC_BLOB_REFS)?;
    let mut out = Vec::new();
    for kv in t.range((id, "")..(id + 1, ""))? {
        let (k, v) = kv?;
        let b = BlobId::from_hex(k.value().1).ok_or(StoreError::Corrupt("doc blob ref id"))?;
        out.push((b, dec_ref(v.value())?));
    }
    Ok(out)
}

/// A doc delete: every count doc `id` holds goes back at `now`, and its rows go.
pub(super) fn delete_rows(w: &WriteTransaction, id: DocId, now: u64) -> Result<(), StoreError> {
    if !has_table(w, DOC_BLOB_REFS.name())? {
        return Ok(());
    }
    for (b, no) in rows_of(w, id)? {
        release(w, no, &b, Some(now))?;
    }
    w.open_table(DOC_BLOB_REFS)?
        .retain_in((id, "")..(id + 1, ""), |_, _| false)?;
    Ok(())
}

/// The orphan sweep's hook: a doc an older kit deleted left its rows and their counts. They go
/// back keeping each blob's time: nothing names it, and an open has no host clock.
pub(super) fn release_orphan(w: &WriteTransaction, id: DocId) -> Result<(), StoreError> {
    for (b, no) in rows_of(w, id)? {
        release(w, no, &b, None)?;
    }
    Ok(())
}

/// A purge: recompute the doc's references from what's left in `w` (its versions, its revert
/// records and `live`), so a record an older kit wrote counts as any other. A name nothing
/// holds yet is taken; a row nothing names any more goes, with its count at `now`. Returns the
/// blobs released, with the references each still has.
pub(super) fn recompute_in(
    w: &WriteTransaction,
    id: DocId,
    roots: &[&str],
    live: &BTreeSet<BlobId>,
    now: u64,
) -> Result<Vec<PurgedBlob>, StoreError> {
    if roots.is_empty() {
        return Ok(Vec::new());
    }
    let mut named = live.clone();
    for kv in w.open_table(VERSIONS)?.range((id, 0)..=(id, u64::MAX))? {
        let (v, _) = dec_version_full(kv?.1.value())?;
        names_in_content(&v.content, roots, &mut named);
    }
    for kv in w.open_table(REVERTS)?.range((id, 0)..=(id, u64::MAX))? {
        let rec = dec_record(kv?.1.value())?;
        named.extend(names_in_elems(&rec.elems, roots).0);
    }
    take_in(w, id, &named, &BTreeSet::new(), now)?;
    if !has_table(w, DOC_BLOB_REFS.name())? {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (b, no) in rows_of(w, id)? {
        if named.contains(&b) {
            continue;
        }
        release(w, no, &b, Some(now))?;
        w.open_table(DOC_BLOB_REFS)?
            .remove((id, b.to_hex().as_str()))?;
        if no == 0 {
            continue;
        }
        let row = {
            let t = w.open_table(BLOBS)?;
            let v = t.get((no, b.as_bytes().as_slice()))?;
            v.map(|v| dec_row(v.value())).transpose()?
        };
        if let Some(r) = row {
            out.push((no, b, r.size, r.refs));
        }
    }
    Ok(out)
}

impl DocStore {
    /// Whether `no` (or, for 0, the doc's space now) holds blob `b`.
    fn ref_state(
        r: &redb::ReadTransaction,
        no: Option<u64>,
        b: &BlobId,
    ) -> Result<BlobRefState, StoreError> {
        let Some(no) = no else {
            return Ok(BlobRefState::Missing);
        };
        let held = r
            .open_table(BLOBS)?
            .get((no, b.as_bytes().as_slice()))?
            .is_some();
        Ok(if held {
            BlobRefState::Present
        } else {
            BlobRefState::Missing
        })
    }

    /// The number of doc `id`'s space now, if it has a blob key.
    fn space_no_of(r: &redb::ReadTransaction, id: DocId) -> Result<Option<u64>, StoreError> {
        let space = {
            let docs = r.open_table(DOCS)?;
            let m = docs.get(id)?.ok_or(StoreError::NoDoc(id))?;
            dec_meta(m.value())?.key.space
        };
        super::blob::space_no(r, &space)
    }

    /// The blobs doc `id` holds references on, or names with nothing behind them, each with its
    /// state now, in id order. Every name on record, whoever asks: this is the host's. A session
    /// holds it to the caller's rights ([`crate::session::Session::blob_refs`]).
    pub fn blob_refs(&self, id: DocId) -> Result<BlobRefs, StoreError> {
        let r = self.begin_read()?;
        let space = Self::space_no_of(&r, id)?;
        let t = match r.open_table(DOC_BLOB_REFS) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(BlobRefs::default()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for kv in t.range((id, "")..(id + 1, ""))? {
            let (k, v) = kv?;
            let blob =
                BlobId::from_hex(k.value().1).ok_or(StoreError::Corrupt("doc blob ref id"))?;
            let no = Some(dec_ref(v.value())?).filter(|n| *n != 0).or(space);
            out.push(BlobRef {
                blob,
                state: Self::ref_state(&r, no, &blob)?,
            });
        }
        Ok(out.into_iter().collect())
    }

    /// The blobs version `vid` of doc `id` names in `roots`
    /// ([`crate::session::Pack::blob_ref_roots`]), each with its state now, in id order. A
    /// purged value names nothing.
    pub fn blob_refs_at(
        &self,
        id: DocId,
        vid: VersionId,
        roots: &[&str],
    ) -> Result<BlobRefs, StoreError> {
        let content = self.version(id, vid)?.content;
        self.blob_refs_in_content(id, &content, roots)
    }

    /// The blobs `content`'s `roots` name, each with its state now, in id order.
    pub(crate) fn blob_refs_in_content(
        &self,
        id: DocId,
        content: &[(String, Neutral)],
        roots: &[&str],
    ) -> Result<BlobRefs, StoreError> {
        let mut names = BTreeSet::new();
        names_in_content(content, roots, &mut names);
        let r = self.begin_read()?;
        let space = Self::space_no_of(&r, id)?;
        let t = match r.open_table(DOC_BLOB_REFS) {
            Ok(t) => Some(t),
            Err(redb::TableError::TableDoesNotExist(_)) => None,
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::with_capacity(names.len());
        for blob in names {
            let row = match &t {
                Some(t) => {
                    let v = t.get((id, blob.to_hex().as_str()))?;
                    v.map(|v| dec_ref(v.value())).transpose()?
                }
                None => None,
            };
            let no = row.filter(|n| *n != 0).or(space);
            out.push(BlobRef {
                blob,
                state: Self::ref_state(&r, no, &blob)?,
            });
        }
        Ok(out.into_iter().collect())
    }

    /// A session's load: the live doc names `live`, and an older kit may have written some of
    /// them with no reference, or the blob behind a name may have arrived since. Takes what's
    /// missing and returns how many names needed it. Writes nothing when none do.
    pub(crate) fn heal_blob_refs(
        &self,
        id: DocId,
        live: &BTreeSet<BlobId>,
        now: u64,
    ) -> Result<u64, StoreError> {
        if live.is_empty() {
            return Ok(0);
        }
        let mut need = BTreeSet::new();
        {
            let r = self.begin_read()?;
            let space = Self::space_no_of(&r, id)?;
            let t = match r.open_table(DOC_BLOB_REFS) {
                Ok(t) => Some(t),
                Err(redb::TableError::TableDoesNotExist(_)) => None,
                Err(e) => return Err(e.into()),
            };
            for b in live {
                let row = match &t {
                    Some(t) => {
                        let v = t.get((id, b.to_hex().as_str()))?;
                        v.map(|v| dec_ref(v.value())).transpose()?
                    }
                    None => None,
                };
                let wanted = match row {
                    None => true,
                    Some(0) => Self::ref_state(&r, space, b)? == BlobRefState::Present,
                    Some(_) => false,
                };
                if wanted {
                    need.insert(*b);
                }
            }
        }
        if need.is_empty() {
            return Ok(0);
        }
        let w = self.begin_write()?;
        take_in(&w, id, &need, &BTreeSet::new(), now)?;
        w.commit()?;
        Ok(need.len() as u64)
    }
}
