//! Agent-reply pushes to App guests (prd-custom-app-push-v1 §9, CP34),
//! **off by default per App** (`k2 app push set <app> --agent-replies on`).
//!
//! When an agent posts a text reply in a room's pinned Chat, each App with
//! agent replies on (and push on) notifies its own guests who hold
//! `thread:read` in that room and have devices, minus guests who muted that
//! Thread (their `guest:<principal>` read state) and guests viewing it.
//! Choice and secret cards never push to a guest (guests never see them).
//!
//! - **Coalescing** per (App, conversation, guest), the Thread push's shape:
//!   the first post in a quiet window pushes at once; later posts in the
//!   [`WINDOW`] are held, and when it closes one trailing "N new messages"
//!   push replaces the banner (same collapse id). Viewing is checked again
//!   at flush.
//! - **Budget**: its own per-App bucket ([`PER_APP_HOUR`]) and per-guest
//!   bucket ([`PER_GUEST_HOUR`]), so a chatty agent never spends the App's
//!   `push.send` budget.
//! - **Text** (CP31): the agent's display name as the title; the body is the
//!   post's preview only when the owner's `pushMessagePreviews`, the App's
//!   `previews` and the device's `previews` are all on, else "New message".
//! - `data`: `k2Kind: agent`, `k2Room` (the room's handle), `k2Seq`,
//!   `k2Conv` (guests already see that id through `thread.*`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::skin::app_push as store;

use super::budget::Limiter;
use super::payload::{self, Notification, Stamps};
use super::sendlog::{self, SendRecord, Skipped};
use super::sender::Target;
use super::worker::{self, Job};

/// The hold window after a push (the Thread push's 45 s).
pub const WINDOW: Duration = Duration::from_secs(45);
/// A window with nothing held and no push for this long is forgotten.
const FORGET_AFTER: Duration = Duration::from_secs(3600);
/// Agent-reply device sends per App an hour.
pub const PER_APP_HOUR: usize = 2_000;
/// Agent-reply pushes per guest per App an hour.
pub const PER_GUEST_HOUR: usize = 30;
/// The body when previews are off.
pub const GENERIC: &str = "New message";

/// One coalescing key: (App, conversation, guest).
pub type Key = (String, String, String);

#[derive(Debug, Clone)]
struct Win {
    opened: Instant,
    held: u32,
    /// The latest held post (for the trailing push's data).
    last: Option<Post>,
}

/// What a push says about the post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    pub room: String,
    pub room_handle: String,
    pub conversation: String,
    pub agent_name: String,
    pub seq: i64,
    pub preview: String,
}

/// The coalescer: pure state, driven with an explicit clock.
#[derive(Default)]
pub struct Windows {
    map: HashMap<Key, Win>,
}

/// What to do for one key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Push this post now (the leading edge).
    Now,
    /// Held in an open window.
    Held,
}

impl Windows {
    /// A post for `key` at `now`.
    pub fn post(&mut self, key: Key, post: &Post, now: Instant) -> Act {
        match self.map.get_mut(&key) {
            Some(w) if now.saturating_duration_since(w.opened) < WINDOW => {
                w.held += 1;
                w.last = Some(post.clone());
                Act::Held
            }
            _ => {
                self.map.insert(key, Win { opened: now, held: 0, last: None });
                Act::Now
            }
        }
    }

    /// Windows that closed with posts held: `(key, held, last post)`. Each
    /// opens a new window from `now` (the trailing push is a push).
    pub fn due(&mut self, now: Instant) -> Vec<(Key, u32, Post)> {
        let mut out = Vec::new();
        self.map.retain(|k, w| {
            let age = now.saturating_duration_since(w.opened);
            if age >= WINDOW && w.held > 0 {
                if let Some(p) = w.last.take() {
                    out.push((k.clone(), w.held, p));
                }
                w.held = 0;
                w.opened = now;
                return true;
            }
            age < FORGET_AFTER
        });
        out
    }
}

fn windows() -> &'static Mutex<Windows> {
    static W: OnceLock<Mutex<Windows>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(Windows::default()))
}

fn per_app() -> &'static Limiter {
    static L: OnceLock<Limiter> = OnceLock::new();
    L.get_or_init(|| Limiter::new(Duration::from_secs(3600)))
}

