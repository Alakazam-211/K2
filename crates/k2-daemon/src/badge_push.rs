//! Silent badge-correction pushes (thread-read-v1, 0.46.1; Rosson
//! 2026-10-17 moved them into 0.46.1).
//!
//! When a person's totals change because they read, muted or answered on
//! one device, their phones get a SILENT push (`data.kind = "badge"`,
//! `unreadTotal`, `waitingTotal`, the server stamp and an opaque per-person
//! `badgeKey`; no title, body or sound) so the icon badge is right without
//! opening the app. The relay sends it as an APNs background push / an FCM
//! data message, only on the per-daemon path.
//!
//! Rules:
//! - **Coalesced** per person: at most one per [`WINDOW`] (10 s). The
//!   first trigger in a quiet window sends at once; later ones schedule
//!   ONE send at the end of the window, which reads the totals THEN (the
//!   latest numbers, never a queue of stale ones).
//! - **Skipped** when the totals equal what this person's phones last got
//!   (from a badge push or the `unreadTotal` of a Thread/project push).
//! - **Only that person's devices**, and only when their server bell is on.
//! - **Its own budget** ([`budget::BudgetClass::Badge`]): it can never
//!   starve a Thread or ticket push. Over budget, or on a relay 429, the
//!   push is DROPPED, never retried: the next one carries the full totals.
//! - Air-gap on, or no per-daemon relay path (no paired tunnel): nothing.
//!
//! The [`Coalescer`] is pure state + an injected [`BadgeEnv`] (fake clock
//! and capture sink in tests). The daemon ticks one global instance.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_core::push::{BadgePush, PushEvent};
use k2_core::read_state::Totals;

/// At most one badge push per person per window.
pub const WINDOW: Duration = Duration::from_secs(10);
/// Forget a person whose last push is this old and who has nothing due.
const FORGET_AFTER: Duration = Duration::from_secs(60 * 60);

/// What the coalescer needs from the daemon.
pub trait BadgeEnv {
    /// The person's totals now (`None` = can't tell; nothing is sent).
    fn totals(&self, person: &str) -> Option<Totals>;
    /// The person's device logins whose bell is on (empty = no push).
    fn devices(&self, person: &str) -> Vec<String>;
    /// Take one call from the badge budget at `now`.
    fn take_budget(&self, now: Instant) -> bool;
    /// Air-gap on, or no per-daemon relay path: nothing leaves.
    fn blocked(&self) -> bool;
    /// Send one badge push.
    fn send(&self, event: PushEvent);
}

#[derive(Debug, Default, Clone)]
struct Entry {
    /// The totals this person's phones were last told.
    last_told: Option<Totals>,
    /// When the last badge push went out.
    last_send: Option<Instant>,
    /// A send scheduled for the end of the window.
    due: Option<Instant>,
}

/// Per-person badge state.
#[derive(Debug, Default)]
pub struct Coalescer {
    people: HashMap<String, Entry>,
}

impl Coalescer {
    pub fn new() -> Self {
        Self::default()
    }

    /// `person`'s totals may have changed (a read, mute or answer).
    pub fn trigger(&mut self, env: &dyn BadgeEnv, person: &str, now: Instant) {
        let e = self.people.entry(person.to_string()).or_default();
        if e.due.is_some() {
            return;
        }
        match e.last_send {
            Some(t) if now.saturating_duration_since(t) < WINDOW => e.due = Some(t + WINDOW),
            _ => self.flush(env, person, now),
        }
    }

    /// The phones got these totals another way (a Thread / project push's
    /// `unreadTotal`): a badge push with the same numbers is skipped.
    pub fn note_told(&mut self, person: &str, totals: Totals) {
        self.people.entry(person.to_string()).or_default().last_told = Some(totals);
    }

