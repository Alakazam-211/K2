//! K2SO performance benchmarks.
//!
//! Each group compares two or more approaches to the same hot path so we
//! have apples-to-apples numbers driving the P1/P2 code changes. Run with:
//!
//! ```text
//! cd src-tauri && cargo bench
//! ```
//!
//! Criterion saves the previous run's results under `target/criterion/` and
//! prints deltas on subsequent invocations, so the workflow is:
//!
//!   1. `cargo bench`  ← establishes baseline
//!   2. land P1/P2 code changes
//!   3. `cargo bench`  ← reports improvement/regression per metric
//!
//! The printed + HTML outputs become the source of truth for the release
//! notes table.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use k2_lib::terminal::grid_types::{CompactLine, GridUpdate, StyleSpan};
use k2_lib::terminal::reflow;
use std::hash::{Hash, Hasher};

// ── Fixtures ──────────────────────────────────────────────────────────────

/// Build a realistic grid: `cols` wide, `rows` tall, with a mix of plain
/// and styled lines. Style density mirrors what we see in Claude Code
/// output (~20% of lines have color spans).
fn make_grid(cols: u16, rows: u16) -> GridUpdate {
    let mut lines = Vec::with_capacity(rows as usize);
    for row in 0..rows {
        // Text: rolling alphanumeric pattern that fills most of the line.
        let text_len = (cols.saturating_sub(2)) as usize;
        let text: String = (0..text_len)
            .map(|i| {
                let c = ((row as usize + i) % 62) as u8;
                match c {
                    0..=9 => (b'0' + c) as char,
                    10..=35 => (b'a' + (c - 10)) as char,
                    _ => (b'A' + (c - 36)) as char,
                }
            })
            .collect();
        // Every 5th line gets a style span, mimicking prompt/output contrast.
        let spans = if row % 5 == 0 {
            vec![
                StyleSpan { s: 0, e: 10, fg: Some(14120791), bg: None, fl: None },
                StyleSpan { s: 12, e: 40, fg: Some(10066329), bg: None, fl: None },
            ]
        } else {
            Vec::new()
        };
        lines.push(CompactLine {
            row,
            text,
            spans,
            wrapped: false,
        });
    }
    GridUpdate {
        cols,
        rows,
        cursor_col: cols / 2,
        cursor_row: rows - 1,
        cursor_visible: true,
        cursor_shape: "bar".to_string(),
        lines,
        full: true,
        mode: 0,
        display_offset: 0,
        selection: None,
        perf: None,
        seqno: 1,
    }
}

/// Mutate one cell's text to force a hash mismatch while preserving
/// structure — models a typical "user typed a character" delta.
fn mutate_grid(grid: &mut GridUpdate) {
    if let Some(line) = grid.lines.last_mut() {
        if !line.text.is_empty() {
            // Flip the last character through the alphabet.
            let mut chars: Vec<char> = line.text.chars().collect();
            if let Some(last) = chars.last_mut() {
                *last = if *last == 'Z' { 'a' } else { (*last as u8 + 1) as char };
            }
            line.text = chars.into_iter().collect();
        }
    }
    grid.cursor_col = (grid.cursor_col + 1) % grid.cols;
}

// ── Bench 1: grid change detection ────────────────────────────────────────
//
// Three approaches to "has this grid changed since last broadcast?":
//
//   a) siphash      — current prod code (DefaultHasher = SipHash-2-4)
//   b) ahash        — P1.1 target (drop-in, faster, non-cryptographic)
//   c) seqno_u64    — P2.1 target (per-line monotonic counter, integer ==)
//
// Seqno isn't a hash at all — it's a counter. The bench proves: once we
// track dirty state with seqnos, change detection is ~free.

