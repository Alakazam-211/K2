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

/// Two Spaces in Sew, bound to two Apps in K2 (the bind reaches Sew in the
/// sweep), and the App's settings row.
fn bind_both(e: &Env) {
    // Sew's side is bound by the route before K2 records it (as
    // `k2 hostchat app-space bind` does).
    e.sew.add_app_space(SPACE, APP);
    e.sew.add_app_space("5102", OTHER_APP);
    hook::app_space_bind(APP, SPACE, "owner").expect("bind");
    hook::app_space_bind(OTHER_APP, "5102", "owner").expect("bind");
    hook::set_app_home(APP, false, "Jobs", "owner").expect("app row");
}

pub const OTHER_APP: &str = "8e5f3b21-2222-4333-8444-555566667777";

/// K2's recording wishes: an App's settings (with `cap_required`), a
/// Space override, a lock on another Space, and an org policy.
fn want_everything() {
    let mut f = serde_json::Map::new();
    f.insert("start_level".into(), json!("moderator"));
    f.insert("server_read".into(), json!(true));
    f.insert("cap_required".into(), json!(true));
    hook::recording_set_app(APP, &f, "owner").expect("app recording");
    hook::recording_set_space(SPACE, Some(json!("member")), Some(json!(true)), "owner").expect("space override");
    hook::recording_set_lock("5102", true, "owner").expect("lock");
    hook::recording_set_policy("recording.required", &json!("multi_person"), "owner").expect("policy");
}

/// PRD §7 + §8 K-h prep ("recording_contract_fake_sew"): K2's sweep against
/// Sew 0.8.2 sends no recording field, no recording route and no recording
/// policy, whatever K2 wants; every body is one 0.8.2 takes.
#[tokio::test]
async fn k2_sends_nothing_about_recording_to_sew_0_8_2() {
    let e = setup(&SEW_082).await;
    bind_both(&e);
    e.sweep().await;
    want_everything();
    e.sweep().await;
    let settings = e.sew.app_settings(APP).expect("the App's settings went");
    assert!(settings.get("recording").is_none(), "{settings}");
    assert_eq!(e.sew.space_recording(SPACE), (None, None));
    assert_eq!(e.sew.space_recording("5102"), (None, None));
    assert!(e.sew.policy("recording.required").is_none());
    let log = e.sew.log();
    assert!(!log.contains("recording"), "no recording name crossed: {log}");
    e.sew.assert_clean();
}

