//! C7a: undo to spec v1.4 (regions per hunk, `--partial`, redo, `--drop`, `--for`),
//! versions (automatic cuts, named, retention, restore), the history reads and their right,
//! and `revert` with its window, carry, rights and the pending on-behalf cap. Public API
//! only. Test names carry the spec's CT names where one applies (`hist_*`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::session::{
    Action, ApprovalState, AuditEvent, ChangeKind, Config, ConflictKind, ConnId, Host, LogItem,
    LogQuery, Need, Pack, Participant, ParticipantKind, Rights, Session, SessionError,
    SessionManager, Stay, TextFilePack, UndoMode, UndoOutcome, UndoRequest, WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore, VersionReason};
use k2_mpc_base::tools::{exit, session_exit};

const MD: TextFilePack = TextFilePack::new("markdown", 4 << 20);
const ROOT: &str = TextFilePack::ROOT;
const DAY: u64 = 24 * 60 * 60_000;

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
            "k2mb-history-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
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
    config: Config,
    store: Option<Arc<DocStore>>,
    mgr: SessionManager,
}

fn config() -> Config {
    let mut c = Config::default();
    c.edit_burst = 100_000;
    c.edit_rate_per_s = 100_000;
    c
}

fn fx_with(tag: &str, config: Config) -> Fx {
    let dir = TmpDir::new(tag);
    let host = TestHost::new();
    let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store.clone(), host.clone(), config.clone());
    Fx {
        dir,
        host,
        config,
        store: Some(store),
        mgr,
    }
}

