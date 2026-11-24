//! Project chats on the Thread store (project-chat-thread-v1), headless:
//! the real dispatcher in-process on a temp `$HOME`, over loopback.
//!
//! - A project's chat is the Thread conversation `project:<group id>`: the
//!   Thread routes read it (by id, name or prefix) and post to it, and the
//!   old project routes read and write the same lines (views over it).
//! - One write path: a Thread post is a project line (stored, delivered
//!   `[project:<name>]` to the point of contact, its own lines never), with
//!   send-once across both doors; the card routes are refused.
//! - Access is the project's: a member agent reads and posts, any other
//!   agent is `not_a_member`; an App pass needs `thread:read` in every
//!   member room (else `skin_room`) and `project:write` to post.
//! - Delete: the sender's own line becomes a tombstone, the old route
//!   leaves it out and lists it in `removed`, the point of contact hears it
//!   without the text.
//! - Files ride a line, kept in the point of contact's workspace; a member
//!   agent's own file is copied there; the point of contact gets
//!   `Attached:` lines.
//! - A Thread read mark on `project:<id>` moves the project's read cursor.

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

/// An App pass with `caps` in each of `rooms`.
async fn pass_in(port: u16, rooms: &[&Ws], name: &str, caps: &[&str]) -> String {
    owner_ok(port, "/cli/skin/roles", json!({ "name": name })).await;
    for ws in rooms {
        owner_ok(port, "/cli/skin/roles/room", json!({ "name": name, "handle": ws.handle, "caps": caps })).await;
    }
    login_with_role(port, &format!("u{name}"), name).await
}

fn code_of(r: &Resp) -> String {
    r.json()["error"]["code"].as_str().unwrap_or_else(|| panic!("no code: {}", r.body)).to_string()
}

async fn thread(port: u16, token: &str, addr: &str, extra: &str) -> Resp {
    get(port, "/cli/thread", token, &format!("addr={}&{extra}", enc(addr))).await
}

