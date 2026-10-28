//! The parity harness for tier 0 (any file, K-PAR-3): the same check as [`super::check`],
//! on blob docs. Each case runs the **person path** (a tab: the [`BlobSession`] API
//! directly) and the **agent path** (registered verbs only, through a reference binding)
//! on two identical blob docs, then compares the file, the versions, the lease, the actor's
//! presence, the exit code and the output.

use std::collections::HashMap;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::action::PersonAction;
use super::harness::{
    list_check, wrong_tier, Failure, Output, ParityHost, ParityRow, Report, RowStatus, Side, Step,
    TmpDir, VerbOut, VerbValue, ACTOR, AGENT_VIA, OBSERVER, PERSON_VIA,
};
use super::registry::{verb_form, Api, Registry, Tier};
use crate::blob::{BlobConfig, BlobStore};
use crate::session::{
    ApprovalState, BlobPack, BlobSession, Config, ConnId, DiskCheck, DiskState, LeaseInfo,
    Participant, ParticipantKind, Rights, Role, SessionError, SessionManager, Stay,
};
use crate::store::{BlobReason, DocKey, DocStore, VersionId};
use crate::tools;

/// The blob doc's path (its file in the stage's folder).
pub const BLOB_PATH: &str = "report.pdf";
/// The default blob doc's bytes.
pub const BLOB_DOC: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog >> endobj\n%%EOF\n";
/// The stage's blob limit: a file over it is declined (`too_large`, P25).
pub const BLOB_MAX_BYTES: u64 = 64 << 10;

/// Others' activity before the paths run.
pub type BlobSetupFn = fn(&mut BlobStage) -> Result<(), String>;
/// The person path: what a tab showing the file does, through the session API directly.
pub type BlobPersonFn = fn(&mut BlobPerson<'_>) -> Result<Step, String>;
/// The agent path: registered verbs only ([`BlobAgent::verb`]).
pub type BlobAgentFn = fn(&mut BlobAgent<'_>) -> Result<Step, String>;

/// One tier-0 parity check: an action, the verb under test, the file, others' setup, and
/// both paths.
#[derive(Clone, Copy)]
pub struct BlobCase {
    pub action: PersonAction,
    /// The registry form under test (the agent path must run it).
    pub verb: &'static str,
    /// What this case shows, a few words.
    pub name: &'static str,
    pub initial: &'static [u8],
    pub setup: BlobSetupFn,
    pub person: BlobPersonFn,
    pub agent: BlobAgentFn,
}

fn no_setup(_: &mut BlobStage) -> Result<(), String> {
    Ok(())
}

impl BlobCase {
    /// A case on [`BLOB_DOC`] with no setup.
    pub const fn new(
        action: PersonAction,
        verb: &'static str,
        name: &'static str,
        person: BlobPersonFn,
        agent: BlobAgentFn,
    ) -> Self {
        BlobCase {
            action,
            verb,
            name,
            initial: BLOB_DOC,
            setup: no_setup,
            person,
            agent,
        }
    }
    pub const fn initial(self, initial: &'static [u8]) -> Self {
        BlobCase { initial, ..self }
    }
    pub const fn setup(self, setup: BlobSetupFn) -> Self {
        BlobCase { setup, ..self }
    }
}

impl fmt::Debug for BlobCase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:?} (tier 0: {})", self.action, self.verb, self.name)
    }
}

// ---------------------------------------------------------------------------------------
// the world
// ---------------------------------------------------------------------------------------

/// One tier-0 world: a store, a blob store, a manager, the file opened as a blob doc, an
/// observer, and the other participants. Setup and both paths use it for **others'**
/// actions; the actor acts only through [`BlobPerson`] or [`BlobAgent`].
pub struct BlobStage {
    host: Arc<ParityHost>,
    mgr: SessionManager,
    session: Arc<BlobSession>,
    key: DocKey,
    file: PathBuf,
    root_dir: PathBuf,
    observer: ConnId,
    others: HashMap<String, ConnId>,
    marks: HashMap<&'static str, u64>,
    // last: dropped after the stores and sessions
    _dir: TmpDir,
}

