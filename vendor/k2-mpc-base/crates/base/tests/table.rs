//! The `table` module through the public API, as property tests.
//!
//! - **Cross-implementation:** the plain applier ([`Table::apply`], what every builder does
//!   to its view) and the kit Doc (Yrs) agree after every change of a random grid-op script:
//!   `Table::from_keyed(doc.keyed_state())` equals the builder's view.
//! - **The aligner:** `reconcile(t, t.render())` is no edit; for `g` reached from `t` by
//!   random grid ops, applying `reconcile(t, g.render())` to `t` (the plain table and the
//!   doc) renders as `g` (quote exceptions included), and rows whose record is unchanged and
//!   unique keep their keys; it is a pure function. (A file carries no identity: where two
//!   readings render the same, as a swap of two values or a swap of two columns, the aligner
//!   picks one; the unit tests pin the plain cases.)
//! - **The merge:** with no concurrent change, `merge3(b, o, b)` applied to `b` is `o`
//!   exactly; with no change of ours it is nothing; ours' changed cells are applied or named
//!   by a conflict, and theirs' writes are never overwritten (no last-writer-wins); clean
//!   merges are symmetric in content (orders may differ within a gap); the doc agrees.
//! - Sorts and block moves are one forward chain.
//!
//! `PROPTEST_CASES` scales it (256 by default; run more on a compute node). Each property
//! prints a `RUNREC` line with its case count and wall time (`table_pt_<name>`).

use k2_mpc_base::elem::{Anchor, ListOp, MapOp};
use k2_mpc_base::table::{
    self, merge3, reconcile, ConflictRule, Field, Held, Record, Region, Shape, SortKey, Table,
};
use k2_mpc_base::{Doc, Edit, Key, Schema};
use proptest::prelude::*;

const S: Schema = Schema {
    name: "table",
    roots: table::ROOTS,
};
const ME: u64 = 5;
const VALUES: &[&str] = &["", "a", "b", "x,y", "q\"q", "l1\nl2", "10", "9"];

/// A random property's config (every random grid property uses it): the run's seed is
/// `PROPTEST_RNG_SEED` if set, else drawn here, and printed first, so a failure's captured
/// output names it and `PROPTEST_RNG_SEED=<seed>` replays the run exactly; failing cases persist
/// next to this file (`*.proptest-regressions`, proptest's own format) and run first next time.
fn seeded(cases: u32, name: &str) -> proptest::test_runner::Config {
    let seed = std::env::var("PROPTEST_RNG_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            use std::hash::{BuildHasher, Hasher};
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos()),
            );
            h.finish()
        });
    println!("PROPTEST_RNG_SEED={seed} replays {name}");
    let mut c = proptest::test_runner::Config::with_cases(cases);
    c.rng_seed = proptest::test_runner::RngSeed::Fixed(seed);
    c.source_file = Some(file!());
    c
}

/// `MPC_MERGE_MUTANT` runs `f` with a merge fault planted on this thread
/// (`k2_mpc_base::differential::MergeMutant`, the `differential` feature, on for these tests), so
/// the keyed merges' tests can be shown to catch it: `keyed-merge-neutral-switch` is mutant
/// (xxxviii), `keyed-outside-neutral-switch` is (xxxix). Unset, `f` runs as is. Any other value
/// panics.
fn under_merge_mutant<R>(f: impl FnOnce() -> R) -> R {
    use k2_mpc_base::differential::{with_merge_mutant, MergeMutant};
    match std::env::var("MPC_MERGE_MUTANT").as_deref() {
        Ok("keyed-merge-neutral-switch") => {
            with_merge_mutant(MergeMutant::KeyedMergeNeutralSwitch, f)
        }
        Ok("keyed-outside-neutral-switch") => {
            with_merge_mutant(MergeMutant::KeyedOutsideNeutralSwitch, f)
        }
        Ok(other) => panic!("MPC_MERGE_MUTANT={other}: not a merge mutant this suite plants"),
        Err(_) => f(),
    }
}

/// The case count: `PROPTEST_CASES` (proptest's own default config reads it; 256 if unset).
fn cases() -> u32 {
    ProptestConfig::default().cases
}

/// The run record of one property (`tools/evidence/README.md`): its cases and wall time.
fn runrec(name: &str, start: std::time::Instant) {
    println!(
        "RUNREC {{\"job\":null,\"run\":\"table_pt_{name}\",\"tool\":\"proptest\",\"status\":\"PASSED\",\"cases\":{},\"wall_s\":{:.1}}}",
        cases(),
        start.elapsed().as_secs_f64()
    );
}

fn v(i: u8) -> String {
    VALUES[i as usize % VALUES.len()].to_owned()
}

/// One grid op, by indices into the live rows and columns (taken modulo their counts).
#[derive(Debug, Clone)]
enum Op {
    Set(usize, usize, u8),
    Clear(usize, usize),
    InsRow(u8, usize, u8),
    MoveRow(usize, u8, usize),
    DelRow(usize),
    ResRow(usize),
    InsCol(u8, usize, bool),
    MoveCol(usize, u8, usize),
    DelCol(usize),
    ResCol(usize),
    Rename(usize, u8),
    Sort(usize, bool, bool),
    Block(usize, usize, u8, usize),
    Header(bool),
}

