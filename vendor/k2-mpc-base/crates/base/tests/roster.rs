//! The roster push (`Session::subscribe_roster`): a snapshot first, then deltas coalesced over
//! `Config::roster_interval_ms`. Joins, leaves, claims and notes are never coalesced away;
//! carets collapse to their last place; a slow receiver gets a fresh snapshot, bounded by
//! `Config::roster_queue`; every subscriber sees the same order. Public API only.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use k2_mpc_base::session::{
    At, AuditEvent, ClaimEnd, ClaimPolicy, Config, ConnId, CursorAt, Host, JoinOptions, Joined,
    LeaveReason, LineCol, LineRange, NoteKind, Participant, ParticipantKind, Rights, Role,
    RosterEntry, RosterEvent, Session, SessionError, SessionManager, Stay, TextFilePack, Who,
    DEVICE_LABEL_MAX,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::ToolOptions;
use k2_mpc_base::ClientId;

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
        ..
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

// ---------- per-connection carets (v0.10.1) ----------
//
// The same person in two tabs: one roster row, a `Device` per connection with its own caret,
// and a `Cursor` event per connection (`client`). Run with `--features planted-device-bugs`
// (every connection shows the most active one's caret: "first connection only") these fail.

impl Fx {
    /// Another connection of `pid` (a person), with an optional label.
    fn tab(&self, pid: &str, label: Option<&str>) -> Joined {
        let p = Participant::new(
            pid,
            pid.to_uppercase(),
            "#123456",
            ParticipantKind::Person,
            "tab",
        );
        let opts = match label {
            Some(l) => JoinOptions::new().label(l),
            None => JoinOptions::new(),
        };
        self.s.join_with(p, Stay::Held, &opts).unwrap()
    }
    /// `pid`'s roster row as `viewer` sees it.
    fn row(&self, viewer: ConnId, pid: &str) -> RosterEntry {
        self.s
            .roster(viewer)
            .unwrap()
            .into_iter()
            .find(|e| e.participant.pid == pid)
            .unwrap_or_else(|| panic!("{pid} isn't on the roster"))
    }
}

/// A caret event of `a`'s: who, the connection, the head's `line:col`.
type CaretEvent = (String, Option<ClientId>, Option<(usize, usize)>);

/// A device as a test compares it: its client, its caret's head `line:col`, its label.
type DeviceRow<'a> = (ClientId, Option<(usize, usize)>, Option<&'a str>);

/// Each of `a`'s `Cursor` events (others' carets aside: a typist's caret moves too).
fn caret_events(evs: &[RosterEvent]) -> Vec<CaretEvent> {
    evs.iter()
        .filter_map(|e| match e {
            RosterEvent::Cursor {
                who: w,
                cursor,
                client,
                ..
            } if *w == who("a") => Some((
                w.to_string(),
                *client,
                cursor.as_ref().map(|c| (c.head.line, c.head.col)),
            )),
            _ => None,
        })
        .collect()
}

/// What a consumer that ignores `client` holds for `pid` after `evs`: the last event's caret
/// (`None` when there is no event: unchanged).
fn old_fold(evs: &[RosterEvent], pid: &str) -> Option<Option<CursorAt>> {
    evs.iter().rev().find_map(|e| match e {
        RosterEvent::Cursor { who, cursor, .. } if who.pid() == Some(pid) => Some(cursor.clone()),
        _ => None,
    })
}

fn head(c: &Option<CursorAt>) -> Option<(usize, usize)> {
    c.as_ref().map(|c| (c.head.line, c.head.col))
}

/// Two connections of one pid: each has its own `Device` with its own caret, in join order,
/// one roster row; `cursor` is still the most active connection's.
#[test]
fn two_connections_of_one_pid_each_get_their_own_device_caret() {
    let f = fx("devices");
    let w = f.person("w");
    let t1 = f.tab("a", None);
    let t2 = f.tab("a", Some("another tab"));
    f.s.set_cursor(t1.conn, ROOT, lc(2, 1), lc(2, 1)).unwrap();
    f.advance(10);
    f.s.set_cursor(t2.conn, ROOT, lc(5, 3), lc(5, 3)).unwrap();
    let rows = f.s.roster(w).unwrap();
    assert_eq!(
        rows.iter().filter(|e| e.participant.pid == "a").count(),
        1,
        "one row a person: {rows:?}"
    );
    let a = f.row(w, "a");
    assert_eq!(a.connections, 2);
    let got: Vec<DeviceRow<'_>> = a
        .devices
        .iter()
        .map(|d| (d.client, head(&d.cursor), d.label.as_deref()))
        .collect();
    assert_eq!(
        got,
        vec![
            (t1.client, Some((2, 1)), None),
            (t2.client, Some((5, 3)), Some("another tab")),
        ],
        "each connection's own caret, in join order"
    );
    // the per-person caret is the most active connection's (t2 moved last), as before
    assert_eq!(head(&a.cursor), Some((5, 3)));
    // the first tab moves: its device follows, the other stays
    f.advance(10);
    f.s.set_cursor(t1.conn, ROOT, lc(3, 2), lc(3, 2)).unwrap();
    let a = f.row(w, "a");
    assert_eq!(head(&a.devices[0].cursor), Some((3, 2)));
    assert_eq!(head(&a.devices[1].cursor), Some((5, 3)));
    assert_eq!(head(&a.cursor), Some((3, 2)));
    // the snapshot carries the same devices as the roster
    let rx = f.s.subscribe_roster(w).unwrap();
    match drain(&rx).remove(0) {
        RosterEvent::Snapshot { members, .. } => {
            let m = members.iter().find(|m| m.who == who("a")).unwrap();
            assert_eq!(m.entry.devices, a.devices);
        }
        e => panic!("{e:?}"),
    }
    // a second tab with no caret is a device with none
    let t3 = f.tab("a", None);
    let a = f.row(w, "a");
    assert_eq!(a.devices.len(), 3);
    assert_eq!(a.devices[2].client, t3.client);
    assert_eq!(a.devices[2].cursor, None);
}

