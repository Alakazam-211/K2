//! Access changes reach open sockets (`prd-live-access-changes-v1` S1a).
//!
//! Real daemon, real routes, real sockets, headless. Every socket, every kind
//! of loss: the frame stops and the socket closes. Beside each loss, a person
//! whose access did NOT change stays connected and keeps getting frames.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::time::Duration;

use app_fixture::*;
use futures_util::{SinkExt, StreamExt};
use k2_core::db::schema::WorkspaceSession;
use k2_daemon::session_events::{self, SessionEvent};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

const PW: &str = "s3cret-horse-staple";
const LOGIN_PW: &str = "correct-horse-battery-staple-9";
/// A socket's idle close is a 5 s tick away; a little room for a busy node.
const CLOSE_WITHIN_MS: u64 = 7_000;

type Sock = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, Default)]
struct Seen {
    frames: Vec<String>,
    close: Option<u16>,
    ended: bool,
}

impl Seen {
    fn has(&self, needle: &str) -> bool {
        self.frames.iter().any(|f| f.contains(needle))
    }
}

/// What arrives within `ms`, or until the socket ends.
async fn watch(sock: &mut Sock, ms: u64) -> Seen {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    let mut out = Seen::default();
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, sock.next()).await {
            Err(_) => return out,
            Ok(None) | Ok(Some(Err(_))) => {
                out.ended = true;
                return out;
            }
            Ok(Some(Ok(Message::Text(t)))) => out.frames.push(t.to_string()),
            Ok(Some(Ok(Message::Close(c)))) => {
                out.close = c.map(|c| u16::from(c.code));
                out.ended = true;
                return out;
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

async fn open(port: u16, pq: &str) -> Sock {
    let url = format!("ws://127.0.0.1:{port}{pq}");
    let (s, _) = tokio_tungstenite::connect_async(&url).await.unwrap_or_else(|e| panic!("open {pq}: {e}"));
    s
}

fn pin_chat(ws: &Ws) -> String {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    conv
}

async fn say(port: u16, ws: &Ws, text: &str) {
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": ws.handle, "from": "k2", "text": text })).await;
    assert_eq!(r.status, 200, "post: {}", r.body);
}

fn ticket(room: &str, id: &str) {
    session_events::emit(SessionEvent::TicketChanged {
        project_id: room.to_string(),
        id: id.to_string(),
        change: "answered".into(),
        status: "answered".into(),
        via: None,
        has_brief: false,
    })
    .expect("emit");
}

fn file_changed(ws: &Ws, rel: &str) {
    let root = ws.path.to_string_lossy().to_string();
    session_events::emit(SessionEvent::FsChanged { workspace_path: root.clone(), paths: vec![format!("{root}/{rel}")] }).expect("emit");
}

async fn owner_ok(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

const ALL_CAPS: &[&str] = &["thread:read", "thread:post", "activity:read", "tickets:read", "files:read", "room:members", "heartbeats:read"];

/// One guest on its own role in `ws`. Returns `(role, pass)`.
async fn guest_in(port: u16, ws: &Ws, username: &str, caps: &[&str]) -> (String, String) {
    let role = format!("role-{username}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    owner_ok(port, "/cli/skin/users", json!({ "username": username })).await;
    k2_daemon::test_harness::guest_with_own_password(username, PW);
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.expect("join");
    assert_eq!(r.status, 200, "guest login: {}", r.body);
    (role, r.json()["token"].as_str().expect("token").to_string())
}

// ── App uninstall ────────────────────────────────────────────────────

/// `k2 app uninstall --drop-access` says it drops access: the App's login
/// sessions end. Another App's session, and a box login, are untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uninstalling_an_app_ends_its_guests_sessions_and_no_one_elses() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "uninst");
    let (role, box_login) = guest_in(port, &ws, "ulla", ALL_CAPS).await;
    let (other_role, _) = guest_in(port, &ws, "ugo", ALL_CAPS).await;

    let new_app = |app: &str| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let svc = k2_core::published_services::insert(&conn, &ws.id, app, k2_core::published_services::CMD_APP_SENTINEL, &ws.path.to_string_lossy(), 0, "local", "stopped", "cmd", "")
            .expect("service")
            .id;
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp {
                app_id: app,
                service_id: &svc,
                seed_id: None,
                owner_workspace: &ws.id,
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .expect("insert app");
        svc
    };
    let grant = |user: &str, role: &str, svc: &str| {
        let pid = k2_core::skin::host_login_detail(user).expect("guest").principal_id;
        k2_core::skin::create_grant(&k2_core::skin::SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: pid,
            kind: "app".into(),
            target_id: svc.to_string(),
            role: Some(role.to_string()),
            scope: None,
            created_by: Some("owner".into()),
        })
        .expect("app grant");
        k2_core::skin::create_app_session_token(user, svc).expect("App session").raw
    };
    let gone_svc = new_app("uninstgone");
    let kept_svc = new_app("uninstkept");
    let gone_session = grant("ulla", &role, &gone_svc);
    let kept_session = grant("ugo", &other_role, &kept_svc);
    for s in [&gone_session, &kept_session, &box_login] {
        let r = get(port, "/cli/skin/me", s, "x=1").await;
        assert_eq!(r.status, 200, "before: {}", r.body);
    }

    owner_ok(port, "/cli/app/uninstall", json!({ "app": "uninstgone", "dropAccess": true })).await;

    let after = get(port, "/cli/skin/me", &gone_session, "x=1").await;
    assert_eq!(after.status, 401, "the removed App's session must end: {}", after.body);
    let thread = get(port, "/cli/thread", &gone_session, &format!("addr={}", ws.handle)).await;
    assert_eq!(thread.status, 401, "and every door with it: {}", thread.body);
    // Control: nobody else's access changed.
    for (who, s) in [("the other App's session", &kept_session), ("the same person's box login", &box_login)] {
        let r = get(port, "/cli/skin/me", s, "x=1").await;
        assert_eq!(r.status, 200, "{who} keeps working: {}", r.body);
    }
}

// ── Connect logins ───────────────────────────────────────────────────

async fn connect_login(port: u16, username: &str) -> String {
    owner_ok(port, "/cli/users/add", json!({ "username": username, "password": LOGIN_PW, "mustChangePassword": false })).await;
    let b = json!({ "username": username, "password": LOGIN_PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/auth/login", Some(b))).await.expect("join");
    assert_eq!(r.status, 200, "login: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

/// A login's four read sockets on one room.
struct LoginSockets {
    overlay: Sock,
    stream: Sock,
    activity: Sock,
    files: Sock,
}

async fn login_sockets(port: u16, ws: &Ws, conv: &str, token: &str) -> LoginSockets {
    let q = enc(token);
    LoginSockets {
        overlay: open(port, &format!("/cli/overlay/events?conversation={conv}&token={q}")).await,
        stream: open(port, &format!("/cli/thread/stream?rooms=all&token={q}")).await,
        activity: open(port, &format!("/cli/activity/events?workspace={}&token={q}", ws.handle)).await,
        files: open(port, &format!("/cli/fs/events?workspace={}&token={q}", ws.handle)).await,
    }
}

fn cause_every_frame(port: u16, ws: &Ws, marker: &str) -> impl std::future::Future<Output = ()> {
    k2_daemon::overlay_ws::publish(k2_daemon::overlay_ws::OverlayFrame {
        collection: "chatter".into(),
        seq: 1,
        id: format!("{marker}-HOSTWIDE"),
        doc: None,
        activity: None,
        conversation_id: None,
        rev: 0,
    });
    ticket(&ws.id, &format!("{marker}-TICKET"));
    file_changed(ws, &format!("{marker}-file.txt"));
    let (handle, marker) = (ws.handle.clone(), marker.to_string());
    async move {
        let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": handle, "from": "k2", "text": format!("{marker}-THREAD") })).await;
        assert_eq!(r.status, 200, "post: {}", r.body);
    }
}

/// A kicked login stays on the box; a removed one doesn't. Either way every
/// socket stops at once and closes 4003, with no frame caused or with one. The
/// owner, and another login, see no change at all.
async fn login_loses_access(remove: bool) {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    // The database is one per test process: each test's names are its own.
    let tag = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
    let ws = seed_ws(&env, &format!("conn{tag}"));
    let conv = pin_chat(&ws);
    let (gone_name, idle_name) = (format!("gone{tag}"), format!("idle{tag}"));
    let gone = connect_login(port, &gone_name).await;
    let idle = connect_login(port, &idle_name).await;
    let kept = connect_login(port, &format!("keep{tag}")).await;
    let mut gone_s = login_sockets(port, &ws, &conv, &gone).await;
    let mut idle_s = login_sockets(port, &ws, &conv, &idle).await;
    let mut kept_s = login_sockets(port, &ws, &conv, &kept).await;
    let mut owner_s = login_sockets(port, &ws, &conv, OWNER).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The rig: everyone hears the frames while access lasts.
    cause_every_frame(port, &ws, "BEFORE").await;
    let (a, b, c, e) = tokio::join!(watch(&mut gone_s.overlay, 1_500), watch(&mut gone_s.stream, 1_500), watch(&mut gone_s.activity, 1_500), watch(&mut gone_s.files, 1_500));
    assert!(a.has("BEFORE-THREAD") && a.has("BEFORE-HOSTWIDE"), "rig: overlay {a:?}");
    assert!(b.has("BEFORE-THREAD"), "rig: stream {b:?}");
    assert!(c.has("BEFORE-TICKET"), "rig: activity {c:?}");
    assert!(e.has("BEFORE-file.txt"), "rig: files {e:?}");

    let path = if remove { "/cli/users/remove" } else { "/cli/presence/kick" };
    let t0 = std::time::Instant::now();
    for u in [&gone_name, &idle_name] {
        owner_ok(port, path, json!({ "username": u })).await;
    }
    let denied = get(port, "/cli/projects", &gone, "x=1").await;
    assert_ne!(denied.status, 200, "rig: the door refuses the token: {}", denied.body);

    // A frame right after: nothing reaches the socket that lost access.
    cause_every_frame(port, &ws, "AFTER").await;
    let (a, b, c, e) = tokio::join!(
        watch(&mut gone_s.overlay, CLOSE_WITHIN_MS),
        watch(&mut gone_s.stream, CLOSE_WITHIN_MS),
        watch(&mut gone_s.activity, CLOSE_WITHIN_MS),
        watch(&mut gone_s.files, CLOSE_WITHIN_MS)
    );
    for (what, seen) in [("overlay", &a), ("stream", &b), ("activity", &c), ("files", &e)] {
        for marker in ["AFTER-THREAD", "AFTER-HOSTWIDE", "AFTER-TICKET", "AFTER-file.txt"] {
            assert!(!seen.has(marker), "{what}: {marker} reached a login that lost access: {seen:?}");
        }
        assert_eq!(seen.close, Some(4003), "{what} closes 4003: {seen:?}");
    }
    // Sockets with nothing to send close too. By now the tick has passed.
    let (a, b, c, e) = tokio::join!(watch(&mut idle_s.overlay, 2_000), watch(&mut idle_s.stream, 2_000), watch(&mut idle_s.activity, 2_000), watch(&mut idle_s.files, 2_000));
    for (what, seen) in [("idle overlay", &a), ("idle stream", &b), ("idle activity", &c), ("idle files", &e)] {
        assert_eq!(seen.close, Some(4003), "{what}: {seen:?}");
    }
    assert!(t0.elapsed() < Duration::from_secs(20), "closes came late: {:?}", t0.elapsed());

    // Control: their access didn't change, so they saw every frame and no close.
    for (who, s) in [("another login", &mut kept_s), ("the owner token", &mut owner_s)] {
        let (a, b, c, e) = tokio::join!(watch(&mut s.overlay, 1_000), watch(&mut s.stream, 1_000), watch(&mut s.activity, 1_000), watch(&mut s.files, 1_000));
        assert!(a.has("AFTER-THREAD") && a.has("AFTER-HOSTWIDE") && !a.ended, "{who} overlay: {a:?}");
        assert!(b.has("AFTER-THREAD") && !b.ended, "{who} stream: {b:?}");
        assert!(c.has("AFTER-TICKET") && !c.ended, "{who} activity: {c:?}");
        assert!(e.has("AFTER-file.txt") && !e.ended, "{who} files: {e:?}");
    }
    cause_every_frame(port, &ws, "LATER").await;
    let a = watch(&mut kept_s.overlay, 1_000).await;
    assert!(a.has("LATER-THREAD") && !a.ended, "another login, past the tick: {a:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kicked_login_loses_its_open_sockets() {
    login_loses_access(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removed_login_loses_its_open_sockets() {
    login_loses_access(true).await;
}

// ── Numbered frames: nothing published after the loss returned ───────

/// The highest `RACE-<n>-` in these frames.
fn highest_race_id(frames: &[String]) -> Option<u64> {
    let mut best: Option<u64> = None;
    for f in frames {
        for part in f.split("RACE-").skip(1) {
            let n: String = part.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(v) = n.parse::<u64>() {
                best = Some(best.map_or(v, |b| b.max(v)));
            }
        }
    }
    best
}

/// Publish numbered frames as fast as a socket takes them, lose access, and
/// check the socket: the counter moves before each publish, so its value when
/// the loss returned is an upper bound on what may have left under old access.
async fn race(name: &str, mut sock: Sock, emit: std::sync::Arc<dyn Fn(u64) + Send + Sync>, lose_now: impl std::future::Future<Output = ()>, close: u16) {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    let counter = std::sync::Arc::new(AtomicU64::new(0));
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let (c2, s2, e2) = (counter.clone(), stop.clone(), emit.clone());
    let publisher = std::thread::spawn(move || {
        while !s2.load(Ordering::SeqCst) {
            let i = c2.fetch_add(1, Ordering::SeqCst) + 1;
            e2(i);
            std::thread::sleep(Duration::from_micros(300));
        }
    });
    let before = watch(&mut sock, 250).await;
    lose_now.await;
    let at_return = counter.load(Ordering::SeqCst);
    let after = watch(&mut sock, CLOSE_WITHIN_MS).await;
    stop.store(true, Ordering::SeqCst);
    publisher.join().expect("publisher");
    assert!(highest_race_id(&before.frames).is_some(), "rig {name}: frames flowed before the loss");
    let last = highest_race_id(&after.frames);
    assert!(last.is_none_or(|l| l <= at_return), "{name}: frame {last:?} was published after the loss returned at {at_return}");
    assert_eq!(after.close, Some(close), "{name}: {:?}", after.close);
}

fn numbered_thread_frame(conv: &str, i: u64) -> k2_daemon::overlay_ws::OverlayFrame {
    k2_daemon::overlay_ws::OverlayFrame {
        collection: "thread".into(),
        seq: i as i64,
        id: format!("RACE-{i}-"),
        doc: None,
        activity: None,
        conversation_id: Some(conv.to_string()),
        rev: 0,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kicked_logins_overlay_race() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    for round in 0..3 {
        let tag = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
        let ws = seed_ws(&env, &format!("raceov{tag}"));
        let conv = pin_chat(&ws);
        let user = format!("race{tag}");
        let token = connect_login(port, &user).await;
        let s = open(port, &format!("/cli/overlay/events?conversation={conv}&token={}", enc(&token))).await;
        let c2 = conv.clone();
        race(
            &format!("login overlay #{round}"),
            s,
            std::sync::Arc::new(move |i| k2_daemon::overlay_ws::publish(numbered_thread_frame(&c2, i))),
            async move {
                owner_ok(port, "/cli/presence/kick", json!({ "username": user })).await;
            },
            4003,
        )
        .await;
    }
}

/// The stream's other doors for a login: an owner turn frame and a subscribe
/// answer right after a kick, before any tick.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kicked_logins_stream_sends_no_turn_and_no_room_list() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    for round in 0..3 {
        let tag = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
        let ws = seed_ws(&env, &format!("racest{tag}"));
        let conv = pin_chat(&ws);
        let (turn_user, sub_user) = (format!("turn{tag}"), format!("subs{tag}"));
        let turn_token = connect_login(port, &turn_user).await;
        let sub_token = connect_login(port, &sub_user).await;
        let mut turn = open(port, &format!("/cli/thread/stream?rooms=all&ear=1&token={}", enc(&turn_token))).await;
        let mut subs = open(port, &format!("/cli/thread/stream?rooms=all&token={}", enc(&sub_token))).await;
        let (a, b) = tokio::join!(watch(&mut turn, 300), watch(&mut subs, 300));
        assert!(a.has("inbox_subscribed") && b.has("inbox_subscribed"), "rig #{round}: {a:?} {b:?}");

        owner_ok(port, "/cli/presence/kick", json!({ "username": turn_user })).await;
        owner_ok(port, "/cli/presence/kick", json!({ "username": sub_user })).await;
        k2_daemon::overlay_ws::publish_activity(k2_daemon::overlay_ws::OverlayFrame {
            collection: "activity".into(),
            seq: 0,
            id: "turn".into(),
            doc: None,
            activity: Some(json!({ "marker": "TURN-AFTER-KICK" })),
            conversation_id: Some(conv.clone()),
            rev: 0,
        });
        subs.send(Message::Text(json!({ "t": "inbox", "all": true }).to_string().into())).await.expect("send");
        let (a, b) = tokio::join!(watch(&mut turn, 2_000), watch(&mut subs, 2_000));
        assert!(!a.has("TURN-AFTER-KICK"), "#{round}: a turn frame after the kick: {a:?}");
        assert_eq!(a.close, Some(4003), "#{round}: {a:?}");
        assert!(!b.has("inbox_subscribed"), "#{round}: a room list after the kick: {b:?}");
        assert_eq!(b.close, Some(4003), "#{round}: {b:?}");
    }
}

/// Numbered edits, as fast as they're acked: nothing edited after the kick
/// returned reaches the kicked login's doc socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kicked_logins_doc_socket_race() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    tokio::task::spawn_blocking(k2_daemon::docs::start).await.expect("docs start");
    let on: std::collections::BTreeMap<String, bool> = [("markdown".to_string(), true), ("text".to_string(), true)].into_iter().collect();
    tokio::task::spawn_blocking(move || k2_daemon::docs::cores::sync_multiplayer_cores(&on, "owner-token")).await.expect("flip");
    let tag = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
    let ws = seed_ws(&env, &format!("docrace{tag}"));
    let doc = ws.path.join("race.md");
    std::fs::write(&doc, "hello\n").expect("doc");
    let doc = doc.to_string_lossy().to_string();
    let url = |tok: &str| format!("/cli/docs/live?token={}&doc={}&root=source", enc(tok), enc(&doc));
    let user = format!("docr{tag}");
    let token = connect_login(port, &user).await;
    let mut owner_w = open(port, &url(OWNER)).await;
    let mut victim = open(port, &url(&token)).await;
    let mut base = doc_until(&mut owner_w, 10, "welcome").await.expect("welcome")["version"].as_u64().expect("version");
    doc_until(&mut victim, 10, "welcome").await.expect("welcome");

    let counter = std::sync::Arc::new(AtomicU64::new(0));
    let c2 = counter.clone();
    let editor = tokio::spawn(async move {
        for op in 1.. {
            let i = c2.fetch_add(1, Ordering::SeqCst) + 1;
            if owner_w.send(doc_edit(op, base, &format!("RACE-{i}- "))).await.is_err() {
                return;
            }
            let Some(ack) = doc_until(&mut owner_w, 5, "ack").await else { return };
            base = ack["version"].as_u64().expect("version");
        }
    });
    let before = watch(&mut victim, 400).await;
    assert!(highest_race_id(&before.frames).is_some(), "rig: the doc socket got changes before the kick: {before:?}");
    owner_ok(port, "/cli/presence/kick", json!({ "username": user })).await;
    let at_return = counter.load(Ordering::SeqCst);
    let after = watch(&mut victim, CLOSE_WITHIN_MS).await;
    editor.abort();
    let last = highest_race_id(&after.frames);
    assert!(last.is_none_or(|l| l <= at_return), "edit {last:?} came after the kick returned at {at_return}");
    assert_eq!(after.close, Some(4003), "{:?}", after.close);
    assert!(after.has("\"closed\""), "the socket's own closed frame first: {:?}", after.close);
}

/// A shell ends only on a real refusal. A database error is none, whether the
/// read says so or turns it into "no": the shell waits for the next tick.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_database_error_never_ends_a_shell() {
    use k2_daemon::apps::exec::ShellAccess;
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let tag = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
    let ws = seed_ws(&env, &format!("shdb{tag}"));
    let app = format!("shdb{tag}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let svc = k2_core::published_services::insert(&conn, &ws.id, &app, k2_core::published_services::CMD_APP_SENTINEL, &ws.path.to_string_lossy(), 0, "local", "stopped", "cmd", "")
            .expect("service")
            .id;
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp { app_id: &app, service_id: &svc, seed_id: None, owner_workspace: &ws.id, isolation: "container", memory_limit_bytes: None, front_port: 0, created_by: "owner" },
        )
        .expect("app");
    }
    allow_build_only(&ws, true);
    let pass = passport(&ws);
    let owner = ShellAccess::new(&format!("token={OWNER}"), None, OWNER);
    let agent = ShellAccess::new(&format!("token={pass}"), None, OWNER);
    let stranger = ShellAccess::new("token=not-a-real-token", None, OWNER);
    assert!(owner.allowed_now(&app) && agent.allowed_now(&app), "rig: both may exec");
    assert!(!stranger.allowed_now(&app), "rig: a bad token is a real refusal");

    let rename = |from: &str, to: &str| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute_batch(&format!("ALTER TABLE {from} RENAME TO {to};")).expect("rename");
    };
    // The App's own row can't be read: the read says it failed.
    rename("apps", "apps_gone");
    assert!(owner.allowed_now(&app), "a read error isn't a refusal");
    assert!(agent.allowed_now(&app), "a read error isn't a refusal");
    rename("apps_gone", "apps");
    // The agent's switches can't be read: those reads say "off".
    rename("projects", "projects_gone");
    assert!(agent.allowed_now(&app), "an error that reads as \"off\" isn't a refusal");
    rename("projects_gone", "projects");

    assert!(agent.allowed_now(&app), "back to normal");
    allow_build_only(&ws, false);
    assert!(!agent.allowed_now(&app), "the switch off is a real refusal");
    drop(d);
}

// ── Live docs ────────────────────────────────────────────────────────

async fn doc_until(ws: &mut Sock, secs: u64, ty: &str) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, ws.next()).await {
            Err(_) | Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => return None,
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: Value = serde_json::from_str(&t).expect("json");
                if v["type"] == ty {
                    return Some(v);
                }
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

fn doc_edit(op: u64, base: u64, text: &str) -> Message {
    Message::Text(json!({ "type": "edit", "op": op, "root": "source", "base": base, "hunks": [{ "start": 0, "end": 0, "insert": text }] }).to_string().into())
}

/// A login that loses its session keeps nothing on a Live doc: no writes, no
/// reads, a `closed` frame first (the socket's own rule), then 4003. Its rights
/// come from the login, so a kick alone used to change nothing here.
async fn doc_login_loses_access(remove: bool) {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    tokio::task::spawn_blocking(k2_daemon::docs::start).await.expect("docs start");
    assert!(k2_daemon::docs::live_ready(), "rig: live docs started");
    let on: std::collections::BTreeMap<String, bool> = [("markdown".to_string(), true), ("text".to_string(), true)].into_iter().collect();
    tokio::task::spawn_blocking(move || k2_daemon::docs::cores::sync_multiplayer_cores(&on, "owner-token")).await.expect("flip");
    let tag = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
    let ws = seed_ws(&env, &format!("doc{tag}"));
    let doc = ws.path.join("notes.md");
    std::fs::write(&doc, "hello\n").expect("notes");
    let doc = doc.to_string_lossy().to_string();
    let url = |tok: &str| format!("/cli/docs/live?token={}&doc={}&root=source", enc(tok), enc(&doc));

    let (pat_name, sam_name) = (format!("pat{tag}"), format!("sam{tag}"));
    let pat = connect_login(port, &pat_name).await;
    let sam = connect_login(port, &sam_name).await;
    let kim = connect_login(port, &format!("kim{tag}")).await;
    let mut owner_w = open(port, &url(OWNER)).await;
    let mut pat_w = open(port, &url(&pat)).await;
    let mut sam_w = open(port, &url(&sam)).await;
    let mut kim_w = open(port, &url(&kim)).await;
    let v0 = doc_until(&mut owner_w, 10, "welcome").await.expect("welcome")["version"].as_u64().expect("version");
    for s in [&mut pat_w, &mut sam_w, &mut kim_w] {
        doc_until(s, 10, "welcome").await.expect("welcome");
    }

    let path = if remove { "/cli/users/remove" } else { "/cli/presence/kick" };
    owner_ok(port, path, json!({ "username": pat_name })).await;
    owner_ok(port, path, json!({ "username": sam_name })).await;

    // pat writes straight after, before any tick: refused, and it says why.
    pat_w.send(doc_edit(1, v0, "KICKED-WROTE ")).await.expect("send");
    let seen = watch(&mut pat_w, 3_000).await;
    assert_eq!(seen.close, Some(4003), "the doc socket closes 4003: {seen:?}");
    let want = if remove { "rights_lost" } else { "kicked" };
    assert!(seen.has("\"closed\"") && seen.has(want), "a closed frame first, reason {want}: {seen:?}");
    assert!(!seen.has("\"ack\""), "no ack for a write after the token ended: {seen:?}");

    // sam only watches: closed too, and the owner's next edit never reaches it.
    owner_w.send(doc_edit(1, v0, "OWNER-AFTER ")).await.expect("send");
    let ack = doc_until(&mut owner_w, 5, "ack").await.expect("the owner's edit is acked");
    let v1 = ack["version"].as_u64().expect("version");
    let seen = watch(&mut sam_w, CLOSE_WITHIN_MS).await;
    assert_eq!(seen.close, Some(4003), "{seen:?}");
    assert!(!seen.has("OWNER-AFTER"), "no reads after the token ended: {seen:?}");

    // Control: kim's access didn't change. She reads the owner's edit and writes.
    let change = doc_until(&mut kim_w, 5, "change").await.expect("another login still reads");
    assert!(change.to_string().contains("OWNER-AFTER"), "{change}");
    tokio::time::sleep(k2_daemon::socket_access::TICK + Duration::from_millis(500)).await;
    kim_w.send(doc_edit(1, v1, "KIM-WROTE ")).await.expect("send");
    doc_until(&mut kim_w, 5, "ack").await.expect("another login still writes, past the tick");

    let r = get(port, "/cli/docs/read", OWNER, &format!("path={}", enc(&doc))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!r.body.contains("KICKED-WROTE"), "the write never landed: {}", r.body);
    assert!(r.body.contains("KIM-WROTE"), "{}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kicked_login_cannot_read_or_write_a_live_doc() {
    doc_login_loses_access(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removed_login_cannot_read_or_write_a_live_doc() {
    doc_login_loses_access(true).await;
}

// ── App guests ───────────────────────────────────────────────────────

async fn project(port: u16, name: &str, members: &[&Ws]) -> String {
    let g = owner_ok(port, "/cli/project-group/create", json!({ "name": name })).await;
    let id = g["id"].as_str().expect("group id").to_string();
    for m in members {
        owner_ok(port, "/cli/project-group/add-member", json!({ "group": id, "workspace": m.id })).await;
    }
    id
}

/// A guest's five sockets on one room, plus the same overlay with the ear on.
struct GuestSockets {
    overlay: Sock,
    overlay_ear: Sock,
    activity: Sock,
    files: Sock,
    stream: Sock,
    project: Sock,
}

async fn guest_sockets(port: u16, ws: &Ws, conv: &str, group: &str, pass: &str) -> GuestSockets {
    let q = enc(pass);
    GuestSockets {
        overlay: open(port, &format!("/cli/overlay/events?conversation={conv}&token={q}")).await,
        overlay_ear: open(port, &format!("/cli/overlay/events?conversation={conv}&ear=1&token={q}")).await,
        activity: open(port, &format!("/cli/activity/events?workspace={}&token={q}", ws.handle)).await,
        files: open(port, &format!("/cli/fs/events?workspace={}&token={q}", ws.handle)).await,
        stream: open(port, &format!("/cli/thread/stream?rooms=all&token={q}")).await,
        project: open(port, &format!("/cli/skin/projects/events?group={group}&token={q}")).await,
    }
}

/// A frame for each socket.
async fn cause_guest_frames(port: u16, ws: &Ws, group: &str, marker: &str) {
    say(port, ws, &format!("{marker}-THREAD")).await;
    ticket(&ws.id, &format!("{marker}-TICKET"));
    session_events::emit(SessionEvent::RoomMembersChanged { room: Some(ws.id.clone()) }).expect("emit");
    file_changed(ws, &format!("{marker}-file.txt"));
    owner_ok(port, "/cli/project-group/msg", json!({ "group": group, "body": format!("{marker}-PROJECT") })).await;
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Loss {
    SessionRevoked,
    RoomTaken,
    UserDeleted,
}

impl Loss {
    /// The close each room-scoped socket owes: the pass gone, or only the room.
    fn close(self) -> u16 {
        match self {
            Loss::SessionRevoked | Loss::UserDeleted => 4003,
            Loss::RoomTaken => 4013,
        }
    }
}

async fn lose(port: u16, username: &str, pass: &str, how: Loss) {
    match how {
        Loss::SessionRevoked => {
            let id = k2_core::skin::resolve_skin_token(pass).expect("live pass").id;
            assert!(k2_core::skin::revoke_token(&id).expect("revoke"), "revoked");
        }
        Loss::RoomTaken => {
            owner_ok(port, "/cli/skin/roles/unassign", json!({ "username": username })).await;
        }
        Loss::UserDeleted => {
            owner_ok(port, "/cli/skin/users/remove", json!({ "username": username })).await;
        }
    }
}

/// Each socket, each kind of loss: no frame after the commit and the right
/// close, whether a frame was caused or the socket sat idle. Two guests with
/// the same rights who lost nothing keep every frame and stay open.
async fn guest_loses(how: Loss) {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
    let ws = seed_ws(&env, &format!("loud{tag}"));
    let quiet = seed_ws(&env, &format!("quiet{tag}"));
    let conv = pin_chat(&ws);
    let quiet_conv = pin_chat(&quiet);
    let (gone_name, idle_name, kept_name) = (format!("gone{tag}"), format!("idle{tag}"), format!("kept{tag}"));
    let (_, gone) = guest_in(port, &ws, &gone_name, ALL_CAPS).await;
    let (_, idle) = guest_in(port, &quiet, &idle_name, ALL_CAPS).await;
    let (_, kept) = guest_in(port, &ws, &kept_name, ALL_CAPS).await;
    let group = project(port, &format!("loud-{tag}"), &[&ws]).await;
    let quiet_group = project(port, &format!("quiet-{tag}"), &[&quiet]).await;
    let mut gone_s = guest_sockets(port, &ws, &conv, &group, &gone).await;
    let mut idle_s = guest_sockets(port, &quiet, &quiet_conv, &quiet_group, &idle).await;
    let mut kept_s = guest_sockets(port, &ws, &conv, &group, &kept).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The rig: frames reach everyone in the room while access lasts.
    cause_guest_frames(port, &ws, &group, "BEFORE").await;
    let (a, b, c, e, f) = tokio::join!(
        watch(&mut gone_s.overlay, 1_500),
        watch(&mut gone_s.activity, 1_500),
        watch(&mut gone_s.files, 1_500),
        watch(&mut gone_s.stream, 1_500),
        watch(&mut gone_s.project, 1_500)
    );
    assert!(a.has("BEFORE-THREAD"), "rig: overlay {a:?}");
    assert!(b.has("BEFORE-TICKET") && b.has("room_members_changed"), "rig: activity {b:?}");
    assert!(c.has("BEFORE-file.txt"), "rig: files {c:?}");
    assert!(e.has("BEFORE-THREAD"), "rig: stream {e:?}");
    assert!(f.has("BEFORE-PROJECT"), "rig: project {f:?}");

    lose(port, &gone_name, &gone, how).await;
    lose(port, &idle_name, &idle, how).await;
    let door = get(port, "/cli/skin/me", &gone, "x=1").await;
    match how {
        Loss::RoomTaken => assert_eq!((door.status, door.json()["rooms"].clone()), (200, json!([])), "rig: the door says no rooms: {}", door.body),
        _ => assert_eq!(door.status, 401, "rig: the door refuses the pass: {}", door.body),
    }

    // Frames right after the commit: none reach the sockets that lost access.
    cause_guest_frames(port, &ws, &group, "AFTER").await;
    let (a, ae, b, c, f) = tokio::join!(
        watch(&mut gone_s.overlay, 2_000),
        watch(&mut gone_s.overlay_ear, CLOSE_WITHIN_MS),
        watch(&mut gone_s.activity, 2_000),
        watch(&mut gone_s.files, 2_000),
        watch(&mut gone_s.project, 2_000)
    );
    for (what, seen) in [("overlay", &a), ("overlay with the ear", &ae), ("activity", &b), ("files", &c), ("project chat", &f)] {
        for marker in ["AFTER-THREAD", "AFTER-TICKET", "AFTER-file.txt", "AFTER-PROJECT", "room_members_changed"] {
            assert!(!seen.has(marker), "{how:?} {what}: {marker} went out after the loss: {seen:?}");
        }
        assert_eq!(seen.close, Some(how.close()), "{how:?} {what} closes: {seen:?}");
    }
    // The room stream: a dead pass closes; a lost room shrinks its set (one
    // socket across rooms), with no frame from that room.
    let e = watch(&mut gone_s.stream, 2_500).await;
    assert!(!e.has("AFTER-THREAD"), "{how:?} stream: {e:?}");
    match how {
        Loss::RoomTaken => assert!(!e.ended && e.has("inbox_subscribed"), "the set shrinks and the socket stays: {e:?}"),
        _ => assert_eq!(e.close, Some(4003), "{how:?} stream: {e:?}"),
    }

    // Idle: no frame was caused in its room, and the tick closes it anyway.
    let (a, ae, b, c, f) = tokio::join!(
        watch(&mut idle_s.overlay, CLOSE_WITHIN_MS),
        watch(&mut idle_s.overlay_ear, CLOSE_WITHIN_MS),
        watch(&mut idle_s.activity, CLOSE_WITHIN_MS),
        watch(&mut idle_s.files, CLOSE_WITHIN_MS),
        watch(&mut idle_s.project, CLOSE_WITHIN_MS)
    );
    for (what, seen) in [("idle overlay", &a), ("idle overlay with the ear", &ae), ("idle activity", &b), ("idle files", &c), ("idle project chat", &f)] {
        assert_eq!(seen.close, Some(how.close()), "{how:?} {what}: {seen:?}");
    }

    // Control: their access didn't change, past several ticks by now.
    let (a, ae, b, c, e, f) = tokio::join!(
        watch(&mut kept_s.overlay, 1_000),
        watch(&mut kept_s.overlay_ear, 1_000),
        watch(&mut kept_s.activity, 1_000),
        watch(&mut kept_s.files, 1_000),
        watch(&mut kept_s.stream, 1_000),
        watch(&mut kept_s.project, 1_000)
    );
    assert!(a.has("AFTER-THREAD") && !a.ended, "kept overlay: {a:?}");
    assert!(ae.has("AFTER-THREAD") && !ae.ended, "kept overlay with the ear: {ae:?}");
    assert!(b.has("AFTER-TICKET") && !b.ended, "kept activity: {b:?}");
    assert!(c.has("AFTER-file.txt") && !c.ended, "kept files: {c:?}");
    assert!(e.has("AFTER-THREAD") && !e.ended, "kept stream: {e:?}");
    assert!(f.has("AFTER-PROJECT") && !f.ended, "kept project chat: {f:?}");
    cause_guest_frames(port, &ws, &group, "LATER").await;
    let (a, b, c, f) = tokio::join!(watch(&mut kept_s.overlay, 1_500), watch(&mut kept_s.activity, 1_500), watch(&mut kept_s.files, 1_500), watch(&mut kept_s.project, 1_500));
    assert!(a.has("LATER-THREAD") && !a.ended, "{a:?}");
    assert!(b.has("LATER-TICKET") && !b.ended, "{b:?}");
    assert!(c.has("LATER-file.txt") && !c.ended, "{c:?}");
    assert!(f.has("LATER-PROJECT") && !f.ended, "{f:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revoked_guest_loses_every_open_socket() {
    guest_loses(Loss::SessionRevoked).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_who_loses_the_room_loses_every_room_socket() {
    guest_loses(Loss::RoomTaken).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deleted_guest_loses_every_open_socket() {
    guest_loses(Loss::UserDeleted).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guests_overlay_activity_and_files_race() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    // The publisher outlives the socket it races: a bus with nobody left on it
    // refuses the emit.
    let _keep_the_bus_open = session_events::subscribe();
    let new_guest = |caps: &'static [&'static str]| {
        let tag = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
        let ws = seed_ws(&env, &format!("raceg{tag}"));
        let conv = pin_chat(&ws);
        async move {
            let user = format!("rg{tag}");
            let (_, pass) = guest_in(port, &ws, &user, caps).await;
            (ws, conv, user, pass)
        }
    };
    let revoke = |pass: String| async move {
        tokio::task::spawn_blocking(move || {
            let id = k2_core::skin::resolve_skin_token(&pass).expect("live").id;
            assert!(k2_core::skin::revoke_token(&id).expect("revoke"));
        })
        .await
        .expect("join");
    };
    for round in 0..3 {
        let (_ws, conv, _user, pass) = new_guest(ALL_CAPS).await;
        let s = open(port, &format!("/cli/overlay/events?conversation={conv}&token={}", enc(&pass))).await;
        let c2 = conv.clone();
        race(&format!("guest overlay revoked #{round}"), s, std::sync::Arc::new(move |i| k2_daemon::overlay_ws::publish(numbered_thread_frame(&c2, i))), revoke(pass), 4003).await;

        let (_ws, conv, user, pass) = new_guest(ALL_CAPS).await;
        let s = open(port, &format!("/cli/overlay/events?conversation={conv}&token={}", enc(&pass))).await;
        let c2 = conv.clone();
        race(
            &format!("guest overlay lost the room #{round}"),
            s,
            std::sync::Arc::new(move |i| k2_daemon::overlay_ws::publish(numbered_thread_frame(&c2, i))),
            async move {
                owner_ok(port, "/cli/skin/roles/unassign", json!({ "username": user })).await;
            },
            4013,
        )
        .await;
    }
    let (ws, _conv, _user, pass) = new_guest(ALL_CAPS).await;
    let s = open(port, &format!("/cli/activity/events?workspace={}&token={}", ws.handle, enc(&pass))).await;
    let room = ws.id.clone();
    race("guest activity revoked", s, std::sync::Arc::new(move |i| ticket(&room, &format!("RACE-{i}-"))), revoke(pass), 4003).await;
    let (ws, _conv, _user, pass) = new_guest(ALL_CAPS).await;
    let s = open(port, &format!("/cli/fs/events?workspace={}&token={}", ws.handle, enc(&pass))).await;
    let root = ws.path.to_string_lossy().to_string();
    race(
        "guest files revoked",
        s,
        std::sync::Arc::new(move |i| {
            session_events::emit(SessionEvent::FsChanged { workspace_path: root.clone(), paths: vec![format!("{root}/RACE-{i}-.txt")] }).expect("emit");
        }),
        revoke(pass),
        4003,
    )
    .await;
}

/// A database error is no answer about a pass: a plain close, never the 4003
/// that sends a person to sign in again. Nothing changed, so the door works
/// again once the database does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_database_error_is_not_a_dead_pass() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
    let ws = seed_ws(&env, &format!("dberr{tag}"));
    let conv = pin_chat(&ws);
    let (_, pass) = guest_in(port, &ws, &format!("dbe{tag}"), ALL_CAPS).await;
    let group = project(port, &format!("dberr-{tag}"), &[&ws]).await;
    let mut s = guest_sockets(port, &ws, &conv, &group, &pass).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // A second connection, so the daemon's one sees a read fail.
    let path = k2_core::paths::k2_home().join("skin.db");
    let other = rusqlite::Connection::open(&path).expect("second connection");
    other.execute_batch("ALTER TABLE principal_agent_ties RENAME TO principal_agent_ties_gone;").expect("rename");
    let (a, ae, b, c, e, f) = tokio::join!(
        watch(&mut s.overlay, CLOSE_WITHIN_MS),
        watch(&mut s.overlay_ear, CLOSE_WITHIN_MS),
        watch(&mut s.activity, CLOSE_WITHIN_MS),
        watch(&mut s.files, CLOSE_WITHIN_MS),
        watch(&mut s.stream, CLOSE_WITHIN_MS),
        watch(&mut s.project, CLOSE_WITHIN_MS)
    );
    other.execute_batch("ALTER TABLE principal_agent_ties_gone RENAME TO principal_agent_ties;").expect("restore");
    for (what, seen) in [("overlay", &a), ("overlay with the ear", &ae), ("activity", &b), ("files", &c), ("stream", &e), ("project chat", &f)] {
        assert!(seen.ended, "{what} closes on the tick: {seen:?}");
        assert_ne!(seen.close, Some(4003), "{what}: a read error says the pass is dead: {seen:?}");
        assert_ne!(seen.close, Some(4013), "{what}: a read error says access is lost: {seen:?}");
    }
    let door = get(port, "/cli/skin/me", &pass, "x=1").await;
    assert_eq!(door.status, 200, "the pass still works: {}", door.body);
    let _again = open(port, &format!("/cli/overlay/events?conversation={conv}&token={}", enc(&pass))).await;
}

/// Activity: one cap of several goes, that kind of frame stops and the rest
/// keep coming on the same socket. The last cap goes: 4013.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn losing_one_cap_stops_that_frame_and_keeps_the_socket() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "capsrow");
    let (role, pass) = guest_in(port, &ws, "cappy", &["activity:read", "tickets:read", "room:members"]).await;
    let mut s = open(port, &format!("/cli/activity/events?workspace={}&token={}", ws.handle, enc(&pass))).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    ticket(&ws.id, "BASE-TICKET");
    let before = watch(&mut s, 1_500).await;
    assert!(before.has("BASE-TICKET"), "rig: {before:?}");

    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["activity:read", "room:members"] })).await;
    ticket(&ws.id, "NOCAP-TICKET");
    session_events::emit(SessionEvent::RoomMembersChanged { room: Some(ws.id.clone()) }).expect("emit");
    let after = watch(&mut s, 2_000).await;
    assert!(!after.has("NOCAP-TICKET"), "no ticket frame without tickets:read: {after:?}");
    assert!(after.has("room_members_changed"), "the caps it keeps keep working: {after:?}");
    assert!(!after.ended, "the socket stays while a socket cap is left: {after:?}");

    // Past the tick: still open.
    let still = watch(&mut s, CLOSE_WITHIN_MS).await;
    assert!(!still.ended, "{still:?}");

    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read"] })).await;
    let end = watch(&mut s, CLOSE_WITHIN_MS).await;
    assert_eq!(end.close, Some(4013), "no socket cap left: {end:?}");
}

/// Files: a narrowed folder filters at once, a widened one's frames arrive
/// (not only after some other frame gets through), losing `files:read` is 4013.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_files_socket_follows_folder_changes_both_ways() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "foldrow");
    let (role, pass) = guest_in(port, &ws, "foldy", &["files:read", "thread:read"]).await;
    let set_paths = |read: Vec<&'static str>| {
        let role = role.clone();
        let handle = ws.handle.clone();
        async move {
            let r = post(port, "/cli/skin/roles/paths", OWNER, json!({ "name": role, "handle": handle, "read": read })).await;
            assert_eq!(r.status, 200, "set folders: {}", r.body);
        }
    };
    set_paths(vec!["jobs/a"]).await;
    let mut s = open(port, &format!("/cli/fs/events?workspace={}&token={}", ws.handle, enc(&pass))).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    file_changed(&ws, "jobs/a/first.txt");
    file_changed(&ws, "jobs/b/hidden.txt");
    let seen = watch(&mut s, 1_500).await;
    assert!(seen.has("jobs/a/first.txt") && !seen.has("hidden.txt"), "rig: {seen:?}");

    // Widened: jobs/b's very next frame arrives. Narrowed off jobs/a in the same edit.
    set_paths(vec!["jobs/b"]).await;
    file_changed(&ws, "jobs/b/widened.txt");
    file_changed(&ws, "jobs/a/narrowed.txt");
    let seen = watch(&mut s, 1_500).await;
    assert!(seen.has("jobs/b/widened.txt"), "a widened folder's frame gets through: {seen:?}");
    assert!(!seen.has("narrowed.txt"), "a narrowed folder's frame doesn't: {seen:?}");
    assert!(!seen.ended, "folders changed, nobody lost the room: {seen:?}");

    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read"] })).await;
    file_changed(&ws, "jobs/b/after-cap.txt");
    let seen = watch(&mut s, 1_500).await;
    assert!(!seen.has("after-cap.txt"), "{seen:?}");
    assert_eq!(seen.close, Some(4013), "{seen:?}");
}

/// Overlay: narrowed folders reach the very next frame. The attachment goes
/// `hidden`, under the pass as it is now, and the socket stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn narrowed_folders_hide_an_attachment_on_the_next_frame() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "attrow");
    let conv = pin_chat(&ws);
    for dir in ["open", "closed"] {
        std::fs::create_dir_all(ws.path.join(dir)).expect("mkdir");
        std::fs::write(ws.path.join(dir).join("a.txt"), b"a").expect("file");
    }
    let (role, pass) = guest_in(port, &ws, "atty", &["thread:read", "files:read"]).await;
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": role, "handle": ws.handle, "read": ["open", "closed"] })).await;
    let mut s = open(port, &format!("/cli/overlay/events?conversation={conv}&token={}", enc(&pass))).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let send = |text: &'static str| post(port, "/cli/thread/post", OWNER, json!({ "addr": ws.handle, "from": "k2", "text": text, "attachments": [{ "path": "closed/a.txt" }] }));
    let r = send("BEFORE-NARROW").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let seen = watch(&mut s, 1_500).await;
    assert!(seen.has("BEFORE-NARROW") && !seen.has("\"hidden\""), "rig: the folder is readable: {seen:?}");

    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": role, "handle": ws.handle, "read": ["open"] })).await;
    let r = send("AFTER-NARROW").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let seen = watch(&mut s, 1_500).await;
    assert!(seen.has("AFTER-NARROW") && seen.has("\"hidden\""), "the attachment is hidden at once: {seen:?}");
    assert!(!seen.has("a.txt"), "no name either: {seen:?}");
    assert!(!seen.ended, "narrowed isn't lost: {seen:?}");
}

/// Overlay: a re-pinned room's old conversation stops, with a plain close (not
/// 4013: nobody lost access; the client re-reads the pin). A room with no pin
/// this moment holds frames and stays. The new conversation opens fine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_moved_pin_is_not_lost_access() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "pinrow");
    let conv = pin_chat(&ws);
    let (_, pass) = guest_in(port, &ws, "pinny", ALL_CAPS).await;
    let mut s = open(port, &format!("/cli/overlay/events?conversation={conv}&token={}", enc(&pass))).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let set_pin = |pin: Option<&str>| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match pin {
            Some(p) => WorkspaceSession::update_session_id(&conn, &ws.id, p).expect("pin"),
            None => WorkspaceSession::clear_session_id(&conn, &ws.id).expect("clear"),
        };
    };
    // No pin for a moment (a Chat restarting): past the tick, still open.
    set_pin(None);
    let held = watch(&mut s, CLOSE_WITHIN_MS).await;
    assert!(!held.ended, "no pin is not a reason to close: {held:?}");
    set_pin(Some(&conv));
    say(port, &ws, "BACK-AGAIN").await;
    let back = watch(&mut s, 1_500).await;
    assert!(back.has("BACK-AGAIN") && !back.ended, "{back:?}");

    // Another conversation took the pin.
    let next = uuid::Uuid::new_v4().to_string();
    set_pin(Some(&next));
    let moved = watch(&mut s, CLOSE_WITHIN_MS).await;
    assert!(moved.ended, "the old conversation's socket ends: {moved:?}");
    assert_ne!(moved.close, Some(4013), "a re-pin isn't lost access: {moved:?}");
    assert_ne!(moved.close, Some(4003), "nor a dead pass: {moved:?}");
    let _new = open(port, &format!("/cli/overlay/events?conversation={next}&token={}", enc(&pass))).await;
}

