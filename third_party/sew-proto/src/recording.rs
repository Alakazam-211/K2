//! Call recording and per-speaker transcripts (prd-sew-35 v2 §8–§19, §23): the recording
//! object, consent and capture, the consent modal contract (rev 91), the record-message summary,
//! transcript turns, the audio listing and the compliance download.
//!
//! No engine identity, join id, room name, key or certificate ever appears in these types
//! (§18.1); the recorder's engine identity reaches clients only in S50's `recorder` dispatch.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::e2ee::ConsentState;
use crate::error::{Validate, ValidationError};
use crate::id::Id;
use crate::time::Timestamp;

/// The recorder's display name (rev 90): the roster entry, the recording banner and every
/// notice text use it. Never sent to the engine (its engine name stays empty).
pub const RECORDER_DISPLAY_NAME: &str = crate::RECORDER_DISPLAY_NAME;
/// The optional feature label templates may show for recording + transcripts (rev 90).
pub const RECORDING_FEATURE_LABEL: &str = "Sew Notes";
/// Most active recordings in READY `recordings` (§18.3).
pub const READY_RECORDINGS_MAX: usize = 100;
/// Most bytes of a compliance download's `reason` (§19.4).
pub const DOWNLOAD_REASON_MAX: usize = 500;
/// The format string of recorder-sealed audio (§19.2, §19.4).
pub const SEALED_AUDIO_FORMAT: &str = "sew-stream-v1+audio/ogg; codecs=opus";

/// Whisper language codes accepted as a hint (§8.2 check 10; mostly ISO 639-1, plus e.g.
/// `haw`, `yue`), besides `auto`.
pub const WHISPER_LANGUAGES: &[&str] = &[
    "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv", "it",
    "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no", "th", "ur",
    "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk", "te", "fa", "lv", "bn", "sr", "az", "sl", "kn",
    "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw", "gl", "mr", "pa", "si",
    "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo",
    "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln",
    "ha", "ba", "jw", "su", "yue",
];

/// Whether `s` is `auto` or a known Whisper language code.
pub fn language_ok(s: &str) -> bool {
    s == "auto" || WHISPER_LANGUAGES.contains(&s)
}

/// `details.reason` values (§23; rev 91).
pub mod reasons {
    /// `503`: `[recording] enabled = false`.
    pub const RECORDING_UNAVAILABLE: &str = "recording_unavailable";
    /// `503`: the recorder is unhealthy.
    pub const RECORDER_UNAVAILABLE: &str = "recorder_unavailable";
    /// `503`: the transcriber is unhealthy.
    pub const TRANSCRIBER_UNAVAILABLE: &str = "transcriber_unavailable";
    /// `503`: too many active recordings.
    pub const RECORDING_CAPACITY: &str = "recording_capacity";
    /// `503`: the transcription backlog is too large.
    pub const TRANSCRIBE_BACKLOG: &str = "transcribe_backlog";
    /// `503`: the room's key group is rebuilding.
    pub const E2EE_REBUILDING: &str = "e2ee_rebuilding";
    /// `403 policy_denied`: policy `recording.enabled` is off.
    pub const RECORDING_DISABLED: &str = "recording_disabled";
    /// `403 policy_denied`: policy `recording.dm_enabled` is off.
    pub const DM_RECORDING_DISABLED: &str = "dm_recording_disabled";
    /// `403 policy_denied`: agents and act-as never start, stop or consent.
    pub const AGENTS_CANNOT_RECORD: &str = "agents_cannot_record";
    /// `403 policy_denied`: agents never get audio.
    pub const AGENTS_NO_AUDIO: &str = "agents_no_audio";
    /// `403 policy_denied`: declining means not joining (`recording.require_consent_to_join`).
    pub const RECORDING_CONSENT_REQUIRED_TO_JOIN: &str = "recording_consent_required_to_join";
    /// `409`: a recording is already active in the room.
    pub const RECORDING_ACTIVE: &str = "recording_active";
    /// `409`: the join needs the consent modal's answer.
    pub const RECORDING_CONSENT_REQUIRED: &str = crate::e2ee::RECORDING_CONSENT_REQUIRED;
    /// `422`: the recording is not active.
    pub const RECORDING_NOT_ACTIVE: &str = "recording_not_active";
    /// `422`: the record message cannot be deleted while the recording or its transcript runs.
    pub const RECORDING_IN_PROGRESS: &str = "recording_in_progress";
    /// `422`: the record message cannot be edited.
    pub const RECORDING_RECORD_IMMUTABLE: &str = "recording_record_immutable";
    /// `422`: transcript turns cannot be edited.
    pub const TRANSCRIPT_IMMUTABLE: &str = "transcript_immutable";
    /// `422`: the audio is no longer available.
    pub const AUDIO_EXPIRED: &str = "audio_expired";
    /// `422`: the audio was purged.
    pub const AUDIO_PURGED: &str = "audio_purged";
    /// `422`: the caller has no voice entry in the room.
    pub const NOT_IN_VOICE: &str = "not_in_voice";
    /// `422`: not a voice channel, DM or group DM.
    pub const NOT_VOICE_CHANNEL: &str = "not_voice_channel";
}