/// A caret move on connection 2 is its own event, `client: Some(c2)`; connection 1's caret is
/// not re-sent. A consumer that ignores `client` still holds the person's caret
/// (`RosterEntry::cursor`) after every flush, a connection leaving with its caret included.
#[test]
fn a_caret_move_on_connection_2_emits_its_client() {
    let f = fx("device-events");
    let w = f.person("w");
    let t1 = f.tab("a", None);
    let t2 = f.tab("a", None);
    let rx = f.s.subscribe_roster(w).unwrap();
    drain(&rx);
    f.s.set_cursor(t1.conn, ROOT, lc(2, 1), lc(2, 1)).unwrap();
    f.tick();
    let evs = drain(&rx);
    assert_eq!(
        caret_events(&evs),
        vec![("a".to_owned(), Some(t1.client), Some((2, 1)))]
    );
    f.s.set_cursor(t2.conn, ROOT, lc(4, 2), lc(4, 2)).unwrap();
    f.tick();
    let evs = drain(&rx);
    assert_eq!(
        caret_events(&evs),
        vec![("a".to_owned(), Some(t2.client), Some((4, 2)))],
        "only connection 2's caret: {evs:?}"
    );
    assert_eq!(head(&old_fold(&evs, "a").unwrap()), Some((4, 2)));
    // both move in one interval: one event each, the most active connection's last
    f.s.set_cursor(t2.conn, ROOT, lc(6, 1), lc(6, 1)).unwrap();
    f.advance(10);
    f.s.set_cursor(t1.conn, ROOT, lc(7, 1), lc(7, 1)).unwrap();
    f.tick();
    let evs = drain(&rx);
    assert_eq!(
        caret_events(&evs),
        vec![
            ("a".to_owned(), Some(t2.client), Some((6, 1))),
            ("a".to_owned(), Some(t1.client), Some((7, 1))),
        ],
        "{evs:?}"
    );
    assert_eq!(
        old_fold(&evs, "a").unwrap(),
        f.row(w, "a").cursor,
        "the last event is the person's caret"
    );
    // the most active tab (t1) leaves with its caret: its caret goes (client t1, none), and a
    // `client: None` event gives the person's caret now (t2's), for a consumer that keeps one
    f.s.leave(t1.conn).unwrap();
    f.tick();
    let evs = drain(&rx);
    assert_eq!(
        caret_events(&evs),
        vec![
            ("a".to_owned(), Some(t1.client), None),
            ("a".to_owned(), None, Some((6, 1))),
        ],
        "{evs:?}"
    );
    let a = f.row(w, "a");
    assert_eq!(old_fold(&evs, "a").unwrap(), a.cursor);
    assert_eq!(a.devices.len(), 1);
    assert_eq!(a.devices[0].client, t2.client);
    // a caret carried through someone else's edit is that connection's event
    let b = f.person("b");
    type_at(&f, b, 0, "new first line\n");
    f.tick();
    let evs = drain(&rx);
    assert_eq!(
        caret_events(&evs),
        vec![("a".to_owned(), Some(t2.client), Some((7, 1)))],
        "{evs:?}"
    );
}

