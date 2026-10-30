//! inbox-stream-v1 (0.46.7, K1) headless: the real dispatcher in-process on
//! a temp `$HOME`, over loopback. `contract-phone-inbox-stream-0462.md` §2.
//!
//! - `/boot-status` lists `inbox-stream-v1`.
//! - A login's `/cli/sessions/events` socket subscribes by conversation id
//!   and by address and gets `inbox_subscribed` (`resolved`, `unknown`,
//!   `rev`, `instanceId`).
//! - Scoping: only the set's Thread items arrive, as `thread_message`
//!   with the overlay doc and a rising server `rev`; another chat's post
//!   never does. A new subscribe replaces the set; `off` stops it.
//! - A post over 32 KiB arrives without its body (`truncated`,
//!   `bodyBytes`), still naming its item.
//! - Refusals: an App pass (`k2skn_`) and no token get 403 before the
//!   upgrade; App guests keep `/cli/thread/stream`.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::time::Duration;

use app_fixture::*;
use futures_util::{SinkExt, StreamExt};
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Sock = WebSocketStream<MaybeTlsStream<TcpStream>>;

const FRAME_WAIT: Duration = Duration::from_secs(10);
const PW: &str = "s3cret-horse-staple";

fn pin_chat(ws: &Ws) -> String {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    conv
}

async fn post_text(port: u16, ws: &Ws, text: &str) -> String {
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": ws.handle, "text": text })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["id"].as_str().expect("id").to_string()
}

async fn open(port: u16, token: &str) -> Sock {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?path=&token={}", enc(token));
    let (sock, _) = tokio::time::timeout(FRAME_WAIT, tokio_tungstenite::connect_async(url.clone()))
        .await
        .unwrap_or_else(|_| panic!("connect {url} in time"))
        .unwrap_or_else(|e| panic!("connect {url}: {e}"));
    sock
}

