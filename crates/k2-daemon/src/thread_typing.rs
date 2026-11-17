//! Who is typing in a Thread (thread-typing-v1, 0.46.15; Rosson via K2
//! Design 2026-11-17). Sew's typing rules, which already work smoothly
//! (sew-server `gateway/conn.rs` `typing`, sew-proto `TYPING_INTERVAL_MS`).
//!
//! **Client → daemon.** On the Thread socket (`WS /cli/overlay/events?
//! conversation=`), beside `{"t":"viewing"}`, a client sends
//! `{"t":"typing","thread":"<thread address>"}` at most once every 8 s
//! while its person types. There is no stop frame, ever. No ack, no error
//! frame. The frame is dropped silently when:
//! - this connection already sent one in the last [`INTERVAL`] (8 s): the
//!   extra is dropped and counted ([`throttled_total`]), never queued. The
//!   key is the connection's Thread (one Thread per socket; a sidecar's
//!   Thread is its own conversation, so its own key);
//! - nobody posts in this Thread ([`takes_posts`]: imported history, a Grok
//!   Bot's Thread, a Voice & Chat only conversation), or the sender may not
//!   post in it: an App guest needs `thread:post` in the room
//!   ([`pass_may_post`], Sew's SEND_MESSAGES rule); a login on this socket
//!   is a Member or above, which posts;
//! - `thread` does not name this socket's own Thread ([`address_if_names`]:
//!   its conversation id, its current address, or a name it had before);
//!   a socket is only ever open on a Thread its caller may read, so a
//!   frame for any other Thread is never relayed;
//! - the sender is not a person ([`typist_of_login`], [`typist_of_pass`]):
//!   agents never type (their passports can't open this socket at all, and
//!   a reserved login such as `agent` is nobody), and an App's own pass is
//!   a service account, not a person;
//! - an App guest's pass, as it resolves NOW, no longer reads this Thread
//!   (the socket's own `guest_now` check).
//!
//! **The hub path** (hub-viewing-v1 servers: a Thread viewed through `WS
//! /cli/sessions/events` has no overlay socket): the same client frame,
//! `{"t":"typing","thread":"<addr>","conversationId":"<id>"?}`, on the hub
//! socket. `conversationId` names the Thread (else the socket's current
//! view). Same rules: a login on the hub (Member floor) reads and posts in
//! every Thread but those [`takes_posts`] refuses, the same 8 s throttle
//! per Thread per connection, lossy.
//!
//! **Daemon → clients.** One frame on its own broadcast. On the overlay
//! socket: `{"collection":"typing","seq":0,"id":<thread address>,
//! "member":<member>,"name":<display name>,"kind":"person"|"app_guest",
//! "memberKind":<same as kind>,"at":<ms>}`. On the hub, to the sockets
//! viewing that Thread: the same keys, but `kind` is the hub's frame tag
//! `"thread_typing"` (as `thread_activity`), plus `conversation_id`; the
//! typist's kind is `memberKind` ([`hub_wire`]).
//! - `id`: the Thread's current address as the daemon names it (the
//!   workspace handle for a pinned Chat, `ws/<chat>` for a sidecar), or the
//!   conversation id when it has none.
//! - `member`: the `author.login` that member's Thread posts carry: `owner`
//!   for the owner token, the Connect username, the App guest's pass
//!   username. A client clears the name when that member's message lands.
//! - `name`: what the Thread shows as `from` on that member's posts (the
//!   owner's stamp name, the login's username, the guest's pass username).
//! - Lossy: never stored, never replayed on a reconnect or catch-up, and a
//!   socket that falls behind on the bus skips it.
//! - Never to any socket of the typist's own person (all windows and
//!   devices, overlay or hub): each socket skips frames whose internal
//!   person key ([`Typist::person`]: `owner` for the owner token and every
//!   login the owner linked) is its own.
//! - Only to sockets on that Thread (every overlay socket is gated on read
//!   access to it; a hub socket only while it views it). An App guest's
//!   socket gets it only when it opted in to the ear (`?ear=1`,
//!   `overlay_ws::EAR_PARAM`) and its pass still reads the Thread when the
//!   frame goes out; an App that doesn't render it never receives it.
//! - No text, ever: the frame carries who and when, nothing they typed.
//!
//! Memory only, no SQL migration. Boot-status key [`FEATURE`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::sync::broadcast;

/// Boot-status feature key.
pub const FEATURE: &str = "thread-typing-v1";