impl BlobStage {
    fn new(pack: Arc<dyn BlobPack>, initial: &[u8]) -> Result<Self, String> {
        let dir = TmpDir::new()?;
        let host = Arc::new(ParityHost {
            now: AtomicU64::new(1_700_000_000_000),
            rights: Mutex::new(HashMap::new()),
        });
        let store =
            Arc::new(DocStore::open(dir.0.join("docs.redb")).map_err(|e| format!("store: {e}"))?);
        let h = host.clone();
        let blobs = BlobStore::open(
            store.clone(),
            dir.0.join("blobs"),
            BlobConfig {
                max_blob_bytes: BLOB_MAX_BYTES,
                ..BlobConfig::default()
            },
            Arc::new(move || h.now.load(Ordering::SeqCst)),
        )
        .map_err(|e| format!("blob store: {e}"))?;
        let mgr = SessionManager::new(store, host.clone(), Config::default())
            .with_blob_store(Arc::new(blobs));
        let root_dir = dir.0.join("docs");
        let file = root_dir.join(BLOB_PATH);
        std::fs::write(&file, initial).map_err(|e| format!("write file: {e}"))?;
        let key = DocKey {
            space: "ws".into(),
            path: BLOB_PATH.into(),
        };
        let session = mgr
            .open_blob_file(pack, &key, &file, &root_dir)
            .map_err(|e| format!("open_blob_file: {e}"))?;
        let mut stage = BlobStage {
            host,
            mgr,
            session,
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
                    PERSON_VIA,
                ),
                Stay::Held,
            )
            .map_err(|e| format!("observer join: {e}"))?
            .conn;
        Ok(stage)
    }

    /// The session (for others' actions).
    pub fn session(&self) -> &Arc<BlobSession> {
        &self.session
    }
    pub fn manager(&self) -> &SessionManager {
        &self.mgr
    }
    pub fn key(&self) -> &DocKey {
        &self.key
    }
    /// The doc's file.
    pub fn file(&self) -> &Path {
        &self.file
    }
    /// The folder the file lives in (the jail root).
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
    /// Set `pid`'s rights (the actor starts with [`Rights::full`], not owner).
    pub fn grant(&self, pid: &str, r: Rights) {
        if let Ok(mut m) = self.host.rights.lock() {
            m.insert(pid.to_owned(), r);
        }
    }
    /// Remember a value (a version) for the paths.
    pub fn set_mark(&mut self, name: &'static str, v: u64) {
        self.marks.insert(name, v);
    }
    pub fn mark(&self, name: &'static str) -> Result<u64, String> {
        self.marks
            .get(name)
            .copied()
            .ok_or_else(|| format!("no mark {name:?} (set it in setup)"))
    }

    /// Join `pid` (not the actor) with full rights through a tab, or return its connection.
    pub fn other(&mut self, pid: &str) -> Result<ConnId, String> {
        self.other_as(Participant::new(
            pid,
            pid.to_uppercase(),
            "#123456",
            ParticipantKind::Person,
            PERSON_VIA,
        ))
    }

    /// Join `pid` as an assistant agent acting for `person` (an editor, through the CLI), or
    /// return its connection. Its writes record a `Pending` approval only `person` settles.
    pub fn assistant(&mut self, pid: &str, person: &str) -> Result<ConnId, String> {
        if !self.others.contains_key(pid) {
            self.grant(pid, Rights::editor());
        }
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
            return Err("the actor acts only through BlobPerson or BlobAgent".into());
        }
        if let Some(c) = self.others.get(&p.pid) {
            return Ok(*c);
        }
        let none = self
            .host
            .rights
            .lock()
            .map(|m| !m.contains_key(&p.pid))
            .unwrap_or(true);
        if none {
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

    /// `pid` writes `bytes` from the current version. Returns the new version.
    pub fn write_as(&mut self, pid: &str, bytes: &[u8]) -> Result<VersionId, String> {
        let c = self.other(pid)?;
        let base = self.version()?;
        let w = self
            .session
            .write_bytes(c, bytes, base)
            .map_err(|e| format!("{pid} write: {e}"))?;
        Ok(w.version)
    }

    /// `pid` takes the lease.
    pub fn lease_as(&mut self, pid: &str) -> Result<LeaseInfo, String> {
        let c = self.other(pid)?;
        self.session
            .acquire_lease(c, None)
            .map_err(|e| format!("{pid} lease: {e}"))
    }

    /// Another program writes the file; the host's watcher reports it.
    pub fn write_file(&self, bytes: &[u8]) -> Result<DiskCheck, String> {
        std::fs::write(&self.file, bytes).map_err(|e| format!("write file: {e}"))?;
        self.session.file_changed().map_err(|e| e.to_string())
    }

    /// The current version.
    pub fn version(&self) -> Result<VersionId, String> {
        Ok(self.session.status().map_err(|e| e.to_string())?.version)
    }

    /// The host renames the file to `to` (a plain name in the same folder) and tells the
    /// kit: `SessionManager::rekey`, then `BlobSession::file_moved`. What a tab's rename and
    /// the `mv` verb both do.
    pub fn host_rename(&mut self, to: &str) -> Result<DiskCheck, SessionError> {
        if to.contains('/') || to.is_empty() || to.starts_with('.') {
            return Err(SessionError::BadPath(format!(
                "mv: {to:?} must be a plain file name"
            )));
        }
        let new_file = self.root_dir.join(to);
        let new_key = DocKey {
            space: self.key.space.clone(),
            path: to.to_owned(),
        };
        std::fs::rename(&self.file, &new_file)
            .map_err(|e| SessionError::Disk(format!("rename: {e}")))?;
        self.mgr.rekey(&self.key, new_key.clone())?;
        let d = self.session.file_moved(&new_file)?;
        self.file = new_file;
        self.key = new_key;
        Ok(d)
    }

    fn actor(&self, kind: ParticipantKind, via: &str) -> Participant {
        Participant::new(ACTOR, "Actor", "#2a9d8f", kind, via)
    }
}

