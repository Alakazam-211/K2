//! The roster push (`Session::subscribe_roster`): a snapshot first, then deltas coalesced over
//! `Config::roster_interval_ms`. Joins, leaves, claims and notes are never coalesced away;
//! carets collapse to their last place; a slow receiver gets a fresh snapshot, bounded by
//! `Config::roster_queue`; every subscriber sees the same order. Public API only.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use k2_mpc_base::session::{
    At, AuditEvent, ClaimEnd, ClaimPolicy, Config, ConnId, Host, LeaveReason, LineCol, LineRange,
    NoteKind, Participant, ParticipantKind, Rights, Role, RosterEvent, Session, SessionError,
    SessionManager, Stay, TextFilePack, Who,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::ToolOptions;

const ROOT: &str = TextFilePack::ROOT;
const DOC: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";
/// Past the default interval (250 ms).
const TICK: u64 = 300;

// ---------- harness ----------

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
            .unwrap_or(Rights::editor())
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

fn fx_with(tag: &str, config: Config) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-roster-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), config);
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

fn fx(tag: &str) -> Fx {
    fx_with(tag, Config::default())
}

fn who(pid: &str) -> Who {
    Who::Pid(pid.into())
}

fn lines(first: usize, last: usize) -> LineRange {
    LineRange {
        root: ROOT.into(),
        first,
        last,
    }
}

fn lc(line: usize, col: usize) -> At {
    At::Line(LineCol { line, col })
}

impl Fx {
    fn join(&self, pid: &str, kind: ParticipantKind) -> ConnId {
        let p = Participant::new(pid, pid.to_uppercase(), "#123456", kind, "tab");
        self.s.join(p, Stay::Held).unwrap().conn
    }
    fn person(&self, pid: &str) -> ConnId {
        self.join(pid, ParticipantKind::Person)
    }
    fn agent(&self, pid: &str) -> ConnId {
        self.join(pid, ParticipantKind::Agent)
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    /// Past the interval, then a flush (as the host's timer does).
    fn tick(&self) {
        self.advance(TICK);
        self.mgr.flush_rosters();
    }
}

/// Everything queued now, without waiting.
fn drain(rx: &Receiver<RosterEvent>) -> Vec<RosterEvent> {
    let mut v = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(e) => v.push(e),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return v,
        }
    }
}

