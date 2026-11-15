//! Phone ringing for DM calls (prd-home-people-channels-v1 §11a.3, slice
//! S14; HP-41).
//!
//! Sew pushes nothing to Apple or Google. It writes `call_ring` (while a
//! loud ring is pending) and `call_missed` to the person's feed, `GET
//! /mgmt/v1/people/{external_id}/notifications`, a long-poll with ids only
//! that Sew holds during Do Not Disturb. K2 does the push:
//!
//! - **Who.** Every box person (`k2:owner`, `k2:connect:<uuid>`) with at
//!   least one registered push device, while Home chat is usable here. One
//!   poller thread per person; the supervisor starts and stops them.
//! - **What.** Push kind `call` ([`k2_core::push::PUSH_KIND_CALL`]): the ring
//!   is an alert with sound and the caller's name, `collapseId` `call:<call
//!   id>`. Exactly one push replaces it: "Missed call" when Sew's feed says
//!   `call_missed`, or "Call ended" when the ring stopped any other way
//!   (answered on another device, declined, the caller cancelled, the call
//!   ended), which the person's follower hears as `call_ring {ringing:
//!   false}` ([`ring_seen`]). A cancelled ring is never a missed call.
//! - **Budget.** Its own class ([`k2_core::push::budget::BudgetClass::Call`]),
//!   so rings never wait behind Thread or badge pushes.
//! - Not VoIP or CallKit in v1; the Companion handles kind `call`.
//!
//! [`CallPushes`] is the pure state machine (each call: at most one ring
//! push and one replacement); [`poll_once`] is one feed read.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;

use k2_core::push::{CallPush, CallPushState};

use super::deps;

/// How long Sew holds an empty feed read, seconds.
pub const FEED_WAIT_S: u64 = 25;
/// The supervisor's look at who has a push device.
const SUPERVISE_EVERY: Duration = Duration::from_secs(30);
/// Calls remembered per person (oldest dropped first).
const CALLS_KEPT: usize = 256;

/// One feed entry K2 acts on (ids only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedItem {
    /// `call_ring` or `call_missed` (anything else is ignored).
    pub kind: String,
    pub call_id: String,
    pub channel_id: Option<String>,
    pub started_by: Option<String>,
}

