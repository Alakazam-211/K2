//! `/cli/app/egress*` and the held-widening routes (S15): who may propose,
//! narrow, approve and deny; approval needs the owner in person with a
//! fresh confirm bound to the request; narrowing is immediate.

use std::sync::Mutex;

use k2_core::deploy::store;
use serde_json::json;

use super::test_seams;
use super::*;

static MSGS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    ws_a: String,
    ws_b: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let (ws_a, ws_b) = (format!("ws-a-{tag}"), format!("ws-b-{tag}"));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for ws in [&ws_a, &ws_b] {
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)",
                rusqlite::params![ws, format!("/tmp/k2-egress-test/{ws}")],
            )
            .unwrap();
        }
        for (app, ws, iso) in [("jobs", &ws_a, "container"), ("other", &ws_b, "container"), ("plain", &ws_a, "process")] {
            store::insert_app(
                &conn,
                &store::NewApp {
                    app_id: app,
                    service_id: &format!("svc-{app}-{tag}"),
                    seed_id: None,
                    owner_workspace: ws,
                    isolation: iso,
                    memory_limit_bytes: None,
                    front_port: 0,
                    created_by: "owner",
                },
            )
            .unwrap();
        }
    }
    super::super::alerts::set_sink(Some(Box::new(|ws: &str, text: &str| MSGS.lock().unwrap().push((ws.into(), text.into())))));
    super::super::egress_proxy::set_own_names_for_test(Some(vec!["rosson.k2.dev".into()]));
    Fx { _db: db, _home: home, ws_a, ws_b }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn shell(ws: &str) -> Who {
    Who::Shell { project_id: ws.into(), session_id: format!("tab-{ws}") }
}

fn admin() -> Who {
    Who::Login { username: "ada".into(), role: Role::Admin }
}

fn owner_login() -> Who {
    Who::Login { username: "rosson".into(), role: Role::Owner }
}

fn call(path: &str, who: &Who, body: J) -> Result<J, (u16, &'static str, String)> {
    handle(path, who, body.to_string().as_bytes(), &HashMap::new()).map_err(|r| (r.status, r.code, r.hint))
}

fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> Result<J, (u16, &'static str, String)> {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    handle(path, who, b"", &p).map_err(|r| (r.status, r.code, r.hint))
}

fn token_for(req: &str, stamp: &str) -> String {
    test_seams::vault_handle().confirm.mint(CONFIRM_ACTION, Some(req), stamp, "local-auth")
}

fn allowed(app: &str) -> Vec<String> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    pol::policy(&conn, app).unwrap().allowed.iter().map(HostSpec::display).collect()
}

fn msgs_to(ws: &str) -> Vec<String> {
    MSGS.lock().unwrap().iter().filter(|(w, _)| w == ws).map(|(_, t)| t.clone()).collect()
}

