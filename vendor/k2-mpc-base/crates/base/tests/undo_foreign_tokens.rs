//! D1 (undo spec v1.10, the authored-ranges amendment): a change is never built from someone
//! else's text, so an undo never changes text someone else wrote. The kit's side of the
//! amendment's case classes; the oracle's are `crates/spec-undo/tests/d1_authored_ranges.rs`.
//!
//! - **(a)** found by the soak at call 241,426 of `SOAK_SEED=1` (v0.9.6 diagnosis; released
//!   v0.9.4 too), shrunk to four calls. p16 types `p16:3027` and changes its `7` to a `9`
//!   (its own text: one character). p14 replaces the line with `p14:3169`. Before the
//!   amendment the replace was aligned by character, so p14's line kept p16's `p1`, `9` and
//!   line break, and p16's undo put the `7` back inside p14's line (`p14:3167`). Now p14's
//!   replace rewrites the whole token and p16's undo conflicts.
//! - **(b)** two words of one line changed by two people, merged by an agent's `write
//!   --base` from the original: each undoes their own word.
//! - **(c)** one word changed in a three-author paragraph: everyone else's blame and undo
//!   are intact.
//! - The hunk channel reports the authored hunks, not the widened ones (E-LIVE r2.1), and
//!   the store keeps them beside the widened record, across a restart (v0.9.7).
//! - The three D1 mutants (the `differential` feature's switch) each fail their class.

use std::sync::{Arc, Mutex};

use k2_mpc_base::differential::{with_d1_mutant, D1Mutant};
use k2_mpc_base::session::{
    AuditEvent, ChangeEvent, Config, ConnId, Host, Participant, ParticipantKind, Rights, Session,
    SessionManager, Stay, TextFilePack, UndoOutcome, UndoRequest, WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore};

const ROOT: &str = TextFilePack::ROOT;