/// The pids in a snapshot, in roster order.
fn snapshot_pids(ev: &RosterEvent) -> Vec<String> {
    match ev {
        RosterEvent::Snapshot { members, .. } => members
            .iter()
            .map(|m| m.who.pid().unwrap().to_owned())
            .collect(),
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

/// A short name per event, for whole-sequence assertions.
fn tag(ev: &RosterEvent) -> String {
    match ev {
        RosterEvent::Snapshot { members, .. } => format!("snapshot({})", members.len()),
        RosterEvent::Joined { who, .. } => format!("joined {who}"),
        RosterEvent::Left { who, reason } => format!("left {who} {reason:?}"),
        RosterEvent::Rights { who, .. } => format!("rights {who}"),
        RosterEvent::Role { who, role } => format!("role {who} {role:?}"),
        RosterEvent::Away { who, away } => format!("away {who} {away}"),
        RosterEvent::Cursor { who, cursor, .. } => format!(
            "cursor {who} {}",
            cursor.as_ref().map_or("none".to_owned(), |c| format!(
                "{}:{}",
                c.head.line, c.head.col
            ))
        ),
        RosterEvent::ClaimStarted { who, lines, .. } => {
            format!("claim {who} {}-{}", lines.first, lines.last)
        }
        RosterEvent::ClaimEnded { who, how, .. } => format!("claim-ended {who} {}", how.as_str()),
        RosterEvent::Note { to, from, kind, .. } => {
            format!("note {to}<-{from} {}", kind.as_str())
        }
        other => format!("{other:?}"),
    }
}

fn tags(evs: &[RosterEvent]) -> Vec<String> {
    evs.iter().map(tag).collect()
}

// ---------- tests ----------

/// The first event is the whole roster; joins and leaves follow as deltas.
#[test]
fn snapshot_first_then_joins_and_leaves() {
    let f = fx("join");
    let watcher = f.person("w");
    let a = f.agent("a");
    let rx = f.s.subscribe_roster(watcher).unwrap();
    let first = drain(&rx);
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(snapshot_pids(&first[0]), vec!["w", "a"]);
    if let RosterEvent::Snapshot {
        version,
        members,
        watching,
    } = &first[0]
    {
        assert_eq!(*watching, 0);
        assert_eq!(*version, f.s.status().unwrap().version);
        assert_eq!(members[1].who, who("a"));
        assert_eq!(members[1].rights, Rights::editor());
    }

    let b = f.person("b");
    f.s.leave(a).unwrap();
    // nothing before the interval
    f.mgr.flush_rosters();
    assert_eq!(drain(&rx), vec![], "coalesced: nothing before the interval");
    f.tick();
    assert_eq!(tags(&drain(&rx)), vec!["joined b", "left a Left"]);

    // a second connection of a present pid is no join; closing one of two is no leave
    let b2 = f.person("b");
    f.s.leave(b).unwrap();
    f.tick();
    assert_eq!(drain(&rx), vec![]);
    f.s.leave(b2).unwrap();
    f.tick();
    assert_eq!(tags(&drain(&rx)), vec!["left b Left"]);
}

/// A caret moved many times in one interval is one event, at its last place.
#[test]
fn cursor_moves_coalesce_to_the_last() {
    let f = fx("cursor");
    let watcher = f.person("w");
    let a = f.person("a");
    let rx = f.s.subscribe_roster(watcher).unwrap();
    drain(&rx);
    for col in 1..=5 {
        f.s.set_cursor(a, ROOT, lc(3, col), lc(3, col)).unwrap();
        f.advance(10);
    }
    f.tick();
    let evs = drain(&rx);
    let cursors: Vec<String> = tags(&evs)
        .into_iter()
        .filter(|t| t.starts_with("cursor"))
        .collect();
    assert_eq!(cursors, vec!["cursor a 3:5"], "{evs:?}");
    // placing a caret makes a person an editor
    assert!(tags(&evs).contains(&"role a Editor".to_owned()), "{evs:?}");
    // an unchanged caret says nothing
    f.s.set_cursor(a, ROOT, lc(3, 5), lc(3, 5)).unwrap();
    f.tick();
    let again: Vec<String> = tags(&drain(&rx))
        .into_iter()
        .filter(|t| t.starts_with("cursor"))
        .collect();
    assert_eq!(again, Vec::<String>::new());
}

/// Insert `x` at byte `at` as `c`; the version after.
fn type_at(f: &Fx, c: ConnId, at: usize, x: &str) -> k2_mpc_base::store::Seq {
    f.s.edit(
        c,
        vec![k2_mpc_base::Edit::Text {
            root: ROOT.into(),
            hunks: vec![k2_mpc_base::text::diff::Hunk {
                start: at,
                end: at,
                insert: x.into(),
            }],
        }],
        None,
    )
    .unwrap()
    .version
}

/// The live text of the root and its version, as `c` reads it.
fn read_text(f: &Fx, c: ConnId) -> (k2_mpc_base::store::Seq, String) {
    let r = f.s.read(c).unwrap();
    let t = r
        .text_by_root
        .into_iter()
        .find(|(root, _)| root == ROOT)
        .unwrap()
        .1;
    (r.version, t)
}

/// The caret's events, with their versions.
fn cursor_events(
    evs: &[RosterEvent],
) -> Vec<(
    Who,
    Option<k2_mpc_base::session::CursorAt>,
    k2_mpc_base::store::Seq,
)> {
    evs.iter()
        .filter_map(|e| match e {
            RosterEvent::Cursor {
                who,
                cursor,
                version,
                ..
            } => Some((who.clone(), cursor.clone(), *version)),
            _ => None,
        })
        .collect()
}

/// A caret's event carries the version its byte offsets are in: a caret set, then someone
/// else's edits before it land before the delivery; the delivered offsets are in the text at
/// the event's version (the doc's version then), not the one the caret was set at.
#[test]
fn a_cursor_event_carries_the_version_its_offsets_are_in() {
    let f = fx("cursor-version");
    let watcher = f.person("w");
    let a = f.person("a");
    let b = f.person("b");
    let rx = f.s.subscribe_roster(watcher).unwrap();
    drain(&rx);
    // a's caret on "three", line 3 col 1 ("one\ntwo\n" is 8 bytes)
    let (v0, t0) = read_text(&f, watcher);
    f.s.set_cursor(a, ROOT, lc(3, 1), lc(3, 3)).unwrap();
    assert_eq!(&t0[8..10], "th");
    // b types twice before it, inside the same interval (one with a multi-byte char)
    let v1 = type_at(&f, b, 0, "zero\n");
    let v2 = type_at(&f, b, 9, "é ");
    assert!(v0 < v1 && v1 < v2);
    let (now_v, now_t) = read_text(&f, watcher);
    assert_eq!(now_v, v2);
    f.tick();
    let evs = drain(&rx);
    // (b's own caret is at its last edit: only a's is checked here)
    let cs: Vec<_> = cursor_events(&evs)
        .into_iter()
        .filter(|(w, ..)| w == &who("a"))
        .collect();
    assert_eq!(cs.len(), 1, "{evs:?}");
    let (_, c, version) = &cs[0];
    // the version is the doc's at delivery, and the offsets are in its text
    assert_eq!(*version, v2, "{evs:?}");
    let c = c.as_ref().expect("a caret");
    let (s, e) = (c.cursor.anchor, c.cursor.head);
    assert!(
        now_t.is_char_boundary(s) && now_t.is_char_boundary(e),
        "{c:?} in {now_t:?}"
    );
    assert_eq!(&now_t[s..e], "th", "{c:?} in {now_t:?}");
    assert_eq!((c.anchor.line, c.anchor.col), (4, 1), "{c:?}");
    // and they are not the caret's original offsets (they moved with the edits)
    assert_ne!((s, e), (8, 10));
    // the snapshot's version is the one its carets are in, too
    let rx2 = f.s.subscribe_roster(watcher).unwrap();
    match drain(&rx2).remove(0) {
        RosterEvent::Snapshot {
            version, members, ..
        } => {
            assert_eq!(version, v2);
            let m = members.iter().find(|m| m.who == who("a")).unwrap();
            let c = m.entry.cursor.as_ref().unwrap();
            assert_eq!(&now_t[c.cursor.anchor..c.cursor.head], "th");
        }
        e => panic!("{e:?}"),
    }
    // an edit before the caret after a delivery: a new event, at the new version
    let v3 = type_at(&f, b, 0, "-");
    let (_, t3) = read_text(&f, watcher);
    f.tick();
    let cs: Vec<_> = cursor_events(&drain(&rx))
        .into_iter()
        .filter(|(w, ..)| w == &who("a"))
        .collect();
    assert_eq!(cs.len(), 1, "{cs:?}");
    let (_, c, version) = &cs[0];
    assert_eq!(*version, v3);
    let c = c.as_ref().unwrap();
    assert_eq!(&t3[c.cursor.anchor..c.cursor.head], "th");
}

/// Claims are never coalesced away: a claim taken and released inside one interval is two
/// events, in order, with lines and version; and so is a join and leave in one interval.
#[test]
fn claims_and_joins_inside_one_interval_are_all_delivered() {
    let f = fx("claims");
    let watcher = f.person("w");
    let a = f.agent("a");
    let rx = f.s.subscribe_roster(watcher).unwrap();
    drain(&rx);
    let v = f.s.status().unwrap().version;
    let c1 = f.s.claim(a, lines(2, 3), None, None, false).unwrap().id;
    f.s.release_claim(a, Some(c1)).unwrap();
    let c2 = f.s.claim(a, lines(5, 6), None, None, true).unwrap().id;
    let x = f.agent("x");
    f.s.leave(x).unwrap();
    f.tick();
    let evs = drain(&rx);
    assert_eq!(
        tags(&evs),
        vec![
            "claim a 2-3",
            "claim-ended a released",
            "claim a 5-6",
            "joined x",
            "left x Left"
        ]
    );
    match &evs[0] {
        RosterEvent::ClaimStarted {
            claim,
            version,
            hold,
            ..
        } => {
            assert_eq!((*claim, *version, *hold), (c1, v, false));
        }
        e => panic!("{e:?}"),
    }
    match &evs[2] {
        RosterEvent::ClaimStarted { claim, hold, .. } => assert_eq!((*claim, *hold), (c2, true)),
        e => panic!("{e:?}"),
    }
    // a claim that ends by leaving is delivered too
    f.s.leave(a).unwrap();
    f.tick();
    assert_eq!(tags(&drain(&rx)), vec!["claim-ended a left", "left a Left"]);
}

/// Many claims inside one interval: every one is delivered (up to the queue's bound).
#[test]
fn a_burst_of_claims_is_never_coalesced() {
    let f = fx("burst");
    let watcher = f.person("w");
    let a = f.agent("a");
    let rx = f.s.subscribe_roster(watcher).unwrap();
    drain(&rx);
    for n in 1..=10 {
        let id = f.s.claim(a, lines(n, n), None, None, false).unwrap().id;
        f.s.release_claim(a, Some(id)).unwrap();
    }
    f.tick();
    let evs = drain(&rx);
    let started = evs
        .iter()
        .filter(|e| matches!(e, RosterEvent::ClaimStarted { .. }))
        .count();
    let ended = evs
        .iter()
        .filter(|e| {
            matches!(
                e,
                RosterEvent::ClaimEnded {
                    how: ClaimEnd::Released,
                    ..
                }
            )
        })
        .count();
    assert_eq!((started, ended), (10, 10), "{:?}", tags(&evs));
}

/// A heads-up note delivered to a participant is pushed, addressed.
#[test]
fn a_delivered_note_is_pushed() {
    let f = fx("note");
    let watcher = f.person("w");
    let c = f.agent("c");
    let a = f.agent("a");
    let rx = f.s.subscribe_roster(watcher).unwrap();
    drain(&rx);
    f.s.claim(c, lines(3, 4), None, Some("rewriting".into()), false)
        .unwrap();
    f.advance(8_000);
    let argv: Vec<String> = ["view", "--lines", "3,4"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let out =
        f.s.run_tool_with(
            a,
            ROOT,
            &argv,
            b"",
            &ToolOptions::new().policy(ClaimPolicy::Ignore),
        )
        .unwrap();
    assert!(!out.notes.notes.is_empty(), "the call got a note");
    f.tick();
    let evs = drain(&rx);
    let note = evs
        .iter()
        .find_map(|e| match e {
            RosterEvent::Note {
                to,
                from,
                kind,
                lines,
                claim,
            } => Some((
                to.clone(),
                from.clone(),
                *kind,
                lines.clone(),
                claim.is_some(),
            )),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no note in {:?}", tags(&evs)));
    assert_eq!(
        note,
        (who("a"), who("c"), NoteKind::Claimed, lines(3, 4), true)
    );
}

/// Rights, away and back, and the role's decay are deltas too.
#[test]
fn rights_away_and_role_changes() {
    let mut cfg = Config::default();
    cfg.away_after_ms = 1_000;
    cfg.editor_window_ms = 5_000;
    let f = fx_with("away", cfg);
    let watcher = f.person("w");
    let a = f.person("a");
    let rx = f.s.subscribe_roster(watcher).unwrap();
    drain(&rx);
    f.s.set_cursor(a, ROOT, lc(1, 1), lc(1, 1)).unwrap();
    f.s.set_visible(a, false).unwrap();
    f.tick();
    assert_eq!(tags(&drain(&rx)), vec!["role a Editor", "cursor a 1:1"]);
    f.advance(1_000);
    f.mgr.flush_rosters();
    assert_eq!(tags(&drain(&rx)), vec!["away a true"]);
    f.s.set_visible(a, true).unwrap();
    f.tick();
    assert_eq!(tags(&drain(&rx)), vec!["away a false"]);
    f.advance(5_000);
    f.mgr.flush_rosters();
    assert_eq!(tags(&drain(&rx)), vec!["role a Viewer"]);
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("a".into(), Rights::read_only());
    f.s.recheck_rights().unwrap();
    f.tick();
    let evs = drain(&rx);
    assert!(
        evs.iter().any(|e| matches!(e, RosterEvent::Rights { who: w, rights } if *w == who("a") && *rights == Rights::read_only())),
        "{evs:?}"
    );
}

/// Every subscriber sees the same events in the same order.
#[test]
fn two_subscribers_see_the_same_order() {
    let f = fx("two");
    let w1 = f.person("w1");
    let w2 = f.person("w2");
    let rx1 = f.s.subscribe_roster(w1).unwrap();
    let rx2 = f.mgr.subscribe_roster(w2).unwrap();
    drain(&rx1);
    drain(&rx2);
    let a = f.agent("a");
    let b = f.person("b");
    for round in 0..5 {
        let id =
            f.s.claim(a, lines(1 + round, 2 + round), None, None, false)
                .unwrap()
                .id;
        f.s.set_cursor(b, ROOT, lc(2 + round, 1), lc(2 + round, 2))
            .unwrap();
        f.tick();
        f.s.release_claim(a, Some(id)).unwrap();
        f.tick();
    }
    f.s.leave(a).unwrap();
    f.tick();
    let e1 = drain(&rx1);
    let e2 = drain(&rx2);
    assert!(e1.len() > 15, "{:?}", tags(&e1));
    assert_eq!(e1, e2);
}

/// A subscriber that falls `roster_queue` events behind holds at most that many, then gets
/// a fresh snapshot once it has room: never unbounded memory, never a stale roster.
#[test]
fn a_slow_receiver_gets_a_fresh_snapshot() {
    let mut cfg = Config::default();
    cfg.roster_queue = 4;
    let f = fx_with("slow", cfg);
    let watcher = f.person("w");
    let rx = f.s.subscribe_roster(watcher).unwrap();
    let mut joined = Vec::new();
    // the snapshot takes one slot; then a join per flush, never read
    for n in 0..10 {
        joined.push(f.agent(&format!("a{n}")));
        f.tick();
    }
    let behind = drain(&rx);
    assert_eq!(
        behind.len(),
        4,
        "bounded by roster_queue: {:?}",
        tags(&behind)
    );
    assert_eq!(
        tags(&behind),
        vec!["snapshot(1)", "joined a0", "joined a1", "joined a2"]
    );
    // it has room now: the next flush brings the whole roster, not the joins it missed
    f.tick();
    let fresh = drain(&rx);
    assert_eq!(fresh.len(), 1, "{:?}", tags(&fresh));
    let mut want = vec!["w".to_owned()];
    want.extend((0..10).map(|n| format!("a{n}")));
    assert_eq!(snapshot_pids(&fresh[0]), want);
    // and deltas again after it
    f.s.leave(joined[0]).unwrap();
    f.tick();
    assert_eq!(tags(&drain(&rx)), vec!["left a0 Left"]);
}

/// More events in one interval than a queue holds: every subscriber resyncs from a snapshot.
#[test]
fn a_burst_past_the_queue_becomes_a_snapshot() {
    let mut cfg = Config::default();
    cfg.roster_queue = 8;
    let f = fx_with("overflow", cfg);
    let watcher = f.person("w");
    let a = f.agent("a");
    let rx = f.s.subscribe_roster(watcher).unwrap();
    drain(&rx);
    for n in 1..=10 {
        let id = f.s.claim(a, lines(n, n), None, None, false).unwrap().id;
        f.s.release_claim(a, Some(id)).unwrap();
    }
    f.tick();
    let evs = drain(&rx);
    assert_eq!(tags(&evs), vec!["snapshot(2)"]);
    // and deltas resume
    f.s.claim(a, lines(1, 1), None, None, false).unwrap();
    f.tick();
    assert_eq!(tags(&drain(&rx)), vec!["claim a 1-1"]);
}

/// Dropping the receiver unsubscribes; the subscriber's connection leaving closes it.
#[test]
fn unsubscribe_on_drop_and_close_on_leave() {
    let f = fx("drop");
    let w = f.person("w");
    let v = f.person("v");
    let rx = f.s.subscribe_roster(w).unwrap();
    drop(rx);
    f.agent("a");
    f.tick(); // the dead subscriber is dropped here; nothing else to see
    let rv = f.s.subscribe_roster(v).unwrap();
    assert_eq!(snapshot_pids(&drain(&rv)[0]), vec!["w", "v", "a"]);
    f.s.leave(v).unwrap();
    f.tick();
    assert!(matches!(rv.try_recv(), Err(TryRecvError::Disconnected)));
}

/// Read is needed; the manager finds the doc by connection.
#[test]
fn subscribing_needs_read_and_a_joined_connection() {
    let f = fx("rights");
    let w = f.person("w");
    f.s.leave(w).unwrap();
    assert!(matches!(
        f.mgr.subscribe_roster(w),
        Err(SessionError::NotJoined { .. })
    ));
    let r = f.person("r");
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("r".into(), Rights::none());
    assert!(matches!(
        f.s.subscribe_roster(r),
        Err(SessionError::Forbidden { .. })
    ));
}

/// The pushed roster is the roster: a snapshot equals `Session::roster`, row for row.
#[test]
fn the_snapshot_is_the_roster() {
    let f = fx("same");
    let w = f.person("w");
    let a = f.agent("a");
    let p = f.person("p");
    f.s.set_cursor(p, ROOT, lc(2, 1), lc(4, 1)).unwrap();
    f.s.claim(a, lines(6, 7), None, None, true).unwrap();
    let rows = f.s.roster(w).unwrap();
    let rx = f.s.subscribe_roster(w).unwrap();
    match drain(&rx).remove(0) {
        RosterEvent::Snapshot { members, .. } => {
            let entries: Vec<_> = members.into_iter().map(|m| m.entry).collect();
            assert_eq!(entries, rows);
            assert_eq!(rows[0].role, Role::Editor);
        }
        e => panic!("{e:?}"),
    }
    let _ = LeaveReason::Left;
}
