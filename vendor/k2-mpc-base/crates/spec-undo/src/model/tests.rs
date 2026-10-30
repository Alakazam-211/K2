//! The spec's worked cases, on the oracle alone.

use super::*;

fn e(s: usize, t: usize, i: &str) -> Edit {
    Edit {
        start: s,
        end: t,
        insert: i.into(),
    }
}

/// The worked cases of the U and O rules run on letters, one token each (D1 is the identity
/// there, v1.10); D1's own cases set words.
fn chars() -> Bounds {
    Bounds {
        tokens: k2_mpc_base::text::lines::Tokens::Chars,
        ..Bounds::default()
    }
}

fn one(text: &str) -> Model {
    Model::new(vec![text.into()], 1, chars())
}

fn words(text: &str) -> Model {
    Model::new(vec![text.into()], 1, Bounds::default())
}

fn all(n: usize) -> Request {
    Request::new(n, Mode::All)
}

fn partial(n: usize) -> Request {
    Request::new(n, Mode::Partial)
}

#[test]
fn undo_redo_own_batch() {
    let mut m = one("abc\n");
    assert_eq!(
        m.edit("a", None, vec![(0, vec![e(1, 2, "X")])]),
        Outcome::Changed(Some(2))
    );
    m.edit("b", None, vec![(0, vec![e(4, 4, "z\n")])]);
    assert_eq!(m.depths("a"), (1, 0));
    let (o, _) = m.undo("a", None, &all(1));
    assert!(matches!(o, Outcome::Applied { steps: 1, .. }), "{o:?}");
    assert_eq!(m.texts[0], "abc\nz\n");
    assert_eq!(m.depths("a"), (0, 1));
    m.redo("a", None, &all(1));
    assert_eq!(m.texts[0], "aXc\nz\n");
    // U7: a new batch empties the redo stack
    m.undo("a", None, &all(1));
    m.edit("a", None, vec![(0, vec![e(0, 0, "!")])]);
    assert_eq!(m.depths("a"), (1, 0));
    assert_eq!(
        m.redo("a", None, &all(1)).0,
        Outcome::Nothing(Notice::Plain)
    );
}

#[test]
fn u12_worked_case_and_whole_doc_delete() {
    // B deletes the last line, someone appends: undo lands theirs first
    let mut m = one("a\nx\n");
    m.edit("b", None, vec![(0, vec![e(2, 4, "")])]);
    m.edit("c", None, vec![(0, vec![e(2, 2, "y\n")])]);
    let (o, h) = m.undo("b", None, &all(1));
    assert!(matches!(o, Outcome::Applied { .. }), "{o:?}");
    assert!(h.last_line_after_append);
    assert_eq!(m.texts[0], "a\ny\nx\n");
    // B deletes the whole doc, someone appends: the base is empty, so it conflicts
    let mut m = one("x\n");
    m.edit("b", None, vec![(0, vec![e(0, 2, "")])]);
    m.edit("c", None, vec![(0, vec![e(0, 0, "y\n")])]);
    let (o, h) = m.undo("b", None, &all(1));
    assert_eq!(
        o,
        Outcome::Conflicts {
            step: 1,
            regions: vec![Report {
                root: 0,
                start: 0,
                end: 2,
                lines: (1, 1),
                kind: Kind::Overlap
            }]
        }
    );
    assert!(h.whole_doc_delete_then_append);
}

