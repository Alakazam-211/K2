//! 0.46.17: an agent's test login into the rooms its role names, on an
//! owner's card (`apps.test-guest-rooms`). Test DB, temp HOME, the real skin
//! store; no real agent is woken (turns go to `inject`'s sink).

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Floor, Proof, WallCoverage};
use k2_core::permission::request::RequestState;
use k2_core::skin::{self, test_guests as tg};
use serde_json::{json, Value};

use super::test_guest_rooms::{self as tgr, ROOMS_OK};
use super::test_guests::{add_checked, guest_app_for, NewTestGuest, NOT_A_TEST_GUEST};
use crate::compute_routes::Who;
use crate::permission::actions::held::tests::{agent, answer_as, below_floor, card_of, id_of, login, row, seed, view, Ws};

struct Fx {
    a: Ws,
    b: Ws,
    c: Ws,
    app_id: String,
    svc: String,
    role: String,
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
}

fn uniq() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

/// Workspace `a` (the agent) with an App, a role of that App naming `a`'s and
/// `b`'s rooms, and a third workspace `c`.
fn fx(tag: &str) -> Fx {
    let db = k2_core::db::scoped_for_test();
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (a, b, c) = (seed(&format!("{tag}a")), seed(&format!("{tag}b")), seed(&format!("{tag}c")));
    let app_id = format!("qa{}", uniq());
    let svc = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let s = k2_core::published_services::insert(&conn, &a.id, &app_id, "node s.js", "/tmp", 47_611, "local", "running", "cmd", "").expect("svc");
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp {
                app_id: &app_id,
                service_id: &s.id,
                seed_id: None,
                owner_workspace: &a.id,
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "test",
            },
        )
        .expect("runtime app");
        s.id
    };
    let role = format!("tester{}", uniq());
    skin::create_role_for_app(&role, &policy(&[(&a.id, &["thread:read", "thread:post"]), (&b.id, &["thread:read", "thread:post"])]), Some(&svc))
        .expect("role");
    Fx { a, b, c, app_id, svc, role, _db: db, _home: home }
}

fn policy(lines: &[(&str, &[&str])]) -> skin::RoomPolicy {
    lines.iter().map(|(r, caps)| (r.to_string(), caps.iter().map(|c| c.to_string()).collect())).collect()
}

/// The agent's own `k2 app user add <app> <login> --test --role R`.
fn agent_test_login(f: &Fx, role: &str) -> (String, Value) {
    let app = guest_app_for(&agent(&f.a), Some(&f.app_id)).expect("app");
    let user = format!("qa{}", uniq());
    let out = add_checked(
        &app,
        &format!("agent:{}", f.a.handle),
        Some(&f.a.id),
        &NewTestGuest { username: &user, role: Some(role), expires: Some("7d"), rooms: &[], password: Some("pw-test-only-1") },
    )
    .unwrap_or_else(|e| panic!("add: {} {}", e.code, e.hint));
    (user, out)
}

/// The rooms a sign-in through the App's link gets now.
fn rooms_now(user: &str, svc: &str) -> Vec<String> {
    let raw = tg::with_link_login(Some(svc), || skin::create_app_session_token(user, svc)).expect("session").raw;
    let mut rooms = skin::resolve_skin_token(&raw).expect("live").rooms;
    rooms.sort();
    rooms
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn rooms_ok(who: &Who, app: &str, user: &str) -> Result<Value, (String, u16, String)> {
    tgr::handle(who, json!({ "app": app, "username": user }).to_string().as_bytes(), &HashMap::new()).map_err(|e| (e.code.to_string(), e.status, e.hint))
}

/// The card's request, as the agent filed it with these values (a stale card).
fn ask_with(ws: &Ws, params: Value) -> Value {
    let (s, v) = crate::permission::actions::held::tests::ask(ws, tgr::ACTION, params);
    assert_eq!(s, "200 OK", "{v}");
    v
}

#[test]
fn it_is_registered_for_an_owner_or_admin_with_a_click() {
    let _db = k2_core::db::scoped_for_test();
    let r = crate::permission::actions::find(tgr::ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.coverage), (Floor::Admin, Proof::Click, WallCoverage::None));
    assert!(crate::routes::route_policy::lookup(ROOMS_OK).and_then(|x| x.post).is_some(), "the route has a POST row");
}

