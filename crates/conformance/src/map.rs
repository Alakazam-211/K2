//! Map rules, over [`Maps`] (E-MAP-1, E-MAP-3; engine answer E5).
//!
//! - `map.lww`: concurrent sets of one key: the higher `(lamport, agent)` wins (the set made
//!   after more of its writer's own changes; at equal lamport the higher agent), and the key's
//!   winning write is that writer's.
//! - `map.delete_vs_set`: a delete and a concurrent set of one key are one register: the
//!   higher `(lamport, agent)` wins, delete included (E5: final), whichever agent is higher.
//! - `map.steady_bounded`: overwriting one key keeps memory O(1): 2,000 more overwrites (two
//!   writers alternating, synced each time) grow what a replica loaded from its checkpoint
//!   holds by less than 16 KiB. Needs the counting allocator ([`crate::heap`]).
//!
//! As in [`crate::mlist`], a listed deviation is asserted to still happen, and
//! [`run_strict`] runs the conforming check regardless (the pinned rule's planted negative).

use crate::caps::{Maps, CELLS};
use crate::grid::*;
use crate::heap;
use crate::{deviates, TextReplica};

/// Every Map rule, by id.
pub const RULES: &[&str] = &["map.lww", "map.delete_vs_set", "map.steady_bounded"];

/// Run rule `id` on engine `S` (its deviation list applies). An unknown id panics.
pub fn run<S: Maps>(id: &str) {
    run_as::<S>(id, deviates::<S::Engine>(id));
}

/// Run rule `id`'s conforming check, ignoring `S`'s deviation list.
pub fn run_strict<S: Maps>(id: &str) {
    run_as::<S>(id, false);
}

fn run_as<S: Maps>(id: &str, dev: bool) {
    match id {
        "map.lww" => lww::<S>(dev),
        "map.delete_vs_set" => delete_vs_set::<S>(dev),
        "map.steady_bounded" => steady_bounded::<S>(dev),
        _ => panic!("no map rule {id:?}"),
    }
}

fn get<S: Maps>(r: &S, key: &str) -> Option<String> {
    r.map(CELLS)
        .get(key)
        .map(|v| v.as_str().unwrap_or("<null>").to_owned())
}

fn assert_maps_agree<S: Maps>(g: &Grid<S>, what: &str) {
    for (i, r) in g.r.iter().enumerate() {
        assert_eq!(
            r.map(CELLS),
            g.r[0].map(CELLS),
            "{}: {what}: replica {i} diverged",
            S::Engine::ENGINE
        );
    }
}

/// `map.lww`
pub fn lww<S: Maps>(dev: bool) {
    let e = S::Engine::ENGINE;
    // equal lamport: the higher agent wins (on every engine)
    let mut g = Grid::<S>::new(&[101, 102], &[cells(vec![mset("k", "0")])]);
    g.edit(0, vec![cells(vec![mset("k", "x")])]);
    g.edit(1, vec![cells(vec![mset("k", "y")])]);
    g.sync_all();
    assert_maps_agree(&g, "equal lamport");
    assert_eq!(
        get(&g.r[0], "k").as_deref(),
        Some("y"),
        "{e}: equal lamport"
    );
    // x (101) sets after three changes of its own: the higher lamport
    let mut g = Grid::<S>::new(&[101, 102, 103], &[cells(vec![mset("k", "0")])]);
    g.bump(0, 3);
    g.edit(0, vec![cells(vec![mset("k", "x")])]);
    g.edit(1, vec![cells(vec![mset("k", "y")])]);
    g.sync_all();
    assert_maps_agree(&g, "x's set is later");
    let (v, w) = (
        get(&g.r[0], "k"),
        g.r[0].map_winner(CELLS, "k").map(|w| S::agent_of(&w)),
    );
    if dev {
        assert_eq!(
            (v.as_deref(), w),
            (Some("y"), Some(102)),
            "{e}: the pinned tie-break changed (y won by agent ID): re-check map.lww"
        );
    } else {
        assert_eq!(
            (v.as_deref(), w),
            (Some("x"), Some(101)),
            "{e}: the higher (lamport, agent) set wins, and it is the key's winner"
        );
    }
}

/// `map.delete_vs_set`: x deletes after three changes of its own (the higher lamport); y
/// sets concurrently.
pub fn delete_vs_set<S: Maps>(dev: bool) {
    let e = S::Engine::ENGINE;
    for (cx, cy) in [(101, 102), (102, 101)] {
        let mut g = Grid::<S>::new(&[cx, cy], &[cells(vec![mset("k", "0")])]);
        g.bump(0, 3);
        g.edit(0, vec![cells(vec![mdel("k")])]);
        g.edit(1, vec![cells(vec![mset("k", "y")])]);
        g.sync_all();
        assert_maps_agree(&g, "delete vs set");
        let v = get(&g.r[0], "k");
        if dev {
            assert_eq!(
                v.as_deref(),
                Some("y"),
                "{e}: agents {cx}/{cy}: the set no longer survives: re-check map.delete_vs_set"
            );
        } else {
            assert_eq!(
                v, None,
                "{e}: agents {cx}/{cy}: the later delete wins over the set"
            );
        }
    }
}

/// `map.steady_bounded`
pub fn steady_bounded<S: Maps>(dev: bool) {
    let e = S::Engine::ENGINE;
    let mut g = Grid::<S>::new(&[101, 102], &[cells(vec![mset("k", "0")])]);
    let overwrite = |g: &mut Grid<S>, range: std::ops::Range<usize>| {
        for i in range {
            let w = i % 2;
            g.edit(
                w,
                vec![cells(vec![mset("k", &format!("{:04}", i % 10_000))])],
            );
            g.sync(w, 1 - w);
        }
    };
    overwrite(&mut g, 0..10);
    let (r, small) = heap::retained(|| g.r[0].reloaded());
    drop(r);
    overwrite(&mut g, 10..2010);
    let (r, big) = heap::retained(|| g.r[0].reloaded());
    let growth = big - small;
    eprintln!("{e}: 2000 more overwrites of one key: a reloaded replica holds {growth} B more");
    assert_eq!(get(&r, "k").as_deref(), Some("2009"), "{e}");
    if dev {
        assert!(
            growth > 2000 * 40,
            "{e}: 2000 more overwrites grew a reloaded replica by {growth} B only: \
             re-check map.steady_bounded"
        );
    } else {
        assert!(
            growth < 16 * 1024,
            "{e}: 2000 more overwrites of one key grew a reloaded replica by {growth} B"
        );
    }
}