#[test]
fn partial_stays_on_top_and_drop() {
    let mut m = one("one\ntwo\nthree\n");
    // one batch, two regions
    m.edit(
        "a",
        None,
        vec![(0, vec![e(0, 3, "ONE"), e(8, 13, "THREE")])],
    );
    m.edit("b", None, vec![(0, vec![e(10, 10, "x")])]); // inside THREE
    let (o, _) = m.undo("a", None, &all(1));
    let reported = match o {
        Outcome::Conflicts { step: 1, regions } => regions,
        o => panic!("{o:?}"),
    };
    let (o, h) = m.undo("a", None, &partial(1));
    assert!(h.multi_hunk_partial);
    assert_eq!(
        o,
        Outcome::Applied {
            steps: 1,
            skipped: reported,
            blocked: None
        }
    );
    assert_eq!(m.texts[0], "one\ntwo\nTHxREE\n");
    // redo reapplies only what the undo applied
    let (_, h) = m.redo("a", None, &all(1));
    assert!(h.redo_after_partial);
    assert_eq!(m.texts[0], "ONE\ntwo\nTHxREE\n");
    // an all-conflicting step stays on top until dropped
    let mut m = one("ab");
    m.edit("a", None, vec![(0, vec![e(0, 1, "A")])]);
    m.edit("a", None, vec![(0, vec![e(1, 2, "B")])]);
    m.edit("b", None, vec![(0, vec![e(1, 2, "C")])]);
    let (o, h) = m.undo("a", None, &partial(2));
    assert!(matches!(o, Outcome::Blocked { .. }), "{o:?}");
    assert!(h.stays_on_top);
    assert_eq!(m.depths("a"), (2, 0));
    assert_eq!(
        m.undo("a", None, &Request::new(1, Mode::Drop)).0,
        Outcome::Dropped
    );
    m.undo("a", None, &all(1));
    assert_eq!(m.texts[0], "aC");
    assert_eq!(m.depths("a"), (0, 1));
}

#[test]
fn taint_and_shared_edge() {
    let mut m = one("abc def ghi");
    m.edit("a", None, vec![(0, vec![e(4, 7, "DEF")])]);
    m.edit("b", None, vec![(0, vec![e(6, 8, "")])]); // across DEF's end
    let (o, h) = m.undo("a", None, &all(1));
    assert!(h.taint);
    assert!(matches!(
        o,
        Outcome::Conflicts { ref regions, .. } if regions[0].kind == Kind::Taint
    ));
    // U8: two regions come to touch; an insertion at the shared edge goes left only
    let mut m = one("a\nX\nb\nY\nc\n");
    m.edit("a", None, vec![(0, vec![e(2, 3, "1"), e(6, 7, "2")])]);
    m.edit("b", None, vec![(0, vec![e(3, 6, "")])]); // "a\n12\nc\n"
    m.edit("b", None, vec![(0, vec![e(3, 3, "Z")])]); // "a\n1Z2\nc\n"
    let r = &m.stacks["a"].undo[0].regions;
    assert_eq!((r[0].start, r[0].end, r[1].start, r[1].end), (2, 4, 4, 5));
}

#[test]
fn revert_purge_restart_window() {
    let mut m = one("hello\n");
    let secret = "QWERTYUIOPASDFGHJKLZXCVBNMQWERTY";
    m.edit("a", None, vec![(0, vec![e(6, 6, secret)])]);
    m.edit("a", Some("p"), vec![(0, vec![e(0, 1, "H")])]);
    m.purge_text("o", None, secret);
    assert_eq!(m.texts[0], "Hello\n");
    assert_eq!(m.depths("a"), (0, 0));
    let (o, h) = m.undo("a", None, &all(1));
    assert_eq!(o, Outcome::Nothing(Notice::ClearedByPurge));
    assert!(h.cleared_by_purge);
    // the secret's change is erased in its record
    let (o, _) = m.revert("o", None, 2, false);
    assert!(matches!(o, Outcome::Conflicts { ref regions, .. } if regions[0].kind == Kind::Erased));
    // restart: stacks lost, revert still works
    m.restart();
    assert_eq!(
        m.undo("a", None, &all(1)).0,
        Outcome::Nothing(Notice::HistoryLost)
    );
    let (o, _) = m.revert("a", None, 3, false);
    assert!(matches!(o, Outcome::Applied { .. }), "{o:?}");
    assert_eq!(m.texts[0], "hello\n");
    // the window: everything but the pending on-behalf change goes
    m.bounds.keep_changes = 0;
    m.tick();
    assert_eq!(m.revert("a", None, 2, false).0, Outcome::TooOld);
    assert!(m.records.contains_key(&3));
}

