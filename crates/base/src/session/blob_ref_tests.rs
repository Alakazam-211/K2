//! A doc's references on the blobs its values name, through sessions (`store/blob_refs.rs`,
//! `docs/SESSIONS.md`, "Blob references"). A synthetic pack: a list `items` and maps `props` and
//! `notes`, with a field naming an image-like blob; only `items` and `props` are declared.
//!
//! `MPC_STRUCT_MUTANT` plants a fault for the test that catches it:
//! `ref-after-commit` (`BlobRefMutant::RefAfterCommit`, the interrupted write),
//! `no-heal-at-load` (`BlobRefMutant::NoHealAtLoad`, the heal), `gc-ignores-names`
//! (`BlobRefMutant::GcIgnoresNames`, a blob arriving after its name), `stale-strict-flag`
//! (`BlobRefMutant::StaleStrictFlag`, an undo refused for a missing blob) and
//! `refs-ignore-rights` (`BlobRefMutant::RefsIgnoreRights`, a reader shown history). Any other
//! value is refused.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::blob::{BlobConfig, BlobId, BlobRefState, BlobRefs, BlobStore};
use crate::doc::{Edit, Neutral};
use crate::elem::{Anchor, Key, ListOp, MapOp, Value};
use crate::schema::{Kind, Schema};
use crate::store::{CommitInterrupt, DocId, DocKey, DocStore, INTERRUPT_COMMIT};
use crate::tools::session_exit;

const GRACE: u64 = 60 * 60_000;
/// The participant [`H`] gives read and nothing else.
const READER: &str = "reader";

/// One line per value of `props` in a file: `<key>=<value>`.
struct Items {
    roots: &'static [&'static str],
}

const DECLARED: Items = Items {
    roots: &["items", "props"],
};
/// The same doc under a kit that takes no references, as an older one's writes are.
const UNDECLARED: Items = Items { roots: &[] };

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
    fn decode(&self, b: &[u8]) -> std::result::Result<PackContent, Declined> {
        let props: BTreeMap<String, Value> = std::str::from_utf8(b)
            .unwrap()
            .lines()
            .map(|l| {
                let (k, v) = l.split_once('=').unwrap();
                (k.to_owned(), Value::Str(v.to_owned()))
            })
            .collect();
        Ok(PackContent {
            roots: vec![
                ("items".to_owned(), Neutral::List(Vec::new())),
                ("props".to_owned(), Neutral::Map(props)),
                ("notes".to_owned(), Neutral::Map(BTreeMap::new())),
            ],
            codec: Vec::new(),
        })
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> std::result::Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
    fn blob_ref_roots(&self) -> &'static [&'static str] {
        self.roots
    }
}

struct H(Arc<AtomicU64>);
impl Host for H {
    fn rights(&self, who: &Participant, _: &DocKey) -> Rights {
        if who.pid == READER {
            return Rights::read_only();
        }
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

/// Removes the test's folder when the last fixture over it goes.
struct Folder(PathBuf);
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    folder: Arc<Folder>,
    now: Arc<AtomicU64>,
    store: Arc<DocStore>,
    blobs: Arc<BlobStore>,
    mgr: SessionManager,
}

impl Fx {
    fn new(tag: &str) -> Self {
        Self::fresh(tag, true)
    }

