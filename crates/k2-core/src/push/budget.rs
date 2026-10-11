//! Local push budgets (prd-thread-push-notifications-v1 §6.3, TP2).
//!
//! The relay counts dispatch CALLS per sliding hour. Before this, a daemon
//! had no limit of its own, so one chatty agent could spend the whole
//! fleet's shared budget. Each daemon now keeps two independent sliding
//! windows:
//!
//! - [`BudgetClass::Thread`]: [`THREAD_PER_HOUR`] calls an hour on the
//!   per-daemon relay path, [`THREAD_PER_HOUR_SHARED`] while the daemon is
//!   on the shared token (a relay without S0), so it can't starve the
//!   fleet's one global window.
//! - [`BudgetClass::Other`] (tickets, project chat): [`OTHER_PER_HOUR`].
//!   Thread pushes can never use it, so a chatty agent never blocks a
//!   ticket.
//!
//! Over budget, a push is dropped (never queued — the `PushTarget`
//! contract); the Thread coalescer carries the dropped count into its next
//! push ("N new messages"). The clock is injected so tests never sleep.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Thread calls an hour on the per-daemon relay path (Q9).
pub const THREAD_PER_HOUR: usize = 30;
/// Thread calls an hour while on the shared token (§6.3).
pub const THREAD_PER_HOUR_SHARED: usize = 5;
/// Ticket + project calls an hour.
pub const OTHER_PER_HOUR: usize = 30;
/// The sliding window.
pub const WINDOW: Duration = Duration::from_secs(60 * 60);

/// Which bucket a send draws from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetClass {
    Thread,
    Other,
}

/// Two independent sliding windows of send instants.
#[derive(Debug, Default)]
pub struct Budgets {
    thread: Mutex<VecDeque<Instant>>,
    other: Mutex<VecDeque<Instant>>,
}

impl Budgets {
    pub fn new() -> Self {
        Self::default()
    }

    fn slot(&self, class: BudgetClass) -> &Mutex<VecDeque<Instant>> {
        match class {
            BudgetClass::Thread => &self.thread,
            BudgetClass::Other => &self.other,
        }
    }

    /// Take one send from `class` at `now`, allowing at most `per_hour`
    /// sends in the trailing [`WINDOW`]. `false` = over budget (nothing
    /// recorded).
    pub fn try_take(&self, class: BudgetClass, per_hour: usize, now: Instant) -> bool {
        let mut q = self.slot(class).lock().unwrap_or_else(|p| p.into_inner());
        while q.front().is_some_and(|t| now.saturating_duration_since(*t) >= WINDOW) {
            q.pop_front();
        }
        if q.len() >= per_hour {
            return false;
        }
        q.push_back(now);
        true
    }

    /// Sends recorded in the trailing window at `now`.
    pub fn used(&self, class: BudgetClass, now: Instant) -> usize {
        let q = self.slot(class).lock().unwrap_or_else(|p| p.into_inner());
        q.iter().filter(|t| now.saturating_duration_since(**t) < WINDOW).count()
    }
}

/// The daemon-wide budgets.
pub fn global() -> &'static Budgets {
    static B: std::sync::OnceLock<Budgets> = std::sync::OnceLock::new();
    B.get_or_init(Budgets::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_exhaustion_never_blocks_ticket_sends() {
        let b = Budgets::new();
        let t0 = Instant::now();
        for i in 0..THREAD_PER_HOUR {
            assert!(b.try_take(BudgetClass::Thread, THREAD_PER_HOUR, t0), "thread send {i}");
        }
        assert!(!b.try_take(BudgetClass::Thread, THREAD_PER_HOUR, t0), "31st thread send dropped");
        assert_eq!(b.used(BudgetClass::Thread, t0), THREAD_PER_HOUR);
        // The ticket/project bucket is untouched by Thread traffic.
        assert_eq!(b.used(BudgetClass::Other, t0), 0);
        for i in 0..OTHER_PER_HOUR {
            assert!(b.try_take(BudgetClass::Other, OTHER_PER_HOUR, t0), "ticket send {i}");
        }
        assert!(!b.try_take(BudgetClass::Other, OTHER_PER_HOUR, t0));
    }

    #[test]
    fn the_window_slides() {
        let b = Budgets::new();
        let t0 = Instant::now();
        for _ in 0..THREAD_PER_HOUR_SHARED {
            assert!(b.try_take(BudgetClass::Thread, THREAD_PER_HOUR_SHARED, t0));
        }
        assert!(!b.try_take(BudgetClass::Thread, THREAD_PER_HOUR_SHARED, t0 + Duration::from_secs(3599)));
        // An hour after the first five, the window is empty again.
        assert!(b.try_take(BudgetClass::Thread, THREAD_PER_HOUR_SHARED, t0 + WINDOW));
        assert_eq!(b.used(BudgetClass::Thread, t0 + WINDOW), 1);
    }

    #[test]
    fn a_refused_take_records_nothing() {
        let b = Budgets::new();
        let t0 = Instant::now();
        assert!(!b.try_take(BudgetClass::Thread, 0, t0), "zero budget refuses");
        assert_eq!(b.used(BudgetClass::Thread, t0), 0);
    }
}