fn fx(tag: &str) -> Fx {
    fx_with(tag, config())
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

fn person(pid: &str) -> Participant {
    Participant::new(
        pid,
        pid.to_uppercase(),
        "#123456",
        ParticipantKind::Person,
        "tab",
    )
}

fn agent(pid: &str) -> Participant {
    Participant::new(
        pid,
        pid.to_uppercase(),
        "#654321",
        ParticipantKind::Agent,
        "cli",
    )
}

impl Fx {
    fn open(&self, path: &str, bytes: &[u8]) -> Arc<Session> {
        self.mgr.open(pack(), &key(path), Some(bytes)).unwrap()
    }
    fn join(&self, s: &Session, p: Participant, r: Rights) -> ConnId {
        self.host.grant(&p.pid, r);
        s.join(p, Stay::Held).unwrap().conn
    }
    /// Drop every session and the store, as a daemon restart does, and open them again.
    fn restart(&mut self) {
        self.mgr.release().unwrap();
        self.store = None;
        let store = Arc::new(DocStore::open(self.dir.0.join("docs.redb")).unwrap());
        self.mgr = SessionManager::new(store.clone(), self.host.clone(), self.config.clone());
        self.store = Some(store);
    }
    fn store(&self) -> &DocStore {
        self.store.as_ref().unwrap()
    }
    fn reopen(&self, path: &str) -> Arc<Session> {
        self.mgr.open(pack(), &key(path), None).unwrap()
    }
}

fn text(s: &Session, c: ConnId) -> String {
    s.read(c).unwrap().text_by_root.remove(0).1
}

fn set(s: &Session, c: ConnId, t: &str) -> u64 {
    s.replace_text(c, ROOT, t).unwrap().version
}

/// Replace the first `old` with `new` through `c`; returns the change's log position.
fn replace(s: &Session, c: ConnId, old: &str, new: &str) -> u64 {
    s.edit_with(c, ROOT, |t| match t.find(old) {
        Some(_) => Ok(t.replacen(old, new, 1)),
        None => Err(format!("{old:?} not in {t:?}")),
    })
    .unwrap()
    .unwrap_or_else(|| panic!("replace {old:?} changed nothing"))
    .version
}

fn partial() -> UndoRequest {
    UndoRequest::default().mode(UndoMode::Partial)
}

fn lines(n: usize, tag: &str) -> String {
    (1..=n).map(|i| format!("{tag} {i}\n")).collect()
}

// ---------- undo to spec v1.4 ----------

#[test]
fn hist_undo_last_line_after_foreign_append() {
    let f = fx("lastline");
    let s = f.open("l.md", b"a\nx\n");
    let a = f.join(&s, person("a"), Rights::editor());
    let b = f.join(&s, agent("b"), Rights::editor());
    set(&s, a, "a\n");
    set(&s, b, "a\ny\n");
    // the doc's base ("a\n") is non-empty: O5 lands the line after the append, cleanly
    let out = s.undo(a, 1).unwrap();
    assert!(
        matches!(out, UndoOutcome::Applied { steps: 1, .. }),
        "{out:?}"
    );
    assert_eq!(out.exit(), exit::OK);
    assert_eq!(text(&s, a), "a\ny\nx\n");
}

#[test]
fn hist_undo_whole_doc_delete_then_append_conflicts() {
    // spec v1.4 m11: the merge's base is the whole doc after the delete, ""
    let f = fx("wholedoc");
    let s = f.open("w.md", b"x\n");
    let a = f.join(&s, person("a"), Rights::editor());
    let b = f.join(&s, agent("b"), Rights::editor());
    set(&s, a, "");
    set(&s, b, "y\n");
    match s.undo(a, 1).unwrap() {
        UndoOutcome::Conflicts { step: 1, conflicts } => {
            assert_eq!(conflicts.len(), 1);
            assert_eq!(conflicts[0].kind, ConflictKind::Overlap);
        }
        o => panic!("{o:?}"),
    }
    assert_eq!(text(&s, a), "y\n");
}

#[test]
fn merge_empty_base_same_point_conflicts() {
    // K-BR-11: two write --base on an empty doc conflict
    let f = fx("emptybase");
    let s = f.open("e.md", b"");
    let a = f.join(&s, agent("a"), Rights::editor());
    let b = f.join(&s, agent("b"), Rights::editor());
    let v = s.read(a).unwrap().version;
    s.read(b).unwrap();
    assert!(matches!(
        s.write_base(a, ROOT, v, "from a\n").unwrap(),
        WriteOutcome::Applied(_)
    ));
    let out = s.write_base(b, ROOT, v, "from b\n").unwrap();
    assert!(matches!(out, WriteOutcome::Conflicts(_)), "{out:?}");
    assert_eq!(out.exit(), exit::NO);
    assert_eq!(WriteOutcome::BaseExpired.exit(), exit::TOO_OLD);
}

/// a changes lines 1 and 5 in one call (two regions); b then edits inside line 5.
fn two_region_fixture(f: &Fx) -> (Arc<Session>, ConnId, ConnId) {
    let s = f.open("p.md", b"one\ntwo\nthree\nfour\nfive\n");
    let a = f.join(&s, person("a"), Rights::editor());
    let b = f.join(&s, agent("b"), Rights::editor());
    set(&s, a, "ONE\ntwo\nthree\nfour\nFIVE\n");
    replace(&s, b, "FIVE", "FIxVE");
    (s, a, b)
}

#[test]
fn hist_undo_partial_and_reports_identical() {
    let f = fx("partial");
    let (s, a, _) = two_region_fixture(&f);
    let before = text(&s, a);
    // all or nothing: refused, exactly the second region reported, nothing applied
    let all = match s.undo(a, 1).unwrap() {
        UndoOutcome::Conflicts { step: 1, conflicts } => conflicts,
        o => panic!("{o:?}"),
    };
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].lines, (5, 5));
    assert_eq!(all[0].range, 19..24);
    assert_eq!(
        (all[0].theirs.as_str(), all[0].ours.as_str()),
        ("FIxVE", "five")
    );
    assert_eq!(text(&s, a), before);
    // --partial: applies the clean region and skips exactly the same set (U15)
    let out = s.undo_with(a, &partial()).unwrap();
    match &out {
        UndoOutcome::Applied {
            steps: 1,
            skipped,
            blocked: None,
            ..
        } => assert_eq!(*skipped, all),
        o => panic!("{o:?}"),
    }
    assert_eq!(out.exit(), exit::NO, "anything skipped exits 1");
    assert_eq!(text(&s, a), "one\ntwo\nthree\nfour\nFIxVE\n");
}

#[test]
fn hist_redo_after_partial() {
    let f = fx("redopartial");
    let (s, a, _) = two_region_fixture(&f);
    s.undo_with(a, &partial()).unwrap();
    // redo reapplies exactly what the undo applied: line 1 only
    let out = s.redo(a, 1).unwrap();
    assert!(
        matches!(out, UndoOutcome::Applied { steps: 1, .. }),
        "{out:?}"
    );
    assert_eq!(text(&s, a), "ONE\ntwo\nthree\nfour\nFIxVE\n");
    // and undo again reverts that again
    s.undo(a, 1).unwrap();
    assert_eq!(text(&s, a), "one\ntwo\nthree\nfour\nFIxVE\n");
}