coded_enum! {
    /// A recording's lifecycle state (§8.5). Stored in `recordings.state`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum RecordingState {
        /// Admitting and certifying the recorder.
        Starting = "starting" => 1,
        /// The recorder joined.
        Recording = "recording" => 2,
        /// Draining the recorder.
        Stopping = "stopping" => 3,
        /// Over.
        Stopped = "stopped" => 4,
    }
}

impl RecordingState {
    /// `starting`, `recording` or `stopping`.
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            RecordingState::Starting | RecordingState::Recording | RecordingState::Stopping
        )
    }
}

coded_enum! {
    /// Why a recording stopped (§8.5). Stored in `recordings.stop_reason`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum StopReason {
        /// The stop route.
        Stopped = "stopped" => 1,
        /// No standard voice entries left in the room.
        RoomEmpty = "room_empty" => 2,
        /// The DM call ended.
        CallEnded = "call_ended" => 3,
        /// `recording.max_duration_ms` reached.
        MaxDuration = "max_duration" => 4,
        /// The recorder failed (join timeout, certificate, unreachable, too many rejoins).
        RecorderFailed = "recorder_failed" => 5,
        /// The engine room was lost beyond the re-admit grace.
        MediaRestarted = "media_restarted" => 6,
        /// Three consecutive `storage_full` segment writes.
        StorageFull = "storage_full" => 7,
        /// Recording was disabled (policy, config, archive, `sew recording disable`).
        Disabled = "disabled" => 8,
        /// `recording.max_bytes` reached.
        MaxBytes = "max_bytes" => 9,
    }
}

coded_enum! {
    /// What a participant said to the recording notice (§9.2). Stored in
    /// `recording_participants.consent`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum Consent {
        /// In the room, has not answered.
        Pending = "pending" => 1,
        /// Agreed.
        Granted = "granted" => 2,
        /// Refused; stays in the room unrecorded (unless declining means not joining).
        Declined = "declined" => 3,
        /// Agreed earlier, then withdrew.
        Withdrawn = "withdrawn" => 4,
    }
}

impl Consent {
    /// The three-valued state S50's `ConsentSource` and S51's `Input::Recording` take (step
    /// 0b fallback: `Granted` iff `granted`; declined and withdrawn are `Denied`).
    pub const fn to_state(self) -> ConsentState {
        match self {
            Consent::Granted => ConsentState::Granted,
            Consent::Pending => ConsentState::Pending,
            Consent::Declined | Consent::Withdrawn | Consent::Unknown => ConsentState::Denied,
        }
    }
}

unit_enum! {
    /// Whether a granted participant's microphone is actually captured (§9.2).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum Capture {
        /// The recorder captures this member.
        Recorded = "recorded",
        /// Not captured (see the reason).
        NotRecorded = "not_recorded",
    }
}

coded_enum! {
    /// Why a granted participant is not captured (§9.2). Stored in
    /// `recording_participants.capture_reason`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum NotRecordedReason {
        /// A track cap is reached; not allowlisted.
        TrackCap = "track_cap" => 1,
        /// No microphone published.
        NoMicrophone = "no_microphone" => 2,
        /// The recorder could not decrypt the member's frames.
        NoKey = "no_key" => 3,
        /// The track was published unencrypted.
        Unencrypted = "unencrypted" => 4,
        /// The track is not Opus.
        Codec = "codec" => 5,
        /// Not subscribable under the room's permissions.
        Permission = "permission" => 6,
    }
}