    /// Send every scheduled push whose window is over.
    pub fn tick(&mut self, env: &dyn BadgeEnv, now: Instant) {
        let due: Vec<String> = self
            .people
            .iter()
            .filter(|(_, e)| e.due.is_some_and(|t| now >= t))
            .map(|(p, _)| p.clone())
            .collect();
        for p in due {
            self.flush(env, &p, now);
        }
        // A person's last-told totals are kept (one small entry per
        // person); only an idle entry with nothing to remember goes.
        self.people.retain(|_, e| {
            e.due.is_some()
                || e.last_told.is_some()
                || e.last_send.is_some_and(|t| now.saturating_duration_since(t) < FORGET_AFTER)
        });
    }

    fn flush(&mut self, env: &dyn BadgeEnv, person: &str, now: Instant) {
        let Some(e) = self.people.get_mut(person) else { return };
        e.due = None;
        if env.blocked() {
            return;
        }
        let Some(totals) = env.totals(person) else { return };
        if e.last_told == Some(totals) {
            return;
        }
        let devices = env.devices(person);
        if devices.is_empty() {
            return;
        }
        if !env.take_budget(now) {
            // Dropped, never queued: the next trigger reads fresh totals.
            crate::push_routes::note_budget_drop(k2_core::push::budget::BudgetClass::Badge);
            return;
        }
        e.last_told = Some(totals);
        e.last_send = Some(now);
        env.send(PushEvent::Badge(BadgePush {
            principal: person.to_string(),
            device_usernames: devices,
            unread_total: totals.total.to_string(),
            waiting_total: totals.waiting_total.to_string(),
        }));
    }

    #[cfg(test)]
    fn due(&self, person: &str) -> Option<Instant> {
        self.people.get(person).and_then(|e| e.due)
    }
}

// ── The daemon's instance ─────────────────────────────────────────────

/// People, prefs, devices and read state from the daemon's stores.
pub struct DaemonEnv;

impl BadgeEnv for DaemonEnv {
    fn totals(&self, person: &str) -> Option<Totals> {
        let people = k2_core::notify_prefs::People::load().ok()?;
        let p = k2_core::read_state::Person::of_login(&people, person);
        let db = k2_core::db::shared();
        let conn = db.lock();
        if !k2_core::read_state::tables_exist(&conn) {
            return None;
        }
        k2_core::read_state::totals(&conn, &p, k2_core::read_state::now_secs()).ok()
    }

    fn devices(&self, person: &str) -> Vec<String> {
        person_bell_devices(person)
    }

    fn take_budget(&self, now: Instant) -> bool {
        crate::push_routes::take_badge_budget(now)
    }

    fn blocked(&self) -> bool {
        crate::push_routes::airgap_blocks() || !crate::push_routes::per_daemon_path()
    }

    fn send(&self, event: PushEvent) {
        crate::push_routes::deliver_badge(event);
    }
}

/// A person's device logins, when their server bell is on (removed or
/// disabled logins never count, TP11).
pub(crate) fn person_bell_devices(person: &str) -> Vec<String> {
    let Ok(people) = k2_core::notify_prefs::People::load() else { return Vec::new() };
    let devices = k2_core::push_devices::list_devices().unwrap_or_default();
    let prefs = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if !k2_core::notify_prefs::tables_exist(&conn) {
            return Vec::new();
        }
        k2_core::notify_prefs::PrefsSnapshot::load(&conn).unwrap_or_default()
    };
    if !prefs.bell(person).on {
        return Vec::new();
    }
    let mut out: Vec<String> = devices
        .into_iter()
        .filter(|d| people.device_person(&d.username).as_deref() == Some(person))
        .map(|d| d.username)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Every person with at least one registered device (an answered card
/// changes everyone's `waitingTotal`).
fn people_with_devices() -> Vec<String> {
    let Ok(people) = k2_core::notify_prefs::People::load() else { return Vec::new() };
    let mut out: Vec<String> = k2_core::push_devices::list_devices()
        .unwrap_or_default()
        .iter()
        .filter_map(|d| people.device_person(&d.username))
        .collect();
    out.sort();
    out.dedup();
    out
}

fn global() -> &'static Mutex<Coalescer> {
    static C: std::sync::OnceLock<Mutex<Coalescer>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(Coalescer::new()))
}