/// After every flush, a consumer that ignores `client` holds each person's
/// `RosterEntry::cursor`, over a run of moves, drops, joins and leaves on several tabs.
#[test]
fn a_consumer_ignoring_client_keeps_the_persons_caret() {
    let f = fx("device-old");
    let w = f.person("w");
    let rx = f.s.subscribe_roster(w).unwrap();
    drain(&rx);
    let mut tabs: Vec<Joined> = (0..3).map(|_| f.tab("a", None)).collect();
    let mut held: Option<CursorAt> = None;
    let mut seed = 7u64;
    for step in 0..120 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let r = (seed >> 33) as usize;
        let k = r % tabs.len().max(1);
        match r % 7 {
            _ if tabs.is_empty() => tabs.push(f.tab("a", None)),
            0 => {
                let t = tabs.remove(k);
                f.s.leave(t.conn).unwrap();
            }
            1 => tabs.push(f.tab("a", None)),
            2 => {
                let b = f.person("b");
                type_at(&f, b, (r / 7) % 20, "x");
            }
            _ => {
                let line = 1 + (r / 7) % 10;
                f.s.set_cursor(tabs[k].conn, ROOT, lc(line, 1), lc(line, 1))
                    .unwrap();
            }
        }
        f.advance(5);
        if step % 3 == 0 {
            f.tick();
            let evs = drain(&rx);
            // the old client's fold: joined and left start over, a caret event replaces
            for e in &evs {
                match e {
                    RosterEvent::Joined { who: x, .. } | RosterEvent::Left { who: x, .. }
                        if *x == who("a") =>
                    {
                        held = None
                    }
                    RosterEvent::Cursor { who: x, cursor, .. } if *x == who("a") => {
                        held = cursor.clone()
                    }
                    _ => {}
                }
            }
            let now =
                f.s.roster(w)
                    .unwrap()
                    .into_iter()
                    .find(|e| e.participant.pid == "a")
                    .and_then(|e| e.cursor);
            assert_eq!(held, now, "step {step}: {evs:?}");
        }
    }
}

/// One connection: the same events as before v0.10.1, each with its `client`, and never a
/// `client: None` one; its one device is its caret.
#[test]
fn a_single_connection_participant_is_unchanged() {
    let f = fx("device-one");
    let w = f.person("w");
    let a = f.tab("a", None);
    let rx = f.s.subscribe_roster(w).unwrap();
    drain(&rx);
    let mut all = Vec::new();
    for (line, col) in [(2, 1), (2, 3), (5, 1)] {
        f.s.set_cursor(a.conn, ROOT, lc(line, col), lc(line, col))
            .unwrap();
        f.tick();
        all.extend(drain(&rx));
    }
    let b = f.person("b");
    type_at(&f, b, 0, "x\n");
    f.tick();
    all.extend(drain(&rx));
    assert_eq!(
        caret_events(&all),
        vec![
            ("a".to_owned(), Some(a.client), Some((2, 1))),
            ("a".to_owned(), Some(a.client), Some((2, 3))),
            ("a".to_owned(), Some(a.client), Some((5, 1))),
            ("a".to_owned(), Some(a.client), Some((6, 1))),
        ]
    );
    let row = f.row(w, "a");
    assert_eq!(row.devices.len(), 1);
    assert_eq!(row.devices[0].client, a.client);
    assert_eq!(row.devices[0].cursor, row.cursor);
    assert_eq!(row.devices[0].label, None);
    // the caret goes: one event, none
    f.s.leave(a.conn).unwrap();
    let a2 = f.tab("a", None);
    f.s.set_cursor(a2.conn, ROOT, lc(1, 1), lc(1, 1)).unwrap();
    f.tick();
    let evs = drain(&rx);
    assert!(
        caret_events(&evs).iter().all(|(_, c, _)| c.is_some()),
        "{evs:?}"
    );
}

/// The host's label round-trips onto its connection's device, cleaned, and never fails a
/// join; a shared join with a label relabels its connection.
#[test]
fn the_join_label_round_trips() {
    let f = fx("device-label");
    let w = f.person("w");
    let t1 = f.tab("a", Some("Chrome on Mac"));
    let t2 = f.tab("a", Some("  phone\u{7}\n tab  "));
    let t3 = f.tab("a", Some(" \t "));
    let long = "x".repeat(DEVICE_LABEL_MAX + 10);
    let t4 = f.tab("a", Some(&long));
    let a = f.row(w, "a");
    let labels: Vec<(ClientId, Option<String>)> = a
        .devices
        .iter()
        .map(|d| (d.client, d.label.clone()))
        .collect();
    assert_eq!(
        labels,
        vec![
            (t1.client, Some("Chrome on Mac".to_owned())),
            (t2.client, Some("phone tab".to_owned())),
            (t3.client, None),
            (t4.client, Some("x".repeat(DEVICE_LABEL_MAX))),
        ]
    );
    // in the pushed snapshot too
    let rx = f.s.subscribe_roster(w).unwrap();
    match drain(&rx).remove(0) {
        RosterEvent::Snapshot { members, .. } => {
            let m = members.iter().find(|m| m.who == who("a")).unwrap();
            assert_eq!(m.entry.devices[0].label.as_deref(), Some("Chrome on Mac"));
        }
        e => panic!("{e:?}"),
    }
    // a shared join: the first makes its connection, a second with a label relabels it
    let p = Participant::new("c", "C", "#123456", ParticipantKind::Agent, "cli");
    let j1 =
        f.s.join_with(p.clone(), Stay::Held, &JoinOptions::new().shared(true))
            .unwrap();
    let j2 =
        f.s.join_with(
            p,
            Stay::Held,
            &JoinOptions::new().shared(true).label("terminal"),
        )
        .unwrap();
    assert_eq!(j1.conn, j2.conn);
    let c = f.row(w, "c");
    assert_eq!(c.devices.len(), 1);
    assert_eq!(c.devices[0].label.as_deref(), Some("terminal"));
}
