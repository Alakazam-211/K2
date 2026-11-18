//! One person's chat state as the daemon follows it (§14.2, §15.1): per
//! conversation, unread, the DM's `mention_count`, Sew's `unread_count`
//! (when Sew has it) and the last message. Pure: the follower feeds it
//! READY, gateway events and catch-up answers; the routes and the events
//! socket read it. No chat text is kept (§16).
//!
//! **DM calls (§11a.3, §11a.4).** The DM calls the person can see now
//! ([`CallSeen`]: READY `calls`, `call_ring`, `call_updated`), so a fresh
//! client socket's `hello` shows a ring that started before it opened, and a
//! join knows whether it answers a ring. A call's record message
//! (`Message.call`, Sew's CallSummary) makes the DM unread from the call's
//! start but counts nothing then; once it ends as a missed call for this
//! person it adds one to the DM's `mention_count` (Sew's own count does the
//! same), once per call. A decline adds nothing, and a cancelled ring is
//! never a missed call.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

/// One conversation (a DM or a channel the person is in).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Conv {
    pub channel_id: String,
    pub dm: bool,
    pub space_id: Option<String>,
    pub unread: bool,
    pub mention_count: u32,
    pub unread_count: Option<u32>,
    pub last_read_id: Option<String>,
    pub last_message_id: Option<String>,
    /// When the last message was posted (Sew's RFC 3339 text).
    pub last_at: Option<String>,
    /// A channel is invite-only (Sew `private`) or open (`public`); `None`
    /// for a DM, or while Sew hasn't said.
    pub invite_only: Option<bool>,
    /// `dm` came from something that says (a channel list, a message, K2's
    /// own DM ledger), not from the default. A read state can arrive first
    /// (Sew moves the poster's cursor before the message event) and says
    /// nothing of the kind: the first event that does sets it.
    pub kind_known: bool,
}

/// Sew's channel `visibility` as invite-only: `private` yes, `public` no,
/// anything else unknown.
pub fn invite_only_of(visibility: &Value) -> Option<bool> {
    match visibility.as_str() {
        Some("private") => Some(true),
        Some("public") => Some(false),
        _ => None,
    }
}

/// One DM call the person can see (§11a.3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallSeen {
    pub call_id: String,
    pub channel_id: String,
    pub started_by: Option<String>,
    /// It rings for this person now.
    pub ringing_me: bool,
    /// A silent ring (busy in another call, DND, a muted DM).
    pub silent: bool,
    pub expires_at: Option<String>,
    /// Sew's last `Call` object for it (`call_updated`, READY).
    pub call: Value,
}

impl CallSeen {
    /// What `hello` and the routes show of it.
    pub fn view(&self) -> Value {
        json!({
            "callId": self.call_id, "conv": self.channel_id, "startedBy": self.started_by,
            "ringing": self.ringing_me, "silent": self.silent, "expiresAt": self.expires_at, "call": self.call,
        })
    }
}

/// Does this call record (Sew's CallSummary) end as a missed call for
/// member `me`? Not when cancelled (Sew 0.10.0), not when they declined.
pub fn missed_for(summary: &Value, me: &str) -> bool {
    let has = |k: &str| summary[k].as_array().is_some_and(|a| a.iter().any(|x| id_of(x).is_some_and(|x| x == me)));
    !me.is_empty() && summary["end_reason"].as_str() != Some("cancelled") && has("missed_ids") && !has("declined_ids")
}

/// One more on a DM's count (a missed call).
fn bump(c: &mut Conv, unread_count_flag: bool) {
    c.mention_count = c.mention_count.saturating_add(1);
    if unread_count_flag {
        c.unread_count = Some(c.unread_count.unwrap_or(0).saturating_add(1).min(k2_core::home_chat::BADGE_CAP));
    }
}

/// The person's whole followed state.
#[derive(Debug, Clone, Default)]
pub struct PersonState {
    /// The person's Sew member id (from READY or the session).
    pub me: String,
    pub convs: HashMap<String, Conv>,
    /// Sew's member change-log head (READY `member_seq`, catch-up `next_since`).
    pub member_seq: u64,
    /// The gateway session K2 may RESUME, and the last dispatch seq.
    pub gateway_session: Option<String>,
    pub seq: u64,
    /// Conversations a client of this person has open (typing is
    /// subscribed for these only).
    pub viewing: HashSet<String>,
    /// DM calls the person can see now, by Sew call id.
    pub calls: HashMap<String, CallSeen>,
    /// Call records already counted as a missed call (each once).
    pub missed_counted: HashSet<String>,
}

