//! `WS /cli/thread/stream` (app-room-stream, K2 0.46.3), headless: the real
//! dispatcher in-process on a temp `$HOME`, over loopback.
//!
//! - One socket streams every readable room's pinned-Chat Thread, in the
//!   inbox stream format, in bus order with a rising per-socket `rev` (a
//!   login's `rev` is the server's, with `instanceId`).
//! - Isolation (R3): a pass for rooms A and B never receives a frame, a
//!   name or an id from room C, including after C is granted to another
//!   guest; a requested C is `unknown`, exactly like an id that doesn't
//!   exist. Attachments outside the guest's folders stay hidden.
//! - Grants and revokes apply live: a granted room joins the set; a
//!   revoked room's very next frame is never sent; a dead pass closes 4003.
//! - Quotas on an App's link: 1 for the socket plus 1 per room; a set over
//!   the App's subscriptions is cut with `inbox_error`; all of it is
//!   released when the socket closes.
//! - Reconnect: per-room `seq` + `POST /cli/thread/catchup` fill the gap.
//! - The per-room overlay socket is unchanged (no `kind`, no guest `rev`).
//! - Refusals: no `rooms=`, an agent passport, a pass without
//!   `thread:read`, a revoked pass; on the link, GA1 and no cookies.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use app_fixture::*;
use futures_util::{SinkExt, StreamExt};
use k2_core::db::schema::WorkspaceSession;
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Sock = WebSocketStream<MaybeTlsStream<TcpStream>>;

const PW: &str = "s3cret-horse-staple";
const FRAME_WAIT: Duration = Duration::from_secs(10);

