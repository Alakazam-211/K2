//! Org policies (C13, gaps G10, G29; contracts revs 6, 10).
//!
//! Keys are `area.name`. Set by the install owner or an admin through
//! `POST /api/v1/install/policies` or the mgmt API; every change is audited.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::notify::{DigestDetail, NotifyLevel};
use crate::e2ee::VerificationCode;
use crate::error::{Validate, ValidationError, check_known};
use crate::http::Patch;

unit_enum! {
    /// A policy key (C13).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum PolicyKey {
        /// Who can create spaces.
        SpacesCreate = "spaces.create",
        /// Extra install-level gate on top of bit 41.
        ChannelsCreatePublicFloor = "channels.create_public_floor",
        /// Extra install-level gate on top of bit 42.
        ChannelsCreatePrivateFloor = "channels.create_private_floor",
        /// Who can archive channels.
        ChannelsArchive = "channels.archive",
        /// `@channel`/`@here` in bigger channels needs MANAGE_CHANNELS.
        MentionsChannelMaxMembers = "mentions.channel_max_members",
        /// Who can invite guests.
        GuestsInvite = "guests.invite",
        /// Whether guests may DM people they share no channel with.
        GuestsDmNonChannelMates = "guests.dm_non_channel_mates",
        /// Edit window in seconds; 0 = forever.
        MessagesEditWindowS = "messages.edit_window_s",
        /// Whether members may delete their own messages.
        MessagesDeleteOwn = "messages.delete_own",
        /// GIF search on/off.
        GifsEnabled = "gifs.enabled",
        /// GIF provider name.
        GifsProvider = "gifs.provider",
        /// Reference to the GIF provider's API key.
        GifsApiKeyRef = "gifs.api_key_ref",
        /// Link previews on/off.
        LinkPreviewsEnabled = "link_previews.enabled",
        /// Largest upload in bytes.
        FilesMaxBytes = "files.max_bytes",
        /// Per-space file quota in bytes; 0 = none.
        FilesSpaceQuotaBytes = "files.space_quota_bytes",
        /// Custom emoji per space.
        EmojiMaxPerSpace = "emoji.max_per_space",
        /// Company retention default in days; 0 = forever.
        RetentionDefaultDays = "retention.default_days",
        /// Whether space admins may join private channels by themselves.
        PrivateSpaceAdminsCanJoin = "private.space_admins_can_join",
        /// Whether guests see presence.
        PresenceShowToGuests = "presence.show_to_guests",
        /// Email digest on/off.
        DigestEnabled = "digest.enabled",
        /// Whether members may DM agent members.
        AgentsAllowedInDms = "agents.allowed_in_dms",
        /// Strip GPS and other location metadata from image originals at upload (S15 G14).
        FilesStripImageLocation = "files.strip_image_location",
        /// Days deleted and edited-away content is kept before purge; 0 = off (S22, Q12).
        /// A read-only mirror of the install retention row.
        MessagesDeletedPurgeDays = "messages.deleted_purge_days",
        /// Guest access on/off; off suspends every guest reversibly (S19 §7.7, contracts
        /// rev 23).
        GuestsEnabled = "guests.enabled",
        /// The install's default notification level for people (S21 gap G1).
        NotificationsDefaultLevel = "notifications.default_level",
        /// What an email digest names: `names` or `counts` (S21 gap G13).
        DigestDetail = "digest.detail",
        /// Whether the call security code is shown: `shown` or `hidden`; `required` is
        /// reserved and refused (S50 §16.4, G17).
        CallsVerificationCode = "calls.verification_code",
        /// Whether members may start calls in DMs and group DMs (S34 v2 §7.2, §24).
        CallsDmEnabled = "calls.dm_enabled",
        /// Whether members may record calls (S35 v2 §21.1).
        RecordingEnabled = "recording.enabled",
        /// Whether persons may record DM and group-DM calls (S35 v2; feeds
        /// `PermPolicies.recording_dm_enabled`).
        RecordingDmEnabled = "recording.dm_enabled",
        /// The default audio keep days of a recording (S35 v2 §12.1).
        RecordingAudioDefaultDays = "recording.audio_default_days",
        /// The most audio keep days a recording may ask for (S35 v2 §12.1).
        RecordingAudioMaxDays = "recording.audio_max_days",
        /// Declining the recording notice means not joining (S35 v2 §9.11, rev 91).
        RecordingRequireConsentToJoin = "recording.require_consent_to_join",
        /// Meetings on/off (Meet MC1, CC8).
        MeetingsEnabled = "meetings.enabled",
        /// Who may host meetings (Meet MC1, CC8).
        MeetingsCreate = "meetings.create",
        /// Most people in one meeting (Meet MC1, CC8).
        MeetingsMaxParticipants = "meetings.max_participants",
        /// Longest planned meeting, `ends_at` − `starts_at`, in minutes (Meet MC1, CC8).
        MeetingsMaxDurationMinutes = "meetings.max_duration_minutes",
        /// How long an empty room waits after `ends_at` before the meeting ends (Meet MC1, CC8).
        MeetingsGraceMinutes = "meetings.grace_minutes",
        /// Free text naming who to ask about meeting privacy, shown to guests (Meet MC1, CC8).
        MeetingsPrivacyContact = "meetings.privacy_contact",
        /// Meeting links let outside guests in (Meet MC2a, CC8): off refuses `peek` and
        /// `redeem` and revokes every link-guest session.
        MeetingsLinkGuests = "meetings.link_guests",
        /// The call-stats beacon on/off (F21, prd-meet-01 CC14): on, voice joins carry
        /// `stats` and `POST /api/v1/me/voice/stats` takes aggregate samples; off (the
        /// default), the route answers `403 policy_denied`.
        CallsStatsBeacon = "calls.stats_beacon",
    }
}

