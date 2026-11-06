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
        // The install stamp, stale Space records and hub cursors too.
        conn.execute("DELETE FROM sew_chat_settings WHERE key LIKE 'sew.%' OR key LIKE 'hub.cursor/%'", [])
            .expect("clear install stamp");
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

/// voicetest-pub 11-04: a restart on a Pro box planned before the tier was
/// read and closed every room. Unknown now changes nothing; the reading's
/// sync restores rooms an earlier build closed, same ids, slugs and names.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_tier_closes_nothing_and_pro_reopens_what_was_closed() {
    let env = setup(&SEW_070, true, Hosting::Ok).await;
    k2_core::app_settings::update(serde_json::json!({"voiceRoomsCount": 3})).expect("count 3");
    settle(&env).await;
    let sp = space(&env);
    let ids = |env: &Env| -> Vec<(String, u64, String, bool)> {
        let mut v: Vec<_> = env.sew.channels(&sp).iter().map(|(slug, c)| (slug.clone(), c.id, c.name.clone(), c.archived)).collect();
        v.sort();
        v
    };
    let before = ids(&env);
    assert_eq!(before.len(), 3, "{before:?}");
    assert!(before.iter().all(|r| !r.3));
    env.sew.voice_join(&sp, "room-1", "k2:owner").expect("join");

    // The restart: the sweep runs before the tier is read.
    voice_rooms::set_hosting_for_tests(Some(Hosting::Unknown));
    settle(&env).await;
    assert_eq!(ids(&env), before, "nothing closed or archived");
    assert!(env.sew.channels(&sp).values().all(|c| !c.closing), "nothing closing");
    assert_eq!(hook::admin_space_last_summary().and_then(|s| s.rooms_dormant).as_deref(), Some("tier_unknown"));

    // An earlier build closed them all; the tier then says Pro.
    env.sew.voice_leave("k2:owner");
    voice_rooms::set_hosting_for_tests(Some(Hosting::NotPro));
    settle(&env).await;
    assert!(env.sew.channels(&sp).values().all(|c| c.archived), "a real not-Pro reading still applies the rule");
    voice_rooms::set_hosting_for_tests(Some(Hosting::Ok));
    hook::kick_reconcile();
    settle(&env).await;
    let after = ids(&env);
    assert_eq!(after.iter().map(|r| (r.0.clone(), r.1, r.2.clone())).collect::<Vec<_>>(),
        before.iter().map(|r| (r.0.clone(), r.1, r.2.clone())).collect::<Vec<_>>(), "the same rooms come back");
    assert!(after.iter().all(|r| !r.3), "reopened: {after:?}");
    env.sew.assert_clean();
    k2_core::app_settings::update(serde_json::json!({"voiceRoomsCount": 1})).expect("count back");
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

// ── a lost K5 Space (voicetest 11-04) ────────────────────────────────

/// An agent workspace with a chat grant and access to the K5 Space `sp`.
fn agent_in(sp: &str) -> String {
    let ws = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'Bobbin', ?2, ?3)",
            rusqlite::params![ws, format!("/tmp/k2-va-{ws}"), format!("bobbin-{}", &ws[..8])],
        )
        .expect("project");
        store::set_access(
            &conn,
            &store::Access {
                project_id: ws.clone(),
                space_id: sp.into(),
                space_slug: sew_wire::ADMIN_SPACE_SLUG.into(),
                level: Level::Reply,
                approval: ApprovalMode::None,
                updated_by: "user:owner".into(),
                updated_at: 1,
            },
        )
        .expect("access");
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
    ws
}

fn access_spaces(ws: &str) -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::list_access(&conn, ws).expect("access").into_iter().map(|a| a.space_id).collect()
}

/// The ledger's rooms and the fake's channels agree, room by room.
fn assert_rooms_linked(env: &Env, sp: &str, slugs: &[&str]) {
    let ch = env.sew.channels(sp);
    let v = hook::admin_space_view().unwrap();
    for slug in slugs {
        let c = ch.get(*slug).unwrap_or_else(|| panic!("{slug} in Sew:\n{}", env.sew.log()));
        let r = v.rooms.iter().find(|r| r.slug == *slug).unwrap_or_else(|| panic!("{slug} in the ledger"));
        assert_eq!((r.channel_id.clone(), r.state.as_str()), (Some(c.id.to_string()), "active"), "{slug} re-linked");
        assert!(!c.archived && c.kind == "voice");
    }
}

