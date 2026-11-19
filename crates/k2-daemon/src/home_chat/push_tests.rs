//! Home chat pushes (prd-home-chat-pushes-v1; vs-live HC-24's in-process
//! seam): a fake Sew person feed through the real poller (`poll_once`), the
//! rules with a fake env, the sealed push opened with a test key, the stored
//! cursor, the notify and push-prefs routes, and the Voice & Chat only
//! role's rows. Each test fails loudly.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::call_push::{self, Poll};
use super::chat_push::{self, tests::Fake};
use super::tests::{FakeApi, ALEX, SERIAL};
use super::{people, routes, set_deps, Deps};
use k2_core::connect_users::Role;
use k2_core::push::{HomeChatReason, PushEvent};
use k2_core::tool_hook::features::{HomeChatFeatures, HomePushFeatures};

const SECRET: &str = chat_push::tests::SECRET;

/// A fake Sew feed: every item with a seq above `after`, `next` the last.
#[derive(Default)]
struct Feed {
    items: Mutex<Vec<Value>>,
    /// Answer this once instead (a 410, say).
    once: Mutex<Option<(u16, Value)>>,
    reads: Mutex<Vec<u64>>,
}

impl Feed {
    fn page(&self, after: u64) -> (u16, Value) {
        self.reads.lock().unwrap().push(after);
        if let Some(a) = self.once.lock().unwrap().take() {
            return a;
        }
        let items: Vec<Value> = self.items.lock().unwrap().iter().filter(|i| i["seq"].as_u64().unwrap() > after).cloned().collect();
        let next = items.last().and_then(|i| i["seq"].as_u64()).unwrap_or(after);
        (200, json!({"items": items, "next": next}))
    }
}

type Mgmt = Arc<Mutex<Vec<(String, String, Option<Value>)>>>;

fn deps(api: Arc<FakeApi>, feed: Arc<Feed>, flags: HomePushFeatures, mgmt: Mgmt, mgmt_answer: Value) -> Arc<Deps> {
    let d = super::tests::deps_with(api, "127.0.0.1:9".into(), HomeChatFeatures::default());
    let mut d = Arc::try_unwrap(d).unwrap_or_else(|_| panic!("fresh deps"));
    d.feed = Arc::new(move |_ext: &str, after: u64, _wait: u64| Ok(feed.page(after)));
    d.devices = Arc::new(|_ext: &str| Some(("hc-push-person".to_string(), vec!["hc-push-login-nobody".to_string()])));
    d.push_flags = Arc::new(move || flags);
    d.mgmt = Arc::new(move |m: &str, p: &str, b: Option<&Value>| {
        mgmt.lock().unwrap().push((m.to_string(), p.to_string(), b.cloned()));
        Ok((200, mgmt_answer.clone()))
    });
    Arc::new(d)
}

/// skin.db made, the fake Sew, the fake env, nothing kept from before.
fn push_box(flags: HomePushFeatures) -> (k2_core::test_env::TempHome, Arc<FakeApi>, Arc<Feed>, Mgmt, Arc<Fake>) {
    let home = k2_core::test_env::TempHome::new();
    k2_core::skin::list_principals().expect("skin.db");
    let api = Arc::new(FakeApi::default());
    let feed = Arc::new(Feed::default());
    let mgmt: Mgmt = Default::default();
    set_deps(Some(deps(api.clone(), feed.clone(), flags, mgmt.clone(), json!({"head": 40}))));
    let env = Arc::new(Fake::default());
    chat_push::set_env_for_tests(Some(env.clone() as Arc<dyn chat_push::Env>));
    call_push::reset_for_tests();
    (home, api, feed, mgmt, env)
}

fn finish() {
    super::follower::release_all();
    chat_push::set_env_for_tests(None);
    call_push::reset_for_tests();
    set_deps(None);
}

fn now_rfc(ago: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::seconds(ago)).to_rfc3339()
}

fn dm_item(seq: u64, msg: &str, ago: i64) -> Value {
    json!({"seq": seq, "kind": "dm", "at": now_rfc(ago), "space_id": null, "channel_id": "900", "message_id": msg, "from_member_id": "42", "app_id": null, "participant_app_ids": [], "preview": SECRET})
}

