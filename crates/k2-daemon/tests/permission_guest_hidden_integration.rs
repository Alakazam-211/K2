//! Permission cards lane F, the guest arm (K2's rule, 2026-10-21): App
//! guests never see an action or permission card on any path — not as a
//! card, not as fallback text — whatever `cards` they send, while the
//! "Permission cards" switch is on.
//!
//! P0 (`604189f47`) drops action cards on every guest path and A3 tests the
//! `cards=2` read doors with a freshly posted card. This file covers what a
//! card does **after** it is posted, with the switch on and `cards=2` on
//! every guest door:
//! - the rewrite of a pending card (a note, `applying`, `already_done`) and
//!   its close (approved, as A1's `refresh_card` writes it) as frames on
//!   `WS /cli/overlay/events` and `WS /cli/thread/stream`;
//! - the delta's `changed[]` (`GET /cli/thread?after_seq=N&cards=2`) and
//!   `POST /cli/thread/catchup {cards:2}` after those rewrites;
//! - `GET /cli/thread/latest?cards=2` and `GET /cli/thread/unread`;
//! - the person-only doors: `POST /cli/thread/answer` (the card stays
//!   pending), and every `/cli/permission/*` route answers a guest pass
//!   with no card, request or setting in the body.
//!
//! Its own binary: it turns the process-wide server switch on (lane B's
//! rule). In-process daemon on a temp `$HOME`.

#![recursion_limit = "256"]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::time::Duration;

use app_fixture::*;
use futures_util::StreamExt;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Sock = WebSocketStream<MaybeTlsStream<TcpStream>>;

const PW: &str = "s3cret-horse-staple";
const FRAME_WAIT: Duration = Duration::from_secs(10);
const SETTING: &str = "Allow compute nodes";
const WHY: &str = "Run the test suite on a mini";

struct Room {
    ws: Ws,
    conv: String,
    guest: String,
}

fn tag() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

async fn owner_ok(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

/// A room (workspace + pinned Chat) and an App guest with `thread:read`
/// and `thread:post` there (so it could answer a card if it ever saw one).
async fn room(env: &Env, port: u16) -> Room {
    let t = tag();
    let ws = seed_ws(env, &format!("pgh{t}"));
    let conv = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    }
    let role = format!("gr{t}");
    let user = format!("gu{t}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read", "thread:post"] })).await;
    owner_ok(port, "/cli/skin/users", json!({ "username": user })).await;
    k2_daemon::test_harness::guest_with_own_password(&user, PW);
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": user, "role": role })).await;
    let body = json!({ "username": user, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "guest login: {}", r.body);
    let guest = r.json()["token"].as_str().expect("guest token").to_string();
    Room { ws, conv, guest }
}

fn view(rid: &str, h: &str, state: &str, notes: Value) -> Value {
    json!({"v":1,"kind":"permission","requestId":rid,"state":state,
        "viewer":{"login":"","role":"","via":"","mode":"wait","canAnswer":false,"isAsker":false},
        "action":"compute.use","setting":SETTING,
        "whatChanges":format!("Let {h} run jobs on this server's compute nodes."),
        "direction":"loosen","grant":"standing","scope":h,"duration":"until someone turns it off",
        "consequences":"Every active machine; default limits.",
        "values":[{"key":"workspace","value":h}],
        "why":{"by":h,"text":WHY},
        "notes":notes,"askedFor":null,"askers":null,
        "approvers":{"floor":"admin","label":"owner or admin","names":[]},
        "proof":{"kind":"click"},"expiresAt":k2_core::overlay::now_secs() + 3600,"settingsPage":format!("workspaces/{h}/agent"),
        "noteBox":{"allowed":false,"max":500,"mine":null},"answer":null,"undo":null})
}

