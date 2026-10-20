//! Messages, reactions, references and threads (C5, C8; S13, S15).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::extras::{Poll, PollInput};
use crate::api::media::{CustomEmojiRef, LinkPreview};
use crate::error::{Validate, ValidationError};
use crate::http::Nonce;
use crate::id::{Id, IdList};
use crate::limits::{
    FORWARD_BODY_MAX_BYTES, MESSAGE_BODY_MAX_BYTES, PAGE_LIMIT_DEFAULT, PAGE_LIMIT_MAX,
};
use crate::time::Timestamp;

/// Longest Unicode emoji value in bytes.
pub const EMOJI_VALUE_MAX_BYTES: usize = 64;
/// Longest custom emoji name in bytes.
pub const EMOJI_NAME_MAX_BYTES: usize = 64;

/// An emoji on a reaction: Unicode or a custom emoji.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Emoji {
    /// A Unicode emoji.
    Unicode {
        /// The emoji text, for example `👍`.
        value: String,
    },
    /// A custom emoji.
    Custom {
        /// The emoji id.
        id: Id,
        /// The emoji name, for example `party`.
        name: String,
        /// An animated GIF (S16).
        #[serde(default)]
        animated: bool,
        /// Signed image URL; absent when not available (S16).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// `false` once the emoji is deleted (S16). Old payloads read as `true`.
        #[serde(default = "yes")]
        available: bool,
    },
    /// A kind this build does not know.
    #[serde(other)]
    #[schemars(skip)]
    Unknown,
}

fn yes() -> bool {
    true
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// An emoji in a request: Unicode or a custom emoji.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EmojiInput {
    /// A Unicode emoji.
    Unicode {
        /// The emoji text.
        value: String,
    },
    /// A custom emoji.
    Custom {
        /// The emoji id.
        id: Id,
        /// The emoji name.
        name: String,
    },
}

impl Validate for EmojiInput {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            EmojiInput::Unicode { value } => {
                if value.is_empty() || value.len() > EMOJI_VALUE_MAX_BYTES {
                    return Err(ValidationError::invalid(
                        "value",
                        format!("value must be 1–{EMOJI_VALUE_MAX_BYTES} bytes"),
                    ));
                }
            }
            EmojiInput::Custom { name, .. } => {
                if name.is_empty() || name.len() > EMOJI_NAME_MAX_BYTES {
                    return Err(ValidationError::invalid(
                        "name",
                        format!("name must be 1–{EMOJI_NAME_MAX_BYTES} bytes"),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// One emoji's reaction count on a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReactionSummary {
    /// The emoji.
    pub emoji: Emoji,
    /// How many members reacted with it.
    pub count: u32,
    /// Whether the viewer reacted with it.
    pub me: bool,
}

/// Who a message mentions, after expansion rules (S13).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MentionTargets {
    /// Mentioned members.
    pub members: Vec<Id>,
    /// Mentioned roles.
    pub roles: Vec<Id>,
    /// Mentioned user groups.
    pub groups: Vec<Id>,
    /// `@channel` (or an alias).
    pub channel: bool,
    /// `@here`.
    pub here: bool,
}

unit_enum! {
    /// How a message refers to another.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ReferenceKind {
        /// An inline reply.
        Reply = "reply",
        /// A quote.
        Quote = "quote",
        /// A forward.
        Forward = "forward",
    }
}

/// A message's reference to another message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageReference {
    /// Reply, quote or forward.
    pub kind: ReferenceKind,
    /// The referenced message.
    pub message_id: Id,
    /// Its channel.
    pub channel_id: Id,
}

/// A file attached to a message.
///
/// `id` = the upload id = the attachment id (S15). URLs are signed, path-absolute and valid for
/// 55–60 minutes (`url_expires_at`); every render inside one 5-minute window gives the same
/// strings. `width`/`height` are the displayed size (after EXIF orientation) of an image or the
/// poster frame of a video.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Attachment {
    /// The attachment id.
    pub id: Id,
    /// The file name.
    pub filename: String,
    /// The stored (sniffed) MIME type, not the served one (S15 §10).
    pub content_type: String,
    /// Size in bytes.
    pub size: u64,
    /// Signed download URL.
    pub url: String,
    /// Signed thumbnail URL (images, and video posters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    /// Image or video width in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Image or video height in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// The kind (S15; payloads from before S15 read as `other`).
    #[serde(default)]
    pub kind: crate::api::files::FileKind,
    /// Served `inline` (only sniffed image, video and audio types, S15 §10.4).
    #[serde(default)]
    pub inline: bool,
    /// An animated GIF or WebP.
    #[serde(default)]
    pub animated: bool,
    /// A thumbnail or poster is still being made; `attachments_updated` follows.
    #[serde(default)]
    pub media_pending: bool,
    /// Signed large-preview URL (images wider than 1,920 px, BMP, TIFF).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    /// Signed poster-frame URL (videos, when the server has `ffmpeg`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster_url: Option<String>,
    /// When the signed URLs stop working (S15; absent in payloads from before S15).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_expires_at: Option<Timestamp>,
}

