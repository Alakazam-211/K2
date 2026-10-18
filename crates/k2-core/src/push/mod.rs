//! Pluggable push-notification adapter.
//!
//! The daemon wants to *fire notifications* when an agent needs attention,
//! a heartbeat fails, or another long-running workflow crosses a user-visible
//! boundary. The daemon does *not* want to know how those notifications
//! reach the user's phone. That decision is pushed out to a `PushTarget`
//! implementation chosen by user settings.
//!
//! ### Shipped implementations (v1, 0.33.0)
//!
//! - [`NoOp`]   — default. Silently drops events. Users who haven't opted in.
//! - [`Webhook`]  — `POST` the event as JSON to a user-supplied URL.
//!   Power users route to Slack incoming-webhooks, Discord, their own ntfy
//!   server, Apple Shortcuts HTTP, etc.
//! - [`NtfySh`] — convenience wrapper for <https://ntfy.sh>. Same shape as
//!   `Webhook` but with the topic / server baked in and the payload format
//!   matched to ntfy's expected fields.
//!
//! ### Companion C4 implementation (the reserved cloud seam, filled)
//!
//! - [`K2Cloud`] (naming: k2, not k2so) — POSTs the daemon-held device
//!   registry ([`crate::push_devices`]) + a content-free payload to the
//!   relay's push gateway (`POST <gateway>/push/dispatch`), which fans
//!   out to APNs/FCM so push reaches locked iOS/Android screens.
//!   Config = gateway URL + shared gateway token
//!   (`K2_PUSH_GATEWAY_URL`/`K2_PUSH_GATEWAY_TOKEN` env, falling back
//!   to the `pushGatewayUrl`/`pushGatewayToken` app settings). When
//!   unset, [`K2Cloud::from_config`] returns `None` and the impl is
//!   never constructed — dispatch is a startup no-op: **DORMANT**.
//!   Payloads are deliberately dumb (§4.5 invariant): generic title +
//!   attribution line + IDs ONLY for tickets and project chat. The one
//!   exception is a Thread push's message PREVIEW (0.46.1, see
//!   [`preview`]): opt-in per device, owner master switch, never for a
//!   secret card. A preview passes through K2's relay and Apple/Google.
//!
//! ### Design constraints (ratified in the persistent-agents PRD)
//!
//! - **No cloud dependency in v1.** None of the shipped impls phone home.
//! - **Fire-and-forget semantics.** `send` returns on send/attempt
//!   completion; the daemon does not queue for retry on behalf of the
//!   adapter. Individual impls may internally retry with backoff before
//!   returning an error, but the daemon never stores undelivered events.
//! - **Thread-safe.** `PushTarget: Send + Sync` so a daemon-wide instance
//!   can be wrapped in `Arc` and shared across tokio tasks.

use std::fmt;

pub mod budget;
pub mod preview;

/// Events the daemon emits to the user's configured push target.
#[derive(Debug, Clone)]
pub enum PushEvent {
    /// An agent hit a decision point and wants human attention.
    AgentNeedsAttention {
        agent: String,
        summary: String,
        /// Deep link back into the Tauri app / mobile companion, e.g.
        /// `k2so://agents/<id>` or the ngrok tunnel's `/agents/<id>`
        /// path.
        action_url: String,
    },
    /// A scheduled heartbeat didn't complete successfully.
    HeartbeatFailed {
        heartbeat: String,
        reason: String,
    },
    /// Companion C4: an agent filed a ticket (`feedback:created`).
    /// CONTENT-FREE by construction (§4.5 invariant): asking agent name
    /// + ticket id ONLY — never the ask's title/body. Optional
    /// `assignees` narrows push to those usernames' devices (empty =
    /// fan out to everyone registered on this server).
    FeedbackCreated {
        agent_name: String,
        feedback_id: String,
        /// Username snapshots from `feedback_assignees` (may be empty).
        assignees: Vec<String>,
    },
    /// New comment on a ticket (thread activity). CONTENT-FREE; same
    /// assignee targeting as [`Self::FeedbackCreated`].
    FeedbackCommented {
        feedback_id: String,
        assignees: Vec<String>,
    },
    /// Companion C4: a non-owner author posted into a project-group
    /// chat (`project-group:message-created`, author != "owner").
    /// CONTENT-FREE: project name + group id ONLY — never the message
    /// text.
    ProjectMessageCreated {
        project_name: String,
        group_id: String,
    },
    /// thread-read-v1 (0.46.1): a project chat line for ONE person (never
    /// its author, never a person who muted the chat), with their badge
    /// numbers. Content-free like [`Self::ProjectMessageCreated`].
    ProjectPost(ProjectPush),
    /// thread-read-v1 (0.46.1): a SILENT push that corrects one person's
    /// app badge after they read, muted or answered on another device.
    /// No title, body or sound: the relay sends it as an APNs background
    /// push (`content-available`) / FCM data message.
    Badge(BadgePush),
    /// Thread push (prd-thread-push-notifications-v1 §7): an agent posted
    /// in a Thread. The title is the agent's name. The body is a preview
    /// of the message (0.46.1, [`ThreadPush::preview`]) when there is one,
    /// else a fixed line by message kind or count. A secret card's name
    /// and prompt never ride a push.
    ThreadPost(ThreadPush),
}

/// One person's project chat push (thread-read-v1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPush {
    pub project_name: String,
    pub group_id: String,
    /// The recipient person key (TP29).
    pub principal: String,
    /// That person's device logins. Empty = nobody, never "everyone".
    pub device_usernames: Vec<String>,
    /// `data.unreadTotal`: the person's badge total on this server.
    pub unread_total: String,
    /// `data.waitingTotal`: cards waiting on the person.
    pub waiting_total: String,
}

/// One person's silent badge correction (thread-read-v1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadgePush {
    pub principal: String,
    pub device_usernames: Vec<String>,
    pub unread_total: String,
    pub waiting_total: String,
}

/// `data.badgeKey`: a stable, opaque key per (server, person). Every
/// mobile push carries it INSTEAD of the person's name (a username must
/// never reach Apple or Google); the phone matches it to one of its
/// logins (`badgeKey` comes back on register-device, `GET
/// /cli/push/device` and `GET /cli/thread/unread`), and the relay uses it
/// as a silent badge push's collapse id.
///
/// The first 32 hex chars of `sha256("k2-badge\0" + server_salt + "\0" +
/// person_key)`. `server_salt` is this server's random badge salt
/// (`read_server.badge_salt`, minted once by migration 0190, never shown):
/// it makes the key stable across restarts, upgrades, token refreshes,
/// re-logins and device re-registration, and keeps it from being guessed
/// from a username. `person_key` is [`crate::notify_prefs::People::person_key`]
/// (the owner token and every linked login are `owner`). See
/// [`crate::read_state::badge_key_for`].
pub fn badge_key(server_salt: &str, person_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"k2-badge\0");
    h.update(server_salt.trim().as_bytes());
    h.update(b"\0");
    h.update(person_key.trim().as_bytes());
    let digest = h.finalize();
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// The kind of Thread post a push announces (`data.msgKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadMsgKind {
    /// A text reply.
    Text,
    /// A choice card (`k2 thread ask`).
    Choice,
    /// A secret card (`k2 thread secret`). Never carries more than this
    /// payload, even once snippets exist (§7).
    Secret,
}

impl ThreadMsgKind {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Choice => "choice",
            Self::Secret => "secret",
        }
    }
}

/// One Thread push, already coalesced (§6.3): the same payload goes to
/// every recipient in ONE gateway call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadPush {
    /// The posting agent's display name (the notification title).
    pub agent_name: String,
    pub conversation_id: String,
    /// The address the conversation answers to now.
    pub addr: String,
    /// The seq of the LAST post this push covers.
    pub seq: i64,
    pub project_id: String,
    /// The kind of the last post (used for the body when `count == 1`).
    pub msg_kind: ThreadMsgKind,
    /// How many posts this push covers (at least 1).
    pub count: u32,
    /// The ONE recipient person key this push is for (TP29: `owner` or a
    /// Connect username). The phone can hold two accounts on one host, so
    /// every dispatch is per (Thread, person) and says whose it is.
    pub principal: String,
    /// That person's `push_devices.username` values (their logins).
    /// Empty = nobody, never "everyone".
    pub device_usernames: Vec<String>,
    /// `data.unreadTotal` (thread-read-v1): the recipient's badge total on
    /// this server at dispatch (unmuted unread + every waiting card), a
    /// decimal string. The PHONE sets the icon badge (summing servers);
    /// the server never sends `aps.badge` (TP27).
    pub unread_total: String,
    /// `data.waitingTotal`: cards waiting on the recipient.
    pub waiting_total: String,
    /// `data.avatarRev`: [`crate::projects_ops::project_icon_rev`] of the
    /// workspace (`projectId`) — the same value its `GET
    /// /cli/projects/list` row carries as `iconRev`. `""` = no avatar
    /// (the key is always sent).
    pub avatar_rev: String,
    /// `data.agentColor`: the workspace's `projects.color` (`#rrggbb`) as
    /// the projects list shows it; `""` if the row is gone.
    pub agent_color: String,
    /// The latest post's message preview ([`preview::message_preview`]),
    /// or `""` for the generic body. Already cleared by the daemon when
    /// the owner's `pushMessagePreviews` switch is off; ignored for a
    /// secret card whatever it holds. [`K2Cloud::dispatch`] sends it only
    /// to devices with `previews` on and the generic body to the rest.
    ///
    /// PRIVACY: a non-empty preview is message text that leaves this
    /// server through K2's push relay and Apple (APNs) or Google (FCM).
    pub preview: String,
}

impl ThreadPush {
    /// The preview this push would show (`None` for a secret card or an
    /// empty preview).
    pub fn shown_preview(&self) -> Option<&str> {
        if self.msg_kind == ThreadMsgKind::Secret || self.preview.trim().is_empty() {
            None
        } else {
            Some(self.preview.as_str())
        }
    }
}

/// A push's `data` object: string keys and **string values only** (TP7 —
/// the companion's iOS plugin drops anything else). The type makes a
/// non-string value impossible; numbers are sent as decimal strings.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(transparent)]
pub struct PushData(pub std::collections::BTreeMap<String, String>);

