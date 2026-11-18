//! DM calls (prd-home-people-channels-v1 §11a; S11–S14) against a fake Sew:
//! the door routes, the ring state and its fan-out, the missed-call badge,
//! and the phone pushes from the person feed. Each test fails loudly; none
//! touches `~/.k2`, the keychain or a daemon.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use k2_core::connect_users::Role;
use k2_core::push::{CallPushState, PushEvent};
use k2_core::tool_hook::features::HomeChatFeatures;
use k2_core::tool_hook::sew_wire::ExternalId;
use k2_core::tool_hook::ConnectChatGate;
use k2_core::voice_rooms::Hosting as Tier;

use super::*;
use crate::home_chat::follower;
use crate::home_chat::state::{Conv, PersonState};
use crate::home_chat::tests::{deps_with, FakeApi, SERIAL};
use crate::voice::bff::{Client, Presence, SewAnswer, SewApi};
use crate::voice::routes::{self, Caller};

const UUID_SAM: &str = "5a6b7c8d-0000-4000-8000-000000000031";

fn sam() -> Person {
    let ident = crate::chat_pass::identity::connect_identity("sam", Role::VoiceChat, UUID_SAM, "cu_dmcall_sam", false);
    let ext = ExternalId::parse(&ident.sub).unwrap();
    Person { ident, ext, login: "sam".into(), role: Role::VoiceChat, client: Client::Login }
}

/// The login session's own Sew (the BFF's leave goes there).
#[derive(Default)]
struct LoginSew {
    calls: Mutex<Vec<String>>,
}

impl SewApi for LoginSew {
    fn post(&self, _bearer: Option<&str>, path: &str, _body: &Value) -> Result<SewAnswer, String> {
        self.calls.lock().unwrap().push(path.to_string());
        match path {
            bff::paths::AUTH_K2 => Ok(SewAnswer { status: 200, body: json!({"token": "sews_login.x"}) }),
            bff::paths::ME_VOICE_LEAVE => Ok(SewAnswer { status: 200, body: json!({"ok": true}) }),
            _ => Err(format!("login sew: nothing at {path}")),
        }
    }
}

fn bff_deps(sew: Arc<LoginSew>) -> Arc<bff::Deps> {
    Arc::new(bff::Deps {
        sew,
        mint: Arc::new(|i: &crate::chat_pass::identity::PassIdentity| Ok::<_, String>(format!("pass-for-{}", i.sub))),
        connect_gate: Arc::new(|_: &str, _: &str| Ok::<_, String>(ConnectChatGate::Ready)),
        level_pending: Arc::new(|_: &ExternalId| Ok::<_, String>(false)),
        lane_hold: Duration::from_millis(200),
        member_wait: Duration::from_millis(100),
        presence: Arc::new(|_: &ExternalId| Presence { feed: "off".into(), listed: None }),
    })
}

fn ctx(call_cancel: bool, login: Arc<LoginSew>) -> Ctx {
    Ctx { chat: (true, None), tier: Tier::Ok, call_cancel, bff: bff_deps(login), audio_max_days: None }
}

/// Sam's held session knows DM 900 (Sew's READY listed it).
fn sam_in_dm_900() -> Arc<follower::Held> {
    let h = follower::hold(&sam().ext.to_string());
    let mut st = h.state.lock().unwrap();
    st.me = "41".into();
    st.convs.insert("900".into(), Conv { channel_id: "900".into(), dm: true, ..Default::default() });
    drop(st);
    h
}

fn call_join_answer() -> Value {
    json!({
        "call": {"id": "88", "channel_id": "900", "started_by": "41", "state": "active", "rings": []},
        "created": true,
        "voice": {"url": "wss://media.example.test/rtc", "token": "media-token", "e2ee": {"group": "g"}, "media": {"relay_only": true}},
        "recording": null,
    })
}

fn code(r: &Refusal) -> (u16, &str) {
    (r.status, r.code.as_str())
}

fn body_of(r: &crate::cli_response::CliResponse) -> Value {
    serde_json::from_str(&r.body).expect("json")
}

// ── the door ─────────────────────────────────────────────────────────

