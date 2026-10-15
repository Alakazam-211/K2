//! The Sew seam: [`SewChat`] is every call `k2 chat` makes to Sew, always
//! AS the agent (act-as by external id `k2:workspace:<uuid>`, Sew S9 §8.1).
//!
//! **Real-client seam (K15).** Sew's `sew-client` crate isn't vendorable
//! yet (SW10). When it is, K15 writes `struct SewClientChat` implementing
//! this trait over `MgmtClient::act_as(ActAsTarget::External(..), SewVia {
//! agent, k2_session })` (plus `Sew-Actor` for an approval, G7, and the
//! `wait_ms` long-poll, G3), caches `me` per workspace, and calls
//! [`install`] once the Sew target is enabled. Until then no backend is
//! installed and every chat route answers 503 `chat_unavailable`. The
//! route layer depends only on this trait; tests install a recording fake
//! (`sew_chat::fake`) through [`test_seam`].
//!
//! The trait is blocking: chat handlers run on the blocking pool like the
//! other `/cli/*` handlers. An async client bridges with
//! `tokio::runtime::Handle::block_on` from inside the trait impl.

use std::sync::{Arc, RwLock};

/// Who Sew acts as, and on whose request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActAs {
    /// The agent workspace (lowercase uuid; never from a request body).
    pub workspace_uuid: String,
    /// The agent's cell session (`Sew-Via: k2; agent=…; k2_session=…`).
    pub k2_session: String,
    /// `--as <user>`: posting on behalf of the person the agent assists
    /// (Sew enforces its assistant rules).
    pub on_behalf_of: Option<String>,
    /// `Sew-Actor` (G7): who approved a queued post and how, e.g.
    /// `person:owner via local-auth` or `agent:ops`.
    pub actor: Option<String>,
}

impl ActAs {
    /// The external id Sew acts as.
    pub fn external_id(&self) -> String {
        format!("k2:workspace:{}", self.workspace_uuid)
    }
}

/// A Sew error, as Sew answered it. A 404 is Sew's masked "not found" and
/// passes through unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SewError {
    pub status: u16,
    pub code: String,
    pub hint: String,
    /// Sew's `details.reason` (for 403 missing_permission / 422).
    pub reason: Option<String>,
    pub retry_after_ms: Option<u64>,
}

impl SewError {
    pub fn new(status: u16, code: &str, hint: &str) -> Self {
        Self { status, code: code.into(), hint: hint.into(), reason: None, retry_after_ms: None }
    }

    /// Sew's masked not-found.
    pub fn not_found(hint: &str) -> Self {
        Self::new(404, "not_found", hint)
    }
}

/// The acting agent's Sew member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Me {
    pub member_id: String,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Space {
    pub id: String,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Text,
    Dm,
    GroupDm,
}

impl ChannelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelKind::Text => "text",
            ChannelKind::Dm => "dm",
            ChannelKind::GroupDm => "group_dm",
        }
    }

    pub fn is_dm(self) -> bool {
        !matches!(self, ChannelKind::Text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub id: String,
    /// `None` for a DM outside every Space (governed by the default Space).
    pub space_id: Option<String>,
    pub space_slug: Option<String>,
    pub slug: String,
    pub name: String,
    pub kind: ChannelKind,
    pub private: bool,
    pub archived: bool,
    /// Sew's `my_perms` SEND_MESSAGES for the agent.
    pub can_send: bool,
    pub dm_member_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// Sew's id (K2 shows it as `msg_<id>`, see [`wire_msg_id`]).
    pub id: String,
    pub channel_id: String,
    pub author_id: String,
    pub author_username: String,
    pub author_is_agent: bool,
    pub on_behalf_of: Option<String>,
    pub body: String,
    pub created_at: i64,
    pub edited_at: Option<i64>,
    pub deleted: bool,
    /// The thread root when this is a reply.
    pub reply_to: Option<String>,
    pub reply_count: i64,
    /// Per-viewer: the agent follows this thread.
    pub following: bool,
    /// Per-viewer `mentions_me` (direct, role or group; never @here) (G6).
    pub mentions_me: bool,
    /// KM1, optional: absent = a plain text message. Never keyed on.
    pub message_kind: Option<String>,
    pub wake: Option<bool>,
    pub response: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub messages: Vec<Message>,
    pub has_more: bool,
}

/// A post to create (send, reply, dm).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    pub channel_id: String,
    pub body: String,
    /// Reply under this root.
    pub reply_to: Option<String>,
    /// `reply --broadcast`: also show it in the channel.
    pub broadcast: bool,
    /// `Idempotency-Key` (`k2c-<client key>`).
    pub idempotency_key: String,
}