fn check_outcome() -> (String, Option<String>, Option<String>) {
    let s = hook::admin_space_view().unwrap().space.expect("space row");
    let c = s.last_check.clone().unwrap_or_else(|| panic!("no check recorded: {s:?}"));
    (c["outcome"].as_str().expect("outcome").to_string(), c["from"].as_str().map(str::to_string), c["to"].as_str().map(str::to_string))
}

/// Sew loses the Space: the first add that hits it (an unmasked mgmt 404)
/// asks for a check, the check (the idempotent create) answers a new Space,
/// and K2 re-makes the rooms, re-adds every member (the blocked add too),
/// moves the agents' access, and skips what named the old id. `k2 voice
/// repair` after it finds the Space there and changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_space_is_confirmed_over_mgmt_then_made_again_and_relinked() {
    let env = setup(&SEW_070, true, Hosting::Ok).await;
    k2_core::app_settings::update(serde_json::json!({"voiceRoomsCount": 2})).expect("count 2");
    let ada = login("ada", Role::Admin);
    let mo = login("mo", Role::Member);
    settle(&env).await;
    let old = space(&env);
    let ws = agent_in(&old);
    hook::kick_reconcile();
    settle(&env).await;
    let agent = format!("k2:workspace:{ws}");
    assert!(env.sew.space(&old).unwrap().members.contains(&agent), "the agent joined");
    let old_rooms = env.sew.channels(&old);
    // Made long ago: a not_found may ask for a check now.
    hook::admin_space_rewind_for_tests(3600, false).unwrap();

    // Sew loses the Space and its channels.
    env.sew.drop_space(&old);
    // A new login's add hits the gone Space: 404 over mgmt (blocked).
    let cy = login("cy", Role::Member);
    let _ = hook::take_reconcile_request();
    worker::reconcile_once().await.expect("sweep");
    for _ in 0..20 {
        hook::force_due_for_tests().unwrap();
        worker::pass_once(&cfg()).await.expect("pass");
    }
    let blocked: Vec<_> = rows().into_iter().filter(|r| r.state == "blocked" && r.subject_key.starts_with(&format!("admin:m:{old}:"))).collect();
    assert!(!blocked.is_empty(), "the add to the gone Space is blocked:\n{}", env.sew.log());
    let s = hook::admin_space_view().unwrap().space.unwrap();
    assert_eq!(s.verify.as_deref(), Some(admin_space::VERIFY_SPACE), "the 404 asked for a check: {s:?}");
    assert!(s.suspect.as_deref().is_some_and(|w| w.contains("not_found")), "{s:?}");
    assert_eq!(k2_daemon::voice::routes::space_health(Some(&s), &k2_daemon::voice::occupancy::FeedStatus::default())["state"], "checking", "never 'ready' now");

    // The check, the re-link and everything after it.
    settle(&env).await;
    let new = space(&env);
    assert_ne!(new, old, "Sew made it again under a new id");
    assert_eq!(env.sew.server_space_count(), 1);
    assert_eq!(hook::admin_space_id().unwrap().as_deref(), Some(new.as_str()));
    assert_eq!(check_outcome(), ("remade".to_string(), Some(old.clone()), Some(new.clone())));
    // Every room again, linked to its new channel.
    assert_rooms_linked(&env, &new, &["room-1", "room-2"]);
    for (slug, c) in &old_rooms {
        assert_ne!(env.sew.channels(&new)[slug].id, c.id, "{slug} has a new channel id");
    }
    // Every member again at their level, the blocked add included.
    let sp = env.sew.space(&new).unwrap();
    for (ext, level) in [("k2:owner", "moderator"), (ada.as_str(), "moderator"), (mo.as_str(), "member"), (cy.as_str(), "member"), (agent.as_str(), "member")] {
        assert!(sp.members.contains(ext), "{ext} in the new Space:\n{}", env.sew.log());
        assert_eq!(env.sew.level(&new, ext).as_deref(), Some(level), "{ext}");
    }
    // What named the old id was skipped; nothing holds the sync.
    let old_rows: Vec<_> = rows().into_iter().filter(|r| r.subject_key.contains(&old)).filter(|r| r.state == "pending" || r.state == "blocked").collect();
    assert!(old_rows.is_empty(), "rows naming the old Space are closed: {old_rows:?}");
    let v = hook::admin_space_view().unwrap();
    assert!(v.holds.is_empty(), "{:?}", v.holds);
    assert!(v.members.iter().all(|m| m.state == "added"), "{:?}", v.members);
    // The agent's access followed the id; the default Space is still k5-admin.
    assert_eq!(access_spaces(&ws), [new.clone()]);
    assert_eq!(default_space().as_deref(), Some("k5-admin"));

    // `k2 voice repair` now: the Space is there, nothing changes.
    let ids = |env: &Env| -> Vec<(String, u64)> { env.sew.channels(&new).iter().map(|(k, c)| (k.clone(), c.id)).collect() };
    let before_ids = ids(&env);
    let start = k2_daemon::voice_admin::repair_begin("test repair").expect("repair");
    assert!(start.checking);
    settle(&env).await;
    let after = hook::admin_space_view().unwrap();
    assert!(k2_daemon::voice_admin::repair_settled(&start, &after));
    let rep = k2_daemon::voice_admin::repair_report(&start, &after, true).expect("report");
    assert_eq!(check_outcome().0, "same", "{rep}");
    assert!(rep["space"].as_str().unwrap().contains("is there"), "{rep}");
    assert_eq!(rep["rooms"], serde_json::json!([]), "no room moved: {rep}");
    assert_eq!(rep["notRestored"], serde_json::Value::Null);
    assert_eq!(rep["members"]["added"], 5, "{rep}");
    assert_eq!(space(&env), new);
    assert_eq!(ids(&env), before_ids, "same channels");
    assert_eq!(env.sew.server_space_count(), 1);
    env.sew.assert_clean();
    k2_core::app_settings::update(serde_json::json!({"voiceRoomsCount": 1})).expect("count back");
}

