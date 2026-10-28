//! Tier 0 in the store: the blob index (per-space keys, one row per blob with its size and
//! reference count) and blob docs (the pointer history and the disk-sync state).
//!
//! The bytes themselves are files under a folder the host chooses ([`crate::blob`]); only the
//! index is here, so a version and the reference it takes on a blob commit in **one**
//! transaction.

use redb::{ReadableTable, TableDefinition, WriteTransaction};

use super::enc::{R, W};
use super::{
    dec_approval, enc_approval, Approval, DocId, DocMeta, DocStore, StoreError, VersionId, DOCS,
    META, PATHS,
};
use crate::blob::BlobId;

/// Space name → its key record.
pub(super) const BLOB_SPACES: TableDefinition<&str, &[u8]> = TableDefinition::new("blob_spaces");
/// (space number, blob id) → size, references, since.
pub(super) const BLOBS: TableDefinition<(u64, &[u8]), &[u8]> = TableDefinition::new("blobs");
/// (doc, version) → a blob doc's pointer history.
pub(super) const BLOB_VERSIONS: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("blob_versions");
/// (doc, slot) → disk sync of a blob doc: slot 0 synced, 1 a write-back in flight.
pub(super) const BLOB_SYNC: TableDefinition<(u64, u8), &[u8]> = TableDefinition::new("blob_sync");
/// (doc, version) → the approval record of an on-behalf blob version (format 4). The same
/// record as a text change's (`approvals`), in its own table: a blob version number and a
/// text change's log position never share a key space, so no reader of `approvals` (text
/// purge, the revert window, the pending cap) can take one for the other.
pub(super) const BLOB_APPROVALS: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("blob_approvals");

/// A purged space: its key's number and the blobs (id, size) whose rows were removed.
pub(crate) type PurgedSpace = (u64, Vec<(BlobId, u64)>);

/// A blob a purge released: the space's key number, its id and size, and the references
/// it still has (0: nothing else uses it, so its bytes can go now).
pub(crate) type PurgedBlob = (u64, BlobId, u64, u64);

const SYNCED: u8 = 0;
const PENDING: u8 = 1;

/// The pack name stored for a blob doc. A text session opening one gets `PackMismatch`.
pub const BLOB_DOC_PACK: &str = "kit:blob";

pub(super) fn create_tables(w: &WriteTransaction) -> Result<(), StoreError> {
    w.open_table(BLOB_SPACES)?;
    w.open_table(BLOBS)?;
    w.open_table(BLOB_VERSIONS)?;
    w.open_table(BLOB_SYNC)?;
    w.open_table(BLOB_APPROVALS)?;
    Ok(())
}

/// Record a version and, when an agent wrote it on someone's behalf, its `Pending` approval,
/// in `w` (the caller's transaction: both or neither).
fn put_version(
    w: &WriteTransaction,
    id: DocId,
    vid: VersionId,
    v: &BlobVersion,
) -> Result<(), StoreError> {
    w.open_table(BLOB_VERSIONS)?
        .insert((id, vid), enc_version(v).as_slice())?;
    if let Some(p) = &v.on_behalf_of {
        w.open_table(BLOB_APPROVALS)?.insert(
            (id, vid),
            enc_approval(&Approval::pending(v.author.clone(), p.clone())).as_slice(),
        )?;
    }
    Ok(())
}

/// Format 3 → 4: every on-behalf blob version a format-3 store holds gets the `Pending`
/// approval it would have had (nobody could settle it before), in `w`, the open's own
/// transaction. Versions that already have one are left alone. Returns how many it added.
pub(super) fn backfill_approvals(w: &WriteTransaction) -> Result<u64, StoreError> {
    let mut missing = Vec::new();
    {
        let versions = w.open_table(BLOB_VERSIONS)?;
        let approvals = w.open_table(BLOB_APPROVALS)?;
        for kv in versions.iter()? {
            let (k, v) = kv?;
            let v = dec_version(v.value())?;
            if let Some(p) = v.on_behalf_of {
                if approvals.get(k.value())?.is_none() {
                    missing.push((k.value(), Approval::pending(v.author, p)));
                }
            }
        }
    }
    let mut t = w.open_table(BLOB_APPROVALS)?;
    for (k, a) in &missing {
        t.insert(*k, enc_approval(a).as_slice())?;
    }
    Ok(missing.len() as u64)
}

