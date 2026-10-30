//! `app.login-grant`'s wall and the agent's read, through the real
//! dispatcher with the real switches (lib tests never flip the process-wide
//! one): an App's own agent runs `k2 app grant add … --kind app`.
//!
//! - cards off, and on with the agent's own switch off: today's `owner_only`
//!   refusal, the same bytes, no `permission`;
//! - both on: the same refusal plus the Ask-with line naming the person, the
//!   App and the role; another kind of grant stays "It can't be requested";
//! - `GET /cli/skin/grants/own` lists the logins on the agent's own
//!   workspace's Apps only; `GET /cli/skin/grants` stays the owner's.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use serde_json::{json, Value};

const GRANTS: &str = "/cli/skin/grants";

struct SwitchOff;
impl Drop for SwitchOff {
    fn drop(&mut self) {
        k2_core::permission::gate::set_server_switch(false);
    }
}

fn service(ws: &Ws, name: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::insert(&conn, &ws.id, name, "node s.mjs", &ws.path.to_string_lossy(), 47_601, "local", "stopped", "cmd", "").expect("service").id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_refusal_teaches_the_login_card_and_the_agent_reads_its_own_apps_logins() {
    let env = setup(&[]);
    let _off = SwitchOff;
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "loginagent");
    let other = seed_ws(&env, "loginother");
    let jobs = service(&ws, "jobs");
    let theirs = service(&other, "theirs");
    k2_core::skin::add_principal_for_app("dana", None, &theirs, None, "owner-token").expect("a person on the box");
    k2_core::skin::add_principal_for_app("erin", None, &theirs, None, "owner-token").expect("another workspace's guest");
    k2_core::skin::add_principal_for_app("finn", None, &jobs, None, "owner-token").expect("a guest of its own App");
    let agent = passport(&ws);
    let body = json!({ "subjectKind": "principal", "subjectId": "dana", "kind": "app", "targetId": "jobs", "roleId": "viewer" });

    // Off, and on with the agent's own switch off: the same bytes.
    let off = post(port, GRANTS, &agent, body.clone()).await;
    assert_eq!(off.status, 403, "{}", off.body);
    assert_eq!(off.code(), "owner_only", "{}", off.body);
    assert!(!off.body.contains("\"permission\""), "{}", off.body);
    k2_core::permission::gate::set_server_switch(true);
    let r = post(port, GRANTS, &agent, body.clone()).await;
    assert_eq!((r.status, r.body.as_str()), (403, off.body.as_str()), "the agent's switch off");

    // Both on: today's refusal, then the line to ask with.
    k2_core::workspace::settings::set_agents_can_request_permissions(&ws.id, true).expect("agent switch");
    let r = post(port, GRANTS, &agent, body.clone()).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let head = off.body.trim_end().trim_end_matches('}');
    assert!(r.body.starts_with(head), "the refusal's own text is unchanged:\n{}\n{}", off.body, r.body);
    let v: Value = r.json();
    assert_eq!(v["permission"]["action"], "app.login-grant", "{v}");
    assert_eq!(v["permission"]["approvers"], "owner", "{v}");
    assert_eq!(v["permission"]["command"], "k2 permission request app.login-grant app=jobs op=add role=viewer user=dana --why \"<what you need it for>\"", "{v}");
    let r = post(port, "/cli/skin/grants/delete", &agent, json!({ "subjectKind": "principal", "subjectId": "dana", "kind": "app", "targetId": "jobs" })).await;
    assert_eq!(r.json()["permission"]["command"], "k2 permission request app.login-grant app=jobs op=remove user=dana --why \"<what you need it for>\"", "{}", r.body);
    // Another kind of grant: not this card.
    let r = post(port, GRANTS, &agent, json!({ "subjectKind": "principal", "subjectId": "dana", "kind": "mailbox", "targetId": "x", "scope": "y" })).await;
    assert_eq!(r.json()["permission"]["requestable"], json!(false), "{}", r.body);

    // The read: its own workspace's Apps only; the owner's list stays the owner's.
    let r = get(port, "/cli/skin/grants/own", &agent, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let rows = r.json()["grants"].as_array().cloned().expect("rows");
    assert!(rows.iter().any(|g| g["username"] == "finn" && g["targetId"] == jobs.as_str()), "{}", r.body);
    assert!(rows.iter().all(|g| g["targetId"] == jobs.as_str()), "{}", r.body);
    assert!(!r.body.contains("erin") && !r.body.contains(&theirs), "another workspace's: {}", r.body);
    let r = get(port, GRANTS, &agent, "").await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    let r = get(port, "/cli/skin/grants/own", OWNER, "").await;
    assert_eq!(r.status, 403, "the owner reads the whole list instead: {}", r.body);
    drop(env);
}
