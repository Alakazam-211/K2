//! Voice rooms S1 + S2 (prd-k2-voice-rooms-v1 v1.9 §2, §3), end to end
//! against the fake Sew speaking Sew 0.7.0's contract (§6.6 2.2): the K2
//! Admin Space, its members and levels, the fixed default Space with the
//! agents' access carried over, and the rooms (count, rename, closing,
//! Close now, dormant below Pro).
//!
//! Tests drive `tool_hook_worker::{reconcile_once, pass_once}` directly.
//! Isolation: `TempHome::short()` (the env lock, so tests run one at a
//! time); never the real `~/.k2` or `/run/sew`. Every test fails loudly.

#![cfg(unix)]

#[path = "support/fake_sew.rs"]
mod fake_sew;

use std::time::Duration;

use fake_sew::FakeSew;
use k2_core::connect_users::{self, Role};
use k2_core::sew_chat::{store, ApprovalMode, Level};
use k2_core::skin::{self, SkinGrantWrite};
use k2_core::test_env::TempHome;
use k2_core::tool_hook::{self as hook, admin_space, features, sew_wire, CHAT_KIND};
use k2_core::voice_rooms::{self, Hosting};
use k2_daemon::tool_hook_worker::{self as worker, WorkerConfig};

const SEW_070: [&str; 6] = [
    "app_governed_spaces",
    features::SERVER_GOVERNED_SPACE,
    features::MGMT_VOICE_STATES,
    features::VOICE_CHANNEL_CLOSING,
    features::VOICE_MEDIA_PATH,
    features::TURN_RELAY_ONLY,
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
    _home: TempHome,
    sew: FakeSew,
}

impl Drop for Env {
    fn drop(&mut self) {
        k2_core::voice_preview::set_enabled(false);
        voice_rooms::set_hosting_for_tests(None);
    }
}

/// Chat on, the fake Sew reporting `flags`, the preview `preview`, and
/// this box hosting `hosting`.
async fn setup(flags: &[&str], preview: bool, hosting: Hosting) -> Env {
    let home = TempHome::short();
    hook::allow_target_off_linux_for_tests(true);
    let sew = FakeSew::start(&home.path().join("s")).await;
    sew.set_features(flags);
    k2_core::voice_preview::set_enabled(preview);
    voice_rooms::set_hosting_for_tests(Some(hosting));
    hook::set_target(CHAT_KIND, true, Some(sew.socket.to_str().unwrap()), Some(sew.token_file.to_str().unwrap()), "owner-token")
        .expect("enable chat target");
    // The main DB is one per process (not per TempHome): start each test
    // with no default Space and no agent access.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_default_space(&conn, None, "test", 0).expect("clear default space");
        conn.execute("DELETE FROM sew_chat_access", []).expect("clear access");
    }
    Env { _home: home, sew }
}

fn rows() -> Vec<hook::outbox::OutboxRow> {
    let mut r = hook::log(5000, None).expect("log");
    r.reverse();
    r
}

/// Sweep, then deliver until nothing is pending; again while a delivered
/// row asks for another sweep (the Space became ready).
async fn settle(env: &Env) {
    for _ in 0..15 {
        let _ = hook::take_reconcile_request();
        let r = worker::reconcile_once().await.expect("sweep");
        assert!(r.failed.is_empty(), "sweep failed: {:?}", r.failed);
        for _ in 0..40 {
            if rows().iter().all(|r| r.state != "pending") {
                break;
            }
            hook::force_due_for_tests().unwrap();
            worker::pass_once(&cfg()).await.expect("pass");
        }
        if rows().iter().all(|r| r.state != "pending") && !hook::reconcile_requested() {
            return;
        }
    }
    panic!(
        "never settled: {:?}\nfake log:\n{}",
        rows().iter().filter(|r| r.state == "pending").collect::<Vec<_>>(),
        env.sew.log()
    );
}

fn login(name: &str, role: Role) -> String {
    connect_users::add_user(name, "correct horse battery 9").expect("add login");
    if role != Role::Member {
        connect_users::set_role(name, role).expect("role");
    }
    let id = connect_users::admin_space_facts()
        .expect("facts")
        .into_iter()
        .find(|(_, u, _, _)| u == name)
        .map(|(id, ..)| id)
        .expect("the login has an id");
    format!("k2:connect:{id}")
}

fn space(env: &Env) -> String {
    env.sew.server_space_id().unwrap_or_else(|| panic!("no server Space in Sew:\n{}", env.sew.log()))
}

fn default_space() -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::default_space(&conn).expect("default space")
}

