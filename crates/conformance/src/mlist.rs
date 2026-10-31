//! MovableList rules, over [`MLists`] (E-MLIST-1/-2, E-LIST-1; engine answers E1, E8, E9).
//!
//! - `mlist.converge`: random scripts of inserts, moves, block moves, deletes, restores and
//!   sets on three replicas with partial syncs: after a full sync every replica holds the same
//!   list, each key at most once, and every key ever inserted is live or deleted, never lost.
//! - `mlist.move_no_duplicate`: two concurrent moves of one element leave one copy.
//! - `mlist.delete_wins`: a delete beats a concurrent move (and a restore then brings the
//!   element back at the move's place) and a concurrent restore (E1), whichever agent is
//!   higher.
//! - `mlist.insert_beside_moved`: an insert anchored to an element moved concurrently stays at
//!   the element's old spot.
//! - `mlist.move_and_set`: a move and a concurrent set of one element both apply.
//! - `mlist.move_lww`: of two concurrent moves of one element, the higher `(lamport, agent)`
//!   wins (the one made after more of its writer's own changes).
//! - `mlist.block_contiguous`: two concurrent block moves (forward chains, `elem::chain`) into
//!   one gap stay contiguous, each in its order (E9).
//! - `mlist.chain_one_run`: a sort (`elem::reorder`) and a block move (`elem::chain`) leave
//!   their elements' places as one run of the engine's ID index (E8).
//! - `mlist.yata_separating_case`: `text.yata_separating_case` as list inserts between two
//!   bracket elements: FugueMax keeps one writer's run whole.
//!
//! Each rule panics when the engine breaks it, or, for a listed deviation, when the deviation
//! stops happening. [`run_strict`] runs the conforming check even for a listed deviation: the
//! planted negative of a pinned rule (the deviating engine must fail it).

use k2_mpc_base::elem::{chain, reorder, Anchor, ElemState};
use k2_mpc_base::{Edit, Key};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use crate::caps::{MLists, Structured, ROWS};
use crate::grid::*;
use crate::{deviates, TextReplica};

/// Every MovableList rule, by id.
pub const RULES: &[&str] = &[
    "mlist.converge",
    "mlist.move_no_duplicate",
    "mlist.delete_wins",
    "mlist.insert_beside_moved",
    "mlist.move_and_set",
    "mlist.move_lww",
    "mlist.block_contiguous",
    "mlist.chain_one_run",
    "mlist.yata_separating_case",
];

/// Run rule `id` on engine `S` (its deviation list applies). An unknown id panics.
pub fn run<S: MLists>(id: &str) {
    run_as::<S>(id, deviates::<S::Engine>(id));
}

/// Run rule `id`'s conforming check, ignoring `S`'s deviation list.
pub fn run_strict<S: MLists>(id: &str) {
    run_as::<S>(id, false);
}

fn run_as<S: MLists>(id: &str, dev: bool) {
    match id {
        "mlist.converge" => converge::<S>(),
        "mlist.move_no_duplicate" => move_no_duplicate::<S>(),
        "mlist.delete_wins" => delete_wins::<S>(),
        "mlist.insert_beside_moved" => insert_beside_moved::<S>(),
        "mlist.move_and_set" => move_and_set::<S>(),
        "mlist.move_lww" => move_lww::<S>(dev),
        "mlist.block_contiguous" => block_contiguous::<S>(),
        "mlist.chain_one_run" => chain_one_run::<S>(),
        "mlist.yata_separating_case" => yata_separating_case::<S>(dev),
        _ => panic!("no mlist rule {id:?}"),
    }
}

fn engine<S: Structured>() -> &'static str {
    S::Engine::ENGINE
}

/// Every replica holds the same list, each key at most once, and `elem` agrees with it.
fn assert_lists_agree<S: MLists>(g: &Grid<S>, root: &str, what: &str) {
    let e = engine::<S>();
    let first = g.r[0].list(root);
    for (i, r) in g.r.iter().enumerate() {
        let l = r.list(root);
        assert_eq!(l, first, "{e}: {what}: replica {i} diverged");
        let mut keys: Vec<&Key> = l.iter().map(|(k, _)| k).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "{e}: {what}: a key shows twice: {l:?}");
        for (at, (k, _)) in l.iter().enumerate() {
            assert_eq!(
                r.elem(root, k),
                ElemState::Live(at),
                "{e}: {what}: elem({k}) on replica {i}"
            );
        }
    }
}

