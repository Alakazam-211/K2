//! Custom emoji, link previews, GIF search and install secrets (S16, prd-sew-16 §16).
//!
//! - Custom emoji live per space; bodies carry them as `<:name:id>` tokens
//!   ([`crate::emoji_token`]); messages list the usable ones in `Message.emojis`.
//! - Link previews are built by the server after a post commits and arrive as
//!   `message_previews_updated`; `Message.previews` carries them afterwards.
//! - GIF search is off by default (`gifs.enabled`); every media URL a client sees is a Sew URL.
//! - Install secrets (the GIF provider key) are write-only: no response ever carries a value.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Validate, ValidationError};
use crate::http::{Page, Patch};
use crate::id::Id;
use crate::time::Timestamp;

/// `details.reason` values S16 adds (prd-sew-16 §16).
pub mod reasons {
    /// A name or alias breaks `[a-z0-9_-]{2,32}`.
    pub const BAD_NAME: &str = "bad_name";
    /// The same alias twice in one request.
    pub const DUPLICATE_ALIAS: &str = "duplicate_alias";
    /// A standard Unicode shortcode (`smile`, `thumbsup`).
    pub const RESERVED_NAME: &str = "reserved_name";
    /// A live emoji of the space already uses the name or alias (`409`).
    pub const EMOJI_NAME_TAKEN: &str = "emoji_name_taken";
    /// Not PNG, JPEG, GIF or WebP (SVG included).
    pub const UNSUPPORTED_IMAGE: &str = "unsupported_image";
    /// Source or output too large in pixels or bytes.
    pub const IMAGE_TOO_LARGE: &str = "image_too_large";
    /// The image does not decode.
    pub const BAD_IMAGE: &str = "bad_image";
    /// An animated GIF with more than 300 frames.
    pub const TOO_MANY_FRAMES: &str = "too_many_frames";
    /// More than 50 distinct custom emoji tokens in one body.
    pub const TOO_MANY_EMOJI: &str = "too_many_emoji";
    /// GIF search is on but the provider or its key is missing (`503`).
    pub const GIF_PROVIDER_UNCONFIGURED: &str = "gif_provider_unconfigured";
    /// The provider refused the key (`503`).
    pub const GIF_PROVIDER_REJECTED_KEY: &str = "gif_provider_rejected_key";
    /// The provider has no GIF with that id (`404`).
    pub const GIF_NOT_FOUND: &str = "gif_not_found";
    /// The provider rate-limited the install, or the install bucket is empty (`429`).
    pub const PROVIDER_RATE: &str = "provider_rate";
    /// The provider or its media failed (`503`).
    pub const UPSTREAM_ERROR: &str = "upstream_error";
    /// A secret name v1 does not know (`400`).
    pub const UNKNOWN_SECRET: &str = "unknown_secret";
    /// A signed URL whose signature does not verify (`403`).
    pub const URL_INVALID: &str = "url_invalid";
    /// A signed URL past its `exp` (`403`).
    pub const URL_EXPIRED: &str = "url_expired";
    /// Only the author (or a message moderator) may do this (`403`).
    pub const NOT_AUTHOR: &str = "not_author";
    /// An install owner or admin only (`403`, S12 style).
    pub const INSTALL_ROLE: &str = "install_role";
}

// ---------------------------------------------------------------- custom emoji

/// A custom emoji of a space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CustomEmoji {
    /// The emoji id (the `ID` of `<:name:ID>`).
    pub id: Id,
    /// Its space.
    pub space_id: Id,
    /// The primary name, `[a-z0-9_-]{2,32}`.
    pub name: String,
    /// Up to 5 more names, in the order they were set.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// An animated GIF.
    pub animated: bool,
    /// Width in pixels (≤ 128).
    pub width: u32,
    /// Height in pixels (≤ 128).
    pub height: u32,
    /// Signed image URL, stable for a day and valid ≥ 7 days.
    pub url: String,
    /// Signed first-frame PNG URL (animated emoji only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub still_url: Option<String>,
    /// Who uploaded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Id>,
    /// When it was uploaded.
    pub created_at: Timestamp,
    /// The last rename.
    pub updated_at: Timestamp,
}

/// A page of custom emoji (`GET /api/v1/spaces/{id}/emoji`: every live emoji, one page).
pub type CustomEmojiPage = Page<CustomEmoji>;

