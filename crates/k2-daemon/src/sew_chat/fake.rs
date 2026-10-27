//! A recording fake Sew for the route tests (no socket, no Sew).
//!
//! It models members (agents by external id, persons), Spaces, channels
//! with per-member visibility, and messages with per-viewer `following` /
//! `mentions_me`. Every call is recorded with its act-as identity. Like
//! Sew it masks hidden channels and messages as 404, dedups writes on
//! `(acting agent, Idempotency-Key)`, and refuses a post where the
//! agent lacks SEND_MESSAGES. It panics on a K2 bug Sew would reject:
//! a missing `k2_session`, or a write without a `k2c-` idempotency key.
//!
//! Community management (K20): Space membership, roles, overwrites, bans,
//! timeouts and channel creation are modelled per Space. Like Sew S12, a
//! management call needs the acting member to be a manager of that Space
//! ([`FakeSew::make_manager`]); otherwise 403 `missing_permission`. A
//! Space created through mgmt refuses an agent owner (400) and makes the
//! acting agent a manager.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use super::sew::*;
use k2_core::sew_chat::hub;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub op: String,
    pub external_id: String,
    pub k2_session: String,
    pub on_behalf_of: Option<String>,
    pub actor: Option<String>,
    pub key: Option<String>,
    pub target: String,
}

struct FChannel {
    c: Channel,
    /// `None` = every member sees it.
    visible_to: Option<HashSet<String>>,
}

struct FMessage {
    m: Message,
    mentions: HashSet<String>,
    followers: HashSet<String>,
}

#[derive(Default)]
struct State {
    agents: HashMap<String, String>,
    members: HashMap<String, (String, bool)>,
    spaces: Vec<Space>,
    default_space: Option<String>,
    channels: Vec<FChannel>,
    messages: Vec<FMessage>,
    next_id: u64,
    calls: Vec<Call>,
    idem: HashMap<(String, String), String>,
    fail_next: Option<SewError>,
    lose_next_response: bool,
    /// Space id → member ids.
    space_members: HashMap<String, HashSet<String>>,
    /// Space id → managers (Sew's MANAGE_* holders).
    managers: HashMap<String, HashSet<String>>,
    /// Space id → roles; role id → members.
    roles: HashMap<String, Vec<Role>>,
    role_members: HashMap<String, HashSet<String>>,
    /// Channel id → overwrites.
    overwrites: HashMap<String, Vec<Overwrite>>,
    /// (space, member) → ban reason.
    bans: HashMap<(String, String), Option<String>>,
    /// (space, member) → timed out until.
    timeouts: HashMap<(String, String), i64>,
    /// Space creates: (slug, owner, manager external id, Sew-Actor).
    space_creates: Vec<(String, String, String, Option<String>)>,
    /// S31: member id → its delivery log (seq, entry JSON), oldest first.
    feed: HashMap<String, Vec<(u64, serde_json::Value)>>,
    /// Member id → the log's floor (entries at or below are trimmed).
    feed_floor: HashMap<String, u64>,
    /// The install's sync epoch.
    epoch: Option<String>,
    /// (member, channel) → the last message read.
    last_read: HashMap<(String, String), u64>,
    /// (member, channel): notify level `all` (a channel follow).
    channel_follows: HashSet<(String, String)>,
}

#[derive(Default)]
pub struct FakeSew {
    s: Mutex<State>,
}

impl FakeSew {
    pub fn new() -> Self {
        let f = Self::default();
        f.s.lock().unwrap().next_id = 1000;
        f
    }

    fn id(s: &mut State) -> String {
        s.next_id += 1;
        s.next_id.to_string()
    }

    /// An agent member for a K2 workspace. Returns its member id.
    pub fn add_agent(&self, workspace_uuid: &str, username: &str) -> String {
        let mut s = self.s.lock().unwrap();
        let id = format!("mem-{username}");
        s.agents.insert(format!("k2:workspace:{workspace_uuid}"), id.clone());
        s.members.insert(id.clone(), (username.into(), true));
        for m in s.space_members.values_mut() {
            m.insert(id.clone());
        }
        id
    }

    pub fn add_person(&self, username: &str) -> String {
        let mut s = self.s.lock().unwrap();
        let id = format!("mem-{username}");
        s.members.insert(id.clone(), (username.into(), false));
        for m in s.space_members.values_mut() {
            m.insert(id.clone());
        }
        id
    }

    /// Give a member Sew's MANAGE_* in a Space (a Space manager).
    pub fn make_manager(&self, space_id: &str, member_id: &str) {
        self.s.lock().unwrap().managers.entry(space_id.into()).or_default().insert(member_id.into());
    }

