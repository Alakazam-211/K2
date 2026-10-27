//! In-memory guards for the App-user password doors (prd-app-user-profile-v1
//! P0; vs-live U5, U9, U21).
//!
//! - **Hash slots (U5 c).** Every skin argon2 run (login, password set,
//!   change, reset, the dummy verify) takes one of [`MAX_CONCURRENT_HASHES`]
//!   box-wide slots. One run holds about 19 MiB, so N concurrent junk
//!   requests can no longer hold N × 19 MiB; the rest wait their turn.
//! - **Wrong current passwords (U5 a).** `password.change` (and anything
//!   else that asks for the current password) counts wrong answers per
//!   person: [`CURRENT_PASSWORD_LIMIT`] per [`CURRENT_PASSWORD_WINDOW_SECS`],
//!   then `429 rate_limited` before any argon2. Its own counter, never the
//!   login lockout, so a stolen session can't lock the person out of
//!   signing in.
//! - **Forgot throttles (U9).** The per-name forgot clock lives here, not
//!   in skin.db: an unauthenticated request no longer writes a skin.db row
//!   for every name typed. Bounded: a full table drops its oldest key.
//! - **Reset mail budgets (U9).** At most [`RESET_MAILS_PER_PERSON`] App
//!   reset mails per person per day (across Apps and login names), and at
//!   most [`RESET_MAILS_PER_APP`] per App per hour, so a stranger can't
//!   mail-bomb a person or spend an agent's own mail budget. The owner's
//!   reset is never counted.
//!
//! Everything here is restart-resets by design (like the login limiter).

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

use parking_lot::{Condvar, Mutex};

/// Box-wide concurrent argon2 runs (each about 19 MiB).
pub const MAX_CONCURRENT_HASHES: usize = 4;
/// Wrong current passwords per person before `429`.
pub const CURRENT_PASSWORD_LIMIT: usize = 5;
/// The window those count in.
pub const CURRENT_PASSWORD_WINDOW_SECS: i64 = 15 * 60;
/// App reset mails one person may get per [`PERSON_MAIL_WINDOW_SECS`].
pub const RESET_MAILS_PER_PERSON: usize = 3;
pub const PERSON_MAIL_WINDOW_SECS: i64 = 24 * 3600;
/// App reset mails one App may send per [`APP_MAIL_WINDOW_SECS`]: half the
/// hosted mail's 20/hour per-From cap, so a stranger can never spend all
/// of the agent's own hour.
pub const RESET_MAILS_PER_APP: usize = 10;
pub const APP_MAIL_WINDOW_SECS: i64 = 3600;
/// Keys one table keeps before it drops the least recently seen.
pub const TABLE_CAPACITY: usize = 4096;

fn now_secs() -> i64 {
    super::now_secs()
}

// ── hash and decode slots ───────────────────────────────────────────

/// Picture decodes at once, box-wide (vs-live U17 a: each may hold up
/// to 64 MiB while it decodes and shrinks).
pub const MAX_CONCURRENT_DECODES: usize = 2;

struct Slots {
    cap: usize,
    busy: Mutex<usize>,
    freed: Condvar,
    peak: Mutex<usize>,
}

impl Slots {
    const fn new(cap: usize) -> Self {
        Slots { cap, busy: parking_lot::const_mutex(0), freed: Condvar::new(), peak: parking_lot::const_mutex(0) }
    }

    fn take(&'static self) -> Slot {
        let mut busy = self.busy.lock();
        while *busy >= self.cap {
            self.freed.wait(&mut busy);
        }
        *busy += 1;
        let now = *busy;
        drop(busy);
        let mut peak = self.peak.lock();
        if now > *peak {
            *peak = now;
        }
        Slot(self)
    }
}

static HASHES: Slots = Slots::new(MAX_CONCURRENT_HASHES);
static DECODES: Slots = Slots::new(MAX_CONCURRENT_DECODES);

