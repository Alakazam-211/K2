//! K2's side of native voice against Sew's contract and the plugin's fake engine.
//! Real devices aren't here: `mic_denied` and `device_lost` come from the fake.
//! The real ones are N3's per-OS calls.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri_plugin_sew_voice::fake::{FakeEngine, RecordingSink};
use tauri_plugin_sew_voice::wire::{EVENT_NAMES, EVENT_PREFIX};
use tauri_plugin_sew_voice::{
    CallEvent, CallState, DeviceList, EngineError, ErrorInfo, JoinArgs, JoinProvider, JoinRequest, JoinResult, MuteArgs, Participant, ProviderError, RejoinReason,
    SetDeviceArgs, Spawn, Speaking, StateSnapshot, VoiceEngine, VoiceService, COMMANDS, PLUGIN_NAME,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::lazy::LazyEngine;
use super::provider::{answer_from, api_base, find_host, host_base_url, join_body, K2Provider, Login, LoginSource, LOCAL_TARGET};

const LOGIN_TOKEN: &str = "k2-login-token-for-tests";
const MEDIA_TOKEN: &str = "media-token-for-tests";
const CALL_CRED: &str = "sewcc_sel.secret-for-tests";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("repo root").to_path_buf()
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
    assert_eq!(f["events"], json!(EVENT_NAMES));
    assert_eq!(f["commands"], json!(COMMANDS));
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
            "media": { "relay_only": true },
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
    let req = |muted, deafened, rejoin| JoinRequest { target: "h".into(), room: "room-2".into(), muted, deafened, rejoin };
    assert_eq!(join_body(&req(false, false, None)), json!({"room": "room-2", "join": {"call_credential": true, "self_mute": false, "self_deaf": false}}));
    assert_eq!(join_body(&req(false, true, None)), json!({"room": "room-2", "join": {"call_credential": true, "self_mute": true, "self_deaf": true}}));
    for r in [RejoinReason::AdmitLost, RejoinReason::CredentialExpired] {
        assert_eq!(join_body(&req(true, false, Some(r))), join_body(&req(true, false, None)), "a re-join asks the same as a join");
    }
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
    JoinRequest { target: "host-1".into(), room: "room-1".into(), muted: true, deafened: false, rejoin }
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
    let last = p.last_target().expect("the provider knows the room");
    assert_eq!((last.target.as_str(), last.room.as_str()), ("host-1", "room-1"));
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
    assert!(p.last_target().is_none());
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
    JoinArgs { target: "host-1".into(), room: "room-1".into(), muted: None, deafened: None }
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
    ["state", "participants", "speaking", "errors", "devices"].iter().map(|e| format!("{:?}", sink.of(e))).collect()
}

#[tokio::test]
async fn a_call_through_k2s_provider_puts_no_token_in_any_event() {
    let (base, _seen) = fake_daemon(vec![(200, daemon_answer())]).await;
    let w = world(Arc::new(provider_for(&base)));
    let r = w.svc.join(join_args()).await.expect("join");
    assert_eq!(r.call_id, "r_fakeroomfakeroomfakeroomfa");
    assert!(w.sink.of("state").contains(&json!({"state": "connected"})), "{:?}", w.sink.of("state"));
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
    assert!(w.sink.of("state").contains(&json!({"state": "ended", "reason": "mic_denied"})), "{:?}", w.sink.of("state"));
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
    assert!(w.sink.of("state").contains(&json!({"state": "ended", "reason": "signin_required"})));
}

#[tokio::test]
async fn a_server_refusal_reaches_the_window_as_given() {
    let (base, _seen) = fake_daemon(vec![(409, json!({"error": {"code": "room_full", "hint": "full", "details": {"limit": 5}}}))]).await;
    let w = world(Arc::new(provider_for(&base)));
    let e = w.svc.join(join_args()).await.err().expect("the join fails");
    assert_eq!(e.code, "room_full");
    assert_eq!(e.detail, json!({"hint": "full", "details": {"limit": 5}}));
}

// ── the lazy engine ──────────────────────────────────────────────────

#[tokio::test]
async fn nothing_starts_until_something_needs_the_engine() {
    let made = Arc::new(Mutex::new(0));
    let fake = Arc::new(FakeEngine::with_demo_devices());
    let (m, f) = (Arc::clone(&made), Arc::clone(&fake));
    let lazy = LazyEngine::new(move || {
        *m.lock().expect("count") += 1;
        Arc::clone(&f) as Arc<dyn VoiceEngine>
    });
    // What the plugin does at app start.
    let mut events = lazy.device_events();
    assert!(!lazy.started());
    assert_eq!(*made.lock().expect("count"), 0, "nothing made at start");

    assert_eq!(lazy.devices().unwrap_or_else(|e| panic!("{e}")).inputs.len(), 2);
    assert!(lazy.started());
    lazy.devices().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(*made.lock().expect("count"), 1, "made once");

    // Hot-plug after that reaches a receiver taken before the engine existed.
    fake.set_devices(DeviceList::default());
    let got = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await.expect("in time").expect("a list");
    assert_eq!(got, DeviceList::default());
}

// ── what ships with it ───────────────────────────────────────────────

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let p = e.expect("entry").path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Every vendored file is what VENDOR.txt pinned: nothing changed, missing or
/// extra, the plugin's build script's output included.
#[test]
fn vendor_manifest_matches_the_tree() {
    let root = repo_root().join("third_party/sew-voice");
    let text = std::fs::read_to_string(root.join("VENDOR.txt")).expect("VENDOR.txt");
    let manifest = text.split("# manifest\n").nth(1).expect("a manifest section");
    let want: HashMap<&str, &str> = manifest.lines().map(|l| l.split_once("  ").map(|(h, p)| (p, h)).expect("sha  path")).collect();
    let mut files = Vec::new();
    walk(&root, &mut files);
    let mut seen = 0;
    for f in &files {
        let rel = f.strip_prefix(&root).expect("under root").to_string_lossy().replace('\\', "/");
        if rel == "VENDOR.txt" {
            continue;
        }
        let bytes = std::fs::read(f).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        let have = format!("{:x}", Sha256::digest(&bytes));
        let pinned = want.get(rel.as_str()).unwrap_or_else(|| panic!("{rel} isn't in the manifest"));
        assert_eq!(&have, pinned, "{rel} changed");
        seen += 1;
    }
    assert_eq!(seen, want.len(), "a pinned file is missing");
}

#[test]
fn the_app_ships_sew_lk_compats_notice() {
    let root = repo_root();
    let shipped = std::fs::read(root.join("resources/notices/sew-lk-compat-NOTICE")).expect("the copy");
    let vendored = std::fs::read(root.join("third_party/sew-voice/crates/sew-lk-compat/NOTICE")).expect("Sew's");
    assert_eq!(shipped, vendored);
    let conf = std::fs::read_to_string(root.join("src-tauri/tauri.conf.json")).expect("conf");
    assert!(conf.contains("../resources/notices/sew-lk-compat-NOTICE"), "not in bundle.resources");
}
