//! Session errors.

use super::awareness::ClaimId;
use super::host::Need;
use super::pack::{Declined, PackError, PackRefusal};
use super::participant::ConnId;
use crate::doc::DocError;
use crate::elem::Key;
use crate::store::{DocKey, Seq, StoreError};

/// One node of a tier-0 verb's `--base` refusal ([`SessionError::BaseMoved`]): the node
/// (`address`, its key), the field both changes touch (a props entry such as `attrs.layer`, or
/// `text`, or the node-level change: `added`, `removed`, `moved`, `renamed`, `content`), and
/// who changed it since the base, in which version.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BaseConflict {
    pub address: String,
    pub key: String,
    pub field: String,
    pub by: String,
    pub change: crate::store::VersionId,
}

/// Who holds the index nodes a claim was refused on ([`SessionError::NodesClaimed`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NodesClaim {
    pub id: ClaimId,
    pub by: String,
    pub name: String,
    /// The nodes the two claims share, in key order.
    pub keys: Vec<crate::index::NodeKey>,
    pub remaining_ms: u64,
    pub note: Option<String>,
}

/// Who holds the elements an edit or a claim was refused on ([`SessionError::ElemsClaimed`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ElemsClaim {
    pub id: ClaimId,
    pub by: String,
    pub name: String,
    pub root: String,
    /// The claim's keys, in key order.
    pub keys: Vec<Key>,
    pub remaining_ms: u64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SessionError {
    /// The participant lacks a right. When it was read, they were also removed from the doc.
    #[error("refused: needs {need:?} rights")]
    Forbidden { need: Need },
    /// The connection isn't (or is no longer) in this doc: it left, was kicked or expired,
    /// or belongs to another session. Join again.
    #[error("{conn} has not joined this doc")]
    NotJoined { conn: ConnId },
    /// The connection joined past [`super::Config::participants_max`]: it watches,
    /// read-only, until a slot frees and the host promotes it
    /// ([`super::Session::promote_watcher`]). Nothing was applied. Exit 3.
    #[error("watching: the doc has its {max} participants; read-only until the host lets you in")]
    Watching { max: usize },
    /// The doc holds all the writers it allows ([`super::Config::writers_max`]) and this
    /// connection holds a writer it doesn't count ([`super::Joined::writers_full`]): it reads,
    /// but never edits. Nothing was applied. `MPC-E908`, exit 2.
    #[error("read-only: this doc has {count} writers (the most is {max})")]
    WritersFull { count: u64, max: u64 },
    /// `edit(.., if_version: Some(expected))` (or an edit verb's `--if-version`, or a tier 0
    /// write from an old base) but the doc is at `actual`. Nothing applied. One line, no doc
    /// text: `doc changed since 12 (now 14)`. Exit 1.
    #[error("doc changed since {expected} (now {actual})")]
    Stale { expected: Seq, actual: Seq },
    /// A structured edit's positional addresses count in its `--base` version,
    /// and that version isn't held any more (not live, not a recent read, not stored).
    /// Nothing applied. Exit 8 (`MPC-E112 base_expired`): never retry the same request.
    #[error("base version {base} is unknown or no longer cached; nothing applied")]
    BaseExpired { base: Seq },
    /// `note_opened` named a version the doc never had. Nothing noted. Exit 4
    /// (`MPC-E180 opened_version_unknown`).
    #[error("no version {version} of this doc; nothing noted")]
    OpenedUnknown { version: u64 },
    /// `note_opened` named a version the kit doesn't hold (purged, or not kept). Nothing
    /// noted. Exit 8 (`MPC-E181 opened_version_gone`).
    #[error("version {version} is gone (purged or not kept); nothing noted")]
    OpenedGone { version: u64 },
    /// `note_opened` on a doc with no file: no app saves it, so there is nothing to note. Exit
    /// 2 (`MPC-E182 opened_no_file`).
    #[error("this doc has no file; nothing noted")]
    OpenedNoFile,
    #[error("too many edits; retry in {retry_after_ms} ms")]
    RateLimited { retry_after_ms: u64 },
    /// The file can't be a live doc, or live editing was turned off for it.
    #[error("live editing is off: {0}")]
    Declined(Declined),
    /// No stored doc at this key, and no file bytes to create one from.
    #[error("no doc at {0:?}")]
    NotFound(DocKey),
    #[error("doc was stored by pack {stored:?}, not {pack:?}")]
    PackMismatch { stored: String, pack: String },
    /// The stored doc's engine stamp ([`crate::store::DocMeta`]'s `engine` and
    /// `engine_format`) names another engine, or another stored-state format, than this
    /// build runs (v0.10.1): it is not loaded, so no engine reads another's state. A stamp of
    /// this engine's family at another library version, with this format, loads; so does an
    /// unstamped doc (an empty `engine`). Nothing changed. Exit 6, as a store from a newer
    /// build.
    #[error(
        "this doc was written by engine {stored} (format {stored_format}); this kit runs \
         engine {engine} (format {engine_format})"
    )]
    EngineMismatch {
        stored: String,
        stored_format: u32,
        engine: String,
        engine_format: u32,
    },
    /// The stored doc is on the native engine, and this build doesn't have it: that doc alone
    /// is refused (opening, deleting or purging it), nothing of it read or changed, while the
    /// store and its other docs work. Every session call that meets such a doc gives this
    /// variant, never a store error. `MPC-E169` `native_engine_missing`. Exit 6.
    #[error(
        "this doc is on the native engine (format {stored_format}), which this build \
         doesn't have"
    )]
    NativeEngineMissing { stored: String, stored_format: u32 },
    /// A host asked for a new doc on the native engine (a Beta) while the Beta is off
    /// ([`super::Config::native_beta`]): nothing was read or made. Docs already on the native
    /// engine open whatever the switch says. `MPC-E167` `native_beta_off`. Exit 2.
    #[error("the native engine (a Beta) is off on this host: a new doc can't be made on it")]
    NativeBetaOff,
    /// A change would grow a doc on the native engine (a Beta) past its live-text cap: its
    /// text roots together over the engine's per-change string limit (`string_bytes_per_op`,
    /// 4 MiB at the engine's default limits, which the kit never changes). Nothing applied,
    /// and nothing already in the doc is dropped. `MPC-E909` `native_text_cap`. Exit 2, as
    /// [`SessionError::AtLimit`] (the standard engine's size limits).
    #[error(
        "this doc's live text would be {would_be} bytes, past the native engine's {limit}-byte \
         cap"
    )]
    NativeTextCap {
        limit: usize,
        /// What the change would have made it.
        would_be: usize,
    },
    /// A host asked to demote a doc on the native engine (a Beta) to tier 0, or to promote a
    /// tier-0 doc whose key holds one (`verb`: `demote` or `promote`): in the Beta a native
    /// doc doesn't move to or from tier 0. Nothing changed. `MPC-E721` `native_no_tier0`.
    /// Exit 4, as the standard engine's refusals of the same calls
    /// ([`SessionError::Declined`] `not_text`, [`SessionError::PackMismatch`]).
    #[error(
        "this doc is on the native engine (a Beta), which doesn't move to or from tier 0 \
         ({verb})"
    )]
    NativeNoTier0 { verb: &'static str },
    #[error("bad participant: {0}")]
    BadParticipant(String),
    #[error("bad position: {0}")]
    BadPosition(String),
    #[error("bad edit: {0}")]
    BadEdit(String),
    /// [`super::Session::set_approval`] / [`super::Session::approval`]: change `seq` was not
    /// made on anyone's behalf (or doesn't exist). From
    /// [`super::BlobSession::set_approval`], `seq` is the blob version.
    #[error("change {seq} has no approval (it wasn't made on anyone's behalf)")]
    NoApproval { seq: Seq },
    /// An approval can only go from pending to approved or declined, once.
    #[error("bad approval: {0}")]
    BadApproval(String),
    /// [`super::SessionManager::open_file`] / [`super::Session::file_moved`]: the path isn't
    /// a regular file inside the root (a folder, a symlink resolving outside it...).
    #[error("bad path: {0}")]
    BadPath(String),
    /// Refused for now, not for good: `approvals_backlog` (too many pending on-behalf
    /// changes; the person approves or declines some first). Exit 2.
    #[error("held: {reason}")]
    Held { reason: &'static str },
    /// `revert`: the change is older than the revert window. Exit 8; never
    /// retry. The way back is `restore --version`.
    #[error("change {seq} is too old to revert (revert_too_old); use restore --version")]
    RevertTooOld { seq: Seq },
    /// No change at that log position (`revert`), or no such version. Exit 4.
    #[error("no change or version {0} in this doc")]
    NoSuchChange(u64),
    /// `file_at` of a version of a pack that keeps a carrier, when the version has none
    /// (cut by an older kit or before this build, or a purge erased it): its file can't be
    /// rebuilt exactly, so none is given. Exit 4 (`MPC-E208`).
    #[error("version {version} has no copy of its file's own bytes (version_no_carrier)")]
    VersionNoCarrier { version: u64 },
    /// The live carrier's pairing doesn't fit it ([`super::SpliceError::PairingMismatch`]):
    /// the file type can't tell which unit of the file a key is, so nothing was spliced or
    /// written; the file is as it was and the doc keeps its changes. A pack bug or a damaged
    /// store. Exit 1 (`MPC-E521`).
    #[error("the file's pairing doesn't fit it: {reason} (pairing_mismatch)")]
    PairingMismatch { reason: String },
    /// A cell write ([`super::Session::edit_ops`]) under a row or a column the table doesn't
    /// know: dropped by `rows drop-deleted` (`dropped`: the drop's change and who ran it, while
    /// the store still holds its record), or never in it. Its cell could never be seen,
    /// restored or swept, so the whole edit is refused; nothing applied. `MPC-E805
    /// no_element`, exit 4, as the grid verbs' unknown key. A deleted key the table still
    /// knows stays a hidden write (csv contract §4.3).
    #[error("{}", unknown_elem_text(root, key, dropped.as_ref()))]
    UnknownElem {
        root: &'static str,
        key: String,
        dropped: Option<DroppedBy>,
    },
    /// Reading or writing the file failed. Nothing is lost: the doc keeps its changes and
    /// write-back retries.
    #[error("disk: {0}")]
    Disk(String),
    /// Tier 0, [`super::BlobSession::record_saved`]: the file doesn't hold the bytes the host
    /// says it saved (`size` of them; `file_size` is what the file holds, `None` when there is
    /// no file). Another program saved after the host, or the host's save didn't land.
    /// Nothing was recorded and the file wasn't touched. Exit 1.
    #[error("{}", save_mismatch_text(*size, *file_size))]
    SaveMismatch { size: u64, file_size: Option<u64> },
    /// Tier 0, [`super::BlobSession::record_saved`]: the ticket can't be recorded by this call:
    /// it was already recorded (or refused), it is another connection's, or this doc never
    /// issued it (or issued it so many tickets ago it is forgotten). A host bug; nothing was
    /// recorded. Exit 2.
    #[error("bad save ticket: a ticket is good for one record_saved, by the connection that asked for it, on its doc")]
    SaveTicketInvalid,
    /// Tier 0: someone else holds the doc's lease (until `expires_ms`, host clock). Nothing
    /// was written. Suggested exit **5** (`leased`), printing the holder and expiry.
    #[error("leased by {name} ({holder}) until {expires_ms}")]
    Leased {
        holder: String,
        name: String,
        expires_ms: u64,
    },
    /// Tier 0: renewing a lease the caller doesn't hold (none is held).
    #[error("no lease held")]
    NoLease,
    /// Another participant (`by`) holds a live claim on these lines (amendment 13): a new
    /// overlapping claim, or an agent's edit under [`super::ClaimPolicy::Respect`]. Nothing
    /// was applied. Exit **5**, as `Leased`: someone else holds it for now; retry after
    /// `remaining_ms`, or wait ([`super::Session::register_waiter`]). `lines` are the
    /// claim's lines now (1-based, inclusive) in the root the call named.
    #[error("{} claimed by {name} ({by}){}; {remaining_ms} ms left",
        crate::session::awareness::pair_text(*.lines),
        .note.as_deref().map(|n| format!(" \"{}\"", crate::session::awareness::one_line(n))).unwrap_or_default())]
    Claimed {
        id: ClaimId,
        by: String,
        name: String,
        lines: (usize, usize),
        remaining_ms: u64,
        note: Option<String>,
    },
    /// [`SessionError::Claimed`] for a claim on elements ([`super::Session::claim_elems`]),
    /// which has no lines: the same id and exit. Boxed, to keep `SessionError` small.
    #[error("{} claimed by {} ({}){}; {} ms left",
        crate::session::awareness::elems_text(&.0.root, &.0.keys),
        .0.name,
        .0.by,
        .0.note.as_deref().map(|n| format!(" \"{}\"", crate::session::awareness::one_line(n))).unwrap_or_default(),
        .0.remaining_ms)]
    ElemsClaimed(Box<ElemsClaim>),
    /// Tier 0: another participant's live claim names some of the same index nodes
    /// ([`super::BlobSession::claim_nodes`]). The same id and exit as
    /// [`SessionError::Claimed`]: exit 5. Claims never refuse a write.
    #[error("{} claimed by {} ({}){}; {} ms left",
        crate::session::awareness::nodes_text(&.0.keys),
        .0.name,
        .0.by,
        .0.note.as_deref().map(|n| format!(" \"{}\"", crate::session::awareness::one_line(n))).unwrap_or_default(),
        .0.remaining_ms)]
    NodesClaimed(Box<NodesClaim>),
    /// No live claim by that id (it ended, or never existed here), or, with no id, the
    /// caller holds none. Exit 4.
    #[error("{}", match .0 { Some(id) => format!("no live claim {id}"), None => "you hold no claim here".to_owned() })]
    NoClaim(Option<ClaimId>),
    /// Renewing a claim, or extending it with `hold`, while other participants wait for it
    /// (anti-starvation): refused; the claim still ends normally. Exit 2.
    #[error("{id} has {waiters} waiting; it can't be renewed or held ({remaining_ms} ms left)")]
    ClaimContended {
        id: ClaimId,
        waiters: usize,
        remaining_ms: u64,
    },
    /// A claim call the kit refuses: not your claim, waiting on your own claim, a zero TTL,
    /// too many claims, several claims and no id, a note that is too long. Exit 2.
    #[error("bad claim: {0}")]
    BadClaim(String),
    /// An element selection with more keys than [`super::ELEM_SELECTION_MAX`], or a grid's
    /// cells with more ranges than [`crate::grid::CELLS_RANGES_MAX`]. Exit 2.
    #[error("selection too large: {len} elements or cell ranges (the most is {max})")]
    SelectionTooLarge { len: usize, max: usize },
    /// Tier 0: the blob store failed (or a blob is damaged; it is never served).
    #[error(transparent)]
    Blob(#[from] crate::blob::BlobError),
    /// Tier 0: the manager has no blob store ([`super::SessionManager::with_blob_store`]).
    #[error("no blob store configured")]
    NoBlobStore,
    /// The doc's type has no structure index. `MPC-E711`, exit 2.
    #[error("this file type has no structure index")]
    IndexUnsupported,
    /// A version has no index, and won't get one without a change (past a cap, unreadable, or
    /// purged). `MPC-E712`, exit 4.
    #[error("version {version} has no index: {why}")]
    NotIndexed {
        version: crate::store::VersionId,
        why: crate::index::NotIndexedWhy,
    },
    /// A version's index isn't ready yet (no job has run, or one is running). `MPC-E713`, exit
    /// 1: retry after `retry_after_ms`.
    #[error("the index of version {version} isn't ready; retry in {retry_after_ms} ms")]
    IndexPending {
        version: crate::store::VersionId,
        retry_after_ms: u64,
    },
    /// A structure-index read refused for an address or a cursor, with its own id, exit and
    /// details: `MPC-E170 ambiguous_address` (exit 2, `details.candidates`), `MPC-E173
    /// stale_cursor` (exit 1), `MPC-E174 bad_cursor` (exit 2), `MPC-E175 over_budget` (exit 2).
    /// Nothing was read. The message is the report's.
    #[error("{}", .0.message())]
    IndexRead(Box<crate::errors::ErrorReport>),
    /// An agent's `write` on a type whose pack refuses them ([`super::BlobPack::agent_writes`]).
    /// `MPC-E715`, exit 2.
    #[error("an agent can't write this file type")]
    AgentWriteRefused,
    /// A pack's verb that runs only under the caller's own lease (`merge: Lease`,
    /// [`super::BlobVerb`]) was called without it: take the lease (`ttl_ms`, the default), then
    /// call again. Nothing was written. `MPC-E718`, exit 2.
    #[error(
        "{verb} edits this file only under your own lease; take it first ({ttl_ms} ms by default)"
    )]
    NeedsLease { verb: String, ttl_ms: u64 },
    /// A structure diff spans an outside save whose writer the file type's pack says reissues
    /// node keys ([`super::SavePairing::Reissues`]): across it a key may name another thing, so
    /// the kit pairs nothing. The widest diff that doesn't cross a save like it is
    /// `safe_from` to `to`. `MPC-E722`, exit 1.
    #[error("version {save}'s writer reissues node keys: a diff across it pairs nothing")]
    KeysReissued {
        save: crate::store::VersionId,
        safe_from: crate::store::VersionId,
        to: crate::store::VersionId,
    },
    /// A tier-0 verb's `--base V` spans an outside save (another program's) whose writer the
    /// file type's pack doesn't prove keeps node keys ([`super::SavePairing`] `Unknown` or
    /// `Reissues`): across it a key may name another thing, so the call can't be paired with
    /// what was read at `base`. Nothing was written. `MPC-E719`, exit 1.
    #[error(
        "version {save} is another program's save, not proven to keep node keys: --base {base} \
         can't pair {} across it (head {head})",
        .addresses.join(" ")
    )]
    BaseAcrossUnprovenSave {
        save: crate::store::VersionId,
        base: crate::store::VersionId,
        head: crate::store::VersionId,
        addresses: Vec<String>,
    },
    /// A tier-0 verb's `--base V`: a node it changes was changed since `base` in a way that
    /// overlaps (`conflicts`, each with who changed it and in which version). Nothing was
    /// written. `MPC-E723`, exit 1.
    #[error(
        "{} changed since --base {base} (head {head}); nothing applied",
        .addresses.join(" ")
    )]
    BaseMoved {
        base: crate::store::VersionId,
        head: crate::store::VersionId,
        addresses: Vec<String>,
        conflicts: Vec<BaseConflict>,
    },
    /// A tier-0 verb's `--base V` would change nodes whose keys the kit minted (the file gives
    /// them no ID of its own), which can't be paired across versions for sure. Nothing was
    /// written. `MPC-E724`, exit 2.
    #[error(
        "{} have no key of the file's own: --base can't pair them; guard the call with \
         --if-version {head}",
        .keys.join(" ")
    )]
    BaseWeakKey {
        head: crate::store::VersionId,
        keys: Vec<String>,
    },
    /// A pack's verb returned a plan the kit refuses (its spans out of order, overlapping,
    /// outside the file or over a cap; or spans from a read verb): a pack bug. Nothing was
    /// written. `MPC-E725`, exit 2.
    #[error("the verb's plan is refused: {why}")]
    BadPlan { why: String },
    /// A pack's verb changed nodes its plan didn't declare (`strayed`, by key): a pack bug.
    /// Nothing was written. `MPC-E726`, exit 2.
    #[error("the verb's output changed nodes its plan didn't name: {}", .keys.join(", "))]
    Strayed { keys: Vec<String> },
    /// The file type's own index refuses the verb's output: a pack bug. Nothing was written.
    /// `MPC-E727`, exit 2.
    #[error("the file type's index refuses the verb's output")]
    OutputUnreadable { why: String },
    /// An agent's whole-file write under [`super::ClaimPolicy::Respect`], while others hold
    /// nodes, that the file type's index couldn't read within its limits (`why`): the kit can't
    /// tell which nodes it changes. `holders`: the participants holding nodes now. Nothing was
    /// written. `MPC-E306`, exit 5.
    #[error("the write can't be checked against the holds on this doc ({why})")]
    HoldUnindexable { why: String, holders: Vec<String> },
    /// A tier-0 revert of version `version` the file type can't make: it has no merge, or its
    /// merge can't take these bytes (`why`). Nothing was written.
    /// `MPC-E730`, exit 2: `restore` the version before it instead, which also replaces what
    /// changed since.
    #[error(
        "version {version} can't be reverted alone: {why} (version {} is the one before it)",
        .version.saturating_sub(1)
    )]
    RevertUnsupported {
        version: crate::store::VersionId,
        why: String,
    },
    /// A tier-0 revert of version `version` whose nodes were changed since in a way that
    /// overlaps (`conflicts`, each with who changed it and in which version), or that the file
    /// type's merge refuses. Nothing was written. `MPC-E731`, exit 1.
    #[error(
        "version {version}'s change to {} was changed since (head {head}); nothing reverted",
        .addresses.join(" ")
    )]
    RevertConflict {
        version: crate::store::VersionId,
        head: crate::store::VersionId,
        addresses: Vec<String>,
        conflicts: Vec<BaseConflict>,
    },
    /// A verb's call is over a bound (`what`: the version a whole-file verb reads, the spans of
    /// its plan...), named with the limit. Nothing was written. `MPC-E728`, exit 2.
    #[error("{what} is {size}, over the limit of {limit}")]
    VerbTooLarge {
        what: &'static str,
        size: u64,
        limit: u64,
    },
    /// `--expect DIGEST`: the bytes the call would store aren't the ones its dry run showed.
    /// Nothing was written. `MPC-E729`, exit 1.
    #[error("the bytes this call would store are not the ones previewed (expected {expected}, now {actual})")]
    NotAsPreviewed { expected: String, actual: String },
    /// No annotation thread `thread` in this doc (or no comment `comment` on it). `MPC-E320`,
    /// exit 4.
    #[error("{}", no_annotation_text(*thread, *comment))]
    NoAnnotation {
        thread: crate::annot::ThreadId,
        comment: Option<u32>,
    },
    /// A comment call the kit refuses: an empty body, a bad mention, attachment or hint, an
    /// anchor naming no node (or a key several nodes hold). Nothing changed. `MPC-E321`,
    /// exit 2.
    #[error("bad annotation: {0}")]
    BadAnnotation(String),
    /// Past an annotation cap (`what`: `body bytes`, `threads`, `comments`;
    /// [`super::Config::annot_body_max_bytes`] and the others). Nothing changed. `MPC-E907`,
    /// exit 2.
    #[error("annotation limit: at most {limit} {what}")]
    AnnotationCap { what: &'static str, limit: u64 },
    /// Deleting a comment that isn't the caller's: only its author, the person it was made
    /// for, or an owner may. `MPC-E117`, exit 3.
    #[error(
        "refused: only its author, the person it was made for, or an owner can delete comment \
         {comment} of {thread}"
    )]
    NotCommentAuthor {
        thread: crate::annot::ThreadId,
        comment: u32,
    },
    #[error(transparent)]
    Doc(#[from] DocError),
    /// A store error. A native doc's refusal on a build without the native engine
    /// ([`StoreError::NativeEngineMissing`]) is never this: it converts to
    /// [`SessionError::NativeEngineMissing`], so a host matches it as its own.
    #[error(transparent)]
    Store(StoreError),
    #[error(transparent)]
    Pack(#[from] PackError),
    /// A grid change would take the table past a size limit: nothing applied,
    /// and nothing already in the table is dropped. Exit 2. `what` is `cells`, `columns` or
    /// `rows` (blank rows included); or `value bytes` for a cell value or column name longer than
    /// `Config::grid_max_value_len` (`would_be` is its length). A keyed doc that isn't a table
    /// has its own, [`SessionError::KeyedAtLimit`] (before, this with `what` `elements and
    /// fields`, which still maps to `MPC-E822`).
    #[error("{}", at_limit(what, *limit, *would_be))]
    AtLimit {
        what: &'static str,
        limit: usize,
        /// What the change would have made it.
        would_be: usize,
    },
    /// A change would take a keyed doc that isn't a table (a deck, a board) past its live size
    /// limit: its elements and fields (its list elements, deleted ones included until dropped,
    /// plus its map entries). Nothing applied, and nothing already in the doc is dropped.
    /// `MPC-E822`, exit 2. `limit` is the one that applied to the doc's type and `from` where
    /// it came from ([`super::Config::keyed_limit`]); both are in the report's details
    /// (`limit`, `limit_source`).
    #[error("{}", at_limit("elements and fields", *limit, *would_be))]
    #[non_exhaustive]
    KeyedAtLimit {
        limit: usize,
        /// What the change would have made it.
        would_be: usize,
        from: super::LimitSource,
    },
    /// A live change the doc's file type refuses ([`super::Pack::check_edit`]): its
    /// write-back couldn't write it. Nothing applied and the version didn't move. `MPC-E823`
    /// `pack_refused`, exit 2; the pack's own code is in the report's details (`code`).
    #[error("the file type refuses this change: {0}")]
    PackRefused(PackRefusal),
    /// A whole-file write (`write --base V`, a table's `write --base` or `--seen V`) from a
    /// version whose whole read was given while the file couldn't be rendered
    /// ([`super::ReadResult::render_failed`]): that read's bytes were the file as last synced,
    /// not version V, so nothing written from them can be merged at V. Nothing applied.
    /// `MPC-E824` `base_unrendered`, exit 2.
    #[error(
        "version {base} was read while the file couldn't be written back: that read's bytes \
         are the file as last synced, not version {base}; nothing applied"
    )]
    BaseUnrendered { base: Seq },
    /// An insert into a root whose pack declares tagged keys ([`super::Pack::tagged_keys`])
    /// names a key that isn't `<prefix><n>.<tag>` with a declared prefix and the inserting
    /// connection's own writer tag `tag`. Nothing applied. Exit 2.
    #[error("insert {key} on {root} doesn't carry your writer tag {tag}")]
    KeyNotOwn {
        root: String,
        key: String,
        tag: String,
    },
    /// A connection's edit writes a value naming a blob ([`crate::blob::BlobId::parse_ref`], in
    /// a root of [`super::Pack::blob_ref_roots`]) that this doc's space doesn't hold. Nothing
    /// applied. Exit 4.
    #[error("no blob {blob} in this doc's space")]
    BlobRefUnknown {
        root: String,
        key: String,
        blob: String,
    },
    /// A line address (`LN`, `claim --lines`) on a file that can't be rendered now: it might hold
    /// a lone CR, so the line number has no checked meaning. Nothing applied. Exit 2.
    #[error(
        "{address}: the file can't be rendered now, so its line numbers can't be checked; name \
         the row by its key or #N"
    )]
    LinesUnknown { address: String },
    /// [`super::Session::mint_key`]: no key `<prefix><n>.<tag>` fits a list key's 24 bytes
    /// (or every number is taken). Nothing written. Exit 2.
    #[error("no key fits 24 bytes for prefix {prefix} and tag {tag}")]
    KeySpaceFull { prefix: String, tag: String },
    /// An agent's grid edit names no base: exit 2, "read first and pass --base".
    #[error("an agent's grid edit needs a base: read first and pass --base")]
    NeedsBase,
    /// `--base V`: a facet the edit touches was written after V by someone else (on
    /// this engine a write with no identity counts as someone else's). Nothing
    /// applied. Exit 1.
    #[error("{} changed since your base: {}", if .0.len() == 1 { "1 element" } else { "elements" }, .0.iter().map(|c| c.to_string()).collect::<Vec<_>>().join("; "))]
    FacetConflict(Vec<super::live::FacetConflict>),
    /// This session handle was unloaded from the pool; open the doc again.
    #[error("session unloaded; open the doc again")]
    Unloaded,
    /// The manager was released (process hand-off).
    #[error("sessions released")]
    Released,
    /// The doc was deleted ([`super::SessionManager::delete_doc`] / `delete_space`) while
    /// this session was loaded. Exit 4.
    #[error("the doc was deleted")]
    Deleted,
    /// The host closed this doc's live session ([`super::SessionManager::close`]), members or
    /// not; `reason` is the host's code (for example `core_off`). Nothing acknowledged was lost.
    /// Whether it opens again is the host's decision. Exit 1.
    #[error("the host closed this doc's live session ({reason})")]
    Closed { reason: &'static str },
    /// A store write failed and reloading from the store failed too. The session is closed;
    /// open the doc again once the store is healthy.
    #[error("session failed: {0}")]
    Broken(String),
}

