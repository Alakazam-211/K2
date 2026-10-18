//! thread-delta-v1 (0.46.1, K2 Companion's on-device chat cache) end to
//! end through the REAL dispatcher (in-process, temp `$HOME`, loopback):
//!
//! 1. `GET /cli/thread?after_seq=N`: items with `seq > N` oldest first,
//!    `limit` + `has_more` (newer items remain), `conversation_id` kept;
//!    without `after_seq` the old newest-N / `since_seq` / `before_seq`
//!    pages are byte-for-byte the old shape; a card answered or voided
//!    after the reader's copy comes back in `changed`; bad params are 400.
//! 2. `GET /cli/project-group/messages?after_seq=N`: the same paging by
//!    the per-group `seq`; without it, the old latest-N / `after=<ts>`.
//! 3. Auth unchanged: owner, same-workspace agent, other-workspace agent,
//!    no token, and an App guest (own room, room without `thread:read`,
//!    room it doesn't hold) get the same status and error code with and
//!    without `after_seq`.
//! 4. `/boot-status` lists `thread-delta-v1`.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};

/// A registered workspace with a pinned Chat (its handle is a Thread
/// address); returns it and the pinned conversation id.
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

async fn ok_get(port: u16, path: &str, token: &str, query: &str) -> Value {
    let r = get(port, path, token, query).await;
    assert_eq!(r.status, 200, "{path}?{query}: {}", r.body);
    r.json()
}

/// An agent-shaped Thread post (owner token, no compose: nothing wakes).
async fn agent_post(port: u16, addr: &str, text: &str) -> i64 {
    ok_post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": text })).await["seq"].as_i64().expect("seq")
}

