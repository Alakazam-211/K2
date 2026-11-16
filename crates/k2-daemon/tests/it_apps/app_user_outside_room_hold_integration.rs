//! `k2 app user …` by an App's own agent (with "Let this agent manage its
//! Apps' users") on a login of its OWN App that talks in another workspace's
//! room (0.46.14), headless: the real dispatcher in process on a temp `$HOME`,
//! over loopback.
//!
//! - add (with a box-wide role naming that room), remove, role and the
//!   password: one held request with a card for the owner (202), never the
//!   refusal of before; Allow applies exactly that change, Deny none;
//! - a reset: held, and the approval mails the person the App's link;
//! - the password is never on the request, its card or the audit: the
//!   approval lets the agent set it once;
//! - another workspace's App's login, and an agent without the switch, are
//!   refused as before (nothing held).
//!
//! Never the real `~/.k2`, never the keychain, no real mail.

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
    k2_core::published_services::insert(&conn, &ws.id, name, "node s.mjs", &dir.to_string_lossy(), 47_533, "local", "stopped", "cmd", "")
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

async fn approve(port: u16, id: &str) -> Value {
    let r = post(port, "/cli/app/approve", OWNER, json!({ "request": id, "confirmToken": confirm(id) })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()
}

async fn deny(port: u16, id: &str) {
    let r = post(port, "/cli/app/deny", OWNER, json!({ "request": id })).await;
    assert_eq!(r.status, 200, "{}", r.body);
}

/// The held App requests the workspace filed.
async fn held_of(port: u16, ws: &Ws) -> Vec<Value> {
    let r = get(port, "/cli/app/approvals", OWNER, "state=all").await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["requests"].as_array().expect("requests").iter().filter(|x| x["requesterWorkspace"] == json!(ws.id)).cloned().collect()
}

fn exists(username: &str) -> bool {
    k2_core::skin::principal_exists(username).expect("lookup")
}

fn principal(username: &str) -> k2_core::skin::SkinPrincipal {
    k2_core::skin::list_principals().expect("list").into_iter().find(|p| p.username == username).unwrap_or_else(|| panic!("no {username}"))
}

/// The role id on `username`'s login on App `app` (`None`: no role).
fn grant_role(username: &str, app: &str) -> Option<String> {
    let p = principal(username);
    let grants = k2_core::skin::list_grants().expect("grants");
    let g = grants
        .iter()
        .find(|g| g.subject_kind == "principal" && g.subject_id == p.id && g.kind == "app" && g.target_id == app)
        .unwrap_or_else(|| panic!("{username} has no login on {app}"));
    g.role_id.clone()
}

fn role_id(name: &str) -> String {
    k2_core::skin::find_role(name).expect("lookup").unwrap_or_else(|| panic!("no role {name}")).id
}

/// Every place a password could leak to: the held requests (body, diff,
/// summary, result) and the Access Audit.
fn leaks(secret: &str) -> Vec<String> {
    let mut out = Vec::new();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut st = conn
            .prepare("SELECT id, body_json || ' ' || diff_json || ' ' || summary || ' ' || COALESCE(result_json, '') FROM app_widenings")
            .expect("prepare");
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).expect("query");
        for row in rows {
            let (id, text) = row.expect("row");
            if text.contains(secret) {
                out.push(format!("request {id}"));
            }
        }
    }
    let audit = k2_core::auth_audit::read_all().expect("audit");
    if audit.iter().any(|x| x.to_string().contains(secret)) {
        out.push("the Access Audit".into());
    }
    out
}

struct Box3 {
    sales: Ws,
    ops: Ws,
    shop: String,
    shop_name: String,
    wide: String,
    agent: String,
}