pub type Result<T, E = SessionError> = std::result::Result<T, E>;

/// Error from [`super::Session::edit_with`]: the session refused or failed, or the caller's
/// own computation returned an error (nothing applied either way).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditWithError<E> {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("{0}")]
    Compute(E),
}

/// [`SessionError::NoAnnotation`]'s message.
fn no_annotation_text(thread: crate::annot::ThreadId, comment: Option<u32>) -> String {
    match comment {
        Some(n) => format!("no comment {n} on {thread}"),
        None => format!("no annotation {thread}"),
    }
}

/// [`SessionError::AtLimit`]'s message.
fn save_mismatch_text(size: u64, file_size: Option<u64>) -> String {
    let file = match file_size {
        Some(n) => format!("{n} bytes on disk"),
        None => "no file on disk".to_owned(),
    };
    format!("the file doesn't hold the bytes saved ({size} bytes given, {file}); nothing recorded")
}

fn at_limit(what: &str, limit: usize, would_be: usize) -> String {
    if what == "value bytes" {
        return format!("a value or name is {would_be} bytes; the limit is {limit} bytes");
    }
    if what == "elements and fields" {
        return format!(
            "this doc is at its live size limit ({limit} elements and fields, deleted elements \
             included); nothing was applied"
        );
    }
    format!("this table is at its size limit ({limit} {what}, deleted rows included); the owner can free space with `rows drop-deleted`")
}

