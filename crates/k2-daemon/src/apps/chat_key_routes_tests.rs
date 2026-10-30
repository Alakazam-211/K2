//! App-governed Spaces S1/S6 routes in process: only a person turns a chat
//! key on or widens it (the owner in person, an Admin within the
//! defaults), always with a fresh confirm bound to the App; narrowing and
//! revoke need no confirm and the App's own agent may; host, the home
//! client, a Sew without the flag are refused before a confirm is spent;
//! the home client can never be a keyed App.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::skin::app_chat_key as ck;
use rusqlite::params;
use serde_json::{json, Value as J};

use super::*;

struct Fx {
    _home: crate::test_support::TempHome,
    _db: k2_core::db::TestDbGuard,
    ws: String,
    app: String,
    other: String,
}

fn fx(tag: &str, flag: bool) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let ws = format!("ws-ck-{tag}");
    let (app, other) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![ws, format!("/tmp/{ws}")]).unwrap();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![format!("{ws}-b"), format!("/tmp/{ws}-b")]).unwrap();
        let a = k2_core::published_services::insert(&conn, &ws, "jobs", "node s.js", "/tmp", 4700, "local", "running", "cmd", "").unwrap();
        let b = k2_core::published_services::insert(&conn, &format!("{ws}-b"), "crm", "node s.js", "/tmp", 4701, "local", "running", "cmd", "").unwrap();
        (a.id, b.id)
    };
    if flag {
        k2_core::tool_hook::note_features_for_tests(&["app_governed_spaces"]).unwrap();
    }
    Fx { _home: home, _db: db, ws, app, other }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn login(role: Role) -> Who {
    Who::Login { username: "ada".into(), role }
}

/// "Build and run Apps" (switch 1) for `ws`'s agent.
fn build_switch(ws: &str, on: bool) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute("UPDATE projects SET agents_can_manage_skin = ?2 WHERE id = ?1", params![ws, on as i64]).expect("switch");
}

fn call(path: &str, who: &Who, post: bool, body: J, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = crate::apps::guest_rooms::handle(path, who, post, body.to_string().as_bytes(), &p);
    let st = r.status.split_whitespace().next().expect("status").parse().expect("number");
    (st, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json ({e}): {}", r.body)))
}

fn set(who: &Who, body: J) -> (u16, J) {
    call(KEY, who, true, body, &[])
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

fn on_body(f: &Fx) -> J {
    json!({ "app": f.app, "caps": ["chat:read", "chat:post", "voice:join"] })
}

#[test]
fn turning_a_key_on_needs_the_owner_in_person_and_a_fresh_confirm() {
    let f = fx("on", false);
    let vault = crate::vault::ensure_test_vault();
    build_switch(&f.ws, true);
    // Agents (with "Build and run Apps" or not), a shell tab, an Admin above
    // the defaults: today's plain owner_only. A Member: read-only.
    for who in [agent(&f.ws), Who::Shell { project_id: f.ws.clone(), session_id: "s".into() }] {
        let (st, v) = set(&who, on_body(&f));
        assert_eq!((st, code(&v)), (403, "owner_only"), "{who:?}: {v}");
    }
    build_switch(&f.ws, false);
    let (st, v) = set(&agent(&f.ws), on_body(&f));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    let (st, v) = set(&login(Role::Admin), on_body(&f));
    assert_eq!((st, code(&v)), (403, "owner_only"), "a --cmd service has no approved caps: only the owner names them: {v}");
    let (st, v) = set(&login(Role::Member), on_body(&f));
    assert_eq!((st, code(&v)), (403, crate::apps::access::MEMBER_READ_ONLY), "{v}");
    assert_eq!(ck::get(&f.app).unwrap(), None, "nothing written");

    // The owner: Sew without app_governed_spaces is refused first.
    let (st, v) = set(&Who::Owner, on_body(&f));
    assert_eq!((st, code(&v)), (409, "sew_feature_missing"), "{v}");
    assert_eq!(v["error"]["feature"], "app_governed_spaces");
    k2_core::tool_hook::note_features_for_tests(&["app_governed_spaces"]).unwrap();
    // The owner token alone isn't a person (agents can read it).
    let (st, v) = set(&Who::Owner, on_body(&f));
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "{v}");
    assert_eq!((v["error"]["serviceId"].as_str(), v["error"]["action"].as_str()), (Some(f.app.as_str()), Some(CONFIRM_ACTION)));
    // Bad values are refused before the confirm is spent.
    let tok = vault.confirm.mint(CONFIRM_ACTION, Some(&f.app), "owner", "local-auth");
    let mut bad = on_body(&f);
    bad["caps"] = json!(["files:read"]);
    bad["confirmToken"] = json!(tok);
    let (st, v) = set(&Who::Owner, bad);
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
    let mut bad = on_body(&f);
    bad["maxLevel"] = json!("owner");
    let (st, v) = set(&Who::Owner, bad);
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
    let (st, v) = set(&Who::Owner, json!({ "app": f.app }));
    assert_eq!((st, code(&v)), (400, "usage"), "a manifest without chat caps: name them: {v}");
    // A confirm bound to another App opens nothing here.
    let wrong = vault.confirm.mint(CONFIRM_ACTION, Some(&f.other), "owner", "local-auth");
    let mut b = on_body(&f);
    b["confirmToken"] = json!(wrong);
    let (st, v) = set(&Who::Owner, b);
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "{v}");
    // The owner in person: on, with the defaults for what wasn't named.
    let mut b = on_body(&f);
    b["confirmToken"] = json!(tok);
    let (st, v) = set(&Who::Owner, b.clone());
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["widened"], true);
    assert_eq!(
        (v["key"]["enabled"].clone(), v["key"]["caps"].clone(), v["key"]["maxLevel"].clone(), v["key"]["uploads"].clone(), v["key"]["maxMembers"].clone()),
        (json!(true), json!(["chat:read", "chat:post", "voice:join"]), json!("moderator"), json!(true), json!(500))
    );
    // A confirm works once: widening again needs a new one.
    b["maxLevel"] = json!("admin");
    let (st, v) = set(&Who::Owner, b);
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "{v}");
    // An Owner login, in person: admin level.
    let otok = vault.confirm.mint(CONFIRM_ACTION, Some(&f.app), "connect:ada", "connect-password");
    let (st, v) = set(&login(Role::Owner), json!({ "app": f.app, "maxLevel": "admin", "confirmToken": otok }));
    assert_eq!((st, v["key"]["maxLevel"].as_str()), (200, Some("admin")), "{v}");
    let (st, v) = call(KEY, &agent(&f.ws), false, json!({}), &[("app", &f.app)]);
    assert_eq!((st, v["key"]["maxLevel"].as_str()), (200, Some("admin")), "its own agent reads it: {v}");
    let (st, v) = call("/cli/app/grants", &Who::Owner, false, json!({}), &[("app", &f.app)]);
    assert_eq!(st, 200, "{v}");
    assert!(v["grants"].as_array().expect("grants").iter().any(|g| g["kind"] == "chat-key"), "{v}");
}

