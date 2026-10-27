//! Persistence: one redb file holding every doc's snapshot, update log, attribution and
//! versions.
//!
//! - **Durable acks.** [`DocStore::append`] commits one write transaction (redb's default,
//!   immediate durability) before it returns, so a caller may ack once it returns. Batch
//!   the updates of one tick into one call.
//! - **Only kit-made bytes.** The log holds updates the kit encoded itself (agent edits
//!   today; re-encoded client updates once the strict decoder lands). Never raw client bytes.
//! - **Engine-neutral versions.** A version stores each root's plain content (text for text
//!   roots), not engine state, so versions survive an engine change.
//! - **Format stamps.** The store has a format number, and each doc records its pack,
//!   engine and engine-format, so a later release can re-encode on upgrade.
//!
//! The host chooses the file path and calls [`DocStore::open_with_retry`] across its own
//! process hand-offs (redb holds an exclusive lock while open).

mod enc;

use std::path::Path;
use std::time::{Duration, Instant};

use redb::{Database, ReadableTable, TableDefinition};

use crate::doc::{ClientId, Neutral};
use enc::{R, W};

/// Version of this store's layout. Bumped on incompatible changes; older stores are migrated.
///
/// - 1: first layout.
/// - 2: log records v2 (`on_behalf_of`), approvals, the host table. A format-1 store opens
///   as is (its records still decode) and is stamped 2; a build reading only format 1
///   refuses it with [`StoreError::Newer`] instead of misreading v2 log records.
pub const STORE_FORMAT: u64 = 2;

pub type DocId = u64;
/// Position in a doc's update log (1-based, increasing, never reused).
pub type Seq = u64;
pub type VersionId = u64;

const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
const DOCS: TableDefinition<u64, &[u8]> = TableDefinition::new("docs");
const PATHS: TableDefinition<&str, u64> = TableDefinition::new("paths");
const SNAPSHOTS: TableDefinition<u64, &[u8]> = TableDefinition::new("snapshots");
const UPDATES: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("updates");
const CLIENTS: TableDefinition<(u64, u64), &str> = TableDefinition::new("clients");
const VERSIONS: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("versions");
const APPROVALS: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("approvals");
const HOST: TableDefinition<(&str, &str), &[u8]> = TableDefinition::new("host");

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("store: {0}")]
    Db(String),
    #[error("store is format {found}; this build reads up to {STORE_FORMAT}")]
    Newer { found: u64 },
    #[error("store record is damaged: {0}")]
    Corrupt(&'static str),
    #[error("no doc {0}")]
    NoDoc(DocId),
    #[error("path already has a doc: {0:?}")]
    KeyTaken(DocKey),
    #[error("no version {1} for doc {0}")]
    NoVersion(DocId, VersionId),
    #[error("store still locked by another process after waiting")]
    Locked,
    #[error("version {1} of doc {0} is named; retention never deletes named versions")]
    NamedVersion(DocId, VersionId),
    #[error("put_meta can't change a doc's key; use rekey")]
    KeyChange,
    #[error("no approval for change {1} of doc {0}")]
    NoApproval(DocId, Seq),
}

macro_rules! db_err {
    ($($t:ty),*) => {$(impl From<$t> for StoreError { fn from(e: $t) -> Self { StoreError::Db(e.to_string()) } })*};
}
db_err!(
    redb::Error,
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError
);

/// Where a doc lives: an opaque space (e.g. a workspace id) and a normalised relative path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocKey {
    pub space: String,
    pub path: String,
}

impl DocKey {
    fn index(&self) -> String {
        format!("{}\u{0}{}", self.space, self.path)
    }
}

/// Per-doc metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocMeta {
    pub key: DocKey,
    /// The pack's schema name (e.g. "markdown").
    pub pack: String,
    /// Engine identity and its encoding version, e.g. "yrs-0.28" / 2 (v2 updates).
    pub engine: String,
    pub engine_format: u32,
    /// The pack codec's own settings for this file (e.g. BOM and line endings), opaque here.
    pub codec: Vec<u8>,
    pub created_ms: u64,
    /// Disk sync: the hash of the bytes last written to or read from the file, and the log
    /// position they correspond to.
    pub synced_hash: Option<Vec<u8>>,
    pub synced_seq: Seq,
    /// Why live editing is off for this doc, if it is.
    pub declined: Option<String>,
}

