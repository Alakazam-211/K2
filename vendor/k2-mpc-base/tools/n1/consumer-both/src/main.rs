//! N−1 both ways (tools/n1/n1.sh `layout-both`, v0.10.1): between two builds that both read
//! format 6, each store the other build wrote opens here. text.redb and grid.redb come from
//! the other build's `n1_producer` with `K2MB_N1_RESTORE`: a write, a named version, a
//! revert, and a revert of that revert, so the store holds restored elements (provenance
//! rows), change clocks and a revert record with roots. This build opens both stores, blames
//! the restored line, reverts the other build's record, then writes, restores, blames, has an
//! agent join and write, rewrites, and deletes the doc (the last step: the kit's own build then
//! reads what the delete left, `n1_kinds_after_delete`). It prints `N-1 OK`.
//!
//! After v0.10.2 the newer producer (`K2MB_N1_AGENTS`) also leaves `client_kinds` rows (an own
//! replica's and an agent's). A build that doesn't know the table opens, reads and writes the
//! store as any other, and refuses the rewrite before anything changes (`NewerData`, the
//! table named): reported, and the store opened again after it. Its delete isn't refused, and
//! it can't erase rows of a table it doesn't know.
//!
//! v0.14.0's producer (`K2MB_N1_PERSONS`) also leaves person-device writers (`client_kinds`
//! kind 2), listed in `persons-<store>`: a build of either side never hands one of a.md's to
//! its own joins here (a person's, an agent's), and its rewrite keeps them (the kit's
//! `n1_persons_after` takes keep.md's back after this).
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
    for file in ["text.redb", "grid.redb"] {
        let store = Arc::new(DocStore::open(file).expect("the store opens"));
        println!("{file}: opened, format {}", store.format().unwrap());
        let id = store.find(&key).unwrap().expect("the doc");
        println!("{file}: versions {}", store.versions(id).unwrap().len());
        let mgr = SessionManager::new(store.clone(), Arc::new(H), Config::default());
        let s = mgr
            .open(Arc::new(TextFilePack::new("markdown", 1 << 20)), &key, None)
            .expect("the doc opens");
        let pj = s
            .join(
                Participant::new("p", "P", "#111111", ParticipantKind::Person, "cli"),
                Stay::Held,
            )
            .unwrap();
        let c = pj.conn;
        // the producer's person-device writers of a.md (v0.14.0), if it wrote any
        let persons: Vec<u64> = std::fs::read_to_string(format!("persons-{file}"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("a.md "))
            .map(|l| l.split(' ').nth(2).unwrap().parse().unwrap())
            .collect();
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
        let j = s
            .join(
                Participant::new("bot", "BOT", "#222222", ParticipantKind::Agent, "cli"),
                Stay::Held,
            )
            .unwrap();
        s.replace_text(j.conn, TextFilePack::ROOT, "one\nthree\nfour\n")
            .unwrap();
        println!("{file}: agent bot joined as client {} and wrote", j.client);
        assert!(
            !persons.contains(&pj.client) && !persons.contains(&j.client),
            "{file}: a person-device writer was handed out ({} or {} in {persons:?})",
            pj.client,
            j.client
        );
        if !persons.is_empty() {
            println!("{file}: none of a.md's {} person-device writers handed out", persons.len());
            // the changes those writers landed (keep.md: one each, released and parked) read
            // here exactly, attributed to their person
            let keep = DocKey {
                space: "ws".into(),
                path: "keep.md".into(),
            };
            let k = mgr
                .open(Arc::new(TextFilePack::new("markdown", 1 << 20)), &keep, None)
                .expect("keep.md opens");
            let kc = k
                .join(
                    Participant::new("q", "Q", "#333333", ParticipantKind::Person, "cli"),
                    Stay::Held,
                )
                .unwrap()
                .conn;
            let kt = k.read(kc).unwrap().text_by_root.remove(0).1;
            assert_eq!(
                kt, "keep\nfree\nparked\n",
                "{file}: keep.md as the other build's person-device writers left it"
            );
            let kb = k.blame(kc, TextFilePack::ROOT, None).unwrap();
            let authors: Vec<&str> = kb.lines.iter().map(|l| l.author.as_str()).collect();
            assert_eq!(
                &authors[1..],
                ["p", "p"],
                "{file}: keep.md's person-device lines are the person's"
            );
            println!("{file}: keep.md read as written under person-device writers, blamed to p");
        }
        mgr.release().unwrap();
        drop((s, mgr));
        let check = store.check_rewritable();
        println!("{file}: check_rewritable: {check:?}");
        match check {
            // a newer kit's table (client_kinds, after v0.10.2) this build doesn't know
            Err(StoreError::NewerData { table }) if table == "client_kinds" => {
                println!("{file}: rewrite refused before any change (table {table:?})");
            }
            other => {
                other.expect("rewritable");
                let r = store.rewrite_file().expect("no table this build doesn't know");
                println!("{file}: rewrite_file ok ({r:?})");
            }
        }
        drop(store);
        let store = DocStore::open(file).expect("the store opens again");
        let id = store.find(&key).unwrap().expect("the doc");
        println!(
            "{file}: opened again, versions {}",
            store.versions(id).unwrap().len()
        );
        let deleted = store.delete_doc(id, 3_000_000);
        println!(
            "{file}: delete_doc: {}; doc found after it: {}",
            if deleted.is_ok() { "ok" } else { "refused" },
            store.find(&key).unwrap().is_some()
        );
        deleted.expect("delete_doc");
    }
    println!("N-1 OK");
}
