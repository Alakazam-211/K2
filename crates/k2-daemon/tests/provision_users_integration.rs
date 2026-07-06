//! K2 Cloud P1-A (prd-k2-cloud-hosted-servers §6, slices S1+S2) —
//! provisioning integration tests over the REAL dispatcher
//! (`k2_daemon::test_harness::start`, the #630 harness).
//!
//! Coverage (per the slice spec):
//!   (a) users/add with mustChangePassword → login carries the flag →
//!       an arbitrary session-authed route 403s `password_change_required`
//!       → whoami + logout still work → change-password → flag cleared →
//!       full access on the fresh session,
//!   (b) the OWNER token is never restricted,
//!   (c) seed-users file consumed: users exist with roles+flags, the
//!       file is deleted (also: existing users skipped, malformed file
//!       deleted without seeding),
//!   (d) a legacy connect-users.json (no must_change_password field)
//!       deserializes with the flag false and logs in unrestricted,
//!   (e) POST-only method guards on the touched mutating routes.
//!
//! ISOLATION: connect-user stores + `$HOME` are process-wide — every
//! test serializes on `TEST_LOCK` and points `$HOME` at a fresh tempdir
//! (presence-suite pattern).

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::connect_users::{self, Role};
use k2_daemon::seed_users::{self, SeedOutcome};
use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-provision-p1a";

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
/// connect-sessions.json + seed-users.json) and run `f`, restoring
/// `$HOME` after. Caller holds `TEST_LOCK`.
fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp =
        std::env::temp_dir().join(format!("k2-provision-p1a-{}-{nanos}", std::process::id()));
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
    serde_json::from_str(&r.body).expect("login body is JSON")
}

