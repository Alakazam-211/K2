//! The packs inside the kit: a blob session indexes a photoplot and a drill file through the
//! kit's job (in process and through the worker's wire form), the kit accepts every key as the
//! file's own, a new version keeps the keys of what it didn't change, and agents can't write
//! the files.

mod support;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::index::{index_to_wire, IndexStatus, IndexView, KeyClass};
use k2_mpc_base::session::{
    AuditEvent, BlobPack, BlobSession, Config, ConnId, Host, Participant, ParticipantKind, Rights,
    SessionError, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_fab::{DrillPack, PhotoplotPack};
use support::{BOARD, BOARD_XNC};

struct TestHost {
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::none())
    }
    fn now_ms(&self) -> u64 {
        1_000_000
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2fab-session-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(|| 1_000_000),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host.clone(), c).with_blob_store(blobs);
    Fx { dir, host, mgr }
}

impl Fx {
    fn open(&self, pack: Arc<dyn BlobPack>, path: &str, bytes: &[u8]) -> Arc<BlobSession> {
        let key = DocKey {
            space: "ws".into(),
            path: path.into(),
        };
        self.mgr
            .open_blob(pack, &key, Some(&mut &bytes[..]))
            .unwrap()
    }
    fn join(&self, s: &BlobSession, pid: &str, kind: ParticipantKind, r: Rights) -> ConnId {
        self.host.rights.lock().unwrap().insert(pid.into(), r);
        s.join(
            Participant::new(pid, pid, "#123456", kind, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
}

/// Runs the current version's job and returns its view.
fn indexed(s: &BlobSession) -> IndexView {
    let st = s
        .index_job(None)
        .unwrap()
        .expect("a job is due")
        .run()
        .expect("the job records");
    assert!(matches!(st, IndexStatus::Ready { .. }), "{st:?}");
    s.index_view(None).unwrap()
}

#[test]
fn both_packs_index_through_the_kits_job() {
    let f = fx("both");
    for (pack, path, bytes, first) in [
        (
            Arc::new(PhotoplotPack) as Arc<dyn BlobPack>,
            "top.gbr",
            BOARD,
            "@family:apertures",
        ),
        (Arc::new(DrillPack), "pth.xnc", BOARD_XNC, "@family:tools"),
    ] {
        let s = f.open(pack, path, bytes.as_bytes());
        let v = indexed(&s);
        assert_eq!(v.nodes[0].key.as_str(), first, "{path}");
        for n in &v.nodes {
            assert_eq!(n.key_class, KeyClass::Native, "{path}: {} minted", n.key);
            assert_eq!(n.parent.is_none(), n.kind == "family", "{path}: {}", n.key);
        }
    }
}

#[test]
fn a_new_version_keeps_the_keys_of_what_it_kept() {
    let f = fx("versions");
    let s = f.open(Arc::new(PhotoplotPack), "top.gbr", BOARD.as_bytes());
    let alice = f.join(&s, "alice", ParticipantKind::Person, Rights::editor());
    let v1 = indexed(&s);
    let moved = BOARD.replace("X8000000Y8000000D03*", "X8100000Y8000000D03*");
    s.write_bytes(alice, moved.as_bytes(), s.status().unwrap().version)
        .unwrap();
    let v2 = indexed(&s);
    let keys = |v: &IndexView| v.nodes.iter().map(|n| n.key.clone()).collect::<Vec<_>>();
    assert_eq!(keys(&v1), keys(&v2));
    let d12 = |v: &IndexView| {
        v.nodes
            .iter()
            .find(|n| n.key.as_str() == "@objects:obround,1.2,0.8;SMDPad,CuDef")
            .map(|n| n.content_hash)
    };
    assert_ne!(
        d12(&v1),
        d12(&v2),
        "the moved pad changed its group's content"
    );
}

#[test]
fn an_agent_cannot_write_the_file_and_a_person_can() {
    let f = fx("agent");
    let s = f.open(Arc::new(DrillPack), "pth.xnc", BOARD_XNC.as_bytes());
    let bot = f.join(&s, "bot", ParticipantKind::Agent, Rights::editor());
    let v = s.status().unwrap().version;
    let e = s.write_bytes(bot, b"agent bytes", v).unwrap_err();
    assert_eq!(e, SessionError::AgentWriteRefused);
    assert_eq!(e.report().id(), "MPC-E715");
    let alice = f.join(&s, "alice", ParticipantKind::Person, Rights::editor());
    s.write_bytes(
        alice,
        BOARD_XNC.replace("X8.0Y8.0", "X8.5Y8.0").as_bytes(),
        v,
    )
    .unwrap();
}

#[test]
fn a_worker_result_commits_like_an_in_process_run() {
    let f = fx("wire");
    let s = f.open(Arc::new(DrillPack), "pth.xnc", BOARD_XNC.as_bytes());
    let mut job = s.index_job(None).unwrap().expect("a job");
    let (mut input, limits) = job.open().unwrap();
    let wire = index_to_wire(&DrillPack, &mut input, job.size(), &limits).unwrap();
    let st = job.commit_wire(input, &wire).unwrap();
    assert!(
        matches!(st, IndexStatus::Ready { version: 1, .. }),
        "{st:?}"
    );
    let v = s.index_view(None).unwrap();
    assert_eq!(v.source, "drill-xnc-2021.11");
    let hits = v
        .nodes
        .iter()
        .find(|n| n.key.as_str() == "@hits:0.3;ViaDrill")
        .unwrap();
    assert_eq!(
        hits.parent.as_ref().map(|k| k.as_str()),
        Some("@tool:0.3;ViaDrill")
    );
}

#[test]
fn a_refused_file_is_recorded_not_indexed() {
    let f = fx("refused");
    let s = f.open(
        Arc::new(PhotoplotPack),
        "bad.gbr",
        b"%MOMM*%\n%ADD10C,0.1*%\nD10*\nX0Y0D03*\nM02*\n",
    );
    let st = s
        .index_job(None)
        .unwrap()
        .expect("a job is due")
        .run()
        .expect("the job records");
    assert!(
        format!("{st:?}").contains("missing-format"),
        "recorded as {st:?}"
    );
}
