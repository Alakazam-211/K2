//! Who is looking at which Thread (prd-thread-push-notifications-v1 §6.4,
//! slice S4; TP16, TP26, TP28).
//!
//! A client on the overlay WS (`/cli/overlay/events?conversation=`) sends
//! `{"t":"viewing","visible":true|false}` text frames: the desktop while
//! its Thread pane is visible and focused, the phone while its Thread
//! screen is in the foreground, repeated every 30 s. Fire-and-forget: no
//! ack. The daemon keeps them as STATE with a [`VIEW_TTL`] of 90 s
//! (TP28): `(person key, conversation) → visible_until`. `false`, or the
//! socket closing, clears that socket's entry.
//!
//! The coalescer asks [`Viewers::is_viewing`] when a window FLUSHES (TP26),
//! so a frame that lands before the flush still quiets the push. A view
//! also resets that person's backoff for the Thread
//! (`thread_push::note_view`). v1 quiets only THAT Thread (Q8).
//!
//! Identity: the dispatcher resolves the socket's login (owner token →
//! `owner`, a Connect session → its username); the person key is computed
//! per frame, so a role change or a new owner link applies at once.
//!
//! **App guests** (prd-tauri-core-0467-v1 TC-24, vs-live TA-23) register
//! under their read-state person key, `guest:<principalId>`
//! ([`on_guest_frame`]): from their own overlay socket's `viewing` frames,
//! and from `push.viewing` (an App server says its signed-in person is
//! looking). The App agent-reply push (`app_push::agent_reply`) asks this
//! registry under that key, so a guest who has the Thread open gets no
//! agent-reply push. They are never K2 Thread push recipients (TP13), so
//! the K2 person coalescer (`thread_push::note_view`) is not touched. An
//! App's own pass (`app:<serviceId>`) is not a person and never registers.
//!
//! **Last visible** (prd-agent-settings-cards-v1 rev 3.1 §6.7, C28): beside
//! the live views, the registry remembers when each (person, conversation)
//! was last seen looking ([`Viewers::last_visible_at`]): a permission card
//! files a Tickets item only when nobody who may answer it looked at that
//! Thread in the last 10 minutes. In memory only: after a restart it is
//! empty, so the ticket is filed (the safe side).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long one `visible:true` frame counts (TP28; clients repeat every 30 s).
pub const VIEW_TTL: Duration = Duration::from_secs(90);

type Key = (String, String);

/// The registry: per (person, conversation), each socket's expiry.
#[derive(Debug, Default)]
pub struct Viewers {
    map: Mutex<HashMap<Key, HashMap<u64, Instant>>>,
    /// When each (person, conversation) was last seen looking: the last
    /// `visible:true` frame, or the moment a live view ended (`false`, the
    /// socket closing).
    seen: Mutex<HashMap<Key, Instant>>,
}

impl Viewers {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Key, HashMap<u64, Instant>>> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Record a frame from `socket`. `visible` sets `now + VIEW_TTL`;
    /// `false` clears this socket's entry.
    pub fn set(&self, socket: u64, person: &str, conversation: &str, visible: bool, now: Instant) {
        let key = (person.to_string(), conversation.to_string());
        let mut map = self.lock();
        if visible {
            map.entry(key.clone()).or_default().insert(socket, now + VIEW_TTL);
            self.note_seen(key, now);
        } else if let Some(sockets) = map.get_mut(&key) {
            // A live view that ends now was visible until now.
            if sockets.get(&socket).is_some_and(|until| now < *until) {
                self.note_seen(key.clone(), now);
            }
            sockets.remove(&socket);
            if sockets.is_empty() {
                map.remove(&key);
            }
        }
    }

    fn note_seen(&self, key: Key, now: Instant) {
        let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        let at = seen.entry(key).or_insert(now);
        if *at < now {
            *at = now;
        }
    }

    /// When `person` was last seen looking at `conversation`: `now` while a
    /// view is live, else the last frame or view end. `None` = not since
    /// this daemon started.
    pub fn last_visible_at(&self, person: &str, conversation: &str, now: Instant) -> Option<Instant> {
        if self.is_viewing(person, conversation, now) {
            return Some(now);
        }
        self.seen
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&(person.to_string(), conversation.to_string()))
            .copied()
    }

    /// `person` looked at `conversation` within `window` before `now`.
    pub fn visible_within(&self, person: &str, conversation: &str, window: Duration, now: Instant) -> bool {
        self.last_visible_at(person, conversation, now).is_some_and(|at| now.saturating_duration_since(at) <= window)
    }

    /// True when any socket of `person` has a live view of `conversation`.
    pub fn is_viewing(&self, person: &str, conversation: &str, now: Instant) -> bool {
        self.lock()
            .get(&(person.to_string(), conversation.to_string()))
            .is_some_and(|s| s.values().any(|until| now < *until))
    }

    /// A socket closed: every entry it held goes.
    pub fn close_socket(&self, socket: u64) {
        let now = Instant::now();
        let mut map = self.lock();
        map.retain(|key, sockets| {
            if sockets.get(&socket).is_some_and(|until| now < *until) {
                self.note_seen(key.clone(), now);
            }
            sockets.remove(&socket);
            !sockets.is_empty()
        });
    }
}

