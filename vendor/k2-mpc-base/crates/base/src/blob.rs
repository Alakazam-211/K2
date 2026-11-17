//! The blob service (tier 0): content-addressed bytes for files the kit can't model as a
//! CRDT (images, PDFs, office files, binaries). Blobs are **never CRDT values**: a blob doc
//! is a pointer to a blob, its versions are the pointer's history, and a lease guards
//! writers (`docs/SESSIONS.md`, "Tier 0").
//!
//! **Where the bytes live.** Files under a folder the host chooses
//! (`<dir>/<space no>/<2 hex>/<64 hex>`), not in the redb store: a 1 GB video would
//! otherwise bloat the store file and every backup and compaction of it. Only the **index**
//! (size, reference count, the per-space key) is in the [`DocStore`], so a version and the
//! reference it takes on a blob commit in one transaction. Put the folder on the same volume
//! as the store, and back them up together.
//!
//! **Addresses are keyed.** A blob's id is `HMAC-SHA256(space key, bytes)`, with a random
//! 32-byte key per space kept in the store. The same file in two spaces gets unrelated ids
//! (no cross-space equality leaks), and a small file can't be brute-forced from its id.
//! [`BlobStore::purge_space_key`] destroys the key and the space's blobs.
//!
//! **Host cache keys.** [`BlobStore::host_cache_key`] derives a key from a space's key and a
//! purpose label, for a host to encrypt its own caches of the space's content (previews, for
//! one) at rest. The kit never stores it; destroying the space key ends it
//! ([`BlobStore::host_cache_key_at`] then refuses, `MPC-E603`).
//!
//! **Every read is verified.** [`BlobReader`] re-computes the HMAC as it streams and fails
//! the read that reaches the end if it doesn't match (or the length differs). A caller that
//! forwards a stream must treat only a clean end as success: the kit's own copies (write-back,
//! conflict copies) write to a temp file and rename only after a verified end, and
//! [`BlobStore::read_to_vec`] returns nothing unverified.
//!
//! **Streaming.** `put` and every read go through 64 KiB buffers: memory doesn't grow with
//! the blob's size.
//!
//! **GC.** A blob is referenced by blob-doc versions, by a doc whose values name it
//! ([`BlobId::parse_ref`], `DocStore::blob_refs`), or by [`BlobStore::retain`]. An
//! unreferenced blob is removed by [`BlobStore::gc`] once it has been unreferenced for
//! [`BlobConfig::gc_grace_ms`], so a blob just put and about to be referenced is never
//! collected under its writer. A removal **at once** (a purge's, a replaced carrier's) never
//! takes a blob a put is waiting to reference either: each put is **waiting** until its
//! writer has taken its reference (the kit's own writers say so) or the grace has run out, and
//! a waiting blob stays (it is reported as shared). So a writer never finds the bytes it just
//! put gone when it references them.

use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::hash::{hex, HmacSha256};
use crate::store::{DocStore, SpaceKey, StoreError};

mod cdc;
mod chunked;
mod compact;
mod host_cache;
mod rechunk;
pub(crate) use chunked::{CheapSet, HintSource, LockWait, VersionPut};
pub use host_cache::HostCacheKey;
pub use rechunk::{RechunkError, RechunkReport};

const BUF: usize = 64 << 10;
const TMP: &str = "tmp";

/// A blob's address within its space: `HMAC-SHA256(space key, bytes)`. Meaningless in
/// another space.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlobId([u8; 32]);

impl BlobId {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn from_slice(b: &[u8]) -> Option<Self> {
        b.try_into().ok().map(Self)
    }

    /// Lowercase hex (64 characters).
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }

    /// Parse [`BlobId::to_hex`]'s form.
    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 64 || !s.is_ascii() {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, o) in out.iter_mut().enumerate() {
            *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(Self(out))
    }
}

/// What starts a blob reference ([`BlobId::parse_ref`]).
const REF_PREFIX: &str = "blob:";

impl BlobId {
    /// This blob as a doc value: `blob:<64 lowercase hex>` ([`BlobId::parse_ref`]).
    pub fn to_ref(&self) -> String {
        format!("{REF_PREFIX}{}", self.to_hex())
    }

    /// The blob a doc value names, if the value is **exactly** a reference: `blob:` and 64
    /// lowercase hex digits, the spelling [`BlobId::to_ref`] makes, and nothing else. A value
    /// that merely contains one, with another case or anything around it, is ordinary text.
    /// One spelling per blob, so every reader and client agrees on what a value names.
    pub fn parse_ref(value: &str) -> Option<Self> {
        let hex = value.strip_prefix(REF_PREFIX)?;
        let digit = |b: &u8| b.is_ascii_digit() || (b'a'..=b'f').contains(b);
        if hex.len() != 64 || !hex.as_bytes().iter().all(digit) {
            return None;
        }
        Self::from_hex(hex)
    }
}