#[test]
fn undo_for_person() {
    let mut m = one("abc");
    m.edit("bot", Some("p"), vec![(0, vec![e(0, 1, "A")])]);
    m.edit("bot", Some("q"), vec![(0, vec![e(2, 3, "C")])]);
    let mut r = all(1);
    r.for_person = Some("p".into());
    let (o, h) = m.undo("bot", Some("p"), &r);
    assert!(matches!(o, Outcome::Applied { .. }), "{o:?}");
    assert!(h.undo_for);
    assert_eq!(m.texts[0], "abC");
}

#[test]
fn write_base_rules() {
    let mut m = Model::new(vec![String::new()], 1, chars());
    m.write_base("a", None, 0, "", "x\n");
    let (o, h) = m.write_base("b", None, 0, "", "y\n");
    assert_eq!(o, Outcome::WriteConflict);
    assert!(h.empty_base_double_write);
    let (o, h) = m.write_base("b", None, 0, "x\n", "x\ny\n");
    assert_eq!(o, Outcome::Changed(Some(3)));
    assert!(!h.eof_append_write);
    m.write_base("a", None, 0, "x\n", "x\nz\n");
    assert_eq!(m.texts[0], "x\ny\nz\n");
}

#[test]
fn u22a_revert_inherits_a_touching_neighbours_end_taint() {
    // two regions; a delete crosses the left one's end; another deletes the gap
    let mut m = one("abcdefghij\n");
    m.edit("a", None, vec![(0, vec![e(1, 2, "B"), e(7, 8, "H")])]); // change 2
    m.edit("b", None, vec![(0, vec![e(1, 3, "")])]);
    m.edit("b", None, vec![(0, vec![e(1, 5, "")])]); // "aHij\n": they touch
    let undo = m.clone().undo("a", None, &all(1)).0;
    let (rv, h) = m.clone().revert("a", None, 2, false);
    let taint = |s, t| Report {
        root: 0,
        start: s,
        end: t,
        lines: (1, 1),
        kind: Kind::Taint,
    };
    assert_eq!(
        undo,
        Outcome::Conflicts {
            step: 1,
            regions: vec![taint(1, 1)]
        }
    );
    assert_eq!(
        rv,
        Outcome::Conflicts {
            step: 1,
            regions: vec![taint(1, 1), taint(1, 2)]
        }
    );
    assert!(h.revert_superset_ok);
    assert_eq!((h.revert_regions, h.revert_extra_conflicts), (2, 1));
}

/// U22a's property on random histories: revert's conflicts ⊇ undo's on the same state, the
/// same bounds, and the same text wherever both apply.
#[test]
fn u22a_revert_conflicts_are_a_superset_of_undos() {
    use rand::rngs::SmallRng;
    use rand::{Rng, SeedableRng};
    let mut rng = SmallRng::seed_from_u64(22);
    let mut extra = 0;
    for _ in 0..3000 {
        let mut m = one("ab\nabc\naab\nb\n");
        let n = m.texts[0].len();
        let mut cuts = [
            rng.gen_range(0..n),
            rng.gen_range(0..n),
            rng.gen_range(0..n),
        ];
        cuts.sort_unstable();
        let mut hs = Vec::new();
        let mut at = 0;
        for c in cuts {
            if c < at {
                continue;
            }
            let end = (c + rng.gen_range(0..3)).min(n);
            let ins = ["", "a", "ab", "\n"][rng.gen_range(0..4)];
            if c == end && ins.is_empty() {
                continue;
            }
            hs.push(e(c, end, ins));
            at = end + 1;
        }
        if hs.is_empty() {
            continue;
        }
        let Outcome::Changed(Some(id)) = m.edit("a", None, vec![(0, hs)]) else {
            continue;
        };
        for _ in 0..rng.gen_range(1..8) {
            let t = m.texts[0].clone();
            // half the time at an edge of one of the change's regions
            let edges: Vec<usize> = m.records[&id]
                .regions
                .iter()
                .flat_map(|r| [r.start, r.end])
                .collect();
            let p = if rng.gen_bool(0.5) {
                edges[rng.gen_range(0..edges.len())]
            } else {
                rng.gen_range(0..=t.len())
            };
            let p = p.saturating_sub(rng.gen_range(0..2)).min(t.len());
            let end = (p + rng.gen_range(0..3)).min(t.len());
            let ins = ["", "a", "b\n"][rng.gen_range(0..3)];
            if p == end && ins.is_empty() {
                continue;
            }
            m.edit("b", None, vec![(0, vec![e(p, end, ins)])]);
        }
        let (_, h) = m.clone().revert("a", None, id, false);
        assert!(h.revert_superset_ok, "{m:?}");
        extra += h.revert_extra_conflicts;
    }
    assert!(
        extra > 0,
        "the inheritance never fired: the test reaches nothing"
    );
}

