//! The fake Sew speaks Sew 0.7.0's voice-rooms contract word for word
//! (K2 PRD v1.9 §6.6 2.2 = sew-56 v1.1d §2.2): SW-1, SW-3, SW-4, SW-11.
//!
//! These drive the fake through K2's own mgmt client and the
//! `k2_core::tool_hook::sew_wire` types, so every lane that builds on the
//! contract (Admin Space, rooms, the BFF feed, the media-path sender) gets
//! the same answers it will get from a real Sew 0.7.0. Isolation:
//! `TempHome::short()`; never the real `~/.k2` or `/run/sew`. Every test
//! fails loudly.

#![cfg(unix)]

#[path = "support/fake_sew.rs"]
mod fake_sew;

use std::time::{Duration, Instant};

use fake_sew::FakeSew;
use k2_core::test_env::TempHome;
use k2_core::tool_hook::features;
use k2_core::tool_hook::sew_wire::{self, ExternalId, MemberKind};
use k2_daemon::sew_mgmt::{long_poll_timeout, MgmtClient, MgmtResponse};
use serde_json::{json, Value};

const OWNER: &str = "k2:owner";
const ADMIN: &str = "k2:connect:3c4d5e6f-0000-4000-8000-000000000001";
const MEMBER: &str = "k2:connect:3c4d5e6f-0000-4000-8000-000000000002";
const AGENT: &str = "k2:workspace:3c4d5e6f-0000-4000-8000-000000000003";
const APP_USER: &str = "k2:principal:3c4d5e6f-0000-4000-8000-000000000004";

const ALL_070: [&str; 6] = [
    features::SERVER_GOVERNED_SPACE,
    features::MGMT_VOICE_STATES,
    features::VOICE_CHANNEL_CLOSING,
    features::TURN_RELAY_ONLY,
    features::VOICE_MEDIA_PATH,
    features::IMPORT_IDENTITY_MAP,
];

struct Env {
    _home: TempHome,
    sew: FakeSew,
    c: MgmtClient,
    n: std::cell::Cell<u32>,
}

async fn setup(flags: &[&str]) -> Env {
    let home = TempHome::short();
    let sew = FakeSew::start(&home.path().join("s")).await;
    sew.set_features(flags);
    for (ext, kind, user) in [
        (OWNER, MemberKind::Person, "owner"),
        (ADMIN, MemberKind::Person, "ada"),
        (MEMBER, MemberKind::Person, "mo"),
        (AGENT, MemberKind::Agent, "bobbin"),
        (APP_USER, MemberKind::Person, "appy"),
    ] {
        sew.seed(ext, kind, user, user, false);
    }
    let c = MgmtClient::new(sew.socket.clone(), sew.token_file.clone());
    Env { _home: home, sew, c, n: std::cell::Cell::new(0) }
}

impl Env {
    async fn post(&self, path: &str, body: Value) -> MgmtResponse {
        self.n.set(self.n.get() + 1);
        let key = format!("k2vt_{}", self.n.get());
        self.c
            .post(path, body.to_string().as_bytes(), &key, "k2:test")
            .await
            .unwrap_or_else(|e| panic!("POST {path}: {e}"))
    }
    async fn get(&self, path: &str) -> MgmtResponse {
        self.c.get(path).await.unwrap_or_else(|e| panic!("GET {path}: {e}"))
    }
}

fn json_of(r: &MgmtResponse) -> Value {
    serde_json::from_str(&r.body_text()).unwrap_or_else(|e| panic!("json {}: {e}", r.body_text()))
}

/// `(status, code, details.reason)` of an error answer.
fn refusal(r: &MgmtResponse) -> (u16, String, String) {
    let e = r.error().unwrap_or_else(|| panic!("expected an error, got {} {}", r.status, r.body_text()));
    let reason = e.error.detail_str("reason").unwrap_or_default().to_string();
    (r.status, e.error.code, reason)
}

