//! Sew chat for agents — the pure half of `k2 chat` (slice S30 core, K16).
//!
//! An agent uses Sew chat through K2 under two locks
//! (plan-sew-s30-s32-on-k2-v1 §1, `design-k2-chat-hostchat-cli-v1-help.txt`):
//! 1. **Sew roles decide WHERE.** The daemon calls Sew as the agent (act-as);
//!    Sew's permissions and its masked 404s apply in full.
//! 2. **K2 decides WHAT, per Space.** A [`Level`] (`read < reply < send <
//!    manage`) and an [`ApprovalMode`] (`none | required`) per (agent
//!    workspace, Space) in `sew_chat_access`.
//!
//! This module holds the words, the pure rules (levels, the addressed
//! rules, limits, key and body checks) and the SQLite store ([`store`]).
//! Routes, caller identity and the Sew seam live in k2-daemon
//! (`sew_chat::routes`).

/// The chat delivery hub's pure half and store (S31 K22).
pub mod hub;
pub mod person;
pub mod store;
/// What agents are taught about `k2 chat` / `k2 hostchat` (K21 gates).
pub mod teach;

use sha2::{Digest, Sha256};

/// Largest message body (bytes, UTF-8).
pub const BODY_MAX: usize = 16 * 1024;
/// Content writes (send, reply, dm, edit) per workspace per minute.
pub const CONTENT_PER_MIN: i64 = 20;
/// Content writes per workspace per hour.
pub const CONTENT_PER_HOUR: i64 = 300;
/// Light writes (delete) per workspace per minute.
pub const LIGHT_PER_MIN: i64 = 60;
/// Queued items per workspace.
pub const QUEUE_CAP: i64 = 20;
/// A queued item is denied by `system:expired` after this long (chat goes
/// stale fast; mail keeps 7 days).
pub const APPROVAL_TTL_SECS: i64 = 24 * 3600;
/// The approver's preview of a queued body (characters).
pub const PREVIEW_CHARS: usize = 280;
/// Idempotency keys: 1–48 of `[A-Za-z0-9_-]`.
pub const KEY_MAX: usize = 48;
/// Concurrent waits per workspace.
pub const WAITS_PER_WORKSPACE: usize = 4;
/// `wait` default and maximum (seconds): CLI `--timeout` default 15m, max 1h.
pub const WAIT_DEFAULT_SECS: u64 = 15 * 60;
pub const WAIT_MAX_SECS: u64 = 60 * 60;
/// One Sew long-poll (`wait_ms`, Sew G3) is at most this long.
pub const SEW_WAIT_MS_MAX: u64 = 25_000;

/// The settings key for who may approve queued posts.
pub const POLICY_APPROVALS_BY: &str = "approvals.by";

/// Vault confirm action for one approval (`secretId` = the `out_…` id).
pub const CONFIRM_APPROVE: &str = "chat-approve";
/// Vault confirm action for a batch (`secretId` = [`batch_hash`]).
pub const CONFIRM_APPROVE_BATCH: &str = "chat-approve-batch";
/// Vault confirm action for a person approving, after the fact, a post an
/// assistant made on their behalf (`secretId` = the write's `act_…` id;
/// Rosson 2026-10-27: on-behalf posts never wait, the person reviews them).
pub const CONFIRM_BEHALF_APPROVE: &str = "chat-behalf-approve";

/// What an agent may do in one Space. Derived `Ord`: read < reply < send <
/// manage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// View only.
    Read,
    /// Reply under existing messages (also `--broadcast`).
    Reply,
    /// Post, reply and DM.
    Send,
    /// Also the community-management verbs (with the Manage Sew switch).
    Manage,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Read => "read",
            Level::Reply => "reply",
            Level::Send => "send",
            Level::Manage => "manage",
        }
    }

    /// Parse a level word. `post` is accepted as the old name of `send`
    /// (plan §1 S30 amend 5); anything else is an error, never a default.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "read" => Ok(Level::Read),
            "reply" => Ok(Level::Reply),
            "send" | "post" => Ok(Level::Send),
            "manage" => Ok(Level::Manage),
            other => Err(format!("level must be read, reply, send or manage (got '{other}')")),
        }
    }
}

/// Whether an agent's writes wait for a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Writes post at once.
    None,
    /// Every send/reply/dm/edit queues (`out_…`).
    Required,
}

impl ApprovalMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalMode::None => "none",
            ApprovalMode::Required => "required",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(ApprovalMode::None),
            "required" => Ok(ApprovalMode::Required),
            other => Err(format!("approval must be required or none (got '{other}')")),
        }
    }
}

/// Who may approve queued posts on this box (per-box admin policy, Rosson
/// 2026-10-15). Default [`ApprovalsBy::People`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalsBy {
    /// Only a person: the owner or an admin, with a vault confirm.
    People,
    /// Also agents whose workspace has Manage Sew on (labelled as an
    /// agent's approval, never person proof).
    PeopleAndAgents,
}

