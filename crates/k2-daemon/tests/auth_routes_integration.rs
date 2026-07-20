//! #630 — daemon AUTH-ROUTE integration regression tests.
//!
//! These drive REAL HTTP requests through the REAL
//! `routes::dispatcher::dispatch` (started in-process on an ephemeral
//! 127.0.0.1 port by `k2_daemon::test_harness::start`) and assert the
//! status code + body for the full dispatch + auth-gate + handler stack.
//! This is the highest-value coverage: it locks the authorization
//! contract every `/cli/*`, `/cli/auth/*`, `/cli/users/*`, and
//! `/cli/tunnel/*` route depends on (including the 0.39.20 / 847a830
//! catchall-accepts-session fix and the #629 role matrix).
//!
//! HARNESS DESIGN (see report): the daemon's `dispatch` fn + its handler
//! tree only compile inside the crate (every `crate::*` path resolves
//! against the binary's private `mod` graph). `lib.rs` is an INDEPENDENT
//! parallel compilation of the same source files; #630 enlarged it to
//! mirror the full module set + `DaemonState`/`BANNER` + a
//! `test_harness::start` that binds an ephemeral listener and spawns the
//! real accept loop. No runtime behavior is added to the production
//! binary. We then talk to it over a raw loopback TCP socket (no extra
//! deps) and assert status + body.
//!
//! ISOLATION: every test serializes on `TEST_LOCK` (the in-memory
//! connect-users session/lockout stores + the on-disk connect-users.json
//! are process-wide singletons) and points `$HOME` at a fresh tempdir so
//! `connect-users.json` writes never touch the real store. The in-memory
//! DB (`db::init_for_tests`) backs the catchall data routes.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex as StdMutex;

use k2_core::connect_users::{self, Role};
use k2_core::session::SessionId;
use k2_daemon::session_token::{self, HookPrincipal};
use k2_daemon::test_harness;

/// Serialize: connect-users sessions/lockouts (in-memory singletons),
/// the on-disk store, `$HOME`, and the shared in-memory DB are all
/// process-wide. Parallel tests would trample each other.
static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A minimal parsed HTTP response: the numeric status + the body +
/// optional Set-Cookie header value (hosted web cookie auth tests).
struct Resp {
    status: u16,
    body: String,
    /// Raw `Set-Cookie` header value (first one), if any.
    set_cookie: Option<String>,
}

/// Fire one raw HTTP request at `127.0.0.1:<port>` and return the parsed
/// status + body. Synchronous + dependency-free; the daemon's accept loop
/// services it on its own spawned task. `body` is `None` for GET (no
/// Content-Length / body sent), `Some(json)` for POST.
fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    http_with_headers(port, method, path_and_query, body, &[])
}

/// Like [`http`] but appends extra request header lines (each `"Name: value"`,
/// no trailing CRLF). Used by the hosted-web cookie auth tests for
/// `Cookie:` / `X-K2-Client:` / `X-Forwarded-Proto:`.
fn http_with_headers(
    port: u16,
    method: &str,
    path_and_query: &str,
    body: Option<&str>,
    extra_headers: &[&str],
) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("set read timeout");

    // NOTE: we deliberately do NOT send `Connection: close`. Some GET
    // branches in the dispatcher (e.g. /cli/tunnel/config) respond without
    // consuming the peeked request bytes; closing the client side while the
    // server still has unread bytes queued can surface as a RST
    // (ConnectionReset) on macOS before we finish reading the response. By
    // letting the server keep the socket alive and reading exactly
    // Content-Length bytes, we read the full response then drop the socket
    // ourselves — robust regardless of whether a given arm drains the body.
    let mut extra = String::new();
    for h in extra_headers {
        extra.push_str(h);
        extra.push_str("\r\n");
    }
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Content-Type: application/json\r\n\
             {extra}\
             Content-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!(
            "{method} {path_and_query} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             {extra}\
             \r\n"
        ),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");

    // Read until we have headers + the full Content-Length body. Tolerate a
    // mid-read RST/EOF: if we've already parsed a complete response, return
    // it; only panic if we got nothing usable.
    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some((status, body, set_cookie, complete)) = try_parse(&raw) {
            if complete {
                return Resp {
                    status,
                    body,
                    set_cookie,
                };
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => break, // clean EOF
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
    // "HTTP/1.1 200 OK" → 200
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));

    let (headers, body) = match text.split_once("\r\n\r\n") {
        Some((h, b)) => (h, b.to_string()),
        None => ("", String::new()),
    };
    let set_cookie = headers.lines().find_map(|l| {
        let lower = l.to_ascii_lowercase();
        lower
            .strip_prefix("set-cookie:")
            .map(|_| l.split_once(':').map(|(_, v)| v.trim().to_string()).unwrap())
    });
    Resp {
        status,
        body,
        set_cookie,
    }
}

/// Try to parse a (possibly partial) HTTP/1.1 response out of `raw`.
/// Returns `(status, body, set_cookie, complete)` once the status line +
/// full headers are present; `complete` is true when the body has reached
/// the advertised Content-Length (or no Content-Length header is present).
fn try_parse(raw: &[u8]) -> Option<(u16, String, Option<String>, bool)> {
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
    let set_cookie = headers.lines().find_map(|l| {
        if l.to_ascii_lowercase().starts_with("set-cookie:") {
            l.split_once(':').map(|(_, v)| v.trim().to_string())
        } else {
            None
        }
    });
    let complete = match content_len {
        Some(clen) => body.len() >= clen,
        None => true,
    };
    Some((status, body.to_string(), set_cookie, complete))
}

