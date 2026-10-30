//! One loaded doc and the people and agents in it.

use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};

mod at_base;
mod blame;
mod claims;
mod group;
mod history;
mod purge;
mod removed;
mod tier0_history;

pub use at_base::EditAtBase;
#[cfg(feature = "conformance")]
pub use blame::CharBlame;
pub use blame::{Blame, BlameLine};
pub use history::{DiffOutput, LogItem, LogQuery};
pub use purge::{NotReached, PurgeOutcome, PurgeRequest, PurgeScope};

use super::awareness::{self, ActivityEntry, ActivityKind, Aware, ClaimPolicy, Notes};
use super::carry::RevertIndex;
use super::config::Config;
use super::disk::{self, DiskCheck, DiskState, FlushOutcome, Probe, ReadFile};
use super::error::{EditWithError, Result, SessionError};
use super::handles::Handles;
use super::host::{Action, AuditEvent, Host, LeaveReason, Need, PurgedScope, Rights};
use super::pack::{Declined, Pack, PackError};
use super::participant::{
    ConnId, CursorAt, Participant, ParticipantKind, RosterEntry, Stay, EXTERNAL_PID, IMPORT_PID,
    RESERVED_PID_PREFIX, UNKNOWN_PID,
};
use super::position::{self, At, Bias, Cursor, LineRange};
use super::presence::{self, Presence};
use super::prov::{coalesce_with, ElemId, Ids, Inserted, Prov};
use super::ratelimit::Bucket;
use super::roster::{self, RosterEvent, RosterHub, Who};
use super::summary;
use super::undo::{self, Batch, ConflictRegion, Mode, Plan, Region};
use crate::doc::{ClientId, Doc, DocError, Edit, Neutral, MAX_CLIENT_ID};
use crate::engine::{ENGINE_FORMAT, ENGINE_NAME};
use crate::hash::{file_hash, hex, hmac, legacy_matches};
use crate::schema::{Kind, Schema};
use crate::store::{
    Approval, ApprovalState, ChangeInfo, ChangeKind, ClockSpan, DocId, DocKey, DocMeta, DocStore,
    HunkRoots, LogEntry, NewChange, ProvRow, RecordHunk, RecordRoot, RevertRecord, RootRuns, Seq,
    SyncState, Version, VersionReason,
};
use crate::text::diff::{self, Hunk};
use crate::text::lines;
use crate::text::merge::{self, Conflict, Merge};

/// Returned by [`Session::join`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Joined {
    /// The connection: pass it to every call.
    pub conn: ConnId,
    /// This connection's client ID in this doc. No two present connections share one. A
    /// new connection gets a fresh one, except an agent's, which takes back a client ID it
    /// used before when no present connection holds it (v0.9.7,
    /// [`Config::reuse_agent_clients`]). Every change made through it is attributed to it,
    /// and the store maps it to the pid.
    pub client: ClientId,
    pub version: Seq,
}

/// A file's bytes at a version, for exports ([`Session::file_at`], [`Session::file_now`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileBytes {
    /// The file's bytes: the pack's rendering, as a write-back writes it.
    pub bytes: Vec<u8>,
    /// The doc version they are (the number `log` and `show` give): for a stored version, the
    /// log position it reflects. Name an export after this, never an internal version id.
    pub version: Seq,
    /// How the codec settings (line endings, BOM, final newline) were applied.
    pub codec: FileCodec,
}

/// What [`Session::file_at_to`] / [`Session::file_now_to`] and
/// [`super::BlobSession::file_at_to`] / [`super::BlobSession::file_now_to`] copied.
///
/// **Only an `Ok` means the copy is good.** On an error the caller must discard everything
/// already written to the writer: for a tier-0 file the bytes are verified as they stream,
/// and the check (the blob's HMAC and size) completes only at the clean end, so a damaged
/// blob's bytes may have reached the writer before the error. Copy to a temporary file and
/// rename it into place only after `Ok`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileCopy {
    /// The version copied: a live doc's doc version ([`FileBytes::version`]); a tier-0 file's
    /// version id (the number its `log` shows).
    pub version: u64,
    /// Bytes written.
    pub bytes: u64,
    /// How the codec was applied (tier 0: `Exact`, the stored bytes themselves).
    pub codec: FileCodec,
}

/// How [`FileBytes`] applied the codec settings. Versions store content only, not the
/// codec settings in effect then, so a past version is rendered with today's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileCodec {
    /// The current doc, rendered as the write-back writes it: exact.
    Exact,
    /// Rendered with today's line endings, BOM and final newline: exact unless an outside
    /// save changed them since that version.
    Current,
    /// Rendered with today's settings, and the doc's log shows a settings change since that
    /// version: the bytes differ from the file then in those settings (warn). Not returned
    /// by this build (it records no settings changes in the log).
    ChangedSince,
}

/// Returned by [`Session::read`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadResult {
    /// The bytes the file would hold now (the pack's encoding: BOM and line endings restored).
    pub bytes: Vec<u8>,
    /// Each text root's text, in schema order.
    pub text_by_root: Vec<(String, String)>,
    /// The version read. A valid `base` for [`Session::write_base`] while it stays cached.
    pub version: Seq,
    /// Who else changed the doc since this participant's previous read (or, on a first read,
    /// since the session loaded), oldest first. Excludes the caller and the import.
    pub contributors_since_last_read: Vec<String>,
}

/// Returned by an applied edit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct EditResult {
    /// The version after the edit (unchanged when the edit changed nothing).
    pub version: Seq,
    /// Lines of the new text the edit touched (empty when nothing changed).
    pub changed: Vec<LineRange>,
    pub lines_added: usize,
    pub lines_removed: usize,
}

/// Returned by [`Session::write_base`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    Applied(EditResult),
    /// The write overlaps changes made since the base. Nothing was applied.
    Conflicts(Vec<Conflict>),
    /// The base version is unknown or no longer cached: re-read and retry.
    BaseExpired,
}

impl WriteOutcome {
    /// The exit code (K-EXIT-1): 0 applied, 1 conflict, 8 base expired (never retry).
    pub fn exit(&self) -> u8 {
        match self {
            WriteOutcome::Applied(_) => crate::tools::exit::OK,
            WriteOutcome::Conflicts(_) => crate::tools::exit::NO,
            WriteOutcome::BaseExpired => crate::tools::exit::TOO_OLD,
        }
    }
}

/// The host's choices for one [`Session::edit_with_options`] (or
/// [`Session::edit_at_base`]) call: the claim policy and the caller's version guard, both
/// checked under the session lock (as [`crate::tools::ToolOptions`] and `--if-version` are
/// for `run_tool_with`), and a person's undo group token ([`EditOptions::group`]).
/// `#[non_exhaustive]`: start from [`EditOptions::default`] (or `new`) and use the setters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct EditOptions {
    /// What an agent's edit does about others' claims (default [`ClaimPolicy::Ignore`]).
    pub policy: ClaimPolicy,
    /// Apply only if the doc is still at this version (default `None`: no guard).
    pub if_version: Option<Seq>,
    /// The undo group token's hash ([`EditOptions::group`]; default `None`: its own step).
    pub(crate) group: Option<u64>,
}

impl EditOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// The claim policy ([`ClaimPolicy::Respect`]: an agent's edit touching another
    /// participant's live claim is refused).
    pub fn policy(mut self, policy: ClaimPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Apply only if the doc is at version `v` ([`SessionError::Stale`] otherwise).
    pub fn if_version(mut self, v: Seq) -> Self {
        self.if_version = Some(v);
        self
    }

    /// **Undo grouping** (undo spec U3, K4): consecutive edits from one connection with the
    /// same `token` merge into **one undo step**. The editor proposes the token (the JS
    /// client sends one per edit, a new one when the kind of edit changes); the session
    /// closes the group, so the next edit starts a new step, on:
    /// - **idle**: the group's last edit is [`Config::undo_group_idle_ms`] old or more on the
    ///   host clock (U3 (a));
    /// - a **non-contiguous** edit: one not overlapping or touching the group's span, or of
    ///   more than one hunk (U3 (b));
    /// - a **word boundary**: a space, tab or punctuation typed after a word character
    ///   starts the next step (U3 (c));
    /// - a **newline** typed: a step of its own (U3 (d));
    /// - **another token**, an edit without one, or any other change by the same person
    ///   (another connection, a verb, a write, a revert), and any **undo or redo** by them
    ///   (U3 (e), (f));
    /// - **anyone else's change** landing inside the group's span or touching it, in the
    ///   order the session applies changes (U3 (g)).
    ///
    /// Agents never group (one verb = one step): the token is ignored for them.
    pub fn group(mut self, token: &str) -> Self {
        use std::hash::{DefaultHasher, Hash};
        let mut h = DefaultHasher::new();
        token.hash(&mut h);
        self.group = Some(h.finish());
        self
    }
}

/// Returned by [`Session::write_base_with`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WriteReport {
    pub outcome: WriteOutcome,
    /// For [`WriteOutcome::Conflicts`]: who claimed, or last edited, each conflicting
    /// region, as `(index into the conflicts, note)` with kind `Claimed` or `Edited` (who,
    /// their lines now, age, claim note). Not deduped or rate-limited: it is the conflict's
    /// own report ("lines 40–60 are claimed by agent-b, 12 s ago"). Editors are those still
    /// present with their latest change there.
    pub conflict_notes: Vec<(usize, awareness::Note)>,
    /// For an applied write: heads-up notes on the lines it changed.
    pub notes: Notes,
}

impl WriteReport {
    fn of(outcome: WriteOutcome) -> Self {
        WriteReport {
            outcome,
            conflict_notes: Vec::new(),
            notes: Notes::default(),
        }
    }
}

/// What a read verb showed ([`Session::read_with`]).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Shown {
    /// The viewing range to set (P4).
    pub view: Option<(usize, usize)>,
    /// The lines it looked at (notes, activity, `--for-edit`).
    pub touched: Option<(usize, usize)>,
}

/// The activity ring's verb for a change made through a direct API call.
fn kind_verb(k: ChangeKind) -> &'static str {
    match k {
        ChangeKind::Edit => "edit",
        ChangeKind::Undo => "undo",
        ChangeKind::Redo => "redo",
        ChangeKind::Restore => "restore",
        ChangeKind::Revert => "revert",
        ChangeKind::External => "external",
        ChangeKind::Import => "import",
    }
}

/// The span of an edit's changed lines in its first root (`first..=last` over them).
fn changed_span(changed: &[LineRange]) -> Option<LineRange> {
    let root = &changed.first()?.root;
    let mine = changed.iter().filter(|r| r.root == *root);
    Some(LineRange {
        root: root.clone(),
        first: mine.clone().map(|r| r.first).min()?,
        last: mine.map(|r| r.last).max()?,
    })
}

/// How an undo or redo treats conflicts (undo spec U13, U14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum UndoMode {
    /// All or nothing (`undo`): any conflicting region applies nothing (U13).
    #[default]
    All,
    /// `--partial`: apply every region that merges cleanly, report the rest; a step whose
    /// every region conflicts stays on top (U14). A person's Ctrl+Z.
    Partial,
    /// `--drop`: remove the top step without applying it (the escape from a blocked step).
    Drop,
}

/// What [`Session::undo_with`] / [`Session::redo_with`] do.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UndoRequest {
    /// Steps (batches), newest first. Default 1.
    pub steps: usize,
    pub mode: UndoMode,
    /// `--for <person>`: only batches made on that person's behalf (an assistant, U2).
    pub for_person: Option<String>,
}

impl Default for UndoRequest {
    fn default() -> Self {
        Self {
            steps: 1,
            mode: UndoMode::All,
            for_person: None,
        }
    }
}

impl UndoRequest {
    pub fn steps(steps: usize) -> Self {
        Self {
            steps,
            ..Self::default()
        }
    }
    pub fn mode(self, mode: UndoMode) -> Self {
        Self { mode, ..self }
    }
    pub fn for_person(self, pid: impl Into<String>) -> Self {
        Self {
            for_person: Some(pid.into()),
            ..self
        }
    }
}

/// Returned by [`Session::undo`], [`Session::redo`] (and their `_with` forms) and
/// [`Session::revert`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UndoOutcome {
    /// `steps` batches were undone (fewer than asked when the stack ran out or, with
    /// `--partial`, when a step was blocked). `skipped`: conflicting regions `--partial`
    /// left alone. `blocked`: with `--partial`, the next step's regions, all conflicting;
    /// that step stays on top.
    Applied {
        steps: usize,
        result: EditResult,
        skipped: Vec<ConflictRegion>,
        blocked: Option<Vec<ConflictRegion>>,
    },
    /// All or nothing: step `step` (1-based) conflicts; every conflicting region of it, in
    /// live coordinates (U11). Nothing was applied.
    Conflicts {
        step: usize,
        conflicts: Vec<ConflictRegion>,
    },
    /// `--partial`: the top step's every region conflicts. Nothing was applied and it stays
    /// on top; `--drop` removes it.
    Blocked { conflicts: Vec<ConflictRegion> },
    /// `--drop` removed the top step (change `change`) without applying it.
    Dropped { change: Seq },
    /// Nothing to undo (or redo).
    Nothing,
    /// Nothing to undo: this participant's history was lost when the session unloaded or
    /// the daemon restarted (U6, U18). `revert` still reaches their changes.
    HistoryLost,
    /// Nothing to undo: a purge cleared every stack of this doc (U19).
    ClearedByPurge,
}

impl UndoOutcome {
    /// The exit code (K-EXIT-1): 1 when anything conflicted or was skipped, else 0.
    pub fn exit(&self) -> u8 {
        match self {
            UndoOutcome::Applied {
                skipped, blocked, ..
            } if skipped.is_empty() && blocked.is_none() => crate::tools::exit::OK,
            UndoOutcome::Applied { .. }
            | UndoOutcome::Conflicts { .. }
            | UndoOutcome::Blocked { .. } => crate::tools::exit::NO,
            _ => crate::tools::exit::OK,
        }
    }
}

/// One applied change to one root, sent to subscribers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChangeEvent {
    /// Version after the change. Versions are consecutive, so a gap means a missed event.
    pub version: Seq,
    pub pid: String,
    pub client: ClientId,
    pub via: String,
    pub root: String,
    /// The change, in the coordinates of the root's text before it.
    pub hunks: Vec<Hunk>,
    pub at_ms: u64,
    /// The person an assistant agent made this change for (its approval starts `Pending`).
    pub on_behalf_of: Option<String>,
}

/// Returned by [`Session::status`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Status {
    pub key: DocKey,
    pub version: Seq,
    /// Distinct participants (pids) present.
    pub participants: usize,
    /// Open connections (a pid may hold several).
    pub connections: usize,
    /// Log entries since the last snapshot.
    pub log_entries: u64,
    /// Memory estimate (state size times [`Config::memory_factor`]).
    pub estimated_bytes: u64,
    /// Why live editing is off, when it is (edits are then refused).
    pub declined: Option<String>,
    /// Where the file stands, for a session opened with
    /// [`super::SessionManager::open_file`] (`None` without a file).
    pub disk: Option<DiskState>,
}

/// What [`Session::audit_disk_sync`] found (K-AUD-1, v0.9.4): the doc reports clean, yet
/// the disk replica of one of its roots is not the doc's. Metadata only.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SyncDrift {
    /// The replica's text is not the doc's.
    Text { root: String },
    /// The same text on other elements (deleted from the doc, or never there): the stale
    /// base that corrupted an outside edit before v0.9.5.
    Elements { root: String },
    /// The doc (`replica: false`) or the disk replica holds an update it couldn't integrate
    /// (missing dependencies) or is poisoned (v0.9.5). Checked whatever the disk state.
    Pending { replica: bool },
}

/// A participant whose rights changed in [`Session::recheck_rights`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RightsChange {
    pub pid: String,
    /// The new rights. Without read, the participant was removed.
    pub rights: Rights,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Edit,
    Undo,
    Redo,
}

/// One connection: the shared presence core (it derefs to it) plus the text's own.
struct Member {
    pres: Presence,
    client: ClientId,
    cursor: Option<Cursor>,
    /// Viewing range as a byte span of its root (carried: expanding), and when it was set.
    view: Option<(String, usize, usize)>,
    view_ms: Option<u64>,
    /// The span its latest change touched, in the root's text now (carried: not expanding;
    /// gone once its text is all removed or replaced, `awareness::carry_edit_span`).
    edited: Option<(String, usize, usize)>,
}

impl std::ops::Deref for Member {
    type Target = Presence;
    fn deref(&self) -> &Presence {
        &self.pres
    }
}

impl std::ops::DerefMut for Member {
    fn deref_mut(&mut self) -> &mut Presence {
        &mut self.pres
    }
}

/// Per-participant state that outlives presence (until the session unloads).
#[derive(Default)]
struct History {
    undo: VecDeque<Batch>,
    redo: VecDeque<Batch>,
    /// Bytes held by both stacks (bounded by [`Config::undo_max_bytes`]), as of the last
    /// push or removal: regions grow as they carry later hunks (theirs), so it is counted
    /// afresh each time.
    bytes: usize,
    /// A purge cleared the stacks; said once on the next undo or redo.
    cleared: bool,
    bucket: Option<Bucket>,
    last_read: Option<Seq>,
    /// The person's open undo group (U3, K4): the top of `undo` while edits may still join
    /// it. Any other push, and any undo or redo, closes it.
    open: Option<OpenGroup>,
}

/// An open undo group (U3, K4): whose edits may join it, and its newest change.
struct OpenGroup {
    conn: ConnId,
    token: u64,
    /// The host time of its last edit (U3 (a)).
    last_ms: u64,
    /// Its batch's change (the top of the undo stack while it is open).
    seq: Seq,
}

impl History {
    /// Bytes both stacks hold now.
    fn held_bytes(&self) -> usize {
        self.undo
            .iter()
            .chain(self.redo.iter())
            .map(Batch::bytes)
            .sum()
    }
}

struct ReadBase {
    version: Seq,
    at_ms: u64,
    texts: Vec<(&'static str, String)>,
    bytes: usize,
}

struct State {
    key: DocKey,
    store: Option<Arc<DocStore>>,
    closed: Option<SessionError>,
    doc: Doc,
    /// Each root's current text, in schema order (kept in step with `doc`).
    texts: Vec<(&'static str, String)>,
    codec: Vec<u8>,
    declined: Option<String>,
    version: Seq,
    log_entries: u64,
    log_bytes: u64,
    state_bytes: u64,
    members: Vec<Member>,
    joins: u64,
    /// Each client ID's owner (pid), from the store's client table: read in one place
    /// ([`Self::owners`]) for D1 (whose text a change touches, v0.9.7) and for agent client-ID
    /// reuse ([`Config::reuse_agent_clients`], v0.9.7). Built on first need, kept as clients
    /// join; `None` after a purge, which renames owners.
    owners: Option<HashMap<ClientId, String>>,
    /// Client IDs a participant with its own replica held in this load: never reused.
    replica_clients: std::collections::HashSet<ClientId>,
    hist: HashMap<String, History>,
    /// The version the session loaded (or reloaded) at: undo history before it was lost.
    loaded_seq: Seq,
    /// The newest stored version's log position, and when the doc last changed.
    last_version_seq: Seq,
    last_change_ms: u64,
    /// When the next retention pass is due ([`Config::history_tick_ms`]).
    next_history_ms: u64,
    /// Carried bounds of the revert records; built on first use (`None`: not built).
    reverts: Option<RevertIndex>,
    /// The provenance map (undo spec U23): every live element's ID and root, and the live
    /// restored elements by root. Built from the engine and the store on first need (`None`:
    /// not built, or dropped by a path that changed the text without keeping it exact).
    prov: Option<Prov>,
    /// Blames served since load: one in [`Config::blame_check_every`] is checked against a
    /// rebuild of `prov` (v0.9.5).
    blames: u64,
    /// Of those, the ones sampled for the check.
    blame_checks: u64,
    read_bases: VecDeque<ReadBase>,
    read_base_bytes: usize,
    /// Outside saves ingested, for the sampled identity check ([`Config::edit_check_every`]).
    outside_saves: u64,
    /// Recent change authors, for "contributors since last read".
    authors: VecDeque<(Seq, String)>,
    /// Change subscribers: the connection, an App viewer's pid (its events go through its
    /// handles), the queue.
    subs: Vec<(ConnId, Option<String>, SyncSender<ChangeEvent>)>,
    /// Every App viewer's handles in this doc (K5).
    handles: Handles,
    /// Roster subscribers and what they were last told ([`Session::subscribe_roster`]).
    roster: RosterHub,
    disk: Option<Disk>,
    /// The doc's hash key (in the store; a purge replaces it): every file hash is keyed by
    /// it, so none is a plain content hash, and a purge makes the old ones meaningless.
    hash_key: [u8; 32],
    /// Claims, note dedupe, the activity ring (amendment 13). Memory only.
    aware: Aware,
    /// The verb the change being committed came from (`sed -i`), for the activity ring.
    verb_hint: Option<String>,
    /// The undo group token of the edit being committed ([`EditOptions::group`]).
    group_hint: Option<u64>,
}

const AUTHORS_KEPT: usize = 1024;

/// `hunks` (on `text`) in the form the change at element identity takes: touching hunks
/// joined (an element kept between two changes is what separates them), and no hunk that
/// changes nothing, whether empty or a replace of text by the same text (which would only
/// delete elements and type them again). The outside save's merge is made canonical before
/// the engine applies it (v0.9.7), so the hunks it is handed are exactly its change at
/// identity.
fn canonical_hunks(text: &str, hunks: &[Hunk]) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::with_capacity(hunks.len());
    for h in hunks {
        if h.start == h.end && h.insert.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.end == h.start => {
                last.end = h.end;
                last.insert.push_str(&h.insert);
            }
            _ => out.push(h.clone()),
        }
    }
    // joining first: a deletion and the same text typed again beside it are one such hunk
    out.retain(|h| text.get(h.start..h.end) != Some(h.insert.as_str()));
    out
}

/// A watcher's rights (K11): read at most, whatever the host grants.
fn watched(rights: Rights, watcher: bool) -> Rights {
    match (watcher, rights.read) {
        (false, _) => rights,
        (true, true) => Rights::read_only(),
        (true, false) => Rights::none(),
    }
}
/// Hashes of the kit's own recent writes kept for echo suppression (and at most
/// [`Config::own_write_ttl_ms`] old).
const OWN_WRITES_KEPT: usize = 8;

/// Disk sync for a session opened with a file.
struct Disk {
    /// As the host gave it (symlinks are resolved on every read and write).
    path: PathBuf,
    /// Canonical; the file must resolve inside it.
    root: PathBuf,
    /// The doc as the file holds it: its text is the file's text, the base an outside save
    /// is three-way merged onto the live text from (v0.9.5: its text only, never its
    /// elements; [`Session::ingest`]). Whenever the doc renders to the synced bytes, it is
    /// the doc itself (same elements, not only the same text): [`Session::resync_replica`]
    /// (v0.9.4), which K-AUD-1 checks. After a save the doc holds more than, it holds a
    /// client the doc never has ([`Session::replica_to_file`]).
    replica: Doc,
    replica_bytes: u64,
    /// The keyed hash of the file's bytes as last synced (empty: unknown, after a purge).
    synced_hash: Vec<u8>,
    synced_seq: Seq,
    /// Recent own writes (keyed hash, when written): a file holding one of them is an echo,
    /// never an outside edit. In memory only; expire after [`Config::own_write_ttl_ms`];
    /// cleared by a purge (the key changes) and by every outside save taken in (v0.9.5: a
    /// write of ours from before a person's save, back again, is that person's save).
    own: VecDeque<(Vec<u8>, u64)>,
    /// The echo last reported (`AuditEvent::EchoSuppressed`), so a write-back's own check of
    /// the same file doesn't report it again; cleared by a write or an outside save.
    echoed: Option<Vec<u8>>,
    /// The file as last seen (`None`: read it on the next check), with git's racy mark.
    stat: Option<disk::Seen>,
    missing: bool,
    /// The first change not yet written.
    dirty_since: Option<u64>,
    last_change_ms: u64,
    last_check_ms: u64,
    external_client: Option<ClientId>,
    /// Every update the live doc took since `replica` was last the live doc's state, in
    /// order (v0.9.7): the next write-back applies them to the replica instead of reloading
    /// it from a whole encode of the doc. `None`: not known (after a load that left the
    /// replica behind, a recovery, a purge); the next write-back reloads it as before.
    /// Holds text: a purge clears it.
    catch_up: Option<Vec<Vec<u8>>>,
    /// Write-backs so far, and outside saves that made the replica the doc (the sampled
    /// check's count).
    writebacks: u64,
    /// Times the replica was brought forward by the catch-up list and passed the check (no
    /// reload), for the tests.
    caught_up: u64,
}

