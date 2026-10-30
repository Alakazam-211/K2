//! Participant overflow (K11): past `Config::participants_max` distinct participants, a new
//! pid joins as a watcher: read-only whatever its rights, no presence detail, shown in the
//! roster only as `watching: N`. A freed slot promotes nobody by itself; the host lets the
//! earliest watcher in. An edit by a watcher is refused `MPC-E305` (exit 3). Public API only.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use k2_mpc_base::session::{
    At, AuditEvent, Config, ConnId, Host, LineCol, LineRange, Participant, ParticipantKind, Rights,
    RosterEvent, Session, SessionError, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};

const ROOT: &str = TextFilePack::ROOT;
const DOC: &str = "one\ntwo\nthree\n";

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::full())
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct TmpDir(std::path::PathBuf);

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    _dir: TmpDir,
    host: Arc<TestHost>,
    mgr: SessionManager,
    s: Arc<Session>,
}

/// A doc admitting `max` participants.
fn fx(tag: &str, max: usize) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-overflow-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let mut cfg = Config::default();
    cfg.participants_max = max;
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), cfg);
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
        _dir: TmpDir(dir),
        host,
        mgr,
        s,
    }
}

impl Fx {
    fn join(&self, pid: &str) -> ConnId {
        let p = Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab");
        self.s.join(p, Stay::Held).unwrap().conn
    }
    fn pids(&self, viewer: ConnId) -> Vec<String> {
        self.s
            .roster(viewer)
            .unwrap()
            .into_iter()
            .map(|e| e.participant.pid)
            .collect()
    }
    fn tick(&self) {
        self.host.now.fetch_add(300, Ordering::SeqCst);
        self.mgr.flush_rosters();
    }
    fn edit(&self, c: ConnId) -> Result<(), SessionError> {
        let t = self.s.read(c)?.text_by_root.remove(0).1;
        self.s.replace_text(c, ROOT, &format!("{t}x\n")).map(|_| ())
    }
}

fn drain(rx: &Receiver<RosterEvent>) -> Vec<RosterEvent> {
    let mut v = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(e) => v.push(e),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return v,
        }
    }
}

fn lc(line: usize, col: usize) -> At {
    At::Line(LineCol { line, col })
}

/// Past the limit: read-only, no row, counted; edits refused with MPC-E305 (exit 3).
#[test]
fn joins_past_the_limit_watch_read_only() {
    let f = fx("watch", 2);
    let a = f.join("a");
    let b = f.join("b");
    let c = f.join("c");
    assert_eq!(f.pids(a), vec!["a", "b"]);
    assert_eq!(f.s.watching(a).unwrap(), 1);
    assert_eq!(
        f.s.status().unwrap().version,
        f.s.read(c).unwrap().version,
        "c reads"
    );

    // full rights from the host, but read-only while watching
    let e = f.edit(c).expect_err("a watcher can't edit");
    assert_eq!(e, SessionError::Watching { max: 2 });
    let r = e.report();
    assert_eq!(r.id(), "MPC-E305");
    assert_eq!(k2_mpc_base::tools::session_exit(&e), 3);
    assert!(matches!(
        f.s.set_cursor(c, ROOT, lc(1, 1), lc(1, 1)),
        Err(SessionError::Watching { .. })
    ));
    let range = LineRange {
        root: ROOT.into(),
        first: 1,
        last: 2,
    };
    assert!(matches!(
        f.s.claim(c, range.clone(), None, None, false),
        Err(SessionError::Watching { .. })
    ));
    // no presence detail: a view is not kept, and the watcher is in no roster row
    f.s.set_view(c, range).unwrap();
    f.s.set_visible(c, false).unwrap();
    assert!(f
        .s
        .roster(a)
        .unwrap()
        .iter()
        .all(|e| e.participant.pid != "c" && e.view.is_none()));
    // a second connection of a participant is no watcher; of a watcher, still one
    let b2 = f.join("b");
    f.edit(b2).unwrap();
    let c2 = f.join("c");
    assert!(matches!(f.edit(c2), Err(SessionError::Watching { .. })));
    assert_eq!(f.s.watching(a).unwrap(), 1);
    f.edit(a).unwrap();
    let _ = b;
}

/// A freed slot promotes nobody by itself; the host lets the earliest watcher in.
#[test]
fn only_the_host_promotes_the_earliest_watcher() {
    let f = fx("promote", 2);
    let a = f.join("a");
    let b = f.join("b");
    let c = f.join("c");
    let d = f.join("d");
    assert_eq!(f.s.watching(a).unwrap(), 2);
    assert_eq!(f.s.promote_watcher().unwrap(), None, "no slot free");
    f.s.leave(b).unwrap();
    // nobody moves up by themselves
    assert!(matches!(f.edit(c), Err(SessionError::Watching { .. })));
    assert_eq!(f.pids(a), vec!["a"]);
    // the host asks: c joined first
    assert_eq!(f.s.promote_watcher().unwrap(), Some("c".to_owned()));
    f.edit(c).unwrap();
    assert_eq!(f.s.watching(a).unwrap(), 1);
    assert!(f.pids(a).contains(&"c".to_owned()));
    assert_eq!(f.s.promote_watcher().unwrap(), None, "full again");
    assert!(matches!(f.edit(d), Err(SessionError::Watching { .. })));
    // the host's rights still apply after promotion (no rights change by the kit)
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("c".into(), Rights::read_only());
    assert!(matches!(f.edit(c), Err(SessionError::Forbidden { .. })));
}

/// The roster push: watchers show only as a count, never as joins or rows.
#[test]
fn the_roster_push_counts_watchers() {
    let f = fx("push", 1);
    let a = f.join("a");
    let rx = f.s.subscribe_roster(a).unwrap();
    match drain(&rx).remove(0) {
        RosterEvent::Snapshot {
            members, watching, ..
        } => {
            assert_eq!((members.len(), watching), (1, 0));
        }
        e => panic!("{e:?}"),
    }
    let w1 = f.join("w1");
    f.join("w2");
    f.tick();
    let evs = drain(&rx);
    assert_eq!(evs, vec![RosterEvent::Watching { count: 2 }]);
    f.s.leave(w1).unwrap();
    f.tick();
    assert_eq!(drain(&rx), vec![RosterEvent::Watching { count: 1 }]);
    // a's slot: when a leaves... a is the subscriber; free a slot with a second doc member
    let w3 = f.join("w3");
    f.tick();
    assert_eq!(drain(&rx), vec![RosterEvent::Watching { count: 2 }]);
    let _ = w3;
}

/// The default limit is 32.
#[test]
fn the_default_limit_is_32() {
    assert_eq!(Config::default().participants_max, 32);
    let f = fx("default", 32);
    let first = f.join("p0");
    for n in 1..32 {
        f.join(&format!("p{n}"));
    }
    assert_eq!(f.s.watching(first).unwrap(), 0);
    f.join("p32");
    assert_eq!(f.s.watching(first).unwrap(), 1);
    assert_eq!(f.pids(first).len(), 32);
}
