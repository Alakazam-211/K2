//! Sew 0.9.0 "recording in Apps" (K2 PRD prd-k2-bobbin-in-apps-v1 §8, K-h
//! prep and §7 "inert on Sew 0.8.x"): the fake Sew speaks Sew 0.9.0's
//! recording names when it reports the four flags, and refuses every one of
//! them like Sew 0.8.2 when it doesn't. The K2 tests on top drive K2's own
//! sweep and mgmt client against both, so a recording field, a recording
//! route or `voice:record` never reaches a Sew without its flag.
//!
//! Isolation: `TempHome::short()`; never the real `~/.k2` or `/run/sew`.
//! Every test fails loudly.

#![cfg(unix)]

use crate::support::fake_sew;

use std::time::Duration;

use fake_sew::FakeSew;
use k2_core::test_env::TempHome;
use k2_core::tool_hook::{self as hook, features, sew_wire, CHAT_KIND};
use k2_daemon::sew_mgmt::{MgmtClient, MgmtResponse};
use k2_daemon::tool_hook_worker::{self as worker, WorkerConfig};
use serde_json::{json, Value};

pub const APP: &str = "7d4e2a10-1111-4222-8333-444455556666";
pub const SPACE: &str = "5101";

/// Sew 0.8.2's thirty-one flags (the pin before 0.9.0).
pub const SEW_082: [&str; 31] = [
    "app_channel_create",
    "app_controls",
    "app_governed_spaces",
    "app_member_channels",
    "app_mode_gate",
    "app_origin",
    "app_quotas",
    "app_scoped_sessions",
    "app_session_member_role",
    "app_voice_channel_create",
    "home_client",
    "home_client_owner_approval",
    "k2_managed_avatar",
    "k2_managed_guard",
    "member_external_id",
    "person_notifications",
    "principal_relink",
    "private_spaces",
    "space_unbind",
    "voice_video_cap",
    "server_governed_space",
    "import_identity_map",
    "mgmt_voice_states",
    "voice_channel_closing",
    "voice_media_path",
    "turn_relay_only",
    "call_credential",
    "call_stats_native",
    "app_origins",
    "app_participant_links",
    "principal_erase",
];

/// Sew 0.9.0: 0.8.2's thirty-one and the four recording flags.
pub fn sew_090() -> Vec<&'static str> {
    let mut v = SEW_082.to_vec();
    v.extend([features::APP_RECORDING, features::VOICE_RECORD_CAP, features::RECORDING_REQUIRED, features::MGMT_RECORDING_READS]);
    v
}

pub fn cfg() -> WorkerConfig {
    WorkerConfig {
        tick: Duration::from_millis(50),
        backoff_base: Duration::from_millis(20),
        backoff_cap: Duration::from_millis(200),
        reconcile_every: Duration::from_secs(3600),
        reconcile_debounce: Duration::from_millis(10),
        dep_wait: Duration::from_secs(3600),
    }
}

pub struct Env {
    pub _home: TempHome,
    pub sew: FakeSew,
    pub c: MgmtClient,
    n: std::cell::Cell<u32>,
}

pub async fn setup(flags: &[&str]) -> Env {
    let home = TempHome::short();
    hook::allow_target_off_linux_for_tests(true);
    let sew = FakeSew::start(&home.path().join("s")).await;
    sew.set_features(flags);
    hook::set_target(CHAT_KIND, true, Some(sew.socket.to_str().unwrap()), Some(sew.token_file.to_str().unwrap()), "owner-token")
        .expect("enable chat target");
    let c = MgmtClient::new(sew.socket.clone(), sew.token_file.clone());
    Env { _home: home, sew, c, n: std::cell::Cell::new(0) }
}

impl Env {
    pub async fn post(&self, path: &str, body: Value) -> MgmtResponse {
        self.n.set(self.n.get() + 1);
        let key = format!("k2rc_{}", self.n.get());
        self.c.post(path, body.to_string().as_bytes(), &key, "k2:test").await.unwrap_or_else(|e| panic!("POST {path}: {e}"))
    }
    pub async fn get_as(&self, path: &str, actor: &str) -> MgmtResponse {
        self.c.get_as(path, actor).await.unwrap_or_else(|e| panic!("GET {path}: {e}"))
    }
    /// One sweep (status refresh, settings plan) and the worker's pass.
    pub async fn sweep(&self) {
        let c = cfg();
        worker::reconcile_once().await.expect("reconcile");
        for _ in 0..4 {
            worker::pass_once(&c).await.expect("pass");
        }
    }
}

fn reason(r: &MgmtResponse) -> String {
    r.error().and_then(|e| e.error.detail_str("reason").map(str::to_string)).unwrap_or_else(|| panic!("no reason: {}", r.body_text()))
}

