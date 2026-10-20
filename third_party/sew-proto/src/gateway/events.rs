//! Dispatch events (C7, gap G14).
//!
//! Names are `lower_snake`, `<entity>_<past-tense verb>`. Clients must ignore unknown `t`:
//! [`Event::decode`] returns [`Event::Unknown`] for them, never an error. S1 registers the 23
//! C7 names and gives bodies to 19; owning slices add bodies and new names additively.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::server::Ready;
use super::to_value_or_panic;
use crate::api::channels::Channel;
use crate::api::compliance::ContentVanished;
use crate::api::extras::{
    Bookmark, BookmarkDeleted, DeletedRef, Draft, DraftDeleted, PollUpdated, Reminder,
    ReminderFired, SavedItem, SavedItemDeleted, ScheduledMessage,
};
use crate::api::files::AttachmentsUpdated;
use crate::api::media::{CustomEmoji, LinkPreview};
use crate::api::members::Member;
use crate::api::messages::{Emoji, Message, ThreadSummary};
use crate::api::notify::{Notification, NotificationPrefsUpdated, NotificationsReleased};
use crate::api::people::{
    MemberListUpdated, PresenceSettings, PresenceSnapshot, ProfileFieldsUpdated, UserGroup,
    UserGroupDeleted,
};
use crate::api::read_state::{BulkAckResponse, ReadState, ThreadReadUpdated};
use crate::api::spaces::{Category, Role, Space, SpaceMember};
use crate::calls::{CallRingEvent, CallUpdated};
use crate::e2ee::CallKeysMessage;
use crate::id::Id;
use crate::perms::Perms;
use crate::policy::PolicyDigest;
use crate::voice::{MediaPolicy, VoiceServerUpdated, VoiceStateUpdated};

