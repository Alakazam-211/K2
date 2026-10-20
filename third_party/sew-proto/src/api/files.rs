//! Files (S15): uploads, the per-channel file list, attachment removal and the
//! `attachments_updated` event.
//!
//! Upload id = attachment id = blob link id: one id from create to download. No type here
//! carries a blob id, a content hash, a dedup flag or a key scope (S7 G9, contracts revs 3, 5).
//! Signed URLs are path-absolute (`/api/v1/…`, contracts rev 15); act-as responses prefix
//! `[server] internal_base_url` (rev 11). An expired or bad signature is `403
//! missing_permission` with `details.reason = url_expired | url_invalid` (revs 16, 28).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::messages::Attachment;
use crate::error::{Validate, ValidationError, check_known, check_unique_ids};
use crate::id::Id;
use crate::limits::{
    ATTACHMENTS_PER_MESSAGE_MAX, FILENAME_MAX_BYTES, PAGE_LIMIT_MAX, UPLOAD_CONTENT_TYPE_MAX_BYTES,
};
use crate::time::Timestamp;

/// `details.reason` values of S15 errors.
pub mod reasons {
    /// `403`: a signed URL past its `exp` (the only one a client refreshes on, rev 16).
    pub const URL_EXPIRED: &str = "url_expired";
    /// `403`: a missing, malformed or wrong signature, or an unknown query parameter.
    pub const URL_INVALID: &str = "url_invalid";
    /// `404`: the upload session expired (24 h after create).
    pub const UPLOAD_EXPIRED: &str = "upload_expired";
    /// `400`: `Content-Range` total differs from the declared size.
    pub const SIZE_MISMATCH: &str = "size_mismatch";
    /// `400`: `Content-Range` end before its start, or past the total.
    pub const BAD_RANGE: &str = "bad_range";
    /// `409`: the range does not start at `committed_bytes` (in `details`).
    pub const OFFSET_MISMATCH: &str = "offset_mismatch";
    /// `400`: the range does not end on a chunk boundary and is not the last one.
    pub const UNALIGNED_RANGE: &str = "unaligned_range";
    /// `400`: no `Content-Length` (chunked bodies are refused).
    pub const LENGTH_REQUIRED: &str = "length_required";
    /// `400`: `Content-Length` differs from the range length.
    pub const LENGTH_MISMATCH: &str = "length_mismatch";
    /// `409`: another data request of the same upload is running.
    pub const UPLOAD_BUSY: &str = "upload_busy";
    /// `400`: more than 10 `upload_ids`.
    pub const TOO_MANY_ATTACHMENTS: &str = "too_many_attachments";
    /// `400`: an `upload_id` appears twice.
    pub const DUPLICATE_UPLOAD_ID: &str = "duplicate_upload_id";
    /// `422`: the caller's own upload in this channel has not received all its bytes.
    pub const UPLOAD_INCOMPLETE: &str = "upload_incomplete";
    /// `422`: any other upload that cannot be attached (missing, another member's, another
    /// channel's, expired, already attached): one reason for all of them.
    pub const UPLOAD_UNAVAILABLE: &str = "upload_unavailable";
    /// `422`: an attachment id that is not on the message.
    pub const ATTACHMENT_NOT_FOUND: &str = "attachment_not_found";
    /// `413`: a cross-space forward would copy more than `[files] forward_copy_max_bytes`.
    pub const FORWARD_COPY_TOO_LARGE: &str = "forward_copy_too_large";
    /// `429` bucket name: five uploads already in flight.
    pub const UPLOADS_IN_FLIGHT: &str = "uploads_in_flight";
}

unit_enum! {
    /// What a file is, from its sniffed content type (S15 §10).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum FileKind {
        /// A raster image (`image/*` from the magic table, or SVG).
        Image = "image",
        /// A video.
        Video = "video",
        /// Audio.
        Audio = "audio",
        /// A document: PDF, text, office formats.
        Document = "document",
        /// An archive: zip, gzip, 7z, rar.
        Archive = "archive",
        /// Anything else.
        Other = "other",
    }
}

impl Default for FileKind {
    /// Payloads from before S15 carry no kind; they read as `other`.
    fn default() -> Self {
        FileKind::Other
    }
}

impl FileKind {
    /// The stored `smallint` code (S15 §5): 1 image … 6 other.
    pub const fn code(self) -> i16 {
        match self {
            FileKind::Image => 1,
            FileKind::Video => 2,
            FileKind::Audio => 3,
            FileKind::Document => 4,
            FileKind::Archive => 5,
            FileKind::Other | FileKind::Unknown => 6,
        }
    }

    /// From the stored code; unknown codes read as `other`.
    pub const fn from_code(c: i16) -> FileKind {
        match c {
            1 => FileKind::Image,
            2 => FileKind::Video,
            3 => FileKind::Audio,
            4 => FileKind::Document,
            5 => FileKind::Archive,
            _ => FileKind::Other,
        }
    }
}

/// Body of `POST /api/v1/channels/{id}/uploads`: step 1 of an upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateUploadRequest {
    /// The file name. The server keeps the part after the last `/` or `\`, trims it, and
    /// needs 1–255 bytes with no control characters (not `.` or `..`).
    pub filename: String,
    /// The size in bytes. Over `files.max_bytes` is `413 too_large`.
    pub size: u64,
    /// The declared content type (`type/subtype`, advice only; the server sniffs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

