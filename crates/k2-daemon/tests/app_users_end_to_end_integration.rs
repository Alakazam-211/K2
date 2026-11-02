//! An agent manages its App's users end to end (Rosson 2026-11-02, 0.46.8),
//! headless: the real dispatcher in process on a temp `$HOME`, over loopback.
//!
//! - A login an agent makes works at once in the App's own room and the rooms
//!   the owner approved for that App; a room past those is refused, naming
//!   the broad path. The tie stays and still does its other work.
//! - Nothing but an owner reset on K2's own page unties.
//! - New access reaches a signed-in person at the next request.
//! - An App's own agent reads and ends its own guests' sessions.
//! - Logout on a link ends the one session presented, and nothing else.
//!
//! Never the real `~/.k2`, never the keychain, never a real mail send.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::sync::{Arc, Mutex};

use app_fixture::*;
use k2_core::sew_revoke::{self, SewSessionRevoke};
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::{json, Value};

const PW: &str = "s3cret-horse";
const TEMP: &str = "set-by-someone-1";

async fn owner_post(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

fn tag() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

fn app_row(ws: &Ws, name: &str) -> String {
    let dir = ws.path.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::insert(&conn, &ws.id, name, "node s.mjs", &dir.to_string_lossy(), 47_521, "local", "stopped", "cmd", "")
        .expect("insert service")
        .id
}

fn principal_id(username: &str) -> String {
    k2_core::skin::list_principals().unwrap().into_iter().find(|p| p.username == username).unwrap_or_else(|| panic!("no {username}")).id
}

fn tie_of(username: &str) -> Option<k2_core::skin::ties::Tie> {
    k2_core::skin::ties::get(&principal_id(username)).expect("tie lookup")
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn rooms_of(token: &str) -> Vec<String> {
    sorted(k2_core::skin::resolve_skin_token(token).expect("a live pass").room_policy.into_keys().collect())
}

async fn login(port: u16, body: Value) -> Resp {
    let b = body.to_string();
    tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(b))).await.unwrap()
}