/// Wake the agent with a person's compose post and wait until the address
/// answers at one stable conversation other than the seeded one.
async fn warm_up(port: u16, handle: &str, seeded: &str) -> String {
    let v = ok_post(port, "/cli/thread/post", OWNER, json!({ "addr": handle, "text": "warm-up", "via": "compose" })).await;
    assert_eq!(v["seq"].as_i64(), Some(1), "{v}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let (mut last, mut stable) = (String::new(), 0);
    loop {
        let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={handle}&limit=1")).await;
        let conv = v["conversation_id"].as_str().expect("conversation_id").to_string();
        stable = if conv != seeded && conv == last { stable + 1 } else { 0 };
        last = conv;
        if stable >= 2 {
            return last;
        }
        assert!(std::time::Instant::now() < deadline, "the pin never swapped after the warm-up: {v}");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

fn seqs(items: &Value) -> Vec<i64> {
    items.as_array().unwrap_or_else(|| panic!("not an array: {items}")).iter().map(|i| i["seq"].as_i64().expect("seq")).collect()
}

fn status_of(item: &Value) -> String {
    item["doc"]["choice"]["status"].as_str().unwrap_or_else(|| panic!("no card status: {item}")).to_string()
}

/// A guest login whose role holds `caps` in each room; returns its pass.
async fn guest(port: u16, username: &str, rooms: &[(&Ws, &[&str])]) -> String {
    ok_post(port, "/cli/skin/users", OWNER, json!({ "username": username })).await;
    ok_post(port, "/cli/skin/users/password", OWNER, json!({ "username": username, "password": "s3cret-horse" })).await;
    let role = format!("r-{username}");
    ok_post(port, "/cli/skin/roles", OWNER, json!({ "name": role })).await;
    for (ws, caps) in rooms {
        ok_post(port, "/cli/skin/roles/room", OWNER, json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    }
    ok_post(port, "/cli/skin/roles/assign", OWNER, json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": "s3cret-horse" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    let tok = r.json()["token"].as_str().expect("token").to_string();
    assert!(tok.starts_with("k2skn_"), "{tok}");
    tok
}

/// `(status, error code or "")` of a response.
fn outcome(r: &Resp) -> (u16, String) {
    if r.status == 200 {
        return (200, String::new());
    }
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    let code = v["error"]["code"].as_str().or_else(|| v["error"].as_str()).or_else(|| v["code"].as_str()).unwrap_or("").to_string();
    (r.status, code)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thread_after_seq_pages_forward_and_carries_card_changes() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let _shim = k2_core::test_env::AgentShim::install();
    let (sales, seeded) = pinned_ws(&env, &format!("tdsales{tag}"));
    let h = sales.handle.clone();
    let agent = passport(&sales);
    // A card answer or void wakes the agent; its first wake spawns the
    // Chat (the shim) and pins that new session, so the address answers
    // at a NEW conversation from then on. Wake it first so everything
    // below lands on one conversation (empty: seqs start at 1 there).
    let conv = warm_up(port, &h, &seeded).await;

    // 4. The feature key.
    let boot = ok_get(port, "/boot-status", OWNER, "").await;
    assert!(boot["features"].as_array().expect("features").iter().any(|f| f == "thread-delta-v1"), "feature key: {boot}");

    for i in 1..=5 {
        assert_eq!(agent_post(port, &h, &format!("m{i}")).await, i);
    }

    // Forward pages, oldest first, has_more = NEWER items remain.
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&after_seq=0&limit=2")).await;
    assert_eq!((seqs(&v["items"]), v["has_more"].as_bool()), (vec![1, 2], Some(true)), "{v}");
    assert_eq!(v["conversation_id"], conv.as_str(), "conversation_id kept: {v}");
    assert_eq!((v["after_seq"].as_i64(), v["collection"].as_str(), v["addr"].as_str()), (Some(0), Some("thread"), Some(h.as_str())), "{v}");
    assert_eq!((v["changed"].clone(), v["removed"].clone()), (json!([]), json!([])), "{v}");
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&after_seq=2&limit=2")).await;
    assert_eq!((seqs(&v["items"]), v["has_more"].as_bool()), (vec![3, 4], Some(true)), "{v}");
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&after_seq=4&limit=2")).await;
    assert_eq!((seqs(&v["items"]), v["has_more"].as_bool()), (vec![5], Some(false)), "{v}");
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&after_seq=5")).await;
    assert_eq!((seqs(&v["items"]), v["has_more"].as_bool()), (vec![], Some(false)), "{v}");
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&after_seq=1&limit=0")).await;
    assert_eq!((seqs(&v["items"]), v["has_more"].as_bool()), (vec![2, 3, 4, 5], Some(false)), "limit 0 = all: {v}");
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&after_seq=99")).await;
    assert_eq!((seqs(&v["items"]), v["has_more"].as_bool()), (vec![], Some(false)), "past the end: {v}");

    // No after_seq: exactly the old pages and the old keys.
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&limit=2")).await;
    assert_eq!((seqs(&v["items"]), v["has_more"].as_bool()), (vec![4, 5], Some(true)), "newest-N, has_more = older remain: {v}");
    let mut keys: Vec<&str> = v.as_object().expect("object").keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["addr", "collection", "conversation_id", "has_more", "items", "ok", "pastAddresses"], "old shape: {v}");
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&since_seq=3")).await;
    assert_eq!(seqs(&v["items"]), vec![4, 5], "{v}");
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&since_seq=0&limit=2")).await;
    assert_eq!((seqs(&v["items"]), v["has_more"].as_bool()), (vec![4, 5], Some(true)), "since_seq stays newest-N: {v}");
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&before_seq=3")).await;
    assert_eq!(seqs(&v["items"]), vec![1, 2], "{v}");
    assert!(v.get("after_seq").is_none() && v.get("changed").is_none(), "{v}");

    // Cards: the reader holds 1..=7 (both cards pending).
    let a = ok_post(port, "/cli/thread/ask", OWNER, json!({ "addr": h, "prompt": "Ship A?", "options": "Ship,Wait" })).await;
    let b = ok_post(port, "/cli/thread/ask", OWNER, json!({ "addr": h, "prompt": "Ship B?", "options": "Ship,Wait" })).await;
    let (a_id, b_id) = (a["id"].as_str().expect("a id").to_string(), b["id"].as_str().expect("b id").to_string());
    assert_eq!((a["seq"].as_i64(), b["seq"].as_i64()), (Some(6), Some(7)), "{a} {b}");
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&after_seq=5")).await;
    assert_eq!(seqs(&v["items"]), vec![6, 7]);
    assert_eq!((status_of(&v["items"][0]), status_of(&v["items"][1])), ("pending".to_string(), "pending".to_string()), "{v}");
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&after_seq=7")).await;
    assert_eq!(v["changed"], json!([]), "nothing changed yet: {v}");

    // Card A is answered (the asking agent's own answer: no wake).
    ok_post(port, "/cli/thread/answer", &agent, json!({ "addr": h, "id": a_id, "answer": "Ship" })).await;
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&after_seq=7")).await;
    assert_eq!(v["conversation_id"], conv.as_str(), "still the warmed-up conversation: {v}");
    assert_eq!((seqs(&v["items"]), seqs(&v["changed"])), (vec![], vec![6]), "the answer is in changed: {v}");
    assert_eq!(v["changed"][0]["id"], a_id.as_str());
    assert_eq!(status_of(&v["changed"][0]), "answered");
    assert_eq!(v["changed"][0]["doc"]["choice"]["answer"], "Ship");

    // Card B is voided (`k2 thread void`): both changes, seq order.
    ok_post(port, "/cli/thread/void", &agent, json!({ "addr": h, "id": b_id })).await;
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&after_seq=7")).await;
    assert_eq!(seqs(&v["changed"]), vec![6, 7], "{v}");
    assert_eq!((status_of(&v["changed"][0]), status_of(&v["changed"][1])), ("answered".to_string(), "voided".to_string()));
    assert_eq!(v["changed"][1]["id"], b_id.as_str());

    // A reader at 6 gets B in items (current state), A in changed, never twice.
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&after_seq=6")).await;
    assert_eq!((seqs(&v["items"]), seqs(&v["changed"])), (vec![7], vec![6]), "{v}");
    assert_eq!(status_of(&v["items"][0]), "voided");
    // A has_more page still carries every change below the cursor.
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&after_seq=0&limit=1")).await;
    assert_eq!((seqs(&v["items"]), v["has_more"].as_bool(), seqs(&v["changed"])), (vec![1], Some(true), vec![]), "{v}");
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&after_seq=6&limit=0")).await;
    assert_eq!(seqs(&v["changed"]), vec![6], "{v}");

    // After a newer post the reader that fetched it (cursor 8) has seen both.
    assert_eq!(agent_post(port, &h, "m8").await, 8);
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&after_seq=8")).await;
    assert_eq!((seqs(&v["items"]), seqs(&v["changed"])), (vec![], vec![]), "{v}");
    // The old reads still show the cards' current state.
    let v = ok_get(port, "/cli/thread", OWNER, &format!("addr={h}&limit=3")).await;
    assert_eq!(seqs(&v["items"]), vec![6, 7, 8]);
    assert_eq!((status_of(&v["items"][0]), status_of(&v["items"][1])), ("answered".to_string(), "voided".to_string()));

    // Bad params are loud.
    for q in ["after_seq=-1", "after_seq=abc", "after_seq=", "after_seq=2&since_seq=1", "after_seq=2&before_seq=5"] {
        let r = get(port, "/cli/thread", OWNER, &format!("addr={h}&{q}")).await;
        assert_eq!(r.status, 400, "{q}: {}", r.body);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_messages_after_seq_pages_forward_and_keeps_the_old_reads() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let (member, _) = pinned_ws(&env, &format!("tdmember{tag}"));
    let g = ok_post(port, "/cli/project-group/create", OWNER, json!({ "name": format!("Delta {tag}") })).await;
    let gid = g["group"]["id"].as_str().or_else(|| g["id"].as_str()).unwrap_or_else(|| panic!("group id: {g}")).to_string();
    ok_post(port, "/cli/project-group/add-member", OWNER, json!({ "group": gid, "workspace": member.id })).await;
    for i in 1..=5 {
        let m = ok_post(port, "/cli/project-group/msg", OWNER, json!({ "group": gid, "body": format!("p{i}") })).await;
        assert_eq!(m["seq"].as_i64(), Some(i), "{m}");
    }
    let msgs = |v: &Value| seqs(&v["messages"]);

    let v = ok_get(port, "/cli/project-group/messages", OWNER, &format!("group={gid}&after_seq=0&limit=2")).await;
    assert_eq!((msgs(&v), v["has_more"].as_bool(), v["truncated"].as_bool()), (vec![1, 2], Some(true), Some(true)), "{v}");
    assert_eq!((v["groupId"].as_str(), v["after_seq"].as_i64()), (Some(gid.as_str()), Some(0)), "{v}");
    assert_eq!((v["changed"].clone(), v["removed"].clone()), (json!([]), json!([])), "project messages never change: {v}");
    assert_eq!(v["messages"][0]["body"], "p1");
    let v = ok_get(port, "/cli/project-group/messages", OWNER, &format!("group={gid}&after_seq=2&limit=2")).await;
    assert_eq!((msgs(&v), v["has_more"].as_bool()), (vec![3, 4], Some(true)), "{v}");
    let v = ok_get(port, "/cli/project-group/messages", OWNER, &format!("group={gid}&after_seq=4&limit=2")).await;
    assert_eq!((msgs(&v), v["has_more"].as_bool()), (vec![5], Some(false)), "{v}");
    let v = ok_get(port, "/cli/project-group/messages", OWNER, &format!("group={gid}&after_seq=5")).await;
    assert_eq!((msgs(&v), v["has_more"].as_bool()), (vec![], Some(false)), "{v}");
    let v = ok_get(port, "/cli/project-group/messages", OWNER, &format!("group={gid}&after_seq=1")).await;
    assert_eq!((msgs(&v), v["has_more"].as_bool()), (vec![2, 3, 4, 5], Some(false)), "default limit fits: {v}");

    // No after_seq: the old latest-N page and its old keys.
    let v = ok_get(port, "/cli/project-group/messages", OWNER, &format!("group={gid}&limit=2")).await;
    assert_eq!((msgs(&v), v["truncated"].as_bool()), (vec![4, 5], Some(true)), "{v}");
    let mut keys: Vec<&str> = v.as_object().expect("object").keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["groupId", "messages", "ok", "truncated"], "old shape: {v}");
    let v = ok_get(port, "/cli/project-group/messages", OWNER, &format!("group={gid}&after=0")).await;
    assert_eq!(msgs(&v), vec![1, 2, 3, 4, 5], "after=<ts> unchanged: {v}");

    for q in ["after_seq=-1", "after_seq=x", "after_seq=1&after=0", "after_seq=1&limit=0"] {
        let r = get(port, "/cli/project-group/messages", OWNER, &format!("group={gid}&{q}")).await;
        assert_eq!(r.status, 400, "{q}: {}", r.body);
    }
    let r = get(port, "/cli/project-group/messages", OWNER, "group=no-such-project-at-all&after_seq=0").await;
    assert_eq!(r.status, 404, "{}", r.body);

    // Auth unchanged: owner and agents read; no token and an App guest don't.
    let agent = passport(&member);
    let guest_pass = guest(port, &format!("tdg{tag}"), &[(&member, &["thread:read"])]).await;
    for (who, tok) in [("owner", OWNER), ("agent", agent.as_str()), ("no token", ""), ("app guest", guest_pass.as_str())] {
        let old = get(port, "/cli/project-group/messages", tok, &format!("group={gid}")).await;
        let new = get(port, "/cli/project-group/messages", tok, &format!("group={gid}&after_seq=0")).await;
        assert_eq!(outcome(&old), outcome(&new), "{who}: {} | {}", old.body, new.body);
        if matches!(who, "owner" | "agent") {
            assert_eq!(new.status, 200, "{who}: {}", new.body);
        } else {
            assert!(new.status == 401 || new.status == 403, "{who} is refused: {} {}", new.status, new.body);
            assert!(!new.body.contains("p1"), "{who}: a refusal carries no message text: {}", new.body);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thread_after_seq_keeps_every_callers_read_rules() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let (sales, _) = pinned_ws(&env, &format!("tdasales{tag}"));
    let (ops, _) = pinned_ws(&env, &format!("tdaops{tag}"));
    let (quiet, _) = pinned_ws(&env, &format!("tdaquiet{tag}"));
    for ws in [&sales, &ops, &quiet] {
        agent_post(port, &ws.handle, "hello").await;
    }
    let same = passport(&sales);
    let other = passport(&ops);
    let dana = guest(port, &format!("tdadana{tag}"), &[(&sales, &["thread:read"]), (&quiet, &["files:read"])]).await;

    let cases: Vec<(&str, &str, &Ws, u16)> = vec![
        ("owner", OWNER, &sales, 200),
        ("same-workspace agent", same.as_str(), &sales, 200),
        ("other-workspace agent", other.as_str(), &sales, 403),
        ("no token", "", &sales, 403),
        ("guest, own room", dana.as_str(), &sales, 200),
        ("guest, room without thread:read", dana.as_str(), &quiet, 403),
        ("guest, room it doesn't hold", dana.as_str(), &ops, 0),
    ];
    for (who, tok, ws, want) in cases {
        let old = get(port, "/cli/thread", tok, &format!("addr={}", ws.handle)).await;
        let new = get(port, "/cli/thread", tok, &format!("addr={}&after_seq=0", ws.handle)).await;
        assert_eq!(outcome(&old), outcome(&new), "{who}: same status and code with and without after_seq: {} | {}", old.body, new.body);
        if want != 0 {
            assert_eq!(new.status, want, "{who}: {}", new.body);
        } else {
            assert_ne!(new.status, 200, "{who} must not read a room it doesn't hold: {}", new.body);
        }
        if new.status == 200 {
            let v = new.json();
            assert_eq!(seqs(&v["items"]), vec![1], "{who}: {v}");
            assert_eq!(v["after_seq"], 0);
        } else {
            assert!(!new.body.contains("hello"), "{who}: a refusal carries no Thread text: {}", new.body);
        }
    }
}
