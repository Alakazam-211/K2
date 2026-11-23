//! An App's own agent adds a room to a role of its App (`k2 app role room`),
//! headless: the real dispatcher in process on a temp `$HOME`, over loopback.
//!
//! - "Let this agent manage its Apps' users" on: any role of its own App, the
//!   owner's too, takes any room on this box at once, audited.
//! - On, but a box-wide role or another workspace's App role: refused, nothing
//!   held, and the refusal names what to do instead.
//! - Off: one held request with a card for the owner on the agent's Thread
//!   (202, exit 2 in the CLI). Allow applies exactly that change; Deny none.
//!
//! Never the real `~/.k2`, never the keychain.

use crate::support::app_fixture;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};

fn tag() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

fn app_row(ws: &Ws, name: &str) -> String {
    let dir = ws.path.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::insert(&conn, &ws.id, name, "node s.mjs", &dir.to_string_lossy(), 47_531, "local", "stopped", "cmd", "")
        .expect("insert service")
        .id
}

/// The workspace's pinned chat, where K2 puts its cards.
fn pin_chat(ws: &Ws) {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
}

fn confirm(bound: &str) -> String {
    k2_daemon::vault::get().expect("vault open").confirm.mint("app-approve", Some(bound), "owner", "local-auth")
}

async fn owner_post(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

/// The caps a role has in room `room` (`None`: the room isn't on the role).
fn caps_in(role: &str, room: &Ws) -> Option<Vec<String>> {
    let r = k2_core::skin::find_role(role).expect("lookup").unwrap_or_else(|| panic!("no role {role}"));
    r.room_policy.get(&room.id).cloned()
}

fn line(ws: &Ws) -> Value {
    json!({ "handle": ws.handle, "caps": ["thread:read", "thread:post"] })
}

/// The held App requests the workspace filed.
async fn held_of(port: u16, ws: &Ws) -> Vec<Value> {
    let r = get(port, "/cli/app/approvals", OWNER, "state=all").await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["requests"].as_array().expect("requests").iter().filter(|x| x["requesterWorkspace"] == json!(ws.id)).cloned().collect()
}

struct Box3 {
    sales: Ws,
    ops: Ws,
    shop: String,
    shop_name: String,
    agent: String,
}

/// `sales` runs the App `shop`; `ops` is another workspace (the room it adds).
fn box3(env: &Env, t: &str, users: bool) -> Box3 {
    let sales = seed_ws(env, &format!("rsales{t}"));
    let ops = seed_ws(env, &format!("rops{t}"));
    let shop_name = format!("rshop{t}");
    let shop = app_row(&sales, &shop_name);
    if users {
        // As on the server it was reported from: "Build and run Apps", "…and
        // manage them end to end" and "Let this agent manage its Apps' users".
        allow_manage(&sales, true);
        k2_core::workspace::settings::set_agents_can_manage_apps_full(&sales.id, true).expect("switch 2");
        assert!(k2_core::workspace::settings::agents_can_manage_app_users(&sales.id));
    } else {
        allow_build_only(&sales, true);
    }
    let agent = passport(&sales);
    Box3 { sales, ops, shop, shop_name, agent }
}

/// The field case: the owner made a role of the agent's own App for a person;
/// the agent adds another workspace's room to it, with the switch on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_the_switch_on_an_apps_agent_adds_any_room_to_any_role_of_its_own_app() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let b = box3(&env, &t, true);

    let role = format!("person-owner{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": role, "app": b.shop, "roomAccess": [line(&b.sales)] })).await;
    let r = post(port, "/cli/skin/roles/room", &b.agent, json!({ "name": role, "handle": b.ops.handle, "caps": ["thread:read", "files:read"] })).await;
    assert_eq!(r.status, 200, "its own App's role takes another workspace's room: {}", r.body);
    assert_eq!(caps_in(&role, &b.ops), Some(vec!["thread:read".to_string(), "files:read".to_string()]), "applied at once");
    assert_eq!(caps_in(&role, &b.sales), Some(vec!["thread:read".to_string(), "thread:post".to_string()]), "the other line stays");
    let audited = k2_core::auth_audit::read_all().expect("audit");
    assert!(
        audited.iter().any(|x| x["event"] == "app_user_role_room_changed"
            && x["user"].as_str().is_some_and(|u| u.starts_with("agent:"))
            && x["outcome"].as_str().is_some_and(|o| o.contains(&b.ops.handle))),
        "audited as the agent's change: {audited:?}"
    );

    // A role the owner made already reaching elsewhere is its App's too.
    let portal = format!("portal{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": portal, "app": b.shop, "roomAccess": [line(&b.ops)] })).await;
    let r = post(port, "/cli/skin/roles/room", &b.agent, json!({ "name": portal, "handle": b.ops.handle, "caps": ["thread:read"] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(caps_in(&portal, &b.ops), Some(vec!["thread:read".to_string()]));
    let r = post(port, "/cli/skin/roles/room", &b.agent, json!({ "name": portal, "handle": b.ops.handle, "clear": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(caps_in(&portal, &b.ops), None);
    assert!(held_of(port, &b.sales).await.is_empty(), "nothing held with the switch on");
}

/// With the switch on, another workspace's App role stays the owner's:
/// refused (nothing changed, nothing held), naming the next step, and the
/// refusal says who this terminal acts as. A box-wide role is one held
/// request with an Owner-only card (0.46.18), its hint naming an App role
/// first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_the_switch_on_a_box_wide_role_is_held_for_the_owner_and_another_apps_role_stays_refused() {
    let env = setup(&[]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let b = box3(&env, &t, true);
    pin_chat(&b.sales);
    let crm = app_row(&b.ops, &format!("rcrm{t}"));

    let wide = format!("wide{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": wide, "roomAccess": [line(&b.sales)] })).await;
    let r = post(port, "/cli/skin/roles/room", &b.agent, json!({ "name": wide, "handle": b.ops.handle, "caps": ["thread:read"] })).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "a box-wide role: {}", r.body);
    let hint = r.json()["hint"].as_str().expect("hint").to_string();
    let app_role = hint.find("k2 app role create").expect("names an App role");
    let held_at = hint.find("held for the owner").expect("then the card");
    assert!(hint.contains("box-wide") && app_role < held_at, "an App role first: {hint}");
    assert!(r.json()["card"]["card"].as_str().is_some_and(|c| !c.is_empty()), "a card: {}", r.body);
    assert_eq!(caps_in(&wide, &b.ops), None, "nothing changed yet");
    let held = held_of(port, &b.sales).await;
    assert_eq!(held.len(), 1, "{held:?}");
    let summary = held[0]["summary"].as_str().expect("summary");
    assert!(summary.contains("box-wide role") && summary.contains("box logins too"), "{summary}");

    let theirs = format!("theirs{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": theirs, "app": crm, "roomAccess": [line(&b.ops)] })).await;
    let r = post(port, "/cli/skin/roles/room", &b.agent, json!({ "name": theirs, "handle": b.sales.handle, "caps": ["thread:read"] })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "another workspace's App role: {}", r.body);
    let hint = r.json()["error"]["hint"].as_str().expect("hint").to_string();
    assert!(hint.contains("another workspace's App") && hint.contains("k2 app role create"), "says what to do instead: {hint}");
    assert_eq!(caps_in(&theirs, &b.sales), None, "nothing changed");
    assert_eq!(r.json()["error"]["actingAs"], json!(format!("agent:{}", b.sales.handle)), "who this terminal acts as: {}", r.body);
    assert!(
        hint.contains(&format!("This terminal acts as agent @{} (its passport), even when you type here", b.sales.handle)),
        "said plainly: {hint}"
    );
    assert_eq!(held_of(port, &b.sales).await.len(), 1, "only the box-wide ask is held");
    k2_daemon::apps::alerts::set_sink(None);
}

