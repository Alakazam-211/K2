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

fn audit_text() -> String {
    std::fs::read_to_string(k2_core::auth_audit::path()).unwrap_or_default()
}

/// AG-21: the home client is the owner's alone (the owner token, an Owner
/// login or a K2 shell tab). An agent with Manage Sew can't name its own
/// workspace's App the home client (that App's sessions would see every
/// Space each person is in), nor clear it; an Admin or Member login can't
/// either. Every change and refusal is audited; refusals store nothing.
#[test]
fn only_the_owner_sets_or_clears_the_home_client_ag21() {
    let f = fx();
    // The agent of ws-h13, which owns both Apps, with Manage Sew on.
    k2_core::workspace::settings::set_agents_can_manage_chat("ws-h13", true).unwrap();
    let agent = Who::Agent { project_id: "ws-h13".into(), session_id: "s-agent".into() };
    assert!(need_manage(&agent).is_ok(), "the agent has Manage Sew");
    let admin = Who::Login { username: "adm".into(), role: Role::Admin };
    let member = Who::Login { username: "mem".into(), role: Role::Member };

    for (who, label) in [(&agent, "agent with Manage Sew"), (&admin, "Admin login"), (&member, "Member login")] {
        for app in [json!(f.jobs), json!(f.hearth), json!(null)] {
            let (st, v) = call(who, HOME_CLIENT, json!({ "app": app }));
            assert_eq!((st, v["code"].clone()), (403, json!("owner_only")), "{label} {app}: {v}");
        }
    }
    let (_, v) = call(&agent, HOME_CLIENT, json!({"app": f.jobs}));
    assert!(v["hint"].as_str().unwrap().contains("never an agent's"), "{v}");
    assert!(k2_core::tool_hook::home_settings().unwrap().is_empty(), "a refusal stores nothing");
    let audit = audit_text();
    assert!(audit.contains("sew-home-client") && audit.contains(&format!("refused owner_only {}", f.jobs)), "{audit}");
    assert!(audit.contains("agent:"), "the agent is named in the audit: {audit}");

    // The owner token, an Owner login and a shell tab do it.
    let owner_login = Who::Login { username: "boss".into(), role: Role::Owner };
    let shell = Who::Shell { project_id: "ws-h13".into(), session_id: "s-tab".into() };
    let (st, v) = call(&Who::Owner, HOME_CLIENT, json!({"app": f.hearth}));
    assert_eq!((st, v["homeClient"].clone()), (200, json!(f.hearth)), "{v}");
    let (st, v) = call(&owner_login, HOME_CLIENT, json!({"app": null}));
    assert_eq!((st, v["homeClient"].clone()), (200, json!(null)), "{v}");
    let (st, v) = call(&shell, HOME_CLIENT, json!({"app": f.jobs}));
    assert_eq!((st, v["homeClient"].clone()), (200, json!(f.jobs)), "{v}");
    let stored = k2_core::tool_hook::home_settings().unwrap();
    let home = stored.iter().find(|s| s.subject == k2_core::tool_hook::home::HOME_SUBJECT).expect("home row");
    assert_eq!(home.updated_by, shell.label(), "who set it (Sew-Actor)");
    let audit = audit_text();
    assert!(audit.contains(&format!("set {}", f.hearth)) && audit.contains("set none") && audit.contains(&format!("set {}", f.jobs)), "{audit}");

    // App-only on only narrows: Manage Sew, the agent included. Off widens:
    // a person (the owner or an Admin), never the agent.
    let (st, v) = call(&agent, HOME_APP, json!({"app": f.jobs, "hidden": true}));
    assert_eq!(st, 200, "{v}");
    let (st, v) = call(&agent, HOME_APP, json!({"app": f.jobs, "hidden": false}));
    assert_eq!((st, v["code"].clone()), (403, json!("owner_only")), "{v}");
    let (st, v) = call(&member, HOME_APP, json!({"app": f.jobs, "hidden": false}));
    assert_eq!((st, v["code"].clone()), (403, json!("owner_only")), "{v}");
    let (st, v) = call(&admin, HOME_APP, json!({"app": f.jobs, "hidden": false}));
    assert_eq!((st, v["apps"][0]["hiddenFromHome"].clone()), (200, json!(false)), "{v}");
}

// ── Sew 0.6.0 A23: the owner's approval ──────────────────────────────

/// Only a setting the owner made (AG-21's rule) is vouched for: the owner
/// token, a K2 shell tab, an Owner login (by its role now). An Admin
/// login, an agent, the sweep, a demoted or removed login are not.
#[test]
fn only_the_owners_setting_gets_the_owners_approval() {
    let role = |u: &str| match u {
        "boss" => Some(Role::Owner),
        "adm" => Some(Role::Admin),
        "mem" => Some(Role::Member),
        _ => None,
    };
    for yes in ["owner", "shell:ws-h13", "login:boss"] {
        assert!(owner_actor(yes, &role), "{yes}");
    }
    for no in ["login:adm", "login:mem", "login:gone", "login:", "shell:", "agent:ws-h13", "system:reconcile", "owner-token", "", "Owner"] {
        assert!(!owner_actor(no, &role), "{no:?}");
    }
    // The labels the route stores are exactly these.
    assert!(owner_actor(&Who::Owner.label(), &role));
    assert!(owner_actor(&Who::Shell { project_id: "ws-h13".into(), session_id: "s".into() }.label(), &role));
    assert!(owner_actor(&Who::Login { username: "boss".into(), role: Role::Owner }.label(), &role));
    assert!(!owner_actor(&Who::Agent { project_id: "ws-h13".into(), session_id: "s".into() }.label(), &role));
}

