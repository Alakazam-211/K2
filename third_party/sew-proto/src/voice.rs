//! Voice channels (prd-sew-33 v2 §10, §13, §17–§19; v1 §9, §16–§18, §21.3): voice states, the
//! join, self-state, moderation and move bodies, the two voice dispatch bodies, READY's voice
//! sections and `status.voice`. The key half of a join (`e2ee`) is S50's ([`crate::e2ee`]).
//!
//! A voice state never carries a join id, identity, room, sid, epoch or key data (S33 v2
//! §10.1); a member's own `join_id` travels only in `CallKeysInit` and `voice_server_updated`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::addr::Slug;
use crate::e2ee::{CallId, CallKeysInit, E2eeJoin, JoinIdHex, StatusVoiceE2ee};
use crate::error::{Validate, ValidationError};
use crate::http::{Page, Patch};
use crate::id::Id;
use crate::perms::Perms;
use crate::time::Timestamp;

/// Largest `user_limit` of a voice channel (0 = no per-channel limit; S33 v1 §10.4).
pub const USER_LIMIT_MAX: u8 = 99;
/// Most voice states in READY before `voice_states_partial` (S33 v1 §16.3).
pub const READY_VOICE_STATES_MAX: usize = 5_000;

unit_enum! {
    /// Whether the engine has confirmed the member's media (S33 v2 §10.1).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum VoiceConnection {
        /// The admit is registered; media not confirmed yet.
        Connecting = "connecting",
        /// The engine confirmed the participant (also during a reconnect window).
        Connected = "connected",
    }
}

coded_enum! {
    /// Why a member left voice (S33 v2 §10.4). Sent only to the member themself. Codes are
    /// stable for metrics and logs.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum VoiceLeaveReason {
        /// The member left.
        Left = "left" => 1,
        /// The media connection ended for good.
        Disconnected = "disconnected" => 2,
        /// Another device took over (swap).
        Replaced = "replaced" => 3,
        /// Moved (or switched) to another channel.
        Moved = "moved" => 4,
        /// A moderator disconnected the member.
        RemovedByModerator = "removed_by_moderator" => 5,
        /// CONNECT or VIEW was lost.
        PermsLost = "perms_lost" => 6,
        /// The channel was archived.
        ChannelArchived = "channel_archived" => 7,
        /// The member left or was removed from the space.
        RemovedFromSpace = "removed_from_space" => 8,
        /// The member was timed out.
        TimedOut = "timed_out" => 9,
        /// The account was deprovisioned.
        AccountRemoved = "account_removed" => 10,
        /// The login session was revoked.
        SessionRevoked = "session_revoked" => 11,
        /// The device was revoked (S50 §10.4).
        DeviceRevoked = "device_revoked" => 12,
        /// The media outlived every socket.
        Orphaned = "orphaned" => 13,
        /// Alone in the channel too long.
        AloneTimeout = "alone_timeout" => 14,
        /// Media never connected.
        ConnectTimeout = "connect_timeout" => 15,
        /// The engine restarted or was down and the media did not come back.
        MediaRestarted = "media_restarted" => 16,
        /// Voice was turned off.
        VoiceDisabled = "voice_disabled" => 17,
        /// The sender stayed on a stale key slot (S50 §12.10; rev 78 D4, rev 85 D3).
        E2eeSenderIndexStuck = "e2ee_sender_index_stuck" => 18,
        /// The DM call ended (S34 v2 §14.5).
        CallEnded = "call_ended" => 19,
        /// Self-only (no leave): a camera or screen lease was released unused (S34 v2 §10.2).
        MediaUnused = "media_unused" => 20,
        /// Self-only (no leave): a camera or screen lease was revoked (S34 v2 §10.4).
        MediaRevoked = "media_revoked" => 21,
        /// The device stopped answering key requests (S50 unresponsive removal, F16 #29).
        Unresponsive = "unresponsive" => 22,
        /// The engine refused the device's client (an SDK that cannot switch the sender key index,
        /// rev 73) after Sew admitted it; the join is dropped at once (F16, #24).
        E2eeUnsupportedClient = "e2ee_unsupported_client" => 23,
        /// Declined the recording notice while declining means not joining (S35 v2 §9.11).
        RecordingDeclined = "recording_declined" => 24,
        /// Withdrew recording consent while declining means not joining (S35 v2 §9.11).
        RecordingWithdrawn = "recording_withdrawn" => 25,
        /// Left the recording notice unanswered too long while declining means not joining
        /// (S35 v2 §9.11).
        RecordingConsentTimeout = "recording_consent_timeout" => 26,
    }
}

