//! Server-to-client gateway payloads and READY (C7; gaps G9–G11, G20).

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::events::Event;
use super::{Direction, Frame, Opcode, to_value_or_panic};
use crate::addr::Slug;
use crate::api::channels::{ChannelType, Visibility};
use crate::api::members::Member;
use crate::api::read_state::ReadState;
use crate::id::Id;
use crate::limits::{
    HEARTBEAT_INTERVAL_MS, IDENTIFY_TIMEOUT_MS, MEMBER_LIST_RANGE_LEN_MAX, MEMBER_LIST_RANGES_MAX,
    SUBSCRIBE_PRESENCE_MAX, SUBSCRIBE_TYPING_MAX, TYPING_INTERVAL_MS,
};
use crate::perms::Perms;
use crate::policy::PolicyDigest;

/// Per-session limits sent in HELLO (gap G11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionLimits {
    /// Typing channels per subscription.
    pub max_typing_channels: u32,
    /// Presence ids per subscription.
    pub max_presence_ids: u32,
    /// Member-list ranges per subscription.
    pub max_member_list_ranges: u32,
    /// Rows per member-list range.
    pub max_member_list_range_len: u32,
    /// At most one TYPING per channel per this interval.
    pub typing_interval_ms: u64,
    /// IDENTIFY or RESUME must arrive within this long.
    pub identify_timeout_ms: u64,
}

impl Default for SessionLimits {
    fn default() -> Self {
        SessionLimits {
            max_typing_channels: SUBSCRIBE_TYPING_MAX as u32,
            max_presence_ids: SUBSCRIBE_PRESENCE_MAX as u32,
            max_member_list_ranges: MEMBER_LIST_RANGES_MAX as u32,
            max_member_list_range_len: MEMBER_LIST_RANGE_LEN_MAX,
            typing_interval_ms: TYPING_INTERVAL_MS,
            identify_timeout_ms: IDENTIFY_TIMEOUT_MS,
        }
    }
}

/// Op 8 HELLO: the first frame on a socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hello {
    /// Heartbeat interval.
    pub heartbeat_interval_ms: u64,
    /// Per-session limits.
    pub session_limits: SessionLimits,
}

impl Default for Hello {
    fn default() -> Self {
        Hello {
            heartbeat_interval_ms: HEARTBEAT_INTERVAL_MS,
            session_limits: SessionLimits::default(),
        }
    }
}

/// Op 6 RECONNECT: reconnect and resume after a jittered delay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Reconnect {
    /// Wait this long before reconnecting.
    pub after_ms: u64,
}

/// Op 7 INVALID_SESSION.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InvalidSession {
    /// Whether RESUME may still work (never `true` in v1).
    pub resumable: bool,
}

/// The JSON text of a 4008 close reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RateLimitedCloseReason {
    /// Wait this long before identifying again.
    pub retry_after_ms: u64,
}

/// The install section of READY.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadyInstall {
    /// The install id (gap G20).
    pub id: Id,
    /// The install name.
    pub name: String,
    /// Policy values clients need.
    pub policy_digest: PolicyDigest,
    /// The install's sync epoch: an opaque decimal string that changes only on a restore. A
    /// value different from the client's stored one means "drop every cache and cursor"
    /// (contracts rev 29). Empty from servers that predate it.
    #[serde(default)]
    pub sync_epoch: String,
    /// The custom field definitions etag (`fields_rev`, S18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_fields_version: Option<String>,
    /// `[voice] enabled` (S33 v1 §16.3). Absent means `false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub voice_enabled: bool,
    /// `[voice.video] enabled` and voice enabled (S34 v2 §20.2). Absent means `false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub video_enabled: bool,
    /// The client media policy for the current tier (S34 v2 §13); absent when voice is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<crate::voice::MediaPolicy>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Etags for lazy per-space fetches with `If-None-Match` (gap G9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadyVersions {
    /// Custom emoji version.
    pub emoji: String,
    /// Role definitions version.
    pub roles: String,
    /// User groups version.
    pub groups: String,
}

