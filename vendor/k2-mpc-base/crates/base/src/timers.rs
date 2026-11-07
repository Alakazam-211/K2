//! Production timers (v0.10.1): per-path call counts, total and largest time, and a
//! log-linear histogram for p50/p95/p99, in every build of the kit (no feature needed).
//!
//! **Off by default.** Switched on and off at run time with [`set_enabled`]. While off, a
//! timed path costs one relaxed atomic load and a branch: no clock read, no allocation, no
//! store. While on, it costs two clock reads ([`Instant::now`]) and four relaxed atomic
//! adds. [`calibrate`] measures both on the machine it runs on.
//!
//! **Process-wide.** One registry per process, shared by every [`crate::SessionManager`],
//! store and doc in it (a host runs one manager; a process with several sees their sum).
//!
//! **What is timed** is a fixed list of paths ([`Path`]), placed at the kit's seam (never
//! inside the engine), so every engine is measured at the same points: `docs/SESSIONS.md`,
//! "Timers", gives each path's place and the requirement it serves.
//!
//! **Reading:**
//! - [`read`]: one path's `(count, total ns)`, without allocating. A single-threaded caller
//!   reads it before and after one call, and the difference is that call's exact time on
//!   the path (how a harness subtracts the durable commit from an edit).
//! - [`snapshot`]: every path's count, total, largest, p50/p95/p99 and buckets.
//! - [`reset`]: everything back to zero (not atomic against calls in flight).
//!
//! Percentiles come from the histogram: 16 buckets per power of two, so a bucket is at most
//! 1/16 (6.25%) of its lower bound wide, and a percentile (the bucket's midpoint) is within
//! about 3% of the true value; below 16 ns every nanosecond has its own bucket. Times of
//! 2^36 ns (about 69 s) and more share the last bucket (reported as the largest seen).
//! About 4.2 KiB per path.
//!
//! The test-only `prof!` scopes (`differential::profile_take`) are a separate, finer
//! developer profile; they stay as they were.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Instant;

/// Whether this build has the kit's test-only hooks compiled in (the `differential` or
/// `conformance` feature). A plain host build says `false`. Records quote it, so a figure
/// taken with the hooks on is marked as such.
pub const HOOKS: bool = cfg!(any(feature = "differential", feature = "conformance"));