/// A member's voice state (S33 v2 §10.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VoiceState {
    /// The member.
    pub member_id: Id,
    /// The channel's space; absent for DM calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// The voice channel.
    pub channel_id: Id,
    /// `connecting` or `connected`.
    pub connection: VoiceConnection,
    /// The member's device is in the activated key epoch (S50 `secure_changed`).
    pub secure: bool,
    /// Muted by the member.
    pub self_mute: bool,
    /// Deafened by the member (implies `self_mute`).
    pub self_deaf: bool,
    /// Muted by a moderator (persistent per space).
    pub server_mute: bool,
    /// Deafened by a moderator (persistent per space).
    pub server_deaf: bool,
    /// The member lacks SPEAK here; clients show it as muted.
    pub speak_denied: bool,
    /// When the join succeeded (a move resets it).
    pub joined_at: Timestamp,
    /// The DM call (`calls.id`) of a call entry (S34 v2 §20.1); absent in voice channels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<Id>,
    /// The member holds a camera lease (S34 v2 §10.2).
    #[serde(default)]
    pub self_video: bool,
    /// The member holds a screen-share lease (S34 v2 §10.2).
    #[serde(default)]
    pub self_stream: bool,
}

/// `POST /api/v1/channels/{id}/voice/join` body (S33 v2 §13.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VoiceJoinRequest {
    /// Join muted.
    #[serde(default)]
    pub self_mute: bool,
    /// Join deafened (implies muted).
    #[serde(default)]
    pub self_deaf: bool,
    /// Join with a camera lease (S34 v2 §10.2).
    #[serde(default)]
    pub self_video: bool,
    /// The key join (S50 K1). Required while voice is enabled; `None` answers
    /// `422 e2ee_unsupported_client` with `details.unsupported = e2ee`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e2ee: Option<E2eeJoin>,
    /// The answer to the recording consent modal (S35 v2 §9.4, §9.10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_notice: Option<crate::recording::RecordingNotice>,
}

impl Validate for VoiceJoinRequest {
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

/// The join answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VoiceJoinResponse {
    /// The signalling URL (`wss://voice.<host>`).
    pub url: String,
    /// The ES256 media token (`sub` = identity, `jti` = join id).
    pub token: String,
    /// When the token stops admitting a connect.
    pub expires_at: Timestamp,
    /// The call's room.
    pub room: CallId,
    /// The member's state.
    pub state: VoiceState,
    /// The key join's answer (S50 `CallKeysInit`, with `join_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e2ee: Option<CallKeysInit>,
    /// The client media policy for the current tier (S34 v2 §13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaPolicy>,
    /// The DM call (`calls.id`) of a call join (S34 v2 §14.3); not the engine room, which is
    /// `e2ee.call_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<Id>,
    /// The room's active recording as the joiner sees it (S35 v2 §9.4 rule 4): its `id` and
    /// `my_consent` feed S51's `Input::Recording` before the Welcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<crate::recording::Recording>,
    /// F21: the call-stats beacon is on (`calls.stats_beacon`); absent when it is off, and the
    /// client then sends nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<CallStatsBeacon>,
}

/// The call-stats beacon's terms for this join (F21, prd-meet-01 CC14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallStatsBeacon {
    /// Send one [`CallStatsSample`] per this many milliseconds while connected, and one when
    /// leaving (the server refuses more than [`CALL_STATS_PER_MINUTE`] a minute per session).
    pub interval_ms: u32,
}

/// The beacon's sample period (F21).
pub const CALL_STATS_INTERVAL_MS: u32 = 15_000;
/// Samples a session may send per minute (F21; the period's four plus a leave).
pub const CALL_STATS_PER_MINUTE: u32 = 6;
/// The largest body `POST /api/v1/me/voice/stats` reads (F21); a bigger one is `413` unread.
pub const CALL_STATS_BODY_MAX_BYTES: usize = 1_024;
/// The longest window one sample may cover (F21).
pub const CALL_STATS_WINDOW_MAX_MS: u32 = 120_000;
/// Most frames one sample may count (120 fps × 25 video tracks × 120 s).
pub const CALL_STATS_FRAMES_MAX: u32 = 360_000;
/// Most freezes or decrypt failures one sample may count.
pub const CALL_STATS_EVENTS_MAX: u32 = 100_000;
/// The longest round-trip time a sample may report.
pub const CALL_STATS_RTT_MAX_MS: u32 = 60_000;

