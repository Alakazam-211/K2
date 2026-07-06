//! W6 (0.40.30) — `/cli/presets/*` driven end-to-end through the REAL
//! dispatcher (`k2_daemon::test_harness::start`, the #630 harness).
//! Harness plumbing cribbed from `tunnel_config_role_gate_integration.rs`.
//!
//! The contract under test:
//!
//!   1. `GET /cli/presets/list` serves the migration-0070 metadata
//!      columns (the seeded Claude built-in declares its danger flag).
//!   2. `GET /cli/presets/get?id=` — 200 with metadata for a known id,
//!      uniform 404 for an unknown one, 400 with no id.
//!   3. Owner-token CRUD: create a CUSTOM preset with a slug id +
//!      danger_flags/env/readiness, edit + clear metadata via update
//!      (`""` = clear), delete it; a built-in's metadata is editable
//!      but its DELETE is refused; invalid metadata is a 400.
//!   4. ROLE GATE: preset mutations are owner-or-admin — a member
//!      session is 403'd (and nothing is written), an admin session
//!      passes; reads stay open to any authed session.
//!   5. POST-only guard: `GET /cli/presets/create` is an explicit 405
//!      (feedback_post_only_route_guards), not a silent 404/mutation.
//!
//! ISOLATION: connect-user stores are process-wide — role-gate tests
//! serialize on `TEST_LOCK` and redirect `$HOME` to a tempdir. The
//! preset rows live in the process-global in-memory DB, so every id is
//! uniquified per test.

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

const OWNER_TOKEN: &str = "owner-token-deadbeef-preset-w6";

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

fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir()
        .join(format!("k2-preset-w6-{}-{nanos}", std::process::id()));
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

/// Provision `username` through the REAL routes (owner token) and log in.
fn provision_and_login(port: u16, username: &str, password: &str, role: &str) -> String {
    let r = http(
        port,
        "POST",
        &format!("/cli/users/add?token={OWNER_TOKEN}"),
        Some(&format!(
            r#"{{"username":"{username}","password":"{password}","mustChangePassword":false}}"#
        )),
    );
    assert_eq!(r.status, 200, "users/add({username}); body={}", r.body);
    if role != "member" {
        let r = http(
            port,
            "POST",
            &format!("/cli/users/set-role?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"username":"{username}","role":"{role}"}}"#)),
        );
        assert_eq!(r.status, 200, "set-role({username}→{role}); body={}", r.body);
    }
    let r = http(
        port,
        "POST",
        "/cli/auth/login",
        Some(&format!(
            r#"{{"username":"{username}","password":"{password}"}}"#
        )),
    );
    assert_eq!(r.status, 200, "login({username}); body={}", r.body);
    json(&r.body)["token"]
        .as_str()
        .expect("login token")
        .to_string()
}

fn uid(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("w6-{tag}-{}-{nanos}", std::process::id())
}

// ─────────────────────────────────────────────────────────────────────
// (1) + (2) — reads: list carries metadata; get is 200/404/400.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presets_list_and_get_serve_metadata() {
    let _g = lock();
    let d = futures_block(test_harness::start(OWNER_TOKEN));

    let r = http(
        d.port,
        "GET",
        &format!("/cli/presets/list?token={OWNER_TOKEN}"),
        None,
    );
    assert_eq!(r.status, 200, "list; body={}", r.body);
    let list = json(&r.body);
    let arr = list.as_array().expect("list is a JSON array");
    let claude = arr
        .iter()
        .find(|p| p["label"] == "Claude" && p["is_built_in"] == 1)
        .expect("seeded Claude built-in present");
    // Migration-0070 metadata rides the list read.
    let flags = claude["danger_flags"]
        .as_str()
        .expect("Claude declares danger_flags");
    assert!(
        flags.contains("--dangerously-skip-permissions"),
        "truthful seed: {flags}"
    );
    assert_eq!(claude["readiness"], serde_json::json!("bracketed-paste"));

    // get by id — same row, metadata included.
    let id = claude["id"].as_str().expect("id");
    let r = http(
        d.port,
        "GET",
        &format!("/cli/presets/get?token={OWNER_TOKEN}&id={id}"),
        None,
    );
    assert_eq!(r.status, 200, "get; body={}", r.body);
    let got = json(&r.body);
    assert_eq!(got["id"], serde_json::json!(id));
    assert!(got["danger_flags"].as_str().is_some(), "{got}");

    // Unknown id → uniform 404; missing id → 400.
    let r = http(
        d.port,
        "GET",
        &format!("/cli/presets/get?token={OWNER_TOKEN}&id=no-such-preset-w6"),
        None,
    );
    assert_eq!(r.status, 404, "unknown id must be the uniform 404; body={}", r.body);
    let r = http(
        d.port,
        "GET",
        &format!("/cli/presets/get?token={OWNER_TOKEN}"),
        None,
    );
    assert_eq!(r.status, 400, "missing id; body={}", r.body);
}

