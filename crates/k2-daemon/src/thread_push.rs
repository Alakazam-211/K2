//! Thread push trigger + coalescer (prd-thread-push-notifications-v1 §6,
//! slice S3; TP26, TP28, TP29).
//!
//! **Trigger (§6.1).** The three agent-reply sites in `overlay_routes`
//! (text reply, choice card, secret card) call [`on_agent_post`] next to
//! `thread_activity::note_reply`. A post pushes only when its `from` is not
//! a human (the owner, the owner's display name, or any Connect username —
//! TP14), it is not an app guest's (the callers never reach here for
//! guests, TP13), and air-gap is off (TP5).
//!
//! **Coalescing (§6.3, TP29).** One window per (conversation, recipient
//! PERSON): every push carries the recipient's `principal`, so a person's
//! devices get their own dispatch and nobody's name rides another's call.
//! - Leading edge: the first post in a quiet window pushes at once.
//! - Later posts within the window are held; when it closes, one trailing
//!   push says "N new messages" with the same `collapseId`, replacing the
//!   banner.
//! - Backoff: when the person has not opened the Thread since their last
//!   push, the next window is [`BACKOFF`] (5 min) instead of [`WINDOW`]
//!   (45 s). A `viewing` frame resets it to 45 s.
//! - Budget: each dispatch takes one token from the daemon's Thread bucket
//!   (30/h, 5/h on the shared relay token) per gateway call it makes.
//!   Over budget, the held count is kept and rides the next push.
//!
//! **Previews (0.46.1).** A push's body is a preview of the latest post
//! (`k2_core::push::preview`), never for a secret card. The owner's
//! `pushMessagePreviews` switch off → no preview for anyone. Otherwise each
//! phone's own `previews` choice decides; when one person's phones
//! disagree, the dispatch is TWO gateway calls (preview / generic) and
//! takes two tokens. If only one token is left, it goes out as ONE generic
//! call to all of that person's phones (nobody misses the push, no text
//! leaks to a phone that said no). PRIVACY: a preview passes through K2's
//! relay and Apple (APNs) or Google (FCM).
//!
//! **Viewing (TP26).** Checked at FLUSH time, not post time: a `viewing`
//! frame that lands before the window flushes still quiets the push.
//!
//! The [`Coalescer`] is pure state + an injected [`PushEnv`], so tests drive
//! it with a fake clock and a capture sink (no network, no sleeps). The
//! daemon holds one global instance ticked once a second.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_core::notify_prefs::{People, PrefsSnapshot, Recipient};
use k2_core::push::{PushEvent, ThreadMsgKind, ThreadPush};

/// The hold window after a push (Q9).
pub const WINDOW: Duration = Duration::from_secs(45);
/// The hold window when the person has not looked since their last push.
pub const BACKOFF: Duration = Duration::from_secs(5 * 60);
/// A window entry with no open window and no push for this long is dropped.
const FORGET_AFTER: Duration = Duration::from_secs(60 * 60);

/// One agent post, as the trigger sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    pub conversation_id: String,
    pub project_id: String,
    pub addr: String,
    pub agent_name: String,
    pub seq: i64,
    pub kind: ThreadMsgKind,
    /// The workspace's icon rev at post time (`data.avatarRev`; `""` = none).
    pub avatar_rev: String,
    /// The workspace's colour at post time (`data.agentColor`).
    pub agent_color: String,
    /// The post's message preview (`k2_core::push::preview::message_preview`
    /// of the text, or a choice card's question); `""` for a secret card or
    /// nothing readable. The raw text is never held.
    pub preview: String,
}

/// How one person's phones split on previews right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreviewSplit {
    /// Phones with previews on.
    pub on: usize,
    /// Phones with previews off.
    pub off: usize,
}

/// What the coalescer needs from the daemon.
pub trait PushEnv {
    /// Everyone who would get a push for `project_id` right now, ignoring
    /// who is viewing (§6.2 steps 1–3).
    fn recipients(&self, project_id: &str) -> Vec<Recipient>;
    /// Is `person` looking at `conversation_id` at `now`? (TP26, TP28)
    fn viewing(&self, person: &str, conversation_id: &str, now: Instant) -> bool;
    /// Take `calls` gateway calls from the Thread budget at `now`, all or
    /// nothing.
    fn take_budget(&self, now: Instant, calls: usize) -> bool;
    /// The owner's `pushMessagePreviews` switch.
    fn previews_allowed(&self) -> bool;
    /// How these logins' phones split on previews.
    fn preview_split(&self, device_usernames: &[String]) -> PreviewSplit;
    /// Send one dispatch (air-gap is checked again inside).
    fn send(&self, event: PushEvent);
    /// Air-gap on: nothing is held or sent.
    fn airgap(&self) -> bool;
}

