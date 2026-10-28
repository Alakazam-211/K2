//! Tier 0: a crash between a write-back's rename and recording it (crate-internal: it sets
//! the write-in-flight record directly), the same pattern as the text sessions' C4 tests.

use std::fs;
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::blob::{BlobConfig, BlobStore};
use crate::store::{BlobReason, BlobSync, BlobVersion, DocKey, DocStore};

struct H(AtomicU64);
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: "a.pdf".into(),
    }
}

fn manager(dir: &std::path::Path, store: &Arc<DocStore>, host: &Arc<H>) -> SessionManager {
    let h = host.clone();
    let blobs = BlobStore::open(
        store.clone(),
        dir.join("blobs"),
        BlobConfig::default(),
        Arc::new(move || h.now_ms()),
    )
    .unwrap();
    SessionManager::new(store.clone(), host.clone(), Config::default())
        .with_blob_store(Arc::new(blobs))
}

/// A participant's version is committed, then "crash" during its write-back: the
/// write-in-flight record is stored and (when `landed`) the file renamed into place, but it
/// is never recorded. Returns the reopened doc's versions and the file's bytes.
fn crash_during_write_back(tag: &str, landed: bool) -> (Vec<BlobReason>, Vec<u8>, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("k2mb-t0crash-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("ws")).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    let (ws, file) = (dir.join("ws"), dir.join("ws/a.pdf"));
    fs::write(&file, b"v1").unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let host = Arc::new(H(AtomicU64::new(1_000)));
    {
        let mgr = manager(&dir, &store, &host);
        let s = mgr
            .open_blob_file(Arc::new(AnyFile), &key(), &file, &ws)
            .unwrap();
        let blobs = mgr.blob_store().unwrap();
        let info = blobs.put_bytes("ws", b"v2").unwrap();
        let id = store.find(&key()).unwrap().unwrap();
        let v = BlobVersion::new(
            info.id,
            info.size,
            2_000,
            "agent",
            "A",
            "cli",
            BlobReason::Write,
        );
        store.commit_blob_versions(id, "ws", 1, &[v], None).unwrap();
        store
            .set_blob_pending(
                id,
                &BlobSync {
                    blob: info.id,
                    stat: None,
                },
            )
            .unwrap();
        if landed {
            fs::write(&file, b"v2").unwrap();
        }
        // the process dies: nothing is recorded
        drop(s);
    }
    let mgr = manager(&dir, &store, &host);
    let s = mgr
        .open_blob_file(Arc::new(AnyFile), &key(), &file, &ws)
        .unwrap();
    let c = s
        .join(
            Participant::new("agent", "A", "#000", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    s.flush(c).unwrap();
    let mut cur = Vec::new();
    s.read(c).unwrap().reader.read_to_end(&mut cur).unwrap();
    let reasons = s
        .versions(c)
        .unwrap()
        .into_iter()
        .map(|(_, v)| v.reason)
        .collect();
    let id = store.find(&key()).unwrap().unwrap();
    assert_eq!(
        store.blob_sync_states(id).unwrap().1,
        None,
        "nothing left in flight"
    );
    let bytes = fs::read(&file).unwrap();
    drop(s);
    mgr.release().unwrap();
    let _ = fs::remove_dir_all(&dir);
    (reasons, cur, bytes)
}

#[test]
fn a_blob_write_back_that_landed_is_recognised_not_taken_as_an_outside_save() {
    // without the in-flight record the file would look like an outside save of v2 (and,
    // the doc being ahead of the file, a conflict with a copy)
    let (reasons, cur, bytes) = crash_during_write_back("landed", true);
    assert_eq!(reasons, [BlobReason::Import, BlobReason::Write]);
    assert_eq!(cur, b"v2");
    assert_eq!(bytes, b"v2");
}

#[test]
fn a_blob_write_back_that_did_not_land_is_redone() {
    let (reasons, cur, bytes) = crash_during_write_back("lost", false);
    assert_eq!(reasons, [BlobReason::Import, BlobReason::Write]);
    assert_eq!(cur, b"v2");
    assert_eq!(bytes, b"v2", "the reopened session wrote it");
}
