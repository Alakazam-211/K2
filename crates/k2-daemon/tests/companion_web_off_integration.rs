//! phone-web-off (Rosson 2026-10-24, security review F1–F7): "web access
//! off" blocks browsers but never a paired Companion phone.
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
//! - who may mint for whom: the owner token for a named login or its linked
//!   login (409 `owner_login_not_linked` without one); a login only for
//!   itself and only with its password again; never from a browser;
//! - an ended device session answers 403 `device_session_ended`, sockets
//!   included; audit rows name the actor; the pair notice is filed;
//! - the `/pair` page and the iOS app-site-association file;
//! - web on → unchanged.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::connect_users::{self, Role};
use k2_daemon::test_harness;
use serde_json::Value;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-phone-web-off-0aa1";
const PW: &str = "password123";

/// The headers the K2 Connect edge adds to every proxied request.
const EDGE: &[&str] = &["X-K2-Client: web", "X-Forwarded-Host: rosson.app.k2.dev", "X-Forwarded-Proto: https"];

struct Resp {
    status: u16,
    head: String,
    body: String,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {:?}", self.body))
    }
    fn code(&self) -> String {
        match self.json()["error"]["code"].as_str() {
            Some(c) => c.to_string(),
            None => panic!("no error.code in {} {:?}", self.status, self.body),
        }
    }
    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().find_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    }
}