/// The frame's `collection`.
pub const COLLECTION: &str = "typing";

/// The client frame's `t`.
pub const CLIENT_T: &str = "typing";

/// One connection sends at most one typing frame per this, for its Thread
/// (Sew's `TYPING_INTERVAL_MS`); clients send at most once per this too.
pub const INTERVAL: Duration = Duration::from_millis(8_000);

/// Longest `thread` value a client frame may carry (a longer one is not an
/// address and is ignored).
pub const MAX_THREAD_CHARS: usize = 512;

/// Typing frames are a few a second box-wide; a socket this far behind skips.
const BUS_CAP: usize = 256;

/// Who is typing: a person with a login on this box, or an App guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypistKind {
    Person,
    AppGuest,
}

impl TypistKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TypistKind::Person => "person",
            TypistKind::AppGuest => "app_guest",
        }
    }
}

/// The member behind a typing frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typist {
    /// The wire `member`: the `author.login` this member's Thread posts
    /// carry (`owner`, the Connect username, the App guest's pass username),
    /// so a client clears the name when that member's message lands.
    pub member: String,
    /// The display name the Thread shows on this member's posts.
    pub name: String,
    pub kind: TypistKind,
    /// Internal, never on the wire: the person key every socket of this
    /// person carries (`owner` for the owner token AND every login the owner
    /// linked; else the lowercased username; `guest:<principalId>`). A
    /// socket skips frames whose `person` is its own.
    pub person: String,
}

/// One typing frame on the bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypingFrame {
    /// Internal filter key: the socket's conversation.
    pub conversation_id: String,
    /// The wire `id`: the Thread's current address.
    pub address: String,
    pub typist: Typist,
    /// Unix ms when the daemon took the frame.
    pub at: i64,
}

/// `{"t":"typing","thread":"<addr>"}` → the trimmed `thread`; `None` for
/// anything else (another frame, a missing, empty, non-string or oversized
/// `thread`). Extra keys are ignored (never relayed).
pub fn parse_typing(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("t").and_then(serde_json::Value::as_str) != Some(CLIENT_T) {
        return None;
    }
    let thread = v.get("thread")?.as_str()?.trim();
    if thread.is_empty() || thread.chars().count() > MAX_THREAD_CHARS || thread.chars().any(char::is_control) {
        return None;
    }
    Some(thread.to_string())
}

/// The typist behind a login socket: the owner token (`owner`) or a Connect
/// login's username. `None` for a reserved name (`agent`, …) or a login
/// with no person key: nobody types under those.
pub fn typist_of_login(login: &str) -> Option<Typist> {
    let login = login.trim();
    if login.is_empty() {
        return None;
    }
    if login.eq_ignore_ascii_case("owner") {
        // `PostAuthor::owner()`: login `owner`, the stamp name as `from`.
        return Some(Typist {
            member: "owner".to_string(),
            name: crate::workspace_msg::resolve_owner_from(),
            kind: TypistKind::Person,
            person: k2_core::notify_prefs::OWNER.to_string(),
        });
    }
    if k2_core::connect_users::is_reserved_username(login) {
        return None;
    }
    let people = match k2_core::notify_prefs::People::load() {
        Ok(p) => p,
        Err(e) => {
            k2_core::log_debug!("[daemon/thread_typing] people load failed: {e}");
            return None;
        }
    };
    let person = people.login_person_key(login)?;
    // `PostAuthor::user(username)` and the compose rule's `from`: the
    // username as the session carries it.
    Some(Typist { member: login.to_string(), name: login.to_string(), kind: TypistKind::Person, person })
}

/// The typist behind an App pass: an App guest (a person, keyed
/// `guest:<principalId>`). `None` for an App's own pass or a platform
/// token: a service account never types.
pub fn typist_of_pass(pass: &k2_core::skin::SkinPass) -> Option<Typist> {
    let reader = crate::read_routes::pass_reader(pass);
    if !reader.person {
        return None;
    }
    // A guest's post: `from` and `PostAuthor::guest` login are both the
    // pass username (`thread_sender`).
    let login = crate::overlay_routes::guest_post_from(pass);
    Some(Typist { member: login.clone(), name: login, kind: TypistKind::AppGuest, person: reader.key })
}

/// The person key a socket skips as its own ([`Typist::person`]) for a
/// login socket.
pub fn own_person_of_login(login: &str) -> Option<String> {
    typist_of_login(login).map(|t| t.person)
}

