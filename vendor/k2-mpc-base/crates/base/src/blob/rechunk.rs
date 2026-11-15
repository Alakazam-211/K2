//! Converting a doc's whole-file versions to chunked ones, on request
//! (`docs/design/version-storage-r2.md`, section 12; `docs/SESSIONS.md`, "Chunked versions").

use super::chunked::{planted, Plant, Src};
use super::{BlobError, BlobId, BlobStore, CheapSet};
use crate::store::{DocId, StoreError, BLOB_DOC_PACK};

/// Returned by [`BlobStore::rechunk`]: what it did with each version of the doc.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RechunkReport {
    /// Versions stored chunked now (every version naming bytes it converted counts).
    pub converted: u64,
    /// The bytes it converted: each whole-file blob's size, once.
    pub bytes: u64,
    /// Versions left whole: under [`super::BlobConfig::chunk_min_bytes`].
    pub under_minimum: u64,
    /// Versions skipped: purged (their content is gone).
    pub redacted: u64,
    /// Versions already stored chunked.
    pub already_chunked: u64,
    /// Versions skipped because a write held the doc's internal lock: a later rechunk
    /// converts them.
    pub busy: u64,
    /// Versions whose whole-file blob isn't in the store: left as they are.
    pub missing: u64,
    /// Whole-file blobs removed at once (nothing else referenced them).
    pub blobs_removed: u64,
}

/// Why [`BlobStore::rechunk`] refused, or failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RechunkError {
    /// [`super::BlobConfig::chunked_versions`] is off: nothing was changed.
    #[error("rechunk refused: chunked versions are off (BlobConfig::chunked_versions)")]
    ChunkingOff,
    /// The doc isn't a tier-0 doc of that space.
    #[error("no tier-0 doc {doc} in space {space}")]
    NoDoc { space: String, doc: DocId },
    #[error(transparent)]
    Blob(#[from] BlobError),
}

impl From<StoreError> for RechunkError {
    fn from(e: StoreError) -> Self {
        Self::Blob(e.into())
    }
}