#[derive(Debug, Clone)]
struct Window {
    post: Post,
    /// Posts not yet announced (held, or dropped over budget).
    pending: u32,
    /// When the current hold window ends; `None` = quiet.
    open_until: Option<Instant>,
    /// When the last push to this person for this Thread went out.
    last_push: Option<Instant>,
    /// The person viewed the Thread since `last_push`.
    viewed_since_push: bool,
    /// The person's device logins, from the latest resolution.
    devices: Vec<String>,
}

/// The per-(conversation, person) coalescing state.
#[derive(Debug, Default)]
pub struct Coalescer {
    windows: HashMap<(String, String), Window>,
}

impl Coalescer {
    pub fn new() -> Self {
        Self::default()
    }

    /// An agent posted. Leading edges flush at once; others are held.
    pub fn post(&mut self, env: &dyn PushEnv, post: Post, now: Instant) {
        if env.airgap() {
            return;
        }
        for r in env.recipients(&post.project_id) {
            let key = (post.conversation_id.clone(), r.person.clone());
            let w = self.windows.entry(key.clone()).or_insert_with(|| Window {
                post: post.clone(),
                pending: 0,
                open_until: None,
                last_push: None,
                viewed_since_push: false,
                devices: Vec::new(),
            });
            w.post = post.clone();
            w.devices = r.device_usernames.clone();
            w.pending += 1;
            let held = w.open_until.is_some_and(|t| now < t);
            if !held {
                self.flush(env, &key, now);
            }
        }
    }

    /// Flush every window whose hold time is over.
    pub fn tick(&mut self, env: &dyn PushEnv, now: Instant) {
        let due: Vec<_> = self
            .windows
            .iter()
            .filter(|(_, w)| w.open_until.is_some_and(|t| now >= t))
            .map(|(k, _)| k.clone())
            .collect();
        for key in due {
            self.flush(env, &key, now);
        }
        self.windows.retain(|_, w| {
            w.open_until.is_some() || w.last_push.is_some_and(|t| now.saturating_duration_since(t) < FORGET_AFTER)
        });
    }

    /// `person` viewed `conversation_id` (a `viewing` frame): their backoff
    /// resets, and an open 5-minute window shrinks back to 45 s from their
    /// last push (§6.3).
    pub fn note_view(&mut self, conversation_id: &str, person: &str) {
        if let Some(w) = self.windows.get_mut(&(conversation_id.to_string(), person.to_string())) {
            w.viewed_since_push = true;
            if let (Some(until), Some(last)) = (w.open_until, w.last_push) {
                w.open_until = Some(until.min(last + WINDOW));
            }
        }
    }

    /// Test/inspection: the hold length the window would use for its next
    /// push, and its pending count.
    #[cfg(test)]
    fn state(&self, conversation_id: &str, person: &str) -> Option<(u32, Option<Instant>, bool)> {
        self.windows
            .get(&(conversation_id.to_string(), person.to_string()))
            .map(|w| (w.pending, w.open_until, w.viewed_since_push))
    }