#[test]
fn an_agent_proposes_and_only_the_owner_in_person_approves() {
    let f = fx("approve");
    let a = agent(&f.ws_a);
    // Propose: held, not live.
    let r = call("/cli/app/egress/add", &a, json!({"app": "jobs", "hosts": ["API.stripe.com", "hooks.slack.com:443"], "reason": "payments"})).unwrap();
    assert_eq!(r["status"], "held");
    let id = r["request"]["id"].as_str().unwrap().to_string();
    assert_eq!(r["request"]["hosts"], json!(["api.stripe.com:443", "hooks.slack.com:443"]));
    assert!(r["hint"].as_str().unwrap().contains(&format!("k2 app approve {id}")));
    assert!(allowed("jobs").is_empty(), "pending is never live");
    let shown = get("/cli/app/egress", &a, &[("app", "jobs")]).unwrap();
    assert_eq!(shown["enforced"], true);
    assert_eq!(shown["pending"][0]["id"], id.as_str());
    assert_eq!(shown["hosts"][0]["status"], "pending");

    // Requests: the agent sees its own workspace's; another agent sees none.
    let mine = get("/cli/app/approvals", &a, &[]).unwrap();
    assert_eq!(mine["requests"].as_array().unwrap().len(), 1);
    assert!(get("/cli/app/approvals", &agent(&f.ws_b), &[]).unwrap()["requests"].as_array().unwrap().is_empty());

    // Never an agent, a K2 shell tab or an admin; the owner needs a confirm.
    for who in [a.clone(), shell(&f.ws_a), admin()] {
        let e = call("/cli/app/approve", &who, json!({"request": id, "confirmToken": token_for(&id, "owner")})).unwrap_err();
        assert_eq!(e.1, codes::OWNER_ONLY, "{who:?}");
    }
    let e = call("/cli/app/approve", &Who::Owner, json!({"request": id})).unwrap_err();
    assert_eq!(e.1, codes::OWNER_CONFIRM_REQUIRED, "the owner token alone is not an approval (AR14)");
    let e = call("/cli/app/approve", &Who::Owner, json!({"request": id, "confirmToken": token_for("w_other", "owner")})).unwrap_err();
    assert_eq!(e.1, codes::OWNER_CONFIRM_REQUIRED, "bound to the request");
    let wrong_action = test_seams::vault_handle().confirm.mint("node-admin", Some(&id), "owner", "local-auth");
    assert_eq!(call("/cli/app/approve", &Who::Owner, json!({"request": id, "confirmToken": wrong_action})).unwrap_err().1, codes::OWNER_CONFIRM_REQUIRED);
    let e = call("/cli/app/approve", &owner_login(), json!({"request": id, "confirmToken": token_for(&id, "owner")})).unwrap_err();
    assert_eq!(e.1, codes::OWNER_CONFIRM_REQUIRED, "bound to the confirming actor");

    // The owner, confirmed: live now; the agent hears.
    let tok = token_for(&id, "owner");
    let r = call("/cli/app/approve", &Who::Owner, json!({"request": &id[..8], "confirmToken": tok})).unwrap();
    assert_eq!(r["request"]["status"], "approved");
    assert_eq!(r["request"]["confirmVia"], "local-auth");
    assert_eq!(allowed("jobs"), vec!["api.stripe.com:443", "hooks.slack.com:443"]);
    assert!(msgs_to(&f.ws_a).iter().any(|m| m.contains("approved") && m.contains(&id)));
    // Single use; and decided is decided.
    let e = call("/cli/app/approve", &Who::Owner, json!({"request": id, "confirmToken": tok})).unwrap_err();
    assert_eq!(e.1, codes::OWNER_CONFIRM_REQUIRED);
    let e = call("/cli/app/approve", &Who::Owner, json!({"request": id, "confirmToken": token_for(&id, "owner")})).unwrap_err();
    assert_eq!((e.0, e.1), (409, codes::NOT_PENDING));

    // The audit chain has the request and the approval.
    let rows = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        deploy::audit::list_for(&conn, deploy::audit::SUBJECT_APP, "jobs", 50).unwrap()
    };
    let events: Vec<String> = rows.iter().map(|r| r.event.clone()).collect();
    assert!(events.contains(&"egress_requested".to_string()) && events.contains(&"widening_approved".to_string()), "{events:?}");
}

#[test]
fn every_caller_is_held_even_the_owner_and_asking_twice_joins() {
    let f = fx("held");
    let r = call("/cli/app/egress/add", &Who::Owner, json!({"app": "jobs", "host": "api.github.com"})).unwrap();
    assert_eq!(r["status"], "held", "a new egress host is per-event for every caller (K25, AR14)");
    let id = r["request"]["id"].as_str().unwrap().to_string();
    let r = call("/cli/app/egress/add", &Who::Owner, json!({"app": "jobs", "host": "uploads.github.com"})).unwrap();
    assert_eq!(r["request"]["id"], id.as_str(), "the same requester's open request grows");
    assert_eq!(r["request"]["hosts"].as_array().unwrap().len(), 2);
    let r = call("/cli/app/egress/add", &shell(&f.ws_a), json!({"app": "jobs", "host": "x.example.com"})).unwrap();
    assert_ne!(r["request"]["id"], id.as_str(), "another requester files its own");
    assert!(allowed("jobs").is_empty());
}

#[test]
fn bad_own_and_private_hosts_are_refused_at_the_door() {
    let f = fx("bad");
    let a = agent(&f.ws_a);
    for (host, code) in [
        ("https://api.stripe.com", codes::EGRESS_HOST_INVALID),
        ("localhost:8080", codes::EGRESS_HOST_INVALID),
        ("10.0.0.5:443", codes::EGRESS_HOST_INVALID),
        ("169.254.169.254:80", codes::EGRESS_HOST_INVALID),
        ("[::1]:443", codes::EGRESS_HOST_INVALID),
        ("rosson.k2.dev", codes::EGRESS_SELF),
        ("jobs.rosson.k2.dev", codes::EGRESS_SELF),
    ] {
        let e = call("/cli/app/egress/add", &a, json!({"app": "jobs", "host": host})).unwrap_err();
        assert_eq!(e.1, code, "{host}");
    }
    assert_eq!(call("/cli/app/egress/add", &a, json!({"app": "jobs"})).unwrap_err().0, 400);
    // A foreign App doesn't exist for this agent.
    let e = call("/cli/app/egress/add", &a, json!({"app": "other", "host": "api.stripe.com"})).unwrap_err();
    assert_eq!(e.1, dcodes::APP_NOT_FOUND);
    assert_eq!(get("/cli/app/egress", &a, &[("app", "other")]).unwrap_err().1, dcodes::APP_NOT_FOUND);
    // App passes and bad tokens: refused.
    assert_eq!(call("/cli/app/egress/add", &Who::Wrong, json!({"app": "jobs", "host": "a.example.com"})).unwrap_err().1, dcodes::WRONG_CREDENTIAL);
}

