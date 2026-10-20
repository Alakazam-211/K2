//! The messaging extras (S17): drafts, saved items, scheduled messages, reminders, polls and
//! channel bookmarks (prd-sew-17 §20; contracts rev 26).
//!
//! Times of "when" fields are [`crate::when::When`]; the server converts them to UTC once, at
//! the request, and stores the instant plus the zone name it used. Range rules that carry a
//! `details.reason` (too soon, blank drafts, poll option rules) are checked by the server;
//! `validate()` here only enforces the byte caps (`413 too_large`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::messages::Message;
use crate::error::{Validate, ValidationError};
use crate::http::Patch;
use crate::id::{Id, IdList};
use crate::limits::{DRAFT_BODY_MAX_BYTES, NOTE_MAX_BYTES, PAGE_LIMIT_MAX};
use crate::time::Timestamp;
use crate::when::When;

/// `details.reason` values of S17 errors (prd-sew-17 §20).
pub mod reasons {
    /// `400`: a draft with an empty body and no uploads (use the delete route).
    pub const BLANK_DRAFT: &str = "blank_draft";
    /// `409`: the draft's `rev` is not `if_rev`; `details.current` holds the current draft.
    pub const DRAFT_REV_MISMATCH: &str = "draft_rev_mismatch";
    /// `403`: agents have no drafts or saved items.
    pub const AGENT_NOT_SUPPORTED: &str = "agent_not_supported";
    /// `422`: agents set reminders on messages only.
    pub const AGENT_REMINDER_NEEDS_MESSAGE: &str = "agent_reminder_needs_message";
    /// `422`: an upload the caller may not use (missing, another member's or channel's,
    /// expired, already attached).
    pub const UPLOAD_UNAVAILABLE: &str = "upload_unavailable";
    /// `422`: this server has no pending-upload source.
    pub const UPLOADS_UNAVAILABLE: &str = "uploads_unavailable";
    /// `422`: a pending upload would expire before the send window ends.
    pub const ATTACHMENT_EXPIRES_BEFORE_SEND: &str = "attachment_expires_before_send";
    /// `409`: the scheduled message was already sent.
    pub const ALREADY_SENT: &str = "already_sent";
    /// `400`: the time is too close to now.
    pub const TOO_SOON: &str = "too_soon";
    /// `400`: the time is too far from now.
    pub const TOO_FAR: &str = "too_far";
    /// `400`: an unknown IANA time zone.
    pub const BAD_TIMEZONE: &str = "bad_timezone";
    /// `400`: a local time not in `YYYY-MM-DDTHH:MM`.
    pub const BAD_LOCAL_TIME: &str = "bad_local_time";
    /// `422`: the message has no poll.
    pub const NOT_A_POLL: &str = "not_a_poll";
    /// `422`: the poll is closed.
    pub const POLL_CLOSED: &str = "poll_closed";
    /// `400`: an option id the poll does not have.
    pub const UNKNOWN_OPTION: &str = "unknown_option";
    /// `400`: more options than `max_choices`.
    pub const TOO_MANY_CHOICES: &str = "too_many_choices";
    /// `400`: two options with the same text.
    pub const DUPLICATE_OPTION: &str = "duplicate_option";
    /// `400`: a poll question or option outside its length, or a bad option count.
    pub const BAD_POLL: &str = "bad_poll";
    /// `403`: voters of an anonymous poll are never listed.
    pub const ANONYMOUS_POLL: &str = "anonymous_poll";
    /// `403`: results are hidden until the poll closes.
    pub const RESULTS_HIDDEN: &str = "results_hidden";
    /// `422`: bookmarks exist only in space channels.
    pub const NOT_SPACE_CHANNEL: &str = "not_space_channel";
    /// `409`: a reorder must name exactly the current bookmarks.
    pub const STALE_BOOKMARK_LIST: &str = "stale_bookmark_list";
    /// `400`: a bookmark URL that is not absolute http(s) without user info.
    pub const BAD_URL: &str = "bad_url";
    /// `400`: a bookmark emoji that is not exactly one Unicode emoji.
    pub const BAD_EMOJI: &str = "bad_emoji";
    /// `400`: more than 10 uploads on a draft or scheduled message.
    pub const TOO_MANY_UPLOADS: &str = "too_many_uploads";
    /// `400`: a reminder needs a message or a note.
    pub const NEEDS_MESSAGE_OR_NOTE: &str = "needs_message_or_note";
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn check_bytes(field: &str, v: &str, max: usize) -> Result<(), ValidationError> {
    if v.len() > max {
        return Err(ValidationError::too_large(
            field,
            format!("{field} is {} bytes; at most {max}", v.len()),
        ));
    }
    Ok(())
}

fn check_limit(limit: Option<u32>) -> Result<(), ValidationError> {
    match limit {
        Some(l) if l == 0 || l > PAGE_LIMIT_MAX => Err(ValidationError::invalid(
            "limit",
            format!("limit must be 1–{PAGE_LIMIT_MAX}"),
        )),
        _ => Ok(()),
    }
}

/// `GET /api/v1/me/drafts`.
pub type DraftPage = crate::http::Page<Draft>;
/// `GET /api/v1/me/saved`.
pub type SavedItemPage = crate::http::Page<SavedItem>;
/// `GET /api/v1/me/scheduled`.
pub type ScheduledMessagePage = crate::http::Page<ScheduledMessage>;
/// `GET /api/v1/me/reminders`.
pub type ReminderPage = crate::http::Page<Reminder>;
/// `GET /api/v1/messages/{id}/poll/voters`.
pub type PollVoterPage = crate::http::Page<PollVoter>;
/// `GET /api/v1/channels/{id}/bookmarks`, reorder.
pub type BookmarkPage = crate::http::Page<Bookmark>;

/// A deleted object's id (`POST …/delete` responses, `*_deleted` events).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeletedRef {
    /// The id.
    pub id: Id,
}

