//! Planning a single list insert or delete costs what it touches (`engine/structured.rs`, the
//! Yrs path): the engine keeps each list root's slot index between changes and plans one
//! insert or delete against it, instead of reading the whole list.
//!
//! - **What it touches:** on a 10,000-element list, a single insert or delete reads a few
//!   dozen index nodes, not 10,000 slots (the engine visit count, timing-free);
//! - **The same change:** over random op sequences on lists of 10, 1,000 and 10,000 elements
//!   (single inserts at every kind of anchor, deletes of live and deleted elements, refused
//!   ops, and the changes the full read still plans: moves, sets, restores, drops, several
//!   ops at once, a reload), a doc planning by the index and a doc planning by the full read
//!   give the same applied stream, the same update bytes and the same state;
//! - the planted fault "a fast insert after an anchor lands one slot late"
//!   (`ListPlanMutant`) is caught by that row, by name;
//! - the measurement (ignored; run on a build node): the plan's time per single insert and
//!   delete at 1,000 and 10,000 elements, from the kit's profile scopes, and the ratio.
//!
//! Each row is a function returning `Err("<row name>: ...")`. Public API only.

use k2_mpc_base::differential::{
    take_engine_visits, with_list_plan_fast, with_list_plan_mutant, ListPlanMutant,
};
use k2_mpc_base::doc::Doc;
use k2_mpc_base::elem::{Anchor, Applied, Edited, Key, ListOp, Value};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::{DocError, Edit};

const ITEMS: &str = "items";
const SCHEMA: Schema = Schema {
    name: "list-plan",
    roots: &[(ITEMS, Kind::MovableList)],
};
const CLIENT: u64 = 7;

fn k(i: u64) -> Key {
    Key::new(format!("k{i}")).unwrap()
}

fn list(ops: Vec<ListOp>) -> Vec<Edit> {
    vec![Edit::List {
        root: ITEMS.into(),
        ops,
    }]
}

/// A doc holding `n` elements `k0..k{n-1}`, made as one change (an import).
fn doc_of(n: u64) -> Doc {
    let mut d = Doc::new(SCHEMA).unwrap();
    let ops = (0..n)
        .map(|i| ListOp::Insert {
            key: k(i),
            at: Anchor::End,
            value: Value::str(format!("v{i}")),
        })
        .collect();
    d.edit_applied(CLIENT, &list(ops)).unwrap();
    d
}

/// A small deterministic generator.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// A random change: mostly a single insert (every anchor kind) or delete, sometimes an op the
/// full read plans (a move, a set, a restore, a drop, two ops at once), sometimes a refused
/// one (an insert of a key the list holds, a delete of one it never held).
fn random_change(rng: &mut Rng, next: &mut u64, keys: &[Key]) -> (String, Vec<Edit>) {
    let any = |rng: &mut Rng| keys[rng.below(keys.len() as u64) as usize].clone();
    let fresh = |next: &mut u64| {
        *next += 1;
        k(*next)
    };
    let anchor = |rng: &mut Rng| match rng.below(4) {
        0 => Anchor::Start,
        1 => Anchor::End,
        2 => Anchor::After(any(rng)),
        _ => Anchor::Before(any(rng)),
    };
    let r = rng.below(100);
    let (what, ops) = if r < 40 {
        let key = fresh(next);
        let at = anchor(rng);
        (
            format!("insert {key} {at:?}"),
            vec![ListOp::Insert {
                key,
                at,
                value: Value::str("n"),
            }],
        )
    } else if r < 75 {
        let key = any(rng);
        (format!("delete {key}"), vec![ListOp::Delete { key }])
    } else if r < 80 {
        let key = any(rng);
        let to = anchor(rng);
        (format!("move {key} {to:?}"), vec![ListOp::Move { key, to }])
    } else if r < 84 {
        let key = any(rng);
        (
            format!("set {key}"),
            vec![ListOp::Set {
                key,
                value: Value::str("s"),
            }],
        )
    } else if r < 88 {
        let key = any(rng);
        (format!("restore {key}"), vec![ListOp::Restore { key }])
    } else if r < 90 {
        let key = any(rng);
        (format!("drop {key}"), vec![ListOp::Drop { key }])
    } else if r < 94 {
        let (a, b) = (fresh(next), any(rng));
        (
            format!("insert {a} then delete {b}"),
            vec![
                ListOp::Insert {
                    key: a,
                    at: Anchor::End,
                    value: Value::Null,
                },
                ListOp::Delete { key: b },
            ],
        )
    } else if r < 97 {
        let key = any(rng);
        (
            format!("insert {key} again (refused)"),
            vec![ListOp::Insert {
                key,
                at: Anchor::Start,
                value: Value::Null,
            }],
        )
    } else {
        let key = k(1_000_000 + rng.below(10));
        (
            format!("delete never-held {key}"),
            vec![ListOp::Delete { key }],
        )
    };
    (what, list(ops))
}

