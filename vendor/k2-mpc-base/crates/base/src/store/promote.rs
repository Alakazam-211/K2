//! Tier 0 → text promotion in the store (v0.9.2): a blob doc becomes a text doc **in one
//! transaction**, keeping its doc id and key. No new table and no new record version: the
//! store format stays 5.
//!
//! What the transaction writes, all or nothing:
//!
//! - the doc's metadata (the text pack, engine and codec in place of `kit:blob`);
//! - the text history the session built from the blob versions that read as text: the
//!   clients that made it, its log entries (with their pending approvals, replaced by the
//!   carried record when the blob version's was settled), change summaries and revert
//!   records, and one version per imported blob version;
//! - the disk-sync state and the doc's hash key, as an import writes them;
//! - the blob doc's own rows: its sync state goes; each **moved** blob version (its
//!   content and approval now live in the text history) loses its row and approval and
//!   releases its reference on its blob. Every other blob version (non-text, purged, or an
//!   on-behalf version whose text made no change to carry its approval) keeps its row,
//!   approval and reference: the doc's **tier-0 history**, read with the text session's
//!   `tier0_*` calls.
//!
//! A crash before the commit leaves the blob doc exactly as it was; after it, the text doc.

use redb::ReadableTable;

use super::blob::{self, PurgedBlob, BLOB_APPROVALS, BLOB_SYNC, BLOB_VERSIONS};
use super::history::{self, NewChange};
use super::{
    dec_meta, enc_approval, enc_doc_key, enc_meta, enc_sync, enc_version, put_entry, Approval,
    ApprovalState, DocId, DocMeta, DocStore, LogEntry, StoreError, SyncState, Version, VersionId,
    APPROVALS, BLOB_DOC_PACK, CLIENTS, DOCS, DOC_KEYS, SNAPSHOTS, SYNC, SYNCED, UPDATES, VERSIONS,
};
use crate::doc::ClientId;

/// What [`DocStore::promote_blob_doc`] writes.
pub(crate) struct PromoteRows<'a> {
    /// The blob doc's current version as the session read it: anything newer stops the
    /// promotion ([`StoreError::LogMoved`]).
    pub expect_last: VersionId,
    /// The text doc's metadata (same key).
    pub meta: &'a DocMeta,
    /// Each client of the text history and the pid that owns it.
    pub clients: &'a [(ClientId, String)],
    /// The text history's log, positions 1, 2, ... in order.
    pub entries: &'a [LogEntry],
    /// One per entry: its summary and revert record.
    pub changes: &'a [NewChange],
    /// `(entry index, record)`: the blob version's approval, carried to the entry's change.
    pub approvals: &'a [(usize, Approval)],
    /// The text versions (ids 1, 2, ... in order), each with its log position set.
    pub versions: &'a [Version],
    pub sync: &'a SyncState,
    pub doc_key: &'a [u8; 32],
    /// Blob versions whose rows go (moved into the text history).
    pub moved: &'a [VersionId],
    /// Host clock: when released blobs lost their last reference.
    pub now: u64,
}