/// [`own_person_of_login`] for an App pass (a guest's `guest:<principalId>`).
pub fn own_person_of_pass(pass: &k2_core::skin::SkinPass) -> String {
    crate::read_routes::pass_reader(pass).key
}

/// An App pass may show as typing in `room`'s Thread only with
/// `thread:post` there (Sew's SEND_MESSAGES rule: only someone who can post
/// is typing). Reading alone lets it receive, never send.
pub fn pass_may_post(pass: &k2_core::skin::SkinPass, room: &str) -> bool {
    pass.has_cap_in_room(room, crate::skin_routes::THREAD_POST)
}

/// Whether anyone may post in `conversation` at all: never in imported
/// history (`import:`), a Grok Bot's collected Thread (`gb:`) or a Voice &
/// Chat only login's own conversation (`vc:`); typing there is dropped.
pub fn takes_posts(conversation: &str) -> bool {
    let c = conversation.trim();
    !(k2_core::overlay::transfer::is_import_key(c)
        || crate::grokbots_routes::is_gb_addr(c)
        || c.starts_with(crate::overlay_routes::VC_PREFIX))
}

/// When `thread` names `conversation` (its id, its current address, or a
/// handle it answered at before, compared without case), the address the
/// frame goes out under: the current address, else the conversation id.
/// `None` = another Thread, or unknown: the frame is dropped.
pub fn address_if_names(conn: &rusqlite::Connection, conversation: &str, thread: &str) -> Option<String> {
    use k2_core::workspace_session_handles as h;
    let thread = thread.trim();
    let conversation = conversation.trim();
    if thread.is_empty() || conversation.is_empty() {
        return None;
    }
    let project_id = h::project_id_for_session_id(conn, conversation).ok().flatten();
    let current = project_id.as_deref().and_then(|p| h::current_address_for(conn, p, conversation).ok().flatten());
    let same = |a: &str| a.trim().eq_ignore_ascii_case(thread);
    let names = same(conversation)
        || current.as_deref().is_some_and(same)
        || project_id.as_deref().is_some_and(|p| {
            // A sidecar renamed since the client read its address: `ws/<old>`.
            let ws = h::workspace_address_name(conn, p).ok();
            let canonical = h::conversation_is_canonical(conn, p, conversation);
            !canonical
                && ws.is_some_and(|ws| {
                    h::past_handles_for(conn, p, conversation)
                        .unwrap_or_default()
                        .iter()
                        .any(|old| same(&format!("{ws}/{old}")))
                })
        });
    names.then(|| current.unwrap_or_else(|| conversation.to_string()))
}

/// Typing frames dropped by a connection's throttle since the daemon
/// started (counted, never queued).
static THROTTLED: AtomicU64 = AtomicU64::new(0);

/// How many typing frames connections' throttles dropped since start.
pub fn throttled_total() -> u64 {
    THROTTLED.load(Ordering::Relaxed)
}

/// One connection's throttle (Sew's `typing_last`): at most one typing
/// frame per Thread per [`INTERVAL`]. A frame inside the window is dropped
/// and counted; only a frame that went out starts a new window. An overlay
/// socket has one Thread; the hub may type in several, each its own key.
#[derive(Debug, Default)]
pub struct Throttle {
    last: HashMap<String, Instant>,
}

impl Throttle {
    /// May a frame for `conversation` from this connection go out at `now`?
    /// `false` = drop it (counted in [`throttled_total`]).
    pub fn due(&self, conversation: &str, now: Instant) -> bool {
        if self.last.get(conversation).is_some_and(|t| now.saturating_duration_since(*t) < INTERVAL) {
            THROTTLED.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        true
    }

    /// A frame for `conversation` went out at `now`: its window starts.
    pub fn sent(&mut self, conversation: &str, now: Instant) {
        // A long-lived hub socket keeps only windows that still matter.
        if self.last.len() >= 64 {
            self.last.retain(|_, t| now.saturating_duration_since(*t) < INTERVAL);
        }
        self.last.insert(conversation.to_string(), now);
    }
}

fn bus() -> &'static broadcast::Sender<TypingFrame> {
    static TX: OnceLock<broadcast::Sender<TypingFrame>> = OnceLock::new();
    TX.get_or_init(|| broadcast::channel(BUS_CAP).0)
}

pub fn subscribe() -> broadcast::Receiver<TypingFrame> {
    bus().subscribe()
}

