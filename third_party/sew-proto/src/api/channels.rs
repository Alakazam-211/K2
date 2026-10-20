//! Channels, membership and DMs (C4, C12; S10; gap G8).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::addr::{Handle, Slug};
use crate::error::{
    Validate, ValidationError, check_chars, check_known, check_opt_chars, check_unique_ids,
};
use crate::http::Patch;
use crate::id::Id;
use crate::limits::{GROUP_DM_MAX_OTHERS, NAME_MAX_CHARS, PURPOSE_MAX_CHARS, TOPIC_MAX_CHARS};
use crate::perms::Perms;
use crate::time::Timestamp;

unit_enum! {
    /// Channel type (gap G8 adds `dm` and `group_dm`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ChannelType {
        /// A text channel.
        Text = "text",
        /// A voice channel.
        Voice = "voice",
        /// An announcement channel: `@everyone` posts only in threads by default.
        Announcement = "announcement",
        /// A 1:1 DM (no space, slug or name; always private).
        Dm = "dm",
        /// A group DM of up to 9 people.
        GroupDm = "group_dm",
    }
}

impl ChannelType {
    /// Whether this is a DM or group DM.
    pub const fn is_dm(self) -> bool {
        matches!(self, ChannelType::Dm | ChannelType::GroupDm)
    }
}

unit_enum! {
    /// Channel visibility.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum Visibility {
        /// Every space member can see and join it.
        Public = "public",
        /// Members only.
        Private = "private",
    }
}

/// A channel, DM or group DM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Channel {
    /// The channel id.
    pub id: Id,
    /// Its space; absent for DMs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// Its category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_id: Option<Id>,
    /// The channel type.
    #[serde(rename = "type")]
    pub kind: ChannelType,
    /// Public or private; DMs are private.
    pub visibility: Visibility,
    /// The slug; absent for DMs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<Slug>,
    /// The name; absent for DMs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The topic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// The purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Sort position in its category.
    pub position: u32,
    /// Whether overwrites follow the category.
    pub synced: bool,
    /// When the channel was archived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<Timestamp>,
    /// Head of the channel change log.
    pub head_seq: u64,
    /// The newest message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_id: Option<Id>,
    /// The DM's members; empty for space channels.
    pub dm_member_ids: Vec<Id>,
    /// The viewer's effective permissions here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_perms: Option<Perms>,
    /// When the channel was created.
    pub created_at: Timestamp,
    /// Joined members (S10). Old payloads without it read as 0.
    #[serde(default)]
    pub member_count: u32,
    /// Who created the channel (S10). Optional on the wire so older payloads still parse; the
    /// server always sends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Id>,
    /// Slow mode in seconds, 0 = off (S12 §7). Always sent; old payloads read as 0.
    #[serde(default)]
    pub slow_mode_s: u32,
    /// Effective message retention in days (S22 §14.3; holds ignored). Absent = forever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u32>,
    /// Effective file retention in days (S22 §14.3). Absent = forever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_retention_days: Option<u32>,
    /// At least one guest is a member (S19 §15): "this channel has guests" / "this
    /// conversation has guests". Old payloads read as `false`.
    #[serde(default)]
    pub has_guests: bool,
    /// Assistants that read this DM or group DM through their pinned person (contracts rev 11
    /// G21, S19 §8). Empty (and absent) for space channels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assistant_reader_ids: Vec<Id>,
    /// Voice channels: at most this many people in the call, 0 = no per-channel limit (S33 v1
    /// §10.4). Absent (0) for other types.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub user_limit: u8,
    /// Voice channels: a recording starts on the first join (S35 v2 §8.3). Absent = off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recording_auto: bool,
    /// The channel's default audio keep days for recordings (S35 v2 §12.1); absent = the org
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_audio_days: Option<u16>,
    /// Voice channels: declining the recording notice means not joining (S35 v2 §9.11);
    /// absent = the org policy `recording.require_consent_to_join`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_require_consent: Option<bool>,
    /// A meeting channel's meeting (Meet MC2a, CC6); absent on every other channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting: Option<crate::api::meetings::ChannelMeeting>,
}

/// The largest `recording_audio_days` a channel may store (S35 v2 §10; the org's
/// `recording.audio_max_days` clamps what a recording uses).
pub const RECORDING_AUDIO_DAYS_MAX: u16 = 36_500;

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