    /// Take a member out of a Space (test setup; no act-as).
    pub fn kick(&self, space_id: &str, member_id: &str) {
        self.s.lock().unwrap().space_members.get_mut(space_id).expect("space").remove(member_id);
    }

    pub fn is_space_member(&self, space_id: &str, member_id: &str) -> bool {
        self.s.lock().unwrap().space_members.get(space_id).is_some_and(|m| m.contains(member_id))
    }

    pub fn is_banned(&self, space_id: &str, member_id: &str) -> bool {
        self.s.lock().unwrap().bans.contains_key(&(space_id.to_string(), member_id.to_string()))
    }

    pub fn timeout_until(&self, space_id: &str, member_id: &str) -> Option<i64> {
        self.s.lock().unwrap().timeouts.get(&(space_id.to_string(), member_id.to_string())).copied()
    }

    pub fn overwrites_of(&self, channel_id: &str) -> Vec<Overwrite> {
        self.s.lock().unwrap().overwrites.get(channel_id).cloned().unwrap_or_default()
    }

    pub fn role_holders(&self, role_id: &str) -> Vec<String> {
        let mut v: Vec<String> =
            self.s.lock().unwrap().role_members.get(role_id).map(|m| m.iter().cloned().collect()).unwrap_or_default();
        v.sort();
        v
    }

    pub fn space_creates(&self) -> Vec<(String, String, String, Option<String>)> {
        self.s.lock().unwrap().space_creates.clone()
    }

    /// A channel's privacy as Sew holds it.
    pub fn channel_private(&self, channel_id: &str) -> Option<bool> {
        self.s.lock().unwrap().channels.iter().find(|c| c.c.id == channel_id).map(|c| c.c.private)
    }

    fn need_manager(s: &State, space_id: &str, me: &str) -> Result<(), SewError> {
        if !s.spaces.iter().any(|x| x.id == space_id) || !s.space_members.get(space_id).is_some_and(|m| m.contains(me)) {
            return Err(SewError::not_found("space not found"));
        }
        if s.managers.get(space_id).is_some_and(|m| m.contains(me)) {
            Ok(())
        } else {
            let mut e = SewError::new(403, "missing_permission", "you don't manage this Space");
            e.reason = Some("MANAGE_SPACE".into());
            Err(e)
        }
    }

    fn member_ref(s: &State, id: &str) -> MemberRef {
        let (username, is_agent) = s.members[id].clone();
        MemberRef { member_id: id.into(), username, is_agent }
    }

    pub fn add_space(&self, id: &str, slug: &str, default: bool) {
        let mut s = self.s.lock().unwrap();
        s.spaces.push(Space { id: id.into(), slug: slug.into(), name: slug.to_uppercase() });
        let all: HashSet<String> = s.members.keys().cloned().collect();
        s.space_members.insert(id.into(), all);
        if default {
            s.default_space = Some(id.into());
        }
    }

    /// A text channel. `visible_to` None = everyone; `can_send` = the
    /// agent's SEND_MESSAGES.
    pub fn add_channel(&self, space_id: &str, id: &str, slug: &str, visible_to: Option<&[&str]>, can_send: bool) {
        let mut s = self.s.lock().unwrap();
        let space_slug = s.spaces.iter().find(|x| x.id == space_id).map(|x| x.slug.clone());
        s.channels.push(FChannel {
            c: Channel {
                id: id.into(),
                space_id: Some(space_id.into()),
                space_slug,
                slug: slug.into(),
                name: slug.into(),
                kind: ChannelKind::Text,
                private: visible_to.is_some(),
                archived: false,
                can_send,
                dm_member_ids: vec![],
            },
            visible_to: visible_to.map(|v| v.iter().map(|x| x.to_string()).collect()),
        });
    }

    /// A DM between members (outside every Space).
    pub fn add_dm(&self, id: &str, members: &[&str]) {
        let mut s = self.s.lock().unwrap();
        let slug = members.iter().map(|m| m.trim_start_matches("mem-")).collect::<Vec<_>>().join("+");
        s.channels.push(FChannel {
            c: Channel {
                id: id.into(),
                space_id: None,
                space_slug: None,
                slug,
                name: "dm".into(),
                kind: ChannelKind::Dm,
                private: true,
                archived: false,
                can_send: true,
                dm_member_ids: members.iter().map(|m| m.to_string()).collect(),
            },
            visible_to: Some(members.iter().map(|m| m.to_string()).collect()),
        });
    }

