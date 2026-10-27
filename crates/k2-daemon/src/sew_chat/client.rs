//! The real [`SewChat`] backend (S30 K15): Sew 0.3.0's own client
//! (`sew-client`, vendored under `third_party/`, see `SEW-VENDOR.txt`) over
//! Sew's mgmt socket, acting as the agent on every `/api/v1` call.
//!
//! **Identity.** Every chat call is `MgmtClient::act_as(ActAsTarget::External(
//! k2:workspace:<uuid>), SewVia { agent, k2_session })`: Sew's roles,
//! permissions and masked 404s apply to the agent in full. Only
//! [`SewChat::space_create`] is a plain mgmt call (Sew's SW11 route names
//! the acting agent as a manager; the owner is never an agent). The service
//! token comes from the root helper through [`crate::sew::supervisor::mgmt_token`]
//! (never Sew's token file); a 401 refreshes it once.
//!
//! **Sew-Actor (G7).** `sew-client`'s typed calls can't set extra headers,
//! so a write that carries [`ActAs::actor`] (an approved queued post) goes
//! out through `Client::request` with `Sew-Actor`, `Idempotency-Key` and
//! the act-as headers, and the answer is decoded here. The header grammar is
//! `[A-Za-z0-9_.:@-]{1,128}` ([`actor_header`] maps K2's labels onto it).
//!
//! **On behalf of.** Sew has no request field for it: Sew stamps
//! `on_behalf_of` itself on every post by an assistant member (S27's
//! `assistant:<person>` grant). [`ActAs::on_behalf_of`] is K2-side only
//! (the audit row and the approval gate); it is never sent.
//!
//! **Async bridge.** Chat handlers are blocking (the blocking pool, or the
//! cell socket's dispatch). Each call runs on a fresh scoped OS thread that
//! `block_on`s this backend's own small runtime, so it never blocks or
//! re-enters a caller's runtime (a `block_on` inside a runtime worker
//! panics). The runtime is shut down in the background on drop.
//!
//! **Caches.** Per acting agent: its Spaces (slug ↔ id, 60 s), each
//! Space's channels (for the display-name fallback of `resolve`, refreshed
//! once on a miss) and the members it has seen (username, agent or person,
//! 5 min). Nothing is persisted.
//!
//! **Not in Sew 0.3.0** (answered `501 not_supported`; the routes must say
//! so): a Space ban / unban, and changing a channel's visibility after it
//! exists (`UpdateChannelRequest` has no `visibility`). Sew has no "default
//! Space" either: K2's is the slug in `sew_chat_settings` key
//! [`DEFAULT_SPACE_KEY`] (unset = DMs outside every Space are governed by
//! no Space, so the routes answer `not_granted`).

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use sew_client::proto::addr::{Addr, ChannelRef, Handle, Slug, SpaceRef};
use sew_client::proto::api::channels::{Channel as PChannel, ChannelType, Visibility};
use sew_client::proto::api::members::{Member as PMember, MemberKind};
use sew_client::proto::api::messages::Message as PMessage;
use sew_client::proto::api::overwrites::{ChannelOverwrites, SetOverwriteRequest};
use sew_client::proto::api::resolve::ResolveResponse;
use sew_client::proto::api::spaces::{
    AddSpaceMembersRequest, Overwrite as POverwrite, OverwriteTarget, RemoveSpaceMemberRequest, Role as PRole,
    Space as PSpace, SpaceMember,
};
use sew_client::proto::external_id::ExternalId;
use sew_client::proto::http::Page as PPage;
use sew_client::proto::id::Id;
use sew_client::proto::mgmt::{ActAsTarget, MgmtPrincipalPage, MgmtSpaceCreateRequest, MgmtSpaceResponse, SewVia};
use sew_client::proto::perms::Perms;
use sew_client::proto::routes as r;
use sew_client::{Client, ClientError, MgmtClient, RequestOptions};

use super::sew::{
    ActAs, Channel, ChannelKind, ChannelPerm, Me, MemberLookup, MemberRef, Message, NewMessage, NewSpace, Overwrite,
    Page, Role, SewChat, SewError, Space,
};

/// `sew_chat_settings` key naming K2's default Space (a Space slug). Sew
/// 0.3.0 has no such notion; DMs outside every Space are governed by it.
pub const DEFAULT_SPACE_KEY: &str = k2_core::sew_chat::store::DEFAULT_SPACE_KEY;

/// Sew's long-poll cap (`limits::MESSAGES_WAIT_MAX_MS`).
const WAIT_MS_MAX: u64 = 25_000;
/// Pages read per listing before giving up (100 items each).
const MAX_PAGES: usize = 50;
const SPACES_TTL: Duration = Duration::from_secs(60);
const MEMBERS_TTL: Duration = Duration::from_secs(300);

/// Where the service token comes from: `refresh` = Sew answered 401.
pub type TokenSource = Arc<dyn Fn(bool) -> Result<String, String> + Send + Sync>;

type R<T> = Result<T, SewError>;

/// The real Sew backend. Cheap to share (`Arc`).
pub struct SewClientChat {
    rt: Option<tokio::runtime::Runtime>,
    socket: PathBuf,
    tokens: TokenSource,
    mgmt: Mutex<Option<MgmtClient>>,
    /// Acting external id → (when, Spaces it sees).
    spaces: Mutex<HashMap<String, (Instant, Vec<Space>)>>,
    /// (acting external id, Space id) → channels it sees.
    channels: Mutex<HashMap<(String, String), Vec<Channel>>>,
    /// (acting external id, member id) → (when, member).
    members: Mutex<HashMap<(String, String), (Instant, MemberRef)>>,
}

