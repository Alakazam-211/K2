//! The App egress proxy (S15), end to end on loopback: a fake upstream, a
//! fake resolver for test names (everything else goes through the real
//! egress gate), a recording sink, and a temp `$HOME` for the log.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::time::Duration;

use super::*;

static SEEN: Mutex<Vec<(String, String, u16, bool, String)>> = Mutex::new(Vec::new());
/// Test names → 127.0.0.1 (on the asked port).
const TEST_NAMES: &[&str] = &["up.example.test", "other.example.test", "a.wild.example.test"];

fn seams() {
    set_resolver(Some(Arc::new(|host: &str, port: u16| {
        if TEST_NAMES.contains(&host) {
            return Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))]);
        }
        crate::egress::vet_host_port(host, port)?.socket_addrs(port)
    })));
    set_recorder(Some(Arc::new(|app: &str, host: &str, port: u16, through: bool, _rule: Option<&HostSpec>, code: &str| {
        SEEN.lock().unwrap().push((app.into(), host.into(), port, through, code.into()));
    })));
    set_own_names_for_test(Some(vec!["rosson.k2.dev".into(), "jobs.example.com".into()]));
}

fn seen(app: &str) -> Vec<(String, u16, bool, String)> {
    SEEN.lock().unwrap().iter().filter(|s| s.0 == app).map(|s| (s.1.clone(), s.2, s.3, s.4.clone())).collect()
}

/// An upstream that records what it got and echoes it back with a prefix.
fn upstream() -> (u16, std::sync::mpsc::Receiver<Vec<u8>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            let tx = tx.clone();
            std::thread::spawn(move || {
                s.set_read_timeout(Some(Duration::from_millis(400))).unwrap();
                let mut got = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    match s.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            got.extend_from_slice(&buf[..n]);
                            let _ = s.write_all(b"echo:");
                            let _ = s.write_all(&buf[..n]);
                        }
                        Err(_) => break,
                    }
                }
                let _ = tx.send(got);
            });
        }
    });
    (port, rx)
}

