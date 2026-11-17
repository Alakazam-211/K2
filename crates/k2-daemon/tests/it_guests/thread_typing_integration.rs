//! Who is typing in a Thread (thread-typing-v1, 0.46.15), headless: the real
//! dispatcher in process on a temp `$HOME`, over loopback.
//!
//! A person (the owner token, a Connect login) or an App guest sends
//! `{"t":"typing","thread"}` on its overlay socket; the Thread's other
//! viewers get `{collection:"typing", seq:0, id, member, name, kind, at}`.
//! Never the typist's own sockets, never a socket on another Thread, an App
//! guest's socket only with `?ear=1`, one frame per connection per 8 s
//! (Sew's rule), only from someone who may post, and agents never type.

use crate::support::app_fixture;

use std::time::{Duration, Instant};

use app_fixture::*;
use futures_util::{SinkExt, StreamExt};
use k2_core::connect_users::{self, Role};
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

const PW: &str = "s3cret-horse-staple";

/// How long a socket must stay quiet to count as "got nothing". Frames are
/// published before the sender's next one is read, so once one viewer has
/// its frame every other socket would already have had it.
const QUIET: Duration = Duration::from_millis(1200);

type Sock = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// One test at a time in this binary (each swaps the process `$HOME`).
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn tag() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
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

/// A guest on the box login, `thread:read` + `thread:post` in `ws`: its
/// session pass, its person key (`guest:<principalId>`) and its username.
async fn guest(port: u16, ws: &Ws, username: &str) -> (String, String) {
    guest_with(port, ws, username, &["thread:read", "thread:post"]).await
}