// ---------------------------------------------------------------- drafts (§7)

/// A pending upload held by a draft or a scheduled message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DraftUpload {
    /// The upload id (the attachment id once sent).
    pub upload_id: Id,
    /// The file name.
    pub filename: String,
    /// Bytes.
    pub size: u64,
    /// The stored (or declared) content type.
    pub content_type: String,
    /// When the pending upload expires.
    pub expires_at: Timestamp,
}

/// The caller's unsent text in a channel or thread, synced across devices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Draft {
    /// The draft id (kept across saves).
    pub id: Id,
    /// The channel.
    pub channel_id: Id,
    /// The thread, for a reply draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// The body (may be empty when uploads are attached).
    pub body: String,
    /// Pending uploads that are still live.
    pub uploads: Vec<DraftUpload>,
    /// A reply draft that also goes to the channel.
    pub also_send_to_channel: bool,
    /// Server-ordered revision, 1 on the first save.
    pub rev: u32,
    /// First save.
    pub created_at: Timestamp,
    /// Last save.
    pub updated_at: Timestamp,
}

/// Body of `POST /api/v1/channels/{id}/draft`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveDraftRequest {
    /// The thread, for a reply draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// The body, 0–16,384 bytes.
    #[serde(default)]
    pub body: String,
    /// Pending uploads of the caller in this channel, 0–10.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upload_ids: Vec<Id>,
    /// For a reply draft: also send to the channel.
    #[serde(default, skip_serializing_if = "is_false")]
    pub also_send_to_channel: bool,
    /// Expected current `rev` (`0` = no draft yet); a mismatch is `409`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_rev: Option<u32>,
}

impl Validate for SaveDraftRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_bytes("body", &self.body, DRAFT_BODY_MAX_BYTES)
    }
}

/// Body of `POST /api/v1/channels/{id}/draft/delete`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteDraftRequest {
    /// The thread, for a reply draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// Expected current `rev` (`0` = no draft); a mismatch is `409`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_rev: Option<u32>,
}

impl Validate for DeleteDraftRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Query of `GET /api/v1/me/drafts`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftsQuery {
    /// Only these drafts (catch-up refetch; missing = deleted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<IdList>,
    /// Only this channel's drafts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<Id>,
}

