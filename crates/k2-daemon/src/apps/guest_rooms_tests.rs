//! RS-A15, the owner's grant and the CLI door in process: who may grant,
//! revoke and set; agents only on their own App and within the grant.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::skin::{self, guest_rooms as gr};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::*;

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    ws_a: String,
    ws_b: String,
    acme: String,
    jobs: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let (ws_a, ws_b, acme) = (format!("wsa-{tag}"), format!("wsb-{tag}"), format!("acme-{tag}"));
    let jobs = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for r in [&ws_a, &ws_b, &acme] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, format!("/tmp/grc-{r}")])
                .unwrap();
        }
        k2_core::published_services::insert(&conn, &ws_a, "jobs", "node s.js", "/tmp", 4300, "local", "running", "cmd", "").unwrap().id
    };
    skin::add_principal_for_app("dana", None, &jobs, None, "test").unwrap();
    Fx { _db: db, _home: home, ws_a, ws_b, acme, jobs }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn login(role: Role) -> Who {
    Who::Login { username: "ada".into(), role }
}

fn post(path: &str, who: &Who, body: J) -> (u16, J) {
    let r = handle(path, who, true, body.to_string().as_bytes(), &HashMap::new());
    (r.status.split_whitespace().next().unwrap().parse().unwrap(), serde_json::from_str(&r.body).unwrap())
}

fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = handle(path, who, false, b"", &p);
    (r.status.split_whitespace().next().unwrap().parse().unwrap(), serde_json::from_str(&r.body).unwrap())
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

#[test]
fn only_the_owner_grants_and_the_grant_lists() {
    let f = fx("grant");
    let body = json!({ "app": f.jobs, "kind": "guests-rooms", "rooms": [f.acme] });
    for who in [agent(&f.ws_a), login(Role::Admin), login(Role::Member), Who::Wrong, Who::Invalid] {
        let (st, v) = post("/cli/app/grant", &who, body.clone());
        assert_eq!(st, 403, "{who:?}: {v}");
    }
    assert_eq!(gr::get_grant(&f.jobs).unwrap(), None);
    let (st, v) = post("/cli/app/grant", &Who::Owner, json!({ "app": f.jobs, "kind": "nope" }));
    assert_eq!(st, 400, "{v}");
    let (st, v) = post("/cli/app/grant", &Who::Owner, json!({ "app": f.jobs, "kind": "guests-rooms", "rooms": ["ghost-room"] }));
    assert_eq!(st, 400, "unknown rooms aren't granted: {v}");
    let (st, v) = post("/cli/app/grant", &login(Role::Owner), body);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["grant"]["rooms"], json!([f.acme]));
    let mut hands = v["grant"]["handsOut"].as_array().unwrap().clone();
    hands.sort_by_key(|x| x.as_str().unwrap().to_string());
    let mut want = vec![json!(f.acme), json!(f.ws_a)];
    want.sort_by_key(|x| x.as_str().unwrap().to_string());
    assert_eq!(hands, want, "the grant's rooms plus the App's own");
    assert_eq!(gr::get_grant(&f.jobs).unwrap().unwrap().granted_by, "owner:ada");
    // An agent reads its own App's grants; another workspace's agent can't see the App.
    let (st, v) = get("/cli/app/grants", &agent(&f.ws_a), &[("app", "jobs")]);
    assert_eq!((st, v["grants"].as_array().map(Vec::len)), (200, Some(1)), "{v}");
    let (st, v) = get("/cli/app/grants", &agent(&f.ws_b), &[("app", &f.jobs)]);
    assert_eq!((st, code(&v)), (404, "app_not_found"), "{v}");
}

#[test]
fn revoke_narrows_for_the_apps_own_agent() {
    let f = fx("revoke");
    gr::grant(&f.jobs, &[], "owner").unwrap();
    let (st, v) = post("/cli/app/grant/revoke", &agent(&f.ws_b), json!({ "app": f.jobs, "kind": "guests-rooms" }));
    assert_eq!(st, 404, "{v}");
    let (st, v) = post("/cli/app/grant/revoke", &login(Role::Member), json!({ "app": f.jobs, "kind": "guests-rooms" }));
    assert_eq!(st, 403, "{v}");
    let (st, v) = post("/cli/app/grant/revoke", &agent(&f.ws_a), json!({ "app": "jobs", "kind": "guests-rooms" }));
    assert_eq!((st, v["revoked"].clone()), (200, json!(true)), "{v}");
    assert_eq!(gr::get_grant(&f.jobs).unwrap(), None);
}

#[test]
fn agents_set_within_the_grant_and_the_owner_with_owner_rights() {
    let f = fx("set");
    let body = |rooms: &[&str]| json!({ "app": "jobs", "username": "dana", "rooms": rooms });
    // No grant: the App's own agent is refused; the owner isn't.
    let (st, v) = post(GUEST_ROOMS, &agent(&f.ws_a), body(&[&f.ws_a]));
    assert_eq!((st, code(&v)), (403, "grant_required"), "{v}");
    let (st, v) = get(GUEST_ROOMS, &agent(&f.ws_a), &[("app", "jobs"), ("username", "dana")]);
    assert_eq!((st, code(&v)), (403, "grant_required"), "{v}");
    let (st, v) = post(GUEST_ROOMS, &Who::Owner, body(&[&f.ws_b]));
    assert_eq!((st, v["changed"].clone()), (200, json!(true)), "owner rights reach any room: {v}");
    assert_eq!(v["before"], json!([]));

    gr::grant(&f.jobs, &[f.acme.clone()], "owner").unwrap();
    let (st, v) = post(GUEST_ROOMS, &agent(&f.ws_a), body(&[&f.acme, &f.ws_a]));
    assert_eq!((st, v["changed"].clone()), (200, json!(true)), "{v}");
    assert_eq!(v["beforeHandles"], json!([f.ws_b]));
    let (st, v) = post(GUEST_ROOMS, &agent(&f.ws_a), body(&[&f.ws_b]));
    assert_eq!((st, code(&v)), (403, "outside_app_rooms"), "{v}");
    let (st, v) = get(GUEST_ROOMS, &agent(&f.ws_a), &[("app", "jobs"), ("username", "dana")]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["rooms"].as_array().unwrap().len(), 2);
    // Another workspace's agent can't name the App; logins below Owner can't set.
    let (st, v) = post(GUEST_ROOMS, &agent(&f.ws_b), json!({ "app": f.jobs, "username": "dana", "rooms": [] }));
    assert_eq!((st, code(&v)), (404, "app_not_found"), "{v}");
    for role in [Role::Admin, Role::Member] {
        let (st, v) = post(GUEST_ROOMS, &login(role), json!({ "app": f.jobs, "username": "dana", "rooms": [] }));
        assert_eq!((st, code(&v)), (403, "owner_only"), "{role:?}: {v}");
    }
    // Not a guest of this App: not_found.
    let (st, v) = get(GUEST_ROOMS, &Who::Owner, &[("app", &f.jobs), ("username", "ghost")]);
    assert_eq!((st, code(&v)), (404, "not_found"), "{v}");
    let rows = gr::audit_rows(&f.jobs, 10).unwrap();
    let changes: Vec<_> = rows.iter().filter(|r| r.event == gr::AUDIT_CHANGE).collect();
    assert_eq!(changes.len(), 2, "{rows:?}");
    assert_eq!(changes[0].actor, format!("agent:{}", f.ws_a));
    assert_eq!(changes[1].actor, "owner");
    assert_eq!(changes[0].payload["via"], json!("cli"));
}
