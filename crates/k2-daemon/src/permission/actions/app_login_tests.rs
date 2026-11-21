//! `app.login-grant` end to end: an App's own agent asks to give a person a
//! login on its App (or take one away), the owner taps Allow, and the card
//! runs the grants route's own handler as the owner. Test DB, temp HOME; no
//! real agent is woken (turns go to `inject`'s sink).

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Floor, Grant, Proof, Undo};
use k2_core::skin::{self, RoomPolicy, SkinGrantWrite};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, ask, ask_ok, below_floor, card_of, id_of, last_turn, login, row, seed, view, Ws};
use super::*;

fn uniq() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

fn app_of(ws: &Ws, name: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::insert(&conn, &ws.id, name, "node s.js", "/tmp", 47_501, "local", "stopped", "cmd", "").expect("App").id
}

/// A person on the box: an App user of `app`.
fn person(app: &str, name: &str) -> String {
    let u = format!("{name}{}", uniq());
    skin::add_principal_for_app(&u, None, app, None, "owner-token").expect("person");
    u
}

/// A role that reads and posts in `ws`'s Thread.
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

fn add(user: &str, app: &str, role: &str) -> Value {
    json!({ "op": "add", "user": user, "app": app, "role": role })
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
fn the_card_is_registered_with_the_routes_floor_and_a_click() {
    let _db = k2_core::db::scoped_for_test();
    let r = super::super::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant), (Floor::Owner, Proof::Click, Grant::OneTime));
    assert!(matches!(r.spec.undo, Undo::None));
    for route in [ROUTE_ADD, ROUTE_REMOVE] {
        let floor = crate::routes::route_policy::lookup(route).and_then(|x| x.post).expect("a POST row");
        assert!(!floor.admits(Role::Admin), "{route}: the card's floor is the route's own (the owner)");
    }
}

#[test]
fn the_owner_allows_a_login_once_and_the_card_says_what_the_role_allows() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let ws = seed("alg");
    let app_name = format!("jobs{}", uniq());
    let app = app_of(&ws, &app_name);
    // A person already on the box, through another App.
    let dana = person(&app_of(&seed("alo"), &format!("elsewhere{}", uniq())), "dana");
    let viewer = role(&ws, None);

    let asked = ask_ok(&ws, ACTION, add(&dana, &app_name, &viewer));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], format!("{dana}'s login on {app_name}"), "{vw}");
    assert_eq!(vw["whatChanges"], format!("Let {} give {dana} a login on its App {app_name}, with the role {viewer}.", ws.handle), "{vw}");
    let effects = vw["consequences"].as_str().unwrap_or("");
    assert!(effects.contains(&format!("With the role {viewer}, {dana} can in {}: read the Thread and post in the Thread.", ws.handle)), "{effects}");
    assert!(effects.contains(&format!("{app_name}'s server can see it")), "the password goes through the App: {effects}");

    // The agent never answers; an Admin is below the route's floor.
    let (s, a) = as_agent(&ws, &card);
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("action_card_person_only")), "{a}");
    let admin = login("alga", Role::Admin);
    below_floor(answer_as(&admin, &ws, &card, json!({ "answer": "Allow" })), "owner");
    let who = Answerer::Login { user: admin, role: "admin".into() };
    assert!(matches!((LOGIN_GRANT_APPLY)(&present_ctx(&id), &who, None), Applied::Refused { .. }), "the route's own floor at the click");
    assert!(login_of(&dana, &app).is_none(), "nothing changed");

    // The owner: the route runs as the owner.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    let g = login_of(&dana, &app).expect("the login");
    let role_id = skin::list_roles().unwrap().into_iter().find(|r| r.name == viewer).unwrap().id;
    assert_eq!((g.role_id.as_deref(), g.created_by.as_deref()), (Some(role_id.as_str()), Some("owner-token")));
    assert!(last_turn(&ws).contains(&format!("{dana} has a login on {app_name} with the role {viewer} now.")), "{}", last_turn(&ws));

    // Once: a second answer applies nothing; asking again is already done.
    let (s, _) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "409 Conflict");
    let (s, again) = ask(&ws, ACTION, add(&dana, &app_name, &viewer));
    assert_eq!((s.as_str(), &again["status"]), ("200 OK", &json!("already_done")), "{again}");
    assert_eq!(row(&id).state, k2_core::permission::request::RequestState::Approved);

    // Take it away again, on a card.
    let asked = ask_ok(&ws, ACTION, json!({ "op": "remove", "user": dana, "app": app_name }));
    let card = card_of(&asked);
    let vw = view(&ws, &card);
    assert_eq!(vw["whatChanges"], format!("Let {} remove {dana}'s login on its App {app_name}.", ws.handle), "{vw}");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert!(login_of(&dana, &app).is_none(), "removed");
}