fn op() -> impl Strategy<Value = Op> {
    let i = || 0usize..16;
    prop_oneof![
        4 => (i(), i(), any::<u8>()).prop_map(|(a, b, c)| Op::Set(a, b, c)),
        1 => (i(), i()).prop_map(|(a, b)| Op::Clear(a, b)),
        2 => (any::<u8>(), i(), any::<u8>()).prop_map(|(a, b, c)| Op::InsRow(a, b, c)),
        2 => (i(), any::<u8>(), i()).prop_map(|(a, b, c)| Op::MoveRow(a, b, c)),
        1 => i().prop_map(Op::DelRow),
        1 => i().prop_map(Op::ResRow),
        1 => (any::<u8>(), i(), any::<bool>()).prop_map(|(a, b, c)| Op::InsCol(a, b, c)),
        1 => (i(), any::<u8>(), i()).prop_map(|(a, b, c)| Op::MoveCol(a, b, c)),
        1 => i().prop_map(Op::DelCol),
        1 => i().prop_map(Op::ResCol),
        1 => (i(), any::<u8>()).prop_map(|(a, b)| Op::Rename(a, b)),
        1 => (i(), any::<bool>(), any::<bool>()).prop_map(|(a, b, c)| Op::Sort(a, b, c)),
        1 => (i(), i(), any::<u8>(), i()).prop_map(|(a, b, c, d)| Op::Block(a, b, c, d)),
        1 => any::<bool>().prop_map(Op::Header),
    ]
}

fn pick(keys: &[Key], i: usize) -> Option<Key> {
    (!keys.is_empty()).then(|| keys[i % keys.len()].clone())
}

fn anchor(keys: &[Key], how: u8, i: usize) -> Anchor {
    match (how % 4, pick(keys, i)) {
        (0, _) | (_, None) => Anchor::Start,
        (1, _) => Anchor::End,
        (2, Some(k)) => Anchor::After(k),
        (_, Some(k)) => Anchor::Before(k),
    }
}

/// Run `op` on `t` through its builder; `None` when the builder refuses it (as a verb would).
fn run(t: &mut Table, op: &Op) -> Option<Vec<Edit>> {
    let (rows, cols) = (t.live_rows(), t.live_cols());
    let hidden = |a: &[Key], all: Vec<Key>, i: usize| {
        let d: Vec<Key> = all.into_iter().filter(|k| !a.contains(k)).collect();
        pick(&d, i)
    };
    let r = match op {
        Op::Set(a, b, c) => t.set_cell(&pick(&rows, *a)?, &pick(&cols, *b)?, &v(*c)),
        Op::Clear(a, b) => t.clear_cell(&pick(&rows, *a)?, &pick(&cols, *b)?),
        Op::InsRow(how, i, n) => {
            let vals: Vec<String> = (0..(*n as usize % (cols.len() + 1)))
                .map(|j| v(n.wrapping_add(j as u8)))
                .collect();
            t.insert_row(anchor(&rows, *how, *i), &vals).map(|(_, e)| e)
        }
        Op::MoveRow(a, how, i) => {
            let k = pick(&rows, *a)?;
            t.move_row(&k, anchor(&rows, *how, *i))
        }
        Op::DelRow(a) => t.delete_row(&pick(&rows, *a)?),
        Op::ResRow(a) => t.restore_row(&hidden(&rows, t.rows.deleted().cloned().collect(), *a)?),
        Op::InsCol(how, i, named) => {
            let name = (*named && t.header).then_some("n");
            t.insert_col(anchor(&cols, *how, *i), name).map(|(_, e)| e)
        }
        Op::MoveCol(a, how, i) => {
            let k = pick(&cols, *a)?;
            t.move_col(&k, anchor(&cols, *how, *i))
        }
        Op::DelCol(a) => t.delete_col(&pick(&cols, *a)?),
        Op::ResCol(a) => t.restore_col(&hidden(&cols, t.cols.deleted().cloned().collect(), *a)?),
        Op::Rename(a, c) => t.rename_col(&pick(&cols, *a)?, &v(*c)),
        Op::Sort(a, desc, numeric) => {
            let e = t.sort(&[SortKey {
                col: pick(&cols, *a)?,
                desc: *desc,
                numeric: *numeric,
            }]);
            if let Ok(e) = &e {
                assert_chain(e);
            }
            e
        }
        Op::Block(a, n, how, i) => {
            let start = *a % rows.len().max(1);
            let block: Vec<Key> = rows.iter().skip(start).take(1 + n % 3).cloned().collect();
            let to = anchor(&rows, *how, *i);
            let e = t.move_rows(&block, to);
            if let Ok(e) = &e {
                assert_chain(e);
            }
            e
        }
        Op::Header(on) => t.set_header(*on),
    };
    r.ok()
}

/// Every list op after the first moves right after the previous one (one forward chain).
fn assert_chain(edits: &[Edit]) {
    for e in edits {
        if let Edit::List { ops, .. } = e {
            for w in ops.windows(2) {
                match (&w[0], &w[1]) {
                    (ListOp::Move { key, .. }, ListOp::Move { to, .. }) => {
                        assert_eq!(to, &Anchor::After(key.clone()), "not one chain: {ops:?}")
                    }
                    other => panic!("not moves: {other:?}"),
                }
            }
        }
    }
}

fn records_strategy() -> impl Strategy<Value = Vec<Record>> {
    prop::collection::vec(prop::collection::vec(any::<u8>(), 0..4), 0..6).prop_map(|rows| {
        rows.into_iter()
            .map(|r| r.into_iter().map(|c| Field::new(v(c))).collect())
            .collect()
    })
}

/// A table and its doc: an import, then `ops`.
fn grid(records: &[Record], header: bool, ragged: bool, ops: &[Op]) -> (Table, Doc) {
    let shape = if ragged {
        Shape::Ragged
    } else {
        Shape::detect(records)
    };
    let (mut t, edits) = Table::import(records, header, shape).expect("an import");
    let mut doc = Doc::new(S).expect("a doc");
    doc.edit_applied(ME, &edits).expect("the import applies");
    for o in ops {
        if let Some(e) = run(&mut t, o) {
            if !e.is_empty() {
                doc.edit_applied(ME, &e).expect("a builder's edits apply");
            }
        }
    }
    (t, doc)
}

