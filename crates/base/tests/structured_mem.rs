//! Memory and latency of the structured kinds on Yrs (CSV seam design §4.4, §3), counted
//! with a global allocator.
//!
//! - `measure_*` (ignored; run on a compute node, release, `--ignored --nocapture
//!   --test-threads 1`): memory per row and per cell of a loaded doc, and `cell set` latency
//!   (p50/p99/max) on a doc at the interim cap (100,000 cells, about 2 MiB as CSV; target
//!   p99 ≤ 50 ms), with the peak heap during one edit (the throwaway replica). Each prints
//!   one `MEASURE` line per figure, and progress as it goes.
//! - The rest run in CI at a small size: loose upper bounds that catch a gross regression.
//!   (The pinned `map.steady_bounded` deviation is a conformance rule now:
//!   `crates/conformance`, `tests/structured.rs`.)
//!
//! Every test takes `LOCK`: the allocator counts the whole process.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_mpc_base::elem::{Anchor, ListOp, MapOp};
use k2_mpc_base::{Doc, Edit, Key, Kind, Schema, Value};

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size() as isize, Ordering::SeqCst) + l.size() as isize;
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l);
        LIVE.fetch_sub(l.size() as isize, Ordering::SeqCst);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = System.realloc(p, l, n);
        if !q.is_null() {
            let now = LIVE.fetch_add(n as isize - l.size() as isize, Ordering::SeqCst) + n as isize
                - l.size() as isize;
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

static LOCK: Mutex<()> = Mutex::new(());

fn live() -> isize {
    LIVE.load(Ordering::SeqCst)
}

/// The CSV contract's table, minus the small maps.
const S: Schema = Schema {
    name: "grid",
    roots: &[
        ("rows", Kind::MovableList),
        ("cols", Kind::MovableList),
        ("cells", Kind::Map),
    ],
};

/// A participant client ID as sessions mint them (random, 53 bits): stamps in the registers
/// are as long as in production.
const IMPORT: u64 = 0x1d_3a5f_09c2_b7e1;

fn key(prefix: char, i: usize) -> Key {
    Key::new(format!("{prefix}{i}")).unwrap()
}

/// Keys inserted in file order, as an import does: one forward chain.
fn chain_inserts(prefix: char, n: usize) -> Vec<ListOp> {
    (0..n)
        .map(|i| ListOp::Insert {
            key: key(prefix, i),
            at: if i == 0 {
                Anchor::Start
            } else {
                Anchor::After(key(prefix, i - 1))
            },
            value: Value::Null,
        })
        .collect()
}

/// A cell value of `len` bytes, distinct per cell.
fn value(r: usize, c: usize, len: usize) -> Value {
    let mut s = format!("{r}:{c}:");
    while s.len() < len {
        s.push('x');
    }
    Value::Str(s)
}

/// A table of `rows` × `cols`, every cell present with a `len`-byte value, imported as one
/// change.
fn table(rows: usize, cols: usize, len: usize) -> Doc {
    let mut d = Doc::new(S).unwrap();
    let mut cells = Vec::with_capacity(rows * cols);
    for r in 0..rows {
        for c in 0..cols {
            cells.push(MapOp::Set {
                key: format!("r{r}\u{1f}c{c}"),
                value: value(r, c, len),
            });
        }
    }
    d.edit_as(
        IMPORT,
        &[
            Edit::List {
                root: "rows".into(),
                ops: chain_inserts('r', rows),
            },
            Edit::List {
                root: "cols".into(),
                ops: chain_inserts('c', cols),
            },
            Edit::Map {
                root: "cells".into(),
                ops: cells,
            },
        ],
    )
    .unwrap()
    .unwrap();
    d
}

/// Heap of a doc loaded from `state` (what a session holds after a reload), with its list
/// orders computed (the cache a reader fills).
fn loaded_heap(state: &[u8]) -> (isize, Doc) {
    let before = live();
    let d = Doc::load(S, state).unwrap();
    d.list("rows").unwrap();
    d.list("cols").unwrap();
    (live() - before, d)
}

fn report(name: &str, value: f64, unit: &str) {
    println!("MEASURE {name} = {value:.1} {unit}");
    // a run record: the value rounded, and in tenths (citable integers)
    println!(
        "RUNREC {{\"job\":null,\"run\":\"measure_{name}\",\"tool\":\"structured_mem\",\"status\":\"MEASURED\",\"unit\":\"{unit}\",\"value\":{},\"value_x10\":{}}}",
        value.round() as i64,
        (value * 10.0).round() as i64
    );
}

fn progress(what: &str) {
    eprintln!("[progress] {what}");
}

/// (heap per row, heap per cell, stored state per cell): rows-only and cells-only tables,
/// differenced against a smaller one so fixed costs cancel.
fn per_element(n_small: usize, n_big: usize, len: usize) -> (f64, f64, f64) {
    let rows_only = |n| loaded_heap(&table(n, 0, len).encode_state().unwrap()).0;
    let per_row = (rows_only(n_big) - rows_only(n_small)) as f64 / (n_big - n_small) as f64;
    // cells: one column, so rows count too; subtract them
    let cells = |n| {
        let s = table(n, 1, len).encode_state().unwrap();
        (loaded_heap(&s).0, s.len())
    };
    let (h_small, s_small) = cells(n_small);
    let (h_big, s_big) = cells(n_big);
    let per_cell_and_row = (h_big - h_small) as f64 / (n_big - n_small) as f64;
    let state_per_cell = (s_big - s_small) as f64 / (n_big - n_small) as f64;
    (per_row, per_cell_and_row - per_row, state_per_cell)
}

#[test]
#[ignore = "measurement: run on a compute node (release, --ignored --nocapture)"]
fn measure_memory_per_row_and_cell() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    for len in [8usize, 20, 64] {
        progress(&format!("memory per element, {len}-byte values"));
        let (row, cell, state) = per_element(10_000, 50_000, len);
        report(&format!("heap_per_row_v{len}"), row, "B");
        report(&format!("heap_per_cell_v{len}"), cell, "B");
        report(&format!("state_per_cell_v{len}"), state, "B");
    }
    // the capped doc as a whole: 10,000 rows x 10 columns, 20-byte values (about 2 MiB as CSV)
    progress("capped doc (10,000 x 10, 20-byte values)");
    let d = table(10_000, 10, 20);
    let state = d.encode_state().unwrap();
    let (heap, l) = loaded_heap(&state);
    report("capped_doc_heap", heap as f64 / (1 << 20) as f64, "MiB");
    report(
        "capped_doc_state",
        state.len() as f64 / (1 << 20) as f64,
        "MiB",
    );
    report(
        "capped_doc_csv_bytes",
        (10_000 * 10 * 21) as f64 / (1 << 20) as f64,
        "MiB",
    );
    drop(l);
}

