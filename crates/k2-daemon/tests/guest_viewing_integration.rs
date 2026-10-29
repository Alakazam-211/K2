//! App guests say "I'm looking at this Thread" (prd-tauri-core-0467-v1
//! TC-24, vs-live TA-23), headless: the real dispatcher in process on a
//! temp `$HOME`, over loopback.
//!
//! The agent-reply push skips a guest who is viewing the Thread
//! (`app_push::agent_reply`, keyed `guest:<principalId>`). Before this
//! fix the dispatcher dropped every guest socket's `{"t":"viewing"}`
//! frame, so that check could never fire. A guest's overlay socket now
//! registers its frames under its person key, the same key the push reads.
//!
//! `push.viewing` (TC-24) sets the same state from the App's server, with
//! the person's own pass, on any door; and `push.register` over the App's
//! link with a guest session of that App (TC-23: phone → App server → K2).

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::time::{Duration, Instant};

use app_fixture::*;
use futures_util::SinkExt;
use k2_core::db::schema::WorkspaceSession;
use k2_daemon::apps::gateway::{self, LinkSpec};
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
    guest_in_role(port, &format!("gv{tag}"), &role).await
}

/// `username` on the box login with `role`: its session pass and person key.
async fn guest_in_role(port: u16, username: &str, role: &str) -> (String, String) {
    let username = username.to_string();
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

/// A published App in `ws` with an App grant for `username` (`role`).
fn app_for(ws: &Ws, name: &str, username: &str, role: &str, port: u16) -> String {
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::insert(&conn, &ws.id, name, "node s.mjs", &ws.path.to_string_lossy(), port, "local", "stopped", "cmd", "")
            .expect("service")
            .id
    };
    let pid = k2_core::skin::host_login_detail(username).expect("guest").principal_id;
    k2_core::skin::create_grant(&k2_core::skin::SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: pid,
        kind: "app".into(),
        target_id: svc.clone(),
        role: Some(role.into()),
        scope: None,
        created_by: Some("owner".into()),
    })
    .expect("app grant");
    svc
}

/// One request with the pass as `Authorization: Bearer` (an App's link, or
/// the main port for a native app or an App server off the link).
fn bearer_blocking(port: u16, method: &str, pq: &str, pass: &str, body: Option<&str>) -> Resp {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).expect("timeout");
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {pass}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {pass}\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).expect("write");
    let mut out = Vec::new();
    s.read_to_end(&mut out).expect("read the answer");
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no status for {method} {pq}: {text:?}"));
    Resp { status, body: text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default() }
}

async fn bearer(port: u16, method: &'static str, pq: &str, pass: &str, body: Option<Value>) -> Resp {
    let (pq, pass, body) = (pq.to_string(), pass.to_string(), body.map(|b| b.to_string()));
    tokio::task::spawn_blocking(move || bearer_blocking(port, method, &pq, &pass, body.as_deref())).await.expect("join")
}