/// Tests only: (the live doc's state, the disk replica's, whether it is in step, the stored
/// synced state).
#[cfg(test)]
pub(crate) type WritebackState = (Vec<u8>, Option<Vec<u8>>, bool, Option<Vec<u8>>);

/// One loaded doc: its live text, the participants in it, their carets, undo history and
/// rate limits, and the subscribers to its changes. Thread-safe (`Send + Sync`): every call
/// takes the session's lock for its duration. Get one from
/// [`super::SessionManager::open`].
///
/// Every call names a connection ([`ConnId`], from [`Session::join`]). A participant (pid)
/// may hold several connections, each with its own client ID. Rights are per pid and asked
/// of the [`Host`] on every call: read for all of them, edit again on every edit. A
/// participant who has lost read is removed, with all their connections, on the spot. Undo
/// history and the edit rate limit are per pid too.
pub struct Session {
    host: Arc<dyn Host>,
    config: Arc<Config>,
    pack: Arc<dyn Pack>,
    schema: Schema,
    doc_id: DocId,
    state: Mutex<State>,
    last_used_ms: AtomicU64,
    /// The manager's blob store, for a doc promoted from tier 0 that kept tier-0 history
    /// (v0.9.2; `tier0_history.rs`). Set by the manager when it has one.
    blobs: std::sync::OnceLock<Arc<crate::blob::BlobStore>>,
}

fn random_client() -> ClientId {
    #[cfg(any(test, feature = "differential"))]
    if let Some(c) = crate::differential::next_client() {
        return c;
    }
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    h.write_u128(nanos);
    h.finish() & MAX_CLIENT_ID
}

/// A fresh client ID: random 53 bits, never 0 or 1, never one this doc has used.
pub(super) fn fresh_client(
    store: &DocStore,
    id: DocId,
    taken: impl Fn(ClientId) -> bool,
) -> Result<ClientId> {
    loop {
        let c = random_client();
        if c > 1 && !taken(c) && store.client_owner(id, c)?.is_none() {
            return Ok(c);
        }
    }
}

/// D1 for one root (v1.10): `hunks` widened to the tokens they touch, keeping inside only
/// the characters whose provenance root `pid` wrote ([`crate::text::lines::authored_hunks`]).
/// Ownership is read off the provenance map over the lines the hunks touch (a token never
/// spans a line break); an unknown client or the import's is someone else's. `extra`: live
/// ranges the test-only `rewrite-merged-ranges` mutant also takes.
#[allow(clippy::too_many_arguments)]
fn d1_hunks(
    text: &str,
    hunks: &[Hunk],
    tokens: crate::text::lines::Tokens,
    prov: &Prov,
    root: &str,
    owners: &HashMap<ClientId, String>,
    pid: &str,
    extra: &[(usize, usize)],
) -> Vec<Hunk> {
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::d1_mutant() == crate::differential::D1Mutant::ReuseForeign {
        return hunks.to_vec();
    }
    let mut hs: Vec<Hunk> = hunks.to_vec();
    for &(lo, hi) in extra {
        if lo < hi && hi <= text.len() && !hs.iter().any(|h| h.start < hi && lo < h.end) {
            hs.push(Hunk {
                start: lo,
                end: hi,
                insert: text[lo..hi].to_owned(),
            });
        }
    }
    hs.sort_by_key(|h| (h.start, h.end));
    let hs = diff::coalesce(hs);
    let (Some(first), Some(last)) = (hs.first(), hs.last()) else {
        return hs;
    };
    // the lines the hunks touch: no token reaches past them
    let lo = text[..first.start].rfind('\n').map_or(0, |k| k + 1);
    let hi = text[last.end..]
        .find('\n')
        .map_or(text.len(), |k| last.end + k + 1);
    let mut runs: Vec<(usize, usize, bool)> = Vec::new();
    let mut at = lo;
    for r in prov.query(root, text, lo, hi) {
        let owner = owners.get(&r.root.unwrap_or(r.id).client);
        let end = at + r.bytes as usize;
        runs.push((at, end, owner.is_some_and(|o| o == pid)));
        at = end;
    }
    let owned = |b: usize| -> bool {
        let k = runs.partition_point(|r| r.1 <= b);
        runs.get(k).is_some_and(|r| r.0 <= b && r.2)
    };
    crate::text::lines::authored_hunks(text, &hs, tokens, &owned)
}

/// The test-only `rewrite-merged-ranges` mutant's extra ranges, set by `write --base`.
#[cfg(any(test, feature = "differential"))]
fn take_d1_extra() -> Vec<(usize, usize)> {
    crate::differential::take_d1_extra()
}

#[cfg(not(any(test, feature = "differential")))]
fn take_d1_extra() -> Vec<(usize, usize)> {
    Vec::new()
}

fn texts_of(doc: &Doc, schema: &Schema) -> Result<Vec<(&'static str, String)>> {
    schema
        .roots
        .iter()
        .map(|(r, _)| Ok((*r, doc.text(r)?)))
        .collect()
}

/// An engine error after which the in-memory doc can't be trusted (see
/// [`crate::doc::DocError::Pending`]): the session reloads from the store.
fn poisons(e: &SessionError) -> bool {
    matches!(
        e,
        SessionError::Doc(DocError::Pending | DocError::Panic | DocError::Apply(_))
    )
}

/// A pack's decoded roots in schema order: every root exactly once, of the right kind.
pub(super) fn ordered_roots(
    schema: &Schema,
    roots: Vec<(String, Neutral)>,
) -> Result<Vec<(String, Neutral)>> {
    if roots.len() != schema.roots.len() {
        return Err(PackError(format!(
            "decode gave {} roots; schema {:?} has {}",
            roots.len(),
            schema.name,
            schema.roots.len()
        ))
        .into());
    }
    let mut ordered = Vec::with_capacity(schema.roots.len());
    for (name, kind) in schema.roots {
        let found: Vec<&Neutral> = roots
            .iter()
            .filter(|(r, _)| r == name)
            .map(|(_, v)| v)
            .collect();
        match (found.as_slice(), kind) {
            ([v @ Neutral::Text(_)], Kind::Text) => {
                ordered.push(((*name).to_owned(), (*v).clone()))
            }
            _ => {
                return Err(
                    PackError(format!("decode must give root {name:?} once, as {kind:?}")).into(),
                )
            }
        }
    }
    Ok(ordered)
}

