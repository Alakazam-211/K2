//! A crash between a write-back's rename and recording it (crate-internal: it sets the
//! write-in-flight record directly).

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::doc::Doc;
use crate::hash::sha256;
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
/// reopened session's text and the file's bytes.
fn crash_during_write_back(tag: &str, landed: bool) -> (String, Vec<u8>, Arc<DocStore>) {
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
                    hash: sha256(&bytes).to_vec(),
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
    drop(s);
    mgr.release().unwrap();
    let _ = fs::remove_dir_all(&dir);
    (text, bytes, store)
}

#[test]
fn a_write_back_that_landed_is_recognised_not_merged_again() {
    // without the in-flight record the file would look like an outside edit adding "b"
    // to the old synced text, and merging it would give "abb"
    let (text, bytes, _) = crash_during_write_back("landed", true);
    assert_eq!(text, "ab\n");
    assert_eq!(bytes, b"ab\n");
}

#[test]
fn a_write_back_that_did_not_land_is_redone() {
    let (text, bytes, _) = crash_during_write_back("lost", false);
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
