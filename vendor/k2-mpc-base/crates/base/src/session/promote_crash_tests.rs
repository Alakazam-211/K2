//! Tier 0 → text promotion (v0.9.2): a crash mid-promote, simulated inside the store's
//! transaction (crate-internal: a test-only fail point after every write, before the
//! commit; the dropped transaction is what a crash leaves), keeps the blob doc exactly as it
//! was, through a restart; the promotion then succeeds.

use std::fs;
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::blob::{BlobConfig, BlobStore};
use crate::store::{
    BlobReason, DocKey, DocStore, StoreError, BLOB_DOC_PACK, FAIL_PROMOTE_BEFORE_COMMIT,
};

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

fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: "notes.md".into(),
    }
}

fn pack() -> Arc<dyn Pack> {
    Arc::new(TextFilePack::new("markdown", 4 << 20))
}

fn manager(dir: &std::path::Path, store: &Arc<DocStore>, host: &Arc<H>) -> SessionManager {
    let h = host.clone();
    let blobs = BlobStore::open(
        store.clone(),
        dir.join("blobs"),
        BlobConfig::default(),
        Arc::new(move || h.0.load(Ordering::SeqCst)),
    )
    .unwrap();
    SessionManager::new(store.clone(), host.clone(), Config::default())
        .with_blob_store(Arc::new(blobs))
}

#[test]
fn a_crash_mid_promote_leaves_the_blob_doc_intact() {
    let dir = std::env::temp_dir().join(format!("k2mb-promote-crash-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("ws")).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    let (ws, file) = (dir.join("ws"), dir.join("ws/notes.md"));
    fs::write(&file, b"one\r\ntwo\n").unwrap();
    let host = Arc::new(H(AtomicU64::new(1_000)));
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let (id, before_versions, before_refs) = {
        let mgr = manager(&dir, &store, &host);
        // refused as text (mixed line endings): it lives as a blob doc
        let e = mgr.open_file(pack(), &key(), &file, &ws).err().unwrap();
        assert!(matches!(e, SessionError::Declined(ref d) if d.code == "mixed_line_endings"));
        let s = mgr
            .open_blob_file(Arc::new(AnyFile), &key(), &file, &ws)
            .unwrap();
        // fixed by a desktop app: an outside save, a second version
        fs::write(&file, b"one\ntwo\n").unwrap();
        assert!(matches!(
            s.file_changed().unwrap(),
            DiskCheck::Ingested { .. }
        ));
        let id = store.find(&key()).unwrap().unwrap();
        let versions = store.blob_versions(id).unwrap();
        assert_eq!(versions.len(), 2);
        let space = store.blob_space("ws").unwrap().unwrap();
        let refs: Vec<u64> = versions
            .iter()
            .map(|(_, v)| store.blob_row(space.no, &v.blob).unwrap().unwrap().refs)
            .collect();

        // the crash: every row written, then the transaction dies before its commit
        FAIL_PROMOTE_BEFORE_COMMIT.with(|f| f.set(true));
        let e = mgr
            .promote_blob_file(pack(), &key(), &file, &ws)
            .err()
            .unwrap();
        FAIL_PROMOTE_BEFORE_COMMIT.with(|f| f.set(false));
        assert!(
            matches!(e, SessionError::Store(StoreError::Corrupt(_))),
            "{e:?}"
        );

        // in this process: the blob session is still open and whole
        let c = s
            .join(
                Participant::new("p", "P", "#000", ParticipantKind::Person, "tab"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        assert_eq!(s.versions(c).unwrap(), versions);
        mgr.release().unwrap();
        (id, versions, refs)
    };

    // the restart: the store says what it said before the promotion, row for row
    drop(store);
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    assert_eq!(store.meta(id).unwrap().pack, BLOB_DOC_PACK);
    assert_eq!(store.blob_versions(id).unwrap(), before_versions);
    let space = store.blob_space("ws").unwrap().unwrap();
    let refs: Vec<u64> = before_versions
        .iter()
        .map(|(_, v)| store.blob_row(space.no, &v.blob).unwrap().unwrap().refs)
        .collect();
    assert_eq!(refs, before_refs, "no reference released");
    assert!(store.load(id).unwrap().updates.is_empty(), "no text log");
    assert!(store.versions(id).unwrap().is_empty(), "no text versions");
    assert_eq!(store.sync_states(id).unwrap(), (None, None), "no text sync");
    assert!(
        store.blob_sync_states(id).unwrap().0.is_some(),
        "blob sync kept"
    );

    let mgr = manager(&dir, &store, &host);
    let e = mgr.open_file(pack(), &key(), &file, &ws).err().unwrap();
    assert!(
        matches!(e, SessionError::PackMismatch { ref stored, .. } if stored == BLOB_DOC_PACK),
        "{e:?}"
    );
    let s = mgr
        .open_blob_file(Arc::new(AnyFile), &key(), &file, &ws)
        .unwrap();
    let c = s
        .join(
            Participant::new("p", "P", "#000", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let mut cur = Vec::new();
    s.read(c).unwrap().reader.read_to_end(&mut cur).unwrap();
    assert_eq!(cur, b"one\ntwo\n");
    let reasons: Vec<BlobReason> = s
        .versions(c)
        .unwrap()
        .into_iter()
        .map(|(_, v)| v.reason)
        .collect();
    assert_eq!(reasons, [BlobReason::Import, BlobReason::External]);
    s.leave(c).unwrap();
    drop(s);

    // and the promotion, retried, goes through
    let (t, done) = mgr.promote_blob_file(pack(), &key(), &file, &ws).unwrap();
    assert_eq!(done.changes, 1);
    let c = t
        .join(
            Participant::new("p", "P", "#000", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    assert_eq!(t.read(c).unwrap().text_by_root[0].1, "one\ntwo\n");
    assert_eq!(store.meta(id).unwrap().pack, "markdown");
    drop(t);
    mgr.release().unwrap();
    drop(mgr);
    drop(store);
    let _ = fs::remove_dir_all(&dir);
}