macro_rules! paths {
    ($($(#[$doc:meta])* $v:ident = $name:literal,)*) => {
        /// A timed path. The list grows; a path's name never changes once released.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[non_exhaustive]
        pub enum Path {
            $($(#[$doc])* $v,)*
        }

        impl Path {
            /// Every path, in order.
            pub const ALL: &'static [Path] = &[$(Path::$v,)*];

            /// The path's stable name (dotted: `area.part`).
            pub fn name(self) -> &'static str {
                match self {
                    $(Path::$v => $name,)*
                }
            }

            /// The path named `name`, if there is one.
            pub fn from_name(name: &str) -> Option<Path> {
                Path::ALL.iter().copied().find(|p| p.name() == name)
            }
        }
    };
}

paths! {
    /// `Doc::edit_as` / `Doc::edit_applied` / the session's `Doc::edit_made` (the shared
    /// body): one change applied to an engine doc (the session's, a disk replica's or a bare
    /// doc's). E-PERF-30.
    EditAs = "edit_as",
    /// A session's commit of a change: the store, then the bookkeeping (every mutating verb's
    /// shared tail).
    Commit = "commit",
    /// The store's durable transaction commit alone (the time E-PERF-29 leaves out of a
    /// ratio).
    StoreTxnCommit = "store.txn_commit",
    /// `Session::blame` (a line range or the whole doc). E-PERF-31.
    Blame = "blame",
    /// Undo's and revert's provenance lookups, per region planned (`undo::plan`): the
    /// window's roots (`Prov::roots`) and the region's revert, which asks which runs' roots
    /// are live (`Prov::any_live`). Timed as one block: a lookup alone (about 160 ns on a
    /// release build) costs about what a running timer does. E-PERF-32.
    ProvLookup = "prov.lookup",
    /// Rebuilding the provenance map from scratch (the cold path, O(doc)).
    ProvRebuild = "prov.rebuild",
    /// A disk write-back (render, replica, file). E-PERF-33.
    WriteBack = "write_back",
    /// A write-back's atomic file write alone.
    WriteBackFile = "write_back.file",
    /// `Session::write_base` / `write_base_with` on a text root.
    WriteBase = "write_base",
    /// `write --base`: the text at the base version (the Yrs stand-in for `changed_since`,
    /// E-PERF-34).
    WriteBaseBaseText = "write_base.base_text",
    /// `write --base`: the three-way merge.
    WriteBaseMerge = "write_base.merge",
    /// `Session::load`: a stored doc loaded into a session. E-PERF-36, -47.
    Load = "load",
    /// A load's store read (snapshot and log).
    LoadStoreRead = "load.store_read",
    /// A load's engine doc built from the store's bytes (`Doc::from_store`). E-PERF-36.
    LoadDoc = "load.doc",
    /// `Session::join` / `join_shared` / `join_replacing`. E-PERF-47.
    Join = "join",
    /// An outside save merged in (text roots). E-PERF-41.
    Ingest = "ingest",
    /// An outside save: the diff of the save against the synced text, per root.
    IngestDiff = "ingest.diff",
    /// An outside save: the three-way merge with the live text, per root.
    IngestMerge = "ingest.merge",
    /// An outside save: the merge trimmed and made canonical, per root.
    IngestCanon = "ingest.canon",
    /// `SessionManager::tick`. E-PERF-42.
    Tick = "tick",
    /// A tick's history work, per loaded session.
    TickHistory = "tick.history",
    /// A tick's disk work (stat backup, due write-back), per loaded session.
    TickDisk = "tick.disk",
    /// Undo and revert planning (`undo::plan`, text roots). E-PERF-43.
    UndoPlan = "undo.plan",
    /// `Session::undo_with` / `redo_with` (also `undo` / `redo`).
    Undo = "undo",
    /// `Session::revert`.
    Revert = "revert",
    /// Notes delivered to a member (by `take_notes` or riding on any verb's reply).
    /// E-PERF-44.
    NotesDeliver = "notes.deliver",
    /// `Session::take_notes`.
    Notes = "notes",
    /// `Session::events_since` (the Live editor's catch-up). E-PERF-46.
    EventsSince = "events_since",
    /// `events_since`: the window read from the store.
    EventsSinceWindow = "events_since.window",
    /// `write --base` on a grid. E-PERF-45.
    GridWriteBase = "grid.write_base",
    /// An outside save merged into a grid (reconcile). E-PERF-45.
    GridIngest = "grid.ingest",
    /// Undo planning on a grid, per step.
    GridUndoPlan = "grid.undo.plan",
}

/// Number of paths.
const N: usize = Path::ALL.len();
/// Sub-buckets per power of two (2^SUB_BITS).
const SUB_BITS: u32 = 4;
const SUB: u64 = 1 << SUB_BITS;
/// The largest power of two with its own buckets: values of 2^(MAX_E + 1) ns and more go to
/// the last bucket.
const MAX_E: u32 = 35;
/// Exact buckets (0..SUB), then SUB per power of two from 2^SUB_BITS to 2^MAX_E, then one
/// last bucket.
const BUCKETS: usize = SUB as usize + (MAX_E - SUB_BITS + 1) as usize * SUB as usize + 1;
const LAST_BUCKET: usize = BUCKETS - 1;

/// The bucket a time of `ns` goes to.
fn bucket_of(ns: u64) -> usize {
    if ns < SUB {
        return ns as usize;
    }
    let e = 63 - ns.leading_zeros();
    if e > MAX_E {
        return LAST_BUCKET;
    }
    let sub = (ns >> (e - SUB_BITS)) & (SUB - 1);
    (SUB + u64::from(e - SUB_BITS) * SUB + sub) as usize
}

/// Bucket `i`'s bounds: `(lowest, width)`; the last bucket's width is 0 (unbounded).
fn bucket_bounds(i: usize) -> (u64, u64) {
    if i < SUB as usize {
        return (i as u64, 1);
    }
    if i == LAST_BUCKET {
        return (1u64 << (MAX_E + 1), 0);
    }
    let k = i as u64 - SUB;
    let o = (k / SUB) as u32;
    let s = k % SUB;
    ((SUB + s) << o, 1u64 << o)
}

/// One path's counters and histogram.
pub(crate) struct Hist {
    count: AtomicU64,
    total_ns: AtomicU64,
    max_ns: AtomicU64,
    buckets: [AtomicU64; BUCKETS],
}

impl Hist {
    pub(crate) const fn new() -> Self {
        Hist {
            count: AtomicU64::new(0),
            total_ns: AtomicU64::new(0),
            max_ns: AtomicU64::new(0),
            buckets: [const { AtomicU64::new(0) }; BUCKETS],
        }
    }

    fn record(&self, ns: u64) {
        self.count.fetch_add(1, Relaxed);
        self.total_ns.fetch_add(ns, Relaxed);
        self.max_ns.fetch_max(ns, Relaxed);
        self.buckets[bucket_of(ns)].fetch_add(1, Relaxed);
    }

    fn reset(&self) {
        self.count.store(0, Relaxed);
        self.total_ns.store(0, Relaxed);
        self.max_ns.store(0, Relaxed);
        for b in &self.buckets {
            b.store(0, Relaxed);
        }
    }

    fn read(&self) -> (u64, u64) {
        (self.count.load(Relaxed), self.total_ns.load(Relaxed))
    }

    /// The `q` quantile (0 < q <= 1) from the buckets: the midpoint of the bucket holding the
    /// rank `ceil(q * n)`, never above the largest time seen; 0 with no samples.
    fn quantile(counts: &[u64], max_ns: u64, q: f64) -> u64 {
        let n: u64 = counts.iter().sum();
        if n == 0 {
            return 0;
        }
        let rank = ((q * n as f64).ceil() as u64).clamp(1, n);
        let mut seen = 0u64;
        for (i, &c) in counts.iter().enumerate() {
            seen += c;
            if seen >= rank {
                let (lo, w) = bucket_bounds(i);
                if w == 0 {
                    return max_ns.max(lo);
                }
                return (lo + (w - 1) / 2).min(max_ns.max(lo));
            }
        }
        max_ns
    }

    fn stats(&self, path: Path) -> PathStats {
        let counts: Vec<u64> = self.buckets.iter().map(|b| b.load(Relaxed)).collect();
        let max_ns = self.max_ns.load(Relaxed);
        PathStats {
            path,
            name: path.name(),
            count: self.count.load(Relaxed),
            total_ns: self.total_ns.load(Relaxed),
            max_ns,
            p50_ns: Self::quantile(&counts, max_ns, 0.50),
            p95_ns: Self::quantile(&counts, max_ns, 0.95),
            p99_ns: Self::quantile(&counts, max_ns, 0.99),
            buckets: counts
                .iter()
                .enumerate()
                .filter(|(_, &c)| c > 0)
                .map(|(i, &c)| (bucket_bounds(i).0, c))
                .collect(),
        }
    }
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static HISTS: [Hist; N] = [const { Hist::new() }; N];

/// A timing scope: from creation to drop, when the switch was on at creation. Made by the
/// crate's `timed!` macro.
#[must_use]
pub(crate) struct Scope<'a> {
    hist: &'a Hist,
    start: Option<Instant>,
}

impl<'a> Scope<'a> {
    #[inline(always)]
    pub(crate) fn new_in(on: &AtomicBool, hist: &'a Hist) -> Self {
        Scope {
            hist,
            start: if on.load(Relaxed) {
                Some(Instant::now())
            } else {
                None
            },
        }
    }
}

impl Scope<'static> {
    /// A scope on the process's registry.
    #[inline(always)]
    pub(crate) fn new(path: Path) -> Self {
        Self::new_in(&ENABLED, &HISTS[path as usize])
    }
}

impl Drop for Scope<'_> {
    #[inline(always)]
    fn drop(&mut self) {
        if let Some(t) = self.start {
            finish(self.hist, t);
        }
    }
}

// The on path only: out of line and cold, so a scope's off path stays one load and a branch.
#[cold]
#[inline(never)]
fn finish(hist: &Hist, t: Instant) {
    let ns = u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX);
    hist.record(ns);
}

