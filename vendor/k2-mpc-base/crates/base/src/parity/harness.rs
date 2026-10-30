//! The parity harness (K-PAR-3): each person action, the person's way and the agent's way,
//! on two identical docs, then the observable effects compared.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use super::action::PersonAction;
use super::registry::{verb_form, Api, Registry, Tier};
use crate::doc::Edit;
use crate::session::{
    Approval, ApprovalState, At, AuditEvent, ChangeEvent, ChangeKind, Config, ConnId, CursorAt,
    DiskState, EditResult, Host, LineCol, LineRange, LogItem, LogQuery, Pack, Participant,
    ParticipantKind, Rights, Role, Session, SessionError, SessionManager, Stay, UndoMode,
    UndoOutcome, UndoRequest, VersionInfo, WriteOutcome,
};
use crate::store::{DocKey, DocStore, Seq, VersionReason};
use crate::text::diff::Hunk;
use crate::tools::{self, ToolOutput};

/// The participant whose action is compared: a person (`via: "tab"`) in one world, an agent
/// (`via: "cli"`) in the other, with the same pid, name and colour.
pub const ACTOR: &str = "actor";
/// A participant with full rights who observes both worlds (roster, log, versions).
pub const OBSERVER: &str = "observer";
/// The doc's path (its display name in tool output).
pub const DOC_PATH: &str = "notes.md";

pub(super) const PERSON_VIA: &str = "tab";
pub(super) const AGENT_VIA: &str = "cli";

// ---------------------------------------------------------------------------------------
// the parity table, as data
// ---------------------------------------------------------------------------------------

/// Where a row stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStatus {
    /// Every verb listed exists and behaves as the person path does.
    Live,
    /// Not built yet: the row's verbs (`absent`) must **not** exist; when one lands, the
    /// harness fails until the row is made live.
    Deferred {
        until: &'static str,
        why: &'static str,
    },
    /// The file type doesn't have this action. `absent` verbs must not exist; `verbs`, if
    /// any, show the reason (P24: `cat` gives the file's exact bytes).
    NotApplicable { reason: &'static str },
}

/// One row of a pack's parity list (`docs/PARITY.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParityRow {
    pub action: PersonAction,
    /// The verbs serving it (registry forms, see [`verb_form`]); each needs a case.
    pub verbs: &'static [&'static str],
    /// Verbs of this row that must not be registered (not built yet, or N/A).
    pub absent: &'static [&'static str],
    pub status: RowStatus,
}

impl ParityRow {
    /// A live row.
    pub const fn live(action: PersonAction, verbs: &'static [&'static str]) -> Self {
        ParityRow {
            action,
            verbs,
            absent: &[],
            status: RowStatus::Live,
        }
    }

    /// A live row with some verbs still to come (they must not be registered yet).
    pub const fn live_but(
        action: PersonAction,
        verbs: &'static [&'static str],
        later: &'static [&'static str],
    ) -> Self {
        ParityRow {
            action,
            verbs,
            absent: later,
            status: RowStatus::Live,
        }
    }

    /// A row whose verbs arrive later (`until`: the slice or release).
    pub const fn deferred(
        action: PersonAction,
        later: &'static [&'static str],
        until: &'static str,
        why: &'static str,
    ) -> Self {
        ParityRow {
            action,
            verbs: &[],
            absent: later,
            status: RowStatus::Deferred { until, why },
        }
    }

    /// An action the type doesn't have.
    pub const fn not_applicable(
        action: PersonAction,
        shown_by: &'static [&'static str],
        absent: &'static [&'static str],
        reason: &'static str,
    ) -> Self {
        ParityRow {
            action,
            verbs: shown_by,
            absent,
            status: RowStatus::NotApplicable { reason },
        }
    }
}

// ---------------------------------------------------------------------------------------
// cases
// ---------------------------------------------------------------------------------------

/// What one path returned: its exit code and what it showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub exit: u8,
    pub output: Output,
}

impl Step {
    pub fn ok(output: Output) -> Self {
        Step { exit: 0, output }
    }
    pub fn exit(exit: u8, output: Output) -> Self {
        Step { exit, output }
    }
    /// Exit 0, nothing shown.
    pub fn done() -> Self {
        Self::ok(Output::None)
    }
}

/// What a path showed, normalised so both paths can be compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    None,
    Bytes(Vec<u8>),
    Text(String),
    /// Line numbers (find).
    Lines(Vec<usize>),
    /// Participants, in roster order.
    Pids(Vec<String>),
    /// A caret, `line:col` (anchor, head).
    Caret(Option<(LineCol, LineCol)>),
    /// Change events: version, pid, via, root, hunks, on-behalf-of.
    Events(Vec<EventObs>),
    /// A structured value, rendered canonically (`{:?}`).
    Value(String),
}

impl Output {
    /// [`Output::Value`] of `{:?}`.
    pub fn value(v: impl fmt::Debug) -> Self {
        Output::Value(format!("{v:?}"))
    }
}

/// A change event without the sender's client ID (random per connection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventObs {
    pub version: Seq,
    pub pid: String,
    pub via: String,
    pub root: String,
    pub hunks: Vec<Hunk>,
    pub on_behalf_of: Option<String>,
}

impl From<ChangeEvent> for EventObs {
    fn from(e: ChangeEvent) -> Self {
        EventObs {
            version: e.version,
            pid: e.pid,
            via: e.via,
            root: e.root,
            hunks: e.hunks,
            on_behalf_of: e.on_behalf_of,
        }
    }
}

/// Others' activity before the paths run.
pub type SetupFn = fn(&mut Stage) -> Result<(), String>;
/// The person path: what an editor tab does, through the session API directly.
pub type PersonFn = fn(&mut Person<'_>) -> Result<Step, String>;
/// The agent path: registered verbs only ([`Agent::verb`]).
pub type AgentFn = fn(&mut Agent<'_>) -> Result<Step, String>;

/// One parity check: an action, the verb under test, the doc, others' setup, and both paths.
#[derive(Clone, Copy)]
pub struct Case {
    pub action: PersonAction,
    /// The registry form under test (the agent path must run it).
    pub verb: &'static str,
    /// What this case shows, a few words.
    pub name: &'static str,
    pub initial: &'static str,
    pub setup: SetupFn,
    pub person: PersonFn,
    pub agent: AgentFn,
}

/// The default doc (13 lines).
pub const DOC: &str = "# Plan\n\nIntro line.\n\n## Goals\n\n- ship the core\n- keep files plain\n\n## Notes\n\nTODO: write more.\nEnd.\n";

fn no_setup(_: &mut Stage) -> Result<(), String> {
    Ok(())
}

impl Case {
    /// A case on [`DOC`] with no setup.
    pub const fn new(
        action: PersonAction,
        verb: &'static str,
        name: &'static str,
        person: PersonFn,
        agent: AgentFn,
    ) -> Self {
        Case {
            action,
            verb,
            name,
            initial: DOC,
            setup: no_setup,
            person,
            agent,
        }
    }
    pub const fn initial(self, initial: &'static str) -> Self {
        Case { initial, ..self }
    }
    pub const fn setup(self, setup: SetupFn) -> Self {
        Case { setup, ..self }
    }
}

impl fmt::Debug for Case {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:?} ({})", self.action, self.verb, self.name)
    }
}

