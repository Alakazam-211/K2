//! DM and channel pushes for an App's users in the App's own Sew Space
//! (prd-home-chat-pushes-v1, vs-live HC-25 = slice P4, with Sew's 11-18
//! answer to S-8). Rosson 11-18: "Apps that use DMs or Channels can also get
//! push notifications just like they can get push notifications for
//! threads."
//!
//! **The path.** These pushes go the way every App push goes: to the App's
//! own phone app, on the guest devices registered with `push.register`, sent
//! by the daemon to Apple and Google with **the App owner's own keys**
//! (`app_push::worker`). A tap opens the App's own phone app, never K2's
//! Home. `k2Kind: chat`.
//!
//! **Switches.** Per App, `chatPushes` (`k2 app push set <app> --chat on`),
//! off by default, set by the owner, an Admin or the App's own agent with
//! "…and manage them end to end" (switch 2, like `agentReplies`). The App's
//! push must be on too ([`super::state`]). On the box, `K2_APP_CHAT_PUSH=1`
//! turns the path on: off by default until proven, and `K2_APP_PUSH=off`
//! stops it with every other App push. It doesn't need the owner's
//! `homeChatPush`: these are the App's pushes.
//!
//! **Who is read.** One Sew person feed per App user (Sew keeps one waiter
//! each), shared by all their Apps: the principals with an `app`-channel
//! device on an App with chat pushes on, whom K2 added to that App's Space
//! ([`store::chat_push_people`]). Async tasks on the daemon's runtime, not a
//! thread each: an App can have hundreds of users.
//!
//! **Routing, never across Apps** ([`Coalescer::route`]):
//! - A channel item goes to the item's `app_id` (Sew reads it when the feed
//!   is read, S-8), and only when K2's own bind history says the Space was
//!   that App's at the item's own `at` ([`k2_core::tool_hook::app_space::bound_app_at`]):
//!   an unbind or a rebind between the message and the read drops it. An
//!   unbound Space (`app_id` null) is never an App's.
//! - A DM item carries no App. K2 decides with [`dm_route`]: a DM between
//!   two users of the same App goes to that App only if both are only that
//!   App's users; otherwise it is Home's, and an App user has no Home, so
//!   nothing is pushed.
//! - Only to an App the person is a user of (K2's grants, never Sew's
//!   word), and only to that App's devices for that person.
//!
//! **Same rules as Home** (D1, D2): DMs always push; a channel pushes at the
//! person's own level, All messages or @mentions only (default @mentions,
//! set by the App's phone app on its own Sew session, `POST
//! /api/v1/channels/{id}/notify/set`); pushes for All wait for Sew 0.11.0.
//! Dropped: the person's own posts, agents' posts, unknown senders, items
//! older than 10 minutes beyond [`OLD_SUMMARIES_PER_PAGE`] quiet summaries a
//! page. Held per (App, person, conversation): the first pushes at once,
//! the rest in [`WINDOW`] go as one trailing `N new messages`, a mention if
//! any was. At flush: the App and its chat pushes still on, the person still
//! a chat member (and in the App's Space for a channel), no `push.viewing`
//! lease on `chat:<conversation id>`. Budgets: [`PER_APP_HOUR`] device
//! sends per App and [`PER_GUEST_HOUR`] pushes per person per App, their
//! own buckets (never the App's `push.send` budget, never K2's relay).
//!
//! **Privacy.** This path is not sealed: Apple and Google read what it
//! carries. So the sender's name and the message text ride a push only when
//! every preview switch is on: the owner's `pushMessagePreviews`, the App's
//! `previews`, the device's own `previews`, and Sew's feed preview (K2 asks
//! Sew for it only while the owner's `homeChatPush` and
//! `homeChatPushPreviews` are on, and strips it itself otherwise). Off, the
//! title is the App's name and the body fixed words (`New message`,
//! `Mentioned you`, `Replied to you`, `N new messages`). No channel name
//! ever rides (K2 doesn't know an App's channel names), and never a
//! username, principal id or K2 host in `data`. Text is held only while a
//! window is open, in memory; `Debug` hides it, and the send log keeps
//! counts only.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use k2_core::push::HomeChatReason;
use k2_core::skin::app_push as store;
use serde_json::Value;

use super::budget::Limiter;
use super::payload::{self, Notification, Stamps};
use super::sendlog::{self, SendRecord, Skipped};
use super::sender::Target;
use super::worker::{self, Job};
use crate::home_chat::call_push::{self, FeedItem, Poll, ITEM_MAX_AGE_SECS, MESSAGE_KINDS};