impl Validate for DraftsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Body of `draft_deleted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DraftDeleted {
    /// The draft.
    pub id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// Its thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
}

// ---------------------------------------------------------------- saved items (§8)

coded_enum! {
    /// A saved item's state (`saved_items.state`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum SavedState {
        /// In the "save for later" list.
        Active = "active" => 1,
        /// Completed.
        Done = "done" => 2,
        /// Kept, out of the way.
        Archived = "archived" => 3,
    }
}

/// A message the member saved for later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SavedItem {
    /// The saved item id.
    pub id: Id,
    /// The message.
    pub message_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// The state.
    pub state: SavedState,
    /// The member's private note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The message as the member sees it now (lists; never in events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Box<Message>>,
    /// Saved.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
    /// Last state change.
    pub state_changed_at: Timestamp,
}

/// Body of `POST /api/v1/messages/{id}/save`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveMessageRequest {
    /// A private note, 1–2,048 bytes (replaces an existing note).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Validate for SaveMessageRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        match &self.note {
            Some(n) => check_bytes("note", n, NOTE_MAX_BYTES),
            None => Ok(()),
        }
    }
}

/// Body of `POST /api/v1/saved/{id}/edit`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditSavedRequest {
    /// A new state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<SavedState>,
    /// Set or clear the note.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub note: Patch<String>,
}

impl Validate for EditSavedRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(s) = self.state {
            crate::error::check_known("state", s)?;
        }
        match self.note.as_set() {
            Some(n) => check_bytes("note", n, NOTE_MAX_BYTES),
            None => Ok(()),
        }
    }
}

/// Query of `GET /api/v1/me/saved`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SavedQuery {
    /// The state to list (default `active`; ignored with `ids`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<SavedState>,
    /// Only these items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<IdList>,
    /// Items saved before this id (newest first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    /// Items saved after this id (oldest first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Validate for SavedQuery {
    // At most one cursor; the server refuses both (the full fixture shows the shape).
    fn validate(&self) -> Result<(), ValidationError> {
        check_limit(self.limit)
    }
}

/// Body of `saved_item_deleted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SavedItemDeleted {
    /// The saved item.
    pub id: Id,
    /// Its message.
    pub message_id: Id,
}

// ---------------------------------------------------------------- scheduled messages (§9)

coded_enum! {
    /// A scheduled message's state (`scheduled_messages.state`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ScheduledState {
        /// Waiting for its time.
        Pending = "pending" => 1,
        /// Sent (`message_id` set).
        Sent = "sent" => 2,
        /// Not sent; `fail_reason` says why. The author edits, sends now or deletes it.
        Failed = "failed" => 3,
    }
}

coded_enum! {
    /// Why a scheduled message was not sent (`scheduled_messages.fail_reason`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ScheduledFailReason {
        /// The author can no longer see the channel or thread.
        NoAccess = "no_access" => 1,
        /// The author lacks a permission the post needs.
        MissingPermission = "missing_permission" => 2,
        /// An org policy refused the post.
        PolicyDenied = "policy_denied" => 3,
        /// The channel is archived.
        ChannelArchived = "channel_archived" => 4,
        /// The thread root was deleted.
        ThreadRootDeleted = "thread_root_deleted" => 5,
        /// The 1:1 DM partner is deactivated.
        DmPartnerDeactivated = "dm_partner_deactivated" => 6,
        /// Slow mode would hold it past the grace window.
        SlowMode = "slow_mode" => 7,
        /// The message rate would hold it past the grace window.
        RateLimited = "rate_limited" => 8,
        /// More than 60 minutes late (after downtime).
        Overdue = "overdue" => 9,
        /// An attached upload expired.
        AttachmentExpired = "attachment_expired" => 10,
        /// The author is deactivated.
        AuthorInactive = "author_inactive" => 11,
        /// The body breaks a content rule (mention limits, blank body).
        InvalidContent = "invalid_content" => 12,
        /// The send kept failing.
        Internal = "internal" => 13,
    }
}

/// A message scheduled for later. Only its author sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScheduledMessage {
    /// The scheduled message id.
    pub id: Id,
    /// The channel.
    pub channel_id: Id,
    /// The thread, for a reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// A reply that also goes to the channel.
    pub also_send_to_channel: bool,
    /// The body (empty once sent).
    pub body: String,
    /// Pending uploads (empty once sent).
    pub uploads: Vec<DraftUpload>,
    /// When it goes out (UTC).
    pub send_at: Timestamp,
    /// The zone a local `when` was given in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// The state.
    pub state: ScheduledState,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_reason: Option<ScheduledFailReason>,
    /// When it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<Timestamp>,
    /// The sent message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Id>,
    /// When it was sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<Timestamp>,
    /// Bumped by every edit.
    pub version: u32,
    /// Created.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
}

