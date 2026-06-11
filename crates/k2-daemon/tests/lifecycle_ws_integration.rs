//! P3 — Lifecycle + WS integration regression tests.
//!
//! These drive REAL HTTP requests through the REAL
//! `routes::dispatcher::dispatch` (started in-process on an ephemeral
//! 127.0.0.1 port by `k2_daemon::test_harness::start`) — the same
//! harness the #630 auth-route suite uses — and assert the lifecycle +
//! WS-gate contract:
//!
//!   - `/boot-status` SHAPE: the versioned readiness handshake JSON
//!     (`version` / `protocol` / `phase` / `detail`) the renderer's
//!     ConnectionGate polls to decide whether to mount.
//!   - 503 readiness gate: while `phase != ready` the dispatcher must
//!     503 every real `/cli/*` route (but keep `/ping`, `/health`,
//!     `/boot-status` answering) so no handler runs against
//!     half-migrated state.
//!   - WS token gate BEFORE upgrade: `/events`, `/cli/sessions/subscribe`,
//!     and `/cli/awareness/subscribe` must reject a missing/garbage token
//!     with an HTTP 403 — NOT a dangling WS upgrade.
//!
//! ISOLATION: `boot_status` (phase) + the connect-users stores are
//! process-wide singletons. Each `tests/*.rs` integration file is its
//! OWN test binary/process, so this file does not race the #630 file's
//! global state. Within this file we still serialize on `TEST_LOCK` and
//! always restore `phase = ready` after a 503 test so later tests in the
//! same process see a ready daemon.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex as StdMutex;

use k2_daemon::{boot_status, test_harness};

/// Serialize: the global boot phase + connect-users state are
/// process-wide. The 503 test in particular flips `phase` to migrating
/// and back, which would corrupt a concurrently-running test.
static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-p3";

/// A minimal parsed HTTP response: the numeric status + the body.
struct Resp {
    status: u16,
    body: String,
}

/// Fire one raw HTTP request at `127.0.0.1:<port>` and return the parsed
/// status + body. Synchronous + dependency-free. Mirrors the #630
/// auth-route harness's `http` helper (kept local so the two suites stay
/// independent).
fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
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
    let status_line = text.lines().next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));
    let body = match text.split_once("\r\n\r\n") {
        Some((_headers, b)) => b.to_string(),
        None => String::new(),
    };
    Resp { status, body }
}

