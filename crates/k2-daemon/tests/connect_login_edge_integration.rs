//! PRD `prd-connect-login-edge-only-v1.md` §12 — daemon-side integration
//! tests for the tunnel ingress gate, edge attestation, audit log,
//! `set-password` R1, `users/audit` L3 and the 7-day session TTL (T1).
//!
//! Drives REAL HTTP requests through the REAL dispatcher via
//! `k2_daemon::test_harness::start`, which binds BOTH a main loopback
//! listener (`Ingress::Loopback`) and a per-harness tunnel-ingress
//! listener (`Ingress::Tunnel`, `TestDaemon.tunnel_port`) — exactly what
//! the E2E TLS splice / cleartext frpc feed in production.
//!
//! ISOLATION: same as `auth_routes_integration.rs` — every test serializes
//! on `TEST_LOCK` and points `$HOME` at a fresh tempdir (connect-users
//! store, tunnel.json, edge-keys.json, auth-audit.jsonl all live there).

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex as StdMutex;

use k2_core::connect_users::{self, Role};
use k2_core::edge_attest::{random_nonce, EdgeSigner};
use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-edge-login-1";
const SUB: &str = "rosson";
const KID: &str = "k2-edge-test";
const LOGIN: &str = "/cli/auth/login";

/// Parsed HTTP response: status, body, raw header block.
struct Resp {
    status: u16,
    body: String,
    headers: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<String> {
        self.headers.lines().find_map(|l| {
            let (n, v) = l.split_once(':')?;
            if n.trim().eq_ignore_ascii_case(name) {
                Some(v.trim().to_string())
            } else {
                None
            }
        })
    }
}