/// U22a (v1.6), restart transparency: a pending on-behalf change's two regions; a deletion
/// crosses the left one's **start**; the window moves; the regions are made to touch. The
/// right region inherits the left's (clean) end, so revert conflicts on the left region
/// only, with a restart at any point or none (v1.5 tainted both after a restart).
#[test]
fn u22a_revert_is_restart_transparent() {
    let run = |restart_at: Option<usize>| {
        let mut m = one("abcdefghij\n");
        m.bounds.keep_changes = 2;
        for k in 0..8 {
            let n = m.texts[0].len();
            match k {
                0 => m.edit(
                    "bot",
                    Some("p"),
                    vec![(0, vec![e(1, 3, "BC"), e(7, 8, "H")])],
                ),
                1 => m.edit("b", None, vec![(0, vec![e(0, 2, "")])]), // across the left start
                2..=4 => m.edit("b", None, vec![(0, vec![e(n, n, &format!("{k}\n"))])]),
                5 => {
                    m.tick();
                    Outcome::Changed(None)
                }
                _ => m.edit("b", None, vec![(0, vec![e(1, 3, "")])]), // at 7: they touch
            };
            if restart_at == Some(k) {
                m.restart();
            }
        }
        assert!(m.records.contains_key(&2) && !m.records.contains_key(&3));
        (
            m.clone().revert("b", None, 2, false).0,
            m.revert("b", None, 2, true).0,
        )
    };
    let want = run(None);
    assert_eq!(
        want.0,
        Outcome::Conflicts {
            step: 1,
            regions: vec![Report {
                root: 0,
                start: 0,
                end: 1,
                lines: (1, 1),
                kind: Kind::Taint
            }]
        }
    );
    assert!(matches!(&want.1, Outcome::Applied { skipped, .. } if skipped.len() == 1));
    for k in 0..8 {
        assert_eq!(run(Some(k)), want, "a restart after step {k}");
    }
}

/// F3 (v1.7, exact hunk sets): p2's insertion at the start of p1's region repeats its text;
/// p1's undo turns back its own `b` (p2's stays in front), so p2's undo applies, and neither
/// deletes a character whose root it may not remove.
#[test]
fn f3_exact_hunk_sets() {
    let mut m = one("ab");
    m.edit("p1", None, vec![(0, vec![e(0, 1, "b")])]); // "bb"
    m.edit("p2", None, vec![(0, vec![e(0, 0, "b")])]); // "bbb", p2's first
    let (o, h) = m.undo("p1", None, &all(1));
    assert!(matches!(o, Outcome::Applied { .. }), "{o:?}");
    assert!(h.rediff_differs && !h.foreign_deleted);
    assert_eq!(m.texts[0], "bab");
    let (o, h) = m.undo("p2", None, &all(1));
    assert!(
        matches!(o, Outcome::Applied { .. }) && !h.foreign_deleted,
        "{o:?}"
    );
    assert_eq!(m.texts[0], "ab");
}

/// The roots of root 0's live characters.
fn roots(m: &Model) -> Vec<Prov> {
    m.elems[0].iter().map(|x| x.prov).collect()
}

