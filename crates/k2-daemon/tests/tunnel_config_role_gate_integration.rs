//! K2 Cloud re-pair — owner-ROLE tunnel-config writes, driven end-to-end
//! through the REAL dispatcher (`k2_daemon::test_harness::start`, the #630
//! harness). Cribbed from `api_keys_role_gate_integration.rs` (8ca53aa).
//!
//! The contract under test — `POST /cli/tunnel/config` gates on
//! `owner_role_identity` (owner TOKEN or Owner-ROLE connect session, the
//! `can_change_roles` bar shared with `/cli/api-keys/*` + `/cli/users/
//! set-role`), because the k2.dev server-management modal re-pairs a hosted
//! server's subdomain through the `k2cloud` Owner-ROLE session and never
//! holds the on-box daemon token:
//!
//!   1. An Owner-ROLE session can POST tunnel/config — and the persisted
//!      `~/.k2/tunnel.json` CONTENT actually changes — and can GET the
//!      redacted config view + /cli/tunnel/status.
//!   2. An Admin-role session is 403'd on POST (tunnel identity is
//!      ownership-level, NOT owner-or-admin) — and nothing is persisted.
//!   3. A Member-role session is 403'd on POST — and nothing is persisted.
//!   4. The owner TOKEN still writes config (the pre-existing path is
//!      unchanged).
//!   5. A must-change-password Owner-role session is blocked by the
//!      `session_password_gate` chokepoint (403 `password_change_required`)
//!      BEFORE the role gate — and can write after rotating.
//!   6. `/cli/tunnel/start` + `/cli/tunnel/stop` REMAIN owner-token-only:
//!      even an Owner-ROLE session is 403'd (a remote session severing its
//!      own tunnel is a footgun; /cli/daemon/restart covers the re-dial).
//!      Their POST-only guards hold too.
//!
//! ISOLATION: connect-user stores + `$HOME` (→ `~/.k2/tunnel.json`) are
//! process-wide — every test serializes on `TEST_LOCK`, redirects `$HOME`
//! to a fresh tempdir, and uses per-test usernames/subdomains.

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

const OWNER_TOKEN: &str = "owner-token-deadbeef-tunnel-repair";

/// The pinned rejection bodies. The tunnel arm keeps its historical 403
/// shape; the password chokepoint has its own.
const FORBIDDEN: &str = r#"{"error":"invalid or missing token"}"#;
const PASSWORD_GATE: &str = r#"{"error":"password_change_required"}"#;

/// A minimal parsed HTTP response: numeric status + body.
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
/// connect-sessions.json + `~/.k2/tunnel.json`) and run `f`, restoring
/// `$HOME` after. Caller holds `TEST_LOCK`.
fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir()
        .join(format!("k2-tunnel-repair-{}-{nanos}", std::process::id()));
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