/// Algorithm of a space key: 1 = HMAC-SHA256.
const ALG_HMAC_SHA256: u8 = 1;

/// A space's blob key. The key is secret: blob ids are `HMAC(key, bytes)`, so equal files in
/// two spaces get unrelated ids, and a small file can't be guessed from its id.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SpaceKey {
    /// Number of this key generation (names the space's folder; never reused).
    pub no: u64,
    pub key: [u8; 32],
}

impl std::fmt::Debug for SpaceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SpaceKey {{ no: {}, key: <secret> }}", self.no)
    }
}

fn enc_space(k: &SpaceKey) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1).u8(ALG_HMAC_SHA256).u64(k.no).bytes(&k.key);
    w.0
}

fn dec_space(b: &[u8]) -> Result<SpaceKey, StoreError> {
    let mut r = R::new(b, "blob space");
    if r.u8()? != 1 || r.u8()? != ALG_HMAC_SHA256 {
        return Err(StoreError::Corrupt("blob space version"));
    }
    let no = r.u64()?;
    let key: [u8; 32] = r
        .bytes()?
        .try_into()
        .map_err(|_| StoreError::Corrupt("blob space key"))?;
    r.end()?;
    Ok(SpaceKey { no, key })
}

/// One blob's index row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlobRow {
    pub size: u64,
    /// References held by blob-doc versions (and [`crate::blob::BlobStore::retain`]).
    pub refs: u64,
    /// When it was stored, re-stored, or last lost its last reference (host clock). GC
    /// removes an unreferenced blob only once this is older than the grace period.
    pub since_ms: u64,
}

fn enc_row(r: &BlobRow) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1).u64(r.size).u64(r.refs).u64(r.since_ms);
    w.0
}

fn dec_row(b: &[u8]) -> Result<BlobRow, StoreError> {
    let mut r = R::new(b, "blob row");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("blob row version"));
    }
    let row = BlobRow {
        size: r.u64()?,
        refs: r.u64()?,
        since_ms: r.u64()?,
    };
    r.end()?;
    Ok(row)
}

/// Why a blob doc's pointer moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BlobReason {
    /// The doc's first content (from the file, or the bytes it was opened with).
    Import,
    /// A participant's `write --base`.
    Write,
    /// Another program saved the file.
    External,
    /// A participant restored an older version (a forward change).
    Restore,
    /// A pack merged a conflicting save (tier 0.5).
    Merged,
}

impl BlobReason {
    fn to_u8(self) -> u8 {
        match self {
            Self::Import => 1,
            Self::Write => 2,
            Self::External => 3,
            Self::Restore => 4,
            Self::Merged => 5,
        }
    }
    fn from_u8(v: u8) -> Result<Self, StoreError> {
        Ok(match v {
            1 => Self::Import,
            2 => Self::Write,
            3 => Self::External,
            4 => Self::Restore,
            5 => Self::Merged,
            _ => return Err(StoreError::Corrupt("blob version reason")),
        })
    }
}

/// One version of a blob doc: where the pointer pointed, who set it and why. The doc's
/// current content is its latest version; versions **are** the pointer's history.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobVersion {
    /// The content (keyed by the doc's space).
    pub blob: BlobId,
    pub size: u64,
    /// The file's modification time (ms since the Unix epoch) when the content came from
    /// disk; `None` otherwise.
    pub mtime_ms: Option<u64>,
    /// When the pointer moved (host clock).
    pub at_ms: u64,
    /// Who moved it: a pid, or the kit's own (`kit:import`, `kit:external`).
    pub author: String,
    /// Their display name then (for conflict copies and history).
    pub author_name: String,
    /// Surface: `"cli"`, `"tab"`, `"disk"`...
    pub via: String,
    pub on_behalf_of: Option<String>,
    pub reason: BlobReason,
    /// A named version is never removed by retention.
    pub name: Option<String>,
    /// For [`BlobReason::Restore`]: the version restored.
    pub restored_from: Option<VersionId>,
    /// The conflict copy the kit wrote when this save conflicted, holding the content that
    /// was current before it (the previous version): its path relative to the file's folder,
    /// a plain file name when beside it (as every copy before v0.4.0 is), or a path into the
    /// host's conflict folder (`Host::conflict_dir`).
    pub conflict_copy: Option<String>,
    /// A purge removed this version's content (format 3): the entry stays (who, when, why,
    /// `size`), `blob` is the zero id, and reading or restoring it fails.
    pub redacted: bool,
}

