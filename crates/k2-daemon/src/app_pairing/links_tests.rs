//! The link files (S4): the bodies, which requests K2 answers itself, at
//! any position on a keep-alive connection through `client_ip::forward`,
//! and before the App is dialed (the App down, the first request), on the
//! App's own hosts only.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;

fn sample(app_id: &str, host: &str) -> LinkSet {
    LinkSet {
        service_id: format!("svc-{app_id}"),
        app_name: "Jobs <script>alert(1)</script>".into(),
        ios: vec!["ABCDE12345.com.jobs.app".into(), "ABCDE12345.com.jobs.beta".into()],
        android: vec![AndroidApp {
            package: "com.jobs.app".into(),
            sha256: vec!["AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89".into()],
        }],
        link_paths: vec!["/jobs/*".into()],
        hosts: vec![host.to_string()],
        store_ios: Some("https://apps.apple.com/app/id1?x=\"1\"".into()),
        store_android: None,
    }
}

#[test]
fn the_aasa_names_the_owners_ios_apps_for_the_pair_path_and_the_apps_own_paths() {
    let v: serde_json::Value = serde_json::from_str(&aasa_body(&sample("a", "a.box.k2.dev"))).expect("json");
    assert_eq!(v["applinks"]["apps"], json!([]));
    let d = v["applinks"]["details"].as_array().expect("details");
    assert_eq!(d.len(), 2);
    assert_eq!(d[0]["appID"], "ABCDE12345.com.jobs.app");
    assert_eq!(d[0]["paths"], json!(["/.well-known/k2-pair", "/jobs/*"]));
    assert_eq!(d[0]["appIDs"], json!(["ABCDE12345.com.jobs.app"]));
    assert_eq!(d[0]["components"], json!([{"/": "/.well-known/k2-pair"}, {"/": "/jobs/*"}]));
    assert_eq!(d[1]["appID"], "ABCDE12345.com.jobs.beta");
    let a: serde_json::Value = serde_json::from_str(&assetlinks_body(&sample("a", "a.box.k2.dev"))).expect("json");
    assert_eq!(
        a,
        json!([{
            "relation": ["delegate_permission/common.handle_all_urls"],
            "target": {"namespace": "android_app", "package_name": "com.jobs.app",
                       "sha256_cert_fingerprints": ["AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89"]}
        }])
    );
}

#[test]
fn the_landing_page_escapes_loads_nothing_and_warns() {
    let html = page_html(&sample("a", "a.box.k2.dev"), "n0nce");
    assert!(html.contains("<script nonce=\"n0nce\">") && html.contains("<style nonce=\"n0nce\">"));
    assert!(!html.contains("<script>alert(1)</script>"), "the App name is escaped");
    assert!(html.contains("Jobs &lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(html.contains("href=\"https://apps.apple.com/app/id1?x=&quot;1&quot;\""), "store links are escaped");
    assert!(html.contains("Never share this code"));
    assert!(html.contains("location.hash"));
    for banned in ["fetch(", "XMLHttpRequest", "sendBeacon", "<img", "<link", "src=\"http", "k2companion://", "://pair"] {
        assert!(!html.contains(banned), "{banned}");
    }
    let csp = page_csp("n0nce");
    assert!(csp.contains("connect-src 'none'") && csp.contains("frame-ancestors 'none'") && csp.starts_with("default-src 'none'"));
}

