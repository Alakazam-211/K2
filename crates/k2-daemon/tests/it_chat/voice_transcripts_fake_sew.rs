//! K2 0.46.11, the headless check (Rosson 2026-11-10: agents review the
//! transcripts of this server's own calls): a real daemon over HTTP against
//! the fake Sew speaking Sew 0.9.0. The K5 rooms take `require_consent`; an
//! agent without "Read call transcripts" is refused; only the owner turns it
//! on; a read goes to Sew's compliance view as `k2:owner`, is logged with the
//! agent, and never reaches an App's recording; the rooms list says agents
//! read; a Voice & Chat only login never records; a room's ready transcript
//! notifies only the agent chosen for it.
//!
//! Isolation: `TempHome::short()`; never the real `~/.k2` or `/run/sew`.

use crate::support::fake_sew;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fake_sew::FakeSew;
use k2_core::connect_users::{self, Role};
use k2_core::session::SessionId;
use k2_core::test_env::TempHome;
use k2_core::tool_hook::{self as hook, features, CHAT_KIND};
use k2_core::voice_rooms::{self, Hosting};
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::sew_mgmt::MgmtClient;
use k2_daemon::tool_hook_worker::{self as worker, WorkerConfig};
use k2_daemon::voice::transcripts;
use rusqlite::params;
use serde_json::{json, Value};

const OWNER: &str = "owner-token-voice-transcripts";

const SEW_090: [&str; 10] = [
    "app_governed_spaces",
    features::SERVER_GOVERNED_SPACE,
    features::MGMT_VOICE_STATES,
    features::VOICE_CHANNEL_CLOSING,
    features::VOICE_MEDIA_PATH,
    features::TURN_RELAY_ONLY,
    features::APP_RECORDING,
    features::VOICE_RECORD_CAP,
    features::RECORDING_REQUIRED,
    features::MGMT_RECORDING_READS,
];

fn cfg() -> WorkerConfig {
    WorkerConfig {
        tick: Duration::from_millis(50),
        backoff_base: Duration::from_millis(20),
        backoff_cap: Duration::from_millis(200),
        reconcile_every: Duration::from_secs(3600),
        reconcile_debounce: Duration::from_millis(10),
        dep_wait: Duration::from_secs(3600),
    }
}

struct Env {
    home: TempHome,
    sew: FakeSew,
}

impl Drop for Env {
    fn drop(&mut self) {
        k2_core::voice_preview::set_enabled(false);
        voice_rooms::set_hosting_for_tests(None);
        transcripts::set_note_sink(None);
    }
}

async fn setup() -> Env {
    let home = TempHome::short();
    hook::allow_target_off_linux_for_tests(true);
    let sew = FakeSew::start(&home.path().join("s")).await;
    sew.set_features(&SEW_090);
    k2_core::voice_preview::set_enabled(true);
    voice_rooms::set_hosting_for_tests(Some(Hosting::Ok));
    hook::set_target(CHAT_KIND, true, Some(sew.socket.to_str().unwrap()), Some(sew.token_file.to_str().unwrap()), "owner-token")
        .expect("enable chat target");
    Env { home, sew }
}

fn pending() -> bool {
    hook::log(5000, None).expect("log").iter().any(|r| r.state == "pending")
}

/// Sweep and deliver until the K5 Space and its room are made.
async fn settle(env: &Env) {
    for _ in 0..15 {
        let _ = hook::take_reconcile_request();
        let r = worker::reconcile_once().await.expect("sweep");
        assert!(r.failed.is_empty(), "sweep failed: {:?}", r.failed);
        for _ in 0..40 {
            if !pending() {
                break;
            }
            hook::force_due_for_tests().unwrap();
            worker::pass_once(&cfg()).await.expect("pass");
        }
        if !pending() && !hook::reconcile_requested() {
            return;
        }
    }
    panic!("never settled; fake log:\n{}", env.sew.log());
}

struct Resp {
    status: u16,
    body: Value,
}

fn http(port: u16, method: &str, pq: &str, body: Option<Value>) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let raw = match body {
        Some(b) => {
            let b = b.to_string();
            format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}", b.len())
        }
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    Resp { status, body: serde_json::from_str(body).unwrap_or_else(|e| panic!("{method} {pq}: not JSON ({e}): {body:?}")) }
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

async fn call(port: u16, method: &'static str, path: &str, token: &str, query: &str, body: Option<Value>) -> Resp {
    let pq = format!("{path}?token={}{}{query}", enc(token), if query.is_empty() { "" } else { "&" });
    tokio::task::spawn_blocking(move || http(port, method, &pq, body)).await.unwrap()
}

fn code(r: &Resp) -> &str {
    r.body["error"]["code"].as_str().unwrap_or_else(|| panic!("no error code: {}", r.body))
}

fn seed_ws(env: &Env, handle: &str) -> (String, String) {
    let path = env.home.path().join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, path.to_string_lossy()]).unwrap();
    (id, path.to_string_lossy().to_string())
}

fn passport(ws_id: &str) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: ws_id.to_string(), agent_address: ws_id.to_string() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

fn login(name: &str, role: Role) -> String {
    connect_users::add_user(name, "correct-horse-battery-staple-9").expect("add user");
    if role != Role::Member {
        connect_users::set_role(name, role).expect("role");
    }
    connect_users::create_session(name)
}