/// One random change on replica `r`'s list root `root`, minting keys `<prefix><n>`.
pub(crate) fn random_list_ops<S: MLists>(
    rng: &mut SmallRng,
    r: &S,
    root: &str,
    known: &[Key],
    mint: &mut dyn FnMut() -> Key,
) -> Vec<k2_mpc_base::elem::ListOp> {
    let live: Vec<Key> = r.list(root).into_iter().map(|(k, _)| k).collect();
    let anchor = |rng: &mut SmallRng, not: &[Key]| -> Anchor {
        let cands: Vec<&Key> = live.iter().filter(|k| !not.contains(k)).collect();
        let roll = rng.gen_range(0..4);
        if cands.is_empty() || roll == 0 {
            if rng.gen_bool(0.5) {
                Anchor::Start
            } else {
                Anchor::End
            }
        } else if roll < 3 {
            Anchor::After(cands[rng.gen_range(0..cands.len())].clone())
        } else {
            Anchor::Before(cands[rng.gen_range(0..cands.len())].clone())
        }
    };
    let pick = |rng: &mut SmallRng| live[rng.gen_range(0..live.len())].clone();
    let dead: Vec<&Key> = known
        .iter()
        .filter(|k| r.elem(root, k) == ElemState::Deleted)
        .collect();
    use k2_mpc_base::elem::ListOp;
    use k2_mpc_base::Value;
    let roll = rng.gen_range(0..100);
    match roll {
        _ if live.is_empty() || roll < 30 => {
            let key = mint();
            vec![ListOp::Insert {
                key,
                at: anchor(rng, &[]),
                value: Value::str(format!("v{}", rng.gen_range(0..9))),
            }]
        }
        30..=49 => {
            let key = pick(rng);
            let to = anchor(rng, std::slice::from_ref(&key));
            vec![ListOp::Move { key, to }]
        }
        50..=59 => vec![ListOp::Delete { key: pick(rng) }],
        60..=69 if !dead.is_empty() => vec![ListOp::Restore {
            key: dead[rng.gen_range(0..dead.len())].clone(),
        }],
        60..=84 => vec![ListOp::Set {
            key: pick(rng),
            value: Value::str(format!("s{}", rng.gen_range(0..9))),
        }],
        _ => {
            let mut block: Vec<Key> = Vec::new();
            for _ in 0..rng.gen_range(2..4) {
                let x = pick(rng);
                if !block.contains(&x) {
                    block.push(x);
                }
            }
            let to = anchor(rng, &block);
            chain(&block, to)
        }
    }
}

/// `mlist.converge`
pub fn converge<S: MLists>() {
    let e = engine::<S>();
    for seed in 0..40u64 {
        let mut rng = SmallRng::seed_from_u64(seed);
        let mut g = Grid::<S>::new(&[101, 102, 103], &[seed_list(ROWS, "a b c d e f")]);
        let mut known: Vec<Key> = ks("a b c d e f");
        let mut n = 0;
        for _ in 0..40 {
            let i = rng.gen_range(0..3);
            let mut mint = || {
                n += 1;
                k(&format!("n{n}"))
            };
            let ops = random_list_ops(&mut rng, &g.r[i], ROWS, &known, &mut mint);
            for op in &ops {
                if let k2_mpc_base::elem::ListOp::Insert { key, .. } = op {
                    known.push(key.clone());
                }
            }
            g.edit(i, vec![rows(ops)]);
            if rng.gen_bool(0.4) {
                let (a, b) = (rng.gen_range(0..3), rng.gen_range(0..3));
                if a != b {
                    g.sync(a, b);
                }
            }
        }
        g.sync_all();
        let what = format!("script {seed}");
        assert_lists_agree(&g, ROWS, &what);
        for (i, r) in g.r.iter().enumerate() {
            for key in &known {
                assert_ne!(
                    r.elem(ROWS, key),
                    ElemState::Unknown,
                    "{e}: {what}: replica {i} lost {key}"
                );
            }
        }
    }
}