impl PushData {
    fn of(pairs: &[(&str, String)]) -> Self {
        Self(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn insert(&mut self, key: &str, value: impl Into<String>) {
        self.0.insert(key.to_string(), value.into());
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::Value::Object(
            self.0
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect(),
        )
    }
}

impl PartialEq<serde_json::Value> for PushData {
    fn eq(&self, other: &serde_json::Value) -> bool {
        &self.to_value() == other
    }
}

/// Who is sending, stamped into every mobile push's `data` (TP8, TP25):
/// the tunnel subdomain (omitted when the daemon has none) and the
/// daemon's `/boot-status` instance id. The phone matches a server on
/// `instanceId` first, then `subdomain`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushStamp {
    pub subdomain: String,
    pub instance_id: String,
}

static INSTANCE_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The daemon registers its `/boot-status` instance id once at boot; every
/// mobile push carries it as `data.instanceId`.
pub fn set_instance_id(id: &str) {
    let _ = INSTANCE_ID.set(id.to_string());
}

/// The registered instance id, or empty before the daemon set it.
pub fn instance_id() -> String {
    INSTANCE_ID.get().cloned().unwrap_or_default()
}

/// The `sound` every mobile push carries (TP3): the platform default.
pub const PUSH_SOUND_DEFAULT: &str = "default";

/// APNs `apns-collapse-id` limit (bytes).
pub const COLLAPSE_ID_MAX: usize = 64;

impl PushEvent {
    /// Short one-line title suitable for a mobile notification header.
    pub fn title(&self) -> String {
        match self {
            Self::AgentNeedsAttention { agent, .. } => {
                format!("{agent} needs your attention")
            }
            Self::HeartbeatFailed { heartbeat, .. } => {
                format!("Heartbeat failed: {heartbeat}")
            }
            // C4 mobile pushes use the generic app title; the body
            // carries the (still content-free) attribution line.
            Self::FeedbackCreated { .. }
            | Self::FeedbackCommented { .. }
            | Self::ProjectMessageCreated { .. }
            | Self::ProjectPost(_) => "K2".to_string(),
            // Silent: no visible text.
            Self::Badge(_) => String::new(),
            // §7: the agent's display name; name-only.
            Self::ThreadPost(t) => t.agent_name.clone(),
        }
    }

    /// Body text for the notification.
    pub fn body(&self) -> String {
        match self {
            Self::AgentNeedsAttention { summary, .. } => summary.clone(),
            Self::HeartbeatFailed { reason, .. } => reason.clone(),
            Self::FeedbackCreated { agent_name, .. } => {
                format!("{agent_name} needs you on a ticket")
            }
            Self::FeedbackCommented { .. } => "New reply on a ticket".to_string(),
            Self::ProjectMessageCreated { project_name, .. }
            | Self::ProjectPost(ProjectPush { project_name, .. }) => {
                format!("New message in {project_name}")
            }
            Self::Badge(_) => String::new(),
            Self::ThreadPost(t) => thread_body(t.msg_kind, t.count, t.shown_preview()),
        }
    }

    /// Tappable URL if the push surface supports one, else empty.
    pub fn action_url(&self) -> &str {
        match self {
            Self::AgentNeedsAttention { action_url, .. } => action_url,
            Self::HeartbeatFailed { .. }
            | Self::FeedbackCreated { .. }
            | Self::FeedbackCommented { .. }
            | Self::ProjectMessageCreated { .. }
            | Self::ProjectPost(_)
            | Self::Badge(_)
            | Self::ThreadPost(_) => "",
        }
    }

    /// When `Some(non-empty)`, only devices for these usernames receive
    /// the push. `None` / empty → all registered devices on this host.
    ///
    /// A Thread push is ALWAYS targeted: its (possibly empty) recipient
    /// list is never widened to every device.
    pub fn target_usernames(&self) -> Option<&[String]> {
        match self {
            Self::FeedbackCreated { assignees, .. } | Self::FeedbackCommented { assignees, .. }
                if !assignees.is_empty() =>
            {
                Some(assignees.as_slice())
            }
            Self::ThreadPost(t) => Some(t.device_usernames.as_slice()),
            Self::ProjectPost(p) => Some(p.device_usernames.as_slice()),
            Self::Badge(b) => Some(b.device_usernames.as_slice()),
            _ => None,
        }
    }

    /// `true` for the kinds the mobile companion routes (ticket, project,
    /// thread): they carry `sound` and the server stamp.
    pub fn is_mobile(&self) -> bool {
        !matches!(self, Self::AgentNeedsAttention { .. } | Self::HeartbeatFailed { .. })
    }

    /// True when this push shows message text (a Thread preview).
    pub fn has_preview(&self) -> bool {
        matches!(self, Self::ThreadPost(t) if t.shown_preview().is_some())
    }

    /// The same push without message text: the generic body for devices
    /// with previews off. Other kinds are returned unchanged.
    pub fn without_preview(&self) -> PushEvent {
        match self {
            Self::ThreadPost(t) => Self::ThreadPost(ThreadPush { preview: String::new(), ..t.clone() }),
            other => other.clone(),
        }
    }

    /// The local budget a send of this event draws from (TP2).
    pub fn budget_class(&self) -> budget::BudgetClass {
        match self {
            Self::ThreadPost(_) => budget::BudgetClass::Thread,
            Self::Badge(_) => budget::BudgetClass::Badge,
            _ => budget::BudgetClass::Other,
        }
    }

    /// `aps.thread-id` / grouping key (§7): `thread:<conversationId>`.
    pub fn thread_id(&self) -> Option<String> {
        match self {
            Self::ThreadPost(t) => Some(format!("thread:{}", t.conversation_id)),
            _ => None,
        }
    }

    /// `apns-collapse-id` / FCM `tag` (§7): a trailing push REPLACES the
    /// earlier banner. Always at most [`COLLAPSE_ID_MAX`] bytes: an
    /// over-long conversation id is hashed instead of cut (a cut could
    /// make two Threads collide).
    pub fn collapse_id(&self) -> Option<String> {
        let id = self.thread_id()?;
        if id.len() <= COLLAPSE_ID_MAX {
            return Some(id);
        }
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(id.as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        Some(format!("thread:{}", &hex[..COLLAPSE_ID_MAX - "thread:".len()]))
    }

    /// The mobile deep-link `data` object ([`K2Cloud`] payloads): IDs
    /// + a `kind` discriminator ([`PUSH_KIND_TICKET`] / [`PUSH_KIND_PROJECT`]) ONLY — tap-to-open navigates on these,
    /// content is pulled in-app (feedback PRD §8.6).
    ///
    /// Strings only (TP7). The server stamp (`subdomain`, `instanceId`) is
    /// added at send time by [`K2Cloud`], not here.
    pub fn data(&self) -> PushData {
        match self {
            Self::FeedbackCreated { feedback_id, .. }
            | Self::FeedbackCommented { feedback_id, .. } => PushData::of(&[
                ("kind", PUSH_KIND_TICKET.to_string()),
                ("feedbackId", feedback_id.clone()),
            ]),
            Self::ProjectMessageCreated { group_id, .. } => PushData::of(&[
                ("kind", PUSH_KIND_PROJECT.to_string()),
                ("groupId", group_id.clone()),
            ]),
            Self::ProjectPost(p) => PushData::of(&[
                ("kind", PUSH_KIND_PROJECT.to_string()),
                ("groupId", p.group_id.clone()),
                ("unreadTotal", p.unread_total.clone()),
                ("waitingTotal", p.waiting_total.clone()),
            ]),
            Self::Badge(b) => PushData::of(&[
                ("kind", PUSH_KIND_BADGE.to_string()),
                ("unreadTotal", b.unread_total.clone()),
                ("waitingTotal", b.waiting_total.clone()),
            ]),
            Self::ThreadPost(t) => PushData::of(&[
                ("kind", PUSH_KIND_THREAD.to_string()),
                ("addr", t.addr.clone()),
                ("conversationId", t.conversation_id.clone()),
                ("seq", t.seq.to_string()),
                ("projectId", t.project_id.clone()),
                ("agentName", t.agent_name.clone()),
                ("msgKind", t.msg_kind.as_wire().to_string()),
                ("count", t.count.to_string()),
                ("unreadTotal", t.unread_total.clone()),
                ("waitingTotal", t.waiting_total.clone()),
                // iOS Communication Notifications: the phone's Notification
                // Service Extension draws the agent's avatar (cached by
                // rev, compared with the list's `iconRev`) and colour.
                ("avatarRev", t.avatar_rev.clone()),
                ("agentColor", t.agent_color.clone()),
            ]),
            // Legacy desktop-era events carry no mobile deep link.
            Self::AgentNeedsAttention { .. } | Self::HeartbeatFailed { .. } => PushData::default(),
        }
    }
}

/// The Thread push body (§7, previews 0.46.1).
///
/// - One post with a preview: the preview (a choice card's question).
/// - Several posts: `"N new messages"`, plus `" · <preview of the latest>"`
///   when the latest has one, the whole body within
///   [`preview::PREVIEW_MAX_CHARS`].
/// - No preview (previews off, empty after stripping): the fixed line by
///   kind. A secret card NEVER shows a preview, whatever is passed.
pub fn thread_body(kind: ThreadMsgKind, count: u32, preview: Option<&str>) -> String {
    let preview = match kind {
        ThreadMsgKind::Secret => None,
        _ => preview.map(str::trim).filter(|p| !p.is_empty()),
    };
    if count > 1 {
        let head = format!("{count} new messages");
        return match preview {
            Some(p) => {
                let sep = " · ";
                let room = preview::PREVIEW_MAX_CHARS
                    .saturating_sub(head.chars().count() + sep.chars().count());
                let p = preview::cap_chars(p, room);
                if p.is_empty() {
                    head
                } else {
                    format!("{head}{sep}{p}")
                }
            }
            None => head,
        };
    }
    if let Some(p) = preview {
        return preview::cap_chars(p, preview::PREVIEW_MAX_CHARS);
    }
    match kind {
        ThreadMsgKind::Text => "New message in Thread".to_string(),
        ThreadMsgKind::Choice => "Asks you to choose".to_string(),
        ThreadMsgKind::Secret => "Asks for a secret".to_string(),
    }
}

/// Push `data.kind` for a ticket event (created / commented). The wire
/// value is `"ticket"` since the 2026-07-23 Tickets rebrand (`ec845928`);
/// the mobile companion routes on it. Never `"feedback"`.
pub const PUSH_KIND_TICKET: &str = "ticket";

/// Push `data.kind` for a project-group chat message.
pub const PUSH_KIND_PROJECT: &str = "project";

/// Push `data.kind` for an agent's Thread post (prd-thread-push-notifications-v1).
pub const PUSH_KIND_THREAD: &str = "thread";

/// Push `data.kind` for a silent badge correction (thread-read-v1).
pub const PUSH_KIND_BADGE: &str = "badge";

/// What happens when a push attempt fails. Adapters return `Err` after
/// exhausting their own retry policy; the daemon does not queue events for
/// later retry. This is fire-and-forget by design — the source of truth is
/// the workspace DB + foreground companion WS, not the push channel.
#[derive(Debug)]
pub enum PushError {
    /// Network / HTTP transport error (DNS, TCP, TLS, timeout).
    Transport(String),
    /// Target returned a non-2xx status.
    Remote { status: u16, body: String },
    /// Adapter rejected the event before sending (e.g. missing config).
    BadConfig(String),
}

impl fmt::Display for PushError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(msg) => write!(f, "push transport error: {msg}"),
            Self::Remote { status, body } => {
                write!(f, "push remote error: HTTP {status}: {body}")
            }
            Self::BadConfig(msg) => write!(f, "push adapter misconfigured: {msg}"),
        }
    }
}

impl std::error::Error for PushError {}

/// Implementations of this trait carry the user's push-target configuration
/// and know how to deliver a [`PushEvent`] to it. The daemon holds one
/// instance per configured target in an `Arc<dyn PushTarget>`.
pub trait PushTarget: Send + Sync {
    /// Deliver (or attempt to deliver) a push event. Fire-and-forget — the
    /// daemon does not persist unsent events.
    fn send(&self, event: &PushEvent) -> Result<(), PushError>;

