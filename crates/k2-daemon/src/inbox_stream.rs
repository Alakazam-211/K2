//! The phone inbox stream on `/cli/sessions/events` (inbox-stream-v1,
//! 0.46.7; `.k2/prds/contract-phone-inbox-stream-0462.md` §2).
//!
//! K2 Companion keeps every chat on its Home and its recent chats current
//! while the app is open: it holds ONE `/cli/sessions/events` socket and
//! sends
//! ```json
//! {"t":"inbox","conversationIds":["c-1"],"addrs":["sales","ops/reviewer"]}
//! {"t":"inbox","all":true}
//! {"t":"inbox","off":true}
//! ```
//! (each replaces the last; at most [`MAX_ENTRIES`] ids + addrs). The
//! socket then also carries, for that set:
//! - `inbox_subscribed` (the answer, and again whenever a subscribed
//!   address now answers at another conversation): `rev`, `instanceId`,
//!   `conversations`, `resolved` (addr → conversation), `unknown`, `max`;
//! - `thread_message` (`conversation_id`, `collection`, `seq`, `id`, `rev`,
//!   `doc`): the doc is exactly what the overlay socket sends this login
//!   (`overlay_ws::wire_json_for`), capped at 32 KiB (`truncated`,
//!   `bodyBytes`, then no doc);
//! - `thread_moved` / `thread_address`; the set follows a move in place;
//! - `inbox_reset`: the socket fell behind the overlay bus. The set is
//!   kept; the phone runs `POST /cli/thread/catchup` for it.
//!
//! **Who.** Only a login reaches this socket (the owner token or a Connect
//! session: `token_ok` at the dispatcher). A login reads every Thread
//! (`GET /cli/thread`'s rule), so the stream adds no access. An App pass
//! (`k2skn_`) and an agent passport are refused before the upgrade; App
//! guests use `/cli/thread/stream`, which keeps them to their own rooms.
//! The socket's 5 s re-auth closes a revoked login's stream with it.
//!
//! Frames come from a receiver on the overlay bus (1024 slots), never the
//! 256-slot session bus, so a lag costs an `inbox_reset`, not the socket.
//! `chatter`, `chatterlog` and `activity` frames are never forwarded.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::overlay_ws::{self, OverlayFrame};

/// Most `conversationIds` + `addrs` one subscribe may name.
pub const MAX_ENTRIES: usize = 200;

/// The frame kinds this module sends (not `SessionEvent` variants; the
/// shared registry lists them with class `inbox`).
pub const INBOX_KINDS: &[&str] =
    &["inbox_subscribed", "inbox_error", "thread_message", "thread_moved", "thread_address", "inbox_reset"];

/// What a subscribe message asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    /// Every Thread on the server.
    All,
    /// Stop (the socket stays open for its other events).
    Off,
    /// These conversations and addresses.
    Some { ids: Vec<String>, addrs: Vec<String> },
}

/// The socket's current set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InboxSet {
    pub all: bool,
    /// Subscribed conversation ids, in request order (moves swap in place).
    pub ids: Vec<String>,
    /// Each resolved addr → the conversation it answers at now.
    pub resolved: BTreeMap<String, String>,
    /// Addrs that did not resolve.
    pub unknown: Vec<String>,
}

impl InboxSet {
    /// Is `conversation` in the set?
    pub fn contains(&self, conversation: &str) -> bool {
        self.all || self.ids.iter().any(|c| c == conversation) || self.resolved.values().any(|c| c == conversation)
    }

    /// Every subscribed conversation, once, ids first.
    pub fn conversations(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for c in self.ids.iter().chain(self.resolved.values()) {
            if !out.contains(c) {
                out.push(c.clone());
            }
        }
        out
    }

