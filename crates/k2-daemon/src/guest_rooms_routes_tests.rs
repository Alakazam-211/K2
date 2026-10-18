//! RS-A15, the App transport in process: real passes (remote pass, guest
//! session, platform pass) through the handler the dispatcher calls.

use std::collections::HashMap;

use k2_core::skin::{self, guest_rooms as gr, link as passes, RoomPolicy};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::*;

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    jobs: String,
    other: String,
    own: String,
    acme: String,
    beta: String,
    /// The Jobs App's remote pass (raw).
    pass: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let (own, acme, beta) = (format!("jobsws-{tag}"), format!("acme-{tag}"), format!("beta-{tag}"));
    let (jobs, other) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for r in [&own, &acme, &beta] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, format!("/tmp/grr-{r}")])
                .unwrap();
        }
        let j = k2_core::published_services::insert(&conn, &own, "jobs", "node s.js", "/tmp", 4200, "local", "running", "cmd", "").unwrap();
        let o = k2_core::published_services::insert(&conn, &acme, "other", "node s.js", "/tmp", 4201, "local", "running", "cmd", "").unwrap();
        (j.id, o.id)
    };
    let pass = passes::mint_remote_pass(&jobs, "jobs", &["thread:read".to_string()], &[own.clone()], 3600).unwrap().raw;
    Fx { _db: db, _home: home, jobs, other, own, acme, beta, pass }
}

/// (status, body) for one call with the raw credential, the way the
/// dispatcher resolves it off a link.
fn call(raw: &str, post: bool, body: J, q: &[(&str, &str)]) -> (u16, J) {
    let pass = crate::apps::gateway::resolve_pass(raw);
    let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = handle(pass, None, post, body.to_string().as_bytes(), &params);
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body)))
}

