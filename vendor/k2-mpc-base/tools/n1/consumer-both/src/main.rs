//! N−1 both ways (tools/n1/n1.sh `layout-both`, v0.10.1): between two builds that both read
//! format 6, each store the other build wrote opens here. text.redb and grid.redb come from
//! the other build's `n1_producer` with `K2MB_N1_RESTORE`: a write, a named version, a
//! revert, and a revert of that revert, so the store holds restored elements (provenance
//! rows), change clocks and a revert record with roots. This build opens both stores, blames
//! the restored line, reverts the other build's record, then writes, restores, blames and
//! rewrites. It prints `N-1 OK`.
use std::sync::Arc;

use k2_mpc_base::session::{
    AuditEvent, Config, Host, Participant, ParticipantKind, Rights, SessionManager, Stay,
    TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};

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
    for file in ["text.redb", "grid.redb"] {
        let store = Arc::new(DocStore::open(file).expect("the store opens"));
        println!("{file}: opened, format {}", store.format().unwrap());
        let id = store.find(&key).unwrap().expect("the doc");
        println!("{file}: versions {}", store.versions(id).unwrap().len());
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
        // the producer (job-patched): wrote "two", reverted it, then reverted that revert,
        // which put "two" back with provenance rows and a record with roots
        let r = s.read(c).unwrap();
        let (t, last) = (r.text_by_root[0].1.clone(), r.version);
        assert_eq!(t, "one\ntwo\n", "the text as the other build left it");
        let b0 = s.blame(c, TextFilePack::ROOT, None).unwrap();
        assert_eq!(b0.lines.len(), 2);
        println!(
            "{file}: read ok, blamed {} lines (line 2 restored_by {:?})",
            b0.lines.len(),
            b0.lines[1].restored_by
        );
        // revert the other build's restoring change, through its stored record's roots
        s.revert(c, last, false).unwrap();
        let t = s.read(c).unwrap().text_by_root.remove(0).1;
        assert_eq!(t, "one\n", "the stored restore reverted");
        println!("{file}: reverted change {last} (the other build's record)");
        let log = s.log(c, &Default::default()).unwrap();
        println!("{file}: log {} items", log.len());
        s.replace_text(c, TextFilePack::ROOT, "one\nthree\n").unwrap();
        let b1 = s.blame(c, TextFilePack::ROOT, None).unwrap();
        assert_eq!(b1.lines.len(), 2);
        let v = s.versions(c).unwrap();
        s.restore(c, v[0].id).unwrap();
        let b = s.blame(c, TextFilePack::ROOT, None).unwrap();
        println!("{file}: wrote, blamed, restored, blamed {} lines", b.lines.len());
        mgr.release().unwrap();
        drop((s, mgr));
        let r = store.rewrite_file().expect("no table this build doesn't know");
        println!("{file}: rewrite_file ok ({r:?})");
    }
    println!("N-1 OK");
}
