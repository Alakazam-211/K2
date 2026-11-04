//! The blobs a doc's values name, in sessions (`docs/SESSIONS.md`, "Blob references"). The store
//! takes the references ([`crate::store::BlobRefScan`], in each change's transaction); this is
//! the refusal for a connection's edit, the reads, the load's heal and the purge's removal.

use std::collections::BTreeSet;

use super::*;
use crate::blob::{BlobId, BlobRefs};
use crate::elem::{KeyedRoot, ListOp, MapOp, Value};
use crate::store::PurgeCounts;

/// The blobs `doc` names in `roots`, hidden elements included (a restore brings them back).
pub(super) fn live_names(doc: &Doc, roots: &[&str]) -> Result<BTreeSet<BlobId>> {
    let mut out = BTreeSet::new();
    if roots.is_empty() {
        return Ok(out);
    }
    let named = |v: &Value| match v {
        Value::Str(s) => BlobId::parse_ref(s),
        Value::Null => None,
    };
    for (root, r) in doc.keyed_state()?.roots {
        if !roots.contains(&root.as_str()) {
            continue;
        }
        match r {
            KeyedRoot::List { live, hidden, .. } => {
                out.extend(live.iter().filter_map(|(_, v)| named(v)));
                out.extend(hidden.values().filter_map(named));
            }
            KeyedRoot::Map(m) => out.extend(m.values().filter_map(named)),
            _ => {}
        }
    }
    Ok(out)
}

/// A load's heal: the store takes a reference for each blob the live doc names and holds none on
/// (an older kit wrote the name, or the blob arrived after it). Until this runs, such a blob has
/// no protection from GC. Writes nothing when there is nothing to take.
pub(super) fn heal_at_load(
    store: &DocStore,
    id: DocId,
    doc: &Doc,
    roots: &[&str],
    now: u64,
) -> Result<()> {
    if roots.is_empty() || planted_no_heal() {
        return Ok(());
    }
    store.heal_blob_refs(id, &live_names(doc, roots)?, now)?;
    Ok(())
}

/// The planted fault [`crate::differential::BlobRefMutant::NoHealAtLoad`].
#[cfg(any(test, feature = "differential"))]
fn planted_no_heal() -> bool {
    crate::differential::blob_ref_mutant() == Some(crate::differential::BlobRefMutant::NoHealAtLoad)
}

#[cfg(not(any(test, feature = "differential")))]
fn planted_no_heal() -> bool {
    false
}

/// The planted fault [`crate::differential::BlobRefMutant::StaleStrictFlag`].
#[cfg(any(test, feature = "differential"))]
pub(super) fn planted_stale_strict() -> bool {
    crate::differential::blob_ref_mutant()
        == Some(crate::differential::BlobRefMutant::StaleStrictFlag)
}

#[cfg(not(any(test, feature = "differential")))]
pub(super) fn planted_stale_strict() -> bool {
    false
}

/// The planted fault [`crate::differential::BlobRefMutant::RefsIgnoreRights`].
#[cfg(any(test, feature = "differential"))]
fn planted_refs_ignore_rights() -> bool {
    crate::differential::blob_ref_mutant()
        == Some(crate::differential::BlobRefMutant::RefsIgnoreRights)
}

#[cfg(not(any(test, feature = "differential")))]
fn planted_refs_ignore_rights() -> bool {
    false
}