/// A recording's transcript line (Sew S35 v2 §15.2): a message that carries
/// `transcript` (Sew's TranscriptTurn, on every turn it posts, as a reply
/// under the recording card). Kept off the timeline and out of every count
/// (HR-10). A thread reply without `transcript` (a person replying in a Sew
/// thread from Sew's own app) is an ordinary message: shown and counted
/// (Home has no thread view yet, so it shows as a normal message).
pub fn is_transcript_line(m: &Value) -> bool {
    m["transcript"].is_object()
}

fn id_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn u32_of(v: &Value) -> Option<u32> {
    v.as_u64().map(|n| n.min(u64::from(u32::MAX)) as u32)
}

fn is_dm_kind(k: &str) -> bool {
    matches!(k, "dm" | "group_dm")
}

impl PersonState {
    /// One read state (READY, `read_state_updated`, `me/read_states`, `me/sync`).
    pub fn apply_read_state(&mut self, rs: &Value) {
        let Some(id) = id_of(&rs["channel_id"]) else { return };
        let c = self.convs.entry(id.clone()).or_insert_with(|| Conv { channel_id: id, ..Default::default() });
        c.unread = rs["unread"].as_bool().unwrap_or(false);
        c.mention_count = u32_of(&rs["mention_count"]).unwrap_or(0);
        c.unread_count = u32_of(&rs["unread_count"]);
        if let Some(r) = id_of(&rs["last_read_id"]) {
            c.last_read_id = Some(r);
        }
        if let Some(m) = id_of(&rs["last_message_id"]) {
            c.last_message_id = Some(m);
        }
    }

    /// Conversations that read as a channel with no Space: a row a read
    /// state made before anything said its kind (or, before 0.46.13, a DM
    /// stored as a channel that way). The summary asks K2's DM ledger, then
    /// Sew, what each is.
    pub fn kind_unsure(&self, convs: &[String]) -> Vec<String> {
        let mut v: Vec<String> = self
            .convs
            .values()
            .filter(|c| convs.is_empty() || convs.contains(&c.channel_id))
            .filter(|c| !c.dm && (!c.kind_known || c.space_id.is_none()))
            .map(|c| c.channel_id.clone())
            .collect();
        v.sort();
        v
    }

    /// K2's own DM ledger keeps `conv` as a DM: it is one, whichever Sew
    /// event made the row first. True when that changed the row.
    pub fn note_dm(&mut self, conv: &str) -> bool {
        let Some(c) = self.convs.get_mut(conv) else { return false };
        let changed = !c.dm;
        c.dm = true;
        c.kind_known = true;
        c.invite_only = None;
        changed
    }

    /// Conversations no event has said the kind of yet.
    pub fn kind_unknown(&self, convs: &[String]) -> Vec<String> {
        let mut v: Vec<String> = self
            .convs
            .values()
            .filter(|c| (convs.is_empty() || convs.contains(&c.channel_id)) && !c.kind_known)
            .map(|c| c.channel_id.clone())
            .collect();
        v.sort();
        v
    }

    /// Sew's own `Channel` for one conversation (`GET /api/v1/channels/{id}`).
    /// Whatever it says, the row's kind is settled (no second read).
    pub fn apply_channel(&mut self, conv: &str, ch: &Value) {
        self.apply_channels(&Value::Array(vec![ch.clone()]));
        if let Some(c) = self.convs.get_mut(conv) {
            c.kind_known = true;
        }
    }

    /// The channel list of READY or `me/sync` (kinds, Spaces, heads).
    fn apply_channels(&mut self, channels: &Value) {
        for ch in channels.as_array().into_iter().flatten() {
            let Some(id) = id_of(&ch["id"]) else { continue };
            let kind = ch["kind"].as_str().or_else(|| ch["type"].as_str()).unwrap_or("text");
            if !(is_dm_kind(kind) || kind == "text") {
                continue; // voice rooms are not conversations
            }
            let c = self.convs.entry(id.clone()).or_insert_with(|| Conv { channel_id: id, ..Default::default() });
            c.dm = is_dm_kind(kind);
            c.kind_known = true;
            c.space_id = id_of(&ch["space_id"]);
            if !c.dm {
                c.invite_only = invite_only_of(&ch["visibility"]).or(c.invite_only);
            }
            if let Some(m) = id_of(&ch["last_message_id"]) {
                c.last_message_id = Some(m);
            }
        }
    }