impl std::fmt::Debug for SewClientChat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SewClientChat").field("socket", &self.socket).finish()
    }
}

impl Drop for SewClientChat {
    fn drop(&mut self) {
        // `Runtime::drop` panics inside an async context; this never does.
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

impl SewClientChat {
    /// A backend for Sew's mgmt `socket`, with tokens from `tokens`.
    pub fn new(socket: impl Into<PathBuf>, tokens: TokenSource) -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("sew-chat")
            .enable_all()
            .build()
            .map_err(|e| format!("sew chat runtime: {e}"))?;
        Ok(Self {
            rt: Some(rt),
            socket: socket.into(),
            tokens,
            mgmt: Mutex::new(None),
            spaces: Mutex::new(HashMap::new()),
            channels: Mutex::new(HashMap::new()),
            members: Mutex::new(HashMap::new()),
        })
    }

    fn mgmt(&self, refresh: bool) -> R<MgmtClient> {
        let mut g = self.mgmt.lock().unwrap_or_else(|p| p.into_inner());
        if !refresh {
            if let Some(m) = g.as_ref() {
                return Ok(m.clone());
            }
        }
        let token = (self.tokens)(refresh).map_err(|e| SewError::new(503, "unavailable", &format!("no Sew mgmt token: {e}")))?;
        let m = MgmtClient::new(self.socket.clone(), token);
        *g = Some(m.clone());
        Ok(m)
    }

    /// Run `fut` to completion on this backend's runtime, from a fresh
    /// thread (never inside the caller's runtime).
    fn block<F>(&self, fut: F) -> F::Output
    where
        F: Future + Send,
        F::Output: Send,
    {
        let rt = self.rt.as_ref().expect("sew chat runtime lives until drop");
        std::thread::scope(|s| match s.spawn(|| rt.block_on(fut)).join() {
            Ok(v) => v,
            Err(p) => std::panic::resume_unwind(p),
        })
    }

    /// One logical call; a 401 refreshes the token and runs it once more
    /// (writes are idempotent by key, so the repeat is safe).
    fn run<T, Fut>(&self, f: impl Fn(MgmtClient) -> Fut) -> R<T>
    where
        T: Send,
        Fut: Future<Output = R<T>> + Send,
    {
        let m = self.mgmt(false)?;
        match self.block(f(m)) {
            Err(e) if e.status == 401 => {
                let m = self.mgmt(true)?;
                self.block(f(m))
            }
            r => r,
        }
    }

    // ── caches ───────────────────────────────────────────────────────

    fn spaces_of(&self, a: &ActAs, refresh: bool) -> R<Vec<Space>> {
        let ext = a.external_id();
        if !refresh {
            let g = self.spaces.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((t, v)) = g.get(&ext) {
                if t.elapsed() < SPACES_TTL {
                    return Ok(v.clone());
                }
            }
        }
        let a2 = a.clone();
        let v = self.run(move |m| {
            let a = a2.clone();
            async move {
                let c = act(&m, &a)?;
                let mut out = Vec::new();
                let mut after: Option<String> = None;
                for _ in 0..MAX_PAGES {
                    let mut q = vec![("scope", "mine".to_string()), ("limit", "100".to_string())];
                    if let Some(x) = &after {
                        q.push(("after", x.clone()));
                    }
                    let p: PPage<PSpace> = get(&c, r::SPACES, &q).await?;
                    after = p.items.last().map(|s| s.id.to_string());
                    out.extend(p.items.iter().map(map_space));
                    if !p.has_more || after.is_none() {
                        break;
                    }
                }
                Ok(out)
            }
        })?;
        self.spaces.lock().unwrap_or_else(|p| p.into_inner()).insert(ext, (Instant::now(), v.clone()));
        Ok(v)
    }

    /// The slug of a Space id as `a` sees it (one refresh on a miss).
    fn space_slug(&self, a: &ActAs, space_id: &str) -> R<Option<String>> {
        for refresh in [false, true] {
            if let Some(s) = self.spaces_of(a, refresh)?.into_iter().find(|s| s.id == space_id) {
                return Ok(Some(s.slug));
            }
        }
        Ok(None)
    }

    fn map_channels(&self, a: &ActAs, raw: Vec<PChannel>) -> R<Vec<Channel>> {
        let mut out = Vec::with_capacity(raw.len());
        for c in raw {
            let slug = match &c.space_id {
                Some(s) => self.space_slug(a, &s.to_string())?,
                None => None,
            };
            out.push(map_channel(&c, slug));
        }
        Ok(out)
    }

    fn space_channels(&self, a: &ActAs, space_id: &str, refresh: bool) -> R<Vec<Channel>> {
        let key = (a.external_id(), space_id.to_string());
        if !refresh {
            if let Some(v) = self.channels.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
                return Ok(v.clone());
            }
        }
        let sid = parse_id(space_id)?;
        let a2 = a.clone();
        let raw = self.run(move |m| {
            let a = a2.clone();
            async move {
                let c = act(&m, &a)?;
                let mut out = Vec::new();
                let mut after: Option<String> = None;
                for _ in 0..MAX_PAGES {
                    let mut q = vec![("limit", "100".to_string())];
                    if let Some(x) = &after {
                        q.push(("after", x.clone()));
                    }
                    let p: PPage<PChannel> = get(&c, &r::with_id(r::SPACE_CHANNELS, sid), &q).await?;
                    after = p.items.last().map(|x| x.id.to_string());
                    out.extend(p.items);
                    if !p.has_more || after.is_none() {
                        break;
                    }
                }
                Ok(out)
            }
        })?;
        let v = self.map_channels(a, raw)?;
        self.channels.lock().unwrap_or_else(|p| p.into_inner()).insert(key, v.clone());
        Ok(v)
    }

