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
pub(crate) static SERIAL: Mutex<()> = Mutex::new(());

pub(crate) const ALEX: &str = "k2:connect:0f8fad5b-d9cb-469f-a165-70867728950e";

/// The fake Sew client API. `auth/k2` answers a session that is already
/// due for renewal (expires in 200 s), `auth/renew` a fresh hour.
#[derive(Default)]
pub(crate) struct FakeApi {
    calls: Mutex<Vec<(String, String, Option<String>, Value)>>,
    /// Path → (status, body) overrides.
    answers: Mutex<HashMap<String, (u16, Value)>>,
    /// `Idempotency-Key` of every keyed POST, in order.
    keys: Mutex<Vec<String>>,
    /// The next keyed POST fails like a dropped connection (after it is recorded).
    net_fail_once: std::sync::atomic::AtomicBool,
    /// Each renew takes this long (to race two renews).
    renew_delay_ms: std::sync::atomic::AtomicU64,
}

impl FakeApi {
    pub(crate) fn answer(&self, path: &str, status: u16, body: Value) {
        self.answers.lock().unwrap().insert(path.to_string(), (status, body));
    }
    pub(crate) fn calls_to(&self, path: &str) -> Vec<(Option<String>, Value)> {
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
    fn post_keyed(&self, bearer: Option<&str>, path: &str, body: &Value, key: &str) -> Result<SewAnswer, String> {
        self.keys.lock().unwrap().push(key.to_string());
        std::thread::sleep(Duration::from_millis(self.renew_delay_ms.load(Ordering::SeqCst)));
        if self.net_fail_once.swap(false, Ordering::SeqCst) {
            self.calls.lock().unwrap().push(("POST".into(), path.into(), bearer.map(str::to_string), body.clone()));
            return Err("connection reset".into());
        }
        Ok(self.reply("POST", bearer, path, body))
    }
}

/// A fake Sew gateway on loopback: HELLO, then READY after IDENTIFY; every
/// frame a client sends is recorded; `connections` counts sockets.
struct FakeGateway {
    addr: String,
    frames: Arc<Mutex<Vec<Value>>>,
    connections: Arc<std::sync::atomic::AtomicU32>,
    /// A dispatch sent here goes out on every open socket (after READY).
    push: tokio::sync::broadcast::Sender<Value>,
}

async fn fake_gateway(ready: Value) -> FakeGateway {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let frames = Arc::new(Mutex::new(Vec::new()));
    let connections = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let push = tokio::sync::broadcast::channel::<Value>(16).0;
    let (f2, c2, p2) = (frames.clone(), connections.clone(), push.clone());
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = l.accept().await else { return };
            c2.fetch_add(1, Ordering::SeqCst);
            let (frames, ready, mut pushed) = (f2.clone(), ready.clone(), p2.subscribe());
            tokio::spawn(async move {
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                ws.send(Message::Text(json!({"op": 8, "d": {"heartbeat_interval_ms": 60_000}}).to_string())).await.unwrap();
                let mut ready_sent = false;
                loop {
                    tokio::select! {
                        m = ws.next() => {
                            let Some(Ok(m)) = m else { return };
                            let Message::Text(t) = m else { continue };
                            let v: Value = serde_json::from_str(&t).unwrap();
                            let op = v["op"].as_u64();
                            frames.lock().unwrap().push(v);
                            if op == Some(2) {
                                ws.send(Message::Text(json!({"op": 0, "t": "ready", "s": 1, "d": ready}).to_string())).await.unwrap();
                                ready_sent = true;
                            }
                        }
                        d = pushed.recv() => {
                            let Ok(d) = d else { return };
                            assert!(ready_sent, "a test pushed a dispatch before READY");
                            if ws.send(Message::Text(d.to_string())).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            });
        }
    });
    FakeGateway { addr, frames, connections, push }
}

pub(crate) fn deps_with(api: Arc<FakeApi>, gateway: String, flags: HomeChatFeatures) -> Arc<Deps> {
    Arc::new(Deps {
        sew: api,
        mint: Arc::new(|ext, sid| Ok(format!("pass:{ext}:{sid}"))),
        gateway_addr: Arc::new(move || gateway.clone()),
        flags: Arc::new(move || flags),
        platform_ok: true,
        feed: Arc::new(|_, _, _| Err("no feed in this test".to_string())),
        devices: Arc::new(|_| None),
        push_flags: Arc::new(Default::default),
        mgmt: Arc::new(|_, p, _| Err(format!("no mgmt in this test: {p}"))),
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

/// Each channel in the summary says whether it is invite-only (data for the
/// row's glyph); a DM says nothing, nor does a channel Sew didn't describe.
#[test]
fn summary_says_which_channels_are_invite_only() {
    let mut ready = ready_body();
    ready["channels"][1]["visibility"] = json!("private");
    ready["channels"].as_array_mut().expect("channels").push(json!({"id": "902", "space_id": "7001", "kind": "text", "visibility": "public"}));
    let mut st = PersonState::default();
    st.apply_ready(&ready);
    let row = |st: &PersonState, conv: &str| st.summary(&[conv.to_string()], false)[0].clone();
    assert_eq!(row(&st, "901")["inviteOnly"], json!(true));
    assert_eq!(row(&st, "902")["inviteOnly"], json!(false));
    assert_eq!(row(&st, "900")["inviteOnly"], Value::Null, "a DM");
    st.apply_event("message_created", &json!({"message": {"id": "8", "channel_id": "903", "space_id": "7001", "author_id": "42"}}), false);
    assert_eq!(row(&st, "903")["inviteOnly"], Value::Null, "not described yet");
    assert_eq!(super::state::invite_only_of(&json!("private")), Some(true));
    assert_eq!(super::state::invite_only_of(&json!("restricted")), None);
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

/// Sew's renew contract: Bearer the current token and a fresh pass with the
/// SAME sid as the opening pass; the new token is used at once; two renews
/// at the same time make one call; a retry after a network error sends the
/// same Idempotency-Key; any 401 from renew falls back to `/auth/k2`.
#[test]
fn renew_follows_sews_contract() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    let api = Arc::new(FakeApi::default());
    set_deps(Some(deps_with(api.clone(), "127.0.0.1:9".into(), HomeChatFeatures::default())));
    // No runtime here: hold() starts no follower, so only these calls renew.
    let h = follower::hold(ALEX);
    assert_eq!(h.token().unwrap(), "sews_first.x");
    let now = super::now_secs();

    // Two renews race (each Sew call takes 200 ms): one call, one answer.
    api.renew_delay_ms.store(200, Ordering::SeqCst);
    let (a, b) = (h.clone(), h.clone());
    let t1 = std::thread::spawn(move || a.renew_now(now));
    let t2 = std::thread::spawn(move || b.renew_now(now));
    let (r1, r2) = (t1.join().unwrap().unwrap(), t2.join().unwrap().unwrap());
    assert_eq!((r1.as_str(), r2.as_str()), ("sews_renewed.x", "sews_renewed.x"));
    let renews = api.calls_to(super::sessions::AUTH_RENEW);
    assert_eq!(renews.len(), 1, "single-flighted: {renews:?}");
    assert_eq!(renews[0].0.as_deref(), Some("sews_first.x"), "Bearer the current token");
    assert_eq!(h.token().unwrap(), "sews_renewed.x", "the new token is used straight away");
    let opening = api.calls_to(super::sessions::AUTH_K2)[0].1["pass"].as_str().unwrap().to_string();
    let renewing = renews[0].1["pass"].as_str().unwrap().to_string();
    assert_eq!(opening, renewing, "the same sub and sid as the opening pass (the fake pass names both)");
    assert!(renewing.contains(":hc_"), "{renewing}");
    api.renew_delay_ms.store(0, Ordering::SeqCst);

    // A network error: the retry carries the same key (and the same pass).
    api.keys.lock().unwrap().clear();
    api.net_fail_once.store(true, Ordering::SeqCst);
    h.renew_now(now).expect("the retry succeeds");
    let keys = api.keys.lock().unwrap().clone();
    assert_eq!(keys.len(), 2, "{keys:?}");
    assert_eq!(keys[0], keys[1], "same Idempotency-Key on the retry");
    assert!(keys[0].starts_with("k2hc-renew-"));
    let tries = api.calls_to(super::sessions::AUTH_RENEW);
    assert_eq!(tries[tries.len() - 1].1, tries[tries.len() - 2].1, "same body on the retry");

    // Any 401 from renew: a fresh /auth/k2.
    api.answer(super::sessions::AUTH_RENEW, 401, json!({"error": {"code": "session_expired"}}));
    let signins = api.calls_to(super::sessions::AUTH_K2).len();
    assert_eq!(h.renew_now(now).unwrap(), "sews_first.x", "signed in again");
    assert_eq!(api.calls_to(super::sessions::AUTH_K2).len(), signins + 1);
    assert_eq!(h.token().unwrap(), "sews_first.x");
    follower::release_all();
    set_deps(None);
}

/// A members read names its channel by Sew id or by K2's slug (the creator
/// waits on a pending channel by its slug); anything else is a usage error,
/// before any store is read.
#[test]
fn members_take_a_conv_or_a_home_slug() {
    let q = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<_, _>>();
    let r = routes::named_channel(&q(&[])).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (400, "usage"));
    for bad in ["general", "c-short", "c-ABCDEFGHIJ", "../c-0123456789"] {
        let r = routes::named_channel(&q(&[("slug", bad)])).unwrap_err();
        assert_eq!((r.status, r.code.as_str()), (400, "usage"), "{bad}");
    }
    let r = routes::named_channel(&q(&[("conv", "../600")])).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (400, "usage"));
}

fn alex() -> people::Caller {
    people::Caller { ext: ALEX.into(), login: "alex".into(), role: Role::Member }
}

/// A route as the dispatcher runs it once Home chat is on (the status gate
/// is the dispatcher's; these tests drive the route itself).
fn call(path: &str, body: Value) -> Result<Value, crate::voice::bff::Refusal> {
    routes::route(path, &alex(), &HashMap::new(), body.to_string().as_bytes())
}

/// A fake Sew, skin.db made (K2's Home chat ledger lives there), and no
/// answers kept from an earlier test.
fn chat_box() -> (k2_core::test_env::TempHome, Arc<FakeApi>) {
    let home = k2_core::test_env::TempHome::new();
    k2_core::skin::list_principals().expect("skin.db");
    let api = Arc::new(FakeApi::default());
    set_deps(Some(deps_with(api.clone(), "127.0.0.1:9".into(), HomeChatFeatures::default())));
    routes::sends::forget_for_tests();
    (home, api)
}

fn done() {
    follower::release_all();
    routes::sends::forget_for_tests();
    set_deps(None);
}

/// §14.4 `send`: a retry with the same nonce is one message. The retry gets
/// the first post's answer and Sew isn't asked again; a retry after a
/// daemon restart reaches Sew with the same Idempotency-Key (Sew replays);
/// two at once make one post. Another nonce, or another conversation, is a
/// new post.
#[test]
fn a_retried_send_posts_once() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api) = chat_box();
    let path = "/api/v1/channels/900/messages";
    api.answer(path, 200, json!({"id": "77", "channel_id": "900", "body": "hi", "nonce": "n-1"}));
    let send = |conv: &str, nonce: &str| call("/cli/home-chat/send", json!({"conv": conv, "text": "hi", "nonce": nonce}));
    let first = send("900", "n-1").expect("first send");
    assert_eq!(first["message"]["id"], json!("77"));
    // Were Sew asked again it would post a second message.
    api.answer(path, 200, json!({"id": "78", "channel_id": "900", "body": "hi", "nonce": "n-1"}));
    assert_eq!(send("900", "n-1").expect("retry"), first, "the retry answers the first message");
    assert_eq!(api.calls_to(path).len(), 1, "one post to Sew");
    let keys = api.keys.lock().unwrap().clone();
    assert_eq!(keys.len(), 1, "{keys:?}");
    let k = keys[0].clone();
    assert!(k.starts_with("k2hc-send-") && k.len() <= 64 && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'), "{k}");

    // Another nonce is another message; another conversation too.
    assert_eq!(send("900", "n-2").expect("new nonce")["message"]["id"], json!("78"));
    api.answer("/api/v1/channels/901/messages", 200, json!({"id": "90", "channel_id": "901"}));
    assert_eq!(send("901", "n-1").expect("other conv")["message"]["id"], json!("90"));
    let keys = api.keys.lock().unwrap().clone();
    assert_eq!(keys.len(), 3);
    assert!(keys[1] != k && keys[2] != k && keys[1] != keys[2], "{keys:?}");

    // A daemon restart forgets K2's answers: Sew gets the same key again.
    routes::sends::forget_for_tests();
    send("900", "n-1").expect("retry after a restart");
    assert_eq!(api.keys.lock().unwrap().last().cloned(), Some(k.clone()), "Sew replays on the same Idempotency-Key");

    // Two at once (each Sew call takes 300 ms): one post, one answer.
    routes::sends::forget_for_tests();
    let before = api.calls_to(path).len();
    api.renew_delay_ms.store(300, Ordering::SeqCst);
    let t: Vec<_> = (0..2)
        .map(|_| std::thread::spawn(|| call("/cli/home-chat/send", json!({"conv": "900", "text": "hi", "nonce": "n-3"}))))
        .collect();
    let answers: Vec<Value> = t.into_iter().map(|h| h.join().unwrap().expect("concurrent send")).collect();
    api.renew_delay_ms.store(0, Ordering::SeqCst);
    assert_eq!(answers[0], answers[1]);
    assert_eq!(api.calls_to(path).len(), before + 1, "one post for both");

    // A refusal is never kept: the next try asks Sew again.
    api.answer(path, 503, json!({"error": {"code": "unavailable"}}));
    assert!(send("900", "n-4").is_err());
    api.answer(path, 200, json!({"id": "99", "channel_id": "900"}));
    assert_eq!(send("900", "n-4").expect("after a refusal")["message"]["id"], json!("99"));
    done();
}

/// The brief's answer for a post in an archived channel is 409
/// `channel_archived`: from K2's ledger before Sew is asked, and from Sew's
/// own `422 invalid_state {reason: channel_archived}`.
#[test]
fn a_post_in_an_archived_channel_is_409_channel_archived() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api) = chat_box();
    k2_core::tool_hook::with_home_chat_db(|conn| {
        let row = k2_core::tool_hook::admin_channels::request_create(conn, "old-news", false, ALEX, &[], 1)?;
        conn.execute("UPDATE tool_admin_chat_channels SET channel_id = '904', state = 'archived' WHERE slug = ?1", [&row.slug])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .expect("an archived Home channel");
    let r = call("/cli/home-chat/send", json!({"conv": "904", "text": "hi", "nonce": "a1"})).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (409, "channel_archived"));
    assert!(api.calls_to("/api/v1/channels/904/messages").is_empty(), "Sew isn't asked");
    // A channel K2's ledger doesn't know as archived: Sew says so.
    api.answer(
        "/api/v1/channels/905/messages",
        422,
        json!({"error": {"code": "invalid_state", "hint": "channel is archived", "details": {"reason": "channel_archived"}}}),
    );
    let r = call("/cli/home-chat/send", json!({"conv": "905", "text": "hi", "nonce": "a2"})).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (409, "channel_archived"), "not Sew's 422 invalid_state");
    done();
}

