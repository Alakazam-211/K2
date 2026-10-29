//! Sessions as a host uses them: rights, presence, connections, edits, write --base, undo,
//! events, persistence, the pool, limits. Uses only the public API, with no untrusted bytes.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::session::{
    Action, ApprovalState, At, AuditEvent, Config, ConnId, EditWithError, Host, LeaveReason,
    LineCol, LineRange, Need, Pack, Participant, ParticipantKind, Rights, Role, Session,
    SessionError, SessionManager, Stay, TextFilePack, UndoOutcome, WriteOutcome, IMPORT_PID,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::{self, Hunk};
use k2_mpc_base::Edit;

const MD: TextFilePack = TextFilePack::new("markdown", 4 << 20);
const ROOT: &str = TextFilePack::ROOT;

// ---------- harness ----------

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
    events: Mutex<Vec<AuditEvent>>,
}

impl TestHost {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            now: AtomicU64::new(1_000_000),
            rights: Mutex::new(HashMap::new()),
            events: Mutex::new(Vec::new()),
        })
    }
    fn grant(&self, pid: &str, r: Rights) {
        self.rights.lock().unwrap().insert(pid.into(), r);
    }
    fn advance(&self, ms: u64) {
        self.now.fetch_add(ms, Ordering::SeqCst);
    }
    fn events(&self) -> Vec<AuditEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::none())
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

struct TmpDir(std::path::PathBuf);
impl TmpDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "k2mb-sessions-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
    fn db(&self) -> std::path::PathBuf {
        self.0.join("docs.redb")
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    dir: TmpDir,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    mgr: SessionManager,
}

fn fx_with(tag: &str, config: Config) -> Fx {
    let dir = TmpDir::new(tag);
    let host = TestHost::new();
    let store = Arc::new(DocStore::open(dir.db()).unwrap());
    let mgr = SessionManager::new(store.clone(), host.clone(), config);
    Fx {
        dir,
        host,
        store,
        mgr,
    }
}

fn fx(tag: &str) -> Fx {
    fx_with(tag, Config::default())
}