#[test]
fn hist_undo_partial_stays_on_top_and_drop() {
    let f = fx("blocked");
    let s = f.open("b.md", b"alpha\nbeta\n");
    let a = f.join(&s, person("a"), Rights::editor());
    let b = f.join(&s, agent("b"), Rights::editor());
    replace(&s, a, "alpha", "ALPHA");
    let blocked_change = replace(&s, a, "beta", "BETA");
    replace(&s, b, "BETA", "BExTA");
    // the top step's only region conflicts: nothing applied, it stays on top (twice)
    for _ in 0..2 {
        match s.undo_with(a, &partial()).unwrap() {
            UndoOutcome::Blocked { conflicts } => assert_eq!(conflicts[0].lines, (2, 2)),
            o => panic!("{o:?}"),
        }
    }
    assert_eq!(text(&s, a), "ALPHA\nBExTA\n");
    // --drop removes it, unapplied, and the older step is reachable again
    assert_eq!(
        s.undo_with(a, &UndoRequest::default().mode(UndoMode::Drop))
            .unwrap(),
        UndoOutcome::Dropped {
            change: blocked_change
        }
    );
    assert!(matches!(
        s.undo(a, 1).unwrap(),
        UndoOutcome::Applied { steps: 1, .. }
    ));
    assert_eq!(text(&s, a), "alpha\nBExTA\n");
    // a dropped step isn't on the redo stack: redo gives back only the alpha change
    s.redo(a, 5).unwrap();
    assert_eq!(text(&s, a), "ALPHA\nBExTA\n");
}

#[test]
fn hist_undo_multi_step_partial_stops_at_a_blocked_step() {
    let f = fx("multistep");
    let s = f.open("m.md", b"a\nb\nc\n");
    let a = f.join(&s, person("a"), Rights::editor());
    let b = f.join(&s, agent("b"), Rights::editor());
    replace(&s, a, "a\n", "A\n");
    replace(&s, a, "b\n", "B\n");
    replace(&s, b, "B", "Z");
    replace(&s, a, "c\n", "C\n");
    // newest first: C undoes, then the B step is blocked and stays; A isn't reached
    match s
        .undo_with(a, &UndoRequest::steps(3).mode(UndoMode::Partial))
        .unwrap()
    {
        UndoOutcome::Applied {
            steps: 1,
            blocked: Some(c),
            ..
        } => assert_eq!(c.len(), 1),
        o => panic!("{o:?}"),
    }
    assert_eq!(text(&s, a), "A\nZ\nc\n");
    // all or nothing with 3 steps refuses at step 1 (now the blocked one)
    assert!(matches!(
        s.undo(a, 3).unwrap(),
        UndoOutcome::Conflicts { step: 1, .. }
    ));
    // two steps undone together are redone in order
    let s2 = f.open("m2.md", b"x\n");
    let p = f.join(&s2, person("p"), Rights::editor());
    set(&s2, p, "x\ny\n");
    set(&s2, p, "x\ny\nz\n");
    assert!(matches!(
        s2.undo(p, 2).unwrap(),
        UndoOutcome::Applied { steps: 2, .. }
    ));
    assert_eq!(text(&s2, p), "x\n");
    s2.redo(p, 2).unwrap();
    assert_eq!(text(&s2, p), "x\ny\nz\n");
}

#[test]
fn undo_for_person() {
    // K-HIST-21: an assistant's undo --for <person> undoes only edits made for them
    let f = fx("undofor");
    let s = f.open("f.md", b"one\ntwo\n");
    f.host.grant("asst", Rights::editor());
    let for_p = s
        .join(agent("asst").on_behalf_of("p"), Stay::Held)
        .unwrap()
        .conn;
    let for_q = s
        .join(agent("asst").on_behalf_of("q"), Stay::Held)
        .unwrap()
        .conn;
    replace(&s, for_p, "one", "ONE");
    replace(&s, for_q, "two", "TWO");
    // plain undo would undo q's (newest); --for p reaches past it
    let out = s
        .undo_with(for_q, &UndoRequest::default().for_person("p"))
        .unwrap();
    assert!(
        matches!(out, UndoOutcome::Applied { steps: 1, .. }),
        "{out:?}"
    );
    assert_eq!(text(&s, for_q), "one\nTWO\n");
    assert_eq!(
        s.undo_with(for_q, &UndoRequest::default().for_person("p"))
            .unwrap(),
        UndoOutcome::Nothing
    );
    s.undo(for_p, 1).unwrap();
    assert_eq!(text(&s, for_q), "one\ntwo\n");
}

#[test]
fn undo_survives_rejoin_not_unload() {
    // K-HIST-17, U6, U18
    let mut c = config();
    c.idle_unload_ms = 1_000;
    let f = fx_with("rejoin", c);
    let s = f.open("r.md", b"hello\n");
    let a = f.join(&s, person("a"), Rights::editor());
    replace(&s, a, "hello", "HELLO");
    replace(&s, a, "HELLO", "HELLO!");
    s.leave(a).unwrap();
    let a = f.join(&s, person("a"), Rights::editor());
    s.undo(a, 1).unwrap();
    assert_eq!(text(&s, a), "HELLO\n");
    s.leave(a).unwrap();
    drop(s);
    f.host.advance(2_000);
    assert_eq!(f.mgr.unload_idle().unwrap(), 1);
    let s = f.reopen("r.md");
    let a = f.join(&s, person("a"), Rights::editor());
    assert_eq!(s.undo(a, 1).unwrap(), UndoOutcome::HistoryLost);
    // someone who never edited has plainly nothing to undo
    let z = f.join(&s, person("z"), Rights::editor());
    assert_eq!(s.undo(z, 1).unwrap(), UndoOutcome::Nothing);
}