impl Validate for CreateUploadRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        // The server normalizes the name first (last path segment, trimmed); the raw value
        // gets only a generous bound here so a path prefix does not fail it.
        if self.filename.len() > 4 * FILENAME_MAX_BYTES {
            return Err(ValidationError::invalid(
                "filename",
                format!("filename must be at most {FILENAME_MAX_BYTES} bytes"),
            ));
        }
        if let Some(ct) = &self.content_type
            && ct.len() > UPLOAD_CONTENT_TYPE_MAX_BYTES
        {
            return Err(ValidationError::invalid(
                "content_type",
                format!("content_type must be at most {UPLOAD_CONTENT_TYPE_MAX_BYTES} bytes"),
            ));
        }
        Ok(())
    }
}

/// Response to `POST /api/v1/channels/{id}/uploads`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateUploadResponse {
    /// The upload id (= the future attachment id).
    pub upload_id: Id,
    /// The signed data URL: `POST` chunks, `GET` the committed offset. No Bearer.
    pub upload_url: String,
    /// When `upload_url` stops working (55–60 min); `GET /uploads/{id}` issues a fresh one.
    pub url_expires_at: Timestamp,
    /// Data requests end on a multiple of this (except the last).
    pub chunk_size: u32,
    /// Bytes stored so far.
    pub committed_bytes: u64,
    /// The declared size.
    pub size: u64,
    /// All bytes arrived and the upload is finalized.
    pub complete: bool,
    /// When the unattached upload expires (24 h).
    pub expires_at: Timestamp,
}

/// Response of the signed data route (`POST` and `GET <upload_url>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UploadStatus {
    /// The upload id.
    pub upload_id: Id,
    /// Bytes stored so far; continue from here.
    pub committed_bytes: u64,
    /// The declared size.
    pub size: u64,
    /// All bytes arrived and the upload is finalized.
    pub complete: bool,
    /// When the unattached upload expires.
    pub expires_at: Timestamp,
}

/// Response of `GET /api/v1/uploads/{id}` (the uploader only): the status plus a fresh
/// `upload_url`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Upload {
    /// The upload id.
    pub upload_id: Id,
    /// Bytes stored so far.
    pub committed_bytes: u64,
    /// The declared size.
    pub size: u64,
    /// All bytes arrived and the upload is finalized.
    pub complete: bool,
    /// When the unattached upload expires.
    pub expires_at: Timestamp,
    /// The channel it may be attached in.
    pub channel_id: Id,
    /// The file name.
    pub filename: String,
    /// The chunk size.
    pub chunk_size: u32,
    /// A fresh signed data URL.
    pub upload_url: String,
    /// When `upload_url` stops working.
    pub url_expires_at: Timestamp,
    /// The stored (sniffed) type, once complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// The kind, once complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<FileKind>,
    /// Image or poster width, once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Image or poster height, once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// A thumbnail is still being made.
    #[serde(default)]
    pub media_pending: bool,
    /// Signed thumbnail URL (the uploader's preview), once made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
}

/// Body of `POST /api/v1/messages/{id}/attachments/remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveAttachmentsRequest {
    /// 1–10 distinct attachment ids of the message.
    pub attachment_ids: Vec<Id>,
}

impl Validate for RemoveAttachmentsRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_unique_ids(
            "attachment_ids",
            &self.attachment_ids,
            1,
            ATTACHMENTS_PER_MESSAGE_MAX,
        )
    }
}

/// Query of `GET /api/v1/channels/{id}/files`: at most one cursor (attachment ids), a page
/// size, and filters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilesQuery {
    /// Files older than this attachment id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    /// Files newer than this attachment id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// Files around this attachment id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub around: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Only this kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<FileKind>,
    /// Only files on messages by this author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Id>,
}

impl FilesQuery {
    /// The cursor part as a [`crate::http::PageQuery`].
    pub fn page(&self) -> crate::http::PageQuery {
        crate::http::PageQuery {
            before: self.before,
            after: self.after,
            around: self.around,
            limit: self.limit,
        }
    }
}

impl Validate for FilesQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        self.page().validate()?;
        if let Some(l) = self.limit
            && !(1..=PAGE_LIMIT_MAX).contains(&l)
        {
            return Err(ValidationError::invalid(
                "limit",
                format!("limit must be 1–{PAGE_LIMIT_MAX}"),
            ));
        }
        if let Some(k) = self.kind {
            check_known("kind", k)?;
        }
        Ok(())
    }
}

/// One file in a channel: the attachment and where it lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelFile {
    /// The attachment, with fresh signed URLs.
    pub attachment: Attachment,
    /// The message it is attached to.
    pub message_id: Id,
    /// The channel.
    pub channel_id: Id,
    /// The thread root, for files on replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// The message's author.
    pub author_id: Id,
    /// When the file was attached.
    pub created_at: Timestamp,
}

/// A page of channel files.
pub type ChannelFilePage = crate::http::Page<ChannelFile>;

/// Body of `attachments_updated`: a message's live attachments after a thumbnail landed or a
/// file was removed. Identical for every viewer (contracts rev 12); clients replace the
/// message's `attachments`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentsUpdated {
    /// The channel.
    pub channel_id: Id,
    /// The message.
    pub message_id: Id,
    /// Its thread root, for replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// The message's live attachments, in position order.
    pub attachments: Vec<Attachment>,
    /// The seq of the `attachment_changed` change entry.
    #[serde(default)]
    pub seq: u64,
}