#[test]
fn only_get_and_head_of_the_three_paths_on_the_apps_hosts_are_answered() {
    let id = "links-answer-unit";
    set(id, Some(sample(id, "jobs.box.k2.dev")));
    let ok = |m: &str, p: &str, h: Option<&str>| answer_on(true, id, m, p, h);
    let r = String::from_utf8(ok("GET", AASA, Some("jobs.box.k2.dev")).expect("aasa")).expect("utf8");
    assert!(r.starts_with("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"), "{r}");
    assert!(r.contains("Connection: close\r\n") && r.contains("Cache-Control: public, max-age=300\r\n") && r.contains("X-Content-Type-Options: nosniff"));
    assert!(ok("GET", "/.well-known/assetlinks.json?x=1", Some("JOBS.box.k2.dev:443")).is_some(), "query ignored, Host folded");
    let page = String::from_utf8(ok("GET", PAIR_PATH, Some("jobs.box.k2.dev")).expect("page")).expect("utf8");
    assert!(page.contains("Content-Security-Policy: default-src 'none'") && page.contains("Cache-Control: no-store") && page.contains("Referrer-Policy: no-referrer"));
    let head = String::from_utf8(ok("HEAD", AASA, Some("jobs.box.k2.dev")).expect("head")).expect("utf8");
    assert!(head.ends_with("\r\n\r\n"), "HEAD: headers only: {head}");
    let full = String::from_utf8(ok("GET", AASA, Some("jobs.box.k2.dev")).expect("get")).expect("utf8");
    assert_eq!(head, full.split("\r\n\r\n").next().expect("head") .to_string() + "\r\n\r\n", "same headers, same Content-Length");
    for (m, p, h) in [
        ("POST", AASA, Some("jobs.box.k2.dev")),
        ("GET", "/", Some("jobs.box.k2.dev")),
        ("GET", "/.well-known/k2-pair/x", Some("jobs.box.k2.dev")),
        ("GET", "/.well-known/security.txt", Some("jobs.box.k2.dev")),
        ("GET", AASA, Some("other.box.k2.dev")),
        ("GET", AASA, None),
    ] {
        assert!(ok(m, p, h).is_none(), "{m} {p} {h:?} reaches the App");
    }
    assert!(answer_on(false, id, "GET", AASA, Some("jobs.box.k2.dev")).is_none(), "K2_APP_PAIRING=off: the App's own files");
    assert!(answer_on(true, "links-not-pairing", "GET", AASA, Some("jobs.box.k2.dev")).is_none(), "pairing off: the App's own files");
    // No iOS app: the App's own AASA passes through; no Android: its assetlinks.
    let mut android_only = sample(id, "jobs.box.k2.dev");
    android_only.ios.clear();
    set(id, Some(android_only));
    assert!(ok("GET", AASA, Some("jobs.box.k2.dev")).is_none());
    assert!(ok("GET", ASSETLINKS, Some("jobs.box.k2.dev")).is_some());
    set(id, None);
    assert!(ok("GET", ASSETLINKS, Some("jobs.box.k2.dev")).is_none());
}

struct Fake(Vec<(&'static str, Fetched)>);

impl Fetcher for Fake {
    fn get(&self, url: &str) -> Fetched {
        self.0.iter().find(|(u, _)| url.contains(u)).map(|(_, f)| f.clone()).unwrap_or(Fetched::Unreachable)
    }
}

#[test]
fn check_maps_each_answer() {
    let mut l = sample("c", "a.box.k2.dev");
    l.hosts = vec!["a.box.k2.dev".into(), "b.box.k2.dev".into(), "c.box.k2.dev".into()];
    let aasa = aasa_body(&l);
    let assets = assetlinks_body(&l);
    let f = Fake(vec![
        ("cdn-apple.com/a/v1/a.box", Fetched::Ok { status: 200, body: aasa.clone() }),
        ("a.box.k2.dev/.well-known/apple", Fetched::Ok { status: 200, body: aasa.clone() }),
        ("a.box.k2.dev/.well-known/asset", Fetched::Ok { status: 200, body: assets }),
        ("b.box.k2.dev/.well-known/apple", Fetched::Ok { status: 200, body: "{\"applinks\":{}}".into() }),
        ("b.box.k2.dev/.well-known/asset", Fetched::Ok { status: 404, body: String::new() }),
        ("c.box.k2.dev", Fetched::Tls),
    ]);
    let r = check_hosts(&f, &l);
    assert_eq!((r[0].aasa, r[0].assetlinks, r[0].apple_cdn), ("ok", "ok", "ok"));
    assert_eq!((r[1].aasa, r[1].assetlinks, r[1].apple_cdn), ("stale", "not_served", "not_served"));
    assert_eq!((r[2].aasa, r[2].assetlinks), ("tls_error", "tls_error"));
    l.ios.clear();
    assert_eq!(check_hosts(&f, &l)[0].aasa, "none");
}

// ── over real loopback ───────────────────────────────────────────────

async fn read_all(mut s: TcpStream) -> String {
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out)).await.expect("closed in time");
    String::from_utf8_lossy(&out).to_string()
}

