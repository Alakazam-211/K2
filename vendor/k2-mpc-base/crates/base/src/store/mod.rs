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
//! - **Erasure.** Records can hold redactions instead of content (format 3); a purge redacts
//!   a doc in one transaction (`DocStore::purge_doc`, for sessions), then
//!   [`DocStore::rewrite_file`] leaves no byte of it in the file, which
//!   [`DocStore::scan_for`] checks. No plain content hash is stored: sync hashes are keyed
//!   per doc, blob ids per space.
//!
//! The host chooses the file path and calls [`DocStore::open_with_retry`] across its own
//! process hand-offs (redb holds an exclusive lock while open).

mod blob;
mod delete;
mod enc;
mod history;
mod promote;
mod purge;
mod rewrite;
mod scan;

pub use blob::{BlobReason, BlobVersion, BLOB_DOC_PACK};
pub(crate) use blob::{BlobSync, SpaceKey};
pub use history::Redaction;
pub(crate) use history::{
    window_plan, Carry, ClockSpan, HunkRoots, NewChange, ProvRow, RecordHunk, RecordRoot,
    Retention, RevertRecord, RevertWindow, RootRuns, RootShape, SpanOf,
};
pub use history::{ChangeInfo, ChangeKind, RootHunks, VersionInfo};
pub(crate) use promote::PromoteRows;
#[cfg(test)]
pub(crate) use promote::FAIL_BEFORE_COMMIT as FAIL_PROMOTE_BEFORE_COMMIT;
pub use purge::PurgeCounts;
pub(crate) use purge::{DocPurge, Pick};
pub use rewrite::RewriteReport;
pub(crate) use scan::Needles;
pub use scan::{scan_bytes, scan_file, Finding, MIN_MATCH};

use std::path::{Path, PathBuf};
use std::sync::{RwLock, RwLockReadGuard};
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
/// - 3: erasure. A per-doc hash key (`doc_keys`) and keyed sync hashes (HMAC-SHA256, 33
///   bytes: a tag byte, then the MAC); redacted records: log entries v3 (`redacted {byte
///   length}` in place of the update), versions v2 (purged spans cut out, their ranges
///   kept), blob versions v2 (a purged version keeps its entry, not its blob). A format-2
///   store opens as is and is stamped 3: its records still decode, and a doc's plain
///   SHA-256 sync hash is read once, when its file is next read, and replaced by a keyed one
///   (a purge replaces it too). A format-2 build refuses a format-3 store
///   ([`StoreError::Newer`]).
///
///   v0.3.0 adds one table, `change_clocks` (blame's index from engine clocks to changes),
///   **without a format bump** (K-FMT-10, N−1 reads): a v0.2.0 build reads and writes such a
///   store as before (its changes get no clock rows; blame shows their text by client
///   owner, without a change id), and only its `rewrite_file` refuses the table it doesn't
///   know (an error, never a silent drop).
/// - 4: approvals of on-behalf blob versions (K-T0-9), in a new table `blob_approvals` keyed
///   `(doc, version)`: the same record as a text change's approval, kept apart from
///   `approvals` (keyed `(doc, seq)`) so a blob version and a text change can never be taken
///   for each other. A format-3 store opens as is and is stamped 4 **in place**, in the
///   open's own transaction: the table is created, and every on-behalf blob version it holds
///   gets the `Pending` approval it would have had. **No downgrade:** a format-3 build
///   refuses a format-4 store ([`StoreError::Newer`]) instead of writing on-behalf blob
///   versions without approvals, leaving their rows behind on a delete, or refusing the
///   table in `rewrite_file` mid-purge.
/// - 5: provenance of restored text (undo spec v1.8, U23). A new table, `provenance`, keyed
///   `(doc, client, first clock)`: for each run of elements an undo, redo or revert put
///   back, the root it restores (element IDs only, no content), written in the change's own
///   transaction; and revert records version 2, with the roots of each hunk's inserted and
///   deleted elements (version-1 records still decode, with unknown roots, and are reverted
///   as before). A format-4 store opens as is and is stamped 5 **in place**, in the open's
///   own transaction: the table is created, and nothing is backfilled (every element made
///   before has no provenance row, so it is its own root, which is what it is). **No
///   downgrade:** a format-4 build refuses a format-5 store ([`StoreError::Newer`]) instead
///   of writing changes without their provenance rows (a later build would then read text
///   restored by them as fresh), reading version-2 records as damaged, or refusing the
///   table in `rewrite_file` mid-purge.
///
///   v0.9.7 adds one table, `authored` (the hunks the author wrote, for a change D1
///   widened; `store/history.rs`), **without a format bump**: the revert record keeps its
///   version, and a store without the table (or without a change's row) reads as "authored
///   = the record's hunks", exact for every change made before. A v0.9.5 or v0.9.6 build
///   reads and writes such a store as before (its changes get no rows), but leaves the rows
///   behind where v0.9.7 drops them (a doc delete, records leaving the window), and its
///   `rewrite_file`, and so a purge, refuses the table before changing anything
///   ([`StoreError::NewerData`], `MPC-E502`; never a silent drop).
pub const STORE_FORMAT: u64 = 5;

pub type DocId = u64;
/// Position in a doc's update log (1-based, increasing, never reused).
pub type Seq = u64;
pub type VersionId = u64;

