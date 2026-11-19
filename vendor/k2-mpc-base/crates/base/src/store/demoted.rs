//! Text → tier 0 in the store (`docs/SESSIONS.md`, "Text → tier 0"): a live text doc becomes a
//! blob doc **in one transaction**, keeping its doc id and key.
//!
//! What the transaction writes, all or nothing:
//!
//! - the doc's metadata (`kit:blob`, no engine, in place of the text pack's);
//! - one blob version per text version, then the live doc's final state and, when the file was
//!   saved from outside since the last sync, that save (each taking a reference on its blob);
//! - the file's sync state, as tier 0 keeps it;
//! - the **live log**: one row per change the text doc logged, in `demoted_changes`, with its
//!   summary (kind, author, via, time, lines added and removed, the person it was made for,
//!   its turn and pack verb) and its approval record. No row holds doc content.
//!
//! and erases the text doc's own rows: its log, snapshot, clients, versions (now blob
//! versions), approvals and summaries (now `demoted_changes` rows), revert records, sync
//! replica and hash key, carriers and value references (released). The tier-0 rows a doc
//! promoted from tier 0 kept (its tier-0 history), its annotations and its structure-index
//! keys stay. Every per-doc table is named in `KEPT` or [`ERASED`]; a unit test fails on one
//! in neither, so a new table can't be forgotten here.
//!
//! A process that stops before the commit leaves the text doc exactly as it was; after it, the
//! blob doc.
//!
//! **`demoted_changes`** is one additive per-doc table, **without a format bump** (the terms of
//! `change_turns` and `change_verbs`): created lazily by the first demotion, keyed `(doc, n)`
//! with `n` counting on from the doc's last row (a doc demoted, promoted and demoted again
//! keeps both runs). **An older kit** opens a demoted doc as the tier-0 doc it is (every
//! version, its bytes and the current one read as this kit wrote them) without its live log;
//! it refuses a rewrite, and so a purge or an erasure, before changing anything
//! (`StoreError::NewerData`, `MPC-E502`; never a silent drop), and leaves the rows of a doc it
//! deletes, which every open here removes (`sweep_orphans`).
//!
//! **Erasure** renames the erased participant in every row (author, the person a change was
//! made for, the approval's names) and drops the turn and verb of their changes, as a text
//! doc's erasure does. A purge has no content here to remove.

use redb::{ReadableTable, TableDefinition, WriteTransaction};

use super::blob::{self, BlobSync, BLOB_SYNC};
use super::enc::{R, W};
use super::{
    dec_approval, dec_meta, enc_approval, enc_meta, Approval, ChangeInfo, ChangeKind, DocId,
    DocMeta, DocStore, StoreError, VersionId, DOCS,
};
use crate::store::BlobVersion;
use crate::turn::Turn;

/// `(doc, n)` → one change of a demoted doc's live log (record version 1).
pub(super) const DEMOTED_CHANGES: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("demoted_changes");

/// The per-doc tables a demotion leaves as they are: the doc's record, the tier-0 tables, the
/// annotations, the structure index's keys and the live log (the unit test's list: every
/// per-doc table is here or in [`ERASED`]).
#[cfg(test)]
pub(super) const KEPT: &[&str] = &[
    "docs",
    "blob_versions",
    "blob_sync",
    "blob_approvals",
    "blob_struct_keys",
    "blob_turns",
    "blob_verbs",
    "annot_threads",
    "annot_comments",
    "annot_log",
    "annot_thread_log",
    "annot_changed",
    "demoted_changes",
    // the chunked versions a doc kept from an earlier life at tier 0
    "doc_chunk_keys",
    "doc_chunks",
    "doc_manifests",
    "doc_segments",
    "doc_removals",
];