fn admin_posts(env: &Env) -> Vec<fake_sew::Call> {
    env.sew
        .posts()
        .into_iter()
        .filter(|c| {
            c.path == sew_wire::MGMT_SPACES_CREATE
                || c.path.ends_with("/closing")
                || (c.path == sew_wire::MGMT_CHANNELS_BULK && c.body.get("app_id").is_none())
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_off_nothing_is_created_and_nothing_changes() {
    let env = setup(&SEW_070, false, Hosting::Ok).await;
    login("ada", Role::Admin);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_default_space(&conn, Some("hearth"), "user:owner", 1).unwrap();
    }
    settle(&env).await;
    assert!(env.sew.server_space_id().is_none(), "no Admin Space");
    assert!(admin_posts(&env).is_empty(), "nothing sent: {}", env.sew.log());
    assert!(rows().iter().all(|r| !r.subject_key.starts_with("admin:")), "no Admin Space rows");
    assert_eq!(default_space().as_deref(), Some("hearth"), "the default Space is untouched");
    let v = hook::admin_space_view().unwrap();
    assert!(v.space.is_none() && v.members.is_empty() && v.rooms.is_empty());
    assert_eq!(
        hook::admin_space_last_summary().and_then(|s| s.waiting).as_deref(),
        Some("preview_off"),
        "the Admin Space step stopped at the switch"
    );
    env.sew.assert_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sew_without_server_spaces_gets_nothing() {
    // Sew 0.6.0: no server_governed_space.
    let env = setup(&["app_governed_spaces"], true, Hosting::Ok).await;
    login("ada", Role::Admin);
    settle(&env).await;
    assert!(env.sew.server_space_id().is_none());
    assert!(admin_posts(&env).is_empty(), "{}", env.sew.log());
    assert_eq!(hook::admin_space_last_summary().and_then(|s| s.waiting).as_deref(), Some("sew_too_old"));
    env.sew.assert_clean();
}

/// S1: the Space (word for word), box users at their levels, grants
/// written ahead, Room 1, and the fixed default Space.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_space_box_users_levels_room_one_and_the_default_space() {
    let env = setup(&SEW_070, true, Hosting::Ok).await;
    let ada = login("ada", Role::Admin);
    let mo = login("mo", Role::Member);
    let gone = login("dis", Role::Member);
    connect_users::set_disabled("dis", true).unwrap();
    settle(&env).await;
    let sp = space(&env);
    // The create, word for word (§6.6 2.2 SW-1).
    let create = env.sew.posts().into_iter().find(|c| c.path == sew_wire::MGMT_SPACES_CREATE).expect("a create");
    assert_eq!(create.body, serde_json::json!({"slug": "k5-admin", "name": "K5 Space", "owner": "k2:server", "governance": "server"}));
    assert_eq!(create.actor.as_deref(), Some(admin_space::ACTOR));
    // Box users only, at their levels; the disabled login stays out.
    let s = env.sew.space(&sp).unwrap();
    for (ext, level) in [("k2:owner", "moderator"), (ada.as_str(), "moderator"), (mo.as_str(), "member")] {
        assert!(s.members.contains(ext), "{ext} in the Admin Space");
        assert_eq!(env.sew.level(&sp, ext).as_deref(), Some(level), "{ext}");
    }
    assert!(!s.members.contains(&gone), "a disabled login is not in it");
    assert!(s.members.iter().all(|m| !m.starts_with("k2:principal:") && !m.starts_with("app:")));
    // Grants written ahead (§2.6): each login's own connect chat grant.
    for ext in [&ada, &mo] {
        let id = ext.trim_start_matches("k2:connect:");
        let g = skin::list_grants()
            .unwrap()
            .into_iter()
            .find(|g| g.kind == CHAT_KIND && g.subject_kind == "connect" && g.subject_id == id)
            .expect("connect chat grant");
        assert_eq!(g.created_by.as_deref(), Some(admin_space::ACTOR));
    }
    // No add or level ever carried an app_id.
    for c in env.sew.posts().iter().filter(|c| c.path == sew_wire::MGMT_SPACE_MEMBERS_ADD || c.path.ends_with("/channels")) {
        if c.body["space_id"] == sp.as_str() || c.path.contains(&format!("/spaces/{sp}/")) {
            assert!(c.body.get("app_id").is_none(), "no app_id in a server-Space body: {}", c.body);
        }
    }
    // Room 1: voice, public, user limit 25.
    let room = env.sew.channels(&sp).get("room-1").cloned().expect("room-1");
    assert_eq!((room.kind.as_str(), room.name.as_str(), room.private, room.user_limit, room.archived), ("voice", "Room 1", false, Some(25), false));
    assert_eq!(env.sew.channels(&sp).len(), 1);
    // The default Space is k5-admin now, and the view shows it all.
    assert_eq!(default_space().as_deref(), Some("k5-admin"));
    let v = hook::admin_space_view().unwrap();
    assert_eq!(v.space.as_ref().map(|s| (s.state.as_str(), s.space_id.clone())), Some(("ready", Some(sp.clone()))));
    assert!(v.space.as_ref().is_some_and(|s| s.count_stored), "voiceRoomsCount = 1 written at set-up");
    assert_eq!(k2_core::app_settings::load().voice_rooms_count, 1);
    assert!(v.holds.is_empty(), "nothing holds the sync: {:?}", v.holds);
    assert_eq!(v.rooms.iter().map(|r| (r.slug.as_str(), r.state.as_str())).collect::<Vec<_>>(), [("room-1", "active")]);
    assert!(v.members.iter().all(|m| m.state == "added" && m.level == m.want_level), "{:?}", v.members);
    // A second sweep sends nothing new.
    let before = env.sew.posts().len();
    settle(&env).await;
    assert_eq!(env.sew.posts().len(), before, "steady state:\n{}", env.sew.log());
    env.sew.assert_clean();
}

/// §2.6 + V13: a role change is a level sync; a removed login leaves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn role_changes_sync_levels_and_removed_logins_leave() {
    let env = setup(&SEW_070, true, Hosting::Ok).await;
    let ada = login("ada", Role::Admin);
    let mo = login("mo", Role::Member);
    settle(&env).await;
    let sp = space(&env);
    assert_eq!(env.sew.level(&sp, &mo).as_deref(), Some("member"));
    connect_users::set_role("mo", Role::Admin).unwrap();
    assert!(hook::reconcile_requested(), "set_role asks for a sweep");
    let mo_ext = sew_wire::ExternalId::parse(&mo).unwrap();
    settle(&env).await;
    assert_eq!(env.sew.level(&sp, &mo).as_deref(), Some("moderator"));
    assert!(!hook::admin_level_pending(&mo_ext).unwrap());
    connect_users::set_role("ada", Role::Member).unwrap();
    settle(&env).await;
    assert_eq!(env.sew.level(&sp, &ada).as_deref(), Some("member"));
    connect_users::remove_user("ada").unwrap();
    settle(&env).await;
    assert!(!env.sew.space(&sp).unwrap().members.contains(&ada), "a removed login leaves the Admin Space");
    assert!(hook::admin_space_view().unwrap().members.iter().all(|m| m.external_id != ada));
    env.sew.assert_clean();
}

/// S2: count up and down, rename, closing a live room, Close now, and
/// the rooms coming back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rooms_follow_the_count_rename_close_and_close_now() {
    let env = setup(&SEW_070, true, Hosting::Ok).await;
    let mo = login("mo", Role::Member);
    settle(&env).await;
    let sp = space(&env);
    hook::admin_rooms_set_count(3, false).unwrap();
    hook::admin_room_rename("2", "Standup").unwrap();
    settle(&env).await;
    let ch = env.sew.channels(&sp);
    assert_eq!(ch.len(), 3);
    assert_eq!(ch["room-2"].name, "Standup", "a renamed room");
    assert_eq!(ch["room-3"].name, "Room 3");
    assert!(ch.values().all(|c| c.kind == "voice" && !c.archived));
    // A rename of an existing room.
    hook::admin_room_rename("room-1", "Lobby").unwrap();
    settle(&env).await;
    assert_eq!(env.sew.channels(&sp)["room-1"].name, "Lobby");
    // Lower to 1 with someone in room-3: room-3 closing, room-2 archived.
    env.sew.voice_join(&sp, "room-3", &mo).expect("join room-3");
    hook::admin_rooms_set_count(1, false).unwrap();
    settle(&env).await;
    let ch = env.sew.channels(&sp);
    assert!(ch["room-2"].archived, "an empty room is archived at once");
    assert!(ch["room-3"].closing && !ch["room-3"].archived, "a live room is closing");
    assert_eq!(env.sew.voice_join(&sp, "room-3", "k2:owner"), Err(sew_wire::reasons::CHANNEL_CLOSING.into()));
    let v = hook::admin_space_view().unwrap();
    let st: Vec<(&str, &str)> = v.rooms.iter().map(|r| (r.slug.as_str(), r.state.as_str())).collect();
    assert_eq!(st, [("room-1", "active"), ("room-2", "archived"), ("room-3", "closing")]);
    // The last person leaves: Sew archives it (the SW-3 feed tells K2).
    env.sew.voice_leave(&mo);
    assert!(env.sew.channels(&sp)["room-3"].archived);
    let room3 = env.sew.channels(&sp)["room-3"].id.to_string();
    hook::admin_room_state(&room3, "archived").unwrap();
    // Back up to 3: both come back (unarchived), names kept.
    hook::admin_rooms_set_count(3, false).unwrap();
    settle(&env).await;
    let ch = env.sew.channels(&sp);
    assert!(!ch["room-2"].archived && !ch["room-3"].archived && !ch["room-3"].closing);
    assert_eq!(ch["room-2"].name, "Standup");
    // Close now: down to 1 with people in room-2: archived, calls end.
    env.sew.voice_join(&sp, "room-2", &mo).expect("join room-2");
    let notice = hook::admin_rooms_above(1).unwrap();
    assert_eq!(notice.iter().map(|r| r.slug.as_str()).collect::<Vec<_>>(), ["room-2", "room-3"]);
    let marked = hook::admin_rooms_set_count(1, true).unwrap();
    assert_eq!(marked, ["room-2", "room-3"]);
    settle(&env).await;
    let ch = env.sew.channels(&sp);
    assert!(ch["room-2"].archived && ch["room-3"].archived);
    assert!(env.sew.voice_occupants(&sp).is_empty(), "archive ended the call");
    // No room op ever asked for video or carried an app_id.
    for c in admin_posts(&env).iter().filter(|c| c.path == sew_wire::MGMT_CHANNELS_BULK) {
        for op in c.body["ops"].as_array().unwrap() {
            assert!(op.get("type").is_none_or(|t| t == "voice"), "{op}");
        }
    }
    env.sew.assert_clean();
}