/// `x` without what no reader of a merge can see: rows and columns it inserted since `b` and
/// deleted again, and their cells (a merge carries changes, not that history).
fn seen(b: &Table, x: &Table) -> Table {
    let gone = |a: &table::Axis, ba: &table::Axis| -> Vec<Key> {
        a.deleted().filter(|k| !ba.knows(k)).cloned().collect()
    };
    let (gr, gc) = (gone(&x.rows, &b.rows), gone(&x.cols, &b.cols));
    let mut y = x.clone();
    y.rows = table::Axis::from_live(x.live_rows());
    y.cols = table::Axis::from_live(x.live_cols());
    let dead = |a: &table::Axis, g: &[Key]| -> Vec<Key> {
        a.deleted().filter(|k| !g.contains(k)).cloned().collect()
    };
    // keep the deleted keys base knew, as deleted
    let mut e = Vec::new();
    for (root, keys) in [
        (table::ROWS, dead(&x.rows, &gr)),
        (table::COLS, dead(&x.cols, &gc)),
    ] {
        let mut ops = Vec::new();
        for k in keys {
            ops.push(ListOp::Insert {
                key: k.clone(),
                at: Anchor::End,
                value: k2_mpc_base::Value::Null,
            });
            ops.push(ListOp::Delete { key: k });
        }
        e.push(Edit::List {
            root: root.into(),
            ops,
        });
    }
    y.apply(&e).unwrap();
    y.cells
        .retain(|(r, c), _| !gr.contains(r) && !gc.contains(c));
    y.quoting.retain(|at, _| match at {
        table::QuoteAt::Cell(r, c) => !gr.contains(r) && !gc.contains(c),
        table::QuoteAt::Header(c) => !gc.contains(c),
    });
    y.names.retain(|c, _| !gc.contains(c));
    y
}

fn read(doc: &Doc) -> Table {
    Table::from_keyed(&doc.keyed_state().expect("a keyed state")).expect("a table")
}

fn values(t: &Table) -> Vec<Vec<String>> {
    t.render()
        .into_iter()
        .map(|r| r.into_iter().map(|f| f.value).collect())
        .collect()
}

#[test]
fn the_plain_applier_and_the_doc_agree() {
    let start = std::time::Instant::now();
    proptest!(seeded(cases(), "the_plain_applier_and_the_doc_agree"), |(
            records in records_strategy(),
            header in any::<bool>(),
            ragged in any::<bool>(),
            ops in prop::collection::vec(op(), 0..24),
        )| {
            let shape = if ragged { Shape::Ragged } else { Shape::detect(&records) };
            let (mut t, edits) = Table::import(&records, header, shape).unwrap();
            let mut doc = Doc::new(S).unwrap();
            doc.edit_applied(ME, &edits).unwrap();
            prop_assert_eq!(&read(&doc), &t, "after the import");
            prop_assert_eq!(t.render(), records.clone(), "an import renders its records");
            for (i, o) in ops.iter().enumerate() {
                let before = t.clone();
                let Some(e) = run(&mut t, o) else { continue };
                let mut again = before.clone();
                again.apply(&e).unwrap();
                prop_assert_eq!(&again, &t, "op {} ({:?}): the builder's view is its edits", i, o);
                if !e.is_empty() {
                    doc.edit_applied(ME, &e).unwrap();
                }
                prop_assert_eq!(&read(&doc), &t, "op {} ({:?}): the doc and the view differ", i, o);
            }
    });
    runrec("the_plain_applier_and_the_doc_agree", start);
}

/// C1k (request 3): `header on|off` never changes the rendering; the refusals are
/// `on` where the only live row is blank, and `on` with no row over names kept unrendered
/// (`MPC-E806`); each changes nothing.
#[test]
fn the_header_switch_never_changes_the_rendering() {
    let start = std::time::Instant::now();
    proptest!(seeded(cases(), "the_header_switch_never_changes_the_rendering"), |(
            records in records_strategy(),
            header in any::<bool>(),
            ragged in any::<bool>(),
            setup in prop::collection::vec(op(), 0..12),
        )| {
            let (mut t, _) = grid(&records, header, ragged, &setup);
            let before = t.clone();
            let r = t.render();
            match t.set_header(!before.header) {
                Ok(e) => {
                    prop_assert_eq!(t.render(), r, "the switch changed the rendering");
                    let mut again = before.clone();
                    again.apply(&e).unwrap();
                    prop_assert_eq!(again, t);
                }
                Err(table::TableError::HeaderOnlyRowBlank) => {
                    prop_assert!(!before.header && before.live_rows().len() == 1);
                    prop_assert_eq!(t, before, "refused: nothing applied");
                }
                // with no row, over names kept while the header was off (MPC-E806)
                Err(table::TableError::HeaderNamesUnrendered) => {
                    prop_assert!(!before.header && before.live_rows().is_empty());
                    prop_assert_eq!(t, before, "refused: nothing applied");
                }
                Err(e) => prop_assert!(false, "{e}"),
            }
    });
    runrec("the_header_switch_never_changes_the_rendering", start);
}