    /// Human-readable label for logs and the Settings UI. `"NoOp"`,
    /// `"Webhook(https://hooks.slack.com/…)"`, etc.
    fn label(&self) -> String;
}

/// Default adapter. Silently drops every event. Chosen when the user hasn't
/// opted into any push configuration, which is the out-of-the-box state.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoOp;

impl PushTarget for NoOp {
    fn send(&self, _event: &PushEvent) -> Result<(), PushError> {
        Ok(())
    }

    fn label(&self) -> String {
        "NoOp".to_string()
    }
}

/// HTTP `POST` adapter: sends the event to a user-configured URL as JSON.
/// Payload shape is `{"title": "...", "body": "...", "action_url": "..."}` —
/// simple enough that users can route it into Slack webhooks, Discord,
/// Apple Shortcuts HTTP, their own ntfy server, etc.
#[derive(Debug, Clone)]
pub struct Webhook {
    pub url: String,
}

impl PushTarget for Webhook {
    fn send(&self, event: &PushEvent) -> Result<(), PushError> {
        let payload = serde_json::json!({
            "title": event.title(),
            "body": event.body(),
            "action_url": event.action_url(),
        });
        let response = reqwest::blocking::Client::new()
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("User-Agent", "k2so-daemon")
            .body(payload.to_string())
            .send()
            .map_err(|e| PushError::Transport(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(PushError::Remote {
                status: status.as_u16(),
                body,
            });
        }
        Ok(())
    }

    fn label(&self) -> String {
        format!("Webhook({})", self.url)
    }
}

/// Convenience wrapper for <https://ntfy.sh>. Supports user-specified ntfy
/// servers for fully self-hosted delivery. Payload matches the ntfy HTTP
/// protocol: `POST <server>/<topic>` with title / message / click headers
/// (a plain text body carries the message).
#[derive(Debug, Clone)]
pub struct NtfySh {
    /// `https://ntfy.sh` or the user's self-hosted server URL.
    pub server: String,
    /// Topic under `<server>/<topic>`.
    pub topic: String,
}

impl PushTarget for NtfySh {
    fn send(&self, event: &PushEvent) -> Result<(), PushError> {
        let url = format!("{}/{}", self.server.trim_end_matches('/'), self.topic);
        let mut request = reqwest::blocking::Client::new()
            .post(&url)
            .header("User-Agent", "k2so-daemon")
            .header("Title", event.title());
        let action = event.action_url();
        if !action.is_empty() {
            request = request.header("Click", action);
        }
        let response = request
            .body(event.body())
            .send()
            .map_err(|e| PushError::Transport(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(PushError::Remote {
                status: status.as_u16(),
                body,
            });
        }
        Ok(())
    }

    fn label(&self) -> String {
        format!("NtfySh({}/{})", self.server, self.topic)
    }
}

// ── K2Cloud (companion C4) ──────────────────────────────────────────────

/// How the daemon authenticates one dispatch call to the push gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayAuth {
    /// Per-daemon auth (prd-thread-push-notifications-v1 §8, slice S0):
    /// the daemon's TUNNEL bearer token plus `X-K2-Subdomain: <label>`.
    /// The gateway validates it like `cert.k2.dev` does and keeps a
    /// budget per subdomain.
    Tunnel { token: String, subdomain: String },
    /// The legacy fleet-wide shared bearer (one global budget). Kept for
    /// relays without S0 and for daemons with no paired tunnel.
    Shared { token: String },
}

impl GatewayAuth {
    pub fn bearer(&self) -> &str {
        match self {
            Self::Tunnel { token, .. } | Self::Shared { token } => token,
        }
    }

    /// The `X-K2-Subdomain` header value (per-daemon auth only).
    pub fn subdomain_header(&self) -> Option<&str> {
        match self {
            Self::Tunnel { subdomain, .. } => Some(subdomain),
            Self::Shared { .. } => None,
        }
    }

    pub fn is_tunnel(&self) -> bool {
        matches!(self, Self::Tunnel { .. })
    }
}

/// The response header an S0 gateway sets on EVERY per-daemon-path
/// response, success or error (k2-connect PR #13 review, finding 2). Its
/// absence on a refusal means an old gateway that does not know the
/// per-daemon path — the only case the daemon falls back to the shared
/// token.
pub const S0_MARKER_HEADER: &str = "X-K2-Push-Gateway";
/// The marker's value.
pub const S0_MARKER_VALUE: &str = "s0";

/// One gateway HTTP response, whatever its status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayResponse {
    pub status: u16,
    pub body: String,
    /// The response carried [`S0_MARKER_HEADER`]: [`S0_MARKER_VALUE`].
    pub s0: bool,
}

impl GatewayResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The gateway's error code (`error.code`, `error` or `code` in the
    /// JSON body), else `HTTP <status>`.
    pub fn reason(&self) -> String {
        let v: serde_json::Value = serde_json::from_str(&self.body).unwrap_or_default();
        let code = v
            .pointer("/error/code")
            .and_then(|c| c.as_str())
            .or_else(|| v.get("error").and_then(|c| c.as_str()))
            .or_else(|| v.get("code").and_then(|c| c.as_str()));
        match code {
            Some(c) if !c.trim().is_empty() => c.trim().to_string(),
            _ => format!("HTTP {}", self.status),
        }
    }
}

/// The one HTTP call [`K2Cloud`] makes, behind a seam so tests inject
/// a recorder instead of a live socket (no network in tests — the
/// gateway itself is exercised in the k2-connect repo). `Ok` carries any
/// HTTP response (the caller reads the status and the S0 marker); `Err`
/// is a transport failure only.
pub trait DispatchSender: Send + Sync {
    fn post(
        &self,
        url: &str,
        auth: &GatewayAuth,
        body: &serde_json::Value,
    ) -> Result<GatewayResponse, PushError>;
}

/// Production sender: blocking reqwest POST with the chosen bearer (and
/// `X-K2-Subdomain` on the per-daemon path). Mirrors the [`Webhook`]
/// transport idiom (fire-and-forget; no internal retry — the gateway is
/// on the same relay the daemon already depends on).
#[derive(Debug, Default, Clone, Copy)]
pub struct HttpDispatchSender;

impl DispatchSender for HttpDispatchSender {
    fn post(
        &self,
        url: &str,
        auth: &GatewayAuth,
        body: &serde_json::Value,
    ) -> Result<GatewayResponse, PushError> {
        let mut request = reqwest::blocking::Client::new()
            .post(url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", auth.bearer()))
            .header("User-Agent", "k2-daemon");
        if let Some(sub) = auth.subdomain_header() {
            request = request.header("X-K2-Subdomain", sub);
        }
        let response = request
            .body(body.to_string())
            .send()
            .map_err(|e| PushError::Transport(e.to_string()))?;
        let status = response.status().as_u16();
        let s0 = response
            .headers()
            .get(S0_MARKER_HEADER)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim().eq_ignore_ascii_case(S0_MARKER_VALUE));
        let body = response.text().unwrap_or_default();
        Ok(GatewayResponse { status, body, s0 })
    }
}

/// The gateway a paired daemon uses when nothing overrides it (TP6).
pub const DEFAULT_GATEWAY_URL: &str = "https://push.k2.dev";

/// Most devices one dispatch call may carry (the gateway's
/// `MAX_DEVICES_PER_DISPATCH`); a larger recipient set is split.
pub const MAX_DEVICES_PER_CALL: usize = 100;

/// Resolved push-gateway configuration. `None` from the resolvers =
/// DORMANT (no URL, or no credential at all).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayConfig {
    /// Gateway base URL (no `/push/dispatch` suffix).
    pub url: String,
    /// The paired tunnel's `(token, subdomain)`, when both are set.
    pub tunnel: Option<(String, String)>,
    /// The legacy shared gateway token, when configured.
    pub shared_token: Option<String>,
    /// The tunnel subdomain stamped into `data.subdomain` (may be set
    /// without a token; empty = none).
    pub subdomain: String,
}

fn non_blank(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

impl GatewayConfig {
    /// Pure resolution (TP6): a paired tunnel (token + subdomain) is
    /// enough — the URL defaults to [`DEFAULT_GATEWAY_URL`]. Without a
    /// tunnel, the URL and the shared token must both be set (today's
    /// dormancy gate). An explicit URL always wins.
    pub fn resolve(
        url: Option<String>,
        shared_token: Option<String>,
        tunnel_token: Option<String>,
        tunnel_subdomain: Option<String>,
    ) -> Option<Self> {
        let subdomain = non_blank(tunnel_subdomain).map(|s| s.to_ascii_lowercase());
        let tunnel = match (non_blank(tunnel_token), subdomain.clone()) {
            (Some(t), Some(s)) => Some((t, s)),
            _ => None,
        };
        let shared_token = non_blank(shared_token);
        let url = match non_blank(url) {
            Some(u) => K2Cloud::normalize_gateway_base(&u),
            None if tunnel.is_some() => DEFAULT_GATEWAY_URL.to_string(),
            None => return None,
        };
        if url.is_empty() || (tunnel.is_none() && shared_token.is_none()) {
            return None;
        }
        Some(Self {
            url,
            tunnel,
            shared_token,
            subdomain: subdomain.unwrap_or_default(),
        })
    }

    /// Resolve from the environment, settings and the tunnel config.
    /// Env wins over settings (the `K2_*`-override house pattern):
    /// [`ENV_GATEWAY_URL`]/[`ENV_GATEWAY_TOKEN`], falling back to the
    /// `pushGatewayUrl`/`pushGatewayToken` app settings; the tunnel
    /// token + subdomain come from `tunnel.json` (or its vault).
    pub fn from_config() -> Option<Self> {
        let settings = crate::app_settings::load();
        let url = std::env::var(ENV_GATEWAY_URL)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or(settings.push_gateway_url);
        let token = std::env::var(ENV_GATEWAY_TOKEN)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or(settings.push_gateway_token);
        let tunnel = crate::tunnel::config::load().unwrap_or_default();
        Self::resolve(url, token, Some(tunnel.token), Some(tunnel.subdomain))
    }
}

// ── Relay path state (§6.3, k2-connect PR #13 review finding 2) ─────────
//
// Two different refusals of the per-daemon (tunnel-token) auth:
//
// - **Old gateway** (no S0 marker on a 401/403): the relay does not know the
//   per-daemon path. The daemon falls back to the shared token with the
//   small Thread bucket, and tries the tunnel token again after
//   [`TUNNEL_RETRY_SECS`].
// - **Rejected** (an S0-marked 401/403: `subdomain_not_owned`, unknown
//   token, suspended account): the relay knows this daemon and said no.
//   The daemon NEVER falls back (a suspended account must not keep pushing
//   on the shared token), sends nothing, and reports `push: "dormant"` with
//   the reason until a retry after [`TUNNEL_RETRY_SECS`] succeeds.
//
// A 400 never falls back either: the body is wrong, not the auth.

/// How long either refusal holds before the daemon tries the tunnel token
/// again.
pub const TUNNEL_RETRY_SECS: i64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
enum TunnelState {
    /// Never refused, or accepted since.
    Ok,
    /// An old gateway refused it at this time (fall back to shared).
    OldGateway { at: i64 },
    /// An S0 gateway rejected it at this time, with its reason.
    Rejected { at: i64, reason: String },
}

static TUNNEL_STATE: std::sync::Mutex<TunnelState> = std::sync::Mutex::new(TunnelState::Ok);

fn tunnel_state() -> std::sync::MutexGuard<'static, TunnelState> {
    TUNNEL_STATE.lock().unwrap_or_else(|p| p.into_inner())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// An old gateway (no S0 marker) refused the tunnel token at `now`.
pub fn note_old_gateway(now: i64) {
    *tunnel_state() = TunnelState::OldGateway { at: now };
}

/// An S0 gateway rejected the tunnel token at `now` (marked 401/403).
pub fn note_tunnel_rejected(now: i64, reason: &str) {
    *tunnel_state() = TunnelState::Rejected { at: now, reason: reason.to_string() };
}

/// The gateway accepted the tunnel token: forget any refusal.
pub fn note_tunnel_accepted() {
    *tunnel_state() = TunnelState::Ok;
}

/// True while an old-gateway refusal is younger than [`TUNNEL_RETRY_SECS`].
pub fn old_gateway_recently(now: i64) -> bool {
    matches!(*tunnel_state(), TunnelState::OldGateway { at } if now - at < TUNNEL_RETRY_SECS)
}

/// The reason of an S0 rejection younger than [`TUNNEL_RETRY_SECS`].
pub fn tunnel_rejected_recently(now: i64) -> Option<String> {
    match &*tunnel_state() {
        TunnelState::Rejected { at, reason } if now - at < TUNNEL_RETRY_SECS => Some(reason.clone()),
        _ => None,
    }
}

/// Test-only: reset the process-wide relay state.
#[cfg(any(test, feature = "test-util"))]
pub fn reset_relay_state_for_test() {
    note_tunnel_accepted();
}

/// The `push` field of `GET /cli/notify/prefs` (§5.6, TP25).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushStatus {
    /// Pushes leave this daemon.
    Ready,
    /// Nothing is sent: no gateway, no credential, or the gateway rejected
    /// this daemon (`reason`).
    Dormant { reason: Option<String> },
    /// Air-gap is on: no push of any kind (TP5).
    Airgap,
}

