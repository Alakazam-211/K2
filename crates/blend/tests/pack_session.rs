//! The pack inside the kit: a blob session indexes a BLEND file through the kit's job (in
//! process and through the worker's wire form), diffs two versions by data-block, mints keys
//! where the pack leaves them out, and refuses a write on a stale version. And what a rename does: the
//! old key is gone, and an anchor on it is offered the renamed object as a candidate.

mod support;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::anchor::{resolve, Anchor, AnchorDoubt, AnchorState, CandidateWhy};
use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::index::{index_to_wire, IndexStatus, IndexView, KeyClass, NodeChangeKind};
use k2_mpc_base::session::{
    AuditEvent, BlobPack, BlobSession, Config, ConnId, Host, Participant, ParticipantKind, Rights,
    SessionError, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_blend::testgen::SceneBuilder;
use k2_mpc_blend::{BlendPack, HeaderForm};
use support::{spec, studio, Studio};

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
        "k2blend-session-{tag}-{}-{}",
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
            path: "scene.blend".into(),
        };
        self.mgr
            .open_blob(
                Arc::new(BlendPack::default()) as Arc<dyn BlobPack>,
                &key,
                Some(&mut &bytes[..]),
            )
            .unwrap()
    }
    fn join(&self, s: &BlobSession, pid: &str, kind: ParticipantKind) -> ConnId {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::editor().with_history(true));
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
fn the_kits_job_indexes_a_scene() {
    let f = fx("job");
    let s = f.open(&studio(&Studio::default()));
    let v = indexed(&s);
    assert_eq!(v.source, "blend-4.03");
    assert_eq!(v.nodes.len(), 16);
    assert!(
        v.nodes.iter().all(|n| n.key_class == KeyClass::Native),
        "every key is the file's"
    );
    let by = |k: &str| v.nodes.iter().find(|n| n.key.as_str() == k).unwrap();
    assert_eq!(by("@ob:Child").depth, 3);
    assert_eq!(
        by("@ob:Child").parent.as_ref().map(|k| k.as_str()),
        Some("@gr:Inner")
    );
    assert_eq!(
        by("@ob:Cube").attrs.get("loc").map(String::as_str),
        Some("0,0,0.25")
    );
    assert!(by("@ob:Cube").content_hash.is_some());
    assert_eq!(
        by("@ob:Cube")
            .fingerprint
            .weak
            .get("data")
            .map(String::as_str),
        Some("@me:Cube")
    );
}

#[test]
fn a_worker_result_commits_like_an_in_process_run() {
    let f = fx("wire");
    let s = f.open(&studio(&Studio::default()));
    let mut job = s.index_job(None).unwrap().expect("a job");
    let (mut input, limits) = job.open().unwrap();
    let wire = index_to_wire(&BlendPack::default(), &mut input, job.size(), &limits).unwrap();
    let st = job.commit_wire(input, &wire).unwrap();
    assert_eq!(
        st,
        IndexStatus::Ready {
            version: 1,
            nodes: 16
        }
    );
    let v = s.index_view(None).unwrap();
    assert_eq!(v.nodes[0].key.as_str(), "@sc:Scene");
}

#[test]
fn a_key_the_pack_leaves_out_is_minted() {
    let f = fx("mint");
    let mut b = SceneBuilder::new(HeaderForm::Large, 8, false, 501, false);
    b.scene("Scene", &[], &[], None);
    b.object_named(&[b' '; 255], &spec(None));
    let s = f.open(&b.bytes());
    let v = indexed(&s);
    let long = &v.nodes[1];
    assert!(long.key.as_str().starts_with("@k:"), "{}", long.key);
    assert!(matches!(long.key_class, KeyClass::Minted { .. }));
}

#[test]
fn another_save_diffs_to_nothing_and_a_move_to_one_object() {
    let f = fx("diff");
    let s = f.open(&studio(&Studio::default()));
    let alice = f.join(&s, "alice", ParticipantKind::Person);
    indexed(&s);
    // the same scene saved again: every address different
    let v = s.status().unwrap().version;
    s.write_bytes(
        alice,
        &studio(&Studio {
            shift: 0x10_0000,
            ..Studio::default()
        }),
        v,
    )
    .unwrap();
    indexed(&s);
    let v2 = s.status().unwrap().version;
    let d = s.index_diff(alice, v, v2).unwrap();
    assert!(d.changes.is_empty(), "{:?}", d.changes);
    // the cube moved
    s.write_bytes(
        alice,
        &studio(&Studio {
            shift: 0x20_0000,
            cube_loc: [1.0, 1.0, 1.0],
            ..Studio::default()
        }),
        v2,
    )
    .unwrap();
    indexed(&s);
    let v3 = s.status().unwrap().version;
    let d = s.index_diff(alice, v2, v3).unwrap();
    assert_eq!(d.changes.len(), 1, "{:?}", d.changes);
    let c = &d.changes[0];
    assert_eq!(c.key.as_str(), "@ob:Cube");
    let kinds: BTreeSet<NodeChangeKind> = [NodeChangeKind::Props, NodeChangeKind::Content].into();
    assert_eq!(c.changes, kinds);
    assert!(c.props.iter().any(|p| p == "attrs.loc"), "{:?}", c.props);
}