/// Body of `POST /api/v1/channels/{id}/scheduled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateScheduledRequest {
    /// The body (may be empty with uploads).
    #[serde(default)]
    pub body: String,
    /// Reply in this thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// For a reply: also send to the channel.
    #[serde(default, skip_serializing_if = "is_false")]
    pub also_send_to_channel: bool,
    /// Finished uploads of the caller in this channel, 0–10.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upload_ids: Vec<Id>,
    /// When to send: now + 10 s … now + 120 days.
    pub when: When,
    /// Delete the caller's draft for the same channel and thread.
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_draft: bool,
}

impl Validate for CreateScheduledRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_bytes("body", &self.body, DRAFT_BODY_MAX_BYTES)
    }
}

/// Body of `POST /api/v1/scheduled/{id}/edit`. Channel and thread cannot change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditScheduledRequest {
    /// A new body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// New uploads (replaces the list).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_ids: Option<Vec<Id>>,
    /// Change the also-send flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub also_send_to_channel: Option<bool>,
    /// A new time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<When>,
}

impl Validate for EditScheduledRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        match &self.body {
            Some(b) => check_bytes("body", b, DRAFT_BODY_MAX_BYTES),
            None => Ok(()),
        }
    }
}

/// Query of `GET /api/v1/me/scheduled`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledQuery {
    /// Only this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ScheduledState>,
    /// Only this channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<Id>,
    /// Only these ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<IdList>,
}

impl Validate for ScheduledQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

// ---------------------------------------------------------------- reminders (§10)

coded_enum! {
    /// A reminder's state (`reminders.state`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ReminderState {
        /// Waiting for its time.
        Pending = "pending" => 1,
        /// Came due; stays until completed, snoozed or deleted.
        Fired = "fired" => 2,
        /// Completed.
        Done = "done" => 3,
    }
}

/// "Remind me": on a message, or free text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Reminder {
    /// The reminder id.
    pub id: Id,
    /// The message, for a message reminder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Id>,
    /// The message's channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<Id>,
    /// The member's note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// When it comes due (UTC).
    pub due_at: Timestamp,
    /// The zone a local `when` was given in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// The state.
    pub state: ReminderState,
    /// When it last fired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_at: Option<Timestamp>,
    /// When it was completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_at: Option<Timestamp>,
    /// Snoozes so far.
    pub snooze_count: u32,
    /// Bumped by every change.
    pub version: u32,
    /// Created.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
    /// For a message reminder: whether the message is still there and readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_available: Option<bool>,
    /// The message as the member sees it (lists and `reminder_fired`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Box<Message>>,
}

/// Body of `POST /api/v1/me/reminders`: at least one of `message_id` and `note`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateReminderRequest {
    /// Remind about this message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Id>,
    /// A note, 1–2,048 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// When: now + 10 s … now + 366 days.
    pub when: When,
}

impl Validate for CreateReminderRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        match &self.note {
            Some(n) => check_bytes("note", n, NOTE_MAX_BYTES),
            None => Ok(()),
        }
    }
}

/// Body of `POST /api/v1/reminders/{id}/edit`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditReminderRequest {
    /// Set or clear the note.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub note: Patch<String>,
    /// A new time (the reminder goes back to `pending`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<When>,
}

impl Validate for EditReminderRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        match self.note.as_set() {
            Some(n) => check_bytes("note", n, NOTE_MAX_BYTES),
            None => Ok(()),
        }
    }
}

/// Body of `POST /api/v1/reminders/{id}/snooze`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnoozeReminderRequest {
    /// The new time.
    pub when: When,
}

impl Validate for SnoozeReminderRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Query of `GET /api/v1/me/reminders`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemindersQuery {
    /// The state to list (default `pending`; ignored with `ids`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ReminderState>,
    /// Only these reminders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<IdList>,
    /// `done` reminders before this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Validate for RemindersQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        check_limit(self.limit)
    }
}

/// Body of `reminder_fired`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReminderFired {
    /// The reminder (state `fired`), with the message when it is readable.
    pub reminder: Reminder,
    /// How late it fired.
    pub late_ms: u64,
}

// ---------------------------------------------------------------- polls (§11)

coded_enum! {
    /// When a poll's counts are shown (`polls.results_visibility`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum PollResultsVisibility {
        /// Always.
        Always = "always" => 1,
        /// Only once the poll is closed.
        AfterClose = "after_close" => 2,
    }
}

