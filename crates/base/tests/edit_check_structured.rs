//! The v0.9.3 edit path for **structured** edits (C1e rebase): a grid change is made on the doc
//! itself as the client (`transact_mut_as`) and its structured part is emitted by whichever
//! path makes it, so the canary and the differential shadow see the same bytes as for text.
//!
//! - **Differential:** random grid scripts (cell sets and clears, row and column inserts,
//!   moves, deletes, restores, renames, sorts, header switches, drop-deleted) through the
//!   production path, each edit shadowed by the throwaway path on a twin: any difference in
//!   the update, the state, the state vector or the texts panics.
//! - **Canary:** the same scripts with every edit sampled (`set_edit_check_every(1)`): the two
//!   paths agree, so nothing is found and the doc never falls back.
//! - **Scalar-only guard (until k2-yrs k2.4):** every value a structured edit puts on the wire
//!   is one scalar string, so no grid op reaches the Yrs paths whose encoded bytes follow a
//!   hash map's order (multi-key prelims, `Any::Map`, attributes, embeds, subdocs). The two
//!   checks above compare bytes and rely on it.

use k2_mpc_base::differential::{with_side, Path, Side};
use k2_mpc_base::elem::Anchor;
use k2_mpc_base::table::{self, Field, Record, Shape, SortKey, Table};
use k2_mpc_base::{Doc, Edit, Key, Schema};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

const S: Schema = Schema {
    name: "grid",
    roots: table::ROOTS,
};

/// One random grid op's edits against `t` (applied to it), or none.
fn random_op(t: &mut Table, rng: &mut SmallRng) -> Option<Vec<Edit>> {
    let rows = t.live_rows();
    let cols = t.live_cols();
    let r = |i: usize| rows.get(i % rows.len().max(1)).cloned();
    let c = |i: usize| cols.get(i % cols.len().max(1)).cloned();
    let (i, j) = (rng.gen_range(0..9), rng.gen_range(0..9));
    let v = ["a", "b", "", "xyz"][rng.gen_range(0..4)];
    let e = match rng.gen_range(0..13) {
        0..=3 => t.set_cell(&r(i)?, &c(j)?, v),
        4 => t.clear_cell(&r(i)?, &c(j)?),
        5 => t
            .insert_row(r(i).map_or(Anchor::End, Anchor::After), &[v.into()])
            .map(|x| x.1),
        6 => t.move_row(&r(i)?, Anchor::Start),
        7 => t.delete_row(&r(i)?),
        8 => {
            let d: Vec<Key> = t.rows.deleted().cloned().collect();
            t.restore_row(d.get(i % d.len().max(1))?)
        }
        9 => t
            .insert_col(c(j).map_or(Anchor::End, Anchor::After), None)
            .map(|x| x.1),
        10 => t.sort(&[SortKey::by(c(j)?)]),
        11 => t.set_header(rng.gen_bool(0.5)),
        _ => t.drop_deleted(),
    };
    e.ok().filter(|e| !e.is_empty())
}

/// A grid doc and its table from a small file.
fn grid(seed: u64) -> (Doc, Table) {
    let recs: Vec<Record> = (0..4)
        .map(|r| {
            (0..3)
                .map(|c| Field::new(format!("v{seed}{r}{c}")))
                .collect()
        })
        .collect();
    let (t, edits) = Table::import(&recs, false, Shape::Rect).unwrap();
    let mut d = Doc::new(S).unwrap();
    d.edit_as(7, &edits).unwrap();
    (d, t)
}

#[test]
fn grid_edits_on_the_doc_equal_the_throwaway_path() {
    let mut shadow_checks = 0;
    for seed in 0..40 {
        let mut rng = SmallRng::seed_from_u64(seed);
        let (mut d, mut t) = grid(seed);
        let mut side = Side::new(Path::MainDoc).shadowed_by(Path::Throwaway);
        for _ in 0..30 {
            let Some(e) = random_op(&mut t, &mut rng) else {
                continue;
            };
            let client = 100 + rng.gen_range(0..3);
            with_side(&mut side, || d.edit_applied(client, &e)).unwrap();
        }
        assert_eq!(Table::from_keyed(&d.keyed_state().unwrap()).unwrap(), t);
        shadow_checks += side.shadow_checks();
    }
    assert!(shadow_checks > 500, "{shadow_checks} shadowed edits");
}

#[test]
fn the_canary_checks_grid_edits_and_finds_nothing() {
    for seed in 0..20 {
        let mut rng = SmallRng::seed_from_u64(1000 + seed);
        let (mut d, mut t) = grid(seed);
        d.set_edit_check_every(1);
        for _ in 0..30 {
            if let Some(e) = random_op(&mut t, &mut rng) {
                d.edit_applied(100 + rng.gen_range(0..3), &e).unwrap();
            }
        }
        assert!(!d.edit_path_fallback(), "seed {seed}: the canary fell back");
        assert_eq!(Table::from_keyed(&d.keyed_state().unwrap()).unwrap(), t);
    }
}