const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
/// `meta` key: an open's sweep removed rows and the file is still to be rewritten (v0.9.7).
const REWRITE_PENDING: &str = "rewrite_pending";
const DOCS: TableDefinition<u64, &[u8]> = TableDefinition::new("docs");
const PATHS: TableDefinition<&str, u64> = TableDefinition::new("paths");
const SNAPSHOTS: TableDefinition<u64, &[u8]> = TableDefinition::new("snapshots");
const UPDATES: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("updates");
const CLIENTS: TableDefinition<(u64, u64), &str> = TableDefinition::new("clients");
const VERSIONS: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("versions");
const APPROVALS: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("approvals");
const HOST: TableDefinition<(&str, &str), &[u8]> = TableDefinition::new("host");
/// Disk sync: `(doc, 0)` is the synced state, `(doc, 1)` a write-back in flight.
const SYNC: TableDefinition<(u64, u8), &[u8]> = TableDefinition::new("sync");
/// Doc → its hash key: the key of its keyed sync hashes (format 3). A purge replaces it.
const DOC_KEYS: TableDefinition<u64, &[u8]> = TableDefinition::new("doc_keys");
const SYNCED: u8 = 0;
const PENDING: u8 = 1;

/// How a per-doc table is keyed: the doc id first, so a doc's rows are one key range.
#[derive(Clone, Copy)]
pub(super) enum DocRows {
    /// `doc` → record.
    One(TableDefinition<'static, u64, &'static [u8]>),
    /// `(doc, n)` → record.
    Pair(TableDefinition<'static, (u64, u64), &'static [u8]>),
    /// `(doc, client)` → owner (`clients`).
    Owner(TableDefinition<'static, (u64, u64), &'static str>),
    /// `(doc, slot)` → record (the sync tables).
    Slot(TableDefinition<'static, (u64, u8), &'static [u8]>),
    /// `(doc, client, clock)` → record.
    Clock(TableDefinition<'static, (u64, u64, u64), &'static [u8]>),
}

/// How a doc delete erases a per-doc table's rows of the doc.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Erase {
    /// The doc's key range, as is.
    Range,
    /// By `blob::delete_rows`, which first releases each blob version's reference on its
    /// blob (tier 0).
    Blob,
}

/// Every table keyed by doc id, with how a delete erases it (v0.9.7): the one list a doc
/// delete erases, a store rewrite copies and the delete tests check, so a per-doc table
/// added later is in all three or in none (and a rewrite refuses a table in no list).
/// Tables not keyed by doc are [`SHARED_TABLES`].
pub(super) const DOC_TABLES: &[(DocRows, Erase)] = &[
    (DocRows::One(DOCS), Erase::Range),
    (DocRows::One(SNAPSHOTS), Erase::Range),
    (DocRows::One(DOC_KEYS), Erase::Range),
    (DocRows::Pair(UPDATES), Erase::Range),
    (DocRows::Owner(CLIENTS), Erase::Range),
    (DocRows::Pair(VERSIONS), Erase::Range),
    (DocRows::Pair(APPROVALS), Erase::Range),
    (DocRows::Slot(SYNC), Erase::Range),
    (DocRows::Pair(history::CHANGES), Erase::Range),
    (DocRows::Pair(history::REVERTS), Erase::Range),
    (DocRows::Pair(history::REVERT_INDEX), Erase::Range),
    (DocRows::Pair(history::REVERT_CARRY), Erase::Range),
    (DocRows::One(history::REVERT_STATS), Erase::Range),
    (DocRows::Pair(history::AUTHORED), Erase::Range),
    (DocRows::Clock(history::CHANGE_CLOCKS), Erase::Range),
    (DocRows::Clock(history::PROVENANCE), Erase::Range),
    (DocRows::Pair(blob::BLOB_VERSIONS), Erase::Blob),
    (DocRows::Slot(blob::BLOB_SYNC), Erase::Blob),
    (DocRows::Pair(blob::BLOB_APPROVALS), Erase::Blob),
];

/// The tables not keyed by doc id: `meta` (the store's own), `paths` (key → doc: a delete
/// removes the doc's entry by its key), `host` (opaque to the kit), and the blob spaces and
/// blobs (per space). A rewrite copies them; a doc delete leaves them (but `paths`) alone.
pub(super) const SHARED_TABLES: &[&str] = &["meta", "paths", "host", "blob_spaces", "blobs"];

impl DocRows {
    pub(super) fn name(&self) -> &str {
        use redb::TableHandle;
        match self {
            Self::One(t) => t.name(),
            Self::Pair(t) => t.name(),
            Self::Owner(t) => t.name(),
            Self::Slot(t) => t.name(),
            Self::Clock(t) => t.name(),
        }
    }