/// The box switch: `1`/`on`/`true`/`yes` turns App chat pushes on (default
/// off until proven).
pub const SWITCH_ENV: &str = "K2_APP_CHAT_PUSH";
/// The hold window after a push (the Thread and Home rule).
pub const WINDOW: Duration = Duration::from_secs(45);
/// Chat device sends per App an hour (agent replies' number, its own bucket).
pub const PER_APP_HOUR: usize = super::agent_reply::PER_APP_HOUR;
/// Chat pushes per person per App an hour.
pub const PER_GUEST_HOUR: usize = super::agent_reply::PER_GUEST_HOUR;
/// Old items' quiet summaries per feed page (HC-4).
pub const OLD_SUMMARIES_PER_PAGE: usize = 5;
/// Sew's preview is at most this long.
pub const TEXT_MAX_CHARS: usize = 120;
/// `push.viewing`'s conversation for an App chat: `chat:<Sew channel id>`.
pub const LEASE_PREFIX: &str = "chat:";
/// The Android notification channel id the App's phone app creates.
pub const ANDROID_CHANNEL: &str = "chat";
/// An App user's Sew external id.
pub const PRINCIPAL_PREFIX: &str = "k2:principal:";

/// Does the switch's value mean on?
pub fn switch_on(v: Option<&str>) -> bool {
    matches!(v.map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("1" | "on" | "true" | "yes"))
}

/// Is the App chat push path on for this box (`K2_APP_CHAT_PUSH` on, and
/// `K2_APP_PUSH` not off)?
pub fn box_enabled() -> bool {
    super::box_enabled() && switch_on(std::env::var(SWITCH_ENV).ok().as_deref())
}

// ── the DM decision ──────────────────────────────────────────────────

/// Where a DM's pushes go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// This App's own phone app.
    App(String),
    /// Home's (K2 Companion). An App user has no Home: nothing is pushed
    /// on the App path.
    Home,
}

/// THE rule for a DM (Sew's S-8 answer: DM items carry no App, K2
/// decides). `recipient_apps` and `sender_apps` are the Apps each side is a
/// user of now (K2's grants; a box person or an agent has none): the DM is
/// that App's only if both are users of exactly one App, the same one, and
/// it isn't the box login. Anything else is Home's.
pub fn dm_route(recipient_apps: &[String], sender_apps: &[String]) -> Route {
    match (recipient_apps, sender_apps) {
        ([a], [b]) if a == b && a != k2_core::skin::HOST_APP_ID => Route::App(a.clone()),
        _ => Route::Home,
    }
}

// ── the rules ────────────────────────────────────────────────────────

/// A sender as K2 knows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sender {
    /// `k2:principal:<id>`, `k2:owner`, `k2:connect:<uuid>`, `k2:workspace:<uuid>`.
    pub ext: String,
    /// An agent (a `k2:workspace:` member, an agent kind in Sew's member
    /// map, or an App login marked as an agent's).
    pub agent: bool,
}

/// What the rules need from the daemon (a fake in tests).
pub trait Env: Send + Sync {
    /// The box switches ([`box_enabled`]) and not air-gapped.
    fn box_on(&self) -> bool;
    /// The App's push is on and its `chatPushes` is on.
    fn app_on(&self, app: &str) -> bool;
    /// The App K2's bind history says `space` was bound to at `at`.
    fn bound_at(&self, space: &str, at: i64) -> Option<String>;
    /// The Apps `pid` is a user of now (K2's grants).
    fn user_apps(&self, pid: &str) -> Vec<String>;
    /// The item's sender (`None`: not known here, never pushed).
    fn sender(&self, item: &FeedItem) -> Option<Sender>;
    /// The sender's name as the App's chat shows it (`""` unknown). Read
    /// only for a push that may show it.
    fn sender_name(&self, sender_ext: &str) -> String;
    /// `pid` is still a chat member, and for a channel still in `app`'s
    /// Space (HC-7).
    fn still_member(&self, app: &str, pid: &str, space: Option<&str>) -> bool;
    /// `pid` holds a live `push.viewing` lease on `chat:<conv>`.
    fn viewing(&self, pid: &str, conv: &str, now: Instant) -> bool;
    /// The owner's `pushMessagePreviews`, the App's `previews` and K2's own
    /// record that Sew's feed preview may flow are all on. (The device's
    /// own switch is the last one, per device at delivery.)
    fn text_allowed(&self, app: &str) -> bool;
    /// Send one push to `push.app`'s devices for `push.principal`. True when
    /// it was queued.
    fn deliver(&self, push: &ChatPush) -> bool;
}

