//! The manager's listing, rights re-check and delete (C7b). Crate-internal: the delete
//! tests enumerate the store's actual tables (`DocStore::tables_holding`), so a per-doc table
//! added later without being erased fails them.

use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::blob::{BlobConfig, BlobStore};
use crate::store::{DocKey, DocStore};

struct H {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
    /// `rights` panics for this pid (to poison a session's lock).
    panic_for: Mutex<Option<String>>,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for H {
    fn rights(&self, who: &Participant, _: &DocKey) -> Rights {
        if self.panic_for.lock().unwrap().as_deref() == Some(who.pid.as_str()) {
            panic!("host rights failed for {}", who.pid);
        }
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::full())
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
const GRACE: u64 = 5_000;

struct Fx {
    dir: std::path::PathBuf,
    store: Arc<DocStore>,
    host: Arc<H>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    let dir = std::env::temp_dir().join(format!("k2mb-mgr-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("ws")).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let host = Arc::new(H {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
        panic_for: Mutex::new(None),
        events: Mutex::new(Vec::new()),
    });
    let h = host.clone();
    let bc = BlobConfig {
        gc_grace_ms: GRACE,
        ..BlobConfig::default()
    };
    let blobs = BlobStore::open(
        store.clone(),
        dir.join("blobs"),
        bc,
        Arc::new(move || h.now.load(Ordering::SeqCst)),
    )
    .unwrap();
    let c = Config {
        edit_burst: 10_000,
        edit_rate_per_s: 10_000,
        compact_entries: 3, // a snapshot early
        // the window moves, with a pending record older than its cut
        revert_keep_changes: 2,
        revert_keep_ms: 1_000,
        history_tick_ms: 0,
        ..Config::default()
    };
    let mgr = SessionManager::new(store.clone(), host.clone(), c).with_blob_store(Arc::new(blobs));
    Fx {
        dir,
        store,
        host,
        mgr,
    }
}

fn key(space: &str, path: &str) -> DocKey {
    DocKey {
        space: space.into(),
        path: path.into(),
    }
}

fn person(pid: &str) -> Participant {
    Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab")
}

impl Fx {
    fn text(&self, k: &DocKey, body: &str) -> Arc<Session> {
        self.mgr
            .open(Arc::new(MD), k, Some(body.as_bytes()))
            .unwrap()
    }

    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }

    fn id(&self, k: &DocKey) -> u64 {
        self.store.find(k).unwrap().unwrap()
    }

    /// Doc `ws/a.md` with rows in **every** per-doc text table: a file (sync), an assistant's
    /// on-behalf edit (approval, pending revert record), enough edits for a snapshot, and a
    /// revert window that moved past the pending record (its carried bounds persisted).
    fn full_text_doc(&self) -> (Arc<Session>, DocKey, ConnId) {
        let k = key("ws", "a.md");
        let file = self.dir.join("ws/a.md");
        fs::write(&file, "one\ntwo\nthree\n").unwrap();
        let s = self
            .mgr
            .open_file(Arc::new(MD), &k, &file, &self.dir.join("ws"))
            .unwrap();
        let p = s.join(person("p"), Stay::Held).unwrap().conn;
        let asst = s
            .join(
                Participant::new("asst", "A", "#000000", ParticipantKind::Agent, "cli")
                    .on_behalf_of("p"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        s.replace_text(asst, "source", "one\ntwo (asst)\nthree\n")
            .unwrap();
        for i in 0..6 {
            s.edit_with(p, "source", |t| Ok::<_, ()>(format!("{t}line {i}\n")))
                .unwrap();
        }
        self.advance(10 * 60_000);
        let rep = self.mgr.tick().unwrap();
        assert!(rep.reverts_dropped > 0, "{rep:?}");
        s.flush(p).unwrap();
        (s, k, p)
    }
}

const TEXT_TABLES: [&str; 13] = [
    "approvals",
    "changes",
    "clients",
    "docs",
    "paths",
    "revert_carry",
    "revert_index",
    "revert_stats",
    "reverts",
    "snapshots",
    "sync",
    "updates",
    "versions",
];

#[test]
fn loaded_keys_lists_text_and_tier0_sorted() {
    let f = fx("keys");
    assert!(f.mgr.loaded_keys().is_empty());
    let _b = f.text(&key("ws", "b.md"), "b\n");
    let _a = f.text(&key("ws", "a.md"), "a\n");
    let _z = f.text(&key("other", "z.md"), "z\n");
    let _blob = f
        .mgr
        .open_blob(Arc::new(AnyFile), &key("ws", "c.bin"), Some(&mut &b"c"[..]))
        .unwrap();
    assert_eq!(
        f.mgr.loaded_keys(),
        vec![
            key("other", "z.md"),
            key("ws", "a.md"),
            key("ws", "b.md"),
            key("ws", "c.bin")
        ]
    );
}

#[test]
fn recheck_rights_over_every_loaded_doc_and_reports_failures() {
    let f = fx("recheck");
    let (ka, kb, kc) = (key("ws", "a.md"), key("ws", "b.md"), key("ws", "c.bin"));
    let a = f.text(&ka, "a\n");
    let b = f.text(&kb, "b\n");
    let c = f
        .mgr
        .open_blob(Arc::new(AnyFile), &kc, Some(&mut &b"c"[..]))
        .unwrap();
    a.join(person("x"), Stay::Held).unwrap();
    let bx = b.join(person("x"), Stay::Held).unwrap().conn;
    b.join(person("y"), Stay::Held).unwrap();
    c.join(person("x"), Stay::Held).unwrap();
    assert!(f.mgr.recheck_rights().is_empty());
    // x goes read-only everywhere: one change per doc they are in
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("x".into(), Rights::read_only());
    let got: Vec<(DocKey, String, Rights)> = f
        .mgr
        .recheck_rights()
        .into_iter()
        .map(|(k, c)| (k, c.pid, c.rights))
        .collect();
    let ro = Rights::read_only();
    assert_eq!(
        got,
        vec![
            (ka.clone(), "x".into(), ro),
            (kb.clone(), "x".into(), ro),
            (kc.clone(), "x".into(), ro)
        ]
    );
    // losing read removes them (and only them)
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("x".into(), Rights::none());
    assert_eq!(f.mgr.recheck_rights().len(), 3);
    assert_eq!(b.status().unwrap().participants, 1);
    assert!(matches!(b.read(bx), Err(SessionError::NotJoined { .. })));
    // a session that fails is skipped and reported, not swallowed: poison b's lock
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("x".into(), Rights::full());
    a.join(person("x"), Stay::Held).unwrap();
    *f.host.panic_for.lock().unwrap() = Some("y".into());
    let b2 = b.clone();
    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _ = b2.recheck_rights();
    }));
    assert!(poisoned.is_err());
    *f.host.panic_for.lock().unwrap() = None;
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("x".into(), Rights::editor());
    f.host.events.lock().unwrap().clear();
    let got = f.mgr.recheck_rights();
    assert_eq!(got.len(), 1, "{got:?}"); // x in a; b failed
    assert_eq!(got[0].0, ka);
    let failed: Vec<DocKey> = f
        .host
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AuditEvent::RecheckFailed { key, error } => {
                assert!(error.contains("poisoned"), "{error}");
                Some(key.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(failed, vec![kb]);
    // a closed session is skipped without a report
    f.mgr.delete_doc(&kc).unwrap();
    f.host.events.lock().unwrap().clear();
    let _ = c.recheck_rights();
    f.mgr.recheck_rights();
    assert!(!f
        .host
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, AuditEvent::RecheckFailed { key, .. } if *key == kc)));
}