async fn admin_space(env: &Env) -> String {
    let r = env.post(sew_wire::MGMT_SPACES_CREATE, serde_json::to_value(sew_wire::ServerSpaceCreateRequest::admin("sewlab")).unwrap()).await;
    assert_eq!(r.status, 200, "{}", r.body_text());
    let a: sew_wire::SpaceCreateResponse = serde_json::from_str(&r.body_text()).expect("create answer");
    assert_eq!((a.slug.as_str(), a.governance.as_deref(), a.created), ("k2-admin", Some("server"), true));
    a.space_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sw1_create_once_then_server_space_exists_names_it() {
    let env = setup(&ALL_070).await;
    let id = admin_space(&env).await;
    assert_eq!(env.sew.server_space_id().as_deref(), Some(id.as_str()));
    // A second create with a new key: 409 server_space_exists + details.space_id.
    let r = env.post(sew_wire::MGMT_SPACES_CREATE, serde_json::to_value(sew_wire::ServerSpaceCreateRequest::admin("sewlab")).unwrap()).await;
    assert_eq!(refusal(&r), (409, "conflict".into(), sew_wire::reasons::SERVER_SPACE_EXISTS.into()));
    assert_eq!(r.error().unwrap().error.detail_str("space_id"), Some(id.as_str()));
    // `k2:server` exists in Sew now and is never provisioned by K2.
    assert!(env.sew.member(sew_wire::K2_SERVER).is_some());
    // Any other create naming k2-admin: 422 slug_reserved.
    let r = env.post(sew_wire::MGMT_SPACES_CREATE, json!({"slug": "k2-admin", "name": "x", "owner": "k2:owner"})).await;
    assert_eq!(refusal(&r), (422, "validation_failed".into(), sew_wire::reasons::SLUG_RESERVED.into()));
    env.sew.assert_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sw1_slug_taken_and_identity_not_k2() {
    let env = setup(&ALL_070).await;
    env.sew.add_sew_space_with_slug("6001", "k2-admin");
    let body = serde_json::to_value(sew_wire::ServerSpaceCreateRequest::admin("sewlab")).unwrap();
    let r = env.post(sew_wire::MGMT_SPACES_CREATE, body.clone()).await;
    assert_eq!(refusal(&r), (409, "conflict".into(), sew_wire::reasons::SLUG_TAKEN.into()));

    let env = setup(&ALL_070).await;
    env.sew.set_provider("standalone");
    let r = env.post(sew_wire::MGMT_SPACES_CREATE, body).await;
    assert_eq!(refusal(&r), (409, "conflict".into(), sew_wire::reasons::IDENTITY_NOT_K2.into()));
    env.sew.assert_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sw1_without_the_flag_the_governance_field_is_refused_and_recorded() {
    let env = setup(&[]).await;
    let r = env.post(sew_wire::MGMT_SPACES_CREATE, serde_json::to_value(sew_wire::ServerSpaceCreateRequest::admin("sewlab")).unwrap()).await;
    assert_eq!(r.status, 400);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| env.sew.assert_clean()));
    assert!(caught.is_err(), "assert_clean must catch a server create sent to a Sew without the flag");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sw1_members_and_levels_box_users_and_agents_only() {
    let env = setup(&ALL_070).await;
    let id = admin_space(&env).await;
    let add = |ids: &[&str]| json!({"space_id": id, "external_ids": ids});
    let r = env.post(sew_wire::MGMT_SPACE_MEMBERS_ADD, add(&[OWNER, ADMIN, MEMBER, AGENT])).await;
    assert_eq!(r.status, 200, "{}", r.body_text());
    let a: sew_wire::SpaceMembersAddResponse = serde_json::from_str(&r.body_text()).unwrap();
    assert_eq!(a.added.len(), 4);
    // App users never: 422 app_user_not_allowed (and nothing added).
    let r = env.post(sew_wire::MGMT_SPACE_MEMBERS_ADD, add(&[APP_USER])).await;
    assert_eq!(refusal(&r), (422, "invalid_state".into(), sew_wire::reasons::APP_USER_NOT_ALLOWED.into()));
    assert!(!env.sew.space(&id).unwrap().members.contains(APP_USER));

    // Levels: bulk, app_id omitted.
    let item = |ext: &str, level: &str| sew_wire::MemberChannelsItem {
        external_id: ExternalId::parse(ext).unwrap(),
        channel_ids: vec![],
        level: level.into(),
    };
    let body = sew_wire::ServerMemberChannelsBulkRequest {
        items: vec![
            item(OWNER, sew_wire::LEVEL_MODERATOR),
            item(ADMIN, sew_wire::LEVEL_MODERATOR),
            item(MEMBER, sew_wire::LEVEL_MEMBER),
            item(AGENT, sew_wire::LEVEL_MEMBER),
            item(MEMBER, "admin"),
            item(AGENT, sew_wire::LEVEL_MODERATOR),
            item(APP_USER, sew_wire::LEVEL_MEMBER),
        ],
    };
    let r = env.post(&sew_wire::member_channels_bulk_path(&id), serde_json::to_value(&body).unwrap()).await;
    assert_eq!(r.status, 200, "{}", r.body_text());
    let b: sew_wire::MemberChannelsBulkResponse = serde_json::from_str(&r.body_text()).unwrap();
    let st: Vec<(u16, String)> = b
        .items
        .iter()
        .map(|i| (i.status, i.error.as_ref().map(|e| e.reason().to_string()).unwrap_or_default()))
        .collect();
    assert_eq!(
        st,
        vec![
            (200, String::new()),
            (200, String::new()),
            (200, String::new()),
            (200, String::new()),
            (422, sew_wire::reasons::LEVEL_ABOVE_CEILING.into()),
            (422, sew_wire::reasons::AGENT_LEVEL.into()),
            (422, sew_wire::reasons::APP_USER_NOT_ALLOWED.into()),
        ]
    );
    assert_eq!(env.sew.level(&id, OWNER).as_deref(), Some("moderator"));
    assert_eq!(env.sew.level(&id, MEMBER).as_deref(), Some("member"));
    // Single, app_id omitted: a Member promoted to Admin → moderator.
    let ext = ExternalId::parse(MEMBER).unwrap();
    let r = env
        .post(
            &sew_wire::member_channels_path(&id, &ext),
            serde_json::to_value(sew_wire::ServerMemberChannelsRequest { channel_ids: vec![], level: "moderator".into() }).unwrap(),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.body_text());
    assert_eq!(env.sew.level(&id, MEMBER).as_deref(), Some("moderator"));
    // Remove works with app_id omitted.
    let r = env.post(sew_wire::MGMT_SPACE_MEMBERS_REMOVE, json!({"space_id": id, "external_id": MEMBER})).await;
    assert_eq!(json_of(&r)["removed"], json!(true));
    env.sew.assert_clean();
}

async fn rooms(env: &Env, id: &str, ops: Vec<sew_wire::ChannelOp>) -> sew_wire::ChannelsBulkResponse {
    let r = env.post(sew_wire::MGMT_CHANNELS_BULK, serde_json::to_value(sew_wire::ServerChannelsBulkRequest { space_id: id.into(), ops }).unwrap()).await;
    assert_eq!(r.status, 200, "{}", r.body_text());
    serde_json::from_str(&r.body_text()).unwrap()
}

fn ensure(n: u32) -> sew_wire::ChannelOp {
    sew_wire::ChannelOp {
        op: sew_wire::channel_ops::ENSURE.into(),
        slug: Some(format!("room-{n}")),
        channel_id: None,
        name: Some(format!("Room {n}")),
        kind: Some(sew_wire::CHANNEL_TYPE_VOICE.into()),
        private: Some(false),
        user_limit: Some(25),
    }
}

fn by_slug(op: &str, slug: &str, name: Option<&str>) -> sew_wire::ChannelOp {
    sew_wire::ChannelOp { op: op.into(), slug: Some(slug.into()), channel_id: None, name: name.map(str::to_string), kind: None, private: None, user_limit: None }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sw1_rooms_sw3_feed_sw4_closing() {
    let env = setup(&ALL_070).await;
    let id = admin_space(&env).await;
    env.post(sew_wire::MGMT_SPACE_MEMBERS_ADD, json!({"space_id": id, "external_ids": [OWNER, ADMIN, MEMBER]})).await;
    let a = rooms(&env, &id, vec![ensure(1), ensure(2)]).await;
    assert!(a.results.iter().all(|r| r.created && r.error.is_none()), "{a:?}");
    let room2 = a.results[1].channel_id.clone().expect("id");
    let renamed = rooms(&env, &id, vec![by_slug("rename", "room-1", Some("Standup"))]).await;
    assert_eq!(renamed.results[0].name.as_deref(), Some("Standup"));
    assert_eq!(env.sew.channels(&id)["room-1"].user_limit, Some(25));

    // SW-3 snapshot.
    let r = env.get(&sew_wire::voice_states_path(&id)).await;
    let snap: sew_wire::VoiceStatesResponse = serde_json::from_str(&r.body_text()).unwrap();
    assert!(snap.states.is_empty());
    env.sew.voice_join(&id, "room-2", ADMIN).expect("join");
    env.sew.voice_join(&id, "room-1", MEMBER).expect("join");
    let r = env.get(&sew_wire::voice_changes_path(&id, &snap.cursor, 0, 500)).await;
    let feed: sew_wire::VoiceChangesResponse = serde_json::from_str(&r.body_text()).unwrap();
    let kinds: Vec<&str> = feed.changes.iter().map(|c| c.kind.as_str()).collect();
    assert_eq!(kinds, ["joined", "joined"]);
    assert_eq!(feed.changes[0].external_id, Some(ExternalId::parse(ADMIN).unwrap()));
    assert_eq!(feed.changes[0].state.as_ref().map(|s| s.connection.as_str()), Some("connected"));

    // SW-4: a live room goes closing; joins answer channel_closing; the last
    // leave archives it.
    let r = env.post(&sew_wire::channel_closing_path(&room2), serde_json::to_value(sew_wire::ChannelClosingRequest { closing: true }).unwrap()).await;
    let c: sew_wire::ChannelClosingResponse = serde_json::from_str(&r.body_text()).unwrap();
    assert_eq!((c.closing, c.archived), (true, false));
    assert_eq!(env.sew.voice_join(&id, "room-2", OWNER), Err(sew_wire::reasons::CHANNEL_CLOSING.into()));
    env.sew.voice_leave(ADMIN);
    assert!(env.sew.channels(&id)["room-2"].archived);
    let r = env.get(&sew_wire::voice_changes_path(&id, &feed.next, 0, 500)).await;
    let feed2: sew_wire::VoiceChangesResponse = serde_json::from_str(&r.body_text()).unwrap();
    let kinds: Vec<&str> = feed2.changes.iter().map(|c| c.kind.as_str()).collect();
    assert_eq!(kinds, ["channel_closing", "left", "channel_archived"]);
    // Un-closing an archived room: 409 channel_archived.
    let r = env.post(&sew_wire::channel_closing_path(&room2), json!({"closing": false})).await;
    assert_eq!(refusal(&r), (409, "conflict".into(), sew_wire::reasons::CHANNEL_ARCHIVED.into()));
    // An empty room closes straight to archived.
    let room1 = env.sew.channels(&id)["room-1"].id.to_string();
    env.sew.voice_leave(MEMBER);
    let r = env.post(&sew_wire::channel_closing_path(&room1), json!({"closing": true})).await;
    assert_eq!(json_of(&r), json!({"closing": true, "archived": true}));
    // Unarchive brings it back, not closing.
    rooms(&env, &id, vec![by_slug("unarchive", "room-1", None)]).await;
    let ch = &env.sew.channels(&id)["room-1"];
    assert!(!ch.archived && !ch.closing);
    // Archive with people in the call: everyone leaves channel_archived.
    env.sew.voice_join(&id, "room-1", OWNER).expect("join");
    let before = env.get(&sew_wire::voice_states_path(&id)).await;
    let cur: sew_wire::VoiceStatesResponse = serde_json::from_str(&before.body_text()).unwrap();
    rooms(&env, &id, vec![by_slug("archive", "room-1", None)]).await;
    let r = env.get(&sew_wire::voice_changes_path(&id, &cur.cursor, 0, 500)).await;
    let f: sew_wire::VoiceChangesResponse = serde_json::from_str(&r.body_text()).unwrap();
    assert_eq!(f.changes[0].reason.as_deref(), Some(sew_wire::leave_reasons::CHANNEL_ARCHIVED));
    // A text channel can't close.
    env.sew.add_channels(&id, 1);
    let text = env.sew.channels(&id)["job-0"].id.to_string();
    let r = env.post(&sew_wire::channel_closing_path(&text), json!({"closing": true})).await;
    assert_eq!(refusal(&r), (422, "invalid_state".into(), sew_wire::reasons::NOT_VOICE_CHANNEL.into()));
    env.sew.assert_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sw3_cursor_expires_after_a_restart_and_the_long_poll_holds() {
    let env = setup(&ALL_070).await;
    let id = admin_space(&env).await;
    let r = env.get(&sew_wire::voice_states_path(&id)).await;
    let snap: sew_wire::VoiceStatesResponse = serde_json::from_str(&r.body_text()).unwrap();
    // The long-poll: an empty answer is held up to `wait`; K2's call
    // timeout must be longer than `wait`.
    env.sew.set_voice_hold(Duration::from_millis(1500));
    let t0 = Instant::now();
    let r = env
        .c
        .get_with_timeout(&sew_wire::voice_changes_path(&id, &snap.cursor, 2, 100), long_poll_timeout(Duration::from_secs(2)))
        .await
        .expect("long poll");
    assert_eq!(r.status, 200);
    assert!(t0.elapsed() >= Duration::from_millis(1400), "held {:?}", t0.elapsed());
    assert_eq!(env.sew.voice_feed_queries().last().map(String::as_str), Some(format!("after={}&wait=2&limit=100", snap.cursor).as_str()));
    // A Sew restart: the old cursor answers 410 gone cursor_expired + head.
    env.sew.restart();
    let r = env.get(&sew_wire::voice_changes_path(&id, &snap.cursor, 0, 100)).await;
    assert_eq!(refusal(&r), (410, sew_wire::codes::GONE.into(), sew_wire::reasons::CURSOR_EXPIRED.into()));
    assert!(r.error().unwrap().error.detail_str("head").is_some());
    env.sew.assert_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sw11_media_path_down_ends_calls_and_refuses_joins_up_lifts() {
    let env = setup(&ALL_070).await;
    let id = admin_space(&env).await;
    env.post(sew_wire::MGMT_SPACE_MEMBERS_ADD, json!({"space_id": id, "external_ids": [OWNER, ADMIN]})).await;
    rooms(&env, &id, vec![ensure(1)]).await;
    env.sew.voice_join(&id, "room-1", OWNER).expect("join");
    let r = env.post(sew_wire::MGMT_VOICE_MEDIA_PATH, serde_json::to_value(sew_wire::MediaPathRequest::down(sew_wire::MEDIA_PATH_REASON_VOICE_LANE_DOWN)).unwrap()).await;
    let a: sew_wire::MediaPathResponse = serde_json::from_str(&r.body_text()).unwrap();
    assert_eq!((a.state.as_str(), a.ended_calls), ("down", 1));
    assert!(env.sew.voice_occupants(&id).is_empty(), "live calls end");
    assert_eq!(env.sew.voice_join(&id, "room-1", ADMIN), Err(sew_wire::reasons::MEDIA_PATH_DOWN.into()));
    let st: sew_wire::Status = serde_json::from_str(&env.get(sew_wire::MGMT_STATUS).await.body_text()).unwrap();
    let mp = st.voice.and_then(|v| v.media_path).expect("status.voice.media_path");
    assert_eq!((mp.state.as_str(), mp.media_path_reason.as_deref()), ("down", Some("voice_lane_down")));
    // Idempotent; then up lifts it at once.
    env.post(sew_wire::MGMT_VOICE_MEDIA_PATH, json!({"state": "down", "reason": "voice_lane_down"})).await;
    env.post(sew_wire::MGMT_VOICE_MEDIA_PATH, serde_json::to_value(sew_wire::MediaPathRequest::up()).unwrap()).await;
    env.sew.voice_join(&id, "room-1", ADMIN).expect("joins work again");
    // A restart starts `up` (K2 re-asserts down, M11).
    env.post(sew_wire::MGMT_VOICE_MEDIA_PATH, json!({"state": "down", "reason": "voice_lane_down"})).await;
    env.sew.restart();
    assert_eq!(env.sew.media_path(), None);
    env.sew.assert_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_070_flags_the_voice_routes_do_not_exist() {
    let env = setup(&[]).await;
    env.sew.add_server_space("7100");
    for path in [sew_wire::voice_states_path("7100"), sew_wire::voice_changes_path("7100", "0.0", 0, 1)] {
        assert_eq!(env.get(&path).await.status, 404, "{path}");
    }
    assert_eq!(env.post(&sew_wire::channel_closing_path("1"), json!({"closing": true})).await.status, 404);
    assert_eq!(env.post(sew_wire::MGMT_VOICE_MEDIA_PATH, json!({"state": "up"})).await.status, 404);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| env.sew.assert_clean()));
    assert!(caught.is_err(), "bad_paths records every call made without the flag");
}
