//! `me.events`: an open screen hears that its access changed
//! (`prd-live-access-changes-v1` S3, §5, §14).
//!
//! Real daemon, real sockets, headless. Every trigger on the PRD's list reaches an
//! open socket, and `me.get` then shows the change. A no-op, or someone else's
//! change, sends nothing.

#[path = "support/app_fixture.rs"]
mod app_fixture;
#[path = "support/live_sockets.rs"]
mod live_sockets;
#[path = "support/fake_sew.rs"]
mod fake_sew;

use std::time::{Duration, Instant};

use app_fixture::*;
use k2_core::skin;
use live_sockets::*;
use serde_json::{json, Value};

const FRAME: &str = r#"{"kind":"access_changed"}"#;
/// A walk is 100 ms away; room for a busy node.
const HEARS_WITHIN_MS: u64 = 2_500;

async fn me_socket(port: u16, pass: &str) -> Sock {
    let s = open(port, &format!("/cli/skin/me/events?scope=me&token={}", enc(pass))).await;
    s
}

async fn hears(s: &mut Sock, what: &str) {
    match next_frame(s, HEARS_WITHIN_MS).await {
        Ok(Some(f)) => assert_eq!(f, FRAME, "{what}: the frame carries nothing else"),
        Ok(None) => panic!("{what}: no access_changed within {HEARS_WITHIN_MS} ms"),
        Err(close) => panic!("{what}: the socket closed instead of telling: {close:?}"),
    }
    // A writer with several writes in a row may be more than one walk.
    settle(s).await;
}

/// Take whatever else the same change sent.
async fn settle(s: &mut Sock) {
    while let Ok(Some(_)) = next_frame(s, 600).await {}
}

async fn silent(s: &mut Sock, ms: u64, what: &str) {
    let seen = watch(s, ms).await;
    assert!(seen.frames.is_empty() && !seen.ended, "{what}: {seen:?}");
}

async fn closes(s: &mut Sock, want: u16, what: &str) {
    let seen = watch(s, HEARS_WITHIN_MS).await;
    assert_eq!(seen.close, Some(want), "{what}: {seen:?}");
}

async fn me(port: u16, pass: &str) -> Value {
    let r = get(port, "/cli/skin/me", pass, "x=1").await;
    assert_eq!(r.status, 200, "me.get: {}", r.body);
    r.json()
}

fn caps_in(me: &Value, room: &str) -> Vec<String> {
    let rooms = me["rooms"].as_array().expect("rooms");
    match rooms.iter().find(|r| r["room"] == room) {
        Some(r) => r["caps"].as_array().expect("caps").iter().map(|c| c.as_str().expect("cap").to_string()).collect(),
        None => Vec::new(),
    }
}

fn holds(me: &Value, room: &str) -> bool {
    me["rooms"].as_array().expect("rooms").iter().any(|r| r["room"] == room)
}

fn pid_of(pass: &str) -> String {
    skin::resolve_skin_token(pass).expect("live").principal_id.expect("a person")
}

fn host_grant_id(pid: &str) -> String {
    let grants = skin::list_grants().expect("grants");
    grants.iter().find(|g| g.subject_id == pid && g.kind == "app").unwrap_or_else(|| panic!("no app grant for {pid}: {grants:?}")).id.clone()
}

fn new_app(ws: &Ws, app: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let svc = k2_core::published_services::insert(&conn, &ws.id, app, k2_core::published_services::CMD_APP_SENTINEL, &ws.path.to_string_lossy(), 0, "local", "stopped", "cmd", "").expect("service").id;
    k2_core::deploy::store::insert_app(
        &conn,
        &k2_core::deploy::store::NewApp { app_id: app, service_id: &svc, seed_id: None, owner_workspace: &ws.id, isolation: "process", memory_limit_bytes: None, front_port: 0, created_by: "owner" },
    )
    .expect("insert app");
    svc
}

/// An App guest with its own role in `rooms`, signed in to `svc`.
fn app_guest(user: &str, svc: &str, role: &str) -> String {
    skin::add_principal(user).expect("person");
    let pid = skin::host_login_detail(user).expect("guest").principal_id;
    skin::create_grant(&skin::SkinGrantWrite { subject_kind: "principal".into(), subject_id: pid, kind: "app".into(), target_id: svc.to_string(), role: Some(role.to_string()), scope: None, created_by: Some("owner".into()) })
        .expect("app grant");
    skin::create_app_session_token(user, svc).expect("App session").raw
}