fn burst(n: u64) -> Config {
    let mut c = Config::default();
    c.edit_burst = n;
    c
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

fn pack() -> Arc<dyn Pack> {
    Arc::new(MD)
}

fn who(pid: &str, kind: ParticipantKind) -> Participant {
    Participant::new(pid, pid.to_uppercase(), "#123456", kind, "cli")
}

fn agent(pid: &str) -> Participant {
    who(pid, ParticipantKind::Agent)
}

impl Fx {
    fn open(&self, path: &str, bytes: &[u8]) -> Arc<Session> {
        self.mgr.open(pack(), &key(path), Some(bytes)).unwrap()
    }
    /// Grant `r` to `pid` and open a new connection for it.
    fn join(&self, s: &Session, pid: &str, r: Rights) -> ConnId {
        self.host.grant(pid, r);
        s.join(agent(pid), Stay::UntilIdle).unwrap().conn
    }
}

fn text(s: &Session, c: ConnId) -> String {
    s.read(c).unwrap().text_by_root.remove(0).1
}

fn ins(at: usize, s: &str) -> Vec<Edit> {
    vec![Edit::Text {
        root: ROOT.into(),
        hunks: vec![Hunk {
            start: at,
            end: at,
            insert: s.into(),
        }],
    }]
}

/// Replace the first `old` with `new` through `c`, atomically against the live text.
fn replace(s: &Session, c: ConnId, old: &str, new: &str) {
    let r = s
        .edit_with(c, ROOT, |t| match t.find(old) {
            Some(_) => Ok(t.replacen(old, new, 1)),
            None => Err(format!("{old:?} not in {t:?}")),
        })
        .unwrap();
    assert!(r.is_some(), "replace {old:?} changed nothing");
}

fn roster_pids(s: &Session, c: ConnId) -> Vec<String> {
    s.roster(c)
        .unwrap()
        .into_iter()
        .map(|e| e.participant.pid)
        .collect()
}

// ---------- tests ----------

#[test]
fn concurrent_agents_lose_nothing_and_are_attributed() {
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let f = fx_with("concurrent", c);
    const AGENTS: usize = 4;
    const EACH: usize = 15;
    let seed: String = (0..AGENTS)
        .map(|k| format!("## agent {k}\nend {k}\n"))
        .collect();
    let s = f.open("c.md", seed.as_bytes());
    let conns: Vec<ConnId> = (0..AGENTS)
        .map(|k| f.join(&s, &format!("a{k}"), Rights::editor()))
        .collect();
    std::thread::scope(|sc| {
        for (k, &conn) in conns.iter().enumerate() {
            let s = s.clone();
            sc.spawn(move || {
                let pid = format!("a{k}");
                for j in 0..EACH {
                    // half via write --base from a read, half via edit_with: both merge
                    if j % 2 == 0 {
                        let r = s.read(conn).unwrap();
                        let t = &r.text_by_root[0].1;
                        let at = t.find(&format!("end {k}\n")).unwrap();
                        let mine = format!("{}{pid} line {j}\n{}", &t[..at], &t[at..]);
                        match s.write_base(conn, ROOT, r.version, &mine).unwrap() {
                            WriteOutcome::Applied(e) => assert_eq!(e.lines_added, 1),
                            o => panic!("disjoint sections must merge: {o:?}"),
                        }
                    } else {
                        let e = s
                            .edit_with(conn, ROOT, |t| {
                                let at = t.find(&format!("end {k}\n")).unwrap();
                                Ok::<_, ()>(format!("{}{pid} line {j}\n{}", &t[..at], &t[at..]))
                            })
                            .unwrap()
                            .unwrap();
                        assert_eq!(e.lines_added, 1);
                    }
                }
            });
        }
    });
    let checker = f.join(&s, "checker", Rights::read_only());
    let expect: String = (0..AGENTS)
        .map(|k| {
            let lines: String = (0..EACH).map(|j| format!("a{k} line {j}\n")).collect();
            format!("## agent {k}\n{lines}end {k}\n")
        })
        .collect();
    assert_eq!(text(&s, checker), expect);
    assert_eq!(s.status().unwrap().version, 1 + (AGENTS * EACH) as u64);
    // every logged update is attributed to the participant that made it
    let id = f.store.find(&key("c.md")).unwrap().unwrap();
    let log = f.store.load(id).unwrap();
    assert_eq!(log.updates.len(), 1 + AGENTS * EACH);
    let mut per: HashMap<String, usize> = HashMap::new();
    for (_, e) in &log.updates {
        assert_eq!(
            f.store.client_owner(id, e.client).unwrap().as_deref(),
            Some(e.author.as_str())
        );
        *per.entry(e.author.clone()).or_default() += 1;
    }
    assert_eq!(per[IMPORT_PID], 1);
    for k in 0..AGENTS {
        assert_eq!(per[&format!("a{k}")], EACH);
    }
}

#[test]
fn one_pid_many_connections() {
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let f = fx_with("conns", c);
    let s = f.open("m.md", b"top\nbottom\n");
    f.host.grant("sam", Rights::editor());
    let tab1 = s
        .join(who("sam", ParticipantKind::Person), Stay::Held)
        .unwrap();
    let tab2 = s
        .join(who("sam", ParticipantKind::Person), Stay::Held)
        .unwrap();
    assert_ne!(tab1.conn, tab2.conn);
    assert_ne!(
        tab1.client, tab2.client,
        "each connection has its own client id"
    );
    let watcher = f.join(&s, "w", Rights::read_only());
    let rx = s.subscribe(tab2.conn).unwrap();
    // both connections edit concurrently, in different places
    std::thread::scope(|sc| {
        for (conn, anchor) in [(tab1.conn, "top\n"), (tab2.conn, "bottom\n")] {
            let s = s.clone();
            sc.spawn(move || {
                for j in 0..20 {
                    s.edit_with(conn, ROOT, |t| {
                        let at = t.find(anchor).unwrap() + anchor.len();
                        Ok::<_, ()>(format!("{}{j}\n{}", &t[..at], &t[at..]))
                    })
                    .unwrap()
                    .unwrap();
                }
            });
        }
    });
    let t = text(&s, watcher);
    assert_eq!(t.lines().count(), 42, "{t}");
    // every update is attributed to sam, through both client ids
    let id = f.store.find(&key("m.md")).unwrap().unwrap();
    let log = f.store.load(id).unwrap();
    let clients: HashSet<u64> = log
        .updates
        .iter()
        .filter(|(_, e)| e.author == "sam")
        .map(|(_, e)| e.client)
        .collect();
    assert_eq!(clients, HashSet::from([tab1.client, tab2.client]));
    for c in [tab1.client, tab2.client] {
        assert_eq!(f.store.client_owner(id, c).unwrap().as_deref(), Some("sam"));
    }
    // one roster entry per pid, with a connection count
    let r = s.roster(watcher).unwrap();
    assert_eq!(r.len(), 2);
    let sam = r.iter().find(|e| e.participant.pid == "sam").unwrap();
    assert_eq!((sam.connections, sam.role), (2, Role::Editor));
    let st = s.status().unwrap();
    assert_eq!((st.participants, st.connections), (2, 3));
    // undo is per pid: tab2 undoes tab1's last edit if it was the pid's newest
    s.edit(tab1.conn, ins(0, "X"), None).unwrap();
    s.undo(tab2.conn, 1).unwrap();
    assert!(!text(&s, watcher).starts_with('X'));
    // away only when every connection is away
    s.set_visible(tab1.conn, false).unwrap();
    f.host.advance(61_000);
    let away = |s: &Session| {
        s.roster(watcher)
            .unwrap()
            .into_iter()
            .find(|e| e.participant.pid == "sam")
            .unwrap()
            .away
    };
    assert!(!away(&s));
    s.set_visible(tab2.conn, false).unwrap();
    f.host.advance(61_000);
    assert!(away(&s));
    // leaving one connection keeps the pid present
    assert!(s.leave(tab1.conn).unwrap());
    assert!(!s.leave(tab1.conn).unwrap(), "already gone");
    assert_eq!(
        s.read(tab1.conn),
        Err(SessionError::NotJoined { conn: tab1.conn })
    );
    let r = s.roster(watcher).unwrap();
    let sam = r.iter().find(|e| e.participant.pid == "sam").unwrap();
    assert_eq!(sam.connections, 1);
    s.read(tab2.conn).unwrap();
    // kick closes every connection of the pid, with their subscriptions
    let tab3 = s
        .join(who("sam", ParticipantKind::Person), Stay::Held)
        .unwrap();
    assert_eq!(s.kick("sam").unwrap(), 2);
    assert!(matches!(
        s.read(tab2.conn),
        Err(SessionError::NotJoined { .. })
    ));
    assert!(matches!(
        s.read(tab3.conn),
        Err(SessionError::NotJoined { .. })
    ));
    assert_eq!(roster_pids(&s, watcher), vec!["w"]);
    while rx.try_recv().is_ok() {}
    assert!(rx.recv().is_err(), "subscription closed by the kick");
    let kicked: Vec<ConnId> = f
        .host
        .events()
        .into_iter()
        .filter_map(|e| match e {
            AuditEvent::Left {
                conn,
                reason: LeaveReason::Kicked,
                ..
            } => Some(conn),
            _ => None,
        })
        .collect();
    assert_eq!(kicked.len(), 2);
    // losing read removes every connection of the pid
    let a1 = f.join(&s, "x", Rights::read_only());
    let a2 = f.join(&s, "x", Rights::read_only());
    f.host.grant("x", Rights::none());
    assert_eq!(
        s.read(a1),
        Err(SessionError::Forbidden { need: Need::Read })
    );
    assert_eq!(s.read(a2), Err(SessionError::NotJoined { conn: a2 }));
}

#[test]
fn shared_joins_reuse_a_connection_and_fresh_ones_never_reuse_clients() {
    let f = fx("shared");
    let s = f.open("a.md", b"x\n");
    f.host.grant("a", Rights::editor());
    let j1 = s.join_shared(agent("a"), Stay::UntilIdle).unwrap();
    let j2 = s.join_shared(agent("a"), Stay::UntilIdle).unwrap();
    assert_eq!(
        (j1.conn, j1.client),
        (j2.conn, j2.client),
        "verbs share one"
    );
    assert!(j1.client > 1 && j1.client < (1 << 53));
    let mut tab = agent("a");
    tab.via = "tab".into();
    let j3 = s.join_shared(tab, Stay::UntilIdle).unwrap();
    assert_ne!(j1.conn, j3.conn, "another surface is another connection");
    let j4 = s.join(agent("a"), Stay::UntilIdle).unwrap();
    assert_ne!(j1.conn, j4.conn, "join always opens a new one");
    s.leave(j1.conn).unwrap();
    let j5 = s.join_shared(agent("a"), Stay::UntilIdle).unwrap();
    assert_ne!(j1.client, j5.client, "fresh after leaving");
    let id = f.store.find(&key("a.md")).unwrap().unwrap();
    for j in [j1, j3, j4, j5] {
        assert_eq!(
            f.store.client_owner(id, j.client).unwrap().as_deref(),
            Some("a")
        );
    }
    // identity namespace: the kit's own pids are refused
    f.host.grant("kit:import", Rights::editor());
    assert!(matches!(
        s.join(agent("kit:import"), Stay::UntilIdle),
        Err(SessionError::BadParticipant(_))
    ));
    // a connection id from another session is not joined here
    let other = f.open("b.md", b"y\n");
    let elsewhere = other.join(agent("a"), Stay::UntilIdle).unwrap().conn;
    assert_eq!(
        s.edit(elsewhere, ins(0, "x"), None),
        Err(SessionError::NotJoined { conn: elsewhere })
    );
}

#[test]
fn rights_are_checked_on_every_call() {
    let f = fx("rights");
    let s = f.open("r.md", b"line one\nline two\n");
    let ed = f.join(&s, "ed", Rights::editor());
    let ro = f.join(&s, "ro", Rights::read_only());
    // no read: can't join
    assert_eq!(
        s.join(agent("nobody"), Stay::UntilIdle),
        Err(SessionError::Forbidden { need: Need::Read })
    );
    // a reader reads and is listed as a viewer, but can't edit or place a caret
    assert_eq!(text(&s, ro), "line one\nline two\n");
    assert_eq!(
        s.edit(ro, ins(0, "x"), None),
        Err(SessionError::Forbidden { need: Need::Edit })
    );
    assert_eq!(
        s.set_cursor(ro, ROOT, At::Byte(0), At::Byte(0)),
        Err(SessionError::Forbidden { need: Need::Edit })
    );
    assert!(matches!(
        s.write_base(ro, ROOT, 1, "y"),
        Err(SessionError::Forbidden { need: Need::Edit })
    ));
    assert!(matches!(
        s.undo(ro, 1),
        Err(SessionError::Forbidden { need: Need::Edit })
    ));
    assert!(matches!(
        s.edit_with(ro, ROOT, |t| Ok::<_, ()>(t.to_owned() + "x")),
        Err(EditWithError::Session(SessionError::Forbidden {
            need: Need::Edit
        }))
    ));
    s.set_view(
        ro,
        LineRange {
            root: ROOT.into(),
            first: 2,
            last: 9,
        },
    )
    .unwrap();
    s.edit(ed, ins(0, "# T\n"), None).unwrap();
    let roster = s.roster(ro).unwrap();
    let r = roster.iter().find(|e| e.participant.pid == "ro").unwrap();
    assert_eq!(r.role, Role::Viewer);
    assert_eq!(
        r.view.as_ref().map(|v| (v.first, v.last)),
        Some((3, 4)),
        "view follows the edit above it"
    );
    let ev = f.host.events();
    for action in [
        Action::Edit,
        Action::Cursor,
        Action::WriteBase,
        Action::Undo,
    ] {
        assert!(
            ev.iter().any(|e| matches!(e, AuditEvent::Refused { pid, action: a, need: Need::Edit, .. } if pid == "ro" && *a == action)),
            "no refusal event for {action:?}"
        );
    }
    // edit revoked mid-session: the very next edit is refused
    f.host.grant("ed", Rights::read_only());
    assert_eq!(
        s.edit(ed, ins(0, "x"), None),
        Err(SessionError::Forbidden { need: Need::Edit })
    );
    // read revoked: refused and removed
    f.host.grant("ro", Rights::none());
    assert_eq!(
        s.read(ro),
        Err(SessionError::Forbidden { need: Need::Read })
    );
    assert!(f.host.events().iter().any(|e| matches!(e, AuditEvent::Left { pid, reason: LeaveReason::RightsLost, .. } if pid == "ro")));
    assert_eq!(roster_pids(&s, ed), vec!["ed"]);
    assert_eq!(s.read(ro), Err(SessionError::NotJoined { conn: ro }));
    // the periodic re-check reports changes (once per pid) and removes who lost read
    let ed2 = f.join(&s, "ed", Rights::read_only());
    f.host.grant("ed", Rights::none());
    let ch = s.recheck_rights().unwrap();
    assert_eq!(ch.len(), 1);
    assert_eq!(ch[0].pid, "ed");
    assert_eq!(s.status().unwrap().connections, 0);
    assert!(matches!(s.touch(ed2), Err(SessionError::NotJoined { .. })));
    // events carry no content
    for e in f.host.events() {
        assert!(!format!("{e:?}").contains("line one"));
    }
}

#[test]
fn positional_edits_are_guarded_and_edit_with_is_atomic() {
    let f = fx_with("guard", burst(100));
    let s = f.open("g.md", b"alpha\nbeta\n");
    let a = f.join(&s, "a", Rights::editor());
    let b = f.join(&s, "b", Rights::editor());
    let r = s.read(a).unwrap();
    // hunks computed at r.version apply while the doc hasn't moved
    s.edit(a, ins(0, ">"), Some(r.version)).unwrap();
    // now it has: the same guard is stale, nothing applied
    let v = s.status().unwrap().version;
    assert_eq!(
        s.edit(a, ins(0, ">"), Some(r.version)),
        Err(SessionError::Stale {
            expected: r.version,
            actual: v
        })
    );
    assert_eq!(text(&s, a), ">alpha\nbeta\n");
    // edit_with computes on the live text and applies as one change
    replace(&s, b, "beta", "BETA");
    let e = s
        .edit_with(a, ROOT, |t| Ok::<_, ()>(t.replace("alpha", "ALPHA")))
        .unwrap()
        .unwrap();
    assert_eq!((e.lines_added, e.lines_removed), (1, 1));
    assert_eq!(text(&s, a), ">ALPHA\nBETA\n");
    // unchanged: None, no version
    let v = s.status().unwrap().version;
    assert_eq!(
        s.edit_with(a, ROOT, |t| Ok::<_, ()>(t.to_owned())).unwrap(),
        None
    );
    assert_eq!(s.status().unwrap().version, v);
    // the caller's own error comes back, nothing applied
    assert_eq!(
        s.edit_with(a, ROOT, |_| Err::<String, _>("no match")),
        Err(EditWithError::Compute("no match"))
    );
    assert_eq!(s.status().unwrap().version, v);
    // output is normalised by the pack
    s.edit_with(a, ROOT, |t| Ok::<_, ()>(format!("{t}x\r\n")))
        .unwrap()
        .unwrap();
    assert_eq!(text(&s, a), ">ALPHA\nBETA\nx\n");
    // a panic in the computation reaches the caller; the session stays usable
    let s2 = s.clone();
    let p = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = s2.edit_with(a, ROOT, |_| -> Result<String, ()> { panic!("tool bug") });
    }));
    assert!(p.is_err());
    assert_eq!(text(&s, a), ">ALPHA\nBETA\nx\n");
    s.edit(a, ins(0, "!"), None).unwrap();
    // an edit-verb's undo works like any other
    assert!(matches!(
        s.undo(a, 2).unwrap(),
        UndoOutcome::Applied { steps: 2, .. }
    ));
    assert_eq!(text(&s, a), ">ALPHA\nBETA\n");
}