/// A pending permission card on the room's pinned Chat plus its request
/// row (as A1 stores them), so the owner can answer it through the real
/// route. Returns (card id, request id).
fn post_card(r: &Room) -> (String, String) {
    let rid = format!("pr_{}", tag());
    let h = &r.ws.handle;
    let action: k2_core::overlay::CardAction = serde_json::from_value(json!({
        "id": "permission", "params": { "request": rid }, "dedupe": format!("compute.use:{rid}"),
        "view": view(&rid, h, "pending", json!([])),
    }))
    .expect("a permission action");
    let now = k2_core::overlay::now_secs();
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (item, links) = k2_core::overlay::post_choice_with_action(
        &conn,
        &r.conv,
        &r.ws.id,
        h,
        h,
        &format!("{h} asks: Let {h} run jobs on this server's compute nodes ({SETTING})."),
        vec!["Allow".to_string(), "Deny".to_string()],
        false,
        Some(now + 3600),
        Some(action),
    )
    .expect("post the card");
    conn.execute(
        "INSERT INTO permission_requests (id, workspace_id, conversation_id, addr, card_id, action, params_json, params_hash, \
         text_hash, why, state, floor, created_at, expires_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'compute.use', '{}', 'h', 't', ?6, 'pending', 'admin', ?7, ?8)",
        rusqlite::params![rid, r.ws.id, r.conv, h, item.id, WHY, now, now + 3600],
    )
    .expect("store the request");
    drop(conn);
    k2_daemon::overlay_ws::emit_links(&links, &item.doc);
    (item.id, rid)
}

/// Rewrite the pending card's view (a note arrived) and publish it, the way
/// A1's `refresh_card` does.
fn rewrite_card(r: &Room, card: &str, rid: &str) {
    let notes = json!([{"by":"dana","role":"member","text":"I need this today","at":k2_core::overlay::now_secs()}]);
    let cb = k2_core::overlay::update_action_view(&r.conv, card, view(rid, &r.ws.handle, "pending", notes)).expect("rewrite the view");
    k2_daemon::overlay_ws::publish(k2_daemon::overlay_ws::OverlayFrame {
        collection: "thread".to_string(),
        seq: cb.seq,
        id: cb.doc_id.clone(),
        doc: Some(cb.doc.clone()),
        activity: None,
        conversation_id: Some(cb.conversation_id.clone()),
        rev: 0,
    });
}

/// Close the card as approved (`close_action_card` + `record_action_outcome`)
/// and publish it.
fn close_card(r: &Room, card: &str, rid: &str) {
    let by = k2_core::overlay::PostAuthor { kind: "owner".to_string(), login: "owner".to_string(), name: None };
    k2_core::overlay::close_action_card(&r.conv, card, "Allow", Some(by)).expect("close the card");
    let mut v = view(rid, &r.ws.handle, "approved", json!([]));
    v["answer"] = json!({"decision":"allow","by":"owner","via":"desktop","at":k2_core::overlay::now_secs(),"reason":null});
    let result = json!({"message":"Allowed. Turn it off again in Settings → Workspaces → x → Agent.","outcome":"approved"});
    let cb = k2_core::overlay::record_action_outcome(&r.conv, card, Some(result), v).expect("record the outcome");
    assert_eq!(cb.doc.choice.as_ref().map(|c| c.status.as_str()), Some("answered"), "closed");
    k2_daemon::overlay_ws::publish(k2_daemon::overlay_ws::OverlayFrame {
        collection: "thread".to_string(),
        seq: cb.seq,
        id: cb.doc_id.clone(),
        doc: Some(cb.doc.clone()),
        activity: None,
        conversation_id: Some(cb.conversation_id.clone()),
        rev: 0,
    });
}

async fn post_text(port: u16, ws: &Ws, text: &str) -> (String, i64) {
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": ws.handle, "text": text })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    (v["id"].as_str().expect("id").to_string(), v["seq"].as_i64().unwrap_or_else(|| panic!("seq in {v}")))
}

/// Neither the card nor any of its words, ids or its request is in `body`.
fn assert_hidden(body: &str, card: &str, rid: &str, what: &str) {
    for needle in [card, rid, SETTING, WHY, "compute.use", "\"permission\"", "requestId"] {
        assert!(!body.contains(needle), "{what}: a guest saw {needle:?}: {body}");
    }
}