fn neutral(texts: &[(&'static str, String)]) -> Vec<(String, Neutral)> {
    texts
        .iter()
        .map(|(r, t)| ((*r).to_owned(), Neutral::Text(t.clone())))
        .collect()
}

impl Session {
    // ----- loading (crate-private; see SessionManager) -----

    fn build(
        host: Arc<dyn Host>,
        config: Arc<Config>,
        pack: Arc<dyn Pack>,
        store: Arc<DocStore>,
        init: Init,
    ) -> Result<Self> {
        let Init {
            doc_id,
            meta,
            doc,
            version: loaded_version,
            log,
            state_bytes,
            authors,
            now,
        } = init;
        let schema = pack.schema();
        let texts = texts_of(&doc, &schema)?;
        let last_version_seq = store.last_version(doc_id)?.map_or(0, |(_, v)| v.seq);
        let hash_key = store.doc_key(doc_id)?;
        Ok(Self {
            host,
            config,
            pack,
            schema,
            doc_id,
            last_used_ms: AtomicU64::new(now),
            blobs: std::sync::OnceLock::new(),
            state: Mutex::new(State {
                key: meta.key,
                store: Some(store),
                closed: None,
                doc,
                texts,
                codec: meta.codec,
                declined: meta.declined,
                version: loaded_version,
                log_entries: log.0,
                log_bytes: log.1,
                state_bytes,
                members: Vec::new(),
                joins: 0,
                owners: None,
                replica_clients: std::collections::HashSet::new(),
                hist: HashMap::new(),
                loaded_seq: loaded_version,
                last_version_seq,
                last_change_ms: now,
                next_history_ms: now,
                reverts: None,
                prov: None,
                blames: 0,
                blame_checks: 0,
                read_bases: VecDeque::new(),
                read_base_bytes: 0,
                authors,
                outside_saves: 0,
                subs: Vec::new(),
                handles: Handles::default(),
                roster: RosterHub::default(),
                disk: None,
                hash_key,
                aware: Aware::default(),
                verb_hint: None,
                group_hint: None,
            }),
        })
    }

    /// Load a stored doc.
    pub(crate) fn load(
        host: Arc<dyn Host>,
        config: Arc<Config>,
        pack: Arc<dyn Pack>,
        store: Arc<DocStore>,
        doc_id: DocId,
    ) -> Result<Self> {
        let schema = pack.schema();
        let meta = store.meta(doc_id)?;
        if meta.pack != schema.name {
            return Err(SessionError::PackMismatch {
                stored: meta.pack,
                pack: schema.name.to_owned(),
            });
        }
        let loaded = store.load(doc_id)?;
        let mut doc = Doc::from_store(schema, &loaded)?;
        doc.set_edit_check_every(config.edit_check_every);
        doc.set_edit_check_scale(config.edit_check_scale_units);
        doc.set_position_check_every(config.position_check_every);
        let snap_seq = loaded.snapshot.as_ref().map_or(0, |(s, _)| *s);
        let version = loaded.updates.last().map_or(snap_seq, |(s, _)| *s);
        let log_bytes: u64 = loaded
            .updates
            .iter()
            .map(|(_, e)| e.update.len() as u64)
            .sum();
        let state_bytes = loaded.snapshot.as_ref().map_or(0, |(_, s)| s.len() as u64) + log_bytes;
        let mut authors: VecDeque<(Seq, String)> = loaded
            .updates
            .iter()
            .map(|(s, e)| (*s, e.author.clone()))
            .collect();
        while authors.len() > AUTHORS_KEPT {
            authors.pop_front();
        }
        let now = host.now_ms();
        let init = Init {
            doc_id,
            meta,
            doc,
            version,
            log: (loaded.updates.len() as u64, log_bytes),
            state_bytes,
            authors,
            now,
        };
        Self::build(host, config, pack, store, init)
    }

    /// Create a doc at `key` from file bytes, imported by [`IMPORT_PID`], with an import
    /// version, all in one store transaction.
    pub(crate) fn import(
        host: Arc<dyn Host>,
        config: Arc<Config>,
        pack: Arc<dyn Pack>,
        store: Arc<DocStore>,
        key: DocKey,
        bytes: &[u8],
    ) -> Result<Self> {
        let schema = pack.schema();
        let content = pack.decode(bytes).map_err(SessionError::Declined)?;
        let ordered = ordered_roots(&schema, content.roots)?;
        let mut doc = Doc::new(schema)?;
        doc.set_edit_check_every(config.edit_check_every);
        doc.set_edit_check_scale(config.edit_check_scale_units);
        doc.set_position_check_every(config.position_check_every);
        let client = loop {
            let c = random_client();
            if c > 1 {
                break c;
            }
        };
        let edits: Vec<Edit> = ordered
            .iter()
            .filter_map(|(root, v)| match v {
                Neutral::Text(t) if !t.is_empty() => Some(Edit::Text {
                    root: root.clone(),
                    hunks: diff::hunks("", t),
                }),
                Neutral::Text(_) => None,
            })
            .collect();
        let update = doc.edit_as(client, &edits)?;
        // each root's imported text: fresh elements from the import client's clocks
        let firsts = crate::engine::insert_clocks(0, &edits);
        let now = host.now_ms();
        let meta = DocMeta {
            key,
            pack: schema.name.to_owned(),
            engine: ENGINE_NAME.to_owned(),
            engine_format: ENGINE_FORMAT,
            codec: content.codec,
            created_ms: now,
            synced_hash: None,
            synced_seq: 0,
            declined: None,
        };
        let entry = update.map(|u| LogEntry {
            author: IMPORT_PID.to_owned(),
            client,
            at_ms: now,
            via: "disk".to_owned(),
            update: u,
            on_behalf_of: None,
        });
        let version = Version {
            seq: 0,
            at_ms: now,
            reason: VersionReason::Import,
            name: None,
            authors: vec![IMPORT_PID.to_owned()],
            content: ordered.clone(),
        };
        // the bytes are the file's: the doc as imported is what the file holds
        let hash_key = crate::store::fresh_key()?;
        let sync = SyncState {
            hash: file_hash(&hash_key, bytes),
            seq: 0,
            state: doc.encode_state()?,
        };
        let change = NewChange {
            info: ChangeInfo {
                lines_added: ordered
                    .iter()
                    .map(|(_, Neutral::Text(t))| t.lines().count() as u64)
                    .sum(),
                ..ChangeInfo::new(ChangeKind::Import, IMPORT_PID, "disk", now)
            },
            record: RevertRecord {
                author: IMPORT_PID.to_owned(),
                on_behalf_of: None,
                at_ms: now,
                roots: ordered
                    .iter()
                    .filter(|(_, Neutral::Text(t))| !t.is_empty())
                    .zip(&firsts)
                    .map(|((root, Neutral::Text(t)), first)| RecordRoot {
                        root: root.clone(),
                        empty_after: false,
                        hunks: vec![RecordHunk {
                            start: 0,
                            end: 0,
                            insert: t.clone(),
                            deleted: String::new(),
                            redacted: false,
                            roots: Some(HunkRoots {
                                inserted: root_runs(&Ids::fresh(
                                    t,
                                    ElemId::new(client, first.first().copied().unwrap_or(0)),
                                )),
                                deleted: Vec::new(),
                            }),
                        }],
                    })
                    .collect(),
            },
            span: Some(ClockSpan {
                client,
                start: 0,
                end: doc.clock_of(client),
            }),
            provenance: Vec::new(),
            authored: None,
        };
        let (doc_id, seq, _) = store.create_imported_synced(
            &meta,
            client,
            IMPORT_PID,
            entry.as_ref(),
            &version,
            Some((&sync, &hash_key)),
            Some(&change),
        )?;
        let ub = entry.as_ref().map_or(0, |e| e.update.len() as u64);
        let mut authors = VecDeque::new();
        if let Some(s) = seq {
            authors.push_back((s, IMPORT_PID.to_owned()));
        }
        let init = Init {
            doc_id,
            meta,
            doc,
            version: seq.unwrap_or(0),
            log: (u64::from(seq.is_some()), ub),
            state_bytes: ub,
            authors,
            now,
        };
        Self::build(host, config, pack, store, init)
    }

    // ----- plumbing -----

    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        let g = self.lock_any()?;
        if let Some(e) = &g.closed {
            return Err(e.clone());
        }
        Ok(g)
    }

    fn lock_any(&self) -> Result<MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| SessionError::Broken("session lock poisoned by a panic".into()))
    }

    fn now(&self) -> u64 {
        let now = self.host.now_ms();
        self.last_used_ms.fetch_max(now, Ordering::Relaxed);
        now
    }

    fn store<'a>(&self, st: &'a State) -> Result<&'a Arc<DocStore>> {
        st.store.as_ref().ok_or(SessionError::Unloaded)
    }

    fn emit(&self, ev: AuditEvent) {
        self.host.on_event(&ev);
    }

    /// A doc this session edits through, with the configured runtime check.
    fn checked(&self, mut doc: Doc) -> Doc {
        doc.set_edit_check_every(self.config.edit_check_every);
        doc.set_edit_check_scale(self.config.edit_check_scale_units);
        doc.set_position_check_every(self.config.position_check_every);
        doc
    }

    /// Report what the engine's runtime check found in the live doc and the disk replica
    /// during change `change` (v0.9.3). Metadata only: no content.
    fn report_engine_checks(&self, st: &mut State, change: Seq) {
        let mut found = st.doc.take_engine_checks();
        if let Some(d) = st.disk.as_mut() {
            found.extend(d.replica.take_engine_checks());
        }
        for f in found {
            self.emit(match f {
                crate::engine::EngineCheck::EditPath { client } => AuditEvent::EditPathMismatch {
                    key: st.key.clone(),
                    client,
                    change,
                },
                crate::engine::EngineCheck::TextCache { root } => AuditEvent::TextCacheMismatch {
                    key: st.key.clone(),
                    root: root.to_owned(),
                    change,
                },
                crate::engine::EngineCheck::Position {
                    root,
                    index,
                    marker,
                } => AuditEvent::PositionIndexMismatch {
                    key: st.key.clone(),
                    root,
                    index,
                    marker,
                    change,
                },
            });
        }
    }

    fn member(st: &State, conn: ConnId) -> Result<usize> {
        st.members
            .iter()
            .position(|m| m.conn == conn)
            .ok_or(SessionError::NotJoined { conn })
    }

    fn remove_member(&self, st: &mut State, i: usize, reason: LeaveReason) {
        let m = st.members.remove(i);
        st.subs.retain(|(c, _, _)| *c != m.conn);
        st.roster.drop_conn(m.conn);
        self.conn_gone(st, m.pres.conn);
        if !m.pres.watcher && !st.members.iter().any(|o| o.p.pid == m.pres.p.pid) {
            st.roster.record(
                RosterEvent::Left {
                    who: Who::of(&m.pres.p.pid),
                    reason,
                },
                self.config.roster_queue,
            );
        }
        self.emit(AuditEvent::Left {
            key: st.key.clone(),
            pid: m.pres.p.pid,
            conn: m.pres.conn,
            reason,
        });
    }

    /// Remove every connection of `pid`; returns how many.
    fn remove_pid(&self, st: &mut State, pid: &str, reason: LeaveReason) -> usize {
        let mut n = 0;
        while let Some(i) = st.members.iter().position(|m| m.p.pid == pid) {
            self.remove_member(st, i, reason);
            n += 1;
        }
        n
    }

    /// Re-ask the host for the connection's pid's rights and admit the call, or refuse it
    /// (removing all of the pid's connections when read is gone).
    fn admit(
        &self,
        st: &mut State,
        conn: ConnId,
        need: Need,
        action: Action,
        now: u64,
    ) -> Result<usize> {
        self.roster_flush(st, now, false);
        let i = Self::member(st, conn)?;
        let watcher = st.members[i].watcher;
        let rights = watched(self.host.rights(&st.members[i].p, &st.key), watcher);
        let pid = st.members[i].p.pid.clone();
        for m in st.members.iter_mut().filter(|m| m.p.pid == pid) {
            m.rights = rights;
        }
        if let Some(n) = presence::refusal(rights, need) {
            if watcher && n != Need::Read {
                // past participants_max: read-only until the host promotes it (K11)
                self.emit(AuditEvent::Refused {
                    key: st.key.clone(),
                    pid: pid.clone(),
                    action,
                    need: n,
                });
                return Err(SessionError::Watching {
                    max: self.config.participants_max,
                });
            }
            self.emit(AuditEvent::Refused {
                key: st.key.clone(),
                pid: pid.clone(),
                action,
                need: n,
            });
            if n == Need::Read {
                self.remove_pid(st, &pid, LeaveReason::RightsLost);
            }
            return Err(SessionError::Forbidden { need: n });
        }
        st.members[i].last_active_ms = now;
        Ok(i)
    }

    /// Admit an edit call: edit rights, not declined, under the pid's rate limit.
    fn admit_edit(&self, st: &mut State, conn: ConnId, action: Action, now: u64) -> Result<usize> {
        let i = self.admit(st, conn, Need::Edit, action, now)?;
        if let Some(reason) = &st.declined {
            return Err(SessionError::Declined(super::pack::Declined {
                code: "declined",
                message: reason.clone(),
            }));
        }
        let c = &self.config;
        let h = st.hist.entry(st.members[i].p.pid.clone()).or_default();
        let b = h
            .bucket
            .get_or_insert_with(|| Bucket::full(c.edit_burst, now));
        b.take(now, c.edit_rate_per_s, c.edit_burst)
            .map_err(|retry_after_ms| SessionError::RateLimited { retry_after_ms })?;
        Ok(i)
    }

    fn text_of<'a>(&self, st: &'a State, root: &str) -> Result<(usize, &'a str)> {
        let i = st
            .texts
            .iter()
            .position(|(r, _)| *r == root)
            .ok_or_else(|| crate::doc::DocError::NoRoot {
                schema: self.schema.name,
                root: root.to_owned(),
            })?;
        Ok((i, st.texts[i].1.as_str()))
    }

    // ----- presence -----

    /// Open a new connection for `p`. Needs read. Each connection gets its own client ID
    /// (fresh, random, recorded as `p.pid`'s), so one pid may hold several: two tabs, or a
    /// command-line call while a `watch` runs. Use this for anything with its own lifetime
    /// (a socket, a `watch`); use [`Session::join_shared`] for one-shot verbs.
    pub fn join(&self, p: Participant, stay: Stay) -> Result<Joined> {
        self.join_impl(p, stay, false)
    }

    /// Like [`Session::join`], but reuses the pid's existing shared connection with the same
    /// `via`, if one is present (refreshing name, colour and kind; a hold is sticky). One-shot
    /// command-line verbs use this, so an agent running many verbs keeps one connection and
    /// one client ID instead of minting one per call (every client ID stays in the doc's
    /// state for good).
    pub fn join_shared(&self, p: Participant, stay: Stay) -> Result<Joined> {
        self.join_impl(p, stay, true)
    }

    fn join_impl(&self, p: Participant, stay: Stay, shared: bool) -> Result<Joined> {
        presence::check_participant(&p)?;
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let rights = self.host.rights(&p, &st.key);
        if !rights.read {
            self.emit(AuditEvent::Refused {
                key: st.key.clone(),
                pid: p.pid.clone(),
                action: Action::Join,
                need: Need::Read,
            });
            self.remove_pid(st, &p.pid, LeaveReason::RightsLost);
            return Err(SessionError::Forbidden { need: Need::Read });
        }
        for m in st.members.iter_mut().filter(|m| m.p.pid == p.pid) {
            m.rights = watched(rights, m.watcher);
        }
        if shared {
            if let Some(m) = st.members.iter_mut().find(|m| m.shares_with(&p)) {
                m.p = p;
                m.last_active_ms = now;
                if stay == Stay::Held {
                    m.stay = Stay::Held;
                }
                return Ok(Joined {
                    conn: m.pres.conn,
                    client: m.client,
                    version: st.version,
                });
            }
        }
        let client = match self.reusable_client(st, &p)? {
            Some(c) => c,
            None => {
                let store = self.store(st)?;
                let c = fresh_client(store, self.doc_id, |c| {
                    st.members.iter().any(|m| m.client == c)
                })?;
                store.set_client(self.doc_id, c, &p.pid)?;
                if p.own_replica {
                    st.replica_clients.insert(c);
                }
                if let Some(o) = st.owners.as_mut() {
                    o.insert(c, p.pid.clone());
                }
                c
            }
        };
        st.joins += 1;
        // K11: a new pid past participants_max watches (read-only, no presence detail); a pid
        // already here keeps what it is
        let here = |w: bool| {
            st.members
                .iter()
                .any(|m| m.p.pid == p.pid && m.watcher == w)
        };
        let watcher = here(true)
            || (!here(false) && Self::participant_count(st) >= self.config.participants_max);
        let mut pres = Presence::new(p, shared, watched(rights, watcher), stay, st.joins, now);
        pres.watcher = watcher;
        let conn = pres.conn;
        self.emit(AuditEvent::Joined {
            key: st.key.clone(),
            pid: pres.p.pid.clone(),
            conn,
            client,
        });
        if st.disk.is_some() {
            // a stat check on join (errors are reported as events)
            let _ = self.disk_check(st, now, false);
        }
        if !watcher && !st.members.iter().any(|m| m.p.pid == pres.p.pid) {
            st.roster.record(
                RosterEvent::Joined {
                    who: Who::of(&pres.p.pid),
                    participant: pres.p.clone(),
                    rights,
                },
                self.config.roster_queue,
            );
        }
        st.members.push(Member {
            pres,
            client,
            cursor: None,
            view: None,
            view_ms: None,
            edited: None,
        });
        self.roster_flush(st, now, false);
        Ok(Joined {
            conn,
            client,
            version: st.version,
        })
    }

    /// A client ID agent `p` used before in this doc that no present connection holds
    /// (v0.9.7, [`Config::reuse_agent_clients`]): the lowest such, or `None` (mint one). The
    /// doc holds everything that client wrote (only this doc's sessions ever write as it,
    /// one connection at a time), so its edits continue its history: the one-writer
    /// invariant holds as for a fresh ID.
    fn reusable_client(&self, st: &mut State, p: &Participant) -> Result<Option<ClientId>> {
        if !self.config.reuse_agent_clients || p.own_replica || p.kind != ParticipantKind::Agent {
            return Ok(None);
        }
        // the owners are read in one place (`owners`, shared with D1)
        self.owners(st)?;
        let owners = st.owners.as_ref().expect("built above");
        let held = |c: &ClientId| st.members.iter().any(|m| m.client == *c);
        Ok(owners
            .iter()
            .filter(|(c, owner)| **owner == p.pid && !st.replica_clients.contains(c) && !held(c))
            .map(|(c, _)| *c)
            .min())
    }

    /// Close one connection now. Returns false when it wasn't present. The pid stays present
    /// while it has other connections; its undo history stays until the session unloads.
    pub fn leave(&self, conn: ConnId) -> Result<bool> {
        self.now();
        let mut st = self.lock()?;
        match Self::member(&st, conn) {
            Ok(i) => {
                self.remove_member(&mut st, i, LeaveReason::Left);
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    /// Remove a participant on the host's say-so: every connection of `pid` closes, with its
    /// subscriptions. Returns how many connections closed.
    pub fn kick(&self, pid: &str) -> Result<usize> {
        let mut st = self.lock()?;
        Ok(self.remove_pid(&mut st, pid, LeaveReason::Kicked))
    }

    /// Keep a connection present (any call does this too). Needs read.
    pub fn touch(&self, conn: ConnId) -> Result<()> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Touch, now)?;
        Ok(())
    }

    /// Set how long a connection stays without activity.
    pub fn set_stay(&self, conn: ConnId, stay: Stay) -> Result<()> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::Touch, now)?;
        st.members[i].stay = stay;
        Ok(())
    }

    /// Whether this connection's view of the doc is visible (a tab in front). A participant
    /// shows as away when every one of their connections has been not visible for
    /// [`Config::away_after_ms`].
    pub fn set_visible(&self, conn: ConnId, visible: bool) -> Result<()> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::View, now)?;
        if !st.members[i].watcher {
            st.members[i].set_visible(visible, now);
        }
        Ok(())
    }

    /// Place this connection's caret or selection (`cursor --at L:C`). Needs edit; readers
    /// use [`Session::set_view`] instead. Returns the caret in byte offsets.
    pub fn set_cursor(&self, conn: ConnId, root: &str, anchor: At, head: At) -> Result<Cursor> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Edit, Action::Cursor, now)?;
        let (_, text) = self.text_of(&st, root)?;
        let res = |a: At| {
            a.resolve(text)
                .ok_or_else(|| SessionError::BadPosition(format!("{a:?} is not in {root:?}")))
        };
        let c = Cursor {
            root: root.to_owned(),
            anchor: res(anchor)?,
            head: res(head)?,
        };
        let m = &mut st.members[i];
        m.cursor = Some(c.clone());
        m.last_cursor_ms = Some(now);
        Ok(c)
    }

    /// Set the lines this connection is looking at (`head`, `sed -n a,bp`, a scrolled tab).
    /// Needs read. `last` past the end is clamped; returns the range as stored.
    pub fn set_view(&self, conn: ConnId, range: LineRange) -> Result<LineRange> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::View, now)?;
        let (_, text) = self.text_of(&st, &range.root)?;
        let count = position::line_count(text);
        if range.first == 0 || range.first > range.last || range.first > count {
            return Err(SessionError::BadPosition(format!(
                "lines {}..{} of a {count}-line text",
                range.first, range.last
            )));
        }
        let last = range.last.min(count);
        let s = position::line_start(text, range.first).expect("line checked");
        let e = position::line_end_from(
            text,
            position::line_start(text, last).expect("line checked"),
        );
        // a watcher (K11) shows no presence: its view is not kept
        if !st.members[i].watcher {
            st.members[i].view = Some((range.root.clone(), s, e));
            st.members[i].view_ms = Some(now);
        }
        Ok(LineRange { last, ..range })
    }

    /// Who is here: one entry per participant (pid), editors first, then viewers, then away;
    /// in order of first join within each. Needs read.
    pub fn roster(&self, conn: ConnId) -> Result<Vec<RosterEntry>> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::Roster, now)?;
        self.expire_claims(&mut st, now);
        let rows: Vec<RosterEntry> = self
            .roster_rows(&st, now)
            .into_iter()
            .map(|r| r.entry)
            .collect();
        Ok(match Self::app_viewer(&st, i) {
            Some(v) => {
                let mut lens = st.handles.lens(&v);
                rows.into_iter().map(|e| lens.entry(e)).collect()
            }
            None => rows,
        })
    }

    /// The roster rows from presence now (claims already expired), in roster order, each with
    /// its participant's rights: what [`Session::roster`] returns and the roster push reads.
    fn roster_rows(&self, st: &State, now: u64) -> Vec<roster::Row> {
        let text = |root: &str| {
            st.texts
                .iter()
                .find(|(r, _)| *r == root)
                .map(|(_, t)| t.as_str())
        };
        let mut rows = presence::fold_roster(&st.members, now, &self.config, |_| false);
        for (conns, entry, _) in &mut rows {
            entry.cursor = conns.iter().find_map(|m| {
                let cur = m.cursor.as_ref()?;
                let t = text(&cur.root)?;
                Some(CursorAt {
                    cursor: cur.clone(),
                    anchor: position::line_col(t, cur.anchor)?,
                    head: position::line_col(t, cur.head)?,
                })
            });
            entry.view = conns.iter().find_map(|m| {
                let (root, s, e) = m.view.as_ref()?;
                let t = text(root)?;
                Some(LineRange {
                    root: root.clone(),
                    first: position::line_of(t, *s),
                    last: position::line_of(t, *e),
                })
            });
            entry.last_edit = conns.iter().find_map(|m| {
                let (root, s, e) = m.edited.as_ref()?;
                awareness::edit_span_lines(root, text(root)?, *s, *e)
            });
            entry.claims = st
                .aware
                .claims
                .iter()
                .filter(|c| c.pid == entry.participant.pid)
                .filter_map(|c| Some(c.info(text(&c.root)?, now)))
                .collect();
        }
        let rows = rows
            .into_iter()
            .map(|(conns, entry, joined)| (conns[0].rights, entry, joined))
            .collect();
        presence::sort_roster_with(rows)
            .into_iter()
            .map(|(rights, entry)| roster::Row { entry, rights })
            .collect()
    }

    /// Push this doc's roster to `conn` from now on: a full [`RosterEvent::Snapshot`] first,
    /// then deltas (joins, leaves, rights, role, away and back, carets, claim starts and ends,
    /// heads-up notes), coalesced over [`Config::roster_interval_ms`]. Needs read; closes when
    /// the connection leaves, or its participant is kicked or loses read; drop the receiver
    /// to unsubscribe.
    ///
    /// Deliveries happen when the host drives them: [`SessionManager::flush_rosters`] (or
    /// `tick`, or [`Session::flush_roster`]) at least every `roster_interval_ms`; any call on
    /// the doc also delivers what is due. A receiver that falls [`Config::roster_queue`]
    /// events behind gets a fresh `Snapshot` once it has room, instead of what it missed.
    ///
    /// [`SessionManager::flush_rosters`]: super::SessionManager::flush_rosters
    pub fn subscribe_roster(&self, conn: ConnId) -> Result<Receiver<RosterEvent>> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::Subscribe, now)?;
        let app = Self::app_viewer(&st, i);
        // the others get what is pending first, so the new subscriber starts where they are
        self.roster_flush(&mut st, now, true);
        self.expire_claims(&mut st, now);
        let rows = self.roster_rows(&st, now);
        let (tx, rx) = sync_channel(self.config.roster_queue.max(1));
        let version = st.version;
        let watching = Self::watching_count(&st);
        let st = &mut *st;
        let pid = st.members[i].p.pid.clone();
        let overwritten = self
            .waiting_for_subscribers(st, now)
            .remove(&pid)
            .unwrap_or_default();
        st.roster.subscribe(
            conn,
            pid,
            overwritten,
            app,
            tx,
            now,
            version,
            rows,
            watching,
            &mut st.handles,
        );
        Ok(rx)
    }

    /// Deliver this doc's roster events if [`Config::roster_interval_ms`] has passed since
    /// the last delivery ([`SessionManager::flush_rosters`] does this for every loaded doc).
    ///
    /// [`SessionManager::flush_rosters`]: super::SessionManager::flush_rosters
    pub fn flush_roster(&self) -> Result<()> {
        // the host's clock, not `now()`: a delivery is not a use (an idle doc still unloads)
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        self.roster_flush(&mut st, now, false);
        Ok(())
    }

    /// Whether this connection is in this doc.
    pub(crate) fn has_conn(&self, conn: ConnId) -> bool {
        self.lock()
            .is_ok_and(|st| st.members.iter().any(|m| m.conn == conn))
    }

    /// Deliver the roster's pending events and presence changes when due (or now, `force`).
    fn roster_flush(&self, st: &mut State, now: u64, force: bool) {
        if st.roster.is_empty() || !(force || st.roster.due(now, self.config.roster_interval_ms)) {
            return;
        }
        self.expire_claims(st, now);
        let rows = self.roster_rows(st, now);
        let version = st.version;
        let watching = Self::watching_count(st);
        self.notes_at_flush(st);
        let waiting = if st.roster.owes_snapshot() && !st.aware.pending.is_empty() {
            self.waiting_for_subscribers(st, now)
        } else {
            HashMap::new()
        };
        st.roster
            .flush(now, version, rows, watching, &mut st.handles, &waiting);
    }

    /// Let the earliest watcher in (K11), when a slot is free: a participant left since it
    /// joined past [`Config::participants_max`]. **Only on the host's say-so**: a slot that
    /// frees never promotes anyone by itself. Its connections get presence, and their rights
    /// are the host's again from the next call. Returns the pid let in, or `None` (no slot
    /// free, or nobody watching).
    pub fn promote_watcher(&self) -> Result<Option<String>> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        let st = &mut *st;
        if Self::participant_count(st) >= self.config.participants_max {
            return Ok(None);
        }
        // members are in join order: the first watcher joined earliest
        let Some(pid) = st
            .members
            .iter()
            .find(|m| m.watcher)
            .map(|m| m.p.pid.clone())
        else {
            return Ok(None);
        };
        let k = st
            .members
            .iter()
            .position(|m| m.p.pid == pid)
            .expect("found");
        let rights = self.host.rights(&st.members[k].p, &st.key);
        for m in st.members.iter_mut().filter(|m| m.p.pid == pid) {
            m.watcher = false;
            m.rights = rights;
        }
        st.roster.record(
            RosterEvent::Joined {
                who: Who::of(&pid),
                participant: st.members[k].p.clone(),
                rights,
            },
            self.config.roster_queue,
        );
        self.roster_flush(st, now, false);
        Ok(Some(pid))
    }

    /// How many participants are watching past [`Config::participants_max`] (K11): the
    /// roster's `watching: N`. Needs read.
    pub fn watching(&self, conn: ConnId) -> Result<usize> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Roster, now)?;
        Ok(Self::watching_count(&st))
    }

    /// Close connections without a hold that have been idle past [`Config::idle_leave_ms`],
    /// and drop expired read bases. Driven by the host's clock. Returns the connections
    /// that left, with their pids.
    pub fn expire_idle(&self) -> Result<Vec<(ConnId, String)>> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        self.prune_read_bases(&mut st, now);
        self.expire_claims(&mut st, now);
        let c = &self.config;
        st.aware
            .prune_noted(now, c.note_repeat_ms, c.note_interval_ms);
        let mut gone = Vec::new();
        let mut i = 0;
        while i < st.members.len() {
            let m = &st.members[i];
            // a connection holding or waiting for a live claim stays (bounded by its TTL)
            if m.is_idle(now, &self.config) && !Self::in_claims(&st, m.conn) {
                gone.push((m.conn, m.p.pid.clone()));
                self.remove_member(&mut st, i, LeaveReason::Idle);
            } else {
                i += 1;
            }
        }
        self.roster_flush(&mut st, now, false);
        // forget full rate buckets of absent participants with no history; a purge notice
        // still owed is history (U19: said on the holder's next undo or redo, present or not)
        let c = &self.config;
        let present: Vec<String> = st.members.iter().map(|m| m.p.pid.clone()).collect();
        st.hist.retain(|pid, h| {
            present.contains(pid)
                || !h.undo.is_empty()
                || !h.redo.is_empty()
                || h.cleared
                || h.last_read.is_some()
                || !h
                    .bucket
                    .as_ref()
                    .map_or(true, |b| b.is_full(now, c.edit_rate_per_s, c.edit_burst))
        });
        Ok(gone)
    }

    /// Re-resolve every participant's rights (the host's periodic re-check), once per pid.
    /// Participants who lost read are removed with all their connections. Returns the pids
    /// whose rights changed.
    pub fn recheck_rights(&self) -> Result<Vec<RightsChange>> {
        let mut st = self.lock()?;
        let st = &mut *st;
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        let mut i = 0;
        while i < st.members.len() {
            let pid = st.members[i].p.pid.clone();
            if seen.contains(&pid) {
                i += 1;
                continue;
            }
            seen.push(pid.clone());
            let r = self.host.rights(&st.members[i].p, &st.key);
            if r != st.members[i].rights {
                out.push(RightsChange {
                    pid: pid.clone(),
                    rights: r,
                });
            }
            for m in st.members.iter_mut().filter(|m| m.p.pid == pid) {
                m.rights = r;
            }
            if !r.read {
                self.remove_pid(st, &pid, LeaveReason::RightsLost);
            } else {
                i += 1;
            }
        }
        let now = self.host.now_ms();
        self.roster_flush(st, now, false);
        Ok(out)
    }

    /// Receive every change from now on (`watch`, `tail -f`). Needs read; closes when the
    /// connection leaves, or its participant is kicked or loses read.
    ///
    /// The queue holds [`Config::subscriber_queue`] events. A subscriber that falls that far
    /// behind is cut off: it drains what was queued, then sees the channel disconnected, and
    /// should re-read and subscribe again. Versions are consecutive, so a gap is visible.
    pub fn subscribe(&self, conn: ConnId) -> Result<Receiver<ChangeEvent>> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::Subscribe, now)?;
        let (tx, rx) = sync_channel(self.config.subscriber_queue.max(1));
        let app = Self::app_viewer(&st, i);
        st.subs.push((conn, app, tx));
        Ok(rx)
    }

    // ----- reading -----

    /// The doc as the file would hold it now. Needs read. Records this version as a valid
    /// `write --base` for a while ([`Config::read_bases_max`], `read_base_ttl_ms`,
    /// `read_bases_max_bytes`).
    ///
    /// "Contributors since last read" is per participant (pid), across their connections.
    pub fn read(&self, conn: ConnId) -> Result<ReadResult> {
        let out = self.read_inner(conn);
        self.for_viewer(conn, out, |l, mut r| {
            r.contributors_since_last_read = std::mem::take(&mut r.contributors_since_last_read)
                .into_iter()
                .map(|p| l.pid(&p))
                .collect();
            r
        })
    }

    fn read_inner(&self, conn: ConnId) -> Result<ReadResult> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::Read, now)?;
        self.read_locked(&mut st, i, now)
    }

    /// [`Session::read`], then `f` on `root`'s text under the same lock (the tools' read
    /// verbs). `f` returns its result and the lines it showed (1-based, inclusive): `view`
    /// becomes the connection's viewing range (P4); `touched` (the lines the call looked
    /// at) gets the heads-up notes, the activity row, and, with `for_edit: Some(ttl)`, an
    /// implicit claim. Like `edit_with`'s `f`, it must be bounded and must not call back
    /// into the kit; a panic is passed on after the lock is released.
    pub(crate) fn read_with<R>(
        &self,
        conn: ConnId,
        root: &str,
        verb: &str,
        for_edit: Option<Option<u64>>,
        f: impl FnOnce(&ReadResult, &str) -> (R, Shown),
    ) -> Result<(ReadResult, R, Notes, Option<awareness::ForEdit>)> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::Read, now)?;
        self.text_of(&st, root)?;
        let read = self.read_locked(&mut st, i, now)?;
        let (r, Shown { view, touched }) = {
            let text = self.text_of(&st, root)?.1;
            match catch_unwind(AssertUnwindSafe(|| f(&read, text))) {
                Ok(r) => r,
                Err(panic) => {
                    drop(st);
                    resume_unwind(panic);
                }
            }
        };
        if let Some((first, last)) = view {
            let text = self.text_of(&st, root)?.1;
            let count = position::line_count(text);
            if first >= 1 && first <= last && first <= count {
                let s = position::line_start(text, first).expect("line checked");
                let e = position::line_end_from(
                    text,
                    position::line_start(text, last.min(count)).expect("line checked"),
                );
                st.members[i].view = Some((root.to_owned(), s, e));
                st.members[i].view_ms = Some(now);
            }
        }
        let st = &mut *st;
        self.expire_claims(st, now);
        let for_edit =
            for_edit.map(|ttl| self.for_edit_claim(st, i, root, touched, ttl, verb, now));
        let notes = self.notes_for(st, i, root, touched, now);
        let lines = touched.map(|(first, last)| LineRange {
            root: root.to_owned(),
            first,
            last,
        });
        self.log_activity(st, i, ActivityKind::Read, verb, lines, now, None);
        Ok((read, r, notes, for_edit))
    }

    fn read_locked(&self, st: &mut State, i: usize, now: u64) -> Result<ReadResult> {
        let pid = st.members[i].p.pid.clone();
        let pid = pid.as_str();
        let since = st.hist.get(pid).and_then(|h| h.last_read);
        let mut contributors: Vec<String> = Vec::new();
        for (s, a) in &st.authors {
            if since.map_or(true, |v| *s > v)
                && a != pid
                && a != IMPORT_PID
                && !contributors.contains(a)
            {
                contributors.push(a.clone());
            }
        }
        let version = st.version;
        st.hist.entry(pid.to_owned()).or_default().last_read = Some(version);
        let content: Vec<(String, Neutral)> = st
            .texts
            .iter()
            .map(|(r, t)| ((*r).to_owned(), Neutral::Text(t.clone())))
            .collect();
        let bytes = self.pack.encode(&content, &st.codec)?;
        self.record_read_base(st, now);
        Ok(ReadResult {
            bytes,
            text_by_root: content
                .into_iter()
                .map(|(r, n)| match n {
                    Neutral::Text(t) => (r, t),
                })
                .collect(),
            version,
            contributors_since_last_read: contributors,
        })
    }

    fn record_read_base(&self, st: &mut State, now: u64) {
        let c = &self.config;
        if let Some(last) = st.read_bases.back_mut() {
            if last.version == st.version {
                last.at_ms = now;
                return;
            }
        }
        let bytes: usize = st.texts.iter().map(|(_, t)| t.len()).sum();
        if bytes > c.read_bases_max_bytes || c.read_bases_max == 0 {
            return;
        }
        st.read_bases.push_back(ReadBase {
            version: st.version,
            at_ms: now,
            texts: st.texts.clone(),
            bytes,
        });
        st.read_base_bytes += bytes;
        while st.read_bases.len() > c.read_bases_max || st.read_base_bytes > c.read_bases_max_bytes
        {
            let old = st.read_bases.pop_front().expect("non-empty");
            st.read_base_bytes -= old.bytes;
        }
        self.prune_read_bases(st, now);
    }

    fn prune_read_bases(&self, st: &mut State, now: u64) {
        let ttl = self.config.read_base_ttl_ms;
        while st
            .read_bases
            .front()
            .is_some_and(|b| now.saturating_sub(b.at_ms) > ttl)
        {
            let old = st.read_bases.pop_front().expect("non-empty");
            st.read_base_bytes -= old.bytes;
        }
    }

    /// Text of `root` at version `base`: live, a cached read, or a stored version.
    fn base_text(&self, st: &mut State, root: &str, base: Seq, now: u64) -> Result<Option<String>> {
        if base == st.version {
            return Ok(Some(self.text_of(st, root)?.1.to_owned()));
        }
        self.prune_read_bases(st, now);
        if let Some(b) = st.read_bases.iter().find(|b| b.version == base) {
            return Ok(b
                .texts
                .iter()
                .find(|(r, _)| *r == root)
                .map(|(_, t)| t.clone()));
        }
        let store = self.store(st)?;
        // a version a purge cut spans from is no merge base: its text isn't what was read
        let Some(vid) = store
            .version_infos(self.doc_id)?
            .into_iter()
            .find(|v| v.seq == base && v.redacted.is_empty())
            .map(|v| v.id)
        else {
            return Ok(None);
        };
        Ok(store
            .version(self.doc_id, vid)?
            .content
            .into_iter()
            .find(|(r, _)| r == root)
            .map(|(_, n)| match n {
                Neutral::Text(t) => t,
            }))
    }

    /// Version, participants, log size, memory estimate, declined reason. Metadata only.
    pub fn status(&self) -> Result<Status> {
        let st = self.lock()?;
        Ok(Status {
            key: st.key.clone(),
            version: st.version,
            participants: presence::participants(&st.members),
            connections: st.members.len(),
            log_entries: st.log_entries,
            estimated_bytes: self.estimate(&st),
            declined: st.declined.clone(),
            disk: Self::disk_state(&st),
        })
    }

    /// The doc's current key (it changes with [`super::SessionManager::rekey`]).
    pub fn key(&self) -> Result<DocKey> {
        Ok(self.lock()?.key.clone())
    }

    // ----- editing -----

    /// Apply `edits` (hunks in byte offsets of the live text) through `conn`, as one atomic
    /// change. Needs edit (re-checked now). Inserted text is normalised by the pack. The
    /// change is durable in the store before this returns. Each root may appear once.
    ///
    /// Hunks are positional, so they are only right against the version they were computed
    /// on: pass that version as `if_version`, and a doc that has moved since gives
    /// [`SessionError::Stale`] with nothing applied (re-read and recompute, or use
    /// [`Session::edit_with`]).
    pub fn edit(
        &self,
        conn: ConnId,
        edits: Vec<Edit>,
        if_version: Option<Seq>,
    ) -> Result<EditResult> {
        prof!("session.edit");
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit_edit(&mut st, conn, Action::Edit, now)?;
        Self::check_version(&st, if_version)?;
        self.check_backlog(&st, i)?;
        let edits = self.normalize(edits)?;
        self.commit_edit(&mut st, i, edits, ChangeKind::Edit, None, now, Vec::new())
    }

    /// Read live, compute, apply, atomically: run `f` on the live text of `root` under the
    /// session lock, then apply its result through a minimal diff as one change (`None` when
    /// it changes nothing). Needs edit. This is how edit verbs run (`sed -i`, `patch`,
    /// `insert`, `replace-section`): nothing can change between the read and the write.
    ///
    /// `f` runs with the session locked: it must be bounded (no I/O, bounded regexes) and
    /// must not call back into the kit. An error from `f` is returned as
    /// [`EditWithError::Compute`] with nothing applied. A panic in `f` is passed on to the
    /// caller after the lock is released (the session stays usable).
    pub fn edit_with<E>(
        &self,
        conn: ConnId,
        root: &str,
        f: impl FnOnce(&str) -> std::result::Result<String, E>,
    ) -> std::result::Result<Option<EditResult>, EditWithError<E>> {
        self.edit_with_policy(conn, root, ClaimPolicy::Ignore, f)
    }

    /// [`Session::edit_with`] under the host's claim `policy` (amendment 13): with
    /// [`ClaimPolicy::Respect`], an agent's edit touching another participant's live claim
    /// is refused with [`SessionError::Claimed`] (exit 5) and nothing applies. A person is
    /// never refused. `edit_with` is this with [`ClaimPolicy::Ignore`].
    pub fn edit_with_policy<E>(
        &self,
        conn: ConnId,
        root: &str,
        policy: ClaimPolicy,
        f: impl FnOnce(&str) -> std::result::Result<String, E>,
    ) -> std::result::Result<Option<EditResult>, EditWithError<E>> {
        self.edit_with_impl(conn, root, policy, None, None, None, f)
            .map(|(r, _)| r)
    }

    /// [`Session::edit_with`] with the host's [`EditOptions`]: the claim policy (as
    /// [`Session::edit_with_policy`]) and a version guard (as [`Session::edit`]'s
    /// `if_version` and the edit verbs' `--if-version`, amendment 14). Both are checked
    /// under the session lock, in the edit's critical section, as `run_tool_with` checks
    /// them for edit verbs: nothing can land between the checks and the apply, so a host
    /// needs no text comparison of its own. Order: admission, then the version
    /// ([`SessionError::Stale`], nothing applied), the approvals backlog, `f`, the claims
    /// ([`SessionError::Claimed`]).
    ///
    /// Whole-text replacement under a guard is `edit_with_options(conn, root, &opts, |_|
    /// Ok::<_, Infallible>(new_text))`.
    pub fn edit_with_options<E>(
        &self,
        conn: ConnId,
        root: &str,
        opts: &EditOptions,
        f: impl FnOnce(&str) -> std::result::Result<String, E>,
    ) -> std::result::Result<Option<EditResult>, EditWithError<E>> {
        self.edit_with_impl(
            conn,
            root,
            opts.policy,
            opts.if_version,
            None,
            opts.group,
            f,
        )
        .map(|(r, _)| r)
    }

    /// `if_version: Some(V)` and the doc is not at V: [`SessionError::Stale`]. Called with
    /// the session locked, right after admission, so nothing can move the doc between this
    /// check and the apply; and before every other refusal (backlog, the computation's own
    /// "no", claims).
    fn check_version(st: &State, if_version: Option<Seq>) -> Result<()> {
        match if_version {
            Some(expected) if expected != st.version => Err(SessionError::Stale {
                expected,
                actual: st.version,
            }),
            _ => Ok(()),
        }
    }

    /// `edit_with` with a policy, an `if_version` guard (as [`Session::edit`]'s, amendment
    /// 14: `run_tool`'s `--if-version`), the verb for the activity ring, a person's undo
    /// group token ([`EditOptions::group`]), and the heads-up notes on the lines it changed.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn edit_with_impl<E>(
        &self,
        conn: ConnId,
        root: &str,
        policy: ClaimPolicy,
        if_version: Option<Seq>,
        verb: Option<&str>,
        group: Option<u64>,
        f: impl FnOnce(&str) -> std::result::Result<String, E>,
    ) -> std::result::Result<(Option<EditResult>, Notes), EditWithError<E>> {
        prof!("session.edit_with");
        let now = self.now();
        let mut st = {
            prof!("session.lock");
            self.lock()?
        };
        let i = {
            prof!("session.admit");
            self.admit_edit(&mut st, conn, Action::Edit, now)?
        };
        Self::check_version(&st, if_version)?;
        {
            prof!("session.backlog");
            self.check_backlog(&st, i)?;
        }
        let hunks = {
            prof!("session.compute_diff");
            let live = self.text_of(&st, root)?.1;
            let computed = match catch_unwind(AssertUnwindSafe(|| f(live))) {
                Ok(r) => r.map_err(EditWithError::Compute)?,
                Err(panic) => {
                    drop(st);
                    resume_unwind(panic);
                }
            };
            let new_text = self.pack.normalize_input(root, &computed);
            lines::edit_hunks(live, &new_text)
        };
        if hunks.is_empty() {
            return Ok((None, Notes::default()));
        }
        {
            prof!("session.check_claims");
            self.check_claims(&mut st, i, root, &hunks, policy, now)?;
        }
        let edits = vec![Edit::Text {
            root: root.to_owned(),
            hunks,
        }];
        st.verb_hint = verb.map(str::to_owned);
        st.group_hint = group;
        let res = self.commit_edit(&mut st, i, edits, ChangeKind::Edit, None, now, Vec::new())?;
        // a verb returns its notes; the host's own edit calls return only the result, so they
        // leave waiting notes where they are (taken, they'd be lost)
        let notes = {
            prof!("session.notes");
            #[allow(unused_mut)]
            let mut deliver = verb.is_some();
            #[cfg(any(test, feature = "differential"))]
            if crate::differential::note_mutant()
                == Some(crate::differential::NoteMutant::EditWithDrains)
            {
                deliver = true;
            }
            self.edit_notes(&mut st, i, &res, now, deliver)
        };
        Ok((Some(res), notes))
    }

    /// The heads-up notes on the lines an applied change touched (the first root's).
    /// `deliver`: also the waiting notes (an outside save's), for a call that returns them.
    fn edit_notes(
        &self,
        st: &mut State,
        i: usize,
        res: &EditResult,
        now: u64,
        deliver: bool,
    ) -> Notes {
        if deliver {
            return match changed_span(&res.changed) {
                Some(l) => self.notes_for(st, i, &l.root, Some((l.first, l.last)), now),
                None => self.notes_for(st, i, self.schema.roots[0].0, None, now),
            };
        }
        match changed_span(&res.changed) {
            Some(l) => self.fresh_notes(st, i, &l.root, Some((l.first, l.last)), now),
            None => Notes::default(),
        }
    }

    /// Make `root` equal `new_text` through `conn`, through a minimal diff against the live
    /// text. This is last-writer-wins for that root (`write --base live`): use
    /// [`Session::write_base`] when the text was derived from an earlier read. As for every
    /// `write` body, one leading U+FEFF is taken off when the file has a BOM
    /// ([`Pack::writes_bom`]).
    pub fn replace_text(&self, conn: ConnId, root: &str, new_text: &str) -> Result<EditResult> {
        self.replace_text_with(conn, root, new_text, ClaimPolicy::Ignore)
    }

    /// [`Session::replace_text`] under the host's claim `policy` (as
    /// [`Session::edit_with_policy`]).
    pub fn replace_text_with(
        &self,
        conn: ConnId,
        root: &str,
        new_text: &str,
        policy: ClaimPolicy,
    ) -> Result<EditResult> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit_edit(&mut st, conn, Action::Edit, now)?;
        self.check_backlog(&st, i)?;
        let new_text = self
            .pack
            .normalize_input(root, self.write_body(&st, root, new_text));
        let live = self.text_of(&st, root)?.1;
        let hunks = lines::edit_hunks(live, &new_text);
        self.check_claims(&mut st, i, root, &hunks, policy, now)?;
        let edits = vec![Edit::Text {
            root: root.to_owned(),
            hunks,
        }];
        self.commit_edit(&mut st, i, edits, ChangeKind::Edit, None, now, Vec::new())
    }

    /// A `write` body as the doc holds it: when the file starts with a byte-order mark
    /// ([`Pack::writes_bom`]), one leading U+FEFF is the BOM `cat` printed, not text, and is
    /// taken off. Without a BOM in the file, a leading U+FEFF is the caller's text and stays.
    fn write_body<'a>(&self, st: &State, root: &str, text: &'a str) -> &'a str {
        match text.strip_prefix('\u{feff}') {
            Some(rest) if self.pack.writes_bom(root, &st.codec) => rest,
            _ => text,
        }
    }

    /// `write --base V`: apply only what the caller changed between version `base` and
    /// `new_text`, three-way merged onto the live text. Overlap with a change made since
    /// `base` gives [`WriteOutcome::Conflicts`] and applies nothing, except that two
    /// appends at the end of the text both land, the live one first (merge rule O5). Valid
    /// bases: the live version, recently read versions, and stored versions. When the file
    /// has a BOM ([`Pack::writes_bom`]), one leading U+FEFF of `new_text` is taken off: it is
    /// the BOM `cat` printed.
    pub fn write_base(
        &self,
        conn: ConnId,
        root: &str,
        base: Seq,
        new_text: &str,
    ) -> Result<WriteOutcome> {
        Ok(self
            .write_base_with(conn, root, base, new_text, ClaimPolicy::Ignore)?
            .outcome)
    }

    /// [`Session::write_base`] under the host's claim `policy` (as
    /// [`Session::edit_with_policy`]), with who claimed or last edited each conflicting
    /// region ([`WriteReport::conflict_notes`]) and the heads-up notes of an applied write.
    pub fn write_base_with(
        &self,
        conn: ConnId,
        root: &str,
        base: Seq,
        new_text: &str,
        policy: ClaimPolicy,
    ) -> Result<WriteReport> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit_edit(st, conn, Action::WriteBase, now)?;
        self.check_backlog(st, i)?;
        let live = self.text_of(st, root)?.1.to_owned();
        let Some(base_text) = self.base_text(st, root, base, now)? else {
            return Ok(WriteReport::of(WriteOutcome::BaseExpired));
        };
        let ours = self
            .pack
            .normalize_input(root, self.write_body(st, root, new_text));
        match merge::merge3(&base_text, &ours, &live) {
            Merge::Conflicts(c) => {
                self.expire_claims(st, now);
                // each conflict's base range, carried onto the live text (through the
                // merge's own theirs side, merge::write_base_sides: whole lines on the
                // closed texts, so a span may end one past an open live text's end)
                let since = merge::write_base_sides(&base_text, &ours, &live).1;
                let pid = st.members[i].p.pid.clone();
                let mut who = Vec::new();
                for (k, cf) in c.iter().enumerate() {
                    let (s0, e0) = awareness::carry_span(cf.base.start, cf.base.end, &since);
                    let span = (s0.min(live.len()), e0.min(live.len()));
                    for n in self.collect_notes(st, &pid, root, span, None, now) {
                        if matches!(
                            n.kind,
                            awareness::NoteKind::Claimed | awareness::NoteKind::Edited
                        ) {
                            who.push((k, n));
                        }
                    }
                }
                let who = match Self::app_viewer(st, i) {
                    Some(v) => {
                        let mut l = st.handles.lens(&v);
                        who.into_iter().map(|(k, n)| (k, l.note(n))).collect()
                    }
                    None => who,
                };
                Ok(WriteReport {
                    outcome: WriteOutcome::Conflicts(c),
                    conflict_notes: who,
                    notes: Notes::default(),
                })
            }
            Merge::Clean(c) => {
                self.check_claims(st, i, root, &c.hunks, policy, now)?;
                // the test-only rewrite-merged-ranges mutant: theirs' ranges (what the live
                // text changed since the base) become authored too
                #[cfg(any(test, feature = "differential"))]
                if crate::differential::d1_mutant()
                    == crate::differential::D1Mutant::RewriteMergedRanges
                {
                    let mut shift: isize = 0;
                    let theirs: Vec<(usize, usize)> = diff::hunks(&base_text, &live)
                        .into_iter()
                        .map(|h| {
                            let at = (h.start as isize + shift) as usize;
                            shift += h.insert.len() as isize - (h.end - h.start) as isize;
                            (at, at + h.insert.len())
                        })
                        .collect();
                    crate::differential::set_d1_extra(theirs);
                }
                let edits = vec![Edit::Text {
                    root: root.to_owned(),
                    hunks: c.hunks,
                }];
                st.verb_hint = Some("write --base".to_owned());
                let res =
                    self.commit_edit(st, i, edits, ChangeKind::Edit, None, now, Vec::new())?;
                let notes = self.edit_notes(st, i, &res, now, true);
                Ok(WriteReport {
                    outcome: WriteOutcome::Applied(res),
                    conflict_notes: Vec::new(),
                    notes,
                })
            }
        }
    }

    /// Undo the participant's own last `steps` batches, newest first, all or nothing (spec
    /// U13): [`UndoRequest::steps`] with [`UndoMode::All`].
    pub fn undo(&self, conn: ConnId, steps: usize) -> Result<UndoOutcome> {
        self.undo_with(conn, &UndoRequest::steps(steps))
    }

    /// Redo what the participant's undo undid, all or nothing.
    pub fn redo(&self, conn: ConnId, steps: usize) -> Result<UndoOutcome> {
        self.redo_with(conn, &UndoRequest::steps(steps))
    }

    /// Undo to the undo spec (U8–U18): the caller's own batches only, per **pid** across
    /// their connections, never anyone else's change. Each region (one canonical hunk) is
    /// three-way merged against the live text. [`UndoMode::All`] applies every requested
    /// step or nothing; [`UndoMode::Partial`] applies every clean region and reports the
    /// rest; [`UndoMode::Drop`] removes the top step unapplied. Needs edit.
    pub fn undo_with(&self, conn: ConnId, req: &UndoRequest) -> Result<UndoOutcome> {
        self.undo_redo(conn, req, Origin::Undo)
    }

    /// Redo (U17): reapplies exactly the regions the undo applied. A new edit by the
    /// participant clears their redo stack. Modes as [`Session::undo_with`].
    pub fn redo_with(&self, conn: ConnId, req: &UndoRequest) -> Result<UndoOutcome> {
        self.undo_redo(conn, req, Origin::Redo)
    }

    fn undo_redo(&self, conn: ConnId, req: &UndoRequest, origin: Origin) -> Result<UndoOutcome> {
        let now = self.now();
        let mut st = self.lock()?;
        let action = if origin == Origin::Undo {
            Action::Undo
        } else {
            Action::Redo
        };
        let i = self.admit_edit(&mut st, conn, action, now)?;
        let st = &mut *st;
        let pid = st.members[i].p.pid.clone();
        let h = st.hist.entry(pid.clone()).or_default();
        // an undo or redo closes the person's open group (U3 (f))
        h.open = None;
        let stack = if origin == Origin::Undo {
            &h.undo
        } else {
            &h.redo
        };
        let take = if req.mode == UndoMode::Drop {
            1
        } else {
            req.steps.max(1)
        };
        let picked: Vec<usize> = (0..stack.len())
            .rev()
            .filter(|&k| {
                req.for_person
                    .as_ref()
                    .map_or(true, |p| stack[k].on_behalf_of.as_ref() == Some(p))
            })
            .take(take)
            .collect();
        if picked.is_empty() {
            if h.cleared {
                h.cleared = false;
                return Ok(UndoOutcome::ClearedByPurge);
            }
            if origin == Origin::Undo && stack.is_empty() && self.history_lost(st, &pid)? {
                return Ok(UndoOutcome::HistoryLost);
            }
            return Ok(UndoOutcome::Nothing);
        }
        if req.mode == UndoMode::Drop {
            let b = Self::take_batches(st, &pid, origin, &picked).remove(0);
            return Ok(UndoOutcome::Dropped { change: b.seq });
        }
        let batches: Vec<Batch> = picked.iter().map(|&k| stack[k].clone()).collect();
        let mode = if req.mode == UndoMode::Partial {
            Mode::Partial
        } else {
            Mode::All
        };
        self.prov(st)?;
        let client = st.members[i].client;
        let next = ElemId::new(client, st.doc.clock_of(client));
        let prov = st.prov.as_ref().expect("just built");
        let (steps, blocked) = match undo::plan(&st.texts, prov, next, batches, mode) {
            Plan::Conflict { step, conflicts } => {
                return Ok(UndoOutcome::Conflicts { step, conflicts })
            }
            Plan::Steps { steps, blocked } if steps.is_empty() => {
                return Ok(UndoOutcome::Blocked {
                    conflicts: blocked.unwrap_or_default(),
                })
            }
            Plan::Steps { steps, blocked } => (steps, blocked),
        };
        let n = steps.len();
        let kind = if origin == Origin::Undo {
            ChangeKind::Undo
        } else {
            ChangeKind::Redo
        };
        let mut skipped = Vec::new();
        let mut inverses = Vec::with_capacity(n);
        let commit_steps: Vec<Step> = steps
            .into_iter()
            .map(|p| {
                skipped.extend(p.skipped);
                inverses.push(p.inverse);
                Step {
                    edits: p.edits,
                    kind,
                    of: None,
                    restores: Some(p.restores),
                }
            })
            .collect();
        let (result, done) = self.commit(st, i, commit_steps, now, Vec::new())?;
        let consumed = Self::take_batches(st, &pid, origin, &picked[..n]);
        for ((inv, d), b) in inverses.into_iter().zip(done).zip(consumed) {
            let batch = Batch {
                seq: d.map_or(st.version, |x| x.0),
                on_behalf_of: b.on_behalf_of,
                regions: inv,
                touched: false,
            };
            self.push_batch(st, &pid, batch, origin);
        }
        Ok(UndoOutcome::Applied {
            steps: n,
            result,
            skipped,
            blocked,
        })
    }

    /// Remove the batches at `idx` (newest first) from `pid`'s undo (or redo) stack;
    /// returns them in the same order.
    fn take_batches(st: &mut State, pid: &str, origin: Origin, idx: &[usize]) -> Vec<Batch> {
        let h = st.hist.entry(pid.to_owned()).or_default();
        let stack = if origin == Origin::Undo {
            &mut h.undo
        } else {
            &mut h.redo
        };
        let mut sorted: Vec<usize> = idx.to_vec();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        let mut taken: Vec<(usize, Batch)> = Vec::with_capacity(idx.len());
        for k in sorted {
            if let Some(b) = stack.remove(k) {
                taken.push((k, b));
            }
        }
        h.bytes = h.held_bytes();
        idx.iter()
            .filter_map(|k| {
                let at = taken.iter().position(|(j, _)| j == k)?;
                Some(taken.swap_remove(at).1)
            })
            .collect()
    }

    /// Whether `pid` changed this doc before the session loaded (their undo history was
    /// lost with the previous session, U18). Looks back at most 10,000 changes.
    fn history_lost(&self, st: &State, pid: &str) -> Result<bool> {
        if st.loaded_seq == 0 {
            return Ok(false);
        }
        let mut found = false;
        let mut seen = 0;
        self.store(st)?.scan_changes(self.doc_id, |c| {
            seen += 1;
            if c.seq <= st.loaded_seq && c.author == pid {
                found = true;
            }
            !found && seen < 10_000
        })?;
        Ok(found)
    }

    /// Clear every participant's undo and redo stacks (a purge does this, spec U19). Their
    /// next undo or redo says [`UndoOutcome::ClearedByPurge`]. Revert records are not
    /// touched here ([`Session::purge`] redacts them).
    pub fn clear_undo_history(&self) -> Result<()> {
        let mut st = self.lock()?;
        Self::clear_stacks(&mut st);
        Ok(())
    }

    /// `pid`'s undo and redo stack depths in this doc. Test-only (feature `conformance`,
    /// off by default): the undo spec's trace test compares them with its oracle's.
    #[cfg(feature = "conformance")]
    pub fn undo_depths(&self, pid: &str) -> Result<(usize, usize)> {
        let st = self.lock()?;
        Ok(st
            .hist
            .get(pid)
            .map_or((0, 0), |h| (h.undo.len(), h.redo.len())))
    }

    /// Clear every stack; returns the batches dropped.
    fn clear_stacks(st: &mut State) -> u64 {
        let mut n = 0;
        for h in st.hist.values_mut() {
            if !h.undo.is_empty() || !h.redo.is_empty() {
                h.cleared = true;
            }
            n += (h.undo.len() + h.redo.len()) as u64;
            h.undo.clear();
            h.redo.clear();
            h.bytes = 0;
        }
        n
    }

    fn normalize(&self, edits: Vec<Edit>) -> Result<Vec<Edit>> {
        let mut seen: Vec<String> = Vec::with_capacity(edits.len());
        let mut out = Vec::with_capacity(edits.len());
        for e in edits {
            let Edit::Text { root, hunks } = e;
            if seen.contains(&root) {
                return Err(SessionError::BadEdit(format!(
                    "root {root:?} appears twice in one edit; give all its hunks in one Edit"
                )));
            }
            seen.push(root.clone());
            let hunks: Vec<Hunk> = hunks
                .into_iter()
                .map(|h| {
                    let insert = match self.pack.normalize_input(&root, &h.insert) {
                        std::borrow::Cow::Borrowed(_) => h.insert,
                        std::borrow::Cow::Owned(s) => s,
                    };
                    Hunk { insert, ..h }
                })
                .filter(|h| !(h.start == h.end && h.insert.is_empty()))
                .collect();
            out.push(Edit::Text { root, hunks });
        }
        Ok(out)
    }

    /// Refuse a new on-behalf edit while the doc's pending on-behalf records are over
    /// their cap (K-HIST-23): `held: approvals_backlog`, so revertability is never lost.
    fn check_backlog(&self, st: &State, i: usize) -> Result<()> {
        if st.members[i].p.on_behalf_of.is_none() {
            return Ok(());
        }
        let stats = self.store(st)?.revert_stats(self.doc_id)?;
        let c = &self.config;
        if stats.pending_count >= c.pending_max_records
            || stats.pending_bytes >= c.pending_max_bytes
        {
            return Err(SessionError::Held {
                reason: "approvals_backlog",
            });
        }
        Ok(())
    }

    /// [`Session::commit`] of one edit step, then the batch onto the caller's undo stack.
    #[allow(clippy::too_many_arguments)]
    fn commit_edit(
        &self,
        st: &mut State,
        i: usize,
        edits: Vec<Edit>,
        kind: ChangeKind,
        of: Option<u64>,
        now: u64,
        versions: Vec<Version>,
    ) -> Result<EditResult> {
        self.commit_edit_restoring(st, i, edits, None, kind, of, now, versions)
    }

    /// [`Session::commit_edit`] of a step that puts elements back (a revert: per edit, per
    /// hunk, the roots it restores, U23).
    #[allow(clippy::too_many_arguments)]
    fn commit_edit_restoring(
        &self,
        st: &mut State,
        i: usize,
        edits: Vec<Edit>,
        restores: Option<Vec<Option<Vec<Ids>>>>,
        kind: ChangeKind,
        of: Option<u64>,
        now: u64,
        versions: Vec<Version>,
    ) -> Result<EditResult> {
        // a person's grouped edit (U3, K4): what it may join, read before it lands
        let group = st
            .group_hint
            .take()
            .filter(|_| kind == ChangeKind::Edit && restores.is_none())
            .filter(|_| st.members[i].p.kind != ParticipantKind::Agent)
            .map(|token| (token, self.group_join(st, i, token, &edits, now)));
        let step = Step {
            edits,
            kind,
            of,
            restores,
        };
        let (res, mut done) = self.commit(st, i, vec![step], now, versions)?;
        if let Some((seq, regions)) = done.pop().flatten() {
            let pid = st.members[i].p.pid.clone();
            let batch = Batch {
                seq,
                on_behalf_of: st.members[i].p.on_behalf_of.clone(),
                regions,
                touched: false,
            };
            match group {
                Some((token, join)) => self.push_grouped(st, i, token, join, batch, now),
                None => self.push_batch(st, &pid, batch, Origin::Edit),
            }
        }
        Ok(res)
    }

    /// A version of `texts` at log position `seq`, with the authors since the last one.
    fn version_of(
        st: &State,
        seq: Seq,
        reason: VersionReason,
        name: Option<String>,
        texts: &[(&'static str, String)],
        now: u64,
    ) -> Version {
        let mut authors: Vec<String> = Vec::new();
        for (_, a) in st
            .authors
            .iter()
            .filter(|(s, _)| *s > st.last_version_seq && *s <= seq)
        {
            if !authors.contains(a) {
                authors.push(a.clone());
            }
        }
        Version {
            seq,
            at_ms: now,
            reason,
            name,
            authors,
            content: neutral(texts),
        }
    }

    /// Apply `steps` (each one change) as member `i`, log them with their change records
    /// (and `versions`, stored before them) in one durable transaction, and do the
    /// bookkeeping: attribution, carets, events, compaction. An agent's edit over
    /// [`Config::agent_version_lines`] lines or [`Config::agent_version_percent`] of the doc
    /// cuts a version of the text before it first (K-HIST-4). Returns, per step, its log
    /// position and regions (`None` when it changed nothing); undo stacks are the caller's.
    #[allow(clippy::type_complexity)]
    fn commit(
        &self,
        st: &mut State,
        i: usize,
        steps: Vec<Step>,
        now: u64,
        mut versions: Vec<Version>,
    ) -> Result<(EditResult, Vec<Option<(Seq, Vec<Region>)>>)> {
        prof!("session.commit");
        let n_steps = steps.len();
        let verb = st.verb_hint.take().unwrap_or_else(|| {
            kind_verb(steps.first().map_or(ChangeKind::Edit, |s| s.kind)).to_owned()
        });
        let agent_cut = n_steps == 1
            && steps[0].kind == ChangeKind::Edit
            && st.members[i].p.kind == ParticipantKind::Agent
            && st.last_version_seq != st.version;
        // the touched roots' text before everything (multi-step summary, agent versions)
        let orig: Vec<(&'static str, String)> = if n_steps > 1 || agent_cut {
            st.texts.clone()
        } else {
            Vec::new()
        };
        let version_before = st.version;
        let mut applied: Vec<Applied> = Vec::new();
        let mut which: Vec<Option<usize>> = Vec::with_capacity(n_steps);
        for step in steps {
            match self.apply_local(st, i, step.edits, step.restores) {
                Ok(Some(mut a)) => {
                    a.kind = step.kind;
                    a.of = step.of;
                    which.push(Some(applied.len()));
                    applied.push(a);
                }
                Ok(None) => which.push(None),
                // a doc the engine poisoned is reloaded, even when nothing was applied
                Err(e) if applied.is_empty() && !poisons(&e) => return Err(e),
                Err(e) => {
                    self.recover(st, &e.to_string());
                    return Err(e);
                }
            }
        }
        if applied.is_empty() {
            let res = EditResult {
                version: st.version,
                ..EditResult::default()
            };
            return Ok((res, which.into_iter().map(|_| None).collect()));
        }
        if agent_cut {
            let a = &applied[0];
            let lines = a.added.max(a.removed);
            let total: usize = orig.iter().map(|(_, t)| position::line_count(t)).sum();
            let c = &self.config;
            if lines > c.agent_version_lines
                || lines.saturating_mul(100) > total.saturating_mul(c.agent_version_percent)
            {
                let v = Self::version_of(
                    st,
                    version_before,
                    VersionReason::BeforeAgent,
                    None,
                    &orig,
                    now,
                );
                versions.insert(0, v);
            }
        }
        let pid = st.members[i].p.pid.clone();
        let m = &st.members[i];
        let (client, via, obo) = (m.client, m.p.via.clone(), m.p.on_behalf_of.clone());
        let mut entries = Vec::with_capacity(applied.len());
        let mut changes = Vec::with_capacity(applied.len());
        for a in &mut applied {
            entries.push(LogEntry {
                author: pid.clone(),
                client,
                at_ms: now,
                via: via.clone(),
                update: std::mem::take(&mut a.update),
                on_behalf_of: obo.clone(),
            });
            changes.push(NewChange {
                info: ChangeInfo {
                    of: a.of,
                    on_behalf_of: obo.clone(),
                    lines_added: a.added as u64,
                    lines_removed: a.removed as u64,
                    ..ChangeInfo::new(a.kind, &pid, &via, now)
                },
                record: RevertRecord {
                    author: pid.clone(),
                    on_behalf_of: obo.clone(),
                    at_ms: now,
                    roots: std::mem::take(&mut a.record),
                },
                span: a.span.map(|(start, end)| ClockSpan { client, start, end }),
                provenance: std::mem::take(&mut a.prov_rows),
                // D1 widened it: the store keeps what the channel reports (v0.9.7)
                authored: a.widened.then(|| a.roots.clone()),
            });
        }
        let appended = {
            prof!("commit.store");
            self.store(st)
                .and_then(|s| Ok(s.commit_changes(self.doc_id, &entries, &changes, &versions)?))
        };
        let seqs = match appended {
            Ok(s) => s,
            Err(e) => {
                self.recover(st, &e.to_string());
                return Err(e);
            }
        };
        if let Some(ix) = st.reverts.as_mut() {
            prof!("commit.bind_reverts");
            ix.bind(&seqs);
        }
        if let Some(c) = st.disk.as_mut().and_then(|d| d.catch_up.as_mut()) {
            c.extend(entries.iter().map(|e| e.update.clone()));
        }
        if let Some(v) = versions.iter().map(|v| v.seq).max() {
            st.last_version_seq = st.last_version_seq.max(v);
        }
        st.last_change_ms = now;
        // bookkeeping
        let mut changed = Vec::new();
        let (mut added, mut removed) = (0, 0);
        let mut last_end = None;
        let mut regions_of = Vec::with_capacity(applied.len());
        for ((a, seq), e) in applied.into_iter().zip(&seqs).zip(&entries) {
            prof!("commit.bookkeeping");
            st.log_entries += 1;
            st.log_bytes += e.update.len() as u64;
            st.state_bytes += e.update.len() as u64;
            st.authors.push_back((*seq, pid.clone()));
            if st.authors.len() > AUTHORS_KEPT {
                st.authors.pop_front();
            }
            for (root, hunks) in &a.roots {
                let ev = ChangeEvent {
                    version: *seq,
                    pid: pid.clone(),
                    client,
                    via: via.clone(),
                    root: root.clone(),
                    hunks: hunks.clone(),
                    at_ms: now,
                    on_behalf_of: obo.clone(),
                };
                Self::broadcast(st, &ev);
            }
            if n_steps == 1 {
                changed = a.changed;
                (added, removed) = (a.added, a.removed);
            }
            last_end = a.last_end.or(last_end);
            regions_of.push(Some((*seq, a.regions)));
        }
        st.version = *seqs.last().expect("one seq per entry");
        Self::mark_dirty(st, now);
        if n_steps > 1 {
            for (root, before) in &orig {
                let after = &self.text_of(st, root)?.1;
                if before != after {
                    let hs = diff::hunks(before, after);
                    changed.extend(summary::changed_lines(root, after, &hs));
                    let (a, r) = summary::line_counts(before, after, &hs);
                    added += a;
                    removed += r;
                }
            }
        }
        let span = changed_span(&changed);
        let edited = span.as_ref().and_then(|l| {
            let t = self.text_of(st, &l.root).ok()?.1;
            let (s, e, _) = awareness::line_span(t, l.first, l.last)?;
            Some((l.root.clone(), s, e))
        });
        let m = &mut st.members[i];
        m.last_edit_ms = Some(now);
        if edited.is_some() {
            m.edited = edited;
        }
        if let Some((root, at)) = last_end {
            m.cursor = Some(Cursor {
                root,
                anchor: at,
                head: at,
            });
        }
        {
            prof!("commit.end_claims");
            self.end_edited_claims(st, now);
        }
        {
            prof!("commit.activity");
            self.log_activity(st, i, ActivityKind::Edit, &verb, span, now, None);
        }
        {
            prof!("commit.compact");
            self.maybe_compact(st);
        }
        let done = which
            .into_iter()
            .map(|w| w.and_then(|k| regions_of[k].take()))
            .collect();
        Ok((
            EditResult {
                version: st.version,
                changed,
                lines_added: added,
                lines_removed: removed,
            },
            done,
        ))
    }

    /// Put a batch on `pid`'s undo stack (an edit, a redo) or redo stack (an undo), within
    /// [`Config::undo_depth`] and [`Config::undo_max_bytes`] per pid (oldest first, U6). A
    /// new edit clears the redo stack (U7).
    fn push_batch(&self, st: &mut State, pid: &str, batch: Batch, origin: Origin) {
        let c = &self.config;
        let h = st.hist.entry(pid.to_owned()).or_default();
        let History {
            undo, redo, bytes, ..
        } = h;
        if origin == Origin::Edit {
            redo.clear();
        }
        let dst = if origin == Origin::Undo {
            &mut *redo
        } else {
            &mut *undo
        };
        dst.push_back(batch);
        while dst.len() > c.undo_depth {
            dst.pop_front();
        }
        // a region's size changes as it is carried (theirs, U10 v1.7): counted afresh
        *bytes = undo.iter().chain(redo.iter()).map(Batch::bytes).sum();
        // over the byte bound: drop this pid's oldest batches, either stack
        while *bytes > c.undo_max_bytes {
            let from_undo = match (undo.front(), redo.front()) {
                (Some(u), Some(r)) => u.seq <= r.seq,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };
            let s = if from_undo { &mut *undo } else { &mut *redo };
            *bytes -= s.pop_front().map_or(0, |b| b.bytes());
        }
        h.cleared = false;
        // any push closes the open group; a grouped edit opens it again (U3 (e), K4)
        h.open = None;
    }

    /// Apply one change to the in-memory doc and carry every stored position through it.
    /// Hunks are validated and made canonical (touching hunks coalesced) first. Nothing is
    /// logged here. `Ok(None)` when it changes nothing; an error changes nothing.
    ///
    /// `restores` (an undo, redo or revert): per edit, per hunk, the roots of the elements it
    /// puts back (U23); every other insertion is fresh, its own root. The provenance map
    /// follows exactly: the new elements' IDs are the member's next clocks in the engine
    /// adapter's order ([`crate::engine::insert_clocks`]), and each restored run gets a
    /// provenance row ([`Applied::prov_rows`]).
    fn apply_local(
        &self,
        st: &mut State,
        i: usize,
        edits: Vec<Edit>,
        restores: Option<Vec<Option<Vec<Ids>>>>,
    ) -> Result<Option<Applied>> {
        prof!("session.apply_local");
        let client = st.members[i].client;
        let pid = st.members[i].p.pid.clone();
        // D1 applies to a fresh change: no roots given (undo, redo and revert put back
        // exact elements and are left as they are)
        let fresh = restores.is_none();
        let mut canon = Vec::with_capacity(edits.len());
        let mut roots_of: Vec<Option<Vec<Ids>>> = Vec::with_capacity(edits.len());
        let mut restores = restores.map(Vec::into_iter);
        for e in edits {
            prof!("session.validate");
            let Edit::Text { root, hunks } = e;
            let (_, text) = self.text_of(st, &root)?;
            diff::validate(text, &hunks).map_err(DocError::from)?;
            let r = restores.as_mut().and_then(Iterator::next).flatten();
            let (hunks, r) = match r {
                Some(r) if r.len() == hunks.len() => {
                    let (h, r) = coalesce_with(hunks, r);
                    (h, Some(r))
                }
                Some(_) => {
                    debug_assert!(false, "restored roots don't fit the hunks");
                    (diff::coalesce(hunks), None)
                }
                None => (diff::coalesce(hunks), None),
            };
            canon.push(Edit::Text { root, hunks });
            roots_of.push(r);
        }
        // the map is needed to name what the change deletes (built once per load)
        {
            prof!("session.prov_build");
            self.prov(st)?;
        }
        // D1 (undo spec v1.10, the authored-ranges amendment): a fresh change is never
        // built from someone else's text. Inside the tokens its hunks touch, the characters
        // others wrote are rewritten (deleted and inserted afresh) with it. The engine, the
        // provenance map, undo regions and the revert record take the widened hunks; the
        // hunk channel takes the authored ones (E-LIVE r2.1), which give the same text.
        let mut channel: Vec<Option<Vec<Hunk>>> = vec![None; canon.len()];
        if fresh {
            self.owners(st)?;
            let tokens = self.d1_tokens();
            let extra = take_d1_extra();
            for (k, e) in canon.iter_mut().enumerate() {
                let Edit::Text { root, hunks } = e;
                if hunks.is_empty() {
                    continue;
                }
                let text = self.text_of(st, root)?.1;
                let prov = st.prov.as_ref().expect("built above");
                let owners = st.owners.as_ref().expect("built above");
                let wide = d1_hunks(text, hunks, tokens, prov, root, owners, &pid, &extra);
                if wide != *hunks {
                    channel[k] = Some(std::mem::replace(hunks, wide));
                }
            }
        }
        let clock_before = st.doc.clock_of(client);
        let Some(update) = st.doc.edit_as(client, &canon)? else {
            return Ok(None);
        };
        {
            prof!("session.engine_checks");
            self.report_engine_checks(st, st.version + 1);
        }
        let firsts = crate::engine::insert_clocks(clock_before, &canon);
        let mut a = Applied {
            span: Some((clock_before, st.doc.clock_of(client))),
            update,
            roots: Vec::new(),
            regions: Vec::new(),
            record: Vec::new(),
            prov_rows: Vec::new(),
            widened: false,
            kind: ChangeKind::Edit,
            of: None,
            changed: Vec::new(),
            added: 0,
            removed: 0,
            last_end: None,
        };
        let mut prov = st.prov.take().expect("built above");
        for (((e, restored), firsts), authored) in
            canon.into_iter().zip(roots_of).zip(firsts).zip(channel)
        {
            let Edit::Text { root, hunks } = e;
            if hunks.is_empty() {
                continue;
            }
            // what the channel and the summaries report: the authored hunks (D1)
            a.widened |= authored.is_some();
            let told = authored.unwrap_or_else(|| hunks.clone());
            let (ti, _) = self.text_of(st, &root)?;
            let before = std::mem::take(&mut st.texts[ti].1);
            let after = diff::apply(&before, &hunks);
            debug_assert_eq!(Ok(after.as_str()), st.doc.text(&root).as_deref());
            // per hunk: the roots it deletes, what it inserts (its IDs, the roots it puts
            // back), and the provenance rows of what it puts back
            let mut pairs = Vec::with_capacity(hunks.len());
            let mut ins = Vec::with_capacity(hunks.len());
            for (k, h) in hunks.iter().enumerate() {
                prof!("session.prov_roots");
                let id = ElemId::new(client, firsts[k]);
                let deleted = prov.roots(&root, &before, h.start, h.end);
                let put_back = restored
                    .as_ref()
                    .map(|r| r[k].clone())
                    .filter(|r| !h.insert.is_empty() && !r.is_empty());
                let inserted = match &put_back {
                    Some(r) => {
                        let mut off = 0u32;
                        for run in r.runs() {
                            a.prov_rows.push(ProvRow {
                                client,
                                start: id.clock + off,
                                end: id.clock + off + run.units,
                                root: (run.start.client, run.start.clock),
                            });
                            off += run.units;
                        }
                        r.clone()
                    }
                    None => Ids::fresh(&h.insert, id),
                };
                pairs.push((deleted, inserted));
                ins.push(Inserted {
                    id,
                    roots: put_back,
                });
            }
            {
                prof!("session.prov_apply");
                prov.apply(&root, &before, &hunks, &ins);
            }
            {
                prof!("session.record");
                a.record.push(record_root(
                    &root,
                    &before,
                    after.is_empty(),
                    &hunks,
                    Some(&pairs),
                ));
                a.regions
                    .extend(Region::of_change(&root, &before, &hunks, Some(pairs)));
            }
            {
                prof!("session.claims");
                // the owner's own edit touching a claim ends it once durable (unless held)
                let fates = awareness::claim_fates(&st.aware.claims, &root, &told, &pid);
                for (c, f) in st.aware.claims.iter_mut().zip(&fates) {
                    if c.pid == pid && !c.hold && f.touched {
                        c.edited = true;
                    }
                }
                // ... and the reserved waiter's own edit there uses its reservation
                for r in st.aware.reservations.iter_mut() {
                    if r.pid == pid && r.touched_by(&root, &told) {
                        r.edited = true;
                    }
                }
            }
            {
                prof!("session.carry");
                Self::carry(st, &root, &told, &hunks, &pid);
            }
            prof!("session.summary");
            a.changed
                .extend(summary::changed_lines(&root, &after, &told));
            let (ad, rm) = summary::line_counts(&before, &after, &told);
            a.added += ad;
            a.removed += rm;
            let delta: isize = told
                .iter()
                .map(|h| h.insert.len() as isize - (h.end - h.start) as isize)
                .sum();
            let last = told.last().expect("non-empty");
            a.last_end = Some((root.clone(), (last.end as isize + delta) as usize));
            st.texts[ti].1 = after;
            a.roots.push((root, told));
        }
        st.prov = Some(prov);
        prof!("session.revert_stage");
        // the revert index has been carried past this change: add its own regions
        if let Some(ix) = st.reverts.as_mut() {
            let r: Vec<(String, usize, usize)> = a
                .regions
                .iter()
                .map(|g| (g.root.clone(), g.start, g.end))
                .collect();
            ix.stage(&r);
        }
        Ok(Some(a))
    }

    /// Each client ID's owner, from the store's client table, built on first need: the one
    /// place the session reads ownership from (D1, and agent client-ID reuse).
    fn owners(&self, st: &mut State) -> Result<()> {
        if st.owners.is_none() {
            let o = self
                .store(st)?
                .clients_of(self.doc_id)?
                .into_iter()
                .collect();
            st.owners = Some(o);
        }
        Ok(())
    }

    /// The pack's token rule for D1, or the test-only mutant's.
    fn d1_tokens(&self) -> crate::text::lines::Tokens {
        #[cfg(any(test, feature = "differential"))]
        if crate::differential::d1_mutant() == crate::differential::D1Mutant::RewriteWholeLine {
            return crate::text::lines::Tokens::Lines;
        }
        self.pack.tokens()
    }

    /// The provenance map, built on first need ([`Self::rebuild_prov`]): O(doc) once per
    /// load (and after a purge or a recovery), then kept exact by every change the session
    /// applies, its own and outside edits alike.
    fn prov<'a>(&self, st: &'a mut State) -> Result<&'a mut Prov> {
        if st.prov.is_none() {
            st.prov = Some(self.rebuild_prov(st)?);
        }
        Ok(st.prov.as_mut().expect("just built"))
    }

    /// The provenance map from scratch, from the engine's blame runs (each live element's
    /// ID: a throwaway replica, O(doc)) and the store's provenance rows (the roots of
    /// restored elements). The cold path, and the oracle blame's runtime check and the
    /// differential test compare the kept map with. Changes nothing.
    fn rebuild_prov(&self, st: &State) -> Result<Prov> {
        let store = self.store(st)?.clone();
        Self::prov_from(self.doc_id, &st.doc, &st.texts, &store)
    }

    /// [`Self::rebuild_prov`] from a doc and its texts that needn't be the session's (blame's
    /// off-lock check builds from a copy of the doc, v0.9.7). Provenance rows are written
    /// once, with the change that inserts their elements, and only deleted with the doc, so
    /// the store answers the same for a doc's elements whenever it is read after the doc's
    /// changes were committed.
    pub(super) fn prov_from(
        doc_id: DocId,
        doc: &Doc,
        texts: &[(&'static str, String)],
        store: &DocStore,
    ) -> Result<Prov> {
        let broken = |why: &str| SessionError::Doc(DocError::Apply(format!("provenance: {why}")));
        let mut built = Vec::with_capacity(texts.len());
        for (root, text) in texts {
            let runs = doc.blame_runs(root)?;
            let mut q = Vec::with_capacity(runs.len());
            let mut at = 0usize;
            for r in &runs {
                let e = at + r.bytes;
                let t = text
                    .get(at..e)
                    .ok_or_else(|| broken("the engine's runs don't fit the text"))?;
                q.push((r.client, r.clock, r.clock + t.encode_utf16().count() as u32));
                at = e;
            }
            if at != text.len() {
                return Err(broken("the engine's runs don't cover the text"));
            }
            let rows = store.provenance_rows(doc_id, &q)?;
            built.push((
                *root,
                text.as_str(),
                super::prov::live_runs(text, &runs, &rows),
            ));
        }
        Ok(Prov::build(built))
    }

    /// Carry every stored position through a change to `root` by `typist`: carets, views,
    /// claims and reservations through its text hunks (`hunks`, the authored ones, as the
    /// channel reports them), undo regions and revert records through its element hunks
    /// (`elems`: D1's widened hunks, v0.9.7) (who takes text typed where two participants'
    /// claims meet: [`awareness::claim_fates`]).
    fn carry(st: &mut State, root: &str, hunks: &[Hunk], elems: &[Hunk], typist: &str) {
        for m in &mut st.members {
            if let Some(c) = m.cursor.as_mut().filter(|c| c.root == root) {
                c.anchor = position::transform(c.anchor, hunks, Bias::After);
                c.head = position::transform(c.head, hunks, Bias::After);
            }
            if let Some((_, s, e)) = m.view.as_mut().filter(|v| v.0 == root) {
                *s = position::transform(*s, hunks, Bias::Before);
                *e = position::transform(*e, hunks, Bias::After).max(*s);
            }
            if let Some((_, s, e)) = m.edited.as_ref().filter(|v| v.0 == root) {
                match awareness::carry_edit_span(*s, *e, hunks) {
                    Some(span) => {
                        let ed = m.edited.as_mut().expect("just read");
                        (ed.1, ed.2) = span;
                    }
                    None => m.edited = None,
                }
            }
        }
        let fates = awareness::claim_fates(&st.aware.claims, root, hunks, typist);
        for (c, f) in st.aware.claims.iter_mut().zip(fates) {
            if c.root == root {
                c.carry(hunks, f);
            }
        }
        for r in st.aware.reservations.iter_mut().filter(|r| r.root == root) {
            r.carry(hunks);
        }
        // undo regions and revert records are carried at element identity: through the
        // change as the engine made it (D1's widened hunks), so a region whose elements
        // were rewritten sees them go
        for h in st.hist.values_mut() {
            for b in h.undo.iter_mut().chain(h.redo.iter_mut()) {
                b.transform(root, elems);
            }
        }
        if let Some(ix) = st.reverts.as_mut() {
            ix.carry(root, elems);
        }
    }

    /// Send an event to every subscriber; one that is full or gone is cut off.
    fn broadcast(st: &mut State, ev: &ChangeEvent) {
        let handles = &mut st.handles;
        st.subs.retain(|(_, app, tx)| {
            let ev = match app {
                Some(v) => handles.lens(v).change(ev.clone()),
                None => ev.clone(),
            };
            match tx.try_send(ev) {
                Ok(()) => true,
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
            }
        });
    }

    /// `out` as `conn` may see it: an App viewer's (K5) goes through its handles (`f` for the
    /// value; a refusal naming someone else, by handle). Anyone else's is unchanged.
    pub(super) fn for_viewer<T>(
        &self,
        conn: ConnId,
        out: Result<T>,
        f: impl FnOnce(&mut super::handles::Lens<'_>, T) -> T,
    ) -> Result<T> {
        let Ok(mut st) = self.lock() else {
            return out;
        };
        let Some(i) = st.members.iter().position(|m| m.conn == conn) else {
            return out;
        };
        let Some(v) = Self::app_viewer(&st, i) else {
            return out;
        };
        let mut lens = st.handles.lens(&v);
        match out {
            Ok(t) => Ok(f(&mut lens, t)),
            Err(e) => Err(lens.error(e)),
        }
    }

    /// A tool run's output (`run_tool_with`) as `conn` may see it: an App viewer's (K5) names
    /// the others by handle. Its notes are lensed where they are made; the rest here.
    pub(crate) fn tool_output_as_seen(
        &self,
        conn: ConnId,
        out: std::result::Result<crate::tools::ToolOutput, crate::tools::ToolRunError>,
    ) -> std::result::Result<crate::tools::ToolOutput, crate::tools::ToolRunError> {
        use crate::tools::ToolRunError;
        let Ok(mut st) = self.lock() else {
            return out;
        };
        let Some(i) = st.members.iter().position(|m| m.conn == conn) else {
            return out;
        };
        let Some(v) = Self::app_viewer(&st, i) else {
            return out;
        };
        let mut l = st.handles.lens(&v);
        match out {
            Ok(mut o) => {
                if let Some(f) = o.fence.as_mut() {
                    f.contributors = std::mem::take(&mut f.contributors)
                        .into_iter()
                        .map(|p| l.pid(&p))
                        .collect();
                }
                if let Some(fe) = o.for_edit.take() {
                    o.for_edit = Some(Box::new(match *fe {
                        awareness::ForEdit::Claimed(c) => awareness::ForEdit::Claimed(l.claim(c)),
                        awareness::ForEdit::Conflict(c) => awareness::ForEdit::Conflict(l.claim(c)),
                        awareness::ForEdit::NotTaken(e) => awareness::ForEdit::NotTaken(l.error(e)),
                    }));
                }
                Ok(o)
            }
            Err(ToolRunError::Session(e)) => Err(ToolRunError::Session(l.error(e))),
            Err(e) => Err(e),
        }
    }

    /// `q` with an App viewer's handles (`--by p2`, `--on-behalf-of p1`) turned back into
    /// pids (K5); anyone else's unchanged.
    pub(super) fn unhandle_query(&self, conn: ConnId, q: &LogQuery) -> LogQuery {
        let mut q = q.clone();
        let Ok(mut st) = self.lock() else {
            return q;
        };
        let Some(i) = st.members.iter().position(|m| m.conn == conn) else {
            return q;
        };
        let Some(v) = Self::app_viewer(&st, i) else {
            return q;
        };
        let l = st.handles.lens(&v);
        q.by = q.by.map(|b| l.unpid(&b));
        q.on_behalf_of = q.on_behalf_of.map(|b| l.unpid(&b));
        q
    }

    /// Member `i`'s pid when it is an App viewer (it sees handles, never pids: K5).
    fn app_viewer(st: &State, i: usize) -> Option<String> {
        let m = &st.members[i];
        (m.p.kind == ParticipantKind::App).then(|| m.p.pid.clone())
    }

    /// Distinct pids with presence (not watching).
    fn participant_count(st: &State) -> usize {
        let mut p: Vec<&str> = st
            .members
            .iter()
            .filter(|m| !m.watcher)
            .map(|m| m.p.pid.as_str())
            .collect();
        p.sort_unstable();
        p.dedup();
        p.len()
    }

    /// Distinct pids watching past `participants_max` (K11).
    fn watching_count(st: &State) -> usize {
        let mut p: Vec<&str> = st
            .members
            .iter()
            .filter(|m| m.watcher)
            .map(|m| m.p.pid.as_str())
            .collect();
        p.sort_unstable();
        p.dedup();
        p.len()
    }

    /// After a failed store write: rebuild from the store so memory matches what is durable.
    /// Carets, views and undo history are reset (they were moved by a change that didn't
    /// land). If the reload fails too, the session closes.
    fn recover(&self, st: &mut State, error: &str) {
        let reloaded = (|| -> Result<_> {
            let store = self.store(st)?;
            let loaded = store.load(self.doc_id)?;
            let mut doc = Doc::from_store(self.schema, &loaded)?;
            doc.set_edit_check_every(self.config.edit_check_every);
            doc.set_edit_check_scale(self.config.edit_check_scale_units);
            doc.set_position_check_every(self.config.position_check_every);
            let texts = texts_of(&doc, &self.schema)?;
            let snap = loaded.snapshot.as_ref().map_or(0, |(s, _)| *s);
            let version = loaded.updates.last().map_or(snap, |(s, _)| *s);
            Ok((doc, texts, version, loaded))
        })();
        match reloaded {
            Ok((doc, texts, version, loaded)) => {
                st.doc = doc;
                st.texts = texts;
                st.version = version;
                st.log_entries = loaded.updates.len() as u64;
                st.log_bytes = loaded
                    .updates
                    .iter()
                    .map(|(_, e)| e.update.len() as u64)
                    .sum();
                for m in &mut st.members {
                    m.cursor = None;
                    m.view = None;
                    m.edited = None;
                }
                self.end_all_claims(st, awareness::ClaimEnd::Reset);
                for h in st.hist.values_mut() {
                    h.undo.clear();
                    h.redo.clear();
                    h.bytes = 0;
                }
                st.loaded_seq = version;
                st.reverts = None;
                st.prov = None;
                self.reload_disk(st);
                self.emit(AuditEvent::Recovered {
                    key: st.key.clone(),
                    error: error.to_owned(),
                });
            }
            Err(e) => {
                st.closed = Some(SessionError::Broken(format!(
                    "{error}; reloading from the store failed: {e}"
                )));
                st.store = None;
                st.subs.clear();
                st.roster.close();
            }
        }
    }

    fn maybe_compact(&self, st: &mut State) {
        let c = &self.config;
        if st.log_entries >= c.compact_entries || st.log_bytes >= c.compact_bytes {
            self.compact(st);
        }
    }

    fn compact(&self, st: &mut State) {
        // never compact from a doc that can't be trusted: reload it instead
        let state = match st.doc.encode_state() {
            Ok(s) => s,
            Err(e) => {
                self.emit(AuditEvent::CompactFailed {
                    key: st.key.clone(),
                    error: e.to_string(),
                });
                self.recover(st, &e.to_string());
                return;
            }
        };
        let res = self
            .store(st)
            .and_then(|s| Ok(s.compact(self.doc_id, st.version, &state)?));
        match res {
            Ok(()) => {
                st.log_entries = 0;
                st.log_bytes = 0;
                st.state_bytes = state.len() as u64;
            }
            Err(e) => self.emit(AuditEvent::CompactFailed {
                key: st.key.clone(),
                error: e.to_string(),
            }),
        }
    }

    // ----- approvals (on-behalf changes) -----

    /// Settle the approval of change `seq`, made by an assistant agent on behalf of a person:
    /// `Approved` or `Declined`, once. Only that person may do it: any other connection gets
    /// [`SessionError::Forbidden`] with [`Need::Approver`] (and an [`AuditEvent::Refused`]).
    /// Needs read. The change itself is never held or undone by this; a declined change is
    /// reversed only by an explicit edit, a restore, or the agent's own undo.
    ///
    /// Errors: [`SessionError::NoApproval`] when `seq` wasn't made on anyone's behalf;
    /// [`SessionError::BadApproval`] for `Pending`, or for a change already settled the other
    /// way. Settling it again the same way returns the record unchanged.
    pub fn set_approval(&self, conn: ConnId, seq: Seq, state: ApprovalState) -> Result<Approval> {
        let out = self.set_approval_inner(conn, seq, state);
        self.for_viewer(conn, out, |l, a| l.approval(a))
    }

    fn set_approval_inner(&self, conn: ConnId, seq: Seq, state: ApprovalState) -> Result<Approval> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::Approve, now)?;
        let pid = st.members[i].p.pid.clone();
        let store = self.store(&st)?.clone();
        let Some(cur) = store.approval(self.doc_id, seq)? else {
            return Err(SessionError::NoApproval { seq });
        };
        if cur.on_behalf_of != pid {
            self.emit(AuditEvent::Refused {
                key: st.key.clone(),
                pid,
                action: Action::Approve,
                need: Need::Approver,
            });
            return Err(SessionError::Forbidden {
                need: Need::Approver,
            });
        }
        if state == ApprovalState::Pending {
            return Err(SessionError::BadApproval(
                "an approval can only be settled as approved or declined".into(),
            ));
        }
        match cur.state {
            ApprovalState::Pending => {}
            s if s == state => return Ok(cur),
            s => {
                return Err(SessionError::BadApproval(format!(
                    "change {seq} was already settled as {s:?}"
                )))
            }
        }
        let new = cur.settled(state, pid.clone(), now);
        store.set_approval(self.doc_id, seq, &new)?;
        self.emit(AuditEvent::Approval {
            key: st.key.clone(),
            seq,
            pid,
            state,
        });
        Ok(new)
    }

    /// The approval record of change `seq` (`None` when it wasn't made on anyone's behalf).
    /// Needs read; and [`Rights::history`] unless the caller is the change's author or the
    /// person it was made for.
    pub fn approval(&self, conn: ConnId, seq: Seq) -> Result<Option<Approval>> {
        let out = self.approval_inner(conn, seq);
        self.for_viewer(conn, out, |l, v| v.map(|a| l.approval(a)))
    }

    fn approval_inner(&self, conn: ConnId, seq: Seq) -> Result<Option<Approval>> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::ReadApproval, now)?;
        let pid = st.members[i].p.pid.clone();
        let a = self.store(&st)?.approval(self.doc_id, seq)?;
        if a.as_ref()
            .is_some_and(|a| a.author == pid || a.on_behalf_of == pid)
        {
            return Ok(a);
        }
        self.admit(&mut st, conn, Need::History, Action::ReadApproval, now)?;
        Ok(a)
    }

    /// Approval records of this doc, oldest change first (`log --pending`). With
    /// [`Rights::history`], all of them; without, only those where the caller is the author
    /// or the person the change was made for. Needs read.
    pub fn approvals(&self, conn: ConnId) -> Result<Vec<(Seq, Approval)>> {
        let out = self.approvals_inner(conn);
        self.for_viewer(conn, out, |l, v| {
            v.into_iter().map(|(id, a)| (id, l.approval(a))).collect()
        })
    }

    fn approvals_inner(&self, conn: ConnId) -> Result<Vec<(Seq, Approval)>> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::ReadApproval, now)?;
        let (pid, history) = (st.members[i].p.pid.clone(), st.members[i].rights.history);
        let mut all = self.store(&st)?.approvals(self.doc_id)?;
        if !history {
            all.retain(|(_, a)| a.author == pid || a.on_behalf_of == pid);
        }
        Ok(all)
    }

    /// Tests only: who can write as which client ID (the one-writer invariant,
    /// `one_writer_tests`): the live doc's clocks with each client's owner in the store, the
    /// disk replica's clocks and the outside-edit client (when the doc has a file), and the
    /// present connections' clients.
    #[cfg(test)]
    pub(crate) fn writers_for_tests(&self) -> Result<super::one_writer_tests::Writers> {
        let st = self.lock()?;
        let store = self.store(&st)?.clone();
        let live = st
            .doc
            .clocks_for_tests()
            .into_iter()
            .map(|(c, k)| Ok((c, k, store.client_owner(self.doc_id, c)?)))
            .collect::<Result<Vec<_>>>()?;
        Ok(super::one_writer_tests::Writers {
            live,
            replica: st.disk.as_ref().map(|d| d.replica.clocks_for_tests()),
            external: st.disk.as_ref().and_then(|d| d.external_client),
            external_owned: store.client_owned_by(self.doc_id, EXTERNAL_PID)?,
            members: st
                .members
                .iter()
                .map(|m| (m.p.pid.clone(), m.client))
                .collect(),
        })
    }

    /// Test builds only (the `differential` feature): the disk sync state the session holds in
    /// memory, as bytes: the disk replica's encoded state, then each update it is still to
    /// catch up with (v0.9.7). Empty without a file. The erasure tests scan it.
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn disk_memory_for_tests(&self) -> Result<Vec<Vec<u8>>> {
        let st = self.lock()?;
        let Some(d) = st.disk.as_ref() else {
            return Ok(Vec::new());
        };
        let mut out = vec![d.replica.encode_state()?];
        out.extend(d.catch_up.iter().flatten().cloned());
        Ok(out)
    }

    /// Tests only: lose the last update the disk replica is to catch up with (a bug the
    /// write-back's check must catch).
    #[cfg(test)]
    pub(crate) fn lose_catch_up_for_tests(&self) -> Result<bool> {
        let mut st = self.lock()?;
        Ok(st
            .disk
            .as_mut()
            .and_then(|d| d.catch_up.as_mut())
            .and_then(|c| c.pop())
            .is_some())
    }

    /// Tests only: the live doc's encoded state, the disk replica's, whether the replica is
    /// in step (its catch-up list known), and the stored synced state.
    #[cfg(test)]
    pub(crate) fn writeback_state_for_tests(&self) -> Result<WritebackState> {
        let st = self.lock()?;
        let live = st.doc.encode_state()?;
        let replica = match st.disk.as_ref() {
            Some(d) => Some(d.replica.encode_state()?),
            None => None,
        };
        let known = st.disk.as_ref().is_some_and(|d| d.catch_up.is_some());
        let stored = self
            .store(&st)?
            .sync_states(self.doc_id)?
            .0
            .map(|s| s.state);
        Ok((live, replica, known, stored))
    }

    /// Tests only: how many times the disk replica was brought forward by its catch-up list
    /// (no reload), write-backs and outside saves alike.
    #[cfg(test)]
    pub(crate) fn replica_caught_up_for_tests(&self) -> Result<u64> {
        Ok(self.lock()?.disk.as_ref().map_or(0, |d| d.caught_up))
    }

    /// Tests only: make the live doc's engine cache hold a wrong text for `root`.
    #[cfg(test)]
    pub(crate) fn corrupt_text_cache_for_tests(&self, root: &str, text: &str) -> Result<()> {
        self.lock()?.doc.corrupt_text_cache_for_tests(root, text);
        Ok(())
    }

    /// Tests only: apply a raw update to the live doc, as a bug or a damaged log would.
    #[cfg(test)]
    pub(crate) fn apply_raw_for_tests(&self, update: &[u8]) -> Result<()> {
        let mut st = self.lock()?;
        st.prov = None;
        if let Some(d) = st.disk.as_mut() {
            d.catch_up = None;
        }
        Ok(st.doc.apply_trusted(update)?)
    }

    // ----- for the manager -----

    /// The pack's own verb `name` ([`Pack::verbs`]), when `name` isn't a kit verb.
    pub(crate) fn pack_verb(&self, name: &str) -> Option<crate::tools::PackVerb> {
        if crate::tools::is_kit_verb(name) {
            return None;
        }
        self.pack.verbs().iter().find(|v| v.name() == name).copied()
    }

    pub(crate) fn schema_name(&self) -> &'static str {
        self.schema.name
    }

    pub(crate) fn last_used_ms(&self) -> u64 {
        self.last_used_ms.load(Ordering::Relaxed)
    }

    /// (participants, memory estimate), or `None` once closed.
    pub(crate) fn pool_info(&self) -> Option<(usize, u64)> {
        let st = self.lock().ok()?;
        Some((st.members.len(), self.estimate(&st)))
    }

    /// Rename under the session's lock, so no edit interleaves with the store's rekey.
    pub(crate) fn rekey(&self, new: DocKey) -> Result<()> {
        let mut st = self.lock()?;
        self.store(&st)?.rekey(self.doc_id, new.clone())?;
        st.key = new;
        Ok(())
    }

    /// The host closes this session ([`super::SessionManager::close`]), members or not: what
    /// the roster owes is delivered, every subscription gets [`RosterEvent::Closed`] last, the
    /// audit names the connections, then it closes as an unload does (a pending write-back
    /// written, claims ended); later calls fail with [`SessionError::Closed`]. Returns the
    /// notes still waiting, for the manager to keep.
    pub(crate) fn close_by_host(
        &self,
        reason: &'static str,
    ) -> HashMap<String, Vec<awareness::Note>> {
        {
            let Ok(mut st) = self.lock_any() else {
                return HashMap::new();
            };
            if st.closed.is_some() {
                return HashMap::new();
            }
            let now = self.host.now_ms();
            self.roster_flush(&mut st, now, true);
            st.roster.end_with(RosterEvent::Closed { reason });
            let connections = st
                .members
                .iter()
                .map(|m| (m.conn, m.p.pid.clone()))
                .collect();
            self.emit(AuditEvent::SessionClosed {
                key: st.key.clone(),
                connections,
                reason,
            });
        }
        self.close(SessionError::Closed { reason }, false)
    }

    /// Close this handle because its doc is being deleted: no write-back, no compaction (the
    /// rows are about to go). Emits [`AuditEvent::DocDeleted`] with the connections that
    /// were in it; later calls fail with [`SessionError::Deleted`].
    pub(crate) fn close_deleted(&self) {
        let Ok(mut st) = self.lock_any() else {
            return; // poisoned: it is dropped with the pool entry
        };
        if st.closed.is_some() {
            return;
        }
        let connections = st
            .members
            .iter()
            .map(|m| (m.conn, m.p.pid.clone()))
            .collect();
        self.emit(AuditEvent::DocDeleted {
            key: st.key.clone(),
            connections,
        });
        st.closed = Some(SessionError::Deleted);
        st.store = None;
        st.subs.clear();
        st.roster.close();
        st.members.clear();
    }

    /// Close this handle: optionally compact a long log first, then drop the store handle
    /// and subscribers. Later calls fail with `reason`.
    ///
    /// Returns the notes still waiting ([`awareness::NoteKind::Overwritten`]), for the manager
    /// to hand to the next session of this doc: an unload isn't a reason to lose them.
    pub(crate) fn close(
        &self,
        reason: SessionError,
        compact_long_log: bool,
    ) -> HashMap<String, Vec<awareness::Note>> {
        let Ok(mut st) = self.lock_any() else {
            return HashMap::new(); // poisoned: nothing safe to do; it is dropped with the pool entry
        };
        if st.closed.is_some() {
            return HashMap::new();
        }
        // write back what the file doesn't have yet (an error is reported and kept: the
        // changes are in the log, and the next load writes them)
        if st.disk.as_ref().is_some_and(|d| d.dirty_since.is_some()) {
            let now = self.host.now_ms();
            let _ = self.write_back(&mut st, now);
        }
        if compact_long_log
            && st.log_entries > 0
            && st.log_entries >= self.config.unload_compact_entries
        {
            self.compact(&mut st);
        }
        // claims end (waiting hosts are told), then the presence extras go with the session
        self.end_all_claims(&mut st, awareness::ClaimEnd::Left);
        let waiting = std::mem::take(&mut st.aware.pending);
        st.aware = Aware::default();
        st.closed = Some(reason);
        st.store = None;
        st.subs.clear();
        st.roster.close();
        st.members.clear();
        waiting
    }
}

