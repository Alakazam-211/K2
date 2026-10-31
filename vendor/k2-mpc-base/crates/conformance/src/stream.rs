//! The applied stream of merged updates, over [`Merged`] with [`MLists`] and [`Maps`] (CSV
//! seam design §4.2, engine answer E4): what another replica's changes did here, for
//! subscribers, presence, the `table.*` reports and undo's conflict checks.
//!
//! - `stream.merged_replays`: random scripts on three replicas (row and column moves, inserts,
//!   deletes, restores, sets, block moves, cell sets and deletes) with partial syncs: the
//!   stream of every merge, replayed over the receiver's lists and map before it, gives them
//!   after it.
//! - `stream.merged_matches_local`: a change delivered alone reports, on the receiver, what
//!   it reported on its author: the same ops (kind, key, live indices, values, effect), for
//!   each op kind (insert, move, delete, restore, shown and hidden set, map set and delete).
//! - `stream.merged_writes`: and each op carries the write its author's stream named.

use std::fmt::Debug;

use k2_mpc_base::elem::{Anchor, Applied, MapOp};
use k2_mpc_base::Edit;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use crate::caps::{MLists, Maps, Merged, COLS, ROWS};
use crate::grid::*;
use crate::{deviates, TextReplica};

/// Every merged-stream rule, by id.
pub const RULES: &[&str] = &[
    "stream.merged_replays",
    "stream.merged_matches_local",
    "stream.merged_writes",
];

/// Run rule `id` on engine `S` (its deviation list applies). An unknown id panics.
pub fn run<S: Merged + MLists + Maps>(id: &str) {
    run_as::<S>(id, deviates::<S::Engine>(id));
}

/// Run rule `id`'s conforming check, ignoring `S`'s deviation list.
pub fn run_strict<S: Merged + MLists + Maps>(id: &str) {
    run_as::<S>(id, false);
}

fn run_as<S: Merged + MLists + Maps>(id: &str, dev: bool) {
    match id {
        "stream.merged_replays" => merged_replays::<S>(),
        "stream.merged_matches_local" => merged_matches_local::<S>(),
        "stream.merged_writes" => merged_writes::<S>(dev),
        _ => panic!("no stream rule {id:?}"),
    }
}

/// `stream.merged_replays`
pub fn merged_replays<S: Merged + MLists + Maps>() {
    let e = S::Engine::ENGINE;
    for seed in 0..30u64 {
        let mut rng = SmallRng::seed_from_u64(seed);
        let mut g = Grid::<S>::new(&[101, 102, 103], &seed_table("r1 r2 r3 r4", "c1 c2"));
        let mut known = ks("r1 r2 r3 r4 c1 c2");
        let mut n = 0;
        for step in 0..40 {
            let i = rng.gen_range(0..3);
            let edits: Vec<Edit> = match rng.gen_range(0..10) {
                0..=4 => {
                    let mut mint = || {
                        n += 1;
                        k(&format!("r.{n}"))
                    };
                    let ops =
                        crate::mlist::random_list_ops(&mut rng, &g.r[i], ROWS, &known, &mut mint);
                    vec![rows(ops)]
                }
                5..=6 => {
                    let mut mint = || {
                        n += 1;
                        k(&format!("c.{n}"))
                    };
                    let ops =
                        crate::mlist::random_list_ops(&mut rng, &g.r[i], COLS, &known, &mut mint);
                    vec![cols(ops)]
                }
                _ => {
                    let r = format!("r{}", rng.gen_range(1..6));
                    let c = format!("c{}", rng.gen_range(1..3));
                    let op = if rng.gen_bool(0.2) {
                        MapOp::Delete { key: cell(&r, &c) }
                    } else {
                        mset(&cell(&r, &c), &format!("v{step}"))
                    };
                    vec![cells(vec![op])]
                }
            };
            for ed in &edits {
                if let Edit::List { ops, .. } = ed {
                    for op in ops {
                        if let k2_mpc_base::elem::ListOp::Insert { key, .. } = op {
                            known.push(key.clone());
                        }
                    }
                }
            }
            g.edit(i, edits);
            if rng.gen_bool(0.5) {
                let (a, b) = (rng.gen_range(0..3), rng.gen_range(0..3));
                if a != b {
                    merge_and_replay(&mut g, a, b, &format!("script {seed}, step {step}"));
                }
            }
        }
        for a in 0..3 {
            for b in 0..3 {
                if a != b {
                    merge_and_replay(&mut g, a, b, &format!("script {seed}, full sync"));
                }
            }
        }
        g.sync_all();
        assert_converged(&g, &format!("{e}: script {seed}"));
    }
}

