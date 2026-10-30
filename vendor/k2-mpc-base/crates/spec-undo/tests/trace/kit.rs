//! The kit side of a trace: a real session on Yrs over a real store in a temporary
//! directory, through the public session API (plus the `conformance` feature's stack-depth
//! hook). Nothing simulated.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use k2_mpc_base::session::{
    AuditEvent, ChangeEvent, Config, ConnId, Declined, Host, Pack, PackContent, PackError,
    Participant, ParticipantKind, PurgeRequest, PurgeScope, Rights, Session, SessionError,
    SessionManager, Stay, UndoMode, UndoOutcome, UndoRequest, WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore, VersionId};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::{Edit, Kind, Neutral, Schema};

pub const ROOTS: [&str; 2] = ["r0", "r1"];
const SEP: char = '\u{1e}';

/// Two text roots; the file is the two texts joined by `SEP`.
struct TwoRoots;

impl Pack for TwoRoots {
    fn schema(&self) -> Schema {
        Schema {
            name: "spec-undo-two-roots",
            roots: &[("r0", Kind::Text), ("r1", Kind::Text)],
        }
    }

    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        let declined = |m: &str| Declined {
            code: "not_two_roots",
            message: m.into(),
        };
        let s = std::str::from_utf8(bytes).map_err(|_| declined("not UTF-8"))?;
        let (a, b) = s.split_once(SEP).ok_or_else(|| declined("no separator"))?;
        Ok(PackContent {
            roots: vec![
                (ROOTS[0].into(), Neutral::Text(a.into())),
                (ROOTS[1].into(), Neutral::Text(b.into())),
            ],
            codec: Vec::new(),
        })
    }

    fn encode(&self, content: &[(String, Neutral)], _codec: &[u8]) -> Result<Vec<u8>, PackError> {
        let text = |name: &str| {
            content
                .iter()
                .find(|(r, _)| r == name)
                .map(|(_, Neutral::Text(t))| t.clone())
                .ok_or_else(|| PackError(format!("no root {name}")))
        };
        Ok(format!("{}{SEP}{}", text(ROOTS[0])?, text(ROOTS[1])?).into_bytes())
    }

    fn normalize_input<'a>(&self, _root: &str, text: &'a str) -> Cow<'a, str> {
        Cow::Borrowed(text)
    }
}

/// Everyone may do everything (owner included); time is the trace's.
struct TraceHost {
    now: AtomicU64,
}

impl Host for TraceHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct TmpDir(PathBuf);