    /// Erase doc `id`'s rows in `w`. A table the store doesn't have (a lazily created one,
    /// [`history::AUTHORED`]) has none, and stays absent.
    pub(super) fn erase(self, w: &redb::WriteTransaction, id: DocId) -> Result<(), StoreError> {
        if !history::has_table(w, self.name())? {
            return Ok(());
        }
        match self {
            Self::One(t) => {
                w.open_table(t)?.remove(id)?;
            }
            Self::Pair(t) => w
                .open_table(t)?
                .retain_in((id, 0)..=(id, u64::MAX), |_, _| false)?,
            Self::Owner(t) => w
                .open_table(t)?
                .retain_in((id, 0)..=(id, u64::MAX), |_, _| false)?,
            Self::Slot(t) => w
                .open_table(t)?
                .retain_in((id, 0)..=(id, u8::MAX), |_, _| false)?,
            Self::Clock(t) => w
                .open_table(t)?
                .retain_in((id, 0, 0)..=(id, u64::MAX, u64::MAX), |_, _| false)?,
        }
        Ok(())
    }

    /// Whether the table holds a row of doc `id` (the delete tests).
    #[cfg(test)]
    pub(super) fn holds(self, r: &redb::ReadTransaction, id: DocId) -> Result<bool, StoreError> {
        Ok(match self {
            Self::One(t) => r.open_table(t)?.get(id)?.is_some(),
            Self::Pair(t) => r
                .open_table(t)?
                .range((id, 0)..=(id, u64::MAX))?
                .next()
                .is_some(),
            Self::Owner(t) => r
                .open_table(t)?
                .range((id, 0)..=(id, u64::MAX))?
                .next()
                .is_some(),
            Self::Slot(t) => r
                .open_table(t)?
                .range((id, 0)..=(id, u8::MAX))?
                .next()
                .is_some(),
            Self::Clock(t) => r
                .open_table(t)?
                .range((id, 0, 0)..=(id, u64::MAX, u64::MAX))?
                .next()
                .is_some(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    #[error("store: {0}")]
    Db(String),
    #[error("store is format {found}; this build reads up to {STORE_FORMAT}")]
    Newer { found: u64 },
    /// The store holds a table this kit doesn't know: data a newer kit added without a
    /// format bump. Reading and editing still work; a rewrite (and so a
    /// purge) is refused before anything changes (v0.9.5). In the error registry this is
    /// `MPC-E502` `store_newer`, with details `{table}`; its message is the guidance. Exit 6.
    #[error("{}", newer_data_guidance(table))]
    NewerData { table: String },
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
    #[error("doc {0}'s log moved under the session: expected position {1}, found {2}")]
    LogMoved(DocId, Seq, Seq),
    /// Tier 0: the space has no blob key (never used, or purged).
    #[error("no blob key for this space (never used, or purged)")]
    NoBlobSpace,
    /// Tier 0: no index row for this blob (hex id).
    #[error("no blob {0}")]
    NoBlob(String),
    /// Tier 0: version {1} of blob doc {0} is its current content and can't be deleted.
    #[error("version {1} of doc {0} is its current content")]
    CurrentVersion(DocId, VersionId),
    /// No randomness source for a new key (the kit reads `/dev/urandom`).
    #[error("no randomness source: {0}")]
    NoRandom(String),
    /// Tier 0: version {1} of blob doc {0} was purged; its content is gone.
    #[error("version {1} of doc {0} was purged")]
    PurgedVersion(DocId, VersionId),
}

/// Tables a newer kit adds without a format bump, with the kit that adds them (none known
/// now: v0.9.7's `authored`, listed here by v0.9.5, is this kit's own, in [`DOC_TABLES`]).
const FUTURE_TABLES: &[(&str, &str)] = &[];

/// [`StoreError::NewerData`]'s message: what holds the store, and what to upgrade to.
fn newer_data_guidance(table: &str) -> String {
    let kit = FUTURE_TABLES
        .iter()
        .find(|(t, _)| *t == table)
        .map_or_else(|| "a newer kit".to_owned(), |(_, v)| format!("kit ≥ {v}"));
    format!(
        "This store holds data from a newer kit (table \"{table}\"). Upgrade K2 to a version \
         with {kit} to purge or rewrite it; reading and editing still work."
    )
}

/// Disk sync: the doc state the file on disk corresponds to. Its text is the file's text
/// (decoded), so an outside edit can be diffed against it and merged into the live doc as a
/// change made from that state. Kept per doc so a restart can still merge correctly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncState {
    /// The keyed hash of the file's bytes (`hash::file_hash`, under the doc's key). Empty
    /// after a purge that couldn't read the file: it then matches nothing, so the file is
    /// read and compared with the replica's text on the next check. A format-2 store may
    /// hold a plain SHA-256 (32 bytes) until the file is next read.
    pub hash: Vec<u8>,
    /// Log position the file corresponds to.
    pub seq: Seq,
    /// Engine state whose text is the file's text.
    pub state: Vec<u8>,
}

/// Algorithm of a doc's hash key: 1 = HMAC-SHA256.
const ALG_HMAC_SHA256: u8 = 1;

fn enc_doc_key(k: &[u8; 32]) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1).u8(ALG_HMAC_SHA256).bytes(k);
    w.0
}

fn dec_doc_key(b: &[u8]) -> Result<[u8; 32], StoreError> {
    let mut r = R::new(b, "doc key");
    if r.u8()? != 1 || r.u8()? != ALG_HMAC_SHA256 {
        return Err(StoreError::Corrupt("doc key version"));
    }
    let k = r
        .bytes()?
        .try_into()
        .map_err(|_| StoreError::Corrupt("doc key"))?;
    r.end()?;
    Ok(k)
}

/// A fresh random key.
pub(crate) fn fresh_key() -> Result<[u8; 32], StoreError> {
    crate::hash::random32().map_err(|e| StoreError::NoRandom(e.to_string()))
}

fn enc_sync(s: &SyncState) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1).bytes(&s.hash).u64(s.seq).bytes(&s.state);
    w.0
}