#[test]
fn delete_doc_erases_every_table_and_closes_its_session() {
    let f = fx("deldoc");
    let (s, ka, p) = f.full_text_doc();
    let rx = s.subscribe(p).unwrap();
    let ida = f.id(&ka);
    // the fixture really fills every per-doc text table (or the check below means nothing)
    assert_eq!(f.store.tables_holding(ida).unwrap(), TEXT_TABLES.to_vec());
    // neighbours: another doc of the space, loaded, and one of another space, not loaded
    let kc = key("ws", "c.md");
    let c = f.text(&kc, "c\n");
    let cc = c.join(person("q"), Stay::Held).unwrap().conn;
    c.replace_text(cc, "source", "c2\n").unwrap();
    let kd = key("other", "d.md");
    drop(f.text(&kd, "d\n"));
    let (idc, idd) = (f.id(&kc), f.id(&kd));
    let before = (
        f.store.tables_holding(idc).unwrap(),
        f.store.tables_holding(idd).unwrap(),
    );
    f.host.events.lock().unwrap().clear();

    f.mgr.delete_doc(&ka).unwrap();

    // nothing of it is left, anywhere
    assert_eq!(f.store.tables_holding(ida).unwrap(), Vec::<&str>::new());
    assert_eq!(f.store.find(&ka).unwrap(), None);
    assert!(f.store.meta(ida).is_err());
    // its session closed, typed: participants are told, handles fail, subscriptions end
    let ev: Vec<AuditEvent> = f.host.events.lock().unwrap().clone();
    let deleted: Vec<&AuditEvent> = ev
        .iter()
        .filter(|e| matches!(e, AuditEvent::DocDeleted { .. }))
        .collect();
    assert_eq!(deleted.len(), 1);
    match deleted[0] {
        AuditEvent::DocDeleted { key, connections } => {
            assert_eq!(*key, ka);
            let mut pids: Vec<&str> = connections.iter().map(|(_, p)| p.as_str()).collect();
            pids.sort_unstable();
            assert_eq!(pids, vec!["asst", "p"]);
        }
        _ => unreachable!(),
    }
    assert_eq!(s.read(p).unwrap_err(), SessionError::Deleted);
    assert_eq!(
        crate::tools::session_exit(&SessionError::Deleted),
        crate::tools::exit::NOT_FOUND
    );
    assert!(rx.recv().is_err());
    assert!(f.mgr.get(&ka).is_none());
    assert!(!f.mgr.loaded_keys().contains(&ka));
    // the file is the host's: untouched
    assert!(f.dir.join("ws/a.md").exists());
    // the others are untouched
    assert_eq!(
        (
            f.store.tables_holding(idc).unwrap(),
            f.store.tables_holding(idd).unwrap()
        ),
        before
    );
    assert_eq!(c.read(cc).unwrap().text_by_root[0].1, "c2\n");
    // a new doc can take the key; deleting nothing is NotFound
    f.mgr.delete_doc(&ka).unwrap_err();
    assert_eq!(
        f.mgr.delete_doc(&key("ws", "nope.md")),
        Err(SessionError::NotFound(key("ws", "nope.md")))
    );
    let again = f.text(&ka, "fresh\n");
    assert_ne!(f.id(&ka), ida);
    let r = again.join(person("p"), Stay::Held).unwrap().conn;
    assert_eq!(again.read(r).unwrap().text_by_root[0].1, "fresh\n");
}