unit_enum! {
    /// A dispatch event name (the frame's `t`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum EventName {
        /// The boot snapshot after IDENTIFY.
        Ready = "ready",
        /// Replay after RESUME finished.
        Resumed = "resumed",
        /// A message was posted.
        MessageCreated = "message_created",
        /// A message was edited.
        MessageUpdated = "message_updated",
        /// A message was deleted.
        MessageDeleted = "message_deleted",
        /// A reaction was added.
        ReactionAdded = "reaction_added",
        /// A reaction was removed.
        ReactionRemoved = "reaction_removed",
        /// A channel was created or became visible.
        ChannelCreated = "channel_created",
        /// A channel changed.
        ChannelUpdated = "channel_updated",
        /// A member joined a channel.
        ChannelMemberAdded = "channel_member_added",
        /// A member left a channel.
        ChannelMemberRemoved = "channel_member_removed",
        /// A read state changed.
        ReadStateUpdated = "read_state_updated",
        /// Someone started typing.
        TypingStarted = "typing_started",
        /// Someone's presence changed.
        PresenceUpdated = "presence_updated",
        /// A member's profile changed.
        MemberUpdated = "member_updated",
        /// A space changed.
        SpaceUpdated = "space_updated",
        /// A role changed.
        RoleUpdated = "role_updated",
        /// The member's effective permissions changed.
        PermsChanged = "perms_changed",
        /// A thread's summary changed.
        ThreadUpdated = "thread_updated",
        /// A poll changed (S17; body `PollUpdated`).
        PollUpdated = "poll_updated",
        /// A reminder fired (S17; body `ReminderFired`).
        ReminderFired = "reminder_fired",
        /// A notification was written (S21; body `Notification`).
        NotificationCreated = "notification_created",
        /// Voice state changed (S33; body `VoiceStateUpdated`).
        VoiceStateUpdated = "voice_state_updated",
        /// A member joined a space (S10 gap G6).
        SpaceMemberAdded = "space_member_added",
        /// A member left or was removed from a space (S10 gap G6).
        SpaceMemberRemoved = "space_member_removed",
        /// A category was created or changed (S10 gap G6; body `Category`).
        CategoryUpdated = "category_updated",
        /// A category was deleted (S10 gap G6).
        CategoryDeleted = "category_deleted",
        /// A message was pinned (S13 gap G7).
        MessagePinned = "message_pinned",
        /// A message was unpinned (S13 gap G7).
        MessageUnpinned = "message_unpinned",
        /// A role was deleted (S12 gap G4; body `RoleDeleted`).
        RoleDeleted = "role_deleted",
        /// A member's roles or timeout changed (S12 gap G4; body `SpaceMember`).
        SpaceMemberUpdated = "space_member_updated",
        /// The install's policy digest changed (S12 gap G4; body `InstallUpdated`).
        InstallUpdated = "install_updated",
        /// Presence of every id a SUBSCRIBE named (S18).
        PresenceSnapshot = "presence_snapshot",
        /// The member's own presence settings changed (S18).
        PresenceSettingsUpdated = "presence_settings_updated",
        /// Member-list range operations (S18).
        MemberListUpdated = "member_list_updated",
        /// A user group was created or changed (S18).
        UserGroupUpdated = "user_group_updated",
        /// A user group was deleted (S18).
        UserGroupDeleted = "user_group_deleted",
        /// Custom field definitions changed (S18).
        ProfileFieldsUpdated = "profile_fields_updated",
        /// A message's attachments changed: a thumbnail landed or a file was removed (S15 G8).
        AttachmentsUpdated = "attachments_updated",
        /// A custom emoji was created or edited (S16; body `EmojiUpdated`).
        EmojiUpdated = "emoji_updated",
        /// A custom emoji was deleted (S16; body `EmojiDeleted`).
        EmojiDeleted = "emoji_deleted",
        /// A message's link previews changed (S16; body `MessagePreviewsUpdated`).
        MessagePreviewsUpdated = "message_previews_updated",
        /// Retention hid a batch of messages and files in a channel (S22 §14.2).
        ContentVanished = "content_vanished",
        /// The member's draft was saved (S17; body `Draft`).
        DraftUpdated = "draft_updated",
        /// The member's draft was deleted (S17; body `DraftDeleted`).
        DraftDeleted = "draft_deleted",
        /// A saved item was saved or changed (S17; body `SavedItem`).
        SavedItemUpdated = "saved_item_updated",
        /// A saved item was removed (S17; body `SavedItemDeleted`).
        SavedItemDeleted = "saved_item_deleted",
        /// A scheduled message was created, changed, sent or failed (S17; body
        /// `ScheduledMessage`).
        ScheduledMessageUpdated = "scheduled_message_updated",
        /// A scheduled message was cancelled (S17; body `DeletedRef`).
        ScheduledMessageDeleted = "scheduled_message_deleted",
        /// A reminder was created or changed (S17; body `Reminder`).
        ReminderUpdated = "reminder_updated",
        /// A reminder was deleted (S17; body `DeletedRef`).
        ReminderDeleted = "reminder_deleted",
        /// A channel bookmark was created or changed (S17; body `Bookmark`).
        BookmarkUpdated = "bookmark_updated",
        /// A channel bookmark was deleted (S17; body `BookmarkDeleted`).
        BookmarkDeleted = "bookmark_deleted",
        /// A thread read state changed, with inbox totals when present (S14 gap G9).
        ThreadReadUpdated = "thread_read_updated",
        /// A bulk ack moved read states (S14 gap G9).
        ReadStatesBulkUpdated = "read_states_bulk_updated",
        /// DND ended: the held notifications still unread (S21; body `NotificationsReleased`).
        NotificationsReleased = "notifications_released",
        /// The member's notification settings or a channel's or space's prefs changed (S21;
        /// body `NotificationPrefsUpdated`).
        NotificationPrefsUpdated = "notification_prefs_updated",
        /// Call key delivery to one session: the MLS delivery service's messages, tagged by
        /// `kind` (S50 §9.3; body `CallKeysMessage`).
        CallKeysDelivered = "call_keys_delivered",
        /// A moved member's new media server, token and key join (S33 v2 §13.6; body
        /// `VoiceServerUpdated`).
        VoiceServerUpdated = "voice_server_updated",
        /// A DM call started, changed or ended (S34 v2 §17; body `CallUpdated`).
        CallUpdated = "call_updated",
        /// A ring started or stopped for the callee's sessions (S34 v2 §17; body
        /// `CallRingEvent`).
        CallRing = "call_ring",
        /// The client media policy changed with the budget tier (S34 v2 §13; body
        /// `MediaPolicy`).
        MediaPolicyUpdated = "media_policy_updated",
        /// A recording's state, consent or capture changed (S35 v2 §18.2; body
        /// `RecordingUpdated`, with the receiver's own `my_consent`).
        RecordingUpdated = "recording_updated",
        /// A meeting was created, changed, started, ended or cancelled (Meet MC1; body
        /// `MeetingUpdated`, to the meeting channel's members).
        MeetingUpdated = "meeting_updated",
        /// A lobby entry was made or decided (Meet MC2b; body `MeetingLobbyUpdated`, to the
        /// meeting's hosts only).
        MeetingLobbyUpdated = "meeting_lobby_updated",
        /// A host decided on the receiver's knock, or it expired (Meet MC2b; body
        /// `MeetingAdmission`, to the knocking member).
        MeetingAdmission = "meeting_admission",
    }
}

/// Body of `resumed`: empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Resumed {}

/// Body of `message_deleted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageDeleted {
    /// The deleted message.
    pub id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// Its thread root, for replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// The seq of the `message_deleted` change entry (S13).
    #[serde(default)]
    pub seq: u64,
    /// The server keeps a tombstone in the timeline (a root with live replies, S13 §13.2).
    #[serde(default)]
    pub tombstone: bool,
    /// A whole-thread delete: drop the root and every reply (S13 §12.6).
    #[serde(default)]
    pub whole_thread: bool,
    /// The channel's newest live timeline message after the delete (contracts rev 29).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_last_message_id: Option<Id>,
}