impl BlobStore {
    /// Convert doc `doc`'s whole-file versions in `space` to chunked ones, on request (the
    /// host's maintenance call; nothing converts on its own). Each distinct whole file of the
    /// doc at or over [`super::BlobConfig::chunk_min_bytes`] is read back (verified), cut and
    /// written as a chunked write is (the doc's internal lock, waiting marks, one new segment:
    /// temp file, synced, renamed, its folder synced), and committed in **its own
    /// transaction**: the manifest and chunk rows, and each version naming those bytes moving
    /// its reference from the whole-file blob to the manifest. Every version's file id is
    /// unchanged. **The whole-file blob it dropped is removed at once** (not after the `gc`
    /// grace) when nothing else references it and no put waits on it: another doc of the
    /// space holding the same bytes keeps it. Versions under the minimum stay whole; purged
    /// ones are skipped. The first conversion stamps the store format 8 with the chunked
    /// feature, if it isn't yet.
    ///
    /// **The cheap set** is the one the kit uses for a write it makes for no connection, the
    /// doc's current version, plus the versions this rechunk has converted already: a version
    /// converted later reuses the chunks it shares with them. That opens no guess of history:
    /// the chunks it reuses are of versions the doc already holds, which this call itself just
    /// wrote, and the caller is the host (an owner's maintenance call, made for no
    /// connection), which may read every version anyway; nothing it reports reaches a
    /// connection.
    ///
    /// **Never waits on a write**: a version whose doc lock a write holds is skipped and
    /// reported ([`RechunkReport::busy`]); a later rechunk converts it. **A stop at any step**
    /// leaves each version whole either way: before the commit it is still a whole-file
    /// version (its segment, with no row, goes at the next open or `gc`); after it, chunked,
    /// its old blob at no references (`gc` takes it, and so does the next rechunk, at once).
    ///
    /// Cut by content only: the blob store has no pack, so a pack's hints
    /// (`BlobPack::chunk_hints`) don't apply to a converted version. Refused with
    /// [`RechunkError::ChunkingOff`] when chunked versions are off, changing nothing. There is
    /// no way back yet (rebuilding whole files is a later maintenance call); the stamp stays.
    pub fn rechunk(&self, space: &str, doc: DocId) -> Result<RechunkReport, RechunkError> {
        if !self.config.chunked_versions {
            return Err(RechunkError::ChunkingOff);
        }
        let no_doc = || RechunkError::NoDoc {
            space: space.to_owned(),
            doc,
        };
        let meta = match self.docs.meta(doc) {
            Ok(m) => m,
            Err(StoreError::NoDoc(_)) => return Err(no_doc()),
            Err(e) => return Err(e.into()),
        };
        if meta.pack != BLOB_DOC_PACK || meta.key.space != space {
            return Err(no_doc());
        }
        let Some(k) = self.docs.blob_space(space)? else {
            return Err(no_doc());
        };
        let versions = self.docs.blob_versions(doc)?;
        let current = versions.last().map(|(_, v)| v.blob);
        let mut rep = RechunkReport::default();
        // each distinct file, in the order the versions first name it, with how many name it
        let mut files: Vec<(BlobId, u64, u64)> = Vec::new();
        for (_, v) in &versions {
            if v.redacted {
                rep.redacted += 1;
                continue;
            }
            match files.iter_mut().find(|(id, _, _)| *id == v.blob) {
                Some(e) => e.2 += 1,
                None => files.push((v.blob, v.size, 1)),
            }
        }
        let mut done: Vec<BlobId> = Vec::new();
        for &(file, size, n) in &files {
            if size < self.config.chunk_min_bytes {
                rep.under_minimum += n;
                continue;
            }
            if self.is_chunked(doc, &file)? {
                rep.already_chunked += n;
                // a whole-file blob of these bytes at no references (a stop after an earlier
                // rechunk's commit left it) goes now
                if self
                    .docs
                    .blob_row(k.no, &file)?
                    .is_some_and(|r| r.refs == 0)
                {
                    rep.blobs_removed += self.remove_unreferenced_now(&[(k.no, file, size)])?.0;
                }
                continue;
            }
            if self.docs.blob_row(k.no, &file)?.is_none() {
                rep.missing += n;
                continue;
            }
            let Some(guard) = self.doc_locks.take(doc, false) else {
                rep.busy += n;
                continue;
            };
            // under the lock: a write before it may have stored these bytes chunked
            if self.is_chunked(doc, &file)? {
                rep.already_chunked += n;
                continue;
            }
            let (key, new_key) = match self.docs.chunk_read(|r| r.key(doc))? {
                Some(key) => (key, false),
                None => (self.first_key(size)?, true),
            };
            let cheap =
                CheapSet::Versions(current.into_iter().chain(done.iter().copied()).collect());
            let mut src = self.reader(space, &file)?;
            let put = self.put_chunked(
                space,
                doc,
                Src::Stream(&mut src),
                key,
                new_key,
                Some(guard),
                &cheap,
            )?;
            drop(src);
            let info = put.info();
            let Some(c) = put.chunked() else {
                return Err(BlobError::Io("a rechunk's write isn't chunked".into()).into());
            };
            if info.id != file || info.size != size {
                return Err(BlobError::Damaged(format!("{file}: read back as other bytes")).into());
            }
            let moved = self.docs.blob_rechunk_commit(space, c)?;
            // the marks and the doc's lock go after the commit
            drop(put);
            if moved == 0 {
                continue;
            }
            rep.converted += moved;
            rep.bytes += size;
            done.push(file);
            if !planted(Plant::RechunkKeepsOldBlob) {
                rep.blobs_removed += self.remove_unreferenced_now(&[(k.no, file, size)])?.0;
            }
            #[cfg(any(test, feature = "differential"))]
            if crate::differential::chunk_hook(crate::differential::ChunkStep::OldBlobRemoved) {
                return Err(BlobError::Io(
                    "stopped after the old blob's removal (test hook)".into(),
                )
                .into());
            }
        }
        Ok(rep)
    }
}