/// Project chat: adding a room the guest doesn't hold takes the project away
/// with no write to the guest's own access at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_project_that_grows_past_a_guest_closes_its_socket() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let (ws, other) = (seed_ws(&env, "projrow"), seed_ws(&env, "projother"));
    let (_, pass) = guest_in(port, &ws, "proj1", &["thread:read"]).await;
    let (_, idle_pass) = guest_in(port, &ws, "proj2", &["thread:read"]).await;
    let group = project(port, "growing", &[&ws]).await;
    let idle_group = project(port, "growing-too", &[&ws]).await;
    let kept_group = project(port, "unchanged", &[&ws]).await;
    let mut s = open(port, &format!("/cli/skin/projects/events?group={group}&token={}", enc(&pass))).await;
    let mut idle = open(port, &format!("/cli/skin/projects/events?group={idle_group}&token={}", enc(&idle_pass))).await;
    let mut kept = open(port, &format!("/cli/skin/projects/events?group={kept_group}&token={}", enc(&pass))).await;
    owner_ok(port, "/cli/project-group/add-member", json!({ "group": group, "workspace": other.id })).await;
    owner_ok(port, "/cli/project-group/add-member", json!({ "group": idle_group, "workspace": other.id })).await;
    owner_ok(port, "/cli/project-group/msg", json!({ "group": group, "body": "AFTER-GROWN" })).await;
    let seen = watch(&mut s, 1_500).await;
    assert!(!seen.has("AFTER-GROWN"), "{seen:?}");
    assert_eq!(seen.close, Some(4013), "{seen:?}");
    let seen = watch(&mut idle, CLOSE_WITHIN_MS).await;
    assert_eq!(seen.close, Some(4013), "idle: {seen:?}");
    owner_ok(port, "/cli/project-group/msg", json!({ "group": kept_group, "body": "KEPT-LINE" })).await;
    let seen = watch(&mut kept, 1_500).await;
    assert!(seen.has("KEPT-LINE") && !seen.ended, "the same pass on a project it still holds: {seen:?}");
}