async fn next_text(sock: &mut Sock, what: &str) -> Value {
    loop {
        let m = tokio::time::timeout(FRAME_WAIT, sock.next())
            .await
            .unwrap_or_else(|_| panic!("a frame in time: {what}"))
            .unwrap_or_else(|| panic!("socket open: {what}"))
            .unwrap_or_else(|e| panic!("frame ok ({what}): {e}"));
        match m {
            Message::Text(t) => {
                let f: Value = serde_json::from_str(&t).unwrap_or_else(|e| panic!("JSON frame ({e}): {t}"));
                if f["collection"] == "activity" {
                    continue;
                }
                return f;
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected frame {other:?} ({what})"),
        }
    }
}

/// Every frame up to (and with) the one carrying `marker`.
async fn until(sock: &mut Sock, marker: &str, what: &str) -> Vec<Value> {
    let mut out = Vec::new();
    loop {
        let f = next_text(sock, what).await;
        let done = f["id"] == marker;
        out.push(f);
        if done {
            return out;
        }
    }
}

fn frames_for<'a>(frames: &'a [Value], id: &str) -> Vec<&'a Value> {
    frames.iter().filter(|f| f["id"] == id).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_never_gets_a_permission_card_on_any_door_through_its_whole_life() {
    let env = setup(&[]);
    k2_core::permission::gate::set_server_switch(true);
    assert!(k2_core::permission::gate::server_on(), "the Permission cards switch is on for this binary");
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let r = room(&env, port).await;

    let overlay = |token: String| {
        let url = format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={}&token={}&cards=2", r.conv, enc(&token));
        async move { tokio_tungstenite::connect_async(url).await.expect("overlay socket").0 }
    };
    let stream = |token: String| {
        let url = format!("ws://127.0.0.1:{port}/cli/thread/stream?rooms=all&token={}&cards=2", enc(&token));
        async move { tokio_tungstenite::connect_async(url).await.expect("thread stream").0 }
    };
    let mut o_owner = overlay(OWNER.to_string()).await;
    let mut o_guest = overlay(r.guest.clone()).await;
    let mut s_owner = stream(OWNER.to_string()).await;
    let mut s_guest = stream(r.guest.clone()).await;
    for (s, who) in [(&mut s_owner, "stream owner"), (&mut s_guest, "stream guest")] {
        loop {
            if next_text(s, who).await["kind"] == "inbox_subscribed" {
                break;
            }
        }
    }

    // 1. Posted: the owner's sockets carry it, the guest's never do.
    let (before, _) = post_text(port, &r.ws, "before").await;
    let (card, rid) = post_card(&r);
    let (m1, m1_seq) = post_text(port, &r.ws, "marker one").await;
    // 2. Rewritten while pending (a note): the same id and seq again.
    rewrite_card(&r, &card, &rid);
    let (m2, _) = post_text(port, &r.ws, "marker two").await;

    for (sock, who, marker) in [(&mut o_owner, "overlay owner", &m2), (&mut s_owner, "stream owner", &m2)] {
        let seen = until(sock, marker, who).await;
        assert_eq!(frames_for(&seen, &card).len(), 2, "{who}: the post and the rewrite: {seen:?}");
    }
    for (sock, who) in [(&mut o_guest, "overlay guest"), (&mut s_guest, "stream guest")] {
        let seen = until(sock, &m2, who).await;
        assert!(frames_for(&seen, &before).len() == 1 && frames_for(&seen, &m1).len() == 1, "{who} still gets the plain posts: {seen:?}");
        assert_hidden(&serde_json::to_string(&seen).unwrap(), &card, &rid, who);
    }

    // 3. The delta after the rewrite: the owner's `changed[]` has the card,
    //    the guest's answer has nothing of it.
    let h = enc(&r.ws.handle);
    let q = format!("addr={h}&after_seq={m1_seq}&limit=20&cards=2");
    let owner = get(port, "/cli/thread", OWNER, &q).await;
    assert_eq!(owner.status, 200, "{}", owner.body);
    let changed = owner.json()["changed"].clone();
    assert!(changed.as_array().is_some_and(|a| a.iter().any(|i| i["id"] == card.as_str())), "the owner's delta reports the rewrite: {changed}");
    let guest = get(port, "/cli/thread", &r.guest, &q).await;
    assert_eq!(guest.status, 200, "{}", guest.body);
    assert_hidden(&guest.body, &card, &rid, "delta changed[]");
    let guest_page = get(port, "/cli/thread", &r.guest, &format!("addr={h}&limit=50&cards=2")).await;
    assert_eq!(guest_page.status, 200, "{}", guest_page.body);
    assert_hidden(&guest_page.body, &card, &rid, "page");

    // 4. A guest can't answer it (person-only), and it stays pending.
    for body in [json!({ "addr": r.ws.handle, "id": card, "answer": "Allow" }), json!({ "addr": r.ws.handle, "id": card, "answer": "Close" })] {
        let g = post(port, "/cli/thread/answer", &r.guest, body.clone()).await;
        assert_ne!(g.status, 200, "a guest answered a permission card: {}", g.body);
        assert_hidden(&g.body, &card, &rid, "answer refusal");
    }
    let doc = k2_core::overlay::get_thread_doc(&r.conv, &card).expect("read").expect("the card");
    assert_eq!(doc.choice.as_ref().map(|c| c.status.as_str()), Some("pending"), "a guest's tap changed nothing");

    // 5. Every /cli/permission/* route answers a guest pass with nothing.
    for path in ["/cli/permission/pending", "/cli/permission/enabled", "/cli/permission/status", "/cli/permission/list"] {
        let g = get(port, path, &r.guest, &format!("id={rid}")).await;
        assert_ne!(g.status, 200, "GET {path} served a guest: {}", g.body);
        assert_hidden(&g.body, &card, &rid, path);
    }
    for path in ["/cli/permission/request", "/cli/permission/update", "/cli/permission/cancel"] {
        let g = post(port, path, &r.guest, json!({ "action": "compute.use", "id": rid, "why": "x" })).await;
        assert_ne!(g.status, 200, "POST {path} served a guest: {}", g.body);
        assert!(!g.body.contains(&card) && !g.body.contains(SETTING), "POST {path}: {}", g.body);
    }

    // 6. Closed (approved), the way A1's `refresh_card` closes a card: the
    //    closing rewrite goes to the owner's sockets and never to the
    //    guest's. (The real answer route needs a registered catalog action;
    //    a release build has none until lanes E1/E2.)
    close_card(&r, &card, &rid);
    let (m3, _) = post_text(port, &r.ws, "marker three").await;
    for (sock, who) in [(&mut o_owner, "overlay owner"), (&mut s_owner, "stream owner")] {
        let seen = until(sock, &m3, who).await;
        let closed: Vec<_> = frames_for(&seen, &card).into_iter().filter(|f| f["doc"]["choice"]["status"] == "answered").collect();
        assert!(!closed.is_empty(), "{who}: the close reaches the owner: {seen:?}");
    }
    for (sock, who) in [(&mut o_guest, "overlay guest"), (&mut s_guest, "stream guest")] {
        let seen = until(sock, &m3, who).await;
        assert_hidden(&serde_json::to_string(&seen).unwrap(), &card, &rid, who);
    }

    // 7. After the close: catchup, delta, latest and unread still show a
    //    guest nothing of it.
    for body in [json!({ "threads": [{ "addr": r.ws.handle, "afterSeq": 0 }], "cards": 2 }), json!({ "threads": [{ "addr": r.ws.handle, "newest": true }], "cards": 2 })] {
        let g = post(port, "/cli/thread/catchup", &r.guest, body).await;
        assert_eq!(g.status, 200, "{}", g.body);
        assert_hidden(&g.body, &card, &rid, "catchup");
    }
    let g = get(port, "/cli/thread", &r.guest, &format!("addr={h}&after_seq={m1_seq}&limit=20&cards=2")).await;
    assert_eq!(g.status, 200, "{}", g.body);
    assert_hidden(&g.body, &card, &rid, "delta after the close");
    let g = get(port, "/cli/thread/latest", &r.guest, &format!("addrs={h}&cards=2")).await;
    assert_eq!(g.status, 200, "{}", g.body);
    assert_hidden(&g.body, &card, &rid, "latest");
    let g = get(port, "/cli/thread/unread", &r.guest, "").await;
    assert_eq!(g.status, 200, "{}", g.body);
    assert_eq!(g.json()["waitingTotal"].as_i64(), Some(0), "a guest never waits on a permission card: {}", g.body);
    assert_hidden(&g.body, &card, &rid, "unread");

    k2_core::permission::gate::set_server_switch(false);
    drop(env);
}
