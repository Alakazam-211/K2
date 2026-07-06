//! F4 (prd-v1-api-completion §6) — owner-ROLE API-key management, driven
//! end-to-end through the REAL dispatcher (`k2_daemon::test_harness::start`,
//! the #630 harness).
//!
//! The contract under test — `/cli/api-keys/{create,revoke,list}` gate on
//! `api_key_manager_identity` (owner TOKEN or Owner-ROLE connect session,
//! the `can_change_roles` bar shared with `/cli/users/set-role`):
//!
//!   1. An Owner-ROLE session can create + list + revoke keys (hosted
//!      customers never hold the daemon token).
//!   2. An Admin-role session is 403'd on all three (Admin does NOT get
//!      key management) — and nothing is minted.
//!   3. A Member-role session is 403'd on all three.
//!   4. An API key can NEVER manage keys: a freshly-minted `k2sk_…` raw
//!      key presented as the token is 403'd on all three.
//!   5. The owner TOKEN still manages keys (the pre-F4 path is unchanged).
//!   6. A must-change-password Owner-role session is blocked by the
//!      `session_password_gate` chokepoint (403 `password_change_required`)
//!      BEFORE the role gate — and regains key management after rotating.
//!   7. POST-only method guards hold on create/revoke (house rule).
//!
//! ISOLATION: connect-user stores + `$HOME` are process-wide — every test
//! serializes on `TEST_LOCK`, redirects `$HOME` to a fresh tempdir, and
//! uses per-test usernames/labels (provision-suite pattern).

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

const OWNER_TOKEN: &str = "owner-token-deadbeef-api-keys-f4";

/// The pinned rejection bodies. Role-gate rejections use the shared
/// `CliResponse::forbidden()` shape; the password chokepoint has its own.
const FORBIDDEN: &str = r#"{"error":"Invalid or missing auth token"}"#;
const PASSWORD_GATE: &str = r#"{"error":"password_change_required"}"#;