// ── The door ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_door() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "medoor");
    let (_, pass) = guest_in(port, &ws, "medoorguest", ALL_CAPS).await;
    let refused = |pq: String, want: u16| async move {
        match tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}{pq}")).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status().as_u16(), want, "{pq}"),
            other => panic!("{pq}: {:?}", other.map(|_| "opened")),
        }
    };
    refused("/cli/skin/me/events?scope=me".to_string(), 403).await;
    refused(format!("/cli/skin/me/events?scope=me&token={OWNER}"), 403).await;
    refused("/cli/skin/me/events?scope=me&token=k2skn_a_dead_pass_not_real".to_string(), 401).await;
    refused(format!("/cli/skin/me/events?token={}", enc(&pass)), 400).await;
    refused(format!("/cli/skin/me/events?scope=everyone&token={}", enc(&pass)), 400).await;

    // A Connect login can't open it: a role change signs that person out (ruled).
    k2_core::connect_users::add_user("mellogin", "password123").expect("add_user");
    let login = k2_core::connect_users::create_session("mellogin");
    refused(format!("/cli/skin/me/events?scope=me&token={}", enc(&login)), 403).await;

    let mut open_ones = Vec::new();
    for _ in 0..16 {
        open_ones.push(me_socket(port, &pass).await);
    }
    refused(format!("/cli/skin/me/events?scope=me&token={}", enc(&pass)), 429).await;
    drop(open_ones);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _again = me_socket(port, &pass).await;

    // The feature key tells a client it's here.
    let m = me(port, &pass).await;
    assert!(m["features"].as_array().expect("features").iter().any(|f| f == "live-access-v1"), "{m}");
    assert_eq!(m["chat"], false, "{m}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_the_hub_off_there_is_no_socket_and_no_feature() {
    let env = setup(&[("K2_ACCESS_HUB", "off")]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "meoff");
    let (_, pass) = guest_in(port, &ws, "meoffguest", ALL_CAPS).await;
    match tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/cli/skin/me/events?scope=me&token={}", enc(&pass))).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status().as_u16(), 404),
        other => panic!("{:?}", other.map(|_| "opened")),
    }
    let m = me(port, &pass).await;
    assert!(!m["features"].as_array().expect("features").iter().any(|f| f == "live-access-v1"), "{m}");
    assert!(m.get("chat").is_some(), "me.get keeps its field: {m}");
}

// ── Every trigger ────────────────────────────────────────────────────

