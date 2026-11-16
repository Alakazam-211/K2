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
use k2_mpc_base::store::{
    DocId, DocKey, DocStore, StoreError, FEATURE_CHUNKED, FEATURE_NATIVE, STORE_FORMAT_READ,
};
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
/// attached or read. A native doc's stamp: without the native engine, refused as that doc's
/// own (`MPC-E169`, its own variant); with it, sessions don't take native docs yet
/// (`MPC-E520`).
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
    {
        assert_eq!(
            e,
            SessionError::NativeEngineMissing {
                stored: "mpce-0.1".into(),
                stored_format: 1,
            }
        );
        assert_eq!(
            (e.report().id(), session_exit(&e)),
            ("MPC-E169", exit::STORE)
        );
    }
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

// ------------------------------------------- store formats 7 and 8 (the required-features word)

/// Stamp the store at `path` format `f`, with features word `word` (`None`: none), and a
/// pending rewrite when `pending`, through redb itself: as another build would leave it. This
/// build never writes format 7, and stamps format 8 only with a store's first chunked version.
fn stamp_store(path: &std::path::Path, f: u64, word: Option<u64>, pending: bool) {
    use redb::{Database, TableDefinition};
    const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
    let d = Database::open(path).unwrap();
    let w = d.begin_write().unwrap();
    {
        let mut m = w.open_table(META).unwrap();
        m.insert("store_format", f).unwrap();
        match word {
            Some(v) => m.insert("required_features", v).unwrap(),
            None => m.remove("required_features").unwrap(),
        };
        if pending {
            m.insert("rewrite_pending", 1).unwrap();
        } else {
            m.remove("rewrite_pending").unwrap();
        }
    }
    w.commit().unwrap();
}

/// A store made by this build (a Yrs doc, `y.md`) and then stamped format `f` with features
/// word `word` by hand. Returns its folder and path.
fn stamped_store(tag: &str, f: u64, word: Option<u64>) -> (PathBuf, PathBuf, Arc<TestHost>) {
    let dir = std::env::temp_dir().join(format!("k2mb-stamp{f}-{tag}-{}", std::process::id()));
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
        drop(
            mgr.open(Arc::new(MD), &key("y.md"), Some(TEXT.as_bytes()))
                .unwrap(),
        );
        mgr.release().unwrap();
    }
    stamp_store(&path, f, word, false);
    (dir, path, host)
}

