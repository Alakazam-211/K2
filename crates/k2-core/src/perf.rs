//! Lightweight performance instrumentation for K2SO.
//!
//! Two entry points:
//!
//! - [`perf_timer!`]       — single-call timing. Logs the elapsed microseconds
//!                           via `log_debug!` under a `[perf]` prefix when
//!                           enabled, otherwise compiles to a no-op for the
//!                           given block.
//!
//! - [`perf_hist!`]        — rolling histogram. Returns a RAII guard; when
//!                           dropped, the elapsed duration is recorded into a
//!                           process-global histogram keyed by name. Every
//!                           [`FLUSH_EVERY`] samples the histogram prints a
//!                           p50 / p99 / mean / count / min / max line.
//!
//! Instrumentation is gated on [`is_enabled`]: true whenever K2SO is a debug
//! build OR the `K2SO_PERF` environment variable is set (any non-empty value).
//! In release builds without the env var, every macro body short-circuits and
//! the cost is a single `Instant::now()` call plus a boolean check — not free,
//! but small enough to leave in permanently.
//!
//! Histogram flush format:
//! ```text
//! [perf] terminal_poll_tick — p50=84µs p99=412µs mean=104µs count=100 min=51µs max=918µs
//! ```
//!
//! Intentional scope: this module is a bespoke measurement aid for the 0.32.13
//! performance pass. It is not a replacement for `tracing` — if we ever need
//! spans across async boundaries, structured filtering, or external collectors,
//! migrate then.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Number of samples to collect before auto-flushing a histogram summary.
pub const FLUSH_EVERY: usize = 100;

/// Maximum samples retained per histogram. Bounds memory at roughly
/// `MAX_SAMPLES × 8 bytes × number_of_named_histograms`.
const MAX_SAMPLES: usize = 500;

static HISTS: OnceLock<Mutex<HashMap<&'static str, Histogram>>> = OnceLock::new();
static ENABLED: OnceLock<bool> = OnceLock::new();

/// Returns true if performance instrumentation should record samples.
/// Cached on first call so repeated lookups are a single atomic read.
///
/// Opt-in via `K2SO_PERF=1`. Pre-0.36.2 this was auto-on in debug
/// builds, which drowned the dev console in `[perf] *_tick` lines and
/// made other tracing (e.g. `[legacy-per-agent-heartbeat]`) hard to
/// spot. If you actually want a perf measurement run, set the env.
pub fn is_enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var("K2SO_PERF")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
    })
}

fn hists() -> &'static Mutex<HashMap<&'static str, Histogram>> {
    HISTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record a single observation against the named histogram. Prints an
/// automatic summary line every [`FLUSH_EVERY`] samples.
pub fn record(name: &'static str, elapsed: Duration) {
    let mut map = hists().lock();
    let hist = map.entry(name).or_default();
    hist.push(elapsed);
    if hist.count == 0 || hist.count % FLUSH_EVERY as u64 != 0 {
        return;
    }
    let line = hist.summary_line(name);
    drop(map);
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "[perf] {}", line);
}

/// Print a summary line for every live histogram. Useful on shutdown or on
/// explicit `K2SO_PERF_FLUSH` user action. Not currently wired to any
/// trigger — exposed for future use (e.g. a debug menu item or a Unix signal
/// handler).
#[allow(dead_code)]
pub fn flush_all() {
    let map = hists().lock();
    use std::io::Write;
    for (name, hist) in map.iter() {
        let _ = writeln!(std::io::stderr(), "[perf] {}", hist.summary_line(name));
    }
}

/// Rolling histogram over the last [`MAX_SAMPLES`] observations. Oldest
/// samples are evicted FIFO — percentiles reflect recent behavior rather than
/// a growing all-time distribution.
#[derive(Default)]
pub struct Histogram {
    samples: Vec<Duration>,
    count: u64,
    min: Duration,
    max: Duration,
}

impl Histogram {
    fn push(&mut self, d: Duration) {
        if self.samples.len() >= MAX_SAMPLES {
            self.samples.remove(0);
        }
        self.samples.push(d);
        self.count += 1;
        if self.min == Duration::ZERO || d < self.min {
            self.min = d;
        }
        if d > self.max {
            self.max = d;
        }
    }

