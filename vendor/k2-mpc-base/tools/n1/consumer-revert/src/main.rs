//! N−1 of a tier-0 revert's record (tools/n1/n1.sh `layout-revert`), built against the
//! previous release, whose `blob_verbs` rows are all record version 1. The store comes from
//! this repo's `n1_revert_producer`: one blob doc with the import, a version a verb made and a
//! revert of it (a record version 2). The previous release must read the versions with the
//! revert's as no verb (never another verb, never a failure), write one, and rewrite the store;
//! this repo's `n1_revert_after` then reads the revert's record back whole.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    AnyFile, AuditEvent, Config, Host, Participant, ParticipantKind, Rights, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};

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

fn main() {
    let dir = std::env::current_dir().unwrap();
    let now = Arc::new(AtomicU64::new(1_793_400_000_000));
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
    let key = DocKey {
        space: "ws".into(),
        path: "revert.idx".into(),
    };
    let who = || Participant::new("p", "P", "#111111", ParticipantKind::Person, "cli");
    let s = mgr.open_blob(Arc::new(AnyFile), &key, None).unwrap();
    let c = s.join(who(), Stay::Held).unwrap().conn;
    let verbs: Vec<Option<String>> = s.versions(c).unwrap().into_iter().map(|(_, v)| v.verb).collect();
    assert_eq!(
        verbs,
        vec![None, Some("part set".to_owned()), None],
        "the previous release reads the revert's record as no verb"
    );
    let head = s.versions(c).unwrap().last().unwrap().0;
    let w = s.write_bytes(c, b"IDX\n@n:1|k|a|-|1|-|-|older\n", head).unwrap();
    assert!(w.changed, "the previous release writes a version");
    println!("versions read (the revert's as no verb); a version written");
    mgr.release().unwrap();
    drop((s, mgr));
    store.check_rewritable().expect("the previous release knows every table");
    store.rewrite_file().expect("the previous release rewrites the store");
    println!("store rewritten");
    println!("N-1 REVERT (older side) OK");
}