async fn user_row(port: u16, username: &str) -> Value {
    let r = get(port, "/cli/skin/users", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["users"].as_array().expect("users").iter().find(|u| u["username"] == username).cloned().unwrap_or_else(|| panic!("no {username}"))
}

/// A password someone else set: the person chooses their own at first sign-in.
async fn person_chooses(port: u16, username: &str, temp: &str, app: &str, new_pw: &str) {
    let r = login(port, json!({ "username": username, "password": temp, "app": app })).await;
    assert_eq!((r.status, r.code()), (403, "password_change_required".to_string()), "{}", r.body);
    let tok = r.json()["changeToken"].as_str().expect("a change token").to_string();
    let b = json!({ "token": tok, "password": new_pw }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/password/reset", Some(b))).await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
}

async fn session(port: u16, username: &str, app: &str) -> String {
    let r = login(port, json!({ "username": username, "password": PW, "app": app })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

/// A Bearer call, the way an App's server (or a native app) makes one.
fn bearer_blocking(port: u16, method: &str, pq: &str, pass: &str, body: Option<&str>) -> Resp {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let b = body.unwrap_or("");
    let req = format!(
        "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nAuthorization: Bearer {pass}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
        b.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).expect("status");
    Resp { status, body: text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default() }
}

async fn bearer(port: u16, method: &'static str, pq: &str, pass: &str, body: Option<Value>) -> Resp {
    let (pq, pass, body) = (pq.to_string(), pass.to_string(), body.map(|b| b.to_string()));
    tokio::task::spawn_blocking(move || bearer_blocking(port, method, &pq, &pass, body.as_deref())).await.unwrap()
}

struct Box2 {
    sales: Ws,
    ops: Ws,
    desk: Ws,
    shop: String,
    shop_name: String,
    agent: String,
}

/// `sales` runs the App `shop`, with the App-user switch on. `ops` and `desk`
/// are other workspaces on the box.
fn box2(env: &Env, t: &str) -> Box2 {
    let sales = seed_ws(env, &format!("sales{t}"));
    let ops = seed_ws(env, &format!("ops{t}"));
    let desk = seed_ws(env, &format!("desk{t}"));
    let shop_name = format!("shop{t}");
    let shop = app_row(&sales, &shop_name);
    allow_manage(&sales, true);
    let agent = passport(&sales);
    Box2 { sales, ops, desk, shop, shop_name, agent }
}

fn line(ws: &Ws) -> Value {
    json!({ "handle": ws.handle, "caps": ["thread:read", "thread:post"] })
}

/// The first path of an agent's App users: a role inside the App's rooms.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_login_inside_the_apps_rooms_works_at_once_and_outside_is_refused() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let b = box2(&env, &t);
    let (shop, agent) = (b.shop.clone(), b.agent.clone());

    // The owner's role: the App's own room and ops, for this App.
    let wide = format!("wide{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": wide, "app": shop, "roomAccess": [line(&b.sales), line(&b.ops)] })).await;

    // No grant yet: ops is outside, the refusal names the card, nobody made.
    let u0 = format!("u0{t}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u0, "app": shop, "role": wide })).await;
    assert_eq!((r.status, r.code()), (403, "outside_app_rooms".to_string()), "{}", r.body);
    let hint = r.json()["error"]["hint"].as_str().expect("hint").to_string();
    assert!(
        hint.contains(&format!("k2 app grant {} --kind guests-rooms --room {}", b.shop_name, b.ops.handle)),
        "names the broad path, for this App and this room: {hint}"
    );
    assert_eq!(r.json()["error"]["rooms"], json!([b.ops.handle]), "{}", r.body);
    assert!(!k2_core::skin::list_principals().unwrap().iter().any(|p| p.username == u0), "nobody was made");

    // The owner approves ops for this App.
    owner_post(port, "/cli/app/grant", json!({ "app": shop, "kind": "guests-rooms", "rooms": [b.ops.handle] })).await;

    // The agent makes the login with a temporary password: no owner step.
    let u1 = format!("u1{t}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u1, "app": shop, "role": wide, "generate": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let temp = r.json()["generatedPassword"].as_str().expect("shown once").to_string();
    assert_eq!(r.json()["passwordMustChange"], json!(true), "{}", r.body);
    person_chooses(port, &u1, &temp, &shop, PW).await;

    // Fully usable, the role's rooms and caps, nothing waiting.
    let s1 = session(port, &u1, &shop).await;
    let both = sorted(vec![b.sales.id.clone(), b.ops.id.clone()]);
    assert_eq!(rooms_of(&s1), both, "both rooms at once");
    let pass = k2_core::skin::resolve_skin_token(&s1).unwrap();
    assert!(pass.has_cap_in_room(&b.ops.id, "thread:post"), "{pass:?}");
    let row = user_row(port, &u1).await;
    assert_eq!(row["pendingReset"], json!(false), "{row}");
    let r = bearer(port, "GET", "/cli/skin/agents", &s1, None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains(&b.ops.id), "the App's person reaches ops: {}", r.body);

    // The tie stays, and keeps its other work: an agent could know this
    // password, so never a chat pass, and never the box login.
    assert!(tie_of(&u1).is_some(), "still tied");
    k2_core::skin::insert_chat_grant_for_tests(&principal_id(&u1)).expect("chat grant");
    let r = bearer(port, "POST", "/cli/sew/pass", &s1, Some(json!({}))).await;
    assert_eq!((r.status, r.code()), (403, "tied_pending_reset".to_string()), "{}", r.body);
    owner_post(port, "/cli/skin/grants/delete", json!({ "subjectKind": "principal", "subjectId": principal_id(&u1), "kind": "chat", "targetId": "sew" })).await;
    let r = login(port, json!({ "username": u1, "password": PW })).await;
    assert_eq!(r.status, 401, "no box login: {}", r.body);

    // A role past the grant, the agent's own rooms route too.
    let wider = format!("wider{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": wider, "roomAccess": [line(&b.sales), line(&b.desk)] })).await;
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": format!("u2{t}"), "app": shop, "role": wider })).await;
    assert_eq!((r.status, r.code()), (403, "outside_app_rooms".to_string()), "{}", r.body);
    let u3 = format!("u3{t}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u3, "app": shop })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/skin/users/rooms", &agent, json!({ "username": u3, "app": shop, "rooms": [b.sales.handle, b.ops.handle] })).await;
    assert_eq!(r.status, 200, "inside the App's rooms: {}", r.body);
    let r = post(port, "/cli/skin/users/rooms", &agent, json!({ "username": u3, "app": shop, "rooms": [b.desk.handle] })).await;
    assert_eq!((r.status, r.code()), (403, "outside_app_rooms".to_string()), "{}", r.body);
    assert!(r.json()["error"]["hint"].as_str().unwrap().contains(&format!("--room {}", b.desk.handle)), "{}", r.body);

    // The agent still manages such a login (it isn't "outside its workspace").
    let r = post(port, "/cli/skin/users/email", &agent, json!({ "username": u1, "email": format!("{u1}@example.test") })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/skin/users/unlock", &agent, json!({ "username": u1 })).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // Its own App's role: a granted room, Thread caps only.
    let mine = format!("mine{t}");
    let r = post(port, "/cli/skin/roles", &agent, json!({ "name": mine, "app": shop, "roomAccess": [line(&b.sales), line(&b.ops)] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/skin/roles/room", &agent, json!({ "name": mine, "handle": b.ops.handle, "caps": ["thread:read", "files:write"] })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "more than Thread in another workspace's room: {}", r.body);
    assert!(r.json()["error"]["hint"].as_str().unwrap().contains("files:write"), "{}", r.body);
    let r = post(port, "/cli/skin/roles/room", &agent, json!({ "name": mine, "handle": b.desk.handle })).await;
    assert_eq!((r.status, r.code()), (403, "outside_app_rooms".to_string()), "{}", r.body);
    // A role for no App stays its own room only.
    let r = post(port, "/cli/skin/roles", &agent, json!({ "name": format!("box{t}"), "roomAccess": [line(&b.ops)] })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);

    // Taking the grant back takes the rooms from these logins at once.
    owner_post(port, "/cli/app/grant/revoke", json!({ "app": shop, "kind": "guests-rooms" })).await;
    assert_eq!(rooms_of(&s1), vec![b.sales.id.clone()], "ops is gone at the next request");
    assert_eq!(user_row(port, &u1).await["pendingReset"], json!(true));
}

/// A tied login the owner gave more, before the grant: it clears by itself,
/// with the same password and the same session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_existing_tied_login_clears_when_its_rooms_fall_inside_the_grant() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let b = box2(&env, &t);
    let (shop, agent) = (b.shop.clone(), b.agent.clone());

    let u = format!("u{t}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u, "app": shop, "password": TEMP })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    person_chooses(port, &u, TEMP, &shop, PW).await;
    let s = session(port, &u, &shop).await;

    // The owner widens it past the ceiling: it waits, and says so.
    owner_post(port, "/cli/skin/users/rooms", json!({ "username": u, "app": shop, "rooms": [b.sales.handle, b.ops.handle], "applyTokens": true })).await;
    assert_eq!(rooms_of(&s), vec![b.sales.id.clone()], "held back");
    assert_eq!(user_row(port, &u).await["pendingReset"], json!(true));

    // skin_room names the tie and what clears it, for this person's own room;
    // the untied control and the hint's bytes stay as they were.
    let r = bearer(port, "GET", &format!("/cli/thread?addr={}", b.ops.handle), &s, None).await;
    assert_eq!((r.status, r.code()), (403, "skin_room".to_string()), "{}", r.body);
    let hint = r.json()["error"]["hint"].as_str().expect("hint").to_string();
    assert!(hint.contains("Waiting for reset (agent-tied)"), "{hint}");
    assert!(hint.contains(&b.ops.handle), "{hint}");
    assert!(hint.contains(&format!("k2 app grant {} --kind guests-rooms --room {}", b.shop_name, b.ops.handle)), "{hint}");
    assert!(hint.contains(&format!("k2 app user reset {u}")), "{hint}");
    // No oracle: a room this person's grant doesn't name gets the same words.
    let r2 = bearer(port, "GET", "/cli/thread?addr=no-such-room", &s, None).await;
    assert_eq!((r2.status, r2.code()), (403, "skin_room".to_string()), "{}", r2.body);
    assert_eq!(r2.body, r.body, "the answer never says which rooms exist");

    // Approved for the App: the same pass, at its next request.
    let pid = principal_id(&u);
    let hash_before: String = rusqlite::Connection::open(k2_core::paths::k2_home().join("skin.db"))
        .unwrap()
        .query_row("SELECT password_hash FROM principals WHERE id = ?1", [&pid], |r| r.get(0))
        .unwrap();
    owner_post(port, "/cli/app/grant", json!({ "app": shop, "kind": "guests-rooms", "rooms": [b.ops.handle] })).await;
    assert_eq!(rooms_of(&s), sorted(vec![b.sales.id.clone(), b.ops.id.clone()]), "no sign-out, no reset");
    assert_eq!(user_row(port, &u).await["pendingReset"], json!(false));
    let hash_after: String = rusqlite::Connection::open(k2_core::paths::k2_home().join("skin.db"))
        .unwrap()
        .query_row("SELECT password_hash FROM principals WHERE id = ?1", [&pid], |r| r.get(0))
        .unwrap();
    assert_eq!(hash_before, hash_after, "the password didn't change");
    assert!(tie_of(&u).is_some());

    // An untied login: skin_room's old answer, byte for byte.
    let c = format!("c{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": c, "app": shop, "password": TEMP })).await;
    person_chooses(port, &c, TEMP, &shop, PW).await;
    let sc = session(port, &c, &shop).await;
    let r = bearer(port, "GET", &format!("/cli/thread?addr={}", b.desk.handle), &sc, None).await;
    assert_eq!((r.status, r.body.as_str()), (403, k2_daemon::skin_routes::SKIN_ROOM_JSON), "{}", r.body);
}

/// Only an owner reset on K2's own page unties: not the must-change screen,
/// not an agent's password write, not anything an agent can call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_cant_untie_a_login_any_other_way() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let b = box2(&env, &t);
    let (shop, agent) = (b.shop.clone(), b.agent.clone());
    let u = format!("u{t}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u, "app": shop, "password": TEMP })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(tie_of(&u).is_some());

    // The person choosing their own, with the App's change token: an App's
    // server can host that screen, so it proves nothing.
    person_chooses(port, &u, TEMP, &shop, PW).await;
    assert!(tie_of(&u).is_some(), "a must-change reset doesn't untie");
    // The agent setting it again, and asking for a reset link.
    let r = post(port, "/cli/skin/users/password", &agent, json!({ "username": u, "password": "another-pass-22" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(tie_of(&u).is_some());
    let r = post(port, "/cli/skin/users/reset-link", &agent, json!({ "username": u, "app": shop, "code": true })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert!(tie_of(&u).is_some());
    // The agent's other doors: a role it can't lift the tie through.
    assert!(k2_core::skin::creators::clear_agent_tie(&principal_id(&u), &format!("agent:{}", b.sales.handle)).is_err());
    assert!(tie_of(&u).is_some());
    // The owner's reset link, used: the one thing that does.
    owner_post(port, "/cli/skin/front-door", json!({ "mode": "direct", "url": format!("https://door{t}.example.test"), "apply": false })).await;
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": u, "app": shop, "code": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let tok = r.json()["link"].as_str().expect("the link").split('#').nth(1).expect("the token").to_string();
    let body = json!({ "token": tok, "password": "chosen-by-them-33" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/password/reset", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(tie_of(&u).is_none(), "the owner's reset unties");
}

/// New access at the next request, box logins and App logins alike, with no
/// sign-out and no --apply-tokens. Taking away still works the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn more_access_reaches_a_signed_in_person_at_the_next_request() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let b = box2(&env, &t);
    let shop = b.shop.clone();

    // A box login, Thread-only in sales; no --apply-tokens on the change.
    let bx = format!("bx{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": bx })).await;
    k2_daemon::test_harness::guest_with_own_password(&bx, PW);
    owner_post(port, "/cli/skin/users/rooms", json!({ "username": bx, "rooms": [b.sales.handle] })).await;
    let r = login(port, json!({ "username": bx, "password": PW })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let sb = r.json()["token"].as_str().expect("token").to_string();
    assert_eq!(rooms_of(&sb), vec![b.sales.id.clone()]);
    owner_post(port, "/cli/skin/users/rooms", json!({ "username": bx, "rooms": [b.sales.handle, b.ops.handle] })).await;
    assert_eq!(rooms_of(&sb), sorted(vec![b.sales.id.clone(), b.ops.id.clone()]), "a box login's new room, at once");
    // A role on the box login: its caps too.
    let role = format!("r{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": role, "roomAccess": [{ "handle": b.ops.handle, "caps": ["thread:read"] }] })).await;
    owner_post(port, "/cli/skin/roles/assign", json!({ "username": bx, "role": role })).await;
    assert_eq!(rooms_of(&sb), vec![b.ops.id.clone()], "the role decides now");
    owner_post(port, "/cli/skin/roles/room", json!({ "name": role, "handle": b.ops.handle, "caps": ["thread:read", "files:read"] })).await;
    assert!(k2_core::skin::resolve_skin_token(&sb).unwrap().has_cap_in_room(&b.ops.id, "files:read"), "a role's new cap, at once");
    owner_post(port, "/cli/skin/roles/room", json!({ "name": role, "handle": b.ops.handle, "caps": ["thread:read"] })).await;
    assert!(!k2_core::skin::resolve_skin_token(&sb).unwrap().has_cap_in_room(&b.ops.id, "files:read"), "and it goes at once");

    // An owner's App login: rooms, then a role in place, same pass.
    let u = format!("u{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": u, "app": shop, "password": TEMP })).await;
    person_chooses(port, &u, TEMP, &shop, PW).await;
    let s = session(port, &u, &shop).await;
    assert_eq!(rooms_of(&s), Vec::<String>::new());
    owner_post(port, "/cli/skin/users/rooms", json!({ "username": u, "app": shop, "rooms": [b.ops.handle] })).await;
    assert_eq!(rooms_of(&s), vec![b.ops.id.clone()], "an App login's new room, no --apply-tokens");
    let r2 = format!("r2{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": r2, "app": shop, "roomAccess": [line(&b.desk)] })).await;
    let grant = k2_core::skin::list_grants().unwrap().into_iter().find(|g| g.subject_id == principal_id(&u) && g.target_id == shop).expect("the login");
    owner_post(port, "/cli/skin/grants/role", json!({ "id": grant.id, "role": r2 })).await;
    let pass = k2_core::skin::resolve_skin_token(&s).expect("still signed in");
    assert_eq!(sorted(pass.room_policy.keys().cloned().collect()), vec![b.desk.id.clone()]);
    assert!(pass.has_cap_in_room(&b.desk.id, "thread:post"), "the role's caps: {pass:?}");
    // Grant off: no access at once. On again: the same pass, the role's rooms.
    owner_post(port, "/cli/skin/grants/enabled", json!({ "id": grant.id, "enabled": false })).await;
    assert!(k2_core::skin::resolve_skin_token(&s).is_none(), "off: no access");
}

/// An App's own agent sees and ends its own guests' sessions. It only
/// tightens, so the App-user switch is enough; never a box login, never
/// another App's person, and the person's Sew session ends with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_apps_agent_ends_its_own_guests_sessions() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let b = box2(&env, &t);
    let (shop, agent) = (b.shop.clone(), b.agent.clone());
    let their_app = app_row(&b.ops, &format!("crm{t}"));

    let seen: Arc<Mutex<Vec<SewSessionRevoke>>> = Arc::default();
    let sink = seen.clone();
    sew_revoke::set_sink(Arc::new(move |_c: &rusqlite::Connection, op: &SewSessionRevoke| {
        sink.lock().unwrap().push(op.clone());
        Ok(())
    }));

    let u = format!("u{t}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u, "app": shop, "password": TEMP })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    person_chooses(port, &u, TEMP, &shop, PW).await;
    k2_core::skin::insert_chat_grant_for_tests(&principal_id(&u)).expect("chat grant");
    let (s1, s2) = (session(port, &u, &shop).await, session(port, &u, &shop).await);

    let q = format!("app={}&user={}", enc(&shop), enc(&u));
    let r = get(port, "/cli/skin/users/sessions", &agent, &q).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let rows = r.json()["sessions"].as_array().expect("sessions").clone();
    assert_eq!(rows.len(), 2, "{}", r.body);
    assert!(!r.body.contains(&s1) && !r.body.contains(&s2), "never a pass: {}", r.body);

    // One by id: only that one, and its Sew session.
    let id = rows[0]["id"].as_str().expect("id").to_string();
    let r = post(port, "/cli/skin/users/sessions/revoke", &agent, json!({ "app": shop, "user": u, "id": id })).await;
    assert_eq!((r.status, r.json()["ended"].clone()), (200, json!(1)), "{}", r.body);
    let live = [&s1, &s2].iter().filter(|s| k2_core::skin::resolve_skin_token(s).is_some()).count();
    assert_eq!(live, 1, "one ended, one stays");
    assert_eq!(seen.lock().unwrap().iter().filter(|o| matches!(o, SewSessionRevoke::K2Sid(_))).count(), 1, "the Sew session ends with it");
    // All of them.
    let r = post(port, "/cli/skin/users/sessions/revoke", &agent, json!({ "app": shop, "user": u, "all": true })).await;
    assert_eq!((r.status, r.json()["ended"].clone()), (200, json!(1)), "{}", r.body);
    assert!(k2_core::skin::resolve_skin_token(&s1).is_none() && k2_core::skin::resolve_skin_token(&s2).is_none());
    assert_eq!(seen.lock().unwrap().iter().filter(|o| matches!(o, SewSessionRevoke::K2Sid(_))).count(), 2);
    sew_revoke::clear_sink();

    // A GET on the revoke route is 405.
    let r = get(port, "/cli/skin/users/sessions/revoke", &agent, "").await;
    assert_eq!(r.status, 405, "{}", r.body);

    // Another workspace's App, and someone not on this App.
    let q2 = format!("app={}&user={}", enc(&their_app), enc(&u));
    let r = get(port, "/cli/skin/users/sessions", &agent, &q2).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    let other = format!("o{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": other, "app": their_app, "password": TEMP })).await;
    let q3 = format!("app={}&user={}", enc(&shop), enc(&other));
    let r = get(port, "/cli/skin/users/sessions", &agent, &q3).await;
    assert_eq!((r.status, r.code()), (404, "not_found".to_string()), "{}", r.body);
    let nobody = format!("app={}&user=nobody{t}", enc(&shop));
    let r2 = get(port, "/cli/skin/users/sessions", &agent, &nobody).await;
    assert_eq!(r2.status, 404, "the same answer as a person who isn't on this App: {}", r2.body);

    // A box login on its App: refused, naming the card.
    let bx = format!("bx{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": bx })).await;
    owner_post(port, "/cli/skin/users/rooms", json!({ "username": bx, "app": shop, "rooms": [b.sales.handle] })).await;
    let q4 = format!("app={}&user={}", enc(&shop), enc(&bx));
    let r = get(port, "/cli/skin/users/sessions", &agent, &q4).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    assert!(r.body.contains("apps.user-grant-box"), "{}", r.body);

    // The switch off: its own refusal.
    allow_manage(&b.sales, false);
    let q5 = format!("app={}&user={}", enc(&shop), enc(&u));
    let r = get(port, "/cli/skin/users/sessions", &agent, &q5).await;
    assert_eq!((r.status, r.code()), (403, "app_users_off".to_string()), "{}", r.body);
    allow_manage(&b.sales, true);

    // The owner still can.
    let r = get(port, "/cli/skin/users/sessions", OWNER, &q5).await;
    assert_eq!(r.status, 200, "{}", r.body);
}

