//! The pack inside the kit: a blob session indexes a drawing through the kit's job (in process
//! and through the worker's wire form), the kit keeps every handle as the file's own key and
//! mints keys only where the file gives none (or gives one twice), and a person's new version
//! keeps the keys of what it didn't change.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::index::{index_to_wire, IndexStatus, IndexView, KeyClass};
use k2_mpc_base::session::{
    AuditEvent, BlobPack, BlobSession, Config, ConnId, Host, Participant, ParticipantKind, Rights,
    SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_dxf::testgen::{self, Ent, Spec, Ver};
use k2_mpc_dxf::DxfPack;

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
        "k2cad-session-{tag}-{}-{}",
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
    fn open(&self, path: &str, bytes: &[u8]) -> Arc<BlobSession> {
        let key = DocKey {
            space: "ws".into(),
            path: path.into(),
        };
        self.mgr
            .open_blob(
                Arc::new(DxfPack) as Arc<dyn BlobPack>,
                &key,
                Some(&mut &bytes[..]),
            )
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
fn the_kit_keeps_handles_as_keys() {
    let f = fx("keys");
    let b = testgen::sampler(Ver::R2018).build();
    let s = f.open("plan.dxf", &b.bytes);
    let v = indexed(&s);
    assert_eq!(v.nodes[0].key.as_str(), "@doc:dxf");
    assert_eq!(v.attrs.get("units").map(String::as_str), Some("mm"));
    let mut native = 0;
    for n in &v.nodes {
        assert_eq!(n.key_class, KeyClass::Native, "{} minted", n.key);
        native += 1;
    }
    let wall = v
        .nodes
        .iter()
        .find(|n| n.key.as_str() == b.addr("wall"))
        .expect("the wall by its handle");
    assert_eq!(wall.attrs.get("layer").map(String::as_str), Some("A-WALL"));
    println!("the_kit_keeps_handles_as_keys: {native} nodes keyed by the file");
}

#[test]
fn the_kit_mints_keys_where_the_file_gives_none() {
    let f = fx("minted");
    let mut s = testgen::sampler(Ver::R12);
    s.opts.handles = false;
    let sess = f.open("old.dxf", &s.build().bytes);
    let v = indexed(&sess);
    let minted = v
        .nodes
        .iter()
        .filter(|n| matches!(n.key_class, KeyClass::Minted { .. }))
        .count();
    assert!(minted >= 10, "only {minted} minted keys");
    let dup = Spec::new(Ver::R2018)
        .model(Ent::line([0.0, 0.0], [1.0, 0.0]).handle(0x500))
        .model(Ent::line([2.0, 0.0], [3.0, 0.0]).handle(0x500))
        .build();
    let sess = f.open("dup.dxf", &dup.bytes);
    let v = indexed(&sess);
    let twice: Vec<_> = v
        .nodes
        .iter()
        .filter(|n| n.native.as_ref().map(|k| k.as_str()) == Some("@h:500"))
        .collect();
    assert_eq!(twice.len(), 2);
    assert!(
        twice
            .iter()
            .all(|n| matches!(n.key_class, KeyClass::Minted { .. })),
        "a handle given twice is the file's own key for neither"
    );
}

#[test]
fn a_new_version_keeps_the_keys_of_what_it_kept() {
    let f = fx("versions");
    let base = || {
        Spec::new(Ver::R2013)
            .layer("A-WALL", 1)
            .model(
                Ent::line([0.0, 0.0], [10.0, 0.0])
                    .on("A-WALL")
                    .label("wall"),
            )
            .model(Ent::circle([5.0, 5.0], 1.0).on("A-WALL").label("lamp"))
    };
    let a = base().build();
    let s = f.open("plan.dxf", &a.bytes);
    let alice = f.join(&s, "alice", ParticipantKind::Person, Rights::editor());
    let v1 = indexed(&s);
    let moved = k2_mpc_dxf::testgen::splice_move(&a.bytes, a.handle("wall"), 1.0);
    s.write_bytes(alice, &moved, s.status().unwrap().version)
        .unwrap();
    let v2 = indexed(&s);
    let keys = |v: &IndexView| v.nodes.iter().map(|n| n.key.clone()).collect::<Vec<_>>();
    assert_eq!(keys(&v1), keys(&v2));
    let node = |v: &IndexView, k: &str| v.nodes.iter().find(|n| n.key.as_str() == k).cloned();
    let (w1, w2) = (
        node(&v1, &a.addr("wall")).unwrap(),
        node(&v2, &a.addr("wall")).unwrap(),
    );
    assert_ne!(
        w1.attrs.get("geom"),
        w2.attrs.get("geom"),
        "the moved wall's geometry"
    );
    let (l1, l2) = (
        node(&v1, &a.addr("lamp")).unwrap(),
        node(&v2, &a.addr("lamp")).unwrap(),
    );
    assert_eq!(l1.record_hash, l2.record_hash, "the lamp didn't change");
}

#[test]
fn a_worker_result_commits_like_an_in_process_run() {
    let f = fx("wire");
    let b = testgen::nested_blocks(Ver::R2010).build();
    let s = f.open("blocks.dxf", &b.bytes);
    let mut job = s.index_job(None).unwrap().expect("a job");
    let (mut input, limits) = job.open().unwrap();
    let wire = index_to_wire(&DxfPack, &mut input, job.size(), &limits).unwrap();
    let st = job.commit_wire(input, &wire).unwrap();
    assert!(
        matches!(st, IndexStatus::Ready { version: 1, .. }),
        "{st:?}"
    );
    let v = s.index_view(None).unwrap();
    assert_eq!(v.source, "dxf-AC1024");
    let blocks = v.nodes.iter().filter(|n| n.kind == "block").count();
    assert_eq!(blocks, 4, "A, B, C and D");
}