#[test]
fn an_agents_test_login_reaches_its_roles_rooms_only_after_an_owner_allows_the_card() {
    let f = fx("tg1");
    let (user, added) = agent_test_login(&f, &f.role);
    // The add says what waits and how to ask.
    assert_eq!(added["roomsWait"]["rooms"], json!([f.b.handle]), "{added}");
    assert_eq!(added["roomsWait"]["ask"], format!("k2 app user rooms-ok {} {user}", f.app_id), "{added}");
    // The line after the command's name, for a CLI to put its own in front.
    assert_eq!(added["roomsWait"]["askTail"], format!("app user rooms-ok {} {user}", f.app_id), "{added}");
    // Today's rule: only the agent's own room.
    assert_eq!(rooms_now(&user, &f.svc), vec![f.a.id.clone()]);
    // 0.46.18 papercut 5: what waits names the rooms-ok card, never a
    // password reset (a test login has no person to choose one).
    let note = crate::skin_routes::waiting_note(&user).expect("a room waits");
    assert_eq!((note["test"].clone(), note["rooms"].clone()), (json!(true), json!([f.b.handle])), "{note}");
    assert_eq!(note["ask"], format!("k2 app user rooms-ok {} {user}", f.app_id), "{note}");

    // The agent asks: one card on its Thread naming the login, role and rooms.
    let asked = rooms_ok(&agent(&f.a), &f.app_id, &user).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(asked["status"], "asked", "{asked}");
    let id = asked["card"]["id"].as_str().expect("id").to_string();
    let card = asked["card"]["card"].as_str().expect("card").to_string();
    let vw = view(&f.a, &card);
    assert_eq!(vw["whatChanges"], format!("Let {}'s test login {user} into {} with its role {}.", f.a.handle, f.b.handle, f.role), "{vw}");
    assert_eq!(vw["approvers"]["floor"], "admin");
    assert_eq!(rooms_now(&user, &f.svc), vec![f.a.id.clone()], "asking changes nothing");

    // The agent never answers its own card; a Member is below the floor.
    let passport: HashMap<String, String> = [
        (crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string()),
        ("project_id".to_string(), f.a.id.clone()),
        ("from".to_string(), f.a.id.clone()),
    ]
    .into();
    let body = json!({ "addr": f.a.handle, "id": card, "answer": "Allow" }).to_string();
    let r = crate::overlay_routes::dispatch_post("/cli/thread/answer", &passport, body.as_bytes());
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let mel = login("tgrm", Role::Member);
    below_floor(answer_as(&mel, &f.a, &card, json!({ "answer": "Allow" })), "admin");
    assert_eq!(rooms_now(&user, &f.svc), vec![f.a.id.clone()]);

    // An Admin allows: exactly the card's rooms, audited.
    let ada = login("tgra", Role::Admin);
    let (s, a) = answer_as(&ada, &f.a, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(row(&id).state, RequestState::Approved);
    assert_eq!(rooms_now(&user, &f.svc), sorted(vec![f.a.id.clone(), f.b.id.clone()]), "the role's other room opens");
    let audit = k2_core::auth_audit::read_all().expect("audit").iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
    assert!(audit.contains("app-test-guest-rooms") && audit.contains(&user) && audit.contains(&format!("user:{ada}")), "{audit}");

    // Asked again: already open, no card.
    assert_eq!(rooms_ok(&agent(&f.a), &f.app_id, &user).expect("again")["status"], "already_open");
}

#[test]
fn rooms_outside_the_card_stay_refused_and_a_changed_role_needs_a_new_card() {
    let f = fx("tg2");
    let (user, _) = agent_test_login(&f, &f.role);
    let p = skin::list_principals().expect("list").into_iter().find(|p| p.username == user).expect("principal");
    // The owner opens them from a terminal (the same rules, at once).
    let opened = rooms_ok(&Who::Owner, &f.app_id, &user).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(opened["status"], "opened", "{opened}");
    assert_eq!(rooms_now(&user, &f.svc), sorted(vec![f.a.id.clone(), f.b.id.clone()]));

    // The role names a new room later: that one stays out.
    skin::update_role(
        &f.role,
        Some(&policy(&[(&f.a.id, &["thread:read"]), (&f.b.id, &["thread:read"]), (&f.c.id, &["thread:read"])])),
    )
    .expect("role grows");
    assert_eq!(rooms_now(&user, &f.svc), sorted(vec![f.a.id.clone(), f.b.id.clone()]), "c was never on a card");

    // A card asked for the role it had; the role changes before the click
    // (to one naming the same rooms, so only the role tells them apart):
    // stale, nothing opens, and what the old role opened doesn't carry over.
    let other = format!("other{}", uniq());
    skin::create_role_for_app(
        &other,
        &policy(&[(&f.a.id, &["thread:read"]), (&f.b.id, &["thread:read"]), (&f.c.id, &["thread:read"])]),
        Some(&f.svc),
    )
    .expect("role 2");
    let asked = ask_with(&f.a, json!({ "app": f.app_id, "login": user, "role": f.role, "rooms": [f.b.handle, f.c.handle] }));
    let card = card_of(&asked);
    let grant = skin::list_grants().expect("grants").into_iter().find(|g| g.subject_id == p.id && g.target_id == f.svc).expect("grant");
    skin::set_app_grant_role(&grant.id, &other).expect("new role");
    // The old role's rooms don't carry over to the new role.
    assert_eq!(rooms_now(&user, &f.svc), vec![f.a.id.clone()], "a new role starts at the own room again");
    let (s, a) = answer_as("owner", &f.a, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id_of(&asked)).state, RequestState::Stale);
    assert_eq!(rooms_now(&user, &f.svc), vec![f.a.id.clone()], "the stale card opened nothing");
}