/// A new DM's read state can come before its first message (Sew moves the
/// poster's cursor first) and says nothing of the kind: the message that
/// follows makes it a DM, so it counts like one. A channel stays a channel.
#[test]
fn a_dm_is_a_dm_whatever_sew_event_comes_first() {
    let mut st = PersonState::default();
    st.apply_ready(&ready_body());
    // The person's own first message in a new DM.
    st.apply_event("read_state_updated", &json!({"channel_id": "910", "unread": false, "mention_count": 0, "last_message_id": "11"}), false);
    st.apply_event("message_created", &json!({"message": {"id": "11", "channel_id": "910", "author_id": "41"}}), false);
    // Someone else's new DM, read state first too.
    st.apply_event("read_state_updated", &json!({"channel_id": "911", "unread": true, "mention_count": 1}), false);
    st.apply_event("message_created", &json!({"message": {"id": "12", "channel_id": "911", "author_id": "42"}}), false);
    // A channel the same way.
    st.apply_event("read_state_updated", &json!({"channel_id": "912", "unread": true, "mention_count": 0}), false);
    st.apply_event("message_created", &json!({"message": {"id": "13", "channel_id": "912", "space_id": "7001", "author_id": "42"}}), false);
    fn row(st: &PersonState, c: &str) -> Value {
        st.summary(&[c.to_string()], false)[0].clone()
    }
    assert_eq!(row(&st, "910")["kind"], json!("dm"));
    assert_eq!(row(&st, "911")["kind"], json!("dm"));
    assert_eq!(row(&st, "911")["badge"], serde_json::to_value(Badge::Count { n: 2 }).unwrap(), "a DM counts every message");
    assert_eq!(row(&st, "912")["kind"], json!("channel"));
    assert_eq!(st.convs["912"].space_id.as_deref(), Some("7001"));
    // A later message never turns a known kind around.
    st.apply_event("message_created", &json!({"message": {"id": "14", "channel_id": "901", "author_id": "42"}}), false);
    assert_eq!(row(&st, "901")["kind"], json!("channel"));
}