    /// Members by id as `a` sees them (cached; unknown or hidden ones are
    /// left out).
    fn members_by_id(&self, a: &ActAs, ids: &[String]) -> R<HashMap<String, MemberRef>> {
        let ext = a.external_id();
        let mut found = HashMap::new();
        let mut missing: Vec<Id> = Vec::new();
        {
            let g = self.members.lock().unwrap_or_else(|p| p.into_inner());
            for id in ids {
                match g.get(&(ext.clone(), id.clone())) {
                    Some((t, m)) if t.elapsed() < MEMBERS_TTL => {
                        found.insert(id.clone(), m.clone());
                    }
                    _ => {
                        if let Ok(i) = id.parse::<Id>() {
                            if !missing.contains(&i) {
                                missing.push(i);
                            }
                        }
                    }
                }
            }
        }
        if missing.is_empty() {
            return Ok(found);
        }
        let a2 = a.clone();
        let fetched: Vec<PMember> = self.run(move |m| {
            let a = a2.clone();
            let missing = missing.clone();
            async move {
                let c = act(&m, &a)?;
                let mut out = Vec::new();
                for chunk in missing.chunks(100) {
                    let list = chunk.iter().map(Id::to_string).collect::<Vec<_>>().join(",");
                    let p: PPage<PMember> = get(&c, r::MEMBERS, &[("ids", list)]).await?;
                    out.extend(p.items);
                }
                Ok(out)
            }
        })?;
        let mut g = self.members.lock().unwrap_or_else(|p| p.into_inner());
        for pm in fetched {
            let m = map_member(&pm);
            g.insert((ext.clone(), m.member_id.clone()), (Instant::now(), m.clone()));
            found.insert(m.member_id.clone(), m);
        }
        Ok(found)
    }

    fn map_messages(&self, a: &ActAs, raw: &[PMessage]) -> R<Vec<Message>> {
        let ids: Vec<String> = raw.iter().map(|m| m.author_id.to_string()).collect();
        let authors = self.members_by_id(a, &ids)?;
        Ok(raw.iter().map(|m| map_message(m, &authors)).collect())
    }

    fn post_write<T: DeserializeOwned + Send>(&self, a: &ActAs, path: String, body: Value, key: &str) -> R<T> {
        let a2 = a.clone();
        let key = key.to_string();
        self.run(move |m| {
            let (a, path, body, key) = (a2.clone(), path.clone(), body.clone(), key.clone());
            async move {
                let c = act(&m, &a)?;
                post(&c, &path, &body, Some(&key), a.actor.as_deref()).await
            }
        })
    }

    fn post_plain<T: DeserializeOwned + Send>(&self, a: &ActAs, path: String, body: Value) -> R<T> {
        let a2 = a.clone();
        self.run(move |m| {
            let (a, path, body) = (a2.clone(), path.clone(), body.clone());
            async move {
                let c = act(&m, &a)?;
                post(&c, &path, &body, None, None).await
            }
        })
    }

    fn get_as<T: DeserializeOwned + Send>(&self, a: &ActAs, path: String, q: Vec<(&'static str, String)>) -> R<T> {
        let a2 = a.clone();
        self.run(move |m| {
            let (a, path, q) = (a2.clone(), path.clone(), q.clone());
            async move {
                let c = act(&m, &a)?;
                get(&c, &path, &q).await
            }
        })
    }

    fn default_space_slug() -> Option<String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT value FROM sew_chat_settings WHERE key = ?1", rusqlite::params![DEFAULT_SPACE_KEY], |r| {
            r.get::<_, String>(0)
        })
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    }
}

// ── the trait ─────────────────────────────────────────────────────────

impl SewChat for SewClientChat {
    fn me(&self, a: &ActAs) -> R<Me> {
        let m: PMember = self.get_as(a, r::ME.to_string(), vec![])?;
        Ok(Me { member_id: m.id.to_string(), username: m.username.as_str().to_string() })
    }

    fn spaces(&self, a: &ActAs) -> R<Vec<Space>> {
        self.spaces_of(a, true)
    }

    fn default_space(&self, a: &ActAs) -> R<Option<Space>> {
        let Some(slug) = Self::default_space_slug() else {
            return Ok(None);
        };
        for refresh in [false, true] {
            if let Some(s) = self.spaces_of(a, refresh)?.into_iter().find(|s| s.slug == slug) {
                return Ok(Some(s));
            }
        }
        Ok(None)
    }

    fn channels(&self, a: &ActAs, space_id: Option<&str>) -> R<Vec<Channel>> {
        match space_id {
            Some(s) => self.space_channels(a, s, true),
            None => {
                let mut out = Vec::new();
                for s in self.spaces_of(a, true)? {
                    out.extend(self.space_channels(a, &s.id, true)?);
                }
                let dms: PPage<PChannel> =
                    self.get_as(a, r::ME_CHANNELS.to_string(), vec![("kind", "dm".into()), ("limit", "100".into())])?;
                out.extend(self.map_channels(a, dms.items)?);
                Ok(out)
            }
        }
    }

