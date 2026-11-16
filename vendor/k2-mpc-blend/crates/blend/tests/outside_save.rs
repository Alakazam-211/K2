//! A person's app save landing on the kit's versions, in a blob session that keeps a real file
//! in sync: nothing a person or an agent changed may be lost, silently or across a restart.
//!
//! - **A clash** (the person and the agent moved the same object) keeps both: the save is the
//!   file, the agent's version a conflict copy recorded in the store, so a restart still has it.
//! - **A stale save after a kit write:** the kit wrote an agent's change to the file, the next
//!   agent change waits to be written, and the app, still holding the version it opened before
//!   the kit's write, saves. Merged against what the file held, the save
//!   reads as the person undoing the kit's write; it must keep both instead.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    AuditEvent, BlobPack, BlobSession, Config, ConnId, Host, Participant, ParticipantKind, Rights,
    SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_blend::testgen::{ObjectSpec, SceneBuilder};
use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{read_blend, read_scene, write, BlendPack, Change, Edit, ReadLimits};

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

/// A folder with the store, the blobs and the synced file; removed at the end.
struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dir(tag: &str) -> Dir {
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "k2blend-outside-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("files")).unwrap();
    Dir(d)
}

/// The kit as a host runs it, on the folder `d`: a fresh one each call is a restart.
struct Kit {
    host: Arc<TestHost>,
    /// Kept for the session's life.
    _mgr: SessionManager,
    s: Arc<BlobSession>,
}

fn file(d: &Dir) -> PathBuf {
    d.0.join("files").join("scene.blend")
}

fn kit(d: &Dir) -> Kit {
    let host = Arc::new(TestHost {
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(d.0.join("docs.redb")).unwrap());
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            d.0.join("blobs"),
            BlobConfig::default(),
            Arc::new(|| 1_000_000),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host.clone(), c).with_blob_store(blobs);
    let key = DocKey {
        space: "ws".into(),
        path: "scene.blend".into(),
    };
    let s = mgr
        .open_blob_file(
            Arc::new(BlendPack::default()) as Arc<dyn BlobPack>,
            &key,
            &file(d),
            &d.0.join("files"),
        )
        .unwrap();
    Kit { host, _mgr: mgr, s }
}

impl Kit {
    fn join(&self, pid: &str, kind: ParticipantKind) -> ConnId {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::editor().with_history(true));
        self.s
            .join(
                Participant::new(pid, pid, "#123456", kind, "tab"),
                Stay::Held,
            )
            .unwrap()
            .conn
    }

    /// The agent's write of `edits` onto the current version, as the verbs make it.
    fn agent_writes(&self, bot: ConnId, edits: &[Edit]) {
        let mut r = self.s.read(bot).unwrap();
        let mut now = Vec::new();
        r.reader.read_to_end(&mut now).unwrap();
        let bytes = write(&now, edits, &ReadLimits::default())
            .expect("check: the agent's write")
            .bytes;
        self.s.write_bytes(bot, &bytes, r.version).unwrap();
    }
}

/// A (mesh data A) and B (mesh data B); `shift` moves everything in memory, as an app's save
/// does.
#[derive(Clone, Copy)]
struct Scene {
    shift: u64,
    a_loc: [f32; 3],
    b_scale: [f32; 3],
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            shift: 0,
            a_loc: [0.0; 3],
            b_scale: [1.0; 3],
        }
    }
}

fn scene(s: Scene) -> Vec<u8> {
    let mut b = SceneBuilder::modern();
    b.shift = s.shift;
    b.scene("Scene", &["A", "B"], &[], None);
    b.object(
        "A",
        &ObjectSpec {
            data: Some("ME:A".into()),
            loc: s.a_loc,
            rotmode: 1,
            ..ObjectSpec::default()
        },
    );
    b.object(
        "B",
        &ObjectSpec {
            data: Some("ME:B".into()),
            scale: s.b_scale,
            rotmode: 1,
            ..ObjectSpec::default()
        },
    );
    b.mesh("A", &[], 3);
    b.mesh("B", &[], 3);
    b.bytes()
}

