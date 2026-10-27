//! Per-agent unread and ticket counts (agent-unread-tickets-v1, K2 0.46.4;
//! Rosson 2026-10-24: "the unread and the ticket stuff should probably
//! even exist as a surface area that we make visible for people making
//! other apps on top of the agents"), headless: the real dispatcher in
//! process on a temp `$HOME`, over loopback.
//!
//! - **Home (owner / Connect login).** `GET /cli/presence/summary`
//!   `agentActivity[]` carries the CALLER's `unread` / `waiting` / `muted`
//!   on each agent's Threads and the agent's `ticketsWaiting` /
//!   `ticketsWaitingMe`; an idle agent with any of these is listed. Two
//!   people see their own unread; a read by one never moves the other.
//!   `/boot-status` lists the key. An App pass is refused (403).
//! - **Apps.** `GET /cli/skin/agents` (`roomAgents.list`) and
//!   `GET /cli/skin/me` (`agents[]`) carry `unread` / `waiting` / `muted`
//!   only where the pass holds `thread:read` in the room and
//!   `ticketsWaiting` only where it holds `tickets:read`; a room outside
//!   the pass is never listed, counted or named. A guest's unread is its
//!   own read state (`guest:<principalId>`), not the owner's.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};

const PW: &str = "s3cret-horse-staple";

/// One test at a time in this binary (each swaps the process `$HOME`).
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn pin_chat(ws: &Ws) -> String {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    conv
}

