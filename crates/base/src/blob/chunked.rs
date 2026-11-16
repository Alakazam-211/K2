//! Chunked tier-0 versions in the blob store (`docs/design/version-storage-r2.md`, sections 1,
//! 5, 7 and 9; `docs/SESSIONS.md`, "Chunked versions"): the writer, the waiting marks and the
//! doc's lock, the segment sweep, and the reader.
//!
//! **The writer** ([`BlobStore::put_version`]) takes the doc's lock (internal and short: never
//! the person's lease), cuts the bytes with the doc's chunker, marks every chunk id it names
//! waiting (new or stored, before it looks whether it may reuse it), writes the chunks it may
//! not reuse to **one** new segment (a temp file, synced, its name marked waiting, renamed into
//! place, its folder synced; a folder it makes has its parent synced), and hands the version's
//! commit ([`ChunkCommit`]) to the store, which commits it in the version's own transaction.
//! The marks and the lock are released when the put is dropped: after the commit, or the
//! refusal.
//!
//! **The cheap path** is narrow and given, never assumed: a write reuses (counts, writing no
//! byte) only the chunks of the versions its [`CheapSet`] names, which the session works out
//! from the writing connection's rights (every version's with the history right, else the
//! current version's) and the store resolves to chunk ids ([`may_reuse`] is the one place the
//! rule is applied). Every other chunk is written again, a copy no chunk row points at, so the
//! work of a write never says whether its bytes are in a history its writer can't read.
//!
//! **The reader** takes the manifest and the chunk rows in one read transaction, opens every
//! segment file it needs before the first byte streams (an open file stays readable after an
//! unlink; a segment missing at that point means the rows are read again, once), and reads
//! ahead on a thread into a bounded channel. Every chunk is checked against its keyed id before
//! its bytes are handed out, and the whole file against its id at the end.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};

use super::cdc::{check_hints, Cdc, Sizes};
use super::{io_err, BlobError, BlobId, BlobInfo, BlobStore, BUF, TMP};
use crate::hash::HmacSha256;
use crate::store::{
    chunk_id, enc_entries, read_at_file, read_entries, segment_dir, segment_path, ChunkCommit,
    ChunkId, ChunkKey, ChunkRead, DocId, EntriesError, SegmentList, SEGMENTS,
};

/// How many chunks the reader reads ahead of its caller.
const READ_AHEAD: usize = 4;

/// What is waiting, in memory, under the blob store's lock: a chunked write's segment file name
/// from before its rename to its release, and every chunk id it names. `gc` and the removals at
/// once leave what is waiting. After a restart nothing is.
#[derive(Default)]
pub(super) struct Marks {
    /// (space no, doc, segment) → how many writes wait on it.
    segments: HashMap<(u64, DocId, u64), u32>,
    /// (doc, chunk id) → how many writes wait on it.
    chunks: HashMap<(DocId, ChunkId), u32>,
}

impl Marks {
    pub(super) fn segment_waiting(&self, no: u64, doc: DocId, seg: u64) -> bool {
        self.segments.contains_key(&(no, doc, seg))
    }

    /// Whether a write waits on chunk `id` of doc `doc`.
    pub(super) fn chunk_waiting(&self, doc: DocId, id: &ChunkId) -> bool {
        self.chunks.contains_key(&(doc, *id))
    }

    /// Mark segment `seg` of doc `doc` (space `no`) waiting: a compaction's output, from before
    /// its rename to its commit.
    pub(super) fn mark_segment(&mut self, no: u64, doc: DocId, seg: u64) {
        *self.segments.entry((no, doc, seg)).or_insert(0) += 1;
    }

    /// Release a mark [`Marks::mark_segment`] took.
    pub(super) fn release_segment(&mut self, no: u64, doc: DocId, seg: u64) {
        release(&mut self.segments, (no, doc, seg));
    }
}

/// The docs' internal locks: one chunked writer per doc at a time (and later each purge and
/// compaction). Not the person's lease.
#[derive(Default)]
pub(super) struct DocLocks {
    held: Mutex<HashSet<DocId>>,
    freed: Condvar,
}

/// A doc's lock, released on drop.
pub(crate) struct DocGuard {
    locks: Arc<DocLocks>,
    doc: DocId,
}

impl Drop for DocGuard {
    fn drop(&mut self) {
        let mut held = self.locks.held.lock().unwrap_or_else(|p| p.into_inner());
        held.remove(&self.doc);
        self.locks.freed.notify_all();
    }
}

impl DocLocks {
    /// Doc `doc`'s lock, waiting for it.
    pub(crate) fn take_waiting(self: &Arc<Self>, doc: DocId) -> DocGuard {
        self.take(doc, true)
            .expect("a waiting take returns the lock")
    }

    /// Doc `doc`'s lock: waiting for it when `wait`, else `None` when another holds it.
    pub(super) fn take(self: &Arc<Self>, doc: DocId, wait: bool) -> Option<DocGuard> {
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        while held.contains(&doc) {
            if !wait {
                return None;
            }
            held = self.freed.wait(held).unwrap_or_else(|p| p.into_inner());
        }
        held.insert(doc);
        Some(DocGuard {
            locks: self.clone(),
            doc,
        })
    }
}

/// How a version write takes the doc's lock ([`BlobStore::put_version`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockWait {
    /// Wait for it (a caller holding no session lock).
    Wait,
    /// Take it if free; else the version is written as a whole-file blob (a caller holding a
    /// session lock never waits on a doc's lock, so no lock order can cross).
    Try,
}

/// A version's bytes, stored: a whole-file blob, or chunks waiting for their commit.
pub(crate) enum VersionPut {
    Whole(BlobInfo),
    Chunked(Box<ChunkedPut>),
}

impl VersionPut {
    pub(crate) fn info(&self) -> BlobInfo {
        match self {
            Self::Whole(i) => *i,
            Self::Chunked(c) => c.info,
        }
    }

