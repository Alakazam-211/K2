//! Ticket discussions on the Thread store (ticket-thread-v1, 0.46.17),
//! headless: the real dispatcher in-process on a temp `$HOME`, over
//! loopback.
//!
//! - A ticket's discussion is the Thread conversation `ticket:<id>`: the
//!   Thread routes read it (pages and `after_seq` deltas) and post to it.
//! - A Thread post keeps every ticket rule: an option pick is the answer
//!   (`ask --wait` reads it on `show`), free text moves the ticket to
//!   `needs_discussion` and reopens a resolved one, an agent's post is
//!   stored only, a person's is delivered as `[feedback:<id>]`, a send-once
//!   repeat stores nothing, `command` and the card routes are refused.
//! - The ticket routes answer the same shapes, reading and writing the
//!   same Thread (the facade).
//! - A ticket's Thread never reaches a person's unread summary, but its
//!   own read cursor moves.
//! - Access follows the ticket's rights: an App pass needs `tickets:read`
//!   (not `thread:read`) in the ticket's room; a Member gets not found for
//!   K2's own notices; another workspace's agent is refused; files are
//!   checked against the ticket's workspace; file links map `ticket:<id>`
//!   to its workspace.

#![cfg(unix)]

use crate::support::app_fixture;

use std::path::Path;

use app_fixture::*;
use serde_json::{json, Value};

const PW: &str = "s3cret-horse-staple";

fn write(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn owner_ok(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

async fn login_with_role(port: u16, username: &str, role: &str) -> String {
    owner_ok(port, "/cli/skin/users", json!({ "username": username })).await;
    k2_daemon::test_harness::guest_with_own_password(username, PW);
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body)))
        .await
        .unwrap();
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

