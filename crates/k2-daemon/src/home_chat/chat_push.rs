//! Phone pushes for Home chat (prd-home-chat-pushes-v1; vs-live HC-1 to
//! HC-24). Rosson 11-18: DMs always push; per channel each person picks All
//! messages or @mentions only (two choices, no Off; mute stays Sew's), and a
//! channel defaults to @mentions only; agents' posts don't push unless the
//! person opts in; message text on the lock screen when the owner turns
//! pushes on, sealed to the person's own phone.
//!
//! **Where items come from.** The person feed poller ([`super::call_push`],
//! one per person: Sew keeps one waiter each) hands every page here
//! ([`on_feed`]). Sew writes `dm`, `mention` and `reply_to_me` items, gated
//! by the member's level, mute and Do Not Disturb (Sew holds the feed during
//! DND); 0.11.0 adds `all`, `keyword` and the `read` marker. The poller drops
//! nothing here; these rules do, in the daemon (headless: no client needed).
//!
//! **Dropped at once** (nothing held): `homeChatPush` off; an App's item
//! (`app_id`), or one outside the K5 Space and Home's own DMs (HC-5); a
//! sender who isn't a box person (HC-6: Sew system posts, unknown members, a
//! recording card's transcript turns); the person's own post; an agent's
//! post unless the person opted in (D4).
//!
//! **Held and checked at flush** (one window per (person, conversation),
//! the Thread rules): the first item pushes at once; later ones wait for the
//! window ([`WINDOW`] 45 s, [`BACKOFF`] 5 min while the person hasn't opened
//! the conversation since their last push) and go as one trailing
//! `N new messages`, a mention if any held item was one. At flush:
//! the person still has phones, a live login and their bell on (HC-7, HC-18);
//! still in the channel, or the DM isn't gone (HC-7); no client of theirs
//! holds a viewing lease on it (HC-15); they haven't read past it; their own
//! cap ([`PERSON_PER_HOUR`]) and the daemon's Home chat budget (HC-19).
//! Over either, the alert becomes a badge correction.
//!
//! **Old items** (more than 10 minutes old by their own `at`: a restart's
//! backlog, a DND release; HC-4) never alert one by one: each conversation
//! gets one passive summary (no sound), at most [`OLD_SUMMARIES_PER_PAGE`]
//! a page, and the rest only correct the badge.
//!
//! **Text** (D6, HC-16): only Sew's feed `preview`, and only while the
//! owner's `homeChatPush` and `homeChatPushPreviews` and the person's own
//! "Show message text" are on; otherwise K2 strips it itself, seal
//! included. It is held only while a window is open, in memory, and is never
//! logged: [`Pending`] hides it from `Debug`, and the logs, the attempt ring
//! and the device log carry kinds, reasons, ids and outcomes only.
//!
//! **Viewing leases** (HC-15): `{"t":"viewing","conv","visible"}` frames on
//! the person's `WS /cli/home-chat/events`, kept in `thread_viewers` under
//! (`ext`, `hc:<conv>`) for 90 s and cleared when that socket closes.
//!
//! **Read-clear** (HC-11): the follower's `read_state_updated` (and Sew
//! 0.11.0's `read` item) tell the badge path `h:<conv>:<message id>`, sent
//! only when an alert may be showing.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use k2_core::push::{HomeChatMsg, HomeChatPush, HomeChatReason, PushEvent};
use k2_core::tool_hook::home_push::PushPrefs;

use super::call_push::{FeedItem, ITEM_MAX_AGE_SECS, MESSAGE_KINDS, READ_KIND};

/// The hold window after a push.
pub const WINDOW: Duration = Duration::from_secs(45);
/// The hold window while the person hasn't opened the conversation since
/// their last push.
pub const BACKOFF: Duration = Duration::from_secs(5 * 60);
/// Chat alerts one person gets an hour; past it, badge corrections only.
pub const PERSON_PER_HOUR: usize = 60;
/// Old items' passive summaries per feed page (HC-4).
pub const OLD_SUMMARIES_PER_PAGE: usize = 5;
/// Sew's preview is at most this long (the push carries no more).
pub const TEXT_MAX_CHARS: usize = 120;
/// The `thread_viewers` conversation key prefix of a Home conversation.
pub const LEASE_PREFIX: &str = "hc:";

/// A sender as this server knows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sender {
    /// `k2:owner`, `k2:connect:<uuid>` or `k2:workspace:<uuid>` (an agent).
    pub ext: String,
    pub name: String,
}

/// A person's phones, as a push reaches them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    /// The person key (badge key, budgets).
    pub principal: String,
    /// The device logins; empty = nobody (no phone, bell off).
    pub logins: Vec<String>,
}

/// What the rules need from the daemon (a fake in tests).
pub trait Env: Send + Sync {
    /// `homeChatPush` is on.
    fn push_on(&self) -> bool;
    /// The owner's switches let text reach the push path (D6).
    fn previews_allowed(&self) -> bool;
    /// The K5 Space's Sew id.
    fn k5_space(&self) -> Option<String>;
    /// `conv` is one of Home's own DMs for `ext` (K2's DM ledger, or the
    /// person's followed state).
    fn is_home_dm(&self, ext: &str, conv: &str) -> bool;
    /// The item's sender (`None` = not known here: never pushed).
    fn sender(&self, item: &FeedItem) -> Option<Sender>;
    /// A channel's name (`""` unknown).
    fn channel_name(&self, conv: &str) -> String;
    /// A sender's person-picture rev (`""` = no picture).
    fn avatar_rev(&self, ext: &str) -> String;
    /// `ext` is still in the channel, or the DM isn't gone (HC-7).
    fn still_in(&self, ext: &str, conv: &str, dm: bool) -> bool;
    /// `ext`'s phones now (`None` = no live login: disabled, removed).
    fn recipient(&self, ext: &str) -> Option<Recipient>;
    fn prefs(&self, ext: &str) -> PushPrefs;
    /// A client of `ext` holds a live viewing lease on `conv`.
    fn viewing(&self, ext: &str, conv: &str, now: Instant) -> bool;
    /// `ext`'s read cursor in `conv` (a Sew message id), when known.
    fn read_through(&self, ext: &str, conv: &str) -> Option<String>;
    /// The Thread and project badge totals (`unreadTotal`, `waitingTotal`).
    fn totals(&self, principal: &str) -> (String, String);
    /// `homeUnread`, or `""` when no held session knows it.
    fn home_unread(&self, ext: &str) -> String;
    /// Take the gateway calls this push makes from the daemon's budget.
    fn take_budget(&self, r: &Recipient, now: Instant) -> bool;
    fn send(&self, event: PushEvent);
    /// A badge correction for `principal` (an alert that didn't go).
    fn badge(&self, principal: &str);
}