/// Switch the timers on or off (process-wide). A call already in flight when they switch is
/// timed by the switch's state when it started.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Relaxed);
}

/// Whether the timers are on.
pub fn enabled() -> bool {
    ENABLED.load(Relaxed)
}

/// One path's `(calls timed, total ns)` so far. Doesn't allocate.
pub fn read(path: Path) -> (u64, u64) {
    HISTS[path as usize].read()
}

/// Every path back to zero. Calls in flight may still add theirs.
pub fn reset() {
    for h in &HISTS {
        h.reset();
    }
}

/// One path's figures.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PathStats {
    pub path: Path,
    /// [`Path::name`].
    pub name: &'static str,
    /// Calls timed.
    pub count: u64,
    pub total_ns: u64,
    /// The longest call timed.
    pub max_ns: u64,
    /// Percentiles from the histogram (within about 3%; module docs).
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    /// The non-empty buckets: `(lowest ns, calls)`, in order (to merge runs or recompute
    /// other percentiles).
    pub buckets: Vec<(u64, u64)>,
}

/// Every path's figures at one moment (each path read on its own: not one atomic cut).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Snapshot {
    /// Whether the timers were on.
    pub enabled: bool,
    /// [`HOOKS`]: this build has the test-only hooks.
    pub hooks: bool,
    /// One entry per [`Path::ALL`], in order.
    pub paths: Vec<PathStats>,
}