    /// The commit a chunked put hands the store with its version.
    pub(crate) fn chunked(&self) -> Option<&ChunkCommit> {
        match self {
            Self::Whole(_) => None,
            Self::Chunked(c) => Some(&c.commit),
        }
    }

    /// The chunked put itself.
    pub(crate) fn as_chunked(&self) -> Option<&ChunkedPut> {
        match self {
            Self::Whole(_) => None,
            Self::Chunked(c) => Some(c),
        }
    }
}

/// A chunked write between its segment and its commit: its marks and the doc's lock are held
/// until it is dropped.
pub(crate) struct ChunkedPut {
    pub(crate) commit: ChunkCommit,
    info: BlobInfo,
    no: u64,
    marked_chunks: Vec<ChunkId>,
    marked_segment: Option<u64>,
    lock: Arc<Mutex<()>>,
    marks: Arc<Mutex<Marks>>,
    /// `None` for a put made under another's lock ([`BlobStore::put_version_after`]).
    _doc: Option<DocGuard>,
}

impl Drop for ChunkedPut {
    fn drop(&mut self) {
        {
            let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
            let mut m = self.marks.lock().unwrap_or_else(|p| p.into_inner());
            let doc = self.commit.doc;
            if let Some(seg) = self.marked_segment {
                release(&mut m.segments, (self.no, doc, seg));
            }
            for id in &self.marked_chunks {
                release(&mut m.chunks, (doc, *id));
            }
        }
        #[cfg(any(test, feature = "differential"))]
        crate::differential::chunk_hook(crate::differential::ChunkStep::MarksReleased);
    }
}

fn release<K: std::hash::Hash + Eq>(m: &mut HashMap<K, u32>, k: K) {
    if let Some(n) = m.get_mut(&k) {
        *n = n.saturating_sub(1);
        if *n == 0 {
            m.remove(&k);
        }
    }
}

/// The planted faults this module reads (`crate::differential::ChunkPlant`).
#[derive(Clone, Copy)]
pub(super) enum Plant {
    GcIgnoresWaitingSegment,
    CheapPathBeyondCurrent,
    ReaderOpensLazily,
    ReaderSkipsChunkCheck,
    HistoryChunkCheap,
    ManifestReuseSkipsFullWrite,
    RechunkKeepsOldBlob,
    HintsRequired,
    HintsUnmerged,
}

/// Whether `p` is planted on this thread (test builds only).
pub(super) fn planted(p: Plant) -> bool {
    // a build without the plants still reads `p` (no unused variable there)
    let _ = p;
    plant_on!({
        use crate::differential::ChunkPlant as C;
        let want = match p {
            Plant::GcIgnoresWaitingSegment => C::GcIgnoresWaitingSegment,
            Plant::CheapPathBeyondCurrent => C::CheapPathBeyondCurrent,
            Plant::ReaderOpensLazily => C::ReaderOpensLazily,
            Plant::ReaderSkipsChunkCheck => C::ReaderSkipsChunkCheck,
            Plant::HistoryChunkCheap => C::HistoryChunkCheap,
            Plant::ManifestReuseSkipsFullWrite => C::ManifestReuseSkipsFullWrite,
            Plant::RechunkKeepsOldBlob => C::RechunkKeepsOldBlob,
            Plant::HintsRequired => C::HintsRequired,
            Plant::HintsUnmerged => C::HintsUnmerged,
        };
        crate::differential::chunk_plant() == Some(want)
    })
}

/// The versions whose chunks a chunked write may take on the cheap path
/// (`docs/design/version-storage-r2.md`, section 6): worked out by the session from the rights
/// of the connection it writes for, at each write, and handed to the store with the bytes. The
/// store never decides rights: it resolves the set to chunk ids and refuses the cheap path for
/// every id outside it ([`may_reuse`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheapSet {
    /// The chunks (their manifests' own included) of these versions of the doc, by file id: for
    /// a connection without the history right, the current version and a base it may read; for
    /// a write the kit makes for no connection, the current version. Empty: no chunk (a new
    /// doc's first version).
    Versions(Vec<BlobId>),
    /// The chunks of every version of the doc: a connection with the history right, which can
    /// read them all anyway (a restore of a large version writes no duplicate).
    History,
}

impl CheapSet {
    /// No chunk: every byte of the write is written.
    pub(crate) fn none() -> Self {
        Self::Versions(Vec::new())
    }
}

/// Where a chunked write may cut besides its content: a tier-0 pack's hints
/// (`BlobPack::chunk_hints`), handed to the store with the bytes. Pure: the same bytes give the
/// same offsets. The store checks them before it uses one ([`check_hints`]).
pub(crate) trait HintSource {
    /// The largest version [`HintSource::hints`] is asked for (read into memory); 0: never.
    fn up_to(&self) -> u64;
    /// The offsets for `bytes`.
    fn hints(&self, bytes: &[u8]) -> Vec<u64>;
}

/// A chunked write's bytes: streamed (cut by content only), or in memory with the pack's hints
/// (unchecked).
pub(super) enum Src<'a> {
    Stream(&'a mut dyn Read),
    Mem(&'a [u8], Vec<u64>),
}

/// The chunks of a write's bytes, in order: a [`Window`] over a stream, or the cut of bytes in
/// memory.
enum Cuts<'a> {
    Stream(Window<'a>),
    Mem {
        b: &'a [u8],
        cut: Vec<(usize, usize)>,
        at: usize,
    },
}

impl Cuts<'_> {
    fn next(&mut self, cdc: &Cdc) -> Result<Option<&[u8]>, BlobError> {
        match self {
            Self::Stream(w) => w.next(cdc),
            Self::Mem { b, cut, at } => {
                let Some(&(o, n)) = cut.get(*at) else {
                    return Ok(None);
                };
                *at += 1;
                Ok(Some(&b[o..o + n]))
            }
        }
    }
}