// ---------------------------------------------------------------------------------------
// the person path
// ---------------------------------------------------------------------------------------

/// The actor as a person with the file open in a tab: the session API directly, one held
/// connection.
pub struct BlobPerson<'a> {
    stage: &'a mut BlobStage,
    conn: Option<ConnId>,
}

impl BlobPerson<'_> {
    /// The world, for others' actions in the middle of the path.
    pub fn stage(&mut self) -> &mut BlobStage {
        self.stage
    }
    pub fn session(&self) -> Arc<BlobSession> {
        self.stage.session.clone()
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

    /// What the tab shows: the current bytes.
    pub fn bytes(&mut self) -> Result<Vec<u8>, SessionError> {
        let c = self.conn().map_err(SessionError::BadEdit)?;
        let r = self.stage.session.read(c)?;
        read_all(r.reader)
    }
}

fn read_all(mut r: impl Read) -> Result<Vec<u8>, SessionError> {
    let mut out = Vec::new();
    r.read_to_end(&mut out)
        .map_err(|e| SessionError::Disk(format!("read: {e}")))?;
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// the agent path
// ---------------------------------------------------------------------------------------

/// The actor as an agent on a blob doc: it can only run **registered verbs** (the tier-0
/// registry), the way a host's `<host> doc <verb>` would.
pub struct BlobAgent<'a> {
    stage: &'a mut BlobStage,
    registry: &'a Registry,
    conn: Option<ConnId>,
    problems: Vec<Failure>,
    action: PersonAction,
}

// one per verb call, never stored: boxing the value buys nothing (`ToolOutput` grew notes)
#[allow(clippy::large_enum_variant)]
enum Bound {
    Value(VerbValue, u8),
    Refused(String, u8),
}

enum BindError {
    Session(SessionError),
    Harness(String),
}