/// Whether the bytes behind a doc's reference are in the blob store now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BlobRefState {
    /// The blob is in the store's index.
    Present,
    /// The doc names it and the store has no such blob: never stored here, collected before
    /// this kit's reference was taken, or its space was purged. The value stays as it is, and a
    /// read of the bytes fails ([`BlobError::NotFound`]).
    Missing,
}

/// One blob a doc names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobRef {
    pub blob: BlobId,
    pub state: BlobRefState,
}

/// The blobs a doc (or one of its versions) names, by id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobRefs {
    pub refs: Vec<BlobRef>,
    /// How many of `refs` are [`BlobRefState::Missing`].
    pub missing: u64,
}

impl FromIterator<BlobRef> for BlobRefs {
    fn from_iter<I: IntoIterator<Item = BlobRef>>(it: I) -> Self {
        let refs: Vec<BlobRef> = it.into_iter().collect();
        let missing = refs
            .iter()
            .filter(|r| r.state == BlobRefState::Missing)
            .count() as u64;
        Self { refs, missing }
    }
}

impl fmt::Debug for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlobId({})", &self.to_hex()[..16])
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Blob store settings.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobConfig {
    /// Largest blob accepted ([`BlobError::TooLarge`] past it). Default 4 GiB.
    pub max_blob_bytes: u64,
    /// An unreferenced blob is collected only after this long (host clock). Default 1 h.
    pub gc_grace_ms: u64,
    /// Store a tier-0 version of [`BlobConfig::chunk_min_bytes`] or more as chunks
    /// (`docs/SESSIONS.md`, "Chunked versions"). Default off. **The first chunked version
    /// stamps the store format 8**, which kits up to v0.18.0 refuse to open: turn it on only
    /// once the release a host would roll back to reads format 8.
    pub chunked_versions: bool,
    /// The smallest version stored chunked when [`BlobConfig::chunked_versions`] is on;
    /// smaller ones stay whole files. Default 1 MiB.
    pub chunk_min_bytes: u64,
}

impl Default for BlobConfig {
    fn default() -> Self {
        Self {
            max_blob_bytes: 4 << 30,
            gc_grace_ms: 60 * 60_000,
            chunked_versions: false,
            chunk_min_bytes: 1 << 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BlobError {
    /// The bytes are over [`BlobConfig::max_blob_bytes`]. Nothing was stored.
    #[error("blob is over the {limit}-byte limit")]
    TooLarge { limit: u64 },
    /// No such blob in this space (never stored, collected, or its space was purged).
    #[error("no blob {0} in this space")]
    NotFound(String),
    /// The blob's bytes don't match its id or size. It is never served.
    #[error("blob {0} is damaged (its bytes don't match its id)")]
    Damaged(String),
    #[error("blob i/o: {0}")]
    Io(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    /// No randomness source for a space key (the OS generator: `/dev/urandom`, or `RtlGenRandom` on Windows).
    #[error("no randomness source: {0}")]
    NoRandom(String),
    /// The space key a host cache key of `generation` was derived from is destroyed
    /// ([`BlobStore::purge_space_key`]): no key for that generation is given again, and the
    /// host drops every cache entry sealed under it ([`BlobStore::host_cache_key_at`]).
    #[error("the space key of cache generation {generation} was destroyed")]
    CacheKeyDestroyed { generation: u64 },
}

fn io_err(what: &str, e: impl fmt::Display) -> BlobError {
    BlobError::Io(format!("{what}: {e}"))
}

/// Returned by [`BlobStore::put`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobInfo {
    pub id: BlobId,
    pub size: u64,
    /// False when the store already had these bytes (in this space).
    pub new: bool,
}

/// Returned by [`BlobStore::gc`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct GcReport {
    pub removed: usize,
    pub bytes: u64,
    /// Files with no index row (a crash between storing and indexing), or of purged spaces.
    pub orphan_files: usize,
}

/// The planted fault [`crate::differential::KeyedMutant::RemovalIgnoresWaitingPuts`].
fn planted_ignores_waiting() -> bool {
    plant_on!(
        crate::differential::keyed_mutant()
            == Some(crate::differential::KeyedMutant::RemovalIgnoresWaitingPuts)
    )
}

/// Returned by [`BlobStore::purge_space_key`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PurgeReport {
    /// Whether the space had a key (now destroyed).
    pub key_destroyed: bool,
    pub blobs_removed: usize,
    pub bytes_removed: u64,
    /// Structure-index rows of the space's blobs removed with them (their content leaves the
    /// file with the rewrite, `store_rewritten`).
    pub index_rows_removed: u64,
    /// Whether the store file was rewritten after destroying the key, so the key's
    /// bytes and any index rows' leave the file. `false` when the space had no key, or when
    /// the rewrite failed (the key and rows are gone from the store, their old bytes stay in
    /// freed pages until a later rewrite).
    pub store_rewritten: bool,
}

/// Returned by [`BlobStore::stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobStats {
    pub blobs: u64,
    pub bytes: u64,
    /// Bytes no version references (collectable once their grace runs out).
    pub unreferenced_bytes: u64,
    /// Versions stored as whole-file blobs, since this blob store was opened, because a chunked
    /// write found its doc's internal lock busy (in memory: 0 after a restart).
    pub chunked_busy_fallbacks: u64,
    /// Chunked versions whose pack's cut hints were ignored, since this blob store was opened,
    /// because one was outside the file or out of order (`BlobPack::chunk_hints`; in memory: 0
    /// after a restart).
    pub chunk_hints_ignored: u64,
}