/// §8: below Pro, rooms are dormant: none made, live ones go closing, the
/// count is kept. The Space is made anyway.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn below_pro_rooms_are_dormant_and_losing_pro_closes_them() {
    let env = setup(&SEW_070, true, Hosting::NotPro).await;
    settle(&env).await;
    let sp = space(&env);
    assert!(env.sew.channels(&sp).is_empty(), "no rooms below Pro");
    assert_eq!(hook::admin_space_last_summary().and_then(|s| s.rooms_dormant).as_deref(), Some("not_pro"));
    assert_eq!(default_space().as_deref(), Some("k5-admin"), "the Space and default exist whatever the tier");
    voice_rooms::set_hosting_for_tests(Some(Hosting::Ok));
    hook::kick_reconcile();
    settle(&env).await;
    assert!(env.sew.channels(&sp).contains_key("room-1"));
    env.sew.voice_join(&sp, "room-1", "k2:owner").expect("join");
    voice_rooms::set_hosting_for_tests(Some(Hosting::NotPro));
    settle(&env).await;
    let r1 = &env.sew.channels(&sp)["room-1"];
    assert!(r1.closing && !r1.archived, "losing Pro: live rooms go closing, the call stays");
    assert_eq!(k2_core::app_settings::load().voice_rooms_count, 1, "the count is kept");
    env.sew.assert_clean();
}