/// Read the RAW persisted `~/.k2/tunnel.json` ("" when absent) — the
/// proof-of-persistence side channel the route assertions pair with.
fn tunnel_json_raw() -> String {
    std::fs::read_to_string(k2_core::tunnel::config::config_path()).unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────
// (1) Owner-ROLE session: POST config persists to disk; GET config +
//     GET status work.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_role_session_writes_config_and_reads_status() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            let sess =
                provision_and_login(d.port, "tr_owner1", "hunter2-strong-1", "owner", false);

            let before = tunnel_json_raw();
            assert!(
                !before.contains("repaired-by-session"),
                "precondition: fresh HOME has no repaired subdomain"
            );

            // POST with the SESSION token — the K2 Cloud re-pair path.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/tunnel/config?token={sess}"),
                Some(r#"{"subdomain":"repaired-by-session"}"#),
            );
            assert_eq!(r.status, 200, "owner-role config POST must succeed; body={}", r.body);
            let view = json(&r.body);
            assert_eq!(view["subdomain"], serde_json::json!("repaired-by-session"), "{view}");

            // The persisted file CONTENT actually changed.
            let after = tunnel_json_raw();
            assert_ne!(before, after, "tunnel.json must change on a successful POST");
            let stored = json(&after);
            assert_eq!(
                stored["subdomain"],
                serde_json::json!("repaired-by-session"),
                "persisted tunnel.json must carry the new subdomain: {after}"
            );

            // GET config with the session → redacted view (token never leaks).
            let r = http(d.port, "GET", &format!("/cli/tunnel/config?token={sess}"), None);
            assert_eq!(r.status, 200, "owner-role config GET; body={}", r.body);
            let view = json(&r.body);
            assert_eq!(view["subdomain"], serde_json::json!("repaired-by-session"), "{view}");
            assert!(view.get("token").is_none(), "redacted view must not carry the token: {view}");

            // GET /cli/tunnel/status with the session → 200 (owner-role read).
            let r = http(d.port, "GET", &format!("/cli/tunnel/status?token={sess}"), None);
            assert_eq!(r.status, 200, "owner-role status GET; body={}", r.body);
            assert_eq!(json(&r.body)["running"], serde_json::json!(false), "body={}", r.body);
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (2)+(3) Admin-role and Member-role sessions: 403 on POST, nothing
//         persisted.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_and_member_sessions_cannot_write_config() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            for (username, password, role) in [
                ("tr_admin1", "hunter2-strong-2", "admin"),
                ("tr_member1", "hunter2-strong-3", "member"),
            ] {
                let sess = provision_and_login(d.port, username, password, role, false);
                let before = tunnel_json_raw();
                let r = http(
                    d.port,
                    "POST",
                    &format!("/cli/tunnel/config?token={sess}"),
                    Some(&format!(r#"{{"subdomain":"hijack-{role}"}}"#)),
                );
                assert_eq!(r.status, 403, "{role}: config POST must 403; body={}", r.body);
                assert_eq!(r.body, FORBIDDEN, "{role}: 403 body is pinned");
                assert_eq!(
                    tunnel_json_raw(),
                    before,
                    "{role}: a 403'd POST must persist NOTHING"
                );
            }
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (4) The owner TOKEN still writes config (pre-existing path unchanged).
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_token_still_writes_config() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            let before = tunnel_json_raw();
            let r = http(
                d.port,
                "POST",
                &format!("/cli/tunnel/config?token={OWNER_TOKEN}"),
                Some(r#"{"subdomain":"owner-token-write"}"#),
            );
            assert_eq!(r.status, 200, "owner-token config POST; body={}", r.body);
            let after = tunnel_json_raw();
            assert_ne!(before, after, "tunnel.json must change");
            assert_eq!(
                json(&after)["subdomain"],
                serde_json::json!("owner-token-write"),
                "persisted subdomain: {after}"
            );
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (5) must-change-password Owner session → 403 password_change_required
//     (the chokepoint fires BEFORE the role gate), released after
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
                "tr_rotate1",
                "temp-password-9",
                "owner",
                true,
            );

            let before = tunnel_json_raw();
            let r = http(
                d.port,
                "POST",
                &format!("/cli/tunnel/config?token={sess}"),
                Some(r#"{"subdomain":"blocked-by-rotation"}"#),
            );
            assert_eq!(
                r.status, 403,
                "restricted owner session must 403 on config POST; body={}",
                r.body
            );
            assert_eq!(r.body, PASSWORD_GATE, "must be the password chokepoint body");
            assert_eq!(tunnel_json_raw(), before, "a password-gated POST persists NOTHING");

            // Rotate → fresh session → the write works (proves the block
            // above was the password gate, not the role gate).
            let r = http(
                d.port,
                "POST",
                &format!("/cli/auth/change-password?token={sess}"),
                Some(r#"{"currentPassword":"temp-password-9","newPassword":"rotated-password-10"}"#),
            );
            assert_eq!(r.status, 200, "change-password; body={}", r.body);
            let v = login(d.port, "tr_rotate1", "rotated-password-10");
            assert_eq!(v["mustChangePassword"], serde_json::json!(false), "{v}");
            let fresh = v["token"].as_str().expect("token");
            let r = http(
                d.port,
                "POST",
                &format!("/cli/tunnel/config?token={fresh}"),
                Some(r#"{"subdomain":"post-rotation-repair"}"#),
            );
            assert_eq!(r.status, 200, "post-rotation config POST; body={}", r.body);
            assert_eq!(
                json(&tunnel_json_raw())["subdomain"],
                serde_json::json!("post-rotation-repair"),
                "post-rotation write persisted"
            );
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (6) start/stop REMAIN owner-token-only — even an Owner-ROLE session is
//     403'd — and their POST-only guards hold.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_stop_stay_owner_token_only_even_for_owner_role() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            let sess =
                provision_and_login(d.port, "tr_owner2", "hunter2-strong-4", "owner", false);

            for path in ["/cli/tunnel/start", "/cli/tunnel/stop"] {
                // Owner-ROLE session → 403 (the deliberate footgun guard:
                // config-POST relaxed, start/stop NOT).
                let r = http(d.port, "POST", &format!("{path}?token={sess}"), Some(""));
                assert_eq!(
                    r.status, 403,
                    "owner-ROLE session must NOT reach {path} (owner-token-only); body={}",
                    r.body
                );

                // POST-only guard (house rule) still holds.
                let r = http(d.port, "GET", &format!("{path}?token={OWNER_TOKEN}"), None);
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