/// One room's channel is gone but the Space is there: `k2 voice repair`
/// makes just that room again and re-links it; the Space and the other
/// room keep their ids. A second run changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_room_is_made_again_alone() {
    let env = setup(&SEW_070, true, Hosting::Ok).await;
    k2_core::app_settings::update(serde_json::json!({"voiceRoomsCount": 2})).expect("count 2");
    login("ada", Role::Admin);
    settle(&env).await;
    let sp = space(&env);
    let room1 = env.sew.channels(&sp)["room-1"].id;
    let room2 = env.sew.channels(&sp)["room-2"].id;
    env.sew.drop_channel(&sp, "room-2");

    let start = k2_daemon::voice_admin::repair_begin("test repair").expect("repair");
    settle(&env).await;
    let rep = k2_daemon::voice_admin::repair_report(&start, &hook::admin_space_view().unwrap(), true).expect("report");
    assert_eq!(check_outcome().0, "same");
    assert_eq!(space(&env), sp, "the Space keeps its id");
    let ch = env.sew.channels(&sp);
    assert_eq!(ch["room-1"].id, room1, "room-1 untouched");
    assert_ne!(ch["room-2"].id, room2, "room-2 made again");
    assert_rooms_linked(&env, &sp, &["room-1", "room-2"]);
    assert_eq!(rep["rooms"], serde_json::json!([{"room": "room-2", "from": room2.to_string(), "to": ch["room-2"].id.to_string()}]), "{rep}");

    // Twice is a no-op.
    let posts = env.sew.posts().iter().filter(|c| c.path == sew_wire::MGMT_SPACES_CREATE).count();
    let start = k2_daemon::voice_admin::repair_begin("again").expect("repair");
    settle(&env).await;
    let rep = k2_daemon::voice_admin::repair_report(&start, &hook::admin_space_view().unwrap(), true).expect("report");
    assert_eq!(rep["rooms"], serde_json::json!([]), "{rep}");
    assert_eq!(env.sew.channels(&sp)["room-2"].id, ch["room-2"].id);
    assert_eq!(env.sew.posts().iter().filter(|c| c.path == sew_wire::MGMT_SPACES_CREATE).count(), posts + 1, "one check, no second Space");
    assert_eq!(env.sew.server_space_count(), 1);
    env.sew.assert_clean();
    k2_core::app_settings::update(serde_json::json!({"voiceRoomsCount": 1})).expect("count back");
}

