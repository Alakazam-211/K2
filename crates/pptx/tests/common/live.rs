//! A live session over a deck file: the kit's session manager, a store, a blob store for the
//! conflict copies and the carrier, and a host that grants everyone everything.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Host, Participant, ParticipantKind, Rights, Session, SessionError,
    SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{ToolOutput, ToolRunError};
use k2_mpc_pptx::live::{PptxLive, SHAPES};

pub struct TestHost {
    pub now: AtomicU64,
    pub events: Mutex<Vec<AuditEvent>>,
}

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, e: &AuditEvent) {
        self.events.lock().unwrap().push(e.clone());
    }
}

pub struct Fx {
    dir: PathBuf,
    pub ws: PathBuf,
    pub host: Arc<TestHost>,
    pub store: Arc<DocStore>,
    pub blobs: Arc<BlobStore>,
    pub mgr: SessionManager,
    pub pack: Arc<PptxLive>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub const NAME: &str = "talk.pptx";

/// A fixture with the kit's default config, as `set` changes it.
pub fn fx(tag: &str, set: impl FnOnce(&mut Config)) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2pptx-live-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&p);
    let ws = p.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_793_111_400_000),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(p.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            p.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    set(&mut c);
    let mgr = SessionManager::new(store.clone(), host.clone(), c).with_blob_store(blobs.clone());
    Fx {
        dir: p,
        ws,
        host,
        store,
        blobs,
        mgr,
        pack: Arc::new(PptxLive::default()),
    }
}

pub fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: NAME.into(),
    }
}

impl Fx {
    pub fn path(&self) -> PathBuf {
        self.ws.join(NAME)
    }
    pub fn write(&self, bytes: &[u8]) {
        fs::write(self.path(), bytes).unwrap();
    }
    pub fn bytes(&self) -> Vec<u8> {
        fs::read(self.path()).unwrap()
    }
    pub fn try_open(&self) -> Result<Arc<Session>, SessionError> {
        self.mgr
            .open_file(self.pack.clone(), &key(), &self.path(), &self.ws)
    }
    pub fn open(&self) -> Arc<Session> {
        self.try_open().expect("the deck opens live")
    }
    pub fn join(&self, s: &Session, pid: &str, kind: ParticipantKind) -> ConnId {
        s.join(
            Participant::new(pid, pid, "#123456", kind, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
    pub fn person(&self, s: &Session) -> ConnId {
        self.join(s, "ada", ParticipantKind::Person)
    }
    pub fn agent(&self, s: &Session) -> ConnId {
        self.join(s, "atlas", ParticipantKind::Agent)
    }
    /// A restart: a new manager on the same stores and a new pack (nothing the old process
    /// held in memory survives), the old manager dropped without a write-back. Drop every
    /// session of the old manager first.
    pub fn restart(&mut self) {
        self.mgr = SessionManager::new(
            self.store.clone(),
            self.host.clone(),
            self.mgr.config().clone(),
        )
        .with_blob_store(self.blobs.clone());
        self.pack = Arc::new(PptxLive::default());
    }
    /// The doc, opened from the store alone: no file is read.
    pub fn open_without_file(&self) -> Result<Arc<Session>, SessionError> {
        self.mgr.open(self.pack.clone(), &key(), None)
    }
    /// Move the host's clock on.
    pub fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
}

/// An app's save: written beside the file, then renamed over it.
pub fn app_save(path: &Path, bytes: &[u8]) {
    let tmp = path.with_file_name(".app-save.tmp");
    fs::write(&tmp, bytes).unwrap();
    fs::rename(&tmp, path).unwrap();
}

pub fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

/// Run a verb as `c`, with no base.
pub fn run(s: &Session, c: ConnId, a: &[&str]) -> Result<ToolOutput, ToolRunError> {
    s.run_tool(c, SHAPES, &args(a), b"")
}

/// Run an edit verb as `c`, with the version it last read as its base.
pub fn run_based(s: &Session, c: ConnId, a: &[&str]) -> Result<ToolOutput, ToolRunError> {
    let v = s.read(c).unwrap().version;
    let mut all = args(a);
    all.push("--base".into());
    all.push(v.to_string());
    s.run_tool(c, SHAPES, &all, b"")
}

pub fn stdout(o: &ToolOutput) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// The live doc's roots, as `c` reads them.
pub fn content(s: &Session, c: ConnId) -> Vec<(String, Neutral)> {
    s.read(c).unwrap().content
}
