//! App read state per person (app-read-state-v1, K2 0.46.3; Rosson
//! 2026-10-20: "read state belongs to the signed-in person", shared across
//! desktop, Companion, Garden and custom Apps; research groups B + R3),
//! headless: the real dispatcher in process on a temp `$HOME`, over
//! loopback, real sockets, and an App's gateway link.
//!
//! - **One person, one key.** An App guest's sign-in on the box login (the
//!   desktop browser) and its sign-in on a published App (the phone App),
//!   the App's link with that guest's session (AR50): the same unread,
//!   cursor, mute and `badgeKey`, never the owner's.
//! - **Live `read` frames.** A cursor or mute moved on one surface reaches
//!   the guest's overlay socket (`collection: read`) and activity socket
//!   (`kind: read`) on another; never another reader's moves.
//! - **`thread.mute`** for a guest (its own state); an App's own pass is
//!   not a person (no badgeKey, no mute) and keeps its own bookmark.
//! - **R3.** Counts, rows, mutes and frames cover the pass's own rooms
//!   only; another room is the uniform `skin_room`.
//! - A removed guest's read state goes with them.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::time::Duration;

use app_fixture::*;
use futures_util::StreamExt;
use k2_core::db::schema::WorkspaceSession;
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

const PW: &str = "s3cret-horse-staple";

type Sock = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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

/// A role with `caps` in each of `rooms`.
async fn role(port: u16, name: &str, rooms: &[&Ws], caps: &[&str]) {
    owner_ok(port, "/cli/skin/roles", json!({ "name": name })).await;
    for ws in rooms {
        owner_ok(port, "/cli/skin/roles/room", json!({ "name": name, "handle": ws.handle, "caps": caps })).await;
    }
}

/// A guest on the box login with `role`: its session pass.
async fn guest_login(port: u16, username: &str, role: &str) -> String {
    owner_ok(port, "/cli/skin/users", json!({ "username": username })).await;
    k2_daemon::test_harness::guest_with_own_password(&username, PW);
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "guest login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

/// The agent posts `n` lines to its own room's Thread (an agent post wakes
/// nobody, so the pinned Chat stays put).
async fn agent_posts(port: u16, ws: &Ws, n: usize) -> Vec<i64> {
    let agent = passport(ws);
    let mut seqs = Vec::new();
    for i in 0..n {
        let r = post(port, "/cli/thread/post", &agent, json!({ "addr": ws.handle, "text": format!("line {i}") })).await;
        assert_eq!(r.status, 200, "{}", r.body);
        seqs.push(r.json()["seq"].as_i64().expect("seq"));
    }
    seqs
}

async fn unread(port: u16, token: &str) -> Value {
    let r = get(port, "/cli/thread/unread", token, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()
}

fn row<'a>(u: &'a Value, conv: &str) -> &'a Value {
    u["byConversation"].as_array().expect("rows").iter().find(|r| r["id"] == conv).unwrap_or_else(|| panic!("no row for {conv}: {u}"))
}

async fn open(port: u16, path: &str) -> Sock {
    let url = format!("ws://127.0.0.1:{port}{path}");
    let (ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap_or_else(|e| panic!("socket {path}: {e}"));
    ws
}

fn is_read(v: &Value) -> bool {
    v["collection"] == "read" || v["kind"] == "read"
}

/// The next `read` frame on `sock` (other frames skipped), within 5 s.
async fn next_read(sock: &mut Sock, what: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let msg = tokio::time::timeout(left, sock.next())
            .await
            .unwrap_or_else(|_| panic!("no read frame in 5 s: {what}"))
            .unwrap_or_else(|| panic!("socket closed: {what}"))
            .unwrap_or_else(|e| panic!("socket error ({what}): {e}"));
        if let Message::Text(t) = msg {
            let v: Value = serde_json::from_str(&t).unwrap_or_else(|e| panic!("{what}: {e}: {t}"));
            if is_read(&v) {
                return v;
            }
        }
    }
}

