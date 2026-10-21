//! A failed agent start never moves the pinned Chat (0.46.3 smoke,
//! medium). Headless: the real dispatcher in process on a temp `$HOME`,
//! loopback HTTP, real PTY spawns.
//!
//! The bug: a workspace whose agent CLI can't start (no `claude` on the
//! box) got a NEW pinned Chat on every failed start. `k2 msg` wakes the
//! dormant agent; the resume resolver saw a saved pin with no Claude
//! transcript on disk, minted a fresh id and persisted it BEFORE the
//! spawn, and nothing rolled it back when the spawn failed (three times
//! per `k2 msg`: one try plus two retries). The Thread is keyed on the
//! pin, so `k2 thread <ws>` showed an empty conversation: the earlier
//! messages, a card and guests' read state seemed to vanish.
//!
//! Each test: a pinned Chat with agent lines, a card and a guest's read
//! cursor; N failed `k2 msg` wakes (`POST /cli/workspace/msg`); then
//! `GET /cli/thread` is the same conversation with every earlier item, and
//! the guest's `GET /cli/thread/unread` row is unchanged.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};

/// One test at a time in this binary (each swaps the process `$HOME`).
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Wake attempts per test (each `k2 msg` is itself three spawn tries).
const FAILED_WAKES: usize = 3;

fn pin_chat(ws: &Ws) -> String {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running")
        .expect("pin");
    conn.execute("UPDATE projects SET agent_enabled = 1 WHERE id = ?1", [&ws.id]).expect("agent on");
    conv
}

fn saved_pin(ws: &Ws) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::get(&conn, &ws.id).expect("workspace_sessions read").and_then(|r| r.session_id)
}

