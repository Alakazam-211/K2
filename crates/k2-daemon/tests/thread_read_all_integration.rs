//! Mark all as read (thread-read-all-v1, K2 0.46.5; Rosson 2026-10-26:
//! "In Agent NSI I have a message that says unread but I can never read
//! it, so the marker always shows 1"), headless: the real dispatcher in
//! process on a temp `$HOME`, over loopback.
//!
//! - **The stuck badge, reproduced.** An agent row's badge (`presence/
//!   summary` `agentActivity[].unread`) sums every Thread of the
//!   workspace, but reading the agent reads only the conversation its
//!   address reaches NOW (the pinned Chat). Another Thread of the workspace
//!   (an earlier chat's, a closed tab's) keeps its unread and no read by
//!   address clears it.
//! - **`POST /cli/thread/read-all`** clears all of them for the caller's
//!   person only (an owner's read never moves a Connect login's count, and
//!   back), by agent handle or `projectId`. A Connect login is exactly what
//!   a remote desktop holds for another server (Home rows, rooms), so the
//!   login half is the remote path. POST-only (405 on GET), agents 403,
//!   unknown agent 404, nothing named 400.
//! - **Apps** (`thread.markAllRead`): an App guest reads its room's Thread
//!   to the newest on its own cursor; another room is `skin_room`, a room
//!   without `thread:read` is `cap_not_granted`, and the owner's count
//!   never moves.

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

/// Pin a new Chat for `ws` (a re-pin replaces the old one: the row is
/// keyed by workspace). Returns its conversation id.
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

/// The agent posts `n` lines to its pinned Chat's Thread.
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

/// The agent posts `n` lines; the conversation each landed in.
async fn agent_post_convs(port: u16, ws: &Ws, n: usize) -> Vec<String> {
    let agent = passport(ws);
    let mut convs = Vec::new();
    for i in 0..n {
        let r = post(port, "/cli/thread/post", &agent, json!({ "addr": ws.handle, "text": format!("line {i}") })).await;
        assert_eq!(r.status, 200, "{}", r.body);
        let v = r.json();
        convs.push(v["conversation_id"].as_str().unwrap_or_else(|| panic!("conversation_id: {v}")).to_string());
    }
    convs
}

