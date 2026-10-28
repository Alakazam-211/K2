//! The spec's worked cases, on the oracle alone.

use super::*;

fn e(s: usize, t: usize, i: &str) -> Edit {
    Edit {
        start: s,
        end: t,
        insert: i.into(),
    }
}

fn one(text: &str) -> Model {
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
    let mut m = Model::new(vec![String::new()], 1, Bounds::default());
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