/// Sew's `OwnerApproval::OwnerPass` (sew-proto `mgmt.rs`, fixture
/// `MgmtHomeClientRequest.full.json`): `{app_id, approval: {kind:
/// "owner_pass", pass}}`, the pass a fresh 60 s `k2:owner` pass for this
/// install with no caps and no `app`, a new `jti` (and key) every time.
#[test]
fn the_approval_is_a_fresh_owner_pass_on_the_exact_body() {
    let a = approval_claims("555555555", 1_000).expect("claims");
    let b = approval_claims("555555555", 1_000).expect("claims");
    assert_eq!((a.sub.as_str(), a.aud.as_str(), a.sid.as_str()), ("k2:owner", "sew:555555555", APPROVAL_SID));
    assert_eq!((a.iat, a.exp), (1_000, 1_000 + crate::chat_pass::PASS_TTL_SECS));
    assert_eq!((a.caps.clone(), a.app.clone()), (None, None), "no ceiling claims, no App: Sew's install owner itself");
    assert_ne!(a.jti, b.jti, "one pass per send: Sew spends the jti");
    assert!(approval_claims("0123", 1_000).is_err(), "never an aud Sew can't hold");

    let body: serde_json::Value = serde_json::from_slice(&with_approval(r#"{"app_id":"hearth"}"#, "P.Q.R").unwrap()).unwrap();
    assert_eq!(body, json!({"app_id": "hearth", "approval": {"kind": "owner_pass", "pass": "P.Q.R"}}));
    let clear: serde_json::Value = serde_json::from_slice(&with_approval(r#"{"app_id":null}"#, "P.Q.R").unwrap()).unwrap();
    assert_eq!(clear, json!({"app_id": null, "approval": {"kind": "owner_pass", "pass": "P.Q.R"}}), "a clear carries it too");
    assert!(with_approval(r#"{}"#, "P").is_err() && with_approval("[1]", "P").is_err());

    let (ka, kb) = (approval_key(&a.jti), approval_key(&b.jti));
    assert_ne!(ka, kb, "a new Idempotency-Key per send (Sew hashes the body under the key)");
    for k in [&ka, &kb] {
        assert!(k.len() <= 64 && k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'), "Sew's key grammar: {k}");
    }
}

/// Sew 0.6.0's answers to a home-client write, said clearly: approval
/// refused (with the approval: block, after the worker's wait; without:
/// re-read the flags and retry), and the home client never governs.
#[test]
fn sews_home_client_refusals_map_to_clear_errors() {
    let (blocks, m) = sew_refusal(403, "owner_approval_required", Some("hearth"), true).expect("mapped");
    assert!(blocks && m.contains("owner_approval_required") && m.contains("k2:owner"), "{m}");
    let (blocks, m) = sew_refusal(403, "owner_approval_required", Some("hearth"), false).expect("mapped");
    assert!(!blocks && m.contains("approval"), "{m}");
    let (blocks, m) = sew_refusal(409, "home_client_cannot_govern", Some("jobs-id"), true).expect("mapped");
    assert!(blocks && m.contains("home_client_cannot_govern") && m.contains("jobs-id") && m.contains("k2 hostchat home client"), "{m}");
    assert_eq!(sew_refusal(409, "app_mismatch", None, true), None, "anything else: the worker's usual rules");
    assert_eq!(sew_refusal(400, "owner_approval_required", None, true), None);
}

/// An agent or an Admin is refused at the route, before anything is
/// stored, so no home-client row is ever planned for Sew: even with Sew
/// 0.6.0's flags, the sweep owes Sew nothing.
#[test]
fn a_refused_caller_never_reaches_sew() {
    let f = fx();
    let flags: Vec<String> = ["home_client", "home_client_owner_approval"].iter().map(|s| s.to_string()).collect();
    k2_core::tool_hook::note_features_for_tests(&["home_client", "home_client_owner_approval"]).unwrap();
    k2_core::workspace::settings::set_agents_can_manage_chat("ws-h13", true).unwrap();
    let agent = Who::Agent { project_id: "ws-h13".into(), session_id: "s-agent".into() };
    let admin = Who::Login { username: "adm".into(), role: Role::Admin };
    for who in [&agent, &admin] {
        for app in [json!(f.jobs), json!(null)] {
            let (st, v) = call(who, HOME_CLIENT, json!({ "app": app }));
            assert_eq!((st, v["code"].clone()), (403, json!("owner_only")), "{v}");
        }
    }
    assert!(k2_core::tool_hook::home_settings().unwrap().is_empty());
    k2_core::tool_hook::apply_settings(k2_core::tool_hook::CHAT_KIND, &flags, &k2_core::tool_hook::PolicyRead::NotRead).unwrap();
    let rows = k2_core::tool_hook::log(50, Some(k2_core::tool_hook::home::HOME_SUBJECT)).unwrap();
    assert!(rows.is_empty(), "nothing for Sew: {rows:?}");
    let (_, v) = call(&Who::Owner, HOME, json!({}));
    assert_eq!((v["homeClientDelivered"].clone(), v["homeClientError"].clone()), (json!(true), json!(null)), "{v}");
    assert_eq!(v["sew"]["homeClientOwnerApproval"], json!(true), "{v}");
}