#[cfg(test)]
thread_local! {
    /// Test-only: fail the promotion after every write, before the commit (a crash
    /// mid-promote: the transaction is dropped, so nothing of it lands).
    pub(crate) static FAIL_BEFORE_COMMIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl DocStore {
    /// Promote blob doc `id` to a text doc, in one transaction (see the module docs).
    /// Returns the blobs the moved versions released, with the references each still has
    /// (0: nothing else uses it, so its bytes can go now).
    ///
    /// Errors: [`StoreError::NoDoc`]; [`StoreError::Corrupt`] when `id` isn't a blob doc or
    /// already has text rows; [`StoreError::KeyChange`] when `meta` names another key;
    /// [`StoreError::LogMoved`] when a version was added since `expect_last`.
    pub(crate) fn promote_blob_doc(
        &self,
        id: DocId,
        p: &PromoteRows<'_>,
    ) -> Result<Vec<PurgedBlob>, StoreError> {
        if p.entries.len() != p.changes.len() {
            return Err(StoreError::Corrupt("promotion: one change per entry"));
        }
        let w = self.begin_write()?;
        let released = {
            let old = {
                let docs = w.open_table(DOCS)?;
                let m = docs.get(id)?.ok_or(StoreError::NoDoc(id))?;
                dec_meta(m.value())?
            };
            if old.pack != BLOB_DOC_PACK {
                return Err(StoreError::Corrupt("promotion: not a blob doc"));
            }
            if old.key != p.meta.key {
                return Err(StoreError::KeyChange);
            }
            let last = blob::last_vid(&w, id)?;
            if last != p.expect_last {
                return Err(StoreError::LogMoved(id, p.expect_last, last));
            }
            let has_text_rows = w
                .open_table(UPDATES)?
                .range((id, 0)..=(id, u64::MAX))?
                .next()
                .is_some()
                || w.open_table(VERSIONS)?
                    .range((id, 0)..=(id, u64::MAX))?
                    .next()
                    .is_some()
                || w.open_table(SNAPSHOTS)?.get(id)?.is_some();
            if has_text_rows {
                return Err(StoreError::Corrupt("promotion: the blob doc has text rows"));
            }

            // the text doc
            w.open_table(DOCS)?
                .insert(id, enc_meta(p.meta).as_slice())?;
            {
                let mut t = w.open_table(CLIENTS)?;
                for (c, owner) in p.clients {
                    t.insert((id, *c), owner.as_str())?;
                }
            }
            let seqs: Vec<u64> = (1..=p.entries.len() as u64).collect();
            for (seq, e) in seqs.iter().zip(p.entries) {
                put_entry(&w, id, *seq, e)?;
            }
            history::put_changes(&w, id, &seqs, p.changes)?;
            for (i, a) in p.approvals {
                let seq = *seqs
                    .get(*i)
                    .ok_or(StoreError::Corrupt("promotion: approval of no entry"))?;
                if p.entries[*i].on_behalf_of.as_deref() != Some(a.on_behalf_of.as_str()) {
                    return Err(StoreError::Corrupt("promotion: approval of another person"));
                }
                w.open_table(APPROVALS)?
                    .insert((id, seq), enc_approval(a).as_slice())?;
                if a.state != ApprovalState::Pending {
                    history::settle_pending(&w, id, seq)?;
                }
            }
            {
                let mut t = w.open_table(VERSIONS)?;
                for (vid, v) in (1u64..).zip(p.versions) {
                    t.insert((id, vid), enc_version(v).as_slice())?;
                }
            }
            {
                let mut t = w.open_table(SYNC)?;
                t.retain_in((id, 0)..=(id, u8::MAX), |_, _| false)?;
                t.insert((id, SYNCED), enc_sync(p.sync).as_slice())?;
            }
            w.open_table(DOC_KEYS)?
                .insert(id, enc_doc_key(p.doc_key).as_slice())?;

            // the blob doc's rows: its sync state, and the versions that moved
            w.open_table(BLOB_SYNC)?
                .retain_in((id, 0)..=(id, u8::MAX), |_, _| false)?;
            let space = match blob::space_in(&w, &old.key.space) {
                Ok(k) => Some(k),
                Err(StoreError::NoBlobSpace) => None,
                Err(e) => return Err(e),
            };
            let mut gone = Vec::new();
            for &vid in p.moved {
                let v = {
                    let t = w.open_table(BLOB_VERSIONS)?;
                    let g = t.get((id, vid))?.ok_or(StoreError::NoVersion(id, vid))?;
                    blob::dec_version(g.value())?
                };
                if v.redacted {
                    // a purged version is never imported, so never moved
                    return Err(StoreError::PurgedVersion(id, vid));
                }
                w.open_table(BLOB_VERSIONS)?.remove((id, vid))?;
                w.open_table(BLOB_APPROVALS)?.remove((id, vid))?;
                if let Some(k) = &space {
                    match blob::adjust_ref(&w, k.no, &v.blob, -1, p.now) {
                        Ok(()) => gone.push((k.no, v.blob, v.size)),
                        Err(StoreError::NoBlob(_)) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            let mut out: Vec<PurgedBlob> = Vec::new();
            if let Some(k) = &space {
                let t = w.open_table(super::blob::BLOBS)?;
                for (no, b, size) in gone {
                    if out.iter().any(|r| r.1 == b) {
                        continue;
                    }
                    let refs = t
                        .get((k.no, b.as_bytes().as_slice()))?
                        .map(|v| blob::dec_row(v.value()))
                        .transpose()?
                        .map_or(0, |r| r.refs);
                    out.push((no, b, size, refs));
                }
            }
            out
        };
        #[cfg(test)]
        if FAIL_BEFORE_COMMIT.with(std::cell::Cell::get) {
            return Err(StoreError::Corrupt(
                "test: crash before the promotion's commit",
            ));
        }
        w.commit()?;
        Ok(released)
    }
}
