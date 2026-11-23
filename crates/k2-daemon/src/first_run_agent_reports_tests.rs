//! The agent-reports row and card (plan-agent-reports-opt-in-app-v1)
//! through the route: each role before and after an answer, "Ask me
//! later" per person, and the card offered with the setup list off. A temp
//! home and a per-thread database. Fail loudly.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use serde_json::{json, Value};

use super::*;
use crate::compute_routes::Who;

struct Env {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    _git: first_run::DevToolsTestGuard,
}

/// A temp home and database with the setup list as given, Kessel off, and
/// git running (no dev_tools row).
fn env(list: bool) -> Env {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let git = first_run::dev_tools_for_test(first_run::DevTools::NoRow);
    k2_core::app_settings::update(json!({ "setupChecklist": list, "kesselFirstRun": false })).expect("switches");
    Env { _db: db, _home: home, _git: git }
}

fn login(name: &str, role: Role) -> Actor {
    k2_core::connect_users::add_user(name, "password123").expect("add");
    if role != Role::Member {
        k2_core::connect_users::set_role(name, role).expect("role");
    }
    Actor::login(name)
}

fn json_of(r: &CliResponse) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{}: {e}: {}", r.status, r.body))
}

fn status_for(actor: &Actor) -> Value {
    let r = handle(false, STATUS_PATH, b"", actor);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    json_of(&r)
}

fn dismiss(body: Value, actor: &Actor) -> CliResponse {
    handle(true, OFFER_DISMISS_PATH, body.to_string().as_bytes(), actor)
}

/// The agent_reports row, or None.
fn row(v: &Value) -> Option<Value> {
    v["rows"].as_array().expect("rows").iter().find(|r| r["id"] == json!("agent_reports")).cloned()
}

fn offered(v: &Value) -> bool {
    match v.get("offers") {
        None => false,
        Some(o) => o.as_array().expect("offers is a list").iter().any(|x| x == "agent_reports"),
    }
}

/// The people at the Admin floor and a Member.
fn people() -> (Vec<(&'static str, Actor)>, Actor) {
    let admins = vec![
        ("the owner token", Actor::owner_token()),
        ("an Owner login", login("olga", Role::Owner)),
        ("an Admin login", login("adam", Role::Admin)),
    ];
    (admins, login("mia", Role::Member))
}

/// The owner answers "No" through the settings route, the one POST the
/// app sends. `enabled: false` keeps the drain from starting a thread.
fn answer_no() {
    let body = json!({ "enabled": false, "agents": false, "agentPlatform": false, "agentAttach": "none" }).to_string();
    let r = crate::product_feedback_routes::handle(
        crate::product_feedback_routes::SETTINGS,
        true,
        Who::Owner,
        body.as_bytes(),
        &HashMap::new(),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = json_of(&r);
    assert_eq!((v["agentsAsked"].clone(), v["agents"].clone()), (json!(true), json!(false)), "{v}");
}

#[test]
fn each_role_before_and_after_an_answer() {
    let _e = env(false);
    let (admins, member) = people();
    for (who, a) in &admins {
        let v = status_for(a);
        assert_eq!(row(&v), Some(json!({ "id": "agent_reports", "state": "todo", "opens": "agent_reports" })), "{who}: {v}");
        let ids: Vec<&str> = v["rows"].as_array().expect("rows").iter().map(|r| r["id"].as_str().expect("id")).collect();
        let at = ids.iter().position(|id| *id == "agent_reports").expect("row");
        assert_eq!((ids[at - 1], ids[at + 1]), ("llm_sign_in", "first_agent"), "{who}: {ids:?}");
        assert!(offered(&v), "{who}: the list is off, so the card is offered: {v}");
    }
    let v = status_for(&member);
    assert_eq!(row(&v), None, "a Member never sees the row: {v}");
    assert!(!offered(&v), "nor the card: {v}");

    let mut rx = crate::session_events::sender().subscribe();
    answer_no();
    let mut everyone = false;
    while let Ok(ev) = rx.try_recv() {
        everyone |= matches!(ev, crate::session_events::SessionEvent::FirstRunChanged { person: None });
    }
    assert!(everyone, "the first answer tells every socket to re-read the list");

    for (who, a) in &admins {
        let v = status_for(a);
        assert_eq!(row(&v).expect("row")["state"], json!("done"), "{who}: {v}");
        assert!(!offered(&v), "{who}: answered, so no card for anyone: {v}");
    }
    let v = status_for(&member);
    assert!(row(&v).is_none() && !offered(&v), "{v}");
    // k2 setup's line once answered.
    let r = handle_as(false, STATUS_PATH, b"", &Actor::owner_token(), true);
    assert!(r.body.contains("\n  [x] Agent reports\n"), "{}", r.body);
}

#[test]
fn ask_me_later_is_per_person_and_works_with_kessel_off() {
    let _e = env(false);
    let (admins, member) = people();
    let (_, owner) = &admins[0];
    let (_, adam) = &admins[2];
    // Meet Kessel's dismiss is still Kessel's: 404 with Kessel off.
    for body in [json!({}), json!({ "offer": "meet_kessel" })] {
        let r = dismiss(body.clone(), owner);
        assert_eq!(r.status, "404 Not Found", "{body}: {}", r.body);
    }
    let r = dismiss(json!({ "offer": "agent_reports" }), owner);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = json_of(&r);
    assert!(!offered(&v), "{v}");
    assert_eq!(row(&v).expect("row")["state"], json!("skipped"), "stored as the row's skip: {v}");
    assert!(!offered(&status_for(owner)), "kept");
    assert!(offered(&status_for(adam)), "the owner's later is the owner's");
    // Unanswered still: the settings say nothing was chosen.
    assert!(!k2_core::product_feedback::settings::load().agents_asked());
    // A Member has no such row.
    let r = dismiss(json!({ "offer": "agent_reports" }), &member);
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    assert!(r.body.contains("unknown_row"), "{}", r.body);
    // An offer that isn't one.
    let r = dismiss(json!({ "offer": "nope" }), owner);
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    assert!(r.body.contains("usage"), "{}", r.body);
    // Start setup again asks again.
    let r = handle(true, RESTART_PATH, b"{}", owner);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = status_for(owner);
    assert_eq!(row(&v).expect("row")["state"], json!("todo"), "{v}");
}

#[test]
fn the_card_waits_for_the_list_and_shows_with_the_list_off() {
    // A new server: the list is on and in progress, so its row asks.
    let _e = env(true);
    let owner = Actor::owner_token();
    let v = status_for(&owner);
    assert_eq!((v["enabled"].clone(), v["finished"].clone()), (json!(true), json!(false)), "{v}");
    assert!(!offered(&v), "the row asks while the list is in progress: {v}");
    // Hidden as its Get started button: still in progress.
    assert_eq!(handle(true, HIDE_PATH, b"{}", &owner).status, "200 OK");
    assert!(!offered(&status_for(&owner)));
    // The owner turns the list off: the card instead.
    k2_core::app_settings::update(json!({ "setupChecklist": false })).expect("off");
    let v = status_for(&owner);
    assert_eq!(v["enabled"], json!(false));
    assert!(offered(&v), "{v}");
    assert_eq!(row(&v).expect("row")["state"], json!("todo"), "k2 setup still lists it: {v}");
}