/// Redirect `$HOME` to a fresh tempdir (so connect-users.json writes are
/// isolated) and run `f`, restoring `$HOME` after. The caller already
/// holds `TEST_LOCK`. The in-memory DB is initialized once per process.
fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!(
        "k2so-630-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);
    // In-memory DB (process-wide singleton) so the catchall data routes
    // (/cli/projects/list etc.) have a real connection to read.
    let _ = k2_core::db::init_for_tests();

    f();

    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Seed a connect-user with a role and return a live session token for it.
fn seed_user_session(username: &str, password: &str, role: Role) -> String {
    connect_users::add_user(username, password).expect("add_user");
    connect_users::set_role(username, role).expect("set_role");
    connect_users::create_session(username)
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-630";

// ─────────────────────────────────────────────────────────────────────
// Group 1 — generic /cli/* catchall accepts a connect-user SESSION
// (locks the 0.39.20 / 847a830 fix). A Member session must reach the
// general data routes; no token / garbage token must 403.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catchall_accepts_member_session_for_projects_list() {
    let _g = lock();
    with_temp_home(|| {
        let member = seed_user_session("member1", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Member session → 200 (NOT 403). This is the catchall fix.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/projects/list?token={member}"),
            None,
        );
        assert_eq!(
            r.status, 200,
            "member session must reach /cli/projects/list (catchall fix); body={}",
            r.body
        );

        // Owner token also 200.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/projects/list?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(r.status, 200, "owner token must reach catchall; body={}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catchall_accepts_member_session_for_fs_read_dir() {
    let _g = lock();
    with_temp_home(|| {
        let member = seed_user_session("member2", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let dir = std::env::temp_dir();
        let r = http(
            d.port,
            "GET",
            &format!(
                "/cli/fs/read-dir?token={member}&path={}",
                urlencode(dir.to_str().unwrap())
            ),
            None,
        );
        assert_eq!(
            r.status, 200,
            "member session must reach /cli/fs/read-dir; body={}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catchall_rejects_missing_and_garbage_token() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        // No token → 403.
        let r = http(d.port, "GET", "/cli/projects/list", None);
        assert_eq!(r.status, 403, "no token must 403; body={}", r.body);
        // Garbage token → 403.
        let r = http(d.port, "GET", "/cli/projects/list?token=not-a-real-token", None);
        assert_eq!(r.status, 403, "garbage token must 403; body={}", r.body);
    });
}

// ─────────────────────────────────────────────────────────────────────
// Group 2 — #629 role matrix on /cli/users/*
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn users_list_role_matrix() {
    let _g = lock();
    with_temp_home(|| {
        let admin = seed_user_session("admin1", "password123", Role::Admin);
        let member = seed_user_session("member3", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Owner token → 200.
        let r = http(d.port, "GET", &format!("/cli/users?token={OWNER_TOKEN}"), None);
        assert_eq!(r.status, 200, "owner lists users; body={}", r.body);
        // Admin session → 200.
        let r = http(d.port, "GET", &format!("/cli/users?token={admin}"), None);
        assert_eq!(r.status, 200, "admin lists users; body={}", r.body);
        // Member session → 403.
        let r = http(d.port, "GET", &format!("/cli/users?token={member}"), None);
        assert_eq!(r.status, 403, "member must NOT list users; body={}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn users_add_role_matrix_and_creates_member() {
    let _g = lock();
    with_temp_home(|| {
        let admin = seed_user_session("admin2", "password123", Role::Admin);
        let member = seed_user_session("member4", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Member session → 403 (cannot add).
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/add?token={member}"),
            Some(r#"{"username":"newby1","password":"password123"}"#),
        );
        assert_eq!(r.status, 403, "member must NOT add users; body={}", r.body);

        // Admin session → 200, and the new user is created as a Member.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/add?token={admin}"),
            Some(r#"{"username":"newby2","password":"password123"}"#),
        );
        assert_eq!(r.status, 200, "admin adds a user; body={}", r.body);
        assert_eq!(
            connect_users::role_for_user("newby2"),
            Some(Role::Member),
            "newly added user defaults to Member"
        );

        // Owner token → 200.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/add?token={OWNER_TOKEN}"),
            Some(r#"{"username":"newby3","password":"password123"}"#),
        );
        assert_eq!(r.status, 200, "owner adds a user; body={}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn users_remove_is_owner_only() {
    let _g = lock();
    with_temp_home(|| {
        // Targets to remove.
        connect_users::add_user("victim_a", "password123").expect("add victim_a");
        connect_users::add_user("victim_b", "password123").expect("add victim_b");
        let admin = seed_user_session("admin3", "password123", Role::Admin);
        let member = seed_user_session("member5", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Member → 403.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/remove?token={member}"),
            Some(r#"{"username":"victim_a"}"#),
        );
        assert_eq!(r.status, 403, "member must NOT remove; body={}", r.body);

        // Admin → 403 (remove is owner-only / can_change_roles).
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/remove?token={admin}"),
            Some(r#"{"username":"victim_a"}"#),
        );
        assert_eq!(r.status, 403, "admin must NOT remove (owner-only); body={}", r.body);
        assert!(
            connect_users::role_for_user("victim_a").is_some(),
            "victim_a must still exist after the rejected removes"
        );

        // Owner token → 200.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/remove?token={OWNER_TOKEN}"),
            Some(r#"{"username":"victim_a"}"#),
        );
        assert_eq!(r.status, 200, "owner removes; body={}", r.body);
        assert!(
            connect_users::role_for_user("victim_a").is_none(),
            "victim_a must be gone after owner remove"
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn users_set_role_is_owner_only() {
    let _g = lock();
    with_temp_home(|| {
        connect_users::add_user("target1", "password123").expect("add target1");
        let admin = seed_user_session("admin4", "password123", Role::Admin);
        let member = seed_user_session("member6", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Member → 403.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/set-role?token={member}"),
            Some(r#"{"username":"target1","role":"admin"}"#),
        );
        assert_eq!(r.status, 403, "member must NOT set-role; body={}", r.body);

        // Admin → 403 (set-role is owner-only via can_change_roles).
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/set-role?token={admin}"),
            Some(r#"{"username":"target1","role":"admin"}"#),
        );
        assert_eq!(r.status, 403, "admin must NOT set-role; body={}", r.body);
        assert_eq!(
            connect_users::role_for_user("target1"),
            Some(Role::Member),
            "target1 role unchanged after rejected set-role"
        );

        // Owner token → 200.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/set-role?token={OWNER_TOKEN}"),
            Some(r#"{"username":"target1","role":"admin"}"#),
        );
        assert_eq!(r.status, 200, "owner sets role; body={}", r.body);
        assert_eq!(connect_users::role_for_user("target1"), Some(Role::Admin));
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn users_set_disabled_admin_can_act_on_member_but_not_owner() {
    let _g = lock();
    with_temp_home(|| {
        // A Member target and an Owner-role target.
        connect_users::add_user("dtarget_member", "password123").expect("add member target");
        connect_users::add_user("dtarget_owner", "password123").expect("add owner target");
        connect_users::set_role("dtarget_owner", Role::Owner).expect("promote owner target");
        let admin = seed_user_session("admin5", "password123", Role::Admin);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Admin disabling a Member → 200.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/set-disabled?token={admin}"),
            Some(r#"{"username":"dtarget_member","disabled":true}"#),
        );
        assert_eq!(
            r.status, 200,
            "admin may disable a Member target; body={}",
            r.body
        );

        // Admin disabling an Owner-role target → 403 (can_act_on).
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/set-disabled?token={admin}"),
            Some(r#"{"username":"dtarget_owner","disabled":true}"#),
        );
        assert_eq!(
            r.status, 403,
            "admin must NOT disable an Owner-role target; body={}",
            r.body
        );

        // Admin disabling an Admin target → 200 (can_act_on Admin->Admin).
        connect_users::add_user("dtarget_admin", "password123").expect("add admin target");
        connect_users::set_role("dtarget_admin", Role::Admin).expect("promote admin target");
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/set-disabled?token={admin}"),
            Some(r#"{"username":"dtarget_admin","disabled":true}"#),
        );
        assert_eq!(
            r.status, 200,
            "admin may disable an Admin target; body={}",
            r.body
        );
    });
}

// ─────────────────────────────────────────────────────────────────────
// Group 3 — POST-only method gate on /cli/users/*
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn users_add_get_is_405_post_is_200() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        // GET on a POST-only mutating route → 405 (must NOT add).
        let r = http(
            d.port,
            "GET",
            &format!("/cli/users/add?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(
            r.status, 405,
            "GET /cli/users/add must be 405 (POST-only gate); body={}",
            r.body
        );
        // POST → 200.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/add?token={OWNER_TOKEN}"),
            Some(r#"{"username":"postonly1","password":"password123"}"#),
        );
        assert_eq!(r.status, 200, "POST /cli/users/add must be 200; body={}", r.body);
    });
}

// ─────────────────────────────────────────────────────────────────────
// Group 4 — /cli/tunnel/* gating
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tunnel_config_get_accepts_session_post_owner_only() {
    let _g = lock();
    with_temp_home(|| {
        let member = seed_user_session("tmember", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // GET config → token_ok (a session may READ the redacted view).
        let r = http(
            d.port,
            "GET",
            &format!("/cli/tunnel/config?token={member}"),
            None,
        );
        assert_eq!(
            r.status, 200,
            "session may READ tunnel config (GET, token_ok); body={}",
            r.body
        );
        // GET config with owner token → 200.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/tunnel/config?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(r.status, 200, "owner reads tunnel config; body={}", r.body);

        // POST config with a session → 403 (owner-only write).
        let r = http(
            d.port,
            "POST",
            &format!("/cli/tunnel/config?token={member}"),
            Some(r#"{"subdomain":"hijack"}"#),
        );
        assert_eq!(
            r.status, 403,
            "session must NOT write tunnel config (POST owner-only); body={}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tunnel_start_stop_are_owner_only() {
    let _g = lock();
    with_temp_home(|| {
        let member = seed_user_session("tmember2", "password123", Role::Member);
        let admin = seed_user_session("tadmin", "password123", Role::Admin);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // A Member session → 403 on start.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/tunnel/start?token={member}"),
            Some(""),
        );
        assert_eq!(r.status, 403, "member must NOT start tunnel; body={}", r.body);

        // An Admin session → still 403 (tunnel control is OWNER-token-only,
        // not merely can_manage_users — require_owner uses token_is_owner).
        let r = http(
            d.port,
            "POST",
            &format!("/cli/tunnel/stop?token={admin}"),
            Some(""),
        );
        assert_eq!(
            r.status, 403,
            "admin session must NOT stop tunnel (owner-token-only); body={}",
            r.body
        );

        // Stop with the owner token → NOT a 403/405 (owner is authorized;
        // the action itself may 200 or 400 depending on whether a tunnel is
        // running, but it passes the gate). We assert it is NOT rejected by
        // the auth/method gate.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/tunnel/stop?token={OWNER_TOKEN}"),
            Some(""),
        );
        assert!(
            r.status != 403 && r.status != 405,
            "owner token must pass the tunnel/stop gate (got {}); body={}",
            r.status,
            r.body
        );
    });
}

// ─────────────────────────────────────────────────────────────────────
// Group 5 — /cli/users/policy gating
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_get_is_owner_or_session_post_owner_only() {
    let _g = lock();
    with_temp_home(|| {
        let member = seed_user_session("pmember", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // GET policy with a session → 200 (authorized read).
        let r = http(
            d.port,
            "GET",
            &format!("/cli/users/policy?token={member}"),
            None,
        );
        assert_eq!(r.status, 200, "session may READ policy; body={}", r.body);
        // GET policy with owner token → 200.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/users/policy?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(r.status, 200, "owner reads policy; body={}", r.body);
        // GET policy with garbage token → 403.
        let r = http(d.port, "GET", "/cli/users/policy?token=garbage", None);
        assert_eq!(r.status, 403, "garbage token must NOT read policy; body={}", r.body);

        // POST policy with a session → 403 (owner-only write).
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/policy?token={member}"),
            Some(r#"{"minLength":8,"requireSpecial":false,"requireNumber":false,"requireUppercase":false}"#),
        );
        assert_eq!(r.status, 403, "session must NOT write policy; body={}", r.body);

        // POST policy with owner token → 200.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/policy?token={OWNER_TOKEN}"),
            Some(r#"{"minLength":10,"requireSpecial":false,"requireNumber":false,"requireUppercase":false}"#),
        );
        assert_eq!(r.status, 200, "owner writes policy; body={}", r.body);
        assert_eq!(connect_users::get_policy().min_length, 10);
    });
}

// ─────────────────────────────────────────────────────────────────────
// Group 6 — /cli/auth/login is PUBLIC; generic 401; lockout
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_is_public_and_succeeds_with_good_creds() {
    let _g = lock();
    with_temp_home(|| {
        connect_users::add_user("loginuser", "password123").expect("add loginuser");
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        // No token in the query — login is PUBLIC.
        let r = http(
            d.port,
            "POST",
            "/cli/auth/login",
            Some(r#"{"username":"loginuser","password":"password123"}"#),
        );
        assert_eq!(r.status, 200, "good creds must 200 (public route); body={}", r.body);
        assert!(r.body.contains("\"token\""), "login 200 returns a token; body={}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_bad_creds_is_generic_401_no_enumeration() {
    let _g = lock();
    with_temp_home(|| {
        connect_users::add_user("realuser", "password123").expect("add realuser");
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Wrong password for an existing user.
        let r = http(
            d.port,
            "POST",
            "/cli/auth/login",
            Some(r#"{"username":"realuser","password":"wrongpass"}"#),
        );
        assert_eq!(r.status, 401, "wrong password → 401; body={}", r.body);
        // Unknown user.
        let r2 = http(
            d.port,
            "POST",
            "/cli/auth/login",
            Some(r#"{"username":"ghostuser","password":"whatever"}"#),
        );
        assert_eq!(r2.status, 401, "unknown user → 401; body={}", r2.body);
        // Both bodies must be the SAME generic message (no enumeration:
        // the response must NOT reveal which of user/password was wrong).
        assert_eq!(
            r.body, r2.body,
            "wrong-password and unknown-user 401s must be byte-identical (no user enumeration)"
        );
        assert!(
            !r.body.to_lowercase().contains("no such user")
                && !r.body.to_lowercase().contains("not found"),
            "401 body must not enumerate users; body={}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_three_fails_then_lockout_blocks_correct_password() {
    let _g = lock();
    with_temp_home(|| {
        connect_users::add_user("lockuser", "rightpass1").expect("add lockuser");
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // 3 failed logins → lockout (threshold is 3).
        for i in 0..3 {
            let r = http(
                d.port,
                "POST",
                "/cli/auth/login",
                Some(r#"{"username":"lockuser","password":"WRONG"}"#),
            );
            assert_eq!(r.status, 401, "failed attempt {i} → 401; body={}", r.body);
        }
        // Now the CORRECT password is still blocked (within the lockout
        // window) — same generic 401, no success token.
        let r = http(
            d.port,
            "POST",
            "/cli/auth/login",
            Some(r#"{"username":"lockuser","password":"rightpass1"}"#),
        );
        assert_eq!(
            r.status, 401,
            "correct password during lockout window must still 401; body={}",
            r.body
        );
        assert!(
            !r.body.contains("\"token\""),
            "lockout response must NOT issue a session token; body={}",
            r.body
        );
        assert!(
            connect_users::is_locked("lockuser"),
            "account must be locked after 3 failures"
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn change_password_lockout_via_repeated_wrong_current() {
    let _g = lock();
    with_temp_home(|| {
        let user = "chpwuser";
        connect_users::add_user(user, "rightpass1").expect("add chpwuser");
        let session = connect_users::create_session(user);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // 3 wrong-current change-password attempts via the self-service
        // route (authorized by the user's own SESSION token) → lockout.
        for i in 0..3 {
            let r = http(
                d.port,
                "POST",
                &format!("/cli/auth/change-password?token={session}"),
                Some(r#"{"currentPassword":"WRONG","newPassword":"brandnewpass"}"#),
            );
            assert_eq!(
                r.status, 401,
                "wrong-current attempt {i} → 401; body={}",
                r.body
            );
        }
        assert!(
            connect_users::is_locked(user),
            "self-service change-password is subject to the same lockout"
        );
    });
}

// ─────────────────────────────────────────────────────────────────────
// Group 7 — /cli/auth/whoami identity
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whoami_owner_token_reports_owner() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let r = http(
            d.port,
            "GET",
            &format!("/cli/auth/whoami?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(r.status, 200, "owner whoami → 200; body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("whoami json");
        assert_eq!(v["owner"], serde_json::json!(true), "owner flag true; body={}", r.body);
        assert_eq!(v["role"], serde_json::json!("owner"), "owner role; body={}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whoami_session_reports_its_role() {
    let _g = lock();
    with_temp_home(|| {
        let admin = seed_user_session("whoadmin", "password123", Role::Admin);
        let member = seed_user_session("whomember", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        let r = http(d.port, "GET", &format!("/cli/auth/whoami?token={admin}"), None);
        assert_eq!(r.status, 200, "admin whoami → 200; body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("admin whoami json");
        assert_eq!(v["owner"], serde_json::json!(false), "session is not owner");
        assert_eq!(v["role"], serde_json::json!("admin"), "admin role; body={}", r.body);
        assert_eq!(v["username"], serde_json::json!("whoadmin"));

        let r = http(d.port, "GET", &format!("/cli/auth/whoami?token={member}"), None);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("member whoami json");
        assert_eq!(v["role"], serde_json::json!("member"), "member role; body={}", r.body);

        // Garbage token → 403 (forbidden, not an identity).
        let r = http(d.port, "GET", "/cli/auth/whoami?token=garbage", None);
        assert_eq!(r.status, 403, "garbage token whoami → 403; body={}", r.body);
    });
}

// ─────────────────────────────────────────────────────────────────────
// Group 8 — K2 Connect remote-files Phase 2: POST /cli/fs/upload-binary
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_binary_writes_file_and_returns_path() {
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        // A fresh destination dir under the temp HOME so the write is
        // isolated. `$HOME` was redirected by with_temp_home.
        let home = std::env::var("HOME").expect("HOME set by harness");
        let dest = std::path::Path::new(&home).join("up-dest");
        std::fs::create_dir_all(&dest).expect("create dest dir");
        // The handler canonicalizes `dir` (validate_path); on macOS the
        // tempdir under $TMPDIR is symlinked (/var → /private/var), so
        // compare the response against the canonical destination.
        let dest_canon = dest.canonicalize().expect("canonicalize dest");

        let payload = b"upload-test-bytes";
        let body = format!(
            r#"{{"dir":{dir},"filename":"hello.txt","base64":"{b64}"}}"#,
            dir = serde_json::to_string(dest.to_str().unwrap()).unwrap(),
            b64 = B64.encode(payload),
        );
        let r = http(
            d.port,
            "POST",
            &format!("/cli/fs/upload-binary?token={OWNER_TOKEN}"),
            Some(&body),
        );
        assert_eq!(r.status, 200, "owner upload must 200; body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("upload json");
        let written = v["path"].as_str().expect("path in response");
        assert_eq!(written, dest_canon.join("hello.txt").to_str().unwrap());
        assert_eq!(
            std::fs::read(&written).expect("read written file"),
            payload,
            "uploaded bytes must round-trip"
        );

        // A connect-user SESSION (any authed user) is also accepted — the
        // isolated gate is `token_ok`.
        let member = seed_user_session("upmember", "password123", Role::Member);
        let body2 = format!(
            r#"{{"dir":{dir},"filename":"member.txt","base64":"{b64}"}}"#,
            dir = serde_json::to_string(dest.to_str().unwrap()).unwrap(),
            b64 = B64.encode(b"member-bytes"),
        );
        let r = http(
            d.port,
            "POST",
            &format!("/cli/fs/upload-binary?token={member}"),
            Some(&body2),
        );
        assert_eq!(r.status, 200, "member session upload must 200; body={}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_binary_rejects_missing_and_garbage_token() {
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let home = std::env::var("HOME").expect("HOME set");
        let dest = std::path::Path::new(&home).join("up-dest-noauth");
        std::fs::create_dir_all(&dest).expect("create dest dir");
        let body = format!(
            r#"{{"dir":{dir},"filename":"x.txt","base64":"{b64}"}}"#,
            dir = serde_json::to_string(dest.to_str().unwrap()).unwrap(),
            b64 = B64.encode(b"nope"),
        );
        // No token → 403.
        let r = http(d.port, "POST", "/cli/fs/upload-binary", Some(&body));
        assert_eq!(r.status, 403, "no token must 403; body={}", r.body);
        // Garbage token → 403.
        let r = http(
            d.port,
            "POST",
            "/cli/fs/upload-binary?token=not-real",
            Some(&body),
        );
        assert_eq!(r.status, 403, "garbage token must 403; body={}", r.body);
        // Nothing was written by the rejected requests.
        assert!(!dest.join("x.txt").exists(), "rejected upload must not write");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_binary_get_does_not_mutate() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        // A GET can NOT write a file. Because upload-binary is in the
        // `post_allowed` set, a GET isn't 405'd at the top-level gate; it
        // falls through the POST-only arms to the `/cli/` catchall, which
        // has no GET handler for it → 404 "route not found". This is the
        // SAME no-silent-mutation contract as the other Unit 6 fs POST
        // routes (see the dispatcher's Unit-6 arm comment): the status
        // differs from a literal 405 but no write is possible.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/fs/upload-binary?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(
            r.status, 404,
            "GET upload-binary must not mutate (404 via catchall); body={}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_binary_bad_base64_is_400() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let home = std::env::var("HOME").expect("HOME set");
        let dest = std::path::Path::new(&home).join("up-dest-bad");
        std::fs::create_dir_all(&dest).expect("create dest dir");
        let body = format!(
            r#"{{"dir":{dir},"filename":"y.txt","base64":"!!!not-base64!!!"}}"#,
            dir = serde_json::to_string(dest.to_str().unwrap()).unwrap(),
        );
        let r = http(
            d.port,
            "POST",
            &format!("/cli/fs/upload-binary?token={OWNER_TOKEN}"),
            Some(&body),
        );
        assert_eq!(r.status, 400, "garbage base64 must 400; body={}", r.body);
    });
}

// ─────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────

/// Minimal percent-encoding for a filesystem path going into a query
/// string. Only encodes the characters that would break query parsing.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Block on a future from inside a sync helper invoked within a
/// `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`. The harness's `start` is async only because it
/// adopts a tokio listener; we drive it to completion on the current
/// runtime handle.
fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(fut)
    })
}

// ─────────────────────────────────────────────────────────────────────
// Group 5 — #651 supervisor-agnostic daemon restart gating.
//
// CRITICAL SAFETY: the test harness builds `DaemonState` with
// `shutdown_tx: None` (see `k2_daemon::test_harness::start`). That is
// the SEAM: the `POST /cli/daemon/restart` handler returns its 200
// "restarting" ack and then SKIPS the live shutdown trigger because
// `shutdown_tx` is `None`. So NO real restart, SIGTERM, or process kill
// EVER occurs in these tests — the happy-path is asserted as
// "200 + would-restart" without firing it. These tests lock:
//   - the route is in `post_allowed` (a POST is dispatched, not top-level
//     405'd),
//   - a GET is 405 (require_post),
//   - a POST without/with-garbage token is 403 (require_owner_or_admin),
//   - a POST with a Member connect-user SESSION token is 403 (Member is
//     barred — restarting needs the owner-or-admin tier),
//   - a POST with an Owner- OR Admin-role SESSION token is 200 (#660: a
//     remote user restarting the host OVER K2 Connect authorizes with a
//     session token, since the on-box owner token never leaves the box),
//   - a POST with the owner token is 200 with `"restarting":true`
//     (handler reached; NO real restart fires thanks to the None seam).
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_restart_get_is_405() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        // GET on the POST-only restart route → 405 (require_post). A curl
        // GET must never bounce the daemon.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/daemon/restart?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(
            r.status, 405,
            "GET /cli/daemon/restart must be 405 (POST-only gate); body={}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_restart_no_token_is_403() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        // POST without a token → 403 (require_owner).
        let r = http(d.port, "POST", "/cli/daemon/restart", Some(""));
        assert_eq!(
            r.status, 403,
            "POST /cli/daemon/restart with no token must 403; body={}",
            r.body
        );
        // POST with a garbage token → 403.
        let r = http(
            d.port,
            "POST",
            "/cli/daemon/restart?token=not-the-owner-token",
            Some(""),
        );
        assert_eq!(
            r.status, 403,
            "POST /cli/daemon/restart with garbage token must 403; body={}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_restart_rejects_member_session() {
    let _g = lock();
    with_temp_home(|| {
        // A Member connect-user reaches the daemon THROUGH the tunnel but is
        // NOT in the owner-or-admin tier, so it must NOT be able to restart
        // the host. require_owner_or_admin maps the session token → Member
        // role → can_manage_users == false → 403.
        let member = seed_user_session("restart_member", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        let r = http(
            d.port,
            "POST",
            &format!("/cli/daemon/restart?token={member}"),
            Some(""),
        );
        assert_eq!(
            r.status, 403,
            "member session must NOT restart the daemon (owner-or-admin only); body={}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_restart_admin_session_gets_200_would_restart_without_firing() {
    let _g = lock();
    with_temp_home(|| {
        // K2SO #660: an Admin-role connect-user session is the canonical
        // remote-reboot path — a user restarting the host OVER K2 Connect
        // authenticates with a session token, never the on-box owner token.
        // require_owner_or_admin authorizes it; the None shutdown_tx seam
        // means the 200 ack lands WITHOUT firing a real restart.
        let admin = seed_user_session("restart_admin", "password123", Role::Admin);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        let r = http(
            d.port,
            "POST",
            &format!("/cli/daemon/restart?token={admin}"),
            Some(""),
        );
        assert_eq!(
            r.status, 200,
            "admin session POST /cli/daemon/restart must reach the handler (200); body={}",
            r.body
        );
        assert!(
            r.body.contains("\"restarting\":true") && r.body.contains("\"ok\":true"),
            "admin 200 body must be the would-restart ack; body={}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_restart_owner_role_session_gets_200_would_restart_without_firing() {
    let _g = lock();
    with_temp_home(|| {
        // An Owner-ROLE connect-user session (distinct from the on-box owner
        // TOKEN) also authorizes the remote restart. Same None seam → 200 ack
        // without any real restart.
        let owner_sess = seed_user_session("restart_owner", "password123", Role::Owner);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        let r = http(
            d.port,
            "POST",
            &format!("/cli/daemon/restart?token={owner_sess}"),
            Some(""),
        );
        assert_eq!(
            r.status, 200,
            "owner-role session POST /cli/daemon/restart must reach the handler (200); body={}",
            r.body
        );
        assert!(
            r.body.contains("\"restarting\":true") && r.body.contains("\"ok\":true"),
            "owner-role 200 body must be the would-restart ack; body={}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_restart_owner_gets_200_would_restart_without_firing() {
    let _g = lock();
    with_temp_home(|| {
        // The harness's DaemonState has `shutdown_tx: None`, so the handler
        // returns its 200 ack and SKIPS the live shutdown trigger. We assert
        // the happy-path WITHOUT any real restart occurring — if the seam
        // were wired live, this would SIGTERM the test process instead.
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let r = http(
            d.port,
            "POST",
            &format!("/cli/daemon/restart?token={OWNER_TOKEN}"),
            Some(""),
        );
        assert_eq!(
            r.status, 200,
            "owner POST /cli/daemon/restart must reach the handler (200); body={}",
            r.body
        );
        assert!(
            r.body.contains("\"restarting\":true"),
            "200 body must signal would-restart; body={}",
            r.body
        );
        assert!(
            r.body.contains("\"ok\":true"),
            "200 body must be the ok ack; body={}",
            r.body
        );
        // The test process is STILL ALIVE here — proof the None seam
        // prevented a live restart. Reaching this assert is the assertion.
    });
}

// ─────────────────────────────────────────────────────────────────────
// Group 9 — K2SO P3 remote daemon self-UPDATE route gating.
//
// CRITICAL SAFETY: these tests NEVER fire a real download, swap, or
// restart of the daemon.
//   - The auth/method gate runs BEFORE the handler body, so the 403/405
//     cases never reach any network or process code.
//   - For the owner/admin "passes the gate" cases on check/start we point
//     `K2SO_DAEMON_MANIFEST_URL` at a LOCAL in-process stub server (no
//     external network) and assert the request is NOT rejected by the
//     gate (NOT 403/405). `start`'s detached download worker fetches the
//     stub's fake artifact bytes, fails minisign verify, and marks the job
//     `failed` on a BACKGROUND thread — it never touches the test process.
//   - `apply` rides the SAME `shutdown_tx == None` seam as restart: the
//     handler returns its 200 "would-apply" ack and SKIPS the backup +
//     detached helper spawn + shutdown trigger, so NO binary is swapped
//     and the test process is never killed.
//
// These lock:
//   - the three POST routes are in `post_allowed` (POST dispatched, not
//     top-level 405'd) and `require_post`-gated (GET → 405),
//   - all four routes (check/start/status/apply) are owner/admin-gated:
//     member session → 403, no/garbage token → 403,
//     owner token + owner/admin session → pass the gate,
//   - apply with the None seam acks WITHOUT any real swap/restart.
// ─────────────────────────────────────────────────────────────────────

/// Spin a one-shot local HTTP server that answers ANY request with the
/// given JSON body (200). Returns its `http://127.0.0.1:<port>/` base URL.
/// Used as the `daemon-latest.json` source so check/start never reach the
/// public network. The server loops serving the same body to every
/// connection (manifest + the artifact + the `.sig` all get the same
/// bytes; that's fine — the artifact bytes just fail minisign verify on a
/// background thread, which is the safe direction).
fn start_stub_manifest_server(body: &'static str) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind stub server");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            // Read + discard the request (don't care about the path).
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = s.write_all(resp.as_bytes());
            let _ = s.flush();
        }
    });
    format!("http://127.0.0.1:{port}/")
}

const STUB_MANIFEST: &str = r#"{"version":"999.0.0","pub_date":"2026-06-05T00:00:00Z","notes":"stub","artifacts":{"macos-aarch64":{"url":"PLACEHOLDER","sig":"PLACEHOLDER","sha256":"00"},"macos-x86_64":{"url":"PLACEHOLDER","sig":"PLACEHOLDER","sha256":"00"},"linux-x86_64":{"url":"PLACEHOLDER","sig":"PLACEHOLDER","sha256":"00"},"linux-aarch64":{"url":"PLACEHOLDER","sig":"PLACEHOLDER","sha256":"00"}}}"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_check_get_is_405() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        // GET on the POST-only check route → 405 (require_post).
        let r = http(
            d.port,
            "GET",
            &format!("/cli/daemon/update/check?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(
            r.status, 405,
            "GET /cli/daemon/update/check must be 405 (POST-only); body={}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_start_get_is_405() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let r = http(
            d.port,
            "GET",
            &format!("/cli/daemon/update/start?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(r.status, 405, "GET /cli/daemon/update/start must be 405; body={}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_apply_get_is_405() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let r = http(
            d.port,
            "GET",
            &format!("/cli/daemon/update/apply?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(r.status, 405, "GET /cli/daemon/update/apply must be 405; body={}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_routes_reject_member_and_garbage_and_missing_token() {
    let _g = lock();
    with_temp_home(|| {
        let member = seed_user_session("upd_member", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        for route in [
            "/cli/daemon/update/check",
            "/cli/daemon/update/start",
            "/cli/daemon/update/apply",
        ] {
            // Member session → 403 (owner-or-admin only).
            let r = http(d.port, "POST", &format!("{route}?token={member}"), Some("{}"));
            assert_eq!(r.status, 403, "member must NOT use {route}; body={}", r.body);
            // No token → 403.
            let r = http(d.port, "POST", route, Some("{}"));
            assert_eq!(r.status, 403, "no token must 403 on {route}; body={}", r.body);
            // Garbage token → 403.
            let r = http(d.port, "POST", &format!("{route}?token=garbage"), Some("{}"));
            assert_eq!(r.status, 403, "garbage token must 403 on {route}; body={}", r.body);
        }

        // GET status is also owner/admin-gated.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/daemon/update/status?token={member}&job_id=x"),
            None,
        );
        assert_eq!(r.status, 403, "member must NOT read update status; body={}", r.body);
        let r = http(d.port, "GET", "/cli/daemon/update/status?job_id=x", None);
        assert_eq!(r.status, 403, "no token must 403 on status; body={}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_check_owner_and_admin_pass_the_gate() {
    let _g = lock();
    with_temp_home(|| {
        // Point the manifest fetch at a LOCAL stub so no external network is
        // touched. STUB_MANIFEST publishes 999.0.0 with an artifact for this
        // platform, so check returns 200 with available:true.
        let base = start_stub_manifest_server(STUB_MANIFEST);
        std::env::set_var("K2SO_DAEMON_MANIFEST_URL", format!("{base}daemon-latest.json"));

        let admin = seed_user_session("upd_admin", "password123", Role::Admin);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Owner token → 200 (passes the gate; manifest comes from the stub).
        let r = http(
            d.port,
            "POST",
            &format!("/cli/daemon/update/check?token={OWNER_TOKEN}"),
            Some("{}"),
        );
        assert_eq!(r.status, 200, "owner check must 200; body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("check json");
        assert_eq!(v["latest"], serde_json::json!("999.0.0"), "latest from stub; body={}", r.body);
        assert_eq!(v["available"], serde_json::json!(true), "999 is newer; body={}", r.body);

        // Admin session → 200 too (#660 owner-or-admin tier).
        let r = http(
            d.port,
            "POST",
            &format!("/cli/daemon/update/check?token={admin}"),
            Some("{}"),
        );
        assert_eq!(r.status, 200, "admin check must 200; body={}", r.body);

        std::env::remove_var("K2SO_DAEMON_MANIFEST_URL");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_start_owner_gets_job_id_no_real_download() {
    let _g = lock();
    with_temp_home(|| {
        // Stub manifest with an artifact for THIS platform so start enqueues a
        // job. The detached worker will fetch the stub's bytes as the
        // "artifact", FAIL minisign verify, and mark the job failed on a
        // BACKGROUND thread — never touching the test process or the real
        // binary. No external network.
        let base = start_stub_manifest_server(STUB_MANIFEST);
        std::env::set_var("K2SO_DAEMON_MANIFEST_URL", format!("{base}daemon-latest.json"));
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        let r = http(
            d.port,
            "POST",
            &format!("/cli/daemon/update/start?token={OWNER_TOKEN}"),
            Some("{}"),
        );
        // 200 with a job_id (gate passed; manifest resolved; job enqueued).
        // If the stub has no artifact for this platform the handler 400s with
        // "no artifact" — still proves the gate passed, but STUB_MANIFEST
        // covers the common platforms so we expect 200 here.
        assert!(
            r.status == 200 || r.status == 400,
            "owner start must pass the gate (200 job_id, or 400 no-artifact), not 403/405; got {} body={}",
            r.status,
            r.body
        );
        if r.status == 200 {
            let v: serde_json::Value = serde_json::from_str(&r.body).expect("start json");
            assert!(v["job_id"].as_str().is_some(), "200 must carry a job_id; body={}", r.body);
        }

        std::env::remove_var("K2SO_DAEMON_MANIFEST_URL");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_apply_unknown_job_passes_gate_then_400() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        // Owner token passes the gate; an unknown job_id → 400 from the
        // handler (NOT 403/405). The None shutdown_tx seam guarantees no real
        // swap/restart even on the staged path; here we don't even reach it.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/daemon/update/apply?token={OWNER_TOKEN}"),
            Some(r#"{"job_id":"does-not-exist"}"#),
        );
        assert_eq!(
            r.status, 400,
            "owner apply with unknown job → 400 (gate passed, handler rejects); body={}",
            r.body
        );
        assert!(r.body.contains("unknown job_id"), "body should explain; body={}", r.body);
        // Test process is STILL ALIVE — no swap, no restart fired.
    });
}

// ─────────────────────────────────────────────────────────────────────
// Group 10 — #58 Phase-1 close: owner-over-TCP /hook/complete parity (flag
// ON) + the owner-only /cli/daemon/hook-revoke-all kill switch.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hook_complete_owner_token_over_tcp_is_200_even_with_flag_on() {
    let _g = lock();
    with_temp_home(|| {
        // Flag ON: the scoped machinery is live, but the OWNER arm of
        // /hook/complete (ct_eq_token) is independent of it — Phase 1 is
        // dual-accept, so the owner token keeps completing the hook over TCP.
        std::env::set_var("K2_HOOK_SCOPED", "1");
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let r = http(
            d.port,
            "GET",
            &format!("/hook/complete?token={OWNER_TOKEN}&paneId=p1&eventType=stop"),
            None,
        );
        assert_eq!(
            r.status, 200,
            "owner token must still complete the hook over TCP with the scoped flag ON; body={}",
            r.body
        );
        std::env::remove_var("K2_HOOK_SCOPED");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hook_revoke_all_is_owner_only_and_kills_scoped_tokens() {
    let _g = lock();
    with_temp_home(|| {
        // Mint a REAL scoped token via the process registry (under temp HOME).
        let sid = SessionId::new();
        let principal = HookPrincipal {
            workspace_uuid: "ws-revoke".to_string(),
            agent_address: "agent-revoke".to_string(),
        };
        let token = session_token::mint_session_token(&sid, "pane-1", principal, session_token::CredMode::ApiKey, session_token::Provider::Anthropic);
        assert!(
            session_token::validate_hook(&token).is_some(),
            "freshly minted scoped token validates"
        );

        let member = seed_user_session("revoke_member", "password123", Role::Member);
        let admin = seed_user_session("revoke_admin", "password123", Role::Admin);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // GET → 405 (POST-only kill switch; a stray GET must not trip it).
        let r = http(
            d.port,
            "GET",
            &format!("/cli/daemon/hook-revoke-all?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(r.status, 405, "GET hook-revoke-all must be 405; body={}", r.body);

        // Member session → 403; Admin session → 403 (OWNER-ONLY, NOT
        // owner-or-admin — a connect-user must not mass-revoke agent creds).
        for (who, tok) in [("member", &member), ("admin", &admin)] {
            let r = http(
                d.port,
                "POST",
                &format!("/cli/daemon/hook-revoke-all?token={tok}"),
                Some(""),
            );
            assert_eq!(r.status, 403, "{who} session must NOT revoke-all; body={}", r.body);
        }
        // The rejected calls did NOT fire revoke_all — the token still validates.
        assert!(
            session_token::validate_hook(&token).is_some(),
            "scoped token survives the rejected (non-owner) revoke-all calls"
        );

        // Owner token → 200, and EVERY minted scoped token is now stale.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/daemon/hook-revoke-all?token={OWNER_TOKEN}"),
            Some(""),
        );
        assert_eq!(r.status, 200, "owner revoke-all must 200; body={}", r.body);
        assert!(
            r.body.contains("all-scoped-hook-tokens"),
            "200 body acks the global kill switch; body={}",
            r.body
        );
        assert!(
            session_token::validate_hook(&token).is_none(),
            "after the owner revoke-all the prior scoped token validates to None"
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_status_unknown_job_is_404_for_owner() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        // Owner passes the gate; unknown job → 404 from the handler.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/daemon/update/status?token={OWNER_TOKEN}&job_id=ghost"),
            None,
        );
        assert_eq!(r.status, 404, "owner status for unknown job → 404; body={}", r.body);
    });
}

// ─────────────────────────────────────────────────────────────────────
// Group — Hosted web cookie auth transport (PRD §2.3 / §9.2, phase 2a)
//
// 1. login with web flag → Set-Cookie: k2_session=…
// 2. Cookie-only + X-K2-Client → gated route (whoami) succeeds
// 3. Cookie-only POST without CSRF header → 403
// 4. Query token still works without cookie
// 5. Secure omitted on plain HTTP; present with X-Forwarded-Proto: https
// 6. Logout clears cookie + revokes session
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_login_sets_k2_session_cookie() {
    let _g = lock();
    with_temp_home(|| {
        connect_users::add_user("webuser", "password123").expect("add_user");
        connect_users::set_role("webuser", Role::Member).expect("set_role");
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Body flag web: true.
        let body = r#"{"username":"webuser","password":"password123","web":true}"#;
        let r = http(d.port, "POST", "/cli/auth/login", Some(body));
        assert_eq!(r.status, 200, "web login must 200; body={}", r.body);
        let cookie = r
            .set_cookie
            .as_deref()
            .expect("web login must Set-Cookie k2_session");
        assert!(
            cookie.starts_with("k2_session="),
            "cookie name: {cookie}"
        );
        assert!(cookie.contains("HttpOnly"), "HttpOnly required: {cookie}");
        assert!(
            cookie.contains("SameSite=Strict"),
            "SameSite=Strict required: {cookie}"
        );
        assert!(cookie.contains("Path=/"), "Path=/ required: {cookie}");
        assert!(
            cookie.contains("Max-Age="),
            "Max-Age required: {cookie}"
        );
        // Local HTTP test harness: Secure must be OMITTED.
        assert!(
            !cookie.contains("Secure"),
            "Secure must be omitted without X-Forwarded-Proto: https: {cookie}"
        );

        // Cookie value must equal the JSON token (connect-users session).
        let json: serde_json::Value =
            serde_json::from_str(&r.body).expect("login body JSON");
        let token = json["token"].as_str().expect("token field");
        assert!(
            cookie.contains(&format!("k2_session={token}")),
            "cookie value must be the session token; cookie={cookie} token={token}"
        );
        // Never the owner daemon token.
        assert!(
            !cookie.contains(OWNER_TOKEN),
            "owner daemon token must NEVER be in the cookie"
        );

        // Non-web login (no flag, no header) must NOT set the cookie.
        let r2 = http(
            d.port,
            "POST",
            "/cli/auth/login",
            Some(r#"{"username":"webuser","password":"password123"}"#),
        );
        assert_eq!(r2.status, 200, "plain login still 200; body={}", r2.body);
        assert!(
            r2.set_cookie.is_none(),
            "non-web login must not Set-Cookie; got {:?}",
            r2.set_cookie
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_login_via_header_sets_cookie_and_secure_with_forwarded_proto() {
    let _g = lock();
    with_temp_home(|| {
        connect_users::add_user("webhdr", "password123").expect("add_user");
        connect_users::set_role("webhdr", Role::Member).expect("set_role");
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        let body = r#"{"username":"webhdr","password":"password123"}"#;
        let r = http_with_headers(
            d.port,
            "POST",
            "/cli/auth/login",
            Some(body),
            &["X-K2-Client: web", "X-Forwarded-Proto: https"],
        );
        assert_eq!(r.status, 200, "header web login must 200; body={}", r.body);
        let cookie = r
            .set_cookie
            .as_deref()
            .expect("X-K2-Client: web login must Set-Cookie");
        assert!(
            cookie.contains("Secure"),
            "Secure required when X-Forwarded-Proto: https: {cookie}"
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cookie_only_whoami_succeeds_with_web_client_header() {
    let _g = lock();
    with_temp_home(|| {
        let session = seed_user_session("cookiewho", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Cookie only + X-K2-Client on GET whoami (CSRF not required on GET).
        let r = http_with_headers(
            d.port,
            "GET",
            "/cli/auth/whoami",
            None,
            &[
                &format!("Cookie: k2_session={session}"),
                "X-K2-Client: web",
            ],
        );
        assert_eq!(
            r.status, 200,
            "cookie-only whoami must 200; body={}",
            r.body
        );
        assert!(
            r.body.contains("cookiewho"),
            "whoami should report the connect-user; body={}",
            r.body
        );

        // Cookie only WITHOUT web header on GET still works (CSRF is
        // mutating-only). Confirms cookie is accepted as an auth source.
        let r2 = http_with_headers(
            d.port,
            "GET",
            "/cli/auth/whoami",
            None,
            &[&format!("Cookie: k2_session={session}")],
        );
        assert_eq!(
            r2.status, 200,
            "cookie-only GET whoami without X-K2-Client still 200; body={}",
            r2.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cookie_only_post_without_csrf_header_is_403() {
    let _g = lock();
    with_temp_home(|| {
        let session = seed_user_session("csrfuser", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Cookie only, no X-K2-Client → 403 csrf_required on POST logout.
        let r = http_with_headers(
            d.port,
            "POST",
            "/cli/auth/logout",
            Some(""),
            &[&format!("Cookie: k2_session={session}")],
        );
        assert_eq!(
            r.status, 403,
            "cookie-only POST without CSRF header → 403; body={}",
            r.body
        );
        assert!(
            r.body.contains("csrf_required") || r.body.contains("csrf"),
            "403 body should name CSRF; body={}",
            r.body
        );

        // Session still live (CSRF rejected before handler).
        assert_eq!(
            connect_users::validate_session(&session),
            Some("csrfuser".to_string()),
            "CSRF rejection must not revoke the session"
        );

        // Same POST with X-K2-Client: web → 200 + clear cookie.
        let r2 = http_with_headers(
            d.port,
            "POST",
            "/cli/auth/logout",
            Some(""),
            &[
                &format!("Cookie: k2_session={session}"),
                "X-K2-Client: web",
            ],
        );
        assert_eq!(
            r2.status, 200,
            "cookie-only POST with X-K2-Client → 200; body={}",
            r2.body
        );
        let clear = r2
            .set_cookie
            .as_deref()
            .expect("logout must clear Set-Cookie");
        assert!(
            clear.contains("Max-Age=0"),
            "logout cookie must Max-Age=0: {clear}"
        );
        assert_eq!(
            connect_users::validate_session(&session),
            None,
            "logout must revoke the connect-user session server-side"
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn query_token_still_works_without_cookie() {
    let _g = lock();
    with_temp_home(|| {
        let session = seed_user_session("qtokuser", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Classic CLI path: ?token= only, no Cookie, no X-K2-Client.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/auth/whoami?token={session}"),
            None,
        );
        assert_eq!(
            r.status, 200,
            "query token whoami must still 200; body={}",
            r.body
        );
        assert!(
            r.body.contains("qtokuser"),
            "whoami body: {}",
            r.body
        );

        // Mutating POST with query token (no cookie) must NOT require CSRF.
        let r2 = http(
            d.port,
            "POST",
            &format!("/cli/auth/logout?token={session}"),
            Some(""),
        );
        assert_eq!(
            r2.status, 200,
            "query-token POST logout must 200 without CSRF header; body={}",
            r2.body
        );
        assert_eq!(
            connect_users::validate_session(&session),
            None,
            "query-token logout must still revoke the session"
        );
    });
}
