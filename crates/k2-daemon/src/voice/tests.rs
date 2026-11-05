//! Lane C unit tests: hosting state, occupancy, the lane gate, the BFF
//! against a recording fake Sew, and the routes' caller refusals. Every
//! test fails loudly; none touches `~/.k2`, the keychain or a daemon.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use k2_core::connect_users::Role;
use k2_core::tool_hook::admin_space::{RoomRow, SpaceRow, ROOM_ACTIVE, ROOM_ARCHIVED, ROOM_CLOSING, SPACE_BLOCKED, SPACE_READY};
use k2_core::tool_hook::features::VoiceFeatures;
use k2_core::tool_hook::sew_wire::{ExternalId, MgmtVoiceChange, MgmtVoiceState, VoiceStatesResponse};
use k2_core::tool_hook::ConnectChatGate;
use k2_core::voice_rooms::Hosting as Tier;

use super::bff::{self, Client, Deps, Forward, Person, Presence, SewAnswer, SewApi};
use super::lane::{self, Gate, LaneKind, LaneState};
use super::occupancy::{person_name, Occupancy, Occupant};
use super::routes::{self, Caller};
use super::{hosting_of, HostingFacts, HOSTING_OK};

const UUID_ADA: &str = "3c4d5e6f-0000-4000-8000-000000000001";
const UUID_MO: &str = "3c4d5e6f-0000-4000-8000-000000000002";

// ── hosting ──────────────────────────────────────────────────────────

fn ready_facts() -> HostingFacts {
    HostingFacts {
        linux: true,
        preview: true,
        chat_on: true,
        sew_standalone: false,
        flags: VoiceFeatures { server_governed_space: true, mgmt_voice_states: true, voice_channel_closing: true, ..Default::default() },
        space: Some(SpaceRow { space_id: Some("7001".into()), state: SPACE_READY.into(), ..Default::default() }),
        tier: Tier::Ok,
        not_pro_reason: None,
        count: 1,
        lane: LaneState::not_applicable(),
    }
}

#[test]
fn hosting_state_follows_the_table_in_order() {
    assert_eq!(hosting_of(&ready_facts()).state, HOSTING_OK);
    let cases: Vec<(Box<dyn Fn(&mut HostingFacts)>, &str)> = vec![
        (Box::new(|f: &mut HostingFacts| f.linux = false), "not_linux"),
        (Box::new(|f: &mut HostingFacts| f.preview = false), "preview_off"),
        (Box::new(|f: &mut HostingFacts| f.chat_on = false), "sew_off"),
        (Box::new(|f: &mut HostingFacts| f.flags.server_governed_space = false), "voice_off"),
        (Box::new(|f: &mut HostingFacts| f.space = None), "starting"),
        (
            Box::new(|f: &mut HostingFacts| f.space = Some(SpaceRow { state: SPACE_BLOCKED.into(), why: Some("slug_taken".into()), ..Default::default() })),
            "voice_off",
        ),
        (Box::new(|f: &mut HostingFacts| f.tier = Tier::NotPro), "not_pro"),
        (Box::new(|f: &mut HostingFacts| f.flags.voice_channel_closing = false), "voice_off"),
        (Box::new(|f: &mut HostingFacts| f.count = 0), "count_zero"),
        (Box::new(|f: &mut HostingFacts| f.lane = LaneState::reconnecting("frpc-voice exited")), "voice_lane_reconnecting"),
        (Box::new(|f: &mut HostingFacts| f.lane = LaneState::down("voice_lane_down")), "voice_lane_down"),
        (Box::new(|f: &mut HostingFacts| f.lane = LaneState::up()), HOSTING_OK),
    ];
    for (change, want) in cases {
        let mut f = ready_facts();
        change(&mut f);
        let h = hosting_of(&f);
        assert_eq!(h.state, want, "{f:?}");
        assert!(!h.line.is_empty());
    }
    // Sew installed with its own sign-ins: said plainly, not "isn't on".
    let mut f = ready_facts();
    f.chat_on = false;
    assert_eq!(hosting_of(&f).line, "Voice rooms are off on this server: Chat / Voice Hosting isn't on (k2 hostchat status).");
    assert_eq!(hosting_of(&f).detail, None);
    f.sew_standalone = true;
    let h = hosting_of(&f);
    assert_eq!(h.state, "sew_off");
    assert_eq!(h.line, "This server's Chat / Voice Hosting uses its own sign-ins (standalone), so it can't host K2 voice rooms.");
    assert_eq!(h.detail.as_deref(), Some("identity: standalone (k2 hostchat status)"));
    // The earlier checks still come first.
    f.preview = false;
    assert_eq!(hosting_of(&f).state, "preview_off");
    assert_eq!(hosting_of(&f).line, "Voice rooms are a preview: the owner turns them on (k2 settings set voiceRoomsPreview true, or K2_VOICE_ROOMS=1).");

    let mut f = ready_facts();
    f.space = Some(SpaceRow { state: SPACE_BLOCKED.into(), why: Some("slug_taken".into()), ..Default::default() });
    assert_eq!(hosting_of(&f).detail.as_deref(), Some("K2 Admin Space: slug_taken"));
    // The wire carries `detail`, the field the client parser reads.
    let v = serde_json::to_value(hosting_of(&f)).unwrap();
    assert_eq!(v["state"], "voice_off");
    assert_eq!(v["detail"], "K2 Admin Space: slug_taken");
}

// ── occupancy ────────────────────────────────────────────────────────

fn st(channel: &str, member: &str, ext: &str, joined: &str) -> MgmtVoiceState {
    MgmtVoiceState {
        channel_id: channel.into(),
        member_id: member.into(),
        external_id: Some(ExternalId::parse(ext).unwrap()),
        connection: "connected".into(),
        joined_at: joined.into(),
        ..Default::default()
    }
}