/// Body of the create-channel route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateChannelRequest {
    /// The slug, unique in the space.
    pub slug: Slug,
    /// The name, 1–100 characters.
    pub name: String,
    /// `text`, `voice` or `announcement`.
    #[serde(rename = "type")]
    pub kind: ChannelType,
    /// Public or private.
    pub visibility: Visibility,
    /// The category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_id: Option<Id>,
    /// The topic, at most 250 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// The purpose, at most 250 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Position in its group; default last (S10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    /// Members to add at creation, besides the creator: at most 100, distinct, never the caller
    /// (S10).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub member_ids: Vec<Id>,
    /// Voice channels only: the per-channel limit, 0–99 (S33 v1 §8). Any value on another type
    /// is refused (`400 validation_failed`, field `user_limit`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_limit: Option<u8>,
}

impl Validate for CreateChannelRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_unique_ids("member_ids", &self.member_ids, 0, 100)?;
        crate::voice::check_user_limit(self.user_limit)?;
        if self.user_limit.is_some() && self.kind != ChannelType::Voice {
            return Err(ValidationError::invalid(
                "user_limit",
                "user_limit is for voice channels only",
            ));
        }
        check_chars("name", &self.name, 1, NAME_MAX_CHARS)?;
        match self.kind {
            ChannelType::Text | ChannelType::Voice | ChannelType::Announcement => {}
            _ => {
                return Err(ValidationError::invalid(
                    "type",
                    "type must be text, voice or announcement",
                ));
            }
        }
        check_known("visibility", self.visibility)?;
        check_opt_chars("topic", self.topic.as_deref(), 0, TOPIC_MAX_CHARS)?;
        check_opt_chars("purpose", self.purpose.as_deref(), 0, PURPOSE_MAX_CHARS)
    }
}

/// Body of the update-channel route.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateChannelRequest {
    /// A new slug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<Slug>,
    /// A new name, 1–100 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Set or clear the topic (≤ 250).
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub topic: Patch<String>,
    /// Set or clear the purpose (≤ 250).
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub purpose: Patch<String>,
    /// Move to a category or out of one.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub category_id: Patch<Id>,
    /// A new position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    /// Voice channels only: a new per-channel limit, 0–99 (0 = none; S33 v1 §8). The server
    /// refuses it on another channel type (`400 validation_failed`, field `user_limit`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_limit: Option<u8>,
    /// Voice channels only: auto-record (S35 v2 §8.3). Needs MANAGE_CHANNELS and
    /// RECORD_CALLS; on another channel type `400 validation_failed`, field `recording_auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_auto: Option<bool>,
    /// Voice channels only: set (0–36,500) or clear (the org default) the channel's default
    /// audio keep days (S35 v2 §12.1). Needs MANAGE_CHANNELS and RECORD_CALLS.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub recording_audio_days: Patch<u16>,
    /// Voice channels only: set or clear (inherit) "declining means not joining" (S35 v2
    /// §9.11). Needs MANAGE_CHANNELS and RECORD_CALLS.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub recording_require_consent: Patch<bool>,
}

impl Validate for UpdateChannelRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        crate::voice::check_user_limit(self.user_limit)?;
        if let Patch::Set(d) = self.recording_audio_days
            && d > RECORDING_AUDIO_DAYS_MAX
        {
            return Err(ValidationError::invalid(
                "recording_audio_days",
                format!("recording_audio_days must be 0–{RECORDING_AUDIO_DAYS_MAX}"),
            ));
        }
        check_opt_chars("name", self.name.as_deref(), 1, NAME_MAX_CHARS)?;
        check_opt_chars(
            "topic",
            self.topic.as_set().map(String::as_str),
            0,
            TOPIC_MAX_CHARS,
        )?;
        check_opt_chars(
            "purpose",
            self.purpose.as_set().map(String::as_str),
            0,
            PURPOSE_MAX_CHARS,
        )
    }
}

/// A channel membership row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelMember {
    /// The channel.
    pub channel_id: Id,
    /// The member.
    pub member_id: Id,
    /// When they joined.
    pub joined_at: Timestamp,
}

/// Body of the add-channel-members route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddChannelMembersRequest {
    /// 1–100 distinct member ids.
    pub member_ids: Vec<Id>,
}

impl Validate for AddChannelMembersRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_unique_ids("member_ids", &self.member_ids, 1, 100)
    }
}

/// Body of the remove-channel-member route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveChannelMemberRequest {
    /// The member to remove.
    pub member_id: Id,
}