// ─────────────────────────────────────────────────────────────────────
// (a) full forced-rotation round trip through the real dispatcher
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_change_password_round_trip_restricts_then_releases() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            // Provision with the opt-in flag through the REAL route.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/users/add?token={OWNER_TOKEN}"),
                Some(r#"{"username":"prov_alice","password":"temp-password-1","mustChangePassword":true}"#),
            );
            assert_eq!(r.status, 200, "users/add must succeed; body={}", r.body);

            // Login carries the pinned always-present boolean, true.
            let v = login(d.port, "prov_alice", "temp-password-1");
            assert_eq!(
                v["mustChangePassword"],
                serde_json::json!(true),
                "login must flag the restriction: {v}"
            );
            let sess = v["token"].as_str().expect("login token").to_string();

            // ARBITRARY session-authed routes → 403 password_change_required.
            for path in [
                format!("/status?token={sess}"),
                format!("/cli/presence/roster?token={sess}"),
                format!("/cli/users/policy?token={sess}"),
            ] {
                let r = http(d.port, "GET", &path, None);
                assert_eq!(r.status, 403, "restricted session must 403 on {path}; body={}", r.body);
                let b: serde_json::Value = serde_json::from_str(&r.body).expect("403 body is JSON");
                assert_eq!(
                    b["error"], "password_change_required",
                    "pinned error body on {path}: {}",
                    r.body
                );
            }
            // Mutating routes are blocked too (POST arm).
            let r = http(
                d.port,
                "POST",
                &format!("/cli/presence/grant?token={sess}"),
                Some(r#"{"username":"prov_alice","granted":true}"#),
            );
            assert_eq!(r.status, 403, "restricted POST must 403; body={}", r.body);
            assert!(
                r.body.contains("password_change_required"),
                "pinned error body: {}",
                r.body
            );

            // whoami stays reachable and carries the flag.
            let r = http(d.port, "GET", &format!("/cli/auth/whoami?token={sess}"), None);
            assert_eq!(r.status, 200, "whoami must stay reachable; body={}", r.body);
            let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
            assert_eq!(v["username"], "prov_alice");
            assert_eq!(v["mustChangePassword"], serde_json::json!(true));

            // logout stays reachable: mint a second session and log it out.
            let v2 = login(d.port, "prov_alice", "temp-password-1");
            let sess2 = v2["token"].as_str().expect("token").to_string();
            let r = http(
                d.port,
                "POST",
                &format!("/cli/auth/logout?token={sess2}"),
                Some("{}"),
            );
            assert_eq!(r.status, 200, "logout must stay reachable; body={}", r.body);
            assert!(r.body.contains("\"success\":true"), "got: {}", r.body);

            // change-password stays reachable — the way out.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/auth/change-password?token={sess}"),
                Some(r#"{"currentPassword":"temp-password-1","newPassword":"rotated-password-2"}"#),
            );
            assert_eq!(r.status, 200, "change-password must succeed; body={}", r.body);

            // Existing behavior: change-password revoked EVERY session.
            let r = http(d.port, "GET", &format!("/cli/auth/whoami?token={sess}"), None);
            assert_eq!(r.status, 403, "old session must be revoked; body={}", r.body);

            // Fresh login: flag cleared, full access restored.
            let v = login(d.port, "prov_alice", "rotated-password-2");
            assert_eq!(
                v["mustChangePassword"],
                serde_json::json!(false),
                "flag must be cleared after rotation: {v}"
            );
            let fresh = v["token"].as_str().expect("token").to_string();
            let r = http(d.port, "GET", &format!("/status?token={fresh}"), None);
            assert_eq!(r.status, 200, "full access after rotation; body={}", r.body);
            let r = http(
                d.port,
                "GET",
                &format!("/cli/presence/roster?token={fresh}"),
                None,
            );
            assert_eq!(r.status, 200, "roster after rotation; body={}", r.body);
            let r = http(d.port, "GET", &format!("/cli/auth/whoami?token={fresh}"), None);
            assert_eq!(r.status, 200);
            let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
            assert_eq!(v["mustChangePassword"], serde_json::json!(false));
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (a2) owner set-password also clears the flag
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_set_password_clears_the_flag() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            let r = http(
                d.port,
                "POST",
                &format!("/cli/users/add?token={OWNER_TOKEN}"),
                Some(r#"{"username":"prov_reset","password":"temp-password-1","mustChangePassword":true}"#),
            );
            assert_eq!(r.status, 200, "users/add; body={}", r.body);
            assert!(connect_users::must_change_password("prov_reset"));

            let r = http(
                d.port,
                "POST",
                &format!("/cli/users/set-password?token={OWNER_TOKEN}"),
                Some(r#"{"username":"prov_reset","password":"owner-issued-2"}"#),
            );
            assert_eq!(r.status, 200, "set-password; body={}", r.body);
            assert!(
                !connect_users::must_change_password("prov_reset"),
                "owner set-password must clear the flag"
            );
            let v = login(d.port, "prov_reset", "owner-issued-2");
            assert_eq!(v["mustChangePassword"], serde_json::json!(false));
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (b) the OWNER token is never restricted
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_token_is_never_restricted() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            // Flagged users existing must not affect the owner token.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/users/add?token={OWNER_TOKEN}"),
                Some(r#"{"username":"prov_flagged","password":"temp-password-1","mustChangePassword":true}"#),
            );
            assert_eq!(r.status, 200, "users/add; body={}", r.body);

            for path in [
                format!("/status?token={OWNER_TOKEN}"),
                format!("/cli/presence/roster?token={OWNER_TOKEN}"),
                format!("/cli/users?token={OWNER_TOKEN}"),
                format!("/cli/auth/whoami?token={OWNER_TOKEN}"),
            ] {
                let r = http(d.port, "GET", &path, None);
                assert_eq!(r.status, 200, "owner must reach {path}; body={}", r.body);
            }
            // whoami reports the owner unflagged.
            let r = http(
                d.port,
                "GET",
                &format!("/cli/auth/whoami?token={OWNER_TOKEN}"),
                None,
            );
            let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
            assert_eq!(v["owner"], serde_json::json!(true));
            assert_eq!(v["mustChangePassword"], serde_json::json!(false));
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (c) seed-users file: consumed once, roles+flags applied, skips existing
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seed_users_file_provisions_and_is_deleted() {
    let _g = lock();
    with_temp_home(|| {
        // Pre-existing user the seed file must NOT modify.
        connect_users::add_user("seed_existing", "keep-this-pass").expect("add existing");

        let dir = dirs::home_dir().expect("home").join(".k2");
        std::fs::create_dir_all(&dir).expect("mkdir .k2");
        let seed_path = dir.join("seed-users.json");
        std::fs::write(
            &seed_path,
            r#"[
                {"username":"seed_owner","password":"seed-pass-1","role":"owner","mustChangePassword":true},
                {"username":"seed_ops","password":"seed-pass-2","role":"admin"},
                {"username":"seed_existing","password":"OVERWRITE-attempt","role":"viewer","mustChangePassword":true},
                {"username":"seed_bad","password":"seed-pass-4","role":"superuser"}
            ]"#,
        )
        .expect("write seed file");

        // Consume — the SAME module function main.rs's boot calls.
        let results = seed_users::consume_seed_file();

        // The file is GONE (consume-once; no plaintext left on disk).
        assert!(
            !seed_path.exists(),
            "seed-users.json must be deleted after consumption"
        );

        // Per-user outcomes.
        assert_eq!(results.len(), 4, "one outcome per row: {results:?}");
        assert_eq!(results[0], ("seed_owner".to_string(), SeedOutcome::Created));
        assert_eq!(results[1], ("seed_ops".to_string(), SeedOutcome::Created));
        assert_eq!(
            results[2],
            ("seed_existing".to_string(), SeedOutcome::SkippedExists)
        );
        match &results[3] {
            (u, SeedOutcome::Failed(e)) => {
                assert_eq!(u, "seed_bad");
                assert!(e.contains("invalid role"), "got: {e}");
            }
            other => panic!("bad-role row must fail loudly, got {other:?}"),
        }

        // Roles + flags landed.
        assert_eq!(connect_users::role_for_user("seed_owner"), Some(Role::Owner));
        assert!(connect_users::must_change_password("seed_owner"));
        assert_eq!(connect_users::role_for_user("seed_ops"), Some(Role::Admin));
        assert!(!connect_users::must_change_password("seed_ops"));
        assert_eq!(connect_users::role_for_user("seed_bad"), None, "failed row not created");

        // The existing user is untouched: old password + role + no flag.
        assert!(
            connect_users::verify("seed_existing", "keep-this-pass"),
            "existing user's password must be untouched"
        );
        assert_eq!(connect_users::role_for_user("seed_existing"), Some(Role::Member));
        assert!(!connect_users::must_change_password("seed_existing"));

        // The seeded credentials WORK end-to-end through the daemon,
        // restricted as flagged.
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            let v = login(d.port, "seed_owner", "seed-pass-1");
            assert_eq!(v["mustChangePassword"], serde_json::json!(true));
            let sess = v["token"].as_str().expect("token").to_string();
            let r = http(d.port, "GET", &format!("/status?token={sess}"), None);
            assert_eq!(r.status, 403, "seeded flagged user is restricted; body={}", r.body);

            let v = login(d.port, "seed_ops", "seed-pass-2");
            assert_eq!(v["mustChangePassword"], serde_json::json!(false));
            let sess = v["token"].as_str().expect("token").to_string();
            let r = http(d.port, "GET", &format!("/status?token={sess}"), None);
            assert_eq!(r.status, 200, "seeded unflagged admin has access; body={}", r.body);
        });
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seed_users_malformed_file_is_still_deleted() {
    let _g = lock();
    with_temp_home(|| {
        let dir = dirs::home_dir().expect("home").join(".k2");
        std::fs::create_dir_all(&dir).expect("mkdir .k2");
        let seed_path = dir.join("seed-users.json");
        std::fs::write(&seed_path, "this is { not json [").expect("write garbage");

        let results = seed_users::consume_seed_file();
        assert!(results.is_empty(), "nothing seeded from garbage: {results:?}");
        assert!(
            !seed_path.exists(),
            "malformed seed file must STILL be deleted (never leave secrets)"
        );
        assert_eq!(
            connect_users::list_users().expect("list").len(),
            0,
            "no accounts created from a malformed file"
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seed_users_absent_file_is_a_noop() {
    let _g = lock();
    with_temp_home(|| {
        let results = seed_users::consume_seed_file();
        assert!(results.is_empty(), "absent file → no outcomes: {results:?}");
    });
}

// ─────────────────────────────────────────────────────────────────────
// (d) legacy connect-users.json (no flag field) loads + logs in unrestricted
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_store_without_flag_logs_in_unrestricted() {
    let _g = lock();
    with_temp_home(|| {
        // Provision through the primitives, then STRIP the new field from
        // the on-disk JSON to reproduce a store written by a pre-S1 daemon
        // (real argon2 hash, no must_change_password key anywhere).
        connect_users::add_user("legacy_user", "legacy-pass-1").expect("add");
        let store_path = connect_users::store_path();
        let raw = std::fs::read_to_string(&store_path).expect("read store");
        let mut v: serde_json::Value = serde_json::from_str(&raw).expect("store JSON");
        let users = v["users"].as_array_mut().expect("users array");
        for u in users.iter_mut() {
            let obj = u.as_object_mut().expect("user object");
            assert!(
                obj.remove("must_change_password").is_some(),
                "test must actually strip the field"
            );
        }
        std::fs::write(&store_path, serde_json::to_string_pretty(&v).unwrap())
            .expect("write legacy store");
        assert!(
            !std::fs::read_to_string(&store_path)
                .unwrap()
                .contains("must_change_password"),
            "legacy blob must not contain the field"
        );

        // Loads with the flag defaulting false…
        assert!(!connect_users::must_change_password("legacy_user"));

        // …and logs in fully unrestricted through the real dispatcher.
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            let v = login(d.port, "legacy_user", "legacy-pass-1");
            assert_eq!(
                v["mustChangePassword"],
                serde_json::json!(false),
                "legacy row defaults false on the wire: {v}"
            );
            let sess = v["token"].as_str().expect("token").to_string();
            let r = http(d.port, "GET", &format!("/status?token={sess}"), None);
            assert_eq!(r.status, 200, "legacy user unrestricted; body={}", r.body);
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (e) POST-only guards on the touched mutating routes
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn touched_mutating_routes_reject_get_405() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            // The S1-touched mutating surface: add (modified), plus the
            // routes the restricted allowlist exposes (change-password,
            // logout) and login — all must stay POST-only.
            for path in [
                format!("/cli/users/add?token={OWNER_TOKEN}"),
                format!("/cli/auth/change-password?token={OWNER_TOKEN}"),
                format!("/cli/auth/logout?token={OWNER_TOKEN}"),
                "/cli/auth/login".to_string(),
            ] {
                let r = http(d.port, "GET", &path, None);
                assert_eq!(r.status, 405, "GET {path} must 405; body={}", r.body);
            }
        });
    });
}