/// v1.8 regression (a): a revert of a change undone and redone **applies** (v1.7 conflicted:
/// the redo's elements are new, U18a; by root they are the change's again, U23).
#[test]
fn regression_a_revert_after_undo_and_redo_applies() {
    let mut m = one("x\n");
    m.edit("a", None, vec![(0, vec![e(2, 2, "a\n")])]); // change 2
    let made = roots(&m);
    m.undo("a", None, &all(1));
    m.redo("a", None, &all(1));
    assert_eq!(m.texts[0], "x\na\n");
    // the redo's elements are new (U18a), with the change's roots (U23)
    assert_ne!(m.elems[0][2].id, m.elems[0][2].prov);
    assert_eq!(roots(&m), made);
    let (o, h) = m.revert("b", None, 2, false);
    assert!(matches!(o, Outcome::Applied { .. }), "{o:?}");
    assert!(!h.foreign_deleted && h.base_by_root && h.concurrent_restore);
    assert_eq!(m.texts[0], "x\n");
}

/// v1.8 regression (b): b edits inside a's region and undoes it; the text b's undo puts back
/// has a's roots, so **a's undo applies** (v1.7: a conflict, U10a by element).
#[test]
fn regression_b_undo_after_a_touch_and_undo_inside_applies() {
    let mut m = one("xy");
    m.edit("a", None, vec![(0, vec![e(0, 1, "A")])]); // "Ay"
    m.edit("b", None, vec![(0, vec![e(0, 1, "B")])]); // "By", inside a's region
    let (o, _) = m.undo("b", None, &all(1)); // "Ay": a new element, root a's A
    assert!(matches!(o, Outcome::Applied { .. }), "{o:?}");
    assert_eq!(m.texts[0], "Ay");
    assert_eq!(m.who[m.elems[0][0].owner as usize], "b");
    let (o, h) = m.undo("a", None, &all(1));
    assert!(matches!(o, Outcome::Applied { .. }), "{o:?}");
    // it removed b's element, whose root a inserted: U10a by root
    assert!(!h.foreign_deleted && h.base_by_root);
    assert_eq!(m.texts[0], "xy");
    assert_eq!(roots(&m), roots(&one("xy")));
}

/// v1.8, the duplication trap (I1): b deletes the seed's `X`; b undoes (X1, root X); c
/// deletes X1; then c's undo and a's revert of b's delete (change 2) both want root X back.
/// Whichever comes second finds it identical (O4: taken once): never two live X. Mid-text
/// and at the doc's end, in both orders.
#[test]
fn duplication_trap_is_taken_once() {
    for seed in ["pXq", "pX"] {
        for revert_first in [false, true] {
            let mut m = one(seed);
            m.edit("b", None, vec![(0, vec![e(1, 2, "")])]); // change 2
            m.undo("b", None, &all(1)); // X1, root X
            m.edit("c", None, vec![(0, vec![e(1, 2, "")])]); // deletes X1
            let first = |m: &mut Model| {
                if revert_first {
                    m.revert("a", None, 2, false)
                } else {
                    m.undo("c", None, &all(1))
                }
            };
            let (o, h) = first(&mut m);
            assert!(matches!(o, Outcome::Applied { .. }), "{seed} {o:?}");
            assert!(!h.i1_violated && !h.concurrent_restore);
            assert_eq!(m.texts[0], seed);
            let (o, h) = if revert_first {
                m.undo("c", None, &all(1))
            } else {
                m.revert("a", None, 2, false)
            };
            assert!(matches!(o, Outcome::Applied { .. }), "{seed} {o:?}");
            assert!(
                h.o4_restore && h.concurrent_restore && !h.i1_violated,
                "{h:?}"
            );
            assert_eq!(m.texts[0], seed, "taken once");
            assert!(m.i1());
        }
    }
}

