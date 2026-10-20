//! DM and group-DM calls (prd-sew-34 v2 §14–§20, §24): the call object, rings, the call-record
//! summary on the DM's message, the call routes' bodies and the call events.
//!
//! A call never carries a room name, a join id, an identity, a key-group id or an epoch on the
//! wire (§14.1). `voice.call_id` of a join answer is the `calls.id` (this module's [`Call::id`]);
//! `voice.e2ee.call_id` is S50's room name (S34-m6).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::e2ee::E2eeJoin;
use crate::error::{Validate, ValidationError};
use crate::id::Id;
use crate::time::Timestamp;
use crate::voice::VoiceJoinResponse;

/// Most members a re-ring may name (a group DM holds at most 9 people).
pub const RING_TARGETS_MAX: usize = 16;
/// Most active calls in READY `calls` (§20.2).
pub const READY_CALLS_MAX: usize = 100;

unit_enum! {
    /// Whether a call is live (§14.1).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum CallState {
        /// Live.
        Active = "active",
        /// Over (`ended_at` and `end_reason` set).
        Ended = "ended",
    }
}

unit_enum! {
    /// A ring's state as every DM member sees it (§14.1, §15).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum RingState {
        /// Ringing.
        Ringing = "ringing",
        /// Ringing, but the callee is already in voice (a silent ring).
        Busy = "busy",
        /// The callee joined.
        Accepted = "accepted",
        /// The callee declined.
        Declined = "declined",
        /// Not answered in time, or the call ended first.
        Missed = "missed",
    }
}

coded_enum! {
    /// Why a call ended (§14.5). Stored in `calls.end_reason`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum CallEndReason {
        /// Nobody left in the call.
        Empty = "empty" => 1,
        /// A 1:1 callee declined.
        Declined = "declined" => 2,
        /// Nobody answered.
        NoAnswer = "no_answer" => 3,
        /// One person alone too long.
        AloneTimeout = "alone_timeout" => 4,
        /// The media engine lost the call.
        MediaRestarted = "media_restarted" => 5,
        /// The media engine failed before anyone connected.
        MediaFailed = "media_failed" => 6,
        /// Voice was turned off.
        VoiceDisabled = "voice_disabled" => 7,
        /// Nobody left who may connect.
        PermsLost = "perms_lost" => 8,
    }
}

/// One ring of a call (§14.1). Only rung members appear; the starter never does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallRing {
    /// The callee.
    pub member_id: Id,
    /// Its state.
    pub state: RingState,
    /// When an unanswered ring becomes `missed` (only while `ringing` or `busy`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
}

/// The member's own ring in READY (`Call.my_ring`, the member's copy only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MyRing {
    /// A silent ring (busy, DND or a muted DM).
    pub silent: bool,
    /// `busy`, `dnd` or `muted` for a silent ring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A call (§14.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Call {
    /// The call (`calls.id`).
    pub id: Id,
    /// The DM or group DM.
    pub channel_id: Id,
    /// The call-record message in the DM.
    pub message_id: Id,
    /// Who started it.
    pub started_by: Id,
    /// When.
    pub started_at: Timestamp,
    /// `active` or `ended`.
    pub state: CallState,
    /// When it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    /// Why it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<CallEndReason>,
    /// Members with a voice entry in the call now (connecting or connected), ascending.
    pub participant_ids: Vec<Id>,
    /// Rings, by member id.
    pub rings: Vec<CallRing>,
    /// The viewer's own ring (READY, the member's copy only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_ring: Option<MyRing>,
}

/// The call summary on the call-record message (`Message.call`, §18.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallSummary {
    /// The call.
    pub call_id: Id,
    /// Who started it.
    pub started_by: Id,
    /// When.
    pub started_at: Timestamp,
    /// When it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    /// `ended_at − started_at` once ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Why it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<CallEndReason>,
    /// Everyone who ever connected, the starter included, ascending.
    pub joined_ids: Vec<Id>,
    /// Everyone ever rung, ascending.
    pub rung_ids: Vec<Id>,
    /// Rings resolved `declined`.
    pub declined_ids: Vec<Id>,
    /// Rings resolved `missed`.
    pub missed_ids: Vec<Id>,
}

/// `POST /api/v1/channels/{id}/call/join` body (§14.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallJoinRequest {
    /// Ring the other members when this join starts a call (default `true`).
    #[serde(default = "default_true")]
    pub ring: bool,
    /// Join muted.
    #[serde(default)]
    pub self_mute: bool,
    /// Join deafened (implies muted).
    #[serde(default)]
    pub self_deaf: bool,
    /// Join with a camera lease.
    #[serde(default)]
    pub self_video: bool,
    /// The key join (S50 K1). `None` answers `422 e2ee_unsupported_client`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e2ee: Option<E2eeJoin>,
    /// The answer to the recording consent modal (S35 v2 §9.4, §9.10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_notice: Option<crate::recording::RecordingNotice>,
}

fn default_true() -> bool {
    true
}