/// A platform pass (`session = 0`) keeps its rooms on its own row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_platform_pass_loses_its_sockets_too() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let (ws, other) = (seed_ws(&env, "platrow"), seed_ws(&env, "platother"));
    let conv = pin_chat(&ws);
    async fn mint(port: u16, name: &str, room: &str) -> Value {
        owner_ok(port, "/cli/skin/users", json!({ "username": name })).await;
        owner_ok(port, "/cli/skin-tokens", json!({ "name": name, "caps": ["thread:read", "tickets:read", "files:read"], "rooms": [room] })).await
    }
    let moved = mint(port, "plat-moved", &ws.handle).await;
    let revoked = mint(port, "plat-revoked", &ws.handle).await;
    let kept = mint(port, "plat-kept", &ws.handle).await;
    let raw = |v: &Value| v["token"].as_str().unwrap_or_else(|| panic!("token in {v}")).to_string();
    let id = |v: &Value| v["id"].as_str().unwrap_or_else(|| panic!("id in {v}")).to_string();
    let mut m_o = open(port, &format!("/cli/overlay/events?conversation={conv}&token={}", enc(&raw(&moved)))).await;
    let mut m_a = open(port, &format!("/cli/activity/events?workspace={}&token={}", ws.handle, enc(&raw(&moved)))).await;
    let mut m_f = open(port, &format!("/cli/fs/events?workspace={}&token={}", ws.handle, enc(&raw(&moved)))).await;
    let mut r_o = open(port, &format!("/cli/overlay/events?conversation={conv}&token={}", enc(&raw(&revoked)))).await;
    let mut k_o = open(port, &format!("/cli/overlay/events?conversation={conv}&token={}", enc(&raw(&kept)))).await;
    owner_ok(port, "/cli/skin-tokens/rooms", json!({ "id": id(&moved), "rooms": [other.handle] })).await;
    owner_ok(port, "/cli/skin-tokens/revoke", json!({ "id": id(&revoked) })).await;
    say(port, &ws, "PLAT-AFTER").await;
    ticket(&ws.id, "PLAT-TICKET");
    file_changed(&ws, "plat-after.txt");
    let (a, b, c, e, k) = tokio::join!(watch(&mut m_o, 1_500), watch(&mut m_a, 1_500), watch(&mut m_f, 1_500), watch(&mut r_o, 1_500), watch(&mut k_o, 1_500));
    assert!(!a.has("PLAT-AFTER") && a.close == Some(4013), "moved pass, overlay: {a:?}");
    assert!(!b.has("PLAT-TICKET") && b.close == Some(4013), "moved pass, activity: {b:?}");
    assert!(!c.has("plat-after.txt") && c.close == Some(4013), "moved pass, files: {c:?}");
    assert!(!e.has("PLAT-AFTER") && e.close == Some(4003), "revoked pass: {e:?}");
    assert!(k.has("PLAT-AFTER") && !k.ended, "another pass on the room: {k:?}");
}