/// The host's clock (milliseconds), as [`crate::session::Host::now_ms`].
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Content-addressed blobs: files under a host-given folder, indexed in the [`DocStore`].
/// One per store and folder; thread-safe.
pub struct BlobStore {
    docs: Arc<DocStore>,
    dir: PathBuf,
    config: BlobConfig,
    clock: Clock,
    /// Serialises "file in place + index row" against GC, so GC never removes a file a put
    /// is about to index; a chunked write's waiting marks change under it too.
    lock: Arc<Mutex<()>>,
    /// Puts waiting for their reference: `(space no, id)` -> (how many, when the last was
    /// put). Read and written under `lock`. Removal at once leaves these blobs.
    waiting: Mutex<HashMap<(u64, BlobId), (u32, u64)>>,
    /// Chunked writes' waiting marks (segment names and chunk ids), changed under `lock`.
    marks: Arc<Mutex<chunked::Marks>>,
    /// The docs' internal locks, one chunked writer per doc.
    doc_locks: Arc<chunked::DocLocks>,
    /// Versions stored whole because a chunked write found its doc's lock busy
    /// ([`BlobStats::chunked_busy_fallbacks`]).
    busy_fallbacks: AtomicU64,
    /// Chunked versions whose pack's hints were ignored ([`BlobStats::chunk_hints_ignored`]).
    hints_ignored: AtomicU64,
}

/// 32 random bytes from the OS.
fn random_key() -> Result<[u8; 32], BlobError> {
    crate::hash::random32().map_err(|e| BlobError::NoRandom(e.to_string()))
}

impl BlobStore {
    /// Open the blob folder `dir` (created if missing) over `docs`. Temp files a crash left
    /// are removed, and so is every segment file of a chunked version no row names (nothing is
    /// waiting after a restart: a write a stop interrupted before its commit). `clock` is the
    /// host's (for GC's grace period).
    pub fn open(
        docs: Arc<DocStore>,
        dir: impl AsRef<Path>,
        config: BlobConfig,
        clock: Clock,
    ) -> Result<Self, BlobError> {
        let dir = dir.as_ref().to_owned();
        fs::create_dir_all(dir.join(TMP)).map_err(|e| io_err("blob folder", e))?;
        if let Ok(rd) = fs::read_dir(dir.join(TMP)) {
            for e in rd.flatten() {
                let _ = fs::remove_file(e.path());
            }
        }
        let lock = Arc::new(Mutex::new(()));
        let marks: Arc<Mutex<chunked::Marks>> = Arc::default();
        let m = marks.clone();
        docs.link_blob_store(crate::store::BlobLink {
            dir: dir.clone(),
            lock: lock.clone(),
            waiting: Arc::new(move |doc, id| {
                m.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .chunk_waiting(doc, id)
            }),
        });
        let s = Self {
            docs,
            dir,
            config,
            clock,
            lock,
            waiting: Mutex::new(HashMap::new()),
            marks,
            doc_locks: Arc::default(),
            busy_fallbacks: AtomicU64::new(0),
            hints_ignored: AtomicU64::new(0),
        };
        // a removal list a stop after a purge's commit left is finished before anything else
        // reads or writes a chunked doc
        s.finish_removals_at_open()?;
        {
            let _g = s.guard();
            s.sweep_segments(false)?;
        }
        Ok(s)
    }

