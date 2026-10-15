//! A recording fake Sew for the route tests (no socket, no Sew).
//!
//! It models members (agents by external id, persons), Spaces, channels
//! with per-member visibility, and messages with per-viewer `following` /
//! `mentions_me`. Every call is recorded with its act-as identity. Like
//! Sew it masks hidden channels and messages as 404, dedups writes on
//! `(acting agent, Idempotency-Key)`, and refuses a post where the
//! agent lacks SEND_MESSAGES. It panics on a K2 bug Sew would reject:
//! a missing `k2_session`, or a write without a `k2c-` idempotency key.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use super::sew::*;

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
        id
    }

    pub fn add_person(&self, username: &str) -> String {
        let mut s = self.s.lock().unwrap();
        let id = format!("mem-{username}");
        s.members.insert(id.clone(), (username.into(), false));
        id
    }

    pub fn add_space(&self, id: &str, slug: &str, default: bool) {
        let mut s = self.s.lock().unwrap();
        s.spaces.push(Space { id: id.into(), slug: slug.into(), name: slug.to_uppercase() });
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
        self.enter(&mut s, "spaces", a, None, "")?;
        Ok(s.spaces.clone())
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
        let f = s.messages.iter_mut().find(|m| m.m.id == id).unwrap();
        if f.m.author_id != me {
            return Err(SewError::new(403, "missing_permission", "you can only delete your own messages"));
        }
        f.m.deleted = true;
        f.m.body.clear();
        Ok(())
    }
}
