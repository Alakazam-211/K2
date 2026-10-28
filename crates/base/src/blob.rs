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
//! **Every read is verified.** [`BlobReader`] re-computes the HMAC as it streams and fails
//! the read that reaches the end if it doesn't match (or the length differs). A caller that
//! forwards a stream must treat only a clean end as success: the kit's own copies (write-back,
//! conflict copies) write to a temp file and rename only after a verified end, and
//! [`BlobStore::read_to_vec`] returns nothing unverified.
//!
//! **Streaming.** `put` and every read go through 64 KiB buffers: memory doesn't grow with
//! the blob's size.
//!
//! **GC.** A blob is referenced by blob-doc versions (or [`BlobStore::retain`]). An
//! unreferenced blob is removed by [`BlobStore::gc`] once it has been unreferenced for
//! [`BlobConfig::gc_grace_ms`], so a blob just put and about to be referenced is never
//! collected under its writer.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::hash::{hex, HmacSha256};
use crate::store::{DocStore, SpaceKey, StoreError};

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
}

impl Default for BlobConfig {
    fn default() -> Self {
        Self {
            max_blob_bytes: 4 << 30,
            gc_grace_ms: 60 * 60_000,
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
    /// No randomness source for a space key (the kit reads `/dev/urandom`).
    #[error("no randomness source: {0}")]
    NoRandom(String),
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

/// Returned by [`BlobStore::purge_space_key`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PurgeReport {
    /// Whether the space had a key (now destroyed).
    pub key_destroyed: bool,
    pub blobs_removed: usize,
    pub bytes_removed: u64,
}

/// Returned by [`BlobStore::stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobStats {
    pub blobs: u64,
    pub bytes: u64,
    /// Bytes no version references (collectable once their grace runs out).
    pub unreferenced_bytes: u64,
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
    /// is about to index.
    lock: Mutex<()>,
}

/// 32 random bytes from the OS.
fn random_key() -> Result<[u8; 32], BlobError> {
    crate::hash::random32().map_err(|e| BlobError::NoRandom(e.to_string()))
}

impl BlobStore {
    /// Open the blob folder `dir` (created if missing) over `docs`. Temp files a crash left
    /// are removed. `clock` is the host's (for GC's grace period).
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
        Ok(Self {
            docs,
            dir,
            config,
            clock,
            lock: Mutex::new(()),
        })
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
    /// [`BlobStore::retain`]) within [`BlobConfig::gc_grace_ms`].
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
            File::open(parent)?.sync_all()
        })();
        if let Err(e) = placed {
            let _ = fs::remove_file(&tmp);
            return Err(io_err("placing the blob", e));
        }
        // a crash here leaves a file without a row: gc sweeps it
        let new = self.docs.blob_index(k.no, &id, size, (self.clock)())?;
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
            f,
            mac: Some(HmacSha256::new(&k.key)),
            bad: None,
            id: *id,
            left: row.size,
            size: row.size,
        })
    }

    /// The whole blob in memory, verified, if it is at most `max` bytes
    /// ([`BlobError::TooLarge`] otherwise).
    pub fn read_to_vec(&self, space: &str, id: &BlobId, max: u64) -> Result<Vec<u8>, BlobError> {
        let mut r = self.reader(space, id)?;
        if r.size() > max {
            return Err(BlobError::TooLarge { limit: max });
        }
        let mut out = Vec::with_capacity(r.size() as usize);
        r.read_to_end(&mut out).map_err(|e| r.error(e))?;
        Ok(out)
    }

    /// Copy the blob to `dst`, verified (the copy fails at the end if the bytes are bad; the
    /// caller discards what it wrote). Returns the bytes copied.
    pub fn copy_to(&self, space: &str, id: &BlobId, dst: &mut dyn Write) -> Result<u64, BlobError> {
        let mut r = self.reader(space, id)?;
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

    /// Take a reference on a blob for something other than a blob-doc version (a pack's
    /// embedded image, say), so GC keeps it. One durable transaction.
    pub fn retain(&self, space: &str, id: &BlobId) -> Result<(), BlobError> {
        Ok(self.docs.blob_adjust_ref(space, id, 1, (self.clock)())?)
    }

    /// Drop a reference taken with [`BlobStore::retain`].
    pub fn release(&self, space: &str, id: &BlobId) -> Result<(), BlobError> {
        Ok(self.docs.blob_adjust_ref(space, id, -1, (self.clock)())?)
    }

    /// Remove blobs unreferenced for [`BlobConfig::gc_grace_ms`] (index row first, then the
    /// file), files with no index row, and the folders of purged spaces.
    pub fn gc(&self) -> Result<GcReport, BlobError> {
        let _g = self.guard();
        let now = (self.clock)();
        let due = self.docs.blob_collectable(now, self.config.gc_grace_ms)?;
        let gone = self.docs.blob_remove_unreferenced(&due)?;
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
    /// re-imports them. Audit it as metadata only.
    pub fn purge_space_key(&self, space: &str) -> Result<PurgeReport, BlobError> {
        let _g = self.guard();
        let Some((no, rows)) = self.docs.blob_purge_space(space)? else {
            return Ok(PurgeReport::default());
        };
        let folder = self.dir.join(no.to_string());
        match fs::remove_dir_all(&folder) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err("removing the space's blobs", e)),
        }
        Ok(PurgeReport {
            key_destroyed: true,
            blobs_removed: rows.len(),
            bytes_removed: rows.iter().map(|(_, s)| s).sum(),
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
        let gone = self.docs.blob_remove_unreferenced(rows)?;
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
        })
    }
}

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

/// A blob's bytes, verified as they stream ([`BlobStore::reader`]).
pub struct BlobReader {
    f: File,
    /// `None` once the end was verified.
    mac: Option<HmacSha256>,
    /// Set once the bytes failed verification: every later read fails too.
    bad: Option<&'static str>,
    id: BlobId,
    left: u64,
    size: u64,
}

impl BlobReader {
    /// The blob's indexed size.
    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn id(&self) -> BlobId {
        self.id
    }

    fn damaged(&self, why: &str) -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("blob {} is damaged: {why}", self.id),
        )
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
}