/// The summary names a DM K2 kept as a DM (with its person), also one whose
/// row was made as a channel by a read state; one no event has named yet is
/// read from Sew once.
#[test]
fn the_summary_settles_a_dm_from_k2s_ledger_and_sew() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api) = chat_box();
    k2_core::tool_hook::with_home_chat_db(|conn| k2_core::tool_hook::admin_channels::set_dm_channel(conn, ALEX, "k2:owner", "920", 1))
        .expect("K2 kept the DM");
    let h = follower::hold(ALEX);
    {
        let mut st = h.state.lock().unwrap();
        st.apply_ready(&ready_body());
        // Stored as a channel with no Space (a read state came first).
        st.convs.insert(
            "920".into(),
            super::state::Conv { channel_id: "920".into(), unread: true, mention_count: 1, kind_known: true, ..Default::default() },
        );
        // Nothing has said what this one is yet.
        st.apply_read_state(&json!({"channel_id": "921", "unread": true, "mention_count": 3}));
    }
    api.answer("/api/v1/channels/921", 200, json!({"id": "921", "type": "dm"}));
    let summary = || routes::route("/cli/home-chat/summary", &alex(), &HashMap::new(), b"").expect("summary");
    let s = summary();
    let row = |s: &Value, c: &str| s["convs"].as_array().unwrap().iter().find(|r| r["conv"] == c).cloned().unwrap_or_else(|| panic!("{c}: {s}"));
    let dm = row(&s, "920");
    assert_eq!((dm["kind"].clone(), dm["with"].clone()), (json!("dm"), json!("k2:owner")), "{dm}");
    assert_eq!(dm["badge"], serde_json::to_value(Badge::Count { n: 1 }).unwrap());
    assert_eq!(row(&s, "921")["kind"], json!("dm"));
    assert_eq!(row(&s, "901")["kind"], json!("channel"));
    assert_eq!(api.calls_to("/api/v1/channels/921").len(), 1);
    summary();
    assert_eq!(api.calls_to("/api/v1/channels/921").len(), 1, "settled once");
    for c in ["900", "901", "920"] {
        assert!(api.calls_to(&format!("/api/v1/channels/{c}")).is_empty(), "{c}: no Sew read for a known kind");
    }
    done();
}

