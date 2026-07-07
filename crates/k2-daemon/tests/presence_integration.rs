//! S1 (presence/multiplayer arc) — presence-substrate integration tests.
//!
//! Drive the REAL dispatcher (`k2_daemon::test_harness::start`, the #630
//! harness) end-to-end: open real `/cli/sessions/events` WebSockets with
//! real tokens (owner + seeded connect-user sessions), then assert the
//! S1 contract:
//!
//!   1. roster aggregation over two identities (`GET /cli/presence/roster`
//!      + the auth gate),
//!   2. drop-one → a `presence_changed` broadcast reaches the survivor,
//!   3. a `?path=` workspace subscription surfaces in `workspaces` and
//!      leaves on close,
//!   4. `close_connections_for` tears both of a user's sockets down and
//!      the roster updates,
//!   5. (S4) viewer-role rows + `POST /cli/presence/grant`: grant flips
//!      `grantedEdit` in the broadcast, the last disconnect auto-revokes,
//!   6. (S4) grant-route gates: member-role target → 400, disconnected
//!      target → 400, member actor → 403, GET → 405.
//!
//! ISOLATION: the presence registry, session-events bus, connect-user
//! stores, and `$HOME` are process-wide — every test serializes on
//! `TEST_LOCK` and points `$HOME` at a fresh tempdir (auth-suite
//! pattern). Each test also waits for the registry to drain back to
//! empty before releasing the lock so leakage across tests fails loudly
//! at the source.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

use k2_core::connect_users::{self, Role};
use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-presence-s1";

type WsClient = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// A minimal parsed HTTP response: numeric status + body. (Auth-suite
/// harness pattern — raw loopback socket, no extra deps.)
struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream =
        StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!(
            "{method} {path_and_query} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\r\n"
        ),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");

    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some((status, body, complete)) = try_parse(&raw) {
            if complete {
                return Resp { status, body };
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                break
            }
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));
    let body = match text.split_once("\r\n\r\n") {
        Some((_h, b)) => b.to_string(),
        None => String::new(),
    };
    Resp { status, body }
}

fn try_parse(raw: &[u8]) -> Option<(u16, String, bool)> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())?;
    let content_len = headers.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let complete = match content_len {
        Some(clen) => body.len() >= clen,
        None => true,
    };
    Some((status, body.to_string(), complete))
}

/// Redirect `$HOME` to a fresh tempdir (isolating connect-users.json +
/// connect-sessions.json writes) and run `f`, restoring `$HOME` after.
/// Caller holds `TEST_LOCK`.
fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("k2-presence-s1-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);

    f();

    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Seed a connect-user with a role and return a live session token.
fn seed_user_session(username: &str, password: &str, role: Role) -> String {
    connect_users::add_user(username, password).expect("add_user");
    connect_users::set_role(username, role).expect("set_role");
    connect_users::create_session(username)
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

/// Open a `/cli/sessions/events` WS with `token` + `path`, and consume
/// the `hello` frame. When `hello` has been received, the daemon has
/// ALREADY registered the connection in the presence registry (register
/// happens before the hello send), so a subsequent roster GET is
/// deterministic — no sleeps.
async fn connect_events_ws(port: u16, token: &str, path: &str) -> WsClient {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?path={path}&token={token}");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("events WS connect");
    let hello = next_json_frame(&mut ws, "hello frame").await;
    assert_eq!(
        hello["kind"], "hello",
        "first frame must be the hello: {hello}"
    );
    ws
}

/// Next Text frame parsed as JSON. Fails loud on timeout/close/non-text.
async fn next_json_frame(ws: &mut WsClient, what: &str) -> serde_json::Value {
    let msg = timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
        .unwrap_or_else(|| panic!("stream closed waiting for {what}"))
        .expect("ws message Ok");
    match msg {
        Message::Text(t) => serde_json::from_str(&t).expect("frame is JSON"),
        other => panic!("expected Text frame waiting for {what}, got {other:?}"),
    }
}

/// Read frames until a `presence_changed` whose roster satisfies `pred`
/// arrives. Skips other event kinds and non-matching rosters (the bus
/// carries intermediate presence states + unrelated events). Fails loud
/// after `max_frames` or on timeout.
async fn next_presence_matching<F>(
    ws: &mut WsClient,
    what: &str,
    mut pred: F,
) -> serde_json::Value
where
    F: FnMut(&serde_json::Value) -> bool,
{
    for _ in 0..50 {
        let msg = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
            .unwrap_or_else(|| panic!("stream closed waiting for {what}"));
        let text = match msg.expect("ws message Ok") {
            Message::Text(t) => t,
            // Liveness pings etc. are fine — skip.
            _ => continue,
        };
        let v: serde_json::Value = serde_json::from_str(&text).expect("frame is JSON");
        if v["kind"] == "presence_changed" && pred(&v["roster"]) {
            return v;
        }
    }
    panic!("never received a presence_changed matching: {what}");
}

/// Find the roster row for `user` in a roster JSON array.
fn row<'a>(roster: &'a serde_json::Value, user: &str) -> Option<&'a serde_json::Value> {
    roster
        .as_array()
        .expect("roster is an array")
        .iter()
        .find(|r| r["user"] == user)
}

