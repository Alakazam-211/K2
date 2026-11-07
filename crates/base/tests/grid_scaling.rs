//! What a grid's render and reconcile paths cost as its stored rows and columns grow: the
//! property is that each is linear in the grid's stored cells, rows and columns (a blank row
//! costs O(1) on every path), not in live rows × live columns.
//!
//! `probe_grid_scaling` (ignored; minutes in release: run it on a compute node) builds grids
//! in code, of three shapes:
//! - `wide_blank`: header on, `rect`, C named columns, R rows with no present cell;
//! - `ragged_wide`: header off, `ragged`, the first row holds a cell in every column, the
//!   other rows none;
//! - `dense`: header on, `rect`, every cell present (the honest grid).
//!
//! and measures, per path, a **timing-free** cost (heap allocations on the calling thread,
//! counted by this binary's allocator) and the wall time of one call: `Table::render`,
//! `Table::line_map`, the aligner (`table::reconcile`, given the table's own rendering and
//! one with a cell set), the structured pack's default reconcile
//! (`session::default_reconcile`), the three-way merges (`merge3`, `merge3_text`,
//! `merge3_outside_text`), and (`probe_grid_blame_scaling`) a session's grid blame. Each
//! series varies one size: `rows` (C fixed), `cols` (R fixed) and `diag` (both, R = 16 C);
//! `SLOPE` lines give the least-squares exponent of the allocations in the varied size and in
//! the stored size (cells + rows + columns + names).
//!
//! ```sh
//! cargo test -p k2-mpc-base --release --test grid_scaling -- --ignored --nocapture --test-threads 1
//! ```
//!
//! **The regression** (not ignored): `grid_paths_are_linear_in_stored_size` and
//! `grid_blame_is_linear_in_stored_size` hold each path's allocations, on a 1,024-column grid
//! of 4,000 blank rows (and its ragged twin), to a fixed multiple of its stored size (cells +
//! rows + columns + names): [`TABLE_BOUND`] for the table paths, [`BLAME_BOUND`] for a blame.
//! A walk over every live column of every row costs thousands per stored element there.
//! Planted fault (`MPC_GRID_COST_MUTANT=per-column`, the setting echoed): the per-column walk
//! (`k2_mpc_base::differential::GridCostMutant::PerColumn`), which both tests catch.
//!
//! `PROBE_BUDGET_S` (default 30): a series stops growing for a path once one call took longer.
//! `PROBE_WORST=1` also runs `Table::render` and `Table::line_map` once at the largest grid a
//! 2 MiB CSV file can hold under the kit's default limits (1,024 columns, 2,095,104 blank rows).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Key, Value};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    default_reconcile, AuditEvent, Config, Declined, Host, Pack, PackContent, PackError,
    Participant, ParticipantKind, Rights, SessionManager, Stay, View,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::table::{self, Axis, Shape, Table};

mod grid_pack;