/// A DM id K2 kept that Sew no longer has (a rollback of Sew's data) is
/// made again on the next open, never handed back; a send to it says so
/// and forgets it.
#[test]
fn a_kept_dm_sew_no_longer_has_is_made_again() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api) = chat_box();
    k2_core::tool_hook::map_member_for_tests("k2:owner", "50", "person", "owner", k2_core::tool_hook::members::ACTIVE).expect("owner's member");
    let kept = |conv: &str| {
        k2_core::tool_hook::with_home_chat_db(|conn| k2_core::tool_hook::admin_channels::set_dm_channel(conn, ALEX, "k2:owner", conv, 1))
            .expect("kept DM")
    };
    let ledger = || k2_core::tool_hook::with_home_chat_db(|conn| k2_core::tool_hook::admin_channels::dm_channel(conn, ALEX, "k2:owner")).expect("ledger");
    kept("920");
    // Sew still has it: the kept id, nothing made.
    api.answer("/api/v1/channels/920", 200, json!({"id": "920", "type": "dm"}));
    let r = call("/cli/home-chat/dm", json!({"ext": "k2:owner"})).expect("open");
    assert_eq!((r["conv"].clone(), r["created"].clone()), (json!("920"), json!(false)));
    assert!(api.calls_to("/api/v1/dms").is_empty());
    // Sew's data went back: it answers 404 for the kept id.
    api.answer("/api/v1/channels/920", 404, json!({"error": {"code": "not_found"}}));
    api.answer("/api/v1/dms", 200, json!({"id": "930", "type": "dm"}));
    let r = call("/cli/home-chat/dm", json!({"ext": "k2:owner"})).expect("open again");
    assert_eq!((r["conv"].clone(), r["created"].clone()), (json!("930"), json!(true)), "made again, not the stale id");
    assert_eq!(api.calls_to("/api/v1/dms")[0].1["member_ids"], json!(["50"]));
    assert_eq!(ledger().as_deref(), Some("930"));
    // A send to a kept DM Sew lost: 404 conversation_gone, and forgotten.
    api.answer("/api/v1/channels/930/messages", 404, json!({"error": {"code": "not_found"}}));
    let e = call("/cli/home-chat/send", json!({"conv": "930", "text": "hi", "nonce": "g1"})).unwrap_err();
    assert_eq!((e.status, e.code.as_str()), (404, "conversation_gone"));
    assert_eq!(ledger(), None);
    done();
}