    /// A message by `author` (member id); returns its Sew id.
    pub fn post(&self, author: &str, channel_id: &str, body: &str, reply_to: Option<&str>, mentions: &[&str]) -> String {
        let mut s = self.s.lock().unwrap();
        let id = Self::id(&mut s);
        let (username, is_agent) = s.members.get(author).cloned().expect("known author");
        let created_at = 1_000 + s.next_id as i64;
        if let Some(root) = reply_to {
            let r = s.messages.iter_mut().find(|m| m.m.id == root).expect("known root");
            r.m.reply_count += 1;
            r.followers.insert(author.to_string());
        }
        s.messages.push(FMessage {
            m: Message {
                id: id.clone(),
                channel_id: channel_id.into(),
                author_id: author.into(),
                author_username: username,
                author_is_agent: is_agent,
                on_behalf_of: None,
                body: body.into(),
                created_at,
                edited_at: None,
                deleted: false,
                reply_to: reply_to.map(str::to_string),
                reply_count: 0,
                following: false,
                mentions_me: false,
                message_kind: None,
                wake: None,
                response: None,
            },
            mentions: mentions.iter().map(|m| m.to_string()).collect(),
            followers: [author.to_string()].into_iter().collect(),
        });
        id
    }

    pub fn follow(&self, member: &str, root: &str) {
        let mut s = self.s.lock().unwrap();
        s.messages.iter_mut().find(|m| m.m.id == root).expect("root").followers.insert(member.into());
    }

    // ── S31 delivery feed ────────────────────────────────────────────

    /// Append a delivery of message `msg_id` to `member`'s feed (Sew writes
    /// these itself). `place` None = an older Sew that sends no place;
    /// `extra` overrides or adds message fields (KM1, on behalf…). Returns
    /// the entry's seq.
    pub fn deliver(&self, member: &str, reason: &str, msg_id: &str, place: Option<&str>, extra: serde_json::Value) -> u64 {
        let mut s = self.s.lock().unwrap();
        let f = s.messages.iter().find(|m| m.m.id == msg_id).expect("known message");
        let m = f.m.clone();
        let ch = s.channels.iter().find(|c| c.c.id == m.channel_id).expect("known channel").c.clone();
        let addr = match (&ch.space_slug, ch.kind.is_dm()) {
            (Some(sp), false) => format!("{sp}/#{}", ch.slug),
            _ => format!("dm/@{}", m.author_username),
        };
        let mut msg = serde_json::json!({
            "id": m.id, "channel_id": m.channel_id, "author_id": m.author_id,
            "author_display_name": m.author_username.to_uppercase(), "author_username": m.author_username,
            "author_kind": if m.author_is_agent { "agent" } else { "person" },
            "addr": addr, "kind": "text", "body": m.body, "created_at": "2026-10-27T10:00:00.000Z",
        });
        if let Some(sp) = &ch.space_id {
            msg["space_id"] = sp.clone().into();
        }
        if let Some(r) = &m.reply_to {
            msg["thread_root_id"] = r.clone().into();
        }
        if let Some(p) = place {
            msg["place"] = p.into();
        }
        for (k, v) in extra.as_object().expect("extra is an object") {
            msg[k] = v.clone();
        }
        let log = s.feed.entry(member.to_string()).or_default();
        let seq = log.last().map(|(q, _)| q + 1).unwrap_or(1);
        log.push((seq, serde_json::json!({"seq": seq, "at": "2026-10-27T10:00:00.000Z", "reason": reason, "message": msg})));
        seq
    }

    /// The feed head of `member` (0 = empty).
    pub fn feed_head(&self, member: &str) -> u64 {
        self.s.lock().unwrap().feed.get(member).and_then(|l| l.last()).map(|(q, _)| *q).unwrap_or(0)
    }

    /// Trim `member`'s feed up to `floor` (a cursor below it gets 410
    /// behind_floor).
    pub fn trim_feed(&self, member: &str, floor: u64) {
        self.s.lock().unwrap().feed_floor.insert(member.to_string(), floor);
    }

    /// Change the install's sync epoch (a restore).
    pub fn set_epoch(&self, e: &str) {
        self.s.lock().unwrap().epoch = Some(e.to_string());
    }

    pub fn follows_channel(&self, member: &str, channel_id: &str) -> bool {
        self.s.lock().unwrap().channel_follows.contains(&(member.to_string(), channel_id.to_string()))
    }

    pub fn follows_thread(&self, member: &str, root: &str) -> bool {
        self.s.lock().unwrap().messages.iter().any(|m| m.m.id == root && m.followers.contains(member))
    }