impl ApprovalsBy {
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalsBy::People => "people",
            ApprovalsBy::PeopleAndAgents => "people+agents",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            // `person_only` / `agents_allowed` are Rosson's words (2026-10-15).
            "people" | "person_only" => Ok(ApprovalsBy::People),
            "people+agents" | "agents_allowed" => Ok(ApprovalsBy::PeopleAndAgents),
            other => Err(format!("approvals.by must be people or people+agents (got '{other}')")),
        }
    }
}

/// A chat write. KM1 kinds (`answer|decline|void|ask`) have room in the
/// table but no verb until Sew KM1 is on the pinned tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Send,
    Reply,
    Dm,
    Edit,
    Delete,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Send => "send",
            Action::Reply => "reply",
            Action::Dm => "dm",
            Action::Edit => "edit",
            Action::Delete => "delete",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "send" => Action::Send,
            "reply" => Action::Reply,
            "dm" => Action::Dm,
            "edit" => Action::Edit,
            "delete" => Action::Delete,
            _ => return None,
        })
    }

    /// Content writes carry a body, count toward the content limit and
    /// queue under `approval required`. A delete does neither.
    pub fn is_content(self) -> bool {
        !matches!(self, Action::Delete)
    }
}

/// The level a write needs. `edit` needs the level that would create the
/// edited message now (a top-level post needs `send`, a reply `reply`);
/// deleting your own message needs `reply`.
pub fn level_needed(action: Action, target_is_reply: bool) -> Level {
    match action {
        Action::Send | Action::Dm => Level::Send,
        Action::Reply | Action::Delete => Level::Reply,
        Action::Edit => {
            if target_is_reply {
                Level::Reply
            } else {
                Level::Send
            }
        }
    }
}

/// Why a write target counts as addressed to the agent (Sew S30 §7.3; on
/// Sew's per-viewer `mentions_me`, G6). Recorded on the action row; it is
/// what a `wait --mention` and the delivery hub (S31) key on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addressed {
    /// A1: a DM/group DM the agent is a member of.
    DmMember,
    /// A2: the thread root mentions the agent (direct, role or group).
    RootMentionsMe,
    /// A3: the agent follows the thread.
    Following,
    /// A4: the agent started the thread.
    Started,
    /// A5: one of the newest replies mentions the agent.
    RecentReplyMentionsMe,
}

impl Addressed {
    pub fn as_str(self) -> &'static str {
        match self {
            Addressed::DmMember => "dm_member",
            Addressed::RootMentionsMe => "mentioned",
            Addressed::Following => "following",
            Addressed::Started => "started",
            Addressed::RecentReplyMentionsMe => "recent_mention",
        }
    }
}

/// What the Sew reads say about a target, for [`addressed`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AddressFacts {
    pub dm_member: bool,
    pub root_mentions_me: bool,
    pub following: bool,
    pub root_author_is_me: bool,
    pub recent_reply_mentions_me: bool,
}

/// The first addressed rule that holds, in A1..A5 order. A top-level post
/// in a Space channel and `@here`/`@channel` never count (Sew's
/// `mentions_me` excludes them).
pub fn addressed(f: &AddressFacts) -> Option<Addressed> {
    if f.dm_member {
        Some(Addressed::DmMember)
    } else if f.root_mentions_me {
        Some(Addressed::RootMentionsMe)
    } else if f.following {
        Some(Addressed::Following)
    } else if f.root_author_is_me {
        Some(Addressed::Started)
    } else if f.recent_reply_mentions_me {
        Some(Addressed::RecentReplyMentionsMe)
    } else {
        None
    }
}

/// Check an idempotency key.
pub fn check_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > KEY_MAX || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(format!("key must be 1–{KEY_MAX} characters of A-Z a-z 0-9 _ -"));
    }
    Ok(())
}

