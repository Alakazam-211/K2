//! N−1 check (C1c, K-FMT-10 item 22), built against the previous release (k2-mpc-base
//! v0.9.2): a store the new build used for text docs only opens, reads, writes, reverts and
//! rewrites here; a store holding a grid doc (format 6) is refused with one line.
use std::sync::Arc;

use k2_mpc_base::session::{
    AuditEvent, Config, Host, Participant, ParticipantKind, Rights, SessionManager, Stay,
    TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore, StoreError};

struct H;
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        2_000_000
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn main() {
    let key = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    {
        let store = Arc::new(DocStore::open("text.redb").expect("v0.9.2 opens the text-only store"));
        let id = store.find(&key).unwrap().expect("the doc");
        println!("text.redb: opened; versions {}", store.versions(id).unwrap().len());
        let mgr = SessionManager::new(store.clone(), Arc::new(H), Config::default());
        let s = mgr
            .open(Arc::new(TextFilePack::new("markdown", 1 << 20)), &key, None)
            .expect("the doc opens");
        let c = s
            .join(
                Participant::new("p", "P", "#111111", ParticipantKind::Person, "cli"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        let t = s.read(c).unwrap().text_by_root.remove(0).1;
        assert_eq!(t, "one\n", "the text as the new build left it");
        let log = s.log(c, &Default::default()).unwrap();
        println!("text.redb: read ok, log {} items", log.len());
        s.replace_text(c, TextFilePack::ROOT, "one\nthree\n").unwrap();
        let v = s.versions(c).unwrap();
        s.restore(c, v[0].id).unwrap();
        let b = s.blame(c, TextFilePack::ROOT, None).unwrap();
        println!("text.redb: wrote, restored, blamed {} lines", b.lines.len());
        mgr.release().unwrap();
        drop((s, mgr));
        let r = store.rewrite_file().expect("no table this build doesn't know");
        println!("text.redb: rewrite_file ok ({r:?})");
    }
    match DocStore::open("grid.redb") {
        Err(e @ StoreError::Newer { found: 6 }) => {
            let line = e.to_string();
            assert!(!line.contains('\n'));
            println!("grid.redb: refused with one line: {line}");
        }
        Err(e) => panic!("grid.redb: refused, but not as newer: {e}"),
        Ok(_) => panic!("grid.redb: a format-6 store opened on v0.9.2"),
    }
    println!("N-1 OK");
}
