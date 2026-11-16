//! The purge of chunked versions and the compaction of their segments, in the blob store
//! (`docs/design/version-storage-r2.md`, sections 2 to 4 and 9; `docs/SESSIONS.md`, "Chunked
//! versions").
//!
//! **A purge** ([`BlobStore::purge_versions`]) runs under the doc's internal lock (its caller
//! holds it: no write of the doc is between its marks and its commit). It plans in a transaction
//! it drops (the purge's own changes, then [`plan_in`]: every segment of the doc holding a
//! purged chunk or any other dead range), writes each output segment (the segment's kept chunks
//! only, each checked against its id; a temp file, synced, its name marked waiting, renamed into
//! place, its folder synced), then commits the purge and the compaction in one transaction,
//! which checks the plan still holds ([`apply_in`]; a doc that changed since is planned again).
//! After the commit it deletes the files on the removal list and clears them from it, and
//! sweeps the doc's segment folder: a segment no row names and a temp file go.
//!
//! **The removal list** is finished after every commit that writes it, and at every
//! [`BlobStore::open`]; an entry whose file can't be deleted stays for the next try.
//!
//! **`gc`** runs the compaction retention deferred: a segment less than half live, of a doc
//! whose lock is free (a busy doc waits for the next `gc`). A write never starts a compaction.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use redb::WriteTransaction;

use super::chunked::ensure_dir;
use super::{io_err, BlobError, BlobId, BlobStore, TMP};
use crate::store::{
    apply_in, chunk_id, plan_in, read_at_file, segment_dir, segment_path, ChunkId, Compaction,
    DocId, Select, StoreError, VersionId,
};

/// What a purge released ([`crate::store::DocStore::blob_purge_versions`]): each blob or
/// manifest with the references it still has, the approval records reached, and the anchors'
/// copies forgotten.
pub(crate) type Purged = (Vec<(u64, BlobId, u64, u64)>, u64, u64);

/// How many times a purge plans again when the doc changed between its plan and its commit.
const PLAN_TRIES: usize = 3;

/// The planted faults this module reads (`crate::differential::ChunkPlant`).
#[derive(Clone, Copy)]
enum Plant {
    PurgeSkipsCompaction,
    RemovalListNotFinishedAtOpen,
    CompactionSkipsDeadRanges,
    CompactionOutputNotWaiting,
    PurgeSkipsDocLock,
}

/// Whether `p` is planted on this thread (test builds only).
fn planted(p: Plant) -> bool {
    // a build without the plants still reads `p` (no unused variable there)
    let _ = p;
    plant_on!({
        use crate::differential::ChunkPlant as C;
        let want = match p {
            Plant::PurgeSkipsCompaction => C::PurgeSkipsCompaction,
            Plant::RemovalListNotFinishedAtOpen => C::RemovalListNotFinishedAtOpen,
            Plant::CompactionSkipsDeadRanges => C::CompactionSkipsDeadRanges,
            Plant::CompactionOutputNotWaiting => C::CompactionOutputNotWaiting,
            Plant::PurgeSkipsDocLock => C::PurgeSkipsDocLock,
        };
        crate::differential::chunk_plant() == Some(want)
    })
}

/// A step a test hook can see (and, after a purge's commit, stop at).
#[derive(Clone, Copy)]
enum Step {
    Renamed,
    Committed,
}

#[cfg(any(test, feature = "differential"))]
fn hook(step: Step) -> bool {
    use crate::differential::{chunk_hook, ChunkStep};
    chunk_hook(match step {
        Step::Renamed => ChunkStep::CompactionRenamed,
        Step::Committed => ChunkStep::PurgeCommitted,
    })
}

#[cfg(not(any(test, feature = "differential")))]
fn hook(_: Step) -> bool {
    false
}

/// A compaction's output segments between their rename and their commit: their names are
/// marked waiting until it is dropped, and, unless [`Outputs::committed`] said so, the files are
/// removed then (no row will name them).
pub(super) struct Outputs<'a> {
    store: &'a BlobStore,
    no: u64,
    doc: DocId,
    marked: Vec<u64>,
    files: Vec<PathBuf>,
    committed: bool,
}

impl Outputs<'_> {
    /// The commit naming them landed: the files stay; their marks go when this is dropped.
    pub(super) fn committed(mut self) {
        self.committed = true;
    }
}

impl Drop for Outputs<'_> {
    fn drop(&mut self) {
        let _g = self.store.guard();
        if !self.committed {
            for f in &self.files {
                let _ = fs::remove_file(f);
            }
        }
        let mut m = self.store.marks();
        for seg in &self.marked {
            m.release_segment(self.no, self.doc, *seg);
        }
    }
}