    pub fn config(&self) -> &BlobConfig {
        &self.config
    }

    /// The store this index lives in.
    pub fn doc_store(&self) -> &Arc<DocStore> {
        &self.docs
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn key(&self, space: &str) -> Result<SpaceKey, BlobError> {
        if let Some(k) = self.docs.blob_space(space)? {
            return Ok(k);
        }
        Ok(self.docs.blob_space_or_create(space, random_key()?)?)
    }

    fn existing_key(&self, space: &str, id: &BlobId) -> Result<SpaceKey, BlobError> {
        self.docs
            .blob_space(space)?
            .ok_or_else(|| BlobError::NotFound(id.to_hex()))
    }

    fn path_of(&self, no: u64, id: &BlobId) -> PathBuf {
        let h = id.to_hex();
        self.dir.join(no.to_string()).join(&h[..2]).join(h)
    }

    /// Store the bytes `src` yields in `space`, streaming (constant memory). Returns their
    /// id; storing the same bytes again in the same space returns the same id (`new: false`).
    /// A new blob has no references: reference it (a blob-doc version, or
    /// [`BlobStore::retain`]) within [`BlobConfig::gc_grace_ms`]. Until then the put is
    /// **waiting**: neither GC nor a removal at once (a purge's) takes the blob.
    pub fn put(&self, space: &str, src: &mut dyn Read) -> Result<BlobInfo, BlobError> {
        static N: AtomicU64 = AtomicU64::new(0);
        let k = self.key(space)?;
        let tmp = self.dir.join(TMP).join(format!(
            "{}-{}.part",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let written = (|| {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .map_err(|e| io_err("blob temp file", e))?;
            let mut mac = HmacSha256::new(&k.key);
            let mut buf = vec![0u8; BUF];
            let mut size: u64 = 0;
            loop {
                let n = match src.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(io_err("reading the blob's source", e)),
                };
                size += n as u64;
                if size > self.config.max_blob_bytes {
                    return Err(BlobError::TooLarge {
                        limit: self.config.max_blob_bytes,
                    });
                }
                mac.update(&buf[..n]);
                f.write_all(&buf[..n])
                    .map_err(|e| io_err("writing the blob", e))?;
            }
            f.sync_all().map_err(|e| io_err("syncing the blob", e))?;
            Ok((BlobId(mac.finish()), size))
        })();
        let (id, size) = match written {
            Ok(v) => v,
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                return Err(e);
            }
        };
        let _g = self.guard();
        let path = self.path_of(k.no, &id);
        let placed = (|| -> io::Result<()> {
            if path.is_file() {
                fs::remove_file(&tmp)?;
                return Ok(());
            }
            let parent = path.parent().expect("blob path has a folder");
            fs::create_dir_all(parent)?;
            fs::rename(&tmp, &path)?;
            crate::hash::sync_dir(parent)
        })();
        if let Err(e) = placed {
            let _ = fs::remove_file(&tmp);
            return Err(io_err("placing the blob", e));
        }
        // a crash here leaves a file without a row: gc sweeps it
        let now = (self.clock)();
        let new = self.docs.blob_index(k.no, &id, size, now)?;
        // waiting for its reference: a removal at once leaves it (under the lock it takes)
        let mut waiting = self.waiting();
        let e = waiting.entry((k.no, id)).or_insert((0, now));
        e.0 = e.0.saturating_add(1);
        e.1 = now;
        Ok(BlobInfo { id, size, new })
    }

    /// [`BlobStore::put`] from memory.
    pub fn put_bytes(&self, space: &str, bytes: &[u8]) -> Result<BlobInfo, BlobError> {
        self.put(space, &mut &bytes[..])
    }

    /// The id `src`'s bytes would have in `space`, without storing them (streaming).
    /// Returns `(id, size)`.
    pub fn id_of(&self, space: &str, src: &mut dyn Read) -> Result<(BlobId, u64), BlobError> {
        let k = self.key(space)?;
        let mut mac = HmacSha256::new(&k.key);
        let mut buf = vec![0u8; BUF];
        let mut size: u64 = 0;
        loop {
            let n = match src.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(io_err("reading", e)),
            };
            size += n as u64;
            mac.update(&buf[..n]);
        }
        Ok((BlobId(mac.finish()), size))
    }

    /// The blob's size and reference count, if it is in `space`.
    pub fn info(&self, space: &str, id: &BlobId) -> Result<Option<(u64, u64)>, BlobError> {
        let Some(k) = self.docs.blob_space(space)? else {
            return Ok(None);
        };
        Ok(self.docs.blob_row(k.no, id)?.map(|r| (r.size, r.refs)))
    }

