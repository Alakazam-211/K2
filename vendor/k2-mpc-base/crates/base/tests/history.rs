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

thread_local! {
    /// This test's text is single letters, one token each (see [`letters`]).
    static LETTERS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The undo rules' worked cases on single letters (`ab`, `aXb`, `abcdefghij`): every
/// character a token, under which D1 (v1.10) is the identity, as in the oracle's worked
/// cases. Under words, such a text is one token and D1 would rewrite the cases' setups.
fn letters() {
    LETTERS.with(|c| c.set(true));
}

fn pack() -> Arc<dyn Pack> {
    if LETTERS.with(|c| c.get()) {
        Arc::new(MD.with_tokens(k2_mpc_base::text::lines::Tokens::Chars))
    } else {
        Arc::new(MD)
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

#[test]
fn hist_undo_purge_notice_outlives_an_idle_absence() {
    // U19: the notice is said on the holder's next undo, even after they left and the
    // session's idle sweep forgot their (empty) stacks
    let f = fx("purgeidle");
    let s = f.open("c.md", b"one\n");
    let a = f.join(&s, person("a"), Rights::editor());
    let b = f.join(&s, person("b"), Rights::editor());
    s.edit(
        a,
        vec![k2_mpc_base::Edit::Text {
            root: ROOT.into(),
            hunks: vec![k2_mpc_base::text::diff::Hunk {
                start: 0,
                end: 0,
                insert: "zero\n".into(),
            }],
        }],
        None,
    )
    .unwrap();
    s.leave(a).unwrap();
    s.clear_undo_history().unwrap();
    f.host.advance(60_000);
    s.expire_idle().unwrap();
    let a = f.join(&s, person("a"), Rights::editor());
    assert_eq!(s.undo(a, 1).unwrap(), UndoOutcome::ClearedByPurge);
    assert_eq!(s.undo(a, 1).unwrap(), UndoOutcome::Nothing);
    assert_eq!(
        s.undo(b, 1).unwrap(),
        UndoOutcome::Nothing,
        "b held nothing"
    );
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
    letters();
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

/// Undo spec U22a (v1.6), restart transparency (E-HIST-3): a pending on-behalf change's
/// two regions; a deletion crosses the left region's **start** only; the window moves
/// (the record's carry is persisted); the regions are then made to touch. The right region
/// inherits the left's **end** taint, which is clean, so revert conflicts on the left
/// region only, with or without a restart between the change and the revert and between
/// the later deletes. v0.8.1 persisted one flag per region and reloaded it at both edges:
/// after a restart the right region conflicted too.
#[test]
fn revert_after_restart_keeps_each_edges_taint() {
    letters();
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
    let run = |restarts: bool| {
        let mut c = config();
        c.revert_keep_changes = 3;
        c.revert_keep_ms = 0;
        c.history_tick_ms = 0;
        let mut f = fx_with(if restarts { "edgetaint-r" } else { "edgetaint" }, c);
        let s = f.open("e.md", b"abcdefghij\n");
        f.host.grant("asst", Rights::editor());
        let asst = s
            .join(agent("asst").on_behalf_of("p"), Stay::Held)
            .unwrap()
            .conn;
        let p = f.join(&s, person("p"), Rights::editor());
        let change = s
            .edit(
                asst,
                vec![Edit::Text {
                    root: ROOT.into(),
                    hunks: vec![
                        Hunk {
                            start: 1,
                            end: 3,
                            insert: "BC".into(),
                        },
                        Hunk {
                            start: 7,
                            end: 8,
                            insert: "H".into(),
                        },
                    ],
                }],
                None,
            )
            .unwrap()
            .version; // "aBCdefgHij\n": regions [1, 3) and [7, 8)
        s.edit(p, at(0, 2, ""), None).unwrap(); // across the left start only: "CdefgHij\n"
        for i in 0..5 {
            s.edit_with(p, ROOT, |t| Ok::<_, ()>(format!("{t}{i}\n")))
                .unwrap();
        }
        f.host.advance(1);
        let rep = f.mgr.tick().unwrap();
        assert!(rep.reverts_dropped >= 3, "the window moved: {rep:?}");
        let mut s = s;
        let mut p = p;
        let restart = |f: &mut Fx, s: &mut Arc<Session>, p: &mut ConnId| {
            if restarts {
                f.restart();
                *s = f.reopen("e.md");
                *p = f.join(s, person("p"), Rights::editor());
            }
        };
        restart(&mut f, &mut s, &mut p);
        s.edit(p, at(1, 3, ""), None).unwrap(); // "CfgHij…"
        restart(&mut f, &mut s, &mut p);
        s.edit(p, at(1, 3, ""), None).unwrap(); // "CHij…": the regions touch
        restart(&mut f, &mut s, &mut p);
        let all = s.revert(p, change, false).unwrap();
        let part = s.revert(p, change, true).unwrap();
        (all, part, text(&s, p))
    };
    let want = run(false);
    match &want.0 {
        UndoOutcome::Conflicts { step: 1, conflicts } => {
            let got: Vec<_> = conflicts
                .iter()
                .map(|c| (c.range.clone(), c.kind))
                .collect();
            assert_eq!(got, vec![(0..1, ConflictKind::Taint)]);
        }
        o => panic!("{o:?}"),
    }
    assert!(
        matches!(&want.1, UndoOutcome::Applied { skipped, .. } if skipped.len() == 1),
        "{:?}",
        want.1
    );
    assert!(want.2.starts_with("Chij\n"), "{}", want.2);
    assert_eq!(run(true), want, "a restart changed the revert's outcome");
}

// ---------- exact hunk sets (undo spec v1.7, U10; finding F3) ----------

/// One hunk on the root, as an explicit `edit`.
fn hunk_at(s: &Session, c: ConnId, start: usize, end: usize, insert: &str) -> u64 {
    use k2_mpc_base::text::Hunk;
    use k2_mpc_base::Edit;
    let e = Edit::Text {
        root: ROOT.into(),
        hunks: vec![Hunk {
            start,
            end,
            insert: insert.into(),
        }],
    };
    s.edit(c, vec![e], None).unwrap().version
}

/// The live elements as `(pid, client, clock, root pid)`; with feature `conformance` only.
#[cfg(feature = "conformance")]
fn elements(s: &Session) -> Vec<(Option<String>, u64, u32, Option<String>)> {
    s.char_blame(ROOT)
        .unwrap()
        .into_iter()
        .map(|b| (b.pid, b.client, b.clock, b.root_pid))
        .collect()
}

/// Run `op`; every element it removed must be one whose **root** is `who`'s, or one `who`
/// put back itself (a redo removing what its undo restored): `hist.undo_own_only` by root
/// (undo spec v1.8, U10a).
fn own_only<T>(s: &Session, who: &str, op: impl FnOnce() -> T) -> T {
    #[cfg(feature = "conformance")]
    let before = elements(s);
    let out = op();
    #[cfg(feature = "conformance")]
    {
        let after = elements(s);
        for e in before.iter().filter(|e| !after.contains(e)) {
            assert!(
                e.3.as_deref() == Some(who) || e.0.as_deref() == Some(who),
                "{who} removed {e:?}, neither its root nor its own restoration"
            );
        }
    }
    let _ = (s, who);
    out
}

/// I1 (undo spec v1.8): at most one live element per provenance root; with feature
/// `conformance` only.
fn assert_i1(s: &Session) {
    #[cfg(feature = "conformance")]
    {
        let mut seen = std::collections::HashSet::new();
        for b in s.char_blame(ROOT).unwrap() {
            assert!(
                seen.insert((b.root_client, b.root_clock)),
                "I1: two live elements of root {:?}: {:?}",
                (b.root_client, b.root_clock),
                s.char_blame(ROOT).unwrap()
            );
        }
    }
    let _ = s;
}

/// The root pid of each live character; with feature `conformance` only (else empty).
fn root_pids(s: &Session) -> Vec<(char, Option<String>)> {
    #[cfg(feature = "conformance")]
    return s
        .char_blame(ROOT)
        .unwrap()
        .into_iter()
        .map(|b| (b.ch, b.root_pid))
        .collect();
    #[cfg(not(feature = "conformance"))]
    {
        let _ = s;
        Vec::new()
    }
}

fn applied(o: UndoOutcome) {
    assert!(
        matches!(o, UndoOutcome::Applied { ref skipped, .. } if skipped.is_empty()),
        "{o:?}"
    );
}

/// F3 (v0.8.2's finding): p2's insertion at the start of p1's region repeats its text. p1's
/// undo must turn back **its own** `b`, leaving p2's, so p2's undo then applies cleanly.
/// A merge on diffed sides put p2's `b` after the region's own and deleted it.
#[test]
fn undo_exact_hunks_f3_insertion_at_a_region_start() {
    letters();
    let f = fx("f3");
    let s = f.open("f3.md", b"ab");
    let p1 = f.join(&s, agent("p1"), Rights::editor());
    let p2 = f.join(&s, agent("p2"), Rights::editor());
    hunk_at(&s, p1, 0, 1, "b"); // "bb"
    hunk_at(&s, p2, 0, 0, "b"); // "bbb": p2's b first
    applied(own_only(&s, "p1", || s.undo(p1, 1).unwrap()));
    assert_eq!(text(&s, p1), "bab", "p2's b stays in front");
    applied(own_only(&s, "p2", || s.undo(p2, 1).unwrap()));
    assert_eq!(text(&s, p1), "ab");
    applied(own_only(&s, "p2", || s.redo(p2, 1).unwrap()));
    applied(own_only(&s, "p1", || s.redo(p1, 1).unwrap()));
    assert_eq!(text(&s, p1), "bbb");
}

/// Two identical lines: p2 inserts `a\n` right at the start of p1's appended `a\n`. Each
/// undo removes its own line, by undo and by revert.
#[test]
fn undo_exact_hunks_repeated_lines() {
    for by_revert in [false, true] {
        let f = fx(if by_revert {
            "rep-lines-r"
        } else {
            "rep-lines"
        });
        let s = f.open("l.md", b"x\n");
        let p1 = f.join(&s, agent("p1"), Rights::editor());
        let p2 = f.join(&s, agent("p2"), Rights::editor());
        let c1 = hunk_at(&s, p1, 2, 2, "a\n"); // "x\na\n"
        hunk_at(&s, p2, 2, 2, "a\n"); // "x\na\na\n": p2's line first
        if by_revert {
            applied(own_only(&s, "p1", || s.revert(p1, c1, false).unwrap()));
        } else {
            applied(own_only(&s, "p1", || s.undo(p1, 1).unwrap()));
        }
        assert_eq!(text(&s, p1), "x\na\n");
        applied(own_only(&s, "p2", || s.undo(p2, 1).unwrap()));
        assert_eq!(
            text(&s, p1),
            "x\n",
            "both lines gone (by revert: {by_revert})"
        );
    }
}

/// A run `aaaa` grown by two participants at the same spot, undone each by its own, and a
/// replaced run: `aa` → p1 replaces the second `a` with `aa`, p2 types `a` in front of it.
#[test]
fn undo_exact_hunks_a_run() {
    letters();
    let f = fx("run");
    let s = f.open("r.md", b"aa");
    let p1 = f.join(&s, agent("p1"), Rights::editor());
    let p2 = f.join(&s, agent("p2"), Rights::editor());
    hunk_at(&s, p1, 1, 1, "a"); // "aaa": p1's at 1
    hunk_at(&s, p2, 1, 1, "a"); // "aaaa": p2's at 1, p1's at 2
    applied(own_only(&s, "p1", || s.undo(p1, 1).unwrap()));
    applied(own_only(&s, "p2", || s.undo(p2, 1).unwrap()));
    assert_eq!(text(&s, p1), "aa");
    applied(own_only(&s, "p2", || s.redo(p2, 1).unwrap()));
    applied(own_only(&s, "p1", || s.redo(p1, 1).unwrap()));
    assert_eq!(text(&s, p1), "aaaa");
    // a replacement: p1's undo puts the seed's `a` back and leaves p2's `a` in front
    let c = hunk_at(&s, p1, 1, 2, "bb"); // "abbaa"
    hunk_at(&s, p2, 1, 1, "b"); // "abbbaa": p2's b in front of p1's
    applied(own_only(&s, "p1", || s.revert(p1, c, false).unwrap()));
    assert_eq!(text(&s, p1), "abaaa");
    applied(own_only(&s, "p2", || s.undo(p2, 1).unwrap()));
    assert_eq!(text(&s, p1), "aaaa");
}

// ---------- provenance: restored text by root (undo spec v1.8, U23) ----------

/// Regression (a): a change undone and redone has its roots live again (the redo put them
/// back), so its revert applies (v1.7 conflicted: the redo's elements were new). With and
/// without a restart between the redo and the revert (roots come from the store then).
#[test]
fn revert_of_a_change_undone_and_redone_applies() {
    for restart in [false, true] {
        let mut f = fx(if restart { "prov-a-r" } else { "prov-a" });
        let s = f.open("a.md", b"one\ntwo\n");
        let a = f.join(&s, agent("a"), Rights::editor());
        let b = f.join(&s, agent("b"), Rights::editor());
        let c = replace(&s, a, "two", "TWO");
        hunk_at(&s, b, 0, 0, "zero\n"); // someone else's change elsewhere
        applied(own_only(&s, "a", || s.undo(a, 1).unwrap()));
        applied(own_only(&s, "a", || s.redo(a, 1).unwrap()));
        assert_eq!(text(&s, a), "zero\none\nTWO\n");
        let (s, a) = if restart {
            f.restart();
            let s = f.reopen("a.md");
            let a = f.join(&s, agent("a"), Rights::editor());
            (s, a)
        } else {
            (s, a)
        };
        applied(own_only(&s, "a", || s.revert(a, c, false).unwrap()));
        assert_eq!(text(&s, a), "zero\none\ntwo\n", "restart: {restart}");
        assert_i1(&s);
    }
}

/// Regression (b): b edits inside a's region and undoes it; what b put back has a's roots,
/// so a's undo applies (v1.7 conflicted: b's restoration was b's elements). And a's revert
/// of the same change applies too, after a restart (the roots from the store).
#[test]
fn undo_after_a_foreign_edit_inside_and_its_undo_applies() {
    let mut f = fx("prov-b");
    let s = f.open("b.md", b"one\ntwo\nthree\n");
    let a = f.join(&s, agent("a"), Rights::editor());
    let b = f.join(&s, agent("b"), Rights::editor());
    let c = replace(&s, a, "three", "THREE");
    replace(&s, b, "THREE", "THR3E");
    // while b's edit stands, a's undo conflicts and changes nothing
    assert!(matches!(
        s.undo(a, 1).unwrap(),
        UndoOutcome::Conflicts { step: 1, .. }
    ));
    applied(own_only(&s, "b", || s.undo(b, 1).unwrap()));
    assert_eq!(text(&s, a), "one\ntwo\nTHREE\n");
    if cfg!(feature = "conformance") {
        // the `E` b put back is a new element of b's change, rooted at a's `E`
        let r = root_pids(&s);
        assert!(r
            .iter()
            .filter(|x| x.0 != '\n')
            .skip(8)
            .all(|x| x.1.as_deref() == Some("a")));
    }
    // a's undo applies (by root: its region is as a left it)
    applied(own_only(&s, "a", || s.undo(a, 1).unwrap()));
    assert_eq!(text(&s, a), "one\ntwo\nthree\n");
    applied(own_only(&s, "a", || s.redo(a, 1).unwrap()));
    assert_eq!(text(&s, a), "one\ntwo\nTHREE\n");
    // after a restart (stacks lost), the revert of a's change applies the same way
    f.restart();
    let s = f.reopen("b.md");
    let a = f.join(&s, agent("a"), Rights::editor());
    applied(own_only(&s, "a", || s.revert(a, c, false).unwrap()));
    assert_eq!(text(&s, a), "one\ntwo\nthree\n");
    assert_i1(&s);
}

/// The duplication trap (I1, O4, O4a): b deletes a's `X`; b undoes it (`X1`, root `X`); c
/// deletes `X1`; then c's undo and a's revert of b's delete both want root `X` back, in
/// either order, mid-text and at the doc's end. The second finds it identical (O4: taken
/// once, nothing changes), never a second live `X` (by element identity, mutant (x), the
/// two would be different insertions, and at the doc's end both would land).
#[test]
fn concurrent_restores_of_one_root_keep_i1() {
    letters();
    for (seed, at, tag) in [(&b"aXb"[..], 1, "mid"), (&b"aX"[..], 1, "end")] {
        for revert_first in [false, true] {
            let f = fx(&format!("i1-{tag}-{revert_first}"));
            let s = f.open("i.md", seed);
            // a reverts b's change: an owner may
            let a = f.join(&s, agent("a"), Rights::editor().with_owner(true));
            let b = f.join(&s, agent("b"), Rights::editor());
            let c = f.join(&s, agent("c"), Rights::editor());
            // a's own X (not the import's): a writes it, then the trap
            hunk_at(&s, a, at, at + 1, "X");
            let want = text(&s, a);
            let cb = hunk_at(&s, b, at, at + 1, "");
            applied(own_only(&s, "b", || s.undo(b, 1).unwrap()));
            assert_eq!(text(&s, a), want);
            hunk_at(&s, c, at, at + 1, "");
            let one = |s: &Session| {
                let out = if revert_first {
                    own_only(s, "a", || s.revert(a, cb, false).unwrap())
                } else {
                    own_only(s, "c", || s.undo(c, 1).unwrap())
                };
                assert!(matches!(out, UndoOutcome::Applied { .. }), "{tag}: {out:?}");
            };
            one(&s);
            assert_eq!(text(&s, a), want, "{tag}: the first restore puts X back");
            assert_i1(&s);
            let v = s.status().unwrap().version;
            let second = if revert_first {
                own_only(&s, "c", || s.undo(c, 1).unwrap())
            } else {
                own_only(&s, "a", || s.revert(a, cb, false).unwrap())
            };
            // the second restore's window holds the first one's `X` where it would put `X`
            // back, with the same root: O4, identical, taken once (nothing to apply). (A
            // live root elsewhere is O4a's: `o4a_refuses_a_live_root_at_the_doc_end`.)
            match &second {
                UndoOutcome::Applied { skipped, .. } => {
                    assert!(skipped.is_empty());
                    assert_eq!(s.status().unwrap().version, v, "{tag}: nothing applied");
                }
                o => panic!("{tag}: {o:?}"),
            }
            assert_eq!(text(&s, a), want, "{tag}: never two live X");
            assert_i1(&s);
        }
    }
}

/// O4a at the doc's end: the second restore of a root lands where O5 would let both
/// insertions land (theirs is the live restoration followed by another append), so only
/// O4a stops a second live `X`.
#[test]
fn o4a_refuses_a_live_root_at_the_doc_end() {
    letters();
    let f = fx("o4a-end");
    let s = f.open("o.md", b"a");
    let a = f.join(&s, agent("a"), Rights::editor().with_owner(true));
    let b = f.join(&s, agent("b"), Rights::editor());
    let c = f.join(&s, agent("c"), Rights::editor());
    let d = f.join(&s, agent("d"), Rights::editor());
    hunk_at(&s, a, 1, 1, "X"); // "aX"
    let cb = hunk_at(&s, b, 1, 2, ""); // "a"
    applied(own_only(&s, "b", || s.undo(b, 1).unwrap())); // "aX1"
    hunk_at(&s, d, 2, 2, "Z"); // "aX1Z": appended, inside b's delete's carried window
    hunk_at(&s, c, 1, 2, ""); // "aZ"
    applied(own_only(&s, "c", || s.undo(c, 1).unwrap())); // "aX2Z"
    assert_eq!(text(&s, a), "aXZ");
    assert_i1(&s);
    // a's revert of b's delete: theirs is `X2 Z` (roots X, Z), ours `X`: not identical, both
    // at the end (O5 would land both), but X is live: O4a
    match s.revert(a, cb, false).unwrap() {
        UndoOutcome::Conflicts { conflicts, .. } => {
            assert_eq!(conflicts.len(), 1);
            assert_eq!(conflicts[0].kind, ConflictKind::Overlap);
        }
        o => panic!("{o:?}"),
    }
    assert_eq!(text(&s, a), "aXZ");
    assert_i1(&s);
}

/// O4 is judged on (position, root), not on text: a fresh `X` someone typed where an undo
/// would put `X` back is not that `X`. Mid-text it is a same-point conflict (O5); at the
/// doc's end both land, the fresh one first (theirs), each its own root.
#[test]
fn o4_a_fresh_identical_text_is_not_the_restored_root() {
    letters();
    for (seed, tag) in [(&b"aXb"[..], "mid"), (&b"aX"[..], "end")] {
        let f = fx(&format!("o4-{tag}"));
        let s = f.open("o.md", seed);
        let b = f.join(&s, agent("b"), Rights::editor());
        let c = f.join(&s, agent("c"), Rights::editor());
        hunk_at(&s, b, 1, 2, ""); // b deletes the import's X
        hunk_at(&s, c, 1, 1, "X"); // c types a fresh X there
        let before = text(&s, b);
        let out = s.undo(b, 1).unwrap();
        if tag == "mid" {
            assert!(
                matches!(&out, UndoOutcome::Conflicts { conflicts, .. }
                    if conflicts[0].kind == ConflictKind::Overlap),
                "{out:?}"
            );
            assert_eq!(text(&s, b), before);
        } else {
            applied(out);
            assert_eq!(text(&s, b), "aXX");
            if cfg!(feature = "conformance") {
                let r = root_pids(&s);
                assert_eq!(
                    r[1..],
                    [('X', Some("c".into())), ('X', Some("kit:import".into()))]
                );
            }
        }
        assert_i1(&s);
    }
}

/// Blame by root (U18a) after a restart: the root's author and change, and the restorer,
/// read from the store's provenance rows.
#[test]
fn blame_by_root_survives_a_restart() {
    let mut f = fx("prov-blame");
    let s = f.open("p.md", b"keep\n");
    let a = f.join(&s, agent("a"), Rights::full());
    let b = f.join(&s, agent("b"), Rights::full());
    let c1 = replace(&s, a, "keep\n", "keep\nmine\n");
    replace(&s, b, "mine\n", "");
    let u = match s.undo(b, 1).unwrap() {
        UndoOutcome::Applied { result, .. } => result.version,
        o => panic!("{o:?}"),
    };
    let check = |s: &Session, c: ConnId| {
        let bl = s.blame(c, ROOT, None).unwrap();
        let l = &bl.lines[1];
        assert_eq!(l.text, "mine");
        assert_eq!(
            (l.author.as_str(), l.change, l.kind),
            ("a", Some(c1), Some(ChangeKind::Edit))
        );
        assert_eq!(
            (l.restored_by.as_deref(), l.restored_in),
            (Some("b"), Some(u))
        );
        assert_eq!(bl.lines[0].restored_by, None);
    };
    check(&s, a);
    f.restart();
    let s = f.reopen("p.md");
    let a = f.join(&s, agent("a"), Rights::full());
    check(&s, a);
    assert_i1(&s);
}

/// Store format 4 → 5 on a session's store: stamp a store back to 4 and drop its (empty)
/// provenance table, as a format-4 build left it; it opens, is stamped 5, and undo, revert
/// and blame keep working (every existing element its own root).
#[test]
fn a_format_4_store_opens_and_keeps_working() {
    use redb::{Database, ReadableTable, TableDefinition};
    const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
    const PROV: TableDefinition<(u64, u64, u64), &[u8]> = TableDefinition::new("provenance");
    let mut f = fx("fmt4");
    let c = {
        let s = f.open("f.md", b"one\ntwo\n");
        let a = f.join(&s, agent("a"), Rights::full());
        replace(&s, a, "two", "TWO")
    };
    f.mgr.release().unwrap();
    f.store = None;
    {
        let db = Database::open(f.dir.0.join("docs.redb")).unwrap();
        let w = db.begin_write().unwrap();
        assert!(w.delete_table(PROV).unwrap());
        w.open_table(META)
            .unwrap()
            .insert("store_format", 4u64)
            .unwrap();
        w.commit().unwrap();
    }
    f.restart(); // the open upgrades it in place
    f.mgr.release().unwrap();
    f.store = None;
    {
        let d = Database::open(f.dir.0.join("docs.redb")).unwrap();
        let r = d.begin_read().unwrap();
        let v = r
            .open_table(META)
            .unwrap()
            .get("store_format")
            .unwrap()
            .unwrap()
            .value();
        assert_eq!(v, k2_mpc_base::store::STORE_FORMAT);
        assert_eq!(v, 5);
        assert_eq!(r.open_table(PROV).unwrap().iter().unwrap().count(), 0);
    }
    f.restart();
    let s = f.reopen("f.md");
    let a = f.join(&s, agent("a"), Rights::full());
    let b = f.join(&s, agent("b"), Rights::full());
    replace(&s, b, "one", "ONE");
    applied(own_only(&s, "b", || s.undo(b, 1).unwrap()));
    applied(own_only(&s, "a", || s.revert(a, c, false).unwrap()));
    assert_eq!(text(&s, a), "one\ntwo\n");
    let bl = s.blame(a, ROOT, None).unwrap();
    assert_eq!(bl.lines[0].author, "kit:import");
    assert_eq!(bl.lines[0].restored_by.as_deref(), Some("b"));
    assert_i1(&s);
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

// ---------- person undo grouping (undo spec U3, K4; hist.undo_group_person) ----------
//
// Each closer as a scenario (a function that panics when the rule is broken, so the mutant
// test can run it under a planted fault), then the trace class: seeded histories of a person
// typing with group tokens (two connections), another person editing, idle gaps, undo and
// redo, run on two sessions alike, one grouped and one not. An independent model of the
// closers says how many of the ungrouped session's steps each grouped step is; every undo
// and redo must give the same text on both, and every step must be the model's.

mod group {
    use super::*;
    use k2_mpc_base::differential::{with_group_mutant, GroupMutant};
    use k2_mpc_base::session::EditOptions;
    use k2_mpc_base::text::Hunk;
    use k2_mpc_base::Edit;
    use rand::rngs::SmallRng;
    use rand::{Rng, SeedableRng};

    pub(super) fn ver(s: &Session, c: ConnId) -> u64 {
        s.read(c).unwrap().version
    }

    /// A live edit (the hunk channel) at the doc's version, with `token` as its group.
    pub(super) fn live(
        s: &Session,
        c: ConnId,
        start: usize,
        end: usize,
        insert: &str,
        token: Option<&str>,
    ) {
        let opts = token.map_or_else(EditOptions::new, |t| EditOptions::new().group(t));
        let h = Hunk {
            start,
            end,
            insert: insert.into(),
        };
        let v = ver(s, c);
        let r = s.edit_at_base(c, ROOT, v, vec![h], &opts).unwrap();
        assert!(r.result.version > v, "the edit applied");
    }

    /// Type `chars` one keystroke each at the end of the text, all with `token`.
    pub(super) fn type_end(s: &Session, c: ConnId, chars: &str, token: &str) {
        for ch in chars.chars() {
            let n = text(s, c).len();
            live(s, c, n, n, &ch.to_string(), Some(token));
        }
    }

    /// Undo one step and return the text after it (the undo must apply).
    pub(super) fn undo1(s: &Session, c: ConnId) -> String {
        applied(s.undo(c, 1).unwrap());
        text(s, c)
    }

    pub(super) fn nothing(s: &Session, c: ConnId) {
        assert_eq!(s.undo(c, 1).unwrap(), UndoOutcome::Nothing);
    }

    pub(super) fn one_person(tag: &str, seed: &[u8]) -> (Fx, Arc<Session>, ConnId) {
        let f = fx(tag);
        let s = f.open(&format!("{tag}.md"), seed);
        let p = f.join(&s, person("p"), Rights::editor());
        (f, s, p)
    }

    // --- the scenarios ---

    /// Keystrokes with one token are one step, through both edit paths with a group
    /// (`edit_at_base` and `edit_with_options`); redo puts the whole group back.
    pub(super) fn typing_is_one_step() {
        let (_f, s, p) = one_person("g-one", b"x\n");
        type_end(&s, p, "abc", "input-1");
        let opts = EditOptions::new().group("input-1");
        s.edit_with_options(p, ROOT, &opts, |t| {
            Ok::<_, std::convert::Infallible>(format!("{t}d"))
        })
        .unwrap()
        .unwrap();
        assert_eq!(text(&s, p), "x\nabcd");
        assert_eq!(undo1(&s, p), "x\n");
        nothing(&s, p);
        applied(s.redo(p, 1).unwrap());
        assert_eq!(text(&s, p), "x\nabcd");
        assert_eq!(s.redo(p, 1).unwrap(), UndoOutcome::Nothing);
        assert_eq!(undo1(&s, p), "x\n");
    }

    /// (a) idle on the host clock: [`Config::undo_group_idle_ms`] (1 s) after the group's
    /// last edit closes it; a moment less doesn't.
    pub(super) fn idle_closes() {
        assert_eq!(Config::default().undo_group_idle_ms, 1_000);
        let (f, s, p) = one_person("g-idle", b"");
        type_end(&s, p, "ab", "t");
        f.host.advance(999);
        type_end(&s, p, "c", "t");
        f.host.advance(1_000);
        type_end(&s, p, "d", "t");
        assert_eq!(undo1(&s, p), "abc");
        assert_eq!(undo1(&s, p), "");
        nothing(&s, p);
        // 0: groups never merge
        let mut c = config();
        c.undo_group_idle_ms = 0;
        let f = fx_with("g-idle0", c);
        let s = f.open("z.md", b"");
        let p = f.join(&s, person("p"), Rights::editor());
        type_end(&s, p, "ab", "t");
        assert_eq!(undo1(&s, p), "a");
    }

    /// (a) as the spec writes it (U3 (a): 500 ms), with `undo_group_idle_ms` set to 500:
    /// 499 ms after the last edit joins, 500 ms closes.
    pub(super) fn idle_closes_spec_500() {
        let mut c = config();
        c.undo_group_idle_ms = 500;
        let f = fx_with("g-idle500", c);
        let s = f.open("i.md", b"");
        let p = f.join(&s, person("p"), Rights::editor());
        type_end(&s, p, "ab", "t");
        f.host.advance(499);
        type_end(&s, p, "c", "t");
        f.host.advance(500);
        type_end(&s, p, "d", "t");
        assert_eq!(undo1(&s, p), "abc");
        assert_eq!(undo1(&s, p), "");
        nothing(&s, p);
    }

    /// (b) a non-contiguous edit closes the group; a deletion touching it joins.
    pub(super) fn non_contiguous_closes() {
        // one imported token under words: the case is about grouping, so on letters (D1
        // is the identity there)
        letters();
        let (_f, s, p) = one_person("g-far", b"0123456789");
        live(&s, p, 10, 10, "a", Some("t"));
        live(&s, p, 11, 11, "b", Some("t"));
        live(&s, p, 2, 2, "x", Some("t")); // far away: a new step
        live(&s, p, 3, 3, "y", Some("t"));
        live(&s, p, 3, 5, "", Some("t")); // deletes `y2`: touches the step's span, joins
        assert_eq!(text(&s, p), "01x3456789ab");
        assert_eq!(undo1(&s, p), "0123456789ab");
        assert_eq!(undo1(&s, p), "0123456789");
        nothing(&s, p);
    }

    /// (c) a word boundary (a space or punctuation typed after a word character) starts the
    /// next step; a space after a space doesn't.
    pub(super) fn word_boundary_closes() {
        let (_f, s, p) = one_person("g-word", b"");
        type_end(&s, p, "hello  world, ok", "t");
        assert_eq!(undo1(&s, p), "hello  world, ok".replace(", ok", ""));
        assert_eq!(undo1(&s, p), "hello");
        assert_eq!(undo1(&s, p), "");
        nothing(&s, p);
    }

    /// (d) a newline is a step of its own.
    pub(super) fn newline_closes() {
        let (_f, s, p) = one_person("g-nl", b"");
        type_end(&s, p, "ab\ncd", "t");
        assert_eq!(undo1(&s, p), "ab\n");
        assert_eq!(undo1(&s, p), "ab");
        assert_eq!(undo1(&s, p), "");
        nothing(&s, p);
    }

    /// (e) another token, an edit without one, the same person's other connection.
    pub(super) fn token_and_connection_close() {
        let (f, s, p) = one_person("g-tok", b"");
        let p2 = f.join(&s, person("p"), Rights::editor());
        type_end(&s, p, "ab", "t1");
        type_end(&s, p, "cd", "t2");
        let n = text(&s, p).len();
        live(&s, p, n, n, "e", None);
        type_end(&s, p, "f", "t2");
        type_end(&s, p2, "g", "t2"); // same pid, another connection
        type_end(&s, p, "h", "t2");
        assert_eq!(text(&s, p), "abcdefgh");
        for want in ["abcdefg", "abcdef", "abcde", "abcd", "ab", ""] {
            assert_eq!(undo1(&s, p), want);
        }
        nothing(&s, p);
    }

    /// (f) an undo or redo closes the group, even one with nothing to do.
    pub(super) fn undo_redo_close() {
        let (_f, s, p) = one_person("g-undo", b"");
        type_end(&s, p, "ab", "t");
        assert_eq!(s.redo(p, 1).unwrap(), UndoOutcome::Nothing);
        type_end(&s, p, "c", "t");
        assert_eq!(undo1(&s, p), "ab");
        applied(s.redo(p, 1).unwrap());
        type_end(&s, p, "d", "t");
        assert_eq!(undo1(&s, p), "abc");
        assert_eq!(undo1(&s, p), "ab");
        assert_eq!(undo1(&s, p), "");
    }

    /// (g) another participant's change inside the group's span, or touching it (here a
    /// deletion just before it, and an insertion at its end), closes it in host apply order;
    /// one elsewhere doesn't.
    pub(super) fn foreign_change_closes() {
        // q types inside p's word: under words D1 would make it q's; the case is about the
        // closer, so on letters (D1 the identity)
        letters();
        // inside
        let f = fx("g-in");
        let s = f.open("in.md", b"x ");
        let p = f.join(&s, person("p"), Rights::editor());
        let q = f.join(&s, person("q"), Rights::editor());
        type_end(&s, p, "ab", "t");
        hunk_at(&s, q, 3, 3, "Z"); // x aZb
        type_end(&s, p, "c", "t");
        assert_eq!(undo1(&s, p), "x aZb");
        // touching: a deletion ending at the span's start
        let f = fx("g-touch");
        let s = f.open("t.md", b"xy");
        let p = f.join(&s, person("p"), Rights::editor());
        let q = f.join(&s, person("q"), Rights::editor());
        type_end(&s, p, "ab", "t");
        hunk_at(&s, q, 1, 2, ""); // xab
        type_end(&s, p, "c", "t");
        assert_eq!(undo1(&s, p), "xab");
        assert_eq!(undo1(&s, p), "x");
        // touching: an insertion at the span's end
        let f = fx("g-end");
        let s = f.open("e.md", b"");
        let p = f.join(&s, person("p"), Rights::editor());
        let q = f.join(&s, person("q"), Rights::editor());
        type_end(&s, p, "ab", "t");
        hunk_at(&s, q, 2, 2, "Z");
        live(&s, p, 3, 3, "c", Some("t"));
        assert_eq!(undo1(&s, p), "abZ");
        // elsewhere (not touching): still one step
        let f = fx("g-away");
        let s = f.open("w.md", b"top\n\n");
        let p = f.join(&s, person("p"), Rights::editor());
        let q = f.join(&s, person("q"), Rights::editor());
        type_end(&s, p, "ab", "t");
        hunk_at(&s, q, 0, 0, "Q");
        type_end(&s, p, "cd", "t");
        assert_eq!(undo1(&s, p), "Qtop\n\n");
        nothing(&s, p);
    }

    /// Agents never group: one verb, one step, whatever token they pass.
    pub(super) fn agents_never_group() {
        let f = fx("g-agent");
        let s = f.open("a.md", b"");
        let a = f.join(&s, agent("a"), Rights::editor());
        type_end(&s, a, "ab", "t");
        assert_eq!(undo1(&s, a), "a");
    }

    /// A group that undid itself (typed, then deleted with the same token) leaves nothing to
    /// undo; the step before it is still there.
    pub(super) fn self_undone_group_leaves_nothing() {
        let (_f, s, p) = one_person("g-noop", b"");
        type_end(&s, p, "x ", "t0");
        type_end(&s, p, "ab", "t");
        live(&s, p, 3, 4, "", Some("t"));
        live(&s, p, 2, 3, "", Some("t"));
        assert_eq!(text(&s, p), "x ");
        assert_eq!(undo1(&s, p), "x");
        assert_eq!(undo1(&s, p), "");
        nothing(&s, p);
    }

    /// A group that runs into someone else's text: its undo puts back exactly the elements
    /// it removed (by root, U23) and removes only its own (U10a, I1); then their undo
    /// applies cleanly.
    pub(super) fn group_over_others_text() {
        let f = fx("g-over");
        let s = f.open("o.md", b"");
        let p = f.join(&s, person("p"), Rights::editor());
        let q = f.join(&s, agent("q"), Rights::editor());
        hunk_at(&s, q, 0, 0, "XY");
        live(&s, p, 1, 1, "a", Some("t"));
        live(&s, p, 2, 2, "b", Some("t"));
        live(&s, p, 3, 4, "", Some("t")); // deletes q's `Y`: touches the span's end
        live(&s, p, 0, 1, "", Some("t")); // and q's `X`, touching its start
        assert_eq!(text(&s, p), "ab");
        own_only(&s, "p", || applied(s.undo(p, 1).unwrap()));
        assert_eq!(text(&s, p), "XY");
        assert_i1(&s);
        assert!(root_pids(&s).iter().all(|(_, r)| r.as_deref() != Some("p")));
        applied(s.undo(q, 1).unwrap());
        assert_eq!(text(&s, q), "");
        nothing(&s, p);
    }

    #[test]
    fn hist_undo_group_person_typing_is_one_step() {
        typing_is_one_step();
    }
    #[test]
    fn hist_undo_group_person_idle_closes() {
        idle_closes();
    }
    #[test]
    fn hist_undo_group_person_idle_closes_spec_500() {
        idle_closes_spec_500();
    }
    #[test]
    fn hist_undo_group_person_non_contiguous_closes() {
        non_contiguous_closes();
    }
    #[test]
    fn hist_undo_group_person_word_boundary_closes() {
        word_boundary_closes();
    }
    #[test]
    fn hist_undo_group_person_newline_closes() {
        newline_closes();
    }
    #[test]
    fn hist_undo_group_person_token_and_connection_close() {
        token_and_connection_close();
    }
    #[test]
    fn hist_undo_group_person_undo_redo_close() {
        undo_redo_close();
    }
    #[test]
    fn hist_undo_group_person_foreign_change_closes() {
        foreign_change_closes();
    }
    #[test]
    fn hist_undo_group_person_agents_never_group() {
        agents_never_group();
    }
    #[test]
    fn hist_undo_group_person_self_undone_group_leaves_nothing() {
        self_undone_group_leaves_nothing();
    }
    #[test]
    fn hist_undo_group_person_over_others_text() {
        group_over_others_text();
    }

    // --- the trace class ---

    fn env(name: &str, default: u64) -> u64 {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    fn word(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    fn boundary(c: char) -> bool {
        c.is_whitespace() || c.is_ascii_punctuation()
    }

    /// The model of one grouped step, as plain strings (no element IDs): its window `s..e`
    /// in the live text, holding `after` (on the undo stack) or `before` (on the redo
    /// stack), and whether any change since touched it (inside or at an edge). An untouched
    /// step's undo (redo) must give exactly the live text with the window replaced by
    /// `before` (`after`): undoing a group returns its window to what it was before the
    /// group's first edit.
    #[derive(Debug, Clone)]
    struct G {
        s: usize,
        e: usize,
        before: String,
        after: String,
        clean: bool,
        /// Bytes of older text the group took (0 and an empty window: it undid itself).
        took: usize,
    }

    impl G {
        /// Carry through one change's hunk (in the coordinates before it).
        fn carry(&mut self, h: &Hunk) {
            if h.start <= self.e && h.end >= self.s {
                self.clean = false;
            } else if h.end < self.s {
                let d = h.insert.len() as isize - (h.end - h.start) as isize;
                self.s = (self.s as isize + d) as usize;
                self.e = (self.e as isize + d) as usize;
            }
        }
    }

    /// The closers' model (U3): the grouped person's undo and redo stacks and the open
    /// group (the top of `undo`, with its token and last edit's time).
    #[derive(Default)]
    struct Model {
        undo: Vec<G>,
        redo: Vec<G>,
        open: Option<(u32, u64)>,
    }

    impl Model {
        /// Carry every step through a change's hunks, `skip_top` leaving the top of the undo
        /// stack alone (the open group taking its own edit).
        fn carry(&mut self, hunks: &[Hunk], skip_top: bool) {
            let n = self.undo.len();
            for (k, g) in self.undo.iter_mut().enumerate() {
                if skip_top && k + 1 == n {
                    continue;
                }
                let mut shift: isize = 0;
                for h in hunks {
                    let h = Hunk {
                        start: (h.start as isize + shift) as usize,
                        end: (h.end as isize + shift) as usize,
                        insert: h.insert.clone(),
                    };
                    shift += h.insert.len() as isize - (h.end - h.start) as isize;
                    g.carry(&h);
                }
            }
            for g in self.redo.iter_mut() {
                let mut shift: isize = 0;
                for h in hunks {
                    let h = Hunk {
                        start: (h.start as isize + shift) as usize,
                        end: (h.end as isize + shift) as usize,
                        insert: h.insert.clone(),
                    };
                    shift += h.insert.len() as isize - (h.end - h.start) as isize;
                    g.carry(&h);
                }
            }
        }

        /// Someone else's change, or the person's other connection's: closes the open group
        /// when it touches its span (U3 (g)); every step carried.
        fn other(&mut self, hunks: &[Hunk]) {
            self.carry(hunks, false);
            if self.undo.last().is_some_and(|g| !g.clean) {
                self.open = None;
            }
        }

        /// The person's grouped edit `h` on `text` (`after_text`: the text after it), at
        /// `now`, with `token`. Returns true when the group undid itself (no step left).
        fn typed(&mut self, text: &str, after_text: &str, h: &Hunk, token: u32, now: u64) -> bool {
            self.redo.clear();
            let mut prev = text[..h.start].chars().next_back();
            let (mut nl, mut bounded) = (false, false);
            for c in h.insert.chars() {
                nl |= c == '\n';
                bounded |= boundary(c) && prev.is_some_and(word);
                prev = Some(c);
            }
            let join = !nl
                && !bounded
                && matches!(self.open, Some((t, last)) if t == token && now - last < 1_000)
                && self
                    .undo
                    .last()
                    .is_some_and(|g| g.clean && h.start <= g.e && h.end >= g.s);
            let grow = h.insert.len() as isize - (h.end - h.start) as isize;
            if join {
                self.carry(std::slice::from_ref(h), true);
                let g = self.undo.last_mut().unwrap();
                let s = g.s.min(h.start);
                let e1 = g.e.max(h.end);
                // the window before the edit, with the group's own part turned back
                let before = format!("{}{}{}", &text[s..g.s], g.before, &text[g.e..e1]);
                let e = (e1 as isize + grow) as usize;
                g.took += g.s.saturating_sub(h.start) + h.end.saturating_sub(g.e);
                (g.s, g.e, g.before) = (s, e, before);
                g.after = after_text[s..e].to_owned();
                self.open = Some((token, now));
                if g.s == g.e && g.took == 0 {
                    assert!(g.before.is_empty());
                    self.undo.pop();
                    self.open = None;
                    return true;
                }
            } else {
                self.carry(std::slice::from_ref(h), false);
                self.undo.push(G {
                    s: h.start,
                    e: h.start + h.insert.len(),
                    before: text[h.start..h.end].to_owned(),
                    after: h.insert.clone(),
                    clean: true,
                    took: h.end - h.start,
                });
                self.open = Some((token, now));
            }
            if nl {
                self.open = None;
            }
            false
        }
    }

    #[derive(Default, Debug)]
    struct Counts {
        traces: u64,
        typed: u64,
        joined: u64,
        self_undone: u64,
        undos_exact: u64,
        redos_exact: u64,
        undos_other: u64,
        conflicts: u64,
        groups_unwound: u64,
    }

    /// One history, `steps` long, on one session: a person typing with group tokens through
    /// the hunk channel (connection 1), the same person's other connection and another
    /// person editing, idle gaps, undo and redo. `Err` on any difference from the model.
    fn trace(seed: u64, steps: usize, n: &mut Counts) -> Result<(), String> {
        // the string model has no authors: on letters, where D1 (v1.10) is the identity (the
        // oracle's grouping class covers D1 with grouping on words: spec-undo's traces)
        letters();
        let mut rng = SmallRng::seed_from_u64(seed);
        let init = ["", "x\n", "ab cd\n\nef"][rng.gen_range(0..3)];
        let f = fx(&format!("g-trace-{seed}"));
        let s = f.open("t.md", init.as_bytes());
        let p1 = f.join(&s, person("p"), Rights::editor());
        let p2 = f.join(&s, person("p"), Rights::editor());
        let q = f.join(&s, person("q"), Rights::editor());
        let events = s.subscribe(q).unwrap();
        let mut m = Model::default();
        let mut token = 0u32;
        let mut caret = init.len();
        let mut log: Vec<String> = Vec::new();
        let drain = |m: &mut Model, own: bool| {
            let evs: Vec<_> = events.try_iter().collect();
            if !own {
                for ev in &evs {
                    m.carry(&ev.hunks, false);
                }
            }
            evs
        };
        // one undo or redo of the top grouped step, checked against the model
        let step = |m: &mut Model, redo: bool, log: &mut Vec<String>, n: &mut Counts| {
            m.open = None;
            let live0 = text(&s, p1);
            let top = if redo { m.redo.pop() } else { m.undo.pop() };
            let o = if redo {
                s.redo(p1, 1).unwrap()
            } else {
                s.undo(p1, 1).unwrap()
            };
            let live1 = text(&s, p1);
            log.push(format!(
                "{} {top:?}: {o:?} -> {live1:?}",
                if redo { "redo" } else { "undo" }
            ));
            let Some(mut g) = top else {
                return match o {
                    UndoOutcome::Nothing => Ok(true),
                    o => Err(format!(
                        "the model has no step, the session: {o:?}\n{log:#?}"
                    )),
                };
            };
            let applied = matches!(o, UndoOutcome::Applied { steps: 1, .. });
            if !applied && !matches!(o, UndoOutcome::Conflicts { .. }) {
                return Err(format!("{o:?}\n{log:#?}"));
            }
            let (from, to) = if redo {
                (&g.before, &g.after)
            } else {
                (&g.after, &g.before)
            };
            if g.clean {
                if live0.get(g.s..g.e) != Some(from.as_str()) {
                    return Err(format!("model window {:?} not in {live0:?}\n{log:#?}", g));
                }
                let want = format!("{}{to}{}", &live0[..g.s], &live0[g.e..]);
                if !applied || live1 != want {
                    return Err(format!(
                        "an untouched step: want {want:?}, got {o:?} {live1:?}\n{log:#?}"
                    ));
                }
                if redo {
                    n.redos_exact += 1;
                } else {
                    n.undos_exact += 1;
                }
            } else if !applied {
                n.conflicts += 1;
                if live1 != live0 {
                    return Err(format!("a conflict changed the text\n{log:#?}"));
                }
                // it stays where it was
                if redo {
                    m.redo.push(g);
                } else {
                    m.undo.push(g);
                }
                drain(m, false);
                return Ok(false);
            } else {
                n.undos_other += 1;
            }
            // the step's own change: every other step carried through it
            let evs = drain(m, false);
            if evs.len() > 1 || (evs.is_empty() && (g.clean || live1 != live0)) {
                return Err(format!("one step, {} changes\n{log:#?}", evs.len()));
            }
            if g.clean {
                g.e = g.s + to.len();
            }
            if redo {
                m.undo.push(g);
            } else {
                m.redo.push(g);
            }
            Ok(true)
        };
        for _ in 0..steps {
            let live_text = text(&s, p1);
            let r = rng.gen_range(0..100);
            if r < 60 {
                // the person types (or deletes) at the caret, mostly; one keystroke
                if rng.gen_bool(0.1) {
                    token += 1;
                }
                if rng.gen_bool(0.15) || caret > live_text.len() {
                    caret = rng.gen_range(0..=live_text.len());
                    while !live_text.is_char_boundary(caret) {
                        caret -= 1;
                    }
                }
                let back = rng.gen_bool(0.2) && caret > 0;
                let fwd = !back && rng.gen_bool(0.08) && caret < live_text.len();
                let h = if back || fwd {
                    // backspace or forward delete, one character or up to three
                    let k = if rng.gen_bool(0.7) {
                        1
                    } else {
                        rng.gen_range(2..=3)
                    };
                    let (mut s0, mut e0) = (caret, caret);
                    for _ in 0..k {
                        if back && s0 > 0 {
                            s0 -= 1;
                            while !live_text.is_char_boundary(s0) {
                                s0 -= 1;
                            }
                        } else if fwd && e0 < live_text.len() {
                            e0 += 1;
                            while !live_text.is_char_boundary(e0) {
                                e0 += 1;
                            }
                        }
                    }
                    Hunk {
                        start: s0,
                        end: e0,
                        insert: String::new(),
                    }
                } else {
                    let ins: String = (0..rng.gen_range(1..=2))
                        .map(|_| ['a', 'b', 'a', ' ', ',', '\n'][rng.gen_range(0..6)])
                        .collect();
                    Hunk {
                        start: caret,
                        end: caret,
                        insert: ins,
                    }
                };
                let now = f.host.now_ms();
                let tok = format!("t{token}");
                live(&s, p1, h.start, h.end, &h.insert, Some(&tok));
                drain(&mut m, true);
                n.typed += 1;
                let depth = m.undo.len();
                let after_text = text(&s, p1);
                if m.typed(&live_text, &after_text, &h, token, now) {
                    n.self_undone += 1;
                } else if m.undo.len() == depth {
                    n.joined += 1;
                }
                caret = h.start + h.insert.len();
                log.push(format!("p types {h:?} ({tok}, t={now})"));
            } else if r < 75 {
                // someone else, or the person's other connection
                let (who, c) = if rng.gen_bool(0.75) {
                    ("q", q)
                } else {
                    ("p2", p2)
                };
                let h = if rng.gen_bool(0.6) || live_text.is_empty() {
                    let mut at = rng.gen_range(0..=live_text.len());
                    while !live_text.is_char_boundary(at) {
                        at -= 1;
                    }
                    Hunk {
                        start: at,
                        end: at,
                        insert: ["X", "Y", "Z "][rng.gen_range(0..3)].into(),
                    }
                } else {
                    let mut at = rng.gen_range(0..live_text.len());
                    while !live_text.is_char_boundary(at) {
                        at -= 1;
                    }
                    let len = live_text[at..].chars().next().unwrap().len_utf8();
                    Hunk {
                        start: at,
                        end: at + len,
                        insert: String::new(),
                    }
                };
                let e = Edit::Text {
                    root: ROOT.into(),
                    hunks: vec![h.clone()],
                };
                s.edit(c, vec![e], None).unwrap();
                drain(&mut m, true);
                m.other(std::slice::from_ref(&h));
                if who == "p2" {
                    // the same person's stack: a step of its own, closing the group (e)
                    m.redo.clear();
                    m.undo.push(G {
                        s: h.start,
                        e: h.start + h.insert.len(),
                        before: live_text[h.start..h.end].to_owned(),
                        after: h.insert.clone(),
                        clean: true,
                        took: h.end - h.start,
                    });
                    m.open = None;
                }
                if caret >= h.end {
                    caret = caret + h.insert.len() - (h.end - h.start);
                } else if caret > h.start {
                    caret = h.start;
                }
                log.push(format!("{who} edits {h:?}"));
            } else if r < 87 {
                let gap = if rng.gen_bool(0.2) {
                    rng.gen_range(900..1_500)
                } else {
                    rng.gen_range(0..400)
                };
                f.host.advance(gap);
                log.push(format!("+{gap} ms"));
            } else {
                let redo = r >= 95;
                // an undo is often taken back at once (Ctrl+Z, Ctrl+Shift+Z)
                if step(&mut m, redo, &mut log, n)? && !redo && rng.gen_bool(0.3) {
                    step(&mut m, true, &mut log, n)?;
                }
                caret = caret.min(text(&s, p1).len());
            }
        }
        // unwind: every grouped step is the model's, down to an empty stack
        while !m.undo.is_empty() {
            if !step(&mut m, false, &mut log, n)? {
                // blocked: drop it, as Ctrl+Z's "skip this change" does
                let o = s
                    .undo_with(p1, &UndoRequest::default().mode(UndoMode::Drop))
                    .unwrap();
                if !matches!(o, UndoOutcome::Dropped { .. }) {
                    return Err(format!("drop: {o:?}\n{log:#?}"));
                }
                m.undo.pop();
            }
            n.groups_unwound += 1;
        }
        let o = s.undo(p1, 1).unwrap();
        if o != UndoOutcome::Nothing {
            return Err(format!(
                "the model's stack is empty, the session's: {o:?}\n{log:#?}"
            ));
        }
        n.traces += 1;
        Ok(())
    }

    /// The trace class of `hist.undo_group_person` (U3; the undo spec's §8 class "grouping
    /// with the (g) split", on the kit against a string model): `GROUP_TRACES` histories
    /// (default 96; ≥ 10^4 on a node), `GROUP_STEPS` steps each (default 60).
    #[test]
    fn hist_undo_group_person_traces() {
        let traces = env("GROUP_TRACES", 96);
        let steps = env("GROUP_STEPS", 60) as usize;
        let seed = env("GROUP_SEED", 0x6b34);
        let t0 = std::time::Instant::now();
        let mut n = Counts::default();
        for i in 0..traces {
            if let Err(e) = trace(seed + i, steps, &mut n) {
                panic!("trace seed {}: {e}", seed + i);
            }
        }
        eprintln!("[group traces] {n:?}");
        assert!(
            n.joined > 0 && n.undos_exact > 0 && n.redos_exact > 0 && n.self_undone > 0,
            "{n:?}"
        );
        eprintln!(
            "RUNREC {{\"job\":null,\"run\":\"undo_group_traces\",\"tool\":\"history\",\"status\":\"COMPLETE\",\"traces\":{},\"steps\":{steps},\"typed\":{},\"joined\":{},\"self_undone\":{},\"undos_exact\":{},\"redos_exact\":{},\"undos_other\":{},\"conflicts\":{},\"groups_unwound\":{},\"seed\":{seed},\"wall_s\":{}}}",
            n.traces,
            n.typed,
            n.joined,
            n.self_undone,
            n.undos_exact,
            n.redos_exact,
            n.undos_other,
            n.conflicts,
            n.groups_unwound,
            t0.elapsed().as_secs()
        );
    }

    /// The planted faults ([`GroupMutant`]) are each caught: by their closer's scenario and
    /// by the trace class (within `GROUP_MUTANT_TRACES` histories, default 96); the catching
    /// history passes without the fault.
    #[test]
    fn hist_undo_group_person_mutants_are_caught() {
        let traces = env("GROUP_MUTANT_TRACES", 96);
        let all: [(GroupMutant, &str, fn()); 3] = [
            (
                GroupMutant::NeverClosesOnForeign,
                "never_closes_on_foreign",
                foreign_change_closes,
            ),
            (
                GroupMutant::NeverClosesOnIdle,
                "never_closes_on_idle",
                idle_closes,
            ),
            (
                GroupMutant::NeverClosesOnBoundary,
                "never_closes_on_boundary",
                word_boundary_closes,
            ),
        ];
        // (a planted fault's panics print; the panic hook is the process's, so it is left be)
        let quiet = |f: &dyn Fn() -> bool| f();
        for (m, name, scenario) in all {
            let caught =
                quiet(&|| with_group_mutant(m, || std::panic::catch_unwind(scenario).is_err()));
            assert!(caught, "{name}: its scenario passes with the fault planted");
            let mut hit = None;
            for i in 0..traces {
                let seed = 9_000_000 + i;
                let failed = quiet(&|| {
                    with_group_mutant(m, || {
                        let mut n = Counts::default();
                        !matches!(
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                trace(seed, 60, &mut n)
                            })),
                            Ok(Ok(()))
                        )
                    })
                });
                if failed {
                    hit = Some(seed);
                    break;
                }
            }
            let seed = hit.unwrap_or_else(|| panic!("{name}: no trace catches it in {traces}"));
            let mut n = Counts::default();
            if let Err(e) = trace(seed, 60, &mut n) {
                panic!("{name}: trace {seed} fails without the fault too: {e}");
            }
            eprintln!("[group mutant {name}] caught by its scenario and by trace {seed}");
            eprintln!(
                "RUNREC {{\"job\":null,\"run\":\"undo_group_mutant_{name}\",\"tool\":\"history\",\"status\":\"CAUGHT\",\"trace_seed\":{seed}}}"
            );
        }
    }
}