fn write(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
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

/// A role holding `thread:read` in each of `rooms` (optionally with
/// files:read on `docs/` only in the first).
async fn role(port: u16, name: &str, rooms: &[&Ws], files_in_first: bool) {
    owner_ok(port, "/cli/skin/roles", json!({ "name": name })).await;
    for (i, ws) in rooms.iter().enumerate() {
        let caps = if i == 0 && files_in_first { json!(["thread:read", "files:read"]) } else { json!(["thread:read"]) };
        owner_ok(port, "/cli/skin/roles/room", json!({ "name": name, "handle": ws.handle, "caps": caps })).await;
        if i == 0 && files_in_first {
            owner_ok(port, "/cli/skin/roles/paths", json!({ "name": name, "handle": ws.handle, "read": ["docs/"] })).await;
        }
    }
}

async fn guest_login(port: u16, username: &str, role: &str) -> String {
    owner_ok(port, "/cli/skin/users", json!({ "username": username })).await;
    owner_ok(port, "/cli/skin/users/password", json!({ "username": username, "password": PW })).await;
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "guest login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

async fn post_text(port: u16, ws: &Ws, text: &str, files: Option<Value>) -> String {
    let mut body = json!({ "addr": ws.handle, "text": text });
    if let Some(f) = files {
        body["attachments"] = f;
    }
    let r = post(port, "/cli/thread/post", OWNER, body).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["id"].as_str().expect("id").to_string()
}

async fn open(port: u16, query: &str, token: &str) -> Sock {
    let url = format!("ws://127.0.0.1:{port}/cli/thread/stream?{query}&token={}", enc(token));
    let (sock, _) = tokio::time::timeout(FRAME_WAIT, tokio_tungstenite::connect_async(url.clone()))
        .await
        .unwrap_or_else(|_| panic!("connect {url} in time"))
        .unwrap_or_else(|e| panic!("connect {url}: {e}"));
    sock
}

/// The HTTP status a refused upgrade answered.
async fn refused(port: u16, path_query: &str) -> u16 {
    let url = format!("ws://127.0.0.1:{port}{path_query}");
    match tokio_tungstenite::connect_async(url.clone()).await {
        Ok(_) => panic!("{url} must be refused"),
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => resp.status().as_u16(),
        Err(e) => panic!("{url}: expected an HTTP refusal, got {e}"),
    }
}

/// The next JSON text frame (pings skipped). A close or a timeout fails.
async fn next(sock: &mut Sock) -> Value {
    loop {
        let m = tokio::time::timeout(FRAME_WAIT, sock.next())
            .await
            .expect("a frame in time")
            .expect("socket open")
            .expect("frame ok");
        match m {
            Message::Text(t) => return serde_json::from_str(&t).unwrap_or_else(|e| panic!("JSON frame ({e}): {t}")),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

async fn next_kind(sock: &mut Sock, kind: &str) -> Value {
    loop {
        let f = next(sock).await;
        if f["kind"] == kind {
            return f;
        }
    }
}

/// Every frame up to and including the `thread_message` whose doc id is
/// `marker`. Frames published after the marker can't be in it, so a frame
/// that was going to be sent before it is in the list.
async fn until_marker(sock: &mut Sock, marker: &str) -> Vec<Value> {
    let mut out = Vec::new();
    loop {
        let f = next(sock).await;
        let done = f["kind"] == "thread_message" && f["id"] == marker;
        out.push(f);
        if done {
            return out;
        }
    }
}

fn messages(frames: &[Value]) -> Vec<&Value> {
    frames.iter().filter(|f| f["kind"] == "thread_message").collect()
}

/// Wait (bounded) for an `inbox_subscribed` whose rooms satisfy `want`.
async fn subscribed_where(sock: &mut Sock, what: &str, want: impl Fn(&[String]) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        assert!(Instant::now() < deadline, "no inbox_subscribed with {what} in time");
        let f = next_kind(sock, "inbox_subscribed").await;
        let rooms: Vec<String> = f["rooms"].as_array().expect("rooms").iter().map(|r| r.as_str().unwrap().to_string()).collect();
        if want(&rooms) {
            return f;
        }
    }
}

struct World {
    a: Ws,
    a_conv: String,
    b: Ws,
    b_conv: String,
    c: Ws,
    c_conv: String,
}

fn world(env: &Env, tag: &str) -> World {
    let a = seed_ws(env, &format!("sa{tag}"));
    let a_conv = pin_chat(&a);
    let b = seed_ws(env, &format!("sb{tag}"));
    let b_conv = pin_chat(&b);
    let c = seed_ws(env, &format!("sc{tag}"));
    let c_conv = pin_chat(&c);
    World { a, a_conv, b, b_conv, c, c_conv }
}

fn tag() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_socket_streams_every_readable_room_in_order_with_a_rising_rev() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let w = world(&env, &t);
    write(&w.a.path, "docs/a.md", b"# A\n");
    write(&w.a.path, "private/c.txt", b"not for guests");
    let boot = get(port, "/boot-status", OWNER, "").await.json();
    assert!(boot["features"].as_array().expect("features").iter().any(|f| f == "thread-stream-v1"), "{boot}");

    let r = format!("rr{t}");
    role(port, &r, &[&w.a, &w.b], true).await;
    let guest = guest_login(port, &format!("sg{t}"), &r).await;
    let mut s = open(port, "rooms=all", &guest).await;
    let sub = next(&mut s).await;
    assert_eq!(sub["kind"], "inbox_subscribed", "the first frame names the set: {sub}");
    let mut want = vec![w.a.id.clone(), w.b.id.clone()];
    want.sort();
    assert_eq!(sub["rooms"], json!(want), "{sub}");
    assert_eq!(sub["resolved"], json!({ w.a.id.clone(): w.a_conv.clone(), w.b.id.clone(): w.b_conv.clone() }), "{sub}");
    assert_eq!((sub["all"].clone(), sub["revScope"].clone(), sub["max"].clone()), (json!(true), json!("socket"), json!(200)), "{sub}");
    assert!(sub.get("instanceId").is_none(), "C4: no instanceId for a guest: {sub}");
    assert!(!sub.to_string().contains(&w.c.id) && !sub.to_string().contains(&w.c_conv), "R3: {sub}");

    // Interleaved posts in both rooms arrive in post order on the one socket.
    let plan: Vec<(&Ws, &str)> = vec![(&w.a, "a1"), (&w.b, "b1"), (&w.a, "a2"), (&w.b, "b2"), (&w.a, "a3")];
    let mut ids = Vec::new();
    for (ws, text) in &plan {
        ids.push(post_text(port, ws, text, None).await);
    }
    let files = post_text(port, &w.a, "files", Some(json!([{ "path": "docs/a.md" }, { "path": "private/c.txt" }]))).await;
    let frames = until_marker(&mut s, &files).await;
    let got = messages(&frames);
    assert_eq!(got.len(), plan.len() + 1, "one frame per post: {frames:?}");
    let mut last_rev = sub["rev"].as_u64().expect("rev");
    let mut last_seq: std::collections::HashMap<String, i64> = Default::default();
    for (i, f) in got.iter().enumerate() {
        let rev = f["rev"].as_u64().unwrap_or_else(|| panic!("rev on {f}"));
        assert!(rev > last_rev, "rev rises: {last_rev} then {f}");
        last_rev = rev;
        let room = f["room"].as_str().expect("room").to_string();
        let seq = f["seq"].as_i64().expect("seq");
        assert!(seq > *last_seq.get(&room).unwrap_or(&0), "seq rises per room: {f}");
        last_seq.insert(room.clone(), seq);
        assert_eq!(f["collection"], "thread");
        let conv = if room == w.a.id { &w.a_conv } else { &w.b_conv };
        assert_eq!((&f["conversation_id"], &f["conversation"]), (&json!(conv), &json!(conv)), "{f}");
        if i < plan.len() {
            assert_eq!(f["id"], ids[i].as_str(), "post order: {frames:?}");
            assert_eq!(f["doc"]["body"], plan[i].1, "{f}");
            assert_eq!(room, plan[i].0.id, "{f}");
        }
    }
    assert_eq!(last_seq.len(), 2, "both rooms streamed: {frames:?}");
    // The doc is the guest's view: the file outside its folders is hidden.
    let last = got.last().unwrap();
    let body = last.to_string();
    assert!(!body.contains("private/") && !body.contains("c.txt"), "a hidden file leaked on the stream: {body}");
    assert_eq!(last["doc"]["attachments"][0]["path"], "docs/a.md", "{last}");
    assert_eq!(last["doc"]["attachments"][1], json!({ "kind": "file", "state": "hidden" }), "{last}");

    // A login streams a list of rooms with the server's rev and instanceId.
    let mut o = open(port, &format!("rooms={},{}", w.a.id, w.b_conv), OWNER).await;
    let osub = next_kind(&mut o, "inbox_subscribed").await;
    assert_eq!((osub["revScope"].clone(), osub["all"].clone()), (json!("server"), json!(false)), "{osub}");
    assert_eq!(osub["instanceId"], boot["instanceId"], "{osub}");
    assert_eq!(osub["rooms"].as_array().map(Vec::len), Some(2), "by room id or by conversation: {osub}");
    let id = post_text(port, &w.b, "owner sees b", None).await;
    let f = next_kind(&mut o, "thread_message").await;
    assert_eq!((f["id"].as_str(), f["room"].as_str()), (Some(id.as_str()), Some(w.b.id.as_str())), "{f}");
    assert!(f["rev"].as_u64().expect("rev") > osub["rev"].as_u64().expect("rev"), "{f}");

    // Replacing the set by message: off, then just A.
    o.send(Message::Text(r#"{"t":"inbox","off":true}"#.into())).await.unwrap();
    let off = next_kind(&mut o, "inbox_subscribed").await;
    assert_eq!(off["rooms"], json!([]), "{off}");
    o.send(Message::Text(json!({ "t": "inbox", "rooms": [w.a.id] }).to_string())).await.unwrap();
    let only_a = next_kind(&mut o, "inbox_subscribed").await;
    assert_eq!(only_a["rooms"], json!([w.a.id]), "{only_a}");
    o.send(Message::Text(r#"{"t":"inbox","rooms":"nope"}"#.into())).await.unwrap();
    let e = next_kind(&mut o, "inbox_error").await;
    assert_eq!(e["code"], "usage", "{e}");
    drop(env);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pass_for_rooms_a_and_b_never_sees_room_c() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let w = world(&env, &t);
    let r1 = format!("ab{t}");
    role(port, &r1, &[&w.a, &w.b], false).await;
    let g1 = guest_login(port, &format!("g1{t}"), &r1).await;

    let mut all = open(port, "rooms=all", &g1).await;
    let sub = next_kind(&mut all, "inbox_subscribed").await;
    assert_eq!(sub["rooms"].as_array().map(Vec::len), Some(2), "{sub}");

    // Asking for C by id or by conversation: `unknown`, the same answer as
    // an id that doesn't exist (no oracle); nothing about C comes back.
    let ghost = uuid::Uuid::new_v4().to_string();
    let mut named = open(port, &format!("rooms={},{},{},{}", w.a.id, w.c.id, w.c_conv, ghost), &g1).await;
    let nsub = next_kind(&mut named, "inbox_subscribed").await;
    assert_eq!(nsub["rooms"], json!([w.a.id]), "{nsub}");
    assert_eq!(nsub["unknown"], json!([w.c.id, w.c_conv, ghost]), "C answers exactly like a room that doesn't exist: {nsub}");
    assert!(nsub["resolved"].get(&w.c.id).is_none(), "{nsub}");

    // C gets traffic, then C is granted to someone else, then more traffic.
    let c1 = post_text(port, &w.c, &format!("secret-c-one-{t}"), None).await;
    let r2 = format!("cc{t}");
    role(port, &r2, &[&w.c], false).await;
    let g2 = guest_login(port, &format!("g2{t}"), &r2).await;
    let mut other = open(port, "rooms=all", &g2).await;
    let osub = next_kind(&mut other, "inbox_subscribed").await;
    assert_eq!(osub["rooms"], json!([w.c.id]), "{osub}");
    let c2 = post_text(port, &w.c, &format!("secret-c-two-{t}"), None).await;
    let f = next_kind(&mut other, "thread_message").await;
    assert_eq!(f["id"], c2.as_str(), "C's own guest does get C (the traffic is real): {f}");
    let c3 = post_text(port, &w.c, &format!("secret-c-three-{t}"), None).await;
    // Markers in A and B: every frame published before them has arrived.
    let mb = post_text(port, &w.b, "marker-b", None).await;
    let ma = post_text(port, &w.a, "marker-a", None).await;

    let seen_all = until_marker(&mut all, &ma).await;
    assert!(messages(&seen_all).iter().any(|f| f["id"] == mb.as_str()), "B's marker came first: {seen_all:?}");
    // The named socket holds A only: it sees A's marker, never B's.
    let seen_named = until_marker(&mut named, &ma).await;
    assert!(!messages(&seen_named).iter().any(|f| f["id"] == mb.as_str()), "{seen_named:?}");
    for (which, frames) in [("all", &seen_all), ("named", &seen_named)] {
        let text = serde_json::to_string(frames).unwrap();
        for leak in [w.c.id.as_str(), w.c_conv.as_str(), w.c.handle.as_str(), c1.as_str(), c2.as_str(), c3.as_str(), "secret-c-"] {
            assert!(!text.contains(leak), "{which}: room C leaked ({leak}): {text}");
        }
        for f in frames.iter() {
            assert!(f["room"].is_null() || f["room"] == w.a.id.as_str() || f["room"] == w.b.id.as_str(), "{which}: {f}");
        }
    }
    drop(env);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grants_and_revokes_apply_live() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let w = world(&env, &t);
    let r = format!("gr{t}");
    role(port, &r, &[&w.a], false).await;
    let username = format!("gl{t}");
    let guest = guest_login(port, &username, &r).await;
    let mut s = open(port, "rooms=all", &guest).await;
    let sub = next_kind(&mut s, "inbox_subscribed").await;
    assert_eq!(sub["rooms"], json!([w.a.id]), "{sub}");

    // Granted while open: B joins the set, its frames flow.
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": r, "handle": w.b.handle, "caps": ["thread:read"] })).await;
    let b_id = w.b.id.clone();
    let joined = subscribed_where(&mut s, "B", |rooms| rooms.contains(&b_id)).await;
    assert_eq!(joined["resolved"][&w.b.id], w.b_conv.as_str(), "{joined}");
    let b1 = post_text(port, &w.b, "after grant", None).await;
    let f = next_kind(&mut s, "thread_message").await;
    assert_eq!((f["id"].as_str(), f["room"].as_str()), (Some(b1.as_str()), Some(w.b.id.as_str())), "{f}");

    // Revoked: the very next B frame is never sent (checked per frame, not
    // at the next tick), and the set drops B.
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": r, "handle": w.b.handle, "clear": true })).await;
    let b2 = post_text(port, &w.b, "after revoke", None).await;
    let ma = post_text(port, &w.a, "marker", None).await;
    let frames = until_marker(&mut s, &ma).await;
    assert!(!messages(&frames).iter().any(|f| f["id"] == b2.as_str() || f["room"] == w.b.id.as_str()), "a revoked room's frame went out: {frames:?}");
    let left = match frames.iter().rev().find(|f| f["kind"] == "inbox_subscribed") {
        Some(f) => f.clone(),
        None => subscribed_where(&mut s, "B gone", |rooms| !rooms.iter().any(|x| x == &w.b.id)).await,
    };
    assert_eq!(left["rooms"], json!([w.a.id]), "{left}");

    // A cap removed from the room (the room stays, thread:read goes).
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": r, "handle": w.a.handle, "caps": ["files:read"] })).await;
    let a2 = post_text(port, &w.a, "no longer readable", None).await;
    let gone = subscribed_where(&mut s, "no rooms", |rooms| rooms.is_empty()).await;
    assert_eq!(gone["rooms"], json!([]), "{gone}");
    let leftovers: Vec<Value> = {
        let mut v = Vec::new();
        while let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(1500), s.next()).await {
            if let Message::Text(t) = m {
                v.push(serde_json::from_str::<Value>(&t).unwrap());
            }
        }
        v
    };
    assert!(!leftovers.iter().any(|f| f["id"] == a2.as_str()), "{leftovers:?}");

    // The pass dies (the guest is removed): the socket closes 4003.
    k2_core::skin::remove_principal(&username).expect("remove guest");
    let deadline = Instant::now() + Duration::from_secs(8);
    let code = loop {
        assert!(Instant::now() < deadline, "the stream stayed open on a dead pass");
        match tokio::time::timeout(Duration::from_secs(8), s.next()).await {
            Ok(Some(Ok(Message::Close(Some(cf))))) => break u16::from(cf.code),
            Ok(Some(Ok(_))) => continue,
            other => panic!("expected a close frame, got {other:?}"),
        }
    };
    assert_eq!(code, 4003, "revoked");
    drop(env);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnect_fills_the_gap_with_the_catch_up() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let w = world(&env, &t);
    let r = format!("rc{t}");
    role(port, &r, &[&w.a, &w.b], false).await;
    let guest = guest_login(port, &format!("rg{t}"), &r).await;

    let mut s = open(port, "rooms=all", &guest).await;
    next_kind(&mut s, "inbox_subscribed").await;
    post_text(port, &w.a, "one", None).await;
    let f = next_kind(&mut s, "thread_message").await;
    let seen_a = f["seq"].as_i64().expect("seq");
    s.close(None).await.unwrap();
    drop(s);

    // Missed while away.
    let two = post_text(port, &w.a, "two", None).await;
    let three = post_text(port, &w.b, "three", None).await;

    // Reconnect: subscribe first, then catch up per room from its seq.
    let mut s = open(port, "rooms=all", &guest).await;
    let sub = next_kind(&mut s, "inbox_subscribed").await;
    assert_eq!(sub["rev"], 0, "a guest's rev restarts with its socket: {sub}");
    let r = post(
        port,
        "/cli/thread/catchup",
        &guest,
        json!({ "threads": [
            { "addr": sub["resolved"][&w.a.id], "afterSeq": seen_a },
            { "addr": sub["resolved"][&w.b.id], "afterSeq": 0 },
        ] }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let ids = |e: &Value| e["items"].as_array().expect("items").iter().map(|i| i["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(ids(&v["threads"][0]), vec![two.clone()], "{v}");
    assert_eq!(ids(&v["threads"][1]), vec![three.clone()], "{v}");
    let caught = v["threads"][0]["items"][0]["seq"].as_i64().unwrap();
    // And the live stream carries on after it.
    let four = post_text(port, &w.a, "four", None).await;
    let f = next_kind(&mut s, "thread_message").await;
    assert_eq!(f["id"], four.as_str(), "{f}");
    assert_eq!(f["seq"].as_i64(), Some(caught + 1), "no gap between the catch-up and the stream: {f}");
    drop(env);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_per_room_socket_is_unchanged_and_refusals_are_loud() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let w = world(&env, &t);
    let r = format!("pr{t}");
    role(port, &r, &[&w.a], false).await;
    let guest = guest_login(port, &format!("pg{t}"), &r).await;

    // The per-room overlay socket: same frames as before, side by side.
    let url = format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={}&token={}", w.a_conv, enc(&guest));
    let (mut room, _) = tokio_tungstenite::connect_async(url).await.expect("per-room socket");
    let mut stream = open(port, "rooms=all", &guest).await;
    next_kind(&mut stream, "inbox_subscribed").await;
    let id = post_text(port, &w.a, "both", None).await;
    let per_room = next(&mut room).await;
    assert_eq!(per_room["collection"], "thread", "{per_room}");
    assert_eq!(per_room["id"], id.as_str());
    assert!(per_room.get("kind").is_none() && per_room.get("rev").is_none() && per_room.get("room").is_none(), "unchanged wire: {per_room}");
    let streamed = next_kind(&mut stream, "thread_message").await;
    assert_eq!(streamed["doc"], per_room["doc"], "the same doc on both sockets");
    assert_eq!(streamed["seq"], per_room["seq"]);
    assert_eq!(
        refused(port, &format!("/cli/overlay/events?conversation={}&token={}", w.b_conv, enc(&guest))).await,
        403,
        "the per-room socket still refuses another room"
    );

    // Refusals on the new socket.
    assert_eq!(refused(port, &format!("/cli/thread/stream?token={}", enc(&guest))).await, 400, "no rooms=");
    assert_eq!(refused(port, &format!("/cli/thread/stream?rooms=&token={}", enc(&guest))).await, 400, "empty rooms=");
    assert_eq!(refused(port, "/cli/thread/stream?rooms=all").await, 403, "no credential");
    assert_eq!(refused(port, &format!("/cli/thread/stream?rooms=all&token={}", enc(&passport(&w.a)))).await, 403, "an agent passport");
    let (_, no_read) = k2_core::skin::create_token(&format!("nr{t}"), Some(&["files:read".to_string()]), &[w.a.id.clone()]).unwrap();
    assert_eq!(refused(port, &format!("/cli/thread/stream?rooms=all&token={}", enc(&no_read))).await, 403, "a pass without thread:read");
    let dead = "k2skn_00000000000000000000000000000000000000000";
    assert_eq!(refused(port, &format!("/cli/thread/stream?rooms=all&token={dead}")).await, 401, "a revoked pass");
    // A platform token with thread:read streams its rooms too.
    let (_, platform) = k2_core::skin::create_token(&format!("pt{t}"), Some(&["thread:read".to_string()]), &[w.b.id.clone()]).unwrap();
    let mut p = open(port, "rooms=all", &platform).await;
    let psub = next_kind(&mut p, "inbox_subscribed").await;
    assert_eq!(psub["rooms"], json!([w.b.id]), "{psub}");
    drop(env);
}

/// A guarded App row (as the engine writes it) with a quota override.
fn app_in(ws: &Ws, app_id: &str, quota: Value) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let dir = ws.path.join(app_id);
    std::fs::create_dir_all(&dir).unwrap();
    let svc = k2_core::published_services::insert(&conn, &ws.id, app_id, "node server.mjs", &dir.to_string_lossy(), 47_100, "local", "stopped", "cmd", "")
        .expect("insert service")
        .id;
    k2_core::deploy::store::insert_app(
        &conn,
        &k2_core::deploy::store::NewApp {
            app_id,
            service_id: &svc,
            seed_id: None,
            owner_workspace: &ws.id,
            isolation: "container",
            memory_limit_bytes: None,
            front_port: 0,
            created_by: "owner",
        },
    )
    .expect("insert app");
    conn.execute("UPDATE apps SET quota_json = ?2 WHERE app_id = ?1", rusqlite::params![app_id, quota.to_string()]).unwrap();
    svc
}

fn on_link(port: u16, method: &str, pq: &str, pass: &str, extra: &str, body: Option<&str>) -> Resp {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect link");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {pass}\r\n{extra}Connection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {pass}\r\n{extra}Connection: close\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("no status: {text:?}"));
    Resp { status, body: text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default() }
}

async fn live_subscriptions(port: u16, app: &str) -> u64 {
    let r = get(port, "/cli/app/quota", OWNER, &format!("app={app}")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["live"]["subscriptions"].as_u64().expect("live subscriptions")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn on_an_apps_link_the_socket_counts_one_plus_its_rooms() {
    let env = setup(&[]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let w = world(&env, &t);
    let app = format!("str{t}");
    let svc = app_in(&w.a, &app, json!({ "subscriptions": 3 }));
    let lina = format!("lina{t}");
    k2_core::skin::add_principal_for_app(&lina, None, &svc, None, "owner").expect("an App guest");
    k2_core::skin::set_app_grant_rooms(&lina, &svc, &[w.a.id.clone(), w.b.id.clone(), w.c.id.clone()], false, "owner").expect("rooms");
    k2_core::skin::set_principal_password(&lina, Some(PW)).unwrap();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_daemon::apps::grants::record(&conn, &app, &["thread:read".to_string()]).expect("approved set");
    }
    let pass = gateway::open(
        LinkSpec { app_id: app.clone(), service_id: svc.clone(), workspace_id: w.a.id.clone(), port: 0, caps: vec!["thread:read".into()], rooms: vec![w.a.id.clone()] },
        Some(Arc::new(|_: &str| Ok(()))),
    )
    .expect("open link");
    let lport = gateway::port_of(&app).expect("listening");
    let (p, s2) = (pass.clone(), svc.clone());
    let lina2 = lina.clone();
    let login = tokio::task::spawn_blocking(move || {
        on_link(lport, "POST", "/cli/skin/login", &p, "", Some(&json!({ "username": lina2, "password": PW, "app": s2 }).to_string()))
    })
    .await
    .unwrap();
    assert_eq!(login.status, 200, "{}", login.body);
    let session = login.json()["token"].as_str().expect("token").to_string();

    // GA1 and no cookies on the link, before any handler.
    let (p, sess) = (pass.clone(), session.clone());
    let up = "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n";
    let (no_param, cookie) = tokio::task::spawn_blocking(move || {
        (on_link(lport, "GET", "/cli/thread/stream", &p, up, None), on_link(lport, "GET", "/cli/thread/stream?rooms=all", &sess, &format!("{up}Cookie: k2skn=x\r\n"), None))
    })
    .await
    .unwrap();
    assert_eq!((no_param.status, no_param.code()), (404, "not_on_link".to_string()), "GA1: {}", no_param.body);
    assert_eq!((cookie.status, cookie.code()), (400, "cookies_not_allowed".to_string()), "{}", cookie.body);

    assert_eq!(live_subscriptions(port, &app).await, 0);
    // The guest's three rooms: the socket (1) + 2 rooms fit a quota of 3.
    let mut s = open(lport, "rooms=all", &session).await;
    let e = next(&mut s).await;
    assert_eq!((e["kind"].as_str(), e["code"].as_str(), e["limit"].as_u64()), (Some("inbox_error"), Some("too_many_subscriptions"), Some(3)), "{e}");
    let sub = next_kind(&mut s, "inbox_subscribed").await;
    let mut all = vec![w.a.id.clone(), w.b.id.clone(), w.c.id.clone()];
    all.sort();
    assert_eq!(sub["rooms"], json!(all[..2]), "the rooms that fit, in room order: {sub}");
    assert_eq!(live_subscriptions(port, &app).await, 3, "1 for the socket + 1 per room");
    // The cut set still streams.
    let first_room = all[0].clone();
    let ws_of = |id: &str| -> &Ws { [&w.a, &w.b, &w.c].into_iter().find(|x| x.id == id).unwrap() };
    let id = post_text(port, ws_of(&first_room), "on the link", None).await;
    let f = next_kind(&mut s, "thread_message").await;
    assert_eq!((f["id"].as_str(), f["room"].as_str()), (Some(id.as_str()), Some(first_room.as_str())), "{f}");
    // Fewer rooms: the slots come back at once.
    s.send(Message::Text(json!({ "t": "inbox", "rooms": [first_room] }).to_string())).await.unwrap();
    let one = next_kind(&mut s, "inbox_subscribed").await;
    assert_eq!(one["rooms"].as_array().map(Vec::len), Some(1), "{one}");
    assert_eq!(live_subscriptions(port, &app).await, 2);
    // Closing releases the socket and its rooms.
    s.close(None).await.unwrap();
    drop(s);
    let deadline = Instant::now() + Duration::from_secs(8);
    while live_subscriptions(port, &app).await != 0 {
        assert!(Instant::now() < deadline, "the stream's subscriptions were never released");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    gateway::close(&app, None, true);
    drop(env);
}