type Outcome = Result<(Option<Vec<u8>>, Vec<Applied>), DocError>;

fn outcome(r: Result<Edited, DocError>) -> Outcome {
    r.map(|e| (e.update, e.applied))
}

/// Every key the doc holds or held (live and deleted), for picking ops.
fn known(d: &Doc) -> Vec<Key> {
    let s = d.keyed_state().unwrap();
    let mut out = Vec::new();
    for (_, r) in &s.roots {
        if let k2_mpc_base::elem::KeyedRoot::List { live, hidden, .. } = r {
            out.extend(live.iter().map(|(k, _)| k.clone()));
            out.extend(hidden.keys().cloned());
        }
    }
    out
}

/// The fast plan is the full read: two docs, one planning by the index and one by the full
/// read, given the same changes, give the same outcomes, update bytes and states.
fn row_the_fast_plan_is_the_full_read() -> Result<(), String> {
    const ROW: &str = "row_the_fast_plan_is_the_full_read";
    for (n, steps) in [(10u64, 300u32), (1_000, 200), (10_000, 60)] {
        for seed in [1u64, 2] {
            let mut fast = doc_of(n);
            let mut full = with_list_plan_fast(false, || doc_of(n));
            let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (seed * 31 + n));
            let mut next = n;
            let mut keys = known(&fast);
            for step in 1..=steps {
                let (what, edits) = random_change(&mut rng, &mut next, &keys);
                let a = outcome(fast.edit_applied(CLIENT, &edits));
                let b = outcome(with_list_plan_fast(false, || {
                    full.edit_applied(CLIENT, &edits)
                }));
                if a != b {
                    return Err(format!(
                        "{ROW}: n {n}, seed {seed}, step {step} ({what}): the index planned \
                         {a:?}, the full read {b:?}"
                    ));
                }
                if step % 20 == 0 || step == steps {
                    let (sa, sb) = (fast.keyed_state().unwrap(), full.keyed_state().unwrap());
                    if sa != sb {
                        return Err(format!(
                            "{ROW}: n {n}, seed {seed}, after step {step} ({what}): the states \
                             differ"
                        ));
                    }
                    keys = known(&fast);
                }
                if step % 50 == 0 {
                    // a reload: the index is built again from the doc
                    let bytes = fast.encode_state().unwrap();
                    if bytes != full.encode_state().unwrap() {
                        return Err(format!(
                            "{ROW}: n {n}, seed {seed}, after step {step}: the encoded states \
                             differ"
                        ));
                    }
                    fast = Doc::load(SCHEMA, &bytes).unwrap();
                    full = Doc::load(SCHEMA, &bytes).unwrap();
                }
            }
        }
    }
    Ok(())
}

#[test]
fn the_fast_plan_is_the_full_read() {
    row_the_fast_plan_is_the_full_read().unwrap();
}

#[test]
fn plant_fast_insert_after_off_by_one_is_caught() {
    let e = with_list_plan_mutant(
        ListPlanMutant::FastInsertAfterOffByOne,
        row_the_fast_plan_is_the_full_read,
    )
    .expect_err("the row passes with a fast insert after an anchor one slot late");
    assert!(
        e.starts_with("row_the_fast_plan_is_the_full_read: "),
        "caught, but not by the row: {e}"
    );
}

/// On a 10,000-element list, a single insert (at every anchor kind) or delete reads at most a
/// few dozen of the engine's index nodes per change (the full read: 10,000 slots).
#[test]
fn a_single_insert_or_delete_plans_what_it_touches() {
    const N: u64 = 10_000;
    const MAX: u64 = 200;
    let mut d = doc_of(N);
    let mut rng = Rng(42);
    // the first single op builds the index (one read of the list)
    d.edit_applied(CLIENT, &list(vec![ListOp::Delete { key: k(0) }]))
        .unwrap();
    let mut next = N;
    for i in 0..100u64 {
        let ops = if i % 2 == 0 {
            next += 1;
            let at = match i % 8 {
                0 => Anchor::Start,
                2 => Anchor::End,
                4 => Anchor::After(k(1 + rng.below(N - 1))),
                _ => Anchor::Before(k(1 + rng.below(N - 1))),
            };
            vec![ListOp::Insert {
                key: k(next),
                at,
                value: Value::Null,
            }]
        } else {
            vec![ListOp::Delete {
                key: k(1 + rng.below(N - 1)),
            }]
        };
        take_engine_visits();
        d.edit_applied(CLIENT, &list(ops.clone())).unwrap();
        let v = take_engine_visits();
        assert!(
            v <= MAX,
            "op {i} ({ops:?}) read {v} engine entries on a list of {N} (at most {MAX})"
        );
    }
}
