//! Examples for the wire types S35 v2 adds (prd-sew-35 v2 §23): recordings, consent, the
//! consent modal, the record message's summary, transcript turns, the audio listing, compliance
//! downloads, `recording_updated` and `status.recording`.

use super::{ID_BOB, ID_CAROL, ID_CHANNEL, ID_DM, ID_ME, ID_MESSAGE, ID_SPACE, id, t};
use crate::id::Id;
use crate::recording::*;
use crate::registry::Example;

/// The example recording.
pub(super) const ID_RECORDING: Id = id(506);
/// An example segment.
const ID_SEGMENT: Id = id(507);
/// An example compliance download.
const ID_DOWNLOAD: Id = id(508);
/// An example DM-call record.
const ID_CALL_RECORD: Id = id(340);

const KEY: &str = "q83vEjRWeJCrze8SNFZ4kKvN7xI0VniQq83vEjRWeJA=";

fn participants() -> Vec<RecordingParticipant> {
    vec![
        RecordingParticipant {
            member_id: ID_ME,
            consent: Consent::Granted,
            capture: Some(Capture::Recorded),
            reason: None,
        },
        RecordingParticipant {
            member_id: ID_BOB,
            consent: Consent::Granted,
            capture: Some(Capture::NotRecorded),
            reason: Some(NotRecordedReason::NoKey),
        },
        RecordingParticipant {
            member_id: ID_CAROL,
            consent: Consent::Pending,
            capture: None,
            reason: None,
        },
    ]
}

/// The consent modal as Carol sees it (every slot set).
pub(super) fn consent_prompt_full() -> ConsentPrompt {
    ConsentPrompt {
        recording_id: RecordingRef::Id(ID_RECORDING),
        notice_version: 0,
        recorder_name: RECORDER_DISPLAY_NAME.into(),
        feature_label: RECORDING_FEATURE_LABEL.into(),
        started_by: Some(PromptMember {
            member_id: ID_ME,
            display_name: "Rosson".into(),
        }),
        room: PromptRoom {
            channel_id: ID_CHANNEL,
            kind: PromptRoomKind::Voice,
            name: Some("standup".into()),
        },
        audio_keep_days: 30,
        transcript: PromptTranscript::default(),
        admin_download: true,
        decline_mode: DeclineMode::JoinUnrecorded,
        recorded_now: vec![ID_ME],
    }
    .sealed()
}

/// An auto-record channel with no recording yet.
fn consent_prompt_auto() -> ConsentPrompt {
    ConsentPrompt {
        recording_id: RecordingRef::Auto,
        notice_version: 0,
        recorder_name: RECORDER_DISPLAY_NAME.into(),
        feature_label: RECORDING_FEATURE_LABEL.into(),
        started_by: None,
        room: PromptRoom {
            channel_id: ID_CHANNEL,
            kind: PromptRoomKind::Voice,
            name: None,
        },
        audio_keep_days: 0,
        transcript: PromptTranscript::default(),
        admin_download: true,
        decline_mode: DeclineMode::NotJoin,
        recorded_now: vec![],
    }
    .sealed()
}

/// A live recording as Carol (still `pending`) sees it: every field set.
pub(super) fn recording_full() -> Recording {
    Recording {
        id: ID_RECORDING,
        channel_id: ID_CHANNEL,
        call_record_id: Some(ID_CALL_RECORD),
        space_id: Some(ID_SPACE),
        state: RecordingState::Recording,
        started_by: ID_ME,
        started_at: t(0),
        auto: false,
        stopped_at: Some(t(3_600_000)),
        stop_reason: Some(StopReason::Stopped),
        message_id: ID_MESSAGE,
        mixed: true,
        audio_keep_days: 30,
        language: Some("en".into()),
        max_duration_ms: 14_400_000,
        participants: participants(),
        my_consent: Some(Consent::Pending),
        consent_prompt: Some(consent_prompt_full()),
    }
}

/// A recording seen by a non-participant: the required fields only.
pub(super) fn recording_min() -> Recording {
    Recording {
        id: ID_RECORDING,
        channel_id: ID_DM,
        call_record_id: None,
        space_id: None,
        state: RecordingState::Starting,
        started_by: ID_ME,
        started_at: t(0),
        auto: true,
        stopped_at: None,
        stop_reason: None,
        message_id: ID_MESSAGE,
        mixed: false,
        audio_keep_days: 0,
        language: None,
        max_duration_ms: 14_400_000,
        participants: vec![],
        my_consent: None,
        consent_prompt: None,
    }
}