#[test]
fn undo_bounds_are_per_pid() {
    let mut c = config();
    assert_eq!((c.undo_depth, c.undo_max_bytes), (100, 4 << 20), "spec U6");
    c.undo_max_bytes = 600;
    let f = fx_with("bounds", c);
    let s = f.open("u.md", b"start\n");
    let a = f.join(&s, person("a"), Rights::editor());
    let b = f.join(&s, person("b"), Rights::editor());
    let big = "x".repeat(200);
    let at = text(&s, b).len();
    s.edit(
        b,
        vec![k2_mpc_base::Edit::Text {
            root: ROOT.into(),
            hunks: vec![k2_mpc_base::text::diff::Hunk {
                start: at,
                end: at,
                insert: "b\n".into(),
            }],
        }],
        None,
    )
    .unwrap();
    for i in 0..10 {
        let at = text(&s, a).len();
        s.edit(
            a,
            vec![k2_mpc_base::Edit::Text {
                root: ROOT.into(),
                hunks: vec![k2_mpc_base::text::diff::Hunk {
                    start: at,
                    end: at,
                    insert: format!("{i}{big}\n"),
                }],
            }],
            None,
        )
        .unwrap();
    }
    // a's oldest batches were dropped; b's batch is untouched by a's bytes
    match s.undo(a, 100).unwrap() {
        UndoOutcome::Applied { steps, .. } => assert!((1..10).contains(&steps), "{steps}"),
        o => panic!("{o:?}"),
    }
    assert!(matches!(
        s.undo(b, 1).unwrap(),
        UndoOutcome::Applied { steps: 1, .. }
    ));
}

#[test]
fn hist_undo_purge_clears() {
    let f = fx("purgeclears");
    let s = f.open("c.md", b"one\n");
    let a = f.join(&s, person("a"), Rights::editor());
    replace(&s, a, "one", "ONE");
    s.clear_undo_history().unwrap();
    assert_eq!(s.undo(a, 1).unwrap(), UndoOutcome::ClearedByPurge);
    assert_eq!(s.undo(a, 1).unwrap(), UndoOutcome::Nothing);
}

// ---------- revert ----------

#[test]
fn revert_named_change_after_many_edits() {
    // positions are carried through every later change (R4, m9)
    let f = fx("revertold");
    let s = f.open("v.md", lines(20, "line").as_bytes());
    let a = f.join(&s, person("a"), Rights::editor());
    let b = f.join(&s, agent("b"), Rights::editor());
    let change = replace(&s, a, "line 10\n", "line ten\n");
    for i in 0..150 {
        // b inserts at the top and appends at the end, around a's change
        s.edit_with(b, ROOT, |t| {
            Ok::<_, ()>(format!("top {i}\n{t}bottom {i}\n"))
        })
        .unwrap();
    }
    replace(&s, b, "line 9\n", "line nine\n"); // next to it, not overlapping
    let out = s.revert(a, change, false).unwrap();
    assert!(
        matches!(out, UndoOutcome::Applied { steps: 1, .. }),
        "{out:?}"
    );
    let t = text(&s, a);
    assert!(t.contains("line nine\nline 10\nline 11\n"), "{t}");
    assert!(!t.contains("line ten"));
    // the revert is the reverter's batch: their undo reverses it
    s.undo(a, 1).unwrap();
    assert!(text(&s, a).contains("line nine\nline ten\nline 11\n"));
    // log names it
    f.host.grant("a", Rights::full());
    let log = s.log(a, &LogQuery::default().by("a")).unwrap();
    let kinds: Vec<(ChangeKind, Option<u64>)> = log
        .iter()
        .filter_map(|i| match i {
            LogItem::Change { info, .. } => Some((info.kind, info.of)),
            _ => None,
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            (ChangeKind::Undo, None),
            (ChangeKind::Revert, Some(change)),
            (ChangeKind::Edit, None)
        ]
    );
}