#[test]
fn write_base_merges_conflicts_and_expires() {
    let mut c = Config::default();
    c.read_bases_max = 2;
    // stored versions are always valid bases: keep agent edits from cutting any here
    c.agent_version_lines = usize::MAX;
    c.agent_version_percent = usize::MAX;
    let f = fx_with("wbase", c);
    let s = f.open("w.md", b"alpha\nbeta\ngamma\n");
    let a = f.join(&s, "a", Rights::editor());
    let b = f.join(&s, "b", Rights::editor());
    let r = s.read(a).unwrap();
    let v0 = r.version;
    assert_eq!(v0, 1, "the import is version 1");
    // b edits elsewhere meanwhile
    replace(&s, b, "gamma", "GAMMA");
    // a's write from v0 merges onto it
    match s.write_base(a, ROOT, v0, "ALPHA\nbeta\ngamma\n").unwrap() {
        WriteOutcome::Applied(e) => {
            assert_eq!((e.lines_added, e.lines_removed), (1, 1));
            assert_eq!(
                e.changed
                    .iter()
                    .map(|c| (c.first, c.last))
                    .collect::<Vec<_>>(),
                vec![(1, 1)]
            );
        }
        o => panic!("{o:?}"),
    }
    assert_eq!(text(&s, a), "ALPHA\nbeta\nGAMMA\n");
    // an overlapping write from v0 conflicts and changes nothing
    let before = s.status().unwrap().version;
    match s.write_base(a, ROOT, v0, "alpha\nbeta\ngam\n").unwrap() {
        WriteOutcome::Conflicts(c) => {
            assert_eq!(c.len(), 1);
            assert_eq!(c[0].lines, (3, 3));
        }
        o => panic!("{o:?}"),
    }
    assert_eq!(s.status().unwrap().version, before);
    assert_eq!(text(&s, a), "ALPHA\nbeta\nGAMMA\n");
    // an unknown base is expired
    assert_eq!(
        s.write_base(a, ROOT, 999, "x").unwrap(),
        WriteOutcome::BaseExpired
    );
    // reads at newer versions push an older read out of a 2-entry cache
    let v_mid = s.read(b).unwrap().version;
    replace(&s, b, "beta", "b1");
    s.read(b).unwrap();
    replace(&s, b, "b1", "b2");
    s.read(b).unwrap();
    replace(&s, b, "b2", "b3");
    s.read(b).unwrap();
    assert_eq!(
        s.write_base(a, ROOT, v_mid, "zzz").unwrap(),
        WriteOutcome::BaseExpired
    );
    // ... and a base also expires with time
    let r = s.read(a).unwrap();
    replace(&s, b, "b3", "b4");
    f.host.advance(31 * 60_000);
    assert_eq!(
        s.write_base(a, ROOT, r.version, "q").unwrap(),
        WriteOutcome::BaseExpired
    );
    // stored versions stay valid bases: the import (v1) merges from the original text
    match s
        .write_base(a, ROOT, v0, "alpha\nbeta\ngamma\nadded\n")
        .unwrap()
    {
        WriteOutcome::Applied(_) => {}
        o => panic!("{o:?}"),
    }
    assert_eq!(text(&s, a), "ALPHA\nb4\nGAMMA\nadded\n");
    // write input is normalised by the pack
    let v = s.read(a).unwrap().version;
    s.write_base(a, ROOT, v, "ALPHA\r\nb4\nGAMMA\nadded\n")
        .unwrap();
    assert_eq!(text(&s, a), "ALPHA\nb4\nGAMMA\nadded\n");
}

