//! A live kit session for the doc-level capabilities ([`crate::kit_undo::KitUndo`],
//! [`crate::kit_history::KitHistory`]): the public session API over a real store in a
//! temporary directory, nothing simulated.
//!
//! Each agent is a participant with its own pid (random per doc, so a byte scan for it can't
//! match anything else) and full rights, owner included: rights are the host's policy, not
//! a rule's. A watcher connection subscribes to the session's change events: each
//! **version** is one atomic change (one engine batch, one log entry), with its author and
//! the roots it touched.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use k2_mpc_base::session::{
    AuditEvent, ChangeEvent, Config, ConnId, Host, Pack, Participant, ParticipantKind, Rights,
    Session, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};

use crate::caps::Change;
use crate::Hunk;

/// SplitMix64's finalizer: distinct inputs give unrelated outputs.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The hunks one change applied, per root: (root index, hunks against its text before).
pub(crate) type RootHunks = Vec<(usize, Vec<Hunk>)>;

/// Everyone may do everything; time stands still.
struct OpenHost;

impl Host for OpenHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        1_000_000
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

/// A temporary directory, removed on drop.
struct TmpDir(PathBuf);

impl TmpDir {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "mpc-conformance-{}-{}",
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

/// One doc's live session, its store, and a connection per agent.
pub(crate) struct KitDoc {
    pub session: Arc<Session>,
    pub store: Arc<DocStore>,
    /// The watcher's connection: reads, history and the change events.
    pub watcher: ConnId,
    roots: &'static [&'static str],
    conns: HashMap<u64, ConnId>,
    seed: u64,
    watch: Receiver<ChangeEvent>,
    // kept alive for the session; dropped last
    _mgr: SessionManager,
    _dir: TmpDir,
}

impl KitDoc {
    /// Open `pack`'s doc `name`, imported from `bytes`, with text roots `roots` (in the
    /// order the rules index them).
    pub fn open(
        pack: Arc<dyn Pack>,
        name: &str,
        bytes: &[u8],
        roots: &'static [&'static str],
    ) -> Self {
        let dir = TmpDir::new();
        let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).expect("open the store"));
        let mut config = Config::default();
        config.edit_burst = 100_000;
        config.edit_rate_per_s = 100_000;
        let mgr = SessionManager::new(store.clone(), Arc::new(OpenHost), config);
        let key = DocKey {
            space: "conformance".into(),
            path: name.into(),
        };
        let session = mgr.open(pack, &key, Some(bytes)).expect("open the session");
        let seed: u64 = rand::random();
        let p = Participant::new(
            format!("watcher-{:016x}", mix(seed)),
            "watcher",
            "#999999",
            ParticipantKind::Agent,
            "cli",
        );
        let watcher = session.join(p, Stay::UntilIdle).expect("join").conn;
        let watch = session.subscribe(watcher).expect("subscribe");
        KitDoc {
            session,
            store,
            watcher,
            roots,
            conns: HashMap::new(),
            seed,
            watch,
            _mgr: mgr,
            _dir: dir,
        }
    }

    /// `agent`'s pid on this doc: its number and a suffix of its own, random per doc, so a
    /// byte scan for one pid doesn't find another's.
    pub fn pid(&self, agent: u64) -> String {
        format!("agent{agent}-{:016x}", mix(self.seed ^ agent))
    }

    /// The agent whose pid is `pid`.
    pub(crate) fn agent_of(&self, pid: &str) -> Option<u64> {
        let (n, _) = pid.strip_prefix("agent")?.split_once('-')?;
        let n: u64 = n.parse().ok()?;
        (self.pid(n) == pid).then_some(n)
    }

    /// `agent`'s connection (joined on first use).
    pub fn conn(&mut self, agent: u64) -> ConnId {
        if let Some(c) = self.conns.get(&agent) {
            return *c;
        }
        let pid = self.pid(agent);
        let p = Participant::new(pid, "agent", "#336699", ParticipantKind::Agent, "cli");
        let c = self.session.join(p, Stay::UntilIdle).expect("join").conn;
        self.conns.insert(agent, c);
        c
    }

    /// Forget `agent`'s connection (the session removed it).
    pub fn forget(&mut self, agent: u64) {
        self.conns.remove(&agent);
    }

    /// The changes since the last call: one per version.
    pub fn changes(&self) -> Vec<Change> {
        self.applied().into_iter().map(|(c, _)| c).collect()
    }

    /// The changes since the last call, one per version, each with the hunks the engine
    /// applied per root (root index, hunks against that root's text before it).
    pub fn applied(&self) -> Vec<(Change, RootHunks)> {
        type Entry = (u64, BTreeSet<usize>, RootHunks);
        let mut by_version: BTreeMap<u64, Entry> = BTreeMap::new();
        for ev in self.watch.try_iter() {
            let author = self
                .agent_of(&ev.pid)
                .unwrap_or_else(|| panic!("a change by {:?}", ev.pid));
            let root = self
                .roots
                .iter()
                .position(|r| *r == ev.root)
                .expect("a known root");
            let e = by_version
                .entry(ev.version)
                .or_insert((author, BTreeSet::new(), Vec::new()));
            assert_eq!(e.0, author, "one version, two authors");
            e.1.insert(root);
            e.2.push((root, ev.hunks));
        }
        by_version
            .into_values()
            .map(|(author, roots, hunks)| {
                let c = Change {
                    author,
                    roots: roots.into_iter().collect(),
                };
                (c, hunks)
            })
            .collect()
    }

    /// Every root's current text, in `roots` order.
    pub fn texts(&self) -> Vec<String> {
        let mut r = self.session.read(self.watcher).expect("read").text_by_root;
        self.roots
            .iter()
            .map(|name| {
                let i = r.iter().position(|(n, _)| n == name).expect("root");
                r.remove(i).1
            })
            .collect()
    }
}