// ---------------------------------------------------------------------------------------
// the world one path runs in
// ---------------------------------------------------------------------------------------

pub(super) struct ParityHost {
    pub(super) now: AtomicU64,
    pub(super) rights: Mutex<HashMap<String, Rights>>,
}

impl Host for ParityHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .map(|r| r.get(&who.pid).copied().unwrap_or(Rights::none()))
            .unwrap_or(Rights::none())
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

pub(super) struct TmpDir(pub(super) PathBuf);

impl TmpDir {
    pub(super) fn new() -> Result<Self, String> {
        static N: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let p = std::env::temp_dir().join(format!(
            "k2mpc-parity-{}-{}-{nanos}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(p.join("docs")).map_err(|e| format!("temp dir: {e}"))?;
        Ok(TmpDir(p))
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One world: a store, a manager, the doc opened from its file, an observer, and the other
/// participants. Setup and both paths use it for **others'** actions; the actor acts only
/// through [`Person`] or [`Agent`].
pub struct Stage {
    host: Arc<ParityHost>,
    mgr: SessionManager,
    session: Arc<Session>,
    pack: Arc<dyn Pack>,
    key: DocKey,
    file: PathBuf,
    root_dir: PathBuf,
    observer: ConnId,
    others: HashMap<String, ConnId>,
    marks: HashMap<&'static str, u64>,
    // last: dropped after the store and sessions
    _dir: TmpDir,
}

impl Stage {
    fn new(pack: Arc<dyn Pack>, initial: &str) -> Result<Self, String> {
        let dir = TmpDir::new()?;
        let host = Arc::new(ParityHost {
            now: AtomicU64::new(1_700_000_000_000),
            rights: Mutex::new(HashMap::new()),
        });
        let store = DocStore::open(dir.0.join("docs.redb")).map_err(|e| format!("store: {e}"))?;
        let mgr = SessionManager::new(Arc::new(store), host.clone(), Config::default());
        let root_dir = dir.0.join("docs");
        let file = root_dir.join(DOC_PATH);
        std::fs::write(&file, initial).map_err(|e| format!("write doc: {e}"))?;
        let key = DocKey {
            space: "ws".into(),
            path: DOC_PATH.into(),
        };
        let session = mgr
            .open_file(pack.clone(), &key, &file, &root_dir)
            .map_err(|e| format!("open_file: {e}"))?;
        let mut stage = Stage {
            host,
            mgr,
            session,
            pack,
            key,
            file,
            root_dir,
            observer: ConnId::fresh(),
            others: HashMap::new(),
            marks: HashMap::new(),
            _dir: dir,
        };
        stage.grant(OBSERVER, Rights::full());
        stage.grant(ACTOR, Rights::full());
        stage.observer = stage
            .session
            .join(
                Participant::new(
                    OBSERVER,
                    "Observer",
                    "#888888",
                    ParticipantKind::Person,
                    "tab",
                ),
                Stay::Held,
            )
            .map_err(|e| format!("observer join: {e}"))?
            .conn;
        Ok(stage)
    }

    /// The session (for others' actions).
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }
    pub fn manager(&self) -> &SessionManager {
        &self.mgr
    }
    /// The doc's first text root.
    pub fn root(&self) -> &'static str {
        self.pack.schema().roots[0].0
    }
    pub fn key(&self) -> &DocKey {
        &self.key
    }
    /// The doc's file.
    pub fn file(&self) -> &Path {
        &self.file
    }
    /// The folder the doc's file lives in (the jail root).
    pub fn dir(&self) -> &Path {
        &self.root_dir
    }
    pub fn now(&self) -> u64 {
        self.host.now.load(Ordering::SeqCst)
    }
    /// Move the host clock on.
    pub fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    /// Set `pid`'s rights (the actor starts with [`Rights::full`]).
    pub fn grant(&self, pid: &str, r: Rights) {
        if let Ok(mut m) = self.host.rights.lock() {
            m.insert(pid.to_owned(), r);
        }
    }
    fn rights_of(&self, pid: &str) -> Rights {
        self.host
            .rights
            .lock()
            .map(|m| m.get(pid).copied().unwrap_or(Rights::none()))
            .unwrap_or(Rights::none())
    }
    /// Remember a value (a version, a change) for the paths.
    pub fn set_mark(&mut self, name: &'static str, v: u64) {
        self.marks.insert(name, v);
    }
    pub fn mark(&self, name: &'static str) -> Result<u64, String> {
        self.marks
            .get(name)
            .copied()
            .ok_or_else(|| format!("no mark {name:?} (set it in setup)"))
    }

    /// Join `pid` (not the actor) with full rights, or return its connection. A person joins
    /// through a tab, an agent through the CLI.
    pub fn other(&mut self, pid: &str, kind: ParticipantKind) -> Result<ConnId, String> {
        self.other_as(Participant::new(
            pid,
            pid.to_uppercase(),
            "#123456",
            kind,
            if kind == ParticipantKind::Person {
                PERSON_VIA
            } else {
                AGENT_VIA
            },
        ))
    }

    /// An assistant agent `pid` acting on behalf of `person`.
    pub fn assistant(&mut self, pid: &str, person: &str) -> Result<ConnId, String> {
        self.other_as(
            Participant::new(
                pid,
                pid.to_uppercase(),
                "#654321",
                ParticipantKind::Agent,
                AGENT_VIA,
            )
            .on_behalf_of(person),
        )
    }

    fn other_as(&mut self, p: Participant) -> Result<ConnId, String> {
        if p.pid == ACTOR {
            return Err("the actor acts only through Person or Agent".into());
        }
        if let Some(c) = self.others.get(&p.pid) {
            return Ok(*c);
        }
        if self.rights_of(&p.pid) == Rights::none() {
            self.grant(&p.pid, Rights::full());
        }
        let pid = p.pid.clone();
        let c = self
            .session
            .join(p, Stay::Held)
            .map_err(|e| format!("{pid} join: {e}"))?
            .conn;
        self.others.insert(pid, c);
        Ok(c)
    }

    /// `pid` (joined with [`Stage::other`] or [`Stage::assistant`]) edits the live text
    /// with `f`. Returns the version after.
    pub fn edit_as(&mut self, pid: &str, f: impl FnOnce(&str) -> String) -> Result<Seq, String> {
        let c = match self.others.get(pid) {
            Some(c) => *c,
            None => self.other(pid, ParticipantKind::Person)?,
        };
        let root = self.root();
        let r = self
            .session
            .edit_with(c, root, |t| Ok::<_, ()>(f(t)))
            .map_err(|e| format!("{pid} edit: {e:?}"))?;
        r.map(|r| r.version)
            .ok_or_else(|| format!("{pid}'s edit changed nothing"))
    }

    /// The live text (as the observer reads it).
    pub fn text(&self) -> Result<String, String> {
        let r = self
            .session
            .read(self.observer)
            .map_err(|e| format!("observer read: {e}"))?;
        Ok(r.text_by_root
            .into_iter()
            .find(|(r, _)| r == self.root())
            .map(|(_, t)| t)
            .unwrap_or_default())
    }

    /// The doc's version now.
    pub fn version(&self) -> Result<Seq, String> {
        Ok(self.session.status().map_err(|e| e.to_string())?.version)
    }

    /// Every version (oldest first).
    pub fn versions(&self) -> Result<Vec<VersionInfo>, String> {
        self.session
            .versions(self.observer)
            .map_err(|e| format!("versions: {e}"))
    }

    /// The host renames the doc's file to `to` (a plain name in the same folder) and tells
    /// the kit: `SessionManager::rekey`, then `Session::file_moved`. What a tab's rename and
    /// the `mv` verb both do.
    pub fn host_rename(&mut self, to: &str) -> Result<crate::session::DiskCheck, SessionError> {
        let (new_file, new_key) = rename_target(self, to).map_err(SessionError::BadPath)?;
        std::fs::rename(&self.file, &new_file)
            .map_err(|e| SessionError::Disk(format!("rename: {e}")))?;
        self.mgr.rekey(&self.key, new_key.clone())?;
        let d = self.session.file_moved(&new_file)?;
        self.file = new_file;
        self.key = new_key;
        Ok(d)
    }

    /// Another program writes the file; the host's watcher reports it.
    pub fn write_file(&self, bytes: &[u8]) -> Result<crate::session::DiskCheck, String> {
        std::fs::write(&self.file, bytes).map_err(|e| format!("write file: {e}"))?;
        self.session.file_changed().map_err(|e| e.to_string())
    }

    fn actor(&self, kind: ParticipantKind, via: &str) -> Participant {
        Participant::new(ACTOR, "Actor", "#2a9d8f", kind, via)
    }
}

// ---------------------------------------------------------------------------------------
// the person path
// ---------------------------------------------------------------------------------------

/// The actor as a person in an editor tab: the session API directly, positions in bytes,
/// one held connection.
pub struct Person<'a> {
    stage: &'a mut Stage,
    conn: Option<ConnId>,
    rx: Option<Receiver<ChangeEvent>>,
}

impl Person<'_> {
    /// The world, for others' actions in the middle of the path.
    pub fn stage(&mut self) -> &mut Stage {
        self.stage
    }
    pub fn session(&self) -> Arc<Session> {
        self.stage.session.clone()
    }
    pub fn root(&self) -> &'static str {
        self.stage.root()
    }

    /// The tab's connection (opens the tab on first use: `join`, held).
    pub fn conn(&mut self) -> Result<ConnId, String> {
        if let Some(c) = self.conn {
            return Ok(c);
        }
        let p = self.stage.actor(ParticipantKind::Person, PERSON_VIA);
        let c = self
            .stage
            .session
            .join(p, Stay::Held)
            .map_err(|e| format!("person join: {e}"))?
            .conn;
        self.conn = Some(c);
        Ok(c)
    }

    /// Close the tab.
    pub fn close(&mut self) -> Result<(), String> {
        let c = self.conn()?;
        self.stage.session.leave(c).map_err(|e| e.to_string())?;
        self.conn = None;
        Ok(())
    }

    /// What the tab shows: the live text (a read).
    pub fn text(&mut self) -> Result<String, String> {
        let c = self.conn()?;
        let root = self.root();
        let r = self.stage.session.read(c).map_err(|e| e.to_string())?;
        Ok(r.text_by_root
            .into_iter()
            .find(|(r, _)| r == root)
            .map(|(_, t)| t)
            .unwrap_or_default())
    }

    /// The byte offset of `line:col` in the live text.
    pub fn offset(&mut self, line: usize, col: usize) -> Result<usize, String> {
        let t = self.text()?;
        crate::session::position::offset(&t, LineCol { line, col })
            .ok_or_else(|| format!("{line}:{col} is not in the text"))
    }

    /// Typing: hunks against the live text, sent with the version they were computed on.
    pub fn edit(&mut self, hunks: Vec<Hunk>) -> Result<EditResult, SessionError> {
        let c = self.conn().map_err(SessionError::BadEdit)?;
        let v = self.stage.session.status()?.version;
        let root = self.root().to_owned();
        self.stage
            .session
            .edit(c, vec![Edit::Text { root, hunks }], Some(v))
    }

    /// One hunk: replace bytes `start..end` with `insert`.
    pub fn type_at(
        &mut self,
        start: usize,
        end: usize,
        insert: &str,
    ) -> Result<EditResult, SessionError> {
        self.edit(vec![Hunk {
            start,
            end,
            insert: insert.to_owned(),
        }])
    }

    /// An editor command over the whole text (replace all): one change.
    pub fn edit_with(
        &mut self,
        f: impl FnOnce(&str) -> String,
    ) -> Result<Option<EditResult>, SessionError> {
        let c = self.conn().map_err(SessionError::BadEdit)?;
        let root = self.root();
        self.stage
            .session
            .edit_with(c, root, |t| Ok::<_, ()>(f(t)))
            .map_err(|e| match e {
                crate::session::EditWithError::Session(e) => e,
                crate::session::EditWithError::Compute(()) => {
                    SessionError::BadEdit("compute".into())
                }
            })
    }

    /// Ctrl+Z (the tab's undo is `--partial`) or another undo mode.
    pub fn undo(&mut self, mode: UndoMode) -> Result<UndoOutcome, SessionError> {
        let c = self.conn().map_err(SessionError::BadEdit)?;
        self.stage
            .session
            .undo_with(c, &UndoRequest::steps(1).mode(mode))
    }

    /// Ctrl+Shift+Z.
    pub fn redo(&mut self) -> Result<UndoOutcome, SessionError> {
        let c = self.conn().map_err(SessionError::BadEdit)?;
        self.stage.session.redo_with(c, &UndoRequest::steps(1))
    }

    /// The tab subscribes to changes.
    pub fn subscribe(&mut self) -> Result<(), String> {
        let c = self.conn()?;
        self.rx = Some(self.stage.session.subscribe(c).map_err(|e| e.to_string())?);
        Ok(())
    }

    /// Events received so far.
    pub fn events(&mut self) -> Vec<EventObs> {
        self.rx
            .as_ref()
            .map(|rx| rx.try_iter().map(EventObs::from).collect())
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------------------
// the agent path: registered verbs only
// ---------------------------------------------------------------------------------------

/// What a verb returned (besides its exit code).
#[derive(Debug)]
#[non_exhaustive]
pub enum VerbValue {
    None,
    Tool(ToolOutput),
    Roster(Vec<crate::session::RosterEntry>),
    Caret(Option<CursorAt>),
    View(LineRange),
    Write(WriteOutcome),
    Undo(UndoOutcome),
    Status(crate::session::Status),
    Flush(crate::session::FlushOutcome),
    Log(Vec<LogItem>),
    Diff(crate::session::DiffOutput),
    Show(Vec<(String, String)>),
    Edit(EditResult),
    Version(u64),
    Approval(Approval),
    Moved(crate::session::DiskCheck),
    Blame(crate::session::Blame),
    /// Tier 0: bytes (`cat`, `show --version`).
    Bytes(Vec<u8>),
    /// Tier 0: `write --base V`, `restore`.
    BlobWrite(crate::session::BlobWriteResult),
    /// Tier 0: `status`.
    BlobStatus(Box<crate::session::BlobStatus>),
    /// Tier 0: `log`.
    BlobLog(Vec<crate::session::BlobLogEntry>),
    /// Tier 0: `blame`.
    BlobBlame(Box<crate::session::BlobBlame>),
    /// Tier 0: `approve`, `decline` (the record, and a declined version's way back).
    BlobApproval(crate::session::BlobApproval),
    /// Tier 0: `lease`, `lease --renew`, `lease --break` (the lease taken, or broken).
    Lease(Option<crate::session::LeaseInfo>),
    /// Tier 0: `lease --release` (whether the caller held it).
    Released(bool),
    /// `claim`, `claim --renew` (the claim as it is now).
    Claim(Box<crate::session::ClaimInfo>),
    /// `claim --release` (the claims released).
    ClaimsReleased(Vec<crate::session::ClaimId>),
    /// `activity`.
    Activity(Vec<crate::session::ActivityEntry>),
}

/// One verb's result.
#[derive(Debug)]
pub struct VerbOut {
    /// The registry form that ran.
    pub verb: String,
    pub exit: u8,
    /// The refusal, when the session or tool refused the call (nothing applied).
    pub refused: Option<String>,
    pub value: VerbValue,
}

impl VerbOut {
    /// A tool's stdout (empty for other verbs).
    pub fn stdout(&self) -> &[u8] {
        match &self.value {
            VerbValue::Tool(t) => &t.stdout,
            _ => &[],
        }
    }
    /// The version a tool read or wrote.
    pub fn version(&self) -> Option<Seq> {
        match &self.value {
            VerbValue::Tool(t) => Some(t.version),
            _ => None,
        }
    }
}

/// The actor as an agent: it can only run **registered verbs**, the way a host's
/// `<host> doc <verb>` would. A verb missing from the registry fails the case.
pub struct Agent<'a> {
    stage: &'a mut Stage,
    registry: &'a Registry,
    conn: Option<ConnId>,
    rx: Option<Receiver<ChangeEvent>>,
    problems: Vec<Failure>,
    action: PersonAction,
}