/// The per-doc tables a demotion erases (the text doc's own). `live_carriers`,
/// `version_carriers` and `doc_blob_refs` release their blob references first.
pub(super) const ERASED: &[&str] = &[
    "snapshots",
    "doc_keys",
    "updates",
    "clients",
    "client_kinds",
    "versions",
    "approvals",
    "sync",
    "changes",
    "reverts",
    "revert_index",
    "revert_carry",
    "revert_stats",
    "authored",
    "change_clocks",
    "provenance",
    "elem_writes",
    "row_marks",
    "overwrite_notes",
    "live_copies",
    "change_turns",
    "change_verbs",
    "live_carriers",
    "version_carriers",
    "carrier_pairings",
    "doc_blob_refs",
    // a native doc's engine store (no demotion reaches a native doc today; its engine state
    // goes with the rest)
    "native_records",
    "native_gens",
];

/// One change of a demoted doc's live log ([`DocStore::demoted_changes`]): its summary as the
/// text doc's `log` listed it, and its approval record (an on-behalf change's). No content.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DemotedChange {
    pub info: ChangeInfo,
    pub approval: Option<Approval>,
}

impl DemotedChange {
    pub(crate) fn new(info: ChangeInfo, approval: Option<Approval>) -> Self {
        Self { info, approval }
    }
}

fn enc_row(c: &DemotedChange) -> Vec<u8> {
    let i = &c.info;
    let mut w = W::default();
    w.u8(1)
        .u64(i.seq)
        .u8(i.kind.to_u8())
        .u8(u8::from(i.of.is_some()))
        .u64(i.of.unwrap_or(0))
        .str(&i.author)
        .str(&i.via)
        .u64(i.at_ms)
        .opt_str(i.on_behalf_of.as_deref())
        .u64(i.lines_added)
        .u64(i.lines_removed)
        .opt_str(i.turn.as_ref().map(Turn::as_str))
        .opt_str(i.verb.as_deref())
        .opt_bytes(c.approval.as_ref().map(enc_approval).as_deref());
    w.0
}

fn dec_row(b: &[u8]) -> Result<DemotedChange, StoreError> {
    let mut r = R::new(b, "demoted change");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("demoted change version"));
    }
    let seq = r.u64()?;
    let kind = ChangeKind::from_u8(r.u8()?)?;
    let has_of = r.u8()?;
    let of = r.u64()?;
    let mut info = ChangeInfo::new(kind, "", "", 0);
    info.seq = seq;
    info.of = match has_of {
        0 => None,
        1 => Some(of),
        _ => return Err(StoreError::Corrupt("demoted change")),
    };
    info.author = r.str()?;
    info.via = r.str()?;
    info.at_ms = r.u64()?;
    info.on_behalf_of = r.opt_str()?;
    info.lines_added = r.u64()?;
    info.lines_removed = r.u64()?;
    info.turn = r.opt_str()?.and_then(|t| Turn::new(&t).ok());
    info.verb = r.opt_str()?;
    let approval = r.opt_bytes()?.map(|b| dec_approval(&b)).transpose()?;
    r.end()?;
    Ok(DemotedChange { info, approval })
}

/// What [`DocStore::demote_text_doc`] writes.
pub(crate) struct DemoteRows<'a> {
    /// The text doc's pack, as the session holds it: another stops the demotion.
    pub pack: &'a str,
    /// The blob doc's metadata (same key).
    pub meta: &'a DocMeta,
    /// The blob versions, oldest first: they take the ids after the doc's last blob version
    /// (past its lost ones, `seq_floor.rs`).
    pub versions: &'a [BlobVersion],
    /// What the file holds, when the doc has one.
    pub sync: Option<&'a BlobSync>,
    /// The live log, oldest first.
    pub changes: &'a [DemotedChange],
    /// Host clock: when released blobs lost a reference.
    pub now: u64,
}