async fn connect_login(port: u16, username: &str) -> String {
    owner_ok(port, "/cli/users/add", json!({ "username": username, "password": "password123" })).await;
    let body = json!({ "username": username, "password": "password123" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/auth/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

/// The caller's unread on `ws`'s agent row (`presence/summary`). A
/// workspace the summary doesn't list has nothing unread, waiting or
/// ticketed: 0.
/// The caller's whole badge on this server (`GET /cli/thread/unread`
/// `total`). Other tests in this binary leave Threads in the same process
/// DB, so tests compare it against a baseline read first, never a literal.
async fn badge_total(port: u16, token: &str) -> i64 {
    let r = get(port, "/cli/thread/unread", token, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    v["total"].as_i64().unwrap_or_else(|| panic!("total: {v}"))
}

async fn row_unread(port: u16, token: &str, ws: &Ws) -> i64 {
    let r = get(port, "/cli/presence/summary", token, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let s = r.json();
    match s["agentActivity"].as_array().expect("agentActivity").iter().find(|a| a["workspaceId"] == ws.id.as_str()) {
        None => 0,
        Some(e) => e["unread"].as_i64().unwrap_or_else(|| panic!("unread: {e}")),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mark_all_read_clears_an_agents_unreachable_threads_for_the_caller_only() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let a = seed_ws(&env, &format!("ara{tag}"));
    let b = seed_ws(&env, &format!("arb{tag}"));
    let second = pin_chat(&a);
    pin_chat(&b);
    let anna = connect_login(port, &format!("anna{tag}")).await;

    let boot = tokio::task::spawn_blocking(move || req_blocking(port, "GET", &format!("/boot-status?token={}", enc(OWNER)), None))
        .await
        .unwrap();
    assert_eq!(boot.status, 200, "{}", boot.body);
    assert!(
        boot.json()["features"].as_array().expect("features").iter().any(|f| f == "thread-read-all-v1"),
        "{}",
        boot.body
    );

    // Both people are seen before anything is posted; the owner's badge
    // before this test's messages is the baseline.
    let baseline = badge_total(port, OWNER).await;
    assert_eq!(row_unread(port, OWNER, &a).await, 0);
    assert_eq!(row_unread(port, &anna, &a).await, 0);

    // An earlier chat's Thread (an old session, a closed tab) still holds
    // two of the agent's lines: a conversation of this workspace that the
    // agent's address no longer reaches. Then the agent writes once in its
    // pinned Chat.
    let earlier = format!("earlier-{tag}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for i in 0..2 {
            k2_core::overlay::post_thread(&conn, &earlier, &a.id, "agent", "agent", &format!("old line {i}"), "thread")
                .expect("old chat line");
        }
    }
    assert_eq!(agent_post_convs(port, &a, 1).await, vec![second.clone()]);
    agent_posts(port, &b, 1).await;
    assert_eq!(row_unread(port, OWNER, &a).await, 3);
    // The badge rose by exactly agent A's 3 plus agent B's 1.
    assert_eq!(badge_total(port, OWNER).await, baseline + 3 + 1);
    // THE BUG: reading the agent (its address) reads only the pinned Chat.
    // The earlier chat's two lines stay on the row, and reading again
    // changes nothing: no address reaches them.
    let r = owner_ok(port, "/cli/thread/read", json!({ "addr": a.handle })).await;
    assert_eq!(r["id"], second.as_str(), "{r}");
    assert_eq!(row_unread(port, OWNER, &a).await, 2, "the stuck count");
    owner_ok(port, "/cli/thread/read", json!({ "addr": a.handle })).await;
    assert_eq!(row_unread(port, OWNER, &a).await, 2, "still stuck after another read");
    assert_eq!(badge_total(port, OWNER).await, baseline + 2 + 1, "the badge holds A's stuck 2 and B's 1");

    // POST-only, people only, a known agent.
    let r = get(port, "/cli/thread/read-all", OWNER, &format!("agent={}", enc(&a.handle))).await;
    assert_eq!(r.status, 405, "GET is refused: {}", r.body);
    let r = post(port, "/cli/thread/read-all", &passport(&a), json!({ "agent": a.handle })).await;
    assert_eq!(r.status, 403, "an agent has no read state: {}", r.body);
    let r = post(port, "/cli/thread/read-all", OWNER, json!({ "agent": format!("nosuch{tag}") })).await;
    assert_eq!(r.status, 404, "{}", r.body);
    let r = post(port, "/cli/thread/read-all", OWNER, json!({ "projectId": "no-such-workspace" })).await;
    assert_eq!(r.status, 404, "{}", r.body);
    let r = post(port, "/cli/thread/read-all", OWNER, json!({})).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert_eq!(row_unread(port, OWNER, &a).await, 2, "refusals change nothing");

    // Mark all as read: the earlier chat's Thread moves; the count is 0.
    let r = owner_ok(port, "/cli/thread/read-all", json!({ "agent": a.handle })).await;
    assert_eq!(r["changed"], true, "{r}");
    assert_eq!(r["projectId"], a.id.as_str(), "{r}");
    assert_eq!(r["marked"], json!([earlier]), "only the Thread behind its head moved: {r}");
    assert_eq!((r["unread"].as_i64(), r["waiting"].as_i64()), (Some(0), Some(0)), "{r}");
    // Agent A's whole 3 left the badge (1 by reading its Chat, 2 by mark
    // all); only agent B's line remains above the baseline.
    assert_eq!(r["total"].as_i64(), Some(baseline + 1), "baseline {baseline}: {r}");
    assert_eq!(badge_total(port, OWNER).await, baseline + 1);
    assert_eq!(row_unread(port, OWNER, &a).await, 0);
    assert_eq!(row_unread(port, OWNER, &b).await, 1, "another agent is untouched");
    let again = owner_ok(port, "/cli/thread/read-all", json!({ "agent": a.handle })).await;
    assert_eq!((again["changed"].as_bool(), again["marked"].clone()), (Some(false), json!([])), "idempotent: {again}");
    assert_eq!(again["total"].as_i64(), Some(baseline + 1), "idempotent total: {again}");
    // Agent B too: the badge lands back on the baseline exactly.
    let rb = owner_ok(port, "/cli/thread/read-all", json!({ "agent": b.handle })).await;
    assert_eq!(rb["total"].as_i64(), Some(baseline), "baseline {baseline}: {rb}");
    assert_eq!(badge_total(port, OWNER).await, baseline);

    // Per person: anna's count is hers.
    assert_eq!(row_unread(port, &anna, &a).await, 3, "an owner's read never moves another login");
    // The remote half: anna holds a Connect login, as a remote desktop
    // does for this server; by workspace id, as a Home row knows it.
    let r = post(port, "/cli/thread/read-all", &anna, json!({ "projectId": a.id })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["marked"].as_array().map(|m| m.len()), Some(2), "both of anna's Threads: {}", r.body);
    assert_eq!(row_unread(port, &anna, &a).await, 0);
    assert_eq!(row_unread(port, &anna, &b).await, 1);

    // A later post counts again.
    agent_posts(port, &a, 1).await;
    assert_eq!(row_unread(port, OWNER, &a).await, 1);
    assert_eq!(row_unread(port, &anna, &a).await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_guest_marks_its_own_rooms_thread_read_and_nothing_else() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let room = seed_ws(&env, &format!("agr{tag}"));
    pin_chat(&room);
    let no_read = seed_ws(&env, &format!("agn{tag}"));
    pin_chat(&no_read);
    let outside = seed_ws(&env, &format!("ago{tag}"));
    pin_chat(&outside);

    let rn = format!("staff{tag}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": rn })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": rn, "handle": room.handle, "caps": ["thread:read"] })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": rn, "handle": no_read.handle, "caps": ["tickets:read"] })).await;
    let guest = format!("pat{tag}");
    owner_ok(port, "/cli/skin/users", json!({ "username": guest })).await;
    k2_daemon::test_harness::guest_with_own_password(&guest, PW);
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": guest, "role": rn })).await;
    let body = json!({ "username": guest, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    let pass = r.json()["token"].as_str().expect("pass").to_string();

    // The guest and the owner are seen before the posts.
    let r = get(port, "/cli/skin/agents", &pass, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(row_unread(port, OWNER, &room).await, 0);

    agent_posts(port, &room, 3).await;
    agent_posts(port, &no_read, 1).await;
    agent_posts(port, &outside, 1).await;

    let guest_unread = |body: &str| -> i64 {
        let v: Value = serde_json::from_str(body).expect("json");
        let rows = v["agents"].as_array().expect("agents").clone();
        let row = rows.iter().find(|x| x["projectId"] == room.id.as_str()).unwrap_or_else(|| panic!("room row: {body}"));
        row["unread"].as_i64().unwrap_or_else(|| panic!("unread: {row}"))
    };
    let r = get(port, "/cli/skin/agents", &pass, "").await;
    assert_eq!(guest_unread(&r.body), 3);

    // POST-only for a pass too.
    let r = get(port, "/cli/thread/read-all", &pass, &format!("addr={}", enc(&room.handle))).await;
    assert_eq!(r.status, 405, "{}", r.body);
    // Never another room, never a room without thread:read.
    let r = post(port, "/cli/thread/read-all", &pass, json!({ "addr": outside.handle })).await;
    assert!(r.status == 403 || r.status == 404, "{}: {}", r.status, r.body);
    assert!(r.body.contains("skin_room"), "{}", r.body);
    let r = post(port, "/cli/thread/read-all", &pass, json!({ "addr": no_read.handle })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert!(r.body.contains("cap_not_granted"), "{}", r.body);
    let r = post(port, "/cli/thread/read-all", &pass, json!({})).await;
    assert_eq!(r.status, 400, "{}", r.body);

    // thread.markAllRead: the guest's own cursor, to the newest.
    let r = post(port, "/cli/thread/read-all", &pass, json!({ "addr": room.handle })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["changed"].as_bool(), v["unread"].as_i64(), v["addr"].as_str()), (Some(true), Some(0), Some(room.handle.as_str())), "{v}");
    assert_eq!(v["lastReadSeq"], v["lastSeq"], "{v}");
    let r = get(port, "/cli/skin/agents", &pass, "").await;
    assert_eq!(guest_unread(&r.body), 0);
    // The owner's count is the owner's.
    assert_eq!(row_unread(port, OWNER, &room).await, 3, "a guest's read never moves the owner");
    assert_eq!(row_unread(port, OWNER, &outside).await, 1);
}
