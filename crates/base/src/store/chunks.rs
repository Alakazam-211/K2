//! Chunked tier-0 versions in the store (`docs/design/version-storage-r2.md`, sections 1, 5 and
//! 8; `docs/SESSIONS.md`, "Chunked versions"). The bytes are segment files under the blob
//! folder ([`crate::blob`]); these tables say what each file holds and what each version names.
//!
//! - [`DOC_CHUNK_KEYS`]: doc → its chunk key (32 random bytes: a chunk's id is
//!   `HMAC-SHA256(chunk key, chunk bytes)`), its chunk sizes and its next segment number. Made by
//!   the doc's first chunked version; the sizes never change after.
//! - [`DOC_CHUNKS`]: (doc, chunk id) → where the chunk is (segment, offset, length), its
//!   references and when it was made. **A chunk's location lives here only**: a manifest names
//!   ids, never places.
//! - [`DOC_MANIFESTS`]: (doc, file id) → the versions naming it, the file's size and sizes code,
//!   and its root: the manifest's own chunks. The manifest's entries (each chunk id and its
//!   length, in order) are cut with the doc's chunker and stored as chunks of the doc, counted
//!   as any chunk.
//! - [`DOC_SEGMENTS`]: (doc, segment) → everything the segment file holds, a copy no chunk row
//!   points at included.
//! - [`DOC_REMOVALS`]: (doc, segment) → the removal list: a segment file a compaction's commit
//!   took out of the tables, which the blob store deletes after the commit (and every open
//!   finishes), then clears from the list in its own transaction.
//!
//! The first four are created lazily, by a store's first chunked version, in the transaction
//! that stamps it format 8 with the chunked feature, the removal list by the first compaction.
//! A store without them reads as holding no chunked version. They are per-doc tables
//! ([`super::DOC_TABLES`]): a rewrite copies them, and a doc delete erases them after releasing
//! the doc's whole-file references.
//!
//! **Compaction** ([`plan_in`], [`apply_in`]; `docs/design/version-storage-r2.md`, section 4)
//! works from the segment lists: an entry no chunk row points at is a **dead range** (a dropped
//! chunk's bytes, or a copy the equal-work rule wrote). A segment chosen for compaction gets a new
//! segment holding its kept chunks only, written by the blob store before the commit; the commit
//! points the kept chunks' rows at it, writes its list, drops the old segment's row and puts the
//! old file on the removal list. A segment with nothing kept is just removed.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use redb::{ReadableTable, TableDefinition, TableHandle, WriteTransaction};

use super::enc::{R, W};
use super::features::{FEATURE_CHUNKED, REQUIRED_FEATURES};
use super::{history, DocId, DocStore, StoreError, META, STORE_FORMAT_CHUNKED};
use crate::blob::BlobId;

/// Doc → its chunk key record.
pub(super) const DOC_CHUNK_KEYS: TableDefinition<u64, &[u8]> =
    TableDefinition::new("doc_chunk_keys");
/// (doc, chunk id) → its row.
pub(super) const DOC_CHUNKS: TableDefinition<(u64, &[u8]), &[u8]> =
    TableDefinition::new("doc_chunks");
/// (doc, file id) → its manifest.
pub(super) const DOC_MANIFESTS: TableDefinition<(u64, &[u8]), &[u8]> =
    TableDefinition::new("doc_manifests");
/// (doc, segment) → what the segment file holds.
pub(super) const DOC_SEGMENTS: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("doc_segments");
/// (doc, segment) → the space number of a segment file to delete (the removal list).
pub(super) const DOC_REMOVALS: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("doc_removals");

fn enc_removal(no: u64) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1).u64(no);
    w.0
}

fn dec_removal(b: &[u8]) -> Result<u64, StoreError> {
    let mut r = R::new(b, "doc removal");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("doc removal version"));
    }
    let no = r.u64()?;
    r.end()?;
    Ok(no)
}

/// A chunk's id within its doc: `HMAC-SHA256(doc chunk key, chunk bytes)`.
pub(crate) type ChunkId = [u8; 32];

/// What a segment file holds: (chunk id, offset, length), in file order.
pub(crate) type SegmentList = Vec<(ChunkId, u64, u32)>;

/// One manifest entry's bytes: a chunk id and its length (u32, little-endian).
const ENTRY: usize = 36;

/// A doc's chunk key record.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ChunkKey {
    pub key: [u8; 32],
    /// [`crate::blob::cdc::Sizes::code`].
    pub sizes: u8,
    /// The number the doc's next segment takes.
    pub next_segment: u64,
}

impl std::fmt::Debug for ChunkKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ChunkKey {{ key: <secret>, sizes: {}, next_segment: {} }}",
            self.sizes, self.next_segment
        )
    }
}

fn enc_key(k: &ChunkKey) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1).bytes(&k.key).u8(k.sizes).u64(k.next_segment);
    w.0
}

fn dec_key(b: &[u8]) -> Result<ChunkKey, StoreError> {
    let mut r = R::new(b, "doc chunk key");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("doc chunk key version"));
    }
    let key = id32(&r.bytes()?, "doc chunk key")?;
    let k = ChunkKey {
        key,
        sizes: r.u8()?,
        next_segment: r.u64()?,
    };
    r.end()?;
    Ok(k)
}

/// One chunk's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkRow {
    pub segment: u64,
    pub offset: u64,
    pub len: u32,
    /// Manifest entries naming it (a manifest's own chunks included), one per occurrence.
    pub refs: u64,
    pub since_ms: u64,
}

fn enc_chunk(c: &ChunkRow) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1)
        .u64(c.segment)
        .u64(c.offset)
        .u32(c.len)
        .u64(c.refs)
        .u64(c.since_ms);
    w.0
}

fn dec_chunk(b: &[u8]) -> Result<ChunkRow, StoreError> {
    let mut r = R::new(b, "doc chunk");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("doc chunk version"));
    }
    let c = ChunkRow {
        segment: r.u64()?,
        offset: r.u64()?,
        len: r.u32()?,
        refs: r.u64()?,
        since_ms: r.u64()?,
    };
    r.end()?;
    Ok(c)
}

