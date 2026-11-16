//! The agent's verbs: the registry data, `scene list`, each edit as a pure function with its
//! refusals and exits, and each run through a blob session on the version the agent read:
//! refused, never last-writer-wins, when the doc has moved on.

mod support;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    AuditEvent, BlobPack, BlobSession, Config, ConnId, Host, Participant, ParticipantKind, Rights,
    SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::exit::{NO, NOT_FOUND, USAGE};
use k2_mpc_blend::verbs::{apply, list, registry, run, run_list, Verb, VerbError, ROWS, VERBS};
use k2_mpc_blend::{BlendPack, ReadLimits, WriteError};
use support::{studio, Studio};

fn limits() -> ReadLimits {
    ReadLimits::default()
}

fn place(object: &str, loc: Option<[f32; 3]>, rot: Option<[f32; 3]>) -> Verb {
    Verb::Place {
        object: object.into(),
        loc,
        rot,
        scale: None,
    }
}

#[test]
fn the_verbs_register_and_serve_their_rows() {
    let r = registry().expect("check: the registry takes the verbs");
    for v in VERBS {
        assert!(r.get(v.name).is_some(), "{}", v.name);
        assert!(v.serves.iter().all(|s| ROWS.contains(s)), "{}", v.name);
    }
    for row in ROWS {
        assert!(
            VERBS.iter().any(|v| v.serves.contains(row)),
            "{row} is served"
        );
    }
}

#[test]
fn scene_list_shows_collections_and_objects() {
    let l = list(&studio(&Studio::default()), &limits()).unwrap();
    let cols: Vec<&str> = l.collections.iter().map(|c| c.address.as_str()).collect();
    assert_eq!(cols, ["@gr:Set", "@gr:Inner", "@gr:Props"]);
    let set = &l.collections[0];
    assert_eq!(set.holders, ["@sc:Scene"]);
    assert_eq!(set.objects, ["@ob:Cube", "@ob:Light"]);
    assert_eq!(set.hidden, [Some(false); 3]);
    let objs: Vec<&str> = l.objects.iter().map(|o| o.address.as_str()).collect();
    assert_eq!(objs, ["@ob:Camera", "@ob:Cube", "@ob:Child", "@ob:Light"]);
    let cube = &l.objects[1];
    assert_eq!(cube.collections, ["@gr:Set", "@gr:Props"]);
    assert_eq!(cube.data, Some(("@me:Cube".into(), "mesh".into())));
    assert_eq!(cube.location.as_deref(), Some("0,0,0.25"));
    assert_eq!(cube.rotation_mode.as_deref(), Some("quaternion"));
    let child = &l.objects[2];
    assert_eq!(child.parent.as_deref(), Some("@ob:Cube"));
    assert_eq!(child.hidden, [Some(true), Some(false), Some(true)]);
    // three collections, four objects, and the camera
    let lines = l.lines();
    assert_eq!(lines.len(), 8);
    assert!(lines[5].contains("off: viewport,render"), "{}", lines[5]);
}

#[test]
fn scene_list_names_each_camera() {
    if let Err(e) = support::row_cameras_named() {
        panic!("{e}");
    }
}

/// The file verbs and the merge, under the pack's default limits, refuse a small file that
/// decodes past the live tier's raw cap (`support::row_whole_file_default_bounded`).
#[test]
fn whole_file_paths_default_to_a_bounded_raw_cap() {
    if let Err(e) = support::row_whole_file_default_bounded() {
        panic!("{e}");
    }
}

#[test]
fn each_edit_verb_writes_its_fields() {
    let b = studio(&Studio::default());
    let w = apply(
        &b,
        &Verb::Place {
            object: "@ob:Cube".into(),
            loc: Some([1.0, 2.0, 3.0]),
            rot: None,
            scale: Some([2.0; 3]),
        },
        &limits(),
    )
    .unwrap();
    assert_eq!(w.patched.len(), 2);
    let l = list(&w.bytes, &limits()).unwrap();
    assert_eq!(l.objects[1].location.as_deref(), Some("1,2,3"));
    assert_eq!(l.objects[1].scale.as_deref(), Some("2,2,2"));
    let w = apply(
        &b,
        &Verb::Visibility {
            object: "@ob:Light".into(),
            viewport: Some(true),
            select: None,
            render: Some(true),
        },
        &limits(),
    )
    .unwrap();
    let l = list(&w.bytes, &limits()).unwrap();
    assert_eq!(l.objects[3].hidden, [Some(true), Some(false), Some(true)]);
    let w = apply(
        &b,
        &Verb::Rename {
            object: "@ob:Light".into(),
            name: "Key Light".into(),
        },
        &limits(),
    )
    .unwrap();
    let l = list(&w.bytes, &limits()).unwrap();
    assert_eq!(l.objects[3].address, "@ob:Key%20Light");
    assert_eq!(l.collections[0].objects, ["@ob:Cube", "@ob:Key%20Light"]);
}

