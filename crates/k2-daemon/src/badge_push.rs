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
//! **Stale hints (push-content-available-v1, 0.46.7).** A chat that changed
//! with no alert to this person (a human post, a card closing, a muted
//! chat, an agent post they were viewing on the desktop or that went over
//! the Thread budget) is noted with [`note_stale`]. Notes wait up to
//! [`STALE_WINDOW`] (60 s) and ride the person's next badge push as
//! `stale` (`conversationId:seq` pairs, newest first, at most 8; `seq` =
//! the chat's newest seq when the push is built). The same limits apply:
//! one push per person per 10 s, the badge budget, the per-daemon path,
//! a drop is never retried. An alert to the person for that chat clears
//! its note ([`note_alerted`]): the alert already carries it.
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
/// How long a stale-only note waits to merge into one push.
pub const STALE_WINDOW: Duration = Duration::from_secs(60);
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
    /// A chat's newest Thread seq now (`None` = unknown: left out).
    fn latest_seq(&self, conversation_id: &str) -> Option<i64>;
}

#[derive(Debug, Default, Clone)]
struct Entry {
    /// The totals this person's phones were last told.
    last_told: Option<Totals>,
    /// When the last badge push went out.
    last_send: Option<Instant>,
    /// A send scheduled for the end of the window.
    due: Option<Instant>,
    /// Chats that changed without an alert to this person, oldest note
    /// first (push-content-available-v1).
    stale: Vec<String>,
    /// When the stale notes go out on their own.
    stale_due: Option<Instant>,
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

    /// `conversation` changed without an alert to `person`: hint it on
    /// their next badge push, within [`STALE_WINDOW`].
    pub fn note_stale(&mut self, person: &str, conversation: &str, now: Instant) {
        let e = self.people.entry(person.to_string()).or_default();
        e.stale.retain(|c| c != conversation);
        e.stale.push(conversation.to_string());
        if e.stale_due.is_none() {
            e.stale_due = Some(now + STALE_WINDOW);
        }
    }

    /// An alert to `person` for `conversation` went out: its note is moot.
    pub fn note_alerted(&mut self, person: &str, conversation: &str) {
        if let Some(e) = self.people.get_mut(person) {
            e.stale.retain(|c| c != conversation);
            if e.stale.is_empty() {
                e.stale_due = None;
            }
        }
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
        // Stale notes whose wait is over: sent now, or at the end of the
        // person's 10 s window.
        let stale_due: Vec<String> = self
            .people
            .iter()
            .filter(|(_, e)| e.due.is_none() && e.stale_due.is_some_and(|t| now >= t))
            .map(|(p, _)| p.clone())
            .collect();
        for p in stale_due {
            let Some(e) = self.people.get_mut(&p) else { continue };
            match e.last_send {
                Some(t) if now.saturating_duration_since(t) < WINDOW => e.due = Some(t + WINDOW),
                _ => self.flush(env, &p, now),
            }
        }
        // A person's last-told totals are kept (one small entry per
        // person); only an idle entry with nothing to remember goes.
        self.people.retain(|_, e| {
            e.due.is_some()
                || e.stale_due.is_some()
                || e.last_told.is_some()
                || e.last_send.is_some_and(|t| now.saturating_duration_since(t) < FORGET_AFTER)
        });
    }