fn dec_sync(b: &[u8]) -> Result<SyncState, StoreError> {
    let mut r = R::new(b, "sync state");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("sync state version"));
    }
    let s = SyncState {
        hash: r.bytes()?,
        seq: r.u64()?,
        state: r.bytes()?,
    };
    r.end()?;
    Ok(s)
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
    /// Disk sync: the keyed hash (HMAC-SHA256 under the doc's own key, never a plain content
    /// hash; format 3) of the bytes last written to or read from the file, and the log
    /// position they correspond to. Opaque to hosts; meaningless after a purge rotates the
    /// key.
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
    /// Every transaction holds a read guard for its whole life; [`DocStore::rewrite_file`]
    /// takes the write guard to swap in the rewritten file, so no transaction ever spans
    /// the swap.
    db: RwLock<Database>,
    path: PathBuf,
    /// Commits skip the sync to disk ([`DocStore::unsynced_for_tests`]).
    #[cfg(any(feature = "conformance", feature = "differential"))]
    unsynced: std::sync::atomic::AtomicBool,
}

/// A write transaction with the store's read guard held until it commits or drops.
pub(crate) struct Wtx<'a> {
    tx: redb::WriteTransaction,
    _g: RwLockReadGuard<'a, Database>,
}

impl Wtx<'_> {
    pub(crate) fn commit(self) -> Result<(), redb::CommitError> {
        self.tx.commit()
    }
}

impl std::ops::Deref for Wtx<'_> {
    type Target = redb::WriteTransaction;
    fn deref(&self) -> &redb::WriteTransaction {
        &self.tx
    }
}

/// A read transaction with the store's read guard held.
pub(crate) struct Rtx<'a> {
    tx: redb::ReadTransaction,
    _g: RwLockReadGuard<'a, Database>,
}

impl std::ops::Deref for Rtx<'_> {
    type Target = redb::ReadTransaction;
    fn deref(&self) -> &redb::ReadTransaction {
        &self.tx
    }
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

/// Log record version 3: version 2 with the update either present (`0`, its bytes) or
/// purged (`1`, its byte length only: K-PURGE-2's `redacted {byte length}`).
fn enc_entry(e: &LogEntry) -> Vec<u8> {
    enc_entry_as(e, None)
}

/// [`enc_entry`], or (`redacted: Some(n)`) the entry's metadata with `redacted {n}` in place
/// of its update.
fn enc_entry_as(e: &LogEntry, redacted: Option<u64>) -> Vec<u8> {
    let mut w = W::default();
    w.u8(3)
        .str(&e.author)
        .u64(e.client)
        .u64(e.at_ms)
        .str(&e.via);
    match redacted {
        None => w.u8(0).bytes(&e.update),
        Some(n) => w.u8(1).u64(n),
    };
    w.opt_str(e.on_behalf_of.as_deref());
    w.0
}

/// Decodes log records version 1, 2 and 3. A purged entry decodes with an empty update.
#[cfg(test)]
fn dec_entry(b: &[u8]) -> Result<LogEntry, StoreError> {
    Ok(dec_entry_full(b)?.0)
}

/// The entry, and the purged update's byte length when it was purged.
fn dec_entry_full(b: &[u8]) -> Result<(LogEntry, Option<u64>), StoreError> {
    let mut r = R::new(b, "log entry");
    let v = r.u8()?;
    if !(1..=3).contains(&v) {
        return Err(StoreError::Corrupt("log entry version"));
    }
    let (author, client, at_ms, via) = (r.str()?, r.u64()?, r.u64()?, r.str()?);
    let (update, redacted) = if v < 3 {
        (r.bytes()?, None)
    } else {
        match r.u8()? {
            0 => (r.bytes()?, None),
            1 => (Vec::new(), Some(r.u64()?)),
            _ => return Err(StoreError::Corrupt("log entry")),
        }
    };
    let e = LogEntry {
        author,
        client,
        at_ms,
        via,
        update,
        on_behalf_of: if v >= 2 { r.opt_str()? } else { None },
    };
    r.end()?;
    Ok((e, redacted))
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
    enc_version_redacted(v, &[])
}

/// Version record 2: the header, the purged spans (`redacted`, in the coordinates of the
/// text before any purge), then each root's content with those spans cut out.
fn enc_version_redacted(v: &Version, redacted: &[Redaction]) -> Vec<u8> {
    let mut w = W::default();
    w.u8(2)
        .u64(v.seq)
        .u64(v.at_ms)
        .u8(v.reason as u8)
        .opt_str(v.name.as_deref())
        .strs(&v.authors);
    w.u32(redacted.len() as u32);
    for x in redacted {
        w.str(&x.root).u64(x.start).u64(x.len);
    }
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
    Ok(dec_version_full(b)?.0)
}