/// One logged change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// Participant reference (opaque to the kit; the host's stable id for who it was).
    pub author: String,
    pub client: ClientId,
    pub at_ms: u64,
    /// Surface it came through, e.g. "cli", "tab", "app", "disk", "restore".
    pub via: String,
    /// The engine update (kit-made).
    pub update: Vec<u8>,
    /// The person an assistant agent made this change for (`author` is the agent). An
    /// entry with this set gets a `Pending` [`Approval`] in the same transaction it is
    /// appended in. Record version 2; version-1 records decode with `None`.
    pub on_behalf_of: Option<String>,
}

/// Where an on-behalf change stands with the person it was made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApprovalState {
    Pending,
    Approved,
    Declined,
}

impl ApprovalState {
    fn from_u8(v: u8) -> Result<Self, StoreError> {
        Ok(match v {
            0 => Self::Pending,
            1 => Self::Approved,
            2 => Self::Declined,
            _ => return Err(StoreError::Corrupt("approval state")),
        })
    }
    fn to_u8(self) -> u8 {
        match self {
            Self::Pending => 0,
            Self::Approved => 1,
            Self::Declined => 2,
        }
    }
}

/// The approval record of one on-behalf change, keyed `(doc, seq)`. It survives compaction
/// (the log entry may not).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Approval {
    pub state: ApprovalState,
    /// Who settled it (always the `on_behalf_of` person); `None` while pending.
    pub by: Option<String>,
    /// When it was settled, on the host's clock; `None` while pending.
    pub at_ms: Option<u64>,
    /// The agent that made the change.
    pub author: String,
    /// The person it was made for: the only one who may settle it.
    pub on_behalf_of: String,
}

impl Approval {
    /// A new pending approval for a change by `author` on behalf of `on_behalf_of`.
    pub fn pending(author: impl Into<String>, on_behalf_of: impl Into<String>) -> Self {
        Self {
            state: ApprovalState::Pending,
            by: None,
            at_ms: None,
            author: author.into(),
            on_behalf_of: on_behalf_of.into(),
        }
    }

    /// This approval settled as `state` by `by` at `at_ms`.
    pub fn settled(&self, state: ApprovalState, by: impl Into<String>, at_ms: u64) -> Self {
        Self {
            state,
            by: Some(by.into()),
            at_ms: Some(at_ms),
            ..self.clone()
        }
    }
}

/// One write in [`DocStore::host_batch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOp<'a> {
    Put {
        ns: &'a str,
        key: &'a str,
        value: &'a [u8],
    },
    Delete {
        ns: &'a str,
        key: &'a str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionReason {
    Import = 1,
    Idle = 2,
    BeforeExternal = 3,
    External = 4,
    BeforeAgent = 5,
    BeforeRestore = 6,
    Named = 7,
}

impl VersionReason {
    fn from_u8(v: u8) -> Result<Self, StoreError> {
        Ok(match v {
            1 => Self::Import,
            2 => Self::Idle,
            3 => Self::BeforeExternal,
            4 => Self::External,
            5 => Self::BeforeAgent,
            6 => Self::BeforeRestore,
            7 => Self::Named,
            _ => return Err(StoreError::Corrupt("version reason")),
        })
    }
}

/// A whole-doc checkpoint, engine-neutral.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// Log position this version reflects.
    pub seq: Seq,
    pub at_ms: u64,
    pub reason: VersionReason,
    pub name: Option<String>,
    /// Participants whose changes are in this version since the previous one.
    pub authors: Vec<String>,
    pub content: Vec<(String, Neutral)>,
}

/// What a session needs to rebuild a doc: the latest snapshot and the log after it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Loaded {
    pub snapshot: Option<(Seq, Vec<u8>)>,
    pub updates: Vec<(Seq, LogEntry)>,
}

pub struct DocStore {
    db: Database,
}

fn enc_meta(m: &DocMeta) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1)
        .str(&m.key.space)
        .str(&m.key.path)
        .str(&m.pack)
        .str(&m.engine)
        .u32(m.engine_format)
        .bytes(&m.codec)
        .u64(m.created_ms)
        .opt_bytes(m.synced_hash.as_deref())
        .u64(m.synced_seq)
        .opt_str(m.declined.as_deref());
    w.0
}