unit_enum! {
    /// The browser family of a call-stats sample (F21): the family only, never a version or a
    /// user agent.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum CallStatsClient {
        /// Chrome, Edge and other Chromium browsers.
        Chromium = "chromium",
        /// Firefox.
        Firefox = "firefox",
        /// Safari (WebKit).
        Safari = "safari",
        /// Anything else.
        Other = "other",
    }
}

unit_enum! {
    /// The type of the local candidate of the selected ICE pair (F21; RFC 8445 §5.1.1).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum CallStatsCandidate {
        /// A host address.
        Host = "host",
        /// A server-reflexive address (STUN).
        Srflx = "srflx",
        /// A peer-reflexive address.
        Prflx = "prflx",
        /// A TURN relay.
        Relay = "relay",
    }
}

unit_enum! {
    /// How the browser reaches its TURN relay (F21; `relay` candidates only).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum CallStatsRelayProtocol {
        /// TURN over UDP.
        Udp = "udp",
        /// TURN over TCP.
        Tcp = "tcp",
        /// TURN over TLS.
        Tls = "tls",
    }
}

/// `POST /api/v1/me/voice/stats` body (F21, prd-meet-01 CC14): one aggregate, anonymous
/// sample of this browser's call over the last `interval_ms`. It names no member, room,
/// channel, call, track, address, URL, key or SDP (unknown fields are refused); Sew adds it to
/// `/metrics` families and keeps nothing per person or per call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallStatsSample {
    /// The window the counts cover, ms (1..=120,000).
    pub interval_ms: u32,
    /// The browser family.
    pub client: CallStatsClient,
    /// Video frames decoded in the window, summed over received tracks.
    pub frames_decoded: u32,
    /// Video frames dropped in the window (`framesDropped`).
    pub frames_dropped: u32,
    /// Video freezes in the window (`freezeCount`).
    pub freezes: u32,
    /// Time frozen in the window, ms (`totalFreezesDuration`).
    pub freeze_ms: u32,
    /// Frames that failed to decrypt in the window (E2EE).
    pub decrypt_failures: u32,
    /// The local candidate type of the selected pair (absent before ICE selected one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<CallStatsCandidate>,
    /// The relay protocol (`relay` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_protocol: Option<CallStatsRelayProtocol>,
    /// The selected pair's current round-trip time, ms (absent when unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<u32>,
}

impl Validate for CallStatsSample {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.interval_ms == 0 || self.interval_ms > CALL_STATS_WINDOW_MAX_MS {
            return Err(ValidationError::invalid(
                "interval_ms",
                "between 1 and 120000",
            ));
        }
        for (field, v, max) in [
            ("frames_decoded", self.frames_decoded, CALL_STATS_FRAMES_MAX),
            ("frames_dropped", self.frames_dropped, CALL_STATS_FRAMES_MAX),
            ("freezes", self.freezes, CALL_STATS_EVENTS_MAX),
            (
                "decrypt_failures",
                self.decrypt_failures,
                CALL_STATS_EVENTS_MAX,
            ),
        ] {
            if v > max {
                return Err(ValidationError::invalid(field, "too large for one sample"));
            }
        }
        // 25 video tracks frozen for the whole window at most.
        if u64::from(self.freeze_ms) > u64::from(self.interval_ms) * 25 {
            return Err(ValidationError::invalid(
                "freeze_ms",
                "longer than the window allows",
            ));
        }
        if self.rtt_ms.is_some_and(|r| r > CALL_STATS_RTT_MAX_MS) {
            return Err(ValidationError::invalid("rtt_ms", "at most 60000"));
        }
        if self.relay_protocol.is_some() && self.candidate != Some(CallStatsCandidate::Relay) {
            return Err(ValidationError::invalid(
                "relay_protocol",
                "only with a relay candidate",
            ));
        }
        Ok(())
    }
}

/// `POST /api/v1/me/voice/state` body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VoiceSelfStateRequest {
    /// Set self-mute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_mute: Option<bool>,
    /// Set self-deafen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_deaf: Option<bool>,
    /// Ask for (`true`) or release (`false`) a camera lease (S34 v2 §14.8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_video: Option<bool>,
    /// Ask for (`true`) or release (`false`) a screen-share lease (S34 v2 §14.8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_stream: Option<bool>,
}