unit_enum! {
    /// `meetings.create`: who may host a meeting (Meet MC1).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MeetingsCreate {
        /// Any active non-guest member of the Meetings Space (on a standalone install, any
        /// active non-guest member: the first create adds them to the Space).
        Members = "members",
        /// Install owners and admins only.
        Admins = "admins",
    }
}

/// Default of `meetings.max_participants`.
pub const DEFAULT_MEETINGS_MAX_PARTICIPANTS: u32 = 50;
/// Highest `meetings.max_participants` (one room's capacity, prd-meet-01 §6.12).
pub const MEETINGS_MAX_PARTICIPANTS_LIMIT: u32 = 200;
/// Default of `meetings.max_duration_minutes` (8 h).
pub const DEFAULT_MEETINGS_MAX_DURATION_MINUTES: u32 = 480;
/// Highest `meetings.max_duration_minutes` (the 12 h hard cap).
pub const MEETINGS_MAX_DURATION_LIMIT: u32 = 720;
/// Default of `meetings.grace_minutes`.
pub const DEFAULT_MEETINGS_GRACE_MINUTES: u32 = 30;
/// Highest `meetings.grace_minutes`.
pub const MEETINGS_GRACE_LIMIT: u32 = 720;
/// Longest `meetings.privacy_contact`, in characters.
pub const MEETINGS_PRIVACY_CONTACT_MAX_CHARS: usize = 300;

fn default_meetings_create() -> MeetingsCreate {
    MeetingsCreate::Members
}

fn default_meetings_max_participants() -> u32 {
    DEFAULT_MEETINGS_MAX_PARTICIPANTS
}

fn default_meetings_max_duration_minutes() -> u32 {
    DEFAULT_MEETINGS_MAX_DURATION_MINUTES
}

fn default_meetings_grace_minutes() -> u32 {
    DEFAULT_MEETINGS_GRACE_MINUTES
}

unit_enum! {
    /// `spaces.create`: who can create spaces.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum SpacesCreate {
        /// Install owners and admins only.
        Admins = "admins",
        /// Any member.
        Members = "members",
    }
}

unit_enum! {
    /// `channels.create_*_floor`: the install-level gate on top of the create bit
    /// (gap G29 as amended by contracts revs 6 and 10).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ChannelCreateFloor {
        /// The permission bit alone suffices.
        Perm = "perm",
        /// The bit plus being a space admin.
        SpaceAdmins = "space_admins",
        /// The bit plus being an install owner or admin.
        InstallAdmins = "install_admins",
    }
}

unit_enum! {
    /// `channels.archive`: who can archive a channel.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ArchiveWho {
        /// The channel's creator and channel managers.
        CreatorAndManagers = "creator_and_managers",
        /// Channel managers only.
        Managers = "managers",
    }
}

unit_enum! {
    /// `guests.invite`: who can invite guests.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum GuestInvite {
        /// Install owners and admins only.
        Admins = "admins",
        /// Anyone holding INVITE_GUESTS (bit 44).
        Perm = "perm",
    }
}

/// Default for `mentions.channel_max_members`.
pub const DEFAULT_MENTIONS_CHANNEL_MAX_MEMBERS: u32 = 1_000;
/// Default for `files.max_bytes` (1 GiB).
pub const DEFAULT_FILES_MAX_BYTES: u64 = 1_073_741_824;
/// Default for `emoji.max_per_space`.
pub const DEFAULT_EMOJI_MAX_PER_SPACE: u32 = 500;
/// Longest `gifs.provider`.
pub const GIFS_PROVIDER_MAX_LEN: usize = 64;
/// Longest `gifs.api_key_ref`.
pub const GIFS_API_KEY_REF_MAX_LEN: usize = 256;