    /// A verifying reader over the blob's bytes. The read that reaches the end fails with
    /// [`io::ErrorKind::InvalidData`] if the bytes don't match the id or the indexed size;
    /// treat a stream as good only after a clean end.
    /// A whole-file blob's reader, for a host's named blobs and [`BlobStore::retain`]ed ones.
    /// A tier-0 version is read with [`BlobStore::reader_of`], which also reads a chunked one.
    pub fn reader(&self, space: &str, id: &BlobId) -> Result<BlobReader, BlobError> {
        let k = self.existing_key(space, id)?;
        let row = self
            .docs
            .blob_row(k.no, id)?
            .ok_or_else(|| BlobError::NotFound(id.to_hex()))?;
        let f = match File::open(self.path_of(k.no, id)) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(BlobError::Damaged(format!("{id}: file missing")))
            }
            Err(e) => return Err(io_err("opening the blob", e)),
        };
        Ok(BlobReader {
            inner: Inner::Whole(WholeReader {
                f,
                mac: Some(HmacSha256::new(&k.key)),
                bad: None,
                id: *id,
                left: row.size,
                size: row.size,
            }),
        })
    }

    /// The whole blob in memory, verified, if it is at most `max` bytes
    /// ([`BlobError::TooLarge`] otherwise).
    pub fn read_to_vec(&self, space: &str, id: &BlobId, max: u64) -> Result<Vec<u8>, BlobError> {
        read_all(self.reader(space, id)?, max)
    }

    /// Copy the blob to `dst`, verified (the copy fails at the end if the bytes are bad; the
    /// caller discards what it wrote). Returns the bytes copied.
    pub fn copy_to(&self, space: &str, id: &BlobId, dst: &mut dyn Write) -> Result<u64, BlobError> {
        copy_all(self.reader(space, id)?, dst)
    }

    fn waiting(&self) -> MutexGuard<'_, HashMap<(u64, BlobId), (u32, u64)>> {
        self.waiting.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The writer of a put of `id` in `space` has taken its reference (or given up on it): the
    /// put no longer waits. The kit's own writers call it after the transaction that references
    /// what they put; a put nobody settles stops waiting after [`BlobConfig::gc_grace_ms`].
    /// How many puts wait for their reference now (test-only: a put a caller never finished
    /// stays counted).
    #[cfg(any(test, feature = "differential"))]
    pub fn waiting_puts(&self) -> u64 {
        self.waiting().values().map(|(n, _)| u64::from(*n)).sum()
    }

    pub(crate) fn settle(&self, space: &str, id: &BlobId) {
        let Ok(Some(k)) = self.docs.blob_space(space) else {
            return;
        };
        let _g = self.guard();
        let mut waiting = self.waiting();
        if let Some(e) = waiting.get_mut(&(k.no, *id)) {
            e.0 = e.0.saturating_sub(1);
            if e.0 == 0 {
                waiting.remove(&(k.no, *id));
            }
        }
    }

    /// Take a reference on a blob for something other than a blob-doc version or a doc value
    /// a pack declares ([`crate::session::Pack::blob_ref_roots`]), so GC keeps it. One durable
    /// transaction.
    pub fn retain(&self, space: &str, id: &BlobId) -> Result<(), BlobError> {
        Ok(self.docs.blob_adjust_ref(space, id, 1, (self.clock)())?)
    }

    /// Drop a reference taken with [`BlobStore::retain`].
    pub fn release(&self, space: &str, id: &BlobId) -> Result<(), BlobError> {
        Ok(self.docs.blob_adjust_ref(space, id, -1, (self.clock)())?)
    }

    /// Remove blobs unreferenced for [`BlobConfig::gc_grace_ms`] (index row first, then the
    /// file), files with no index row, the folders of purged spaces, and structure-index rows
    /// whose blob is gone (left by a kit that didn't know the table). For chunked versions:
    /// chunk rows at no references nothing waits on, segment files no row names (never one a
    /// write or a compaction waits on), the compaction retention deferred (a segment less than
    /// half live, of a doc whose internal lock is free), and the removal list's entries an
    /// earlier try couldn't delete.
    pub fn gc(&self) -> Result<GcReport, BlobError> {
        let rep = self.gc_locked()?;
        self.compact_sparse()?;
        self.finish_removals(None)?;
        Ok(rep)
    }

    /// [`BlobStore::gc`]'s part under the blob store's lock.
    fn gc_locked(&self) -> Result<GcReport, BlobError> {
        let _g = self.guard();
        let now = (self.clock)();
        let due = self.docs.blob_collectable(now, self.config.gc_grace_ms)?;
        let gone = self.docs.blob_remove_unreferenced(&due)?;
        self.docs.blob_struct_sweep_orphans()?;
        let mut rep = GcReport::default();
        for (no, id, size) in &gone {
            match fs::remove_file(self.path_of(*no, id)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_err("removing a blob", e)),
            }
            rep.removed += 1;
            rep.bytes += size;
        }
        // chunk rows retention kept at no references for a write that waited on them, once
        // nothing waits; then chunked versions' segments: files without rows (unless a write
        // waits on one), and purged spaces' folders
        {
            let marks = self.marks();
            self.docs
                .chunk_remove_unreferenced(&|doc, id| marks.chunk_waiting(doc, id))?;
        }
        rep.orphan_files += self.sweep_segments(true)?;
        // the sweep: files without rows, and purged spaces' folders
        let live = self.docs.blob_space_nos()?;
        let rd = fs::read_dir(&self.dir).map_err(|e| io_err("blob folder", e))?;
        for e in rd.flatten() {
            let Some(no) = e.file_name().to_str().and_then(|n| n.parse::<u64>().ok()) else {
                continue; // tmp, or not ours
            };
            if !live.contains(&no) {
                rep.orphan_files += count_files(&e.path());
                let _ = fs::remove_dir_all(e.path());
                continue;
            }
            for sub in fs::read_dir(e.path()).into_iter().flatten().flatten() {
                for f in fs::read_dir(sub.path()).into_iter().flatten().flatten() {
                    let indexed = f
                        .file_name()
                        .to_str()
                        .and_then(BlobId::from_hex)
                        .map(|id| self.docs.blob_indexed(no, &id))
                        .transpose()?;
                    if indexed != Some(true) {
                        let _ = fs::remove_file(f.path());
                        rep.orphan_files += 1;
                    }
                }
            }
        }
        Ok(rep)
    }

    /// Erase a space's blobs: destroy its key (so ids made under it can never be
    /// recomputed, and the same bytes stored again get an unrelated id) and remove every
    /// blob stored under it, index and files. Blob docs in the space keep their history
    /// records, but their content is gone ([`BlobError::NotFound`]); the host deletes or
    /// re-imports them. Every host cache key derived from the key goes with it
    /// ([`BlobStore::host_cache_key_at`] refuses its generation from now on). The store file
    /// is rewritten, so the key's bytes leave it. Audit it as metadata only.
    pub fn purge_space_key(&self, space: &str) -> Result<PurgeReport, BlobError> {
        let _g = self.guard();
        let Some((no, rows, index_rows_removed)) = self.docs.blob_purge_space(space)? else {
            return Ok(PurgeReport::default());
        };
        // its whole-file blobs, and its chunked versions' segments
        for folder in [
            self.dir.join(no.to_string()),
            self.dir.join(crate::store::SEGMENTS).join(no.to_string()),
        ] {
            match fs::remove_dir_all(&folder) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_err("removing the space's blobs", e)),
            }
        }
        // the key's own bytes must leave the file (a key read back from a freed page would
        // re-derive every host cache key of the space), and so must index rows' names and text
        let store_rewritten = self.docs.rewrite_file().is_ok();
        Ok(PurgeReport {
            key_destroyed: true,
            blobs_removed: rows.len(),
            bytes_removed: rows.iter().map(|(_, s)| s).sum(),
            index_rows_removed,
            store_rewritten,
        })
    }

    /// Remove now, without the GC grace, the blobs in `rows` (`(space no, id, size)`) that
    /// nothing references: a purge's blobs. Index row first, then the file. Returns the
    /// blobs and bytes removed.
    pub(crate) fn remove_unreferenced_now(
        &self,
        rows: &[(u64, BlobId, u64)],
    ) -> Result<(u64, u64), BlobError> {
        let _g = self.guard();
        // a put waiting for its reference keeps its blob (it lapses after the grace)
        let waiting = {
            let now = (self.clock)();
            let grace = self.config.gc_grace_ms;
            let mut waiting = self.waiting();
            waiting.retain(|_, (_, at)| now.saturating_sub(*at) < grace);
            waiting.clone()
        };
        let keep =
            |no: u64, id: &BlobId| !planted_ignores_waiting() && waiting.contains_key(&(no, *id));
        let gone = self.docs.blob_remove_unreferenced_but(rows, &keep)?;
        let (mut n, mut bytes) = (0, 0);
        for (no, id, size) in &gone {
            match fs::remove_file(self.path_of(*no, id)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_err("removing a purged blob", e)),
            }
            n += 1;
            bytes += size;
        }
        Ok((n, bytes))
    }

    /// Totals over the index.
    pub fn stats(&self) -> Result<BlobStats, BlobError> {
        let (blobs, bytes, unreferenced_bytes) = self.docs.blob_totals()?;
        Ok(BlobStats {
            blobs,
            bytes,
            unreferenced_bytes,
            chunked_busy_fallbacks: self.busy_fallbacks.load(Ordering::Relaxed),
            chunk_hints_ignored: self.hints_ignored.load(Ordering::Relaxed),
        })
    }

    /// Test-only: how many entries the removal list holds (segment files a purge's commit named
    /// for deletion and the kit hasn't deleted yet), every doc's.
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn pending_removals_for_tests(&self) -> Result<usize, BlobError> {
        Ok(self.docs.chunk_removals(None)?.len())
    }

    /// Test-only: purge versions `vids` of doc `doc` in `space` with no session, as a session's
    /// purge does it in the blob store (under the doc's internal lock): their rows, their chunks'
    /// bytes and the removal list.
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn purge_versions_for_tests(
        &self,
        space: &str,
        doc: crate::store::DocId,
        vids: &[crate::store::VersionId],
    ) -> Result<(), BlobError> {
        self.purge_for_tests(space, doc, vids)
    }

    /// Test-only: hold doc `doc`'s internal lock (the one a chunked write takes) until the
    /// value is dropped, as another chunked writer of the doc would.
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn hold_doc_lock_for_tests(&self, doc: crate::store::DocId) -> DocLockHeld {
        DocLockHeld(self.doc_locks.take_waiting(doc))
    }

    /// Test-only: the folder of doc `doc`'s segment files in `space`.
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn segment_dir(&self, space: &str, doc: crate::store::DocId) -> Result<PathBuf, BlobError> {
        let k = self
            .docs
            .blob_space(space)?
            .ok_or_else(|| BlobError::NotFound(space.to_owned()))?;
        Ok(self.segment_dir_of(k.no, doc))
    }

    /// Test-only: what the store holds of doc `doc`'s chunked versions.
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn chunk_report(
        &self,
        space: &str,
        doc: crate::store::DocId,
    ) -> Result<ChunkReport, BlobError> {
        let k = self
            .docs
            .blob_space(space)?
            .ok_or_else(|| BlobError::NotFound(space.to_owned()))?;
        self.report_of(k.no, doc)
    }
}