/// A client whose connection `forward_with` carries to a fresh echo App,
/// with `app_id`'s link files.
async fn via_forward(app_id: &str) -> TcpStream {
    let app = crate::client_ip::tests::echo_app().await;
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = l.local_addr().expect("addr");
    let local = local_for(app_id);
    tokio::spawn(async move {
        let (mut down, _) = l.accept().await.expect("accept");
        let mut up = TcpStream::connect(("127.0.0.1", app)).await.expect("app");
        let _ = crate::client_ip::forward_with(&mut down, &mut up, crate::client_ip::Mode::Tunnel(None), local).await;
        let _ = down.shutdown().await;
    });
    TcpStream::connect(addr).await.expect("connect")
}

#[tokio::test]
async fn forward_answers_the_link_files_at_any_position_then_closes() {
    let id = "links-fwd-1";
    set(id, Some(sample(id, "jobs.box.k2.dev")));
    // First request: K2's file, never the App's echo.
    let mut c = via_forward(id).await;
    c.write_all(b"GET /.well-known/apple-app-site-association HTTP/1.1\r\nHost: jobs.box.k2.dev\r\n\r\n").await.unwrap();
    let out = read_all(c).await;
    assert!(out.starts_with("HTTP/1.1 200 OK\r\nContent-Type: application/json"), "{out}");
    assert!(out.contains("ABCDE12345.com.jobs.app") && !out.contains("Host: jobs.box.k2.dev"), "{out}");
    // Behind an App request on a keep-alive connection (pipelined in one
    // write): the App answers first, then K2, then the connection closes.
    let mut c = via_forward(id).await;
    c.write_all(
        b"GET /hello HTTP/1.1\r\nHost: jobs.box.k2.dev\r\n\r\nGET /.well-known/k2-pair HTTP/1.1\r\nHost: jobs.box.k2.dev\r\n\r\nGET /after HTTP/1.1\r\nHost: jobs.box.k2.dev\r\n\r\n",
    )
    .await
    .unwrap();
    let out = read_all(c).await;
    let app = out.find("GET /hello").expect("the App answered the first");
    let k2 = out.find("Open this in the Jobs").expect("K2 answered the second");
    assert!(app < k2, "{out}");
    assert!(!out.contains("GET /after"), "nothing after K2's answer reaches the App: {out}");
    // Another path, another host, and a request with a body reach the App.
    for req in [
        "GET /jobs/1 HTTP/1.1\r\nHost: jobs.box.k2.dev\r\n\r\n",
        "GET /.well-known/apple-app-site-association HTTP/1.1\r\nHost: elsewhere.example\r\n\r\n",
        "GET /.well-known/apple-app-site-association HTTP/1.1\r\nHost: jobs.box.k2.dev\r\nContent-Length: 2\r\n\r\nhi",
    ] {
        let mut c = via_forward(id).await;
        c.write_all(req.as_bytes()).await.unwrap();
        c.shutdown().await.unwrap();
        let out = read_all(c).await;
        assert!(out.contains(req.lines().next().expect("line")), "{req:?} reached the App: {out}");
    }
    // An App whose pairing is off: its own files, from the App.
    let mut c = via_forward("links-fwd-off").await;
    c.write_all(b"GET /.well-known/apple-app-site-association HTTP/1.1\r\nHost: jobs.box.k2.dev\r\n\r\n").await.unwrap();
    c.shutdown().await.unwrap();
    assert!(read_all(c).await.contains("GET /.well-known/apple-app-site-association"));
    set(id, None);
}

#[tokio::test]
async fn two_apps_on_two_hosts_each_get_their_own_files() {
    let (a, b) = ("links-two-a", "links-two-b");
    let mut sa = sample(a, "a.box.k2.dev");
    sa.ios = vec!["AAAAAAAAAA.com.a".into()];
    let mut sb = sample(b, "b.box.k2.dev");
    sb.ios = vec!["BBBBBBBBBB.com.b".into()];
    set(a, Some(sa));
    set(b, Some(sb));
    let ga = String::from_utf8(answer_on(true, a, "GET", AASA, Some("a.box.k2.dev")).expect("a")).expect("utf8");
    let gb = String::from_utf8(answer_on(true, b, "GET", AASA, Some("b.box.k2.dev")).expect("b")).expect("utf8");
    assert!(ga.contains("AAAAAAAAAA.com.a") && !ga.contains("BBBBBBBBBB"));
    assert!(gb.contains("BBBBBBBBBB.com.b") && !gb.contains("AAAAAAAAAA"));
    assert!(answer_on(true, a, "GET", AASA, Some("b.box.k2.dev")).is_none(), "App A on App B's host: never B's file");
    set(a, None);
    set(b, None);
}