/// [`guest`] with these caps in `ws`.
async fn guest_with(port: u16, ws: &Ws, username: &str, caps: &[&str]) -> (String, String) {
    let role = format!("r{username}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    owner_ok(port, "/cli/skin/users", json!({ "username": username })).await;
    k2_daemon::test_harness::guest_with_own_password(username, PW);
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.expect("join");
    assert_eq!(r.status, 200, "guest login: {}", r.body);
    let pass = r.json()["token"].as_str().expect("token").to_string();
    let pid = k2_core::skin::host_login_detail(username).expect("guest").principal_id;
    (pass, format!("guest:{pid}"))
}

/// A Connect login with `role`: its session token.
fn login(username: &str, role: Role) -> String {
    connect_users::add_user(username, "password123").expect("add_user");
    connect_users::set_role(username, role).expect("set_role");
    connect_users::create_session(username)
}

fn url(port: u16, conv: &str, token: &str, ear: bool) -> String {
    format!(
        "ws://127.0.0.1:{port}/cli/overlay/events?conversation={}&token={}{}",
        enc(conv),
        enc(token),
        if ear { "&ear=1" } else { "" }
    )
}

async fn open(port: u16, conv: &str, token: &str, ear: bool) -> Sock {
    tokio_tungstenite::connect_async(&url(port, conv, token, ear)).await.expect("the overlay socket opens").0
}

fn viewing(key: &str, conv: &str) -> bool {
    k2_daemon::thread_viewers::global().is_viewing(key, conv, Instant::now())
}

async fn wait_viewing(key: &str, conv: &str, want: bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while viewing(key, conv) != want {
        assert!(Instant::now() < deadline, "viewing never became {want} for {key}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The socket's loop is running, so every bus it hears is subscribed: its
/// `viewing` frame lands (`key`: the socket's person key). Leaves it not
/// viewing, so the next socket of the same person can be checked the same way.
async fn ready(sock: &mut Sock, key: &str, conv: &str) {
    sock.send(Message::Text(r#"{"t":"viewing","visible":true}"#.into())).await.expect("send viewing");
    wait_viewing(key, conv, true).await;
    sock.send(Message::Text(r#"{"t":"viewing","visible":false}"#.into())).await.expect("send hidden");
    wait_viewing(key, conv, false).await;
}

async fn typing(sock: &mut Sock, thread: &str) {
    sock.send(Message::Text(json!({ "t": "typing", "thread": thread }).to_string())).await.expect("send typing");
}

/// The next `typing` frame on `sock` within `within` (other frames are
/// skipped), or `None`.
async fn next_typing(sock: &mut Sock, within: Duration) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        match tokio::time::timeout(left, sock.next()).await {
            Err(_) => return None,
            Ok(None) => panic!("the socket ended"),
            Ok(Some(Err(e))) => panic!("socket error: {e}"),
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: Value = serde_json::from_str(&t).unwrap_or_else(|e| panic!("frame JSON ({e}): {t}"));
                if v["collection"] == "typing" {
                    return Some(v);
                }
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

/// The doc of the `thread` frame whose body is `body` on an overlay socket.
async fn next_thread(sock: &mut Sock, body: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let m = tokio::time::timeout(left, sock.next()).await.expect("a thread frame in time").expect("socket open").expect("frame ok");
        if let Message::Text(t) = m {
            let v: Value = serde_json::from_str(&t).unwrap_or_else(|e| panic!("frame JSON ({e}): {t}"));
            if v["collection"] == "thread" && v["doc"]["body"] == body {
                return v["doc"].clone();
            }
        }
    }
}

/// A login's hub socket (`/cli/sessions/events`), after its hello.
async fn open_hub(port: u16, token: &str) -> Sock {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?path=&token={}", enc(token));
    let (mut sock, _) = tokio_tungstenite::connect_async(&url).await.expect("the hub socket opens");
    hub_kind(&mut sock, "hello").await;
    sock
}

/// The next hub frame of `kind` (others skipped).
async fn hub_kind(sock: &mut Sock, kind: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let m = tokio::time::timeout(left, sock.next())
            .await
            .unwrap_or_else(|_| panic!("no {kind} in time"))
            .expect("socket open")
            .expect("frame ok");
        if let Message::Text(t) = m {
            let v: Value = serde_json::from_str(&t).unwrap_or_else(|e| panic!("frame JSON ({e}): {t}"));
            if v["kind"] == kind {
                return v;
            }
        }
    }
}

/// View `conv` on a hub socket (hub-viewing-v1), answered `viewing_ok`.
async fn hub_view(sock: &mut Sock, conv: &str) {
    let f = json!({ "t": "viewing", "conversationId": conv, "visible": true }).to_string();
    sock.send(Message::Text(f)).await.expect("send viewing");
    let ok = hub_kind(sock, "viewing_ok").await;
    assert_eq!(ok["conversation_id"], json!(conv), "{ok}");
}

/// Typing on the hub, naming the Thread by `conversationId` or not.
async fn hub_typing(sock: &mut Sock, thread: &str, conv: Option<&str>) {
    let mut f = json!({ "t": "typing", "thread": thread });
    if let Some(c) = conv {
        f["conversationId"] = json!(c);
    }
    sock.send(Message::Text(f.to_string())).await.expect("send hub typing");
}

/// The hub frame's exact shape.
fn assert_hub_frame(v: &Value, conv: &str, address: &str, member: &str, name: &str, kind: &str) {
    let at = v["at"].as_i64().unwrap_or_else(|| panic!("at is ms: {v}"));
    assert_eq!(
        v,
        &json!({
            "kind": "thread_typing", "conversation_id": conv, "collection": "typing", "seq": 0,
            "id": address, "member": member, "name": name, "memberKind": kind, "at": at,
        }),
        "the hub frame carries who and when, nothing else"
    );
}

async fn got(sock: &mut Sock, what: &str) -> Value {
    next_typing(sock, Duration::from_secs(10)).await.unwrap_or_else(|| panic!("{what}: no typing frame"))
}

async fn nothing(sock: &mut Sock, what: &str) {
    if let Some(v) = next_typing(sock, QUIET).await {
        panic!("{what}: got a typing frame {v}");
    }
}

/// The exact contract shape (`at` is the daemon's ms clock).
fn assert_frame(v: &Value, address: &str, member: &str, name: &str, kind: &str) {
    let at = v["at"].as_i64().unwrap_or_else(|| panic!("at is ms: {v}"));
    assert!(at > 1_700_000_000_000, "at is unix ms: {v}");
    assert_eq!(
        v,
        &json!({ "collection": "typing", "seq": 0, "id": address, "member": member, "name": name, "kind": kind, "memberKind": kind, "at": at }),
        "the frame carries who and when, nothing else"
    );
}

/// Two people on one Thread: the owner's typing reaches the Connect login,
/// never the owner's own other socket; the login's typing reaches both of
/// the owner's sockets, never its own. The name is the Thread's `from`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persons_typing_reaches_the_other_person_not_their_own_sockets() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let ws = seed_ws(&env, &format!("tpa{t}"));
    let conv = pin_chat(&ws);
    let bea = format!("bea{t}");
    let bea_token = login(&bea, Role::Member);

    let mut a1 = open(port, &conv, OWNER, false).await;
    ready(&mut a1, "owner", &conv).await;
    let mut a2 = open(port, &conv, OWNER, false).await;
    ready(&mut a2, "owner", &conv).await;
    let mut b = open(port, &conv, &bea_token, false).await;
    ready(&mut b, &bea, &conv).await;

    typing(&mut a1, &ws.handle).await;
    let v = got(&mut b, "the login hears the owner").await;
    assert_frame(&v, &ws.handle, "owner", &k2_daemon::workspace_msg::resolve_owner_from(), "person");
    nothing(&mut a2, "the owner's other window").await;
    nothing(&mut a1, "the owner's own socket").await;

    // The conversation id names the same Thread; the frame says its address.
    typing(&mut b, &conv).await;
    let v = got(&mut a1, "the owner hears the login").await;
    assert_frame(&v, &ws.handle, &bea, &bea, "person");
    let v = got(&mut a2, "on every window").await;
    assert_frame(&v, &ws.handle, &bea, &bea, "person");
    nothing(&mut b, "the login's own socket").await;

    // `member` is the `author.login` that person's posts carry, so a client
    // clears the name when their message lands.
    let r = post(port, "/cli/thread/post", &bea_token, json!({ "addr": ws.handle, "text": "hi" })).await;
    assert_eq!(r.status, 200, "the login posts: {}", r.body);
    let doc = next_thread(&mut a1, "hi").await;
    assert_eq!(doc["author"]["login"], json!(bea), "the login's post: {doc}");
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": ws.handle, "text": "hello" })).await;
    assert_eq!(r.status, 200, "the owner posts: {}", r.body);
    let doc = next_thread(&mut b, "hello").await;
    assert_eq!(doc["author"]["login"], json!("owner"), "the owner's post: {doc}");
}

/// Nothing for anyone who can't read the Thread: a guest of another room
/// (ear on) hears nothing; a Voice & Chat only login and a guest without
/// the room can't open the Thread's socket at all. A frame naming another
/// Thread on this socket is dropped (and doesn't use up the connection's 8 s window).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_reader_gets_nothing() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let ws = seed_ws(&env, &format!("tpb{t}"));
    let conv = pin_chat(&ws);
    let other = seed_ws(&env, &format!("tpc{t}"));
    let other_conv = pin_chat(&other);
    let bea_token = login(&format!("bea{t}"), Role::Member);
    let (outsider, outsider_key) = guest(port, &other, &format!("go{t}")).await;

    // No socket on this Thread for those who can't read it.
    let vc = login(&format!("vc{t}"), Role::VoiceChat);
    assert!(tokio_tungstenite::connect_async(&url(port, &conv, &vc, false)).await.is_err(), "Voice & Chat only can't open an agent's Thread");
    assert!(
        tokio_tungstenite::connect_async(&url(port, &conv, &outsider, true)).await.is_err(),
        "a guest of another room can't open this Thread"
    );

    let mut a = open(port, &conv, OWNER, false).await;
    ready(&mut a, "owner", &conv).await;
    let mut b = open(port, &conv, &bea_token, false).await;
    ready(&mut b, &format!("bea{t}"), &conv).await;
    let mut x = open(port, &other_conv, &outsider, true).await;
    ready(&mut x, &outsider_key, &other_conv).await;

    typing(&mut a, &other.handle).await;
    nothing(&mut b, "a frame naming another Thread is dropped").await;
    nothing(&mut x, "and never reaches that other Thread").await;

    typing(&mut a, &ws.handle).await;
    let v = got(&mut b, "a reader hears it").await;
    assert_eq!(v["id"], json!(ws.handle), "{v}");
    nothing(&mut x, "a guest on another Thread, ear on").await;
}

/// Agents never type: a passport can't open the overlay socket, so it has
/// no way to send a typing frame; a reserved login name types as nobody.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_never_types() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let ws = seed_ws(&env, &format!("tpd{t}"));
    let conv = pin_chat(&ws);
    let agent = passport(&ws);
    assert!(tokio_tungstenite::connect_async(&url(port, &conv, &agent, false)).await.is_err(), "an agent passport is refused at the socket");
    assert!(k2_daemon::thread_typing::typist_of_login("agent").is_none(), "the agent name is nobody");

    // The owner's socket still works on that Thread, and hears no agent.
    let mut a = open(port, &conv, OWNER, false).await;
    ready(&mut a, "owner", &conv).await;
    nothing(&mut a, "nobody typed").await;
}

/// An App guest's typing reaches the box's people and the room's other
/// guests who opted in to the ear (`?ear=1`), never a guest socket without
/// it and never the typist's own other socket. A person's typing reaches
/// an opted-in guest too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_guests_typing_reaches_people_and_opted_in_guests() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let ws = seed_ws(&env, &format!("tpe{t}"));
    let conv = pin_chat(&ws);
    let gina = format!("gg{t}");
    let hal = format!("gh{t}");
    let (g_pass, g_key) = guest(port, &ws, &gina).await;
    let (h_pass, h_key) = guest(port, &ws, &hal).await;

    let mut a = open(port, &conv, OWNER, false).await;
    ready(&mut a, "owner", &conv).await;
    let mut g = open(port, &conv, &g_pass, false).await;
    ready(&mut g, &g_key, &conv).await;
    let mut g_ear = open(port, &conv, &g_pass, true).await;
    ready(&mut g_ear, &g_key, &conv).await;
    let mut h_plain = open(port, &conv, &h_pass, false).await;
    ready(&mut h_plain, &h_key, &conv).await;
    let mut h_ear = open(port, &conv, &h_pass, true).await;
    ready(&mut h_ear, &h_key, &conv).await;

    typing(&mut g, &ws.handle).await;
    let v = got(&mut a, "the box's person hears the guest").await;
    assert_frame(&v, &ws.handle, &gina, &gina, "app_guest");
    let v = got(&mut h_ear, "another guest with the ear").await;
    assert_frame(&v, &ws.handle, &gina, &gina, "app_guest");
    nothing(&mut h_plain, "a guest socket without ?ear=1").await;
    nothing(&mut g_ear, "the guest's own other socket").await;

    typing(&mut a, &ws.handle).await;
    let v = got(&mut h_ear, "a guest with the ear hears the person").await;
    assert_frame(&v, &ws.handle, "owner", &k2_daemon::workspace_msg::resolve_owner_from(), "person");
    let v = got(&mut g_ear, "and so does the typist guest's ear socket").await;
    assert_eq!(v["member"], json!("owner"), "{v}");
    nothing(&mut h_plain, "still nothing without the ear").await;
    nothing(&mut g, "nor on the guest's socket without the ear").await;

    // Only someone who may post shows as typing: a read-only guest's frame
    // goes nowhere (it still hears others, with the ear).
    let reader = format!("gr{t}");
    let (r_pass, r_key) = guest_with(port, &ws, &reader, &["thread:read"]).await;
    let mut r = open(port, &conv, &r_pass, true).await;
    ready(&mut r, &r_key, &conv).await;
    typing(&mut r, &ws.handle).await;
    nothing(&mut a, "a guest without thread:post never shows as typing").await;
    nothing(&mut h_ear, "to anyone").await;
}

/// Sew's throttle: one typing frame per Thread per CONNECTION per 8 s;
/// extras are dropped and counted, never queued. Another connection of the
/// same person has its own window (clients dedupe per member).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_frame_per_connection_per_eight_seconds() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let ws = seed_ws(&env, &format!("tpf{t}"));
    let conv = pin_chat(&ws);
    let bea = format!("bea{t}");
    let bea_token = login(&bea, Role::Member);
    let mut a = open(port, &conv, OWNER, false).await;
    ready(&mut a, "owner", &conv).await;
    let mut a2 = open(port, &conv, OWNER, false).await;
    ready(&mut a2, "owner", &conv).await;
    let mut b = open(port, &conv, &bea_token, false).await;
    ready(&mut b, &bea, &conv).await;

    let dropped_before = k2_daemon::thread_typing::throttled_total();
    let first = Instant::now();
    typing(&mut a, &ws.handle).await;
    typing(&mut a, &ws.handle).await;
    typing(&mut a, &ws.handle).await;
    got(&mut b, "the connection's first frame").await;
    nothing(&mut b, "its repeats inside 8 s are dropped").await;
    assert!(k2_daemon::thread_typing::throttled_total() >= dropped_before + 2, "the drops are counted");

    // The same person's other window is its own connection.
    typing(&mut a2, &ws.handle).await;
    let v = got(&mut b, "another connection's first frame").await;
    assert_eq!(v["member"], json!("owner"), "{v}");

    // Past the first connection's window, with room for a slow first frame.
    let wait = Duration::from_millis(8_600).saturating_sub(first.elapsed());
    tokio::time::sleep(wait).await;
    typing(&mut a, &ws.handle).await;
    got(&mut b, "a frame after the window").await;
}

/// The hub path (hub-viewing-v1): a Thread viewed through
/// `/cli/sessions/events` has no overlay socket. Typing crosses both ways:
/// an overlay typist is seen by the hub viewers of that Thread, a hub
/// typist by its overlay sockets and other hub viewers. Never the typist's
/// own sockets on either, never a hub socket viewing another Thread, and
/// the hub keeps the 8 s throttle per Thread per connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typing_crosses_between_the_overlay_socket_and_the_hub() {
    let _serial = serial();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let t = tag();
    let ws = seed_ws(&env, &format!("tph{t}"));
    let conv = pin_chat(&ws);
    let other = seed_ws(&env, &format!("tpi{t}"));
    let other_conv = pin_chat(&other);
    let bea = format!("bea{t}");
    let bea_token = login(&bea, Role::Member);
    let cy_token = login(&format!("cy{t}"), Role::Member);
    let owner_name = k2_daemon::workspace_msg::resolve_owner_from();

    // Overlay sockets first: a hub view also marks its person viewing.
    let mut a = open(port, &conv, OWNER, false).await;
    ready(&mut a, "owner", &conv).await;
    let mut b = open(port, &conv, &bea_token, false).await;
    ready(&mut b, &bea, &conv).await;
    let mut ho = open_hub(port, OWNER).await;
    hub_view(&mut ho, &conv).await;
    let mut hb = open_hub(port, &bea_token).await;
    hub_view(&mut hb, &conv).await;
    let mut hc = open_hub(port, &cy_token).await;
    hub_view(&mut hc, &other_conv).await;

    // Overlay → hub.
    typing(&mut b, &ws.handle).await;
    let v = got(&mut ho, "the owner's hub view hears the login's overlay").await;
    assert_hub_frame(&v, &conv, &ws.handle, &bea, &bea, "person");
    let v = got(&mut a, "and the owner's overlay socket").await;
    assert_frame(&v, &ws.handle, &bea, &bea, "person");
    nothing(&mut hb, "the typist's own hub socket").await;
    nothing(&mut hc, "a hub socket viewing another Thread").await;

    // Hub → overlay and hub, the Thread taken from the socket's view.
    hub_typing(&mut ho, &ws.handle, None).await;
    let v = got(&mut b, "the login's overlay hears the owner's hub").await;
    assert_frame(&v, &ws.handle, "owner", &owner_name, "person");
    let v = got(&mut hb, "and the login's hub view").await;
    assert_hub_frame(&v, &conv, &ws.handle, "owner", &owner_name, "person");
    nothing(&mut a, "the typist's own overlay socket").await;
    nothing(&mut ho, "the typist's own hub socket").await;
    nothing(&mut hc, "not a viewer of this Thread").await;

    // `conversationId` names the Thread; repeats inside 8 s are dropped.
    hub_typing(&mut hb, &ws.handle, Some(&conv)).await;
    hub_typing(&mut hb, &ws.handle, Some(&conv)).await;
    let v = got(&mut ho, "the owner's hub hears the login's hub").await;
    assert_eq!((v["member"].clone(), v["conversation_id"].clone()), (json!(bea), json!(conv)), "{v}");
    got(&mut a, "and the owner's overlay").await;
    nothing(&mut ho, "the hub connection's repeat inside 8 s").await;
    nothing(&mut a, "anywhere").await;

    // A hub frame whose address names another Thread than its
    // `conversationId` is dropped.
    hub_typing(&mut hc, &other.handle, Some(&conv)).await;
    nothing(&mut ho, "the address names another Thread").await;
    nothing(&mut b, "on any socket").await;
}

/// `GET /boot-status` lists `thread-typing-v1`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_status_lists_thread_typing() {
    let _serial = serial();
    let _env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let r = get(d.port, "/boot-status", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let features = r.json()["features"].as_array().cloned().unwrap_or_else(|| panic!("features: {}", r.body));
    assert!(features.contains(&json!("thread-typing-v1")), "features: {features:?}");
    assert_eq!(k2_daemon::boot_status::FEATURE_THREAD_TYPING, "thread-typing-v1");
}
