//! The sign-in door: the path matcher (vs-live V2's table), origins, the
//! PKCE checks, the code store, and the forwarder with a stub door against
//! a recording fake App (the App never gets a byte of a door request).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use crate::client_ip::{self, DoorHook, Mode};

// ── the path ─────────────────────────────────────────────────────────

#[test]
fn exact_verbs_are_handled_and_every_other_spelling_is_the_doors_404() {
    for (t, want) in [
        ("/.k2/auth/ping", DoorVerb::Ping),
        ("/.k2/auth/sign-in", DoorVerb::SignIn),
        ("/.k2/auth/sign-in?x=1", DoorVerb::SignIn),
        ("/.k2/auth/set-password", DoorVerb::SetPassword),
        ("/.k2/auth/change-password", DoorVerb::ChangePassword),
    ] {
        assert_eq!(classify(t), Some(want), "{t}");
    }
    // V2: every one of these reached the App with an exact-prefix matcher.
    for t in [
        "/.k2/auth",
        "/.k2/auth/",
        "/.k2/auth/sign-in/",
        "/.k2/auth/sign-in;x",
        "/.k2/auth//sign-in",
        "/.k2/auth/../x",
        "/.k2/auth/%2e%2e/x",
        "/.K2/AUTH/sign-in",
        "/.k2/AUTH/sign-in",
        "//.k2/auth/sign-in",
        "/.k2//auth/sign-in",
        "/%2ek2/auth/sign-in",
        "/%252ek2/auth/sign-in",
        "/.k2%2fauth%2fsign-in",
        "/x/../.k2/auth/sign-in",
        "/./.k2/auth/sign-in",
        "/%2e%2e/.k2/auth/sign-in",
        "/.k2\\auth\\sign-in",
        "https://shop.example.com/.k2/auth/sign-in",
        "http://evil.example/.k2/auth/sign-in",
        "/.k2/auth%3fx",
        "/.k2/auth/unknown",
    ] {
        assert_eq!(classify(t), Some(DoorVerb::Other), "{t} is the door's (404), never the App's");
        assert!(claims(t), "{t}");
    }
    // The App's own paths, untouched.
    for t in ["/", "/.k2/other", "/.k2/authx", "/x/.k2/auth/sign-in", "/auth/sign-in", "/.well-known/x", "/api?next=/.k2/auth", "/k2/auth/sign-in"] {
        assert_eq!(classify(t), None, "{t} is the App's");
    }
}

#[test]
fn canonical_resolves_like_a_lenient_server() {
    assert_eq!(canonical("/A/./b/../C;p/%2E%2e/d"), "/a/d");
    assert_eq!(canonical("https://h.example"), "/");
    assert_eq!(canonical("/%5c.k2%5Cauth"), "/.k2/auth");
}

// ── origins and PKCE ─────────────────────────────────────────────────

#[test]
fn an_origin_is_absent_same_host_listed_or_foreign() {
    let listed = vec!["tauri://localhost".to_string()];
    assert_eq!(origin_is(None, Some("shop.example.com"), &listed), OriginIs::Absent, "a native app or curl");
    assert_eq!(origin_is(Some("https://shop.example.com"), Some("shop.example.com"), &listed), OriginIs::SameHost);
    assert_eq!(origin_is(Some("HTTPS://Shop.Example.com"), Some("shop.example.com"), &listed), OriginIs::SameHost);
    assert_eq!(origin_is(Some("http://localhost:3000"), Some("localhost:3000"), &listed), OriginIs::SameHost);
    assert_eq!(origin_is(Some("tauri://localhost"), Some("shop.example.com"), &listed), OriginIs::Listed("tauri://localhost".into()));
    assert_eq!(origin_is(Some("https://evil.example"), Some("shop.example.com"), &listed), OriginIs::Foreign);
    assert_eq!(origin_is(Some("null"), Some("shop.example.com"), &listed), OriginIs::Foreign, "null only when listed");
    assert_eq!(origin_is(Some("null"), Some("h"), &["null".to_string()]), OriginIs::Listed("null".into()));
    assert_eq!(origin_is(Some("https://shop.example.com"), None, &[]), OriginIs::Foreign, "no Host to match");
}

const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

