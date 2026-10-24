//! phone-web-off (Rosson 2026-10-24): "web access off" blocks browsers but
//! never a paired Companion phone.
//!
//! The Companion signs in through the same public door as the web app
//! (`<label>.app.k2.dev`, where the edge stamps `X-K2-Client: web` and an
//! `X-Forwarded-Host` under `.app.k2.dev`). These tests send exactly those
//! edge markers to a real in-process daemon (temp `$HOME`) and check:
//!
//! - web off + a device session (Bearer / `?token=`) → allowed;
//! - web off + a browser session (cookie, or a password login's token) →
//!   403 `WEB_CLIENT_DISABLED`;
//! - web off + a forged header with no valid device session → refused;
//! - web off: the phone's first sign-in (`POST /cli/auth/device/pair`)
//!   works, and the App device sign-in refresh is not walled;
//! - a pairing code can't be minted from a browser-shaped request, web on
//!   or off; who may mint for whom; list + revoke;
//! - web on → unchanged.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::connect_users;
use k2_daemon::test_harness;
use serde_json::Value;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-phone-web-off-0aa1";

/// The headers the K2 Connect edge adds to every proxied request.
const EDGE: &[&str] = &["X-K2-Client: web", "X-Forwarded-Host: rosson.app.k2.dev", "X-Forwarded-Proto: https"];

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {:?}", self.body))
    }
    fn code(&self) -> String {
        match self.json()["error"]["code"].as_str() {
            Some(c) => c.to_string(),
            None => panic!("no error.code in {:?}", self.body),
        }
    }
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>, extra_headers: &[&str]) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream.set_read_timeout(Some(Duration::from_secs(10))).expect("set read timeout");
    let mut extra = String::new();
    for h in extra_headers {
        extra.push_str(h);
        extra.push_str("\r\n");
    }
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: rosson.k2.dev\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: rosson.k2.dev\r\nConnection: close\r\n{extra}\r\n"),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");
    let mut raw = Vec::new();
    loop {
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof) => break,
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));
    let body = match text.split_once("\r\n\r\n") {
        Some((_h, b)) => b.to_string(),
        None => panic!("no header/body split in {text:?}"),
    };
    Resp { status, body }
}

struct HomeEnv {
    prev_home: Option<std::ffi::OsString>,
    tmp_home: std::path::PathBuf,
}