impl PushStatus {
    pub fn as_wire(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Dormant { .. } => "dormant",
            Self::Airgap => "airgap",
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Dormant { reason } => reason.as_deref(),
            _ => None,
        }
    }

    /// Pure: from the resolved config, the air-gap flag and the clock.
    pub fn of(cfg: Option<&GatewayConfig>, airgap: bool, now: i64) -> Self {
        if airgap {
            return Self::Airgap;
        }
        let Some(cfg) = cfg else {
            return Self::Dormant { reason: Some("no push gateway: pair a tunnel".into()) };
        };
        if cfg.tunnel.is_some() {
            if let Some(reason) = tunnel_rejected_recently(now) {
                return Self::Dormant { reason: Some(format!("the push gateway refused this server: {reason}")) };
            }
        }
        Self::Ready
    }
}

/// Which relay budget this daemon is on (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayPath {
    /// Per-daemon auth: the relay's per-subdomain windows.
    PerDaemon,
    /// The shared token: the fleet's one global window of 60 an hour,
    /// so the daemon's Thread bucket drops to
    /// [`budget::THREAD_PER_HOUR_SHARED`].
    Shared,
}

impl RelayPath {
    pub fn of(cfg: &GatewayConfig, now: i64) -> Self {
        if cfg.tunnel.is_some() && !old_gateway_recently(now) {
            Self::PerDaemon
        } else {
            Self::Shared
        }
    }

    /// The Thread bucket size on this path (per hour).
    pub fn thread_per_hour(self) -> usize {
        match self {
            Self::PerDaemon => budget::THREAD_PER_HOUR,
            Self::Shared => budget::THREAD_PER_HOUR_SHARED,
        }
    }
}

/// Companion C4 mobile push adapter — the module's reserved cloud
/// seam, filled (naming: k2, not k2so). `send` reads the daemon-held
/// device registry ([`crate::push_devices`]) and POSTs
/// `{devices:[{platform,token}], payload:{title, body, data, sound,
/// threadId?, collapseId?}}` to `<gateway>/push/dispatch`; the stateless
/// relay gateway fans out to APNs/FCM. Tokens the response marks
/// `dead:true` are pruned from the registry (aggressive dead-token
/// hygiene, §4.5).
///
/// Auth (TP1): the per-daemon tunnel token first; when the gateway
/// refuses it (403/401/400 — a relay without S0), the same body goes
/// again with the shared token, and the refusal is remembered for
/// [`TUNNEL_RETRY_SECS`].
pub struct K2Cloud {
    config: GatewayConfig,
    sender: std::sync::Arc<dyn DispatchSender>,
}

/// The exact env/settings keys [`GatewayConfig::from_config`] reads.
pub const ENV_GATEWAY_URL: &str = "K2_PUSH_GATEWAY_URL";
pub const ENV_GATEWAY_TOKEN: &str = "K2_PUSH_GATEWAY_TOKEN";

impl K2Cloud {
    /// Normalize a gateway base URL so `send` can always append
    /// `/push/dispatch` once.
    ///
    /// Accepts either form that ships in the wild:
    /// - base: `https://push.k2.dev` (docs / `from_parts` tests)
    /// - full dispatch: `https://push.k2.dev/push/dispatch` (some
    ///   app/settings writers store the full path)
    ///
    /// Without this strip, a full-path setting becomes
    /// `…/push/dispatch/push/dispatch` → gateway 404, so feedback +
    /// project-chat pushes silently fail while a hand-crafted POST to
    /// the correct URL still works.
    pub fn normalize_gateway_base(url: &str) -> String {
        let mut u = url.trim().trim_end_matches('/').to_string();
        // Case-insensitive path suffix (env/settings may vary casing).
        let lower = u.to_ascii_lowercase();
        if let Some(stripped) = lower.strip_suffix("/push/dispatch") {
            let keep = stripped.len();
            u.truncate(keep);
            u = u.trim_end_matches('/').to_string();
        }
        u
    }

    /// Shared-token-only construction from resolved parts. `None` unless
    /// BOTH the gateway URL and the gateway token are present and
    /// non-blank — the legacy dormancy gate, kept pure so it's directly
    /// testable.
    pub fn from_parts(url: Option<String>, token: Option<String>) -> Option<Self> {
        GatewayConfig::resolve(url, token, None, None).map(|c| Self::with_config(c, std::sync::Arc::new(HttpDispatchSender)))
    }

    /// Resolve config (env, settings, tunnel) and construct, or `None` =
    /// DORMANT.
    pub fn from_config() -> Option<Self> {
        GatewayConfig::from_config().map(|c| Self::with_config(c, std::sync::Arc::new(HttpDispatchSender)))
    }

    /// Construct from a resolved config with an injected transport.
    pub fn with_config(config: GatewayConfig, sender: std::sync::Arc<dyn DispatchSender>) -> Self {
        Self { config, sender }
    }

    /// Test seam: the shared-token adapter with an injected transport.
    pub fn with_sender(
        gateway_url: &str,
        gateway_token: &str,
        sender: std::sync::Arc<dyn DispatchSender>,
    ) -> Self {
        Self::with_config(
            GatewayConfig {
                url: Self::normalize_gateway_base(gateway_url),
                tunnel: None,
                shared_token: Some(gateway_token.to_string()),
                subdomain: String::new(),
            },
            sender,
        )
    }

    pub fn config(&self) -> &GatewayConfig {
        &self.config
    }

    /// The `payload` object for ONE recipient person. Mobile kinds carry
    /// `sound`, the server stamp and the person's `badgeKey` (TP29: whose
    /// push it is, 0.46.1: never their name); a Thread push also carries
    /// `threadId` + `collapseId` (§7). No `aps.badge`, ever (TP27).
    pub fn payload(event: &PushEvent, stamp: &PushStamp, badge_key: &str) -> serde_json::Value {
        let mut data = event.data();
        if event.is_mobile() {
            if !badge_key.is_empty() {
                data.insert("badgeKey", badge_key.to_string());
            }
            if !stamp.subdomain.is_empty() {
                data.insert("subdomain", stamp.subdomain.clone());
            }
            if !stamp.instance_id.is_empty() {
                data.insert("instanceId", stamp.instance_id.clone());
            }
        }
        if let PushEvent::Badge(_) = event {
            // Silent (thread-read-v1): no title, body or sound; the relay
            // sends it as a background push and collapses on `badgeKey`.
            return serde_json::json!({ "data": data });
        }
        let mut payload = serde_json::json!({
            "title": event.title(),
            "body": event.body(),
            "data": data,
        });
        if event.is_mobile() {
            payload["sound"] = serde_json::json!(PUSH_SOUND_DEFAULT);
        }
        if let Some(t) = event.thread_id() {
            payload["threadId"] = serde_json::json!(t);
        }
        if let Some(c) = event.collapse_id() {
            payload["collapseId"] = serde_json::json!(c);
        }
        payload
    }

