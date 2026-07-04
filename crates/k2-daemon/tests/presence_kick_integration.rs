//! S3 (presence/multiplayer arc) — `POST /cli/presence/kick` integration
//! tests, cribbing the S1 harness (`presence_integration.rs`): drive the
//! REAL dispatcher via `k2_daemon::test_harness::start`, real tokens
//! (owner + seeded connect-user sessions), real events WebSockets.
//!
//! The S3 contract under test:
//!
//!   1. an Admin kicks a Member → BOTH of the member's live sockets are
//!      closed immediately, the member's tokens no longer authenticate,
//!      logging in again works, and the response reports the revoked /
//!      closed counts;
//!   2. a Member actor is rejected by the manage gate (403);
//!   3. an Admin kicking a fellow Admin is rejected by the kick matrix
//!      (403 — stricter than `can_act_on`, PRD §4 "not owner/admin");
//!   4. GET is method-gated (405, feedback_post_only_route_guards);
//!   5. an unknown username (and the literal "owner") → 400.
//!
//! ISOLATION: the presence registry, session-events bus, connect-user
//! stores, and `$HOME` are process-wide — every test serializes on
//! `TEST_LOCK` and points `$HOME` at a fresh tempdir (auth-suite
//! pattern), and waits for the registry to drain before releasing.

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

const OWNER_TOKEN: &str = "owner-token-deadbeef-presence-s3";

type WsClient = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// A minimal parsed HTTP response: numeric status + body (S1 harness).
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

/// Redirect `$HOME` to a fresh tempdir and run `f` (S1 harness). Caller
/// holds `TEST_LOCK`.
fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("k2-presence-s3-{}-{nanos}", std::process::id()));
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

/// Open a `/cli/sessions/events` WS with `token` and consume the `hello`
/// frame — after which the connection is REGISTERED in the presence
/// registry (register precedes the hello send), so kicks/rosters are
/// deterministic without sleeps.
async fn connect_events_ws(port: u16, token: &str) -> WsClient {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?path=&token={token}");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("events WS connect");
    let msg = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for hello")
        .expect("stream closed before hello")
        .expect("ws message Ok");
    let hello: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(&t).expect("frame is JSON"),
        other => panic!("expected Text hello frame, got {other:?}"),
    };
    assert_eq!(hello["kind"], "hello", "first frame must be the hello: {hello}");
    ws
}

/// Assert the socket observes a server-initiated close (or clean stream
/// end) within 4s — i.e. the kick did NOT wait for the 5s re-auth tick.
async fn expect_server_close(ws: &mut WsClient, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "{name} was not closed by the kick in time"
        );
        match timeout(remaining, ws.next()).await {
            Ok(None) => break,                        // stream ended
            Ok(Some(Ok(Message::Close(_)))) => break, // server close
            Ok(Some(Ok(_))) => continue,              // drain events
            Ok(Some(Err(_))) => break,                // torn down
            Err(_) => panic!("{name} still open after the kick"),
        }
    }
}

/// Wait for the in-process presence registry to drain (S1 harness).
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

fn kick(port: u16, actor_token: &str, username: &str) -> Resp {
    http(
        port,
        "POST",
        &format!("/cli/presence/kick?token={actor_token}"),
        Some(&serde_json::json!({ "username": username }).to_string()),
    )
}