// one per verb call, never stored: boxing the value buys nothing (`ToolOutput` grew notes)
#[allow(clippy::large_enum_variant)]
enum Bound {
    Value(VerbValue, u8),
    Refused(String, u8),
}

fn arg_after(args: &[String], flag: &str) -> Result<String, String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
        .or_else(|| {
            args.iter()
                .find_map(|a| a.strip_prefix(&format!("{flag}=")).map(str::to_owned))
        })
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn num(s: &str) -> Result<u64, String> {
    s.parse().map_err(|_| format!("not a number: {s:?}"))
}

fn line_col(s: &str) -> Result<LineCol, String> {
    let (l, c) = s.split_once(':').ok_or_else(|| format!("not L:C: {s:?}"))?;
    Ok(LineCol {
        line: num(l)? as usize,
        col: num(c)? as usize,
    })
}

/// `--id ID` (`claim-12` or `12`), if given.
fn claim_id(args: &[String]) -> Result<Option<crate::session::ClaimId>, String> {
    match arg_after(args, "--id") {
        Ok(s) => crate::session::ClaimId::parse(&s)
            .map(Some)
            .ok_or_else(|| format!("not a claim id: {s:?}")),
        Err(_) => Ok(None),
    }
}

fn positional(args: &[String]) -> Option<&String> {
    args.iter().find(|a| !a.starts_with('-'))
}

