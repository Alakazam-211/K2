//! `apps.user-grant-box` end to end: an App's own agent asks to give a box
//! login access to its App, nobody but the owner can allow it, and Allow
//! runs the grants route's own handler as the owner. Test DB, temp HOME; no
//! real agent is woken.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Direction, Floor, Grant, Proof, Undo, WallCoverage};
use k2_core::skin::{self, RoomPolicy};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, ask_ok, ask_refused, below_floor, card_of, id_of, last_turn, login, row, seed, view, Ws};
use super::*;

fn uniq() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

fn app_of(ws: &Ws, name: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::insert(&conn, &ws.id, name, "node s.js", "/tmp", 47_511, "local", "stopped", "cmd", "").expect("App").id
}

/// A box login: a principal with the `host` grant.
fn box_login(name: &str) -> String {
    let u = format!("{name}{}", uniq());
    skin::add_principal(&u).expect("box login");
    let p = skin::list_principals().unwrap().into_iter().find(|p| p.username == u).expect("person");
    let grants = skin::list_grants().unwrap();
    assert!(crate::skin_routes::has_box_login(&p.id, &grants), "add_principal gives the box login");
    u
}

fn role(ws: &Ws, app: Option<&str>) -> String {
    let name = format!("viewer{}", uniq());
    let mut p = RoomPolicy::new();
    p.insert(ws.id.clone(), vec!["thread:read".into(), "thread:post".into()]);
    skin::create_role_for_app(&name, &p, app).expect("role");
    name
}

fn login_of(user: &str, app: &str) -> Option<skin::SkinGrant> {
    let p = skin::list_principals().unwrap().into_iter().find(|p| p.username == user).expect("person");
    skin::list_grants().unwrap().into_iter().find(|g| g.subject_id == p.id && g.kind == "app" && g.target_id == app)
}

fn host_of(user: &str) -> Option<skin::SkinGrant> {
    login_of(user, skin::HOST_APP_ID)
}

/// An agent's own answer: the passport stamps these on the call.
fn as_agent(ws: &Ws, card: &str) -> (String, Value) {
    let params: HashMap<String, String> = [
        (crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string()),
        ("project_id".to_string(), ws.id.clone()),
        ("from".to_string(), ws.handle.clone()),
    ]
    .into();
    let b = json!({ "addr": ws.handle, "id": card, "answer": "Allow" });
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &params, b.to_string().as_bytes(), "owner");
    (r.status.to_string(), serde_json::from_str(&r.body).expect("json"))
}

#[test]
fn the_card_is_registered_at_the_owners_floor_and_reached_by_asking() {
    let r = super::super::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant, r.spec.direction), (Floor::Owner, Proof::Click, Grant::OneTime, Direction::Loosen));
    assert!(matches!(r.spec.undo, Undo::None));
    assert_eq!(r.spec.coverage, WallCoverage::None);
    assert_eq!(
        crate::routes::route_policy::lookup(ROUTE_ADD).and_then(|x| x.post),
        Some(crate::routes::route_policy::Floor::Owner),
        "{ROUTE_ADD}: the card's floor is the route's"
    );
    assert_eq!(crate::routes::route_policy::lookup(ROUTE_ROLE).and_then(|x| x.post), Some(crate::routes::route_policy::Floor::Owner));
}