impl Session {
    /// The blobs this doc names, each with whether its bytes are there now, and how many of those
    /// aren't ([`BlobRefs::missing`]). **How a host finds a missing blob:** a read shows the value
    /// as it is. A name with nothing behind it arrives through an import, an outside save, an undo
    /// or a restore; a connection's edit is refused. Needs read.
    ///
    /// **What it returns depends on the history right,** since a blob's id says the bytes were once
    /// in the doc. With it: every name on record, including those only a version, a revert record
    /// or a deleted element holds. Without it: only what [`ReadResult::content`] shows.
    pub fn blob_refs(&self, conn: ConnId) -> Result<BlobRefs> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Read, Action::Read, now)?;
        let store = self.store(st)?.clone();
        if st.members[i].rights.history || planted_refs_ignore_rights() {
            return Ok(store.blob_refs(self.doc_id)?);
        }
        let content = self.content(st)?;
        Ok(store.blob_refs_in_content(self.doc_id, &content, self.pack.blob_ref_roots())?)
    }

    /// The blobs version `vid` names, each with whether its bytes are there now, and how many
    /// aren't. Needs read and history.
    pub fn blob_refs_at(&self, conn: ConnId, vid: crate::store::VersionId) -> Result<BlobRefs> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::History, now)?;
        self.admit(&mut st, conn, Need::History, Action::History, now)?;
        self.store(&st)?
            .blob_refs_at(self.doc_id, vid, self.pack.blob_ref_roots())
            .map_err(|e| match e {
                crate::store::StoreError::NoVersion(..) => SessionError::NoSuchChange(vid),
                e => e.into(),
            })
    }

    /// A connection's edit writing a value that names a blob must name one this doc's space
    /// holds ([`SessionError::BlobRefUnknown`], exit 4). Checked here so nothing applies; the
    /// store checks again in the change's transaction, since blob GC runs outside this lock.
    pub(super) fn check_blob_refs(&self, st: &mut State, edits: &[Edit]) -> Result<()> {
        st.next_refs_strict = false;
        let roots = self.pack.blob_ref_roots();
        if roots.is_empty() {
            return Ok(());
        }
        let store = self.store(st)?;
        let space = store.blob_space(&st.key.space)?.map(|k| k.no);
        let mut named = false;
        for e in edits.iter().filter(|e| roots.contains(&e.root())) {
            let values: Vec<(&str, &Value)> = match e {
                Edit::Text { .. } => Vec::new(),
                Edit::List { ops, .. } => ops
                    .iter()
                    .filter_map(|o| match o {
                        ListOp::Insert { key, value, .. } | ListOp::Set { key, value } => {
                            Some((key.as_str(), value))
                        }
                        _ => None,
                    })
                    .collect(),
                Edit::Map { ops, .. } => ops
                    .iter()
                    .filter_map(|o| match o {
                        MapOp::Set { key, value } => Some((key.as_str(), value)),
                        MapOp::Delete { .. } => None,
                    })
                    .collect(),
            };
            for (key, value) in values {
                let Value::Str(s) = value else { continue };
                let Some(id) = BlobId::parse_ref(s) else {
                    continue;
                };
                named = true;
                let held = match space {
                    Some(no) => store.blob_row(no, &id)?.is_some(),
                    None => false,
                };
                if !held {
                    return Err(SessionError::BlobRefUnknown {
                        root: e.root().to_owned(),
                        key: key.to_owned(),
                        blob: id.to_hex(),
                    });
                }
            }
        }
        st.next_refs_strict = named;
        Ok(())
    }

    /// A purge's blobs: what no record of the doc names any more and nothing else references goes
    /// now, as a tier-0 version's does (K-T0-9); one still referenced stays, and is reported.
    pub(super) fn remove_released_refs(
        &self,
        released: &[(u64, BlobId, u64, u64)],
        counts: &mut PurgeCounts,
        not_reached: &mut Vec<NotReached>,
    ) -> Result<()> {
        let free: Vec<(u64, BlobId, u64)> = released
            .iter()
            .filter(|r| r.3 == 0)
            .map(|r| (r.0, r.1, r.2))
            .collect();
        let mut shared = (released.len() - free.len()) as u64;
        if !free.is_empty() {
            match self.blobs.get() {
                Some(blobs) => {
                    let (removed, _) = blobs.remove_unreferenced_now(&free)?;
                    counts.blobs_removed += removed;
                    // what stayed: another doc's name was on record, and it took the count
                    shared += free.len() as u64 - removed;
                }
                None => not_reached.push(NotReached::LeftForBlobGc(free.len() as u64)),
            }
        }
        if shared > 0 {
            not_reached.push(NotReached::SharedBlobs(shared));
        }
        Ok(())
    }
}
