//! The long-poll behind `k2 approve request --wait` and
//! `GET /cli/approve/wait`: a change counter every state change bumps, and
//! a bounded wait on it. In memory only; a restart just ends the poll and
//! the CLI asks again.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

static CHANGE: (Mutex<u64>, Condvar) = (Mutex::new(0), Condvar::new());

/// The longest one wait call holds the connection.
pub(crate) const MAX_WAIT_SECS: u64 = 50;
/// The default when the caller names none.
pub(crate) const DEFAULT_WAIT_SECS: u64 = 25;

/// Some request changed state: wake every waiter (each re-reads its row).
pub(crate) fn notify() {
    let (m, cv) = &CHANGE;
    let mut g = m.lock().unwrap_or_else(|p| p.into_inner());
    *g = g.wrapping_add(1);
    cv.notify_all();
}

/// The counter now (read before checking the row, so a change between the
/// check and the wait is never missed).
pub(crate) fn counter() -> u64 {
    *CHANGE.0.lock().unwrap_or_else(|p| p.into_inner())
}

/// Block until the counter moves past `seen` or `until` passes. True when
/// it moved.
pub(crate) fn wait_past(seen: u64, until: Instant) -> bool {
    let (m, cv) = &CHANGE;
    let mut g = m.lock().unwrap_or_else(|p| p.into_inner());
    while *g == seen {
        let now = Instant::now();
        if now >= until {
            return false;
        }
        let (next, _) = cv.wait_timeout(g, until - now).unwrap_or_else(|p| p.into_inner());
        g = next;
    }
    true
}

/// Request ids a wait call is holding right now (so the outcome line isn't
/// typed into a session whose command is about to print the answer).
static HELD: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Mark `id` as waited on until the guard drops.
pub(crate) fn hold(id: &str) -> HoldGuard {
    HELD.lock().unwrap_or_else(|p| p.into_inner()).push(id.to_string());
    HoldGuard(id.to_string())
}

pub(crate) struct HoldGuard(String);

impl Drop for HoldGuard {
    fn drop(&mut self) {
        let mut g = HELD.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(i) = g.iter().position(|x| x == &self.0) {
            g.remove(i);
        }
    }
}

/// A wait call holds `id` now.
pub(crate) fn is_waited(id: &str) -> bool {
    HELD.lock().unwrap_or_else(|p| p.into_inner()).iter().any(|x| x == id)
}

/// The wait the caller asked for (`timeout` seconds), within the bounds.
pub(crate) fn clamp(timeout: Option<&str>) -> Duration {
    let secs = timeout.and_then(|t| t.trim().parse::<u64>().ok()).unwrap_or(DEFAULT_WAIT_SECS);
    Duration::from_secs(secs.clamp(1, MAX_WAIT_SECS))
}