/// One manifest row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Manifest {
    /// The versions naming it.
    pub refs: u64,
    pub size: u64,
    pub sizes: u8,
    /// The manifest's own chunks (its entries' bytes, cut with the doc's chunker), in order.
    pub root: Vec<(ChunkId, u32)>,
}

fn enc_manifest(m: &Manifest) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1).u64(m.refs).u64(m.size).u8(m.sizes);
    w.u32(u32::try_from(m.root.len()).expect("a manifest root over u32 chunks"));
    for (id, len) in &m.root {
        w.bytes(id).u32(*len);
    }
    w.0
}

fn dec_manifest(b: &[u8]) -> Result<Manifest, StoreError> {
    let mut r = R::new(b, "doc manifest");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("doc manifest version"));
    }
    let (refs, size, sizes) = (r.u64()?, r.u64()?, r.u8()?);
    let n = r.u32()? as usize;
    // each entry is at least 40 bytes: bound n by what's left before allocating
    if n > r.left() / 40 {
        return Err(StoreError::Corrupt("doc manifest"));
    }
    let mut root = Vec::with_capacity(n);
    for _ in 0..n {
        root.push((id32(&r.bytes()?, "doc manifest")?, r.u32()?));
    }
    r.end()?;
    Ok(Manifest {
        refs,
        size,
        sizes,
        root,
    })
}

fn enc_segment(s: &SegmentList) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1)
        .u32(u32::try_from(s.len()).expect("a segment over u32 chunks"));
    for (id, off, len) in s {
        w.bytes(id).u64(*off).u32(*len);
    }
    w.0
}

fn dec_segment(b: &[u8]) -> Result<SegmentList, StoreError> {
    let mut r = R::new(b, "doc segment");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("doc segment version"));
    }
    let n = r.u32()? as usize;
    if n > r.left() / 48 {
        return Err(StoreError::Corrupt("doc segment"));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push((id32(&r.bytes()?, "doc segment")?, r.u64()?, r.u32()?));
    }
    r.end()?;
    Ok(out)
}

fn id32(b: &[u8], what: &'static str) -> Result<[u8; 32], StoreError> {
    b.try_into().map_err(|_| StoreError::Corrupt(what))
}

/// A manifest's entries as stored in its own chunks: each chunk id and its length, in order.
pub(crate) fn enc_entries(e: &[(ChunkId, u32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(e.len() * ENTRY);
    for (id, len) in e {
        out.extend_from_slice(id);
        out.extend_from_slice(&len.to_le_bytes());
    }
    out
}

/// [`enc_entries`]' bytes back as entries (`None`: not a whole number of entries).
pub(crate) fn dec_entries(b: &[u8]) -> Option<Vec<(ChunkId, u32)>> {
    if b.len() % ENTRY != 0 {
        return None;
    }
    Some(
        b.chunks_exact(ENTRY)
            .map(|e| {
                let id: ChunkId = e[..32].try_into().expect("32 bytes");
                let len = u32::from_le_bytes(e[32..].try_into().expect("4 bytes"));
                (id, len)
            })
            .collect(),
    )
}

/// A chunk's id: `HMAC-SHA256(doc chunk key, bytes)`.
pub(crate) fn chunk_id(key: &[u8; 32], bytes: &[u8]) -> ChunkId {
    crate::hash::hmac(key, bytes)
}

/// Where doc `doc`'s segments live: `<blob folder>/segments/<space no>/<doc>/`.
pub(crate) fn segment_dir(blob_dir: &Path, no: u64, doc: DocId) -> PathBuf {
    blob_dir
        .join(SEGMENTS)
        .join(no.to_string())
        .join(doc.to_string())
}

/// Segment `seg` of doc `doc`: `<n>.seg` in its folder (a per-doc counter, never a hash).
pub(crate) fn segment_path(blob_dir: &Path, no: u64, doc: DocId, seg: u64) -> PathBuf {
    segment_dir(blob_dir, no, doc).join(format!("{seg}.seg"))
}

/// The segments' top folder under the blob folder.
pub(crate) const SEGMENTS: &str = "segments";

/// `len` bytes at `offset` of the file at `path`.
pub(crate) fn read_at(path: &Path, offset: u64, len: u32) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    read_at_file(&mut f, offset, len)
}

/// `len` bytes at `offset` of `f`.
pub(crate) fn read_at_file(
    f: &mut std::fs::File,
    offset: u64,
    len: u32,
) -> std::io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(offset))?;
    let mut b = vec![0u8; len as usize];
    f.read_exact(&mut b)?;
    Ok(b)
}

/// Why a manifest's entries couldn't be read.
#[derive(Debug)]
pub(crate) enum EntriesError {
    Store(StoreError),
    /// A segment file is missing.
    Missing(PathBuf),
    /// The bytes don't match their chunk's id, or don't parse.
    Damaged(String),
    Io(String),
}

impl From<StoreError> for EntriesError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

impl From<EntriesError> for StoreError {
    fn from(e: EntriesError) -> Self {
        match e {
            EntriesError::Store(e) => e,
            EntriesError::Missing(_) | EntriesError::Damaged(_) => {
                StoreError::Corrupt("a manifest's chunks")
            }
            EntriesError::Io(m) => StoreError::Db(format!("reading a manifest's chunks: {m}")),
        }
    }
}

/// A manifest's entries, from its root chunks: each root chunk's row from `chunk`, its bytes
/// from its segment file (under `blob_dir`), checked against its id under `key`.
pub(crate) fn read_entries(
    chunk: &dyn Fn(&ChunkId) -> Result<Option<ChunkRow>, StoreError>,
    blob_dir: &Path,
    no: u64,
    doc: DocId,
    key: &[u8; 32],
    root: &[(ChunkId, u32)],
) -> Result<Vec<(ChunkId, u32)>, EntriesError> {
    let mut bytes = Vec::new();
    for (id, len) in root {
        let row = chunk(id)?.ok_or(StoreError::Corrupt("a manifest chunk without its row"))?;
        if row.len != *len {
            return Err(EntriesError::Damaged("a manifest chunk's length".into()));
        }
        let path = segment_path(blob_dir, no, doc, row.segment);
        let b = match read_at(&path, row.offset, row.len) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(EntriesError::Missing(path))
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(EntriesError::Damaged("a short segment".into()))
            }
            Err(e) => return Err(EntriesError::Io(e.to_string())),
        };
        if chunk_id(key, &b) != *id {
            return Err(EntriesError::Damaged(
                "a manifest chunk doesn't match its id".into(),
            ));
        }
        bytes.extend_from_slice(&b);
    }
    dec_entries(&bytes).ok_or_else(|| EntriesError::Damaged("a manifest's entries".into()))
}