fn http(port: u16, method: &str, path: &str, body: Option<&str>, extra: &[&str]) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("set read timeout");
    let mut extra_lines = String::new();
    for h in extra {
        extra_lines.push_str(h);
        extra_lines.push_str("\r\n");
    }
    let has_host = extra.iter().any(|h| h.to_ascii_lowercase().starts_with("host:"));
    let host_line = if has_host { "" } else { "Host: 127.0.0.1\r\n" };
    let req = match body {
        Some(b) => format!(
            "{method} {path} HTTP/1.1\r\n{host_line}Content-Type: application/json\r\n{extra_lines}Content-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path} HTTP/1.1\r\n{host_line}{extra_lines}\r\n"),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");

    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some((r, complete)) = try_parse(&raw) {
            if complete {
                return r;
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
    match try_parse(&raw) {
        Some((r, _)) => r,
        None => panic!("no parseable response: {:?}", String::from_utf8_lossy(&raw)),
    }
}

fn try_parse(raw: &[u8]) -> Option<(Resp, bool)> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())?;
    let content_len = headers.lines().find_map(|l| {
        let lower = l.to_ascii_lowercase();
        lower
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let complete = match content_len {
        Some(clen) => body.len() >= clen,
        None => true,
    };
    Some((
        Resp {
            status,
            body: body.to_string(),
            headers: headers.to_string(),
        },
        complete,
    ))
}

fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("k2-edge-login-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(tmp.join(".k2")).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();
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

/// Seed: tunnel subdomain `rosson` (A4), an ephemeral trusted edge key
/// (A1 override file), and a Member user. Returns the signer.
fn seed(username: &str, password: &str, role: Role) -> EdgeSigner {
    k2_core::tunnel::config::save(&k2_core::tunnel::config::TunnelConfig {
        token: "tok".to_string(),
        subdomain: SUB.to_string(),
        ..Default::default()
    })
    .expect("seed tunnel config");
    let signer = EdgeSigner::generate(KID).expect("ephemeral edge key");
    signer.install_trust_file().expect("install edge-keys.json");
    connect_users::add_user(username, password).expect("add_user");
    connect_users::set_role(username, role).expect("set_role");
    signer
}

fn now() -> i64 {
    k2_core::edge_attest::now_unix()
}

fn audit_events() -> Vec<serde_json::Value> {
    k2_core::auth_audit::tail(1000).expect("read audit log")
}

fn audit_events_for(event: &str) -> Vec<serde_json::Value> {
    audit_events()
        .into_iter()
        .filter(|e| e["event"] == event)
        .collect()
}

fn set_ingress_mode(mode: &str) {
    k2_core::app_settings::update(serde_json::json!({ "connectLoginIngress": mode }))
        .expect("set connectLoginIngress");
}

fn attested_login(
    d: &test_harness::TestDaemon,
    signer: &EdgeSigner,
    body: &str,
    ts: i64,
    nonce: &str,
    ip: &str,
    sub: &str,
    extra: &[&str],
) -> Resp {
    let header = format!(
        "X-K2-Edge-Sig: {}",
        signer.header("POST", LOGIN, sub, ts, nonce, ip, body.as_bytes())
    );
    let mut headers: Vec<&str> = vec![&header];
    headers.extend_from_slice(extra);
    http(d.tunnel_port, "POST", LOGIN, Some(body), &headers)
}

// ─────────────────────────────────────────────────────────────────────
// G1 — no HTML on the tunnel; loopback unchanged.
// ─────────────────────────────────────────────────────────────────────

fn assert_web_client_redirect(r: &Resp, what: &str) {
    assert_eq!(r.status, 302, "{what} must 302; headers={} body={}", r.headers, r.body);
    assert!(
        r.headers.starts_with("HTTP/1.1 302 Found"),
        "{what} status line: {}",
        r.headers
    );
    assert_eq!(
        r.header("Location").as_deref(),
        Some("https://rosson.app.k2.dev/"),
        "{what} Location comes from the configured tunnel subdomain"
    );
    assert_eq!(r.header("Content-Length").as_deref(), Some("0"), "{what}");
    assert_eq!(r.header("Cache-Control").as_deref(), Some("no-store"), "{what}");
    assert_eq!(r.header("Connection").as_deref(), Some("close"), "{what}");
    assert_eq!(r.body, "", "{what} has no body");
}

fn assert_plain_404(r: &Resp, what: &str) {
    assert_eq!(r.status, 404, "{what} must 404; headers={} body={}", r.headers, r.body);
    assert_eq!(r.body, "Not Found", "{what}");
    assert_eq!(r.header("Content-Type").as_deref(), Some("text/plain"), "{what}");
    assert_eq!(r.header("Connection").as_deref(), Some("close"), "{what}");
    assert!(r.header("Location").is_none(), "{what} must not redirect");
}

fn assert_account_html(r: &Resp, what: &str) {
    assert_eq!(r.status, 200, "{what} must serve the page; body={}", r.body);
    assert_eq!(
        r.header("Content-Type").as_deref(),
        Some("text/html; charset=utf-8"),
        "{what}"
    );
    assert!(
        r.body.contains("<html") || r.body.contains("<!DOCTYPE"),
        "{what} is HTML"
    );
}

/// Seed the tunnel label directly (for the empty / invalid label cases).
fn set_tunnel_subdomain(sub: &str) {
    k2_core::tunnel::config::save(&k2_core::tunnel::config::TunnelConfig {
        token: "tok".to_string(),
        subdomain: sub.to_string(),
        ..Default::default()
    })
    .expect("save tunnel config");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tunnel_root_and_account_redirect_to_web_client_while_loopback_serves_html() {
    let _g = lock();
    with_temp_home(|| {
        let _s = seed("g1user", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        for path in ["/", "/account"] {
            let r = http(d.tunnel_port, "GET", path, None, &[]);
            assert_web_client_redirect(&r, &format!("tunnel GET {path}"));
            let r = http(d.tunnel_port, "HEAD", path, None, &[]);
            assert_web_client_redirect(&r, &format!("tunnel HEAD {path}"));

            // No open redirect: a spoofed Host never shapes Location.
            let r = http(d.tunnel_port, "GET", path, None, &["Host: evil.example"]);
            assert_web_client_redirect(&r, &format!("tunnel GET {path} Host: evil.example"));
            let r = http(d.tunnel_port, "HEAD", path, None, &["Host: evil.app.k2.dev"]);
            assert_web_client_redirect(&r, &format!("tunnel HEAD {path} Host: evil.app.k2.dev"));
            let r = http(
                d.tunnel_port,
                "GET",
                &format!("{path}?next=https://evil.example/"),
                None,
                &["X-Forwarded-Host: evil.example"],
            );
            assert_web_client_redirect(&r, &format!("tunnel GET {path}?next=… X-Forwarded-Host"));

            // G5: loopback unchanged.
            let r = http(d.port, "GET", path, None, &[]);
            assert_account_html(&r, &format!("loopback GET {path}"));
        }

        // Other paths on the tunnel are untouched: byte-identical to loopback.
        let t = http(d.tunnel_port, "GET", "/some/other/path", None, &[]);
        let l = http(d.port, "GET", "/some/other/path", None, &[]);
        assert_ne!(t.status, 302, "other paths never redirect");
        assert!(t.header("Location").is_none(), "other paths carry no Location");
        assert_eq!((t.status, &t.body), (l.status, &l.body), "tunnel /some/other/path == loopback");

        // Methods other than GET/HEAD on those paths: same as before the
        // redirect (and as loopback) — POST / is 405, OPTIONS / is 204.
        for path in ["/", "/account"] {
            let t = http(d.tunnel_port, "POST", path, Some("{}"), &[]);
            let l = http(d.port, "POST", path, Some("{}"), &[]);
            assert_eq!(t.status, 405, "tunnel POST {path}; body={}", t.body);
            assert_eq!((t.status, &t.body), (l.status, &l.body), "tunnel POST {path} == loopback");
            let t = http(d.tunnel_port, "OPTIONS", path, None, &[]);
            let l = http(d.port, "OPTIONS", path, None, &[]);
            assert_eq!(t.status, 204, "tunnel OPTIONS {path}");
            assert_eq!((t.status, &t.body), (l.status, &l.body), "tunnel OPTIONS {path} == loopback");
            assert!(t.header("Location").is_none());
        }

        // Login gate unchanged: unattested tunnel login is still a bare 404.
        let r = http(
            d.tunnel_port,
            "POST",
            LOGIN,
            Some(r#"{"username":"g1user","password":"password123"}"#),
            &[],
        );
        assert_eq!(r.status, 404, "unattested tunnel login; body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("login 404 is JSON");
        assert_eq!(v["error"], "not_found");
        assert!(r.header("Location").is_none(), "login is never redirected");

        // Page hits are not audit events (the login above is).
        assert!(
            audit_events().iter().all(|e| e["event"] == "login"),
            "page hits are not audit events: {:?}",
            audit_events()
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tunnel_root_without_usable_subdomain_stays_plain_404() {
    let _g = lock();
    with_temp_home(|| {
        let _s = seed("g1bad", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        for sub in ["", "   ", "app", "www", "evil.example", "bad_label"] {
            set_tunnel_subdomain(sub);
            for path in ["/", "/account"] {
                let r = http(d.tunnel_port, "GET", path, None, &["Host: evil.example"]);
                assert_plain_404(&r, &format!("tunnel GET {path} with subdomain {sub:?}"));
                // HEAD keeps its pre-redirect answer (the 405 allowlist).
                let r = http(d.tunnel_port, "HEAD", path, None, &[]);
                assert_eq!(r.status, 405, "tunnel HEAD {path} with subdomain {sub:?}");
                assert!(r.header("Location").is_none());
            }
        }

        // Config missing (empty default label) or malformed (load error) → 404.
        let home = std::env::var("HOME").expect("HOME");
        let cfg = k2_core::tunnel::config::config_path();
        assert!(cfg.starts_with(&home), "tunnel config must live under the temp HOME: {cfg:?}");
        std::fs::remove_file(&cfg).expect("remove tunnel config");
        let r = http(d.tunnel_port, "GET", "/", None, &[]);
        assert_plain_404(&r, "tunnel GET / with no tunnel config");
        std::fs::write(&cfg, "{ not json").expect("write malformed tunnel config");
        assert!(k2_core::tunnel::config::load().is_err(), "malformed config must fail to load");
        let r = http(d.tunnel_port, "GET", "/account", None, &[]);
        assert_plain_404(&r, "tunnel GET /account with malformed tunnel config");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tunnel_root_mode_any_serves_page_and_off_is_plain_404() {
    let _g = lock();
    with_temp_home(|| {
        let _s = seed("g1mode", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        set_ingress_mode("any");
        for path in ["/", "/account"] {
            let r = http(d.tunnel_port, "GET", path, None, &[]);
            assert_account_html(&r, &format!("mode=any tunnel GET {path}"));
            assert!(r.header("Location").is_none());
            let t = http(d.tunnel_port, "HEAD", path, None, &[]);
            let l = http(d.port, "HEAD", path, None, &[]);
            assert_eq!(t.status, 405, "mode=any tunnel HEAD {path} behaves like loopback");
            assert_eq!((t.status, &t.body), (l.status, &l.body));
        }

        set_ingress_mode("off");
        for path in ["/", "/account"] {
            let r = http(d.tunnel_port, "GET", path, None, &[]);
            assert_plain_404(&r, &format!("mode=off tunnel GET {path}"));
            let r = http(d.tunnel_port, "HEAD", path, None, &[]);
            assert_eq!(r.status, 405, "mode=off tunnel HEAD {path} unchanged");
            assert!(r.header("Location").is_none());
            // Loopback still serves the page regardless of the tunnel mode.
            let r = http(d.port, "GET", path, None, &[]);
            assert_account_html(&r, &format!("mode=off loopback GET {path}"));
        }
    });
}

// ─────────────────────────────────────────────────────────────────────
// G2 (edge, default) — unattested tunnel login is a dead end.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tunnel_login_without_header_404_no_lockout_no_bad_creds_audit() {
    let _g = lock();
    with_temp_home(|| {
        let _s = seed("scanned", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        assert_eq!(
            k2_core::app_settings::load().connect_login_ingress(),
            k2_core::app_settings::ConnectLoginIngress::Edge,
            "default mode is edge"
        );

        // Wrong password, five times — would lock the account on loopback.
        for _ in 0..5 {
            let r = http(
                d.tunnel_port,
                "POST",
                LOGIN,
                Some(r#"{"username":"scanned","password":"WRONG"}"#),
                &[],
            );
            assert_eq!(r.status, 404, "unattested tunnel login must 404; body={}", r.body);
            assert_eq!(r.body, r#"{"error":"not_found"}"#);
        }
        // Correct password, still 404 — no attestation, no oracle.
        let r = http(
            d.tunnel_port,
            "POST",
            LOGIN,
            Some(r#"{"username":"scanned","password":"password123"}"#),
            &[],
        );
        assert_eq!(r.status, 404);

        assert_eq!(connect_users::lockout_failed_count("scanned"), 0, "lockout counter untouched");
        assert!(!connect_users::is_locked("scanned"));

        let logins = audit_events_for("login");
        assert_eq!(logins.len(), 6, "every blocked attempt is audited; got {logins:?}");
        for e in &logins {
            assert_eq!(e["outcome"], "blocked_ingress", "{e}");
            assert_eq!(e["ingress"], "tunnel", "{e}");
            assert_eq!(e["user"], "scanned", "{e}");
            assert_eq!(e["ip"], "-", "{e}");
            assert_eq!(e["client"], "api", "{e}");
        }
        assert!(
            !logins.iter().any(|e| e["outcome"] == "bad_creds"),
            "no bad_creds record: argon2 never ran"
        );

        // A local login still works afterwards (no lockout was tripped).
        let r = http(
            d.port,
            "POST",
            LOGIN,
            Some(r#"{"username":"scanned","password":"password123"}"#),
            &[],
        );
        assert_eq!(r.status, 200, "loopback login unchanged; body={}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tunnel_login_attested_bad_creds_401_good_creds_200_with_edge_audit() {
    let _g = lock();
    with_temp_home(|| {
        let signer = seed("edgeuser", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Valid signature, wrong password → the ordinary generic 401.
        let body = r#"{"username":"edgeuser","password":"WRONG"}"#;
        let r = attested_login(&d, &signer, body, now(), &random_nonce(), "203.0.113.9", SUB, &[]);
        assert_eq!(r.status, 401, "attested bad creds → 401; body={}", r.body);
        assert!(r.body.contains("invalid username or password"));
        assert_eq!(connect_users::lockout_failed_count("edgeuser"), 1, "lockout counts attested fails");

        // Valid signature, good password (+ web mode) → 200 + cookie.
        let body = r#"{"username":"edgeuser","password":"password123"}"#;
        let r = attested_login(
            &d,
            &signer,
            body,
            now(),
            &random_nonce(),
            "203.0.113.9",
            SUB,
            &["X-K2-Client: web", "X-Forwarded-Proto: https"],
        );
        assert_eq!(r.status, 200, "attested good creds → 200; body={}", r.body);
        let json: serde_json::Value = serde_json::from_str(&r.body).expect("login JSON");
        assert!(json["token"].as_str().is_some_and(|t| !t.is_empty()));
        assert_eq!(json["mustChangePassword"], false);
        let cookie = r.header("Set-Cookie").expect("web login sets the session cookie");
        assert!(cookie.starts_with("k2_session="), "{cookie}");
        assert!(cookie.contains("Secure"), "https via edge → Secure; {cookie}");
        assert_eq!(connect_users::lockout_failed_count("edgeuser"), 0, "success clears the counter");

        let logins = audit_events_for("login");
        assert_eq!(logins.len(), 2, "{logins:?}");
        assert_eq!(logins[0]["outcome"], "bad_creds");
        assert_eq!(logins[0]["ingress"], format!("edge:{KID}"));
        assert_eq!(logins[0]["ip"], "203.0.113.9");
        assert_eq!(logins[0]["client"], "api");
        assert_eq!(logins[1]["outcome"], "ok");
        assert_eq!(logins[1]["ingress"], format!("edge:{KID}"));
        assert_eq!(logins[1]["ip"], "203.0.113.9");
        assert_eq!(logins[1]["client"], "web");
        assert_eq!(logins[1]["user"], "edgeuser");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tunnel_login_attestation_failures_404_and_audit_reason() {
    let _g = lock();
    with_temp_home(|| {
        let signer = seed("attuser", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let body = r#"{"username":"attuser","password":"password123"}"#;

        // Stale ts.
        let r = attested_login(&d, &signer, body, now() - 121, &random_nonce(), "-", SUB, &[]);
        assert_eq!(r.status, 404, "stale ts → 404; body={}", r.body);
        assert_eq!(r.body, r#"{"error":"not_found"}"#);

        // Reused nonce: first attempt succeeds, the replay is refused.
        let nonce = random_nonce();
        let r = attested_login(&d, &signer, body, now(), &nonce, "-", SUB, &[]);
        assert_eq!(r.status, 200, "first use of the nonce → 200; body={}", r.body);
        let r = attested_login(&d, &signer, body, now(), &nonce, "-", SUB, &[]);
        assert_eq!(r.status, 404, "replayed nonce → 404; body={}", r.body);

        // Wrong sub: signed for another customer's host (Host hint names it).
        let r = attested_login(
            &d,
            &signer,
            body,
            now(),
            &random_nonce(),
            "-",
            "julie",
            &["Host: julie.k2.dev"],
        );
        assert_eq!(r.status, 404, "wrong sub → 404; body={}", r.body);

        // Unknown kid.
        let stranger = EdgeSigner::generate("k2-edge-stranger").expect("key");
        let r = attested_login(&d, &stranger, body, now(), &random_nonce(), "-", SUB, &[]);
        assert_eq!(r.status, 404, "unknown kid → 404; body={}", r.body);

        // Garbage header.
        let r = http(
            d.tunnel_port,
            "POST",
            LOGIN,
            Some(body),
            &["X-K2-Edge-Sig: v1;kid=k2-edge-test;ts=abc"],
        );
        assert_eq!(r.status, 404, "malformed header → 404; body={}", r.body);

        // Tampered body: sign one body, send another.
        let header = format!(
            "X-K2-Edge-Sig: {}",
            signer.header("POST", LOGIN, SUB, now(), &random_nonce(), "-", br#"{"username":"attuser","password":"x"}"#)
        );
        let r = http(d.tunnel_port, "POST", LOGIN, Some(body), &[&header]);
        assert_eq!(r.status, 404, "body hash mismatch → 404; body={}", r.body);

        assert_eq!(connect_users::lockout_failed_count("attuser"), 0, "no failure touched the counter");

        let outcomes: Vec<String> = audit_events_for("login")
            .iter()
            .map(|e| e["outcome"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            outcomes,
            vec![
                "bad_attest:stale",
                "ok",
                "bad_attest:replay",
                "bad_attest:wrong_sub",
                "bad_attest:unknown_kid",
                "bad_attest:malformed",
                "bad_attest:bad_sig",
            ],
            "audit reasons in order"
        );
        for e in audit_events_for("login") {
            if e["outcome"] != "ok" {
                assert_eq!(e["ingress"], "tunnel", "failed attestations are tagged by listener: {e}");
                assert_eq!(e["user"], "attuser", "{e}");
            }
        }
    });
}

// ─────────────────────────────────────────────────────────────────────
// A7 — the header is inert off the tunnel; G3/G4 — other routes unchanged.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_login_ignores_garbage_and_valid_edge_headers() {
    let _g = lock();
    with_temp_home(|| {
        let signer = seed("localuser", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let body = r#"{"username":"localuser","password":"password123"}"#;

        let r = http(d.port, "POST", LOGIN, Some(body), &["X-K2-Edge-Sig: total garbage"]);
        assert_eq!(r.status, 200, "garbage header on loopback is ignored; body={}", r.body);

        let bad = r#"{"username":"localuser","password":"WRONG"}"#;
        let r = http(d.port, "POST", LOGIN, Some(bad), &["X-K2-Edge-Sig: total garbage"]);
        assert_eq!(r.status, 401, "401 as before; body={}", r.body);

        // A VALID edge header on loopback grants nothing extra and is not
        // recorded as an edge login.
        let header = format!(
            "X-K2-Edge-Sig: {}",
            signer.header("POST", LOGIN, SUB, now(), &random_nonce(), "203.0.113.9", body.as_bytes())
        );
        let r = http(d.port, "POST", LOGIN, Some(body), &[&header]);
        assert_eq!(r.status, 200);
        let logins = audit_events_for("login");
        assert_eq!(logins.len(), 3);
        for e in &logins {
            assert_eq!(e["ingress"], "loopback", "{e}");
            assert_eq!(e["ip"], "-", "{e}");
        }
        assert_eq!(logins[1]["outcome"], "bad_creds");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tunnel_token_bearing_and_session_routes_are_unchanged() {
    let _g = lock();
    with_temp_home(|| {
        let _s = seed("tunneluser", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // G4: owner token over the tunnel reaches the data plane.
        let r = http(d.tunnel_port, "GET", &format!("/cli/projects/list?token={OWNER_TOKEN}"), None, &[]);
        assert_eq!(r.status, 200, "token traffic unaffected on tunnel; body={}", r.body);

        // G3: a session (minted locally) may whoami / change-password /
        // logout over the tunnel.
        let session = connect_users::create_session("tunneluser");
        let r = http(d.tunnel_port, "GET", &format!("/cli/auth/whoami?token={session}"), None, &[]);
        assert_eq!(r.status, 200, "whoami over tunnel; body={}", r.body);

        let r = http(
            d.tunnel_port,
            "POST",
            &format!("/cli/auth/change-password?token={session}"),
            Some(r#"{"currentPassword":"password123","newPassword":"password456"}"#),
            &[],
        );
        assert_eq!(r.status, 200, "change-password over tunnel; body={}", r.body);
        let cp = audit_events_for("change-password");
        assert_eq!(cp.len(), 1, "{cp:?}");
        assert_eq!(cp[0]["user"], "tunneluser");
        assert_eq!(cp[0]["outcome"], "ok");
        assert_eq!(cp[0]["ingress"], "tunnel");

        // change-password revoked the session; a fresh one logs out.
        let session2 = connect_users::create_session("tunneluser");
        let r = http(d.tunnel_port, "POST", &format!("/cli/auth/logout?token={session2}"), Some("{}"), &[]);
        assert_eq!(r.status, 200, "logout over tunnel; body={}", r.body);
        let lo = audit_events_for("logout");
        assert_eq!(lo.len(), 1, "{lo:?}");
        assert_eq!(lo[0]["user"], "tunneluser");
        assert_eq!(lo[0]["ingress"], "tunnel");

        // An unknown-token logout is not attributed (no record).
        let r = http(d.tunnel_port, "POST", "/cli/auth/logout?token=nope", Some("{}"), &[]);
        assert_eq!(r.status, 200);
        assert_eq!(audit_events_for("logout").len(), 1);

        // /ping, /health, /boot-status keep answering on the tunnel.
        for p in ["/ping", "/health", "/boot-status"] {
            let r = http(d.tunnel_port, "GET", p, None, &[]);
            assert_eq!(r.status, 200, "{p} on tunnel");
        }
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tunnel_options_preflight_on_login_matches_loopback_and_is_not_audited() {
    let _g = lock();
    with_temp_home(|| {
        let _s = seed("optuser", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        let preflight_headers = [
            "Origin: https://rosson.app.k2.dev",
            "Access-Control-Request-Method: POST",
            "Access-Control-Request-Headers: content-type,x-k2-client",
        ];
        let via_tunnel = http(d.tunnel_port, "OPTIONS", LOGIN, None, &preflight_headers);
        let via_loopback = http(d.port, "OPTIONS", LOGIN, None, &preflight_headers);
        assert_eq!(via_tunnel.status, via_loopback.status, "same preflight status");
        assert_eq!(via_tunnel.status, 204);
        let norm = |h: &str| -> Vec<String> {
            let mut v: Vec<String> = h
                .lines()
                .skip(1)
                .map(|l| l.trim().to_ascii_lowercase())
                .collect();
            v.sort();
            v
        };
        assert_eq!(norm(&via_tunnel.headers), norm(&via_loopback.headers), "same preflight headers");
        assert!(
            via_tunnel.header("Access-Control-Allow-Origin").is_some(),
            "CORS headers present on the tunnel preflight: {}",
            via_tunnel.headers
        );
        // OPTIONS / is answered as before (not the G1 redirect). HEAD / on
        // the tunnel follows GET in edge mode (G1 302); loopback stays 405.
        let o = http(d.tunnel_port, "OPTIONS", "/", None, &[]);
        assert_eq!(o.status, 204);
        let h_t = http(d.tunnel_port, "HEAD", "/", None, &[]);
        let h_l = http(d.port, "HEAD", "/", None, &[]);
        assert_web_client_redirect(&h_t, "tunnel HEAD / (edge)");
        assert_eq!(h_l.status, 405, "loopback HEAD / unchanged");
        assert!(audit_events().is_empty(), "preflight / HEAD are not audit events");
    });
}

// ─────────────────────────────────────────────────────────────────────
// S1 — the setting: any / off; I6 — boot-status advertises it.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ingress_mode_any_allows_tunnel_login_and_off_refuses_everything() {
    let _g = lock();
    with_temp_home(|| {
        let signer = seed("modeuser", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let body = r#"{"username":"modeuser","password":"password123"}"#;

        // Default (edge) advertised on /boot-status.
        let r = http(d.tunnel_port, "GET", "/boot-status", None, &[]);
        let bs: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(bs["connectLogin"]["ingress"], "edge");

        // any → unattested tunnel login works (old behaviour), audited as tunnel.
        set_ingress_mode("any");
        let r = http(d.tunnel_port, "GET", "/boot-status", None, &[]);
        let bs: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(bs["connectLogin"]["ingress"], "any");
        let r = http(d.tunnel_port, "POST", LOGIN, Some(body), &[]);
        assert_eq!(r.status, 200, "mode=any → tunnel login allowed; body={}", r.body);
        let last = audit_events_for("login").pop().unwrap();
        assert_eq!(last["outcome"], "ok");
        assert_eq!(last["ingress"], "tunnel");
        let bad = r#"{"username":"modeuser","password":"WRONG"}"#;
        let r = http(d.tunnel_port, "POST", LOGIN, Some(bad), &[]);
        assert_eq!(r.status, 401, "mode=any → real 401; body={}", r.body);
        assert_eq!(connect_users::lockout_failed_count("modeuser"), 1);

        // off → even an attested login is 404; loopback still logs in.
        set_ingress_mode("off");
        let r = attested_login(&d, &signer, body, now(), &random_nonce(), "203.0.113.9", SUB, &[]);
        assert_eq!(r.status, 404, "mode=off → 404 even attested; body={}", r.body);
        let last = audit_events_for("login").pop().unwrap();
        assert_eq!(last["outcome"], "blocked_ingress");
        assert_eq!(last["ingress"], "tunnel");
        let r = http(d.port, "POST", LOGIN, Some(body), &[]);
        assert_eq!(r.status, 200, "mode=off → loopback login still works; body={}", r.body);
        let r = http(d.tunnel_port, "GET", &format!("/cli/projects/list?token={OWNER_TOKEN}"), None, &[]);
        assert_eq!(r.status, 200, "mode=off → tokens still work on the tunnel");

        // `k2 tunnel status` surface (S2).
        let r = http(d.port, "GET", &format!("/cli/tunnel/status?token={OWNER_TOKEN}"), None, &[]);
        assert_eq!(r.status, 200, "{}", r.body);
        let st: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(st["loginIngress"], "off");

        // The setting is owner-tier: a Member session may not flip it.
        let member = connect_users::create_session("modeuser");
        let r = http(
            d.port,
            "POST",
            &format!("/cli/settings/update?token={member}"),
            Some(r#"{"connectLoginIngress":"any"}"#),
            &[],
        );
        assert_eq!(r.status, 403, "Member cannot change connectLoginIngress; body={}", r.body);
        assert_eq!(
            k2_core::app_settings::load().connect_login_ingress(),
            k2_core::app_settings::ConnectLoginIngress::Off
        );
    });
}

// ─────────────────────────────────────────────────────────────────────
// R1 — set-password matrix.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_password_matrix_owner_token_owner_session_admin_member_self() {
    let _g = lock();
    with_temp_home(|| {
        let _s = seed("target", "password123", Role::Member);
        connect_users::add_user("boss", "password123").unwrap();
        connect_users::set_role("boss", Role::Owner).unwrap();
        connect_users::add_user("deputy", "password123").unwrap();
        connect_users::set_role("deputy", Role::Admin).unwrap();
        connect_users::add_user("peer", "password123").unwrap();
        connect_users::set_role("peer", Role::Member).unwrap();
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let path = "/cli/users/set-password";

        // Owner token: ok, default mustChangePassword=false (CLI behaviour).
        let target_sess = connect_users::create_session("target");
        let r = http(
            d.port,
            "POST",
            &format!("{path}?token={OWNER_TOKEN}"),
            Some(r#"{"username":"target","password":"newpass123"}"#),
            &[],
        );
        assert_eq!(r.status, 200, "owner token ok; body={}", r.body);
        let j: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(j["success"], true);
        assert_eq!(j["mustChangePassword"], false);
        assert!(!connect_users::must_change_password("target"));
        assert!(
            connect_users::validate_session(&target_sess).is_none(),
            "password reset revokes the target's sessions"
        );

        // Owner-ROLE session on a Member: ok, default mustChangePassword=true,
        // sessions revoked.
        let boss = connect_users::create_session("boss");
        let target_sess = connect_users::create_session("target");
        let r = http(
            d.port,
            "POST",
            &format!("{path}?token={boss}"),
            Some(r#"{"username":"target","password":"temppass123"}"#),
            &[],
        );
        assert_eq!(r.status, 200, "Owner session ok on Member; body={}", r.body);
        let j: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(j["mustChangePassword"], true);
        assert!(connect_users::must_change_password("target"), "flag set");
        assert!(connect_users::validate_session(&target_sess).is_none(), "sessions revoked");
        // The temp password logs in but the session is restricted.
        let r = http(
            d.port,
            "POST",
            LOGIN,
            Some(r#"{"username":"target","password":"temppass123"}"#),
            &[],
        );
        assert_eq!(r.status, 200, "{}", r.body);
        let j: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(j["mustChangePassword"], true);
        let restricted = j["token"].as_str().unwrap().to_string();
        let r = http(d.port, "GET", &format!("/cli/projects/list?token={restricted}"), None, &[]);
        assert_eq!(r.status, 403, "restricted until rotation; body={}", r.body);
        assert!(r.body.contains("password_change_required"));

        // Explicit mustChangePassword:false from a session actor is honoured.
        let r = http(
            d.port,
            "POST",
            &format!("{path}?token={boss}"),
            Some(r#"{"username":"target","password":"plainpass123","mustChangePassword":false}"#),
            &[],
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(!connect_users::must_change_password("target"), "explicit false clears the flag");

        // Admin session: 403 (password reset stays Owner-level).
        let deputy = connect_users::create_session("deputy");
        let r = http(
            d.port,
            "POST",
            &format!("{path}?token={deputy}"),
            Some(r#"{"username":"target","password":"adminpass123"}"#),
            &[],
        );
        assert_eq!(r.status, 403, "Admin session 403; body={}", r.body);

        // Member session: 403.
        let peer = connect_users::create_session("peer");
        let r = http(
            d.port,
            "POST",
            &format!("{path}?token={peer}"),
            Some(r#"{"username":"target","password":"peerpass123"}"#),
            &[],
        );
        assert_eq!(r.status, 403, "Member session 403; body={}", r.body);

        // Self-target from a session: 400.
        let boss = connect_users::create_session("boss");
        let r = http(
            d.port,
            "POST",
            &format!("{path}?token={boss}"),
            Some(r#"{"username":"boss","password":"selfpass123"}"#),
            &[],
        );
        assert_eq!(r.status, 400, "self-target 400; body={}", r.body);
        assert!(r.body.contains("change-password"), "{}", r.body);
        assert!(connect_users::validate_session(&boss).is_some(), "self-target is a no-op");

        // Unknown target: 400.
        let r = http(
            d.port,
            "POST",
            &format!("{path}?token={boss}"),
            Some(r#"{"username":"ghost","password":"ghostpass123"}"#),
            &[],
        );
        assert_eq!(r.status, 400, "{}", r.body);

        // No token: 403. GET: 405.
        let r = http(d.port, "POST", path, Some(r#"{"username":"target","password":"x"}"#), &[]);
        assert_eq!(r.status, 403);
        let r = http(d.port, "GET", &format!("{path}?token={OWNER_TOKEN}"), None, &[]);
        assert_eq!(r.status, 405);

        // Every attempt that passed the credential gate is audited as a
        // set-password event on the target (gate-level 403s — Admin /
        // Member / no token — never reach the handler and are not
        // attributable, so they are not recorded): 3 ok + self-target 400
        // + unknown-target 400.
        let sp = audit_events_for("set-password");
        let outcomes: Vec<(String, String)> = sp
            .iter()
            .map(|e| (e["user"].as_str().unwrap().to_string(), e["outcome"].as_str().unwrap().to_string()))
            .collect();
        assert_eq!(
            outcomes,
            vec![
                ("target".to_string(), "ok".to_string()),
                ("target".to_string(), "ok".to_string()),
                ("target".to_string(), "ok".to_string()),
                ("boss".to_string(), "rejected:400".to_string()),
                ("ghost".to_string(), "rejected:400".to_string()),
            ],
            "{sp:?}"
        );
        for e in &sp {
            assert_eq!(e["ingress"], "loopback", "{e}");
        }
    });
}

// ─────────────────────────────────────────────────────────────────────
// L3 — GET /cli/users/audit.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn users_audit_is_get_only_owner_tier_and_tail_bounded() {
    let _g = lock();
    with_temp_home(|| {
        let _s = seed("audited", "password123", Role::Member);
        connect_users::add_user("boss", "password123").unwrap();
        connect_users::set_role("boss", Role::Owner).unwrap();
        connect_users::add_user("deputy", "password123").unwrap();
        connect_users::set_role("deputy", Role::Admin).unwrap();
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Generate 60 login records (3 bad → lockout, then 57 locked).
        for _ in 0..60 {
            let _ = http(
                d.port,
                "POST",
                LOGIN,
                Some(r#"{"username":"audited","password":"WRONG"}"#),
                &[],
            );
        }
        assert_eq!(audit_events_for("login").len(), 60);
        let locked = audit_events_for("login")
            .iter()
            .filter(|e| e["outcome"] == "locked")
            .count();
        assert_eq!(locked, 57, "3 bad_creds then locked");

        // Owner token, default tail 50.
        let r = http(d.port, "GET", &format!("/cli/users/audit?token={OWNER_TOKEN}"), None, &[]);
        assert_eq!(r.status, 200, "{}", r.body);
        let j: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(j["tail"], 50);
        assert_eq!(j["events"].as_array().unwrap().len(), 50);
        assert_eq!(j["events"][49]["outcome"], "locked");

        // Explicit tail + clamping.
        let r = http(d.port, "GET", &format!("/cli/users/audit?token={OWNER_TOKEN}&tail=5"), None, &[]);
        let j: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(j["events"].as_array().unwrap().len(), 5);
        let r = http(d.port, "GET", &format!("/cli/users/audit?token={OWNER_TOKEN}&tail=0"), None, &[]);
        let j: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(j["tail"], 1);
        let r = http(d.port, "GET", &format!("/cli/users/audit?token={OWNER_TOKEN}&tail=99999"), None, &[]);
        let j: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(j["tail"], 1000);
        assert_eq!(j["events"].as_array().unwrap().len(), 60);
        let r = http(d.port, "GET", &format!("/cli/users/audit?token={OWNER_TOKEN}&tail=abc"), None, &[]);
        assert_eq!(r.status, 400, "{}", r.body);

        // Owner-role session ok; Admin / Member / none 403.
        let boss = connect_users::create_session("boss");
        let r = http(d.port, "GET", &format!("/cli/users/audit?token={boss}&tail=1"), None, &[]);
        assert_eq!(r.status, 200, "Owner session; {}", r.body);
        let deputy = connect_users::create_session("deputy");
        let r = http(d.port, "GET", &format!("/cli/users/audit?token={deputy}"), None, &[]);
        assert_eq!(r.status, 403, "Admin session; {}", r.body);
        let member = connect_users::create_session("audited");
        let r = http(d.port, "GET", &format!("/cli/users/audit?token={member}"), None, &[]);
        assert_eq!(r.status, 403, "Member session; {}", r.body);
        let r = http(d.port, "GET", "/cli/users/audit", None, &[]);
        assert_eq!(r.status, 403, "no token; {}", r.body);

        // POST → 405 even with the owner token.
        let r = http(d.port, "POST", &format!("/cli/users/audit?token={OWNER_TOKEN}"), Some("{}"), &[]);
        assert_eq!(r.status, 405, "{}", r.body);

        // Readable over the tunnel with the owner token too (G4).
        let r = http(d.tunnel_port, "GET", &format!("/cli/users/audit?token={OWNER_TOKEN}&tail=1"), None, &[]);
        assert_eq!(r.status, 200);
    });
}

// ─────────────────────────────────────────────────────────────────────
// T1 — 7-day sessions: cookie Max-Age follows session_ttl_days().
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_login_cookie_max_age_is_seven_days() {
    let _g = lock();
    with_temp_home(|| {
        let _s = seed("cookieuser", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        assert_eq!(connect_users::session_ttl_days(), 7);
        let r = http(
            d.port,
            "POST",
            LOGIN,
            Some(r#"{"username":"cookieuser","password":"password123","web":true}"#),
            &[],
        );
        assert_eq!(r.status, 200, "{}", r.body);
        let cookie = r.header("Set-Cookie").expect("Set-Cookie");
        assert!(cookie.contains("Max-Age=604800"), "7 days = 604800 s; got {cookie}");
        let j: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        let exp = chrono::DateTime::parse_from_rfc3339(j["expiresAt"].as_str().unwrap()).unwrap();
        let delta = exp.with_timezone(&chrono::Utc) - chrono::Utc::now();
        assert!(
            delta > chrono::Duration::days(6) && delta <= chrono::Duration::days(7),
            "expiresAt ≈ now + 7d; delta={delta}"
        );
    });
}

fn assert_rate_limited(r: &Resp, what: &str) {
    assert_eq!(r.status, 429, "{what} must 429; headers={} body={}", r.headers, r.body);
    assert_eq!(r.body, r#"{"error":"rate_limited"}"#, "{what}");
    assert_eq!(r.header("Retry-After").as_deref(), Some("300"), "{what}");
    assert_eq!(
        r.header("Content-Type").as_deref(),
        Some("application/json"),
        "{what}"
    );
}

// ─────────────────────────────────────────────────────────────────────
// T1–T10 — per-IP login throttle (5 / 300s), after 144, before argon2.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tunnel_sixth_attested_login_from_one_ip_is_429_argon2_skipped() {
    let _g = lock();
    with_temp_home(|| {
        k2_daemon::login_throttle::reset();
        let signer = seed("throt0", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let ip = "198.51.100.10";

        for i in 1..=5 {
            let user = format!("throt{i}");
            connect_users::add_user(&user, "password123").expect("add_user");
            let body = format!(r#"{{"username":"{user}","password":"WRONG"}}"#);
            let r = attested_login(&d, &signer, &body, now(), &random_nonce(), ip, SUB, &[]);
            assert_eq!(r.status, 401, "attempt {i} still verifies; body={}", r.body);
            assert_eq!(
                connect_users::lockout_failed_count(&user),
                1,
                "attempt {i} counted as a username fail"
            );
        }

        connect_users::add_user("throt6", "password123").expect("add_user");
        let body = r#"{"username":"throt6","password":"WRONG"}"#;
        let r = attested_login(&d, &signer, body, now(), &random_nonce(), ip, SUB, &[]);
        assert_rate_limited(&r, "6th attested login from one IP");
        assert_eq!(
            connect_users::lockout_failed_count("throt6"),
            0,
            "6th must not run argon2 / username lockout"
        );
        let last = audit_events_for("login").pop().expect("rate_limited audit");
        assert_eq!(last["outcome"], "rate_limited", "{last}");
        assert_eq!(last["ip"], ip, "{last}");
        assert_eq!(last["user"], "throt6", "{last}");
        assert!(
            last["ingress"].as_str().expect("ingress").starts_with("edge:"),
            "{last}"
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_sixth_login_is_not_429() {
    let _g = lock();
    with_temp_home(|| {
        k2_daemon::login_throttle::reset();
        let _s = seed("loopt", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let body = r#"{"username":"loopt","password":"WRONG"}"#;
        for i in 1..=6 {
            let r = http(d.port, "POST", LOGIN, Some(body), &[]);
            assert_ne!(r.status, 429, "loopback attempt {i} must not 429; body={}", r.body);
            assert_eq!(r.status, 401, "loopback attempt {i}; body={}", r.body);
        }
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsigned_tunnel_login_stays_404_not_429() {
    let _g = lock();
    with_temp_home(|| {
        k2_daemon::login_throttle::reset();
        let _s = seed("scan", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let body = r#"{"username":"scan","password":"WRONG"}"#;
        for i in 1..=6 {
            let r = http(d.tunnel_port, "POST", LOGIN, Some(body), &[]);
            assert_eq!(r.status, 404, "unsigned tunnel attempt {i} must 404; body={}", r.body);
            assert_ne!(r.status, 429, "unsigned tunnel must not 429 (no oracle)");
            assert_eq!(r.body, r#"{"error":"not_found"}"#, "attempt {i}");
        }
        assert_eq!(
            connect_users::lockout_failed_count("scan"),
            0,
            "unsigned tunnel must not touch username lockout"
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_ip_spray_does_not_429_skin_login() {
    let _g = lock();
    with_temp_home(|| {
        k2_daemon::login_throttle::reset();
        let signer = seed("sprayc", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let ip = "198.51.100.11";
        for i in 1..=5 {
            let body = r#"{"username":"sprayc","password":"WRONG"}"#;
            let r = attested_login(&d, &signer, body, now(), &random_nonce(), ip, SUB, &[]);
            assert_eq!(r.status, 401, "connect attempt {i}; body={}", r.body);
        }
        let sixth = attested_login(
            &d,
            &signer,
            r#"{"username":"sprayc","password":"WRONG"}"#,
            now(),
            &random_nonce(),
            ip,
            SUB,
            &[],
        );
        assert_rate_limited(&sixth, "6th Connect from one IP");

        let skin = http(
            d.port,
            "POST",
            "/cli/skin/login",
            Some(r#"{"username":"guest","password":"WRONG"}"#),
            &[&format!("X-Forwarded-For: {ip}")],
        );
        assert_ne!(
            skin.status, 429,
            "Connect spray must not 429 skin guests; body={}",
            skin.body
        );
        assert_eq!(skin.status, 401, "skin still verifies; body={}", skin.body);
        assert_eq!(
            skin.body.trim(),
            r#"{"error":"invalid username or password"}"#,
            "generic 401 body unchanged; got {}",
            skin.body
        );
    });
}