/// voicetest 11-04 itself: Sew purged and installed again. The new install
/// id alone makes K2 check the Space (re-made, re-linked), set aside agent
/// Spaces from the old install (recorded, audited, access gone) and drop
/// hub cursors keyed to old member ids.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_sew_install_relinks_the_space_and_sets_aside_old_agent_spaces() {
    use rusqlite::OptionalExtension;
    let env = setup(&SEW_070, true, Hosting::Ok).await;
    login("ada", Role::Admin);
    settle(&env).await;
    let old = space(&env);
    let ws = agent_in(&old);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_access(
            &conn,
            &store::Access {
                project_id: ws.clone(),
                space_id: "6060".into(),
                space_slug: "u1".into(),
                level: Level::Manage,
                approval: ApprovalMode::None,
                updated_by: "user:owner".into(),
                updated_at: 1,
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sew_chat_settings (key, value, updated_by, updated_at) VALUES (?1, '{\"member\":\"1\",\"after\":0,\"held\":0}', 'hub', 1)",
            rusqlite::params![format!("hub.cursor/{ws}")],
        )
        .unwrap();
    }
    settle(&env).await;
    assert_eq!(access_spaces(&ws).len(), 2, "the same install keeps everything");

    env.sew.reinstall("2002");
    env.sew.drop_space(&old);
    settle(&env).await;
    let new = space(&env);
    assert_ne!(new, old);
    assert_eq!(check_outcome(), ("remade".to_string(), Some(old.clone()), Some(new.clone())));
    let s = hook::admin_space_view().unwrap().space.unwrap();
    assert_eq!(s.install_id.as_deref(), Some("2002"), "stamped with the new install");
    assert!(env.sew.space(&new).unwrap().members.contains("k2:owner"));
    assert_rooms_linked(&env, &new, &["room-1"]);
    // The old agent Space: recorded, access gone; the K5 access followed.
    assert_eq!(access_spaces(&ws), [new.clone()]);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let stale = k2_core::sew_chat::install::stale_spaces(&conn).unwrap();
    assert_eq!(stale.len(), 1, "{stale:?}");
    assert_eq!((stale[0].space_id.as_str(), stale[0].slug.as_str(), stale[0].install.as_deref()), ("6060", "u1", Some("1001")));
    assert_eq!(stale[0].access[0].project_id, ws);
    assert_eq!(stale[0].access[0].level, "manage");
    let cursor: Option<String> = conn
        .query_row("SELECT value FROM sew_chat_settings WHERE key = ?1", rusqlite::params![format!("hub.cursor/{ws}")], |r| r.get(0))
        .optional()
        .unwrap();
    assert_eq!(cursor, None, "the hub cursor keyed to an old member id is gone");
    assert_eq!(k2_core::sew_chat::install::stamp(&conn).unwrap().as_deref(), Some("2002"));
    drop(conn);
    env.sew.assert_clean();
}

/// A box a K2 from before the install stamp left with a lost Space (the
/// state voicetest is in after an upgrade): the first sweep checks it,
/// re-makes it and re-links it, with no signal and no repair.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unstamped_space_is_checked_once_after_an_upgrade() {
    let env = setup(&SEW_070, true, Hosting::Ok).await;
    let ada = login("ada", Role::Admin);
    settle(&env).await;
    let old = space(&env);
    let creates = || env.sew.posts().iter().filter(|c| c.path == sew_wire::MGMT_SPACES_CREATE).count();
    let n = creates();
    // A healthy unstamped Space: one check, nothing else changes.
    hook::admin_space_rewind_for_tests(0, true).unwrap();
    hook::kick_reconcile();
    settle(&env).await;
    assert_eq!((creates(), space(&env)), (n + 1, old.clone()), "one check");
    assert_eq!(check_outcome().0, "same");
    settle(&env).await;
    assert_eq!(creates(), n + 1, "stamped now: no second check");
    // The lost one.
    hook::admin_space_rewind_for_tests(0, true).unwrap();
    env.sew.drop_space(&old);
    hook::kick_reconcile();
    settle(&env).await;
    let new = space(&env);
    assert_ne!(new, old);
    assert_eq!(check_outcome().0, "remade");
    assert!(env.sew.space(&new).unwrap().members.contains(&ada));
    assert_rooms_linked(&env, &new, &["room-1"]);
    env.sew.assert_clean();
}