/// Every Sew call `k2 chat` makes. All act as `a`.
pub trait SewChat: Send + Sync {
    /// `GET /api/v1/me`.
    fn me(&self, a: &ActAs) -> Result<Me, SewError>;
    /// `GET /spaces?scope=mine`.
    fn spaces(&self, a: &ActAs) -> Result<Vec<Space>, SewError>;
    /// The box's default Space (K2's own; DMs are governed by its access).
    fn default_space(&self, a: &ActAs) -> Result<Option<Space>, SewError>;
    /// Channels the agent can see (`space_id` None = every Space and DMs).
    fn channels(&self, a: &ActAs, space_id: Option<&str>) -> Result<Vec<Channel>, SewError>;
    /// `<space-slug>/<channel-slug>` (or `#<id>`), Sew-resolved; hidden =
    /// masked 404.
    fn resolve(&self, a: &ActAs, space: &str, channel: &str) -> Result<Channel, SewError>;
    fn channel(&self, a: &ActAs, id: &str) -> Result<Channel, SewError>;
    /// Newest last; `before` pages back.
    fn messages(&self, a: &ActAs, channel_id: &str, limit: u32, before: Option<&str>) -> Result<Page, SewError>;
    fn message(&self, a: &ActAs, id: &str) -> Result<Message, SewError>;
    /// Messages after `after` (oldest first), long-polling up to `wait_ms`
    /// (Sew G3). Empty = nothing new in that window.
    fn wait(&self, a: &ActAs, channel_id: &str, after: Option<&str>, wait_ms: u64) -> Result<Vec<Message>, SewError>;
    /// Open (or find) the DM with a K2 username. Until SW12 Sew takes
    /// member ids only, so the real client returns Sew's masked 404 for a
    /// DM it can't find (G5).
    fn open_dm(&self, a: &ActAs, username: &str) -> Result<Channel, SewError>;
    fn create(&self, a: &ActAs, m: &NewMessage) -> Result<Message, SewError>;
    fn edit(&self, a: &ActAs, id: &str, body: &str, idempotency_key: &str) -> Result<Message, SewError>;
    fn delete(&self, a: &ActAs, id: &str, idempotency_key: &str) -> Result<(), SewError>;
}

/// `msg_<sew id>` (the CLI's id form).
pub fn wire_msg_id(sew_id: &str) -> String {
    if sew_id.starts_with("msg_") {
        sew_id.to_string()
    } else {
        format!("msg_{sew_id}")
    }
}

/// The Sew id behind a `msg_…` (a bare id is accepted too).
pub fn sew_msg_id(wire: &str) -> Option<String> {
    let w = wire.trim();
    let id = w.strip_prefix("msg_").unwrap_or(w);
    (!id.is_empty() && !id.contains('/') && !id.contains(char::is_whitespace)).then(|| id.to_string())
}

/// A `<space>/<channel>` address. `#<id>` names a channel by id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addr {
    pub space: String,
    pub channel: String,
}

impl Addr {
    pub fn parse(s: &str) -> Result<Self, String> {
        let t = s.trim();
        let (space, channel) = t
            .split_once('/')
            .ok_or_else(|| format!("'{t}' isn't an address: use <space>/<channel>, e.g. hearth/general"))?;
        if space.is_empty() || channel.is_empty() || channel.contains('/') || t.contains(char::is_whitespace) {
            return Err(format!("'{t}' isn't an address: use <space>/<channel>, e.g. hearth/general"));
        }
        Ok(Addr { space: space.to_string(), channel: channel.to_string() })
    }
}

/// The address K2 shows for a channel.
pub fn channel_addr(c: &Channel) -> String {
    match (&c.space_slug, c.kind.is_dm()) {
        (Some(s), false) => format!("{s}/{}", c.slug),
        _ => format!("dm/{}", c.slug),
    }
}

// ── backend registry ─────────────────────────────────────────────────

static BACKEND: RwLock<Option<Arc<dyn SewChat>>> = RwLock::new(None);

/// Install the real Sew client (K15, once the Sew target is enabled).
pub fn install(b: Arc<dyn SewChat>) {
    *BACKEND.write().expect("sew backend lock") = Some(b);
}

/// Remove it (Sew disabled or uninstalled): routes answer 503 again.
pub fn uninstall() {
    *BACKEND.write().expect("sew backend lock") = None;
}

/// The installed backend, if Sew chat is on.
pub(crate) fn backend() -> Option<Arc<dyn SewChat>> {
    #[cfg(test)]
    if let Some(b) = test_seam::get() {
        return Some(b);
    }
    BACKEND.read().expect("sew backend lock").clone()
}

#[cfg(test)]
pub(crate) mod test_seam {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static B: RefCell<Option<Arc<dyn SewChat>>> = const { RefCell::new(None) };
    }

    pub(crate) fn set(b: Option<Arc<dyn SewChat>>) {
        B.with(|c| *c.borrow_mut() = b);
    }

    pub(super) fn get() -> Option<Arc<dyn SewChat>> {
        B.with(|c| c.borrow().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_addresses() {
        assert_eq!(wire_msg_id("123"), "msg_123");
        assert_eq!(wire_msg_id("msg_9"), "msg_9");
        assert_eq!(sew_msg_id("msg_123").as_deref(), Some("123"));
        assert_eq!(sew_msg_id("123").as_deref(), Some("123"));
        assert_eq!(sew_msg_id("msg_"), None);
        assert_eq!(sew_msg_id("a/b"), None);
        assert_eq!(Addr::parse("hearth/general").unwrap(), Addr { space: "hearth".into(), channel: "general".into() });
        assert_eq!(Addr::parse("hearth/#42").unwrap().channel, "#42");
        for bad in ["hearth", "/general", "hearth/", "a/b/c", "hearth/gen eral"] {
            assert!(Addr::parse(bad).is_err(), "{bad}");
        }
        let a = ActAs { workspace_uuid: "u-1".into(), k2_session: "s".into(), on_behalf_of: None, actor: None };
        assert_eq!(a.external_id(), "k2:workspace:u-1");
    }
}