/// Publish one typing frame. Lossy: a socket that lags skips it; nothing
/// is stored or replayed.
pub fn publish(frame: TypingFrame) {
    let _ = bus().send(frame);
}

/// A client's typing frame for `conversation`, from `typist` (the socket
/// already checked its throttle and, for an App pass, `thread:post`):
/// dropped for a Thread nobody posts in or one `thread` doesn't name, else
/// published. Reads the database (run it off the async workers). True when
/// a frame went out (the connection's window starts).
pub fn on_frame(conversation: &str, thread: &str, typist: Typist) -> bool {
    if !takes_posts(conversation) {
        return false;
    }
    let address = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        address_if_names(&conn, conversation, thread)
    };
    let Some(address) = address else {
        return false;
    };
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default();
    publish(TypingFrame { conversation_id: conversation.to_string(), address, typist, at });
    true
}

/// The overlay socket's frame: `kind` is the typist's kind as contracted,
/// and `memberKind` repeats it under the name the hub frame uses.
pub fn wire(frame: &TypingFrame) -> String {
    serde_json::json!({
        "collection": COLLECTION,
        "seq": 0,
        "id": frame.address,
        "member": frame.typist.member,
        "name": frame.typist.name,
        "kind": frame.typist.kind.as_str(),
        "memberKind": frame.typist.kind.as_str(),
        "at": frame.at,
    })
    .to_string()
}

/// The hub's frame kind ([`HUB_KIND`]): on `/cli/sessions/events` every
/// frame's `kind` is its tag (as `thread_activity`), so the typist's kind
/// rides as `memberKind` only.
pub const HUB_KIND: &str = "thread_typing";

/// The hub socket's frame (hub-viewing-v1 viewers): the overlay frame's
/// keys plus `kind: "thread_typing"` and `conversation_id`, with the
/// typist's kind as `memberKind`.
pub fn hub_wire(frame: &TypingFrame) -> String {
    serde_json::json!({
        "kind": HUB_KIND,
        "conversation_id": frame.conversation_id,
        "collection": COLLECTION,
        "seq": 0,
        "id": frame.address,
        "member": frame.typist.member,
        "name": frame.typist.name,
        "memberKind": frame.typist.kind.as_str(),
        "at": frame.at,
    })
    .to_string()
}

/// Longest hub `conversationId` (as `hub_viewing::MAX_ID_CHARS`).
pub const MAX_CONVERSATION_CHARS: usize = 128;

/// The hub's client frame: `{"t":"typing","thread":"<addr>",
/// "conversationId":"<id>"?}` → `(thread, conversationId)`. `None` for
/// anything else, or a bad `conversationId` (ignored, as every unknown
/// frame on the hub).
pub fn parse_hub_typing(text: &str) -> Option<(String, Option<String>)> {
    let thread = parse_typing(text)?;
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let conversation = match v.get("conversationId") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => {
            let s = s.trim();
            let n = s.chars().count();
            if n == 0 || n > MAX_CONVERSATION_CHARS {
                return None;
            }
            Some(s.to_string())
        }
        Some(_) => return None,
    };
    Some((thread, conversation))
}