impl Snapshot {
    /// The figures for `path`.
    pub fn get(&self, path: Path) -> Option<&PathStats> {
        self.paths.iter().find(|p| p.path == path)
    }
}

/// Every path's figures now.
pub fn snapshot() -> Snapshot {
    Snapshot {
        enabled: enabled(),
        hooks: HOOKS,
        paths: Path::ALL
            .iter()
            .map(|&p| HISTS[p as usize].stats(p))
            .collect(),
    }
}

/// What a timed scope costs on this machine, in ns per scope (mean over `rounds`), with the
/// timers off and on. Measured on a private histogram: the process's figures are untouched.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Calibration {
    /// An empty scope with the timers off (one relaxed load and a branch).
    pub off_ns: f64,
    /// An empty scope with the timers on (two clock reads and the record).
    pub on_ns: f64,
    pub rounds: u32,
}

/// Measure [`Calibration`] over `rounds` empty scopes each way (at least 1).
pub fn calibrate(rounds: u32) -> Calibration {
    let rounds = rounds.max(1);
    let hist = Box::new(Hist::new());
    let each = |on: bool| {
        let flag = AtomicBool::new(on);
        let t = Instant::now();
        for _ in 0..rounds {
            let s = Scope::new_in(std::hint::black_box(&flag), &hist);
            std::hint::black_box(&s);
        }
        t.elapsed().as_nanos() as f64 / f64::from(rounds)
    };
    let off_ns = each(false);
    let on_ns = each(true);
    Calibration {
        off_ns,
        on_ns,
        rounds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn names_are_unique_and_round_trip() {
        let mut names: Vec<&str> = Path::ALL.iter().map(|p| p.name()).collect();
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n, "two paths share a name");
        for &p in Path::ALL {
            assert_eq!(Path::from_name(p.name()), Some(p));
            assert_eq!(Path::ALL[p as usize], p, "ALL is in declaration order");
        }
        assert_eq!(Path::from_name("no.such.path"), None);
    }

    #[test]
    fn buckets_cover_and_are_monotonic() {
        assert_eq!(bucket_of(0), 0);
        assert_eq!(bucket_of(SUB - 1), SUB as usize - 1);
        assert_eq!(bucket_of(SUB), SUB as usize);
        assert_eq!(bucket_of(u64::MAX), LAST_BUCKET);
        assert_eq!(bucket_of(1 << (MAX_E + 1)), LAST_BUCKET);
        assert_eq!(bucket_of((1 << (MAX_E + 1)) - 1), LAST_BUCKET - 1);
        // every bucket's bounds hold the values that map to it, and the buckets tile the line
        let mut next = 0u64;
        for i in 0..LAST_BUCKET {
            let (lo, w) = bucket_bounds(i);
            assert_eq!(
                lo,
                next,
                "bucket {i} starts where {} ended",
                i.saturating_sub(1)
            );
            assert!(w >= 1);
            assert_eq!(bucket_of(lo), i);
            assert_eq!(bucket_of(lo + w - 1), i);
            // width at most 1/SUB of the lower bound (past the exact buckets)
            if i >= SUB as usize {
                assert!(w * SUB <= lo, "bucket {i}: width {w} at {lo}");
            }
            next = lo + w;
        }
        assert_eq!(bucket_bounds(LAST_BUCKET).0, next);
    }

    #[test]
    fn counts_total_max_exact_and_reset() {
        let h = Hist::new();
        for ns in [5u64, 100, 1_000, 1_000_000, 0, 7] {
            h.record(ns);
        }
        assert_eq!(h.read(), (6, 1_001_112));
        let s = h.stats(Path::EditAs);
        assert_eq!((s.count, s.total_ns, s.max_ns), (6, 1_001_112, 1_000_000));
        assert_eq!(s.buckets.iter().map(|b| b.1).sum::<u64>(), 6);
        h.reset();
        let s = h.stats(Path::EditAs);
        assert_eq!((s.count, s.total_ns, s.max_ns, s.p50_ns), (0, 0, 0, 0));
        assert!(s.buckets.is_empty());
    }

    #[test]
    fn small_values_are_exact() {
        let h = Hist::new();
        for ns in 1..=20u64 {
            h.record(ns);
        }
        let s = h.stats(Path::EditAs);
        assert_eq!(s.p50_ns, 10);
        assert_eq!(s.p95_ns, 19);
        assert_eq!(s.p99_ns, 20);
    }

    #[test]
    fn off_records_nothing_on_records_once() {
        let h = Hist::new();
        let off = AtomicBool::new(false);
        let on = AtomicBool::new(true);
        drop(Scope::new_in(&off, &h));
        assert_eq!(h.read().0, 0, "a scope with the timers off recorded");
        drop(Scope::new_in(&on, &h));
        assert_eq!(h.read().0, 1);
        // the switch's state at creation decides
        let s = Scope::new_in(&off, &h);
        on.store(true, Relaxed);
        drop(s);
        assert_eq!(h.read().0, 1);
    }

    #[test]
    fn threads_add_up_exactly() {
        let h = Hist::new();
        std::thread::scope(|s| {
            for t in 0..8u64 {
                let h = &h;
                s.spawn(move || {
                    for k in 0..10_000u64 {
                        h.record(t * 1_000 + k % 977);
                    }
                });
            }
        });
        let st = h.stats(Path::Tick);
        assert_eq!(st.count, 80_000);
        assert_eq!(st.buckets.iter().map(|b| b.1).sum::<u64>(), 80_000);
        let want: u64 = (0..8u64)
            .map(|t| (0..10_000u64).map(|k| t * 1_000 + k % 977).sum::<u64>())
            .sum();
        assert_eq!(st.total_ns, want);
        assert_eq!(st.max_ns, 7_000 + 976);
    }

    #[test]
    fn calibrate_measures_both() {
        // the kit's unit tests never switch the process's timers on, so its figures stay put
        let before: Vec<(u64, u64)> = Path::ALL.iter().map(|&p| read(p)).collect();
        let c = calibrate(1_000);
        assert_eq!(c.rounds, 1_000);
        assert!(c.off_ns.is_finite() && c.on_ns.is_finite() && c.on_ns > 0.0);
        let after: Vec<(u64, u64)> = Path::ALL.iter().map(|&p| read(p)).collect();
        assert_eq!(before, after, "calibrate touched the process's figures");
    }

    /// The exact `q` quantile by the same rank rule.
    fn exact(sorted: &[u64], q: f64) -> u64 {
        let n = sorted.len() as u64;
        let rank = ((q * n as f64).ceil() as u64).clamp(1, n);
        sorted[(rank - 1) as usize]
    }

    proptest! {
        /// Each percentile is within its bucket of the exact one: at most 1/SUB of the true
        /// value off (exact below SUB ns).
        #[test]
        fn percentiles_within_a_bucket(
            raw in proptest::collection::vec((0u32..40, 0u64..u64::MAX), 1..400)
        ) {
            // log-spread values: a power of two up to 2^39 (past the last bucket), then a fraction
            let vals: Vec<u64> = raw
                .iter()
                .map(|&(e, f)| if e == 0 { f % SUB } else { (1u64 << e) + f % (1u64 << e) })
                .collect();
            let h = Hist::new();
            for &v in &vals {
                h.record(v);
            }
            let mut sorted = vals.clone();
            sorted.sort_unstable();
            let st = h.stats(Path::Join);
            prop_assert_eq!(st.count, vals.len() as u64);
            prop_assert_eq!(st.max_ns, *sorted.last().unwrap());
            for (q, got) in [(0.5, st.p50_ns), (0.95, st.p95_ns), (0.99, st.p99_ns)] {
                let want = exact(&sorted, q);
                prop_assert!(got <= st.max_ns);
                if want >= 1u64 << (MAX_E + 1) {
                    // the last bucket: reported as the largest seen
                    prop_assert_eq!(got, st.max_ns);
                } else {
                    // (positional: a macro-expanded format string can't capture names)
                    prop_assert_eq!(
                        bucket_of(got),
                        bucket_of(want),
                        "q {}: got {} want {}",
                        q,
                        got,
                        want
                    );
                    let err = got.abs_diff(want);
                    prop_assert!(
                        err * SUB <= want.max(SUB),
                        "q {}: got {} want {}",
                        q,
                        got,
                        want
                    );
                }
            }
        }
    }
}