/// A reset of Sew's data (`k2 hostchat rollback --accept-data-loss`, a
/// restore): K2 drops the Sew ids its Home chat ledger learnt and sends
/// what people asked for again, every held person starts over at once,
/// and the sweep (status and Sew's features) is asked for now.
#[test]
fn a_sew_data_reset_drops_k2s_sew_ids_and_starts_over() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, _api) = chat_box();
    use k2_core::tool_hook::admin_channels as ledger;
    const KIM: &str = "k2:connect:3b241101-e2bb-4255-8caf-4136c566a962";
    let (live, going) = k2_core::tool_hook::with_home_chat_db(|conn| {
        ledger::set_dm_channel(conn, ALEX, "k2:owner", "920", 1)?;
        let live = ledger::request_create(conn, "plans", false, ALEX, &[KIM.to_string()], 1)?;
        let going = ledger::request_create(conn, "old", false, ALEX, &[], 1)?;
        let e = |e: rusqlite::Error| e.to_string();
        conn.execute("UPDATE tool_admin_chat_channels SET channel_id = '940', state = 'active' WHERE slug = ?1", [&live.slug]).map_err(e)?;
        conn.execute("UPDATE tool_admin_chat_channels SET channel_id = '941', state = 'active', want_archive = 1 WHERE slug = ?1", [&going.slug])
            .map_err(e)?;
        conn.execute("UPDATE tool_admin_chat_members SET state = 'in' WHERE slug = ?1 AND external_id = ?2", rusqlite::params![live.slug, ALEX]).map_err(e)?;
        conn.execute("UPDATE tool_admin_chat_members SET state = 'pending_remove' WHERE slug = ?1 AND external_id = ?2", rusqlite::params![live.slug, KIM])
            .map_err(e)?;
        conn.execute(
            "INSERT INTO tool_admin_member_channels (external_id, channel_ids_want, channel_ids_sent, state, updated_at) VALUES (?1, '[\"940\"]', '[\"940\"]', 'synced', 1)",
            [ALEX],
        )
        .map_err(e)?;
        Ok((live.slug, going.slug))
    })
    .expect("a ledger with Sew ids");
    let h = follower::hold(ALEX);
    h.token().expect("a held session");
    h.state.lock().unwrap().apply_ready(&ready_body());
    let _ = k2_core::tool_hook::take_reconcile_request();

    super::sew_data_reset("a test rollback");

    k2_core::tool_hook::with_home_chat_db(|conn| {
        assert_eq!(ledger::dm_channel(conn, ALEX, "k2:owner")?, None, "the DM id is forgotten");
        let c = ledger::channel_by_slug(conn, &live)?.expect("the live channel");
        assert_eq!((c.channel_id.as_deref(), c.state.as_str()), (None, ledger::CH_PENDING), "made again by its slug");
        let m = ledger::members_of(conn, &live)?;
        assert_eq!(m.iter().map(|m| (m.external_id.as_str(), m.state.as_str())).collect::<Vec<_>>(), vec![(ALEX, ledger::M_PENDING_ADD)], "back on the way in; the leaver is out");
        let g = ledger::channel_by_slug(conn, &going)?.expect("the archiving channel");
        assert_eq!(g.state, ledger::CH_ARCHIVED, "never made again only to be archived");
        assert_eq!(ledger::sent_list(conn, ALEX)?, None, "the confirmed list is unknown again");
        Ok(())
    })
    .expect("the ledger after the reset");
    assert!(h.session.lock().unwrap().is_none(), "the held session is dropped");
    let st = h.state.lock().unwrap().clone();
    assert!(st.convs.is_empty() && st.gateway_session.is_none() && st.member_seq == 0, "no read state kept");
    assert_eq!(h.status(), "connecting");
    assert!(k2_core::tool_hook::take_reconcile_request(), "the sweep runs now (status and Sew's features)");
    done();
}

/// The box re-test: after a reset of Sew's data forgot a DM, a client still
/// holding the old id gets 404 `conversation_gone` (open it again), on a
/// send and on a read, not Sew's bare 404; someone else's 404 for that id
/// stays Sew's. Opening the DM again clears it.
#[test]
fn a_dm_forgotten_by_a_sew_reset_answers_conversation_gone() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api) = chat_box();
    k2_core::tool_hook::with_home_chat_db(|conn| k2_core::tool_hook::admin_channels::set_dm_channel(conn, ALEX, "k2:owner", "920", 1))
        .expect("K2 kept the DM");
    super::sew_data_reset("a test rollback");
    api.answer("/api/v1/channels/920/messages", 404, json!({"error": {"code": "not_found", "hint": "channel not found"}}));
    api.answer("/api/v1/channels/920/messages?limit=50", 404, json!({"error": {"code": "not_found", "hint": "channel not found"}}));
    let e = call("/cli/home-chat/send", json!({"conv": "920", "text": "hi", "nonce": "z1"})).unwrap_err();
    assert_eq!((e.status, e.code.as_str()), (404, "conversation_gone"), "a send");
    let q: HashMap<String, String> = [("conv".to_string(), "920".to_string())].into();
    let e = routes::route("/cli/home-chat/messages", &alex(), &q, b"").unwrap_err();
    assert_eq!((e.status, e.code.as_str()), (404, "conversation_gone"), "a read");
    // Not a DM of theirs: Sew's own answer.
    api.answer("/api/v1/channels/921/messages", 404, json!({"error": {"code": "not_found", "hint": "channel not found"}}));
    let e = call("/cli/home-chat/send", json!({"conv": "921", "text": "hi", "nonce": "z2"})).unwrap_err();
    assert_eq!((e.status, e.code.as_str()), (404, "not_found"));
    // Sew answers the same id again on an open: it isn't gone.
    k2_core::tool_hook::with_home_chat_db(|conn| {
        k2_core::tool_hook::admin_channels::set_dm_channel(conn, ALEX, "k2:owner", "920", 2)?;
        k2_core::tool_hook::admin_channels::dm_gone(conn, ALEX, "920")
    })
    .map(|gone| assert!(!gone, "cleared"))
    .expect("ledger");
    done();
}

/// After a reset of Sew's data the follower connects again at once, with a
/// fresh sign-in and IDENTIFY, whatever backoff the restart's drops built.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sew_data_reset_reconnects_the_follower_at_once() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    let api = Arc::new(FakeApi::default());
    let gw = fake_gateway(ready_body()).await;
    set_deps(Some(deps_with(api.clone(), gw.addr.clone(), HomeChatFeatures::default())));
    let h = follower::hold(ALEX);
    until("live", || h.status() == "live").await;
    let identifies = || gw.frames.lock().unwrap().iter().filter(|f| f["op"] == 2).count();
    assert_eq!(identifies(), 1);
    let signins = api.calls_to(super::sessions::AUTH_K2).len();
    let t0 = std::time::Instant::now();
    h.start_over();
    until("a second IDENTIFY", || identifies() == 2).await;
    until("live again", || h.status() == "live").await;
    assert!(t0.elapsed() < Duration::from_secs(3), "at once, not after a backoff: {:?}", t0.elapsed());
    assert_eq!(gw.connections.load(Ordering::SeqCst), 2);
    assert_eq!(api.calls_to(super::sessions::AUTH_K2).len(), signins + 1, "a fresh sign-in");
    assert!(gw.frames.lock().unwrap().iter().all(|f| f["op"] != 4), "no RESUME of the old gateway session");
    follower::release_all();
    set_deps(None);
}