/// p50, p99 and max of `samples`.
fn quantiles(mut samples: Vec<Duration>) -> (Duration, Duration, Duration) {
    samples.sort();
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    (at(0.5), at(0.99), *samples.last().unwrap())
}

/// `cell set` latency on a doc of `rows` x 10 columns (20-byte values): `n` sets of random
/// cells by 16 clients. Prints p50/p99/max, and the peak heap one edit adds (the replica).
fn cell_set_latency(rows: usize, n: usize) -> (Doc, Duration) {
    let cells = rows * 10;
    progress(&format!(
        "building a doc of {rows} rows x 10 columns ({cells} cells)"
    ));
    let t = Instant::now();
    let mut d = table(rows, 10, 20);
    report(
        &format!("import_{cells}_cells"),
        t.elapsed().as_secs_f64() * 1e3,
        "ms",
    );
    let mut times = Vec::with_capacity(n);
    let mut peak_extra = 0isize;
    for i in 0..n {
        let (r, c) = ((i * 7919) % rows, (i * 31) % 10);
        let op = MapOp::Set {
            key: format!("r{r}\u{1f}c{c}"),
            value: Value::str(format!("edited {i}")),
        };
        let base = live();
        PEAK.store(base, Ordering::SeqCst);
        let t = Instant::now();
        let u = d
            .edit_as(
                0x15_0000_0000_0000 + (i as u64 % 16),
                &[Edit::Map {
                    root: "cells".into(),
                    ops: vec![op],
                }],
            )
            .unwrap();
        times.push(t.elapsed());
        peak_extra = peak_extra.max(PEAK.load(Ordering::SeqCst) - base);
        assert!(u.is_some());
        if i % 50 == 49 {
            progress(&format!("{} cell sets", i + 1));
        }
    }
    let (p50, p99, max) = quantiles(times);
    report(
        &format!("cell_set_{cells}_p50"),
        p50.as_secs_f64() * 1e3,
        "ms",
    );
    report(
        &format!("cell_set_{cells}_p99"),
        p99.as_secs_f64() * 1e3,
        "ms",
    );
    report(
        &format!("cell_set_{cells}_max"),
        max.as_secs_f64() * 1e3,
        "ms",
    );
    report(
        &format!("cell_set_{cells}_peak_extra_heap"),
        peak_extra as f64 / (1 << 20) as f64,
        "MiB",
    );
    (d, p99)
}

