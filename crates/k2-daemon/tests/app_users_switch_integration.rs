//! "Let this agent manage its Apps' users" (`apps.manage-users`, migration
//! 0225; Rosson 2026-10-26), end to end through the REAL dispatcher with
//! nothing but the agent's passport once the owner has turned it on:
//!
//! - E2E: the agent installs a fresh App, makes a test guest of it in its
//!   own room, sets the guest's password, gives the App the guests-rooms
//!   grant in its own room, and the guest signs in through the App's link;
//!   the guest is tied to the agent's workspace;
//! - only the owner flips the switch (an Admin login can't, an agent never);
//! - with it off: a test guest and the grant are one held request each
//!   (a card for the owner), the App-user roster answers `app_users_off`;
//! - never: another workspace's room (held, the owner's), a box login, or
//!   another workspace's App;
//! - copy-host is always a held request listing exactly the logins: the
//!   approval copies exactly those, and refuses when the list changed.
//!
//! ISOLATION: a temp `$HOME`, the in-memory test DB, alerts to a sink. No
//! real agent CLI, no real `~/.k2`, no running daemon.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use app_fixture::*;
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::{json, Value};

fn quiet_alerts() {
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
}

fn confirm(action: &str, bound: &str) -> String {
    k2_daemon::vault::get().expect("vault open").confirm.mint(action, Some(bound), "owner", "local-auth")
}

/// One request on a fresh link connection (`Connection: close`).
fn on_link(port: u16, path: &str, pass: &str, body: &Value) -> (u16, String) {
    let b = body.to_string();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {pass}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
        b.len()
    );
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(req.as_bytes()).unwrap();
    let mut got = Vec::new();
    let _ = s.read_to_end(&mut got);
    let text = String::from_utf8_lossy(&got).to_string();
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or(0);
    (status, text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
}

/// The guarded engine's part: open the App's gateway link (an App pass).
fn open_link(app: &str, svc: &str, ws: &Ws) -> (String, u16) {
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE apps SET approved_caps_json = ?2 WHERE app_id = ?1",
            rusqlite::params![app, serde_json::to_string(&["thread:read"]).unwrap()],
        )
        .unwrap();
    }
    let pass = gateway::open(
        LinkSpec {
            app_id: app.into(),
            service_id: svc.into(),
            workspace_id: ws.id.clone(),
            port: 0,
            caps: vec!["thread:read".into()],
            rooms: vec![ws.id.clone()],
        },
        Some(Arc::new(|_: &str| Ok(()))),
    )
    .expect("open link");
    (pass, gateway::port_of(app).expect("link port"))
}