impl SessionError {
    /// This error with every absolute path in its text made safe to show (v0.9.8):
    /// [`crate::errors::redact`] over `root` (inside it, relative; elsewhere, `<outside>`).
    /// The variant, and with it the id and exit code, doesn't change.
    pub fn redacted(self, root: &std::path::Path) -> SessionError {
        use crate::blob::BlobError;
        use crate::errors::redact as r;
        use crate::store::StoreError;
        let store = |e: StoreError| match e {
            StoreError::Db(m) => StoreError::Db(r(&m, root)),
            other => other,
        };
        match self {
            SessionError::BadPath(m) => SessionError::BadPath(r(&m, root)),
            SessionError::Disk(m) => SessionError::Disk(r(&m, root)),
            SessionError::Broken(m) => SessionError::Broken(r(&m, root)),
            SessionError::Declined(d) => SessionError::Declined(Declined {
                message: r(&d.message, root),
                ..d
            }),
            SessionError::Pack(p) => SessionError::Pack(PackError(r(&p.0, root))),
            SessionError::PackRefused(p) => SessionError::PackRefused(PackRefusal {
                message: r(&p.message, root),
                ..p
            }),
            SessionError::PairingMismatch { reason } => SessionError::PairingMismatch {
                reason: r(&reason, root),
            },
            SessionError::Store(e) => SessionError::Store(store(e)),
            SessionError::Blob(BlobError::Io(m)) => SessionError::Blob(BlobError::Io(r(&m, root))),
            SessionError::Blob(BlobError::Store(e)) => {
                SessionError::Blob(BlobError::Store(store(e)))
            }
            other => other,
        }
    }
}