/// Every org policy value. `Default` is the C13 defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Policies {
    /// Who can create spaces.
    #[serde(rename = "spaces.create")]
    pub spaces_create: SpacesCreate,
    /// Extra gate on top of CREATE_PUBLIC_CHANNELS.
    #[serde(rename = "channels.create_public_floor")]
    pub channels_create_public_floor: ChannelCreateFloor,
    /// Extra gate on top of CREATE_PRIVATE_CHANNELS.
    #[serde(rename = "channels.create_private_floor")]
    pub channels_create_private_floor: ChannelCreateFloor,
    /// Who can archive channels.
    #[serde(rename = "channels.archive")]
    pub channels_archive: ArchiveWho,
    /// Above this many members, `@channel`/`@here` need MANAGE_CHANNELS (≥ 2).
    #[serde(rename = "mentions.channel_max_members")]
    pub mentions_channel_max_members: u32,
    /// Who can invite guests.
    #[serde(rename = "guests.invite")]
    pub guests_invite: GuestInvite,
    /// Whether guests may DM people they share no channel with.
    #[serde(rename = "guests.dm_non_channel_mates")]
    pub guests_dm_non_channel_mates: bool,
    /// Edit window in seconds; 0 = forever.
    #[serde(rename = "messages.edit_window_s")]
    pub messages_edit_window_s: u64,
    /// Whether members may delete their own messages.
    #[serde(rename = "messages.delete_own")]
    pub messages_delete_own: bool,
    /// GIF search on/off. Uploaded GIFs always work.
    #[serde(rename = "gifs.enabled")]
    pub gifs_enabled: bool,
    /// GIF provider name (≤ 64).
    #[serde(
        rename = "gifs.provider",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub gifs_provider: Option<String>,
    /// Reference to the GIF provider's API key (≤ 256); never the key itself.
    #[serde(
        rename = "gifs.api_key_ref",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub gifs_api_key_ref: Option<String>,
    /// Link previews on/off.
    #[serde(rename = "link_previews.enabled")]
    pub link_previews_enabled: bool,
    /// Largest upload in bytes (≥ 1).
    #[serde(rename = "files.max_bytes")]
    pub files_max_bytes: u64,
    /// Per-space file quota in bytes; 0 = none.
    #[serde(rename = "files.space_quota_bytes")]
    pub files_space_quota_bytes: u64,
    /// Custom emoji per space.
    #[serde(rename = "emoji.max_per_space")]
    pub emoji_max_per_space: u32,
    /// Company retention default in days; 0 = forever.
    #[serde(rename = "retention.default_days")]
    pub retention_default_days: u32,
    /// Whether space admins may join private channels by themselves.
    #[serde(rename = "private.space_admins_can_join")]
    pub private_space_admins_can_join: bool,
    /// Whether guests see presence.
    #[serde(rename = "presence.show_to_guests")]
    pub presence_show_to_guests: bool,
    /// Email digest on/off.
    #[serde(rename = "digest.enabled")]
    pub digest_enabled: bool,
    /// Whether members may DM agent members.
    #[serde(rename = "agents.allowed_in_dms")]
    pub agents_allowed_in_dms: bool,
    /// Strip GPS and other location EXIF/XMP from JPEG, PNG, WebP and HEIC originals when an
    /// upload finishes; orientation and colour profile stay (S15, contracts rev 25 G14).
    #[serde(rename = "files.strip_image_location", default = "default_true")]
    pub files_strip_image_location: bool,
    /// Days deleted and edited-away content is kept before S22 purges it; 0 = off (Q12). A
    /// read-only mirror of the install retention row (contracts rev 33 G1).
    #[serde(
        rename = "messages.deleted_purge_days",
        default = "default_deleted_purge_days"
    )]
    pub messages_deleted_purge_days: u32,
    /// Guest access on/off (S19 §7.7): off revokes every guest session and refuses guest logins
    /// until it is on again; memberships, roles and DMs stay.
    #[serde(rename = "guests.enabled", default = "default_true")]
    pub guests_enabled: bool,
    /// The notification level people's space channels fall back to (S21 gap G1). Agents stay
    /// at `mentions` (contracts rev 11); DMs default to `all`.
    #[serde(
        rename = "notifications.default_level",
        default = "default_notify_level"
    )]
    pub notifications_default_level: NotifyLevel,
    /// What an email digest names (S21 gap G13).
    #[serde(rename = "digest.detail", default = "default_digest_detail")]
    pub digest_detail: DigestDetail,
    /// Whether "Call info → Security code" is shown (S50 §16.4). Clients read it in READY
    /// `call_keys.verification_code` and the policy digest.
    #[serde(
        rename = "calls.verification_code",
        default = "default_verification_code"
    )]
    pub calls_verification_code: VerificationCode,
    /// Whether members may start and join calls in DMs and group DMs (S34 v2 §7.2); off
    /// answers `403 policy_denied` with reason `dm_calls_disabled`.
    #[serde(rename = "calls.dm_enabled", default = "default_true")]
    pub calls_dm_enabled: bool,
    /// The call-stats beacon (F21): on, browsers send aggregate, anonymous call samples
    /// (`POST /api/v1/me/voice/stats`) that Sew adds to `/metrics`; off (the default), joins
    /// carry no `stats` and the route answers `403 policy_denied` `call_stats_off`.
    #[serde(rename = "calls.stats_beacon", default)]
    pub calls_stats_beacon: bool,
    /// Whether members may record calls (S35 v2 §21.1); off answers `403 policy_denied`
    /// `recording_disabled`.
    #[serde(rename = "recording.enabled", default)]
    pub recording_enabled: bool,
    /// Whether persons may record DM calls (S35 v2; `dm_perms` grants RECORD_CALLS).
    #[serde(rename = "recording.dm_enabled", default)]
    pub recording_dm_enabled: bool,
    /// The default audio keep days (S35 v2 §12.1).
    #[serde(
        rename = "recording.audio_default_days",
        default = "default_recording_audio_default_days"
    )]
    pub recording_audio_default_days: u16,
    /// The most audio keep days (S35 v2 §12.1).
    #[serde(
        rename = "recording.audio_max_days",
        default = "default_recording_audio_max_days"
    )]
    pub recording_audio_max_days: u16,
    /// Declining the recording notice means not joining (S35 v2 §9.11, rev 91).
    #[serde(rename = "recording.require_consent_to_join", default)]
    pub recording_require_consent_to_join: bool,
    /// Meetings on/off (Meet MC1): off refuses the meeting routes with `403 policy_denied`
    /// `meetings_disabled`; meetings already made keep their channels.
    #[serde(rename = "meetings.enabled", default)]
    pub meetings_enabled: bool,
    /// Who may host meetings (Meet MC1, §13 Q7).
    #[serde(rename = "meetings.create", default = "default_meetings_create")]
    pub meetings_create: MeetingsCreate,
    /// Most people in one meeting (2–200).
    #[serde(
        rename = "meetings.max_participants",
        default = "default_meetings_max_participants"
    )]
    pub meetings_max_participants: u32,
    /// Longest planned meeting in minutes (1–720).
    #[serde(
        rename = "meetings.max_duration_minutes",
        default = "default_meetings_max_duration_minutes"
    )]
    pub meetings_max_duration_minutes: u32,
    /// Minutes an empty room waits after `ends_at` before the meeting ends (0–720).
    #[serde(
        rename = "meetings.grace_minutes",
        default = "default_meetings_grace_minutes"
    )]
    pub meetings_grace_minutes: u32,
    /// Meeting links let outside guests in (Meet MC2a, CC8; default on). Off: `peek` and
    /// `redeem` answer `403 policy_denied`, and every link-guest session is revoked.
    /// `guests.enabled` does not govern link guests.
    #[serde(rename = "meetings.link_guests", default = "default_true")]
    pub meetings_link_guests: bool,
    /// Who to ask about meeting privacy (≤ 300 characters; optional).
    #[serde(
        rename = "meetings.privacy_contact",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub meetings_privacy_contact: Option<String>,
}

