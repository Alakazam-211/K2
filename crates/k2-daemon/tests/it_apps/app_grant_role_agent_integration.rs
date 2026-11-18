//! `k2 app grant role <grant-id> --role <role>` by an App's own agent
//! (0.46.16, Rosson 2026-11-18), headless: the real dispatcher in process on
//! a temp `$HOME`, over loopback.
//!
//! - with "Let this agent manage its Apps' users" it moves a login of its OWN
//!   App onto another role of that App in place: the same grant, the person's
//!   live session kept and on the new role (no card);
//! - without the switch it is refused `app_users_off`, and with permission
//!   cards on that refusal ends with the Ask-with line for the switch (not
//!   "It can't be requested."); nothing changes;
//! - another workspace's App's login, the box login, an agent's login and a
//!   role of another App are still refused with the switch on (another
//!   workspace's login: not requestable);
//! - the owner's own `k2 app grant role` is as before.
//!
//! Never the real `~/.k2`, never the keychain.

use crate::support::app_fixture;

use app_fixture::*;
use serde_json::{json, Value};

const ROLE: &str = "/cli/skin/grants/role";

struct SwitchOff;
impl Drop for SwitchOff {
    fn drop(&mut self) {
        k2_core::permission::gate::set_server_switch(false);
    }
}

fn tag() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

fn service(ws: &Ws, name: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::insert(&conn, &ws.id, name, "node s.mjs", &ws.path.to_string_lossy(), 47_611, "local", "stopped", "cmd", "")
        .expect("service")
        .id
}

/// An App role reading and posting in `ws`'s Thread (`files` adds files:read).
fn role(ws: &Ws, app: &str, name: &str, files: bool) -> String {
    let mut caps = vec!["thread:read".to_string(), "thread:post".to_string()];
    if files {
        caps.push("files:read".to_string());
    }
    let mut p = k2_core::skin::RoomPolicy::new();
    p.insert(ws.id.clone(), caps);
    k2_core::skin::create_role_for_app(name, &p, Some(app)).expect("role").id
}