fn link_spec(app_id: &str, svc: &str, ws: &Ws) -> LinkSpec {
    LinkSpec {
        app_id: app_id.to_string(),
        service_id: svc.to_string(),
        workspace_id: ws.id.clone(),
        port: 0,
        caps: vec!["thread:read".into()],
        rooms: vec![ws.id.clone()],
    }
}

/// Logout on a link takes any live session the caller holds and ends only
/// that one. Never another session, never a non-session pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logout_on_a_link_ends_the_session_presented_and_nothing_else() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let t = tag();
    let b = box2(&env, &t);
    let (shop, port) = (b.shop.clone(), d.port);
    let crm = app_row(&b.sales, &format!("crm{t}"));
    let (shop_id, crm_id) = (format!("shopa{t}"), format!("crma{t}"));
    let shop_pass = gateway::open(link_spec(&shop_id, &shop, &b.sales), None).expect("shop link");
    let crm_pass = gateway::open(link_spec(&crm_id, &crm, &b.sales), None).expect("crm link");
    let lport = gateway::port_of(&shop_id).expect("listening");

    // One person with sessions on both Apps.
    let u = format!("u{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": u, "app": shop, "password": TEMP })).await;
    person_chooses(port, &u, TEMP, &shop, PW).await;
    owner_post(port, "/cli/skin/users/rooms", json!({ "username": u, "app": crm, "rooms": [b.sales.handle] })).await;
    let (on_shop, on_crm_a, on_crm_b) = (session(port, &u, &shop).await, session(port, &u, &crm).await, session(port, &u, &crm).await);

    // Another App's session on this link: holding it is the proof, and only
    // that one ends. Every other route still refuses it.
    let r = bearer(lport, "GET", "/gw/me", &on_crm_a, None).await;
    assert_eq!((r.status, r.code()), (403, "pass_wrong_link".to_string()), "{}", r.body);
    let r = bearer(lport, "POST", "/cli/skin/logout", &on_crm_a, Some(json!({}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(k2_core::skin::resolve_skin_token(&on_crm_a).is_none(), "the one presented ended");
    assert!(k2_core::skin::resolve_skin_token(&on_crm_b).is_some(), "the same person's other session stays");
    assert!(k2_core::skin::resolve_skin_token(&on_shop).is_some(), "their session on this App stays");

    // A session of this App: as ever.
    let r = bearer(lport, "POST", "/cli/skin/logout", &on_shop, Some(json!({}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(k2_core::skin::resolve_skin_token(&on_shop).is_none());

    // Never a non-session pass: this link's own (401, stays live), another
    // App's link pass and a platform pass (403), and nothing else ends.
    let live = |p: &str| !matches!(k2_core::skin::link::classify_presented(p), k2_core::skin::link::PresentedPass::Unknown);
    let r = bearer(lport, "POST", "/cli/skin/logout", &shop_pass, Some(json!({}))).await;
    assert_eq!(r.status, 401, "{}", r.body);
    assert!(live(&shop_pass), "the App's own pass is no session");
    let r = bearer(lport, "POST", "/cli/skin/logout", &crm_pass, Some(json!({}))).await;
    assert_eq!((r.status, r.code()), (403, "pass_wrong_link".to_string()), "{}", r.body);
    assert!(live(&crm_pass));
    let (_, platform) = k2_core::skin::create_token(&format!("p{t}"), Some(&["thread:read".to_string()]), &[b.sales.id.clone()]).unwrap();
    let r = bearer(lport, "POST", "/cli/skin/logout", &platform, Some(json!({}))).await;
    assert_eq!((r.status, r.code()), (403, "pass_wrong_link".to_string()), "{}", r.body);
    assert!(k2_core::skin::resolve_skin_token(&platform).is_some(), "a platform pass stays");
    // A session that already ended, and nonsense: 401, as every bad pass is.
    let r = bearer(lport, "POST", "/cli/skin/logout", &on_crm_a, Some(json!({}))).await;
    assert_eq!((r.status, r.code()), (401, "pass_invalid".to_string()), "{}", r.body);
    assert!(k2_core::skin::resolve_skin_token(&on_crm_b).is_some(), "still there");

    gateway::close(&shop_id, None, true);
    gateway::close(&crm_id, None, true);
}