impl From<StoreError> for SessionError {
    /// A store error as a session's: a native doc's refusal on a build without the native
    /// engine as its own variant, any other as [`SessionError::Store`].
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NativeEngineMissing {
                stored,
                stored_format,
            } => SessionError::NativeEngineMissing {
                stored,
                stored_format,
            },
            e => SessionError::Store(e),
        }
    }
}

impl From<super::pack::SpliceError> for SessionError {
    fn from(e: super::pack::SpliceError) -> Self {
        match e {
            super::pack::SpliceError::PairingMismatch(reason) => {
                SessionError::PairingMismatch { reason }
            }
            super::pack::SpliceError::Pack(p) => SessionError::Pack(p),
        }
    }
}

/// The `rows drop-deleted` that dropped a key ([`SessionError::UnknownElem`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedBy {
    pub change: Seq,
    pub by: String,
    pub at_ms: u64,
}

fn unknown_elem_text(root: &str, key: &str, dropped: Option<&DroppedBy>) -> String {
    match dropped {
        Some(d) => format!(
            "{root}: {key} was dropped (rows drop-deleted by {} in change {}); a dropped key \
             never comes back; nothing applied",
            d.by, d.change
        ),
        None => format!(
            "{root}: {key} is not in this table (never added, or dropped before the kept \
             history); nothing applied"
        ),
    }
}