#[test]
fn reconcile_of_a_rendering() {
    let start = std::time::Instant::now();
    proptest!(seeded(cases(), "reconcile_of_a_rendering"), |(
            records in records_strategy(),
            header in any::<bool>(),
            ragged in any::<bool>(),
            setup in prop::collection::vec(op(), 0..10),
            change in prop::collection::vec(op(), 0..10),
        )| {
            let (t, doc) = grid(&records, header, ragged, &setup);
            let none = reconcile(&t, &t.render()).unwrap();
            prop_assert!(none.is_empty(), "reconcile(render(t)) = {:?}", none.edits);
            prop_assert_eq!(&none.rows, &t.live_rows());

            let mut g = t.clone();
            for o in &change {
                run(&mut g, o);
            }
            let file = g.render();
            let r = reconcile(&t, &file).unwrap();
            prop_assert_eq!(&reconcile(&t.clone(), &file).unwrap(), &r, "not a pure function");
            let mut u = t.clone();
            u.apply(&r.edits).unwrap();
            prop_assert_eq!(values(&u), values(&g), "the result renders as the file");
            prop_assert_eq!(u.render(), file.clone(), "quote exceptions too");
            prop_assert_eq!(&r.rows, &u.live_rows());
            // the doc agrees
            let mut doc = doc;
            if !r.edits.is_empty() {
                doc.edit_applied(ME, &r.edits).unwrap();
            }
            prop_assert_eq!(read(&doc).render(), file.clone(), "the doc renders as the file");
            // rows whose record is unchanged and unique keep their keys (columns unchanged)
            if g.header == t.header && t.live_cols() == g.live_cols() && r.cols == g.live_cols() {
                let old = t.render();
                let (oo, of) = (old.len() - t.live_rows().len(), file.len() - g.live_rows().len());
                let count = |recs: &[Record], x: &Record| recs.iter().filter(|y| *y == x).count();
                for (n, k) in g.live_rows().iter().enumerate() {
                    let rec = &file[of + n];
                    let Some(o) = t.live_rows().iter().position(|x| x == k) else { continue };
                    if &old[oo + o] == rec && count(&old, rec) == 1 && count(&file, rec) == 1 {
                        prop_assert_eq!(&r.rows[n], k, "an unchanged unique row lost its key");
                    }
                }
            }
    });
    runrec("reconcile_of_a_rendering", start);
}

#[test]
fn merge3_properties() {
    under_merge_mutant(merge3_properties_run);
}

fn merge3_properties_run() {
    let start = std::time::Instant::now();
    proptest!(seeded(cases(), "merge3_properties"), |(
            records in records_strategy(),
            header in any::<bool>(),
            ragged in any::<bool>(),
            setup in prop::collection::vec(op(), 0..8),
            ours in prop::collection::vec(op(), 0..8),
            theirs in prop::collection::vec(op(), 0..8),
        )| {
            let (b, doc) = grid(&records, header, ragged, &setup);
            let mut o = b.clone();
            for x in &ours {
                run(&mut o, x);
            }
            // theirs mints elsewhere, so the symmetric merge needs no re-keying
            let mut t = b.clone();
            t.next_row += 1000;
            t.next_col += 1000;
            for x in &theirs {
                run(&mut t, x);
            }
            // no concurrent change: ours, exactly
            let m = merge3(&b, &o, &b).unwrap();
            prop_assert!(m.is_clean(), "{:?}", m.conflicts);
            let mut r = b.clone();
            r.apply(&m.edits).unwrap();
            prop_assert_eq!(&r, &seen(&b, &o), "merge3(b, o, b) is o");
            // no change of ours: nothing
            let m = merge3(&b, &b, &t).unwrap();
            prop_assert!(m.is_clean() && m.edits.is_empty(), "{:?}", m);

            let m = merge3(&b, &o, &t).unwrap();
            prop_assert_eq!(&merge3(&b, &o, &t).unwrap(), &m, "not a pure function");
            let mut r = t.clone();
            r.apply(&m.edits).unwrap();
            // the doc agrees: theirs as a doc, then the merge
            let mut d = doc;
            let mut tt = b.clone();
            tt.next_row += 1000;
            tt.next_col += 1000;
            for x in &theirs {
                if let Some(e) = run(&mut tt, x) {
                    if !e.is_empty() {
                        d.edit_applied(ME, &e).unwrap();
                    }
                }
            }
            if !m.edits.is_empty() {
                d.edit_applied(ME, &m.edits).unwrap();
            }
            prop_assert_eq!(read(&d).render(), r.render(), "the doc and the plain merge differ");
            // theirs' writes are never overwritten; ours' are applied or reported
            let named = |row: &Key, col: &Key| m.conflicts.iter().any(|c| {
                c.at == Region::Cell { row: row.clone(), col: col.clone() }
            });
            for ((row, col), tv) in &t.cells {
                if b.cells.get(&(row.clone(), col.clone())) != Some(tv) {
                    prop_assert_eq!(r.cell(row, col), Some(tv.as_str()), "theirs' {}/{} lost", row, col);
                }
            }
            let uncarried = |row: &Key, col: &Key| {
                (!b.rows.knows(row) && !o.rows.is_live(row)) || (!b.cols.knows(col) && !o.cols.is_live(col))
            };
            for ((row, col), ov) in &o.cells {
                if b.cells.get(&(row.clone(), col.clone())) != Some(ov) && !named(row, col)
                    && !uncarried(row, col)
                {
                    let tv = t.cells.get(&(row.clone(), col.clone()));
                    prop_assert!(
                        r.cell(row, col) == Some(ov.as_str()) || tv.is_some_and(|x| x == ov),
                        "ours' {}/{} neither applied nor reported", row, col
                    );
                }
            }
            for c in &m.conflicts {
                let on_a_cell = matches!(c.at, Region::Cell { .. });
                if c.rule == ConflictRule::Cell {
                    prop_assert!(on_a_cell, "a cell conflict not on a cell: {}", c);
                }
            }
            // clean both ways: the same content (orders may differ within a gap)
            let back = merge3(&b, &t, &o).unwrap();
            if m.is_clean() && back.is_clean() {
                let mut q = o.clone();
                q.apply(&back.edits).unwrap();
                let (r, q) = (seen(&b, &r), seen(&b, &q));
                let sorted = |mut v: Vec<Key>| { v.sort(); v };
                prop_assert_eq!(&r.cells, &q.cells);
                prop_assert_eq!(&r.names, &q.names);
                prop_assert_eq!(&r.quoting, &q.quoting);
                prop_assert_eq!(sorted(r.live_rows()), sorted(q.live_rows()));
                prop_assert_eq!(sorted(r.live_cols()), sorted(q.live_cols()));
                prop_assert_eq!(r.header, q.header);
            }
    });
    runrec("merge3_properties", start);
}

