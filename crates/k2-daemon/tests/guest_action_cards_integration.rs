//! App guests never see action (settings) cards (K2's rule, 2026-10-21:
//! owner/admin asks are none of a guest's business). The keep-as-mine card
//! (`zen.widget.keep`) is posted into the agent's pinned Chat, the room a
//! guest reads; permission cards will be too. On every guest path the card
//! is dropped, neither drawn as a card nor as any fallback text, while the
//! owner still gets it. A guest already gets seq gaps for hidden items.
//!
//! One test per path: the plain read (newest page and `after_seq` delta),
//! `POST /cli/thread/catchup`, the per-room overlay WS, the cross-room
//! `WS /cli/thread/stream`, and `GET /cli/thread/latest` (the preview).
//! Project chat has no action cards (they post only on an agent's Thread),
//! so it has no path to cover.
//!
//! In-process daemon on a temp `$HOME`; the card is written with the same
//! core call and frame publish `present_action_card` uses.

#![cfg(unix)]

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
const CARD_PROMPT: &str = "pog changed your widget. Keep it as yours?";

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

/// A room (workspace with a pinned Chat) and a guest with `thread:read` in it.
async fn room(env: &Env, port: u16) -> Room {
    let t = tag();
    let ws = seed_ws(env, &format!("gac{t}"));
    let conv = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    }
    let role = format!("gr{t}");
    let user = format!("gu{t}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read"] })).await;
    owner_ok(port, "/cli/skin/users", json!({ "username": user })).await;
    owner_ok(port, "/cli/skin/users/password", json!({ "username": user, "password": PW })).await;
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": user, "role": role })).await;
    let body = json!({ "username": user, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "guest login: {}", r.body);
    let guest = r.json()["token"].as_str().expect("guest token").to_string();
    Room { ws, conv, guest }
}

async fn post_text(port: u16, ws: &Ws, text: &str) -> String {
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": ws.handle, "text": text })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["id"].as_str().expect("id").to_string()
}

/// A keep-as-mine action card on the room's pinned Chat, posted and
/// published exactly as `present_action_card` does.
fn post_action_card(r: &Room) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    // Built from JSON so this commit cherry-picks with or without the
    // permission-cards P0 (which adds `CardAction.view`).
    let action: k2_core::overlay::CardAction = serde_json::from_value(json!({
        "id": "zen.widget.keep",
        "params": { "widget": "kanban", "hash": "a".repeat(64), "agent": r.ws.handle },
        "dedupe": "kanban",
    }))
    .expect("a keep-as-mine action");
    let (item, links) = k2_core::overlay::post_choice_with_action(
        &conn,
        &r.conv,
        &r.ws.id,
        &r.ws.handle,
        &r.ws.handle,
        CARD_PROMPT,
        vec!["Keep as mine".to_string(), "Leave it as pog's".to_string()],
        false,
        Some(k2_core::overlay::now_secs() + 3600),
        Some(action),
    )
    .expect("post the action card");
    drop(conn);
    k2_daemon::overlay_ws::emit_links(&links, &item.doc);
    item.id
}

/// Neither the card nor any text of it (prompt, options, action id) is in
/// `body`.
fn assert_no_card(body: &str, card: &str, what: &str) {
    for needle in [card, CARD_PROMPT, "Keep as mine", "zen.widget.keep"] {
        assert!(!body.contains(needle), "{what}: a guest saw {needle:?}: {body}");
    }
}