fn names(ext: Option<&ExternalId>, member: &str) -> String {
    person_name(ext, member, &|u: &str| (u == UUID_ADA).then(|| "ada".to_string()))
}

#[test]
fn person_names_map_connect_uuids_to_logins() {
    assert_eq!(names(Some(&ExternalId::K2Owner), "5"), "owner");
    assert_eq!(names(Some(&ExternalId::K2Connect(UUID_ADA.into())), "6"), "ada");
    assert_eq!(names(Some(&ExternalId::K2Connect(UUID_MO.into())), "7"), format!("k2:connect:{UUID_MO}"), "an unknown uuid keeps its id");
    assert_eq!(names(Some(&ExternalId::K2Workspace(UUID_MO.into())), "8"), format!("agent:{UUID_MO}"));
    assert_eq!(names(None, "9"), "member:9");
}

#[test]
fn occupancy_snapshot_then_changes_in_order() {
    let mut o = Occupancy::default();
    let snap = VoiceStatesResponse {
        states: vec![st("901", "5", "k2:owner", "2026-11-01T00:00:01Z"), st("901", "6", &format!("k2:connect:{UUID_ADA}"), "2026-11-01T00:00:00Z")],
        cursor: "1.0".into(),
    };
    assert!(o.replace(&snap, &names));
    assert!(!o.replace(&snap, &names), "the same snapshot changes nothing");
    let by = o.by_channel();
    let people: Vec<&str> = by["901"].iter().map(|x| x.person.as_str()).collect();
    assert_eq!(people, vec!["ada", "owner"], "by join time");

    let mut muted = st("901", "6", &format!("k2:connect:{UUID_ADA}"), "2026-11-01T00:00:00Z");
    muted.self_mute = true;
    let changes = vec![
        MgmtVoiceChange { seq: 2, kind: "updated".into(), channel_id: "901".into(), member_id: Some("6".into()), state: Some(muted), ..Default::default() },
        MgmtVoiceChange { seq: 3, kind: "left".into(), channel_id: "901".into(), member_id: Some("5".into()), ..Default::default() },
        MgmtVoiceChange { seq: 4, kind: "channel_closing".into(), channel_id: "902".into(), ..Default::default() },
        MgmtVoiceChange { seq: 5, kind: "some_new_kind".into(), channel_id: "902".into(), ..Default::default() },
    ];
    let (changed, notes) = o.apply(&changes, &names);
    assert!(changed);
    assert_eq!(notes, vec![("902".to_string(), ROOM_CLOSING)]);
    let by = o.by_channel();
    assert_eq!(by["901"].len(), 1);
    assert!(by["901"][0].self_mute && by["901"][0].person == "ada");

    let (changed, notes) = o.apply(
        &[MgmtVoiceChange { seq: 6, kind: "channel_archived".into(), channel_id: "901".into(), ..Default::default() }],
        &names,
    );
    assert!(changed);
    assert_eq!(notes, vec![("901".to_string(), ROOM_ARCHIVED)]);
    assert!(o.is_empty(), "archiving ends the room's calls");
    let (changed, _) = o.apply(
        &[MgmtVoiceChange { seq: 7, kind: "left".into(), channel_id: "901".into(), member_id: Some("6".into()), ..Default::default() }],
        &names,
    );
    assert!(!changed, "a left for someone already gone changes nothing");
}

// ── the lane gate ────────────────────────────────────────────────────

#[test]
fn lane_gate_goes_holds_and_refuses() {
    let _l = lane::test_lock();
    lane::publish(LaneState::not_applicable());
    assert!(matches!(lane::gate_join(Duration::from_secs(5)), Gate::Go { .. }));
    lane::publish(LaneState::up());
    assert!(matches!(lane::gate_join(Duration::from_secs(5)), Gate::Go { .. }));

    // Down: refused at once, with lane A's reason.
    lane::publish(LaneState::down("voice_lane_down"));
    let t = Instant::now();
    match lane::gate_join(Duration::from_secs(5)) {
        Gate::Refuse { state, .. } => assert_eq!((state.state, state.reason_text().as_str()), (LaneKind::Down, "voice_lane_down")),
        g => panic!("expected a refusal, got {g:?}"),
    }
    assert!(t.elapsed() < Duration::from_secs(1), "a down lane never holds");

    // Reconnecting, then back up: the held join goes on.
    lane::publish(LaneState::reconnecting("frpc-voice exited"));
    let up = std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(200));
        lane::publish(LaneState::up());
    });
    match lane::gate_join(Duration::from_secs(5)) {
        Gate::Go { held_ms } => assert!((150..5000).contains(&held_ms), "held {held_ms} ms"),
        g => panic!("expected go, got {g:?}"),
    }
    up.join().unwrap();

    // Reconnecting through the whole hold: refused with the reason.
    lane::publish(LaneState::reconnecting("frpc-voice exited"));
    match lane::gate_join(Duration::from_millis(300)) {
        Gate::Refuse { state, held_ms } => {
            assert_eq!(state.state, LaneKind::Reconnecting);
            assert_eq!(state.reason_text(), "frpc-voice exited");
            assert!(held_ms >= 250, "held {held_ms} ms");
        }
        g => panic!("expected a refusal, got {g:?}"),
    }
    lane::publish(LaneState::not_applicable());
}

// ── the BFF against a fake Sew ───────────────────────────────────────

#[derive(Default)]
struct FakeSew {
    calls: Mutex<Vec<(Option<String>, String, Value)>>,
    /// path → queued answers (the last one repeats).
    answers: Mutex<BTreeMap<String, Vec<SewAnswer>>>,
}