    fn flush(&mut self, env: &dyn BadgeEnv, person: &str, now: Instant) {
        let Some(e) = self.people.get_mut(person) else { return };
        e.due = None;
        // Whatever happens next, the stale notes go with this attempt: a
        // drop is never retried (the foreground catch-up is the guarantee).
        let stale = std::mem::take(&mut e.stale);
        e.stale_due = None;
        if env.blocked() {
            return;
        }
        let Some(totals) = env.totals(person) else { return };
        // Newest note first, with each chat's newest seq now.
        let pairs: Vec<(String, i64)> =
            stale.iter().rev().filter_map(|c| env.latest_seq(c).map(|s| (c.clone(), s))).collect();
        if e.last_told == Some(totals) && pairs.is_empty() {
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
            stale: k2_core::push::StaleHint::of(pairs),
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

    fn latest_seq(&self, conversation_id: &str) -> Option<i64> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT last_thread_seq FROM overlay_conversations WHERE conversation_id = ?1",
            rusqlite::params![conversation_id],
            |r| r.get::<_, i64>(0),
        )
        .ok()
        .filter(|s| *s > 0)
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

/// push-content-available-v1: `conversation` changed with no alert to
/// `person` (see the module docs). Off the caller's thread.
pub fn note_stale(person: &str, conversation: &str) {
    let (person, conversation) = (person.to_string(), conversation.to_string());
    run_off_thread(move || {
        ensure_ticker();
        lock().note_stale(&person, &conversation, Instant::now());
    });
}

/// A Thread alert to `person` for `conversation` went out.
pub fn note_alerted(person: &str, conversation: &str) {
    lock().note_alerted(person, conversation);
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
        seqs: RefCell<HashMap<String, i64>>,
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
        fn latest_seq(&self, conversation_id: &str) -> Option<i64> {
            self.seqs.borrow().get(conversation_id).copied()
        }
    }

    /// push-content-available-v1 (K3): stale notes wait 60 s and merge into
    /// ONE push (newest first, each chat's seq read at send time), even
    /// with unchanged totals; they ride an earlier badge push instead; an
    /// alert clears its chat; the 10 s window and the budget still apply,
    /// and a dropped hint is not retried.
    #[test]
    fn stale_notes_merge_into_one_push_within_the_window() {
        let env = Fake::with("gina", &["gina"], 2, 0);
        env.seqs.borrow_mut().extend([("c-1".to_string(), 40), ("c-2".to_string(), 7), ("c-3".to_string(), 9)]);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.note_told("gina", Totals { total: 2, waiting_total: 0 });
        c.note_stale("gina", "c-1", t0);
        c.note_stale("gina", "c-2", t0 + Duration::from_secs(5));
        c.note_stale("gina", "c-3", t0 + Duration::from_secs(6));
        c.note_stale("gina", "c-1", t0 + Duration::from_secs(10));
        c.note_alerted("gina", "c-3");
        c.tick(&env, t0 + Duration::from_secs(59));
        assert!(env.sent().is_empty(), "nothing before the window ends");
        env.seqs.borrow_mut().insert("c-1".into(), 42);
        c.tick(&env, t0 + STALE_WINDOW);
        let sent = env.sent();
        assert_eq!(sent.len(), 1, "one push for every note");
        assert_eq!(sent[0].stale.pairs, vec![("c-1".to_string(), 42), ("c-2".to_string(), 7)], "newest first, seq at send");
        assert_eq!(sent[0].unread_total, "2", "same totals still go: the hint is new");
        assert_eq!(sent[0].device_usernames, vec!["gina".to_string()]);
        c.tick(&env, t0 + STALE_WINDOW + Duration::from_secs(61));
        assert_eq!(env.sent().len(), 1, "a sent hint is gone");

        // A badge push inside the window carries the notes along.
        let env = Fake::with("gina", &["gina"], 3, 0);
        env.seqs.borrow_mut().insert("c-9".into(), 5);
        let mut c = Coalescer::new();
        c.note_stale("gina", "c-9", t0);
        c.trigger(&env, "gina", t0 + Duration::from_secs(1));
        assert_eq!(env.sent().len(), 1);
        assert_eq!(env.sent()[0].stale.pairs, vec![("c-9".to_string(), 5)]);
        c.tick(&env, t0 + STALE_WINDOW + Duration::from_secs(1));
        assert_eq!(env.sent().len(), 1, "nothing left to send");

        // A chat with no known seq is left out; with nothing left and the
        // same totals, nothing is sent.
        let env = Fake::with("gina", &["gina"], 4, 0);
        let mut c = Coalescer::new();
        c.note_told("gina", Totals { total: 4, waiting_total: 0 });
        c.note_stale("gina", "c-unknown", t0);
        c.tick(&env, t0 + STALE_WINDOW);
        assert!(env.sent().is_empty());

        // Over budget: dropped, never retried.
        let env = Fake::with("gina", &["gina"], 4, 0);
        env.seqs.borrow_mut().insert("c-6".into(), 3);
        env.budget.set(Some(0));
        let mut c = Coalescer::new();
        c.note_stale("gina", "c-6", t0);
        c.tick(&env, t0 + STALE_WINDOW);
        c.tick(&env, t0 + STALE_WINDOW * 3);
        assert!(env.sent().is_empty());

        // Blocked (air-gap / no per-daemon path) or no phones: nothing.
        let env = Fake::with("gina", &[], 4, 0);
        env.seqs.borrow_mut().insert("c-7".into(), 3);
        let mut c = Coalescer::new();
        c.note_stale("gina", "c-7", t0);
        c.tick(&env, t0 + STALE_WINDOW);
        assert!(env.sent().is_empty());
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
            stale: Default::default(),
        });
        assert_eq!(e.budget_class(), k2_core::push::budget::BudgetClass::Badge);
    }
}