#[test]
fn host_and_the_home_client_never_hold_a_key() {
    let f = fx("home", true);
    let vault = crate::vault::ensure_test_vault();
    let (st, v) = set(&Who::Owner, json!({ "app": "host", "caps": ["chat:read"] }));
    assert_eq!((st, code(&v)), (400, "not_for_host"), "{v}");
    k2_core::tool_hook::set_home_client(Some(&f.app), "owner").unwrap();
    let tok = vault.confirm.mint(CONFIRM_ACTION, Some(&f.app), "owner", "local-auth");
    let mut b = on_body(&f);
    b["confirmToken"] = json!(tok.clone());
    let (st, v) = set(&Who::Owner, b.clone());
    assert_eq!((st, code(&v)), (409, "home_client_cannot_govern"), "{v}");
    assert_eq!(ck::get(&f.app).unwrap(), None);
    // The confirm wasn't spent on a refusal: it still works once the App
    // isn't the home client.
    k2_core::tool_hook::set_home_client(None, "owner").unwrap();
    let (st, v) = set(&Who::Owner, b);
    assert_eq!(st, 200, "{v}");
    // ...and now the keyed App can't become the home client.
    let r = crate::sew::home::route(crate::sew::home::HOME_CLIENT, &crate::sew::routes::Who::Owner, json!({ "app": f.app }).to_string().as_bytes(), &HashMap::new())
        .expect("a home route");
    let e = r.expect_err("refused");
    assert_eq!((e.status, e.code), (409, "home_client_cannot_govern"));
    // Another App can.
    let r = crate::sew::home::route(crate::sew::home::HOME_CLIENT, &crate::sew::routes::Who::Owner, json!({ "app": f.other }).to_string().as_bytes(), &HashMap::new())
        .expect("a home route");
    assert!(r.is_ok(), "{r:?}");
}