/// v1.8, O4a at the doc's end: seed `ab`; p1 deletes `b`; p1 undoes (b1, root b); p2 types
/// `a` before b1 (pulled into change 2's window at its start). Revert of change 2: theirs
/// inserts `ab1`, ours `b` at the same point, at the doc's end (O5 would land both); b's
/// root is live, so O4a: a conflict, never two live b. Mutant (x)'s witness is the same
/// without p2's edit (`tests/mutants.rs`).
#[test]
fn o4a_at_the_end() {
    let mut m = one("ab");
    m.edit("p1", None, vec![(0, vec![e(1, 2, "")])]); // change 2: "a"
    m.undo("p1", None, &all(1)); // "ab"
    m.edit("p2", None, vec![(0, vec![e(1, 1, "a")])]); // "aab"
    let (o, h) = m.revert("p1", None, 2, false);
    let want = vec![Report {
        root: 0,
        start: 1,
        end: 3,
        lines: (1, 1),
        kind: Kind::Overlap,
    }];
    assert_eq!(
        o,
        Outcome::Conflicts {
            step: 1,
            regions: want
        }
    );
    assert!(h.o4a && h.concurrent_restore && h.notes[0].o4a && h.notes[0].at_end);
    // without p2's edit, the revert finds b's restoration identical (O4)
    let mut m = one("ab");
    m.edit("p1", None, vec![(0, vec![e(1, 2, "")])]);
    m.undo("p1", None, &all(1));
    let (o, h) = m.revert("p1", None, 2, false);
    assert!(
        matches!(o, Outcome::Applied { .. }) && h.o4_restore,
        "{o:?}"
    );
    assert_eq!(m.texts[0], "ab");
}

/// v1.8, O4a in mid-text (the bounded world's least case, depth 5): p1 inserts `a` at 1; p2
/// deletes the seed's `a` and replaces `b` with `a` (two regions); p1 undoes its insert;
/// p1 reverts p2's change: its two hunks touch and are joined, a replacement starting at
/// p2's first region (empty, at 0), which so stays empty (R1) while the seed's `a` comes back
/// outside it. p2's undo: that region's ours puts back the seed's `a`, whose root is live
/// (O4a), and `--partial` must not apply it.
#[test]
fn o4a_mid_text() {
    let mut m = one("ab");
    m.edit("p1", None, vec![(0, vec![e(1, 1, "a")])]); // "aab"
    m.edit("p2", None, vec![(0, vec![e(0, 1, ""), e(2, 3, "a")])]); // change 3: "aa"
    m.undo("p1", None, &all(1)); // "a"
    let (o, _) = m.revert("p1", None, 3, false); // "ab", the seed's roots back
    assert!(matches!(o, Outcome::Applied { .. }), "{o:?}");
    assert_eq!(roots(&m), roots(&one("ab")));
    let (o, h) = m.undo("p2", None, &partial(1));
    assert!(matches!(o, Outcome::Blocked { .. }), "{o:?}");
    let n = &h.notes[0];
    assert!(n.o4a && !n.at_end && h.o4a, "{:?}", h.notes);
    assert_eq!(m.texts[0], "ab");
    assert!(m.i1());
}

/// v1.8, O4 on roots, not text: a fresh `X` someone typed where an undo would put back `X`
/// is not that `X`. At the doc's end both land (O5, theirs first); in mid-text the two
/// insertions at one point conflict. v1.7 took them once.
#[test]
fn o4_is_on_roots_not_text() {
    let mut m = one("pX");
    m.edit("b", None, vec![(0, vec![e(1, 2, "")])]); // "p"
    m.edit("d", None, vec![(0, vec![e(1, 1, "X")])]); // a fresh X
    let (o, h) = m.undo("b", None, &all(1));
    assert!(matches!(o, Outcome::Applied { .. }), "{o:?}");
    assert!(h.eof_append_undo && !h.o4_restore && !h.i1_violated);
    assert_eq!(m.texts[0], "pXX");
    let mut m = one("pXq");
    m.edit("b", None, vec![(0, vec![e(1, 2, "")])]);
    m.edit("d", None, vec![(0, vec![e(1, 1, "X")])]);
    let (o, _) = m.undo("b", None, &all(1));
    assert!(matches!(o, Outcome::Conflicts { .. }), "{o:?}");
}