fn ids(items: &Value) -> Vec<String> {
    items.as_array().unwrap_or_else(|| panic!("items: {items}")).iter().map(|i| i["id"].as_str().unwrap().to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_plain_read_and_delta_drop_action_cards_for_guests() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let r = room(&env, port).await;
    let before = post_text(port, &r.ws, "before").await;
    let card = post_action_card(&r);
    let after = post_text(port, &r.ws, "after").await;

    for q in [format!("addr={}&limit=20", enc(&r.ws.handle)), format!("addr={}&after_seq=0&limit=20", enc(&r.ws.handle))] {
        let owner = get(port, "/cli/thread", OWNER, &q).await;
        assert_eq!(owner.status, 200, "{}", owner.body);
        assert_eq!(ids(&owner.json()["items"]), vec![before.clone(), card.clone(), after.clone()], "the owner still gets the card ({q})");
        let guest = get(port, "/cli/thread", &r.guest, &q).await;
        assert_eq!(guest.status, 200, "{}", guest.body);
        assert_eq!(ids(&guest.json()["items"]), vec![before.clone(), after.clone()], "the guest gets the rest ({q})");
        assert_no_card(&guest.body, &card, &q);
    }
    drop(env);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catch_up_drops_action_cards_for_guests() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let r = room(&env, port).await;
    let before = post_text(port, &r.ws, "before").await;
    let card = post_action_card(&r);
    for (want, label) in [(json!({ "addr": r.ws.handle, "afterSeq": 0 }), "after_seq"), (json!({ "addr": r.ws.handle, "newest": true }), "newest")] {
        let body = json!({ "threads": [want] });
        let owner = post(port, "/cli/thread/catchup", OWNER, body.clone()).await;
        assert_eq!(owner.status, 200, "{}", owner.body);
        assert_eq!(ids(&owner.json()["threads"][0]["items"]), vec![before.clone(), card.clone()], "owner {label}");
        let guest = post(port, "/cli/thread/catchup", &r.guest, body).await;
        assert_eq!(guest.status, 200, "{}", guest.body);
        assert_eq!(ids(&guest.json()["threads"][0]["items"]), vec![before.clone()], "guest {label}");
        assert_no_card(&guest.body, &card, label);
    }
    drop(env);
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

/// Every Thread-message frame up to the one carrying `marker`.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_per_room_overlay_socket_never_sends_a_guest_an_action_card() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let r = room(&env, port).await;
    let open = |token: String| {
        let url = format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={}&token={}", r.conv, enc(&token));
        async move { tokio_tungstenite::connect_async(url).await.expect("overlay socket").0 }
    };
    let mut owner = open(OWNER.to_string()).await;
    let mut guest = open(r.guest.clone()).await;
    let card = post_action_card(&r);
    let marker = post_text(port, &r.ws, "marker").await;

    let seen = until(&mut owner, &marker, "owner").await;
    assert!(seen.iter().any(|f| f["id"] == card.as_str()), "the owner gets the card's frame: {seen:?}");
    let seen = until(&mut guest, &marker, "guest").await;
    let all = serde_json::to_string(&seen).unwrap();
    assert_no_card(&all, &card, "per-room overlay socket");
    drop(env);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cross_room_stream_never_sends_a_guest_an_action_card() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let r = room(&env, port).await;
    let open = |token: String| {
        let url = format!("ws://127.0.0.1:{port}/cli/thread/stream?rooms=all&token={}", enc(&token));
        async move { tokio_tungstenite::connect_async(url).await.expect("thread stream").0 }
    };
    let mut owner = open(OWNER.to_string()).await;
    let mut guest = open(r.guest.clone()).await;
    // Both sockets have subscribed before anything is posted.
    for (s, who) in [(&mut owner, "owner"), (&mut guest, "guest")] {
        loop {
            if next_text(s, who).await["kind"] == "inbox_subscribed" {
                break;
            }
        }
    }
    let card = post_action_card(&r);
    let marker = post_text(port, &r.ws, "marker").await;

    let seen = until(&mut owner, &marker, "owner").await;
    assert!(seen.iter().any(|f| f["id"] == card.as_str()), "the owner's stream carries the card: {seen:?}");
    let seen = until(&mut guest, &marker, "guest").await;
    let all = serde_json::to_string(&seen).unwrap();
    assert_no_card(&all, &card, "thread stream");
    drop(env);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_latest_preview_never_shows_a_guest_an_action_card() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let r = room(&env, port).await;
    let before = post_text(port, &r.ws, "before").await;
    let card = post_action_card(&r);
    let q = format!("addrs={}", enc(&r.ws.handle));

    let owner = get(port, "/cli/thread/latest", OWNER, &q).await;
    assert_eq!(owner.status, 200, "{}", owner.body);
    let row = &owner.json()["items"][0];
    assert_eq!(row["kind"], "choice", "the owner's preview is the card: {row}");
    assert!(row["preview"].as_str().unwrap_or("").contains(CARD_PROMPT), "{row}");

    let guest = get(port, "/cli/thread/latest", &r.guest, &q).await;
    assert_eq!(guest.status, 200, "{}", guest.body);
    let row = &guest.json()["items"][0];
    assert_eq!(row["preview"], "before", "the guest's preview is the newest item it may see: {row}");
    assert_eq!(row["kind"], "text", "{row}");
    assert_no_card(&guest.body, &card, "thread/latest");
    let _ = before;
    drop(env);
}
