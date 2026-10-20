//! Notifications (S21, prd-sew-21 §12, §18; contracts rev 34): per-member notification levels,
//! mutes and keywords, the `notification_created` / `notifications_released` /
//! `notification_prefs_updated` event bodies, READY's notification section and the email digest
//! switch.
//!
//! A level resolves channel → space → account → install (`notifications.default_level`); DMs
//! and group DMs default to `all`; agents default to `mentions`. Keyword rules that need Unicode
//! normalization (length after trimming, duplicates, the 2,048-byte total, `@` prefixes) are
//! checked by the server; `validate()` here only checks the count.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::changes::NotificationReason;
use crate::error::{Validate, ValidationError, check_known};
use crate::http::Patch;
use crate::id::Id;
use crate::time::Timestamp;

/// `details.reason` values of S21 errors.
pub mod reasons {
    /// `422`: prefs of a public channel the caller is not a member of.
    pub const NOT_A_MEMBER: &str = "not_a_member";
    /// `422`: the digest cannot be turned on without `[mail.smtp]`; the mail test route needs it.
    pub const MAIL_UNCONFIGURED: &str = "mail_unconfigured";
    /// `400`: a keyword starting with `@` (mentions have their own path).
    pub const KEYWORD_IS_MENTION: &str = "keyword_is_mention";
    /// `409`: `if_rev` is not the settings' current rev (`details.current_rev`).
    pub const REV_MISMATCH: &str = "rev_mismatch";
}

/// `NotifySettings.digest.reason` values (§12.2).
pub mod digest_reasons {
    /// The org policy `digest.enabled` is off.
    pub const POLICY_OFF: &str = "policy_off";
    /// The server has no `[mail.smtp]`.
    pub const MAIL_UNCONFIGURED: &str = "mail_unconfigured";
    /// The member has no email address.
    pub const NO_EMAIL: &str = "no_email";
    /// Agents never get a digest.
    pub const AGENT: &str = "agent";
    /// The member turned the digest off.
    pub const INTERVAL_OFF: &str = "interval_off";
}

/// Keywords per member.
pub const KEYWORDS_MAX: usize = 50;
/// Characters of one keyword after trimming.
pub const KEYWORD_MIN_CHARS: usize = 2;
/// Characters of one keyword after trimming.
pub const KEYWORD_MAX_CHARS: usize = 64;
/// Total bytes of the normalized keyword list.
pub const KEYWORDS_MAX_BYTES: usize = 2_048;
/// A mute lasts at most this many days.
pub const MUTE_MAX_DAYS: i64 = 366;

coded_enum! {
    /// A notification level (`*.level` codes 1–3).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum NotifyLevel {
        /// Every message.
        All = "all" => 1,
        /// Mentions, DMs, keywords and followed threads.
        Mentions = "mentions" => 2,
        /// Nothing (reminders still arrive).
        None = "none" => 3,
    }
}

unit_enum! {
    /// How often the email digest may be sent (`notify_settings.digest_interval` codes 0–3).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum DigestInterval {
        /// No digest.
        Off = "off",
        /// At most every 15 minutes.
        M15 = "15m",
        /// At most every hour (the default).
        H1 = "1h",
        /// At most every day.
        D1 = "1d",
    }
}

impl DigestInterval {
    /// The storage code (0 off, 1 15m, 2 1h, 3 1d). `Unknown` stores as the default `1h`.
    pub const fn as_i16(self) -> i16 {
        match self {
            DigestInterval::Off => 0,
            DigestInterval::M15 => 1,
            DigestInterval::H1 | DigestInterval::Unknown => 2,
            DigestInterval::D1 => 3,
        }
    }

    /// Looks up a storage code.
    pub const fn from_i16(code: i16) -> Option<Self> {
        match code {
            0 => Some(DigestInterval::Off),
            1 => Some(DigestInterval::M15),
            2 => Some(DigestInterval::H1),
            3 => Some(DigestInterval::D1),
            _ => None,
        }
    }

    /// The interval in milliseconds (`Off` → 0).
    pub const fn ms(self) -> u64 {
        match self {
            DigestInterval::M15 => 15 * 60_000,
            DigestInterval::H1 => 60 * 60_000,
            DigestInterval::D1 => 24 * 60 * 60_000,
            DigestInterval::Off | DigestInterval::Unknown => 0,
        }
    }
}

unit_enum! {
    /// Org policy `digest.detail`: what a digest mail names (gap G13).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum DigestDetail {
        /// Conversations, counts, author names and times.
        Names = "names",
        /// Only the total and an "Open Sew" link.
        Counts = "counts",
    }
}

/// Whether the member gets an email digest, and why not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DigestAvailability {
    /// The member's interval.
    pub interval: DigestInterval,
    /// A digest can be sent to this member now.
    pub available: bool,
    /// Why not: `policy_off`, `mail_unconfigured`, `no_email`, `agent`, `interval_off`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The member's account-level notification settings (`GET /api/v1/me/notify`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotifySettings {
    /// The account default; `null` = the install default.
    pub default_level: Option<NotifyLevel>,
    /// What space channels fall back to for this member.
    pub effective_default_level: NotifyLevel,
    /// Replies in followed threads notify.
    pub thread_replies: bool,
    /// `@channel` and `@here` notify.
    pub broad_mentions: bool,
    /// Direct mentions still notify in muted channels.
    pub mute_lets_mentions: bool,
    /// The email digest.
    pub digest: DigestAvailability,
    /// The keywords (only in `GET /me/notify` and `POST /me/notify/set`; never in READY or
    /// events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    /// How many keywords are set.
    pub keyword_count: u32,
    /// Bumped by every change (`if_rev`).
    pub rev: u32,
}