/// A [`CheapSet`] as a write checks its chunks against it ([`BlobStore::resolve`]).
pub(crate) enum Cheap {
    /// These chunk ids: those of the versions a [`CheapSet::Versions`] names.
    Ids(HashSet<ChunkId>),
    /// Every chunk a version of the doc names ([`CheapSet::History`]): a chunk row with a
    /// reference (a chunk's count is the manifests naming it), looked up per chunk, so the set
    /// is never built from every manifest.
    Referenced,
}

/// Whether a write may take chunk `id` on the cheap path (its count raised, no byte written):
/// only when it is in `cheap` and its row is there. `refs` reads the chunk's row (its count;
/// `None`: no row), after the write marked it waiting. The one place the rule is applied.
pub(crate) fn may_reuse(cheap: &Cheap, id: &ChunkId, refs: impl FnOnce() -> Option<u64>) -> bool {
    if planted(Plant::CheapPathBeyondCurrent) {
        return refs().is_some();
    }
    match cheap {
        Cheap::Ids(ids) => ids.contains(id) && refs().is_some(),
        Cheap::Referenced => refs().is_some_and(|n| n > 0),
    }
}

/// A reader over a chunker's input that keeps at least a chunk's maximum ahead, so its cuts are
/// those [`Cdc::chunks`] makes over the whole input.
struct Window<'a> {
    src: &'a mut dyn Read,
    buf: Vec<u8>,
    at: usize,
    eof: bool,
    max: usize,
}

impl<'a> Window<'a> {
    fn new(src: &'a mut dyn Read, max: usize) -> Self {
        Self {
            src,
            buf: Vec::with_capacity(2 * max + BUF),
            at: 0,
            eof: false,
            max,
        }
    }

    /// The next chunk's bytes (`None` at the end).
    fn next(&mut self, cdc: &Cdc) -> Result<Option<&[u8]>, BlobError> {
        if self.at > 0 && self.buf.len() - self.at < self.max {
            self.buf.drain(..self.at);
            self.at = 0;
        }
        while !self.eof && self.buf.len() - self.at < self.max {
            let have = self.buf.len();
            self.buf.resize(have + BUF, 0);
            let n = loop {
                match self.src.read(&mut self.buf[have..]) {
                    Ok(n) => break n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        self.buf.truncate(have);
                        return Err(io_err("reading the version's source", e));
                    }
                }
            };
            self.buf.truncate(have + n);
            self.eof = n == 0;
        }
        if self.at == self.buf.len() {
            return Ok(None);
        }
        let n = cdc.cut(&self.buf[self.at..]);
        let out = &self.buf[self.at..self.at + n];
        self.at += n;
        Ok(Some(out))
    }
}

/// The segment a write is filling: its temp file, opened at its first new chunk.
struct Seg {
    tmp: PathBuf,
    f: Option<File>,
    list: SegmentList,
    len: u64,
    written: HashSet<ChunkId>,
}

impl Seg {
    fn put(&mut self, id: ChunkId, b: &[u8]) -> Result<(), BlobError> {
        if !self.written.insert(id) {
            return Ok(());
        }
        if self.f.is_none() {
            let f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.tmp)
                .map_err(|e| io_err("segment temp file", e))?;
            self.f = Some(f);
        }
        let f = self.f.as_mut().expect("opened above");
        f.write_all(b).map_err(|e| io_err("writing a segment", e))?;
        let len = u32::try_from(b.len()).expect("a chunk is at most 1 MiB");
        self.list.push((id, self.len, len));
        self.len += b.len() as u64;
        Ok(())
    }
}

/// A write stopped by a test hook (test builds only): the error, nothing cleaned up.
fn stop_at(step: StepShim) -> Result<(), BlobError> {
    if step.stop() {
        return Err(BlobError::Io(format!(
            "stopped at {} (test hook)",
            step.name()
        )));
    }
    Ok(())
}

/// A write step a test hook can stop at.
#[derive(Clone, Copy)]
enum StepShim {
    TempWritten,
    Renamed,
    FolderSynced,
}

impl StepShim {
    fn name(self) -> &'static str {
        match self {
            Self::TempWritten => "temp written",
            Self::Renamed => "renamed",
            Self::FolderSynced => "folder synced",
        }
    }

    #[cfg(any(test, feature = "differential"))]
    fn stop(self) -> bool {
        use crate::differential::{chunk_hook, ChunkStep};
        chunk_hook(match self {
            Self::TempWritten => ChunkStep::TempWritten,
            Self::Renamed => ChunkStep::Renamed,
            Self::FolderSynced => ChunkStep::FolderSynced,
        })
    }

    #[cfg(not(any(test, feature = "differential")))]
    fn stop(self) -> bool {
        false
    }
}

/// Create `dir` and every missing folder above it, syncing the parent of each one made, so a
/// commit never outlives the folder entry of a file it names.
pub(super) fn ensure_dir(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        ensure_dir(parent)?;
    }
    match fs::create_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(()),
        Err(e) => return Err(e),
    }
    if let Some(parent) = dir.parent() {
        File::open(parent)?.sync_all()?;
        chunk_work!(folder_syncs);
    }
    Ok(())
}

/// Where a chunk is, for a read.
struct Located {
    id: ChunkId,
    segment: u64,
    offset: u64,
    len: u32,
}

/// A version's chunks, from one read of the rows.
struct Layout {
    key: [u8; 32],
    size: u64,
    chunks: Vec<Located>,
}

/// Why a layout couldn't be read or opened.
enum OpenError {
    /// A segment file is missing: the rows are read again, once.
    Missing,
    Blob(BlobError),
}

impl From<BlobError> for OpenError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

impl From<crate::store::StoreError> for OpenError {
    fn from(e: crate::store::StoreError) -> Self {
        Self::Blob(e.into())
    }
}