/// PRD §5, row by row. Each: an open socket hears, and `me.get` then shows it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_trigger_reaches_an_open_screen() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "mea");
    let other = seed_ws(&env, "meb");

    // Role's rooms or caps.
    let (role, pass) = guest_in(port, &ws, "metcaps", &["thread:read", "files:read"]).await;
    let mut s = me_socket(port, &pass).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read"] })).await;
    hears(&mut s, "a role's caps").await;
    assert_eq!(caps_in(&me(port, &pass).await, &ws.id), ["thread:read"]);
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": other.handle, "caps": ["thread:read"] })).await;
    hears(&mut s, "a role's rooms").await;
    assert!(holds(&me(port, &pass).await, &other.id));

    // Role's folder lines.
    let scope = skin::RoomPathScope { read: vec!["docs".into()], ..Default::default() };
    skin::paths::set_role_room_paths(&role, &ws.id, &scope).expect("folders");
    hears(&mut s, "a role's folders").await;
    let m = me(port, &pass).await;
    let row = m["rooms"].as_array().expect("rooms").iter().find(|r| r["room"] == ws.id.as_str()).expect("room").clone();
    assert_eq!(row["folders"]["read"], json!(["docs"]), "{m}");

    drop(s);

    // A person's role: unassign, then assign.
    let (role, pass) = guest_in(port, &ws, "metrole", &["thread:read"]).await;
    let mut s = me_socket(port, &pass).await;
    owner_ok(port, "/cli/skin/roles/unassign", json!({ "username": "metrole" })).await;
    hears(&mut s, "unassigned").await;
    // Lost everything but still signed in: told, not closed.
    let m = me(port, &pass).await;
    assert_eq!(m["rooms"], json!([]), "{m}");
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": "metrole", "role": role })).await;
    hears(&mut s, "assigned: a roomless socket hears a room arrive").await;
    assert!(holds(&me(port, &pass).await, &ws.id));
    drop(s);

    // Chat on and off.
    let (_, pass) = guest_in(port, &ws, "metchat", &["thread:read"]).await;
    let mut s = me_socket(port, &pass).await;
    // Chat has to be on for a grant to exist. No Sew behind it: the grant's row is
    // what K2 answers from, and its outbox row just waits.
    k2_core::tool_hook::allow_target_off_linux_for_tests(true);
    let (sew_socket, sew_token) = (env.path().join("sew.sock"), env.path().join("sew.token"));
    std::fs::write(&sew_token, "test-token").expect("token file");
    k2_core::tool_hook::set_target(skin::CHAT_GRANT_KIND, true, Some(sew_socket.to_str().expect("path")), Some(sew_token.to_str().expect("path")), "owner-token").expect("enable chat");
    let chat = skin::create_grant(&skin::SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: pid_of(&pass),
        kind: skin::CHAT_GRANT_KIND.into(),
        target_id: skin::CHAT_GRANT_TARGET.into(),
        role: None,
        scope: None,
        created_by: Some("owner".into()),
    })
    .expect("chat grant");
    hears(&mut s, "chat granted").await;
    assert_eq!(me(port, &pass).await["chat"], true);
    assert!(skin::delete_grant_by_id(&chat.id).expect("delete"));
    hears(&mut s, "chat taken").await;
    assert_eq!(me(port, &pass).await["chat"], false);
    drop(s);

    // Grant disabled: no enabled grant, so the session's dead too.
    let (_, pass) = guest_in(port, &ws, "metoff", &["thread:read"]).await;
    let mut s = me_socket(port, &pass).await;
    skin::set_grant_enabled(&host_grant_id(&pid_of(&pass)), false).expect("disable");
    closes(&mut s, 4003, "a grant disabled").await;
    assert_eq!(get(port, "/cli/skin/me", &pass, "x=1").await.status, 401);

    // Grant removed.
    let (_, pass) = guest_in(port, &ws, "metdel", &["thread:read"]).await;
    let mut s = me_socket(port, &pass).await;
    assert!(skin::delete_grant_by_id(&host_grant_id(&pid_of(&pass))).expect("delete"));
    closes(&mut s, 4003, "a grant removed").await;

    // Session ended.
    let (_, pass) = guest_in(port, &ws, "metend", &["thread:read"]).await;
    let mut s = me_socket(port, &pass).await;
    assert!(skin::revoke_token(&skin::resolve_skin_token(&pass).expect("live").id).expect("revoke"));
    closes(&mut s, 4003, "a session ended").await;

    // User deleted.
    let (_, pass) = guest_in(port, &ws, "metgone", &["thread:read"]).await;
    let mut s = me_socket(port, &pass).await;
    owner_ok(port, "/cli/skin/users/remove", json!({ "username": "metgone" })).await;
    closes(&mut s, 4003, "a user deleted").await;

    // Platform pass: its rooms, then its revoke (V12).
    owner_ok(port, "/cli/skin/users", json!({ "username": "metplat" })).await;
    let plat = owner_ok(port, "/cli/skin-tokens", json!({ "name": "metplat", "caps": ["thread:read"], "rooms": [ws.handle] })).await;
    let (raw, id) = (plat["token"].as_str().expect("token").to_string(), plat["id"].as_str().expect("id").to_string());
    let mut s = me_socket(port, &raw).await;
    owner_ok(port, "/cli/skin-tokens/rooms", json!({ "id": id, "rooms": [other.handle] })).await;
    hears(&mut s, "a platform pass's rooms").await;
    assert!(holds(&me(port, &raw).await, &other.id));
    owner_ok(port, "/cli/skin-tokens/revoke", json!({ "id": id })).await;
    closes(&mut s, 4003, "a platform pass revoked").await;

    // A room's agent is gone: the pass still names it (main database, a hint).
    let doomed = seed_ws(&env, "medoomed");
    let (_, pass) = guest_in(port, &ws, "metdoom", &["thread:read"]).await;
    let role = "role-metdoom";
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": doomed.handle, "caps": ["thread:read", "files:read", "activity:read"] })).await;
    let mut s = me_socket(port, &pass).await;
    let mut doomed_files = open(port, &format!("/cli/fs/events?workspace={}&token={}", doomed.handle, enc(&pass))).await;
    let mut doomed_activity = open(port, &format!("/cli/activity/events?workspace={}&token={}", doomed.handle, enc(&pass))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(holds(&me(port, &pass).await, &doomed.id), "rig");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::delete(&conn, &doomed.id).expect("delete");
    }
    hears(&mut s, "a room's agent removed").await;
    assert!(!holds(&me(port, &pass).await, &doomed.id));
    // The role still names the room, and its sockets go anyway: the same folder could
    // come back under a new id, and this pass has no rights there.
    closes(&mut doomed_files, 4013, "a removed room's files socket").await;
    closes(&mut doomed_activity, 4013, "a removed room's activity socket").await;
    drop(s);

    // A project's members, also in the main database (V11).
    let group = project(port, "mepro", &[&ws]).await;
    let (_, pass) = guest_in(port, &ws, "metproj", &["thread:read"]).await;
    let mut s = me_socket(port, &pass).await;
    owner_ok(port, "/cli/project-group/add-member", json!({ "group": group, "workspace": other.id })).await;
    hears(&mut s, "a project grew past this person").await;
    drop(s);

    // App: a login's grant lines, its role, a tie, a handover, an uninstall.
    let svc = new_app(&ws, "metapp");
    let (r1, _) = guest_in(port, &ws, "metrolea", &["thread:read"]).await;
    let (r2, _) = guest_in(port, &ws, "metroleb", &["thread:read", "files:read"]).await;
    let pass = app_guest("metguest", &svc, &r1);
    let mut s = me_socket(port, &pass).await;
    let grant = skin::list_grants().expect("grants").into_iter().find(|g| g.target_id == svc && g.subject_id == pid_of(&pass)).expect("grant");
    skin::set_app_grant_role(&grant.id, &r2).expect("role");
    hears(&mut s, "an App grant's role").await;
    assert_eq!(caps_in(&me(port, &pass).await, &ws.id), ["thread:read", "files:read"]);

    // A handover moves the App's own roles (a shared role naming the old room blocks
    // it instead), so this login takes one first.
    let mut own_policy = skin::RoomPolicy::new();
    own_policy.insert(ws.id.clone(), vec!["thread:read".to_string()]);
    skin::create_role_for_app("metapprole", &own_policy, Some(&svc)).expect("the App's own role");
    skin::set_app_grant_role(&grant.id, "metapprole").expect("role");
    hears(&mut s, "an App's own role").await;
    let moved_to = seed_ws(&env, "memoved");
    let report = skin::handover::move_rooms(&svc, &ws.id, &moved_to.id).expect("handover");
    assert_eq!(report.roles, 1, "rig: the handover moved the App's role");
    hears(&mut s, "a handover").await;
    let m = me(port, &pass).await;
    assert!(holds(&m, &moved_to.id) && !holds(&m, &ws.id), "{m}");

    let out = post(port, "/cli/app/uninstall", OWNER, json!({ "app": "metapp", "dropAccess": true })).await;
    assert_eq!(out.status, 200, "uninstall: {}", out.body);
    closes(&mut s, 4003, "an App uninstalled").await;
}

