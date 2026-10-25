//! prd-app-caps-0464-v1 items C and N (0.46.4), headless over loopback:
//!
//! - C (AC24, AC27): an agent's passport can't answer its own Thread's
//!   choice card (403 `card_person_only`, nothing written); with
//!   `K2_CARD_PERSON_ONLY=off` in the daemon's environment it can again.
//! - N (AC29, AC30): `GET /cli/thread/activity` allows a burst of 10 per
//!   caller, then 429 `rate_limited` with `retryAfterMs` and
//!   `retryAfterSecs` (the main port answers HEAD 405 before any handler,
//!   so only a GET reaches the bucket here); another guest is unaffected;
//!   the owner token is never limited.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};

fn pinned_ws(env: &Env, handle: &str) -> (Ws, String) {
    let ws = seed_ws(env, handle);
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    (ws, conv)
}

async fn ok_post(port: u16, path: &str, token: &str, body: Value) -> Value {
    let r = post(port, path, token, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

async fn guest(port: u16, username: &str, ws: &Ws, caps: &[&str]) -> String {
    ok_post(port, "/cli/skin/users", OWNER, json!({ "username": username })).await;
    ok_post(port, "/cli/skin/users/password", OWNER, json!({ "username": username, "password": "s3cret-horse" })).await;
    let role = format!("r-{username}");
    ok_post(port, "/cli/skin/roles", OWNER, json!({ "name": role })).await;
    ok_post(port, "/cli/skin/roles/room", OWNER, json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    ok_post(port, "/cli/skin/roles/assign", OWNER, json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": "s3cret-horse" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

fn card_status(port: u16, handle: &str, id: &str) -> String {
    let r = req_blocking(port, "GET", &format!("/cli/thread?token={}&addr={}&limit=50", enc(OWNER), enc(handle)), None);
    assert_eq!(r.status, 200, "{}", r.body);
    let items = r.json()["items"].as_array().cloned().expect("items");
    let item = items.iter().find(|i| i["id"] == id).unwrap_or_else(|| panic!("card {id} in {items:?}"));
    item["doc"]["choice"]["status"].as_str().expect("a choice card").to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_cant_answer_its_own_choice_card() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let (ws, _) = pinned_ws(&env, &format!("cpo{tag}"));
    let agent = passport(&ws);
    let ask = ok_post(port, "/cli/thread/ask", &agent, json!({ "addr": ws.handle, "prompt": "Ship it?", "options": "Go,Stop", "from": ws.handle })).await;
    let id = ask["id"].as_str().expect("card id").to_string();
    let r = post(port, "/cli/thread/answer", &agent, json!({ "addr": ws.handle, "id": id, "answer": "Go" })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.code(), "card_person_only", "{}", r.body);
    let pid = id.clone();
    let h = ws.handle.clone();
    assert_eq!(tokio::task::spawn_blocking(move || card_status(port, &h, &pid)).await.unwrap(), "pending");
    // The agent still closes its own card.
    let r = post(port, "/cli/thread/void", &agent, json!({ "addr": ws.handle, "id": id })).await;
    assert_eq!(r.status, 200, "k2 thread void: {}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_switch_off_is_the_old_path() {
    let env = setup(&[("K2_CARD_PERSON_ONLY", "off")]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let (ws, _) = pinned_ws(&env, &format!("cpf{tag}"));
    let agent = passport(&ws);
    let ask = ok_post(port, "/cli/thread/ask", &agent, json!({ "addr": ws.handle, "prompt": "Ship it?", "options": "Go,Stop", "from": ws.handle })).await;
    let id = ask["id"].as_str().expect("card id").to_string();
    let r = post(port, "/cli/thread/answer", &agent, json!({ "addr": ws.handle, "id": id, "answer": "Go" })).await;
    assert_eq!(r.status, 200, "K2_CARD_PERSON_ONLY=off: {}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_activity_catch_up_is_limited_per_caller_and_never_for_the_owner() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let (ws, _) = pinned_ws(&env, &format!("n1{tag}"));
    let ana = guest(port, &format!("ana{tag}"), &ws, &["thread:read", "activity:read"]).await;
    let bo = guest(port, &format!("bo{tag}"), &ws, &["thread:read", "activity:read"]).await;
    let h = ws.handle.clone();
    let out = tokio::task::spawn_blocking(move || {
        let call = |method: &str, tok: &str| req_blocking(port, method, &format!("/cli/thread/activity?token={}&addr={}", enc(tok), enc(&h)), None);
        // The burst, then 429. (HEAD is 405 on the main port: it never
        // reaches the handler, so it can't spend or dodge the bucket.)
        let mut answered = 0;
        let refusal = loop {
            let method = "GET";
            let r = call(method, &ana);
            if r.status == 429 {
                break r;
            }
            assert_eq!(r.status, 200, "{method} call {answered}: {}", r.body);
            answered += 1;
            assert!(answered <= 20, "the limit never refused");
        };
        assert!(answered >= 10, "a burst of 10 (got {answered})");
        let head = call("HEAD", &ana);
        assert_eq!(head.status, 405, "HEAD stops before the handler: {}", head.body);
        let v = refusal.json();
        assert_eq!(v["error"], "rate_limited", "{v}");
        let ms = v["retryAfterMs"].as_u64().expect("retryAfterMs");
        assert!((1..=500).contains(&ms), "{v}");
        assert_eq!(v["retryAfterSecs"].as_u64(), Some(ms.div_ceil(1000)), "{v}");
        // Another guest is unaffected.
        let r = call("GET", &bo);
        assert_eq!(r.status, 200, "{}", r.body);
        // After the wait the first guest gets one more.
        std::thread::sleep(std::time::Duration::from_millis(ms + 50));
        let r = call("GET", &ana);
        assert_eq!(r.status, 200, "after retryAfterMs: {}", r.body);
        // The owner token: never limited.
        for i in 0..30 {
            let r = call("GET", OWNER);
            assert_eq!(r.status, 200, "owner call {i}: {}", r.body);
        }
        answered
    })
    .await
    .unwrap();
    assert!(out >= 10);
}