impl FakeSew {
    fn answer(&self, path: &str, status: u16, body: Value) {
        self.answers.lock().unwrap().entry(path.into()).or_default().push(SewAnswer { status, body });
    }
    fn calls(&self) -> Vec<(Option<String>, String, Value)> {
        self.calls.lock().unwrap().clone()
    }
}

impl SewApi for FakeSew {
    fn post(&self, bearer: Option<&str>, path: &str, body: &Value) -> Result<SewAnswer, String> {
        self.calls.lock().unwrap().push((bearer.map(str::to_string), path.to_string(), body.clone()));
        let mut g = self.answers.lock().unwrap();
        let q = g.get_mut(path).ok_or_else(|| format!("fake sew: no answer for {path}"))?;
        Ok(if q.len() > 1 { q.remove(0) } else { q[0].clone() })
    }
}

fn deps(sew: Arc<FakeSew>) -> Deps {
    Deps {
        sew,
        mint: Arc::new(|i: &crate::chat_pass::identity::PassIdentity| Ok::<_, String>(format!("pass-for-{}", i.sub))),
        connect_gate: Arc::new(|_: &str, _: &str| Ok::<_, String>(ConnectChatGate::Ready)),
        level_pending: Arc::new(|_: &ExternalId| Ok::<_, String>(false)),
        lane_hold: Duration::from_millis(200),
        member_wait: Duration::from_millis(100),
        // No feed: presence is unknown, so leaves always reach Sew.
        presence: Arc::new(|_: &ExternalId| Presence { feed: "off".into(), listed: None }),
    }
}

/// Deps whose presence comes from `who`: external id → (channel, member id).
fn deps_with_presence(sew: Arc<FakeSew>, who: Arc<Mutex<BTreeMap<String, (String, String)>>>) -> Deps {
    Deps {
        presence: Arc::new(move |ext: &ExternalId| {
            let listed = who.lock().unwrap().get(&ext.to_string()).map(|(ch, member)| {
                (
                    ch.clone(),
                    Occupant {
                        member_id: member.clone(),
                        external_id: Some(ext.to_string()),
                        person: "someone".into(),
                        connection: "connected".into(),
                        self_mute: false,
                        self_deaf: false,
                        server_mute: false,
                        server_deaf: false,
                        joined_at: "2026-11-04T00:00:00Z".into(),
                    },
                )
            });
            Presence { feed: bff::FEED_LIVE.into(), listed }
        }),
        ..deps(sew)
    }
}

fn person(login: &str, uuid: &str, role: Role, sid_token: &str) -> Person {
    let ident = crate::chat_pass::identity::connect_identity(login, role, uuid, sid_token, false);
    let ext = ExternalId::parse(&ident.sub).unwrap();
    Person { ident, ext, login: login.into(), role, client: Client::Login }
}

/// The same person on a paired phone: another login session of theirs.
fn phone(login: &str, uuid: &str, role: Role, sid_token: &str, name: &str) -> Person {
    Person { client: Client::Phone { name: name.into() }, ..person(login, uuid, role, sid_token) }
}

fn leaves(sew: &FakeSew) -> usize {
    sew.calls().iter().filter(|c| c.1 == bff::paths::ME_VOICE_LEAVE).count()
}

fn room(n: u32, channel: &str, state: &str) -> RoomRow {
    RoomRow {
        n,
        slug: format!("room-{n}"),
        channel_id: Some(channel.into()),
        name: Some(format!("Room {n}")),
        want_name: None,
        state: state.into(),
        close_now: false,
        why: None,
    }
}

fn session_ok(sew: &FakeSew, token: &str) {
    sew.answer(bff::paths::AUTH_K2, 200, json!({"token": token, "expires_at": "2099-01-01T00:00:00Z", "member": {"id": "6"}}));
}

#[test]
fn join_trades_a_pass_once_and_hands_back_sews_answer() {
    let _l = lane::test_lock();
    lane::publish(LaneState::not_applicable());
    bff::forget_all();
    let sew = Arc::new(FakeSew::default());
    session_ok(&sew, "sews_a");
    sew.answer(
        &bff::paths::channel_join("901"),
        200,
        json!({"url": "wss://voice.box.k2.dev", "token": "media", "media": {"relay_only": true}, "e2ee": {"join_id": "j"}}),
    );
    let d = deps(sew.clone());
    let ada = person("ada", UUID_ADA, Role::Member, "tok-ada-1");
    let rooms = vec![room(1, "901", ROOM_ACTIVE), room(2, "902", ROOM_CLOSING)];

    let v = bff::join(&ada, "room-1", &json!({"self_mute": true}), &rooms, &d).expect("join");
    assert_eq!(v["relayOnly"], true, "media.relay_only reaches the client");
    assert_eq!(v["join"]["url"], "wss://voice.box.k2.dev");
    assert_eq!(v["room"]["id"], "room-1");
    let call = v["callId"].as_str().unwrap().to_string();
    assert!(call.starts_with("vc_"));

    // The second join reuses the session (one pass, one exchange).
    bff::join(&ada, "1", &json!({}), &rooms, &d).expect("join again");
    let calls = sew.calls();
    let auths: Vec<_> = calls.iter().filter(|c| c.1 == bff::paths::AUTH_K2).collect();
    assert_eq!(auths.len(), 1, "{calls:?}");
    assert_eq!(auths[0].0, None, "the exchange carries no bearer");
    assert_eq!(auths[0].2, json!({"pass": format!("pass-for-k2:connect:{UUID_ADA}")}));
    let joins: Vec<_> = calls.iter().filter(|c| c.1 == bff::paths::channel_join("901")).collect();
    assert_eq!(joins.len(), 2);
    assert_eq!(joins[0].0.as_deref(), Some("sews_a"));
    assert_eq!(joins[0].2, json!({"self_mute": true}), "Sew's join body goes as is");
    assert_eq!(bff::counts(), (1, 1), "one call per person");

    // The first call id is gone (a join elsewhere is a move).
    let r = bff::forward(&ada, Forward::State, &call, &json!({"self_mute": false}), &d).unwrap_err();
    assert_eq!(r.code, "call_not_found");

    // A closing room, an unknown room.
    assert_eq!(bff::join(&ada, "room-2", &json!({}), &rooms, &d).unwrap_err().code, "room_closing");
    assert_eq!(bff::join(&ada, "room-9", &json!({}), &rooms, &d).unwrap_err().code, "room_not_found");
    assert_eq!(bff::join(&ada, "room-1", &json!([1]), &rooms, &d).unwrap_err().code, "usage");
    bff::forget_all();
}

