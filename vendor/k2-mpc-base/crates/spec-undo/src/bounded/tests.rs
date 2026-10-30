//! The bounded world's reductions, machine-checked at small bounds, and the oracle of record
//! clean at them.

use std::collections::{BTreeMap, BTreeSet};

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

/// The oracle of record at a tiny bound with a revert: it violates no rule, `own_only` and
/// U12 (restated on exact hunk sets, v1.7) included. At v0.8.2 both were violated at depth
/// 3 (findings F3 and F4, `docs/MODEL-CHECK.md`):
/// - **F3, `own_only`:** seed `ab`; p1 replaces `a` with `b`; p2 inserts `b` at 0, pulled
///   into p1's region at its start; p1's undo deleted p2's `b` (the merge's base→theirs
///   diff put p2's insertion after the region's own `b`). Exact sets: p2's `b` stays.
/// - **F4, U12's old note:** whole-text merge3 of re-diffed sides differs from the window
///   form in repeated text; on exact sets the two forms agree.
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
    assert!(c.violations.is_empty(), "{:?}", c.violations);
    assert!(c.distinct() > 100);
}

/// v1.8: the case class "concurrent restores of one root" is reached (O4 on roots, O4a, a
/// window character counted as base by root only), and I1 holds, at a tiny bound with two
/// undos and a revert.
#[test]
fn concurrent_restores_are_reached() {
    let b = Bound {
        pids: 2,
        events: 1,
        undos: 2,
        reverts: 1,
        maxlen: 3,
        init: "ab".into(),
        ..Bound::small()
    };
    let mut n = BTreeMap::new();
    let c = bfs(&b, false, STOP, false, |s, _| {
        for h in &s.hit {
            *n.entry(*h).or_insert(0usize) += 1;
        }
    });
    assert!(c.violations.is_empty(), "{:?}", c.violations);
    // the class and each of its parts (v0.9.2)
    for class in [
        "concurrent_restore",
        "concurrent_restore_o4",
        "concurrent_restore_o4a",
        "concurrent_restore_base_by_root",
    ] {
        assert!(
            n.get(class).copied().unwrap_or(0) > 0,
            "{class} never reached: {n:?}"
        );
    }
}

/// A restart changes no revert (RT, U22a v1.6) on the oracle of record, and the case
/// classes the walks count are reached.
#[test]
fn restarts_are_transparent() {
    let b = Bound {
        pids: 1,
        events: 2,
        undos: 1,
        reverts: 1,
        restarts: 1,
        maxlen: 4,
        init: "abba".into(),
        ..Bound::small()
    };
    let mut hit = BTreeSet::new();
    let c = bfs(&b, false, STOP, false, |s, _| {
        hit.extend(s.hit.iter().copied());
    });
    assert!(c.violations.is_empty(), "{:?}", c.violations);
    for class in ["restart", "taint", "edge_insert", "exact_vs_rediff"] {
        assert!(hit.contains(class), "class {class} not reached: {hit:?}");
    }
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
        Bound {
            restarts: 1,
            ..tiny()
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

/// The view is exact (v1.8): roots as labels (states equal up to renaming roots), the
/// carried theirs and a reduced head left out. Two oracle states found with one view (in
/// different oracle states: other root names, another carry, another head) have the same
/// actions, and each action gives the same outcome and the same next view.
#[test]
fn the_view_is_exact() {
    let b = Bound { undos: 2, ..tiny() };
    let mut first: std::collections::HashMap<View, State> = std::collections::HashMap::new();
    let mut fr = vec![State::init(&b)];
    let (mut checked, mut renamed) = (0, 0);
    while !fr.is_empty() {
        let mut next = Vec::new();
        for s in &fr {
            for a in s.actions(&b) {
                let Some((t, _)) = s.step(&b, &a) else {
                    continue;
                };
                let v = t.view(&identity(b.pids));
                match first.get(&v) {
                    None => {
                        first.insert(v, t.clone());
                        next.push(t);
                    }
                    Some(u) if u.m != t.m && checked < 4000 => {
                        checked += 1;
                        renamed += usize::from(u.m.elems != t.m.elems);
                        let succ = |x: &State| -> Vec<_> {
                            x.actions(&b)
                                .into_iter()
                                .map(|a| {
                                    let r = x.step(&b, &a).map(|(y, o)| {
                                        (crate::itf::normalise(&o), y.view(&identity(b.pids)))
                                    });
                                    (a, r)
                                })
                                .collect()
                        };
                        assert_eq!(succ(u), succ(&t), "one view, two futures: {v:?}");
                    }
                    Some(_) => {}
                }
            }
        }
        fr = next;
    }
    eprintln!("view exactness: {checked} pairs checked, {renamed} with other root names");
    assert!(renamed > 0, "no state reached twice under other root names");
}