impl Agent<'_> {
    /// The world, for others' actions in the middle of the path.
    pub fn stage(&mut self) -> &mut Stage {
        self.stage
    }

    /// Events received since `watch` / `tail -f`.
    pub fn events(&mut self) -> Vec<EventObs> {
        self.rx
            .as_ref()
            .map(|rx| rx.try_iter().map(EventObs::from).collect())
            .unwrap_or_default()
    }

    /// Run one verb: `argv` as the agent types it after `<host> doc <path>`. The form must
    /// be registered (else the case fails: missing verb), the binding may only make the
    /// session calls the registry declares, and the exit code must be a declared one (or a
    /// session refusal).
    pub fn verb(&mut self, argv: &[&str], stdin: &[u8]) -> Result<VerbOut, String> {
        let argv: Vec<String> = argv.iter().map(|s| (*s).to_owned()).collect();
        let form = verb_form(&argv);
        let Some(spec) = self.registry.get(&form).copied() else {
            self.problems.push(Failure::MissingVerb {
                action: self.action,
                verb: form.clone(),
                at: "agent path",
            });
            return Err(format!("verb {form:?} is not in the registry"));
        };
        let mut used = Vec::new();
        let bound = self.bind(&form, &argv, stdin, &mut used)?;
        // any verb joins (P1): `join_shared` is implied for every verb
        for api in used.iter().filter(|a| **a != Api::JoinShared) {
            if !spec.api.contains(api) {
                self.problems.push(Failure::WrongApi {
                    verb: form.clone(),
                    declared: spec.api.to_vec(),
                    called: *api,
                });
            }
        }
        Ok(match bound {
            Bound::Value(value, exit) => {
                if !spec.exits.contains(&exit) {
                    self.problems.push(Failure::UndeclaredExit {
                        verb: form.clone(),
                        exit,
                    });
                }
                VerbOut {
                    verb: form,
                    exit,
                    refused: None,
                    value,
                }
            }
            Bound::Refused(why, exit) => VerbOut {
                verb: form,
                exit,
                refused: Some(why),
                value: VerbValue::None,
            },
        })
    }

    fn join(&mut self, stay: Stay, used: &mut Vec<Api>) -> Result<ConnId, SessionError> {
        if let Some(c) = self.conn {
            return Ok(c);
        }
        let p = self.stage.actor(ParticipantKind::Agent, AGENT_VIA);
        let j = if stay == Stay::Held {
            used.push(Api::Join);
            self.stage.session.join(p, Stay::Held)?
        } else {
            used.push(Api::JoinShared);
            self.stage.session.join_shared(p, Stay::UntilIdle)?
        };
        self.conn = Some(j.conn);
        Ok(j.conn)
    }

    /// The reference binding of verbs to session calls (what a host's CLI does).
    fn bind(
        &mut self,
        form: &str,
        argv: &[String],
        stdin: &[u8],
        used: &mut Vec<Api>,
    ) -> Result<Bound, String> {
        match self.bind_inner(form, argv, stdin, used) {
            Ok(b) => Ok(b),
            Err(BindError::Session(e)) => {
                Ok(Bound::Refused(e.to_string(), tools::session_exit(&e)))
            }
            Err(BindError::Tool(e)) => Ok(Bound::Refused(e.to_string(), e.exit())),
            Err(BindError::Harness(e)) => Err(e),
        }
    }

    fn bind_inner(
        &mut self,
        form: &str,
        argv: &[String],
        stdin: &[u8],
        used: &mut Vec<Api>,
    ) -> Result<Bound, BindError> {
        let args = &argv[1..];
        let s = self.stage.session.clone();
        let root = self.stage.root();
        let v = |value: VerbValue| Ok(Bound::Value(value, 0));
        match form {
            "open" => {
                self.join(Stay::Held, used)?;
                v(VerbValue::None)
            }
            "leave" => {
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Leave);
                s.leave(c)?;
                self.conn = None;
                v(VerbValue::None)
            }
            "who" => {
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Roster);
                v(VerbValue::Roster(s.roster(c)?))
            }
            "follow" => {
                let pid = positional(args).ok_or("follow needs a pid")?.clone();
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Roster);
                let caret = s
                    .roster(c)?
                    .into_iter()
                    .find(|e| e.participant.pid == pid)
                    .and_then(|e| e.cursor);
                let exit = if caret.is_some() { 0 } else { 1 };
                Ok(Bound::Value(VerbValue::Caret(caret), exit))
            }
            "cursor" => {
                let c = self.join(Stay::UntilIdle, used)?;
                let (a, h) = if let Ok(at) = arg_after(args, "--at") {
                    let p = line_col(&at)?;
                    (p, p)
                } else {
                    let sel = arg_after(args, "--select")?;
                    let (a, h) = sel.split_once('-').ok_or("--select L:C-L:C")?;
                    (line_col(a)?, line_col(h)?)
                };
                if self.stage.rights_of(ACTOR).edit {
                    used.push(Api::SetCursor);
                    s.set_cursor(c, root, At::Line(a), At::Line(h))?;
                    v(VerbValue::None)
                } else {
                    // a reader gets a viewing range instead of a caret
                    used.push(Api::SetView);
                    let r = LineRange {
                        root: root.to_owned(),
                        first: a.line.min(h.line),
                        last: a.line.max(h.line),
                    };
                    v(VerbValue::View(s.set_view(c, r)?))
                }
            }
            "write" => {
                let base = arg_after(args, "--base")?;
                let text = std::str::from_utf8(stdin).map_err(|_| "stdin is not UTF-8")?;
                let c = self.join(Stay::UntilIdle, used)?;
                if base == "live" {
                    used.push(Api::ReplaceText);
                    v(VerbValue::Edit(s.replace_text(c, root, text)?))
                } else {
                    used.push(Api::WriteBase);
                    let out = s.write_base(c, root, num(&base)?, text)?;
                    let exit = out.exit();
                    Ok(Bound::Value(VerbValue::Write(out), exit))
                }
            }
            "undo" | "undo --partial" | "undo --drop" | "redo" => {
                let steps = match arg_after(args, "--steps") {
                    Ok(n) => num(&n)? as usize,
                    Err(_) => 1,
                };
                let mode = match form {
                    "undo --partial" => UndoMode::Partial,
                    "undo --drop" => UndoMode::Drop,
                    _ => UndoMode::All,
                };
                let mut req = UndoRequest::steps(steps).mode(mode);
                if let Ok(p) = arg_after(args, "--for") {
                    req = req.for_person(p);
                }
                let c = self.join(Stay::UntilIdle, used)?;
                let out = if form == "redo" {
                    used.push(Api::RedoWith);
                    s.redo_with(c, &req)?
                } else {
                    used.push(Api::UndoWith);
                    s.undo_with(c, &req)?
                };
                let exit = out.exit();
                Ok(Bound::Value(VerbValue::Undo(out), exit))
            }
            "status" => {
                self.join(Stay::UntilIdle, used)?;
                used.push(Api::Status);
                v(VerbValue::Status(s.status()?))
            }
            "flush" => {
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Flush);
                v(VerbValue::Flush(s.flush(c)?))
            }
            "watch" | "tail -f" => {
                // a watch is its own held connection
                let p = self.stage.actor(ParticipantKind::Agent, AGENT_VIA);
                used.push(Api::Join);
                let c = s.join(p, Stay::Held)?.conn;
                if self.conn.is_none() {
                    self.conn = Some(c);
                }
                used.push(Api::Subscribe);
                self.rx = Some(s.subscribe(c)?);
                v(VerbValue::None)
            }
            "log" | "log --pending" | "approvals" => {
                let mut q = LogQuery::default();
                if let Ok(p) = arg_after(args, "--by") {
                    q = q.by(p);
                }
                if let Ok(n) = arg_after(args, "-n") {
                    q = q.limit(num(&n)? as usize);
                }
                if form == "log --pending" || form == "approvals" {
                    q = q.pending();
                }
                if let Ok(p) = arg_after(args, "--on-behalf-of") {
                    q = q.on_behalf_of(if p == "me" { ACTOR.to_owned() } else { p });
                }
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Log);
                v(VerbValue::Log(s.log(c, &q)?))
            }
            "diff --version" => {
                let from = num(&arg_after(args, "--version")?)?;
                let to = match arg_after(args, "--to") {
                    Ok(t) => Some(num(&t)?),
                    Err(_) => None,
                };
                let ctx = match arg_after(args, "-U") {
                    Ok(n) => num(&n)? as usize,
                    Err(_) => 3,
                };
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::DiffVersion);
                let d = s.diff_version(c, root, from, to, ctx)?;
                let exit = d.exit();
                Ok(Bound::Value(VerbValue::Diff(d), exit))
            }
            "show --version" => {
                let vid = num(&arg_after(args, "--version")?)?;
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Show);
                v(VerbValue::Show(s.show(c, vid)?))
            }
            "restore" => {
                let vid = num(&arg_after(args, "--version")?)?;
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Restore);
                v(VerbValue::Edit(s.restore(c, vid)?))
            }
            "version --name" => {
                let name = arg_after(args, "--name")?;
                let vid = match arg_after(args, "--version") {
                    Ok(x) => Some(num(&x)?),
                    Err(_) => None,
                };
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::NameVersion);
                v(VerbValue::Version(s.name_version(c, &name, vid)?))
            }
            "mv" => {
                let to = positional(args).ok_or("mv needs a new path")?.clone();
                self.join(Stay::UntilIdle, used)?;
                used.extend([Api::Rekey, Api::FileMoved]);
                v(VerbValue::Moved(self.stage.host_rename(&to)?))
            }
            "approve" | "decline" => {
                let seq = num(positional(args).ok_or("which change?")?)?;
                let state = if form == "approve" {
                    ApprovalState::Approved
                } else {
                    ApprovalState::Declined
                };
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::SetApproval);
                v(VerbValue::Approval(s.set_approval(c, seq, state)?))
            }
            "claim" => {
                let lines = arg_after(args, "--lines")?;
                let (a, b) = lines.split_once(',').ok_or("--lines a,b")?;
                let range = LineRange {
                    root: root.to_owned(),
                    first: num(a)? as usize,
                    last: num(b)? as usize,
                };
                let ttl = arg_after(args, "--ttl").ok().map(|t| num(&t)).transpose()?;
                let note = arg_after(args, "--note").ok();
                let hold = args.iter().any(|a| a == "--hold");
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Claim);
                v(VerbValue::Claim(Box::new(
                    s.claim(c, range, ttl, note, hold)?,
                )))
            }
            "claim --renew" => {
                let id = claim_id(args)?;
                let ttl = arg_after(args, "--ttl").ok().map(|t| num(&t)).transpose()?;
                let hold = if args.iter().any(|a| a == "--hold") {
                    Some(true)
                } else if args.iter().any(|a| a == "--no-hold") {
                    Some(false)
                } else {
                    None
                };
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::RenewClaim);
                v(VerbValue::Claim(Box::new(s.renew_claim(c, id, ttl, hold)?)))
            }
            "claim --release" => {
                let id = claim_id(args)?;
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::ReleaseClaim);
                let gone = s.release_claim(c, id)?;
                let exit = if gone.is_empty() { 1 } else { 0 };
                Ok(Bound::Value(VerbValue::ClaimsReleased(gone), exit))
            }
            "activity" => {
                let n = match arg_after(args, "-n") {
                    Ok(n) => num(&n)? as usize,
                    Err(_) => 10,
                };
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Activity);
                v(VerbValue::Activity(s.activity(c, n)?))
            }
            "blame" => {
                // `-L a,b`, `-La,b` or `-L=a,b` (git's forms); `b` may be past the end
                let spec = args
                    .iter()
                    .position(|a| a == "-L")
                    .and_then(|i| args.get(i + 1).cloned())
                    .or_else(|| {
                        args.iter().find_map(|a| {
                            a.strip_prefix("-L")
                                .filter(|r| !r.is_empty())
                                .map(|r| r.trim_start_matches('=').to_owned())
                        })
                    });
                let lines = match spec {
                    None => None,
                    Some(r) => {
                        let (a, b) = r.split_once(',').ok_or("-L a,b")?;
                        Some(LineRange {
                            root: root.to_owned(),
                            first: num(a)? as usize,
                            last: num(b)? as usize,
                        })
                    }
                };
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Blame);
                v(VerbValue::Blame(s.blame(c, root, lines)?))
            }
            "revert" => {
                let seq = num(positional(args).ok_or("which change?")?)?;
                let partial = args.iter().any(|a| a == "--partial");
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Revert);
                let out = s.revert(c, seq, partial)?;
                let exit = out.exit();
                Ok(Bound::Value(VerbValue::Undo(out), exit))
            }
            // the kit's text tools and the pack's own verbs
            _ => {
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::RunTool);
                let out = s.run_tool(c, root, argv, stdin)?;
                let exit = out.exit;
                Ok(Bound::Value(VerbValue::Tool(out), exit))
            }
        }
    }
}

