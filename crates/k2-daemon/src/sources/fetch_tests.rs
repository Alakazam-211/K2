//! The source fetcher against a fake upstream on loopback (an Owner-typed
//! exact IP, `lan = 1`), plus refusals that never connect.

use super::*;
use crate::egress::test_util::AirgapEnv;
use k2_core::sources::FetchPlan;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A loopback HTTP server answering every request with `resp`; counts
/// accepted connections.
pub(crate) struct Fake {
    pub port: u16,
    pub conns: Arc<AtomicUsize>,
}

impl Fake {
    pub(crate) fn start(resp: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) -> Fake {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = l.local_addr().expect("addr").port();
        let conns = Arc::new(AtomicUsize::new(0));
        let c2 = conns.clone();
        let resp = Arc::new(resp);
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { return };
                c2.fetch_add(1, Ordering::SeqCst);
                let r = resp.clone();
                std::thread::spawn(move || {
                    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut head = Vec::new();
                    let mut buf = [0u8; 2048];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match s.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let _ = s.write_all(&r(&String::from_utf8_lossy(&head)));
                    let _ = s.flush();
                });
            }
        });
        Fake { port, conns }
    }

    pub(crate) fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub(crate) fn conns(&self) -> usize {
        self.conns.load(Ordering::SeqCst)
    }
}

pub(crate) fn http(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut r = format!("HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        r.push_str(&format!("{k}: {v}\r\n"));
    }
    r.push_str("\r\n");
    let mut r = r.into_bytes();
    r.extend_from_slice(body);
    r
}

pub(crate) fn plan(origin: &str, path: &str, lan: bool) -> FetchPlan {
    FetchPlan {
        source_id: Some("s1".into()),
        target_key: None,
        url: UrlRef::Plain(format!("{origin}{path}")),
        origin: format!("{origin}"),
        lan_exact_ip: lan,
        headers: vec![],
        secret: None,
        timeout_secs: 5,
        max_bytes: 1 << 20,
    }
}

