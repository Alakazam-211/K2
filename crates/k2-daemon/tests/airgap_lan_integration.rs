//! Air-gap flag + LAN listen (prd-air-gap-and-lan-listen-v1).
//!
//! Loud tests: no skip-if-missing, no live GitHub/Connect. LAN bind is
//! proven by `local_addr()` being unspecified (`0.0.0.0`).

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::connect_users;
use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-airgap-lan-v1";

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
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
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
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

struct EnvRestore {
    air: Option<std::ffi::OsString>,
}
impl EnvRestore {
    fn airgap(val: Option<&str>) -> Self {
        let air = std::env::var_os("K2_AIRGAP");
        match val {
            Some(v) => std::env::set_var("K2_AIRGAP", v),
            None => std::env::remove_var("K2_AIRGAP"),
        }
        Self { air }
    }
}
impl Drop for EnvRestore {
    fn drop(&mut self) {
        match &self.air {
            Some(p) => std::env::set_var("K2_AIRGAP", p),
            None => std::env::remove_var("K2_AIRGAP"),
        }
        k2_core::listen::set_lan_bound(false);
        k2_core::airgap::set_setting_enabled(false);
    }
}

fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("k2-airgap-lan-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(tmp.join(".k2")).expect("temp HOME/.k2");
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();
    f();
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_status_advertises_airgap_and_listen_without_bumping_protocol() {
    let _g = lock();
    let _env = EnvRestore::airgap(Some("1"));
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let r = http(d.port, "GET", "/boot-status", None);
        assert_eq!(r.status, 200, "boot-status must answer; body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(
            v["protocol"],
            serde_json::json!(k2_daemon::boot_status::PROTOCOL)
        );
        assert_eq!(
            v["airgap"]["enabled"],
            serde_json::json!(true),
            "airgap.enabled must be true; body={}",
            r.body
        );
        assert_eq!(
            v["airgap"]["baked"],
            serde_json::json!(k2_core::airgap::baked()),
            "airgap.baked must track the cargo feature; body={}",
            r.body
        );
        assert!(
            v["listen"]["lan"].is_boolean(),
            "listen.lan must be present; body={}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_against_unspecified_listener() {
    let _g = lock();
    let _env = EnvRestore::airgap(None);
    with_temp_home(|| {
        connect_users::add_user("lanuser", "password123").expect("add lanuser");
        let d = futures_block(test_harness::start_on(OWNER_TOKEN, "0.0.0.0:0"));
        assert!(
            d.local_addr.ip().is_unspecified(),
            "LAN test listener local_addr must be 0.0.0.0, got {}",
            d.local_addr.ip()
        );
        let r = http(
            d.port,
            "POST",
            "/cli/auth/login",
            Some(r#"{"username":"lanuser","password":"password123"}"#),
        );
        assert_eq!(
            r.status, 200,
            "login against 0.0.0.0-bound listener must 200; body={}",
            r.body
        );
        assert!(
            r.body.contains("\"token\""),
            "login 200 returns a token; body={}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_check_is_noop_when_airgap() {
    let _g = lock();
    let _env = EnvRestore::airgap(Some("garbage")); // fail closed = on
    with_temp_home(|| {
        assert!(
            k2_core::airgap::enabled(),
            "K2_AIRGAP=garbage must enable (fail closed)"
        );
        let r = k2_daemon::update_routes::handle_check();
        assert_eq!(r.status, "403 Forbidden", "body={}", r.body);
        assert!(
            r.body.contains("K2_AIRGAP=1"),
            "teaching error must name the env; body={}",
            r.body
        );
    });
}

fn futures_block<F: std::future::Future>(f: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(f))
}