/// Phones (gap-voice-calls-mobile-v1 §11 row 4), through the real
/// `/cli/voice/*` handler and the real login store: a paired phone's device
/// session joins exactly like the same person's desktop login (same rules,
/// the same answer shape for sew-call-client), "in a call elsewhere" comes
/// from Sew's presence, leaves are idempotent, and an ended pairing is
/// refused by name. Sew's client API is a recording fake (`bff::set_deps`);
/// the presence is the real occupancy store fed from the fake Sew.
///
/// Linux only: voice rooms are hosted on Linux (`hosting_state` is
/// `not_linux` anywhere else, by design).
#[cfg(target_os = "linux")]
mod phone {
    use super::*;
    use std::sync::{Arc, Mutex};

    use k2_core::connect_users::TokenState;
    use k2_core::tool_hook::sew_wire::ExternalId;
    use k2_core::tool_hook::ConnectChatGate;
    use k2_daemon::sew_mgmt::MgmtClient;
    use k2_daemon::voice::bff::{self, Deps, SewAnswer, SewApi};
    use k2_daemon::voice::occupancy::{self, Feed};
    use k2_daemon::voice::routes::{self, Caller};
    use serde_json::{json, Value};

    const OWNER_TOKEN: &str = "owner-token-voice-phone-test";

    /// Sew's client API as the BFF sees it: every exchange, join and leave.
    #[derive(Default)]
    struct ClientSew {
        calls: Mutex<Vec<(Option<String>, String, Value)>>,
    }

    impl ClientSew {
        fn count(&self, path: &str) -> usize {
            self.calls.lock().unwrap().iter().filter(|c| c.1 == path).count()
        }
        fn passes(&self) -> Vec<Value> {
            self.calls.lock().unwrap().iter().filter(|c| c.1 == bff::paths::AUTH_K2).map(|c| c.2.clone()).collect()
        }
    }

    impl SewApi for ClientSew {
        fn post(&self, bearer: Option<&str>, path: &str, body: &Value) -> Result<SewAnswer, String> {
            self.calls.lock().unwrap().push((bearer.map(str::to_string), path.to_string(), body.clone()));
            let body = if path == bff::paths::AUTH_K2 {
                let n = self.count(bff::paths::AUTH_K2);
                json!({"token": format!("sews_{n}"), "expires_at": "2099-01-01T00:00:00Z"})
            } else if path.starts_with("/api/v1/channels/") && path.ends_with("/voice/join") {
                json!({"url": "wss://voice.lab.k2.dev", "token": "media", "media": {"relay_only": true},
                       "call_credential": {"token": "sewcc_sel.secret", "expires_at": "2099-01-01T00:00:00Z", "join_id": "j1", "device_scope": "d"}})
            } else if path == bff::paths::ME_VOICE_LEAVE {
                json!({"ok": true})
            } else if path.starts_with("/api/v1/recordings/") && path.ends_with("/consent") {
                json!({"id": "77", "state": "recording", "my_consent": if body["consent"] == true { "granted" } else { "declined" }})
            } else {
                return Err(format!("client sew fake: no answer for {path}"));
            };
            Ok(SewAnswer { status: 200, body })
        }
    }

    fn call(token: &str, path: &str, post: bool, body: Value) -> (String, Value) {
        let c = Caller::from_tcp("", Some(token), OWNER_TOKEN);
        let raw = if post { body.to_string().into_bytes() } else { Vec::new() };
        let r = routes::handle(path, c, OWNER_TOKEN, post, &raw);
        let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: json {e}: {}", r.body));
        (r.status.to_string(), v)
    }

