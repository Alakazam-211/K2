//! `apps.chat-key` and `sew.space-governance` walls, with the real switches
//! (lib tests never flip the process-wide one), headless over loopback:
//!
//! - cards off, and cards on with the agent's own switch off: an agent's
//!   `k2 app grant <app> --kind chat-key` and `k2 hostchat app-space
//!   governance` refusals are today's, byte for byte, with no extra field
//!   and no `permission`;
//! - both on: the chat key's refusal names the App and what was asked, and
//!   both teach the exact Ask-with line;
//! - another workspace's App stays "It can't be requested"; a person is
//!   never taught a card.
//!
//! The cards themselves are tested in `permission/actions/chat_cards_tests.rs`.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use serde_json::{json, Value};

const KEY: &str = "/cli/app/chat-key";
const GOV: &str = "/cli/sew/app-spaces/governance";

struct SwitchOff;
impl Drop for SwitchOff {
    fn drop(&mut self) {
        k2_core::permission::gate::set_server_switch(false);
    }
}

fn agent_switch(ws: &Ws, on: bool) {
    k2_core::workspace::settings::set_agents_can_request_permissions(&ws.id, on).expect("agent switch");
}

fn without_permission(body: &str) -> Value {
    let mut v: Value = serde_json::from_str(body).unwrap_or_else(|e| panic!("json ({e}): {body}"));
    v.as_object_mut().expect("an object").remove("permission");
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_chat_refusals_teach_their_cards_only_while_both_switches_are_on() {
    let env = setup(&[]);
    let _off = SwitchOff;
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "chatcard");
    let other = seed_ws(&env, "chatother");
    let (name, svc, theirs) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mine = k2_core::published_services::insert(&conn, &ws.id, "jobsk", "node s.mjs", &ws.path.to_string_lossy(), 47_411, "local", "stopped", "cmd", "").expect("service");
        let theirs = k2_core::published_services::insert(&conn, &other.id, "crmk", "node s.mjs", &other.path.to_string_lossy(), 47_412, "local", "stopped", "cmd", "").expect("service");
        (mine.name, mine.id, theirs.name)
    };
    k2_core::workspace::settings::set_agents_can_manage_chat(&ws.id, true).expect("Manage Sew");
    let agent = passport(&ws);
    let key_body = json!({ "app": name, "caps": ["chat:read", "voice:join"], "maxLevel": "admin" });
    let key_today = json!({ "error": { "code": "owner_only", "hint":
        "only the server's owner, in person, gives an App a chat key (k2 app grant <app> --kind chat-key); an agent may only narrow or revoke its own App's key (the owner token or an Owner login plus a confirm; never an agent)"
    } })
    .to_string();
    let gov_body = json!({ "app": name, "spaceId": "5101", "governance": "app" });

    // Off: today's bytes, no permission.
    let r = post(port, KEY, &agent, key_body.clone()).await;
    assert_eq!((r.status, r.body.as_str()), (403, key_today.as_str()), "cards off");
    let gov_off = post(port, GOV, &agent, gov_body.clone()).await;
    assert_eq!((gov_off.status, gov_off.code()), (403, "person_only".to_string()), "{}", gov_off.body);
    assert!(gov_off.json().get("permission").is_none(), "{}", gov_off.body);
    // On, with the agent's own switch off: the same.
    k2_core::permission::gate::set_server_switch(true);
    agent_switch(&ws, false);
    let r = post(port, KEY, &agent, key_body.clone()).await;
    assert_eq!((r.status, r.body.as_str()), (403, key_today.as_str()), "the agent's switch off");
    let r = post(port, GOV, &agent, gov_body.clone()).await;
    assert_eq!((r.status, r.body.as_str()), (403, gov_off.body.as_str()), "the agent's switch off");

    // Both on: the exact lines to ask with.
    agent_switch(&ws, true);
    let r = post(port, KEY, &agent, key_body.clone()).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let v = r.json();
    assert_eq!(v["error"]["code"], "owner_only", "{v}");
    assert_eq!((v["error"]["app"].as_str(), v["error"]["level"].as_str(), v["error"]["caps"].as_str()), (Some("jobsk"), Some("admin"), Some("chat:read,voice:join")), "{v}");
    assert_eq!(v["permission"]["action"], "apps.chat-key", "{v}");
    assert_eq!(v["permission"]["approvers"], "owner or admin", "{v}");
    assert_eq!(v["permission"]["command"], "k2 permission request apps.chat-key app=jobsk caps=chat:read,voice:join level=admin --why \"<what you need it for>\"", "{v}");
    let r = post(port, GOV, &agent, gov_body.clone()).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(without_permission(&r.body), gov_off.json(), "the route's own body is unchanged");
    let v = r.json();
    assert_eq!(v["permission"]["action"], "sew.space-governance", "{v}");
    assert_eq!(v["permission"]["command"], "k2 permission request sew.space-governance app=jobsk space=5101 to=app --why \"<what you need it for>\"", "{v}");

    // Another workspace's App: not this card's wall.
    let r = post(port, KEY, &agent, json!({ "app": theirs, "caps": ["chat:read"] })).await;
    assert_ne!(r.json()["permission"]["action"], json!("apps.chat-key"), "{}", r.body);
    assert!(r.json()["error"].get("app").is_none(), "{}", r.body);
    // A person is never taught a card.
    let r = post(port, KEY, OWNER, json!({ "app": svc, "caps": ["chat:read"] })).await;
    assert!(r.json().get("permission").is_none(), "{}", r.body);
    drop(env);
}