/// A version record's spans, versions 1 and 2 (version 1 has none).
fn dec_redactions(r: &mut R<'_>, v: u8, total: usize) -> Result<Vec<Redaction>, StoreError> {
    if v < 2 {
        return Ok(Vec::new());
    }
    let n = r.u32()? as usize;
    if n > total {
        return Err(StoreError::Corrupt("version redactions"));
    }
    (0..n)
        .map(|_| {
            Ok(Redaction {
                root: r.str()?,
                start: r.u64()?,
                len: r.u64()?,
            })
        })
        .collect()
}

/// A version (its content as stored: purged spans cut out) and its purged spans.
fn dec_version_full(b: &[u8]) -> Result<(Version, Vec<Redaction>), StoreError> {
    let mut r = R::new(b, "version");
    let ver = r.u8()?;
    if ver != 1 && ver != 2 {
        return Err(StoreError::Corrupt("version record version"));
    }
    let (seq, at_ms, reason, name, authors) = (
        r.u64()?,
        r.u64()?,
        VersionReason::from_u8(r.u8()?)?,
        r.opt_str()?,
        r.strs()?,
    );
    let redacted = dec_redactions(&mut r, ver, b.len())?;
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
    Ok((
        Version {
            seq,
            at_ms,
            reason,
            name,
            authors,
            content,
        },
        redacted,
    ))
}