    /// A Thread moved `from` → `to`: swapped in place, so the count never
    /// grows (if `to` is already in the set, `from` is just dropped).
    /// Returns whether the set held either key.
    pub fn apply_move(&mut self, from: &str, to: &str) -> bool {
        let touched = self.contains(from) || self.contains(to);
        let had_to = self.ids.iter().any(|c| c == to);
        if had_to {
            self.ids.retain(|c| c != from);
        } else {
            for c in self.ids.iter_mut().filter(|c| *c == from) {
                *c = to.to_string();
            }
        }
        for c in self.resolved.values_mut().filter(|c| *c == from) {
            *c = to.to_string();
        }
        touched
    }

    /// The addrs to re-resolve.
    pub fn addrs(&self) -> Vec<String> {
        self.resolved.keys().chain(self.unknown.iter()).cloned().collect()
    }
}

fn strings(v: &Value, key: &str) -> Result<Vec<String>, (&'static str, String)> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => {
            let mut out: Vec<String> = Vec::new();
            for x in a {
                match x.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    Some(s) => {
                        if !out.iter().any(|o| o == s) {
                            out.push(s.to_string());
                        }
                    }
                    None => return Err(("usage", format!("{key} holds non-empty strings"))),
                }
            }
            Ok(out)
        }
        Some(_) => Err(("usage", format!("{key} is an array of strings"))),
    }
}

/// A client text frame. `None` = not an inbox message (ignored, as every
/// other client text on this socket is).
pub fn parse_subscribe(text: &str) -> Option<Result<Want, (&'static str, String)>> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("t").and_then(Value::as_str) != Some("inbox") {
        return None;
    }
    if v.get("off") == Some(&Value::Bool(true)) {
        return Some(Ok(Want::Off));
    }
    if v.get("all") == Some(&Value::Bool(true)) {
        return Some(Ok(Want::All));
    }
    let ids = match strings(&v, "conversationIds") {
        Ok(x) => x,
        Err(e) => return Some(Err(e)),
    };
    let addrs = match strings(&v, "addrs") {
        Ok(x) => x,
        Err(e) => return Some(Err(e)),
    };
    let n = ids.len() + addrs.len();
    if n > MAX_ENTRIES {
        return Some(Err(("too_many", format!("at most {MAX_ENTRIES} conversationIds + addrs, got {n}"))));
    }
    if n == 0 {
        return Some(Err(("usage", "name conversationIds and/or addrs, or send all:true or off:true".into())));
    }
    Some(Ok(Want::Some { ids, addrs }))
}

/// The set for `want`, resolving each addr with `resolve` (the login
/// resolver `GET /cli/thread` uses). `None` for `off`.
pub fn build_set(want: &Want, resolve: &dyn Fn(&str) -> Option<String>) -> Option<InboxSet> {
    match want {
        Want::Off => None,
        Want::All => Some(InboxSet { all: true, ..Default::default() }),
        Want::Some { ids, addrs } => {
            let mut set = InboxSet { ids: ids.clone(), ..Default::default() };
            for a in addrs {
                match resolve(a) {
                    Some(c) => {
                        set.resolved.insert(a.clone(), c);
                    }
                    None => set.unknown.push(a.clone()),
                }
            }
            Some(set)
        }
    }
}

/// Re-resolve the set's addrs; `true` when any now answers elsewhere (or
/// a missing one now resolves, or a resolved one is gone).
pub fn reresolve(set: &mut InboxSet, resolve: &dyn Fn(&str) -> Option<String>) -> bool {
    let mut resolved = BTreeMap::new();
    let mut unknown = Vec::new();
    for a in set.addrs() {
        match resolve(&a) {
            Some(c) => {
                resolved.insert(a, c);
            }
            None => unknown.push(a),
        }
    }
    unknown.sort();
    let mut old_unknown = set.unknown.clone();
    old_unknown.sort();
    let changed = resolved != set.resolved || unknown != old_unknown;
    if changed {
        set.resolved = resolved;
        set.unknown = unknown;
    }
    changed
}