/// SW-1: `409 server_space_exists` → K2 reads `details.space_id` back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_existing_server_space_is_read_back() {
    let env = setup(&SEW_070, true, Hosting::Ok).await;
    env.sew.add_server_space("7777");
    settle(&env).await;
    assert_eq!(hook::admin_space_id().unwrap().as_deref(), Some("7777"));
    assert_eq!(env.sew.server_space_id().as_deref(), Some("7777"), "no second Space");
    let create = rows().into_iter().find(|r| r.subject_key == admin_space::SUBJECT_SPACE).expect("create row");
    assert_eq!((create.state.as_str(), create.last_status), ("done", Some(409)));
    assert!(env.sew.space("7777").unwrap().members.contains("k2:owner"));
    assert!(env.sew.channels("7777").contains_key("room-1"));
    env.sew.assert_clean();
}

/// SW-1 (sew-56 v1.2): the same create body replays `200 created: false`;
/// K2 takes it as success and uses that Space.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replayed_create_with_created_false_is_success() {
    let env = setup(&SEW_070, true, Hosting::Ok).await;
    // An earlier K2 made it under the name this box sends ("K5 Space").
    env.sew.add_server_space_named("7778", Some("K5 Space"));
    settle(&env).await;
    assert_eq!(hook::admin_space_id().unwrap().as_deref(), Some("7778"));
    assert_eq!(env.sew.server_space_id().as_deref(), Some("7778"), "no second Space");
    let create = rows().into_iter().find(|r| r.subject_key == admin_space::SUBJECT_SPACE).expect("create row");
    assert_eq!((create.state.as_str(), create.last_status), ("done", Some(200)));
    assert_eq!(create.result.as_ref().and_then(|r| r["created"].as_bool()), Some(false), "a replay");
    assert!(env.sew.channels("7778").contains_key("room-1"), "rooms follow");
    assert_eq!(default_space().as_deref(), Some("k5-admin"));
    env.sew.assert_clean();
}

