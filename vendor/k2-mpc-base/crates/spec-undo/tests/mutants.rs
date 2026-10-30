//! The model check's mutants (spec §8 Mutants; `model::mutant`) at a tiny bound: each must
//! violate the rule it breaks, at the depth given, on the enumerator; the oracle of record
//! violates none of the rules the runs stop at. The same set fails TLC on `undo.qnt`
//! (`tools/quint-undo/mutants.sh`). Needs `--features mutants`; one test, so one mutant is
//! active at a time in this process.

use spec_undo::bounded::{bfs, Bound, STOP};
use spec_undo::model::mutant::{self, Mutant};

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

#[test]
fn every_mutant_fails_its_rule() {
    let want: [(Mutant, &str, usize, Bound); 9] = [
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
    ];
    mutant::set(Mutant::None);
    for b in [tiny(), restart_bound(), provenance_bound()] {
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
}