/// The log line lands just after the client sees the close: wait for it.
fn wait_log(app: &str, what: &str, pred: impl Fn(&J) -> bool) -> Vec<J> {
    let t = Instant::now();
    loop {
        let log = tail(app, 100);
        if log.iter().any(&pred) {
            return log;
        }
        assert!(t.elapsed() < Duration::from_secs(5), "{what}: {log:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn listen(app: &str, p: Policy) -> u16 {
    ensure(app, 0).expect("listen");
    set_policy(app, p);
    listening(app).expect("port")
}

fn allow(hosts: &[&str]) -> Policy {
    Policy { broad: false, allowed: hosts.iter().map(|h| pol::parse_spec(h).unwrap()).collect() }
}

fn client(px: u16) -> std::net::TcpStream {
    let c = std::net::TcpStream::connect(("127.0.0.1", px)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c
}

/// Read the proxy's answer head (and any body bytes already there).
fn answer(c: &mut std::net::TcpStream) -> String {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match c.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                let s = String::from_utf8_lossy(&out);
                if s.starts_with("HTTP/1.1 200 Connection Established\r\n\r\n") {
                    break;
                }
                if let Some(i) = s.find("\r\n\r\n") {
                    let len = s[..i]
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .and_then(|n| n.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if out.len() >= i + 4 + len {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn connect(px: u16, target: &str) -> (std::net::TcpStream, String) {
    let mut c = client(px);
    c.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes()).unwrap();
    let a = answer(&mut c);
    (c, a)
}

fn code_of(a: &str) -> String {
    let body = a.split("\r\n\r\n").nth(1).unwrap_or("");
    let v: J = serde_json::from_str(body).unwrap_or(J::Null);
    v["error"]["code"].as_str().unwrap_or("").to_string()
}

/// A real TLS ClientHello for `name` (rustls), as the App would send it.
fn client_hello(name: &str) -> Vec<u8> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let mut c = rustls::ClientConnection::new(Arc::new(cfg), rustls::pki_types::ServerName::try_from(name.to_string()).unwrap()).unwrap();
    let mut out = Vec::new();
    c.write_tls(&mut out).unwrap();
    out
}

#[test]
fn heads_parse_strictly() {
    let h = parse_head("CONNECT API.Example.com:443 HTTP/1.1\r\nHost: x").unwrap();
    assert!(h.connect);
    assert_eq!((h.host.as_str(), h.port), ("api.example.com", 443));
    let h = parse_head("CONNECT [2001:db8::1]:8443 HTTP/1.1").unwrap();
    assert_eq!((h.host.as_str(), h.port), ("2001:db8::1", 8443));
    let h = parse_head("POST http://api.example.com:8080/v1/x?a=b HTTP/1.1\r\nHost: evil.com\r\nContent-Length: 2").unwrap();
    assert!(!h.connect);
    assert_eq!((h.host.as_str(), h.port, h.path.as_str()), ("api.example.com", 8080, "/v1/x?a=b"));
    assert_eq!(parse_head("GET http://api.example.com HTTP/1.1").unwrap().port, 80);
    for (bad, why) in [
        ("GET /cli/projects/list HTTP/1.1", "origin-form: never K2 itself"),
        ("GET * HTTP/1.1", "asterisk"),
        ("GET https://api.example.com/ HTTP/1.1", "https goes through CONNECT"),
        ("GET ftp://x.example.com/ HTTP/1.1", "only http"),
        ("GET http://user:pw@api.example.com/ HTTP/1.1", "user info"),
        ("CONNECT api.example.com HTTP/1.1", "no port"),
        ("CONNECT api.example.com:0 HTTP/1.1", "port 0"),
        ("CONNECT bad_name.example.com:443 HTTP/1.1", "bad name"),
        ("CONNECT api.example.com:443 HTTP/2", "version"),
        ("CONNECT api.example.com:443 HTTP/1.1 x", "extra word"),
        ("GET http://api.example.com/ HTTP/1.1\r\n folded: x", "folding"),
        ("GET http://api.example.com/ HTTP/1.1\r\nbad header", "no colon"),
    ] {
        let e = parse_head(bad).expect_err(why);
        assert_eq!(e.code, codes::BAD_REQUEST, "{why}");
        assert_eq!(e.status, 400);
    }
    assert!(parse_head("GET /x HTTP/1.1").unwrap_err().hint.contains("K2_GATEWAY_URL"));
}

#[test]
fn upstream_head_forces_host_and_one_request() {
    let h = parse_head(
        "GET http://api.example.com:8080/v1?q=1 HTTP/1.1\r\nHost: evil.com\r\nProxy-Authorization: Basic eA==\r\n\
         Proxy-Connection: keep-alive\r\nConnection: keep-alive\r\nAccept: */*",
    )
    .unwrap();
    let out = String::from_utf8(upstream_head(&h)).unwrap();
    assert!(out.starts_with("GET /v1?q=1 HTTP/1.1\r\nHost: api.example.com:8080\r\n"), "{out}");
    assert!(!out.to_ascii_lowercase().contains("evil.com"));
    assert!(!out.to_ascii_lowercase().contains("proxy-"));
    assert!(out.contains("Accept: */*\r\n"));
    assert!(out.contains(&format!("User-Agent: {}\r\n", crate::egress::USER_AGENT)), "K2's default UA when none");
    assert!(out.ends_with("Connection: close\r\n\r\n"));
    assert_eq!(out.matches("Connection:").count(), 1);
    let h = parse_head("GET http://api.example.com/ HTTP/1.1\r\nUser-Agent: app/1").unwrap();
    let out = String::from_utf8(upstream_head(&h)).unwrap();
    assert!(out.contains("Host: api.example.com\r\n") && out.contains("User-Agent: app/1\r\n") && !out.contains("K2/"));
}

#[test]
fn sni_is_read_from_a_real_client_hello() {
    assert_eq!(client_hello_sni(&client_hello("api.example.com")).unwrap().as_deref(), Some("api.example.com"));
    assert!(client_hello_sni(b"\x16\x03\x01\x00\x02\x01\x00").is_err());
    assert!(client_hello_sni(b"GET / HTTP/1.1\r\n").is_err());
}

#[test]
fn the_bucket_refills() {
    let mut b = Bucket { tokens: 2.0, at: Instant::now() };
    assert!(b.take() && b.take());
    assert!(!b.take(), "empty");
    b.at -= Duration::from_secs(1);
    assert!(b.take(), "refilled at {RATE_PER_SEC}/s");
}

#[test]
fn policy_check_refuses_own_local_and_unlisted_names() {
    let _home = crate::test_support::TempHome::new();
    seams();
    set_policy("chk", allow(&["api.example.com", "*.wild.example.test"]));
    assert_eq!(check("chk", "api.example.com", 443).unwrap(), Verdict::Listed(pol::parse_spec("api.example.com").unwrap()));
    assert!(matches!(check("chk", "a.wild.example.test", 443).unwrap(), Verdict::Listed(_)));
    assert_eq!(check("chk", "api.example.com", 80).unwrap_err().code, codes::DENIED);
    assert_eq!(check("chk", "rosson.k2.dev", 443).unwrap_err().code, codes::SELF);
    assert_eq!(check("chk", "jobs.rosson.k2.dev", 443).unwrap_err().code, codes::SELF, "nested App names route here too");
    assert_eq!(check("chk", "jobs.example.com", 443).unwrap_err().code, codes::SELF, "a bound custom name");
    assert!(check("chk", "x.jobs.example.com", 443).is_err(), "not listed (not own: only exact custom names)");
    assert_eq!(check("chk", "localhost", 443).unwrap_err().code, codes::DENIED);
    assert_eq!(check("nobody", "api.example.com", 443).unwrap_err().code, codes::DENIED, "no policy: fail closed");
    let e = check("chk", "evil.example.com", 443).unwrap_err();
    assert!(e.hint.contains("k2 app egress add chk evil.example.com:443"), "{}", e.hint);
    // Broad lets public names through, never own or local ones.
    set_policy("chk", Policy { broad: true, allowed: vec![] });
    assert_eq!(check("chk", "evil.example.com", 443).unwrap(), Verdict::Broad);
    assert_eq!(check("chk", "rosson.k2.dev", 443).unwrap_err().code, codes::SELF);
    // The key-proxy seam: same rules, vetted addresses back.
    set_policy("chk", allow(&["up.example.test:9443"]));
    assert_eq!(check_for_key_proxy("chk", "UP.example.test", 9443).unwrap(), vec![SocketAddr::from(([127, 0, 0, 1], 9443))]);
    assert_eq!(check_for_key_proxy("chk", "other.example.test", 9443).unwrap_err().code, codes::DENIED);
    stop("chk");
}

#[test]
fn connect_tunnels_allowed_hosts_and_refuses_the_rest() {
    let _home = crate::test_support::TempHome::new();
    seams();
    let (up, rx) = upstream();
    let app = "egr-connect";
    let px = listen(app, allow(&[&format!("up.example.test:{up}")]));

    // Allowed: 200, then bytes both ways.
    let (mut c, a) = connect(px, &format!("up.example.test:{up}"));
    assert_eq!(a, "HTTP/1.1 200 Connection Established\r\n\r\n");
    c.write_all(b"hello").unwrap();
    let mut buf = [0u8; 64];
    let n = c.read(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"echo:hello");
    drop(c);
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), b"hello");

    // Not listed (another name, the same name on another port), own,
    // local, metadata, private, loopback: refused with a code, never dialed.
    for (target, code) in [
        (format!("other.example.test:{up}"), codes::DENIED),
        ("up.example.test:1".to_string(), codes::DENIED),
        ("rosson.k2.dev:443".to_string(), codes::SELF),
        ("localhost:443".to_string(), codes::DENIED),
        ("169.254.169.254:80".to_string(), codes::DENIED),
        ("10.0.0.5:443".to_string(), codes::DENIED),
        (format!("127.0.0.1:{up}"), codes::DENIED),
    ] {
        let (_c, a) = connect(px, &target);
        assert!(a.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{target}: {a}");
        assert!(a.contains("X-K2-Egress: refused"));
        assert_eq!(code_of(&a), code, "{target}");
    }
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "nothing refused reached the upstream");
    // Even listed, a private/loopback address is refused by the egress gate.
    set_policy(app, allow(&["169.254.169.254:80", &format!("127.0.0.1:{up}")]));
    for target in ["169.254.169.254:80".to_string(), format!("127.0.0.1:{up}")] {
        let (_c, a) = connect(px, &target);
        assert_eq!(code_of(&a), codes::DENIED, "{target}: {a}");
    }

    // Talking to the proxy as if it were K2: refused.
    let mut c = client(px);
    c.write_all(b"GET /cli/projects/list?token=x HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
    let a = answer(&mut c);
    assert!(a.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{a}");
    assert_eq!(code_of(&a), codes::BAD_REQUEST);

    // Every decision was recorded; refused ones with their code.
    let s = seen(app);
    assert!(s.contains(&("up.example.test".into(), up, true, "ok".into())), "{s:?}");
    assert!(s.contains(&("other.example.test".into(), up, false, codes::DENIED.into())), "{s:?}");
    assert!(s.contains(&("rosson.k2.dev".into(), 443, false, codes::SELF.into())), "{s:?}");
    // The log has one line per connection, host and port only.
    let log = wait_log(app, "the ok tunnel", |l| l["result"] == "ok" && l["host"] == "up.example.test");
    assert!(log.iter().any(|l| l["result"] == "ok" && l["up"] == 5 && l["down"] == 10), "bytes both ways: {log:?}");
    assert!(log.iter().any(|l| l["result"] == "refused" && l["code"] == codes::SELF));
    let raw = std::fs::read_to_string(log_path(app)).unwrap();
    assert!(!raw.contains("token=x") && !raw.contains("/cli/"), "never paths or queries: {raw}");
    let st = status_json(app);
    assert_eq!(st["listening"], true);
    assert!(st["refused"].as_u64().unwrap() >= 8, "{st}");
    stop(app);
    assert!(listening(app).is_none());
    assert!(std::net::TcpStream::connect(("127.0.0.1", px)).is_err() || {
        // The accept loop may take a tick to drop the socket.
        std::thread::sleep(Duration::from_millis(300));
        std::net::TcpStream::connect(("127.0.0.1", px)).is_err()
    });
}

#[test]
fn plain_http_is_one_request_with_the_host_forced() {
    let _home = crate::test_support::TempHome::new();
    seams();
    let (up, rx) = upstream();
    let app = "egr-http";
    let px = listen(app, allow(&[&format!("up.example.test:{up}")]));
    let mut c = client(px);
    c.write_all(
        format!(
            "POST http://up.example.test:{up}/v1/x?y=1 HTTP/1.1\r\nHost: evil.com\r\nProxy-Authorization: Basic eA==\r\n\
             Content-Length: 4\r\n\r\nbody"
        )
        .as_bytes(),
    )
    .unwrap();
    let got = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let got = String::from_utf8(got).unwrap();
    assert!(got.starts_with(&format!("POST /v1/x?y=1 HTTP/1.1\r\nHost: up.example.test:{up}\r\n")), "{got}");
    assert!(!got.contains("evil.com") && !got.to_ascii_lowercase().contains("proxy-authorization"), "{got}");
    assert!(got.contains("Connection: close\r\n") && got.ends_with("\r\n\r\nbody"), "{got}");
    // The upstream's answer comes back through.
    let mut back = Vec::new();
    let _ = c.read_to_end(&mut back);
    assert!(String::from_utf8_lossy(&back).starts_with("echo:POST /v1/x"));
    // An https:// URL as a plain request is refused (CONNECT only).
    let mut c = client(px);
    c.write_all(format!("GET https://up.example.test:{up}/ HTTP/1.1\r\n\r\n").as_bytes()).unwrap();
    assert_eq!(code_of(&answer(&mut c)), codes::BAD_REQUEST);
    stop(app);
}

#[test]
fn tls_must_name_the_connect_host() {
    let _home = crate::test_support::TempHome::new();
    seams();
    let (up, rx) = upstream();
    let app = "egr-sni";
    let px = listen(app, allow(&[&format!("up.example.test:{up}")]));
    // Fronting: CONNECT to an allowed name, TLS to another: closed, nothing sent.
    let (mut c, a) = connect(px, &format!("up.example.test:{up}"));
    assert!(a.starts_with("HTTP/1.1 200"));
    c.write_all(&client_hello("evil.example.com")).unwrap();
    let mut buf = [0u8; 64];
    assert!(matches!(c.read(&mut buf), Ok(0) | Err(_)), "closed");
    // The upstream (an allowed host) was dialed, but no byte reached it.
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(got) => assert!(got.is_empty(), "the upstream got {} bytes", got.len()),
        Err(_) => {}
    }
    // The matching name passes, bytes and all.
    let (mut c, _) = connect(px, &format!("up.example.test:{up}"));
    let hello = client_hello("up.example.test");
    c.write_all(&hello).unwrap();
    let n = c.read(&mut buf).unwrap();
    assert!(n > 0 && buf.starts_with(b"echo:"));
    drop(c);
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), hello);
    wait_log(app, "the SNI refusal", |l| l["code"] == codes::SNI);
    stop(app);
}

#[test]
fn narrowing_and_stopping_cut_open_tunnels_and_idle_ones_close() {
    let _home = crate::test_support::TempHome::new();
    seams();
    let (up, _rx) = upstream();
    let app = "egr-cut";
    let target = format!("up.example.test:{up}");
    let px = listen(app, allow(&[&target]));
    let (mut c, a) = connect(px, &target);
    assert!(a.starts_with("HTTP/1.1 200"));
    c.write_all(b"x").unwrap();
    let mut buf = [0u8; 16];
    assert!(c.read(&mut buf).unwrap() > 0);
    // Narrow: the host is gone from the list; the open tunnel is cut now.
    let p = Policy::default();
    set_policy(app, p.clone());
    cut_unlisted(app, &p);
    c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    assert!(matches!(c.read(&mut buf), Ok(0) | Err(_)), "cut by narrowing");
    // App stop cuts tunnels too.
    set_policy(app, allow(&[&target]));
    let (mut c, _) = connect(px, &target);
    stop(app);
    assert!(matches!(c.read(&mut buf), Ok(0) | Err(_)), "cut by stop");
    // Idle tunnels close (IDLE is 1.5 s in tests).
    let px = listen(app, allow(&[&target]));
    let (mut c, _) = connect(px, &target);
    c.set_read_timeout(Some(Duration::from_secs(6))).unwrap();
    let t = Instant::now();
    assert!(matches!(c.read(&mut buf), Ok(0) | Err(_)));
    assert!(t.elapsed() < Duration::from_secs(5), "closed for idling");
    let log = wait_log(app, "the idle close", |l| l["end"] == "idle");
    for end in ["narrowed", "app_stopped", "idle"] {
        assert!(log.iter().any(|l| l["end"] == end), "{end}: {log:?}");
    }
    stop(app);
}