/// Body of `POST /api/v1/me/notify/set`. Returns [`NotifySettings`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateNotifySettingsRequest {
    /// The account default; `null` = back to the install default.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub default_level: Patch<NotifyLevel>,
    /// Replies in followed threads notify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_replies: Option<bool>,
    /// `@channel` and `@here` notify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broad_mentions: Option<bool>,
    /// Direct mentions still notify in muted channels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mute_lets_mentions: Option<bool>,
    /// The digest interval (accepted and ignored for agents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest_interval: Option<DigestInterval>,
    /// The keywords; replaces the list (`[]` clears it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    /// Refuse the write (`409 conflict`) unless the settings' `rev` is this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_rev: Option<u32>,
}

impl Validate for UpdateNotifySettingsRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(l) = self.default_level.as_set() {
            check_known("default_level", *l)?;
        }
        if let Some(i) = self.digest_interval {
            check_known("digest_interval", i)?;
        }
        if let Some(k) = &self.keywords
            && k.len() > KEYWORDS_MAX
        {
            return Err(ValidationError::invalid(
                format!("keywords[{KEYWORDS_MAX}]"),
                format!("at most {KEYWORDS_MAX} keywords"),
            ));
        }
        Ok(())
    }
}

/// A channel mute in a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MuteInput {
    /// Muted.
    pub muted: bool,
    /// Muted until then (after now, at most 366 days ahead); absent = until unmuted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Timestamp>,
}

/// A member's prefs for one channel or DM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelNotifyPrefs {
    /// The channel.
    pub channel_id: Id,
    /// The channel's own level; `null` = inherit.
    pub level: Option<NotifyLevel>,
    /// The level in effect for this member here.
    pub effective_level: NotifyLevel,
    /// Muted now.
    pub muted: bool,
    /// When the mute ends; absent = until unmuted (or not muted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted_until: Option<Timestamp>,
}

/// Body of `POST /api/v1/channels/{id}/notify/set`. Returns [`ChannelNotifyPrefs`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetChannelNotifyRequest {
    /// The level; `null` = inherit.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub level: Patch<NotifyLevel>,
    /// The mute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mute: Option<MuteInput>,
}

impl Validate for SetChannelNotifyRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(l) = self.level.as_set() {
            check_known("level", *l)?;
        }
        if self.level.is_keep() && self.mute.is_none() {
            return Err(ValidationError::invalid(
                "level",
                "set level or mute (or both)",
            ));
        }
        if let Some(m) = &self.mute
            && !m.muted
            && m.until.is_some()
        {
            return Err(ValidationError::invalid(
                "mute.until",
                "mute.until needs muted = true",
            ));
        }
        Ok(())
    }
}

/// A member's level for one space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpaceNotifyPrefs {
    /// The space.
    pub space_id: Id,
    /// The space's level; `null` = inherit.
    pub level: Option<NotifyLevel>,
    /// What the space's channels fall back to for this member.
    pub effective_level: NotifyLevel,
}

/// Body of `POST /api/v1/spaces/{id}/notify/set`. Returns [`SpaceNotifyPrefs`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSpaceNotifyRequest {
    /// The level; `null` = inherit.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub level: Patch<NotifyLevel>,
}

impl Validate for SetSpaceNotifyRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        match &self.level {
            Patch::Keep => Err(ValidationError::invalid(
                "level",
                "level is required (null = inherit)",
            )),
            Patch::Set(l) => check_known("level", *l),
            Patch::Clear => Ok(()),
        }
    }
}

/// Response of `GET /api/v1/me/notify/prefs`: explicit rows only, for channels and spaces the
/// caller can see.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotifyPrefsList {
    /// Channels and DMs with a level or a mute.
    pub channels: Vec<ChannelNotifyPrefs>,
    /// Spaces with a level.
    pub spaces: Vec<SpaceNotifyPrefs>,
}

/// Body of `notification_created`: ids only, never the message (gap G20). The client already
/// holds the message from `message_created`, or fetches it by id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Notification {
    /// Why.
    pub reason: NotificationReason,
    /// The message.
    pub message_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// Its space (`null` in DMs).
    pub space_id: Option<Id>,
    /// Its thread root, for replies.
    pub thread_root_id: Option<Id>,
    /// Its author.
    pub author_id: Id,
    /// When the message was created.
    pub message_created_at: Timestamp,
    /// When the notification was written.
    pub at: Timestamp,
}

/// Body of `notifications_released`: what DND held and is still unread, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationsReleased {
    /// At most 50 items.
    pub items: Vec<Notification>,
    /// Released items beyond `items`.
    pub more: u32,
}

/// Body of `notification_prefs_updated`: exactly one field is set. `settings` never carries
/// keywords.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationPrefsUpdated {
    /// The account settings changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<NotifySettings>,
    /// A channel's prefs changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<ChannelNotifyPrefs>,
    /// A space's prefs changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<SpaceNotifyPrefs>,
}

/// READY's notification section: the settings (without keywords) and the explicit rows for
/// READY's channels and spaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadyNotify {
    /// Account settings, without keywords.
    pub settings: NotifySettings,
    /// Channels and DMs with a level or a mute.
    pub channel_prefs: Vec<ChannelNotifyPrefs>,
    /// Spaces with a level.
    pub space_prefs: Vec<SpaceNotifyPrefs>,
}