/// Body of `reaction_added` and `reaction_removed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReactionEvent {
    /// The message.
    pub message_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// Who reacted.
    pub member_id: Id,
    /// The emoji.
    pub emoji: Emoji,
    /// The emoji's count on the message after the change (S13).
    #[serde(default)]
    pub count: u32,
    /// The seq of the `reaction_changed` change entry (S13).
    #[serde(default)]
    pub seq: u64,
}

/// Body of `message_pinned` (S13 gap G7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessagePinned {
    /// The channel.
    pub channel_id: Id,
    /// The pinned message.
    pub message_id: Id,
    /// Who pinned it.
    pub pinned_by: Id,
    /// When.
    pub pinned_at: crate::time::Timestamp,
    /// The seq of the `pin_changed` change entry.
    pub seq: u64,
}

/// Body of `message_unpinned` (S13 gap G7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageUnpinned {
    /// The channel.
    pub channel_id: Id,
    /// The unpinned message.
    pub message_id: Id,
    /// The seq of the `pin_changed` change entry.
    pub seq: u64,
}

/// Body of `channel_member_added` and `channel_member_removed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelMemberEvent {
    /// The channel.
    pub channel_id: Id,
    /// The member.
    pub member_id: Id,
    /// The channel change seq of the `member_joined` / `member_left` entry (S10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// The channel; set only in the copy sent to the joining member (S10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<Box<Channel>>,
    /// Meet MC3b: on a meeting channel's `channel_member_added` only, the joiner's name and
    /// kind, as `GET /api/v1/channels/{id}/people` shows them (a link guest learns other
    /// people's names this way and from that route only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<crate::api::meetings::MeetingPerson>,
}

/// Body of `space_member_added` (S10 gap G6). `space` and `categories` are filled only in the
/// copy sent to the added member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpaceMemberAdded {
    /// The space.
    pub space_id: Id,
    /// The member who joined.
    pub member_id: Id,
    /// The space (the added member's copy only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<Box<Space>>,
    /// The space's categories (the added member's copy only).
    #[serde(default)]
    pub categories: Vec<Category>,
}

/// Body of `space_member_removed` (S10 gap G6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpaceMemberRemoved {
    /// The space.
    pub space_id: Id,
    /// The member who left.
    pub member_id: Id,
}

/// Body of `category_deleted` (S10 gap G6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CategoryDeleted {
    /// The category.
    pub id: Id,
    /// Its space.
    pub space_id: Id,
}

/// Body of `typing_started`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TypingStarted {
    /// The channel.
    pub channel_id: Id,
    /// The thread, when typing a reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Id>,
    /// Who is typing.
    pub member_id: Id,
}

unit_enum! {
    /// A member's visible presence.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum PresenceStatus {
        /// Online.
        Online = "online",
        /// Idle.
        Idle = "idle",
        /// Do not disturb.
        Dnd = "dnd",
        /// Offline or invisible.
        Offline = "offline",
    }
}

/// Body of `presence_updated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceUpdated {
    /// The member.
    pub member_id: Id,
    /// Their presence.
    pub status: PresenceStatus,
}

/// One channel's new effective permissions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelPerms {
    /// The channel.
    pub channel_id: Id,
    /// The member's effective permissions there.
    pub my_perms: Perms,
}

/// Body of `perms_changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermsChanged {
    /// The space whose permissions changed; absent for DMs and when the gateway batched
    /// channel changes only. Set whenever `my_space_perms` is (S12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// New effective permissions per channel.
    pub channels: Vec<ChannelPerms>,
    /// The member's new base permissions in `space_id` (S12 §7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_space_perms: Option<Perms>,
    /// `true`: `channels` lists every joined channel of `space_id` (zeros included); replace
    /// them all. `false` (omitted on the wire): channels not listed are unchanged (S12 §11.4).
    #[serde(default, skip_serializing_if = "is_false")]
    pub full: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Body of `role_deleted` (S12 gap G4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoleDeleted {
    /// The space.
    pub space_id: Id,
    /// The deleted role.
    pub role_id: Id,
}

/// Body of `install_updated` (S12 gap G4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstallUpdated {
    /// The new policy digest.
    pub policy_digest: PolicyDigest,
}

/// Body of `emoji_updated` (S16): a custom emoji was created or edited. `version` is the
/// space's new emoji ETag (`e<n>`, READY `versions.emoji`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EmojiUpdated {
    /// The space.
    pub space_id: Id,
    /// The space's emoji version after the write.
    pub version: String,
    /// The emoji.
    pub emoji: CustomEmoji,
}

/// Body of `emoji_deleted` (S16).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EmojiDeleted {
    /// The space.
    pub space_id: Id,
    /// The space's emoji version after the delete.
    pub version: String,
    /// The deleted emoji.
    pub id: Id,
}

/// Body of `message_previews_updated` (S16): the message's previews now, identical for every
/// viewer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessagePreviewsUpdated {
    /// The message.
    pub message_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// Its previews, in body order.
    pub previews: Vec<LinkPreview>,
    /// Previews are turned off for the message.
    pub previews_suppressed: bool,
    /// The seq of the `preview_changed` change entry.
    pub seq: u64,
}