#[test]
fn write_base_appends_at_the_end_both_land() {
    let f = fx_with("wbase-eof", burst(100));
    let s = f.open("log.md", b"# Log\n- one\n");
    let a = f.join(&s, "a", Rights::EDIT);
    let b = f.join(&s, "b", Rights::EDIT);
    // both agents read the same version, then each appends its own line from it
    let v = s.read(a).unwrap().version;
    assert_eq!(s.read(b).unwrap().version, v);
    for (c, line) in [(a, "- from a\n"), (b, "- from b\n")] {
        match s
            .write_base(c, ROOT, v, &format!("# Log\n- one\n{line}"))
            .unwrap()
        {
            WriteOutcome::Applied(e) => assert_eq!(e.lines_added, 1),
            o => panic!("EOF appends must both land (O5): {o:?}"),
        }
    }
    // theirs (a's, already live) first, then b's
    assert_eq!(text(&s, a), "# Log\n- one\n- from a\n- from b\n");
    // the same two inserts in the middle of the text still conflict
    let v = s.read(a).unwrap().version;
    s.write_base(a, ROOT, v, "# Log\nA\n- one\n- from a\n- from b\n")
        .unwrap();
    assert!(matches!(
        s.write_base(b, ROOT, v, "# Log\nB\n- one\n- from a\n- from b\n")
            .unwrap(),
        WriteOutcome::Conflicts(_)
    ));
}

#[test]
fn undo_applies_the_eof_rule_only_at_the_end_of_the_doc() {
    let f = fx_with("undo-eof", burst(100));
    let s = f.open("u.md", b"one\ntwo\nthree\n");
    let a = f.join(&s, "a", Rights::EDIT);
    let b = f.join(&s, "b", Rights::EDIT);
    // a deletes the last line, b then appends at the end: undoing a's deletion re-inserts
    // at the end of the doc, after b's append (O5: theirs first), as a whole-text merge would
    replace(&s, a, "three\n", "");
    s.edit(b, ins(text(&s, b).len(), "four\n"), None).unwrap();
    match s.undo(a, 1).unwrap() {
        UndoOutcome::Applied { steps: 1, .. } => {}
        o => panic!("{o:?}"),
    }
    assert_eq!(text(&s, a), "one\ntwo\nfour\nthree\n");
    // the same shape in the middle of the doc conflicts: the region ends there, not the doc
    replace(&s, a, "two\n", "");
    s.edit(b, ins(4, "2\n"), None).unwrap();
    assert_eq!(text(&s, a), "one\n2\nfour\nthree\n");
    assert!(matches!(
        s.undo(a, 1).unwrap(),
        UndoOutcome::Conflicts { step: 1, .. }
    ));
}

/// Regression (undo spec U10, element identity): an undo applies the clean merge's own
/// hunks. Two participants append the same line; each undoes its own. A re-diff of the
/// undone window would delete the other's identical line (the text looks right), and the
/// other's undo then has nothing left to remove: the doc kept a line both undid.
#[test]
fn f1_each_undoes_their_own_append() {
    let f = fx_with("undo-own", burst(100));
    let s = f.open("u.md", b"x\n");
    let a = f.join(&s, "a", Rights::EDIT);
    let b = f.join(&s, "b", Rights::EDIT);
    let w = f.join(&s, "w", Rights::READ);
    s.edit(a, ins(2, "a\n"), None).unwrap();
    s.edit(b, ins(4, "a\n"), None).unwrap();
    let rx = s.subscribe(w).unwrap();
    assert!(matches!(
        s.undo(a, 1).unwrap(),
        UndoOutcome::Applied { steps: 1, .. }
    ));
    let ev = rx.try_recv().unwrap();
    assert_eq!(
        ev.hunks,
        vec![Hunk {
            start: 2,
            end: 4,
            insert: String::new()
        }],
        "a's undo must delete a's line, the first of the two"
    );
    assert_eq!(text(&s, a), "x\na\n");
    assert!(matches!(
        s.undo(b, 1).unwrap(),
        UndoOutcome::Applied { steps: 1, .. }
    ));
    assert_eq!(text(&s, a), "x\n", "both undid their line");
    // and back: each redo re-inserts its own line
    s.redo(b, 1).unwrap();
    s.redo(a, 1).unwrap();
    assert_eq!(text(&s, a), "x\na\na\n");
}

#[test]
fn undo_redo_is_per_participant() {
    let f = fx_with("undo", burst(100));
    let s = f.open("u.md", b"one\ntwo\nthree\nfour\n");
    let a = f.join(&s, "a", Rights::editor());
    let b = f.join(&s, "b", Rights::editor());
    replace(&s, a, "two", "TWO");
    replace(&s, b, "four", "FOUR");
    // a's undo reverts only a's change
    match s.undo(a, 1).unwrap() {
        UndoOutcome::Applied {
            steps: 1, result, ..
        } => {
            assert_eq!((result.lines_added, result.lines_removed), (1, 1))
        }
        o => panic!("{o:?}"),
    }
    assert_eq!(text(&s, a), "one\ntwo\nthree\nFOUR\n");
    // redo restores it
    assert!(matches!(
        s.redo(a, 1).unwrap(),
        UndoOutcome::Applied { steps: 1, .. }
    ));
    assert_eq!(text(&s, a), "one\nTWO\nthree\nFOUR\n");
    // undo again, then a new edit clears the redo stack
    s.undo(a, 1).unwrap();
    replace(&s, a, "one", "ONE");
    assert_eq!(s.redo(a, 1).unwrap(), UndoOutcome::Nothing);
    // overlap: b edits inside what a changed, so a's undo conflicts and changes nothing
    replace(&s, a, "three", "THREE");
    replace(&s, b, "THREE", "THR3E");
    let v = s.status().unwrap().version;
    match s.undo(a, 1).unwrap() {
        UndoOutcome::Conflicts { step: 1, conflicts } => {
            assert_eq!(conflicts.len(), 1);
            assert_eq!(conflicts[0].lines.0, 3);
        }
        o => panic!("{o:?}"),
    }
    assert_eq!(s.status().unwrap().version, v);
    assert_eq!(text(&s, a), "ONE\ntwo\nTHR3E\nFOUR\n");
    // b's own undo works and leaves a's text alone
    s.undo(b, 1).unwrap();
    assert_eq!(text(&s, a), "ONE\ntwo\nTHREE\nFOUR\n");
    // the `E` it put back is a new element (U18a) whose root is a's `E` (U23): read by root,
    // a's region is as a left it (U8, v1.8), so a can undo both remaining batches in one
    // atomic call (regression (b): v1.7 conflicted here)
    match s.undo(a, 5).unwrap() {
        UndoOutcome::Applied {
            steps: 2, result, ..
        } => assert_eq!(result.lines_removed, 2),
        o => panic!("{o:?}"),
    }
    assert_eq!(text(&s, a), "one\ntwo\nthree\nFOUR\n");
    assert_eq!(s.undo(a, 1).unwrap(), UndoOutcome::Nothing);
    // two redos restore both
    assert!(matches!(
        s.redo(a, 2).unwrap(),
        UndoOutcome::Applied { steps: 2, .. }
    ));
    assert_eq!(text(&s, a), "ONE\ntwo\nTHREE\nFOUR\n");
    // undo is per pid: it survives leave/rejoin (until unload), on the new connection
    s.leave(a).unwrap();
    let a2 = f.join(&s, "a", Rights::editor());
    s.undo(a2, 1).unwrap();
    assert_eq!(text(&s, a2), "ONE\ntwo\nthree\nFOUR\n");
}