/// SW-1: `409 slug_taken` blocks the Space, loudly; nothing else is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slug_taken_blocks_and_is_reported() {
    let env = setup(&SEW_070, true, Hosting::Ok).await;
    env.sew.add_sew_space_with_slug("6001", "k5-admin");
    settle(&env).await;
    // The next sweep sees the blocked Space and sends nothing more.
    settle(&env).await;
    let v = hook::admin_space_view().unwrap();
    let s = v.space.expect("a space row");
    assert_eq!(s.state, "blocked");
    assert!(s.why.as_deref().is_some_and(|w| w.contains("slug_taken") || w.contains("k5-admin")), "{s:?}");
    assert_eq!(v.holds.len(), 1, "the create is the one hold: {:?}", v.holds);
    assert_eq!(hook::admin_space_last_summary().and_then(|s| s.waiting).as_deref(), Some("space_blocked"));
    assert!(env.sew.posts().iter().all(|c| c.path != sew_wire::MGMT_CHANNELS_BULK), "no rooms");
    assert_ne!(default_space().as_deref(), Some("k5-admin"), "the default isn't switched to a Space that isn't there");
    env.sew.assert_clean();
}

/// §6.6 2.7 (c, d): on upgrade, a box with another default carries its
/// agents' access over (audited) and the agents join over mgmt; `set
/// default_space` is then fixed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_default_carries_agents_access_over_and_agents_join_over_mgmt() {
    let env = setup(&SEW_070, true, Hosting::Ok).await;
    let ws = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'Bobbin', ?2, 'bobbin')",
            rusqlite::params![ws, format!("/tmp/k2-va-{ws}")],
        )
        .expect("project");
        store::set_default_space(&conn, Some("hearth"), "user:owner", 1).unwrap();
        store::set_access(
            &conn,
            &store::Access {
                project_id: ws.clone(),
                space_id: "201".into(),
                space_slug: "hearth".into(),
                level: Level::Reply,
                approval: ApprovalMode::None,
                updated_by: "user:owner".into(),
                updated_at: 1,
            },
        )
        .unwrap();
    }
    skin::create_grant(&SkinGrantWrite {
        subject_kind: "workspace".into(),
        subject_id: ws.clone(),
        kind: CHAT_KIND.into(),
        target_id: String::new(),
        role: None,
        scope: Some("coworker".into()),
        created_by: Some("user:owner".into()),
    })
    .expect("agent chat grant");
    assert_eq!(k2_daemon::voice_admin::default_space_fixed(Some("other")).unwrap(), None, "nothing fixed before the Space exists");
    settle(&env).await;
    let sp = space(&env);
    assert_eq!(default_space().as_deref(), Some("k5-admin"));
    let carried = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_access(&conn, &ws, &sp).unwrap().expect("access carried to k5-admin")
    };
    assert_eq!((carried.space_slug.as_str(), carried.level, carried.updated_by.as_str()), ("k5-admin", Level::Reply, admin_space::ACTOR));
    let s = hook::admin_space_view().unwrap().space.unwrap();
    assert_eq!((s.default_from.as_deref(), s.carried.clone()), (Some("hearth"), vec![ws.clone()]));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let audit = store::audit_recent(&conn, 20).unwrap();
        assert!(audit.iter().any(|a| a.op == "default_space" && a.actor == admin_space::ACTOR), "the switch is audited");
    }
    // The agent joined over mgmt, as a member, never through act-as.
    let agent = format!("k2:workspace:{ws}");
    assert!(env.sew.space(&sp).unwrap().members.contains(&agent));
    assert_eq!(env.sew.level(&sp, &agent).as_deref(), Some("member"));
    assert!(env.sew.calls().iter().all(|c| c.act_as.is_none()), "no act-as anywhere");
    // `set default_space` is fixed now (409 default_space_fixed).
    assert!(k2_daemon::voice_admin::default_space_fixed(Some("hearth")).unwrap().is_some());
    assert!(k2_daemon::voice_admin::default_space_fixed(None).unwrap().is_some(), "clearing it is refused too");
    assert_eq!(k2_daemon::voice_admin::default_space_fixed(Some("k5-admin")).unwrap(), None);
    env.sew.assert_clean();
}