/// One coalesced App chat push. `preview` and `sender_name` exist only when
/// [`Env::text_allowed`] said so; `Debug` hides both.
#[derive(Clone, PartialEq, Eq)]
pub struct ChatPush {
    pub app: String,
    pub principal: String,
    pub reason: HomeChatReason,
    /// The Sew channel id (decimal).
    pub conversation: String,
    pub dm: bool,
    pub message_id: String,
    pub count: u32,
    pub mention: bool,
    pub sender_name: Option<String>,
    pub preview: Option<String>,
    /// An old items' summary: no sound, normal priority.
    pub passive: bool,
}

impl std::fmt::Debug for ChatPush {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatPush")
            .field("app", &self.app)
            .field("principal", &self.principal)
            .field("reason", &self.reason)
            .field("conversation", &self.conversation)
            .field("dm", &self.dm)
            .field("message_id", &self.message_id)
            .field("count", &self.count)
            .field("mention", &self.mention)
            .field("sender_name", &self.sender_name.as_ref().map(|_| "<hidden>"))
            .field("preview", &self.preview.as_ref().map(|_| "<hidden>"))
            .field("passive", &self.passive)
            .finish()
    }
}

/// One held message. `preview` and `sender_ext` stay private in `Debug`.
#[derive(Clone, PartialEq, Eq)]
struct Held {
    message_id: String,
    sender_ext: String,
    reason: HomeChatReason,
    dm: bool,
    space: Option<String>,
    preview: Option<String>,
}

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Held")
            .field("message_id", &self.message_id)
            .field("reason", &self.reason)
            .field("dm", &self.dm)
            .field("preview", &self.preview.as_ref().map(|_| "<hidden>"))
            .finish()
    }
}

fn reason_of(kind: &str) -> HomeChatReason {
    match kind {
        "dm" => HomeChatReason::Dm,
        "mention" => HomeChatReason::Mention,
        "reply_to_me" => HomeChatReason::Reply,
        "keyword" => HomeChatReason::Keyword,
        _ => HomeChatReason::All,
    }
}

/// Sew ids are time-sortable decimals: `a` is after `b`.
fn id_after(a: &str, b: &str) -> bool {
    crate::home_chat::chat_push::id_after(a, b)
}

/// (App, principal, conversation).
type Key = (String, String, String);

#[derive(Debug, Default)]
struct Win {
    open_until: Option<Instant>,
    pending: Vec<Held>,
}

/// Every App user's windows. Pure: the [`Env`] does the rest.
#[derive(Debug, Default)]
pub struct Coalescer {
    windows: HashMap<Key, Win>,
}

