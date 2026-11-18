//! A `--skin` site onto an App (0.46.16): the take's hold for an agent
//! without "manage its Apps' users", the copy before any move, the
//! approved card's list, move-plan for a site. In process, on a scoped DB
//! and a temp HOME (skin.db); the live take and return run in
//! `tests/it_apps/app_selfmove_integration.rs`.

use rusqlite::params;
use serde_json::json;

use super::super::handover_tests::{agent, call, code, fx, get, guest, Fx};
use super::*;

/// A public `--skin` site of `f.a` named `name` (a row as `k2 publish run
/// --skin` leaves it). Returns its service id.
fn site(f: &Fx, name: &str) -> String {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let row = ps::insert(&conn, &f.a, name, ps::CMD_SKIN_SENTINEL, "/tmp/k2-hov-test/site", 45991, ps::EXPOSE_TUNNEL, ps::DESIRED_RUNNING, ps::KIND_SKIN, "site-ui").unwrap();
    row.id
}

fn switches(ws: &str, build: bool, users: bool) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute(
        "UPDATE projects SET agents_can_manage_skin = ?2, agents_can_manage_app_users = ?3 WHERE id = ?1",
        params![ws, build as i64, users as i64],
    )
    .unwrap();
}

fn app_row(f: &Fx) -> AppRow {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::deploy::store::get_app(&conn, &f.app).unwrap().expect("app")
}

fn on_app(f: &Fx, pid: &str) -> bool {
    k2_core::skin::list_grants().unwrap().iter().any(|g| g.subject_id == pid && g.kind == "app" && g.target_id == f.svc)
}

#[test]
fn an_agent_without_the_users_switch_asks_once_naming_the_sites_guests() {
    let f = fx("skmv1");
    let svc = site(&f, "site1");
    let ada = guest("ada", &svc, &[]);
    let bob = guest("bob", &svc, &[]);
    switches(&f.a, true, false);
    let (st, v) = call("/cli/app/label/take", &agent(&f.a), json!({ "app": f.app, "label": "site1" }));
    assert_eq!((st, v["status"].as_str(), v["kind"].as_str()), (202, Some("held"), Some("manage")), "{v}");
    assert_eq!(v["diff"]["users"], json!(["ada", "bob"]), "the card names the guests: {v}");
    let body = &v["request"]["body"];
    assert_eq!(body["route"], "/cli/app/label/take", "{v}");
    assert_eq!(body["body"]["expectUsers"], json!(["ada", "bob"]), "the replay copies exactly these: {v}");
    assert!(!on_app(&f, &ada) && !on_app(&f, &bob), "nothing copied before the owner approves");

    // An agent naming the list itself is still held: expectUsers never
    // stands in for the switch.
    let (st, v) = call("/cli/app/label/take", &agent(&f.a), json!({ "app": f.app, "label": "site1", "expectUsers": ["ada", "bob"] }));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    assert!(!on_app(&f, &ada), "still nothing copied");
}

#[test]
fn with_the_users_switch_the_take_goes_on_and_copies_nothing_before_the_app_answers() {
    let f = fx("skmv2");
    let svc = site(&f, "site2");
    let ada = guest("ada", &svc, &[]);
    switches(&f.a, true, true);
    let (st, v) = call("/cli/app/label/take", &agent(&f.a), json!({ "app": f.app, "label": "site2" }));
    assert_eq!((st, code(&v)), (409, "app_not_healthy"), "no hold, the move's own checks: {v}");
    assert!(!on_app(&f, &ada), "the copy comes after the health check");
    // A person's take isn't held either.
    let (st, v) = call("/cli/app/label/take", &Who::Owner, json!({ "app": f.app, "label": "site2" }));
    assert_eq!((st, code(&v)), (409, "app_not_healthy"), "{v}");
    // An App's own service is still nothing to take.
    let (st, v) = call("/cli/app/label/take", &Who::Owner, json!({ "app": f.app, "label": f.other }));
    assert_eq!((st, code(&v)), (409, "bad_request"), "{v}");
}

#[test]
fn the_approved_list_is_copied_exactly_or_nothing_is() {
    let f = fx("skmv3");
    let svc = site(&f, "site3");
    let row = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        ps::get_by_id(&conn, &svc).unwrap().unwrap()
    };
    let app = app_row(&f);
    let ada = guest("ada", &svc, &[]);
    let bob = guest("bob", &svc, &[]);
    let e = copy_guests(&Who::Owner, &app, &row, Some(&["ada".to_string()])).expect_err("the list grew");
    assert_eq!(e.code, GUESTS_CHANGED, "{}", e.hint);
    assert!(!on_app(&f, &ada), "nothing written on a mismatch");
    let r = copy_guests(&Who::Owner, &app, &row, Some(&["bob".to_string(), "ada".to_string()])).expect("the same list");
    assert_eq!(r.added, vec!["ada".to_string(), "bob".to_string()]);
    assert!(on_app(&f, &ada) && on_app(&f, &bob));
    // The answer says what moved and that guests sign in once more.
    let a = take_answer(&app, "site3", 4242, &row, &r);
    assert_eq!(a["site"], true);
    assert_eq!(a["copied"]["added"], json!(["ada", "bob"]));
    assert!(a["note"].as_str().unwrap().contains("same username and"), "{a}");
    assert!(a["note"].as_str().unwrap().contains(&format!("k2 app label return {} site3", f.app)), "{a}");
    // expectUsers must be a list of names.
    assert!(expect_users(&json!({ "expectUsers": "ada" })).is_err());
    assert_eq!(expect_users(&json!({})).unwrap(), None);
}

#[test]
fn move_plan_plans_a_skin_site_with_its_guests_and_no_box_login_copy() {
    let f = fx("skmv4");
    let svc = site(&f, "site4");
    guest("ada", &svc, &[]);
    let (st, v) = get("/cli/app/move-plan", &Who::Owner, &[("service", "site4"), ("app", &f.app)]);
    assert_eq!(st, 200, "{v}");
    let ids: Vec<&str> = v["steps"].as_array().unwrap().iter().filter_map(|s| s["id"].as_str()).collect();
    assert!(ids.ends_with(&["copy_guests", "label_take", "stop_old"]), "{ids:?}");
    assert!(!ids.contains(&"copy_host") && !ids.contains(&"data_put"), "{ids:?}");
    let copy = v["steps"].as_array().unwrap().iter().find(|s| s["id"] == "copy_guests").unwrap();
    assert_eq!(copy["userCount"], 1, "{copy}");
    assert_eq!(copy["users"], json!(["ada"]), "the owner sees the names: {copy}");
    assert_eq!(copy["command"], format!("k2 app label take {} site4", f.app), "{copy}");
    // An App's own row is still not a move.
    let (st, v) = get("/cli/app/move-plan", &Who::Owner, &[("service", &f.app)]);
    assert_eq!((st, code(&v)), (409, "not_a_cmd_service"), "{v}");
}