    fn flush(&mut self, env: &dyn PushEnv, key: &(String, String), now: Instant) {
        let Some(w) = self.windows.get_mut(key) else {
            return;
        };
        if w.pending == 0 {
            // A window closed with nothing held: quiet again (the backoff
            // memory — last push, viewed — stays).
            w.open_until = None;
            return;
        }
        let (conversation, person) = key;
        if env.airgap() {
            w.pending = 0;
            w.open_until = None;
            return;
        }
        // TP26: viewing is checked now, at send time. A person looking at
        // the Thread has seen these posts.
        if env.viewing(person, conversation, now) {
            w.pending = 0;
            w.open_until = None;
            w.viewed_since_push = true;
            return;
        }
        // Previews: the master switch, then this person's phones. A split
        // (some on, some off) is two gateway calls: one with the preview,
        // one generic.
        let mut preview = if env.previews_allowed() && w.post.kind != ThreadMsgKind::Secret {
            w.post.preview.clone()
        } else {
            String::new()
        };
        let mut calls = 1;
        if !preview.is_empty() {
            let split = env.preview_split(&w.devices);
            if split.on == 0 {
                preview.clear();
            } else if split.off > 0 {
                calls = 2;
            }
        }
        let mut granted = env.take_budget(now, calls);
        if !granted && calls == 2 {
            // One token left: one generic call to every phone.
            preview.clear();
            granted = env.take_budget(now, 1);
        }
        if !granted {
            // Over budget: keep the count for the next push and try again
            // after another window (logged by the env, never queued).
            crate::push_routes::note_budget_drop(k2_core::push::budget::BudgetClass::Thread);
            w.open_until = Some(now + WINDOW);
            return;
        }
        let len = if w.last_push.is_some() && !w.viewed_since_push { BACKOFF } else { WINDOW };
        let event = PushEvent::ThreadPost(ThreadPush {
            agent_name: w.post.agent_name.clone(),
            conversation_id: conversation.clone(),
            addr: w.post.addr.clone(),
            seq: w.post.seq,
            project_id: w.post.project_id.clone(),
            msg_kind: w.post.kind,
            count: w.pending,
            principal: person.clone(),
            device_usernames: w.devices.clone(),
            // The latest post's look: a trailing push shows the current avatar.
            avatar_rev: w.post.avatar_rev.clone(),
            agent_color: w.post.agent_color.clone(),
            // The latest post's preview ("N new messages · …" when count > 1).
            preview,
        });
        w.pending = 0;
        w.last_push = Some(now);
        w.viewed_since_push = false;
        w.open_until = Some(now + len);
        env.send(event);
    }
}

// ── The daemon's instance ─────────────────────────────────────────────

/// The real environment: people + prefs + devices from the daemon's
/// stores, the `thread_viewers` registry, the global Thread bucket, and
/// `push_routes::deliver_thread`.
pub struct DaemonEnv;

impl PushEnv for DaemonEnv {
    fn recipients(&self, project_id: &str) -> Vec<Recipient> {
        let people = match People::load() {
            Ok(p) => p,
            Err(e) => {
                k2_core::log_debug!("[thread_push] people load failed: {e}");
                return Vec::new();
            }
        };
        let devices = k2_core::push_devices::list_devices().unwrap_or_default();
        let db = k2_core::db::shared();
        let conn = db.lock();
        if !k2_core::notify_prefs::tables_exist(&conn) {
            return Vec::new();
        }
        let prefs = match PrefsSnapshot::load(&conn) {
            Ok(p) => p,
            Err(e) => {
                k2_core::log_debug!("[thread_push] prefs load failed: {e}");
                return Vec::new();
            }
        };
        k2_core::notify_prefs::recipients(&people, &prefs, &devices, project_id, &|_| false)
    }

    fn viewing(&self, person: &str, conversation_id: &str, now: Instant) -> bool {
        crate::thread_viewers::global().is_viewing(person, conversation_id, now)
    }

    fn take_budget(&self, now: Instant, calls: usize) -> bool {
        crate::push_routes::take_thread_budget(now, calls)
    }

    fn previews_allowed(&self) -> bool {
        crate::push_routes::previews_allowed()
    }

    fn preview_split(&self, device_usernames: &[String]) -> PreviewSplit {
        let devices = k2_core::push_devices::list_devices_for_usernames(device_usernames).unwrap_or_default();
        let on = devices.iter().filter(|d| d.previews).count();
        PreviewSplit { on, off: devices.len() - on }
    }

    fn send(&self, event: PushEvent) {
        crate::push_routes::deliver_thread(event);
    }

    fn airgap(&self) -> bool {
        crate::push_routes::airgap_blocks()
    }
}

/// The daemon-wide coalescer.
pub fn global() -> &'static Mutex<Coalescer> {
    static C: std::sync::OnceLock<Mutex<Coalescer>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(Coalescer::new()))
}

fn lock() -> std::sync::MutexGuard<'static, Coalescer> {
    global().lock().unwrap_or_else(|p| p.into_inner())
}