#[test]
fn s256_matches_rfc_7636_and_formats_are_checked() {
    assert_eq!(s256(VERIFIER), CHALLENGE, "RFC 7636 appendix B");
    assert!(valid_challenge(CHALLENGE));
    for bad in ["", "short", &CHALLENGE[..42], &format!("{CHALLENGE}x"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw+cM"] {
        assert!(!valid_challenge(bad), "{bad}");
    }
    assert!(valid_verifier(VERIFIER));
    assert!(!valid_verifier(&VERIFIER[..42]), "too short");
    assert!(!valid_verifier(&"a".repeat(129)), "too long");
    assert!(!valid_verifier(&format!("{}!", &VERIFIER[..43])), "bad character");
}

// ── the code store ───────────────────────────────────────────────────

fn entry(app: &str) -> Entry {
    Entry {
        app: app.into(),
        principal_id: "p1".into(),
        challenge: CHALLENGE.into(),
        hash: "h".into(),
        auth_at: 1,
        label: None,
        via: "sign-in",
        made: Instant::now(),
    }
}

#[test]
fn a_code_is_one_use_bound_to_its_app_and_its_verifier() {
    let code = mint_code(entry("svc-a1")).unwrap();
    assert!(code.starts_with(CODE_PREFIX) && code.len() == CODE_PREFIX.len() + 64, "{code}");
    // Another App: refused, and the code stays (no denial between Apps).
    assert!(take_code(&code, "svc-b1", VERIFIER).is_none());
    // The right App and verifier: once.
    let e = take_code(&code, "svc-a1", VERIFIER).expect("first use");
    assert_eq!(e.principal_id, "p1");
    assert!(take_code(&code, "svc-a1", VERIFIER).is_none(), "used");

    // A wrong verifier uses the code up.
    let code = mint_code(entry("svc-a2")).unwrap();
    assert!(take_code(&code, "svc-a2", &"x".repeat(43)).is_none());
    assert!(take_code(&code, "svc-a2", VERIFIER).is_none(), "gone after a wrong verifier");
}

#[test]
fn a_code_expires_after_its_ttl() {
    let code = mint_code(entry("svc-ttl")).unwrap();
    age_codes_for_tests(CODE_TTL + Duration::from_secs(1));
    assert!(take_code(&code, "svc-ttl", VERIFIER).is_none(), "expired");
}

// ── the answer, without a database ───────────────────────────────────

#[test]
fn answers_carry_no_store_close_the_connection_and_the_frame_headers() {
    let o = refuse_after(429, "rate_limited", "wait", 300);
    let bytes = write(&o, Some("tauri://localhost"), false);
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with("HTTP/1.1 429 Too Many Requests\r\n"), "{text}");
    for want in [
        "Cache-Control: no-store\r\n",
        "Retry-After: 300\r\n",
        "Access-Control-Allow-Origin: tauri://localhost\r\nVary: Origin\r\n",
        "Connection: close\r\n",
        k2_core::frame_policy::CONNECT_HEADER_LINES,
    ] {
        assert!(text.contains(want), "missing {want:?} in {text}");
    }
    assert!(!text.to_ascii_lowercase().contains("set-cookie"), "the door never sets a cookie");
    let v: serde_json::Value = serde_json::from_str(text.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(v["error"]["code"], "rate_limited");
    assert_eq!(v["error"]["retryAfterSecs"], 300);
    // HEAD: the headers only.
    let head = String::from_utf8(write(&Out::ok(json!({"k2": true})), None, true)).unwrap();
    assert!(head.ends_with("\r\n\r\n"), "{head}");
}

// ── the forwarder: the App never sees a door request ─────────────────

/// A fake App that records every byte it receives and answers each
/// request head with `200 app` (keep-alive).
async fn recording_app() -> (u16, Arc<Mutex<Vec<u8>>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Arc<Mutex<Vec<u8>>> = Arc::default();
    let seen2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            let seen = seen2.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let mut pending: Vec<u8> = Vec::new();
                loop {
                    let n = match s.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    seen.lock().unwrap().extend_from_slice(&buf[..n]);
                    pending.extend_from_slice(&buf[..n]);
                    while let Some(i) = pending.windows(4).position(|w| w == b"\r\n\r\n") {
                        pending.drain(..i + 4);
                        if s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\napp").await.is_err() {
                            return;
                        }
                    }
                }
            });
        }
    });
    (port, seen)
}

/// A stub door: answers `door:<verb>:<body len>:<body kind>`.
fn stub_hook() -> DoorHook {
    DoorHook {
        claims,
        answer: Arc::new(|call: client_ip::DoorCall| {
            Box::pin(async move {
                let kind = match &call.body {
                    client_ip::DoorBody::Bytes(b) => format!("bytes{}", b.len()),
                    other => format!("{other:?}"),
                };
                let body = format!("door:{}:{kind}", classify(&call.path).map(|v| v.name()).unwrap_or("-"));
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
            })
        }),
        max_body: MAX_BODY,
        body_deadline: Duration::from_millis(400),
    }
}

/// One proxied connection: client ↔ forward_doored ↔ the fake App.
async fn proxied(app_port: u16) -> TcpStream {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let (c, _) = l.accept().await.unwrap();
        let up = TcpStream::connect(("127.0.0.1", app_port)).await.unwrap();
        let _ = client_ip::forward_doored(c, up, Mode::Tunnel(None), None, None, Some(stub_hook())).await;
    });
    TcpStream::connect(addr).await.unwrap()
}