// ── Over an App's link ───────────────────────────────────────────────

/// A link's context is task-local, and these checks run on a blocking thread
/// where it can't be seen: the same rows through a real gateway link, with an
/// App guest's session. Also a suspended App's guests lose the link's sockets.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_rows_hold_over_an_apps_link() {
    use k2_daemon::apps::gateway::{self, LinkSpec};
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "linkrow");
    let conv = pin_chat(&ws);
    let group = project(port, "linkproj", &[&ws]).await;
    let app = "linkrowapp";
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let svc = k2_core::published_services::insert(&conn, &ws.id, app, k2_core::published_services::CMD_APP_SENTINEL, &ws.path.to_string_lossy(), 0, "local", "stopped", "cmd", "")
            .expect("service")
            .id;
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp { app_id: app, service_id: &svc, seed_id: None, owner_workspace: &ws.id, isolation: "process", memory_limit_bytes: None, front_port: 0, created_by: "owner" },
        )
        .expect("insert app");
        svc
    };
    let session_of = |user: &'static str, role: String| {
        let svc = svc.clone();
        async move {
            let pid = k2_core::skin::host_login_detail(user).expect("guest").principal_id;
            k2_core::skin::create_grant(&k2_core::skin::SkinGrantWrite {
                subject_kind: "principal".into(),
                subject_id: pid,
                kind: "app".into(),
                target_id: svc.clone(),
                role: Some(role),
                scope: None,
                created_by: Some("owner".into()),
            })
            .expect("app grant");
            k2_core::skin::create_app_session_token(user, &svc).expect("App session").raw
        }
    };
    let (gone_role, _) = guest_in(port, &ws, "linkgone", ALL_CAPS).await;
    let (revoked_role, _) = guest_in(port, &ws, "linkrevoked", ALL_CAPS).await;
    let (kept_role, _) = guest_in(port, &ws, "linkkept", ALL_CAPS).await;
    let gone = session_of("linkgone", gone_role.clone()).await;
    let revoked = session_of("linkrevoked", revoked_role).await;
    let kept = session_of("linkkept", kept_role).await;

    let spec = LinkSpec { app_id: app.into(), service_id: svc.clone(), workspace_id: ws.id.clone(), port: 0, caps: ALL_CAPS.iter().map(|c| c.to_string()).collect(), rooms: vec![ws.id.clone()] };
    let _own_pass = gateway::open(spec, None).expect("open link");
    let link = gateway::port_of(app).expect("listening");

    let sockets = |pass: String| {
        let (conv, group, handle) = (conv.clone(), group.clone(), ws.handle.clone());
        async move {
            let q = enc(&pass);
            (
                open(link, &format!("/cli/overlay/events?conversation={conv}&token={q}")).await,
                open(link, &format!("/cli/activity/events?workspace={handle}&token={q}")).await,
                open(link, &format!("/cli/fs/events?workspace={handle}&token={q}")).await,
                open(link, &format!("/cli/skin/projects/events?group={group}&token={q}")).await,
            )
        }
    };
    let (mut g_o, mut g_a, mut g_f, mut g_p) = sockets(gone.clone()).await;
    let (mut r_o, mut r_a, mut r_f, mut r_p) = sockets(revoked.clone()).await;
    let (mut k_o, mut k_a, mut k_f, mut k_p) = sockets(kept.clone()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    cause_guest_frames(port, &ws, &group, "LINKBEFORE").await;
    let (a, b, c, f) = tokio::join!(watch(&mut k_o, 1_500), watch(&mut k_a, 1_500), watch(&mut k_f, 1_500), watch(&mut k_p, 1_500));
    assert!(a.has("LINKBEFORE-THREAD") && b.has("LINKBEFORE-TICKET") && c.has("LINKBEFORE-file") && f.has("LINKBEFORE-PROJECT"), "rig: {a:?} {b:?} {c:?} {f:?}");

    // The App session's role, narrowed to what opens none of these sockets.
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": gone_role, "handle": ws.handle, "caps": ["thread:post"] })).await;
    let rid = k2_core::skin::resolve_skin_token(&revoked).expect("live").id;
    assert!(k2_core::skin::revoke_token(&rid).expect("revoke"));
    cause_guest_frames(port, &ws, &group, "LINKAFTER").await;
    let (a1, a2, a3, a4, b1, b2, b3, b4) = tokio::join!(
        watch(&mut g_o, 2_000),
        watch(&mut g_a, 2_000),
        watch(&mut g_f, 2_000),
        watch(&mut g_p, 2_000),
        watch(&mut r_o, 2_000),
        watch(&mut r_a, 2_000),
        watch(&mut r_f, 2_000),
        watch(&mut r_p, 2_000)
    );
    for (what, seen, code) in [
        ("lost room, overlay", &a1, 4013),
        ("lost room, activity", &a2, 4013),
        ("lost room, files", &a3, 4013),
        ("lost room, project", &a4, 4013),
        ("revoked, overlay", &b1, 4003),
        ("revoked, activity", &b2, 4003),
        ("revoked, files", &b3, 4003),
        ("revoked, project", &b4, 4003),
    ] {
        assert!(!seen.has("LINKAFTER"), "{what}: {seen:?}");
        assert_eq!(seen.close, Some(code), "{what}: {seen:?}");
    }
    let (a, b, c, f) = tokio::join!(watch(&mut k_o, 1_500), watch(&mut k_a, 1_500), watch(&mut k_f, 1_500), watch(&mut k_p, 1_500));
    assert!(a.has("LINKAFTER-THREAD") && !a.ended, "kept: {a:?}");
    assert!(b.has("LINKAFTER-TICKET") && !b.ended, "kept: {b:?}");
    assert!(c.has("LINKAFTER-file") && !c.ended, "kept: {c:?}");
    assert!(f.has("LINKAFTER-PROJECT") && !f.ended, "kept: {f:?}");

    // A suspended App: its link's open sockets end, as a dead pass.
    let held = post(port, "/cli/app/suspend", OWNER, json!({ "app": app, "reason": "test" })).await;
    assert_eq!(held.status, 200, "suspend: {}", held.body);
    let (a, b, c, f) = tokio::join!(
        watch(&mut k_o, CLOSE_WITHIN_MS),
        watch(&mut k_a, CLOSE_WITHIN_MS),
        watch(&mut k_f, CLOSE_WITHIN_MS),
        watch(&mut k_p, CLOSE_WITHIN_MS)
    );
    for (what, seen) in [("overlay", &a), ("activity", &b), ("files", &c), ("project", &f)] {
        assert_eq!(seen.close, Some(4003), "suspended App, {what}: {seen:?}");
    }
    gateway::close(app, None, true);
}