/// Whether a socket on (or viewing) `conversation`, whose own person key is
/// `own`, sends `frame`. Guest sockets also need the ear opt-in and a live
/// pass (the socket checks those).
pub fn for_socket(frame: &TypingFrame, conversation: &str, own: Option<&str>) -> bool {
    frame.conversation_id == conversation && own != Some(frame.typist.person.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(person: &str) -> TypingFrame {
        TypingFrame {
            conversation_id: "conv-1".into(),
            address: "sales".into(),
            typist: Typist { member: "dana".into(), name: "dana".into(), kind: TypistKind::AppGuest, person: person.into() },
            at: 1_700_000_000_000,
        }
    }

    #[test]
    fn parse_takes_only_a_typing_frame_with_a_thread() {
        assert_eq!(parse_typing(r#"{"t":"typing","thread":" sales "}"#), Some("sales".into()));
        assert_eq!(parse_typing(r#"{"t":"typing","thread":"sales","text":"secret"}"#), Some("sales".into()), "extra keys are ignored, never relayed");
        assert_eq!(parse_typing(r#"{"t":"typing"}"#), None);
        assert_eq!(parse_typing(r#"{"t":"typing","thread":""}"#), None);
        assert_eq!(parse_typing(r#"{"t":"typing","thread":7}"#), None);
        assert_eq!(parse_typing(r#"{"t":"typing","thread":"a\nb"}"#), None);
        assert_eq!(parse_typing(&serde_json::json!({ "t": "typing", "thread": "x".repeat(MAX_THREAD_CHARS + 1) }).to_string()), None);
        assert_eq!(parse_typing(r#"{"t":"viewing","visible":true}"#), None);
        assert_eq!(parse_typing("not json"), None);
    }

    #[test]
    fn the_hub_frame_may_name_its_conversation() {
        assert_eq!(parse_hub_typing(r#"{"t":"typing","thread":"sales"}"#), Some(("sales".into(), None)));
        assert_eq!(
            parse_hub_typing(r#"{"t":"typing","thread":"sales","conversationId":" c-1 "}"#),
            Some(("sales".into(), Some("c-1".into())))
        );
        assert_eq!(parse_hub_typing(r#"{"t":"typing","thread":"sales","conversationId":""}"#), None);
        assert_eq!(parse_hub_typing(r#"{"t":"typing","thread":"sales","conversationId":7}"#), None);
        let long = serde_json::json!({ "t": "typing", "thread": "sales", "conversationId": "c".repeat(MAX_CONVERSATION_CHARS + 1) });
        assert_eq!(parse_hub_typing(&long.to_string()), None);
        assert_eq!(parse_hub_typing(r#"{"t":"viewing","conversationId":"c-1","visible":true}"#), None);
    }

    #[test]
    fn a_connection_sends_one_frame_per_thread_per_eight_seconds_and_counts_the_drops() {
        assert_eq!(INTERVAL, Duration::from_millis(8_000), "Sew's TYPING_INTERVAL_MS");
        let mut t = Throttle::default();
        let t0 = Instant::now();
        assert!(t.due("c1", t0), "the first frame");
        t.sent("c1", t0);
        let before = throttled_total();
        assert!(!t.due("c1", t0 + Duration::from_millis(500)));
        assert!(!t.due("c1", t0 + Duration::from_millis(7_999)));
        assert!(throttled_total() >= before + 2, "drops are counted");
        assert!(t.due("c2", t0 + Duration::from_millis(500)), "another Thread is its own key");
        assert!(t.due("c1", t0 + INTERVAL), "after the window");
        // A dropped or refused frame never starts a window: only `sent` does.
        let fresh = Throttle::default();
        assert!(fresh.due("c1", t0) && fresh.due("c1", t0), "nothing sent, nothing throttled");
    }

    #[test]
    fn nobody_types_where_nobody_posts() {
        assert!(takes_posts("0b6c1c9e-2d3a-4a51-9f4e-1a2b3c4d5e6f"));
        assert!(!takes_posts("import:abc"), "imported history is read-only");
        assert!(!takes_posts("vc:proj:dana"), "a Voice & Chat only conversation");
    }

    #[test]
    fn wire_has_the_contract_shape_and_no_text() {
        let f = frame("guest:p1");
        let v: serde_json::Value = serde_json::from_str(&wire(&f)).expect("json");
        assert_eq!(
            v,
            serde_json::json!({
                "collection": "typing",
                "seq": 0,
                "id": "sales",
                "member": "dana",
                "name": "dana",
                "kind": "app_guest",
                "memberKind": "app_guest",
                "at": 1_700_000_000_000i64,
            }),
            "the person key never goes on the wire"
        );
        let v: serde_json::Value = serde_json::from_str(&hub_wire(&f)).expect("json");
        assert_eq!(
            v,
            serde_json::json!({
                "kind": "thread_typing",
                "conversation_id": "conv-1",
                "collection": "typing",
                "seq": 0,
                "id": "sales",
                "member": "dana",
                "name": "dana",
                "memberKind": "app_guest",
                "at": 1_700_000_000_000i64,
            })
        );
        assert_eq!(TypistKind::Person.as_str(), "person");
    }

    #[test]
    fn a_socket_skips_its_own_person_and_other_threads() {
        let f = frame("owner");
        assert!(for_socket(&f, "conv-1", Some("bea")));
        assert!(for_socket(&f, "conv-1", None));
        assert!(!for_socket(&f, "conv-1", Some("owner")), "never the typist's own sockets");
        assert!(!for_socket(&f, "conv-2", Some("bea")), "only that Thread's sockets");
    }

    #[test]
    fn reserved_logins_never_type() {
        assert_eq!(typist_of_login("agent"), None);
        assert_eq!(typist_of_login(""), None);
    }
}