impl Coalescer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Which App (if any) item `it` is for, for App user `pid`, and what to
    /// hold. `None` drops it.
    fn route(env: &dyn Env, pid: &str, it: &FeedItem) -> Option<(String, Held)> {
        let me = format!("{PRINCIPAL_PREFIX}{pid}");
        let (Some(_conv), Some(msg)) = (it.channel_id.as_ref(), it.message_id.clone()) else { return None };
        let sender = env.sender(it)?;
        if sender.ext == me || sender.agent {
            return None; // own posts; agents' posts
        }
        let mine = env.user_apps(pid);
        let app = match it.space_id.as_deref() {
            Some(space) => {
                // Sew's app_id is the binding at read time; K2's history
                // must agree it was that App's at the item's own time.
                let app = it.app_id.clone()?;
                let at = it.at?;
                if env.bound_at(space, at).as_deref() != Some(app.as_str()) {
                    return None;
                }
                app
            }
            None => {
                if it.app_id.is_some() {
                    return None; // a DM never carries one (S-8): not ours to guess
                }
                let theirs = sender.ext.strip_prefix(PRINCIPAL_PREFIX).map(|p| env.user_apps(p)).unwrap_or_default();
                match dm_route(&mine, &theirs) {
                    Route::App(a) => a,
                    Route::Home => return None,
                }
            }
        };
        if !mine.contains(&app) || !env.app_on(&app) {
            return None;
        }
        let preview = if env.text_allowed(&app) {
            it.preview().map(str::trim).filter(|t| !t.is_empty()).map(|t| t.chars().take(TEXT_MAX_CHARS).collect())
        } else {
            None
        };
        let dm = it.space_id.is_none();
        let held = Held {
            message_id: msg,
            sender_ext: sender.ext,
            reason: if dm && it.kind == "dm" { HomeChatReason::Dm } else { reason_of(&it.kind) },
            dm,
            space: it.space_id.clone(),
            preview,
        };
        Some((app, held))
    }

    /// One feed page's items for App user `pid`. Returns the pushes queued.
    pub fn on_items(&mut self, env: &dyn Env, pid: &str, items: &[FeedItem], now_unix: i64, now: Instant) -> usize {
        if !env.box_on() {
            return 0;
        }
        let mut sent = 0;
        let mut old: BTreeMap<(String, String), Vec<Held>> = BTreeMap::new();
        for it in items.iter().filter(|i| MESSAGE_KINDS.contains(&i.kind.as_str())) {
            let Some((app, held)) = Self::route(env, pid, it) else { continue };
            let conv = it.channel_id.clone().unwrap_or_default();
            if it.older_than(ITEM_MAX_AGE_SECS, now_unix) {
                old.entry((app, conv)).or_default().push(held);
            } else {
                sent += usize::from(self.add(env, (app, pid.to_string(), conv), held, now));
            }
        }
        // Old items: one quiet summary per conversation, newest first, at
        // most OLD_SUMMARIES_PER_PAGE; the rest are dropped.
        let mut convs: Vec<((String, String), Vec<Held>)> = old.into_iter().collect();
        let newest = |v: &Vec<Held>| v.iter().map(|h| h.message_id.clone()).fold(String::new(), |m, x| if m.is_empty() || id_after(&x, &m) { x } else { m });
        convs.sort_by(|a, b| if id_after(&newest(&a.1), &newest(&b.1)) { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater });
        for ((app, conv), held) in convs.into_iter().take(OLD_SUMMARIES_PER_PAGE) {
            sent += usize::from(self.push(env, &(app, pid.to_string(), conv), held, true, now));
        }
        sent
    }

    fn add(&mut self, env: &dyn Env, key: Key, held: Held, now: Instant) -> bool {
        let w = self.windows.entry(key.clone()).or_default();
        if w.pending.iter().any(|h| h.message_id == held.message_id) {
            return false; // a repeated page
        }
        match w.open_until {
            Some(until) if now < until => {
                w.pending.push(held);
                false
            }
            _ => self.push(env, &key, vec![held], false, now),
        }
    }

    /// The flush rules, then one push for `items`. True when it was queued.
    fn push(&mut self, env: &dyn Env, key: &Key, mut items: Vec<Held>, passive: bool, now: Instant) -> bool {
        let (app, pid, conv) = key;
        if !env.box_on() || !env.app_on(app) {
            return false;
        }
        items.sort_by(|a, b| if id_after(&a.message_id, &b.message_id) { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less });
        let Some(newest) = items.last().cloned() else { return false };
        if !env.still_member(app, pid, newest.space.as_deref()) {
            return false; // HC-7: removed during the window, text and all
        }
        if env.viewing(pid, conv, now) {
            return false;
        }
        let mention = items.iter().any(|h| h.reason == HomeChatReason::Mention);
        let sender_name = newest.preview.as_ref().map(|_| env.sender_name(&newest.sender_ext)).filter(|n| !n.trim().is_empty());
        let push = ChatPush {
            app: app.clone(),
            principal: pid.clone(),
            reason: if mention { HomeChatReason::Mention } else { newest.reason },
            conversation: conv.clone(),
            dm: newest.dm,
            message_id: newest.message_id.clone(),
            count: u32::try_from(items.len()).unwrap_or(u32::MAX),
            mention,
            sender_name,
            preview: newest.preview.clone(),
            passive,
        };
        let queued = env.deliver(&push);
        if !passive {
            self.windows.entry(key.clone()).or_default().open_until = Some(now + WINDOW);
        }
        queued
    }

    /// Flush every window that closed with items held.
    pub fn tick(&mut self, env: &dyn Env, now: Instant) {
        let due: Vec<Key> = self
            .windows
            .iter()
            .filter(|(_, w)| !w.pending.is_empty() && w.open_until.is_none_or(|t| now >= t))
            .map(|(k, _)| k.clone())
            .collect();
        for key in due {
            let held = self.windows.get_mut(&key).map(|w| std::mem::take(&mut w.pending)).unwrap_or_default();
            self.push(env, &key, held, false, now);
        }
        self.windows.retain(|_, w| !w.pending.is_empty() || w.open_until.is_some_and(|t| now < t + WINDOW));
    }

    /// `pid`'s poller stopped: every window of theirs goes, text included.
    pub fn forget_person(&mut self, pid: &str) {
        self.windows.retain(|(_, p, _), _| p != pid);
    }

    /// Tests: what (App, person, conversation) holds.
    #[cfg(test)]
    pub fn held(&self, app: &str, pid: &str, conv: &str) -> usize {
        self.windows.get(&(app.to_string(), pid.to_string(), conv.to_string())).map_or(0, |w| w.pending.len())
    }
}