/// `merge3_properties`' shrunk case of seed 683950541548579763 (v0.10.0), by key: base row r1
/// has a present cell (c3, set to `""`), so it renders every column and its absent c2 reads as
/// an empty field; theirs clears c2 (absent to `""`: the same bytes) and ours sets it to `"a"`.
/// By key a clear is a write (K-CSV-10a), so both set r1/c2: one `cell` conflict, theirs' `""`
/// stands. (The row delete, ours' insert and theirs' move are the shrunk case's; they only shape
/// which rows the random ops picked.)
#[test]
fn merge3_a_live_clear_of_an_absent_cell_ours_set_is_a_cell_conflict() {
    let k = |s: &str| Key::new(s).unwrap();
    let recs = vec![
        vec![],
        vec![],
        vec![],
        vec![Field::new(""), Field::new(""), Field::new("")],
    ];
    let (mut b, _) = Table::import(&recs, true, Shape::detect(&recs)).unwrap();
    assert_eq!(b.live_rows(), [k("r1"), k("r2"), k("r3")]);
    assert_eq!(b.live_cols(), [k("c1"), k("c2"), k("c3")]);
    // setup: DelRow(1), Set(2, 2, 0)
    b.delete_row(&k("r2")).unwrap();
    b.set_cell(&k("r1"), &k("c3"), "").unwrap();
    assert_eq!(b.cell(&k("r1"), &k("c2")), None, "r1/c2 absent in base");
    // ours: InsRow(69, 0, 0), Set(15, 1, 1)
    let mut o = b.clone();
    let (r4, _) = o.insert_row(Anchor::End, &[]).unwrap();
    o.set_cell(&k("r1"), &k("c2"), "a").unwrap();
    // theirs: MoveRow(1, 175, 0), Clear(7, 7)
    let mut t = b.clone();
    t.next_row += 1000;
    t.next_col += 1000;
    t.move_row(&k("r3"), Anchor::Before(k("r1"))).unwrap();
    t.clear_cell(&k("r1"), &k("c2")).unwrap();
    assert_eq!(t.cell(&k("r1"), &k("c2")), Some(""));

    let m = under_merge_mutant(|| merge3(&b, &o, &t)).unwrap();
    let cell = Region::Cell {
        row: k("r1"),
        col: k("c2"),
    };
    let at: Vec<(ConflictRule, &Region)> = m.conflicts.iter().map(|c| (c.rule, &c.at)).collect();
    assert_eq!(at, [(ConflictRule::Cell, &cell)], "both set r1/c2: {m:?}");
    let mut r = t.clone();
    r.apply(&m.edits).unwrap();
    assert_eq!(r.cell(&k("r1"), &k("c2")), Some(""), "theirs' r1/c2 lost");
    // the clean part still lands: ours' new row, at the end
    assert_eq!(r.live_rows(), [k("r3"), k("r1"), r4]);
}

/// A keyed side's cell made absent: a raw keyed cell delete (`Edit::Map`, no verb makes one;
/// a clear is a set to `""`), so the switches between empty and absent go both ways.
fn unset(t: &mut Table, row: &Key, col: &Key) {
    let ops = vec![MapOp::Delete {
        key: table::cell_key(row, col),
    }];
    t.apply(&[Edit::Map {
        root: table::CELLS.into(),
        ops,
    }])
    .expect("a cell delete applies");
}

/// `merge3_outside`'s rule (its doc comment, contract §2.9.1): "Where both sides changed one
/// element, the file's side (`ours` …) wins, and the overlap is reported in
/// [`Merge::conflicts`] as **overridden**, not left out: `cell`, `name`, `quoting`: the file's
/// value is set". By key a switch between absent and `""` is a write of the cell (a clear is a
/// set to `""`, K-CSV-10a), even where the row renders the same bytes either way: the
/// byte-neutral rule is a text's (a text can't tell an empty field from an absent cell). So:
/// base r1 renders every column (rect, `x,y` and an inserted, absent c3); theirs switches
/// r1/c3 (absent to `""`, and the reverse); ours (the file) sets it to `"a"`: the file's `"a"`,
/// and one overridden `cell` at r1/c3 holding all three values.
#[test]
fn merge3_outside_over_a_live_byte_neutral_switch_reports_the_override() {
    let k = |s: &str| Key::new(s).unwrap();
    let held = |v: Option<&str>| Held::Value(v.map(str::to_owned));
    let recs = vec![vec![Field::new("x"), Field::new("y")]];
    for absent_in_base in [true, false] {
        let (mut b, _) = Table::import(&recs, false, Shape::detect(&recs)).unwrap();
        let (c3, _) = b.insert_col(Anchor::End, None).unwrap();
        let (r1, at) = (
            k("r1"),
            Region::Cell {
                row: k("r1"),
                col: c3.clone(),
            },
        );
        if !absent_in_base {
            b.clear_cell(&r1, &c3).unwrap();
        }
        let mut t = b.clone();
        t.next_row += 1000;
        t.next_col += 1000;
        if absent_in_base {
            t.clear_cell(&r1, &c3).unwrap();
        } else {
            unset(&mut t, &r1, &c3);
        }
        // the switch is byte-neutral: the live row renders as base's
        assert_eq!(t.render(), b.render(), "a byte-neutral switch");
        assert_ne!(t.cell(&r1, &c3), b.cell(&r1, &c3), "theirs switched r1/c3");
        let mut o = b.clone();
        o.set_cell(&r1, &c3, "a").unwrap();

        let m = under_merge_mutant(|| table::merge3_outside(&b, &o, &t)).unwrap();
        let rules: Vec<(ConflictRule, &Region)> =
            m.conflicts.iter().map(|c| (c.rule, &c.at)).collect();
        assert_eq!(
            rules,
            [(ConflictRule::Cell, &at)],
            "absent in base {absent_in_base}: the file's override of r1/c3 not reported: {m:?}"
        );
        let c = &m.conflicts[0];
        assert_eq!(c.base, held(b.cell(&r1, &c3)));
        assert_eq!(c.ours, held(Some("a")));
        assert_eq!(c.theirs, held(t.cell(&r1, &c3)));
        let mut r = t.clone();
        r.apply(&m.edits).unwrap();
        assert_eq!(r.cell(&r1, &c3), Some("a"), "the file's value");
    }
}