impl TmpDir {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "mpc-spec-undo-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("make a temporary directory");
        TmpDir(p)
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The knobs a trace runs with (shared with the oracle's bounds).
pub struct Knobs {
    pub undo_depth: usize,
    pub keep_changes: u64,
    pub compact_entries: u64,
}

/// A kit element: (text root, client, clock).
pub type KitId = (usize, u64, u32);

/// One live character on the kit (char blame, feature `conformance`).
#[derive(Debug, Clone)]
pub struct KitChar {
    pub ch: char,
    /// Its element.
    pub id: KitId,
    /// The change that inserted the element, if the store recorded one.
    pub change: Option<u64>,
    /// Its provenance root (U23): `id` unless an undo, redo or revert put it back.
    pub root: KitId,
    /// The pid owning the root's client (U18a: blame reports the root's author).
    pub root_pid: Option<String>,
}

/// What a kit call returned, normalised for the comparison.
#[derive(Debug)]
pub enum KitOut {
    Edit(Result<bool, SessionError>),
    /// The outcome and the version before the write.
    Write(Result<WriteOutcome, SessionError>, u64),
    Undo(Result<UndoOutcome, SessionError>),
}

/// One doc's live session, reopened from its store on a restart.
pub struct Kit {
    store: Arc<DocStore>,
    host: Arc<TraceHost>,
    config: Config,
    key: DocKey,
    mgr: SessionManager,
    pub session: Arc<Session>,
    watcher: ConnId,
    watch: Receiver<ChangeEvent>,
    /// (pid name, connection slot) → connection.
    conns: HashMap<(String, usize), ConnId>,
    _dir: TmpDir,
}

impl Kit {
    pub fn open(seeds: [&str; 2], knobs: &Knobs) -> Self {
        let dir = TmpDir::new();
        let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).expect("open the store"));
        // the store is never reopened from its file (a restart reloads the session from it)
        store.unsynced_for_tests();
        let host = Arc::new(TraceHost {
            now: AtomicU64::new(1_000_000),
        });
        let mut config = Config::default();
        config.edit_burst = 1_000_000;
        config.edit_rate_per_s = 1_000_000;
        config.undo_depth = knobs.undo_depth;
        config.revert_keep_changes = knobs.keep_changes;
        config.revert_keep_ms = 0;
        config.history_tick_ms = 0;
        config.compact_entries = knobs.compact_entries;
        let key = DocKey {
            space: "spec-undo".into(),
            path: "trace.txt".into(),
        };
        let mgr = SessionManager::new(store.clone(), host.clone(), config.clone());
        let bytes = format!("{}{SEP}{}", seeds[0], seeds[1]);
        let session = mgr
            .open(Arc::new(TwoRoots), &key, Some(bytes.as_bytes()))
            .expect("open the session");
        let (watcher, watch) = Self::watch(&session);
        Kit {
            store,
            host,
            config,
            key,
            mgr,
            session,
            watcher,
            watch,
            conns: HashMap::new(),
            _dir: dir,
        }
    }

    fn watch(session: &Session) -> (ConnId, Receiver<ChangeEvent>) {
        let p = Participant::new(
            "watcher",
            "watcher",
            "#999999",
            ParticipantKind::Agent,
            "cli",
        );
        let c = session.join(p, Stay::Held).expect("join").conn;
        let rx = session.subscribe(c).expect("subscribe");
        (c, rx)
    }

    /// The host restarts: the session is released and loaded again from the store.
    pub fn restart(&mut self) {
        self.mgr.release().expect("release");
        let mgr = SessionManager::new(self.store.clone(), self.host.clone(), self.config.clone());
        let session = mgr
            .open(Arc::new(TwoRoots), &self.key, None)
            .expect("reopen the session");
        let (watcher, watch) = Self::watch(&session);
        self.mgr = mgr;
        self.session = session;
        self.watcher = watcher;
        self.watch = watch;
        self.conns.clear();
    }

    /// Time passes by one millisecond.
    pub fn advance(&self) {
        self.host.now.fetch_add(1, Ordering::SeqCst);
    }

    /// The host's tick (the revert window moves, U22).
    pub fn tick(&self) {
        let rep = self.mgr.tick().expect("tick");
        assert!(rep.errors.is_empty(), "tick errors: {:?}", rep.errors);
    }

    /// `pid`'s connection in `slot` (joined on first use, acting for `obo`).
    pub fn conn(&mut self, pid: &str, slot: usize, obo: Option<&str>) -> ConnId {
        if let Some(c) = self.conns.get(&(pid.to_owned(), slot)) {
            return *c;
        }
        let mut p = Participant::new(pid, pid, "#336699", ParticipantKind::Agent, "cli");
        if let Some(o) = obo {
            p = p.on_behalf_of(o);
        }
        let c = self.session.join(p, Stay::Held).expect("join").conn;
        self.conns.insert((pid.to_owned(), slot), c);
        c
    }

    /// The connection leaves (its pid's stacks stay, U6).
    pub fn leave(&mut self, pid: &str, slot: usize) {
        if let Some(c) = self.conns.remove(&(pid.to_owned(), slot)) {
            assert!(self.session.leave(c).expect("leave"));
        }
    }

    pub fn version(&self) -> u64 {
        self.session.status().expect("status").version
    }