fn per_guest() -> &'static Limiter {
    static L: OnceLock<Limiter> = OnceLock::new();
    L.get_or_init(|| Limiter::new(Duration::from_secs(3600)))
}

fn guest_key(pid: &str) -> String {
    format!("{}{pid}", crate::read_routes::GUEST_KEY_PREFIX)
}

fn muted(pid: &str, conversation: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    // A read error is not a mute: the guest still gets the push.
    k2_core::read_state::is_muted(&conn, &guest_key(pid), k2_core::read_state::KIND_THREAD, conversation, k2_core::read_state::now_secs())
        .unwrap_or(false)
}

fn viewing(pid: &str, conversation: &str) -> bool {
    crate::thread_viewers::global().is_viewing(&guest_key(pid), conversation, Instant::now())
}

/// One agent post, queued for the fan-out thread.
struct Event {
    conversation: String,
    room: String,
    agent_name: String,
    seq: i64,
    preview: String,
}

/// Agent posts waiting for the fan-out thread; past this, a post's guest
/// pushes are skipped (logged), never queued without bound.
pub const MAX_QUEUED_POSTS: usize = 1_024;

fn events() -> &'static Mutex<std::sync::mpsc::SyncSender<Event>> {
    static TX: OnceLock<Mutex<std::sync::mpsc::SyncSender<Event>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Event>(MAX_QUEUED_POSTS);
        let spawned = std::thread::Builder::new().name("app-push-replies".into()).spawn(move || {
            let mut last_flush = Instant::now();
            loop {
                match rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(e) => fan_out(&e.conversation, &e.room, &e.agent_name, e.seq, &e.preview),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
                if last_flush.elapsed() >= Duration::from_secs(1) {
                    flush(Instant::now());
                    last_flush = Instant::now();
                }
            }
        });
        if spawned.is_err() {
            super::sender::log("the agent-reply thread didn't start");
        }
        Mutex::new(tx)
    })
}

/// The trigger (`thread_push::on_agent_post`, text replies only). It only
/// queues the post: the recipients' lookups and the coalescing run on the
/// agent-reply thread, never on the agent's post request (review B-L3).
pub fn on_agent_post(conversation: &str, room: &str, agent_name: &str, seq: i64, preview: &str) {
    if !super::box_enabled() || crate::push_routes::airgap_blocks() {
        return;
    }
    let e = Event { conversation: conversation.into(), room: room.into(), agent_name: agent_name.into(), seq, preview: preview.into() };
    match events().lock().unwrap_or_else(|p| p.into_inner()).try_send(e) {
        Ok(()) => {}
        Err(std::sync::mpsc::TrySendError::Full(_)) => {
            super::sender::log(&format!("agent reply not pushed: {MAX_QUEUED_POSTS} posts already wait for the agent-reply thread"));
        }
        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => super::sender::log("agent reply not queued: the agent-reply thread is gone"),
    }
}

/// The fan-out for one post (the agent-reply thread).
fn fan_out(conversation: &str, room: &str, agent_name: &str, seq: i64, preview: &str) {
    if !super::box_enabled() || crate::push_routes::airgap_blocks() {
        return;
    }
    let apps = match store::apps_with_agent_replies() {
        Ok(a) if !a.is_empty() => a,
        Ok(_) => return,
        Err(e) => {
            super::sender::log(&format!("agent replies: {e}"));
            return;
        }
    };
    // Only a room's pinned Chat (what guests read through thread.*).
    if crate::overlay_routes::pinned_chat_of(room).as_deref() != Some(conversation) {
        return;
    }
    let handle = k2_core::workspace_session_handles::workspace_address_name_shared(room).unwrap_or_else(|_| room.to_string());
    let post = Post {
        room: room.to_string(),
        room_handle: handle,
        conversation: conversation.to_string(),
        agent_name: agent_name.to_string(),
        seq,
        preview: preview.to_string(),
    };
    for app in apps {
        if super::state(&app).is_err() {
            continue;
        }
        let guests = match store::room_guests(&app, room, Some(k2_core::skin::CAP_THREAD_READ)) {
            Ok(g) => g,
            Err(e) => {
                super::sender::log(&format!("{app} agent replies: {e}"));
                continue;
            }
        };
        let now = Instant::now();
        for pid in guests {
            if muted(&pid, conversation) {
                continue;
            }
            let act = windows().lock().unwrap_or_else(|e| e.into_inner()).post((app.clone(), conversation.to_string(), pid.clone()), &post, now);
            if act == Act::Now && !viewing(&pid, conversation) {
                deliver(&app, &pid, &post, 1);
            }
        }
    }
}

