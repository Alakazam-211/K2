//! hub-viewing-v1 (0.46.8, `vslive-phone-speed-b1-b2-rev-v1.md` B1)
//! headless: the real dispatcher in-process on a temp `$HOME`, over
//! loopback, a login's `/cli/sessions/events` socket.
//!
//! - `/boot-status` lists `hub-viewing-v1`.
//! - `{"t":"viewing","conversationId","visible":true}` registers the view
//!   (the same registry Thread pushes ask: quiet while it lasts) and
//!   answers `viewing_ok` with `rev`, `convRev` (the chat's last published
//!   `rev`, 0 when none), `instanceId` and `turn` (what
//!   `GET /cli/thread/activity` returns). A repeat sends nothing; another
//!   chat replaces the view; `visible:false` and the socket closing clear
//!   it; the registry entry lapses after 90 s.
//! - While viewing, the chat's `thread_message` arrives even when it is
//!   not in the inbox set, once when it is, and never another chat's; its
//!   turn frames arrive as `thread_activity`, a lag as
//!   `thread_activity_reset` (never a close); a move moves the view.
//! - Two devices are two sockets: closing one leaves the other viewing.
//! - Refusals are answered (`viewing_error`), never a silent ack: a bad
//!   frame (`usage`), a Grok Bot Thread while Show is off (`forbidden`).
//!   An agent passport never reaches the hub (403 before the upgrade).
//! - A removal `activity_changed` whose row had no workspace carries
//!   `workspaces` (every rollup); one with a workspace doesn't.

use crate::support::app_fixture;

use std::time::{Duration, Instant};

use app_fixture::*;
use futures_util::{SinkExt, StreamExt};
use k2_core::db::schema::WorkspaceSession;
use k2_core::overlay::OverlayDoc;
use k2_daemon::overlay_ws::{self, OverlayFrame};
use k2_daemon::{activity_store, thread_activity, thread_viewers};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Sock = WebSocketStream<MaybeTlsStream<TcpStream>>;

const FRAME_WAIT: Duration = Duration::from_secs(10);

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

/// A Thread frame straight onto the overlay bus (as a post publishes it).
fn publish_thread(conv: &str, id: &str, body: &str) {
    overlay_ws::publish(OverlayFrame {
        rev: 0,
        collection: "thread".into(),
        seq: 1,
        id: id.into(),
        doc: Some(OverlayDoc::text(id.into(), "k2".into(), "owner".into(), body.into(), "thread")),
        activity: None,
        conversation_id: Some(conv.into()),
    });
}

/// A turn frame straight onto the activity bus (as the tracker publishes it).
fn publish_turn(conv: &str, turn_id: &str) {
    overlay_ws::publish_activity(OverlayFrame {
        rev: 0,
        collection: "activity".into(),
        seq: 0,
        id: turn_id.into(),
        doc: None,
        activity: Some(json!({ "turnId": turn_id, "state": "working" })),
        conversation_id: Some(conv.into()),
    });
}

