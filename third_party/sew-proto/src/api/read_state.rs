//! Read cursors, unread and mention counts, the threads inbox and boot sync (C8; S14,
//! prd-sew-14 §15; contracts rev 29).
//!
//! Nothing here is a stored counter: every count is computed by the server from cursors,
//! pointers and mention rows at read time. Every read-state object carries `seq`, the member's
//! change-log head it is current as of; a client keeps the copy with the higher `seq`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::messages::{Message, ThreadSummary};
use crate::error::{Validate, ValidationError};
use crate::gateway::server::ThreadsInbox;
use crate::id::{Id, IdList};
use crate::limits::{
    BULK_ACK_CHANNELS_MAX, BULK_ACK_THREADS_MAX, PAGE_LIMIT_DEFAULT, PAGE_LIMIT_MAX,
};

fn is_false(b: &bool) -> bool {
    !*b
}

/// A member's read state in one channel (C8; prd-sew-14 §5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadState {
    /// The channel.
    pub channel_id: Id,
    /// The read cursor: a comparator, possibly a synthetic id (mark unread writes
    /// `message_id − 1`). Absent = nothing read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_read_id: Option<Id>,
    /// The channel's newest timeline message (omitted in READY, where the channel entry has
    /// it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_id: Option<Id>,
    /// Unread mentions (a muted channel counts direct mentions only).
    pub mention_count: u32,
    /// Whether a message by someone else is past the cursor.
    pub unread: bool,
    /// The member muted the channel (S21); present only when true.
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    /// The member change-log head this state is current as of (S14 gap G9).
    #[serde(default)]
    pub seq: u64,
}

/// A member's read state in one thread (prd-sew-14 §5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadReadState {
    /// The thread root.
    pub root_id: Id,
    /// The root's channel (always set by S14).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<Id>,
    /// The thread cursor; the root id when no reply was read (always set by S14).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_read_id: Option<Id>,
    /// The newest live reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reply_id: Option<Id>,
    /// Whether a reply is past the cursor.
    #[serde(default)]
    pub unread: bool,
    /// Live replies past the cursor (exact, no cap).
    #[serde(default)]
    pub unread_replies: u32,
    /// Unread mentions in the thread.
    #[serde(default)]
    pub mention_count: u32,
    /// Whether the member follows the thread.
    #[serde(default)]
    pub following: bool,
    /// The member change-log head this state is current as of.
    #[serde(default)]
    pub seq: u64,
}

/// Body of `POST /api/v1/channels/{id}/ack` and `POST /api/v1/threads/{root_id}/ack`: move
/// the cursor forward (never backward).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AckRequest {
    /// The newest message read; absent = the newest message now (S14 gap G18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Id>,
}

impl Validate for AckRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Body of `POST /api/v1/channels/{id}/mark_unread` and `/threads/{root_id}/mark_unread`:
/// the cursor moves back to just before `message_id` (S14 gap G3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MarkUnreadRequest {
    /// The first message to show as unread.
    pub message_id: Id,
}

impl Validate for MarkUnreadRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// One channel of a bulk ack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AckItem {
    /// The channel.
    pub channel_id: Id,
    /// The newest message read; absent = the newest now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Id>,
}

/// One thread of a bulk ack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadAckItem {
    /// The thread root.
    pub root_id: Id,
    /// The newest reply read; absent = the newest now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Id>,
}

/// Body of `POST /api/v1/me/read_states/ack` ("mark all read", prd-sew-14 §8.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BulkAckRequest {
    /// Channels, 0–500, unique.
    #[serde(default)]
    pub channels: Vec<AckItem>,
    /// Threads, 0–500, unique.
    #[serde(default)]
    pub threads: Vec<ThreadAckItem>,
    /// Also ack every thread of each listed channel the member follows or has an unread
    /// mention in.
    #[serde(default)]
    pub include_threads: bool,
}

