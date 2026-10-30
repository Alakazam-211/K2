//! K9 (v0.9.8, K-DISK-14): a file with more than one name (a hard link) isn't opened live,
//! as a text doc or a tier-0 blob doc: `Declined` `hard_linked`, `MPC-E412` (exit 4), the
//! message naming no path. A host keys sessions by `FileIdentity` (device and inode) as well
//! as the path: two names of one file have one identity, and an open session reports its
//! file's. Public API only; Unix (where files have device and inode numbers).
#![cfg(unix)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    file_identity, AnyFile, AuditEvent, Config, Host, Participant, Rights, SessionError,
    SessionManager, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};

struct H(AtomicU64);
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct Dir(std::path::PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

#[test]
fn hard_linked_files_are_declined_and_identities_name_files() {
    let dir = std::env::temp_dir().join(format!("k2mb-hardlinks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let _d = Dir(dir.clone());
    let ws = std::fs::canonicalize(ws).unwrap();
    let host = Arc::new(H(AtomicU64::new(1)));
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(store, host, Config::default()).with_blob_store(blobs);

    // two names of one file
    let one = ws.join("a.md");
    let two = ws.join("b.md");
    std::fs::write(&one, "x\n").unwrap();
    std::fs::hard_link(&one, &two).unwrap();
    let declined = |e: SessionError| {
        let r = e.report();
        assert_eq!((r.id(), r.exit()), ("MPC-E412", 4), "{}", r.to_json());
        assert!(
            r.to_json().contains("\"code\":\"hard_linked\""),
            "{}",
            r.to_json()
        );
        assert!(
            !r.to_json().contains(&*dir.to_string_lossy()),
            "{}",
            r.to_json()
        );
    };
    let text = mgr.open_file(
        Arc::new(TextFilePack::new("markdown", 1 << 20)),
        &key("a.md"),
        &one,
        &ws,
    );
    declined(text.err().expect("a hard-linked file isn't opened live"));
    let blob = mgr.open_blob_file(Arc::new(AnyFile), &key("b.md"), &two, &ws);
    declined(blob.err().expect("nor as a blob doc"));
    // one file, one identity
    let (i1, i2) = (
        file_identity(&one).unwrap().unwrap(),
        file_identity(&two).unwrap().unwrap(),
    );
    assert_eq!(i1, i2);

    // with the extra name gone, it opens; the session reports its file's identity, and a
    // copy is another file
    std::fs::remove_file(&two).unwrap();
    let s = mgr
        .open_file(
            Arc::new(TextFilePack::new("markdown", 1 << 20)),
            &key("a.md"),
            &one,
            &ws,
        )
        .unwrap();
    assert_eq!(s.file_identity().unwrap(), Some(i1));
    let copy = ws.join("c.md");
    std::fs::copy(&one, &copy).unwrap();
    assert_ne!(file_identity(&copy).unwrap().unwrap(), i1);
    let b = mgr
        .open_blob_file(Arc::new(AnyFile), &key("c.md"), &copy, &ws)
        .unwrap();
    assert_eq!(b.file_identity().unwrap(), file_identity(&copy).unwrap());
}