// ── homes-people-group-v1: arrivals land in People ──

/// The owner's Homes as the store holds them now (live items, wire form).
fn owner_homes() -> Vec<Value> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::homes_sync::pull(&conn, "owner", None, None, chrono::Utc::now().timestamp_millis())
        .expect("pull")
        .changes
        .iter()
        .map(k2_core::homes_sync::Item::to_json)
        .collect()
}

/// The owner syncs Homes here: their groups (and rows) as a desk pushed them,
/// nothing kept from an earlier test.
fn owner_syncs(changes: Value) {
    super::arrivals::forget_for_tests();
    let db = k2_core::db::shared();
    let conn = db.lock();
    for t in ["homes_items", "homes_chat_items", "homes_people"] {
        conn.execute(&format!("DELETE FROM {t} WHERE person = 'owner'"), []).expect("forget the owner's Homes");
    }
    let r = k2_core::homes_sync::push(&conn, "owner", "desk", changes.as_array().expect("changes"), chrono::Utc::now().timestamp_millis())
        .expect("push");
    assert!(r.rejected.is_empty(), "{:?}", r.rejected);
}

fn sid() -> String {
    crate::boot_status::server_id().expect("serverId")
}

fn rows_in(homes: &[Value], group: &str) -> Vec<Value> {
    homes.iter().filter(|v| v["kind"] != "home" && v["homeId"] == group).cloned().collect()
}

/// Someone DMs the owner: the DM (a person row for the sender) lands in the
/// owner's People group, made for it, whatever group they have open; each
/// later message changes nothing. A row the owner placed themselves (an
/// explicit add, in the open group) stays where it is, and agents' rows too.
#[test]
fn a_dm_someone_sends_lands_in_people_once() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api) = chat_box();
    let at = chrono::Utc::now().timestamp_millis() - 1000;
    let agent = format!("atlas::{}", sid());
    owner_syncs(json!([
        {"kind": "home", "id": "h1", "name": "Home", "order": "V", "updatedAt": at},
        {"kind": "home", "id": "h2", "name": "Work", "order": "l", "updatedAt": at},
        {"kind": "row", "homeId": "h2", "address": agent, "label": "Atlas", "order": "V", "updatedAt": at},
    ]));
    k2_core::tool_hook::with_home_chat_db(|conn| k2_core::tool_hook::admin_channels::set_dm_channel(conn, ALEX, "k2:owner", "920", 1))
        .expect("Alex's DM with the owner");
    api.answer("/api/v1/channels/920/messages", 200, json!({"id": "1", "channel_id": "920"}));
    call("/cli/home-chat/send", json!({"conv": "920", "text": "hi", "nonce": "p1"})).expect("send");
    let homes = owner_homes();
    let people: Vec<&Value> = homes.iter().filter(|v| v["kind"] == "home" && v["id"] == "people").collect();
    assert_eq!(people.len(), 1, "{homes:?}");
    assert_eq!((people[0]["name"].clone(), people[0]["builtin"].clone()), (json!("People"), json!("people")));
    let landed = rows_in(&homes, "people");
    assert_eq!(landed.len(), 1, "{homes:?}");
    assert_eq!((landed[0]["kind"].clone(), landed[0]["address"].clone()), (json!("person"), json!(format!("{ALEX}::{}", sid()))));
    // The agent row stays in the group it was put in.
    assert_eq!(rows_in(&homes, "h2").iter().map(|r| r["address"].clone()).collect::<Vec<_>>(), vec![json!(agent)]);
    // More messages: nothing new.
    call("/cli/home-chat/send", json!({"conv": "920", "text": "again", "nonce": "p2"})).expect("send again");
    super::arrivals::forget_for_tests();
    call("/cli/home-chat/send", json!({"conv": "920", "text": "after a restart", "nonce": "p3"})).expect("send after a restart");
    assert_eq!(owner_homes(), homes, "the store didn't change");
    // A DM the owner added themselves (an explicit add in the open group):
    // a message from that person leaves it there.
    let placed = format!("k2:connect:1a2b3c4d-0000-4000-8000-000000000001::{}", sid());
    owner_syncs(json!([
        {"kind": "home", "id": "h1", "name": "Home", "order": "V", "updatedAt": at},
        {"kind": "home", "id": "h2", "name": "Work", "order": "l", "updatedAt": at},
        {"kind": "person", "homeId": "h2", "address": format!("{ALEX}::{}", sid()), "label": "Alex", "order": "V", "updatedAt": at},
        {"kind": "person", "homeId": "h2", "address": placed, "label": "Sam", "order": "k", "updatedAt": at},
    ]));
    let before = owner_homes();
    call("/cli/home-chat/send", json!({"conv": "920", "text": "hi", "nonce": "p4"})).expect("send");
    assert_eq!(owner_homes(), before, "placed on purpose: not moved, not doubled, no People group made for it");
    done();
}

/// Switch off: nothing arrives.
#[test]
fn nothing_arrives_while_the_people_group_is_off() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api) = chat_box();
    let _off = k2_core::test_env::EnvVar::set(k2_core::homes_sync::PEOPLE_SWITCH_ENV, "off");
    owner_syncs(json!([{"kind": "home", "id": "h1", "name": "Home", "order": "V", "updatedAt": 1_000}]));
    k2_core::tool_hook::with_home_chat_db(|conn| k2_core::tool_hook::admin_channels::set_dm_channel(conn, ALEX, "k2:owner", "921", 1))
        .expect("DM");
    api.answer("/api/v1/channels/921/messages", 200, json!({"id": "1", "channel_id": "921"}));
    call("/cli/home-chat/send", json!({"conv": "921", "text": "hi", "nonce": "o1"})).expect("send");
    let homes = owner_homes();
    assert_eq!(homes.len(), 1, "no People group, no row: {homes:?}");
    done();
}

