//! Phone ringing for DM calls (prd-home-people-channels-v1 §11a.3, slice
//! S14; HP-41).
//!
//! Sew pushes nothing to Apple or Google. It writes to the person's feed,
//! `GET /mgmt/v1/people/{external_id}/notifications` (Sew's
//! `MGMT_PERSON_NOTIFICATIONS`), and K2 does the push:
//!
//! **The feed** ([`poll_once`]): `?after=<n>&limit=<1-500>&wait=<0-30>`.
//! `after` is required: 0 the first time, then the last page's `next`, sent
//! back ALWAYS (it moves even when there are no items). The answer is
//! `{items, next}`; each item has `seq, kind, at, channel_id,
//! from_member_id, app_id`, and a call's also `call_id, message_id`. Kinds
//! `call_ring` (a loud ring only), `call_missed` (a ring that timed out, the
//! same `call_id`), `mention`, `dm`, `reply_to_me`; K2 acts on the first two
//! and ignores the rest. Sew holds the feed (`items: []`) during Do Not
//! Disturb. A ring answered, declined or cancelled before the poll never
//! shows, and the feed has no ring-ended item.
//! - `410 cursor_expired {oldest_seq, head}`: resume at `oldest_seq`; the
//!   gap pushes nothing.
//! - `404` (an unknown, agent or malformed id) and `403 not_provisioned`
//!   (deactivated): that person's poller stops (the supervisor tries again
//!   after [`STOPPED_FOR`]).
//! - Sew keeps one waiter per person (a second poll ends the first), so
//!   there is never more than one poller thread per person, even while one
//!   is stopping.
//!
//! **The ring's end.** The ring push is replaced, with the same collapse id
//! `call:<id>`, by exactly one push:
//! - at once, when the person's held session hears `call_ring {ringing:
//!   false, reason}` ([`ring_seen`]): `accepted` → "Answered on another
//!   device", `timeout` → "Missed call", `declined`, `cancelled`,
//!   `call_ended` → "Call ended";
//! - or the feed's `call_missed` → "Missed call";
//! - or, with no live held session to hear the end, "Call ended" once the
//!   ring is [`RING_FALLBACK_SECS`] old (Sew rings for 45 s).
//!
//! Push kind `call` ([`k2_core::push::PUSH_KIND_CALL`]), its own budget
//! class. Not VoIP or CallKit in v1.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;

use k2_core::push::{CallPush, CallPushState};

use super::{deps, now_secs};

/// How long Sew holds an empty feed read, seconds (Sew allows 0–30).
pub const FEED_WAIT_S: u64 = 25;
/// The shorter hold while a ring waits for its end (the fallback is timed).
pub const FEED_WAIT_RINGING_S: u64 = 5;
/// Items per read (Sew allows 1–500).
pub const FEED_LIMIT: u32 = 100;
/// A ring with no end heard is replaced after this long, when no live held
/// session would hear its end (Sew rings for 45 s, then writes
/// `call_missed`; the margin lets that come first).
pub const RING_FALLBACK_SECS: i64 = 55;
/// A poller Sew stopped (404, 403 `not_provisioned`) starts again after this.
pub const STOPPED_FOR: Duration = Duration::from_secs(600);
/// The supervisor's look at who has a push device.
const SUPERVISE_EVERY: Duration = Duration::from_secs(30);
/// Calls remembered per person (oldest dropped first).
const CALLS_KEPT: usize = 256;

/// One feed item K2 acts on (ids only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedItem {
    /// `call_ring` or `call_missed`.
    pub kind: String,
    pub call_id: String,
    pub channel_id: Option<String>,
    /// The caller (`from_member_id`).
    pub started_by: Option<String>,
}