/// The daemon-wide registry.
pub fn global() -> &'static Viewers {
    static V: std::sync::OnceLock<Viewers> = std::sync::OnceLock::new();
    V.get_or_init(Viewers::new)
}

/// A process-unique id for one overlay socket.
pub fn next_socket_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Parse a client text frame: `Some(visible)` for
/// `{"t":"viewing","visible":bool}`, `None` for anything else (ignored).
pub fn parse_viewing(text: &str) -> Option<bool> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("t").and_then(|t| t.as_str()) != Some("viewing") {
        return None;
    }
    v.get("visible").and_then(|b| b.as_bool())
}

/// Apply one `viewing` frame from a socket whose login is `login`.
pub fn on_frame(socket: u64, login: &str, conversation: &str, visible: bool) {
    let person = match k2_core::notify_prefs::People::load() {
        Ok(p) => p.person_key(login),
        Err(e) => {
            k2_core::log_debug!("[thread_viewers] people load failed: {e}");
            return;
        }
    };
    global().set(socket, &person, conversation, visible, Instant::now());
    if visible {
        crate::thread_push::note_view(conversation, &person);
    }
}

/// The registry slot of one pass's `push.viewing` calls (TC-24): stable
/// per pass, so a repeat refreshes it and `visible:false` clears it, and
/// never one of the sequential socket ids ([`next_socket_id`]: the top bit
/// is set).
pub fn verb_socket(pass_id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    "push.viewing".hash(&mut h);
    pass_id.hash(&mut h);
    h.finish() | (1 << 63)
}

/// Apply one `viewing` frame from an App guest's socket, under its person
/// key `guest:<principalId>` (TA-23). A key that is not a guest person key
/// (an App's own pass, a K2 login) is ignored: those never use this path.
pub fn on_guest_frame(socket: u64, guest_key: &str, conversation: &str, visible: bool) {
    if !is_guest_person_key(guest_key) {
        return;
    }
    global().set(socket, guest_key, conversation, visible, Instant::now());
}