/// A format-8 store naming only the chunked feature opens here as any store this build writes:
/// it serves the Yrs doc in it (read, edited, blamed), never lowers its stamp or drops its
/// word, and a purge (which rewrites the file) and a whole-store rewrite copy it with its meta.
#[test]
fn a_format_8_store_serves_yrs_docs_and_is_rewritten_here() {
    let (dir, path, host) = stamped_store("serve", 8, Some(FEATURE_CHUNKED));
    {
        let store = DocStore::open(&path).expect("row: a format-8 store naming chunked opens");
        assert_eq!(store.format().unwrap(), 8);
        assert_eq!(store.required_features().unwrap(), FEATURE_CHUNKED);
        assert_eq!(STORE_FORMAT_READ, 8);
    }
    let store = Arc::new(DocStore::open(&path).unwrap());
    let mgr = SessionManager::new(store.clone(), host.clone(), Config::default());
    let s = mgr.open(Arc::new(MD), &key("y.md"), None).unwrap();
    let who = Participant::new("a", "A", "#123456", ParticipantKind::Person, "cli");
    let c = s.join(who, Stay::Held).unwrap().conn;
    assert_eq!(s.read(c).unwrap().text_by_root[0].1, TEXT);
    s.replace_text(c, TextFilePack::ROOT, "# Title\n\nbody, edited\n")
        .unwrap();
    assert_eq!(s.blame(c, TextFilePack::ROOT, None).unwrap().lines.len(), 3);
    assert_eq!(
        store.format().unwrap(),
        8,
        "the stamp stays 8: never lowered"
    );
    assert_eq!(store.required_features().unwrap(), FEATURE_CHUNKED);
    store
        .rewrite_file()
        .expect("row: a format-8 store is rewritten here");
    assert_eq!(
        (store.format().unwrap(), store.required_features().unwrap()),
        (8, FEATURE_CHUNKED),
        "row: the rewrite keeps the stamp and the word"
    );
    s.purge(c, PurgeRequest::new(PurgeScope::History))
        .expect("row: a purge of a format-8 store runs here");
    assert_eq!(
        s.read(c).unwrap().text_by_root[0].1,
        "# Title\n\nbody, edited\n"
    );
    assert_eq!(
        (store.format().unwrap(), store.required_features().unwrap()),
        (8, FEATURE_CHUNKED),
        "row: the purge's rewrite keeps the stamp and the word"
    );
    mgr.release().unwrap();
    drop((s, mgr, store));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Format 7 was never written (declared for the native engine's stores and retired): a store
/// stamped 7 is refused at open with `MPC-E503`, exit 6, the file left as it was, with a
/// pending rewrite or a features word too. A rewrite another build left pending on a format-8
/// store is finished here, the stamp and the word kept. Past what this build reads (format 9),
/// the store is refused as newer.
#[test]
fn a_format_7_store_is_refused_untouched() {
    let (dir, path, _host) = stamped_store("never", 7, None);
    for (word, pending) in [(None, false), (None, true), (Some(FEATURE_CHUNKED), false)] {
        stamp_store(&path, 7, word, pending);
        let before = std::fs::read(&path).unwrap();
        let e = DocStore::open(&path)
            .err()
            .expect("row: format 7 is refused at open");
        assert_eq!(
            e,
            StoreError::NeverWritten { found: 7 },
            "word {word:?}, pending {pending}"
        );
        assert_eq!(e.to_string(), "store is format 7, which no kit writes");
        assert_eq!(
            (e.report().id(), e.report().exit()),
            ("MPC-E503", exit::STORE)
        );
        assert!(
            std::fs::read(&path).unwrap() == before,
            "a refused open wrote to the store"
        );
    }
    stamp_store(&path, 8, Some(FEATURE_CHUNKED), true);
    {
        let s = DocStore::open(&path).expect("row: a pending rewrite on format 8 is finished");
        assert_eq!(
            (s.format().unwrap(), s.required_features().unwrap()),
            (8, FEATURE_CHUNKED),
            "row: the finished rewrite keeps the stamp and the word"
        );
    }
    stamp_store(&path, 9, None, false);
    assert_eq!(
        DocStore::open(&path).err(),
        Some(StoreError::Newer { found: 9 })
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------- N−1 of store format 8 (tools/n1/n1.sh)

/// The directory `tools/n1/n1.sh node-format` names (`K2MB_N1_OUT`).
fn n1_out() -> PathBuf {
    PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT: run by tools/n1/n1.sh"))
}

/// The three format-8 stores the N−1 check hands the previous release: each made by this build
/// (a Yrs doc, `y.md`) and stamped format 8 by hand, its word naming chunked, native, and both.
const N1_WORDS: [(&str, u64); 3] = [
    ("chunked", FEATURE_CHUNKED),
    ("native", FEATURE_NATIVE),
    ("both", FEATURE_CHUNKED | FEATURE_NATIVE),
];

/// N−1 of store format 8, this build's half before the previous release's
/// (`tools/n1/n1.sh node-format`): `chunked.redb`, `native.redb` and `both.redb` in
/// `K2MB_N1_OUT`, and `chunked.versions`, the chunked store's doc's versions as this build
/// reads them (`{:?}`), for the previous release to check by its read range
/// (`tools/n1/consumer-format`): a release that reads up to format 7 refuses each at open; one
/// that reads format 8 opens the chunked store and reads the doc as written, and refuses the
/// two that set native.
#[test]
#[ignore = "N-1: tools/n1/n1.sh node-format"]
fn n1_format_producer() {
    let out = n1_out();
    std::fs::create_dir_all(&out).unwrap();
    for (name, word) in N1_WORDS {
        let (dir, path, _host) = stamped_store(&format!("n1-{name}"), 8, Some(word));
        std::fs::copy(&path, out.join(format!("{name}.redb"))).unwrap();
        if word == FEATURE_CHUNKED {
            let store = DocStore::open(&path).unwrap();
            let id = store.find(&key("y.md")).unwrap().expect("y.md");
            let versions = store.versions(id).unwrap();
            let file = out.join(format!("{name}.versions"));
            std::fs::write(file, format!("{versions:?}")).unwrap();
            let n = versions.len();
            println!("N-1 FORMAT produced {name}.versions: {n} version(s)");
        }
        let _ = std::fs::remove_dir_all(&dir);
        println!("N-1 FORMAT produced {name}.redb: format 8, features word {word}");
    }
}

/// N−1 of store format 8, this build's half after the previous release's: the store the
/// previous release wrote (`prev.redb`, a text doc) opens here as before, keeps its format
/// (only a chunked version stamps 8) with no features word, reads, takes a write and rewrites;
/// the format-8 stores the previous release read or refused are as this build made them:
/// `chunked` opens at format 8 with its word and its versions as `chunked.versions` holds them,
/// `native` and `both` are refused (no engine store) with their bytes unchanged.
#[test]
#[ignore = "N-1: tools/n1/n1.sh node-format"]
fn n1_format_after() {
    let out = n1_out();
    let prev = out.join("prev.redb");
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_800_000_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    host.rights
        .lock()
        .unwrap()
        .insert("a".into(), Rights::full().with_owner(true));
    {
        let store = Arc::new(DocStore::open(&prev).unwrap());
        let format = store.format().unwrap();
        assert!(format < 8, "the previous release wrote format {format}");
        assert_eq!(store.required_features().unwrap(), 0);
        let mgr = SessionManager::new(store.clone(), host.clone(), Config::default());
        let k = DocKey {
            space: "ws".into(),
            path: "a.md".into(),
        };
        let s = mgr.open(Arc::new(MD), &k, None).unwrap();
        let who = Participant::new("a", "A", "#123456", ParticipantKind::Person, "cli");
        let c = s.join(who, Stay::Held).unwrap().conn;
        assert_eq!(
            s.read(c).unwrap().text_by_root[0].1,
            "# Written by the previous release\n\nbody\n"
        );
        s.replace_text(
            c,
            TextFilePack::ROOT,
            "# Written by the previous release\n\nedited\n",
        )
        .unwrap();
        mgr.release().unwrap();
        drop((s, mgr));
        store.rewrite_file().unwrap();
        assert_eq!(
            store.format().unwrap(),
            format,
            "the format stays: only a chunked version stamps 8"
        );
        assert_eq!(store.required_features().unwrap(), 0);
        println!("N-1 FORMAT AFTER prev.redb: opened, written and rewritten at format {format}");
    }
    for (name, word) in N1_WORDS {
        let p = out.join(format!("{name}.redb"));
        let before = std::fs::read(&p).unwrap();
        match DocStore::open(&p) {
            Ok(s) => {
                assert_eq!(word, FEATURE_CHUNKED, "{name} opened");
                assert_eq!(s.format().unwrap(), 8);
                assert_eq!(s.required_features().unwrap(), word);
                let id = s.find(&key("y.md")).unwrap().expect("y.md");
                let versions = format!("{:?}", s.versions(id).unwrap());
                let want = std::fs::read_to_string(out.join(format!("{name}.versions"))).unwrap();
                assert_eq!(
                    versions, want,
                    "{name}: the versions as this build wrote them"
                );
                println!(
                    "N-1 FORMAT AFTER {name}.redb: opens at format 8, its versions as written"
                );
            }
            Err(e) => {
                assert_ne!(word, FEATURE_CHUNKED, "{name}: {e}");
                assert_eq!(e, StoreError::NativeWithoutEngine, "{name}");
                assert!(
                    std::fs::read(&p).unwrap() == before,
                    "{name}: a refused open changed the file"
                );
                println!("N-1 FORMAT AFTER {name}.redb: refused: {e}");
            }
        }
    }
    println!("N-1 FORMAT AFTER OK");
}