coded_enum! {
    /// How a consent answer arrived (§9; audit `via`). Stored in `recording_participants.via`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ConsentVia {
        /// The starter's start request.
        Start = "start" => 1,
        /// A join's `recording_notice`.
        Join = "join" => 2,
        /// The in-room prompt (`POST …/consent`).
        Prompt = "prompt" => 3,
        /// A withdrawal.
        Withdraw = "withdraw" => 4,
        /// The joiner of an auto-record channel.
        Auto = "auto" => 5,
    }
}

coded_enum! {
    /// Where the person answered (§9.10). Stored in `recording_participants.surface`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ConsentSurface {
        /// The blocking modal before joining.
        Modal = "modal" => 1,
        /// The in-room prompt.
        Prompt = "prompt" => 2,
        /// The starter's confirmation before `start`.
        Start = "start" => 3,
    }
}

unit_enum! {
    /// What declining does (§9.11, from `recording.require_consent_to_join`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum DeclineMode {
        /// The decliner joins, unrecorded (the default).
        JoinUnrecorded = "join_unrecorded",
        /// Declining means not joining.
        NotJoin = "not_join",
    }
}

coded_enum! {
    /// The recording's audio (§12). Stored in `recordings.audio_state`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum AudioState {
        /// Kept.
        Available = "available" => 1,
        /// Hidden; links may still exist (held or not yet purged).
        Expired = "expired" => 2,
        /// Every link deleted; the content key is gone.
        Purged = "purged" => 3,
    }
}

coded_enum! {
    /// The transcript (§14.5). Stored in `recordings.transcript_state`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum TranscriptState {
        /// No segment transcribed yet.
        Pending = "pending" => 1,
        /// Turns are being posted.
        Running = "running" => 2,
        /// Final, complete.
        Done = "done" => 3,
        /// Final, with gaps or failed segments.
        Partial = "partial" => 4,
        /// Final, no turn posted and at least one segment failed.
        Failed = "failed" => 5,
    }
}

coded_enum! {
    /// Why a span of a recording is missing (§24). Stored in `recording_gaps.reason`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum GapReason {
        /// The recorder process restarted.
        RecorderRestarted = "recorder_restarted" => 1,
        /// The recorder's buffer was full.
        RecorderBufferFull = "recorder_buffer_full" => 2,
        /// The blob store was full.
        StorageFull = "storage_full" => 3,
        /// A segment exceeded `recording.segment_max_bytes`.
        TooLarge = "too_large" => 4,
        /// Transcription failed for a segment.
        TranscribeFailed = "transcribe_failed" => 5,
        /// The audio was purged before it was transcribed.
        AudioGone = "audio_gone" => 6,
        /// A track cap.
        TrackCap = "track_cap" => 7,
        /// The recorder held no key for a consenting speaker.
        NoKey = "no_key" => 8,
        /// The recorder rejoined with a new admit.
        RecorderReconnected = "recorder_reconnected" => 9,
        /// A key was missing mid-segment.
        KeyMissing = "key_missing" => 10,
        /// A media discontinuity.
        Discontinuity = "discontinuity" => 11,
        /// The engine restarted (same admit, fresh token).
        EngineRestart = "engine_restart" => 12,
    }
}

unit_enum! {
    /// Whether the audio listing hands out keys (§19.2; P3 in v1, rev 91).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum Playback {
        /// No in-app playback (P3, the default).
        Off = "off",
        /// P1: keys and ciphertext URLs for local unsealing.
        ClientUnseal = "client_unseal",
    }
}

coded_enum! {
    /// What a compliance download covers (§19.4). Stored in `recording_downloads.what`.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum DownloadWhat {
        /// The mixed parts.
        Mixed = "mixed" => 1,
        /// The per-speaker segments.
        Segments = "segments" => 2,
        /// Both.
        #[default]
        Both = "both" => 3,
    }
}

/// What a `recording_notice` refers to: a live recording, or `"auto"` for an auto-record
/// channel with no recording yet (§8.3, §9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordingRef {
    /// The live recording.
    Id(Id),
    /// Start one on join.
    Auto,
}

impl fmt::Display for RecordingRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordingRef::Id(id) => write!(f, "{id}"),
            RecordingRef::Auto => f.write_str("auto"),
        }
    }
}

impl Serialize for RecordingRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            RecordingRef::Id(id) => id.serialize(s),
            RecordingRef::Auto => s.serialize_str("auto"),
        }
    }
}

