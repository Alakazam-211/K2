//! App guests say "I'm looking at this Thread" (prd-tauri-core-0467-v1
//! TC-24, vs-live TA-23), headless: the real dispatcher in process on a
//! temp `$HOME`, over loopback.
//!
//! The agent-reply push skips a guest who is viewing the Thread
//! (`app_push::agent_reply`, keyed `guest:<principalId>`). Before this
//! fix the dispatcher dropped every guest socket's `{"t":"viewing"}`
//! frame, so that check could never fire. A guest's overlay socket now
//! registers its frames under its person key, the same key the push reads.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::time::{Duration, Instant};

use app_fixture::*;
use futures_util::SinkExt;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

const PW: &str = "s3cret-horse-staple";

/// One test at a time in this binary (each swaps the process `$HOME`).
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
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

/// A guest on the box login with `thread:read` in `ws`: its session pass
/// and its person key.
async fn guest(port: u16, ws: &Ws, tag: &str) -> (String, String) {
    let role = format!("view{tag}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read", "thread:post"] })).await;
    let username = format!("gv{tag}");
    owner_ok(port, "/cli/skin/users", json!({ "username": username })).await;
    k2_daemon::test_harness::guest_with_own_password(&username, PW);
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.expect("join");
    assert_eq!(r.status, 200, "guest login: {}", r.body);
    let pass = r.json()["token"].as_str().expect("token").to_string();
    let pid = k2_core::skin::host_login_detail(&username).expect("guest").principal_id;
    (pass, format!("guest:{pid}"))
}

fn viewing(key: &str, conv: &str) -> bool {
    k2_daemon::thread_viewers::global().is_viewing(key, conv, Instant::now())
}

async fn wait_viewing(key: &str, conv: &str, want: bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while viewing(key, conv) != want {
        assert!(Instant::now() < deadline, "{what}: viewing never became {want} for {key}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// TA-23: a guest's overlay socket `viewing` frames mark that guest (its
/// `guest:<principalId>` person key) as viewing that Thread only; `false`
/// and the socket closing clear it. Never the owner, never another Thread.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_socket_viewing_frame_marks_the_guest_viewing() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let ws = seed_ws(&env, &format!("gva{tag}"));
    let conv = pin_chat(&ws);
    let (pass, key) = guest(port, &ws, tag).await;

    let url = format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={conv}&token={}", enc(&pass));
    let (mut sock, _) = tokio_tungstenite::connect_async(&url).await.expect("the guest's overlay socket opens");
    assert!(!viewing(&key, &conv), "nobody looks before the first frame");

    sock.send(Message::Text(r#"{"t":"viewing","visible":true}"#.into())).await.expect("send viewing");
    wait_viewing(&key, &conv, true, "after visible:true").await;
    assert!(!viewing("owner", &conv), "a guest's frame never marks the owner");
    assert!(!viewing(&key, "another-thread"), "only that Thread");

    sock.send(Message::Text(r#"{"t":"viewing","visible":false}"#.into())).await.expect("send hidden");
    wait_viewing(&key, &conv, false, "after visible:false").await;

    sock.send(Message::Text(r#"{"t":"viewing","visible":true}"#.into())).await.expect("send viewing again");
    wait_viewing(&key, &conv, true, "after visible:true again").await;
    sock.close(None).await.expect("close");
    wait_viewing(&key, &conv, false, "after the socket closed").await;
}