/// A custom emoji a message body uses (`Message.emojis`), deleted ones included.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct CustomEmojiRef {
    /// The emoji id.
    pub id: Id,
    /// Its space.
    pub space_id: Id,
    /// Its current name (a deleted emoji keeps its last name).
    pub name: String,
    /// An animated GIF.
    pub animated: bool,
    /// Signed image URL; absent when not available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Signed first-frame URL of an animated emoji; absent when not available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub still_url: Option<String>,
    /// `false` once the emoji is deleted.
    pub available: bool,
}

/// Query of `POST /api/v1/spaces/{id}/emoji` (the body is the raw image).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateEmojiQuery {
    /// The primary name, `[a-z0-9_-]{2,32}`.
    pub name: String,
    /// Up to 5 aliases, comma-separated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<String>,
}

impl CreateEmojiQuery {
    /// The aliases as a list (empty entries dropped).
    pub fn alias_list(&self) -> Vec<String> {
        self.aliases
            .as_deref()
            .map(|a| {
                a.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Validate for CreateEmojiQuery {
    // Name rules carry `details.reason` and run in the server (prd-sew-16 §6.1).
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Body of `POST /api/v1/emoji/{id}/edit`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateEmojiRequest {
    /// A new primary name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The whole alias list (replaces it); `null` clears it.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub aliases: Patch<Vec<String>>,
}

impl Validate for UpdateEmojiRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

// ---------------------------------------------------------------- link previews

unit_enum! {
    /// What a link preview shows.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum LinkPreviewKind {
        /// An HTML page (title, description, optional image).
        Page = "page",
        /// The link is an image.
        Image = "image",
    }
}

/// A preview's image, stored and served by Sew.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PreviewImage {
    /// Signed URL (1 h).
    pub url: String,
    /// Width in pixels (≤ 800).
    pub width: u32,
    /// Height in pixels (≤ 800).
    pub height: u32,
}

/// One link preview of a message. Text fields are plain text: never render them as HTML or
/// markdown.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct LinkPreview {
    /// The URL as the author typed it.
    pub url: String,
    /// Its host.
    pub host: String,
    /// Page or image.
    pub kind: LinkPreviewKind,
    /// The page title (≤ 256 characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The page description (≤ 512 characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The site name (≤ 100 characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
    /// The final host when a redirect changed it; clients should show it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirected_host: Option<String>,
    /// The preview image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<PreviewImage>,
}

// ---------------------------------------------------------------- GIFs

/// Query of `GET /api/v1/gifs/search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GifSearchQuery {
    /// The search words, 1–50 characters after trimming.
    pub q: String,
    /// Page size, 1–50 (default 24).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Offset, 0–499.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
}

/// Default GIF page size.
pub const GIF_LIMIT_DEFAULT: u32 = 24;
/// Largest GIF page size.
pub const GIF_LIMIT_MAX: u32 = 50;
/// Largest GIF offset.
pub const GIF_OFFSET_MAX: u32 = 499;
/// Longest GIF search, in characters after trimming.
pub const GIF_QUERY_MAX_CHARS: usize = 50;

fn check_paging(limit: Option<u32>, offset: Option<u32>) -> Result<(), ValidationError> {
    if let Some(l) = limit
        && !(1..=GIF_LIMIT_MAX).contains(&l)
    {
        return Err(ValidationError::invalid(
            "limit",
            format!("limit must be 1–{GIF_LIMIT_MAX}"),
        ));
    }
    if let Some(o) = offset
        && o > GIF_OFFSET_MAX
    {
        return Err(ValidationError::invalid(
            "offset",
            format!("offset must be 0–{GIF_OFFSET_MAX}"),
        ));
    }
    Ok(())
}

impl GifSearchQuery {
    /// The trimmed search words.
    pub fn words(&self) -> &str {
        self.q.trim()
    }
}

impl Validate for GifSearchQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        let n = self.words().chars().count();
        if n == 0 || n > GIF_QUERY_MAX_CHARS {
            return Err(ValidationError::invalid(
                "q",
                format!("q must be 1–{GIF_QUERY_MAX_CHARS} characters"),
            ));
        }
        check_paging(self.limit, self.offset)
    }
}

/// Query of `GET /api/v1/gifs/trending`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GifPageQuery {
    /// Page size, 1–50 (default 24).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Offset, 0–499.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
}