    /// READY: the whole state again (a fresh gateway session).
    pub fn apply_ready(&mut self, d: &Value) {
        let viewing = std::mem::take(&mut self.viewing);
        // A missed call counted before stays counted (Sew's read states in
        // READY already hold it).
        let missed_counted = std::mem::take(&mut self.missed_counted);
        *self = PersonState { viewing, missed_counted, ..Default::default() };
        self.me = id_of(&d["member"]["id"]).unwrap_or_default();
        self.member_seq = d["member_seq"].as_u64().unwrap_or(0);
        self.gateway_session = d["session_id"].as_str().map(str::to_string);
        self.apply_channels(&d["channels"]);
        for rs in d["read_states"].as_array().into_iter().flatten() {
            self.apply_read_state(rs);
        }
        for call in d["calls"].as_array().into_iter().flatten() {
            self.apply_call(call);
        }
    }

    /// One `Call` object (READY `calls`, `call_updated`): kept while live,
    /// dropped once ended. Whether it rings for this person comes from its
    /// `rings` (this member `ringing` or `busy`) or `my_ring`.
    fn apply_call(&mut self, call: &Value) {
        let Some(id) = id_of(&call["id"]) else { return };
        if call["state"].as_str() == Some("ended") {
            self.calls.remove(&id);
            return;
        }
        let me = self.me.clone();
        let my_ring = call["rings"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| id_of(&r["member_id"]).is_some_and(|m| m == me))
            .map(|r| (r["state"].as_str() == Some("ringing") || r["state"].as_str() == Some("busy"), r["state"].as_str() == Some("busy"), r["expires_at"].as_str().map(str::to_string)));
        let c = self.calls.entry(id.clone()).or_insert_with(|| CallSeen { call_id: id, ..Default::default() });
        c.channel_id = id_of(&call["channel_id"]).unwrap_or_else(|| c.channel_id.clone());
        c.started_by = id_of(&call["started_by"]).or(c.started_by.take());
        c.call = call.clone();
        match my_ring {
            Some((ringing, busy, expires)) => {
                c.ringing_me = ringing;
                c.silent = busy || call["my_ring"]["silent"].as_bool().unwrap_or(false);
                c.expires_at = expires.or(c.expires_at.take());
            }
            None if call["my_ring"].is_object() => {
                c.ringing_me = true;
                c.silent = call["my_ring"]["silent"].as_bool().unwrap_or(false);
            }
            None => c.ringing_me = false,
        }
    }

    /// `call_ring {call_id, channel_id, started_by, ringing, silent, reason?,
    /// expires_at}`: it rings for this person, or stopped for any reason
    /// (accepted, declined, timeout, call_ended, cancelled).
    fn apply_call_ring(&mut self, d: &Value) {
        let Some(id) = id_of(&d["call_id"]) else { return };
        let ringing = d["ringing"].as_bool().unwrap_or(false);
        if !ringing && !self.calls.contains_key(&id) {
            return;
        }
        let c = self.calls.entry(id.clone()).or_insert_with(|| CallSeen { call_id: id.clone(), ..Default::default() });
        c.channel_id = id_of(&d["channel_id"]).unwrap_or_else(|| c.channel_id.clone());
        c.started_by = id_of(&d["started_by"]).or(c.started_by.take());
        c.ringing_me = ringing;
        c.silent = ringing && d["silent"].as_bool().unwrap_or(false);
        c.expires_at = if ringing { d["expires_at"].as_str().map(str::to_string) } else { None };
        // A ring that stopped because the call is over (and K2 never saw
        // the call itself) leaves nothing to show.
        let over = matches!(d["reason"].as_str(), Some("timeout" | "declined" | "call_ended" | "cancelled"));
        if !ringing && over && c.call.is_null() {
            self.calls.remove(&id);
        }
    }