#[test]
fn revert_record_taint_on_crossing_delete() {
    let f = fx("reverttaint");
    let s = f.open("t.md", b"aaa bbb ccc\n");
    let a = f.join(&s, person("a"), Rights::editor());
    let b = f.join(&s, agent("b"), Rights::editor());
    let change = replace(&s, a, "bbb", "BBB");
    replace(&s, b, "a BB", "a"); // deletes across the region's start
    match s.revert(a, change, false).unwrap() {
        UndoOutcome::Conflicts { step: 1, conflicts } => {
            assert_eq!(conflicts[0].kind, ConflictKind::Taint)
        }
        o => panic!("{o:?}"),
    }
    // --partial: a fully conflicting revert applies nothing either
    let out = s.revert(a, change, true).unwrap();
    assert!(matches!(out, UndoOutcome::Blocked { .. }), "{out:?}");
    assert_eq!(out.exit(), exit::NO);
    assert_eq!(text(&s, a), "aaaB ccc\n");
}

/// C7b: revert is as conservative as undo where two regions of a change came to touch,
/// someone inserted at the shared edge and a deletion then crossed it. Undo reports both
/// regions (Taint); C7a's revert missed the right one and, with `--partial`, applied it.
#[test]
fn revert_as_conservative_as_undo_at_a_crossed_shared_edge() {
    use k2_mpc_base::text::Hunk;
    use k2_mpc_base::Edit;
    let at = |s: usize, e: usize, t: &str| {
        vec![Edit::Text {
            root: ROOT.into(),
            hunks: vec![Hunk {
                start: s,
                end: e,
                insert: t.into(),
            }],
        }]
    };
    let f = fx("revertedge");
    let s = f.open("e.md", b"x--2abQ\n");
    let a = f.join(&s, person("a"), Rights::editor());
    let b = f.join(&s, agent("b"), Rights::editor());
    // a's change: two regions, the right one wider than what changed ("2abQ" -> "2abc")
    let change = s
        .edit(
            a,
            vec![Edit::Text {
                root: ROOT.into(),
                hunks: vec![
                    Hunk {
                        start: 0,
                        end: 1,
                        insert: "L".into(),
                    },
                    Hunk {
                        start: 3,
                        end: 7,
                        insert: "2abc".into(),
                    },
                ],
            }],
            None,
        )
        .unwrap()
        .version;
    s.edit(b, at(1, 3, ""), None).unwrap(); // "L2abc": the regions touch
    s.edit(b, at(1, 1, "Z"), None).unwrap(); // "LZ2abc": an insertion at the shared edge
    s.edit(b, at(1, 3, ""), None).unwrap(); // "Labc": a deletion across it
    assert_eq!(text(&s, a), "Labc\n");
    let kinds = |o: UndoOutcome| match o {
        UndoOutcome::Conflicts { conflicts, .. } | UndoOutcome::Blocked { conflicts } => conflicts
            .into_iter()
            .map(|c| (c.range, c.kind))
            .collect::<Vec<_>>(),
        o => panic!("{o:?}"),
    };
    let want = vec![(0..1, ConflictKind::Taint), (1..4, ConflictKind::Taint)];
    assert_eq!(kinds(s.revert(a, change, false).unwrap()), want);
    assert_eq!(kinds(s.revert(a, change, true).unwrap()), want); // --partial: Blocked
    assert_eq!(kinds(s.undo_with(a, &partial()).unwrap()), want);
    assert_eq!(text(&s, a), "Labc\n");
}

#[test]
fn hist_revert_declined_after_restart() {
    let mut f = fx("revertrestart");
    let (change, after_more) = {
        let s = f.open("d.md", b"# Plan\n- one\n- two\n");
        f.host.grant("asst", Rights::editor());
        let asst = s
            .join(agent("asst").on_behalf_of("p"), Stay::Held)
            .unwrap()
            .conn;
        let p = f.join(&s, person("p"), Rights::editor());
        let change = replace(&s, asst, "- two\n", "- two\n- three (assistant)\n");
        s.set_approval(p, change, ApprovalState::Declined).unwrap();
        replace(&s, p, "# Plan", "# The plan");
        (change, text(&s, p))
    };
    assert!(after_more.contains("three (assistant)"));
    f.restart();
    let s = f.reopen("d.md");
    let p = f.join(&s, person("p"), Rights::editor());
    // undo history is gone with the session; the record isn't
    assert_eq!(s.undo(p, 1).unwrap(), UndoOutcome::HistoryLost);
    let out = s.revert(p, change, false).unwrap();
    assert!(matches!(out, UndoOutcome::Applied { .. }), "{out:?}");
    assert_eq!(text(&s, p), "# The plan\n- one\n- two\n");
}