/// A tie narrows a live login to its App's rooms.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tie_set_on_a_live_login_reaches_its_screen() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let (ws, other) = (seed_ws(&env, "metie"), seed_ws(&env, "metieother"));
    let svc = new_app(&ws, "metieapp");
    let (role, _) = guest_in(port, &ws, "metierole", &["thread:read"]).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": other.handle, "caps": ["thread:read"] })).await;
    let pass = app_guest("metieguest", &svc, &role);
    assert!(holds(&me(port, &pass).await, &other.id), "rig");
    let mut s = me_socket(port, &pass).await;
    skin::ties::set(&pid_of(&pass), skin::ties::TieWrite { workspace_id: &ws.id, set_by: "agent:metie", reason: "test" }).expect("tie");
    hears(&mut s, "a tie").await;
    let m = me(port, &pass).await;
    assert!(holds(&m, &ws.id) && !holds(&m, &other.id), "the tie holds it to the App's room: {m}");
}

// ── Nothing changed ──────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_changed_nothing_sent() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "menone");
    let (role, pass) = guest_in(port, &ws, "menonea", &["thread:read"]).await;
    let (other_role, other) = guest_in(port, &ws, "menoneb", &["thread:read"]).await;
    let mut s = me_socket(port, &pass).await;
    let mut control = me_socket(port, &other).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The same role again: a write, no change.
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": "menonea", "role": role })).await;
    silent(&mut s, 800, "the same role again").await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read"] })).await;
    silent(&mut s, 800, "the same caps again").await;

    // What isn't access.
    skin::set_principal_full_name("menonea", Some("A New Name")).expect("name");
    silent(&mut s, 800, "a new name").await;

    // Someone else's change, a second sign-in, a failed one.
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": other_role, "handle": ws.handle, "caps": ["thread:read", "files:read"] })).await;
    hears(&mut control, "the control hears its own change").await;
    let body = json!({ "username": "menonea", "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.expect("join");
    assert_eq!(r.status, 200, "{}", r.body);
    let body = json!({ "username": "menonea", "password": "wrong-horse-staple" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.expect("join");
    assert_eq!(r.status, 401, "{}", r.body);
    silent(&mut s, 800, "someone else's change, a sign-in and a failed one").await;

    // Ending this person's other session isn't this one's business.
    silent(&mut s, 200, "still open").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_device_hears() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "metwo");
    let (role, phone) = guest_in(port, &ws, "metwodevices", &["thread:read"]).await;
    let body = json!({ "username": "metwodevices", "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.expect("join");
    assert_eq!(r.status, 200, "{}", r.body);
    let laptop = r.json()["token"].as_str().expect("token").to_string();
    let (mut a, mut b, mut b2) = (me_socket(port, &phone).await, me_socket(port, &laptop).await, me_socket(port, &laptop).await);
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read", "files:read"] })).await;
    hears(&mut a, "the phone").await;
    hears(&mut b, "the laptop").await;
    hears(&mut b2, "the laptop's other tab").await;
}

/// Room sockets close in the walk; the event still reaches the socket that's left.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn losing_a_room_closes_its_socket_and_tells_me_events() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "meboth");
    let conv = pin_chat(&ws);
    let (_, pass) = guest_in(port, &ws, "mebothguest", ALL_CAPS).await;
    let mut s = me_socket(port, &pass).await;
    let mut o = open(port, &format!("/cli/overlay/events?conversation={conv}&token={}", enc(&pass))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    owner_ok(port, "/cli/skin/roles/unassign", json!({ "username": "mebothguest" })).await;
    let (a, b) = tokio::join!(hears(&mut s, "me.events"), closes(&mut o, 4013, "the overlay"));
    let _ = (a, b);
}

/// The hub wakes at a pass's expiry, however long the sweep is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expiring_pass_closes_when_it_expires() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "meexp");
    let (_, pass) = guest_in(port, &ws, "meexpguest", &["thread:read"]).await;
    let id = skin::resolve_skin_token(&pass).expect("live").id;
    let at = chrono::Utc::now().timestamp() + 3;
    // Another connection: the hook hears nothing, so only the expiry can do it.
    let other = rusqlite::Connection::open(env.path().join(".k2/skin.db")).expect("another connection");
    other.execute("UPDATE tokens SET expires_at = ?1 WHERE id = ?2", rusqlite::params![at, id]).expect("write");
    drop(other);
    let mut s = me_socket(port, &pass).await;
    let seen = watch(&mut s, 7_000).await;
    assert_eq!(seen.close, Some(4003), "{seen:?}");
}

// ── Races ────────────────────────────────────────────────────────────

/// A client that reads after a frame sees the write, every time (§9).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_after_the_frame_sees_the_write() {
    let loops: usize = std::env::var("K2_LIVE_ACCESS_LOOPS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "merace");
    let (role, pass) = guest_in(port, &ws, "meraceguest", &["thread:read"]).await;
    let mut s = me_socket(port, &pass).await;
    for i in 0..loops {
        let caps: &[&str] = if i % 2 == 0 { &["thread:read", "files:read"] } else { &["thread:read"] };
        owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
        match next_frame(&mut s, HEARS_WITHIN_MS).await {
            Ok(Some(f)) => assert_eq!(f, FRAME),
            other => panic!("loop {i}: {other:?}"),
        }
        assert_eq!(caps_in(&me(port, &pass).await, &ws.id), caps, "loop {i}");
    }
}

/// A write while the socket opens is never missed: either the read after the open
/// has it, or a frame comes (§9).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_during_the_open_is_never_missed() {
    let loops: usize = std::env::var("K2_LIVE_ACCESS_LOOPS").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "meopen");
    let (role, pass) = guest_in(port, &ws, "meopenguest", &["thread:read"]).await;
    let mut seen_frames = 0;
    for i in 0..loops {
        let caps: Vec<&str> = if i % 2 == 0 { vec!["thread:read", "files:read"] } else { vec!["thread:read"] };
        let (r, h, c) = (role.clone(), ws.handle.clone(), caps.clone());
        let writer = tokio::spawn(async move {
            // Land anywhere around the open.
            tokio::time::sleep(Duration::from_micros(((i * 700) % 6_000) as u64)).await;
            owner_ok(port, "/cli/skin/roles/room", json!({ "name": r, "handle": h, "caps": c })).await;
        });
        let mut s = me_socket(port, &pass).await;
        let read = me(port, &pass).await;
        writer.await.expect("writer");
        if caps_in(&read, &ws.id) != caps {
            match next_frame(&mut s, HEARS_WITHIN_MS).await {
                Ok(Some(f)) => {
                    assert_eq!(f, FRAME);
                    seen_frames += 1;
                }
                other => panic!("loop {i}: the read missed the write and no frame came: {other:?}"),
            }
            assert_eq!(caps_in(&me(port, &pass).await, &ws.id), caps, "loop {i}");
        }
    }
    eprintln!("[me.events] {loops} opens, {seen_frames} told by a frame");
}

// ── A link ───────────────────────────────────────────────────────────

/// The same socket over an App's link, with the task-local gone on the blocking
/// thread (V27). An App's own pass opens it; another App's guest doesn't; a
/// suspended App's link closes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn over_an_apps_link() {
    use k2_daemon::apps::gateway::{self, LinkSpec};
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "melink");
    let svc = new_app(&ws, "melinkapp");
    let other_svc = new_app(&ws, "melinkother");
    let (role, _) = guest_in(port, &ws, "melinkrole", &["thread:read"]).await;
    let guest = app_guest("melinkguest", &svc, &role);
    let stranger = app_guest("melinkstranger", &other_svc, &role);
    let spec = LinkSpec { app_id: "melinkapp".into(), service_id: svc.clone(), workspace_id: ws.id.clone(), port: 0, caps: vec!["thread:read".into()], rooms: vec![ws.id.clone()] };
    let own = gateway::open(spec, None).expect("open link");
    let link = gateway::port_of("melinkapp").expect("listening");

    let dial = |pass: String| async move {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!("ws://127.0.0.1:{link}/cli/skin/me/events?scope=me").into_client_request().expect("request");
        req.headers_mut().insert("Authorization", format!("Bearer {pass}").parse().expect("header"));
        tokio_tungstenite::connect_async(req).await
    };
    match dial(stranger).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status().as_u16(), 403, "another App's guest"),
        other => panic!("another App's guest: {:?}", other.map(|_| "opened")),
    }
    let (mut g, _) = dial(guest.clone()).await.expect("the App's guest opens it");
    let (mut a, _) = dial(own).await.expect("the App's own pass opens it");
    tokio::time::sleep(Duration::from_millis(300)).await;

    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read", "files:read"] })).await;
    hears(&mut g, "a guest on the link").await;
    silent(&mut a, 500, "the App's own pass: its access didn't change").await;

    let out = post(port, "/cli/app/suspend", OWNER, json!({ "app": "melinkapp", "reason": "test" })).await;
    assert_eq!(out.status, 200, "suspend: {}", out.body);
    let t0 = Instant::now();
    closes(&mut g, 4003, "a suspended App's guest on its link").await;
    closes(&mut a, 4003, "a suspended App's own pass").await;
    eprintln!("[me.events] suspend closed the link's sockets within {:?}", t0.elapsed());
    gateway::close("melinkapp", None, true);
}