/// Default of `recording.audio_default_days` (S35 v2 §12.1).
pub const DEFAULT_RECORDING_AUDIO_DEFAULT_DAYS: u16 = 30;
/// Default of `recording.audio_max_days` (S35 v2 §12.1).
pub const DEFAULT_RECORDING_AUDIO_MAX_DAYS: u16 = 365;
/// Highest `recording.audio_max_days` (the column bound).
pub const RECORDING_AUDIO_DAYS_LIMIT: u16 = 36_500;

fn default_recording_audio_default_days() -> u16 {
    DEFAULT_RECORDING_AUDIO_DEFAULT_DAYS
}

fn default_recording_audio_max_days() -> u16 {
    DEFAULT_RECORDING_AUDIO_MAX_DAYS
}

fn default_verification_code() -> VerificationCode {
    VerificationCode::Shown
}

fn default_notify_level() -> NotifyLevel {
    NotifyLevel::Mentions
}

fn default_digest_detail() -> DigestDetail {
    DigestDetail::Names
}

/// Default of `messages.deleted_purge_days` (Q12 recommendation).
pub const DEFAULT_DELETED_PURGE_DAYS: u32 = 30;

fn default_deleted_purge_days() -> u32 {
    DEFAULT_DELETED_PURGE_DAYS
}