// ── the notification ─────────────────────────────────────────────────

/// The words a push shows when no text may.
fn fixed_body(reason: HomeChatReason) -> &'static str {
    match reason {
        HomeChatReason::Mention => "Mentioned you",
        HomeChatReason::Reply => "Replied to you",
        HomeChatReason::Dm | HomeChatReason::All | HomeChatReason::Keyword => "New message",
    }
}

/// The notification for one device. `with_text`: every preview switch is on
/// for this device (and the push holds text). Without it, the title is the
/// App's name and the body fixed words: no sender, no channel, no text.
pub fn notification(p: &ChatPush, app_name: &str, with_text: bool) -> Notification {
    let app_name: String = app_name.chars().filter(|c| !c.is_control()).take(100).collect();
    let text = p.preview.as_deref().filter(|_| with_text);
    let (title, body) = match text {
        Some(t) => {
            let title = p.sender_name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| app_name.clone());
            let body = if p.count > 1 { format!("{} new messages · {t}", p.count) } else { t.to_string() };
            (title, body)
        }
        None => {
            let body = if p.count > 1 { format!("{} new messages", p.count) } else { fixed_body(p.reason).to_string() };
            (app_name.clone(), body)
        }
    };
    let title = if title.trim().is_empty() { "New message".to_string() } else { title };
    let mut data = BTreeMap::new();
    data.insert("k2Reason".to_string(), p.reason.as_wire().to_string());
    data.insert("k2Conv".to_string(), p.conversation.clone());
    data.insert("k2ConvKind".to_string(), if p.dm { "dm" } else { "channel" }.to_string());
    data.insert("k2Msg".to_string(), p.message_id.clone());
    data.insert("k2Count".to_string(), p.count.to_string());
    data.insert("k2Mention".to_string(), if p.mention { "1" } else { "0" }.to_string());
    let tag = format!("{LEASE_PREFIX}{}", p.conversation.chars().take(payload::MAX_COLLAPSE_BYTES - LEASE_PREFIX.len()).collect::<String>());
    Notification {
        title: Some(title.chars().take(100).collect()),
        body: Some(body.chars().take(300).collect()),
        data,
        topic: None,
        collapse_id: Some(tag.clone()),
        thread_id: Some(tag),
        sound: (!p.passive).then(|| "default".to_string()),
        badge: None,
        silent: false,
        high_priority: !p.passive,
        ttl_secs: payload::DEFAULT_TTL_SECS,
        mutable: false,
        android_channel: Some(ANDROID_CHANNEL.to_string()),
    }
}

/// The last preview switch, per device: a push that holds text shows it on
/// a device whose own `previews` isn't off (`None` is the device's default,
/// on). The other three switches already decided whether the push holds
/// any text ([`Env::text_allowed`]).
pub fn shows_text(p: &ChatPush, device_previews: Option<bool>) -> bool {
    p.preview.is_some() && device_previews != Some(false)
}

// ── the daemon's instance ────────────────────────────────────────────

fn coalescer() -> &'static Mutex<Coalescer> {
    static C: OnceLock<Mutex<Coalescer>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Coalescer::new()))
}

fn lock() -> std::sync::MutexGuard<'static, Coalescer> {
    coalescer().lock().unwrap_or_else(|p| p.into_inner())
}

fn env_seam() -> &'static RwLock<Option<Arc<dyn Env>>> {
    static S: RwLock<Option<Arc<dyn Env>>> = RwLock::new(None);
    &S
}

fn env() -> Arc<dyn Env> {
    if let Some(e) = env_seam().read().unwrap_or_else(|p| p.into_inner()).clone() {
        return e;
    }
    Arc::new(LiveEnv)
}

/// Tests: swap the env (`None` = live) and forget every window.
#[doc(hidden)]
pub fn set_env_for_tests(e: Option<Arc<dyn Env>>) {
    *env_seam().write().unwrap_or_else(|p| p.into_inner()) = e;
    *lock() = Coalescer::new();
}