/// What a chunked write commits with its version ([`DocStore::commit_blob_versions_with`]):
/// the file's manifest, the chunk rows it makes or counts, and its new segment's row.
#[derive(Debug, Clone)]
pub(crate) struct ChunkCommit {
    pub doc: DocId,
    pub file: BlobId,
    pub size: u64,
    /// The chunk key the write cut and hashed with, and whether the write made it (the doc's
    /// first chunked version).
    pub key: ChunkKey,
    pub new_key: bool,
    /// The segment the write wrote, and what it holds (`None`: no new bytes).
    pub segment: Option<(u64, SegmentList)>,
    /// The manifest's entries.
    pub entries: Vec<(ChunkId, u32)>,
    /// The manifest's own chunks.
    pub root: Vec<(ChunkId, u32)>,
    /// The chunks the write took on the cheap path, with no bytes written: their rows must
    /// still be there at commit.
    pub reused: Vec<ChunkId>,
    pub now: u64,
}

/// Whether the planted fault [`crate::differential::ChunkPlant::CommitSkipsChunkCheck`] is on.
fn planted_skips_check() -> bool {
    plant_on!(
        crate::differential::chunk_plant()
            == Some(crate::differential::ChunkPlant::CommitSkipsChunkCheck)
    )
}

/// Apply `c` in `w` (the version's transaction): the doc's key record, the manifest (made with
/// no references yet, or as it is when the doc already holds one for these bytes), each chunk
/// of a new manifest made or counted, the segment's row, and the store's stamp (format 8, the
/// chunked feature). **The same rows are written either way**: when the doc already holds the
/// manifest, each of its chunk rows is written again with its count as it is (its `since`
/// reset) and the manifest too, so the transaction a write commits has the shape a brand-new
/// file's has, whoever wrote it (`docs/design/version-storage-r2.md`, section 6). **Fails
/// closed**: a chunk `c` reuses whose row is gone, or a key record other than the one the write
/// used, refuses it all ([`StoreError::ChunkGone`]).
///
/// Returns `false`, changing nothing, when the doc holds no manifest for these bytes and the
/// space holds them as a whole-file blob: the version takes that blob, as a whole-file write
/// would (the write's segment then has no row, and `gc` removes it).
pub(super) fn apply(w: &WriteTransaction, no: u64, c: &ChunkCommit) -> Result<bool, StoreError> {
    apply_with(w, no, c, false)
}

/// [`apply`] for bytes the space holds as a whole-file blob that a rechunk converts
/// ([`DocStore::blob_rechunk_commit`]): the manifest is made whatever the blob row.
pub(super) fn apply_over_whole(
    w: &WriteTransaction,
    no: u64,
    c: &ChunkCommit,
) -> Result<(), StoreError> {
    apply_with(w, no, c, true).map(|_| ())
}

fn apply_with(
    w: &WriteTransaction,
    no: u64,
    c: &ChunkCommit,
    over_whole: bool,
) -> Result<bool, StoreError> {
    let doc = c.doc;
    let file = c.file.as_bytes().as_slice();
    let existing = manifest_in(w, doc, &c.file)?;
    let has_manifest = existing.is_some();
    if !has_manifest && !over_whole && w.open_table(super::blob::BLOBS)?.get((no, file))?.is_some()
    {
        return Ok(false);
    }
    {
        let mut keys = w.open_table(DOC_CHUNK_KEYS)?;
        let found = keys.get(doc)?.map(|v| dec_key(v.value())).transpose()?;
        let next = c.segment.as_ref().map_or(0, |(n, _)| n + 1);
        let rec = match found {
            None if c.new_key => ChunkKey {
                next_segment: next.max(c.key.next_segment),
                ..c.key.clone()
            },
            None => return Err(StoreError::ChunkGone),
            Some(k) if k.key != c.key.key || k.sizes != c.key.sizes => {
                return Err(StoreError::ChunkGone)
            }
            Some(k) => ChunkKey {
                next_segment: k.next_segment.max(next),
                ..k
            },
        };
        keys.insert(doc, enc_key(&rec).as_slice())?;
        chunk_work!(rows_written);
    }
    let skip = planted_skips_check();
    let mut chunks = w.open_table(DOC_CHUNKS)?;
    if !skip {
        for id in &c.reused {
            if chunks.get((doc, id.as_slice()))?.is_none() {
                return Err(StoreError::ChunkGone);
            }
        }
    }
    let placed: HashMap<&ChunkId, (u64, u32)> = c
        .segment
        .iter()
        .flat_map(|(_, list)| list.iter().map(|(id, off, len)| (id, (*off, *len))))
        .collect();
    let seg = c.segment.as_ref().map_or(0, |(n, _)| *n);
    for (id, _) in c.entries.iter().chain(&c.root) {
        let k = (doc, id.as_slice());
        let row = chunks.get(k)?.map(|v| dec_chunk(v.value())).transpose()?;
        let row = match (row, placed.get(id)) {
            (Some(mut r), _) => {
                // a manifest already held counts its chunks once, whichever versions name it
                if !has_manifest {
                    r.refs += 1;
                }
                // `since` is reset when a version takes a reference: a kept chunk's would
                // otherwise say its region existed when a later-purged version was written
                r.since_ms = c.now;
                r
            }
            (None, Some(&(offset, len))) if !has_manifest => ChunkRow {
                segment: seg,
                offset,
                len,
                refs: 1,
                since_ms: c.now,
            },
            (None, _) if skip => continue,
            (None, _) => return Err(StoreError::ChunkGone),
        };
        chunks.insert(k, enc_chunk(&row).as_slice())?;
        chunk_work!(rows_written);
    }
    let m = existing.unwrap_or_else(|| Manifest {
        refs: 0,
        size: c.size,
        sizes: c.key.sizes,
        root: c.root.clone(),
    });
    w.open_table(DOC_MANIFESTS)?
        .insert((doc, file), enc_manifest(&m).as_slice())?;
    chunk_work!(rows_written);
    drop(chunks);
    if let Some((n, list)) = &c.segment {
        w.open_table(DOC_SEGMENTS)?
            .insert((doc, *n), enc_segment(list).as_slice())?;
        chunk_work!(rows_written);
    }
    stamp(w)
}