/// The other byte-neutral shape of an outside save (`file_renders_live` in the merge): base
/// r1/c2 holds `"y"`; the file empties it one way (absent, or `""`) and the live side the other.
/// Distinct from the test above: base's cell is not empty, so no base row switch is involved,
/// and the file's row renders the same bytes with either value. By key both sides changed r1/c2
/// to different values, so by the rule above: the file's value, and one overridden `cell`.
#[test]
fn merge3_outside_over_a_live_empty_the_file_emptied_otherwise_reports_the_override() {
    let k = |s: &str| Key::new(s).unwrap();
    let held = |v: Option<&str>| Held::Value(v.map(str::to_owned));
    let recs = vec![vec![Field::new("x"), Field::new("y")]];
    for file_absent in [true, false] {
        let (b, _) = Table::import(&recs, false, Shape::detect(&recs)).unwrap();
        let (r1, c2) = (k("r1"), k("c2"));
        assert_eq!(b.cell(&r1, &c2), Some("y"));
        let (mut o, mut t) = (b.clone(), b.clone());
        t.next_row += 1000;
        t.next_col += 1000;
        if file_absent {
            unset(&mut o, &r1, &c2);
            t.clear_cell(&r1, &c2).unwrap();
        } else {
            o.clear_cell(&r1, &c2).unwrap();
            unset(&mut t, &r1, &c2);
        }
        assert_eq!(
            o.render(),
            t.render(),
            "the file's row renders the live value too"
        );
        assert_ne!(o.cell(&r1, &c2), t.cell(&r1, &c2));

        let m = under_merge_mutant(|| table::merge3_outside(&b, &o, &t)).unwrap();
        let at = Region::Cell {
            row: r1.clone(),
            col: c2.clone(),
        };
        let rules: Vec<(ConflictRule, &Region)> =
            m.conflicts.iter().map(|c| (c.rule, &c.at)).collect();
        assert_eq!(
            rules,
            [(ConflictRule::Cell, &at)],
            "file absent {file_absent}: the file's override of r1/c2 not reported: {m:?}"
        );
        let c = &m.conflicts[0];
        assert_eq!(c.base, held(Some("y")));
        assert_eq!(c.ours, held(o.cell(&r1, &c2)));
        assert_eq!(c.theirs, held(t.cell(&r1, &c2)));
        let mut r = t.clone();
        r.apply(&m.edits).unwrap();
        assert_eq!(r.cell(&r1, &c2), o.cell(&r1, &c2), "the file's value");
    }
}

/// One op of a keyed side in [`merge3_outside_properties`]: a grid op, or a present cell of a
/// live row and column made absent ([`unset`]).
#[derive(Debug, Clone)]
enum KOp {
    Grid(Op),
    Unset(usize),
}

fn k_op() -> impl Strategy<Value = KOp> {
    prop_oneof![
        8 => op().prop_map(KOp::Grid),
        1 => (0usize..64).prop_map(KOp::Unset),
    ]
}

fn run_k(t: &mut Table, op: &KOp) {
    match op {
        KOp::Grid(x) => {
            run(t, x);
        }
        KOp::Unset(i) => {
            let present: Vec<(Key, Key)> = t
                .cells
                .keys()
                .filter(|(r, c)| t.rows.is_live(r) && t.cols.is_live(c))
                .cloned()
                .collect();
            if !present.is_empty() {
                let (r, c) = present[i % present.len()].clone();
                unset(t, &r, &c);
            }
        }
    }
}

/// The keyed outside save (`merge3_outside`) against its rule (quoted on
/// [`merge3_outside_over_a_live_byte_neutral_switch_reports_the_override`]), cell by cell over
/// every cell any version holds, comparing values by key (absent and `""` differ):
/// - a cell both sides changed holds the file's value;
/// - a cell both sides changed to different values (a byte-neutral switch between absent and
///   `""` included) is reported as exactly one overridden `cell`, holding base's, the file's and
///   the live value; a cell only one side changed (or both to the same value) is not;
/// - a cell the live side changed and the file didn't keeps the live value (nothing lost);
/// - it is a pure function.
#[test]
fn merge3_outside_properties() {
    under_merge_mutant(merge3_outside_properties_run);
}