/// One feed page for App user `pid`. Returns the pushes queued.
pub fn on_feed(pid: &str, items: &[FeedItem], now_unix: i64) -> usize {
    if !items.iter().any(|i| MESSAGE_KINDS.contains(&i.kind.as_str())) {
        return 0;
    }
    let e = env();
    lock().on_items(e.as_ref(), pid, items, now_unix, Instant::now())
}

/// Flush what is due (the ticker, once a second; tests call it).
pub fn tick(now: Instant) {
    let e = env();
    lock().tick(e.as_ref(), now);
}

fn per_app() -> &'static Limiter {
    static L: OnceLock<Limiter> = OnceLock::new();
    L.get_or_init(|| Limiter::new(Duration::from_secs(3600)))
}

fn per_guest() -> &'static Limiter {
    static L: OnceLock<Limiter> = OnceLock::new();
    L.get_or_init(|| Limiter::new(Duration::from_secs(3600)))
}

/// The daemon's stores.
pub struct LiveEnv;

impl Env for LiveEnv {
    fn box_on(&self) -> bool {
        box_enabled() && !crate::push_routes::airgap_blocks()
    }

    fn app_on(&self, app: &str) -> bool {
        super::state(app).is_ok() && store::config(app).is_ok_and(|c| c.chat_pushes)
    }

    fn bound_at(&self, space: &str, at: i64) -> Option<String> {
        k2_core::tool_hook::with_home_chat_db(|c| k2_core::tool_hook::app_space::bound_app_at(c, space, at)).ok().flatten()
    }

    fn user_apps(&self, pid: &str) -> Vec<String> {
        store::user_apps(pid).unwrap_or_default()
    }

    fn sender(&self, item: &FeedItem) -> Option<Sender> {
        let row = item.started_by.as_deref().and_then(|m| k2_core::tool_hook::chat_member_by_id(m).ok().flatten());
        let ext = item.from_ext.clone().or_else(|| row.as_ref().map(|r| r.external_id.clone()))?;
        let agent = ext.starts_with("k2:workspace:")
            || row.as_ref().is_some_and(|r| r.kind == "agent")
            || ext.strip_prefix(PRINCIPAL_PREFIX).is_some_and(|p| store::principal_is_agent(p).unwrap_or(true));
        Some(Sender { ext, agent })
    }

    fn sender_name(&self, sender_ext: &str) -> String {
        match sender_ext.strip_prefix(PRINCIPAL_PREFIX) {
            Some(p) => store::principal_chat_name(p).ok().flatten().unwrap_or_default(),
            None => String::new(),
        }
    }

    fn still_member(&self, app: &str, pid: &str, space: Option<&str>) -> bool {
        store::still_chat_member(app, pid, space).unwrap_or(false)
    }

    fn viewing(&self, pid: &str, conv: &str, now: Instant) -> bool {
        crate::thread_viewers::global().is_viewing(&format!("{}{pid}", crate::read_routes::GUEST_KEY_PREFIX), &format!("{LEASE_PREFIX}{conv}"), now)
    }

    fn text_allowed(&self, app: &str) -> bool {
        crate::push_routes::previews_allowed() && k2_core::home_chat::push_previews_allowed() && store::config(app).is_ok_and(|c| c.previews)
    }

    fn deliver(&self, p: &ChatPush) -> bool {
        deliver(p)
    }
}

/// Queue one person's push on their devices for the App (per device: the
/// text only when its own `previews` isn't off).
fn deliver(p: &ChatPush) -> bool {
    let app = p.app.as_str();
    let devices = match store::targets(app, std::slice::from_ref(&p.principal)) {
        Ok(d) if !d.is_empty() => d,
        _ => return false,
    };
    let Ok(cfg) = store::config(app) else { return false };
    let configured = |d: &store::PushDevice| (d.platform == store::PLATFORM_APNS && cfg.apns.is_some()) || (d.platform == store::PLATFORM_FCM && cfg.fcm.is_some());
    let sendable: Vec<&store::PushDevice> = devices.iter().filter(|d| configured(d)).collect();
    if sendable.is_empty() || per_app().take_n(app, sendable.len(), PER_APP_HOUR).is_err() {
        return false;
    }
    if per_guest().take(&format!("{app}\0{}", p.principal), PER_GUEST_HOUR).is_err() {
        return false;
    }
    let Ok(who) = super::who(app, &p.principal) else { return false };
    let name = super::published(app).map(|r| r.name).unwrap_or_default();
    let with_text = Arc::new(notification(p, &name, true));
    let generic = Arc::new(notification(p, &name, false));
    let send_id = payload::new_send_id();
    let jobs: Vec<Job> = sendable
        .iter()
        .map(|d| Job {
            target: Target::from(*d),
            notification: if shows_text(p, d.previews) { with_text.clone() } else { generic.clone() },
            stamps: Stamps { app: app.to_string(), send_id: send_id.clone(), kind: payload::KIND_CHAT, who: who.clone() },
        })
        .collect();
    let queued = jobs.len();
    sendlog::start(app, SendRecord::new(&send_id, "k2:chat", "chat", 1, None, queued, Skipped::default()));
    if worker::enqueue(app, jobs).is_err() {
        sendlog::refused(app, &send_id, "push_queue_full");
        return false;
    }
    true
}

