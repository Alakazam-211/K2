//! Decision 13 routes in process: who may set the home client and an
//! App's "App-only" mark, and what is stored for the sweep.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use rusqlite::params;
use serde_json::json;

use super::*;

struct Fx {
    _home: crate::test_support::TempHome,
    _db: k2_core::db::TestDbGuard,
    hearth: String,
    jobs: String,
}

fn fx() -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let (hearth, jobs) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES ('ws-h13', 'ws-h13', '/tmp/ws-h13', 'ws-h13')", params![]).unwrap();
        let a = k2_core::published_services::insert(&conn, "ws-h13", "hearth", "node s.js", "/tmp", 4600, "local", "running", "cmd", "").unwrap();
        let b = k2_core::published_services::insert(&conn, "ws-h13", "jobs", "node s.js", "/tmp", 4601, "local", "running", "cmd", "").unwrap();
        (a.id, b.id)
    };
    Fx { _home: home, _db: db, hearth, jobs }
}

fn call(who: &Who, path: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
    let r = route(path, who, body.to_string().as_bytes(), &HashMap::new()).expect("a decision-13 route");
    match r {
        Ok(v) => (200, v),
        Err(f) => (f.status, json!({"code": f.code, "hint": f.hint})),
    }
}

#[test]
fn the_owner_sets_the_home_client_and_an_app_only_mark() {
    let f = fx();
    let (st, v) = call(&Who::Owner, HOME, json!({}));
    assert_eq!((st, v["homeClient"].clone()), (200, json!(null)), "{v}");
    let (st, v) = call(&Who::Owner, HOME_CLIENT, json!({"app": f.hearth}));
    assert_eq!((st, v["homeClient"].clone(), v["changed"].clone()), (200, json!(f.hearth), json!(true)), "{v}");
    assert_eq!(v["homeClientDelivered"], json!(true), "no home_client flag yet: nothing owed: {v}");
    k2_core::tool_hook::note_features_for_tests(&["home_client"]).unwrap();
    let (_, v) = call(&Who::Owner, HOME, json!({}));
    assert_eq!(v["homeClientDelivered"], json!(false), "with the flag the sweep owes it: {v}");
    let (_, v) = call(&Who::Owner, HOME_CLIENT, json!({"app": f.hearth}));
    assert_eq!(v["changed"], json!(false), "the same value again changes nothing");
    let (st, v) = call(&Who::Owner, HOME_APP, json!({"app": f.jobs, "hidden": true}));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["apps"], json!([{"app": f.jobs, "name": "jobs", "hiddenFromHome": true, "origin": null, "delivered": false, "updatedBy": Who::Owner.label()}]));
    assert_eq!(v["sew"]["homeClient"], json!(true), "the flag noted above: the sweep owes the App's settings");
    // Clearing the home client.
    let (_, v) = call(&Who::Owner, HOME_CLIENT, json!({"app": null}));
    assert_eq!(v["homeClient"], json!(null));
}

#[test]
fn bad_apps_and_non_managers_are_refused() {
    let f = fx();
    let (st, v) = call(&Who::Owner, HOME_CLIENT, json!({"app": "no-such-app"}));
    assert_eq!((st, v["code"].clone()), (404, json!("not_found")), "{v}");
    // By name: the one published service called `jobs`.
    let (st, v) = call(&Who::Owner, HOME_APP, json!({"app": "jobs", "hidden": false}));
    assert_eq!((st, v["apps"][0]["app"].clone()), (200, json!(f.jobs)), "a name resolves to its service id: {v}");
    let (st, _) = call(&Who::Owner, HOME_APP, json!({"app": f.jobs}));
    assert_eq!(st, 400, "hidden is required");
    // A Member login reads but never writes.
    let member = Who::Login { username: "ana".into(), role: Role::Member };
    assert_eq!(call(&member, HOME, json!({})).0, 200);
    let (st, _) = call(&member, HOME_APP, json!({"app": f.jobs, "hidden": true}));
    assert_eq!(st, 403, "Manage Sew only");
    assert_eq!(call(&Who::Invalid, HOME, json!({})).0, 403);
    let stored = k2_core::tool_hook::home_settings().unwrap();
    assert_eq!(stored.len(), 1, "only the owner's one by-name set; refusals store nothing: {stored:?}");
    assert_eq!(stored[0].body["hidden_from_home"], json!(false));
}