fn merge3_outside_properties_run() {
    let start = std::time::Instant::now();
    proptest!(seeded(cases(), "merge3_outside_properties"), |(
            records in records_strategy(),
            header in any::<bool>(),
            ragged in any::<bool>(),
            setup in prop::collection::vec(k_op(), 0..8),
            ours in prop::collection::vec(k_op(), 0..8),
            theirs in prop::collection::vec(k_op(), 0..8),
        )| {
            let (mut b, _) = grid(&records, header, ragged, &[]);
            for x in &setup {
                run_k(&mut b, x);
            }
            let mut o = b.clone();
            for x in &ours {
                run_k(&mut o, x);
            }
            // theirs mints elsewhere, so no key of ours is re-minted
            let mut t = b.clone();
            t.next_row += 1000;
            t.next_col += 1000;
            for x in &theirs {
                run_k(&mut t, x);
            }

            let m = table::merge3_outside(&b, &o, &t).unwrap();
            prop_assert_eq!(&table::merge3_outside(&b, &o, &t).unwrap(), &m, "not a pure function");
            let mut r = t.clone();
            r.apply(&m.edits).unwrap();
            let held = |v: Option<&str>| Held::Value(v.map(str::to_owned));
            let every: std::collections::BTreeSet<&(Key, Key)> =
                b.cells.keys().chain(o.cells.keys()).chain(t.cells.keys()).collect();
            for (row, col) in every {
                let (bv, ov, tv) = (b.cell(row, col), o.cell(row, col), t.cell(row, col));
                let at = Region::Cell { row: row.clone(), col: col.clone() };
                let reported: Vec<_> = m
                    .conflicts
                    .iter()
                    .filter(|c| c.rule == ConflictRule::Cell && c.at == at)
                    .collect();
                let (o_changed, t_changed) = (ov != bv, tv != bv);
                if o_changed && t_changed {
                    prop_assert_eq!(r.cell(row, col), ov, "both changed {}/{}: not the file's value", row, col);
                }
                if o_changed && t_changed && ov != tv {
                    prop_assert_eq!(
                        reported.len(), 1,
                        "both changed {}/{} (base {:?}, file {:?}, live {:?}): not reported once as overridden: {:?}",
                        row, col, bv, ov, tv, m.conflicts
                    );
                    let c = reported[0];
                    prop_assert_eq!(&c.base, &held(bv));
                    prop_assert_eq!(&c.ours, &held(ov));
                    prop_assert_eq!(&c.theirs, &held(tv));
                } else {
                    prop_assert!(
                        reported.is_empty(),
                        "{}/{} (base {:?}, file {:?}, live {:?}) reported as overridden",
                        row, col, bv, ov, tv
                    );
                }
                if t_changed && !o_changed {
                    prop_assert_eq!(r.cell(row, col), tv, "the live {}/{} lost", row, col);
                }
            }
    });
    runrec("merge3_outside_properties", start);
}

/// One old row's fate in a combined outside save.
#[derive(Debug, Clone, Copy)]
enum Fate {
    Keep,
    /// Change these non-tag cells (a bit per column), at most half of the row.
    Edit(u8),
    /// Every field new, its tag too: a new row unless it stays in its old slot (then it is
    /// paired by position).
    Rewrite,
    /// Edited into a blank line (a blank line edited back into a full row: `Edit`).
    Blank,
    Delete,
}

fn fate() -> impl Strategy<Value = Fate> {
    prop_oneof![
        3 => Just(Fate::Keep),
        4 => any::<u8>().prop_map(Fate::Edit),
        1 => Just(Fate::Rewrite),
        1 => Just(Fate::Blank),
        2 => Just(Fate::Delete),
    ]
}

/// Shared non-empty fields when the two are similar (at least half of either side's, at
/// least one), else 0: the aligner's own measure, computed here independently.
fn likeness(a: &Record, b: &Record) -> usize {
    let nz = |r: &Record| r.iter().filter(|f| !f.value.is_empty()).count();
    let eq = a
        .iter()
        .zip(b)
        .filter(|(x, y)| !x.value.is_empty() && x.value == y.value)
        .count();
    if eq >= 1 && 2 * eq >= nz(a).max(nz(b)) {
        eq
    } else {
        0
    }
}

/// The unique best partner of `x` among `ys` (by [`likeness`]; `None` on a tie or none).
fn unique_best(x: &Record, ys: &[Record]) -> Option<usize> {
    let s: Vec<usize> = ys.iter().map(|y| likeness(x, y)).collect();
    let m = *s.iter().max()?;
    (m > 0 && s.iter().filter(|&&v| v == m).count() == 1).then(|| s.iter().position(|&v| v == m))?
}