/// `sales` runs the App `shop` and its agent has "Let this agent manage its
/// Apps' users"; `ops` is another workspace; `wide` is a box-wide role whose
/// room is ops' (how an App's logins come to talk in another workspace's room).
async fn box3(env: &Env, port: u16, t: &str) -> Box3 {
    let sales = seed_ws(env, &format!("hsales{t}"));
    let ops = seed_ws(env, &format!("hops{t}"));
    let shop_name = format!("hshop{t}");
    let shop = app_row(&sales, &shop_name);
    allow_manage(&sales, true);
    assert!(k2_core::workspace::settings::agents_can_manage_app_users(&sales.id));
    pin_chat(&sales);
    let wide = format!("wide{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": wide, "roomAccess": [{ "handle": ops.handle, "caps": ["thread:read", "thread:post"] }] })).await;
    let agent = passport(&sales);
    Box3 { sales, ops, shop, shop_name, wide, agent }
}

/// The held answer: 202 "held", a card on the agent's Thread, a summary that
/// names the login, the App and the other workspace's room.
fn assert_held(r: &Resp, login: &str, b: &Box3) -> String {
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "held, not refused: {}", r.body);
    let v = r.json();
    assert_eq!(v["kind"], "manage", "{v}");
    assert!(v["card"]["card"].as_str().is_some_and(|c| !c.is_empty()), "a card on the agent's Thread: {v}");
    assert!(v["hint"].as_str().expect("hint").starts_with("held for the owner"), "{v}");
    let summary = v["request"]["summary"].as_str().expect("summary").to_string();
    for want in [login, b.shop_name.as_str(), b.ops.handle.as_str()] {
        assert!(summary.contains(want), "the card names {want}: {summary}");
    }
    v["request"]["id"].as_str().expect("id").to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_and_role_on_its_apps_login_in_another_room_are_held_and_allow_applies_exactly_that() {
    let env = setup(&[]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let port = d.port;
    let t = tag();
    let b = box3(&env, port, &t).await;

    // The field case: the owner's login on the agent's App talks in ops' room.
    let ann = format!("ann{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": ann, "app": b.shop, "role": b.wide })).await;
    let r = post(port, "/cli/skin/users/remove", &b.agent, json!({ "username": ann })).await;
    let id = assert_held(&r, &ann, &b);
    assert!(r.json()["request"]["summary"].as_str().unwrap().starts_with(&format!("remove the login {ann}")), "{}", r.body);
    assert!(exists(&ann), "nothing changes before the owner's Allow");
    let done = approve(port, &id).await;
    assert_eq!(done["status"], "approved", "{done}");
    assert!(!exists(&ann), "Allow removed exactly that login");

    // Deny: nothing.
    let bob = format!("bob{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": bob, "app": b.shop, "role": b.wide })).await;
    let r = post(port, "/cli/skin/users/remove", &b.agent, json!({ "username": bob })).await;
    let id = assert_held(&r, &bob, &b);
    deny(port, &id).await;
    assert!(exists(&bob), "denied: still there");

    // A role whose room is another workspace's, on a login of its own App.
    let cara = format!("cara{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": cara, "app": b.shop })).await;
    assert_eq!(grant_role(&cara, &b.shop), None);
    let r = post(port, "/cli/skin/roles/assign", &b.agent, json!({ "username": cara, "role": b.wide })).await;
    let id = assert_held(&r, &cara, &b);
    assert!(r.json()["request"]["summary"].as_str().unwrap().contains(&b.wide), "the card names the role: {}", r.body);
    assert_eq!(grant_role(&cara, &b.shop), None, "nothing before Allow");
    let done = approve(port, &id).await;
    assert_eq!(done["status"], "approved", "{done}");
    assert_eq!(grant_role(&cara, &b.shop), Some(role_id(&b.wide)), "Allow gave exactly that role on that login");

    // Each is on the App's approvals, filed by the agent.
    let held = held_of(port, &b.sales).await;
    assert_eq!(held.len(), 3, "{held:?}");
    k2_daemon::apps::alerts::set_sink(None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_and_password_are_held_without_the_password_and_the_approval_opens_one_set() {
    let env = setup(&[]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let port = d.port;
    let t = tag();
    let b = box3(&env, port, &t).await;
    let pw = format!("Sent-with-the-ask-{t}");

    // Add with a box-wide role naming ops' room: held, the password not used.
    let dan = format!("dan{t}");
    let r = post(port, "/cli/skin/users", &b.agent, json!({ "username": dan, "app": b.shop, "role": b.wide, "password": pw })).await;
    let id = assert_held(&r, &dan, &b);
    assert_eq!(r.json()["passwordNotUsed"], true, "{}", r.body);
    assert!(!exists(&dan), "nothing before Allow");
    let done = approve(port, &id).await;
    assert_eq!(done["status"], "approved", "{done}");
    assert!(exists(&dan), "Allow made exactly that login");
    let p = principal(&dan);
    assert_eq!(grant_role(&dan, &b.shop), Some(role_id(&b.wide)), "with that role on that App");
    assert!(!p.has_password, "no password from the request");
    assert_eq!(k2_core::skin::creators::workspace_of(&p.id).expect("stamp").as_deref(), Some(b.sales.id.as_str()), "made as the agent's login");

    // The approval opened one password set: the agent sets it once.
    let chosen = format!("Chosen-after-approval-{t}");
    let r = post(port, "/cli/skin/users/password", &b.agent, json!({ "username": dan, "password": chosen })).await;
    assert_eq!(r.status, 200, "the one set: {}", r.body);
    assert_eq!(r.json()["once"], true, "{}", r.body);
    assert!(principal(&dan).has_password);
    // A second set is a new ask, held again.
    let again = format!("Second-try-{t}");
    let r = post(port, "/cli/skin/users/password", &b.agent, json!({ "username": dan, "password": again })).await;
    let id = assert_held(&r, &dan, &b);
    assert_eq!(r.json()["passwordNotUsed"], true, "{}", r.body);
    assert!(r.json()["afterApproval"].as_str().is_some_and(|a| a.contains("--password-stdin")), "{}", r.body);
    let done = approve(port, &id).await;
    assert_eq!(done["status"], "approved", "{done}");
    let third = format!("Third-after-approval-{t}");
    let r = post(port, "/cli/skin/users/password", &b.agent, json!({ "username": dan, "password": third })).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // Never on a request, a card's text or the audit.
    for secret in [pw.as_str(), chosen.as_str(), again.as_str(), third.as_str()] {
        assert_eq!(leaks(secret), Vec::<String>::new(), "the password {secret} leaked");
    }
    k2_daemon::apps::alerts::set_sink(None);
}

/// Every hosted submit, in place of the mail server (no real sends).
#[derive(Default)]
struct Recorder(std::sync::Mutex<Vec<k2_daemon::mail::send::OutboundMessage>>);

impl k2_daemon::mail::send::SubmitBackend for Recorder {
    fn submit(&self, _account_id: &str, msg: &k2_daemon::mail::send::OutboundMessage) -> Result<(), String> {
        self.0.lock().unwrap().push(msg.clone());
        Ok(())
    }
}

fn seed_mail(ws: &Ws, domain: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_domains (id, domain, send_mode, status, created_at) VALUES (?1, ?2, 'direct', 'verified', 100)",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), domain],
    )
    .expect("domain");
    conn.execute(
        "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, owner_project_id, status, created_at)
         VALUES (?1, ?2, 'dom-x', ?3, ?4, 'active', 100)",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), format!("bot@{domain}"), format!("acct-{domain}"), ws.id],
    )
    .expect("address");
    conn.execute("UPDATE projects SET mail_agent_send = 'on' WHERE id = ?1", rusqlite::params![ws.id]).expect("mode");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reset_is_held_and_the_approval_mails_the_apps_link() {
    let env = setup(&[]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let port = d.port;
    let rec = std::sync::Arc::new(Recorder::default());
    let backend: std::sync::Arc<dyn k2_daemon::mail::send::SubmitBackend + Send + Sync> = rec.clone();
    k2_daemon::app_user_invites::set_backend_override(Some(backend));
    let t = tag();
    let b = box3(&env, port, &t).await;
    seed_mail(&b.sales, &format!("rst{t}.example.test"));
    let door = format!("https://door{t}.example.test");
    owner_post(port, "/cli/skin/front-door", json!({ "mode": "direct", "url": door, "apply": false })).await;
    owner_post(port, "/cli/skin/access/invites", json!({ "app": b.shop, "setPasswordUrl": format!("{door}/set") })).await;

    let eve = format!("eve{t}");
    let email = format!("{eve}@example.test");
    owner_post(port, "/cli/skin/users", json!({ "username": eve, "email": email, "app": b.shop, "role": b.wide })).await;
    let r = post(port, "/cli/skin/users/reset-link", &b.agent, json!({ "username": eve, "app": b.shop })).await;
    let id = assert_held(&r, &eve, &b);
    let mailed = || rec.0.lock().unwrap().iter().filter(|m| m.to.iter().any(|x| x == &email)).count();
    assert_eq!(mailed(), 0, "nothing mailed before Allow");
    let done = approve(port, &id).await;
    assert_eq!(done["status"], "approved", "{done}");
    assert_eq!(mailed(), 1, "Allow mailed the App's reset link to the person: {done}");
    assert!(!done.to_string().contains("k2inv_"), "nobody else sees the link: {done}");
    k2_daemon::app_user_invites::set_backend_override(None);
    k2_daemon::apps::alerts::set_sink(None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_apps_login_and_an_agent_without_the_switch_stay_refused() {
    let env = setup(&[]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let port = d.port;
    let t = tag();
    let b = box3(&env, port, &t).await;
    let crm = app_row(&b.ops, &format!("hcrm{t}"));

    // Another workspace's App's login.
    let fay = format!("fay{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": fay, "app": crm, "role": b.wide })).await;
    for (path, body) in [
        ("/cli/skin/users/remove", json!({ "username": fay })),
        ("/cli/skin/users/password", json!({ "username": fay, "password": format!("Never-{t}-set") })),
        ("/cli/skin/roles/assign", json!({ "username": fay, "role": b.wide })),
    ] {
        let r = post(port, path, &b.agent, body).await;
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{path}: {}", r.body);
    }
    let r = post(port, "/cli/skin/users/reset-link", &b.agent, json!({ "username": fay, "app": crm })).await;
    assert!(matches!(r.status, 403 | 404), "{}", r.body);
    assert!(exists(&fay));

    // A login on its own App AND another workspace's: still another App's.
    let gus = format!("gus{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": gus, "app": b.shop, "role": b.wide })).await;
    owner_post(port, "/cli/skin/grants", json!({ "subjectKind": "principal", "subject": principal(&gus).id, "kind": "app", "target": crm, "rooms": [] })).await;
    let r = post(port, "/cli/skin/users/remove", &b.agent, json!({ "username": gus })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);

    // A role of another workspace's App on its own login: refused, not held.
    let theirs = format!("theirs{t}");
    owner_post(port, "/cli/skin/roles", json!({ "name": theirs, "app": crm, "roomAccess": [{ "handle": b.ops.handle, "caps": ["thread:read"] }] })).await;
    let hal = format!("hal{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": hal, "app": b.shop })).await;
    let r = post(port, "/cli/skin/roles/assign", &b.agent, json!({ "username": hal, "role": theirs })).await;
    assert_ne!(r.status, 202, "another App's role is never held: {}", r.body);
    assert_ne!(r.status, 200, "{}", r.body);

    // No App-users switch: its own refusal (the ask-for-the-switch path).
    let ivy = format!("ivy{t}");
    owner_post(port, "/cli/skin/users", json!({ "username": ivy, "app": b.shop, "role": b.wide })).await;
    allow_manage(&b.sales, false);
    let r = post(port, "/cli/skin/users/remove", &b.agent, json!({ "username": ivy })).await;
    assert_eq!((r.status, r.code()), (403, "app_users_off".to_string()), "{}", r.body);
    assert!(exists(&ivy));

    assert!(held_of(port, &b.sales).await.is_empty(), "nothing was held");
    k2_daemon::apps::alerts::set_sink(None);
}