#[test]
fn each_refusal_has_its_exit() {
    let b = studio(&Studio::default());
    let refuse = |v: Verb| apply(&b, &v, &limits()).unwrap_err();
    let e = refuse(place("@ob:Cube", None, None));
    assert!(matches!(e, VerbError::Usage(_)));
    assert_eq!(e.exit(), USAGE);
    let e = refuse(place("@ob:Nobody", Some([0.0; 3]), None));
    assert!(matches!(e, VerbError::Write(WriteError::NotFound(_))));
    assert_eq!(e.exit(), NOT_FOUND);
    // the studio's objects rotate by quaternion: an Euler rotation is refused
    let e = refuse(place("@ob:Cube", None, Some([0.1, 0.0, 0.0])));
    assert!(matches!(
        e,
        VerbError::Write(WriteError::RotationMode { .. })
    ));
    assert_eq!(e.exit(), USAGE);
    let e = refuse(Verb::Rename {
        object: "@ob:Cube".into(),
        name: "Light".into(),
    });
    assert!(matches!(e, VerbError::Write(WriteError::Name { .. })));
    let e = refuse(place("@li:lib/ma:Shared", Some([0.0; 3]), None));
    assert!(matches!(e, VerbError::Write(WriteError::Linked(_))));
}

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

fn fx() -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2blend-verbs-{}-{}",
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
            .insert(pid.into(), Rights::editor());
        s.join(
            Participant::new(pid, pid, "#123456", kind, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
}

#[test]
fn an_agent_runs_verbs_on_the_version_it_read() {
    let f = fx();
    let s = f.open(&studio(&Studio::default()));
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let (v1, l) = run_list(&s, bot, &limits()).unwrap();
    assert_eq!(l.objects.len(), 4);
    let r = run(
        &s,
        bot,
        v1,
        &place("@ob:Cube", Some([5.0, 0.0, 0.0]), None),
        &limits(),
    )
    .expect("check: the verb runs");
    assert!(r.version > v1);
    let (v2, l) = run_list(&s, bot, &limits()).unwrap();
    assert_eq!(v2, r.version);
    assert_eq!(l.objects[1].location.as_deref(), Some("5,0,0"));
    // the version it read is gone: refused, nothing written
    let e = run(
        &s,
        bot,
        v1,
        &place("@ob:Cube", Some([6.0, 0.0, 0.0]), None),
        &limits(),
    )
    .unwrap_err();
    match e {
        VerbError::Stale { expected, actual } => assert_eq!((expected, actual), (v1, v2)),
        ref other => panic!("{other:?}"),
    }
    assert_eq!(e.exit(), NO);
    assert_eq!(
        run_list(&s, bot, &limits()).unwrap().0,
        v2,
        "nothing written"
    );
    // each verb, on the current version
    let r = run(
        &s,
        bot,
        v2,
        &Verb::Rename {
            object: "@ob:Cube".into(),
            name: "Hero".into(),
        },
        &limits(),
    )
    .unwrap();
    let r = run(
        &s,
        bot,
        r.version,
        &Verb::Visibility {
            object: "@ob:Hero".into(),
            viewport: Some(true),
            select: None,
            render: None,
        },
        &limits(),
    )
    .unwrap();
    let (v, l) = run_list(&s, bot, &limits()).unwrap();
    assert_eq!(v, r.version);
    let hero = l.objects.iter().find(|o| o.address == "@ob:Hero").unwrap();
    assert_eq!(hero.location.as_deref(), Some("5,0,0"));
    assert_eq!(hero.hidden[0], Some(true));
    // a refused verb writes nothing
    let e = run(
        &s,
        bot,
        v,
        &place("@ob:Cube", Some([1.0; 3]), None),
        &limits(),
    )
    .unwrap_err();
    assert_eq!(e.exit(), NOT_FOUND);
    assert_eq!(run_list(&s, bot, &limits()).unwrap().0, v);
}