impl<'de> Deserialize<'de> for RecordingRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s == "auto" {
            return Ok(RecordingRef::Auto);
        }
        s.parse::<Id>()
            .map(RecordingRef::Id)
            .map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for RecordingRef {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "RecordingRef".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^(auto|[1-9][0-9]{0,18})$",
            "description": "A recording id, or \"auto\" for an auto-record channel with no recording yet."
        })
    }
}

/// The answer to the consent modal, sent with a voice join or call join/accept (§9.4, §9.10).
/// Joining never implies consent: `consent` is required and `notice_version` must echo the
/// current [`ConsentPrompt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordingNotice {
    /// The recording (or `"auto"`).
    pub recording_id: RecordingRef,
    /// The person's answer. Missing → `400 validation_failed`, field `recording_notice.consent`.
    #[serde(default)]
    pub consent: Option<bool>,
    /// The prompt version the person saw.
    #[serde(default)]
    pub notice_version: Option<u32>,
}

impl Validate for RecordingNotice {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.consent.is_none() {
            return Err(ValidationError::invalid(
                "consent",
                "the consent modal's answer is required; joining never implies consent",
            ));
        }
        Ok(())
    }
}

/// One participant of a recording, as everyone in the audience sees it (§18.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordingParticipant {
    /// The member.
    pub member_id: Id,
    /// What they said.
    pub consent: Consent,
    /// Whether they are captured (only for `granted`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<Capture>,
    /// Why not (only for `not_recorded`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<NotRecordedReason>,
}

/// The starter as the consent modal shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PromptMember {
    /// The member.
    pub member_id: Id,
    /// Their display name.
    pub display_name: String,
}

unit_enum! {
    /// The room kind the modal names.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum PromptRoomKind {
        /// A voice channel.
        Voice = "voice",
        /// A DM call.
        Dm = "dm",
        /// A group-DM call.
        GroupDm = "group_dm",
    }
}

/// Where the recording is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PromptRoom {
    /// The voice channel or DM.
    pub channel_id: Id,
    /// Its kind.
    pub kind: PromptRoomKind,
    /// The channel's name (voice channels only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// What happens to the words (§9.10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PromptTranscript {
    /// Transcripts are posted into the channel as ordinary messages.
    pub posted_to_channel: bool,
    /// Who can read them: `channel_members`, `search`, `agents`.
    pub readable_by: Vec<String>,
    /// Transcripts are not end-to-end encrypted.
    pub e2ee: bool,
}

impl Default for PromptTranscript {
    fn default() -> Self {
        PromptTranscript {
            posted_to_channel: true,
            readable_by: vec![
                "channel_members".to_owned(),
                "search".to_owned(),
                "agents".to_owned(),
            ],
            e2ee: false,
        }
    }
}

/// The consent modal's data (rev 91, §9.10). Every slot is data; templates supply the
/// sentences. `notice_version` is a stable hash of every other slot; the answer must echo it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConsentPrompt {
    /// What the answer refers to.
    pub recording_id: RecordingRef,
    /// A stable hash of every other slot value.
    pub notice_version: u32,
    /// [`RECORDER_DISPLAY_NAME`].
    pub recorder_name: String,
    /// [`RECORDING_FEATURE_LABEL`].
    pub feature_label: String,
    /// Who started it (`None` for an auto channel with no recording yet).
    pub started_by: Option<PromptMember>,
    /// Where.
    pub room: PromptRoom,
    /// How long audio is kept (0 = until the transcript is final).
    pub audio_keep_days: u16,
    /// What happens to the words.
    pub transcript: PromptTranscript,
    /// Authorised compliance admins can download the audio (always `true` in v1).
    pub admin_download: bool,
    /// What declining does.
    pub decline_mode: DeclineMode,
    /// Who is currently recorded (granted and captured), ascending.
    pub recorded_now: Vec<Id>,
}

impl ConsentPrompt {
    /// The stable hash of every slot but `notice_version` (FNV-1a 32 over a canonical text).
    pub fn compute_version(&self) -> u32 {
        let mut text = String::new();
        let started = self
            .started_by
            .as_ref()
            .map(|m| format!("{}:{}", m.member_id, m.display_name))
            .unwrap_or_default();
        let recorded: Vec<String> = self.recorded_now.iter().map(Id::to_string).collect();
        text.push_str(&format!(
            "v1|{}|{}|{}|{}|{}|{:?}|{:?}|{}|{}|{}|{:?}|{}|{}|{}",
            self.recording_id,
            self.recorder_name,
            self.feature_label,
            started,
            self.room.channel_id,
            self.room.kind.as_str(),
            self.room.name,
            self.audio_keep_days,
            self.transcript.posted_to_channel,
            self.transcript.readable_by.join(","),
            self.transcript.e2ee,
            self.admin_download,
            self.decline_mode.as_str(),
            recorded.join(","),
        ));
        let mut h: u32 = 0x811c_9dc5;
        for b in text.bytes() {
            h ^= u32::from(b);
            h = h.wrapping_mul(0x0100_0193);
        }
        h
    }

