//! N−1 of the version an app opened (tools/n1/n1.sh `layout-opened`), built against the previous
//! release, which knows nothing of it. The store comes from this repo's `n1_opened_producer`:
//! a blob doc and two live docs, each with its pin in slot 3 of its sync rows. The previous
//! release must take the store as any other: open both kinds of doc and write to one, check and
//! run a rewrite (a purge's), and delete a doc; this repo's `n1_opened_after` then reads the
//! pins that should be left.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    AnyFile, AuditEvent, Config, Host, Participant, ParticipantKind, Rights, SessionManager, Stay,
    TextFilePack,
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

fn key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

fn main() {
    let dir = std::env::current_dir().unwrap();
    let ws = std::fs::canonicalize(dir.join("ws")).unwrap();
    let now = Arc::new(AtomicU64::new(1_793_200_000_000));
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
    // both kinds of doc open, and a live doc takes a write
    let b = mgr
        .open_blob_file(Arc::new(AnyFile), &key("paper.pdf"), &ws.join("paper.pdf"), &ws)
        .unwrap();
    b.join(who(), Stay::Held).unwrap();
    let md = TextFilePack::new("md", 1 << 20);
    let s = mgr
        .open_file(Arc::new(md), &key("keep.md"), &ws.join("keep.md"), &ws)
        .unwrap();
    let c = s.join(who(), Stay::Held).unwrap().conn;
    let o = s
        .run_tool(c, TextFilePack::ROOT, &["append".to_owned()], b"three\n")
        .unwrap();
    assert_eq!(o.exit, 0, "a write on a live doc holding a pin");
    s.flush(c).unwrap();
    println!("both doc kinds open; a live doc written");
    mgr.release().unwrap();
    drop((b, s, mgr));
    // a rewrite (a purge's) takes the store, and a doc delete runs
    store.check_rewritable().expect("the store is rewritable");
    store.rewrite_file().expect("the store rewrites");
    println!("rewrite ok (the pins' slot is in the sync tables)");
    let gone = store.find(&key("gone.md")).unwrap().expect("gone.md");
    store.delete_doc(gone, now.load(Ordering::SeqCst)).unwrap();
    println!("doc delete ok");
    println!("N-1 OPENED (older side) OK");
}