/// C1d part 2 and C1k: random grids (one to four columns, blank lines among the rows),
/// and ONE outside save combining moves, cell edits, inserts, deletes, rewrites and
/// blank-line edits. The result renders as the file; reconcile of a rendering is a no-op;
/// wherever the choice is unambiguous (a surviving row whose old and new records are each
/// other's unique best match) the row keeps its key, its changed cells as cell edits; a
/// row that shares nothing with the old row whose key it takes sits in that row's slot
/// (its neighbours are that row's, or the same end); and every run of new rows in an
/// unchanged slot as long as the run of deleted rows there took their keys (a blank line
/// edited, a one-column cell edited, a row rewritten in place). A row moved and rewritten
/// is a new row.
#[test]
fn reconcile_combined_saves_keep_keys() {
    let start = std::time::Instant::now();
    proptest!(seeded(cases(), "reconcile_combined_saves_keep_keys"), |(
            width in 1usize..5,
            cells in prop::collection::vec(
                (prop::collection::vec(0u8..3, 4), prop::bool::weighted(0.15)),
                1..9,
            ),
            fates in prop::collection::vec(fate(), 9),
            order in prop::collection::vec(any::<u16>(), 9),
            inserts in prop::collection::vec((any::<u16>(), prop::collection::vec(0u8..3, 4)), 0..3),
        )| {
            // column j's values are its own (`a1`, `b1`...): columns never match each other by
            // content; column 0 is a tag unique per row; a blank line has no field
            let val = |j: usize, x: u8| format!("{}{}", ["a", "b", "c"][x as usize], j);
            let old: Vec<Record> = cells
                .iter()
                .enumerate()
                .map(|(i, (r, blank))| {
                    if *blank {
                        return Vec::new();
                    }
                    (0..width)
                        .map(|j| Field::new(if j == 0 { format!("t{i}") } else { val(j, r[j]) }))
                        .collect()
                })
                .collect();
            prop_assume!(old.iter().any(|r| !r.is_empty()));
            let (t, edits) = Table::import(&old, false, Shape::Rect).unwrap();
            let mut doc = Doc::new(S).unwrap();
            doc.edit_applied(ME, &edits).unwrap();
            let none = reconcile(&t, &t.render()).unwrap();
            prop_assert!(none.is_empty(), "reconcile(render(t)) = {:?}", none.edits);
            let keys = t.live_rows();
            // the save: (old index or None, record)
            let mut save: Vec<(Option<usize>, Record)> = Vec::new();
            for (i, rec) in old.iter().enumerate() {
                match fates[i] {
                    Fate::Keep => save.push((Some(i), rec.clone())),
                    Fate::Edit(_) if rec.is_empty() => save.push((
                        Some(i),
                        (0..width).map(|j| Field::new(format!("e{j}{i}"))).collect(),
                    )),
                    Fate::Edit(bits) => {
                        let mut r = rec.clone();
                        let mut changed = 0;
                        for (j, f) in r.iter_mut().enumerate().skip(1) {
                            if bits & (1 << j) != 0 && 2 * (changed + 1) <= width {
                                *f = Field::new(format!("e{j}{i}"));
                                changed += 1;
                            }
                        }
                        save.push((Some(i), r));
                    }
                    Fate::Rewrite => save.push((
                        None,
                        (0..width).map(|j| Field::new(format!("w{j}{i}"))).collect(),
                    )),
                    Fate::Blank => save.push((Some(i), Vec::new())),
                    Fate::Delete => {}
                }
            }
            // moves: reorder by the random keys
            let mut idx: Vec<usize> = (0..save.len()).collect();
            idx.sort_by_key(|&k| (order[k % order.len()], k));
            let mut save: Vec<(Option<usize>, Record)> = idx.into_iter().map(|k| save[k].clone()).collect();
            for (n, (at, r)) in inserts.iter().enumerate() {
                let rec: Record = (0..width)
                    .map(|j| Field::new(if j == 0 { format!("n{n}") } else { val(j, r[j]) }))
                    .collect();
                let at = *at as usize % (save.len() + 1);
                save.insert(at, (None, rec));
            }
            let file: Vec<Record> = save.iter().map(|(_, r)| r.clone()).collect();
            let r = reconcile(&t, &file).unwrap();
            let mut u = t.clone();
            u.apply(&r.edits).unwrap();
            prop_assert_eq!(u.render(), file.clone(), "the result renders as the file");
            if !r.edits.is_empty() {
                doc.edit_applied(ME, &r.edits).unwrap();
            }
            prop_assert_eq!(read(&doc).render(), file.clone(), "the doc agrees");
            if file.iter().all(Vec::is_empty) {
                // a file of blank lines (or none) holds no column: nothing to key
                return Ok(());
            }
            prop_assert_eq!(&r.cols, &t.live_cols(), "no column changed");
            let none = reconcile(&u, &u.render()).unwrap();
            prop_assert!(none.is_empty(), "reconcile(render(g)) = {:?}", none.edits);
            for (n, (from, rec)) in save.iter().enumerate() {
                if let Some(o) = from {
                    let unambiguous = unique_best(rec, &old) == Some(*o)
                        && unique_best(&old[*o], &file) == Some(n);
                    if unambiguous {
                        prop_assert_eq!(&r.rows[n], &keys[*o], "row {} lost its key", o);
                    }
                    // every kept row whose record is unchanged keeps its key (tags make them
                    // unique; blank lines are all alike)
                    if *rec == old[*o] && !rec.is_empty() {
                        prop_assert_eq!(&r.rows[n], &keys[*o]);
                    }
                }
            }
            // a row that took the key of an old row it shares nothing with is in its slot
            let at = |key: &Key| keys.iter().position(|x| x == key);
            let (last_n, last_o) = (file.len() - 1, old.len() - 1);
            for (n, rec) in file.iter().enumerate() {
                let Some(o) = at(&r.rows[n]) else { continue };
                if likeness(rec, &old[o]) > 0 || *rec == old[o] {
                    continue;
                }
                let left = (n == 0 && o == 0) || (n > 0 && o > 0 && r.rows[n - 1] == keys[o - 1]);
                let right = (n == last_n && o == last_o)
                    || (n < last_n && o < last_o && r.rows[n + 1] == keys[o + 1]);
                prop_assert!(left && right, "row {} took {}'s key out of its slot: {:?}", n, o, r.rows);
            }
            // every run of new rows in an unchanged slot, as long as the run of deleted rows there,
            // took their keys (paired by position)
            let mut n1 = 0;
            while n1 < file.len() {
                if at(&r.rows[n1]).is_some() {
                    n1 += 1;
                    continue;
                }
                let mut n2 = n1;
                while n2 < file.len() && at(&r.rows[n2]).is_none() {
                    n2 += 1;
                }
                let o1 = if n1 == 0 { Some(0) } else { at(&r.rows[n1 - 1]).map(|o| o + 1) };
                let o2 = if n2 == file.len() { Some(old.len()) } else { at(&r.rows[n2]) };
                if let (Some(o1), Some(o2)) = (o1, o2) {
                    let gone = |o: usize| !r.rows.contains(&keys[o]);
                    prop_assert!(
                        !(o2 >= o1 && o2 - o1 == n2 - n1 && (o1..o2).all(gone)),
                        "new rows {}..{} in old rows {}..{}'s unchanged slot: {:?}", n1, n2, o1, o2, r.rows
                    );
                }
                n1 = n2;
            }
    });
    runrec("reconcile_combined_saves_keep_keys", start);
}
