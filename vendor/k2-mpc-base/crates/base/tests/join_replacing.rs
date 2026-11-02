//! `Session::join_replacing` (kit v0.10.1): a reconnect's join that closes the pid's previous
//! connection (by its client ID) as `leave` does, under the same lock, and ignores a `prev`
//! that isn't the pid's own present connection. Public API only.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use k2_mpc_base::session::{
    At, AuditEvent, ClaimEnd, ClaimId, Config, ConnId, Host, Joined, LineRange, Participant,
    ParticipantKind, Rights, RosterEvent, Session, SessionError, SessionManager, Stay,
    TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::Edit;

const ROOT: &str = TextFilePack::ROOT;
const DOC: &str = "one\ntwo\nthree\nfour\n";

// ---------- harness ----------

struct TestHost {
    now: AtomicU64,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for TestHost {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

struct Fx {
    dir: std::path::PathBuf,
    host: Arc<TestHost>,
    _mgr: SessionManager,
    s: Arc<Session>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-join-replacing-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), Config::default());
    let s = mgr
        .open(
            Arc::new(TextFilePack::new("markdown", 4 << 20)),
            &DocKey {
                space: "ws".into(),
                path: "notes.md".into(),
            },
            Some(DOC.as_bytes()),
        )
        .unwrap();
    Fx {
        dir,
        host,
        _mgr: mgr,
        s,
    }
}

fn person(pid: &str) -> Participant {
    Participant::new(
        pid,
        pid.to_uppercase(),
        "#123456",
        ParticipantKind::Person,
        "tab",
    )
}