/// A dispatch event: one typed variant per name with a body, plus `Unknown`.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// `ready`.
    Ready(Box<Ready>),
    /// `resumed`.
    Resumed(Resumed),
    /// `message_created`.
    MessageCreated(Box<Message>),
    /// `message_updated`.
    MessageUpdated(Box<Message>),
    /// `message_deleted`.
    MessageDeleted(MessageDeleted),
    /// `reaction_added`.
    ReactionAdded(ReactionEvent),
    /// `reaction_removed`.
    ReactionRemoved(ReactionEvent),
    /// `channel_created`.
    ChannelCreated(Box<Channel>),
    /// `channel_updated`.
    ChannelUpdated(Box<Channel>),
    /// `channel_member_added`.
    ChannelMemberAdded(ChannelMemberEvent),
    /// `channel_member_removed`.
    ChannelMemberRemoved(ChannelMemberEvent),
    /// `read_state_updated`.
    ReadStateUpdated(ReadState),
    /// `typing_started`.
    TypingStarted(TypingStarted),
    /// `presence_updated`.
    PresenceUpdated(PresenceUpdated),
    /// `member_updated`.
    MemberUpdated(Box<Member>),
    /// `space_updated`.
    SpaceUpdated(Box<Space>),
    /// `role_updated`.
    RoleUpdated(Box<Role>),
    /// `perms_changed`.
    PermsChanged(PermsChanged),
    /// `thread_updated`.
    ThreadUpdated(ThreadSummary),
    /// `space_member_added` (S10).
    SpaceMemberAdded(Box<SpaceMemberAdded>),
    /// `space_member_removed` (S10).
    SpaceMemberRemoved(SpaceMemberRemoved),
    /// `category_updated` (S10).
    CategoryUpdated(Box<Category>),
    /// `category_deleted` (S10).
    CategoryDeleted(CategoryDeleted),
    /// `message_pinned` (S13).
    MessagePinned(MessagePinned),
    /// `message_unpinned` (S13).
    MessageUnpinned(MessageUnpinned),
    /// `role_deleted` (S12).
    RoleDeleted(RoleDeleted),
    /// `space_member_updated` (S12).
    SpaceMemberUpdated(Box<SpaceMember>),
    /// `install_updated` (S12).
    InstallUpdated(InstallUpdated),
    /// `presence_snapshot` (S18).
    PresenceSnapshot(PresenceSnapshot),
    /// `presence_settings_updated` (S18).
    PresenceSettingsUpdated(Box<PresenceSettings>),
    /// `member_list_updated` (S18).
    MemberListUpdated(Box<MemberListUpdated>),
    /// `user_group_updated` (S18).
    UserGroupUpdated(Box<UserGroup>),
    /// `user_group_deleted` (S18).
    UserGroupDeleted(UserGroupDeleted),
    /// `profile_fields_updated` (S18).
    ProfileFieldsUpdated(ProfileFieldsUpdated),
    /// `attachments_updated` (S15).
    AttachmentsUpdated(Box<AttachmentsUpdated>),
    /// `emoji_updated` (S16).
    EmojiUpdated(Box<EmojiUpdated>),
    /// `emoji_deleted` (S16).
    EmojiDeleted(EmojiDeleted),
    /// `message_previews_updated` (S16).
    MessagePreviewsUpdated(Box<MessagePreviewsUpdated>),
    /// `content_vanished` (S22).
    ContentVanished(Box<ContentVanished>),
    /// `poll_updated` (S17).
    PollUpdated(Box<PollUpdated>),
    /// `reminder_fired` (S17).
    ReminderFired(Box<ReminderFired>),
    /// `draft_updated` (S17).
    DraftUpdated(Box<Draft>),
    /// `draft_deleted` (S17).
    DraftDeleted(DraftDeleted),
    /// `saved_item_updated` (S17).
    SavedItemUpdated(Box<SavedItem>),
    /// `saved_item_deleted` (S17).
    SavedItemDeleted(SavedItemDeleted),
    /// `scheduled_message_updated` (S17).
    ScheduledMessageUpdated(Box<ScheduledMessage>),
    /// `scheduled_message_deleted` (S17).
    ScheduledMessageDeleted(DeletedRef),
    /// `reminder_updated` (S17).
    ReminderUpdated(Box<Reminder>),
    /// `reminder_deleted` (S17).
    ReminderDeleted(DeletedRef),
    /// `bookmark_updated` (S17).
    BookmarkUpdated(Box<Bookmark>),
    /// `bookmark_deleted` (S17).
    BookmarkDeleted(BookmarkDeleted),
    /// `thread_read_updated` (S14).
    ThreadReadUpdated(Box<ThreadReadUpdated>),
    /// `read_states_bulk_updated` (S14).
    ReadStatesBulkUpdated(Box<BulkAckResponse>),
    /// `notification_created` (S21).
    NotificationCreated(Box<Notification>),
    /// `notifications_released` (S21).
    NotificationsReleased(Box<NotificationsReleased>),
    /// `notification_prefs_updated` (S21).
    NotificationPrefsUpdated(Box<NotificationPrefsUpdated>),
    /// `voice_state_updated` (S33).
    VoiceStateUpdated(Box<VoiceStateUpdated>),
    /// `call_keys_delivered` (S50).
    CallKeysDelivered(Box<CallKeysMessage>),
    /// `voice_server_updated` (S33).
    VoiceServerUpdated(Box<VoiceServerUpdated>),
    /// `call_updated` (S34).
    CallUpdated(Box<CallUpdated>),
    /// `call_ring` (S34).
    CallRing(Box<CallRingEvent>),
    /// `media_policy_updated` (S34).
    MediaPolicyUpdated(Box<MediaPolicy>),
    /// `recording_updated` (S35 v2).
    RecordingUpdated(Box<crate::recording::RecordingUpdated>),
    /// `meeting_updated` (Meet MC1).
    MeetingUpdated(Box<crate::api::meetings::MeetingUpdated>),
    /// `meeting_updated` as a link guest gets it (Meet MC2b: no `meeting` key).
    MeetingGuestUpdated(Box<crate::api::meetings::MeetingGuestUpdated>),
    /// `meeting_lobby_updated` (Meet MC2b).
    MeetingLobbyUpdated(Box<crate::api::meetings::MeetingLobbyUpdated>),
    /// `meeting_admission` (Meet MC2b).
    MeetingAdmission(Box<crate::api::meetings::MeetingAdmission>),
    /// A name this build has no body for (unknown, or registered without a body yet).
    Unknown {
        /// The event name.
        t: String,
        /// The raw body.
        d: Value,
    },
}