/// `mlist.move_no_duplicate`
pub fn move_no_duplicate<S: MLists>() {
    let e = engine::<S>();
    let mut g = Grid::<S>::new(&[101, 102, 103], &[seed_list(ROWS, "a b c d")]);
    g.edit(0, vec![rows(vec![mv("a", Anchor::End)])]);
    g.edit(1, vec![rows(vec![mv("a", after("b"))])]);
    g.sync_all();
    assert_lists_agree(&g, ROWS, "two moves of a");
    let o = order(&g.r[0], ROWS);
    assert_eq!(o.split(' ').filter(|x| *x == "a").count(), 1, "{e}: {o}");
    assert_eq!(o.split(' ').count(), 4, "{e}: {o}");
}

/// `mlist.delete_wins`
pub fn delete_wins<S: MLists>() {
    let e = engine::<S>();
    // a delete concurrent with a move: deleted; a restore brings it back at the move's place
    let mut g = Grid::<S>::new(&[101, 102, 103], &[seed_list(ROWS, "a b c d")]);
    g.edit(0, vec![rows(vec![mv("a", Anchor::End)])]);
    g.edit(1, vec![rows(vec![del("a")])]);
    g.sync_all();
    assert_lists_agree(&g, ROWS, "delete vs move");
    assert_eq!(order(&g.r[0], ROWS), "b c d", "{e}: delete vs move");
    g.edit(2, vec![rows(vec![res("a")])]);
    g.sync_all();
    assert_eq!(
        order(&g.r[0], ROWS),
        "b c d a",
        "{e}: restored at the move's place"
    );
    // a delete concurrent with a restore: deleted, whichever agent is higher
    for (rx, ry) in [(101, 102), (102, 101)] {
        let seed = [seed_list(ROWS, "a b c"), rows(vec![del("b")])];
        let mut g = Grid::<S>::new(&[rx, ry], &seed);
        g.edit(0, vec![rows(vec![res("b")])]);
        // y never saw x's restore: it restores and deletes again, one change
        g.edit(1, vec![rows(vec![res("b"), del("b")])]);
        g.sync_all();
        assert_lists_agree(&g, ROWS, "delete vs restore");
        assert_eq!(
            order(&g.r[0], ROWS),
            "a c",
            "{e}: delete vs restore, agents {rx}/{ry}"
        );
    }
}

/// `mlist.insert_beside_moved`
pub fn insert_beside_moved<S: MLists>() {
    let e = engine::<S>();
    let mut g = Grid::<S>::new(&[101, 102, 103], &[seed_list(ROWS, "a b c d")]);
    g.edit(0, vec![rows(vec![mv("b", Anchor::End)])]);
    g.edit(1, vec![rows(vec![ins("n", after("b"))])]);
    g.sync_all();
    assert_lists_agree(&g, ROWS, "insert beside a moved element");
    assert_eq!(order(&g.r[0], ROWS), "a n c d b", "{e}: the old spot");
}

/// `mlist.move_and_set`
pub fn move_and_set<S: MLists>() {
    let e = engine::<S>();
    let mut g = Grid::<S>::new(&[101, 102, 103], &[seed_list(ROWS, "a b c")]);
    g.edit(0, vec![rows(vec![mv("a", Anchor::End)])]);
    g.edit(1, vec![rows(vec![set("a", "v")])]);
    g.sync_all();
    assert_lists_agree(&g, ROWS, "move and set");
    assert_eq!(
        g.r[0].list(ROWS),
        vec![
            (k("b"), k2_mpc_base::Value::Null),
            (k("c"), k2_mpc_base::Value::Null),
            (k("a"), k2_mpc_base::Value::str("v")),
        ],
        "{e}: both apply"
    );
}