    pub fn calls(&self) -> Vec<Call> {
        self.s.lock().unwrap().calls.clone()
    }

    /// Recorded writes (create/edit/delete).
    pub fn writes(&self) -> Vec<Call> {
        self.calls().into_iter().filter(|c| matches!(c.op.as_str(), "create" | "edit" | "delete")).collect()
    }

    pub fn body_of(&self, id: &str) -> Option<String> {
        self.s.lock().unwrap().messages.iter().find(|m| m.m.id == id).map(|m| m.m.body.clone())
    }

    pub fn message_count(&self) -> usize {
        self.s.lock().unwrap().messages.len()
    }

    /// The next call fails with `e` (after it is recorded).
    pub fn fail_next(&self, e: SewError) {
        self.s.lock().unwrap().fail_next = Some(e);
    }

    /// The next write is applied but its response is lost (503).
    pub fn lose_next_response(&self) {
        self.s.lock().unwrap().lose_next_response = true;
    }

    fn enter(&self, s: &mut State, op: &str, a: &ActAs, key: Option<&str>, target: &str) -> Result<String, SewError> {
        assert!(!a.k2_session.is_empty(), "act-as without a k2_session ({op})");
        if matches!(op, "create" | "edit" | "delete") {
            let k = key.expect("a write carries an Idempotency-Key");
            assert!(k.starts_with("k2c-") && k.len() > 4, "Idempotency-Key must be k2c-<client key>, got {k}");
        }
        s.calls.push(Call {
            op: op.into(),
            external_id: a.external_id(),
            k2_session: a.k2_session.clone(),
            on_behalf_of: a.on_behalf_of.clone(),
            actor: a.actor.clone(),
            key: key.map(str::to_string),
            target: target.into(),
        });
        if let Some(e) = s.fail_next.take() {
            return Err(e);
        }
        s.agents
            .get(&a.external_id())
            .cloned()
            .ok_or_else(|| SewError::new(403, "not_provisioned", "this agent has no Sew member yet"))
    }

    fn visible(s: &State, me: &str, channel_id: &str) -> Result<Channel, SewError> {
        s.channels
            .iter()
            .find(|c| c.c.id == channel_id && c.visible_to.as_ref().is_none_or(|v| v.contains(me)))
            .map(|c| c.c.clone())
            .ok_or_else(|| SewError::not_found("channel not found"))
    }

    fn view(f: &FMessage, me: &str) -> Message {
        let mut m = f.m.clone();
        m.following = f.followers.contains(me);
        m.mentions_me = f.mentions.contains(me);
        m
    }

    fn find_msg(s: &State, me: &str, id: &str) -> Result<Message, SewError> {
        let f = s.messages.iter().find(|m| m.m.id == id).ok_or_else(|| SewError::not_found("message not found"))?;
        Self::visible(s, me, &f.m.channel_id).map_err(|_| SewError::not_found("message not found"))?;
        Ok(Self::view(f, me))
    }
}

