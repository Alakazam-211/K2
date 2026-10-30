//! A crash between a write-back's rename and recording it (crate-internal: it sets the
//! write-in-flight record directly).

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::doc::Doc;
use crate::hash::{file_hash, sha256};
use crate::store::{DocKey, DocStore, SyncState};

struct H(AtomicU64);
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::editor()
    }
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);

fn setup(tag: &str) -> (std::path::PathBuf, Arc<DocStore>, Arc<H>) {
    let dir = std::env::temp_dir().join(format!("k2mb-crash-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("ws")).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    (dir, store, Arc::new(H(AtomicU64::new(1_000))))
}

fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    }
}

/// Edit, then "crash" during a write-back: the write-in-flight record is stored and (when
/// `landed`) the file renamed into place, but the promotion never happens. Returns the
/// reopened session's text and the file's bytes. `legacy`: the record holds a format-2
/// plain SHA-256, as a store written before format 3 does.
fn crash_during_write_back(
    tag: &str,
    landed: bool,
    legacy: bool,
) -> (String, Vec<u8>, Arc<DocStore>) {
    let (dir, store, host) = setup(tag);
    let (ws, file) = (dir.join("ws"), dir.join("ws/a.md"));
    fs::write(&file, b"a\n").unwrap();
    {
        let mgr = SessionManager::new(store.clone(), host.clone(), Config::default());
        let s = mgr.open_file(Arc::new(MD), &key(), &file, &ws).unwrap();
        let p = Participant::new("agent", "A", "#000", ParticipantKind::Agent, "cli");
        let c = s.join(p, Stay::Held).unwrap().conn;
        s.replace_text(c, TextFilePack::ROOT, "ab\n").unwrap();
        let bytes = s.read(c).unwrap().bytes;
        let id = store.find(&key()).unwrap().unwrap();
        let live = Doc::from_store(MD.schema(), &store.load(id).unwrap()).unwrap();
        store
            .set_pending_sync(
                id,
                &SyncState {
                    hash: if legacy {
                        sha256(&bytes).to_vec()
                    } else {
                        file_hash(&store.doc_key(id).unwrap(), &bytes)
                    },
                    seq: s.status().unwrap().version,
                    state: live.encode_state().unwrap(),
                },
            )
            .unwrap();
        if landed {
            fs::write(&file, &bytes).unwrap();
        }
        // the process dies: nothing is flushed or promoted
    }
    let mgr = SessionManager::new(store.clone(), host, Config::default());
    let s = mgr.open_file(Arc::new(MD), &key(), &file, &ws).unwrap();
    let p = Participant::new("agent", "A", "#000", ParticipantKind::Agent, "cli");
    let c = s.join(p, Stay::Held).unwrap().conn;
    let text = s.read(c).unwrap().text_by_root.remove(0).1;
    s.flush(c).unwrap();
    let bytes = fs::read(&file).unwrap();
    let id = store.find(&key()).unwrap().unwrap();
    assert_eq!(
        store.sync_states(id).unwrap().1,
        None,
        "nothing left in flight"
    );
    // no plain content hash is left in the store (format 3: keyed, 33 bytes)
    let synced = store.sync_states(id).unwrap().0.unwrap().hash;
    assert_eq!(synced.len(), 33);
    assert_eq!(store.meta(id).unwrap().synced_hash, Some(synced));
    drop(s);
    mgr.release().unwrap();
    let _ = fs::remove_dir_all(&dir);
    (text, bytes, store)
}

#[test]
fn a_write_back_that_landed_is_recognised_not_merged_again() {
    // without the in-flight record the file would look like an outside edit adding "b"
    // to the old synced text, and merging it would give "abb"
    let (text, bytes, _) = crash_during_write_back("landed", true, false);
    assert_eq!(text, "ab\n");
    assert_eq!(bytes, b"ab\n");
}

/// A format-2 store's write in flight (a plain SHA-256) is still recognised, then re-keyed.
#[test]
fn a_format_2_write_in_flight_is_recognised_and_rekeyed() {
    let (text, bytes, _) = crash_during_write_back("landed-legacy", true, true);
    assert_eq!(text, "ab\n", "not merged again");
    assert_eq!(bytes, b"ab\n");
}