fn id_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn u64_of(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// A feed page (`{items, next}`): its call items, and `next` (`None` when
/// the answer has none: not Sew's page).
pub fn parse_feed(body: &Value) -> (Vec<FeedItem>, Option<u64>) {
    let mut out = Vec::new();
    for e in body["items"].as_array().into_iter().flatten() {
        let kind = e["kind"].as_str().unwrap_or("");
        if kind != "call_ring" && kind != "call_missed" {
            continue; // mention, dm, reply_to_me, and kinds K2 doesn't know
        }
        let Some(call_id) = id_of(&e["call_id"]) else { continue };
        out.push(FeedItem { kind: kind.to_string(), call_id, channel_id: id_of(&e["channel_id"]), started_by: id_of(&e["from_member_id"]) });
    }
    (out, u64_of(&body["next"]))
}

/// A push to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub call_id: String,
    pub channel_id: String,
    pub started_by: Option<String>,
    pub state: CallPushState,
}

#[derive(Debug, Clone)]
struct Seen {
    channel_id: String,
    started_by: Option<String>,
    /// The ring push went (at this time) and nothing replaced it yet.
    ringing_since: Option<i64>,
    /// The replacement went (or nothing more is owed).
    done: bool,
}

/// One person's DM call pushes: each call gets at most one ring push and
/// one push that replaces it.
#[derive(Debug, Default)]
pub struct CallPushes {
    calls: HashMap<String, Seen>,
    order: VecDeque<String>,
}

impl CallPushes {
    fn remember(&mut self, id: &str, s: Seen) {
        if self.calls.insert(id.to_string(), s).is_none() {
            self.order.push_back(id.to_string());
        }
        while self.order.len() > CALLS_KEPT {
            if let Some(old) = self.order.pop_front() {
                self.calls.remove(&old);
            }
        }
    }

    /// Feed items in order (at `now`) → the pushes they make.
    pub fn on_feed(&mut self, items: &[FeedItem], now: i64) -> Vec<Action> {
        let mut out = Vec::new();
        for it in items {
            let known = self.calls.get(&it.call_id).cloned();
            match (it.kind.as_str(), known) {
                ("call_ring", None) => {
                    let channel_id = it.channel_id.clone().unwrap_or_default();
                    self.remember(&it.call_id, Seen { channel_id: channel_id.clone(), started_by: it.started_by.clone(), ringing_since: Some(now), done: false });
                    out.push(Action { call_id: it.call_id.clone(), channel_id, started_by: it.started_by.clone(), state: CallPushState::Ringing });
                }
                ("call_missed", known) if !known.as_ref().is_some_and(|k| k.done) => {
                    let channel_id = known.as_ref().map(|k| k.channel_id.clone()).filter(|c| !c.is_empty()).or_else(|| it.channel_id.clone()).unwrap_or_default();
                    let started_by = known.as_ref().and_then(|k| k.started_by.clone()).or_else(|| it.started_by.clone());
                    self.remember(&it.call_id, Seen { channel_id: channel_id.clone(), started_by: started_by.clone(), ringing_since: None, done: true });
                    out.push(Action { call_id: it.call_id.clone(), channel_id, started_by, state: CallPushState::Missed });
                }
                _ => {}
            }
        }
        out
    }

    fn replace(&mut self, call_id: &str, state: CallPushState) -> Option<Action> {
        let s = self.calls.get_mut(call_id)?;
        if s.ringing_since.is_none() || s.done {
            return None;
        }
        s.ringing_since = None;
        s.done = true;
        Some(Action { call_id: call_id.to_string(), channel_id: s.channel_id.clone(), started_by: s.started_by.clone(), state })
    }

    /// The held session heard `call_ring {ringing: false, reason}`: a ring K2
    /// pushed is replaced at once.
    pub fn on_ring_stopped(&mut self, call_id: &str, reason: &str) -> Option<Action> {
        let state = match reason {
            "accepted" => CallPushState::Answered,
            "timeout" => CallPushState::Missed,
            _ => CallPushState::Ended,
        };
        self.replace(call_id, state)
    }

    /// With no live held session: rings pushed [`RING_FALLBACK_SECS`] ago or
    /// more, with no end heard, are replaced by "Call ended".
    pub fn expire(&mut self, now: i64) -> Vec<Action> {
        let due: Vec<String> = self
            .calls
            .iter()
            .filter(|(_, s)| !s.done && s.ringing_since.is_some_and(|t| now - t >= RING_FALLBACK_SECS))
            .map(|(id, _)| id.clone())
            .collect();
        due.iter().filter_map(|id| self.replace(id, CallPushState::Ended)).collect()
    }