/// Check a message body: 1..=[`BODY_MAX`] bytes, not only whitespace.
pub fn check_body(text: &str) -> Result<(), BodyError> {
    if text.trim().is_empty() {
        return Err(BodyError::Empty);
    }
    if text.len() > BODY_MAX {
        return Err(BodyError::TooLarge);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyError {
    Empty,
    TooLarge,
}

/// Lowercase hex sha256 of a body (the only trace of content K2 keeps).
pub fn body_sha256(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

/// The `secretId` a `chat-approve-batch` confirm is bound to:
/// `sha256_hex(join("\n", sort(ids)))`.
pub fn batch_hash(ids: &[String]) -> String {
    let mut v: Vec<&str> = ids.iter().map(String::as_str).collect();
    v.sort_unstable();
    v.dedup();
    hex(&Sha256::digest(v.join("\n").as_bytes()))
}

/// First [`PREVIEW_CHARS`] characters of a body (for approvers only).
pub fn preview(text: &str) -> String {
    text.chars().take(PREVIEW_CHARS).collect()
}

/// A new random id with `prefix` (`out_`, `act_`): 24 lowercase hex.
pub fn new_id(prefix: &str) -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}{}", &u[..24])
}

/// A CLI-less caller's key (the daemon makes one when the body has none).
pub fn generated_key() -> String {
    new_id("k2c_")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Wall clock in seconds.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Parse a `wait --timeout` value: plain seconds or `30s`, `10m`, `2h`,
/// `1d`. Clamped to 1..=[`WAIT_MAX_SECS`]; absent = [`WAIT_DEFAULT_SECS`].
pub fn parse_timeout(s: Option<&str>) -> Result<u64, String> {
    let Some(s) = s.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(WAIT_DEFAULT_SECS);
    };
    let (num, mult) = match s.char_indices().last() {
        Some((i, 's')) => (&s[..i], 1),
        Some((i, 'm')) => (&s[..i], 60),
        Some((i, 'h')) => (&s[..i], 3600),
        Some((i, 'd')) => (&s[..i], 86_400),
        _ => (s, 1),
    };
    let n: u64 = num.parse().map_err(|_| format!("timeout must be like 30s, 10m or 1h (got '{s}')"))?;
    let secs = n.saturating_mul(mult);
    if secs == 0 {
        return Err("timeout must be at least 1s".into());
    }
    Ok(secs.min(WAIT_MAX_SECS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_order_parse_and_refuse_unknown_words() {
        assert!(Level::Read < Level::Reply && Level::Reply < Level::Send && Level::Send < Level::Manage);
        assert_eq!(Level::parse("post").unwrap(), Level::Send, "post is the old name of send");
        for l in [Level::Read, Level::Reply, Level::Send, Level::Manage] {
            assert_eq!(Level::parse(l.as_str()).unwrap(), l);
        }
        assert!(Level::parse("admin").is_err());
        assert!(Level::parse("").is_err());
        assert_eq!(ApprovalMode::parse("required").unwrap(), ApprovalMode::Required);
        assert!(ApprovalMode::parse("approval").is_err(), "the CLI words are required|none");
        assert_eq!(ApprovalsBy::parse("people+agents").unwrap(), ApprovalsBy::PeopleAndAgents);
        assert!(ApprovalsBy::parse("agents").is_err());
    }

    #[test]
    fn level_needed_per_action() {
        assert_eq!(level_needed(Action::Send, false), Level::Send);
        assert_eq!(level_needed(Action::Dm, false), Level::Send);
        assert_eq!(level_needed(Action::Reply, true), Level::Reply);
        assert_eq!(level_needed(Action::Edit, false), Level::Send, "editing a top-level post needs send now");
        assert_eq!(level_needed(Action::Edit, true), Level::Reply);
        assert_eq!(level_needed(Action::Delete, false), Level::Reply);
        assert!(!Action::Delete.is_content() && Action::Edit.is_content());
    }

    #[test]
    fn addressed_rules_in_order() {
        assert_eq!(addressed(&AddressFacts::default()), None, "a top-level post is never addressed");
        let f = AddressFacts { dm_member: true, root_mentions_me: true, ..Default::default() };
        assert_eq!(addressed(&f), Some(Addressed::DmMember));
        let f = AddressFacts { root_mentions_me: true, following: true, ..Default::default() };
        assert_eq!(addressed(&f), Some(Addressed::RootMentionsMe));
        let f = AddressFacts { following: true, ..Default::default() };
        assert_eq!(addressed(&f), Some(Addressed::Following));
        let f = AddressFacts { root_author_is_me: true, ..Default::default() };
        assert_eq!(addressed(&f), Some(Addressed::Started));
        let f = AddressFacts { recent_reply_mentions_me: true, ..Default::default() };
        assert_eq!(addressed(&f), Some(Addressed::RecentReplyMentionsMe));
    }

    #[test]
    fn keys_bodies_timeouts_and_hashes() {
        assert!(check_key("k2c_abc-1").is_ok());
        assert!(check_key("").is_err());
        assert!(check_key(&"a".repeat(49)).is_err());
        assert!(check_key("has space").is_err());
        assert_eq!(check_body("  "), Err(BodyError::Empty));
        assert_eq!(check_body(&"x".repeat(BODY_MAX + 1)), Err(BodyError::TooLarge));
        assert!(check_body(&"x".repeat(BODY_MAX)).is_ok());
        assert_eq!(parse_timeout(None).unwrap(), 900);
        assert_eq!(parse_timeout(Some("30s")).unwrap(), 30);
        assert_eq!(parse_timeout(Some("10m")).unwrap(), 600);
        assert_eq!(parse_timeout(Some("2h")).unwrap(), 3600, "clamped to 1h");
        assert_eq!(parse_timeout(Some("45")).unwrap(), 45);
        assert!(parse_timeout(Some("0")).is_err());
        assert!(parse_timeout(Some("soon")).is_err());
        assert_eq!(batch_hash(&["b".into(), "a".into()]), batch_hash(&["a".into(), "b".into()]));
        assert_ne!(batch_hash(&["a".into()]), batch_hash(&["a".into(), "b".into()]));
        let id = new_id("out_");
        assert!(id.starts_with("out_") && id.len() == 28, "{id}");
        assert_eq!(preview(&"é".repeat(400)).chars().count(), PREVIEW_CHARS);
    }
}
