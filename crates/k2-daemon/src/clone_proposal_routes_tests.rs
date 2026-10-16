//! Clone proposals: agents prepare, a person approves (CL21, §4.4, test
//! plan 18). Loud: every refusal is checked by code.

use std::sync::Mutex;

use super::*;
use k2_core::connect_users::Role;
use serde_json::json;

static MSGS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    ws_a: String,
    ws_b: String,
    handle_a: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    crate::vault::ensure_test_vault();
    let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let (ws_a, ws_b) = (format!("cc-a-{tag}-{suffix}"), format!("cc-b-{tag}-{suffix}"));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for ws in [&ws_a, &ws_b] {
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)",
                rusqlite::params![ws, format!("/tmp/k2-cc-proposal-test/{ws}")],
            )
            .expect("project");
        }
        // A pinned main Chat for A, so the proposal notice lands on its Thread.
        k2_core::db::schema::WorkspaceSession::upsert(
            &conn,
            &format!("ws-{ws_a}"),
            &ws_a,
            None,
            Some(&format!("conv-{ws_a}")),
            "claude",
            "system",
            "running",
        )
        .expect("pin");
    }
    notify::set_sink(Some(Box::new(|ws: &str, text: &str| MSGS.lock().unwrap().push((ws.into(), text.into())))));
    Fx { _db: db, _home: home, handle_a: ws_a.clone(), ws_a, ws_b }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn call(path: &str, who: &Who, body: J) -> (String, J) {
    let r = handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new(), "test");
    let v: J = serde_json::from_str(&r.body).expect("json body");
    (r.status.to_string(), v)
}

fn list(who: &Who, q: &[(&str, &str)]) -> J {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = handle(LIST, who.clone(), b"", &p, "test");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    serde_json::from_str(&r.body).expect("json")
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().expect("refusal code")
}

fn token_for(id: &str, stamp: &str) -> String {
    crate::vault::ensure_test_vault().confirm.mint(CONFIRM_ACTION, Some(id), stamp, "local-auth")
}

fn msgs_to(ws: &str) -> Vec<String> {
    MSGS.lock().unwrap().iter().filter(|(w, _)| w == ws).map(|(_, t)| t.clone()).collect()
}