async fn refused(port: u16, query: &str) -> u16 {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?{query}");
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

/// Every inbox frame up to and including the `thread_message` for
/// `marker` (session events in between are skipped: they're the socket's
/// other traffic).
async fn until_marker(sock: &mut Sock, marker: &str) -> Vec<Value> {
    let inbox = ["inbox_subscribed", "inbox_error", "thread_message", "thread_moved", "thread_address", "inbox_reset"];
    let mut out = Vec::new();
    loop {
        let f = next(sock).await;
        let done = f["kind"] == "thread_message" && f["id"] == marker;
        if inbox.iter().any(|k| f["kind"] == *k) {
            out.push(f);
        }
        if done {
            return out;
        }
    }
}

async fn send(sock: &mut Sock, v: Value) {
    sock.send(Message::Text(v.to_string())).await.expect("send");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_login_streams_only_its_subscribed_chats() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
    let a = seed_ws(&env, &format!("ia{t}"));
    let a_conv = pin_chat(&a);
    let b = seed_ws(&env, &format!("ib{t}"));
    let b_conv = pin_chat(&b);
    let c = seed_ws(&env, &format!("ic{t}"));
    let c_conv = pin_chat(&c);

    let boot = get(port, "/boot-status", OWNER, "").await.json();
    let features = boot["features"].as_array().expect("features");
    for key in ["inbox-stream-v1", "push-content-available-v1", "push-e2e-v2"] {
        assert!(features.iter().any(|f| f == key), "{key}: {boot}");
    }

    let mut s = open(port, OWNER).await;
    assert_eq!(next(&mut s).await["kind"], "hello");
    // Bad input is answered, never ignored.
    send(&mut s, json!({ "t": "inbox", "addrs": "nope" })).await;
    let e = next_kind(&mut s, "inbox_error").await;
    assert_eq!(e["code"], "usage", "{e}");

    send(&mut s, json!({ "t": "inbox", "conversationIds": [a_conv], "addrs": [b.handle, format!("nope{t}")] })).await;
    let sub = next_kind(&mut s, "inbox_subscribed").await;
    let mut convs: Vec<String> = sub["conversations"].as_array().expect("conversations").iter().map(|c| c.as_str().unwrap().to_string()).collect();
    convs.sort();
    let mut want = vec![a_conv.clone(), b_conv.clone()];
    want.sort();
    assert_eq!(convs, want, "{sub}");
    assert_eq!(sub["resolved"], json!({ b.handle.clone(): b_conv.clone() }), "{sub}");
    assert_eq!(sub["unknown"], json!([format!("nope{t}")]), "{sub}");
    assert_eq!(sub["max"], 200);
    assert_eq!(sub["instanceId"].as_str().map(str::len), Some(16), "{sub}");
    let sub_rev = sub["rev"].as_u64().expect("rev");

    // Interleaved posts in a, c (not subscribed) and b.
    let a1 = post_text(port, &a, "a1").await;
    let c1 = post_text(port, &c, "c1 canary-not-mine").await;
    let b1 = post_text(port, &b, "b1").await;
    let marker = post_text(port, &a, "a2").await;
    let frames = until_marker(&mut s, &marker).await;
    let ids: Vec<&str> = frames.iter().map(|f| f["id"].as_str().unwrap_or("")).collect();
    assert_eq!(ids, vec![a1.as_str(), b1.as_str(), marker.as_str()], "only the set, in post order: {frames:?}");
    let text = serde_json::to_string(&frames).unwrap();
    assert!(!text.contains(&c1) && !text.contains(&c_conv) && !text.contains("canary-not-mine"), "another chat leaked: {text}");
    let mut last = sub_rev;
    for f in &frames {
        assert_eq!(f["kind"], "thread_message");
        assert_eq!(f["collection"], "thread");
        assert!(f["seq"].as_i64().expect("seq") > 0);
        let rev = f["rev"].as_u64().expect("rev");
        assert!(rev > last, "rev rises past the subscribe: {f}");
        last = rev;
        assert_eq!(f["doc"]["id"], f["id"], "the overlay doc: {f}");
    }
    assert_eq!(frames[0]["conversation_id"], a_conv.as_str());
    assert_eq!(frames[0]["doc"]["body"], "a1");
    assert_eq!(frames[1]["conversation_id"], b_conv.as_str());

    // Over 32 KiB: the body goes, the item is still named.
    let big = "x".repeat(40 * 1024);
    let big_id = post_text(port, &a, &big).await;
    let f = until_marker(&mut s, &big_id).await.pop().expect("frame");
    assert_eq!((f["truncated"].clone(), f["bodyBytes"].clone()), (json!(true), json!(40 * 1024)), "{}", &f.to_string()[..400.min(f.to_string().len())]);
    assert!(f["doc"].get("body").is_none());
    assert_eq!(f["conversation_id"], a_conv.as_str());

    // A new subscribe replaces the set: now c only.
    send(&mut s, json!({ "t": "inbox", "conversationIds": [c_conv] })).await;
    let sub = next_kind(&mut s, "inbox_subscribed").await;
    assert_eq!(sub["conversations"], json!([c_conv.clone()]), "{sub}");
    let a3 = post_text(port, &a, "a3").await;
    let c2 = post_text(port, &c, "c2").await;
    let frames = until_marker(&mut s, &c2).await;
    assert_eq!(frames.len(), 1, "a is no longer streamed: {frames:?}");
    assert!(!serde_json::to_string(&frames).unwrap().contains(&a3));

    // Off: an empty answer, and nothing after it.
    send(&mut s, json!({ "t": "inbox", "off": true })).await;
    let off = next_kind(&mut s, "inbox_subscribed").await;
    assert_eq!(off["conversations"], json!([]), "{off}");
    let c3 = post_text(port, &c, "c3").await;
    send(&mut s, json!({ "t": "inbox", "conversationIds": [b_conv] })).await;
    let again = loop {
        let f = next(&mut s).await;
        assert_ne!(f["kind"], "thread_message", "off sends nothing (c3 = {c3}): {f}");
        if f["kind"] == "inbox_subscribed" {
            break f;
        }
    };
    assert_eq!(again["conversations"], json!([b_conv.clone()]), "{again}");
    drop(env);
}

/// Only a login reaches the socket: an App pass and no token are refused
/// before the upgrade, so a guest never gets the stream (it keeps
/// `/cli/thread/stream`, scoped to its own rooms).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_pass_never_gets_the_inbox_stream() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
    let a = seed_ws(&env, &format!("ga{t}"));
    let _ = pin_chat(&a);
    let role = format!("ir{t}");
    for (path, body) in [
        ("/cli/skin/roles", json!({ "name": role })),
        ("/cli/skin/roles/room", json!({ "name": role, "handle": a.handle, "caps": ["thread:read"] })),
        ("/cli/skin/users", json!({ "username": format!("ig{t}") })),
    ] {
        let r = post(port, path, OWNER, body).await;
        assert_eq!(r.status, 200, "{path}: {}", r.body);
    }
    let user = format!("ig{t}");
    k2_daemon::test_harness::guest_with_own_password(&user, PW);
    let r = post(port, "/cli/skin/roles/assign", OWNER, json!({ "username": user, "role": role })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let body = json!({ "username": user, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "guest login: {}", r.body);
    let guest = r.json()["token"].as_str().expect("token").to_string();
    assert!(guest.starts_with("k2skn_"), "an App pass: {guest}");

    assert_eq!(refused(port, &format!("path=&token={}", enc(&guest))).await, 403, "an App pass");
    assert_eq!(refused(port, "path=").await, 403, "no token");
    drop(env);
}