    fn resolve(&self, a: &ActAs, space: &str, channel: &str) -> R<Channel> {
        let addr = build_addr(space, channel)?;
        let path = format!("{}?addr={}", r::RESOLVE, addr.to_query_value());
        let first: R<ResolveResponse> = self.get_as(a, path, vec![]);
        match first {
            Ok(rr) => self.channel(a, &rr.channel_id.to_string()),
            Err(e) if e.status == 404 && !channel.starts_with('#') => {
                // Display-name fallback (plan note): the slug is exact, a
                // name may not be. One refresh of the Space's channels.
                let Some(sp) = self.spaces_of(a, false)?.into_iter().find(|s| s.slug == space) else {
                    return Err(e);
                };
                let hits: Vec<Channel> = self
                    .space_channels(a, &sp.id, true)?
                    .into_iter()
                    .filter(|c| c.name.eq_ignore_ascii_case(channel) || c.slug.eq_ignore_ascii_case(channel))
                    .collect();
                match hits.len() {
                    0 => Err(e),
                    1 => Ok(hits.into_iter().next().expect("one hit")),
                    _ => Err(ambiguous(space, &hits)),
                }
            }
            Err(e) => Err(e),
        }
    }

    fn channel(&self, a: &ActAs, id: &str) -> R<Channel> {
        let id = parse_id(id)?;
        let c: PChannel = self.get_as(a, r::with_id(r::CHANNEL, id), vec![])?;
        Ok(self.map_channels(a, vec![c])?.remove(0))
    }

    fn messages(&self, a: &ActAs, channel_id: &str, limit: u32, before: Option<&str>) -> R<Page> {
        let ch = parse_id(channel_id)?;
        let mut q = vec![("limit", limit.clamp(1, 100).to_string())];
        if let Some(b) = before {
            q.push(("before", parse_id(b)?.to_string()));
        }
        let p: PPage<PMessage> = self.get_as(a, r::with_id(r::CHANNEL_MESSAGES, ch), q)?;
        // Sew pages newest-first; K2 shows newest last.
        let mut items = p.items;
        items.reverse();
        Ok(Page { messages: self.map_messages(a, &items)?, has_more: p.has_more })
    }

    fn message(&self, a: &ActAs, id: &str) -> R<Message> {
        let id = parse_id(id)?;
        let p: PPage<PMessage> = self.get_as(a, r::MESSAGES_BY_IDS.to_string(), vec![("ids", id.to_string())])?;
        let m = p.items.into_iter().find(|m| m.id == id).ok_or_else(|| SewError::not_found("message not found"))?;
        Ok(self.map_messages(a, std::slice::from_ref(&m))?.remove(0))
    }

    fn wait(&self, a: &ActAs, channel_id: &str, after: Option<&str>, wait_ms: u64) -> R<Vec<Message>> {
        let ch = parse_id(channel_id)?;
        let path = r::with_id(r::CHANNEL_MESSAGES, ch);
        let raw: Vec<PMessage> = match after {
            Some(x) => {
                let x = parse_id(x)?;
                let w = wait_ms.min(WAIT_MS_MAX);
                let a2 = a.clone();
                let path2 = path.clone();
                let p: PPage<PMessage> = self.run(move |m| {
                    let (a, path) = (a2.clone(), path2.clone());
                    async move {
                        let c = act(&m, &a)?;
                        let q = vec![("after", x.to_string()), ("limit", "100".to_string()), ("wait_ms", w.to_string())];
                        let opts = RequestOptions::new().timeout(Duration::from_millis(w) + sew_client::mgmt::LONG_POLL_SLACK);
                        get_opts(&c, &path, &q, opts).await
                    }
                })?;
                // `after` pages are oldest-first already.
                p.items
            }
            None => {
                let p: PPage<PMessage> = self.get_as(a, path, vec![("limit", "50".to_string())])?;
                let mut v = p.items;
                v.reverse();
                v
            }
        };
        self.map_messages(a, &raw)
    }

    fn open_dm(&self, a: &ActAs, username: &str) -> R<Channel> {
        // SW12: Sew 0.3.0 opens a DM by handle; unknown or hidden = masked 404.
        let h = Handle::new(username).map_err(|_| SewError::not_found("member not found"))?;
        let c: PChannel = self.post_plain(a, r::DMS.to_string(), json!({ "usernames": [h.as_str()] }))?;
        Ok(self.map_channels(a, vec![c])?.remove(0))
    }

    fn create(&self, a: &ActAs, m: &NewMessage) -> R<Message> {
        let ch = parse_id(&m.channel_id)?;
        let mut body = json!({ "body": m.body });
        if let Some(root) = &m.reply_to {
            body["thread_root_id"] = parse_id(root)?.to_string().into();
            if m.broadcast {
                body["also_send_to_channel"] = true.into();
            }
        }
        let pm: PMessage = self.post_write(a, r::with_id(r::CHANNEL_MESSAGES, ch), body, &m.idempotency_key)?;
        Ok(self.map_messages(a, std::slice::from_ref(&pm))?.remove(0))
    }

    fn edit(&self, a: &ActAs, id: &str, body: &str, key: &str) -> R<Message> {
        let id = parse_id(id)?;
        let pm: PMessage = self.post_write(a, r::with_id(r::MESSAGE_EDIT, id), json!({ "body": body }), key)?;
        Ok(self.map_messages(a, std::slice::from_ref(&pm))?.remove(0))
    }

    fn delete(&self, a: &ActAs, id: &str, key: &str) -> R<()> {
        let id = parse_id(id)?;
        let _: PMessage = self.post_write(a, r::with_id(r::MESSAGE_DELETE, id), json!({}), key)?;
        Ok(())
    }