/// Raise the store to format 8 with the chunked feature, never lowering either.
fn stamp(w: &WriteTransaction) -> Result<bool, StoreError> {
    let mut meta = w.open_table(META)?;
    let f = meta.get("store_format")?.map_or(0, |v| v.value());
    if f < STORE_FORMAT_CHUNKED {
        meta.insert("store_format", STORE_FORMAT_CHUNKED)?;
    }
    let word = meta.get(REQUIRED_FEATURES)?.map_or(0, |v| v.value());
    if word & FEATURE_CHUNKED == 0 {
        meta.insert(REQUIRED_FEATURES, word | FEATURE_CHUNKED)?;
    }
    Ok(true)
}

/// Whether doc `doc` holds a manifest for `file` in `w` (a store without the table holds none).
fn manifest_in(
    w: &WriteTransaction,
    doc: DocId,
    file: &BlobId,
) -> Result<Option<Manifest>, StoreError> {
    if !history::has_table(w, DOC_MANIFESTS.name())? {
        return Ok(None);
    }
    let t = w.open_table(DOC_MANIFESTS)?;
    let v = t.get((doc, file.as_bytes().as_slice()))?;
    v.map(|v| dec_manifest(v.value())).transpose()
}

/// Whether version bytes `file` of doc `doc` are stored chunked (its manifest is there), in
/// `w`. Every version of a doc naming the same bytes is stored the same way: chunked while the
/// doc holds their manifest, a whole-file blob otherwise.
pub(super) fn is_chunked(
    w: &WriteTransaction,
    doc: DocId,
    file: &BlobId,
) -> Result<bool, StoreError> {
    Ok(manifest_in(w, doc, file)?.is_some())
}

/// The versions naming doc `doc`'s manifest for `file`, in `w` (0: it has none).
pub(super) fn manifest_refs(
    w: &WriteTransaction,
    doc: DocId,
    file: &BlobId,
) -> Result<u64, StoreError> {
    Ok(manifest_in(w, doc, file)?.map_or(0, |m| m.refs))
}

/// A version naming `file` takes a reference on its manifest, in `w`: `false` when the doc
/// holds none (the version takes its whole-file blob's instead).
pub(super) fn take_ref(
    w: &WriteTransaction,
    doc: DocId,
    file: &BlobId,
) -> Result<bool, StoreError> {
    let Some(mut m) = manifest_in(w, doc, file)? else {
        return Ok(false);
    };
    m.refs += 1;
    w.open_table(DOC_MANIFESTS)?.insert(
        (doc, file.as_bytes().as_slice()),
        enc_manifest(&m).as_slice(),
    )?;
    Ok(true)
}

/// Whether the planted fault [`crate::differential::ChunkPlant::RetentionIgnoresChunkMarks`]
/// is on.
fn planted_ignores_marks() -> bool {
    plant_on!(
        crate::differential::chunk_plant()
            == Some(crate::differential::ChunkPlant::RetentionIgnoresChunkMarks)
    )
}

/// A version naming `file` drops its reference on its manifest, in `w`: `None` when the doc
/// holds none (the version held its whole-file blob's), else the references the manifest still
/// has. At no references the manifest goes and each of its chunks (its own included) loses one
/// reference; a chunk at none loses its row, its bytes a dead range in their segment until a
/// compaction, **unless `waiting` says a write marks it** (a write that reuses it, between its
/// mark and its commit): then its row stays, at no references, for the write's commit to count
/// (a later `gc` removes it once nothing waits on it). The manifest's entries are read from its
/// chunks in the blob folder `blob_dir` of space `no`. The ids of the rows it removes are added
/// to `erased`.
pub(super) fn drop_ref(
    w: &WriteTransaction,
    doc: DocId,
    no: u64,
    file: &BlobId,
    blob_dir: Option<&Path>,
    waiting: &dyn Fn(&ChunkId) -> bool,
    erased: &mut Vec<ChunkId>,
) -> Result<Option<u64>, StoreError> {
    let Some(mut m) = manifest_in(w, doc, file)? else {
        return Ok(None);
    };
    let fk = (doc, file.as_bytes().as_slice());
    m.refs = m
        .refs
        .checked_sub(1)
        .ok_or(StoreError::Corrupt("manifest reference count below zero"))?;
    if m.refs > 0 {
        w.open_table(DOC_MANIFESTS)?
            .insert(fk, enc_manifest(&m).as_slice())?;
        return Ok(Some(m.refs));
    }
    let dir = blob_dir.ok_or_else(|| {
        StoreError::Db("a chunked version is released with no blob folder open".into())
    })?;
    let key = w
        .open_table(DOC_CHUNK_KEYS)?
        .get(doc)?
        .map(|v| dec_key(v.value()))
        .transpose()?
        .ok_or(StoreError::Corrupt("a chunked doc without its chunk key"))?;
    let entries = {
        let t = w.open_table(DOC_CHUNKS)?;
        let get = |id: &ChunkId| -> Result<Option<ChunkRow>, StoreError> {
            t.get((doc, id.as_slice()))?
                .map(|v| dec_chunk(v.value()))
                .transpose()
        };
        read_entries(&get, dir, no, doc, &key.key, &m.root)?
    };
    w.open_table(DOC_MANIFESTS)?.remove(fk)?;
    // its structure index rows go with it, unless other rows still hold its bytes
    drop_index_rows(w, no, file)?;
    let mut t = w.open_table(DOC_CHUNKS)?;
    for (id, _) in entries.iter().chain(&m.root) {
        let k = (doc, id.as_slice());
        let mut row =
            t.get(k)?
                .map(|v| dec_chunk(v.value()))
                .transpose()?
                .ok_or(StoreError::Corrupt(
                    "a manifest names a chunk without its row",
                ))?;
        row.refs = row
            .refs
            .checked_sub(1)
            .ok_or(StoreError::Corrupt("chunk reference count below zero"))?;
        if row.refs == 0 && (planted_ignores_marks() || !waiting(id)) {
            t.remove(k)?;
            erased.push(*id);
        } else {
            t.insert(k, enc_chunk(&row).as_slice())?;
        }
    }
    Ok(Some(0))
}