impl BlobStore {
    /// Store a tier-0 version's bytes for doc `doc` (`None`: a doc about to be created, whose
    /// id is reserved here if the version is chunked). With [`super::BlobConfig::chunked_versions`]
    /// on and the bytes at least [`super::BlobConfig::chunk_min_bytes`], they are stored as
    /// chunks (module docs), the doc's lock taken as `wait` says; otherwise, or when the lock
    /// is busy, as a whole-file blob ([`BlobStore::put`]). Hand the put's commit to the store
    /// with the version, then [`BlobStore::finish`] it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn put_version(
        &self,
        space: &str,
        doc: Option<DocId>,
        src: &mut dyn Read,
        wait: LockWait,
        cheap: &CheapSet,
        hints: &dyn HintSource,
    ) -> Result<VersionPut, BlobError> {
        if !self.config.chunked_versions {
            return self.put(space, src).map(VersionPut::Whole);
        }
        let min = self.config.chunk_min_bytes;
        // a version the pack gives hints for is read into memory whole (up to its bound): the
        // hints are offsets into it
        let up_to = hints.up_to().min(self.config.max_blob_bytes);
        let hinted = up_to >= min;
        let mut head = Vec::new();
        (&mut *src)
            .take(if hinted { up_to.saturating_add(1) } else { min })
            .read_to_end(&mut head)
            .map_err(|e| io_err("reading the version's source", e))?;
        if (head.len() as u64) < min {
            return self.put_bytes(space, &head).map(VersionPut::Whole);
        }
        if hinted && (head.len() as u64) <= up_to {
            return self.put_version_bytes(space, doc, &head, wait, cheap, hints);
        }
        let doc = match doc {
            Some(d) => d,
            None => self.docs.reserve_doc_id()?,
        };
        let Some(guard) = self.doc_locks.take(doc, wait == LockWait::Wait) else {
            self.busy_fallbacks.fetch_add(1, Ordering::Relaxed);
            return self
                .put(space, &mut (&head[..]).chain(src))
                .map(VersionPut::Whole);
        };
        let key = self.docs.chunk_read(|r| r.key(doc))?;
        match key {
            Some(k) => self.put_chunked(
                space,
                doc,
                Src::Stream(&mut (&head[..]).chain(src)),
                k,
                false,
                Some(guard),
                cheap,
            ),
            None => {
                // the doc's first chunked version: its sizes come from its length, so the bytes
                // are spooled to learn it
                let (mut f, tmp, len) = self.spool(&head, src)?;
                let out = self.first_key(len).and_then(|k| {
                    f.seek_start()?;
                    self.put_chunked(space, doc, Src::Stream(&mut f), k, true, Some(guard), cheap)
                });
                drop(f);
                let _ = fs::remove_file(&tmp);
                out
            }
        }
    }

    /// [`BlobStore::put_version`] from memory: `hints` are asked when the bytes are within
    /// their bound.
    pub(crate) fn put_version_bytes(
        &self,
        space: &str,
        doc: Option<DocId>,
        bytes: &[u8],
        wait: LockWait,
        cheap: &CheapSet,
        hints: &dyn HintSource,
    ) -> Result<VersionPut, BlobError> {
        if !self.config.chunked_versions || (bytes.len() as u64) < self.config.chunk_min_bytes {
            return self.put_bytes(space, bytes).map(VersionPut::Whole);
        }
        let doc = match doc {
            Some(d) => d,
            None => self.docs.reserve_doc_id()?,
        };
        let Some(guard) = self.doc_locks.take(doc, wait == LockWait::Wait) else {
            self.busy_fallbacks.fetch_add(1, Ordering::Relaxed);
            return self.put_bytes(space, bytes).map(VersionPut::Whole);
        };
        let (key, new) = match self.docs.chunk_read(|r| r.key(doc))? {
            Some(k) => (k, false),
            None => (self.first_key(bytes.len() as u64)?, true),
        };
        let src = Src::Mem(bytes, Self::hints_for(bytes, hints));
        self.put_chunked(space, doc, src, key, new, Some(guard), cheap)
    }

    /// The pack's hints for `bytes`, when they are within its bound.
    fn hints_for(bytes: &[u8], hints: &dyn HintSource) -> Vec<u64> {
        if (bytes.len() as u64) <= hints.up_to() {
            hints.hints(bytes)
        } else {
            Vec::new()
        }
    }

    /// A second version's bytes for the doc `prev` is writing, under `prev`'s lock, committed
    /// in the same transaction (a merge recorded with the save it merges): its key, and the
    /// segment after `prev`'s. A whole-file blob when `bytes` are under the minimum.
    pub(crate) fn put_version_after(
        &self,
        space: &str,
        prev: &ChunkedPut,
        bytes: &[u8],
        cheap: &CheapSet,
        hints: &dyn HintSource,
    ) -> Result<VersionPut, BlobError> {
        if (bytes.len() as u64) < self.config.chunk_min_bytes {
            return self.put_bytes(space, bytes).map(VersionPut::Whole);
        }
        let mut key = prev.commit.key.clone();
        key.next_segment = prev
            .commit
            .segment
            .as_ref()
            .map_or(key.next_segment, |(n, _)| n + 1);
        let new = prev.commit.new_key;
        self.put_chunked(
            space,
            prev.commit.doc,
            Src::Mem(bytes, Self::hints_for(bytes, hints)),
            key,
            new,
            None,
            cheap,
        )
    }

    /// Release a put: a whole-file put stops waiting ([`BlobStore::settle`]); a chunked one
    /// releases its marks and the doc's lock.
    pub(crate) fn finish(&self, space: &str, put: VersionPut) {
        match put {
            VersionPut::Whole(i) => self.settle(space, &i.id),
            VersionPut::Chunked(c) => drop(c),
        }
    }

    /// A new doc chunk key for a first chunked version of `len` bytes.
    pub(super) fn first_key(&self, len: u64) -> Result<ChunkKey, BlobError> {
        Ok(ChunkKey {
            key: super::random_key()?,
            sizes: Sizes::for_first(len).code(),
            next_segment: 1,
        })
    }

    /// `head` and the rest of `src` in a temp file (not synced: it is read back and removed).
    fn spool(&self, head: &[u8], src: &mut dyn Read) -> Result<(Spool, PathBuf, u64), BlobError> {
        static N: AtomicU64 = AtomicU64::new(0);
        let tmp = self.dir.join(TMP).join(format!(
            "{}-{}.spool",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let out = (|| -> Result<(Spool, u64), BlobError> {
            let mut f = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&tmp)
                .map_err(|e| io_err("spool file", e))?;
            f.write_all(head).map_err(|e| io_err("spooling", e))?;
            let mut len = head.len() as u64;
            let mut buf = vec![0u8; BUF];
            loop {
                let n = match src.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(io_err("reading the version's source", e)),
                };
                len += n as u64;
                if len > self.config.max_blob_bytes {
                    return Err(BlobError::TooLarge {
                        limit: self.config.max_blob_bytes,
                    });
                }
                f.write_all(&buf[..n]).map_err(|e| io_err("spooling", e))?;
            }
            Ok((Spool(f), len))
        })();
        match out {
            Ok((f, len)) => Ok((f, tmp, len)),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    /// What a write of doc `doc` checks its chunks against for `set` ([`may_reuse`]): the chunk
    /// ids (their manifests' own included) of the versions a [`CheapSet::Versions`] names (one
    /// stored whole, or gone, adds none), or every chunk a version names for
    /// [`CheapSet::History`].
    fn resolve(&self, no: u64, doc: DocId, set: &CheapSet) -> Result<Cheap, BlobError> {
        let ids = match set {
            CheapSet::History => return Ok(Cheap::Referenced),
            _ if planted(Plant::HistoryChunkCheap) => return Ok(Cheap::Referenced),
            CheapSet::Versions(ids) => ids,
        };
        self.docs.chunk_read(|r| {
            let Some(k) = r.key(doc)? else {
                return Ok(Cheap::Ids(HashSet::new()));
            };
            let get = |id: &ChunkId| r.chunk(doc, id);
            let mut out = HashSet::new();
            for id in ids {
                let Some(m) = r.manifest(doc, id)? else {
                    continue;
                };
                let entries =
                    read_entries(&get, &self.dir, no, doc, &k.key, &m.root).map_err(entries_err)?;
                out.extend(entries.into_iter().map(|e| e.0));
                out.extend(m.root.iter().map(|e| e.0));
            }
            Ok(Cheap::Ids(out))
        })
    }

    /// The chunked write (module docs): `src`'s bytes cut and placed under `key`, reusing only
    /// the chunks of the versions `set` names.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn put_chunked(
        &self,
        space: &str,
        doc: DocId,
        src: Src<'_>,
        key: ChunkKey,
        new_key: bool,
        guard: Option<DocGuard>,
        set: &CheapSet,
    ) -> Result<VersionPut, BlobError> {
        static N: AtomicU64 = AtomicU64::new(0);
        let k = self.key(space)?;
        let sizes = Sizes::from_code(key.sizes).ok_or(BlobError::Store(
            crate::store::StoreError::Corrupt("chunk sizes"),
        ))?;
        let cdc = Cdc::new(sizes);
        let cheap = if new_key {
            Cheap::Ids(HashSet::new())
        } else {
            self.resolve(k.no, doc, set)?
        };
        let seg_no = key.next_segment;
        let mut put = ChunkedPut {
            commit: ChunkCommit {
                doc,
                file: BlobId([0; 32]),
                size: 0,
                key: key.clone(),
                new_key,
                segment: None,
                entries: Vec::new(),
                root: Vec::new(),
                reused: Vec::new(),
                now: (self.clock)(),
            },
            info: BlobInfo {
                id: BlobId([0; 32]),
                size: 0,
                new: true,
            },
            no: k.no,
            marked_chunks: Vec::new(),
            marked_segment: None,
            lock: self.lock.clone(),
            marks: self.marks.clone(),
            _doc: guard,
        };
        let mut seg = Seg {
            tmp: self.dir.join(TMP).join(format!(
                "{}-{}.seg.part",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            )),
            f: None,
            list: Vec::new(),
            len: 0,
            written: HashSet::new(),
        };
        let cuts = match src {
            Src::Stream(r) => Cuts::Stream(Window::new(r, cdc.max())),
            Src::Mem(b, raw) => Cuts::Mem {
                b,
                cut: self.cut_at_hints(&cdc, b, &raw),
                at: 0,
            },
        };
        let written = self.cut_and_place(&cdc, cuts, &key.key, &k.key, &cheap, &mut put, &mut seg);
        let (file, size) = match written {
            Ok(v) => v,
            Err(e) => {
                drop(seg.f.take());
                let _ = fs::remove_file(&seg.tmp);
                return Err(e);
            }
        };
        put.commit.file = file;
        put.commit.size = size;
        let stored = self
            .docs
            .chunk_read(|r| r.manifest(doc, &file).map(|m| m.is_some()))?;
        // "already stored" only for the bytes of a version the cheap set names: for any other,
        // what a brand-new file's write says
        let named = match set {
            CheapSet::History => true,
            CheapSet::Versions(ids) => ids.contains(&file),
        };
        put.info = BlobInfo {
            id: file,
            size,
            new: !(stored && named),
        };
        if stored && planted(Plant::ManifestReuseSkipsFullWrite) {
            // the planted fault: bytes a stored manifest names are taken whole, nothing written
            drop(seg.f.take());
            let _ = fs::remove_file(&seg.tmp);
            put.commit.reused.extend(seg.written.drain());
            seg.list.clear();
        }
        if let Some(f) = seg.f.take() {
            if let Err(e) = f.sync_all() {
                let _ = fs::remove_file(&seg.tmp);
                return Err(io_err("syncing a segment", e));
            }
            chunk_work!(segment_bytes, seg.len);
            chunk_work!(file_syncs);
            drop(f);
            stop_at(StepShim::TempWritten)?;
            let path = segment_path(&self.dir, k.no, doc, seg_no);
            let folder = path.parent().expect("a segment has a folder").to_owned();
            {
                // the name waits before it exists, and the rename runs under the lock `gc`
                // sweeps under, so `gc` never sees the file without its mark
                let _g = self.guard();
                {
                    let mut m = self.marks();
                    *m.segments.entry((k.no, doc, seg_no)).or_insert(0) += 1;
                }
                put.marked_segment = Some(seg_no);
                let placed = ensure_dir(&folder).and_then(|()| fs::rename(&seg.tmp, &path));
                if let Err(e) = placed {
                    let _ = fs::remove_file(&seg.tmp);
                    return Err(io_err("placing a segment", e));
                }
            }
            stop_at(StepShim::Renamed)?;
            File::open(&folder)
                .and_then(|d| d.sync_all())
                .map_err(|e| io_err("syncing a segment's folder", e))?;
            chunk_work!(folder_syncs);
            stop_at(StepShim::FolderSynced)?;
            put.commit.segment = Some((seg_no, std::mem::take(&mut seg.list)));
        }
        Ok(VersionPut::Chunked(Box::new(put)))
    }

    /// Cut `src`, mark each chunk waiting, reuse or write it, then the manifest's own chunks;
    /// returns the file's id (under `space_key`) and size.
    #[allow(clippy::too_many_arguments)]
    fn cut_and_place(
        &self,
        cdc: &Cdc,
        mut cuts: Cuts<'_>,
        chunk_key: &[u8; 32],
        space_key: &[u8; 32],
        cheap: &Cheap,
        put: &mut ChunkedPut,
        seg: &mut Seg,
    ) -> Result<(BlobId, u64), BlobError> {
        let doc = put.commit.doc;
        let mut mac = HmacSha256::new(space_key);
        let mut size: u64 = 0;
        let mut entries = Vec::new();
        while let Some(b) = cuts.next(cdc)? {
            size += b.len() as u64;
            if size > self.config.max_blob_bytes {
                return Err(BlobError::TooLarge {
                    limit: self.config.max_blob_bytes,
                });
            }
            mac.update(b);
            let id = chunk_id(chunk_key, b);
            self.place(doc, id, b, cheap, put, seg)?;
            entries.push((
                id,
                u32::try_from(b.len()).expect("a chunk is at most 1 MiB"),
            ));
        }
        let listed = enc_entries(&entries);
        let mut root = Vec::new();
        for (at, n) in cdc.chunks(&listed) {
            let b = &listed[at..at + n];
            let id = chunk_id(chunk_key, b);
            self.place(doc, id, b, cheap, put, seg)?;
            root.push((id, u32::try_from(n).expect("a chunk is at most 1 MiB")));
        }
        put.commit.entries = entries;
        put.commit.root = root;
        Ok((BlobId(mac.finish()), size))
    }

    /// The cut of `b` in memory: at the pack's hints `raw` once checked, merged and capped
    /// ([`check_hints`]; ignored, and counted, when one is bad), by content between them; with
    /// no hints, the chunker's cut.
    fn cut_at_hints(&self, cdc: &Cdc, b: &[u8], raw: &[u64]) -> Vec<(usize, usize)> {
        if raw.is_empty() {
            return cdc.chunks(b);
        }
        let merge = !planted(Plant::HintsUnmerged);
        match check_hints(raw, b.len(), cdc.min(), merge) {
            Some(cuts) => cdc.chunks_at(b, &cuts, !planted(Plant::HintsRequired)),
            None => {
                self.hints_ignored.fetch_add(1, Ordering::Relaxed);
                cdc.chunks(b)
            }
        }
    }

    /// One chunk of a write: marked waiting first, then reused (no bytes) when the cheap path
    /// allows it, else written to the write's segment (once per write).
    fn place(
        &self,
        doc: DocId,
        id: ChunkId,
        b: &[u8],
        cheap: &Cheap,
        put: &mut ChunkedPut,
        seg: &mut Seg,
    ) -> Result<(), BlobError> {
        {
            let _g = self.guard();
            *self.marks().chunks.entry((doc, id)).or_insert(0) += 1;
        }
        put.marked_chunks.push(id);
        let refs = || {
            self.docs
                .chunk_read(|r| r.chunk(doc, &id).map(|c| c.map(|c| c.refs)))
                .unwrap_or(None)
        };
        // the chunk's row is looked at after its mark: a remover that ran before the mark has
        // left it gone (the chunk is written), and one after it keeps it
        if !seg.written.contains(&id) && may_reuse(cheap, &id, refs) {
            put.commit.reused.push(id);
            return Ok(());
        }
        seg.put(id, b)
    }

    /// Whether doc `doc` holds version bytes `id` chunked (their manifest is there).
    pub(crate) fn is_chunked(&self, doc: DocId, id: &BlobId) -> Result<bool, BlobError> {
        Ok(self
            .docs
            .chunk_read(|r| r.manifest(doc, id).map(|m| m.is_some()))?)
    }

    /// Doc `doc`'s segments' folder in space `no`.
    pub(super) fn segment_dir_of(&self, no: u64, doc: DocId) -> PathBuf {
        segment_dir(&self.dir, no, doc)
    }

    /// Remove segment files no row names (unless a write is waiting on one, when
    /// `honour_waiting`), the segment folders of purged spaces, and empty doc folders. Under
    /// the blob store's lock (the caller's). Returns the files removed.
    pub(super) fn sweep_segments(&self, honour_waiting: bool) -> Result<usize, BlobError> {
        let top = self.dir.join(SEGMENTS);
        let Ok(rd) = fs::read_dir(&top) else {
            return Ok(0);
        };
        let live = self.docs.blob_space_nos()?;
        let ignore = planted(Plant::GcIgnoresWaitingSegment);
        let mut removed = 0;
        for space in rd.flatten() {
            let no = space
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u64>().ok());
            let Some(no) = no.filter(|n| live.contains(n)) else {
                removed += super::count_files(&space.path());
                let _ = fs::remove_dir_all(space.path());
                continue;
            };
            for d in fs::read_dir(space.path()).into_iter().flatten().flatten() {
                let Some(doc) = d.file_name().to_str().and_then(|n| n.parse::<u64>().ok()) else {
                    removed += super::count_files(&d.path());
                    let _ = fs::remove_dir_all(d.path());
                    continue;
                };
                for f in fs::read_dir(d.path()).into_iter().flatten().flatten() {
                    let seg = f
                        .file_name()
                        .to_str()
                        .and_then(|n| n.strip_suffix(".seg"))
                        .and_then(|n| n.parse::<u64>().ok());
                    if let Some(seg) = seg {
                        if honour_waiting && !ignore && self.marks().segment_waiting(no, doc, seg) {
                            continue;
                        }
                        if self.docs.chunk_read(|r| r.has_segment(doc, seg))? {
                            continue;
                        }
                    }
                    if fs::remove_file(f.path()).is_ok() {
                        removed += 1;
                    }
                }
                // an empty doc folder goes (a write makes its folder under this lock)
                let _ = fs::remove_dir(d.path());
            }
        }
        Ok(removed)
    }

    /// After doc `doc` of `space` is deleted: remove its segment files (but one a write still
    /// waits on, which `gc` removes once it has no row) and its folder.
    pub(crate) fn remove_doc_segments(&self, space: &str, doc: DocId) -> Result<(), BlobError> {
        let Some(k) = self.docs.blob_space(space)? else {
            return Ok(());
        };
        let _g = self.guard();
        let dir = self.segment_dir_of(k.no, doc);
        for f in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let seg = f
                .file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".seg"))
                .and_then(|n| n.parse::<u64>().ok());
            if seg.is_some_and(|s| self.marks().segment_waiting(k.no, doc, s)) {
                continue;
            }
            match fs::remove_file(f.path()) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_err("removing a deleted doc's segment", e)),
            }
        }
        // a segment a write still waits on keeps the folder
        let _ = fs::remove_dir(&dir);
        Ok(())
    }

    /// A verifying reader over version `id` of doc `doc` in `space`: its chunks when the doc
    /// holds a manifest for it, else its whole-file blob ([`BlobStore::reader`]).
    pub fn reader_of(
        &self,
        space: &str,
        doc: DocId,
        id: &BlobId,
    ) -> Result<super::BlobReader, BlobError> {
        let k = self.existing_key(space, id)?;
        for attempt in 0..2 {
            let layout = match self.layout(k.no, doc, id) {
                Ok(None) => return self.reader(space, id),
                Ok(Some(l)) => l,
                Err(OpenError::Missing) if attempt == 0 => continue,
                Err(OpenError::Missing) => {
                    return Err(BlobError::Damaged(format!("{id}: a segment is missing")))
                }
                Err(OpenError::Blob(e)) => return Err(e),
            };
            #[cfg(any(test, feature = "differential"))]
            if attempt == 0 {
                crate::differential::chunk_hook(crate::differential::ChunkStep::ReaderRowsRead);
            }
            match self.open_layout(k.no, doc, layout, &k.key, *id) {
                Ok(r) => return Ok(r),
                Err(OpenError::Missing) if attempt == 0 => continue,
                Err(OpenError::Missing) => {
                    return Err(BlobError::Damaged(format!("{id}: a segment is missing")))
                }
                Err(OpenError::Blob(e)) => return Err(e),
            }
        }
        unreachable!("the loop returns on its second pass")
    }

    /// Version `id` of doc `doc` in memory, verified, if it is at most `max` bytes.
    pub fn read_to_vec_of(
        &self,
        space: &str,
        doc: DocId,
        id: &BlobId,
        max: u64,
    ) -> Result<Vec<u8>, BlobError> {
        super::read_all(self.reader_of(space, doc, id)?, max)
    }

    /// Copy version `id` of doc `doc` to `dst`, verified ([`BlobStore::copy_to`]).
    pub fn copy_to_of(
        &self,
        space: &str,
        doc: DocId,
        id: &BlobId,
        dst: &mut dyn Write,
    ) -> Result<u64, BlobError> {
        super::copy_all(self.reader_of(space, doc, id)?, dst)
    }

    /// The chunks of version `id` of doc `doc`, in one read transaction (`None`: the doc holds
    /// no manifest for it).
    fn layout(&self, no: u64, doc: DocId, id: &BlobId) -> Result<Option<Layout>, OpenError> {
        self.docs
            .chunk_read(|r: &ChunkRead| -> Result<Option<Layout>, OpenError> {
                let Some(m) = r.manifest(doc, id)? else {
                    return Ok(None);
                };
                let damaged =
                    |why: &str| OpenError::Blob(BlobError::Damaged(format!("{id}: {why}")));
                let key = r.key(doc)?.ok_or_else(|| damaged("no chunk key"))?;
                let get = |c: &ChunkId| r.chunk(doc, c);
                let entries = match read_entries(&get, &self.dir, no, doc, &key.key, &m.root) {
                    Ok(e) => e,
                    Err(EntriesError::Missing(_)) => return Err(OpenError::Missing),
                    Err(e) => return Err(OpenError::Blob(entries_err(e))),
                };
                let mut chunks = Vec::with_capacity(entries.len());
                let mut total: u64 = 0;
                for (cid, len) in entries {
                    let row = r
                        .chunk(doc, &cid)?
                        .ok_or_else(|| damaged("a chunk has no row"))?;
                    if row.len != len {
                        return Err(damaged("a chunk's length"));
                    }
                    total += u64::from(len);
                    chunks.push(Located {
                        id: cid,
                        segment: row.segment,
                        offset: row.offset,
                        len,
                    });
                }
                if total != m.size {
                    return Err(damaged("its chunks don't add up to its size"));
                }
                Ok(Some(Layout {
                    key: key.key,
                    size: m.size,
                    chunks,
                }))
            })
    }

    /// Open every segment `layout` needs (lazily under the planted fault) and start the
    /// read-ahead.
    fn open_layout(
        &self,
        no: u64,
        doc: DocId,
        layout: Layout,
        space_key: &[u8; 32],
        id: BlobId,
    ) -> Result<super::BlobReader, OpenError> {
        let lazy = planted(Plant::ReaderOpensLazily);
        let skip = planted(Plant::ReaderSkipsChunkCheck);
        let mut files: HashMap<u64, File> = HashMap::new();
        if !lazy {
            for c in &layout.chunks {
                if files.contains_key(&c.segment) {
                    continue;
                }
                match File::open(segment_path(&self.dir, no, doc, c.segment)) {
                    Ok(f) => {
                        files.insert(c.segment, f);
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        return Err(OpenError::Missing)
                    }
                    Err(e) => return Err(OpenError::Blob(io_err("opening a segment", e))),
                }
            }
        }
        let (tx, rx) = sync_channel::<Result<Vec<u8>, String>>(READ_AHEAD);
        let dir = segment_dir(&self.dir, no, doc);
        let key = layout.key;
        let chunks = layout.chunks;
        std::thread::Builder::new()
            .name("blob-read-ahead".into())
            .spawn(move || {
                for c in chunks {
                    if let Entry::Vacant(v) = files.entry(c.segment) {
                        match File::open(dir.join(format!("{}.seg", c.segment))) {
                            Ok(f) => {
                                v.insert(f);
                            }
                            Err(e) => {
                                let _ = tx.send(Err(format!("segment {}: {e}", c.segment)));
                                return;
                            }
                        }
                    }
                    let f = files.get_mut(&c.segment).expect("opened above");
                    let out = match read_at_file(f, c.offset, c.len) {
                        Ok(b) if skip || chunk_id(&key, &b) == c.id => Ok(b),
                        Ok(_) => Err("a chunk doesn't match its id".to_owned()),
                        Err(e) => Err(format!("segment {}: {e}", c.segment)),
                    };
                    let stop = out.is_err();
                    if tx.send(out).is_err() || stop {
                        return;
                    }
                }
            })
            .map_err(|e| OpenError::Blob(io_err("starting a read", e)))?;
        Ok(super::BlobReader::chunked(ChunkedReader {
            rx,
            cur: Vec::new(),
            at: 0,
            mac: Some(HmacSha256::new(space_key)),
            bad: None,
            id,
            left: layout.size,
            size: layout.size,
        }))
    }

    pub(super) fn marks(&self) -> std::sync::MutexGuard<'_, Marks> {
        self.marks.lock().unwrap_or_else(|p| p.into_inner())
    }
}