    /// A ring pushed and not yet replaced.
    pub fn ringing(&self) -> bool {
        self.calls.values().any(|s| s.ringing_since.is_some() && !s.done)
    }
}

fn pushers() -> &'static Mutex<HashMap<String, CallPushes>> {
    static P: OnceLock<Mutex<HashMap<String, CallPushes>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// Send `a` to `ext`'s phones (nothing when they have none).
fn send(ext: &str, a: &Action) {
    let Some((principal, device_usernames)) = (deps().devices)(ext) else { return };
    if device_usernames.is_empty() {
        return;
    }
    let caller = a.started_by.as_deref().map(super::people::name_of_member).unwrap_or_default();
    crate::push_routes::notify_call(CallPush {
        principal,
        device_usernames,
        call_id: a.call_id.clone(),
        channel_id: a.channel_id.clone(),
        caller,
        state: a.state,
    });
}

/// What one feed read means for the poller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// Read again from the cursor (this many pushes went).
    Again(usize),
    /// Sew refused this person (404, or 403 `not_provisioned`): stop.
    Stop(String),
}

fn error_code(body: &Value) -> &str {
    body.pointer("/error/code").or_else(|| body.get("code")).and_then(Value::as_str).unwrap_or("")
}

/// One read of `ext`'s feed from `cursor` (blocking); the pushes it makes
/// go out and `cursor` moves to the page's `next`.
pub fn poll_once(ext: &str, cursor: &mut u64) -> Result<Poll, String> {
    let waiting = pushers().lock().unwrap_or_else(|p| p.into_inner()).get(ext).is_some_and(CallPushes::ringing);
    let wait = if waiting { FEED_WAIT_RINGING_S } else { FEED_WAIT_S };
    let (status, body) = (deps().feed)(ext, *cursor, wait)?;
    match status {
        200..=299 => {}
        410 if error_code(&body) == "cursor_expired" => {
            let oldest = body.pointer("/error/details/oldest_seq").or_else(|| body.get("oldest_seq")).and_then(u64_of);
            *cursor = oldest.ok_or("410 cursor_expired without oldest_seq")?;
            return Ok(Poll::Again(0));
        }
        404 => return Ok(Poll::Stop(format!("404 {}", error_code(&body)))),
        403 if error_code(&body) == "not_provisioned" || body.pointer("/error/details/reason").and_then(Value::as_str) == Some("not_provisioned") => {
            return Ok(Poll::Stop("403 not_provisioned".into()));
        }
        s => return Err(format!("the feed answered {s} {}", error_code(&body))),
    }
    let (items, next) = parse_feed(&body);
    *cursor = next.ok_or("the feed's page had no next")?;
    let actions = pushers().lock().unwrap_or_else(|p| p.into_inner()).entry(ext.to_string()).or_default().on_feed(&items, now_secs());
    for a in &actions {
        send(ext, a);
    }
    Ok(Poll::Again(actions.len()))
}

/// The fallback for `ext` at `now`: with no live held session (`heard` is
/// false), a ring pushed long enough ago is replaced. Returns the pushes.
pub fn expire_rings(ext: &str, now: i64, heard: bool) -> usize {
    if heard {
        return 0;
    }
    let due = pushers().lock().unwrap_or_else(|p| p.into_inner()).get_mut(ext).map(|p| p.expire(now)).unwrap_or_default();
    for a in &due {
        send(ext, a);
    }
    due.len()
}

/// Does a live held session follow `ext` (so a ring's end is heard)?
fn heard_live(ext: &str) -> bool {
    super::follower::get(ext).is_some_and(|h| h.status() == "live")
}

/// The follower heard a `call_ring` for `ext`: a stopped ring K2 pushed is
/// replaced at once.
pub fn ring_seen(ext: &str, d: &Value) {
    if d["ringing"].as_bool().unwrap_or(true) {
        return;
    }
    let Some(call) = id_of(&d["call_id"]) else { return };
    let reason = d["reason"].as_str().unwrap_or("");
    let a = pushers().lock().unwrap_or_else(|p| p.into_inner()).get_mut(ext).and_then(|p| p.on_ring_stopped(&call, reason));
    if let Some(a) = a {
        send(ext, &a);
    }
}

