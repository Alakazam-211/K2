//! External agents: agent products that run on this computer but are not
//! K2 workspaces (no PTY, no session, no `.k2`). K2 detects them, lists
//! them next to its own agents, reads their chat and — only when the owner
//! turns it on — sends a message to them as the human.
//!
//! One adapter per product, behind [`ExternalAgentProvider`], so the daemon
//! routes, the watcher and the renderer never learn a product's internals.
//! The first (and today only) provider is Grok Bot ([`grokbot`]).
//!
//! Rules every provider keeps:
//! - **Same OS user only.** Read the daemon user's own home. Never scan
//!   other homes.
//! - **Secrets stay put.** A product's local API token is used for loopback
//!   calls on this box and is never logged, returned, sent off the box, or
//!   carried into federation.
//! - **Fail soft.** A product changes its internals without notice. An
//!   unknown layout lists the agent and answers the thread with
//!   [`ThreadPage::unsupported`] ("Open in Grok Bot") instead of an error.
//! - **Off by default.** Nothing here runs unless the per-server setting
//!   ([`settings`]) says so; the daemon checks it before every call.

pub mod grokbot;
pub mod live_tail;
pub mod rate_limit;
pub mod settings;

use serde::{Deserialize, Serialize};

/// One agent of an external product, as K2 shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAgent {
    /// The product's own id for the agent (Grok Bot: a UUID).
    pub id: String,
    /// Provider kind (`"grokbot"`). The renderer's tag keys on it.
    pub kind: &'static str,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `data:image/...;base64,...` or an `https://` URL; never a local path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    /// The product's avatar colour (shown when there is no image).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Where the row came from: `"gateway"` (live roster) or `"disk"`
    /// (profile fallback while the product is not running).
    pub source: &'static str,
    /// Running a turn or composing a message right now.
    pub working: bool,
    /// The agent is waiting on the human (a pending question/card).
    pub needs_you: bool,
    /// Unread lines in the product's own app.
    pub unread_count: u32,
    /// The agent's live conversation runs outside this computer (another
    /// box, or the product's cloud): the local chat store is behind, so
    /// K2 shows its history as old and adds the live tail
    /// ([`live_tail`]) after it. Sends still go through: the product's
    /// gateway delivers them to the live conversation wherever it runs.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
    /// The product's own one-line preview of the latest message (plain
    /// text, capped). Current even when the local chat store is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_preview: Option<String>,
    /// Latest activity anywhere (ms since epoch).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<i64>,
    /// Latest activity on this computer (ms since epoch): how far the local
    /// chat store goes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_activity_at: Option<i64>,
    /// Change marker for the agent's chat (product-specific; compared for
    /// equality only). Never sent to clients.
    #[serde(skip)]
    pub rev: String,
    /// The roster's summary of the agent's latest entry (feeds the live
    /// tail of a stale agent). `None` for the disk roster. Never sent to
    /// clients as is.
    #[serde(skip)]
    pub live: Option<live_tail::LiveSummary>,
}

/// Box-wide status. Grok Bot runs one turn at a time for the whole box, so
/// "working" is a single active agent id, not a per-agent flag.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    /// The product's local service answered.
    pub reachable: bool,
    pub busy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_busy_at_ms: Option<i64>,
}

/// Who wrote a chat line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    /// The human (a user turn).
    Human,
    /// The external agent itself.
    Agent,
    /// Another agent writing to this one (inbound peer message).
    Peer,
}

/// A named agent reference on a chat line (peer chips).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentRef {
    pub id: String,
    pub name: String,
}

/// One human-visible chat line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadMessage {
    /// Monotonic cursor (SQLite rowid). Pass the last one back as `since`.
    pub seq: i64,
    pub author: Author,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<i64>,
    /// Outbound peer chip (`toAgent`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_agent: Option<AgentRef>,
    /// Inbound peer chip (`fromAgent`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_agent: Option<AgentRef>,
    /// The line was sent from K2 (its client nonce carries K2's mark).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub via_k2: bool,
    /// Set when the line is something K2 can't show (a widget, connector,
    /// attachment, …): the value names it and the client offers "Open in
    /// Grok Bot" instead of the text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_in_app: Option<String>,
    /// A read-only card for an interactive line (a question, a connect
    /// request, a credential request, a shared template). Answered in the
    /// product's own app, never through K2; never carries a secret value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<Card>,
}

/// One choice on a [`Card::Choice`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardOption {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The human picked this one.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub chosen: bool,
}

/// How the human dealt with a question card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CardState {
    Open,
    Answered,
    Skipped,
    Dismissed,
}

/// Read-only interactive lines (see [`ThreadMessage::card`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Card {
    /// A question with choice buttons.
    #[serde(rename_all = "camelCase")]
    Choice {
        prompt: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        help_text: Option<String>,
        options: Vec<CardOption>,
        state: CardState,
        /// Free-text answer that matched no option.
        #[serde(skip_serializing_if = "Option::is_none")]
        custom_answer: Option<String>,
    },
    /// "Connect this integration."
    #[serde(rename_all = "camelCase")]
    Connector {
        connector: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        variant: Option<String>,
    },
    /// A credential request. Only whether it was provided — never a value.
    #[serde(rename_all = "camelCase")]
    SecretRequest {
        label: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        platform: Option<String>,
        provided: bool,
    },
    /// A shared bot template.
    #[serde(rename_all = "camelCase")]
    TemplateShare {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        /// `https://` only.
        #[serde(skip_serializing_if = "Option::is_none")]
        share_url: Option<String>,
    },
}