/// A thread's summary, carried on its root message and in `thread_updated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadSummary {
    /// The root message.
    pub root_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// Number of replies.
    pub reply_count: u32,
    /// The newest reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reply_id: Option<Id>,
    /// When the newest reply was posted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reply_at: Option<Timestamp>,
    /// Whether the viewer follows the thread.
    pub following: bool,
    /// The channel seq this summary is current as of; set in `thread_updated` events (S13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

/// The snapshot a forward copies (S13 §12.3): the origin's text at forward time, sealed under
/// the target channel's key. Source edits and deletes never change it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForwardedMessage {
    /// The origin message.
    pub message_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// Its author.
    pub author_id: Id,
    /// Its body at forward time.
    pub body: String,
    /// When the origin was posted.
    pub created_at: Timestamp,
}

/// A message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Message {
    /// The message id.
    pub id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// The author.
    pub author_id: Id,
    /// For assistant posts, the person the assistant acts for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_behalf_of: Option<Id>,
    /// The body; empty when deleted.
    pub body: String,
    /// The sender's nonce, echoed back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<Nonce>,
    /// The thread root, for replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// Whether a thread reply was also sent to the channel.
    pub also_in_channel: bool,
    /// A reply, quote or forward reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<MessageReference>,
    /// For a forward: the snapshot of the origin (S13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarded: Option<ForwardedMessage>,
    /// Who the message mentions.
    pub mentions: MentionTargets,
    /// Reaction counts.
    pub reactions: Vec<ReactionSummary>,
    /// Attachments.
    pub attachments: Vec<Attachment>,
    /// Whether it is pinned.
    pub pinned: bool,
    /// Thread summary, on thread roots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadSummary>,
    /// The channel seq this message is current as of.
    pub seq: u64,
    /// When it was posted.
    pub created_at: Timestamp,
    /// When it was last edited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<Timestamp>,
    /// When it was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<Timestamp>,
    /// Link previews in body order (S16); `[]` on tombstones and until the preview job lands.
    #[serde(default)]
    pub previews: Vec<LinkPreview>,
    /// The author (or a moderator) turned previews off for this message (S16).
    #[serde(default)]
    pub previews_suppressed: bool,
    /// The usable custom emoji of the body, ascending id (S16); `[]` on tombstones.
    #[serde(default)]
    pub emojis: Vec<CustomEmojiRef>,
    /// The poll posted with the message (S17); absent on tombstones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll: Option<Poll>,
    /// The message mentions the viewer directly (per viewer; v1 counts direct member
    /// mentions only, contracts rev 31). Present only when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mentions_me: bool,
    /// The DM call this message records (S34 v2 §18.2); absent on tombstones and forwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<crate::calls::CallSummary>,
    /// The recording this message records (S35 v2 §15.1); absent on tombstones and forwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<crate::recording::RecordingSummary>,
    /// The transcript turn this message is (S35 v2 §15.2): numbers only, the text is the body;
    /// absent on tombstones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<crate::recording::TranscriptTurn>,
}

