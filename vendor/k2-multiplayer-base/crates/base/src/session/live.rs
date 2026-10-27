//! One loaded doc and the people and agents in it.

use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};

use super::config::Config;
use super::error::{EditWithError, Result, SessionError};
use super::host::{Action, AuditEvent, Host, LeaveReason, Need, Rights};
use super::pack::{Pack, PackError};
use super::participant::{
    ConnId, CursorAt, Participant, Role, RosterEntry, Stay, IMPORT_PID, RESERVED_PID_PREFIX,
};
use super::position::{self, At, Bias, Cursor, LineRange};
use super::ratelimit::Bucket;
use super::summary;
use super::undo::{self, Batch, Region};
use crate::doc::{ClientId, Doc, Edit, Neutral, MAX_CLIENT_ID};
use crate::engine::{ENGINE_FORMAT, ENGINE_NAME};
use crate::schema::{Kind, Schema};
use crate::store::{DocId, DocKey, DocMeta, DocStore, LogEntry, Seq, Version, VersionReason};
use crate::text::diff::{self, Hunk};
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

/// Returned by [`Session::undo`] and [`Session::redo`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoOutcome {
    /// `steps` batches were reverted (fewer than asked when the stack ran out).
    Applied { steps: usize, result: EditResult },
    /// Step `step` (1-based) overlaps a later edit by someone else. Nothing was applied.
    /// Positions are approximate locations in the live text.
    Conflicts {
        step: usize,
        conflicts: Vec<Conflict>,
    },
    /// Nothing to undo (or redo).
    Nothing,
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

/// One connection.
struct Member {
    p: Participant,
    conn: ConnId,
    /// Reusable by [`Session::join_shared`].
    shared: bool,
    client: ClientId,
    rights: Rights,
    stay: Stay,
    joined: u64,
    last_active_ms: u64,
    last_edit_ms: Option<u64>,
    last_cursor_ms: Option<u64>,
    hidden_since_ms: Option<u64>,
    cursor: Option<Cursor>,
    /// Viewing range as a byte span of its root.
    view: Option<(String, usize, usize)>,
}

/// Per-participant state that outlives presence (until the session unloads).
#[derive(Default)]
struct History {
    undo: VecDeque<Batch>,
    redo: VecDeque<Batch>,
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
    undo_bytes: usize,
    read_bases: VecDeque<ReadBase>,
    read_base_bytes: usize,
    /// Recent change authors, for "contributors since last read".
    authors: VecDeque<(Seq, String)>,
    subs: Vec<(ConnId, SyncSender<ChangeEvent>)>,
}