#[test]
fn narrowing_is_immediate_and_withdraws_an_empty_request() {
    let f = fx("narrow");
    let a = agent(&f.ws_a);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        pol::allow(&conn, "jobs", &[pol::parse_spec("api.stripe.com").unwrap()], pol::SOURCE_REQUEST, "owner").unwrap();
    }
    assert_eq!(allowed("jobs"), vec!["api.stripe.com:443"]);
    // An agent narrows at once, no switch, no owner.
    let r = call("/cli/app/egress/rm", &a, json!({"app": "jobs", "host": "api.stripe.com:443"})).unwrap();
    assert_eq!(r["removed"], true);
    assert!(allowed("jobs").is_empty());
    assert_eq!(call("/cli/app/egress/rm", &a, json!({"app": "jobs", "host": "api.stripe.com"})).unwrap()["removed"], false);
    // Removing the only pending host withdraws its request.
    let r = call("/cli/app/egress/add", &a, json!({"app": "jobs", "host": "a.example.com"})).unwrap();
    let id = r["request"]["id"].as_str().unwrap().to_string();
    call("/cli/app/egress/rm", &a, json!({"app": "jobs", "host": "a.example.com"})).unwrap();
    let all = get("/cli/app/approvals", &a, &[("state", "all")]).unwrap();
    let w = all["requests"].as_array().unwrap().iter().find(|w| w["id"] == id.as_str()).unwrap().clone();
    assert_eq!(w["status"], "withdrawn");
    // Mode: allowlist is narrowing; broad is the open path's.
    assert_eq!(call("/cli/app/egress/mode", &a, json!({"app": "jobs", "mode": "allowlist"})).unwrap()["mode"], "allowlist");
    let e = call("/cli/app/egress/mode", &Who::Owner, json!({"app": "jobs", "mode": "broad"})).unwrap_err();
    assert_eq!(e.1, codes::OPEN_PATH_ONLY);
    assert_eq!(call("/cli/app/egress/mode", &a, json!({"app": "jobs", "mode": "wide"})).unwrap_err().0, 400);
    // A process-engine App: listed but not enforced, and it says so.
    let shown = get("/cli/app/egress", &a, &[("app", "plain")]).unwrap();
    assert_eq!(shown["enforced"], false);
    assert!(shown["note"].as_str().unwrap().contains("not enforced"));
}

#[test]
fn deny_is_for_admins_withdraw_for_the_requester() {
    let f = fx("deny");
    let a = agent(&f.ws_a);
    let id = call("/cli/app/egress/add", &a, json!({"app": "jobs", "host": "a.example.com"})).unwrap()["request"]["id"].as_str().unwrap().to_string();
    // Another workspace's agent can't even see it.
    assert_eq!(call("/cli/app/deny", &agent(&f.ws_b), json!({"request": id})).unwrap_err().1, codes::REQUEST_NOT_FOUND);
    // A Member login can't deny.
    let member = Who::Login { username: "mo".into(), role: Role::Member };
    assert_eq!(call("/cli/app/deny", &member, json!({"request": id})).unwrap_err().1, codes::OWNER_ONLY);
    // An admin denies; the agent hears why; the pending row is gone.
    let r = call("/cli/app/deny", &admin(), json!({"request": id, "reason": "use the vault proxy"})).unwrap();
    assert_eq!(r["request"]["status"], "denied");
    assert_eq!(r["request"]["decisionReason"], "use the vault proxy");
    assert!(msgs_to(&f.ws_a).iter().any(|m| m.contains("denied") && m.contains("use the vault proxy")));
    assert!(get("/cli/app/egress", &a, &[("app", "jobs")]).unwrap()["hosts"].as_array().unwrap().is_empty());
    // The requester withdraws its own.
    let id = call("/cli/app/egress/add", &a, json!({"app": "jobs", "host": "b.example.com"})).unwrap()["request"]["id"].as_str().unwrap().to_string();
    assert_eq!(call("/cli/app/deny", &a, json!({"request": id})).unwrap()["request"]["status"], "withdrawn");
    assert_eq!(call("/cli/app/deny", &a, json!({"request": id})).unwrap_err().1, codes::NOT_PENDING);
    let _ = f.ws_b;
}

#[test]
fn an_expired_request_cant_be_approved() {
    let f = fx("expire");
    let id = call("/cli/app/egress/add", &agent(&f.ws_a), json!({"app": "jobs", "host": "a.example.com"})).unwrap()["request"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("UPDATE app_widenings SET expires_at = 1 WHERE id = ?1", rusqlite::params![id]).unwrap();
    }
    let e = call("/cli/app/approve", &Who::Owner, json!({"request": id, "confirmToken": token_for(&id, "owner")})).unwrap_err();
    assert_eq!((e.0, e.1), (410, codes::REQUEST_EXPIRED));
    assert!(allowed("jobs").is_empty());
}