fn stored_cursor(ext: &str) -> Option<u64> {
    k2_core::tool_hook::with_home_chat_db(|c| k2_core::tool_hook::home_push::cursor(c, ext)).expect("skin.db")
}

/// The whole path: a fake Sew `dm` item through the real poller and the
/// rules becomes one `home_chat` push with §5's fields; sealed for a test
/// key it opens to the text, and the text is nowhere in the clear (HC-8):
/// the generic lane is exactly `K2` / `New message`. An unknown kind is
/// ignored; the call ring on the same page still rings.
#[test]
fn a_fake_feed_item_becomes_one_sealed_push() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, _api, feed, _mgmt, env) = push_box(HomePushFeatures::default());
    let mark = crate::push_routes::test_push_capture::mark();
    feed.items.lock().unwrap().extend([
        dm_item(1, "880001", 2),
        json!({"seq": 2, "kind": "brand_new_kind", "at": now_rfc(1), "channel_id": "900", "message_id": "880002", "from_member_id": "42"}),
        json!({"seq": 3, "kind": "call_ring", "at": now_rfc(1), "channel_id": "900", "call_id": "77", "from_member_id": "42"}),
    ]);
    let mut cursor = call_push::initial_cursor(ALEX);
    assert_eq!(cursor, 0, "no stored cursor and no feed_head: from 0");
    assert_eq!(call_push::poll_once(ALEX, &mut cursor).expect("a page"), Poll::Again(2), "the ring and the DM");
    assert_eq!(cursor, 3);
    let sent = env.sent();
    assert_eq!(sent.len(), 1, "one home_chat push");
    let h = &sent[0];
    assert_eq!((h.reason, h.conversation_id.as_str(), h.message_id.as_str(), h.count, h.dm), (HomeChatReason::Dm, "900", "880001", 1, true));
    let rings: Vec<PushEvent> = crate::push_routes::test_push_capture::since(mark).into_iter().filter(|e| matches!(e, PushEvent::Call(c) if c.call_id == "77")).collect();
    assert_eq!(rings.len(), 1, "the fresh ring rang");

    let event = PushEvent::HomeChat(h.clone());
    let data = event.data();
    for (k, v) in [("kind", "home_chat"), ("reason", "dm"), ("conversationId", "900"), ("convKind", "dm"), ("messageId", "880001"), ("count", "1"), ("mention", "0"), ("senderMemberId", "42"), ("senderName", "Sam Lee"), ("avatarRev", "rev-sam-hash"), ("homeUnread", "2")] {
        assert_eq!(data.get(k), Some(v), "data.{k}");
    }
    assert!(data.get("channelName").is_none(), "a DM has no channel name");
    // Seal it for a phone and open it.
    let (sk, pk) = k2_core::push::seal::derive_keypair(&[9; 32]);
    let psk = vec![7u8; 32];
    let ctx = k2_core::push::e2e::SealCtx { kid: k2_core::push::seal::key_id(&pk), ctr: 1, ts: 1, stamp: Default::default(), badge_key: "bk".into() };
    let plain = k2_core::push::e2e::alert_plaintext(&event, &ctx).expect("sealable");
    let fitted = k2_core::push::e2e::fit_alert(&plain).expect("fits format 1");
    let blob = k2_core::push::seal::seal_in(k2_core::push::seal::SealKind::Alert, k2_core::push::seal::Format::V1, &pk, &psk, &fitted).expect("sealed");
    assert!(!blob.contains(SECRET));
    let opened: Value = serde_json::from_slice(&k2_core::push::seal::open(k2_core::push::seal::SealKind::Alert, &sk, &psk, &blob).expect("opens")).expect("json");
    assert_eq!((opened["kind"].clone(), opened["title"].clone(), opened["body"].clone()), (json!("home_chat"), json!("Sam Lee"), json!(SECRET)));
    assert_eq!(opened["group"], "hc:900");
    assert_eq!(opened["msg"]["text"], SECRET);
    // The generic lane: no names, no ids, no text.
    let generic = k2_core::push::e2e::generic_payload(&event, &Default::default(), "bk").to_string();
    for leak in [SECRET, "Sam Lee", "880001", "\"900\"", "42"] {
        assert!(!generic.contains(leak), "{leak} in {generic}");
    }
    assert!(!format!("{:?}", call_push::parse_feed(&json!({"items": [dm_item(9, "1", 1)], "next": 9})).0).contains(SECRET), "Debug hides the text");
    finish();
}