unit_enum! {
    /// Whether a poll takes votes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum PollState {
        /// Takes votes.
        Open = "open",
        /// Closed by hand or by its time.
        Closed = "closed",
    }
}

/// A poll on a new message (`CreateMessageRequest.poll`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PollInput {
    /// The question, 1–300 characters.
    pub question: String,
    /// 2–10 options, 1–100 characters each, distinct.
    pub options: Vec<String>,
    /// How many options a voter may pick, 1..=options (default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_choices: Option<u8>,
    /// Hide who voted, from everyone.
    #[serde(default, skip_serializing_if = "is_false")]
    pub anonymous: bool,
    /// When counts are shown (default `always`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub results_visibility: Option<PollResultsVisibility>,
    /// Close after this many seconds, 60–2,764,800 (absent = closed by hand).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<u32>,
}

/// One poll option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PollOption {
    /// 1-based option id.
    pub id: u8,
    /// The text.
    pub text: String,
    /// Votes; `null` while results are hidden.
    pub votes: Option<u32>,
}

/// A message's poll.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Poll {
    /// The question.
    pub question: String,
    /// The options.
    pub options: Vec<PollOption>,
    /// How many options a voter may pick.
    pub max_choices: u8,
    /// Voters are never shown.
    pub anonymous: bool,
    /// When counts are shown.
    pub results_visibility: PollResultsVisibility,
    /// When it closes by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closes_at: Option<Timestamp>,
    /// When it was closed (by hand, or its `closes_at` once the close ran).
    pub closed_at: Option<Timestamp>,
    /// Who closed it by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_by: Option<Id>,
    /// Open or closed (closed once `closes_at` passed, even before the close ran).
    pub state: PollState,
    /// Members who voted; `null` while results are hidden.
    pub voter_count: Option<u32>,
    /// The viewer's own votes (reads and the voter's own event only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_votes: Option<Vec<u8>>,
}

/// Body of `POST /api/v1/messages/{id}/poll/vote`: the whole selection (empty = no vote).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PollVoteRequest {
    /// Option ids.
    pub option_ids: Vec<u8>,
}

impl Validate for PollVoteRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Query of `GET /api/v1/messages/{id}/poll/voters`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PollVotersQuery {
    /// The option.
    pub option_id: u8,
    /// Voters after this member id (ascending).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Validate for PollVotersQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        check_limit(self.limit)
    }
}

/// One voter of a non-anonymous poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PollVoter {
    /// The member.
    pub member_id: Id,
    /// When they picked this option.
    pub voted_at: Timestamp,
}

/// Body of `poll_updated`. `poll.my_votes` rides only in the voter's own event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PollUpdated {
    /// The poll's message.
    pub message_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// The poll now.
    pub poll: Poll,
    /// The channel seq the counts are current as of.
    pub seq: u64,
}

// ---------------------------------------------------------------- bookmarks (§12)

/// A link in a channel's header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Bookmark {
    /// The bookmark id.
    pub id: Id,
    /// The channel.
    pub channel_id: Id,
    /// The title, 1–100 characters.
    pub title: String,
    /// An absolute http(s) URL.
    pub url: String,
    /// One Unicode emoji.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    /// 0-based position.
    pub position: u32,
    /// Who created it.
    pub created_by: Id,
    /// Created.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
    /// The `bookmark_changed` seq (events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

/// Body of `POST /api/v1/channels/{id}/bookmarks`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateBookmarkRequest {
    /// The title.
    pub title: String,
    /// The URL.
    pub url: String,
    /// One Unicode emoji.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
}

impl Validate for CreateBookmarkRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Body of `POST /api/v1/bookmarks/{id}/edit`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditBookmarkRequest {
    /// A new title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A new URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Set or clear the emoji.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub emoji: Patch<String>,
}

impl Validate for EditBookmarkRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Body of `POST /api/v1/channels/{id}/bookmarks/reorder`: every bookmark, in the new order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReorderBookmarksRequest {
    /// The bookmark ids.
    pub ids: Vec<Id>,
}

impl Validate for ReorderBookmarksRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Query of `GET /api/v1/channels/{id}/bookmarks`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BookmarksQuery {
    /// Only these bookmarks (catch-up refetch; missing = deleted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<IdList>,
}

impl Validate for BookmarksQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Body of `bookmark_deleted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BookmarkDeleted {
    /// The bookmark.
    pub id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// The `bookmark_changed` seq.
    pub seq: u64,
}
