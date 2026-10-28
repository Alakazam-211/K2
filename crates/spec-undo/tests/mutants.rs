//! The model check's mutants (spec §8 Mutants; `model::mutant`) at a tiny bound: each must
//! violate the rule it breaks, at the depth given, on the enumerator; the oracle of record
//! violates none of the rules the runs stop at. The same set fails TLC on `undo.qnt`
//! (`tools/undo/counts.sh`, `PART=m`). Needs `--features mutants`; one test, so one mutant is
//! active at a time in this process.

use spec_undo::bounded::{bfs, Bound, STOP};
use spec_undo::model::mutant::{self, Mutant};
use spec_undo::model::{Bounds, Edit, Mode, Model, Outcome, Request};

fn tiny() -> Bound {
    Bound {
        pids: 2,
        events: 1,
        undos: 1,
        reverts: 0,
        maxlen: 4,
        // three letters, so a two-region edit exists from the start (mutant (iv))
        init: "abb".into(),
        ..Bound::small()
    }
}

/// The restart mutant's bound: one pid, three events, a revert and a restart, from `abba`
/// (its least counterexample: a two-region edit, deletes making the regions touch with one
/// tainted at its start only, then the restart).
fn restart_bound() -> Bound {
    Bound {
        pids: 1,
        events: 3,
        undos: 0,
        reverts: 1,
        restarts: 1,
        maxlen: 4,
        init: "abba".into(),
        ..Bound::small()
    }
}

/// Mutant (x)'s bound: one pid, one event, one undo and one revert, from `ab` (its least
/// counterexample: p1 deletes `b` at the doc's end, undoes, and reverts the delete; by
/// element identity the revert's `b` and the undo's are different insertions at the doc's
/// end, so O5 lands both: two live `b` with one root).
fn provenance_bound() -> Bound {
    Bound {
        pids: 1,
        events: 1,
        undos: 1,
        reverts: 1,
        maxlen: 4,
        init: "ab".into(),
        ..Bound::small()
    }
}

/// Mutant (xi)'s bound: two pids, one event each, one undo and one revert, from `ab` (its
/// least counterexample, at the doc's end: p1 deletes `b`, undoes it, p2 types `a` before
/// the restored `b`, and p1 reverts its delete; theirs inserts `a` and the restored `b`,
/// ours `b`, not identical by roots (O4), so O5 lands both: two live `b` with one root).
fn no_o4a_bound() -> Bound {
    Bound {
        pids: 2,
        events: 1,
        undos: 1,
        reverts: 1,
        maxlen: 4,
        init: "ab".into(),
        ..Bound::small()
    }
}

#[test]
fn every_mutant_fails_its_rule() {
    let want: [(Mutant, &str, usize, Bound); 10] = [
        (Mutant::NoTaint, "U9", 3, tiny()),
        (Mutant::PartialHidesTaint, "U15", 3, tiny()),
        (Mutant::RedoRetriesSkipped, "U17", 3, tiny()),
        (Mutant::O5EveryWindowEnd, "O5", 3, tiny()),
        (Mutant::O5RegionBase, "O5", 3, tiny()),
        (Mutant::SkipU19, "U19", 1, tiny()),
        // F3: the merge's sides found by diffing strings
        (Mutant::Rediff, "own_only", 3, tiny()),
        // v0.8.1's persisted carry: one taint flag per region
        (Mutant::RestartSingleTaint, "RT", 4, restart_bound()),
        // v1.8: O4 on element identity, no O4a
        (Mutant::ProvenanceIgnored, "I1", 3, provenance_bound()),
        // v1.9: O4 on roots, no O4a
        (Mutant::NoO4a, "I1", 4, no_o4a_bound()),
    ];
    mutant::set(Mutant::None);
    for b in [tiny(), restart_bound(), provenance_bound(), no_o4a_bound()] {
        let clean = bfs(&b, false, STOP, false, |_, _| {});
        assert!(
            clean.violations.is_empty(),
            "the oracle: {:?}",
            clean.violations.keys().collect::<Vec<_>>()
        );
    }
    for (m, rule, depth, b) in want {
        mutant::set(m);
        let c = bfs(&b, false, STOP, true, |_, _| {});
        let got: Vec<_> = c
            .violations
            .iter()
            .filter(|(r, _)| STOP.contains(r))
            .map(|(r, (d, _))| (*r, *d))
            .collect();
        eprintln!("{}: {got:?}", m.name());
        assert!(
            got.contains(&(rule, depth)),
            "{}: wanted {rule} at depth {depth}, got {got:?}",
            m.name()
        );
    }
    mutant::set(Mutant::None);
    no_o4a_fails_i1_in_mid_text();
}

fn e(start: usize, end: usize, insert: &str) -> Edit {
    Edit {
        start,
        end,
        insert: insert.into(),
    }
}

/// Mutant (xi) on O4a's mid-text witness (the oracle's `o4a_mid_text`; depth 5, two undos):
/// p1 inserts `a` at 1; p2 deletes the seed's `a` and replaces `b` (two regions); p1 undoes;
/// p1 reverts p2's change, which puts the seed's `a` back outside the window of p2's first
/// region. p2's `undo --partial` then puts the seed's `a` root back a second time: the oracle
/// refuses it (O4a, blocked); without O4a it applies and I1 fails. Called from the one test
/// (one mutant active at a time in this process).
fn no_o4a_fails_i1_in_mid_text() {
    let run = |m: Mutant| {
        mutant::set(m);
        let mut o = Model::new(vec!["ab".into()], 1, Bounds::default());
        o.edit("p1", None, vec![(0, vec![e(1, 1, "a")])]); // "aab"
        o.edit("p2", None, vec![(0, vec![e(0, 1, ""), e(2, 3, "a")])]); // change 3: "aa"
        o.undo("p1", None, &Request::new(1, Mode::All)); // "a"
        let (r, _) = o.revert("p1", None, 3, false); // "ab", the seed's roots back
        assert!(matches!(r, Outcome::Applied { .. }), "{}: {r:?}", m.name());
        let (u, h) = o.undo("p2", None, &Request::new(1, Mode::Partial));
        mutant::set(Mutant::None);
        (u, h.o4a, h.i1_violated, o.i1(), o.texts[0].clone())
    };
    let (u, o4a, viol, i1, text) = run(Mutant::None);
    assert!(
        matches!(u, Outcome::Blocked { .. }) && o4a && !viol && i1,
        "{u:?}"
    );
    assert_eq!(text, "ab");
    let (u, o4a, viol, i1, _) = run(Mutant::NoO4a);
    assert!(matches!(u, Outcome::Applied { .. }) && !o4a, "{u:?}");
    assert!(viol && !i1, "no O4a: two live elements with one root");
}
