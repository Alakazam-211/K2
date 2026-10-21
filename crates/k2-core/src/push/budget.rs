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
//! - [`BudgetClass::Permission`] (permission cards, prd-agent-settings-cards-v1
//!   rev 3.1 §6.7, §17.7): [`PERMISSION_PER_HOUR`] pushes an hour **per
//!   person** (keyed by the person key), apart from the Thread bucket, so a
//!   chatty agent's posts never hold back a card and an agent asking a lot
//!   never spends another person's budget.
//!
//! Over budget, a push is dropped (never queued — the `PushTarget`
//! contract); the Thread coalescer carries the dropped count into its next
//! push ("N new messages"). The clock is injected so tests never sleep.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Thread calls an hour on the per-daemon relay path (Q9).
pub const THREAD_PER_HOUR: usize = 30;
/// Thread calls an hour while on the shared token (§6.3).
pub const THREAD_PER_HOUR_SHARED: usize = 5;
/// Ticket + project calls an hour.
pub const OTHER_PER_HOUR: usize = 30;
/// Silent badge corrections an hour (thread-read-v1). Their own bucket:
/// a person reading on several devices can never starve a Thread or
/// ticket push. The relay allows 600/h per server.
pub const BADGE_PER_HOUR: usize = 240;
/// Permission-card pushes an hour, per person (§6.7, §17.7).
pub const PERMISSION_PER_HOUR: usize = 30;
/// The sliding window.
pub const WINDOW: Duration = Duration::from_secs(60 * 60);

/// Which bucket a send draws from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetClass {
    Thread,
    Other,
    /// Silent badge corrections (thread-read-v1).
    Badge,
    /// Permission-card pushes (rev 3.1 §6.7): one window per person; see
    /// [`Budgets::try_take_for`].
    Permission,
}

/// Independent sliding windows of send instants.
#[derive(Debug, Default)]
pub struct Budgets {
    thread: Mutex<VecDeque<Instant>>,
    other: Mutex<VecDeque<Instant>>,
    badge: Mutex<VecDeque<Instant>>,
    /// [`BudgetClass::Permission`], one window per person key.
    permission: Mutex<HashMap<String, VecDeque<Instant>>>,
}

fn take_from(q: &mut VecDeque<Instant>, per_hour: usize, now: Instant, n: usize) -> bool {
    while q.front().is_some_and(|t| now.saturating_duration_since(*t) >= WINDOW) {
        q.pop_front();
    }
    if q.len() + n > per_hour {
        return false;
    }
    for _ in 0..n {
        q.push_back(now);
    }
    true
}

fn used_in(q: &VecDeque<Instant>, now: Instant) -> usize {
    q.iter().filter(|t| now.saturating_duration_since(**t) < WINDOW).count()
}

impl Budgets {
    pub fn new() -> Self {
        Self::default()
    }

    fn slot(&self, class: BudgetClass) -> Option<&Mutex<VecDeque<Instant>>> {
        match class {
            BudgetClass::Thread => Some(&self.thread),
            BudgetClass::Other => Some(&self.other),
            BudgetClass::Badge => Some(&self.badge),
            BudgetClass::Permission => None,
        }
    }

    /// Take one send from `class` at `now`, allowing at most `per_hour`
    /// sends in the trailing [`WINDOW`]. `false` = over budget (nothing
    /// recorded).
    pub fn try_take(&self, class: BudgetClass, per_hour: usize, now: Instant) -> bool {
        self.try_take_n(class, per_hour, now, 1)
    }

    /// Take `n` sends at once, all or nothing (a Thread push whose phones
    /// split on previews is two gateway calls). `false` = not enough room
    /// (nothing recorded).
    ///
    /// A keyed class ([`BudgetClass::Permission`]) takes from the window
    /// with the empty key here; use [`Self::try_take_for`].
    pub fn try_take_n(&self, class: BudgetClass, per_hour: usize, now: Instant, n: usize) -> bool {
        self.try_take_for(class, "", per_hour, now, n)
    }

    /// [`Self::try_take_n`] for one person's window (`key` = the person
    /// key) of a keyed class; the shared classes ignore `key`.
    pub fn try_take_for(&self, class: BudgetClass, key: &str, per_hour: usize, now: Instant, n: usize) -> bool {
        match self.slot(class) {
            Some(slot) => take_from(&mut slot.lock().unwrap_or_else(|p| p.into_inner()), per_hour, now, n),
            None => {
                let mut map = self.permission.lock().unwrap_or_else(|p| p.into_inner());
                // Forget idle people so the map never grows without bound.
                map.retain(|_, q| q.back().is_some_and(|t| now.saturating_duration_since(*t) < WINDOW));
                take_from(map.entry(key.to_string()).or_default(), per_hour, now, n)
            }
        }
    }

    /// Sends recorded in the trailing window at `now`. For a keyed class:
    /// every person's sends together.
    pub fn used(&self, class: BudgetClass, now: Instant) -> usize {
        match self.slot(class) {
            Some(slot) => used_in(&slot.lock().unwrap_or_else(|p| p.into_inner()), now),
            None => self.permission.lock().unwrap_or_else(|p| p.into_inner()).values().map(|q| used_in(q, now)).sum(),
        }
    }