    /// Sets `notice_version` from the other slots.
    pub fn sealed(mut self) -> ConsentPrompt {
        self.notice_version = self.compute_version();
        self
    }
}

/// A recording (§18.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Recording {
    /// The recording.
    pub id: Id,
    /// The voice channel or DM.
    pub channel_id: Id,
    /// The DM call (`calls.id`) of a DM-call recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_record_id: Option<Id>,
    /// The channel's space (absent for DMs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// Its state.
    pub state: RecordingState,
    /// Who started it.
    pub started_by: Id,
    /// When.
    pub started_at: Timestamp,
    /// Started by an auto-record channel's first join.
    pub auto: bool,
    /// When it stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<Timestamp>,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    /// The record message.
    pub message_id: Id,
    /// A mixed file is made after stop.
    pub mixed: bool,
    /// Audio keep days (0 = until the transcript is final).
    pub audio_keep_days: u16,
    /// The language hint (`None` = auto).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// `recording.max_duration_ms`.
    pub max_duration_ms: u64,
    /// Members in the room plus members who ever granted, ascending.
    pub participants: Vec<RecordingParticipant>,
    /// The viewer's own consent (self only; absent for non-participants).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_consent: Option<Consent>,
    /// The consent modal (self only, while the viewer is `pending`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent_prompt: Option<ConsentPrompt>,
}

impl Recording {
    /// The pair S51's `Input::Recording` takes (§9.9 item 9): the live recording id and the
    /// viewer's consent mapped to S50's three values.
    pub fn e2ee_input(&self) -> (Option<Id>, ConsentState) {
        let live = self.state.is_active().then_some(self.id);
        let c = self
            .my_consent
            .map(Consent::to_state)
            .unwrap_or(ConsentState::Pending);
        (live, c)
    }
}

/// `POST /api/v1/channels/{id}/recording/start` body (§8.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartRecordingRequest {
    /// Audio keep days (clamped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_keep_days: Option<u16>,
    /// Make a mixed file after stop (default `true`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mixed: Option<bool>,
    /// A Whisper language code or `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

impl Validate for StartRecordingRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(l) = &self.language
            && !language_ok(l)
        {
            return Err(ValidationError::invalid(
                "language",
                "not a supported language code",
            ));
        }
        Ok(())
    }
}

/// `POST /api/v1/recordings/{id}/consent` body (§9.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsentRequest {
    /// Grant or decline/withdraw.
    pub consent: bool,
    /// The prompt version the person saw (rev 91).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_version: Option<u32>,
}

impl Validate for ConsentRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// `POST /api/v1/recordings/{id}/audio/retention` body (§12.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AudioRetentionRequest {
    /// New keep days (clamped).
    pub audio_keep_days: u16,
}

impl Validate for AudioRetentionRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        // Any value is clamped (§12.1).
        Ok(())
    }
}

/// `GET /api/v1/channels/{id}/recording` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActiveRecording {
    /// The active recording, if any.
    pub recording: Option<Recording>,
}

/// The audio part of a [`RecordingSummary`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryAudio {
    /// Its state.
    pub state: AudioState,
    /// When it expires (set at transcript final).
    pub expires_at: Option<Timestamp>,
    /// A mixed file is (or will be) made.
    pub mixed: bool,
    /// Keep days.
    pub keep_days: u16,
}

/// The transcript part of a [`RecordingSummary`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryTranscript {
    /// Its state.
    pub state: TranscriptState,
    /// Turns posted.
    pub turns: u32,
    /// Total gap time.
    pub gaps_ms: u64,
    /// The majority language of the turns.
    pub lang: Option<String>,
}