/// `guest:<principalId>`, with a non-empty principal id.
pub fn is_guest_person_key(key: &str) -> bool {
    key.strip_prefix(crate::read_routes::GUEST_KEY_PREFIX).is_some_and(|p| !p.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_lasts_90_seconds_and_only_for_that_thread() {
        let v = Viewers::new();
        let t0 = Instant::now();
        v.set(1, "owner", "ca", true, t0);
        assert!(v.is_viewing("owner", "ca", t0 + Duration::from_secs(89)));
        assert!(!v.is_viewing("owner", "ca", t0 + VIEW_TTL), "expired after 90 s");
        assert!(!v.is_viewing("owner", "cb", t0), "another Thread is not quieted");
        assert!(!v.is_viewing("gina", "ca", t0), "another person is not quieted");
        // A repeat every 30 s keeps it alive.
        v.set(1, "owner", "ca", true, t0 + Duration::from_secs(60));
        assert!(v.is_viewing("owner", "ca", t0 + Duration::from_secs(120)));
    }

    #[test]
    fn false_and_socket_close_clear_only_that_socket() {
        let v = Viewers::new();
        let t0 = Instant::now();
        v.set(1, "owner", "ca", true, t0); // desktop
        v.set(2, "owner", "ca", true, t0); // phone
        v.set(1, "owner", "ca", false, t0);
        assert!(v.is_viewing("owner", "ca", t0), "the phone still looks");
        v.close_socket(2);
        assert!(!v.is_viewing("owner", "ca", t0), "socket close cleared it");
        v.set(3, "owner", "cx", true, t0);
        v.set(3, "owner", "cy", true, t0);
        v.close_socket(3);
        assert!(!v.is_viewing("owner", "cx", t0) && !v.is_viewing("owner", "cy", t0));
    }

    #[test]
    fn only_viewing_frames_parse() {
        assert_eq!(parse_viewing(r#"{"t":"viewing","visible":true}"#), Some(true));
        assert_eq!(parse_viewing(r#"{"t":"viewing","visible":false}"#), Some(false));
        assert_eq!(parse_viewing(r#"{"t":"viewing","visible":"yes"}"#), None);
        assert_eq!(parse_viewing(r#"{"t":"other","visible":true}"#), None);
        assert_eq!(parse_viewing("not json"), None);
    }

    /// rev 3.1 §6.7 (C28): the registry remembers when a person last looked
    /// at a Thread, live views and ended ones; another Thread or person
    /// never counts, and nothing is remembered before the first frame.
    #[test]
    fn last_visible_at_remembers_the_last_look_per_person_and_thread() {
        let v = Viewers::new();
        let t0 = Instant::now();
        let ten = Duration::from_secs(600);
        assert_eq!(v.last_visible_at("owner", "ca", t0), None, "nothing before the first frame (a restart files the ticket)");
        assert!(!v.visible_within("owner", "ca", ten, t0));
        v.set(1, "owner", "ca", true, t0);
        assert_eq!(v.last_visible_at("owner", "ca", t0 + Duration::from_secs(30)), Some(t0 + Duration::from_secs(30)), "live = now");
        // The view lapses (no repeat): last seen at the frame.
        let later = t0 + VIEW_TTL + Duration::from_secs(1);
        assert_eq!(v.last_visible_at("owner", "ca", later), Some(t0));
        assert!(v.visible_within("owner", "ca", ten, later));
        assert!(!v.visible_within("owner", "ca", ten, t0 + Duration::from_secs(601)), "older than 10 minutes");
        // `false` ends a live view now.
        let t1 = t0 + Duration::from_secs(1000);
        v.set(2, "gina", "ca", true, t1);
        v.set(2, "gina", "ca", false, t1 + Duration::from_secs(20));
        assert_eq!(v.last_visible_at("gina", "ca", t1 + Duration::from_secs(700)), Some(t1 + Duration::from_secs(20)));
        assert_eq!(v.last_visible_at("gina", "cb", t1), None, "another Thread");
        assert_eq!(v.last_visible_at("dana", "ca", t1), None, "another person");
        // A socket closing ends its live view now.
        let conv = format!("lv-{}", std::process::id());
        v.set(3, "lee", &conv, true, Instant::now());
        v.close_socket(3);
        assert!(!v.is_viewing("lee", &conv, Instant::now()));
        assert!(v.visible_within("lee", &conv, ten, Instant::now()), "seen just now");
    }

    /// TA-23: a guest frame lands under the guest's own person key, the
    /// key `app_push::agent_reply` asks; anything else is ignored.
    #[test]
    fn guest_frames_register_under_the_guest_person_key_only() {
        let conv = format!("gview-{}", uuid::Uuid::new_v4());
        let key = crate::read_routes::guest_key_of_principal("3f0c-pid");
        let socket = next_socket_id();
        on_guest_frame(socket, &key, &conv, true);
        assert!(global().is_viewing("guest:3f0c-pid", &conv, Instant::now()));
        assert!(!global().is_viewing("owner", &conv, Instant::now()), "never the owner");
        for bad in ["app:svc-1", "owner", "guest:", "guest:  "] {
            let s = next_socket_id();
            on_guest_frame(s, bad, &conv, true);
            assert!(!global().is_viewing(bad, &conv, Instant::now()), "{bad} is not a guest person key");
        }
        on_guest_frame(socket, &key, &conv, false);
        assert!(!global().is_viewing(&key, &conv, Instant::now()));
        global().close_socket(socket);
    }

    /// A frame from a linked login lands on the owner person, and resets
    /// the owner's backoff in the coalescer.
    #[test]
    fn frames_map_logins_to_person_keys() {
        let _home = crate::test_support::TempHome::new();
        k2_core::connect_users::add_user("viewz", "password123").expect("user");
        k2_core::app_settings::update(serde_json::json!({ "ownerAccounts": ["viewz"] })).expect("link");
        let conv = format!("view-{}", uuid::Uuid::new_v4());
        let socket = next_socket_id();
        on_frame(socket, "viewz", &conv, true);
        assert!(global().is_viewing("owner", &conv, Instant::now()), "the phone login is the owner person");
        assert!(!global().is_viewing("viewz", &conv, Instant::now()));
        on_frame(socket, "viewz", &conv, false);
        assert!(!global().is_viewing("owner", &conv, Instant::now()));
        global().close_socket(socket);
    }
}
