//! Examples for the wire types S33 v2 adds (prd-sew-33 v2 §19; v1 §18).

use super::{ID_BOB, ID_CATEGORY, ID_CHANNEL, ID_ME, ID_OTHER_CHANNEL, ID_SPACE, slug, t};
use crate::e2ee::JoinIdHex;
use crate::http::Patch;
use crate::perms;
use crate::registry::Example;
use crate::voice::*;

const TOKEN: &str = "eyJhbGciOiJFUzI1NiIsImtpZCI6IjlhMWYzMzA0In0.eyJzdWIiOiJwX2xtaGEifQ.c2ln";
const VOICE_URL: &str = "wss://voice.chat.example.com";

pub(super) fn voice_state_full() -> VoiceState {
    VoiceState {
        member_id: ID_ME,
        space_id: Some(ID_SPACE),
        channel_id: ID_CHANNEL,
        connection: VoiceConnection::Connected,
        secure: true,
        self_mute: true,
        self_deaf: false,
        server_mute: false,
        server_deaf: false,
        speak_denied: false,
        joined_at: t(0),
        call_id: None,
        self_video: true,
        self_stream: true,
    }
}

fn voice_state_min() -> VoiceState {
    VoiceState {
        space_id: None,
        connection: VoiceConnection::Connecting,
        secure: false,
        self_mute: false,
        self_video: false,
        self_stream: false,
        ..voice_state_full()
    }
}

/// A DM call entry (S34 v2 §20.1): no space, `call_id` set.
fn voice_state_call() -> VoiceState {
    VoiceState {
        space_id: None,
        channel_id: super::ID_DM,
        call_id: Some(super::s34::ID_CALL),
        self_stream: false,
        ..voice_state_full()
    }
}

impl Example for VoiceState {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", voice_state_full()),
            ("full_call", voice_state_call()),
            ("min", voice_state_min()),
        ]
    }
}

impl Example for VoiceJoinRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                VoiceJoinRequest {
                    self_mute: true,
                    self_deaf: false,
                    self_video: true,
                    e2ee: Some(super::s50::e2ee_join_full()),
                    recording_notice: Some(super::s35::recording_notice_full()),
                },
            ),
            ("min", VoiceJoinRequest::default()),
        ]
    }
}

impl Example for VoiceJoinResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = VoiceJoinResponse {
            url: VOICE_URL.into(),
            token: TOKEN.into(),
            expires_at: t(120_000),
            room: super::s50::call_id(),
            state: voice_state_min(),
            e2ee: Some(super::s50::init_full()),
            media: Some(super::s34::media_policy_full()),
            call_id: Some(super::s34::ID_CALL),
            recording: Some(super::s35::recording_full()),
            stats: Some(CallStatsBeacon {
                interval_ms: CALL_STATS_INTERVAL_MS,
            }),
        };
        vec![
            ("full", full.clone()),
            (
                "min",
                VoiceJoinResponse {
                    e2ee: None,
                    media: None,
                    call_id: None,
                    stats: None,
                    ..full
                },
            ),
        ]
    }
}

impl Example for CallStatsBeacon {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            CallStatsBeacon {
                interval_ms: CALL_STATS_INTERVAL_MS,
            },
        )]
    }
}

impl Example for CallStatsSample {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = CallStatsSample {
            interval_ms: 15_000,
            client: CallStatsClient::Chromium,
            frames_decoded: 450,
            frames_dropped: 3,
            freezes: 1,
            freeze_ms: 420,
            decrypt_failures: 0,
            candidate: Some(CallStatsCandidate::Relay),
            relay_protocol: Some(CallStatsRelayProtocol::Tls),
            rtt_ms: Some(48),
        };
        vec![
            ("full", full.clone()),
            (
                "audio_only",
                CallStatsSample {
                    client: CallStatsClient::Firefox,
                    frames_decoded: 0,
                    frames_dropped: 0,
                    freezes: 0,
                    freeze_ms: 0,
                    candidate: Some(CallStatsCandidate::Host),
                    relay_protocol: None,
                    rtt_ms: None,
                    ..full
                },
            ),
            (
                "min",
                CallStatsSample {
                    client: CallStatsClient::Other,
                    frames_decoded: 0,
                    frames_dropped: 0,
                    freezes: 0,
                    freeze_ms: 0,
                    candidate: None,
                    relay_protocol: None,
                    rtt_ms: None,
                    ..full
                },
            ),
        ]
    }
}

impl Example for VoiceSelfStateRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                VoiceSelfStateRequest {
                    self_mute: Some(true),
                    self_deaf: Some(true),
                    self_video: Some(true),
                    self_stream: Some(false),
                },
            ),
            (
                "mute",
                VoiceSelfStateRequest {
                    self_mute: Some(false),
                    self_deaf: None,
                    self_video: None,
                    self_stream: None,
                },
            ),
            (
                "camera",
                VoiceSelfStateRequest {
                    self_mute: None,
                    self_deaf: None,
                    self_video: Some(true),
                    self_stream: None,
                },
            ),
        ]
    }
}

