//! Examples for the wire types S34 v2 adds (prd-sew-34 v2 §24): DM calls, rings, the call
//! summary, the client media policy and S34's part of `status.voice`.

use super::{ID_BOB, ID_CAROL, ID_DM, ID_ME, ID_MESSAGE, id, t};
use crate::calls::*;
use crate::id::Id;
use crate::registry::Example;
use crate::voice::*;

/// The example call (`calls.id`).
pub(super) const ID_CALL: Id = id(340);

fn layers(top: u32, kbps: [u32; 3]) -> Vec<LayerHint> {
    let rids = ["q", "h", "f"];
    let heights = [top / 4, top / 2, top];
    rids.iter()
        .zip(heights)
        .zip(kbps)
        .map(|((rid, height), max_kbps)| LayerHint {
            rid: (*rid).into(),
            height,
            max_kbps,
        })
        .collect()
}

/// A `normal`-tier policy with VP9 configured (every field set).
pub(super) fn media_policy_full() -> MediaPolicy {
    MediaPolicy {
        tier: MediaTier::Normal,
        video_enabled: true,
        e2ee: "required".into(),
        audio_codecs: vec!["opus".into()],
        codecs: vec![VideoCodec::Vp8, VideoCodec::H264, VideoCodec::Vp9],
        backup_codec: false,
        vp9: Some(Vp9Hint {
            scalability_mode: "L3T3_KEY".into(),
            requires_dd: true,
        }),
        camera: TrackHint {
            max_width: 1280,
            max_height: 720,
            max_fps: 30,
            layers: layers(720, [150, 500, 1_500]),
        },
        screen: TrackHint {
            max_width: 1920,
            max_height: 1080,
            max_fps: 15,
            layers: layers(1080, [200, 800, 2_500]),
        },
        dynacast: true,
        adaptive_stream: true,
    }
}

/// Video off: no codecs, no layers.
fn media_policy_video_off() -> MediaPolicy {
    let none = TrackHint {
        max_width: 0,
        max_height: 0,
        max_fps: 0,
        layers: vec![],
    };
    MediaPolicy {
        tier: MediaTier::Critical,
        video_enabled: false,
        codecs: vec![],
        vp9: None,
        camera: none.clone(),
        screen: none,
        ..media_policy_full()
    }
}

impl Example for MediaPolicy {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", media_policy_full()),
            ("video_off", media_policy_video_off()),
        ]
    }
}

/// An active group-DM call: alice started it, bob joined, carol is ringing.
pub(super) fn call_full() -> Call {
    Call {
        id: ID_CALL,
        channel_id: ID_DM,
        message_id: ID_MESSAGE,
        started_by: ID_ME,
        started_at: t(0),
        state: CallState::Active,
        ended_at: None,
        end_reason: None,
        participant_ids: vec![ID_ME, ID_BOB],
        rings: vec![
            CallRing {
                member_id: ID_BOB,
                state: RingState::Accepted,
                expires_at: None,
            },
            CallRing {
                member_id: ID_CAROL,
                state: RingState::Ringing,
                expires_at: Some(t(45_000)),
            },
        ],
        my_ring: Some(MyRing {
            silent: true,
            reason: Some(ring_reason::DND.into()),
        }),
    }
}

/// An ended 1:1 call that bob declined.
fn call_ended() -> Call {
    Call {
        state: CallState::Ended,
        ended_at: Some(t(5_000)),
        end_reason: Some(CallEndReason::Declined),
        participant_ids: vec![],
        rings: vec![CallRing {
            member_id: ID_BOB,
            state: RingState::Declined,
            expires_at: None,
        }],
        my_ring: None,
        ..call_full()
    }
}

impl Example for Call {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", call_full()), ("full_ended", call_ended())]
    }
}

/// An ended call's summary (every field set).
pub(super) fn call_summary_full() -> CallSummary {
    CallSummary {
        call_id: ID_CALL,
        started_by: ID_ME,
        started_at: t(0),
        ended_at: Some(t(754_000)),
        duration_ms: Some(754_000),
        end_reason: Some(CallEndReason::Empty),
        joined_ids: vec![ID_ME, ID_BOB],
        rung_ids: vec![ID_BOB, ID_CAROL],
        declined_ids: vec![],
        missed_ids: vec![ID_CAROL],
    }
}