/// `mlist.move_lww`: x (101) moves `a` after three changes of its own, so its move has the
/// higher lamport; y (102) moves it once.
pub fn move_lww<S: MLists>(dev: bool) {
    let e = engine::<S>();
    let mut g = Grid::<S>::new(&[101, 102, 103], &[seed_list(ROWS, "a b c d")]);
    g.bump(0, 3);
    g.edit(0, vec![rows(vec![mv("a", Anchor::End)])]);
    g.edit(1, vec![rows(vec![mv("a", after("b"))])]);
    g.sync_all();
    assert_lists_agree(&g, ROWS, "two moves");
    let o = order(&g.r[0], ROWS);
    if dev {
        assert_eq!(
            o, "b a c d",
            "{e}: the pinned tie-break changed (y's move won by agent ID): re-check mlist.move_lww"
        );
    } else {
        assert_eq!(o, "b c d a", "{e}: the higher (lamport, agent) move wins");
    }
}

/// `mlist.block_contiguous`
pub fn block_contiguous<S: MLists>() {
    let e = engine::<S>();
    let mut g = Grid::<S>::new(&[101, 102, 103], &[seed_list(ROWS, "a b c d e f g h")]);
    g.edit(0, vec![rows(chain(&ks("a b c"), after("h")))]);
    g.edit(1, vec![rows(chain(&ks("d e f"), after("h")))]);
    g.sync_all();
    assert_lists_agree(&g, ROWS, "two block moves");
    let o = format!(" {} ", order(&g.r[0], ROWS));
    assert!(
        o.contains(" a b c ") && o.contains(" d e f "),
        "{e}: the blocks interleaved: {o}"
    );
    assert!(o.starts_with(" g h "), "{e}: {o}");
}

/// `mlist.chain_one_run`
pub fn chain_one_run<S: MLists>() {
    let e = engine::<S>();
    let keys: Vec<String> = (0..40).map(|i| format!("r{i}")).collect();
    let mut g = Grid::<S>::new(&[101], &[seed_list(ROWS, &keys.join(" "))]);
    let now: Vec<Key> = keys.iter().map(|s| k(s)).collect();
    let target: Vec<Key> = now.iter().rev().cloned().collect();
    g.edit(
        0,
        vec![rows(reorder(&now, &target).expect("a permutation"))],
    );
    let got: Vec<Key> = g.r[0].list(ROWS).into_iter().map(|(k, _)| k).collect();
    assert_eq!(got, target, "{e}: the sort's order");
    assert_eq!(
        g.r[0].place_runs(ROWS, &target),
        Some(1),
        "{e}: a sort as a forward chain is one run"
    );
    let block = ks("r10 r11 r12");
    g.edit(0, vec![rows(chain(&block, after("r30")))]);
    let o = format!(" {} ", order(&g.r[0], ROWS));
    assert!(o.contains(" r30 r10 r11 r12 "), "{e}: the block move: {o}");
    assert_eq!(
        g.r[0].place_runs(ROWS, &block),
        Some(1),
        "{e}: a block move as a forward chain is one run"
    );
}

/// `mlist.yata_separating_case`: the agents and steps of `text.yata_separating_case`, with
/// `lb`/`rb` for its brackets.
pub fn yata_separating_case<S: MLists>(dev: bool) {
    let e = engine::<S>();
    let mut g = Grid::<S>::new(&[102, 103, 101], &[seed_list(ROWS, "lb rb")]);
    let ins_after = |key: &str, a: &str| -> Vec<Edit> { vec![rows(vec![ins(key, after(a))])] };
    g.edit(1, ins_after("e", "lb"));
    g.sync(1, 2);
    g.edit(0, ins_after("a", "lb"));
    g.edit(2, ins_after("p", "lb"));
    g.sync(2, 1);
    assert_eq!(order(&g.r[1], ROWS), "lb p e rb", "{e}");
    g.edit(1, ins_after("f", "p"));
    assert_eq!(order(&g.r[1], ROWS), "lb p f e rb", "{e}");
    g.sync_all();
    assert_lists_agree(&g, ROWS, "the separating script");
    let o = order(&g.r[0], ROWS);
    if dev {
        assert_eq!(
            o, "lb p f a e rb",
            "{e}: the known YATA order changed: re-check mlist.yata_separating_case"
        );
    } else {
        assert_eq!(
            o, "lb a p f e rb",
            "{e}: FugueMax keeps the writer's run `f e` whole"
        );
    }
}