    /// One person's sends in a keyed class's trailing window at `now`.
    pub fn used_for(&self, class: BudgetClass, key: &str, now: Instant) -> usize {
        match self.slot(class) {
            Some(_) => self.used(class, now),
            None => self.permission.lock().unwrap_or_else(|p| p.into_inner()).get(key).map_or(0, |q| used_in(q, now)),
        }
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

    /// thread-read-v1: badge corrections have their own bucket; spending
    /// it never touches the Thread or ticket/project budgets, and theirs
    /// never blocks a badge.
    #[test]
    fn badge_budget_is_isolated_both_ways() {
        let b = Budgets::new();
        let t0 = Instant::now();
        for i in 0..BADGE_PER_HOUR {
            assert!(b.try_take(BudgetClass::Badge, BADGE_PER_HOUR, t0), "badge send {i}");
        }
        assert!(!b.try_take(BudgetClass::Badge, BADGE_PER_HOUR, t0), "over the badge budget");
        assert_eq!(b.used(BudgetClass::Thread, t0), 0);
        assert_eq!(b.used(BudgetClass::Other, t0), 0);
        assert!(b.try_take(BudgetClass::Thread, THREAD_PER_HOUR, t0), "a Thread push still goes");
        assert!(b.try_take(BudgetClass::Other, OTHER_PER_HOUR, t0), "a ticket push still goes");
        let fresh = Budgets::new();
        for _ in 0..THREAD_PER_HOUR {
            assert!(fresh.try_take(BudgetClass::Thread, THREAD_PER_HOUR, t0));
        }
        assert!(fresh.try_take(BudgetClass::Badge, BADGE_PER_HOUR, t0), "a spent Thread bucket never blocks a badge");
    }

    /// A two-call take (a preview split) is all or nothing.
    #[test]
    fn a_multi_call_take_is_all_or_nothing() {
        let b = Budgets::new();
        let t0 = Instant::now();
        assert!(b.try_take_n(BudgetClass::Thread, 3, t0, 2));
        assert_eq!(b.used(BudgetClass::Thread, t0), 2);
        assert!(!b.try_take_n(BudgetClass::Thread, 3, t0, 2), "one left: a two-call take is refused");
        assert_eq!(b.used(BudgetClass::Thread, t0), 2, "and records nothing");
        assert!(b.try_take_n(BudgetClass::Thread, 3, t0, 1), "the single call still fits");
        assert_eq!(b.used(BudgetClass::Thread, t0), 3);
    }

    /// Permission cards (rev 3.1 §6.7, §13.3): their own bucket, counted
    /// per person. A spent Thread bucket never blocks a card push, a spent
    /// card budget never blocks a Thread push, and one person's 30 never
    /// touch another's.
    #[test]
    fn permission_budget_is_per_person_and_apart_from_the_thread_bucket() {
        let b = Budgets::new();
        let t0 = Instant::now();
        for _ in 0..THREAD_PER_HOUR {
            assert!(b.try_take(BudgetClass::Thread, THREAD_PER_HOUR, t0));
        }
        for i in 0..PERMISSION_PER_HOUR {
            assert!(b.try_take_for(BudgetClass::Permission, "rosson", PERMISSION_PER_HOUR, t0, 1), "card push {i}");
        }
        assert!(!b.try_take_for(BudgetClass::Permission, "rosson", PERMISSION_PER_HOUR, t0, 1), "the 31st is dropped");
        assert_eq!(b.used_for(BudgetClass::Permission, "rosson", t0), PERMISSION_PER_HOUR);
        assert!(b.try_take_for(BudgetClass::Permission, "dana", PERMISSION_PER_HOUR, t0, 1), "another person's budget is untouched");
        assert_eq!(b.used(BudgetClass::Permission, t0), PERMISSION_PER_HOUR + 1, "used() sums every person");
        assert_eq!(b.used(BudgetClass::Thread, t0), THREAD_PER_HOUR, "the Thread bucket is its own");
        assert_eq!(b.used(BudgetClass::Other, t0), 0);
        assert_eq!(b.used(BudgetClass::Badge, t0), 0);
        let fresh = Budgets::new();
        for _ in 0..PERMISSION_PER_HOUR {
            assert!(fresh.try_take_for(BudgetClass::Permission, "rosson", PERMISSION_PER_HOUR, t0, 1));
        }
        assert!(fresh.try_take(BudgetClass::Thread, THREAD_PER_HOUR, t0), "a spent card budget never blocks a Thread push");
        // The window slides per person, and idle people are forgotten.
        assert!(b.try_take_for(BudgetClass::Permission, "rosson", PERMISSION_PER_HOUR, t0 + WINDOW, 1));
        assert_eq!(b.used_for(BudgetClass::Permission, "rosson", t0 + WINDOW), 1);
        assert_eq!(b.used_for(BudgetClass::Permission, "dana", t0 + WINDOW), 0);
    }
}
