//! Dismissed and withdrawn Thread cards, headless, through the REAL
//! dispatcher on a test daemon (in-process, temp `$HOME`, loopback HTTP:
//! what `curl` against a headless `k2-daemon` sees; never this machine's
//! daemon). prd-popup-close-and-card-dismiss-v1 B1–B3, 0.46.11.
//!
//! 1. `/cli/thread/void` is POST-only (a GET is 405 and changes nothing).
//! 2. A person's dismiss with a reason is stored by the daemon and served
//!    to every reader: `status: voided`, `closedAs: dismissed`,
//!    `closeReason`.
//! 3. The asking agent's withdraw: `closedAs: withdrawn`.
//! 4. Another workspace's agent can't close the card (403), and it stays
//!    waiting.

use crate::support::app_fixture;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};

fn pinned_ws(env: &Env, handle: &str) -> Ws {
    let ws = seed_ws(env, handle);
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    k2_core::workspace::resume_chat::note_unstarted_pin(&ws.id, &conv);
    ws
}

async fn card(port: u16, handle: &str, id: &str) -> Value {
    let r = get(port, "/cli/thread", OWNER, &format!("addr={}", enc(handle))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let item = v["items"].as_array().expect("items").iter().find(|i| i["id"] == id).cloned().unwrap_or_else(|| panic!("card {id} not served: {v}"));
    item["doc"]["choice"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dismiss_and_withdraw_are_daemon_states_read_headless() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let _shim = k2_core::test_env::AgentShim::install();
    let ws = pinned_ws(&env, &format!("cdsales{tag}"));
    let other = pinned_ws(&env, &format!("cdother{tag}"));
    let h = ws.handle.clone();
    let agent = passport(&ws);

    let ask = |prompt: &'static str| {
        let (h, agent) = (h.clone(), agent.clone());
        async move {
            let r = post(port, "/cli/thread/ask", &agent, json!({ "addr": h, "prompt": prompt, "options": "Ship,Wait", "from": h })).await;
            assert_eq!(r.status, 200, "ask: {}", r.body);
            r.json()["id"].as_str().expect("card id").to_string()
        }
    };
    let a = ask("Ship A?").await;
    let b = ask("Ship B?").await;

    // 1. POST only.
    let r = get(port, "/cli/thread/void", OWNER, &format!("addr={}&id={a}&reason=nope", enc(&h))).await;
    assert_eq!(r.status, 405, "GET must not close a card: {}", r.body);
    assert_eq!(card(port, &h, &a).await["status"], "pending");

    // 4. Another workspace's agent: refused, nothing changes.
    let r = post(port, "/cli/thread/void", &passport(&other), json!({ "addr": h, "id": a, "reason": "not mine" })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let c = card(port, &h, &a).await;
    assert_eq!((c["status"].clone(), c["closedAs"].clone()), (json!("pending"), Value::Null), "{c}");

    // 2. The person dismisses A, with a reason.
    let r = post(port, "/cli/thread/void", OWNER, json!({ "addr": h, "id": a, "reason": "Shipped yesterday" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["closedAs"], "dismissed", "{}", r.body);
    let c = card(port, &h, &a).await;
    assert_eq!(
        (c["status"].clone(), c["closedAs"].clone(), c["closeReason"].clone()),
        (json!("voided"), json!("dismissed"), json!("Shipped yesterday")),
        "every reader gets the daemon's state: {c}"
    );
    assert!(c["answeredBy"].is_object(), "who dismissed it: {c}");

    // 3. The asking agent withdraws B.
    let r = post(port, "/cli/thread/void", &agent, json!({ "addr": h, "id": b, "why": "No longer relevant" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["closedAs"], "withdrawn", "{}", r.body);
    let c = card(port, &h, &b).await;
    assert_eq!((c["closedAs"].clone(), c["closeReason"].clone()), (json!("withdrawn"), json!("No longer relevant")), "{c}");

    // Closed is final: a second dismiss is a 409 that carries the card.
    let r = post(port, "/cli/thread/void", OWNER, json!({ "addr": h, "id": a })).await;
    assert_eq!(r.status, 409, "{}", r.body);
    assert_eq!(r.code(), "card_not_pending");
}