#[test]
fn narrowing_and_revoke_need_no_confirm_and_the_apps_agent_may() {
    let f = fx("narrow", true);
    let w = ck::Want { caps: vec!["chat:read".into(), "voice:join".into()], max_level: ck::Level::Admin, uploads: true, max_members: 100 };
    ck::set(&f.app, &w, "owner").unwrap();
    // Without "Build and run Apps": not even narrowing.
    let (st, v) = set(&agent(&f.ws), json!({ "app": f.app, "maxLevel": "member" }));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    build_switch(&f.ws, true);
    let (st, v) = set(&agent(&f.ws), json!({ "app": f.app, "maxLevel": "member", "uploads": false, "maxMembers": 10, "caps": ["chat:read"] }));
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["widened"].clone(), v["key"]["maxLevel"].clone(), v["key"]["uploads"].clone()), (json!(false), json!("member"), json!(false)));
    // Raising anything back is the owner's.
    for raise in [json!({"maxLevel": "moderator"}), json!({"uploads": true}), json!({"maxMembers": 11}), json!({"caps": ["chat:read", "voice:join"]})] {
        let mut b = raise.clone();
        b["app"] = json!(f.app);
        let (st, v) = set(&agent(&f.ws), b);
        assert_eq!((st, code(&v)), (403, "owner_only"), "{raise}: {v}");
    }
    // Another workspace's agent can't even see the App.
    let (st, _) = set(&agent(&format!("{}-b", f.ws)), json!({ "app": f.app, "maxLevel": "member" }));
    assert_eq!(st, 404);
    // An Admin narrows without a confirm too.
    let (st, v) = set(&login(Role::Admin), json!({ "app": f.app, "maxMembers": 5 }));
    assert_eq!((st, v["key"]["maxMembers"].as_u64()), (200, Some(5)), "{v}");
    // Revoke: the App's agent may; off at once; Sew is asked to end the
    // App's sessions (app_id) when it has the flag.
    let (st, v) = call(REVOKE, &agent(&f.ws), true, json!({ "app": f.app }), &[]);
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["revoked"].clone(), v["key"]["enabled"].clone()), (json!(true), json!(false)));
    assert_eq!(ck::enabled(&f.app).unwrap(), None);
    let (st, v) = call(REVOKE, &login(Role::Admin), true, json!({ "app": f.app }), &[]);
    assert_eq!((st, v["revoked"].clone()), (200, json!(false)), "already off: {v}");
    let (st, v) = call(REVOKE, &login(Role::Member), true, json!({ "app": f.app }), &[]);
    assert_eq!((st, code(&v)), (403, crate::apps::access::MEMBER_READ_ONLY), "{v}");
    // Turning it back on is a widening again: the owner in person.
    let (st, v) = set(&login(Role::Admin), json!({ "app": f.app }));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
}

#[test]
fn the_admin_ceiling_is_the_defaults_or_less() {
    let approved = vec!["chat:read".to_string(), "chat:post".to_string()];
    let w = |caps: &[&str], level: Level, uploads: bool, max: u32| Want { caps: caps.iter().map(|c| c.to_string()).collect(), max_level: level, uploads, max_members: max };
    assert!(within_admin_ceiling(None, &w(&["chat:read", "chat:post"], Level::Moderator, true, 500), Some(&approved)));
    assert!(within_admin_ceiling(None, &w(&["chat:read"], Level::Member, false, 10), Some(&approved)));
    assert!(!within_admin_ceiling(None, &w(&["chat:read"], Level::Admin, true, 10), Some(&approved)), "admin level");
    assert!(!within_admin_ceiling(None, &w(&["chat:read", "voice:join"], Level::Member, true, 10), Some(&approved)), "beyond the approved caps");
    assert!(!within_admin_ceiling(None, &w(&["chat:read"], Level::Member, true, 501), Some(&approved)), "beyond 500");
    assert!(!within_admin_ceiling(None, &w(&["chat:read"], Level::Member, true, 10), None), "nothing approved");
    let off_uploads = ChatKey {
        service_id: "s".into(),
        kind: ck::KIND,
        enabled: true,
        caps: vec!["chat:read".into()],
        max_level: Level::Member,
        uploads: false,
        max_members: 10,
        granted_by: "owner".into(),
        granted_at: 1,
        revoked_at: None,
    };
    assert!(!within_admin_ceiling(Some(&off_uploads), &w(&["chat:read"], Level::Member, true, 10), Some(&approved)), "uploads back on");
    assert!(within_admin_ceiling(Some(&off_uploads), &w(&["chat:read"], Level::Moderator, false, 10), Some(&approved)));
}

#[test]
fn the_confirm_sheet_names_the_app_from_its_record() {
    let f = fx("sheet", true);
    assert_eq!(confirm_reason("app-key-mint", Some(&f.app)), None);
    assert_eq!(confirm_reason(CONFIRM_ACTION, None), Some(Err("secretId (the App's service id) is required")));
    assert_eq!(confirm_reason(CONFIRM_ACTION, Some("nope")), Some(Err("no published App by that service id")));
    let r = confirm_reason(CONFIRM_ACTION, Some(&f.app)).expect("ours").expect("an App");
    assert!(r.contains("App jobs") && r.contains(&f.app) && r.contains("chat key"), "{r}");
}
