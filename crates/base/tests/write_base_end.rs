//! `write --base` at the end of the text (undo spec §4, amendment "O5 end-of-text" r3, whole
//! lines): a line break separates text one side left without a final line break from the
//! other side's line written after it, as part of the writer's change; the final line break
//! follows whoever wrote the last line. Through a session, public API only: the amendment's
//! table with the shapes that showed a character merge's faults; the writer's undo and revert;
//! the live window; D1; a CRLF document; the `--partial` pair, ignored until it exists.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::differential::{with_merge_mutant, MergeMutant};
use k2_mpc_base::session::{
    AuditEvent, ConnId, EditOptions, Host, Participant, ParticipantKind, Rights, Session,
    SessionManager, Stay, TextFilePack, UndoOutcome, WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::text::merge::{merge3, Merge};
use k2_mpc_base::Edit;

const PACK: TextFilePack = TextFilePack::new("wbe", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;

struct TestHost(AtomicU64);

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: std::path::PathBuf,
    _mgr: SessionManager,
    s: Arc<Session>,
    /// A person typing live, and an agent writing with `write --base`.
    person: ConnId,
    agent: ConnId,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(file: &[u8]) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-wbe-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(
        store,
        Arc::new(TestHost(AtomicU64::new(1_000_000))),
        Default::default(),
    );
    let key = DocKey {
        space: "ws".into(),
        path: "notes.txt".into(),
    };
    let s = mgr.open(Arc::new(PACK), &key, Some(file)).unwrap();
    let join = |pid: &str, kind| {
        s.join(
            Participant::new(pid, pid, "#123456", kind, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    };
    let person = join("pat", ParticipantKind::Person);
    let agent = join("agt", ParticipantKind::Agent);
    Fx {
        dir,
        _mgr: mgr,
        s,
        person,
        agent,
    }
}

impl Fx {
    fn text(&self) -> String {
        self.s.read(self.agent).unwrap().text_by_root.remove(0).1
    }
    fn version(&self) -> u64 {
        self.s.read(self.agent).unwrap().version
    }
    /// The person's live change: the doc becomes `new` (one hunk over what differs).
    fn live(&self, new: &str) {
        let old = self.text();
        let pre = old
            .bytes()
            .zip(new.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        let suf = old[pre..]
            .bytes()
            .rev()
            .zip(new[pre..].bytes().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let h = Hunk {
            start: pre,
            end: old.len() - suf,
            insert: new[pre..new.len() - suf].to_owned(),
        };
        let e = Edit::Text {
            root: ROOT.into(),
            hunks: vec![h],
        };
        self.s.edit(self.person, vec![e], None).unwrap();
    }
    fn write_base(&self, base: u64, ours: &str) -> WriteOutcome {
        self.s.write_base(self.agent, ROOT, base, ours).unwrap()
    }
}

/// One case: the doc starts as `base`; the agent reads it; the person makes it `theirs`; the
/// agent writes `ours` from its read. Returns the fixture and the write's version.
fn case(base: &str, theirs: &str, ours: &str, want: &str) -> (Fx, u64) {
    let f = fx(base.as_bytes());
    let v = f.version();
    f.live(theirs);
    assert_eq!(f.text(), theirs);
    let WriteOutcome::Applied(r) = f.write_base(v, ours) else {
        panic!("not applied: base {base:?} theirs {theirs:?} ours {ours:?}")
    };
    assert_eq!(
        f.text(),
        want,
        "base {base:?} theirs {theirs:?} ours {ours:?}"
    );
    (f, r.version)
}

fn h(start: usize, end: usize, insert: &str) -> Hunk {
    Hunk {
        start,
        end,
        insert: insert.into(),
    }
}

/// Amendment O5 end-of-text r3, the merge itself: `(name, base, live, agent, result)`; `None` is
/// a conflict. A–D as r2 gave them; r2's character merges, now whole lines, are conflicts; and
/// the shapes that showed a character merge's faults, each a conflict or whole lines.
#[test]
fn the_table_of_amendment_r3() {
    let cases: &[(&str, &str, &str, &str, Option<&str>)] = &[
        ("A", "b\n", "a", "b\nZ\n", Some("a\nZ\n")),
        ("B", "x\n", "x\nA", "x\nB\n", Some("x\nA\nB\n")),
        ("C", "x\n", "x\nT\n", "x", Some("x\nT\n")),
        ("D", "x\n", "x\nA", "x\nB", Some("x\nA\nB")),
        ("leading break", "b\n", "a", "b\n\nZ\n", Some("a\n\nZ\n")),
        (
            "C, more deleted",
            "x\nk\n",
            "x\nk\nT\n",
            "x",
            Some("x\nT\n"),
        ),
        (
            "open at its own end",
            "x\n",
            "x\nA\n",
            "x\nB",
            Some("x\nA\nB"),
        ),
        ("both closed", "x\n", "x\nA\n", "x\nB\n", Some("x\nA\nB\n")),
        ("nothing opened", "x\n", "x\n", "x\nB\n", Some("x\nB\n")),
        ("empty base", "", "a\n", "b\n", None),
        ("identity, theirs", "x\n", "x", "x\n", Some("x")),
        ("identity, ours", "b\n", "b\n", "a", Some("a")),
        ("changed: no final break", "x", "xA", "xB", None),
        ("changed: no final break", "b", "c", "xb", None),
        ("changed: narrowing", "\n", "aaa", "b\n", None),
        ("changed: narrowing", "foo\n", "foo bar", "Xfoo\n", None),
        ("changed: break removed", "a\n", "g\n", "a", Some("g\n")),
        ("xz", "p\nz\n", "z\n", "xp\nz\n", None),
        ("xq", "p\n", "q\nr\n", "xp\n", None),
        ("x0x0", "p00:0\n", "x0p00:0\n", "x0p00:0", Some("x0p00:0")),
        (
            "edited and closed",
            "p00:0\np00:0",
            "p00:0\np00:0\n",
            "p00:0\np00:0x",
            Some("p00:0\np00:0x"),
        ),
        (
            "emoji, closed and rewritten",
            "🎉",
            "\n🎉日",
            "🎉\n",
            Some("\n🎉日"),
        ),
        ("one line split, closed", "||🎉", "||\n", "ab\n🎉||🎉", None),
        (
            "different words",
            "p01:0 w0\n",
            "wt w0\n",
            "p01:0 wo\n",
            None,
        ),
        (
            "both rewrite one line",
            "p01:2\n",
            "p00:0 w1 w0\np00:0\np02:3\n",
            "p01:3 w0 w0\np00:3 w0 w3\n",
            None,
        ),
    ];
    for (name, b, t, o, want) in cases {
        let got = match merge3(b, o, t) {
            Merge::Clean(c) => Some(c.text),
            Merge::Conflicts(_) => None,
        };
        assert_eq!(
            got.as_deref(),
            *want,
            "{name}: {b:?} live {t:?} agent {o:?}"
        );
    }
}

/// r2's clause: the separator is part of the writer's applied change. Its undo removes it, and
/// so does a revert of that change. C applies nothing: no change at all.
#[test]
fn the_separator_is_the_writers_undo_and_revert_remove_it() {
    for (base, theirs, ours, want) in [
        ("b\n", "a", "b\nZ\n", "a\nZ\n"),
        ("x\n", "x\nA", "x\nB\n", "x\nA\nB\n"),
        ("x\n", "x\nA", "x\nB", "x\nA\nB"),
    ] {
        let (f, _) = case(base, theirs, ours, want);
        let out = f.s.undo(f.agent, 1).unwrap();
        assert!(matches!(out, UndoOutcome::Applied { .. }), "{out:?}");
        assert_eq!(f.text(), theirs, "undo: {base:?}");

        let (f, change) = case(base, theirs, ours, want);
        let out = f.s.revert(f.agent, change, false).unwrap();
        assert!(matches!(out, UndoOutcome::Applied { .. }), "{out:?}");
        assert_eq!(f.text(), theirs, "revert: {base:?}");
    }
    // C: the agent's text lost its final break, the person appended a line: nothing applies
    let f = fx(b"x\n");
    let v = f.version();
    f.live("x\nT\n");
    let before = f.version();
    assert!(matches!(f.write_base(v, "x"), WriteOutcome::Applied(_)));
    assert_eq!(f.text(), "x\nT\n");
    assert_eq!(f.version(), before, "no change");
}

/// r2's clause: the separator is in the E-LIVE window as part of that change's hunks. A live edit
/// made at the version before the write is carried over the whole of it, the separator's byte
/// included; before it, nothing moves.
#[test]
fn the_separator_is_in_the_live_window() {
    let (f, write) = case("b\n", "a", "b\nZ\n", "a\nZ\n");
    let before_write = write - 1;
    let at = |hunks: Vec<Hunk>| {
        f.s.edit_at_base(f.person, ROOT, before_write, hunks, &EditOptions::new())
            .unwrap()
    };
    // at the point the write inserted: the change applied first stays left, so the person's
    // text goes after all three bytes of the write ("\nZ\n")
    let r = at(vec![h(1, 1, "!")]);
    assert_eq!(r.hunks, vec![h(4, 4, "!")]);
    assert_eq!(f.text(), "a\nZ\n!");
    // and before it, untouched
    let (f, write) = case("b\n", "a", "b\nZ\n", "a\nZ\n");
    let r =
        f.s.edit_at_base(
            f.person,
            ROOT,
            write - 1,
            vec![h(0, 0, "x")],
            &EditOptions::new(),
        )
        .unwrap();
    assert_eq!(r.hunks, vec![h(0, 0, "x")]);
    assert_eq!(f.text(), "xa\nZ\n");
}

/// r2's clause: D1 holds. In case A the agent's line doesn't join the person's word, so D1 has
/// nothing to widen over: the person's `a` stays theirs, and their undo finds it. The
/// control: with v0.9.7's merge planted, the agent's `Z` joins `a` into one token, D1 makes it
/// the agent's, and the person's undo no longer finds their `a`.
#[test]
fn d1_case_a_keeps_the_persons_word_theirs() {
    let (f, _) = case("b\n", "a", "b\nZ\n", "a\nZ\n");
    // the line's own break is the agent's separator, and blame gives a line to its newest
    // character, its "\n" included (a `\n` typed into a line attributes the first half to its
    // typist): the label moves; the person's characters and their undo don't
    let b = f.s.blame(f.agent, ROOT, None).unwrap();
    assert_eq!(b.lines[0].text, "a");
    assert_eq!(b.lines[0].author, "agt");
    let out = f.s.undo(f.person, 1).unwrap();
    assert!(matches!(out, UndoOutcome::Applied { .. }), "{out:?}");
    assert_eq!(f.text(), "b\n\nZ\n");
    with_merge_mutant(MergeMutant::CharSides, || {
        let (f, _) = case("b\n", "a", "b\nZ\n", "aZ\n");
        let out = f.s.undo(f.person, 1).unwrap();
        assert!(
            !matches!(out, UndoOutcome::Applied { .. }),
            "with v0.9.7's merge, D1 took the person's word: {out:?}"
        );
    });
}

/// r2's clause: the rule runs on the doc's text, after `normalize_input`. So through a session a
/// file ending in "\r\n" ends with a final break, and so does a writer's body in CRLF: the doc
/// is `a\nZ\n` and the file `a\r\nZ\r\n`, never mixed.
#[test]
fn crlf_document() {
    let f = fx(b"b\r\n");
    assert_eq!(f.text(), "b\n");
    let v = f.version();
    f.live("a");
    let out = f.write_base(v, "b\r\nZ\r\n");
    assert!(matches!(out, WriteOutcome::Applied(_)), "{out:?}");
    assert_eq!(f.text(), "a\nZ\n");
    assert_eq!(f.s.read(f.agent).unwrap().bytes, b"a\r\nZ\r\n");
}

/// r2's `--partial` clause: the separator goes in only if the landing change is applied.
#[test]
#[ignore = "write --base --partial isn't implemented yet"]
fn partial_with_the_landing_change_applied_gets_the_separator() {
    unimplemented!("write --base --partial: the landing change applied → the separator");
}

/// r2's `--partial` clause: the landing change skipped (it conflicts), so no separator.
#[test]
#[ignore = "write --base --partial isn't implemented yet"]
fn partial_with_the_landing_change_skipped_gets_no_separator() {
    unimplemented!("write --base --partial: the landing change skipped → no separator");
}