#[test]
fn json_304_and_status_codes_map_to_outcomes() {
    let _a = AirgapEnv::set(false);
    let f = Fake::start(|head| {
        let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
        match path.as_str() {
            "/ok" => http("200 OK", &[("ETag", "\"v1\""), ("Content-Type", "application/json")], br#"{"phase":"ready"}"#),
            "/nm" => http("304 Not Modified", &[("ETag", "\"v1\"")], b""),
            "/html" => http("200 OK", &[], b"<html>nope</html>"),
            "/auth" => http("401 Unauthorized", &[], b"{}"),
            "/forbid" => http("403 Forbidden", &[], b"{}"),
            "/missing" => http("404 Not Found", &[], b"{}"),
            "/teapot" => http("418 I'm a teapot", &[], b"{}"),
            "/boom" => http("500 Internal Server Error", &[], b"{}"),
            "/away" => http("302 Found", &[("Location", "http://127.0.0.2:9/x")], b""),
            _ => http("404 Not Found", &[], b""),
        }
    });
    let o = f.origin();
    match fetch_once(&plan(&o, "/ok", true)) {
        FetchOutcome::Ok { status, body, not_modified, etag, .. } => {
            assert_eq!((status, not_modified, etag.as_deref()), (200, false, Some("\"v1\"")));
            assert_eq!(body["phase"], "ready");
        }
        other => panic!("{other:?}"),
    }
    match fetch_once(&plan(&o, "/nm", true)) {
        FetchOutcome::Ok { status, not_modified, .. } => assert_eq!((status, not_modified), (304, true)),
        other => panic!("{other:?}"),
    }
    let reason = |p: &str| match fetch_once(&plan(&o, p, true)) {
        FetchOutcome::Err { reason, .. } => reason,
        other => panic!("{p}: {other:?}"),
    };
    assert_eq!(reason("/html"), Reason::NotJson);
    assert_eq!(reason("/auth"), Reason::HttpAuth);
    assert_eq!(reason("/forbid"), Reason::HttpAuth);
    assert_eq!(reason("/missing"), Reason::Http404);
    assert_eq!(reason("/teapot"), Reason::Http4xx);
    assert_eq!(reason("/boom"), Reason::Http5xx);
    assert_eq!(reason("/away"), Reason::RedirectOtherOrigin);
}

#[test]
fn rate_limit_headers_park_the_origin() {
    let _a = AirgapEnv::set(false);
    let f = Fake::start(|head| {
        if head.starts_with("GET /slow") {
            http("429 Too Many Requests", &[("Retry-After", "120")], b"{}")
        } else if head.starts_with("GET /busy") {
            http("503 Service Unavailable", &[("Retry-After", "99999")], b"{}")
        } else {
            http("200 OK", &[("X-RateLimit-Remaining", "0"), ("X-RateLimit-Reset", "30")], b"{}")
        }
    });
    let r = fetch_detailed(&plan(&f.origin(), "/slow", true));
    assert_eq!(r.park_secs, Some(120));
    assert!(matches!(r.outcome, FetchOutcome::Err { reason: Reason::RateLimited, retry_after_secs: Some(120), .. }));
    let r = fetch_detailed(&plan(&f.origin(), "/busy", true));
    assert_eq!(r.park_secs, Some(k2_core::sources::PARK_MAX_SECS), "parks cap at 1 h");
    let r = fetch_detailed(&plan(&f.origin(), "/spent", true));
    assert!(matches!(r.outcome, FetchOutcome::Ok { .. }), "the answer still counts");
    assert_eq!(r.park_secs, Some(30));
}

#[test]
fn too_large_is_never_parsed() {
    let _a = AirgapEnv::set(false);
    let big = vec![b'7'; 2 << 20];
    let f = Fake::start(move |_| http("200 OK", &[], &big));
    let before = PARSES.with(|c| c.get());
    let out = fetch_once(&plan(&f.origin(), "/", true));
    assert!(matches!(out, FetchOutcome::Err { reason: Reason::TooLarge, .. }), "{out:?}");
    assert_eq!(PARSES.with(|c| c.get()), before, "the extractor never saw the cut body");
}

#[test]
fn a_secretless_ip_source_without_lan_never_connects() {
    let _a = AirgapEnv::set(false);
    let f = Fake::start(|_| http("200 OK", &[], b"{}"));
    let out = fetch_once(&plan(&f.origin(), "/", false));
    assert!(matches!(out, FetchOutcome::Err { reason: Reason::BlockedAddress, .. }), "{out:?}");
    assert_eq!(f.conns(), 0);
}

#[test]
fn transport_failures_are_fixed_reasons() {
    let _a = AirgapEnv::set(false);
    let dead = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = dead.local_addr().expect("addr").port();
    drop(dead);
    let out = fetch_once(&plan(&format!("http://127.0.0.1:{port}"), "/", true));
    assert!(matches!(out, FetchOutcome::Err { reason: Reason::Connect, .. }), "{out:?}");

    let silent = TcpListener::bind("127.0.0.1:0").expect("bind");
    let sport = silent.local_addr().expect("addr").port();
    let hold = std::thread::spawn(move || {
        let c = silent.accept().expect("accept");
        std::thread::sleep(Duration::from_secs(3));
        drop(c);
    });
    let mut p = plan(&format!("http://127.0.0.1:{sport}"), "/", true);
    p.timeout_secs = 1;
    let out = fetch_once(&p);
    assert!(matches!(out, FetchOutcome::Err { reason: Reason::Timeout, .. }), "{out:?}");
    hold.join().expect("silent thread");

    let _pin = crate::egress::test_dns::pin("nowhere.test", vec![]);
    let e = VaultError::new("upstream_error", 502, "couldn't resolve nowhere.test");
    assert_eq!(reason_for(&e), Reason::Dns);
    assert_eq!(reason_for(&VaultError::new("upstream_error", 502, "TLS failed talking to x")), Reason::Tls);
}

#[test]
fn airgap_pauses_names_without_a_lookup_and_runs_owner_ip_sources() {
    let _a = AirgapEnv::set(true);
    let before = crate::egress::test_dns::lookups();
    let out = fetch_once(&plan("https://status.example.com:443", "/api/v2/summary.json", false));
    assert!(matches!(out, FetchOutcome::Err { reason: Reason::Airgap, .. }), "{out:?}");
    assert_eq!(crate::egress::test_dns::lookups(), before, "no DNS lookup under air-gap");
    let f = Fake::start(|_| http("200 OK", &[], br#"{"up":true}"#));
    assert!(matches!(fetch_once(&plan(&f.origin(), "/", true)), FetchOutcome::Ok { .. }));
}

#[test]
fn a_named_secret_reads_secret_missing_until_bound() {
    let _a = AirgapEnv::set(false);
    let f = Fake::start(|_| http("200 OK", &[], b"{}"));
    let mut p = plan(&f.origin(), "/", true);
    p.secret = Some("STRIPE".into());
    assert!(matches!(fetch_once(&p), FetchOutcome::Err { reason: Reason::SecretMissing, .. }));
    assert_eq!(f.conns(), 0);
}