async fn read_all(s: &mut TcpStream) -> String {
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

const SIGN_IN: &str = "POST /.k2/auth/sign-in HTTP/1.1\r\nHost: shop.example.com\r\nContent-Type: application/json\r\nContent-Length: 27\r\n\r\n{\"password\":\"hunter2-pass\"}";

#[tokio::test]
async fn a_door_request_mid_keep_alive_never_reaches_the_app() {
    let (port, seen) = recording_app().await;
    let mut c = proxied(port).await;
    c.write_all(b"GET /first HTTP/1.1\r\nHost: shop.example.com\r\n\r\n").await.unwrap();
    // Head and body split over three writes.
    let (a, b) = SIGN_IN.split_at(40);
    let (b, d) = b.split_at(b.len() - 10);
    c.write_all(a.as_bytes()).await.unwrap();
    c.write_all(b.as_bytes()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    c.write_all(d.as_bytes()).await.unwrap();
    let got = read_all(&mut c).await;
    assert!(got.contains("app") && got.contains("door:sign-in:bytes27"), "{got}");
    assert!(got.find("app").unwrap() < got.find("door:").unwrap(), "in order: {got}");
    let seen = String::from_utf8_lossy(&seen.lock().unwrap()).into_owned();
    assert!(seen.contains("GET /first"), "{seen}");
    assert!(!seen.contains("hunter2") && !seen.contains(".k2/auth"), "the App saw: {seen}");
}

#[tokio::test]
async fn pipelined_requests_after_a_door_request_never_move() {
    let (port, seen) = recording_app().await;
    let mut c = proxied(port).await;
    let all = format!("GET /a HTTP/1.1\r\nHost: h\r\n\r\n{SIGN_IN}GET /smuggled HTTP/1.1\r\nHost: h\r\n\r\n");
    c.write_all(all.as_bytes()).await.unwrap();
    let got = read_all(&mut c).await;
    assert!(got.contains("door:sign-in"), "{got}");
    let seen = String::from_utf8_lossy(&seen.lock().unwrap()).into_owned();
    assert!(seen.contains("GET /a ") && !seen.contains("/smuggled") && !seen.contains("hunter2"), "the App saw: {seen}");
}

#[tokio::test]
async fn chunked_oversize_upgrade_and_a_stalled_body_are_refused_by_the_door() {
    let (port, seen) = recording_app().await;
    for (req, want) in [
        ("POST /.k2/auth/sign-in HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n".to_string(), "LengthRequired"),
        ("POST /.k2/auth/sign-in HTTP/1.1\r\nHost: h\r\nContent-Length: 999999\r\n\r\n".to_string(), "TooLarge"),
        ("GET /.k2/auth/ping HTTP/1.1\r\nHost: h\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n".to_string(), "Upgrade"),
        ("POST /.k2/auth/sign-in HTTP/1.1\r\nHost: h\r\nContent-Length: 50\r\n\r\n{\"password\":".to_string(), "Timeout"),
    ] {
        let mut c = proxied(port).await;
        c.write_all(req.as_bytes()).await.unwrap();
        let got = read_all(&mut c).await;
        assert!(got.contains(&format!("door:{}", if req.contains("ping") { "ping" } else { "sign-in" })) && got.contains(want), "{want}: {got}");
    }
    // Framing the forwarder refuses never reaches the door or the App.
    let mut c = proxied(port).await;
    c.write_all(b"POST /.k2/auth/sign-in HTTP/1.1\r\nHost: h\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\npass").await.unwrap();
    let got = read_all(&mut c).await;
    assert!(got.starts_with("HTTP/1.1 400"), "{got}");
    assert!(seen.lock().unwrap().is_empty(), "the App saw nothing at all");
}

#[tokio::test]
async fn the_apps_own_traffic_passes_unchanged() {
    let (port, seen) = recording_app().await;
    let mut c = proxied(port).await;
    for path in ["/.k2/other", "/.k2/authx", "/x/.k2/auth/sign-in"] {
        c.write_all(format!("GET {path} HTTP/1.1\r\nHost: h\r\n\r\n").as_bytes()).await.unwrap();
    }
    c.write_all(b"GET /last HTTP/1.1\r\nHost: h\r\nConnection: close\r\n\r\n").await.unwrap();
    let got = read_all(&mut c).await;
    assert_eq!(got.matches("app").count(), 4, "{got}");
    let seen = String::from_utf8_lossy(&seen.lock().unwrap()).into_owned();
    for path in ["/.k2/other", "/.k2/authx", "/x/.k2/auth/sign-in", "/last"] {
        assert!(seen.contains(&format!("GET {path} HTTP/1.1")), "{path}: {seen}");
    }
}

#[tokio::test]
async fn a_first_request_on_the_door_is_answered_before_anything_is_dialed() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut c, _) = l.accept().await.unwrap();
        let p = client_ip::peek_head(&mut c).await;
        let target = client_ip::first_target(&p.bytes).expect("a request line");
        assert!(claims(&target), "{target}");
        client_ip::serve_door_first(&mut c, p.bytes, Mode::Tunnel(None), &stub_hook()).await;
    });
    let mut c = TcpStream::connect(addr).await.unwrap();
    c.write_all(SIGN_IN.as_bytes()).await.unwrap();
    let got = read_all(&mut c).await;
    assert!(got.contains("door:sign-in:bytes27"), "{got}");
}