#[test]
fn revert_rights() {
    let f = fx("revertrights");
    let s = f.open("r.md", b"x\n");
    let a = f.join(&s, agent("a"), Rights::editor());
    let c = f.join(&s, person("c"), Rights::editor());
    let o = f.join(&s, person("o"), Rights::editor().with_owner(true));
    let change = replace(&s, a, "x", "y");
    let e = s.revert(c, change, false).unwrap_err();
    assert_eq!(
        e,
        SessionError::Forbidden {
            need: Need::AuthorOrOwner
        }
    );
    assert_eq!(session_exit(&e), exit::REFUSED);
    assert!(f.host.events.lock().unwrap().iter().any(|ev| matches!(
        ev,
        AuditEvent::Refused { pid, action: Action::Revert, need: Need::AuthorOrOwner, .. }
            if pid == "c"
    )));
    assert!(matches!(
        s.revert(o, change, false).unwrap(),
        UndoOutcome::Applied { .. }
    ));
    assert_eq!(text(&s, a), "x\n");
    // read-only: refused before anything (edit is needed)
    let r = f.join(&s, person("r"), Rights::read_only());
    assert_eq!(
        s.revert(r, change, false).unwrap_err(),
        SessionError::Forbidden { need: Need::Edit }
    );
    // no such change
    let e = s.revert(o, 999, false).unwrap_err();
    assert_eq!(e, SessionError::NoSuchChange(999));
    assert_eq!(session_exit(&e), exit::NOT_FOUND);
}

#[test]
fn revert_too_old_exit_8_and_window_retention() {
    let mut c = config();
    c.revert_keep_changes = 5;
    c.revert_keep_ms = 60_000;
    let mut f = fx_with("revertwindow", c);
    let (old, pending) = {
        let s = f.open("w.md", lines(10, "l").as_bytes());
        let a = f.join(&s, person("a"), Rights::editor());
        f.host.grant("asst", Rights::editor());
        let asst = s
            .join(agent("asst").on_behalf_of("a"), Stay::Held)
            .unwrap()
            .conn;
        let old = replace(&s, a, "l 2\n", "l two\n");
        let pending = replace(&s, asst, "l 5\n", "l five\n");
        for i in 0..20 {
            s.edit_with(a, ROOT, |t| Ok::<_, ()>(format!("{t}tail {i}\n")))
                .unwrap();
        }
        f.host.advance(2 * 60_000);
        let rep = f.mgr.tick().unwrap();
        assert!(rep.reverts_dropped >= 15, "{rep:?}");
        // outside the window: exit 8, never retry
        let e = s.revert(a, old, false).unwrap_err();
        assert_eq!(e, SessionError::RevertTooOld { seq: old });
        assert_eq!(session_exit(&e), exit::TOO_OLD);
        // a recent change is still revertable
        (old, pending)
    };
    // the pending on-behalf record is kept whatever the window says, across a restart,
    // from its persisted carried bounds
    f.restart();
    let s = f.reopen("w.md");
    let a = f.join(&s, person("a"), Rights::editor());
    for i in 0..5 {
        s.edit_with(a, ROOT, |t| Ok::<_, ()>(format!("head {i}\n{t}")))
            .unwrap();
    }
    assert!(matches!(
        s.revert(a, old, false),
        Err(SessionError::RevertTooOld { .. })
    ));
    let out = s.revert(a, pending, false).unwrap();
    assert!(matches!(out, UndoOutcome::Applied { .. }), "{out:?}");
    let t = text(&s, a);
    assert!(
        t.contains("l 4\nl 5\nl 6\n") && t.starts_with("head 4\n"),
        "{t}"
    );
}

#[test]
fn on_behalf_backlog_refuses_new() {
    let mut c = config();
    c.pending_max_records = 3;
    let f = fx_with("backlog", c);
    let s = f.open("o.md", b"a\nb\nc\nd\n");
    f.host.grant("asst", Rights::editor());
    let asst = s
        .join(agent("asst").on_behalf_of("p"), Stay::Held)
        .unwrap()
        .conn;
    let p = f.join(&s, person("p"), Rights::editor());
    let first = replace(&s, asst, "a\n", "A\n");
    replace(&s, asst, "b\n", "B\n");
    replace(&s, asst, "c\n", "C\n");
    let e = s.replace_text(asst, ROOT, "nope\n").unwrap_err();
    assert_eq!(
        e,
        SessionError::Held {
            reason: "approvals_backlog"
        }
    );
    assert_eq!(session_exit(&e), exit::HELD);
    // the person's own edits aren't held
    replace(&s, p, "d\n", "D\n");
    // settling one frees a slot
    s.set_approval(p, first, ApprovalState::Approved).unwrap();
    replace(&s, asst, "D\n", "D!\n");
}