    fn fresh(tag: &str, with_blob_store: bool) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "k2mb-blob-refs-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self::open(
            Arc::new(Folder(dir)),
            Arc::new(AtomicU64::new(1_000_000)),
            with_blob_store,
        )
    }

    /// Over the test's files, with the store open afresh.
    fn open(folder: Arc<Folder>, now: Arc<AtomicU64>, with_blob_store: bool) -> Self {
        let dir = &folder.0;
        let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
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
        // a test's edits come in one burst on a clock that stands still
        let config = Config {
            edit_rate_per_s: 1_000_000,
            edit_burst: 1_000_000,
            ..Config::default()
        };
        let mgr = SessionManager::new(store.clone(), Arc::new(H(now.clone())), config);
        let mgr = match with_blob_store {
            true => mgr.with_blob_store(blobs.clone()),
            false => mgr,
        };
        Self {
            folder,
            now,
            store,
            blobs,
            mgr,
        }
    }

    /// Everything closed, as a process that ended; the store open afresh.
    fn reopen(self) -> Self {
        let Self {
            folder,
            now,
            store,
            blobs,
            mgr,
        } = self;
        mgr.release().unwrap();
        drop(mgr);
        drop(blobs);
        drop(store);
        Self::open(folder, now, true)
    }

    fn doc(&self, path: &str, pack: Items, bytes: &[u8]) -> (Arc<Session>, ConnId) {
        let s = self
            .mgr
            .open(Arc::new(pack), &key(path), Some(bytes))
            .unwrap();
        let c = s
            .join(
                Participant::new("p", "P", "#111111", ParticipantKind::Person, "cli"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        (s, c)
    }

    fn put(&self, bytes: &[u8]) -> BlobId {
        self.blobs.put_bytes("ws", bytes).unwrap().id
    }

    fn refs_on(&self, b: &BlobId) -> Option<u64> {
        self.blobs.info("ws", b).unwrap().map(|(_, r)| r)
    }

    fn past_grace(&self) {
        self.now.fetch_add(GRACE + 1, Ordering::SeqCst);
    }

    fn id(&self, path: &str) -> DocId {
        self.store.find(&key(path)).unwrap().unwrap()
    }

    fn holds_table(&self) -> bool {
        use redb::TableHandle;
        let r = self.store.begin_read().unwrap();
        let held = r
            .list_tables()
            .unwrap()
            .any(|t| t.name() == "doc_blob_refs");
        held
    }
}

fn key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

fn set(field: &str, value: &str) -> Vec<Edit> {
    vec![Edit::Map {
        root: "props".into(),
        ops: vec![MapOp::Set {
            key: field.to_owned(),
            value: Value::Str(value.to_owned()),
        }],
    }]
}

fn edit(s: &Session, c: ConnId, edits: Vec<Edit>) -> std::result::Result<EditResult, SessionError> {
    s.edit_ops(c, edits, &EditOptions::new())
}

fn prop(s: &Session, c: ConnId, field: &str) -> Option<Value> {
    let content = s.read(c).unwrap().content;
    match &content.iter().find(|(r, _)| r == "props")?.1 {
        Neutral::Map(m) => m.get(field).cloned(),
        other => panic!("props is a map: {other:?}"),
    }
}

fn ids(r: &BlobRefs) -> Vec<(BlobId, BlobRefState)> {
    r.refs.iter().map(|x| (x.blob, x.state)).collect()
}

fn under_mutant<R>(name: &str, m: crate::differential::BlobRefMutant, f: impl FnOnce() -> R) -> R {
    let v = std::env::var("MPC_STRUCT_MUTANT");
    println!("MPC_STRUCT_MUTANT={}", v.as_deref().unwrap_or("(unset)"));
    match v.as_deref() {
        Ok(x) if x == name => crate::differential::with_blob_ref_mutant(m, f),
        Ok(other) => panic!("MPC_STRUCT_MUTANT={other}: not a mutant this test plants"),
        Err(_) => f(),
    }
}

/// A value exactly naming a blob takes the doc's one count, in the change's transaction, however
/// many values name it.
#[test]
fn a_named_blob_gets_one_count_per_doc() {
    let f = Fx::new("one-count");
    let (s, c) = f.doc("a.items", DECLARED, b"");
    let (x, y) = (f.put(b"image x"), f.put(b"image y"));
    edit(&s, c, set("i1\x1fimage", &x.to_ref())).unwrap();
    edit(&s, c, set("i2\x1fimage", &x.to_ref())).unwrap();
    edit(&s, c, set("i3\x1fimage", &y.to_ref())).unwrap();
    assert_eq!(f.refs_on(&x), Some(1));
    assert_eq!(f.refs_on(&y), Some(1));
    let refs = s.blob_refs(c).unwrap();
    assert_eq!(refs.missing, 0);
    let mut want = vec![(x, BlobRefState::Present), (y, BlobRefState::Present)];
    want.sort_by_key(|w| w.0);
    assert_eq!(ids(&refs), want);
    // a list element's value counts too
    let z = f.put(b"image z");
    edit(
        &s,
        c,
        vec![Edit::List {
            root: "items".into(),
            ops: vec![ListOp::Insert {
                key: Key::new("i9").unwrap(),
                at: Anchor::End,
                value: Value::Str(z.to_ref()),
            }],
        }],
    )
    .unwrap();
    assert_eq!(f.refs_on(&z), Some(1));
}

/// Only a value that is exactly a reference counts, and only in a declared root; a pack that
/// declares none gets no table, and its values can spell anything.
#[test]
fn only_an_exact_value_in_a_declared_root_counts() {
    let f = Fx::new("exact");
    let (s, c) = f.doc("a.items", DECLARED, b"");
    let x = f.put(b"image x");
    let good = x.to_ref();
    for v in [
        format!("see {good}"),
        format!("{good} "),
        good.to_uppercase(),
        format!("blob:{}", "zz".repeat(32)),
    ] {
        edit(&s, c, set("i1\x1flabel", &v)).unwrap();
    }
    edit(
        &s,
        c,
        vec![Edit::Map {
            root: "notes".into(),
            ops: vec![MapOp::Set {
                key: "n".into(),
                value: Value::Str(good.clone()),
            }],
        }],
    )
    .unwrap();
    assert_eq!(f.refs_on(&x), Some(0));
    assert_eq!(s.blob_refs(c).unwrap(), BlobRefs::default());
    assert!(!f.holds_table(), "nothing named: the table isn't created");

    let (t, tc) = f.doc("b.items", UNDECLARED, b"");
    let missing = BlobId::from_slice(&[9; 32]).unwrap();
    edit(&t, tc, set("i1\x1fimage", &missing.to_ref())).unwrap();
    assert!(!f.holds_table(), "a pack that declares none takes none");
}

/// A connection's edit naming a blob the doc's space doesn't hold is refused whole, with exit 4;
/// overwriting or deleting such a name isn't.
#[test]
fn an_edit_naming_no_blob_is_refused_and_nothing_applies() {
    let f = Fx::new("refused");
    let unknown = BlobId::from_slice(&[9; 32]).unwrap();
    let dangling = format!("i1\x1fimage={}\n", unknown.to_ref());
    let (s, c) = f.doc("a.items", DECLARED, dangling.as_bytes());
    let elsewhere = f.blobs.put_bytes("other", b"in another space").unwrap().id;
    let before = s.read(c).unwrap().version;
    for b in [BlobId::from_slice(&[8; 32]).unwrap(), elsewhere] {
        let e = edit(&s, c, set("i2\x1fimage", &b.to_ref())).unwrap_err();
        assert!(
            matches!(&e, SessionError::BlobRefUnknown { blob, .. } if *blob == b.to_hex()),
            "{e:?}"
        );
        assert_eq!(session_exit(&e), 4);
        assert_eq!(crate::errors::Refusal::report(&e).id(), "MPC-E720");
    }
    assert_eq!(s.read(c).unwrap().version, before, "nothing applied");
    assert_eq!(prop(&s, c, "i2\x1fimage"), None);
    // the name that's already there can be replaced, and deleted
    edit(&s, c, set("i1\x1fimage", "plain")).unwrap();
    edit(
        &s,
        c,
        vec![Edit::Map {
            root: "props".into(),
            ops: vec![MapOp::Delete {
                key: "i1\x1fimage".into(),
            }],
        }],
    )
    .unwrap();
}

/// Two docs share a blob, and one's older version names it. The blob survives GC while any record
/// of either names it. Each purge recomputes that doc's references; the last one to let go removes
/// the bytes at once, leaving no trace of the id in the store file.
#[test]
fn a_blob_survives_gc_until_the_last_record_naming_it_is_purged() {
    let f = Fx::new("gc-safety");
    let (a, ac) = f.doc("a.items", DECLARED, b"");
    let (b, bc) = f.doc("b.items", DECLARED, b"");
    let x = f.put(b"shared image");
    edit(&a, ac, set("i1\x1fimage", &x.to_ref())).unwrap();
    edit(&b, bc, set("i1\x1fimage", &x.to_ref())).unwrap();
    assert_eq!(f.refs_on(&x), Some(2));

    // a's value moves on; its version still names it, and so does its record
    let v = a.name_version(ac, "with the image", None).unwrap();
    edit(&a, ac, set("i1\x1fimage", "plain")).unwrap();
    edit(&b, bc, set("i1\x1fimage", "plain")).unwrap();
    assert_eq!(
        f.refs_on(&x),
        Some(2),
        "overwriting a value releases nothing"
    );
    f.past_grace();
    assert_eq!(f.blobs.gc().unwrap().removed, 0);
    assert_eq!(
        ids(&a.blob_refs_at(ac, v).unwrap()),
        vec![(x, BlobRefState::Present)]
    );

    // b's history goes: its names with it, a's hold stays
    let done = b.purge(bc, PurgeRequest::new(PurgeScope::History)).unwrap();
    assert_eq!(f.refs_on(&x), Some(1));
    assert_eq!(done.counts.blobs_removed, 0);
    assert!(
        done.not_reached.contains(&NotReached::SharedBlobs(1)),
        "{:?}",
        done.not_reached
    );
    assert_eq!(b.blob_refs(bc).unwrap(), BlobRefs::default());
    f.past_grace();
    f.blobs.gc().unwrap();
    assert!(
        f.blobs.read_to_vec("ws", &x, 1024).is_ok(),
        "a still names it"
    );

    // a's history goes: the last record, so the bytes go at once
    let done = a.purge(ac, PurgeRequest::new(PurgeScope::History)).unwrap();
    assert_eq!(done.counts.blobs_removed, 1);
    assert_eq!(f.blobs.info("ws", &x).unwrap(), None);
    assert!(matches!(
        f.blobs.reader("ws", &x),
        Err(crate::blob::BlobError::NotFound(_))
    ));
    let hex = x.to_hex();
    assert_eq!(
        f.store
            .scan_for(&[hex.as_bytes(), x.as_bytes().as_slice()])
            .unwrap(),
        Vec::new(),
        "a purged reference leaves no byte of the id in the store file"
    );
}

/// A purge keeps what the doc still names: the live value, and a hidden element's (a restore
/// brings it back). A scoped purge recomputes too: a version outside its scope keeps the hold.
#[test]
fn a_purge_keeps_what_the_doc_still_names() {
    let f = Fx::new("purge-keeps");
    let (s, c) = f.doc("a.items", DECLARED, b"");
    let (live, hidden) = (f.put(b"live image"), f.put(b"hidden image"));
    edit(&s, c, set("i1\x1fimage", &live.to_ref())).unwrap();
    let k = Key::new("i9").unwrap();
    edit(
        &s,
        c,
        vec![Edit::List {
            root: "items".into(),
            ops: vec![ListOp::Insert {
                key: k.clone(),
                at: Anchor::End,
                value: Value::Str(hidden.to_ref()),
            }],
        }],
    )
    .unwrap();
    edit(
        &s,
        c,
        vec![Edit::List {
            root: "items".into(),
            ops: vec![ListOp::Delete { key: k.clone() }],
        }],
    )
    .unwrap();
    s.purge(c, PurgeRequest::new(PurgeScope::History)).unwrap();
    assert_eq!(f.refs_on(&live), Some(1));
    assert_eq!(f.refs_on(&hidden), Some(1));
    f.past_grace();
    assert_eq!(f.blobs.gc().unwrap().removed, 0);
    assert_eq!(s.blob_refs(c).unwrap().refs.len(), 2);
}

/// With no blob store on the session, a purge gives the count back and blob GC collects the blob
/// after its grace, not before. It's reported as not reached.
#[test]
fn without_a_blob_store_gc_collects_what_a_purge_released() {
    let g = Fx::fresh("gc-after-purge", false);
    let (s, c) = g.doc("a.items", DECLARED, b"");
    let x = g.put(b"image");
    edit(&s, c, set("i1\x1fimage", &x.to_ref())).unwrap();
    edit(&s, c, set("i1\x1fimage", "plain")).unwrap();
    let done = s.purge(c, PurgeRequest::new(PurgeScope::History)).unwrap();
    assert!(
        done.not_reached.contains(&NotReached::LeftForBlobGc(1)),
        "{:?}",
        done.not_reached
    );
    assert!(
        !done.not_reached.contains(&NotReached::SharedBlobs(1)),
        "nothing else uses it: {:?}",
        done.not_reached
    );
    assert_eq!(g.refs_on(&x), Some(0));
    assert_eq!(
        g.blobs.gc().unwrap().removed,
        0,
        "its grace starts at the purge"
    );
    g.past_grace();
    assert_eq!(g.blobs.gc().unwrap().removed, 1);
    assert_eq!(g.blobs.info("ws", &x).unwrap(), None);
}

/// A doc delete erases the rows and gives each count back: proven on the store, not by the list.
/// A doc moved to another space gives the count back in the space it was taken in.
#[test]
fn a_doc_delete_gives_its_references_back_and_leaves_no_row() {
    let f = Fx::new("delete");
    let (a, ac) = f.doc("a.items", DECLARED, b"");
    let (b, bc) = f.doc("b.items", DECLARED, b"");
    let (m, mc) = f.doc("m.items", DECLARED, b"");
    let (x, y) = (f.put(b"shared"), f.put(b"moved"));
    edit(&a, ac, set("f", &x.to_ref())).unwrap();
    edit(&b, bc, set("f", &x.to_ref())).unwrap();
    edit(&m, mc, set("f", &y.to_ref())).unwrap();
    let (id_a, id_b) = (f.id("a.items"), f.id("b.items"));
    assert!(f
        .store
        .tables_holding(id_a)
        .unwrap()
        .contains(&"doc_blob_refs"));

    f.mgr.delete_doc(&key("a.items")).unwrap();
    assert_eq!(f.store.tables_holding(id_a).unwrap(), Vec::<&str>::new());
    assert_eq!(f.refs_on(&x), Some(1));
    assert!(f
        .store
        .tables_holding(id_b)
        .unwrap()
        .contains(&"doc_blob_refs"));

    f.mgr.delete_doc(&key("b.items")).unwrap();
    assert_eq!(f.refs_on(&x), Some(0));
    assert_eq!(
        f.blobs.gc().unwrap().removed,
        0,
        "its grace starts at the delete"
    );
    f.past_grace();
    assert_eq!(f.blobs.gc().unwrap().removed, 1);
    assert_eq!(f.blobs.info("ws", &x).unwrap(), None);

    // moved to another space, then deleted: the count goes back where it was taken
    let to = DocKey {
        space: "elsewhere".into(),
        path: "m.items".into(),
    };
    f.mgr.rekey(&key("m.items"), to.clone()).unwrap();
    assert_eq!(
        ids(&m.blob_refs(mc).unwrap()),
        vec![(y, BlobRefState::Present)],
        "the row knows its space"
    );
    f.mgr.delete_doc(&to).unwrap();
    assert_eq!(f.refs_on(&y), Some(0));
}

/// A rewrite copies the rows, and the counts stay.
#[test]
fn a_rewrite_keeps_the_references() {
    let f = Fx::new("rewrite");
    let (s, c) = f.doc("a.items", DECLARED, b"");
    let x = f.put(b"image");
    edit(&s, c, set("f", &x.to_ref())).unwrap();
    let before = s.blob_refs(c).unwrap();
    f.store.check_rewritable().unwrap();
    f.store.rewrite_file().unwrap();
    assert_eq!(s.blob_refs(c).unwrap(), before);
    assert_eq!(f.refs_on(&x), Some(1));
}

/// The reference is in the change's own transaction: a write interrupted before the commit leaves
/// neither, one interrupted after it leaves both, through a reopen of the store. Stops are made by
/// the store's test-only interrupt, and nothing loads the doc again before the checks, so a load's
/// heal can't cover for it. `ref-after-commit` plants `BlobRefMutant::RefAfterCommit`, caught at
/// "the change committed without its reference".
#[test]
fn an_interrupted_write_leaves_the_change_and_its_reference_together() {
    under_mutant(
        "ref-after-commit",
        crate::differential::BlobRefMutant::RefAfterCommit,
        || {
            let f = Fx::new("interrupted");
            let (s, c) = f.doc("a.items", DECLARED, b"");
            let x = f.put(b"image");
            let id = f.id("a.items");
            let entries = f.store.log_size(id).unwrap().0;

            INTERRUPT_COMMIT.with(|i| i.set(Some(CommitInterrupt::Before)));
            let r = edit(&s, c, set("f", &x.to_ref()));
            INTERRUPT_COMMIT.with(|i| i.set(None));
            assert!(r.is_err());
            assert_eq!(f.store.log_size(id).unwrap().0, entries, "no change");
            assert_eq!(f.refs_on(&x), Some(0), "no reference");
            assert_eq!(f.store.blob_refs(id).unwrap(), BlobRefs::default());

            INTERRUPT_COMMIT.with(|i| i.set(Some(CommitInterrupt::After)));
            let r = edit(&s, c, set("f", &x.to_ref()));
            INTERRUPT_COMMIT.with(|i| i.set(None));
            assert!(r.is_err());
            drop(s);
            let f = f.reopen();
            assert_eq!(f.store.log_size(id).unwrap().0, entries + 1, "the change");
            assert_eq!(
                f.refs_on(&x),
                Some(1),
                "the change committed without its reference"
            );
            assert_eq!(
                ids(&f.store.blob_refs(id).unwrap()),
                vec![(x, BlobRefState::Present)]
            );
            f.past_grace();
            f.blobs.gc().unwrap();
            assert!(f.blobs.read_to_vec("ws", &x, 1024).is_ok());
        },
    );
}

/// An older kit's write names a blob and takes no reference (here, the same doc under a pack that
/// declares no roots). The next load takes it, so GC keeps the blob from then on; a second load
/// has nothing to do. `no-heal-at-load` plants `BlobRefMutant::NoHealAtLoad`, caught at "the load
/// took no hold".
#[test]
fn a_load_takes_a_hold_for_a_name_an_older_kit_wrote() {
    under_mutant(
        "no-heal-at-load",
        crate::differential::BlobRefMutant::NoHealAtLoad,
        || {
            let f = Fx::new("heal");
            let (s, c) = f.doc("a.items", UNDECLARED, b"");
            let x = f.put(b"image");
            edit(&s, c, set("f", &x.to_ref())).unwrap();
            assert_eq!(f.refs_on(&x), Some(0), "an older kit's write takes none");
            drop(s);
            f.mgr.close(&key("a.items"), "test").unwrap();

            let (s, c) = f.doc("a.items", DECLARED, b"");
            assert_eq!(f.refs_on(&x), Some(1), "the load took no hold");
            assert_eq!(
                ids(&s.blob_refs(c).unwrap()),
                vec![(x, BlobRefState::Present)]
            );
            f.past_grace();
            assert_eq!(f.blobs.gc().unwrap().removed, 0);
            drop(s);
            f.mgr.close(&key("a.items"), "test").unwrap();
            f.doc("a.items", DECLARED, b"");
            assert_eq!(f.refs_on(&x), Some(1), "one count, however many loads");
        },
    );
}

/// A name that arrives with no blob behind it isn't refused off a connection's edit, and it isn't
/// silent either: a read says it's missing and counts it. Here, through an import, an undo and a
/// version restore, with the doc's value left as it was.
#[test]
fn a_name_with_no_blob_behind_it_is_visible_on_read() {
    let f = Fx::new("visible");
    // the import: a name whose bytes haven't arrived
    let bytes: &[u8] = b"a late image";
    let (late, _) = f.blobs.id_of("ws", &mut &bytes[..]).unwrap();
    let file = format!("i1\x1fimage={}\n", late.to_ref());
    let (s, c) = f.doc("a.items", DECLARED, file.as_bytes());
    let refs = s.blob_refs(c).unwrap();
    assert_eq!(ids(&refs), vec![(late, BlobRefState::Missing)]);
    assert_eq!(refs.missing, 1);
    let v = s.versions(c).unwrap()[0].id;
    let at = s.blob_refs_at(c, v).unwrap();
    assert_eq!(ids(&at), vec![(late, BlobRefState::Missing)]);
    assert_eq!(at.missing, 1);
    assert_eq!(
        prop(&s, c, "i1\x1fimage"),
        Some(Value::Str(late.to_ref())),
        "the value stays"
    );

    // the bytes arrive: it reads as present, and the next load takes the hold
    assert_eq!(f.put(bytes), late);
    assert_eq!(s.blob_refs(c).unwrap().missing, 0);
    assert_eq!(f.refs_on(&late), Some(0));
    drop(s);
    f.mgr.close(&key("a.items"), "test").unwrap();
    let (s, c) = f.doc("a.items", DECLARED, b"");
    assert_eq!(f.refs_on(&late), Some(1));

    // an undo and a restore bring a name back after its space's blobs were destroyed
    let x = f.put(b"image x");
    edit(&s, c, set("f", &x.to_ref())).unwrap();
    let v = s.name_version(c, "x", None).unwrap();
    edit(&s, c, set("f", "plain")).unwrap();
    f.blobs.purge_space_key("ws").unwrap();
    s.undo(c, 1).unwrap();
    assert_eq!(prop(&s, c, "f"), Some(Value::Str(x.to_ref())));
    let refs = s.blob_refs(c).unwrap();
    assert!(refs.refs.iter().all(|r| r.state == BlobRefState::Missing));
    assert_eq!(refs.missing, refs.refs.len() as u64);
    edit(&s, c, set("f", "plain again")).unwrap();
    s.restore(c, v).unwrap();
    assert_eq!(prop(&s, c, "f"), Some(Value::Str(x.to_ref())));
    assert_eq!(s.blob_refs_at(c, v).unwrap().missing, 2);
    // a connection's own edit of that name is still refused
    let e = edit(&s, c, set("g", &x.to_ref())).unwrap_err();
    assert!(matches!(e, SessionError::BlobRefUnknown { .. }), "{e:?}");
}

/// A name on record with no count, and the blob arrives: this kit's GC never collects it, and
/// takes the doc's count instead. No load, purge or change comes between. The count is a real
/// one: the doc's delete gives it back, and only then does GC collect the blob.
/// `gc-ignores-names` plants `BlobRefMutant::GcIgnoresNames`, caught at "a blob whose name is on
/// record was collected".
#[test]
fn a_blob_that_arrives_after_its_name_is_never_collected() {
    under_mutant(
        "gc-ignores-names",
        crate::differential::BlobRefMutant::GcIgnoresNames,
        || {
            let f = Fx::new("arrives");
            let bytes: &[u8] = b"a late image";
            let (late, _) = f.blobs.id_of("ws", &mut &bytes[..]).unwrap();
            let file = format!("i1\x1fimage={}\n", late.to_ref());
            let (s, c) = f.doc("a.items", DECLARED, file.as_bytes());
            let (t, tc) = f.doc("b.items", DECLARED, file.as_bytes());
            assert_eq!(s.blob_refs(c).unwrap().missing, 1);
            assert_eq!(f.put(bytes), late);
            assert_eq!(
                f.refs_on(&late),
                Some(0),
                "no count until something takes it"
            );
            f.past_grace();
            assert_eq!(
                f.blobs.gc().unwrap().removed,
                0,
                "a blob whose name is on record was collected"
            );
            assert_eq!(f.refs_on(&late), Some(2), "one count per doc naming it");
            assert!(f.blobs.read_to_vec("ws", &late, 1024).is_ok());
            assert_eq!(
                ids(&s.blob_refs(c).unwrap()),
                vec![(late, BlobRefState::Present)]
            );
            assert_eq!(ids(&t.blob_refs(tc).unwrap()).len(), 1);
            f.past_grace();
            assert_eq!(
                f.blobs.gc().unwrap().removed,
                0,
                "the counts hold from then on"
            );

            f.mgr.delete_doc(&key("a.items")).unwrap();
            f.mgr.delete_doc(&key("b.items")).unwrap();
            assert_eq!(f.refs_on(&late), Some(0));
            f.past_grace();
            assert_eq!(f.blobs.gc().unwrap().removed, 1, "nothing names it now");
        },
    );
}

/// The same for a purge's removal, which has no grace: doc a lets go of a blob doc b names with
/// no count yet. The blob stays, b takes the count, and the purge says it was left.
#[test]
fn a_purges_removal_spares_a_blob_another_docs_name_is_waiting_on() {
    under_mutant(
        "gc-ignores-names",
        crate::differential::BlobRefMutant::GcIgnoresNames,
        || {
            let f = Fx::new("arrives-purge");
            let bytes: &[u8] = b"a late image";
            let (late, _) = f.blobs.id_of("ws", &mut &bytes[..]).unwrap();
            let file = format!("i1\x1fimage={}\n", late.to_ref());
            let (_b, _bc) = f.doc("b.items", DECLARED, file.as_bytes());
            let (a, ac) = f.doc("a.items", DECLARED, b"");
            assert_eq!(f.put(bytes), late);
            edit(&a, ac, set("f", &late.to_ref())).unwrap();
            edit(&a, ac, set("f", "plain")).unwrap();
            assert_eq!(f.refs_on(&late), Some(1), "only a's so far");
            let done = a.purge(ac, PurgeRequest::new(PurgeScope::History)).unwrap();
            assert_eq!(done.counts.blobs_removed, 0, "b's name is on record");
            assert!(
                done.not_reached.contains(&NotReached::SharedBlobs(1)),
                "{:?}",
                done.not_reached
            );
            assert_eq!(f.refs_on(&late), Some(1), "b's count");
            assert!(f.blobs.read_to_vec("ws", &late, 1024).is_ok());
        },
    );
}

/// Blob ids are keyed per space: a name in another space's doc is about another blob, so it holds
/// nothing here, and GC collects this one as usual.
#[test]
fn a_name_in_another_space_holds_nothing_here() {
    let f = Fx::new("other-space");
    let x = f.put(b"unreferenced image");
    let elsewhere = DocKey {
        space: "elsewhere".into(),
        path: "a.items".into(),
    };
    let file = format!("i1\x1fimage={}\n", x.to_ref());
    let s = f
        .mgr
        .open(Arc::new(DECLARED), &elsewhere, Some(file.as_bytes()))
        .unwrap();
    let c = s
        .join(
            Participant::new("p", "P", "#111111", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    assert_eq!(s.blob_refs(c).unwrap().missing, 1);
    f.past_grace();
    assert_eq!(f.blobs.gc().unwrap().removed, 1);
    assert_eq!(f.blobs.info("ws", &x).unwrap(), None);
}

/// Only the edit checked for it refuses a name with nothing behind it. An edit that names a held
/// blob and is refused after that check leaves nothing for the change after it: an undo and a
/// version restore bringing back a name whose blob has gone both land, and a read says it's
/// missing. A guard: `stale-strict-flag` plants `BlobRefMutant::StaleStrictFlag`, caught at "a
/// later change ran strict".
#[test]
fn a_strict_edit_leaves_nothing_behind_for_the_next_change() {
    under_mutant(
        "stale-strict-flag",
        crate::differential::BlobRefMutant::StaleStrictFlag,
        || {
            let f = Fx::new("strict-stale");
            let (s, c) = f.doc("a.items", DECLARED, b"");
            let (held, gone) = (f.put(b"a held image"), f.put(b"an image that goes"));
            let insert = |value: Value| {
                vec![Edit::List {
                    root: "items".into(),
                    ops: vec![ListOp::Insert {
                        key: Key::new("i1").unwrap(),
                        at: Anchor::End,
                        value,
                    }],
                }]
            };
            edit(&s, c, insert(Value::Null)).unwrap();
            edit(&s, c, set("g", &held.to_ref())).unwrap();
            edit(&s, c, set("f", &gone.to_ref())).unwrap();
            let v = s.name_version(c, "with the image", None).unwrap();
            edit(&s, c, set("f", "plain")).unwrap();

            // passes the check, then refused by the engine: the key is taken
            let e = edit(&s, c, insert(Value::Str(held.to_ref()))).unwrap_err();
            assert!(!matches!(e, SessionError::BlobRefUnknown { .. }), "{e:?}");
            assert_eq!(prop(&s, c, "f"), Some(Value::Str("plain".to_owned())));

            f.blobs.purge_space_key("ws").unwrap();
            s.undo(c, 1).expect("a later change ran strict");
            assert_eq!(prop(&s, c, "f"), Some(Value::Str(gone.to_ref())));
            let refs = s.blob_refs(c).unwrap();
            assert!(
                ids(&refs).contains(&(gone, BlobRefState::Missing)),
                "{refs:?}"
            );
            assert_eq!(refs.missing, refs.refs.len() as u64);

            edit(&s, c, set("f", "plain again")).unwrap();
            s.restore(c, v).expect("a later change ran strict");
            assert_eq!(prop(&s, c, "f"), Some(Value::Str(gone.to_ref())));
        },
    );
}

/// `blob_refs` must not show a reader what only history holds: a blob's id says the bytes were
/// once in the doc. Without the history right it returns what a read of the doc shows, and
/// `missing` counts those alone; with it, every name on record. The doc's holds don't depend on
/// who asks. `refs-ignore-rights` plants `BlobRefMutant::RefsIgnoreRights`, caught at "a reader
/// was shown a name only history holds".
#[test]
fn without_the_history_right_blob_refs_shows_what_a_read_shows() {
    under_mutant(
        "refs-ignore-rights",
        crate::differential::BlobRefMutant::RefsIgnoreRights,
        || {
            let f = Fx::new("refs-rights");
            let (gone_old, gone_live) = (
                BlobId::from_slice(&[1; 32]).unwrap(),
                BlobId::from_slice(&[2; 32]).unwrap(),
            );
            let file = format!("a={}\nb={}\n", gone_old.to_ref(), gone_live.to_ref());
            let (s, c) = f.doc("a.items", DECLARED, file.as_bytes());
            let r = s
                .join(
                    Participant::new(READER, "R", "#222222", ParticipantKind::Person, "cli"),
                    Stay::Held,
                )
                .unwrap()
                .conn;
            let (old, live, hidden) = (
                f.put(b"only an old version names this"),
                f.put(b"the doc names this"),
                f.put(b"a deleted element names this"),
            );
            edit(&s, c, set("c", &old.to_ref())).unwrap();
            let v = s.name_version(c, "old", None).unwrap();
            edit(&s, c, set("c", "plain")).unwrap();
            edit(&s, c, set("d", &live.to_ref())).unwrap();
            edit(&s, c, set("a", "plain")).unwrap();
            let k = Key::new("i1").unwrap();
            edit(
                &s,
                c,
                vec![Edit::List {
                    root: "items".into(),
                    ops: vec![ListOp::Insert {
                        key: k.clone(),
                        at: Anchor::End,
                        value: Value::Str(hidden.to_ref()),
                    }],
                }],
            )
            .unwrap();
            edit(
                &s,
                c,
                vec![Edit::List {
                    root: "items".into(),
                    ops: vec![ListOp::Delete { key: k }],
                }],
            )
            .unwrap();

            let sorted = |mut v: Vec<(BlobId, BlobRefState)>| {
                v.sort_by_key(|x| x.0);
                v
            };
            let seen = s.blob_refs(r).unwrap();
            assert_eq!(
                ids(&seen),
                sorted(vec![
                    (live, BlobRefState::Present),
                    (gone_live, BlobRefState::Missing),
                ]),
                "a reader was shown a name only history holds"
            );
            assert_eq!(seen.missing, 1, "only what it can see");

            let all = s.blob_refs(c).unwrap();
            assert_eq!(
                ids(&all),
                sorted(vec![
                    (old, BlobRefState::Present),
                    (live, BlobRefState::Present),
                    (hidden, BlobRefState::Present),
                    (gone_old, BlobRefState::Missing),
                    (gone_live, BlobRefState::Missing),
                ])
            );
            assert_eq!(all.missing, 2);

            let e = s.blob_refs_at(r, v).unwrap_err();
            assert!(matches!(e, SessionError::Forbidden { .. }), "{e:?}");
            assert_eq!(
                ids(&s.blob_refs_at(c, v).unwrap()).len(),
                3,
                "a version names what it held"
            );
            assert_eq!(
                f.refs_on(&old),
                Some(1),
                "the hold doesn't depend on who asks"
            );
        },
    );
}

/// The exits of the apply itself, which no public call reaches on this engine: an edit that passed
/// the check, then changed nothing, was refused or poisoned the doc. Each leaves nothing for the
/// next change: a version restore bringing back a name whose blob has gone lands.
/// `stale-strict-flag` plants `BlobRefMutant::StaleStrictFlag`, caught at "a later change ran
/// strict".
#[test]
fn a_strict_edit_that_lands_nothing_leaves_nothing_behind() {
    use crate::differential::{with_apply_fault, ApplyFault};
    under_mutant(
        "stale-strict-flag",
        crate::differential::BlobRefMutant::StaleStrictFlag,
        || {
            for (tag, fault) in [
                ("no-change", ApplyFault::NoChange),
                ("refused", ApplyFault::Refused),
                ("poisoned", ApplyFault::Poisoned),
            ] {
                let f = Fx::new(&format!("strict-{tag}"));
                let (s, c) = f.doc("a.items", DECLARED, b"");
                let (held, gone) = (f.put(b"a held image"), f.put(b"an image that goes"));
                edit(&s, c, set("g", &held.to_ref())).unwrap();
                edit(&s, c, set("f", &gone.to_ref())).unwrap();
                let v = s.name_version(c, "with the image", None).unwrap();
                edit(&s, c, set("f", "plain")).unwrap();
                let at = s.read(c).unwrap().version;

                let r = with_apply_fault(fault, || edit(&s, c, set("g", &held.to_ref())));
                assert_eq!(r.is_ok(), fault == ApplyFault::NoChange, "{tag}: {r:?}");
                assert_eq!(s.read(c).unwrap().version, at, "{tag}: nothing landed");

                f.blobs.purge_space_key("ws").unwrap();
                s.restore(c, v).expect("a later change ran strict");
                assert_eq!(prop(&s, c, "f"), Some(Value::Str(gone.to_ref())), "{tag}");
            }
        },
    );
}