/// Someone adds the owner to a channel: it lands in the owner's People
/// group. A new channel Sew made with the owner in it (the ledger's sink)
/// lands there too.
#[test]
fn a_channel_someone_puts_you_in_lands_in_people() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, _api) = chat_box();
    owner_syncs(json!([{"kind": "home", "id": "h1", "name": "Home", "order": "V", "updatedAt": 1_000}]));
    k2_core::tool_hook::with_home_chat_db(|conn| {
        let row = k2_core::tool_hook::admin_channels::request_create(conn, "plans", false, ALEX, &[], 1)?;
        conn.execute("UPDATE tool_admin_chat_channels SET channel_id = '906', state = 'active' WHERE slug = ?1", [&row.slug])
            .map_err(|e| e.to_string())?;
        conn.execute("UPDATE tool_admin_chat_members SET state = 'in' WHERE slug = ?1", [&row.slug]).map_err(|e| e.to_string())?;
        Ok(())
    })
    .expect("Alex's live channel");
    let r = call("/cli/home-chat/channels/add", json!({"conv": "906", "ext": ["k2:owner"]})).expect("add");
    assert_eq!(r["results"][0]["outcome"], json!("added"), "{r}");
    let homes = owner_homes();
    let landed = rows_in(&homes, "people");
    assert_eq!(landed.len(), 1, "{homes:?}");
    assert_eq!(
        (landed[0]["kind"].clone(), landed[0]["address"].clone(), landed[0]["label"].clone()),
        (json!("channel"), json!(format!("906::{}", sid())), json!("plans"))
    );
    // Added again ("already"): nothing new.
    let r = call("/cli/home-chat/channels/add", json!({"conv": "906", "ext": ["k2:owner"]})).expect("add again");
    assert_eq!(r["results"][0]["outcome"], json!("already"), "{r}");
    assert_eq!(owner_homes(), homes);
    // A channel Sew just made with the owner in it, through the installed
    // sink (it lands off the ledger's thread).
    super::arrivals::install();
    k2_core::home_chat::channel_arrived(vec![k2_core::home_chat::ChannelArrival {
        ext: "k2:owner".into(),
        channel_id: "907".into(),
        name: "launch".into(),
    }]);
    let want = json!(format!("907::{}", sid()));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !rows_in(&owner_homes(), "people").iter().any(|r| r["address"] == want) {
        assert!(std::time::Instant::now() < deadline, "the new channel never landed: {:?}", owner_homes());
        std::thread::sleep(Duration::from_millis(50));
    }
    k2_core::home_chat::set_arrival_sink(None);
    let landed = rows_in(&owner_homes(), "people");
    assert_eq!(landed.len(), 2, "{landed:?}");
    let order = |a: &Value| landed.iter().find(|r| r["address"] == *a).expect("row")["order"].as_str().expect("order").to_string();
    assert!(order(&want) > order(&json!(format!("906::{}", sid()))), "appended after the first");
    done();
}

type ClientWs = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The next frame of `kind` on a client's events socket (others skipped).
async fn next_of(ws: &mut ClientWs, kind: &str) -> Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["type"] == kind {
                    return v;
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(other) => panic!("the socket ended before a {kind} frame: {other:?}"),
            Err(_) => panic!("no {kind} frame in 10 s"),
        }
    }
}

/// Open the real events socket handler for `ext` on a loopback port and
/// connect a client to it (the dispatcher's checks are the test's own).
async fn open_events(ext: &str, token: &str, owner: &str) -> (ClientWs, tokio::task::JoinHandle<()>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let (ext, token, owner) = (ext.to_string(), token.to_string(), owner.to_string());
    let server = tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        super::events_ws::serve(&mut s, ext, token, owner).await;
    });
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}{}", routes::EVENTS)).await.expect("the socket opens");
    (ws, server)
}