impl Default for CallJoinRequest {
    fn default() -> Self {
        CallJoinRequest {
            ring: true,
            self_mute: false,
            self_deaf: false,
            self_video: false,
            e2ee: None,
            recording_notice: None,
        }
    }
}

impl Validate for CallJoinRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(n) = &self.recording_notice {
            n.validate().map_err(|e| e.nested("recording_notice"))?;
        }
        match &self.e2ee {
            Some(e) => e.validate().map_err(|e| e.nested("e2ee")),
            None => Ok(()),
        }
    }
}

/// The call join answer (§14.3). `voice.call_id` is [`Call::id`]; `voice.e2ee.call_id` is the
/// engine room (S50's key-group scope).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallJoinResponse {
    /// The call after the join.
    pub call: Call,
    /// Whether this join started the call.
    pub created: bool,
    /// The media half (S33's join answer with `media` and `call_id`).
    pub voice: VoiceJoinResponse,
    /// The call's active recording as the joiner sees it (S35 v2 §9.4; also in
    /// `voice.recording`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<crate::recording::Recording>,
}

/// `POST /api/v1/channels/{id}/call/decline` body (§14.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallDeclineRequest {
    /// The active call.
    pub call_id: Id,
}

impl Validate for CallDeclineRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// `POST /api/v1/channels/{id}/call/ring` body (§15.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallRingRequest {
    /// The active call.
    pub call_id: Id,
    /// Whom to ring (default: every ringable member not in the call and not ringing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_ids: Option<Vec<Id>>,
}

impl Validate for CallRingRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(ids) = &self.member_ids {
            if ids.is_empty() {
                return Err(ValidationError::invalid(
                    "member_ids",
                    "name at least one member, or leave member_ids out",
                ));
            }
            if ids.len() > RING_TARGETS_MAX {
                return Err(ValidationError::invalid(
                    "member_ids",
                    format!("at most {RING_TARGETS_MAX} members"),
                ));
            }
        }
        Ok(())
    }
}

/// `GET /api/v1/channels/{id}/call`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelCall {
    /// The DM's active call, if any.
    pub call: Option<Call>,
}

/// `GET /api/v1/me/calls`: active calls in the caller's DMs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MyCalls {
    /// The calls.
    pub items: Vec<Call>,
}

/// Body of `call_updated` (§17), to the DM's members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallUpdated {
    /// The call now.
    pub call: Call,
}

/// Body of `call_ring` (§17), to every session of the callee.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallRingEvent {
    /// The call.
    pub call_id: Id,
    /// Its DM (S51 prepares a KeyPackage scoped `call:<channel_id>` on a ring).
    pub channel_id: Id,
    /// Who started it.
    pub started_by: Id,
    /// `true`: ring; `false`: stop ringing.
    pub ringing: bool,
    /// A silent ring (busy, DND, muted).
    pub silent: bool,
    /// Why: `busy`, `dnd`, `muted` (a silent ring); `accepted`, `declined`, `timeout`,
    /// `call_ended` (a stop).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When the ring becomes missed (while ringing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
}

/// `call_ring.reason` and `MyRing.reason` values.
pub mod ring_reason {
    /// The callee is already in voice.
    pub const BUSY: &str = "busy";
    /// The callee is in Do Not Disturb.
    pub const DND: &str = "dnd";
    /// The callee muted the DM.
    pub const MUTED: &str = "muted";
    /// The callee joined.
    pub const ACCEPTED: &str = "accepted";
    /// The callee declined.
    pub const DECLINED: &str = "declined";
    /// Nobody answered in time.
    pub const TIMEOUT: &str = "timeout";
    /// The call ended first.
    pub const CALL_ENDED: &str = "call_ended";
}

/// Every call reason string under the existing error codes (§7.2, §14, §15, §18.3, §24).
pub mod reasons {
    /// `422`: a call needs a DM or group DM.
    pub const NOT_DM: &str = "not_dm";
    /// `403`: policy `calls.dm_enabled` is off.
    pub const DM_CALLS_DISABLED: &str = "dm_calls_disabled";
    /// `422`: nobody in the DM can be rung.
    pub const NO_CALLABLE_MEMBERS: &str = "no_callable_members";
    /// `422`: the call is not the DM's active call.
    pub const CALL_NOT_ACTIVE: &str = "call_not_active";
    /// `422`: the caller has no pending ring.
    pub const NOT_RINGING: &str = "not_ringing";
    /// `422`: a re-ring needs the caller in the call.
    pub const NOT_IN_CALL: &str = "not_in_call";
    /// `422`: the member cannot be rung (`details.member_id`).
    pub const NOT_RINGABLE: &str = "not_ringable";
    /// `429`: a member was rung too often in this call.
    pub const RING_LIMIT: &str = "ring_limit";
    /// `422`: the call-record message cannot be edited.
    pub const CALL_RECORD_IMMUTABLE: &str = "call_record_immutable";
    /// `422`: moderation aimed at a DM call.
    pub const NOT_SPACE_CHANNEL: &str = "not_space_channel";
}