async fn messages(port: u16, gid: &str, extra: &str) -> Value {
    let r = get(port, "/cli/project-group/messages", OWNER, &format!("group={gid}&{extra}")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()
}

/// A project with `members` (the first is its point of contact).
async fn project(port: u16, name: &str, members: &[&Ws]) -> String {
    let g = owner_ok(port, "/cli/project-group/create", json!({ "name": name })).await;
    let gid = g["group"]["id"].as_str().or_else(|| g["id"].as_str()).expect("group id").to_string();
    for m in members {
        owner_ok(port, "/cli/project-group/add-member", json!({ "group": gid, "workspace": m.id })).await;
    }
    gid
}

/// Bites: one store behind both doors, the PoC delivery, send-once across
/// doors, the refusals.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_project_chat_is_a_thread_behind_both_doors() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let poc = seed_ws(&env, &format!("pcp{tag}"));
    let member = seed_ws(&env, &format!("pcm{tag}"));
    let name = format!("Launch {tag}");
    let gid = project(port, &name, &[&poc, &member]).await;
    let addr = format!("project:{gid}");

    // The old door: stored as Thread item 1 of `project:<id>`.
    let m1 = owner_ok(port, "/cli/project-group/msg", json!({ "group": gid, "body": "kickoff", "clientMsgId": "pc-1" })).await;
    assert_eq!((m1["seq"].clone(), m1["author"].clone(), m1["authorKind"].clone()), (json!(1), json!("owner"), json!("person")), "{m1}");
    assert_eq!(m1["deliveryReason"], "queued", "a person's line answers before delivery: {m1}");
    let line = k2_daemon::project_group_routes::last_poc_line(m1["id"].as_str().unwrap()).expect("the PoC got the line");
    assert!(line.contains(&format!("[project:{name}] kickoff")), "{line}");

    // The Thread door reads it, by id, by name and by a unique name
    // prefix (`k2 project`'s own lookup).
    for a in [addr.clone(), format!("project:{name}"), format!("project:{}", &name[..11])] {
        let r = thread(port, OWNER, &a, "").await;
        assert_eq!(r.status, 200, "{a}: {}", r.body);
        let v = r.json();
        assert_eq!(v["conversation_id"], addr.as_str(), "{v}");
        assert_eq!(v["items"][0]["id"], m1["id"], "{v}");
        assert_eq!(v["items"][0]["doc"]["author"]["kind"], "owner", "{v}");
    }

    // A Thread post is a project line: same seq run, same delivery.
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "agenda?", "clientMsgId": "pc-2" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let p = r.json();
    assert_eq!((p["seq"].clone(), p["mine"].clone()), (json!(2), json!(true)), "{p}");
    assert_eq!(p["project"]["groupId"], gid.as_str(), "{p}");
    assert_eq!(p["project"]["deliveryReason"], "queued", "{p}");
    let v = messages(port, &gid, "").await;
    assert_eq!(v["messages"].as_array().unwrap().len(), 2, "{v}");
    assert_eq!(v["messages"][1]["body"], "agenda?");
    assert!(v["messages"][1].get("attachments").is_none(), "a text line's wire is unchanged: {v}");

    // Send-once across doors: the old door's key answers the Thread door.
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "kickoff", "clientMsgId": "pc-1" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((r.json()["duplicate"].clone(), r.json()["id"].clone()), (json!(true), m1["id"].clone()), "{}", r.body);
    let other = project(port, &format!("Other {tag}"), &[&poc]).await;
    let r = post(port, "/cli/project-group/msg", OWNER, json!({ "group": other, "body": "kickoff", "clientMsgId": "pc-1" })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (409, "client_msg_id_reused"), "{}", r.body);

    // A member agent posts as itself; the PoC hears it. The PoC's own line
    // is never sent back to it.
    let r = post(port, "/cli/thread/post", &passport(&member), json!({ "addr": addr, "text": "on it" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let a = r.json();
    assert_eq!(a["from"], member.handle.as_str(), "stored as the workspace's name: {a}");
    let line = k2_daemon::project_group_routes::last_poc_line(a["id"].as_str().unwrap()).expect("the PoC got the agent's line");
    assert!(line.contains("on it"), "{line}");
    let r = post(port, "/cli/thread/post", &passport(&poc), json!({ "addr": addr, "text": "noted" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["project"]["deliveryReason"], "author-is-poc", "{}", r.body);

    // Cards are refused.
    for path in ["/cli/thread/ask", "/cli/thread/secret"] {
        let r = post(port, path, OWNER, json!({ "addr": addr, "text": "pick", "options": ["a", "b"] })).await;
        assert_eq!((r.status, code_of(&r).as_str()), (400, "project_messages_only"), "{path}: {}", r.body);
    }

    // A Thread read mark is the project's cursor.
    let r = post(port, "/cli/thread/read", OWNER, json!({ "conversationId": addr, "seq": 2 })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let item = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let owner = k2_core::read_state::Person::of_login(&k2_core::notify_prefs::People::default(), "owner");
        k2_core::read_state::item(&conn, &owner, k2_core::read_state::KIND_PROJECT, &gid, k2_core::read_state::now_secs()).expect("item")
    };
    assert_eq!(item.last_read_seq, 2, "{item:?}");
    assert_eq!(item.last_seq, 4, "{item:?}");
}

/// Bites: access is the project's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_project_chat_follows_the_projects_rights() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let a = seed_ws(&env, &format!("pra{tag}"));
    let b = seed_ws(&env, &format!("prb{tag}"));
    let outsider = seed_ws(&env, &format!("pro{tag}"));
    let gid = project(port, &format!("Rights {tag}"), &[&a, &b]).await;
    let addr = format!("project:{gid}");
    owner_ok(port, "/cli/project-group/msg", json!({ "group": gid, "body": "hello" })).await;

    // A member agent reads; an outsider is not a member, on every door.
    assert_eq!(thread(port, &passport(&b), &addr, "").await.status, 200);
    let r = thread(port, &passport(&outsider), &addr, "").await;
    assert_eq!((r.status, code_of(&r).as_str()), (403, "not_a_member"), "{}", r.body);
    let r = post(port, "/cli/thread/post", &passport(&outsider), json!({ "addr": addr, "text": "let me in" })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (403, "not_a_member"), "{}", r.body);
    assert_eq!(messages(port, &gid, "").await["messages"].as_array().unwrap().len(), 1, "nothing stored");

    // An App pass: thread:read in one member room is not enough (no oracle).
    let one = pass_in(port, &[&a], &format!("one{tag}"), &["thread:read", "project:write"]).await;
    let r = thread(port, &one, &addr, "").await;
    assert_eq!((r.status, code_of(&r).as_str()), (403, "skin_room"), "{}", r.body);
    let r = thread(port, &one, "project:no-such-project", "").await;
    assert_eq!((r.status, code_of(&r).as_str()), (403, "skin_room"), "an unknown project answers the same: {}", r.body);
    // In every member room: reads; posting needs project:write too.
    let reader = pass_in(port, &[&a, &b], &format!("rd{tag}"), &["thread:read", "thread:post"]).await;
    assert_eq!(thread(port, &reader, &addr, "").await.status, 200);
    let r = post(port, "/cli/thread/post", &reader, json!({ "addr": addr, "text": "hi" })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let writer = pass_in(port, &[&a, &b], &format!("wr{tag}"), &["thread:read", "project:write"]).await;
    let r = post(port, "/cli/thread/post", &writer, json!({ "addr": addr, "text": "from the app" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["author"]["kind"], "guest", "{}", r.body);
    let v = messages(port, &gid, "").await;
    assert_eq!(v["messages"][1]["author"], format!("guest:uwr{tag}"), "{v}");
    assert_eq!(v["messages"][1]["authorKind"], "guest", "{v}");
}

/// Bites: delete and files.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_lines_delete_and_carry_files() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let poc = seed_ws(&env, &format!("pfp{tag}"));
    let member = seed_ws(&env, &format!("pfm{tag}"));
    write(&poc.path, "docs/a.md", b"# A\n");
    write(&member.path, "notes.md", b"mine\n");
    let name = format!("Files {tag}");
    let gid = project(port, &name, &[&poc, &member]).await;
    let addr = format!("project:{gid}");

    // Delete: the owner's own line becomes a tombstone.
    let m = owner_ok(port, "/cli/project-group/msg", json!({ "group": gid, "body": "oops a secret" })).await;
    owner_ok(port, "/cli/project-group/msg", json!({ "group": gid, "body": "fine" })).await;
    let r = post(port, "/cli/thread/message/delete", OWNER, json!({ "addr": addr, "id": m["id"] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = messages(port, &gid, "").await;
    assert_eq!(v["messages"].as_array().unwrap().len(), 1, "{v}");
    assert!(!v.to_string().contains("oops a secret"), "{v}");
    let v = messages(port, &gid, "after_seq=0").await;
    assert_eq!(v["removed"], json!([{ "id": m["id"], "seq": 1 }]), "{v}");
    let v = messages(port, &gid, "include_deleted=1").await;
    assert_eq!(v["messages"][0]["deleted"], true, "{v}");
    let note = k2_daemon::project_group_routes::last_poc_line(&format!("note:{gid}")).expect("the PoC heard the delete");
    assert!(note.contains("deleted a message") && !note.contains("oops"), "{note}");
    // An agent never deletes.
    let r = post(port, "/cli/thread/message/delete", &passport(&member), json!({ "addr": addr, "seq": 2 })).await;
    assert_eq!(r.status, 403, "{}", r.body);

    // A person's file, named in the point of contact's workspace.
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "see", "attachments": [{ "path": "docs/a.md" }] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let p = r.json();
    assert_eq!(p["attachments"][0]["path"], "docs/a.md", "{p}");
    assert_eq!(p["attachments"][0]["workspace"], poc.id.as_str(), "{p}");
    let line = k2_daemon::project_group_routes::last_poc_line(p["id"].as_str().unwrap()).expect("line");
    assert!(line.contains(&format!("Attached: {}", poc.path.join("docs/a.md").display())), "{line}");
    // Outside it: refused, nothing stored.
    let outside = member.path.join("notes.md").to_string_lossy().to_string();
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "theirs", "attachments": [{ "path": outside }] })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (400, "attachment_outside_workspace"), "{}", r.body);

    // A member agent's own file is copied into the PoC's project folder.
    let r = post(port, "/cli/project-group/msg", &passport(&member), json!({ "group": gid, "body": "my notes", "attachments": [{ "path": "notes.md" }] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let rel = format!(".k2/downloads/projects/{gid}/notes.md");
    assert_eq!(v["attachments"][0]["path"], rel.as_str(), "{v}");
    assert_eq!(std::fs::read(poc.path.join(&rel)).expect("the copy"), b"mine\n");
    // Read back through the old route with its live state.
    let v = messages(port, &gid, "").await;
    let last = v["messages"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["attachments"][0]["state"], "ok", "{last}");
    // The Thread door serves it.
    let r = get(port, "/cli/thread/attachment", OWNER, &format!("addr={}&id={}&n=0&mode=meta", enc(&addr), last["id"].as_str().unwrap())).await;
    assert_eq!(r.status, 200, "{}", r.body);
}