// ---- the counting allocator (this test binary only) ----

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn bump() {
    let _ = ALLOCS.try_with(|a| a.set(a.get() + 1));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        bump();
        System.alloc(l)
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        bump();
        System.alloc_zeroed(l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        bump();
        System.realloc(p, l, n)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
}

#[global_allocator]
static A: Counting = Counting;

fn allocs() -> u64 {
    ALLOCS.with(Cell::get)
}

/// One call of `f`: (allocations, seconds). Its result is dropped after the count.
fn measure<T>(f: impl FnOnce() -> T) -> (u64, f64) {
    let a = allocs();
    let t = Instant::now();
    let out = f();
    let secs = t.elapsed().as_secs_f64();
    let n = allocs() - a;
    drop(out);
    (n, secs)
}

// ---- grids ----

fn k(s: String) -> Key {
    Key::new(s).unwrap()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grid {
    WideBlank,
    RaggedWide,
    Dense,
}

impl Grid {
    fn name(self) -> &'static str {
        match self {
            Grid::WideBlank => "wide_blank",
            Grid::RaggedWide => "ragged_wide",
            Grid::Dense => "dense",
        }
    }
}

fn build(g: Grid, rows: usize, cols: usize) -> Table {
    let (header, shape) = match g {
        Grid::WideBlank | Grid::Dense => (true, Shape::Rect),
        Grid::RaggedWide => (false, Shape::Ragged),
    };
    let mut t = Table::new(header, shape);
    let rk: Vec<Key> = (1..=rows).map(|i| k(format!("r{i}"))).collect();
    let ck: Vec<Key> = (1..=cols).map(|i| k(format!("c{i}"))).collect();
    if header {
        for (i, c) in ck.iter().enumerate() {
            t.names.insert(c.clone(), format!("n{i}"));
        }
    }
    match g {
        Grid::WideBlank => {}
        Grid::RaggedWide => {
            if let Some(r) = rk.first() {
                for c in &ck {
                    t.cells.insert((r.clone(), c.clone()), "v".to_owned());
                }
            }
        }
        Grid::Dense => {
            for r in &rk {
                for c in &ck {
                    t.cells.insert((r.clone(), c.clone()), "v".to_owned());
                }
            }
        }
    }
    t.rows = Axis::from_live(rk);
    t.cols = Axis::from_live(ck);
    t.next_row = rows as u64 + 1;
    t.next_col = cols as u64 + 1;
    t
}

fn stored(t: &Table) -> usize {
    t.cells.len() + t.rows.live_len() + t.cols.live_len() + t.names.len()
}

/// `t` with cell (row `r`, col `c`) set to `v` (1-based keys).
fn with_cell(t: &Table, r: usize, c: usize, v: &str) -> Table {
    let mut o = t.clone();
    o.cells
        .insert((k(format!("r{r}")), k(format!("c{c}"))), v.to_owned());
    o
}

/// A table's six roots as decoded content (no deleted elements, no quote exceptions).
fn roots_of(t: &Table) -> Vec<(String, Neutral)> {
    let list = |a: &Axis| Neutral::List(a.live().map(|k| (k.clone(), Value::Null)).collect());
    let cells = t
        .cells
        .iter()
        .map(|((r, c), v)| (table::cell_key(r, c), Value::str(v.clone())))
        .collect();
    let names = t
        .names
        .iter()
        .map(|(c, v)| (c.as_str().to_owned(), Value::str(v.clone())))
        .collect();
    let mut meta = BTreeMap::new();
    meta.insert(
        "header".to_owned(),
        Value::str(if t.header { "on" } else { "off" }),
    );
    meta.insert(
        "shape".to_owned(),
        Value::str(match t.shape {
            Shape::Rect => "rect",
            Shape::Ragged => "ragged",
        }),
    );
    meta.insert("next_row".to_owned(), Value::str(t.next_row.to_string()));
    meta.insert("next_col".to_owned(), Value::str(t.next_col.to_string()));
    vec![
        (table::ROWS.to_owned(), list(&t.rows)),
        (table::COLS.to_owned(), list(&t.cols)),
        (table::CELLS.to_owned(), Neutral::Map(cells)),
        (table::NAMES.to_owned(), Neutral::Map(names)),
        (table::QUOTING.to_owned(), Neutral::Map(BTreeMap::new())),
        (table::META.to_owned(), Neutral::Map(meta)),
    ]
}

/// A structured table pack whose decode hands back fixed content: the default reconcile's
/// cost without a codec's.
struct Fixed {
    content: Vec<(String, Neutral)>,
}

const FIXED: Schema = Schema {
    name: "probe-grid",
    roots: table::ROOTS,
};

impl Pack for Fixed {
    fn schema(&self) -> Schema {
        FIXED
    }
    fn decode(&self, _: &[u8]) -> Result<PackContent, Declined> {
        Ok(PackContent {
            roots: self.content.clone(),
            codec: vec![1],
        })
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
}

// ---- series ----

/// One measurement: (rows, columns, stored elements, allocations, seconds).
type Point = (usize, usize, usize, u64, f64);

type Series = (&'static str, Vec<(usize, usize)>);

fn series(g: Grid) -> Vec<Series> {
    match g {
        Grid::Dense => vec![
            ("rows", vec![(1_000, 10), (10_000, 10), (100_000, 10)]),
            ("cols", vec![(1_000, 10), (1_000, 100), (1_000, 1_000)]),
            ("diag", vec![(250, 16), (1_000, 64), (4_000, 256)]),
        ],
        _ => vec![
            (
                "rows",
                vec![(1_000, 64), (10_000, 64), (100_000, 64), (1_000_000, 64)],
            ),
            ("cols", vec![(10_000, 64), (10_000, 256), (10_000, 1_024)]),
            ("diag", vec![(1_000, 64), (4_000, 256), (16_000, 1_024)]),
        ],
    }
}

/// Least-squares slope of ln y over ln x.
fn slope(pts: &[(f64, f64)]) -> f64 {
    let pts: Vec<(f64, f64)> = pts
        .iter()
        .filter(|(x, y)| *x > 0.0 && *y > 0.0)
        .map(|(x, y)| (x.ln(), y.ln()))
        .collect();
    let n = pts.len() as f64;
    if pts.len() < 2 {
        return f64::NAN;
    }
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
    let (mx, my) = (sx / n, sy / n);
    let (num, den) = pts.iter().fold((0.0, 0.0), |(a, b), (x, y)| {
        (a + (x - mx) * (y - my), b + (x - mx) * (x - mx))
    });
    num / den
}

fn budget() -> f64 {
    std::env::var("PROBE_BUDGET_S")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30.0)
}

const PATHS: &[&str] = &[
    "render",
    "line_map",
    "table_reconcile_same",
    "table_reconcile_edit",
    "default_reconcile",
    "merge3",
    "merge3_text",
    "merge3_outside_text",
];

/// One path on one grid: (allocations, seconds).
fn run(path: &str, t: &Table, rows: usize, cols: usize) -> (u64, f64) {
    // one cell set in the first row, and another in the last (theirs)
    let ours = with_cell(t, 1, 1, "x");
    let theirs = with_cell(t, rows, cols, "y");
    match path {
        "render" => measure(|| t.render()),
        "line_map" => measure(|| t.line_map()),
        "table_reconcile_same" => {
            let recs = t.render();
            measure(|| table::reconcile(t, &recs).unwrap())
        }
        "table_reconcile_edit" => {
            let recs = ours.render();
            measure(|| table::reconcile(t, &recs).unwrap())
        }
        "default_reconcile" => {
            let pack = Fixed {
                content: roots_of(&ours),
            };
            let synced = View::of_content(&roots_of(t));
            measure(|| default_reconcile(&pack, &synced, b"").unwrap())
        }
        "merge3" => measure(|| table::merge3(t, &ours, &theirs).unwrap()),
        "merge3_text" | "merge3_outside_text" => {
            // every row exact but the one the text changed (what the aligner reports)
            let exact: HashSet<Key> = t.rows.live().skip(1).cloned().collect();
            if path == "merge3_text" {
                measure(|| table::merge3_text(t, &ours, &theirs, &exact).unwrap())
            } else {
                measure(|| table::merge3_outside_text(t, &ours, &theirs, &exact).unwrap())
            }
        }
        _ => unreachable!(),
    }
}

fn report(tag: &str, path: &str, g: Grid, s: &str, pts: &[Point], var_rows: bool) {
    let var: Vec<(f64, f64)> = pts
        .iter()
        .map(|&(r, c, _, a, _)| ((if var_rows { r } else { c }) as f64, a as f64))
        .collect();
    let st: Vec<(f64, f64)> = pts
        .iter()
        .map(|&(_, _, n, a, _)| (n as f64, a as f64))
        .collect();
    let tm: Vec<(f64, f64)> = pts.iter().map(|&(_, _, n, _, t)| (n as f64, t)).collect();
    let per = pts
        .last()
        .map_or(f64::NAN, |&(_, _, n, a, _)| a as f64 / n as f64);
    println!(
        "SLOPE {tag} {path} {} {s}: allocs exp in {} {:.2}, in stored {:.2}; time exp in stored {:.2}; allocs/stored at largest {per:.1}",
        g.name(),
        if var_rows { "rows" } else { "cols" },
        slope(&var),
        slope(&st),
        slope(&tm),
    );
}

#[test]
#[ignore]
fn probe_grid_scaling() {
    let budget = budget();
    for g in [Grid::WideBlank, Grid::RaggedWide, Grid::Dense] {
        for (s, sizes) in series(g) {
            let mut pts: BTreeMap<&str, Vec<Point>> = BTreeMap::new();
            let mut stopped: HashSet<&str> = HashSet::new();
            for &(rows, cols) in &sizes {
                let t = build(g, rows, cols);
                let n = stored(&t);
                for &p in PATHS {
                    if stopped.contains(p) {
                        println!(
                            "SKIP kit {p} {} {s} R={rows} C={cols} (a smaller size took over {budget}s)",
                            g.name()
                        );
                        continue;
                    }
                    let (a, secs) = run(p, &t, rows, cols);
                    println!(
                        "PROBE kit {p} {} {s} R={rows} C={cols} stored={n} allocs={a} secs={secs:.4}",
                        g.name()
                    );
                    pts.entry(p).or_default().push((rows, cols, n, a, secs));
                    if secs > budget {
                        stopped.insert(p);
                    }
                }
            }
            for (p, v) in &pts {
                report("kit", p, g, s, v, s == "rows");
            }
        }
    }
    if std::env::var("PROBE_WORST").as_deref() == Ok("1") {
        // a 2 MiB CSV file: a header of 1,024 one-letter names (2,048 bytes with its line
        // break), then one line break per blank row
        let (rows, cols) = (2 * 1024 * 1024 - 2_048, 1_024);
        let t = build(Grid::WideBlank, rows, cols);
        let n = stored(&t);
        for p in ["render", "line_map"] {
            let (a, secs) = run(p, &t, rows, cols);
            println!(
                "WORST kit {p} wide_blank R={rows} C={cols} stored={n} allocs={a} secs={secs:.2}"
            );
        }
    }
}

// ---- the regression ----

/// Allocations per stored element a table path may make on the regression's grids.
const TABLE_BOUND: u64 = 64;
/// The same for a session's grid blame (it also reads the history and builds its lines).
const BLAME_BOUND: u64 = 256;
/// The regression's grids: wide, and mostly blank rows.
const REG_ROWS: usize = 4_000;
const REG_COLS: usize = 1_024;

/// `MPC_GRID_COST_MUTANT` runs `f` with a grid cost fault planted on this thread
/// (`k2_mpc_base::differential::GridCostMutant`). Unset, `f` runs as is.
fn under_cost_mutant<R>(f: impl FnOnce() -> R) -> R {
    // one list of the faults this file plants (the second sweep's too)
    under_cost_mutant_2(f)
}

#[test]
fn grid_paths_are_linear_in_stored_size() {
    under_cost_mutant(|| {
        for g in [Grid::WideBlank, Grid::RaggedWide] {
            let t = build(g, REG_ROWS, REG_COLS);
            let n = stored(&t) as u64;
            for &p in PATHS {
                let (a, _) = run(p, &t, REG_ROWS, REG_COLS);
                println!("{p} {}: {a} allocations, {n} stored", g.name());
                assert!(
                    a <= TABLE_BOUND * n,
                    "{p} on {}: {a} allocations for {n} stored elements (bound {TABLE_BOUND} each)",
                    g.name()
                );
            }
        }
    });
}

#[test]
fn grid_blame_is_linear_in_stored_size() {
    under_cost_mutant(|| {
        let dir = std::env::temp_dir().join(format!("k2mb-gridreg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let t = build(Grid::WideBlank, REG_ROWS, REG_COLS);
        let n = stored(&t) as u64;
        let store = Arc::new(DocStore::open(dir.join("d.redb")).unwrap());
        let mgr = SessionManager::new(store, Arc::new(H(AtomicU64::new(1))), Config::default());
        let key = DocKey {
            space: "ws".into(),
            path: "g.csv".into(),
        };
        let bytes = file_of(&t);
        let sess = mgr
            .open(Arc::new(grid_pack::GridLite), &key, Some(bytes.as_slice()))
            .unwrap();
        let p = sess
            .join(
                Participant::new("p1", "p1", "#111111", ParticipantKind::Person, "cli"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        let _ = sess.blame(p, table::ROWS, None).unwrap();
        let (a, _) = measure(|| sess.blame(p, table::ROWS, None).unwrap());
        println!("grid_blame wide_blank: {a} allocations, {n} stored");
        drop(sess);
        drop(mgr);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            a <= BLAME_BOUND * n,
            "grid_blame on wide_blank: {a} allocations for {n} stored elements (bound {BLAME_BOUND} each)"
        );
    });
}

// ---- a session's grid blame ----

struct H(AtomicU64);
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

/// `grid-lite`'s file for a grid: the header line, then one line per row.
fn file_of(t: &Table) -> Vec<u8> {
    grid_pack::render(t).into_bytes()
}

#[test]
#[ignore]
fn probe_grid_blame_scaling() {
    let budget = budget();
    let dir = std::env::temp_dir().join(format!("k2mb-gridscale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut n_doc = 0;
    for g in [Grid::WideBlank, Grid::Dense] {
        let all = match g {
            Grid::Dense => vec![
                ("rows", vec![(250, 10), (1_000, 10), (2_500, 10)]),
                ("cols", vec![(100, 10), (100, 40), (100, 160)]),
            ],
            _ => vec![
                ("rows", vec![(1_000, 64), (10_000, 64), (100_000, 64)]),
                ("cols", vec![(10_000, 64), (10_000, 256), (10_000, 1_024)]),
                ("diag", vec![(1_000, 64), (4_000, 256), (16_000, 1_024)]),
            ],
        };
        for (s, sizes) in all {
            let mut pts = Vec::new();
            for &(rows, cols) in &sizes {
                let t = build(g, rows, cols);
                let n = stored(&t);
                n_doc += 1;
                let store = Arc::new(DocStore::open(dir.join(format!("d{n_doc}.redb"))).unwrap());
                let mut c = Config::default();
                c.edit_burst = 1_000_000;
                c.edit_rate_per_s = 1_000_000;
                let mgr = SessionManager::new(store, Arc::new(H(AtomicU64::new(1))), c);
                let key = DocKey {
                    space: "ws".into(),
                    path: format!("g{n_doc}.csv"),
                };
                let bytes = file_of(&t);
                let s0 = Instant::now();
                let sess = mgr
                    .open(Arc::new(grid_pack::GridLite), &key, Some(bytes.as_slice()))
                    .unwrap();
                let open_s = s0.elapsed().as_secs_f64();
                let p = sess
                    .join(
                        Participant::new("p1", "p1", "#111111", ParticipantKind::Person, "cli"),
                        Stay::Held,
                    )
                    .unwrap()
                    .conn;
                // the first blame warms the session's caches; the second is measured
                let _ = sess.blame(p, table::ROWS, None).unwrap();
                let (a, secs) = measure(|| sess.blame(p, table::ROWS, None).unwrap());
                println!(
                    "PROBE kit grid_blame {} {s} R={rows} C={cols} stored={n} allocs={a} secs={secs:.4} (open {open_s:.1}s)",
                    g.name()
                );
                pts.push((rows, cols, n, a, secs));
                drop(sess);
                drop(mgr);
                if secs > budget {
                    break;
                }
            }
            report("kit", "grid_blame", g, s, &pts, s == "rows");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- the second sweep: an axis's places, a merge's conflict listing, `row list` ----
//
// Each path's cost is also counted in elements visited (`differential::visits`: an axis's place
// lookups and shifts, a position walk, a line map's scan), as allocations don't show a search.
// Planted faults (`MPC_GRID_COST_MUTANT`): `axis-walk` (a place found by a walk from the
// first, as the axis's vector was), `conflict-walk` (the conflict listing walks the axis, the
// line map and the live columns per conflict), `row-list-walk` (`row list` walks the line map
// per row and builds each field as its own string): each caught by its test below.

/// Visits (and, where shown, allocations) per stored element (or op) the second sweep's paths
/// may make.
const VISIT_BOUND: u64 = 32;

/// `MPC_GRID_COST_MUTANT` for the second sweep's faults (and the first's), as
/// [`under_cost_mutant`].
fn under_cost_mutant_2<R>(f: impl FnOnce() -> R) -> R {
    use k2_mpc_base::differential::{with_grid_cost_mutant, GridCostMutant};
    let m = std::env::var("MPC_GRID_COST_MUTANT");
    println!("MPC_GRID_COST_MUTANT={}", m.as_deref().unwrap_or("(unset)"));
    match m.as_deref() {
        Ok("per-column") => with_grid_cost_mutant(GridCostMutant::PerColumn, f),
        Ok("axis-walk") => with_grid_cost_mutant(GridCostMutant::AxisWalk, f),
        Ok("conflict-walk") => with_grid_cost_mutant(GridCostMutant::ConflictWalk, f),
        Ok("row-list-walk") => with_grid_cost_mutant(GridCostMutant::RowListWalk, f),
        Ok("rekey-walk") => with_grid_cost_mutant(GridCostMutant::RekeyWalk, f),
        Ok(other) => unreachable!("MPC_GRID_COST_MUTANT={other}: not a mutant this test plants"),
        Err(_) => f(),
    }
}

/// One call of `f`: (allocations, visits, seconds).
fn measure_v<T>(f: impl FnOnce() -> T) -> (u64, u64, f64) {
    let v = k2_mpc_base::differential::visits();
    let (a, secs) = measure(f);
    (a, k2_mpc_base::differential::visits() - v, secs)
}

fn key(s: String) -> Key {
    k(s)
}

/// One change inserting `n` rows after `r1`, each after the one before (a paste in the middle
/// of the table), keys from `r<from>`.
fn inserts(n: usize, from: usize) -> Vec<k2_mpc_base::Edit> {
    use k2_mpc_base::elem::{Anchor, ListOp};
    let mut ops = Vec::with_capacity(n);
    let mut prev = key("r1".into());
    for i in 0..n {
        let r = key(format!("r{}", from + i));
        ops.push(ListOp::Insert {
            key: r.clone(),
            at: Anchor::After(prev),
            value: Value::Null,
        });
        prev = r;
    }
    vec![k2_mpc_base::Edit::List {
        root: table::ROWS.into(),
        ops,
    }]
}

/// One change moving every row to the start, first to last (the order reversed).
fn moves(rows: usize) -> Vec<k2_mpc_base::Edit> {
    use k2_mpc_base::elem::{Anchor, ListOp};
    let ops = (1..=rows)
        .map(|i| ListOp::Move {
            key: key(format!("r{i}")),
            to: Anchor::Start,
        })
        .collect();
    vec![k2_mpc_base::Edit::List {
        root: table::ROWS.into(),
        ops,
    }]
}

/// One path of the second sweep on a `rows` × `cols` grid: (allocations, visits, seconds,
/// stored elements plus ops).
fn run_2(path: &str, rows: usize, cols: usize) -> (u64, u64, f64, usize) {
    match path {
        "axis_insert" => {
            let mut t = build(Grid::Dense, rows, cols);
            let e = inserts(rows, rows + 1);
            let n = stored(&t) + rows;
            let (a, v, s) = measure_v(|| t.apply(&e).unwrap());
            (a, v, s, n)
        }
        "axis_move" => {
            let mut t = build(Grid::Dense, rows, cols);
            let e = moves(rows);
            let n = stored(&t) + rows;
            let (a, v, s) = measure_v(|| t.apply(&e).unwrap());
            (a, v, s, n)
        }
        "table_reconcile_insert" => {
            // an outside save that pastes as many rows again after the first
            let t = build(Grid::Dense, rows, cols);
            let mut g = t.clone();
            g.apply(&inserts(rows, rows + 1)).unwrap();
            for i in rows + 1..=2 * rows {
                for c in 1..=cols {
                    g.cells.insert(
                        (key(format!("r{i}")), key(format!("c{c}"))),
                        format!("w{i}"),
                    );
                }
            }
            let recs = g.render();
            let n = stored(&g);
            let (a, v, s) = measure_v(|| table::reconcile(&t, &recs).unwrap());
            (a, v, s, n)
        }
        "row_list" => {
            let t = build(Grid::WideBlank, rows, cols);
            let map = t.line_map();
            let live = t.live_cols();
            let n = stored(&t);
            let (a, v, s) = measure_v(|| table::verbs::row_listing(&t, &map, &live, None, None));
            (a, v, s, n)
        }
        "conflict_listing" => conflict_listing(rows, cols),
        _ => unreachable!(),
    }
}

/// A `write --base` whose text changed every row's last cell after a person changed every
/// row's first: one `edited_row` conflict per row, each listed with its context. (allocations,
/// visits, seconds, stored elements) of the write.
fn conflict_listing(rows: usize, cols: usize) -> (u64, u64, f64, usize) {
    use k2_mpc_base::elem::MapOp;
    use k2_mpc_base::session::{EditOptions, GridWrite, GridWriteOptions};
    let t = build(Grid::Dense, rows, cols);
    let n = stored(&t);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-gridconf-{}-{rows}-{cols}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("d.redb")).unwrap());
    let mut c = Config::default();
    c.edit_burst = 1_000_000;
    c.edit_rate_per_s = 1_000_000;
    c.grid_max_cells = 100_000_000;
    let mgr = SessionManager::new(store, Arc::new(H(AtomicU64::new(1))), c);
    let dk = DocKey {
        space: "ws".into(),
        path: "c.csv".into(),
    };
    let bytes = file_of(&t);
    let sess = mgr
        .open(Arc::new(grid_pack::GridLite), &dk, Some(bytes.as_slice()))
        .unwrap();
    let join = |pid: &str, kind| {
        sess.join(
            Participant::new(pid, pid, "#111111", kind, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    };
    let (p, a) = (
        join("p", ParticipantKind::Person),
        join("a", ParticipantKind::Agent),
    );
    let base = sess.read(a).unwrap().version;
    let ops = (1..=rows)
        .map(|r| MapOp::Set {
            key: table::cell_key(&key(format!("r{r}")), &key("c1".into())),
            value: Value::str("p"),
        })
        .collect();
    sess.edit_ops(
        p,
        vec![k2_mpc_base::Edit::Map {
            root: table::CELLS.into(),
            ops,
        }],
        &EditOptions::new(),
    )
    .unwrap();
    let mut ours = t.clone();
    for r in 1..=rows {
        ours.cells
            .insert((key(format!("r{r}")), key(format!("c{cols}"))), "a".into());
    }
    let body = file_of(&ours);
    let mut listed = 0;
    let (al, v, s) = measure_v(|| {
        let r = sess
            .write_base_grid(a, base, &body, &GridWriteOptions::default())
            .unwrap();
        if let GridWrite::Conflicts(cs) = &r.outcome {
            listed = cs.len();
        }
    });
    assert_eq!(listed, rows, "one conflict per row");
    drop(sess);
    drop(mgr);
    let _ = std::fs::remove_dir_all(&dir);
    (al, v, s, n)
}

#[test]
fn axis_inserts_and_moves_are_linear() {
    under_cost_mutant_2(|| {
        for p in ["axis_insert", "axis_move"] {
            let (a, v, _, n) = run_2(p, 4_000, 4);
            println!("{p}: {v} visits, {a} allocations, {n} stored and ops");
            assert!(
                v <= VISIT_BOUND * n as u64,
                "{p}: {v} visits for {n} stored elements and ops (bound {VISIT_BOUND} each)"
            );
        }
    });
}

#[test]
fn conflict_listing_is_linear() {
    under_cost_mutant_2(|| {
        let (a, v, _, n) = run_2("conflict_listing", 1_000, 4);
        println!("conflict_listing: {v} visits, {a} allocations, {n} stored");
        assert!(
            v <= VISIT_BOUND * n as u64,
            "conflict_listing: {v} visits for {n} stored elements (bound {VISIT_BOUND} each)"
        );
    });
}

#[test]
fn row_list_is_linear() {
    under_cost_mutant_2(|| {
        let (a, v, _, n) = run_2("row_list", REG_ROWS, REG_COLS);
        println!("row_list: {v} visits, {a} allocations, {n} stored");
        assert!(
            a <= TABLE_BOUND * n as u64,
            "row_list: {a} allocations for {n} stored elements (bound {TABLE_BOUND} each)"
        );
        assert!(
            v <= VISIT_BOUND * n as u64,
            "row_list: {v} visits for {n} stored elements (bound {VISIT_BOUND} each)"
        );
    });
}

#[test]
#[ignore]
fn probe_grid_scaling_2() {
    let budget = budget();
    let all: [(&str, Vec<Series>); 5] = [
        (
            "axis_insert",
            vec![(
                "rows",
                vec![(1_000, 4), (4_000, 4), (16_000, 4), (64_000, 4)],
            )],
        ),
        (
            "axis_move",
            vec![(
                "rows",
                vec![(1_000, 4), (4_000, 4), (16_000, 4), (64_000, 4)],
            )],
        ),
        (
            "table_reconcile_insert",
            vec![(
                "rows",
                vec![(1_000, 4), (4_000, 4), (16_000, 4), (64_000, 4)],
            )],
        ),
        (
            "row_list",
            vec![
                (
                    "rows",
                    vec![(1_000, 64), (4_000, 64), (16_000, 64), (64_000, 64)],
                ),
                ("diag", vec![(1_000, 64), (4_000, 256), (16_000, 1_024)]),
            ],
        ),
        (
            "conflict_listing",
            vec![("rows", vec![(250, 4), (1_000, 4), (4_000, 4), (16_000, 4)])],
        ),
    ];
    for (p, series) in all {
        for (s, sizes) in series {
            let mut pts: Vec<(f64, f64, f64)> = Vec::new();
            for (rows, cols) in sizes {
                let (a, v, secs, n) = run_2(p, rows, cols);
                println!(
                    "PROBE2 kit {p} {s} R={rows} C={cols} stored={n} visits={v} allocs={a} secs={secs:.4}"
                );
                pts.push((n as f64, v as f64, a as f64));
                if secs > budget {
                    println!("SKIP2 kit {p} {s}: past R={rows} (over {budget}s)");
                    break;
                }
            }
            let ex = |f: &dyn Fn(&(f64, f64, f64)) -> f64| {
                slope(&pts.iter().map(|x| (x.0, f(x))).collect::<Vec<_>>())
            };
            println!(
                "SLOPE2 kit {p} {s}: visits exp in stored {:.2}, allocs exp in stored {:.2}",
                ex(&|x| x.1),
                ex(&|x| x.2)
            );
        }
    }
}

// ---- the rekey of elements the live side dropped ----

/// The owner, on a host that gives every participant owner rights.
struct Owner(AtomicU64);
impl Host for Owner {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

/// A live write from an older version, after the owner deleted every row but the first and
/// dropped them (`rows drop-deleted`): the text still holds them, so each is checked for
/// changes before the merge. (allocations, visits, seconds, stored elements) of the write.
fn rekey_after_drop(rows: usize, cols: usize) -> (u64, u64, f64, usize) {
    use k2_mpc_base::elem::ListOp;
    use k2_mpc_base::session::{EditOptions, GridWrite, GridWriteOptions};
    let t = build(Grid::Dense, rows, cols);
    let n = stored(&t);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-gridrekey-{}-{rows}-{cols}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("d.redb")).unwrap());
    let mut c = Config::default();
    c.edit_burst = 1_000_000;
    c.edit_rate_per_s = 1_000_000;
    c.grid_max_cells = 100_000_000;
    let mgr = SessionManager::new(store, Arc::new(Owner(AtomicU64::new(1))), c);
    let dk = DocKey {
        space: "ws".into(),
        path: "k.csv".into(),
    };
    let bytes = file_of(&t);
    let sess = mgr
        .open(Arc::new(grid_pack::GridLite), &dk, Some(bytes.as_slice()))
        .unwrap();
    let p = sess
        .join(
            Participant::new("p", "p", "#111111", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let seen = sess.read(p).unwrap().version;
    let ops = (2..=rows)
        .map(|r| ListOp::Delete {
            key: key(format!("r{r}")),
        })
        .collect();
    sess.edit_ops(
        p,
        vec![k2_mpc_base::Edit::List {
            root: table::ROWS.into(),
            ops,
        }],
        &EditOptions::new(),
    )
    .unwrap();
    sess.drop_deleted(p).unwrap();
    // the text from `seen`, its first row's first cell changed
    let body = file_of(&with_cell(&t, 1, 1, "x"));
    let (a, v, s) = measure_v(|| {
        let r = sess
            .write_live_grid(p, seen, &body, &GridWriteOptions::default())
            .unwrap();
        assert!(matches!(r.outcome, GridWrite::Applied(_)), "{r:?}");
    });
    drop(sess);
    drop(mgr);
    let _ = std::fs::remove_dir_all(&dir);
    (a, v, s, n)
}

#[test]
fn rekey_of_dropped_rows_is_linear() {
    under_cost_mutant_2(|| {
        let (a, v, _, n) = rekey_after_drop(1_000, 4);
        println!("rekey_after_drop: {v} visits, {a} allocations, {n} stored");
        assert!(
            v <= VISIT_BOUND * n as u64,
            "rekey_after_drop: {v} visits for {n} stored elements (bound {VISIT_BOUND} each)"
        );
    });
}

#[test]
#[ignore]
fn probe_rekey_scaling() {
    let budget = budget();
    let mut pts: Vec<(f64, f64, f64)> = Vec::new();
    for rows in [250, 1_000, 4_000, 16_000] {
        let (a, v, secs, n) = rekey_after_drop(rows, 4);
        println!(
            "PROBE3 kit rekey_after_drop rows R={rows} C=4 stored={n} visits={v} allocs={a} secs={secs:.4}"
        );
        pts.push((n as f64, v as f64, a as f64));
        if secs > budget {
            println!("SKIP3 kit rekey_after_drop: past R={rows} (over {budget}s)");
            break;
        }
    }
    let ex = |f: &dyn Fn(&(f64, f64, f64)) -> f64| {
        slope(&pts.iter().map(|x| (x.0, f(x))).collect::<Vec<_>>())
    };
    println!(
        "SLOPE3 kit rekey_after_drop rows: visits exp in stored {:.2}, allocs exp in stored {:.2}",
        ex(&|x| x.1),
        ex(&|x| x.2)
    );
}