#[tokio::test]
async fn agents_review_the_k5_rooms_transcripts_on_their_switch() {
    let env = setup().await;
    settle(&env).await;
    let view = hook::admin_space_view().expect("view");
    let space = view.space.as_ref().and_then(|s| s.space_id.clone()).expect("the K5 Space");
    let ch = view.rooms[0].channel_id.clone().expect("room-1's channel");

    // Rosson 2026-11-10: K2's own rooms are "declining means not joining".
    assert_eq!(env.sew.channel_recording(&ch), Some(json!({"require_consent": true})), "{}", env.sew.log());

    let d = k2_daemon::test_harness::start(OWNER).await;
    let (scribe, scribe_path) = seed_ws(&env, "scribe");
    let (other, _) = seed_ws(&env, "other");
    let (p_scribe, p_other) = (passport(&scribe), passport(&other));
    env.sew.add_recording("71", &space, &ch);
    env.sew.add_turn("71", "k2:owner", "Ship it on Friday.");
    env.sew.add_recording("72", "5101", "9");

    // No switch: refused, with the wall's code.
    let r = call(d.port, "GET", "/cli/voice/transcript", &p_scribe, "id=71", None).await;
    assert_eq!((r.status, code(&r)), (403, "transcripts_read_off"), "{}", r.body);
    let r = call(d.port, "GET", "/cli/voice/rooms", OWNER, "", None).await;
    assert_eq!(r.body["agentReaders"], false);

    // Only the owner turns it on: an Admin's login is refused.
    let admin = login("ada", Role::Admin);
    let set = json!({"toggle": "voice_transcripts", "workspace": scribe_path, "value": true});
    let r = call(d.port, "POST", "/cli/agent-access/set", &admin, "", Some(set.clone())).await;
    assert_eq!((r.status, code(&r)), (403, "owner_only"), "{}", r.body);
    let r = call(d.port, "POST", "/cli/agent-access/set", OWNER, "", Some(set)).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // Now it reads, as k2:owner on Sew's compliance view, and the read is logged.
    let r = call(d.port, "GET", "/cli/voice/transcript", &p_scribe, "id=71", None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["lines"][0]["text"], "Ship it on Friday.");
    assert_eq!(r.body["lines"][0]["speaker"], "owner");
    assert!(r.body.to_string().find("audio").is_none(), "no audio link ever crosses: {}", r.body);
    let log = env.sew.log();
    assert!(log.contains("GET /mgmt/v1/recordings/71/transcript"), "{log}");
    assert!(log.contains("actor=Some(\"k2:owner\")"), "{log}");
    let reads = k2_core::voice_transcripts::reads(Some("71"), 10).expect("log");
    assert_eq!((reads.len(), reads[0].reader.as_str(), reads[0].lines), (1, "agent:scribe", 1), "{reads:?}");
    // The other agent still can't; an App's recording isn't this server's.
    let r = call(d.port, "GET", "/cli/voice/transcript", &p_other, "id=71", None).await;
    assert_eq!(code(&r), "transcripts_read_off");
    let r = call(d.port, "GET", "/cli/voice/transcript", &p_scribe, "id=72", None).await;
    assert_eq!((r.status, code(&r)), (404, "not_found"), "{}", r.body);
    let r = call(d.port, "GET", "/cli/voice/recordings", &p_scribe, "", None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let ids: Vec<&str> = r.body["recordings"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["71"], "the K5 Space only");
    // The bare owner token lists but never reads.
    let r = call(d.port, "GET", "/cli/voice/transcript", OWNER, "id=71", None).await;
    assert_eq!(code(&r), "owner_token_cannot_read");
    // Every client's notice now names agents.
    let r = call(d.port, "GET", "/cli/voice/rooms", OWNER, "", None).await;
    assert_eq!(r.body["agentReaders"], true);

    // A Voice & Chat only login never records.
    let vc = login("vic", Role::VoiceChat);
    let r = call(d.port, "POST", "/cli/voice/recording/start", &vc, "", Some(json!({"room": "room-1"}))).await;
    assert_eq!(r.status, 403, "{}", r.body);

    // Subscriptions: only an agent that can read; the note goes to it alone.
    let r = call(d.port, "POST", "/cli/voice/transcripts/subscribe", OWNER, "", Some(json!({"room": "room-1", "agent": "other"}))).await;
    assert_eq!((r.status, code(&r)), (409, "agent_cannot_read"), "{}", r.body);
    let r = call(d.port, "POST", "/cli/voice/transcripts/subscribe", OWNER, "", Some(json!({"room": "room-1", "agent": "scribe"}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["rooms"][0]["sendTo"], json!(["scribe"]));
    // The fake's recordings all start at one fixed time: subscribe from
    // before it, so they count as after the subscription.
    k2_core::voice_transcripts::with_store(|c| {
        c.execute("UPDATE voice_transcript_subscriptions SET set_at = 0", []).map(|_| ()).map_err(|e| e.to_string())
    })
    .unwrap();
    let notes: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = notes.clone();
    transcripts::set_note_sink(Some(Arc::new(move |ws: &str, _t: &str, body: &str| sink.lock().unwrap().push((ws.to_string(), body.to_string())))));
    let client = MgmtClient::new(env.sew.socket.clone(), env.sew.token_file.clone());
    assert_eq!(transcripts::notify_once(&client).await.expect("notify"), 1);
    let got = notes.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, scribe, "only the chosen agent");
    assert!(got[0].1.contains("k2 voice transcript 71"), "{}", got[0].1);
    assert_eq!(transcripts::notify_once(&client).await.expect("again"), 0, "once");
}
