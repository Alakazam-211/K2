//! Sew for Apps A2, the App transport in process: real passes (remote
//! pass, guest session, platform pass, the owner token) through the handler
//! the dispatcher calls.

use std::collections::HashMap;

use k2_core::skin::{self, app_chat as ac, link as passes};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::*;

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    jobs: String,
    other: String,
    /// The Jobs App's remote pass (raw).
    pass: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::tool_hook::allow_target_off_linux_for_tests(true);
    let (own, acme) = (format!("jobsws-{tag}"), format!("acme-{tag}"));
    let (jobs, other) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for r in [&own, &acme] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, format!("/tmp/gcr-{r}")])
                .unwrap();
        }
        let j = k2_core::published_services::insert(&conn, &own, "jobs", "node s.js", "/tmp", 4400, "local", "running", "cmd", "").unwrap();
        let o = k2_core::published_services::insert(&conn, &acme, "other", "node s.js", "/tmp", 4401, "local", "running", "cmd", "").unwrap();
        (j.id, o.id)
    };
    k2_core::tool_hook::set_target(k2_core::tool_hook::CHAT_KIND, true, Some("/tmp/k2-test-sew.sock"), Some("/tmp/k2-test-sew.token"), "owner-token")
        .expect("chat on");
    k2_core::tool_hook::note_features_for_tests(&["app_scoped_sessions"]).unwrap();
    let pass = passes::mint_remote_pass(&jobs, "jobs", &["thread:read".to_string()], &[own], 3600).unwrap().raw;
    Fx { _db: db, _home: home, jobs, other, pass }
}

fn call(raw: &str, post: bool, body: J, q: &[(&str, &str)]) -> (u16, J) {
    let pass = crate::apps::gateway::resolve_pass(raw);
    let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = handle(pass, None, post, body.to_string().as_bytes(), &params);
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body)))
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

#[test]
fn no_grant_is_403_then_the_app_gives_and_takes_its_guests_chat() {
    let f = fx("give");
    skin::add_principal_for_app("dana", None, &f.jobs, None, "test").unwrap();
    let (st, v) = call(&f.pass, true, json!({"username": "dana", "scope": "member"}), &[]);
    assert_eq!((st, code(&v)), (403, "grant_required"), "{v}");
    let (st, v) = call(&f.pass, false, json!({}), &[("username", "dana")]);
    assert_eq!((st, code(&v)), (403, "grant_required"), "reads need the grant too: {v}");

    ac::grant(&f.jobs, Some(5), "owner-token").unwrap();
    let (st, v) = call(&f.pass, true, json!({"username": "dana", "scope": "member", "app": f.jobs}), &[]);
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["changed"].clone(), v["scope"].clone(), v["managedBy"].clone()), (json!(true), json!("member"), json!("app")));
    assert_eq!((v["members"].clone(), v["maxMembers"].clone()), (json!(1), json!(5)));
    assert!(v["principalId"].as_str().is_some_and(|p| p.len() == 36), "{v}");
    // The grant it wrote queued the person's Sew provision (S27).
    let pid = v["principalId"].as_str().unwrap().to_string();
    let log = k2_core::tool_hook::log(50, Some(&format!("k2:principal:{pid}"))).unwrap();
    assert!(log.iter().any(|r| r.op == "provision"), "a provision is queued: {log:?}");
    let (st, v) = call(&f.pass, false, json!({}), &[("username", "dana")]);
    assert_eq!((st, v["changed"].clone(), v["scope"].clone()), (200, json!(false), json!("member")), "{v}");
    let (st, v) = call(&f.pass, true, json!({"username": "dana", "scope": "none"}), &[]);
    assert_eq!((st, v["scope"].clone()), (200, J::Null), "{v}");
    let log = k2_core::tool_hook::log(50, Some(&format!("k2:principal:{pid}"))).unwrap();
    assert!(log.iter().any(|r| r.op == "deprovision"), "removing chat deactivates the member: {log:?}");
}

#[test]
fn only_this_apps_own_guests_and_only_its_own_pass() {
    let f = fx("own");
    ac::grant(&f.jobs, None, "owner-token").unwrap();
    skin::add_principal_for_app("eli", None, &f.other, None, "test").unwrap();
    skin::add_principal("fay").unwrap();
    for u in ["eli", "fay", "ghost"] {
        let (st, v) = call(&f.pass, true, json!({"username": u, "scope": "member"}), &[]);
        assert_eq!((st, code(&v)), (404, "not_found"), "{u}: {v}");
    }
    // Naming another App is the same 404.
    skin::add_principal_for_app("gus", None, &f.jobs, None, "test").unwrap();
    let (st, v) = call(&f.pass, true, json!({"username": "gus", "scope": "member", "app": f.other}), &[]);
    assert_eq!((st, code(&v)), (404, "not_found"), "{v}");
    // A guest's own session pass, a platform pass and the owner token are
    // never this App's pass.
    skin::set_principal_password("gus", Some("s3cret-horse-battery")).unwrap();
    let guest = skin::create_app_session_token("gus", &f.jobs).unwrap().raw;
    let (st, v) = call(&guest, true, json!({"username": "gus", "scope": "member"}), &[]);
    assert_eq!((st, code(&v)), (403, "app_pass_required"), "{v}");
    let (_m, platform) = skin::create_token("bff", Some(&["thread:read".to_string()]), &["jobsws-own".to_string()]).unwrap();
    let (st, v) = call(&platform, true, json!({"username": "gus", "scope": "member"}), &[]);
    assert_eq!((st, code(&v)), (403, "app_pass_required"), "{v}");
    let (st, v) = call("k2skn_unknownunknownunknownunknown", true, json!({"username": "gus", "scope": "member"}), &[]);
    assert_eq!((st, code(&v)), (401, "pass_invalid"), "{v}");
    // Nothing was written for anyone.
    assert!(skin::list_grants().unwrap().iter().all(|g| g.kind != "chat"));
}

#[test]
fn usage_ceiling_and_owner_grants_are_refused_loudly() {
    let f = fx("loud");
    ac::grant(&f.jobs, Some(1), "owner-token").unwrap();
    skin::add_principal_for_app("ha", None, &f.jobs, None, "test").unwrap();
    skin::add_principal_for_app("hb", None, &f.jobs, None, "test").unwrap();
    let (st, v) = call(&f.pass, true, json!({"username": "ha", "scope": "admin"}), &[]);
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
    let (st, v) = call(&f.pass, true, json!({"scope": "member"}), &[]);
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
    assert_eq!(call(&f.pass, true, json!({"username": "ha", "scope": "member"}), &[]).0, 200);
    let (st, v) = call(&f.pass, true, json!({"username": "hb", "scope": "member"}), &[]);
    assert_eq!((st, code(&v), v["error"]["maxMembers"].clone()), (409, "chat_limit", json!(1)), "{v}");
    // An owner-made grant is the owner's.
    ac::set(&Caller::Owner, None, "hb", Scope::Guest, "owner-token", "cli").unwrap();
    let (st, v) = call(&f.pass, true, json!({"username": "hb", "scope": "member"}), &[]);
    assert_eq!((st, code(&v)), (409, "not_app_granted"), "{v}");
}