impl From<SessionError> for BindError {
    fn from(e: SessionError) -> Self {
        BindError::Session(e)
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

fn arg_after(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
        .or_else(|| {
            args.iter()
                .find_map(|a| a.strip_prefix(&format!("{flag}=")).map(str::to_owned))
        })
}

fn num(s: &str) -> Result<u64, String> {
    s.parse().map_err(|_| format!("not a number: {s:?}"))
}

fn usage(msg: impl Into<String>) -> Bound {
    let e = tools::ToolError::usage(msg);
    Bound::Refused(e.to_string(), e.exit())
}

impl BlobAgent<'_> {
    /// The world, for others' actions in the middle of the path.
    pub fn stage(&mut self) -> &mut BlobStage {
        self.stage
    }

    /// Run one verb: `argv` as the agent types it after `<host> doc <path>`. As
    /// [`super::Agent::verb`]: the form must be in the (tier-0) registry, the binding may
    /// only make the declared calls, and the exit must be declared (or a session refusal).
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
        let bound = match self.bind(&form, &argv, stdin, &mut used) {
            Ok(b) => b,
            Err(BindError::Session(e)) => Bound::Refused(e.to_string(), tools::session_exit(&e)),
            Err(BindError::Harness(e)) => return Err(e),
        };
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

    /// The reference binding of verbs to blob-session calls (what a host's CLI does).
    fn bind(
        &mut self,
        form: &str,
        argv: &[String],
        stdin: &[u8],
        used: &mut Vec<Api>,
    ) -> Result<Bound, BindError> {
        let args = &argv[1..];
        let s = self.stage.session.clone();
        let v = |value: VerbValue| Ok(Bound::Value(value, 0));
        let ttl = match arg_after(args, "--ttl") {
            Some(t) => Some(num(&t)?),
            None => None,
        };
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
            "cat" => {
                if !args.is_empty() {
                    return Ok(usage(
                        "cat on a blob doc takes no flags (it is the file's bytes)",
                    ));
                }
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Read);
                v(VerbValue::Bytes(read_all(s.read(c)?.reader)?))
            }
            "who" => {
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Roster);
                v(VerbValue::Roster(s.roster(c)?))
            }
            "write" => {
                let Some(base) = arg_after(args, "--base") else {
                    return Ok(usage("write needs --base V"));
                };
                if base == "live" {
                    return Ok(usage(
                        "write --base live: a blob doc is never last-writer-wins; name the version",
                    ));
                }
                let base = num(&base)?;
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::WriteBase);
                v(VerbValue::BlobWrite(s.write_bytes(c, stdin, base)?))
            }
            "status" => {
                self.join(Stay::UntilIdle, used)?;
                used.push(Api::Status);
                v(VerbValue::BlobStatus(Box::new(s.status()?)))
            }
            "flush" => {
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Flush);
                v(VerbValue::Flush(s.flush(c)?))
            }
            "log" | "log --pending" | "approvals" => {
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Log);
                let mut log = s.log(c)?;
                if form == "log --pending" || form == "approvals" {
                    log.retain(|e| e.approval == Some(ApprovalState::Pending));
                }
                if let Some(p) = arg_after(args, "--on-behalf-of") {
                    let p = if p == "me" { ACTOR.to_owned() } else { p };
                    log.retain(|e| e.on_behalf_of.as_ref() == Some(&p));
                }
                if let Some(by) = arg_after(args, "--by") {
                    log.retain(|e| e.author == by);
                }
                if let Some(n) = arg_after(args, "-n") {
                    let n = num(&n)? as usize;
                    log.drain(..log.len().saturating_sub(n));
                }
                v(VerbValue::BlobLog(log))
            }
            "show --version" => {
                let Some(vid) = arg_after(args, "--version") else {
                    return Ok(usage("show --version V"));
                };
                let vid = num(&vid)?;
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Show);
                v(VerbValue::Bytes(read_all(s.read_version(c, vid)?.reader)?))
            }
            "restore" => {
                let Some(vid) = arg_after(args, "--version") else {
                    return Ok(usage("restore --version V"));
                };
                let vid = num(&vid)?;
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Restore);
                v(VerbValue::BlobWrite(s.restore(c, vid)?))
            }
            "version --name" => {
                let Some(name) = arg_after(args, "--name") else {
                    return Ok(usage("version --name NAME [--version V]"));
                };
                let c = self.join(Stay::UntilIdle, used)?;
                let vid = match arg_after(args, "--version") {
                    Some(x) => num(&x)?,
                    None => s.status()?.version,
                };
                used.push(Api::NameVersion);
                s.name_version(c, vid, &name)?;
                v(VerbValue::Version(vid))
            }
            "mv" => {
                let Some(to) = args.iter().find(|a| !a.starts_with('-')).cloned() else {
                    return Ok(usage("mv <new path>"));
                };
                self.join(Stay::UntilIdle, used)?;
                used.extend([Api::Rekey, Api::FileMoved]);
                v(VerbValue::Moved(self.stage.host_rename(&to)?))
            }
            "approve" | "decline" => {
                let Some(vid) = args.iter().find(|a| !a.starts_with('-')) else {
                    return Ok(usage(format!("{form} <version>")));
                };
                let vid = num(vid)?;
                let state = if form == "approve" {
                    ApprovalState::Approved
                } else {
                    ApprovalState::Declined
                };
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::SetApproval);
                v(VerbValue::BlobApproval(s.set_approval(c, vid, state)?))
            }
            "blame" => {
                if !args.is_empty() {
                    return Ok(usage("blame on a blob doc takes no flags (no lines)"));
                }
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::Blame);
                v(VerbValue::BlobBlame(Box::new(s.blame(c)?)))
            }
            "lease" => {
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::AcquireLease);
                v(VerbValue::Lease(Some(s.acquire_lease(c, ttl)?)))
            }
            "lease --renew" => {
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::RenewLease);
                v(VerbValue::Lease(Some(s.renew_lease(c, ttl)?)))
            }
            "lease --release" => {
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::ReleaseLease);
                let held = s.release_lease(c)?;
                Ok(Bound::Value(VerbValue::Released(held), u8::from(!held)))
            }
            "lease --break" => {
                let c = self.join(Stay::UntilIdle, used)?;
                used.push(Api::BreakLease);
                let l = s.break_lease(c)?;
                let exit = u8::from(l.is_none());
                Ok(Bound::Value(VerbValue::Lease(l), exit))
            }
            other => Err(BindError::Harness(format!(
                "verb {other:?} is registered for blob docs but has no binding"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------------------
// observation
// ---------------------------------------------------------------------------------------

/// A blob version, as compared: everything but its surface (`via`, checked against the
/// path) and its blob id (keyed per store, so it differs between the two worlds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobVersionObs {
    pub id: VersionId,
    pub reason: BlobReason,
    pub author: String,
    pub author_name: String,
    pub on_behalf_of: Option<String>,
    pub name: Option<String>,
    pub restored_from: Option<VersionId>,
    pub size: u64,
    pub at_ms: u64,
    pub redacted: bool,
    pub conflict_copy: bool,
}

/// The actor's presence on a blob doc, as others see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobPresenceObs {
    pub name: String,
    pub colour: String,
    pub role: Role,
    pub away: bool,
    pub holds_lease: bool,
    pub region: Option<String>,
    pub connections: usize,
}

/// Everything compared after a tier-0 path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobObservation {
    pub key: DocKey,
    /// The file's bytes.
    pub file: Option<Vec<u8>>,
    pub disk: Option<DiskState>,
    /// The current version.
    pub version: VersionId,
    /// Every version (names, purges and new versions show here).
    pub versions: Vec<BlobVersionObs>,
    pub lease: Option<LeaseInfo>,
    /// Every approval record (version, state, settled by).
    pub approvals: Vec<(VersionId, ApprovalState, Option<String>)>,
    /// The actor in the roster (`None`: not present).
    pub actor: Option<BlobPresenceObs>,
    pub exit: u8,
    pub output: Output,
}