impl BlobVersion {
    /// A version for `blob` (`size` bytes) set by `author` at `at_ms`.
    pub fn new(
        blob: BlobId,
        size: u64,
        at_ms: u64,
        author: impl Into<String>,
        author_name: impl Into<String>,
        via: impl Into<String>,
        reason: BlobReason,
    ) -> Self {
        Self {
            blob,
            size,
            mtime_ms: None,
            at_ms,
            author: author.into(),
            author_name: author_name.into(),
            via: via.into(),
            on_behalf_of: None,
            reason,
            name: None,
            restored_from: None,
            conflict_copy: None,
            redacted: false,
        }
    }
}

fn opt_u64(w: &mut W, v: Option<u64>) {
    match v {
        None => w.u8(0),
        Some(x) => w.u8(1).u64(x),
    };
}

fn r_opt_u64(r: &mut R<'_>) -> Result<Option<u64>, StoreError> {
    match r.u8()? {
        0 => Ok(None),
        1 => Ok(Some(r.u64()?)),
        _ => Err(StoreError::Corrupt("blob version")),
    }
}

/// Blob version record 2: record 1 plus the `redacted` flag at the end.
fn enc_version(v: &BlobVersion) -> Vec<u8> {
    let mut w = W::default();
    w.u8(2).bytes(v.blob.as_bytes()).u64(v.size);
    opt_u64(&mut w, v.mtime_ms);
    w.u64(v.at_ms)
        .str(&v.author)
        .str(&v.author_name)
        .str(&v.via)
        .opt_str(v.on_behalf_of.as_deref())
        .u8(v.reason.to_u8())
        .opt_str(v.name.as_deref());
    opt_u64(&mut w, v.restored_from);
    w.opt_str(v.conflict_copy.as_deref());
    w.u8(u8::from(v.redacted));
    w.0
}

fn dec_version(b: &[u8]) -> Result<BlobVersion, StoreError> {
    let mut r = R::new(b, "blob version");
    let ver = r.u8()?;
    if ver != 1 && ver != 2 {
        return Err(StoreError::Corrupt("blob version record version"));
    }
    let blob = BlobId::from_slice(&r.bytes()?).ok_or(StoreError::Corrupt("blob id"))?;
    let v = BlobVersion {
        blob,
        size: r.u64()?,
        mtime_ms: r_opt_u64(&mut r)?,
        at_ms: r.u64()?,
        author: r.str()?,
        author_name: r.str()?,
        via: r.str()?,
        on_behalf_of: r.opt_str()?,
        reason: BlobReason::from_u8(r.u8()?)?,
        name: r.opt_str()?,
        restored_from: r_opt_u64(&mut r)?,
        conflict_copy: r.opt_str()?,
        redacted: ver >= 2 && r.u8()? != 0,
    };
    r.end()?;
    Ok(v)
}

/// What a blob doc's file holds (or a write-back in flight will make it hold).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlobSync {
    pub blob: BlobId,
    /// The file's stat when it was synced (opaque, `disk::FileStat`), to skip re-hashing an
    /// unchanged file on open. `None` for a write-back in flight.
    pub stat: Option<Vec<u8>>,
}

fn enc_sync(s: &BlobSync) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1)
        .bytes(s.blob.as_bytes())
        .opt_bytes(s.stat.as_deref());
    w.0
}

fn dec_sync(b: &[u8]) -> Result<BlobSync, StoreError> {
    let mut r = R::new(b, "blob sync");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("blob sync version"));
    }
    let blob = BlobId::from_slice(&r.bytes()?).ok_or(StoreError::Corrupt("blob id"))?;
    let s = BlobSync {
        blob,
        stat: r.opt_bytes()?,
    };
    r.end()?;
    Ok(s)
}

fn space_in(w: &WriteTransaction, space: &str) -> Result<SpaceKey, StoreError> {
    let t = w.open_table(BLOB_SPACES)?;
    let v = t.get(space)?.ok_or(StoreError::NoBlobSpace)?;
    dec_space(v.value())
}

