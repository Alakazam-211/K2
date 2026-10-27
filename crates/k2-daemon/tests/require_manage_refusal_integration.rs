//! 0.46.3 (agent CLI audit A3): `require_manage` tells a signed-in caller
//! who can't manage that the route is owner/admin only, instead of
//! "invalid or missing token". Through the REAL dispatcher
//! (`k2_daemon::test_harness`).
//!
//!   1. No token / an unknown token: status and body stay byte-identical
//!      (`403 {"error":"invalid or missing token"}`) — clients pin it.
//!   2. A valid agent passport on a manage route: 403 `owner/admin only`.
//!   3. The owner token still passes.
//!
//! Fail loudly: every assertion names what it saw; no defaults.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

const OWNER_TOKEN: &str = "owner-token-require-manage-refusal";
const INVALID_BODY: &str = r#"{"error":"invalid or missing token"}"#;

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("set read timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{b}",
            b.len()
        ),
        None => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        ),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    let mut raw = Vec::new();
    match stream.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("read response: {e:?}"),
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_else(|| panic!("no header/body split: {text:?}"));
    Resp { status, body }
}

fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!("k2-req-manage-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(tmp.join(".k2")).expect("create temp HOME/.k2");
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

fn mint_passport() -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal {
            workspace_uuid: uuid::Uuid::new_v4().to_string(),
            agent_address: "manage-refusal-agent".to_string(),
        },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthenticated_stays_byte_identical_and_a_passport_hears_owner_admin_only() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_temp_home(|| {
        let _ = k2_core::db::init_for_tests();
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // 1 — no token, empty token, unknown token: unchanged 403 body.
        for (what, q) in [
            ("no token", "/cli/users"),
            ("empty token", "/cli/users?token="),
            ("unknown token", "/cli/users?token=not-a-real-token"),
        ] {
            let r = http(d.port, "GET", q, None);
            assert_eq!(r.status, 403, "{what}: {}", r.body);
            assert_eq!(r.body, INVALID_BODY, "{what}: body must stay byte-identical");
        }
        // The same on a POST manage route.
        let r = http(
            d.port,
            "POST",
            "/cli/users/add?token=not-a-real-token",
            Some(r#"{"username":"x","password":"password123"}"#),
        );
        assert_eq!(r.status, 403, "unknown token POST: {}", r.body);
        assert_eq!(r.body, INVALID_BODY, "unknown token POST: body must stay byte-identical");

        // 2 — a valid agent passport is signed in but can't manage.
        let passport = mint_passport();
        let r = http(d.port, "GET", &format!("/cli/users?token={passport}"), None);
        assert_eq!(r.status, 403, "passport GET /cli/users: {}", r.body);
        assert_ne!(r.body, INVALID_BODY, "a signed-in agent must not hear 'invalid or missing token'");
        let v: serde_json::Value = serde_json::from_str(&r.body)
            .unwrap_or_else(|e| panic!("passport refusal is not JSON ({e}): {}", r.body));
        assert_eq!(v["error"], "owner/admin only", "passport refusal: {v}");
        assert_eq!(v["code"], "owner_admin_only", "passport refusal: {v}");

        // 3 — the owner token still manages.
        let r = http(d.port, "GET", &format!("/cli/users?token={OWNER_TOKEN}"), None);
        assert_eq!(r.status, 200, "owner GET /cli/users: {}", r.body);
    });
}

/// 0.46.3 B1: `GET /cli/daemon/update/latest` is readable by a Member
/// login (the phone's cheap update signal), refused without a token, and
/// never fetches anything (null until a check has run).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_latest_is_member_readable_and_never_fetches() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_temp_home(|| {
        // A URL nothing answers: a fetch here would fail the 200 below.
        std::env::set_var("K2_DAEMON_MANIFEST_URL", "http://127.0.0.1:9/never");
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        k2_core::connect_users::add_user("member_latest", "password123").expect("add_user");
        let member = k2_core::connect_users::create_session("member_latest");

        let r = http(d.port, "GET", &format!("/cli/daemon/update/latest?token={member}"), None);
        assert_eq!(r.status, 200, "member GET latest: {}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body)
            .unwrap_or_else(|e| panic!("latest is not JSON ({e}): {}", r.body));
        assert!(v["current"].as_str().is_some(), "current: {v}");
        assert!(v["installKind"].as_str().is_some(), "installKind: {v}");
        assert!(v.get("latestKnown").is_some(), "latestKnown key present: {v}");

        let r = http(d.port, "GET", "/cli/daemon/update/latest", None);
        assert_eq!(r.status, 403, "no token: {}", r.body);
        assert_eq!(r.body, INVALID_BODY);

        // check stays Admin-only: a Member still can't fetch.
        let r = http(d.port, "POST", &format!("/cli/daemon/update/check?token={member}"), Some("{}"));
        assert_eq!(r.status, 403, "member check: {}", r.body);
        std::env::remove_var("K2_DAEMON_MANIFEST_URL");
    });
}