/// Test-only: what the store holds of one doc's chunked versions
/// ([`BlobStore::chunk_report`]).
#[cfg(any(test, feature = "differential"))]
#[doc(hidden)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(clippy::type_complexity)]
pub struct ChunkReport {
    /// The chunk tables the store holds (none until a store's first chunked version).
    pub tables: Vec<String>,
    /// The doc's manifests: (file id, versions naming it).
    pub manifests: Vec<(BlobId, u64)>,
    /// The doc's chunk rows: (chunk id, segment, offset, length, references).
    pub chunks: Vec<([u8; 32], u64, u64, u32, u64)>,
    /// The doc's segment rows: (segment, what the file holds as (chunk id, offset, length)).
    pub segments: Vec<(u64, Vec<([u8; 32], u64, u32)>)>,
    /// The doc's segment files: (file name, bytes).
    pub files: Vec<(String, u64)>,
}

/// Test-only: a doc's internal lock, held until dropped ([`BlobStore::hold_doc_lock_for_tests`]).
#[cfg(any(test, feature = "differential"))]
#[doc(hidden)]
pub struct DocLockHeld(#[allow(dead_code)] chunked::DocGuard);

fn count_files(dir: &Path) -> usize {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            if e.path().is_dir() {
                count_files(&e.path())
            } else {
                1
            }
        })
        .sum()
}

