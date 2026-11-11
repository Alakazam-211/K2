//! The pack inside the kit: a blob session indexes a PSD through the kit's job, the kit mints
//! keys where the file's layer IDs are missing or repeated, and agents can't write the file.

mod support;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::index::{index_to_wire, IndexStatus, IndexView, KeyClass};
use k2_mpc_base::session::{
    AuditEvent, BlobSession, Config, ConnId, Host, Participant, ParticipantKind, Rights,
    SessionError, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_psd::PsdPack;
use support::*;

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
        "k2psd-session-{tag}-{}-{}",
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
    fn open(&self, bytes: &[u8]) -> Arc<BlobSession> {
        let key = DocKey {
            space: "ws".into(),
            path: "art.psd".into(),
        };
        self.mgr
            .open_blob(Arc::new(PsdPack), &key, Some(&mut &bytes[..]))
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

fn key_of<'a>(v: &'a IndexView, name: &str) -> &'a str {
    v.nodes
        .iter()
        .find(|n| n.name == name)
        .unwrap_or_else(|| panic!("no node {name}"))
        .key
        .as_str()
}

#[test]
fn native_ids_are_keys_and_missing_or_repeated_ones_are_minted() {
    let f = fx("keys");
    let s = f.open(&psd_file(
        false,
        &[
            Lay::new("A").id(5),
            Lay::new("B"),
            Lay::new("C").id(5),
            Lay::new("D").id(9),
        ],
        &[],
        &[],
    ));
    let v = indexed(&s);
    let by = |name: &str| v.nodes.iter().find(|n| n.name == name).unwrap();
    assert_eq!(by("D").key.as_str(), "@lyid:9");
    assert_eq!(by("D").key_class, KeyClass::Native);
    for dup in ["A", "C"] {
        let n = by(dup);
        assert!(n.key.as_str().starts_with("@k:"), "{dup}: {}", n.key);
        assert_eq!(n.key_class, KeyClass::Minted { weak: false });
        assert_eq!(
            n.native.as_ref().map(|k| k.as_str()),
            Some("@lyid:5"),
            "{dup}: the repeated ID stays readable"
        );
    }
    assert_ne!(by("A").key, by("C").key, "every holder gets its own key");
    assert!(by("B").key.as_str().starts_with("@k:"));
    assert_eq!(by("B").native, None);
}

#[test]
fn a_minted_key_is_carried_to_the_next_version_and_a_native_one_stays() {
    let f = fx("carry");
    let one = psd_file(
        false,
        &[
            Lay::end(),
            Lay::new("NoId").pixels(&[1]),
            Lay::folder("Group").id(3),
        ],
        &[],
        &[],
    );
    let s = f.open(&one);
    let c = f.join(&s, "alice", ParticipantKind::Person, Rights::editor());
    let v1 = indexed(&s);
    let minted = key_of(&v1, "NoId").to_owned();
    assert!(minted.starts_with("@k:"));
    assert_eq!(key_of(&v1, "Group"), "@lyid:3");

    // new pixels, a new layer on top: the unkeyed layer keeps its key
    let two = psd_file(
        false,
        &[
            Lay::end(),
            Lay::new("NoId").pixels(&[2]),
            Lay::folder("Group").id(3),
            Lay::new("Top").id(4),
        ],
        &[],
        &[],
    );
    s.write_bytes(c, &two, s.status().unwrap().version).unwrap();
    let v2 = indexed(&s);
    assert_eq!(
        key_of(&v2, "NoId"),
        minted,
        "carried by kind, name and parent"
    );
    assert_eq!(key_of(&v2, "Group"), "@lyid:3");
    assert_eq!(key_of(&v2, "Top"), "@lyid:4");
    let no_id = v2.nodes.iter().find(|n| n.name == "NoId").unwrap();
    let was = v1.nodes.iter().find(|n| n.name == "NoId").unwrap();
    assert_ne!(no_id.content_hash, was.content_hash, "its pixels changed");
    assert_eq!(no_id.record_hash, was.record_hash, "its record did not");
    assert_eq!(no_id.parent.as_ref().map(|k| k.as_str()), Some("@lyid:3"));
    assert_eq!(no_id.depth, 1);
}

#[test]
fn visibility_runs_through_the_tree() {
    let f = fx("vis");
    let s = f.open(&psd_file(
        false,
        &[
            Lay::end(),
            Lay::new("Child").id(2),
            Lay::folder("Hidden").id(1).hidden(),
        ],
        &[],
        &[],
    ));
    let v = indexed(&s);
    let child = v.nodes.iter().find(|n| n.name == "Child").unwrap();
    assert!(child.visible, "its own flag");
    assert!(!child.visible_in_tree, "its group is hidden");
}

#[test]
fn an_agent_cannot_write_a_psd_and_a_person_can() {
    let f = fx("agent");
    let s = f.open(&psd_file(false, &[Lay::new("a").id(1)], &[], &[]));
    let bot = f.join(&s, "bot", ParticipantKind::Agent, Rights::editor());
    let v = s.status().unwrap().version;
    let e = s.write_bytes(bot, b"agent bytes", v).unwrap_err();
    assert_eq!(e, SessionError::AgentWriteRefused);
    assert_eq!(e.report().id(), "MPC-E715");
    let alice = f.join(&s, "alice", ParticipantKind::Person, Rights::editor());
    let two = psd_file(false, &[Lay::new("b").id(1)], &[], &[]);
    s.write_bytes(alice, &two, v).unwrap();
}

#[test]
fn a_worker_result_commits_like_an_in_process_run() {
    let f = fx("wire");
    let bytes = psd_file(
        false,
        &[Lay::end(), Lay::new("a").id(1), Lay::folder("g").id(2)],
        &[],
        &[],
    );
    let s = f.open(&bytes);
    let mut job = s.index_job(None).unwrap().expect("a job");
    let (mut input, limits) = job.open().unwrap();
    let wire = index_to_wire(&PsdPack, &mut input, job.size(), &limits).unwrap();
    let st = job.commit_wire(input, &wire).unwrap();
    assert_eq!(
        st,
        IndexStatus::Ready {
            version: 1,
            nodes: 2
        }
    );
    let v = s.index_view(None).unwrap();
    assert_eq!(v.source, "psd-1");
    assert_eq!(key_of(&v, "g"), "@lyid:2");
    assert_eq!(key_of(&v, "a"), "@lyid:1");
}