#[test]
fn delete_blob_doc_releases_its_blobs_for_gc() {
    let f = fx("delblob");
    let kb = key("ws", "b.bin");
    let file = f.dir.join("ws/b.bin");
    fs::write(&file, b"blob one").unwrap();
    let s = f
        .mgr
        .open_blob_file(Arc::new(AnyFile), &kb, &file, &f.dir.join("ws"))
        .unwrap();
    let w = s.join(person("w"), Stay::Held).unwrap().conn;
    s.write_bytes(w, b"blob two!", 1).unwrap();
    // a second blob doc sharing nothing, and one sharing blob one's bytes
    let kk = key("ws", "keep.bin");
    f.mgr
        .open_blob(Arc::new(AnyFile), &kk, Some(&mut &b"blob one"[..]))
        .unwrap();
    let idb = f.id(&kb);
    assert_eq!(
        f.store.tables_holding(idb).unwrap(),
        vec!["blob_sync", "blob_versions", "docs", "paths"]
    );
    let blobs = f.mgr.blob_store().unwrap().clone();
    assert_eq!(blobs.stats().unwrap().unreferenced_bytes, 0);

    f.mgr.delete_doc(&kb).unwrap();

    assert_eq!(f.store.tables_holding(idb).unwrap(), Vec::<&str>::new());
    assert!(matches!(s.read(w), Err(SessionError::Deleted)));
    assert!(f
        .host
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, AuditEvent::DocDeleted { key, .. } if *key == kb)));
    // "blob two!" lost its only reference; "blob one" is still keep.bin's
    let st = blobs.stats().unwrap();
    assert_eq!((st.blobs, st.unreferenced_bytes), (2, 9));
    assert_eq!(blobs.gc().unwrap().removed, 0); // within the grace
    f.advance(GRACE);
    let gc = blobs.gc().unwrap();
    assert_eq!((gc.removed, gc.bytes), (1, 9));
    let st = blobs.stats().unwrap();
    assert_eq!((st.blobs, st.unreferenced_bytes), (1, 0));
    let k = f.mgr.get_blob(&kk).unwrap();
    let r = k.join(person("r"), Stay::Held).unwrap().conn;
    let cur = k.read(r).unwrap();
    assert_eq!(
        blobs.read_to_vec("ws", &cur.info.blob, 100).unwrap(),
        b"blob one"
    );
}