    pub fn log_entries(&self) -> u64 {
        self.session.status().expect("status").log_entries
    }

    pub fn texts(&self) -> Vec<String> {
        let mut r = self.session.read(self.watcher).expect("read").text_by_root;
        ROOTS
            .iter()
            .map(|name| {
                let i = r.iter().position(|(n, _)| n == name).expect("root");
                r.remove(i).1
            })
            .collect()
    }

    /// Every live character of each root, in order, with its element and its provenance
    /// root (U23) as the kit's char blame reports them.
    pub fn chars(&self) -> Vec<Vec<KitChar>> {
        ROOTS
            .iter()
            .enumerate()
            .map(|(r, name)| {
                self.session
                    .char_blame(name)
                    .expect("blame")
                    .into_iter()
                    .map(|b| KitChar {
                        ch: b.ch,
                        id: (r, b.client, b.clock),
                        change: b.change,
                        root: (r, b.root_client, b.root_clock),
                        root_pid: b.root_pid,
                    })
                    .collect()
            })
            .collect()
    }

    pub fn depths(&self, pid: &str) -> (usize, usize) {
        self.session.undo_depths(pid).expect("depths")
    }

    /// The applied changes since the last call: (version, root, hunks), in order.
    pub fn applied(&self) -> Vec<(u64, usize, Vec<Hunk>)> {
        self.watch
            .try_iter()
            .map(|ev| {
                let root = ROOTS.iter().position(|r| *r == ev.root).expect("root");
                (ev.version, root, ev.hunks)
            })
            .collect()
    }

    pub fn edit(&self, c: ConnId, edits: &[(usize, Vec<Hunk>)]) -> KitOut {
        let before = self.version();
        let edits = edits
            .iter()
            .map(|(r, hunks)| Edit::Text {
                root: ROOTS[*r].into(),
                hunks: hunks.clone(),
            })
            .collect();
        KitOut::Edit(
            self.session
                .edit(c, edits, None)
                .map(|r| r.version != before),
        )
    }

    pub fn replace(&self, c: ConnId, root: usize, new: &str) -> KitOut {
        let before = self.version();
        KitOut::Edit(
            self.session
                .replace_text(c, ROOTS[root], new)
                .map(|r| r.version != before),
        )
    }

    pub fn read(&self, c: ConnId) -> u64 {
        self.session.read(c).expect("read").version
    }

    pub fn write_base(&self, c: ConnId, root: usize, base: u64, new: &str) -> KitOut {
        let before = self.version();
        KitOut::Write(self.session.write_base(c, ROOTS[root], base, new), before)
    }

    pub fn name_version(&self, c: ConnId, name: &str) -> VersionId {
        self.session.name_version(c, name, None).expect("name")
    }

    pub fn restore(&self, c: ConnId, v: VersionId) -> KitOut {
        let before = self.version();
        KitOut::Edit(self.session.restore(c, v).map(|r| r.version != before))
    }

    pub fn undo(
        &self,
        c: ConnId,
        steps: usize,
        mode: UndoMode,
        for_person: Option<&str>,
        redo: bool,
    ) -> KitOut {
        let mut req = UndoRequest::steps(steps).mode(mode);
        if let Some(p) = for_person {
            req = req.for_person(p);
        }
        KitOut::Undo(if redo {
            self.session.redo_with(c, &req)
        } else {
            self.session.undo_with(c, &req)
        })
    }

    pub fn revert(&self, c: ConnId, change: u64, partial: bool) -> KitOut {
        KitOut::Undo(self.session.revert(c, change, partial))
    }

    pub fn purge(&self, c: ConnId, secret: &str) -> KitOut {
        let before = self.version();
        let req = PurgeRequest::new(PurgeScope::Text(secret.into())).and_remove();
        KitOut::Edit(
            self.session
                .purge(c, req)
                .map(|o| o.removed.is_some_and(|r| r.version != before)),
        )
    }
}
