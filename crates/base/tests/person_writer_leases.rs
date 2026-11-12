//! Person-writer reuse (`Config::reuse_person_writers`, v0.14.0;
//! `docs/design/person-writer-reuse-r0.md`): the leases and their rows. A person's join with
//! a device key holds its writer under a lease; a release frees it, any other end parks it;
//! the rows ride in a change's commit and the session's close, never in a join or a leave; a
//! writer gets its row only once a connection holding it can write.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::session::{
    AuditEvent, Config, EditOptions, Host, JoinOptions, Joined, Participant, ParticipantKind,
    Rights, Session, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocId, DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;

/// Everyone edits but the viewer `vic`, who reads only until `vic_edits` is set.
struct H(AtomicU64, AtomicBool);
impl Host for H {
    fn rights(&self, p: &Participant, _: &DocKey) -> Rights {
        if p.pid == "vic" && !self.1.load(Ordering::SeqCst) {
            Rights::read_only()
        } else {
            Rights::full()
        }
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    }
}

fn config(on: bool) -> Config {
    let mut c = Config::default();
    c.reuse_person_writers = on;
    c.edit_rate_per_s = 1 << 30;
    c.edit_burst = 1 << 30;
    c
}

struct Fx {
    dir: std::path::PathBuf,
    store: Arc<DocStore>,
    host: Arc<H>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = self.mgr.release();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(name: &str, on: bool) -> Fx {
    let dir = std::env::temp_dir().join(format!("k2mpc-pwl-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("d.redb")).unwrap());
    let host = Arc::new(H(AtomicU64::new(1), AtomicBool::new(false)));
    let mgr = SessionManager::new(store.clone(), host.clone(), config(on));
    Fx {
        dir,
        store,
        host,
        mgr,
    }
}

impl Fx {
    fn s(&self) -> Arc<Session> {
        self.mgr
            .open(Arc::new(MD), &key(), Some(b"# Title\n"))
            .unwrap()
    }

    fn id(&self) -> DocId {
        self.store.find(&key()).unwrap().unwrap()
    }
}

fn person(pid: &str) -> Participant {
    Participant::new(pid, pid, "#7c3aed", ParticipantKind::Person, "tab")
}

fn tab(s: &Session, dev: &str) -> Joined {
    s.join_with(person("ada"), Stay::Held, &JoinOptions::new().device(dev))
        .unwrap()
}

fn type_at_end(s: &Session, j: &Joined, text: &str) {
    let r = s.read(j.conn).unwrap();
    let at = r.text_by_root[0].1.len();
    s.edit_at_base(
        j.conn,
        ROOT,
        r.version,
        vec![Hunk {
            start: at,
            end: at,
            insert: text.into(),
        }],
        &EditOptions::new(),
    )
    .unwrap();
}

/// A device join holds its writer under a lease; the row lands with the lease's first change
/// (no join or leave writes it); a release frees the writer, any other end parks it with the
/// version it ended at; the close writes both.
#[test]
fn a_lease_and_its_row() {
    let f = fx("lease", true);
    let s = f.s();
    let a = tab(&s, "laptop");
    let b = tab(&s, "laptop");
    let la = a.lease.expect("a device join holds a lease");
    assert_eq!(la.client, a.client);
    assert_ne!(la.token, 0);
    assert_ne!(a.client, b.client);
    assert!(
        f.store.person_writers(f.id()).unwrap().is_empty(),
        "a join writes no row"
    );
    type_at_end(&s, &a, "one ");
    let rows = f.store.person_writers(f.id()).unwrap();
    assert_eq!(
        rows.len(),
        2,
        "the change's commit writes every row changed since"
    );
    let ra = rows.iter().find(|r| r.0 == a.client).unwrap();
    assert_eq!(ra.1, "ada");
    assert!(ra.2.leased && ra.2.token == la.token);
    type_at_end(&s, &b, "two ");
    let ended = s.read(b.conn).unwrap().version;
    assert!(s.release(a.conn).unwrap());
    assert!(s.leave(b.conn).unwrap());
    let rows = f.store.person_writers(f.id()).unwrap();
    assert!(
        rows.iter().all(|r| r.2.leased),
        "a release and a leave write nothing"
    );
    drop(s);
    f.mgr.close(&key(), "test").unwrap();
    let rows = f.store.person_writers(f.id()).unwrap();
    let ra = rows.iter().find(|r| r.0 == a.client).unwrap();
    let rb = rows.iter().find(|r| r.0 == b.client).unwrap();
    assert!(!ra.2.leased && ra.2.token == 0, "released: free");
    assert!(rb.2.leased, "left: parked");
    assert_eq!(rb.2.ended_token, b.lease.unwrap().token);
    assert_eq!(rb.2.ended_seq, ended);
    assert_eq!(
        f.store.client_owner(f.id(), b.client).unwrap().as_deref(),
        Some("ada")
    );
}

/// A lease still held when the session closes is parked at the close's version; one lost to a
/// crash (its end never written) is parked at the next load's.
#[test]
fn held_leases_park_at_a_close() {
    let f = fx("close", true);
    let s = f.s();
    let a = tab(&s, "laptop");
    type_at_end(&s, &a, "one ");
    let v = s.read(a.conn).unwrap().version;
    drop(s);
    f.mgr.close(&key(), "test").unwrap();
    let rows = f.store.person_writers(f.id()).unwrap();
    assert!(rows[0].2.leased && rows[0].2.ended_token == a.lease.unwrap().token);
    assert_eq!(rows[0].2.ended_seq, v);
    let s = f.s();
    let held = s.person_writers_for_tests().unwrap();
    assert!(held[0].2.leased && !held[0].3, "parked after the reload");
}

/// Without a device key, with reuse off, for an agent or an own replica: no lease, no row.
#[test]
fn no_lease_as_before() {
    let f = fx("none", true);
    let s = f.s();
    let plain = s.join(person("ada"), Stay::Held).unwrap();
    assert!(plain.lease.is_none());
    let agent = Participant::new("bot", "bot", "#000000", ParticipantKind::Agent, "cli");
    let a = s
        .join_with(agent, Stay::Held, &JoinOptions::new().device("laptop"))
        .unwrap();
    assert!(a.lease.is_none(), "an agent keeps its own reuse");
    let r = s
        .join_with(
            person("ada").own_replica(),
            Stay::Held,
            &JoinOptions::new().device("laptop"),
        )
        .unwrap();
    assert!(r.lease.is_none(), "an own replica never takes part");
    let long = s
        .join_with(
            person("ada"),
            Stay::Held,
            &JoinOptions::new().device([0u8; 65]),
        )
        .unwrap();
    assert!(long.lease.is_none(), "a key over 64 bytes is ignored");
    assert!(
        s.release(plain.conn).unwrap(),
        "a release without a lease is a leave"
    );
    let off = fx("off", false);
    let s = off.s();
    let j = tab(&s, "laptop");
    assert!(j.lease.is_none(), "reuse off: the key is ignored");
}

/// A viewer's writer gets no row: a read-only device join, then a close, leaves none (every
/// row stays for good). When the viewer gains edit rights and edits, its writer gets exactly
/// one row, the person's.
#[test]
fn a_read_only_lease_leaves_no_row_until_it_can_write() {
    let f = fx("read-only", true);
    let s = f.s();
    let viewer = || {
        s.join_with(
            person("vic"),
            Stay::Held,
            &JoinOptions::new().device("tablet"),
        )
        .unwrap()
    };
    let v = viewer();
    assert!(v.lease.is_some(), "a viewer's device join holds a lease");
    // someone else's change lands the rows changed since: not the viewer's
    let a = tab(&s, "laptop");
    type_at_end(&s, &a, "one ");
    assert!(s.leave(v.conn).unwrap());
    drop(s);
    f.mgr.close(&key(), "test").unwrap();
    let rows = f.store.person_writers(f.id()).unwrap();
    assert!(
        rows.iter().all(|r| r.1 != "vic"),
        "a read-only lease leaves no row: {rows:?}"
    );
    assert_eq!(f.store.client_owner(f.id(), v.client).unwrap(), None);
    // the viewer comes back, gains edit rights and edits: exactly one row, its own
    let s = f.s();
    let v2 = s
        .join_with(
            person("vic"),
            Stay::Held,
            &JoinOptions::new().device("tablet"),
        )
        .unwrap();
    f.host.1.store(true, Ordering::SeqCst);
    type_at_end(&s, &v2, "vic ");
    let rows: Vec<_> = f
        .store
        .person_writers(f.id())
        .unwrap()
        .into_iter()
        .filter(|r| r.1 == "vic")
        .collect();
    assert_eq!(rows.len(), 1, "one row once it can write: {rows:?}");
    assert_eq!(rows[0].0, v2.client);
    assert!(rows[0].2.leased);
}