#[test]
fn join_renews_an_expired_session_once_and_maps_sew_refusals() {
    let _l = lane::test_lock();
    lane::publish(LaneState::not_applicable());
    bff::forget_all();
    let sew = Arc::new(FakeSew::default());
    session_ok(&sew, "sews_old");
    session_ok(&sew, "sews_new");
    let join = bff::paths::channel_join("901");
    sew.answer(&join, 401, json!({"ok": false, "error": {"code": "unauthenticated", "hint": "expired"}}));
    sew.answer(&join, 409, json!({"ok": false, "error": {"code": "conflict", "hint": "closing", "details": {"reason": "channel_closing"}}}));
    sew.answer(
        &join,
        503,
        // sew-56 v1.3 SW-11: the reason, not the code, says voice_unavailable.
        json!({"ok": false, "error": {"code": "unavailable", "hint": "voice is unavailable",
               "details": {"reason": "voice_unavailable", "engine": "media_path_down", "media_path_reason": "voice_lane_down"}}}),
    );
    let d = deps(sew.clone());
    let mo = person("mo", UUID_MO, Role::Member, "tok-mo-1");
    let rooms = vec![room(1, "901", ROOM_ACTIVE)];

    let r = bff::join(&mo, "room-1", &json!({}), &rooms, &d).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (409, "room_closing"), "after one renew, Sew's own refusal");
    let bearers: Vec<Option<String>> = sew.calls().into_iter().filter(|c| c.1 == join).map(|c| c.0).collect();
    assert_eq!(bearers, vec![Some("sews_old".to_string()), Some("sews_new".to_string())]);

    let r = bff::join(&mo, "room-1", &json!({}), &rooms, &d).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (503, "voice_unavailable"));
    assert_eq!(r.details.as_ref().unwrap()["engine"], "media_path_down");
    assert_eq!(r.details.as_ref().unwrap()["reason"], "voice_unavailable");
    assert!(r.hint.contains("voice_lane_down"), "the media path reason is named: {}", r.hint);
    assert_eq!(r.retry_after_secs, Some(bff::RETRY_SECS));
    bff::forget_all();
}

/// sew-56 v1.3: a change already in `states` may repeat, and a move is
/// `left {reason: moved}` then `joined`: applying is idempotent.
#[test]
fn occupancy_applies_repeats_and_moves_idempotently() {
    let mut o = Occupancy::default();
    let ada = format!("k2:connect:{UUID_ADA}");
    let snap = VoiceStatesResponse { states: vec![st("901", "6", &ada, "t0")], cursor: "1.4".into() };
    o.replace(&snap, &names);
    let joined = MgmtVoiceChange {
        seq: 4,
        kind: "joined".into(),
        channel_id: "901".into(),
        member_id: Some("6".into()),
        state: Some(st("901", "6", &ada, "t0")),
        ..Default::default()
    };
    let (changed, _) = o.apply(&[joined.clone(), joined.clone()], &names);
    assert!(!changed, "a join already in the snapshot, twice, moves nothing");
    let moved = vec![
        MgmtVoiceChange { seq: 5, kind: "left".into(), channel_id: "901".into(), member_id: Some("6".into()), reason: Some("moved".into()), ..Default::default() },
        MgmtVoiceChange { seq: 6, kind: "joined".into(), channel_id: "902".into(), member_id: Some("6".into()), state: Some(st("902", "6", &ada, "t1")), ..Default::default() },
    ];
    let (changed, _) = o.apply(&moved, &names);
    assert!(changed);
    let before = o.by_channel();
    assert!(!before.contains_key("901"));
    assert_eq!(before["902"].len(), 1);
    // The same batch again (a repeat from the cursor): the same end state.
    let (changed, _) = o.apply(&moved[1..], &names);
    assert!(!changed, "the repeated joined moves nothing");
    assert_eq!(o.by_channel(), before);
}

