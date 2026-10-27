//! A held App request says where the owner sees it (0.46.4 field reports
//! #9–#11), headless: the real dispatcher in process on a temp `$HOME`.
//!
//! - an agent's held `k2 app install` (no "Build and run Apps": S42)
//!   answers `card` (cards off: where it waits, App Approvals) and
//!   `reused: false`;
//! - asked again later, it is the same request: `reused: true` with its
//!   age, nothing new filed;
//! - permission cards turned on after the first hold: the repeat raises
//!   the `apps.approve` card for that same request (`card.raised`, a new
//!   card), and the next repeat finds it (`new: false`, the same card); no
//!   "Ask with" teaching object rides an answer whose card is up;
//! - the owner's `/cli/app/approvals` lists it with `pendingCount`, and
//!   every filing pushes `app_approvals_changed {pending}` (the badge).

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::time::Duration;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use k2_daemon::session_events::SessionEvent;
use serde_json::json;

fn pin_chat(ws: &Ws) {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn held_app_requests_say_where_the_owner_sees_them() {
    let env = setup(&[]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "heldws");
    pin_chat(&ws);
    allow_manage(&ws, false);
    let agent = passport(&ws);
    let mut events = k2_daemon::session_events::subscribe();

    // Cards off: held, where it waits, a new request.
    let r = post(d.port, "/cli/app/install", &agent, json!({ "app": "heldapp" })).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let v = r.json();
    assert_eq!(v["status"], "held", "{v}");
    let id = v["request"]["id"].as_str().expect("request id").to_string();
    assert_eq!(v["reused"], false, "{v}");
    assert_eq!((v["card"]["raised"].as_bool(), v["card"]["cardsOff"].as_bool()), (Some(false), Some(true)), "{v}");
    assert!(v["card"]["where"].as_str().unwrap().contains("App Approvals"), "{v}");
    assert!(v.get("permission").is_none(), "cards off: no teaching object: {v}");

    // The push: the badge count.
    let pending = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await {
                Ok(SessionEvent::AppApprovalsChanged { pending }) => break pending,
                Ok(_) => continue,
                Err(e) => panic!("session events: {e}"),
            }
        }
    })
    .await
    .expect("app_approvals_changed pushed on the hold");
    assert_eq!(pending, 1);

    // The owner's list has it, with the count.
    let r = get(d.port, "/cli/app/approvals", OWNER, "state=pending").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let l = r.json();
    assert_eq!(l["pendingCount"], 1, "{l}");
    assert_eq!(l["requests"][0]["id"], id.as_str(), "{l}");

    // Asked again an hour later: the same request, said so with its age.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let n = conn.execute("UPDATE app_widenings SET requested_at = requested_at - 3600 WHERE id = ?1", [&id]).expect("backdate");
        assert_eq!(n, 1);
    }
    let r = post(d.port, "/cli/app/install", &agent, json!({ "app": "heldapp" })).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let v = r.json();
    assert_eq!(v["request"]["id"], id.as_str(), "the same request: {v}");
    assert_eq!((v["reused"].as_bool(), v["askedAgo"].as_str()), (Some(true), Some("1 h")), "{v}");

    // The owner turns permission cards on (server + this agent) after the
    // first hold: the repeat raises the card for that same request.
    k2_core::permission::gate::set_server_switch(true);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let n = conn.execute("UPDATE projects SET agents_can_request_permissions = 1 WHERE id = ?1", [&ws.id]).expect("agent switch");
        assert_eq!(n, 1);
    }
    let r = post(d.port, "/cli/app/install", &agent, json!({ "app": "heldapp" })).await;
    let raised = r.json();
    let r2 = post(d.port, "/cli/app/install", &agent, json!({ "app": "heldapp" })).await;
    let again = r2.json();
    k2_core::permission::gate::set_server_switch(false);
    assert_eq!(r.status, 202, "{}", r.body);
    assert_eq!(raised["request"]["id"], id.as_str(), "{raised}");
    assert_eq!((raised["card"]["raised"].as_bool(), raised["card"]["new"].as_bool()), (Some(true), Some(true)), "{raised}");
    let card = raised["card"]["id"].as_str().expect("card id").to_string();
    assert!(raised["card"]["thread"].as_str().is_some_and(|t| !t.is_empty()), "{raised}");
    assert!(raised.get("permission").is_none(), "the card is up: no Ask-with object: {raised}");
    assert_eq!(r2.status, 202, "{}", r2.body);
    assert_eq!((again["card"]["raised"].as_bool(), again["card"]["new"].as_bool()), (Some(true), Some(false)), "{again}");
    assert_eq!(again["card"]["id"], card.as_str(), "the same card: {again}");
    assert!(again["card"]["where"].as_str().unwrap().contains(&card), "{again}");

    // Still one request waiting.
    let r = get(d.port, "/cli/app/approvals", OWNER, "state=pending").await;
    assert_eq!(r.json()["pendingCount"], 1, "{}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}