fn id_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A feed answer: its call entries and the next cursor (when it names one).
pub fn parse_feed(body: &Value) -> (Vec<FeedItem>, Option<String>) {
    let list = if body.is_array() {
        body
    } else {
        ["entries", "items", "notifications"].iter().map(|k| &body[*k]).find(|v| v.is_array()).unwrap_or(&Value::Null)
    };
    let mut out = Vec::new();
    for e in list.as_array().into_iter().flatten() {
        let kind = ["kind", "type", "reason"].iter().find_map(|k| e[*k].as_str()).unwrap_or("");
        if kind != "call_ring" && kind != "call_missed" {
            continue;
        }
        let call = &e["call"];
        let Some(call_id) = id_of(&e["call_id"]).or_else(|| id_of(&call["id"])).or_else(|| id_of(&e["entity_id"])) else { continue };
        out.push(FeedItem {
            kind: kind.to_string(),
            call_id,
            channel_id: id_of(&e["channel_id"]).or_else(|| id_of(&call["channel_id"])),
            started_by: id_of(&e["started_by"]).or_else(|| id_of(&call["started_by"])),
        });
    }
    let cursor = if body.is_object() { ["next_after", "next", "cursor"].iter().find_map(|k| id_of(&body[*k])) } else { None };
    (out, cursor)
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
    /// The ring push went and nothing replaced it yet.
    ringing: bool,
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

    /// Feed entries in order → the pushes they make.
    pub fn on_feed(&mut self, items: &[FeedItem]) -> Vec<Action> {
        let mut out = Vec::new();
        for it in items {
            let known = self.calls.get(&it.call_id).cloned();
            match (it.kind.as_str(), known) {
                ("call_ring", None) => {
                    let channel_id = it.channel_id.clone().unwrap_or_default();
                    self.remember(&it.call_id, Seen { channel_id: channel_id.clone(), started_by: it.started_by.clone(), ringing: true, done: false });
                    out.push(Action { call_id: it.call_id.clone(), channel_id, started_by: it.started_by.clone(), state: CallPushState::Ringing });
                }
                ("call_missed", known) if !known.as_ref().is_some_and(|k| k.done) => {
                    let channel_id = known.as_ref().map(|k| k.channel_id.clone()).filter(|c| !c.is_empty()).or_else(|| it.channel_id.clone()).unwrap_or_default();
                    let started_by = known.as_ref().and_then(|k| k.started_by.clone()).or_else(|| it.started_by.clone());
                    self.remember(&it.call_id, Seen { channel_id: channel_id.clone(), started_by: started_by.clone(), ringing: false, done: true });
                    out.push(Action { call_id: it.call_id.clone(), channel_id, started_by, state: CallPushState::Missed });
                }
                _ => {}
            }
        }
        out
    }

    /// The person's follower heard `call_ring {ringing: false, reason}`. A
    /// ring K2 pushed that stopped any way but a timeout is replaced by
    /// "Call ended" (a timeout's replacement is the feed's `call_missed`).
    pub fn on_ring_stopped(&mut self, call_id: &str, reason: &str) -> Option<Action> {
        let s = self.calls.get_mut(call_id)?;
        if !s.ringing || s.done || reason == "timeout" {
            return None;
        }
        s.ringing = false;
        s.done = true;
        Some(Action { call_id: call_id.to_string(), channel_id: s.channel_id.clone(), started_by: s.started_by.clone(), state: CallPushState::Ended })
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

/// One read of `ext`'s feed (blocking): the pushes it made go out. `cursor`
/// moves to the feed's next one.
pub fn poll_once(ext: &str, cursor: &mut Option<String>) -> Result<usize, String> {
    let body = (deps().feed)(ext, cursor.as_deref())?;
    let (items, next) = parse_feed(&body);
    if next.is_some() {
        *cursor = next;
    }
    let actions = pushers().lock().unwrap_or_else(|p| p.into_inner()).entry(ext.to_string()).or_default().on_feed(&items);
    for a in &actions {
        send(ext, a);
    }
    Ok(actions.len())
}

/// The follower heard a `call_ring` for `ext`: a stopped ring may owe its
/// phones a replacing push.
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

/// The live feed read: mgmt, as the install (never a person's session).
pub fn live_feed(ext: &str, cursor: Option<&str>) -> Result<Value, String> {
    let t = k2_core::tool_hook::get_target(k2_core::tool_hook::CHAT_KIND)?.ok_or("chat isn't set up here")?;
    if !t.enabled {
        return Err("chat is off here".into());
    }
    let client = crate::sew_mgmt::MgmtClient::new(&t.socket, &t.token_file);
    let mut path = format!("/mgmt/v1/people/{}/notifications?wait={FEED_WAIT_S}", k2_core::tool_hook::sew_wire::pct_path(ext));
    if let Some(c) = cursor {
        path.push_str(&format!("&after={}", k2_core::tool_hook::sew_wire::pct_path(c)));
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| format!("runtime: {e}"))?;
    let timeout = crate::sew_mgmt::long_poll_timeout(Duration::from_secs(FEED_WAIT_S));
    let r = rt.block_on(client.get_with_timeout(&path, timeout)).map_err(|e| format!("{path}: {e:?}"))?;
    if !r.is_success() {
        return Err(format!("{path}: {}", r.error_line()));
    }
    serde_json::from_str(&r.body_text()).map_err(|e| format!("{path}: unexpected body ({e})"))
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

fn running() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static R: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn start(ext: &str) {
    let mut g = running().lock().unwrap_or_else(|p| p.into_inner());
    if g.contains_key(ext) {
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    g.insert(ext.to_string(), stop.clone());
    let who = ext.to_string();
    let spawned = std::thread::Builder::new().name("home-chat-call-push".into()).spawn(move || {
        let mut cursor: Option<String> = None;
        let mut backoff = Duration::from_secs(1);
        while !stop.load(Ordering::SeqCst) {
            match poll_once(&who, &mut cursor) {
                Ok(_) => backoff = Duration::from_secs(1),
                Err(e) => {
                    k2_core::log_debug!("[home-chat] {who}: call feed: {e}; retrying");
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        }
    });
    if let Err(e) = spawned {
        g.remove(ext);
        k2_core::log_debug!("[home-chat] {ext}: call feed didn't start: {e}");
    }
}

fn stop_all_but(keep: &std::collections::BTreeSet<String>) {
    let mut g = running().lock().unwrap_or_else(|p| p.into_inner());
    g.retain(|ext, stop| {
        let keep_it = keep.contains(ext);
        if !keep_it {
            stop.store(true, Ordering::SeqCst);
        }
        keep_it
    });
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
        let a = p.on_feed(&[ring("1")]);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].state, CallPushState::Ringing);
        assert!(p.on_feed(&[ring("1")]).is_empty(), "the same ring again: nothing");
        let m = p.on_feed(&[missed("1")]);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].state, m[0].channel_id.as_str(), m[0].started_by.as_deref()), (CallPushState::Missed, "900", Some("42")));
        assert!(p.on_feed(&[missed("1")]).is_empty(), "replaced once");
        assert!(p.on_ring_stopped("1", "call_ended").is_none(), "already replaced");
    }

    /// A ring stopped by an answer elsewhere, a decline, the caller's cancel
    /// or the end of the call: "Call ended". A timeout waits for the feed's
    /// `call_missed`. A ring K2 never pushed (held for Do Not Disturb) owes
    /// nothing.
    #[test]
    fn a_stopped_ring_is_replaced_by_ended_except_a_timeout() {
        for reason in ["accepted", "declined", "cancelled", "call_ended"] {
            let mut p = CallPushes::default();
            p.on_feed(&[ring("7")]);
            let a = p.on_ring_stopped("7", reason).unwrap_or_else(|| panic!("{reason}"));
            assert_eq!(a.state, CallPushState::Ended, "{reason}");
            assert!(p.on_feed(&[missed("7")]).is_empty(), "{reason}: never a second replacement");
        }
        let mut p = CallPushes::default();
        p.on_feed(&[ring("8")]);
        assert!(p.on_ring_stopped("8", "timeout").is_none());
        assert_eq!(p.on_feed(&[missed("8")])[0].state, CallPushState::Missed);
        assert!(CallPushes::default().on_ring_stopped("9", "accepted").is_none(), "never pushed");
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

    #[test]
    fn the_feed_parses_entries_and_the_cursor() {
        let body = json!({"entries": [
            {"seq": 5, "kind": "call_ring", "call_id": "77", "channel_id": "900", "started_by": "42"},
            {"seq": 6, "kind": "mention", "entity_id": "3"},
            {"seq": 7, "type": "call_missed", "call": {"id": 77, "channel_id": 900}},
        ], "next_after": 7});
        let (items, cursor) = parse_feed(&body);
        assert_eq!(cursor.as_deref(), Some("7"));
        assert_eq!(items, vec![
            FeedItem { kind: "call_ring".into(), call_id: "77".into(), channel_id: Some("900".into()), started_by: Some("42".into()) },
            FeedItem { kind: "call_missed".into(), call_id: "77".into(), channel_id: Some("900".into()), started_by: None },
        ]);
        assert_eq!(parse_feed(&json!({"items": []})).0, vec![]);
    }
}