/// A space in READY.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadySpace {
    /// The space id.
    pub id: Id,
    /// The slug.
    pub slug: Slug,
    /// The name.
    pub name: String,
    /// Signed icon URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    /// Signed 64 px icon URL (S16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_thumb_url: Option<String>,
    /// The member's roles in this space.
    pub my_role_ids: Vec<Id>,
    /// Number of members.
    pub member_count: u32,
    /// Etags for lazy fetches.
    pub versions: ReadyVersions,
    /// The member's base permissions in the space (`space_perms`, S12 §7). Old payloads read
    /// as 0.
    #[serde(default)]
    pub my_space_perms: Perms,
    /// Meet MC1 (CC4): a hidden Space; present only for clients that declared `hidden_spaces`
    /// (others never get the Space). Absent when `false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
}

/// A category in READY.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadyCategory {
    /// The category id.
    pub id: Id,
    /// Its space.
    pub space_id: Id,
    /// The name.
    pub name: String,
    /// Sort position.
    pub position: u32,
}

/// A channel in READY: every channel the member belongs to, plus DMs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadyChannel {
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
    /// Public or private.
    pub visibility: Visibility,
    /// The slug; absent for DMs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<Slug>,
    /// The name; absent for DMs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Sort position.
    pub position: u32,
    /// Head of the channel change log.
    pub head_seq: u64,
    /// The newest message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_id: Option<Id>,
    /// The member's effective permissions.
    pub my_perms: Perms,
    /// The DM's members; empty for space channels.
    pub dm_member_ids: Vec<Id>,
    /// Slow mode in seconds; present only when on (S12 §7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_mode_s: Option<u32>,
    /// Effective message retention in days (S22 §14.3); absent = forever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u32>,
    /// Effective file retention in days (S22 §14.3); absent = forever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_retention_days: Option<u32>,
    /// At least one guest is a member (S19 §15). Old payloads read as `false`.
    #[serde(default)]
    pub has_guests: bool,
    /// Assistants that read this DM through their pinned person (contracts rev 11 G21);
    /// absent for space channels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assistant_reader_ids: Vec<Id>,
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
}

/// The threads inbox summary (READY, `GET /api/v1/me/threads/summary`, S14 events).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadsInbox {
    /// Followed threads with unread replies.
    pub unread_count: u32,
    /// Unread thread mentions over every thread in the sync set, followed or not (S14).
    #[serde(default)]
    pub mention_count: u32,
    /// The member change-log head these totals are current as of (S14).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

/// The `ready` dispatch: a small boot snapshot (C7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Ready {
    /// The gateway session id, for RESUME.
    pub session_id: String,
    /// Advisory resume URL (clients resume at their ticket's URL, contracts rev 22).
    pub resume_url: String,
    /// The protocol version (1).
    pub protocol: u32,
    /// The member, with install role.
    pub member: Member,
    /// The install.
    pub install: ReadyInstall,
    /// The member's spaces.
    pub spaces: Vec<ReadySpace>,
    /// Categories in those spaces.
    pub categories: Vec<ReadyCategory>,
    /// Channels the member belongs to, plus DMs.
    pub channels: Vec<ReadyChannel>,
    /// Read states for those channels.
    pub read_states: Vec<ReadState>,
    /// Head of the member's change log.
    pub member_seq: u64,
    /// Threads inbox summary.
    pub threads_inbox: ThreadsInbox,
    /// The member's presence settings (S18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_settings: Option<crate::api::people::PresenceSettings>,
    /// Partners of the most recent DMs, at most 50 (S18).
    #[serde(default)]
    pub dm_members: Vec<Member>,
    /// Notification settings and explicit prefs (S21 gap G17).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<crate::api::notify::ReadyNotify>,
    /// Call key trust anchors and client parameters (S50 §9.4); absent when voice is disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_keys: Option<crate::e2ee::ReadyCallKeys>,
    /// Voice states in voice channels the member can view, at most 5,000 (S33 v1 §16.3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub voice_states: Vec<crate::voice::VoiceState>,
    /// `voice_states` was capped: load `GET /spaces/{id}/voice/states` per space.
    #[serde(default, skip_serializing_if = "is_false")]
    pub voice_states_partial: bool,
    /// Public voice channels the member can view but has not joined (S33 v1 §16.3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub voice_channels: Vec<crate::voice::ReadyVoiceChannel>,
    /// Active calls in the member's DMs, at most 100; the member's own copy of each carries
    /// `my_ring` (S34 v2 §20.2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<crate::calls::Call>,
    /// Active recordings in rooms whose voice state the member may see, at most 100, each
    /// with the member's own `my_consent` (S35 v2 §18.3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recordings: Vec<crate::recording::Recording>,
}