/// `r`'s bytes in memory, verified, if they are at most `max` ([`BlobError::TooLarge`]
/// otherwise).
fn read_all(mut r: BlobReader, max: u64) -> Result<Vec<u8>, BlobError> {
    if r.size() > max {
        return Err(BlobError::TooLarge { limit: max });
    }
    let mut out = Vec::with_capacity(r.size() as usize);
    r.read_to_end(&mut out).map_err(|e| r.error(e))?;
    Ok(out)
}

/// `r`'s bytes copied to `dst`, verified (the copy fails at the end if they are bad).
fn copy_all(mut r: BlobReader, dst: &mut dyn Write) -> Result<u64, BlobError> {
    let mut buf = vec![0u8; BUF];
    let mut n_all = 0;
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => return Ok(n_all),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(r.error(e)),
        };
        dst.write_all(&buf[..n]).map_err(|e| io_err("copying", e))?;
        n_all += n as u64;
    }
}

/// A blob's bytes, verified as they stream ([`BlobStore::reader`], [`BlobStore::reader_of`]):
/// a whole-file blob, or a chunked version's chunks.
pub struct BlobReader {
    inner: Inner,
}

enum Inner {
    Whole(WholeReader),
    Chunked(chunked::ChunkedReader),
}

impl BlobReader {
    fn chunked(r: chunked::ChunkedReader) -> Self {
        Self {
            inner: Inner::Chunked(r),
        }
    }