impl Event {
    /// The event name. `Unknown` gives the name if this build knows it, else `EventName::Unknown`.
    pub fn name(&self) -> EventName {
        match self {
            Event::Ready(_) => EventName::Ready,
            Event::Resumed(_) => EventName::Resumed,
            Event::MessageCreated(_) => EventName::MessageCreated,
            Event::MessageUpdated(_) => EventName::MessageUpdated,
            Event::MessageDeleted(_) => EventName::MessageDeleted,
            Event::ReactionAdded(_) => EventName::ReactionAdded,
            Event::ReactionRemoved(_) => EventName::ReactionRemoved,
            Event::ChannelCreated(_) => EventName::ChannelCreated,
            Event::ChannelUpdated(_) => EventName::ChannelUpdated,
            Event::ChannelMemberAdded(_) => EventName::ChannelMemberAdded,
            Event::ChannelMemberRemoved(_) => EventName::ChannelMemberRemoved,
            Event::ReadStateUpdated(_) => EventName::ReadStateUpdated,
            Event::TypingStarted(_) => EventName::TypingStarted,
            Event::PresenceUpdated(_) => EventName::PresenceUpdated,
            Event::MemberUpdated(_) => EventName::MemberUpdated,
            Event::SpaceUpdated(_) => EventName::SpaceUpdated,
            Event::RoleUpdated(_) => EventName::RoleUpdated,
            Event::PermsChanged(_) => EventName::PermsChanged,
            Event::ThreadUpdated(_) => EventName::ThreadUpdated,
            Event::SpaceMemberAdded(_) => EventName::SpaceMemberAdded,
            Event::SpaceMemberRemoved(_) => EventName::SpaceMemberRemoved,
            Event::CategoryUpdated(_) => EventName::CategoryUpdated,
            Event::CategoryDeleted(_) => EventName::CategoryDeleted,
            Event::MessagePinned(_) => EventName::MessagePinned,
            Event::MessageUnpinned(_) => EventName::MessageUnpinned,
            Event::RoleDeleted(_) => EventName::RoleDeleted,
            Event::SpaceMemberUpdated(_) => EventName::SpaceMemberUpdated,
            Event::InstallUpdated(_) => EventName::InstallUpdated,
            Event::PresenceSnapshot(_) => EventName::PresenceSnapshot,
            Event::PresenceSettingsUpdated(_) => EventName::PresenceSettingsUpdated,
            Event::MemberListUpdated(_) => EventName::MemberListUpdated,
            Event::UserGroupUpdated(_) => EventName::UserGroupUpdated,
            Event::UserGroupDeleted(_) => EventName::UserGroupDeleted,
            Event::ProfileFieldsUpdated(_) => EventName::ProfileFieldsUpdated,
            Event::AttachmentsUpdated(_) => EventName::AttachmentsUpdated,
            Event::EmojiUpdated(_) => EventName::EmojiUpdated,
            Event::EmojiDeleted(_) => EventName::EmojiDeleted,
            Event::MessagePreviewsUpdated(_) => EventName::MessagePreviewsUpdated,
            Event::ContentVanished(_) => EventName::ContentVanished,
            Event::PollUpdated(_) => EventName::PollUpdated,
            Event::ReminderFired(_) => EventName::ReminderFired,
            Event::DraftUpdated(_) => EventName::DraftUpdated,
            Event::DraftDeleted(_) => EventName::DraftDeleted,
            Event::SavedItemUpdated(_) => EventName::SavedItemUpdated,
            Event::SavedItemDeleted(_) => EventName::SavedItemDeleted,
            Event::ScheduledMessageUpdated(_) => EventName::ScheduledMessageUpdated,
            Event::ScheduledMessageDeleted(_) => EventName::ScheduledMessageDeleted,
            Event::ReminderUpdated(_) => EventName::ReminderUpdated,
            Event::ReminderDeleted(_) => EventName::ReminderDeleted,
            Event::BookmarkUpdated(_) => EventName::BookmarkUpdated,
            Event::BookmarkDeleted(_) => EventName::BookmarkDeleted,
            Event::ThreadReadUpdated(_) => EventName::ThreadReadUpdated,
            Event::ReadStatesBulkUpdated(_) => EventName::ReadStatesBulkUpdated,
            Event::NotificationCreated(_) => EventName::NotificationCreated,
            Event::NotificationsReleased(_) => EventName::NotificationsReleased,
            Event::NotificationPrefsUpdated(_) => EventName::NotificationPrefsUpdated,
            Event::VoiceStateUpdated(_) => EventName::VoiceStateUpdated,
            Event::CallKeysDelivered(_) => EventName::CallKeysDelivered,
            Event::VoiceServerUpdated(_) => EventName::VoiceServerUpdated,
            Event::CallUpdated(_) => EventName::CallUpdated,
            Event::CallRing(_) => EventName::CallRing,
            Event::MediaPolicyUpdated(_) => EventName::MediaPolicyUpdated,
            Event::RecordingUpdated(_) => EventName::RecordingUpdated,
            Event::MeetingUpdated(_) | Event::MeetingGuestUpdated(_) => EventName::MeetingUpdated,
            Event::MeetingLobbyUpdated(_) => EventName::MeetingLobbyUpdated,
            Event::MeetingAdmission(_) => EventName::MeetingAdmission,
            Event::Unknown { t, .. } => EventName::from_wire(t).unwrap_or(EventName::Unknown),
        }
    }

