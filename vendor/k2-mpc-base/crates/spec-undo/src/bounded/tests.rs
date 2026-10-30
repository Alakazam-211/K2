//! The bounded world's reductions, machine-checked at small bounds, and the oracle of record
//! clean at them.

use std::collections::BTreeSet;

use super::*;

fn tiny() -> Bound {
    Bound {
        pids: 2,
        events: 1,
        undos: 1,
        reverts: 1,
        maxlen: 3,
        init: "ab".into(),
        ..Bound::small()
    }
}

/// The oracle of record at a tiny bound with a revert: it violates no rule the runs stop at,
/// and two that are observed, each first at depth 3 (findings, `docs/MODEL-CHECK.md`):
/// - **F3, `own_only`:** seed `ab`; p1 replaces `a` with `b`; p2 inserts `b` at 0, pulled
///   into p1's region at its start; p1's undo deletes p2's `b` (the merge's base→theirs
///   diff puts p2's insertion after the region's own `b`).
/// - **F4, U12's equivalence note:** seed `ab`; p1 inserts `a` at 0; p2 replaces it with
///   `b`; p1's undo conflicts (the window form, the definition), while whole-text merge3 of
///   the same inverse is clean (its diff deletes the seed's `a` instead).
#[test]
fn the_oracle_at_a_tiny_bound() {
    let b = Bound {
        u12: true,
        ..tiny()
    };
    let c = bfs(&b, false, STOP, false, |_, _| {});
    eprintln!(
        "tiny: {:?} distinct {} transitions {} cut {}",
        c.per_depth,
        c.distinct(),
        c.transitions,
        c.cut
    );
    assert_eq!(
        c.violations.keys().collect::<Vec<_>>(),
        vec![&"U12", &"own_only"]
    );
    for (rule, (depth, path)) in &c.violations {
        assert_eq!(*depth, 3, "{rule}: {path:?}");
    }
    assert!(c.distinct() > 100);
}

/// Pid symmetry is exact: the canonical forms of every state of the plain enumeration are
/// exactly the states of the symmetric one, and both find the same violations (none).
#[test]
fn pid_symmetry_is_exact() {
    for b in [
        tiny(),
        Bound {
            pids: 3,
            events: 1,
            undos: 1,
            reverts: 0,
            maxlen: 3,
            ..Bound::small()
        },
    ] {
        let mut canon = BTreeSet::new();
        let plain = bfs(&b, false, STOP, false, |s, _| {
            canon.insert(s.canonical(true));
        });
        let mut sym_states = BTreeSet::new();
        let sym = bfs(&b, true, STOP, false, |_, v| {
            sym_states.insert(v.clone());
        });
        assert_eq!(canon, sym_states, "{}", b.describe());
        assert_eq!(sym.distinct() as usize, sym_states.len());
        assert!(plain.distinct() > sym.distinct());
        assert_eq!(
            plain.violations.keys().collect::<Vec<_>>(),
            sym.violations.keys().collect::<Vec<_>>()
        );
    }
}

/// Dropping the records once the revert budget is spent loses nothing: with and without
/// it, the states minus records and head are the same set, with the same violations.
#[test]
fn dropping_spent_records_is_exact() {
    let strip = |v: &View| View {
        records: Vec::new(),
        head: 0,
        ..v.clone()
    };
    let b = tiny();
    let mut with = BTreeSet::new();
    let a = bfs(&b, false, STOP, false, |_, v| {
        with.insert(strip(v));
    });
    let mut without = BTreeSet::new();
    let c = bfs(
        &Bound {
            reduce: false,
            ..b.clone()
        },
        false,
        STOP,
        false,
        |_, v| {
            without.insert(strip(v));
        },
    );
    assert_eq!(with, without);
    assert!(c.distinct() >= a.distinct());
    assert_eq!(a.violations.len(), c.violations.len());
}

/// The cut keeps every string the merge diffs within `maxlen`: the Quint model's diff table
/// covers them.
#[test]
fn the_cut_bounds_every_diffed_string() {
    let b = tiny();
    bfs(&b, false, STOP, false, |s, _| {
        assert!(s.text().len() <= b.maxlen);
        for st in s.m.stacks.values() {
            for r in st.undo.iter().chain(&st.redo).flat_map(|x| &x.regions) {
                assert!(r.before.len() <= b.maxlen && r.after.len() <= b.maxlen);
            }
        }
        for rec in s.m.records.values() {
            for r in &rec.regions {
                assert!(r.before.len() <= b.maxlen && r.after.len() <= b.maxlen);
            }
        }
    });
}