/// S11: every DM call route is POST-only, has its policy row, and is on the
/// Voice & Chat only allowlist (a reviewed edit). With the voice rooms
/// preview off they answer only while Home chat is on, and only for a DM
/// call; agents, the owner token and App passes get join's refusals.
#[test]
fn the_dm_call_door_is_post_only_allowlisted_and_people_only() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // The voice tests' switch lock: they turn the preview on and off too.
    let _l = super::super::lane::test_lock();
    let _home = k2_core::test_env::TempHome::new();
    use crate::routes::route_policy::{lookup, VOICE_CHAT_ALLOWLIST};
    for p in routes::CALL_ROUTES {
        assert!(routes::POST_ROUTES.contains(p) && routes::is_post_route(p), "{p}");
        assert!(lookup(p).is_some_and(|r| r.post.is_some() && r.get.is_none()), "{p} has a POST policy row");
        assert!(VOICE_CHAT_ALLOWLIST.contains(&("POST", *p)), "{p} is on the reviewed allowlist");
        assert!(routes::dm_capable(p));
    }
    for p in [routes::JOIN, routes::LEAVE, routes::SESSION_STATE] {
        assert!(routes::dm_capable(p), "{p}");
    }
    for p in [routes::SETTINGS, routes::MODERATE, routes::ROOMS, routes::RECORDING_SETTINGS] {
        assert!(!routes::dm_capable(p), "{p}");
    }

    k2_core::voice_preview::set_enabled(false);
    k2_core::home_chat::set_enabled(false);
    let st = |r: &crate::cli_response::CliResponse| r.status.split(' ').next().unwrap_or("").to_string();
    let err = |r: &crate::cli_response::CliResponse| body_of(r)["error"]["code"].as_str().unwrap_or("").to_string();
    let r = routes::handle(routes::CALL_RING, Caller::Passport, "o", true, br#"{"dm":"900","callId":"88"}"#);
    assert_eq!((st(&r), err(&r)), ("404".into(), "voice_rooms_off".into()), "both switches off: dark");

    k2_core::home_chat::set_enabled(true);
    for p in routes::CALL_ROUTES {
        assert_eq!(st(&routes::handle(p, Caller::Passport, "o", false, b"")), "405", "{p}: POST only");
    }
    let r = routes::handle(routes::SETTINGS, Caller::OwnerToken, "o", true, br#"{"count":1}"#);
    assert_eq!(err(&r), "voice_rooms_off", "a room route stays dark");
    let r = routes::handle(routes::JOIN, Caller::OwnerToken, "o", true, br#"{"room":"1"}"#);
    assert_eq!(err(&r), "voice_rooms_off", "a room join stays dark");
    let body = br#"{"dm":"900","callId":"88"}"#;
    for p in [routes::JOIN, routes::CALL_DECLINE, routes::CALL_RING, routes::CALL_CANCEL, routes::CALL_LEAVE] {
        let r = routes::handle(p, Caller::Passport, "o", true, body);
        assert_eq!((st(&r), err(&r)), ("403".into(), "agents_not_in_voice".into()), "{p}: agents never call");
        let r = routes::handle(p, Caller::ChatManager, "o", true, body);
        assert_eq!(err(&r), "agents_not_in_voice", "{p}: not even with Manage Chat");
        let r = routes::handle(p, Caller::OwnerToken, "o", true, body);
        assert_eq!((st(&r), err(&r)), ("403".into(), "owner_token_cannot_join".into()), "{p}");
        let r = routes::handle(p, Caller::App, "o", true, body);
        assert_eq!(err(&r), "app_pass_not_allowed", "{p}");
    }
    k2_core::home_chat::set_enabled(false);
}

/// S11 + HP-40: a join with no ring going rings; a join while a call rings
/// for the person answers it. K2 always asks for the call credential. The
/// answer unwraps `.voice` (with `call` beside it) unless the client's call
/// client takes `CallJoinResponse` as is (`callAnswer: true`); the room is
/// `dm:<channel id>`.
#[test]
fn a_dm_join_rings_or_answers_and_shapes_the_answer_by_client() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _l = super::super::lane::test_lock();
    let _home = k2_core::test_env::TempHome::new();
    let api = Arc::new(FakeApi::default());
    crate::home_chat::set_deps(Some(deps_with(api.clone(), "127.0.0.1:9".into(), HomeChatFeatures::default())));
    let h = sam_in_dm_900();
    api.answer("/api/v1/channels/900/call/join", 200, call_join_answer());
    let login = Arc::new(LoginSew::default());
    let c = ctx(false, login);

    let v = join(&sam(), &json!({"dm": "900", "join": {"self_mute": true}}), &c).expect("join");
    let sent = api.calls_to("/api/v1/channels/900/call/join");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0.as_deref(), Some("sews_first.x"), "on the person's held session");
    assert_eq!(sent[0].1, json!({"self_mute": true, "ring": true, "call_credential": true}), "starts the call, ringing");
    let mut want = call_join_answer()["voice"].clone();
    want["call"] = call_join_answer()["call"].clone();
    assert_eq!(v["join"], want, "unwrapped for a call client that takes a room's answer");
    assert_eq!(v["room"], json!({"id": "dm:900", "kind": "dm", "channelId": "900"}));
    assert_eq!(v["relayOnly"], json!(true));
    assert!(v["callId"].as_str().is_some_and(|id| id.starts_with("vc_")), "K2's call id for the leave");

    // A call rings for Sam in the DM: this join answers it.
    h.state.lock().unwrap().apply_call_event("call_ring", &json!({"call_id": "88", "channel_id": "900", "started_by": "42", "ringing": true, "silent": false}));
    let v = join(&sam(), &json!({"dm": "900", "callAnswer": true}), &c).expect("answer");
    let sent = api.calls_to("/api/v1/channels/900/call/join");
    assert_eq!(sent[1].1, json!({"ring": false, "call_credential": true}), "answers, never rings back");
    assert_eq!(v["join"], call_join_answer(), "passed through unchanged for a call client that takes it");

    // Not Sam's DM, a bad id, a bad flag.
    assert_eq!(code(&join(&sam(), &json!({"dm": "901"}), &c).unwrap_err()), (404, "dm_not_found"));
    assert_eq!(code(&join(&sam(), &json!({"dm": "../900"}), &c).unwrap_err()), (400, "usage"));
    assert_eq!(code(&join(&sam(), &json!({"dm": "900", "callAnswer": "yes"}), &c).unwrap_err()), (400, "usage"));
    // The gates: chat off here, then the Pro plan (the same gate as rooms).
    let off = Ctx { chat: (false, Some("sew_off")), ..c.clone() };
    assert_eq!(code(&join(&sam(), &json!({"dm": "900"}), &off).unwrap_err()), (503, "chat_unavailable"));
    let free = Ctx { tier: Tier::NotPro, ..c.clone() };
    assert!(DM_CALLS_NEED_PRO, "DM calls ride the rooms' Pro gate for now");
    assert_eq!(code(&join(&sam(), &json!({"dm": "900"}), &free).unwrap_err()), (403, "host_not_pro"));
    follower::release_all();
    crate::home_chat::set_deps(None);
}

#[test]
fn the_join_answer_unwraps_or_passes_through() {
    let a = call_join_answer();
    let unwrapped = join_answer(&a, false);
    assert_eq!(unwrapped["url"], "wss://media.example.test/rtc");
    assert_eq!(unwrapped["media"]["relay_only"], true);
    assert_eq!(unwrapped["call"]["id"], "88");
    assert!(unwrapped.get("voice").is_none() && unwrapped.get("created").is_none());
    assert_eq!(join_answer(&a, true), a);
    assert_eq!(join_answer(&json!({"call": {"id": "1"}}), false), json!({"call": {"id": "1"}}), "no voice: just the call");
}

