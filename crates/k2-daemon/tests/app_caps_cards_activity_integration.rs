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

/// Review B1: a guest an agent added (and set the password of) is not a
/// person: signed in, it can't answer the agent's card. Once the owner takes
/// it over (`skin::creators::clear_agent_tie`, which also wipes the password
/// the agent knew) and gives it a new password, it can. An owner's guest can.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_its_agent_added_cant_answer_that_agents_cards() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let (ws, _) = pinned_ws(&env, &format!("b1{tag}"));
    allow_manage(&ws, true);
    let agent = passport(&ws);
    // The agent adds a guest of its own App (agents add App users only) and
    // sets its password itself.
    let app = {
        let dir = ws.path.join("b1app");
        std::fs::create_dir_all(&dir).unwrap();
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::insert(&conn, &ws.id, &format!("b1app{tag}"), "node s.mjs", &dir.to_string_lossy(), 47_601, "local", "stopped", "cmd", "")
            .expect("service")
            .id
    };
    let sock = format!("sock{tag}");
    let role = format!("r-{sock}");
    ok_post(port, "/cli/skin/roles", OWNER, json!({ "name": role })).await;
    ok_post(port, "/cli/skin/roles/room", OWNER, json!({ "name": role, "handle": ws.handle, "caps": ["thread:read", "thread:post"] })).await;
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": sock, "app": app, "role": role })).await;
    assert_eq!(r.status, 200, "an agent with manage Apps adds a guest: {}", r.body);
    let r = post(port, "/cli/skin/users/password", &agent, json!({ "username": sock, "password": "s3cret-horse" })).await;
    assert_eq!(r.status, 200, "and sets its password: {}", r.body);
    // Thread rights come with the role on its App grant (nothing here
    // clears the tie).
    let body = json!({ "username": sock, "password": "s3cret-horse", "app": app }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    let sock_pass = r.json()["token"].as_str().expect("token").to_string();
    let person = guest(port, &format!("pat{tag}"), &ws, &["thread:read", "thread:post"]).await;

    let ask = |prompt: &'static str| {
        let agent = agent.clone();
        let h = ws.handle.clone();
        async move {
            let v = ok_post(port, "/cli/thread/ask", &agent, json!({ "addr": h, "prompt": prompt, "options": "Go,Stop", "from": h })).await;
            v["id"].as_str().expect("card id").to_string()
        }
    };
    let id = ask("Ship it?").await;
    let r = post(port, "/cli/thread/answer", &sock_pass, json!({ "addr": ws.handle, "id": id, "answer": "Go" })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.code(), "card_person_only", "{}", r.body);
    let (pid, h) = (id.clone(), ws.handle.clone());
    assert_eq!(tokio::task::spawn_blocking(move || card_status(port, &h, &pid)).await.unwrap(), "pending", "nothing written");
    // An owner's guest is a person.
    let r = post(port, "/cli/thread/answer", &person, json!({ "addr": ws.handle, "id": id, "answer": "Go" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    // Round-2 B1: the agent resets the password of an OWNER-made guest that
    // lives wholly in its workspace: that guest is tied from then on.
    let r = post(port, "/cli/skin/users/password", &agent, json!({ "username": format!("pat{tag}"), "password": "agent-picked-pw" })).await;
    assert_eq!(r.status, 200, "an agent resets its own workspace's guest: {}", r.body);
    let body = json!({ "username": format!("pat{tag}"), "password": "agent-picked-pw" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    let pat_as_agent = r.json()["token"].as_str().expect("token").to_string();
    let id3 = ask("One more?").await;
    let r = post(port, "/cli/thread/answer", &pat_as_agent, json!({ "addr": ws.handle, "id": id3, "answer": "Go" })).await;
    assert_eq!(r.status, 403, "a guest the agent re-keyed is tied: {}", r.body);
    assert_eq!(r.code(), "card_person_only", "{}", r.body);
    // A person takes the agent's guest over: from then on it answers.
    let principal = k2_core::skin::list_principals()
        .expect("principals")
        .into_iter()
        .find(|p| p.username == sock)
        .expect("the agent's guest")
        .id;
    k2_core::skin::creators::clear_agent_tie(&principal, "owner-token").expect("clear");
    // The claim ended the session the agent's password opened.
    let id2 = ask("And this?").await;
    let r = post(port, "/cli/thread/answer", &sock_pass, json!({ "addr": ws.handle, "id": id2, "answer": "Stop" })).await;
    assert_eq!(r.status, 401, "the claim ends every old session: {}", r.body);
    // The owner gives the person a new password: now it answers.
    ok_post(port, "/cli/skin/users/password", OWNER, json!({ "username": sock, "password": "persons-own-pw-1" })).await;
    let body = json!({ "username": sock, "password": "persons-own-pw-1", "app": app }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    let sock_pass = r.json()["token"].as_str().expect("token").to_string();
    let r = post(port, "/cli/thread/answer", &sock_pass, json!({ "addr": ws.handle, "id": id2, "answer": "Stop" })).await;
    assert_eq!(r.status, 200, "after the owner claimed it: {}", r.body);
}

/// Review B2: a Connect login's raw query params never move its activity
/// bucket: not onto an agent's (draining it), not onto a fresh one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_login_can_neither_dodge_nor_drain_another_callers_bucket() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let (ws, _) = pinned_ws(&env, &format!("b2{tag}"));
    ok_post(port, "/cli/users/add", OWNER, json!({ "username": format!("mel{tag}"), "password": "password123" })).await;
    let body = json!({ "username": format!("mel{tag}"), "password": "password123" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/auth/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "connect login: {}", r.body);
    let mel = r.json()["token"].as_str().expect("token").to_string();
    let agent = passport(&ws);
    let (h, id) = (ws.handle.clone(), ws.id.clone());
    tokio::task::spawn_blocking(move || {
        let call = |tok: &str, extra: &str| {
            req_blocking(port, "GET", &format!("/cli/thread/activity?token={}&addr={}{extra}", enc(tok), enc(&h)), None)
        };
        // The login spends its own burst while claiming to be the agent and
        // a new cell on every call.
        let mut answered = 0;
        loop {
            let spoof = format!("&principal_bound=1&project_id={}&from={}&cell_session_id=c{answered}", enc(&id), enc(&h));
            let r = call(&mel, &spoof);
            if r.status == 429 {
                break;
            }
            assert_eq!(r.status, 200, "call {answered}: {}", r.body);
            answered += 1;
            assert!(answered <= 20, "fresh buckets: the limit never refused");
        }
        assert!((10..=12).contains(&answered), "one burst of 10, whatever the params ({answered})");
        // The agent's own bucket is untouched: its full burst still answers.
        for i in 0..10 {
            let r = call(&agent, "");
            assert_eq!(r.status, 200, "agent call {i}: the login didn't drain it: {}", r.body);
        }
    })
    .await
    .unwrap();
}