/// Which segments a compaction takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Select {
    /// A purge's: every segment holding any dead range (a purged chunk's bytes included), so
    /// that when the purge ends every byte of every segment of the doc belongs to a kept chunk.
    /// `only_purged` is the planted fault `compaction-skips-dead-ranges`: only the segments
    /// holding a chunk the purge erased.
    Purge { only_purged: bool },
    /// The compaction retention deferred (`gc`'s): a segment less than half live.
    Sparse,
}

/// One segment a compaction takes: its kept chunks (in file order, at their offsets in it),
/// copied to segment `out` (`None`: nothing is kept, and the segment is just removed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SegMove {
    pub old: u64,
    pub kept: SegmentList,
    pub out: Option<u64>,
}

/// A compaction of one doc's segments ([`plan_in`]): what the blob store writes before the
/// commit, and what [`apply_in`] checks and commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Compaction {
    pub doc: DocId,
    /// The doc's chunk key (each copied chunk is checked against its id under it).
    pub key: [u8; 32],
    pub moves: Vec<SegMove>,
    /// The doc's next segment number before the compaction: the outputs take the next ones.
    pub next_segment: u64,
}

impl Compaction {
    /// The new segment's list for a move: its kept chunks, packed from offset 0.
    pub(crate) fn out_list(m: &SegMove) -> SegmentList {
        let mut at = 0u64;
        m.kept
            .iter()
            .map(|(id, _, len)| {
                let e = (*id, at, *len);
                at += u64::from(*len);
                e
            })
            .collect()
    }
}

/// Remove every chunk row of doc `doc` at no references, in `w` (the caller holds the doc's
/// lock, so no write of it waits on any).
fn erase_unreferenced_of(w: &WriteTransaction, doc: DocId) -> Result<(), StoreError> {
    let mut t = w.open_table(DOC_CHUNKS)?;
    let mut gone: Vec<ChunkId> = Vec::new();
    for kv in t.range((doc, [].as_slice())..(doc + 1, [].as_slice()))? {
        let (k, v) = kv?;
        if dec_chunk(v.value())?.refs == 0 {
            gone.push(id32(k.value().1, "doc chunk")?);
        }
    }
    for id in &gone {
        t.remove((doc, id.as_slice()))?;
    }
    Ok(())
}

/// The compaction `select` asks of doc `doc`, in `w`, after the transaction's own changes (a
/// purge's), with the doc's unreferenced chunk rows erased first: `None` when the doc has no
/// chunked state. `erased`: the chunks this transaction's purge erased.
pub(crate) fn plan_in(
    w: &WriteTransaction,
    doc: DocId,
    select: Select,
    erased: &[ChunkId],
) -> Result<Option<Compaction>, StoreError> {
    if !history::has_table(w, DOC_SEGMENTS.name())? {
        return Ok(None);
    }
    let Some(key) = w
        .open_table(DOC_CHUNK_KEYS)?
        .get(doc)?
        .map(|v| dec_key(v.value()))
        .transpose()?
    else {
        return Ok(None);
    };
    erase_unreferenced_of(w, doc)?;
    let segments: Vec<(u64, SegmentList)> = {
        let t = w.open_table(DOC_SEGMENTS)?;
        let mut out = Vec::new();
        for kv in t.range((doc, 0)..=(doc, u64::MAX))? {
            let (k, v) = kv?;
            out.push((k.value().1, dec_segment(v.value())?));
        }
        out
    };
    let chunks = w.open_table(DOC_CHUNKS)?;
    let purged: std::collections::HashSet<&ChunkId> = erased.iter().collect();
    let mut next = key.next_segment;
    let mut moves = Vec::new();
    for (seg, list) in segments {
        let mut kept = Vec::new();
        let (mut live, mut all) = (0u64, 0u64);
        for (id, off, len) in &list {
            all += u64::from(*len);
            let row = chunks
                .get((doc, id.as_slice()))?
                .map(|v| dec_chunk(v.value()))
                .transpose()?;
            if row.is_some_and(|r| r.segment == seg && r.offset == *off && r.len == *len) {
                kept.push((*id, *off, *len));
                live += u64::from(*len);
            }
        }
        let dead = kept.len() < list.len();
        let take = match select {
            Select::Purge { only_purged: false } => dead,
            Select::Purge { only_purged: true } => list.iter().any(|e| purged.contains(&e.0)),
            Select::Sparse => dead && live * 2 < all,
        };
        if !take {
            continue;
        }
        let out = (!kept.is_empty()).then(|| {
            next += 1;
            next - 1
        });
        moves.push(SegMove {
            old: seg,
            kept,
            out,
        });
    }
    Ok(Some(Compaction {
        doc,
        key: key.key,
        moves,
        next_segment: key.next_segment,
    }))
}