fn default_true() -> bool {
    true
}

impl Default for Policies {
    fn default() -> Self {
        Policies {
            spaces_create: SpacesCreate::Admins,
            channels_create_public_floor: ChannelCreateFloor::Perm,
            channels_create_private_floor: ChannelCreateFloor::Perm,
            channels_archive: ArchiveWho::CreatorAndManagers,
            mentions_channel_max_members: DEFAULT_MENTIONS_CHANNEL_MAX_MEMBERS,
            guests_invite: GuestInvite::Admins,
            guests_dm_non_channel_mates: false,
            messages_edit_window_s: 0,
            messages_delete_own: true,
            gifs_enabled: false,
            gifs_provider: None,
            gifs_api_key_ref: None,
            link_previews_enabled: true,
            files_max_bytes: DEFAULT_FILES_MAX_BYTES,
            files_space_quota_bytes: 0,
            emoji_max_per_space: DEFAULT_EMOJI_MAX_PER_SPACE,
            retention_default_days: 0,
            private_space_admins_can_join: false,
            presence_show_to_guests: false,
            digest_enabled: false,
            agents_allowed_in_dms: true,
            files_strip_image_location: true,
            messages_deleted_purge_days: DEFAULT_DELETED_PURGE_DAYS,
            guests_enabled: true,
            notifications_default_level: NotifyLevel::Mentions,
            digest_detail: DigestDetail::Names,
            calls_verification_code: VerificationCode::Shown,
            calls_dm_enabled: true,
            calls_stats_beacon: false,
            recording_enabled: false,
            recording_dm_enabled: false,
            recording_audio_default_days: DEFAULT_RECORDING_AUDIO_DEFAULT_DAYS,
            recording_audio_max_days: DEFAULT_RECORDING_AUDIO_MAX_DAYS,
            recording_require_consent_to_join: false,
            meetings_enabled: false,
            meetings_create: MeetingsCreate::Members,
            meetings_max_participants: DEFAULT_MEETINGS_MAX_PARTICIPANTS,
            meetings_max_duration_minutes: DEFAULT_MEETINGS_MAX_DURATION_MINUTES,
            meetings_grace_minutes: DEFAULT_MEETINGS_GRACE_MINUTES,
            meetings_link_guests: true,
            meetings_privacy_contact: None,
        }
    }
}

impl Policies {
    /// The subset READY carries (gap G10).
    pub fn digest(&self) -> PolicyDigest {
        PolicyDigest {
            gifs_enabled: self.gifs_enabled,
            link_previews_enabled: self.link_previews_enabled,
            messages_edit_window_s: self.messages_edit_window_s,
            messages_delete_own: self.messages_delete_own,
            files_max_bytes: self.files_max_bytes,
            emoji_max_per_space: self.emoji_max_per_space,
            agents_allowed_in_dms: self.agents_allowed_in_dms,
            guests_enabled: self.guests_enabled,
            guests_invite: self.guests_invite,
            guests_dm_non_channel_mates: self.guests_dm_non_channel_mates,
            calls_verification_code: self.calls_verification_code,
            calls_dm_enabled: self.calls_dm_enabled,
            recording_enabled: self.recording_enabled,
            recording_dm_enabled: self.recording_dm_enabled,
            recording_audio_default_days: self.recording_audio_default_days,
            recording_audio_max_days: self.recording_audio_max_days,
            recording_require_consent_to_join: self.recording_require_consent_to_join,
        }
    }

    /// Applies a validated patch.
    pub fn apply(&mut self, p: PolicyPatch) {
        macro_rules! set {
            ($($f:ident),+) => { $( if let Some(v) = p.$f { self.$f = v; } )+ };
        }
        set!(
            spaces_create,
            channels_create_public_floor,
            channels_create_private_floor,
            channels_archive,
            mentions_channel_max_members,
            guests_invite,
            guests_dm_non_channel_mates,
            messages_edit_window_s,
            messages_delete_own,
            gifs_enabled,
            link_previews_enabled,
            files_max_bytes,
            files_space_quota_bytes,
            emoji_max_per_space,
            retention_default_days,
            private_space_admins_can_join,
            presence_show_to_guests,
            digest_enabled,
            agents_allowed_in_dms,
            files_strip_image_location,
            messages_deleted_purge_days,
            guests_enabled,
            notifications_default_level,
            digest_detail,
            calls_verification_code,
            calls_dm_enabled,
            calls_stats_beacon,
            recording_enabled,
            recording_dm_enabled,
            recording_audio_default_days,
            recording_audio_max_days,
            recording_require_consent_to_join,
            meetings_enabled,
            meetings_create,
            meetings_max_participants,
            meetings_max_duration_minutes,
            meetings_grace_minutes,
            meetings_link_guests
        );
        self.gifs_provider = p.gifs_provider.apply(self.gifs_provider.take());
        self.meetings_privacy_contact = p
            .meetings_privacy_contact
            .apply(self.meetings_privacy_contact.take());
        self.gifs_api_key_ref = p.gifs_api_key_ref.apply(self.gifs_api_key_ref.take());
    }
}