impl Validate for GifPageQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        check_paging(self.limit, self.offset)
    }
}

/// A GIF rendition served through Sew's signed media proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GifMedia {
    /// Signed Sew URL (1 h); never a provider URL.
    pub url: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// One GIF search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GifItem {
    /// The provider's id (pass it to the attach route).
    pub id: String,
    /// The provider's title.
    pub title: String,
    /// Width of the full rendition.
    pub width: u32,
    /// Height of the full rendition.
    pub height: u32,
    /// The thumbnail.
    pub preview: GifMedia,
}

/// A page of GIF results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GifPage {
    /// The provider (`giphy`).
    pub provider: String,
    /// The results.
    pub items: Vec<GifItem>,
    /// The next page's offset, while more results exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u32>,
}

/// Body of `POST /api/v1/channels/{id}/gifs/attach`. The server fetches the GIF itself; a
/// client never sends a media URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GifAttachRequest {
    /// A result's `id`.
    pub id: String,
}

/// Longest provider GIF id.
pub const GIF_ID_MAX_BYTES: usize = 128;

impl Validate for GifAttachRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.id.is_empty()
            || self.id.len() > GIF_ID_MAX_BYTES
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(ValidationError::invalid(
                "id",
                format!("id must be 1–{GIF_ID_MAX_BYTES} of [A-Za-z0-9_-]"),
            ));
        }
        Ok(())
    }
}

/// Response of the GIF attach route: a pending upload to post with `upload_ids`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GifAttachResponse {
    /// The upload id.
    pub upload_id: Id,
    /// `giphy-<id>.gif` (or `.webp`).
    pub filename: String,
    /// `image/gif` or `image/webp`.
    pub content_type: String,
    /// Bytes.
    pub size: u64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// When the pending upload expires unless posted.
    pub expires_at: Timestamp,
}

// ---------------------------------------------------------------- install secrets

/// Longest secret value.
pub const SECRET_VALUE_MAX_BYTES: usize = 512;
/// Longest secret name.
pub const SECRET_NAME_MAX_BYTES: usize = 64;

fn check_secret_name(name: &str) -> Result<(), ValidationError> {
    if name.is_empty()
        || name.len() > SECRET_NAME_MAX_BYTES
        || !name.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-')
        })
    {
        return Err(ValidationError::invalid(
            "name",
            "name must be 1–64 of [a-z0-9_.-]",
        ));
    }
    Ok(())
}

/// Body of `POST /api/v1/install/secrets/set` and `POST /mgmt/v1/secrets/set`. `Debug` never
/// shows the value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSecretRequest {
    /// The secret's name (`gifs.giphy` in v1).
    pub name: String,
    /// The value, 1–512 printable ASCII, no spaces. Never returned or logged.
    pub value: String,
}

impl fmt::Debug for SetSecretRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetSecretRequest")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .finish()
    }
}

impl Validate for SetSecretRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_secret_name(&self.name)?;
        let v = self.value.as_str();
        if v.is_empty()
            || v.len() > SECRET_VALUE_MAX_BYTES
            || !v.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(ValidationError::invalid(
                "value",
                format!("value must be 1–{SECRET_VALUE_MAX_BYTES} printable ASCII, no spaces"),
            ));
        }
        Ok(())
    }
}

/// Body of `POST /api/v1/install/secrets/clear` and `POST /mgmt/v1/secrets/clear`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClearSecretRequest {
    /// The secret's name.
    pub name: String,
}

impl Validate for ClearSecretRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_secret_name(&self.name)
    }
}

unit_enum! {
    /// Where a secret was set.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum SecretSetVia {
        /// The client API (an install owner or admin).
        Client = "client",
        /// The mgmt API (K2's daemon or the `sew` CLI).
        Mgmt = "mgmt",
    }
}

/// A secret's status. The value is never returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SecretStatus {
    /// The name.
    pub name: String,
    /// Whether a value is set.
    pub configured: bool,
    /// `hex(SHA-256(value))[0..8]`, so admins can tell keys apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// When it was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_at: Option<Timestamp>,
    /// Where it was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_via: Option<SecretSetVia>,
}

/// Response of `GET /api/v1/install/secrets` and `GET /mgmt/v1/secrets`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SecretsResponse {
    /// Every known secret name, configured or not.
    pub items: Vec<SecretStatus>,
}