    /// A DM call event (`call_ring`, `call_updated`): the state moves, and the
    /// frame for the person's clients is `{type, conv, d}` as Sew sent `d`.
    pub fn apply_call_event(&mut self, t: &str, d: &Value) -> Vec<Value> {
        let conv = match t {
            "call_ring" => {
                self.apply_call_ring(d);
                id_of(&d["channel_id"])
            }
            "call_updated" => {
                let call = if d.get("call").is_some() { &d["call"] } else { d };
                self.apply_call(call);
                id_of(&call["channel_id"])
            }
            _ => return Vec::new(),
        };
        vec![json!({"type": t, "conv": conv, "d": d})]
    }

    /// The call ringing for this person in `conv`, if one does.
    pub fn incoming_ring(&self, conv: &str) -> Option<&CallSeen> {
        self.calls.values().find(|c| c.channel_id == conv && c.ringing_me)
    }

    /// Every DM call the person can see now (the `hello` frame's `calls`).
    pub fn calls_view(&self) -> Vec<Value> {
        let mut v: Vec<&CallSeen> = self.calls.values().collect();
        v.sort_by(|a, b| a.call_id.cmp(&b.call_id));
        v.into_iter().map(CallSeen::view).collect()
    }

    /// `GET /api/v1/me/sync` after `410 resync_required`: every read state
    /// rebuilt (the gateway session is kept).
    pub fn apply_sync(&mut self, d: &Value) {
        let keep = (self.me.clone(), self.gateway_session.clone(), self.seq, std::mem::take(&mut self.viewing));
        // The calls and the counted missed calls aren't in `me/sync`: kept.
        self.convs.clear();
        self.me = id_of(&d["member_id"]).unwrap_or(keep.0);
        self.member_seq = d["member_seq"].as_u64().unwrap_or(0);
        self.gateway_session = keep.1;
        self.seq = keep.2;
        self.viewing = keep.3;
        self.apply_channels(&d["channels"]);
        for rs in d["read_states"].as_array().into_iter().flatten() {
            self.apply_read_state(rs);
        }
    }