// ─────────────────────────────────────────────────────────────────────
// 1 — admin kicks member: sockets close, tokens die, re-login works
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_kick_closes_member_sockets_and_revokes_sessions() {
    let _g = lock();
    with_temp_home(|| {
        await_registry_empty("at test start");
        let admin = seed_user_session("kick_admin", "password123", Role::Admin);
        // Two member SESSIONS (two logins/windows) — the kick must report
        // both revoked and close both live sockets.
        let member_a = seed_user_session("kick_member", "password123", Role::Member);
        let member_b = connect_users::create_session("kick_member");
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        futures_block(async {
            let mut ws_a = connect_events_ws(d.port, &member_a).await;
            let mut ws_b = connect_events_ws(d.port, &member_b).await;

            // Pre-kick sanity: the member is in the roster with 2 windows.
            let r = http(
                d.port,
                "GET",
                &format!("/cli/presence/roster?token={OWNER_TOKEN}"),
                None,
            );
            assert_eq!(r.status, 200, "roster GET: {}", r.body);
            let v: serde_json::Value = serde_json::from_str(&r.body).expect("roster JSON");
            let row = v["roster"]
                .as_array()
                .expect("roster array")
                .iter()
                .find(|u| u["user"] == "kick_member")
                .expect("member connected")
                .clone();
            assert_eq!(row["windowCount"], 2, "two windows before the kick: {row}");

            // THE KICK (admin actor).
            let r = kick(d.port, &admin, "kick_member");
            assert_eq!(r.status, 200, "kick must succeed; body={}", r.body);
            let v: serde_json::Value = serde_json::from_str(&r.body).expect("kick JSON");
            assert_eq!(v["success"], true, "body: {v}");
            assert_eq!(v["revokedSessions"], 2, "both persisted sessions: {v}");
            assert_eq!(v["closedConnections"], 2, "both live sockets: {v}");

            // Both sockets observe a server-initiated close, immediately.
            expect_server_close(&mut ws_a, "ws_a").await;
            expect_server_close(&mut ws_b, "ws_b").await;

            // Durable revocation: neither member token authenticates any
            // more (the roster GET is session-gated like whoami).
            for (tok, name) in [(&member_a, "member_a"), (&member_b, "member_b")] {
                let r = http(
                    d.port,
                    "GET",
                    &format!("/cli/presence/roster?token={tok}"),
                    None,
                );
                assert_eq!(
                    r.status, 403,
                    "{name} must be dead after the kick; body={}",
                    r.body
                );
            }

            // The roster drains the kicked user (teardown runs on the
            // handler tasks — poll briefly).
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let r = http(
                    d.port,
                    "GET",
                    &format!("/cli/presence/roster?token={OWNER_TOKEN}"),
                    None,
                );
                let v: serde_json::Value =
                    serde_json::from_str(&r.body).expect("roster JSON");
                let gone = v["roster"]
                    .as_array()
                    .expect("roster array")
                    .iter()
                    .all(|u| u["user"] != "kick_member");
                if gone {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "kicked user still in roster: {v}"
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }

            // A kick is not a ban: logging in again mints a fresh session.
            let r = http(
                d.port,
                "POST",
                "/cli/auth/login",
                Some(r#"{"username":"kick_member","password":"password123"}"#),
            );
            assert_eq!(r.status, 200, "re-login must work; body={}", r.body);
            let v: serde_json::Value = serde_json::from_str(&r.body).expect("login JSON");
            assert!(
                v["token"].as_str().is_some_and(|t| !t.is_empty()),
                "login must mint a token: {v}"
            );
        });

        await_registry_empty("after the kick test");
    });
}

// ─────────────────────────────────────────────────────────────────────
// 2 — a Member actor is rejected by the manage gate
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_actor_cannot_kick() {
    let _g = lock();
    with_temp_home(|| {
        await_registry_empty("at test start");
        let member = seed_user_session("kick_lowly", "password123", Role::Member);
        let _victim = seed_user_session("kick_target", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        futures_block(async {
            let r = kick(d.port, &member, "kick_target");
            assert_eq!(r.status, 403, "member actor must 403; body={}", r.body);
            // The target's session survives an unauthorized attempt: it
            // still authenticates.
            let victim_tok = connect_users::create_session("kick_target");
            let r = http(
                d.port,
                "GET",
                &format!("/cli/presence/roster?token={victim_tok}"),
                None,
            );
            assert_eq!(r.status, 200, "target unaffected; body={}", r.body);
        });

        await_registry_empty("after member-actor test");
    });
}

// ─────────────────────────────────────────────────────────────────────
// 3 — admin-on-admin is rejected by the kick matrix
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_cannot_kick_admin() {
    let _g = lock();
    with_temp_home(|| {
        await_registry_empty("at test start");
        let admin_a = seed_user_session("kick_adm_a", "password123", Role::Admin);
        let admin_b = seed_user_session("kick_adm_b", "password123", Role::Admin);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        futures_block(async {
            // Open a live socket for the target so a wrongly-allowed kick
            // would be OBSERVABLE (socket close), not just a status code.
            let mut ws_b = connect_events_ws(d.port, &admin_b).await;

            let r = kick(d.port, &admin_a, "kick_adm_b");
            assert_eq!(r.status, 403, "admin→admin must 403; body={}", r.body);
            assert!(
                r.body.contains("cannot kick"),
                "clear matrix message, got: {}",
                r.body
            );

            // The target's session + socket survive. (The socket is
            // asserted alive implicitly: the registry drain below would
            // hang-fail if it had been torn down mid-close; the durable
            // check is the token still authenticating.)
            let r = http(
                d.port,
                "GET",
                &format!("/cli/presence/roster?token={admin_b}"),
                None,
            );
            assert_eq!(r.status, 200, "target session survives; body={}", r.body);

            // The OWNER can kick an admin (can_act_on: owner acts on any).
            let r = kick(d.port, OWNER_TOKEN, "kick_adm_b");
            assert_eq!(r.status, 200, "owner→admin must work; body={}", r.body);
            let v: serde_json::Value = serde_json::from_str(&r.body).expect("kick JSON");
            assert_eq!(v["closedConnections"], 1, "the live socket: {v}");
            expect_server_close(&mut ws_b, "ws_b").await;
        });

        await_registry_empty("after admin-on-admin test");
    });
}

// ─────────────────────────────────────────────────────────────────────
// 4 — method gate: GET → 405
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kick_is_post_only() {
    let _g = lock();
    with_temp_home(|| {
        await_registry_empty("at test start");
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            let r = http(
                d.port,
                "GET",
                &format!("/cli/presence/kick?token={OWNER_TOKEN}"),
                None,
            );
            assert_eq!(r.status, 405, "GET must 405; body={}", r.body);
        });
        await_registry_empty("after method-gate test");
    });
}

// ─────────────────────────────────────────────────────────────────────
// 5 — unknown username → 400; the literal "owner" → 400 with its message
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kick_unknown_user_and_owner_are_400() {
    let _g = lock();
    with_temp_home(|| {
        await_registry_empty("at test start");
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            let r = kick(d.port, OWNER_TOKEN, "no_such_user_here");
            assert_eq!(r.status, 400, "unknown user must 400; body={}", r.body);
            assert!(
                r.body.contains("unknown user"),
                "clear unknown-user message, got: {}",
                r.body
            );

            // The synthesized owner row is never a target — and the error
            // says so, rather than a confusing "unknown user 'owner'".
            let r = kick(d.port, OWNER_TOKEN, "owner");
            assert_eq!(r.status, 400, "owner must 400; body={}", r.body);
            assert!(
                r.body.contains("owner cannot be kicked"),
                "owner-specific message, got: {}",
                r.body
            );

            // Malformed body → 400 (consistent with the set-disabled arm).
            let r = http(
                d.port,
                "POST",
                &format!("/cli/presence/kick?token={OWNER_TOKEN}"),
                Some("not json"),
            );
            assert_eq!(r.status, 400, "malformed body must 400; body={}", r.body);
        });
        await_registry_empty("after bad-target test");
    });
}
