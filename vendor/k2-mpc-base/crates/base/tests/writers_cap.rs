//! The writers cap (`Config::writers_max`): a doc holds at most that many writers (client IDs
//! its state holds or may come to hold), and every writer stays in the doc for good.
//!
//! The rows:
//! - `cap_mints_until_max_then_read_only`: past the cap a new connection reads only; its edit
//!   is refused `MPC-E908`, and the doc's writers stay at the cap;
//! - `cap_takes_the_persons_free_then_oldest_parked`: at the cap a person's join takes that
//!   person's free writer of another device, else the oldest parked one (let go early: its
//!   reclaim keeps nothing and is told when its lease ended), else reads only;
//! - `cap_exempts_the_outside_save_and_the_import`: the import and an outside save land at the
//!   cap (and count);
//! - `cap_counts_a_rights_gain`: a read-only connection that gains edit rights at the cap
//!   stays read-only;
//! - `cap_new_agent_pid_read_only`: an agent reuses its own writer at the cap; a new agent
//!   reads only;
//! - `cap_a_watcher_holds_no_counted_writer`: a connection watching past `participants_max`
//!   can't write, so its writer isn't counted and gets no owner row, whatever its rights;
//! - `cap_a_promoted_watcher_takes_its_writer_then`: a watcher let in takes its writer then,
//!   under the cap; one let in past the cap reads only and its edit is refused `MPC-E908`;
//! - `cap_promotion_past_it_is_allowed_and_counted`: a tier-0 promotion with more authors than
//!   the cap lands, and the doc then holds them all;
//! - `near_cap_notice_counts_against_8192_without_a_cap`: with no cap, the near-cap notice
//!   comes once, at 80% of `WRITERS_NOTICE_MAX`.
//!
//! Planted faults (`MPC_STRUCT_MUTANT`, the setting echoed), each caught at a row assert:
//! `cap-unchecked` (`ClientMutant::CapUnchecked`), `cap-takes-newest-parked`,
//! `cap-on-external`, `cap-join-only`, `cap-on-promotion`, `no-near-cap-notice`,
//! `watcher-counts-writer`.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    AnyFile, AuditEvent, Config, ConnId, DiskCheck, Host, JoinOptions, Joined, Participant,
    ParticipantKind, Rights, Session, SessionManager, Stay, TextFilePack, WRITERS_NOTICE_MAX,
};
use k2_mpc_base::store::{DocId, DocKey, DocStore};
use k2_mpc_base::ClientId;

fn under_mutant<R>(f: impl FnOnce() -> R) -> R {
    use k2_mpc_base::differential::{with_client_mutant, ClientMutant};
    let m = std::env::var("MPC_STRUCT_MUTANT");
    println!("MPC_STRUCT_MUTANT={}", m.as_deref().unwrap_or("(unset)"));
    match m.as_deref() {
        Ok("cap-unchecked") => with_client_mutant(ClientMutant::CapUnchecked, f),
        Ok("cap-takes-newest-parked") => with_client_mutant(ClientMutant::CapTakesNewestParked, f),
        Ok("cap-on-external") => with_client_mutant(ClientMutant::CapOnExternal, f),
        Ok("cap-join-only") => with_client_mutant(ClientMutant::CapJoinOnly, f),
        Ok("cap-on-promotion") => with_client_mutant(ClientMutant::CapOnPromotion, f),
        Ok("no-near-cap-notice") => with_client_mutant(ClientMutant::NoNearCapNotice, f),
        Ok("watcher-counts-writer") => with_client_mutant(ClientMutant::WatcherCountsWriter, f),
        Ok(other) => panic!("MPC_STRUCT_MUTANT={other}: not a mutant this suite plants"),
        Err(_) => f(),
    }
}

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;
const START: &str = "# Title\n\nbody\n";

struct H {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for H {
    fn rights(&self, p: &Participant, _: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&p.pid)
            .copied()
            .unwrap_or(Rights::editor())
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, e: &AuditEvent) {
        self.events.lock().unwrap().push(e.clone());
    }
}