// ── scope=guests: one socket per App ─────────────────────────────────

async fn named(s: &mut Sock, want: &str, what: &str) {
    match next_frame(s, HEARS_WITHIN_MS).await {
        Ok(Some(f)) => {
            let v: Value = serde_json::from_str(&f).expect("json");
            assert_eq!(v, json!({ "kind": "access_changed", "principalId": want }), "{what}: names that guest and nothing else");
        }
        other => panic!("{what}: {other:?}"),
    }
}

/// One socket for all of an App's guests. It only ever names that App's own guests,
/// and only its own pass opens it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_socket_for_all_of_an_apps_guests() {
    use k2_daemon::apps::gateway::{self, LinkSpec};
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "mefeed");
    let (svc_a, svc_b) = (new_app(&ws, "mefeeda"), new_app(&ws, "mefeedb"));
    let (role1, _) = guest_in(port, &ws, "mefeedr1", &["thread:read"]).await;
    let (role2, _) = guest_in(port, &ws, "mefeedr2", &["thread:read"]).await;
    let (role3, _) = guest_in(port, &ws, "mefeedr3", &["thread:read"]).await;
    let (role4, _) = guest_in(port, &ws, "mefeedr4", &["thread:read"]).await;
    let (ga, gb) = (app_guest("mefeedga", &svc_a, &role1), app_guest("mefeedgb", &svc_a, &role2));
    let gc = app_guest("mefeedgc", &svc_b, &role3);
    // A person on both Apps, on the App A role and a role only App B's grant uses.
    let both_a = app_guest("mefeedboth", &svc_a, &role1);
    let both_pid = pid_of(&both_a);
    skin::create_grant(&skin::SkinGrantWrite { subject_kind: "principal".into(), subject_id: both_pid.clone(), kind: "app".into(), target_id: svc_b.clone(), role: Some(role4.clone()), scope: None, created_by: Some("owner".into()) }).expect("grant on B");

    let spec = |app: &str, svc: &str| LinkSpec { app_id: app.into(), service_id: svc.to_string(), workspace_id: ws.id.clone(), port: 0, caps: vec!["thread:read".into()], rooms: vec![ws.id.clone()] };
    let own_a = gateway::open(spec("mefeeda", &svc_a), None).expect("link a");
    let own_b = gateway::open(spec("mefeedb", &svc_b), None).expect("link b");
    let (link_a, link_b) = (gateway::port_of("mefeeda").expect("a"), gateway::port_of("mefeedb").expect("b"));

    let dial = |link: u16, pass: String| async move {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!("ws://127.0.0.1:{link}/cli/skin/me/events?scope=guests").into_client_request().expect("request");
        req.headers_mut().insert("Authorization", format!("Bearer {pass}").parse().expect("header"));
        tokio_tungstenite::connect_async(req).await
    };
    // Only the App's own pass.
    match dial(link_a, ga.clone()).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status().as_u16(), 403, "a guest's pass"),
        other => panic!("a guest's pass: {:?}", other.map(|_| "opened")),
    }
    match dial(link_a, own_b.clone()).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status().as_u16(), 403, "another App's pass"),
        other => panic!("another App's pass: {:?}", other.map(|_| "opened")),
    }
    match tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/cli/skin/me/events?scope=guests&token={}", enc(&ga))).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status().as_u16(), 403, "a guest's pass, main port"),
        other => panic!("a guest's pass, main port: {:?}", other.map(|_| "opened")),
    }
    owner_ok(port, "/cli/skin/users", json!({ "username": "mefeedplat" })).await;
    let plat = owner_ok(port, "/cli/skin-tokens", json!({ "name": "mefeedplat", "caps": ["thread:read"], "rooms": [ws.handle] })).await;
    match tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/cli/skin/me/events?scope=guests&token={}", enc(plat["token"].as_str().expect("token")))).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status().as_u16(), 403, "a platform pass"),
        other => panic!("a platform pass: {:?}", other.map(|_| "opened")),
    }

    let (mut a, _) = dial(link_a, own_a.clone()).await.expect("App A's feed");
    let (mut b, _) = dial(link_b, own_b.clone()).await.expect("App B's feed");
    // A bound of its own, and another App's feed is outside it.
    let mut more = Vec::new();
    for _ in 0..15 {
        more.push(dial(link_b, own_b.clone()).await.expect("up to 16 feeds"));
    }
    match dial(link_b, own_b.clone()).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status().as_u16(), 429, "the 17th"),
        other => panic!("the 17th feed: {:?}", other.map(|_| "opened")),
    }
    drop(more);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (pa, pb, pc) = (pid_of(&ga), pid_of(&gb), pid_of(&gc));

    // A's guest changes: A hears that guest. B hears nothing.
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role2, "handle": ws.handle, "caps": ["thread:read", "files:read"] })).await;
    named(&mut a, &pb, "App A's guest").await;
    silent(&mut b, 800, "App B: not its guest").await;
    silent(&mut a, 300, "once").await;

    // B's guest changes: B hears; A learns nothing about a guest of another App.
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role3, "handle": ws.handle, "caps": ["thread:read", "files:read"] })).await;
    named(&mut b, &pc, "App B's guest").await;
    silent(&mut a, 800, "App A: not its guest").await;

    // A person on both: what changed under B's grant is B's to hear.
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role4, "handle": ws.handle, "caps": ["thread:read", "files:read"] })).await;
    named(&mut b, &both_pid, "the person on both, under B's grant").await;
    silent(&mut a, 800, "App A: its own grant for them didn't change").await;

    // A no-op.
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role2, "handle": ws.handle, "caps": ["thread:read", "files:read"] })).await;
    silent(&mut a, 800, "the same caps again").await;

    // A session ended: the App holding that pass has to hear.
    let id = skin::resolve_skin_token(&ga).expect("live").id;
    assert!(skin::revoke_token(&id).expect("revoke"));
    named(&mut a, &pa, "a session ended").await;
    silent(&mut b, 500, "not B's").await;
    assert_eq!(get(port, "/cli/skin/me", &ga, "x=1").await.status, 401, "me.get then says so");

    // A session ended beside an unrelated write in the same moment, and a new sign-in
    // (no news) right after: the ended one is still named, once.
    let second = skin::create_app_session_token("mefeedgb", &svc_a).expect("App session").raw;
    let second_id = skin::resolve_skin_token(&second).expect("live").id;
    silent(&mut a, 600, "a new session is not news").await;
    assert!(skin::revoke_token(&second_id).expect("revoke"));
    skin::set_principal_full_name("mefeedplatnot", None).expect_err("an unrelated write that fails");
    skin::set_principal_full_name("mefeedr1", Some("Someone")).expect("an unrelated write");
    named(&mut a, &pb, "a session ended beside another write").await;
    silent(&mut a, 600, "once").await;

    // A grant switched off: gone from the App's list, and named once more.
    let g = skin::list_grants().expect("grants").into_iter().find(|g| g.target_id == svc_a && g.subject_id == pb).expect("grant");
    skin::set_grant_enabled(&g.id, false).expect("disable");
    named(&mut a, &pb, "a guest switched off").await;

    // A new guest of this App.
    let gnew = app_guest("mefeednew", &svc_a, &role1);
    named(&mut a, &pid_of(&gnew), "a new guest").await;

    // The App's pass goes: 4003.
    let out = post(port, "/cli/app/suspend", OWNER, json!({ "app": "mefeeda", "reason": "test" })).await;
    assert_eq!(out.status, 200, "suspend: {}", out.body);
    closes(&mut a, 4003, "a suspended App's feed").await;
    gateway::close("mefeeda", None, true);
    gateway::close("mefeedb", None, true);
}