#[test]
fn an_agent_proposes_its_own_workspace_and_a_person_approves_with_a_confirm() {
    let f = fx("approve");
    let a = agent(&f.ws_a);
    let (st, v) = call(
        NEW,
        &a,
        json!({"to": "luzz.k2.dev", "copy": ["STRIPE_KEY"], "skip": "GMAIL_TOKEN", "sinceDays": 30, "why": "moving to the new box"}),
    );
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["status"], "held");
    let id = v["proposal"]["id"].as_str().expect("id").to_string();
    assert_eq!(v["proposal"]["projectId"], f.ws_a.as_str());
    assert_eq!(v["proposal"]["options"]["secrets"], json!({"GMAIL_TOKEN": "skip", "STRIPE_KEY": "copy"}));
    assert_eq!(v["proposal"]["options"]["sinceDays"], 30);
    assert!(v["proposal"]["proposedBy"].as_str().expect("by").starts_with(&format!("agent:{}/", f.handle_a)));
    assert_eq!(v["threadNotice"], true, "the human sees one Thread line: {v}");
    assert!(v["hint"].as_str().expect("hint").contains(&format!("k2 clone approve {id}")));

    // The agent lists its own; another agent sees none of it.
    assert_eq!(list(&a, &[])["proposals"].as_array().expect("rows").len(), 1);
    assert!(list(&agent(&f.ws_b), &[])["proposals"].as_array().expect("rows").is_empty());

    // Never an agent, a K2 shell tab or a Member; a person needs a confirm.
    let shell = Who::Shell { project_id: f.ws_a.clone(), session_id: "tab".into() };
    for who in [a.clone(), shell] {
        let (st, v) = call(APPROVE, &who, json!({"proposal": id, "confirmToken": token_for(&id, "owner")}));
        assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", codes::PERSON_ONLY), "{who:?}");
    }
    let member = Who::Login { username: "mo".into(), role: Role::Member };
    let (_, v) = call(APPROVE, &member, json!({"proposal": id}));
    assert_eq!(code(&v), codes::FORBIDDEN);
    let (_, v) = call(APPROVE, &Who::Owner, json!({"proposal": id}));
    assert_eq!(code(&v), codes::CONFIRM_REQUIRED, "the owner token alone is not an approval");
    let (_, v) = call(APPROVE, &Who::Owner, json!({"proposal": id, "confirmToken": token_for("cp_other", "owner")}));
    assert_eq!(code(&v), codes::CONFIRM_REQUIRED, "bound to the proposal id");
    let wrong_action = crate::vault::ensure_test_vault().confirm.mint("app-approve", Some(&id), "owner", "local-auth");
    let (_, v) = call(APPROVE, &Who::Owner, json!({"proposal": id, "confirmToken": wrong_action}));
    assert_eq!(code(&v), codes::CONFIRM_REQUIRED, "bound to clone-approve");

    let (st, v) = call(APPROVE, &Who::Owner, json!({"proposal": id, "confirmToken": token_for(&id, "owner")}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["proposal"]["status"], "approved");
    assert_eq!(v["run"]["target"], "luzz.k2.dev");
    assert_eq!(v["run"]["workspacePath"], format!("/tmp/k2-cc-proposal-test/{}", f.ws_a));
    assert_eq!(v["run"]["options"]["secrets"]["STRIPE_KEY"], "copy");
    assert!(msgs_to(&f.ws_a).iter().any(|m| m.contains("approved") && m.contains(&id)), "the agent is told");

    // Once only; then the person records how the run went.
    let (_, v) = call(APPROVE, &Who::Owner, json!({"proposal": id, "confirmToken": token_for(&id, "owner")}));
    assert_eq!(code(&v), codes::NOT_OPEN);
    let (_, v) = call(FINISH, &a, json!({"proposal": id, "ok": true}));
    assert_eq!(code(&v), codes::PERSON_ONLY);
    let (st, v) = call(FINISH, &Who::Owner, json!({"proposal": id, "ok": true}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["proposal"]["status"], "done");
    assert!(msgs_to(&f.ws_a).iter().any(|m| m.contains("is done")));
}

#[test]
fn an_admin_login_approves_with_its_own_confirm_stamp() {
    let f = fx("admin");
    k2_core::connect_users::add_user("ada", "password1").expect("add");
    k2_core::connect_users::set_role("ada", Role::Admin).expect("admin");
    let (_, v) = call(NEW, &agent(&f.ws_a), json!({"to": "here", "why": "a test copy"}));
    let id = v["proposal"]["id"].as_str().expect("id").to_string();
    let ada = Who::Login { username: "ada".into(), role: Role::Admin };
    let (_, v) = call(APPROVE, &ada, json!({"proposal": id, "confirmToken": token_for(&id, "owner")}));
    assert_eq!(code(&v), codes::CONFIRM_REQUIRED, "a confirm minted for someone else");
    // "Can clone" off → the clone gate refuses the approval itself.
    k2_core::connect_users::set_can_clone("ada", false).expect("off");
    let (st, v) = call(APPROVE, &ada, json!({"proposal": id, "confirmToken": token_for(&id, "connect:ada")}));
    assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", crate::clone_gate::codes::NOT_ALLOWED), "{v}");
    k2_core::connect_users::set_can_clone("ada", true).expect("on");
    let (st, v) = call(APPROVE, &ada, json!({"proposal": id, "confirmToken": token_for(&id, "connect:ada")}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["proposal"]["decidedBy"], "login:ada");
}

#[test]
fn agents_propose_only_for_their_own_workspace_and_must_say_why() {
    let f = fx("own");
    let a = agent(&f.ws_a);
    let (st, v) = call(NEW, &a, json!({"to": "here", "workspace": f.ws_b, "why": "x"}));
    assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", codes::FORBIDDEN));
    let (_, v) = call(NEW, &a, json!({"to": "here"}));
    assert_eq!(code(&v), codes::BAD_REQUEST, "why is required for an agent");
    let (_, v) = call(NEW, &a, json!({"to": "bad host!", "why": "x"}));
    assert_eq!(code(&v), codes::BAD_REQUEST);
    let (_, v) = call(NEW, &a, json!({"to": "here", "why": "x", "copy": ["A"], "skip": ["A"]}));
    assert_eq!(code(&v), codes::BAD_REQUEST, "one name, two choices");
    let (_, v) = call(NEW, &a, json!({"to": "here", "why": "x", "copy": ["sk_live=abc"]}));
    assert_eq!(code(&v), codes::BAD_REQUEST, "names only");
    // A person names the workspace.
    let (st, v) = call(NEW, &Who::Owner, json!({"to": "here", "workspace": f.ws_b}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["proposal"]["projectId"], f.ws_b.as_str());
    assert_eq!(call(NEW, &Who::Wrong, json!({"to": "here"})).0, "403 Forbidden");
}

#[test]
fn withdraw_and_deny_follow_who_owns_the_proposal() {
    let f = fx("withdraw");
    let (_, v) = call(NEW, &agent(&f.ws_a), json!({"to": "here", "why": "x"}));
    let id = v["proposal"]["id"].as_str().expect("id").to_string();
    let (st, v) = call(WITHDRAW, &agent(&f.ws_b), json!({"proposal": id}));
    assert_eq!((st.as_str(), code(&v)), ("404 Not Found", codes::NOT_FOUND), "never another workspace's");
    let (st, v) = call(WITHDRAW, &agent(&f.ws_a), json!({"proposal": id}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["proposal"]["status"], "withdrawn");

    let (_, v) = call(NEW, &agent(&f.ws_a), json!({"to": "here", "why": "again"}));
    let id2 = v["proposal"]["id"].as_str().expect("id").to_string();
    let (_, v) = call(DENY, &agent(&f.ws_a), json!({"proposal": id2}));
    assert_eq!(code(&v), codes::PERSON_ONLY);
    let (st, v) = call(DENY, &Who::Owner, json!({"proposal": id2, "reason": "not now"}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["proposal"]["status"], "denied");
    assert!(msgs_to(&f.ws_a).iter().any(|m| m.contains("denied") && m.contains("not now")));
    let all = list(&Who::Owner, &[("status", "all"), ("workspace", &f.ws_a)]);
    assert_eq!(all["proposals"].as_array().expect("rows").len(), 2);
}

#[test]
fn routes_are_wired_for_passports_and_people() {
    for p in AGENT_ROUTES {
        assert!(crate::session_token::is_agent_verb(p), "{p} must be on the passport allowlist");
    }
    for p in [APPROVE, DENY, FINISH] {
        assert!(!crate::session_token::is_agent_verb(p), "{p} is people only");
    }
    for p in POST_ROUTES {
        assert!(crate::routes::route_policy::post_allowed(p), "{p} POST allowlisted");
    }
    assert!(crate::routes::route_policy::lookup(LIST).is_some());
    assert!(crate::routes::route_policy::post_allowed("/cli/clone/preview"));
    assert!(crate::routes::route_policy::post_allowed("/cli/users/set-can-clone"));
}