/// `username`'s login on `app`.
fn login(username: &str, app: &str) -> k2_core::skin::SkinGrant {
    let p = k2_core::skin::list_principals().expect("list").into_iter().find(|p| p.username == username).expect("person");
    k2_core::skin::list_grants()
        .expect("grants")
        .into_iter()
        .find(|g| g.subject_kind == "principal" && g.subject_id == p.id && g.kind == "app" && g.target_id == app)
        .unwrap_or_else(|| panic!("{username} has no login on {app}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn its_own_apps_login_moves_role_in_place_with_the_switch_and_asks_for_it_without() {
    let env = setup(&[]);
    let _off = SwitchOff;
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let ws = seed_ws(&env, &format!("grsales{t}"));
    let other = seed_ws(&env, &format!("grops{t}"));
    let jobs = service(&ws, &format!("grjobs{t}"));
    let theirs = service(&other, &format!("grtheirs{t}"));
    let viewer = role(&ws, &jobs, &format!("viewer{t}"), false);
    let editor = role(&ws, &jobs, &format!("editor{t}"), true);
    let their_role = role(&other, &theirs, &format!("theirrole{t}"), false);
    let finn = format!("finn{t}");
    k2_core::skin::add_principal_for_app(&finn, None, &jobs, Some(&viewer), "owner-token").expect("a login on its own App");
    let erin = format!("erin{t}");
    k2_core::skin::add_principal_for_app(&erin, None, &theirs, Some(&their_role), "owner-token").expect("another workspace's login");
    let before = login(&finn, &jobs);
    assert_eq!(before.role_id.as_deref(), Some(viewer.as_str()));
    let session = k2_core::skin::create_app_session_token(&finn, &jobs).expect("finn signed in").raw;
    let agent = passport(&ws);
    let move_finn = json!({ "id": before.id, "role": editor });

    // Without the users switch: refused, nothing moves; with cards on, the
    // refusal asks for the switch.
    allow_build_only(&ws, true);
    let r = post(port, ROLE, &agent, move_finn.clone()).await;
    assert_eq!((r.status, r.code()), (403, "app_users_off".to_string()), "{}", r.body);
    assert!(!r.body.contains("\"permission\""), "cards off: today's refusal: {}", r.body);
    k2_core::permission::gate::set_server_switch(true);
    k2_core::workspace::settings::set_agents_can_request_permissions(&ws.id, true).expect("agent switch");
    let r = post(port, ROLE, &agent, move_finn.clone()).await;
    assert_eq!((r.status, r.code()), (403, "app_users_off".to_string()), "{}", r.body);
    let v: Value = r.json();
    assert_eq!(v["permission"]["action"], "apps.manage-users", "the Ask-with line, not 'It can't be requested.': {v}");
    assert!(v["permission"]["command"].as_str().is_some_and(|c| c.starts_with("k2 permission request apps.manage-users")), "{v}");
    assert_eq!(login(&finn, &jobs).role_id.as_deref(), Some(viewer.as_str()), "nothing changed");

    // With the switch: in place, the session kept and on the new role.
    allow_manage(&ws, true);
    let r = post(port, ROLE, &agent, move_finn.clone()).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["changed"], json!(true), "{}", r.body);
    let after = login(&finn, &jobs);
    assert_eq!(after.id, before.id, "the same login, not remove + add");
    assert_eq!(after.role_id.as_deref(), Some(editor.as_str()));
    let pass = k2_core::skin::resolve_skin_token(&session).expect("finn's live session is kept");
    assert!(pass.has_cap_in_room(&ws.id, "files:read"), "the live session has the new role now: {pass:?}");
    // Again: nothing to change.
    let r = post(port, ROLE, &agent, move_finn.clone()).await;
    assert_eq!((r.status, r.json()["changed"].clone()), (200, json!(false)), "{}", r.body);

    // Another App's role on its own App's login: refused, nothing moves.
    let r = post(port, ROLE, &agent, json!({ "id": before.id, "role": their_role })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(login(&finn, &jobs).role_id.as_deref(), Some(editor.as_str()));

    // Another workspace's App's login: still refused, and no card opens it.
    let theirs_login = login(&erin, &theirs);
    let r = post(port, ROLE, &agent, json!({ "id": theirs_login.id, "role": their_role })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    assert_eq!(r.json()["permission"]["requestable"], json!(false), "not requestable: {}", r.body);

    // A box login on its own App stays the owner's.
    let boxed = format!("boxed{t}");
    let bp = k2_core::skin::add_principal_for_app(&boxed, None, &jobs, Some(&viewer), "owner-token").expect("boxed");
    k2_core::skin::create_grant(&k2_core::skin::SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: bp.id.clone(),
        kind: "app".into(),
        target_id: k2_core::skin::HOST_APP_ID.into(),
        role: None,
        scope: Some("[]".into()),
        created_by: Some("owner-token".into()),
    })
    .expect("box login");
    let r = post(port, ROLE, &agent, json!({ "id": login(&boxed, &jobs).id, "role": editor })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(login(&boxed, &jobs).role_id.as_deref(), Some(viewer.as_str()), "the box login's App role is the owner's");

    // An agent's own login on its App: never.
    let bot = format!("bot{t}");
    k2_core::skin::add_principal_for_app(&bot, None, &jobs, Some(&viewer), "owner-token").expect("bot");
    k2_core::skin::set_principal_agent(&bot, Some(&ws.handle)).expect("agent mark");
    let r = post(port, ROLE, &agent, json!({ "id": login(&bot, &jobs).id, "role": editor })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(login(&bot, &jobs).role_id.as_deref(), Some(viewer.as_str()));

    // The owner, as before.
    let r = post(port, ROLE, OWNER, json!({ "id": before.id, "role": viewer })).await;
    assert_eq!((r.status, r.json()["changed"].clone()), (200, json!(true)), "{}", r.body);
    let r = post(port, ROLE, OWNER, json!({ "id": theirs_login.id, "role": their_role })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    drop(env);
}