fn lock() -> std::sync::MutexGuard<'static, Coalescer> {
    global().lock().unwrap_or_else(|p| p.into_inner())
}

fn ensure_ticker() {
    #[cfg(not(test))]
    {
        static STARTED: std::sync::Once = std::sync::Once::new();
        STARTED.call_once(|| {
            let spawned = std::thread::Builder::new().name("badge-push-tick".into()).spawn(|| loop {
                std::thread::sleep(Duration::from_secs(1));
                lock().tick(&DaemonEnv, Instant::now());
            });
            if let Err(e) = spawned {
                k2_core::log_debug!("[badge_push] ticker spawn failed: {e}");
            }
        });
    }
}

/// `person` read, muted or answered on some device. Off the caller's
/// thread: reading totals takes the DB lock, which a route may hold.
pub fn trigger(person: &str) {
    let person = person.to_string();
    run_off_thread(move || {
        ensure_ticker();
        lock().trigger(&DaemonEnv, &person, Instant::now());
    });
}

/// A card was answered, voided or expired: every person with a phone may
/// have a new `waitingTotal`.
pub fn trigger_everyone() {
    run_off_thread(|| {
        ensure_ticker();
        let people = people_with_devices();
        let mut c = lock();
        for p in people {
            c.trigger(&DaemonEnv, &p, Instant::now());
        }
    });
}

/// A Thread or project push told `person`'s phones these totals.
pub fn note_told(person: &str, totals: Totals) {
    lock().note_told(person, totals);
}

#[cfg(test)]
fn run_off_thread(f: impl FnOnce() + Send + 'static) {
    // Unit tests drive the Coalescer directly; the daemon wiring is a no-op
    // here so route tests never touch the global badge state.
    let _ = f;
}