/// 0.46.18 papercut 1: a box-wide role's card is the owner's alone. An Admin
/// login can't approve it (its own confirm or not); the owner's Allow
/// applies exactly that line; once the role is an App's, Allow asks again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_box_wide_role_card_is_the_owners_alone_and_allow_applies_exactly_that_line() {
    let env = setup(&[]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let port = d.port;
    let t = tag();
    let b = box3(&env, &t, false);
    pin_chat(&b.sales);

    let admin = format!("badmin{t}");
    k2_core::connect_users::add_user(&admin, "correct-horse-battery-staple-1").expect("add admin");
    k2_core::connect_users::set_role(&admin, k2_core::connect_users::Role::Admin).expect("admin");
    let session = k2_core::connect_users::create_session(&admin);

    let wide = format!("bwide{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": wide, "roomAccess": [line(&b.sales)] })).await;
    let r = post(port, "/cli/skin/roles/room", &b.agent, json!({ "name": wide, "handle": b.ops.handle, "caps": ["thread:read", "files:read"] })).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "switch off, still one card: {}", r.body);
    let held = r.json()["request"]["id"].as_str().expect("id").to_string();

    // The card asks owners only: the Admin login is not asked.
    let listed = held_of(port, &b.sales).await;
    let routed = listed[0]["card"]["routedTo"].as_array().expect("routedTo").clone();
    assert!(!routed.iter().any(|x| x["login"] == json!(admin)), "an Owner floor: {routed:?}");

    // An Admin login: refused before any confirm.
    let r = post(port, "/cli/app/approve", &session, json!({ "request": held, "confirmToken": "x" })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    assert!(r.body.contains("only the owner approves a change to a box-wide role"), "{}", r.body);
    assert_eq!(caps_in(&wide, &b.ops), None, "nothing applied");

    // The owner: exactly that line.
    let r = post(port, "/cli/app/approve", OWNER, json!({ "request": held, "confirmToken": confirm(&held) })).await;
    assert_eq!((r.status, r.json()["status"].clone()), (200, json!("approved")), "{}", r.body);
    assert_eq!(caps_in(&wide, &b.ops), Some(vec!["thread:read".to_string(), "files:read".to_string()]));
    assert_eq!(caps_in(&wide, &b.sales), Some(vec!["thread:read".to_string(), "thread:post".to_string()]), "the rest stays");

    // Narrowed to an App in between: the old card no longer applies.
    let desk = seed_ws(&env, &format!("bdesk{t}"));
    let r = post(port, "/cli/skin/roles/room", &b.agent, json!({ "name": wide, "handle": desk.handle })).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let again = r.json()["request"]["id"].as_str().expect("id").to_string();
    owner_post(port, "/cli/skin/roles/update", json!({ "name": wide, "app": b.shop })).await;
    let r = post(port, "/cli/app/approve", OWNER, json!({ "request": again, "confirmToken": confirm(&again) })).await;
    assert_eq!(r.json()["status"], "failed", "{}", r.body);
    assert_eq!(caps_in(&wide, &desk), None, "never applied to an App's role");
    k2_daemon::apps::alerts::set_sink(None);
}

