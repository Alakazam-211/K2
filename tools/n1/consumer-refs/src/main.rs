//! N−1 of the blob references (tools/n1/n1.sh `layout-refs`), built against the previous release,
//! which knows nothing of them. The store comes from this repo's `n1_refs_producer`: `held` named
//! by a.items, `deleted` named by b.items, `spare` stored and named by nothing (`ids.txt`).
//!
//! - **The held case:** past the grace, this build's blob GC keeps `held` and `deleted` (their
//!   counts are in `blobs.refs`, which it reads) and collects `spare`.
//! - **The limit:** a value this build writes takes no reference. With nothing else holding the
//!   blob, this build's GC collects it past its grace while the doc still names it. Stated and
//!   checked here: it is the documented N−1 limit (`docs/SESSIONS.md`, "Blob references").
//! - It refuses a rewrite, and so a purge, before changing anything (`doc_blob_refs`), and its
//!   doc delete leaves the rows and the count. This repo's `n1_refs_after` reads what's left.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::blob::{BlobConfig, BlobError, BlobId, BlobStore};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{MapOp, Value};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, Config, Declined, EditOptions, Host, Pack, PackContent, PackError, Participant,
    ParticipantKind, Rights, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore, StoreError};

const HOUR: u64 = 60 * 60_000;

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
}

struct H(Arc<AtomicU64>);
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

fn named(ids: &str, what: &str) -> BlobId {
    ids.lines()
        .find_map(|l| l.strip_prefix(what)?.strip_prefix(' '))
        .and_then(BlobId::from_hex)
        .unwrap_or_else(|| panic!("ids.txt has no {what}"))
}

fn set(field: &str, b: &BlobId) -> Vec<Edit> {
    vec![Edit::Map {
        root: "props".into(),
        ops: vec![MapOp::Set {
            key: field.to_owned(),
            value: Value::Str(format!("blob:{}", b.to_hex())),
        }],
    }]
}

fn main() {
    let ids = std::fs::read_to_string("ids.txt").expect("ids.txt");
    let (held, deleted, spare) = (
        named(&ids, "held"),
        named(&ids, "deleted"),
        named(&ids, "spare"),
    );
    let now = Arc::new(AtomicU64::new(1_000_000 + 2 * HOUR));
    let store = Arc::new(DocStore::open("docs.redb").expect("the store opens"));
    let clock = now.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            "blobs",
            BlobConfig::default(),
            Arc::new(move || clock.load(Ordering::SeqCst)),
        )
        .unwrap(),
    );

    // the held case
    let gc = blobs.gc().unwrap();
    println!("older gc past the grace: removed {}", gc.removed);
    assert!(
        blobs.read_to_vec("ws", &held, 1024).is_ok(),
        "the older gc collected a blob this kit took a reference on"
    );
    assert!(blobs.read_to_vec("ws", &deleted, 1024).is_ok());
    assert!(matches!(
        blobs.reader("ws", &spare),
        Err(BlobError::NotFound(_))
    ));
    println!("HELD: a reference the newer kit took survives the older gc");

    // the limit
    let mgr = SessionManager::new(store.clone(), Arc::new(H(now.clone())), Config::default());
    let s = mgr.open(Arc::new(Items), &key("a.items"), None).unwrap();
    let c = s
        .join(
            Participant::new("p", "P", "#111111", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let limit = blobs.put_bytes("ws", b"named on the older kit").unwrap().id;
    s.edit_ops(c, set("limit", &limit), &EditOptions::new())
        .unwrap();
    assert_eq!(blobs.info("ws", &limit).unwrap().map(|x| x.1), Some(0));
    now.fetch_add(2 * HOUR, Ordering::SeqCst);
    blobs.gc().unwrap();
    let v = s.name_version(c, "named", None).unwrap();
    let content = s.show_content(c, v).unwrap();
    let Neutral::Map(m) = &content.iter().find(|(r, _)| r == "props").unwrap().1 else {
        panic!("props is a map")
    };
    assert!(m.contains_key("limit"), "the doc still names it");
    assert!(matches!(
        blobs.reader("ws", &limit),
        Err(BlobError::NotFound(_))
    ));
    println!("LIMIT: a name written on the older kit, with no other reference, is collected");

    // a name still there when the newer kit comes back
    let heal = blobs.put_bytes("ws", b"named, and still here").unwrap().id;
    s.edit_ops(c, set("heal", &heal), &EditOptions::new())
        .unwrap();
    std::fs::write(
        "ids.txt",
        format!("{ids}limit {}\nheal {}\n", limit.to_hex(), heal.to_hex()),
    )
    .unwrap();
    mgr.release().unwrap();
    drop((s, mgr));

    match store.check_rewritable() {
        Err(StoreError::NewerData { table }) if table == "doc_blob_refs" => {
            println!("rewrite refused before any change (table {table:?})");
        }
        other => panic!("expected the rewrite refused, got {other:?}"),
    }
    let b = store.find(&key("b.items")).unwrap().expect("b.items");
    store.delete_doc(b, now.load(Ordering::SeqCst)).unwrap();
    assert_eq!(
        blobs.info("ws", &deleted).unwrap().map(|x| x.1),
        Some(1),
        "its delete can't give back a count it doesn't know"
    );
    println!("doc delete: left the rows and the count");
    println!("N-1 REFS (older side) OK");
}