fn set(f: &Fx, user: &str, rooms: &[&str]) -> (u16, J) {
    call(&f.pass, true, json!({ "username": user, "rooms": rooms }), &[])
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

#[test]
fn no_grant_is_403_then_own_guest_set_and_read() {
    let f = fx("own");
    skin::add_principal_for_app("dana", None, &f.jobs, None, "test").unwrap();
    let (st, v) = set(&f, "dana", &[&f.acme]);
    assert_eq!((st, code(&v)), (403, "grant_required"), "{v}");
    let (st, v) = call(&f.pass, false, json!({}), &[("username", "dana")]);
    assert_eq!((st, code(&v)), (403, "grant_required"), "reads need the grant too: {v}");

    gr::grant(&f.jobs, &[f.acme.clone()], "owner").unwrap();
    let (st, v) = set(&f, "dana", &[&f.acme, &f.own]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["changed"], json!(true));
    assert_eq!(v["app"], json!(f.jobs));
    assert_eq!(v["rooms"], json!([f.acme, f.own]));
    assert_eq!(v["roomHandles"], json!([f.acme, f.own]));
    assert_eq!(v["role"], J::Null);
    let (st, v) = call(&f.pass, false, json!({}), &[("username", "dana"), ("app", &f.jobs)]);
    assert_eq!((st, v["changed"].clone()), (200, json!(false)), "{v}");
    assert_eq!(v["rooms"].as_array().unwrap().len(), 2);
    // Idempotent: the same rooms again write nothing.
    let (st, v) = set(&f, "dana", &[&f.own, &f.acme]);
    assert_eq!((st, v["changed"].clone()), (200, json!(false)), "{v}");
    // A guest's next login reads the new rooms (Thread-only caps there).
    let s = skin::create_app_session_token("dana", &f.jobs).unwrap();
    let mut rooms = s.meta.rooms.clone();
    rooms.sort();
    let mut want = vec![f.acme.clone(), f.own.clone()];
    want.sort();
    assert_eq!(rooms, want);
    // Audited: who (the remote pass's App), before, after.
    let rows = gr::audit_rows(&f.jobs, 10).unwrap();
    let change = rows.iter().find(|r| r.event == gr::AUDIT_CHANGE).expect("a change row");
    assert_eq!(change.actor, format!("app-remote:{}", f.jobs));
    assert_eq!(change.payload["before"], json!([]));
    assert_eq!(change.payload["after"], json!([f.acme, f.own]));
    assert_eq!(change.payload["via"], json!("app"));
}

#[test]
fn other_apps_guests_and_names_are_one_404_with_no_oracle() {
    let f = fx("nf");
    gr::grant(&f.jobs, &[f.acme.clone()], "owner").unwrap();
    skin::add_principal_for_app("erin", None, &f.other, None, "test").unwrap();
    let (st_other, other) = set(&f, "erin", &[&f.acme]);
    let (st_ghost, ghost) = set(&f, "ghost", &[&f.acme]);
    assert_eq!((st_other, code(&other)), (404, "not_found"));
    assert_eq!(other, ghost, "another App's guest and nobody answer the same");
    assert_eq!(st_ghost, 404);
    let (_, read_other) = call(&f.pass, false, json!({}), &[("username", "erin")]);
    let (_, read_ghost) = call(&f.pass, false, json!({}), &[("username", "ghost")]);
    assert_eq!(read_other, read_ghost);
    // Naming another App: the same 404, even for that App's real guest.
    let (st, v) = call(&f.pass, true, json!({ "username": "erin", "app": f.other, "rooms": [] }), &[]);
    assert_eq!((st, v), (404, ghost.clone()));
    // Nothing changed for erin on her own App.
    let g = skin::list_grants().unwrap().into_iter().find(|g| g.target_id == f.other).unwrap();
    assert_eq!(g.scope.as_deref(), Some("[]"));
}

#[test]
fn guest_platform_and_dead_passes_are_refused() {
    let f = fx("who");
    gr::grant(&f.jobs, &[], "owner").unwrap();
    skin::add_principal_for_app("dana", None, &f.jobs, None, "test").unwrap();
    // The App's own guest's session pass: never.
    let guest = skin::create_app_session_token("dana", &f.jobs).unwrap().raw;
    let (st, v) = call(&guest, true, json!({ "username": "dana", "rooms": [] }), &[]);
    assert_eq!((st, code(&v)), (403, "app_pass_required"), "{v}");
    // An owner-minted platform pass (no App): never.
    let (_, plat) = skin::create_token("deck", None, &[f.own.clone()]).unwrap();
    let (st, v) = call(&plat, false, json!({}), &[("username", "dana")]);
    assert_eq!((st, code(&v)), (403, "app_pass_required"), "{v}");
    // Unknown and revoked passes.
    let (st, v) = call("k2skn_nope", false, json!({}), &[("username", "dana")]);
    assert_eq!((st, code(&v)), (401, "pass_invalid"));
    passes::revoke_remote_passes(&f.jobs, None).unwrap();
    let (st, v) = set(&f, "dana", &[]);
    assert_eq!((st, code(&v)), (401, "pass_invalid"));
}

#[test]
fn rooms_outside_the_app_and_above_the_role_are_refused() {
    let f = fx("rules");
    gr::grant(&f.jobs, &[f.acme.clone()], "owner").unwrap();
    skin::add_principal_for_app("dana", None, &f.jobs, None, "test").unwrap();
    let (st, v) = set(&f, "dana", &[&f.acme, &f.beta]);
    assert_eq!((st, code(&v)), (403, "outside_app_rooms"), "{v}");
    assert_eq!(v["error"]["rooms"], json!([f.beta]));
    let (st, v) = set(&f, "dana", &["not-a-room"]);
    assert_eq!((st, code(&v)), (403, "outside_app_rooms"), "an unknown handle is the same answer: {v}");

    let mut policy = RoomPolicy::new();
    policy.insert(f.own.clone(), vec!["thread:read".into(), "files:read".into()]);
    let role = skin::create_role_for_app("crew", &policy, Some(&f.jobs)).unwrap();
    skin::add_principal_for_app("rio", None, &f.jobs, Some(&role.id), "test").unwrap();
    let (st, v) = set(&f, "rio", &[&f.own, &f.acme]);
    assert_eq!((st, code(&v)), (403, "above_role"), "{v}");
    let (st, v) = set(&f, "rio", &[]);
    assert_eq!((st, code(&v)), (409, "has_role"), "{v}");
    let (st, v) = set(&f, "rio", &[&f.own]);
    assert_eq!((st, v["changed"].clone(), v["role"].clone()), (200, json!(false), json!("crew")), "{v}");
}

#[test]
fn sixty_a_minute_then_429_with_retry() {
    let f = fx("rate");
    gr::grant(&f.jobs, &[], "owner").unwrap();
    skin::add_principal_for_app("dana", None, &f.jobs, None, "test").unwrap();
    for i in 0..gr::RATE_PER_MIN {
        let (st, v) = call(&f.pass, false, json!({}), &[("username", "dana")]);
        assert_eq!(st, 200, "call {i}: {v}");
    }
    let (st, v) = set(&f, "dana", &[]);
    assert_eq!((st, code(&v)), (429, "rate_limited"), "{v}");
    assert!(v["error"]["retryAfterMs"].as_u64().is_some_and(|ms| ms > 0 && ms <= 60_000), "{v}");
    assert_eq!(v["error"]["limit"], json!(60));
}

#[test]
fn bad_bodies_are_usage() {
    let f = fx("usage");
    gr::grant(&f.jobs, &[], "owner").unwrap();
    let pass = crate::apps::gateway::resolve_pass(&f.pass);
    let r = handle(pass.clone(), None, true, b"[1]", &HashMap::new());
    assert!(r.status.starts_with("400") && r.body.contains("\"usage\""), "{}", r.body);
    let (st, v) = call(&f.pass, true, json!({ "username": "dana" }), &[]);
    assert_eq!((st, code(&v)), (400, "usage"), "rooms is required: {v}");
    let (st, v) = call(&f.pass, false, json!({}), &[]);
    assert_eq!((st, code(&v)), (400, "usage"));
}

#[test]
fn on_a_link_only_that_links_app_pass_is_the_app() {
    let pass = |session: bool, app: &str| SkinPass {
        id: "t1".into(),
        principal_id: session.then(|| "p1".to_string()),
        username: "applink:jobs:1".into(),
        caps: vec![],
        rooms: vec![],
        session,
        room_policy: Default::default(),
        app: app.into(),
    };
    let link = OnLink { app_id: "jobs".into(), service_id: "svc-1".into() };
    assert_eq!(app_of(&pass(false, "svc-1"), Some(&link)), Some("svc-1".to_string()));
    assert_eq!(app_of(&pass(false, "svc-2"), Some(&link)), None, "another App's pass");
    assert_eq!(app_of(&pass(true, "svc-1"), Some(&link)), None, "a guest session of this App");
    assert_eq!(app_of(&pass(false, ""), None), None, "off a link a non-remote pass is no App");
}

#[test]
fn the_rows_are_one_reveal_on_the_link_never_the_helper() {
    let c = k2_core::contract::catalog();
    for (verb, method) in [("guests.rooms", "GET"), ("guests.setRooms", "POST")] {
        let row = c.verb(verb).unwrap_or_else(|| panic!("{verb} row"));
        assert!(row.exposed_to(k2_core::contract::Exposure::App) && row.exposed_to(k2_core::contract::Exposure::GardenLocal), "{verb}");
        assert_eq!(row.cap.as_deref(), Some(gr::CAP));
        assert_eq!(row.bindings.http.as_ref().map(|h| h.route.as_str()), Some(ROUTE));
        assert!(k2_core::app_surface::link_allows_http(method, ROUTE), "{verb} on the link");
        assert!(!k2_core::app_surface::helper_allows_http(method, ROUTE), "{verb}: a guest's browser never forwards it");
        assert_eq!(k2_core::app_surface::verb_for(method, ROUTE), Some(verb));
    }
}