async fn open(port: u16, token: &str) -> Sock {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?path=&token={}", enc(token));
    let (mut sock, _) = tokio::time::timeout(FRAME_WAIT, tokio_tungstenite::connect_async(url.clone()))
        .await
        .unwrap_or_else(|_| panic!("connect {url} in time"))
        .unwrap_or_else(|e| panic!("connect {url}: {e}"));
    assert_eq!(next(&mut sock).await["kind"], "hello");
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

/// The stream and viewing frames up to and including the
/// `thread_message` for `marker` (session events in between are skipped).
async fn until_marker(sock: &mut Sock, marker: &str) -> Vec<Value> {
    let kinds = [
        "inbox_subscribed", "inbox_error", "thread_message", "thread_moved", "thread_address", "inbox_reset",
        "viewing_ok", "viewing_error", "thread_activity", "thread_activity_reset",
    ];
    let mut out = Vec::new();
    loop {
        let f = next(sock).await;
        let done = f["kind"] == "thread_message" && f["id"] == marker;
        if kinds.iter().any(|k| f["kind"] == *k) {
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

async fn view(sock: &mut Sock, conv: &str, visible: bool) {
    send(sock, json!({ "t": "viewing", "conversationId": conv, "visible": visible })).await;
}

fn viewing(conv: &str) -> bool {
    thread_viewers::global().is_viewing("owner", conv, Instant::now())
}

async fn wait_viewing(conv: &str, want: bool, what: &str) {
    let deadline = Instant::now() + FRAME_WAIT;
    while viewing(conv) != want {
        assert!(Instant::now() < deadline, "{what}: viewing {conv} never became {want}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn tag() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_view_registers_answers_and_streams_its_chat_once() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let a = seed_ws(&env, &format!("va{t}"));
    let a_conv = pin_chat(&a);
    let b = seed_ws(&env, &format!("vb{t}"));
    let b_conv = pin_chat(&b);
    let c = seed_ws(&env, &format!("vc{t}"));
    let c_conv = pin_chat(&c);

    let boot = get(port, "/boot-status", OWNER, "").await.json();
    assert!(boot["features"].as_array().expect("features").iter().any(|f| f == "hub-viewing-v1"), "{boot}");

    let mut s = open(port, OWNER).await;

    // A post in a, seen through the inbox stream: its frame's `rev` is a's
    // last published rev.
    send(&mut s, json!({ "t": "inbox", "conversationIds": [a_conv] })).await;
    next_kind(&mut s, "inbox_subscribed").await;
    let a1 = post_text(port, &a, "a1").await;
    let f = until_marker(&mut s, &a1).await.pop().expect("a1");
    let a1_rev = f["rev"].as_u64().expect("rev");
    send(&mut s, json!({ "t": "inbox", "off": true })).await;
    next_kind(&mut s, "inbox_subscribed").await;
    // Another chat's post moves the global rev, not a's convRev.
    let c1 = post_text(port, &c, "c1").await;

    // The view: registered, then viewing_ok.
    view(&mut s, &a_conv, true).await;
    let ok = next_kind(&mut s, "viewing_ok").await;
    assert_eq!(ok["conversation_id"], a_conv.as_str(), "{ok}");
    assert_eq!(ok["convRev"].as_u64(), Some(a1_rev), "convRev is a's last rev: {ok}");
    let ok_rev = ok["rev"].as_u64().expect("rev");
    assert!(ok_rev > a1_rev, "rev is the global mark, past c1 ({c1}): {ok}");
    assert_eq!(ok["instanceId"].as_str(), Some(boot["instanceId"].as_str().expect("boot instanceId")), "{ok}");
    let act = get(port, "/cli/thread/activity", OWNER, &format!("addr={}", a.handle)).await;
    assert_eq!(act.status, 200, "{}", act.body);
    assert_eq!(ok["turn"], act.json()["turn"], "turn is the catch-up's: {ok}");
    assert!(ok.as_object().expect("object").contains_key("turn"), "{ok}");
    assert!(viewing(&a_conv), "the view is in the push registry before viewing_ok");
    assert!(
        !thread_viewers::global().is_viewing("owner", &a_conv, Instant::now() + thread_viewers::VIEW_TTL + Duration::from_secs(1)),
        "without a repeat the view lapses after 90 s"
    );

    // Another chat replaces the view; a chat never published has convRev 0.
    view(&mut s, &b_conv, true).await;
    let ok_b = next_kind(&mut s, "viewing_ok").await;
    assert_eq!((ok_b["conversation_id"].as_str(), ok_b["convRev"].as_u64()), (Some(b_conv.as_str()), Some(0)), "{ok_b}");
    assert!(ok_b["rev"].as_u64().expect("rev") >= ok_rev);
    assert!(viewing(&b_conv) && !viewing(&a_conv), "one view per socket");

    // A repeat sends nothing; b's post arrives (b is in no inbox set), a's
    // and c's never do.
    view(&mut s, &b_conv, true).await;
    let a2 = post_text(port, &a, "a2 canary-not-viewed").await;
    let c2 = post_text(port, &c, "c2 canary-not-viewed").await;
    let b1 = post_text(port, &b, "b1").await;
    let frames = until_marker(&mut s, &b1).await;
    assert_eq!(frames.len(), 1, "no second viewing_ok, no other chat: {frames:?}");
    let f = &frames[0];
    assert_eq!((f["kind"].as_str(), f["conversation_id"].as_str()), (Some("thread_message"), Some(b_conv.as_str())), "{f}");
    assert_eq!(f["doc"]["body"], "b1", "{f}");
    assert!(f["rev"].as_u64().expect("rev") > ok_b["rev"].as_u64().expect("rev"), "{f}");
    let text = serde_json::to_string(&frames).expect("json");
    assert!(!text.contains(&a2) && !text.contains(&c2) && !text.contains("canary"), "{text}");

    // Viewed AND subscribed: each item exactly once.
    send(&mut s, json!({ "t": "inbox", "conversationIds": [b_conv] })).await;
    next_kind(&mut s, "inbox_subscribed").await;
    let b2 = post_text(port, &b, "b2").await;
    let b3 = post_text(port, &b, "b3").await;
    let frames = until_marker(&mut s, &b3).await;
    let ids: Vec<&str> = frames.iter().map(|f| f["id"].as_str().unwrap_or("")).collect();
    assert_eq!(ids, vec![b2.as_str(), b3.as_str()], "once each: {frames:?}");
    // The inbox set off: the view alone still streams b.
    send(&mut s, json!({ "t": "inbox", "off": true })).await;
    next_kind(&mut s, "inbox_subscribed").await;
    let b4 = post_text(port, &b, "b4").await;
    let frames = until_marker(&mut s, &b4).await;
    assert_eq!(frames.len(), 1, "{frames:?}");

    // visible:false ends it: out of the registry, nothing more streamed.
    view(&mut s, &b_conv, false).await;
    wait_viewing(&b_conv, false, "visible:false").await;
    let b5 = post_text(port, &b, "b5 canary-after-false").await;
    send(&mut s, json!({ "t": "inbox", "conversationIds": [c_conv] })).await;
    loop {
        let f = next(&mut s).await;
        assert_ne!(f["kind"], "thread_message", "nothing after visible:false (b5 = {b5}): {f}");
        if f["kind"] == "inbox_subscribed" {
            break;
        }
    }
    drop(s);
    drop(env);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_view_carries_and_streams_its_turn_and_follows_a_move() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let a = seed_ws(&env, &format!("ta{t}"));
    let a_conv = pin_chat(&a);
    let o = seed_ws(&env, &format!("to{t}"));
    let o_conv = pin_chat(&o);

    // A live turn on a: a row with no lifecycle evidence opens at delivery.
    let sid = uuid::Uuid::new_v4().to_string();
    activity_store::register(activity_store::SessionFacts {
        session_id: sid.clone(),
        agent_name: format!("tab-{t}"),
        cwd: Some(a.path.to_string_lossy().into_owned()),
        program: None,
    });
    let turn_id = format!("turn-{t}");
    let now = chrono::Utc::now().timestamp_millis();
    thread_activity::start_turn(&a_conv, &a.handle, &turn_id, now);
    thread_activity::delivered(&a_conv, &turn_id, Some(&sid), None);
    let act = get(port, "/cli/thread/activity", OWNER, &format!("addr={}", a.handle)).await.json();
    let live_turn = act["turn"]["turnId"].as_str().unwrap_or_else(|| panic!("a live turn: {act}")).to_string();

    let mut s = open(port, OWNER).await;
    view(&mut s, &a_conv, true).await;
    let ok = next_kind(&mut s, "viewing_ok").await;
    assert_eq!(ok["turn"]["turnId"], live_turn.as_str(), "viewing_ok carries the turn: {ok}");
    assert_eq!(ok["turn"]["state"], act["turn"]["state"], "{ok}");

    // Turn frames: the viewed chat's only.
    publish_turn(&o_conv, "other-turn");
    publish_turn(&a_conv, &format!("mine-{t}"));
    let f = next_kind(&mut s, "thread_activity").await;
    assert_eq!(
        f,
        json!({
            "kind": "thread_activity",
            "conversation_id": a_conv,
            "collection": "activity",
            "seq": 0,
            "id": format!("mine-{t}"),
            "activity": { "turnId": format!("mine-{t}"), "state": "working" },
        }),
        "another chat's turn never arrives"
    );

    // A lag on the turn bus: a reset (the turn as it is now), never a close.
    for i in 0..3000 {
        publish_turn(&a_conv, &format!("flood-{i}"));
    }
    let reset = loop {
        let f = next(&mut s).await;
        if f["kind"] == "thread_activity_reset" {
            break f;
        }
    };
    assert_eq!(reset["conversation_id"], a_conv.as_str(), "{reset}");
    assert_eq!(reset["collection"], "activity_reset", "{reset}");
    assert_eq!(reset["activity"]["turnId"], live_turn.as_str(), "the live turn: {reset}");
    assert_eq!(reset["id"], live_turn.as_str(), "{reset}");

    // A move moves the view (and its registry entry).
    let a1 = post_text(port, &a, "before the move").await;
    until_marker(&mut s, &a1).await;
    let moved_to = format!("moved-{}", uuid::Uuid::new_v4());
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mv = k2_core::overlay::move_conversation(&conn, &a_conv, &moved_to).expect("move");
        assert!(mv.is_some(), "a had a Thread to move");
    }
    let m = next_kind(&mut s, "thread_moved").await;
    assert_eq!((m["from"].as_str(), m["to"].as_str()), (Some(a_conv.as_str()), Some(moved_to.as_str())), "{m}");
    assert!(viewing(&moved_to) && !viewing(&a_conv), "the registry follows the move");
    publish_thread(&a_conv, &format!("old-key-{t}"), "canary-old-key");
    let marker = format!("new-key-{t}");
    publish_thread(&moved_to, &marker, "on the new key");
    let frames = until_marker(&mut s, &marker).await;
    let threads: Vec<&Value> = frames.iter().filter(|f| f["kind"] == "thread_message").collect();
    assert_eq!(threads.len(), 1, "only the new key: {frames:?}");
    assert_eq!(threads[0]["conversation_id"], moved_to.as_str());

    activity_store::unregister(&sid);
    thread_activity::clear_for_tests();
    drop(s);
    drop(env);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_devices_view_independently_and_a_close_clears_only_its_own() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let a = seed_ws(&env, &format!("da{t}"));
    let conv = pin_chat(&a);

    let mut phone = open(port, OWNER).await;
    let mut tablet = open(port, OWNER).await;
    view(&mut phone, &conv, true).await;
    next_kind(&mut phone, "viewing_ok").await;
    view(&mut tablet, &conv, true).await;
    next_kind(&mut tablet, "viewing_ok").await;
    // The coalescer's `viewing` is this registry: the owner's Thread pushes
    // for this chat stay quiet while either device looks.
    assert!(viewing(&conv));
    assert!(!thread_viewers::global().is_viewing("gina", &conv, Instant::now()), "only the socket's own person");

    phone.close(None).await.expect("close the phone");
    drop(phone);
    // The tablet's view holds after the phone's socket is gone (give the
    // phone's close time to land; the tablet must still view after).
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(viewing(&conv), "the second device survives the first one's close");
    let p = post_text(port, &a, "still streamed").await;
    let frames = until_marker(&mut tablet, &p).await;
    assert_eq!(frames.len(), 1, "{frames:?}");

    tablet.close(None).await.expect("close the tablet");
    drop(tablet);
    wait_viewing(&conv, false, "both sockets closed").await;
    drop(env);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refusals_are_answered_never_a_silent_ack() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let a = seed_ws(&env, &format!("ra{t}"));
    let conv = pin_chat(&a);

    // An agent passport never reaches the hub.
    assert_eq!(refused(port, &format!("path=&token={}", enc(&passport(&a)))).await, 403, "an agent passport");

    let mut s = open(port, OWNER).await;
    send(&mut s, json!({ "t": "viewing", "visible": true })).await;
    let e = next_kind(&mut s, "viewing_error").await;
    assert_eq!(e["code"], "usage", "the overlay frame has no conversationId: {e}");
    view(&mut s, &"c".repeat(129), true).await;
    let e = next_kind(&mut s, "viewing_error").await;
    assert_eq!(e["code"], "usage", "{e}");

    // A Grok Bot's Thread while Show is off (the default) is not readable,
    // so not viewable.
    let gb = k2_core::external_agents::collect::conversation_id(&format!("bot{t}"));
    view(&mut s, &gb, true).await;
    let e = next_kind(&mut s, "viewing_error").await;
    assert_eq!((e["code"].as_str(), e["conversation_id"].as_str()), (Some("forbidden"), Some(gb.as_str())), "{e}");
    assert!(!viewing(&gb), "a refused view registers nothing");

    // A refusal ends the view it replaced; the socket still views after.
    view(&mut s, &conv, true).await;
    next_kind(&mut s, "viewing_ok").await;
    assert!(viewing(&conv));
    view(&mut s, &gb, true).await;
    next_kind(&mut s, "viewing_error").await;
    assert!(!viewing(&conv), "the old chat left the screen");
    view(&mut s, &conv, true).await;
    let ok = next_kind(&mut s, "viewing_ok").await;
    assert_eq!(ok["conversation_id"], conv.as_str());
    drop(s);
    drop(env);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_keyless_removal_carries_every_rollup() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let a = seed_ws(&env, &format!("ka{t}"));
    let mut s = open(port, OWNER).await;

    let in_ws = format!("in-ws-{}", uuid::Uuid::new_v4());
    let keyless = format!("keyless-{}", uuid::Uuid::new_v4());
    activity_store::register(activity_store::SessionFacts {
        session_id: in_ws.clone(),
        agent_name: format!("tab-{t}"),
        cwd: Some(a.path.to_string_lossy().into_owned()),
        program: None,
    });
    activity_store::register(activity_store::SessionFacts {
        session_id: keyless.clone(),
        agent_name: format!("loose-{t}"),
        cwd: None,
        program: None,
    });
    activity_store::unregister(&keyless);
    let gone = loop {
        let f = next_kind(&mut s, "activity_changed").await;
        if f["removed"] == keyless.as_str() {
            break f;
        }
    };
    assert!(gone["workspace"].is_null(), "no workspace to name: {gone}");
    let all = gone["workspaces"].as_array().unwrap_or_else(|| panic!("every rollup: {gone}"));
    let mine = all.iter().find(|w| w["projectId"] == a.id.as_str()).unwrap_or_else(|| panic!("a's rollup in {gone}"));
    assert_eq!(mine["live"], 1, "{mine}");
    for w in all {
        assert!(w.get("display").is_some() && w.get("counts").is_some(), "the snapshot's rollup shape: {w}");
    }

    activity_store::unregister(&in_ws);
    let gone = loop {
        let f = next_kind(&mut s, "activity_changed").await;
        if f["removed"] == in_ws.as_str() {
            break f;
        }
    };
    assert_eq!(gone["workspace"]["projectId"], a.id.as_str(), "{gone}");
    assert!(gone.get("workspaces").is_none(), "only a keyless removal carries every rollup: {gone}");
    drop(s);
    drop(env);
}
