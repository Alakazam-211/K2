//! A doc's blob references through the public API, on a table pack whose cells can name a blob
//! (`Pack::blob_ref_roots`): an import, and an outside save of the file, are never refused for a
//! name with no blob behind it, and a read says so (`Session::blob_refs`).

use std::collections::BTreeMap;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

mod grid_pack;

use k2_mpc_base::blob::{BlobConfig, BlobId, BlobRefState, BlobStore};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{MapOp, Value};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, Config, Declined, DiskCheck, EditOptions, Host, Pack, PackContent, PackError,
    Participant, ParticipantKind, Rights, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::table::CELLS;

struct Lite;
impl Pack for Lite {
    fn schema(&self) -> Schema {
        grid_pack::GridLite.schema()
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        grid_pack::GridLite.decode(b)
    }
    fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        grid_pack::GridLite.encode(c, codec)
    }
    fn blob_ref_roots(&self) -> &'static [&'static str] {
        &[CELLS]
    }
}

struct H(AtomicU64);
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

#[test]
fn an_import_and_an_outside_save_name_a_missing_blob_and_a_read_says_so() {
    let dir = std::env::temp_dir().join(format!("k2mb-blob-refs-it-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(|| 1_000),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(
        store.clone(),
        Arc::new(H(AtomicU64::new(1_000))),
        Config::default(),
    )
    .with_blob_store(blobs.clone());

    let there = blobs.put_bytes("ws", b"a stored image").unwrap().id;
    let (gone1, gone2) = (
        BlobId::from_slice(&[1; 32]).unwrap(),
        BlobId::from_slice(&[2; 32]).unwrap(),
    );
    let file = ws.join("t.csv");
    fs::write(
        &file,
        format!("name,image\na,{}\nb,{}\n", there.to_ref(), gone1.to_ref()),
    )
    .unwrap();
    let key = DocKey {
        space: "ws".into(),
        path: "t.csv".into(),
    };
    let s = mgr.open_file(Arc::new(Lite), &key, &file, &ws).unwrap();
    let c = s
        .join(
            Participant::new("p", "P", "#111111", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let refs = s.blob_refs(c).unwrap();
    assert_eq!(refs.missing, 1, "{refs:?}");
    let state = |b: &BlobId| refs.refs.iter().find(|r| r.blob == *b).map(|r| r.state);
    assert_eq!(state(&there), Some(BlobRefState::Present));
    assert_eq!(state(&gone1), Some(BlobRefState::Missing));
    assert_eq!(blobs.info("ws", &there).unwrap().map(|x| x.1), Some(1));

    fs::write(
        &file,
        format!("name,image\na,{}\nb,{}\n", there.to_ref(), gone2.to_ref()),
    )
    .unwrap();
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    let refs = s.blob_refs(c).unwrap();
    assert_eq!(
        refs.missing, 2,
        "the record still names the first: {refs:?}"
    );
    assert!(refs.refs.iter().any(|r| r.blob == gone2));
    mgr.release().unwrap();
    let _ = fs::remove_dir_all(&dir);
}

/// An outside save comes after a connection's edit naming a held blob, and names one that's gone:
/// it's never refused, and a read says so.
#[test]
fn an_outside_save_after_a_strict_edit_is_not_refused() {
    let dir = std::env::temp_dir().join(format!("k2mb-blob-refs-save-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(|| 1_000),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(
        store.clone(),
        Arc::new(H(AtomicU64::new(1_000))),
        Config::default(),
    )
    .with_blob_store(blobs.clone());
    let held = blobs.put_bytes("ws", b"a stored image").unwrap().id;
    let gone = BlobId::from_slice(&[3; 32]).unwrap();
    let file = ws.join("t.csv");
    fs::write(&file, format!("name,image\na,{}\n", held.to_ref())).unwrap();
    let key = DocKey {
        space: "ws".into(),
        path: "t.csv".into(),
    };
    let s = mgr.open_file(Arc::new(Lite), &key, &file, &ws).unwrap();
    let c = s
        .join(
            Participant::new("p", "P", "#111111", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    // the same value again, by key: the edit names the blob and the file's bytes don't move
    let value = held.to_ref();
    s.edit_ops_with(c, &EditOptions::new(), |view| {
        let t = view.table().map_err(|e| e.to_string())?;
        // the first record is the header, so the one row left holds the blob
        let (row, col) = (t.live_rows()[0].clone(), t.live_cols()[1].clone());
        Ok::<_, String>(vec![Edit::Map {
            root: CELLS.into(),
            ops: vec![MapOp::Set {
                key: k2_mpc_base::table::cell_key(&row, &col),
                value: Value::Str(value.clone()),
            }],
        }])
    })
    .unwrap();
    fs::write(&file, format!("name,image\na,{}\n", gone.to_ref())).unwrap();
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    let refs = s.blob_refs(c).unwrap();
    assert!(
        refs.refs
            .iter()
            .any(|r| r.blob == gone && r.state == BlobRefState::Missing),
        "{refs:?}"
    );
    mgr.release().unwrap();
    let _ = fs::remove_dir_all(&dir);
}

// ---- tools/n1 `node-refs`: the two ends of the previous release's run ----

const HOUR: u64 = 60 * 60_000;

/// The doc tools/n1/consumer-refs opens, with its map declared.
struct Items;
impl Pack for Items {
    fn schema(&self) -> Schema {
        Schema {
            name: "items",
            roots: &[
                ("items", Kind::MovableList),
                ("props", Kind::Map),
                ("notes", Kind::Map),
            ],
        }
    }
    fn decode(&self, _: &[u8]) -> Result<PackContent, Declined> {
        Ok(PackContent {
            roots: vec![
                ("items".to_owned(), Neutral::List(Vec::new())),
                ("props".to_owned(), Neutral::Map(BTreeMap::new())),
                ("notes".to_owned(), Neutral::Map(BTreeMap::new())),
            ],
            codec: Vec::new(),
        })
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
    fn blob_ref_roots(&self) -> &'static [&'static str] {
        &["props"]
    }
}

struct Clock(Arc<AtomicU64>);
impl Host for Clock {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn n1_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT"))
}

fn n1_key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

fn n1_open(
    dir: &std::path::Path,
    now: u64,
) -> (
    Arc<DocStore>,
    Arc<BlobStore>,
    SessionManager,
    Arc<AtomicU64>,
) {
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let now = Arc::new(AtomicU64::new(now));
    let clock = now.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || clock.load(Ordering::SeqCst)),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(
        store.clone(),
        Arc::new(Clock(now.clone())),
        Config::default(),
    )
    .with_blob_store(blobs.clone());
    (store, blobs, mgr, now)
}

/// A store whose docs hold references, for the previous release (tools/n1/consumer-refs).
#[test]
#[ignore = "tools/n1: writes the store for the previous release's consumer"]
fn n1_refs_producer() {
    let dir = n1_dir();
    let (_store, blobs, mgr, _) = n1_open(&dir, 1_000_000);
    let mut lines = String::new();
    for (path, what, bytes) in [
        ("a.items", "held", &b"held by a.items"[..]),
        ("b.items", "deleted", &b"held by b.items"[..]),
    ] {
        let s = mgr.open(Arc::new(Items), &n1_key(path), Some(b"")).unwrap();
        let c = s
            .join(
                Participant::new("p", "P", "#111111", ParticipantKind::Person, "cli"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        let id = blobs.put_bytes("ws", bytes).unwrap().id;
        s.edit_ops(
            c,
            vec![Edit::Map {
                root: "props".into(),
                ops: vec![MapOp::Set {
                    key: what.to_owned(),
                    value: Value::Str(id.to_ref()),
                }],
            }],
            &EditOptions::new(),
        )
        .unwrap();
        assert_eq!(blobs.info("ws", &id).unwrap().map(|x| x.1), Some(1));
        lines.push_str(&format!("{what} {}\n", id.to_hex()));
    }
    let spare = blobs.put_bytes("ws", b"named by nothing").unwrap().id;
    lines.push_str(&format!("spare {}\n", spare.to_hex()));
    fs::write(dir.join("ids.txt"), lines).unwrap();
    mgr.release().unwrap();
}

/// After the previous release's run: this build's open gives back what its delete left, and a
/// load takes a hold on the name it wrote that's still there, and reports the one it collected.
#[test]
#[ignore = "tools/n1: reads the store the previous release's consumer left"]
fn n1_refs_after() {
    let dir = n1_dir();
    let ids = fs::read_to_string(dir.join("ids.txt")).unwrap();
    let id = |what: &str| {
        ids.lines()
            .find_map(|l| l.strip_prefix(what)?.strip_prefix(' '))
            .and_then(BlobId::from_hex)
            .unwrap_or_else(|| panic!("ids.txt has no {what}"))
    };
    let (held, deleted, limit, heal) = (id("held"), id("deleted"), id("limit"), id("heal"));
    let (_store, blobs, mgr, now) = n1_open(&dir, 1_000_000 + 5 * HOUR);
    let refs_on = |b: &BlobId| blobs.info("ws", b).unwrap().map(|x| x.1);
    assert_eq!(
        refs_on(&deleted),
        Some(0),
        "the open gave back the deleted doc's count"
    );
    println!("the open gave back the count the older delete left");
    assert_eq!(refs_on(&heal), Some(0), "the older write took none");

    let s = mgr.open(Arc::new(Items), &n1_key("a.items"), None).unwrap();
    let c = s
        .join(
            Participant::new("p", "P", "#111111", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    assert_eq!(
        refs_on(&heal),
        Some(1),
        "the load took a hold on what's still there"
    );
    assert_eq!(refs_on(&held), Some(1), "one count, as before");
    let refs = s.blob_refs(c).unwrap();
    let state = |b: &BlobId| refs.refs.iter().find(|r| r.blob == *b).map(|r| r.state);
    assert_eq!(state(&held), Some(BlobRefState::Present));
    assert_eq!(state(&heal), Some(BlobRefState::Present));
    assert_eq!(state(&limit), Some(BlobRefState::Missing), "{refs:?}");
    assert_eq!(refs.missing, 1, "{refs:?}");
    println!("the load took a hold on the name still there; the collected one reads as missing");

    now.fetch_add(2 * HOUR, Ordering::SeqCst);
    blobs.gc().unwrap();
    assert!(blobs.read_to_vec("ws", &held, 1024).is_ok());
    assert!(blobs.read_to_vec("ws", &heal, 1024).is_ok());
    assert_eq!(
        blobs.info("ws", &deleted).unwrap(),
        None,
        "nothing holds it now"
    );
    println!("this build's gc keeps both held blobs and collects the released one");
    mgr.release().unwrap();
}
