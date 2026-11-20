//! K2's side of native voice against Sew's contract and the plugin's fake engine.
//! Real devices aren't here: `mic_denied` and `device_lost` come from the fake.
//! The real ones are N3's per-OS calls.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use tauri_plugin_sew_voice::fake::{FakeEngine, RecordingSink};
use tauri_plugin_sew_voice::wire::{consent, video_code, RecordingInfo, EVENT_NAMES, EVENT_PREFIX};
use tauri_plugin_sew_voice::{
    CallEvent, CallState, CameraArgs, DeviceList, EngineError, ErrorInfo, JoinArgs, JoinProvider, JoinRequest, JoinResult, LocalVideo, LocalVideoState, MuteArgs,
    Participant, PermissionState, PreviewAnswer, PreviewArgs, ProviderError, RejoinReason, ScreenArgs, ScreenSource, SetDeviceArgs, SetVideoDeviceArgs, SinkIdArgs,
    SinkMode, Spawn, Speaking, StateEvent, StateSnapshot, VideoCaps, VideoDevices, VideoSinkAnswer, VideoSinkArgs, VideoSinkUpdateArgs, VideoTrack, VoiceService,
    COMMANDS, DEFAULT_DEVICE, PLUGIN_NAME, VIDEO_COMMANDS,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::provider::{answer_from, api_base, find_host, host_base_url, join_body, K2Provider, Login, LoginSource, SessionLeave, LOCAL_TARGET};

const LOGIN_TOKEN: &str = "k2-login-token-for-tests";
const MEDIA_TOKEN: &str = "media-token-for-tests";
const CALL_CRED: &str = "sewcc_sel.secret-for-tests";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("repo root").to_path_buf()
}

/// A file of one of Sew's crates as published on crates.io (since Sew 0.10.4
/// every one of them is, the voice SDK included), at the version Cargo.lock pins.
fn sew_crate_file(krate: &str, rel: &str) -> Vec<u8> {
    registry_crate_file(krate, rel)
}