#[test]
fn undo_depth_is_bounded() {
    let mut c = burst(100);
    c.undo_depth = 3;
    let f = fx_with("undodepth", c);
    let s = f.open("d.md", b"");
    let a = f.join(&s, "a", Rights::editor());
    for i in 0..6 {
        let len = text(&s, a).len();
        s.edit(a, ins(len, &format!("{i}\n")), None).unwrap();
    }
    assert!(matches!(
        s.undo(a, 10).unwrap(),
        UndoOutcome::Applied { steps: 3, .. }
    ));
    assert_eq!(text(&s, a), "0\n1\n2\n");
}

#[test]
fn carets_follow_edits_and_convert_to_line_col() {
    let f = fx("caret");
    let s = f.open("k.md", "a\nbé🎉c\nd\n".as_bytes());
    let a = f.join(&s, "a", Rights::editor());
    let b = f.join(&s, "b", Rights::editor());
    let lc = |line, col| At::Line(LineCol { line, col });
    let c = s.set_cursor(a, ROOT, lc(2, 4), lc(2, 4)).unwrap();
    assert_eq!(&text(&s, a)[c.head..], "c\nd\n");
    assert!(
        matches!(
            s.set_cursor(a, ROOT, At::Byte(4), At::Byte(4)),
            Err(SessionError::BadPosition(_))
        ),
        "inside a character"
    );
    assert!(matches!(
        s.set_cursor(a, ROOT, lc(9, 1), At::Byte(0)),
        Err(SessionError::BadPosition(_))
    ));
    let caret = |s: &Session, pid: &str| {
        s.roster(a)
            .unwrap()
            .into_iter()
            .find(|e| e.participant.pid == pid)
            .unwrap()
            .cursor
            .unwrap()
    };
    // b inserts a line above: a's caret moves down with its text
    s.edit(b, ins(0, "new\n"), None).unwrap();
    let cur = caret(&s, "a");
    assert_eq!(cur.head, LineCol { line: 3, col: 4 });
    assert_eq!(&text(&s, a)[cur.cursor.head..], "c\nd\n");
    // b deletes the text around a's caret: it snaps to the deletion's start
    let t = text(&s, a);
    let at = t.find("é🎉c").unwrap();
    s.edit(
        b,
        vec![Edit::Text {
            root: ROOT.into(),
            hunks: vec![Hunk {
                start: at,
                end: at + "é🎉c".len(),
                insert: String::new(),
            }],
        }],
        None,
    )
    .unwrap();
    assert_eq!(caret(&s, "a").head, LineCol { line: 3, col: 2 });
    // after an agent edit, its caret sits at the end of its last hunk
    let e = s.edit(b, ins(0, "x\ny"), None).unwrap();
    assert_eq!(e.changed[0].first, 1);
    assert_eq!(caret(&s, "b").head, LineCol { line: 2, col: 2 });
}

#[test]
fn roster_roles_away_and_idle_expiry() {
    let f = fx_with("roster", burst(100));
    let s = f.open("p.md", b"hello\n");
    f.host.grant("tab", Rights::editor());
    let tab = s
        .join(who("tab", ParticipantKind::Person), Stay::Held)
        .unwrap()
        .conn;
    let ag = f.join(&s, "agent", Rights::editor());
    let reader = f.join(&s, "reader", Rights::read_only());
    let roles = |s: &Session| -> Vec<(String, Role, bool)> {
        s.roster(tab)
            .unwrap()
            .into_iter()
            .map(|e| (e.participant.pid, e.role, e.away))
            .collect()
    };
    // nobody active: all viewers, in join order
    assert_eq!(
        roles(&s),
        vec![
            ("tab".into(), Role::Viewer, false),
            ("agent".into(), Role::Viewer, false),
            ("reader".into(), Role::Viewer, false)
        ]
    );
    // an edit makes the agent an editor (listed first) for 30 s
    s.edit(ag, ins(0, "x"), None).unwrap();
    assert_eq!(roles(&s)[0], ("agent".into(), Role::Editor, false));
    f.host.advance(29_000);
    assert_eq!(roles(&s)[0].1, Role::Editor);
    f.host.advance(2_000);
    assert_eq!(roles(&s)[0].0, "tab");
    assert!(roles(&s).iter().all(|r| r.1 == Role::Viewer));
    // a caret makes a person an editor too
    s.set_cursor(tab, ROOT, At::Byte(0), At::Byte(1)).unwrap();
    assert_eq!(roles(&s)[0], ("tab".into(), Role::Editor, false));
    // hidden for 60 s: away, listed last
    s.set_visible(reader, false).unwrap();
    f.host.advance(59_000);
    assert!(!roles(&s).iter().any(|r| r.2));
    f.host.advance(1_000);
    assert_eq!(
        roles(&s).last().unwrap(),
        &("reader".to_string(), Role::Viewer, true)
    );
    s.set_visible(reader, true).unwrap();
    assert!(!roles(&s).iter().any(|r| r.2));
    // idle expiry: the agent and reader (no hold) leave after 60 s without a call; touch
    // keeps the agent; the held tab stays
    f.host.advance(30_000);
    s.touch(ag).unwrap();
    f.host.advance(30_000);
    assert_eq!(
        s.expire_idle().unwrap(),
        vec![(reader, "reader".to_string())]
    );
    f.host.advance(30_000);
    assert_eq!(s.expire_idle().unwrap(), vec![(ag, "agent".to_string())]);
    f.host.advance(10 * 60_000);
    assert!(s.expire_idle().unwrap().is_empty());
    assert_eq!(roles(&s).len(), 1);
    // a hold is sticky across a shared rejoin; set_stay drops it
    f.host.grant("cli", Rights::editor());
    let cli = s.join_shared(agent("cli"), Stay::Held).unwrap().conn;
    assert_eq!(
        s.join_shared(agent("cli"), Stay::UntilIdle).unwrap().conn,
        cli
    );
    f.host.advance(61_000);
    assert!(s.expire_idle().unwrap().is_empty());
    s.set_stay(cli, Stay::UntilIdle).unwrap();
    s.set_stay(tab, Stay::UntilIdle).unwrap();
    f.host.advance(61_000);
    let mut gone = s.expire_idle().unwrap();
    gone.sort();
    assert_eq!(
        gone,
        vec![(tab, "tab".to_string()), (cli, "cli".to_string())]
    );
    let ev = f.host.events();
    assert!(ev.iter().any(
        |e| matches!(e, AuditEvent::Left { pid, reason: LeaveReason::Idle, .. } if pid == "agent")
    ));
    // the manager drives the same expiry across sessions
    let late = f.join(&s, "late", Rights::read_only());
    f.host.advance(61_000);
    assert_eq!(
        f.mgr.expire_idle().unwrap(),
        vec![(key("p.md"), late, "late".to_string())]
    );
}