async fn role_pass(port: u16, ws: &Ws, name: &str, caps: &[&str]) -> String {
    owner_ok(port, "/cli/skin/roles", json!({ "name": name })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": name, "handle": ws.handle, "caps": caps })).await;
    login_with_role(port, &format!("u{name}"), name).await
}

fn code_of(r: &Resp) -> String {
    r.json()["error"]["code"].as_str().unwrap_or_else(|| panic!("no code: {}", r.body)).to_string()
}

async fn thread(port: u16, token: &str, addr: &str, extra: &str) -> Resp {
    get(port, "/cli/thread", token, &format!("addr={}&{extra}", enc(addr))).await
}

async fn show(port: u16, token: &str, id: &str) -> Value {
    let r = get(port, "/cli/feedback/show", token, &format!("id={}", enc(id))).await;
    assert_eq!(r.status, 200, "show: {}", r.body);
    r.json()
}

async fn create(port: u16, ws: &Ws, title: &str) -> String {
    let v = owner_ok(
        port,
        "/cli/feedback/create",
        json!({ "project": ws.id, "title": title, "options": ["Yes", "No"], "agentName": "agent-a", "assignees": ["owner"], "briefHtml": "<p>Pick</p>" }),
    )
    .await;
    v["id"].as_str().expect("ticket id").to_string()
}

/// Bites: the posting rules on a Thread post, the facade shapes, deltas,
/// send-once, the refusals, and the unread exclusion.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ticket_thread_post_keeps_every_ticket_rule() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("ttp{tag}"));
    let id = create(port, &ws, "Ship it?").await;
    let addr = format!("ticket:{id}");

    // The conversation: `ticket:<id>`, its first item the ask.
    let r = thread(port, OWNER, &addr, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["conversation_id"], addr.as_str());
    assert_eq!(v["addr"], addr.as_str());
    assert_eq!(v["items"][0]["seq"], 1);
    assert_eq!(v["items"][0]["doc"]["ticket"], id.as_str(), "each message carries its ticket: {v}");
    assert_eq!(v["items"][0]["doc"]["from"], "agent-a");
    // A unique prefix reaches it too.
    let r = thread(port, OWNER, &format!("ticket:{}", &id[..8]), "").await;
    assert_eq!((r.status, r.json()["conversation_id"].clone()), (200, json!(addr)), "{}", r.body);

    // A person's free text: stored, needs_discussion, delivered as [feedback:].
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "Tell me more", "via": "compose", "clientMsgId": "tt-1" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let p = r.json();
    assert_eq!((p["seq"].clone(), p["ticket"]["status"].clone(), p["ticket"]["answered"].clone()), (json!(2), json!("needs_discussion"), json!(false)), "{p}");
    assert_eq!(p["author"]["kind"], "owner", "{p}");
    assert_eq!(p["mine"], true, "{p}");
    assert_eq!(p["clientMsgId"], "tt-1");
    let post_id = p["id"].as_str().unwrap().to_string();
    let inject = k2_daemon::ticket_attachments::last_inject(&id).expect("a person's words reach the agent");
    assert!(inject.contains(&format!("[feedback:{}]", &id[..8])) && inject.contains("Tell me more"), "{inject}");

    // Send-once: the repeat stores nothing.
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "Tell me more", "via": "compose", "clientMsgId": "tt-1" })).await;
    assert_eq!((r.status, r.json()["duplicate"].clone(), r.json()["id"].clone()), (200, json!(true), json!(post_id)), "{}", r.body);

    // The facade: show answers the same shape, from the same store.
    let s = show(port, OWNER, &id).await;
    let comments = s["comments"].as_array().unwrap().clone();
    assert_eq!(comments.len(), 2, "{s}");
    assert_eq!(comments[1]["id"], post_id.as_str(), "a comment id is its Thread doc id");
    assert_eq!(comments[1]["body"], "Tell me more");
    let mut keys: Vec<&str> = comments[1].as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, vec!["at", "author", "body", "id"], "the comment shape is unchanged");
    assert_eq!((s["commentCount"].clone(), s["status"].clone()), (json!(2), json!("needs_discussion")));

    // An agent comment through the ticket route lands in the same Thread.
    let r = post(port, "/cli/feedback/comment", OWNER, json!({ "id": id, "body": "Here is more", "author": "agent-a" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let mut keys: Vec<String> = r.json().as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, vec!["attachments", "author", "commentId", "id", "ok"], "the agent comment answer is unchanged");
    let r = thread(port, OWNER, &addr, "after_seq=2").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let delta = r.json();
    assert_eq!(delta["items"].as_array().unwrap().len(), 1, "{delta}");
    assert_eq!(delta["items"][0]["doc"]["body"], "Here is more");

    // An option pick through the Thread IS the answer (ask --wait reads show).
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "Yes", "optionPick": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((r.json()["ticket"]["status"].clone(), r.json()["ticket"]["answered"].clone()), (json!("answered"), json!(true)), "{}", r.body);
    let s = show(port, OWNER, &id).await;
    assert_eq!((s["status"].clone(), s["answer"].clone()), (json!("answered"), json!("Yes")), "{s}");

    // Free text reopens a resolved ticket.
    owner_ok(port, "/cli/feedback/resolve", json!({ "id": id })).await;
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "One more thing" })).await;
    assert_eq!(r.json()["ticket"]["status"], "needs_discussion", "{}", r.body);

    // An agent's own post: stored only, no status move, no delivery.
    let agent = passport(&ws);
    let r = post(port, "/cli/thread/post", &agent, json!({ "addr": addr, "text": "On it", "optionPick": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["ticket"]["status"].clone(), v["ticket"]["answered"].clone(), v["ticket"]["delivered"].clone()), (json!("needs_discussion"), json!(false), Value::Null), "{v}");
    assert_eq!(v["author"]["kind"], "agent", "{v}");

    // A ticket takes messages: no command, no cards.
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "x", "via": "compose", "command": "/compact" })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (400, "usage"), "{}", r.body);
    let r = post(port, "/cli/thread/ask", OWNER, json!({ "addr": addr, "text": "Pick?", "options": "a,b" })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (400, "ticket_messages_only"), "{}", r.body);

    // Unread: a ticket's messages never reach the summary; its cursor moves.
    let r = get(port, "/cli/thread/unread", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!r.body.contains("ticket:"), "no ticket conversation in the unread summary: {}", r.body);
    let r = post(port, "/cli/thread/read", OWNER, json!({ "conversationId": addr })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((r.json()["unread"].clone(), r.json()["lastReadSeq"].clone()), (json!(0), r.json()["lastSeq"].clone()), "{}", r.body);
    assert_eq!(r.json()["id"], addr.as_str(), "{}", r.body);

    // The phone's all:true stream leaves ticket conversations out.
    let all = k2_daemon::inbox_stream::InboxSet { all: true, ..Default::default() };
    assert!(!all.contains(&addr) && all.contains("conv-any"));
    let named = k2_daemon::inbox_stream::InboxSet { ids: vec![addr.clone()], ..Default::default() };
    assert!(named.contains(&addr), "a phone that names it gets it");
}