/// Cancel goes to Sew's cancel only with `call_cancel`; without it the
/// caller leaves (Sew then ends the call `empty`, and the callee sees a
/// missed call) and Sew's cancel is never called.
#[test]
fn cancel_is_sews_only_with_the_flag_else_a_leave() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    let api = Arc::new(FakeApi::default());
    crate::home_chat::set_deps(Some(deps_with(api.clone(), "127.0.0.1:9".into(), HomeChatFeatures::default())));
    sam_in_dm_900();
    api.answer("/api/v1/channels/900/call/cancel", 200, json!({"ok": true}));

    let login = Arc::new(LoginSew::default());
    let v = cancel(&sam(), &json!({"dm": "900", "callId": "88"}), &ctx(true, login.clone())).expect("cancel");
    assert_eq!((v["cancelled"].clone(), v["via"].clone()), (json!(true), json!("cancel")));
    assert_eq!(api.calls_to("/api/v1/channels/900/call/cancel")[0].1, json!({"call_id": "88"}));
    assert!(!login.calls.lock().unwrap().iter().any(|p| p == bff::paths::ME_VOICE_LEAVE), "with the flag: no leave");

    let login = Arc::new(LoginSew::default());
    let v = cancel(&sam(), &json!({"dm": "900", "callId": "89"}), &ctx(false, login.clone())).expect("leave");
    assert_eq!((v["cancelled"].clone(), v["via"].clone(), v["left"].clone()), (json!(false), json!("leave"), json!(true)));
    assert!(login.calls.lock().unwrap().iter().any(|p| p == bff::paths::ME_VOICE_LEAVE), "without it: the caller leaves");
    assert_eq!(api.calls_to("/api/v1/channels/900/call/cancel").len(), 1, "Sew's cancel is never called without the flag");
    follower::release_all();
    crate::home_chat::set_deps(None);
}

/// Decline and re-ring go on the held session with Sew's bodies; a re-ring
/// may name members (Sew ids only).
#[test]
fn decline_and_ring_reach_sew_as_the_person() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    let api = Arc::new(FakeApi::default());
    crate::home_chat::set_deps(Some(deps_with(api.clone(), "127.0.0.1:9".into(), HomeChatFeatures::default())));
    sam_in_dm_900();
    api.answer("/api/v1/channels/900/call/decline", 200, json!({"ok": true}));
    api.answer("/api/v1/channels/900/call/ring", 200, json!({"ok": true}));
    let c = ctx(false, Arc::new(LoginSew::default()));
    decline(&sam(), &json!({"dm": "900", "callId": "88"}), &c).expect("decline");
    assert_eq!(api.calls_to("/api/v1/channels/900/call/decline")[0].1, json!({"call_id": "88"}));
    ring(&sam(), &json!({"dm": "900", "callId": "88", "memberIds": ["42"]}), &c).expect("ring");
    assert_eq!(api.calls_to("/api/v1/channels/900/call/ring")[0].1, json!({"call_id": "88", "member_ids": ["42"]}));
    assert_eq!(code(&ring(&sam(), &json!({"dm": "900", "callId": "88", "memberIds": ["x"]}), &c).unwrap_err()), (400, "usage"));
    assert_eq!(code(&decline(&sam(), &json!({"dm": "900"}), &c).unwrap_err()), (400, "usage"), "no call id");
    api.answer("/api/v1/channels/900/call/ring", 429, json!({"error": {"code": "rate_limited", "hint": "at most 3 rings"}}));
    assert_eq!(ring(&sam(), &json!({"dm": "900", "callId": "88"}), &c).unwrap_err().status, 429, "Sew's limit comes back as is");
    follower::release_all();
    crate::home_chat::set_deps(None);
}

// ── ringing (S12) ────────────────────────────────────────────────────

fn ringing_state() -> PersonState {
    let mut st = PersonState::default();
    st.apply_ready(&json!({
        "member": {"id": "41"},
        "channels": [{"id": "900", "kind": "dm"}],
        "read_states": [{"channel_id": "900", "unread": false, "mention_count": 0}],
    }));
    st
}