    fn member(&self, a: &ActAs, who: &MemberLookup) -> R<MemberRef> {
        let pm: PMember = match who {
            MemberLookup::Username(u) => {
                let h = Handle::new(u.trim_start_matches('@')).map_err(|_| SewError::not_found("member not found"))?;
                let p: PPage<PMember> =
                    self.get_as(a, r::MEMBERS.to_string(), vec![("username", h.as_str().to_string())])?;
                p.items.into_iter().next().ok_or_else(|| SewError::not_found("member not found"))?
            }
            MemberLookup::External(e) => {
                let ext: ExternalId = e.parse().map_err(|_| SewError::not_found("member not found"))?;
                // The mgmt list finds the member id; the act-as read then
                // applies what the agent may see (hidden = masked 404).
                let page: MgmtPrincipalPage = self.run(move |m| {
                    let ext = ext.to_string();
                    async move { get(m.client(), r::MGMT_PRINCIPALS, &[("external_id", ext)]).await }
                })?;
                let id = page.items.first().map(|p| p.member_id).ok_or_else(|| SewError::not_found("member not found"))?;
                self.get_as(a, r::with_id(r::MEMBER, id), vec![])?
            }
        };
        let m = map_member(&pm);
        self.members
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert((a.external_id(), m.member_id.clone()), (Instant::now(), m.clone()));
        Ok(m)
    }

    fn space_members(&self, a: &ActAs, space_id: &str) -> R<Vec<MemberRef>> {
        let sid = parse_id(space_id)?;
        let a2 = a.clone();
        let rows: Vec<SpaceMember> = self.run(move |m| {
            let a = a2.clone();
            async move {
                let c = act(&m, &a)?;
                let mut out = Vec::new();
                let mut after: Option<String> = None;
                for _ in 0..MAX_PAGES {
                    let mut q = vec![("limit", "100".to_string())];
                    if let Some(x) = &after {
                        q.push(("after", x.clone()));
                    }
                    let p: PPage<SpaceMember> = get(&c, &r::with_id(r::SPACE_MEMBERS, sid), &q).await?;
                    after = p.items.last().map(|x| x.member_id.to_string());
                    out.extend(p.items);
                    if !p.has_more || after.is_none() {
                        break;
                    }
                }
                Ok(out)
            }
        })?;
        let ids: Vec<String> = rows.iter().map(|m| m.member_id.to_string()).collect();
        let by_id = self.members_by_id(a, &ids)?;
        Ok(ids.iter().filter_map(|i| by_id.get(i).cloned()).collect())
    }

    fn space_member_add(&self, a: &ActAs, space_id: &str, member_id: &str) -> R<()> {
        let req = AddSpaceMembersRequest { member_ids: vec![parse_id(member_id)?] };
        let v: Value = self.post_plain(a, r::with_id(r::SPACE_MEMBERS_ADD, parse_id(space_id)?), to_json(&req)?)?;
        // Sew 0.4.0 (bans; HC2): a banned member comes back in `skipped`, not
        // added. Say so instead of reporting a silent success; K2 never
        // retries it (Rosson 2026-10-27). Sew 0.3.x has no such field.
        if skipped_has(&v, member_id) {
            return Err(SewError::new(409, "banned_in_space", "that member is banned in this Space; unban them first"));
        }
        Ok(())
    }

    fn space_member_remove(&self, a: &ActAs, space_id: &str, member_id: &str) -> R<()> {
        let req = RemoveSpaceMemberRequest { member_id: parse_id(member_id)? };
        let _: Value = self.post_plain(a, r::with_id(r::SPACE_MEMBERS_REMOVE, parse_id(space_id)?), to_json(&req)?)?;
        Ok(())
    }

    fn channel_create(&self, a: &ActAs, space_id: &str, name: &str, private: bool, key: &str) -> R<Channel> {
        let sid = parse_id(space_id)?;
        let slug = slugify(name)?;
        let body = json!({
            "slug": slug.as_str(),
            "name": name.trim(),
            "type": "text",
            "visibility": if private { "private" } else { "public" },
        });
        let c: PChannel = self.post_write(a, r::with_id(r::SPACE_CHANNELS, sid), body, key)?;
        self.channels.lock().unwrap_or_else(|p| p.into_inner()).remove(&(a.external_id(), space_id.to_string()));
        Ok(self.map_channels(a, vec![c])?.remove(0))
    }

    fn channel_set_private(&self, _a: &ActAs, _channel_id: &str, _private: bool) -> R<Channel> {
        Err(not_supported(
            "Sew 0.3.0 can't change a channel's visibility after it exists (no `visibility` on \
             POST /api/v1/channels/{id}/update); create a private channel instead",
        ))
    }

    fn roles(&self, a: &ActAs, space_id: &str) -> R<Vec<Role>> {
        let p: PPage<PRole> = self.get_as(a, r::with_id(r::SPACE_ROLES, parse_id(space_id)?), vec![])?;
        Ok(p.items.iter().map(map_role).collect())
    }

    fn role_create(&self, a: &ActAs, space_id: &str, name: &str, key: &str) -> R<Role> {
        let role: PRole =
            self.post_write(a, r::with_id(r::SPACE_ROLES, parse_id(space_id)?), json!({ "name": name.trim() }), key)?;
        Ok(map_role(&role))
    }

    fn role_member(&self, a: &ActAs, space_id: &str, role_id: &str, member_id: &str, add: bool) -> R<()> {
        let tpl = if add { r::SPACE_MEMBER_ROLES_ADD } else { r::SPACE_MEMBER_ROLES_REMOVE };
        let path = r::with_ids(tpl, parse_id(space_id)?, parse_id(member_id)?);
        let _: SpaceMember = self.post_plain(a, path, json!({ "role_ids": [parse_id(role_id)?.to_string()] }))?;
        Ok(())
    }