/// Bites: access follows the ticket's rights.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ticket_thread_follows_the_tickets_rights() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("tta{tag}"));
    let other = seed_ws(&env, &format!("tto{tag}"));
    write(&ws.path, "docs/a.md", b"# A\n");
    write(&other.path, "secret.md", b"theirs\n");
    let id = create(port, &ws, "Rights").await;
    let addr = format!("ticket:{id}");

    // An App pass with tickets:read/post (no thread:read) reads and posts.
    let tickets_pass = role_pass(port, &ws, &format!("tk{tag}"), &["tickets:read", "tickets:post"]).await;
    let r = thread(port, &tickets_pass, &addr, "").await;
    assert_eq!(r.status, 200, "tickets:read reads the ticket's Thread: {}", r.body);
    let r = post(port, "/cli/thread/post", &tickets_pass, json!({ "addr": addr, "text": "From the app" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["author"]["kind"], "guest", "{}", r.body);
    assert_eq!(show(port, OWNER, &id).await["comments"][1]["author"], format!("utk{tag}"), "stored as the pass's username");

    // thread:read alone never reaches a ticket.
    let thread_pass = role_pass(port, &ws, &format!("th{tag}"), &["thread:read", "thread:post"]).await;
    let r = thread(port, &thread_pass, &addr, "").await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert!(r.body.contains("tickets:read"), "the ticket's cap is named: {}", r.body);
    let r = post(port, "/cli/thread/post", &thread_pass, json!({ "addr": addr, "text": "nope" })).await;
    assert_eq!(r.status, 403, "{}", r.body);

    // Another room's pass: skin_room, no oracle.
    let other_pass = role_pass(port, &other, &format!("ot{tag}"), &["tickets:read", "tickets:post"]).await;
    let r = thread(port, &other_pass, &addr, "").await;
    assert_eq!((r.status, code_of(&r).as_str()), (403, "skin_room"), "{}", r.body);

    // A Member never sees K2's own notices; an ordinary ticket, yes.
    let sentinel = k2_core::db::AUDIT_SENTINEL_IDS[0];
    let notice = k2_core::feedback::create(k2_core::feedback::NewFeedback {
        project_id: sentinel.to_string(),
        agent_name: "K2".into(),
        kind: "fyi".into(),
        title: "Disk".into(),
        ..Default::default()
    })
    .expect("notice");
    let member = format!("tmem{tag}");
    k2_core::connect_users::add_user(&member, "password123").expect("add_user");
    let member_tok = k2_core::connect_users::create_session(&member);
    let notice_addr = format!("ticket:{}", notice.id);
    let r = thread(port, &member_tok, &notice_addr, "").await;
    assert_eq!((r.status, code_of(&r).as_str()), (404, "not_found"), "{}", r.body);
    let r = post(port, "/cli/thread/post", &member_tok, json!({ "addr": notice_addr, "text": "peek" })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (404, "not_found"), "{}", r.body);
    let r = thread(port, OWNER, &notice_addr, "").await;
    assert_eq!(r.status, 200, "the owner sees it: {}", r.body);
    let r = thread(port, &member_tok, &addr, "").await;
    assert_eq!(r.status, 200, "a Member reads an ordinary ticket: {}", r.body);

    // Another workspace's agent: refused; its own workspace's: allowed.
    let r = thread(port, &passport(&other), &addr, "").await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = thread(port, &passport(&ws), &addr, "").await;
    assert_eq!(r.status, 200, "{}", r.body);

    // Files are checked against the TICKET's workspace.
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "see", "attachments": [{ "path": "docs/a.md" }] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["attachments"][0]["path"], "docs/a.md", "{}", r.body);
    let outside = other.path.join("secret.md").to_string_lossy().to_string();
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "theirs", "attachments": [{ "path": outside }] })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (400, "attachment_outside_workspace"), "{}", r.body);
    assert_eq!(show(port, OWNER, &id).await["fileCount"], 1, "the refused post stored nothing");

    // File links: `ticket:<id>` names the ticket's workspace.
    let r = post(port, "/cli/fs/resolve-links", OWNER, json!({ "workspace": addr, "paths": ["docs/a.md"] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["root"], json!(ws.path.to_string_lossy()), "{}", r.body);
    assert_eq!(r.json()["links"][0]["linkable"], true, "{}", r.body);

    // The attachment door on the ticket's address.
    let s = show(port, OWNER, &id).await;
    let with_file = s["comments"].as_array().unwrap().iter().find(|c| c.get("attachments").is_some()).cloned().expect("a message with a file");
    let r = get(port, "/cli/thread/attachment", OWNER, &format!("addr={}&id={}&n=0&mode=meta", enc(&addr), with_file["id"].as_str().unwrap())).await;
    assert_eq!(r.status, 200, "{}", r.body);
}

type Sock = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn frame_for(sock: &mut Sock, id: &str) -> Value {
    use futures_util::StreamExt;
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(10), sock.next())
            .await
            .expect("a frame")
            .expect("open")
            .expect("ok");
        let Ok(t) = msg.to_text() else { continue };
        let Ok(v) = serde_json::from_str::<Value>(t) else { continue };
        if v["id"] == id {
            return v;
        }
    }
}