    /// The frame's `t` and `d`.
    pub fn encode(&self) -> (String, Value) {
        let name = self.name().as_str().to_owned();
        let d = match self {
            Event::Ready(b) => to_value_or_panic("Ready", b),
            Event::Resumed(b) => to_value_or_panic("Resumed", b),
            Event::MessageCreated(b) | Event::MessageUpdated(b) => to_value_or_panic("Message", b),
            Event::MessageDeleted(b) => to_value_or_panic("MessageDeleted", b),
            Event::ReactionAdded(b) | Event::ReactionRemoved(b) => {
                to_value_or_panic("ReactionEvent", b)
            }
            Event::ChannelCreated(b) | Event::ChannelUpdated(b) => to_value_or_panic("Channel", b),
            Event::ChannelMemberAdded(b) | Event::ChannelMemberRemoved(b) => {
                to_value_or_panic("ChannelMemberEvent", b)
            }
            Event::ReadStateUpdated(b) => to_value_or_panic("ReadState", b),
            Event::TypingStarted(b) => to_value_or_panic("TypingStarted", b),
            Event::PresenceUpdated(b) => to_value_or_panic("PresenceUpdated", b),
            Event::MemberUpdated(b) => to_value_or_panic("Member", b),
            Event::SpaceUpdated(b) => to_value_or_panic("Space", b),
            Event::RoleUpdated(b) => to_value_or_panic("Role", b),
            Event::PermsChanged(b) => to_value_or_panic("PermsChanged", b),
            Event::ThreadUpdated(b) => to_value_or_panic("ThreadSummary", b),
            Event::SpaceMemberAdded(b) => to_value_or_panic("SpaceMemberAdded", b),
            Event::SpaceMemberRemoved(b) => to_value_or_panic("SpaceMemberRemoved", b),
            Event::CategoryUpdated(b) => to_value_or_panic("Category", b),
            Event::CategoryDeleted(b) => to_value_or_panic("CategoryDeleted", b),
            Event::MessagePinned(b) => to_value_or_panic("MessagePinned", b),
            Event::MessageUnpinned(b) => to_value_or_panic("MessageUnpinned", b),
            Event::RoleDeleted(b) => to_value_or_panic("RoleDeleted", b),
            Event::SpaceMemberUpdated(b) => to_value_or_panic("SpaceMember", b),
            Event::InstallUpdated(b) => to_value_or_panic("InstallUpdated", b),
            Event::PresenceSnapshot(b) => to_value_or_panic("PresenceSnapshot", b),
            Event::PresenceSettingsUpdated(b) => to_value_or_panic("PresenceSettings", b),
            Event::MemberListUpdated(b) => to_value_or_panic("MemberListUpdated", b),
            Event::UserGroupUpdated(b) => to_value_or_panic("UserGroup", b),
            Event::UserGroupDeleted(b) => to_value_or_panic("UserGroupDeleted", b),
            Event::ProfileFieldsUpdated(b) => to_value_or_panic("ProfileFieldsUpdated", b),
            Event::AttachmentsUpdated(b) => to_value_or_panic("AttachmentsUpdated", b),
            Event::EmojiUpdated(b) => to_value_or_panic("EmojiUpdated", b),
            Event::EmojiDeleted(b) => to_value_or_panic("EmojiDeleted", b),
            Event::MessagePreviewsUpdated(b) => to_value_or_panic("MessagePreviewsUpdated", b),
            Event::ContentVanished(b) => to_value_or_panic("ContentVanished", b),
            Event::PollUpdated(b) => to_value_or_panic("PollUpdated", b),
            Event::ReminderFired(b) => to_value_or_panic("ReminderFired", b),
            Event::DraftUpdated(b) => to_value_or_panic("Draft", b),
            Event::DraftDeleted(b) => to_value_or_panic("DraftDeleted", b),
            Event::SavedItemUpdated(b) => to_value_or_panic("SavedItem", b),
            Event::SavedItemDeleted(b) => to_value_or_panic("SavedItemDeleted", b),
            Event::ScheduledMessageUpdated(b) => to_value_or_panic("ScheduledMessage", b),
            Event::ScheduledMessageDeleted(b) | Event::ReminderDeleted(b) => {
                to_value_or_panic("DeletedRef", b)
            }
            Event::ReminderUpdated(b) => to_value_or_panic("Reminder", b),
            Event::BookmarkUpdated(b) => to_value_or_panic("Bookmark", b),
            Event::BookmarkDeleted(b) => to_value_or_panic("BookmarkDeleted", b),
            Event::ThreadReadUpdated(b) => to_value_or_panic("ThreadReadUpdated", b),
            Event::ReadStatesBulkUpdated(b) => to_value_or_panic("BulkAckResponse", b),
            Event::NotificationCreated(b) => to_value_or_panic("Notification", b),
            Event::NotificationsReleased(b) => to_value_or_panic("NotificationsReleased", b),
            Event::NotificationPrefsUpdated(b) => to_value_or_panic("NotificationPrefsUpdated", b),
            Event::VoiceStateUpdated(b) => to_value_or_panic("VoiceStateUpdated", b),
            Event::CallKeysDelivered(b) => to_value_or_panic("CallKeysMessage", b),
            Event::VoiceServerUpdated(b) => to_value_or_panic("VoiceServerUpdated", b),
            Event::CallUpdated(b) => to_value_or_panic("CallUpdated", b),
            Event::CallRing(b) => to_value_or_panic("CallRingEvent", b),
            Event::MediaPolicyUpdated(b) => to_value_or_panic("MediaPolicy", b),
            Event::RecordingUpdated(b) => to_value_or_panic("RecordingUpdated", b),
            Event::MeetingUpdated(b) => to_value_or_panic("MeetingUpdated", b),
            Event::MeetingGuestUpdated(b) => to_value_or_panic("MeetingGuestUpdated", b),
            Event::MeetingLobbyUpdated(b) => to_value_or_panic("MeetingLobbyUpdated", b),
            Event::MeetingAdmission(b) => to_value_or_panic("MeetingAdmission", b),
            Event::Unknown { t, d } => return (t.clone(), d.clone()),
        };
        (name, d)
    }