impl Validate for VoiceSelfStateRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.self_mute.is_none()
            && self.self_deaf.is_none()
            && self.self_video.is_none()
            && self.self_stream.is_none()
        {
            return Err(ValidationError::invalid(
                "self_mute",
                "set at least one of self_mute, self_deaf, self_video and self_stream",
            ));
        }
        Ok(())
    }
}

/// `POST /api/v1/spaces/{id}/members/{member_id}/voice` body: server mute and deafen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VoiceFlagsRequest {
    /// Set server mute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_mute: Option<bool>,
    /// Set server deafen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_deaf: Option<bool>,
}

impl Validate for VoiceFlagsRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.server_mute.is_none() && self.server_deaf.is_none() {
            return Err(ValidationError::invalid(
                "server_mute",
                "set server_mute, server_deaf or both",
            ));
        }
        Ok(())
    }
}

/// A member's persistent server flags in a space (the flags answer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VoiceFlags {
    /// The space.
    pub space_id: Id,
    /// The member.
    pub member_id: Id,
    /// Server-muted.
    pub server_mute: bool,
    /// Server-deafened.
    pub server_deaf: bool,
}

/// `POST /api/v1/spaces/{id}/members/{member_id}/voice/move` body: a channel moves the member,
/// `null` disconnects them. The key must be present.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VoiceMoveRequest {
    /// The destination voice channel, or `null` to disconnect.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub channel_id: Patch<Id>,
}

impl Validate for VoiceMoveRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.channel_id.is_keep() {
            return Err(ValidationError::invalid(
                "channel_id",
                "channel_id is required (null disconnects)",
            ));
        }
        Ok(())
    }
}

/// Body of `voice_state_updated` (S33 v1 §16.1): `channel_id = null` and `state = null` mean the
/// member left voice. `reason` is only in the copy sent to the member themself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VoiceStateUpdated {
    /// The member.
    pub member_id: Id,
    /// The space; absent for DM calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// The channel now; absent when the member left voice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<Id>,
    /// The channel before a move or a leave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_channel_id: Option<Id>,
    /// The state now; absent when the member left voice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<VoiceState>,
    /// Why the member left or moved (self copy only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<VoiceLeaveReason>,
}

unit_enum! {
    /// How a moved member keys into the destination call (`voice_server_updated.e2ee.key_join`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum KeyJoinMode {
        /// Upload a ref scoped to the destination channel (K16), then key-join with K2.
        Required = "required",
    }
}

/// The key half of `voice_server_updated` (S33 v2 §13.6; S50 §12.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VoiceMoveKeyJoin {
    /// The destination call.
    pub call_id: CallId,
    /// The join id Sew minted for the destination (use it in K2).
    pub join_id: JoinIdHex,
    /// `required`.
    pub key_join: KeyJoinMode,
}

/// Body of `voice_server_updated`, to the moved member only (S33 v2 §13.6, §17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VoiceServerUpdated {
    /// The destination channel.
    pub channel_id: Id,
    /// The destination room.
    pub room: CallId,
    /// The signalling URL.
    pub url: String,
    /// A fresh media token for the destination admit.
    pub token: String,
    /// When the token stops admitting a connect.
    pub expires_at: Timestamp,
    /// The key join for the destination call.
    pub e2ee: VoiceMoveKeyJoin,
    /// Why (`moved`).
    pub reason: VoiceLeaveReason,
    /// The client media policy (S34 v2 §13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaPolicy>,
    /// The DM call, when the destination is a call (S34 v2 §17).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<Id>,
    /// The destination room's active recording, `pending` for the moved member (S35 v2 §9.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<crate::recording::Recording>,
}

/// A public voice channel in READY that the member can VIEW but has not joined (S33 v1 §16.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadyVoiceChannel {
    /// The channel.
    pub id: Id,
    /// Its space.
    pub space_id: Id,
    /// Its category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_id: Option<Id>,
    /// The slug.
    pub slug: Slug,
    /// The name.
    pub name: String,
    /// Sort position.
    pub position: u32,
    /// 0 = no per-channel limit.
    pub user_limit: u8,
    /// The member's effective permissions there.
    pub my_perms: Perms,
}

/// `GET /api/v1/me/voice`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeVoice {
    /// The member's voice state; absent when not in voice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<VoiceState>,
}

/// A list of voice states (`GET …/voice/states`; one page, `has_more: false`).
pub type VoiceStatePage = Page<VoiceState>;