#[test]
fn a_rename_is_a_new_key_and_its_anchor_is_offered_the_renamed_object() {
    let f = fx("rename");
    let s = f.open(&studio(&Studio::default()));
    let alice = f.join(&s, "alice", ParticipantKind::Person);
    let v1 = indexed(&s);
    let cube = v1
        .nodes
        .iter()
        .find(|n| n.key.as_str() == "@ob:Cube")
        .unwrap();
    let anchor = Anchor::on_node(cube, 1, None);
    s.write_bytes(
        alice,
        &studio(&Studio {
            cube: "Hero".into(),
            ..Studio::default()
        }),
        1,
    )
    .unwrap();
    let v2 = indexed(&s);
    let d = s.index_diff(alice, 1, 2).unwrap();
    let changed: Vec<(&str, Vec<NodeChangeKind>)> = d
        .changes
        .iter()
        .map(|c| (c.key.as_str(), c.changes.iter().copied().collect()))
        .collect();
    assert!(
        changed.contains(&("@ob:Hero", vec![NodeChangeKind::Added])),
        "{changed:?}"
    );
    assert!(
        changed.contains(&("@ob:Cube", vec![NodeChangeKind::Removed])),
        "{changed:?}"
    );
    // the anchor: the key is gone; the object that kept the cube's data is a candidate, never
    // taken
    let r = resolve(&anchor, 2, &v2.nodes, &BlendPack::default());
    assert_eq!(r.state, AnchorState::Suspect(AnchorDoubt::CandidateOnly));
    assert_eq!(r.node, None);
    assert_eq!(r.candidates.len(), 1, "{:?}", r.candidates);
    assert_eq!(r.candidates[0].key.as_str(), "@ob:Hero");
    assert_eq!(r.candidates[0].why, CandidateWhy::FingerprintMatch);
    // an anchor on an object that is simply gone is orphaned
    let child = v1
        .nodes
        .iter()
        .find(|n| n.key.as_str() == "@ob:Light")
        .unwrap();
    let a = Anchor::on_node(child, 1, None);
    let mut gone = v2.nodes.clone();
    gone.retain(|n| n.key.as_str() != "@ob:Light");
    assert_eq!(
        resolve(&a, 2, &gone, &BlendPack::default()).state,
        AnchorState::Orphaned
    );
    // the unrenamed objects are still attached
    let cam = v1
        .nodes
        .iter()
        .find(|n| n.key.as_str() == "@ob:Camera")
        .unwrap();
    let a = Anchor::on_node(cam, 1, None);
    assert_eq!(
        resolve(&a, 2, &v2.nodes, &BlendPack::default()).state,
        AnchorState::Attached
    );
}

#[test]
fn agents_and_people_write_on_the_current_version_only() {
    let f = fx("agent");
    let s = f.open(&studio(&Studio::default()));
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = s.status().unwrap().version;
    // agents write (through the verbs, `tests/verbs.rs`), never on a stale version
    let e = s
        .write_bytes(bot, &studio(&Studio::default()), v + 1)
        .unwrap_err();
    assert!(matches!(e, SessionError::Stale { .. }), "{e:?}");
    let alice = f.join(&s, "alice", ParticipantKind::Person);
    s.write_bytes(
        alice,
        &studio(&Studio {
            shift: 4096,
            ..Studio::default()
        }),
        v,
    )
    .unwrap();
}

#[test]
fn a_collection_disabled_in_viewports_hides_what_it_holds() {
    let mut bytes = studio(&Studio::default());
    // set the collection Inner's flag bit 1 (disabled in viewports)
    let b = SceneBuilder::modern();
    let s = &b.file.schema;
    let name_at = s.offset("Collection", "id") + s.offset("ID", "name");
    let at = bytes
        .windows(8)
        .position(|w| w == b"GRInner\0")
        .expect("the collection's name")
        - name_at
        + s.offset("Collection", "flag");
    bytes[at] = 1;
    let f = fx("hidden");
    let s = f.open(&bytes);
    let v = indexed(&s);
    let by = |k: &str| v.nodes.iter().find(|n| n.key.as_str() == k).unwrap();
    assert!(!by("@gr:Inner").visible);
    assert_eq!(
        by("@gr:Inner")
            .attrs
            .get("hide_viewport")
            .map(String::as_str),
        Some("true")
    );
    assert!(
        !by("@ob:Child").visible_in_tree,
        "what it holds is hidden through it"
    );
    assert!(by("@gr:Set").visible && by("@gr:Set").visible_in_tree);
    assert!(by("@ob:Cube").visible && by("@ob:Cube").visible_in_tree);
}