impl Example for CallSummary {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", call_summary_full()),
            (
                "active",
                CallSummary {
                    ended_at: None,
                    duration_ms: None,
                    end_reason: None,
                    joined_ids: vec![ID_ME],
                    rung_ids: vec![ID_BOB],
                    declined_ids: vec![],
                    missed_ids: vec![],
                    ..call_summary_full()
                },
            ),
        ]
    }
}

impl Example for CallJoinRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CallJoinRequest {
                    ring: false,
                    self_mute: true,
                    self_deaf: false,
                    self_video: true,
                    e2ee: Some(super::s50::e2ee_join_full()),
                    recording_notice: Some(super::s35::recording_notice_full()),
                },
            ),
            ("min", CallJoinRequest::default()),
        ]
    }
}

impl Example for CallJoinResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        let voice = match VoiceJoinResponse::examples().into_iter().next() {
            Some((_, v)) => VoiceJoinResponse {
                call_id: Some(ID_CALL),
                ..v
            },
            None => panic!("VoiceJoinResponse has no example"),
        };
        vec![(
            "full",
            CallJoinResponse {
                call: call_full(),
                created: true,
                voice,
                recording: Some(super::s35::recording_full()),
            },
        )]
    }
}

impl Example for CallDeclineRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", CallDeclineRequest { call_id: ID_CALL })]
    }
}

impl Example for CallRingRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CallRingRequest {
                    call_id: ID_CALL,
                    member_ids: Some(vec![ID_CAROL]),
                },
            ),
            (
                "min",
                CallRingRequest {
                    call_id: ID_CALL,
                    member_ids: None,
                },
            ),
        ]
    }
}

impl Example for ChannelCall {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ChannelCall {
                    call: Some(call_full()),
                },
            ),
            ("none", ChannelCall { call: None }),
        ]
    }
}

impl Example for MyCalls {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MyCalls {
                    items: vec![call_full()],
                },
            ),
            ("empty", MyCalls { items: vec![] }),
        ]
    }
}

impl Example for CallUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", CallUpdated { call: call_full() }),
            ("ended", CallUpdated { call: call_ended() }),
        ]
    }
}

impl Example for CallRingEvent {
    fn examples() -> Vec<(&'static str, Self)> {
        let ring = CallRingEvent {
            call_id: ID_CALL,
            channel_id: ID_DM,
            started_by: ID_ME,
            ringing: true,
            silent: true,
            reason: Some(ring_reason::BUSY.into()),
            expires_at: Some(t(45_000)),
        };
        vec![
            ("full", ring.clone()),
            (
                "loud",
                CallRingEvent {
                    silent: false,
                    reason: None,
                    ..ring.clone()
                },
            ),
            (
                "stop",
                CallRingEvent {
                    ringing: false,
                    silent: false,
                    reason: Some(ring_reason::ACCEPTED.into()),
                    expires_at: None,
                    ..ring
                },
            ),
        ]
    }
}

/// S34's part of `status.voice` (every field set).
pub(super) fn voice_media_status_full() -> VoiceMediaStatus {
    VoiceMediaStatus {
        calls_active: 2,
        video_publishers: 7,
        screen_publishers: 1,
        media_tier: MediaTier::Normal,
        budget_source: BudgetSource::Engine,
        egress_mbps: 41.2,
        egress_budget_mbps: 300,
        forward_estimate_mbps: 38.0,
        engine_cpu_pct: Some(12.5),
        video_enabled: true,
    }
}

impl Example for VoiceMediaStatus {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", voice_media_status_full()),
            (
                "estimate",
                VoiceMediaStatus {
                    media_tier: MediaTier::Critical,
                    budget_source: BudgetSource::Estimate,
                    engine_cpu_pct: None,
                    ..voice_media_status_full()
                },
            ),
        ]
    }
}