impl SewChat for FakeSew {
    fn me(&self, a: &ActAs) -> Result<Me, SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "me", a, None, "")?;
        Ok(Me { member_id: me.clone(), username: s.members[&me].0.clone() })
    }

    fn spaces(&self, a: &ActAs) -> Result<Vec<Space>, SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "spaces", a, None, "")?;
        Ok(s.spaces.iter().filter(|x| s.space_members.get(&x.id).is_some_and(|m| m.contains(&me))).cloned().collect())
    }

    fn default_space(&self, a: &ActAs) -> Result<Option<Space>, SewError> {
        let mut s = self.s.lock().unwrap();
        self.enter(&mut s, "default_space", a, None, "")?;
        Ok(s.default_space.as_ref().and_then(|d| s.spaces.iter().find(|x| &x.id == d).cloned()))
    }

    fn channels(&self, a: &ActAs, space_id: Option<&str>) -> Result<Vec<Channel>, SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "channels", a, None, space_id.unwrap_or(""))?;
        Ok(s.channels
            .iter()
            .filter(|c| c.visible_to.as_ref().is_none_or(|v| v.contains(&me)))
            .filter(|c| space_id.is_none_or(|sp| c.c.space_id.as_deref() == Some(sp)))
            .map(|c| c.c.clone())
            .collect())
    }

    fn resolve(&self, a: &ActAs, space: &str, channel: &str) -> Result<Channel, SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "resolve", a, None, &format!("{space}/{channel}"))?;
        let sp = s.spaces.iter().find(|x| x.slug == space).cloned().ok_or_else(|| SewError::not_found("channel not found"))?;
        let id = s
            .channels
            .iter()
            .find(|c| {
                c.c.space_id.as_deref() == Some(sp.id.as_str())
                    && (c.c.slug == channel || channel.strip_prefix('#') == Some(c.c.id.as_str()))
            })
            .map(|c| c.c.id.clone())
            .ok_or_else(|| SewError::not_found("channel not found"))?;
        Self::visible(&s, &me, &id)
    }

    fn channel(&self, a: &ActAs, id: &str) -> Result<Channel, SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "channel", a, None, id)?;
        Self::visible(&s, &me, id)
    }

    fn messages(&self, a: &ActAs, channel_id: &str, limit: u32, before: Option<&str>) -> Result<Page, SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "messages", a, None, channel_id)?;
        Self::visible(&s, &me, channel_id)?;
        let before: Option<u64> = before.and_then(|b| b.parse().ok());
        let all: Vec<Message> = s
            .messages
            .iter()
            .filter(|m| m.m.channel_id == channel_id && m.m.reply_to.is_none())
            .filter(|m| before.is_none_or(|b| m.m.id.parse::<u64>().unwrap() < b))
            .map(|m| Self::view(m, &me))
            .collect();
        let start = all.len().saturating_sub(limit as usize);
        Ok(Page { has_more: start > 0, messages: all[start..].to_vec() })
    }

    fn message(&self, a: &ActAs, id: &str) -> Result<Message, SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "message", a, None, id)?;
        Self::find_msg(&s, &me, id)
    }

    fn wait(&self, a: &ActAs, channel_id: &str, after: Option<&str>, wait_ms: u64) -> Result<Vec<Message>, SewError> {
        assert!(wait_ms <= k2_core::sew_chat::SEW_WAIT_MS_MAX, "wait_ms over Sew's cap: {wait_ms}");
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "wait", a, None, channel_id)?;
        Self::visible(&s, &me, channel_id)?;
        let after: u64 = after.and_then(|x| x.parse().ok()).unwrap_or(0);
        Ok(s.messages
            .iter()
            .filter(|m| m.m.channel_id == channel_id && m.m.id.parse::<u64>().unwrap() > after)
            .map(|m| Self::view(m, &me))
            .collect())
    }

    fn open_dm(&self, a: &ActAs, username: &str) -> Result<Channel, SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "open_dm", a, None, username)?;
        let other = s
            .members
            .iter()
            .find(|(_, (u, _))| u == username)
            .map(|(id, _)| id.clone())
            .ok_or_else(|| SewError::not_found("open the DM from the chat app first"))?;
        s.channels
            .iter()
            .find(|c| c.c.kind.is_dm() && c.c.dm_member_ids.contains(&me) && c.c.dm_member_ids.contains(&other))
            .map(|c| c.c.clone())
            .ok_or_else(|| SewError::not_found("open the DM from the chat app first"))
    }

    fn create(&self, a: &ActAs, m: &NewMessage) -> Result<Message, SewError> {
        let mut st = self.s.lock().unwrap();
        let me = self.enter(&mut st, "create", a, Some(&m.idempotency_key), &m.channel_id)?;
        if let Some(id) = st.idem.get(&(a.external_id(), m.idempotency_key.clone())).cloned() {
            return Self::find_msg(&st, &me, &id);
        }
        let ch = Self::visible(&st, &me, &m.channel_id)?;
        if !ch.can_send {
            let mut e = SewError::new(403, "missing_permission", "you can't post in this channel");
            e.reason = Some("SEND_MESSAGES".into());
            return Err(e);
        }
        let lose = std::mem::take(&mut st.lose_next_response);
        drop(st);
        let id = self.post(&me, &m.channel_id, &m.body, m.reply_to.as_deref(), &[]);
        let mut st = self.s.lock().unwrap();
        if let Some(f) = st.messages.iter_mut().find(|x| x.m.id == id) {
            f.m.on_behalf_of = a.on_behalf_of.clone();
        }
        st.idem.insert((a.external_id(), m.idempotency_key.clone()), id.clone());
        if lose {
            return Err(SewError::new(503, "unavailable", "connection reset"));
        }
        Self::find_msg(&st, &me, &id)
    }

    fn edit(&self, a: &ActAs, id: &str, body: &str, key: &str) -> Result<Message, SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "edit", a, Some(key), id)?;
        Self::find_msg(&s, &me, id)?;
        let f = s.messages.iter_mut().find(|m| m.m.id == id).unwrap();
        if f.m.author_id != me {
            return Err(SewError::new(403, "missing_permission", "you can only edit your own messages"));
        }
        f.m.body = body.into();
        f.m.edited_at = Some(9_999);
        let out = Self::view(f, &me);
        Ok(out)
    }

    fn delete(&self, a: &ActAs, id: &str, key: &str) -> Result<(), SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "delete", a, Some(key), id)?;
        Self::find_msg(&s, &me, id)?;
        let channel_id = s.messages.iter().find(|m| m.m.id == id).unwrap().m.channel_id.clone();
        let space = s.channels.iter().find(|c| c.c.id == channel_id).and_then(|c| c.c.space_id.clone());
        let manager = space.as_ref().is_some_and(|sp| s.managers.get(sp).is_some_and(|m| m.contains(&me)));
        let f = s.messages.iter_mut().find(|m| m.m.id == id).unwrap();
        if f.m.author_id != me && !manager {
            return Err(SewError::new(403, "missing_permission", "you can only delete your own messages"));
        }
        f.m.deleted = true;
        f.m.body.clear();
        Ok(())
    }

    fn member(&self, a: &ActAs, who: &MemberLookup) -> Result<MemberRef, SewError> {
        let mut s = self.s.lock().unwrap();
        let target = match who {
            MemberLookup::Username(u) => u.clone(),
            MemberLookup::External(e) => e.clone(),
        };
        self.enter(&mut s, "member", a, None, &target)?;
        let id = match who {
            MemberLookup::Username(u) => s.members.iter().find(|(_, (n, _))| n == u).map(|(id, _)| id.clone()),
            MemberLookup::External(e) => s.agents.get(e).cloned(),
        };
        id.map(|id| Self::member_ref(&s, &id)).ok_or_else(|| SewError::not_found("member not found"))
    }

    fn space_members(&self, a: &ActAs, space_id: &str) -> Result<Vec<MemberRef>, SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "space_members", a, None, space_id)?;
        let m = s
            .space_members
            .get(space_id)
            .filter(|m| m.contains(&me))
            .ok_or_else(|| SewError::not_found("space not found"))?;
        let mut ids: Vec<String> = m.iter().cloned().collect();
        ids.sort();
        Ok(ids.iter().map(|id| Self::member_ref(&s, id)).collect())
    }

    fn space_member_add(&self, a: &ActAs, space_id: &str, member_id: &str) -> Result<(), SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "space_member_add", a, None, &format!("{space_id}:{member_id}"))?;
        Self::need_manager(&s, space_id, &me)?;
        if s.bans.contains_key(&(space_id.to_string(), member_id.to_string())) {
            return Err(SewError::new(422, "invalid_state", "that member is banned from this Space"));
        }
        s.space_members.get_mut(space_id).expect("space").insert(member_id.into());
        Ok(())
    }

    fn space_member_remove(&self, a: &ActAs, space_id: &str, member_id: &str) -> Result<(), SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "space_member_remove", a, None, &format!("{space_id}:{member_id}"))?;
        Self::need_manager(&s, space_id, &me)?;
        s.space_members.get_mut(space_id).expect("space").remove(member_id);
        Ok(())
    }

    fn channel_create(&self, a: &ActAs, space_id: &str, name: &str, private: bool, key: &str) -> Result<Channel, SewError> {
        assert!(key.starts_with("k2c-"), "channel create carries a k2c- Idempotency-Key, got {key}");
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "channel_create", a, None, &format!("{space_id}/{name}"))?;
        Self::need_manager(&s, space_id, &me)?;
        let slug = name.to_ascii_lowercase();
        if s.channels.iter().any(|c| c.c.space_id.as_deref() == Some(space_id) && c.c.slug == slug) {
            return Err(SewError::new(409, "slug_taken", "a channel with that name already exists"));
        }
        let id = format!("c-{}", Self::id(&mut s));
        let space_slug = s.spaces.iter().find(|x| x.id == space_id).map(|x| x.slug.clone());
        let c = Channel {
            id: id.clone(),
            space_id: Some(space_id.into()),
            space_slug,
            slug,
            name: name.into(),
            kind: ChannelKind::Text,
            private,
            archived: false,
            can_send: true,
            dm_member_ids: vec![],
        };
        s.channels.push(FChannel { c: c.clone(), visible_to: None });
        Ok(c)
    }

    fn channel_set_private(&self, a: &ActAs, channel_id: &str, private: bool) -> Result<Channel, SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "channel_set_private", a, None, channel_id)?;
        let ch = Self::visible(&s, &me, channel_id)?;
        Self::need_manager(&s, ch.space_id.as_deref().unwrap_or(""), &me)?;
        let f = s.channels.iter_mut().find(|c| c.c.id == channel_id).expect("channel");
        f.c.private = private;
        Ok(f.c.clone())
    }

    fn roles(&self, a: &ActAs, space_id: &str) -> Result<Vec<Role>, SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "roles", a, None, space_id)?;
        if !s.space_members.get(space_id).is_some_and(|m| m.contains(&me)) {
            return Err(SewError::not_found("space not found"));
        }
        Ok(s.roles.get(space_id).cloned().unwrap_or_default())
    }

    fn role_create(&self, a: &ActAs, space_id: &str, name: &str, key: &str) -> Result<Role, SewError> {
        assert!(key.starts_with("k2c-"), "role create carries a k2c- Idempotency-Key, got {key}");
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "role_create", a, None, &format!("{space_id}/{name}"))?;
        Self::need_manager(&s, space_id, &me)?;
        if s.roles.get(space_id).is_some_and(|r| r.iter().any(|r| r.name.eq_ignore_ascii_case(name))) {
            return Err(SewError::new(409, "name_taken", "a role with that name already exists"));
        }
        let r = Role { id: format!("role-{}", Self::id(&mut s)), name: name.into() };
        s.roles.entry(space_id.into()).or_default().push(r.clone());
        Ok(r)
    }

    fn role_member(&self, a: &ActAs, space_id: &str, role_id: &str, member_id: &str, add: bool) -> Result<(), SewError> {
        let mut s = self.s.lock().unwrap();
        let op = if add { "role_add" } else { "role_remove" };
        let me = self.enter(&mut s, op, a, None, &format!("{role_id}:{member_id}"))?;
        Self::need_manager(&s, space_id, &me)?;
        if !s.roles.get(space_id).is_some_and(|r| r.iter().any(|r| r.id == role_id)) {
            return Err(SewError::not_found("role not found"));
        }
        let set = s.role_members.entry(role_id.into()).or_default();
        if add {
            set.insert(member_id.into());
        } else {
            set.remove(member_id);
        }
        Ok(())
    }

    fn channel_perms(&self, a: &ActAs, channel_id: &str, o: &Overwrite) -> Result<(), SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "channel_perms", a, None, channel_id)?;
        let ch = Self::visible(&s, &me, channel_id)?;
        Self::need_manager(&s, ch.space_id.as_deref().unwrap_or(""), &me)?;
        let v = s.overwrites.entry(channel_id.into()).or_default();
        v.retain(|x| x.role_id != o.role_id);
        v.push(o.clone());
        Ok(())
    }

    fn ban(&self, a: &ActAs, space_id: &str, member_id: &str, reason: Option<&str>) -> Result<(), SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "ban", a, None, &format!("{space_id}:{member_id}"))?;
        Self::need_manager(&s, space_id, &me)?;
        if member_id == me {
            return Err(SewError::new(422, "invalid_state", "you can't ban yourself"));
        }
        s.bans.insert((space_id.into(), member_id.into()), reason.map(str::to_string));
        s.space_members.get_mut(space_id).expect("space").remove(member_id);
        Ok(())
    }

    fn unban(&self, a: &ActAs, space_id: &str, member_id: &str) -> Result<(), SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "unban", a, None, &format!("{space_id}:{member_id}"))?;
        Self::need_manager(&s, space_id, &me)?;
        s.bans
            .remove(&(space_id.to_string(), member_id.to_string()))
            .map(|_| ())
            .ok_or_else(|| SewError::not_found("not banned"))
    }

    fn timeout(&self, a: &ActAs, space_id: &str, member_id: &str, until: i64, _reason: Option<&str>) -> Result<(), SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "timeout", a, None, &format!("{space_id}:{member_id}"))?;
        Self::need_manager(&s, space_id, &me)?;
        s.timeouts.insert((space_id.into(), member_id.into()), until);
        Ok(())
    }

    fn space_create(&self, a: &ActAs, n: &NewSpace) -> Result<Space, SewError> {
        assert!(n.idempotency_key.starts_with("k2c-"), "space create carries a k2c- Idempotency-Key");
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "space_create", a, None, &n.slug)?;
        if n.owner_external_id.starts_with("k2:workspace:") {
            return Err(SewError::new(400, "invalid_request", "an agent can't own a Space"));
        }
        if s.spaces.iter().any(|x| x.slug == n.slug) {
            return Err(SewError::new(409, "slug_taken", "that Space slug is taken"));
        }
        let id = format!("sp-{}", Self::id(&mut s));
        let sp = Space { id: id.clone(), slug: n.slug.clone(), name: n.name.clone() };
        s.spaces.push(sp.clone());
        s.space_members.insert(id.clone(), [me.clone()].into_iter().collect());
        s.managers.insert(id, [me].into_iter().collect());
        s.space_creates.push((n.slug.clone(), n.owner_external_id.clone(), a.external_id(), a.actor.clone()));
        Ok(sp)
    }

    fn deliveries(&self, a: &ActAs, member_id: &str, after: u64, timeout_ms: u64) -> Result<hub::Step, SewError> {
        assert!(timeout_ms <= k2_core::sew_chat::SEW_WAIT_MS_MAX, "timeout_ms over Sew's cap: {timeout_ms}");
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "deliveries", a, None, &format!("{member_id}>{after}"))?;
        assert_eq!(me, member_id, "the hub reads its own agent's feed only");
        let log = s.feed.get(member_id).cloned().unwrap_or_default();
        let head = log.last().map(|(q, _)| *q).unwrap_or(0);
        let floor = s.feed_floor.get(member_id).copied().unwrap_or(0);
        if after < floor {
            return Ok(hub::Step::Resync { reason: "behind_floor".into(), head, sync_epoch: s.epoch.clone() });
        }
        if after > head {
            return Ok(hub::Step::Resync { reason: "ahead_of_head".into(), head, sync_epoch: s.epoch.clone() });
        }
        let entries: Vec<serde_json::Value> = log.iter().filter(|(q, _)| *q > after).take(100).map(|(_, e)| e.clone()).collect();
        let next = entries.last().and_then(|e| e["seq"].as_u64()).unwrap_or(head.max(after));
        let mut page = serde_json::json!({
            "entries": entries, "head": head, "next_after": next, "has_more": next < head,
        });
        if let Some(e) = &s.epoch {
            page["sync_epoch"] = e.clone().into();
        }
        hub::decode_page(&page).map_err(|e| SewError::new(502, "bad_response", &e))
    }

    fn read_states(&self, a: &ActAs) -> Result<Vec<ReadState>, SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "read_states", a, None, "")?;
        let mut out = Vec::new();
        for c in s.channels.iter().filter(|c| c.visible_to.as_ref().is_none_or(|v| v.contains(&me))) {
            let read = s.last_read.get(&(me.clone(), c.c.id.clone())).copied().unwrap_or(0);
            let msgs: Vec<&FMessage> = s.messages.iter().filter(|m| m.m.channel_id == c.c.id).collect();
            let newer: Vec<&&FMessage> =
                msgs.iter().filter(|m| m.m.author_id != me && m.m.id.parse::<u64>().unwrap() > read).collect();
            out.push(ReadState {
                channel_id: c.c.id.clone(),
                unread: !newer.is_empty(),
                mentions: newer.iter().filter(|m| m.mentions.contains(&me)).count() as u32,
                last_read_id: (read > 0).then(|| read.to_string()),
                last_message_id: msgs.last().map(|m| m.m.id.clone()),
                muted: false,
            });
        }
        Ok(out)
    }

    fn mark_read(&self, a: &ActAs, channel_ids: &[String]) -> Result<(), SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, "mark_read", a, None, &channel_ids.join(","))?;
        for c in channel_ids {
            Self::visible(&s, &me, c)?;
            let newest = s.messages.iter().filter(|m| &m.m.channel_id == c).filter_map(|m| m.m.id.parse::<u64>().ok()).max();
            s.last_read.insert((me.clone(), c.clone()), newest.unwrap_or(0));
        }
        Ok(())
    }

    fn follow_channel(&self, a: &ActAs, channel_id: &str, on: bool) -> Result<(), SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, if on { "follow_channel" } else { "unfollow_channel" }, a, None, channel_id)?;
        Self::visible(&s, &me, channel_id)?;
        if on {
            s.channel_follows.insert((me, channel_id.into()));
        } else {
            s.channel_follows.remove(&(me, channel_id.to_string()));
        }
        Ok(())
    }

    fn follow_thread(&self, a: &ActAs, root_id: &str, on: bool) -> Result<(), SewError> {
        let mut s = self.s.lock().unwrap();
        let me = self.enter(&mut s, if on { "follow_thread" } else { "unfollow_thread" }, a, None, root_id)?;
        Self::find_msg(&s, &me, root_id)?;
        let f = s.messages.iter_mut().find(|m| m.m.id == root_id).expect("found above");
        if on {
            f.followers.insert(me);
        } else {
            f.followers.remove(&me);
        }
        Ok(())
    }
}