/// A decoded server-to-client frame.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerOp {
    /// Op 0: an event with its session seq.
    Dispatch {
        /// The session seq.
        s: u64,
        /// The event.
        event: Event,
    },
    /// Op 6.
    Reconnect(Reconnect),
    /// Op 7.
    InvalidSession(InvalidSession),
    /// Op 8.
    Hello(Hello),
    /// Op 9.
    HeartbeatAck,
}

/// Why a server frame could not be decoded by a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayDecodeError {
    /// The op number is not a known opcode.
    UnknownOpcode(u8),
    /// A client-only opcode arrived from the server.
    WrongDirection(Opcode),
    /// A dispatch lacks `s` or `t`.
    MissingSeqOrName,
    /// The payload does not match its type.
    BadPayload {
        /// The opcode.
        op: Opcode,
        /// The serde error.
        error: String,
    },
}

impl fmt::Display for GatewayDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GatewayDecodeError::UnknownOpcode(op) => write!(f, "unknown opcode {op}"),
            GatewayDecodeError::WrongDirection(op) => {
                write!(f, "opcode {} is client-to-server", op.name())
            }
            GatewayDecodeError::MissingSeqOrName => f.write_str("dispatch without s or t"),
            GatewayDecodeError::BadPayload { op, error } => {
                write!(f, "bad {} payload: {error}", op.name())
            }
        }
    }
}

impl std::error::Error for GatewayDecodeError {}

fn payload<T: for<'de> Deserialize<'de>>(op: Opcode, d: &Value) -> Result<T, GatewayDecodeError> {
    serde_json::from_value(d.clone()).map_err(|e| GatewayDecodeError::BadPayload {
        op,
        error: e.to_string(),
    })
}

impl ServerOp {
    /// The opcode.
    pub fn opcode(&self) -> Opcode {
        match self {
            ServerOp::Dispatch { .. } => Opcode::Dispatch,
            ServerOp::Reconnect(_) => Opcode::Reconnect,
            ServerOp::InvalidSession(_) => Opcode::InvalidSession,
            ServerOp::Hello(_) => Opcode::Hello,
            ServerOp::HeartbeatAck => Opcode::HeartbeatAck,
        }
    }

    /// Encodes the op. Dispatch frames carry numeric `s` and string `t`; others carry nulls.
    pub fn encode(&self) -> Frame {
        match self {
            ServerOp::Dispatch { s, event } => {
                let (t, d) = event.encode();
                Frame {
                    op: Opcode::Dispatch.as_u8(),
                    d,
                    s: Some(*s),
                    t: Some(t),
                }
            }
            ServerOp::Reconnect(r) => {
                Frame::new(Opcode::Reconnect, to_value_or_panic("Reconnect", r))
            }
            ServerOp::InvalidSession(r) => Frame::new(
                Opcode::InvalidSession,
                to_value_or_panic("InvalidSession", r),
            ),
            ServerOp::Hello(r) => Frame::new(Opcode::Hello, to_value_or_panic("Hello", r)),
            ServerOp::HeartbeatAck => Frame::new(Opcode::HeartbeatAck, Value::Null),
        }
    }

    /// Decodes a frame from the server. Unknown events decode as `Event::Unknown`.
    pub fn decode(frame: &Frame) -> Result<ServerOp, GatewayDecodeError> {
        let op = Opcode::from_u8(frame.op).ok_or(GatewayDecodeError::UnknownOpcode(frame.op))?;
        if op.direction() != Direction::ServerToClient {
            return Err(GatewayDecodeError::WrongDirection(op));
        }
        Ok(match op {
            Opcode::Dispatch => {
                let (Some(s), Some(t)) = (frame.s, frame.t.as_deref()) else {
                    return Err(GatewayDecodeError::MissingSeqOrName);
                };
                let event = Event::decode(t, frame.d.clone()).map_err(|e| {
                    GatewayDecodeError::BadPayload {
                        op,
                        error: format!("event {t}: {e}"),
                    }
                })?;
                ServerOp::Dispatch { s, event }
            }
            Opcode::Reconnect => ServerOp::Reconnect(payload(op, &frame.d)?),
            Opcode::InvalidSession => ServerOp::InvalidSession(payload(op, &frame.d)?),
            Opcode::Hello => ServerOp::Hello(payload(op, &frame.d)?),
            Opcode::HeartbeatAck => ServerOp::HeartbeatAck,
            other => return Err(GatewayDecodeError::WrongDirection(other)),
        })
    }
}