fn raw(port: u16, req: &str) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream.set_read_timeout(Some(Duration::from_secs(10))).expect("set read timeout");
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");
    let mut buf = Vec::new();
    loop {
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof) => break,
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));
    let (head, body) = match text.split_once("\r\n\r\n") {
        Some((h, b)) => (h.to_string(), b.to_string()),
        None => panic!("no header/body split in {text:?}"),
    };
    Resp { status, head, body }
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>, extra_headers: &[&str]) -> Resp {
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
    raw(port, &req)
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

fn bearer(tok: &str) -> String {
    format!("Authorization: Bearer {tok}")
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

fn add_login(name: &str, role: Role) -> String {
    connect_users::add_user(name, PW).expect("add_user");
    if role != Role::Member {
        connect_users::set_role(name, role).expect("set_role");
    }
    connect_users::create_session(name)
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
    assert!(v["token"].as_str().is_some_and(|t| t.len() == 64), "{}", r.body);
    assert_eq!(v["device"]["id"], device_id, "{}", r.body);
    assert_eq!(v["slidingDays"], 30, "{}", r.body);
    assert_eq!(v["maxDays"], 180, "{}", r.body);
    assert_eq!(v["mustChangePassword"], false, "{}", r.body);
    v
}

fn audit_rows(port: u16, event: &str) -> Vec<Value> {
    let r = http(port, "GET", &format!("/cli/users/audit?tail=1000&token={OWNER_TOKEN}"), None, &[]);
    assert_eq!(r.status, 200, "audit: {}", r.body);
    let v = r.json();
    let rows = v["events"].as_array().cloned().unwrap_or_else(|| panic!("audit shape: {}", r.body));
    rows.into_iter().filter(|e| e["event"] == event).collect()
}

fn assert_code(r: &Resp, status: u16, code: &str, what: &str) {
    assert_eq!((r.status, r.code()), (status, code.to_string()), "{what}: {}", r.body);
}

#[tokio::test(flavor = "multi_thread")]
async fn web_off_lets_a_paired_phone_through_and_keeps_browsers_out() {
    let _g = lock();
    let _home = HomeEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    let port = d.port;
    connect_users::add_user("phone_user", PW).expect("add phone_user");
    connect_users::add_user("other_user", PW).expect("add other_user");

    // ── web ON: pair a phone the normal way, through the edge ──────────
    let code = owner_code(port, "phone_user");
    let paired = pair_through_edge(port, &code, "ios-install-a");
    let device = paired["token"].as_str().expect("token").to_string();
    assert_eq!(paired["username"], "phone_user");
    let r = http(port, "POST", "/cli/auth/device/pair", Some(&format!(r#"{{"code":"{code}","device":{{"id":"x","name":"x"}}}}"#)), EDGE);
    assert_code(&r, 401, "pair_code_invalid", "a code pairs once");

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
    let v = r.json();
    let list = v["devices"].as_array().cloned().unwrap_or_else(|| panic!("devices list: {}", r.body));
    assert_eq!(list.len(), 1, "{}", r.body);
    assert_eq!(list[0]["deviceId"], "ios-install-a");
    assert_eq!(list[0]["current"], true, "the caller's own phone is marked: {}", r.body);
    assert_eq!(list[0]["pairedBy"], "owner-token", "{}", r.body);
    assert_eq!(v["pairAs"]["user"], "phone_user", "{}", r.body);
    assert_eq!(v["pairAs"]["source"], "self", "{}", r.body);
    assert_eq!(v["pairAs"]["needsPassword"], true, "a login types its password: {}", r.body);

    // 2. A browser session → 403 WEB_CLIENT_DISABLED (cookie, or the
    //    password login's token as a Bearer).
    for (what, hdr) in [("cookie", cookie.clone()), ("bearer", bearer(&browser))] {
        let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&hdr])));
        assert_code(&r, 403, "WEB_CLIENT_DISABLED", &format!("web off + browser {what}"));
    }
    let r = http(port, "POST", "/cli/auth/login", Some(r#"{"username":"phone_user","password":"password123","web":true}"#), EDGE);
    assert_code(&r, 403, "WEB_CLIENT_DISABLED", "web off + password login via the edge");

    // 3. A forged header without a valid device session → refused.
    for (what, hdrs) in [
        ("no credential", vec!["X-K2-Client: web".to_string()]),
        ("garbage token", vec!["X-K2-Client: web".to_string(), bearer("0123456789abcdef")]),
        ("owner token", vec!["X-K2-Client: web".to_string(), bearer(OWNER_TOKEN)]),
        ("device token in a cookie", vec!["X-K2-Client: web".to_string(), format!("Cookie: k2_session={device}")]),
        ("X-K2-CSRF + plain session", vec!["X-K2-CSRF: web".to_string(), bearer(&browser)]),
    ] {
        let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&hdrs));
        assert_code(&r, 403, "WEB_CLIENT_DISABLED", &format!("web off + forged header ({what})"));
    }
    let r = http(port, "GET", &format!("/cli/auth/whoami?token={OWNER_TOKEN}"), None, &[]);
    assert_eq!(r.status, 200, "the CLI path is unchanged: {}", r.body);

    // 4. A browser can't mint (the wall answers web-marked requests; a
    //    device session through the edge passes the wall and the mint
    //    route itself refuses; a browser Origin on the direct host too).
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some("{}"), &refs(&with(EDGE, &[&cookie])));
    assert_code(&r, 403, "WEB_CLIENT_DISABLED", "cookie through the edge");
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some(r#"{"password":"password123"}"#), &refs(&with(EDGE, &[&bearer(&device)])));
    assert_code(&r, 403, "device_pairing_not_from_browser", "device session through the edge");
    let r = http(
        port,
        "POST",
        "/cli/auth/device/pair-code",
        Some(r#"{"password":"password123"}"#),
        &[&bearer(&browser), "Origin: https://rosson.app.k2.dev", "Sec-Fetch-Site: cross-site"],
    );
    assert_code(&r, 403, "device_pairing_not_from_browser", "a browser calling the direct host");

    // 5. The phone's FIRST sign-in works with web off: a code from the
    //    desktop (owner token), redeemed through the edge.
    let second = pair_through_edge(port, &owner_code(port, "phone_user"), "android-install-b");
    let device_b = second["token"].as_str().expect("token").to_string();
    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&bearer(&device_b)])));
    assert_eq!(r.status, 200, "a phone paired while web is off: {}", r.body);
    //    …or a code the signed-in Companion makes for itself (native
    //    request or the Tauri origin, no edge markers) with its password.
    let tauri = ["Origin: tauri://localhost", "Sec-Fetch-Site: cross-site"];
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some("{}"), &refs(&with(&tauri, &[&bearer(&device)])));
    assert_code(&r, 400, "usage", "a login must type its password again");
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some(r#"{"password":"nope-nope"}"#), &refs(&with(&tauri, &[&bearer(&device)])));
    assert_code(&r, 401, "password_invalid", "a wrong password");
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some(r#"{"password":"password123"}"#), &refs(&with(&tauri, &[&bearer(&device)])));
    assert_eq!(r.status, 200, "self pair-code from the Companion: {}", r.body);
    assert_eq!(r.json()["user"], "phone_user", "{}", r.body);
    assert_eq!(r.json()["pairUrl"], Value::Null, "no public host on this test box: {}", r.body);
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some(r#"{"user":"other_user","password":"password123"}"#), &[&bearer(&device)]);
    assert_code(&r, 403, "owner_token_required", "a login can't name someone else");
    let r = http(port, "POST", &format!("/cli/auth/device/pair-code?token={OWNER_TOKEN}"), Some(r#"{"user":"nobody"}"#), &[]);
    assert_code(&r, 400, "no_such_login", "unknown login");
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some(r#"{"user":"phone_user"}"#), &[]);
    assert_code(&r, 401, "login_required", "no credential at all");

    // Mint audit rows name actor and target (F2); the pair row carries the
    // minter; a notice went to the person and the owner (F1).
    let mints = audit_rows(port, "device-pair-code");
    assert!(
        mints.iter().any(|e| e["outcome"] == "ok" && e["user"] == "phone_user" && e["actor"] == "user:phone_user"),
        "self mint row: {mints:?}"
    );
    assert!(
        mints.iter().any(|e| e["outcome"] == "ok" && e["user"] == "phone_user" && e["actor"] == "owner-token"),
        "owner mint row: {mints:?}"
    );
    assert!(mints.iter().any(|e| e["outcome"] == "bad_creds" && e["actor"] == "user:phone_user"), "bad password row: {mints:?}");
    let pairs = audit_rows(port, "device-pair");
    assert!(pairs.iter().any(|e| e["outcome"] == "ok" && e["actor"] == "owner-token"), "pair row: {pairs:?}");
    // Review L1: off the tunnel the rows carry the socket peer, not "-".
    assert!(mints.iter().any(|e| e["actor"] == "owner-token" && e["ip"] == "127.0.0.1"), "mint row ip: {mints:?}");
    assert!(pairs.iter().all(|e| e["ip"] == "127.0.0.1"), "pair row ip: {pairs:?}");
    // Review L3: a token that is not session-shaped is no login.
    let r = http(port, "GET", "/cli/auth/devices", None, &[&bearer("not-a-session-token")]);
    assert_code(&r, 401, "login_required", "a garbage bearer");
    let tickets = http(port, "GET", &format!("/cli/feedback/list-all?token={OWNER_TOKEN}"), None, &[]);
    assert_eq!(tickets.status, 200, "{}", tickets.body);
    assert!(tickets.body.contains("paired for phone_user"), "pair notice filed: {}", tickets.body);

    // Bad redeems: one answer, no oracle.
    let r = http(port, "POST", "/cli/auth/device/pair", Some(r#"{"code":"K2P-00000-00000-00000-00000","device":{"id":"d","name":"D"}}"#), EDGE);
    assert_code(&r, 401, "pair_code_invalid", "unknown code");
    let code = owner_code(port, "phone_user");
    let r = http(port, "POST", "/cli/auth/device/pair", Some(&format!(r#"{{"code":"{code}"}}"#)), EDGE);
    assert_code(&r, 400, "usage", "device is required");
    let r = http(port, "POST", "/cli/auth/device/pair", Some(&format!(r#"{{"code":"{code}","device":{{"id":"bad id!","name":"x"}}}}"#)), EDGE);
    assert_code(&r, 400, "usage", "bad device id");
    let r = http(
        port,
        "POST",
        "/cli/auth/device/pair",
        Some(&format!(r#"{{"code":"{code}","device":{{"id":"d","name":"D"}}}}"#)),
        &refs(&with(EDGE, &["Origin: https://evil.example"])),
    );
    assert_code(&r, 403, "device_pairing_not_from_browser", "a browser page redeeming (F5)");
    let r = http(port, "GET", "/cli/auth/device/pair", None, EDGE);
    assert_eq!(r.status, 405, "pair is POST only: {}", r.body);
    let r = http(port, "GET", &format!("/cli/auth/device/pair-code?token={OWNER_TOKEN}"), None, &[]);
    assert_eq!(r.status, 405, "pair-code is POST only: {}", r.body);

    // App device sign-in (0.46.3) is not walled either.
    let r = http(port, "POST", "/cli/skin/refresh", Some(r#"{"refresh":"k2skr_nope"}"#), EDGE);
    assert_code(&r, 401, "refresh_invalid", "App device refresh must not be walled");

    // 6. Revoke: someone else can't end it; the phone's person can; then
    //    every route answers that token 403 device_session_ended, sockets
    //    included, with web on or off.
    let other = connect_users::create_session("other_user");
    let id_b = second["device"]["sessionId"].as_str().expect("sessionId").to_string();
    let r = http(port, "POST", "/cli/auth/device/revoke", Some(&format!(r#"{{"id":"{id_b}"}}"#)), &[&bearer(&other)]);
    assert_code(&r, 404, "no_such_device", "another login's phone");
    let r = http(port, "POST", "/cli/auth/device/revoke", Some(&format!(r#"{{"id":"{id_b}"}}"#)), &refs(&with(EDGE, &[&bearer(&device)])));
    assert_eq!(r.status, 200, "own phone revoke: {}", r.body);
    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&bearer(&device_b)])));
    assert_code(&r, 403, "device_session_ended", "a revoked phone through the edge");
    let r = http(port, "GET", &format!("/cli/auth/whoami?token={device_b}"), None, &[]);
    assert_code(&r, 403, "device_session_ended", "a revoked phone on the direct path");
    let ws = raw(
        port,
        &format!(
            "GET /events?token={device_b} HTTP/1.1\r\nHost: rosson.k2.dev\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        ),
    );
    assert_code(&ws, 403, "device_session_ended", "a revoked phone opening a socket");
    let revokes = audit_rows(port, "device-revoke");
    assert!(revokes.iter().any(|e| e["actor"] == "user:phone_user"), "revoke row: {revokes:?}");
    //    The owner sees and ends everyone's.
    let r = http(port, "GET", &format!("/cli/auth/devices?token={OWNER_TOKEN}"), None, &[]);
    assert_eq!(r.status, 200, "{}", r.body);
    let all = r.json()["devices"].as_array().cloned().unwrap_or_else(|| panic!("{}", r.body));
    assert_eq!(all.len(), 1, "only ios-install-a is left: {}", r.body);
    let id_a = all[0]["id"].as_str().expect("id").to_string();
    let r = http(port, "POST", &format!("/cli/auth/device/revoke?token={OWNER_TOKEN}"), Some(&format!(r#"{{"id":"{id_a}"}}"#)), &[]);
    assert_eq!(r.status, 200, "{}", r.body);
    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&bearer(&device)])));
    assert_code(&r, 403, "device_session_ended", "the owner revoked it");

    // ── web ON again: unchanged ────────────────────────────────────────
    set_web(port, true);
    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&cookie, "Origin: https://rosson.app.k2.dev"])));
    assert_eq!(r.status, 200, "web on: browser session: {}", r.body);
    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&bearer(&browser)])));
    assert_eq!(r.status, 200, "web on: bearer session through the edge: {}", r.body);
    let r = http(port, "GET", "/cli/auth/whoami", None, &[&bearer(&device)]);
    assert_code(&r, 403, "device_session_ended", "web on: an ended phone is still told so");
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some(r#"{"password":"password123"}"#), &refs(&with(EDGE, &[&cookie, "Origin: https://rosson.app.k2.dev"])));
    assert_code(&r, 403, "device_pairing_not_from_browser", "web on: a cookie session can't mint");
    let r = http(port, "POST", "/cli/auth/device/pair-code", Some(r#"{"password":"password123"}"#), &refs(&with(EDGE, &[&bearer(&browser)])));
    assert_code(&r, 403, "device_pairing_not_from_browser", "web on: the web app can't mint");
}

/// Which login a code pairs as (Rosson 2026-10-24): a login → itself; the
/// owner token with `user` → that login; the owner token alone → the login
/// linked to the owner, else 409 `owner_login_not_linked`. Owner and Admin
/// LOGINS can't mint for anyone else (F2).
#[tokio::test(flavor = "multi_thread")]
async fn who_a_pairing_code_signs_in_as() {
    let _g = lock();
    let _home = HomeEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    let port = d.port;
    let owner_login = add_login("boss", Role::Owner);
    let admin_login = add_login("deputy", Role::Admin);
    let _member = add_login("crew", Role::Member);

    // Owner token, no user, nothing linked → 409; pairAs says so.
    let r = http(port, "POST", &format!("/cli/auth/device/pair-code?token={OWNER_TOKEN}"), Some("{}"), &[]);
    assert_code(&r, 409, "owner_login_not_linked", "owner token with no linked login");
    assert!(r.json()["error"]["hint"].as_str().is_some_and(|h| h.contains("Your account")), "{}", r.body);
    let r = http(port, "GET", &format!("/cli/auth/devices?token={OWNER_TOKEN}"), None, &[]);
    assert_eq!(r.json()["pairAs"]["user"], Value::Null, "{}", r.body);
    assert_eq!(r.json()["pairAs"]["error"]["code"], "owner_login_not_linked", "{}", r.body);

    // Review M1: a Member-only link is not the owner's admin account → 409.
    k2_core::app_settings::update(serde_json::json!({ "ownerAccounts": ["crew"] })).expect("link crew");
    let r = http(port, "POST", &format!("/cli/auth/device/pair-code?token={OWNER_TOKEN}"), Some("{}"), &[]);
    assert_code(&r, 409, "owner_login_not_linked", "a Member-only linked login");
    assert!(r.json()["error"]["hint"].as_str().is_some_and(|h| h.contains("admin login")), "{}", r.body);

    // Linked → the code pairs as the linked login.
    k2_core::app_settings::update(serde_json::json!({ "ownerAccounts": ["deputy"] })).expect("link deputy");
    let r = http(port, "GET", &format!("/cli/auth/devices?token={OWNER_TOKEN}"), None, &[]);
    let pa = r.json()["pairAs"].clone();
    assert_eq!((pa["user"].clone(), pa["source"].clone(), pa["needsPassword"].clone()), (Value::from("deputy"), Value::from("owner-linked"), Value::from(false)), "{}", r.body);
    assert_eq!(pa["choices"], serde_json::json!(["boss", "crew", "deputy"]), "the owner token may pick any enabled login: {}", r.body);
    let r = http(port, "GET", "/cli/auth/devices", None, &[&bearer(&owner_login)]);
    assert_eq!(r.json()["pairAs"].get("choices"), None, "a login gets no choices (F2): {}", r.body);
    let r = http(port, "POST", &format!("/cli/auth/device/pair-code?token={OWNER_TOKEN}"), Some("{}"), &[]);
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["user"], "deputy", "{}", r.body);
    let paired = pair_through_edge(port, r.json()["code"].as_str().expect("code"), "linked-1");
    assert_eq!(paired["username"], "deputy");
    let crew_phone = pair_through_edge(port, &owner_code(port, "crew"), "crew-1");
    assert_eq!(crew_phone["username"], "crew");
    // mine=1 (the profile modal): the owner token sees its linked login's
    // phones only; without it, everyone's.
    let r = http(port, "GET", &format!("/cli/auth/devices?mine=1&token={OWNER_TOKEN}"), None, &[]);
    let mine: Vec<Value> = r.json()["devices"].as_array().cloned().unwrap_or_else(|| panic!("{}", r.body));
    assert_eq!(mine.iter().map(|d| d["username"].clone()).collect::<Vec<_>>(), vec![Value::from("deputy")], "{}", r.body);
    let r = http(port, "GET", &format!("/cli/auth/devices?token={OWNER_TOKEN}"), None, &[]);
    assert_eq!(r.json()["devices"].as_array().map(Vec::len), Some(2), "{}", r.body);

    // Owner token naming a login → that login.
    let named = http(port, "POST", &format!("/cli/auth/device/pair-code?token={OWNER_TOKEN}"), Some(r#"{"user":"crew"}"#), &[]);
    assert_eq!(named.json()["user"], "crew", "{}", named.body);

    // Owner and Admin LOGINS: themselves only.
    for (who, tok) in [("boss", &owner_login), ("deputy", &admin_login)] {
        let r = http(port, "POST", "/cli/auth/device/pair-code", Some(r#"{"user":"crew","password":"password123"}"#), &[&bearer(tok)]);
        assert_code(&r, 403, "owner_token_required", &format!("{who} minting for crew"));
        let r = http(port, "POST", "/cli/auth/device/pair-code", Some(r#"{"password":"password123"}"#), &[&bearer(tok)]);
        assert_eq!(r.status, 200, "{who} for itself: {}", r.body);
        assert_eq!(r.json()["user"], who, "{}", r.body);
    }

    // A login that must change its password gets no code (409).
    connect_users::set_must_change_password("crew", true).expect("flag crew");
    let r = http(port, "POST", &format!("/cli/auth/device/pair-code?token={OWNER_TOKEN}"), Some(r#"{"user":"crew"}"#), &[]);
    assert_code(&r, 409, "password_change_required", "owner token naming a must-change login");

    // Pending codes per login are capped at 3: the oldest stops working.
    let codes: Vec<String> = (0..4).map(|_| owner_code(port, "boss")).collect();
    let body = format!(r#"{{"code":"{}","device":{{"id":"cap-0","name":"C"}}}}"#, codes[0]);
    let r = http(port, "POST", "/cli/auth/device/pair", Some(&body), EDGE);
    assert_code(&r, 401, "pair_code_invalid", "the 4th code evicted the 1st");
    pair_through_edge(port, &codes[3], "cap-3");
}

/// The landing page and the iOS app-site-association file: exact content,
/// no redirect, no credential, open with web access off.
#[tokio::test(flavor = "multi_thread")]
async fn the_pair_page_and_app_links_answer_with_web_off() {
    let _g = lock();
    let _home = HomeEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    let port = d.port;
    set_web(port, false);
    for hdrs in [vec![], EDGE.to_vec()] {
        let r = http(port, "GET", "/.well-known/apple-app-site-association", None, &hdrs);
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.header("content-type").as_deref(), Some("application/json"));
        assert_eq!(r.header("location"), None, "no redirect");
        let v = r.json();
        assert_eq!(
            v,
            serde_json::json!({"applinks":{"apps":[],"details":[{
                "appID":"36B8R93HXV.com.alakazamlabs.k2so.companion",
                "paths":["/pair"],
                "appIDs":["36B8R93HXV.com.alakazamlabs.k2so.companion"],
                "components":[{"/":"/pair"}]
            }]}}),
            "{}",
            r.body
        );

        let r = http(port, "GET", "/pair", None, &hdrs);
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.header("content-type").as_deref(), Some("text/html; charset=utf-8"));
        let csp = r.header("content-security-policy").expect("CSP");
        assert!(csp.starts_with("default-src 'none'; script-src 'nonce-"), "{csp}");
        assert_eq!(r.header("referrer-policy").as_deref(), Some("no-referrer"));
        assert_eq!(r.header("cache-control").as_deref(), Some("no-store"));
        assert!(r.body.contains("Open this in the K2 app"));
        assert!(r.body.contains("k2companion://pair?server="));
    }
    // A query is ignored; a fragment never reaches a server at all.
    let r = http(port, "GET", "/pair?code=leak", None, &[]);
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!r.body.contains("leak"), "the page never echoes the request");
    // Nothing else is served under /.well-known/ by this dispatcher (ACME
    // HTTP-01 has its own :80 listener), so an ACME path answers as before.
    let r = http(port, "GET", "/.well-known/acme-challenge/some-token", None, &[]);
    assert_eq!(r.status, 404, "{}", r.body);
    let r = http(port, "GET", "/.well-known/assetlinks.json", None, &[]);
    assert_eq!(r.status, 404, "not served until the Play fingerprint is known: {}", r.body);
}

/// Companion's pair screen: `POST /cli/auth/device/pair/check` says what a
/// code pairs as WITHOUT using it, works with web access off through the
/// edge, and shares the redeem's buckets and failure accounting.
#[tokio::test(flavor = "multi_thread")]
async fn pair_check_never_uses_a_code_and_is_throttled_like_a_pair() {
    let _g = lock();
    let _home = HomeEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    let port = d.port;
    connect_users::add_user("checked", PW).expect("add");
    set_web(port, false);
    let code = owner_code(port, "checked");
    let check_body = format!(r#"{{"code":"{code}"}}"#);
    for _ in 0..3 {
        let r = http(port, "POST", "/cli/auth/device/pair/check", Some(&check_body), EDGE);
        assert_eq!(r.status, 200, "check with web off through the edge: {}", r.body);
        let v = r.json();
        assert_eq!(v["ok"], true, "{}", r.body);
        assert_eq!(v["user"], "checked", "{}", r.body);
        assert_eq!(v["host"], Value::Null, "no public host on this test box: {}", r.body);
        assert!(v["expiresInSecs"].as_i64().is_some_and(|s| s > 0 && s <= 600), "{}", r.body);
    }
    pair_through_edge(port, &code, "checked-phone");
    let r = http(port, "POST", "/cli/auth/device/pair/check", Some(&check_body), EDGE);
    assert_code(&r, 401, "pair_code_invalid", "a used code");
    let r = http(port, "POST", "/cli/auth/device/pair/check", Some(r#"{"code":"K2P-00000-00000-00000-00000"}"#), EDGE);
    assert_code(&r, 401, "pair_code_invalid", "an unknown code");
    let r = http(port, "POST", "/cli/auth/device/pair/check", Some(&check_body), &refs(&with(EDGE, &["Origin: https://evil.example"])));
    assert_code(&r, 403, "device_pairing_not_from_browser", "a browser page checking");
    let r = http(port, "GET", "/cli/auth/device/pair/check", None, EDGE);
    assert_eq!(r.status, 405, "check is POST only: {}", r.body);
    let checks = audit_rows(port, "device-pair-check");
    assert_eq!(checks.len(), 2, "failed checks are audited, successful ones are not: {checks:?}");
    assert!(checks.iter().all(|e| e["outcome"] == "unknown" && e["ip"] == "127.0.0.1"), "{checks:?}");

    // Throttle, exactly as a hosted server's phone arrives: through
    // <label>.app.k2.dev, i.e. the tunnel ingress (no relay address here)
    // with the Worker's headers, web access off, no Origin. Checks share the
    // redeem's shared bucket, so failed guesses run out for both.
    let bad = r#"{"code":"K2P-11111-11111-11111-11111"}"#;
    let r = http(d.tunnel_port, "POST", "/cli/auth/device/pair/check", Some(&check_body), &refs(&with(EDGE, &["Origin: https://rosson.app.k2.dev"])));
    assert_code(&r, 403, "device_pairing_not_from_browser", "a browser Origin through the edge");
    for i in 0..crate_shared_limit() {
        let r = http(d.tunnel_port, "POST", "/cli/auth/device/pair/check", Some(bad), EDGE);
        assert_code(&r, 401, "pair_code_invalid", &format!("guess {i} through the edge"));
    }
    let r = http(d.tunnel_port, "POST", "/cli/auth/device/pair/check", Some(bad), EDGE);
    assert_code(&r, 429, "rate_limited", "the next check");
    let r = http(
        d.tunnel_port,
        "POST",
        "/cli/auth/device/pair",
        Some(r#"{"code":"K2P-11111-11111-11111-11111","device":{"id":"d","name":"D"}}"#),
        EDGE,
    );
    assert_code(&r, 429, "rate_limited", "a pair after the checks used the bucket");
}

fn crate_shared_limit() -> usize {
    k2_daemon::login_throttle::TUNNEL_SHARED_LIMIT
}

/// With a public host, a code comes with its link and QR.
#[tokio::test(flavor = "multi_thread")]
async fn a_code_comes_with_its_pair_link_and_qr() {
    let _g = lock();
    let _home = HomeEnv::set();
    k2_core::tunnel::config::save(&k2_core::tunnel::config::TunnelConfig {
        token: "tok".to_string(),
        subdomain: "rosson".to_string(),
        ..Default::default()
    })
    .expect("seed tunnel config");
    let d = test_harness::start(OWNER_TOKEN).await;
    let port = d.port;
    connect_users::add_user("linked", PW).expect("add");
    let r = http(port, "POST", &format!("/cli/auth/device/pair-code?token={OWNER_TOKEN}"), Some(r#"{"user":"linked"}"#), &[]);
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let code = v["code"].as_str().expect("code");
    assert_eq!(v["host"], "rosson.k2.dev", "{}", r.body);
    assert_eq!(v["pairUrl"], format!("https://rosson.k2.dev/pair#{code}"), "{}", r.body);
    let size = v["qr"]["size"].as_u64().expect("qr size") as usize;
    let rows = v["qr"]["rows"].as_array().expect("qr rows");
    assert_eq!(rows.len(), size);
    assert!(rows.iter().all(|row| row.as_str().is_some_and(|s| s.len() == size && s.bytes().all(|b| b == b'0' || b == b'1'))));
}

/// A password change ends paired phones (403 `device_session_ended`) and
/// voids an unredeemed code.
#[tokio::test(flavor = "multi_thread")]
async fn a_password_change_ends_paired_phones_and_voids_codes() {
    let _g = lock();
    let _home = HomeEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    let port = d.port;
    connect_users::add_user("rotating_user", PW).expect("add");
    set_web(port, false);
    let paired = pair_through_edge(port, &owner_code(port, "rotating_user"), "ios-1");
    let device = paired["token"].as_str().expect("token").to_string();
    let pending = owner_code(port, "rotating_user");
    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&bearer(&device)])));
    assert_eq!(r.status, 200, "{}", r.body);

    connect_users::set_password("rotating_user", "a-new-password-789").expect("set_password");

    let r = http(port, "GET", "/cli/auth/whoami", None, &refs(&with(EDGE, &[&bearer(&device)])));
    assert_code(&r, 403, "device_session_ended", "after a password change, through the edge");
    let r = http(port, "GET", &format!("/cli/auth/whoami?token={device}"), None, &[]);
    assert_code(&r, 403, "device_session_ended", "after a password change, direct");
    let body = format!(r#"{{"code":"{pending}","device":{{"id":"ios-2","name":"P"}}}}"#);
    let r = http(port, "POST", "/cli/auth/device/pair", Some(&body), EDGE);
    assert_code(&r, 401, "pair_code_invalid", "a code made before the password change");
}

// ── 0.46.6 pair success: the pair_code_changed frame and the status read ──

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn events_socket(port: u16, token: &str) -> Ws {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?token={token}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("events WS opens");
    assert!(next_kind(&mut ws, "hello", Duration::from_secs(5)).await.is_some(), "no hello");
    ws
}

async fn next_kind(ws: &mut Ws, kind: &str, wait: Duration) -> Option<Value> {
    use futures_util::StreamExt;
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        match tokio::time::timeout(left, ws.next()).await {
            Err(_) => return None,
            Ok(None) => panic!("events socket closed"),
            Ok(Some(Err(e))) => panic!("events socket error: {e}"),
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t)))) => {
                let v: Value = serde_json::from_str(&t).expect("frame JSON");
                if v["kind"] == kind {
                    return Some(v);
                }
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

/// The next `pair_code_changed` frame for `code_id` (frames for other
/// tests' codes in this process are skipped).
async fn next_code(ws: &mut Ws, code_id: &str, wait: Duration) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let f = next_kind(ws, "pair_code_changed", left).await?;
        if f["codeId"] == code_id {
            return Some(f);
        }
    }
}

/// `http` off the async worker (a blocking socket).
fn call(port: u16, method: &str, path_and_query: &str, body: Option<&str>, extra: &[&str]) -> Resp {
    tokio::task::block_in_place(|| http(port, method, path_and_query, body, extra))
}

fn status_of(port: u16, token: &str, code_id: &str) -> Resp {
    call(port, "GET", &format!("/cli/auth/device/pair-code/status?id={code_id}&token={token}"), None, &[])
}

/// The frame carries the code id, state, login, device name and session
/// id; never the code (in any spelling) or the device token.
fn assert_no_secrets(frame: &Value, code: &str, token: Option<&str>) {
    let text = frame.to_string();
    let body = code.trim_start_matches("K2P-").replace('-', "");
    assert!(!text.contains(code) && !text.contains(&body), "the code rode the frame: {text}");
    if let Some(t) = token {
        assert!(!text.contains(t), "the device token rode the frame: {text}");
    }
    let keys: std::collections::BTreeSet<&str> =
        frame.as_object().expect("frame object").keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["codeId", "deviceName", "kind", "sessionId", "state", "user"].into_iter().collect(),
        "exactly the documented fields: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_redeem_and_an_expiry_reach_only_the_minters_sockets_without_secrets() {
    let _g = lock();
    let _home = HomeEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    let port = d.port;
    connect_users::add_user("ps_phone", PW).expect("add ps_phone");
    // An Owner-role login the owner did NOT link: its own person.
    let other = add_login("ps_other", Role::Owner);
    let mut owner_ws = events_socket(port, OWNER_TOKEN).await;
    let mut other_ws = events_socket(port, &other).await;

    // Mint: the answer carries a code id that is not the code.
    let r = call(port, "POST", &format!("/cli/auth/device/pair-code?token={OWNER_TOKEN}"), Some(r#"{"user":"ps_phone"}"#), &[]);
    assert_eq!(r.status, 200, "{}", r.body);
    let code = r.json()["code"].as_str().expect("code").to_string();
    let code_id = r.json()["codeId"].as_str().expect("codeId").to_string();
    assert!(code_id.starts_with("pc_") && code_id.len() == 27, "{code_id}");

    // Status: pending for the minter; 404 for anyone else; GET only.
    let s = status_of(port, OWNER_TOKEN, &code_id);
    assert_eq!(s.status, 200, "{}", s.body);
    assert_eq!((s.json()["state"].clone(), s.json()["user"].clone()), (Value::from("pending"), Value::from("ps_phone")), "{}", s.body);
    assert!(!s.body.contains(&code), "the status never echoes the code: {}", s.body);
    assert_code(&status_of(port, &other, &code_id), 404, "no_such_code", "another login reads the owner's code");
    assert_code(&status_of(port, OWNER_TOKEN, "pc_000000000000000000000000"), 404, "no_such_code", "an unknown id");
    let r = call(port, "GET", &format!("/cli/auth/device/pair-code/status?id={code_id}"), None, &[]);
    assert_code(&r, 401, "login_required", "no token");
    let r = call(port, "GET", &format!("/cli/auth/device/pair-code/status?token={OWNER_TOKEN}"), None, &[]);
    assert_code(&r, 400, "usage", "no id");
    let r = call(port, "POST", &format!("/cli/auth/device/pair-code/status?id={code_id}&token={OWNER_TOKEN}"), Some("{}"), &[]);
    assert_eq!(r.status, 405, "POST is refused: {}", r.body);

    // The phone redeems: the answer adds serverName (never deviceName).
    let body = format!(r#"{{"code":"{code}","device":{{"id":"ios-ps","name":"iPhone"}}}}"#);
    let r = call(port, "POST", "/cli/auth/device/pair", Some(&body), EDGE);
    assert_eq!(r.status, 200, "{}", r.body);
    let paired = r.json();
    assert!(paired["serverName"].as_str().is_some_and(|n| !n.trim().is_empty()), "{}", r.body);
    assert_eq!(paired.get("deviceName"), None, "no top-level deviceName: {}", r.body);
    let token = paired["token"].as_str().expect("token").to_string();
    let session_id = paired["device"]["sessionId"].as_str().expect("sessionId").to_string();

    let f = next_code(&mut owner_ws, &code_id, Duration::from_secs(5)).await.expect("the minter hears the redeem");
    assert_eq!(
        f,
        serde_json::json!({
            "kind": "pair_code_changed",
            "codeId": code_id,
            "state": "paired",
            "user": "ps_phone",
            "deviceName": "iPhone",
            "sessionId": session_id,
        })
    );
    assert_no_secrets(&f, &code, Some(&token));
    assert!(
        next_code(&mut other_ws, &code_id, Duration::from_millis(1200)).await.is_none(),
        "another person's socket never hears it"
    );
    // The frame's sessionId is the new row in the phones list.
    let r = call(port, "GET", &format!("/cli/auth/devices?token={OWNER_TOKEN}"), None, &[]);
    assert!(r.json()["devices"].as_array().expect("devices").iter().any(|d| d["id"] == session_id.as_str()), "{}", r.body);
    let s = status_of(port, OWNER_TOKEN, &code_id);
    assert_eq!(
        (s.json()["state"].clone(), s.json()["deviceName"].clone(), s.json()["sessionId"].clone()),
        (Value::from("paired"), Value::from("iPhone"), Value::from(session_id.clone())),
        "{}",
        s.body
    );

    // Expiry: an unused code is reported expired once its time is up.
    let r = call(port, "POST", &format!("/cli/auth/device/pair-code?token={OWNER_TOKEN}"), Some(r#"{"user":"ps_phone"}"#), &[]);
    let late_code = r.json()["code"].as_str().expect("code").to_string();
    let late_id = r.json()["codeId"].as_str().expect("codeId").to_string();
    assert!(k2_core::connect_device_pairing::expire_now_for_test(&late_id), "pending");
    assert!(k2_core::connect_device_pairing::sweep_expired() >= 1, "the code ran out");
    let f = next_code(&mut owner_ws, &late_id, Duration::from_secs(5)).await.expect("the minter hears the expiry");
    assert_eq!(
        f,
        serde_json::json!({
            "kind": "pair_code_changed",
            "codeId": late_id,
            "state": "expired",
            "user": "ps_phone",
            "deviceName": null,
            "sessionId": null,
        })
    );
    assert_no_secrets(&f, &late_code, None);
    assert!(next_code(&mut other_ws, &late_id, Duration::from_millis(1200)).await.is_none());
    assert_eq!(status_of(port, OWNER_TOKEN, &late_id).json()["state"], "expired");

    // A login's own code reaches only that login's sockets.
    let r = call(port, "POST", "/cli/auth/device/pair-code", Some(r#"{"password":"password123"}"#), &[&bearer(&other)]);
    assert_eq!(r.status, 200, "{}", r.body);
    let own_code = r.json()["code"].as_str().expect("code").to_string();
    let own_id = r.json()["codeId"].as_str().expect("codeId").to_string();
    assert_code(&status_of(port, OWNER_TOKEN, &own_id), 404, "no_such_code", "the owner token did not mint it");
    assert_eq!(status_of(port, &other, &own_id).json()["state"], "pending");
    let body = format!(r#"{{"code":"{own_code}","device":{{"id":"and-ps","name":"Android phone"}}}}"#);
    let r = call(port, "POST", "/cli/auth/device/pair", Some(&body), EDGE);
    assert_eq!(r.status, 200, "{}", r.body);
    let f = next_code(&mut other_ws, &own_id, Duration::from_secs(5)).await.expect("the minting login hears it");
    assert_eq!(
        (f["codeId"].clone(), f["deviceName"].clone(), f["user"].clone()),
        (Value::from(own_id.clone()), Value::from("Android phone"), Value::from("ps_other"))
    );
    assert!(next_code(&mut owner_ws, &own_id, Duration::from_millis(1200)).await.is_none(), "the owner's sockets don't");
}