// ─────────────────────────────────────────────────────────────────────
// (3) — owner-token CRUD with metadata.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_crud_with_metadata_end_to_end() {
    let _g = lock();
    let d = futures_block(test_harness::start(OWNER_TOKEN));
    let slug = uid("crud");

    // Create with a slug id + full metadata.
    let r = http(
        d.port,
        "POST",
        &format!("/cli/presets/create?token={OWNER_TOKEN}"),
        Some(&format!(
            r#"{{"id":"{slug}","label":"W6 Aider","command":"aider --model ollama/qwen3",
                "dangerFlags":"[\"--yes-always\"]",
                "env":"{{\"OPENAI_BASE_URL\":\"http://localhost:11434/v1\"}}",
                "readiness":"settle:1500"}}"#
        )),
    );
    assert_eq!(r.status, 200, "create; body={}", r.body);
    let p = json(&r.body);
    assert_eq!(p["id"], serde_json::json!(slug));
    assert_eq!(p["is_built_in"], serde_json::json!(0));
    assert_eq!(p["danger_flags"], serde_json::json!(r#"["--yes-always"]"#));
    assert_eq!(p["readiness"], serde_json::json!("settle:1500"));

    // Update: change readiness, clear env ("" = clear-to-NULL).
    let r = http(
        d.port,
        "POST",
        &format!("/cli/presets/update?token={OWNER_TOKEN}"),
        Some(&format!(
            r#"{{"id":"{slug}","readiness":"bracketed-paste","env":""}}"#
        )),
    );
    assert_eq!(r.status, 200, "update; body={}", r.body);
    let p = json(&r.body);
    assert_eq!(p["readiness"], serde_json::json!("bracketed-paste"));
    assert_eq!(p["env"], serde_json::Value::Null, "'' must clear env: {p}");
    assert_eq!(
        p["danger_flags"],
        serde_json::json!(r#"["--yes-always"]"#),
        "untouched field survives: {p}"
    );

    // Invalid metadata is rejected loudly.
    let r = http(
        d.port,
        "POST",
        &format!("/cli/presets/update?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"id":"{slug}","readiness":"sentinel:hi"}}"#)),
    );
    assert_eq!(r.status, 400, "bad readiness; body={}", r.body);

    // Built-in: metadata editable, delete refused.
    let r = http(
        d.port,
        "GET",
        &format!("/cli/presets/list?token={OWNER_TOKEN}"),
        None,
    );
    let list = json(&r.body);
    let built_in_id = list
        .as_array()
        .expect("array")
        .iter()
        .find(|p| p["is_built_in"] == 1 && p["label"] == "Aider")
        .expect("seeded Aider built-in")["id"]
        .as_str()
        .expect("id")
        .to_string();
    let r = http(
        d.port,
        "POST",
        &format!("/cli/presets/update?token={OWNER_TOKEN}"),
        Some(&format!(
            r#"{{"id":"{built_in_id}","dangerFlags":"[\"--yes-always\"]"}}"#
        )),
    );
    assert_eq!(r.status, 200, "built-in metadata edit; body={}", r.body);
    let r = http(
        d.port,
        "POST",
        &format!("/cli/presets/delete?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"id":"{built_in_id}"}}"#)),
    );
    assert_eq!(r.status, 400, "built-in delete must refuse; body={}", r.body);
    assert!(r.body.contains("built-in"), "body={}", r.body);
    // Restore the seeded NULL so other suites sharing the global DB see
    // the truthful seed state.
    let r = http(
        d.port,
        "POST",
        &format!("/cli/presets/update?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"id":"{built_in_id}","dangerFlags":""}}"#)),
    );
    assert_eq!(r.status, 200, "restore; body={}", r.body);

    // Delete the custom preset; a re-get is the uniform 404.
    let r = http(
        d.port,
        "POST",
        &format!("/cli/presets/delete?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"id":"{slug}"}}"#)),
    );
    assert_eq!(r.status, 200, "custom delete; body={}", r.body);
    let r = http(
        d.port,
        "GET",
        &format!("/cli/presets/get?token={OWNER_TOKEN}&id={slug}"),
        None,
    );
    assert_eq!(r.status, 404, "deleted preset gone; body={}", r.body);
}

// ─────────────────────────────────────────────────────────────────────
// (4) — role gate: member 403 (no write), admin 200; reads open.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preset_mutations_gate_on_owner_or_admin() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            let member = provision_and_login(d.port, "w6_member", "hunter2-strong-1", "member");
            let admin = provision_and_login(d.port, "w6_admin", "hunter2-strong-2", "admin");
            let member_slug = uid("member-denied");
            let admin_slug = uid("admin-allowed");

            // Member: 403, and the row must NOT exist afterwards.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/presets/create?token={member}"),
                Some(&format!(
                    r#"{{"id":"{member_slug}","label":"Nope","command":"nope-agent"}}"#
                )),
            );
            assert_eq!(r.status, 403, "member create must 403; body={}", r.body);
            let r = http(
                d.port,
                "GET",
                &format!("/cli/presets/get?token={OWNER_TOKEN}&id={member_slug}"),
                None,
            );
            assert_eq!(r.status, 404, "403 must mean nothing was written");

            // Member update/delete/reset: 403 too.
            for (route, body) in [
                ("/cli/presets/update", format!(r#"{{"id":"{member_slug}"}}"#)),
                ("/cli/presets/delete", format!(r#"{{"id":"{member_slug}"}}"#)),
                ("/cli/presets/reset", String::from("{}")),
            ] {
                let r = http(
                    d.port,
                    "POST",
                    &format!("{route}?token={member}"),
                    Some(&body),
                );
                assert_eq!(r.status, 403, "member {route} must 403; body={}", r.body);
            }

            // Member READS stay open (any authed session may look).
            let r = http(
                d.port,
                "GET",
                &format!("/cli/presets/list?token={member}"),
                None,
            );
            assert_eq!(r.status, 200, "member list read; body={}", r.body);

            // Admin: create succeeds.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/presets/create?token={admin}"),
                Some(&format!(
                    r#"{{"id":"{admin_slug}","label":"W6 Admin","command":"admin-agent"}}"#
                )),
            );
            assert_eq!(r.status, 200, "admin create; body={}", r.body);
            // Clean up.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/presets/delete?token={OWNER_TOKEN}"),
                Some(&format!(r#"{{"id":"{admin_slug}"}}"#)),
            );
            assert_eq!(r.status, 200, "cleanup delete; body={}", r.body);
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (5) — POST-only guard on the mutation paths.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preset_mutations_are_post_only() {
    let _g = lock();
    let d = futures_block(test_harness::start(OWNER_TOKEN));
    for route in [
        "/cli/presets/create",
        "/cli/presets/update",
        "/cli/presets/delete",
        "/cli/presets/reorder",
        "/cli/presets/reset",
    ] {
        let r = http(
            d.port,
            "GET",
            &format!("{route}?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(
            r.status, 405,
            "GET {route} must be an explicit 405; body={}",
            r.body
        );
    }
}
