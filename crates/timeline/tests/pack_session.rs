//! The packs inside the kit: a blob session indexes a timeline file through the kit's job (in
//! process and through the worker's wire form), the kit mints keys where the pack leaves them
//! out and carries them to the next version, and agents can't write the file.

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
use k2_mpc_timeline::pack::{FcpxmlPack, OtioPack, PrprojPack};

const OTIO: &str = include_str!("data/seed.otio");
const FCPXML: &str = include_str!("data/seed.fcpxml");
const PRPROJ: &str = include_str!("data/seed-prproj.xml");

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
        "k2timeline-session-{tag}-{}-{}",
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
fn each_pack_indexes_through_the_kits_job() {
    let f = fx("each");
    for (pack, path, bytes, first) in [
        (
            Arc::new(OtioPack) as Arc<dyn BlobPack>,
            "cut.otio",
            OTIO,
            "@d-seq:1",
        ),
        (Arc::new(FcpxmlPack), "cut.fcpxml", FCPXML, "@project:P1"),
        (Arc::new(PrprojPack), "cut.prproj", PRPROJ, "@seq:S1"),
    ] {
        let s = f.open(pack, path, bytes.as_bytes());
        let v = indexed(&s);
        assert_eq!(v.nodes[0].key.as_str(), first, "{path}");
        assert_eq!(v.nodes[0].key_class, KeyClass::Native, "{path}");
        assert_eq!(v.nodes[0].kind, "sequence");
        for n in &v.nodes {
            assert_eq!(
                n.parent.is_none(),
                n.kind == "sequence",
                "{path}: sequences, and only they, are roots ({})",
                n.key
            );
        }
    }
}

// the same media and in-point as clip "a", so the same derived key; another name, so the kit
// can align it by name across versions
const SECOND_CLIP: &str = r#"{"OTIO_SCHEMA":"Clip.1","name":"a2","media_reference":{"OTIO_SCHEMA":"ExternalReference.1","target_url":"a.mov"},
"source_range":{"OTIO_SCHEMA":"TimeRange.1","start_time":{"OTIO_SCHEMA":"RationalTime.1","value":2,"rate":24},"duration":{"OTIO_SCHEMA":"RationalTime.1","value":9,"rate":24}}},
{"OTIO_SCHEMA":"Transition.1""#;

#[test]
fn the_kit_mints_a_key_for_a_repeat_and_carries_it() {
    let f = fx("mint");
    let one = OTIO.replacen(r#"{"OTIO_SCHEMA":"Transition.1""#, SECOND_CLIP, 1);
    let s = f.open(Arc::new(OtioPack), "cut.otio", one.as_bytes());
    let alice = f.join(&s, "alice", ParticipantKind::Person, Rights::editor());
    let v1 = indexed(&s);
    let clips: Vec<&str> = v1
        .nodes
        .iter()
        .filter(|n| n.kind == "clip")
        .map(|n| n.key.as_str())
        .collect();
    assert_eq!(clips[0], "@d-seq:1/d-trk:V1/d-clip:a.mov@1_12s");
    assert!(clips[1].starts_with("@k:"), "{}", clips[1]);
    let minted = clips[1].to_owned();

    // the next version lengthens the gap before them: every clip moves, nothing on their
    // path (kind, name, parents) changes, so the minted key is carried
    let two = one.replace(r#""value":5,"rate":24"#, r#""value":7,"rate":24"#);
    assert_ne!(one, two);
    s.write_bytes(alice, two.as_bytes(), s.status().unwrap().version)
        .unwrap();
    let v2 = indexed(&s);
    let clips2: Vec<(&str, &str)> = v2
        .nodes
        .iter()
        .filter(|n| n.kind == "clip")
        .map(|n| (n.key.as_str(), n.attrs["start"].as_str()))
        .collect();
    assert_eq!(
        clips2,
        vec![(clips[0], "7/24s"), (minted.as_str(), "2/3s")],
        "moved, and keyed as before"
    );
    let carried = v2.nodes.iter().find(|n| n.key.as_str() == minted).unwrap();
    assert_eq!(carried.key_class, KeyClass::Minted { weak: false });

    // renaming the sequence changes the path under it: the kit mints anew (a tie or a changed
    // path never pairs)
    let three = two.replace(r#""name":"t""#, r#""name":"renamed""#);
    s.write_bytes(alice, three.as_bytes(), s.status().unwrap().version)
        .unwrap();
    let v3 = indexed(&s);
    let clips3: Vec<&str> = v3
        .nodes
        .iter()
        .filter(|n| n.kind == "clip")
        .map(|n| n.key.as_str())
        .collect();
    assert_eq!(
        clips3[0], clips[0],
        "a derived key doesn't depend on the path"
    );
    assert_ne!(clips3[1], minted);
}

#[test]
fn an_agent_cannot_write_the_file_and_a_person_can() {
    let f = fx("agent");
    let s = f.open(Arc::new(OtioPack), "cut.otio", OTIO.as_bytes());
    let bot = f.join(&s, "bot", ParticipantKind::Agent, Rights::editor());
    let v = s.status().unwrap().version;
    let e = s.write_bytes(bot, b"agent bytes", v).unwrap_err();
    assert_eq!(e, SessionError::AgentWriteRefused);
    assert_eq!(e.report().id(), "MPC-E715");
    let alice = f.join(&s, "alice", ParticipantKind::Person, Rights::editor());
    s.write_bytes(alice, OTIO.replace("\"t\"", "\"u\"").as_bytes(), v)
        .unwrap();
}

#[test]
fn a_worker_result_commits_like_an_in_process_run() {
    let f = fx("wire");
    let s = f.open(Arc::new(PrprojPack), "cut.prproj", PRPROJ.as_bytes());
    let mut job = s.index_job(None).unwrap().expect("a job");
    let (mut input, limits) = job.open().unwrap();
    let wire = index_to_wire(&PrprojPack, &mut input, job.size(), &limits).unwrap();
    let st = job.commit_wire(input, &wire).unwrap();
    assert_eq!(
        st,
        IndexStatus::Ready {
            version: 1,
            nodes: 5
        }
    );
    let v = s.index_view(None).unwrap();
    assert_eq!(v.source, "prproj-3");
    let clip = v.nodes.iter().find(|n| n.kind == "clip").unwrap();
    assert_eq!(clip.key.as_str(), "@clip:C1");
    assert_eq!(clip.parent.as_ref().map(|k| k.as_str()), Some("@track:T1"));
}