/// `Message.recording` of a final recording.
pub(super) fn recording_summary_full() -> RecordingSummary {
    RecordingSummary {
        recording_id: ID_RECORDING,
        started_by: ID_ME,
        started_at: t(0),
        auto: false,
        stopped_at: Some(t(3_600_000)),
        stop_reason: Some(StopReason::Stopped),
        duration_ms: Some(3_600_000),
        speaker_ids: vec![ID_ME, ID_BOB],
        audio: SummaryAudio {
            state: AudioState::Available,
            expires_at: Some(t(30 * 86_400_000)),
            mixed: true,
            keep_days: 30,
        },
        transcript: SummaryTranscript {
            state: TranscriptState::Partial,
            turns: 42,
            gaps_ms: 1_500,
            lang: Some("en".into()),
        },
        e2ee: true,
    }
}

fn recording_summary_running() -> RecordingSummary {
    RecordingSummary {
        recording_id: ID_RECORDING,
        started_by: ID_ME,
        started_at: t(0),
        auto: true,
        stopped_at: None,
        stop_reason: None,
        duration_ms: None,
        speaker_ids: vec![],
        audio: SummaryAudio {
            state: AudioState::Available,
            expires_at: None,
            mixed: false,
            keep_days: 0,
        },
        transcript: SummaryTranscript {
            state: TranscriptState::Running,
            turns: 0,
            gaps_ms: 0,
            lang: None,
        },
        e2ee: true,
    }
}

/// `Message.transcript` of a turn by Bob.
pub(super) fn transcript_turn_full() -> TranscriptTurn {
    TranscriptTurn {
        recording_id: ID_RECORDING,
        speaker_id: ID_BOB,
        start_ms: 3_000,
        end_ms: 6_400,
        spoken_at: t(3_000),
        lang: Some("en".into()),
        pieces: vec![
            TurnPiece {
                start_ms: 3_000,
                end_ms: 4_200,
                offset: 0,
                len: 11,
            },
            TurnPiece {
                start_ms: 4_400,
                end_ms: 6_400,
                offset: 12,
                len: 19,
            },
        ],
    }
}

fn mixed_part_full() -> MixedPart {
    MixedPart {
        part: 1,
        offset_ms: 0,
        duration_ms: 3_600_000,
        bytes: 14_400_000,
        url: Some(
            "https://chat.acme.test/api/v1/recordings/48831765147550202/audio/files/\
             48831765147550205?exp=1790000000&sig=AAAA"
                .into(),
        ),
        key: Some(KEY.into()),
        format: Some(SEALED_AUDIO_FORMAT.into()),
    }
}

fn mixed_part_min() -> MixedPart {
    MixedPart {
        part: 2,
        offset_ms: 3_600_000,
        duration_ms: 600_000,
        bytes: 2_400_000,
        url: None,
        key: None,
        format: None,
    }
}

/// Carol's "Join and allow recording".
pub(super) fn recording_notice_full() -> RecordingNotice {
    RecordingNotice {
        recording_id: RecordingRef::Id(ID_RECORDING),
        consent: Some(true),
        notice_version: Some(consent_prompt_full().notice_version),
    }
}

impl Example for RecordingNotice {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", recording_notice_full()),
            (
                "auto_decline",
                RecordingNotice {
                    recording_id: RecordingRef::Auto,
                    consent: Some(false),
                    notice_version: Some(consent_prompt_auto().notice_version),
                },
            ),
        ]
    }
}

impl Example for ConsentPrompt {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", consent_prompt_full()),
            ("auto", consent_prompt_auto()),
        ]
    }
}

impl Example for Recording {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", recording_full()), ("min", recording_min())]
    }
}

impl Example for RecordingUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            RecordingUpdated {
                recording: recording_full(),
            },
        )]
    }
}

impl Example for StartRecordingRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                StartRecordingRequest {
                    audio_keep_days: Some(14),
                    mixed: Some(false),
                    language: Some("de".into()),
                },
            ),
            ("min", StartRecordingRequest::default()),
        ]
    }
}

impl Example for ConsentRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ConsentRequest {
                    consent: true,
                    notice_version: Some(consent_prompt_full().notice_version),
                },
            ),
            (
                "min",
                ConsentRequest {
                    consent: false,
                    notice_version: None,
                },
            ),
        ]
    }
}