/// The scalar-only guard: each grid edit's update, applied to a bare Yrs doc that observes
/// every root the grid schema declares, inserts nothing but scalar strings (no map or array
/// prelims, `Any::Map`/`Any::Array`, embeds, attributes or subdocs) and creates no other root.
/// Keep it until k2-yrs k2.4 (deterministic encode order for multi-key maps, attributes and
/// subdoc options) is vendored: before that, a multi-key value would make the byte-for-byte
/// differential flaky and the canary raise false mismatches.
#[test]
fn grid_edits_put_only_scalar_strings_on_the_wire() {
    use std::sync::{Arc, Mutex};
    use yrs::types::{Change, EntryChange};
    use yrs::updates::decoder::Decode;
    use yrs::{Any, Observable, Out, Transact, Update};

    const WAIT: &str = "a structured edit put a non-scalar value on the wire: its encoded bytes \
         can follow a hash map's order until k2-yrs k2.4 (deterministic encode order) is \
         vendored; keep grid values one scalar string per key until then";
    fn scalar(o: &Out) -> bool {
        matches!(o, Out::Any(Any::String(_)))
    }
    let bad: Arc<Mutex<Vec<String>>> = Arc::default();
    // values the observers saw: the guard is live, not vacuous
    let seen: Arc<std::sync::atomic::AtomicUsize> = Arc::default();
    let mut applied = 0;
    for seed in 0..40 {
        // a bare Yrs doc observing every root the grid schema declares
        let mirror = yrs::Doc::new();
        let mut names: Vec<String> = Vec::new();
        for (root, kind) in S.roots {
            let maps: Vec<String> = match kind {
                k2_mpc_base::schema::Kind::MovableList => {
                    let slots = mirror.get_or_insert_array(format!("{root}/slots"));
                    names.push(format!("{root}/slots"));
                    let (b, r, n) = (bad.clone(), root.to_string(), seen.clone());
                    slots.observe("guard", move |txn, e: &yrs::types::array::ArrayEvent| {
                        for c in e.delta(txn) {
                            if let Change::Added(v) = c {
                                n.fetch_add(v.len(), std::sync::atomic::Ordering::Relaxed);
                                for o in v.iter().filter(|o| !scalar(o)) {
                                    b.lock().unwrap().push(format!("{r}/slots: {o:?}"));
                                }
                            }
                        }
                    });
                    ["pos", "val", "dead"]
                        .map(|m| format!("{root}/{m}"))
                        .to_vec()
                }
                k2_mpc_base::schema::Kind::Map => vec![root.to_string()],
                other => panic!("the grid schema has a {other:?} root"),
            };
            for name in maps {
                let m = mirror.get_or_insert_map(name.clone());
                names.push(name.clone());
                let (b, n) = (bad.clone(), seen.clone());
                m.observe("guard", move |txn, e: &yrs::types::map::MapEvent| {
                    for (key, c) in e.keys(txn) {
                        let new = match c {
                            EntryChange::Inserted(v) | EntryChange::Updated(_, v) => v,
                            EntryChange::Removed(_) => continue,
                        };
                        n.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if !scalar(new) {
                            b.lock().unwrap().push(format!("{name}[{key}]: {new:?}"));
                        }
                    }
                });
            }
        }
        // the import, then a random script, each update as the wire carries it
        let mut rng = SmallRng::seed_from_u64(2000 + seed);
        let recs: Vec<Record> = (0..4)
            .map(|r| (0..3).map(|c| Field::new(format!("v{r}{c}"))).collect())
            .collect();
        let (mut t, edits) = Table::import(&recs, false, Shape::Rect).unwrap();
        let mut d = Doc::new(S).unwrap();
        let mut ups = vec![d.edit_as(7, &edits).unwrap().expect("the import changes")];
        for _ in 0..30 {
            let Some(e) = random_op(&mut t, &mut rng) else {
                continue;
            };
            if let Some(u) = d.edit_as(100 + rng.gen_range(0..3), &e).unwrap() {
                ups.push(u);
            }
        }
        for u in ups {
            let u = Update::decode_v1(&u).expect("a kit update decodes");
            mirror.transact_mut().apply_update(u).expect("applies");
            applied += 1;
        }
        let txn = mirror.transact();
        for (name, _) in yrs::ReadTxn::root_refs(&txn) {
            assert!(
                names.iter().any(|n| n == name),
                "{WAIT}: a root not in the grid schema: {name}"
            );
        }
    }
    let bad = bad.lock().unwrap();
    assert!(bad.is_empty(), "{WAIT}:\n{}", bad.join("\n"));
    let seen = seen.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        applied > 400 && seen > 2_000,
        "{applied} updates, {seen} values checked"
    );
}