/// The same wishes against Sew 0.9.0: every one arrives, the lock wins over
/// what the Space asked, and `cap_required` rides along.
#[tokio::test]
async fn k2_sends_its_recording_settings_to_sew_0_9_0() {
    let e = setup(&sew_090()).await;
    bind_both(&e);
    e.sweep().await;
    want_everything();
    hook::recording_set_space("5102", Some(json!("member")), None, "agent").expect("asks above the lock");
    e.sweep().await;
    let settings = e.sew.app_settings(APP).expect("the App's settings went");
    assert_eq!(settings["recording"], json!({"start_level": "moderator", "server_read": true, "cap_required": true}), "{settings}");
    assert_eq!(e.sew.space_recording(SPACE), (Some("member".to_string()), Some(true)));
    assert_eq!(e.sew.space_recording("5102"), (Some("none".to_string()), Some(false)), "the lock wins");
    assert_eq!(e.sew.policy("recording.required"), Some(json!("multi_person")));
    let view = hook::recording_app_view(APP).expect("view");
    assert_eq!(view["delivered"], json!(true), "{view}");
    e.sew.assert_clean();
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

// ── K-d: an App server's reads and its feed, through K2 ──────────────

use k2_daemon::sew::app_recordings::{self as reads, Polled};

fn q(kv: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
    kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn body(r: &k2_daemon::cli_response::CliResponse) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json ({e}): {}", r.body))
}

fn sew_reads(e: &Env) -> usize {
    e.sew.calls().into_iter().filter(|c| c.method == "GET" && c.path.contains("recording")).count()
}

/// The App governs SPACE (bound in K2 and Sew), a recording with a turn.
async fn governed(flags: &[&str]) -> Env {
    k2_core::db::init_for_tests();
    let e = setup(flags).await;
    bind_both(&e);
    e.sweep().await;
    e.sew.add_recording("71", SPACE, "9");
    e.sew.add_turn("71", "k2:owner", "hello");
    e
}

fn server_read(on: bool) {
    let mut f = serde_json::Map::new();
    f.insert("server_read".into(), json!(on));
    hook::recording_set_app(APP, &f, "owner").expect("server_read");
}

/// PRD §8 K-d: with `server_read` off, `recordings.transcript` (and every
/// other read) is K2's own 403 `recording_server_read_off`: Sew is never
/// asked. On Sew 0.8.x it is `sew_feature_missing`, again before Sew.
#[tokio::test]
async fn server_read_off_is_refused_by_k2_before_sew() {
    let e = governed(&sew_090()).await;
    for (path, p) in [(reads::TRANSCRIPT, q(&[("id", "71")])), (reads::ONE, q(&[("id", "71")])), (reads::LIST, q(&[("space", SPACE)]))] {
        let r = reads::read_for_app(APP, path, &p).await;
        assert_eq!((r.status, body(&r)["error"]["code"].clone()), ("403 Forbidden", json!("recording_server_read_off")), "{path}: {}", r.body);
    }
    assert_eq!(sew_reads(&e), 0, "Sew was never asked");
    hook::note_features_for_tests(&SEW_082).expect("Sew 0.8.2's flags");
    let r = reads::read_for_app(APP, reads::TRANSCRIPT, &q(&[("id", "71")])).await;
    assert_eq!(body(&r)["error"]["code"], json!("sew_feature_missing"), "{}", r.body);
    assert_eq!(sew_reads(&e), 0);
}

/// PRD §8 K-d (Q7): a participant App (the Space is bound to another App)
/// gets K2's own 409 `app_not_governing`, server_read or not; Sew is never
/// asked.
#[tokio::test]
async fn a_participant_app_is_refused_app_not_governing() {
    let e = governed(&sew_090()).await;
    server_read(true);
    let r = reads::read_for_app(APP, reads::LIST, &q(&[("space", "5102")])).await;
    assert_eq!((r.status, body(&r)["error"]["code"].clone()), ("409 Conflict", json!("app_not_governing")), "{}", r.body);
    assert_eq!(sew_reads(&e), 0);
}

/// PRD §8 K-d: a transcript read sends `Sew-Actor: app:<service id>`,
/// writes K2's `deploy_audit` row, and hands on no field K2 doesn't name
/// (the audio links Sew holds).
#[tokio::test]
async fn a_read_names_the_app_and_is_audited() {
    let e = governed(&sew_090()).await;
    server_read(true);
    e.sweep().await;
    let r = reads::read_for_app(APP, reads::TRANSCRIPT, &q(&[("id", "71")])).await;
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = body(&r);
    assert_eq!(v["turns"][0]["text"], json!("hello"));
    assert!(!r.body.contains("audio"), "{}", r.body);
    let r = reads::read_for_app(APP, reads::LIST, &q(&[("space", SPACE)])).await;
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(body(&r)["recordings"][0]["id"], json!("71"));
    assert!(!r.body.contains("audio"), "{}", r.body);
    let calls: Vec<_> = e.sew.calls().into_iter().filter(|c| c.method == "GET" && c.path.contains("recording")).collect();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls.iter().all(|c| c.actor.as_deref() == Some(format!("app:{APP}").as_str())), "{calls:?}");
    let audits: i64 = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT COUNT(*) FROM deploy_audit WHERE event = 'recording_transcript_read'", [], |r| r.get(0)).expect("audit")
    };
    assert_eq!(audits, 1, "one row per transcript read");
    e.sew.assert_clean();
}

/// PRD §8 K-d: a feed page reaches an open socket as `recording_changed`,
/// with no content; a cursor Sew dropped tells the socket to refetch; the
/// feed ends when server_read goes off.
#[tokio::test]
async fn the_feed_sends_changes_without_content() {
    let e = governed(&sew_090()).await;
    server_read(true);
    e.sweep().await;
    let (tx, mut rx) = tokio::sync::broadcast::channel::<String>(16);
    e.sew.push_recording_change(APP, "started", "71");
    e.sew.push_recording_change(APP, "transcript_ready", "71");
    assert_eq!(reads::poll_once(APP, &tx, 0).await, Polled::Sent(2));
    for want in ["started", "transcript_ready"] {
        let f: Value = serde_json::from_str(&rx.recv().await.expect("a frame")).unwrap();
        assert_eq!((f["kind"].clone(), f["change"].clone(), f["recordingId"].clone()), (json!("recording_changed"), json!(want), json!("71")), "{f}");
        assert!(!f.to_string().contains("never on the feed"), "{f}");
    }
    assert_eq!(hook::recording_feed_cursor(APP).unwrap().as_deref(), Some("2"), "the cursor is kept");
    assert_eq!(reads::poll_once(APP, &tx, 0).await, Polled::Sent(0), "nothing new");
    e.sew.push_recording_change(APP, "stopped", "71");
    e.sew.prune_recording_feed(3);
    assert_eq!(reads::poll_once(APP, &tx, 0).await, Polled::Refetch);
    let f: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert_eq!(f, json!({"kind": "recordings_refetch"}));
    server_read(false);
    assert!(matches!(reads::poll_once(APP, &tx, 0).await, Polled::Ended(_)), "server_read off ends the feed");
    let feed_calls = e.sew.calls().into_iter().filter(|c| c.path.ends_with("/recordings/changes")).count();
    assert_eq!(feed_calls, 3, "the last poll never reached Sew");
}