fn present_ctx(id: &str) -> Ctx {
    crate::permission::present::ctx_of(&row(id))
}

const LOGIN_GRANT_APPLY: fn(&Ctx, &Answerer, Option<&str>) -> Applied = apply;

#[test]
fn only_its_own_app_existing_people_real_roles_and_never_an_agent_or_a_box_login() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (ws, other) = (seed("alh"), seed("ali"));
    let mine = format!("mine{}", uniq());
    let mine_id = app_of(&ws, &mine);
    let theirs = format!("theirs{}", uniq());
    let theirs_id = app_of(&other, &theirs);
    let dana = person(&mine_id, "dana");
    let viewer = role(&ws, None);
    let refused = |params: Value, says: &str| {
        let (s, e) = ask(&ws, ACTION, params.clone());
        assert!(s.starts_with("400"), "{params}: {s}: {e}");
        assert!(e.to_string().contains(says), "{params}: {e}");
    };

    // Another workspace's App, by name or by id.
    refused(add(&dana, &theirs, &viewer), "has no App named");
    refused(add(&dana, &theirs_id, &viewer), "has no App named");
    // Nobody by that name; a role that doesn't exist or is another App's.
    refused(add("nobody-here", &mine, &viewer), "no App user named nobody-here");
    refused(add(&dana, &mine, "no-such-role"), "no role named no-such-role");
    let other_role = role(&ws, Some(&theirs_id));
    refused(add(&dana, &mine, &other_role), "belongs to another App");
    refused(json!({ "op": "add", "user": dana, "app": mine }), "name the role");
    refused(json!({ "op": "remove", "user": dana, "app": mine, "role": viewer }), "role= goes with op=add");

    // An agent's login: never, whichever agent.
    let bot = person(&mine_id, "bot");
    skin::set_principal_agent(&bot, Some(&ws.handle)).expect("agent mark");
    refused(add(&bot, &mine, &viewer), "is an agent's login");
    // A box login stays the owner's.
    let boxed = person(&mine_id, "boxed");
    let p = skin::list_principals().unwrap().into_iter().find(|p| p.username == boxed).unwrap();
    skin::create_grant(&SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: p.id,
        kind: "app".into(),
        target_id: skin::HOST_APP_ID.into(),
        role: None,
        scope: Some("[]".into()),
        created_by: Some("owner-token".into()),
    })
    .expect("box login");
    refused(add(&boxed, &mine, &viewer), "has a box login");
    assert!(login_of(&dana, &theirs_id).is_none());
}

/// `k2 app grant list` for an agent: its own workspace's Apps only.
#[test]
fn an_agent_lists_the_logins_on_its_own_apps_only() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (ws, other) = (seed("alj"), seed("alk"));
    let mine = app_of(&ws, &format!("mine{}", uniq()));
    let theirs = app_of(&other, &format!("theirs{}", uniq()));
    let dana = person(&mine, "dana");
    let erin = person(&theirs, "erin");
    let r = crate::skin_routes::handle_grants_own(&ws.id);
    assert!(r.status.starts_with("200"), "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).expect("json");
    let rows = v["grants"].as_array().expect("rows");
    assert!(rows.iter().any(|g| g["username"] == dana.as_str() && g["targetId"] == mine.as_str()), "{v}");
    assert!(rows.iter().all(|g| g["targetId"] == mine.as_str()), "only its own Apps: {v}");
    assert!(!r.body.contains(&erin) && !r.body.contains(&theirs), "nothing of another workspace's: {}", r.body);
}