#[test]
fn edits_persist_and_compaction_keeps_them() {
    let mut c = burst(1_000);
    c.compact_entries = 10;
    let dir = TmpDir::new("persist");
    let host = TestHost::new();
    host.grant("a", Rights::editor());
    let (text_before, version_before) = {
        let store = Arc::new(DocStore::open(dir.db()).unwrap());
        let mgr = SessionManager::new(store.clone(), host.clone(), c.clone());
        let s = mgr.open(pack(), &key("n.md"), Some(b"start\n")).unwrap();
        let a = s.join(agent("a"), Stay::UntilIdle).unwrap().conn;
        for i in 0..9 {
            let len = text(&s, a).len();
            s.edit(a, ins(len, &format!("line {i}\n")), None).unwrap();
        }
        // import + 9 edits = 10 entries: compacted on the 10th
        let st = s.status().unwrap();
        assert_eq!(st.version, 10);
        assert_eq!(st.log_entries, 0, "compaction should have fired");
        let id = store.find(&key("n.md")).unwrap().unwrap();
        assert_eq!(store.log_size(id).unwrap(), (0, 0));
        s.edit(a, ins(0, "top\n"), None).unwrap();
        assert_eq!(s.status().unwrap().log_entries, 1);
        let r = s.read(a).unwrap();
        (r.text_by_root[0].1.clone(), r.version)
    };
    // everything dropped: reopen the store and the doc
    let store = Arc::new(DocStore::open(dir.db()).unwrap());
    let mgr = SessionManager::new(store, host.clone(), c);
    let s = mgr.open(pack(), &key("n.md"), None).unwrap();
    let a = s.join(agent("a"), Stay::UntilIdle).unwrap().conn;
    let r = s.read(a).unwrap();
    assert_eq!(r.text_by_root[0].1, text_before);
    assert_eq!(r.version, version_before);
    assert!(text_before.starts_with("top\nstart\nline 0\n"));
    // the next change continues the version sequence
    assert_eq!(
        s.edit(a, ins(0, "z"), Some(version_before))
            .unwrap()
            .version,
        version_before + 1
    );
    // no stored doc and no bytes: not found
    assert!(matches!(
        mgr.open(pack(), &key("missing.md"), None),
        Err(SessionError::NotFound(_))
    ));
    // a stored doc only opens with its own pack
    let other: Arc<dyn Pack> = Arc::new(TextFilePack::new("plain", 1 << 20));
    assert!(matches!(
        mgr.open(other.clone(), &key("n.md"), None),
        Err(SessionError::PackMismatch { .. })
    ));
    drop(s);
    mgr.release().unwrap();
}

#[test]
fn pool_unloads_idle_sessions_within_budget() {
    let mut c = Config::default();
    c.memory_budget_bytes = 1; // anything idle is over budget
    let f = fx_with("pool", c);
    f.host.grant("a", Rights::editor());
    let a = f.open("a.md", b"aaa\n");
    let ca = a.join(agent("a"), Stay::UntilIdle).unwrap().conn;
    a.edit(ca, ins(0, "edited "), None).unwrap();
    a.leave(ca).unwrap();
    drop(a);
    // opening b puts the pool over budget: idle a goes (b is being returned, so it stays)
    let b = f.open("b.md", b"bbb\n");
    assert!(f.mgr.get(&key("a.md")).is_none());
    let st = f.mgr.stats().unwrap();
    assert_eq!((st.loaded, st.unloaded_total, st.budget_bytes), (1, 1, 1));
    assert_eq!(st.estimated_bytes, b.status().unwrap().estimated_bytes);
    assert!(st.estimated_bytes > 0);
    // a session with a connection is never unloaded, even over budget
    let cb = b.join(agent("a"), Stay::UntilIdle).unwrap().conn;
    drop(b);
    let a = f.open("a.md", b"ignored: the store has it\n");
    assert!(f.mgr.get(&key("b.md")).is_some(), "b has a connection");
    // a reloaded identically
    let ca = a.join(agent("a"), Stay::UntilIdle).unwrap().conn;
    assert_eq!(text(&a, ca), "edited aaa\n");
    assert_eq!(a.status().unwrap().version, 2);
    let st = f.mgr.stats().unwrap();
    assert_eq!(st.loaded, 2);
    let sum = f
        .mgr
        .get(&key("a.md"))
        .unwrap()
        .status()
        .unwrap()
        .estimated_bytes
        + f.mgr
            .get(&key("b.md"))
            .unwrap()
            .status()
            .unwrap()
            .estimated_bytes;
    assert_eq!(st.estimated_bytes, sum);
    // a handle the host still holds keeps the session loaded
    a.leave(ca).unwrap();
    let b = f.mgr.get(&key("b.md")).unwrap();
    b.leave(cb).unwrap();
    f.host.advance(11 * 60_000);
    assert_eq!(f.mgr.unload_idle().unwrap(), 0, "both handles are held");
    drop(a);
    assert_eq!(f.mgr.unload_idle().unwrap(), 1);
    drop(b);
    assert_eq!(f.mgr.unload_idle().unwrap(), 1);
    assert_eq!(f.mgr.stats().unwrap().loaded, 0);
}

#[test]
fn idle_unload_uses_the_host_clock_and_compacts_long_logs() {
    let mut c = burst(100);
    c.unload_compact_entries = 3;
    let f = fx_with("idleunload", c);
    f.host.grant("a", Rights::editor());
    let s = f.open("x.md", b"x\n");
    let a = s.join(agent("a"), Stay::UntilIdle).unwrap().conn;
    for _ in 0..3 {
        s.edit(a, ins(0, "y"), None).unwrap();
    }
    s.leave(a).unwrap();
    drop(s);
    f.host.advance(9 * 60_000);
    assert_eq!(f.mgr.unload_idle().unwrap(), 0);
    f.host.advance(60_000);
    assert_eq!(f.mgr.unload_idle().unwrap(), 1);
    let id = f.store.find(&key("x.md")).unwrap().unwrap();
    assert_eq!(f.store.log_size(id).unwrap().0, 0, "compacted on unload");
    let s = f.mgr.open(pack(), &key("x.md"), None).unwrap();
    let a2 = s.join(agent("a"), Stay::UntilIdle).unwrap().conn;
    assert_eq!(text(&s, a2), "yyyx\n");
    assert_eq!(s.status().unwrap().version, 4);
    // the old connection id doesn't exist in the reloaded session
    assert!(matches!(s.read(a), Err(SessionError::NotJoined { .. })));
}

#[test]
fn closed_handles_fail_loudly() {
    let f = fx("closed");
    f.host.grant("a", Rights::editor());
    let s = f.open("x.md", b"x\n");
    let a = s.join(agent("a"), Stay::UntilIdle).unwrap().conn;
    f.mgr.release().unwrap();
    assert_eq!(s.read(a), Err(SessionError::Released));
    assert!(matches!(
        f.mgr.open(pack(), &key("x.md"), None),
        Err(SessionError::Released)
    ));
    drop(s);
    // with every handle gone the store file can be opened again (process hand-off)
    let Fx {
        dir, store, mgr, ..
    } = f;
    drop(mgr);
    drop(store);
    DocStore::open(dir.db()).unwrap();
}

#[test]
fn edit_rate_is_limited_on_the_host_clock() {
    let f = fx("rate");
    let s = f.open("r.md", b"");
    let a = f.join(&s, "a", Rights::editor());
    let a_tab = f.join(&s, "a", Rights::editor());
    let b = f.join(&s, "b", Rights::editor());
    for i in 0..10 {
        s.edit(if i % 2 == 0 { a } else { a_tab }, ins(0, "x"), None)
            .unwrap();
    }
    match s.edit(a, ins(0, "x"), None) {
        Err(SessionError::RateLimited { retry_after_ms }) => assert_eq!(retry_after_ms, 100),
        o => panic!("{o:?}"),
    }
    // per pid: another connection of the same pid is limited too; b is not
    assert!(matches!(
        s.edit(a_tab, ins(0, "x"), None),
        Err(SessionError::RateLimited { .. })
    ));
    s.edit(b, ins(0, "y"), None).unwrap();
    // leaving and rejoining doesn't reset it
    s.leave(a).unwrap();
    s.leave(a_tab).unwrap();
    let a = f.join(&s, "a", Rights::editor());
    assert!(matches!(
        s.undo(a, 1),
        Err(SessionError::RateLimited { .. })
    ));
    // it recovers with the clock
    f.host.advance(100);
    s.edit(a, ins(0, "x"), None).unwrap();
    assert!(matches!(
        s.edit(a, ins(0, "x"), None),
        Err(SessionError::RateLimited { .. })
    ));
    f.host.advance(1_000);
    for _ in 0..10 {
        s.edit(a, ins(0, "x"), None).unwrap();
    }
    assert_eq!(text(&s, a).len(), 22);
}