fn dec_meta(b: &[u8]) -> Result<DocMeta, StoreError> {
    let mut r = R::new(b, "doc meta");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("doc meta version"));
    }
    let m = DocMeta {
        key: DocKey {
            space: r.str()?,
            path: r.str()?,
        },
        pack: r.str()?,
        engine: r.str()?,
        engine_format: r.u32()?,
        codec: r.bytes()?,
        created_ms: r.u64()?,
        synced_hash: r.opt_bytes()?,
        synced_seq: r.u64()?,
        declined: r.opt_str()?,
    };
    r.end()?;
    Ok(m)
}

/// Log record version 2: version 1 plus `on_behalf_of` at the end.
fn enc_entry(e: &LogEntry) -> Vec<u8> {
    let mut w = W::default();
    w.u8(2)
        .str(&e.author)
        .u64(e.client)
        .u64(e.at_ms)
        .str(&e.via)
        .bytes(&e.update)
        .opt_str(e.on_behalf_of.as_deref());
    w.0
}

/// Decodes log records version 1 and 2.
fn dec_entry(b: &[u8]) -> Result<LogEntry, StoreError> {
    let mut r = R::new(b, "log entry");
    let v = r.u8()?;
    if v != 1 && v != 2 {
        return Err(StoreError::Corrupt("log entry version"));
    }
    let e = LogEntry {
        author: r.str()?,
        client: r.u64()?,
        at_ms: r.u64()?,
        via: r.str()?,
        update: r.bytes()?,
        on_behalf_of: if v >= 2 { r.opt_str()? } else { None },
    };
    r.end()?;
    Ok(e)
}

fn enc_approval(a: &Approval) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1)
        .u8(a.state.to_u8())
        .opt_str(a.by.as_deref())
        .u8(u8::from(a.at_ms.is_some()))
        .u64(a.at_ms.unwrap_or(0))
        .str(&a.author)
        .str(&a.on_behalf_of);
    w.0
}

fn dec_approval(b: &[u8]) -> Result<Approval, StoreError> {
    let mut r = R::new(b, "approval");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("approval version"));
    }
    let state = ApprovalState::from_u8(r.u8()?)?;
    let by = r.opt_str()?;
    let has_at = r.u8()?;
    let at = r.u64()?;
    let a = Approval {
        state,
        by,
        at_ms: match has_at {
            0 => None,
            1 => Some(at),
            _ => return Err(StoreError::Corrupt("approval")),
        },
        author: r.str()?,
        on_behalf_of: r.str()?,
    };
    r.end()?;
    Ok(a)
}

/// Insert one log entry, and its pending approval when it was made on someone's behalf.
fn put_entry(
    w: &redb::WriteTransaction,
    id: DocId,
    seq: Seq,
    e: &LogEntry,
) -> Result<(), StoreError> {
    w.open_table(UPDATES)?
        .insert((id, seq), enc_entry(e).as_slice())?;
    if let Some(p) = &e.on_behalf_of {
        w.open_table(APPROVALS)?.insert(
            (id, seq),
            enc_approval(&Approval::pending(e.author.clone(), p.clone())).as_slice(),
        )?;
    }
    Ok(())
}

fn enc_version(v: &Version) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1)
        .u64(v.seq)
        .u64(v.at_ms)
        .u8(v.reason as u8)
        .opt_str(v.name.as_deref())
        .strs(&v.authors);
    w.u32(v.content.len() as u32);
    for (root, n) in &v.content {
        w.str(root);
        match n {
            Neutral::Text(t) => w.u8(1).str(t),
        };
    }
    w.0
}

fn dec_version(b: &[u8]) -> Result<Version, StoreError> {
    let mut r = R::new(b, "version");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("version record version"));
    }
    let (seq, at_ms, reason, name, authors) = (
        r.u64()?,
        r.u64()?,
        VersionReason::from_u8(r.u8()?)?,
        r.opt_str()?,
        r.strs()?,
    );
    let n = r.u32()? as usize;
    if n > b.len() {
        return Err(StoreError::Corrupt("version roots"));
    }
    let mut content = Vec::with_capacity(n);
    for _ in 0..n {
        let root = r.str()?;
        let val = match r.u8()? {
            1 => Neutral::Text(r.str()?),
            _ => return Err(StoreError::Corrupt("version value kind")),
        };
        content.push((root, val));
    }
    r.end()?;
    Ok(Version {
        seq,
        at_ms,
        reason,
        name,
        authors,
        content,
    })
}

