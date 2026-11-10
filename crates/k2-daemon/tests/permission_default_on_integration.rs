//! Permission cards on by default (0.46.11, Rosson 2026-11-10), end to end
//! on a real in-process daemon with a temp `$HOME`:
//!
//! 1. **A fresh server** (no settings file): the boot pass leaves it alone,
//!    the gate is on, and `/boot-status` lists `permission-cards-v1`.
//! 2. **An existing server whose switch was never chosen** (0.46.10 saved
//!    `permissionCards: false` on every write): the pass turns it on once,
//!    with one Access Audit line and one notice to the Owner (never an
//!    Admin); a second boot does nothing and files nothing.
//! 3. **An owner's off survives**: off through Settings sticks across
//!    boots, and an older file whose off the Access Audit shows a person
//!    chose stays off (no notice).
//! 4. **App agents** (an agent whose Thread an App's guests use): its card
//!    goes to the Owner and Admins only. The guest never sees it, can't
//!    answer it, and is never an approver, a Tickets assignee or a push
//!    recipient; a Member isn't either.
//!
//! Its own binary: it moves the process-wide switch (lane B's rule). One
//! test function, so nothing in here races.

#![recursion_limit = "256"]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use k2_core::app_settings::PermissionCardsDefault;
use k2_core::connect_users::Role;
use k2_core::db::schema::WorkspaceSession;
use k2_daemon::permission_default;
use serde_json::{json, Value};

const PW: &str = "s3cret-horse-staple";
const WHY: &str = "Open a sidecar to review the App's copy";

fn settings_path(env: &Env) -> std::path::PathBuf {
    env.path().join(".k2").join("settings.json")
}

fn stored(env: &Env) -> Value {
    serde_json::from_str(&std::fs::read_to_string(settings_path(env)).expect("read settings")).expect("settings JSON")
}

fn write_settings(env: &Env, v: Value) {
    std::fs::create_dir_all(env.path().join(".k2")).unwrap();
    std::fs::write(settings_path(env), serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

fn features(r: &Resp) -> Vec<String> {
    r.json()["features"]
        .as_array()
        .unwrap_or_else(|| panic!("no features: {}", r.body))
        .iter()
        .map(|f| f.as_str().expect("feature string").to_string())
        .collect()
}

async fn listed(port: u16) -> bool {
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "GET", "/boot-status", None)).await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    features(&r).contains(&k2_core::permission::FEATURE_KEY.to_string())
}

fn notices() -> Vec<k2_core::feedback::FeedbackItem> {
    k2_core::feedback::list_for_project(k2_core::db::AUDIT_SENTINEL_IDS[0], &k2_core::feedback::ListFilter::All)
        .expect("tickets")
        .into_iter()
        .filter(|t| t.title == permission_default::NOTICE_TITLE)
        .collect()
}

fn switch_lines() -> Vec<Value> {
    k2_core::auth_audit::read_all()
        .expect("read the access audit")
        .into_iter()
        .filter(|l| l["event"] == k2_core::permission::audit_event::SWITCH)
        .collect()
}