impl Example for VoiceFlagsRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                VoiceFlagsRequest {
                    server_mute: Some(true),
                    server_deaf: Some(false),
                },
            ),
            (
                "deafen",
                VoiceFlagsRequest {
                    server_mute: None,
                    server_deaf: Some(true),
                },
            ),
        ]
    }
}

impl Example for VoiceFlags {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            VoiceFlags {
                space_id: ID_SPACE,
                member_id: ID_BOB,
                server_mute: true,
                server_deaf: false,
            },
        )]
    }
}

impl Example for VoiceMoveRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                VoiceMoveRequest {
                    channel_id: Patch::Set(ID_OTHER_CHANNEL),
                },
            ),
            (
                "disconnect",
                VoiceMoveRequest {
                    channel_id: Patch::Clear,
                },
            ),
        ]
    }
}

impl Example for VoiceStateUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                VoiceStateUpdated {
                    member_id: ID_ME,
                    space_id: Some(ID_SPACE),
                    channel_id: Some(ID_CHANNEL),
                    previous_channel_id: Some(ID_OTHER_CHANNEL),
                    state: Some(voice_state_full()),
                    reason: Some(VoiceLeaveReason::Moved),
                },
            ),
            (
                "left",
                VoiceStateUpdated {
                    member_id: ID_ME,
                    space_id: Some(ID_SPACE),
                    channel_id: None,
                    previous_channel_id: Some(ID_CHANNEL),
                    state: None,
                    reason: None,
                },
            ),
        ]
    }
}

fn move_key_join() -> VoiceMoveKeyJoin {
    let join_id: JoinIdHex = match "0123456789abcdef0123456789abcdef".parse() {
        Ok(j) => j,
        Err(e) => panic!("example join id: {e}"),
    };
    VoiceMoveKeyJoin {
        call_id: super::s50::call_id(),
        join_id,
        key_join: KeyJoinMode::Required,
    }
}

impl Example for VoiceMoveKeyJoin {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", move_key_join())]
    }
}

impl Example for VoiceServerUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            VoiceServerUpdated {
                channel_id: ID_OTHER_CHANNEL,
                room: super::s50::call_id(),
                url: VOICE_URL.into(),
                token: TOKEN.into(),
                expires_at: t(120_000),
                e2ee: move_key_join(),
                reason: VoiceLeaveReason::Moved,
                media: Some(super::s34::media_policy_full()),
                call_id: Some(super::s34::ID_CALL),
                recording: Some(super::s35::recording_full()),
            },
        )]
    }
}

pub(super) fn ready_voice_channel_full() -> ReadyVoiceChannel {
    ReadyVoiceChannel {
        id: ID_OTHER_CHANNEL,
        space_id: ID_SPACE,
        category_id: Some(ID_CATEGORY),
        slug: slug("lounge"),
        name: "lounge".into(),
        position: 2,
        user_limit: 25,
        my_perms: perms::DEFAULT_EVERYONE,
    }
}

impl Example for ReadyVoiceChannel {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", ready_voice_channel_full()),
            (
                "min",
                ReadyVoiceChannel {
                    category_id: None,
                    user_limit: 0,
                    ..ready_voice_channel_full()
                },
            ),
        ]
    }
}

impl Example for MeVoice {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MeVoice {
                    state: Some(voice_state_full()),
                },
            ),
            ("min", MeVoice { state: None }),
        ]
    }
}

impl Example for VoiceStatePage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            VoiceStatePage {
                items: vec![voice_state_full(), voice_state_min()],
                has_more: false,
            },
        )]
    }
}

fn engine_full() -> VoiceEngineStatus {
    VoiceEngineStatus {
        state: VoiceEngineState::Down,
        version: Some("0.1.0".into()),
        rooms: 3,
        participants: 11,
        panics_1h: 1,
        down_since: Some(t(60_000)),
        last_event_at: Some(t(59_000)),
        last_snapshot_at: Some(t(30_000)),
        drift_fixes_1h: 2,
    }
}

impl Example for VoiceEngineStatus {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", engine_full()),
            (
                "min",
                VoiceEngineStatus {
                    state: VoiceEngineState::Up,
                    version: None,
                    rooms: 0,
                    participants: 0,
                    panics_1h: 0,
                    down_since: None,
                    last_event_at: None,
                    last_snapshot_at: None,
                    drift_fixes_1h: 0,
                },
            ),
        ]
    }
}

pub(super) fn voice_status_full() -> VoiceStatus {
    VoiceStatus {
        enabled: true,
        public_url: Some(VOICE_URL.into()),
        calls: 3,
        participants: 11,
        secure_participants: 10,
        reconnecting: 1,
        engine: engine_full(),
        e2ee: Some(super::s50::status_e2ee_full()),
        media: Some(super::s34::voice_media_status_full()),
    }
}

impl Example for VoiceStatus {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", voice_status_full()),
            (
                "min",
                VoiceStatus {
                    public_url: None,
                    e2ee: None,
                    media: None,
                    ..voice_status_full()
                },
            ),
        ]
    }
}