/// One page of an agent's chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPage {
    /// `false` when the layout is one K2 doesn't know: the renderer shows
    /// "Open in Grok Bot" instead of messages.
    pub supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub messages: Vec<ThreadMessage>,
    /// Highest cursor in this page (or the `since` passed in when empty).
    pub next_since: i64,
    /// Older lines exist before this page (initial "latest N" load only).
    pub has_more_before: bool,
}

impl ThreadPage {
    pub fn unsupported(reason: impl Into<String>, since: i64) -> Self {
        Self {
            supported: false,
            reason: Some(reason.into()),
            messages: Vec::new(),
            next_since: since,
            has_more_before: false,
        }
    }
}

/// A send the product accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendReceipt {
    /// The client nonce K2 stamped on the send (`k2-<uuid>`).
    pub client_nonce: String,
    /// The agent was stale when the send went out: the caller records the
    /// send in the agent's live tail (its local chat store won't show it).
    #[serde(skip)]
    pub agent_stale: bool,
}

/// What detection found, for the Settings preview. Contains no secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detection {
    /// The product's install marker exists for the daemon's OS user.
    pub installed: bool,
    /// The data folder K2 would read (shown to the owner before enabling).
    pub data_path: String,
    /// Agents found on disk (names only from profile files).
    pub agents: Vec<ExternalAgent>,
}

/// A refusal with a stable wire code. Messages never carry a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtError {
    /// The product is not installed for this user.
    NotInstalled,
    /// No such agent.
    UnknownAgent(String),
    /// The product's local service is not reachable (or refused us).
    Unreachable(String),
    /// The product answered with something K2 doesn't understand.
    Protocol(String),
    /// Bad input from the caller.
    BadInput(String),
}

impl ExtError {
    pub fn code(&self) -> &'static str {
        match self {
            ExtError::NotInstalled => "not_installed",
            ExtError::UnknownAgent(_) => "unknown_agent",
            ExtError::Unreachable(_) => "unreachable",
            ExtError::Protocol(_) => "protocol",
            ExtError::BadInput(_) => "bad_input",
        }
    }
}

impl std::fmt::Display for ExtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtError::NotInstalled => write!(f, "not installed for this user"),
            ExtError::UnknownAgent(id) => write!(f, "no such agent: {id}"),
            ExtError::Unreachable(m) => write!(f, "not reachable: {m}"),
            ExtError::Protocol(m) => write!(f, "unexpected answer: {m}"),
            ExtError::BadInput(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ExtError {}

/// The contract a local agent product's adapter implements. All calls are
/// blocking (loopback HTTP + local files); the daemon runs them off its
/// async workers.
pub trait ExternalAgentProvider: Send + Sync {
    /// Wire kind, e.g. `"grokbot"`.
    fn kind(&self) -> &'static str;
    /// Human label, e.g. `"Grok Bot"`.
    fn label(&self) -> &'static str;
    /// Install marker + disk roster. Reads no chat and uses no token.
    fn detect(&self) -> Detection;
    /// Live roster, falling back to the disk roster when the product's
    /// service is down.
    fn list_agents(&self) -> Result<Vec<ExternalAgent>, ExtError>;
    /// Box-wide status. Never fails: an unreachable service is
    /// `reachable: false`.
    fn status(&self) -> ProviderStatus;
    /// A page of an agent's human-visible chat. `since = None` returns the
    /// latest `limit` lines; `Some(seq)` returns lines after `seq`.
    fn read_thread(
        &self,
        agent_id: &str,
        since: Option<i64>,
        limit: usize,
    ) -> Result<ThreadPage, ExtError>;
    /// Cheap change marker for an agent's chat (highest cursor), used by
    /// the watcher to decide whether to tell clients. `None` = unreadable.
    fn thread_cursor(&self, agent_id: &str) -> Option<i64>;
    /// Files whose change means the agent's chat changed (watch targets).
    fn watch_paths(&self, agent_id: &str) -> Vec<std::path::PathBuf>;
    /// Send `text` to the agent as the human. Runs a full (billed) turn.
    fn send(&self, agent_id: &str, text: &str) -> Result<SendReceipt, ExtError>;
}

/// Agent ids are used in file paths: accept only a conservative alphabet so
/// a crafted id can never walk out of the product's agents folder.
pub fn valid_agent_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_ids_cannot_walk_paths() {
        assert!(valid_agent_id("0e5f5de8-7619-4ba4-9753-32c5470b2346"));
        assert!(!valid_agent_id(""));
        assert!(!valid_agent_id(".."));
        assert!(!valid_agent_id("a/b"));
        assert!(!valid_agent_id("a\\b"));
        assert!(!valid_agent_id(&"a".repeat(129)));
    }
}