fn login(name: &str, role: Role) -> String {
    k2_core::connect_users::add_user(name, "correct-horse-battery-staple-1").expect("add user");
    k2_core::connect_users::set_role(name, role).expect("set role");
    name.to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permission_cards_are_on_by_default_and_an_owners_off_sticks() {
    let env = setup(&[]);
    std::env::remove_var(k2_core::permission::ENV_FORCE_ON);
    k2_core::permission::gate::set_server_switch(false);
    let owner_login = login("pdo-owner", Role::Owner);
    let admin_login = login("pdo-admin", Role::Admin);
    let member_login = login("pdo-member", Role::Member);

    // ── 1. a fresh server: on, and listed ──────────────────────────────
    assert!(!settings_path(&env).exists(), "a fresh server has no settings file");
    assert_eq!(permission_default::boot(), PermissionCardsDefault::NoFile);
    assert!(k2_core::permission::gate::server_on(), "a fresh server has permission cards on");
    assert!(!settings_path(&env).exists(), "the pass writes nothing for a fresh server");
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    assert!(listed(port).await, "/boot-status lists {} on a fresh server", k2_core::permission::FEATURE_KEY);
    assert!(!permission_default::notify(PermissionCardsDefault::NoFile), "no notice for a fresh server");
    assert!(notices().is_empty());

    // ── 2. an existing server, switch never chosen: on once, one notice ─
    write_settings(&env, json!({ "defaultAgent": "claude", "permissionCards": false, "apps": true }));
    k2_core::permission::gate::set_server_switch(false);
    assert!(!listed(port).await, "off before the update");
    let outcome = permission_default::boot();
    assert_eq!(outcome, PermissionCardsDefault::TurnedOn);
    assert!(k2_core::permission::gate::server_on(), "the update turned it on");
    assert!(listed(port).await);
    let v = stored(&env);
    assert_eq!((v["permissionCards"].as_bool(), v["permissionCardsDefaultApplied"].as_bool()), (Some(true), Some(true)), "{v}");
    assert_eq!(v["defaultAgent"], "claude", "other keys are kept: {v}");
    let lines = switch_lines();
    assert_eq!(lines.last().map(|l| l["outcome"].clone()), Some(json!(permission_default::AUDIT_OUTCOME)), "{lines:?}");
    assert!(permission_default::notify(outcome), "the owner is told");
    let n = notices();
    assert_eq!(n.len(), 1, "one notice: {n:?}");
    assert_eq!(n[0].kind, "fyi");
    assert_eq!(n[0].body.as_deref(), Some(permission_default::NOTICE_BODY));
    assert_eq!(n[0].assignees, vec![owner_login.clone()], "the Owner only, never {admin_login} or {member_login}");
    // The next boot: nothing again.
    let again = permission_default::boot();
    assert_eq!(again, PermissionCardsDefault::AlreadyApplied);
    assert!(!permission_default::notify(again));
    assert_eq!(notices().len(), 1, "never a second notice");
    assert_eq!(switch_lines().len(), lines.len(), "no second audit line");

    // ── 3. the owner turns it off: it stays off ────────────────────────
    let r = post(port, "/cli/settings/update", OWNER, json!({ "permissionCards": false })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!k2_core::permission::gate::server_on());
    for _ in 0..2 {
        assert_eq!(permission_default::boot(), PermissionCardsDefault::AlreadyApplied);
        assert!(!k2_core::permission::gate::server_on(), "an owner's off is never turned back on");
        assert!(!listed(port).await);
    }
    // A file from before 0.46.11 (no mark) whose off a person chose (the
    // Access Audit's last server line is "server off"): it stays off.
    assert_eq!(switch_lines().last().map(|l| l["outcome"].clone()), Some(json!("server off")));
    let mut v = stored(&env);
    v.as_object_mut().unwrap().remove("permissionCardsDefaultApplied");
    write_settings(&env, v);
    let kept = permission_default::boot();
    assert_eq!(kept, PermissionCardsDefault::KeptOffByOwner);
    assert!(!k2_core::permission::gate::server_on());
    assert!(!permission_default::notify(kept));
    assert_eq!(notices().len(), 1, "no notice when it stays off");
    assert_eq!(stored(&env)["permissionCardsDefaultApplied"], true);

    // ── 4. an App agent's card reaches the Owner and Admins only ───────
    let r = post(port, "/cli/settings/update", OWNER, json!({ "permissionCards": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(k2_core::permission::gate::server_on());
    let ws = seed_ws(&env, "pdo-shop");
    let conv = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    }
    // The App's guest: thread read + post in this agent's room, so it could
    // answer a card if it ever saw one.
    let ok = |r: Resp, what: &str| assert_eq!(r.status, 200, "{what}: {}", r.body);
    ok(post(port, "/cli/skin/roles", OWNER, json!({ "name": "pdo-shoppers" })).await, "role");
    ok(
        post(port, "/cli/skin/roles/room", OWNER, json!({ "name": "pdo-shoppers", "handle": ws.handle, "caps": ["thread:read", "thread:post"] }))
            .await,
        "room caps",
    );
    ok(post(port, "/cli/skin/users", OWNER, json!({ "username": "pdo-guest" })).await, "guest");
    k2_daemon::test_harness::guest_with_own_password("pdo-guest", PW);
    ok(post(port, "/cli/skin/roles/assign", OWNER, json!({ "username": "pdo-guest", "role": "pdo-shoppers" })).await, "assign");
    let body = json!({ "username": "pdo-guest", "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "guest login: {}", r.body);
    let guest = r.json()["token"].as_str().expect("guest token").to_string();

    // The agent's own switch is a person's (default off): turned on by the
    // owner, as from the workspace's settings.
    let agent = passport(&ws);
    let ask = json!({ "action": "agents.manage", "why": WHY });
    let r = post(port, "/cli/permission/request", &agent, ask.clone()).await;
    assert_ne!(r.status, 200, "the agent's own switch is off by default: {}", r.body);
    ok(
        post(port, "/cli/agent-access/set", OWNER, json!({ "toggle": "permission_cards", "workspace": ws.handle, "value": true })).await,
        "agent switch",
    );
    let r = post(port, "/cli/permission/request", &agent, ask).await;
    assert_eq!(r.status, 200, "the App agent asks: {}", r.body);
    let v = r.json();
    assert_eq!(v["status"], "pending", "{v}");
    let card = v["card"].as_str().unwrap_or_else(|| panic!("no card: {v}")).to_string();
    let rid = v["id"].as_str().unwrap_or_else(|| panic!("no id: {v}")).to_string();

    // Approvers: the Owner and Admin logins, nobody else.
    let action = k2_core::overlay::card_action(&conv, &card).expect("read the card").expect("an action card");
    let view = action.view.expect("a permission view");
    let mut names: Vec<String> =
        view["approvers"]["names"].as_array().expect("approver names").iter().map(|n| n.as_str().unwrap().to_string()).collect();
    names.sort();
    assert_eq!(names, vec![admin_login.clone(), owner_login.clone()], "{view}");
    // The Tickets item: the same people.
    let tickets = k2_core::feedback::list_for_project(&ws.id, &k2_core::feedback::ListFilter::All).expect("tickets");
    assert_eq!(tickets.len(), 1, "{tickets:?}");
    let mut assignees = tickets[0].assignees.clone();
    assignees.sort();
    assert_eq!(assignees, vec![admin_login.clone(), owner_login.clone()], "never the guest or a Member");

    // The guest never sees it, and can't answer it.
    let h = enc(&ws.handle);
    let page = get(port, "/cli/thread", &guest, &format!("addr={h}&limit=50&cards=2")).await;
    assert_eq!(page.status, 200, "{}", page.body);
    for needle in [card.as_str(), rid.as_str(), WHY, "agents.manage", "requestId"] {
        assert!(!page.body.contains(needle), "a guest saw {needle:?}: {}", page.body);
    }
    for answer in ["Allow", "Deny"] {
        let g = post(port, "/cli/thread/answer", &guest, json!({ "addr": ws.handle, "id": card, "answer": answer })).await;
        assert_ne!(g.status, 200, "a guest answered a permission card: {}", g.body);
    }
    let g = get(port, "/cli/permission/pending", &guest, "").await;
    assert_ne!(g.status, 200, "Waiting on you served a guest: {}", g.body);
    assert!(!g.body.contains(&rid), "{}", g.body);
    let doc = k2_core::overlay::get_thread_doc(&conv, &card).expect("read").expect("the card");
    assert_eq!(doc.choice.as_ref().map(|c| c.status.as_str()), Some("pending"), "a guest's tap changed nothing");
    // The owner sees it waiting.
    let p = get(port, "/cli/permission/pending", OWNER, "").await;
    assert_eq!(p.status, 200, "{}", p.body);
    assert!(p.body.contains(&rid), "{}", p.body);

    // Withdraw it before the switch goes off (the off close would send its
    // turn to a real agent address).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE permission_requests SET state = 'cancelled', answered_at = ?2 WHERE id = ?1",
            rusqlite::params![rid, k2_core::overlay::now_secs()],
        )
        .unwrap();
    }
    k2_core::permission::gate::set_server_switch(false);
    drop(d);
    drop(env);
}