/// A partial policy change. Every key is optional; unknown keys are rejected.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyPatch {
    /// `spaces.create`.
    #[serde(
        rename = "spaces.create",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub spaces_create: Option<SpacesCreate>,
    /// `channels.create_public_floor`.
    #[serde(
        rename = "channels.create_public_floor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub channels_create_public_floor: Option<ChannelCreateFloor>,
    /// `channels.create_private_floor`.
    #[serde(
        rename = "channels.create_private_floor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub channels_create_private_floor: Option<ChannelCreateFloor>,
    /// `channels.archive`.
    #[serde(
        rename = "channels.archive",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub channels_archive: Option<ArchiveWho>,
    /// `mentions.channel_max_members` (≥ 2).
    #[serde(
        rename = "mentions.channel_max_members",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub mentions_channel_max_members: Option<u32>,
    /// `guests.invite`.
    #[serde(
        rename = "guests.invite",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub guests_invite: Option<GuestInvite>,
    /// `guests.dm_non_channel_mates`.
    #[serde(
        rename = "guests.dm_non_channel_mates",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub guests_dm_non_channel_mates: Option<bool>,
    /// `messages.edit_window_s`.
    #[serde(
        rename = "messages.edit_window_s",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub messages_edit_window_s: Option<u64>,
    /// `messages.delete_own`.
    #[serde(
        rename = "messages.delete_own",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub messages_delete_own: Option<bool>,
    /// `gifs.enabled`.
    #[serde(
        rename = "gifs.enabled",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub gifs_enabled: Option<bool>,
    /// `gifs.provider` (≤ 64); `null` clears it.
    #[serde(
        rename = "gifs.provider",
        default,
        skip_serializing_if = "Patch::is_keep"
    )]
    pub gifs_provider: Patch<String>,
    /// `gifs.api_key_ref` (≤ 256); `null` clears it.
    #[serde(
        rename = "gifs.api_key_ref",
        default,
        skip_serializing_if = "Patch::is_keep"
    )]
    pub gifs_api_key_ref: Patch<String>,
    /// `link_previews.enabled`.
    #[serde(
        rename = "link_previews.enabled",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub link_previews_enabled: Option<bool>,
    /// `files.max_bytes` (≥ 1).
    #[serde(
        rename = "files.max_bytes",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub files_max_bytes: Option<u64>,
    /// `files.space_quota_bytes`.
    #[serde(
        rename = "files.space_quota_bytes",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub files_space_quota_bytes: Option<u64>,
    /// `emoji.max_per_space`.
    #[serde(
        rename = "emoji.max_per_space",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub emoji_max_per_space: Option<u32>,
    /// `retention.default_days`.
    #[serde(
        rename = "retention.default_days",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub retention_default_days: Option<u32>,
    /// `private.space_admins_can_join`.
    #[serde(
        rename = "private.space_admins_can_join",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub private_space_admins_can_join: Option<bool>,
    /// `presence.show_to_guests`.
    #[serde(
        rename = "presence.show_to_guests",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub presence_show_to_guests: Option<bool>,
    /// `digest.enabled`.
    #[serde(
        rename = "digest.enabled",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub digest_enabled: Option<bool>,
    /// `agents.allowed_in_dms`.
    #[serde(
        rename = "agents.allowed_in_dms",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub agents_allowed_in_dms: Option<bool>,
    /// `files.strip_image_location` (S15).
    #[serde(
        rename = "files.strip_image_location",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub files_strip_image_location: Option<bool>,
    /// `messages.deleted_purge_days` (S22): a read-only mirror; a write is refused with
    /// `managed_by_retention`.
    #[serde(
        rename = "messages.deleted_purge_days",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub messages_deleted_purge_days: Option<u32>,
    /// `guests.enabled` (S19).
    #[serde(
        rename = "guests.enabled",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub guests_enabled: Option<bool>,
    /// `notifications.default_level` (S21).
    #[serde(
        rename = "notifications.default_level",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub notifications_default_level: Option<NotifyLevel>,
    /// `digest.detail` (S21).
    #[serde(
        rename = "digest.detail",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub digest_detail: Option<DigestDetail>,
    /// `calls.verification_code` (S50): `shown` or `hidden`; `required` is reserved.
    #[serde(
        rename = "calls.verification_code",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub calls_verification_code: Option<VerificationCode>,
    /// `calls.dm_enabled` (S34 v2).
    #[serde(
        rename = "calls.dm_enabled",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub calls_dm_enabled: Option<bool>,
    /// `calls.stats_beacon` (F21).
    #[serde(
        rename = "calls.stats_beacon",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub calls_stats_beacon: Option<bool>,
    /// `recording.enabled` (S35 v2).
    #[serde(
        rename = "recording.enabled",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub recording_enabled: Option<bool>,
    /// `recording.dm_enabled` (S35 v2).
    #[serde(
        rename = "recording.dm_enabled",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub recording_dm_enabled: Option<bool>,
    /// `recording.audio_default_days` (S35 v2).
    #[serde(
        rename = "recording.audio_default_days",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub recording_audio_default_days: Option<u16>,
    /// `recording.audio_max_days` (S35 v2).
    #[serde(
        rename = "recording.audio_max_days",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub recording_audio_max_days: Option<u16>,
    /// `recording.require_consent_to_join` (S35 v2, rev 91).
    #[serde(
        rename = "recording.require_consent_to_join",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub recording_require_consent_to_join: Option<bool>,
    /// `meetings.enabled` (Meet MC1).
    #[serde(
        rename = "meetings.enabled",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub meetings_enabled: Option<bool>,
    /// `meetings.create` (Meet MC1).
    #[serde(
        rename = "meetings.create",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub meetings_create: Option<MeetingsCreate>,
    /// `meetings.max_participants` (2–200).
    #[serde(
        rename = "meetings.max_participants",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub meetings_max_participants: Option<u32>,
    /// `meetings.max_duration_minutes` (1–720).
    #[serde(
        rename = "meetings.max_duration_minutes",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub meetings_max_duration_minutes: Option<u32>,
    /// `meetings.grace_minutes` (0–720).
    #[serde(
        rename = "meetings.grace_minutes",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub meetings_grace_minutes: Option<u32>,
    /// `meetings.link_guests` (Meet MC2a).
    #[serde(
        rename = "meetings.link_guests",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub meetings_link_guests: Option<bool>,
    /// `meetings.privacy_contact` (≤ 300 characters); `null` clears it.
    #[serde(
        rename = "meetings.privacy_contact",
        default,
        skip_serializing_if = "Patch::is_keep"
    )]
    pub meetings_privacy_contact: Patch<String>,
}