fn fixture() -> Value {
    let path = repo_root().join("src/renderer/lib/voice/native-voice-contract.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("contract file is JSON")
}

/// `want` parses to a `T` that serializes back to exactly `want`.
fn exact<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(want: &Value) -> T {
    let v: T = serde_json::from_value(want.clone()).unwrap_or_else(|e| panic!("{want} doesn't parse: {e}"));
    assert_eq!(&serde_json::to_value(&v).expect("serialize"), want, "round trip of {want}");
    v
}

/// The file the TS adapter is tested against matches the plugin's real types.
#[test]
fn the_renderers_contract_file_matches_the_plugin() {
    let f = fixture();
    assert_eq!(f["plugin"], PLUGIN_NAME);
    assert_eq!(f["eventPrefix"], EVENT_PREFIX);
    // The six of voice come first; video's four follow (the video contract).
    assert_eq!(f["events"], json!(EVENT_NAMES[..6]));
    assert_eq!(f["commands"], json!(COMMANDS));
    assert_eq!(DEFAULT_DEVICE, "default", "the renderer sends this for System default");
    assert_eq!(exact::<CallState>(&f["states"]["idle"]), CallState::Idle);
    assert_eq!(exact::<CallState>(&f["states"]["connecting"]), CallState::Connecting);
    assert_eq!(exact::<CallState>(&f["states"]["connected"]), CallState::Connected);
    assert_eq!(exact::<CallState>(&f["states"]["reconnecting"]), CallState::Reconnecting { attempt: 3 });
    assert_eq!(exact::<CallState>(&f["states"]["ended"]), CallState::Ended { reason: "channel_archived".into() });
    let people: Vec<Participant> = exact(&f["participants"]);
    assert!(people[0].is_self && people[0].server_deafened);
    let speaking: Vec<Speaking> = exact(&f["speaking"]);
    assert_eq!(speaking[0].level, 0.25);
    let devices: DeviceList = exact(&f["devices"]);
    assert_eq!(devices.output, None);
    assert_eq!(exact::<ErrorInfo>(&f["errors"]["mic_denied"]).code, "mic_denied");
    assert_eq!(exact::<ErrorInfo>(&f["errors"]["device_lost"]).code, "device_lost");
    assert_eq!(exact::<ErrorInfo>(&f["errors"]["refused"]).code, "voice_channel_full");
    let args: JoinArgs = exact(&f["joinArgs"]);
    assert_eq!((args.muted, args.deafened), (Some(true), Some(false)));
    assert_eq!(exact::<JoinResult>(&f["joinResult"]).call_id, "r_x");
    let m: MuteArgs = serde_json::from_value(f["muteArgs"].clone()).expect("mute args");
    assert_eq!(m.muted, Some(true));
    let s: SetDeviceArgs = serde_json::from_value(f["setDeviceArgs"].clone()).expect("set_device args");
    assert_eq!(s.output.as_deref(), Some("out:2"));
    assert_eq!(exact::<StateSnapshot>(&f["snapshots"]["live"]).call_id.as_deref(), Some("r_x"));
    assert_eq!(exact::<StateSnapshot>(&f["snapshots"]["idle"]).call_id, None);
    assert_eq!(f["rejoinReasons"], json!([RejoinReason::AdmitLost, RejoinReason::CredentialExpired]));
    assert_eq!(
        exact::<CallState>(&f["states"]["consent_required"]),
        CallState::ConsentRequired { recording_id: "77".into(), notice_version: 3141 }
    );
    assert_eq!(exact::<RecordingInfo>(&f["recording"]["pending"]).my_consent, consent::PENDING);
    let granted: RecordingInfo = exact(&f["recording"]["granted"]);
    assert!(granted.recorder_present && granted.participants[0].recorded);
    assert_eq!(granted.my_consent, consent::GRANTED);
    assert_eq!(exact::<Option<RecordingInfo>>(&f["recording"]["stopped"]), None);
}

/// Sew's own contract file: the renderer's copy is byte for byte the one in the
/// pinned crates.io release, and every payload parses to the plugin's real types and back.
#[test]
fn sews_contract_matches_the_plugin_and_the_renderers_copy() {
    // Since 0.8.3 (Sew 0.10.5) the crate keeps its contract files in contracts/, not tests/.
    let pinned = sew_crate_file("tauri-plugin-sew-voice", "contracts/native-voice-contract.json");
    let copy = std::fs::read(repo_root().join("src/renderer/lib/voice/sew-native-voice-contract.json")).expect("the renderer's copy");
    assert_eq!(copy, pinned, "the renderer's copy of Sew's contract is stale");
    let f: Value = serde_json::from_slice(&pinned).expect("JSON");
    for group in ["sew_reasons", "fallbacks", "recording_reasons"] {
        for r in f["ended"][group].as_array().expect(group) {
            let want = CallState::Ended { reason: r["reason"].as_str().expect("reason").into() };
            assert_eq!(exact::<StateEvent>(&r["payload"]).state, want);
        }
    }
    let moved = exact::<StateEvent>(&f["moved"]["payload"]);
    assert_eq!(moved.state, CallState::Connected);
    assert_eq!(moved.room.as_deref(), Some("48831765147549710"));
    for (name, list) in f["participants"].as_object().expect("participants") {
        if name != "event" {
            let _: Vec<Participant> = exact(list);
        }
    }
    let denied: Vec<Participant> = serde_json::from_value(f["participants"]["self_speak_denied"].clone()).expect("list");
    assert!(denied[0].speak_denied && denied[0].muted && !denied[0].server_muted);
    assert_eq!(exact::<StateSnapshot>(&f["state_snapshot"]["speak_denied"]).participants.len(), 1);
    assert_eq!(exact::<StateSnapshot>(&f["state_snapshot"]["moved"]).room.as_deref(), Some("48831765147549710"));
    // Recording consent (Sew 0.7.3): a held join, its notice, and the call's recording.
    let held = exact::<StateSnapshot>(&f["state_snapshot"]["consent_required"]);
    assert!(matches!(held.state, CallState::ConsentRequired { notice_version: 3141, .. }), "{:?}", held.state);
    assert_eq!(held.call_id, None);
    let held_state = exact::<StateEvent>(&f["recording"]["held"]["state"]);
    assert!(matches!(held_state.state, CallState::ConsentRequired { .. }), "{:?}", held_state.state);
    let notice: RecordingInfo = exact(&f["recording"]["held"]["recording"]);
    assert_eq!(notice.my_consent, consent::PENDING);
    assert!(notice.consent_prompt.is_some());
    for name in ["granted", "withdrawn"] {
        // Sew 0.10.5 (plugin 0.8.3): started_at, the recording's own start by
        // Sew's clock, survives the plugin's type both ways.
        assert_eq!(f["recording"][name]["started_at"], "2026-11-02T09:58:00.000Z", "{name}");
        let _: RecordingInfo = exact(&f["recording"][name]);
    }
    assert_eq!(exact::<Option<RecordingInfo>>(&f["recording"]["stopped"]), None);
}

fn video_fixture() -> Value {
    let path = repo_root().join("src/renderer/lib/voice/native-video-contract.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("video contract file is JSON")
}

/// `v` parses to a `T` (argument shapes, which serialize their absent options
/// as null and so don't round-trip; the plugin's types refuse unknown keys).
fn parses<T: DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{v} doesn't parse: {e}"))
}

/// The video file the TS adapter is tested against (native-video-engine.test.ts)
/// matches the plugin's real commands, events and types (Sew 0.8.2).
#[test]
fn the_renderers_video_contract_file_matches_the_plugin() {
    let f = video_fixture();
    assert_eq!(f["plugin"], PLUGIN_NAME);
    assert_eq!(f["eventPrefix"], EVENT_PREFIX);
    assert_eq!(f["commands"], json!(VIDEO_COMMANDS));
    // The renderer's video listeners: video's four events, and `errors` (shared
    // with voice) for the video engine errors.
    let mut want: Vec<&str> = EVENT_NAMES[6..].to_vec();
    want.push("errors");
    assert_eq!(f["events"], json!(want));

    let caps: VideoCaps = exact(&f["videoCaps"]);
    assert!(caps.receive && caps.self_view && caps.live_switch, "{caps:?}");
    assert_eq!(exact::<VideoCaps>(&f["videoCapsWithoutVideo"]), VideoCaps::default());
    let old: VideoCaps = parses(&f["videoCapsOld"]);
    assert!(!old.receive && !old.self_view && !old.live_switch, "an older payload reads as false");
    let devices: VideoDevices = exact(&f["videoDevices"]);
    assert_eq!(devices.permission.camera, PermissionState::Granted);
    for p in f["cameraPermission"].as_array().expect("cameraPermission") {
        let _: PermissionState = exact(p);
    }
    let tracks: Vec<VideoTrack> = exact(&f["video"]);
    assert!(tracks[2].is_self);
    for name in ["on", "paused", "refused", "hidden", "lost"] {
        let _: LocalVideo = exact(&f["localVideo"][name]);
    }
    let lost: LocalVideo = exact(&f["localVideo"]["lost"]);
    assert_eq!((lost.camera, lost.reason.as_deref()), (LocalVideoState::Off, Some(video_code::CAMERA_LOST)), "a camera that went away");
    assert_eq!(exact::<LocalVideo>(&f["localVideo"]["hidden"]).screen, LocalVideoState::Paused);
    let sources: Vec<ScreenSource> = exact(&f["screenSources"]);
    assert_eq!(sources[1].app.as_deref(), Some("Acme Docs"));
    let _: PreviewAnswer = exact(&f["previewResult"]);
    let _: VideoSinkArgs = parses(&f["videoSinkArgs"]);
    assert_eq!(exact::<VideoSinkAnswer>(&f["videoSinkResult"]).mode, SinkMode::Encoded);
    assert_eq!(exact::<VideoSinkAnswer>(&f["videoSinkResultDecoded"]).mode, SinkMode::Decoded);
    let early: VideoSinkAnswer = exact(&f["videoSinkResultSelfViewEarly"]);
    assert_eq!((early.mode, early.codec), (SinkMode::Encoded, None), "a self-view sink opened before the camera has no codec yet");
    let _: VideoSinkUpdateArgs = parses(&f["videoSinkUpdateArgs"]);
    let _: SinkIdArgs = parses(&json!({"sink_id": 3}));
    let _: CameraArgs = parses(&json!({"on": true, "device": "cam:1"}));
    let _: ScreenArgs = parses(&json!({"on": true, "source": "display:1"}));
    let _: PreviewArgs = parses(&json!({"on": false}));
    let _: SetVideoDeviceArgs = parses(&json!({"camera": DEFAULT_DEVICE}));
    for group in ["rejects", "errors"] {
        for (_, e) in f[group].as_object().expect(group) {
            let _: ErrorInfo = exact(e);
        }
    }
    let snap: StateSnapshot = exact(&f["snapshotWithVideo"]);
    assert!(snap.has_video && snap.local_video.camera == LocalVideoState::On && snap.policy.is_some());
    let bare: StateSnapshot = exact(&f["snapshotWithoutVideo"]);
    assert!(bare.video.is_empty() && !bare.has_video && bare.policy.is_none());

    // Every refusal and stop reason the plugin names; the renderer has words
    // for each one (native-video-engine.test.ts).
    let codes = [
        video_code::VIDEO_UNSUPPORTED,
        video_code::UNKNOWN_TRACK,
        video_code::UNKNOWN_SINK,
        video_code::DECODED_UNSUPPORTED,
        video_code::DECODED_TILES_FULL,
        video_code::VIDEO_NOT_ALLOWED,
        video_code::VIDEO_DISABLED,
        video_code::CAMERA_DENIED,
        video_code::CAMERA_PERMISSION_DENIED,
        video_code::CAMERA_PERMISSION_NOT_DETERMINED,
        video_code::CAMERA_BUSY,
        video_code::CAMERA_NOT_FOUND,
        video_code::VIDEO_SLOTS_FULL,
        video_code::MEDIA_BUSY,
        video_code::MEDIA_CAPACITY,
        video_code::CODEC_UNAVAILABLE,
        video_code::SCREEN_DENIED,
        video_code::SCREEN_SLOTS_FULL,
        video_code::PICKER_CANCELLED,
        video_code::SCREEN_UNSUPPORTED,
        video_code::VIDEO_STALLED,
        video_code::ENCODER_LOST,
        video_code::CAMERA_LOST,
        video_code::SCREEN_NOT_FOUND,
        video_code::CAPTURE_FAILED,
    ];
    assert_eq!(f["codes"], json!(codes));
    let reasons = [
        tauri_plugin_sew_voice::wire::video_reason::HIDDEN,
        tauri_plugin_sew_voice::wire::video_reason::BACKGROUND,
        tauri_plugin_sew_voice::wire::video_reason::THERMAL,
        tauri_plugin_sew_voice::wire::video_reason::PAUSED,
        tauri_plugin_sew_voice::wire::video_reason::USER_STOPPED,
        tauri_plugin_sew_voice::wire::video_reason::SOURCE_CLOSED,
        tauri_plugin_sew_voice::wire::video_reason::VIDEO_DISABLED,
        tauri_plugin_sew_voice::video::SYSTEM_UI,
        // A camera whose source ended (unplugged, an iPhone walked away): the
        // track is unpublished with the engine's own code as the reason.
        video_code::CAMERA_LOST,
        // Sew 0.8.3: the camera opened but sent no frame. Not in the plugin
        // yet; the renderer has its words ready.
        "no_frames",
    ];
    assert_eq!(f["reasons"], json!(reasons));
}

/// The headless smoke of `video_caps` on the machine that runs the tests: Sew's
/// real engine, as K2's build has it (the plugin's `video` feature: this OS's
/// capture backend). No call, no camera opened, no prompt.
#[test]
fn the_real_engine_has_video_on_this_os() {
    use tauri_plugin_sew_voice::sew_call_client::ClientConfig;
    use tauri_plugin_sew_voice::VoiceEngine;
    let dir = tempfile::tempdir().expect("temp dir");
    let engine = tauri_plugin_sew_voice::real::CallClientEngine::with_mls(ClientConfig::new("dev.k2.video-caps-test", dir.path().to_path_buf()));
    let caps = engine.video_caps();
    eprintln!("video_caps on this machine: {}", serde_json::to_string(&caps).expect("json"));
    assert!(caps.receive, "K2's build receives video: {caps:?}");
    if cfg!(target_os = "macos") {
        assert_eq!(caps.backend, "apple", "{caps:?}");
        assert!(caps.camera && caps.self_view && caps.live_switch, "{caps:?}");
        assert!(caps.screen.supported && caps.screen.list && caps.screen.windows, "ScreenCaptureKit lists displays and windows: {caps:?}");
        assert!(caps.publish_codecs.contains(&tauri_plugin_sew_voice::VideoCodec::Vp8), "{caps:?}");
        assert!(caps.decode_codecs.contains(&tauri_plugin_sew_voice::VideoCodec::Vp8), "{caps:?}");
    }
    if cfg!(target_os = "linux") {
        // A server with no session bus may have no capture backend: still receive.
        assert!(caps.backend == "linux" || caps.backend == "null", "{caps:?}");
        assert!(!caps.decode_codecs.contains(&tauri_plugin_sew_voice::VideoCodec::H264), "Linux has no OS H.264: {caps:?}");
    }
}

/// Lane C's `POST /cli/voice/join` answer around Sew's `VoiceJoinResponse`.
fn daemon_answer() -> Value {
    json!({
        "ok": true,
        "callId": "vc_1",
        "relayOnly": true,
        "room": { "id": "room-1", "name": "Room 1" },
        "server": "lab",
        "join": {
            "url": "wss://voice.lab.k2.dev",
            "token": MEDIA_TOKEN,
            "expires_at": "2026-11-02T12:02:00.000Z",
            "room": "r_fakeroomfakeroomfakeroomfa",
            "state": {
                "member_id": "48831765147549699",
                "channel_id": "48831765147549698",
                "connection": "connecting",
                "secure": false,
                "self_mute": false,
                "self_deaf": false,
                "server_deaf": false,
                "server_mute": false,
                "speak_denied": false,
                "joined_at": "2026-11-02T12:00:00.000Z"
            },
            "call_credential": {
                "token": CALL_CRED,
                "expires_at": "2026-11-02T12:10:00Z",
                "join_id": "0123456789abcdef0123456789abcdef",
                "device_scope": "fedcba9876543210fedcba9876543210"
            }
        }
    })
}

fn login(base: &str) -> Login {
    Login { base_url: base.into(), token: LOGIN_TOKEN.into() }
}

#[test]
fn a_good_answer_keeps_the_credential_and_points_at_the_lane() {
    let body = serde_json::to_vec(&daemon_answer()).expect("json");
    let a = answer_from(200, None, &body, &login("https://lab.k2.dev")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(a.api_base.as_str(), "https://lab.k2.dev/_k2/voice/sew/");
    assert_eq!(a.call_credential().expect("credential").token, CALL_CRED);
    assert_eq!(a.join.token, MEDIA_TOKEN);
    let dbg = format!("{a:?}");
    assert!(!dbg.contains(CALL_CRED) && !dbg.contains(MEDIA_TOKEN), "{dbg}");
}

#[test]
fn a_server_with_no_call_credential_is_refused_by_name() {
    let mut v = daemon_answer();
    v["join"].as_object_mut().expect("join").remove("call_credential");
    let body = serde_json::to_vec(&v).expect("json");
    match answer_from(200, None, &body, &login("https://lab.k2.dev")) {
        Err(ProviderError::Refused { code, .. }) => assert_eq!(code, "voice_server_too_old"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_bad_answer_never_quotes_the_body() {
    for body in [
        b"not json".to_vec(),
        serde_json::to_vec(&json!({"join": {"token": CALL_CRED}})).expect("json"),
        serde_json::to_vec(&json!({"callId": "x"})).expect("json"),
    ] {
        match answer_from(200, None, &body, &login("https://lab.k2.dev")) {
            Err(ProviderError::Other(s)) => assert!(!s.contains("sewcc_") && !s.contains("not json"), "{s}"),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn refusals_map_as_the_guide_says() {
    let l = login("https://lab.k2.dev");
    let err = |status, retry, v: Value| answer_from(status, retry, &serde_json::to_vec(&v).expect("json"), &l).err().expect("an error");
    assert_eq!(err(401, None, json!({"error": {"code": "unauthenticated"}})), ProviderError::SigninRequired);
    // Lane C's refusal: code, hint and details pass through for the §4.6 words.
    let e = err(409, None, json!({"error": {"code": "room_full", "hint": "this room is full", "details": {"limit": 5}}}));
    assert_eq!(e, ProviderError::Refused { code: "room_full".into(), detail: json!({"hint": "this room is full", "details": {"limit": 5}}) });
    let e = err(503, Some(2), json!({"error": {"code": "voice_member_pending", "hint": "retry shortly", "details": null}}));
    assert!(matches!(e, ProviderError::Refused { ref code, .. } if code == "voice_member_pending"), "{e:?}");
    // No K2 refusal in it: busy or broken.
    assert_eq!(err(503, Some(2), json!({})), ProviderError::Unavailable { retry_after_ms: Some(2000) });
    assert_eq!(err(429, None, json!({})), ProviderError::Unavailable { retry_after_ms: None });
    assert!(matches!(err(500, None, json!({})), ProviderError::Other(_)));
}

/// Sew's details for a recorded room (a live recording 77, notice 3141), with
/// the reason Sew puts beside them.
fn consent_details() -> Value {
    let mut d = serde_json::to_value(tauri_plugin_sew_voice::fake::consent_required(77, 3141)).expect("json");
    d["reason"] = json!("recording_consent_required");
    d
}

/// A recorded room is `ConsentRequired` (so the engine holds the join), read
/// from the reason, never the code: from this daemon (Sew's details as its
/// own) and from an older one (nested under `details.sew`). Other 409s, and
/// a consent refusal without a prompt, stay plain refusals.
#[test]
fn a_recorded_room_is_consent_required_and_other_409s_are_refusals() {
    let l = login("https://lab.k2.dev");
    let err = |v: Value| answer_from(409, None, &serde_json::to_vec(&v).expect("json"), &l).err().expect("an error");
    let d = consent_details();
    for body in [
        json!({"ok": false, "code": "recording_consent_required", "error": {"code": "recording_consent_required", "hint": "answer it", "details": d}}),
        json!({"error": {"code": "conflict", "hint": "answer it", "details": {"sew": {"code": "conflict", "hint": "answer it", "details": d}}}}),
    ] {
        match err(body.clone()) {
            ProviderError::ConsentRequired(c) => assert_eq!(c.notice_version(), 3141, "{body}"),
            other => panic!("{body}: {other:?}"),
        }
    }
    let bare = err(json!({"error": {"code": "recording_consent_required", "hint": "x", "details": {"reason": "recording_consent_required"}}}));
    assert!(matches!(bare, ProviderError::Refused { ref code, .. } if code == "recording_consent_required"), "{bare:?}");
    let full = err(json!({"error": {"code": "room_full", "hint": "full", "details": {"limit": 5}}}));
    assert!(matches!(full, ProviderError::Refused { ref code, .. } if code == "room_full"), "{full:?}");
}

#[test]
fn a_refusal_that_quotes_a_token_loses_its_detail() {
    let l = login("https://lab.k2.dev");
    for quoted in [CALL_CRED, LOGIN_TOKEN] {
        let v = json!({"error": {"code": "weird", "hint": format!("bad {quoted}"), "details": null}});
        match answer_from(400, None, &serde_json::to_vec(&v).expect("json"), &l) {
            Err(ProviderError::Refused { code, detail }) => {
                assert_eq!(code, "weird");
                assert_eq!(detail, json!({"hint": null, "details": null}));
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn the_join_body_asks_for_the_credential_and_sends_no_e2ee() {
    let req = |muted, deafened, rejoin| JoinRequest::new("h", "room-2").with_muted(muted).with_deafened(deafened).with_rejoin_opt(rejoin);
    assert_eq!(join_body(&req(false, false, None)), json!({"room": "room-2", "join": {"call_credential": true, "self_mute": false, "self_deaf": false}}));
    assert_eq!(join_body(&req(false, true, None)), json!({"room": "room-2", "join": {"call_credential": true, "self_mute": true, "self_deaf": true}}));
    for r in [RejoinReason::AdmitLost, RejoinReason::CredentialExpired] {
        assert_eq!(join_body(&req(true, false, Some(r))), join_body(&req(true, false, None)), "a re-join asks the same as a join");
    }
}

/// A DM call's room (`dm:<conversation id>`) goes as `{dm}`, and says this
/// build's call client takes Sew's `CallJoinResponse` as is (plugin 0.7.0):
/// the daemon passes it through unchanged.
#[test]
fn a_dm_call_joins_by_dm_and_says_how_its_answer_is_read() {
    let req = JoinRequest::new("h", "dm:900").with_muted(true);
    assert_eq!(
        join_body(&req),
        json!({"dm": "900", "join": {"call_credential": true, "self_mute": true, "self_deaf": false}, "callAnswer": true})
    );
}

/// The daemon's answer to a DM call join with `callAnswer: true`: Sew's
/// `CallJoinResponse` (`{call, created, voice, recording}`) unchanged under
/// `join`. The plugin reads the voice answer and its call credential from
/// `voice` and keeps the call; the DM is the room.
#[test]
fn a_dm_call_answer_passed_through_unchanged_is_read() {
    let voice = daemon_answer()["join"].clone();
    let channel = voice["state"]["channel_id"].clone();
    let body = json!({
        "ok": true,
        "callId": "vc_1",
        "dm": channel,
        "join": {
            "call": {"id": "48831765147549800", "channel_id": channel, "started_by": "48831765147549699", "state": "ringing",
                     "participant_ids": [], "rings": []},
            "created": true,
            "voice": voice,
            "recording": null
        }
    });
    let a = answer_from(200, None, &serde_json::to_vec(&body).expect("json"), &login("https://lab.k2.dev")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(a.call_credential().expect("credential").token, CALL_CRED);
    assert_eq!(a.join.token, MEDIA_TOKEN);
    let call = a.dm_call().expect("a DM call's answer keeps the call");
    assert_eq!(call.call_id.to_string(), "48831765147549800");
    assert_eq!(call.channel_id.to_string(), channel.as_str().expect("id"));
    assert!(call.created);
    let dbg = format!("{a:?}");
    assert!(!dbg.contains(CALL_CRED) && !dbg.contains(MEDIA_TOKEN), "{dbg}");
}

/// The person's answer to a recording notice reaches Sew's join as given, and
/// no answer sends no key.
#[test]
fn the_join_body_carries_the_recording_notice_only_when_there_is_one() {
    let without = JoinRequest::new("h", "room-2");
    assert!(join_body(&without)["join"].get("recording_notice").is_none(), "{}", join_body(&without));
    let notice = json!({"recording_id": "77", "consent": true, "notice_version": 3141});
    let with = without
        .with_rejoin(RejoinReason::AdmitLost)
        .with_recording_notice(serde_json::from_value(notice.clone()).expect("Sew's RecordingNotice"));
    assert_eq!(
        join_body(&with),
        json!({"room": "room-2", "join": {"call_credential": true, "self_mute": false, "self_deaf": false, "recording_notice": notice}})
    );
}

#[test]
fn saved_hosts_resolve_as_the_renderer_builds_them() {
    assert_eq!(host_base_url("lab.k2.dev", 443, true), "https://lab.k2.dev");
    assert_eq!(host_base_url("lab.k2.dev", 8443, true), "https://lab.k2.dev:8443");
    assert_eq!(host_base_url("10.0.0.2", 443, false), "http://10.0.0.2:443");
    let hosts = json!([
        {"id": "a", "hostname": "lab.k2.dev", "port": 443, "secure": true, "label": "lab"},
        {"id": "b", "hostname": "10.0.0.2", "port": 39000},
        {"id": "c", "hostname": "nope"},
    ])
    .to_string();
    assert_eq!(find_host(&hosts, "a"), Some(("https://lab.k2.dev".to_string(), "lab.k2.dev".to_string())));
    assert_eq!(find_host(&hosts, "b"), Some(("http://10.0.0.2:39000".to_string(), "10.0.0.2".to_string())));
    assert_eq!(find_host(&hosts, "c"), None, "no port");
    assert_eq!(find_host(&hosts, "missing"), None);
    assert_eq!(find_host("not json", "a"), None);
    assert_eq!(api_base("https://lab.k2.dev").expect("url").as_str(), "https://lab.k2.dev/_k2/voice/sew/");
}

// ── the provider over HTTP, against a fake daemon ────────────────────

struct Table(Mutex<HashMap<String, Login>>);

impl LoginSource for Table {
    fn login(&self, target: &str) -> Result<Login, ProviderError> {
        self.0.lock().expect("table").get(target).cloned().ok_or(ProviderError::SigninRequired)
    }
}

/// A fake daemon: answers each request in turn and hands the raw requests back.
async fn fake_daemon(answers: Vec<(u16, Value)>) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("http://{}", l.local_addr().expect("addr"));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        for (status, body) in answers {
            let (mut s, _) = l.accept().await.expect("accept");
            let mut got = Vec::new();
            let mut b = [0u8; 8192];
            loop {
                let n = s.read(&mut b).await.expect("read");
                assert!(n > 0, "closed before a whole request");
                got.extend_from_slice(&b[..n]);
                let text = String::from_utf8_lossy(&got).to_string();
                if let Some(i) = text.find("\r\n\r\n") {
                    let len = text
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().expect("len")))
                        .unwrap_or(0);
                    if got.len() >= i + 4 + len {
                        tx.send(text).expect("send");
                        break;
                    }
                }
            }
            let text = body.to_string();
            let msg = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
            s.write_all(msg.as_bytes()).await.expect("write");
        }
    });
    (base, rx)
}

fn provider_for(base: &str) -> K2Provider<Table> {
    let table = HashMap::from([("host-1".to_string(), login(base))]);
    K2Provider::new(Table(Mutex::new(table))).expect("provider")
}

fn request(rejoin: Option<RejoinReason>) -> JoinRequest {
    JoinRequest::new("host-1", "room-1").with_muted(true).with_rejoin_opt(rejoin)
}

#[tokio::test]
async fn join_posts_to_the_bff_with_the_saved_login() {
    let (base, mut seen) = fake_daemon(vec![(200, daemon_answer())]).await;
    let p = provider_for(&base);
    let a = p.join(request(None)).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(a.api_base.as_str(), format!("{base}/_k2/voice/sew/"));
    let got = seen.recv().await.expect("the request");
    assert!(got.starts_with("POST /cli/voice/join HTTP/1.1\r\n"), "{got}");
    assert!(got.contains(&format!("authorization: Bearer {LOGIN_TOKEN}")), "{got}");
    let body = got.split("\r\n\r\n").nth(1).expect("body");
    let v: Value = serde_json::from_str(body).expect("json");
    assert_eq!(v, json!({"room": "room-1", "join": {"call_credential": true, "self_mute": true, "self_deaf": false}}));
}

/// Both reasons Sew can re-join for: it goes back through the login, and each
/// outcome reaches the engine as the guide says.
#[tokio::test]
async fn a_rejoin_for_each_reason_uses_the_login_and_says_what_went_wrong() {
    for reason in [RejoinReason::AdmitLost, RejoinReason::CredentialExpired] {
        let (base, mut seen) = fake_daemon(vec![
            (200, daemon_answer()),
            (401, json!({"error": {"code": "unauthenticated"}})),
            (409, json!({"error": {"code": "room_closing", "hint": "closing", "details": null}})),
        ])
        .await;
        let p = provider_for(&base);
        let a = p.join(request(Some(reason))).await.unwrap_or_else(|e| panic!("{reason:?}: {e}"));
        assert_eq!(a.call_credential().expect("a fresh credential").token, CALL_CRED, "{reason:?}");
        let got = seen.recv().await.expect("the request");
        assert!(got.contains(&format!("authorization: Bearer {LOGIN_TOKEN}")), "{reason:?}: {got}");
        assert_eq!(p.join(request(Some(reason))).await.err(), Some(ProviderError::SigninRequired), "{reason:?}: signed out meanwhile");
        match p.join(request(Some(reason))).await {
            Err(ProviderError::Refused { code, .. }) => assert_eq!(code, "room_closing", "{reason:?}"),
            other => panic!("{reason:?}: {other:?}"),
        }
    }
}

/// After the engine leaves, K2's own call record goes too:
/// `POST /cli/voice/session/leave {callId}` with K2's id from that target's
/// join (the plugin never sees it) and the same login. Once per join; never
/// without the id.
#[tokio::test]
async fn the_session_leave_posts_k2s_call_id_from_the_join_once() {
    let (base, mut seen) = fake_daemon(vec![
        (200, daemon_answer()),
        (200, json!({"ok": true, "left": true, "already": false})),
        (200, json!({"ok": true, "callId": "vc_2", "join": daemon_answer()["join"].clone()})),
        (200, json!({"ok": true, "left": true, "already": true, "why": "sew_dropped"})),
    ])
    .await;
    let p = provider_for(&base);
    // The plugin holds the provider; the app holds a copy. They share the ids.
    let app = p.clone();
    // Nothing joined yet: nothing posted.
    assert_eq!(app.session_leave("host-1").await, Ok(SessionLeave { posted: false, status: None }));
    p.join(request(None)).await.unwrap_or_else(|e| panic!("{e}"));
    seen.recv().await.expect("the join");
    assert_eq!(app.session_leave("host-1").await, Ok(SessionLeave { posted: true, status: Some(200) }));
    let got = seen.recv().await.expect("the leave");
    assert!(got.starts_with("POST /cli/voice/session/leave HTTP/1.1\r\n"), "{got}");
    assert!(got.contains(&format!("authorization: Bearer {LOGIN_TOKEN}")), "the join's login: {got}");
    let body: Value = serde_json::from_str(got.split("\r\n\r\n").nth(1).expect("body")).expect("json");
    assert_eq!(body, json!({"callId": "vc_1"}), "K2's id, not Sew's room");
    // Taken: a second leave posts nothing; another target never had one.
    assert_eq!(app.session_leave("host-1").await, Ok(SessionLeave { posted: false, status: None }));
    assert_eq!(app.session_leave("host-2").await, Ok(SessionLeave { posted: false, status: None }));
    // The engine's re-join gets a new K2 id: the leave sends that one.
    p.join(request(Some(RejoinReason::AdmitLost))).await.unwrap_or_else(|e| panic!("{e}"));
    seen.recv().await.expect("the re-join");
    assert_eq!(app.session_leave("host-1").await, Ok(SessionLeave { posted: true, status: Some(200) }));
    let got = seen.recv().await.expect("the second leave");
    let body: Value = serde_json::from_str(got.split("\r\n\r\n").nth(1).expect("body")).expect("json");
    assert_eq!(body, json!({"callId": "vc_2"}));
    assert!(seen.try_recv().is_err(), "nothing else was sent");
}

#[tokio::test]
async fn the_local_target_and_unknown_hosts_never_send_anything() {
    let (base, mut seen) = fake_daemon(vec![]).await;
    let p = provider_for(&base);
    let mut req = request(None);
    req.target = LOCAL_TARGET.into();
    match p.join(req.clone()).await {
        Err(ProviderError::Refused { code, .. }) => assert_eq!(code, "owner_token_cannot_join"),
        other => panic!("{other:?}"),
    }
    req.target = "not-saved".into();
    assert_eq!(p.join(req).await.err(), Some(ProviderError::SigninRequired));
    assert!(seen.try_recv().is_err(), "nothing was sent");
}

#[tokio::test]
async fn a_server_that_is_down_answers_unavailable() {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("http://{}", l.local_addr().expect("addr"));
    drop(l);
    assert_eq!(provider_for(&base).join(request(None)).await.err(), Some(ProviderError::Unavailable { retry_after_ms: None }));
}

// ── the whole path through the plugin's service, on the fake engine ──

struct World {
    svc: VoiceService,
    engine: Arc<FakeEngine>,
    sink: Arc<RecordingSink>,
}

fn world(p: Arc<dyn JoinProvider>) -> World {
    let engine = Arc::new(FakeEngine::new());
    let sink = Arc::new(RecordingSink::new());
    let spawn: Spawn = Arc::new(|f| {
        tokio::spawn(f);
    });
    let svc = VoiceService::new(engine.clone(), p, sink.clone(), spawn);
    svc.start();
    World { svc, engine, sink }
}

fn join_args() -> JoinArgs {
    JoinArgs::new("host-1", "room-1")
}

async fn until(what: &str, mut pred: impl FnMut() -> bool) {
    for _ in 0..4000 {
        if pred() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("timed out waiting for: {what}");
}

/// What every event carried, as one string: no token may be in it.
fn all_events(sink: &RecordingSink) -> String {
    EVENT_NAMES.iter().map(|e| format!("{:?}", sink.of(e))).collect()
}

#[tokio::test]
async fn a_call_through_k2s_provider_puts_no_token_in_any_event() {
    let (base, _seen) = fake_daemon(vec![(200, daemon_answer())]).await;
    let w = world(Arc::new(provider_for(&base)));
    let r = w.svc.join(join_args()).await.expect("join");
    assert_eq!(r.call_id, "r_fakeroomfakeroomfakeroomfa");
    assert!(w.sink.of("state").contains(&json!({"state": "connected", "target": "host-1", "room": "room-1"})), "{:?}", w.sink.of("state"));
    let events = all_events(&w.sink);
    for secret in [LOGIN_TOKEN, MEDIA_TOKEN, CALL_CRED, "sewcc_"] {
        assert!(!events.contains(secret), "{secret} reached an event: {events}");
    }
    let snap = serde_json::to_string(&w.svc.snapshot()).expect("json");
    assert!(!snap.contains("sewcc_") && !snap.contains(MEDIA_TOKEN), "{snap}");
}

#[tokio::test]
async fn mic_denied_at_join_rejects_the_command_ends_the_call_and_says_so() {
    let (base, mut seen) = fake_daemon(vec![(200, daemon_answer())]).await;
    let w = world(Arc::new(provider_for(&base)));
    w.engine.fail_next_join(EngineError::failed("mic_denied"));
    let e = w.svc.join(join_args()).await.err().expect("the join fails");
    assert_eq!(e.code, "mic_denied");
    assert!(w.sink.of("errors").contains(&json!({"code": "mic_denied", "detail": null})), "{:?}", w.sink.of("errors"));
    assert!(w.sink.of("state").contains(&json!({"state": "ended", "reason": "mic_denied", "target": "host-1", "room": "room-1"})), "{:?}", w.sink.of("state"));
    assert!(seen.try_recv().is_err(), "the fake failed before the provider");
}

#[tokio::test]
async fn mic_denied_and_device_lost_mid_call_reach_the_window_and_the_call_stays() {
    let (base, _seen) = fake_daemon(vec![(200, daemon_answer())]).await;
    let w = world(Arc::new(provider_for(&base)));
    w.svc.join(join_args()).await.expect("join");
    let call = w.engine.last_call().expect("a call");
    for code in ["device_lost", "mic_denied"] {
        call.emit(CallEvent::Error(ErrorInfo::with_detail(code, json!({"audio": "x"}))));
        let want = json!({"code": code, "detail": {"audio": "x"}});
        let sink = Arc::clone(&w.sink);
        until(code, || sink.of("errors").contains(&want)).await;
    }
    assert_eq!(w.svc.snapshot().state, CallState::Connected, "an audio error alone doesn't end the call");
}

#[tokio::test]
async fn a_refused_login_ends_the_call_signin_required() {
    let (base, _seen) = fake_daemon(vec![(401, json!({"error": {"code": "unauthenticated"}}))]).await;
    let w = world(Arc::new(provider_for(&base)));
    let e = w.svc.join(join_args()).await.err().expect("the join fails");
    assert_eq!(e.code, "signin_required");
    assert!(w.sink.of("state").contains(&json!({"state": "ended", "reason": "signin_required", "target": "host-1", "room": "room-1"})));
}

#[tokio::test]
async fn a_server_refusal_reaches_the_window_as_given() {
    let (base, _seen) = fake_daemon(vec![(409, json!({"error": {"code": "room_full", "hint": "full", "details": {"limit": 5}}}))]).await;
    let w = world(Arc::new(provider_for(&base)));
    let e = w.svc.join(join_args()).await.err().expect("the join fails");
    assert_eq!(e.code, "room_full");
    assert_eq!(e.detail, json!({"hint": "full", "details": {"limit": 5}}));
}

/// A recorded room through K2's provider: the plugin holds the join at
/// `consent_required` (no call id, nothing more asked of the daemon), and the
/// window's leave (the desktop has no consent sheet before 0.46.10) ends it
/// `left` at once. It never holds on by itself or hangs.
#[tokio::test]
async fn a_held_join_is_given_up_by_leave_and_never_hangs() {
    let body = json!({"ok": false, "error": {"code": "recording_consent_required", "hint": "answer it", "details": consent_details()}});
    let (base, mut seen) = fake_daemon(vec![(409, body)]).await;
    let w = world(Arc::new(provider_for(&base)));
    let r = tokio::time::timeout(std::time::Duration::from_secs(5), w.svc.join(join_args())).await.expect("join answers").expect("held, not failed");
    assert_eq!(r.call_id, "", "no room while held");
    assert!(matches!(w.svc.snapshot().state, CallState::ConsentRequired { notice_version: 3141, .. }), "{:?}", w.svc.snapshot().state);
    seen.recv().await.expect("the one join");
    tokio::time::timeout(std::time::Duration::from_secs(5), w.svc.leave()).await.expect("leave answers").expect("leave");
    assert_eq!(w.svc.snapshot().state, CallState::Ended { reason: "left".into() });
    assert!(seen.try_recv().is_err(), "nothing more was asked of the daemon");
}

// ── what ships with it ──

#[test]
fn the_app_ships_each_crates_notice() {
    let root = repo_root();
    let conf = std::fs::read_to_string(root.join("src-tauri/tauri.conf.json")).expect("conf");
    for krate in ["sew-call-client", "sew-e2ee", "sew-lk-compat", "sew-lkproto", "sew-mls-crypto", "sew-proto", "tauri-plugin-sew-voice"] {
        let shipped = std::fs::read(root.join(format!("resources/notices/{krate}-NOTICE"))).expect("the copy");
        assert_eq!(shipped, sew_crate_file(krate, "NOTICE"), "{krate}'s copy is stale");
        assert!(conf.contains(&format!("../resources/notices/{krate}-NOTICE")), "{krate} isn't in bundle.resources");
    }
    // sew-noise (Sew 0.10.2) comes from crates.io: its NOTICE carries
    // nnnoiseless's BSD-3-Clause text, which applies with or without dfn3.
    let shipped = std::fs::read(root.join("resources/notices/sew-noise-NOTICE")).expect("the copy");
    assert_eq!(shipped, registry_crate_file("sew-noise", "NOTICE"), "sew-noise's copy is stale");
    assert!(conf.contains("../resources/notices/sew-noise-NOTICE"), "sew-noise isn't in bundle.resources");
    let text = String::from_utf8(shipped).expect("utf-8");
    assert!(text.contains("nnnoiseless") && text.contains("BSD 3-Clause"), "nnnoiseless's notice");
}

/// The DeepFilterNet3 weights ship as `dfn3/<file>` in the app's resources,
/// where the plugin's `dfn3` feature looks (`Builder::dfn3_weights` unset):
/// the bundle globs the staged folder, every build path stages it, and the
/// folder's committed README says what goes there. Never the weights in git.
#[test]
fn the_app_bundles_the_dfn3_weights_folder() {
    let root = repo_root();
    let conf: Value = serde_json::from_str(&std::fs::read_to_string(root.join("src-tauri/tauri.conf.json")).expect("conf")).expect("json");
    let resources: Vec<&str> = conf["bundle"]["resources"].as_array().expect("a list").iter().map(|v| v.as_str().expect("a path")).collect();
    assert!(resources.contains(&"dfn3/*"), "{resources:?}");
    let before_build = conf["build"]["beforeBuildCommand"].as_str().expect("beforeBuildCommand");
    assert!(before_build.contains("./scripts/fetch-dfn3.sh"), "{before_build}");
    let before_dev = conf["build"]["beforeDevCommand"].as_str().expect("beforeDevCommand");
    assert!(before_dev.contains("dev:dfn3"), "{before_dev}");
    assert!(root.join("src-tauri/dfn3/README.md").is_file(), "the folder's README keeps the glob matching");
    let ignore = std::fs::read_to_string(root.join(".gitignore")).expect(".gitignore");
    assert!(ignore.lines().any(|l| l == "src-tauri/dfn3/*"), "the weights stay out of git");
    let manifest = std::fs::read_to_string(root.join("src-tauri/Cargo.toml")).expect("src-tauri/Cargo.toml");
    let row = manifest.lines().find(|l| l.starts_with("tauri-plugin-sew-voice = ")).expect("the plugin row");
    assert!(row.contains("\"dfn3\""), "the plugin's dfn3 feature: {row}");
}

/// A file of a crates.io crate as cargo unpacked it, at the version
/// Cargo.lock pins from crates.io (offline: this build already fetched it).
/// A same-named path crate (the vendored sew-proto 0.3) is not the one.
fn registry_crate_file(krate: &str, rel: &str) -> Vec<u8> {
    let lock = std::fs::read_to_string(repo_root().join("Cargo.lock")).expect("Cargo.lock");
    let name_line = format!("name = \"{krate}\"");
    let mut registry = lock.split("[[package]]").filter(|p| p.lines().any(|l| l == name_line) && p.contains("\nsource = \"registry+"));
    let package = registry.next().unwrap_or_else(|| panic!("{krate} is not in Cargo.lock from crates.io"));
    assert!(registry.next().is_none(), "{krate}: more than one crates.io version in Cargo.lock");
    let version = package
        .lines()
        .find_map(|l| l.strip_prefix("version = \""))
        .expect("a version line")
        .trim_end_matches('"')
        .to_string();
    let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from).unwrap_or_else(|| dirs::home_dir().expect("home").join(".cargo"));
    let src = cargo_home.join("registry/src");
    for entry in std::fs::read_dir(&src).unwrap_or_else(|e| panic!("read {}: {e}", src.display())) {
        let path = entry.expect("entry").path().join(format!("{krate}-{version}")).join(rel);
        if path.exists() {
            return std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        }
    }
    panic!("{krate}-{version}/{rel} isn't under {}", src.display())
}

// ── recording: consent, Record and Stop (Sew 0.9.0, K-g) ─────────────

/// Sew's recording as the person sees it (the daemon's `recording`).
fn sew_recording() -> Value {
    json!({"id": "42", "channel_id": "901", "state": "recording", "started_by": "7", "started_at": "2026-11-10T13:00:00Z",
           "auto": false, "trigger": "manual", "message_id": "5", "mixed": true, "audio_keep_days": 0, "max_duration_ms": 0,
           "participants": [], "my_consent": "granted"})
}

fn sent_body(req: &str) -> Value {
    serde_json::from_str(req.split("\r\n\r\n").nth(1).expect("body")).expect("json")
}

/// The person's answer to a notice reaches `/cli/voice/recording/consent`
/// with their saved login, and Sew's recording comes back to the engine.
#[tokio::test]
async fn consent_posts_the_answer_with_the_saved_login() {
    let (base, mut seen) = fake_daemon(vec![(200, json!({"ok": true, "recordingId": "42", "consent": true, "recording": sew_recording()}))]).await;
    let p = provider_for(&base);
    let answer: tauri_plugin_sew_voice::ConsentAnswer =
        serde_json::from_value(json!({"target": "host-1", "room": "room-1", "recording_id": "42", "consent": true, "notice_version": 3141}))
            .expect("an answer");
    let rec = p.consent(answer).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rec.id.to_string(), "42");
    let got = seen.recv().await.expect("the request");
    assert!(got.starts_with("POST /cli/voice/recording/consent HTTP/1.1\r\n"), "{got}");
    assert!(got.contains(&format!("authorization: Bearer {LOGIN_TOKEN}")), "{got}");
    assert_eq!(sent_body(&got), json!({"recording_id": "42", "consent": true, "notice_version": 3141}));
}

/// Record and Stop call the daemon's routes, Record by the room, Stop by the
/// recording; Sew's refusal comes back as its code.
#[tokio::test]
async fn record_and_stop_call_the_daemons_routes() {
    let refusal = json!({"ok": false, "code": "recording_required", "error": {"code": "recording_required", "hint": "h",
        "details": {"sew": {"details": {"reason": "recording_required"}}}}});
    let (base, mut seen) = fake_daemon(vec![
        (200, json!({"ok": true, "action": "started", "recording": sew_recording()})),
        (200, json!({"ok": true, "action": "stopped", "recording": sew_recording()})),
        (422, refusal),
    ])
    .await;
    let p = provider_for(&base);
    let start: tauri_plugin_sew_voice::RecordingRequest =
        serde_json::from_value(json!({"target": "host-1", "room": "room-1", "action": "start"})).expect("start");
    let stop: tauri_plugin_sew_voice::RecordingRequest =
        serde_json::from_value(json!({"target": "host-1", "room": "room-1", "action": "stop", "recording_id": "42"})).expect("stop");
    p.recording(start).await.unwrap_or_else(|e| panic!("{e}"));
    let got = seen.recv().await.expect("record");
    assert!(got.starts_with("POST /cli/voice/recording/start HTTP/1.1\r\n"), "{got}");
    assert_eq!(sent_body(&got), json!({"room": "room-1"}));
    p.recording(stop.clone()).await.unwrap_or_else(|e| panic!("{e}"));
    let got = seen.recv().await.expect("stop");
    assert!(got.starts_with("POST /cli/voice/recording/stop HTTP/1.1\r\n"), "{got}");
    assert_eq!(sent_body(&got), json!({"recordingId": "42"}));
    match p.recording(stop).await {
        Err(ProviderError::Refused { code, .. }) => assert_eq!(code, "recording_required"),
        other => panic!("a required room's Stop is refused by name: {other:?}"),
    }
}

/// HR-6 (prd-home-call-recording-v1): a DM call's Record names the DM, as
/// its join does, never the room string `dm:<conv>`; a room's stays `{room}`.
#[test]
fn a_dm_calls_record_names_the_dm() {
    let req = |room: &str, action: &str| -> tauri_plugin_sew_voice::RecordingRequest {
        serde_json::from_value(json!({"target": "h", "room": room, "action": action, "recording_id": "42"})).expect("request")
    };
    let (route, body) = super::provider::recording_body(&req("dm:900", "start")).expect("start");
    assert_eq!((route, body), ("recording/start", json!({"dm": "900"})));
    let (route, body) = super::provider::recording_body(&req("room-1", "start")).expect("start");
    assert_eq!((route, body), ("recording/start", json!({"room": "room-1"})));
    let (route, body) = super::provider::recording_body(&req("dm:900", "stop")).expect("stop");
    assert_eq!((route, body), ("recording/stop", json!({"recordingId": "42"})), "Stop names the recording either way");
}

/// The plugin's Record and Stop are their own permission set; K2's
/// renderer knows the two command names.
#[test]
fn the_recording_commands_are_the_plugins() {
    let f = fixture();
    assert_eq!(f["recordingCommands"], json!(tauri_plugin_sew_voice::RECORDING_COMMANDS));
}

// ── noise removal: one log line per change (Sew 0.10.4; Sew 0.11.1's fields) ──

/// What a payload without Sew 0.11.1's fields ends with: each unset.
const QUIET: &str = ", dfn3 none, grace_step_downs 0, stage_grace false, grace_blocks 0, grace_over_budget 0, \
                     last_transient none, clock none, max_wall_us none, preempted_over_budget 0, starved 0";

/// Each `noise_removal` payload becomes one line that says which model ran and,
/// when it isn't DeepFilterNet3, why; the payloads are the plugin's own JSON.
#[test]
fn a_noise_removal_event_is_one_log_line() {
    let line = |v: Value| super::noise_removal_line(&v.to_string()).unwrap_or_else(|| panic!("{v} is a noise_removal payload"));
    assert_eq!(
        line(json!({"mode": "standard", "active": "standard", "model": "dfn3", "in_grace": true})),
        format!("noise removal: mode standard, active standard, model dfn3, reason none, step_downs 0, in_grace true{QUIET}")
    );
    // A step-down: RNNoise runs, and the stage's rule says why (the per-block counters ride along, unlogged).
    assert_eq!(
        line(json!({"mode": "standard", "active": "standard", "model": "rnnoise", "step_downs": 2, "step_down_reason": "slow_window",
                    "last_step_down_ms_ago": 1200, "slowest_block_us": 9100, "blocks_over_budget": 7})),
        format!("noise removal: mode standard, active standard, model rnnoise, reason slow_window, step_downs 2, in_grace false{QUIET}")
    );
    // Still loading: model_reason wins over an older step-down.
    assert_eq!(
        line(json!({"mode": "standard", "active": "standard", "model": "rnnoise", "model_reason": "loading", "step_down_reason": "heavy", "step_downs": 1})),
        format!("noise removal: mode standard, active standard, model rnnoise, reason loading, step_downs 1, in_grace false{QUIET}")
    );
    // Back on DeepFilterNet3 after a step-down: the old rule is not the reason now.
    assert_eq!(
        line(json!({"mode": "standard", "active": "standard", "model": "dfn3", "step_down_reason": "heavy", "step_downs": 1})),
        format!("noise removal: mode standard, active standard, model dfn3, reason none, step_downs 1, in_grace false{QUIET}")
    );
    // Off, and a stage the audio pipeline stopped: no model, the state's own reason.
    assert_eq!(line(json!({"mode": "off", "active": "off"})), format!("noise removal: mode off, active off, model none, reason none, step_downs 0, in_grace false{QUIET}"));
    assert_eq!(
        line(json!({"mode": "standard", "active": "off", "reason": "budget"})),
        format!("noise removal: mode standard, active off, model none, reason budget, step_downs 0, in_grace false{QUIET}")
    );
    // Sew 0.11.1: the camera started mid-call; kessel-audio's grace window is open and
    // excused four slow blocks, and it times the stage on the thread's CPU clock.
    assert_eq!(
        line(json!({"mode": "standard", "active": "standard", "model": "dfn3", "in_grace": true, "dfn3_state": "running",
                    "grace_step_downs": 1, "step_downs": 1, "slowest_block_cpu_us": 1300, "p99_block_us": 6900,
                    "capture_stage_grace": true, "capture_stage_grace_blocks": 150, "capture_stage_grace_over_budget": 4,
                    "capture_stage_last_transient": "camera_start", "capture_stage_clock": "thread_cpu",
                    "capture_stage_max_wall_us": 21000, "capture_stage_preempted_over_budget": 5, "capture_stage_starved": 1})),
        "noise removal: mode standard, active standard, model dfn3, reason none, step_downs 1, in_grace true, \
         dfn3 running, grace_step_downs 1, stage_grace true, grace_blocks 150, grace_over_budget 4, last_transient camera_start, \
         clock thread_cpu, max_wall_us 21000, preempted_over_budget 5, starved 1"
    );
    // Stepped down because the audio thread fell behind (the new rule), waiting for a retry.
    assert_eq!(
        line(json!({"mode": "standard", "active": "standard", "model": "rnnoise", "step_down_reason": "starved", "step_downs": 1,
                    "dfn3_state": "stepped_down", "retry_in_ms": 4200, "capture_stage_last_transient": "call_start",
                    "capture_stage_clock": "wall"})),
        "noise removal: mode standard, active standard, model rnnoise, reason starved, step_downs 1, in_grace false, \
         dfn3 stepped_down, grace_step_downs 0, stage_grace false, grace_blocks 0, grace_over_budget 0, last_transient call_start, \
         clock wall, max_wall_us none, preempted_over_budget 0, starved 0"
    );
    assert_eq!(super::noise_removal_line("{}"), None);
    assert_eq!(super::noise_removal_line(r#"{"mode":"standard","active":"standard","model":"vroom"}"#), None);
}

#[test]
fn the_provider_skips_the_proxy_for_loopback_and_http_only() {
    let p = provider_for("http://127.0.0.1:1");
    for base in ["http://127.0.0.1:39000", "http://[::1]:39000", "https://localhost", "http://10.0.0.2:39000"] {
        assert!(std::ptr::eq(p.http_for(base), &p.direct), "{base}: the no-proxy client");
    }
    for base in ["https://lab.k2.dev", "https://10.0.0.2"] {
        assert!(std::ptr::eq(p.http_for(base), &p.proxied), "{base}: the client with the person's proxy settings");
    }
}