#[test]
fn join_is_held_by_the_lane_and_by_a_pending_level() {
    let _l = lane::test_lock();
    bff::forget_all();
    let sew = Arc::new(FakeSew::default());
    let d = deps(sew.clone());
    let ada = person("ada", UUID_ADA, Role::Admin, "tok-ada-2");
    let rooms = vec![room(1, "901", ROOM_ACTIVE)];

    lane::publish(LaneState::down("voice_lane_down"));
    let r = bff::join(&ada, "room-1", &json!({}), &rooms, &d).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (503, "voice_unavailable"));
    let det = r.details.unwrap();
    assert_eq!((det["engine"].as_str(), det["media_path_reason"].as_str()), (Some("media_path_down"), Some("voice_lane_down")));

    lane::publish(LaneState::reconnecting("frpc-voice exited"));
    let r = bff::join(&ada, "room-1", &json!({}), &rooms, &d).unwrap_err();
    assert_eq!(r.code, "voice_unavailable");
    assert!(r.details.unwrap()["heldMs"].as_u64().unwrap() >= 150, "held for the whole (test) hold");
    lane::publish(LaneState::not_applicable());

    let held = Deps { level_pending: Arc::new(|_: &ExternalId| Ok::<_, String>(true)), ..deps(sew.clone()) };
    let r = bff::join(&ada, "room-1", &json!({}), &rooms, &held).unwrap_err();
    assert_eq!((r.code.as_str(), r.retry_after_secs), ("voice_member_pending", Some(bff::RETRY_SECS)));
    let pending = Deps { connect_gate: Arc::new(|_: &str, _: &str| Ok::<_, String>(ConnectChatGate::Pending)), ..deps(sew.clone()) };
    session_ok(&sew, "sews_x");
    sew.answer(&bff::paths::channel_join("901"), 200, json!({"url": "wss://v", "token": "t"}));
    let r = bff::join(&ada, "room-1", &json!({}), &rooms, &pending).unwrap_err();
    assert_eq!(r.code, "chat_member_pending");
    assert!(sew.calls().is_empty(), "nothing reached Sew while held: {:?}", sew.calls());
    bff::forget_all();
}

#[test]
fn moderation_uses_the_moderators_own_session() {
    let _l = lane::test_lock();
    lane::publish(LaneState::not_applicable());
    bff::forget_all();
    let sew = Arc::new(FakeSew::default());
    session_ok(&sew, "sews_ada");
    sew.answer(&bff::paths::member_move("7001", "7"), 200, json!({}));
    sew.answer(&bff::paths::member_voice("7001", "7"), 200, json!({"server_mute": true}));
    let d = deps(sew.clone());
    let rooms = vec![room(1, "901", ROOM_ACTIVE)];
    let mut occ: BTreeMap<String, Vec<Occupant>> = BTreeMap::new();
    occ.insert(
        "901".into(),
        vec![Occupant {
            member_id: "7".into(),
            external_id: Some(format!("k2:connect:{UUID_MO}")),
            person: "mo".into(),
            connection: "connected".into(),
            self_mute: false,
            self_deaf: false,
            server_mute: false,
            server_deaf: false,
            joined_at: String::new(),
        }],
    );
    let member = person("mo", UUID_MO, Role::Member, "tok-mo-2");
    let r = bff::moderate(&member, "room-1", "mo", "kick", "7001", &rooms, &occ, &d).unwrap_err();
    assert_eq!(r.code, "moderator_only");

    let admin = person("ada", UUID_ADA, Role::Admin, "tok-ada-3");
    let v = bff::moderate(&admin, "room-1", "mo", "kick", "7001", &rooms, &occ, &d).expect("kick");
    assert_eq!((v["person"].as_str(), v["by"].as_str()), (Some("mo"), Some("ada")));
    let v = bff::moderate(&admin, "room-1", "7", "mute", "7001", &rooms, &occ, &d).expect("mute by member id");
    assert_eq!(v["action"], "mute");
    let calls = sew.calls();
    let sent: Vec<(Option<String>, String, Value)> = calls.into_iter().filter(|c| c.1 != bff::paths::AUTH_K2).collect();
    assert_eq!(
        sent,
        vec![
            (Some("sews_ada".to_string()), bff::paths::member_move("7001", "7"), json!({"channel_id": null})),
            (Some("sews_ada".to_string()), bff::paths::member_voice("7001", "7"), json!({"server_mute": true})),
        ]
    );
    assert_eq!(bff::moderate(&admin, "room-1", "nobody", "kick", "7001", &rooms, &occ, &d).unwrap_err().code, "not_in_room");
    assert_eq!(bff::moderate(&admin, "room-1", "mo", "ban", "7001", &rooms, &occ, &d).unwrap_err().code, "usage");
    bff::forget_all();
}

#[test]
fn leave_and_the_call_routes_stay_on_the_persons_own_call() {
    let _l = lane::test_lock();
    lane::publish(LaneState::not_applicable());
    bff::forget_all();
    let sew = Arc::new(FakeSew::default());
    session_ok(&sew, "sews_s");
    sew.answer(&bff::paths::channel_join("901"), 200, json!({"url": "wss://v", "token": "t"}));
    sew.answer(bff::paths::ME_VOICE_STATE, 200, json!({"self_mute": true}));
    sew.answer(bff::paths::ME_VOICE_LEAVE, 200, json!({}));
    let d = deps(sew.clone());
    let rooms = vec![room(1, "901", ROOM_ACTIVE)];
    let ada = person("ada", UUID_ADA, Role::Member, "tok-ada-4");
    let mo = person("mo", UUID_MO, Role::Member, "tok-mo-4");
    let call = bff::join(&ada, "room-1", &json!({}), &rooms, &d).unwrap()["callId"].as_str().unwrap().to_string();

    assert_eq!(bff::forward(&mo, Forward::State, &call, &json!({}), &d).unwrap_err().code, "call_not_found", "another login's call");
    let v = bff::forward(&ada, Forward::State, &call, &json!({"self_mute": true}), &d).expect("state");
    assert_eq!(v["sew"]["self_mute"], true);
    let v = bff::leave(&ada, Some(&call), &d).expect("leave");
    assert_eq!(v["room"], "room-1");
    assert_eq!(v["channelId"], "901");
    assert_eq!((v["left"].as_bool(), v["already"].as_bool()), (Some(true), Some(false)));
    assert_eq!(bff::counts().1, 0);
    assert_eq!(leaves(&sew), 1);
    // Leave twice: a success that says so, and Sew isn't asked again.
    let v = bff::leave(&ada, Some(&call), &d).expect("a second leave is a success");
    assert_eq!((v["left"].as_bool(), v["already"].as_bool(), v["why"].as_str()), (Some(true), Some(true), Some("already_left")));
    assert_eq!((v["room"].as_str(), v["channelId"].as_str()), (Some("room-1"), Some("901")));
    assert_eq!(leaves(&sew), 1, "an ended call never reaches Sew");
    // Another login's call, ended or not, and an id nobody had: refused.
    assert_eq!(bff::leave(&mo, Some(&call), &d).unwrap_err().code, "call_not_found", "someone else's ended call");
    assert_eq!(bff::leave(&mo, Some("vc_never"), &d).unwrap_err().code, "call_not_found");
    let call2 = bff::join(&ada, "room-1", &json!({}), &rooms, &d).unwrap()["callId"].as_str().unwrap().to_string();
    let r = bff::leave(&mo, Some(&call2), &d).unwrap_err();
    assert_eq!((r.status, r.code.as_str()), (404, "call_not_found"), "someone else's live call");
    assert_eq!(bff::counts().1, 1, "a refused leave ends nothing");
    assert_eq!(leaves(&sew), 1);
    let v = bff::leave(&ada, None, &d).expect("leave without a call id leaves whatever call");
    assert_eq!((v["left"].as_bool(), v["already"].as_bool()), (Some(true), Some(false)));
    assert_eq!(leaves(&sew), 2);
    assert_eq!(bff::leave(&ada, Some(&call2), &d).expect("ended by the plain leave")["why"], "already_left");
    bff::forget_all();
}