/// Query of `GET /api/v1/channels/{id}/messages` and `GET /api/v1/messages/{root_id}/replies`:
/// a [`PageQuery`](crate::http::PageQuery) plus an optional long-poll (contracts rev 31, S30
/// G3). With `after` and `wait_ms`, an empty page waits up to `wait_ms` (0–25,000) for a new
/// message and reads again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineQuery {
    /// Items older than this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    /// Items newer than this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// Items around this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub around: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Long-poll: wait up to this long for a message when the `after` page is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<u32>,
}

impl TimelineQuery {
    /// The page part.
    pub fn page(&self) -> crate::http::PageQuery {
        crate::http::PageQuery {
            before: self.before,
            after: self.after,
            around: self.around,
            limit: self.limit,
        }
    }
}

impl From<crate::http::PageQuery> for TimelineQuery {
    fn from(q: crate::http::PageQuery) -> Self {
        TimelineQuery {
            before: q.before,
            after: q.after,
            around: q.around,
            limit: q.limit,
            wait_ms: None,
        }
    }
}

impl Validate for TimelineQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        self.page().validate()?;
        if let Some(w) = self.wait_ms {
            if w > crate::limits::MESSAGES_WAIT_MAX_MS {
                return Err(ValidationError::invalid(
                    "wait_ms",
                    format!("wait_ms must be 0–{}", crate::limits::MESSAGES_WAIT_MAX_MS),
                ));
            }
            if self.after.is_none() {
                return Err(ValidationError::invalid("wait_ms", "wait_ms needs after"));
            }
        }
        Ok(())
    }
}

/// One row of a message's edit history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageVersion {
    /// The revision id (S13; optional on the wire so frozen v1 payloads still parse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The message.
    pub message_id: Id,
    /// The version this body was: 0 at create, +1 per edit (S13).
    #[serde(default)]
    pub version: u32,
    /// The body of this version.
    pub body: String,
    /// When this version was written.
    pub created_at: Timestamp,
    /// When the next version replaced it (S13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_at: Option<Timestamp>,
}

/// A reply or quote reference in a create request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReferenceInput {
    /// The referenced message.
    pub message_id: Id,
    /// `reply` or `quote`.
    pub kind: ReferenceKind,
}

impl Validate for ReferenceInput {
    fn validate(&self) -> Result<(), ValidationError> {
        match self.kind {
            ReferenceKind::Reply | ReferenceKind::Quote => Ok(()),
            _ => Err(ValidationError::invalid(
                "kind",
                "kind must be reply or quote; forwards use the forward route",
            )),
        }
    }
}

fn check_body(field: &str, body: &str, max: usize) -> Result<(), ValidationError> {
    if body.len() > max {
        return Err(ValidationError::too_large(
            field,
            format!("{field} is {} bytes; at most {max}", body.len()),
        ));
    }
    Ok(())
}

/// Body of `POST /api/v1/channels/{id}/messages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateMessageRequest {
    /// The body: 1 byte to 16 KiB of UTF-8. Larger is `413 too_large`.
    pub body: String,
    /// A client nonce, 1–64 characters, echoed back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<Nonce>,
    /// Post as a reply in this thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// For thread replies, also show it in the channel.
    #[serde(default)]
    pub also_send_to_channel: bool,
    /// Reply to or quote a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<ReferenceInput>,
    /// Finished uploads of the caller in this channel to attach, 0–10, in display order
    /// (S15). With at least one, the body may be empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upload_ids: Vec<Id>,
    /// Build no link previews for this message (S16).
    #[serde(default, skip_serializing_if = "is_false")]
    pub suppress_previews: bool,
    /// Post a poll with the message (S17; needs CREATE_POLLS). The body may then be empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll: Option<PollInput>,
    /// Delete the author's draft for this channel and thread in the same write (S17).
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_draft: bool,
}

impl Validate for CreateMessageRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        // An empty body is the server's `blank_body` unless the post attaches files; the count
        // and duplicate rules of `upload_ids` carry reasons and run in the server too (S15 §8).
        check_body("body", &self.body, MESSAGE_BODY_MAX_BYTES)?;
        if let Some(n) = &self.nonce {
            n.check("nonce")?;
        }
        if let Some(r) = &self.reference {
            r.validate().map_err(|e| e.nested("reference"))?;
        }
        Ok(())
    }
}