impl BlobStore {
    /// Doc `doc`'s internal lock, waited for: a purge's, taken before its session's lock (a
    /// write holds the doc's lock across its commit, which takes the session's). `None` under
    /// the planted fault `purge-skips-doc-lock`.
    pub(crate) fn lock_doc(&self, doc: DocId) -> Option<super::chunked::DocGuard> {
        if planted(Plant::PurgeSkipsDocLock) {
            return None;
        }
        Some(self.doc_locks.take_waiting(doc))
    }

    /// Purge versions `vids` of doc `doc` in `space` at `now` (module docs): the store's purge
    /// with the compaction that leaves no byte of a purged chunk, and no other dead range, in
    /// any segment of the doc; then the removal list and the closing sweep. The caller holds
    /// the doc's lock ([`BlobStore::lock_doc`]).
    pub(crate) fn purge_versions(
        &self,
        space: &str,
        doc: DocId,
        vids: &[VersionId],
        now: u64,
    ) -> Result<Purged, BlobError> {
        let no = self.docs.blob_space(space)?.map(|k| k.no);
        let skip = planted(Plant::PurgeSkipsCompaction);
        let select = Select::Purge {
            only_purged: planted(Plant::CompactionSkipsDeadRanges),
        };
        for _ in 0..PLAN_TRIES {
            // the plan: the purge's own changes, in a transaction that is dropped
            let mut plan: Option<Compaction> = None;
            let mut planning =
                |w: &WriteTransaction, erased: &[ChunkId]| -> Result<bool, StoreError> {
                    if !skip {
                        plan = plan_in(w, doc, select, erased)?;
                    }
                    Ok(false)
                };
            self.docs
                .blob_purge_versions_with(doc, vids, now, Some(&mut planning))?;
            let outs = match (&plan, no) {
                (Some(p), Some(no)) => Some(self.write_outputs(no, p)?),
                (Some(p), None) if !p.moves.is_empty() => {
                    return Err(StoreError::Corrupt("chunked segments of a purged space").into())
                }
                _ => None,
            };
            let mut committing =
                |w: &WriteTransaction, erased: &[ChunkId]| -> Result<bool, StoreError> {
                    if skip {
                        return Ok(true);
                    }
                    apply_in(w, no.unwrap_or(0), plan.as_ref(), select, erased)
                };
            let done = self
                .docs
                .blob_purge_versions_with(doc, vids, now, Some(&mut committing))?;
            let Some(done) = done else {
                continue; // the doc changed since the plan: its outputs go, and it plans again
            };
            if let Some(o) = outs {
                o.committed();
            }
            if hook(Step::Committed) {
                return Err(BlobError::Io(
                    "stopped after the purge's commit (test hook)".into(),
                ));
            }
            if let Some(no) = no {
                self.finish_removals(Some(doc))?;
                self.sweep_doc(no, doc);
            }
            return Ok(done);
        }
        Err(
            StoreError::Db("the doc kept changing under its purge; nothing was purged".into())
                .into(),
        )
    }

    /// Write `plan`'s output segments (module docs), each marked waiting before its rename.
    fn write_outputs(&self, no: u64, plan: &Compaction) -> Result<Outputs<'_>, BlobError> {
        static N: AtomicU64 = AtomicU64::new(0);
        let mut outs = Outputs {
            store: self,
            no,
            doc: plan.doc,
            marked: Vec::new(),
            files: Vec::new(),
            committed: false,
        };
        let dir = segment_dir(&self.dir, no, plan.doc);
        let unmarked = planted(Plant::CompactionOutputNotWaiting);
        for m in &plan.moves {
            let Some(out) = m.out else { continue };
            let tmp = self.dir.join(TMP).join(format!(
                "{}-{}.compact.part",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let src = segment_path(&self.dir, no, plan.doc, m.old);
            let written = (|| -> Result<(), BlobError> {
                let mut f = File::open(&src).map_err(|e| io_err("opening a segment", e))?;
                let mut o = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&tmp)
                    .map_err(|e| io_err("compaction temp file", e))?;
                for (id, off, len) in &m.kept {
                    let b = read_at_file(&mut f, *off, *len)
                        .map_err(|e| io_err("reading a segment", e))?;
                    if chunk_id(&plan.key, &b) != *id {
                        return Err(BlobError::Damaged(format!(
                            "segment {}: a chunk doesn't match its id",
                            m.old
                        )));
                    }
                    o.write_all(&b)
                        .map_err(|e| io_err("writing a compacted segment", e))?;
                }
                o.sync_all()
                    .map_err(|e| io_err("syncing a compacted segment", e))
            })();
            if let Err(e) = written {
                let _ = fs::remove_file(&tmp);
                return Err(e);
            }
            let path = segment_path(&self.dir, no, plan.doc, out);
            // the name waits before it exists, under the lock `gc` sweeps under
            let _g = self.guard();
            if !unmarked {
                self.marks().mark_segment(no, plan.doc, out);
                outs.marked.push(out);
            }
            if let Err(e) = ensure_dir(&dir).and_then(|()| fs::rename(&tmp, &path)) {
                let _ = fs::remove_file(&tmp);
                return Err(io_err("placing a compacted segment", e));
            }
            outs.files.push(path);
        }
        if !outs.files.is_empty() {
            File::open(&dir)
                .and_then(|d| d.sync_all())
                .map_err(|e| io_err("syncing a segment's folder", e))?;
            hook(Step::Renamed);
        }
        Ok(outs)
    }