    fn channel_perms(&self, a: &ActAs, channel_id: &str, o: &Overwrite) -> R<()> {
        let ch = parse_id(channel_id)?;
        let cur: ChannelOverwrites = self.get_as(a, r::with_id(r::CHANNEL_OVERWRITES, ch), vec![])?;
        // `@everyone`'s role id is the Space id (Sew G24).
        let target = match &o.role_id {
            Some(rid) => parse_id(rid)?,
            None => cur.space_id,
        };
        let existing = cur.overwrites.iter().find(|x| x.target_kind == OverwriteTarget::Role && x.target_id == target);
        let (allow, deny) = merge_overwrite(existing, o);
        let req = SetOverwriteRequest { target_kind: OverwriteTarget::Role, target_id: target, allow, deny };
        let _: ChannelOverwrites = self.post_plain(a, r::with_id(r::CHANNEL_OVERWRITES_SET, ch), to_json(&req)?)?;
        Ok(())
    }

    fn ban(&self, _a: &ActAs, _space_id: &str, _member_id: &str, _reason: Option<&str>) -> R<()> {
        Err(not_supported(
            "Sew 0.3.0 has no Space ban (needs a Sew route such as POST /api/v1/spaces/{id}/bans/add); \
             remove the member (space members remove) or time them out instead",
        ))
    }

    fn unban(&self, _a: &ActAs, _space_id: &str, _member_id: &str) -> R<()> {
        Err(not_supported("Sew 0.3.0 has no Space ban, so nothing to lift (needs POST /api/v1/spaces/{id}/bans/remove)"))
    }

    fn timeout(&self, a: &ActAs, space_id: &str, member_id: &str, until: i64, _reason: Option<&str>) -> R<()> {
        let (sid, mid) = (parse_id(space_id)?, parse_id(member_id)?);
        let now_ms = chrono::Utc::now().timestamp_millis();
        let ms = until.saturating_mul(1000).saturating_sub(now_ms);
        let _: SpaceMember = if ms <= 0 {
            self.post_plain(a, r::with_ids(r::SPACE_MEMBER_TIMEOUT_CLEAR, sid, mid), json!({}))?
        } else {
            self.post_plain(a, r::with_ids(r::SPACE_MEMBER_TIMEOUT, sid, mid), json!({ "duration_ms": ms }))?
        };
        Ok(())
    }

    fn space_create(&self, a: &ActAs, n: &NewSpace) -> R<Space> {
        let slug = Slug::new(&n.slug).map_err(|_| usage_err("a Space slug is lowercase a-z, 0-9 and -"))?;
        let owner: ExternalId = n.owner_external_id.parse().map_err(|_| usage_err("the Space owner isn't an external id"))?;
        let manager: ExternalId = a.external_id().parse().map_err(|_| usage_err("the acting agent isn't an external id"))?;
        let req = MgmtSpaceCreateRequest {
            slug,
            name: n.name.trim().to_string(),
            owner,
            managers: vec![manager],
            app_id: None,
            hidden: false,
        };
        let body = to_json(&req)?;
        let key = n.idempotency_key.clone();
        let actor = a.actor.clone();
        let resp: MgmtSpaceResponse = self.run(move |m| {
            let (body, key, actor) = (body.clone(), key.clone(), actor.clone());
            async move { post(m.client(), r::MGMT_SPACES_CREATE, &body, Some(&key), actor.as_deref()).await }
        })?;
        self.spaces.lock().unwrap_or_else(|p| p.into_inner()).remove(&a.external_id());
        Ok(Space { id: resp.space_id.to_string(), slug: resp.slug.as_str().to_string(), name: resp.name })
    }
}

// ── transport helpers ─────────────────────────────────────────────────

/// Whether a members/add answer lists `member_id` in `skipped` (Sew ids
/// may be numbers or strings on the wire).
fn skipped_has(v: &Value, member_id: &str) -> bool {
    v.get("skipped").and_then(Value::as_array).is_some_and(|a| {
        a.iter().any(|x| x.as_str() == Some(member_id) || x.as_u64().is_some_and(|n| n.to_string() == member_id))
    })
}

/// The act-as client for `a` (agent workspace + its cell session).
fn act(m: &MgmtClient, a: &ActAs) -> R<Client> {
    let ext: ExternalId =
        a.external_id().parse().map_err(|_| SewError::new(400, "usage", "the agent's workspace id isn't a uuid"))?;
    let via: SewVia = format!("k2; agent={}; k2_session={}", a.workspace_uuid, a.k2_session)
        .parse()
        .map_err(|e: String| SewError::new(400, "usage", &e))?;
    Ok(m.act_as(ActAsTarget::External(ext), via))
}

async fn get<T: DeserializeOwned>(c: &Client, path: &str, q: &[(&str, String)]) -> R<T> {
    get_opts(c, path, q, RequestOptions::new()).await
}

async fn get_opts<T: DeserializeOwned>(c: &Client, path: &str, q: &[(&str, String)], opts: RequestOptions) -> R<T> {
    let q = (!q.is_empty()).then_some(q);
    c.call::<_, (), T>(reqwest::Method::GET, path, q, None, opts).await.map(|r| r.value).map_err(sew_error)
}