/// The new file and key for `mv <to>` (inside the doc's folder).
pub fn rename_target(stage: &Stage, to: &str) -> Result<(PathBuf, DocKey), String> {
    if to.contains('/') || to.is_empty() || to.starts_with('.') {
        return Err(format!("mv: {to:?} must be a plain file name"));
    }
    Ok((
        stage.root_dir.join(to),
        DocKey {
            space: stage.key.space.clone(),
            path: to.to_owned(),
        },
    ))
}

enum BindError {
    Session(SessionError),
    Tool(tools::ToolError),
    Harness(String),
}

impl From<SessionError> for BindError {
    fn from(e: SessionError) -> Self {
        BindError::Session(e)
    }
}

impl From<tools::ToolRunError> for BindError {
    fn from(e: tools::ToolRunError) -> Self {
        match e {
            tools::ToolRunError::Tool(t) => BindError::Tool(t),
            tools::ToolRunError::Session(s) => BindError::Session(s),
        }
    }
}

impl From<String> for BindError {
    fn from(e: String) -> Self {
        BindError::Harness(e)
    }
}

impl From<&str> for BindError {
    fn from(e: &str) -> Self {
        BindError::Harness(e.to_owned())
    }
}

// ---------------------------------------------------------------------------------------
// observation
// ---------------------------------------------------------------------------------------