/// Tests: forget every person's calls.
#[doc(hidden)]
pub fn reset_for_tests() {
    pushers().lock().unwrap_or_else(|p| p.into_inner()).clear();
}

// ── the live feed and devices ────────────────────────────────────────

/// The live feed read: mgmt, as the install (never a person's session),
/// with an HTTP timeout of `wait` + 10 s. Any status comes back with its
/// body; only a transport failure is an error.
pub fn live_feed(ext: &str, after: u64, wait: u64) -> Result<(u16, Value), String> {
    let t = k2_core::tool_hook::get_target(k2_core::tool_hook::CHAT_KIND)?.ok_or("chat isn't set up here")?;
    if !t.enabled {
        return Err("chat is off here".into());
    }
    let client = crate::sew_mgmt::MgmtClient::new(&t.socket, &t.token_file);
    let path = format!(
        "/mgmt/v1/people/{}/notifications?after={after}&limit={FEED_LIMIT}&wait={wait}",
        k2_core::tool_hook::sew_wire::pct_path(ext)
    );
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| format!("runtime: {e}"))?;
    let timeout = crate::sew_mgmt::long_poll_timeout(Duration::from_secs(wait));
    let r = rt.block_on(client.get_with_timeout(&path, timeout)).map_err(|e| format!("{path}: {e:?}"))?;
    let body = serde_json::from_str(&r.body_text()).unwrap_or(Value::Null);
    Ok((r.status, body))
}

/// The live device lookup: `ext`'s person key and the logins their phones
/// registered under (the owner token's `owner`, the owner's linked logins,
/// or the Connect login itself). A Voice & Chat only login's phone rings too:
/// calls are that role's core. Disabled logins get nothing.
pub fn live_devices(ext: &str) -> Option<(String, Vec<String>)> {
    let devices = k2_core::push_devices::list_devices().ok()?;
    let people = k2_core::notify_prefs::People::load().ok()?;
    devices_for(ext, &people, &devices)
}

/// [`live_devices`] from given facts (pure).
pub fn devices_for(ext: &str, people: &k2_core::notify_prefs::People, devices: &[k2_core::push_devices::PushDevice]) -> Option<(String, Vec<String>)> {
    let mut names: Vec<String> = Vec::new();
    let principal;
    if ext == "k2:owner" {
        principal = k2_core::notify_prefs::OWNER.to_string();
        for d in devices {
            let n = d.username.trim();
            let owner_token = n.eq_ignore_ascii_case(k2_core::notify_prefs::OWNER) && people.user(n).is_none();
            let linked = people.is_linked(n) && people.user(n).is_some_and(|u| !u.disabled);
            if owner_token || linked {
                names.push(d.username.clone());
            }
        }
    } else {
        let uuid = ext.strip_prefix("k2:connect:")?;
        let login = k2_core::connect_users::admin_space_facts().ok()?.into_iter().find(|(id, ..)| id == uuid).map(|(_, u, ..)| u)?;
        let user = people.user(&login).filter(|u| !u.disabled)?;
        if people.is_linked(&user.username) {
            return None; // the owner's linked login: its phones ring as the owner
        }
        principal = people.person_key(&user.username);
        names.extend(devices.iter().filter(|d| d.username.eq_ignore_ascii_case(&user.username)).map(|d| d.username.clone()));
    }
    names.sort();
    names.dedup();
    Some((principal, names))
}

// ── the pollers ──────────────────────────────────────────────────────

