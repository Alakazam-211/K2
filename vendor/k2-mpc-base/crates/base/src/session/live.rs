//! One loaded doc and the people and agents in it.

use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};

mod history;

pub use history::{DiffOutput, LogItem, LogQuery};

use super::carry::RevertIndex;
use super::config::Config;
use super::disk::{self, DiskCheck, DiskState, FileStat, FlushOutcome, Probe, ReadFile};
use super::error::{EditWithError, Result, SessionError};
use super::host::{Action, AuditEvent, Host, LeaveReason, Need, Rights};
use super::pack::{Declined, Pack, PackError};
use super::participant::{
    ConnId, CursorAt, Participant, ParticipantKind, RosterEntry, Stay, EXTERNAL_PID, IMPORT_PID,
};
use super::position::{self, At, Bias, Cursor, LineRange};
use super::presence::{self, Presence};
use super::ratelimit::Bucket;
use super::summary;
use super::undo::{self, Batch, ConflictRegion, Mode, Plan, Region};
use crate::doc::{ClientId, Doc, DocError, Edit, Neutral, MAX_CLIENT_ID};
use crate::engine::{ENGINE_FORMAT, ENGINE_NAME};
use crate::hash::{hex, sha256};
use crate::schema::{Kind, Schema};
use crate::store::{
    Approval, ApprovalState, ChangeInfo, ChangeKind, DocId, DocKey, DocMeta, DocStore, LogEntry,
    NewChange, RecordHunk, RecordRoot, RevertRecord, Seq, SyncState, Version, VersionReason,
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
    /// This connection's client ID in this doc (each connection has its own; a new
    /// connection gets a fresh one). Every change made through it is attributed to it, and
    /// the store maps it to the pid.
    pub client: ClientId,
    pub version: Seq,
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
    /// Viewing range as a byte span of its root.
    view: Option<(String, usize, usize)>,
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
    /// Bytes held by both stacks (bounded by [`Config::undo_max_bytes`]).
    bytes: usize,
    /// A purge cleared the stacks; said once on the next undo or redo.
    cleared: bool,
    bucket: Option<Bucket>,
    last_read: Option<Seq>,
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
    read_bases: VecDeque<ReadBase>,
    read_base_bytes: usize,
    /// Recent change authors, for "contributors since last read".
    authors: VecDeque<(Seq, String)>,
    subs: Vec<(ConnId, SyncSender<ChangeEvent>)>,
    disk: Option<Disk>,
}

const AUTHORS_KEPT: usize = 1024;
/// Hashes of the kit's own recent writes kept for echo suppression.
const OWN_WRITES_KEPT: usize = 8;

/// Disk sync for a session opened with a file.
struct Disk {
    /// As the host gave it (symlinks are resolved on every read and write).
    path: PathBuf,
    /// Canonical; the file must resolve inside it.
    root: PathBuf,
    /// The doc as the file holds it: its text is the file's text. Outside edits are made
    /// on it (a diff from the file's previous text to its new text) and the resulting
    /// update is merged into the live doc, so edits not yet written survive.
    replica: Doc,
    replica_bytes: u64,
    synced_hash: Vec<u8>,
    synced_seq: Seq,
    /// Recent own writes: a file holding one of them is an echo, never an outside edit.
    own: VecDeque<Vec<u8>>,
    /// The file as last seen (`None`: read it on the next check).
    stat: Option<FileStat>,
    missing: bool,
    /// The first change not yet written.
    dirty_since: Option<u64>,
    last_change_ms: u64,
    last_check_ms: u64,
    external_client: Option<ClientId>,
}

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
}