async fn set_users(port: u16, token: &str, ws: &Ws, on: bool) -> Resp {
    post(port, "/cli/agent-access/set", token, json!({"workspace": ws.id, "toggle": "app_users", "value": on})).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_sets_up_a_fresh_app_and_a_test_guest_that_signs_in_with_only_its_passport() {
    let env = setup(&[]);
    quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    // A vault on a file key under the temp home (the approvals' confirms).
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "uswa");
    let other = seed_ws(&env, "uswb");
    let agent = passport(&ws);

    // Only the owner flips the switch: an agent never, an Admin login never.
    allow_build_only(&ws, true);
    let r = set_users(d.port, &agent, &ws, true).await;
    assert_ne!(r.status, 200, "an agent never turns it on: {}", r.body);
    k2_core::connect_users::add_user("usw-admin", "correct horse battery 9").unwrap();
    k2_core::connect_users::set_role("usw-admin", k2_core::connect_users::Role::Admin).unwrap();
    let admin = k2_core::connect_users::create_session("usw-admin");
    let r = set_users(d.port, &admin, &ws, true).await;
    assert_eq!(r.status, 403, "an Admin can't: {}", r.body);
    assert!(!k2_core::workspace::settings::agents_can_manage_app_users(&ws.id));
    let r = set_users(d.port, OWNER, &ws, true).await;
    assert_eq!((r.status, r.json()["agentsCanManageAppUsers"].clone()), (200, json!(true)), "{}", r.body);

    // From here on: the agent's passport only.
    let r = post(d.port, "/cli/app/install", &agent, json!({"app": "uswapp"})).await;
    assert_eq!(r.status, 200, "a fresh App: {}", r.body);
    let svc = r.json()["app"]["serviceId"].as_str().expect("serviceId").to_string();
    let r = post(d.port, "/cli/app/test-guest/add", &agent, json!({"app": "uswapp", "username": "usw-tester", "rooms": [ws.handle], "expires": "2h"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["tied"].clone(), v["rooms"].clone()), (json!(true), json!([ws.handle])), "{v}");
    assert!(v["password"].as_str().is_some_and(|p| !p.is_empty()), "shown once: {v}");
    let r = post(d.port, "/cli/skin/users/password", &agent, json!({"username": "usw-tester", "password": "agent-set-pw-123"})).await;
    assert_eq!(r.status, 200, "its own test guest's password: {}", r.body);
    let r = post(d.port, "/cli/app/grant", &agent, json!({"app": "uswapp", "kind": "guests-rooms", "rooms": [ws.handle]})).await;
    assert_eq!(r.status, 200, "the grant in its own room: {}", r.body);
    assert!(k2_core::skin::guest_rooms::get_grant(&svc).unwrap().is_some());
    let p = k2_core::skin::list_principals().unwrap().into_iter().find(|p| p.username == "usw-tester").expect("made");
    assert_eq!(k2_core::skin::creators::agent_tie(&p.id).unwrap().as_deref(), Some(ws.id.as_str()), "tied to the agent's workspace");

    // The guest signs in through the App's link with the agent's password.
    let (pass, port) = open_link("uswapp", &svc, &ws);
    let (status, body) = tokio::task::spawn_blocking(move || {
        on_link(port, "/cli/skin/login", &pass, &json!({"username": "usw-tester", "password": "agent-set-pw-123"}))
    })
    .await
    .unwrap();
    assert_eq!(status, 200, "{body}");
    let session: Value = serde_json::from_str(&body).unwrap();
    assert!(session["token"].as_str().expect("token").starts_with("k2skn_"), "{session}");
    // Never the box login.
    let r = tokio::task::spawn_blocking({
        let port = d.port;
        move || req_blocking(port, "POST", "/cli/skin/login", Some(json!({"username": "usw-tester", "password": "agent-set-pw-123"}).to_string()))
    })
    .await
    .unwrap();
    assert_eq!(r.status, 401, "never the box login: {}", r.body);

    // Never: another workspace's room (a held request, the owner's), a box
    // login, another workspace's App.
    let r = post(d.port, "/cli/app/grant", &agent, json!({"app": "uswapp", "kind": "guests-rooms", "rooms": [ws.handle, other.handle]})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "{}", r.body);
    let r = post(d.port, "/cli/app/test-guest/add", &agent, json!({"app": "uswapp", "username": "usw-wide", "rooms": [other.handle]})).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    let r = post(d.port, "/cli/skin/users", &agent, json!({"username": "usw-boxlogin"})).await;
    assert_eq!(r.status, 403, "never a box login: {}", r.body);
    let r = post(d.port, "/cli/app/test-guest/add", &passport(&other), json!({"app": "uswapp", "username": "usw-other"})).await;
    assert_eq!((r.status, r.code()), (404, "app_not_found".to_string()), "{}", r.body);
    assert!(k2_core::skin::list_principals().unwrap().iter().all(|p| !["usw-wide", "usw-boxlogin", "usw-other"].contains(&p.username.as_str())));

    // Off again (the owner): the roster answers app_users_off, a test guest
    // and the grant are one held request each.
    let r = set_users(d.port, OWNER, &ws, false).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/skin/users/password", &agent, json!({"username": "usw-tester", "password": "agent-set-pw-456"})).await;
    assert_eq!((r.status, r.code()), (403, "app_users_off".to_string()), "{}", r.body);
    let r = post(d.port, "/cli/app/test-guest/add", &agent, json!({"app": "uswapp", "username": "usw-second"})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "{}", r.body);
    assert_eq!(r.json()["kind"], "manage", "{}", r.body);
    let r = post(d.port, "/cli/app/grant/revoke", &agent, json!({"app": "uswapp", "kind": "guests-rooms"})).await;
    assert_eq!(r.status, 200, "revoking narrows: {}", r.body);
    let r = post(d.port, "/cli/app/grant", &agent, json!({"app": "uswapp", "kind": "guests-rooms"})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "{}", r.body);
    let held = r.json()["request"]["id"].as_str().expect("id").to_string();
    let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": held, "confirmToken": confirm("app-approve", &held)})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (200, json!("approved")), "{}", r.body);
    assert!(k2_core::skin::guest_rooms::get_grant(&svc).unwrap().is_some(), "the approval gives exactly the grant");
    gateway::close("uswapp", None, true);
    k2_daemon::apps::alerts::set_sink(None);
}

/// copy-host is the owner's, whatever the agent's switches: the agent's
/// call is one held request listing exactly the logins; the approval
/// copies exactly those, and a list that changed copies nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn copy_host_is_one_held_request_and_the_approval_copies_exactly_what_it_listed() {
    let env = setup(&[]);
    quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "uswc");
    allow_manage(&ws, true);
    let agent = passport(&ws);
    let r = post(d.port, "/cli/app/install", &agent, json!({"app": "uswcopy"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let svc = r.json()["app"]["serviceId"].as_str().expect("serviceId").to_string();
    for u in ["usw-box1", "usw-box2"] {
        k2_core::skin::add_principal(u).expect("add");
        k2_core::skin::set_host_login(u, true).expect("box login");
    }
    let r = post(d.port, "/cli/app/grant/copy-host", &agent, json!({"app": "uswcopy"})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "{}", r.body);
    let held = r.json()["request"]["id"].as_str().expect("id").to_string();
    // Review 0.46.4: the agent learns how many, never who, at every level.
    assert!(!r.body.contains("usw-box"), "no names in the agent's held answer: {}", r.body);
    assert!(r.json()["request"]["summary"].as_str().unwrap().contains("copy 2 box login(s)"), "{}", r.body);
    assert_eq!(r.json()["request"]["body"]["body"]["usersCount"], 2, "{}", r.body);
    // The owner's view names them.
    let r = get(d.port, "/cli/app/approvals", OWNER, "").await;
    let mine = r.json()["requests"].as_array().unwrap().iter().find(|x| x["id"] == json!(held)).cloned().expect("listed");
    let lines = mine["diff"]["lines"].to_string();
    assert!(lines.contains("usw-box1") && lines.contains("usw-box2"), "the owner sees them: {lines}");
    let r = get(d.port, "/cli/app/approvals", &agent, "").await;
    assert!(r.body.contains(&held) && !r.body.contains("usw-box"), "the agent's list has the count only: {}", r.body);
    // A dry run is a read: the agent may look, and sees counts.
    let r = post(d.port, "/cli/app/grant/copy-host", &agent, json!({"app": "uswcopy", "dryRun": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["addedCount"], 2, "{}", r.body);
    assert!(!r.body.contains("usw-box"), "no names in the agent's look: {}", r.body);
    // The list changes before the owner's click: nothing is copied.
    k2_core::skin::add_principal("usw-box3").expect("add");
    k2_core::skin::set_host_login("usw-box3", true).expect("box login");
    let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": held, "confirmToken": confirm("app-approve", &held)})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["status"], "failed", "{}", r.body);
    assert!(r.json()["error"]["hint"].as_str().unwrap_or("").contains("changed"), "{}", r.body);
    let copied = k2_core::skin::copy_host_grants_to_app_in(&svc, Some(&ws.id), "check", true).unwrap();
    assert_eq!(copied.added.len(), 3, "nothing was copied: {copied:?}");
    let r = get(d.port, "/cli/app/approvals", &agent, "state=all").await;
    assert!(r.body.contains(&held) && !r.body.contains("usw-box"), "a failed result names nobody to the agent: {}", r.body);
    // Asked again, approved: exactly the three.
    let r = post(d.port, "/cli/app/grant/copy-host", &agent, json!({"app": "uswcopy"})).await;
    let held = r.json()["request"]["id"].as_str().expect("id").to_string();
    let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": held, "confirmToken": confirm("app-approve", &held)})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (200, json!("approved")), "{}", r.body);
    let after = k2_core::skin::copy_host_grants_to_app_in(&svc, Some(&ws.id), "check", true).unwrap();
    assert!(after.added.is_empty(), "all three copied: {after:?}");
    // Review 0.46.4: a box login copied onto the App is still a box login:
    // the App's key never acts on it at the /v1 door, enabled or not.
    let grant = format!(r#"{{"scopes":["users","rooms"],"rooms":["{}"]}}"#, ws.id);
    let (_, key) = k2_core::api_keys::create_app_key("usw-v1", &svc, &grant).expect("app key");
    let host_grant = k2_core::skin::list_grants()
        .unwrap()
        .into_iter()
        .find(|g| {
            g.kind == "app"
                && g.target_id == k2_core::skin::HOST_APP_ID
                && k2_core::skin::list_principals().unwrap().iter().any(|p| p.id == g.subject_id && p.username == "usw-box2")
        })
        .expect("usw-box2's box login");
    k2_core::skin::set_grant_enabled(&host_grant.id, false).expect("disable the box login");
    for u in ["usw-box1", "usw-box2"] {
        for (verb, body) in [
            ("users/unlock", json!({"username": u})),
            ("users/rooms", json!({"username": u, "rooms": [ws.handle]})),
            ("users/enable", json!({"username": u})),
            ("users/disable", json!({"username": u})),
            ("users/remove", json!({"username": u})),
            ("users/password-reset", json!({"username": u})),
        ] {
            let r = post(d.port, &format!("/v1/apps/{svc}/{verb}"), &key, body).await;
            assert_eq!((r.status, r.code()), (403, "box_login".to_string()), "{u} {verb}: {}", r.body);
        }
    }
    assert!(k2_core::skin::list_principals().unwrap().iter().any(|p| p.username == "usw-box1"), "untouched");
    // The owner still copies at once.
    let r = post(d.port, "/cli/app/grant/copy-host", OWNER, json!({"app": svc})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}

/// Review 0.46.4: never a box login, the one password set after a plain
/// Allow, the pending cap, App keys that manage users, owner-made test
/// guests, and who resumes a suspension.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_review_lines_hold_box_logins_one_password_keys_and_suspensions() {
    let env = setup(&[]);
    quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "uswr");
    let other = seed_ws(&env, "uswr2");
    let agent = passport(&ws);
    allow_build_only(&ws, true);
    let r = post(d.port, "/cli/app/install", &agent, json!({"app": "uswrapp"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let svc = r.json()["app"]["serviceId"].as_str().expect("serviceId").to_string();

    // 1. An owner-made box login whose room is the agent's own is still a
    // box login: refused for every user verb, with the switch on.
    allow_manage(&ws, true);
    k2_core::skin::add_principal("usw-boxed").expect("add");
    k2_core::skin::set_host_login("usw-boxed", true).expect("box login");
    k2_core::skin::set_principal_default_rooms("usw-boxed", &[ws.id.clone()], true).expect("its room");
    for (path, body) in [
        ("/cli/skin/users/password", json!({"username": "usw-boxed", "password": "agent-pw-12345"})),
        ("/cli/skin/users/email", json!({"username": "usw-boxed", "email": "boxed@example.com"})),
        ("/cli/skin/users/full-name", json!({"username": "usw-boxed", "fullName": "Boxed"})),
        ("/cli/skin/users/unlock", json!({"username": "usw-boxed"})),
        ("/cli/skin/users/rooms", json!({"username": "usw-boxed", "rooms": [ws.handle]})),
        ("/cli/skin/users/remove", json!({"username": "usw-boxed"})),
        ("/cli/skin/roles/unassign", json!({"username": "usw-boxed"})),
        ("/cli/app/guest/rooms", json!({"app": "uswrapp", "username": "usw-boxed", "rooms": [ws.handle]})),
    ] {
        let r = post(d.port, path, &agent, body).await;
        assert_eq!(r.status, 403, "{path}: {}", r.body);
        assert!(r.body.contains("box login"), "{path}: {}", r.body);
    }
    assert!(k2_core::skin::list_principals().unwrap().iter().any(|p| p.username == "usw-boxed"), "untouched");

    // 2. A plain Allow gives a usable test guest: the agent sets its
    // password once, never twice, without the switch.
    allow_build_only(&ws, true);
    let r = post(d.port, "/cli/app/test-guest/add", &agent, json!({"app": "uswrapp", "username": "USW-Once"})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "{}", r.body);
    assert!(r.json()["request"]["summary"].as_str().unwrap().contains("usw-once"), "the normalized name: {}", r.body);
    assert!(r.json()["request"]["body"]["body"]["service"].as_str().is_some(), "the service id is kept: {}", r.body);
    let held = r.json()["request"]["id"].as_str().expect("id").to_string();
    let pw = |p: &'static str| json!({"username": "usw-once", "password": p});
    let r = post(d.port, "/cli/app/test-guest/password", &agent, pw("before-approval-1")).await;
    assert_eq!((r.status, r.code()), (404, "not_a_test_guest".to_string()), "nothing made yet: {}", r.body);
    let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": held, "confirmToken": confirm("app-approve", &held)})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (200, json!("approved")), "{}", r.body);
    // A refused password never spends the one set.
    let r = post(d.port, "/cli/app/test-guest/password", &agent, pw("short")).await;
    assert_eq!((r.status, r.code()), (400, "bad_password".to_string()), "{}", r.body);
    let r = post(d.port, "/cli/app/test-guest/password", &agent, pw("first-set-pw-123")).await;
    assert_eq!((r.status, r.json()["once"].clone()), (200, json!(true)), "{}", r.body);
    let r = post(d.port, "/cli/app/test-guest/password", &agent, pw("second-set-pw-123")).await;
    assert_eq!((r.status, r.code()), (403, "app_users_off".to_string()), "only once: {}", r.body);

    // A username is at most 64 characters (review 0.46.4).
    let long = "u".repeat(65);
    let r = post(d.port, "/cli/app/test-guest/add", &agent, json!({"app": "uswrapp", "username": long})).await;
    assert_eq!((r.status, r.code()), (400, "bad_request".to_string()), "{}", r.body);
    assert!(r.body.contains("at most 64"), "{}", r.body);

    // 3. At most 5 open held requests of one kind per agent.
    for n in 0..5 {
        let r = post(d.port, "/cli/app/test-guest/add", &agent, json!({"app": "uswrapp", "username": format!("usw-pend{n}")})).await;
        assert_eq!(r.status, 202, "{n}: {}", r.body);
    }
    let r = post(d.port, "/cli/app/test-guest/add", &agent, json!({"app": "uswrapp", "username": "usw-pend5"})).await;
    assert_eq!((r.status, r.code()), (429, "too_many_pending".to_string()), "{}", r.body);

    // 4. An owner-made test guest is never the agent's to remove.
    let r = post(d.port, "/cli/app/test-guest/add", OWNER, json!({"app": "uswrapp", "username": "usw-ownertg"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    allow_manage(&ws, true);
    let r = post(d.port, "/cli/app/test-guest/rm", &agent, json!({"username": "usw-ownertg"})).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    let r = post(d.port, "/cli/app/test-guest/rm", &passport(&other), json!({"username": "usw-ownertg"})).await;
    assert_eq!((r.status, r.code()), (404, "not_a_test_guest".to_string()), "{}", r.body);

    // 5. An App key that manages users: with switch 2 alone it is held;
    // with the users switch too, its own room only applies at once.
    let r = post(d.port, "/cli/agent-access/set", OWNER, json!({"workspace": ws.id, "toggle": "apps_manage_full", "value": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    k2_core::workspace::settings::set_agents_can_manage_app_users(&ws.id, false).unwrap();
    let r = post(d.port, "/cli/app/key/mint", &agent, json!({"app": "uswrapp", "scopes": ["users"], "rooms": [ws.handle]})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "users switch off: {}", r.body);
    k2_core::workspace::settings::set_agents_can_manage_app_users(&ws.id, true).unwrap();
    let r = post(d.port, "/cli/app/key/mint", &agent, json!({"app": "uswrapp", "scopes": ["users"], "rooms": [ws.handle, other.handle]})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "another room: {}", r.body);
    let r = post(d.port, "/cli/app/key/mint", &agent, json!({"app": "uswrapp", "scopes": ["users"], "rooms": [ws.handle]})).await;
    assert_eq!(r.status, 200, "its own room, both switches: {}", r.body);
    // Review 0.46.4: the key it minted itself is stamped, and stops at the
    // /v1 door while the owner has the users switch off.
    let minted = k2_core::api_keys::get_app_key(r.json()["id"].as_str().expect("key id")).expect("the key row");
    let stamped = k2_core::app_grace::keys::AppKeyGrant::from_stored(minted.grant_json.as_deref());
    assert_eq!(stamped.minted_by_agent.as_deref(), Some(ws.id.as_str()), "{stamped:?}");
    let (_, key) = k2_core::api_keys::create_app_key("usw-agent-key", &svc, &stamped.to_json()).expect("the same grant");
    let r = get(d.port, &format!("/v1/apps/{svc}/users"), &key, "").await;
    assert_eq!(r.status, 200, "switch on: {}", r.body);
    let r = post(d.port, "/cli/agent-access/set", OWNER, json!({"workspace": ws.id, "toggle": "app_users", "value": false})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = get(d.port, &format!("/v1/apps/{svc}/users"), &key, "").await;
    assert_eq!((r.status, r.code()), (403, "app_users_off".to_string()), "switch off: {}", r.body);
    let r = post(d.port, &format!("/v1/apps/{svc}/users/unlock"), &key, json!({"username": "usw-once"})).await;
    assert_eq!((r.status, r.code()), (403, "app_users_off".to_string()), "switch off: {}", r.body);
    let mut owners = stamped.clone();
    owners.minted_by_agent = None;
    let (_, owner_key) = k2_core::api_keys::create_app_key("usw-owner-key", &svc, &owners.to_json()).expect("owner's grant");
    let r = get(d.port, &format!("/v1/apps/{svc}/users"), &owner_key, "").await;
    assert_eq!(r.status, 200, "a key the owner minted doesn't follow the agent's switch: {}", r.body);
    let r = post(d.port, "/cli/agent-access/set", OWNER, json!({"workspace": ws.id, "toggle": "app_users", "value": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = get(d.port, &format!("/v1/apps/{svc}/users"), &key, "").await;
    assert_eq!(r.status, 200, "switch on again: {}", r.body);
    let r = post(d.port, "/cli/app/key/mint", &agent, json!({"app": "uswrapp", "scopes": ["fetch"]})).await;
    assert_eq!(r.status, 200, "a key that manages no users: switch 2 as before: {}", r.body);

    // 6. Suspensions: the agent resumes its own with switch 2; a person who
    // suspends it again takes it over; the owner's stays the owner's.
    let r = post(d.port, "/cli/app/suspend", &agent, json!({"app": "uswrapp"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/app/resume", &agent, json!({"app": "uswrapp"})).await;
    assert_eq!(r.status, 200, "its own suspension: {}", r.body);
    let r = post(d.port, "/cli/app/suspend", &agent, json!({"app": "uswrapp"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/app/suspend", OWNER, json!({"app": "uswrapp"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/app/resume", &agent, json!({"app": "uswrapp"})).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "the owner took it over: {}", r.body);
    let r = post(d.port, "/cli/app/resume", OWNER, json!({"app": "uswrapp"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/app/suspend", OWNER, json!({"app": "uswrapp"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/app/suspend", &agent, json!({"app": "uswrapp"})).await;
    assert_eq!(r.status, 200, "suspending again narrows: {}", r.body);
    let r = post(d.port, "/cli/app/resume", &agent, json!({"app": "uswrapp"})).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "an agent never takes over the owner's: {}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}

/// 0.46.5 item 15 (Agent NSI, 0.46.4): `k2 app user add <app> <name> --test
/// --room R --expires 24h --password-stdin` held for the owner, approved;
/// afterwards the roster showed role and rooms as "—" and login was 401.
/// The approval carries every non-secret field (room scope, role,
/// expiry) through the same add the direct path uses; the password is
/// never kept, the answer and the card say so, and the agent's one
/// password set then gives a login with the room's / role's caps.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_test_guest_keeps_its_room_role_and_expiry_and_signs_in_after_the_one_password_set() {
    let env = setup(&[]);
    quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "uswh");
    let agent = passport(&ws);
    allow_build_only(&ws, true);
    assert!(!k2_core::workspace::settings::agents_can_manage_app_users(&ws.id), "the switch is off: held");
    let r = post(d.port, "/cli/app/install", &agent, json!({"app": "uswhapp"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let svc = r.json()["app"]["serviceId"].as_str().expect("serviceId").to_string();
    // One of the App's own roles, with files in the agent's room.
    let role_name = "uswh-files";
    k2_core::skin::create_role_for_app(role_name, &Default::default(), Some(&svc)).expect("App role");
    let caps = vec!["thread:read".to_string(), "thread:post".to_string(), "files:read".to_string()];
    k2_core::skin::set_role_room(role_name, &ws.id, Some(&caps)).expect("role room");

    let held_of = |r: &Resp| -> String {
        assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "{}", r.body);
        r.json()["request"]["id"].as_str().expect("id").to_string()
    };
    let stored_body = |id: &str| -> Value {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::app_grace::requests::get(&conn, id).expect("db").expect("stored").body
    };
    let grant_of = |user: &str| -> k2_core::skin::SkinGrant {
        let p = k2_core::skin::list_principals().unwrap().into_iter().find(|p| p.username == user).unwrap_or_else(|| panic!("{user} made"));
        k2_core::skin::list_grants()
            .unwrap()
            .into_iter()
            .find(|g| g.kind == "app" && g.subject_id == p.id && g.target_id == svc)
            .unwrap_or_else(|| panic!("{user}'s App grant"))
    };

    // ── 1. --room + --expires + a password on stdin: held, password unused ──
    let before = k2_core::deploy::now();
    let r = post(
        d.port,
        "/cli/app/test-guest/add",
        &agent,
        json!({"app": "uswhapp", "username": "zz-attach", "rooms": [ws.handle], "expires": "24h", "password": "piped-pw-should-not-stay-1"}),
    )
    .await;
    let held = held_of(&r);
    let v = r.json();
    assert_eq!(
        v["afterApproval"],
        "Held for the owner. After approval, set its password once: k2 app user password zz-attach --test --password-stdin.",
        "{v}"
    );
    assert_eq!(v["passwordNotUsed"], true, "the piped password is said to be unused: {v}");
    let lines = v["request"]["diff"]["lines"].as_array().expect("card lines").iter().map(|l| l.as_str().unwrap_or("").to_string()).collect::<Vec<_>>();
    assert!(lines.iter().any(|l| l.contains("k2 app user password zz-attach --test --password-stdin")), "the card says it: {lines:?}");
    let body = stored_body(&held);
    assert!(!body.to_string().contains("piped-pw-should-not-stay-1"), "never stored: {body}");
    assert!(body["body"].get("password").is_none(), "{body}");
    assert_eq!(body["body"]["expires"], "24h", "{body}");
    assert_eq!(body["body"]["rooms"], json!([ws.id]), "{body}");

    let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": held, "confirmToken": confirm("app-approve", &held)})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (200, json!("approved")), "{}", r.body);
    let g = grant_of("zz-attach");
    assert!(g.role_id.is_none(), "rooms, no role: {g:?}");
    assert_eq!(g.scope.as_deref().map(|s| serde_json::from_str::<Vec<String>>(s).unwrap()), Some(vec![ws.id.clone()]), "the room scope: {g:?}");
    let t = k2_core::skin::test_guests::list(Some(&svc)).unwrap();
    let p = k2_core::skin::list_principals().unwrap().into_iter().find(|p| p.username == "zz-attach").unwrap();
    let tg = t.iter().find(|t| t.principal_id == p.id).expect("a test guest");
    assert!((tg.expires_at - (before + 24 * 3600)).abs() < 300, "24 h from the ask: {}", tg.expires_at - before);
    // The roster and the test-guest list show the App grant's rooms.
    let r = get(d.port, "/cli/skin/users", OWNER, "").await;
    let row = r.json()["users"].as_array().unwrap().iter().find(|u| u["username"] == "zz-attach").cloned().expect("listed");
    assert_eq!(row["appGrants"][0]["rooms"], json!([ws.handle]), "{row}");
    let r = get(d.port, "/cli/app/test-guests", OWNER, "app=uswhapp").await;
    let tgl = r.json()["testGuests"].as_array().unwrap().iter().find(|u| u["username"] == "zz-attach").cloned().expect("listed");
    assert_eq!(tgl["rooms"], json!([ws.handle]), "{tgl}");

    // No password yet: login refused; the one set; login gets the room.
    let (pass, port) = open_link("uswhapp", &svc, &ws);
    let login = |pw: &'static str, user: &'static str| {
        let pass = pass.clone();
        tokio::task::spawn_blocking(move || on_link(port, "/cli/skin/login", &pass, &json!({"username": user, "password": pw})))
    };
    let (status, body) = login("piped-pw-should-not-stay-1", "zz-attach").await.unwrap();
    assert_eq!(status, 401, "the held password was never set: {body}");
    let r = post(d.port, "/cli/app/test-guest/password", &agent, json!({"username": "zz-attach", "password": "attach-set-pw-123"})).await;
    assert_eq!((r.status, r.json()["once"].clone()), (200, json!(true)), "{}", r.body);
    let (status, body) = login("attach-set-pw-123", "zz-attach").await.unwrap();
    assert_eq!(status, 200, "{body}");
    let lv: Value = serde_json::from_str(&body).unwrap();
    let access = lv["roomAccess"].as_array().unwrap_or_else(|| panic!("roomAccess: {lv}"));
    assert!(access.iter().any(|a| a["handle"] == ws.handle.as_str()), "the room: {lv}");

    // ── 2. --role + --expires: the role rides the approval ──
    let r = post(d.port, "/cli/app/test-guest/add", &agent, json!({"app": "uswhapp", "username": "zz-roled", "role": role_name, "expires": "2h"})).await;
    let held = held_of(&r);
    assert!(r.json().get("passwordNotUsed").is_none(), "no password sent, no note: {}", r.body);
    let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": held, "confirmToken": confirm("app-approve", &held)})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (200, json!("approved")), "{}", r.body);
    let g = grant_of("zz-roled");
    let role = k2_core::skin::list_roles().unwrap().into_iter().find(|r| r.name == role_name).expect("role");
    assert_eq!(g.role_id.as_deref(), Some(role.id.as_str()), "the role: {g:?}");
    let r = get(d.port, "/cli/skin/users", OWNER, "").await;
    let row = r.json()["users"].as_array().unwrap().iter().find(|u| u["username"] == "zz-roled").cloned().expect("listed");
    assert_eq!(row["appGrants"][0]["role"], role_name, "{row}");
    let r = post(d.port, "/cli/app/test-guest/password", &agent, json!({"username": "zz-roled", "password": "roled-set-pw-123"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let (status, body) = login("roled-set-pw-123", "zz-roled").await.unwrap();
    assert_eq!(status, 200, "{body}");
    let lv: Value = serde_json::from_str(&body).unwrap();
    let access = lv["roomAccess"].as_array().unwrap_or_else(|| panic!("roomAccess: {lv}"));
    assert!(
        access.iter().any(|a| a["handle"] == ws.handle.as_str() && a["caps"].as_array().is_some_and(|c| c.iter().any(|x| x == "files:read"))),
        "the role's caps: {lv}"
    );
    gateway::close("uswhapp", None, true);
    k2_daemon::apps::alerts::set_sink(None);
}