/// One held slot; dropping it frees the slot.
pub struct Slot(&'static Slots);

impl Drop for Slot {
    fn drop(&mut self) {
        let mut busy = self.0.busy.lock();
        *busy = busy.saturating_sub(1);
        self.0.freed.notify_one();
    }
}

/// Wait for a free hash slot (blocking; call on a blocking thread, never
/// while holding the skin.db mutex).
pub fn hash_slot() -> Slot {
    HASHES.take()
}

/// Wait for a free picture-decode slot (blocking thread only).
pub fn decode_slot() -> Slot {
    DECODES.take()
}

/// The most hash slots ever held at once in this process (tests).
#[doc(hidden)]
pub fn hash_slots_peak() -> usize {
    *HASHES.peak.lock()
}

/// The most decode slots ever held at once in this process (tests).
#[doc(hidden)]
pub fn decode_slots_peak() -> usize {
    *DECODES.peak.lock()
}

// ── a bounded sliding-window table ───────────────────────────────────

#[derive(Default)]
struct Window {
    hits: HashMap<String, VecDeque<i64>>,
}

impl Window {
    fn prune(q: &mut VecDeque<i64>, now: i64, window: i64) {
        while q.front().is_some_and(|t| now.saturating_sub(*t) >= window) {
            q.pop_front();
        }
    }

    fn make_room(&mut self, key: &str, now: i64, window: i64) {
        if self.hits.contains_key(key) || self.hits.len() < TABLE_CAPACITY {
            return;
        }
        self.hits.retain(|_, q| {
            Self::prune(q, now, window);
            !q.is_empty()
        });
        if self.hits.len() >= TABLE_CAPACITY {
            if let Some(k) = self.hits.iter().min_by_key(|(_, q)| q.back().copied().unwrap_or(i64::MIN)).map(|(k, _)| k.clone()) {
                self.hits.remove(&k);
            }
        }
    }

    /// Hits for `key` still inside `window` (prunes).
    fn count(&mut self, key: &str, now: i64, window: i64) -> usize {
        let Some(q) = self.hits.get_mut(key) else { return 0 };
        Self::prune(q, now, window);
        let n = q.len();
        if n == 0 {
            self.hits.remove(key);
        }
        n
    }

    /// Seconds until the oldest hit of `key` leaves the window.
    fn retry_after(&self, key: &str, now: i64, window: i64) -> i64 {
        self.hits
            .get(key)
            .and_then(|q| q.front())
            .map(|t| (t.saturating_add(window) - now).max(1))
            .unwrap_or(1)
    }

    fn record(&mut self, key: &str, now: i64, window: i64, keep: usize) {
        self.make_room(key, now, window);
        let q = self.hits.entry(key.to_string()).or_default();
        Self::prune(q, now, window);
        q.push_back(now);
        while q.len() > keep.max(1) {
            q.pop_front();
        }
    }

    /// Record a hit unless `key` already has `limit` in the window.
    /// `true` = recorded (under the limit).
    fn take(&mut self, key: &str, now: i64, window: i64, limit: usize) -> bool {
        if self.count(key, now, window) >= limit {
            return false;
        }
        self.record(key, now, window, limit);
        true
    }

    /// Age every hit of `key` past `window` (tests).
    fn expire(&mut self, key: &str, window: i64) {
        if let Some(q) = self.hits.get_mut(key) {
            for t in q.iter_mut() {
                *t = t.saturating_sub(window);
            }
        }
    }
}

struct Tables {
    current_password: Window,
    forgot: Window,
    person_mail: Window,
    app_mail: Window,
}

fn tables() -> &'static Mutex<Tables> {
    static T: OnceLock<Mutex<Tables>> = OnceLock::new();
    T.get_or_init(|| {
        Mutex::new(Tables {
            current_password: Window::default(),
            forgot: Window::default(),
            person_mail: Window::default(),
            app_mail: Window::default(),
        })
    })
}

// ── wrong current passwords ──────────────────────────────────────────

/// `Some(retry_after_secs)` when `principal_id` is over the wrong-current-
/// password limit (answer `429` without hashing).
pub fn current_password_blocked(principal_id: &str) -> Option<i64> {
    let now = now_secs();
    let mut t = tables().lock();
    let w = &mut t.current_password;
    (w.count(principal_id, now, CURRENT_PASSWORD_WINDOW_SECS) >= CURRENT_PASSWORD_LIMIT)
        .then(|| w.retry_after(principal_id, now, CURRENT_PASSWORD_WINDOW_SECS))
}

/// Count one wrong current password for `principal_id`.
pub fn record_wrong_current_password(principal_id: &str) {
    let now = now_secs();
    tables().lock().current_password.record(principal_id, now, CURRENT_PASSWORD_WINDOW_SECS, CURRENT_PASSWORD_LIMIT);
}

/// Move `principal_id`'s wrong-password window into the past (tests).
#[doc(hidden)]
pub fn expire_current_password_window(principal_id: &str) {
    tables().lock().current_password.expire(principal_id, CURRENT_PASSWORD_WINDOW_SECS);
}

// ── forgot, per presented name ───────────────────────────────────────