impl Validate for BulkAckRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.channels.is_empty() && self.threads.is_empty() {
            return Err(ValidationError::invalid(
                "channels",
                "a bulk ack needs at least one channel or thread",
            ));
        }
        if self.channels.len() > BULK_ACK_CHANNELS_MAX {
            return Err(ValidationError::invalid(
                "channels",
                format!("at most {BULK_ACK_CHANNELS_MAX} channels"),
            ));
        }
        if self.threads.len() > BULK_ACK_THREADS_MAX {
            return Err(ValidationError::invalid(
                "threads",
                format!("at most {BULK_ACK_THREADS_MAX} threads"),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        if !self.channels.iter().all(|c| seen.insert(c.channel_id)) {
            return Err(ValidationError::invalid(
                "channels",
                "channel ids must be unique",
            ));
        }
        let mut seen = std::collections::HashSet::new();
        if !self.threads.iter().all(|t| seen.insert(t.root_id)) {
            return Err(ValidationError::invalid(
                "threads",
                "thread root ids must be unique",
            ));
        }
        Ok(())
    }
}

/// Response of `POST /api/v1/me/read_states/ack`, and the `read_states_bulk_updated` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BulkAckResponse {
    /// Every processed visible channel.
    pub read_states: Vec<ReadState>,
    /// Every processed thread.
    pub threads: Vec<ThreadReadState>,
    /// The threads inbox after the ack.
    pub threads_inbox: ThreadsInbox,
    /// More implied threads than `bulk_max_threads`: repeat the call.
    pub threads_truncated: bool,
}

/// Response of the thread ack and mark-unread routes, and the `thread_read_updated` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadReadUpdated {
    /// The thread's read state.
    pub thread: ThreadReadState,
    /// Exact inbox totals; absent means "per-thread change only".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox: Option<ThreadsInbox>,
}

/// Query of `GET /api/v1/me/read_states`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadStatesQuery {
    /// 1–100 channel ids; absent = every channel of the caller's sync set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_ids: Option<IdList>,
}

impl Validate for ReadStatesQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Query of `GET /api/v1/me/thread_reads`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadReadsQuery {
    /// 1–100 thread root ids.
    pub root_ids: IdList,
}

impl Validate for ThreadReadsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

unit_enum! {
    /// Which threads `GET /api/v1/me/threads` lists.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ThreadsInboxFilter {
        /// Followed threads and threads with an unread mention.
        All = "all",
        /// Only those that are unread or have an unread mention.
        Unread = "unread",
    }
}

/// Query of `GET /api/v1/me/threads`: pages by `activity_id`, newest first (a C5 exception,
/// S14 gap G27).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadsInboxQuery {
    /// Threads whose `activity_id` is below this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// `all` (default) or `unread`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<ThreadsInboxFilter>,
}

impl ThreadsInboxQuery {
    /// The page size, default 50.
    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(PAGE_LIMIT_DEFAULT)
    }
}

impl Validate for ThreadsInboxQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(l) = self.limit
            && !(1..=PAGE_LIMIT_MAX).contains(&l)
        {
            return Err(ValidationError::invalid(
                "limit",
                format!("limit must be 1–{PAGE_LIMIT_MAX}"),
            ));
        }
        if self.filter.is_some_and(ThreadsInboxFilter::is_unknown) {
            return Err(ValidationError::invalid(
                "filter",
                "filter is all or unread",
            ));
        }
        Ok(())
    }
}

/// One row of `GET /api/v1/me/threads`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadInboxItem {
    /// The newest reply, or the root when there is none: the sort key and `before` cursor.
    pub activity_id: Id,
    /// The root, rendered for the caller (a tombstone for a deleted root).
    pub root: Message,
    /// The thread summary (`following` for the caller).
    pub thread: ThreadSummary,
    /// The caller's read state in the thread.
    pub read: ThreadReadState,
}

/// One channel of `GET /api/v1/me/sync`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SyncChannel {
    /// The channel.
    pub id: Id,
    /// Its change-log head.
    pub head_seq: u64,
    /// Its change-log floor.
    pub floor_seq: u64,
    /// Its newest timeline message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_id: Option<Id>,
}

/// Response of `GET /api/v1/me/threads`.
pub type ThreadInboxPage = crate::http::Page<ThreadInboxItem>;
/// Response of `GET /api/v1/me/read_states` (`has_more` is always false).
pub type ReadStatesPage = crate::http::Page<ReadState>;
/// Response of `GET /api/v1/me/thread_reads` (`has_more` is always false).
pub type ThreadReadsPage = crate::http::Page<ThreadReadState>;

/// Response of `GET /api/v1/me/sync`: READY's sync section over HTTP, read in one snapshot
/// (agents and clients without a socket; S14 gap G16).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeSync {
    /// The install.
    pub install_id: Id,
    /// The install's sync epoch (opaque; a change means "drop everything").
    pub sync_epoch: String,
    /// The caller.
    pub member_id: Id,
    /// The caller's member change-log head.
    pub member_seq: u64,
    /// Every channel of the caller's sync set, ascending id.
    pub channels: Vec<SyncChannel>,
    /// Read states of those channels.
    pub read_states: Vec<ReadState>,
    /// The threads inbox summary.
    pub threads_inbox: ThreadsInbox,
}