/// Store `versions` (in order) after the doc's last version, in `w`.
fn put_versions(
    w: &redb::WriteTransaction,
    id: DocId,
    versions: &[Version],
) -> Result<(), StoreError> {
    if versions.is_empty() {
        return Ok(());
    }
    let mut t = w.open_table(VERSIONS)?;
    let mut last = t
        .range((id, 0)..=(id, u64::MAX))?
        .next_back()
        .transpose()?
        .map(|(k, _)| k.value().1)
        .unwrap_or(0);
    for v in versions {
        last += 1;
        t.insert((id, last), enc_version(v).as_slice())?;
    }
    Ok(())
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
    fn guard(&self) -> RwLockReadGuard<'_, Database> {
        self.db.read().unwrap_or_else(|p| p.into_inner())
    }

    /// A write transaction (redb's default durability: committed means on disk).
    pub(crate) fn begin_write(&self) -> Result<Wtx<'_>, StoreError> {
        let g = self.guard();
        #[allow(unused_mut)]
        let mut tx = g.begin_write()?;
        #[cfg(any(feature = "conformance", feature = "differential"))]
        if self.unsynced.load(std::sync::atomic::Ordering::Relaxed) {
            tx.set_durability(redb::Durability::None);
        }
        Ok(Wtx { tx, _g: g })
    }

    pub(crate) fn begin_read(&self) -> Result<Rtx<'_>, StoreError> {
        let g = self.guard();
        Ok(Rtx {
            tx: g.begin_read()?,
            _g: g,
        })
    }

    /// The store file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Open (or create) the store at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_owned();
        // a rewrite a crash interrupted left its temp file: the store itself is whole
        let _ = std::fs::remove_file(rewrite::temp_path(&path));
        let db = Database::create(&path)?;
        let w = db.begin_write()?;
        {
            let mut meta = w.open_table(META)?;
            let found = meta.get("store_format")?.map(|v| v.value());
            match found {
                None => {
                    meta.insert("store_format", STORE_FORMAT)?;
                }
                Some(f) if f > STORE_FORMAT => return Err(StoreError::Newer { found: f }),
                // 1 -> 2 -> 3: nothing to rewrite (older records still decode); new tables
                // below. 3 -> 4: the blob approvals, backfilled below. 4 -> 5: the
                // provenance table (below), empty: every existing element is its own root.
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
            w.open_table(SYNC)?;
            w.open_table(DOC_KEYS)?;
            blob::create_tables(&w)?;
            history::create_tables(&w)?;
            if found.is_some_and(|f| f < 4) {
                blob::backfill_approvals(&w)?;
            }
            // what a kit <= v0.9.6 left in the authored table (v0.9.7, no format bump): the
            // file is rewritten below, here or at the next open if this one fails there
            if history::sweep_authored(&w)? {
                meta.insert(REWRITE_PENDING, 1)?;
            }
        }
        w.commit()?;
        let s = Self {
            db: RwLock::new(db),
            path,
            #[cfg(any(feature = "conformance", feature = "differential"))]
            unsynced: std::sync::atomic::AtomicBool::new(false),
        };
        s.finish_pending_rewrite()?;
        Ok(s)
    }

    /// The rewrite an open's sweep asked for ([`REWRITE_PENDING`]), so no freed page keeps
    /// a byte the sweep removed; the mark is cleared once it is done. A failed rewrite fails
    /// the open (loudly) and stays pending.
    fn finish_pending_rewrite(&self) -> Result<(), StoreError> {
        let pending = {
            let r = self.begin_read()?;
            let m = r.open_table(META)?;
            let v = m.get(REWRITE_PENDING)?.map(|v| v.value());
            v
        };
        if pending.is_none() {
            return Ok(());
        }
        self.rewrite_file()?;
        let w = self.begin_write()?;
        w.open_table(META)?.remove(REWRITE_PENDING)?;
        w.commit()?;
        Ok(())
    }

    /// Test-only (feature `conformance`, off by default): from now on, commit without
    /// syncing the store file to disk. Everything in this process sees every commit as
    /// before; only a crash or a reopen of the file may lose them. For long in-process test
    /// runs (the undo spec's trace test), where a sync per change is most of the time.
    #[cfg(any(feature = "conformance", feature = "differential"))]
    pub fn unsynced_for_tests(&self) {
        self.unsynced
            .store(true, std::sync::atomic::Ordering::Relaxed);
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
        let r = self.begin_read()?;
        Ok(r.open_table(PATHS)?
            .get(key.index().as_str())?
            .map(|v| v.value()))
    }

    /// Create a doc. Fails if its key already has one.
    pub fn create(&self, meta: &DocMeta) -> Result<DocId, StoreError> {
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
        self.create_imported_synced(meta, client, owner, entry, version, None, None)
    }

    /// [`DocStore::create_imported`], also recording the disk-sync state (its `seq` is set
    /// to the import's position) with the doc's hash `key` (its hash is keyed by it), and
    /// the import's change summary and revert record, in the same transaction.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_imported_synced(
        &self,
        meta: &DocMeta,
        client: ClientId,
        owner: &str,
        entry: Option<&LogEntry>,
        version: &Version,
        sync: Option<(&SyncState, &[u8; 32])>,
        change: Option<&NewChange>,
    ) -> Result<(DocId, Option<Seq>, VersionId), StoreError> {
        let w = self.begin_write()?;
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
                    if let Some(c) = change {
                        history::put_changes(&w, id, &[1], std::slice::from_ref(c))?;
                    }
                    Some(1)
                }
                None => None,
            };
            let mut v = version.clone();
            v.seq = seq.unwrap_or(0);
            w.open_table(VERSIONS)?
                .insert((id, 1), enc_version(&v).as_slice())?;
            if let Some((sy, key)) = sync {
                let sy = SyncState {
                    seq: v.seq,
                    ..sy.clone()
                };
                w.open_table(SYNC)?
                    .insert((id, SYNCED), enc_sync(&sy).as_slice())?;
                w.open_table(DOC_KEYS)?
                    .insert(id, enc_doc_key(key).as_slice())?;
            }
            (id, seq, 1)
        };
        w.commit()?;
        Ok(out)
    }

    pub fn meta(&self, id: DocId) -> Result<DocMeta, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(DOCS)?;
        let v = t.get(id)?.ok_or(StoreError::NoDoc(id))?;
        dec_meta(v.value())
    }

    /// Replace a doc's metadata. The key can't change here; use [`DocStore::rekey`].
    pub fn put_meta(&self, id: DocId, meta: &DocMeta) -> Result<(), StoreError> {
        let w = self.begin_write()?;
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
        let w = self.begin_write()?;
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
        let w = self.begin_write()?;
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
        let r = self.begin_read()?;
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
            let (e, redacted) = dec_entry_full(v.value())?;
            if redacted.is_some() {
                // a purge snapshots past every entry it redacts
                return Err(StoreError::Corrupt("purged log entry after the snapshot"));
            }
            updates.push((k.value().1, e));
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
        let w = self.begin_write()?;
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
        let w = self.begin_write()?;
        w.open_table(CLIENTS)?.insert((id, client), owner)?;
        w.commit()?;
        Ok(())
    }

    /// Every client ID recorded for doc `id`, with its owner, in client order.
    pub fn clients_of(&self, id: DocId) -> Result<Vec<(ClientId, String)>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(CLIENTS)?;
        let mut out = Vec::new();
        for e in t.range((id, 0)..=(id, u64::MAX))? {
            let (k, v) = e?;
            out.push((k.value().1, v.value().to_owned()));
        }
        Ok(out)
    }

    pub fn client_owner(&self, id: DocId, client: ClientId) -> Result<Option<String>, StoreError> {
        let r = self.begin_read()?;
        Ok(r.open_table(CLIENTS)?
            .get((id, client))?
            .map(|v| v.value().to_owned()))
    }

    /// Store a version; returns its id (increasing per doc).
    pub fn put_version(&self, id: DocId, v: &Version) -> Result<VersionId, StoreError> {
        let w = self.begin_write()?;
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
        let r = self.begin_read()?;
        let mut out = Vec::new();
        for kv in r.open_table(VERSIONS)?.range((id, 0)..=(id, u64::MAX))? {
            let (k, v) = kv?;
            out.push((k.value().1, dec_version(v.value())?));
        }
        Ok(out)
    }

    pub fn version(&self, id: DocId, vid: VersionId) -> Result<Version, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(VERSIONS)?;
        let v = t.get((id, vid))?.ok_or(StoreError::NoVersion(id, vid))?;
        dec_version(v.value())
    }

    /// Name (or rename) a version.
    pub fn name_version(&self, id: DocId, vid: VersionId, name: &str) -> Result<(), StoreError> {
        let w = self.begin_write()?;
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
        let w = self.begin_write()?;
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
    // ----- disk sync (crate-private: sessions keep these consistent) -----

    /// The synced state, and the write-back in flight if a crash interrupted one.
    pub(crate) fn sync_states(
        &self,
        id: DocId,
    ) -> Result<(Option<SyncState>, Option<SyncState>), StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(SYNC)?;
        let get = |slot| -> Result<Option<SyncState>, StoreError> {
            t.get((id, slot))?.map(|v| dec_sync(v.value())).transpose()
        };
        Ok((get(SYNCED)?, get(PENDING)?))
    }

    /// One transaction for a change that came from disk (or only moves the sync point):
    /// append `entries` (their first position must be `expect_first`, so the records below
    /// can name it) with their change summaries and revert records (`changes`), store
    /// `versions`, replace the doc's metadata, set the synced state and clear any write-back
    /// in flight.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn commit_disk(
        &self,
        id: DocId,
        expect_first: Seq,
        entries: &[LogEntry],
        changes: &[NewChange],
        versions: &[Version],
        meta: &DocMeta,
        synced: &SyncState,
    ) -> Result<Vec<Seq>, StoreError> {
        let w = self.begin_write()?;
        let seqs = self.append_in(&w, id, entries)?;
        if let Some(&first) = seqs.first() {
            if first != expect_first {
                return Err(StoreError::LogMoved(id, expect_first, first));
            }
        }
        history::put_changes(&w, id, &seqs, changes)?;
        {
            let mut docs = w.open_table(DOCS)?;
            let old = dec_meta(docs.get(id)?.ok_or(StoreError::NoDoc(id))?.value())?;
            if old.key != meta.key {
                return Err(StoreError::KeyChange);
            }
            docs.insert(id, enc_meta(meta).as_slice())?;
        }
        put_versions(&w, id, versions)?;
        {
            let mut t = w.open_table(SYNC)?;
            t.insert((id, SYNCED), enc_sync(synced).as_slice())?;
            t.remove((id, PENDING))?;
        }
        w.commit()?;
        Ok(seqs)
    }

    /// Record a write-back about to happen (before the file is renamed into place), so a
    /// crash right after the rename can still tell the file is the kit's own write.
    pub(crate) fn set_pending_sync(&self, id: DocId, s: &SyncState) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        w.open_table(SYNC)?
            .insert((id, PENDING), enc_sync(s).as_slice())?;
        w.commit()?;
        Ok(())
    }

    /// The write-back in flight landed: it becomes the synced state, and `meta` (with the
    /// new synced hash and position) is stored, in one transaction.
    pub(crate) fn promote_pending_sync(&self, id: DocId, meta: &DocMeta) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        {
            let mut t = w.open_table(SYNC)?;
            let p = t
                .remove((id, PENDING))?
                .ok_or(StoreError::Corrupt("no write-back in flight"))?
                .value()
                .to_vec();
            t.insert((id, SYNCED), p.as_slice())?;
            let mut docs = w.open_table(DOCS)?;
            let old = dec_meta(docs.get(id)?.ok_or(StoreError::NoDoc(id))?.value())?;
            if old.key != meta.key {
                return Err(StoreError::KeyChange);
            }
            docs.insert(id, enc_meta(meta).as_slice())?;
        }
        w.commit()?;
        Ok(())
    }

    /// Forget a write-back in flight that didn't land.
    pub(crate) fn clear_pending_sync(&self, id: DocId) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        w.open_table(SYNC)?.remove((id, PENDING))?;
        w.commit()?;
        Ok(())
    }

    /// The doc's hash key (format 3), created at random if it has none yet (a doc from a
    /// format-2 store, or one created without a file). A purge replaces it.
    pub(crate) fn doc_key(&self, id: DocId) -> Result<[u8; 32], StoreError> {
        {
            let r = self.begin_read()?;
            if let Some(v) = r.open_table(DOC_KEYS)?.get(id)? {
                return dec_doc_key(v.value());
            }
        }
        let fresh = fresh_key()?;
        let w = self.begin_write()?;
        let k = {
            if w.open_table(DOCS)?.get(id)?.is_none() {
                return Err(StoreError::NoDoc(id));
            }
            let mut t = w.open_table(DOC_KEYS)?;
            let found = t.get(id)?.map(|v| dec_doc_key(v.value())).transpose()?;
            match found {
                Some(k) => k,
                None => {
                    t.insert(id, enc_doc_key(&fresh).as_slice())?;
                    fresh
                }
            }
        };
        w.commit()?;
        Ok(k)
    }

    /// Replace the synced state and metadata, clearing any write-back in flight. One
    /// transaction. (Tests: stage a format-2 plain hash.)
    #[cfg(test)]
    pub(crate) fn rekey_sync(
        &self,
        id: DocId,
        meta: &DocMeta,
        synced: &SyncState,
    ) -> Result<(), StoreError> {
        self.commit_disk(id, Seq::MAX, &[], &[], &[], meta, synced)
            .map(|_| ())
    }

    /// The client ID owned by `owner` in this doc, if any (the lowest, if several).
    pub fn client_owned_by(&self, id: DocId, owner: &str) -> Result<Option<ClientId>, StoreError> {
        let r = self.begin_read()?;
        for kv in r.open_table(CLIENTS)?.range((id, 0)..=(id, u64::MAX))? {
            let (k, v) = kv?;
            if v.value() == owner {
                return Ok(Some(k.value().1));
            }
        }
        Ok(None)
    }

    /// The newest version of a doc.
    pub fn last_version(&self, id: DocId) -> Result<Option<(VersionId, Version)>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(VERSIONS)?;
        let last = t.range((id, 0)..=(id, u64::MAX))?.next_back().transpose()?;
        last.map(|(k, v)| Ok((k.value().1, dec_version(v.value())?)))
            .transpose()
    }

    // ----- approvals (on-behalf changes) -----

    /// The approval record of change `seq`, if it was made on someone's behalf.
    pub fn approval(&self, id: DocId, seq: Seq) -> Result<Option<Approval>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(APPROVALS)?;
        let v = t.get((id, seq))?;
        v.map(|v| dec_approval(v.value())).transpose()
    }

    /// The approval states of those of changes `seqs` made on someone's behalf, in one read
    /// (blame's lookup: O(seqs), never the doc's whole approval table).
    pub(crate) fn approvals_of(
        &self,
        id: DocId,
        seqs: &std::collections::BTreeSet<Seq>,
    ) -> Result<std::collections::HashMap<Seq, ApprovalState>, StoreError> {
        let r = self.begin_read()?;
        let t = r.open_table(APPROVALS)?;
        let mut out = std::collections::HashMap::new();
        for &seq in seqs {
            if let Some(v) = t.get((id, seq))? {
                out.insert(seq, dec_approval(v.value())?.state);
            }
        }
        Ok(out)
    }

    /// Every approval record of a doc, oldest change first.
    pub fn approvals(&self, id: DocId) -> Result<Vec<(Seq, Approval)>, StoreError> {
        let r = self.begin_read()?;
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
        let w = self.begin_write()?;
        let settles = {
            let mut t = w.open_table(APPROVALS)?;
            let Some(old) = t
                .get((id, seq))?
                .map(|v| dec_approval(v.value()))
                .transpose()?
            else {
                return Err(StoreError::NoApproval(id, seq));
            };
            t.insert((id, seq), enc_approval(a).as_slice())?;
            old.state == ApprovalState::Pending && a.state != ApprovalState::Pending
        };
        if settles {
            history::settle_pending(&w, id, seq)?;
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
        let r = self.begin_read()?;
        let t = r.open_table(HOST)?;
        let v = t.get((ns, key))?;
        Ok(v.map(|v| v.value().to_vec()))
    }

    /// Delete `ns`/`key`; returns whether it existed. One durable transaction.
    pub fn host_delete(&self, ns: &str, key: &str) -> Result<bool, StoreError> {
        let w = self.begin_write()?;
        let existed = w.open_table(HOST)?.remove((ns, key))?.is_some();
        w.commit()?;
        Ok(existed)
    }

    /// Every `(key, value)` in `ns` whose key starts with `prefix` (`""` for all), in key
    /// order, from one consistent read.
    pub fn host_scan(&self, ns: &str, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        let r = self.begin_read()?;
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
        let w = self.begin_write()?;
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

/// The fuzz targets' entry point for the store's own record decoders (feature `fuzzing`):
/// the names (one is the session's), and [`fuzz_record`]. Engine bytes inside a record (a snapshot's state, a log
/// entry's update, a sync state) are carried, never decoded: no fuzzed byte reaches the
/// engine.
#[cfg(feature = "fuzzing")]
pub(crate) const FUZZ_RECORDS: &[&str] = &[
    "doc key",
    "sync state",
    "doc meta",
    "log entry",
    "approval",
    "version",
    "snapshot",
    "change",
    "change head",
    "revert record",
    "authored hunks",
    "provenance row",
    "revert index",
    "revert carry",
    "revert stats",
    "carry taint",
    "version head",
    "blob space",
    "blob row",
    "blob version",
    "blob sync",
    // the session's (`session::disk`), routed by `crate::fuzzing`
    "file stat",
];

/// Decode `b` as record `name` ([`FUZZ_RECORDS`]) and check the oracle
/// ([`enc::fuzz_fixpoint`]); `None` for an unknown name. Panics on a violation.
#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_record(name: &str, b: &[u8]) -> Option<bool> {
    use enc::fuzz_fixpoint as fx;
    Some(match name {
        "doc key" => fx(name, b, true, dec_doc_key, enc_doc_key),
        "sync state" => fx(name, b, true, dec_sync, enc_sync),
        "doc meta" => fx(name, b, false, dec_meta, enc_meta),
        "log entry" => fx(name, b, false, dec_entry_full, |(e, red)| {
            enc_entry_as(e, *red)
        }),
        "approval" => fx(name, b, false, dec_approval, enc_approval),
        "version" => {
            let ok = fx(name, b, false, dec_version_full, |(v, red)| {
                enc_version_redacted(v, red)
            });
            // the version head reads the same header as the full record
            if let Ok((v, red)) = dec_version_full(b) {
                let h = history::fuzz_version_head(b)
                    .unwrap_or_else(|e| panic!("version: the head refuses a record: {e:?}"));
                assert_eq!(
                    (h.seq, h.at_ms, h.reason, &h.name, &h.authors, &h.redacted),
                    (v.seq, v.at_ms, v.reason, &v.name, &v.authors, &red),
                    "version: the head differs from the full record"
                );
            }
            ok
        }
        "snapshot" => fx(name, b, true, dec_snapshot, |(seq, st)| {
            enc_snapshot(*seq, st)
        }),
        other => return history::fuzz_record(other, b).or_else(|| blob::fuzz_record(other, b)),
    })
}

#[cfg(test)]
mod tests;