fn observe(stage: &BlobStage, step: Step, via: &str) -> Result<BlobObservation, String> {
    let s = &stage.session;
    let o = stage.observer;
    let status = s.status().map_err(|e| e.to_string())?;
    let mut versions = Vec::new();
    for (id, v) in s
        .versions(o)
        .map_err(|e| format!("observer versions: {e}"))?
    {
        if v.author == ACTOR && v.via != via {
            return Err(format!(
                "version {id} by the actor came via {:?}, not this path's {via:?}: the path \
                 acted outside its handle",
                v.via
            ));
        }
        versions.push(BlobVersionObs {
            id,
            reason: v.reason,
            author: v.author,
            author_name: v.author_name,
            on_behalf_of: v.on_behalf_of,
            name: v.name,
            restored_from: v.restored_from,
            size: v.size,
            at_ms: v.at_ms,
            redacted: v.redacted,
            conflict_copy: v.conflict_copy.is_some(),
        });
    }
    let actor = s
        .roster(o)
        .map_err(|e| format!("observer roster: {e}"))?
        .into_iter()
        .find(|e| e.participant.pid == ACTOR)
        .map(|e| BlobPresenceObs {
            name: e.participant.name,
            colour: e.participant.colour,
            role: e.role,
            away: e.away,
            holds_lease: e.holds_lease,
            region: e.region,
            connections: e.connections,
        });
    let file = s
        .file_path()
        .map_err(|e| e.to_string())?
        .and_then(|p| std::fs::read(p).ok());
    let approvals = s
        .approvals(o)
        .map_err(|e| format!("observer approvals: {e}"))?
        .into_iter()
        .map(|(v, a)| (v, a.state, a.by))
        .collect();
    Ok(BlobObservation {
        key: status.key,
        file,
        disk: status.disk,
        version: status.version,
        versions,
        lease: status.lease,
        approvals,
        actor,
        exit: step.exit,
        output: step.output,
    })
}

