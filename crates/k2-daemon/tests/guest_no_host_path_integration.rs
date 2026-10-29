//! App guests never see a host path (0.46.5 patch-up item 14, Agent NSI
//! Vanilla: a K2 system line on a room's Thread showed
//! `/home/<owner>/AI Projects/<workspace>` as its sender to App guests).
//!
//! Headless: the real dispatcher on a temp `$HOME`, over loopback.
//! - A system line stored the old way (sender and recipient = the
//!   workspace's folder, the folder in its text) reads clean for a guest
//!   on `GET /cli/thread`, `thread/latest`, the catch-up, and the overlay
//!   socket; the owner's view keeps the stored doc.
//! - The source: a post addressed by the workspace's folder now stores the
//!   workspace's address name as its sender and recipient.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::time::Duration;

use app_fixture::*;
use futures_util::StreamExt;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};

const PW: &str = "s3cret-horse-staple";

fn pin_chat(ws: &Ws) -> String {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running")
        .expect("pin");
    conv
}

async fn owner_ok(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

async fn login_with_role(port: u16, username: &str, role: &str) -> String {
    owner_ok(port, "/cli/skin/users", json!({ "username": username })).await;
    k2_daemon::test_harness::guest_with_own_password(&username, PW);
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body)))
        .await
        .unwrap();
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

type Sock = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn frame_for(sock: &mut Sock, id: &str) -> String {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), sock.next())
            .await
            .expect("a frame")
            .expect("open")
            .expect("ok");
        let Ok(t) = msg.to_text() else { continue };
        let Ok(v) = serde_json::from_str::<Value>(t) else { continue };
        if v["id"] == id {
            return t.to_string();
        }
    }
}

/// No host path anywhere in `body`: the workspace's folder (as stored and
/// as JSON-escaped), the test HOME, or a usual absolute prefix.
fn assert_no_host_path(what: &str, body: &str, ws_path: &str, home: &str) {
    for needle in [ws_path, home, "/home/", "/Users/", "/tmp/", "/private/", "/var/folders/", "/root/"] {
        assert!(!body.contains(needle), "{what}: {needle:?} reached an App guest: {body}");
    }
    let escaped = ws_path.replace('/', "\\/");
    assert!(!body.contains(&escaped), "{what}: escaped folder in {body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn app_guests_never_see_a_host_path_in_a_rooms_thread() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("gp{tag}"));
    let conv = pin_chat(&ws);
    let addr = ws.handle.clone();
    let ws_path = ws.path.to_string_lossy().into_owned();
    let home = env.path().to_string_lossy().into_owned();
    assert!(ws_path.starts_with('/') && ws_path.len() > 5, "{ws_path}");

    // ── A system line stored the old way (before the source fix) ──────
    let legacy_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let body = format!("K2: owner added the test guest **zz** to App **x**. Notes in {ws_path}/out/notes.md");
        let (item, _links) =
            k2_core::overlay::post_thread(&conn, &conv, &ws.id, &ws_path, &ws_path, &body, "k2-apps").expect("legacy post");
        item.id
    };

    // ── The owner still sees what was stored ─────────────────────────
    let r = get(port, "/cli/thread", OWNER, &format!("addr={}", enc(&addr))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let owner_items = r.json()["items"].as_array().expect("items").clone();
    let legacy_owner = owner_items.iter().find(|i| i["id"] == legacy_id.as_str()).expect("legacy item for the owner");
    assert_eq!(legacy_owner["doc"]["from"], ws_path.as_str(), "the owner's view is the stored doc");

    // ── The guest ──────────────────────────────────────────────────────
    let role = format!("room{tag}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read", "thread:post"] })).await;
    let guest = login_with_role(port, &format!("gst{tag}"), &role).await;

    let r = get(port, "/cli/thread", &guest, &format!("addr={}", enc(&addr))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_no_host_path("GET /cli/thread", &r.body, &ws_path, &home);
    let items = r.json()["items"].as_array().expect("items").clone();
    let legacy = items.iter().find(|i| i["id"] == legacy_id.as_str()).expect("the guest sees the system line");
    assert_eq!(legacy["doc"]["from"], addr.as_str(), "sender = the room's name: {legacy}");
    assert_eq!(legacy["doc"]["to"], addr.as_str(), "{legacy}");
    let body = legacy["doc"]["body"].as_str().expect("body");
    assert!(body.ends_with(&format!("Notes in {addr}/out/notes.md")), "the folder in the text becomes the name: {body}");

    let r = get(port, "/cli/thread/latest", &guest, &format!("addrs={}", enc(&addr))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_no_host_path("GET /cli/thread/latest", &r.body, &ws_path, &home);
    assert_eq!(r.json()["items"][0]["from"], addr.as_str(), "{}", r.body);

    let r = post(port, "/cli/thread/catchup", &guest, json!({ "threads": [{ "addr": addr, "afterSeq": 0 }], "limit": 50 })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains(&legacy_id), "the catch-up carries the line: {}", r.body);
    assert_no_host_path("POST /cli/thread/catchup", &r.body, &ws_path, &home);

    // ── The overlay socket: a line posted with the folder as sender ────
    let url = format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={conv}&token={}", enc(&guest));
    let (mut gsock, _) = tokio_tungstenite::connect_async(&url).await.expect("guest overlay WS");
    let owner_url = format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={conv}&token={}", enc(OWNER));
    let (mut osock, _) = tokio_tungstenite::connect_async(&owner_url).await.expect("owner overlay WS");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let r = post(
        port,
        "/cli/thread/post",
        OWNER,
        json!({ "addr": addr, "from": ws_path, "text": format!("K2: job **x** changed; logs in {ws_path}/logs") }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let id = r.json()["id"].as_str().expect("id").to_string();
    let graw = frame_for(&mut gsock, &id).await;
    assert_no_host_path("overlay WS frame", &graw, &ws_path, &home);
    let gv: Value = serde_json::from_str(&graw).unwrap();
    assert_eq!(gv["doc"]["from"], addr.as_str(), "{graw}");
    assert!(gv.get("rev").is_none(), "a guest frame still carries no rev: {graw}");
    let oraw = frame_for(&mut osock, &id).await;
    let ov: Value = serde_json::from_str(&oraw).unwrap();
    assert_eq!(ov["doc"]["from"], ws_path.as_str(), "the owner's frame is the stored doc: {oraw}");

    // ── The source: a post addressed by the folder stores the name ────
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": ws_path, "text": "addressed by folder", "from": "k2" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let new_id = r.json()["id"].as_str().expect("id").to_string();
    let r = get(port, "/cli/thread", OWNER, &format!("addr={}", enc(&addr))).await;
    let stored = r.json()["items"].as_array().expect("items").iter().find(|i| i["id"] == new_id.as_str()).cloned().expect("new item");
    assert_eq!(stored["doc"]["from"], addr.as_str(), "stored sender is the address name, not the folder: {stored}");
    assert_eq!(stored["doc"]["to"], addr.as_str(), "{stored}");

    // And the whole room, read by the guest at the end: still clean.
    let r = get(port, "/cli/thread", &guest, &format!("addr={}", enc(&addr))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_no_host_path("GET /cli/thread (end)", &r.body, &ws_path, &home);
}