/// A POST. With `actor` it goes out raw so `Sew-Actor` rides along (G7);
/// otherwise through `sew-client`'s pipeline (retries reuse `key`, or a
/// generated one when `None`).
async fn post<T: DeserializeOwned>(c: &Client, path: &str, body: &Value, key: Option<&str>, actor: Option<&str>) -> R<T> {
    match actor.and_then(actor_header) {
        None => {
            let mut opts = RequestOptions::new();
            if let Some(k) = key {
                let k = k.parse().map_err(|e| SewError::new(400, "usage", &format!("{e}")))?;
                opts = opts.idempotency_key(k);
            }
            c.call::<(), _, T>(reqwest::Method::POST, path, None, Some(body), opts).await.map(|r| r.value).map_err(sew_error)
        }
        Some(actor) => {
            let bytes = serde_json::to_vec(body).map_err(|e| SewError::new(500, "internal", &e.to_string()))?;
            let mut h: Vec<(&str, &str)> = vec![("content-type", "application/json"), ("sew-actor", actor.as_str())];
            let generated;
            let key = match key {
                Some(k) => k,
                None => {
                    generated = sew_client::new_idempotency_key();
                    generated.as_str()
                }
            };
            h.push(("idempotency-key", key));
            let raw = c.request("POST", path, &h, Some(bytes)).await.map_err(sew_error)?;
            decode_raw(raw.status, &raw.headers, &raw.body)
        }
    }
}

/// Decode a raw answer: 2xx → `T`; else Sew's error envelope.
pub(crate) fn decode_raw<T: DeserializeOwned>(status: u16, headers: &[(String, String)], body: &[u8]) -> R<T> {
    if (200..300).contains(&status) {
        return serde_json::from_slice(body)
            .map_err(|e| SewError::new(502, "bad_response", &format!("Sew's answer didn't parse: {e}")));
    }
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let Some(err) = v.get("error") else {
        return Err(SewError::new(status, "unexpected_status", &format!("Sew answered {status} without an error body")));
    };
    let details = err.get("details").cloned().unwrap_or(Value::Null);
    let mut e = SewError::new(
        status,
        err.get("code").and_then(Value::as_str).unwrap_or("unknown"),
        err.get("hint").and_then(Value::as_str).unwrap_or(""),
    );
    e.reason = details.get("reason").and_then(Value::as_str).map(str::to_string);
    e.retry_after_ms = details.get("retry_after_ms").and_then(Value::as_u64).or_else(|| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
            .and_then(|(_, v)| v.trim().parse::<u64>().ok())
            .map(|s| s * 1000)
    });
    Err(e)
}

fn to_json<T: Serialize>(v: &T) -> R<Value> {
    serde_json::to_value(v).map_err(|e| SewError::new(500, "internal", &e.to_string()))
}

// ── pure mappings (unit-tested) ───────────────────────────────────────

/// A `sew-client` error → K2's [`SewError`]. Sew's envelope passes through
/// unchanged (a 404 stays Sew's masked 404); transport trouble is a 503.
pub(crate) fn sew_error(e: ClientError) -> SewError {
    match e {
        ClientError::Api(f) => {
            let mut s = SewError::new(f.status, &f.code_str, &f.hint);
            s.reason = f.details.get("reason").and_then(Value::as_str).map(str::to_string);
            s.retry_after_ms = f.retry_after.map(|d| d.as_millis() as u64);
            s
        }
        ClientError::UnexpectedStatus { status, .. } => {
            SewError::new(status, "unexpected_status", &format!("Sew answered {status} without an error body"))
        }
        ClientError::Transport(t) => SewError::new(503, "unavailable", &format!("Sew's mgmt socket: {t}")),
        ClientError::Decode { error, .. } => SewError::new(502, "bad_response", &format!("Sew's answer didn't parse: {error}")),
        ClientError::InvalidInput { field, hint } => SewError::new(400, "usage", &format!("{field}: {hint}")),
        ClientError::Unsupported(w) => SewError::new(500, "unsupported", w),
        ClientError::Gateway(g) => SewError::new(503, "unavailable", &format!("{g}")),
        ClientError::Config(c) => SewError::new(503, "unavailable", &c),
        #[allow(unreachable_patterns)]
        other => SewError::new(503, "unavailable", &format!("{other}")),
    }
}

fn not_supported(hint: &str) -> SewError {
    SewError::new(501, "not_supported", hint)
}

fn usage_err(hint: &str) -> SewError {
    SewError::new(400, "usage", hint)
}

/// A K2 id string as a Sew id; anything else is Sew's masked 404 (an id
/// K2 never got from Sew can't name anything the agent may see).
pub(crate) fn parse_id(s: &str) -> R<Id> {
    s.trim().parse::<Id>().map_err(|_| SewError::not_found("not found"))
}

/// `<space>/<channel>` (channel `#<id>` by id) as Sew's address.
pub(crate) fn build_addr(space: &str, channel: &str) -> R<Addr> {
    let space = match space.strip_prefix('#') {
        Some(id) => SpaceRef::Id(parse_id(id)?),
        None => SpaceRef::Slug(Slug::new(space).map_err(|_| usage_err("a Space slug is lowercase a-z, 0-9 and -"))?),
    };
    let channel = match channel.strip_prefix('#') {
        Some(id) => ChannelRef::Id(parse_id(id)?),
        // A display name that isn't a slug still gets the name fallback:
        // ask Sew for its slug form first.
        None => ChannelRef::Slug(slugify(channel)?),
    };
    Ok(Addr::Channel { space, channel, thread: None })
}

fn ambiguous(space: &str, hits: &[Channel]) -> SewError {
    let list = hits.iter().map(|c| format!("{space}/#{} ({})", c.id, c.name)).collect::<Vec<_>>().join(", ");
    SewError::new(400, "ambiguous", &format!("more than one channel is called that in {space}: {list}"))
}

/// A channel name as a Sew slug: lowercase, runs of anything outside
/// `[a-z0-9]` become one `-`, trimmed.
pub(crate) fn slugify(name: &str) -> R<Slug> {
    let mut s = String::new();
    for ch in name.trim().chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            s.push(ch);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_end_matches('-');
    Slug::new(s).map_err(|_| usage_err("a channel name needs letters or digits (it becomes the channel's slug)"))
}