/// v1.8, regression (b) with one participant: p replaces `a` with `b`, then `b` with `c`;
/// undoing both (two undos, or `--steps 2`) applies: the first undo's `b` is a new element
/// with the first change's root, so the second reads it as its base (U8 by root). By
/// element identity (v1.7) the second step saw a foreign `b` and conflicted.
#[test]
fn undo_twice_over_own_replacements_applies() {
    for steps in [1, 2] {
        let mut m = one("a");
        m.edit("p", None, vec![(0, vec![e(0, 1, "b")])]);
        m.edit("p", None, vec![(0, vec![e(0, 1, "c")])]);
        let (o, h) = m.undo("p", None, &all(steps));
        if steps == 1 {
            m.undo("p", None, &all(1));
        } else {
            assert!(h.base_by_root, "{h:?}");
        }
        assert!(matches!(o, Outcome::Applied { .. }), "{o:?}");
        assert_eq!(m.texts[0], "a");
        assert_eq!(roots(&m), roots(&one("a")));
    }
}

// ---------- U3: person grouping ----------

/// A keystroke at `at` by person `p` on connection 0 with token 1 at time `t`.
fn key(m: &mut Model, p: &str, at: usize, s: &str, t: u64) -> GroupNote {
    m.edit_grouped(p, false, 0, 1, t, vec![(0, vec![e(at, at, s)])])
        .1
}

#[test]
fn group_joins_and_undoes_as_one() {
    let mut m = one("x\n");
    assert_eq!(key(&mut m, "p", 1, "a", 0), GroupNote::New(vec![]));
    assert_eq!(key(&mut m, "p", 2, "b", 10), GroupNote::Joined);
    assert_eq!(key(&mut m, "p", 3, "c", 20), GroupNote::Joined);
    assert_eq!(m.depths("p"), (1, 0));
    assert_eq!(m.texts[0], "xabc\n");
    // a revert record per change all the same (U22)
    assert_eq!(m.records.len(), 3);
    let (o, h) = m.undo("p", None, &all(1));
    assert!(matches!(o, Outcome::Applied { steps: 1, .. }), "{o:?}");
    assert!(!h.foreign_deleted);
    assert_eq!(m.texts[0], "x\n");
    m.redo("p", None, &all(1));
    assert_eq!(m.texts[0], "xabc\n");
    assert!(m.i1());
}

#[test]
fn group_closers() {
    // (a) idle: 500 ms or more
    let mut m = one("");
    key(&mut m, "p", 0, "a", 0);
    assert_eq!(key(&mut m, "p", 1, "b", 499), GroupNote::Joined);
    assert_eq!(
        key(&mut m, "p", 2, "c", 999),
        GroupNote::New(vec![Closer::Idle])
    );
    // (b) non-contiguous
    let mut m = one("0123456789");
    key(&mut m, "p", 2, "a", 0);
    assert_eq!(
        key(&mut m, "p", 8, "b", 1),
        GroupNote::New(vec![Closer::NonContiguous])
    );
    // touching on the left is contiguous
    assert_eq!(key(&mut m, "p", 8, "c", 2), GroupNote::Joined);
    // (c) a boundary after a word character opens the next step; the word after it joins
    let mut m = one("");
    key(&mut m, "p", 0, "ab", 0);
    assert_eq!(
        key(&mut m, "p", 2, " ", 1),
        GroupNote::New(vec![Closer::Boundary])
    );
    assert_eq!(key(&mut m, "p", 3, "c", 2), GroupNote::Joined);
    assert_eq!(m.depths("p"), (2, 0));
    // a space after a space is no boundary
    assert_eq!(
        key(&mut m, "p", 4, " ", 3),
        GroupNote::New(vec![Closer::Boundary])
    );
    assert_eq!(key(&mut m, "p", 5, " ", 4), GroupNote::Joined);
    // (d) a newline is a step of its own
    let mut m = one("");
    key(&mut m, "p", 0, " ", 0);
    assert_eq!(
        key(&mut m, "p", 1, "\n", 1),
        GroupNote::New(vec![Closer::Newline])
    );
    assert_eq!(key(&mut m, "p", 2, "a", 2), GroupNote::New(vec![]));
    // (e) another token, another connection, an edit without a token
    let mut m = one("");
    key(&mut m, "p", 0, "a", 0);
    let n = m
        .edit_grouped("p", false, 0, 2, 1, vec![(0, vec![e(1, 1, "b")])])
        .1;
    assert_eq!(n, GroupNote::New(vec![Closer::Token]));
    let n = m
        .edit_grouped("p", false, 1, 2, 2, vec![(0, vec![e(2, 2, "c")])])
        .1;
    assert_eq!(n, GroupNote::New(vec![Closer::Token]));
    m.edit("p", None, vec![(0, vec![e(3, 3, "d")])]);
    let n = m
        .edit_grouped("p", false, 1, 2, 3, vec![(0, vec![e(4, 4, "e")])])
        .1;
    assert_eq!(n, GroupNote::New(vec![]));
    // (f) an undo or redo, even of nothing
    let mut m = one("");
    key(&mut m, "p", 0, "a", 0);
    m.redo("p", None, &all(1));
    assert_eq!(key(&mut m, "p", 1, "b", 1), GroupNote::New(vec![]));
    // (g) someone's change touching the span, not one away from it
    let mut m = one("0123456789");
    key(&mut m, "p", 2, "a", 0);
    m.edit("q", None, vec![(0, vec![e(8, 9, "")])]);
    assert_eq!(key(&mut m, "p", 3, "b", 1), GroupNote::Joined);
    m.edit("q", None, vec![(0, vec![e(4, 4, "Q")])]);
    assert_eq!(
        key(&mut m, "p", 4, "c", 2),
        GroupNote::New(vec![Closer::Foreign])
    );
    // agents never group
    let mut m = one("");
    let n = m
        .edit_grouped("a", true, 0, 1, 0, vec![(0, vec![e(0, 0, "a")])])
        .1;
    assert_eq!(n, GroupNote::Agent);
    let n = m
        .edit_grouped("a", true, 0, 1, 1, vec![(0, vec![e(1, 1, "b")])])
        .1;
    assert_eq!(n, GroupNote::Agent);
    assert_eq!(m.depths("a"), (2, 0));
}