impl Example for AudioRetentionRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", AudioRetentionRequest { audio_keep_days: 7 })]
    }
}

impl Example for ActiveRecording {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ActiveRecording {
                    recording: Some(recording_full()),
                },
            ),
            ("none", ActiveRecording { recording: None }),
        ]
    }
}

impl Example for RecordingSummary {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", recording_summary_full()),
            ("running", recording_summary_running()),
        ]
    }
}

impl Example for TranscriptTurn {
    fn examples() -> Vec<(&'static str, Self)> {
        let min = TranscriptTurn {
            lang: None,
            pieces: vec![],
            ..transcript_turn_full()
        };
        vec![("full", transcript_turn_full()), ("min", min)]
    }
}

impl Example for RecordingAudio {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                RecordingAudio {
                    state: AudioState::Available,
                    expires_at: Some(t(30 * 86_400_000)),
                    playback: Playback::ClientUnseal,
                    mixed: vec![mixed_part_full(), mixed_part_min()],
                    tracks: vec![
                        AudioTrack {
                            member_id: ID_ME,
                            segments: 12,
                            speech_ms: 540_000,
                        },
                        AudioTrack {
                            member_id: ID_BOB,
                            segments: 7,
                            speech_ms: 310_000,
                        },
                    ],
                },
            ),
            (
                "min",
                RecordingAudio {
                    state: AudioState::Purged,
                    expires_at: None,
                    playback: Playback::Off,
                    mixed: vec![],
                    tracks: vec![],
                },
            ),
        ]
    }
}

impl Example for AudioSegment {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = AudioSegment {
            segment_id: ID_SEGMENT,
            start_ms: 3_000,
            duration_ms: 61_200,
            bytes: 241_888,
            url: Some(
                "https://chat.acme.test/api/v1/recordings/48831765147550202/audio/files/\
                 48831765147550203?exp=1790000000&sig=AAAA"
                    .into(),
            ),
            key: Some(KEY.into()),
            format: Some(SEALED_AUDIO_FORMAT.into()),
        };
        let min = AudioSegment {
            url: None,
            key: None,
            format: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for DownloadRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                DownloadRequest {
                    what: DownloadWhat::Segments,
                    reason: Some("legal hold LH-2026-14".into()),
                },
            ),
            ("min", DownloadRequest::default()),
        ]
    }
}

impl Example for RecordingDownload {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = RecordingDownload {
            download_id: ID_DOWNLOAD,
            recording_id: ID_RECORDING,
            channel_id: ID_CHANNEL,
            started_at: t(0),
            stopped_at: Some(t(3_600_000)),
            audio_state: AudioState::Expired,
            format: SEALED_AUDIO_FORMAT.into(),
            expires_at: t(3_700_000),
            speakers: vec![DownloadSpeaker {
                member_id: ID_ME,
                display_name: "Rosson".into(),
            }],
            mixed: vec![mixed_part_full()],
            segments_url: Some(
                "/api/v1/compliance/recordings/48831765147550202/downloads/48831765147550204/\
                 segments"
                    .into(),
            ),
        };
        let min = RecordingDownload {
            stopped_at: None,
            speakers: vec![],
            mixed: vec![],
            segments_url: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for DownloadSegment {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            DownloadSegment {
                segment_id: ID_SEGMENT,
                member_id: ID_BOB,
                start_ms: 3_000,
                duration_ms: 61_200,
                bytes: 241_888,
                url: "https://chat.acme.test/api/v1/compliance/recordings/48831765147550202/\
                      files/48831765147550203?d=48831765147550204&exp=1790000000&sig=AAAA"
                    .into(),
                key: KEY.into(),
            },
        )]
    }
}

/// `status.recording` with every field set.
pub(super) fn status_recording_full() -> StatusRecording {
    StatusRecording {
        enabled: true,
        recorder_ok: true,
        transcriber_ok: false,
        model: Some("large-v3-turbo-q5_0".into()),
        backend: Some("whisper-cpp".into()),
        workers: 2,
        active: 1,
        tracks: 3,
        rejoins_1h: 0,
        backlog_audio_s: 420,
        oldest_pending_s: 95,
        rtf_1h: Some(4.5),
        audio_bytes: 22_000_000,
        held_audio_bytes: 0,
    }
}

impl Example for StatusRecording {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", status_recording_full()),
            ("min", StatusRecording::default()),
        ]
    }
}