/// HC-2: the cursor is stored after a page; a restarted poller reads it
/// and sends nothing for what it already read (no old ring, no old
/// message); a first start with Sew's `feed_head` starts at the head; a
/// 410 whose head is below K2's cursor starts at the head (a restored Sew);
/// a Sew data reset forgets every cursor.
#[test]
fn the_cursor_survives_a_restart_and_a_reset_forgets_it() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, _api, feed, _mgmt, env) = push_box(HomePushFeatures::default());
    feed.items.lock().unwrap().extend([
        json!({"seq": 1, "kind": "call_ring", "at": now_rfc(1), "channel_id": "900", "call_id": "71", "from_member_id": "42"}),
        dm_item(2, "880010", 1),
    ]);
    let mut cursor = call_push::initial_cursor(ALEX);
    call_push::poll_once(ALEX, &mut cursor).expect("a page");
    assert_eq!(stored_cursor(ALEX), Some(2), "stored once the pushes went");
    assert_eq!(env.sent().len(), 1);
    // A daemon restart: memory gone, the row kept.
    call_push::reset_for_tests();
    let env2 = Arc::new(Fake::default());
    chat_push::set_env_for_tests(Some(env2.clone() as Arc<dyn chat_push::Env>));
    let mark = crate::push_routes::test_push_capture::mark();
    let mut cursor = call_push::initial_cursor(ALEX);
    assert_eq!(cursor, 2, "the stored cursor");
    assert_eq!(call_push::poll_once(ALEX, &mut cursor).expect("a page"), Poll::Again(0));
    assert!(env2.sent().is_empty(), "nothing re-fired");
    assert!(crate::push_routes::test_push_capture::since(mark).iter().all(|e| !matches!(e, PushEvent::Call(c) if c.call_id == "71")), "no old ring");
    assert_eq!(feed.reads.lock().unwrap().last(), Some(&2), "read from the stored cursor");

    // 410 with a head below the cursor: Sew went back.
    *feed.once.lock().unwrap() = Some((410, json!({"error": {"code": "cursor_expired", "details": {"oldest_seq": 1, "head": 1}}})));
    let mut cursor = 2;
    assert_eq!(call_push::poll_once(ALEX, &mut cursor).expect("a 410"), Poll::Again(0));
    assert_eq!((cursor, stored_cursor(ALEX)), (1, Some(1)), "start at the head");
    // 410 with the head above: resume at oldest_seq.
    *feed.once.lock().unwrap() = Some((410, json!({"error": {"code": "cursor_expired", "details": {"oldest_seq": 5, "head": 9}}})));
    let mut cursor = 2;
    call_push::poll_once(ALEX, &mut cursor).expect("a 410");
    assert_eq!(cursor, 5, "oldest_seq");

    // A Sew data reset forgets every cursor.
    super::sew_data_reset("a test rollback");
    assert_eq!(stored_cursor(ALEX), None, "forgotten");
    finish();
}

/// The flags a Sew 0.10.6 box reports (its published build's own list).
fn sew_0_10_6() -> HomePushFeatures {
    let f = HomePushFeatures::from_flags(&k2_core::tool_hook::features::SEW_0_10_6_REPORTED);
    assert_eq!(f, HomePushFeatures { feed_head: true, person_channel_notify: true, ..Default::default() }, "0.10.6's two push gates");
    f
}

/// P2 (Sew 0.10.6, `feed_head`): a person K2 never read starts at Sew's
/// head, so nothing retained is replayed.
#[test]
fn a_first_start_with_feed_head_begins_at_the_head() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, _api, _feed, mgmt, _env) = push_box(sew_0_10_6());
    assert_eq!(call_push::initial_cursor(ALEX), 40);
    let calls = mgmt.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "GET");
    assert!(calls[0].1.starts_with("/mgmt/v1/people/") && calls[0].1.ends_with("/notifications/head"), "{}", calls[0].1);
    assert_eq!(stored_cursor(ALEX), Some(40));
    finish();
}