impl Validate for PolicyPatch {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(v) = self.spaces_create {
            check_known("spaces.create", v)?;
        }
        if let Some(v) = self.channels_create_public_floor {
            check_known("channels.create_public_floor", v)?;
        }
        if let Some(v) = self.channels_create_private_floor {
            check_known("channels.create_private_floor", v)?;
        }
        if let Some(v) = self.channels_archive {
            check_known("channels.archive", v)?;
        }
        if let Some(v) = self.guests_invite {
            check_known("guests.invite", v)?;
        }
        if let Some(v) = self.notifications_default_level {
            check_known("notifications.default_level", v)?;
        }
        if let Some(v) = self.digest_detail {
            check_known("digest.detail", v)?;
        }
        if let Some(v) = self.calls_verification_code {
            check_known("calls.verification_code", v)?;
            if v == VerificationCode::Required {
                return Err(ValidationError::invalid(
                    "calls.verification_code",
                    "calls.verification_code = required is reserved; use shown or hidden",
                ));
            }
        }
        if let Some(v) = self.mentions_channel_max_members
            && v < 2
        {
            return Err(ValidationError::invalid(
                "mentions.channel_max_members",
                "mentions.channel_max_members must be at least 2",
            ));
        }
        if let Some(v) = self.recording_audio_max_days
            && v > RECORDING_AUDIO_DAYS_LIMIT
        {
            return Err(ValidationError::invalid(
                "recording.audio_max_days",
                format!("recording.audio_max_days must be at most {RECORDING_AUDIO_DAYS_LIMIT}"),
            ));
        }
        if let (Some(d), Some(m)) = (
            self.recording_audio_default_days,
            self.recording_audio_max_days,
        ) && d > m
        {
            return Err(ValidationError::invalid(
                "recording.audio_default_days",
                "recording.audio_default_days must not exceed recording.audio_max_days",
            ));
        }
        if let Some(v) = self.meetings_create {
            check_known("meetings.create", v)?;
        }
        for (field, v, lo, hi) in [
            (
                "meetings.max_participants",
                self.meetings_max_participants,
                2,
                MEETINGS_MAX_PARTICIPANTS_LIMIT,
            ),
            (
                "meetings.max_duration_minutes",
                self.meetings_max_duration_minutes,
                1,
                MEETINGS_MAX_DURATION_LIMIT,
            ),
            (
                "meetings.grace_minutes",
                self.meetings_grace_minutes,
                0,
                MEETINGS_GRACE_LIMIT,
            ),
        ] {
            if let Some(v) = v
                && !(lo..=hi).contains(&v)
            {
                return Err(ValidationError::invalid(
                    field,
                    format!("{field} must be {lo}–{hi}"),
                ));
            }
        }
        if let Some(v) = self.meetings_privacy_contact.as_set() {
            let n = v.chars().count();
            if n == 0 || n > MEETINGS_PRIVACY_CONTACT_MAX_CHARS {
                return Err(ValidationError::invalid(
                    "meetings.privacy_contact",
                    format!(
                        "meetings.privacy_contact must be 1–{MEETINGS_PRIVACY_CONTACT_MAX_CHARS} characters"
                    ),
                ));
            }
        }
        if self.files_max_bytes == Some(0) {
            return Err(ValidationError::invalid(
                "files.max_bytes",
                "files.max_bytes must be at least 1",
            ));
        }
        if let Some(v) = self.gifs_provider.as_set()
            && (v.is_empty() || v.len() > GIFS_PROVIDER_MAX_LEN)
        {
            return Err(ValidationError::invalid(
                "gifs.provider",
                format!("gifs.provider must be 1–{GIFS_PROVIDER_MAX_LEN} bytes"),
            ));
        }
        if let Some(v) = self.gifs_api_key_ref.as_set()
            && (v.is_empty() || v.len() > GIFS_API_KEY_REF_MAX_LEN)
        {
            return Err(ValidationError::invalid(
                "gifs.api_key_ref",
                format!("gifs.api_key_ref must be 1–{GIFS_API_KEY_REF_MAX_LEN} bytes"),
            ));
        }
        Ok(())
    }
}