/// Body of `POST /api/v1/messages/{id}/edit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditMessageRequest {
    /// The new body (same rules as create).
    pub body: String,
}

impl Validate for EditMessageRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        // A blank body is the server's `blank_body`, except on a message with live files (S15).
        check_body("body", &self.body, MESSAGE_BODY_MAX_BYTES)
    }
}

/// Body of the forward route: copy a message into another channel or thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForwardMessageRequest {
    /// The destination channel.
    pub channel_id: Id,
    /// The destination thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// An optional comment, at most 16 KiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// A client nonce, 1–64 characters, echoed back (S13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<Nonce>,
    /// Copy the source's files too (S15; default `true`). `false` forwards the text only.
    #[serde(default = "default_true")]
    pub attachments: bool,
    /// Build no link previews for the forward's comment (S16).
    #[serde(default, skip_serializing_if = "is_false")]
    pub suppress_previews: bool,
}

fn default_true() -> bool {
    true
}

impl Validate for ForwardMessageRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(b) = &self.body {
            check_body("body", b, FORWARD_BODY_MAX_BYTES)?;
        }
        if let Some(n) = &self.nonce {
            n.check("nonce")?;
        }
        Ok(())
    }
}

/// Body of the add-reaction route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReactionRequest {
    /// The emoji.
    pub emoji: EmojiInput,
}

impl Validate for ReactionRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.emoji.validate().map_err(|e| e.nested("emoji"))
    }
}

/// Body of `POST /api/v1/messages/{id}/reactions/remove` (S13 gap G21). No `member_id`, or the
/// caller's own id, removes the caller's reaction; another id needs MANAGE_MESSAGES.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveReactionRequest {
    /// The emoji.
    pub emoji: EmojiInput,
    /// Whose reaction; absent = the caller's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<Id>,
}

impl Validate for RemoveReactionRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.emoji.validate().map_err(|e| e.nested("emoji"))
    }
}

/// Query of `GET /api/v1/messages?ids=` (S13 §9.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MessagesByIdsQuery {
    /// 1–100 distinct message ids, comma-separated.
    pub ids: IdList,
}

impl Validate for MessagesByIdsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Longest emoji key in a [`ReactorsQuery`] (`u:` + a 64-byte emoji, or `c:` + an id).
pub const EMOJI_KEY_MAX_BYTES: usize = 2 + EMOJI_VALUE_MAX_BYTES;

/// Query of `GET /api/v1/messages/{id}/reactors` (S13 §9.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReactorsQuery {
    /// The emoji in key form: `u:<emoji>` or `c:<custom emoji id>`.
    pub emoji: String,
    /// Members after this id (ascending).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl ReactorsQuery {
    /// The page size.
    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(PAGE_LIMIT_DEFAULT)
    }
}

impl Validate for ReactorsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        let shape_ok = match (self.emoji.strip_prefix("u:"), self.emoji.strip_prefix("c:")) {
            (Some(v), _) => !v.is_empty(),
            (_, Some(v)) => crate::id::parse_id_str(v).is_some(),
            _ => false,
        };
        if !shape_ok || self.emoji.len() > EMOJI_KEY_MAX_BYTES {
            return Err(ValidationError::invalid(
                "emoji",
                "emoji must be u:<emoji> or c:<custom emoji id>",
            ));
        }
        if let Some(l) = self.limit
            && !(1..=PAGE_LIMIT_MAX).contains(&l)
        {
            return Err(ValidationError::invalid(
                "limit",
                format!("limit must be 1–{PAGE_LIMIT_MAX}"),
            ));
        }
        Ok(())
    }
}

/// One member who reacted with an emoji.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Reactor {
    /// The member.
    pub member_id: Id,
    /// When they reacted.
    pub created_at: Timestamp,
}

/// A page of reactors.
pub type ReactorPage = crate::http::Page<Reactor>;