/// Try to parse a (possibly partial) HTTP/1.1 response. Returns
/// `(status, body, complete)` once status line + full headers are
/// present; `complete` is true when the body reached Content-Length.
fn try_parse(raw: &[u8]) -> Option<(u16, String, bool)> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())?;
    let content_len = headers.lines().find_map(|l| {
        let lower = l.to_ascii_lowercase();
        lower
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let complete = match content_len {
        Some(clen) => body.len() >= clen,
        None => true,
    };
    Some((status, body.to_string(), complete))
}

/// Block on a future from inside a sync helper invoked within a
/// `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`.
fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

// ─────────────────────────────────────────────────────────────────────
// Group 1 — /boot-status SHAPE (the versioned readiness handshake)
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_status_returns_versioned_handshake_shape() {
    let _g = lock();
    // start() marks phase = ready.
    let d = futures_block(test_harness::start(OWNER_TOKEN));

    // /boot-status is UNAUTHENTICATED — no token in the query.
    let r = http(d.port, "GET", "/boot-status", None);
    assert_eq!(r.status, 200, "boot-status must be 200 (unauthenticated); body={}", r.body);

    let v: serde_json::Value =
        serde_json::from_str(&r.body).expect("boot-status body must be JSON");

    // version — exact build string (the LocalPaired auto-update guard
    // requires it to equal the app's bundled version). Must be present
    // and equal the daemon crate's compiled version.
    assert_eq!(
        v["version"],
        serde_json::json!(env!("CARGO_PKG_VERSION")),
        "boot-status.version must be the daemon's CARGO_PKG_VERSION; body={}",
        r.body
    );

    // protocol — daemon↔client API version (decoupled from marketing
    // version). Must be the numeric PROTOCOL constant.
    assert_eq!(
        v["protocol"],
        serde_json::json!(boot_status::PROTOCOL),
        "boot-status.protocol must equal boot_status::PROTOCOL; body={}",
        r.body
    );
    assert!(v["protocol"].is_u64(), "protocol must be a number; body={}", r.body);

    // phase — ready here (harness called set_ready).
    assert_eq!(
        v["phase"],
        serde_json::json!("ready"),
        "phase must be 'ready' for a started harness daemon; body={}",
        r.body
    );

    // detail — present (string), empty when ready.
    assert!(v["detail"].is_string(), "detail must be a string field; body={}", r.body);
    assert_eq!(
        v["detail"],
        serde_json::json!(""),
        "detail is empty when ready; body={}",
        r.body
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_status_reports_migrating_phase_and_detail() {
    let _g = lock();
    let d = futures_block(test_harness::start(OWNER_TOKEN));

    // Flip the process-global phase to migrating with a detail string and
    // assert /boot-status reflects it live, then restore ready.
    boot_status::set_migrating("Applying updates… (12/64 workspaces)");
    let r = http(d.port, "GET", "/boot-status", None);
    boot_status::set_ready();

    assert_eq!(r.status, 200, "boot-status answers during migration; body={}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("boot-status JSON");
    assert_eq!(
        v["phase"],
        serde_json::json!("migrating"),
        "phase must report 'migrating' while migrating; body={}",
        r.body
    );
    assert_eq!(
        v["detail"],
        serde_json::json!("Applying updates… (12/64 workspaces)"),
        "detail must carry the live migration progress string; body={}",
        r.body
    );
}

// ─────────────────────────────────────────────────────────────────────
// Group 2 — 503 readiness gate on real routes while migrating
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrating_phase_503s_real_routes_but_serves_liveness() {
    let _g = lock();
    let d = futures_block(test_harness::start(OWNER_TOKEN));

    // Enter the migrating phase: every REAL route must 503, even with a
    // valid owner token (the gate runs BEFORE auth + the handler so no
    // handler touches half-migrated state).
    boot_status::set_migrating("first-boot migration");

    // A real /cli/* data route → 503 (NOT 200, NOT 403).
    let r = http(
        d.port,
        "GET",
        &format!("/cli/projects/list?token={OWNER_TOKEN}"),
        None,
    );
    assert_eq!(
        r.status, 503,
        "real /cli/* route must 503 while migrating (even with owner token); body={}",
        r.body
    );
    assert!(
        r.body.contains("migrating"),
        "503 body must signal migration state; body={}",
        r.body
    );

    // The liveness + handshake routes are EXEMPT from the gate.
    let ping = http(d.port, "GET", "/ping", None);
    assert_eq!(ping.status, 200, "/ping must answer while migrating; body={}", ping.body);
    let health = http(d.port, "GET", "/health", None);
    assert_eq!(health.status, 200, "/health must answer while migrating; body={}", health.body);
    let bs = http(d.port, "GET", "/boot-status", None);
    assert_eq!(bs.status, 200, "/boot-status must answer while migrating; body={}", bs.body);

    // Restore ready and confirm the SAME real route now passes the gate
    // (200 with the owner token) — proving the 503 was the gate, not a
    // broken route.
    boot_status::set_ready();
    let r = http(
        d.port,
        "GET",
        &format!("/cli/projects/list?token={OWNER_TOKEN}"),
        None,
    );
    assert_eq!(
        r.status, 200,
        "real /cli/* route must serve once ready; body={}",
        r.body
    );
}

// ─────────────────────────────────────────────────────────────────────
// Group 3 — WS token gate BEFORE upgrade
//
// Each WS upgrade route must reject a missing/garbage token with an HTTP
// 403 (not a dangling WS upgrade) BEFORE handing off to the WS handler.
// The existing awareness_ws_integration.rs drives the HANDLER directly
// (post-gate), so the dispatcher's pre-upgrade gate is NOT covered there.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_ws_rejects_missing_and_garbage_token_before_upgrade() {
    let _g = lock();
    let d = futures_block(test_harness::start(OWNER_TOKEN));

    // No token → 403 (a plain HTTP forbidden, not a WS 101 upgrade).
    let r = http(d.port, "GET", "/events", None);
    assert_eq!(r.status, 403, "/events with no token must 403 before upgrade; body={}", r.body);

    // Garbage token → 403.
    let r = http(d.port, "GET", "/events?token=not-a-real-token", None);
    assert_eq!(r.status, 403, "/events garbage token must 403 before upgrade; body={}", r.body);

    // NOTE: the owner-token (gate-PASS) leg is intentionally NOT asserted
    // here. With a valid token the dispatcher hands off to
    // `tokio_tungstenite::accept_async`, which — since this plain-HTTP
    // harness sends no `Connection: Upgrade`/`Sec-WebSocket-Key` headers —
    // fails the handshake and closes the socket WITHOUT emitting any
    // parseable HTTP response. The gate behavior under test (403 before
    // upgrade for an unauthenticated client) is fully covered by the two
    // assertions above; the harness cannot observe the post-gate upgrade
    // path without driving a real WS client (see awareness_ws_integration
    // for the handler-level upgrade test).
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_subscribe_ws_rejects_missing_and_garbage_token_before_upgrade() {
    let _g = lock();
    let d = futures_block(test_harness::start(OWNER_TOKEN));

    let r = http(d.port, "GET", "/cli/sessions/subscribe", None);
    assert_eq!(
        r.status, 403,
        "/cli/sessions/subscribe with no token must 403 before upgrade; body={}",
        r.body
    );

    let r = http(d.port, "GET", "/cli/sessions/subscribe?token=garbage", None);
    assert_eq!(
        r.status, 403,
        "/cli/sessions/subscribe garbage token must 403 before upgrade; body={}",
        r.body
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn awareness_subscribe_ws_rejects_missing_and_garbage_token_before_upgrade() {
    let _g = lock();
    let d = futures_block(test_harness::start(OWNER_TOKEN));

    let r = http(d.port, "GET", "/cli/awareness/subscribe", None);
    assert_eq!(
        r.status, 403,
        "/cli/awareness/subscribe with no token must 403 before upgrade; body={}",
        r.body
    );

    let r = http(d.port, "GET", "/cli/awareness/subscribe?token=garbage", None);
    assert_eq!(
        r.status, 403,
        "/cli/awareness/subscribe garbage token must 403 before upgrade; body={}",
        r.body
    );
}