struct H;
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        1_000
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct Fx {
    _mgr: SessionManager,
    s: Arc<Session>,
    dir: std::path::PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str, text: &str) -> Fx {
    let dir = std::env::temp_dir().join(format!("k2mb-d1-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("d.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, Arc::new(H), c);
    let key = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    let s = mgr
        .open(
            Arc::new(TextFilePack::new("markdown", 1 << 20)),
            &key,
            Some(text.as_bytes()),
        )
        .unwrap();
    Fx { _mgr: mgr, s, dir }
}

impl Fx {
    fn join(&self, pid: &str) -> ConnId {
        self.s
            .join(
                Participant::new(pid, pid, "#336699", ParticipantKind::Agent, "cli"),
                Stay::Held,
            )
            .unwrap()
            .conn
    }

    fn text(&self, c: ConnId) -> String {
        self.s.read(c).unwrap().text_by_root[0].1.clone()
    }

    fn undo(&self, c: ConnId) -> UndoOutcome {
        self.s.undo_with(c, &UndoRequest::steps(1)).unwrap()
    }
}

fn applied(o: &UndoOutcome) -> bool {
    matches!(o, UndoOutcome::Applied { steps: 1, .. })
}

/// The mutant switch is per thread, but the classes share nothing else: no lock needed.
static ORDER: Mutex<()> = Mutex::new(());

fn class_a() {
    let f = fx("a", "seed\n");
    let (p16, p14) = (f.join("p16"), f.join("p14"));
    f.s.replace_text(p16, ROOT, "seed\np16:3027\n").unwrap();
    f.s.replace_text(p16, ROOT, "seed\np16:3029\n").unwrap();
    f.s.replace_text(p14, ROOT, "seed\np14:3169\n").unwrap();
    let undo = f.undo(p16);
    assert_eq!(f.text(p14), "seed\np14:3169\n", "p14's line is untouched");
    assert!(
        matches!(undo, UndoOutcome::Conflicts { step: 1, .. }),
        "p16's undo conflicts: {undo:?}"
    );
}

/// (b) words of three writers on adjacent lines, merged by an agent's `write --base` from
/// before the other two: every author undoes their own (v0.9.7.1: the writers' words are on
/// adjacent lines, which still merge; the same words in one line now conflict).
fn class_b() {
    let base_text = "alpha\nbeta\ngamma\n";
    let f = fx("b", base_text);
    let (p1, p2, ag) = (f.join("p1"), f.join("p2"), f.join("ag"));
    let base = f.s.read(ag).unwrap().version;
    f.s.replace_text(p1, ROOT, "alpha\nBETA\ngamma\n").unwrap();
    f.s.replace_text(p2, ROOT, "alpha\nBETA\nGAMMA\n").unwrap();
    let w =
        f.s.write_base(ag, ROOT, base, "ALPHA\nbeta\ngamma\n")
            .unwrap();
    assert!(matches!(w, WriteOutcome::Applied(_)), "{w:?}");
    assert_eq!(f.text(ag), "ALPHA\nBETA\nGAMMA\n");
    assert!(applied(&f.undo(p1)), "p1 undoes BETA");
    assert_eq!(f.text(ag), "ALPHA\nbeta\nGAMMA\n");
    assert!(applied(&f.undo(p2)), "p2 undoes GAMMA");
    assert_eq!(f.text(ag), "ALPHA\nbeta\ngamma\n");
    assert!(applied(&f.undo(ag)), "the agent undoes ALPHA");
    assert_eq!(f.text(ag), base_text);
}

/// (b), the same words in one line: the agent's write conflicts and applies nothing
/// (v0.9.7.1: a write never merges inside a line).
#[test]
fn class_b_in_one_line_the_write_conflicts() {
    let _g = ORDER.lock().unwrap_or_else(|p| p.into_inner());
    let base_text = "alpha beta gamma\n";
    let f = fx("b1", base_text);
    let (p1, p2, ag) = (f.join("p1"), f.join("p2"), f.join("ag"));
    let base = f.s.read(ag).unwrap().version;
    f.s.replace_text(p1, ROOT, "alpha BETA gamma\n").unwrap();
    f.s.replace_text(p2, ROOT, "alpha BETA GAMMA\n").unwrap();
    let w =
        f.s.write_base(ag, ROOT, base, "ALPHA beta gamma\n")
            .unwrap();
    assert!(matches!(w, WriteOutcome::Conflicts(_)), "{w:?}");
    assert_eq!(f.text(ag), "alpha BETA GAMMA\n");
}

fn class_c() {
    let f = fx("c", "");
    let (p1, p2, p3, ag) = (f.join("p1"), f.join("p2"), f.join("p3"), f.join("ag"));
    f.s.replace_text(p1, ROOT, "The quick brown fox").unwrap();
    f.s.replace_text(p2, ROOT, "The quick brown fox jumps over")
        .unwrap();
    f.s.replace_text(p3, ROOT, "The quick brown fox jumps over the lazy dog.\n")
        .unwrap();
    f.s.replace_text(ag, ROOT, "The quick brown fox leaps over the lazy dog.\n")
        .unwrap();
    // blame: only the changed word moved to the agent
    let b = f.s.blame(ag, ROOT, None).unwrap();
    let authors: Vec<&str> = b.lines.iter().map(|l| l.author.as_str()).collect();
    assert_eq!(authors.len(), 1, "{b:?}");
    // the co-authors' undo still applies, each to their own text
    assert!(applied(&f.undo(p3)), "p3 undoes its clause");
    assert_eq!(f.text(ag), "The quick brown fox leaps over");
    assert!(applied(&f.undo(p1)), "p1 undoes its words");
}

#[test]
fn class_a_a_replace_over_another_s_edit_then_their_undo() {
    let _g = ORDER.lock().unwrap_or_else(|p| p.into_inner());
    class_a();
}

#[test]
fn class_b_words_on_adjacent_lines_merged_by_write_base_each_undoes_their_own() {
    let _g = ORDER.lock().unwrap_or_else(|p| p.into_inner());
    class_b();
}

#[test]
fn class_c_a_one_word_edit_in_prose_keeps_the_co_authors() {
    let _g = ORDER.lock().unwrap_or_else(|p| p.into_inner());
    class_c();
}

/// The hunk channel reports what the participant authored (E-LIVE r2.1): p14's change is
/// the replacement the diff found, not the widened rewrite of the token.
#[test]
fn the_channel_reports_the_authored_hunks() {
    let f = fx("chan", "seed\n");
    let (p16, p14) = (f.join("p16"), f.join("p14"));
    f.s.replace_text(p16, ROOT, "seed\np16:3029\n").unwrap();
    let rx = f.s.subscribe(p14).unwrap();
    f.s.replace_text(p14, ROOT, "seed\np14:3169\n").unwrap();
    let ev: ChangeEvent = rx.try_recv().unwrap();
    let authored = k2_mpc_base::text::lines::align_insertions(
        "seed\np16:3029\n",
        k2_mpc_base::text::diff::hunks("seed\np16:3029\n", "seed\np14:3169\n"),
    );
    assert_eq!(ev.hunks, authored, "the authored hunks, not the token");
    assert!(
        ev.hunks.len() == 1 && ev.hunks[0].start > 5,
        "{:?}",
        ev.hunks
    );
}

/// Each D1 mutant fails the class the amendment names.
#[test]
fn each_d1_mutant_fails_its_class() {
    let _g = ORDER.lock().unwrap_or_else(|p| p.into_inner());
    let fails =
        |m: D1Mutant, class: fn()| with_d1_mutant(m, || std::panic::catch_unwind(class).is_err());
    assert!(
        fails(D1Mutant::ReuseForeign, class_a),
        "reuse-foreign-elements-in-changed-token must fail (a)"
    );
    assert!(
        fails(D1Mutant::RewriteWholeLine, class_c),
        "rewrite-whole-line must fail (c)"
    );
    assert!(
        fails(D1Mutant::RewriteMergedRanges, class_b),
        "rewrite-merged-ranges must fail (b)"
    );
}

/// The store keeps the authored hunks of a change D1 widened (v0.9.7): the `authored` row
/// equals what the hunk channel reported, `change_hunks` returns it (and the record's own
/// hunks for a change D1 didn't widen, which has no row), and both survive a restart
/// (release, the store reopened from its file, a new manager).
#[test]
fn the_store_keeps_the_authored_hunks_across_a_restart() {
    let dir = std::env::temp_dir().join(format!("k2mb-d1-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("d.redb");
    let key = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    let pack = || Arc::new(TextFilePack::new("markdown", 1 << 20));
    let config = || {
        let mut c = Config::default();
        c.edit_rate_per_s = 1_000_000;
        c.edit_burst = 1_000_000;
        c
    };
    let agent = |pid: &str| Participant::new(pid, pid, "#336699", ParticipantKind::Agent, "cli");
    let store = Arc::new(DocStore::open(&path).unwrap());
    let mgr = SessionManager::new(store.clone(), Arc::new(H), config());
    let s = mgr.open(pack(), &key, Some(b"seed\n")).unwrap();
    let p16 = s.join(agent("p16"), Stay::Held).unwrap().conn;
    let p14 = s.join(agent("p14"), Stay::Held).unwrap().conn;
    let rx = s.subscribe(p14).unwrap();
    s.replace_text(p16, ROOT, "seed\np16:3029\n").unwrap();
    s.replace_text(p14, ROOT, "seed\np14:3169\n").unwrap();
    let own: ChangeEvent = rx.try_recv().unwrap();
    let wide: ChangeEvent = rx.try_recv().unwrap();
    assert_eq!((own.pid.as_str(), wide.pid.as_str()), ("p16", "p14"));
    let id = store.find(&key).unwrap().unwrap();
    let told = |ev: &ChangeEvent| vec![(ev.root.clone(), ev.hunks.clone())];
    // p14's replace was widened: the record covers the token, the row holds the diff's hunk
    let rec = store.change_hunks(id, wide.version).unwrap().unwrap();
    assert_eq!(
        store.authored_hunks(id, wide.version).unwrap(),
        Some(told(&wide))
    );
    assert_eq!(rec, told(&wide));
    assert!(
        wide.hunks[0].start > 5,
        "the authored hunk starts inside the token"
    );
    // p16's insertion of its own line was not widened: no row, the record's hunks
    assert_eq!(store.authored_hunks(id, own.version).unwrap(), None);
    assert_eq!(
        store.change_hunks(id, own.version).unwrap(),
        Some(told(&own))
    );
    // restart
    drop(rx);
    drop(s);
    mgr.release().unwrap();
    drop(mgr);
    drop(store);
    let store = Arc::new(DocStore::open(&path).unwrap());
    let mgr = SessionManager::new(store.clone(), Arc::new(H), config());
    let s = mgr.open(pack(), &key, None).unwrap();
    let c = s.join(agent("p14"), Stay::Held).unwrap().conn;
    assert_eq!(s.read(c).unwrap().text_by_root[0].1, "seed\np14:3169\n");
    assert_eq!(
        store.change_hunks(id, wide.version).unwrap(),
        Some(told(&wide))
    );
    assert_eq!(
        store.change_hunks(id, own.version).unwrap(),
        Some(told(&own))
    );
    drop(s);
    mgr.release().unwrap();
    drop(mgr);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Blame, the log and the live window read a widened change as authored (v0.9.7), the
/// same before and after a restart (the store reopened from its file, a new manager) and,
/// after a purge of the change, with its authored inserts blanked: blame gives the token to
/// its rewriter and marks the line erased, the log keeps the change with its line counts,
/// and the live window reports the authored hunks (then refuses a base the purge reached).
#[test]
fn blame_log_and_the_window_read_the_authored_hunks_after_a_reopen_and_a_purge() {
    use k2_mpc_base::session::{LogItem, LogQuery, PurgeRequest, PurgeScope};

    /// Every participant may also purge (an owner).
    struct O;
    impl Host for O {
        fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
            Rights::full().with_owner(true)
        }
        fn now_ms(&self) -> u64 {
            1_000
        }
        fn on_event(&self, _: &AuditEvent) {}
    }

    let dir = std::env::temp_dir().join(format!("k2mb-d1-reads-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("d.redb");
    let key = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    let pack = || Arc::new(TextFilePack::new("markdown", 1 << 20));
    let config = || {
        let mut c = Config::default();
        c.edit_rate_per_s = 1_000_000;
        c.edit_burst = 1_000_000;
        c
    };
    let agent = |pid: &str| Participant::new(pid, pid, "#336699", ParticipantKind::Agent, "cli");
    // what a reader sees: per line its author and change and erased; the change's log line;
    // the window's events after `base`
    type Seen = (
        Vec<(String, Option<u64>, bool)>,
        (u64, u64),
        Vec<ChangeEvent>,
    );
    let seen = |s: &Session, c: ConnId, seq: u64, base: u64| -> Seen {
        let blame = s
            .blame(c, ROOT, None)
            .unwrap()
            .lines
            .into_iter()
            .map(|l| (l.author, l.change, l.erased))
            .collect();
        let log = s
            .log(c, &LogQuery::default())
            .unwrap()
            .into_iter()
            .find_map(|i| match i {
                LogItem::Change { info, .. } if info.seq == seq => {
                    Some((info.lines_added, info.lines_removed))
                }
                _ => None,
            })
            .expect("the change is in the log");
        let window = s.events_since(c, base).unwrap();
        (blame, log, window)
    };

    let store = Arc::new(DocStore::open(&path).unwrap());
    let mgr = SessionManager::new(store.clone(), Arc::new(O), config());
    let s = mgr.open(pack(), &key, Some(b"seed\n")).unwrap();
    let p16 = s.join(agent("p16"), Stay::Held).unwrap().conn;
    let p14 = s.join(agent("p14"), Stay::Held).unwrap().conn;
    let rx = s.subscribe(p14).unwrap();
    s.replace_text(p16, ROOT, "seed\np16:3029\n").unwrap();
    let base = s.read(p14).unwrap().version;
    s.replace_text(p14, ROOT, "seed\np14:3169\n").unwrap();
    let _own: ChangeEvent = rx.try_recv().unwrap();
    let wide: ChangeEvent = rx.try_recv().unwrap();
    let id = store.find(&key).unwrap().unwrap();
    assert!(
        store.authored_hunks(id, wide.version).unwrap().is_some(),
        "D1 widened p14's replace"
    );
    let live = seen(&s, p14, wide.version, base);
    // blame: the rewritten token's line is p14's change; the window: the authored hunks
    assert_eq!(
        live.0[1],
        ("p14".to_owned(), Some(wide.version), false),
        "{:?}",
        live.0
    );
    assert_eq!(live.2.len(), 1);
    assert_eq!(
        live.2[0].hunks, wide.hunks,
        "the window is what was broadcast"
    );
    assert_eq!(live.2[0].pid, "p14");

    // restart: the same reads
    drop(rx);
    drop(s);
    mgr.release().unwrap();
    drop(mgr);
    drop(store);
    let store = Arc::new(DocStore::open(&path).unwrap());
    let mgr = SessionManager::new(store.clone(), Arc::new(O), config());
    let s = mgr.open(pack(), &key, None).unwrap();
    let p14 = s.join(agent("p14"), Stay::Held).unwrap().conn;
    assert_eq!(seen(&s, p14, wide.version, base), live, "after a reopen");

    // purge p14's change: its authored inserts blanked, positions kept
    s.purge(
        p14,
        PurgeRequest::new(PurgeScope::Changes {
            from: wide.version,
            to: wide.version,
        }),
    )
    .unwrap();
    let check_purged = |s: &Session, store: &DocStore, c: ConnId| {
        let row = store.authored_hunks(id, wide.version).unwrap().unwrap();
        assert_eq!(row.len(), 1);
        assert_eq!(
            row[0]
                .1
                .iter()
                .map(|h| (h.start, h.end))
                .collect::<Vec<_>>(),
            wide.hunks
                .iter()
                .map(|h| (h.start, h.end))
                .collect::<Vec<_>>(),
            "positions stay"
        );
        assert!(row[0].1.iter().all(|h| h.insert.is_empty()), "{row:?}");
        assert_eq!(store.change_hunks(id, wide.version).unwrap(), Some(row));
        let blame = s.blame(c, ROOT, None).unwrap();
        let l = &blame.lines[1];
        assert_eq!(
            (l.author.as_str(), l.change, l.erased),
            ("p14", Some(wide.version), true),
            "{blame:?}"
        );
        let log = s.log(c, &LogQuery::default()).unwrap();
        assert!(
            log.iter().any(|i| matches!(i, LogItem::Change { info, .. }
                if info.seq == wide.version && (info.lines_added, info.lines_removed) == live.1)),
            "{log:?}"
        );
        let e = s
            .events_since(c, base)
            .expect_err("the purge took its text");
        assert_eq!(e.id(), "MPC-E112", "{e:?}");
        assert_eq!(s.read(c).unwrap().text_by_root[0].1, "seed\np14:3169\n");
    };
    check_purged(&s, &store, p14);

    // and after another reopen
    drop(s);
    mgr.release().unwrap();
    drop(mgr);
    drop(store);
    let store = Arc::new(DocStore::open(&path).unwrap());
    let mgr = SessionManager::new(store.clone(), Arc::new(O), config());
    let s = mgr.open(pack(), &key, None).unwrap();
    let p14 = s.join(agent("p14"), Stay::Held).unwrap().conn;
    check_purged(&s, &store, p14);
    drop(s);
    mgr.release().unwrap();
    drop(mgr);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An outside save is a diff-based change too (the amendment's list): D1 widens it like any
/// other. p1 changes the imported `beta` to `BETA` (its token); the file is then saved
/// outside the kit with `BETA2`. The save rewrites the token as the external participant's,
/// so p1's undo conflicts and leaves the save's word as it is; the channel and the live
/// window report the authored hunk (the `2`), and the store keeps it beside the widened
/// record. Without D1 (the reuse mutant) p1's undo turns `BETA2` into `beta2`.
fn outside_save_case(tag: &str) -> (UndoOutcome, String) {
    use k2_mpc_base::session::EXTERNAL_PID;

    let dir = std::env::temp_dir().join(format!("k2mb-d1-outside-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let file = dir.join("ws").join("a.md");
    std::fs::write(&file, "alpha beta\n").unwrap();
    let store = Arc::new(DocStore::open(dir.join("d.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store.clone(), Arc::new(H), c);
    let key = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    let s = mgr
        .open_file(
            Arc::new(TextFilePack::new("markdown", 1 << 20)),
            &key,
            &file,
            &dir.join("ws"),
        )
        .unwrap();
    let p1 = s
        .join(
            Participant::new("p1", "p1", "#336699", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    s.replace_text(p1, ROOT, "alpha BETA\n").unwrap();
    s.flush(p1).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha BETA\n");
    let base = s.read(p1).unwrap().version;
    let rx = s.subscribe(p1).unwrap();
    std::fs::write(&file, "alpha BETA2\n").unwrap();
    s.file_changed().unwrap();
    let text = |s: &Session| s.read(p1).unwrap().text_by_root[0].1.clone();
    assert_eq!(text(&s), "alpha BETA2\n");
    let ev: ChangeEvent = rx.try_recv().unwrap();
    assert_eq!(ev.pid, EXTERNAL_PID);
    let authored = vec![k2_mpc_base::text::diff::Hunk {
        start: 10,
        end: 10,
        insert: "2".into(),
    }];
    assert_eq!(ev.hunks, authored, "the channel reports the authored hunk");
    let window = s.events_since(p1, base).unwrap();
    assert_eq!(window.len(), 1);
    assert_eq!(window[0].hunks, authored, "so does the live window");
    let id = store.find(&key).unwrap().unwrap();
    let row = store.authored_hunks(id, ev.version).unwrap();
    let undo = s.undo_with(p1, &UndoRequest::steps(1)).unwrap();
    let after = text(&s);
    if row.is_some() {
        assert_eq!(row, Some(vec![(ROOT.to_owned(), authored)]));
        let blame = s.blame(p1, ROOT, None).unwrap();
        assert_eq!(blame.lines[0].author, EXTERNAL_PID, "{blame:?}");
    }
    drop(rx);
    drop(s);
    mgr.release().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (undo, after)
}

#[test]
fn an_outside_save_is_widened_like_any_diff_based_change() {
    let _g = ORDER.lock().unwrap_or_else(|p| p.into_inner());
    let (undo, after) = outside_save_case("d1");
    assert!(
        matches!(undo, UndoOutcome::Conflicts { step: 1, .. }),
        "p1's undo conflicts: {undo:?}"
    );
    assert_eq!(after, "alpha BETA2\n", "the save's word is untouched");
    // the negative control: without D1 the undo changes the save's word
    let (undo, after) = with_d1_mutant(D1Mutant::ReuseForeign, || outside_save_case("reuse"));
    assert!(
        !matches!(undo, UndoOutcome::Conflicts { .. }) && after != "alpha BETA2\n",
        "without D1: {undo:?}, {after:?}"
    );
}