async fn ok_post(port: u16, path: &str, token: &str, body: Value) -> Value {
    let r = post(port, path, token, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

async fn ok_get(port: u16, path: &str, token: &str, query: &str) -> Value {
    let r = get(port, path, token, query).await;
    assert_eq!(r.status, 200, "{path}?{query}: {}", r.body);
    r.json()
}

/// A guest whose role reads the room's Thread; returns its pass.
async fn guest(port: u16, username: &str, ws: &Ws) -> String {
    ok_post(port, "/cli/skin/users", OWNER, json!({ "username": username })).await;
    ok_post(port, "/cli/skin/users/password", OWNER, json!({ "username": username, "password": "s3cret-horse" })).await;
    let role = format!("r-{username}");
    ok_post(port, "/cli/skin/roles", OWNER, json!({ "name": role })).await;
    ok_post(port, "/cli/skin/roles/room", OWNER, json!({ "name": role, "handle": ws.handle, "caps": ["thread:read"] })).await;
    ok_post(port, "/cli/skin/roles/assign", OWNER, json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": "s3cret-horse" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

/// `k2 msg <ws> "text"` as the CLI sends it (form params, wake on).
async fn k2_msg(port: u16, ws: &Ws, text: &str) -> Value {
    let pq = format!(
        "/cli/workspace/msg?token={}&workspace={}&text={}&from=tester&wake=true&wake_timeout=2",
        enc(OWNER),
        enc(&ws.handle),
        enc(text)
    );
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", &pq, None)).await.unwrap();
    assert_eq!(r.status, 200, "workspace/msg: {}", r.body);
    r.json()
}

/// `(id, seq)` of every Thread item, oldest first.
fn item_keys(page: &Value) -> Vec<(String, i64)> {
    page["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no items: {page}"))
        .iter()
        .map(|i| (i["id"].as_str().expect("id").to_string(), i["seq"].as_i64().expect("seq")))
        .collect()
}

fn unread_row(u: &Value, conv: &str) -> Value {
    u["byConversation"]
        .as_array()
        .unwrap_or_else(|| panic!("no rows: {u}"))
        .iter()
        .find(|r| r["id"] == conv)
        .unwrap_or_else(|| panic!("no unread row for {conv}: {u}"))
        .clone()
}

/// The whole scenario; `expect_spawn_failed` = the start fails before the
/// PTY exists (missing binary), so every `k2 msg` must say `spawn_failed`.
async fn failed_wakes_keep_the_thread(env: &Env, label: &str, expect_spawn_failed: bool) {
    let env_tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(env, &format!("{label}{env_tag}"));
    let conv = pin_chat(&ws);
    let agent = passport(&ws);

    // Earlier Thread: three agent lines and a card (agent posts wake nobody).
    for i in 0..3 {
        ok_post(port, "/cli/thread/post", &agent, json!({ "addr": ws.handle, "text": format!("line {i}") })).await;
    }
    let card = ok_post(port, "/cli/thread/ask", &agent, json!({ "addr": ws.handle, "prompt": "Ship it?", "options": "Ship,Wait" })).await;
    assert!(card["id"].is_string(), "card id: {card}");
    let before = ok_get(port, "/cli/thread", OWNER, &format!("addr={}&limit=0", ws.handle)).await;
    assert_eq!(before["conversation_id"], conv.as_str(), "{before}");
    let keys = item_keys(&before);
    assert_eq!(keys.len(), 4, "three lines and a card: {before}");

    // A guest reads through the second line.
    let pass = guest(port, &format!("g{label}{env_tag}"), &ws).await;
    ok_post(port, "/cli/thread/read", &pass, json!({ "addr": ws.handle, "seq": keys[1].1 })).await;
    let guest_before = unread_row(&ok_get(port, "/cli/thread/unread", &pass, "").await, &conv);
    assert_eq!(
        (guest_before["lastReadSeq"].as_i64(), guest_before["unread"].as_i64()),
        (Some(keys[1].1), Some(2)),
        "{guest_before}"
    );

    // N failed wakes.
    for n in 0..FAILED_WAKES {
        let v = k2_msg(port, &ws, &format!("ping {n}")).await;
        if expect_spawn_failed {
            assert_eq!((v["success"].as_bool(), v["reason"].as_str()), (Some(false), Some("spawn_failed")), "wake {n}: {v}");
        }
        assert_eq!(saved_pin(&ws).as_deref(), Some(conv.as_str()), "wake {n} moved the pinned Chat: {v}");
    }

    // Same conversation, every earlier item, same guest read state.
    let after = ok_get(port, "/cli/thread", OWNER, &format!("addr={}&limit=0", ws.handle)).await;
    assert_eq!(after["conversation_id"], conv.as_str(), "the pinned Chat moved after failed starts: {after}");
    assert_eq!(item_keys(&after), keys, "earlier Thread items must all still be there: {after}");
    let delta = ok_get(port, "/cli/thread", OWNER, &format!("addr={}&after_seq=0&limit=0", ws.handle)).await;
    assert_eq!(delta["conversation_id"], conv.as_str(), "{delta}");
    assert_eq!(item_keys(&delta), keys, "{delta}");
    let guest_after = unread_row(&ok_get(port, "/cli/thread/unread", &pass, "").await, &conv);
    assert_eq!(guest_after, guest_before, "the guest's read state must not change");
    let guest_page = ok_get(port, "/cli/thread", &pass, &format!("addr={}&limit=0", ws.handle)).await;
    assert_eq!(guest_page["conversation_id"], conv.as_str(), "{guest_page}");
    assert_eq!(item_keys(&guest_page), keys, "{guest_page}");

    if let Some(s) = k2_daemon::v2_session_map::unregister(&ws.id) {
        s.kill();
    }
}

/// The agent CLI is missing (an empty shim dir: the spawn guard finds no
/// `claude`), as on a box without the harness installed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_wakes_with_a_missing_cli_keep_the_pinned_chat() {
    let _serial = serial();
    let env = setup(&[]);
    let empty = env.path().join("no-agents");
    std::fs::create_dir_all(&empty).unwrap();
    let _shim = k2_core::test_env::EnvVar::set(k2_core::terminal::agent_spawn_guard::SHIM_DIR_ENV, &empty);
    failed_wakes_keep_the_thread(&env, "nocli", true).await;
}

/// The agent CLI exists but exits at once (a broken install).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_wakes_with_a_cli_that_exits_keep_the_pinned_chat() {
    let _serial = serial();
    let env = setup(&[]);
    let _shim = k2_core::test_env::AgentShim::with_body("exit 1");
    failed_wakes_keep_the_thread(&env, "exitcli", false).await;
}