/// A 20-minute-old message and a stale ring never alert loudly after a
/// restart from 0 (HC-3, HC-4): the message is one passive summary, the ring
/// stays quiet and its fresh `call_missed` gives one "Missed call".
#[test]
fn a_backlog_from_zero_stays_quiet() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, _api, feed, _mgmt, env) = push_box(HomePushFeatures::default());
    let mark = crate::push_routes::test_push_capture::mark();
    feed.items.lock().unwrap().extend([
        dm_item(1, "880020", 1200),
        json!({"seq": 2, "kind": "call_ring", "at": now_rfc(300), "channel_id": "900", "call_id": "72", "from_member_id": "42"}),
        json!({"seq": 3, "kind": "call_missed", "at": now_rfc(250), "channel_id": "900", "call_id": "72", "from_member_id": "42"}),
        json!({"seq": 4, "kind": "call_missed", "at": now_rfc(86_400), "channel_id": "900", "call_id": "73", "from_member_id": "42"}),
    ]);
    let mut cursor = 0;
    call_push::poll_once(ALEX, &mut cursor).expect("a page");
    let s = env.sent();
    assert_eq!(s.len(), 1);
    assert!(s[0].passive, "an old message: a passive summary");
    let calls: Vec<k2_core::push::CallPush> = crate::push_routes::test_push_capture::since(mark)
        .into_iter()
        .filter_map(|e| match e {
            PushEvent::Call(c) if c.call_id == "72" || c.call_id == "73" => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(calls.iter().map(|c| (c.call_id.as_str(), c.state)).collect::<Vec<_>>(), vec![("72", k2_core::push::CallPushState::Missed)], "no ring; one Missed call; the day-old one nothing");
    finish();
}

/// `deliver_home_chat` (the live send): the capture sink gets it, and the
/// badge path notes the alert as `h` at its message id, so a later read
/// sends a clear hint (HC-11). A non-numeric id notes nothing.
#[test]
fn a_delivered_alert_is_noted_for_read_clear() {
    let mut h = match chat_push_event("880031") {
        PushEvent::HomeChat(h) => h,
        _ => unreachable!(),
    };
    h.conversation_id = "hc-note-901".into();
    let mark = crate::push_routes::test_push_capture::mark();
    crate::push_routes::deliver_home_chat(PushEvent::HomeChat(h.clone()));
    assert_eq!(crate::push_routes::test_push_capture::since(mark).iter().filter(|e| matches!(e, PushEvent::HomeChat(x) if x.conversation_id == "hc-note-901")).count(), 1);
    let notes = crate::badge_push::test_read_capture::alerts_for("hc-note-901");
    assert_eq!(notes.len(), 1);
    assert_eq!((notes[0].1, notes[0].3), (k2_core::push::ReadKind::Home, 880031));
    h.conversation_id = "hc-note-902".into();
    h.message_id = "not-a-sew-id".into();
    crate::push_routes::deliver_home_chat(PushEvent::HomeChat(h));
    assert!(crate::badge_push::test_read_capture::alerts_for("hc-note-902").is_empty(), "refused");
}

fn chat_push_event(msg: &str) -> PushEvent {
    PushEvent::HomeChat(k2_core::push::HomeChatPush {
        principal: "me".into(),
        device_usernames: vec!["me".into()],
        reason: HomeChatReason::Mention,
        conversation_id: "901".into(),
        dm: false,
        message_id: msg.into(),
        count: 1,
        mention: true,
        sender_ext: "k2:connect:sam".into(),
        sender_member_id: "42".into(),
        sender_name: "Sam Lee".into(),
        avatar_rev: String::new(),
        channel_name: "standup".into(),
        unread_total: "3".into(),
        waiting_total: "1".into(),
        home_unread: "2".into(),
        preview: String::new(),
        msg: None,
        passive: false,
    })
}

fn member() -> people::Caller {
    people::Caller { ext: ALEX.into(), login: "alex".into(), role: Role::Member }
}

fn q(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// §7: GET notify reads the person's level on their own session; POST
/// notify/set takes All messages or @mentions only and sets it there; Off
/// and anything else is 400; a DM is fixed (GET) and 409 (POST); Sew's own
/// refusal passes through; GET on notify/set is 405 (HC-14).
#[test]
fn notify_levels_on_the_persons_session() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api, _feed, mgmt, _env) = push_box(HomePushFeatures::default());
    api.answer("/api/v1/channels/901/notify", 200, json!({"channel_id": "901", "level": null, "effective_level": "mentions", "muted": false}));
    let v = routes::route("/cli/home-chat/notify", &member(), &q(&[("conv", "901")]), b"").expect("GET notify");
    assert_eq!(
        (v["convKind"].clone(), v["level"].clone(), v["effective"].clone(), v["inherited"].clone(), v["fixed"].clone(), v["allMessagesLive"].clone()),
        (json!("channel"), json!("mentions"), json!("mentions"), json!(true), json!(false), json!(false))
    );
    assert_eq!(api.calls_to("/api/v1/channels/901/notify")[0].0.as_deref(), Some("sews_first.x"), "on the person's own session");
    api.answer("/api/v1/channels/902/notify", 200, json!({"channel_id": "902", "level": "none", "effective_level": "none", "muted": false}));
    let v = routes::route("/cli/home-chat/notify", &member(), &q(&[("conv", "902")]), b"").expect("GET notify");
    assert_eq!((v["level"].clone(), v["inherited"].clone()), (json!("none"), json!(false)), "Off, set elsewhere, shows as such");

    api.answer("/api/v1/channels/901/notify/set", 200, json!({"channel_id": "901", "level": "all", "effective_level": "all", "muted": false}));
    let v = routes::route("/cli/home-chat/notify/set", &member(), &HashMap::new(), br#"{"conv":"901","level":"all"}"#).expect("set");
    assert_eq!((v["level"].clone(), v["inherited"].clone()), (json!("all"), json!(false)));
    assert_eq!(api.calls_to("/api/v1/channels/901/notify/set")[0].1, json!({"level": "all"}), "only the level: mute untouched");
    for bad in ["off", "none", "everything", ""] {
        let body = json!({"conv": "901", "level": bad}).to_string();
        let r = routes::route("/cli/home-chat/notify/set", &member(), &HashMap::new(), body.as_bytes()).unwrap_err();
        assert_eq!((r.status, r.code.as_str()), (400, "usage"), "{bad}");
    }
    assert!(mgmt.lock().unwrap().is_empty(), "no mgmt before person_channel_notify");
    // Not a member: Sew's refusal.
    api.answer("/api/v1/channels/903/notify/set", 422, json!({"error": {"code": "invalid_state", "hint": "not a member", "details": {"reason": "not_a_member"}}}));
    let r = routes::route("/cli/home-chat/notify/set", &member(), &HashMap::new(), br#"{"conv":"903","level":"mentions"}"#).unwrap_err();
    assert_eq!(r.status, 422);
    // A DM.
    k2_core::tool_hook::with_home_chat_db(|c| k2_core::tool_hook::admin_channels::set_dm_channel(c, ALEX, "k2:owner", "900", 1)).expect("a DM");
    let r = routes::route("/cli/home-chat/notify/set", &member(), &HashMap::new(), br#"{"conv":"900","level":"mentions"}"#).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (409, "dm_always_notifies"));
    api.answer("/api/v1/channels/900/notify", 200, json!({"channel_id": "900", "level": null, "effective_level": "all", "muted": false}));
    let v = routes::route("/cli/home-chat/notify", &member(), &q(&[("conv", "900")]), b"").expect("GET a DM");
    assert_eq!((v["convKind"].clone(), v["level"].clone(), v["fixed"].clone()), (json!("dm"), json!("all"), json!(true)));
    // HC-14: GET on a POST row is 405; the GET row is a GET.
    let r = routes::handle("/cli/home-chat/notify/set", Ok(member()), false, &HashMap::new(), b"");
    assert!(r.status.starts_with("405"), "{}", r.status);
    assert!(!routes::is_post_route("/cli/home-chat/notify") && !routes::is_post_route("/cli/home-chat/push-prefs"));
    finish();
}

/// P2 (Sew 0.10.6, `person_channel_notify`): the level goes over mgmt for
/// the person, never their session: read at `GET
/// /mgmt/v1/people/{ext}/channels/{id}/notify`, set at `POST …/notify/set`
/// (sew-proto 0.10.6's `MGMT_PERSON_CHANNEL_NOTIFY[_SET]`).
#[test]
fn notify_goes_over_mgmt_with_person_channel_notify() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, api, _feed, mgmt, _env) = push_box(sew_0_10_6());
    let v = routes::route("/cli/home-chat/notify", &member(), &q(&[("conv", "901")]), b"").expect("GET notify");
    assert_eq!(v["convKind"], json!("channel"));
    let v = routes::route("/cli/home-chat/notify/set", &member(), &HashMap::new(), br#"{"conv":"901","level":"mentions"}"#).expect("set");
    assert_eq!(v["ok"], true);
    let calls = mgmt.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "GET");
    assert!(calls[0].1.starts_with("/mgmt/v1/people/") && calls[0].1.ends_with("/channels/901/notify"), "{}", calls[0].1);
    assert_eq!(calls[0].2, None);
    assert_eq!(calls[1].0, "POST");
    assert!(calls[1].1.starts_with("/mgmt/v1/people/") && calls[1].1.ends_with("/channels/901/notify/set"), "{}", calls[1].1);
    assert_eq!(calls[1].2, Some(json!({"level": "mentions"})));
    assert!(api.calls_to("/api/v1/channels/901/notify").is_empty(), "not the session");
    assert!(api.calls_to("/api/v1/channels/901/notify/set").is_empty(), "not the session");
    finish();
}

/// push-prefs: the person's own switches (defaults: text on, agents off);
/// `on` needs the owner's switch, the bell and a phone (HC-18).
#[test]
fn push_prefs_are_the_persons_own() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (_home, _api, _feed, _mgmt, _env) = push_box(HomePushFeatures::default());
    let v = routes::route("/cli/home-chat/push-prefs", &member(), &HashMap::new(), b"").expect("GET");
    assert_eq!((v["previews"].clone(), v["agents"].clone(), v["on"].clone(), v["devices"].clone()), (json!(true), json!(false), json!(false), json!(0)));
    let v = routes::route("/cli/home-chat/push-prefs/set", &member(), &HashMap::new(), br#"{"agents":true}"#).expect("set");
    assert_eq!((v["previews"].clone(), v["agents"].clone()), (json!(true), json!(true)));
    let v = routes::route("/cli/home-chat/push-prefs/set", &member(), &HashMap::new(), br#"{"previews":false}"#).expect("set");
    assert_eq!((v["previews"].clone(), v["agents"].clone()), (json!(false), json!(true)));
    for bad in [&b"{}"[..], &br#"{"bell":false}"#[..]] {
        let r = routes::route("/cli/home-chat/push-prefs/set", &member(), &HashMap::new(), bad).unwrap_err();
        assert_eq!(r.status, 400);
    }
    let other = people::Caller { ext: "k2:connect:someone-else".into(), login: "kim".into(), role: Role::Member };
    let v = routes::route("/cli/home-chat/push-prefs", &other, &HashMap::new(), b"").expect("GET");
    assert_eq!(v["agents"], false, "another person's choices are their own");
    finish();
}

/// D3: a Voice & Chat only login reaches all four routes and the phone's
/// own push rows (register, unregister, device) at the role gate; the
/// allowlist and the floor agree (the route_policy test walks every row).
#[test]
fn a_voice_chat_only_login_reaches_the_push_rows() {
    use crate::routes::route_policy::{role_gate, VOICE_CHAT_ALLOWLIST};
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    const OWNER: &str = "owner-token-for-the-push-rows";
    k2_core::connect_users::add_user("vc-phone", "a-long-password-3").expect("add_user");
    k2_core::connect_users::set_role("vc-phone", Role::VoiceChat).expect("set_role");
    let tok = k2_core::connect_users::create_session("vc-phone");
    let query = format!("token={tok}");
    for (m, p) in [
        ("GET", "/cli/home-chat/notify"),
        ("POST", "/cli/home-chat/notify/set"),
        ("GET", "/cli/home-chat/push-prefs"),
        ("POST", "/cli/home-chat/push-prefs/set"),
        ("GET", "/cli/push/device"),
        ("POST", "/cli/push/register-device"),
        ("POST", "/cli/push/unregister-device"),
    ] {
        assert!(VOICE_CHAT_ALLOWLIST.contains(&(m, p)), "{m} {p} on the allowlist");
        if let Some(r) = role_gate(m, p, &query, OWNER) {
            panic!("{m} {p} refuses a Voice & Chat only login: {} {}", r.status, r.body);
        }
    }
    // Still not Thread or ticket rows.
    assert!(role_gate("GET", "/cli/push/devices", &query, OWNER).is_some(), "the phone list stays the owner's");
}

/// HC-12: a Voice & Chat only login's phone gets Home chat badge pushes
/// (its bell devices count it), its Thread totals are zero, and
/// `device_person` still gives it no Thread, ticket or project push.
#[test]
fn a_voice_chat_only_phone_gets_home_badges_and_nothing_else() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    k2_core::connect_users::add_user("vc-badge", "a-long-password-3").expect("add_user");
    k2_core::connect_users::set_role("vc-badge", Role::VoiceChat).expect("set_role");
    k2_core::push_devices::upsert_device_e2e("vc-badge-phone", "apns", "tok-vc-badge", "vc-badge", Some(true), None, k2_core::push_device_keys::E2eChange::Keep)
        .expect("register");
    let people = k2_core::notify_prefs::People::load().expect("people");
    assert_eq!(people.device_person("vc-badge"), None, "no Thread, ticket or project push");
    assert_eq!(crate::badge_push::person_bell_devices("vc-badge"), vec!["vc-badge".to_string()], "Home chat badge pushes");
    let t = crate::badge_push::totals_for("vc-badge").expect("totals");
    assert_eq!((t.total, t.waiting_total), (0, 0), "Home chat only");
    k2_core::push_devices::remove_device_because("vc-badge-phone", k2_core::push::device_log::Reason::PhoneOff).expect("cleanup");
}

/// Companion's follow-up (prd-home-chat-pushes-v1 §5): `GET
/// /cli/home-chat/people` carries each person's picture rev, the same value
/// a Home chat push names as `avatarRev` (the stored picture's hash, the
/// `imageHash` of `GET /cli/people/picture`), and `null` for a person with
/// no picture.
#[test]
fn people_rows_and_pushes_carry_the_same_picture_rev() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    use k2_core::person_picture;
    k2_core::connect_users::add_user("pic-sam", "a-long-password-3").expect("add_user");
    k2_core::connect_users::add_user("pic-kim", "a-long-password-3").expect("add_user");
    let sam_id = k2_core::connect_users::user_uuid("pic-sam").expect("uuid").expect("an id");
    let kim_id = k2_core::connect_users::user_uuid("pic-kim").expect("uuid").expect("an id");
    let sam_ext = format!("k2:connect:{sam_id}");
    let sam_key = format!("{}{sam_id}", person_picture::CONNECT_PREFIX);
    let (owner_rev, sam_rev) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (
            person_picture::set(&conn, person_picture::OWNER_KEY, b"jpeg-owner-rev-test", "owner").expect("owner picture"),
            person_picture::set(&conn, &sam_key, b"jpeg-sam-rev-test", "pic-sam").expect("sam picture"),
        )
    };
    let rows = people::list(&member()).expect("people");
    let rev = |ext: &str| rows.iter().find(|r| r["ext"] == ext).unwrap_or_else(|| panic!("no row for {ext}: {rows:?}"))["pictureRev"].clone();
    assert_eq!(rev("k2:owner"), json!(owner_rev));
    assert_eq!(rev(&sam_ext), json!(sam_rev));
    assert_eq!(rev(&format!("k2:connect:{kim_id}")), Value::Null, "no picture: null");
    assert_eq!(people::picture_rev(&sam_ext), Some(sam_rev.clone()));
    // The push names the same rev (the live env's lookup).
    use super::chat_push::Env as _;
    assert_eq!(chat_push::LiveEnv.avatar_rev(&sam_ext), sam_rev);
    assert_eq!(chat_push::LiveEnv.avatar_rev(&format!("k2:connect:{kim_id}")), "", "empty only without a picture");
    let db = k2_core::db::shared();
    let conn = db.lock();
    person_picture::clear(&conn, person_picture::OWNER_KEY).expect("cleanup");
    person_picture::clear(&conn, &sam_key).expect("cleanup");
}