fn link_spec(app_id: &str, svc: &str, ws: &Ws) -> LinkSpec {
    LinkSpec {
        app_id: app_id.to_string(),
        service_id: svc.to_string(),
        workspace_id: ws.id.clone(),
        port: 0,
        caps: vec!["thread:read".into()],
        rooms: vec![ws.id.clone()],
    }
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

/// TC-24: `push.viewing` from an App's server with the person's own pass,
/// on the main port and on the App's link (AR50): visible:true marks that
/// guest viewing that Thread for 90 s, false ends it. Only a pinned Chat
/// the pass reads; never an App's own pass or the owner; POST only; an
/// exact body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_viewing_sets_the_guests_own_viewing_state_on_any_door() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let ws = seed_ws(&env, &format!("pva{tag}"));
    let conv = pin_chat(&ws);
    let other = seed_ws(&env, &format!("pvb{tag}"));
    let other_conv = pin_chat(&other);
    let (desk, key) = guest(port, &ws, tag).await;
    let svc = app_for(&ws, &format!("jobs{tag}"), &format!("gv{tag}"), &format!("view{tag}"), 47_421);
    let phone = k2_core::skin::create_app_session_token(&format!("gv{tag}"), &svc).expect("App session").raw;
    const VIEWING: &str = "/cli/skin/push/viewing";

    // Main port, the box login's pass.
    let r = bearer(port, "POST", VIEWING, &desk, Some(json!({ "conversation": conv, "visible": true }))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json(), json!({ "ok": true, "conversation": conv, "visible": true, "ttlSecs": 90 }));
    assert!(viewing(&key, &conv), "the guest views that Thread");
    let t = Instant::now();
    let reg = k2_daemon::thread_viewers::global();
    assert!(reg.is_viewing(&key, &conv, t + Duration::from_secs(85)));
    assert!(!reg.is_viewing(&key, &conv, t + Duration::from_secs(91)), "90 s per call");
    assert!(!viewing("owner", &conv) && !viewing(&key, &other_conv));
    let r = bearer(port, "POST", VIEWING, &desk, Some(json!({ "conversation": conv, "visible": false }))).await;
    assert_eq!((r.status, r.json()["ttlSecs"].as_u64()), (200, Some(0)), "{}", r.body);
    assert!(!viewing(&key, &conv), "false ends it");

    // Refusals: a Thread the pass doesn't read, bad bodies, GET, the owner.
    let r = bearer(port, "POST", VIEWING, &desk, Some(json!({ "conversation": other_conv, "visible": true }))).await;
    assert_eq!((r.status, r.code()), (403, "skin_room".to_string()), "{}", r.body);
    assert!(!viewing(&key, &other_conv));
    for bad in [
        json!({ "conversation": conv }),
        json!({ "visible": true }),
        json!({ "conversation": conv, "visible": "yes" }),
        json!({ "conversation": conv, "visible": true, "guest": "x" }),
    ] {
        let r = bearer(port, "POST", VIEWING, &desk, Some(bad.clone())).await;
        assert_eq!((r.status, r.code()), (400, "usage".to_string()), "{bad}: {}", r.body);
    }
    let r = bearer(port, "GET", VIEWING, &desk, None).await;
    assert_eq!(r.status, 405, "POST only: {}", r.body);
    let r = bearer(port, "POST", VIEWING, OWNER, Some(json!({ "conversation": conv, "visible": true }))).await;
    assert_eq!((r.status, r.code()), (403, "guest_required".to_string()), "the owner token is not a guest: {}", r.body);

    // The App's link: the App server with the person's App session.
    let app_pass = gateway::open(link_spec(&format!("jobs{tag}"), &svc, &ws), None).expect("open link");
    let lport = gateway::port_of(&format!("jobs{tag}")).expect("listening");
    let r = bearer(lport, "POST", VIEWING, &phone, Some(json!({ "conversation": conv, "visible": true }))).await;
    assert_eq!(r.status, 200, "the person's pass on the link: {}", r.body);
    assert!(viewing(&key, &conv), "one person on every door");
    let r = bearer(lport, "POST", VIEWING, &app_pass, Some(json!({ "conversation": conv, "visible": true }))).await;
    assert_eq!((r.status, r.code()), (403, "guest_required".to_string()), "the App's own pass is not a person: {}", r.body);
    let r = bearer(lport, "POST", VIEWING, &phone, Some(json!({ "conversation": conv, "visible": false }))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!viewing(&key, &conv));
    gateway::close(&format!("jobs{tag}"), None, true);
}

/// TC-23: push registration goes phone → App server → K2. The App's server
/// registers the person's device over its link with that person's App
/// session; another App's guest pass and the App's own pass are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_register_over_the_link_with_the_persons_own_session() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let ws = seed_ws(&env, &format!("pra{tag}"));
    let other = seed_ws(&env, &format!("prb{tag}"));
    guest(port, &ws, tag).await;
    let user = format!("gv{tag}");
    let svc = app_for(&ws, &format!("jobs{tag}"), &user, &format!("view{tag}"), 47_431);
    let crm = app_for(&other, &format!("crm{tag}"), &user, &format!("view{tag}"), 47_432);
    for app in [&svc, &crm] {
        let on = k2_core::skin::app_push::Switches { owner_on: Some(true), ..Default::default() };
        k2_core::skin::app_push::set_switches(app, on, "owner").expect("push on");
    }
    let phone = k2_core::skin::create_app_session_token(&user, &svc).expect("App session").raw;
    let crm_phone = k2_core::skin::create_app_session_token(&user, &crm).expect("crm session").raw;
    let app_pass = gateway::open(link_spec(&format!("jobs{tag}"), &svc, &ws), None).expect("open link");
    let lport = gateway::port_of(&format!("jobs{tag}")).expect("listening");
    let device = format!("dev-{tag}");
    let token = "a1".repeat(32);
    let reg = json!({ "deviceId": device, "platform": "apns", "pushToken": token, "environment": "sandbox" });
    let r = bearer(lport, "POST", "/cli/skin/push/register", &phone, Some(reg.clone())).await;
    assert_eq!(r.status, 200, "the App server registers the person's phone: {}", r.body);
    let v = r.json();
    assert_eq!((v["deviceId"].as_str(), v["platform"].as_str()), (Some(device.as_str()), Some("apns")), "{v}");
    assert!(!r.body.contains(&token), "never the token: {}", r.body);
    let r = bearer(lport, "GET", &format!("/cli/skin/push/device?deviceId={device}"), &phone, None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    // Another App's guest session never registers on this App's link.
    let r = bearer(lport, "POST", "/cli/skin/push/register", &crm_phone, Some(reg.clone())).await;
    assert_eq!(r.status, 401, "another App's pass on this link: {}", r.body);
    // The App's own pass is not a guest.
    let r = bearer(lport, "POST", "/cli/skin/push/register", &app_pass, Some(reg)).await;
    assert_eq!((r.status, r.code()), (403, "guest_required".to_string()), "{}", r.body);
    gateway::close(&format!("jobs{tag}"), None, true);
}