fn entries_err(e: EntriesError) -> BlobError {
    match e {
        EntriesError::Store(e) => e.into(),
        EntriesError::Missing(p) => {
            BlobError::Damaged(format!("a segment is missing: {}", p.display()))
        }
        EntriesError::Damaged(m) => BlobError::Damaged(m),
        EntriesError::Io(m) => BlobError::Io(m),
    }
}

/// A spooled source, read back from its start.
struct Spool(File);

impl Spool {
    fn seek_start(&mut self) -> Result<(), BlobError> {
        use std::io::Seek;
        self.0
            .seek(io::SeekFrom::Start(0))
            .map(|_| ())
            .map_err(|e| io_err("reading the spool", e))
    }
}

impl Read for Spool {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

/// A chunked version's bytes, verified as they stream: each chunk against its keyed id before
/// it is handed out, the whole file against its id at the end.
pub(super) struct ChunkedReader {
    rx: Receiver<Result<Vec<u8>, String>>,
    cur: Vec<u8>,
    at: usize,
    /// `None` once the end was verified.
    mac: Option<HmacSha256>,
    bad: Option<String>,
    id: BlobId,
    left: u64,
    size: u64,
}

impl ChunkedReader {
    pub(super) fn size(&self) -> u64 {
        self.size
    }

