//! Opt-in timing of the front's own work, for finding where a run's time
//! goes. Off unless [`TRACE_VAR`] is set and not empty; then each phase
//! the front marks prints one line on stderr as it ends:
//!
//! ```text
//! k5 timing: <phase> <ms> ms
//! ```
//!
//! `<ms>` is the time since the previous mark (the first counts from
//! [`start`], the top of `main`), with three decimals. A last line,
//! `total`, is the time from [`start`] to the last mark. When the
//! variable's value is a number, it is read as the spawner's clock in
//! nanoseconds just before it started this process (`CLOCK_MONOTONIC` on
//! Linux, `CLOCK_UPTIME_RAW` on macOS, both counted from boot), and a
//! first line, `spawn to main`, gives the time from then to [`start`].
//! A spawner in Python reads it with `time.clock_gettime_ns`, not
//! `time.perf_counter_ns()`: on macOS Python's perf counter can count
//! from the interpreter's start, and the line then reads as the uptime.
//!
//! It changes nothing else: the lines go to stderr only, and only when
//! asked for. The variable is the front's own, one name, never read by
//! the shell CLI, which inherits it unchanged.

use std::io::Write as _;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// The variable that turns the timing on.
pub const TRACE_VAR: &str = "K5_CLI_TRACE_TIMING";

/// What the line starts with.
pub const PREFIX: &str = "k5 timing: ";

struct State {
    start: Instant,
    last: Instant,
}

static STATE: OnceLock<Option<Mutex<State>>> = OnceLock::new();

/// Reads [`TRACE_VAR`] and, when it is on, notes the time. Call it first
/// thing in `main`; until it runs, [`mark`] prints nothing.
pub fn start() {
    STATE.get_or_init(|| {
        let value = std::env::var_os(TRACE_VAR).filter(|v| !v.is_empty())?;
        let now = Instant::now();
        if let Some(ms) = value
            .to_str()
            .and_then(|v| v.parse::<u64>().ok())
            .and_then(spawn_to_now_ms)
        {
            line("spawn to main", ms);
        }
        Some(Mutex::new(State {
            start: now,
            last: now,
        }))
    });
}

/// Ends the phase `name`: prints the time since the last mark.
pub fn mark(name: &str) {
    let Some(Some(state)) = STATE.get() else {
        return;
    };
    let mut s = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    line(name, ms(now - s.last));
    s.last = now;
}

/// Ends the phase `name`, then prints `total`: the time from [`start`].
/// The last mark before the process becomes bash or exits.
pub fn finish(name: &str) {
    mark(name);
    let Some(Some(state)) = STATE.get() else {
        return;
    };
    let s = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    line("total", ms(s.last - s.start));
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// One line, `k5 timing: <phase> <ms> ms`, on stderr. A failed write is
/// let go: the timing must never change what the command does.
fn line(name: &str, ms: f64) {
    let _ = std::io::stderr().write_all(format_line(name, ms).as_bytes());
}

/// The line for `name` and `ms`.
pub fn format_line(name: &str, ms: f64) -> String {
    format!("{PREFIX}{name} {ms:.3} ms\n")
}

/// The milliseconds from `then` (the spawner's clock, in nanoseconds) to
/// now on the same clock. `None` where the clock is not known, or when
/// `then` is after now.
fn spawn_to_now_ms(then: u64) -> Option<f64> {
    let now = clock_ns()?;
    let d = now.checked_sub(then)?;
    Some(d as f64 / 1e6)
}

/// The spawner's clock (see the top), in nanoseconds since boot.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn clock_ns() -> Option<u64> {
    #[cfg(target_os = "linux")]
    let id = libc::CLOCK_MONOTONIC;
    #[cfg(target_os = "macos")]
    let id = libc::CLOCK_UPTIME_RAW;
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes one timespec through a pointer to a
    // local that lives for the call.
    let rc = unsafe { libc::clock_gettime(id, &mut ts) };
    if rc != 0 {
        return None;
    }
    let secs = u64::try_from(ts.tv_sec).ok()?;
    let nanos = u64::try_from(ts.tv_nsec).ok()?;
    secs.checked_mul(1_000_000_000)?.checked_add(nanos)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn clock_ns() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_names_the_phase_and_its_milliseconds() {
        assert_eq!(format_line("route", 0.0123), "k5 timing: route 0.012 ms\n");
        assert_eq!(
            format_line("cache open and hash", 3.5),
            "k5 timing: cache open and hash 3.500 ms\n"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn the_spawn_clock_reads_and_a_later_start_is_no_answer() {
        let now = clock_ns().expect("the clock reads");
        let ms = spawn_to_now_ms(now).expect("a time before now");
        assert!((0.0..1000.0).contains(&ms), "{ms}");
        assert_eq!(spawn_to_now_ms(now + 3_600_000_000_000), None);
    }
}