/// `Message.recording` on the record message (§15.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordingSummary {
    /// The recording.
    pub recording_id: Id,
    /// Who started it.
    pub started_by: Id,
    /// When.
    pub started_at: Timestamp,
    /// Auto-started.
    pub auto: bool,
    /// When it stopped.
    pub stopped_at: Option<Timestamp>,
    /// Why.
    pub stop_reason: Option<StopReason>,
    /// `stopped_at − started_at` once stopped.
    pub duration_ms: Option<u64>,
    /// Members whose speech was stored, ascending.
    pub speaker_ids: Vec<Id>,
    /// The audio.
    pub audio: SummaryAudio,
    /// The transcript.
    pub transcript: SummaryTranscript,
    /// Always `true` in v2: every call is end-to-end encrypted.
    pub e2ee: bool,
}

/// One piece of a transcript turn (numbers only; text lives in the message body, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnPiece {
    /// Start, ms from the recording start.
    pub start_ms: u64,
    /// End, ms from the recording start.
    pub end_ms: u64,
    /// Byte offset of the piece's text in the body.
    pub offset: u32,
    /// Byte length of the piece's text in the body.
    pub len: u32,
}

/// `Message.transcript` on a transcript turn (§15.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TranscriptTurn {
    /// The recording.
    pub recording_id: Id,
    /// The speaker (also the message author).
    pub speaker_id: Id,
    /// Start, ms from the recording start.
    pub start_ms: u64,
    /// End, ms from the recording start.
    pub end_ms: u64,
    /// When it was said (`created_at` is posting time).
    pub spoken_at: Timestamp,
    /// The turn's language.
    pub lang: Option<String>,
    /// The pieces.
    pub pieces: Vec<TurnPiece>,
}

/// One mixed part in an audio listing or download (§19.2, §19.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MixedPart {
    /// Part number, from 1.
    pub part: u16,
    /// Offset from the recording start.
    pub offset_ms: u64,
    /// Length (≤ 60 min).
    pub duration_ms: u64,
    /// Sealed size.
    pub bytes: u64,
    /// The ciphertext URL (P1 or download only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The part key, base64 (P1 or download only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// [`SEALED_AUDIO_FORMAT`] (P1 only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// One speaker's track in an audio listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioTrack {
    /// The speaker.
    pub member_id: Id,
    /// Stored segments.
    pub segments: u32,
    /// Their speech.
    pub speech_ms: u64,
}

/// `GET /api/v1/recordings/{id}/audio` (§19.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordingAudio {
    /// The audio's state.
    pub state: AudioState,
    /// When it expires.
    pub expires_at: Option<Timestamp>,
    /// `off` (P3) or `client_unseal` (P1).
    pub playback: Playback,
    /// The mixed parts.
    pub mixed: Vec<MixedPart>,
    /// Per-speaker tracks.
    pub tracks: Vec<AudioTrack>,
}

/// One segment of a track page (§19.2; keys and URLs only with P1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioSegment {
    /// The segment.
    pub segment_id: Id,
    /// Offset from the recording start.
    pub start_ms: u64,
    /// Length.
    pub duration_ms: u32,
    /// Sealed size.
    pub bytes: u32,
    /// The ciphertext URL (P1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The segment key, base64 (P1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// [`SEALED_AUDIO_FORMAT`] (P1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// `POST /api/v1/compliance/recordings/{id}/downloads` body (§19.4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DownloadRequest {
    /// What to download (default `both`).
    #[serde(default)]
    pub what: DownloadWhat,
    /// Why (≤ 500 bytes; audited only as `reason_given`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Validate for DownloadRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self
            .reason
            .as_ref()
            .is_some_and(|r| r.len() > DOWNLOAD_REASON_MAX)
        {
            return Err(ValidationError::invalid("reason", "at most 500 bytes"));
        }
        Ok(())
    }
}

/// A download's speaker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DownloadSpeaker {
    /// The member.
    pub member_id: Id,
    /// Their display name.
    pub display_name: String,
}

/// A compliance download grant (§19.4): sealed files and the keys to open them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordingDownload {
    /// The grant.
    pub download_id: Id,
    /// The recording.
    pub recording_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// When it started.
    pub started_at: Timestamp,
    /// When it stopped.
    pub stopped_at: Option<Timestamp>,
    /// The audio's state.
    pub audio_state: AudioState,
    /// [`SEALED_AUDIO_FORMAT`].
    pub format: String,
    /// When the grant and its URLs expire.
    pub expires_at: Timestamp,
    /// The speakers.
    pub speakers: Vec<DownloadSpeaker>,
    /// The mixed parts with URLs and keys (when asked for).
    pub mixed: Vec<MixedPart>,
    /// The segment pages (when segments were asked for).
    pub segments_url: Option<String>,
}

