//! Home chat's daemon half against a fake Sew: the HTTP seam
//! (`voice::bff::SewApi`) and a real local gateway websocket that speaks
//! Sew's HELLO / IDENTIFY / READY / REAUTH. Each test fails loudly.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use super::follower::{self, CatchUp};
use super::state::PersonState;
use super::{people, routes, set_deps, Deps};
use crate::compute_routes::Who;
use crate::voice::bff::{SewAnswer, SewApi};
use k2_core::connect_users::Role;
use k2_core::home_chat::Badge;
use k2_core::tool_hook::features::HomeChatFeatures;

/// One test at a time: the deps seam and the held people are process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

const ALEX: &str = "k2:connect:0f8fad5b-d9cb-469f-a165-70867728950e";

/// The fake Sew client API. `auth/k2` answers a session that is already
/// due for renewal (expires in 200 s), `auth/renew` a fresh hour.
#[derive(Default)]
struct FakeApi {
    calls: Mutex<Vec<(String, String, Option<String>, Value)>>,
    /// Path → (status, body) overrides.
    answers: Mutex<HashMap<String, (u16, Value)>>,
}

impl FakeApi {
    fn answer(&self, path: &str, status: u16, body: Value) {
        self.answers.lock().unwrap().insert(path.to_string(), (status, body));
    }
    fn calls_to(&self, path: &str) -> Vec<(Option<String>, Value)> {
        self.calls.lock().unwrap().iter().filter(|c| c.1 == path).map(|c| (c.2.clone(), c.3.clone())).collect()
    }
    fn reply(&self, method: &str, bearer: Option<&str>, path: &str, body: &Value) -> SewAnswer {
        self.calls.lock().unwrap().push((method.into(), path.into(), bearer.map(str::to_string), body.clone()));
        if let Some((s, b)) = self.answers.lock().unwrap().get(path).cloned() {
            return SewAnswer { status: s, body: b };
        }
        let at = |secs: i64| (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339();
        match path {
            super::sessions::AUTH_K2 => SewAnswer { status: 200, body: json!({"token": "sews_first.x", "expires_at": at(200), "member": {"id": "41"}}) },
            super::sessions::AUTH_RENEW => SewAnswer { status: 200, body: json!({"token": "sews_renewed.x", "expires_at": at(3600), "member": {"id": "41"}}) },
            crate::sew::gateway::TICKET => SewAnswer { status: 200, body: json!({"ticket": "sewtk_one"}) },
            _ => SewAnswer { status: 404, body: json!({"error": {"code": "not_found", "hint": path}}) },
        }
    }
}

impl SewApi for FakeApi {
    fn post(&self, bearer: Option<&str>, path: &str, body: &Value) -> Result<SewAnswer, String> {
        Ok(self.reply("POST", bearer, path, body))
    }
    fn get(&self, bearer: Option<&str>, path: &str) -> Result<SewAnswer, String> {
        Ok(self.reply("GET", bearer, path, &Value::Null))
    }
}

/// A fake Sew gateway on loopback: HELLO, then READY after IDENTIFY; every
/// frame a client sends is recorded; `connections` counts sockets.
struct FakeGateway {
    addr: String,
    frames: Arc<Mutex<Vec<Value>>>,
    connections: Arc<std::sync::atomic::AtomicU32>,
}

async fn fake_gateway(ready: Value) -> FakeGateway {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let frames = Arc::new(Mutex::new(Vec::new()));
    let connections = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let (f2, c2) = (frames.clone(), connections.clone());
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = l.accept().await else { return };
            c2.fetch_add(1, Ordering::SeqCst);
            let (frames, ready) = (f2.clone(), ready.clone());
            tokio::spawn(async move {
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                ws.send(Message::Text(json!({"op": 8, "d": {"heartbeat_interval_ms": 60_000}}).to_string())).await.unwrap();
                while let Some(Ok(m)) = ws.next().await {
                    let Message::Text(t) = m else { continue };
                    let v: Value = serde_json::from_str(&t).unwrap();
                    let op = v["op"].as_u64();
                    frames.lock().unwrap().push(v);
                    if op == Some(2) {
                        ws.send(Message::Text(json!({"op": 0, "t": "ready", "s": 1, "d": ready}).to_string())).await.unwrap();
                    }
                }
            });
        }
    });
    FakeGateway { addr, frames, connections }
}

fn deps_with(api: Arc<FakeApi>, gateway: String, flags: HomeChatFeatures) -> Arc<Deps> {
    Arc::new(Deps {
        sew: api,
        mint: Arc::new(|ext, sid| Ok(format!("pass:{ext}:{sid}"))),
        gateway_addr: Arc::new(move || gateway.clone()),
        flags: Arc::new(move || flags),
        platform_ok: true,
    })
}