#[test]
fn the_owner_allows_it_from_the_thread_and_the_grant_is_the_effect() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let me = seed("bxa");
    let svc = format!("jobs{}", uniq());
    let app = app_of(&me, &svc);
    let u = box_login("bo");
    let r = role(&me, Some(&app));
    let host_before = host_of(&u).expect("the box login");

    let asked = ask_ok(&me, ACTION, json!({ "user": u, "app": svc, "role": r }));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let vw = view(&me, &card);
    assert_eq!(vw["setting"], format!("{u}'s access to {svc}"), "{vw}");
    let what = vw["whatChanges"].as_str().unwrap_or("");
    assert!(what.contains(&u) && what.contains(&svc) && what.contains(&r), "who, where, which role: {what}");
    let effects = vw["consequences"].as_str().unwrap_or("");
    assert!(effects.contains("read the Thread"), "what the role allows: {effects}");
    assert!(effects.contains(&format!("{} never sees it", me.handle)), "{effects}");
    assert!(login_of(&u, &app).is_none(), "nothing happens at the ask");

    // Nobody below the owner: an agent, its own card included; an Admin.
    let (s, a) = as_agent(&me, &card);
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("action_card_person_only")), "{a}");
    assert!(login_of(&u, &app).is_none(), "an agent's answer does nothing");
    let admin = login("bxadmin", Role::Admin);
    below_floor(answer_as(&admin, &me, &card, json!({ "answer": "Allow" })), "owner");
    let who = Answerer::Login { user: admin, role: "admin".into() };
    let ctx = crate::permission::present::ctx_of(&row(&id));
    assert!(matches!(apply(&ctx, &who, None), Applied::Refused { .. }), "the route's own floor at the click");
    assert!(login_of(&u, &app).is_none(), "an Admin's answer does nothing");

    // The owner's Allow: the App grant, with that role; the box login as it was.
    let (s, a) = answer_as("owner", &me, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    let g = login_of(&u, &app).expect("the App grant");
    let want = skin::find_role(&r).unwrap().expect("role").id;
    assert_eq!(g.role_id.as_deref(), Some(want.as_str()));
    assert_eq!(host_of(&u).as_ref(), Some(&host_before), "the box login is untouched");
    assert!(last_turn(&me).contains("approved"), "the agent hears: {}", last_turn(&me));

    // A second role: the same card changes it in place.
    let r2 = role(&me, Some(&app));
    let asked = ask_ok(&me, ACTION, json!({ "user": u, "app": svc, "role": r2 }));
    let (s, a) = answer_as("owner", &me, &card_of(&asked), json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    let want2 = skin::find_role(&r2).unwrap().expect("role").id;
    let g2 = login_of(&u, &app).expect("the App grant");
    assert_eq!((g2.id.as_str(), g2.role_id.as_deref()), (g.id.as_str(), Some(want2.as_str())), "in place");
}

#[test]
fn the_ask_is_refused_for_anyone_but_a_box_login_on_its_own_app() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (me, other) = (seed("bxb"), seed("bxc"));
    let svc = format!("jobs{}", uniq());
    let app = app_of(&me, &svc);
    let theirs = format!("crm{}", uniq());
    let their_app = app_of(&other, &theirs);
    let r = role(&me, Some(&app));
    let u = box_login("cy");

    // Another workspace's App.
    let why = ask_refused(&me, ACTION, json!({ "user": u, "app": theirs, "role": r }));
    assert!(why.contains(&format!("no App named {theirs}")), "{why}");
    assert!(login_of(&u, &their_app).is_none());
    // An App-only person: the App-user path, named.
    let only = format!("dee{}", uniq());
    skin::add_principal_for_app(&only, None, &app, None, "owner-token").expect("App user");
    let why = ask_refused(&me, ACTION, json!({ "user": only, "app": svc, "role": r }));
    assert!(why.contains("app.login-grant"), "names the App-user path: {why}");
    // Another App's role.
    let their_role = role(&other, Some(&their_app));
    let why = ask_refused(&me, ACTION, json!({ "user": u, "app": svc, "role": their_role }));
    assert!(why.contains("belongs to another App"), "{why}");
    // An agent's login: never.
    let bot = box_login("bot");
    skin::set_principal_agent(&bot, Some(&me.handle)).expect("mark");
    let why = ask_refused(&me, ACTION, json!({ "user": bot, "app": svc, "role": r }));
    assert!(why.contains("agent's login"), "{why}");
    // And app.login-grant names this card for a box login.
    let why = ask_refused(&me, super::super::app_login::ACTION, json!({ "op": "add", "user": u, "app": svc, "role": r }));
    assert!(why.contains(ACTION), "names this card: {why}");
}