/// The `inbox_subscribed` answer.
pub fn subscribed_json(set: Option<&InboxSet>, rev: u64) -> String {
    let empty = InboxSet::default();
    let s = set.unwrap_or(&empty);
    json!({
        "kind": "inbox_subscribed",
        "rev": rev,
        "instanceId": crate::boot_status::instance_id(),
        "all": s.all,
        "conversations": s.conversations(),
        "resolved": s.resolved,
        "unknown": s.unknown,
        "max": MAX_ENTRIES,
    })
    .to_string()
}

/// An `inbox_error` (`usage` | `too_many`).
pub fn error_json(code: &str, hint: &str) -> String {
    let mut v = json!({ "kind": "inbox_error", "code": code, "hint": hint });
    if code == "too_many" {
        v["max"] = json!(MAX_ENTRIES);
    }
    v.to_string()
}

/// `inbox_reset`.
pub fn reset_json(reason: &str) -> String {
    json!({ "kind": "inbox_reset", "reason": reason }).to_string()
}

/// What an overlay frame means for this set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    /// A Thread item (post, card change) in the set: send its doc.
    Thread { conversation: String },
    /// A move touching the set (one frame per move).
    Moved { from: String, to: String },
    /// An address change of a conversation in the set.
    Address { conversation: String, address: Value, previous: Value },
}

/// Classify one overlay frame. Only `thread`, `moved` and `address` frames
/// can hit; `chatter`, `chatterlog` and `activity` never do.
pub fn classify(frame: &OverlayFrame, set: &InboxSet) -> Option<Hit> {
    let conv = frame.conversation_id.as_deref()?;
    match frame.collection.as_str() {
        "thread" if frame.doc.is_some() && set.contains(conv) => Some(Hit::Thread { conversation: conv.to_string() }),
        overlay_ws::MOVED_COLLECTION => {
            // Published under both keys; only the `from` copy counts.
            let from = frame.activity.as_ref().and_then(|a| a.get("from")).and_then(Value::as_str)?;
            let to = frame.id.as_str();
            (from == conv && (set.contains(from) || set.contains(to)))
                .then(|| Hit::Moved { from: from.to_string(), to: to.to_string() })
        }
        overlay_ws::ADDRESS_COLLECTION if set.contains(conv) => {
            let a = frame.activity.clone().unwrap_or(Value::Null);
            Some(Hit::Address { conversation: conv.to_string(), address: a["address"].clone(), previous: a["previous"].clone() })
        }
        _ => None,
    }
}

/// One `thread_message`, capped at 32 KiB like the room stream's (body
/// first, then the doc; the keys always stay).
pub fn thread_message(conversation: &str, seq: i64, id: &str, rev: u64, doc: Value) -> String {
    crate::room_stream_ws::cap_frame(json!({
        "kind": "thread_message",
        "conversation_id": conversation,
        "collection": "thread",
        "seq": seq,
        "id": id,
        "rev": rev,
        "doc": doc,
    }))
}

/// `thread_moved`.
pub fn moved_message(from: &str, to: &str, seq: i64, rev: u64) -> String {
    json!({ "kind": "thread_moved", "from": from, "to": to, "seq": seq, "rev": rev }).to_string()
}

/// `thread_address`.
pub fn address_message(conversation: &str, address: &Value, previous: &Value, rev: u64) -> String {
    json!({ "kind": "thread_address", "conversation_id": conversation, "address": address, "previous": previous, "rev": rev })
        .to_string()
}

/// The doc a login's overlay socket would get for `frame`.
pub(crate) fn doc_for_login(frame: &OverlayFrame, seen: &crate::permission::view::Seen) -> Value {
    crate::room_stream_ws::doc_for(frame, None, seen)
}

/// The login resolver for one addr (`GET /cli/thread?addr=`'s, for a login).
pub fn resolve_for_login(addr: &str) -> Option<String> {
    crate::overlay_routes::resolve_login_addr(addr)
}

#[cfg(test)]
mod tests;