/// K-h prep: against Sew 0.8.2's flags the fake refuses every recording
/// name like the real 0.8.2 (unknown field / unknown route), and records it.
#[tokio::test]
async fn sew_0_8_2_refuses_every_recording_name() {
    let e = setup(&SEW_082).await;
    e.sew.add_app_space(SPACE, APP);
    let r = e.post(&format!("/mgmt/v1/apps/{APP}/settings"), json!({"display_name": "Jobs", "recording": {"start_level": "member"}})).await;
    assert_eq!(r.status, 400, "{}", r.body_text());
    let r = e.post(&sew_wire::space_settings_path(SPACE), json!({"app_id": APP, "recording_required": true})).await;
    assert_eq!(r.status, 400, "{}", r.body_text());
    let r = e.post("/mgmt/v1/policies", json!({"changes": {"recording.required": "multi_person"}})).await;
    assert_eq!(r.status, 400, "{}", r.body_text());
    e.sew.add_recording("71", SPACE, "9");
    let r = e.get_as(&sew_wire::space_recordings_path(SPACE, APP, None, 10), &sew_wire::app_actor(APP)).await;
    assert_eq!(r.status, 404, "{}", r.body_text());
    let bad = e.sew.bad_log();
    assert_eq!(bad.lines().count(), 4, "every refusal is recorded: {bad}");
}

/// K-h prep: Sew 0.9.0's names, word for word: App and Space recording
/// settings, the policies, the four reads (governing App, `server_read`),
/// and the feed, which never carries content.
#[tokio::test]
async fn sew_0_9_0_speaks_the_recording_names() {
    let e = setup(&sew_090()).await;
    e.sew.add_app_space(SPACE, APP);
    e.sew.add_recording("71", SPACE, "9");
    e.sew.add_turn("71", "k2:owner", "hello \\*team\\*");
    let actor = sew_wire::app_actor(APP);
    // server_read off: Sew's own refusal.
    let r = e.get_as(&sew_wire::recording_transcript_path("71", APP, None, 10), &actor).await;
    assert_eq!((r.status, reason(&r).as_str()), (403, sew_wire::reasons::RECORDING_SERVER_READ_OFF));
    let r = e
        .post(&format!("/mgmt/v1/apps/{APP}/settings"), json!({"recording": {"start_level": "moderator", "server_read": true, "cap_required": false}}))
        .await;
    assert_eq!(r.status, 200, "{}", r.body_text());
    assert_eq!(e.sew.app_settings(APP).expect("row")["recording"], json!({"start_level": "moderator", "server_read": true, "cap_required": false}));
    let r = e.post(&sew_wire::space_settings_path(SPACE), json!({"app_id": APP, "recording_start_level": "none", "recording_required": null})).await;
    assert_eq!(r.status, 200, "{}", r.body_text());
    assert_eq!(e.sew.space_recording(SPACE), (Some("none".to_string()), None));
    let r = e.post("/mgmt/v1/policies", json!({"changes": {"recording.required": "multi_person"}})).await;
    assert_eq!(r.status, 200, "{}", r.body_text());
    // Reads: the summary and the transcript parse to K2's types, and K2's
    // types drop the audio links Sew holds.
    let r = e.get_as(&sew_wire::recording_path("71", APP), &actor).await;
    assert_eq!(r.status, 200, "{}", r.body_text());
    assert!(r.body_text().contains("audio_url"), "the fake holds an audio link");
    let one: sew_wire::RecordingSummary = serde_json::from_slice(&r.body).expect("summary");
    assert!(!serde_json::to_string(&one).unwrap().contains("audio"));
    let r = e.get_as(&sew_wire::recording_transcript_path("71", APP, None, 10), &actor).await;
    let t: sew_wire::TranscriptPage = serde_json::from_slice(&r.body).expect("transcript");
    assert_eq!((t.turns.len(), t.turns[0].speaker_external_id.as_str()), (1, "k2:owner"));
    let r = e.get_as(&sew_wire::space_recordings_path(SPACE, APP, None, 10), &actor).await;
    let page: sew_wire::RecordingsPage = serde_json::from_slice(&r.body).expect("page");
    assert_eq!(page.recordings.len(), 1);
    // A participant App: Sew says app_not_governing.
    let r = e.get_as(&sew_wire::recording_path("71", "other-app"), "app:other-app").await;
    assert_eq!((r.status, reason(&r).as_str()), (409, sew_wire::reasons::APP_NOT_GOVERNING));
    // The feed.
    e.sew.push_recording_change(APP, sew_wire::recording::CHANGE_TRANSCRIPT_READY, "71");
    let r = e.get_as(&sew_wire::app_recording_changes_path(APP, None, 0, 50), &actor).await;
    assert_eq!(r.status, 200, "{}", r.body_text());
    let f: sew_wire::RecordingChangesPage = serde_json::from_slice(&r.body).expect("feed");
    assert_eq!((f.changes.len(), f.next.as_str()), (1, "1"));
    assert!(!serde_json::to_string(&f).unwrap().contains("never on the feed"));
    // Every read named the App.
    let reads: Vec<_> = e.sew.calls().into_iter().filter(|c| c.method == "GET" && c.path.contains("recording")).collect();
    assert!(reads.len() >= 6 && reads.iter().all(|c| c.actor.as_deref().is_some_and(|a| a.starts_with("app:"))), "{reads:?}");
    e.sew.assert_clean();
}