impl Fx {
    fn join(&self, pid: &str) -> Joined {
        self.s.join(person(pid), Stay::Held).unwrap()
    }
    fn replacing(&self, pid: &str, prev: u64) -> Joined {
        self.s
            .join_replacing(person(pid), Stay::Held, prev)
            .unwrap()
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    /// A claim on line `line` and a caret at its start.
    fn claim_and_caret(&self, c: ConnId, line: usize) -> ClaimId {
        let id = self
            .s
            .claim(
                c,
                LineRange {
                    root: ROOT.into(),
                    first: line,
                    last: line,
                },
                None,
                None,
                false,
            )
            .unwrap()
            .id;
        let at = DOC.split_inclusive('\n').take(line - 1).map(str::len).sum();
        self.s
            .set_cursor(c, ROOT, At::Byte(at), At::Byte(at))
            .unwrap();
        id
    }
    fn claim_ended(&self, id: ClaimId) -> Option<ClaimEnd> {
        self.host
            .events
            .lock()
            .unwrap()
            .iter()
            .find_map(|e| match e {
                AuditEvent::ClaimEnded { claim, how, .. } if *claim == id => Some(*how),
                _ => None,
            })
    }
    /// `pid`'s roster row as `viewer` sees it: (connections, live claims, caret set).
    fn row(&self, viewer: ConnId, pid: &str) -> Option<(usize, usize, bool)> {
        self.s
            .roster(viewer)
            .unwrap()
            .into_iter()
            .find(|e| e.participant.pid == pid)
            .map(|e| (e.connections, e.claims.len(), e.cursor.is_some()))
    }
    /// Roster events delivered to `rx` after a flush.
    fn flushed(&self, rx: &Receiver<RosterEvent>) -> Vec<RosterEvent> {
        self.advance(1_000);
        self.s.flush_roster().unwrap();
        rx.try_iter().collect()
    }
}

fn left(evs: &[RosterEvent], pid: &str) -> bool {
    evs.iter()
        .any(|e| matches!(e, RosterEvent::Left { who, .. } if who.pid() == Some(pid)))
}

fn joined(evs: &[RosterEvent], pid: &str) -> bool {
    evs.iter()
        .any(|e| matches!(e, RosterEvent::Joined { who, .. } if who.pid() == Some(pid)))
}

fn edit(s: &Session, c: ConnId, insert: &str) -> Result<u64, SessionError> {
    s.edit(
        c,
        vec![Edit::Text {
            root: ROOT.into(),
            hunks: vec![k2_mpc_base::text::diff::Hunk {
                start: 0,
                end: 0,
                insert: insert.into(),
            }],
        }],
        None,
    )
    .map(|r| r.version)
}

// ---------- the calls ----------

/// The pid's own previous connection is closed as `leave` closes it: its calls get
/// `NotJoined`, its claim ended (`Left`), its caret gone, its roster subscription closed; the
/// pid stays present (others see no `Left` and no new `Joined` for it).
#[test]
fn replaces_the_pids_own_connection() {
    let f = fx("replace");
    let old = f.join("ann");
    let bob = f.join("bob").conn;
    let claim = f.claim_and_caret(old.conn, 2);
    let rx_old = f.s.subscribe_roster(old.conn).unwrap();
    let rx_bob = f.s.subscribe_roster(bob).unwrap();
    f.flushed(&rx_bob);
    assert_eq!(f.row(bob, "ann"), Some((1, 1, true)));

    let new = f.replacing("ann", old.client);
    assert_eq!(new.replaced, Some(old.conn));
    assert_ne!(new.conn, old.conn);
    assert_ne!(new.client, old.client, "a new client ID");

    assert!(matches!(
        f.s.read(old.conn),
        Err(SessionError::NotJoined { conn }) if conn == old.conn
    ));
    assert!(matches!(
        f.s.set_cursor(old.conn, ROOT, At::Byte(0), At::Byte(0)),
        Err(SessionError::NotJoined { .. })
    ));
    assert!(!f.s.leave(old.conn).unwrap(), "already gone");
    assert_eq!(f.claim_ended(claim), Some(ClaimEnd::Left));
    assert_eq!(f.row(bob, "ann"), Some((1, 0, false)), "no claim, no caret");
    let _ = rx_old.try_iter().count();
    assert_eq!(rx_old.try_recv(), Err(TryRecvError::Disconnected));
    let evs = f.flushed(&rx_bob);
    assert!(!left(&evs, "ann"), "the pid stayed: {evs:?}");
    assert!(!joined(&evs, "ann"), "the pid stayed: {evs:?}");
    // the new connection works
    f.claim_and_caret(new.conn, 3);
    assert_eq!(f.row(bob, "ann"), Some((1, 1, true)));
}

/// An edit sent on the old connection after the replace is refused and changes nothing.
#[test]
fn the_old_connections_edit_is_refused() {
    let f = fx("old-edit");
    let old = f.join("ann");
    let v = f.s.read(old.conn).unwrap().version;
    let new = f.replacing("ann", old.client);
    assert_eq!(new.replaced, Some(old.conn));
    assert!(matches!(
        edit(&f.s, old.conn, "late "),
        Err(SessionError::NotJoined { .. })
    ));
    let r = f.s.read(new.conn).unwrap();
    assert_eq!((r.version, r.text_by_root[0].1.as_str()), (v, DOC));
    assert_eq!(edit(&f.s, new.conn, "new ").unwrap(), v + 1);
}

/// A `prev` that is another pid's connection is ignored: that connection keeps its claim,
/// caret and roster subscription, all working, and the new join is an ordinary join.
#[test]
fn another_pids_prev_is_ignored() {
    let f = fx("foreign");
    let bob = f.join("bob");
    let claim = f.claim_and_caret(bob.conn, 2);
    let rx_bob = f.s.subscribe_roster(bob.conn).unwrap();
    f.flushed(&rx_bob);

    let ann = f.replacing("ann", bob.client);
    assert_eq!(ann.replaced, None);
    assert_ne!(ann.client, bob.client);

    assert!(f.s.read(bob.conn).is_ok());
    assert_eq!(f.claim_ended(claim), None, "bob's claim stands");
    assert_eq!(f.row(ann.conn, "bob"), Some((1, 1, true)));
    assert_eq!(f.row(bob.conn, "ann"), Some((1, 0, false)));
    let evs = f.flushed(&rx_bob);
    assert!(
        joined(&evs, "ann"),
        "an ordinary join, seen by bob: {evs:?}"
    );
    assert!(!left(&evs, "bob"), "{evs:?}");
    assert_eq!(rx_bob.try_recv(), Err(TryRecvError::Empty), "still open");
    // still working: bob moves his caret and edits
    f.s.set_cursor(bob.conn, ROOT, At::Byte(0), At::Byte(0))
        .unwrap();
    edit(&f.s, bob.conn, "bob ").unwrap();
    assert!(
        !f.flushed(&rx_bob).is_empty(),
        "bob's subscription still delivers"
    );
}

/// A `prev` no present connection holds (never used, or its connection already left) is
/// ignored: an ordinary join, and the pid's other connection untouched.
#[test]
fn an_unknown_or_gone_prev_is_ignored() {
    let f = fx("unknown");
    let a1 = f.join("ann");
    let claim = f.claim_and_caret(a1.conn, 2);
    let unknown = if a1.client == 1 { 2 } else { 1 };
    let a2 = f.replacing("ann", unknown);
    assert_eq!(a2.replaced, None);
    assert!(f.s.read(a1.conn).is_ok());
    assert_eq!(f.claim_ended(claim), None);
    assert_eq!(f.row(a1.conn, "ann"), Some((2, 1, true)));

    // already left
    let gone = f.join("ann");
    assert!(f.s.leave(gone.conn).unwrap());
    let a3 = f.replacing("ann", gone.client);
    assert_eq!(a3.replaced, None);
    assert_eq!(f.row(a1.conn, "ann"), Some((3, 1, true)));
}

/// The pid with another connection besides the one replaced: only `prev`'s connection
/// closes; the other keeps its claim and caret; the pid stays present throughout.
#[test]
fn only_prevs_connection_of_the_pid_closes() {
    let f = fx("two-tabs");
    let tab1 = f.join("ann");
    let tab2 = f.join("ann");
    let bob = f.join("bob").conn;
    let c1 = f.claim_and_caret(tab1.conn, 2);
    let c2 = f.claim_and_caret(tab2.conn, 4);
    let rx_bob = f.s.subscribe_roster(bob).unwrap();
    f.flushed(&rx_bob);
    assert_eq!(f.row(bob, "ann"), Some((2, 2, true)));

    let new = f.replacing("ann", tab1.client);
    assert_eq!(new.replaced, Some(tab1.conn));
    assert!(matches!(
        f.s.read(tab1.conn),
        Err(SessionError::NotJoined { .. })
    ));
    assert!(f.s.read(tab2.conn).is_ok());
    assert_eq!(f.claim_ended(c1), Some(ClaimEnd::Left));
    assert_eq!(f.claim_ended(c2), None);
    assert_eq!(
        f.row(bob, "ann"),
        Some((2, 1, true)),
        "tab 2 and the new one"
    );
    let evs = f.flushed(&rx_bob);
    assert!(!left(&evs, "ann"), "{evs:?}");
}