/// With the switch off: one held request and a card for the owner. Allow
/// applies exactly the change it showed; Deny applies nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_the_switch_off_it_is_one_held_request_with_a_card_and_allow_applies_exactly_that() {
    let env = setup(&[]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let port = d.port;
    let t = tag();
    let b = box3(&env, &t, false);
    pin_chat(&b.sales);
    let desk = seed_ws(&env, &format!("rdesk{t}"));

    let role = format!("person-owner{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": role, "app": b.shop, "roomAccess": [line(&b.sales)] })).await;
    let r = post(port, "/cli/skin/roles/room", &b.agent, json!({ "name": role, "handle": b.ops.handle, "caps": ["thread:read", "files:read"] })).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "{}", r.body);
    let v = r.json();
    assert_eq!(v["kind"], "manage", "{v}");
    assert!(v["card"]["card"].as_str().is_some_and(|c| !c.is_empty()), "a card on the agent's Thread: {v}");
    let summary = v["request"]["summary"].as_str().expect("summary").to_string();
    for want in [role.as_str(), b.shop_name.as_str(), b.ops.handle.as_str(), "thread:read, files:read"] {
        assert!(summary.contains(want), "the card names {want}: {summary}");
    }
    assert!(v["hint"].as_str().expect("hint").starts_with("held for the owner"), "{v}");
    assert_eq!(caps_in(&role, &b.ops), None, "nothing changes before the owner's Allow");

    // The owner changes the role in between: Allow still applies only that line.
    owner_post(port, "/cli/skin/roles/room", json!({ "name": role, "handle": b.sales.handle, "caps": ["thread:read"] })).await;
    let held = v["request"]["id"].as_str().expect("id").to_string();
    let r = post(port, "/cli/app/approve", OWNER, json!({ "request": held, "confirmToken": confirm(&held) })).await;
    assert_eq!((r.status, r.json()["status"].clone()), (200, json!("approved")), "{}", r.body);
    assert_eq!(caps_in(&role, &b.ops), Some(vec!["thread:read".to_string(), "files:read".to_string()]), "exactly that room and caps");
    assert_eq!(caps_in(&role, &b.sales), Some(vec!["thread:read".to_string()]), "the owner's own edit stays");

    // Deny: nothing.
    let r = post(port, "/cli/skin/roles/room", &b.agent, json!({ "name": role, "handle": desk.handle })).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "{}", r.body);
    let held = r.json()["request"]["id"].as_str().expect("id").to_string();
    let r = post(port, "/cli/app/deny", OWNER, json!({ "request": held })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(caps_in(&role, &desk), None, "denied: nothing applied");

    // Still refused outright, switch or not: another workspace's App role.
    let crm = app_row(&b.ops, &format!("rcrm{t}"));
    let theirs = format!("theirs{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": theirs, "app": crm, "roomAccess": [line(&b.ops)] })).await;
    let r = post(port, "/cli/skin/roles/room", &b.agent, json!({ "name": theirs, "handle": b.sales.handle })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    assert_eq!(held_of(port, &b.sales).await.len(), 2, "only the two asks above were held");
    k2_daemon::apps::alerts::set_sink(None);
}