// ── the feed pollers (async tasks) ───────────────────────────────────

/// A poller Sew stopped (404, 403 `not_provisioned`) starts again after this.
const STOPPED_FOR: Duration = call_push::STOPPED_FOR;
/// The supervisor's look at who is wanted.
const SUPERVISE_EVERY: Duration = Duration::from_secs(30);

/// Bumped by [`restart_all`]: a page read before it never stores its cursor.
static RESET_GEN: AtomicU64 = AtomicU64::new(0);

fn running() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static R: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn stopped() -> &'static Mutex<HashMap<String, Instant>> {
    static S: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn store_cursor(ext: &str, cursor: u64) {
    if let Err(e) = k2_core::tool_hook::with_home_chat_db(|c| k2_core::tool_hook::home_push::set_cursor(c, ext, cursor, crate::home_chat::now_secs())) {
        k2_core::log_debug!("[app-push] chat feed cursor write: {e}");
    }
}

fn error_code(body: &Value) -> &str {
    body.pointer("/error/code").or_else(|| body.get("code")).and_then(Value::as_str).unwrap_or("")
}

/// One feed answer for App user `pid` read from `cursor`: the pushes go
/// out and `cursor` moves to the page's `next` (stored once they went).
/// `410 cursor_expired` resumes at `oldest_seq` (or a lower `head`); `404`
/// and `403 not_provisioned` stop this person. Call kinds are Home's.
pub fn handle_page(pid: &str, cursor: &mut u64, status: u16, body: &Value) -> Result<Poll, String> {
    let ext = format!("{PRINCIPAL_PREFIX}{pid}");
    match status {
        200..=299 => {}
        410 if error_code(body) == "cursor_expired" => {
            let field = |k: &str| body.pointer(&format!("/error/details/{k}")).or_else(|| body.get(k)).and_then(Value::as_u64);
            let oldest = field("oldest_seq").ok_or("410 cursor_expired without oldest_seq")?;
            *cursor = match field("head") {
                Some(head) if head < *cursor => head,
                _ => oldest,
            };
            store_cursor(&ext, *cursor);
            return Ok(Poll::Again(0));
        }
        404 => return Ok(Poll::Stop(format!("404 {}", error_code(body)))),
        403 if error_code(body) == "not_provisioned" || body.pointer("/error/details/reason").and_then(Value::as_str) == Some("not_provisioned") => {
            return Ok(Poll::Stop("403 not_provisioned".into()));
        }
        s => return Err(format!("the feed answered {s} {}", error_code(body))),
    }
    let (items, next) = call_push::parse_feed(body);
    let next = next.ok_or("the feed's page had no next")?;
    let sent = on_feed(pid, &items, crate::home_chat::now_secs());
    if next != *cursor {
        store_cursor(&ext, next);
    }
    *cursor = next;
    Ok(Poll::Again(sent))
}

/// The live async feed read, as the install (mgmt).
async fn read_feed(ext: &str, after: u64, wait: u64) -> Result<(u16, Value), String> {
    let t = k2_core::tool_hook::get_target(k2_core::tool_hook::CHAT_KIND)?.ok_or("chat isn't set up here")?;
    if !t.enabled {
        return Err("chat is off here".into());
    }
    let client = crate::sew_mgmt::MgmtClient::new(&t.socket, &t.token_file);
    let path = format!(
        "/mgmt/v1/people/{}/notifications?after={after}&limit={}&wait={wait}",
        k2_core::tool_hook::sew_wire::pct_path(ext),
        call_push::FEED_LIMIT
    );
    let r = client
        .get_with_timeout(&path, crate::sew_mgmt::long_poll_timeout(Duration::from_secs(wait)))
        .await
        .map_err(|e| format!("the feed: {e}"))?;
    Ok((r.status, serde_json::from_str(&r.body_text()).unwrap_or(Value::Null)))
}