/// Commit compaction `plan` in `w` (space `no`), if it is still what `select` asks of the doc
/// ([`plan_in`] again, in `w`): each kept chunk's row points at its new segment, each new
/// segment's list is written, each old segment's row goes and its file goes on the removal
/// list, and the doc's segment counter moves past the outputs. `false`, changing nothing more,
/// when the doc changed since the plan (the caller drops the transaction and plans again).
pub(crate) fn apply_in(
    w: &WriteTransaction,
    no: u64,
    plan: Option<&Compaction>,
    select: Select,
    erased: &[ChunkId],
) -> Result<bool, StoreError> {
    let now = plan_in(w, plan.map_or(0, |p| p.doc), select, erased)?;
    let Some(plan) = plan else {
        return Ok(match now {
            None => true,
            Some(n) => n.moves.is_empty(),
        });
    };
    if now.as_ref() != Some(plan) {
        return Ok(false);
    }
    let doc = plan.doc;
    let mut outs = 0;
    {
        let mut chunks = w.open_table(DOC_CHUNKS)?;
        let mut segs = w.open_table(DOC_SEGMENTS)?;
        let mut removals = w.open_table(DOC_REMOVALS)?;
        for m in &plan.moves {
            if let Some(out) = m.out {
                let list = Compaction::out_list(m);
                for (id, off, _) in &list {
                    let k = (doc, id.as_slice());
                    let mut row = chunks
                        .get(k)?
                        .map(|v| dec_chunk(v.value()))
                        .transpose()?
                        .ok_or(StoreError::Corrupt("a kept chunk without its row"))?;
                    row.segment = out;
                    row.offset = *off;
                    chunks.insert(k, enc_chunk(&row).as_slice())?;
                }
                segs.insert((doc, out), enc_segment(&list).as_slice())?;
                outs += 1;
            }
            segs.remove((doc, m.old))?;
            removals.insert((doc, m.old), enc_removal(no).as_slice())?;
        }
    }
    let mut keys = w.open_table(DOC_CHUNK_KEYS)?;
    let mut k = keys
        .get(doc)?
        .map(|v| dec_key(v.value()))
        .transpose()?
        .ok_or(StoreError::Corrupt("a chunked doc without its chunk key"))?;
    k.next_segment = plan.next_segment + outs;
    keys.insert(doc, enc_key(&k).as_slice())?;
    Ok(true)
}

/// Remove every chunk row at no references that no write waits on (`waiting`), in one
/// transaction: rows retention kept for a write that then committed elsewhere or gave up.
/// Their bytes become dead ranges until a compaction. Returns the rows removed.
fn remove_unreferenced_rows(
    w: &WriteTransaction,
    waiting: &dyn Fn(DocId, &ChunkId) -> bool,
) -> Result<u64, StoreError> {
    if !history::has_table(w, DOC_CHUNKS.name())? {
        return Ok(0);
    }
    let mut t = w.open_table(DOC_CHUNKS)?;
    let mut gone: Vec<(DocId, ChunkId)> = Vec::new();
    for kv in t.iter()? {
        let (k, v) = kv?;
        let (doc, id) = k.value();
        let id = id32(id, "doc chunk")?;
        if dec_chunk(v.value())?.refs == 0 && !waiting(doc, &id) {
            gone.push((doc, id));
        }
    }
    for (doc, id) in &gone {
        t.remove((*doc, id.as_slice()))?;
    }
    Ok(gone.len() as u64)
}

/// What the store keeps of the blob store over it ([`DocStore::link_blob_store`]).
#[derive(Clone)]
pub(crate) struct BlobLink {
    /// The blob folder: where a chunked version's manifest is read when the store drops it.
    pub dir: PathBuf,
    /// The blob store's lock: retention holds it across its transaction, so no write marks a
    /// chunk between retention's look at the marks and its commit.
    pub lock: Arc<Mutex<()>>,
    /// Whether a write waits on chunk `id` of doc `doc` (read under `lock`).
    pub waiting: Waiting,
}

/// Whether a write waits on chunk `id` of doc `doc` ([`BlobLink::waiting`]).
pub(crate) type Waiting = Arc<dyn Fn(DocId, &ChunkId) -> bool + Send + Sync>;

/// Erase doc `doc`'s rows in the chunk tables, in `w` (its segment files are the blob store's to
/// remove), and, in space `no` (`None`: the caller removes the space's index rows itself), the
/// structure index rows of its manifests' bytes that no blob row and no other manifest holds. A
/// table the store doesn't have stays uncreated.
pub(super) fn erase_doc(
    w: &WriteTransaction,
    doc: DocId,
    no: Option<u64>,
) -> Result<(), StoreError> {
    let mut files = Vec::new();
    if history::has_table(w, DOC_MANIFESTS.name())? {
        let t = w.open_table(DOC_MANIFESTS)?;
        for kv in t.range((doc, [].as_slice())..(doc + 1, [].as_slice()))? {
            let (k, _) = kv?;
            files.push(BlobId::from_slice(k.value().1).ok_or(StoreError::Corrupt("blob id"))?);
        }
    }
    for t in CHUNK_TABLES {
        if history::has_table(w, t.name())? {
            t.erase(w, doc)?;
        }
    }
    if let Some(no) = no {
        for f in &files {
            drop_index_rows(w, no, f)?;
        }
    }
    Ok(())
}