/// The phone gap list §6.4: a phone and a desktop are two login sessions of
/// one person, and Sew keeps one call per person.
#[test]
fn a_phone_and_a_desktop_share_one_call_and_leaves_stay_idempotent() {
    let _l = lane::test_lock();
    lane::publish(LaneState::not_applicable());
    bff::forget_all();
    let sew = Arc::new(FakeSew::default());
    session_ok(&sew, "sews_s");
    sew.answer(&bff::paths::channel_join("901"), 200, json!({"url": "wss://v", "token": "t", "call_credential": {"token": "sewcc_a.b"}}));
    sew.answer(&bff::paths::channel_join("902"), 200, json!({"url": "wss://v", "token": "t", "call_credential": {"token": "sewcc_c.d"}}));
    sew.answer(bff::paths::ME_VOICE_LEAVE, 200, json!({"ok": true}));
    let who: Arc<Mutex<BTreeMap<String, (String, String)>>> = Arc::default();
    let d = deps_with_presence(sew.clone(), who.clone());
    let rooms = vec![room(1, "901", ROOM_ACTIVE), room(2, "902", ROOM_ACTIVE)];
    let ada_ext = format!("k2:connect:{UUID_ADA}");
    let desk = person("ada", UUID_ADA, Role::Member, "tok-ada-desk");
    let ph = phone("ada", UUID_ADA, Role::Member, "tok-ada-phone", "Ada's iPhone");
    assert_ne!(desk.ident.sid, ph.ident.sid, "two sessions");
    assert_eq!(desk.ext, ph.ext, "one person");

    // Not listed anywhere: not in a call.
    let me = bff::me_view(&ph, &rooms, &d);
    assert_eq!((me["inCall"].as_bool(), me["user"].as_str(), me["presence"].as_str()), (Some(false), Some("ada"), Some("live")));
    assert!(me.get("elsewhere").is_none(), "{me}");

    // The desktop joins room 1; Sew lists ada there.
    let desk_call = bff::join(&desk, "room-1", &json!({}), &rooms, &d).unwrap()["callId"].as_str().unwrap().to_string();
    who.lock().unwrap().insert(ada_ext.clone(), ("901".into(), "6".into()));
    let me = bff::me_view(&ph, &rooms, &d);
    assert_eq!((me["inCall"].as_bool(), me["elsewhere"].as_bool(), me["here"].as_bool()), (Some(true), Some(true), Some(false)), "{me}");
    assert_eq!((me["room"]["id"].as_str(), me["room"]["name"].as_str()), (Some("room-1"), Some("Room 1")));
    assert_eq!(me["on"], json!({"kind": "login"}));
    assert_eq!(me["callId"], Value::Null, "another session's call id is never shown");
    assert_eq!((me["mayBeLeaving"].as_bool(), me["presenceLagSecs"].as_u64()), (Some(true), Some(45)));
    assert!(me["hint"].as_str().unwrap().contains("may still be leaving"), "{me}");
    let me = bff::me_view(&desk, &rooms, &d);
    assert_eq!((me["here"].as_bool(), me["elsewhere"].as_bool(), me["callId"].as_str()), (Some(true), Some(false), Some(desk_call.as_str())));

    // The phone joins the same room: it takes the call over.
    let phone_call = bff::join(&ph, "room-1", &json!({}), &rooms, &d).unwrap()["callId"].as_str().unwrap().to_string();
    assert_eq!(bff::counts().1, 1, "one call per person");
    let me = bff::me_view(&desk, &rooms, &d);
    assert_eq!((me["elsewhere"].as_bool(), me["on"].clone()), (Some(true), json!({"kind": "phone", "name": "Ada's iPhone"})), "{me}");
    assert_eq!(bff::me_view(&ph, &rooms, &d)["here"], true);
    // The desktop's stale leave must not end the phone's call.
    let v = bff::leave(&desk, Some(&desk_call), &d).expect("a leave of a call taken over is a success");
    assert_eq!((v["already"].as_bool(), v["why"].as_str()), (Some(true), Some("joined_elsewhere")));
    assert_eq!(leaves(&sew), 0, "never reaches Sew");
    assert_eq!(bff::counts().1, 1, "the phone's call stays");
    // The desktop can't leave the phone's call by its id (not its session's).
    assert_eq!(bff::leave(&desk, Some(&phone_call), &d).unwrap_err().code, "call_not_found");

    // The phone is killed: Sew drops ada (here: presence goes empty), then
    // the relaunched phone leaves the old call.
    who.lock().unwrap().clear();
    let v = bff::leave(&ph, Some(&phone_call), &d).expect("leave after Sew dropped the member");
    assert_eq!((v["left"].as_bool(), v["already"].as_bool(), v["why"].as_str()), (Some(true), Some(true), Some("sew_dropped")));
    assert_eq!(leaves(&sew), 1, "Sew's leave is idempotent, so K2 still sends it");
    assert_eq!(bff::leave(&ph, Some(&phone_call), &d).expect("and again")["why"], "already_left");
    assert_eq!(leaves(&sew), 1);

    // A call K2 still holds while Sew lists the person in another room (a
    // join K2 didn't make, or a restart): the leave ends nothing there.
    let c3 = bff::join(&ph, "room-1", &json!({}), &rooms, &d).unwrap()["callId"].as_str().unwrap().to_string();
    who.lock().unwrap().insert(ada_ext.clone(), ("902".into(), "6".into()));
    let v = bff::leave(&ph, Some(&c3), &d).expect("leave");
    assert_eq!((v["already"].as_bool(), v["why"].as_str()), (Some(true), Some("in_another_room")));
    assert_eq!(leaves(&sew), 1, "the room-2 call is untouched");
    // Listed with no K2 record (after a daemon restart): elsewhere, client unknown.
    let me = bff::me_view(&ph, &rooms, &d);
    assert_eq!((me["inCall"].as_bool(), me["elsewhere"].as_bool(), me["on"].clone()), (Some(true), Some(true), Value::Null));
    assert_eq!(me["room"]["id"], "room-2");

    // A move from the same session: the old call id leaves as `moved`.
    who.lock().unwrap().clear();
    let a = bff::join(&ph, "room-1", &json!({}), &rooms, &d).unwrap()["callId"].as_str().unwrap().to_string();
    let b = bff::join(&ph, "room-2", &json!({}), &rooms, &d).unwrap()["callId"].as_str().unwrap().to_string();
    assert_eq!(bff::leave(&ph, Some(&a), &d).expect("old id")["why"], "moved");
    assert_eq!(bff::forward(&ph, Forward::State, &a, &json!({}), &d).unwrap_err().code, "call_not_found", "an ended call has no session routes");
    assert_eq!(leaves(&sew), 1);
    let v = bff::leave(&ph, Some(&b), &d).expect("the live one");
    assert_eq!(v["room"], "room-2");
    assert_eq!(leaves(&sew), 2);
    bff::forget_all();
}