/// S12: `call_ring` reaches every client socket of the person as a
/// `call_ring` frame; the ring stops on each of Sew's reasons; an ended
/// `call_updated` drops the call; READY's calls show a ring that started
/// before the socket opened.
#[test]
fn rings_fan_out_and_stop_on_every_reason() {
    for reason in ["accepted", "declined", "timeout", "call_ended", "cancelled"] {
        let mut st = ringing_state();
        let f = st.apply_call_event("call_ring", &json!({"call_id": "88", "channel_id": "900", "started_by": "42", "ringing": true, "silent": true, "expires_at": "2026-11-14T12:00:45Z"}));
        assert_eq!(f, vec![json!({"type": "call_ring", "conv": "900", "d": {"call_id": "88", "channel_id": "900", "started_by": "42", "ringing": true, "silent": true, "expires_at": "2026-11-14T12:00:45Z"}})]);
        let r = st.incoming_ring("900").expect("rings");
        assert!(r.silent, "a silent ring stays silent");
        assert_eq!(st.calls_view()[0]["ringing"], json!(true));
        let f = st.apply_call_event("call_ring", &json!({"call_id": "88", "channel_id": "900", "ringing": false, "reason": reason}));
        assert_eq!(f[0]["type"], "call_ring");
        assert_eq!(f[0]["d"]["ringing"], json!(false));
        assert!(st.incoming_ring("900").is_none(), "{reason}: the ring stopped");
    }
    // The caller's view: call_updated carries the rings; ended drops it.
    let mut st = ringing_state();
    let call = json!({"id": "90", "channel_id": "900", "started_by": "41", "state": "active", "rings": [{"member_id": "42", "state": "ringing"}]});
    let f = st.apply_call_event("call_updated", &json!({"call": call}));
    assert_eq!((f[0]["type"].clone(), f[0]["conv"].clone()), (json!("call_updated"), json!("900")));
    assert!(st.incoming_ring("900").is_none(), "the caller's own call never rings for them");
    assert_eq!(st.calls_view().len(), 1);
    st.apply_call_event("call_updated", &json!({"call": {"id": "90", "channel_id": "900", "state": "ended", "end_reason": "cancelled"}}));
    assert!(st.calls_view().is_empty());
    // READY with a call ringing for this member.
    let mut st = PersonState::default();
    st.apply_ready(&json!({"member": {"id": "41"}, "channels": [{"id": "900", "kind": "dm"}],
        "calls": [{"id": "91", "channel_id": "900", "started_by": "42", "state": "active", "rings": [{"member_id": "41", "state": "busy"}]}]}));
    let r = st.incoming_ring("900").expect("READY's ring");
    assert!(r.silent, "busy is a silent ring");

    // Every window of the person gets the frames (one broadcast per person).
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let h = follower::hold("k2:connect:5a6b7c8d-0000-4000-8000-0000000000ff");
    let (mut a, mut b) = (h.events.subscribe(), h.events.subscribe());
    let frames = h.state.lock().unwrap().apply_call_event("call_ring", &json!({"call_id": "92", "channel_id": "900", "ringing": false, "reason": "accepted"}));
    h.publish(&[json!({"type": "call_ring", "conv": "900", "d": {"call_id": "92", "ringing": false, "reason": "accepted"}})]);
    assert!(frames.len() == 1);
    for rx in [&mut a, &mut b] {
        let got: Value = serde_json::from_str(&rx.try_recv().expect("each window hears it")).unwrap();
        assert_eq!(got["d"]["reason"], "accepted");
    }
    follower::release_all();
}

// ── in the DM (S13): the missed-call badge ───────────────────────────

fn record(call: &str, summary: Value) -> Value {
    json!({"message": {"id": format!("m{call}"), "channel_id": "900", "author_id": "42", "call": summary}})
}

/// S13: a call's record makes the DM unread but counts nothing at the
/// start; a missed call adds one to the DM's count, once; a decline and a
/// cancelled ring never count. The caller leaving without Sew's cancel is a
/// missed call (Sew 0.10.0's fallback), so it counts.
#[test]
fn a_missed_call_raises_the_dm_badge_once_and_a_cancel_never() {
    let mut st = ringing_state();
    let count = |st: &PersonState| st.convs["900"].mention_count;
    let started = json!({"call_id": "88", "started_by": "42", "rung_ids": ["41"], "joined_ids": ["42"], "declined_ids": [], "missed_ids": []});
    let f = st.apply_event("message_created", &record("88", started), false);
    assert!(f.iter().any(|x| x["type"] == "message"));
    assert!(st.convs["900"].unread, "unread from the call's start");
    assert_eq!(count(&st), 0, "nothing counted while it rings");
    let missed = json!({"call_id": "88", "started_by": "42", "end_reason": "no_answer", "duration_ms": null, "rung_ids": ["41"], "joined_ids": ["42"], "declined_ids": [], "missed_ids": ["41"]});
    let f = st.apply_event("message_updated", &record("88", missed.clone()), false);
    assert_eq!(f[0]["type"], "message_updated", "the record updates in place");
    assert!(f.iter().any(|x| x["type"] == "summary_changed"));
    assert_eq!(count(&st), 1, "a missed call counts");
    st.apply_event("message_updated", &record("88", missed), false);
    assert_eq!(count(&st), 1, "once per call");

    let cancelled = json!({"call_id": "89", "started_by": "42", "end_reason": "cancelled", "rung_ids": ["41"], "joined_ids": ["42"], "declined_ids": [], "missed_ids": ["41"]});
    st.apply_event("message_created", &record("89", json!({"call_id": "89", "missed_ids": []})), false);
    st.apply_event("message_updated", &record("89", cancelled), false);
    assert_eq!(count(&st), 1, "a cancelled ring is not a missed call");
    let declined = json!({"call_id": "90", "end_reason": "declined", "rung_ids": ["41"], "declined_ids": ["41"], "missed_ids": []});
    st.apply_event("message_updated", &record("90", declined), false);
    assert_eq!(count(&st), 1, "a decline adds nothing");
    let left = json!({"call_id": "91", "end_reason": "empty", "rung_ids": ["41"], "joined_ids": ["42"], "declined_ids": [], "missed_ids": ["41"]});
    st.apply_event("message_updated", &record("91", left), true);
    assert_eq!(count(&st), 2, "the caller left without Sew's cancel: a missed call");
    assert_eq!(st.convs["900"].unread_count, Some(1), "unread_count moves too once Sew has it");
    // A missed call another member had is not this person's.
    let theirs = json!({"call_id": "92", "end_reason": "no_answer", "missed_ids": ["43"]});
    st.apply_event("message_updated", &record("92", theirs), false);
    assert_eq!(count(&st), 2);
}

// ── phone ringing (S14) ──────────────────────────────────────────────

fn call_pushes(since: usize, ids: &[&str]) -> Vec<(String, CallPushState)> {
    crate::push_routes::test_push_capture::since(since)
        .into_iter()
        .filter_map(|e| match e {
            PushEvent::Call(c) if ids.contains(&c.call_id.as_str()) => Some((c.call_id, c.state)),
            _ => None,
        })
        .collect()
}

const SAM_EXT: &str = "k2:connect:5a6b7c8d-0000-4000-8000-000000000031";