#[cfg(test)]
thread_local! {
    /// Test-only: fail the demotion after every write, before the commit (a process
    /// stopping mid-demotion: the transaction is dropped, so nothing of it lands).
    pub(crate) static FAIL_BEFORE_COMMIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The last key of `table` for doc `id` (0 when none, or no table).
fn last_n(w: &WriteTransaction, id: DocId) -> Result<u64, StoreError> {
    if !super::history::has_table(w, "demoted_changes")? {
        return Ok(0);
    }
    let t = w.open_table(DEMOTED_CHANGES)?;
    let last = t.range((id, 0)..=(id, u64::MAX))?.next_back().transpose()?;
    Ok(last.map_or(0, |(k, _)| k.value().1))
}

impl DocStore {
    /// Demote text doc `id` to a blob doc, in one transaction (see the module docs). Returns
    /// the ids the versions took, in order.
    ///
    /// Errors: [`StoreError::NoDoc`]; [`StoreError::Corrupt`] when `id` isn't a doc of
    /// `rows.pack`, or there are no versions; [`StoreError::KeyChange`] when `meta` names another
    /// key; [`StoreError::NoBlobSpace`] when the space has no blob key (the versions' puts make
    /// it).
    pub(crate) fn demote_text_doc(
        &self,
        id: DocId,
        p: &DemoteRows<'_>,
    ) -> Result<Vec<VersionId>, StoreError> {
        if p.versions.is_empty() {
            return Err(StoreError::Corrupt("demotion: no version"));
        }
        // a barrier: the doc changes tier (grouped commits, `group.rs`)
        let w = self.begin_barrier("demote")?;
        let vids = {
            let old = {
                let docs = w.open_table(DOCS)?;
                let m = docs.get(id)?.ok_or(StoreError::NoDoc(id))?;
                dec_meta(m.value())?
            };
            if old.pack != p.pack {
                return Err(StoreError::Corrupt("demotion: not that text doc"));
            }
            if old.key != p.meta.key {
                return Err(StoreError::KeyChange);
            }
            // the text doc's rows: references released first, then every erased table
            super::live_carrier::delete_rows(&w, id, p.now)?;
            super::blob_refs::delete_rows(&w, id, p.now)?;
            for (t, _) in super::DOC_TABLES {
                if ERASED.contains(&t.name()) {
                    t.erase(&w, id)?;
                }
            }
            // a native doc's engine store went with them: the store's last one takes the
            // native bit with it (as a delete does)
            super::features::clear_native_if_none(&w)?;
            // the blob doc
            w.open_table(DOCS)?
                .insert(id, enc_meta(p.meta).as_slice())?;
            let k = blob::space_in(&w, &p.meta.key.space)?;
            // the versions take the ids after the last one, past the lost ones
            // (`seq_floor.rs`)
            let mut last = self.blob_before_next_in(&w, id, blob::last_vid(&w, id)?)?;
            let mut vids = Vec::with_capacity(p.versions.len());
            for v in p.versions {
                if !super::chunks::take_ref(&w, id, &v.blob)? {
                    blob::adjust_ref(&w, k.no, &v.blob, 1, v.at_ms)?;
                }
                last += 1;
                blob::put_version(&w, id, last, v)?;
                vids.push(last);
            }
            // the demotion's commit is a barrier (`begin_barrier("demote")` above): durable
            self.raise_blob_floor_in(&w, id, last, true)?;
            {
                let mut t = w.open_table(BLOB_SYNC)?;
                t.retain_in((id, 0)..=(id, u8::MAX), |_, _| false)?;
                if let Some(s) = p.sync {
                    t.insert((id, 0), blob::enc_sync(s).as_slice())?;
                }
            }
            if !p.changes.is_empty() {
                let mut n = last_n(&w, id)?;
                let mut t = w.open_table(DEMOTED_CHANGES)?;
                for c in p.changes {
                    n += 1;
                    t.insert((id, n), enc_row(c).as_slice())?;
                }
            }
            vids
        };
        #[cfg(test)]
        if FAIL_BEFORE_COMMIT.with(std::cell::Cell::get) {
            return Err(StoreError::Corrupt(
                "test: stopped before the demotion's commit",
            ));
        }
        w.commit()?;
        Ok(vids)
    }

    /// The live log a demotion kept for doc `id` (`docs/SESSIONS.md`, "Text → tier 0"), in
    /// the order it was kept: oldest first. Empty for a doc never demoted.
    pub fn demoted_changes(&self, id: DocId) -> Result<Vec<DemotedChange>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(DEMOTED_CHANGES) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for kv in t.range((id, 0)..=(id, u64::MAX))? {
            out.push(dec_row(kv?.1.value())?);
        }
        Ok(out)
    }
}