/// A change, as the log shows it (without its time and via).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeObs {
    pub kind: ChangeKind,
    pub of: Option<u64>,
    pub author: String,
    pub on_behalf_of: Option<String>,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub approval: Option<ApprovalState>,
}

/// A version cut during the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionObs {
    pub reason: VersionReason,
    pub name: Option<String>,
    pub authors: Vec<String>,
}

/// The actor's presence, as others see it in the roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceObs {
    pub name: String,
    pub colour: String,
    pub role: Role,
    pub away: bool,
    pub cursor: Option<(LineCol, LineCol)>,
    pub view: Option<(usize, usize)>,
    pub connections: usize,
    /// Their live claims (amendment 13): lines, hold, note, TTL as taken.
    pub claims: Vec<ClaimObs>,
}

/// A claim as others see it (amendment 13). The TTL is `expires - taken` (the same in both
/// worlds); `--for-edit` and the time left are not compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimObs {
    pub lines: (usize, usize),
    pub hold: bool,
    pub note: Option<String>,
    pub ttl_ms: u64,
}

/// Everything compared after a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// The live text.
    pub text: String,
    /// The doc's key (path) and its file's bytes.
    pub key: DocKey,
    pub file: Option<Vec<u8>>,
    pub disk: Option<DiskState>,
    pub declined: Option<String>,
    /// Changes made during the path (attribution: who, for whom, kind, line counts).
    pub changes: Vec<ChangeObs>,
    /// Versions cut during the path, without the agent-only safety cut before a large agent
    /// edit (`BeforeAgent`, K-HIST-4: a version, not a behaviour difference).
    pub versions: Vec<VersionObs>,
    /// Every approval record (seq, state, by).
    pub approvals: Vec<(Seq, ApprovalState, Option<String>)>,
    /// The actor in the roster (`None`: not present).
    pub actor: Option<PresenceObs>,
    pub exit: u8,
    pub output: Output,
}

struct Mark {
    seq: Seq,
    version_id: u64,
}

fn mark(stage: &Stage) -> Result<Mark, String> {
    Ok(Mark {
        seq: stage.version()?,
        version_id: stage.versions()?.iter().map(|v| v.id).max().unwrap_or(0),
    })
}

fn observe(stage: &Stage, m: &Mark, step: Step, via: &str) -> Result<Observation, String> {
    let s = &stage.session;
    let o = stage.observer;
    let status = s.status().map_err(|e| e.to_string())?;
    let log = s
        .log(o, &LogQuery::default())
        .map_err(|e| format!("observer log: {e}"))?;
    let mut changes = Vec::new();
    for item in log.iter().rev() {
        if let LogItem::Change { info, approval } = item {
            if info.seq <= m.seq {
                continue;
            }
            if info.author == ACTOR && info.via != via {
                return Err(format!(
                    "change {} by the actor came via {:?}, not this path's {via:?}: the path \
                     acted outside its handle",
                    info.seq, info.via
                ));
            }
            changes.push(ChangeObs {
                kind: info.kind,
                of: info.of,
                author: info.author.clone(),
                on_behalf_of: info.on_behalf_of.clone(),
                lines_added: info.lines_added,
                lines_removed: info.lines_removed,
                approval: *approval,
            });
        }
    }
    let versions = stage
        .versions()?
        .into_iter()
        .filter(|v| v.id > m.version_id && v.reason != VersionReason::BeforeAgent)
        .map(|v| VersionObs {
            reason: v.reason,
            name: v.name,
            authors: v.authors,
        })
        .collect();
    let approvals = s
        .approvals(o)
        .map_err(|e| format!("observer approvals: {e}"))?
        .into_iter()
        .map(|(seq, a)| (seq, a.state, a.by))
        .collect();
    let actor = s
        .roster(o)
        .map_err(|e| format!("observer roster: {e}"))?
        .into_iter()
        .find(|e| e.participant.pid == ACTOR)
        .map(|e| PresenceObs {
            name: e.participant.name,
            colour: e.participant.colour,
            role: e.role,
            away: e.away,
            cursor: e.cursor.map(|c| (c.anchor, c.head)),
            view: e.view.map(|v| (v.first, v.last)),
            connections: e.connections,
            claims: e
                .claims
                .iter()
                .map(|c| ClaimObs {
                    lines: (c.lines.first, c.lines.last),
                    hold: c.hold,
                    note: c.note.clone(),
                    ttl_ms: c.expires_ms - c.taken_ms,
                })
                .collect(),
        });
    let file = s
        .file_path()
        .map_err(|e| e.to_string())?
        .and_then(|p| std::fs::read(p).ok());
    Ok(Observation {
        text: stage.text()?,
        key: status.key,
        file,
        disk: status.disk,
        declined: status.declined,
        changes,
        versions,
        approvals,
        actor,
        exit: step.exit,
        output: step.output,
    })
}