async fn owner_ok(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

/// The agent posts `n` lines to its own room's Thread.
async fn agent_posts(port: u16, ws: &Ws, n: usize) -> Vec<i64> {
    let agent = passport(ws);
    let mut seqs = Vec::new();
    for i in 0..n {
        let r = post(port, "/cli/thread/post", &agent, json!({ "addr": ws.handle, "text": format!("line {i}") })).await;
        assert_eq!(r.status, 200, "{}", r.body);
        seqs.push(r.json()["seq"].as_i64().expect("seq"));
    }
    seqs
}

/// A waiting ticket on `ws`, assigned to `assignees`.
fn ticket(ws: &Ws, title: &str, status: &str, assignees: &[&str]) -> String {
    let item = k2_core::feedback::create(k2_core::feedback::NewFeedback {
        project_id: ws.id.clone(),
        agent_name: ws.handle.clone(),
        kind: "question".into(),
        title: title.into(),
        ..Default::default()
    })
    .expect("ticket");
    if status != "waiting" {
        k2_core::feedback::set_status(&item.id, status).expect("status");
    }
    if !assignees.is_empty() {
        let names: Vec<String> = assignees.iter().map(|s| s.to_string()).collect();
        k2_core::feedback::set_assignees(&item.id, &names).expect("assign");
    }
    item.id
}

async fn connect_login(port: u16, username: &str) -> String {
    owner_ok(port, "/cli/users/add", json!({ "username": username, "password": "password123" })).await;
    let body = json!({ "username": username, "password": "password123" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/auth/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

async fn summary(port: u16, token: &str) -> Value {
    let r = get(port, "/cli/presence/summary", token, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()
}

/// The summary's entry for `ws` (panics when it isn't listed).
fn entry<'a>(s: &'a Value, ws: &Ws) -> &'a Value {
    s["agentActivity"]
        .as_array()
        .expect("agentActivity")
        .iter()
        .find(|a| a["workspaceId"] == ws.id.as_str())
        .unwrap_or_else(|| panic!("{} not listed: {s}", ws.handle))
}

fn listed(s: &Value, ws: &Ws) -> bool {
    s["agentActivity"].as_array().expect("agentActivity").iter().any(|a| a["workspaceId"] == ws.id.as_str())
}

fn inbox(e: &Value) -> (i64, i64, bool, i64, i64) {
    (
        e["unread"].as_i64().unwrap_or_else(|| panic!("unread: {e}")),
        e["waiting"].as_i64().unwrap_or_else(|| panic!("waiting: {e}")),
        e["muted"].as_bool().unwrap_or_else(|| panic!("muted: {e}")),
        e["ticketsWaiting"].as_i64().unwrap_or_else(|| panic!("ticketsWaiting: {e}")),
        e["ticketsWaitingMe"].as_i64().unwrap_or_else(|| panic!("ticketsWaitingMe: {e}")),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn summary_counts_are_per_caller_and_list_idle_agents_with_something_waiting() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let a = seed_ws(&env, &format!("aua{tag}"));
    pin_chat(&a);
    let b = seed_ws(&env, &format!("aub{tag}"));
    let quiet = seed_ws(&env, &format!("auq{tag}"));
    let anna_name = format!("anna{tag}");
    let anna = connect_login(port, &anna_name).await;

    // The feature key.
    let boot = tokio::task::spawn_blocking(move || req_blocking(port, "GET", &format!("/boot-status?token={}", enc(OWNER)), None))
        .await
        .unwrap();
    assert_eq!(boot.status, 200, "{}", boot.body);
    assert!(
        boot.json()["features"].as_array().expect("features").iter().any(|f| f == "agent-unread-tickets-v1"),
        "{}",
        boot.body
    );

    // Both people are seen before anything is posted (everything starts read).
    let s0 = summary(port, OWNER).await;
    assert!(!listed(&s0, &a) && !listed(&s0, &b) && !listed(&s0, &quiet), "nothing waiting yet: {s0}");
    summary(port, &anna).await;

    let seqs = agent_posts(port, &a, 3).await;
    ticket(&b, "needs owner", "waiting", &["owner"]);
    ticket(&b, "needs anna", "needs_discussion", &[anna_name.as_str()]);
    ticket(&b, "answered", "answered", &["owner"]);
    ticket(&a, "unassigned", "waiting", &[]);

    let so = summary(port, OWNER).await;
    assert_eq!(inbox(entry(&so, &a)), (3, 0, false, 1, 0), "{so}");
    assert_eq!(inbox(entry(&so, &b)), (0, 0, false, 2, 1), "{so}");
    assert_eq!(entry(&so, &b)["display"], "idle", "listed for its tickets alone: {so}");
    assert!(!listed(&so, &quiet), "an agent with nothing is not listed: {so}");
    let sa = summary(port, &anna).await;
    assert_eq!(inbox(entry(&sa, &a)), (3, 0, false, 1, 0), "{sa}");
    assert_eq!(inbox(entry(&sa, &b)), (0, 0, false, 2, 1), "anna's own ticket is hers: {sa}");

    // The owner reads A's Thread: the owner's unread clears, anna's stays.
    owner_ok(port, "/cli/thread/read", json!({ "addr": a.handle, "seq": seqs[2] })).await;
    let so = summary(port, OWNER).await;
    assert_eq!(inbox(entry(&so, &a)), (0, 0, false, 1, 0), "{so}");
    let sa = summary(port, &anna).await;
    assert_eq!(inbox(entry(&sa, &a)), (3, 0, false, 1, 0), "a read by one person never moves another: {sa}");

    // Anna mutes A: her count stays, greyed.
    let r = post(port, "/cli/thread/mute", &anna, json!({ "addr": a.handle, "muted": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let sa = summary(port, &anna).await;
    assert_eq!(inbox(entry(&sa, &a)), (3, 0, true, 1, 0), "{sa}");

    // An App pass never reads the owner summary.
    owner_ok(port, "/cli/skin/roles", json!({ "name": format!("r{tag}") })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": format!("r{tag}"), "handle": a.handle, "caps": ["thread:read"] })).await;
    let guest = format!("g{tag}");
    owner_ok(port, "/cli/skin/users", json!({ "username": guest })).await;
    owner_ok(port, "/cli/skin/users/password", json!({ "username": guest, "password": PW })).await;
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": guest, "role": format!("r{tag}") })).await;
    let body = json!({ "username": guest, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    let pass = r.json()["token"].as_str().expect("pass").to_string();
    let r = get(port, "/cli/presence/summary", &pass, "").await;
    assert_eq!(r.status, 403, "an App pass is refused: {}", r.body);
    assert!(!r.body.contains("unread"), "{}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn app_agent_rows_carry_counts_only_where_the_caps_allow_and_never_for_other_rooms() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let full = seed_ws(&env, &format!("apf{tag}"));
    pin_chat(&full);
    let chat_only = seed_ws(&env, &format!("apc{tag}"));
    pin_chat(&chat_only);
    let tickets_only = seed_ws(&env, &format!("apt{tag}"));
    let outside = seed_ws(&env, &format!("apo{tag}"));
    let outside_conv = pin_chat(&outside);

    let rn = format!("staff{tag}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": rn })).await;
    for (ws, caps) in [
        (&full, json!(["thread:read", "tickets:read"])),
        (&chat_only, json!(["thread:read"])),
        (&tickets_only, json!(["tickets:read"])),
    ] {
        owner_ok(port, "/cli/skin/roles/room", json!({ "name": rn, "handle": ws.handle, "caps": caps })).await;
    }
    let guest = format!("pat{tag}");
    owner_ok(port, "/cli/skin/users", json!({ "username": guest })).await;
    owner_ok(port, "/cli/skin/users/password", json!({ "username": guest, "password": PW })).await;
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": guest, "role": rn })).await;
    let body = json!({ "username": guest, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    let pass = r.json()["token"].as_str().expect("pass").to_string();

    // The guest and the owner are seen before the posts.
    let r = get(port, "/cli/skin/agents", &pass, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    summary(port, OWNER).await;

    let full_seqs = agent_posts(port, &full, 2).await;
    agent_posts(port, &chat_only, 1).await;
    agent_posts(port, &outside, 4).await;
    ticket(&full, "f1", "waiting", &[]);
    ticket(&full, "f2", "needs_discussion", &[]);
    ticket(&full, "f3 closed", "resolved", &[]);
    ticket(&chat_only, "c1", "waiting", &[]);
    ticket(&tickets_only, "t1", "waiting", &[]);
    ticket(&outside, "o1", "waiting", &[]);

    let check = |rows: &[Value], key: &str, what: &str| {
        let row = |ws: &Ws| -> Value {
            rows.iter()
                .find(|r| r[key] == ws.id.as_str())
                .cloned()
                .unwrap_or_else(|| panic!("{what}: no row for {}: {rows:?}", ws.handle))
        };
        assert_eq!(rows.len(), 3, "{what}: only the pass's rooms: {rows:?}");
        let f = row(&full);
        assert_eq!((f["unread"].as_i64(), f["waiting"].as_i64(), f["muted"].as_bool()), (Some(2), Some(0), Some(false)), "{what}: {f}");
        assert_eq!(f["ticketsWaiting"].as_i64(), Some(2), "{what}: {f}");
        let c = row(&chat_only);
        assert_eq!(c["unread"].as_i64(), Some(1), "{what}: {c}");
        assert!(c.get("ticketsWaiting").is_none(), "{what}: no tickets:read there, no ticket count: {c}");
        let t = row(&tickets_only);
        assert_eq!(t["ticketsWaiting"].as_i64(), Some(1), "{what}: {t}");
        assert!(t.get("unread").is_none() && t.get("waiting").is_none() && t.get("muted").is_none(), "{what}: no thread:read there: {t}");
    };

    let r = get(port, "/cli/skin/agents", &pass, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        !r.body.contains(&outside.id) && !r.body.contains(&outside.handle) && !r.body.contains(&outside_conv),
        "a room outside the pass is never named: {}",
        r.body
    );
    check(r.json()["agents"].as_array().expect("agents"), "projectId", "skin/agents");

    let r = get(port, "/cli/skin/me", &pass, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!r.body.contains(&outside.id) && !r.body.contains(&outside.handle), "{}", r.body);
    check(r.json()["agents"].as_array().expect("agents"), "room", "skin/me");

    // The guest reads its Chat: its own count clears, the owner's doesn't.
    let r = post(port, "/cli/thread/read", &pass, json!({ "addr": full.handle, "seq": full_seqs[1] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = get(port, "/cli/skin/agents", &pass, "").await;
    let rows = r.json()["agents"].as_array().expect("agents").clone();
    let f = rows.iter().find(|x| x["projectId"] == full.id.as_str()).expect("full row");
    assert_eq!(f["unread"].as_i64(), Some(0), "{f}");
    let so = summary(port, OWNER).await;
    assert_eq!(entry(&so, &full)["unread"].as_i64(), Some(2), "a guest's read is its own: {so}");
    assert_eq!(entry(&so, &outside)["unread"].as_i64(), Some(4), "{so}");
    assert_eq!(entry(&so, &outside)["ticketsWaiting"].as_i64(), Some(1), "{so}");
}