/// GET the roster snapshot and return the parsed `roster` array.
fn get_roster(port: u16, token: &str) -> serde_json::Value {
    let r = http(
        port,
        "GET",
        &format!("/cli/presence/roster?token={token}"),
        None,
    );
    assert_eq!(r.status, 200, "roster GET must be 200; body={}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("roster body is JSON");
    assert!(v["roster"].is_array(), "body must carry a roster array: {v}");
    v["roster"].clone()
}

/// Wait (in-process) for the presence registry to drain to empty —
/// deregistration runs on the handler tasks, so it can trail a client
/// close by a beat. Fails loud on deadline so cross-test leakage is
/// caught at the source.
fn await_registry_empty(context: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let roster = k2_daemon::presence::roster();
        if roster.is_empty() {
            return;
        }
        if Instant::now() >= deadline {
            panic!("presence registry not empty ({context}): {roster:?}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

// ─────────────────────────────────────────────────────────────────────
// 1 — roster over two identities + route auth gate
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn roster_aggregates_owner_and_connect_user_identities() {
    let _g = lock();
    with_temp_home(|| {
        await_registry_empty("at test start");
        let member = seed_user_session("pres_member", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        futures_block(async {
            let mut ws_owner = connect_events_ws(d.port, OWNER_TOKEN, "").await;
            let mut ws_member = connect_events_ws(d.port, &member, "").await;

            // Snapshot: both identities present with correct roles +
            // window counts. Registration precedes hello, so this is
            // deterministic.
            let roster = get_roster(d.port, OWNER_TOKEN);
            assert_eq!(
                roster.as_array().unwrap().len(),
                2,
                "exactly the two connected identities: {roster}"
            );
            let owner = row(&roster, "owner").expect("owner row present");
            assert_eq!(owner["role"], "owner");
            assert_eq!(owner["windowCount"], 1);
            assert_eq!(owner["workspaces"], serde_json::json!([]));
            assert_eq!(owner["grantedEdit"], false);
            assert!(
                owner["connectedAt"].as_i64().expect("connectedAt is a number") > 0,
                "connectedAt must be a unix timestamp: {owner}"
            );
            let m = row(&roster, "pres_member").expect("member row present");
            assert_eq!(m["role"], "member");
            assert_eq!(m["windowCount"], 1);
            assert_eq!(m["grantedEdit"], false);

            // The roster is identical through a connect-user session
            // (authorized = owner OR session, the whoami gate).
            let roster_via_member = get_roster(d.port, &member);
            assert_eq!(roster, roster_via_member);

            // Auth gate: no token / garbage token → 403.
            let r = http(d.port, "GET", "/cli/presence/roster", None);
            assert_eq!(r.status, 403, "no token must 403; body={}", r.body);
            let r = http(d.port, "GET", "/cli/presence/roster?token=garbage", None);
            assert_eq!(r.status, 403, "garbage token must 403; body={}", r.body);

            // Method gate: POST → 405 (read-only route).
            let r = http(
                d.port,
                "POST",
                &format!("/cli/presence/roster?token={OWNER_TOKEN}"),
                Some("{}"),
            );
            assert_eq!(r.status, 405, "POST must 405; body={}", r.body);

            let _ = ws_owner.close(None).await;
            let _ = ws_member.close(None).await;
        });

        await_registry_empty("after closing both sockets");
    });
}

// ─────────────────────────────────────────────────────────────────────
// 2 — drop one socket → presence_changed reaches the survivor
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_a_socket_broadcasts_updated_roster_to_survivor() {
    let _g = lock();
    with_temp_home(|| {
        await_registry_empty("at test start");
        let member = seed_user_session("pres_dropper", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        futures_block(async {
            let mut ws_owner = connect_events_ws(d.port, OWNER_TOKEN, "").await;
            let mut ws_member = connect_events_ws(d.port, &member, "").await;

            // The survivor sees the member JOIN first (its subscription
            // predates the member's registration).
            next_presence_matching(&mut ws_owner, "member join", |roster| {
                row(roster, "pres_dropper").is_some()
            })
            .await;

            // Close the member socket → the survivor must receive a
            // presence_changed with the member gone and itself intact.
            ws_member.close(None).await.expect("close member ws");
            let ev = next_presence_matching(&mut ws_owner, "member departure", |roster| {
                row(roster, "pres_dropper").is_none()
            })
            .await;
            let roster = &ev["roster"];
            let owner = row(roster, "owner").expect("survivor still in roster");
            assert_eq!(owner["windowCount"], 1);
            assert_eq!(
                roster.as_array().unwrap().len(),
                1,
                "only the survivor remains: {roster}"
            );

            let _ = ws_owner.close(None).await;
        });

        await_registry_empty("after closing sockets");
    });
}

// ─────────────────────────────────────────────────────────────────────
// 3 — workspace (?path=) subscription surfaces in `workspaces`
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_socket_path_appears_and_leaves_with_the_socket() {
    let _g = lock();
    with_temp_home(|| {
        await_registry_empty("at test start");
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        futures_block(async {
            let mut ws_app = connect_events_ws(d.port, OWNER_TOKEN, "").await;
            let mut ws_workspace =
                connect_events_ws(d.port, OWNER_TOKEN, "/x/presence-workspace").await;

            // Broadcast: the app socket sees the workspace path join the
            // owner row. A workspace socket is NOT a window — the count
            // stays at the one app socket.
            let ev = next_presence_matching(&mut ws_app, "workspace path joins", |roster| {
                row(roster, "owner").is_some_and(|o| {
                    o["workspaces"] == serde_json::json!(["/x/presence-workspace"])
                })
            })
            .await;
            let owner = row(&ev["roster"], "owner").unwrap();
            assert_eq!(
                owner["windowCount"], 1,
                "workspace sockets must not count as windows: {owner}"
            );

            // Snapshot agrees with the broadcast.
            let roster = get_roster(d.port, OWNER_TOKEN);
            let owner = row(&roster, "owner").expect("owner row");
            assert_eq!(
                owner["workspaces"],
                serde_json::json!(["/x/presence-workspace"])
            );

            // Close the workspace socket → the path leaves the roster.
            ws_workspace.close(None).await.expect("close workspace ws");
            next_presence_matching(&mut ws_app, "workspace path leaves", |roster| {
                row(roster, "owner")
                    .is_some_and(|o| o["workspaces"] == serde_json::json!([]))
            })
            .await;

            let _ = ws_app.close(None).await;
        });

        await_registry_empty("after closing sockets");
    });
}

// ─────────────────────────────────────────────────────────────────────
// 5 (S4) — viewer role + edit grant: broadcast + auto-revoke on last
// disconnect
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewer_grant_broadcasts_and_last_disconnect_auto_revokes() {
    let _g = lock();
    with_temp_home(|| {
        await_registry_empty("at test start");
        let viewer = seed_user_session("pres_viewer", "password123", Role::Viewer);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        futures_block(async {
            let mut ws_owner = connect_events_ws(d.port, OWNER_TOKEN, "").await;
            let mut ws_viewer = connect_events_ws(d.port, &viewer, "").await;

            // The viewer role rides the roster wire; starts ungranted.
            let roster = get_roster(d.port, OWNER_TOKEN);
            let v = row(&roster, "pres_viewer").expect("viewer row present");
            assert_eq!(v["role"], "viewer");
            assert_eq!(v["grantedEdit"], false);

            // Grant (owner token) → success body + broadcast flips
            // grantedEdit to true on the viewer's row.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/presence/grant?token={OWNER_TOKEN}"),
                Some(r#"{"username":"pres_viewer","granted":true}"#),
            );
            assert_eq!(r.status, 200, "grant must succeed; body={}", r.body);
            let body: serde_json::Value = serde_json::from_str(&r.body).expect("JSON");
            assert_eq!(body["success"], true);
            assert_eq!(body["username"], "pres_viewer");
            assert_eq!(body["granted"], true);
            next_presence_matching(&mut ws_owner, "grantedEdit flips true", |roster| {
                row(roster, "pres_viewer").is_some_and(|v| v["grantedEdit"] == true)
            })
            .await;

            // Snapshot agrees with the broadcast.
            let roster = get_roster(d.port, OWNER_TOKEN);
            let v = row(&roster, "pres_viewer").expect("viewer row");
            assert_eq!(v["grantedEdit"], true);

            // Revoke via the same route (granted:false) → broadcast false.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/presence/grant?token={OWNER_TOKEN}"),
                Some(r#"{"username":"pres_viewer","granted":false}"#),
            );
            assert_eq!(r.status, 200, "revoke must succeed; body={}", r.body);
            next_presence_matching(&mut ws_owner, "grantedEdit flips false", |roster| {
                row(roster, "pres_viewer").is_some_and(|v| v["grantedEdit"] == false)
            })
            .await;

            // Re-grant, then close the viewer's LAST socket → the grant is
            // auto-revoked: a reconnect comes back grantedEdit=false.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/presence/grant?token={OWNER_TOKEN}"),
                Some(r#"{"username":"pres_viewer","granted":true}"#),
            );
            assert_eq!(r.status, 200, "re-grant must succeed; body={}", r.body);
            next_presence_matching(&mut ws_owner, "re-grant lands", |roster| {
                row(roster, "pres_viewer").is_some_and(|v| v["grantedEdit"] == true)
            })
            .await;
            ws_viewer.close(None).await.expect("close viewer ws");
            next_presence_matching(&mut ws_owner, "viewer departs", |roster| {
                row(roster, "pres_viewer").is_none()
            })
            .await;

            let mut ws_viewer2 = connect_events_ws(d.port, &viewer, "").await;
            let roster = get_roster(d.port, OWNER_TOKEN);
            let v = row(&roster, "pres_viewer").expect("viewer reconnected");
            assert_eq!(
                v["grantedEdit"], false,
                "grant must be auto-revoked on the last disconnect: {v}"
            );

            let _ = ws_viewer2.close(None).await;
            let _ = ws_owner.close(None).await;
        });

        await_registry_empty("after closing sockets");
    });
}

// ─────────────────────────────────────────────────────────────────────
// 6 (S4) — grant-route gates: target role/connection + actor + method
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grant_route_gates_target_actor_and_method() {
    let _g = lock();
    with_temp_home(|| {
        await_registry_empty("at test start");
        let member = seed_user_session("pres_gmember", "password123", Role::Member);
        // A viewer who exists but never connects (the disconnected target).
        let _viewer_offline =
            seed_user_session("pres_goffline", "password123", Role::Viewer);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        futures_block(async {
            let mut ws_member = connect_events_ws(d.port, &member, "").await;

            // Member-role target (connected) → 400: grants only apply to
            // viewer-role users.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/presence/grant?token={OWNER_TOKEN}"),
                Some(r#"{"username":"pres_gmember","granted":true}"#),
            );
            assert_eq!(r.status, 400, "member target must 400; body={}", r.body);
            assert!(
                r.body.contains("viewer-role"),
                "message must explain the viewer-only rule: {}",
                r.body
            );

            // Viewer-role target with NO live connection → 400.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/presence/grant?token={OWNER_TOKEN}"),
                Some(r#"{"username":"pres_goffline","granted":true}"#),
            );
            assert_eq!(
                r.status, 400,
                "disconnected target must 400; body={}",
                r.body
            );
            assert!(
                r.body.contains("not connected"),
                "message must explain the live-connection rule: {}",
                r.body
            );

            // Unknown target → 400.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/presence/grant?token={OWNER_TOKEN}"),
                Some(r#"{"username":"pres_ghost","granted":true}"#),
            );
            assert_eq!(r.status, 400, "unknown target must 400; body={}", r.body);

            // Member ACTOR → 403 (require_manage: owner/admin only).
            let r = http(
                d.port,
                "POST",
                &format!("/cli/presence/grant?token={member}"),
                Some(r#"{"username":"pres_goffline","granted":true}"#),
            );
            assert_eq!(r.status, 403, "member actor must 403; body={}", r.body);

            // No/garbage token → 403.
            let r = http(
                d.port,
                "POST",
                "/cli/presence/grant?token=garbage",
                Some(r#"{"username":"pres_goffline","granted":true}"#),
            );
            assert_eq!(r.status, 403, "garbage token must 403; body={}", r.body);

            // GET → 405 (POST-only mutating route).
            let r = http(
                d.port,
                "GET",
                &format!("/cli/presence/grant?token={OWNER_TOKEN}"),
                None,
            );
            assert_eq!(r.status, 405, "GET must 405; body={}", r.body);

            let _ = ws_member.close(None).await;
        });

        await_registry_empty("after closing sockets");
    });
}

// ─────────────────────────────────────────────────────────────────────
// 4 — close_connections_for closes the user's live sockets
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_connections_for_closes_both_sockets_and_updates_roster() {
    let _g = lock();
    with_temp_home(|| {
        await_registry_empty("at test start");
        let member = seed_user_session("pres_kickme", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        futures_block(async {
            let mut ws_a = connect_events_ws(d.port, &member, "").await;
            let mut ws_b = connect_events_ws(d.port, &member, "").await;

            // Both windows registered (registration precedes hello).
            let roster = get_roster(d.port, OWNER_TOKEN);
            let m = row(&roster, "pres_kickme").expect("user connected");
            assert_eq!(m["windowCount"], 2, "two windows before the kick: {m}");

            // Fire the close handles (the harness daemon runs in-process,
            // so this hits the same registry the WS handlers registered
            // in — exactly what S3's kick route will call).
            let fired = k2_daemon::presence::close_connections_for("pres_kickme");
            assert_eq!(fired, 2, "both live connections must fire");

            // Both sockets must observe a server-initiated Close (or a
            // clean stream end) — NOT hang until the 5s re-auth tick.
            for (ws, name) in [(&mut ws_a, "ws_a"), (&mut ws_b, "ws_b")] {
                let deadline = Instant::now() + Duration::from_secs(4);
                loop {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    assert!(
                        !remaining.is_zero(),
                        "{name} was not closed by close_connections_for in time"
                    );
                    match timeout(remaining, ws.next()).await {
                        Ok(None) => break,                          // stream ended
                        Ok(Some(Ok(Message::Close(_)))) => break,   // server close
                        Ok(Some(Ok(_))) => continue,                // drain events
                        Ok(Some(Err(_))) => break,                  // torn down
                        Err(_) => panic!(
                            "{name} still open after close_connections_for"
                        ),
                    }
                }
            }

            // The handlers deregister on their way out → roster drains
            // the user (poll: teardown runs on the handler tasks).
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let roster = get_roster(d.port, OWNER_TOKEN);
                if row(&roster, "pres_kickme").is_none() {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "kicked user still in roster: {roster}"
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        });

        await_registry_empty("after the kick");
    });
}