/// 0.46.16: `op=role` moves a login its App already has onto another role IN
/// PLACE (the `k2 app grant role` path): the same grant, its live session
/// kept and on the new role. The card names the old role and the new one and
/// how to put it back; the owner allows it once. No login: ask with op=add.
#[test]
fn op_role_moves_its_apps_login_in_place_and_keeps_the_session() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let ws = seed("alr");
    let app_name = format!("jobs{}", uniq());
    let app = app_of(&ws, &app_name);
    let viewer = role(&ws, Some(&app));
    let editor = role(&ws, Some(&app));
    let dana = format!("dana{}", uniq());
    skin::add_principal_for_app(&dana, None, &app, Some(&viewer), "owner-token").expect("dana");
    let before = login_of(&dana, &app).expect("the login");
    let session = skin::create_app_session_token(&dana, &app).expect("signed in").raw;
    let to = |r: &str| json!({ "op": "role", "user": dana, "app": app_name, "role": r });

    let asked = ask_ok(&ws, ACTION, to(&editor));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let vw = view(&ws, &card);
    assert_eq!(
        vw["whatChanges"],
        format!("Let {} move {dana}'s login on its App {app_name} from the role {viewer} to the role {editor}.", ws.handle),
        "{vw}"
    );
    let effects = vw["consequences"].as_str().unwrap_or("");
    assert!(effects.contains("so do their open sessions"), "{effects}");
    assert!(effects.contains(&format!("Put it back with k2 app grant role {} --role {viewer}.", before.id)), "{effects}");

    // Below the floor: an Admin; nothing moves.
    let admin = login("alra", Role::Admin);
    below_floor(answer_as(&admin, &ws, &card, json!({ "answer": "Allow" })), "owner");
    assert_eq!(login_of(&dana, &app).unwrap().role_id, before.role_id);

    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    let after = login_of(&dana, &app).expect("still a login");
    let editor_id = skin::list_roles().unwrap().into_iter().find(|r| r.name == editor).unwrap().id;
    assert_eq!(after.id, before.id, "in place, not remove + add");
    assert_eq!(after.role_id.as_deref(), Some(editor_id.as_str()));
    assert!(skin::resolve_skin_token(&session).is_some(), "the live session is kept");
    assert!(last_turn(&ws).contains("their sessions stayed"), "{}", last_turn(&ws));
    assert_eq!(row(&id).state, k2_core::permission::request::RequestState::Approved);

    // The same role again: already done. No login to move: ask with op=add.
    let (s, again) = ask(&ws, ACTION, to(&editor));
    assert_eq!((s.as_str(), &again["status"]), ("200 OK", &json!("already_done")), "{again}");
    let erin = format!("erin{}", uniq());
    skin::add_principal_for_app(&erin, None, &app_of(&seed("als"), &format!("other{}", uniq())), None, "owner-token").expect("erin");
    let (s, e) = ask(&ws, ACTION, json!({ "op": "role", "user": erin, "app": app_name, "role": editor }));
    assert!(s.starts_with("400") && e.to_string().contains("ask with op=add"), "{s}: {e}");
    // op=add on a login that exists points at op=role.
    let (s, e) = ask(&ws, ACTION, add(&dana, &app_name, &viewer));
    assert!(s.starts_with("400") && e.to_string().contains("op=role"), "{s}: {e}");
    // op=role needs a role.
    let (s, e) = ask(&ws, ACTION, json!({ "op": "role", "user": dana, "app": app_name }));
    assert!(s.starts_with("400") && e.to_string().contains("name the role"), "{s}: {e}");
}

/// 0.46.17 (field case): `k2 permission request app.login-grant … --assign
/// <login>` (the ask's `to`, as on every card) asks only that person. The
/// card's floor is the owner's, so an Admin login can't be named, and
/// nothing is asked when one is.
#[test]
fn assign_sends_the_login_card_to_one_owner_only() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let ws = seed("ala");
    let app_name = format!("jobs{}", uniq());
    app_of(&ws, &app_name);
    let dana = person(&app_of(&seed("alao"), &format!("elsewhere{}", uniq())), "dana");
    let viewer = role(&ws, None);
    let olga = login("alao", Role::Owner);
    let ada = login("alaa", Role::Admin);
    let ask_to = |to: &str| {
        let body = json!({ "action": ACTION, "params": add(&dana, &app_name, &viewer), "why": "Dana runs the jobs board", "to": to });
        let r = crate::permission::routes::handle_gated(
            crate::permission::routes::REQUEST,
            &super::super::held::tests::agent(&ws),
            true,
            body.to_string().as_bytes(),
            &HashMap::new(),
        );
        (r.status.to_string(), serde_json::from_str::<Value>(&r.body).expect("json"))
    };
    let (s, e) = ask_to(&ada);
    assert_eq!((s.as_str(), e["error"]["code"].as_str()), ("400 Bad Request", Some("approver_not_eligible")), "{e}");
    assert!(crate::permission::store::open(Some(&ws.id)).expect("open").is_empty(), "nothing asked");
    let (s, v) = ask_to(&olga);
    assert_eq!((s.as_str(), v["status"].as_str()), ("200 OK", Some("pending")), "{v}");
    let q = row(&id_of(&v));
    assert_eq!(crate::permission::approvers::of(q.id.as_str()), Some(vec![olga.clone()]), "only olga is asked");
}