/// No `read` frame on `sock` for `ms` (other frames are fine).
async fn no_read(sock: &mut Sock, ms: u64, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return;
        }
        match tokio::time::timeout(left, sock.next()).await {
            Err(_) => return,
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: Value = serde_json::from_str(&t).unwrap_or_else(|e| panic!("{what}: {e}: {t}"));
                assert!(!is_read(&v), "{what}: unexpected read frame {v}");
            }
            Ok(Some(Ok(_))) => {}
            Ok(other) => panic!("{what}: socket ended: {other:?}"),
        }
    }
}

/// A published App in `ws` and an App grant for `username` with `role`.
fn app_for(ws: &Ws, name: &str, username: &str, role: &str) -> String {
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::insert(&conn, &ws.id, name, "node s.mjs", &ws.path.to_string_lossy(), 47_311, "local", "stopped", "cmd", "")
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

fn principal_of(username: &str) -> String {
    k2_core::skin::host_login_detail(username).expect("guest").principal_id
}

fn read_people_has(key: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT COUNT(*) FROM read_people WHERE person = ?1", [key], |r| r.get::<_, i64>(0)).expect("read_people") > 0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_is_one_person_on_every_surface_with_live_read_frames() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let ws = seed_ws(&env, &format!("rsa{tag}"));
    let conv = pin_chat(&ws);
    let other = seed_ws(&env, &format!("rsb{tag}"));
    let other_conv = pin_chat(&other);
    let rn = format!("staff{tag}");
    role(port, &rn, &[&ws], &["thread:read", "thread:post", "activity:read"]).await;
    let pat = format!("pat{tag}");
    // Surface 1: the box login in a desktop browser.
    let desk = guest_login(port, &pat, &rn).await;
    // Surface 2: the same person signed in to a published App (a phone).
    let svc = app_for(&ws, &format!("jobs{tag}"), &pat, &rn);
    let phone = k2_core::skin::create_app_session_token(&pat, &svc).expect("App session").raw;
    assert_ne!(desk, phone);
    let key = format!("guest:{}", principal_of(&pat));

    // Both surfaces start fully read and see the agent's three new lines.
    let u_desk = unread(port, &desk).await;
    let u_phone = unread(port, &phone).await;
    let badge = u_desk["badgeKey"].as_str().expect("a person's badgeKey").to_string();
    assert_eq!(u_phone["badgeKey"], badge.as_str(), "one person, one badge key: {u_phone}");
    let owner_badge = unread(port, OWNER).await["badgeKey"].as_str().expect("owner badge").to_string();
    assert_ne!(badge, owner_badge, "a guest is never the owner person");
    assert!(read_people_has(&key), "the guest is seen under {key}");
    let seqs = agent_posts(port, &ws, 3).await;
    agent_posts(port, &other, 2).await;
    for (who, tok) in [("desk", &desk), ("phone", &phone)] {
        let u = unread(port, tok).await;
        assert_eq!((u["total"].as_i64(), row(&u, &conv)["unread"].as_i64()), (Some(3), Some(3)), "{who}: {u}");
        // R3: the other room is never counted, listed or named.
        let rows = u["byConversation"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{who}: only its own room: {u}");
        assert!(!u.to_string().contains(&other_conv) && !u.to_string().contains(&other.handle), "{who}: {u}");
    }

    // The phone listens on its Thread socket and its activity socket.
    let mut phone_thread = open(port, &format!("/cli/overlay/events?conversation={conv}&token={}", enc(&phone))).await;
    let mut phone_activity = open(port, &format!("/cli/activity/events?workspace={}&token={}", ws.handle, enc(&phone))).await;
    // Give both sockets a moment to subscribe before the first move.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The desktop reads through the second line: the phone hears it.
    let r = post(port, "/cli/thread/read", &desk, json!({ "addr": ws.handle, "seq": seqs[1] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["changed"].as_bool(), v["unread"].as_i64(), v["total"].as_i64()), (Some(true), Some(1), Some(1)), "{v}");
    let f = next_read(&mut phone_thread, "thread socket after the desktop's read").await;
    assert_eq!(f["collection"], "read", "{f}");
    assert_eq!((f["id"].as_str(), f["lastReadSeq"].as_i64(), f["unread"].as_i64(), f["total"].as_i64()), (Some(conv.as_str()), Some(seqs[1]), Some(1), Some(1)), "{f}");
    assert_eq!(f["addr"], ws.handle.as_str());
    assert!(f.get("rev").is_none() && f.get("conversation_id").is_none(), "{f}");
    let a = next_read(&mut phone_activity, "activity socket after the desktop's read").await;
    assert_eq!((a["kind"].as_str(), a["workspace"].as_str(), a["lastReadSeq"].as_i64()), (Some("read"), Some(ws.handle.as_str()), Some(seqs[1])), "{a}");
    // The same key result on the phone.
    let u = unread(port, &phone).await;
    assert_eq!((row(&u, &conv)["lastReadSeq"].as_i64(), u["total"].as_i64()), (Some(seqs[1]), Some(1)), "{u}");

    // The phone mutes: the desktop's counts follow and the phone's own
    // sockets hear its own move too.
    let r = post(port, "/cli/thread/mute", &phone, json!({ "addr": ws.handle, "muted": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((r.json()["muted"].as_bool(), r.json()["total"].as_i64()), (Some(true), Some(0)), "{}", r.body);
    let f = next_read(&mut phone_thread, "after the phone's mute").await;
    assert_eq!((f["muted"].as_bool(), f["total"].as_i64(), f["unread"].as_i64()), (Some(true), Some(0), Some(1)), "{f}");
    let a = next_read(&mut phone_activity, "activity socket after the phone's mute").await;
    assert_eq!((a["muted"].as_bool(), a["workspace"].as_str()), (Some(true), Some(ws.handle.as_str())), "{a}");
    let u = unread(port, &desk).await;
    assert_eq!(u["total"], 0, "a muted room leaves the badge: {u}");
    assert_eq!(u["muted"], json!([{ "id": conv, "addr": ws.handle }]), "{u}");
    assert_eq!(row(&u, &conv)["muted"], true);
    let r = post(port, "/cli/thread/mute", &desk, json!({ "addr": ws.handle, "muted": false })).await;
    assert_eq!((r.status, r.json()["muted"].as_bool()), (200, Some(false)), "{}", r.body);
    assert_eq!(next_read(&mut phone_thread, "after the desktop's unmute").await["muted"], false);
    assert_eq!(next_read(&mut phone_activity, "activity socket after the desktop's unmute").await["muted"], false);
    // A mute in a room it doesn't hold: the uniform skin_room.
    let r = post(port, "/cli/thread/mute", &phone, json!({ "addr": other.handle, "muted": true })).await;
    assert_eq!((r.status, r.code()), (403, "skin_room".to_string()), "{}", r.body);
    let r = post(port, "/cli/thread/mute", &phone, json!({ "addr": ws.handle })).await;
    assert_eq!((r.status, r.code()), (400, "usage".to_string()), "muted is required: {}", r.body);
    let r = get(port, "/cli/thread/mute", &phone, &format!("addr={}&muted=true", ws.handle)).await;
    assert_eq!(r.status, 405, "POST only: {}", r.body);

    // Other readers' moves never reach this guest's sockets.
    let r = post(port, "/cli/thread/read", OWNER, json!({ "addr": ws.handle })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let sam = guest_login(port, &format!("sam{tag}"), &rn).await;
    let r = post(port, "/cli/thread/read", &sam, json!({ "addr": ws.handle })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    no_read(&mut phone_thread, 800, "the owner's and another guest's reads").await;
    no_read(&mut phone_activity, 200, "the owner's and another guest's reads").await;
    let u = unread(port, &phone).await;
    assert_eq!(row(&u, &conv)["lastReadSeq"].as_i64(), Some(seqs[1]), "their reads aren't this person's: {u}");
    assert_ne!(unread(port, &sam).await["badgeKey"], badge.as_str(), "another guest is another person");

    // Surface 3: the App's link, with this guest's App session (AR50).
    let alerts = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let a2 = alerts.clone();
    k2_daemon::apps::alerts::set_sink(Some(Box::new(move |w, t| a2.lock().unwrap().push((w.to_string(), t.to_string())))));
    let spec = LinkSpec {
        app_id: format!("jobs{tag}"),
        service_id: svc.clone(),
        workspace_id: ws.id.clone(),
        port: 0,
        caps: vec!["thread:read".into()],
        rooms: vec![ws.id.clone()],
    };
    let app_pass = gateway::open(spec, None).expect("open link");
    let lport = gateway::port_of(&format!("jobs{tag}")).expect("listening");
    let r = link(lport, "GET", "/cli/thread/unread", &phone, None).await;
    assert_eq!(r.status, 200, "the guest on the link: {}", r.body);
    assert_eq!((r.json()["badgeKey"].as_str(), row(&r.json(), &conv)["lastReadSeq"].as_i64()), (Some(badge.as_str()), Some(seqs[1])), "{}", r.body);
    let r = link(lport, "POST", "/cli/thread/read", &phone, Some(json!({ "addr": ws.handle }))).await;
    assert_eq!((r.status, r.json()["unread"].as_i64()), (200, Some(0)), "{}", r.body);
    let f = next_read(&mut phone_thread, "after the link read").await;
    assert_eq!((f["lastReadSeq"].as_i64(), f["unread"].as_i64()), (Some(seqs[2]), Some(0)), "{f}");
    assert_eq!(unread(port, &desk).await["total"], 0, "the desktop sees the link's read");

    // The App's own pass is a service account, not a person.
    let r = link(lport, "GET", "/cli/thread/unread", &app_pass, None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.json().get("badgeKey").is_none(), "no badge key for an App's own pass: {}", r.body);
    // Its own bookmark: its first look starts at the head, like anyone's.
    assert_eq!(row(&r.json(), &conv)["lastReadSeq"].as_i64(), Some(seqs[2]), "{}", r.body);
    let r = link(lport, "POST", "/cli/thread/mute", &app_pass, Some(json!({ "addr": ws.handle, "muted": true }))).await;
    assert_eq!((r.status, r.code()), (403, "forbidden".to_string()), "{}", r.body);
    // A new line: the App's pass reads it, the person hasn't.
    let s4 = agent_posts(port, &ws, 1).await[0];
    let r = link(lport, "POST", "/cli/thread/read", &app_pass, Some(json!({ "addr": ws.handle }))).await;
    assert_eq!((r.status, r.json()["lastReadSeq"].as_i64(), r.json()["unread"].as_i64()), (200, Some(s4), Some(0)), "{}", r.body);
    no_read(&mut phone_thread, 500, "the App's own bookmark").await;
    let u = unread(port, &phone).await;
    assert_eq!((row(&u, &conv)["lastReadSeq"].as_i64(), row(&u, &conv)["unread"].as_i64()), (Some(seqs[2]), Some(1)), "the person's own cursor: {u}");
    gateway::close(&format!("jobs{tag}"), None, true);
    k2_daemon::apps::alerts::set_sink(None);

    // A removed guest's read state goes with them.
    owner_ok(port, "/cli/skin/users/remove", json!({ "username": pat })).await;
    assert!(!read_people_has(&key), "{key} forgotten");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guests_counts_cover_its_own_rooms_only() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let a = seed_ws(&env, &format!("r3a{tag}"));
    let conv_a = pin_chat(&a);
    let b = seed_ws(&env, &format!("r3b{tag}"));
    let conv_b = pin_chat(&b);
    let c = seed_ws(&env, &format!("r3c{tag}"));
    let _conv_c = pin_chat(&c);
    // ana reads A and B; ben reads A and only holds files in C.
    let both = format!("both{tag}");
    role(port, &both, &[&a, &b], &["thread:read", "activity:read"]).await;
    let ana = guest_login(port, &format!("ana{tag}"), &both).await;
    let one = format!("one{tag}");
    role(port, &one, &[&a], &["thread:read", "activity:read"]).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": one, "handle": c.handle, "caps": ["files:read", "activity:read"] })).await;
    let ben = guest_login(port, &format!("ben{tag}"), &one).await;
    // First look: everything that exists now starts read.
    assert_eq!(unread(port, &ana).await["total"], 0);
    assert_eq!(unread(port, &ben).await["total"], 0);
    agent_posts(port, &a, 2).await;
    let sb = agent_posts(port, &b, 3).await;
    agent_posts(port, &c, 4).await;

    let u = unread(port, &ana).await;
    assert_eq!(u["total"], 5, "A + B: {u}");
    let ids: Vec<&str> = u["byConversation"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 2, "{u}");
    let u = unread(port, &ben).await;
    assert_eq!(u["total"], 2, "A only (C has no thread:read): {u}");
    assert_eq!(u["byConversation"].as_array().unwrap().len(), 1, "{u}");
    // ben's activity socket in C (no thread:read there) never carries reads,
    // and B's state is never his to read or move.
    let mut ben_c = open(port, &format!("/cli/activity/events?workspace={}&token={}", c.handle, enc(&ben))).await;
    let mut ana_a = open(port, &format!("/cli/activity/events?workspace={}&token={}", a.handle, enc(&ana))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    for (path, body) in [
        ("/cli/thread/read", json!({ "addr": b.handle })),
        ("/cli/thread/mute", json!({ "addr": b.handle, "muted": true })),
        ("/cli/thread/read", json!({ "conversationId": conv_b })),
        ("/cli/thread/read", json!({ "addr": c.handle })),
    ] {
        let r = post(port, path, &ben, body.clone()).await;
        assert_eq!(r.status, 403, "{path} {body}: {}", r.body);
    }
    let r = get(port, "/cli/thread/unread", &ben, "kind=project").await;
    assert_eq!(r.status, 400, "no project counts for an App: {}", r.body);
    // ana reads B: her A-room activity socket hears nothing (it's B's).
    let r = post(port, "/cli/thread/read", &ana, json!({ "addr": b.handle, "seq": sb[2] })).await;
    assert_eq!((r.status, r.json()["total"].as_i64()), (200, Some(2)), "her badge over her rooms: {}", r.body);
    no_read(&mut ana_a, 600, "a read in another room").await;
    no_read(&mut ben_c, 200, "a socket without thread:read").await;
    // ana marks A: her A socket hears it, ben's never does.
    let r = post(port, "/cli/thread/read", &ana, json!({ "addr": a.handle })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let f = next_read(&mut ana_a, "ana's read in A").await;
    assert_eq!((f["id"].as_str(), f["total"].as_i64()), (Some(conv_a.as_str()), Some(0)), "{f}");
    no_read(&mut ben_c, 300, "another guest's read").await;
    assert_eq!(unread(port, &ben).await["total"], 2, "ana's reads aren't ben's");
}

/// One request on an App's link: the pass as Bearer, JSON on POST.
fn on_link(port: u16, method: &str, pq: &str, pass: &str, body: Option<&str>) -> Resp {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect link");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {pass}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {pass}\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no status for {method} {pq}: {text:?}"));
    Resp { status, body: text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default() }
}

async fn link(port: u16, method: &'static str, pq: &str, pass: &str, body: Option<Value>) -> Resp {
    let (pq, pass, body) = (pq.to_string(), pass.to_string(), body.map(|b| b.to_string()));
    tokio::task::spawn_blocking(move || on_link(port, method, &pq, &pass, body.as_deref())).await.unwrap()
}