/// 0.46.18 papercut 3: a role says its scope. Without an App it is box-wide
/// (`scope: "box"` and a hint naming `--app`); with one, `scope: "app"` and
/// the App as `<workspace>/<name>`; the list says each role's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_role_answers_with_its_scope_and_the_list_shows_each() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let b = box3(&env, &t, true);

    let wide = format!("swide{t}");
    let v = owner_post(port, "/cli/skin/roles", json!({ "name": wide })).await;
    assert_eq!(v["scope"], "box", "{v}");
    let hint = v["hint"].as_str().expect("a box-wide role says so").to_string();
    assert!(hint.contains("box-wide") && hint.contains("--app <app>") && hint.contains("only the owner edits it"), "{hint}");

    let own = format!("sown{t}");
    let v = owner_post(port, "/cli/skin/roles", json!({ "name": own, "app": b.shop })).await;
    assert_eq!((v["scope"].clone(), v["appLabel"].clone()), (json!("app"), json!(format!("{}/{}", b.sales.handle, b.shop_name))), "{v}");
    assert!(v.get("hint").is_none(), "an App's role needs no warning: {v}");

    let r = get(port, "/cli/skin/roles", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let roles = r.json()["roles"].as_array().expect("roles").clone();
    let of = |n: &str| roles.iter().find(|x| x["name"] == json!(n)).cloned().unwrap_or_else(|| panic!("no {n} in {roles:?}"));
    assert_eq!(of(&wide)["scope"], "box");
    assert_eq!(of(&own)["scope"], "app");
    assert_eq!(of(&own)["appLabel"], json!(format!("{}/{}", b.sales.handle, b.shop_name)));
}

/// `k2 app role update <role> --app <app>` narrows a box-wide role to one
/// App, and is refused (409, naming them, nothing changed) while a login
/// holds it on another App or as its own role everywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_box_wide_role_narrows_to_one_app_only_while_no_one_holds_it_elsewhere() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let b = box3(&env, &t, true);
    let crm = app_row(&b.ops, &format!("ncrm{t}"));

    // Held on another App: refused, naming who.
    let wide = format!("nwide{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": wide })).await;
    let guest = format!("nguest{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": guest, "app": crm, "role": wide })).await;
    let r = post(port, "/cli/skin/roles/update", OWNER, json!({ "name": wide, "app": b.shop })).await;
    assert_eq!((r.status, r.code()), (409, "role_held_elsewhere".to_string()), "{}", r.body);
    assert!(r.json()["error"]["hint"].as_str().expect("hint").contains(&guest), "names who would lose it: {}", r.body);
    assert_eq!(k2_core::skin::find_role(&wide).unwrap().unwrap().app_id, None, "nothing changed");

    // Held only on that App: narrowed.
    let fine = format!("nfine{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": fine })).await;
    let on_shop = format!("nshop{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": on_shop, "app": b.shop, "role": fine })).await;
    let v = owner_post(port, "/cli/skin/roles/update", json!({ "name": fine, "app": b.shop })).await;
    assert_eq!(v["scope"], "app", "{v}");
    assert_eq!(k2_core::skin::find_role(&fine).unwrap().unwrap().app_id.as_deref(), Some(b.shop.as_str()));

    // A login's own role (it applies everywhere): refused too.
    let mine = format!("nmine{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": mine })).await;
    let person = format!("nperson{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": person })).await;
    owner_post(port, "/cli/skin/roles/assign", json!({ "username": person, "role": mine })).await;
    let r = post(port, "/cli/skin/roles/update", OWNER, json!({ "name": mine, "app": b.shop })).await;
    assert_eq!((r.status, r.code()), (409, "role_held_elsewhere".to_string()), "{}", r.body);
    assert!(r.body.contains(&person), "{}", r.body);

    // Another App's role never moves to this one.
    let theirs = format!("ntheirs{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": theirs, "app": crm })).await;
    let r = post(port, "/cli/skin/roles/update", OWNER, json!({ "name": theirs, "app": b.shop })).await;
    assert_eq!(r.status, 409, "{}", r.body);
    assert_eq!(k2_core::skin::find_role(&theirs).unwrap().unwrap().app_id.as_deref(), Some(crm.as_str()));

    // An agent narrows only to an App of its own.
    let agents = format!("nagent{t}");
    let r = post(port, "/cli/skin/roles", &b.agent, json!({ "name": agents, "app": b.shop })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/skin/roles/update", &b.agent, json!({ "name": agents, "app": crm })).await;
    assert_eq!(r.status, 403, "another workspace's App: {}", r.body);
}