/// One held message. `preview` is message text: `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct Pending {
    pub message_id: String,
    pub at: i64,
    pub sender: Sender,
    pub sender_member_id: String,
    pub reason: HomeChatReason,
    pub dm: bool,
    preview: Option<String>,
}

impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("message_id", &self.message_id)
            .field("at", &self.at)
            .field("sender", &self.sender.ext)
            .field("reason", &self.reason)
            .field("dm", &self.dm)
            .field("preview", &self.preview.as_ref().map(|_| "<hidden>"))
            .finish()
    }
}

/// Sew ids are time-sortable decimals: `a` is after `b`.
pub fn id_after(a: &str, b: &str) -> bool {
    match (a.parse::<i64>(), b.parse::<i64>()) {
        (Ok(x), Ok(y)) => x > y,
        _ => (a.len(), a) > (b.len(), b),
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

/// The text a push may carry: at most [`TEXT_MAX_CHARS`] characters.
fn held_text(t: &str) -> (String, bool) {
    let t = t.trim();
    if t.chars().count() <= TEXT_MAX_CHARS {
        (t.to_string(), false)
    } else {
        (t.chars().take(TEXT_MAX_CHARS).collect(), true)
    }
}

#[derive(Debug, Default)]
struct Window {
    last_push: Option<Instant>,
    /// The person opened (viewed or read) the conversation since `last_push`.
    opened: bool,
    /// Items wait until then (the window after the last push).
    open_until: Option<Instant>,
    pending: Vec<Pending>,
    due: Option<Instant>,
}

/// Every person's windows and caps. Pure: the [`Env`] does the rest.
#[derive(Debug, Default)]
pub struct Coalescer {
    windows: HashMap<(String, String), Window>,
    sent: HashMap<String, VecDeque<Instant>>,
}

impl Coalescer {
    pub fn new() -> Self {
        Self::default()
    }

    /// One feed page's message items for `ext` (`now_unix` for the age
    /// guard, `now` for the windows). Returns the alerts that went.
    pub fn on_items(&mut self, env: &dyn Env, ext: &str, items: &[FeedItem], now_unix: i64, now: Instant) -> usize {
        if !env.push_on() {
            return 0;
        }
        let k5 = env.k5_space();
        let prefs = env.prefs(ext);
        let text_ok = env.previews_allowed() && prefs.previews;
        let mut fresh = Vec::new();
        let mut old: BTreeMap<String, Vec<Pending>> = BTreeMap::new();
        for it in items.iter().filter(|i| MESSAGE_KINDS.contains(&i.kind.as_str())) {
            let (Some(conv), Some(msg)) = (it.channel_id.clone(), it.message_id.clone()) else { continue };
            if it.app_id.is_some() {
                continue; // an App's item: never Home's (HC-5)
            }
            let dm = match &it.space_id {
                Some(space) if Some(space) == k5.as_ref() => false,
                Some(_) => continue, // another Space
                None if env.is_home_dm(ext, &conv) => true,
                None => continue, // a DM Home doesn't keep
            };
            let Some(sender) = env.sender(it) else { continue };
            if sender.ext == ext {
                continue; // the person's own post (Sew sends none; a belt)
            }
            let agent = sender.ext.starts_with("k2:workspace:");
            if agent && !prefs.agents {
                continue; // D4: agents' posts only with the opt-in
            }
            if !agent && !k2_core::home_chat::is_box_person(&sender.ext) {
                continue; // HC-6
            }
            let preview = if text_ok { it.preview().map(str::to_string) } else { None };
            let p = Pending {
                message_id: msg,
                at: it.at.unwrap_or(0),
                sender,
                sender_member_id: it.started_by.clone().unwrap_or_default(),
                reason: if dm && it.kind == "dm" { HomeChatReason::Dm } else { reason_of(&it.kind) },
                dm,
                preview,
            };
            if it.older_than(ITEM_MAX_AGE_SECS, now_unix) {
                old.entry(conv).or_default().push(p);
            } else {
                fresh.push((conv, p));
            }
        }
        let mut sent = 0;
        for (conv, p) in fresh {
            sent += usize::from(self.add(env, ext, &conv, p, now));
        }
        if !old.is_empty() {
            // Newest conversations first; the rest only correct the badge.
            let mut convs: Vec<(String, Vec<Pending>)> = old.into_iter().collect();
            convs.sort_by(|a, b| {
                let newest = |v: &Vec<Pending>| v.iter().map(|p| p.message_id.clone()).max_by(|x, y| if id_after(x, y) { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less });
                let (x, y) = (newest(&a.1).unwrap_or_default(), newest(&b.1).unwrap_or_default());
                if id_after(&x, &y) {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            });
            let rest = convs.split_off(convs.len().min(OLD_SUMMARIES_PER_PAGE));
            for (conv, items) in convs {
                sent += usize::from(self.push(env, ext, &conv, items, true, now));
            }
            if !rest.is_empty() {
                if let Some(r) = env.recipient(ext) {
                    env.badge(&r.principal);
                }
            }
        }
        sent
    }

    /// Hold `p`, or push at once when no window is open. True when an
    /// alert went.
    fn add(&mut self, env: &dyn Env, ext: &str, conv: &str, p: Pending, now: Instant) -> bool {
        let key = (ext.to_string(), conv.to_string());
        let w = self.windows.entry(key).or_default();
        if w.pending.iter().any(|x| x.message_id == p.message_id) {
            return false; // the same item again (a repeated page)
        }
        w.pending.push(p);
        if w.due.is_some() {
            return false;
        }
        match w.open_until {
            Some(until) if now < until => {
                w.due = Some(until);
                false
            }
            _ => self.flush(env, ext, conv, now),
        }
    }

    /// Send what `(ext, conv)` holds, if the rules let it. True when an
    /// alert went.
    fn flush(&mut self, env: &dyn Env, ext: &str, conv: &str, now: Instant) -> bool {
        let key = (ext.to_string(), conv.to_string());
        let Some(w) = self.windows.get_mut(&key) else { return false };
        w.due = None;
        let pending = std::mem::take(&mut w.pending);
        if pending.is_empty() {
            return false;
        }
        self.push(env, ext, conv, pending, false, now)
    }

    /// The flush rules, then one push for `items` (oldest first). `passive`:
    /// an old items' summary (no window).
    fn push(&mut self, env: &dyn Env, ext: &str, conv: &str, mut items: Vec<Pending>, passive: bool, now: Instant) -> bool {
        let Some(r) = env.recipient(ext) else { return false };
        if r.logins.is_empty() {
            return false;
        }
        let dm = items.iter().any(|p| p.dm);
        if !env.still_in(ext, conv, dm) {
            return false; // HC-7: removed during the window, text and all
        }
        let key = (ext.to_string(), conv.to_string());
        if env.viewing(ext, conv, now) {
            self.windows.entry(key).or_default().opened = true;
            return false;
        }
        if let Some(read) = env.read_through(ext, conv) {
            items.retain(|p| id_after(&p.message_id, &read));
        }
        items.sort_by(|a, b| if id_after(&a.message_id, &b.message_id) { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less });
        let Some(newest) = items.last().cloned() else { return false };
        let q = self.sent.entry(ext.to_string()).or_default();
        while q.front().is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(3600)) {
            q.pop_front();
        }
        if q.len() >= PERSON_PER_HOUR || !env.take_budget(&r, now) {
            env.badge(&r.principal);
            return false;
        }
        q.push_back(now);
        let mention = items.iter().any(|p| p.reason == HomeChatReason::Mention);
        let reason = if mention { HomeChatReason::Mention } else { newest.reason };
        let (unread_total, waiting_total) = env.totals(&r.principal);
        let (text, truncated) = newest.preview.as_deref().map(held_text).unwrap_or_default();
        let msg = newest.preview.as_ref().map(|_| HomeChatMsg {
            id: newest.message_id.clone(),
            conversation_id: conv.to_string(),
            from: newest.sender.ext.clone(),
            at: newest.at,
            text: text.clone(),
            truncated,
        });
        let event = PushEvent::HomeChat(HomeChatPush {
            principal: r.principal.clone(),
            device_usernames: r.logins.clone(),
            reason,
            conversation_id: conv.to_string(),
            dm: newest.dm,
            message_id: newest.message_id.clone(),
            count: u32::try_from(items.len()).unwrap_or(u32::MAX),
            mention,
            sender_ext: newest.sender.ext.clone(),
            sender_member_id: newest.sender_member_id.clone(),
            sender_name: newest.sender.name.clone(),
            avatar_rev: env.avatar_rev(&newest.sender.ext),
            channel_name: if newest.dm { String::new() } else { env.channel_name(conv) },
            unread_total,
            waiting_total,
            home_unread: env.home_unread(ext),
            preview: text,
            msg,
            passive,
        });
        env.send(event);
        if !passive {
            let w = self.windows.entry(key).or_default();
            let len = if w.last_push.is_some() && !w.opened { BACKOFF } else { WINDOW };
            w.last_push = Some(now);
            w.opened = false;
            w.open_until = Some(now + len);
        }
        true
    }

    /// Flush every window that is due.
    pub fn tick(&mut self, env: &dyn Env, now: Instant) {
        let due: Vec<(String, String)> = self.windows.iter().filter(|(_, w)| w.due.is_some_and(|t| now >= t)).map(|(k, _)| k.clone()).collect();
        for (ext, conv) in due {
            self.flush(env, &ext, &conv, now);
        }
        // Forget quiet windows (nothing held, the window long over).
        self.windows.retain(|_, w| !w.pending.is_empty() || w.open_until.is_some_and(|t| now.saturating_duration_since(t) < BACKOFF));
        self.sent.retain(|_, q| q.back().is_some_and(|t| now.saturating_duration_since(*t) < Duration::from_secs(3600)));
    }

    /// The person opened `conv` (a lease or a read): the window resets to
    /// [`WINDOW`], and held items they read go.
    pub fn opened(&mut self, ext: &str, conv: &str, read_through: Option<&str>) {
        if let Some(w) = self.windows.get_mut(&(ext.to_string(), conv.to_string())) {
            w.opened = true;
            if let Some(read) = read_through {
                w.pending.retain(|p| id_after(&p.message_id, read));
                if w.pending.is_empty() {
                    w.due = None;
                }
            }
        }
    }

    /// HC-7: `ext`'s poller stopped: every window of theirs goes.
    pub fn forget_person(&mut self, ext: &str) {
        self.windows.retain(|(e, _), _| e != ext);
    }

    /// Tests: what `(ext, conv)` holds and when it is due.
    #[cfg(test)]
    pub fn held(&self, ext: &str, conv: &str) -> (usize, Option<Instant>) {
        self.windows.get(&(ext.to_string(), conv.to_string())).map_or((0, None), |w| (w.pending.len(), w.due))
    }
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

/// Tests: flush what is due at `now`.
#[doc(hidden)]
pub fn tick_for_tests(now: Instant) {
    let e = env();
    lock().tick(e.as_ref(), now);
}

fn ensure_ticker() {
    #[cfg(not(test))]
    {
        static STARTED: std::sync::Once = std::sync::Once::new();
        STARTED.call_once(|| {
            let spawned = std::thread::Builder::new().name("home-chat-push-tick".into()).spawn(|| loop {
                std::thread::sleep(Duration::from_secs(1));
                let e = env();
                lock().tick(e.as_ref(), Instant::now());
            });
            if let Err(e) = spawned {
                k2_core::log_debug!("[home-chat] push ticker didn't start: {e}");
            }
        });
    }
}

/// One feed page for `ext`: message items into the windows, read markers
/// to the badge path. Returns the alerts that went.
pub fn on_feed(ext: &str, items: &[FeedItem], now_unix: i64) -> usize {
    for it in items.iter().filter(|i| i.kind == READ_KIND) {
        if let (Some(conv), Some(msg)) = (&it.channel_id, &it.message_id) {
            read_moved(ext, conv, msg);
        }
    }
    if !items.iter().any(|i| MESSAGE_KINDS.contains(&i.kind.as_str())) {
        return 0;
    }
    ensure_ticker();
    let e = env();
    lock().on_items(e.as_ref(), ext, items, now_unix, Instant::now())
}

/// `ext` read `conv` through `last_read_id` (the follower's
/// `read_state_updated`, or Sew 0.11.0's `read` item): held items they read
/// go, and their phones may clear that conversation's pushes (`h:` hint,
/// only when an alert may be showing).
pub fn read_moved(ext: &str, conv: &str, last_read_id: &str) {
    lock().opened(ext, conv, Some(last_read_id));
    let Ok(seq) = last_read_id.parse::<i64>() else { return };
    if let Some((principal, _)) = (super::deps().devices)(ext) {
        crate::badge_push::read_moved(&principal, k2_core::home_chat::READ_KIND, conv, seq);
    }
}

/// A viewing frame from one of `ext`'s Home chat sockets (HC-15).
pub fn lease(socket: u64, ext: &str, conv: &str, visible: bool) {
    crate::thread_viewers::global().set(socket, ext, &format!("{LEASE_PREFIX}{conv}"), visible, Instant::now());
    if visible {
        lock().opened(ext, conv, None);
    }
}

/// One of the person's Home chat sockets closed: its leases go.
pub fn close_socket(socket: u64) {
    crate::thread_viewers::global().close_socket(socket);
}

/// A client text frame on `WS /cli/home-chat/events`:
/// `{"t":"viewing","conv":"<sew id>","visible":bool}` → `(conv, visible)`.
/// Anything else is ignored.
pub fn parse_viewing(text: &str) -> Option<(String, bool)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("t").and_then(|t| t.as_str()) != Some("viewing") {
        return None;
    }
    let conv = v.get("conv").and_then(|c| c.as_str()).map(str::trim).filter(|c| k2_core::tool_hook::sew_wire::is_sew_id(c))?;
    Some((conv.to_string(), v.get("visible")?.as_bool()?))
}

/// HC-7: `ext`'s poller stopped.
pub fn forget_person(ext: &str) {
    lock().forget_person(ext);
}

/// After each page (HC-20): on a person's first poll with pushes on and a
/// live held session, their K5 Space level goes to @mentions once, if Sew
/// resolves All for them and they never chose. Recorded either way; never
/// again, never over a choice. Failures are logged (status and code only)
/// and tried again on a later page.
pub fn after_page(ext: &str) {
    static DONE: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let done = DONE.get_or_init(Default::default);
    if !k2_core::home_chat::push_enabled() || done.lock().unwrap_or_else(|p| p.into_inner()).contains(ext) {
        return;
    }
    if !super::follower::get(ext).is_some_and(|h| h.status() == "live") {
        return;
    }
    match set_space_default(ext) {
        Ok(()) => {
            done.lock().unwrap_or_else(|p| p.into_inner()).insert(ext.to_string());
        }
        Err(e) => k2_core::log_debug!("[home-chat] {ext}: Space notify default: {e}"),
    }
}

fn set_space_default(ext: &str) -> Result<(), String> {
    use k2_core::tool_hook::home_push;
    let now = super::now_secs();
    if k2_core::tool_hook::with_home_chat_db(|c| home_push::prefs(c, ext))?.space_default_set {
        return Ok(());
    }
    let space = k2_core::tool_hook::admin_space_id()?.ok_or("Home's Space isn't ready")?;
    let path = format!("/api/v1/spaces/{space}/notify");
    let got = super::routes::sew_as(ext, "GET", &path, None).map_err(|r| format!("{} {}", r.status, r.code))?;
    if !got.ok() {
        return Err(format!("{path} answered {}", got.status));
    }
    let unset = got.body.get("level").is_none_or(serde_json::Value::is_null);
    if unset && got.body["effective_level"].as_str() == Some("all") {
        let set = super::routes::sew_as(ext, "POST", &format!("{path}/set"), Some(&serde_json::json!({"level": "mentions"})))
            .map_err(|r| format!("{} {}", r.status, r.code))?;
        if !set.ok() {
            return Err(format!("{path}/set answered {}", set.status));
        }
    }
    k2_core::tool_hook::with_home_chat_db(|c| home_push::mark_space_default_set(c, ext, now))
}

/// The person's unread Home conversations, from their held session (`None`
/// when none is held: HC-13 leaves `homeUnread` out then).
pub fn home_unread_of(ext: &str) -> Option<u32> {
    let h = super::follower::get(ext)?;
    let st = h.state.lock().unwrap_or_else(|p| p.into_inner());
    Some(u32::try_from(st.convs.values().filter(|c| c.unread).count()).unwrap_or(u32::MAX))
}

/// [`home_unread_of`] for a person key (`owner`, or a Connect login's
/// username): the badge path's `homeUnread`.
pub fn home_unread_for_person(person: &str) -> Option<u32> {
    if person == k2_core::notify_prefs::OWNER {
        return home_unread_of("k2:owner");
    }
    let id = k2_core::connect_users::admin_space_facts()
        .ok()?
        .into_iter()
        .find(|(_, u, ..)| u.eq_ignore_ascii_case(person))
        .map(|(id, ..)| id)
        .filter(|id| !id.is_empty())?;
    home_unread_of(&format!("k2:connect:{id}"))
}

/// The daemon's stores.
pub struct LiveEnv;

impl Env for LiveEnv {
    fn push_on(&self) -> bool {
        k2_core::home_chat::push_enabled()
    }

    fn previews_allowed(&self) -> bool {
        k2_core::home_chat::push_previews_allowed()
    }

    fn k5_space(&self) -> Option<String> {
        k2_core::tool_hook::admin_space_id().ok().flatten()
    }

    fn is_home_dm(&self, ext: &str, conv: &str) -> bool {
        let ledger = k2_core::tool_hook::with_home_chat_db(|c| k2_core::tool_hook::admin_channels::dm_other(c, ext, conv)).ok().flatten().is_some();
        ledger
            || super::follower::get(ext).is_some_and(|h| {
                h.state.lock().unwrap_or_else(|p| p.into_inner()).convs.get(conv).is_some_and(|c| c.dm && c.space_id.is_none())
            })
    }

    fn sender(&self, item: &FeedItem) -> Option<Sender> {
        let row = match item.started_by.as_deref() {
            Some(m) => k2_core::tool_hook::chat_member_by_id(m).ok().flatten(),
            None => None,
        };
        let ext = item.from_ext.clone().or_else(|| row.as_ref().map(|r| r.external_id.clone()))?;
        let name = if ext == "k2:owner" {
            crate::workspace_msg::resolve_owner_from()
        } else if let Some(uuid) = ext.strip_prefix("k2:connect:") {
            super::people::logins().ok()?.into_iter().find(|l| l.id == uuid).map(|l| l.display_name)?
        } else {
            row.map(|r| r.username).unwrap_or_default()
        };
        Some(Sender { ext, name })
    }

    fn avatar_rev(&self, ext: &str) -> String {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::person_picture::rev_of_ext(&conn, ext).ok().flatten().unwrap_or_default()
    }

    fn channel_name(&self, conv: &str) -> String {
        k2_core::tool_hook::with_home_chat_db(|c| k2_core::tool_hook::admin_channels::channel_by_id(c, conv))
            .ok()
            .flatten()
            .map(|ch| ch.desired_name().to_string())
            .unwrap_or_default()
    }

    fn still_in(&self, ext: &str, conv: &str, dm: bool) -> bool {
        use k2_core::tool_hook::admin_channels as ledger;
        if dm {
            return k2_core::tool_hook::with_home_chat_db(|c| ledger::dm_gone(c, ext, conv)).is_ok_and(|gone| !gone);
        }
        match k2_core::tool_hook::with_home_chat_db(|c| ledger::channel_by_id(c, conv)) {
            Ok(Some(ch)) => k2_core::tool_hook::with_home_chat_db(|c| ledger::is_member(c, &ch.slug, ext)).unwrap_or(false),
            // Not in K2's ledger: the person's own Sew view decides.
            Ok(None) => super::follower::get(ext).is_some_and(|h| h.state.lock().unwrap_or_else(|p| p.into_inner()).convs.contains_key(conv)),
            Err(_) => false,
        }
    }

    fn recipient(&self, ext: &str) -> Option<Recipient> {
        let (principal, logins) = (super::deps().devices)(ext)?;
        let bell = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::notify_prefs::tables_exist(&conn)
                && k2_core::notify_prefs::PrefsSnapshot::load(&conn).map(|p| p.bell(&principal).on).unwrap_or(false)
        };
        Some(Recipient { principal, logins: if bell { logins } else { Vec::new() } })
    }

    fn prefs(&self, ext: &str) -> PushPrefs {
        k2_core::tool_hook::with_home_chat_db(|c| k2_core::tool_hook::home_push::prefs(c, ext)).unwrap_or_default()
    }

    fn viewing(&self, ext: &str, conv: &str, now: Instant) -> bool {
        crate::thread_viewers::global().is_viewing(ext, &format!("{LEASE_PREFIX}{conv}"), now)
    }

    fn read_through(&self, ext: &str, conv: &str) -> Option<String> {
        let h = super::follower::get(ext)?;
        let st = h.state.lock().unwrap_or_else(|p| p.into_inner());
        st.convs.get(conv).and_then(|c| c.last_read_id.clone())
    }

    fn totals(&self, principal: &str) -> (String, String) {
        match crate::badge_push::totals_for(principal) {
            Some(t) => (t.total.to_string(), t.waiting_total.to_string()),
            None => (String::new(), String::new()),
        }
    }

    fn home_unread(&self, ext: &str) -> String {
        home_unread_of(ext).map(|n| n.to_string()).unwrap_or_default()
    }

    fn take_budget(&self, r: &Recipient, now: Instant) -> bool {
        let devices = k2_core::push_devices::list_devices_for_usernames(&r.logins).unwrap_or_default();
        crate::push_routes::take_home_chat_budget(now, crate::push_routes::home_chat_calls(&devices))
    }

    fn send(&self, event: PushEvent) {
        crate::push_routes::deliver_home_chat(event);
    }

    fn badge(&self, principal: &str) {
        crate::badge_push::trigger(principal);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const ME: &str = "k2:connect:me";
    const SAM: &str = "k2:connect:sam";
    pub(crate) const K5: &str = "7001";
    pub(crate) const SECRET: &str = "sentinel-text-7f3a";

    /// A fake daemon: every rule's answer is a field.
    pub(crate) struct Fake {
        pub push_on: bool,
        pub previews: bool,
        pub prefs: Mutex<PushPrefs>,
        pub recipient: Mutex<Option<Recipient>>,
        pub still_in: Mutex<bool>,
        pub viewing: Mutex<bool>,
        pub read: Mutex<Option<String>>,
        pub budget: Mutex<usize>,
        pub sent: Mutex<Vec<HomeChatPush>>,
        pub badges: Mutex<Vec<String>>,
    }

    impl Default for Fake {
        fn default() -> Self {
            Fake {
                push_on: true,
                previews: true,
                prefs: Mutex::new(PushPrefs::default()),
                recipient: Mutex::new(Some(Recipient { principal: "me".into(), logins: vec!["me".into()] })),
                still_in: Mutex::new(true),
                viewing: Mutex::new(false),
                read: Mutex::new(None),
                budget: Mutex::new(1000),
                sent: Mutex::new(Vec::new()),
                badges: Mutex::new(Vec::new()),
            }
        }
    }

    impl Fake {
        pub(crate) fn sent(&self) -> Vec<HomeChatPush> {
            self.sent.lock().unwrap().clone()
        }
    }

    impl Env for Fake {
        fn push_on(&self) -> bool {
            self.push_on
        }
        fn previews_allowed(&self) -> bool {
            self.previews
        }
        fn k5_space(&self) -> Option<String> {
            Some(K5.into())
        }
        fn is_home_dm(&self, _: &str, conv: &str) -> bool {
            conv == "900"
        }
        fn sender(&self, item: &FeedItem) -> Option<Sender> {
            match item.started_by.as_deref()? {
                "42" => Some(Sender { ext: SAM.into(), name: "Sam Lee".into() }),
                "41" => Some(Sender { ext: ME.into(), name: "Me".into() }),
                "50" => Some(Sender { ext: "k2:workspace:w1".into(), name: "billing".into() }),
                "60" => Some(Sender { ext: "sew:system".into(), name: String::new() }),
                _ => None,
            }
        }
        fn channel_name(&self, conv: &str) -> String {
            if conv == "901" { "standup".into() } else { String::new() }
        }
        fn avatar_rev(&self, ext: &str) -> String {
            if ext == SAM { "rev-sam-hash".into() } else { String::new() }
        }
        fn still_in(&self, _: &str, _: &str, _: bool) -> bool {
            *self.still_in.lock().unwrap()
        }
        fn recipient(&self, _: &str) -> Option<Recipient> {
            self.recipient.lock().unwrap().clone()
        }
        fn prefs(&self, _: &str) -> PushPrefs {
            *self.prefs.lock().unwrap()
        }
        fn viewing(&self, _: &str, _: &str, _: Instant) -> bool {
            *self.viewing.lock().unwrap()
        }
        fn read_through(&self, _: &str, _: &str) -> Option<String> {
            self.read.lock().unwrap().clone()
        }
        fn totals(&self, _: &str) -> (String, String) {
            ("3".into(), "1".into())
        }
        fn home_unread(&self, _: &str) -> String {
            "2".into()
        }
        fn take_budget(&self, _: &Recipient, _: Instant) -> bool {
            let mut b = self.budget.lock().unwrap();
            if *b == 0 {
                return false;
            }
            *b -= 1;
            true
        }
        fn send(&self, event: PushEvent) {
            let PushEvent::HomeChat(h) = event else { panic!("not a home chat push") };
            self.sent.lock().unwrap().push(h);
        }
        fn badge(&self, principal: &str) {
            self.badges.lock().unwrap().push(principal.to_string());
        }
    }

    const NOW: i64 = 1_800_000_000;

    fn item(kind: &str, conv: &str, msg: &str, from: &str, space: Option<&str>, age: i64) -> FeedItem {
        let mut it = FeedItem::default().with_preview(SECRET);
        it.seq = 1;
        it.kind = kind.into();
        it.at = Some(NOW - age);
        it.channel_id = Some(conv.into());
        it.message_id = Some(msg.into());
        it.space_id = space.map(str::to_string);
        it.started_by = Some(from.into());
        it
    }

    fn dm(msg: &str) -> FeedItem {
        item("dm", "900", msg, "42", None, 1)
    }

    fn mention(msg: &str) -> FeedItem {
        item("mention", "901", msg, "42", Some(K5), 1)
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// One DM: one push at once with §5's fields; a second within 45 s is
    /// held and goes as one trailing `2 new messages` for the same chat.
    #[test]
    fn a_dm_pushes_at_once_and_a_second_trails_after_the_window() {
        let env = Fake::default();
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        assert_eq!(c.on_items(&env, ME, &[dm("1001")], NOW, t0), 1);
        let s = env.sent();
        assert_eq!(s.len(), 1);
        let h = &s[0];
        assert_eq!((h.reason, h.dm, h.conversation_id.as_str(), h.message_id.as_str(), h.count), (HomeChatReason::Dm, true, "900", "1001", 1));
        assert_eq!((h.sender_ext.as_str(), h.sender_name.as_str(), h.sender_member_id.as_str()), (SAM, "Sam Lee", "42"));
        assert_eq!((h.unread_total.as_str(), h.waiting_total.as_str(), h.home_unread.as_str()), ("3", "1", "2"));
        assert_eq!(h.preview, SECRET, "text when every switch is on");
        assert_eq!(h.avatar_rev, "rev-sam-hash", "the sender's picture rev");
        assert_eq!(h.msg.as_ref().map(|m| m.text.as_str()), Some(SECRET));
        assert!(!h.passive);
        let e = PushEvent::HomeChat(h.clone());
        assert_eq!(e.title(), "Sam Lee");
        assert_eq!(e.data().get("kind"), Some("home_chat"));
        assert_eq!(e.data().get("convKind"), Some("dm"));
        assert_eq!(e.collapse_id(), None, "HC-9: the NSE replaces the banner by group");
        assert_eq!(c.on_items(&env, ME, &[dm("1002")], NOW, t0 + secs(10)), 0, "held");
        assert_eq!(c.held(ME, "900"), (1, Some(t0 + WINDOW)));
        c.tick(&env, t0 + secs(44));
        assert_eq!(env.sent().len(), 1, "still open");
        c.tick(&env, t0 + WINDOW);
        let s = env.sent();
        assert_eq!(s.len(), 2, "leading + trailing");
        assert_eq!((s[1].count, s[1].message_id.as_str()), (1, "1002"));
        assert_eq!(PushEvent::HomeChat(s[1].clone()).body(), SECRET);
    }

    /// Three in a window: the trailing push counts the two held; a mention
    /// among them makes it a mention.
    #[test]
    fn the_trailing_push_counts_and_a_mention_wins() {
        let env = Fake::default();
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.on_items(&env, ME, &[item("all", "901", "2001", "42", Some(K5), 1)], NOW, t0);
        c.on_items(&env, ME, &[mention("2002"), item("all", "901", "2003", "42", Some(K5), 1)], NOW, t0 + secs(5));
        c.tick(&env, t0 + WINDOW);
        let s = env.sent();
        assert_eq!(s.len(), 2);
        assert_eq!((s[1].count, s[1].reason, s[1].mention, s[1].message_id.as_str()), (2, HomeChatReason::Mention, true, "2003"));
        let e = PushEvent::HomeChat(s[1].clone());
        assert_eq!(e.title(), "#standup · Sam Lee");
        assert!(e.body().starts_with("2 new messages · "), "{}", e.body());
        assert_eq!(e.data().get("channelName"), Some("standup"));
        assert_eq!(e.data().get("mention"), Some("1"));
    }

    /// Unread since the last push: the next window is 5 min; a lease resets
    /// it to 45 s.
    #[test]
    fn no_open_backs_off_to_5_minutes_and_a_view_resets() {
        let env = Fake::default();
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        c.on_items(&env, ME, &[dm("1")], NOW, t0);
        c.on_items(&env, ME, &[dm("2")], NOW, t0 + secs(5));
        c.tick(&env, t0 + WINDOW); // trailing; not opened since #1 → 5 min
        c.on_items(&env, ME, &[dm("3")], NOW, t0 + WINDOW + secs(10));
        assert_eq!(c.held(ME, "900").1, Some(t0 + WINDOW + BACKOFF));
        c.opened(ME, "900", None);
        c.tick(&env, t0 + WINDOW + BACKOFF);
        assert_eq!(env.sent().len(), 3);
        c.on_items(&env, ME, &[dm("4")], NOW, t0 + WINDOW + BACKOFF + secs(1));
        assert_eq!(c.held(ME, "900").1, Some(t0 + WINDOW + BACKOFF + WINDOW), "opened: 45 s again");
    }

    /// §6 quiet rules: own posts, unknown senders, Sew's own posts (HC-6),
    /// foreign Spaces and Apps (HC-5), and agents unless opted in (D4).
    #[test]
    fn quiet_rules_drop_at_once() {
        let env = Fake::default();
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        let mut app = mention("3001");
        app.app_id = Some("app-1".into());
        let items = [
            item("dm", "900", "3002", "41", None, 1),               // own
            item("mention", "901", "3003", "99", Some(K5), 1),      // unknown member
            item("reply_to_me", "901", "3004", "60", Some(K5), 1),  // a transcript turn / system post
            item("mention", "901", "3005", "42", Some("8888"), 1),  // another Space
            item("dm", "955", "3006", "42", None, 1),               // a DM Home doesn't keep
            item("mention", "901", "3007", "50", Some(K5), 1),      // an agent, no opt-in
            app,
        ];
        assert_eq!(c.on_items(&env, ME, &items, NOW, t0), 0);
        assert!(env.sent().is_empty(), "{:?}", env.sent());
        env.prefs.lock().unwrap().agents = true;
        assert_eq!(c.on_items(&env, ME, &[item("mention", "901", "3008", "50", Some(K5), 1)], NOW, t0), 1, "opted in");
        assert_eq!(env.sent()[0].sender_ext, "k2:workspace:w1");
        let off = Fake { push_on: false, ..Fake::default() };
        assert_eq!(Coalescer::new().on_items(&off, ME, &[dm("1")], NOW, t0), 0, "homeChatPush off: nothing");
    }

    /// Checked at flush: a lease, a read past it, a removal (HC-7), no live
    /// login or the bell off: nothing goes.
    #[test]
    fn flush_rules_quiet_a_held_push() {
        let t0 = Instant::now();
        // A lease.
        let env = Fake::default();
        *env.viewing.lock().unwrap() = true;
        assert_eq!(Coalescer::new().on_items(&env, ME, &[dm("1")], NOW, t0), 0);
        // Read past it.
        let env = Fake::default();
        *env.read.lock().unwrap() = Some("1001".into());
        assert_eq!(Coalescer::new().on_items(&env, ME, &[dm("1001")], NOW, t0), 0);
        // Removed during the window: no trailing push.
        let env = Fake::default();
        let mut c = Coalescer::new();
        c.on_items(&env, ME, &[mention("1")], NOW, t0);
        c.on_items(&env, ME, &[mention("2")], NOW, t0 + secs(5));
        *env.still_in.lock().unwrap() = false;
        c.tick(&env, t0 + WINDOW);
        assert_eq!(env.sent().len(), 1, "removed: the held item is dropped");
        assert_eq!(c.held(ME, "901").0, 0, "text and all");
        // A disabled login, or the bell off.
        let env = Fake::default();
        *env.recipient.lock().unwrap() = None;
        assert_eq!(Coalescer::new().on_items(&env, ME, &[dm("1")], NOW, t0), 0);
        let env = Fake::default();
        *env.recipient.lock().unwrap() = Some(Recipient { principal: "me".into(), logins: vec![] });
        assert_eq!(Coalescer::new().on_items(&env, ME, &[dm("1")], NOW, t0), 0);
        // A poller that stops drops its windows.
        let env = Fake::default();
        let mut c = Coalescer::new();
        c.on_items(&env, ME, &[dm("1")], NOW, t0);
        c.on_items(&env, ME, &[dm("2")], NOW, t0 + secs(1));
        c.forget_person(ME);
        c.tick(&env, t0 + WINDOW);
        assert_eq!(env.sent().len(), 1);
    }

    /// D6, HC-16: either preview switch off → no text anywhere, the fixed
    /// body.
    #[test]
    fn no_text_when_either_switch_is_off() {
        let t0 = Instant::now();
        for (server, person) in [(false, true), (true, false), (false, false)] {
            let env = Fake { previews: server, ..Fake::default() };
            env.prefs.lock().unwrap().previews = person;
            Coalescer::new().on_items(&env, ME, &[mention("5")], NOW, t0);
            let h = env.sent().pop().expect("a push");
            assert_eq!((h.preview.as_str(), h.msg.is_none()), ("", true), "server {server} person {person}");
            let e = PushEvent::HomeChat(h);
            assert_eq!(e.body(), "Mentioned you");
            assert!(!format!("{e:?}").contains(SECRET));
        }
    }

    /// HC-4: old items alert one passive summary per conversation, at most 5
    /// a page; the rest only correct the badge. A restart's 20-minute-old
    /// item never alerts loudly.
    #[test]
    fn old_items_get_one_passive_summary_per_conversation() {
        let env = Fake::default();
        let mut c = Coalescer::new();
        let t0 = Instant::now();
        // Seven conversations, every item 20 minutes old: the DM (two items,
        // the oldest ids) and six channels.
        let mut items = vec![item("dm", "900", "10", "42", None, 1200), item("dm", "900", "11", "42", None, 1200)];
        for (i, conv) in ["901", "902", "903", "904", "905", "906"].iter().enumerate() {
            items.push(item("mention", conv, &format!("{}", 100 + i), "42", Some(K5), 1200));
        }
        let sent = c.on_items(&env, ME, &items, NOW, t0);
        assert_eq!(sent, OLD_SUMMARIES_PER_PAGE);
        let s = env.sent();
        assert!(s.iter().all(|h| h.passive && h.count == 1), "no sound for any of them");
        let mut convs: Vec<&str> = s.iter().map(|h| h.conversation_id.as_str()).collect();
        convs.sort_unstable();
        assert_eq!(convs, ["902", "903", "904", "905", "906"], "the newest five");
        assert_eq!(env.badges.lock().unwrap().as_slice(), ["me"], "the rest: one badge correction");
        assert_eq!(c.held(ME, "900"), (0, None), "old items are never held");
        assert!(PushEvent::HomeChat(s[0].clone()).is_silent_alert());
        assert_eq!(PushEvent::HomeChat(s[0].clone()).data().get("passive"), Some("1"));
    }

    /// HC-19 and the per-person cap: over either, a badge correction.
    #[test]
    fn over_budget_or_cap_becomes_a_badge() {
        let t0 = Instant::now();
        let env = Fake::default();
        *env.budget.lock().unwrap() = 0;
        assert_eq!(Coalescer::new().on_items(&env, ME, &[dm("1")], NOW, t0), 0);
        assert_eq!(env.badges.lock().unwrap().len(), 1);
        let env = Fake::default();
        let mut c = Coalescer::new();
        for i in 0..PERSON_PER_HOUR {
            let conv = format!("{}", 5000 + i);
            assert_eq!(c.on_items(&env, ME, &[item("mention", &conv, "1", "42", Some(K5), 1)], NOW, t0), 1);
        }
        assert_eq!(c.on_items(&env, ME, &[item("mention", "9999", "1", "42", Some(K5), 1)], NOW, t0), 0, "61st: capped");
        assert_eq!(env.badges.lock().unwrap().len(), 1);
    }

    #[test]
    fn viewing_frames_parse() {
        assert_eq!(parse_viewing(r#"{"t":"viewing","conv":"901","visible":true}"#), Some(("901".into(), true)));
        assert_eq!(parse_viewing(r#"{"t":"viewing","conv":"901","visible":false}"#), Some(("901".into(), false)));
        assert_eq!(parse_viewing(r#"{"t":"viewing","conv":"../x","visible":true}"#), None, "a Sew id only");
        assert_eq!(parse_viewing(r#"{"t":"typing","conv":"901"}"#), None);
        assert_eq!(parse_viewing("not json"), None);
        assert!(id_after("1002", "999"));
        assert!(!id_after("999", "1002"));
    }
}
