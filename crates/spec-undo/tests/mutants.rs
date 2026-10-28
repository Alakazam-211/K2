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

#[test]
fn every_mutant_fails_its_rule() {
    let want: [(Mutant, &str, usize); 6] = [
        (Mutant::NoTaint, "U9", 3),
        (Mutant::PartialHidesTaint, "U15", 3),
        (Mutant::RedoRetriesSkipped, "U17", 3),
        (Mutant::O5EveryWindowEnd, "O5", 3),
        (Mutant::O5RegionBase, "O5", 3),
        (Mutant::SkipU19, "U19", 1),
    ];
    mutant::set(Mutant::None);
    let clean = bfs(&tiny(), false, STOP, false, |_, _| {});
    assert!(
        clean.violations.keys().all(|r| !STOP.contains(r)),
        "the oracle: {:?}",
        clean.violations.keys().collect::<Vec<_>>()
    );
    for (m, rule, depth) in want {
        mutant::set(m);
        let c = bfs(&tiny(), false, STOP, true, |_, _| {});
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