/// The bug (a Home DM call's callee never saw the ring): the callee is a
/// Voice & Chat only login. Its Home chat events socket is admitted; a
/// `call_ring` Sew sends the callee reaches that socket as `{type:
/// "call_ring", conv, d}`, and so does the ring's stop; a window that opens
/// mid-ring gets it in `hello`; Answer (`/cli/voice/join` and the call
/// engine's key routes) and Decline pass the role gate. None of this waits
/// for the voice rooms preview, which stays off.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_voice_chat_only_callee_gets_the_ring_on_its_socket_and_may_answer() {
    use crate::routes::route_policy::role_gate;
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    assert!(!k2_core::voice_preview::enabled(), "the voice rooms preview is off");
    const OWNER: &str = "owner-token-for-the-ring-test";
    k2_core::connect_users::add_user("callee", "a-long-password-3").expect("add_user");
    k2_core::connect_users::set_role("callee", Role::VoiceChat).expect("set_role");
    let tok = k2_core::connect_users::create_session("callee");
    let q = format!("token={tok}");

    // The socket and the answer and decline routes are on the role's allowlist.
    if let Some(r) = role_gate("GET", routes::EVENTS, &q, OWNER) {
        panic!("the events socket refuses the callee: {} {}", r.status, r.body);
    }
    for p in [
        crate::voice::routes::JOIN,
        crate::voice::routes::CALL_DECLINE,
        crate::voice::routes::CALL_LEAVE,
        crate::voice::routes::SESSION_KEYS_JOIN,
        crate::voice::routes::SESSION_KEY_PACKAGES,
        crate::voice::routes::SESSION_STATE,
    ] {
        if let Some(r) = role_gate("POST", p, &q, OWNER) {
            panic!("POST {p} refuses the callee: {} {}", r.status, r.body);
        }
    }
    // Home chat's own caller rule takes the login as a person.
    let c = people::caller(&q, None, OWNER).unwrap_or_else(|r| panic!("caller refused: {} {}", r.code, r.hint));
    assert_eq!(c.role, Role::VoiceChat);
    assert!(c.ext.starts_with("k2:connect:"), "{}", c.ext);

    let api = Arc::new(FakeApi::default());
    let gw = fake_gateway(ready_body()).await;
    set_deps(Some(deps_with(api.clone(), gw.addr.clone(), HomeChatFeatures::default())));

    let (mut ws, server) = open_events(&c.ext, &tok, OWNER).await;
    let hello = next_of(&mut ws, "hello").await;
    assert_eq!(hello["calls"], json!([]), "{hello}");
    let h = follower::get(&c.ext).expect("the socket holds the callee");
    until("the callee's follower is live", || h.status() == "live").await;

    // Sew rings the callee.
    let expires = (chrono::Utc::now() + chrono::Duration::seconds(45)).to_rfc3339();
    let ring = json!({"call_id": "77", "channel_id": "900", "started_by": "42", "ringing": true, "silent": false, "expires_at": expires});
    gw.push.send(json!({"op": 0, "t": "call_ring", "s": 2, "d": ring})).expect("the gateway socket is open");
    let f = next_of(&mut ws, "call_ring").await;
    assert_eq!(f["conv"], "900", "{f}");
    assert_eq!(f["d"]["call_id"], "77", "{f}");
    assert_eq!(f["d"]["ringing"], true, "{f}");
    assert_eq!(f["d"]["started_by"], "42", "{f}");

    // A second window opened mid-ring gets the ring in its hello.
    let (mut ws2, server2) = open_events(&c.ext, &tok, OWNER).await;
    let hello2 = next_of(&mut ws2, "hello").await;
    let calls = hello2["calls"].as_array().expect("hello calls");
    assert_eq!(calls.len(), 1, "{hello2}");
    assert_eq!((calls[0]["callId"].clone(), calls[0]["conv"].clone(), calls[0]["ringing"].clone()), (json!("77"), json!("900"), json!(true)));

    // Answered on another device: the stop reaches both sockets.
    let stop = json!({"call_id": "77", "channel_id": "900", "ringing": false, "reason": "accepted"});
    gw.push.send(json!({"op": 0, "t": "call_ring", "s": 3, "d": stop})).expect("the gateway socket is open");
    for w in [&mut ws, &mut ws2] {
        let f = next_of(w, "call_ring").await;
        assert_eq!((f["d"]["ringing"].clone(), f["d"]["reason"].clone()), (json!(false), json!("accepted")), "{f}");
    }
    assert!(h.state.lock().unwrap().incoming_ring("900").is_none(), "no ring left");

    drop((ws, ws2));
    tokio::time::timeout(Duration::from_secs(5), server).await.expect("the first socket's handler ends").expect("join");
    tokio::time::timeout(Duration::from_secs(5), server2).await.expect("the second socket's handler ends").expect("join");
    follower::release_all();
    super::call_push::reset_for_tests();
    set_deps(None);
}

/// prd-home-call-recording-v1 §5 (HR-10): `replies` reads a recording
/// card's transcript lines through the person's own held session, oldest
/// first as Sew pages them; a line of another conversation is dropped; the
/// ids are Sew ids. It is a GET row on the Voice & Chat only allowlist
/// (either person in a DM reads it); agents never reach it (the caller
/// classification answers `agents_use_k2_chat` before any route runs).
#[test]
fn replies_read_a_recording_cards_lines_as_the_person() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api) = chat_box();
    assert!(routes::GET_ROUTES.contains(&"/cli/home-chat/replies") && !routes::is_post_route("/cli/home-chat/replies"));
    let q = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<_, _>>();
    api.answer(
        "/api/v1/messages/2001/replies?limit=100",
        200,
        json!({"items": [
            {"id": "2002", "channel_id": "900", "author_id": "42", "body": "hello", "thread_root_id": "2001", "transcript": {"recording_id": "42"}},
            {"id": "2003", "channel_id": 900, "author_id": "41", "body": "hi", "thread_root_id": "2001", "transcript": {"recording_id": "42"}},
            {"id": "2999", "channel_id": "901", "author_id": "43", "body": "not this DM's", "thread_root_id": "2001"},
        ], "has_more": false}),
    );
    let v = routes::route("/cli/home-chat/replies", &alex(), &q(&[("conv", "900"), ("id", "2001")]), b"").expect("replies");
    let ids: Vec<&str> = v["page"]["items"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["2002", "2003"], "only this DM's lines");
    assert_eq!(v["page"]["has_more"], false);
    let sent = api.calls_to("/api/v1/messages/2001/replies?limit=100");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0.as_deref(), Some("sews_first.x"), "on the person's own held session");
    api.answer("/api/v1/messages/2001/replies?limit=50&after=2003", 200, json!({"items": [], "has_more": false}));
    routes::route("/cli/home-chat/replies", &alex(), &q(&[("conv", "900"), ("id", "2001"), ("limit", "50"), ("after", "2003")]), b"").expect("the next page");
    for bad in [&[("id", "2001")][..], &[("conv", "900")], &[("conv", "900"), ("id", "../2001")], &[("conv", "../900"), ("id", "2001")], &[("conv", "900"), ("id", "2001"), ("limit", "0")]] {
        let r = routes::route("/cli/home-chat/replies", &alex(), &q(bad), b"").unwrap_err();
        assert_eq!((r.status, r.code.as_str()), (400, "usage"), "{bad:?}");
    }
    let r = people::caller_of(Who::Agent { project_id: "p".into(), session_id: "s".into() }, "x", "o").unwrap_err();
    assert_eq!(r.code, "agents_use_k2_chat", "no agent reads a DM recording's lines");
    assert!(crate::routes::route_policy::VOICE_CHAT_ALLOWLIST.contains(&("GET", "/cli/home-chat/replies")));
    done();
}