// ----- disk sync -----

impl Session {
    // --- public: the host reports file events, the kit checks and writes ---

    /// The host's watcher saw the file change (or the host wants a check now): read it and
    /// merge an outside edit in, if there is one. A file holding the kit's own last write
    /// is ignored (an echo). Never writes the file. `Unchanged` for a session without a
    /// file.
    ///
    /// An outside edit is the difference between the file's previous text (the last synced
    /// state) and its new text, applied as [`EXTERNAL_PID`] (`via: "disk"`), with a version
    /// cut before it ([`VersionReason::BeforeExternal`], when the doc moved since the last
    /// version) and after it ([`VersionReason::External`]). Edits not yet written back are
    /// kept: the next write-back writes the merged doc.
    pub fn file_changed(&self) -> Result<DiskCheck> {
        let r = self.file_changed_in();
        self.shown(r)
    }

    fn file_changed_in(&self) -> Result<DiskCheck> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        self.disk_check(&mut st, now, true)
    }

    /// The stat backup: like [`Session::file_changed`], but reads the file only when its
    /// size, mtime or inode changed since the kit last saw it, or when that last look was
    /// inside the file's mtime tick (git's racy rule: an equal stat can't vouch for a save
    /// made within the filesystem's mtime granularity of it).
    pub fn check_disk(&self) -> Result<DiskCheck> {
        let r = self.check_disk_in();
        self.shown(r)
    }

    fn check_disk_in(&self) -> Result<DiskCheck> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        self.disk_check(&mut st, now, false)
    }

    /// The host paired a rename of this doc's file: from now on it lives at `new_path`
    /// (inside the same root; re-checked like [`super::SessionManager::open_file`]). Then
    /// checks it like [`Session::file_changed`]. Rename the doc's key with
    /// [`super::SessionManager::rekey`].
    pub fn file_moved(&self, new_path: &Path) -> Result<DiskCheck> {
        let r = self.file_moved_in(new_path);
        self.shown(r)
    }

    fn file_moved_in(&self, new_path: &Path) -> Result<DiskCheck> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        let st = &mut *st;
        let Some(d) = st.disk.as_mut() else {
            return Err(SessionError::BadPath("this session has no file".into()));
        };
        match disk::probe(new_path, &d.root) {
            Ok(Probe::File { .. }) => {}
            Ok(p) => return Err(SessionError::BadPath(probe_problem(new_path, &p))),
            Err(e) => return Err(SessionError::Disk(format!("{}: {e}", new_path.display()))),
        }
        d.path = new_path.to_owned();
        d.stat = None;
        self.disk_check(st, now, true)
    }

    /// `flush`: write the doc to its file now if the file doesn't hold it (after merging any
    /// outside edit first). Needs read. `written` is false when there was nothing to write,
    /// or nothing could be written (`disk` says why: missing or declined).
    pub fn flush(&self, conn: ConnId) -> Result<FlushOutcome> {
        let r = self.flush_in(conn);
        self.shown(r)
    }

    fn flush_in(&self, conn: ConnId) -> Result<FlushOutcome> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Flush, now)?;
        let written = self.write_back(&mut st, now)?;
        Ok(FlushOutcome {
            written,
            hash: hex(&hmac(&st.hash_key, &self.render(&st)?)),
            disk: Self::disk_state(&st),
        })
    }

    /// `r` as the host may show it: an error's paths made safe (K7): inside this file's
    /// root, relative; elsewhere, `<outside>`.
    fn shown<T>(&self, r: Result<T>) -> Result<T> {
        r.map_err(|e| {
            let root = self
                .lock_any()
                .ok()
                .and_then(|st| st.disk.as_ref().map(|d| d.root.clone()));
            match root {
                Some(root) => e.redacted(&root),
                None => e,
            }
        })
    }

    /// The identity (device and inode) of the file this session syncs with (K9, K-DISK-14):
    /// a host keys sessions by it as well as by the path. `None` without a file, when the
    /// file is gone, or where the platform has none.
    pub fn file_identity(&self) -> Result<Option<super::disk::FileIdentity>> {
        let path = self.lock()?.disk.as_ref().map(|d| d.path.clone());
        Ok(path.and_then(|p| super::disk::file_identity(&p).ok().flatten()))
    }

    /// The file this session syncs with, as the host gave it.
    pub fn file_path(&self) -> Result<Option<PathBuf>> {
        Ok(self.lock()?.disk.as_ref().map(|d| d.path.clone()))
    }

    /// The lost-edit audit's sync check (K-AUD-1, v0.9.4): when the doc is synced with its
    /// file ([`DiskState::Clean`]), the disk replica (the doc as the file holds it, on which
    /// the next outside edit is made) must hold the doc's text **on the doc's elements**.
    /// Same text on other elements is the stale base that corrupted an outside edit after
    /// a change that moved no byte (fixed in v0.9.4): the file and the doc agree, so
    /// comparing them can't see it. Returns what differs, by root (metadata only); empty
    /// when nothing does, or when the session has no file or isn't clean. Never changes
    /// anything. O(doc): a blame of each root on the doc and on the replica.
    ///
    /// First, in any disk state (v0.9.5): neither the doc nor the replica may hold an
    /// update it couldn't integrate ([`SyncDrift::Pending`]; O(1)).
    pub fn audit_disk_sync(&self) -> Result<Vec<SyncDrift>> {
        let st = self.lock()?;
        let mut out = Vec::new();
        if st.doc.unresolved() {
            out.push(SyncDrift::Pending { replica: false });
        }
        let Some(d) = st.disk.as_ref() else {
            return Ok(out);
        };
        if d.replica.unresolved() {
            out.push(SyncDrift::Pending { replica: true });
        }
        if !out.is_empty() || !matches!(Self::disk_state(&st), Some(DiskState::Clean)) {
            return Ok(out);
        }
        for (root, text) in &st.texts {
            if d.replica.text(root)? != *text {
                out.push(SyncDrift::Text {
                    root: (*root).to_owned(),
                });
                continue;
            }
            let ids = |doc: &Doc| {
                doc.blame_runs(root)
                    .ok()
                    .and_then(|runs| super::prov::elem_ids(text, &runs))
            };
            match (ids(&st.doc), ids(&d.replica)) {
                (Some(a), Some(b)) if a == b => {}
                _ => out.push(SyncDrift::Elements {
                    root: (*root).to_owned(),
                }),
            }
        }
        Ok(out)
    }

    /// Test-only (the soak's diagnosis, v0.9.7): the live doc's texts read afresh from the
    /// engine, against the session's cached texts: `(root, engine, cached)`.
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn engine_and_cached_texts_for_tests(&self) -> Result<Vec<(String, String, String)>> {
        let st = self.lock()?;
        let engine = st.doc.engine_texts_for_tests();
        Ok(engine
            .into_iter()
            .map(|(root, e)| {
                let cached = st
                    .texts
                    .iter()
                    .find(|(r, _)| *r == root)
                    .map(|(_, t)| t.clone())
                    .unwrap_or_default();
                (root.to_owned(), e, cached)
            })
            .collect())
    }

    /// Test-only (the soak's diagnosis, v0.9.7): `pid`'s last `last` undo and redo batches
    /// (seq, regions), most recent first.
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn undo_stacks_for_tests(&self, pid: &str, last: usize) -> Result<(String, String)> {
        let st = self.lock()?;
        let Some(h) = st.hist.get(pid) else {
            return Ok((String::new(), String::new()));
        };
        let show = |q: &VecDeque<super::undo::Batch>| {
            q.iter()
                .rev()
                .take(last)
                .map(|b| format!("{b:?}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok((show(&h.undo), show(&h.redo)))
    }

    /// Test-only: the live doc's history in UTF-16 units (the sum of its state vector's
    /// clocks), the measure the edit path's canary scales its interval by
    /// ([`Config::edit_check_scale_units`]; the soak prints it at each milestone, v0.9.7).
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn history_units_for_tests(&self) -> Result<u64> {
        Ok(self.lock()?.doc.history_units_for_tests())
    }

    /// Test-only: the live doc's position index counters since it was loaded (v0.9.7).
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn position_index_stats_for_tests(
        &self,
    ) -> Result<crate::differential::PositionIndexStats> {
        Ok(self.lock()?.doc.position_index_stats())
    }

    /// Test-only: plant a bug in the live doc's position index (v0.9.7), to prove the kit
    /// reports what the index's shadow check finds ([`AuditEvent::PositionIndexMismatch`]).
    /// Lasts until the doc is reloaded.
    #[cfg(feature = "planted-position-bugs")]
    #[doc(hidden)]
    pub fn plant_position_bug_for_tests(
        &self,
        bug: crate::differential::PlantedPositionBug,
    ) -> Result<()> {
        self.lock()?.doc.plant_position_bug(bug);
        Ok(())
    }

    /// Test-only (the soak's D1 check): the `(before, after)` texts of every region of
    /// `pid`'s top `steps` batches on its undo (or, `redo`, its redo) stack.
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn undo_regions_for_tests(
        &self,
        pid: &str,
        redo: bool,
        steps: usize,
    ) -> Result<Vec<(String, String)>> {
        let st = self.lock()?;
        let Some(h) = st.hist.get(pid) else {
            return Ok(Vec::new());
        };
        let q = if redo { &h.redo } else { &h.undo };
        Ok(q.iter()
            .rev()
            .take(steps)
            .flat_map(|b| {
                b.regions
                    .iter()
                    .map(|r| (r.before.clone(), r.after.clone()))
            })
            .collect())
    }

    /// Test-only: the disk replica's sync point and the doc's version, `(synced_seq,
    /// version)`: equal when the replica is up to date with the doc (the marker, v0.9.5).
    /// `None` without a file.
    #[cfg(any(test, feature = "differential"))]
    pub fn disk_sync_point_for_tests(&self) -> Result<Option<(Seq, Seq)>> {
        let st = self.lock()?;
        Ok(st.disk.as_ref().map(|d| (d.synced_seq, st.version)))
    }

    /// Test-only: make the disk replica hold `text` in `root` on fresh elements (a planted
    /// violation for [`Session::audit_disk_sync`]: its own text gives a stale base, another
    /// text a wrong one), and mark it out of date in memory, as a change since the sync
    /// would. Leaves the doc and the store as they are.
    #[cfg(any(test, feature = "differential"))]
    pub fn plant_disk_replica_for_tests(&self, root: &str, text: &str) -> Result<()> {
        let mut st = self.lock()?;
        let version = st.version;
        let d = st
            .disk
            .as_mut()
            .ok_or_else(|| SessionError::BadPath("this session has no file".into()))?;
        let old = d.replica.text(root)?;
        let hunks = vec![Hunk {
            start: 0,
            end: old.len(),
            insert: text.to_owned(),
        }];
        let client = loop {
            let c = random_client();
            if c > 1 {
                break c;
            }
        };
        d.replica.edit_as(
            client,
            &[Edit::Text {
                root: root.to_owned(),
                hunks,
            }],
        )?;
        // out of date, as a change since the sync would leave it (v0.9.5: the marker is the
        // change counter)
        d.synced_seq = version.wrapping_add(1);
        Ok(())
    }

    // --- for the manager ---

    /// Start syncing with the file at `path` (inside canonical `root`): load the sync state,
    /// settle a write-back a crash interrupted, then reconcile (an edit made while the doc
    /// wasn't loaded is merged in like any outside edit; changes the file never got are
    /// written by the next write-back).
    pub(crate) fn attach_file(&self, path: &Path, root: &Path) -> Result<DiskCheck> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        let st = &mut *st;
        if let Some(d) = &st.disk {
            if d.path == path && d.root == root {
                return self.disk_check(st, now, false);
            }
            return Err(SessionError::BadPath(format!(
                "doc is open at {}; report a rename with file_moved",
                d.path.display()
            )));
        }
        let store = self.store(st)?.clone();
        let (synced, pending) = store.sync_states(self.doc_id)?;
        let (replica, replica_bytes, hash, seq) = match synced {
            Some(s) => (
                self.checked(Doc::load(self.schema, &s.state)?),
                s.state.len() as u64,
                s.hash,
                s.seq,
            ),
            // a doc from before disk sync: take its content as what the file held
            None => {
                let state = st.doc.encode_state()?;
                (
                    self.checked(Doc::load(self.schema, &state)?),
                    state.len() as u64,
                    file_hash(&st.hash_key, &self.render(st)?),
                    st.version,
                )
            }
        };
        let key = st.hash_key;
        let mut d = Disk {
            path: path.to_owned(),
            root: root.to_owned(),
            replica,
            replica_bytes,
            synced_hash: hash,
            synced_seq: seq,
            own: VecDeque::new(),
            echoed: None,
            stat: None,
            missing: false,
            dirty_since: None,
            last_change_ms: now,
            last_check_ms: now,
            external_client: None,
            // in step only if the file's state is the live doc's
            catch_up: (seq == st.version).then(Vec::new),
            writebacks: 0,
            caught_up: 0,
        };
        let on_disk = match disk::probe(path, root) {
            Ok(Probe::File { real, .. }) => {
                disk::clean_temps(&real);
                match disk::read(&real, self.config.disk_read_max_bytes) {
                    Ok(ReadFile::Bytes(b, _)) => Some(b),
                    _ => None,
                }
            }
            _ => None,
        };
        // does `stored` (keyed, or a format-2 plain hash) name the file's bytes?
        let names_file = |stored: &[u8]| {
            on_disk
                .as_deref()
                .is_some_and(|b| stored == file_hash(&key, b) || legacy_matches(stored, b))
        };
        // a write-back a crash interrupted: did it land? (A format-2 plain hash is matched
        // here; a synced one is replaced by a keyed one when the check below reads the file.)
        if let Some(p) = pending {
            if names_file(&p.hash) {
                let mut meta = store.meta(self.doc_id)?;
                meta.synced_hash = Some(p.hash.clone());
                meta.synced_seq = p.seq;
                store.promote_pending_sync(self.doc_id, &meta)?;
                d.replica = self.checked(Doc::load(self.schema, &p.state)?);
                d.replica_bytes = p.state.len() as u64;
                d.synced_hash = p.hash;
                d.synced_seq = p.seq;
                d.catch_up = (p.seq == st.version).then(Vec::new);
            } else {
                store.clear_pending_sync(self.doc_id)?;
            }
        }
        st.disk = Some(d);
        let out = self.disk_check(st, now, true)?;
        // changes the file never got (a crash before write-back): write them soon
        let live = file_hash(&st.hash_key, &self.render(st)?);
        if st.disk.as_ref().is_some_and(|d| d.synced_hash != live) {
            Self::mark_dirty(st, now);
        } else {
            // the stored replica may predate a change that moved no byte (a crash before
            // its write-back, or a store from before v0.9.4): it becomes the doc
            self.resync_replica(st)?;
        }
        Ok(out)
    }

    /// One tick of the host's clock: the stat backup while anyone is present (every
    /// [`Config::disk_poll_ms`]), and the write-back when due. Returns what the check found
    /// (if it ran) and whether the file was written.
    pub(crate) fn disk_tick(&self) -> Result<(Option<DiskCheck>, bool)> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        let st = &mut *st;
        let c = &self.config;
        let Some(d) = &st.disk else {
            return Ok((None, false));
        };
        let check =
            if !st.members.is_empty() && now.saturating_sub(d.last_check_ms) >= c.disk_poll_ms {
                Some(self.disk_check(st, now, false)?)
            } else {
                None
            };
        let d = st.disk.as_ref().expect("checked above");
        let due = d.dirty_since.is_some_and(|since| {
            now.saturating_sub(since) >= c.write_max_delay_ms
                || now.saturating_sub(d.last_change_ms) >= c.write_quiet_ms
        }) && !d.missing
            && st.declined.is_none();
        let written = due && self.write_back(st, now)?;
        Ok((check, written))
    }

    // --- internals ---

    fn render(&self, st: &State) -> Result<Vec<u8>> {
        Ok(self.pack.encode(&neutral(&st.texts), &st.codec)?)
    }

    fn estimate(&self, st: &State) -> u64 {
        (st.state_bytes + st.disk.as_ref().map_or(0, |d| d.replica_bytes))
            * self.config.memory_factor
    }

    fn mark_dirty(st: &mut State, now: u64) {
        if let Some(d) = st.disk.as_mut() {
            d.dirty_since.get_or_insert(now);
            d.last_change_ms = now;
        }
    }

    fn disk_state(st: &State) -> Option<DiskState> {
        let d = st.disk.as_ref()?;
        Some(if let Some(r) = &st.declined {
            DiskState::Declined(r.clone())
        } else if d.missing {
            DiskState::Missing
        } else if let Some(since_ms) = d.dirty_since {
            DiskState::Dirty { since_ms }
        } else {
            DiskState::Clean
        })
    }

    fn disk_error(&self, st: &State, what: &str, e: impl std::fmt::Display) -> SessionError {
        let error = format!("{what}: {e}");
        self.emit(AuditEvent::DiskError {
            key: st.key.clone(),
            error: error.clone(),
        });
        SessionError::Disk(error)
    }

    /// Look at the file; read it when forced or when its stat changed; ingest an outside
    /// edit. Never writes the file.
    fn disk_check(&self, st: &mut State, now: u64, force: bool) -> Result<DiskCheck> {
        let Some(d) = st.disk.as_mut() else {
            return Ok(DiskCheck::Unchanged);
        };
        d.last_check_ms = now;
        let (path, root) = (d.path.clone(), d.root.clone());
        let (real, stat) = match disk::probe(&path, &root) {
            Ok(Probe::File { real, stat, .. }) => (real, stat),
            Ok(_) => return Ok(self.set_missing(st)),
            Err(e) => return Err(self.disk_error(st, &path.display().to_string(), e)),
        };
        if !force && !d.missing && d.stat.is_some_and(|s| s.vouches_for(stat)) {
            return Ok(DiskCheck::Unchanged);
        }
        let (bytes, stat) = match disk::read(&real, self.config.disk_read_max_bytes) {
            Ok(ReadFile::Bytes(b, s)) => (b, s),
            Ok(ReadFile::TooLarge(n)) => {
                d.stat = Some(disk::Seen::now(stat));
                d.missing = false;
                return self.decline(
                    st,
                    Declined {
                        code: "too_large",
                        message: format!("file is {n} bytes; live editing reads up to {}", {
                            self.config.disk_read_max_bytes
                        }),
                    },
                );
            }
            Err(e) => return Err(self.disk_error(st, &real.display().to_string(), e)),
        };
        let was_missing = std::mem::replace(&mut d.missing, false);
        d.stat = Some(disk::Seen::now(stat));
        let h = file_hash(&st.hash_key, &bytes);
        if h == d.synced_hash {
            if was_missing || st.declined.is_some() {
                self.restored(st)?;
            }
            return Ok(DiskCheck::Unchanged);
        }
        let ttl = self.config.own_write_ttl_ms;
        d.own.retain(|(_, at)| now.saturating_sub(*at) <= ttl);
        if d.own.iter().any(|(x, _)| *x == h) {
            // an older write of ours is back (an editor saved a stale buffer, or a late
            // echo): not an edit; the next write-back puts the doc back. Only writes made
            // since the last outside save count (the set is cleared by each: v0.9.5), and
            // every suppressed echo is reported
            if d.echoed.as_ref() != Some(&h) {
                d.echoed = Some(h);
                self.emit(AuditEvent::EchoSuppressed {
                    key: st.key.clone(),
                });
            }
            Self::mark_dirty(st, now);
            return Ok(DiskCheck::OwnWrite);
        }
        if was_missing {
            self.emit(AuditEvent::FileRestored {
                key: st.key.clone(),
            });
        }
        self.ingest(st, &bytes, h, now)
    }

    fn set_missing(&self, st: &mut State) -> DiskCheck {
        let d = st.disk.as_mut().expect("has disk");
        if !d.missing {
            d.missing = true;
            d.stat = None;
            let unflushed = d.dirty_since.is_some();
            self.emit(AuditEvent::FileMissing {
                key: st.key.clone(),
                unflushed,
            });
        }
        DiskCheck::Missing
    }

    fn set_declined(&self, st: &mut State, reason: Option<String>) -> Result<()> {
        let store = self.store(st)?;
        let mut meta = store.meta(self.doc_id)?;
        meta.declined = reason.clone();
        store.put_meta(self.doc_id, &meta)?;
        st.declined = reason;
        Ok(())
    }

    fn decline(&self, st: &mut State, why: Declined) -> Result<DiskCheck> {
        if st.declined.is_none() {
            self.set_declined(st, Some(why.to_string()))?;
            self.emit(AuditEvent::FileDeclined {
                key: st.key.clone(),
                code: why.code,
            });
        }
        Ok(DiskCheck::Declined(why))
    }

    /// The file is back as the doc knows it (after missing or declined).
    fn restored(&self, st: &mut State) -> Result<()> {
        if st.declined.is_some() {
            self.set_declined(st, None)?;
        }
        self.emit(AuditEvent::FileRestored {
            key: st.key.clone(),
        });
        Ok(())
    }

    fn external_client(&self, st: &mut State) -> Result<ClientId> {
        if let Some(c) = st.disk.as_ref().and_then(|d| d.external_client) {
            return Ok(c);
        }
        let store = self.store(st)?.clone();
        let c = match store.client_owned_by(self.doc_id, EXTERNAL_PID)? {
            Some(c) => c,
            None => {
                let c = fresh_client(&store, self.doc_id, |c| {
                    st.members.iter().any(|m| m.client == c)
                })?;
                store.set_client(self.doc_id, c, EXTERNAL_PID)?;
                c
            }
        };
        if let Some(d) = st.disk.as_mut() {
            d.external_client = Some(c);
        }
        // D1's owners: the external participant's own text is its own
        if let Some(o) = st.owners.as_mut() {
            o.insert(c, EXTERNAL_PID.to_owned());
        }
        Ok(c)
    }

    /// D1 for an outside save (undo spec v1.10; v0.9.7): each of `edits` (the merge's hunks
    /// on the live text, as the external participant's) widened in place to the tokens of
    /// the new text they lie in, keeping only the external participant's own characters
    /// inside. Returns, per root D1 widened, its authored hunks (canonical), which the
    /// channel reports.
    fn d1_outside(&self, st: &mut State, edits: &mut [Edit]) -> Result<Vec<(String, Vec<Hunk>)>> {
        let mut authored = Vec::new();
        if edits.is_empty() {
            return Ok(authored);
        }
        self.prov(st)?;
        self.owners(st)?;
        let tokens = self.d1_tokens();
        for e in edits.iter_mut() {
            let Edit::Text { root, hunks } = e;
            if hunks.is_empty() {
                continue;
            }
            let text = self.text_of(st, root)?.1;
            let prov = st.prov.as_ref().expect("built above");
            let owners = st.owners.as_ref().expect("built above");
            let told = diff::coalesce(hunks.clone());
            let wide = d1_hunks(text, &told, tokens, prov, root, owners, EXTERNAL_PID, &[]);
            if wide != told {
                *hunks = wide;
                authored.push((root.clone(), told));
            }
        }
        Ok(authored)
    }

    /// Merge the file's new content (`bytes`, hash `h`) into the live doc.
    ///
    /// The outside save is a three-way merge applied to the live doc (v0.9.5): base = the
    /// text the file held at the last sync (the disk replica's text), ours = the file's new
    /// text, theirs = the live text, dirty or not ([`merge::merge3_keep_both`]: `write
    /// --base`'s merge when clean, both kept where they overlap). Its hunks are on the live
    /// text and are applied to the live doc as they are, as the external participant. The
    /// replica's elements are never the base, so a change that moved no byte (the text put
    /// back on new elements), written back or not, can't misplace the save (since v0.1.0;
    /// v0.9.4 closed it only while the doc rendered the synced bytes).
    fn ingest(&self, st: &mut State, bytes: &[u8], h: Vec<u8>, now: u64) -> Result<DiskCheck> {
        let content = match self.pack.decode(bytes) {
            Ok(c) => c,
            Err(why) => return self.decline(st, why),
        };
        let roots = ordered_roots(&self.schema, content.roots)?;
        let store = self.store(st)?.clone();
        let mut meta = store.meta(self.doc_id)?;
        meta.codec = content.codec.clone();
        meta.declined = None;
        meta.synced_hash = Some(h.clone());
        let was_declined = st.declined.is_some();
        // per root: the save on the synced text (for the replica), and the merge on the live
        // text (for the doc)
        let mut file_edits = Vec::new();
        let mut edits = Vec::new();
        // the merge's own hunks, whole lines (for the notes: `live/removed.rs`)
        let mut merge_sides: Vec<(String, Vec<Hunk>)> = Vec::new();
        for (root, Neutral::Text(t)) in &roots {
            let synced = st.disk.as_ref().expect("has disk").replica.text(root)?;
            let saved = diff::hunks(&synced, t);
            if saved.is_empty() {
                continue;
            }
            let live = &st.texts.iter().find(|(r, _)| r == root).expect("a root").1;
            let merged = merge::merge3_keep_both(&synced, t, live);
            merge_sides.push((root.clone(), merged.hunks.clone()));
            // trimmed to the characters that change (v0.9.7): the merge's sides are whole
            // lines (Option B), a guarantee about the merged text; the change applied to the
            // doc is only what differs, so the elements a save keeps keep their authors (D1
            // then widens it to tokens, as for any diff-based change), and the text is the
            // same byte for byte
            let trimmed = diff::trim_hunks(live, &merged.hunks);
            // canonical (v0.9.7): what the engine is handed is exactly the change at element
            // identity, and a root is part of the change iff its hunks are non-empty (its
            // text may still read as before: text deleted and typed again elsewhere)
            let hunks = canonical_hunks(live, &lines::align_insertions(live, trimmed));
            if !hunks.is_empty() {
                edits.push(Edit::Text {
                    root: root.clone(),
                    hunks,
                });
            }
            file_edits.push(Edit::Text {
                root: root.clone(),
                hunks: lines::align_insertions(&synced, saved),
            });
        }
        if edits.is_empty() {
            // the same text in other bytes (line endings, BOM), or a save the doc already
            // holds: adopt the file's format; the doc doesn't change. No update to add to the
            // catch-up list: a replica that becomes the doc takes the ones it has (v0.9.7)
            let (state, held) = match self.replica_to_file(st, &roots, &file_edits) {
                Ok(s) => s,
                Err(e) => {
                    self.recover(st, &e.to_string());
                    return Err(e);
                }
            };
            let d = st.disk.as_mut().expect("has disk");
            if held {
                d.synced_seq = st.version;
            }
            meta.synced_seq = d.synced_seq;
            let sync = SyncState {
                hash: h.clone(),
                seq: d.synced_seq,
                state,
            };
            d.replica_bytes = sync.state.len() as u64;
            store.commit_disk(self.doc_id, st.version + 1, &[], &[], &[], &meta, &sync)?;
            d.synced_hash = h;
            // a person saved since the kit's recent writes: one of them coming back now is
            // their save, not an echo (v0.9.5)
            d.own.clear();
            d.echoed = None;
            st.codec = content.codec;
            if was_declined {
                st.declined = None;
                self.emit(AuditEvent::FileRestored {
                    key: st.key.clone(),
                });
            }
            self.settle_dirty(st, now)?;
            return Ok(DiskCheck::Unchanged);
        }
        let client = self.external_client(st)?;
        // D1 (undo spec v1.10, the authored-ranges amendment): an outside save is a
        // diff-based change, never built from someone else's text. Its authored hunks (the
        // merge's, on the live text) are widened to the tokens of the new text they lie in;
        // inside, only the external participant's own characters are kept. The channel,
        // carets, claims and the change summary take the authored hunks; the engine, the
        // record, undo regions and the revert index the widened ones (v0.9.7).
        let authored = self.d1_outside(st, &mut edits)?;
        // whose text this save may remove (read before it applies; never stops the save)
        let removed = self.outside_removals(st, &edits, &authored, &merge_sides, now);
        let before = st.texts.clone();
        let clock_before = st.doc.clock_of(client);
        // the change is reported at element identity (v0.9.3), never as a diff of the texts:
        // the merge's own hunks, which the engine applies to the live doc as given. One save in
        // `edit_check_every` also computes it in full from the engine (each edited root's
        // element IDs before and after) and compares (`AuditEvent::IdentityMismatch`).
        let every = u64::from(self.config.edit_check_every);
        st.outside_saves += 1;
        let check = every > 0 && (st.outside_saves - 1) % every == 0;
        let ids_before: Vec<(&'static str, Option<Vec<ElemId>>)> = before
            .iter()
            .filter(|_| check)
            .filter(|(root, _)| {
                edits
                    .iter()
                    .any(|e| matches!(e, Edit::Text { root: r, .. } if r == root))
            })
            .map(|(root, text)| {
                let ids = st
                    .doc
                    .blame_runs(root)
                    .ok()
                    .and_then(|runs| super::prov::elem_ids(text, &runs));
                (*root, ids)
            })
            .collect();
        // the provenance map follows the outside edit (below). On a sampled save the engine's
        // IDs just read also check the map's (v0.9.7); a map that differs (or isn't as long as
        // the text) is reported and dropped (the cold path rebuilds it on next need)
        let mut prov = st.prov.take();
        if let Some(p) = &prov {
            let wrong = if !p.aligned(&st.texts) {
                Some("outside edit: the map's length isn't the text's".to_owned())
            } else {
                ids_before.iter().find_map(|(root, ids)| {
                    let ids = ids.as_ref()?;
                    let text = &before.iter().find(|(r, _)| r == root)?.1;
                    let kept = Ids::from_runs(p.query(root, text, 0, text.len()).iter().map(|r| {
                        super::prov::IdRun {
                            bytes: r.bytes,
                            units: r.units,
                            start: r.id,
                        }
                    }));
                    (kept.chars(text) != *ids).then(|| {
                        format!("outside edit: root {root:?}: element IDs other than the engine's")
                    })
                })
            };
            if let Some(what) = wrong {
                self.emit(AuditEvent::ProvenanceMismatch {
                    key: st.key.clone(),
                    version: st.version,
                    what,
                });
                prov = None;
            }
        }
        // the merge is made on the live doc itself, as the external participant: changes made
        // since the last write-back are kept, and nothing is merged in from the replica
        let made = st
            .doc
            .edit_as(client, &edits)
            .map(|u| u.expect("non-empty edits change the doc"));
        let update = match made {
            Ok(u) => u,
            Err(e) => {
                self.recover(st, &e.to_string());
                return Err(e.into());
            }
        };
        self.report_engine_checks(st, st.version + 1);
        let after = texts_of(&st.doc, &self.schema)?;
        // the merged change is one of the updates the replica is to catch up with (v0.9.7):
        // when the doc now holds the file's text, `replica_to_file` brings the replica
        // forward with them, this one included. If the store commit below fails, `recover`
        // reloads the doc and the replica and forgets the list (`reload_disk`)
        if let Some(c) = st.disk.as_mut().and_then(|d| d.catch_up.as_mut()) {
            c.push(update.clone());
        }
        let (replica_state, _) = match self.replica_to_file(st, &roots, &file_edits) {
            Ok(s) => s,
            Err(e) => {
                self.recover(st, &e.to_string());
                return Err(e);
            }
        };
        let seq = st.version + 1;
        let mut versions = Vec::new();
        if st.last_version_seq != st.version {
            versions.push(Self::version_of(
                st,
                st.version,
                VersionReason::BeforeExternal,
                None,
                &before,
                now,
            ));
        }
        // the change as applied to the live doc, at element identity (SPEC-UNDO, preamble):
        // the merge puts the outside edit's elements where its hunks say, on the live doc
        // itself (`edit_as` deletes exactly the elements in each range and inserts new ones at
        // its start), so the merge's canonical hunks are that change (v0.9.7). Not a diff of
        // the texts: in repeated text that would name other elements (a region carried onto a
        // copy of its own line; v0.9.3, `tests/outside_edit_undo.rs`). Every root with hunks
        // is part of the change, its text changed or not.
        let mut mismatched = Vec::new();
        let mut map_wrong = false;
        let root_hunks: Vec<(&'static str, Vec<Hunk>)> = edits
            .iter()
            .map(|e| {
                let Edit::Text { root, hunks: given } = e;
                let (root, old) = before
                    .iter()
                    .find(|(r, _)| r == root)
                    .expect("an edited root");
                let new = &after.iter().find(|(r, _)| r == root).expect("a root").1;
                // the full computation from the engine's element IDs (on a sampled save, when
                // the IDs before the merge were taken)
                let full = |st: &State| {
                    let old_ids = ids_before
                        .iter()
                        .find(|(r, _)| r == root)
                        .and_then(|(_, ids)| ids.as_deref());
                    old_ids.and_then(|old_ids| {
                        let runs = st.doc.blame_runs(root).ok()?;
                        let new_ids = super::prov::elem_ids(new, &runs)?;
                        super::prov::identity_hunks(old, old_ids, new, &new_ids)
                    })
                };
                let hunks = if diff::apply(old, given) == *new {
                    // a sampled save: a difference is reported; the given hunks, which the
                    // engine applied, stay the record's, and the map (which would follow
                    // them) is dropped
                    if check && full(st).is_some_and(|f| f != *given) {
                        mismatched.push(*root);
                        map_wrong = true;
                    }
                    given.clone()
                } else {
                    // never on a correct merge: the full computation on a sampled save, else
                    // a diff of the texts (which keeps the text right); the map is dropped
                    mismatched.push(*root);
                    map_wrong = true;
                    full(st).unwrap_or_else(|| diff::hunks(old, new))
                };
                (*root, hunks)
            })
            .collect();
        for root in mismatched {
            self.emit(AuditEvent::IdentityMismatch {
                key: st.key.clone(),
                root: root.to_owned(),
                change: st.version + 1,
            });
        }
        // the provenance map follows the change as local edits do (`apply_local`), in
        // O(the change): each hunk's inserted elements are the external client's next clocks
        // in the engine's order, fresh (their own roots: an outside edit restores nothing)
        if map_wrong {
            prov = None;
        }
        // per root and hunk, the roots it deletes and the IDs it inserts, read before the map
        // moves on: the revert record keeps them as an edit's does (U23, v0.9.7.2; before, an
        // outside save's record kept no roots, so a revert of it put back fresh text)
        let mut pairs_of: Vec<(String, Vec<(Ids, Ids)>)> = Vec::new();
        if let Some(p) = prov.as_mut() {
            let firsts = crate::engine::insert_clocks(clock_before, &edits);
            // the hunks the engine was handed are canonical: its clocks are the predicted ones
            debug_assert_eq!(
                st.doc.clock_of(client) - clock_before,
                edits
                    .iter()
                    .flat_map(|e| {
                        let Edit::Text { hunks, .. } = e;
                        hunks.iter().map(|h| h.insert.encode_utf16().count() as u32)
                    })
                    .sum::<u32>(),
                "the outside edit's insert clocks"
            );
            for (e, firsts) in edits.iter().zip(&firsts) {
                let Edit::Text { root, hunks } = e;
                let old = &before.iter().find(|(r, _)| r == root).expect("a root").1;
                let pairs: Vec<(Ids, Ids)> = hunks
                    .iter()
                    .zip(firsts.iter())
                    .map(|(h, &clock)| {
                        (
                            p.roots(root, old, h.start, h.end),
                            Ids::fresh(&h.insert, ElemId::new(client, clock)),
                        )
                    })
                    .collect();
                pairs_of.push((root.clone(), pairs));
                let ins: Vec<Inserted> = firsts
                    .iter()
                    .map(|&clock| Inserted {
                        id: ElemId::new(client, clock),
                        roots: None,
                    })
                    .collect();
                p.apply(root, old, hunks, &ins);
            }
        }
        st.prov = prov;
        // per root, what the channel reports: the authored hunks where D1 widened the save
        // (the same text), else the change at element identity, as before
        let told_of = |root: &str, hunks: &[Hunk]| -> Vec<Hunk> {
            authored
                .iter()
                .find(|(r, _)| r == root)
                .map_or_else(|| hunks.to_vec(), |(_, h)| h.clone())
        };
        let mut info = ChangeInfo::new(ChangeKind::External, EXTERNAL_PID, "disk", now);
        let mut record = Vec::new();
        for (root, hunks) in &root_hunks {
            let old = &before.iter().find(|(r, _)| r == root).expect("a root").1;
            let new = &after.iter().find(|(r, _)| r == root).expect("a root").1;
            let (a, r) = summary::line_counts(old, new, &told_of(root, hunks));
            info.lines_added += a as u64;
            info.lines_removed += r as u64;
            // its hunks are at element identity (v0.9.3), and its record keeps their roots
            // (v0.9.7.2). Without a provenance map (an identity mismatch dropped it) it keeps
            // none, and a revert of it puts back fresh text, as before provenance (U22)
            let pairs = pairs_of
                .iter()
                .find(|(r, _)| r.as_str() == *root)
                .map(|(_, p)| p.as_slice())
                .filter(|p| p.len() == hunks.len());
            #[cfg(feature = "differential")]
            let pairs = pairs.filter(|_| {
                crate::differential::prov_mutant()
                    != Some(crate::differential::ProvMutant::OutsideSaveKeepsNoRoots)
            });
            record.push(record_root(root, old, new.is_empty(), hunks, pairs));
        }
        let change = NewChange {
            info,
            record: RevertRecord {
                author: EXTERNAL_PID.to_owned(),
                on_behalf_of: None,
                at_ms: now,
                roots: record,
            },
            span: Some(ClockSpan {
                client,
                start: clock_before,
                end: st.doc.clock_of(client),
            }),
            provenance: Vec::new(),
            // D1 widened it: the store keeps what the channel reports (v0.9.7)
            authored: (!authored.is_empty()).then(|| {
                root_hunks
                    .iter()
                    .map(|(root, hunks)| ((*root).to_owned(), told_of(root, hunks)))
                    .collect()
            }),
        };
        versions.push(Version {
            seq,
            at_ms: now,
            reason: VersionReason::External,
            name: None,
            authors: vec![EXTERNAL_PID.to_owned()],
            content: neutral(&after),
        });
        meta.synced_seq = seq;
        let sync = SyncState {
            hash: h.clone(),
            seq,
            state: replica_state,
        };
        let entry = LogEntry {
            author: EXTERNAL_PID.to_owned(),
            client,
            at_ms: now,
            via: "disk".to_owned(),
            update,
            on_behalf_of: None,
        };
        if let Err(e) = store.commit_disk(
            self.doc_id,
            seq,
            std::slice::from_ref(&entry),
            std::slice::from_ref(&change),
            &versions,
            &meta,
            &sync,
        ) {
            self.recover(st, &e.to_string());
            return Err(e.into());
        }
        // bookkeeping, as for any change
        let mut regions = Vec::new();
        for (root, hunks) in root_hunks {
            let told = told_of(root, &hunks);
            Self::carry(st, root, &told, &hunks, EXTERNAL_PID);
            let mut delta: isize = 0;
            for h in &hunks {
                let s0 = (h.start as isize + delta) as usize;
                regions.push((root.to_owned(), s0, s0 + h.insert.len()));
                delta += h.insert.len() as isize - (h.end - h.start) as isize;
            }
            Self::broadcast(
                st,
                &ChangeEvent {
                    version: seq,
                    pid: EXTERNAL_PID.to_owned(),
                    client,
                    via: "disk".to_owned(),
                    root: root.to_owned(),
                    hunks: told,
                    at_ms: now,
                    on_behalf_of: None,
                },
            );
        }
        if let Some(ix) = st.reverts.as_mut() {
            ix.stage(&regions);
            ix.bind(&[seq]);
        }
        st.last_version_seq = seq;
        st.last_change_ms = now;
        st.texts = after;
        st.version = seq;
        st.codec = content.codec;
        st.log_entries += 1;
        st.log_bytes += entry.update.len() as u64;
        st.state_bytes += entry.update.len() as u64;
        st.authors.push_back((seq, EXTERNAL_PID.to_owned()));
        if st.authors.len() > AUTHORS_KEPT {
            st.authors.pop_front();
        }
        let d = st.disk.as_mut().expect("has disk");
        d.synced_hash = h;
        d.synced_seq = seq;
        d.replica_bytes = sync.state.len() as u64;
        // a person saved since the kit's recent writes: one of them coming back now is their
        // save, not an echo (v0.9.5)
        d.own.clear();
        d.echoed = None;
        if was_declined {
            st.declined = None;
            self.emit(AuditEvent::FileRestored {
                key: st.key.clone(),
            });
        }
        self.emit(AuditEvent::ExternalEdit {
            key: st.key.clone(),
            seq,
        });
        self.finish_removals(st, removed, seq - 1, seq, now);
        self.settle_dirty(st, now)?;
        self.maybe_compact(st);
        Ok(DiskCheck::Ingested { version: seq })
    }

    /// After the sync point moved: dirty when the doc differs from the file, clean if not.
    fn settle_dirty(&self, st: &mut State, now: u64) -> Result<()> {
        let live = file_hash(&st.hash_key, &self.render(st)?);
        let d = st.disk.as_mut().expect("has disk");
        if live == d.synced_hash {
            d.dirty_since = None;
            self.resync_replica(st)?;
        } else {
            Self::mark_dirty(st, now);
        }
        Ok(())
    }

    /// [`Session::resync_replica`] if the doc renders to the synced bytes.
    fn resync_if_synced(&self, st: &mut State) -> Result<()> {
        let Some(d) = st.disk.as_ref() else {
            return Ok(());
        };
        if d.synced_hash.is_empty() || d.synced_seq == st.version {
            return Ok(());
        }
        if file_hash(&st.hash_key, &self.render(st)?) == d.synced_hash {
            self.resync_replica(st)?;
        }
        Ok(())
    }

    /// Bring the disk replica up to the doc, when the doc renders to the synced bytes (the
    /// caller checked) but has changed since the replica was synced: an edit and its undo, a
    /// delete and a retype, a rewrite and back, a restore or a revert to the file's text.
    /// The replica's text is then right but its elements aren't the doc's: the doc put the
    /// text back as new elements and deleted the old ones. An outside edit made on the
    /// replica would delete elements the doc had already deleted (no-ops) and insert beside
    /// the copy, corrupting the edit (since v0.1.0; fixed in v0.9.4).
    ///
    /// The marker is the doc's change counter (v0.9.5): the replica was synced at version
    /// `synced_seq` (every sync records it, and so does the stored sync state, so a restart
    /// keeps it), and any change since moves `version` past it, a change that only deletes
    /// included. v0.9.4 compared state vectors, which a delete-only change leaves equal: for
    /// text such a change can't leave the bytes as they were, but in a structured doc
    /// deleting an empty or absent cell can, and the stale replica stayed. So this is O(1)
    /// when nothing changed, one full encode when something did; the replica takes the
    /// changes since ([`Session::replica_to`], v0.9.7) instead of a reload. The re-synced
    /// replica is stored as the synced state (the write-back's two calls), so a restart
    /// can't bring the old one back; if storing fails it is reported, and the next load,
    /// which runs this again, heals it.
    fn resync_replica(&self, st: &mut State) -> Result<()> {
        let Some(d) = st.disk.as_ref() else {
            return Ok(());
        };
        if d.synced_seq == st.version {
            // the cross-check: synced at this version, the replica holds the doc's inserts
            debug_assert!(
                d.replica.same_state_vector(&st.doc),
                "the disk replica synced at the doc's version holds other inserts"
            );
            return Ok(());
        }
        let state = match st.doc.encode_state() {
            Ok(s) => s,
            Err(e) => {
                self.recover(st, &e.to_string());
                return Err(e.into());
            }
        };
        // the replica follows the doc as at a write-back (v0.9.7): the changes since
        // applied to it, or reloaded from `state` when those aren't known
        self.replica_to(st, &state)?;
        let d = st.disk.as_mut().expect("has disk");
        d.replica_bytes = state.len() as u64;
        d.synced_seq = st.version;
        let sync = SyncState {
            hash: d.synced_hash.clone(),
            seq: st.version,
            state,
        };
        let stored = self.store(st).and_then(|store| {
            store.set_pending_sync(self.doc_id, &sync)?;
            let mut meta = store.meta(self.doc_id)?;
            meta.synced_hash = Some(sync.hash.clone());
            meta.synced_seq = sync.seq;
            store.promote_pending_sync(self.doc_id, &meta)?;
            Ok(())
        });
        if let Err(e) = stored {
            self.disk_error(st, "recording the re-synced disk replica", e);
        }
        Ok(())
    }

    /// After an outside save is merged into the doc (v0.9.5): the disk replica takes the
    /// file's text (`file`, every root; `file_edits` the save on the replica's text). Returns
    /// its state, to be stored as the synced state, and whether it is the doc.
    /// - When the doc now holds the file's text, the replica is the doc: the same elements,
    ///   brought forward by the updates since its last sync ([`Session::replica_to`],
    ///   v0.9.7; the caller has added the merged save's), not reloaded from the encoding.
    /// - Otherwise (the doc holds changes the file doesn't have yet; the write-back follows)
    ///   the save is made on the replica as a client of its own, one the doc never holds.
    ///   The doc doesn't render the synced bytes until it changes again (which moves its
    ///   version past `synced_seq`) or is written back, and either replaces the replica
    ///   whole; nothing is ever merged from it into the doc. An incremental write-back must
    ///   likewise replace such a replica whole (its state vector names a client the doc
    ///   lacks).
    ///
    /// The replica is no longer the base an outside save is made on (only its text is), so
    /// this keeps K-AUD-1's sync check meaningful and the stored state small; it is the one
    /// place an ingest sets the replica.
    fn replica_to_file(
        &self,
        st: &mut State,
        file: &[(String, Neutral)],
        file_edits: &[Edit],
    ) -> Result<(Vec<u8>, bool)> {
        // the doc's texts as they are now (`st.texts` follows after the bookkeeping)
        let mut held = true;
        for (root, Neutral::Text(t)) in file {
            held &= st.doc.text(root)? == *t;
        }
        if held {
            // the stored synced state is the doc's
            let state = st.doc.encode_state()?;
            // the replica becomes the doc as a write-back makes it (v0.9.7): brought forward
            // by the updates the doc took since its last sync (the merged save's included),
            // reloaded from `state` only when those aren't known. Then in step
            self.replica_to(st, &state)?;
            return Ok((state, true));
        }
        let d = st.disk.as_mut().expect("has disk");
        // the replica takes a save the doc never holds (a client of its own, below): the next
        // write-back replaces it whole (v0.9.7), never brings it forward
        d.catch_up = None;
        if !file_edits.is_empty() {
            let own = loop {
                let c = random_client();
                if c > 1 && st.doc.clock_of(c) == 0 && d.replica.clock_of(c) == 0 {
                    break c;
                }
            };
            d.replica.edit_as(own, file_edits)?;
        }
        Ok((d.replica.encode_state()?, false))
    }

    /// Write the doc to its file if the file doesn't hold it. Merges an outside edit first,
    /// so the file is never overwritten with content the kit hasn't seen (except for one
    /// written between that read and the rename). Atomic; keeps the file's mode.
    fn write_back(&self, st: &mut State, now: u64) -> Result<bool> {
        if st.disk.is_none() || st.declined.is_some() {
            return Ok(false);
        }
        // the file holds the synced content unless it holds an older write of ours
        let stale = match self.disk_check(st, now, true)? {
            DiskCheck::Missing | DiskCheck::Declined(_) => return Ok(false),
            DiskCheck::OwnWrite => true,
            _ => false,
        };
        let bytes = self.render(st)?;
        let h = file_hash(&st.hash_key, &bytes);
        let d = st.disk.as_mut().expect("has disk");
        if h == d.synced_hash && !stale {
            // nothing to write, but the doc may have changed since the replica was synced
            // (a change that moved no byte): the replica becomes the doc (v0.9.4)
            d.dirty_since = None;
            self.resync_replica(st)?;
            return Ok(false);
        }
        let real = match disk::probe(&d.path, &d.root) {
            Ok(Probe::File { real, .. }) => real,
            Ok(_) => {
                self.set_missing(st);
                return Ok(false);
            }
            Err(e) => {
                let p = d.path.display().to_string();
                return Err(self.disk_error(st, &p, e));
            }
        };
        let state = match st.doc.encode_state() {
            Ok(s) => s,
            Err(e) => {
                self.recover(st, &e.to_string());
                return Err(e.into());
            }
        };
        let store = self.store(st)?.clone();
        let pending = SyncState {
            hash: h.clone(),
            seq: st.version,
            state,
        };
        store.set_pending_sync(self.doc_id, &pending)?;
        let stat = match disk::atomic_write(&real, &bytes) {
            Ok((stat, dir_sync)) => {
                if let Some(e) = dir_sync {
                    self.disk_error(st, "fsync of the file's folder", e);
                }
                stat
            }
            Err(e) => {
                let _ = store.clear_pending_sync(self.doc_id);
                return Err(self.disk_error(st, &real.display().to_string(), e));
            }
        };
        // the file is in place: memory follows whatever the store does (if recording it
        // fails, the next load sees the file matches the write in flight and promotes it)
        let promoted = store.meta(self.doc_id).and_then(|mut meta| {
            meta.synced_hash = Some(h.clone());
            meta.synced_seq = st.version;
            store.promote_pending_sync(self.doc_id, &meta)
        });
        if let Err(e) = promoted {
            self.disk_error(st, "recording the write-back", e);
        }
        // the file and the store are done: memory follows them, then any failure is reported
        let replica = self.replica_to(st, &pending.state);
        let d = st.disk.as_mut().expect("has disk");
        d.replica_bytes = pending.state.len() as u64;
        d.synced_hash = h.clone();
        d.synced_seq = st.version;
        d.own.push_back((h, now));
        d.echoed = None;
        while d.own.len() > OWN_WRITES_KEPT {
            d.own.pop_front();
        }
        d.stat = Some(disk::Seen::now(stat));
        d.dirty_since = None;
        replica?;
        Ok(true)
    }

    /// The disk replica becomes the live doc's state (`state`, its encoding), as a write-back
    /// or an outside save the doc now holds leaves it (v0.9.7). In step: the updates the live doc took since are applied to it,
    /// O(those changes). Otherwise (not known, an apply that fails, or the test-only switch
    /// to the old path) it is reloaded from `state`, O(the doc). Every time, the result's
    /// state vector is compared with the doc's (O(clients): an insert lost or foreign); one
    /// time in [`Config::writeback_check_every`] its deletions and texts are too (a lost
    /// delete; not the encodings, which may differ for equal content). A mismatch is
    /// reported ([`AuditEvent::DiskReplicaMismatch`]) and the replica reloaded. Fails only if
    /// `state` doesn't load (never on a correct stream: it was just encoded from the live
    /// doc); the changes since are then not known.
    fn replica_to(&self, st: &mut State, state: &[u8]) -> Result<()> {
        let key = st.key.clone();
        let version = st.version;
        let every = u64::from(self.config.writeback_check_every);
        let d = st.disk.as_mut().expect("has disk");
        d.writebacks += 1;
        #[cfg(any(test, feature = "differential"))]
        let old_path = crate::differential::full_writeback();
        #[cfg(not(any(test, feature = "differential")))]
        let old_path = false;
        let caught_up = match d.catch_up.take() {
            Some(updates) if !old_path => updates
                .iter()
                .try_for_each(|u| d.replica.apply_trusted(u))
                .is_ok(),
            _ => false,
        };
        let mut reload = !caught_up;
        // every time, O(clients): the replica holds exactly the doc's inserts (a lost or
        // foreign insert shows); one in `every`, O(doc): the same deletions and texts too (a
        // lost delete). Not the encodings: upstream may split equal content into blocks
        // differently on the two docs.
        if caught_up
            && (!d.replica.same_state_vector(&st.doc)
                || (every != 0 && d.writebacks % every == 0 && !d.replica.same_content(&st.doc)))
        {
            reload = true;
            self.emit(AuditEvent::DiskReplicaMismatch { key, version });
        }
        if !reload {
            st.disk.as_mut().expect("has disk").caught_up += 1;
        }
        if reload {
            match Doc::load(self.schema, state) {
                Ok(r) => {
                    let d = st.disk.as_mut().expect("has disk");
                    d.replica = self.checked(r);
                }
                // never on a correct stream; the next sync tries again from scratch
                Err(e) => {
                    st.disk.as_mut().expect("has disk").catch_up = None;
                    return Err(e.into());
                }
            }
        }
        st.disk.as_mut().expect("has disk").catch_up = Some(Vec::new());
        Ok(())
    }

    /// After a store failure: the disk replica and format back to what the store holds.
    fn reload_disk(&self, st: &mut State) {
        // the live doc was reloaded: the replica is not known to be in step with it, whatever
        // follows (an update already applied to the replica may not have been stored)
        if let Some(d) = st.disk.as_mut() {
            d.catch_up = None;
        }
        let Some(store) = st.store.clone() else {
            return;
        };
        if let Ok(meta) = store.meta(self.doc_id) {
            st.codec = meta.codec;
            st.declined = meta.declined;
        }
        if let (Some(d), Ok((Some(s), _))) = (st.disk.as_mut(), store.sync_states(self.doc_id)) {
            if let Ok(r) = Doc::load(self.schema, &s.state).map(|d| self.checked(d)) {
                d.replica = r;
                d.replica_bytes = s.state.len() as u64;
                d.synced_hash = s.hash;
                d.synced_seq = s.seq;
            }
        }
    }
}

fn probe_problem(path: &Path, p: &Probe) -> String {
    match p {
        Probe::File { .. } => format!("{} is a file", path.display()),
        Probe::Missing => format!("{} does not exist", path.display()),
        Probe::NotFile => format!("{} is not a regular file", path.display()),
        Probe::Outside(r) => format!(
            "{} resolves to {}, outside the root",
            path.display(),
            r.display()
        ),
    }
}

pub(crate) fn probe_error(path: &Path, p: &Probe) -> SessionError {
    SessionError::BadPath(probe_problem(path, p))
}

/// What a loaded or imported doc starts from.
struct Init {
    doc_id: DocId,
    meta: DocMeta,
    doc: Doc,
    version: Seq,
    /// Log since the snapshot: (entries, bytes).
    log: (u64, u64),
    state_bytes: u64,
    authors: VecDeque<(Seq, String)>,
    now: u64,
}

/// One change for [`Session::commit`].
struct Step {
    edits: Vec<Edit>,
    kind: ChangeKind,
    /// A restore's version, a revert's change.
    of: Option<u64>,
    /// An undo, redo or revert: per edit, per hunk, the roots it puts back (U23).
    restores: Option<Vec<Option<Vec<Ids>>>>,
}

/// Ids as the store keeps roots.
fn root_runs(ids: &Ids) -> RootRuns {
    ids.runs()
        .iter()
        .map(|r| (r.bytes, r.units, r.start.client, r.start.clock))
        .collect()
}

/// One root of a change's revert record: its canonical hunks with the deleted text and,
/// when known, per hunk the roots of what it deleted and of what it inserted (record v2).
pub(super) fn record_root(
    root: &str,
    before: &str,
    empty_after: bool,
    hunks: &[Hunk],
    roots: Option<&[(Ids, Ids)]>,
) -> RecordRoot {
    RecordRoot {
        root: root.to_owned(),
        empty_after,
        hunks: hunks
            .iter()
            .enumerate()
            .map(|(k, h)| RecordHunk {
                start: h.start as u64,
                end: h.end as u64,
                insert: h.insert.clone(),
                deleted: before[h.start..h.end].to_owned(),
                redacted: false,
                roots: roots.map(|r| HunkRoots {
                    deleted: root_runs(&r[k].0),
                    inserted: root_runs(&r[k].1),
                }),
            })
            .collect(),
    }
}

struct Applied {
    update: Vec<u8>,
    /// The engine clocks of `update`'s inserted text (blame's index).
    span: Option<(u32, u32)>,
    roots: Vec<(String, Vec<Hunk>)>,
    regions: Vec<Region>,
    record: Vec<RecordRoot>,
    /// The provenance rows of the elements it put back (U23).
    prov_rows: Vec<ProvRow>,
    /// D1 widened a root's hunks: `roots` (authored) differ from `record` (widened), so the
    /// store keeps `roots` beside the record (v0.9.7).
    widened: bool,
    kind: ChangeKind,
    of: Option<u64>,
    changed: Vec<LineRange>,
    added: usize,
    removed: usize,
    last_end: Option<(String, usize)>,
}