// ── Toward Sew ───────────────────────────────────────────────────────

/// Sew refuses a step (a `blocked` row). K2's own answer doesn't wait for it: the
/// frame fires, `me.get` is right both ways and K2's sockets carry on. Sew's answer
/// is no news on K2's socket, and the owner's grant hooks list shows the blocked row
/// (PRD §9, §14). How `k2 app chat` shows a Space's blocked step is
/// `sew::governed`'s own test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blocked_step_toward_sew_doesnt_hold_up_k2s_own_answer() {
    use k2_core::tool_hook as hook;
    use k2_daemon::tool_hook_worker::{self as worker, WorkerConfig};
    let env = setup(&[]);
    hook::allow_target_off_linux_for_tests(true);
    let sew = fake_sew::FakeSew::start(&env.path().join("s")).await;
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "meblocked");
    let conv = pin_chat(&ws);
    hook::set_target(skin::CHAT_GRANT_KIND, true, Some(sew.socket.to_str().expect("path")), Some(sew.token_file.to_str().expect("path")), "owner-token").expect("enable chat");
    let cfg = WorkerConfig {
        tick: Duration::from_millis(50),
        backoff_base: Duration::from_millis(20),
        backoff_cap: Duration::from_millis(200),
        reconcile_every: Duration::from_secs(3600),
        reconcile_debounce: Duration::from_millis(10),
        dep_wait: Duration::from_secs(3600),
    };
    async fn drain(cfg: &WorkerConfig) {
        for _ in 0..40 {
            hook::force_due_for_tests().expect("due");
            worker::pass_once(cfg).await.expect("pass");
            if hook::log(500, None).expect("log").iter().all(|r| r.state != "pending") {
                return;
            }
        }
        panic!("the outbox never drained: {:?}", hook::log(500, None).expect("log"));
    }
    drain(&cfg).await;

    let (_, pass) = guest_in(port, &ws, "meblockedguest", &["thread:read"]).await;
    let pid = pid_of(&pass);
    let mut s = me_socket(port, &pass).await;
    let mut o = open(port, &format!("/cli/overlay/events?conversation={conv}&token={}", enc(&pass))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Chat given, and Sew refuses it.
    sew.fail_next(1, 400, "validation_failed", "");
    let chat = skin::create_grant(&skin::SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: pid.clone(),
        kind: skin::CHAT_GRANT_KIND.into(),
        target_id: skin::CHAT_GRANT_TARGET.into(),
        role: None,
        scope: None,
        created_by: Some("owner".into()),
    })
    .expect("chat grant");
    hears(&mut s, "chat given, before Sew answered").await;
    assert_eq!(me(port, &pass).await["chat"], true);
    drain(&cfg).await;
    let subject = format!("k2:principal:{pid}");
    let rows = hook::log(500, None).expect("log");
    let row = rows.iter().find(|r| r.subject_key == subject).unwrap_or_else(|| panic!("no row for the guest: {rows:?}"));
    assert_eq!(row.state, "blocked", "rig: Sew refused it: {row:?}");
    assert!(!hook::status().expect("status")["blocked"].as_array().expect("blocked").is_empty(), "the owner's list shows it");

    // Sew's answer is no news here, and K2's own sockets are untouched.
    silent(&mut s, 800, "a blocked row isn't an access change").await;
    assert_eq!(me(port, &pass).await["chat"], true, "K2's answer is K2's, whatever Sew said");
    say(port, &ws, "BLOCKED-THREAD").await;
    let seen = watch(&mut o, 1_500).await;
    assert!(seen.has("BLOCKED-THREAD") && !seen.ended, "{seen:?}");

    // Chat taken while Sew's side is stuck: K2's answer and frame still follow.
    assert!(skin::delete_grant_by_id(&chat.id).expect("delete"));
    hears(&mut s, "chat taken, with Sew blocked").await;
    assert_eq!(me(port, &pass).await["chat"], false);
}