fn overlay_url(port: u16, conversation: &str, token: &str) -> String {
    format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={}&token={}", enc(conversation), enc(token))
}

/// Bite: the overlay socket on `ticket:<id>` carries every stored ticket
/// message (from the ticket route too), on the ticket's rights.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ticket_thread_streams_on_the_tickets_rights() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("tts{tag}"));
    let id = create(port, &ws, "Live?").await;
    let conv = format!("ticket:{id}");

    let (mut owner_sock, _) = tokio_tungstenite::connect_async(&overlay_url(port, &conv, OWNER)).await.expect("owner socket");
    let tickets_pass = role_pass(port, &ws, &format!("sk{tag}"), &["tickets:read", "tickets:post"]).await;
    let (mut app_sock, _) = tokio_tungstenite::connect_async(&overlay_url(port, &conv, &tickets_pass)).await.expect("tickets:read opens it");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let r = post(port, "/cli/feedback/comment", OWNER, json!({ "id": id, "body": "via the ticket route", "author": "agent-a" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let cid = r.json()["commentId"].as_str().unwrap().to_string();
    let f = frame_for(&mut owner_sock, &cid).await;
    assert_eq!((f["collection"].clone(), f["doc"]["body"].clone(), f["doc"]["ticket"].clone()), (json!("thread"), json!("via the ticket route"), json!(id)), "{f}");
    let g = frame_for(&mut app_sock, &cid).await;
    assert_eq!(g["doc"]["body"], "via the ticket route", "{g}");

    // (a) An App pass with tickets:post types on its ticket's Thread; the
    // owner's socket hears it.
    {
        use futures_util::SinkExt;
        let frame = json!({ "t": "typing", "thread": conv }).to_string();
        app_sock.send(tokio_tungstenite::tungstenite::Message::Text(frame.into())).await.expect("send typing");
    }
    let t = frame_for(&mut owner_sock, &conv).await;
    assert_eq!(t["collection"], "typing", "{t}");

    // (b) Its activity read needs tickets:read there, not activity:read.
    let r = get(port, "/cli/thread/activity", &tickets_pass, &format!("addr={}", enc(&conv))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["conversation_id"], conv.as_str(), "{}", r.body);

    // thread:read alone, and a Member on K2's own notice: refused.
    let thread_pass = role_pass(port, &ws, &format!("st{tag}"), &["thread:read"]).await;
    assert!(tokio_tungstenite::connect_async(&overlay_url(port, &conv, &thread_pass)).await.is_err(), "thread:read never opens a ticket");
    let notice = k2_core::feedback::create(k2_core::feedback::NewFeedback {
        project_id: k2_core::db::AUDIT_SENTINEL_IDS[0].to_string(),
        agent_name: "K2".into(),
        kind: "fyi".into(),
        title: "Disk".into(),
        ..Default::default()
    })
    .expect("notice");
    let member = format!("smem{tag}");
    k2_core::connect_users::add_user(&member, "password123").expect("add_user");
    let member_tok = k2_core::connect_users::create_session(&member);
    let notice_conv = format!("ticket:{}", notice.id);
    assert!(tokio_tungstenite::connect_async(&overlay_url(port, &notice_conv, &member_tok)).await.is_err(), "a Member never watches a notice");
    assert!(tokio_tungstenite::connect_async(&overlay_url(port, &conv, &member_tok)).await.is_ok(), "a Member watches an ordinary ticket");
}

fn rollback_copy(comment_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT body FROM feedback_comments WHERE id = ?1", rusqlite::params![comment_id], |r| r.get::<_, String>(0))
        .ok()
}

/// Bites: (1) a Thread post lands in the ticket's Thread AND its rollback
/// copy; (d) deleting it tells the agent `[feedback:<id>] …`, takes the
/// copy, and leaves `show`; (c) file links on a notice follow the Member
/// rule.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ticket_message_is_copied_deleted_and_linked_by_the_tickets_rules() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("ttd{tag}"));
    let id = create(port, &ws, "Copies?").await;
    let addr = format!("ticket:{id}");

    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "oops, a password here", "via": "compose" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let mid = r.json()["id"].as_str().unwrap().to_string();
    assert_eq!(rollback_copy(&mid).as_deref(), Some("oops, a password here"), "the post lands in feedback_comments too, same id");

    let r = post(port, "/cli/thread/message/delete", OWNER, json!({ "addr": addr, "id": mid })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["deleted"], true, "{}", r.body);
    let inject = k2_daemon::ticket_attachments::last_inject(&id).expect("the agent hears the delete");
    assert!(inject.contains(&format!("[feedback:{}]", &id[..8])) && inject.contains("deleted a message"), "{inject}");
    assert!(!inject.contains("password"), "never the message: {inject}");
    assert_eq!(rollback_copy(&mid), None, "the copy goes with the message");
    let s = show(port, OWNER, &id).await;
    assert_eq!((s["comments"].as_array().unwrap().len(), s["commentCount"].clone()), (1, json!(1)), "{s}");

    // (c) File links on K2's own notice: a Member gets not found.
    write(&ws.path, "a.md", b"a\n");
    let notice = k2_core::feedback::create(k2_core::feedback::NewFeedback {
        project_id: k2_core::db::AUDIT_SENTINEL_IDS[0].to_string(),
        agent_name: "K2".into(),
        kind: "fyi".into(),
        title: "Disk".into(),
        ..Default::default()
    })
    .expect("notice");
    let member = format!("dmem{tag}");
    k2_core::connect_users::add_user(&member, "password123").expect("add_user");
    let member_tok = k2_core::connect_users::create_session(&member);
    let notice_addr = format!("ticket:{}", notice.id);
    let r = post(port, "/cli/fs/resolve-links", &member_tok, json!({ "workspace": notice_addr, "paths": ["a.md"] })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (404, "not_found"), "{}", r.body);
    let r = post(port, "/cli/fs/resolve-links", &member_tok, json!({ "workspace": addr, "paths": ["a.md"] })).await;
    assert_eq!(r.status, 200, "a Member links an ordinary ticket's files: {}", r.body);
}