fn ready_body() -> Value {
    json!({
        "session_id": "gw1", "member": {"id": "41"}, "member_seq": 7,
        "channels": [
            {"id": "900", "kind": "dm", "last_message_id": "5"},
            {"id": "901", "space_id": "7001", "kind": "text", "last_message_id": "9"},
            {"id": "950", "space_id": "7001", "kind": "voice"},
        ],
        "read_states": [
            {"channel_id": "900", "unread": true, "mention_count": 2},
            {"channel_id": "901", "unread": true, "mention_count": 4},
        ],
    })
}

async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..400 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

/// HP-28: the held session is renewed through `auth/renew` before expiry and
/// the new token rides op 11 REAUTH on the SAME socket: one connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn renew_keeps_the_websocket() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    let api = Arc::new(FakeApi::default());
    let gw = fake_gateway(ready_body()).await;
    set_deps(Some(deps_with(api.clone(), gw.addr.clone(), HomeChatFeatures::default())));
    follower::set_renew_every_for_tests(50);
    let h = follower::hold(ALEX);
    until("IDENTIFY with the ticket", || gw.frames.lock().unwrap().iter().any(|f| f["op"] == 2 && f["d"]["ticket"] == "sewtk_one")).await;
    until("op 11 REAUTH with the renewed token", || gw.frames.lock().unwrap().iter().any(|f| f["op"] == 11 && f["d"]["token"] == "sews_renewed.x")).await;
    assert_eq!(gw.connections.load(Ordering::SeqCst), 1, "the socket never dropped");
    assert_eq!(h.connects.load(Ordering::SeqCst), 1);
    let renews = api.calls_to(super::sessions::AUTH_RENEW);
    assert_eq!(renews[0].0.as_deref(), Some("sews_first.x"), "renewed on the live session");
    assert!(renews[0].1["pass"].as_str().unwrap().starts_with(&format!("pass:{ALEX}:hc_")), "a fresh pass for the held sid");
    assert_eq!(h.status(), "live");
    // READY filled the state: the voice room is not a conversation.
    let st = h.state.lock().unwrap().clone();
    assert_eq!(st.convs.len(), 2);
    assert_eq!(st.me, "41");
    follower::release_all();
    follower::set_renew_every_for_tests(15_000);
    set_deps(None);
}

/// §14.3: a catch-up reads the changes feed; `410 resync_required` rebuilds
/// every read state from `me/sync`.
#[test]
fn catch_up_reads_changes_and_resyncs_on_410() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let api = Arc::new(FakeApi::default());
    set_deps(Some(deps_with(api.clone(), "127.0.0.1:9".into(), HomeChatFeatures::default())));
    let mut st = PersonState::default();
    st.apply_ready(&ready_body());
    api.answer("/api/v1/me/changes?since=7", 200, json!({"entries": [{"seq": 8, "kind": "read_state", "entity_id": "901"}], "next_since": 8, "has_more": false}));
    api.answer("/api/v1/me/read_states?channel_ids=901", 200, json!({"read_states": [{"channel_id": "901", "unread": false, "mention_count": 0}]}));
    assert_eq!(follower::catch_up("sews_t", &mut st).unwrap(), CatchUp::Changes { channels: 1 });
    assert!(!st.convs["901"].unread, "the changed channel was read again");
    assert!(st.convs["900"].unread, "the others are untouched");
    assert_eq!(st.member_seq, 8);
    // The gap is too big: 410, then me/sync rebuilds everything.
    api.answer("/api/v1/me/changes?since=8", 410, json!({"error": {"code": "resync_required"}}));
    api.answer("/api/v1/me/sync", 200, json!({"member_id": "41", "member_seq": 30, "channels": [{"id": "902", "kind": "dm"}], "read_states": [{"channel_id": "902", "unread": true, "mention_count": 1}]}));
    assert_eq!(follower::catch_up("sews_t", &mut st).unwrap(), CatchUp::Resync);
    assert_eq!(st.convs.keys().cloned().collect::<Vec<_>>(), vec!["902".to_string()], "rebuilt from me/sync");
    assert_eq!(st.member_seq, 30);
    set_deps(None);
}

