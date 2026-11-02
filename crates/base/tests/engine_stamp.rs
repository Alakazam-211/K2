//! The engine stamp is checked on load (v0.10.1): a stored doc records the engine and the
//! stored-state format that wrote it (`DocMeta::engine`, `DocMeta::engine_format`), and a
//! session never reads one another engine wrote. Opening it is refused with `MPC-E520`
//! `engine_mismatch` (exit 6) and changes nothing. A stamp of this engine's family at another
//! library version, with this format, loads; so does an unstamped doc (an empty name).
//!
//! The docs are made by this build and then restamped through the store, as a build on
//! another engine (or a rollback from one) would leave them. Uses only the public API.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::errors::Refusal;
use k2_mpc_base::session::{
    AuditEvent, Config, Host, Participant, ParticipantKind, PurgeRequest, PurgeScope, Rights,
    Session, SessionError, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocId, DocKey, DocMeta, DocStore, StoreError, STORE_FORMAT_READ};
use k2_mpc_base::tools::{exit, session_exit};

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
const TEXT: &str = "# Title\n\nbody\n";

struct TestHost {
    now: AtomicU64,
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
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: PathBuf,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-stamp-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let dir = std::fs::canonicalize(dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    host.rights
        .lock()
        .unwrap()
        .insert("a".into(), Rights::editor());
    Fx { dir, host, store }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

impl Fx {
    /// A fresh manager over the store (a new process, as far as the store can tell).
    fn mgr(&self) -> SessionManager {
        SessionManager::new(self.store.clone(), self.host.clone(), Config::default())
    }

    /// A doc at `path` holding [`TEXT`], made by this build, its manager released.
    fn made(&self, path: &str) -> DocId {
        let mgr = self.mgr();
        drop(
            mgr.open(Arc::new(MD), &key(path), Some(TEXT.as_bytes()))
                .unwrap(),
        );
        mgr.release().unwrap();
        self.store.find(&key(path)).unwrap().unwrap()
    }

    /// Restamp doc `id` as written by `engine` in `format`; nothing else changes.
    fn restamp(&self, id: DocId, engine: &str, format: u32) {
        let mut meta = self.store.meta(id).unwrap();
        meta.engine = engine.to_owned();
        meta.engine_format = format;
        self.store.put_meta(id, &meta).unwrap();
    }

    /// This build's own stamp, as it stores it on a doc it makes.
    fn own_stamp(&self) -> (String, u32) {
        let id = self.made("own.md");
        let meta = self.store.meta(id).unwrap();
        (meta.engine, meta.engine_format)
    }
}

fn text(s: &Session) -> String {
    let who = Participant::new("a", "A", "#123456", ParticipantKind::Agent, "cli");
    let c = s.join(who, Stay::UntilIdle).unwrap().conn;
    s.read(c).unwrap().text_by_root.remove(0).1
}

/// The refusal, checked the way a host sees it: the id, the exit, the wording.
#[track_caller]
fn assert_engine_mismatch(e: &SessionError, stored: &str, format: u32) {
    let r = e.report();
    assert_eq!(
        (r.id(), r.exit(), session_exit(e)),
        ("MPC-E520", exit::STORE, exit::STORE),
        "{e:?}"
    );
    let msg = e.to_string();
    let head = format!("this doc was written by engine {stored} (format {format}); ");
    assert!(msg.starts_with(&head), "{msg}");
    assert!(
        msg[head.len()..].starts_with("this kit runs engine yrs"),
        "{msg}"
    );
    assert_eq!(r.message(), msg);
}

/// The failing-first test: a doc stamped by another engine refuses to load, with a typed
/// error, and the refusal changes nothing (the doc isn't loaded, its stamp and content stay).
#[test]
fn a_doc_stamped_by_another_engine_refuses_to_load() {
    let f = fx("other");
    let id = f.made("a.md");
    f.restamp(id, "loro-1.0", 1);
    let mgr = f.mgr();
    let e = mgr
        .open(Arc::new(MD), &key("a.md"), None)
        .err()
        .expect("a doc stamped by another engine must not load");
    match &e {
        SessionError::EngineMismatch {
            stored,
            stored_format,
            ..
        } => assert_eq!((stored.as_str(), *stored_format), ("loro-1.0", 1)),
        other => panic!("{other:?}"),
    }
    assert_engine_mismatch(&e, "loro-1.0", 1);
    // nothing loaded, nothing changed; asking again refuses again
    assert!(mgr.get(&key("a.md")).is_none());
    let meta = f.store.meta(id).unwrap();
    assert_eq!((meta.engine.as_str(), meta.engine_format), ("loro-1.0", 1));
    let e = mgr.open(Arc::new(MD), &key("a.md"), None).err().unwrap();
    assert_engine_mismatch(&e, "loro-1.0", 1);
    // bytes offered for a stored doc don't make it importable: the stored doc wins
    let e = mgr
        .open(Arc::new(MD), &key("a.md"), Some(b"other\n"))
        .err()
        .unwrap();
    assert_engine_mismatch(&e, "loro-1.0", 1);
    mgr.release().unwrap();
}

/// The same through `open_file` (a doc that lives in a file): refused before the file is
/// attached or read.
#[test]
fn open_file_refuses_another_engines_doc() {
    let f = fx("file");
    let ws = f.dir.join("ws");
    std::fs::write(ws.join("b.md"), TEXT).unwrap();
    let id = f.made("b.md");
    f.restamp(id, "mpce-0.1", 1);
    let mgr = f.mgr();
    let e = mgr
        .open_file(Arc::new(MD), &key("b.md"), &ws.join("b.md"), &ws)
        .err()
        .expect("a doc stamped by another engine must not load");
    assert_engine_mismatch(&e, "mpce-0.1", 1);
    assert!(mgr.get(&key("b.md")).is_none());
    assert_eq!(std::fs::read_to_string(ws.join("b.md")).unwrap(), TEXT);
    mgr.release().unwrap();
}

/// This engine at another stored-state format is another engine as far as loading goes; at
/// another library version with this format, it loads.
#[test]
fn the_format_counts_the_library_version_does_not() {
    let f = fx("format");
    let (own, format) = f.own_stamp();
    let family = own.split('-').next().unwrap().to_owned();
    assert_eq!(family, "yrs", "this build's stamp is {own:?}");

    let id = f.made("c.md");
    f.restamp(id, &own, format + 1);
    let mgr = f.mgr();
    let e = mgr.open(Arc::new(MD), &key("c.md"), None).err().unwrap();
    assert_engine_mismatch(&e, &own, format + 1);
    mgr.release().unwrap();

    let id = f.made("d.md");
    f.restamp(id, &format!("{family}-0.1"), format);
    let mgr = f.mgr();
    let s = mgr.open(Arc::new(MD), &key("d.md"), None).unwrap();
    assert_eq!(text(&s), TEXT);
    drop(s);
    mgr.release().unwrap();
}

/// An unstamped doc (an empty engine name; no store the kit wrote has one, but a host's own
/// `DocStore::create` can) loads as this engine's, and keeps working.
#[test]
fn an_unstamped_doc_loads_as_this_engines() {
    let f = fx("unstamped");
    let id = f.made("e.md");
    f.restamp(id, "", 0);
    let mgr = f.mgr();
    let s = mgr.open(Arc::new(MD), &key("e.md"), None).unwrap();
    assert_eq!(text(&s), TEXT);
    drop(s);
    mgr.release().unwrap();
    // and again, after a release
    let mgr = f.mgr();
    let s = mgr.open(Arc::new(MD), &key("e.md"), None).unwrap();
    assert_eq!(text(&s), TEXT);
    drop(s);
    mgr.release().unwrap();
}

// ------------------------------------------------- a format-7 store (v0.10.1, forward-declared)

/// Stamp the store at `path` format `f` (and a pending rewrite, when `pending`), through redb
/// itself: as the native engine's build would leave it. This build never writes format 7.
fn stamp_store(path: &std::path::Path, f: u64, pending: bool) {
    use redb::{Database, TableDefinition};
    const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
    let d = Database::open(path).unwrap();
    let w = d.begin_write().unwrap();
    {
        let mut m = w.open_table(META).unwrap();
        m.insert("store_format", f).unwrap();
        if pending {
            m.insert("rewrite_pending", 1).unwrap();
        }
    }
    w.commit().unwrap();
}

/// A store, made by this build and then stamped format 7 by hand, as the native engine's
/// build would leave it: a Yrs doc (`y.md`), a doc the native engine wrote (`n.md`, stamped
/// `mpce-0.1` format 1), and a table only the native build knows. Returns its folder and path.
fn format_7_store(tag: &str) -> (PathBuf, PathBuf, Arc<TestHost>) {
    let dir = std::env::temp_dir().join(format!("k2mb-stamp7-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("docs.redb");
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    host.rights
        .lock()
        .unwrap()
        .insert("a".into(), Rights::full().with_owner(true));
    {
        let store = Arc::new(DocStore::open(&path).unwrap());
        let mgr = SessionManager::new(store.clone(), host.clone(), Config::default());
        for p in ["y.md", "n.md"] {
            drop(
                mgr.open(Arc::new(MD), &key(p), Some(TEXT.as_bytes()))
                    .unwrap(),
            );
        }
        mgr.release().unwrap();
        drop(mgr);
        let n = store.find(&key("n.md")).unwrap().unwrap();
        let meta = DocMeta {
            engine: "mpce-0.1".into(),
            engine_format: 1,
            ..store.meta(n).unwrap()
        };
        store.put_meta(n, &meta).unwrap();
        store.add_table_for_tests("mpce_events", false).unwrap();
    }
    stamp_store(&path, 7, false);
    (dir, path, host)
}

/// v0.10.1 reads a format-7 store (the native engine's) without writing its format, and never
/// changes what it can't read. Opening the store writes nothing (its bytes are the same after
/// an open and a close). A Yrs doc in it is served: read, edited, blamed. A doc the native
/// engine wrote is refused with `MPC-E520` on open and on delete (alone and with its space),
/// and nothing is deleted. A purge (which rewrites the file) and a whole-store rewrite are
/// refused with `MPC-E502` before anything changes. A purge of the native doc itself can't
/// be asked for: a purge needs the doc open, and opening it is refused (`MPC-E520`); the
/// store's own purge checks the stamp too.
#[test]
fn a_format_7_store_serves_yrs_docs_and_refuses_native_ones() {
    let (dir, path, host) = format_7_store("serve");
    let before = std::fs::read(&path).unwrap();
    {
        let store = DocStore::open(&path).unwrap();
        assert_eq!(store.format().unwrap(), 7);
        assert_eq!(STORE_FORMAT_READ, 7);
    }
    assert!(
        std::fs::read(&path).unwrap() == before,
        "opening a format-7 store wrote to it"
    );
    let store = Arc::new(DocStore::open(&path).unwrap());
    let mgr = SessionManager::new(store.clone(), host.clone(), Config::default());
    // the Yrs doc is served
    let s = mgr.open(Arc::new(MD), &key("y.md"), None).unwrap();
    let who = Participant::new("a", "A", "#123456", ParticipantKind::Person, "cli");
    let c = s.join(who, Stay::Held).unwrap().conn;
    assert_eq!(s.read(c).unwrap().text_by_root[0].1, TEXT);
    s.replace_text(c, TextFilePack::ROOT, "# Title\n\nbody, edited\n")
        .unwrap();
    assert_eq!(s.blame(c, TextFilePack::ROOT, None).unwrap().lines.len(), 3);
    assert_eq!(
        store.format().unwrap(),
        7,
        "the stamp stays 7: never lowered"
    );
    // the native doc: refused on open, delete and delete_space; nothing deleted
    let e = mgr.open(Arc::new(MD), &key("n.md"), None).err().unwrap();
    assert_engine_mismatch(&e, "mpce-0.1", 1);
    let e = mgr.delete_doc(&key("n.md")).unwrap_err();
    assert_engine_mismatch(&e, "mpce-0.1", 1);
    let e = mgr.delete_space("ws").unwrap_err();
    assert_engine_mismatch(&e, "mpce-0.1", 1);
    assert!(store.find(&key("n.md")).unwrap().is_some());
    assert!(store.find(&key("y.md")).unwrap().is_some());
    assert!(
        mgr.get(&key("y.md")).is_some(),
        "a refused delete_space closes nothing"
    );
    // the store's own delete and its whole-store rewrite refuse too
    let n = store.find(&key("n.md")).unwrap().unwrap();
    let e = store.delete_doc(n, 5).unwrap_err();
    assert!(matches!(e, StoreError::ForeignEngine { .. }), "{e:?}");
    assert_eq!(e.report().id(), "MPC-E520");
    let e = store.rewrite_file().unwrap_err();
    assert_eq!(e, StoreError::RewriteNewer { found: 7 });
    assert_eq!(
        (e.report().id(), e.report().exit()),
        ("MPC-E502", exit::STORE)
    );
    // a purge (of the Yrs doc) rewrites the file: refused before anything changes
    let e = s
        .purge(c, PurgeRequest::new(PurgeScope::History))
        .unwrap_err();
    assert_eq!(e.report().id(), "MPC-E502", "{e:?}");
    assert_eq!(
        s.read(c).unwrap().text_by_root[0].1,
        "# Title\n\nbody, edited\n"
    );
    mgr.release().unwrap();
    drop((s, mgr, store));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A rewrite the native engine's build left pending on its format-7 store isn't finished
/// here: the open is refused with `MPC-E502` and the file is left as it was. A store past
/// what this build reads (format 8) is refused as newer.
#[test]
fn a_format_7_store_with_a_pending_rewrite_is_refused_untouched() {
    let (dir, path, _host) = format_7_store("pending");
    stamp_store(&path, 7, true);
    let before = std::fs::read(&path).unwrap();
    let e = DocStore::open(&path).err().expect("refused");
    assert_eq!(e, StoreError::RewriteNewer { found: 7 });
    assert_eq!(e.report().id(), "MPC-E502");
    assert!(
        std::fs::read(&path).unwrap() == before,
        "a refused open wrote to the store"
    );
    stamp_store(&path, 8, false);
    assert_eq!(
        DocStore::open(&path).err(),
        Some(StoreError::Newer { found: 8 })
    );
    let _ = std::fs::remove_dir_all(&dir);
}