#[test]
fn group_that_undid_itself_leaves_no_step() {
    let mut m = one("x");
    key(&mut m, "p", 1, "ab", 0);
    m.edit_grouped("p", false, 0, 1, 1, vec![(0, vec![e(2, 3, "")])]);
    assert_eq!(m.depths("p"), (1, 0));
    let n = m
        .edit_grouped("p", false, 0, 1, 2, vec![(0, vec![e(1, 2, "")])])
        .1;
    assert_eq!(n, GroupNote::SelfUndone);
    assert_eq!(m.depths("p"), (0, 0));
    // deleting someone's text and typing it again is not nothing: the roots differ
    let mut m = one("xyz");
    m.edit_grouped("p", false, 0, 1, 0, vec![(0, vec![e(1, 2, "")])]);
    let n = m
        .edit_grouped("p", false, 0, 1, 1, vec![(0, vec![e(1, 1, "y")])])
        .1;
    assert_eq!(n, GroupNote::Joined);
    m.undo("p", None, &all(1));
    assert_eq!(m.texts[0], "xyz");
    assert_eq!(
        m.elems[0][1].prov,
        Prov(0, 1),
        "the import's y is back, by root"
    );
}

/// A pinned spec-gap case (v1.10, the D1 amendment: a change is never built from someone
/// else's text): the kit's `tests/undo_foreign_tokens.rs` on the oracle. Today's D1 keeps
/// p16's `9` inside p14's replaced line, and p16's undo turns `p14:3169` into `p14:3167`; under
/// the amendment p14 rewrites the whole token and p16's undo conflicts.
#[test]
fn d1_a_replace_never_keeps_foreign_tokens() {
    let mut m = words("seed\n");
    m.replace("p16", None, 0, "seed\np16:3027\n");
    // p16 changes its own 7 to a 9 (its own text: one character)
    m.replace("p16", None, 0, "seed\np16:3029\n");
    m.replace("p14", None, 0, "seed\np14:3169\n");
    let (o, _) = m.undo("p16", None, &all(1));
    assert_eq!(m.texts[0], "seed\np14:3169\n", "p14's line is untouched");
    assert!(matches!(o, Outcome::Conflicts { step: 1, .. }), "{o:?}");
}