#[test]
fn sew_refusals_map_to_the_clients_codes() {
    let r = |status: u16, body: Value| bff::sew_refusal(&SewAnswer { status, body });
    assert_eq!(r(422, json!({"error": {"code": "invalid_state", "hint": "", "details": {"reason": "channel_full"}}})).code, "room_full");
    assert_eq!(r(422, json!({"error": {"code": "invalid_state", "hint": "", "details": {"reason": "not_space_member"}}})).code, "voice_member_pending");
    assert_eq!(r(403, json!({"error": {"code": "agents_not_in_voice", "hint": ""}})).code, "agents_not_in_voice");
    let origin = r(422, json!({"error": {"code": "invalid_state", "hint": "", "details": {"reason": "origin_not_listed", "field": "origin"}}}));
    assert_eq!((origin.status, origin.code.as_str()), (422, "origin_not_listed"), "Sew 0.7.2");
    let other = r(403, json!({"error": {"code": "missing_permission", "hint": "no", "details": {"reason": "x"}}}));
    assert_eq!((other.status, other.code.as_str()), (403, "missing_permission"));
    assert_eq!(other.details.unwrap()["sew"]["code"], "missing_permission");
    assert!(other.hint.contains("(x)"), "the reason is named: {}", other.hint);
    // sew-56 v1.2 SW-1: client routes on the Admin Space.
    for reason in ["app_session_not_allowed", "app_user_not_allowed"] {
        let x = r(403, json!({"ok": false, "error": {"code": "missing_permission", "hint": "", "details": {"reason": reason}}}));
        assert_eq!((x.status, x.code.as_str()), (403, reason));
        assert!(!x.hint.is_empty());
        assert_eq!(x.details.unwrap()["sew"]["details"]["reason"], reason);
    }
    assert_eq!(r(200, Value::Null).status, 502, "a non-error status never passes as a refusal status");
}

#[test]
fn rooms_are_found_by_number_slug_channel_or_name() {
    let mut r2 = room(2, "902", ROOM_ACTIVE);
    r2.want_name = Some("Standup".into());
    let rooms = vec![room(1, "901", ROOM_ACTIVE), r2];
    for (k, n) in [("1", 1), ("room-2", 2), ("901", 1), ("standup", 2), (" 2 ", 2)] {
        assert_eq!(bff::find_room(&rooms, k).unwrap().n, n, "{k}");
    }
    assert_eq!(bff::find_room(&rooms, "11").unwrap_err().code, "room_not_found");
    assert_eq!(bff::find_room(&rooms, "").unwrap_err().code, "usage");
}

// ── routes ───────────────────────────────────────────────────────────

struct Preview;
impl Preview {
    fn on() -> (std::sync::MutexGuard<'static, ()>, Preview) {
        let g = lane::test_lock();
        k2_core::voice_preview::set_enabled(true);
        (g, Preview)
    }
}
impl Drop for Preview {
    fn drop(&mut self) {
        k2_core::voice_preview::set_enabled(false);
    }
}