#[test]
fn declined_imports_say_why() {
    let f = fx("declined");
    let small: Arc<dyn Pack> = Arc::new(TextFilePack::new("markdown", 8));
    let cases: [(&[u8], &str); 4] = [
        (b"123456789", "too_large"),
        (b"\xff\xfe", "not_utf8"),
        (b"a\r\nb\n", "mixed_line_endings"),
        (b"a\0", "binary"),
    ];
    for (i, (bytes, code)) in cases.into_iter().enumerate() {
        let k = key(&format!("d{i}.md"));
        match f.mgr.open(small.clone(), &k, Some(bytes)) {
            Err(SessionError::Declined(d)) => {
                assert_eq!(d.code, code);
                assert!(!d.message.is_empty());
            }
            Err(e) => panic!("{code}: {e:?}"),
            Ok(_) => panic!("{code}: opened"),
        }
        assert_eq!(
            f.store.find(&k).unwrap(),
            None,
            "a declined file creates nothing"
        );
    }
}

#[test]
fn read_gives_the_files_bytes() {
    let f = fx("bytes");
    let raw = b"\xEF\xBB\xBF# Title\r\n\r\nbody\r\n";
    let s = f.open("crlf.md", raw);
    let a = f.join(&s, "a", Rights::editor());
    let r = s.read(a).unwrap();
    assert_eq!(r.bytes, raw);
    assert_eq!(r.text_by_root[0].1, "# Title\n\nbody\n");
    // edits with CRLF are normalised; the file keeps its style
    let len = r.text_by_root[0].1.len();
    s.edit(a, ins(len, "more\r\n"), None).unwrap();
    assert_eq!(
        s.read(a).unwrap().bytes,
        b"\xEF\xBB\xBF# Title\r\n\r\nbody\r\nmore\r\n"
    );
    // an empty file imports as an empty doc at version 0
    let e = f.open("empty.md", b"");
    let ea = f.join(&e, "a", Rights::read_only());
    let r = e.read(ea).unwrap();
    assert_eq!((r.bytes.as_slice(), r.version), (&b""[..], 0));
}

#[test]
fn contributors_since_last_read() {
    let f = fx("contrib");
    let s = f.open("c.md", b"x\n");
    let a = f.join(&s, "a", Rights::editor());
    let a2 = f.join(&s, "a", Rights::editor());
    let b = f.join(&s, "b", Rights::editor());
    let c = f.join(&s, "c", Rights::editor());
    assert!(
        s.read(a).unwrap().contributors_since_last_read.is_empty(),
        "import excluded"
    );
    s.edit(b, ins(0, "b"), None).unwrap();
    s.edit(a2, ins(0, "a"), None).unwrap();
    s.edit(c, ins(0, "c"), None).unwrap();
    s.edit(b, ins(0, "b"), None).unwrap();
    assert_eq!(
        s.read(a).unwrap().contributors_since_last_read,
        vec!["b", "c"],
        "the caller's own other connection doesn't count"
    );
    // per pid: a's other connection has read too
    assert!(s.read(a2).unwrap().contributors_since_last_read.is_empty());
}

#[test]
fn subscribers_get_every_change_in_order() {
    let mut c = Config::default();
    c.subscriber_queue = 3;
    let f = fx_with("subs", c);
    let s = f.open("s.md", b"abc\n");
    let w = f.join(&s, "w", Rights::read_only());
    let a = f.join(&s, "a", Rights::editor());
    let rx = s.subscribe(w).unwrap();
    let slow = s.subscribe(w).unwrap();
    let e1 = s.edit(a, ins(0, "1"), None).unwrap();
    let ev = rx.recv().unwrap();
    assert_eq!(
        (
            ev.version,
            ev.pid.as_str(),
            ev.via.as_str(),
            ev.root.as_str()
        ),
        (e1.version, "a", "cli", ROOT)
    );
    assert_eq!(
        ev.hunks,
        vec![Hunk {
            start: 0,
            end: 0,
            insert: "1".into()
        }]
    );
    for i in 2..=5 {
        s.edit(a, ins(0, &i.to_string()), None).unwrap();
        let ev = rx.recv().unwrap();
        assert_eq!(ev.version, e1.version + i - 1);
    }
    // the slow one got 3 then was cut off: it drains, then sees the end
    let got: Vec<u64> = slow.iter().map(|e| e.version).collect();
    assert_eq!(got, vec![e1.version, e1.version + 1, e1.version + 2]);
    // leaving closes the connection's subscriptions
    s.leave(w).unwrap();
    s.edit(a, ins(0, "z"), None).unwrap();
    assert!(rx.iter().next().is_none());
    // losing read closes them too
    let w = f.join(&s, "w", Rights::read_only());
    let rx = s.subscribe(w).unwrap();
    f.host.grant("w", Rights::none());
    assert_eq!(s.recheck_rights().unwrap().len(), 1);
    s.edit(a, ins(0, "z"), None).unwrap();
    assert!(rx.iter().next().is_none());
}

#[test]
fn rekey_moves_the_doc() {
    let f = fx("rekey");
    let s = f.open("old.md", b"x\n");
    let a = f.join(&s, "a", Rights::editor());
    f.mgr.rekey(&key("old.md"), key("new.md")).unwrap();
    assert_eq!(s.key().unwrap(), key("new.md"));
    assert!(f.mgr.get(&key("old.md")).is_none());
    assert!(Arc::ptr_eq(&f.mgr.get(&key("new.md")).unwrap(), &s));
    s.edit(a, ins(0, "y"), None).unwrap();
    assert_eq!(f.store.find(&key("old.md")).unwrap(), None);
    // not loaded: the store record moves
    let t = f.open("t.md", b"t\n");
    drop(t);
    f.mgr.release().unwrap();
    let store = f.store.clone();
    let mgr = SessionManager::new(store.clone(), f.host.clone(), Config::default());
    mgr.rekey(&key("t.md"), key("t2.md")).unwrap();
    assert!(store.find(&key("t2.md")).unwrap().is_some());
    assert!(matches!(
        mgr.rekey(&key("nope.md"), key("n2.md")),
        Err(SessionError::NotFound(_))
    ));
    assert!(matches!(
        mgr.rekey(&key("t2.md"), key("new.md")),
        Err(SessionError::Store(_))
    ));
}

#[test]
fn bad_edits_change_nothing() {
    let f = fx("bad");
    let s = f.open("b.md", "héllo\n".as_bytes());
    let a = f.join(&s, "a", Rights::editor());
    let v = s.status().unwrap().version;
    assert!(
        matches!(s.edit(a, ins(2, "x"), None), Err(SessionError::Doc(_))),
        "inside a char"
    );
    assert!(matches!(
        s.edit(a, ins(99, "x"), None),
        Err(SessionError::Doc(_))
    ));
    let twice = vec![
        Edit::Text {
            root: ROOT.into(),
            hunks: diff::hunks("", "a"),
        },
        Edit::Text {
            root: ROOT.into(),
            hunks: diff::hunks("", "b"),
        },
    ];
    assert!(matches!(
        s.edit(a, twice, None),
        Err(SessionError::BadEdit(_))
    ));
    assert!(matches!(
        s.edit(
            a,
            vec![Edit::Text {
                root: "nope".into(),
                hunks: vec![]
            }],
            None
        ),
        Err(SessionError::Doc(_))
    ));
    // a no-op edit changes no version
    let e = s.replace_text(a, ROOT, "héllo\n").unwrap();
    assert_eq!(e.version, v);
    assert!(e.changed.is_empty());
    assert_eq!(s.status().unwrap().version, v);
    assert_eq!(text(&s, a), "héllo\n");
}

// ---------- amendment 1: rights, on-behalf edits, approvals ----------

#[test]
fn rights_constructors() {
    let n = Rights::none();
    assert!(!n.read && !n.edit && !n.history);
    let r = Rights::read_only();
    assert!(r.read && !r.edit && !r.history, "history is never implied");
    let e = Rights::editor();
    assert!(e.read && e.edit && !e.history);
    let f = Rights::full();
    assert!(f.read && f.edit && f.history);
    assert_eq!(Rights::editor().with_history(true), Rights::full());
    assert_eq!(Rights::full().with_history(false), Rights::editor());
    assert_eq!(Rights::default(), Rights::none());
    assert_eq!(
        (Rights::NONE, Rights::READ, Rights::EDIT),
        (Rights::none(), Rights::read_only(), Rights::editor())
    );
}