fn differences(p: &Observation, a: &Observation) -> Vec<String> {
    let mut d = Vec::new();
    macro_rules! cmp {
        ($($f:ident),*) => {$(
            if p.$f != a.$f {
                d.push(format!(
                    "{}:\n      person: {:?}\n      agent:  {:?}",
                    stringify!($f), p.$f, a.$f
                ));
            }
        )*};
    }
    cmp!(text, key, file, disk, declined, changes, versions, approvals, actor, exit, output);
    d
}

// ---------------------------------------------------------------------------------------
// failures and the report
// ---------------------------------------------------------------------------------------

/// Which path failed to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Setup,
    Person,
    Agent,
}

/// One way the parity check failed. Every one is loud: the harness collects them all.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Failure {
    /// The parity list has no row for this action.
    MissingRow(PersonAction),
    /// Two rows for one action.
    DuplicateRow(PersonAction),
    /// A row (or the agent path) names a verb the registry doesn't have.
    MissingVerb {
        action: PersonAction,
        verb: String,
        at: &'static str,
    },
    /// A row lists a verb whose registry entry doesn't serve that action.
    VerbDoesNotServe { action: PersonAction, verb: String },
    /// A registered verb serves an action whose row doesn't list it (or isn't live).
    VerbNotListed { action: PersonAction, verb: String },
    /// A verb the row says is N/A or not built yet is registered.
    UnexpectedVerb {
        action: PersonAction,
        verb: String,
        why: String,
    },
    /// A row's verb has no case: nothing shows it behaves as the person path does.
    NoCase { action: PersonAction, verb: String },
    /// A deferred row has cases in the kit: the verbs landed, make the row live.
    StaleDeferral(PersonAction),
    /// The binding made a session call the registry doesn't declare for the verb.
    WrongApi {
        verb: String,
        declared: Vec<Api>,
        called: Api,
    },
    /// The verb ended with an exit code its registry entry doesn't declare.
    UndeclaredExit { verb: String, exit: u8 },
    /// A path couldn't run.
    Path {
        case: String,
        side: Side,
        error: String,
    },
    /// The agent's verb behaved differently from the person path.
    Mismatch { case: String, diffs: Vec<String> },
    /// The registry couldn't be built.
    Registry(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::MissingRow(a) => {
                write!(f, "{a}: no row in the parity list ({})", a.describe())
            }
            Failure::DuplicateRow(a) => write!(f, "{a}: more than one row"),
            Failure::MissingVerb { action, verb, at } => {
                write!(
                    f,
                    "{action}: verb {verb:?} is missing from the registry ({at})"
                )
            }
            Failure::VerbDoesNotServe { action, verb } => {
                write!(
                    f,
                    "{action}: verb {verb:?} is listed but its registry entry doesn't serve it"
                )
            }
            Failure::VerbNotListed { action, verb } => write!(
                f,
                "{action}: verb {verb:?} serves it but the row doesn't list it (or isn't live)"
            ),
            Failure::UnexpectedVerb { action, verb, why } => {
                write!(f, "{action}: verb {verb:?} exists but the row says: {why}")
            }
            Failure::NoCase { action, verb } => {
                write!(f, "{action}: verb {verb:?} has no parity case")
            }
            Failure::StaleDeferral(a) => {
                write!(
                    f,
                    "{a}: deferred in the list, but the kit now has cases: make it live"
                )
            }
            Failure::WrongApi {
                verb,
                declared,
                called,
            } => write!(
                f,
                "verb {verb:?} called {called:?}; the registry declares {declared:?}"
            ),
            Failure::UndeclaredExit { verb, exit } => {
                write!(
                    f,
                    "verb {verb:?} exited {exit}, which its registry entry doesn't declare"
                )
            }
            Failure::Path { case, side, error } => {
                write!(f, "{case}: {side:?} path failed: {error}")
            }
            Failure::Mismatch { case, diffs } => {
                write!(
                    f,
                    "{case}: the agent's verb behaved differently from the person path:"
                )?;
                for d in diffs {
                    write!(f, "\n    {d}")?;
                }
                Ok(())
            }
            Failure::Registry(e) => write!(f, "{e}"),
        }
    }
}

/// Every failure, one per line (for a test's panic message).
pub fn render(failures: &[Failure]) -> String {
    let mut s = format!("agent parity: {} failure(s)\n", failures.len());
    for f in failures {
        s.push_str(&format!("  - {f}\n"));
    }
    s
}

/// What a passing run checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Cases run and matched: (action, verb, case name).
    pub matched: Vec<(PersonAction, &'static str, &'static str)>,
    /// N/A rows, with their reason.
    pub not_applicable: Vec<(PersonAction, &'static str)>,
    /// Deferred rows: (action, until, why).
    pub deferred: Vec<(PersonAction, &'static str, &'static str)>,
}

// ---------------------------------------------------------------------------------------
// the check
// ---------------------------------------------------------------------------------------

/// Run one case: the person path on one fresh world, the agent path on another, then
/// compare. Returns the failures (empty when the paths agree).
pub fn run_case(pack: &Arc<dyn Pack>, registry: &Registry, case: &Case) -> Vec<Failure> {
    let label = format!("{case:?}");
    let fail = |side, error: String| {
        vec![Failure::Path {
            case: label.clone(),
            side,
            error,
        }]
    };
    let world = || -> Result<(Stage, Mark), String> {
        let mut st = Stage::new(pack.clone(), case.initial)?;
        (case.setup)(&mut st)?;
        let m = mark(&st)?;
        Ok((st, m))
    };
    let person = match world() {
        Err(e) => return fail(Side::Setup, e),
        Ok((mut st, m)) => {
            let mut p = Person {
                stage: &mut st,
                conn: None,
                rx: None,
            };
            let step = (case.person)(&mut p);
            drop(p);
            match step.and_then(|s| observe(&st, &m, s, PERSON_VIA)) {
                Ok(o) => o,
                Err(e) => return fail(Side::Person, e),
            }
        }
    };
    let (agent, mut problems) = match world() {
        Err(e) => return fail(Side::Setup, e),
        Ok((mut st, m)) => {
            let mut a = Agent {
                stage: &mut st,
                registry,
                conn: None,
                rx: None,
                problems: Vec::new(),
                action: case.action,
            };
            let step = (case.agent)(&mut a);
            let problems = std::mem::take(&mut a.problems);
            drop(a);
            match step.and_then(|s| observe(&st, &m, s, AGENT_VIA)) {
                Ok(o) => (o, problems),
                Err(e) => {
                    let mut f = problems;
                    // a missing verb is already reported as such
                    if !f.iter().any(|x| matches!(x, Failure::MissingVerb { .. })) {
                        f.extend(fail(Side::Agent, e));
                    }
                    return f;
                }
            }
        }
    };
    let diffs = differences(&person, &agent);
    if !diffs.is_empty() {
        problems.push(Failure::Mismatch { case: label, diffs });
    }
    problems
}