#[test]
fn pending_on_behalf_survives_compaction() {
    // K-HIST-20: the revert record doesn't depend on the log
    let mut c = config();
    c.compact_entries = 5;
    let f = fx_with("compaction", c);
    let s = f.open("c.md", b"keep\nme\n");
    f.host.grant("asst", Rights::editor());
    let asst = s
        .join(agent("asst").on_behalf_of("p"), Stay::Held)
        .unwrap()
        .conn;
    let p = f.join(&s, person("p"), Rights::editor());
    let change = replace(&s, asst, "me\n", "ME\n");
    for i in 0..12 {
        s.edit_with(p, ROOT, |t| Ok::<_, ()>(format!("{t}{i}\n")))
            .unwrap();
    }
    let id = f.store().find(&key("c.md")).unwrap().unwrap();
    assert!(f
        .store()
        .load(id)
        .unwrap()
        .updates
        .iter()
        .all(|(q, _)| *q > change));
    s.revert(p, change, false).unwrap();
    assert!(text(&s, p).starts_with("keep\nme\n0\n"));
}

// ---------- versions ----------

fn reasons(f: &Fx, path: &str) -> Vec<VersionReason> {
    let id = f.store().find(&key(path)).unwrap().unwrap();
    f.store()
        .version_infos(id)
        .unwrap()
        .into_iter()
        .map(|v| v.reason)
        .collect()
}

#[test]
fn versions_auto_cut_rules() {
    let f = fx("autocut");
    let s = f.open("a.md", lines(200, "row").as_bytes());
    let p = f.join(&s, person("p"), Rights::full());
    let g = f.join(&s, agent("g"), Rights::editor());
    assert_eq!(reasons(&f, "a.md"), vec![VersionReason::Import]);
    // a person's edit, then 2 min of quiet: an idle version
    replace(&s, p, "row 1\n", "row one\n");
    f.host.advance(60_000);
    assert!(f.mgr.tick().unwrap().versions_cut.is_empty());
    f.host.advance(60_000);
    assert_eq!(f.mgr.tick().unwrap().versions_cut.len(), 1);
    assert!(
        f.mgr.tick().unwrap().versions_cut.is_empty(),
        "once per quiet spell"
    );
    // a small agent edit (1 of 200 lines) cuts nothing
    replace(&s, g, "row 2\n", "row two\n");
    assert_eq!(reasons(&f, "a.md").len(), 2);
    // an agent edit over 20 lines cuts a version of the doc before it
    let before = text(&s, p);
    let v_before = s.read(p).unwrap().version;
    s.edit_with(g, ROOT, |t| Ok::<_, ()>(t.replace("row 1", "ROW 1")))
        .unwrap();
    let infos = s.versions(p).unwrap();
    let last = infos.last().unwrap();
    assert_eq!(
        (last.reason, last.seq),
        (VersionReason::BeforeAgent, v_before)
    );
    assert_eq!(s.show(p, last.id).unwrap()[0].1, before);
    // a restore cuts one too
    s.restore(p, infos[0].id).unwrap();
    assert_eq!(
        reasons(&f, "a.md"),
        vec![
            VersionReason::Import,
            VersionReason::Idle,
            VersionReason::BeforeAgent,
            VersionReason::BeforeRestore
        ]
    );
}

#[test]
fn versions_restore_forward_and_undoable() {
    let f = fx("restore");
    let s = f.open("r.md", b"v1\n");
    let p = f.join(&s, person("p"), Rights::full());
    set(&s, p, "v2\n");
    let named = s.name_version(p, "draft two", None).unwrap();
    set(&s, p, "v3\n");
    let import = s.versions(p).unwrap()[0].id;
    let res = s.restore(p, import).unwrap();
    assert_eq!(text(&s, p), "v1\n");
    // forward: a new change by the restorer, history kept
    match &s.log(p, &LogQuery::default().limit(1)).unwrap()[0] {
        LogItem::Change { info, .. } => {
            assert_eq!(
                (info.kind, info.of, info.seq),
                (ChangeKind::Restore, Some(import), res.version)
            );
            assert_eq!(info.author, "p");
        }
        o => panic!("{o:?}"),
    }
    // undoable
    s.undo(p, 1).unwrap();
    assert_eq!(text(&s, p), "v3\n");
    s.restore(p, named).unwrap();
    assert_eq!(text(&s, p), "v2\n");
    // restore needs history (and edit)
    let e = f.join(&s, person("e"), Rights::editor());
    assert_eq!(
        s.restore(e, import).unwrap_err(),
        SessionError::Forbidden {
            need: Need::History
        }
    );
    assert_eq!(
        s.restore(p, 999).unwrap_err(),
        SessionError::NoSuchChange(999)
    );
}