/// Flush closed windows (the ticker, once a second; tests call it).
pub fn flush(now: Instant) {
    let due = windows().lock().unwrap_or_else(|e| e.into_inner()).due(now);
    for ((app, conversation, pid), held, post) in due {
        if super::state(&app).is_err() || muted(&pid, &conversation) || viewing(&pid, &conversation) {
            continue;
        }
        deliver(&app, &pid, &post, held);
    }
}

/// The notification for one post: `count` = 1 for the leading push, the
/// held count for a trailing one. `preview` = the device may see the text.
pub fn notification(post: &Post, count: u32, preview: bool) -> Notification {
    let body = if count > 1 {
        format!("{count} new messages")
    } else if preview && !post.preview.trim().is_empty() {
        post.preview.clone()
    } else {
        GENERIC.to_string()
    };
    let mut data = std::collections::BTreeMap::new();
    data.insert("k2Room".to_string(), post.room_handle.clone());
    data.insert("k2Seq".to_string(), post.seq.to_string());
    data.insert("k2Conv".to_string(), post.conversation.clone());
    let title: String = post.agent_name.chars().take(100).collect();
    let body: String = body.chars().take(300).collect();
    Notification {
        title: Some(title),
        body: Some(body),
        data,
        topic: None,
        collapse_id: Some(format!("k2:{}", &post.conversation.chars().take(40).collect::<String>())),
        thread_id: Some(format!("k2:{}", post.room_handle.chars().take(100).collect::<String>())),
        sound: Some("default".into()),
        badge: None,
        silent: false,
        high_priority: true,
        ttl_secs: payload::DEFAULT_TTL_SECS,
        mutable: false,
        android_channel: None,
    }
}

/// Queue one guest's push (per device: preview or generic).
fn deliver(app: &str, pid: &str, post: &Post, count: u32) {
    let devices = match store::targets(app, &[pid.to_string()]) {
        Ok(d) if !d.is_empty() => d,
        _ => return,
    };
    let cfg = match store::config(app) {
        Ok(c) => c,
        Err(_) => return,
    };
    let configured = |d: &store::PushDevice| (d.platform == store::PLATFORM_APNS && cfg.apns.is_some()) || (d.platform == store::PLATFORM_FCM && cfg.fcm.is_some());
    let sendable: Vec<&store::PushDevice> = devices.iter().filter(|d| configured(d)).collect();
    // The App's bucket before the guest's (review B-L5).
    if sendable.is_empty() || per_app().take_n(app, sendable.len(), PER_APP_HOUR).is_err() {
        return;
    }
    if per_guest().take(&format!("{app}\0{pid}"), PER_GUEST_HOUR).is_err() {
        return;
    }
    let Ok(who) = super::who(app, pid) else { return };
    let server_previews = crate::push_routes::previews_allowed() && cfg.previews;
    let with_text = Arc::new(notification(post, count, true));
    let generic = Arc::new(notification(post, count, false));
    let send_id = payload::new_send_id();
    let jobs: Vec<Job> = sendable
        .iter()
        .map(|d| Job {
            target: Target::from(*d),
            notification: if server_previews && d.previews != Some(false) { with_text.clone() } else { generic.clone() },
            stamps: Stamps { app: app.to_string(), send_id: send_id.clone(), kind: payload::KIND_AGENT, who: who.clone() },
        })
        .collect();
    let queued = jobs.len();
    sendlog::start(app, SendRecord::new(&send_id, "k2:agent-reply", "agent-reply", 1, None, queued, Skipped::default()));
    if worker::enqueue(app, jobs).is_err() {
        sendlog::refused(app, &send_id, "push_queue_full");
    }
}

#[cfg(test)]
#[path = "agent_reply_tests.rs"]
mod tests;