unit_enum! {
    /// The engine as `status.voice.engine` sees it (`details.engine` of a voice `503`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum VoiceEngineState {
        /// Healthy.
        Up = "up",
        /// Unreachable for `health_failures_down` checks in a row.
        Down = "down",
        /// Draining (S49).
        Draining = "draining",
        /// Sew is resyncing from a snapshot.
        Syncing = "syncing",
        /// The engine answered with an error, or the call keys failed to load.
        Error = "error",
    }
}

/// `status.voice.engine` (S33 v2 §22; S37 §13.2 health).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VoiceEngineStatus {
    /// The engine's state.
    pub state: VoiceEngineState,
    /// The engine's version, from its last health answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Rooms the engine reports.
    pub rooms: u64,
    /// Participants the engine reports.
    pub participants: u64,
    /// Engine panics in the last hour.
    pub panics_1h: u64,
    /// When the engine went down (state `down`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down_since: Option<Timestamp>,
    /// The last event Sew applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_at: Option<Timestamp>,
    /// The last snapshot audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_snapshot_at: Option<Timestamp>,
    /// Drift the snapshot audits fixed in the last hour.
    pub drift_fixes_1h: u64,
}

/// `status.voice` (S33 v2 §22, v1 §21.3 without the LiveKit fields). Absent when voice is off
/// and was never configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VoiceStatus {
    /// `[voice] enabled`.
    pub enabled: bool,
    /// `[voice] public_url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    /// Live calls (voice channels with occupants).
    pub calls: u64,
    /// Voice entries.
    pub participants: u64,
    /// Entries whose device is in the activated key epoch.
    pub secure_participants: u64,
    /// Entries inside a reconnect window.
    pub reconnecting: u64,
    /// The engine.
    pub engine: VoiceEngineStatus,
    /// The call key service (S50 §19.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e2ee: Option<StatusVoiceE2ee>,
    /// Video, DM calls and the media budget (S34 v2 §25.3), as `status.voice.media` (S1 C15
    /// forbids `flatten` on the wire types, so S34's fields nest like S50's `e2ee`). Absent
    /// while voice is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<VoiceMediaStatus>,
}

/// S34 v2's part of `status.voice` (§25.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VoiceMediaStatus {
    /// Active DM calls.
    pub calls_active: u64,
    /// Camera leases install-wide.
    pub video_publishers: u64,
    /// Screen-share leases install-wide.
    pub screen_publishers: u64,
    /// The media budget tier.
    pub media_tier: MediaTier,
    /// Where the budget's egress comes from.
    pub budget_source: BudgetSource,
    /// The egress the tiers use (the engine's measured egress, or the estimate), Mbit/s.
    pub egress_mbps: f64,
    /// `[voice.video] egress_budget_mbps`.
    pub egress_budget_mbps: u64,
    /// The forwarding estimate, Mbit/s.
    pub forward_estimate_mbps: f64,
    /// The engine cgroup's CPU against its allowance; `null` when unknown.
    pub engine_cpu_pct: Option<f64>,
    /// `[voice.video] enabled` (and voice enabled).
    pub video_enabled: bool,
}

unit_enum! {
    /// Where the media budget's egress comes from (S34 v2 §12.1).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum BudgetSource {
        /// The engine's `stats` events.
        Engine = "engine",
        /// Sew's forwarding estimate (stats missing or stale).
        Estimate = "estimate",
    }
}

unit_enum! {
    /// The media budget tier (S34 v2 §12.2).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MediaTier {
        /// Everything allowed; full hints.
        Normal = "normal",
        /// New camera leases refused; lower hints.
        Constrained = "constrained",
        /// New camera and screen leases refused; lowest hints.
        Critical = "critical",
    }
}

unit_enum! {
    /// A video codec a client may publish under E2EE (S34 v2 D8). There is no AV1 or H.265
    /// value: they are never offered.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum VideoCodec {
        /// VP8 (simulcast).
        Vp8 = "vp8",
        /// H.264 (simulcast).
        H264 = "h264",
        /// VP9, only with the Dependency Descriptor (opt-in).
        Vp9 = "vp9",
    }
}

/// One simulcast layer hint (S34 v2 §13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LayerHint {
    /// `q`, `h` or `f`.
    pub rid: String,
    /// Layer height in pixels.
    pub height: u32,
    /// Layer bitrate cap.
    pub max_kbps: u32,
}

/// Publishing hints for one source (S34 v2 §13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TrackHint {
    /// Capture width cap.
    pub max_width: u32,
    /// Capture height cap.
    pub max_height: u32,
    /// Frame rate cap.
    pub max_fps: u32,
    /// Simulcast layers, lowest first.
    pub layers: Vec<LayerHint>,
}