    /// Decodes `t` and `d`. An unknown `t`, or a name without a body in this build, is
    /// `Ok(Event::Unknown)`. A known name with a bad body is an error.
    pub fn decode(t: &str, d: Value) -> Result<Event, serde_json::Error> {
        let Some(name) = EventName::from_wire(t) else {
            return Ok(Event::Unknown { t: t.to_owned(), d });
        };
        Ok(match name {
            EventName::Ready => Event::Ready(Box::new(serde_json::from_value(d)?)),
            EventName::Resumed => Event::Resumed(serde_json::from_value(d)?),
            EventName::MessageCreated => {
                Event::MessageCreated(Box::new(serde_json::from_value(d)?))
            }
            EventName::MessageUpdated => {
                Event::MessageUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::MessageDeleted => Event::MessageDeleted(serde_json::from_value(d)?),
            EventName::ReactionAdded => Event::ReactionAdded(serde_json::from_value(d)?),
            EventName::ReactionRemoved => Event::ReactionRemoved(serde_json::from_value(d)?),
            EventName::ChannelCreated => {
                Event::ChannelCreated(Box::new(serde_json::from_value(d)?))
            }
            EventName::ChannelUpdated => {
                Event::ChannelUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::ChannelMemberAdded => Event::ChannelMemberAdded(serde_json::from_value(d)?),
            EventName::ChannelMemberRemoved => {
                Event::ChannelMemberRemoved(serde_json::from_value(d)?)
            }
            EventName::ReadStateUpdated => Event::ReadStateUpdated(serde_json::from_value(d)?),
            EventName::TypingStarted => Event::TypingStarted(serde_json::from_value(d)?),
            EventName::PresenceUpdated => Event::PresenceUpdated(serde_json::from_value(d)?),
            EventName::MemberUpdated => Event::MemberUpdated(Box::new(serde_json::from_value(d)?)),
            EventName::SpaceUpdated => Event::SpaceUpdated(Box::new(serde_json::from_value(d)?)),
            EventName::RoleUpdated => Event::RoleUpdated(Box::new(serde_json::from_value(d)?)),
            EventName::PermsChanged => Event::PermsChanged(serde_json::from_value(d)?),
            EventName::ThreadUpdated => Event::ThreadUpdated(serde_json::from_value(d)?),
            EventName::SpaceMemberAdded => {
                Event::SpaceMemberAdded(Box::new(serde_json::from_value(d)?))
            }
            EventName::SpaceMemberRemoved => Event::SpaceMemberRemoved(serde_json::from_value(d)?),
            EventName::CategoryUpdated => {
                Event::CategoryUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::CategoryDeleted => Event::CategoryDeleted(serde_json::from_value(d)?),
            EventName::MessagePinned => Event::MessagePinned(serde_json::from_value(d)?),
            EventName::MessageUnpinned => Event::MessageUnpinned(serde_json::from_value(d)?),
            EventName::RoleDeleted => Event::RoleDeleted(serde_json::from_value(d)?),
            EventName::SpaceMemberUpdated => {
                Event::SpaceMemberUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::InstallUpdated => Event::InstallUpdated(serde_json::from_value(d)?),
            EventName::PresenceSnapshot => Event::PresenceSnapshot(serde_json::from_value(d)?),
            EventName::PresenceSettingsUpdated => {
                Event::PresenceSettingsUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::MemberListUpdated => {
                Event::MemberListUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::UserGroupUpdated => {
                Event::UserGroupUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::UserGroupDeleted => Event::UserGroupDeleted(serde_json::from_value(d)?),
            EventName::ProfileFieldsUpdated => {
                Event::ProfileFieldsUpdated(serde_json::from_value(d)?)
            }
            EventName::AttachmentsUpdated => {
                Event::AttachmentsUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::EmojiUpdated => Event::EmojiUpdated(Box::new(serde_json::from_value(d)?)),
            EventName::EmojiDeleted => Event::EmojiDeleted(serde_json::from_value(d)?),
            EventName::MessagePreviewsUpdated => {
                Event::MessagePreviewsUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::ContentVanished => {
                Event::ContentVanished(Box::new(serde_json::from_value(d)?))
            }
            EventName::PollUpdated => Event::PollUpdated(Box::new(serde_json::from_value(d)?)),
            EventName::ReminderFired => Event::ReminderFired(Box::new(serde_json::from_value(d)?)),
            EventName::DraftUpdated => Event::DraftUpdated(Box::new(serde_json::from_value(d)?)),
            EventName::DraftDeleted => Event::DraftDeleted(serde_json::from_value(d)?),
            EventName::SavedItemUpdated => {
                Event::SavedItemUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::SavedItemDeleted => Event::SavedItemDeleted(serde_json::from_value(d)?),
            EventName::ScheduledMessageUpdated => {
                Event::ScheduledMessageUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::ScheduledMessageDeleted => {
                Event::ScheduledMessageDeleted(serde_json::from_value(d)?)
            }
            EventName::ReminderUpdated => {
                Event::ReminderUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::ReminderDeleted => Event::ReminderDeleted(serde_json::from_value(d)?),
            EventName::BookmarkUpdated => {
                Event::BookmarkUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::BookmarkDeleted => Event::BookmarkDeleted(serde_json::from_value(d)?),
            EventName::ThreadReadUpdated => {
                Event::ThreadReadUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::ReadStatesBulkUpdated => {
                Event::ReadStatesBulkUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::NotificationCreated => {
                Event::NotificationCreated(Box::new(serde_json::from_value(d)?))
            }
            EventName::NotificationsReleased => {
                Event::NotificationsReleased(Box::new(serde_json::from_value(d)?))
            }
            EventName::NotificationPrefsUpdated => {
                Event::NotificationPrefsUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::VoiceStateUpdated => {
                Event::VoiceStateUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::CallKeysDelivered => {
                Event::CallKeysDelivered(Box::new(serde_json::from_value(d)?))
            }
            EventName::VoiceServerUpdated => {
                Event::VoiceServerUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::CallUpdated => Event::CallUpdated(Box::new(serde_json::from_value(d)?)),
            EventName::CallRing => Event::CallRing(Box::new(serde_json::from_value(d)?)),
            EventName::RecordingUpdated => {
                Event::RecordingUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::MediaPolicyUpdated => {
                Event::MediaPolicyUpdated(Box::new(serde_json::from_value(d)?))
            }
            // A member's body has `meeting`; a link guest's (MC2b) has only the guest fields.
            EventName::MeetingUpdated if d.get("meeting").is_none() => {
                Event::MeetingGuestUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::MeetingUpdated => {
                Event::MeetingUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::MeetingLobbyUpdated => {
                Event::MeetingLobbyUpdated(Box::new(serde_json::from_value(d)?))
            }
            EventName::MeetingAdmission => {
                Event::MeetingAdmission(Box::new(serde_json::from_value(d)?))
            }
            EventName::Unknown => Event::Unknown { t: t.to_owned(), d },
        })
    }
}