    /// The blob's indexed size.
    pub fn size(&self) -> u64 {
        match &self.inner {
            Inner::Whole(r) => r.size,
            Inner::Chunked(r) => r.size(),
        }
    }

    pub fn id(&self) -> BlobId {
        match &self.inner {
            Inner::Whole(r) => r.id,
            Inner::Chunked(r) => r.id(),
        }
    }

    /// Map a read error from this reader to a [`BlobError`].
    pub(crate) fn error(&self, e: io::Error) -> BlobError {
        if e.kind() == io::ErrorKind::InvalidData {
            BlobError::Damaged(e.to_string())
        } else {
            io_err("reading the blob", e)
        }
    }
}

impl Read for BlobReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.inner {
            Inner::Whole(r) => r.read(buf),
            Inner::Chunked(r) => r.read(buf),
        }
    }
}

/// A whole-file blob's bytes, verified as they stream.
struct WholeReader {
    f: File,
    /// `None` once the end was verified.
    mac: Option<HmacSha256>,
    /// Set once the bytes failed verification: every later read fails too.
    bad: Option<&'static str>,
    id: BlobId,
    left: u64,
    size: u64,
}

impl WholeReader {
    fn damaged(&self, why: &str) -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("blob {} is damaged: {why}", self.id),
        )
    }
}

impl Read for WholeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(why) = self.bad {
            return Err(self.damaged(why));
        }
        if self.mac.is_none() {
            return Ok(0);
        }
        if self.left == 0 {
            // the end: no extra bytes, and the HMAC matches
            let mut probe = [0u8; 1];
            if self.f.read(&mut probe)? != 0 {
                self.bad = Some("longer than indexed");
            } else if self.mac.take().expect("checked above").finish() != self.id.0 {
                self.bad = Some("bytes don't match the id");
            }
            return match self.bad {
                Some(why) => Err(self.damaged(why)),
                None => Ok(0),
            };
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min(self.left.min(usize::MAX as u64) as usize);
        let n = self.f.read(&mut buf[..want])?;
        if n == 0 {
            self.bad = Some("shorter than indexed");
            return Err(self.damaged("shorter than indexed"));
        }
        self.mac.as_mut().expect("checked above").update(&buf[..n]);
        self.left -= n as u64;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let id = BlobId([0xab; 32]);
        assert_eq!(BlobId::from_hex(&id.to_hex()), Some(id));
        assert_eq!(BlobId::from_hex("zz"), None);
        assert_eq!(BlobId::from_hex(&"g".repeat(64)), None);
    }

    /// A reference is exactly `blob:` and 64 lowercase hex digits: text around it, another
    /// case, a sign the hex parser would take, or a wrong length is ordinary text.
    #[test]
    fn only_the_exact_spelling_is_a_reference() {
        let id = BlobId([0xab; 32]);
        let good = id.to_ref();
        assert_eq!(good, format!("blob:{}", "ab".repeat(32)));
        assert_eq!(BlobId::parse_ref(&good), Some(id));
        let not_refs = [
            format!("see {good}"),
            format!("{good} "),
            format!(" {good}"),
            format!("{good}\n"),
            format!("{good}{good}"),
            good.to_uppercase(),
            format!("blob:{}", "AB".repeat(32)),
            format!("BLOB:{}", "ab".repeat(32)),
            format!("blob:{}", "ab".repeat(31)),
            format!("blob:{}a", "ab".repeat(32)),
            format!("blob:+a{}", "ab".repeat(31)),
            format!("blob:{}gg", "ab".repeat(31)),
            "ab".repeat(32),
            "blob:".to_owned(),
            String::new(),
        ];
        for v in &not_refs {
            assert_eq!(BlobId::parse_ref(v), None, "{v:?} is not a reference");
        }
        // what the from_hex underneath would take on its own
        assert!(BlobId::from_hex(&format!("+a{}", "ab".repeat(31))).is_some());
    }
}