const AUTHORS_KEPT: usize = 1024;

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
                undo_bytes: 0,
                read_bases: VecDeque::new(),
                read_base_bytes: 0,
                authors,
                subs: Vec::new(),
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
        // every schema root exactly once, of the right kind; kept in schema order
        if content.roots.len() != schema.roots.len() {
            return Err(PackError(format!(
                "decode gave {} roots; schema {:?} has {}",
                content.roots.len(),
                schema.name,
                schema.roots.len()
            ))
            .into());
        }
        let mut ordered = Vec::with_capacity(schema.roots.len());
        for (name, kind) in schema.roots {
            let found: Vec<&Neutral> = content
                .roots
                .iter()
                .filter(|(r, _)| r == name)
                .map(|(_, v)| v)
                .collect();
            match (found.as_slice(), kind) {
                ([v @ Neutral::Text(_)], Kind::Text) => {
                    ordered.push(((*name).to_owned(), (*v).clone()))
                }
                _ => {
                    return Err(PackError(format!(
                        "decode must give root {name:?} once, as {kind:?}"
                    ))
                    .into())
                }
            }
        }
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
        });
        let version = Version {
            seq: 0,
            at_ms: now,
            reason: VersionReason::Import,
            name: None,
            authors: vec![IMPORT_PID.to_owned()],
            content: ordered,
        };
        let (doc_id, seq, _) =
            store.create_imported(&meta, client, IMPORT_PID, entry.as_ref(), &version)?;
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
            pid: m.p.pid,
            conn: m.conn,
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
        let refused = if !rights.read {
            Some(Need::Read)
        } else if need == Need::Edit && !rights.edit {
            Some(Need::Edit)
        } else {
            None
        };
        if let Some(n) = refused {
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
        if p.pid.is_empty() || p.pid.starts_with(RESERVED_PID_PREFIX) {
            return Err(SessionError::BadParticipant(format!(
                "pid {:?} is empty or reserved ({RESERVED_PID_PREFIX}*)",
                p.pid
            )));
        }
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
            if let Some(m) = st
                .members
                .iter_mut()
                .find(|m| m.shared && m.p.pid == p.pid && m.p.via == p.via)
            {
                m.p = p;
                m.last_active_ms = now;
                if stay == Stay::Held {
                    m.stay = Stay::Held;
                }
                return Ok(Joined {
                    conn: m.conn,
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
        let conn = ConnId::fresh();
        st.joins += 1;
        self.emit(AuditEvent::Joined {
            key: st.key.clone(),
            pid: p.pid.clone(),
            conn,
            client,
        });
        st.members.push(Member {
            p,
            conn,
            shared,
            client,
            rights,
            stay,
            joined: st.joins,
            last_active_ms: now,
            last_edit_ms: None,
            last_cursor_ms: None,
            hidden_since_ms: None,
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
        let m = &mut st.members[i];
        if visible {
            m.hidden_since_ms = None;
        } else if m.hidden_since_ms.is_none() {
            m.hidden_since_ms = Some(now);
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
        st.members[i].view = Some((range.root.clone(), s, e));
        Ok(LineRange { last, ..range })
    }

    /// Who is here: one entry per participant (pid), editors first, then viewers, then away;
    /// in order of first join within each. Needs read.
    pub fn roster(&self, conn: ConnId) -> Result<Vec<RosterEntry>> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Roster, now)?;
        let c = &self.config;
        let recent = |t: Option<u64>| t.is_some_and(|t| now.saturating_sub(t) < c.editor_window_ms);
        let text = |root: &str| {
            st.texts
                .iter()
                .find(|(r, _)| *r == root)
                .map(|(_, t)| t.as_str())
        };
        // members are in join order, so pids come out in order of first join
        let mut pids: Vec<&str> = Vec::new();
        for m in &st.members {
            if !pids.contains(&m.p.pid.as_str()) {
                pids.push(&m.p.pid);
            }
        }
        let mut rows: Vec<(bool, Role, u64, RosterEntry)> = pids
            .into_iter()
            .map(|pid| {
                // most active first: latest edit or caret move, then latest call, then newest
                let mut conns: Vec<&Member> =
                    st.members.iter().filter(|m| m.p.pid == pid).collect();
                conns.sort_by_key(|m| {
                    std::cmp::Reverse((
                        m.last_edit_ms.max(m.last_cursor_ms),
                        m.last_active_ms,
                        m.joined,
                    ))
                });
                let top = conns[0];
                let role = if top.rights.edit
                    && (recent(top.last_edit_ms) || recent(top.last_cursor_ms))
                {
                    Role::Editor
                } else {
                    Role::Viewer
                };
                let away = conns.iter().all(|m| {
                    m.hidden_since_ms
                        .is_some_and(|t| now.saturating_sub(t) >= c.away_after_ms)
                });
                let cursor = conns.iter().find_map(|m| {
                    let cur = m.cursor.as_ref()?;
                    let t = text(&cur.root)?;
                    Some(CursorAt {
                        cursor: cur.clone(),
                        anchor: position::line_col(t, cur.anchor)?,
                        head: position::line_col(t, cur.head)?,
                    })
                });
                let view = conns.iter().find_map(|m| {
                    let (root, s, e) = m.view.as_ref()?;
                    let t = text(root)?;
                    Some(LineRange {
                        root: root.clone(),
                        first: position::line_of(t, *s),
                        last: position::line_of(t, *e),
                    })
                });
                let first_joined = conns.iter().map(|m| m.joined).min().unwrap_or(0);
                let stay = if conns.iter().any(|m| m.stay == Stay::Held) {
                    Stay::Held
                } else {
                    Stay::UntilIdle
                };
                (
                    away,
                    role,
                    first_joined,
                    RosterEntry {
                        participant: top.p.clone(),
                        role,
                        away,
                        cursor,
                        view,
                        last_edit_ms: conns.iter().filter_map(|m| m.last_edit_ms).max(),
                        stay,
                        connections: conns.len(),
                    },
                )
            })
            .collect();
        rows.sort_by_key(|(away, role, joined, _)| (*away, *role, *joined));
        Ok(rows.into_iter().map(|r| r.3).collect())
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
            if m.stay == Stay::UntilIdle
                && now.saturating_sub(m.last_active_ms) >= self.config.idle_leave_ms
            {
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
        self.record_read_base(&mut st, now);
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
            participants: {
                let mut p: Vec<&str> = st.members.iter().map(|m| m.p.pid.as_str()).collect();
                p.sort_unstable();
                p.dedup();
                p.len()
            },
            connections: st.members.len(),
            log_entries: st.log_entries,
            estimated_bytes: st.state_bytes * self.config.memory_factor,
            declined: st.declined.clone(),
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
        let edits = self.normalize(edits)?;
        self.commit(&mut st, i, vec![edits], Origin::Edit, now)
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
            diff::hunks(live, &new_text)
        };
        if hunks.is_empty() {
            return Ok(None);
        }
        let edits = vec![Edit::Text {
            root: root.to_owned(),
            hunks,
        }];
        Ok(Some(self.commit(
            &mut st,
            i,
            vec![edits],
            Origin::Edit,
            now,
        )?))
    }

    /// Make `root` equal `new_text` through `conn`, through a minimal diff against the live
    /// text. This is last-writer-wins for that root (`write --base live`): use
    /// [`Session::write_base`] when the text was derived from an earlier read.
    pub fn replace_text(&self, conn: ConnId, root: &str, new_text: &str) -> Result<EditResult> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit_edit(&mut st, conn, Action::Edit, now)?;
        let new_text = self.pack.normalize_input(root, new_text);
        let hunks = diff::hunks(self.text_of(&st, root)?.1, &new_text);
        let edits = vec![Edit::Text {
            root: root.to_owned(),
            hunks,
        }];
        self.commit(&mut st, i, vec![edits], Origin::Edit, now)
    }

    /// `write --base V`: apply only what the caller changed between version `base` and
    /// `new_text`, three-way merged onto the live text. Overlap with a change made since
    /// `base` gives [`WriteOutcome::Conflicts`] and applies nothing. Valid bases: the live
    /// version, recently read versions, and stored versions.
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
                Ok(WriteOutcome::Applied(self.commit(
                    &mut st,
                    i,
                    vec![edits],
                    Origin::Edit,
                    now,
                )?))
            }
        }
    }

    /// Revert the participant's own last `steps` edit batches, newest first, three-way
    /// merged against everything since. All steps apply, or none (a step that overlaps
    /// someone else's later edit gives [`UndoOutcome::Conflicts`]). Never touches anyone
    /// else's changes.
    ///
    /// Undo is **per participant (pid)**, across their connections: an undo through one
    /// connection reverts the pid's last edit, whichever connection made it.
    pub fn undo(&self, conn: ConnId, steps: usize) -> Result<UndoOutcome> {
        self.undo_redo(conn, steps, Origin::Undo)
    }

    /// Re-apply what the participant's undo reverted. A new edit by them clears the redo
    /// stack.
    pub fn redo(&self, conn: ConnId, steps: usize) -> Result<UndoOutcome> {
        self.undo_redo(conn, steps, Origin::Redo)
    }

    fn undo_redo(&self, conn: ConnId, steps: usize, origin: Origin) -> Result<UndoOutcome> {
        let now = self.now();
        let mut st = self.lock()?;
        let action = if origin == Origin::Undo {
            Action::Undo
        } else {
            Action::Redo
        };
        let i = self.admit_edit(&mut st, conn, action, now)?;
        let st = &mut *st;
        let h = st.hist.entry(st.members[i].p.pid.clone()).or_default();
        let stack = if origin == Origin::Undo {
            &h.undo
        } else {
            &h.redo
        };
        let n = steps.min(stack.len());
        if n == 0 {
            return Ok(UndoOutcome::Nothing);
        }
        let planned = match undo::plan(&st.texts, stack, n) {
            Ok(p) => p,
            Err((step, conflicts)) => return Ok(UndoOutcome::Conflicts { step, conflicts }),
        };
        let result = self.commit(st, i, planned, origin, now)?;
        Ok(UndoOutcome::Applied { steps: n, result })
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

    /// Apply `steps` (each one change) as member `i`, log them in one durable append, and do
    /// the bookkeeping: attribution, carets, undo, events, compaction.
    fn commit(
        &self,
        st: &mut State,
        i: usize,
        steps: Vec<Vec<Edit>>,
        origin: Origin,
        now: u64,
    ) -> Result<EditResult> {
        let n_steps = steps.len();
        // for a multi-step summary: the touched roots' text before everything
        let orig: Vec<(&'static str, String)> = if n_steps > 1 {
            st.texts.clone()
        } else {
            Vec::new()
        };
        let mut applied: Vec<Applied> = Vec::new();
        for step in steps {
            match self.apply_local(st, i, step) {
                Ok(Some(a)) => applied.push(a),
                Ok(None) => {}
                Err(e) if applied.is_empty() => return Err(e),
                Err(e) => {
                    self.recover(st, &e.to_string());
                    return Err(e);
                }
            }
        }
        // the batches being undone/redone are done with, applied or not
        let pid = st.members[i].p.pid.clone();
        if origin != Origin::Edit {
            let h = st.hist.entry(pid.clone()).or_default();
            let src = if origin == Origin::Undo {
                &mut h.undo
            } else {
                &mut h.redo
            };
            let mut freed = 0;
            for _ in 0..n_steps {
                if let Some(b) = src.pop_back() {
                    freed += b.bytes();
                }
            }
            st.undo_bytes -= freed;
        }
        if applied.is_empty() {
            return Ok(EditResult {
                version: st.version,
                ..EditResult::default()
            });
        }
        let m = &st.members[i];
        let (client, via) = (m.client, m.p.via.clone());
        let entries: Vec<LogEntry> = applied
            .iter_mut()
            .map(|a| LogEntry {
                author: pid.clone(),
                client,
                at_ms: now,
                via: via.clone(),
                update: std::mem::take(&mut a.update),
            })
            .collect();
        let appended = self
            .store(st)
            .and_then(|s| Ok(s.append(self.doc_id, &entries)?));
        let seqs = match appended {
            Ok(s) => s,
            Err(e) => {
                self.recover(st, &e.to_string());
                return Err(e);
            }
        };
        // bookkeeping
        let mut changed = Vec::new();
        let (mut added, mut removed) = (0, 0);
        let mut last_end = None;
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
                };
                st.subs.retain(|(_, tx)| match tx.try_send(ev.clone()) {
                    Ok(()) => true,
                    Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
                });
            }
            if n_steps == 1 {
                changed = a.changed;
                (added, removed) = (a.added, a.removed);
            }
            last_end = a.last_end.or(last_end);
            let batch = Batch {
                seq: *seq,
                regions: a.regions,
            };
            self.push_batch(st, &pid, batch, origin);
        }
        st.version = *seqs.last().expect("one seq per entry");
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
        Ok(EditResult {
            version: st.version,
            changed,
            lines_added: added,
            lines_removed: removed,
        })
    }

    fn push_batch(&self, st: &mut State, pid: &str, batch: Batch, origin: Origin) {
        let c = &self.config;
        let h = st.hist.entry(pid.to_owned()).or_default();
        let mut freed = 0;
        if origin == Origin::Edit {
            freed += h.redo.drain(..).map(|b| b.bytes()).sum::<usize>();
        }
        let added = batch.bytes();
        let dst = if origin == Origin::Undo {
            &mut h.redo
        } else {
            &mut h.undo
        };
        dst.push_back(batch);
        while dst.len() > c.undo_depth {
            freed += dst.pop_front().map_or(0, |b| b.bytes());
        }
        st.undo_bytes = st.undo_bytes + added - freed;
        // over the byte cap: drop the oldest batches across everyone
        while st.undo_bytes > c.undo_max_bytes {
            let oldest = st
                .hist
                .values_mut()
                .flat_map(|h| [&mut h.undo, &mut h.redo])
                .filter(|s| !s.is_empty())
                .min_by_key(|s| s.front().map(|b| b.seq));
            match oldest.and_then(|s| s.pop_front()) {
                Some(b) => st.undo_bytes -= b.bytes(),
                None => break,
            }
        }
    }

    /// Apply one change to the in-memory doc and carry every stored position through it.
    /// Nothing is logged here. `Ok(None)` when it changes nothing; an error changes nothing.
    fn apply_local(&self, st: &mut State, i: usize, edits: Vec<Edit>) -> Result<Option<Applied>> {
        let client = st.members[i].client;
        let Some(update) = st.doc.edit_as(client, &edits)? else {
            return Ok(None);
        };
        let mut a = Applied {
            update,
            roots: Vec::new(),
            regions: Vec::new(),
            changed: Vec::new(),
            added: 0,
            removed: 0,
            last_end: None,
        };
        for e in edits {
            let Edit::Text { root, hunks } = e;
            if hunks.is_empty() {
                continue;
            }
            let (ti, _) = self.text_of(st, &root)?;
            let before = std::mem::take(&mut st.texts[ti].1);
            let after = diff::apply(&before, &hunks);
            debug_assert_eq!(Ok(after.as_str()), st.doc.text(&root).as_deref());
            a.regions.push(Region::of_change(&root, &before, &hunks));
            for m in &mut st.members {
                if let Some(c) = m.cursor.as_mut().filter(|c| c.root == root) {
                    c.anchor = position::transform(c.anchor, &hunks, Bias::After);
                    c.head = position::transform(c.head, &hunks, Bias::After);
                }
                if let Some((_, s, e)) = m.view.as_mut().filter(|v| v.0 == root) {
                    *s = position::transform(*s, &hunks, Bias::Before);
                    *e = position::transform(*e, &hunks, Bias::After).max(*s);
                }
            }
            for h in st.hist.values_mut() {
                for b in h.undo.iter_mut().chain(h.redo.iter_mut()) {
                    b.transform(&root, &hunks);
                }
            }
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
        Ok(Some(a))
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
                }
                st.undo_bytes = 0;
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
        let state = st.doc.encode_state();
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
        Some((st.members.len(), st.state_bytes * self.config.memory_factor))
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

struct Applied {
    update: Vec<u8>,
    roots: Vec<(String, Vec<Hunk>)>,
    regions: Vec<Region>,
    changed: Vec<LineRange>,
    added: usize,
    removed: usize,
    last_end: Option<(String, usize)>,
}