/// Start the once-a-second flusher (idempotent). Not under test: tests
/// drive [`Coalescer::tick`] with their own clock.
fn ensure_ticker() {
    #[cfg(not(test))]
    {
        static STARTED: std::sync::Once = std::sync::Once::new();
        STARTED.call_once(|| {
            let spawned = std::thread::Builder::new()
                .name("thread-push-tick".into())
                .spawn(|| loop {
                    std::thread::sleep(Duration::from_secs(1));
                    lock().tick(&DaemonEnv, Instant::now());
                });
            if let Err(e) = spawned {
                k2_core::log_debug!("[thread_push] ticker spawn failed: {e}");
            }
        });
    }
}

/// A person viewed a Thread (called by the viewers registry).
pub fn note_view(conversation_id: &str, person: &str) {
    lock().note_view(conversation_id, person);
}

/// The posting agent's display name: a workspace handle resolves to that
/// workspace's agent display name; anything else (a sidecar address, a
/// custom `from`) is shown as is.
fn agent_name_for(from: &str) -> String {
    let path: Option<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT path FROM projects WHERE handle = ?1 COLLATE NOCASE LIMIT 1",
            rusqlite::params![from.trim()],
            |r| r.get(0),
        )
        .ok()
    };
    match path {
        Some(p) => k2_core::workspace::display::agent_display_name(&p),
        None => from.trim().to_string(),
    }
}

/// A post's preview: never for a secret card.
fn post_preview(kind: ThreadMsgKind, text: &str) -> String {
    match kind {
        ThreadMsgKind::Secret => String::new(),
        ThreadMsgKind::Text | ThreadMsgKind::Choice => k2_core::push::preview::message_preview(text),
    }
}

/// True when `from` names a human on this server (TP14).
fn is_human_from(from: &str) -> bool {
    let owner = crate::workspace_msg::resolve_owner_from();
    match People::load() {
        Ok(p) => p.is_human_from(from, &owner),
        // Can't tell: fail closed (no push) rather than push a human's post.
        Err(_) => true,
    }
}