/// A feed item as Sew writes it.
fn item(seq: u64, kind: &str, call: &str) -> Value {
    json!({"seq": seq, "kind": kind, "at": "2026-11-14T12:00:00Z", "channel_id": "900", "from_member_id": "42", "app_id": null, "call_id": call, "message_id": "1001"})
}

/// Install a fake feed answering `reads` in order (then empty pages that
/// keep `next`), recording each `(after, wait)`; Sam has one phone.
fn fake_feed(reads: Vec<(u16, Value)>) -> Arc<Mutex<Vec<(u64, u64)>>> {
    let asked: Arc<Mutex<Vec<(u64, u64)>>> = Arc::default();
    let queue = Arc::new(Mutex::new(reads));
    let base = deps_with(Arc::new(FakeApi::default()), "127.0.0.1:9".into(), HomeChatFeatures::default());
    let a2 = asked.clone();
    crate::home_chat::set_deps(Some(Arc::new(crate::home_chat::Deps {
        sew: base.sew.clone(),
        mint: base.mint.clone(),
        gateway_addr: base.gateway_addr.clone(),
        flags: base.flags.clone(),
        platform_ok: true,
        feed: Arc::new(move |ext: &str, after: u64, wait: u64| {
            assert_eq!(ext, SAM_EXT, "one person's feed");
            a2.lock().unwrap().push((after, wait));
            let mut q = queue.lock().unwrap();
            if q.is_empty() {
                Ok((200, json!({"items": [], "next": after})))
            } else {
                Ok(q.remove(0))
            }
        }),
        devices: Arc::new(|ext: &str| (ext == SAM_EXT).then(|| ("sam".to_string(), vec!["sam".to_string()]))),
    })));
    asked
}