impl Validate for RemoveChannelMemberRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Body of the open-DM route: 1 other member opens a `dm`, 2–8 open a `group_dm`. The others
/// are named by id, by handle (SW12, S30 G5), or both: 1–8 distinct members in all. A handle
/// names an active member; one that names nobody the caller may see is the same masked `404`
/// as an unknown id (`details.username`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenDmRequest {
    /// Other members by id (with `usernames`, 1–8 distinct members in all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub member_ids: Vec<Id>,
    /// Other members by handle (SW12; additive).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usernames: Vec<Handle>,
}

impl Validate for OpenDmRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        let total = self.member_ids.len() + self.usernames.len();
        if total == 0 || total > GROUP_DM_MAX_OTHERS {
            return Err(ValidationError::invalid(
                if self.member_ids.is_empty() {
                    "usernames"
                } else {
                    "member_ids"
                },
                format!(
                    "member_ids and usernames must name 1–{GROUP_DM_MAX_OTHERS} members in all, got {total}"
                ),
            ));
        }
        check_unique_ids("member_ids", &self.member_ids, 0, GROUP_DM_MAX_OTHERS)?;
        let mut seen = std::collections::HashSet::with_capacity(self.usernames.len());
        for h in &self.usernames {
            if !seen.insert(h.as_str()) {
                return Err(ValidationError::invalid(
                    "usernames",
                    format!("usernames holds {h} twice"),
                ));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- S10 additions (prd-sew-10 §8)

/// A page of channels (`GET /api/v1/spaces/{id}/channels`, `GET /api/v1/me/channels`).
pub type ChannelPage = crate::http::Page<Channel>;

unit_enum! {
    /// Which channels `GET /api/v1/spaces/{id}/channels` lists.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ChannelListScope {
        /// Public channels the caller can view plus private ones it belongs to (the default).
        Browse = "browse",
        /// Channels the caller belongs to.
        Joined = "joined",
        /// Every channel, private ones redacted (space admins and install admins).
        Admin = "admin",
    }
}

/// Query of `GET /api/v1/spaces/{id}/channels`: ordered by id ascending, `after` cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelListQuery {
    /// Which channels; default `browse`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ChannelListScope>,
    /// Include archived channels (default false).
    #[serde(default)]
    pub include_archived: bool,
    /// Case-insensitive substring of the slug or name, 1–80 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Channels with a larger id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Validate for ChannelListQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(s) = self.scope {
            check_known("scope", s)?;
        }
        check_opt_chars("q", self.q.as_deref(), 1, 80)?;
        crate::api::spaces::check_page_limit(self.limit)
    }
}

unit_enum! {
    /// `GET /api/v1/me/channels?kind=`: DMs and group DMs, or space channels.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MyChannelsKind {
        /// DMs and group DMs.
        Dm = "dm",
        /// Space channels.
        Space = "space",
    }
}

/// Query of `GET /api/v1/me/channels`: every channel the caller belongs to and can view, by id
/// ascending with an `after` cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MyChannelsQuery {
    /// Only DMs or only space channels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<MyChannelsKind>,
    /// Channels with a larger id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Validate for MyChannelsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(k) = self.kind {
            check_known("kind", k)?;
        }
        crate::api::spaces::check_page_limit(self.limit)
    }
}

/// Response of `POST /api/v1/channels/{id}/members/add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AddChannelMembersResponse {
    /// The memberships created.
    pub added: Vec<ChannelMember>,
    /// Ids that already were members.
    pub already: Vec<Id>,
}

/// One channel's new place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReorderChannelItem {
    /// The channel.
    pub channel_id: Id,
    /// Absent keeps the category, `null` moves to uncategorized, an id moves there.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub category_id: Patch<Id>,
    /// The new position in the (new) group.
    pub position: u32,
}

/// Body of `POST /api/v1/spaces/{id}/channels/reorder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReorderChannelsRequest {
    /// 1–500 items with distinct channel ids.
    pub items: Vec<ReorderChannelItem>,
}

impl Validate for ReorderChannelsRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        let ids: Vec<Id> = self.items.iter().map(|i| i.channel_id).collect();
        check_unique_ids("items", &ids, 1, 500)
    }
}

/// Response of `POST /api/v1/spaces/{id}/channels/reorder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReorderChannelsResponse {
    /// Every channel whose position or category changed.
    pub channels: Vec<Channel>,
}
