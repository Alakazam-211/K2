//! F3 (prd-v1-api-completion §5 / Cloud PRD S3) — `/v1` gate split +
//! capability reporting, driven end-to-end through the REAL dispatcher
//! (`k2_daemon::test_harness::start`, the #630 harness).
//!
//! The contract under test:
//!
//!   1. BOTH gates off (the shipped default) → every `/v1/*` path 404s
//!      as if the surface didn't exist (byte-identical to no surface).
//!   2. `K2_API=1` alone → the surface exists (`/v1/ping` 200 with the
//!      capability object, `/v1/w/<ws>/message` reachable) but the
//!      SANDBOX route families (`/v1/sandboxes*`, `/v1/w/<ws>/sessions*`)
//!      stay surface-absent: the same uniform 404 an unknown /v1 path
//!      gets — never a 405/409 oracle. (409 stays reserved for "API on,
//!      engine can't sandbox" inside the handlers.)
//!   3. Legacy `K2_SANDBOX_API=1` alone (existing Dedicated units) →
//!      everything works as pre-split: it implies the surface AND the
//!      sandbox families.
//!   4. `/boot-status` (UNAUTHENTICATED) carries the `api` capability
//!      object `{enabled, sandboxes:"microvm"|"none"}` in ALL gate
//!      combinations; `/v1/ping` echoes the same object.
//!
//! ISOLATION: `K2_API`/`K2_SANDBOX_API` and `$HOME` are process-wide —
//! every test serializes on `TEST_LOCK`, redirects `$HOME` to a fresh
//! tempdir (so workspace/DB lookups never touch the real ~/.k2), and
//! restores both env vars on drop (presence-suite pattern).

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-api-gate-f3";

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

/// Set/clear the two gate env vars and redirect `$HOME` to a fresh tempdir;
/// restores everything on drop. Caller holds `TEST_LOCK` (the vars are
/// process-wide, and the daemon reads them PER REQUEST — no reboot needed
/// between combinations, but tests must never overlap).
struct GateEnv {
    prev_api: Option<std::ffi::OsString>,
    prev_sbx: Option<std::ffi::OsString>,
    prev_home: Option<std::ffi::OsString>,
    tmp_home: std::path::PathBuf,
}