/// The trigger (§6.1). `from` is the stored `item.doc.from`; `text` is
/// the post's text (a choice card's question; ignored for a secret card),
/// kept only as its preview.
pub fn on_agent_post(
    conversation_id: &str,
    project_id: &str,
    addr: &str,
    from: &str,
    seq: i64,
    kind: ThreadMsgKind,
    text: &str,
) {
    if crate::push_routes::airgap_blocks() || is_human_from(from) {
        return;
    }
    // The same rev `GET /cli/projects/list` shows as this row's `iconRev`.
    let (agent_color, avatar_rev) = k2_core::projects_ops::project_push_look(project_id);
    let post = Post {
        conversation_id: conversation_id.to_string(),
        project_id: project_id.to_string(),
        addr: addr.to_string(),
        agent_name: agent_name_for(from),
        seq,
        kind,
        avatar_rev,
        agent_color,
        preview: post_preview(kind, text),
    };
    ensure_ticker();
    lock().post(&DaemonEnv, post, Instant::now());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::HashSet;

    /// Fake env: fixed recipients, a viewer set, a budget counter, and a
    /// capture of every dispatch.
    #[derive(Default)]
    struct Fake {
        people: Vec<(String, Vec<String>)>,
        viewers: RefCell<HashSet<(String, String)>>,
        budget: Cell<Option<u32>>,
        sent: RefCell<Vec<ThreadPush>>,
        airgap: Cell<bool>,
        /// The owner's master switch (`None` = default on).
        previews_off: Cell<bool>,
        /// Logins whose phones have previews OFF.
        previews_off_logins: RefCell<HashSet<String>>,
        /// Gateway calls taken from the budget, in order.
        calls_taken: RefCell<Vec<usize>>,
    }

    impl Fake {
        fn with(people: &[(&str, &[&str])]) -> Self {
            Self {
                people: people
                    .iter()
                    .map(|(p, d)| (p.to_string(), d.iter().map(|s| s.to_string()).collect()))
                    .collect(),
                ..Default::default()
            }
        }

        fn sent(&self) -> Vec<ThreadPush> {
            self.sent.borrow().clone()
        }
    }

    impl PushEnv for Fake {
        fn recipients(&self, _project_id: &str) -> Vec<Recipient> {
            self.people
                .iter()
                .map(|(p, d)| Recipient { person: p.clone(), device_usernames: d.clone() })
                .collect()
        }
        fn viewing(&self, person: &str, conversation_id: &str, _now: Instant) -> bool {
            self.viewers.borrow().contains(&(person.to_string(), conversation_id.to_string()))
        }
        fn take_budget(&self, _now: Instant, calls: usize) -> bool {
            let calls32 = calls as u32;
            let ok = match self.budget.get() {
                None => true,
                Some(n) if n < calls32 => false,
                Some(n) => {
                    self.budget.set(Some(n - calls32));
                    true
                }
            };
            if ok {
                self.calls_taken.borrow_mut().push(calls);
            }
            ok
        }
        fn previews_allowed(&self) -> bool {
            !self.previews_off.get()
        }
        fn preview_split(&self, device_usernames: &[String]) -> PreviewSplit {
            let off_set = self.previews_off_logins.borrow();
            let off = device_usernames.iter().filter(|u| off_set.contains(*u)).count();
            PreviewSplit { on: device_usernames.len() - off, off }
        }
        fn send(&self, event: PushEvent) {
            match event {
                PushEvent::ThreadPost(t) => self.sent.borrow_mut().push(t),
                other => panic!("coalescer sent a non-thread event: {other:?}"),
            }
        }
        fn airgap(&self) -> bool {
            self.airgap.get()
        }
    }

    fn post(conv: &str, seq: i64, kind: ThreadMsgKind) -> Post {
        Post {
            conversation_id: conv.into(),
            project_id: "p1".into(),
            addr: "k2".into(),
            agent_name: "K2".into(),
            seq,
            kind,
            avatar_rev: "0123456789abcdef".into(),
            agent_color: "#22c55e".into(),
            preview: String::new(),
        }
    }

    fn said(conv: &str, seq: i64, kind: ThreadMsgKind, text: &str) -> Post {
        Post { preview: post_preview(kind, text), ..post(conv, seq, kind) }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// Leading and trailing pushes carry the latest post's look, so a
    /// trailing push after an avatar change shows the new one.
    #[test]
    fn pushes_carry_the_latest_posts_avatar_rev_and_color() {
        let env = Fake::with(&[("owner", &["owner"])]);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.post(&env, post("c-look", 1, ThreadMsgKind::Text), t0);
        let mut changed = post("c-look", 2, ThreadMsgKind::Text);
        changed.avatar_rev = "fedcba9876543210".into();
        changed.agent_color = "#ef4444".into();
        c.post(&env, changed, t0 + secs(5));
        c.tick(&env, t0 + WINDOW);
        let sent = env.sent();
        assert_eq!(sent.len(), 2, "leading + trailing");
        assert_eq!((sent[0].avatar_rev.as_str(), sent[0].agent_color.as_str()), ("0123456789abcdef", "#22c55e"));
        assert_eq!((sent[1].avatar_rev.as_str(), sent[1].agent_color.as_str()), ("fedcba9876543210", "#ef4444"));
        let data = PushEvent::ThreadPost(sent[1].clone()).data();
        assert_eq!(data.get("avatarRev"), Some("fedcba9876543210"));
        assert_eq!(data.get("agentColor"), Some("#ef4444"));
    }

    #[test]
    fn five_posts_in_45s_make_exactly_two_sends_with_one_collapse_id() {
        let env = Fake::with(&[("owner", &["owner", "z3thon"])]);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        for i in 0..5u64 {
            c.post(&env, post("c1", 100 + i as i64, ThreadMsgKind::Text), t0 + secs(i * 5));
        }
        assert_eq!(env.sent().len(), 1, "leading edge only so far");
        c.tick(&env, t0 + secs(44));
        assert_eq!(env.sent().len(), 1, "window still open");
        c.tick(&env, t0 + WINDOW);
        let sent = env.sent();
        assert_eq!(sent.len(), 2, "leading + trailing");
        assert_eq!((sent[0].count, sent[0].seq), (1, 100));
        assert_eq!((sent[1].count, sent[1].seq), (4, 104), "trailing: 4 new messages, last seq");
        let a = PushEvent::ThreadPost(sent[0].clone());
        let b = PushEvent::ThreadPost(sent[1].clone());
        assert_eq!(b.body(), "4 new messages", "posts without text: the count alone");
        assert_eq!(a.collapse_id(), b.collapse_id(), "the trailing push replaces the banner");
        assert_eq!(sent[1].device_usernames, vec!["owner".to_string(), "z3thon".to_string()]);
        // Nothing more after the next (quiet) window closes.
        c.tick(&env, t0 + WINDOW + BACKOFF + secs(1));
        assert_eq!(env.sent().len(), 2);
    }

    #[test]
    fn unread_threads_back_off_to_5_minutes_and_a_view_resets_to_45s() {
        let env = Fake::with(&[("owner", &["owner"])]);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.post(&env, post("c2", 1, ThreadMsgKind::Text), t0); // push #1, 45 s window
        c.post(&env, post("c2", 2, ThreadMsgKind::Text), t0 + secs(10)); // held
        c.tick(&env, t0 + WINDOW); // push #2 (trailing); not viewed since #1 → 5 min
        assert_eq!(env.sent().len(), 2);
        let (_, until, _) = c.state("c2", "owner").expect("window");
        assert_eq!(until, Some(t0 + WINDOW + BACKOFF), "no view: the next window is 5 min");
        c.post(&env, post("c2", 3, ThreadMsgKind::Text), t0 + WINDOW + secs(60));
        c.tick(&env, t0 + WINDOW + secs(120));
        assert_eq!(env.sent().len(), 2, "held inside the 5-minute window");
        // A viewing frame (then leaving) resets the window to 45 s from the
        // last push; the held post goes out on the next tick.
        c.note_view("c2", "owner");
        let (_, until, viewed) = c.state("c2", "owner").expect("window");
        assert!(viewed);
        assert_eq!(until, Some(t0 + WINDOW + WINDOW), "reset to 45 s");
        c.tick(&env, t0 + WINDOW + secs(121));
        let sent = env.sent();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[2].seq, 3);
        // Viewed since the previous push → this push's window is 45 s.
        let (_, until, _) = c.state("c2", "owner").expect("window");
        assert_eq!(until, Some(t0 + WINDOW + secs(121) + WINDOW));
    }

    /// TP26: viewing is checked when the window FLUSHES — a frame that
    /// arrives after the post but before the flush still quiets it; and a
    /// viewer of conversation A does not quiet conversation B.
    #[test]
    fn viewing_quiets_at_flush_time_and_only_that_thread() {
        let env = Fake::with(&[("owner", &["owner"])]);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.post(&env, post("ca", 1, ThreadMsgKind::Text), t0); // leading, sent
        c.post(&env, post("ca", 2, ThreadMsgKind::Text), t0 + secs(5)); // held
        env.viewers.borrow_mut().insert(("owner".into(), "ca".into()));
        c.tick(&env, t0 + WINDOW);
        assert_eq!(env.sent().len(), 1, "the person looked before the flush: no trailing push");
        // A viewer at post time is quiet for the leading edge too.
        c.post(&env, post("ca", 3, ThreadMsgKind::Text), t0 + secs(400));
        assert_eq!(env.sent().len(), 1);
        // Another Thread is not quieted.
        c.post(&env, post("cb", 1, ThreadMsgKind::Choice), t0 + secs(400));
        let sent = env.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!((sent[1].conversation_id.as_str(), sent[1].msg_kind), ("cb", ThreadMsgKind::Choice));
    }

    /// TP29: two people on one Thread get two dispatches, each with its own
    /// principal and only their own devices; windows are per person.
    #[test]
    fn two_people_get_two_dispatches_each_with_its_own_principal() {
        let env = Fake::with(&[("owner", &["owner", "z3thon"]), ("gina", &["gina"])]);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.post(&env, post("c3", 7, ThreadMsgKind::Secret), t0);
        let sent = env.sent();
        assert_eq!(sent.len(), 2);
        let by: HashMap<_, _> = sent.iter().map(|t| (t.principal.clone(), t.clone())).collect();
        assert_eq!(by["owner"].device_usernames, vec!["owner".to_string(), "z3thon".to_string()]);
        assert_eq!(by["gina"].device_usernames, vec!["gina".to_string()]);
        let stamp = k2_core::push::PushStamp { subdomain: "rosson".into(), instance_id: "i".into() };
        for t in &sent {
            let payload = k2_core::push::K2Cloud::payload(&PushEvent::ThreadPost(t.clone()), &stamp, &t.principal);
            assert_eq!(payload["data"]["principal"], t.principal.as_str());
            let other = if t.principal == "owner" { "gina" } else { "z3thon" };
            assert!(!payload.to_string().contains(other), "{other} leaked into {}'s payload", t.principal);
        }
        // gina looking quiets only gina.
        env.viewers.borrow_mut().insert(("gina".into(), "c3".into()));
        c.post(&env, post("c3", 8, ThreadMsgKind::Text), t0 + secs(5));
        c.tick(&env, t0 + WINDOW);
        let sent = env.sent();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[2].principal, "owner");
    }

    /// Over budget: nothing sent, the count kept; the next push says how
    /// many it covers. Each dispatch (per person) takes one token.
    #[test]
    fn budget_drops_carry_into_the_next_push() {
        let env = Fake::with(&[("owner", &["owner"]), ("gina", &["gina"])]);
        env.budget.set(Some(1));
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.post(&env, post("c4", 1, ThreadMsgKind::Text), t0);
        assert_eq!(env.sent().len(), 1, "one token: one person's dispatch, the other dropped");
        let dropped = if env.sent()[0].principal == "owner" { "gina" } else { "owner" };
        assert_eq!(c.state("c4", dropped).expect("window").0, 1, "count kept");
        c.post(&env, post("c4", 2, ThreadMsgKind::Text), t0 + secs(5));
        env.budget.set(Some(10));
        c.tick(&env, t0 + WINDOW);
        let last = env.sent().into_iter().filter(|t| t.principal == dropped).last().expect("dropped person later pushed");
        assert_eq!((last.count, last.seq), (2, 2), "the dropped post rides the next push");
    }

    #[test]
    fn airgap_means_zero_sends() {
        let env = Fake::with(&[("owner", &["owner"])]);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.post(&env, post("c5", 1, ThreadMsgKind::Text), t0);
        c.post(&env, post("c5", 2, ThreadMsgKind::Text), t0 + secs(1)); // held
        env.airgap.set(true); // flipped at runtime
        c.tick(&env, t0 + WINDOW);
        c.post(&env, post("c5", 3, ThreadMsgKind::Text), t0 + secs(500));
        assert_eq!(env.sent().len(), 1, "only the pre-air-gap leading edge");
        env.airgap.set(false);
        c.post(&env, post("c5", 4, ThreadMsgKind::Text), t0 + secs(1000));
        assert_eq!(env.sent().len(), 2, "air-gap off again: pushes resume");
    }

    #[test]
    fn no_recipients_means_no_window_and_no_send() {
        let env = Fake::with(&[]);
        let mut c = Coalescer::new();
        c.post(&env, post("c6", 1, ThreadMsgKind::Text), Instant::now());
        assert!(env.sent().is_empty());
        assert!(c.state("c6", "owner").is_none());
    }

    // ── Previews (0.46.1) ────────────────────────────────────────────

    /// Leading push: the post's preview. Trailing push: "N new messages ·
    /// <latest preview>". The updated "4 new messages" test, with text.
    #[test]
    fn previews_ride_the_leading_and_trailing_pushes() {
        let env = Fake::with(&[("owner", &["owner"])]);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.post(&env, said("pv1", 1, ThreadMsgKind::Text, "## Done\n\nThe **build** is green. Shipping next. Then lunch."), t0);
        for i in 0..3u64 {
            c.post(&env, said("pv1", 2 + i as i64, ThreadMsgKind::Text, &format!("Step {} finished.", i + 2)), t0 + secs(5 + i));
        }
        c.tick(&env, t0 + WINDOW);
        let sent = env.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(PushEvent::ThreadPost(sent[0].clone()).body(), "Done — The build is green.");
        assert_eq!(PushEvent::ThreadPost(sent[1].clone()).body(), "3 new messages · Step 4 finished.");
        assert_eq!(*env.calls_taken.borrow(), vec![1, 1]);
    }

    /// A secret card never carries text — even as the latest of a
    /// coalesced batch; a choice card shows its question.
    #[test]
    fn secret_posts_never_preview_and_choices_show_the_question() {
        assert_eq!(post_preview(ThreadMsgKind::Secret, "the key is hunter2"), "");
        let env = Fake::with(&[("owner", &["owner"])]);
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.post(&env, said("pv2", 1, ThreadMsgKind::Choice, "Ship **0.46.1** tonight? Or wait for QA."), t0);
        c.post(&env, said("pv2", 2, ThreadMsgKind::Text, "Some text."), t0 + secs(1));
        c.post(&env, said("pv2", 3, ThreadMsgKind::Secret, "the key is hunter2"), t0 + secs(2));
        c.tick(&env, t0 + WINDOW);
        let sent = env.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(PushEvent::ThreadPost(sent[0].clone()).body(), "Ship 0.46.1 tonight? Or wait for QA.");
        let trailing = PushEvent::ThreadPost(sent[1].clone());
        assert_eq!(trailing.body(), "2 new messages", "the latest is a secret: no preview");
        assert!(sent[1].preview.is_empty());
        let stamp = k2_core::push::PushStamp::default();
        assert!(!k2_core::push::K2Cloud::payload(&trailing, &stamp, "owner").to_string().contains("hunter2"));
    }

    /// The owner's switch off → every push is generic and one call.
    #[test]
    fn master_switch_off_means_generic_for_everyone() {
        let env = Fake::with(&[("owner", &["owner", "z3thon"]), ("gina", &["gina"])]);
        env.previews_off.set(true);
        let mut c = Coalescer::new();
        c.post(&env, said("pv3", 1, ThreadMsgKind::Text, "Private numbers: 42."), Instant::now());
        let sent = env.sent();
        assert_eq!(sent.len(), 2);
        for t in &sent {
            assert!(t.preview.is_empty(), "{t:?}");
            assert_eq!(PushEvent::ThreadPost(t.clone()).body(), "New message in Thread");
        }
        assert_eq!(*env.calls_taken.borrow(), vec![1, 1]);
    }

    /// One person's phones disagree → the dispatch takes TWO budget calls
    /// and carries the preview (K2Cloud splits it per device). All off →
    /// generic, one call. Only one token left → one generic call.
    #[test]
    fn per_device_previews_take_one_call_per_payload_variant() {
        let env = Fake::with(&[("owner", &["owner", "z3thon"]), ("gina", &["gina"])]);
        env.previews_off_logins.borrow_mut().insert("z3thon".into());
        env.previews_off_logins.borrow_mut().insert("gina".into());
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.post(&env, said("pv4", 1, ThreadMsgKind::Text, "Ready for review."), t0);
        let by: HashMap<_, _> = env.sent().into_iter().map(|t| (t.principal.clone(), t)).collect();
        assert_eq!(by["owner"].preview, "Ready for review.", "owner's phones split: preview carried");
        assert!(by["gina"].preview.is_empty(), "gina's only phone said no: generic");
        let mut taken = env.calls_taken.borrow().clone();
        taken.sort();
        assert_eq!(taken, vec![1, 2], "split person = 2 calls, the other = 1");

        // Budget for one call only: the split person still gets ONE push,
        // generic, and nothing more is spent.
        let env = Fake::with(&[("owner", &["owner", "z3thon"])]);
        env.previews_off_logins.borrow_mut().insert("z3thon".into());
        env.budget.set(Some(1));
        let mut c = Coalescer::new();
        c.post(&env, said("pv5", 1, ThreadMsgKind::Text, "Ready for review."), t0);
        let sent = env.sent();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].preview.is_empty(), "fallback is generic: no text to the phone that said no");
        assert_eq!(*env.calls_taken.borrow(), vec![1]);
        assert_eq!(env.budget.get(), Some(0));

        // And the split payloads themselves: K2Cloud sends the preview to
        // the on-phone and the generic body to the off-phone.
        let event = PushEvent::ThreadPost(ThreadPush { preview: "Ready for review.".into(), ..sent[0].clone() });
        let dev = |t: &str, previews: bool| k2_core::push_devices::PushDevice {
            device_id: format!("d-{t}"),
            platform: "apns".into(),
            token: t.into(),
            username: "owner".into(),
            created_at: 1,
            updated_at: 1,
            last_seen_at: 1,
            previews,
        };
        let devices = [dev("on", true), dev("off", false)];
        let groups = k2_core::push::K2Cloud::payload_groups(&event, &devices);
        let shape: Vec<(String, Vec<String>)> =
            groups.iter().map(|(e, d)| (e.body(), d.iter().map(|x| x.token.clone()).collect())).collect();
        assert_eq!(
            shape,
            vec![
                ("Ready for review.".to_string(), vec!["on".to_string()]),
                ("New message in Thread".to_string(), vec!["off".to_string()]),
            ]
        );
    }
}