#[test]
fn delete_space_erases_its_docs_only() {
    let f = fx("delspace");
    let (s, ka, p) = f.full_text_doc();
    let kb = key("ws", "sub/b.md");
    drop(f.text(&kb, "b\n"));
    let kc = key("ws", "c.bin");
    f.mgr
        .open_blob(Arc::new(AnyFile), &kc, Some(&mut &b"cc"[..]))
        .unwrap();
    // spaces whose names extend "ws" are other spaces
    let (kx, ky) = (key("other", "x.md"), key("ws2", "y.md"));
    let x = f.text(&kx, "x\n");
    drop(f.text(&ky, "y\n"));
    let ids: Vec<u64> = [&ka, &kb, &kc].iter().map(|k| f.id(k)).collect();
    let (idx, idy) = (f.id(&kx), f.id(&ky));
    let before = (
        f.store.tables_holding(idx).unwrap(),
        f.store.tables_holding(idy).unwrap(),
    );

    let mut gone = f.mgr.delete_space("ws").unwrap();
    gone.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(gone, vec![ka.clone(), kc.clone(), kb.clone()]);

    for id in ids {
        assert_eq!(f.store.tables_holding(id).unwrap(), Vec::<&str>::new());
    }
    assert_eq!(s.read(p).unwrap_err(), SessionError::Deleted);
    assert_eq!(f.mgr.loaded_keys(), vec![kx.clone(), ky.clone()]);
    assert_eq!(
        (
            f.store.tables_holding(idx).unwrap(),
            f.store.tables_holding(idy).unwrap()
        ),
        before
    );
    let xc = x.join(person("p"), Stay::Held).unwrap().conn;
    assert_eq!(x.read(xc).unwrap().text_by_root[0].1, "x\n");
    // the blob it held is released to GC; the space's key stays (purge is separate)
    let blobs = f.mgr.blob_store().unwrap().clone();
    f.advance(GRACE);
    assert_eq!(blobs.gc().unwrap().removed, 1);
    assert!(f.store.blob_space("ws").unwrap().is_some());
    assert!(f.mgr.delete_space("ws").unwrap().is_empty());
}
