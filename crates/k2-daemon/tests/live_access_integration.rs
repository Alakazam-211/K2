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