fn differences(p: &BlobObservation, a: &BlobObservation) -> Vec<String> {
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
    cmp!(key, file, disk, version, versions, lease, approvals, actor, exit, output);
    d
}

// ---------------------------------------------------------------------------------------
// the check
// ---------------------------------------------------------------------------------------

/// Run one tier-0 case: the person path on one fresh world, the agent path on another, then
/// compare. Returns the failures (empty when the paths agree).
pub fn run_blob_case(
    pack: &Arc<dyn BlobPack>,
    registry: &Registry,
    case: &BlobCase,
) -> Vec<Failure> {
    if let Some(f) = wrong_tier(registry, Tier::Blob) {
        return vec![f];
    }
    let label = format!("{case:?}");
    let fail = |side, error: String| {
        vec![Failure::Path {
            case: label.clone(),
            side,
            error,
        }]
    };
    let world = || -> Result<BlobStage, String> {
        let mut st = BlobStage::new(pack.clone(), case.initial)?;
        (case.setup)(&mut st)?;
        Ok(st)
    };
    let person = match world() {
        Err(e) => return fail(Side::Setup, e),
        Ok(mut st) => {
            let step = (case.person)(&mut BlobPerson {
                stage: &mut st,
                conn: None,
            });
            match step.and_then(|s| observe(&st, s, PERSON_VIA)) {
                Ok(o) => o,
                Err(e) => return fail(Side::Person, e),
            }
        }
    };
    let (agent, mut problems) = match world() {
        Err(e) => return fail(Side::Setup, e),
        Ok(mut st) => {
            let mut a = BlobAgent {
                stage: &mut st,
                registry,
                conn: None,
                problems: Vec::new(),
                action: case.action,
            };
            let step = (case.agent)(&mut a);
            let problems = std::mem::take(&mut a.problems);
            drop(a);
            match step.and_then(|s| observe(&st, s, AGENT_VIA)) {
                Ok(o) => (o, problems),
                Err(e) => {
                    let mut f = problems;
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

fn all_blob_cases(extra: &[BlobCase]) -> Vec<BlobCase> {
    let mut cases: Vec<BlobCase> = PersonAction::ALL
        .iter()
        .flat_map(|a| super::kit_blob_cases(*a))
        .collect();
    cases.extend_from_slice(extra);
    cases
}

fn covered(cases: &[BlobCase]) -> Vec<(PersonAction, &'static str)> {
    cases.iter().map(|c| (c.action, c.verb)).collect()
}

/// The parity check for tier 0 (any file, or a tier-0.5 [`BlobPack`]), as [`super::check`]
/// is for a text pack: one row per action of [`Tier::Blob`]; the list and the tier-0
/// registry ([`Registry::for_tier`]`(Tier::Blob)`) agree both ways; every listed verb has a
/// case ([`super::kit_blob_cases`], or `extra`), and in every case the agent's verb has the
/// same effect as the person path: file, versions, lease, presence, exit and output.
/// Returns every failure.
pub fn check_blob(
    pack: Arc<dyn BlobPack>,
    rows: &[ParityRow],
    registry: &Registry,
    extra: &[BlobCase],
) -> Result<Report, Vec<Failure>> {
    if let Some(f) = wrong_tier(registry, Tier::Blob) {
        return Err(vec![f]);
    }
    let all = all_blob_cases(extra);
    let (mut report, mut failures) = list_check(rows, registry, &covered(&all), |a| {
        !super::kit_blob_cases(a).is_empty()
    });
    for c in all.iter().filter(|c| {
        !rows
            .iter()
            .any(|r| r.action == c.action && matches!(r.status, RowStatus::Deferred { .. }))
    }) {
        let f = run_blob_case(&pack, registry, c);
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

/// Steps 1 and 2 of [`check_blob`] and case coverage, without opening docs.
pub fn check_blob_list(
    rows: &[ParityRow],
    registry: &Registry,
    extra: &[BlobCase],
) -> Vec<Failure> {
    if let Some(f) = wrong_tier(registry, Tier::Blob) {
        return vec![f];
    }
    list_check(rows, registry, &covered(&all_blob_cases(extra)), |a| {
        !super::kit_blob_cases(a).is_empty()
    })
    .1
}