/// The parity check for one pack (K-PAR-1…3):
///
/// 1. the list has exactly one row per [`PersonAction`];
/// 2. the list and the registry agree both ways: every live row's verbs are registered and
///    serve it, every registered verb is listed under every action it serves (an alias,
///    [`super::ALIASES`], may be left out), and N/A or deferred verbs are **not** registered;
/// 3. every verb of every live (or N/A) row has a case — the kit's
///    ([`super::kit_cases`], one exhaustive `match`) or the pack's (`extra`) — and in every
///    case the agent's verb has the same effect as the person path: text, file, attribution,
///    versions, approvals, presence (role, caret, view), exit and output.
///
/// Returns every failure, not just the first.
pub fn check(
    pack: Arc<dyn Pack>,
    rows: &[ParityRow],
    registry: &Registry,
    extra: &[Case],
) -> Result<Report, Vec<Failure>> {
    if let Some(f) = wrong_tier(registry, Tier::Text) {
        return Err(vec![f]);
    }
    let (mut report, mut failures) = list_check(rows, registry, &covered(&all_cases(extra)), |a| {
        !super::kit_cases(a).is_empty()
    });
    let cases: Vec<Case> = all_cases(extra)
        .into_iter()
        .filter(|c| {
            // a deferred row's cases are reported as a stale deferral
            !rows
                .iter()
                .any(|r| r.action == c.action && matches!(r.status, RowStatus::Deferred { .. }))
        })
        .collect();
    // cases are independent worlds: run them on a few threads, keep their order
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 4);
    let chunk = cases.len().div_ceil(threads).max(1);
    let results: Vec<Vec<Failure>> = std::thread::scope(|scope| {
        let handles: Vec<_> = cases
            .chunks(chunk)
            .map(|part| {
                let pack = pack.clone();
                scope.spawn(move || {
                    part.iter()
                        .map(|c| run_case(&pack, registry, c))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| match h.join() {
                Ok(r) => r,
                Err(p) => std::panic::resume_unwind(p),
            })
            .collect()
    });
    for (c, f) in cases.iter().zip(results) {
        if f.is_empty() {
            report.matched.push((c.action, c.verb, c.name));
        }
        failures.extend(f);
    }
    if failures.is_empty() {
        Ok(report)
    } else {
        Err(failures)
    }
}

/// Steps 1 and 2 of [`check`], and "every listed verb has a case", without running the
/// cases (fast: no docs are opened). Returns every failure. A text registry (a blob one:
/// [`super::check_blob_list`]).
pub fn check_list(rows: &[ParityRow], registry: &Registry, extra: &[Case]) -> Vec<Failure> {
    if let Some(f) = wrong_tier(registry, Tier::Text) {
        return vec![f];
    }
    list_check(rows, registry, &covered(&all_cases(extra)), |a| {
        !super::kit_cases(a).is_empty()
    })
    .1
}

/// A failure when `registry` serves another tier than the harness runs.
pub(super) fn wrong_tier(registry: &Registry, tier: Tier) -> Option<Failure> {
    (registry.tier() != tier).then(|| {
        Failure::Registry(format!(
            "this harness runs {tier:?} docs; the registry is for {:?} docs",
            registry.tier()
        ))
    })
}

fn covered(cases: &[Case]) -> Vec<(PersonAction, &'static str)> {
    cases.iter().map(|c| (c.action, c.verb)).collect()
}

fn all_cases(extra: &[Case]) -> Vec<Case> {
    let mut cases: Vec<Case> = PersonAction::ALL
        .iter()
        .flat_map(|a| super::kit_cases(*a))
        .collect();
    cases.extend_from_slice(extra);
    cases
}

/// Steps 1 and 2 and case coverage, for the registry's tier: `covered` lists the (action,
/// verb) pairs that have cases, `kit_has_cases` whether the kit binds an action itself.
pub(super) fn list_check(
    rows: &[ParityRow],
    registry: &Registry,
    covered: &[(PersonAction, &'static str)],
    kit_has_cases: impl Fn(PersonAction) -> bool,
) -> (Report, Vec<Failure>) {
    let mut failures = Vec::new();
    let mut report = Report::default();

    // 1. one row per action of the registry's tier (rows for other tiers' actions may be
    // there; they are checked like any other)
    for a in PersonAction::ALL {
        match rows.iter().filter(|r| r.action == *a).count() {
            0 if !a.in_tier(registry.tier()) => {}
            0 => failures.push(Failure::MissingRow(*a)),
            1 => {}
            _ => failures.push(Failure::DuplicateRow(*a)),
        }
    }

    // 2. the list and the registry agree
    for r in rows {
        let why = match r.status {
            RowStatus::Live => "not built yet".to_owned(),
            RowStatus::Deferred { until, why } => format!("deferred to {until}: {why}"),
            RowStatus::NotApplicable { reason } => format!("N/A: {reason}"),
        };
        for v in r.verbs {
            match registry.get(v) {
                None => failures.push(Failure::MissingVerb {
                    action: r.action,
                    verb: (*v).to_owned(),
                    at: "parity list",
                }),
                // an N/A row's verbs only show its reason (P24: `cat`)
                Some(spec) if r.status == RowStatus::Live && !spec.serves.contains(&r.action) => {
                    failures.push(Failure::VerbDoesNotServe {
                        action: r.action,
                        verb: (*v).to_owned(),
                    })
                }
                Some(_) => {}
            }
        }
        for v in r.absent {
            if registry.get(v).is_some() {
                failures.push(Failure::UnexpectedVerb {
                    action: r.action,
                    verb: (*v).to_owned(),
                    why: why.clone(),
                });
            }
        }
        match r.status {
            RowStatus::Live => {}
            RowStatus::NotApplicable { reason } => report.not_applicable.push((r.action, reason)),
            RowStatus::Deferred { until, why } => {
                report.deferred.push((r.action, until, why));
                if kit_has_cases(r.action) {
                    failures.push(Failure::StaleDeferral(r.action));
                }
            }
        }
    }
    for spec in registry.verbs() {
        // an alias may be listed or not: its verb's own row is what is checked
        if spec.alias_of().is_some_and(|of| registry.get(of).is_some()) {
            continue;
        }
        for a in spec.serves {
            let listed = rows.iter().any(|r| {
                r.action == *a
                    && !matches!(r.status, RowStatus::Deferred { .. })
                    && r.verbs.contains(&spec.name)
            });
            if !listed {
                failures.push(Failure::VerbNotListed {
                    action: *a,
                    verb: spec.name.to_owned(),
                });
            }
        }
    }

    // 3. every listed verb has a case
    for r in rows {
        if matches!(r.status, RowStatus::Deferred { .. }) {
            continue;
        }
        for v in r.verbs {
            if !covered.iter().any(|(a, verb)| *a == r.action && verb == v) {
                failures.push(Failure::NoCase {
                    action: r.action,
                    verb: (*v).to_owned(),
                });
            }
        }
    }
    (report, failures)
}