/// One poller per person: `ext` → its stop flag. A poller removes its own
/// entry when it exits, so a stopping poller still counts and a second one
/// never starts beside it (Sew keeps one waiter per person).
fn running() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static R: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// People Sew refused (404, `not_provisioned`), and when.
fn stopped() -> &'static Mutex<HashMap<String, std::time::Instant>> {
    static S: OnceLock<Mutex<HashMap<String, std::time::Instant>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn start(ext: &str) {
    if stopped().lock().unwrap_or_else(|p| p.into_inner()).get(ext).is_some_and(|at| at.elapsed() < STOPPED_FOR) {
        return;
    }
    let mut g = running().lock().unwrap_or_else(|p| p.into_inner());
    if g.contains_key(ext) {
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    g.insert(ext.to_string(), stop.clone());
    let who = ext.to_string();
    let mine = stop.clone();
    let spawned = std::thread::Builder::new().name("home-chat-call-push".into()).spawn(move || {
        let mut cursor: u64 = 0;
        let mut backoff = Duration::from_secs(1);
        while !stop.load(Ordering::SeqCst) {
            match poll_once(&who, &mut cursor) {
                Ok(Poll::Again(_)) => backoff = Duration::from_secs(1),
                Ok(Poll::Stop(why)) => {
                    k2_core::log_debug!("[home-chat] {who}: call feed stopped ({why})");
                    stopped().lock().unwrap_or_else(|p| p.into_inner()).insert(who.clone(), std::time::Instant::now());
                    break;
                }
                Err(e) => {
                    k2_core::log_debug!("[home-chat] {who}: call feed: {e}; retrying");
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
            expire_rings(&who, now_secs(), heard_live(&who));
        }
        let mut g = running().lock().unwrap_or_else(|p| p.into_inner());
        if g.get(&who).is_some_and(|s| Arc::ptr_eq(s, &mine)) {
            g.remove(&who);
        }
    });
    if let Err(e) = spawned {
        g.remove(ext);
        k2_core::log_debug!("[home-chat] {ext}: call feed didn't start: {e}");
    }
}

/// Ask every poller not in `keep` to stop (each removes itself on exit).
fn stop_all_but(keep: &std::collections::BTreeSet<String>) {
    for (ext, stop) in running().lock().unwrap_or_else(|p| p.into_inner()).iter() {
        if !keep.contains(ext) {
            stop.store(true, Ordering::SeqCst);
        }
    }
}

/// The box people who have a push device here.
fn wanted() -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut people = vec!["k2:owner".to_string()];
    if let Ok(rows) = k2_core::connect_users::admin_space_facts() {
        people.extend(rows.into_iter().filter(|(id, _, _, disabled)| !disabled && !id.is_empty()).map(|(id, ..)| format!("k2:connect:{id}")));
    }
    for ext in people {
        if (deps().devices)(&ext).is_some_and(|(_, d)| !d.is_empty()) {
            out.insert(ext);
        }
    }
    out
}

/// Start the supervisor (daemon boot). Idempotent. Idle while Home chat is
/// off or Sew isn't usable here.
pub fn spawn() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new().name("home-chat-call-push-sup".into()).spawn(|| loop {
        let want = if super::status().0 { wanted() } else { Default::default() };
        stop_all_but(&want);
        for ext in &want {
            start(ext);
        }
        std::thread::sleep(SUPERVISE_EVERY);
    });
    if let Err(e) = spawned {
        k2_core::log_debug!("[home-chat] call push supervisor didn't start: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ring(id: &str) -> FeedItem {
        FeedItem { kind: "call_ring".into(), call_id: id.into(), channel_id: Some("900".into()), started_by: Some("42".into()) }
    }

    fn missed(id: &str) -> FeedItem {
        FeedItem { kind: "call_missed".into(), call_id: id.into(), channel_id: Some("900".into()), started_by: None }
    }

    /// Each call: one ring push, then exactly one push that replaces it.
    #[test]
    fn a_ring_is_pushed_once_then_replaced_once() {
        let mut p = CallPushes::default();
        let a = p.on_feed(&[ring("1")], 100);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].state, CallPushState::Ringing);
        assert!(p.on_feed(&[ring("1")], 101).is_empty(), "the same ring again: nothing");
        let m = p.on_feed(&[missed("1")], 145);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].state, m[0].channel_id.as_str(), m[0].started_by.as_deref()), (CallPushState::Missed, "900", Some("42")));
        assert!(p.on_feed(&[missed("1")], 146).is_empty(), "replaced once");
        assert!(p.on_ring_stopped("1", "call_ended").is_none(), "already replaced");
        assert!(p.expire(1_000).is_empty(), "nothing left to expire");
    }

    /// The held session's ring end replaces the ring at once: accepted →
    /// answered elsewhere, timeout → missed, the rest → ended. A ring K2
    /// never pushed (held for Do Not Disturb) owes nothing.
    #[test]
    fn a_heard_ring_end_replaces_the_ring_at_once() {
        for (reason, want) in [
            ("accepted", CallPushState::Answered),
            ("timeout", CallPushState::Missed),
            ("declined", CallPushState::Ended),
            ("cancelled", CallPushState::Ended),
            ("call_ended", CallPushState::Ended),
        ] {
            let mut p = CallPushes::default();
            p.on_feed(&[ring("7")], 100);
            assert_eq!(p.on_ring_stopped("7", reason).map(|a| a.state), Some(want), "{reason}");
            assert!(p.on_feed(&[missed("7")], 150).is_empty(), "{reason}: never a second replacement");
            assert!(p.expire(1_000).is_empty(), "{reason}");
        }
        assert!(CallPushes::default().on_ring_stopped("9", "accepted").is_none(), "never pushed");
    }

    /// No end heard: the ring is replaced by "Call ended" once it is
    /// RING_FALLBACK_SECS old, and not before.
    #[test]
    fn an_unheard_ring_expires_into_call_ended() {
        let mut p = CallPushes::default();
        p.on_feed(&[ring("8")], 100);
        assert!(p.ringing());
        assert!(p.expire(100 + RING_FALLBACK_SECS - 1).is_empty(), "too soon");
        let e = p.expire(100 + RING_FALLBACK_SECS);
        assert_eq!(e.iter().map(|a| a.state).collect::<Vec<_>>(), vec![CallPushState::Ended]);
        assert!(!p.ringing());
        assert!(p.on_feed(&[missed("8")], 200).is_empty(), "replaced once");
    }

    /// Sew's page: `{items, next}`; only call kinds are kept, unknown kinds
    /// are ignored, `from_member_id` is the caller.
    #[test]
    fn the_feed_page_parses_items_and_next() {
        let body = json!({"items": [
            {"seq": 5, "kind": "call_ring", "at": "2026-11-14T12:00:00Z", "channel_id": "900", "from_member_id": "42", "app_id": null, "call_id": "77", "message_id": "1001"},
            {"seq": 6, "kind": "mention", "at": "2026-11-14T12:00:01Z", "channel_id": "901", "from_member_id": "43", "app_id": null},
            {"seq": 7, "kind": "something_new", "at": "2026-11-14T12:00:02Z", "channel_id": "900", "from_member_id": "42", "app_id": null},
            {"seq": 8, "kind": "call_missed", "at": "2026-11-14T12:00:45Z", "channel_id": "900", "from_member_id": "42", "app_id": null, "call_id": "77", "message_id": "1001"},
        ], "next": 8});
        let (items, next) = parse_feed(&body);
        assert_eq!(next, Some(8));
        assert_eq!(items, vec![
            FeedItem { kind: "call_ring".into(), call_id: "77".into(), channel_id: Some("900".into()), started_by: Some("42".into()) },
            FeedItem { kind: "call_missed".into(), call_id: "77".into(), channel_id: Some("900".into()), started_by: Some("42".into()) },
        ]);
        assert_eq!(parse_feed(&json!({"items": [], "next": 3})), (vec![], Some(3)));
        assert_eq!(parse_feed(&json!({"entries": []})).1, None, "not Sew's page");
    }

    /// The owner's ring reaches the owner token's own phones (and the
    /// owner's linked logins'), never another login's.
    #[test]
    fn the_owners_ring_reaches_only_the_owners_phones() {
        let dev = |u: &str| k2_core::push_devices::PushDevice {
            device_id: format!("d-{u}"),
            platform: "apns".into(),
            token: "t".into(),
            username: u.into(),
            created_at: 0,
            updated_at: 0,
            last_seen_at: 0,
            previews: true,
        };
        let people = k2_core::notify_prefs::People::default();
        let (who, names) = devices_for("k2:owner", &people, &[dev("owner"), dev("sam")]).expect("the owner");
        assert_eq!((who.as_str(), names), ("owner", vec!["owner".to_string()]));
        assert!(devices_for("k2:workspace:x", &people, &[dev("owner")]).is_none(), "agents have no phones here");
    }
}