// ── An older client's refused re-dials ───────────────────────────────

/// A vendored SDK from before the fix re-dials a socket it can't open: about 30
/// times in 5 minutes. On a link that must not wake the App's agent through the
/// owner's refused-calls alert, and each refusal keeps its audit row. The same
/// refusals as plain calls still raise the alert.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refused_socket_dials_keep_their_rows_and_raise_no_alert() {
    use k2_daemon::apps::gateway::{self, LinkSpec};
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
    let ws = seed_ws(&env, &format!("redial{tag}"));
    let conv = pin_chat(&ws);
    let app = format!("redial{tag}");
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let svc = k2_core::published_services::insert(&conn, &ws.id, &app, k2_core::published_services::CMD_APP_SENTINEL, &ws.path.to_string_lossy(), 0, "local", "stopped", "cmd", "")
            .expect("service")
            .id;
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp { app_id: &app, service_id: &svc, seed_id: None, owner_workspace: &ws.id, isolation: "process", memory_limit_bytes: None, front_port: 0, created_by: "owner" },
        )
        .expect("insert app");
        svc
    };
    let alerts = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let a2 = alerts.clone();
    k2_daemon::apps::alerts::set_sink(Some(Box::new(move |_ws: &str, text: &str| {
        a2.lock().expect("alerts").push(text.to_string());
    })));

    let spec = LinkSpec { app_id: app.clone(), service_id: svc.clone(), workspace_id: ws.id.clone(), port: 0, caps: vec!["thread:read".into()], rooms: vec![ws.id.clone()] };
    gateway::open(spec, None).expect("open link");
    let link = gateway::port_of(&app).expect("listening");
    let dead_pass = "k2skn_redial_dead_pass_not_a_real_one_at_all";
    let pq = format!("/cli/overlay/events?conversation={conv}");
    for i in 0..25 {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!("ws://127.0.0.1:{link}{pq}").into_client_request().expect("request");
        req.headers_mut().insert("Authorization", format!("Bearer {dead_pass}").parse().expect("header"));
        match tokio_tungstenite::connect_async(req).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status().as_u16(), 401, "dial {i}"),
            other => panic!("dial {i}: {:?}", other.map(|_| "opened")),
        }
    }
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    let said = alerts.lock().expect("alerts").clone();
    assert!(!said.iter().any(|t| t.contains("refused calls")), "25 refused dials woke the agent: {said:?}");

    // Every refused dial has its row.
    let flush_and_read = get(port, "/cli/app/audit", OWNER, &format!("app={app}&denied=1&limit=100")).await;
    assert_eq!(flush_and_read.status, 200, "{}", flush_and_read.body);
    assert!(flush_and_read.body.matches("pass_invalid").count() >= 25, "the audit keeps each refusal: {}", flush_and_read.body);

    // Control: the same refusals as plain calls are what the alert is for.
    for _ in 0..25 {
        let (p, dead) = (link, dead_pass.to_string());
        let (status, _) = tokio::task::spawn_blocking(move || {
            use std::io::{Read, Write};
            let mut s = std::net::TcpStream::connect(("127.0.0.1", p)).expect("connect link");
            s.write_all(format!("GET /cli/thread HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {dead}\r\nConnection: close\r\n\r\n").as_bytes()).expect("write");
            let mut out = String::new();
            s.read_to_string(&mut out).expect("read");
            (out.split_whitespace().nth(1).expect("status").to_string(), out)
        })
        .await
        .expect("join");
        assert_eq!(status, "401");
    }
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    let said = alerts.lock().expect("alerts").clone();
    k2_daemon::apps::alerts::set_sink(None);
    assert!(said.iter().any(|t| t.contains("refused calls")), "plain refused calls still raise it: {said:?}");
    gateway::close(&app, None, true);
}