fn random_client() -> ClientId {
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
fn fresh_client(store: &DocStore, id: DocId, taken: impl Fn(ClientId) -> bool) -> Result<ClientId> {
    loop {
        let c = random_client();
        if c > 1 && !taken(c) && store.client_owner(id, c)?.is_none() {
            return Ok(c);
        }
    }
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
fn ordered_roots(schema: &Schema, roots: Vec<(String, Neutral)>) -> Result<Vec<(String, Neutral)>> {
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
        Ok(Self {
            host,
            config,
            pack,
            schema,
            doc_id,
            last_used_ms: AtomicU64::new(now),
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
                hist: HashMap::new(),
                loaded_seq: loaded_version,
                last_version_seq,
                last_change_ms: now,
                next_history_ms: now,
                reverts: None,
                read_bases: VecDeque::new(),
                read_base_bytes: 0,
                authors,
                subs: Vec::new(),
                disk: None,
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
        let doc = Doc::from_store(schema, &loaded)?;
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
        let sync = SyncState {
            hash: sha256(bytes).to_vec(),
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
                    .map(|(root, Neutral::Text(t))| RecordRoot {
                        root: root.clone(),
                        empty_after: false,
                        hunks: vec![RecordHunk {
                            start: 0,
                            end: 0,
                            insert: t.clone(),
                            deleted: String::new(),
                            redacted: false,
                        }],
                    })
                    .collect(),
            },
        };
        let (doc_id, seq, _) = store.create_imported_synced(
            &meta,
            client,
            IMPORT_PID,
            entry.as_ref(),
            &version,
            Some(&sync),
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

    fn member(st: &State, conn: ConnId) -> Result<usize> {
        st.members
            .iter()
            .position(|m| m.conn == conn)
            .ok_or(SessionError::NotJoined { conn })
    }

    fn remove_member(&self, st: &mut State, i: usize, reason: LeaveReason) {
        let m = st.members.remove(i);
        st.subs.retain(|(c, _)| *c != m.conn);
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
        let i = Self::member(st, conn)?;
        let rights = self.host.rights(&st.members[i].p, &st.key);
        let pid = st.members[i].p.pid.clone();
        for m in st.members.iter_mut().filter(|m| m.p.pid == pid) {
            m.rights = rights;
        }
        if let Some(n) = presence::refusal(rights, need) {
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
            m.rights = rights;
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
        let client = {
            let store = self.store(st)?;
            let c = fresh_client(store, self.doc_id, |c| {
                st.members.iter().any(|m| m.client == c)
            })?;
            store.set_client(self.doc_id, c, &p.pid)?;
            c
        };
        st.joins += 1;
        let pres = Presence::new(p, shared, rights, stay, st.joins, now);
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
        st.members.push(Member {
            pres,
            client,
            cursor: None,
            view: None,
        });
        Ok(Joined {
            conn,
            client,
            version: st.version,
        })
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
        st.members[i].set_visible(visible, now);
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
        st.members[i].view = Some((range.root.clone(), s, e));
        Ok(LineRange { last, ..range })
    }

    /// Who is here: one entry per participant (pid), editors first, then viewers, then away;
    /// in order of first join within each. Needs read.
    pub fn roster(&self, conn: ConnId) -> Result<Vec<RosterEntry>> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Roster, now)?;
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
        }
        Ok(presence::sort_roster(rows))
    }

    /// Close connections without a hold that have been idle past [`Config::idle_leave_ms`],
    /// and drop expired read bases. Driven by the host's clock. Returns the connections
    /// that left, with their pids.
    pub fn expire_idle(&self) -> Result<Vec<(ConnId, String)>> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        self.prune_read_bases(&mut st, now);
        let mut gone = Vec::new();
        let mut i = 0;
        while i < st.members.len() {
            let m = &st.members[i];
            if m.is_idle(now, &self.config) {
                gone.push((m.conn, m.p.pid.clone()));
                self.remove_member(&mut st, i, LeaveReason::Idle);
            } else {
                i += 1;
            }
        }
        // forget full rate buckets of absent participants with no history
        let c = &self.config;
        let present: Vec<String> = st.members.iter().map(|m| m.p.pid.clone()).collect();
        st.hist.retain(|pid, h| {
            present.contains(pid)
                || !h.undo.is_empty()
                || !h.redo.is_empty()
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
        self.admit(&mut st, conn, Need::Read, Action::Subscribe, now)?;
        let (tx, rx) = sync_channel(self.config.subscriber_queue.max(1));
        st.subs.push((conn, tx));
        Ok(rx)
    }

    // ----- reading -----

    /// The doc as the file would hold it now. Needs read. Records this version as a valid
    /// `write --base` for a while ([`Config::read_bases_max`], `read_base_ttl_ms`,
    /// `read_bases_max_bytes`).
    ///
    /// "Contributors since last read" is per participant (pid), across their connections.
    pub fn read(&self, conn: ConnId) -> Result<ReadResult> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::Read, now)?;
        self.read_locked(&mut st, i, now)
    }

    /// [`Session::read`], then `f` on `root`'s text under the same lock (the tools' read
    /// verbs). `f` returns its result and the lines it showed (1-based, inclusive), which
    /// become the connection's viewing range (P4). Like `edit_with`'s `f`, it must be
    /// bounded and must not call back into the kit; a panic is passed on after the lock is
    /// released.
    pub(crate) fn read_with<R>(
        &self,
        conn: ConnId,
        root: &str,
        f: impl FnOnce(&ReadResult, &str) -> (R, Option<(usize, usize)>),
    ) -> Result<(ReadResult, R)> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::Read, now)?;
        self.text_of(&st, root)?;
        let read = self.read_locked(&mut st, i, now)?;
        let (r, view) = {
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
            }
        }
        Ok((read, r))
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
        for (_, v) in store.versions(self.doc_id)? {
            if v.seq == base {
                return Ok(v
                    .content
                    .into_iter()
                    .find(|(r, _)| r == root)
                    .map(|(_, n)| match n {
                        Neutral::Text(t) => t,
                    }));
            }
        }
        Ok(None)
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
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit_edit(&mut st, conn, Action::Edit, now)?;
        if let Some(expected) = if_version {
            if expected != st.version {
                return Err(SessionError::Stale {
                    expected,
                    actual: st.version,
                });
            }
        }
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
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit_edit(&mut st, conn, Action::Edit, now)?;
        self.check_backlog(&st, i)?;
        let hunks = {
            let live = self.text_of(&st, root)?.1;
            let computed = match catch_unwind(AssertUnwindSafe(|| f(live))) {
                Ok(r) => r.map_err(EditWithError::Compute)?,
                Err(panic) => {
                    drop(st);
                    resume_unwind(panic);
                }
            };
            let new_text = self.pack.normalize_input(root, &computed);
            lines::align_insertions(live, diff::hunks(live, &new_text))
        };
        if hunks.is_empty() {
            return Ok(None);
        }
        let edits = vec![Edit::Text {
            root: root.to_owned(),
            hunks,
        }];
        Ok(Some(self.commit_edit(
            &mut st,
            i,
            edits,
            ChangeKind::Edit,
            None,
            now,
            Vec::new(),
        )?))
    }

    /// Make `root` equal `new_text` through `conn`, through a minimal diff against the live
    /// text. This is last-writer-wins for that root (`write --base live`): use
    /// [`Session::write_base`] when the text was derived from an earlier read.
    pub fn replace_text(&self, conn: ConnId, root: &str, new_text: &str) -> Result<EditResult> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit_edit(&mut st, conn, Action::Edit, now)?;
        self.check_backlog(&st, i)?;
        let new_text = self.pack.normalize_input(root, new_text);
        let live = self.text_of(&st, root)?.1;
        let hunks = lines::align_insertions(live, diff::hunks(live, &new_text));
        let edits = vec![Edit::Text {
            root: root.to_owned(),
            hunks,
        }];
        self.commit_edit(&mut st, i, edits, ChangeKind::Edit, None, now, Vec::new())
    }

    /// `write --base V`: apply only what the caller changed between version `base` and
    /// `new_text`, three-way merged onto the live text. Overlap with a change made since
    /// `base` gives [`WriteOutcome::Conflicts`] and applies nothing, except that two
    /// appends at the end of the text both land, the live one first (merge rule O5). Valid
    /// bases: the live version, recently read versions, and stored versions.
    pub fn write_base(
        &self,
        conn: ConnId,
        root: &str,
        base: Seq,
        new_text: &str,
    ) -> Result<WriteOutcome> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit_edit(&mut st, conn, Action::WriteBase, now)?;
        self.check_backlog(&st, i)?;
        let live = self.text_of(&st, root)?.1.to_owned();
        let Some(base_text) = self.base_text(&mut st, root, base, now)? else {
            return Ok(WriteOutcome::BaseExpired);
        };
        let ours = self.pack.normalize_input(root, new_text);
        match merge::merge3(&base_text, &ours, &live) {
            Merge::Conflicts(c) => Ok(WriteOutcome::Conflicts(c)),
            Merge::Clean(c) => {
                let edits = vec![Edit::Text {
                    root: root.to_owned(),
                    hunks: c.hunks,
                }];
                Ok(WriteOutcome::Applied(self.commit_edit(
                    &mut st,
                    i,
                    edits,
                    ChangeKind::Edit,
                    None,
                    now,
                    Vec::new(),
                )?))
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
        let (steps, blocked) = match undo::plan(&st.texts, batches, mode) {
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
                h.bytes = h.bytes.saturating_sub(b.bytes());
                taken.push((k, b));
            }
        }
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

    /// Clear every participant's undo and redo stacks (a purge, spec U19). Their next undo
    /// or redo says [`UndoOutcome::ClearedByPurge`]. Revert records are not touched here.
    pub fn clear_undo_history(&self) -> Result<()> {
        let mut st = self.lock()?;
        for h in st.hist.values_mut() {
            if !h.undo.is_empty() || !h.redo.is_empty() {
                h.cleared = true;
            }
            h.undo.clear();
            h.redo.clear();
            h.bytes = 0;
        }
        Ok(())
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
        let step = Step { edits, kind, of };
        let (res, mut done) = self.commit(st, i, vec![step], now, versions)?;
        if let Some((seq, regions)) = done.pop().flatten() {
            let pid = st.members[i].p.pid.clone();
            let batch = Batch {
                seq,
                on_behalf_of: st.members[i].p.on_behalf_of.clone(),
                regions,
            };
            self.push_batch(st, &pid, batch, Origin::Edit);
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
        let n_steps = steps.len();
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
            match self.apply_local(st, i, step.edits) {
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
            });
        }
        let appended = self
            .store(st)
            .and_then(|s| Ok(s.commit_changes(self.doc_id, &entries, &changes, &versions)?));
        let seqs = match appended {
            Ok(s) => s,
            Err(e) => {
                self.recover(st, &e.to_string());
                return Err(e);
            }
        };
        if let Some(ix) = st.reverts.as_mut() {
            ix.bind(&seqs);
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
        let m = &mut st.members[i];
        m.last_edit_ms = Some(now);
        if let Some((root, at)) = last_end {
            m.cursor = Some(Cursor {
                root,
                anchor: at,
                head: at,
            });
        }
        self.maybe_compact(st);
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
            *bytes -= redo.drain(..).map(|b| b.bytes()).sum::<usize>();
        }
        *bytes += batch.bytes();
        let dst = if origin == Origin::Undo {
            &mut *redo
        } else {
            &mut *undo
        };
        dst.push_back(batch);
        while dst.len() > c.undo_depth {
            *bytes -= dst.pop_front().map_or(0, |b| b.bytes());
        }
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
    }

    /// Apply one change to the in-memory doc and carry every stored position through it.
    /// Hunks are validated and made canonical (touching hunks coalesced) first. Nothing is
    /// logged here. `Ok(None)` when it changes nothing; an error changes nothing.
    fn apply_local(&self, st: &mut State, i: usize, edits: Vec<Edit>) -> Result<Option<Applied>> {
        let client = st.members[i].client;
        let mut canon = Vec::with_capacity(edits.len());
        for e in edits {
            let Edit::Text { root, hunks } = e;
            let (_, text) = self.text_of(st, &root)?;
            diff::validate(text, &hunks).map_err(DocError::from)?;
            canon.push(Edit::Text {
                root,
                hunks: diff::coalesce(hunks),
            });
        }
        let Some(update) = st.doc.edit_as(client, &canon)? else {
            return Ok(None);
        };
        let mut a = Applied {
            update,
            roots: Vec::new(),
            regions: Vec::new(),
            record: Vec::new(),
            kind: ChangeKind::Edit,
            of: None,
            changed: Vec::new(),
            added: 0,
            removed: 0,
            last_end: None,
        };
        for e in canon {
            let Edit::Text { root, hunks } = e;
            if hunks.is_empty() {
                continue;
            }
            let (ti, _) = self.text_of(st, &root)?;
            let before = std::mem::take(&mut st.texts[ti].1);
            let after = diff::apply(&before, &hunks);
            debug_assert_eq!(Ok(after.as_str()), st.doc.text(&root).as_deref());
            a.regions.extend(Region::of_change(&root, &before, &hunks));
            a.record
                .push(record_root(&root, &before, after.is_empty(), &hunks));
            Self::carry(st, &root, &hunks);
            a.changed
                .extend(summary::changed_lines(&root, &after, &hunks));
            let (ad, rm) = summary::line_counts(&before, &after, &hunks);
            a.added += ad;
            a.removed += rm;
            let delta: isize = hunks
                .iter()
                .map(|h| h.insert.len() as isize - (h.end - h.start) as isize)
                .sum();
            let last = hunks.last().expect("non-empty");
            a.last_end = Some((root.clone(), (last.end as isize + delta) as usize));
            st.texts[ti].1 = after;
            a.roots.push((root, hunks));
        }
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

    /// Carry every stored position (carets, views, undo regions, revert records) through a
    /// change to `root`.
    fn carry(st: &mut State, root: &str, hunks: &[Hunk]) {
        for m in &mut st.members {
            if let Some(c) = m.cursor.as_mut().filter(|c| c.root == root) {
                c.anchor = position::transform(c.anchor, hunks, Bias::After);
                c.head = position::transform(c.head, hunks, Bias::After);
            }
            if let Some((_, s, e)) = m.view.as_mut().filter(|v| v.0 == root) {
                *s = position::transform(*s, hunks, Bias::Before);
                *e = position::transform(*e, hunks, Bias::After).max(*s);
            }
        }
        for h in st.hist.values_mut() {
            for b in h.undo.iter_mut().chain(h.redo.iter_mut()) {
                b.transform(root, hunks);
            }
        }
        if let Some(ix) = st.reverts.as_mut() {
            ix.carry(root, hunks);
        }
    }

    /// Send an event to every subscriber; one that is full or gone is cut off.
    fn broadcast(st: &mut State, ev: &ChangeEvent) {
        st.subs.retain(|(_, tx)| match tx.try_send(ev.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        });
    }

    /// After a failed store write: rebuild from the store so memory matches what is durable.
    /// Carets, views and undo history are reset (they were moved by a change that didn't
    /// land). If the reload fails too, the session closes.
    fn recover(&self, st: &mut State, error: &str) {
        let reloaded = (|| -> Result<_> {
            let store = self.store(st)?;
            let loaded = store.load(self.doc_id)?;
            let doc = Doc::from_store(self.schema, &loaded)?;
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
                }
                for h in st.hist.values_mut() {
                    h.undo.clear();
                    h.redo.clear();
                    h.bytes = 0;
                }
                st.loaded_seq = version;
                st.reverts = None;
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

    /// Tests only: apply a raw update to the live doc, as a bug or a damaged log would.
    #[cfg(test)]
    pub(crate) fn apply_raw_for_tests(&self, update: &[u8]) -> Result<()> {
        let mut st = self.lock()?;
        Ok(st.doc.apply_trusted(update)?)
    }

    // ----- for the manager -----

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

    /// Close this handle: optionally compact a long log first, then drop the store handle
    /// and subscribers. Later calls fail with `reason`.
    pub(crate) fn close(&self, reason: SessionError, compact_long_log: bool) {
        let Ok(mut st) = self.lock_any() else {
            return; // poisoned: nothing safe to do; it is dropped with the pool entry
        };
        if st.closed.is_some() {
            return;
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
        st.closed = Some(reason);
        st.store = None;
        st.subs.clear();
        st.members.clear();
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
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        self.disk_check(&mut st, now, true)
    }

    /// The stat backup: like [`Session::file_changed`], but reads the file only when its
    /// size, mtime or inode changed since the kit last saw it.
    pub fn check_disk(&self) -> Result<DiskCheck> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        self.disk_check(&mut st, now, false)
    }

    /// The host paired a rename of this doc's file: from now on it lives at `new_path`
    /// (inside the same root; re-checked like [`super::SessionManager::open_file`]). Then
    /// checks it like [`Session::file_changed`]. Rename the doc's key with
    /// [`super::SessionManager::rekey`].
    pub fn file_moved(&self, new_path: &Path) -> Result<DiskCheck> {
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
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Flush, now)?;
        let written = self.write_back(&mut st, now)?;
        Ok(FlushOutcome {
            written,
            hash: hex(&sha256(&self.render(&st)?)),
            disk: Self::disk_state(&st),
        })
    }

    /// The file this session syncs with, as the host gave it.
    pub fn file_path(&self) -> Result<Option<PathBuf>> {
        Ok(self.lock()?.disk.as_ref().map(|d| d.path.clone()))
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
                Doc::load(self.schema, &s.state)?,
                s.state.len() as u64,
                s.hash,
                s.seq,
            ),
            // a doc from before disk sync: take its content as what the file held
            None => {
                let state = st.doc.encode_state()?;
                (
                    Doc::load(self.schema, &state)?,
                    state.len() as u64,
                    sha256(&self.render(st)?).to_vec(),
                    st.version,
                )
            }
        };
        let mut d = Disk {
            path: path.to_owned(),
            root: root.to_owned(),
            replica,
            replica_bytes,
            synced_hash: hash,
            synced_seq: seq,
            own: VecDeque::new(),
            stat: None,
            missing: false,
            dirty_since: None,
            last_change_ms: now,
            last_check_ms: now,
            external_client: None,
        };
        let on_disk = match disk::probe(path, root) {
            Ok(Probe::File { real, .. }) => {
                disk::clean_temps(&real);
                match disk::read(&real, self.config.disk_read_max_bytes) {
                    Ok(ReadFile::Bytes(b, _)) => Some(sha256(&b).to_vec()),
                    _ => None,
                }
            }
            _ => None,
        };
        // a write-back a crash interrupted: did it land?
        if let Some(p) = pending {
            if on_disk.as_deref() == Some(p.hash.as_slice()) {
                let mut meta = store.meta(self.doc_id)?;
                meta.synced_hash = Some(p.hash.clone());
                meta.synced_seq = p.seq;
                store.promote_pending_sync(self.doc_id, &meta)?;
                d.replica = Doc::load(self.schema, &p.state)?;
                d.replica_bytes = p.state.len() as u64;
                d.synced_hash = p.hash;
                d.synced_seq = p.seq;
            } else {
                store.clear_pending_sync(self.doc_id)?;
            }
        }
        st.disk = Some(d);
        let out = self.disk_check(st, now, true)?;
        // changes the file never got (a crash before write-back): write them soon
        let live = sha256(&self.render(st)?).to_vec();
        if st.disk.as_ref().is_some_and(|d| d.synced_hash != live) {
            Self::mark_dirty(st, now);
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
            Ok(Probe::File { real, stat }) => (real, stat),
            Ok(_) => return Ok(self.set_missing(st)),
            Err(e) => return Err(self.disk_error(st, &path.display().to_string(), e)),
        };
        if !force && !d.missing && d.stat == Some(stat) {
            return Ok(DiskCheck::Unchanged);
        }
        let (bytes, stat) = match disk::read(&real, self.config.disk_read_max_bytes) {
            Ok(ReadFile::Bytes(b, s)) => (b, s),
            Ok(ReadFile::TooLarge(n)) => {
                d.stat = Some(stat);
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
        d.stat = Some(stat);
        let h = sha256(&bytes).to_vec();
        if h == d.synced_hash {
            if was_missing || st.declined.is_some() {
                self.restored(st)?;
            }
            return Ok(DiskCheck::Unchanged);
        }
        if d.own.contains(&h) {
            // an older write of ours is back (an editor saved a stale buffer, or a late
            // echo): not an edit; the next write-back puts the doc back
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
        Ok(c)
    }

    /// Merge the file's new content (`bytes`, hash `h`) into the live doc.
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
        let d = st.disk.as_mut().expect("has disk");
        let mut edits = Vec::new();
        for (root, Neutral::Text(t)) in &roots {
            let old = d.replica.text(root)?;
            let hunks = lines::align_insertions(&old, diff::hunks(&old, t));
            if !hunks.is_empty() {
                edits.push(Edit::Text {
                    root: root.clone(),
                    hunks,
                });
            }
        }
        if edits.is_empty() {
            // the same text in other bytes (line endings, BOM): adopt the file's format
            meta.synced_seq = d.synced_seq;
            let sync = SyncState {
                hash: h.clone(),
                seq: d.synced_seq,
                state: d.replica.encode_state()?,
            };
            store.commit_disk(self.doc_id, st.version + 1, &[], &[], &[], &meta, &sync)?;
            d.synced_hash = h;
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
        let before = st.texts.clone();
        let d = st.disk.as_mut().expect("has disk");
        // the outside edit is made on the doc as the file held it, then merged into the
        // live doc: changes made since the last write-back are kept
        let made = d.replica.edit_as(client, &edits).and_then(|u| {
            let u = u.expect("non-empty edits change the replica");
            let replica_state = d.replica.encode_state()?;
            st.doc.apply_trusted(&u)?;
            Ok((u, replica_state))
        });
        let (update, replica_state) = match made {
            Ok(u) => u,
            Err(e) => {
                self.recover(st, &e.to_string());
                return Err(e.into());
            }
        };
        let after = texts_of(&st.doc, &self.schema)?;
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
        // the change as applied to the live doc (canonical hunks per root)
        let root_hunks: Vec<(&'static str, Vec<Hunk>)> = before
            .iter()
            .zip(&after)
            .filter(|((_, old), (_, new))| old != new)
            .map(|((root, old), (_, new))| (*root, diff::hunks(old, new)))
            .collect();
        let mut info = ChangeInfo::new(ChangeKind::External, EXTERNAL_PID, "disk", now);
        let mut record = Vec::new();
        for ((root, hunks), ((_, old), (_, new))) in root_hunks.iter().zip(
            before
                .iter()
                .zip(&after)
                .filter(|((_, old), (_, new))| old != new),
        ) {
            let (a, r) = summary::line_counts(old, new, hunks);
            info.lines_added += a as u64;
            info.lines_removed += r as u64;
            record.push(record_root(root, old, new.is_empty(), hunks));
        }
        let change = NewChange {
            info,
            record: RevertRecord {
                author: EXTERNAL_PID.to_owned(),
                on_behalf_of: None,
                at_ms: now,
                roots: record,
            },
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
            Self::carry(st, root, &hunks);
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
                    hunks,
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
        self.settle_dirty(st, now)?;
        self.maybe_compact(st);
        Ok(DiskCheck::Ingested { version: seq })
    }

    /// After the sync point moved: dirty when the doc differs from the file, clean if not.
    fn settle_dirty(&self, st: &mut State, now: u64) -> Result<()> {
        let live = sha256(&self.render(st)?).to_vec();
        let d = st.disk.as_mut().expect("has disk");
        if live == d.synced_hash {
            d.dirty_since = None;
        } else {
            Self::mark_dirty(st, now);
        }
        Ok(())
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
        let h = sha256(&bytes).to_vec();
        let d = st.disk.as_mut().expect("has disk");
        if h == d.synced_hash && !stale {
            d.dirty_since = None;
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
        let replica = Doc::load(self.schema, &state)?;
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
        let d = st.disk.as_mut().expect("has disk");
        d.replica = replica;
        d.replica_bytes = pending.state.len() as u64;
        d.synced_hash = h.clone();
        d.synced_seq = st.version;
        d.own.push_back(h);
        while d.own.len() > OWN_WRITES_KEPT {
            d.own.pop_front();
        }
        d.stat = Some(stat);
        d.dirty_since = None;
        Ok(true)
    }

    /// After a store failure: the disk replica and format back to what the store holds.
    fn reload_disk(&self, st: &mut State) {
        let Some(store) = st.store.clone() else {
            return;
        };
        if let Ok(meta) = store.meta(self.doc_id) {
            st.codec = meta.codec;
            st.declined = meta.declined;
        }
        if let (Some(d), Ok((Some(s), _))) = (st.disk.as_mut(), store.sync_states(self.doc_id)) {
            if let Ok(r) = Doc::load(self.schema, &s.state) {
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
}

/// One root of a change's revert record: its canonical hunks with the deleted text.
fn record_root(root: &str, before: &str, empty_after: bool, hunks: &[Hunk]) -> RecordRoot {
    RecordRoot {
        root: root.to_owned(),
        empty_after,
        hunks: hunks
            .iter()
            .map(|h| RecordHunk {
                start: h.start as u64,
                end: h.end as u64,
                insert: h.insert.clone(),
                deleted: before[h.start..h.end].to_owned(),
                redacted: false,
            })
            .collect(),
    }
}

struct Applied {
    update: Vec<u8>,
    roots: Vec<(String, Vec<Hunk>)>,
    regions: Vec<Region>,
    record: Vec<RecordRoot>,
    kind: ChangeKind,
    of: Option<u64>,
    changed: Vec<LineRange>,
    added: usize,
    removed: usize,
    last_end: Option<(String, usize)>,
}