/// S14 on Sew's contract: `after` starts at 0 and is always the last page's
/// `next` (even an empty page's); a `call_ring` sends ONE ring push; the
/// feed's `call_missed` sends ONE push that replaces it (the same banner);
/// unknown kinds are ignored; a `410 cursor_expired` resumes at `oldest_seq`
/// and pushes nothing for the gap; 404 and 403 `not_provisioned` stop.
#[test]
fn the_feed_long_poll_sends_one_push_then_a_replacement() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    crate::home_chat::call_push::reset_for_tests();
    use crate::home_chat::call_push::{poll_once, Poll, FEED_WAIT_RINGING_S, FEED_WAIT_S};
    let asked = fake_feed(vec![
        (200, json!({"items": [item(1, "call_ring", "7701"), item(2, "dm", "x")], "next": 2})),
        (200, json!({"items": [], "next": 2})),
        (200, json!({"items": [item(3, "call_ring", "7701"), item(4, "call_missed", "7701"), item(5, "something_new", "x")], "next": 5})),
        (410, json!({"error": {"code": "cursor_expired", "hint": "too old", "details": {"oldest_seq": 40, "head": 90}}})),
        (200, json!({"items": [], "next": 90})),
        (404, json!({"error": {"code": "not_found", "hint": "no such person"}})),
        (403, json!({"error": {"code": "not_provisioned", "hint": "deactivated"}})),
    ]);
    let mark = crate::push_routes::test_push_capture::mark();
    let mut cursor = 0u64;
    assert_eq!(poll_once(SAM_EXT, &mut cursor).unwrap(), Poll::Again(1));
    assert_eq!(call_pushes(mark, &["7701"]), vec![("7701".to_string(), CallPushState::Ringing)], "one ring push");
    assert_eq!(poll_once(SAM_EXT, &mut cursor).unwrap(), Poll::Again(0), "an empty page");
    assert_eq!(poll_once(SAM_EXT, &mut cursor).unwrap(), Poll::Again(1));
    assert_eq!(
        call_pushes(mark, &["7701"]),
        vec![("7701".to_string(), CallPushState::Ringing), ("7701".to_string(), CallPushState::Missed)],
        "the ring again is nothing; the missed push replaces it"
    );
    assert_eq!(poll_once(SAM_EXT, &mut cursor).unwrap(), Poll::Again(0));
    assert_eq!(cursor, 40, "410: resume at oldest_seq");
    assert_eq!(poll_once(SAM_EXT, &mut cursor).unwrap(), Poll::Again(0));
    assert!(matches!(poll_once(SAM_EXT, &mut cursor).unwrap(), Poll::Stop(_)), "404 stops");
    assert!(matches!(poll_once(SAM_EXT, &mut cursor).unwrap(), Poll::Stop(_)), "403 not_provisioned stops");
    let asked = asked.lock().unwrap().clone();
    let afters: Vec<u64> = asked.iter().map(|(a, _)| *a).collect();
    assert_eq!(afters, vec![0, 2, 2, 5, 40, 90, 90], "after: 0, then every page's next");
    assert_eq!(asked[1].1, FEED_WAIT_RINGING_S, "a ring waits on its end: shorter holds");
    assert_eq!(asked[3].1, FEED_WAIT_S, "nothing ringing: the long hold");
    let events: BTreeMap<String, PushEvent> = crate::push_routes::test_push_capture::since(mark)
        .into_iter()
        .filter_map(|e| match &e {
            PushEvent::Call(c) if c.call_id == "7701" => Some((c.state.as_wire().to_string(), e.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(events["ringing"].collapse_id(), events["missed"].collapse_id(), "the same banner");
    assert_eq!(events["ringing"].target_usernames(), Some(&["sam".to_string()][..]));
    crate::home_chat::call_push::reset_for_tests();
    crate::home_chat::set_deps(None);
}

/// The ring's END, both paths: the held session hears `call_ring {ringing:
/// false}` and the replacement goes at once ("Answered on another device"
/// for accepted, "Call ended" for a cancel); with no session to hear it, the
/// ring is replaced by "Call ended" once it is old enough, and not before. A
/// live held session never gets the timed fallback.
#[test]
fn a_rings_end_is_pushed_at_once_or_by_the_fallback() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    crate::home_chat::call_push::reset_for_tests();
    use crate::home_chat::call_push::{expire_rings, poll_once, ring_seen, RING_FALLBACK_SECS};
    fake_feed(vec![(
        200,
        json!({"items": [item(1, "call_ring", "7801"), item(2, "call_ring", "7802"), item(3, "call_ring", "7803"), item(4, "call_ring", "7804")], "next": 4}),
    )]);
    let mark = crate::push_routes::test_push_capture::mark();
    let mut cursor = 0u64;
    poll_once(SAM_EXT, &mut cursor).unwrap();
    let rang = crate::home_chat::now_secs();
    ring_seen(SAM_EXT, &json!({"call_id": "7801", "channel_id": "900", "ringing": false, "reason": "accepted"}));
    ring_seen(SAM_EXT, &json!({"call_id": "7802", "channel_id": "900", "ringing": false, "reason": "cancelled"}));
    assert_eq!(
        call_pushes(mark, &["7801", "7802"]),
        vec![
            ("7801".to_string(), CallPushState::Ringing),
            ("7802".to_string(), CallPushState::Ringing),
            ("7801".to_string(), CallPushState::Answered),
            ("7802".to_string(), CallPushState::Ended),
        ],
        "heard: replaced at once"
    );
    // 7803 and 7804: nobody heard their end.
    assert_eq!(expire_rings(SAM_EXT, rang + RING_FALLBACK_SECS, true), 0, "a live held session hears the end: no timed fallback");
    assert_eq!(expire_rings(SAM_EXT, rang + RING_FALLBACK_SECS - 2, false), 0, "too soon");
    assert_eq!(expire_rings(SAM_EXT, rang + RING_FALLBACK_SECS + 1, false), 2);
    assert_eq!(
        call_pushes(mark, &["7803", "7804"]).into_iter().filter(|(_, s)| *s == CallPushState::Ended).count(),
        2,
        "the fallback replaces each with Call ended"
    );
    assert_eq!(expire_rings(SAM_EXT, rang + 1_000, false), 0, "once");
    crate::home_chat::call_push::reset_for_tests();
    crate::home_chat::set_deps(None);
}

// ── recording a DM call (prd-home-call-recording-v1 S1) ──────────────

/// HR-3: with Home chat on and the voice rooms preview OFF, a DM call's
/// recording routes reach the handler (before, the notice's OK got 404
/// `voice_rooms_off` and Sew dropped the person after 60 s): the consent
/// answer and Stop by path, Record and the controls read with `dm`. A
/// room's Record or controls read stays dark. Callers who never record are
/// refused by the handler, so each answer below comes from it.
#[test]
fn a_dm_calls_recording_routes_answer_with_the_preview_off() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _l = super::super::lane::test_lock();
    let _home = k2_core::test_env::TempHome::new();
    for p in [routes::RECORDING_CONSENT, routes::RECORDING_STOP, routes::RECORDING_START, routes::RECORDING_CONTROLS] {
        assert!(routes::dm_capable(p), "{p}: the dispatcher lets it through while Home chat is on");
    }
    k2_core::voice_preview::set_enabled(false);
    k2_core::home_chat::set_enabled(true);
    let st = |r: &crate::cli_response::CliResponse| r.status.split(' ').next().unwrap_or("").to_string();
    let err = |r: &crate::cli_response::CliResponse| body_of(r)["error"]["code"].as_str().unwrap_or("").to_string();
    let consent: &[u8] = br#"{"recording_id":"42","consent":true,"notice_version":3}"#;
    let cases: [(&str, &[u8]); 4] = [
        (routes::RECORDING_CONSENT, consent),
        (routes::RECORDING_STOP, br#"{"recordingId":"42"}"#),
        (routes::RECORDING_START, br#"{"dm":"900"}"#),
        (routes::RECORDING_CONTROLS, br#"{"dm":"900"}"#),
    ];
    for (p, body) in cases {
        let r = routes::handle(p, Caller::Passport, "o", true, body);
        assert_eq!((st(&r), err(&r)), ("403".into(), "agents_not_in_voice".into()), "{p}: the handler answers, not voice_rooms_off");
        let r = routes::handle(p, Caller::ChatManager, "o", true, body);
        assert_eq!(err(&r), "agents_not_in_voice", "{p}: never an agent, Manage chat and voice or not");
        let r = routes::handle(p, Caller::OwnerToken, "o", true, body);
        assert_eq!(err(&r), "owner_token_cannot_join", "{p}");
        // A GET is 405 (the dispatcher answers it by path before any body).
        assert_eq!(st(&routes::handle(p, Caller::Passport, "o", false, body)), "405", "{p}: POST only");
    }
    // A Voice & Chat only login is told and may leave, never records.
    let vc = Caller::Login { username: "sam".into(), role: Role::VoiceChat, token: "t".into() };
    let vc_cases: [(&str, &[u8]); 2] = [(routes::RECORDING_START, br#"{"dm":"900"}"#), (routes::RECORDING_STOP, br#"{"recordingId":"42"}"#)];
    for (p, body) in vc_cases {
        let r = routes::handle(p, vc.clone(), "o", true, body);
        assert_eq!((st(&r), err(&r)), ("403".into(), "voice_chat_not_allowed".into()), "{p}");
    }
    // A room's Record and controls read stay dark with the preview off.
    for p in [routes::RECORDING_START, routes::RECORDING_CONTROLS] {
        let r = routes::handle(p, Caller::Passport, "o", true, br#"{"room":"1"}"#);
        assert_eq!((st(&r), err(&r)), ("404".into(), "voice_rooms_off".into()), "{p} for a room");
    }
    // Home chat off too: everything dark.
    k2_core::home_chat::set_enabled(false);
    let r = routes::handle(routes::RECORDING_CONSENT, Caller::Passport, "o", true, consent);
    assert_eq!(err(&r), "voice_rooms_off");
}

/// HR-4 + Q7: Record in a DM goes to Sew's `channels/{dm}/recording/start`
/// on the person's held Home chat session, asking for 30 days of audio,
/// capped by the server's longest keep; a DM the person isn't in is
/// `dm_not_found` and Sew is never asked; Sew's refusals keep its reason.
#[test]
fn a_dm_record_goes_to_sew_on_the_held_session_with_the_keep_days() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    let api = Arc::new(FakeApi::default());
    crate::home_chat::set_deps(Some(deps_with(api.clone(), "127.0.0.1:9".into(), HomeChatFeatures::default())));
    sam_in_dm_900();
    let path = "/api/v1/channels/900/recording/start";
    api.answer(path, 200, json!({"id": "42", "state": "recording", "started_by": "41"}));
    let c = ctx(false, Arc::new(LoginSew::default()));

    assert_eq!(DM_AUDIO_KEEP_DAYS, 30, "Q7's default: Sew's own 30 days");
    let v = recording_start(&sam(), &json!({"dm": "900"}), &c).expect("record");
    assert_eq!((v["action"].as_str(), v["dm"].as_str(), v["recording"]["id"].as_str()), (Some("started"), Some("900"), Some("42")), "{v}");
    let sent = api.calls_to(path);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0.as_deref(), Some("sews_first.x"), "on the person's held session, as the call");
    assert_eq!(sent[0].1, json!({"audio_keep_days": 30}));

    let capped = Ctx { audio_max_days: Some(7), ..c.clone() };
    recording_start(&sam(), &json!({"dm": "900"}), &capped).expect("record");
    assert_eq!(api.calls_to(path)[1].1, json!({"audio_keep_days": 7}), "the server's longest keep caps it");
    assert_eq!(dm_keep_days(Some(90)), 30, "a longer server keep never lengthens a DM's");

    assert_eq!(code(&recording_start(&sam(), &json!({"dm": "901"}), &c).unwrap_err()), (404, "dm_not_found"));
    assert_eq!(code(&recording_start(&sam(), &json!({"dm": "../900"}), &c).unwrap_err()), (400, "usage"));
    assert_eq!(api.calls_to("/api/v1/channels/901/recording/start").len(), 0, "never asked for another DM");
    let off = Ctx { chat: (false, Some("sew_off")), ..c.clone() };
    assert_eq!(code(&recording_start(&sam(), &json!({"dm": "900"}), &off).unwrap_err()), (503, "chat_unavailable"));
    let free = Ctx { tier: Tier::NotPro, ..c.clone() };
    assert_eq!(code(&recording_start(&sam(), &json!({"dm": "900"}), &free).unwrap_err()), (403, "host_not_pro"), "DM recording needs Pro, as DM calls do");

    // DM recording off on this server: Sew's refusal, its reason kept.
    api.answer(path, 403, json!({"error": {"code": "missing_permission", "hint": "", "details": {"reason": "recording_disabled"}}}));
    let r = recording_start(&sam(), &json!({"dm": "900"}), &c).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (403, "recording_disabled"));
    assert_eq!(r.details.unwrap()["sew"]["details"]["reason"], "recording_disabled");
    follower::release_all();
    crate::home_chat::set_deps(None);
}

/// HR-4 + HR-5: the controls read for a DM reads the DM channel's
/// `my_perms` on the held session; the answer says `dm: true` with the
/// seen `recording.dm_enabled`, and never K2's agent line (HR-8). A Voice &
/// Chat only login is told it can't record.
#[test]
fn a_dm_controls_read_says_dm_and_never_the_agent_line() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    let api = Arc::new(FakeApi::default());
    crate::home_chat::set_deps(Some(deps_with(api.clone(), "127.0.0.1:9".into(), HomeChatFeatures::default())));
    sam_in_dm_900();
    let both = bff::perms::RECORD_CALLS | bff::perms::MANAGE_CHANNELS;
    api.answer("/api/v1/channels/900", 200, json!({"channel": {"id": "900", "kind": "dm", "my_perms": bff::perms::RECORD_CALLS.to_string()}}));
    let c = ctx(false, Arc::new(LoginSew::default()));
    let v = recording_controls(&sam(), &json!({"dm": "900"}), &c).expect("perms");
    assert_eq!((v["room"].as_str(), v["channelId"].as_str()), (Some("dm:900"), Some("900")));
    assert_eq!((v["recordCalls"].as_bool(), v["manageChannels"].as_bool()), (Some(true), Some(false)), "{v}");
    assert_eq!(api.calls_to("/api/v1/channels/900")[0].0.as_deref(), Some("sews_first.x"));
    api.answer("/api/v1/channels/900", 200, json!({"id": "900", "my_perms": both}));
    let v = recording_controls(&sam(), &json!({"dm": "900"}), &c).expect("perms at the top level");
    assert_eq!(v["myPerms"], both.to_string());
    api.answer("/api/v1/channels/900", 200, json!({"id": "900"}));
    assert_eq!(code(&recording_controls(&sam(), &json!({"dm": "900"}), &c).unwrap_err()), (502, "chat_unavailable"));
    assert_eq!(code(&recording_controls(&sam(), &json!({"dm": "901"}), &c).unwrap_err()), (404, "dm_not_found"));

    let facts = |dm: bool, voice_chat: bool| routes::ControlsFacts {
        dm,
        voice_chat,
        agent_readers: true,
        recording_enabled: json!(true),
        recording_dm_enabled: json!(true),
    };
    let a = routes::controls_answer(json!({"recordCalls": true, "manageChannels": false}), &facts(true, false));
    assert_eq!(a["rights"], json!({"dm": true, "recordingEnabled": true, "recordingDmEnabled": true, "recordCapped": false, "agent": false}));
    assert_eq!(a["agentReaders"], false, "Read call transcripts never reaches a DM: no agent line");
    assert_eq!(a["recordCalls"], true);
    let room = routes::controls_answer(json!({"recordCalls": true}), &facts(false, false));
    assert_eq!((room["rights"]["dm"].clone(), room["agentReaders"].clone()), (json!(false), json!(true)), "a K5 room keeps its line");
    let vc = routes::controls_answer(json!({"recordCalls": true, "manageChannels": true}), &facts(true, true));
    assert_eq!((vc["recordCalls"].clone(), vc["voiceChatOnly"].clone()), (json!(false), json!(true)), "Voice & Chat only never records");
    follower::release_all();
    crate::home_chat::set_deps(None);
}

/// HR-10: a transcript line (a message that carries `transcript`, as a
/// reply under the recording card or not) from the other person never adds to the DM's unread, its count or its last
/// message (so the row's preview never shows one); the card itself makes
/// the DM unread but counts nothing. The frame still reaches the windows.
#[test]
fn a_transcript_line_never_counts_toward_the_dms_badge() {
    let mut st = ringing_state();
    let card = json!({"message": {"id": "2001", "channel_id": "900", "author_id": "42", "body": "",
        "recording": {"recording_id": "42", "started_by": "42", "transcript": {"state": "running", "turns": 0}}}});
    let f = st.apply_event("message_created", &card, true);
    assert!(f.iter().any(|x| x["type"] == "message"));
    assert!(st.convs["900"].unread, "the card makes the DM unread");
    assert_eq!((st.convs["900"].mention_count, st.convs["900"].unread_count.unwrap_or(0)), (0, 0), "the card counts nothing");
    assert_eq!(st.convs["900"].last_message_id.as_deref(), Some("2001"));
    st.convs.get_mut("900").unwrap().unread = false;

    let lines = [
        json!({"id": "2002", "channel_id": "900", "author_id": "42", "body": "hello", "thread_root_id": "2001",
            "transcript": {"recording_id": "42", "speaker_id": "42", "start_ms": 0, "end_ms": 900}}),
        json!({"id": "2004", "channel_id": "900", "author_id": "42", "body": "a turn only", "transcript": {"recording_id": "42"}}),
    ];
    for (i, line) in lines.into_iter().enumerate() {
        let f = st.apply_event("message_created", &json!({"message": line}), true);
        assert_eq!(f.len(), 1, "{i}: one frame, no summary_changed");
        assert_eq!(f[0]["type"], "message", "{i}: Show transcript hears it");
        let c = &st.convs["900"];
        assert!(!c.unread, "{i}: not unread");
        assert_eq!((c.mention_count, c.unread_count.unwrap_or(0)), (0, 0), "{i}: never counted");
        assert_eq!(c.last_message_id.as_deref(), Some("2001"), "{i}: the row's preview never shows a transcript line");
    }
    // An ordinary DM message from them still counts (Sew's HP-26 rule).
    st.apply_event("message_created", &json!({"message": {"id": "2005", "channel_id": "900", "author_id": "42", "body": "hi"}}), true);
    assert_eq!(st.convs["900"].mention_count, 1);
}

/// A plain thread reply (a person's reply in a Sew thread, written in Sew's
/// own app: `thread_root_id`, no `transcript`) is an ordinary message, as
/// before 0.46.15: on the timeline, counted, and the row's last message.
/// Only a recording's transcript turn (Sew always puts `transcript` on one)
/// is kept off, whether or not it names a thread root (HR-10).
#[test]
fn a_plain_thread_reply_shows_and_counts_like_any_message() {
    let mut st = ringing_state();
    let reply = json!({"id": "2003", "channel_id": "900", "author_id": "42", "body": "a reply only", "thread_root_id": "2001"});
    assert!(!crate::home_chat::state::is_transcript_line(&reply), "a reply with no transcript is not a transcript line");
    let f = st.apply_event("message_created", &json!({"message": reply}), true);
    let kinds: Vec<&str> = f.iter().map(|x| x["type"].as_str().expect("a frame type")).collect();
    assert_eq!(kinds, ["message", "summary_changed"], "shown, and the badge may move");
    assert_eq!(f[0]["message"]["id"], "2003");
    let c = &st.convs["900"];
    assert!(c.unread, "the reply makes the DM unread");
    assert_eq!((c.mention_count, c.unread_count), (1, Some(1)), "counted like any DM message");
    assert_eq!(c.last_message_id.as_deref(), Some("2003"), "the row's preview may show the reply");

    let turns = [
        json!({"id": "2004", "channel_id": "900", "author_id": "42", "body": "a turn", "thread_root_id": "2001",
            "transcript": {"recording_id": "42", "speaker_id": "42", "start_ms": 0, "end_ms": 900}}),
        json!({"id": "2005", "channel_id": "900", "author_id": "42", "body": "a turn, no root", "transcript": {"recording_id": "42"}}),
    ];
    for (i, turn) in turns.into_iter().enumerate() {
        assert!(crate::home_chat::state::is_transcript_line(&turn), "{i}: a message with transcript is a transcript line");
        let f = st.apply_event("message_created", &json!({"message": turn}), true);
        assert_eq!(f.len(), 1, "{i}: one frame, no summary_changed");
        assert_eq!(f[0]["type"], "message", "{i}: Show transcript hears it");
        let c = &st.convs["900"];
        assert_eq!((c.mention_count, c.unread_count), (1, Some(1)), "{i}: a transcript turn is never counted");
        assert_eq!(c.last_message_id.as_deref(), Some("2003"), "{i}: the preview stays the reply");
    }
}