/// Whether `pid` is named in doc `id`'s live log (an author, the person a change was made
/// for, or an approval's names).
pub(super) fn names(w: &WriteTransaction, id: DocId, pid: &str) -> Result<bool, StoreError> {
    if !super::history::has_table(w, "demoted_changes")? {
        return Ok(false);
    }
    let t = w.open_table(DEMOTED_CHANGES)?;
    for kv in t.range((id, 0)..=(id, u64::MAX))? {
        let c = dec_row(kv?.1.value())?;
        let a = c.approval.as_ref();
        if c.info.author == pid
            || c.info.on_behalf_of.as_deref() == Some(pid)
            || a.is_some_and(|a| {
                a.author == pid || a.on_behalf_of == pid || a.by.as_deref() == Some(pid)
            })
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Participant erasure on doc `id`'s live log, in `w`: `pid` becomes `ps` in every row, and
/// the turn and verb of the changes they made, or that were made for them, go. Returns the
/// rows changed.
pub(super) fn erase_in(
    w: &WriteTransaction,
    id: DocId,
    pid: &str,
    ps: &str,
) -> Result<u64, StoreError> {
    if !super::history::has_table(w, "demoted_changes")? {
        return Ok(0);
    }
    let mut t = w.open_table(DEMOTED_CHANGES)?;
    let rows: Vec<(u64, Vec<u8>)> = t
        .range((id, 0)..=(id, u64::MAX))?
        .map(|kv| kv.map(|(k, v)| (k.value().1, v.value().to_vec())))
        .collect::<Result<_, _>>()?;
    let swap = |s: &mut String| {
        let hit = s == pid;
        if hit {
            *s = ps.to_owned();
        }
        hit
    };
    let mut n = 0;
    for (k, b) in rows {
        let mut c = dec_row(&b)?;
        let mine = swap(&mut c.info.author);
        let obo = c.info.on_behalf_of.as_mut().is_some_and(swap);
        let mut hit = mine | obo;
        if let Some(a) = c.approval.as_mut() {
            hit |=
                swap(&mut a.author) | swap(&mut a.on_behalf_of) | a.by.as_mut().is_some_and(swap);
        }
        if mine | obo {
            c.info.turn = None;
            c.info.verb = None;
        }
        if hit {
            t.insert((id, k), enc_row(&c).as_slice())?;
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ApprovalState;

    /// Every per-doc table is either kept or erased by a demotion, never neither: a new table
    /// fails here until it is placed.
    #[test]
    fn every_doc_table_is_kept_or_erased() {
        for (t, _) in super::super::DOC_TABLES {
            let n = t.name();
            let (k, e) = (KEPT.contains(&n), ERASED.contains(&n));
            assert!(
                k ^ e,
                "table {n}: kept {k}, erased {e} (place it in demoted.rs)"
            );
        }
        for n in KEPT.iter().chain(ERASED) {
            assert!(
                super::super::DOC_TABLES.iter().any(|(t, _)| t.name() == *n),
                "{n} is no per-doc table"
            );
        }
    }

    #[test]
    fn a_row_round_trips() {
        let mut info = ChangeInfo::new(ChangeKind::Revert, "agent", "cli", 42);
        info.seq = 7;
        info.of = Some(3);
        info.on_behalf_of = Some("ann".into());
        info.lines_added = 2;
        info.lines_removed = 1;
        info.turn = Some(Turn::new("run-1").unwrap());
        info.verb = Some("shape move".into());
        let c = DemotedChange {
            info,
            approval: Some(Approval::pending("agent", "ann").settled(
                ApprovalState::Approved,
                "ann",
                50,
            )),
        };
        assert_eq!(dec_row(&enc_row(&c)).unwrap(), c);
        let bare = DemotedChange {
            info: ChangeInfo::new(ChangeKind::Import, "kit:import", "import", 1),
            approval: None,
        };
        assert_eq!(dec_row(&enc_row(&bare)).unwrap(), bare);
        let mut other = enc_row(&bare);
        other[0] = 2;
        assert!(
            dec_row(&other).is_err(),
            "another record version is refused"
        );
    }
}