/// A minimal parsed HTTP response: numeric status + body. (Presence/
/// auth-suite harness pattern — raw loopback socket, no extra deps.)
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
/// connect-sessions.json) and run `f`, restoring `$HOME` after. Caller
/// holds `TEST_LOCK`.
fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp =
        std::env::temp_dir().join(format!("k2-api-keys-f4-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);

    f();

    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

/// POST /cli/auth/login and return the parsed JSON body (asserts 200).
fn login(port: u16, username: &str, password: &str) -> serde_json::Value {
    let r = http(
        port,
        "POST",
        "/cli/auth/login",
        Some(&format!(
            r#"{{"username":"{username}","password":"{password}"}}"#
        )),
    );
    assert_eq!(r.status, 200, "login must succeed; body={}", r.body);
    json(&r.body)
}

/// Provision `username` through the REAL routes (owner token): users/add
/// (+ optional mustChangePassword), then set-role when `role` isn't the
/// default member. Returns a fresh session token for the user.
fn provision_and_login(
    port: u16,
    username: &str,
    password: &str,
    role: &str,
    must_change_password: bool,
) -> String {
    let r = http(
        port,
        "POST",
        &format!("/cli/users/add?token={OWNER_TOKEN}"),
        Some(&format!(
            r#"{{"username":"{username}","password":"{password}","mustChangePassword":{must_change_password}}}"#
        )),
    );
    assert_eq!(r.status, 200, "users/add({username}) must succeed; body={}", r.body);
    if role != "member" {
        let r = http(
            port,
            "POST",
            &format!("/cli/users/set-role?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"username":"{username}","role":"{role}"}}"#)),
        );
        assert_eq!(r.status, 200, "set-role({username}→{role}) must succeed; body={}", r.body);
    }
    let v = login(port, username, password);
    assert_eq!(
        v["mustChangePassword"],
        serde_json::json!(must_change_password),
        "login flag mismatch for {username}: {v}"
    );
    v["token"].as_str().expect("login token").to_string()
}

/// Assert all three api-keys routes reject `token` with the shared 403
/// role-gate body.
fn assert_all_key_routes_403(port: u16, token: &str, ctx: &str) {
    let create = http(
        port,
        "POST",
        &format!("/cli/api-keys/create?token={token}"),
        Some(&format!(r#"{{"label":"forbidden-{ctx}"}}"#)),
    );
    assert_eq!(create.status, 403, "{ctx}: create must 403; body={}", create.body);
    assert_eq!(create.body, FORBIDDEN, "{ctx}: create 403 body is pinned");

    let list = http(port, "GET", &format!("/cli/api-keys/list?token={token}"), None);
    assert_eq!(list.status, 403, "{ctx}: list must 403; body={}", list.body);
    assert_eq!(list.body, FORBIDDEN, "{ctx}: list 403 body is pinned");

    let revoke = http(
        port,
        "POST",
        &format!("/cli/api-keys/revoke?token={token}"),
        Some(r#"{"id":"not-a-real-id"}"#),
    );
    assert_eq!(revoke.status, 403, "{ctx}: revoke must 403; body={}", revoke.body);
    assert_eq!(revoke.body, FORBIDDEN, "{ctx}: revoke 403 body is pinned");
}

/// Owner-token list → the set of key labels currently stored (proves a
/// rejected create really minted NOTHING).
fn owner_list_labels(port: u16) -> Vec<String> {
    let r = http(port, "GET", &format!("/cli/api-keys/list?token={OWNER_TOKEN}"), None);
    assert_eq!(r.status, 200, "owner list must succeed; body={}", r.body);
    json(&r.body)["keys"]
        .as_array()
        .expect("keys array")
        .iter()
        .filter_map(|k| k["label"].as_str().map(str::to_string))
        .collect()
}

// ─────────────────────────────────────────────────────────────────────
// (1) Owner-ROLE session: full create → list → revoke round trip.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_role_session_creates_lists_revokes() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            let sess =
                provision_and_login(d.port, "f4_owner1", "hunter2-strong-1", "owner", false);

            // Create with the SESSION token — the hosted-customer path.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/api-keys/create?token={sess}"),
                Some(r#"{"label":"f4-owner-session-key"}"#),
            );
            assert_eq!(r.status, 200, "owner-role create must succeed; body={}", r.body);
            let created = json(&r.body);
            let id = created["id"].as_str().expect("id").to_string();
            let raw = created["key"].as_str().expect("raw key").to_string();
            assert!(raw.starts_with("k2sk_"), "minted key shape; got {raw:?}");

            // List with the session token shows it, live.
            let r = http(d.port, "GET", &format!("/cli/api-keys/list?token={sess}"), None);
            assert_eq!(r.status, 200, "owner-role list must succeed; body={}", r.body);
            let keys = json(&r.body);
            let mine = keys["keys"]
                .as_array()
                .expect("keys array")
                .iter()
                .find(|k| k["id"] == serde_json::json!(id))
                .unwrap_or_else(|| panic!("created key {id} must be listed: {keys}"))
                .clone();
            assert_eq!(mine["label"], serde_json::json!("f4-owner-session-key"));
            assert_eq!(mine["revokedAt"], serde_json::Value::Null, "fresh key is live");

            // Revoke with the session token.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/api-keys/revoke?token={sess}"),
                Some(&format!(r#"{{"id":"{id}"}}"#)),
            );
            assert_eq!(r.status, 200, "owner-role revoke must succeed; body={}", r.body);
            assert_eq!(json(&r.body)["success"], serde_json::json!(true), "body={}", r.body);

            // And the list reflects it.
            let r = http(d.port, "GET", &format!("/cli/api-keys/list?token={sess}"), None);
            assert_eq!(r.status, 200);
            let keys = json(&r.body);
            let mine = keys["keys"]
                .as_array()
                .expect("keys array")
                .iter()
                .find(|k| k["id"] == serde_json::json!(id))
                .unwrap_or_else(|| panic!("revoked key {id} must still be listed: {keys}"))
                .clone();
            assert_ne!(
                mine["revokedAt"],
                serde_json::Value::Null,
                "revokedAt must be set after revoke: {mine}"
            );
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (2) Admin-role session: 403 on all three, and nothing minted.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_role_session_is_forbidden() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            let sess =
                provision_and_login(d.port, "f4_admin1", "hunter2-strong-2", "admin", false);
            assert_all_key_routes_403(d.port, &sess, "admin");
            // The rejected create minted NOTHING (owner-token audit).
            assert!(
                !owner_list_labels(d.port).iter().any(|l| l == "forbidden-admin"),
                "a 403'd admin create must not mint a key"
            );
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (3) Member-role session: 403 on all three.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_role_session_is_forbidden() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            let sess =
                provision_and_login(d.port, "f4_member1", "hunter2-strong-3", "member", false);
            assert_all_key_routes_403(d.port, &sess, "member");
            assert!(
                !owner_list_labels(d.port).iter().any(|l| l == "forbidden-member"),
                "a 403'd member create must not mint a key"
            );
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (4) An API key can NEVER manage keys (the /v1 boundary invariant).
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_key_cannot_manage_keys() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            // Owner mints a key…
            let r = http(
                d.port,
                "POST",
                &format!("/cli/api-keys/create?token={OWNER_TOKEN}"),
                Some(r#"{"label":"f4-the-key-itself"}"#),
            );
            assert_eq!(r.status, 200, "owner-token create; body={}", r.body);
            let raw = json(&r.body)["key"].as_str().expect("raw key").to_string();
            assert!(raw.starts_with("k2sk_"));

            // …and the RAW KEY presented as the token is 403'd everywhere.
            assert_all_key_routes_403(d.port, &raw, "api-key");
            assert!(
                !owner_list_labels(d.port).iter().any(|l| l == "forbidden-api-key"),
                "a 403'd api-key create must not mint a key"
            );
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (5) The owner TOKEN still manages keys (pre-F4 path unchanged).
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_token_still_manages_keys() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            let r = http(
                d.port,
                "POST",
                &format!("/cli/api-keys/create?token={OWNER_TOKEN}"),
                Some(r#"{"label":"f4-owner-token-key"}"#),
            );
            assert_eq!(r.status, 200, "owner-token create; body={}", r.body);
            let id = json(&r.body)["id"].as_str().expect("id").to_string();

            let r = http(d.port, "GET", &format!("/cli/api-keys/list?token={OWNER_TOKEN}"), None);
            assert_eq!(r.status, 200, "owner-token list; body={}", r.body);
            assert!(r.body.contains(&id), "list includes the minted key: {}", r.body);

            let r = http(
                d.port,
                "POST",
                &format!("/cli/api-keys/revoke?token={OWNER_TOKEN}"),
                Some(&format!(r#"{{"id":"{id}"}}"#)),
            );
            assert_eq!(r.status, 200, "owner-token revoke; body={}", r.body);
            assert_eq!(json(&r.body)["success"], serde_json::json!(true), "body={}", r.body);
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (6) must-change-password Owner session → 403 password_change_required
//     (the P1-A chokepoint fires BEFORE the role gate), released after
//     rotation.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_change_password_owner_session_is_blocked_then_released() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            let sess = provision_and_login(
                d.port,
                "f4_rotate1",
                "temp-password-9",
                "owner",
                true,
            );

            // Every api-keys route is blocked by the PASSWORD gate — the
            // pinned chokepoint body, NOT the role-gate 403.
            for (method, path, body) in [
                ("POST", format!("/cli/api-keys/create?token={sess}"), Some(r#"{"label":"forbidden-rotate"}"#)),
                ("GET", format!("/cli/api-keys/list?token={sess}"), None),
                ("POST", format!("/cli/api-keys/revoke?token={sess}"), Some(r#"{"id":"x"}"#)),
            ] {
                let r = http(d.port, method, &path, body);
                assert_eq!(
                    r.status, 403,
                    "restricted owner session must 403 on {method} {path}; body={}",
                    r.body
                );
                assert_eq!(
                    r.body, PASSWORD_GATE,
                    "{method} {path}: must be the password chokepoint body"
                );
            }
            assert!(
                !owner_list_labels(d.port).iter().any(|l| l == "forbidden-rotate"),
                "a password-gated create must not mint a key"
            );

            // Rotate → fresh session → key management works (proves the
            // block above was the password gate, not the role gate).
            let r = http(
                d.port,
                "POST",
                &format!("/cli/auth/change-password?token={sess}"),
                Some(r#"{"currentPassword":"temp-password-9","newPassword":"rotated-password-10"}"#),
            );
            assert_eq!(r.status, 200, "change-password; body={}", r.body);
            let v = login(d.port, "f4_rotate1", "rotated-password-10");
            assert_eq!(v["mustChangePassword"], serde_json::json!(false), "{v}");
            let fresh = v["token"].as_str().expect("token");
            let r = http(
                d.port,
                "POST",
                &format!("/cli/api-keys/create?token={fresh}"),
                Some(r#"{"label":"f4-post-rotation-key"}"#),
            );
            assert_eq!(r.status, 200, "post-rotation create must succeed; body={}", r.body);
            assert!(json(&r.body)["key"].as_str().expect("key").starts_with("k2sk_"));
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (7) POST-only method guards (house rule) on the two mutating routes.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_and_revoke_are_post_only() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            for path in [
                format!("/cli/api-keys/create?token={OWNER_TOKEN}"),
                format!("/cli/api-keys/revoke?token={OWNER_TOKEN}"),
            ] {
                let r = http(d.port, "GET", &path, None);
                assert_eq!(r.status, 405, "GET {path} must be 405; body={}", r.body);
                assert!(
                    r.body.contains("POST required"),
                    "pinned 405 body on {path}: {}",
                    r.body
                );
            }
        });
    });
}