/// `to` merges everything `from` has; the merge's stream replays over `to`'s state before.
fn merge_and_replay<S: Merged + MLists + Maps>(
    g: &mut Grid<S>,
    from: usize,
    to: usize,
    what: &str,
) {
    let e = S::Engine::ENGINE;
    let mut plain = Plain::of(&g.r[to]);
    let (a, b) = pair(&mut g.r, from, to);
    let stream = b.sync_applied(a);
    if let Err(why) = plain.replay(&stream) {
        panic!("{e}: {what}: the merge's stream doesn't replay: {why}\n{stream:?}");
    }
    assert_eq!(
        plain,
        Plain::of(&g.r[to]),
        "{e}: {what}: replaying the merge's stream doesn't give the merged state\n{stream:?}"
    );
}

/// One change per op kind, each on a fresh pair of replicas: (what, edits, whether every op's
/// write can be named by an engine that keeps no identity for a removal).
fn cases() -> Vec<(&'static str, Vec<Edit>, bool)> {
    vec![
        (
            "insert",
            vec![rows(vec![ins_v("n", after("c"), "nv")])],
            true,
        ),
        ("move", vec![rows(vec![mv("a", after("d"))])], true),
        (
            "move to the start",
            vec![rows(vec![mv("e", Anchor::Start)])],
            true,
        ),
        ("delete", vec![rows(vec![del("c")])], true),
        ("restore", vec![rows(vec![res("b")])], false),
        ("set", vec![rows(vec![set("d", "v")])], true),
        (
            "set of a deleted element",
            vec![rows(vec![set("b", "h")])],
            true,
        ),
        ("map set", vec![cells(vec![mset("k", "1")])], true),
        ("map insert", vec![cells(vec![mset("n", "1")])], true),
        ("map delete", vec![cells(vec![mdel("k")])], false),
    ]
}

/// The seed of [`cases`]: `a b c d e` with `b` deleted, and `k = 0`.
fn case_seed() -> Vec<Edit> {
    vec![
        seed_list(ROWS, "a b c d e"),
        rows(vec![del("b")]),
        cells(vec![mset("k", "0")]),
    ]
}

/// x makes the case's change; y receives it alone. (x's stream, y's stream.)
#[allow(clippy::type_complexity)]
fn local_and_merged<S: Merged + MLists + Maps>(
    edits: &[Edit],
) -> (Vec<Applied<S::Write>>, Vec<Applied<Option<S::Write>>>) {
    let mut g = Grid::<S>::new(&[101, 102], &case_seed());
    let local = g.edit(0, edits.to_vec());
    let changes = g.r[0].own_changes();
    assert_eq!(changes.len(), 1, "{}: one change", S::Engine::ENGINE);
    let merged = g.r[1]
        .deliver(&changes[0])
        .unwrap_or_else(|r| panic!("{}: refused: {r:?}", S::Engine::ENGINE));
    (local, merged)
}

fn without_writes<W>(a: Vec<Applied<W>>) -> Vec<Applied<()>> {
    a.into_iter().map(|x| x.map_write(|_| ())).collect()
}

/// `stream.merged_matches_local`
pub fn merged_matches_local<S: Merged + MLists + Maps>() {
    let e = S::Engine::ENGINE;
    for (what, edits, _) in cases() {
        let (local, merged) = local_and_merged::<S>(&edits);
        assert_eq!(
            without_writes(merged),
            without_writes(local),
            "{e}: {what}: the merged stream differs from the author's"
        );
    }
}

fn writes<W: Clone + Debug>(a: &[Applied<W>]) -> Vec<W> {
    let mut out = Vec::new();
    for x in a {
        match x {
            Applied::List { ops, .. } => out.extend(ops.iter().map(|o| o.write.clone())),
            Applied::Map { ops, .. } => out.extend(ops.iter().map(|o| o.write.clone())),
            _ => {}
        }
    }
    out
}

/// `stream.merged_writes`
pub fn merged_writes<S: Merged + MLists + Maps>(dev: bool) {
    let e = S::Engine::ENGINE;
    for (what, edits, named) in cases() {
        let (local, merged) = local_and_merged::<S>(&edits);
        let local = writes(&local);
        let merged = writes(&merged);
        let all: Vec<Option<S::Write>> = local.iter().cloned().map(Some).collect();
        if dev && !named {
            assert_eq!(
                merged,
                vec![None; local.len()],
                "{e}: {what}: the merged op now names its write: re-check stream.merged_writes"
            );
        } else {
            assert_eq!(merged, all, "{e}: {what}: the merged ops' writes");
        }
    }
}