fn body(r: &crate::cli_response::CliResponse) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {}: {e}", r.body))
}

#[test]
fn routes_are_dark_with_the_preview_off() {
    let _g = lane::test_lock();
    if std::env::var("K2_VOICE_ROOMS").is_ok() {
        panic!("K2_VOICE_ROOMS is set in the test environment; unset it for this test");
    }
    k2_core::voice_preview::set_enabled(false);
    let r = routes::handle(routes::ROOMS, Caller::OwnerToken, "owner", false, b"");
    assert_eq!(r.status, "404 Not Found");
    assert_eq!(body(&r)["error"]["code"], "voice_rooms_off");
}

#[test]
fn routes_refuse_the_owner_token_agents_and_apps_where_the_prd_says() {
    let (_g, _p) = Preview::on();
    let cases: Vec<(&str, Caller, bool, &str, &str)> = vec![
        (routes::JOIN, Caller::OwnerToken, true, "403 Forbidden", "owner_token_cannot_join"),
        (routes::JOIN, Caller::Passport, true, "403 Forbidden", "agents_not_in_voice"),
        (routes::LEAVE, Caller::Passport, true, "403 Forbidden", "agents_not_in_voice"),
        (routes::MODERATE, Caller::OwnerToken, true, "403 Forbidden", "owner_token_cannot_moderate"),
        (routes::MODERATE, Caller::Passport, true, "403 Forbidden", "agents_cannot_moderate"),
        (routes::SETTINGS, Caller::Passport, true, "403 Forbidden", "owner_or_admin_only"),
        (routes::ROOM_RENAME, Caller::Passport, true, "403 Forbidden", "owner_or_admin_only"),
        (routes::ROOMS, Caller::App, false, "403 Forbidden", "app_pass_not_allowed"),
        (routes::JOIN, Caller::Invalid, true, "401 Unauthorized", "unauthenticated"),
        // An ended phone pairing: a named code on every route, never 401.
        (routes::JOIN, Caller::DeviceEnded, true, "403 Forbidden", "device_session_ended"),
        (routes::LEAVE, Caller::DeviceEnded, true, "403 Forbidden", "device_session_ended"),
        (routes::ROOMS, Caller::DeviceEnded, false, "403 Forbidden", "device_session_ended"),
    ];
    for (path, caller, post, status, code) in cases {
        let r = routes::handle(path, caller.clone(), "owner", post, br#"{"room":"room-1","count":1,"name":"x"}"#);
        assert_eq!(r.status, status, "{path} {caller:?}: {}", r.body);
        let b = body(&r);
        assert_eq!(b["error"]["code"], code, "{path} {caller:?}");
        assert_eq!(b["code"], code, "mirrored for the client's parser");
    }
    // POST-only and GET-only.
    assert_eq!(routes::handle(routes::JOIN, Caller::OwnerToken, "owner", false, b"").status, "405 Method Not Allowed");
    assert_eq!(routes::handle(routes::ROOMS, Caller::OwnerToken, "owner", true, b"{}").status, "405 Method Not Allowed");
    // A body that isn't an object.
    let r = routes::handle(routes::SETTINGS, Caller::OwnerToken, "owner", true, b"[1]");
    assert_eq!(body(&r)["error"]["code"], "usage");
    // A Member login can't read status (the route floor says the same).
    let m = Caller::Login { username: "mo".into(), role: Role::Member, token: "t".into() };
    assert_eq!(body(&routes::handle(routes::STATUS, m, "owner", false, b""))["error"]["code"], "owner_or_admin_only");
}

#[test]
fn refusals_carry_retry_after_ms_in_details() {
    let r = routes::refuse(&bff::Refusal::new(503, "voice_member_pending", "retry").retry(2));
    assert_eq!(r.status, "503 Service Unavailable");
    let b = body(&r);
    assert_eq!(b["error"]["details"]["retry_after_ms"], 2000);
    assert_eq!(b["details"]["retry_after_ms"], 2000);
    assert_eq!(b["ok"], false);
}

#[test]
fn room_json_matches_the_client_wire() {
    let ok_h = hosting_of(&ready_facts());
    let people = vec![Occupant {
        member_id: "5".into(),
        external_id: Some("k2:owner".into()),
        person: "owner".into(),
        connection: "connected".into(),
        self_mute: true,
        self_deaf: false,
        server_mute: false,
        server_deaf: true,
        joined_at: "t".into(),
    }];
    let v = routes::room_json(&room(1, "901", ROOM_ACTIVE), &people, &ok_h, None);
    assert_eq!((v["id"].as_str(), v["name"].as_str(), v["limit"].as_u64()), (Some("room-1"), Some("Room 1"), Some(25)));
    assert_eq!(v["closing"], false);
    assert_eq!(v["unavailable"], Value::Null);
    let o = &v["occupants"][0];
    assert_eq!((o["user"].as_str(), o["role"].as_str(), o["muted"].as_bool(), o["deafened"].as_bool()), (Some("owner"), Some("owner"), Some(true), Some(true)));

    let mut f = ready_facts();
    f.lane = LaneState::down("voice_lane_down");
    let v = routes::room_json(&room(1, "901", ROOM_ACTIVE), &[], &hosting_of(&f), None);
    assert_eq!(v["unavailable"], "media_path_down", "M11: each room unavailable with that reason");
    let v = routes::room_json(&room(2, "902", ROOM_CLOSING), &[], &ok_h, Some(42));
    assert_eq!((v["closing"].as_bool(), v["closesAt"].as_i64()), (Some(true), Some(42)));
}