/// VP9 publishing rules when VP9 is configured (S34 v2 §13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Vp9Hint {
    /// `L3T3_KEY`.
    pub scalability_mode: String,
    /// Always `true`: the Dependency Descriptor is required.
    pub requires_dd: bool,
}

/// The client media policy (S34 v2 §13): in join answers, `voice_server_updated`, READY
/// `install.media` and `media_policy_updated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MediaPolicy {
    /// The budget tier the hints are for.
    pub tier: MediaTier,
    /// `[voice.video] enabled`.
    pub video_enabled: bool,
    /// Always `required`.
    pub e2ee: String,
    /// Always `["opus"]`.
    pub audio_codecs: Vec<String>,
    /// Video codecs in preference order (never AV1 or H.265; empty with video off).
    pub codecs: Vec<VideoCodec>,
    /// Always `false`: no backup codecs.
    pub backup_codec: bool,
    /// VP9 rules, when VP9 is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vp9: Option<Vp9Hint>,
    /// Camera hints.
    pub camera: TrackHint,
    /// Screen-share hints.
    pub screen: TrackHint,
    /// Publish with dynacast.
    pub dynacast: bool,
    /// Subscribe with adaptive stream.
    pub adaptive_stream: bool,
}

/// Every voice reason string under the existing error codes (S33 v1 §18; S33 v2 §11.1).
pub mod reasons {
    /// `422`: the channel is not a voice channel.
    pub const NOT_VOICE_CHANNEL: &str = "not_voice_channel";
    /// `403`: the channel is at its `user_limit`.
    pub const CHANNEL_FULL: &str = "channel_full";
    /// `409`: the member is not in voice.
    pub const NOT_IN_VOICE: &str = "not_in_voice";
    /// `403`: agents and assistants never join voice (and key routes refuse act-as).
    pub const AGENTS_NOT_IN_VOICE: &str = "agents_not_in_voice";
    /// `403`: the move target cannot connect to the destination.
    pub const TARGET_CANNOT_CONNECT: &str = "target_cannot_connect";
    /// `503`: `[voice] enabled = false`.
    pub const VOICE_DISABLED: &str = "voice_disabled";
    /// `503`: the engine is down, draining or syncing (`details.engine`).
    pub const VOICE_UNAVAILABLE: &str = "voice_unavailable";
    /// `503`: the install participant cap is reached.
    pub const VOICE_CAPACITY: &str = "voice_capacity";
    /// `409`: a declined member joining while a recording is live, when declining refuses
    /// joins ([`crate::e2ee::RECORDING_DECLINE_JOINS_UNRECORDED`] = `false`).
    pub const RECORDING_CONSENT_REQUIRED: &str = crate::e2ee::RECORDING_CONSENT_REQUIRED;
    /// `403 policy_denied`: `[voice.video] enabled = false` (S34 v2 §7.2 step 8).
    pub const VIDEO_DISABLED: &str = "video_disabled";
    /// `422`: the room's camera cap is reached (`details.limit`).
    pub const VIDEO_SLOTS_FULL: &str = "video_slots_full";
    /// `422`: the room's screen-share cap is reached (`details.limit`).
    pub const SCREEN_SLOTS_FULL: &str = "screen_slots_full";
    /// `503`: the media budget refuses new video (`Retry-After: 30`).
    pub const MEDIA_BUSY: &str = "media_busy";
    /// `503`: the install's video publisher cap is reached.
    pub const MEDIA_CAPACITY: &str = "media_capacity";
    /// `403 policy_denied`: `calls.stats_beacon` is off (F21).
    pub const CALL_STATS_OFF: &str = "call_stats_off";
}

/// `details.engine` values of a voice `503 voice_unavailable`.
pub mod engine_detail {
    /// The engine is unreachable.
    pub const DOWN: &str = "down";
    /// The engine is draining.
    pub const DRAINING: &str = "draining";
    /// Sew is resyncing (or the DS is rebuilding).
    pub const SYNCING: &str = "syncing";
    /// The engine errored, or the call keys failed to load.
    pub const ERROR: &str = "error";
}

/// A channel `user_limit` check (0–99) shared by the create and update validators.
pub(crate) fn check_user_limit(v: Option<u8>) -> Result<(), ValidationError> {
    match v {
        Some(n) if n > USER_LIMIT_MAX => Err(ValidationError::invalid(
            "user_limit",
            format!("user_limit must be 0–{USER_LIMIT_MAX}"),
        )),
        _ => Ok(()),
    }
}