    fn summary_line(&self, name: &str) -> String {
        if self.samples.is_empty() {
            return format!("{} — no samples", name);
        }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let p50 = sorted[sorted.len() * 50 / 100];
        let p99 = sorted[(sorted.len() * 99 / 100).min(sorted.len() - 1)];
        let sum: Duration = self.samples.iter().sum();
        let mean = sum / self.samples.len() as u32;
        format!(
            "{} — p50={}µs p99={}µs mean={}µs count={} min={}µs max={}µs",
            name,
            p50.as_micros(),
            p99.as_micros(),
            mean.as_micros(),
            self.count,
            self.min.as_micros(),
            self.max.as_micros(),
        )
    }
}

/// RAII guard returned by [`perf_hist!`]. Drop records the elapsed time.
pub struct HistGuard {
    name: &'static str,
    start: Instant,
}

impl HistGuard {
    #[inline(always)]
    pub fn new(name: &'static str) -> Self {
        Self { name, start: Instant::now() }
    }
}

impl Drop for HistGuard {
    #[inline(always)]
    fn drop(&mut self) {
        record(self.name, self.start.elapsed());
    }
}

/// Time a block and log its elapsed duration through `log_debug!`.
///
/// ```ignore
/// let hash = perf_timer!("grid_hash", {
///     compute_hash(&grid)
/// });
/// ```
#[macro_export]
macro_rules! perf_timer {
    ($name:expr, $body:block) => {{
        if $crate::perf::is_enabled() {
            let __start = std::time::Instant::now();
            let __result = $body;
            let __elapsed = __start.elapsed();
            log_debug!("[perf] {} — {}µs", $name, __elapsed.as_micros());
            __result
        } else {
            $body
        }
    }};
}

/// Open a scoped histogram guard. The elapsed time from this point until the
/// guard is dropped is recorded under the given name.
///
/// ```ignore
/// let _h = perf_hist!("terminal_poll_tick");
/// // ... do work ...
/// // guard drops here, time is recorded
/// ```
#[macro_export]
macro_rules! perf_hist {
    ($name:expr) => {
        $crate::perf::HistGuard::new($name)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_computes_percentiles() {
        let mut h = Histogram::default();
        for i in 1..=100u64 {
            h.push(Duration::from_micros(i));
        }
        let line = h.summary_line("test");
        // p50 of 1..=100 → sample index 50 → value 51µs
        assert!(line.contains("p50=51µs"), "got: {}", line);
        // p99 → index 99 → value 100µs
        assert!(line.contains("p99=100µs"), "got: {}", line);
        assert!(line.contains("count=100"), "got: {}", line);
        assert!(line.contains("min=1µs"), "got: {}", line);
        assert!(line.contains("max=100µs"), "got: {}", line);
    }

    #[test]
    fn histogram_evicts_oldest_past_max_samples() {
        let mut h = Histogram::default();
        for i in 1..=MAX_SAMPLES as u64 + 50 {
            h.push(Duration::from_micros(i));
        }
        assert_eq!(h.samples.len(), MAX_SAMPLES);
        assert_eq!(h.count, MAX_SAMPLES as u64 + 50);
        // After eviction, the smallest retained sample is 51 (1..50 dropped),
        // so min across retained samples is >= 51 — but historical min stays 1.
        assert_eq!(h.min, Duration::from_micros(1));
    }

    #[test]
    fn histogram_empty_summary() {
        let h = Histogram::default();
        assert_eq!(h.summary_line("empty"), "empty — no samples");
    }

    #[test]
    fn guard_records_on_drop() {
        // Clear any state from prior tests.
        hists().lock().clear();
        {
            let _g = HistGuard::new("guard_test");
            std::thread::sleep(Duration::from_millis(1));
        }
        let map = hists().lock();
        let h = map.get("guard_test").expect("guard should record");
        assert_eq!(h.count, 1);
        assert!(h.samples[0] >= Duration::from_millis(1));
    }
}