/// A format-2 doc's synced hash is a plain SHA-256: on the next open it still names the
/// file (no outside edit is seen), and it is replaced by a keyed one at once.
#[test]
fn a_format_2_synced_hash_is_read_once_then_rekeyed() {
    let (dir, store, host) = setup("legacy-synced");
    let (ws, file) = (dir.join("ws"), dir.join("ws/a.md"));
    fs::write(&file, b"a\n").unwrap();
    let p = || Participant::new("agent", "A", "#000", ParticipantKind::Agent, "cli");
    let id = {
        let mgr = SessionManager::new(store.clone(), host.clone(), Config::default());
        let s = mgr.open_file(Arc::new(MD), &key(), &file, &ws).unwrap();
        let c = s.join(p(), Stay::Held).unwrap().conn;
        s.replace_text(c, TextFilePack::ROOT, "ab\n").unwrap();
        s.flush(c).unwrap();
        let id = store.find(&key()).unwrap().unwrap();
        // as a format-2 store holds it
        let mut meta = store.meta(id).unwrap();
        let mut synced = store.sync_states(id).unwrap().0.unwrap();
        synced.hash = sha256(b"ab\n").to_vec();
        meta.synced_hash = Some(synced.hash.clone());
        store.rekey_sync(id, &meta, &synced).unwrap();
        drop(s);
        mgr.release().unwrap();
        id
    };
    let versions = store.version_infos(id).unwrap().len();
    let mgr = SessionManager::new(store.clone(), host, Config::default());
    let s = mgr.open_file(Arc::new(MD), &key(), &file, &ws).unwrap();
    let c = s.join(p(), Stay::Held).unwrap().conn;
    assert_eq!(s.read(c).unwrap().text_by_root[0].1, "ab\n");
    assert_eq!(
        store.version_infos(id).unwrap().len(),
        versions,
        "no outside edit was seen"
    );
    let synced = store.sync_states(id).unwrap().0.unwrap().hash;
    assert_eq!(synced.len(), 33, "re-keyed");
    assert_eq!(store.meta(id).unwrap().synced_hash, Some(synced));
    assert_eq!(s.check_disk().unwrap(), DiskCheck::Unchanged);
    drop(s);
    mgr.release().unwrap();
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_write_back_that_did_not_land_is_redone() {
    let (text, bytes, _) = crash_during_write_back("lost", false, false);
    assert_eq!(text, "ab\n");
    assert_eq!(bytes, b"ab\n", "the reopened session wrote it");
}

/// Part 3: an update left pending poisons the live doc. The session never encodes or
/// compacts from it: the next edit, compaction or write-back reloads from the store
/// (`AuditEvent::Recovered`), and the session carries on.
#[test]
fn a_poisoned_doc_is_reloaded_never_encoded() {
    use crate::doc::DocError;
    struct Ev(AtomicU64, std::sync::Mutex<Vec<AuditEvent>>);
    impl Host for Ev {
        fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
            Rights::editor()
        }
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
        fn on_event(&self, e: &AuditEvent) {
            self.1.lock().unwrap().push(e.clone());
        }
    }
    let (dir, store, _) = setup("poison");
    let host = Arc::new(Ev(AtomicU64::new(5), Default::default()));
    let recovered = |h: &Ev| {
        h.1.lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, AuditEvent::Recovered { .. }))
            .count()
    };
    let (ws, file) = (dir.join("ws"), dir.join("ws/a.md"));
    fs::write(&file, b"a\n").unwrap();
    let cfg = Config {
        unload_compact_entries: 1, // compact on unload
        ..Config::default()
    };
    let mgr = SessionManager::new(store.clone(), host.clone(), cfg);
    let s = mgr.open_file(Arc::new(MD), &key(), &file, &ws).unwrap();
    let p = Participant::new("agent", "A", "#000", ParticipantKind::Agent, "cli");
    let c = s.join(p, Stay::Held).unwrap().conn;
    let pending = || {
        let mut other = Doc::new(MD.schema()).unwrap();
        other.replace_text_as(4242, "source", "p\n").unwrap();
        other
            .replace_text_as(4242, "source", "p\nq\n")
            .unwrap()
            .unwrap()
    };
    // 1. an edit on a poisoned doc: refused, reloaded, then edits work
    assert_eq!(
        s.apply_raw_for_tests(&pending()),
        Err(SessionError::Doc(DocError::Pending))
    );
    assert_eq!(
        s.replace_text(c, TextFilePack::ROOT, "b\n"),
        Err(SessionError::Doc(DocError::Pending))
    );
    assert_eq!(recovered(&host), 1);
    assert_eq!(s.read(c).unwrap().text_by_root[0].1, "a\n");
    s.replace_text(c, TextFilePack::ROOT, "b\n").unwrap();
    // 2. a write-back from a poisoned doc: reloads instead of encoding
    s.apply_raw_for_tests(&pending()).unwrap_err();
    assert!(s.flush(c).is_err());
    assert_eq!(recovered(&host), 2);
    assert!(s.flush(c).unwrap().written);
    assert_eq!(fs::read(&file).unwrap(), b"b\n");
    // 3. compaction on unload from a poisoned doc: never encoded (reloaded), no panic
    s.apply_raw_for_tests(&pending()).unwrap_err();
    s.leave(c).unwrap();
    drop(s);
    host.0
        .fetch_add(Config::default().idle_unload_ms, Ordering::SeqCst);
    assert_eq!(mgr.unload_idle().unwrap(), 1);
    assert_eq!(recovered(&host), 3);
    assert!(host.1.lock().unwrap().iter().any(|e| matches!(
        e,
        AuditEvent::CompactFailed { error, .. } if error.contains("unresolved")
    )));
    mgr.release().unwrap();
    let mgr = SessionManager::new(store.clone(), host.clone(), Config::default());
    let s = mgr.open_file(Arc::new(MD), &key(), &file, &ws).unwrap();
    let c = s
        .join(
            Participant::new("agent", "A", "#000", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    assert_eq!(s.read(c).unwrap().text_by_root[0].1, "b\n");
    drop(s);
    mgr.release().unwrap();
    let _ = fs::remove_dir_all(&dir);
}

/// v0.9.8 (the disk replica decoded on first need): a stored sync state that doesn't decode
/// is no longer found at the open but at the first disk need after it. It must be as loud
/// and as safe there: the open's error (the same variant, the same error id) from every
/// write-back and outside save, and nothing written to the file or the store (no write in
/// flight, the synced row as it was, no change logged by the disk path).
#[test]
fn a_damaged_sync_state_fails_the_first_write_back_as_the_open_did() {
    let (dir, store, host) = setup("damaged-sync");
    let (ws, file) = (dir.join("ws"), dir.join("ws/a.md"));
    fs::write(&file, b"a\n").unwrap();
    let p = || Participant::new("agent", "A", "#000", ParticipantKind::Agent, "cli");
    let id = {
        let mgr = SessionManager::new(store.clone(), host.clone(), Config::default());
        let s = mgr.open_file(Arc::new(MD), &key(), &file, &ws).unwrap();
        let c = s.join(p(), Stay::Held).unwrap().conn;
        s.replace_text(c, TextFilePack::ROOT, "ab\n").unwrap();
        s.flush(c).unwrap();
        drop(s);
        mgr.release().unwrap();
        store.find(&key()).unwrap().unwrap()
    };
    // the synced row damaged through the store's API: the file's hash kept (so the open
    // sees no outside edit), its engine state not one
    let mut synced = store.sync_states(id).unwrap().0.unwrap();
    synced.state = vec![0xff; 16];
    let meta = store.meta(id).unwrap();
    store.rekey_sync(id, &meta, &synced).unwrap();
    // what the open gave before v0.9.8: the kit's guarded decoder refusing these bytes
    let want = SessionError::from(
        Doc::load(MD.schema(), &synced.state)
            .err()
            .expect("these bytes don't decode"),
    );
    let rows = || (store.sync_states(id).unwrap(), store.log_size(id).unwrap());
    let mgr = SessionManager::new(store.clone(), host.clone(), Config::default());
    let s = mgr.open_file(Arc::new(MD), &key(), &file, &ws).unwrap();
    let c = s.join(p(), Stay::Held).unwrap().conn;
    // an edit (the doc itself is sound), then the write-back
    s.replace_text(c, TextFilePack::ROOT, "abc\n").unwrap();
    let before = rows();
    for _ in 0..2 {
        let e = s.flush(c).unwrap_err();
        assert_eq!(e, want, "the open's error");
        assert_eq!(e.report().id(), want.report().id(), "the same error id");
        assert_eq!(fs::read(&file).unwrap(), b"ab\n", "the file untouched");
        assert_eq!(rows(), before, "nothing written to the store");
        assert_eq!(rows().0 .1, None, "no write in flight");
    }
    // the host's tick reports it, and writes nothing either
    host.0.fetch_add(10_000, Ordering::SeqCst);
    let rep = mgr.tick().unwrap();
    assert!(
        rep.errors.iter().any(|(_, e)| *e == want),
        "the tick reports it: {:?}",
        rep.errors
    );
    assert_eq!(fs::read(&file).unwrap(), b"ab\n");
    assert_eq!(rows(), before);
    // an outside save: refused the same way, nothing merged or logged
    fs::write(&file, b"ab\nz\n").unwrap();
    let e = s.file_changed().unwrap_err();
    assert_eq!(e, want);
    assert_eq!(rows(), before);
    assert_eq!(s.read(c).unwrap().text_by_root[0].1, "abc\n");
    drop(s);
    drop(mgr);
    let _ = fs::remove_dir_all(&dir);
}