/// Whether any doc holds a manifest for bytes `file`, in `w` (a scan of the manifests: there is
/// no table from a file id to its docs, which would be a cross-doc equality index).
fn manifest_anywhere(w: &WriteTransaction, file: &BlobId) -> Result<bool, StoreError> {
    if !history::has_table(w, DOC_MANIFESTS.name())? {
        return Ok(false);
    }
    for kv in w.open_table(DOC_MANIFESTS)?.iter()? {
        if kv?.0.value().1 == file.as_bytes().as_slice() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The structure index rows of bytes `file` in space `no` go, in `w`, unless a blob row or a
/// manifest of those bytes still holds them: an index row lives while its blob row does, and a
/// chunked version's manifest row stands in for the blob row.
fn drop_index_rows(w: &WriteTransaction, no: u64, file: &BlobId) -> Result<(), StoreError> {
    let whole = w
        .open_table(super::blob::BLOBS)?
        .get((no, file.as_bytes().as_slice()))?
        .is_some();
    if !whole && !manifest_anywhere(w, file)? {
        super::blob_struct::remove_rows(w, no, file)?;
    }
    Ok(())
}

/// The file ids of every manifest in the store, from `r` (none without the table): the bytes a
/// structure index row may be kept for besides blob rows.
pub(super) fn manifest_files(
    r: &redb::ReadTransaction,
) -> Result<std::collections::HashSet<Vec<u8>>, StoreError> {
    let mut out = std::collections::HashSet::new();
    if let Some(t) = open_opt(r, DOC_MANIFESTS)? {
        for kv in t.iter()? {
            out.insert(kv?.0.value().1.to_vec());
        }
    }
    Ok(out)
}

/// [`manifest_files`], in a write transaction.
pub(super) fn manifest_files_in(
    w: &WriteTransaction,
) -> Result<std::collections::HashSet<Vec<u8>>, StoreError> {
    let mut out = std::collections::HashSet::new();
    if history::has_table(w, DOC_MANIFESTS.name())? {
        for kv in w.open_table(DOC_MANIFESTS)?.iter()? {
            out.insert(kv?.0.value().1.to_vec());
        }
    }
    Ok(out)
}

/// The chunk tables and the removal list, as [`super::DOC_TABLES`] lists them.
pub(super) const CHUNK_TABLES: [super::DocRows; 5] = [
    super::DocRows::One(DOC_CHUNK_KEYS),
    super::DocRows::Bytes(DOC_CHUNKS),
    super::DocRows::Bytes(DOC_MANIFESTS),
    super::DocRows::Pair(DOC_SEGMENTS),
    super::DocRows::Pair(DOC_REMOVALS),
];

/// A per-doc table keyed by an id's bytes, open for reading.
type ByIdTable = redb::ReadOnlyTable<(u64, &'static [u8]), &'static [u8]>;

/// A read of the chunk tables, in one read transaction ([`DocStore::chunk_read`]). A store
/// without them reads as holding nothing.
pub(crate) struct ChunkRead {
    keys: Option<redb::ReadOnlyTable<u64, &'static [u8]>>,
    chunks: Option<ByIdTable>,
    manifests: Option<ByIdTable>,
    segments: Option<redb::ReadOnlyTable<(u64, u64), &'static [u8]>>,
}

fn open_opt<K: redb::Key + 'static, V: redb::Value + 'static>(
    r: &redb::ReadTransaction,
    t: TableDefinition<K, V>,
) -> Result<Option<redb::ReadOnlyTable<K, V>>, StoreError> {
    match r.open_table(t) {
        Ok(t) => Ok(Some(t)),
        Err(redb::TableError::TableDoesNotExist(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

impl ChunkRead {
    pub(crate) fn key(&self, doc: DocId) -> Result<Option<ChunkKey>, StoreError> {
        let Some(t) = &self.keys else { return Ok(None) };
        let v = t.get(doc)?;
        v.map(|v| dec_key(v.value())).transpose()
    }

    pub(crate) fn chunk(&self, doc: DocId, id: &ChunkId) -> Result<Option<ChunkRow>, StoreError> {
        let Some(t) = &self.chunks else {
            return Ok(None);
        };
        let v = t.get((doc, id.as_slice()))?;
        v.map(|v| dec_chunk(v.value())).transpose()
    }

    pub(crate) fn manifest(
        &self,
        doc: DocId,
        file: &BlobId,
    ) -> Result<Option<Manifest>, StoreError> {
        let Some(t) = &self.manifests else {
            return Ok(None);
        };
        let v = t.get((doc, file.as_bytes().as_slice()))?;
        v.map(|v| dec_manifest(v.value())).transpose()
    }

    pub(crate) fn has_segment(&self, doc: DocId, seg: u64) -> Result<bool, StoreError> {
        let Some(t) = &self.segments else {
            return Ok(false);
        };
        Ok(t.get((doc, seg))?.is_some())
    }
}

impl DocStore {
    /// Read the chunk tables in one read transaction.
    pub(crate) fn chunk_read<T, E: From<StoreError>>(
        &self,
        f: impl FnOnce(&ChunkRead) -> Result<T, E>,
    ) -> Result<T, E> {
        let r = self.begin_read()?;
        let cr = ChunkRead {
            keys: open_opt(&r, DOC_CHUNK_KEYS)?,
            chunks: open_opt(&r, DOC_CHUNKS)?,
            manifests: open_opt(&r, DOC_MANIFESTS)?,
            segments: open_opt(&r, DOC_SEGMENTS)?,
        };
        f(&cr)
    }

    /// The blob store over this store (set by [`crate::blob::BlobStore::open`]): its folder,
    /// where chunked versions are read from when the store drops one, its lock and its writes'
    /// waiting marks.
    pub(crate) fn link_blob_store(&self, link: BlobLink) {
        *self.blob_link.lock().unwrap_or_else(|p| p.into_inner()) = Some(link);
    }

    pub(super) fn blob_link(&self) -> Option<BlobLink> {
        self.blob_link
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Remove every chunk row at no references that no write waits on (`waiting`), in one
    /// transaction ([`crate::blob::BlobStore::gc`], under the blob store's lock). Returns the
    /// rows removed.
    pub(crate) fn chunk_remove_unreferenced(
        &self,
        waiting: &dyn Fn(DocId, &ChunkId) -> bool,
    ) -> Result<u64, StoreError> {
        let w = self.begin_write()?;
        let n = remove_unreferenced_rows(&w, waiting)?;
        w.commit()?;
        Ok(n)
    }

    /// The removal list's entries, as (doc, segment, space no): every doc's, or doc `doc`'s.
    pub(crate) fn chunk_removals(
        &self,
        doc: Option<DocId>,
    ) -> Result<Vec<(DocId, u64, u64)>, StoreError> {
        let r = self.begin_read()?;
        let Some(t) = open_opt(&r, DOC_REMOVALS)? else {
            return Ok(Vec::new());
        };
        let range = match doc {
            Some(d) => (d, 0)..=(d, u64::MAX),
            None => (0, 0)..=(u64::MAX, u64::MAX),
        };
        let mut out = Vec::new();
        for kv in t.range(range)? {
            let (k, v) = kv?;
            let (d, seg) = k.value();
            out.push((d, seg, dec_removal(v.value())?));
        }
        Ok(out)
    }

    /// Clear the removal list's entries `done` (their files are deleted), in one transaction.
    pub(crate) fn chunk_clear_removals(&self, done: &[(DocId, u64)]) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        {
            let mut t = w.open_table(DOC_REMOVALS)?;
            for k in done {
                t.remove(*k)?;
            }
        }
        w.commit()?;
        Ok(())
    }

    /// The compaction retention deferred for doc `doc` ([`Select::Sparse`]), planned in a
    /// transaction that is dropped (the caller holds the doc's lock).
    pub(crate) fn chunk_plan_sparse(&self, doc: DocId) -> Result<Option<Compaction>, StoreError> {
        let w = self.begin_write()?;
        plan_in(&w, doc, Select::Sparse, &[])
    }

    /// Commit `plan` ([`DocStore::chunk_plan_sparse`]) in space `no`, in one transaction:
    /// `false` when the doc changed since the plan (nothing committed).
    pub(crate) fn chunk_compact_sparse(
        &self,
        no: u64,
        plan: &Compaction,
    ) -> Result<bool, StoreError> {
        let w = self.begin_write()?;
        if !apply_in(&w, no, Some(plan), Select::Sparse, &[])? {
            return Ok(false);
        }
        w.commit()?;
        Ok(true)
    }

    /// Every doc with a segment row, with its space.
    pub(crate) fn chunked_docs(&self) -> Result<Vec<(DocId, String)>, StoreError> {
        let r = self.begin_read()?;
        let Some(t) = open_opt(&r, DOC_SEGMENTS)? else {
            return Ok(Vec::new());
        };
        let docs = r.open_table(super::DOCS)?;
        let mut out: Vec<(DocId, String)> = Vec::new();
        for kv in t.iter()? {
            let d = kv?.0.value().0;
            if out.last().is_some_and(|(x, _)| *x == d) {
                continue;
            }
            if let Some(m) = docs.get(d)? {
                out.push((d, super::dec_meta(m.value())?.key.space));
            }
        }
        Ok(out)
    }

    /// Test-only: remove doc `doc`'s chunk row for `id`, as a remover that ignores a write's
    /// waiting marks would.
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn remove_chunk_row_for_tests(&self, doc: DocId, id: &[u8; 32]) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        w.open_table(DOC_CHUNKS)?.remove((doc, id.as_slice()))?;
        w.commit()?;
        Ok(())
    }

    /// Test-only: every row of doc `doc` in the chunk tables, and which of the tables exist.
    #[cfg(any(test, feature = "differential"))]
    pub(crate) fn chunk_rows_for_tests(&self, doc: DocId) -> Result<ChunkRows, StoreError> {
        let r = self.begin_read()?;
        let mut out = ChunkRows::default();
        let names: std::collections::HashSet<String> =
            r.list_tables()?.map(|t| t.name().to_owned()).collect();
        for t in CHUNK_TABLES {
            if names.contains(t.name()) {
                out.tables.push(t.name().to_owned());
            }
        }
        if let Some(t) = open_opt(&r, DOC_MANIFESTS)? {
            for kv in t.range((doc, [].as_slice())..(doc + 1, [].as_slice()))? {
                let (k, v) = kv?;
                let id = BlobId::from_slice(k.value().1).ok_or(StoreError::Corrupt("blob id"))?;
                out.manifests.push((id, dec_manifest(v.value())?.refs));
            }
        }
        if let Some(t) = open_opt(&r, DOC_CHUNKS)? {
            for kv in t.range((doc, [].as_slice())..(doc + 1, [].as_slice()))? {
                let (k, v) = kv?;
                let c = dec_chunk(v.value())?;
                out.chunks.push((
                    id32(k.value().1, "doc chunk")?,
                    c.segment,
                    c.offset,
                    c.len,
                    c.refs,
                ));
            }
        }
        if let Some(t) = open_opt(&r, DOC_SEGMENTS)? {
            for kv in t.range((doc, 0)..=(doc, u64::MAX))? {
                let (k, v) = kv?;
                out.segments.push((k.value().1, dec_segment(v.value())?));
            }
        }
        Ok(out)
    }
}

/// Test-only: a doc's rows in the chunk tables ([`DocStore::chunk_rows_for_tests`]).
#[cfg(any(test, feature = "differential"))]
#[derive(Debug, Default)]
pub(crate) struct ChunkRows {
    pub tables: Vec<String>,
    pub manifests: Vec<(BlobId, u64)>,
    pub chunks: Vec<(ChunkId, u64, u64, u32, u64)>,
    pub segments: Vec<(u64, SegmentList)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_and_refuse_damage() {
        let k = ChunkKey {
            key: [7; 32],
            sizes: 1,
            next_segment: 9,
        };
        assert_eq!(dec_key(&enc_key(&k)).unwrap(), k);
        let c = ChunkRow {
            segment: 3,
            offset: 1 << 33,
            len: 65_536,
            refs: 2,
            since_ms: 5,
        };
        assert_eq!(dec_chunk(&enc_chunk(&c)).unwrap(), c);
        let m = Manifest {
            refs: 1,
            size: 3 << 20,
            sizes: 0,
            root: vec![([1; 32], 1_700), ([2; 32], 36)],
        };
        assert_eq!(dec_manifest(&enc_manifest(&m)).unwrap(), m);
        let s: SegmentList = vec![([3; 32], 0, 10), ([4; 32], 10, 20)];
        assert_eq!(dec_segment(&enc_segment(&s)).unwrap(), s);
        // a cut record, a trailing byte and another version are damage
        for b in [
            enc_manifest(&m)[..20].to_vec(),
            [enc_chunk(&c), vec![0]].concat(),
            [vec![2], enc_key(&k)[1..].to_vec()].concat(),
        ] {
            assert!(matches!(
                dec_manifest(&b)
                    .err()
                    .or(dec_chunk(&b).err())
                    .or(dec_key(&b).err()),
                Some(StoreError::Corrupt(_))
            ));
        }
        // a count no record could hold is refused before anything is allocated
        let mut huge = vec![1u8];
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(dec_segment(&huge), Err(StoreError::Corrupt(_))));
        let e = vec![([5; 32], 7u32), ([6; 32], 8)];
        assert_eq!(dec_entries(&enc_entries(&e)), Some(e));
        assert_eq!(dec_entries(&[0; 35]), None);
    }
}