async fn run(pid: String, stop: Arc<AtomicBool>) {
    let ext = format!("{PRINCIPAL_PREFIX}{pid}");
    let start = ext.clone();
    let mut cursor = tokio::task::spawn_blocking(move || call_push::initial_cursor(&start)).await.unwrap_or(0);
    let mut backoff = Duration::from_secs(1);
    while !stop.load(Ordering::SeqCst) {
        let gen = RESET_GEN.load(Ordering::SeqCst);
        let page = read_feed(&ext, cursor, call_push::FEED_WAIT_S).await;
        if RESET_GEN.load(Ordering::SeqCst) != gen {
            break; // Sew's data was reset during the read: the old Sew's page
        }
        let outcome = match page {
            Ok((status, body)) => {
                let (who, at) = (pid.clone(), cursor);
                tokio::task::spawn_blocking(move || {
                    let mut c = at;
                    handle_page(&who, &mut c, status, &body).map(|p| (p, c))
                })
                .await
                .unwrap_or_else(|e| Err(format!("the page task: {e}")))
            }
            Err(e) => Err(e),
        };
        match outcome {
            Ok((Poll::Again(_), c)) => {
                cursor = c;
                backoff = Duration::from_secs(1);
            }
            Ok((Poll::Stop(why), _)) => {
                k2_core::log_debug!("[app-push] {ext}: chat feed stopped ({why})");
                stopped().lock().unwrap_or_else(|p| p.into_inner()).insert(pid.clone(), Instant::now());
                break;
            }
            Err(e) => {
                k2_core::log_debug!("[app-push] {ext}: chat feed: {e}; retrying");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
    lock().forget_person(&pid);
    let mut g = running().lock().unwrap_or_else(|p| p.into_inner());
    if g.get(&pid).is_some_and(|s| Arc::ptr_eq(s, &stop)) {
        g.remove(&pid);
    }
}

fn start(handle: &tokio::runtime::Handle, pid: &str) {
    if stopped().lock().unwrap_or_else(|p| p.into_inner()).get(pid).is_some_and(|at| at.elapsed() < STOPPED_FOR) {
        return;
    }
    let mut g = running().lock().unwrap_or_else(|p| p.into_inner());
    if g.contains_key(pid) {
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    g.insert(pid.to_string(), stop.clone());
    handle.spawn(run(pid.to_string(), stop));
}

/// The App users whose feed is read now (one per person, all their Apps).
fn wanted() -> std::collections::BTreeSet<String> {
    let chat_on = k2_core::tool_hook::get_target(k2_core::tool_hook::CHAT_KIND).ok().flatten().is_some_and(|t| t.enabled);
    if !box_enabled() || !chat_on {
        return Default::default();
    }
    match store::chat_push_people() {
        Ok(rows) => rows.into_iter().filter(|(app, _)| super::state(app).is_ok()).map(|(_, pid)| pid).collect(),
        Err(e) => {
            k2_core::log_debug!("[app-push] chat push people: {e}");
            Default::default()
        }
    }
}

/// Sew's data was reset (`home_chat::sew_data_reset`, which forgets every
/// cursor): every App chat poller stops and starts again.
pub fn restart_all() {
    RESET_GEN.fetch_add(1, Ordering::SeqCst);
    for stop in running().lock().unwrap_or_else(|p| p.into_inner()).values() {
        stop.store(true, Ordering::SeqCst);
    }
}

/// Start the supervisor and the window ticker (daemon boot). Idempotent;
/// idle while the box switch is off or Sew isn't set up here.
pub fn spawn() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let sup = std::thread::Builder::new().name("app-chat-push-sup".into()).spawn(|| loop {
        let want = wanted();
        for (pid, stop) in running().lock().unwrap_or_else(|p| p.into_inner()).iter() {
            if !want.contains(pid) {
                stop.store(true, Ordering::SeqCst);
            }
        }
        if let Some(h) = crate::rt::handle() {
            for pid in &want {
                start(&h, pid);
            }
        }
        std::thread::sleep(SUPERVISE_EVERY);
    });
    if let Err(e) = sup {
        k2_core::log_debug!("[app-push] chat push supervisor didn't start: {e}");
    }
    let ticker = std::thread::Builder::new().name("app-chat-push-tick".into()).spawn(|| loop {
        std::thread::sleep(Duration::from_secs(1));
        tick(Instant::now());
    });
    if let Err(e) = ticker {
        k2_core::log_debug!("[app-push] chat push ticker didn't start: {e}");
    }
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