    fn keys(v: &Value) -> Vec<String> {
        let mut k: Vec<String> = v.as_object().unwrap_or_else(|| panic!("not an object: {v}")).keys().cloned().collect();
        k.sort();
        k
    }

    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            bff::set_deps(None);
            bff::forget_all();
            occupancy::clear();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_paired_phone_joins_like_the_desktop_sees_elsewhere_and_an_ended_pairing_is_refused() {
        let env = setup(&SEW_070, true, Hosting::Ok).await;
        let _reset = Reset;
        let ada = login("ada", Role::Member);
        settle(&env).await;
        let sp = space(&env);
        let hosting = k2_daemon::voice::hosting_state();
        assert_eq!(hosting.state, "ok", "the box hosts rooms: {hosting:?}");

        let sew = Arc::new(ClientSew::default());
        bff::set_deps(Some(Arc::new(Deps {
            sew: sew.clone(),
            mint: Arc::new(|i: &k2_daemon::chat_pass::identity::PassIdentity| Ok::<_, String>(format!("pass-for-{}", i.sub))),
            connect_gate: Arc::new(|_: &str, _: &str| Ok::<_, String>(ConnectChatGate::Ready)),
            level_pending: Arc::new(|_: &ExternalId| Ok::<_, String>(false)),
            lane_hold: Duration::from_millis(200),
            member_wait: Duration::from_millis(100),
            presence: Arc::new(bff::live_presence),
        })));

        // ada's desktop login and her paired phone.
        let desk = connect_users::create_session("ada");
        let epoch = connect_users::live_user_epoch("ada").expect("ada's epoch");
        let phone = connect_users::create_device_session("ada", epoch, "ios-install-1", "Ada's iPhone", "user:ada").expect("pair the phone");
        assert_eq!(connect_users::token_state(&phone), TokenState::Live);
        for t in [&desk, &phone] {
            match Caller::from_tcp("", Some(t), OWNER_TOKEN) {
                Caller::Login { username, role, .. } => assert_eq!((username.as_str(), role), ("ada", Role::Member), "the same person and role"),
                other => panic!("a login caller, got {other:?}"),
            }
        }

        // Both join room 1 under the same rules and get the same answer shape.
        let join = json!({"room": "room-1", "join": {"call_credential": true, "self_mute": false, "self_deaf": false}});
        let (s, d1) = call(&desk, routes::JOIN, true, join.clone());
        assert_eq!(s, "200 OK", "{d1}");
        let (s, p1) = call(&phone, routes::JOIN, true, join.clone());
        assert_eq!(s, "200 OK", "{p1}");
        assert_eq!(keys(&p1), keys(&d1), "the phone's answer is the desktop's shape");
        assert_eq!(p1["join"], d1["join"], "Sew's join answer as is, call credential included");
        assert_eq!(p1["join"]["call_credential"]["token"], "sewcc_sel.secret");
        assert_eq!((p1["room"]["id"].as_str(), p1["relayOnly"].as_bool()), (Some("room-1"), Some(true)));
        let passes = sew.passes();
        assert_eq!(passes.len(), 2, "one Sew session per login session: {passes:?}");
        assert_eq!(passes[0], passes[1], "the same person: {passes:?}");
        assert_eq!(passes[1]["pass"], format!("pass-for-{ada}"));

        // ...and the same refusals: a Member doesn't moderate, read status or
        // change rooms.
        for t in [&desk, &phone] {
            let (s, v) = call(t, routes::MODERATE, true, json!({"room": "room-1", "person": "ada", "action": "mute"}));
            assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("403 Forbidden", Some("moderator_only")), "{v}");
            let (s, v) = call(t, routes::STATUS, false, json!({}));
            assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("403 Forbidden", Some("owner_or_admin_only")), "{v}");
            let (s, v) = call(t, routes::SETTINGS, true, json!({"count": 2}));
            assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("403 Forbidden", Some("owner_or_admin_only")), "{v}");
        }

        // Sew lists ada in room 1 (the phone's call; the desktop's ended when
        // the phone joined): the real occupancy store, fed from the fake.
        env.sew.voice_join(&sp, "room-1", &ada).expect("ada in room-1 on Sew");
        let mgmt = MgmtClient::new(env.sew.socket.clone(), env.sew.token_file.clone());
        let mut feed = Feed::default();
        feed.step(&mgmt, &sp, 0).await.expect("voice/states");
        let (s, rd) = call(&desk, routes::ROOMS, false, json!({}));
        assert_eq!(s, "200 OK", "{rd}");
        let me = &rd["me"];
        assert_eq!((me["inCall"].as_bool(), me["elsewhere"].as_bool(), me["here"].as_bool()), (Some(true), Some(true), Some(false)), "{me}");
        assert_eq!(me["on"], json!({"kind": "phone", "name": "Ada's iPhone"}));
        assert_eq!((me["room"]["id"].as_str(), me["mayBeLeaving"].as_bool()), (Some("room-1"), Some(true)));
        assert_eq!(me["callId"], Value::Null);
        let (_, rp) = call(&phone, routes::ROOMS, false, json!({}));
        assert_eq!((rp["me"]["here"].as_bool(), rp["me"]["callId"].as_str()), (Some(true), p1["callId"].as_str()), "{rp}");
        // The owner token is nobody in a room.
        let (_, ro) = call(OWNER_TOKEN, routes::ROOMS, false, json!({}));
        assert_eq!(ro["me"], Value::Null);

        // The desktop's stale leave ends nothing; the phone leaves twice.
        let (s, v) = call(&desk, routes::LEAVE, true, json!({"callId": d1["callId"]}));
        assert_eq!((s.as_str(), v["already"].as_bool(), v["why"].as_str()), ("200 OK", Some(true), Some("joined_elsewhere")), "{v}");
        assert_eq!(sew.count(bff::paths::ME_VOICE_LEAVE), 0, "the phone's call is untouched");
        let (s, v) = call(&desk, routes::LEAVE, true, json!({"callId": p1["callId"]}));
        assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("404 Not Found", Some("call_not_found")), "not the desktop's call: {v}");
        let (s, v) = call(&phone, routes::LEAVE, true, json!({"callId": p1["callId"]}));
        assert_eq!((s.as_str(), v["left"].as_bool(), v["already"].as_bool()), ("200 OK", Some(true), Some(false)), "{v}");
        let (s, v) = call(&phone, routes::LEAVE, true, json!({"callId": p1["callId"]}));
        assert_eq!((s.as_str(), v["already"].as_bool(), v["why"].as_str()), ("200 OK", Some(true), Some("already_left")), "{v}");
        assert_eq!(sew.count(bff::paths::ME_VOICE_LEAVE), 1);

        // Sew drops ada: not in a call any more.
        env.sew.voice_leave(&ada);
        feed.step(&mgmt, &sp, 0).await.expect("voice/changes");
        let (_, rp) = call(&phone, routes::ROOMS, false, json!({}));
        assert_eq!(rp["me"]["inCall"], false, "{rp}");

        // A recording notice: the phone answers as ada, on its own Sew
        // session (the one its join used); the owner token can't.
        let (s, v) = call(&phone, routes::RECORDING_CONSENT, true, json!({"recording_id": "77", "consent": true, "notice_version": 2}));
        assert_eq!(s, "200 OK", "{v}");
        assert_eq!((v["recordingId"].as_str(), v["recording"]["my_consent"].as_str()), (Some("77"), Some("granted")), "{v}");
        {
            let calls = sew.calls.lock().unwrap();
            let last = calls.last().cloned().expect("a call");
            assert_eq!((last.1.as_str(), &last.2), ("/api/v1/recordings/77/consent", &json!({"consent": true, "notice_version": 2})));
            let joins: Vec<Option<String>> = calls.iter().filter(|c| c.1.ends_with("/voice/join")).map(|c| c.0.clone()).collect();
            assert!(last.0.is_some(), "a person's Sew session, never none");
            assert_eq!(last.0, joins[1], "the phone's own Sew session (the desktop joined first): {joins:?}");
        }
        let before = sew.calls.lock().unwrap().len();
        let (s, v) = call(OWNER_TOKEN, routes::RECORDING_CONSENT, true, json!({"recording_id": "77", "consent": false}));
        assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("403 Forbidden", Some("owner_token_cannot_join")), "{v}");
        let (s, v) = call(&phone, routes::RECORDING_CONSENT, true, json!({"recording_id": "77", "consent": true}));
        assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("400 Bad Request", Some("usage")), "a grant names its notice: {v}");
        assert_eq!(sew.calls.lock().unwrap().len(), before, "refusals reach nothing");

        // The pairing is revoked: a named code on every route, never a 401,
        // and nothing reaches Sew. The desktop is untouched.
        let id = connect_users::device_sessions(Some("ada"))
            .expect("devices")
            .into_iter()
            .find(|d| d.device_id == "ios-install-1")
            .expect("the phone")
            .id;
        connect_users::revoke_device_session(&id, None).expect("revoke").expect("revoked one");
        let before = sew.calls.lock().unwrap().len();
        for (path, post) in [(routes::JOIN, true), (routes::LEAVE, true), (routes::ROOMS, false), (routes::RECORDING_CONSENT, true)] {
            let (s, v) = call(&phone, path, post, join.clone());
            assert_eq!(
                (s.as_str(), v["error"]["code"].as_str(), v["code"].as_str()),
                ("403 Forbidden", Some("device_session_ended"), Some("device_session_ended")),
                "{path}: {v}"
            );
        }
        assert_eq!(sew.calls.lock().unwrap().len(), before, "an ended pairing reaches nothing");
        let (s, v) = call(&desk, routes::JOIN, true, join);
        assert_eq!(s, "200 OK", "the desktop still joins: {v}");
        env.sew.assert_clean();
    }
}