/// One forgot per `key` (a presented name, lowercased) per `secs`.
/// `true` = throttled. Memory only.
pub fn forgot_throttled(key: &str, secs: i64) -> bool {
    let now = now_secs();
    !tables().lock().forgot.take(key, now, secs, 1)
}

/// Move `key`'s forgot clock `secs` into the past (tests).
#[doc(hidden)]
pub fn expire_forgot(key: &str, secs: i64) {
    tables().lock().forgot.expire(key, secs);
}

/// Keys the forgot table holds (tests: it stays bounded).
#[doc(hidden)]
pub fn forgot_keys() -> usize {
    tables().lock().forgot.hits.len()
}

// ── reset mail budgets ───────────────────────────────────────────────

/// Take one App reset mail for (`principal_id`, `app`) from both budgets.
/// `false` = over a budget: send nothing.
pub fn take_reset_mail(principal_id: &str, app: &str) -> bool {
    let now = now_secs();
    let mut t = tables().lock();
    if t.person_mail.count(principal_id, now, PERSON_MAIL_WINDOW_SECS) >= RESET_MAILS_PER_PERSON
        || t.app_mail.count(app, now, APP_MAIL_WINDOW_SECS) >= RESET_MAILS_PER_APP
    {
        return false;
    }
    t.person_mail.record(principal_id, now, PERSON_MAIL_WINDOW_SECS, RESET_MAILS_PER_PERSON);
    t.app_mail.record(app, now, APP_MAIL_WINDOW_SECS, RESET_MAILS_PER_APP);
    true
}

/// Give back a mail taken by [`take_reset_mail`] that never left (no
/// mail path, a refusal): only sent mails count.
pub fn return_reset_mail(principal_id: &str, app: &str) {
    let mut t = tables().lock();
    let t = &mut *t;
    for (w, k) in [(&mut t.person_mail, principal_id), (&mut t.app_mail, app)] {
        if let Some(q) = w.hits.get_mut(k) {
            q.pop_back();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_password_limit_then_window() {
        let p = format!("guard-test-{}", uuid::Uuid::new_v4());
        for _ in 0..CURRENT_PASSWORD_LIMIT {
            assert_eq!(current_password_blocked(&p), None);
            record_wrong_current_password(&p);
        }
        let retry = current_password_blocked(&p).expect("blocked after the limit");
        assert!(retry > 0 && retry <= CURRENT_PASSWORD_WINDOW_SECS, "{retry}");
        expire_current_password_window(&p);
        assert_eq!(current_password_blocked(&p), None, "a passed window opens it again");
    }

    #[test]
    fn forgot_is_one_per_key_and_bounded() {
        let k = format!("guard-forgot-{}", uuid::Uuid::new_v4());
        assert!(!forgot_throttled(&k, 60));
        assert!(forgot_throttled(&k, 60));
        for i in 0..(TABLE_CAPACITY + 50) {
            let _ = forgot_throttled(&format!("spray-{i}-{k}"), 60);
        }
        assert!(forgot_keys() <= TABLE_CAPACITY, "{}", forgot_keys());
    }

    #[test]
    fn person_budget_spans_apps_and_a_return_gives_it_back() {
        let p = format!("guard-mail-{}", uuid::Uuid::new_v4());
        assert!(take_reset_mail(&p, &format!("a1-{p}")));
        assert!(take_reset_mail(&p, &format!("a2-{p}")));
        assert!(take_reset_mail(&p, &format!("a3-{p}")));
        assert!(!take_reset_mail(&p, &format!("a4-{p}")), "the 4th App mail of a day is refused");
        return_reset_mail(&p, &format!("a3-{p}"));
        assert!(take_reset_mail(&p, &format!("a5-{p}")));
        let app = format!("app-{p}");
        for i in 0..RESET_MAILS_PER_APP {
            assert!(take_reset_mail(&format!("{i}-{p}"), &app), "{i}");
        }
        assert!(!take_reset_mail(&format!("x-{p}"), &app), "the App's hour is spent");
    }

    #[test]
    fn hash_slots_never_exceed_the_cap() {
        let handles: Vec<_> = (0..24)
            .map(|_| {
                std::thread::spawn(|| {
                    let _s = hash_slot();
                    std::thread::sleep(std::time::Duration::from_millis(15));
                })
            })
            .collect();
        for h in handles {
            h.join().expect("slot thread");
        }
        let peak = hash_slots_peak();
        assert!((1..=MAX_CONCURRENT_HASHES).contains(&peak), "peak {peak}");
    }
}