#[cfg(not(test))]
fn run_off_thread(f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name("badge-push".into()).spawn(f) {
        k2_core::log_debug!("[badge_push] spawn failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    struct Fake {
        totals: RefCell<HashMap<String, Totals>>,
        devices: RefCell<HashMap<String, Vec<String>>>,
        budget: Cell<Option<u32>>,
        blocked: Cell<bool>,
        sent: RefCell<Vec<BadgePush>>,
    }

    impl Fake {
        fn with(person: &str, devices: &[&str], total: i64, waiting: i64) -> Self {
            let f = Self::default();
            f.set(person, total, waiting);
            f.devices.borrow_mut().insert(person.into(), devices.iter().map(|d| d.to_string()).collect());
            f
        }
        fn set(&self, person: &str, total: i64, waiting: i64) {
            self.totals.borrow_mut().insert(person.into(), Totals { total, waiting_total: waiting });
        }
        fn sent(&self) -> Vec<BadgePush> {
            self.sent.borrow().clone()
        }
    }

    impl BadgeEnv for Fake {
        fn totals(&self, person: &str) -> Option<Totals> {
            self.totals.borrow().get(person).copied()
        }
        fn devices(&self, person: &str) -> Vec<String> {
            self.devices.borrow().get(person).cloned().unwrap_or_default()
        }
        fn take_budget(&self, _now: Instant) -> bool {
            match self.budget.get() {
                None => true,
                Some(0) => false,
                Some(n) => {
                    self.budget.set(Some(n - 1));
                    true
                }
            }
        }
        fn blocked(&self) -> bool {
            self.blocked.get()
        }
        fn send(&self, event: PushEvent) {
            match event {
                PushEvent::Badge(b) => self.sent.borrow_mut().push(b),
                other => panic!("badge coalescer sent a non-badge push: {other:?}"),
            }
        }
    }

    #[test]
    fn leading_edge_then_one_trailing_push_with_the_latest_totals() {
        let env = Fake::with("gina", &["gina"], 5, 1);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.trigger(&env, "gina", t0);
        assert_eq!(env.sent().len(), 1, "the first trigger in a quiet window sends at once");
        assert_eq!((env.sent()[0].unread_total.as_str(), env.sent()[0].waiting_total.as_str()), ("5", "1"));
        // Three more reads within the window: one scheduled send.
        env.set("gina", 4, 1);
        c.trigger(&env, "gina", t0 + Duration::from_secs(1));
        env.set("gina", 2, 1);
        c.trigger(&env, "gina", t0 + Duration::from_secs(2));
        env.set("gina", 1, 0);
        c.trigger(&env, "gina", t0 + Duration::from_secs(3));
        assert_eq!(env.sent().len(), 1, "held inside the window");
        assert_eq!(c.due("gina"), Some(t0 + WINDOW));
        c.tick(&env, t0 + Duration::from_secs(9));
        assert_eq!(env.sent().len(), 1, "not before the window ends");
        c.tick(&env, t0 + WINDOW);
        let sent = env.sent();
        assert_eq!(sent.len(), 2, "one trailing push");
        assert_eq!((sent[1].unread_total.as_str(), sent[1].waiting_total.as_str()), ("1", "0"), "the latest totals, read at send time");
        // A trigger a full window later is a fresh leading edge.
        env.set("gina", 3, 0);
        c.trigger(&env, "gina", t0 + WINDOW * 3);
        assert_eq!(env.sent().len(), 3);
    }

    #[test]
    fn unchanged_totals_are_skipped_and_cost_no_budget() {
        let env = Fake::with("gina", &["gina"], 2, 0);
        env.budget.set(Some(1));
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        // A Thread push already told the phone 2/0.
        c.note_told("gina", Totals { total: 2, waiting_total: 0 });
        c.trigger(&env, "gina", t0);
        assert!(env.sent().is_empty(), "same numbers as the phone has: skipped");
        assert_eq!(env.budget.get(), Some(1), "a skip takes no budget");
        env.set("gina", 1, 0);
        c.trigger(&env, "gina", t0 + Duration::from_secs(1));
        assert_eq!(env.sent().len(), 1, "a real change goes (the skip opened no window)");
        // Over budget: dropped, not queued or retried.
        env.set("gina", 0, 0);
        c.trigger(&env, "gina", t0 + WINDOW * 2);
        assert_eq!(env.sent().len(), 1, "over budget: dropped");
        assert_eq!(c.due("gina"), None, "nothing queued for a retry");
        c.tick(&env, t0 + WINDOW * 4);
        assert_eq!(env.sent().len(), 1);
    }

    #[test]
    fn only_that_persons_devices_and_never_when_blocked_or_without_devices() {
        let env = Fake::with("gina", &["gina", "gina-ipad"], 3, 1);
        env.devices.borrow_mut().insert("owner".into(), vec!["owner".into(), "rosson".into()]);
        env.set("owner", 9, 0);
        env.set("zed", 4, 0);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.trigger(&env, "gina", t0);
        let sent = env.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].principal, "gina");
        assert_eq!(sent[0].device_usernames, vec!["gina".to_string(), "gina-ipad".to_string()], "gina's logins only");
        c.trigger(&env, "zed", t0);
        assert_eq!(env.sent().len(), 1, "no devices (or bell off): nothing");
        env.blocked.set(true);
        c.trigger(&env, "owner", t0);
        assert_eq!(env.sent().len(), 1, "air-gap / no per-daemon path: nothing");
        env.blocked.set(false);
        c.trigger(&env, "owner", t0 + WINDOW);
        let sent = env.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].principal, "owner");
        assert!(!sent[1].device_usernames.contains(&"gina".to_string()));
    }

    /// The badge push rides its own budget class end to end.
    #[test]
    fn badge_events_draw_from_the_badge_budget() {
        let e = PushEvent::Badge(BadgePush {
            principal: "gina".into(),
            device_usernames: vec!["gina".into()],
            unread_total: "1".into(),
            waiting_total: "0".into(),
        });
        assert_eq!(e.budget_class(), k2_core::push::budget::BudgetClass::Badge);
    }
}