/// The person's app saves `bytes`: a new file renamed over the old, as apps save.
fn app_saves(path: &Path, bytes: &[u8]) {
    let tmp = path.with_extension("blend~");
    std::fs::write(&tmp, bytes).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

/// The field `name` of the object `key` in `bytes`.
fn value(bytes: &[u8], key: &str, name: &str) -> String {
    let l = ReadLimits::default();
    let (_, raw) = decompress(bytes, &l).unwrap();
    let m = read_scene(&read_blend(&mut &raw[..], &l).unwrap()).unwrap();
    let id = m.by_key(key).unwrap_or_else(|| panic!("no {key}"));
    id.fields
        .iter()
        .chain(&id.named)
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| panic!("{key} has no {name}"))
}

/// What the file holds, then each recorded conflict copy that exists.
fn kept(k: &Kit, d: &Dir, conn: ConnId) -> (Vec<u8>, Vec<Vec<u8>>) {
    let copies =
        k.s.conflict_copies(conn)
            .unwrap()
            .into_iter()
            .map(|p| {
                let p = if p.is_absolute() {
                    p
                } else {
                    d.0.join("files").join(p)
                };
                std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
            })
            .collect();
    (std::fs::read(file(d)).unwrap(), copies)
}

#[test]
fn a_clash_keeps_both_and_a_restart_loses_nothing() {
    let d = dir("clash");
    std::fs::write(file(&d), scene(Scene::default())).unwrap();
    {
        let k = kit(&d);
        let bot = k.join("bot", ParticipantKind::Agent);
        // the person moves A in the app and saves; the agent moves A too, before the kit has
        // looked at the file
        app_saves(
            &file(&d),
            &scene(Scene {
                shift: 0x55_0000,
                a_loc: [9.0; 3],
                ..Scene::default()
            }),
        );
        k.agent_writes(bot, &[Edit::new("@ob:A", Change::Location([1.0; 3]))]);
        drop(k);
    }
    // a restart: a new kit on the same store and file, a new pack
    let k = kit(&d);
    let alice = k.join("alice", ParticipantKind::Person);
    let (now, copies) = kept(&k, &d, alice);
    assert_eq!(
        value(&now, "@ob:A", "loc"),
        "9,9,9",
        "the person's save is the file"
    );
    assert!(
        copies.iter().any(|c| value(c, "@ob:A", "loc") == "1,1,1"),
        "after a restart the agent's move is nowhere but history: {} copies",
        copies.len()
    );
}

#[test]
fn a_stale_save_after_a_kit_write_keeps_both() {
    let d = dir("stale");
    let opened = scene(Scene::default());
    std::fs::write(file(&d), &opened).unwrap();
    let k = kit(&d);
    let bot = k.join("bot", ParticipantKind::Agent);
    let alice = k.join("alice", ParticipantKind::Person);
    // the kit writes the agent's move of A to the file
    k.agent_writes(bot, &[Edit::new("@ob:A", Change::Location([1.0; 3]))]);
    assert_eq!(
        value(&std::fs::read(file(&d)).unwrap(), "@ob:A", "loc"),
        "1,1,1",
        "check: the kit wrote the file"
    );
    // the app, still on the version it opened (A at 0), scales B and saves; the agent moves B
    // before the kit has looked at the file, so its change waits to be written
    app_saves(
        &file(&d),
        &scene(Scene {
            shift: 0x55_0000,
            b_scale: [2.0; 3],
            ..Scene::default()
        }),
    );
    k.agent_writes(bot, &[Edit::new("@ob:B", Change::Location([5.0; 3]))]);
    let (now, copies) = kept(&k, &d, alice);
    let all: Vec<&[u8]> = std::iter::once(&now[..])
        .chain(copies.iter().map(Vec::as_slice))
        .collect();
    assert_eq!(value(&now, "@ob:B", "size"), "2,2,2", "the person's change");
    assert!(
        all.iter().any(|b| value(b, "@ob:B", "loc") == "5,5,5"),
        "the agent's second change is lost"
    );
    assert!(
        all.iter().any(|b| value(b, "@ob:A", "loc") == "1,1,1"),
        "the kit's earlier write is lost: the file has A at {}, {} conflict copies",
        value(&now, "@ob:A", "loc"),
        copies.len()
    );
}