/// §15.1 through the follower's state: a DM counts, a channel is a dot,
/// `unread_count` replaces both once the flag shows; own posts read.
#[test]
fn summary_badges_follow_the_rule() {
    let mut st = PersonState::default();
    st.apply_ready(&ready_body());
    let badge = |st: &PersonState, conv: &str, flag: bool| -> Value { st.summary(&[conv.to_string()], flag)[0]["badge"].clone() };
    assert_eq!(badge(&st, "900", false), serde_json::to_value(Badge::Count { n: 2 }).unwrap());
    assert_eq!(badge(&st, "901", false), serde_json::to_value(Badge::Dot).unwrap(), "a channel's mentions never show");
    // Someone else posts in the DM: Sew counts it in mention_count.
    let frames = st.apply_event("message_created", &json!({"message": {"id": "6", "channel_id": "900", "author_id": "42", "created_at": "2026-11-14T10:00:00Z"}}), false);
    assert!(frames.iter().any(|f| f["type"] == "message") && frames.iter().any(|f| f["type"] == "summary_changed"));
    assert_eq!(badge(&st, "900", false), serde_json::to_value(Badge::Count { n: 3 }).unwrap());
    // The person's own post reads the conversation.
    st.apply_event("message_created", &json!({"message": {"id": "7", "channel_id": "900", "author_id": "41"}}), false);
    assert_eq!(badge(&st, "900", false), serde_json::to_value(Badge::None).unwrap());
    // With `unread_count`.
    st.apply_read_state(&json!({"channel_id": "901", "unread": true, "mention_count": 4, "unread_count": 12}));
    assert_eq!(badge(&st, "901", true), serde_json::to_value(Badge::Count { n: 12 }).unwrap());
    // Removed from a channel: the row goes.
    let f = st.apply_event("channel_member_removed", &json!({"channel_id": "901", "member_id": "41"}), true);
    assert!(f.iter().any(|x| x["type"] == "member_removed"));
    assert!(!st.convs.contains_key("901"));
}

/// Typing frames reach clients only for conversations they have open, and
/// never the person's own.
#[test]
fn typing_only_for_viewed_conversations() {
    let mut st = PersonState::default();
    st.apply_ready(&ready_body());
    assert!(st.apply_event("typing_started", &json!({"channel_id": "900", "member_id": "42"}), false).is_empty());
    st.viewing.insert("900".into());
    assert_eq!(st.apply_event("typing_started", &json!({"channel_id": "900", "member_id": "42"}), false).len(), 1);
    assert!(st.apply_event("typing_started", &json!({"channel_id": "900", "member_id": "41"}), false).is_empty());
}

/// §13: the owner token, agents and App sessions are refused with exact codes.
#[test]
fn only_a_persons_own_login_may_chat() {
    let r = people::caller_of(Who::Owner, "owner-token", "owner-token").unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (403, "person_login_required"));
    let r = people::caller_of(Who::Agent { project_id: "p".into(), session_id: "s".into() }, "x", "o").unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (403, "agents_use_k2_chat"));
    let r = people::caller_of(Who::Wrong, "k2skn_x", "o").unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (403, "app_session_not_allowed"));
    let r = people::caller_of(Who::Invalid, "nope", "o").unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (401, "unauthenticated"));
}

/// HP-13: a disabled login is refused as a DM or channel target; agents and
/// App users are not people here.
#[test]
fn disabled_and_non_people_targets_are_refused() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    k2_core::connect_users::add_user("sam", "a-long-password-1").unwrap();
    k2_core::connect_users::add_user("kim", "a-long-password-2").unwrap();
    let ids: HashMap<String, String> = k2_core::connect_users::admin_space_facts().unwrap().into_iter().map(|(id, u, _, _)| (u, id)).collect();
    people::check_target(&format!("k2:connect:{}", ids["kim"])).expect("an enabled login");
    people::check_target("k2:owner").expect("the owner");
    k2_core::connect_users::set_disabled("sam", true).unwrap();
    let r = people::check_target(&format!("k2:connect:{}", ids["sam"])).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (403, "target_disabled"));
    for ext in ["k2:workspace:0f8fad5b-d9cb-469f-a165-70867728950e", "k2:principal:abc"] {
        let r = people::check_target(ext).unwrap_err();
        assert_eq!((r.status, r.code.as_str()), (403, "not_a_person"), "{ext}");
    }
    let r = people::check_target("k2:connect:00000000-0000-4000-8000-000000000000").unwrap_err();
    assert_eq!(r.code, "person_not_found");
}

/// Every mutating route answers 405 to a GET, before anything else.
#[test]
fn every_post_route_refuses_get() {
    for p in routes::POST_ROUTES {
        let c = Ok(people::Caller { ext: ALEX.into(), login: "alex".into(), role: Role::Member });
        let r = routes::handle(p, c, false, &HashMap::new(), b"");
        assert!(r.status.starts_with("405"), "{p}: {}", r.status);
    }
    for p in routes::POST_ROUTES.iter().chain(routes::GET_ROUTES) {
        assert!(crate::routes::route_policy::lookup(p).is_some(), "{p} has a policy row");
    }
    assert!(crate::routes::route_policy::lookup(routes::EVENTS).is_some());
}

/// HP-11: every Home chat row is on the Voice & Chat only allowlist.
#[test]
fn home_chat_rows_are_on_the_voice_chat_allowlist() {
    use crate::routes::route_policy::VOICE_CHAT_ALLOWLIST;
    for p in routes::POST_ROUTES {
        assert!(VOICE_CHAT_ALLOWLIST.contains(&("POST", *p)), "{p}");
    }
    for p in routes::GET_ROUTES.iter().chain([&routes::EVENTS]) {
        assert!(VOICE_CHAT_ALLOWLIST.contains(&("GET", *p)), "{p}");
    }
}