fn helper_for(pid: &str, person: &str) -> Participant {
    agent(pid).on_behalf_of(person)
}

#[test]
fn on_behalf_edits_apply_at_once_and_carry_metadata() {
    let f = fx("obo");
    let s = f.open("o.md", b"one\ntwo\n");
    let person = f.join(&s, "alice", Rights::editor());
    let watch = s.subscribe(person).unwrap();
    f.host.grant("helper", Rights::editor());
    let h = s
        .join(helper_for("helper", "alice"), Stay::UntilIdle)
        .unwrap()
        .conn;
    replace(&s, h, "two", "TWO");
    // not held: it is live at once
    assert_eq!(text(&s, person), "one\nTWO\n");
    let ev = watch.recv().unwrap();
    assert_eq!(
        (ev.pid.as_str(), ev.on_behalf_of.as_deref()),
        ("helper", Some("alice"))
    );
    // logged with the metadata, approval pending
    let id = f.store.find(&key("o.md")).unwrap().unwrap();
    let (seq, e) = f.store.load(id).unwrap().updates.pop().unwrap();
    assert_eq!(
        (e.author.as_str(), e.on_behalf_of.as_deref()),
        ("helper", Some("alice"))
    );
    let a = s.approval(person, seq).unwrap().unwrap();
    assert_eq!(a.state, ApprovalState::Pending);
    assert_eq!(
        (a.author.as_str(), a.on_behalf_of.as_str()),
        ("helper", "alice")
    );
    // the person's own edit has none, and an event says so
    replace(&s, person, "one", "ONE");
    let ev = watch.recv().unwrap();
    assert_eq!(ev.on_behalf_of, None);
    // ...and saying so is history: without that right, only your own records are visible
    assert_eq!(
        s.approval(person, ev.version),
        Err(SessionError::Forbidden {
            need: Need::History
        })
    );
    f.host.grant("alice", Rights::full());
    assert_eq!(s.approval(person, ev.version).unwrap(), None);
    // presence shows "helper for alice"
    let roster = s.roster(person).unwrap();
    let row = roster
        .iter()
        .find(|r| r.participant.pid == "helper")
        .unwrap();
    assert_eq!(row.participant.on_behalf_of.as_deref(), Some("alice"));
    // the agent's undo is its own, and works as for anyone
    assert!(matches!(
        s.undo(h, 1).unwrap(),
        UndoOutcome::Applied { steps: 1, .. }
    ));
    assert_eq!(text(&s, person), "ONE\ntwo\n");
}

#[test]
fn on_behalf_needs_a_real_person_and_rights_are_the_agents() {
    let f = fx("obo-bad");
    let s = f.open("o.md", b"x\n");
    f.host.grant("helper", Rights::read_only());
    for bad in ["", "kit:import", "helper"] {
        assert!(
            matches!(
                s.join(helper_for("helper", bad), Stay::UntilIdle),
                Err(SessionError::BadParticipant(_))
            ),
            "{bad:?}"
        );
    }
    // acting for someone never widens the agent's own rights (the host is asked about the
    // agent, and here grants it read only)
    f.host.grant("alice", Rights::full());
    let h = s
        .join(helper_for("helper", "alice"), Stay::UntilIdle)
        .unwrap()
        .conn;
    assert_eq!(
        s.edit(h, ins(0, "x"), None),
        Err(SessionError::Forbidden { need: Need::Edit })
    );
    // shared joins don't mix principals: acting for another person is another connection
    f.host.grant("helper", Rights::editor());
    let a = s
        .join_shared(helper_for("helper", "alice"), Stay::UntilIdle)
        .unwrap();
    let a2 = s
        .join_shared(helper_for("helper", "alice"), Stay::UntilIdle)
        .unwrap();
    let b = s
        .join_shared(helper_for("helper", "sam"), Stay::UntilIdle)
        .unwrap();
    let own = s.join_shared(agent("helper"), Stay::UntilIdle).unwrap();
    assert_eq!(a.conn, a2.conn);
    assert_ne!(a.conn, b.conn);
    assert_ne!(a.conn, own.conn);
    assert_ne!(b.conn, own.conn);
}

#[test]
fn only_the_named_person_settles_an_approval_once() {
    let f = fx("approve");
    let s = f.open("o.md", b"one\n");
    let person = f.join(&s, "alice", Rights::editor());
    let other = f.join(&s, "sam", Rights::editor());
    let auditor = f.join(&s, "aud", Rights::read_only().with_history(true));
    f.host.grant("helper", Rights::editor());
    let h = s
        .join(helper_for("helper", "alice"), Stay::UntilIdle)
        .unwrap()
        .conn;
    replace(&s, h, "one", "uno");
    let seq = s.status().unwrap().version;
    // the agent can't settle it, nor can anyone else
    for c in [h, other, auditor] {
        assert_eq!(
            s.set_approval(c, seq, ApprovalState::Approved),
            Err(SessionError::Forbidden {
                need: Need::Approver
            })
        );
    }
    assert!(f.host.events().iter().any(|e| matches!(
        e,
        AuditEvent::Refused {
            action: Action::Approve,
            need: Need::Approver,
            ..
        }
    )));
    // reading it: the author and the person may; others need history
    assert!(s.approval(h, seq).unwrap().is_some());
    assert_eq!(
        s.approval(other, seq),
        Err(SessionError::Forbidden {
            need: Need::History
        })
    );
    assert!(s.approval(auditor, seq).unwrap().is_some());
    assert_eq!(s.approvals(other).unwrap(), vec![]);
    assert_eq!(s.approvals(auditor).unwrap().len(), 1);
    assert_eq!(s.approvals(h).unwrap().len(), 1);
    // pending is not a settlement
    assert!(matches!(
        s.set_approval(person, seq, ApprovalState::Pending),
        Err(SessionError::BadApproval(_))
    ));
    // the person declines: attributed, timed, audited (metadata only)
    f.host.advance(5);
    let now = f.host.now_ms();
    let d = s
        .set_approval(person, seq, ApprovalState::Declined)
        .unwrap();
    assert_eq!(
        (d.state, d.by.as_deref(), d.at_ms),
        (ApprovalState::Declined, Some("alice"), Some(now))
    );
    assert!(f.host.events().iter().any(|e| matches!(
        e,
        AuditEvent::Approval { seq: s2, pid, state: ApprovalState::Declined, .. }
            if *s2 == seq && pid == "alice"
    )));
    // once: the same again is a no-op, the other way is refused
    assert_eq!(
        s.set_approval(person, seq, ApprovalState::Declined)
            .unwrap(),
        d
    );
    assert!(matches!(
        s.set_approval(person, seq, ApprovalState::Approved),
        Err(SessionError::BadApproval(_))
    ));
    // a declined change stays applied (reversing it is an explicit act)
    assert_eq!(text(&s, person), "uno\n");
    // a change not made on anyone's behalf has no approval to set
    replace(&s, other, "uno", "un");
    let plain = s.status().unwrap().version;
    assert_eq!(
        s.set_approval(person, plain, ApprovalState::Approved),
        Err(SessionError::NoApproval { seq: plain })
    );
    // the record survives unload and reload
    drop(s);
    f.mgr.release().unwrap();
    let mgr = SessionManager::new(f.store.clone(), f.host.clone(), Config::default());
    let s = mgr.open(pack(), &key("o.md"), None).unwrap();
    let p = s.join(agent("alice"), Stay::UntilIdle).unwrap().conn;
    assert_eq!(s.approval(p, seq).unwrap(), Some(d));
}