fn enc_snapshot(seq: Seq, state: &[u8]) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1).u64(seq).bytes(state);
    w.0
}

fn dec_snapshot(b: &[u8]) -> Result<(Seq, Vec<u8>), StoreError> {
    let mut r = R::new(b, "snapshot");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("snapshot version"));
    }
    let v = (r.u64()?, r.bytes()?);
    r.end()?;
    Ok(v)
}

impl DocStore {
    /// Open (or create) the store at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let db = Database::create(path)?;
        let w = db.begin_write()?;
        {
            let mut meta = w.open_table(META)?;
            let found = meta.get("store_format")?.map(|v| v.value());
            match found {
                None => {
                    meta.insert("store_format", STORE_FORMAT)?;
                }
                Some(f) if f > STORE_FORMAT => return Err(StoreError::Newer { found: f }),
                // 1 -> 2: nothing to rewrite (v1 records still decode); new tables below
                Some(f) if f < STORE_FORMAT => {
                    meta.insert("store_format", STORE_FORMAT)?;
                }
                Some(_) => {}
            }
            w.open_table(DOCS)?;
            w.open_table(PATHS)?;
            w.open_table(SNAPSHOTS)?;
            w.open_table(UPDATES)?;
            w.open_table(CLIENTS)?;
            w.open_table(VERSIONS)?;
            w.open_table(APPROVALS)?;
            w.open_table(HOST)?;
        }
        w.commit()?;
        Ok(Self { db })
    }

    /// Open, retrying while another process (a daemon handing off) still holds the lock.
    pub fn open_with_retry(path: impl AsRef<Path>, wait: Duration) -> Result<Self, StoreError> {
        let deadline = Instant::now() + wait;
        loop {
            match Database::create(path.as_ref()) {
                Ok(db) => {
                    drop(db);
                    return Self::open(path);
                }
                Err(redb::DatabaseError::DatabaseAlreadyOpen) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(redb::DatabaseError::DatabaseAlreadyOpen) => return Err(StoreError::Locked),
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Find the doc at `key`.
    pub fn find(&self, key: &DocKey) -> Result<Option<DocId>, StoreError> {
        let r = self.db.begin_read()?;
        Ok(r.open_table(PATHS)?
            .get(key.index().as_str())?
            .map(|v| v.value()))
    }

    /// Create a doc. Fails if its key already has one.
    pub fn create(&self, meta: &DocMeta) -> Result<DocId, StoreError> {
        let w = self.db.begin_write()?;
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
            w.open_table(DOCS)?.insert(id, enc_meta(meta).as_slice())?;
            id
        };
        w.commit()?;
        Ok(id)
    }

    /// Create a doc with its first content, in one transaction: the metadata, the owner of
    /// the importing client, the import update (none for an empty file) and the import
    /// version. A crash leaves either all of it or none of it, so a doc never exists
    /// without the content it was created from.
    ///
    /// `version.seq` is set here to the import update's position (0 when there is none).
    /// Returns the doc, the update's position and the version id.
    pub fn create_imported(
        &self,
        meta: &DocMeta,
        client: ClientId,
        owner: &str,
        entry: Option<&LogEntry>,
        version: &Version,
    ) -> Result<(DocId, Option<Seq>, VersionId), StoreError> {
        let w = self.db.begin_write()?;
        let out = {
            let mut paths = w.open_table(PATHS)?;
            let k = meta.key.index();
            if paths.get(k.as_str())?.is_some() {
                return Err(StoreError::KeyTaken(meta.key.clone()));
            }
            let mut m = w.open_table(META)?;
            let id = m.get("next_doc_id")?.map(|v| v.value()).unwrap_or(1);
            m.insert("next_doc_id", id + 1)?;
            paths.insert(k.as_str(), id)?;
            w.open_table(DOCS)?.insert(id, enc_meta(meta).as_slice())?;
            w.open_table(CLIENTS)?.insert((id, client), owner)?;
            let seq = match entry {
                Some(e) => {
                    put_entry(&w, id, 1, e)?;
                    Some(1)
                }
                None => None,
            };
            let mut v = version.clone();
            v.seq = seq.unwrap_or(0);
            w.open_table(VERSIONS)?
                .insert((id, 1), enc_version(&v).as_slice())?;
            (id, seq, 1)
        };
        w.commit()?;
        Ok(out)
    }

    pub fn meta(&self, id: DocId) -> Result<DocMeta, StoreError> {
        let r = self.db.begin_read()?;
        let t = r.open_table(DOCS)?;
        let v = t.get(id)?.ok_or(StoreError::NoDoc(id))?;
        dec_meta(v.value())
    }

    /// Replace a doc's metadata. The key can't change here; use [`DocStore::rekey`].
    pub fn put_meta(&self, id: DocId, meta: &DocMeta) -> Result<(), StoreError> {
        let w = self.db.begin_write()?;
        {
            let mut t = w.open_table(DOCS)?;
            let old = dec_meta(t.get(id)?.ok_or(StoreError::NoDoc(id))?.value())?;
            if old.key != meta.key {
                return Err(StoreError::KeyChange);
            }
            t.insert(id, enc_meta(meta).as_slice())?;
        }
        w.commit()?;
        Ok(())
    }

    /// Move a doc to a new key (a rename). Fails if the new key already has a doc.
    pub fn rekey(&self, id: DocId, new: DocKey) -> Result<(), StoreError> {
        let w = self.db.begin_write()?;
        {
            let mut docs = w.open_table(DOCS)?;
            let mut meta = dec_meta(docs.get(id)?.ok_or(StoreError::NoDoc(id))?.value())?;
            let mut paths = w.open_table(PATHS)?;
            if paths.get(new.index().as_str())?.is_some() {
                return Err(StoreError::KeyTaken(new));
            }
            paths.remove(meta.key.index().as_str())?;
            paths.insert(new.index().as_str(), id)?;
            meta.key = new;
            docs.insert(id, enc_meta(&meta).as_slice())?;
        }
        w.commit()?;
        Ok(())
    }

    /// Append entries to a doc's log in one durable transaction; returns their positions.
    /// An entry with `on_behalf_of` gets a `Pending` [`Approval`] in the same transaction.
    pub fn append(&self, id: DocId, entries: &[LogEntry]) -> Result<Vec<Seq>, StoreError> {
        let w = self.db.begin_write()?;
        let seqs = self.append_in(&w, id, entries)?;
        w.commit()?;
        Ok(seqs)
    }

    fn append_in(
        &self,
        w: &redb::WriteTransaction,
        id: DocId,
        entries: &[LogEntry],
    ) -> Result<Vec<Seq>, StoreError> {
        if w.open_table(DOCS)?.get(id)?.is_none() {
            return Err(StoreError::NoDoc(id));
        }
        let mut next = {
            let t = w.open_table(UPDATES)?;
            self.last_seq_in(&t, id)? + 1
        };
        // after a compaction the log may be empty; never reuse a snapshot's position
        if let Some(v) = w.open_table(SNAPSHOTS)?.get(id)? {
            next = next.max(dec_snapshot(v.value())?.0 + 1);
        }
        let mut seqs = Vec::with_capacity(entries.len());
        for e in entries {
            put_entry(w, id, next, e)?;
            seqs.push(next);
            next += 1;
        }
        Ok(seqs)
    }

    fn last_seq_in(
        &self,
        t: &impl ReadableTable<(u64, u64), &'static [u8]>,
        id: DocId,
    ) -> Result<Seq, StoreError> {
        Ok(t.range((id, 0)..=(id, u64::MAX))?
            .next_back()
            .transpose()?
            .map(|(k, _)| k.value().1)
            .unwrap_or(0))
    }

    /// The latest snapshot and every logged update after it, in order.
    pub fn load(&self, id: DocId) -> Result<Loaded, StoreError> {
        let r = self.db.begin_read()?;
        if r.open_table(DOCS)?.get(id)?.is_none() {
            return Err(StoreError::NoDoc(id));
        }
        let snapshot = r
            .open_table(SNAPSHOTS)?
            .get(id)?
            .map(|v| dec_snapshot(v.value()))
            .transpose()?;
        let from = snapshot.as_ref().map(|(s, _)| s + 1).unwrap_or(1);
        let mut updates = Vec::new();
        for kv in r.open_table(UPDATES)?.range((id, from)..=(id, u64::MAX))? {
            let (k, v) = kv?;
            updates.push((k.value().1, dec_entry(v.value())?));
        }
        Ok(Loaded { snapshot, updates })
    }

    /// Log size since the last snapshot: (entries, bytes). Drives compaction.
    pub fn log_size(&self, id: DocId) -> Result<(u64, u64), StoreError> {
        let l = self.load(id)?;
        Ok((
            l.updates.len() as u64,
            l.updates.iter().map(|(_, e)| e.update.len() as u64).sum(),
        ))
    }

    /// Write `state` as the snapshot at position `upto` and drop log entries up to it, in one
    /// transaction. `state` must contain every update up to `upto`.
    pub fn compact(&self, id: DocId, upto: Seq, state: &[u8]) -> Result<(), StoreError> {
        let w = self.db.begin_write()?;
        {
            if w.open_table(DOCS)?.get(id)?.is_none() {
                return Err(StoreError::NoDoc(id));
            }
            w.open_table(SNAPSHOTS)?
                .insert(id, enc_snapshot(upto, state).as_slice())?;
            let mut t = w.open_table(UPDATES)?;
            let old: Vec<u64> = t
                .range((id, 0)..=(id, upto))?
                .map(|kv| kv.map(|(k, _)| k.value().1))
                .collect::<Result<_, _>>()?;
            for s in old {
                t.remove((id, s))?;
            }
        }
        w.commit()?;
        Ok(())
    }

    /// Record which participant owns a CRDT client ID in this doc (attribution, blame).
    pub fn set_client(&self, id: DocId, client: ClientId, owner: &str) -> Result<(), StoreError> {
        let w = self.db.begin_write()?;
        w.open_table(CLIENTS)?.insert((id, client), owner)?;
        w.commit()?;
        Ok(())
    }

    pub fn client_owner(&self, id: DocId, client: ClientId) -> Result<Option<String>, StoreError> {
        let r = self.db.begin_read()?;
        Ok(r.open_table(CLIENTS)?
            .get((id, client))?
            .map(|v| v.value().to_owned()))
    }

    /// Store a version; returns its id (increasing per doc).
    pub fn put_version(&self, id: DocId, v: &Version) -> Result<VersionId, StoreError> {
        let w = self.db.begin_write()?;
        let vid = {
            if w.open_table(DOCS)?.get(id)?.is_none() {
                return Err(StoreError::NoDoc(id));
            }
            let mut t = w.open_table(VERSIONS)?;
            let last = t
                .range((id, 0)..=(id, u64::MAX))?
                .next_back()
                .transpose()?
                .map(|(k, _)| k.value().1)
                .unwrap_or(0);
            t.insert((id, last + 1), enc_version(v).as_slice())?;
            last + 1
        };
        w.commit()?;
        Ok(vid)
    }

    /// All versions of a doc, oldest first.
    pub fn versions(&self, id: DocId) -> Result<Vec<(VersionId, Version)>, StoreError> {
        let r = self.db.begin_read()?;
        let mut out = Vec::new();
        for kv in r.open_table(VERSIONS)?.range((id, 0)..=(id, u64::MAX))? {
            let (k, v) = kv?;
            out.push((k.value().1, dec_version(v.value())?));
        }
        Ok(out)
    }

    pub fn version(&self, id: DocId, vid: VersionId) -> Result<Version, StoreError> {
        let r = self.db.begin_read()?;
        let t = r.open_table(VERSIONS)?;
        let v = t.get((id, vid))?.ok_or(StoreError::NoVersion(id, vid))?;
        dec_version(v.value())
    }

    /// Name (or rename) a version.
    pub fn name_version(&self, id: DocId, vid: VersionId, name: &str) -> Result<(), StoreError> {
        let w = self.db.begin_write()?;
        {
            let mut t = w.open_table(VERSIONS)?;
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

    /// Delete a version (retention thinning). Named versions are refused: retention never drops them.
    pub fn delete_version(&self, id: DocId, vid: VersionId) -> Result<(), StoreError> {
        let w = self.db.begin_write()?;
        {
            let mut t = w.open_table(VERSIONS)?;
            let v = dec_version(
                t.get((id, vid))?
                    .ok_or(StoreError::NoVersion(id, vid))?
                    .value(),
            )?;
            if v.name.is_some() {
                return Err(StoreError::NamedVersion(id, vid));
            }
            t.remove((id, vid))?;
        }
        w.commit()?;
        Ok(())
    }
}

impl DocStore {
    // ----- approvals (on-behalf changes) -----

    /// The approval record of change `seq`, if it was made on someone's behalf.
    pub fn approval(&self, id: DocId, seq: Seq) -> Result<Option<Approval>, StoreError> {
        let r = self.db.begin_read()?;
        let t = r.open_table(APPROVALS)?;
        let v = t.get((id, seq))?;
        v.map(|v| dec_approval(v.value())).transpose()
    }

    /// Every approval record of a doc, oldest change first.
    pub fn approvals(&self, id: DocId) -> Result<Vec<(Seq, Approval)>, StoreError> {
        let r = self.db.begin_read()?;
        let mut out = Vec::new();
        for kv in r.open_table(APPROVALS)?.range((id, 0)..=(id, u64::MAX))? {
            let (k, v) = kv?;
            out.push((k.value().1, dec_approval(v.value())?));
        }
        Ok(out)
    }

    /// Replace an existing approval record. The store doesn't check who may do it: sessions
    /// do ([`crate::session::Session::set_approval`]).
    pub fn set_approval(&self, id: DocId, seq: Seq, a: &Approval) -> Result<(), StoreError> {
        let w = self.db.begin_write()?;
        {
            let mut t = w.open_table(APPROVALS)?;
            if t.get((id, seq))?.is_none() {
                return Err(StoreError::NoApproval(id, seq));
            }
            t.insert((id, seq), enc_approval(a).as_slice())?;
        }
        w.commit()?;
        Ok(())
    }

    // ----- the host's own table -----
    //
    // A small key-value table for the host's own records next to the docs (so they share
    // the store's transactions, hand-off and backups). Namespaces and keys are opaque: the
    // kit never reads, interprets or erases these rows. Erasing them (a person's data, a
    // deleted workspace) is the host's job.

    /// Set `ns`/`key` to `value`, in one durable transaction.
    pub fn host_put(&self, ns: &str, key: &str, value: &[u8]) -> Result<(), StoreError> {
        self.host_batch(&[HostOp::Put { ns, key, value }])
    }

    /// The value at `ns`/`key`.
    pub fn host_get(&self, ns: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let r = self.db.begin_read()?;
        let t = r.open_table(HOST)?;
        let v = t.get((ns, key))?;
        Ok(v.map(|v| v.value().to_vec()))
    }

    /// Delete `ns`/`key`; returns whether it existed. One durable transaction.
    pub fn host_delete(&self, ns: &str, key: &str) -> Result<bool, StoreError> {
        let w = self.db.begin_write()?;
        let existed = w.open_table(HOST)?.remove((ns, key))?.is_some();
        w.commit()?;
        Ok(existed)
    }

    /// Every `(key, value)` in `ns` whose key starts with `prefix` (`""` for all), in key
    /// order, from one consistent read.
    pub fn host_scan(&self, ns: &str, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        let r = self.db.begin_read()?;
        let t = r.open_table(HOST)?;
        let mut out = Vec::new();
        for kv in t.range((ns, prefix)..)? {
            let (k, v) = kv?;
            let (kns, kkey) = k.value();
            if kns != ns || !kkey.starts_with(prefix) {
                break;
            }
            out.push((kkey.to_owned(), v.value().to_vec()));
        }
        Ok(out)
    }

    /// Apply several puts and deletes in **one** durable transaction: all or none.
    pub fn host_batch(&self, ops: &[HostOp<'_>]) -> Result<(), StoreError> {
        let w = self.db.begin_write()?;
        {
            let mut t = w.open_table(HOST)?;
            for op in ops {
                match *op {
                    HostOp::Put { ns, key, value } => {
                        t.insert((ns, key), value)?;
                    }
                    HostOp::Delete { ns, key } => {
                        t.remove((ns, key))?;
                    }
                }
            }
        }
        w.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