#[test]
fn only_a_test_login_an_agent_made_takes_it() {
    let f = fx("tg3");
    let role_id = skin::list_roles().expect("roles").into_iter().find(|r| r.name == f.role).expect("role").id;
    // An agent-made login that isn't --test: today's rule stays.
    let plain = format!("plain{}", uniq());
    skin::add_principal_for_app_by(&plain, None, &f.svc, Some(&role_id), "agent:x", skin::AddedBy { agent_workspace: Some(&f.a.id), ..Default::default() })
        .expect("plain login");
    let (code, status, _) = rooms_ok(&agent(&f.a), &f.app_id, &plain).expect_err("not a test login");
    assert_eq!((code.as_str(), status), (NOT_A_TEST_GUEST, 409));
    let (s, v) =
        crate::permission::actions::held::tests::ask(&f.a, tgr::ACTION, json!({ "app": f.app_id, "login": plain, "role": f.role, "rooms": [f.b.handle] }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("400 Bad Request", Some("bad_params")), "{v}");
    // Even the owner can't open a plain login's rooms this way.
    assert_eq!(rooms_ok(&Who::Owner, &f.app_id, &plain).expect_err("owner, plain login").0, NOT_A_TEST_GUEST);

    // A test login a person made isn't held to the own room at all.
    let app = guest_app_for(&Who::Owner, Some(&f.app_id)).expect("app");
    let by_person = format!("person{}", uniq());
    add_checked(&app, "owner", None, &NewTestGuest { username: &by_person, role: Some(&f.role), expires: None, rooms: &[], password: Some("pw-test-only-2") })
        .expect("person's test login");
    assert_eq!(rooms_now(&by_person, &f.svc), sorted(vec![f.a.id.clone(), f.b.id.clone()]), "a person's test login has its role's rooms");
    assert_eq!(rooms_ok(&agent(&f.a), &f.app_id, &by_person).expect_err("not tied").0, tgr::NOT_TIED);

    // Another workspace's agent never sees the App.
    let (user, _) = agent_test_login(&f, &f.role);
    assert_eq!(rooms_ok(&agent(&f.c), &f.app_id, &user).expect_err("not its App").0, "app_not_found");
    // A Member login opens nothing.
    let mel = login("tg3m", Role::Member);
    let (code, status, _) = rooms_ok(&Who::Login { username: mel, role: Role::Member }, &f.app_id, &user).expect_err("member");
    assert_eq!((code.as_str(), status), ("owner_only", 403));
    assert_eq!(rooms_now(&user, &f.svc), vec![f.a.id.clone()]);
}

#[test]
fn assign_sends_the_card_to_one_admin() {
    let f = fx("tg4");
    let (user, _) = agent_test_login(&f, &f.role);
    let ada = login("tg4a", Role::Admin);
    let _bo = login("tg4b", Role::Admin);
    let asked = super::held_cards::with_assign(Some(vec![ada.clone()]), || rooms_ok(&agent(&f.a), &f.app_id, &user)).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(asked["card"]["routedTo"], json!([ada]), "{asked}");
    let q = row(asked["card"]["id"].as_str().expect("id"));
    assert_eq!(crate::permission::approvers::of(q.id.as_str()), Some(vec![ada.clone()]));
}