fn bench_grid_change_detection(c: &mut Criterion) {
    let mut group = c.benchmark_group("grid_change_detection");

    for &(cols, rows) in &[(80u16, 24u16), (120, 40), (200, 60)] {
        let grid = make_grid(cols, rows);
        let cell_count = (cols as u64) * (rows as u64);
        group.throughput(Throughput::Elements(cell_count));
        let size_label = format!("{}x{}", cols, rows);

        group.bench_with_input(
            BenchmarkId::new("siphash", &size_label),
            &grid,
            |b, grid| {
                b.iter(|| {
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    for line in &grid.lines {
                        line.text.hash(&mut hasher);
                        line.row.hash(&mut hasher);
                    }
                    grid.cursor_col.hash(&mut hasher);
                    grid.cursor_row.hash(&mut hasher);
                    black_box(hasher.finish())
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("ahash", &size_label),
            &grid,
            |b, grid| {
                b.iter(|| {
                    let mut hasher = ahash::AHasher::default();
                    for line in &grid.lines {
                        line.text.hash(&mut hasher);
                        line.row.hash(&mut hasher);
                    }
                    grid.cursor_col.hash(&mut hasher);
                    grid.cursor_row.hash(&mut hasher);
                    black_box(hasher.finish())
                });
            },
        );

        // Simulated seqno approach: assume each line has a seqno field; the
        // entire check is two u64 comparisons (current max seqno vs last
        // broadcast seqno). No iteration over lines.
        group.bench_with_input(
            BenchmarkId::new("seqno_compare", &size_label),
            &grid,
            |b, _grid| {
                let current: u64 = 42_000;
                let last: u64 = 41_999;
                b.iter(|| {
                    // The hot path: "did any line change since last broadcast?"
                    // becomes a single u64 compare, not a grid-wide hash.
                    black_box(current != last)
                });
            },
        );
    }

    group.finish();
}

// ── Bench 2: reflow cache ────────────────────────────────────────────────
//
// P2.3 caches the reflowed grid by `(desktop_seqno, mobile_cols, mobile_rows)`.
// This bench shows the raw `reflow_grid` cost (which runs per-client per-tick
// today) vs a cache-hit path (one HashMap lookup).

fn bench_reflow(c: &mut Criterion) {
    let mut group = c.benchmark_group("reflow");

    // Typical desktop → mobile case: 120×40 desktop, 60×100 mobile
    let desktop = make_grid(120, 40);

    group.bench_function("uncached_reflow", |b| {
        b.iter(|| {
            let reflowed = reflow::reflow_grid(black_box(&desktop), 60, 100);
            black_box(reflowed);
        });
    });

    // Simulate the cache path: a precomputed reflowed grid, one Arc clone.
    // That's what a cache hit reduces to — the actual cache data structure
    // is a HashMap which has similar lookup cost, so Arc::clone is a fair
    // lower bound.
    let precomputed = std::sync::Arc::new(reflow::reflow_grid(&desktop, 60, 100));
    group.bench_function("cached_reflow_hit", |b| {
        b.iter(|| {
            let hit = std::sync::Arc::clone(black_box(&precomputed));
            black_box(hit);
        });
    });

    group.finish();
}

// ── Bench 3: file walker ─────────────────────────────────────────────────
//
// Serial `fs::read_dir` recursion (current `search_walk`) vs the `ignore`
// crate's work-stealing parallel walker (P2.4 target).
//
// We walk the K2SO source tree itself as a realistic corpus — ~thousands of
// files across nested directories, including .git, node_modules-class
// exclusions, etc.

fn bench_file_walker(c: &mut Criterion) {
    let mut group = c.benchmark_group("file_walker");
    group.sample_size(20); // directory walk is slow, fewer samples

    // Walk from the K2SO src-tauri root so we have a realistic tree.
    // Project root inferred from CARGO_MANIFEST_DIR (set by cargo).
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let walk_root = std::path::PathBuf::from(manifest_dir);

    group.bench_function("serial_recursive", |b| {
        b.iter(|| {
            let mut count = 0usize;
            walk_serial(&walk_root, &mut count, 0);
            black_box(count);
        });
    });

    group.bench_function("parallel_ignore", |b| {
        b.iter(|| {
            let count = std::sync::atomic::AtomicUsize::new(0);
            ignore::WalkBuilder::new(&walk_root)
                .hidden(false)
                .build_parallel()
                .run(|| {
                    let count = &count;
                    Box::new(move |entry| {
                        if let Ok(entry) = entry {
                            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                        ignore::WalkState::Continue
                    })
                });
            black_box(count.load(std::sync::atomic::Ordering::Relaxed));
        });
    });

    group.finish();
}

fn walk_serial(path: &std::path::Path, count: &mut usize, depth: u32) {
    if depth > 10 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return; };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip .git and target — matches what search_walk does with its
            // exclusion set, for a fair comparison.
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == ".git" || name == "target" || name == "node_modules" {
                continue;
            }
            walk_serial(&path, count, depth + 1);
        } else {
            *count += 1;
        }
    }
}

// ── Bench 4: SQLite hot queries ──────────────────────────────────────────
//
// P1.3 target: rusqlite `prepare_cached` vs rebuilding the statement on
// every call. Using an in-memory DB so the bench measures the preparation
// cost, not I/O.

fn bench_sqlite_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("sqlite_insert");

    group.bench_function("execute_per_call", |b| {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS perf_test (id TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
        let mut i = 0u64;
        b.iter(|| {
            i += 1;
            let id = format!("row-{}", i);
            conn.execute(
                "INSERT OR REPLACE INTO perf_test (id, value) VALUES (?1, ?2)",
                rusqlite::params![id, "payload"],
            )
            .unwrap();
        });
    });

    group.bench_function("prepare_cached", |b| {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS perf_test (id TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
        let mut i = 0u64;
        b.iter(|| {
            i += 1;
            let id = format!("row-{}", i);
            let mut stmt = conn
                .prepare_cached(
                    "INSERT OR REPLACE INTO perf_test (id, value) VALUES (?1, ?2)",
                )
                .unwrap();
            stmt.execute(rusqlite::params![id, "payload"]).unwrap();
        });
    });

    group.finish();
}

// ── Bench 5: grid change detection over many polls ───────────────────────
//
// Models the actual 10fps poll loop: measure 100 consecutive polls on a
// grid where every poll mutates one cell. The siphash version pays the
// full grid-hash cost on every tick; the seqno version pays ~nothing.

fn bench_poll_simulation(c: &mut Criterion) {
    let mut group = c.benchmark_group("poll_simulation");
    group.sample_size(30);

    let base_grid = make_grid(120, 40);

    group.bench_function("siphash_100_polls", |b| {
        b.iter_batched(
            || base_grid.clone(),
            |mut grid| {
                let mut last_hash: u64 = 0;
                let mut dirty_count = 0usize;
                for _ in 0..100 {
                    mutate_grid(&mut grid);
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    for line in &grid.lines {
                        line.text.hash(&mut hasher);
                        line.row.hash(&mut hasher);
                    }
                    grid.cursor_col.hash(&mut hasher);
                    grid.cursor_row.hash(&mut hasher);
                    let h = hasher.finish();
                    if h != last_hash {
                        dirty_count += 1;
                        last_hash = h;
                    }
                }
                black_box(dirty_count);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function("ahash_100_polls", |b| {
        b.iter_batched(
            || base_grid.clone(),
            |mut grid| {
                let mut last_hash: u64 = 0;
                let mut dirty_count = 0usize;
                for _ in 0..100 {
                    mutate_grid(&mut grid);
                    let mut hasher = ahash::AHasher::default();
                    for line in &grid.lines {
                        line.text.hash(&mut hasher);
                        line.row.hash(&mut hasher);
                    }
                    grid.cursor_col.hash(&mut hasher);
                    grid.cursor_row.hash(&mut hasher);
                    let h = hasher.finish();
                    if h != last_hash {
                        dirty_count += 1;
                        last_hash = h;
                    }
                }
                black_box(dirty_count);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function("seqno_100_polls", |b| {
        b.iter_batched(
            || 0u64,
            |mut current_seqno| {
                let mut last_seqno: u64 = 0;
                let mut dirty_count = 0usize;
                for _ in 0..100 {
                    // Simulated mutation: bump seqno.
                    current_seqno += 1;
                    if current_seqno != last_seqno {
                        dirty_count += 1;
                        last_seqno = current_seqno;
                    }
                }
                black_box(dirty_count);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_grid_change_detection,
    bench_reflow,
    bench_file_walker,
    bench_sqlite_insert,
    bench_poll_simulation,
);
criterion_main!(benches);