impl GateEnv {
    fn set(k2_api: Option<&str>, k2_sandbox_api: Option<&str>) -> Self {
        let prev_api = std::env::var_os("K2_API");
        let prev_sbx = std::env::var_os("K2_SANDBOX_API");
        let prev_home = std::env::var_os("HOME");
        match k2_api {
            Some(v) => std::env::set_var("K2_API", v),
            None => std::env::remove_var("K2_API"),
        }
        match k2_sandbox_api {
            Some(v) => std::env::set_var("K2_SANDBOX_API", v),
            None => std::env::remove_var("K2_SANDBOX_API"),
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_home =
            std::env::temp_dir().join(format!("k2-api-gate-f3-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&tmp_home).expect("create temp HOME");
        std::env::set_var("HOME", &tmp_home);
        Self { prev_api, prev_sbx, prev_home, tmp_home }
    }
}

impl Drop for GateEnv {
    fn drop(&mut self) {
        fn restore(name: &str, prev: &Option<std::ffi::OsString>) {
            match prev {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
        restore("K2_API", &self.prev_api);
        restore("K2_SANDBOX_API", &self.prev_sbx);
        restore("HOME", &self.prev_home);
        let _ = std::fs::remove_dir_all(&self.tmp_home);
    }
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

/// The FROZEN capability wire shape: exactly `{enabled, hostSessions,
/// sandboxes}` with `hostSessions == enabled` (the F1 host-session family
/// ships with the surface gate itself) and `sandboxes` ∈ {"microvm","none"}
/// matching this build's `can_sandbox()` (a mac / feature-off test build →
/// "none"; asserted, not assumed).
fn assert_capability(cap: &serde_json::Value, want_enabled: bool, ctx: &str) {
    assert_eq!(
        cap["enabled"],
        serde_json::json!(want_enabled),
        "{ctx}: api.enabled mismatch; api={cap}"
    );
    assert_eq!(
        cap["hostSessions"],
        serde_json::json!(want_enabled),
        "{ctx}: api.hostSessions ships with the surface gate (F1); api={cap}"
    );
    let expect_tier =
        if k2_daemon::v2_spawn::can_sandbox() { "microvm" } else { "none" };
    assert_eq!(
        cap["sandboxes"],
        serde_json::json!(expect_tier),
        "{ctx}: api.sandboxes must come from can_sandbox(); api={cap}"
    );
    assert_eq!(
        cap.as_object().map(|o| o.len()),
        Some(3),
        "{ctx}: capability object is FROZEN wire shape (exactly enabled+hostSessions+sandboxes); api={cap}"
    );
}

/// The uniform surface-absent 404 an authenticated caller sees for a gated-off
/// or unknown route INSIDE the /v1 arm (`CliResponse::not_found()`).
const ROUTE_NOT_FOUND: &str = r#"{"error":"route not found"}"#;
/// The outer surface-off 404 (whole /v1 arm dark).
const SURFACE_OFF: &str = r#"{"error":"not found"}"#;

// ─────────────────────────────────────────────────────────────────────
// (a) Both gates off → the whole surface is absent.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn both_gates_off_whole_v1_surface_404s() {
    let _g = lock();
    let _env = GateEnv::set(None, None);
    let d = test_harness::start(OWNER_TOKEN).await;

    // Even a fully authenticated owner gets the outer surface-off 404 —
    // BEFORE auth, so unauthenticated probes see the identical response.
    for (method, path, body) in [
        ("GET", format!("/v1/ping?token={OWNER_TOKEN}"), None),
        ("GET", "/v1/ping".to_string(), None),
        ("POST", format!("/v1/sandboxes?token={OWNER_TOKEN}"), Some("{}")),
        ("GET", format!("/v1/sandboxes/abc/messages?token={OWNER_TOKEN}"), None),
        ("POST", format!("/v1/w/x/sessions?token={OWNER_TOKEN}"), Some("{}")),
        ("POST", format!("/v1/w/x/message?token={OWNER_TOKEN}"), Some(r#"{"text":"hi"}"#)),
    ] {
        let r = http(d.port, method, &path, body);
        assert_eq!(r.status, 404, "{method} {path} must be surface-absent; body={}", r.body);
        assert_eq!(
            r.body, SURFACE_OFF,
            "{method} {path}: gate-off body is the uniform outer 404"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────
// (b) K2_API=1 alone → surface on; sandbox families stay absent.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn k2_api_alone_ping_carries_capability_object() {
    let _g = lock();
    let _env = GateEnv::set(Some("1"), None);
    let d = test_harness::start(OWNER_TOKEN).await;

    let r = http(d.port, "GET", &format!("/v1/ping?token={OWNER_TOKEN}"), None);
    assert_eq!(r.status, 200, "K2_API=1 alone must expose /v1/ping; body={}", r.body);
    let v = json(&r.body);
    assert_eq!(v["ok"], serde_json::json!(true), "body={}", r.body);
    assert_eq!(v["principal"], serde_json::json!("owner"), "body={}", r.body);
    assert_capability(&v["api"], true, "/v1/ping");
    assert_eq!(
        v.as_object().map(|o| o.len()),
        Some(3),
        "/v1/ping body is FROZEN wire shape (exactly ok+principal+api); body={}",
        r.body
    );

    // The auth tier is live: no token → 401 (not the surface-off 404).
    let unauth = http(d.port, "GET", "/v1/ping", None);
    assert_eq!(unauth.status, 401, "surface on ⇒ unauthenticated ping is 401; body={}", unauth.body);
}

#[tokio::test(flavor = "multi_thread")]
async fn k2_api_alone_sandbox_families_are_surface_absent() {
    let _g = lock();
    let _env = GateEnv::set(Some("1"), None);
    let d = test_harness::start(OWNER_TOKEN).await;

    // The uniformity yardstick: an UNKNOWN /v1 path for an authenticated
    // caller. Every gated-off sandbox route must be indistinguishable.
    let unknown = http(
        d.port,
        "GET",
        &format!("/v1/definitely-not-a-route?token={OWNER_TOKEN}"),
        None,
    );
    assert_eq!(unknown.status, 404);
    assert_eq!(unknown.body, ROUTE_NOT_FOUND);

    for (method, path, body) in [
        // The spawn route: 404, NOT the handler's 409-can't-sandbox (the
        // family is absent — the handler must never run).
        ("POST", format!("/v1/sandboxes?token={OWNER_TOKEN}"), Some("{}")),
        // A stray GET on the POST-only spawn route: 404, NOT a 405 oracle.
        ("GET", format!("/v1/sandboxes?token={OWNER_TOKEN}"), None),
        ("GET", format!("/v1/sandboxes/abc/messages?token={OWNER_TOKEN}"), None),
        // The workspace sandbox-session family, all four shapes.
        ("POST", format!("/v1/w/x/sessions?token={OWNER_TOKEN}"), Some("{}")),
        ("GET", format!("/v1/w/x/sessions?token={OWNER_TOKEN}"), None),
        ("POST", format!("/v1/w/x/sessions/sid?token={OWNER_TOKEN}"), Some("{}")),
        ("GET", format!("/v1/w/x/sessions/sid/messages?token={OWNER_TOKEN}"), None),
    ] {
        let r = http(d.port, method, &path, body);
        assert_eq!(
            r.status, 404,
            "{method} {path}: sandbox family must be surface-absent under K2_API alone; body={}",
            r.body
        );
        assert_eq!(
            r.body, ROUTE_NOT_FOUND,
            "{method} {path}: must be byte-identical to an unknown /v1 route"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn k2_api_alone_ws_message_route_is_reachable() {
    let _g = lock();
    let _env = GateEnv::set(Some("1"), None);
    let d = test_harness::start(OWNER_TOKEN).await;

    // Reachability proof #1: unauthenticated → 401 from the /v1 auth tier.
    // (With the surface off this exact request is a 404, so a 401 proves the
    // route family exists under K2_API alone.)
    let unauth = http(d.port, "POST", "/v1/w/x/message", Some(r#"{"text":"hi"}"#));
    assert_eq!(
        unauth.status, 401,
        "K2_API alone must expose /v1/w/<ws>/message to the auth tier; body={}",
        unauth.body
    );
    assert_ne!(unauth.body, SURFACE_OFF, "must not be the surface-off 404");

    // Reachability proof #2: authenticated against an unknown workspace the
    // HANDLER answers (404 for ws reasons is fine — the point is it is not
    // the outer surface-off 404 and the handler ran). A fresh $HOME has no
    // workspaces, so any 200 here would be a test-isolation failure.
    let authed = http(
        d.port,
        "POST",
        &format!("/v1/w/x/message?token={OWNER_TOKEN}"),
        Some(r#"{"text":"hi"}"#),
    );
    assert!(
        authed.status == 404 || authed.status == 400,
        "handler must answer (ws-unknown 404 / bad-request 400), got {}; body={}",
        authed.status,
        authed.body
    );
    assert_ne!(authed.body, SURFACE_OFF, "must not be the outer surface-off 404");
}

// ─────────────────────────────────────────────────────────────────────
// (c) Legacy K2_SANDBOX_API=1 alone → everything as pre-split.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn legacy_sandbox_var_alone_keeps_full_surface_working() {
    let _g = lock();
    let _env = GateEnv::set(None, Some("1"));
    let d = test_harness::start(OWNER_TOKEN).await;

    // Ping works (the legacy var implies the surface) and reports capability.
    let r = http(d.port, "GET", &format!("/v1/ping?token={OWNER_TOKEN}"), None);
    assert_eq!(r.status, 200, "legacy K2_SANDBOX_API=1 must keep /v1/ping; body={}", r.body);
    let v = json(&r.body);
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_capability(&v["api"], true, "/v1/ping (legacy gate)");

    // The sandbox family is PRESENT: on a build that cannot sandbox the spawn
    // route answers with the handler's 409 refusal (NOT a surface 404) —
    // exactly today's pre-split behavior. Premise-guard the build first.
    assert!(
        !k2_daemon::v2_spawn::can_sandbox(),
        "test premise: this (mac / feature-off) build cannot sandbox"
    );
    let spawn = http(d.port, "POST", &format!("/v1/sandboxes?token={OWNER_TOKEN}"), Some("{}"));
    assert_eq!(
        spawn.status, 409,
        "sandbox family must be present under the legacy var (409 = handler ran); body={}",
        spawn.body
    );

    // The workspace sandbox-session family is routed too (unknown ws → the
    // HANDLER's uniform 404, not the outer surface-off body).
    let ws = http(d.port, "GET", &format!("/v1/w/x/sessions?token={OWNER_TOKEN}"), None);
    assert_eq!(ws.status, 404, "unknown ws answers 404; body={}", ws.body);
    assert_ne!(ws.body, SURFACE_OFF, "handler-level 404, not the outer gate");

    // Auth tier live: unauthenticated → 401.
    let unauth = http(d.port, "GET", "/v1/ping", None);
    assert_eq!(unauth.status, 401, "body={}", unauth.body);
}

// ─────────────────────────────────────────────────────────────────────
// (d) /boot-status carries the api capability object in ALL combos.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn boot_status_reports_api_capability_in_all_gate_combinations() {
    let _g = lock();
    // (gate-env, expected enabled). The daemon reads the gates per request,
    // so one daemon serves all four combinations.
    let combos: [(Option<&str>, Option<&str>, bool); 4] = [
        (None, None, false),
        (Some("1"), None, true),
        (None, Some("1"), true),
        (Some("1"), Some("1"), true),
    ];
    for (api, sbx, want_enabled) in combos {
        let env = GateEnv::set(api, sbx);
        let d = test_harness::start(OWNER_TOKEN).await;

        // /boot-status is UNAUTHENTICATED — no token in the query.
        let r = http(d.port, "GET", "/boot-status", None);
        assert_eq!(r.status, 200, "boot-status must answer; body={}", r.body);
        let v = json(&r.body);
        let ctx = format!("/boot-status (K2_API={api:?}, K2_SANDBOX_API={sbx:?})");
        assert_capability(&v["api"], want_enabled, &ctx);

        // The pre-F3 handshake fields survive untouched (extend, don't
        // loosen: the lifecycle suite asserts these too).
        assert_eq!(
            v["version"],
            serde_json::json!(env!("CARGO_PKG_VERSION")),
            "{ctx}: version intact; body={}",
            r.body
        );
        assert_eq!(v["phase"], serde_json::json!("ready"), "{ctx}: body={}", r.body);
        assert!(v["protocol"].is_number(), "{ctx}: protocol intact; body={}", r.body);

        drop(env);
    }
}