/// Add `delta` references to a blob; the row must exist.
fn adjust_ref(
    w: &WriteTransaction,
    no: u64,
    id: &BlobId,
    delta: i64,
    now: u64,
) -> Result<(), StoreError> {
    let mut t = w.open_table(BLOBS)?;
    let mut row = dec_row(
        t.get((no, id.as_bytes().as_slice()))?
            .ok_or_else(|| StoreError::NoBlob(id.to_hex()))?
            .value(),
    )?;
    row.refs = if delta >= 0 {
        row.refs + delta as u64
    } else {
        row.refs
            .checked_sub(delta.unsigned_abs())
            .ok_or(StoreError::Corrupt("blob reference count below zero"))?
    };
    if row.refs == 0 {
        row.since_ms = now;
    }
    t.insert((no, id.as_bytes().as_slice()), enc_row(&row).as_slice())?;
    Ok(())
}

/// Erase a blob doc's rows in `w` (deleting the doc), releasing the reference each of its
/// versions held on its blob at `now`. A space whose key was purged has no blob rows left to
/// release, and a missing row has nothing to release either: neither stops the delete.
pub(super) fn delete_rows(
    w: &WriteTransaction,
    id: DocId,
    space: &str,
    now: u64,
) -> Result<(), StoreError> {
    let mut blobs = Vec::new();
    for kv in w
        .open_table(BLOB_VERSIONS)?
        .range((id, 0)..=(id, u64::MAX))?
    {
        blobs.push(dec_version(kv?.1.value())?.blob);
    }
    if !blobs.is_empty() {
        match space_in(w, space) {
            Ok(k) => {
                for b in &blobs {
                    match adjust_ref(w, k.no, b, -1, now) {
                        Ok(()) | Err(StoreError::NoBlob(_)) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            Err(StoreError::NoBlobSpace) => {}
            Err(e) => return Err(e),
        }
    }
    w.open_table(BLOB_VERSIONS)?
        .retain_in((id, 0)..=(id, u64::MAX), |_, _| false)?;
    w.open_table(BLOB_SYNC)?
        .retain_in((id, 0)..=(id, u8::MAX), |_, _| false)?;
    w.open_table(BLOB_APPROVALS)?
        .retain_in((id, 0)..=(id, u64::MAX), |_, _| false)?;
    Ok(())
}

fn last_vid(w: &WriteTransaction, id: DocId) -> Result<VersionId, StoreError> {
    let t = w.open_table(BLOB_VERSIONS)?;
    let last = t.range((id, 0)..=(id, u64::MAX))?.next_back().transpose()?;
    Ok(last.map_or(0, |(k, _)| k.value().1))
}

impl DocStore {
    // ----- blob index (crate-private: the blob store keeps files and rows consistent) -----

    pub(crate) fn blob_space(&self, space: &str) -> Result<Option<SpaceKey>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(BLOB_SPACES)?;
        let v = t.get(space)?;
        v.map(|v| dec_space(v.value())).transpose()
    }

    /// The space's key, created with `fresh` if it has none.
    pub(crate) fn blob_space_or_create(
        &self,
        space: &str,
        fresh: [u8; 32],
    ) -> Result<SpaceKey, StoreError> {
        let w = self.begin_write()?;
        let k = {
            let mut t = w.open_table(BLOB_SPACES)?;
            let found = t.get(space)?.map(|v| dec_space(v.value())).transpose()?;
            match found {
                Some(k) => k,
                None => {
                    let mut m = w.open_table(META)?;
                    let no = m.get("next_blob_space")?.map(|v| v.value()).unwrap_or(1);
                    m.insert("next_blob_space", no + 1)?;
                    let k = SpaceKey { no, key: fresh };
                    t.insert(space, enc_space(&k).as_slice())?;
                    k
                }
            }
        };
        w.commit()?;
        Ok(k)
    }

    /// Every live space number (folders of other numbers belong to purged spaces).
    pub(crate) fn blob_space_nos(&self) -> Result<Vec<u64>, StoreError> {
        let r = self.begin_read()?;
        let mut out = Vec::new();
        for kv in r.open_table(BLOB_SPACES)?.iter()? {
            out.push(dec_space(kv?.1.value())?.no);
        }
        Ok(out)
    }

    pub(crate) fn blob_row(&self, no: u64, id: &BlobId) -> Result<Option<BlobRow>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(BLOBS)?;
        let v = t.get((no, id.as_bytes().as_slice()))?;
        v.map(|v| dec_row(v.value())).transpose()
    }

    /// Index a stored blob: a new row (no references) or, for one already there, a fresh
    /// grace period. Returns whether the row is new.
    pub(crate) fn blob_index(
        &self,
        no: u64,
        id: &BlobId,
        size: u64,
        now: u64,
    ) -> Result<bool, StoreError> {
        let w = self.begin_write()?;
        let new = {
            let mut t = w.open_table(BLOBS)?;
            let k = (no, id.as_bytes().as_slice());
            let old = t.get(k)?.map(|v| dec_row(v.value())).transpose()?;
            match old {
                Some(mut row) => {
                    if row.size != size {
                        return Err(StoreError::Corrupt("blob size differs from its index"));
                    }
                    if row.refs == 0 {
                        row.since_ms = now;
                        t.insert(k, enc_row(&row).as_slice())?;
                    }
                    false
                }
                None => {
                    let row = BlobRow {
                        size,
                        refs: 0,
                        since_ms: now,
                    };
                    t.insert(k, enc_row(&row).as_slice())?;
                    true
                }
            }
        };
        w.commit()?;
        Ok(new)
    }

    /// Add `delta` references to a blob of `space` (one durable transaction).
    pub(crate) fn blob_adjust_ref(
        &self,
        space: &str,
        id: &BlobId,
        delta: i64,
        now: u64,
    ) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        let k = space_in(&w, space)?;
        adjust_ref(&w, k.no, id, delta, now)?;
        w.commit()?;
        Ok(())
    }

    /// Unreferenced blobs whose grace has run out, as `(space number, id, size)`.
    pub(crate) fn blob_collectable(
        &self,
        now: u64,
        grace_ms: u64,
    ) -> Result<Vec<(u64, BlobId, u64)>, StoreError> {
        let r = self.begin_read()?;
        let mut out = Vec::new();
        for kv in r.open_table(BLOBS)?.iter()? {
            let (k, v) = kv?;
            let row = dec_row(v.value())?;
            if row.refs == 0 && now.saturating_sub(row.since_ms) >= grace_ms {
                let (no, id) = k.value();
                let id = BlobId::from_slice(id).ok_or(StoreError::Corrupt("blob id"))?;
                out.push((no, id, row.size));
            }
        }
        Ok(out)
    }

    /// Remove index rows that are still unreferenced (one transaction); returns those removed.
    pub(crate) fn blob_remove_unreferenced(
        &self,
        rows: &[(u64, BlobId, u64)],
    ) -> Result<Vec<(u64, BlobId, u64)>, StoreError> {
        let w = self.begin_write()?;
        let mut gone = Vec::new();
        {
            let mut t = w.open_table(BLOBS)?;
            for (no, id, size) in rows {
                let k = (*no, id.as_bytes().as_slice());
                let row = t.get(k)?.map(|v| dec_row(v.value())).transpose()?;
                if row.is_some_and(|r| r.refs == 0) {
                    t.remove(k)?;
                    gone.push((*no, *id, *size));
                }
            }
        }
        w.commit()?;
        Ok(gone)
    }

    /// Whether `(no, id)` has an index row (for the orphan-file sweep).
    pub(crate) fn blob_indexed(&self, no: u64, id: &BlobId) -> Result<bool, StoreError> {
        Ok(self.blob_row(no, id)?.is_some())
    }

    /// Destroy a space's key and every index row of its blobs, in one transaction. Returns
    /// the key's number and the rows removed (`None`: the space had no key).
    pub(crate) fn blob_purge_space(&self, space: &str) -> Result<Option<PurgedSpace>, StoreError> {
        let w = self.begin_write()?;
        let out = {
            let mut spaces = w.open_table(BLOB_SPACES)?;
            let found = spaces
                .remove(space)?
                .map(|v| dec_space(v.value()))
                .transpose()?;
            match found {
                None => None,
                Some(k) => {
                    let mut t = w.open_table(BLOBS)?;
                    let mut rows = Vec::new();
                    for kv in t.range((k.no, [].as_slice())..)? {
                        let (key, v) = kv?;
                        let (no, id) = key.value();
                        if no != k.no {
                            break;
                        }
                        let id = BlobId::from_slice(id).ok_or(StoreError::Corrupt("blob id"))?;
                        rows.push((id, dec_row(v.value())?.size));
                    }
                    for (id, _) in &rows {
                        t.remove((k.no, id.as_bytes().as_slice()))?;
                    }
                    Some((k.no, rows))
                }
            }
        };
        w.commit()?;
        Ok(out)
    }

    /// (blobs, bytes, of which unreferenced bytes) over the whole index.
    pub(crate) fn blob_totals(&self) -> Result<(u64, u64, u64), StoreError> {
        let r = self.begin_read()?;
        let (mut n, mut bytes, mut unref) = (0, 0, 0);
        for kv in r.open_table(BLOBS)?.iter()? {
            let row = dec_row(kv?.1.value())?;
            n += 1;
            bytes += row.size;
            if row.refs == 0 {
                unref += row.size;
            }
        }
        Ok((n, bytes, unref))
    }

    // ----- blob docs -----

    /// Create a blob doc with its first version (taking a reference on its blob) and,
    /// for a doc with a file, its synced state, in one transaction.
    pub(crate) fn create_blob_doc(
        &self,
        meta: &DocMeta,
        first: &BlobVersion,
        sync: Option<&BlobSync>,
    ) -> Result<(DocId, VersionId), StoreError> {
        let w = self.begin_write()?;
        let id = {
            let mut paths = w.open_table(PATHS)?;
            let k = meta.key.index();
            if paths.get(k.as_str())?.is_some() {
                return Err(StoreError::KeyTaken(meta.key.clone()));
            }
            let mut m = w.open_table(META)?;
            let id = m.get("next_doc_id")?.map(|v| v.value()).unwrap_or(1);
            m.insert("next_doc_id", id + 1)?;
            paths.insert(k.as_str(), id)?;
            w.open_table(DOCS)?
                .insert(id, super::enc_meta(meta).as_slice())?;
            id
        };
        let space = space_in(&w, &meta.key.space)?;
        adjust_ref(&w, space.no, &first.blob, 1, first.at_ms)?;
        put_version(&w, id, 1, first)?;
        if let Some(s) = sync {
            w.open_table(BLOB_SYNC)?
                .insert((id, SYNCED), enc_sync(s).as_slice())?;
        }
        w.commit()?;
        Ok((id, 1))
    }

    /// Append versions to a blob doc (each takes a reference on its blob, and one written on
    /// someone's behalf gets a `Pending` [`Approval`]) and, when given, set its synced state
    /// (clearing a write-back in flight), in one transaction. Fails with
    /// [`StoreError::LogMoved`] unless the doc's latest version is `expect_last`.
    pub(crate) fn commit_blob_versions(
        &self,
        id: DocId,
        space: &str,
        expect_last: VersionId,
        versions: &[BlobVersion],
        sync: Option<&BlobSync>,
    ) -> Result<Vec<VersionId>, StoreError> {
        let w = self.begin_write()?;
        let mut last = last_vid(&w, id)?;
        if last != expect_last {
            return Err(StoreError::LogMoved(id, expect_last, last));
        }
        let k = space_in(&w, space)?;
        let mut vids = Vec::with_capacity(versions.len());
        for v in versions {
            adjust_ref(&w, k.no, &v.blob, 1, v.at_ms)?;
            last += 1;
            put_version(&w, id, last, v)?;
            vids.push(last);
        }
        if let Some(s) = sync {
            let mut t = w.open_table(BLOB_SYNC)?;
            t.insert((id, SYNCED), enc_sync(s).as_slice())?;
            t.remove((id, PENDING))?;
        }
        w.commit()?;
        Ok(vids)
    }

    /// Every version of a blob doc, oldest first.
    pub fn blob_versions(&self, id: DocId) -> Result<Vec<(VersionId, BlobVersion)>, StoreError> {
        let r = self.begin_read()?;
        let mut out = Vec::new();
        for kv in r
            .open_table(BLOB_VERSIONS)?
            .range((id, 0)..=(id, u64::MAX))?
        {
            let (k, v) = kv?;
            out.push((k.value().1, dec_version(v.value())?));
        }
        Ok(out)
    }

    pub fn blob_version(&self, id: DocId, vid: VersionId) -> Result<BlobVersion, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(BLOB_VERSIONS)?;
        let v = t.get((id, vid))?.ok_or(StoreError::NoVersion(id, vid))?;
        dec_version(v.value())
    }

    /// The latest version of a blob doc: its current content.
    pub fn blob_current(&self, id: DocId) -> Result<(VersionId, BlobVersion), StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(BLOB_VERSIONS)?;
        let last = t.range((id, 0)..=(id, u64::MAX))?.next_back().transpose()?;
        let (k, v) = last.ok_or(StoreError::NoDoc(id))?;
        Ok((k.value().1, dec_version(v.value())?))
    }

    /// Name (or rename) a blob doc's version.
    pub fn name_blob_version(
        &self,
        id: DocId,
        vid: VersionId,
        name: &str,
    ) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        {
            let mut t = w.open_table(BLOB_VERSIONS)?;
            let mut v = dec_version(
                t.get((id, vid))?
                    .ok_or(StoreError::NoVersion(id, vid))?
                    .value(),
            )?;
            v.name = Some(name.to_owned());
            t.insert((id, vid), enc_version(&v).as_slice())?;
        }
        w.commit()?;
        Ok(())
    }

    /// Delete a blob doc's version (retention), releasing its reference on the blob. The
    /// store's version rule applies: a named version is refused ([`StoreError::NamedVersion`]),
    /// and so is the latest (it is the current content).
    pub fn delete_blob_version(
        &self,
        id: DocId,
        vid: VersionId,
        now: u64,
    ) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        {
            let key = {
                let docs = w.open_table(DOCS)?;
                let m = docs.get(id)?.ok_or(StoreError::NoDoc(id))?;
                super::dec_meta(m.value())?.key
            };
            if last_vid(&w, id)? == vid {
                return Err(StoreError::CurrentVersion(id, vid));
            }
            let mut t = w.open_table(BLOB_VERSIONS)?;
            let v = dec_version(
                t.get((id, vid))?
                    .ok_or(StoreError::NoVersion(id, vid))?
                    .value(),
            )?;
            if v.name.is_some() {
                return Err(StoreError::NamedVersion(id, vid));
            }
            t.remove((id, vid))?;
            let k = space_in(&w, &key.space)?;
            adjust_ref(&w, k.no, &v.blob, -1, now)?;
        }
        w.commit()?;
        Ok(())
    }

    /// Purge versions `vids` of blob doc `id` (the current one is refused:
    /// [`StoreError::CurrentVersion`]), in one transaction: each keeps its entry with
    /// `redacted` set and the zero id (its keyed id is a digest of the content: it goes too),
    /// and releases its reference on its blob at `now`. Returns the blobs released, with the
    /// references each still has, and the approval records of the purged versions (format
    /// 4). Already purged versions are skipped.
    ///
    /// An approval record is kept as it is: like the version's entry (who, for whom, when),
    /// it is metadata, and it holds no content and no blob id (its fields are the agent, the
    /// person, the state, the settler and the time; the codec has no other field), so there
    /// is nothing in it to redact. It is counted, so the purge's reach is visible.
    pub(crate) fn blob_purge_versions(
        &self,
        id: DocId,
        vids: &[VersionId],
        now: u64,
    ) -> Result<(Vec<PurgedBlob>, u64), StoreError> {
        let w = self.begin_write()?;
        let out = {
            let key = {
                let docs = w.open_table(DOCS)?;
                let m = docs.get(id)?.ok_or(StoreError::NoDoc(id))?;
                super::dec_meta(m.value())?.key
            };
            let last = last_vid(&w, id)?;
            let space = space_in(&w, &key.space).ok();
            let mut released: Vec<(BlobId, u64)> = Vec::new();
            let mut approvals = 0;
            for &vid in vids {
                if vid == last {
                    return Err(StoreError::CurrentVersion(id, vid));
                }
                let mut v = {
                    let t = w.open_table(BLOB_VERSIONS)?;
                    let g = t.get((id, vid))?.ok_or(StoreError::NoVersion(id, vid))?;
                    dec_version(g.value())?
                };
                if v.redacted {
                    continue;
                }
                if let Some(a) = w.open_table(BLOB_APPROVALS)?.get((id, vid))? {
                    // decoded: a damaged record stops the purge rather than riding along
                    dec_approval(a.value())?;
                    approvals += 1;
                }
                let blob = v.blob;
                v.blob = BlobId::from_slice(&[0; 32]).expect("32 bytes");
                v.redacted = true;
                w.open_table(BLOB_VERSIONS)?
                    .insert((id, vid), enc_version(&v).as_slice())?;
                if let Some(k) = &space {
                    match adjust_ref(&w, k.no, &blob, -1, now) {
                        Ok(()) | Err(StoreError::NoBlob(_)) => {}
                        Err(e) => return Err(e),
                    }
                }
                released.push((blob, v.size));
            }
            let mut out = Vec::new();
            if let Some(k) = &space {
                let t = w.open_table(BLOBS)?;
                let mut seen = std::collections::HashSet::new();
                for (b, size) in released {
                    if !seen.insert(b) {
                        continue;
                    }
                    let refs = t
                        .get((k.no, b.as_bytes().as_slice()))?
                        .map(|v| dec_row(v.value()))
                        .transpose()?
                        .map_or(0, |r| r.refs);
                    out.push((k.no, b, size, refs));
                }
            }
            (out, approvals)
        };
        w.commit()?;
        Ok(out)
    }

    // ----- approvals of on-behalf blob versions (format 4) -----

    /// The approval record of blob version `vid`, if an agent wrote it on someone's behalf.
    pub fn blob_approval(&self, id: DocId, vid: VersionId) -> Result<Option<Approval>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(BLOB_APPROVALS)?;
        let v = t.get((id, vid))?;
        v.map(|v| dec_approval(v.value())).transpose()
    }

    /// Every approval record of a blob doc, oldest version first.
    pub fn blob_approvals(&self, id: DocId) -> Result<Vec<(VersionId, Approval)>, StoreError> {
        let r = self.begin_read()?;
        let mut out = Vec::new();
        for kv in r
            .open_table(BLOB_APPROVALS)?
            .range((id, 0)..=(id, u64::MAX))?
        {
            let (k, v) = kv?;
            out.push((k.value().1, dec_approval(v.value())?));
        }
        Ok(out)
    }

    /// Replace an existing blob approval record ([`StoreError::NoApproval`] when there is
    /// none). The store doesn't check who may do it: sessions do
    /// ([`crate::session::BlobSession::set_approval`]).
    pub fn set_blob_approval(
        &self,
        id: DocId,
        vid: VersionId,
        a: &Approval,
    ) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        {
            let mut t = w.open_table(BLOB_APPROVALS)?;
            if t.get((id, vid))?.is_none() {
                return Err(StoreError::NoApproval(id, vid));
            }
            t.insert((id, vid), enc_approval(a).as_slice())?;
        }
        w.commit()?;
        Ok(())
    }

    // ----- blob-doc disk sync (crate-private: sessions keep these consistent) -----

    pub(crate) fn blob_sync_states(
        &self,
        id: DocId,
    ) -> Result<(Option<BlobSync>, Option<BlobSync>), StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(BLOB_SYNC)?;
        let get = |slot| -> Result<Option<BlobSync>, StoreError> {
            t.get((id, slot))?.map(|v| dec_sync(v.value())).transpose()
        };
        Ok((get(SYNCED)?, get(PENDING)?))
    }

    /// Set the synced state (clearing a write-back in flight).
    pub(crate) fn set_blob_synced(&self, id: DocId, s: &BlobSync) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        {
            let mut t = w.open_table(BLOB_SYNC)?;
            t.insert((id, SYNCED), enc_sync(s).as_slice())?;
            t.remove((id, PENDING))?;
        }
        w.commit()?;
        Ok(())
    }

    /// Record a write-back about to happen (before the rename), so a crash right after it
    /// is recognised on the next open as the kit's own write, not an outside edit.
    pub(crate) fn set_blob_pending(&self, id: DocId, s: &BlobSync) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        w.open_table(BLOB_SYNC)?
            .insert((id, PENDING), enc_sync(s).as_slice())?;
        w.commit()?;
        Ok(())
    }

    pub(crate) fn clear_blob_pending(&self, id: DocId) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        w.open_table(BLOB_SYNC)?.remove((id, PENDING))?;
        w.commit()?;
        Ok(())
    }
}