    pub(super) fn id(&self) -> BlobId {
        self.id
    }

    fn damaged(&self, why: &str) -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("blob {} is damaged: {why}", self.id),
        )
    }

    fn fail(&mut self, why: String) -> io::Error {
        let e = self.damaged(&why);
        self.bad = Some(why);
        e
    }
}

impl Read for ChunkedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(why) = &self.bad {
            return Err(self.damaged(why));
        }
        if self.mac.is_none() {
            return Ok(0);
        }
        if self.at == self.cur.len() {
            if self.left == 0 {
                // the end: the file's id matches
                if self.mac.take().expect("checked above").finish() != self.id.0 {
                    return Err(self.fail("bytes don't match the id".into()));
                }
                return Ok(0);
            }
            match self.rx.recv() {
                Ok(Ok(b)) => {
                    if b.is_empty() || b.len() as u64 > self.left {
                        return Err(self.fail("a chunk's length".into()));
                    }
                    self.cur = b;
                    self.at = 0;
                }
                Ok(Err(why)) => return Err(self.fail(why)),
                Err(_) => return Err(self.fail("shorter than its manifest".into())),
            }
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let n = buf.len().min(self.cur.len() - self.at);
        buf[..n].copy_from_slice(&self.cur[self.at..self.at + n]);
        self.at += n;
        self.left -= n as u64;
        self.mac.as_mut().expect("checked above").update(&buf[..n]);
        Ok(n)
    }
}

/// The chunk tables of doc `doc` and its segment files (test builds only).
#[cfg(any(test, feature = "differential"))]
impl BlobStore {
    pub(super) fn report_of(&self, no: u64, doc: DocId) -> Result<super::ChunkReport, BlobError> {
        let rows = self.docs.chunk_rows_for_tests(doc)?;
        let mut files = Vec::new();
        for e in fs::read_dir(self.segment_dir_of(no, doc))
            .into_iter()
            .flatten()
            .flatten()
        {
            let len = e.metadata().map(|m| m.len()).unwrap_or(0);
            files.push((e.file_name().to_string_lossy().into_owned(), len));
        }
        files.sort();
        Ok(super::ChunkReport {
            tables: rows.tables,
            manifests: rows.manifests,
            chunks: rows.chunks,
            segments: rows.segments,
            files,
        })
    }
}