/// Response of `GET /api/v1/install/policies` and `GET /mgmt/v1/policies`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PoliciesResponse {
    /// Every policy value.
    pub policies: Policies,
}

/// Body of `POST /api/v1/install/policies` and `POST /mgmt/v1/policies`. Returns
/// [`PoliciesResponse`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetPoliciesRequest {
    /// The keys to change.
    pub changes: PolicyPatch,
}

impl Validate for SetPoliciesRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.changes.validate()
    }
}

/// The policy values clients need at boot, carried in READY (gap G10). Field names are the
/// policy keys with `.` replaced by `_`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicyDigest {
    /// `gifs.enabled`.
    pub gifs_enabled: bool,
    /// `link_previews.enabled`.
    pub link_previews_enabled: bool,
    /// `messages.edit_window_s`.
    pub messages_edit_window_s: u64,
    /// `messages.delete_own`.
    pub messages_delete_own: bool,
    /// `files.max_bytes`.
    pub files_max_bytes: u64,
    /// `emoji.max_per_space`.
    pub emoji_max_per_space: u32,
    /// `agents.allowed_in_dms`.
    pub agents_allowed_in_dms: bool,
    /// `guests.enabled` (S19). Old payloads without it read as `true`.
    #[serde(default = "default_true")]
    pub guests_enabled: bool,
    /// `guests.invite` (S19): members' clients show "Add guest" only when they may.
    #[serde(default = "default_guest_invite")]
    pub guests_invite: GuestInvite,
    /// `guests.dm_non_channel_mates` (S19): guests' clients explain read-only DMs.
    #[serde(default)]
    pub guests_dm_non_channel_mates: bool,
    /// `calls.verification_code` (S50 §16.4): a change reaches clients in `install_updated`.
    /// Old payloads read as `shown`.
    #[serde(default = "default_verification_code")]
    pub calls_verification_code: VerificationCode,
    /// `calls.dm_enabled` (S34 v2): clients hide the DM call button when off. Old payloads
    /// read as `true`.
    #[serde(default = "default_true")]
    pub calls_dm_enabled: bool,
    /// `recording.enabled` (S35 v2): clients hide Record when off. Old payloads read `false`.
    #[serde(default)]
    pub recording_enabled: bool,
    /// `recording.dm_enabled` (S35 v2). Old payloads read `false`.
    #[serde(default)]
    pub recording_dm_enabled: bool,
    /// `recording.audio_default_days` (S35 v2).
    #[serde(default = "default_recording_audio_default_days")]
    pub recording_audio_default_days: u16,
    /// `recording.audio_max_days` (S35 v2).
    #[serde(default = "default_recording_audio_max_days")]
    pub recording_audio_max_days: u16,
    /// `recording.require_consent_to_join` (S35 v2, rev 91).
    #[serde(default)]
    pub recording_require_consent_to_join: bool,
}

fn default_guest_invite() -> GuestInvite {
    GuestInvite::Admins
}