/// A backend that counts its connections and echoes a fixed answer.
async fn counting_backend() -> (u16, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = n.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            n2.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut b = [0u8; 4096];
                let mut got = Vec::new();
                loop {
                    match s.read(&mut b).await {
                        Ok(0) | Err(_) => return,
                        Ok(k) => got.extend_from_slice(&b[..k]),
                    }
                    while let Some(i) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                        got.drain(..i + 4);
                        if s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\napp").await.is_err() {
                            return;
                        }
                    }
                }
            });
        }
    });
    (port, n)
}

/// One front-port connection for `app_id` (served by `serve_app_conn`).
async fn via_front(app_id: &str) -> TcpStream {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = l.local_addr().expect("addr");
    let id = app_id.to_string();
    tokio::spawn(async move {
        let (mut s, peer) = l.accept().await.expect("accept");
        crate::apps::router::serve_app_conn(&mut s, &id, Some(peer)).await;
    });
    TcpStream::connect(addr).await.expect("connect")
}

#[tokio::test]
async fn the_front_answers_before_dialing_so_a_down_app_still_serves_its_files() {
    // The client-IP opt-in list lives under $HOME: sandbox it.
    let _home = crate::test_support::TempHome::new();
    let id = "links-front-down";
    set(id, Some(sample(id, "jobs.box.k2.dev")));
    let mut c = via_front(id).await;
    c.write_all(b"GET /.well-known/assetlinks.json HTTP/1.1\r\nHost: jobs.box.k2.dev\r\n\r\n").await.unwrap();
    let out = read_all(c).await;
    assert!(out.starts_with("HTTP/1.1 200 OK") && out.contains("com.jobs.app"), "the App is down, the file is served: {out}");
    let mut c = via_front(id).await;
    c.write_all(b"GET / HTTP/1.1\r\nHost: jobs.box.k2.dev\r\n\r\n").await.unwrap();
    assert!(read_all(c).await.starts_with("HTTP/1.1 503"), "anything else is still the down App's 503");
    // App up: the first request is answered without dialing it; a later
    // one on the same connection, after the App's answer, too.
    let (port, dials) = counting_backend().await;
    let _ = crate::apps::router::switch(id, port);
    let mut c = via_front(id).await;
    c.write_all(b"HEAD /.well-known/k2-pair HTTP/1.1\r\nHost: jobs.box.k2.dev\r\n\r\n").await.unwrap();
    let out = read_all(c).await;
    assert!(out.starts_with("HTTP/1.1 200 OK") && out.ends_with("\r\n\r\n"), "HEAD: headers only: {out}");
    assert_eq!(dials.load(Ordering::SeqCst), 0, "the App was never dialed");
    let mut c = via_front(id).await;
    c.write_all(b"GET /x HTTP/1.1\r\nHost: jobs.box.k2.dev\r\n\r\n").await.unwrap();
    let mut first = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(10), c.read(&mut first)).await.expect("answer").unwrap();
    assert!(String::from_utf8_lossy(&first[..n]).ends_with("app"), "the App answered the first request");
    c.write_all(b"GET /.well-known/apple-app-site-association HTTP/1.1\r\nHost: jobs.box.k2.dev\r\n\r\n").await.unwrap();
    let out = read_all(c).await;
    assert!(out.contains("ABCDE12345.com.jobs.app"), "K2 answered the second: {out}");
    assert_eq!(dials.load(Ordering::SeqCst), 1);
    let _ = crate::apps::router::clear(id);
    set(id, None);
}

#[tokio::test]
async fn a_raw_app_gets_no_link_files() {
    let _home = crate::test_support::TempHome::new();
    let id = "links-front-raw";
    set(id, Some(sample(id, "jobs.box.k2.dev")));
    let (port, dials) = counting_backend().await;
    let _ = crate::apps::router::switch_with(id, port, false);
    let mut c = via_front(id).await;
    c.write_all(b"GET /.well-known/apple-app-site-association HTTP/1.1\r\nHost: jobs.box.k2.dev\r\n\r\n").await.unwrap();
    let mut first = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(10), c.read(&mut first)).await.expect("answer").unwrap();
    assert!(String::from_utf8_lossy(&first[..n]).ends_with("app"), "a raw splice: the App answers");
    assert_eq!(dials.load(Ordering::SeqCst), 1);
    let _ = crate::apps::router::clear(id);
    set(id, None);
}
