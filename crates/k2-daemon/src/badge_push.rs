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
//! **Read hints (push-read-hints-v1).** When a person's cursor moves forward
//! ([`read_moved`]), the chat rides their next badge push as a `read` entry,
//! so a phone can clear that chat's notifications with the app closed. The
//! newest 8 go, highest seq per chat, and a push holding one isn't skipped
//! for unchanged totals. The same limits apply: one push per person per
//! 10 s, the badge budget, the per-daemon path, a drop is never retried.
//!
//! An entry is noted only if an alert may be showing for that chat. The
//! desktop posts a read for every new seq while a chat is open, and every own
//! post moves the cursor too, so without this one person chatting would send
//! a push every 10 s (360/h against a whole server's 240/h). [`ChatMark`]
//! keeps that in memory: a chat with no mark sends once (an alert from before
//! a restart may still be showing), then it takes a new alert. The cost: a
//! hint lost on the way isn't resent by a later read of that chat. Opening
//! the app, or the unread total reaching 0, still clears it on the phone.
//!
//! The [`Coalescer`] is pure state + an injected [`BadgeEnv`] (fake clock
//! and capture sink in tests). The daemon ticks one global instance.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_core::push::{BadgePush, PushEvent, ReadEntry, ReadHint, ReadKind};
use k2_core::read_state::Totals;

/// At most one badge push per person per window.
pub const WINDOW: Duration = Duration::from_secs(10);
/// How long a stale-only note waits to merge into one push.
pub const STALE_WINDOW: Duration = Duration::from_secs(60);
/// Forget a person whose last push is this old and who has nothing due.
const FORGET_AFTER: Duration = Duration::from_secs(60 * 60);

/// Most `read` entries held for one person; older ones go. Only 8 ride a push,
/// so the push already says `readMore`.
const READ_HELD_MAX: usize = 64;
/// Most chats one person keeps a [`ChatMark`] for. Past it they're all
/// forgotten, which means "unknown" and errs toward sending.
const READ_MARKS_MAX: usize = 256;

/// What a person's phones may still be showing for one chat.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ChatMark {
    /// The seq of the newest alert sent to this person.
    alerted_through: i64,
    /// The highest seq sent in a push carrying a `read` entry.
    hinted_through: i64,
}

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
    /// Chats read since the last push, oldest first (push-read-hints-v1).
    read: Vec<ReadEntry>,
    /// Which chats may still have an alert showing.
    marks: HashMap<(ReadKind, String), ChatMark>,
}

impl Entry {
    fn mark(&mut self, kind: ReadKind, id: &str) -> &mut ChatMark {
        let chat = (kind, id.to_string());
        if self.marks.len() >= READ_MARKS_MAX && !self.marks.contains_key(&chat) {
            self.marks.clear();
        }
        self.marks.entry(chat).or_default()
    }

    /// Is an alert possibly showing for this chat? No record means it may be.
    fn may_show_alert(&self, kind: ReadKind, id: &str) -> bool {
        self.marks.get(&(kind, id.to_string())).is_none_or(|m| m.alerted_through > m.hinted_through)
    }
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

    /// The phones got these totals another way: a badge push with the same
    /// numbers is skipped.
    #[cfg(test)]
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

    /// `person`'s cursor in this chat moved to `seq`. Run [`Self::trigger`]
    /// next under the same lock, or the entry can sit with nothing due to
    /// send it.
    pub fn note_read(&mut self, person: &str, kind: ReadKind, id: &str, seq: i64) {
        let e = self.people.entry(person.to_string()).or_default();
        if !e.may_show_alert(kind, id) {
            return;
        }
        let same = |r: &ReadEntry| r.kind == kind && r.id == id;
        let seq = e.read.iter().find(|r| same(r)).map_or(seq, |r| r.seq.max(seq));
        e.read.retain(|r| !same(r));
        e.read.push(ReadEntry { kind, id: id.to_string(), seq });
        if e.read.len() > READ_HELD_MAX {
            e.read.remove(0);
        }
    }

