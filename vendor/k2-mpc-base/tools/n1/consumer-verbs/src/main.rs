//! N−1 of a file type's own verbs on a blob doc (tools/n1/n1.sh `layout-verbs`), built against
//! the previous release, which has no `blob_verbs` table. The store comes from this repo's
//! `n1_verbs_producer`: two blob docs, each with a version a verb made (its `blob_verbs` row).
//! The previous release must read the versions, write one, and delete a doc. Its rewrite: a
//! release without the table (v0.21.0 and before) refuses it, naming the table (NewerData); one
//! that knows the table (v0.22.0 and later) rewrites the store. Which it was goes in
//! `n1-rewrite.txt` beside the store. This repo's `n1_verbs_after` then reads the verb back,
//! finds the deleted doc's rows gone (swept at its open, or erased by the delete of a release
//! that knows the table), holds every row left byte for byte to its producer's, and rewrites.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    AnyFile, AuditEvent, Config, Host, Participant, ParticipantKind, Rights, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore, StoreError};

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

fn main() {
    let dir = std::env::current_dir().unwrap();
    let now = Arc::new(AtomicU64::new(1_793_300_000_000));
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let n = now.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || n.load(Ordering::SeqCst)),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(store.clone(), Arc::new(H(now.clone())), Config::default())
        .with_blob_store(blobs.clone());
    let who = || Participant::new("p", "P", "#111111", ParticipantKind::Person, "cli");
    // the versions read, the verb's among them, and a version written
    let s = mgr.open_blob(Arc::new(AnyFile), &key("keep.idx"), None).unwrap();
    let c = s.join(who(), Stay::Held).unwrap().conn;
    let vs = s.versions(c).unwrap();
    assert_eq!(vs.len(), 2, "keep.idx's two versions (the import and the verb's)");
    let head = vs.last().unwrap().0;
    let w = s.write_bytes(c, b"IDX\n@n:1|k|a|-|1|-|-|older\n", head).unwrap();
    assert!(w.changed, "the previous release writes a version");
    println!("versions read ({}); a version written", vs.len());
    mgr.release().unwrap();
    drop((s, mgr));
    // the rewrite: refused naming the table by a release that doesn't know it; done by one
    // that does (this build then holds the rows to its producer's, byte for byte)
    let outcome = match store.check_rewritable() {
        Err(StoreError::NewerData { table }) => {
            assert_eq!(table, "blob_verbs", "the rewrite names the table");
            println!("rewrite refused (NewerData: {table})");
            "refused"
        }
        Ok(()) => {
            store
                .rewrite_file()
                .expect("a release that knows blob_verbs rewrites the store");
            println!("store rewritten (this release knows blob_verbs)");
            "rewritten"
        }
        other => panic!("a rewrite of a store holding blob_verbs: {other:?}"),
    };
    std::fs::write(dir.join("n1-rewrite.txt"), outcome).unwrap();
    // a doc delete runs, leaving its verb rows to the newer build
    let gone = store.find(&key("gone.idx")).unwrap().expect("gone.idx");
    store.delete_doc(gone, now.load(Ordering::SeqCst)).unwrap();
    println!("doc delete ok");
    println!("N-1 VERBS (older side) OK");
}