/// K2's actor label as a `Sew-Actor` value (`[A-Za-z0-9_.:@-]{1,128}`):
/// a space becomes `.`, other characters drop out. `None` if nothing is
/// left.
pub(crate) fn actor_header(label: &str) -> Option<String> {
    let s: String = label
        .trim()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('.'),
            c if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '@' | '-') => Some(c),
            _ => None,
        })
        .take(128)
        .collect();
    (!s.is_empty() && sew_client::proto::mgmt::is_sew_actor(&s)).then_some(s)
}

pub(crate) fn perm_bits(ps: &[ChannelPerm]) -> Perms {
    let mut bits = 0u64;
    for p in ps {
        bits |= match p {
            ChannelPerm::View => Perms::VIEW_CHANNEL.0,
            ChannelPerm::Send => Perms::SEND_MESSAGES.0,
        };
    }
    Perms(bits)
}

/// The new (allow, deny) for a role: K2 owns VIEW_CHANNEL and SEND_MESSAGES
/// only; every other bit of the existing overwrite stays as Sew has it. A
/// bit both allowed and denied is denied.
pub(crate) fn merge_overwrite(existing: Option<&POverwrite>, o: &Overwrite) -> (Perms, Perms) {
    let mask = Perms::VIEW_CHANNEL.0 | Perms::SEND_MESSAGES.0;
    let (old_allow, old_deny) = existing.map(|e| (e.allow.0, e.deny.0)).unwrap_or((0, 0));
    let deny = (old_deny & !mask) | perm_bits(&o.deny).0;
    let allow = ((old_allow & !mask) | perm_bits(&o.allow).0) & !perm_bits(&o.deny).0;
    (Perms(allow), Perms(deny))
}

pub(crate) fn map_space(s: &PSpace) -> Space {
    Space { id: s.id.to_string(), slug: s.slug.as_str().to_string(), name: s.name.clone() }
}

pub(crate) fn map_role(r: &PRole) -> Role {
    Role { id: r.id.to_string(), name: if r.is_everyone { "everyone".to_string() } else { r.name.clone() } }
}

pub(crate) fn map_member(m: &PMember) -> MemberRef {
    MemberRef { member_id: m.id.to_string(), username: m.username.as_str().to_string(), is_agent: m.kind == MemberKind::Agent }
}

pub(crate) fn map_channel(c: &PChannel, space_slug: Option<String>) -> Channel {
    let kind = match c.kind {
        ChannelType::Dm => ChannelKind::Dm,
        ChannelType::GroupDm => ChannelKind::GroupDm,
        _ => ChannelKind::Text,
    };
    let can_send = match c.my_perms {
        Some(p) => p.contains(Perms::SEND_MESSAGES),
        // Sew leaves `my_perms` off DMs: a member of a DM may post in it.
        None => kind.is_dm(),
    };
    Channel {
        id: c.id.to_string(),
        space_id: c.space_id.map(|s| s.to_string()),
        space_slug: if c.space_id.is_some() { space_slug } else { None },
        slug: c.slug.as_ref().map(|s| s.as_str().to_string()).unwrap_or_else(|| c.id.to_string()),
        name: c.name.clone().unwrap_or_else(|| kind.as_str().to_string()),
        kind,
        private: c.visibility == Visibility::Private,
        archived: c.archived_at.is_some(),
        can_send,
        dm_member_ids: c.dm_member_ids.iter().map(Id::to_string).collect(),
    }
}

/// A Sew message as K2's. Authors not in `authors` (hidden from the agent)
/// show as `unknown`. KM1 fields stay `None` (not on Sew 0.3.0's wire).
pub(crate) fn map_message(m: &PMessage, authors: &HashMap<String, MemberRef>) -> Message {
    let author = authors.get(&m.author_id.to_string());
    Message {
        id: m.id.to_string(),
        channel_id: m.channel_id.to_string(),
        author_id: m.author_id.to_string(),
        author_username: author.map(|a| a.username.clone()).unwrap_or_else(|| "unknown".to_string()),
        author_is_agent: author.is_some_and(|a| a.is_agent),
        on_behalf_of: m.on_behalf_of.map(|i| i.to_string()),
        body: if m.deleted_at.is_some() { String::new() } else { m.body.clone() },
        created_at: m.created_at.unix_ms() / 1000,
        edited_at: m.edited_at.map(|t| t.unix_ms() / 1000),
        deleted: m.deleted_at.is_some(),
        reply_to: m.thread_root_id.map(|i| i.to_string()),
        reply_count: m.thread.as_ref().map(|t| i64::from(t.reply_count)).unwrap_or(0),
        following: m.thread.as_ref().is_some_and(|t| t.following),
        mentions_me: m.mentions_me,
        message_kind: None,
        wake: None,
        response: None,
    }
}

// ── install with the Sew lifecycle ────────────────────────────────────

/// Install the real backend over `socket` with tokens from the root helper
/// (Sew chat on, K2 identity). Linux only: on a Mac nothing installs and
/// every chat route stays 503 `chat_unavailable`. Idempotent.
pub fn install_live(socket: &str) -> Result<(), String> {
    if !crate::sew::sew_supported() {
        return Err("Sew runs on Linux only".into());
    }
    let tokens: TokenSource =
        Arc::new(|refresh| crate::sew::supervisor::mgmt_token(&crate::sew::supervisor::live(), refresh));
    let b = SewClientChat::new(socket, tokens)?;
    super::sew::install(Arc::new(b));
    Ok(())
}

#[cfg(test)]
mod tests;