    /// An alert for this chat, at `seq`, went to `person`: a read of it can
    /// clear something now, and the phones have its totals (`None` when it
    /// carried none), so a badge push with the same ones is skipped. A
    /// pending `read` entry stays.
    pub fn note_alert_sent(&mut self, person: &str, kind: ReadKind, id: &str, seq: i64, totals: Option<Totals>) {
        let e = self.people.entry(person.to_string()).or_default();
        if totals.is_some() {
            e.last_told = totals;
        }
        let m = e.mark(kind, id);
        m.alerted_through = m.alerted_through.max(seq);
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
                || !e.marks.is_empty()
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
        // The same for read entries: the phone's next open clears them.
        let read = std::mem::take(&mut e.read);
        if env.blocked() {
            return;
        }
        let Some(totals) = env.totals(person) else { return };
        // Newest note first, with each chat's newest seq now.
        let pairs: Vec<(String, i64)> =
            stale.iter().rev().filter_map(|c| env.latest_seq(c).map(|s| (c.clone(), s))).collect();
        let read = ReadHint::of(read.into_iter().rev().collect());
        if e.last_told == Some(totals) && pairs.is_empty() && read.is_empty() {
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
        for r in &read.entries {
            let m = e.mark(r.kind, &r.id);
            m.hinted_through = m.hinted_through.max(r.seq);
        }
        env.send(PushEvent::Badge(BadgePush {
            principal: person.to_string(),
            device_usernames: devices,
            unread_total: totals.total.to_string(),
            waiting_total: totals.waiting_total.to_string(),
            stale: k2_core::push::StaleHint::of(pairs),
            read,
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

/// `person`'s cursor in this chat moved forward to `seq`: their phones may
/// clear its notifications. Off the caller's thread; the note and the trigger
/// run under one lock, so a flush can't slip between them.
pub fn read_moved(person: &str, kind: &str, id: &str, seq: i64) {
    read_moved_many(person, kind, vec![(id.to_string(), seq)]);
}

/// [`read_moved`] for several chats of one kind at once (Mark all as read):
/// one push carries them, not one per chat.
pub fn read_moved_many(person: &str, kind: &str, chats: Vec<(String, i64)>) {
    #[cfg(test)]
    test_read_capture::capture(person, kind, &chats);
    let kind = ReadKind::of_cursor(kind);
    let person = person.to_string();
    run_off_thread(move || {
        ensure_ticker();
        let now = Instant::now();
        let mut c = lock();
        if let Some(kind) = kind {
            for (id, seq) in &chats {
                c.note_read(&person, kind, id, *seq);
            }
        }
        c.trigger(&DaemonEnv, &person, now);
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

/// A Thread alert for this chat, at `seq`, went to `person`. It runs inside
/// the Thread coalescer, never under the DB lock. Everything else uses
/// [`alert_sent`].
pub fn note_alert_sent(person: &str, kind: ReadKind, id: &str, seq: i64, totals: Totals) {
    lock().note_alert_sent(person, kind, id, seq, Some(totals));
}

/// What one alert tells the coalescer: who, which chat, which seq, and the
/// totals it carried. `None` for an alert that names no read state.
fn alert_note(event: &PushEvent) -> Option<(String, ReadKind, String, i64, Option<Totals>)> {
    let totals = |total: &str, waiting: &str| Some(Totals { total: total.parse().ok()?, waiting_total: waiting.parse().ok()? });
    match event {
        PushEvent::ThreadPost(t) => Some((
            t.principal.clone(),
            ReadKind::Thread,
            t.conversation_id.clone(),
            t.seq,
            totals(&t.unread_total, &t.waiting_total),
        )),
        PushEvent::ProjectPost(p) => {
            Some((p.principal.clone(), ReadKind::Project, p.group_id.clone(), p.seq, totals(&p.unread_total, &p.waiting_total)))
        }
        _ => None,
    }
}

/// A project alert or permission card, past its budget, went out. Off the
/// caller's thread: the tick holds this lock while it waits for the DB, so a
/// note made under the DB lock could deadlock.
pub fn alert_sent(event: &PushEvent) {
    let Some(note) = alert_note(event) else { return };
    #[cfg(test)]
    test_read_capture::capture_alert(note.clone());
    run_off_thread(move || {
        let (person, kind, id, seq, totals) = note;
        lock().note_alert_sent(&person, kind, &id, seq, totals);
    });
}

/// What [`read_moved`] was told, for the route tests (the daemon wiring is a
/// no-op under test).
#[cfg(test)]
pub(crate) mod test_read_capture {
    use std::sync::Mutex;

    static CAPTURED: Mutex<Vec<(String, String, String, i64)>> = Mutex::new(Vec::new());

    pub(crate) fn capture(person: &str, kind: &str, chats: &[(String, i64)]) {
        let mut all = CAPTURED.lock().expect("read capture lock");
        all.extend(chats.iter().map(|(id, seq)| (person.to_string(), kind.to_string(), id.clone(), *seq)));
    }

    /// Everything noted for chats in `ids`, in order.
    pub(crate) fn for_chats(ids: &[&str]) -> Vec<(String, String, String, i64)> {
        CAPTURED.lock().expect("read capture lock").iter().filter(|c| ids.contains(&c.2.as_str())).cloned().collect()
    }

    type AlertNote = (String, k2_core::push::ReadKind, String, i64, Option<k2_core::read_state::Totals>);

    static ALERTS: Mutex<Vec<AlertNote>> = Mutex::new(Vec::new());

    pub(crate) fn capture_alert(note: AlertNote) {
        ALERTS.lock().expect("alert capture lock").push(note);
    }

    /// Every alert noted for this chat, in order.
    pub(crate) fn alerts_for(id: &str) -> Vec<AlertNote> {
        ALERTS.lock().expect("alert capture lock").iter().filter(|a| a.2 == id).cloned().collect()
    }
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

    fn told(total: i64) -> Totals {
        Totals { total, waiting_total: 0 }
    }

    /// What `read_moved` does: the note, then the trigger.
    fn read(c: &mut Coalescer, env: &Fake, kind: ReadKind, id: &str, seq: i64, now: Instant) {
        c.note_read("gina", kind, id, seq);
        c.trigger(env, "gina", now);
    }

    fn wires(env: &Fake) -> Vec<String> {
        env.sent().iter().map(|b| b.read.wire()).collect()
    }

    /// Test 10: the first read in a quiet window goes at once. Later ones
    /// merge into one trailing push: highest seq per chat, newest read first.
    #[test]
    fn read_entries_merge_over_the_window() {
        let env = Fake::with("gina", &["gina"], 2, 0);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        read(&mut c, &env, ReadKind::Thread, "c-1", 40, t0);
        assert_eq!(wires(&env), ["t:c-1:40"], "the first goes at once");
        c.note_alert_sent("gina", ReadKind::Thread, "c-1", 45, None);
        c.note_alert_sent("gina", ReadKind::Project, "g-1", 5, None);
        c.note_alert_sent("gina", ReadKind::Thread, "c-2", 5, None);
        read(&mut c, &env, ReadKind::Thread, "c-1", 42, t0 + Duration::from_secs(1));
        read(&mut c, &env, ReadKind::Project, "g-1", 5, t0 + Duration::from_secs(2));
        read(&mut c, &env, ReadKind::Thread, "c-2", 3, t0 + Duration::from_secs(3));
        read(&mut c, &env, ReadKind::Thread, "c-1", 41, t0 + Duration::from_secs(4));
        assert_eq!(env.sent().len(), 1, "held inside the window");
        assert_eq!(c.due("gina"), Some(t0 + WINDOW));
        c.tick(&env, t0 + WINDOW);
        assert_eq!(env.sent().len(), 2, "one trailing push");
        assert_eq!(env.sent()[1].read.wire(), "t:c-1:42,t:c-2:3,p:g-1:5", "highest seq per chat; a re-read moves it to newest");
        assert!(!env.sent()[1].read.more);
        c.tick(&env, t0 + WINDOW * 3);
        assert_eq!(env.sent().len(), 2, "nothing left to send");
    }

    /// Over 8: the newest go with `readMore`. Over the 64 held: the oldest go.
    #[test]
    fn more_than_eight_entries_set_read_more() {
        let env = Fake::with("gina", &["gina"], 2, 0);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        for i in 0..9 {
            c.note_read("gina", ReadKind::Thread, &format!("c-{i}"), 10 + i);
        }
        c.trigger(&env, "gina", t0);
        let sent = env.sent();
        assert_eq!(sent.len(), 1, "one push, not one per chat");
        assert_eq!(sent[0].read.entries.len(), 8);
        assert_eq!(sent[0].read.entries[0].id, "c-8", "newest first");
        assert!(sent[0].read.more);
        // The one left out wasn't told: its next read may try again.
        read(&mut c, &env, ReadKind::Thread, "c-0", 99, t0 + WINDOW);
        assert_eq!(env.sent()[1].read.wire(), "t:c-0:99");

        let mut c = Coalescer::new();
        let env = Fake::with("gina", &["gina"], 2, 0);
        for i in 0..(READ_HELD_MAX as i64 + 1) {
            c.note_read("gina", ReadKind::Thread, &format!("c-{i}"), 1);
        }
        assert_eq!(c.people["gina"].read.len(), READ_HELD_MAX);
        assert!(c.people["gina"].read.iter().all(|r| r.id != "c-0"), "the oldest goes");
        c.trigger(&env, "gina", t0);
        assert!(env.sent()[0].read.more);
    }

    /// An entry the list can't hold doesn't make a push, and never holds a
    /// real one back.
    #[test]
    fn an_id_the_list_cant_hold_never_forces_a_push() {
        let env = Fake::with("gina", &["gina"], 2, 0);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.note_told("gina", told(2));
        read(&mut c, &env, ReadKind::Thread, "a,b", 5, t0);
        assert!(env.sent().is_empty(), "same totals and nothing to name: skipped");
        read(&mut c, &env, ReadKind::Thread, "c-1", 5, t0 + Duration::from_secs(1));
        assert_eq!(wires(&env), ["t:c-1:5"]);
    }

    /// Test 11 (D2): an entry beats unchanged totals. Without one it's
    /// skipped and costs no budget.
    #[test]
    fn a_read_entry_goes_though_the_totals_are_unchanged() {
        let env = Fake::with("gina", &["gina"], 2, 0);
        env.budget.set(Some(1));
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.note_told("gina", told(2));
        c.trigger(&env, "gina", t0);
        assert!(env.sent().is_empty(), "no entry, same numbers: skipped");
        assert_eq!(env.budget.get(), Some(1), "a skip takes no budget");
        read(&mut c, &env, ReadKind::Thread, "c-1", 7, t0 + Duration::from_secs(1));
        assert_eq!(wires(&env), ["t:c-1:7"], "same totals still go: the entry is new");
        assert_eq!(env.sent()[0].unread_total, "2");
        assert_eq!(env.budget.get(), Some(0), "it takes one token");
    }

    /// Test 12: the storm. An alert, then 30 reads or own posts over five
    /// minutes while chatting along: exactly one entry goes out. This fails
    /// with the filter removed: about 30 pushes.
    #[test]
    fn chatting_along_sends_one_hint_not_a_stream() {
        let env = Fake::with("gina", &["gina"], 2, 0);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.note_told("gina", told(2));
        c.note_alert_sent("gina", ReadKind::Thread, "c-1", 40, None);
        for i in 0..30 {
            read(&mut c, &env, ReadKind::Thread, "c-1", 40 + i, t0 + WINDOW * i as u32);
            c.tick(&env, t0 + WINDOW * i as u32);
        }
        c.tick(&env, t0 + WINDOW * 60);
        assert_eq!(wires(&env), ["t:c-1:40"], "one entry for the one alert");

        // A new alert makes the next read worth sending again, once.
        c.note_alert_sent("gina", ReadKind::Thread, "c-1", 80, None);
        let t1 = t0 + WINDOW * 100;
        read(&mut c, &env, ReadKind::Thread, "c-1", 80, t1);
        read(&mut c, &env, ReadKind::Thread, "c-1", 81, t1 + WINDOW * 2);
        assert_eq!(wires(&env), ["t:c-1:40", "t:c-1:80"]);

        // A read short of the alert leaves it showing, so the next one goes too.
        c.note_alert_sent("gina", ReadKind::Thread, "c-1", 100, None);
        let t2 = t1 + WINDOW * 100;
        read(&mut c, &env, ReadKind::Thread, "c-1", 90, t2);
        read(&mut c, &env, ReadKind::Thread, "c-1", 100, t2 + WINDOW * 2);
        read(&mut c, &env, ReadKind::Thread, "c-1", 101, t2 + WINDOW * 4);
        assert_eq!(wires(&env)[2..], ["t:c-1:90", "t:c-1:100"]);
    }

    /// Test 13: a chat this daemon knows nothing about sends once (an alert
    /// from before a restart may still be showing), then stays quiet.
    #[test]
    fn an_unknown_chat_sends_once_then_quiet() {
        let env = Fake::with("gina", &["gina"], 2, 0);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.note_told("gina", told(2));
        for i in 0..5 {
            read(&mut c, &env, ReadKind::Project, "g-1", 10 + i, t0 + WINDOW * (2 * i as u32));
        }
        assert_eq!(wires(&env), ["p:g-1:10"]);
        // Past the mark limit everything is unknown again: it sends once more.
        for i in 0..READ_MARKS_MAX {
            c.note_alert_sent("gina", ReadKind::Thread, &format!("c-{i}"), 1, None);
        }
        read(&mut c, &env, ReadKind::Project, "g-1", 20, t0 + WINDOW * 40);
        assert_eq!(wires(&env), ["p:g-1:10", "p:g-1:20"]);
    }

    /// Test 14: alert, then mute, then read: the totals don't move but the
    /// entry goes. Muted with no alert since the last hint: nothing.
    #[test]
    fn a_muted_chat_clears_after_its_alert_and_stays_quiet_without_one() {
        let env = Fake::with("gina", &["gina"], 2, 0);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.note_told("gina", told(2));
        c.note_alert_sent("gina", ReadKind::Thread, "c-m", 9, None);
        // A mute goes through `trigger` alone: it isn't a read.
        c.trigger(&env, "gina", t0);
        assert!(env.sent().is_empty(), "the mute itself sends nothing");
        read(&mut c, &env, ReadKind::Thread, "c-m", 9, t0 + Duration::from_secs(1));
        assert_eq!(wires(&env), ["t:c-m:9"]);
        assert_eq!(env.sent()[0].unread_total, "2", "the totals never moved");
        read(&mut c, &env, ReadKind::Thread, "c-m", 12, t0 + WINDOW * 2);
        assert_eq!(env.sent().len(), 1, "no alert since: nothing to clear");
    }

    /// Test 15: over budget the entries are gone and nothing is queued, but
    /// the chat wasn't told, so the next forward read sends.
    #[test]
    fn a_budget_drop_loses_the_entry_but_the_next_read_tries_again() {
        let env = Fake::with("gina", &["gina"], 2, 0);
        env.budget.set(Some(0));
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.note_told("gina", told(2));
        c.note_alert_sent("gina", ReadKind::Thread, "c-1", 5, None);
        read(&mut c, &env, ReadKind::Thread, "c-1", 5, t0);
        assert!(env.sent().is_empty(), "over budget: dropped");
        assert_eq!(c.due("gina"), None, "nothing queued");
        assert!(c.people["gina"].read.is_empty(), "the entry is gone");
        c.tick(&env, t0 + WINDOW * 4);
        assert!(env.sent().is_empty());
        env.budget.set(Some(1));
        read(&mut c, &env, ReadKind::Thread, "c-1", 6, t0 + WINDOW * 5);
        assert_eq!(wires(&env), ["t:c-1:6"], "the next read tries again");
    }

    /// Test 16: blocked or no phones: nothing goes, nothing is held, and the
    /// chat stays worth telling.
    #[test]
    fn blocked_or_no_devices_send_nothing_and_hold_nothing() {
        let t0 = Instant::now();
        let env = Fake::with("gina", &[], 2, 0);
        let mut c = Coalescer::new();
        read(&mut c, &env, ReadKind::Thread, "c-1", 5, t0);
        assert!(env.sent().is_empty(), "no phones");
        assert!(c.people["gina"].read.is_empty());

        let env = Fake::with("gina", &["gina"], 2, 0);
        env.blocked.set(true);
        let mut c = Coalescer::new();
        read(&mut c, &env, ReadKind::Thread, "c-1", 5, t0);
        assert!(env.sent().is_empty(), "air-gap / no per-daemon path");
        assert!(c.people["gina"].read.is_empty());
        env.blocked.set(false);
        c.tick(&env, t0 + WINDOW * 4);
        assert!(env.sent().is_empty(), "nothing was held");
        read(&mut c, &env, ReadKind::Thread, "c-1", 6, t0 + WINDOW * 5);
        assert_eq!(wires(&env), ["t:c-1:6"], "never told, so it's still worth telling");
    }

    /// An alert's own note leaves a pending entry alone (a stale note goes,
    /// a read entry doesn't), and read and stale hints ride one push.
    #[test]
    fn an_alert_never_clears_a_pending_read_entry() {
        let env = Fake::with("gina", &["gina"], 2, 0);
        env.seqs.borrow_mut().insert("c-2".into(), 8);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.note_told("gina", told(2));
        read(&mut c, &env, ReadKind::Thread, "c-1", 3, t0);
        c.note_read("gina", ReadKind::Thread, "c-9", 4);
        c.note_stale("gina", "c-1", t0);
        c.note_stale("gina", "c-2", t0);
        c.note_alerted("gina", "c-1");
        c.note_alert_sent("gina", ReadKind::Thread, "c-9", 5, None);
        c.trigger(&env, "gina", t0 + Duration::from_secs(1));
        c.tick(&env, t0 + WINDOW);
        let last = env.sent().last().cloned().expect("a push");
        assert_eq!(last.read.wire(), "t:c-9:4");
        assert_eq!(last.stale.pairs, vec![("c-2".to_string(), 8)]);
    }

    /// A person with only marks is kept, or the filter would forget them.
    #[test]
    fn marks_outlive_the_idle_sweep() {
        let env = Fake::with("gina", &[], 2, 0);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.note_alert_sent("gina", ReadKind::Thread, "c-1", 5, None);
        c.tick(&env, t0 + FORGET_AFTER * 2);
        assert_eq!(c.people["gina"].marks.len(), 1);
    }

    /// Test 17: told 5, an alert tells 6, a read brings it back to 5: a push
    /// goes with 5. This was skipped before, so the phone's badge stayed at 6.
    #[test]
    fn a_read_after_an_alert_corrects_the_badge() {
        for kind in [ReadKind::Project, ReadKind::Thread] {
            let env = Fake::with("gina", &["gina"], 5, 0);
            let mut c = Coalescer::new();
            let t0 = Instant::now();
            c.note_told("gina", told(5));
            c.trigger(&env, "gina", t0);
            assert!(env.sent().is_empty(), "the phone has 5");
            c.note_alert_sent("gina", kind, "chat", 6, Some(told(6)));
            c.trigger(&env, "gina", t0 + Duration::from_secs(1));
            let sent = env.sent();
            assert_eq!(sent.len(), 1, "{kind:?}: the phone has 6 now, so a 5 corrects it");
            assert_eq!(sent[0].unread_total, "5");
        }
        // An alert with no totals leaves what the phone was told.
        let env = Fake::with("gina", &["gina"], 5, 0);
        let mut c = Coalescer::new();
        c.note_told("gina", told(5));
        c.note_alert_sent("gina", ReadKind::Project, "chat", 6, None);
        c.trigger(&env, "gina", Instant::now());
        assert!(env.sent().is_empty());
    }

    /// Each alert that carries totals maps to one note.
    #[test]
    fn an_alert_note_names_the_person_chat_seq_and_totals() {
        let thread = k2_core::push::ThreadPush {
            agent_name: "a".into(),
            conversation_id: "c-1".into(),
            addr: "a".into(),
            seq: 41,
            project_id: "p".into(),
            msg_kind: k2_core::push::ThreadMsgKind::Choice,
            count: 1,
            principal: "gina".into(),
            device_usernames: vec!["gina".into()],
            unread_total: "6".into(),
            waiting_total: "1".into(),
            avatar_rev: String::new(),
            agent_color: String::new(),
            preview: String::new(),
            msg: None,
            budget: k2_core::push::ThreadBudget::Thread,
        };
        let totals = Some(Totals { total: 6, waiting_total: 1 });
        assert_eq!(
            alert_note(&PushEvent::ThreadPost(thread.clone())),
            Some(("gina".to_string(), ReadKind::Thread, "c-1".to_string(), 41, totals))
        );
        let project = k2_core::push::ProjectPush {
            project_name: "n".into(),
            group_id: "g-1".into(),
            seq: 8,
            principal: "gina".into(),
            device_usernames: vec!["gina".into()],
            unread_total: "6".into(),
            waiting_total: "1".into(),
        };
        assert_eq!(
            alert_note(&PushEvent::ProjectPost(project)),
            Some(("gina".to_string(), ReadKind::Project, "g-1".to_string(), 8, totals))
        );
        // Totals that aren't numbers aren't told, but the alert still counts.
        let odd = k2_core::push::ThreadPush { unread_total: String::new(), ..thread };
        assert_eq!(alert_note(&PushEvent::ThreadPost(odd)).map(|n| n.4), Some(None));
        assert_eq!(alert_note(&PushEvent::FeedbackCommented { feedback_id: "f".into(), assignees: vec![] }), None);
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
            read: Default::default(),
        });
        assert_eq!(e.budget_class(), k2_core::push::budget::BudgetClass::Badge);
    }
}