struct Fx {
    dir: PathBuf,
    ws: PathBuf,
    host: Arc<H>,
    store: Arc<DocStore>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(name: &str, tweak: impl FnOnce(&mut Config)) -> Fx {
    let dir = std::env::temp_dir().join(format!("k2mpc-cap-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let dir = std::fs::canonicalize(dir).unwrap();
    let ws = dir.join("ws");
    let host = Arc::new(H {
        now: AtomicU64::new(1_000),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("d.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now.load(Ordering::SeqCst)),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1 << 30;
    c.edit_burst = 1 << 30;
    c.person_writer_park_ms = 0;
    tweak(&mut c);
    let mgr = SessionManager::new(store.clone(), host.clone(), c).with_blob_store(blobs);
    Fx {
        dir,
        ws,
        host,
        store,
        mgr,
    }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

impl Fx {
    fn s(&self) -> Arc<Session> {
        self.mgr
            .open(Arc::new(MD), &key("a.md"), Some(START.as_bytes()))
            .unwrap()
    }
    fn grant(&self, pid: &str, r: Rights) {
        self.host.rights.lock().unwrap().insert(pid.into(), r);
    }
    fn doc_id(&self, k: &DocKey) -> DocId {
        self.store.find(k).unwrap().expect("the doc is stored")
    }
    fn near_cap(&self) -> Vec<(u64, u64)> {
        self.host
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AuditEvent::WritersNearCap { count, max, .. } => Some((*count, *max)),
                _ => None,
            })
            .collect()
    }
}

fn person(pid: &str) -> Participant {
    Participant::new(pid, pid, "#7c3aed", ParticipantKind::Person, "tab")
}

fn agent(pid: &str) -> Participant {
    Participant::new(pid, pid, "#0f766e", ParticipantKind::Agent, "cli")
}

fn join(s: &Session, p: Participant) -> Joined {
    s.join(p, Stay::Held).unwrap()
}

fn tab(s: &Session, pid: &str, dev: &str) -> Joined {
    s.join_with(person(pid), Stay::Held, &JoinOptions::new().device(dev))
        .unwrap()
}

fn text(s: &Session, conn: ConnId) -> String {
    let r = s.read(conn).unwrap();
    r.text_by_root
        .into_iter()
        .find(|(x, _)| x == ROOT)
        .unwrap()
        .1
}

/// Append `more` as `conn`.
fn type_more(s: &Session, conn: ConnId, more: &str) -> Result<u64, String> {
    let t = text(s, conn);
    s.replace_text(conn, ROOT, &format!("{t}{more}"))
        .map(|r| r.version)
        .map_err(|e| e.report().id().to_owned())
}

/// The writers the doc's log holds changes of (the import's included).
fn log_writers(store: &DocStore, id: DocId) -> BTreeSet<ClientId> {
    store
        .load(id)
        .unwrap()
        .updates
        .iter()
        .map(|(_, e)| e.client)
        .collect()
}

// ---------------------------------------------------------------------------------------------

/// The import counts one; three people write; a fourth reads only and its edit is refused.
#[test]
fn cap_mints_until_max_then_read_only() {
    under_mutant(|| {
        let f = fx("c1", |c| c.writers_max = 4);
        let s = f.s();
        let mut first = None;
        for p in ["p1", "p2", "p3"] {
            let j = join(&s, person(p));
            assert!(!j.writers_full, "{p} joins under the cap");
            type_more(&s, j.conn, p).unwrap();
            first.get_or_insert(j.conn);
        }
        let p4 = join(&s, person("p4"));
        assert!(p4.writers_full, "row: past the cap a new writer reads only");
        assert!(s.read(p4.conn).is_ok(), "it reads");
        let before = text(&s, p4.conn);
        let r = type_more(&s, p4.conn, "p4");
        assert_eq!(
            r,
            Err("MPC-E908".to_owned()),
            "row: its edit is refused MPC-E908"
        );
        assert_eq!(text(&s, p4.conn), before, "nothing changed");
        let e = s.replace_text(p4.conn, ROOT, "x").unwrap_err();
        assert_eq!(
            e.to_string(),
            "read-only: this doc has 4 writers (the most is 4)"
        );
        // a writer the doc holds still edits
        type_more(&s, first.unwrap(), " again").unwrap();
        let n = log_writers(&f.store, f.doc_id(&key("a.md"))).len();
        assert_eq!(n, 4, "row: the doc holds the cap's writers, no more");
    });
}

/// At the cap: the oldest parked writer of the person (let go early), then that person's free
/// writer of another device before a parked one; another person reads only.
#[test]
fn cap_takes_the_persons_free_then_oldest_parked() {
    under_mutant(|| {
        let f = fx("c2", |c| c.writers_max = 3);
        let s = f.s();
        let t1 = tab(&s, "ada", "laptop");
        type_more(&s, t1.conn, "one ").unwrap();
        let e1 = s.read(t1.conn).unwrap().version;
        assert!(s.leave(t1.conn).unwrap()); // parked
        let t2 = tab(&s, "ada", "laptop");
        assert_ne!(t2.client, t1.client, "t1's writer is parked");
        type_more(&s, t2.conn, "two ").unwrap();
        assert!(s.leave(t2.conn).unwrap()); // parked: the doc holds 3 writers
        let t3 = tab(&s, "ada", "phone");
        assert_eq!(
            t3.client, t1.client,
            "row: at the cap, the person's oldest parked writer"
        );
        assert!(!t3.writers_full);
        type_more(&s, t3.conn, "three ").unwrap();
        // t1's reclaim: its writer went to another lease, so it keeps nothing and is told when
        // its lease ended; it takes the next oldest parked writer
        let back = s
            .join_with(
                person("ada"),
                Stay::Held,
                &JoinOptions::new()
                    .device("laptop")
                    .reclaim(t1.lease.unwrap()),
            )
            .unwrap();
        assert!(!back.kept, "row: a writer let go early keeps nothing");
        assert_eq!(
            back.prev_ended,
            Some(e1),
            "row: its reclaim is told when its lease ended"
        );
        assert_eq!(back.client, t2.client, "row: then the next oldest parked");
        assert!(!back.writers_full);
        assert!(s.release(t3.conn).unwrap()); // t1's writer free, on the phone
        assert!(s.leave(back.conn).unwrap()); // t2's writer parked again
        let t4 = tab(&s, "ada", "tablet");
        assert_eq!(
            t4.client, t1.client,
            "row: the person's free writer of another device before a parked one"
        );
        type_more(&s, t4.conn, "four ").unwrap();
        let bea = tab(&s, "bea", "laptop");
        assert!(bea.writers_full, "row: another person reads only");
        assert_eq!(
            type_more(&s, bea.conn, "bea"),
            Err("MPC-E908".to_owned()),
            "row: its edit is refused MPC-E908"
        );
        let n = log_writers(&f.store, f.doc_id(&key("a.md"))).len();
        assert_eq!(n, 3, "row: the doc holds the cap's writers, no more");
    });
}

/// The import fills a one-writer cap; a person reads only; an outside save still lands.
#[test]
fn cap_exempts_the_outside_save_and_the_import() {
    under_mutant(|| {
        let f = fx("c3", |c| c.writers_max = 1);
        let path = f.ws.join("a.md");
        std::fs::write(&path, START).unwrap();
        let s = f
            .mgr
            .open_file(Arc::new(MD), &key("a.md"), &path, &f.ws)
            .unwrap();
        let p1 = join(&s, person("p1"));
        assert_eq!(text(&s, p1.conn), START, "row: the import lands at the cap");
        assert!(p1.writers_full, "row: at the cap a new writer reads only");
        std::fs::write(&path, format!("{START}saved outside\n")).unwrap();
        let r = s.file_changed();
        assert!(
            matches!(r, Ok(DiskCheck::Ingested { .. })),
            "row: the outside save lands at the cap: {r:?}"
        );
        assert_eq!(
            text(&s, p1.conn),
            format!("{START}saved outside\n"),
            "row: its text is in"
        );
        assert!(f
            .host
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, AuditEvent::ExternalEdit { .. })));
        assert_eq!(type_more(&s, p1.conn, "x"), Err("MPC-E908".to_owned()));
        let n = log_writers(&f.store, f.doc_id(&key("a.md"))).len();
        assert_eq!(n, 2, "the import and the outside participant, both counted");
    });
}

/// A connection that joins read-only and gains edit rights at the cap stays read-only.
#[test]
fn cap_counts_a_rights_gain() {
    under_mutant(|| {
        let f = fx("c4", |c| c.writers_max = 2);
        let s = f.s();
        let ed = join(&s, person("ed"));
        type_more(&s, ed.conn, "ed").unwrap(); // the import and ed: the cap
        f.grant("viewer", Rights::read_only());
        let v = join(&s, person("viewer"));
        assert!(!v.writers_full, "a reader needs no writer");
        let before = text(&s, v.conn);
        f.grant("viewer", Rights::editor());
        assert_eq!(
            type_more(&s, v.conn, "v"),
            Err("MPC-E908".to_owned()),
            "row: a rights gain at the cap is refused MPC-E908"
        );
        assert_eq!(text(&s, v.conn), before, "row: nothing changed");
        type_more(&s, ed.conn, " again").unwrap();
    });
}

/// An agent's own writer is reused at the cap; a new agent reads only.
#[test]
fn cap_new_agent_pid_read_only() {
    under_mutant(|| {
        let f = fx("c5", |c| c.writers_max = 3);
        let s = f.s();
        let b1 = join(&s, agent("bot1"));
        type_more(&s, b1.conn, "b1").unwrap();
        assert!(s.leave(b1.conn).unwrap());
        let ed = join(&s, person("ed"));
        type_more(&s, ed.conn, "ed").unwrap(); // the import, bot1 and ed: the cap
        let again = join(&s, agent("bot1"));
        assert_eq!(
            again.client, b1.client,
            "row: an agent reuses its writer at the cap"
        );
        assert!(!again.writers_full);
        type_more(&s, again.conn, " b1").unwrap();
        let b2 = join(&s, agent("bot2"));
        assert!(b2.writers_full, "row: a new agent reads only at the cap");
        assert_eq!(
            type_more(&s, b2.conn, "b2"),
            Err("MPC-E908".to_owned()),
            "row: its edit is refused MPC-E908"
        );
    });
}

/// Watchers (past `participants_max`) with edit rights, a person with a device and an agent,
/// hold writers the doc doesn't count: once a slot frees, a new writer still fits under the cap.
#[test]
fn cap_a_watcher_holds_no_counted_writer() {
    under_mutant(|| {
        let f = fx("cw", |c| {
            c.writers_max = 4;
            c.participants_max = 2;
        });
        let s = f.s();
        let id = f.doc_id(&key("a.md"));
        let ed = join(&s, person("ed"));
        type_more(&s, ed.conn, "ed").unwrap(); // the import and ed: 2 writers
        f.grant("pat", Rights::read_only());
        let pat = join(&s, person("pat")); // the second participant needs no writer
        let wat = tab(&s, "wat", "laptop");
        let bot = join(&s, agent("bot"));
        assert_eq!(s.watching(ed.conn).unwrap(), 2, "both watch");
        assert!(s.leave(pat.conn).unwrap()); // a slot frees; nobody is let in
        let new = join(&s, person("new"));
        assert!(
            !new.writers_full,
            "row: watchers hold no counted writer, so the doc has room for a new one"
        );
        type_more(&s, new.conn, "new").unwrap(); // 3 writers
        assert_eq!(
            f.store.client_owner(id, bot.client).unwrap(),
            None,
            "row: a watching agent's writer gets no owner row"
        );
        assert!(
            !wat.writers_full && !bot.writers_full,
            "a watcher is never at the cap"
        );
        for (who, c) in [("wat", wat.conn), ("bot", bot.conn)] {
            assert_eq!(
                type_more(&s, c, who),
                Err("MPC-E305".to_owned()),
                "row: a watcher's edit is refused watching, not MPC-E908"
            );
        }
        let n = log_writers(&f.store, id).len();
        assert_eq!(n, 3, "the import, ed and new");
    });
}

/// A watcher let in takes its writer then, under the cap; a watcher let in past the cap reads
/// only, and its edit is refused `MPC-E908` as any writer's past the cap.
#[test]
fn cap_a_promoted_watcher_takes_its_writer_then() {
    under_mutant(|| {
        let f = fx("cpw", |c| {
            c.writers_max = 3;
            c.participants_max = 1;
        });
        let s = f.s();
        let id = f.doc_id(&key("a.md"));
        let ed = join(&s, person("ed"));
        type_more(&s, ed.conn, "ed").unwrap(); // the import and ed: 2 writers
        let wat = tab(&s, "wat", "laptop");
        let bot = join(&s, agent("bot"));
        assert_eq!(s.watching(ed.conn).unwrap(), 2, "both watch");
        assert_eq!(type_more(&s, wat.conn, "w"), Err("MPC-E305".to_owned()));
        assert!(s.leave(ed.conn).unwrap());
        assert_eq!(s.promote_watcher().unwrap(), Some("wat".to_owned()));
        assert!(
            type_more(&s, wat.conn, "wat").is_ok(),
            "row: a watcher let in under the cap takes its writer and edits"
        );
        assert!(log_writers(&f.store, id).contains(&wat.client));
        assert!(s.leave(wat.conn).unwrap()); // parked: the doc holds 3 writers
        assert_eq!(s.promote_watcher().unwrap(), Some("bot".to_owned()));
        let before = text(&s, bot.conn);
        assert_eq!(
            type_more(&s, bot.conn, "bot"),
            Err("MPC-E908".to_owned()),
            "row: a watcher let in past the cap is refused MPC-E908"
        );
        let e = s.replace_text(bot.conn, ROOT, "x").unwrap_err();
        assert_eq!(
            e.to_string(),
            "read-only: this doc has 3 writers (the most is 3)"
        );
        assert_eq!(text(&s, bot.conn), before, "nothing changed");
        assert_eq!(
            f.store.client_owner(id, bot.client).unwrap(),
            None,
            "row: its writer gets no owner row"
        );
        assert_eq!(log_writers(&f.store, id).len(), 3);
    });
}

/// A promotion whose history has more authors than the cap lands; the doc then starts over
/// the cap, and a new writer reads only.
#[test]
fn cap_promotion_past_it_is_allowed_and_counted() {
    under_mutant(|| {
        let f = fx("cp", |c| c.writers_max = 2);
        let k = key("p.md");
        let b = f
            .mgr
            .open_blob(Arc::new(AnyFile), &k, Some(&mut &b"zero\n"[..]))
            .unwrap();
        let mut base = 1;
        for (p, t) in [("ada", "one\n"), ("bea", "two\n"), ("cyd", "three\n")] {
            let c = b.join(person(p), Stay::Held).unwrap().conn;
            base = b.write_bytes(c, t.as_bytes(), base).unwrap().version;
        }
        let r = f.mgr.promote_blob(Arc::new(MD), &k);
        assert!(
            r.is_ok(),
            "row: a promotion past the cap is allowed: {:?}",
            r.as_ref().err()
        );
        let (t, _) = r.unwrap();
        let dan = join(&t, person("dan"));
        assert!(dan.writers_full, "row: the promoted authors count");
        assert_eq!(text(&t, dan.conn), "three\n");
        assert_eq!(
            type_more(&t, dan.conn, "dan"),
            Err("MPC-E908".to_owned()),
            "row: a new writer's edit is refused MPC-E908"
        );
    });
}

/// With no cap, the notice comes once per load at 80% of `WRITERS_NOTICE_MAX`.
#[test]
fn near_cap_notice_counts_against_8192_without_a_cap() {
    under_mutant(|| {
        let f = fx("n", |_| {});
        let at = (WRITERS_NOTICE_MAX * 4).div_ceil(5); // 6,554
        let s = f.s();
        let id = f.doc_id(&key("a.md"));
        drop(s);
        f.mgr.close(&key("a.md"), "test").unwrap();
        // the import's writer and `at - 2` others: one short of the notice
        f.store.unsynced_for_tests();
        for i in 0..(at - 2) as u64 {
            f.store
                .set_client(id, 0x5eed_0000_0000 + i, &format!("old{}", i % 50))
                .unwrap();
        }
        let s = f.s();
        let p1 = join(&s, person("p1"));
        assert!(!p1.writers_full, "no cap: nobody reads only");
        let got = f.near_cap();
        assert_eq!(
            got,
            vec![(at as u64, WRITERS_NOTICE_MAX as u64)],
            "row: the notice at 80% of 8,192 with no cap"
        );
        let p2 = join(&s, person("p2"));
        type_more(&s, p2.conn, "p2").unwrap();
        assert_eq!(f.near_cap().len(), 1, "row: once per load");
    });
}