    /// Delete the files on the removal list (every doc's, or doc `doc`'s) and clear their
    /// entries in one transaction; an entry whose file can't be deleted stays, for the next
    /// try. Returns the entries left.
    pub(super) fn finish_removals(&self, doc: Option<DocId>) -> Result<usize, BlobError> {
        let list = self.docs.chunk_removals(doc)?;
        let mut done = Vec::with_capacity(list.len());
        for (d, seg, no) in &list {
            match fs::remove_file(segment_path(&self.dir, *no, *d, *seg)) {
                Ok(()) => done.push((*d, *seg)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => done.push((*d, *seg)),
                Err(_) => {}
            }
        }
        if !done.is_empty() {
            self.docs.chunk_clear_removals(&done)?;
        }
        Ok(list.len() - done.len())
    }

    /// The removal list finished at an open (before any session), unless planted off.
    pub(super) fn finish_removals_at_open(&self) -> Result<(), BlobError> {
        if !planted(Plant::RemovalListNotFinishedAtOpen) {
            self.finish_removals(None)?;
        }
        Ok(())
    }

    /// Sweep doc `doc`'s segment folder (space `no`): every file but a segment a row names or a
    /// write or compaction waits on goes, temp files included. Returns the files removed.
    pub(super) fn sweep_doc(&self, no: u64, doc: DocId) -> usize {
        let _g = self.guard();
        let mut removed = 0;
        for f in fs::read_dir(segment_dir(&self.dir, no, doc))
            .into_iter()
            .flatten()
            .flatten()
        {
            let seg = f
                .file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".seg"))
                .and_then(|n| n.parse::<u64>().ok());
            if let Some(seg) = seg {
                if self.marks().segment_waiting(no, doc, seg) {
                    continue;
                }
                let named = self
                    .docs
                    .chunk_read(|r| r.has_segment(doc, seg))
                    .unwrap_or(true);
                if !named && fs::remove_file(f.path()).is_ok() {
                    removed += 1;
                }
                continue;
            }
            if fs::remove_file(f.path()).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    /// The compaction retention deferred (`gc`'s): every doc's segments less than half live,
    /// for each doc whose lock is free.
    pub(super) fn compact_sparse(&self) -> Result<(), BlobError> {
        for (doc, space) in self.docs.chunked_docs()? {
            let Some(k) = self.docs.blob_space(&space)? else {
                continue;
            };
            let Some(_doc) = self.doc_locks.take(doc, false) else {
                continue;
            };
            let Some(plan) = self.docs.chunk_plan_sparse(doc)? else {
                continue;
            };
            if plan.moves.is_empty() {
                continue;
            }
            let outs = self.write_outputs(k.no, &plan)?;
            if self.docs.chunk_compact_sparse(k.no, &plan)? {
                outs.committed();
                self.finish_removals(Some(doc))?;
            }
        }
        Ok(())
    }

    /// Test-only: purge as a session does, under the doc's lock, with no session.
    #[cfg(any(test, feature = "differential"))]
    pub(super) fn purge_for_tests(
        &self,
        space: &str,
        doc: DocId,
        vids: &[VersionId],
    ) -> Result<(), BlobError> {
        let _doc = self.lock_doc(doc);
        let now = (self.clock)();
        self.purge_versions(space, doc, vids, now)?;
        self.docs.rewrite_file()?;
        self.docs.clear_rewrite_pending()?;
        Ok(())
    }
}