    /// One dispatch body: these devices + the payload.
    fn dispatch_body(
        devices: &[crate::push_devices::PushDevice],
        payload: &serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "devices": devices
                .iter()
                .map(|d| serde_json::json!({ "platform": d.platform, "token": d.token }))
                .collect::<Vec<_>>(),
            "payload": payload,
        })
    }

    /// POST one body (TP1, PR #13 review finding 2).
    ///
    /// With a paired tunnel the per-daemon path goes first:
    /// - 2xx → done (any earlier refusal is forgotten);
    /// - an S0-MARKED 401/403 (`subdomain_not_owned`, unknown token,
    ///   suspended) → logged, `push` turns dormant with the reason, and
    ///   NOTHING is sent — never the shared token;
    /// - an UNMARKED 401/403 → an old gateway: fall back to the shared
    ///   token (when configured) and use it for [`TUNNEL_RETRY_SECS`];
    /// - a 400 → never falls back (the body is wrong, not the auth);
    /// - anything else (429, 5xx) → an error, no fallback.
    ///
    /// While an S0 rejection is fresh, the per-daemon path is not retried
    /// and nothing is sent. Without a tunnel the shared token is the only
    /// path.
    fn post_with_fallback(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, PushError> {
        let now = unix_now();
        let shared = self.config.shared_token.as_ref().map(|t| GatewayAuth::Shared { token: t.clone() });
        if let Some((token, subdomain)) = self.config.tunnel.as_ref() {
            if let Some(reason) = tunnel_rejected_recently(now) {
                return Err(PushError::BadConfig(format!(
                    "push gateway rejected this server ({reason}); not sending"
                )));
            }
            if !old_gateway_recently(now) || shared.is_none() {
                let auth = GatewayAuth::Tunnel { token: token.clone(), subdomain: subdomain.clone() };
                let resp = self.sender.post(url, &auth, body)?;
                if resp.is_success() {
                    note_tunnel_accepted();
                    return Ok(serde_json::from_str(&resp.body).unwrap_or(serde_json::Value::Null));
                }
                let refused = matches!(resp.status, 401 | 403);
                if refused && resp.s0 {
                    let reason = resp.reason();
                    note_tunnel_rejected(now, &reason);
                    crate::log_debug!("[push] gateway rejected this server (HTTP {}, {reason}); push is dormant", resp.status);
                    return Err(PushError::Remote { status: resp.status, body: resp.body });
                }
                if !refused || shared.is_none() {
                    if refused {
                        note_old_gateway(now);
                    }
                    return Err(PushError::Remote { status: resp.status, body: resp.body });
                }
                note_old_gateway(now);
                crate::log_debug!(
                    "[push] gateway has no per-daemon path (HTTP {}, no S0 marker); using the shared token",
                    resp.status
                );
            }
        }
        let Some(auth) = shared else {
            return Err(PushError::BadConfig("no push gateway credential".into()));
        };
        let resp = self.sender.post(url, &auth, body)?;
        if resp.is_success() {
            Ok(serde_json::from_str(&resp.body).unwrap_or(serde_json::Value::Null))
        } else {
            Err(PushError::Remote { status: resp.status, body: resp.body })
        }
    }

    /// Split one person's devices by payload variant: a push with a
    /// message preview goes as is to devices with `previews` on, and
    /// [`PushEvent::without_preview`] (the generic body) to the rest. A
    /// push without a preview is one group. Empty groups are left out, so
    /// the number of groups is the number of gateway calls (per
    /// [`MAX_DEVICES_PER_CALL`] devices) the dispatch makes.
    pub fn payload_groups<'a>(
        event: &PushEvent,
        devices: &'a [crate::push_devices::PushDevice],
    ) -> Vec<(PushEvent, Vec<&'a crate::push_devices::PushDevice>)> {
        if !event.has_preview() {
            return if devices.is_empty() { Vec::new() } else { vec![(event.clone(), devices.iter().collect())] };
        }
        let (on, off): (Vec<_>, Vec<_>) = devices.iter().partition(|d| d.previews);
        let mut out = Vec::new();
        if !on.is_empty() {
            out.push((event.clone(), on));
        }
        if !off.is_empty() {
            out.push((event.without_preview(), off));
        }
        out
    }

    /// Send `event` to ONE person's devices (`badge_key` = that person's
    /// [`badge_key`], stamped into `data`, TP29). The daemon resolved them (Thread
    /// recipients, or bell-filtered ticket/project targets). One call per
    /// payload variant ([`Self::payload_groups`]: a Thread preview to
    /// devices with previews on, the generic body to the rest) and per
    /// [`MAX_DEVICES_PER_CALL`] devices; dead tokens are pruned.
    pub fn dispatch(
        &self,
        event: &PushEvent,
        badge_key: &str,
        devices: &[crate::push_devices::PushDevice],
    ) -> Result<(), PushError> {
        for (variant, group) in Self::payload_groups(event, devices) {
            let group: Vec<_> = group.into_iter().cloned().collect();
            self.dispatch_one(&variant, badge_key, &group)?;
        }
        Ok(())
    }

    /// One payload to these devices.
    fn dispatch_one(
        &self,
        event: &PushEvent,
        badge_key: &str,
        devices: &[crate::push_devices::PushDevice],
    ) -> Result<(), PushError> {
        if devices.is_empty() {
            return Ok(());
        }
        let stamp = PushStamp {
            subdomain: self.config.subdomain.clone(),
            instance_id: instance_id(),
        };
        let payload = Self::payload(event, &stamp, badge_key);
        let url = format!("{}/push/dispatch", self.config.url);
        for chunk in devices.chunks(MAX_DEVICES_PER_CALL) {
            let body = Self::dispatch_body(chunk, &payload);
            let response = self.post_with_fallback(&url, &body)?;
            // Dead-token feedback: the gateway reports per-token outcomes;
            // `dead:true` (APNs 410 Unregistered / FCM NotRegistered)
            // tokens are pruned so they never ride another dispatch. A
            // malformed/missing results array is simply no prunes — the
            // push itself already succeeded.
            if let Some(results) = response.get("results").and_then(|r| r.as_array()) {
                for entry in results {
                    let dead = entry.get("dead").and_then(|d| d.as_bool()).unwrap_or(false);
                    let token = entry.get("token").and_then(|t| t.as_str());
                    if let (true, Some(token)) = (dead, token) {
                        if let Err(e) = crate::push_devices::prune_device_by_token(token) {
                            crate::log_debug!("[push] dead-token prune failed: {e}");
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl PushTarget for K2Cloud {
    fn send(&self, event: &PushEvent) -> Result<(), PushError> {
        let devices = match event.target_usernames() {
            Some(users) => crate::push_devices::list_devices_for_usernames(users)
                .map_err(PushError::BadConfig)?,
            None => crate::push_devices::list_devices().map_err(PushError::BadConfig)?,
        };
        // Nobody registered (or no assignee has a device) — no network call.
        // One dispatch per login (TP29: never another person's devices in
        // a payload). The daemon groups by PERSON instead; this direct
        // path has no Connect-user store, so it groups by login.
        let mut by_login: std::collections::BTreeMap<String, Vec<crate::push_devices::PushDevice>> =
            Default::default();
        for d in devices {
            by_login.entry(d.username.clone()).or_default().push(d);
        }
        for (login, group) in by_login {
            let person = match event {
                PushEvent::ThreadPost(t) => t.principal.clone(),
                PushEvent::ProjectPost(p) => p.principal.clone(),
                PushEvent::Badge(b) => b.principal.clone(),
                _ => login,
            };
            let key = crate::read_state::badge_key_for_shared(&person);
            self.dispatch(event, &key, &group)?;
        }
        Ok(())
    }

    fn label(&self) -> String {
        format!("K2Cloud({})", self.config.url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> PushEvent {
        PushEvent::AgentNeedsAttention {
            agent: "cortana".to_string(),
            summary: "Weekly financial report drafted — ready for review".to_string(),
            action_url: "k2so://agents/cortana".to_string(),
        }
    }

    #[test]
    fn noop_swallows_events() {
        let target = NoOp;
        assert!(target.send(&sample_event()).is_ok());
        assert_eq!(target.label(), "NoOp");
    }

    #[test]
    fn webhook_is_labeled_with_url() {
        let target = Webhook {
            url: "https://example.com/hook".to_string(),
        };
        assert_eq!(target.label(), "Webhook(https://example.com/hook)");
    }

    #[test]
    fn ntfy_is_labeled_with_server_and_topic() {
        let target = NtfySh {
            server: "https://ntfy.sh".to_string(),
            topic: "k2so-rosson".to_string(),
        };
        assert_eq!(target.label(), "NtfySh(https://ntfy.sh/k2so-rosson)");
    }

    #[test]
    fn push_event_renders_title_and_body() {
        let e = sample_event();
        assert!(e.title().contains("cortana"));
        assert!(e.body().contains("financial"));
        assert_eq!(e.action_url(), "k2so://agents/cortana");
    }

    #[test]
    fn push_event_heartbeat_has_no_action_url() {
        let e = PushEvent::HeartbeatFailed {
            heartbeat: "daily-brief".to_string(),
            reason: "wakeup.md missing".to_string(),
        };
        assert_eq!(e.action_url(), "");
        assert!(e.title().contains("daily-brief"));
    }

    #[test]
    fn push_target_object_safe() {
        // Adapters must be usable via `Arc<dyn PushTarget>` so the daemon
        // can hold exactly one and share it across tokio tasks.
        let target: std::sync::Arc<dyn PushTarget> = std::sync::Arc::new(NoOp);
        let _ = target.send(&sample_event());
        assert_eq!(target.label(), "NoOp");
    }

    // ── Companion C4: event shaping + K2Cloud ─────────────────────────

    use std::sync::{Arc, Mutex};

    /// The one place the wire literals are spelled out: the mobile
    /// companion routes on these exact strings.
    #[test]
    fn push_kind_wire_values_are_stable() {
        assert_eq!(PUSH_KIND_TICKET, "ticket");
        assert_eq!(PUSH_KIND_PROJECT, "project");
    }

    #[test]
    fn c4_events_render_generic_title_and_deep_link_data() {
        let e = PushEvent::FeedbackCreated {
            agent_name: "cortana".to_string(),
            feedback_id: "fb-123".to_string(),
            assignees: vec![],
        };
        assert_eq!(e.title(), "K2");
        assert_eq!(e.body(), "cortana needs you on a ticket");
        assert_eq!(e.action_url(), "");
        assert_eq!(
            e.data(),
            serde_json::json!({ "kind": PUSH_KIND_TICKET, "feedbackId": "fb-123" })
        );

        let e = PushEvent::ProjectMessageCreated {
            project_name: "Apollo".to_string(),
            group_id: "pg-9".to_string(),
        };
        assert_eq!(e.title(), "K2");
        assert_eq!(e.body(), "New message in Apollo");
        assert_eq!(e.action_url(), "");
        assert_eq!(
            e.data(),
            serde_json::json!({ "kind": PUSH_KIND_PROJECT, "groupId": "pg-9" })
        );

        // Legacy desktop-era events carry no mobile deep link.
        assert_eq!(sample_event().data(), serde_json::json!({}));
    }

    /// Recording sender: captures every call, answers a canned
    /// response. No network anywhere in these tests.
    struct MockSender {
        calls: Mutex<Vec<(String, GatewayAuth, serde_json::Value)>>,
        /// Scripted responses, in order; when empty, a 200 with `response`.
        script: Mutex<std::collections::VecDeque<GatewayResponse>>,
        response: serde_json::Value,
    }

    impl MockSender {
        fn new(response: serde_json::Value) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                script: Mutex::new(Default::default()),
                response,
            })
        }

        fn scripted(responses: Vec<GatewayResponse>) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                script: Mutex::new(responses.into()),
                response: serde_json::json!({ "results": [] }),
            })
        }

        fn calls(&self) -> Vec<(String, GatewayAuth, serde_json::Value)> {
            self.calls.lock().expect("mock lock").clone()
        }
    }

    impl DispatchSender for MockSender {
        fn post(
            &self,
            url: &str,
            auth: &GatewayAuth,
            body: &serde_json::Value,
        ) -> Result<GatewayResponse, PushError> {
            self.calls.lock().expect("mock lock").push((
                url.to_string(),
                auth.clone(),
                body.clone(),
            ));
            Ok(self.script.lock().expect("mock lock").pop_front().unwrap_or_else(|| GatewayResponse {
                status: 200,
                body: self.response.to_string(),
                s0: false,
            }))
        }
    }

    fn unique_token(label: &str) -> String {
        format!("tok-push-{label}-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn normalize_gateway_base_accepts_base_or_full_dispatch_url() {
        assert_eq!(
            K2Cloud::normalize_gateway_base("https://push.k2.dev"),
            "https://push.k2.dev"
        );
        assert_eq!(
            K2Cloud::normalize_gateway_base("https://push.k2.dev/"),
            "https://push.k2.dev"
        );
        assert_eq!(
            K2Cloud::normalize_gateway_base("https://push.k2.dev/push/dispatch"),
            "https://push.k2.dev"
        );
        assert_eq!(
            K2Cloud::normalize_gateway_base("https://push.k2.dev/push/dispatch/"),
            "https://push.k2.dev"
        );
        // from_parts with full-path setting still builds a clean base.
        let c = K2Cloud::from_parts(
            Some("https://push.k2.dev/push/dispatch".into()),
            Some("tok".into()),
        )
        .expect("configured");
        assert_eq!(c.label(), "K2Cloud(https://push.k2.dev)");
    }

    #[test]
    fn k2cloud_dispatch_payload_is_content_free() {
        // Own DB: an untargeted send lists EVERY device, and other tests'
        // devices (other logins) would add dispatches.
        let _db = crate::db::scoped_for_test();
        set_instance_id(TEST_INSTANCE);
        let token = unique_token("shape");
        let device_id = format!("dev-push-shape-{}", uuid::Uuid::new_v4());
        crate::push_devices::upsert_device(&device_id, "apns", &token, "owner")
            .expect("register device");

        let sender = MockSender::new(serde_json::json!({ "results": [] }));
        // Full-path setting form (matches live ~/.k2/settings.json) must
        // still POST exactly once to …/push/dispatch, not …/dispatch/push/dispatch.
        let cloud = K2Cloud::with_sender(
            "https://push.k2.dev/push/dispatch",
            "gw-secret",
            sender.clone(),
        );
        assert_eq!(cloud.label(), "K2Cloud(https://push.k2.dev)");

        cloud
            .send(&PushEvent::FeedbackCreated {
                agent_name: "cortana".to_string(),
                feedback_id: "fb-77".to_string(),
                assignees: vec![],
            })
            .expect("send");

        let calls = sender.calls();
        assert_eq!(calls.len(), 1, "exactly one dispatch POST");
        let (url, auth, body) = &calls[0];
        assert_eq!(url, "https://push.k2.dev/push/dispatch");
        assert_eq!(auth, &GatewayAuth::Shared { token: "gw-secret".into() });

        // Wire shape: {devices:[{platform,token}], payload:{title,body,
        // data,sound}} and NOTHING else — the §4.5 content-free invariant
        // is structural: no field exists that could carry ask/message text.
        let top: Vec<_> = body.as_object().expect("object").keys().collect();
        assert_eq!(top, vec!["devices", "payload"], "no extra top-level fields");
        let payload = body["payload"].as_object().expect("payload object");
        assert_eq!(
            payload.keys().collect::<Vec<_>>(),
            vec!["body", "data", "sound", "title"],
            "a ticket push carries exactly title/body/data/sound (no badge, TP27)"
        );
        assert_eq!(body["payload"]["title"], "K2");
        assert_eq!(body["payload"]["body"], "cortana needs you on a ticket");
        assert_eq!(body["payload"]["sound"], "default");
        assert_eq!(
            body["payload"]["data"],
            serde_json::json!({
                "kind": PUSH_KIND_TICKET,
                "feedbackId": "fb-77",
                "instanceId": TEST_INSTANCE,
                "badgeKey": crate::read_state::badge_key_for_shared("owner"),
            }),
            "no tunnel subdomain on this config → no subdomain key; instanceId + badgeKey always, never a name"
        );
        let devices = body["devices"].as_array().expect("devices array");
        let mine = devices
            .iter()
            .find(|d| d["token"] == token.as_str())
            .expect("registered device rides the dispatch");
        assert_eq!(mine["platform"], "apns");
        assert_eq!(
            mine.as_object().expect("device object").keys().collect::<Vec<_>>(),
            vec!["platform", "token"],
            "devices carry platform+token only — usernames stay home"
        );

        crate::push_devices::remove_device(&device_id).expect("cleanup");
    }

    #[test]
    fn k2cloud_prunes_tokens_the_gateway_marks_dead() {
        let dead_token = unique_token("dead");
        let live_token = unique_token("live");
        let dead_id = format!("dev-push-dead-{}", uuid::Uuid::new_v4());
        let live_id = format!("dev-push-live-{}", uuid::Uuid::new_v4());
        crate::push_devices::upsert_device(&dead_id, "apns", &dead_token, "owner")
            .expect("register dead");
        crate::push_devices::upsert_device(&live_id, "fcm", &live_token, "owner")
            .expect("register live");

        let sender = MockSender::new(serde_json::json!({
            "results": [
                { "token": dead_token, "dead": true },
                { "token": live_token, "dead": false },
            ]
        }));
        let cloud = K2Cloud::with_sender("https://push.k2.dev", "gw", sender);
        cloud
            .send(&PushEvent::ProjectMessageCreated {
                project_name: "Apollo".to_string(),
                group_id: "pg-1".to_string(),
            })
            .expect("send");

        let ids: Vec<_> = crate::push_devices::list_devices()
            .expect("list")
            .into_iter()
            .map(|d| d.device_id)
            .collect();
        assert!(!ids.contains(&dead_id), "dead-marked token pruned");
        assert!(ids.contains(&live_id), "live token survives");
        crate::push_devices::remove_device(&live_id).expect("cleanup");
    }

    #[test]
    fn k2cloud_from_parts_is_dormant_unless_fully_configured() {
        // The dormancy gate: BOTH url and token, non-blank, or nothing.
        assert!(K2Cloud::from_parts(None, None).is_none());
        assert!(K2Cloud::from_parts(Some("https://push.k2.dev".into()), None).is_none());
        assert!(K2Cloud::from_parts(None, Some("gw".into())).is_none());
        assert!(K2Cloud::from_parts(Some("  ".into()), Some("gw".into())).is_none());
        assert!(K2Cloud::from_parts(Some("https://push.k2.dev".into()), Some(" ".into()))
            .is_none());
        let cloud = K2Cloud::from_parts(
            Some("https://push.k2.dev/".into()),
            Some("gw".into()),
        )
        .expect("fully configured constructs");
        assert_eq!(cloud.label(), "K2Cloud(https://push.k2.dev)");
    }

    // ── prd-thread-push-notifications-v1 S1 ──────────────────────────

    const TEST_INSTANCE: &str = "feedfacefeedface";
    /// A person's badgeKey as the daemon would pass it.
    const TEST_KEY: &str = "0123456789abcdef0123456789abcdef";

    fn thread_event(kind: ThreadMsgKind, count: u32, cid: &str) -> PushEvent {
        PushEvent::ThreadPost(ThreadPush {
            agent_name: "Cortana".into(),
            conversation_id: cid.into(),
            addr: "k2/reviewer".into(),
            seq: 418,
            project_id: "proj-1".into(),
            msg_kind: kind,
            count,
            principal: "owner".into(),
            device_usernames: vec!["owner".into()],
            unread_total: "4".into(),
            waiting_total: "1".into(),
            avatar_rev: "ba7816bf8f01cfea".into(),
            agent_color: "#3b82f6".into(),
            preview: String::new(),
        })
    }

    fn project_post(principal: &str) -> PushEvent {
        PushEvent::ProjectPost(ProjectPush {
            project_name: "Apollo".into(),
            group_id: "pg-9".into(),
            principal: principal.into(),
            device_usernames: vec![principal.into()],
            unread_total: "7".into(),
            waiting_total: "2".into(),
        })
    }

    fn badge_push(principal: &str) -> PushEvent {
        PushEvent::Badge(BadgePush {
            principal: principal.into(),
            device_usernames: vec![principal.into()],
            unread_total: "3".into(),
            waiting_total: "1".into(),
        })
    }

    /// thread-read-v1: a project push is one person's, targeted, with
    /// their totals as strings, and content-free.
    #[test]
    fn project_post_is_targeted_with_totals_and_no_text() {
        let e = project_post("gina");
        assert_eq!(e.target_usernames(), Some(&["gina".to_string()][..]));
        let stamp = PushStamp { subdomain: "rosson".into(), instance_id: TEST_INSTANCE.into() };
        let p = K2Cloud::payload(&e, &stamp, TEST_KEY);
        assert_eq!(
            p,
            serde_json::json!({
                "title": "K2",
                "body": "New message in Apollo",
                "sound": "default",
                "data": {
                    "kind": "project",
                    "groupId": "pg-9",
                    "unreadTotal": "7",
                    "waitingTotal": "2",
                    "badgeKey": TEST_KEY,
                    "subdomain": "rosson",
                    "instanceId": TEST_INSTANCE,
                },
            })
        );
        assert_eq!(e.budget_class(), budget::BudgetClass::Other, "projects keep the ticket/project bucket");
    }

    /// thread-read-v1: the silent badge push has no alert fields at all
    /// (no title, body, sound, threadId, collapseId, badge), carries the
    /// totals, the stamp and an opaque per-person `badgeKey`, and draws
    /// from its own budget.
    #[test]
    fn badge_push_is_silent_with_totals_and_an_opaque_key() {
        let e = badge_push("gina");
        let stamp = PushStamp { subdomain: "rosson".into(), instance_id: TEST_INSTANCE.into() };
        let p = K2Cloud::payload(&e, &stamp, TEST_KEY);
        assert_eq!(
            p,
            serde_json::json!({
                "data": {
                    "kind": "badge",
                    "unreadTotal": "3",
                    "waitingTotal": "1",
                    "subdomain": "rosson",
                    "instanceId": TEST_INSTANCE,
                    "badgeKey": TEST_KEY,
                },
            }),
            "exactly these keys: no principal, no name"
        );
        let key = badge_key("server-salt", "gina");
        for absent in ["title", "body", "sound", "threadId", "collapseId", "badge", "aps"] {
            assert!(p.get(absent).is_none(), "badge push carries {absent}: {p}");
        }
        assert_eq!(key.len(), 32);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{key}");
        assert!(!key.contains("gina"), "never the raw username");
        assert_eq!(key, badge_key("server-salt", " gina "), "stable per (server, person)");
        assert_ne!(key, badge_key("server-salt", "owner"), "per person");
        assert_ne!(key, badge_key("other-salt", "gina"), "per server");
        assert_eq!(e.budget_class(), budget::BudgetClass::Badge);
        assert_eq!(e.target_usernames(), Some(&["gina".to_string()][..]));
        assert!(!e.has_preview());
    }

    fn dev(token: &str) -> crate::push_devices::PushDevice {
        crate::push_devices::PushDevice {
            device_id: format!("d-{token}"),
            platform: "apns".into(),
            token: token.into(),
            username: "owner".into(),
            created_at: 1,
            updated_at: 1,
            last_seen_at: 1,
            previews: true,
        }
    }

    /// 0.46.1 privacy: no push `data`, of any kind, carries a person's name
    /// (the owner, a Connect username, an email-like username): pushes pass
    /// through K2's relay and Apple / Google. Whose push it is rides as the
    /// opaque `badgeKey`; there is no `principal` key at all.
    #[test]
    fn no_push_data_of_any_kind_carries_a_username() {
        for name in ["owner", "gina", "rosson.long@lztek.io"] {
            let key = badge_key("server-salt", name);
            let thread = match thread_event(ThreadMsgKind::Text, 1, "c-privacy") {
                PushEvent::ThreadPost(t) => PushEvent::ThreadPost(ThreadPush {
                    principal: name.into(),
                    device_usernames: vec![name.into()],
                    ..t
                }),
                _ => unreachable!(),
            };
            let events = vec![
                PushEvent::FeedbackCreated { agent_name: "cortana".into(), feedback_id: "f".into(), assignees: vec![name.into()] },
                PushEvent::FeedbackCommented { feedback_id: "f".into(), assignees: vec![name.into()] },
                PushEvent::ProjectMessageCreated { project_name: "Apollo".into(), group_id: "g".into() },
                project_post(name),
                badge_push(name),
                thread,
            ];
            let stamp = PushStamp { subdomain: "k2box".into(), instance_id: TEST_INSTANCE.into() };
            for e in &events {
                let p = K2Cloud::payload(e, &stamp, &key);
                let data = p["data"].as_object().expect("data");
                assert!(!data.contains_key("principal"), "{e:?}: principal in {p}");
                assert_eq!(data.get("badgeKey").and_then(|v| v.as_str()), Some(key.as_str()), "{e:?}: {p}");
                let wire = p.to_string();
                assert!(!wire.contains(name), "{e:?}: '{name}' rides the push: {wire}");
            }
        }
    }

    /// TP7: every `data` value of every kind is a string (the type makes a
    /// non-string impossible; this pins the wire form too).
    #[test]
    fn every_push_data_value_is_a_string() {
        let events = vec![
            sample_event(),
            PushEvent::HeartbeatFailed { heartbeat: "h".into(), reason: "r".into() },
            PushEvent::FeedbackCreated { agent_name: "a".into(), feedback_id: "f".into(), assignees: vec![] },
            PushEvent::FeedbackCommented { feedback_id: "f".into(), assignees: vec!["gina".into()] },
            PushEvent::ProjectMessageCreated { project_name: "p".into(), group_id: "g".into() },
            project_post("gina"),
            badge_push("gina"),
            thread_event(ThreadMsgKind::Text, 1, "c"),
            thread_event(ThreadMsgKind::Secret, 3, "c"),
        ];
        let stamp = PushStamp { subdomain: "rosson".into(), instance_id: TEST_INSTANCE.into() };
        for e in &events {
            let payload = K2Cloud::payload(e, &stamp, TEST_KEY);
            let data = payload["data"].as_object().expect("data object");
            for (k, v) in data {
                assert!(v.is_string(), "{e:?}: data.{k} = {v} is not a string");
            }
        }
    }

    #[test]
    fn thread_push_payload_is_name_only_with_grouping_and_collapse() {
        let cid = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let e = thread_event(ThreadMsgKind::Text, 1, cid);
        let stamp = PushStamp { subdomain: "rosson".into(), instance_id: TEST_INSTANCE.into() };
        let p = K2Cloud::payload(&e, &stamp, TEST_KEY);
        assert_eq!(
            p,
            serde_json::json!({
                "title": "Cortana",
                "body": "New message in Thread",
                "sound": "default",
                "threadId": format!("thread:{cid}"),
                "collapseId": format!("thread:{cid}"),
                "data": {
                    "kind": "thread",
                    "subdomain": "rosson",
                    "instanceId": TEST_INSTANCE,
                    "addr": "k2/reviewer",
                    "conversationId": cid,
                    "seq": "418",
                    "projectId": "proj-1",
                    "agentName": "Cortana",
                    "msgKind": "text",
                    "count": "1",
                    "badgeKey": TEST_KEY,
                    "unreadTotal": "4",
                    "waitingTotal": "1",
                    "avatarRev": "ba7816bf8f01cfea",
                    "agentColor": "#3b82f6",
                },
            })
        );
        assert!(p.get("badge").is_none() && p.get("aps").is_none(), "no badge (TP27)");
        // Bodies by kind and count; a secret card says nothing more.
        assert_eq!(thread_event(ThreadMsgKind::Choice, 1, cid).body(), "Asks you to choose");
        assert_eq!(thread_event(ThreadMsgKind::Secret, 1, cid).body(), "Asks for a secret");
        assert_eq!(thread_event(ThreadMsgKind::Secret, 3, cid).body(), "3 new messages");
        let secret = K2Cloud::payload(&thread_event(ThreadMsgKind::Secret, 1, cid), &stamp, TEST_KEY);
        assert_eq!(secret["data"]["msgKind"], "secret");
        assert_eq!(secret["data"].as_object().expect("data").len(), 15, "no extra secret fields");
        // A Thread push is always targeted, never "every device".
        let empty = PushEvent::ThreadPost(ThreadPush {
            device_usernames: vec![],
            ..match thread_event(ThreadMsgKind::Text, 1, cid) {
                PushEvent::ThreadPost(t) => t,
                _ => unreachable!(),
            }
        });
        assert_eq!(empty.target_usernames(), Some(&[][..]));
        assert_eq!(empty.budget_class(), budget::BudgetClass::Thread);
    }

    /// Communication Notifications: `avatarRev` + `agentColor` ride every
    /// Thread push as strings; no avatar is `""` with the key kept. Other
    /// kinds never carry them.
    #[test]
    fn thread_push_data_carries_avatar_rev_and_agent_color() {
        let cid = "c-avatar";
        let with = thread_event(ThreadMsgKind::Choice, 1, cid).data();
        assert_eq!(with.get("avatarRev"), Some("ba7816bf8f01cfea"));
        assert_eq!(with.get("agentColor"), Some("#3b82f6"));
        for key in ["kind", "addr", "conversationId", "projectId"] {
            assert!(with.get(key).is_some(), "existing key {key} kept: {with:?}");
        }
        let bare = PushEvent::ThreadPost(ThreadPush {
            avatar_rev: String::new(),
            ..match thread_event(ThreadMsgKind::Text, 1, cid) {
                PushEvent::ThreadPost(t) => t,
                _ => unreachable!(),
            }
        });
        let stamp = PushStamp { subdomain: "rosson".into(), instance_id: TEST_INSTANCE.into() };
        let p = K2Cloud::payload(&bare, &stamp, TEST_KEY);
        assert_eq!(p["data"]["avatarRev"], serde_json::json!(""), "no avatar = empty string, key kept");
        assert_eq!(p["data"]["agentColor"], serde_json::json!("#3b82f6"));
        let ticket = PushEvent::FeedbackCommented { feedback_id: "f".into(), assignees: vec![] }.data();
        assert_eq!(ticket.get("avatarRev"), None);
        assert_eq!(ticket.get("agentColor"), None);
    }

    #[test]
    fn collapse_id_never_exceeds_64_bytes() {
        let long = "x".repeat(200);
        let a = thread_event(ThreadMsgKind::Text, 1, &long).collapse_id().expect("collapse");
        let b = thread_event(ThreadMsgKind::Text, 1, &format!("{long}y")).collapse_id().expect("collapse");
        assert!(a.len() <= COLLAPSE_ID_MAX && a.starts_with("thread:"), "{a}");
        assert_ne!(a, b, "long ids hash, not cut");
        let ticket = PushEvent::FeedbackCommented { feedback_id: "f".into(), assignees: vec![] };
        assert_eq!(ticket.collapse_id(), None);
        assert_eq!(ticket.thread_id(), None);
    }

    #[test]
    fn gateway_config_defaults_to_push_k2_dev_when_paired() {
        let c = GatewayConfig::resolve(None, None, Some("tun".into()), Some(" Rosson ".into()))
            .expect("a paired tunnel is enough");
        assert_eq!(c.url, DEFAULT_GATEWAY_URL);
        assert_eq!(c.tunnel, Some(("tun".to_string(), "rosson".to_string())));
        assert_eq!(c.shared_token, None);
        assert_eq!(c.subdomain, "rosson");
        // No tunnel and no shared token: dormant, even with a URL.
        assert!(GatewayConfig::resolve(Some("https://x".into()), None, None, Some("sub".into())).is_none());
        // A subdomain without a token is not a tunnel (TP25: dormant alone).
        assert!(GatewayConfig::resolve(None, None, Some(" ".into()), Some("sub".into())).is_none());
        // Shared only: today's gate.
        let s = GatewayConfig::resolve(Some("https://push.k2.dev/push/dispatch".into()), Some("gw".into()), None, None)
            .expect("shared");
        assert_eq!((s.url.as_str(), s.tunnel.is_none()), ("https://push.k2.dev", true));
        // An explicit URL overrides the default for a paired daemon.
        let o = GatewayConfig::resolve(Some("https://staging/".into()), None, Some("t".into()), Some("s".into()))
            .expect("override");
        assert_eq!(o.url, "https://staging");
    }

    /// The relay state is process-global; these cases run in ONE test so
    /// they never race each other.
    #[test]
    fn per_daemon_auth_falls_back_only_for_an_unmarked_refusal() {
        set_instance_id(TEST_INSTANCE);
        let both = GatewayConfig::resolve(None, Some("shared-gw".into()), Some("tun-tok".into()), Some("rosson".into()))
            .expect("config");
        let tunnel_auth = GatewayAuth::Tunnel { token: "tun-tok".into(), subdomain: "rosson".into() };
        let shared_auth = GatewayAuth::Shared { token: "shared-gw".into() };
        let ev = thread_event(ThreadMsgKind::Text, 1, "c-auth");
        let refused = |status: u16, s0: bool, code: &str| GatewayResponse {
            status,
            body: serde_json::json!({ "error": { "code": code } }).to_string(),
            s0,
        };
        let ok = || GatewayResponse { status: 200, body: r#"{"results":[]}"#.into(), s0: true };
        let now = unix_now();

        // 1. Accepted: one call, tunnel token + X-K2-Subdomain.
        reset_relay_state_for_test();
        let s = MockSender::scripted(vec![ok()]);
        K2Cloud::with_config(both.clone(), s.clone()).dispatch(&ev, "owner", &[dev("t1")]).expect("send");
        let calls = s.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, tunnel_auth);
        assert_eq!(calls[0].1.subdomain_header(), Some("rosson"));
        assert_eq!(RelayPath::of(&both, now), RelayPath::PerDaemon);
        assert_eq!(RelayPath::of(&both, now).thread_per_hour(), budget::THREAD_PER_HOUR);
        assert_eq!(PushStatus::of(Some(&both), false, now), PushStatus::Ready);

        // 2. Old gateway (no S0 marker on a 403): fall back to the shared
        //    token for the same body, then stay on it with the small bucket.
        reset_relay_state_for_test();
        let s = MockSender::scripted(vec![refused(403, false, "bad bearer"), ok(), ok()]);
        let cloud = K2Cloud::with_config(both.clone(), s.clone());
        cloud.dispatch(&ev, "owner", &[dev("t2")]).expect("old gateway: shared fallback delivers");
        let calls = s.calls();
        assert_eq!(calls.len(), 2, "tunnel try + shared retry");
        assert_eq!((calls[0].1.clone(), calls[1].1.clone()), (tunnel_auth.clone(), shared_auth.clone()));
        assert_eq!(calls[0].2, calls[1].2, "the same body");
        assert_eq!(RelayPath::of(&both, now), RelayPath::Shared);
        assert_eq!(RelayPath::of(&both, now).thread_per_hour(), budget::THREAD_PER_HOUR_SHARED);
        assert_eq!(PushStatus::of(Some(&both), false, now), PushStatus::Ready);
        cloud.dispatch(&ev, "owner", &[dev("t2")]).expect("second send");
        assert_eq!(s.calls()[2].1, shared_auth, "within the retry window the shared token goes first");
        assert_eq!(RelayPath::of(&both, now + TUNNEL_RETRY_SECS), RelayPath::PerDaemon, "retried after an hour");

        // 3. A marked 403 (subdomain_not_owned / suspended): NO fallback,
        //    nothing more sent, status dormant with the reason.
        reset_relay_state_for_test();
        let s = MockSender::scripted(vec![refused(403, true, "subdomain_not_owned")]);
        let cloud = K2Cloud::with_config(both.clone(), s.clone());
        let err = cloud.dispatch(&ev, "owner", &[dev("t3")]).expect_err("rejected");
        assert!(matches!(err, PushError::Remote { status: 403, .. }), "{err}");
        assert_eq!(s.calls().len(), 1, "the shared token is never tried after a marked 403");
        let status = PushStatus::of(Some(&both), false, now);
        assert_eq!(status.as_wire(), "dormant");
        assert!(status.reason().expect("reason").contains("subdomain_not_owned"), "{status:?}");
        cloud.dispatch(&ev, "owner", &[dev("t3")]).expect_err("still rejected");
        assert_eq!(s.calls().len(), 1, "while rejected, nothing is sent at all");
        assert_eq!(PushStatus::of(Some(&both), false, now + TUNNEL_RETRY_SECS), PushStatus::Ready);

        // 4. A 400 never falls back, marked or not.
        for s0 in [false, true] {
            reset_relay_state_for_test();
            let s = MockSender::scripted(vec![refused(400, s0, "invalid_body")]);
            K2Cloud::with_config(both.clone(), s.clone())
                .dispatch(&ev, "owner", &[dev("t4")])
                .expect_err("400 is an error");
            assert_eq!(s.calls().len(), 1, "no fallback on 400 (s0={s0})");
            assert_eq!(RelayPath::of(&both, now), RelayPath::PerDaemon, "a 400 says nothing about auth");
        }

        // 5. Air-gap wins over everything in the status.
        assert_eq!(PushStatus::of(Some(&both), true, now), PushStatus::Airgap);
        assert_eq!(PushStatus::of(None, false, now).as_wire(), "dormant");
        reset_relay_state_for_test();
    }

    #[test]
    fn gateway_response_reason_reads_the_error_code() {
        let r = |b: &str| GatewayResponse { status: 403, body: b.into(), s0: true }.reason();
        assert_eq!(r(r#"{"error":{"code":"suspended"}}"#), "suspended");
        assert_eq!(r(r#"{"error":"unknown_token"}"#), "unknown_token");
        assert_eq!(r(r#"{"code":"subdomain_not_owned"}"#), "subdomain_not_owned");
        assert_eq!(r("not json"), "HTTP 403");
    }

    #[test]
    fn dispatch_splits_at_the_gateway_device_cap() {
        let s = MockSender::new(serde_json::json!({ "results": [] }));
        let cloud = K2Cloud::with_sender("https://push.k2.dev", "gw", s.clone());
        let devices: Vec<_> = (0..(MAX_DEVICES_PER_CALL + 1)).map(|i| dev(&format!("split-{i}"))).collect();
        cloud.dispatch(&thread_event(ThreadMsgKind::Text, 1, "c-split"), "owner", &devices).expect("send");
        let calls = s.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].2["devices"].as_array().expect("devices").len(), MAX_DEVICES_PER_CALL);
        assert_eq!(calls[1].2["devices"].as_array().expect("devices").len(), 1);
        cloud.dispatch(&thread_event(ThreadMsgKind::Text, 1, "c-split"), "owner", &[]).expect("no devices");
        assert_eq!(s.calls().len(), 2, "no devices, no call");
    }

    // ── Thread push previews (0.46.1) ────────────────────────────────

    fn previewed(kind: ThreadMsgKind, count: u32, preview: &str) -> PushEvent {
        match thread_event(kind, count, "c-preview") {
            PushEvent::ThreadPost(t) => PushEvent::ThreadPost(ThreadPush { preview: preview.into(), ..t }),
            _ => unreachable!(),
        }
    }

    fn dev_previews(token: &str, previews: bool) -> crate::push_devices::PushDevice {
        crate::push_devices::PushDevice { previews, ..dev(token) }
    }

    /// One text post shows its preview; a choice card shows its question;
    /// a secret card NEVER shows content, even if a preview was set; an
    /// empty preview falls back to the generic line.
    #[test]
    fn thread_bodies_show_the_preview_except_for_secrets() {
        let text = "Build is green. Ready to ship.";
        assert_eq!(previewed(ThreadMsgKind::Text, 1, text).body(), text);
        assert_eq!(
            previewed(ThreadMsgKind::Choice, 1, "Ship 0.46.1 tonight?").body(),
            "Ship 0.46.1 tonight?"
        );
        let secret = previewed(ThreadMsgKind::Secret, 1, "the token is hunter2");
        assert_eq!(secret.body(), "Asks for a secret");
        assert!(!secret.has_preview());
        let stamp = PushStamp { subdomain: "rosson".into(), instance_id: TEST_INSTANCE.into() };
        assert!(
            !K2Cloud::payload(&secret, &stamp, TEST_KEY).to_string().contains("hunter2"),
            "a secret's text never rides a payload"
        );
        assert_eq!(previewed(ThreadMsgKind::Secret, 3, "hunter2").body(), "3 new messages");
        assert_eq!(previewed(ThreadMsgKind::Text, 1, "   ").body(), "New message in Thread");
        assert_eq!(previewed(ThreadMsgKind::Choice, 1, "").body(), "Asks you to choose");
        // The data object is unchanged by a preview (no text in `data`).
        let p = K2Cloud::payload(&previewed(ThreadMsgKind::Text, 1, text), &stamp, TEST_KEY);
        assert_eq!(p["body"], text);
        assert!(!p["data"].to_string().contains("Ready to ship"), "the preview rides `body` only");
        // 13 keys + thread-read-v1's unreadTotal / waitingTotal (numbers, never text).
        assert_eq!(p["data"].as_object().expect("data").len(), 15);
        assert_eq!((p["data"]["unreadTotal"].as_str(), p["data"]["waitingTotal"].as_str()), (Some("4"), Some("1")));
    }

    /// Coalesced: "N new messages · <latest preview>", 140 chars at most;
    /// no preview → today's "N new messages".
    #[test]
    fn coalesced_bodies_lead_with_the_count_and_fit_140() {
        assert_eq!(
            previewed(ThreadMsgKind::Text, 4, "All tests pass.").body(),
            "4 new messages · All tests pass."
        );
        assert_eq!(previewed(ThreadMsgKind::Text, 4, "").body(), "4 new messages");
        let long = preview::message_preview(&"lorem ipsum ".repeat(30));
        assert!(long.chars().count() > preview::PREVIEW_MAX_CHARS - 12 && long.ends_with('…'), "{long}");
        let body = previewed(ThreadMsgKind::Choice, 12, &long).body();
        assert!(body.starts_with("12 new messages · lorem ipsum"), "{body}");
        assert!(body.chars().count() <= preview::PREVIEW_MAX_CHARS, "{} chars: {body}", body.chars().count());
        assert!(body.ends_with('…'), "{body}");
    }

    /// Per-device previews: one push with a preview splits into the
    /// preview payload (devices with previews on) and the generic payload
    /// (devices with previews off) — two gateway calls, each device in
    /// exactly one. All on / all off / no preview → one call.
    #[test]
    fn per_device_previews_split_the_payloads() {
        let text = "Deployed to staging. Smoke tests next.";
        let event = previewed(ThreadMsgKind::Text, 1, text);
        let devices = vec![
            dev_previews("pv-on-1", true),
            dev_previews("pv-off-1", false),
            dev_previews("pv-on-2", true),
        ];
        let s = MockSender::new(serde_json::json!({ "results": [] }));
        let cloud = K2Cloud::with_sender("https://push.k2.dev", "gw", s.clone());
        cloud.dispatch(&event, "owner", &devices).expect("send");
        let calls = s.calls();
        assert_eq!(calls.len(), 2, "one call per payload variant");
        let tokens = |c: &serde_json::Value| -> Vec<String> {
            c["devices"].as_array().expect("devices").iter().map(|d| d["token"].as_str().expect("t").to_string()).collect()
        };
        assert_eq!(calls[0].2["payload"]["body"], text);
        assert_eq!(tokens(&calls[0].2), vec!["pv-on-1", "pv-on-2"]);
        assert_eq!(calls[1].2["payload"]["body"], "New message in Thread");
        assert_eq!(tokens(&calls[1].2), vec!["pv-off-1"]);
        assert!(!calls[1].2.to_string().contains("staging"), "no text reaches a previews-off device");
        // Same collapse id, so either banner replaces the last.
        assert_eq!(calls[0].2["payload"]["collapseId"], calls[1].2["payload"]["collapseId"]);

        assert_eq!(K2Cloud::payload_groups(&event, &devices[..1]).len(), 1, "all on: one call");
        let off = [dev_previews("x", false), dev_previews("y", false)];
        let groups = K2Cloud::payload_groups(&event, &off);
        assert_eq!(groups.len(), 1, "all off: one call");
        assert_eq!(groups[0].0.body(), "New message in Thread");
        let plain = thread_event(ThreadMsgKind::Text, 1, "c-preview");
        assert_eq!(K2Cloud::payload_groups(&plain, &devices).len(), 1, "no preview: one call for everyone");
        assert!(K2Cloud::payload_groups(&event, &[]).is_empty());
        // A secret never splits: nothing to hide.
        let secret = previewed(ThreadMsgKind::Secret, 1, "hunter2");
        assert_eq!(K2Cloud::payload_groups(&secret, &devices).len(), 1);
    }

    /// TP29: one dispatch per person, each saying whose it is; nobody's
    /// devices or name ride another person's call.
    #[test]
    fn untargeted_send_is_one_dispatch_per_login_with_its_principal() {
        let _db = crate::db::scoped_for_test();
        crate::push_devices::upsert_device("p-own", "apns", "tok-own", "owner").expect("owner device");
        crate::push_devices::upsert_device("p-gina", "fcm", "tok-gina", "gina").expect("gina device");
        let s = MockSender::new(serde_json::json!({ "results": [] }));
        K2Cloud::with_sender("https://push.k2.dev", "gw", s.clone())
            .send(&PushEvent::ProjectMessageCreated { project_name: "Apollo".into(), group_id: "g".into() })
            .expect("send");
        let calls = s.calls();
        assert_eq!(calls.len(), 2, "one call per person");
        let owner_key = crate::read_state::badge_key_for_shared("owner");
        let gina_key = crate::read_state::badge_key_for_shared("gina");
        assert_ne!(owner_key, gina_key);
        for (_, _, body) in &calls {
            assert!(body["payload"]["data"].get("principal").is_none(), "never a name: {body}");
            let key = body["payload"]["data"]["badgeKey"].as_str().expect("badgeKey");
            let principal = if key == owner_key { "owner" } else { assert_eq!(key, gina_key); "gina" };
            let tokens: Vec<&str> = body["devices"]
                .as_array()
                .expect("devices")
                .iter()
                .map(|d| d["token"].as_str().expect("token"))
                .collect();
            assert_eq!(tokens, vec![format!("tok-{}", if principal == "owner" { "own" } else { "gina" })]);
            let other = if principal == "owner" { "gina" } else { "owner" };
            assert!(!body.to_string().contains(&format!("\"{other}\"")), "{other} leaked into {principal}'s call");
        }
    }
}