#[test]
fn versions_retention_thinning_keeps_named() {
    let mut c = config();
    c.version_max_count = 4;
    let f = fx_with("thinning", c);
    let s = f.open("t.md", b"0\n");
    let p = f.join(&s, person("p"), Rights::full());
    let named = s.name_version(p, "keep me", None).unwrap();
    for i in 1..=8 {
        set(&s, p, &format!("{i}\n"));
        f.host.advance(3 * 60_000);
        f.mgr.tick().unwrap(); // an idle version each time, then retention
    }
    f.host.advance(DAY);
    let rep = f.mgr.tick().unwrap();
    let infos = s.versions(p).unwrap();
    assert!(infos.len() <= 4, "{infos:?} {rep:?}");
    assert!(infos
        .iter()
        .any(|v| v.id == named && v.name.as_deref() == Some("keep me")));
    // the newest stays
    assert_eq!(s.show(p, infos.last().unwrap().id).unwrap()[0].1, "8\n");
}

#[test]
fn history_right_gates_log_diff_show() {
    let f = fx("historyright");
    let s = f.open("h.md", b"one\ntwo\n");
    let h = f.join(&s, person("h"), Rights::full());
    let e = f.join(&s, person("e"), Rights::editor());
    replace(&s, h, "two", "TWO");
    let v = s.versions(h).unwrap()[0].id;
    let refused = SessionError::Forbidden {
        need: Need::History,
    };
    assert_eq!(s.log(e, &LogQuery::default()).unwrap_err(), refused);
    assert_eq!(s.versions(e).unwrap_err(), refused);
    assert_eq!(s.show(e, v).unwrap_err(), refused);
    assert_eq!(s.diff_version(e, ROOT, v, None, 3).unwrap_err(), refused);
    assert_eq!(session_exit(&refused), exit::REFUSED);
    // with the right
    let d = s.diff_version(h, ROOT, v, None, 3).unwrap();
    assert_eq!(d.exit(), exit::NO);
    assert!(d.text.contains("-two\n+TWO\n"), "{}", d.text);
    assert_eq!(
        s.diff_version(h, ROOT, v, Some(v), 3).unwrap().exit(),
        exit::OK
    );
    assert_eq!(
        s.show(h, v).unwrap(),
        vec![(ROOT.to_owned(), "one\ntwo\n".to_owned())]
    );
    assert_eq!(s.show(h, 77).unwrap_err(), SessionError::NoSuchChange(77));
}

#[test]
fn log_lists_changes_and_versions_with_filters() {
    let f = fx("log");
    let s = f.open("g.md", b"a\nb\nc\n");
    let p = f.join(&s, person("p"), Rights::full());
    f.host.grant("asst", Rights::editor());
    let asst = s
        .join(agent("asst").on_behalf_of("p"), Stay::Held)
        .unwrap()
        .conn;
    let t0 = f.host.now_ms();
    replace(&s, p, "a\n", "A\nA2\n");
    f.host.advance(1_000);
    let t1 = f.host.now_ms();
    let made = replace(&s, asst, "c\n", "C\n");
    let all = s.log(p, &LogQuery::default()).unwrap();
    // newest first: asst's change, the version cut before it (an agent edit of over 10% of
    // a small doc), p's change, the import version, the import change
    let kinds: Vec<String> = all
        .iter()
        .map(|i| match i {
            LogItem::Change { info, .. } => format!("{}:{:?}", info.author, info.kind),
            LogItem::Version(v) => format!("v:{:?}", v.reason),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "asst:Edit",
            "v:BeforeAgent",
            "p:Edit",
            "v:Import",
            "kit:import:Import"
        ]
    );
    match &all[2] {
        LogItem::Change { info, .. } => {
            assert_eq!((info.lines_added, info.lines_removed), (2, 1));
            assert_eq!((info.via.as_str(), info.at_ms), ("tab", t0));
        }
        o => panic!("{o:?}"),
    }
    let by = s.log(p, &LogQuery::default().by("asst")).unwrap();
    assert_eq!(by.len(), 1);
    let since = s.log(p, &LogQuery::default().since_ms(t1)).unwrap();
    assert_eq!(since.len(), 2, "asst's change and the version cut at it");
    assert_eq!(s.log(p, &LogQuery::default().limit(2)).unwrap().len(), 2);
    // log --pending --on-behalf-of p
    let pending = s
        .log(p, &LogQuery::default().pending().on_behalf_of("p"))
        .unwrap();
    match &pending[..] {
        [LogItem::Change { info, approval }] => {
            assert_eq!(info.seq, made);
            assert_eq!(*approval, Some(ApprovalState::Pending));
            assert_eq!(info.on_behalf_of.as_deref(), Some("p"));
        }
        o => panic!("{o:?}"),
    }
    s.set_approval(p, made, ApprovalState::Approved).unwrap();
    assert!(s.log(p, &LogQuery::default().pending()).unwrap().is_empty());
}