#[test]
#[ignore = "measurement: run on a compute node (release, --ignored --nocapture)"]
fn measure_cell_set_latency_at_the_cap() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // below the cap, to find where p99 <= 50 ms holds on this machine
    for rows in [1_000usize, 2_500, 5_000] {
        cell_set_latency(rows, 100);
    }
    // the cap: 100,000 cells, about 2 MiB as CSV
    let (d, _) = cell_set_latency(10_000, 300);
    // a reader after the sets: the rows' order was never read, so it is built once
    let t = Instant::now();
    let rows = d.list("rows").unwrap();
    report(
        "list_rows_100000_cells",
        t.elapsed().as_secs_f64() * 1e3,
        "ms",
    );
    assert_eq!(rows.len(), 10_000);
    let t = Instant::now();
    d.elem("rows", &key('r', 5000)).unwrap();
    report("elem_cached", t.elapsed().as_secs_f64() * 1e6, "us");
}

#[test]
#[ignore = "measurement: run on a compute node (release, --ignored --nocapture)"]
fn measure_sort_and_block_move() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    for rows in [1_000usize, 10_000] {
        progress(&format!("sort of {rows} rows"));
        let mut d = table(rows, 10, 20);
        let current: Vec<Key> = d
            .list("rows")
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        let target: Vec<Key> = current.iter().rev().cloned().collect();
        let ops = k2_mpc_base::elem::reorder(&current, &target).unwrap();
        let state_before = d.encode_state().unwrap().len();
        let t = Instant::now();
        d.edit_as(
            7,
            &[Edit::List {
                root: "rows".into(),
                ops,
            }],
        )
        .unwrap()
        .unwrap();
        report(
            &format!("sort_{rows}_rows"),
            t.elapsed().as_secs_f64() * 1e3,
            "ms",
        );
        let state_after = d.encode_state().unwrap().len();
        report(
            &format!("sort_{rows}_rows_state_growth_per_row"),
            (state_after as f64 - state_before as f64) / rows as f64,
            "B",
        );
        let block: Vec<Key> = (0..100).map(|i| key('r', i)).collect();
        let t = Instant::now();
        d.edit_as(
            7,
            &[Edit::List {
                root: "rows".into(),
                ops: k2_mpc_base::elem::chain(&block, Anchor::End),
            }],
        )
        .unwrap()
        .unwrap();
        report(
            &format!("block_move_100_of_{rows}"),
            t.elapsed().as_secs_f64() * 1e3,
            "ms",
        );
    }
}

/// A forward chain of n inserts (an import, a sort) peaks at O(n) bytes, not O(n^2). The
/// control shows what the check catches: the same n inserts made one by one in one raw yrs
/// transaction (what the kit did before it sent runs as ranges) peak far higher.
#[test]
fn a_forward_chain_peaks_linearly() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    use yrs::{Any, Array, Transact};
    let n = 2000u32;
    let base = live();
    PEAK.store(base, Ordering::SeqCst);
    let d = table(n as usize, 0, 20);
    let kit = PEAK.load(Ordering::SeqCst) - base;
    drop(d);
    let base = live();
    PEAK.store(base, Ordering::SeqCst);
    {
        let y = yrs::Doc::new();
        let a = y.get_or_insert_array("s");
        let mut t = y.transact_mut();
        for i in 0..n {
            a.insert(&mut t, i, Any::String(format!("{i}.0.0\u{1f}r{i}").into()));
        }
    }
    let raw = PEAK.load(Ordering::SeqCst) - base;
    eprintln!("{n} inserts: kit peak {kit} B, raw yrs one by one {raw} B");
    assert!(raw > 20 << 20, "the control no longer peaks high ({raw} B)");
    assert!(kit < 8 << 20, "a {n}-row import peaked at {kit} B");
}

/// CI-size bounds: a loaded table costs less than 1.5 KB per row and 1 KB per cell (D0's
/// estimate is about 1 KB and 250 B + the value), so a layout regression (say, a register
/// per cell more than designed) fails loudly.
#[test]
fn memory_per_element_is_bounded() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (row, cell, state) = per_element(1_000, 3_000, 20);
    eprintln!("per row {row:.0} B, per cell {cell:.0} B, stored {state:.1} B per cell");
    assert!(row > 0.0 && row < 1500.0, "{row} B per row");
    assert!(cell > 0.0 && cell < 1000.0, "{cell} B per cell");
    assert!(state < 150.0, "{state} B stored per cell");
}