/// One segment of a compliance download (§19.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DownloadSegment {
    /// The segment.
    pub segment_id: Id,
    /// The speaker.
    pub member_id: Id,
    /// Offset from the recording start.
    pub start_ms: u64,
    /// Length.
    pub duration_ms: u32,
    /// Sealed size.
    pub bytes: u32,
    /// The viewer-bound ciphertext URL.
    pub url: String,
    /// The segment key, base64.
    pub key: String,
}

/// `recording_updated` (§18.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordingUpdated {
    /// The recording (with the receiver's own `my_consent` / `consent_prompt`).
    pub recording: Recording,
}

/// `status.recording` (§22.3).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StatusRecording {
    /// `[recording] enabled`.
    pub enabled: bool,
    /// The recorder answers health.
    pub recorder_ok: bool,
    /// The transcriber answers health.
    pub transcriber_ok: bool,
    /// The model the transcriber reports.
    pub model: Option<String>,
    /// Its backend.
    pub backend: Option<String>,
    /// Its workers.
    pub workers: u32,
    /// Active recordings.
    pub active: u32,
    /// Allowlisted tracks install-wide.
    pub tracks: u32,
    /// Rejoins in the last hour.
    pub rejoins_1h: u32,
    /// Queued speech awaiting transcription.
    pub backlog_audio_s: u64,
    /// Age of the oldest stored, untranscribed segment.
    pub oldest_pending_s: u64,
    /// Realtime factor over the last hour.
    pub rtf_1h: Option<f64>,
    /// Stored audio bytes.
    pub audio_bytes: u64,
    /// Held audio bytes.
    pub held_audio_bytes: u64,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn recording_ref_round_trip() {
        let a: RecordingRef = serde_json::from_str("\"auto\"").unwrap();
        assert_eq!(a, RecordingRef::Auto);
        let id = Id::new(48_831_765_147_549_701).unwrap();
        let s = serde_json::to_string(&RecordingRef::Id(id)).unwrap();
        assert_eq!(s, "\"48831765147549701\"");
        let b: RecordingRef = serde_json::from_str(&s).unwrap();
        assert_eq!(b, RecordingRef::Id(id));
        assert!(serde_json::from_str::<RecordingRef>("\"nope\"").is_err());
    }

    #[test]
    fn consent_maps_to_s50_states() {
        assert_eq!(Consent::Granted.to_state(), ConsentState::Granted);
        assert_eq!(Consent::Pending.to_state(), ConsentState::Pending);
        assert_eq!(Consent::Declined.to_state(), ConsentState::Denied);
        assert_eq!(Consent::Withdrawn.to_state(), ConsentState::Denied);
    }

    #[test]
    fn notice_without_consent_invalid() {
        let n: RecordingNotice = serde_json::from_str(r#"{"recording_id":"auto"}"#).unwrap();
        assert!(n.validate().is_err());
    }

    #[test]
    fn notice_version_changes_with_every_slot() {
        let id = Id::new(48_831_765_147_549_701).unwrap();
        let p = ConsentPrompt {
            recording_id: RecordingRef::Id(id),
            notice_version: 0,
            recorder_name: RECORDER_DISPLAY_NAME.to_owned(),
            feature_label: RECORDING_FEATURE_LABEL.to_owned(),
            started_by: None,
            room: PromptRoom {
                channel_id: id,
                kind: PromptRoomKind::Voice,
                name: Some("standup".to_owned()),
            },
            audio_keep_days: 30,
            transcript: PromptTranscript::default(),
            admin_download: true,
            decline_mode: DeclineMode::JoinUnrecorded,
            recorded_now: vec![],
        }
        .sealed();
        let mut q = p.clone();
        q.audio_keep_days = 7;
        assert_ne!(p.notice_version, q.compute_version());
        let mut r = p.clone();
        r.decline_mode = DeclineMode::NotJoin;
        assert_ne!(p.notice_version, r.compute_version());
        assert_eq!(p.notice_version, p.compute_version());
    }

    #[test]
    fn languages() {
        assert!(language_ok("auto") && language_ok("de") && language_ok("haw"));
        assert!(!language_ok("xx"));
    }
}
