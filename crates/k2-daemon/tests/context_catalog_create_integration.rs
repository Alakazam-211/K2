//! Host catalog create/delete auth gates.
//!
//! Locks: POST `/cli/context/catalog/create` (and delete) are
//! `require_post` + `require_manage`. GET create → 405. GET catalog stays
//! `token_ok` (member 200). Member/Viewer POST → 403; owner/admin POST → 200.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex as StdMutex;

use k2_core::connect_users::{self, Role};
use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("read timeout");
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
             Host: 127.0.0.1\r\n\
             \r\n"
        ),
    };
    stream.write_all(req.as_bytes()).expect("write");
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
            Err(e) => panic!("read: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn try_parse(raw: &[u8]) -> Option<(u16, String, bool)> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())?;
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
    let tmp = std::env::temp_dir().join(format!(
        "k2-ctx-catalog-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp).expect("temp HOME");
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();
    f();
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

fn seed_user_session(username: &str, password: &str, role: Role) -> String {
    connect_users::add_user(username, password).expect("add_user");
    connect_users::set_role(username, role).expect("set_role");
    connect_users::create_session(username)
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

const OWNER_TOKEN: &str = "owner-token-catalog-create";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_create_get_is_405() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let r = http(
            d.port,
            "GET",
            &format!("/cli/context/catalog/create?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(
            r.status, 405,
            "GET /cli/context/catalog/create must be 405; body={}",
            r.body
        );
        let r = http(
            d.port,
            "GET",
            &format!("/cli/context/catalog/delete?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(r.status, 405, "GET delete must be 405; body={}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_get_includes_user_pack_for_member() {
    let _g = lock();
    with_temp_home(|| {
        let member = seed_user_session("catmember", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let create = http(
            d.port,
            "POST",
            &format!("/cli/context/catalog/create?token={OWNER_TOKEN}"),
            Some(r#"{"id":"user:on-call","label":"On-call"}"#),
        );
        assert_eq!(create.status, 200, "owner create: {}", create.body);

        let r = http(
            d.port,
            "GET",
            &format!("/cli/context/catalog?token={member}"),
            None,
        );
        assert_eq!(r.status, 200, "member GET catalog must be token_ok; {}", r.body);
        assert!(
            r.body.contains("user:on-call") && r.body.contains("catalog:user:on-call"),
            "catalog list must include user pack; {}",
            r.body
        );
        assert!(
            !r.body.contains("\"source\":\"user\""),
            "user pack must not use source=user; {}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_create_member_and_viewer_403_owner_admin_200() {
    let _g = lock();
    with_temp_home(|| {
        let member = seed_user_session("cmember", "password123", Role::Member);
        let viewer = seed_user_session("cviewer", "password123", Role::Viewer);
        let admin = seed_user_session("cadmin", "password123", Role::Admin);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        let r = http(
            d.port,
            "POST",
            &format!("/cli/context/catalog/create?token={member}"),
            Some(r#"{"id":"user:from-member"}"#),
        );
        assert_eq!(r.status, 403, "member create must 403; {}", r.body);

        let r = http(
            d.port,
            "POST",
            &format!("/cli/context/catalog/create?token={viewer}"),
            Some(r#"{"id":"user:from-viewer"}"#),
        );
        assert_eq!(r.status, 403, "viewer create must 403; {}", r.body);

        let r = http(
            d.port,
            "POST",
            &format!("/cli/context/catalog/create?token={OWNER_TOKEN}"),
            Some(r#"{"id":"user:from-owner"}"#),
        );
        assert_eq!(r.status, 200, "owner create must 200; {}", r.body);

        let r = http(
            d.port,
            "POST",
            &format!("/cli/context/catalog/create?token={admin}"),
            Some(r#"{"id":"user:from-admin"}"#),
        );
        assert_eq!(r.status, 200, "admin create must 200; {}", r.body);
    });
}
