//! One person's chat state as the daemon follows it (§14.2, §15.1): per
//! conversation, unread, the DM's `mention_count`, Sew's `unread_count`
//! (when Sew has it) and the last message. Pure: the follower feeds it
//! READY, gateway events and catch-up answers; the routes and the events
//! socket read it. No chat text is kept (§16).

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
        *self = PersonState { viewing, ..Default::default() };
        self.me = id_of(&d["member"]["id"]).unwrap_or_default();
        self.member_seq = d["member_seq"].as_u64().unwrap_or(0);
        self.gateway_session = d["session_id"].as_str().map(str::to_string);
        self.apply_channels(&d["channels"]);
        for rs in d["read_states"].as_array().into_iter().flatten() {
            self.apply_read_state(rs);
        }
    }

    /// `GET /api/v1/me/sync` after `410 resync_required`: every read state
    /// rebuilt (the gateway session is kept).
    pub fn apply_sync(&mut self, d: &Value) {
        let keep = (self.me.clone(), self.gateway_session.clone(), self.seq, std::mem::take(&mut self.viewing));
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
                let mine = id_of(&m["author_id"]).is_some_and(|a| a == self.me);
                let c = self.convs.entry(cid.clone()).or_insert_with(|| Conv {
                    channel_id: cid.clone(),
                    dm: m["space_id"].is_null() && d.get("space_id").is_none_or(Value::is_null),
                    ..Default::default()
                });
                c.last_message_id = id_of(&m["id"]).or(c.last_message_id.take());
                c.last_at = m["created_at"].as_str().map(str::to_string).or(c.last_at.take());
                if mine {
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