impl HomeEnv {
    fn set() -> Self {
        let prev_home = std::env::var_os("HOME");
        let tmp_home = std::env::temp_dir().join(format!("k2-phone-web-off-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp_home).expect("create temp HOME");
        std::env::set_var("HOME", &tmp_home);
        Self { prev_home, tmp_home }
    }
}

impl Drop for HomeEnv {
    fn drop(&mut self) {
        match &self.prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.tmp_home);
    }
}

fn with(a: &[&str], b: &[&str]) -> Vec<String> {
    a.iter().chain(b.iter()).map(|s| s.to_string()).collect()
}

fn refs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

fn set_web(port: u16, on: bool) {
    let r = http(
        port,
        "POST",
        &format!("/cli/settings/update?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"webClientEnabled":{on}}}"#)),
        &[],
    );
    assert_eq!(r.status, 200, "settings update: {}", r.body);
    assert_eq!(r.json()["webClientEnabled"], on, "{}", r.body);
}

/// Owner token (loopback, no web markers) mints a code for `user`.
fn owner_code(port: u16, user: &str) -> String {
    let r = http(
        port,
        "POST",
        &format!("/cli/auth/device/pair-code?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"user":"{user}"}}"#)),
        &[],
    );
    assert_eq!(r.status, 200, "owner pair-code: {}", r.body);
    let v = r.json();
    assert_eq!(v["user"], user, "{}", r.body);
    assert_eq!(v["expiresInSecs"], 600, "{}", r.body);
    match v["code"].as_str() {
        Some(c) if c.starts_with("K2P-") => c.to_string(),
        _ => panic!("no code in {}", r.body),
    }
}

/// The phone redeems `code` through the edge (web markers on).
fn pair_through_edge(port: u16, code: &str, device_id: &str) -> Value {
    let body = format!(r#"{{"code":"{code}","device":{{"id":"{device_id}","name":"Rosson's iPhone"}}}}"#);
    let r = http(port, "POST", "/cli/auth/device/pair", Some(&body), EDGE);
    assert_eq!(r.status, 200, "pair through the edge: {}", r.body);
    let v = r.json();
    assert!(v["token"].as_str().is_some_and(|t| !t.is_empty()), "{}", r.body);
    assert_eq!(v["device"]["id"], device_id, "{}", r.body);
    assert_eq!(v["slidingDays"], 30, "{}", r.body);
    v
}

fn bearer(tok: &str) -> String {
    format!("Authorization: Bearer {tok}")
}

#[tokio::test(flavor = "multi_thread")]
async fn web_off_lets_a_paired_phone_through_and_keeps_browsers_out() {
    let _g = lock();
    let _home = HomeEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    let port = d.port;
    connect_users::add_user("phone_user", "password123").expect("add phone_user");
    connect_users::add_user("other_user", "password123").expect("add other_user");

    // ── web ON: pair a phone the normal way, through the edge ──────────
    let code = owner_code(port, "phone_user");
    let paired = pair_through_edge(port, &code, "ios-install-a");
    let device = paired["token"].as_str().expect("token").to_string();
    assert_eq!(paired["username"], "phone_user");
    let r = http(port, "POST", "/cli/auth/device/pair", Some(&format!(r#"{{"code":"{code}","device":{{"id":"x","name":"x"}}}}"#)), EDGE);
    assert_eq!((r.status, r.code()), (401, "pair_code_invalid".to_string()), "a code pairs once: {}", r.body);

    // A browser session (password login → cookie) while web is ON works.
    let browser = connect_users::create_session("phone_user");
    let cookie = format!("Cookie: k2_session={browser}");
    let web_on = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&cookie, "Origin: https://rosson.app.k2.dev"])));
    assert_eq!(web_on.status, 200, "web on: a browser session works: {}", web_on.body);

    // ── web OFF ────────────────────────────────────────────────────────
    set_web(port, false);

    // 1. A device session through the edge → allowed (Bearer and ?token=).
    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&bearer(&device)])));
    assert_eq!(r.status, 200, "web off + device session (Bearer): {}", r.body);
    assert_eq!(r.json()["username"], "phone_user", "{}", r.body);
    let r = http(port, "GET", &format!("/cli/auth/whoami?token={device}"), None, EDGE);
    assert_eq!(r.status, 200, "web off + device session (?token=): {}", r.body);
    let r = http(port, "GET", "/cli/auth/devices", None, &refs(&with(EDGE, &[&bearer(&device)])));
    assert_eq!(r.status, 200, "{}", r.body);
    let list = r.json()["devices"].as_array().cloned().unwrap_or_else(|| panic!("devices list: {}", r.body));
    assert_eq!(list.len(), 1, "{}", r.body);
    assert_eq!(list[0]["deviceId"], "ios-install-a");
    assert_eq!(list[0]["current"], true, "the caller's own phone is marked: {}", r.body);

    // 2. A browser session → 403 WEB_CLIENT_DISABLED (cookie, or the
    //    password login's token as a Bearer).
    for (what, hdr) in [("cookie", cookie.clone()), ("bearer", bearer(&browser))] {
        let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&hdr])));
        assert_eq!((r.status, r.code()), (403, "WEB_CLIENT_DISABLED".to_string()), "web off + browser {what}: {}", r.body);
    }
    // The browser's password login itself stays walled.
    let r = http(port, "POST", "/cli/auth/login", Some(r#"{"username":"phone_user","password":"password123","web":true}"#), EDGE);
    assert_eq!((r.status, r.code()), (403, "WEB_CLIENT_DISABLED".to_string()), "web off + password login via the edge: {}", r.body);

    // 3. A forged header without a valid device session → refused.
    for (what, hdrs) in [
        ("no credential", vec!["X-K2-Client: web".to_string()]),
        ("garbage token", vec!["X-K2-Client: web".to_string(), bearer("0123456789abcdef")]),
        ("owner token", vec!["X-K2-Client: web".to_string(), bearer(OWNER_TOKEN)]),
        ("device token in a cookie", vec!["X-K2-Client: web".to_string(), format!("Cookie: k2_session={device}")]),
        ("X-K2-CSRF + plain session", vec!["X-K2-CSRF: web".to_string(), bearer(&browser)]),
    ] {
        let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&hdrs));
        assert_eq!((r.status, r.code()), (403, "WEB_CLIENT_DISABLED".to_string()), "web off + forged header ({what}): {}", r.body);
    }
    // The non-web CLI / desktop path is unchanged.
    let r = http(port, "GET", &format!("/cli/auth/whoami?token={OWNER_TOKEN}"), None, &[]);
    assert_eq!(r.status, 200, "{}", r.body);

    // 4. A browser can't mint a pairing code (web off, so the wall answers
    //    for web-marked requests; a device session through the edge passes
    //    the wall and the mint route itself refuses).
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some("{}"), &refs(&with(EDGE, &[&cookie])));
    assert_eq!((r.status, r.code()), (403, "WEB_CLIENT_DISABLED".to_string()), "{}", r.body);
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some("{}"), &refs(&with(EDGE, &[&bearer(&device)])));
    assert_eq!((r.status, r.code()), (403, "device_pairing_not_from_browser".to_string()), "{}", r.body);
    let r = http(
        port,
        "POST",
        "/cli/auth/device/pair-code",
        Some("{}"),
        &[&bearer(&browser), "Origin: https://rosson.app.k2.dev", "Sec-Fetch-Site: cross-site"],
    );
    assert_eq!((r.status, r.code()), (403, "device_pairing_not_from_browser".to_string()), "a browser calling the direct host: {}", r.body);

    // 5. The phone's FIRST sign-in works with web off: a code from the
    //    desktop (owner token), redeemed through the edge.
    let code = owner_code(port, "phone_user");
    let second = pair_through_edge(port, &code, "android-install-b");
    let device_b = second["token"].as_str().expect("token").to_string();
    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&bearer(&device_b)])));
    assert_eq!(r.status, 200, "a phone paired while web is off: {}", r.body);
    //    …or a code the signed-in Companion makes for itself (its own
    //    native request, the Tauri origin, no edge markers).
    let r = http(
        port,
        "POST",
        "/cli/auth/device/pair-code",
        Some("{}"),
        &[&bearer(&device), "Origin: tauri://localhost", "Sec-Fetch-Site: cross-site"],
    );
    assert_eq!(r.status, 200, "self pair-code from the Companion: {}", r.body);
    assert_eq!(r.json()["user"], "phone_user", "{}", r.body);
    //    A login can't name someone else.
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some(r#"{"user":"other_user"}"#), &[&bearer(&device)]);
    assert_eq!((r.status, r.code()), (403, "owner_required".to_string()), "{}", r.body);
    //    The owner token has no login of its own.
    let r = http(port, "POST", &format!("/cli/auth/device/pair-code?token={OWNER_TOKEN}"), Some("{}"), &[]);
    assert_eq!((r.status, r.code()), (400, "usage".to_string()), "{}", r.body);
    //    Unknown login.
    let r = http(port, "POST", &format!("/cli/auth/device/pair-code?token={OWNER_TOKEN}"), Some(r#"{"user":"nobody"}"#), &[]);
    assert_eq!((r.status, r.code()), (400, "no_such_login".to_string()), "{}", r.body);
    //    No credential at all.
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some(r#"{"user":"phone_user"}"#), &[]);
    assert_eq!(r.status, 403, "{}", r.body);

    // Bad redeems: one answer, no oracle.
    let r = http(port, "POST", "/cli/auth/device/pair", Some(r#"{"code":"K2P-00000-00000-00000-00000","device":{"id":"d","name":"D"}}"#), EDGE);
    assert_eq!((r.status, r.code()), (401, "pair_code_invalid".to_string()), "{}", r.body);
    let code = owner_code(port, "phone_user");
    let r = http(port, "POST", "/cli/auth/device/pair", Some(&format!(r#"{{"code":"{code}"}}"#)), EDGE);
    assert_eq!((r.status, r.code()), (400, "usage".to_string()), "device is required: {}", r.body);
    let r = http(port, "POST", "/cli/auth/device/pair", Some(&format!(r#"{{"code":"{code}","device":{{"id":"bad id!","name":"x"}}}}"#)), EDGE);
    assert_eq!((r.status, r.code()), (400, "usage".to_string()), "{}", r.body);
    let r = http(port, "GET", "/cli/auth/device/pair", None, EDGE);
    assert_eq!(r.status, 405, "pair is POST only: {}", r.body);
    let r = http(port, "GET", &format!("/cli/auth/device/pair-code?token={OWNER_TOKEN}"), None, &[]);
    assert_eq!(r.status, 405, "pair-code is POST only: {}", r.body);

    // App device sign-in (0.46.3) is not walled either: its refresh answers
    // for itself (a garbage refresh token → refresh_invalid).
    let r = http(port, "POST", "/cli/skin/refresh", Some(r#"{"refresh":"k2skr_nope"}"#), EDGE);
    assert_ne!(r.status, 403, "App device refresh must not be walled: {}", r.body);
    assert_eq!(r.code(), "refresh_invalid", "{}", r.body);

    // 6. Revoke: someone else can't end it; the phone's person can; then
    //    the wall refuses that token.
    let other = connect_users::create_session("other_user");
    let id_b = second["device"]["sessionId"].as_str().expect("sessionId").to_string();
    let r = http(port, "POST", "/cli/auth/device/revoke", Some(&format!(r#"{{"id":"{id_b}"}}"#)), &[&bearer(&other)]);
    assert_eq!((r.status, r.code()), (404, "no_such_device".to_string()), "{}", r.body);
    let r = http(port, "POST", "/cli/auth/device/revoke", Some(&format!(r#"{{"id":"{id_b}"}}"#)), &refs(&with(EDGE, &[&bearer(&device)])));
    assert_eq!(r.status, 200, "own phone revoke: {}", r.body);
    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&bearer(&device_b)])));
    assert_eq!((r.status, r.code()), (403, "WEB_CLIENT_DISABLED".to_string()), "a revoked phone: {}", r.body);
    //    The owner sees and ends everyone's.
    let r = http(port, "GET", &format!("/cli/auth/devices?token={OWNER_TOKEN}"), None, &[]);
    assert_eq!(r.status, 200, "{}", r.body);
    let all = r.json()["devices"].as_array().cloned().unwrap_or_else(|| panic!("{}", r.body));
    assert_eq!(all.len(), 1, "only ios-install-a is left: {}", r.body);
    assert_eq!(all[0]["username"], "phone_user");
    let id_a = all[0]["id"].as_str().expect("id").to_string();
    let r = http(port, "POST", &format!("/cli/auth/device/revoke?token={OWNER_TOKEN}"), Some(&format!(r#"{{"id":"{id_a}"}}"#)), &[]);
    assert_eq!(r.status, 200, "{}", r.body);
    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&bearer(&device)])));
    assert_eq!((r.status, r.code()), (403, "WEB_CLIENT_DISABLED".to_string()), "{}", r.body);

    // ── web ON again: unchanged ────────────────────────────────────────
    set_web(port, true);
    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&cookie, "Origin: https://rosson.app.k2.dev"])));
    assert_eq!(r.status, 200, "web on: browser session: {}", r.body);
    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&bearer(&browser)])));
    assert_eq!(r.status, 200, "web on: bearer session through the edge: {}", r.body);
    // Even with web on, the web app can't mint a phone credential.
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some("{}"), &refs(&with(EDGE, &[&cookie, "Origin: https://rosson.app.k2.dev"])));
    assert_eq!((r.status, r.code()), (403, "device_pairing_not_from_browser".to_string()), "{}", r.body);
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some("{}"), &refs(&with(EDGE, &[&bearer(&browser)])));
    assert_eq!((r.status, r.code()), (403, "device_pairing_not_from_browser".to_string()), "{}", r.body);
}

/// A password change (or any session revoke) ends paired phones like every
/// other session, and voids an unredeemed code.
#[tokio::test(flavor = "multi_thread")]
async fn a_password_change_ends_paired_phones_and_voids_codes() {
    let _g = lock();
    let _home = HomeEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    let port = d.port;
    connect_users::add_user("rotating_user", "password123").expect("add");
    set_web(port, false);
    let paired = pair_through_edge(port, &owner_code(port, "rotating_user"), "ios-1");
    let device = paired["token"].as_str().expect("token").to_string();
    let pending = owner_code(port, "rotating_user");
    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&bearer(&device)])));
    assert_eq!(r.status, 200, "{}", r.body);

    connect_users::set_password("rotating_user", "a-new-password-789").expect("set_password");

    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&bearer(&device)])));
    assert_eq!((r.status, r.code()), (403, "WEB_CLIENT_DISABLED".to_string()), "{}", r.body);
    let r = http(port, "GET", &format!("/cli/auth/whoami?token={device}"), None, &[]);
    assert_eq!(r.status, 403, "the device session is gone on every path: {}", r.body);
    let body = format!(r#"{{"code":"{pending}","device":{{"id":"ios-2","name":"P"}}}}"#);
    let r = http(port, "POST", "/cli/auth/device/pair", Some(&body), EDGE);
    assert_eq!((r.status, r.code()), (401, "pair_code_invalid".to_string()), "{}", r.body);
}
