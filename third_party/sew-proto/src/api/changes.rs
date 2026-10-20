//! Change-log catch-up pages (C8; S14; gaps G16, G33).
//!
//! Entries hold ids and kinds only, never content. Clients refetch by id.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::channels::ChannelMember;
use crate::api::members::Member;
use crate::api::messages::{Message, MessageVersion};
use crate::changes::{ChannelChangeKind, MemberChangeKind, NotificationReason};
use crate::http::Page;
use crate::id::Id;
use crate::time::Timestamp;

/// One channel change-log entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelChange {
    /// The channel seq.
    pub seq: u64,
    /// What changed.
    pub kind: ChannelChangeKind,
    /// The entity that changed (message, member, …).
    pub entity_id: Id,
    /// Who changed it; absent for system jobs (gap G33).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<Id>,
    /// When.
    pub at: Timestamp,
}

/// One member change-log entry. Kind `notification` carries `reason` and no `entity_id`;
/// every other kind carries `entity_id` and no `reason` (gap G16).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemberChange {
    /// The member seq.
    pub seq: u64,
    /// What changed.
    pub kind: MemberChangeKind,
    /// The entity that changed; absent for `notification`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<Id>,
    /// Why the member was notified; only for `notification`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<NotificationReason>,
    /// A related id (for `notification`, the message).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<Id>,
    /// When.
    pub at: Timestamp,
}

impl MemberChange {
    /// Whether the entry follows the entity-or-reason rule.
    pub fn is_well_formed(&self) -> bool {
        match self.kind {
            MemberChangeKind::Notification => self.reason.is_some() && self.entity_id.is_none(),
            _ => self.entity_id.is_some() && self.reason.is_none(),
        }
    }
}

/// Response of `GET /api/v1/channels/{id}/changes`. Continue with `since = next_since`, never
/// with the last entry's seq: filtered entries are not in the page (S14 gap G13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelChangesPage {
    /// Entries with `seq > since` the caller may see, oldest first.
    pub entries: Vec<ChannelChange>,
    /// The channel's head seq.
    pub head: u64,
    /// The channel's floor seq (S14).
    #[serde(default)]
    pub floor: u64,
    /// The seq of the last entry scanned, kept or dropped (`since` if none) (S14).
    #[serde(default)]
    pub next_since: u64,
    /// Whether more entries follow.
    pub has_more: bool,
    /// The install's sync epoch; compare before applying (S14, contracts rev 29).
    #[serde(default)]
    pub sync_epoch: String,
}

/// Response of `GET /api/v1/me/changes` (same paging rules as [`ChannelChangesPage`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemberChangesPage {
    /// Entries with `seq > since` the caller may see, oldest first.
    pub entries: Vec<MemberChange>,
    /// The member's head seq.
    pub head: u64,
    /// The member log's floor seq (S14).
    #[serde(default)]
    pub floor: u64,
    /// The seq of the last entry scanned, kept or dropped (`since` if none) (S14).
    #[serde(default)]
    pub next_since: u64,
    /// Whether more entries follow.
    pub has_more: bool,
    /// The install's sync epoch (S14).
    #[serde(default)]
    pub sync_epoch: String,
}

/// A page of messages.
pub type MessagePage = Page<Message>;
/// A page of channel members.
pub type ChannelMemberPage = Page<ChannelMember>;
/// A page of members.
pub type MemberPage = Page<Member>;
/// A page of a message's edit history.
pub type MessageVersionPage = Page<MessageVersion>;