    /// One gateway dispatch (op 0, event `t`, body `d`). Returns the frames
    /// for the person's client sockets (`summary_changed` when a badge may
    /// have moved).
    pub fn apply_event(&mut self, t: &str, d: &Value, unread_count_flag: bool) -> Vec<Value> {
        let mut out = Vec::new();
        match t {
            "message_created" => {
                let m = if d.get("message").is_some() { &d["message"] } else { d };
                let Some(cid) = id_of(&m["channel_id"]) else { return out };
                if is_transcript_line(m) {
                    // A recording's transcript line (prd-home-call-recording-v1
                    // HR-10): it carries `transcript`. It reaches the
                    // person's windows (Show transcript) but never moves the
                    // conversation's unread, its count or its last message
                    // (the row's preview never shows a transcript line).
                    out.push(json!({"type": "message", "conv": cid, "message": m}));
                    return out;
                }
                let mine = id_of(&m["author_id"]).is_some_and(|a| a == self.me);
                let c = self.convs.entry(cid.clone()).or_insert_with(|| Conv { channel_id: cid.clone(), ..Default::default() });
                if !c.kind_known {
                    // A message names its Space; a DM has none. A read
                    // state that came first made the row with no kind.
                    c.dm = m["space_id"].is_null() && d.get("space_id").is_none_or(Value::is_null);
                    c.space_id = id_of(&m["space_id"]).or_else(|| d.get("space_id").and_then(id_of));
                    c.kind_known = true;
                }
                c.last_message_id = id_of(&m["id"]).or(c.last_message_id.take());
                c.last_at = m["created_at"].as_str().map(str::to_string).or(c.last_at.take());
                if m["call"].is_object() && !mine {
                    // A call's record: unread from the call's start, counted
                    // only once it ends as missed (message_updated).
                    c.unread = true;
                    let missed = id_of(&m["call"]["call_id"]).filter(|_| c.dm && missed_for(&m["call"], &self.me));
                    if let Some(call) = missed {
                        if self.missed_counted.insert(call) {
                            bump(c, unread_count_flag);
                        }
                    }
                } else if m["recording"].is_object() && !mine {
                    // A recording's card: unread, as a call's record is, but
                    // never counted (the call itself was the news).
                    c.unread = true;
                } else if mine {
                    // Posting reads the channel (Sew moves the cursor).
                    c.unread = false;
                    c.mention_count = 0;
                    c.unread_count = c.unread_count.map(|_| 0);
                    c.last_read_id = c.last_message_id.clone();
                } else {
                    c.unread = true;
                    if c.dm {
                        // Sew counts every DM message in mention_count (HP-26).
                        c.mention_count = c.mention_count.saturating_add(1);
                    }
                    if unread_count_flag {
                        c.unread_count = Some(c.unread_count.unwrap_or(0).saturating_add(1).min(k2_core::home_chat::BADGE_CAP));
                    }
                }
                out.push(json!({"type": "message", "conv": cid, "message": m}));
                out.push(json!({"type": "summary_changed", "conv": cid}));
            }
            "message_updated" => {
                let m = if d.get("message").is_some() { &d["message"] } else { d };
                let Some(cid) = id_of(&m["channel_id"]) else { return out };
                // A call's record ended as a missed call for this person: one
                // more on the DM's count (§11a.4), once per call.
                if m["call"].is_object() && missed_for(&m["call"], &self.me) {
                    if let (Some(call), Some(c)) = (id_of(&m["call"]["call_id"]), self.convs.get_mut(&cid)) {
                        if c.dm && self.missed_counted.insert(call) {
                            c.unread = true;
                            bump(c, unread_count_flag);
                            out.push(json!({"type": "summary_changed", "conv": cid}));
                        }
                    }
                }
                out.insert(0, json!({"type": "message_updated", "conv": cid, "message": m}));
            }
            "message_deleted" => {
                let Some(cid) = id_of(&d["channel_id"]) else { return out };
                if let (Some(c), Some(last)) = (self.convs.get_mut(&cid), id_of(&d["channel_last_message_id"])) {
                    c.last_message_id = Some(last);
                }
                out.push(json!({"type": "deleted", "conv": cid, "id": d["message_id"]}));
            }
            "read_state_updated" => {
                self.apply_read_state(d);
                if let Some(cid) = id_of(&d["channel_id"]) {
                    out.push(json!({"type": "read", "conv": cid}));
                    out.push(json!({"type": "summary_changed", "conv": cid}));
                }
            }
            "reaction_added" | "reaction_removed" => {
                let me = id_of(&d["member_id"]).is_some_and(|m| m == self.me);
                out.push(json!({
                    "type": "reaction", "on": t == "reaction_added", "conv": d["channel_id"], "msg": d["message_id"],
                    "emoji": d["emoji"], "count": d["count"], "member": d["member_id"], "mine": me,
                }));
            }
            "channel_member_removed" => {
                let Some(cid) = id_of(&d["channel_id"]) else { return out };
                if id_of(&d["member_id"]).is_some_and(|m| m == self.me) {
                    self.convs.remove(&cid);
                    self.viewing.remove(&cid);
                    out.push(json!({"type": "member_removed", "conv": cid}));
                    out.push(json!({"type": "summary_changed", "conv": cid}));
                }
            }
            "typing_started" => {
                let Some(cid) = id_of(&d["channel_id"]) else { return out };
                if self.viewing.contains(&cid) && id_of(&d["member_id"]).is_none_or(|m| m != self.me) {
                    out.push(json!({"type": "typing", "conv": cid, "member": d["member_id"]}));
                }
            }
            _ => {}
        }
        out
    }

    /// One conversation's summary row (§14.4 `summary`).
    pub fn summary_row(&self, c: &Conv, unread_count_flag: bool) -> Value {
        let badge = k2_core::home_chat::badge(c.dm, c.unread, c.mention_count, c.unread_count, unread_count_flag);
        json!({
            "conv": c.channel_id,
            "kind": if c.dm { "dm" } else { "channel" },
            "unread": c.unread,
            "badge": badge,
            "lastAt": c.last_at,
            "lastMessageId": c.last_message_id,
            // Data for the row's glyph (# open, a lock invite-only); the
            // routes fill it from K2's ledger when Sew hasn't said.
            "inviteOnly": if c.dm { None } else { c.invite_only },
        })
    }

    /// The summary for `convs` (every conversation when empty).
    pub fn summary(&self, convs: &[String], unread_count_flag: bool) -> Vec<Value> {
        let mut rows: Vec<&Conv> = if convs.is_empty() {
            self.convs.values().collect()
        } else {
            convs.iter().filter_map(|c| self.convs.get(c)).collect()
        };
        rows.sort_by(|a, b| a.channel_id.cmp(&b.channel_id));
        rows.into_iter().map(|c| self.summary_row(c, unread_count_flag)).collect()
    }
}
