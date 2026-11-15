//! The proxy core (prd-compute-mac-no-lan-v1 §10.1, §10.2 without a
//! process): the address table, the request parser, the name rules, the
//! one lookup (rebinding), the answers, the limits and the log.

use super::*;
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::atomic::AtomicU32;

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

/// The node's own prefixes as a fake interface list would give them: a LAN
/// on public IPv4 and a dual-stack LAN's global /64.
fn own() -> Vec<Prefix> {
    prefixes(&["81.2.69.0/24".into(), "81.2.69.7/32".into(), "2a01:4f8:1:2::/64".into(), "2a01:4f8:1:2::7/128".into()])
}

#[test]
fn the_address_table_refuses_every_local_range_and_passes_public_ones() {
    // One address inside every range of Linux no-lan's deny list.
    let in_deny = [
        "0.1.2.3", "10.1.2.3", "100.64.0.1", "100.127.255.254", "169.254.169.254", "172.16.0.1", "172.31.255.255", "192.0.0.8", "192.168.1.1", "198.18.0.1", "198.19.255.1",
        "224.0.0.251", "239.255.255.250", "240.0.0.1", "255.255.255.255", "2002:c0a8:101::1", "64:ff9b::c0a8:101", "64:ff9b:1::1", "fc00::1", "fd00::1", "fe80::1", "fec0::1", "ff02::fb",
    ];
    for a in in_deny {
        let a = ip(a);
        assert!(crate::admin::jobs::NO_LAN_DENY.iter().filter_map(|s| Prefix::parse(s)).any(|p| p.contains(&a)), "{a} is in no deny range: fix the table");
        assert!(deny_reason(&a, &[]).is_some(), "{a} must be refused");
    }
    for a in ["127.0.0.1", "127.255.255.254", "::1"] {
        assert_eq!(deny_reason(&ip(a), &[]), Some("loopback address"), "{a}");
    }
    // IPv6 forms of private IPv4: mapped, compatible, 6to4, NAT64.
    for a in ["::ffff:10.0.0.1", "::ffff:127.0.0.1", "::10.0.0.1", "2002:c0a8:101::", "64:ff9b::192.168.1.1", "::ffff:192.168.1.1"] {
        assert!(deny_reason(&ip(a), &[]).is_some(), "{a} must be refused");
    }
    assert_eq!(deny_reason(&ip("::ffff:127.0.0.1"), &[]), Some("loopback address"));
    assert_eq!(deny_reason(&ip("::"), &[]), Some("private address"));
    assert_eq!(deny_reason(&ip("0.0.0.0"), &[]), Some("private address"));
    // Documentation ranges, and all of NAT64's /96 (Linux parity) even for a public IPv4 inside.
    for a in ["203.0.113.7", "198.51.100.1", "2001:db8::1", "64:ff9b::808:808"] {
        assert_eq!(deny_reason(&ip(a), &[]), Some("private address"), "{a}");
    }
    // The node's own LAN on public addresses (fake interface list).
    for a in ["81.2.69.7", "81.2.69.200", "2a01:4f8:1:2::7", "2a01:4f8:1:2:abcd::9", "::ffff:81.2.69.9"] {
        assert_eq!(deny_reason(&ip(a), &own()), Some("address on this node's own network"), "{a}");
    }
    for a in ["1.1.1.1", "140.82.112.3", "151.101.0.1", "8.8.8.8", "2606:4700:4700::1111", "2a01:4f8:1:3::1", "81.2.70.1", "::ffff:1.1.1.1"] {
        assert_eq!(deny_reason(&ip(a), &own()), None, "{a} is public");
    }
}

#[test]
fn prefixes_parse_and_match_both_families() {
    let p = Prefix::parse("192.168.0.0/16").unwrap();
    assert!(p.contains(&ip("192.168.200.1")) && !p.contains(&ip("192.169.0.1")) && !p.contains(&ip("fe80::1")));
    assert!(p.contains(&ip("::ffff:192.168.3.4")), "a mapped address is its IPv4 address");
    let p6 = Prefix::parse("fe80::/10").unwrap();
    assert!(p6.contains(&ip("febf::1")) && !p6.contains(&ip("fec0::1")));
    assert!(Prefix::parse("0.0.0.0/0").unwrap().contains(&ip("8.8.8.8")));
    for bad in ["10.0.0.0", "10.0.0.0/33", "fe80::/129", "x/8", "10.0.0.0/-1"] {
        assert_eq!(Prefix::parse(bad), None, "{bad}");
    }
    assert_eq!(prefixes(&["10.0.0.0/8".into(), "nope".into()]).len(), 1);
}

#[test]
fn connect_heads_parse_names_and_both_address_families() {
    let ok = |h: &str| parse_head(h).unwrap_or_else(|e| panic!("{h}: {e:?}"));
    assert_eq!(ok("CONNECT github.com:443 HTTP/1.1\r\nHost: github.com:443"), Target { host: "github.com".into(), port: 443, ip: None });
    assert_eq!(ok("CONNECT GitHub.COM.:22 HTTP/1.0"), Target { host: "github.com".into(), port: 22, ip: None });
    assert_eq!(ok("connect static.crates.io:443 HTTP/1.1"), Target { host: "static.crates.io".into(), port: 443, ip: None }, "lowercase method");
    assert_eq!(ok("CONNECT 1.2.3.4:443 HTTP/1.1"), Target { host: "1.2.3.4".into(), port: 443, ip: Some(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))) });
    let v6: Ipv6Addr = "2606:4700::1".parse().unwrap();
    assert_eq!(ok("CONNECT [2606:4700::1]:443 HTTP/1.1"), Target { host: "2606:4700::1".into(), port: 443, ip: Some(IpAddr::V6(v6)) });
    assert_eq!(ok("CONNECT a:65535 HTTP/1.1").port, 65535);
    let refused = |h: &str| parse_head(h).err().unwrap_or_else(|| panic!("{h} parsed"));
    for h in [
        "CONNECT github.com HTTP/1.1",
        "CONNECT github.com:0 HTTP/1.1",
        "CONNECT github.com:65536 HTTP/1.1",
        "CONNECT github.com:x HTTP/1.1",
        "CONNECT [2606:4700::1] HTTP/1.1",
        "CONNECT [nothex]:443 HTTP/1.1",
        "CONNECT 2606:4700::1:443 HTTP/1.1",
        "CONNECT git hub.com:443 HTTP/1.1",
        "CONNECT a..b:443 HTTP/1.1",
        "CONNECT a/b:443 HTTP/1.1",
        "CONNECT :443 HTTP/1.1",
        "CONNECT github.com:443 HTTP/2",
        "CONNECT github.com:443",
        "GET / HTTP/1.1",
        "GET https://github.com/ HTTP/1.1",
        "",
        "C0NNECT github.com:443 HTTP/1.1",
    ] {
        let r = refused(h);
        assert_eq!((r.refusal.status, r.refusal.code), (400, codes::BAD_REQUEST), "{h}");
    }
    let long = format!("CONNECT {}.com:443 HTTP/1.1", "a".repeat(300));
    assert_eq!(refused(&long).refusal.code, codes::BAD_REQUEST);
    // Plain HTTP: 403 with the URL's host, never a lookup.
    for (h, host, port) in [
        ("GET http://example.com/x?y HTTP/1.1", "example.com", 80),
        ("POST HTTP://Example.com:8080/ HTTP/1.1", "example.com", 8080),
        ("GET http://user:pw@10.0.0.1/ HTTP/1.1", "10.0.0.1", 80),
        ("GET http://[fd00::1]:81/ HTTP/1.1", "fd00::1", 81),
    ] {
        let r = refused(h);
        assert_eq!((r.refusal.status, r.refusal.code), (403, codes::PLAIN_HTTP), "{h}");
        let t = r.target.unwrap();
        assert_eq!((t.host.as_str(), t.port), (host, port), "{h}");
    }
}

#[test]
fn local_names_are_known_without_a_lookup() {
    for n in ["localhost", "LOCALHOST.", "foo.localhost", "nas", "printer.local", "gitlab.office.lan", "router.home.arpa", "db.internal", "box.localdomain", "x.LOCAL."] {
        assert!(local_name(n), "{n}");
    }
    for n in ["github.com", "static.crates.io", "local.example.com", "internal.example.org", "lan.party"] {
        assert!(!local_name(n), "{n}");
    }
}

/// A resolver that answers from a script, one answer per call (the last
/// one repeats), and counts its calls.
fn scripted(answers: BTreeMap<&'static str, Vec<Vec<&'static str>>>) -> (Resolve, Arc<AtomicU32>) {
    let calls = Arc::new(AtomicU32::new(0));
    let c = calls.clone();
    let r: Resolve = Arc::new(move |host: &str| {
        let n = c.fetch_add(1, Ordering::SeqCst) as usize;
        let seq = answers.get(host).ok_or_else(|| format!("no such host {host}"))?;
        Ok(seq[n.min(seq.len() - 1)].iter().map(|a| a.parse().unwrap()).collect())
    });
    (r, calls)
}

fn no_lan() -> OwnLan {
    Arc::new(Vec::new)
}

fn target(host: &str, port: u16) -> Target {
    parse_head(&format!("CONNECT {} HTTP/1.1", hostport(host, port))).unwrap()
}

#[tokio::test]
async fn local_names_are_refused_before_any_lookup() {
    let (r, calls) = scripted(BTreeMap::from([("nas.local", vec![vec!["1.1.1.1"]])]));
    let p = Policy::new(r, no_lan());
    for n in ["nas.local", "localhost", "intranet", "gitlab.office.lan"] {
        let e = p.decide(&target(n, 443)).await.unwrap_err();
        assert_eq!((e.status, e.code, e.addr), (403, codes::LOCAL_NAME, None), "{n}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no local name was ever looked up");
}

#[tokio::test]
async fn one_lookup_and_only_its_vetted_addresses() {
    let (r, calls) = scripted(BTreeMap::from([
        ("rebind.example.com", vec![vec!["1.1.1.1"], vec!["192.168.1.20"]]),
        ("mixed.example.com", vec![vec!["1.1.1.1", "10.0.0.5"]]),
        ("v6lan.example.com", vec![vec!["2606:4700::1", "fe80::1"]]),
        ("ours.example.com", vec![vec!["81.2.69.50"]]),
        ("empty.example.com", vec![vec![]]),
    ]));
    let p = Policy::new(r, Arc::new(|| vec!["81.2.69.0/24".to_string()]));
    let got = p.decide(&target("rebind.example.com", 443)).await.unwrap();
    assert_eq!(got, vec!["1.1.1.1:443".parse().unwrap()], "the first answer, as checked");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one lookup per request: the dial uses the checked answer");
    // The next request looks up again and gets the private answer: refused.
    let e = p.decide(&target("rebind.example.com", 443)).await.unwrap_err();
    assert_eq!((e.code, e.addr), (codes::PRIVATE, Some(ip("192.168.1.20"))));
    let e = p.decide(&target("mixed.example.com", 443)).await.unwrap_err();
    assert_eq!((e.status, e.code, e.addr), (403, codes::PRIVATE, Some(ip("10.0.0.5"))), "one private address refuses the name");
    let e = p.decide(&target("v6lan.example.com", 443)).await.unwrap_err();
    assert_eq!(e.addr, Some(ip("fe80::1")));
    let e = p.decide(&target("ours.example.com", 443)).await.unwrap_err();
    assert_eq!((e.code, e.why.as_str()), (codes::PRIVATE, "address on this node's own network"));
    let e = p.decide(&target("empty.example.com", 443)).await.unwrap_err();
    assert_eq!((e.status, e.code), (502, codes::DNS_FAILED));
    let e = p.decide(&target("nosuch.example.com", 443)).await.unwrap_err();
    assert_eq!((e.status, e.code), (502, codes::DNS_FAILED));
    assert!(e.why.contains("does this node have internet?"), "{}", e.why);
    // Literals: no lookup, same check.
    let before = calls.load(Ordering::SeqCst);
    assert_eq!(p.decide(&target("8.8.8.8", 53)).await.unwrap(), vec!["8.8.8.8:53".parse().unwrap()], "any port");
    for a in ["192.168.1.1", "10.0.0.1", "127.0.0.1", "::1", "fd00::1", "81.2.69.9", "169.254.169.254"] {
        let e = p.decide(&target(a, 443)).await.unwrap_err();
        assert_eq!((e.code, e.addr), (codes::PRIVATE, Some(ip(a))), "{a}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), before, "an address literal is never looked up");
}

#[test]
fn refusal_answers_carry_the_code_and_one_line() {
    let t = target("gitlab.office.lan", 443);
    let r = Refusal { status: 403, code: codes::PRIVATE, addr: Some(ip("192.168.1.20")), why: "private address".into() };
    let b = String::from_utf8(refusal_bytes(Some(&t), &r)).unwrap();
    let (head, body) = b.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{head}");
    assert!(head.contains("\r\nX-K2-Refused: private_address\r\n") && head.contains("Connection: close"));
    assert!(head.contains(&format!("Content-Length: {}", body.len())));
    assert_eq!(body, "k2-node no-lan: gitlab.office.lan:443 resolves to 192.168.1.20 (private address); this server's jobs on this node reach the internet only\n");
    let lit = target("10.0.0.5", 22);
    let r2 = Refusal { addr: Some(ip("10.0.0.5")), ..r.clone() };
    assert_eq!(refusal_body(Some(&lit), &r2), "k2-node no-lan: 10.0.0.5:22 is refused (private address); this server's jobs on this node reach the internet only");
    let plain = Refusal { status: 403, code: codes::PLAIN_HTTP, addr: None, why: "plain HTTP isn't carried; use https".into() };
    let pt = Target { host: "example.com".into(), port: 80, ip: None };
    assert_eq!(refusal_body(Some(&pt), &plain), "k2-node no-lan: http://example.com/ isn't carried (plain HTTP); use https");
    let dns = Refusal { status: 502, code: codes::DNS_FAILED, addr: None, why: "DNS lookup failed".into() };
    assert!(String::from_utf8(refusal_bytes(Some(&t), &dns)).unwrap().starts_with("HTTP/1.1 502 Bad Gateway\r\nX-K2-Refused: dns_failed\r\n"));
}

#[test]
fn the_log_is_capped_and_says_how_much_it_dropped() {
    let d = crate::util::temp_dir("egress-log");
    let path = d.join("egress.jsonl");
    let line_len = serde_json::to_vec(&Decision::new("open", "github.com", 443)).unwrap().len() as u64 + 1;
    let log = Log::open(&path, line_len * 3).unwrap();
    for _ in 0..10 {
        log.write(&Decision::new("open", "github.com", 443));
    }
    log.finish();
    let lines = fold::parse_lines(&std::fs::read_to_string(&path).unwrap());
    assert_eq!(lines.len(), 4, "three kept, then the dropped line");
    assert_eq!((lines[3].ev.as_str(), lines[3].n), ("dropped", 7));
    // Appending again (a restarted proxy) keeps what was there.
    let again = Log::open(&path, LOG_CAP).unwrap();
    again.write(&Decision::new("open", "crates.io", 443));
    assert_eq!(fold::parse_lines(&std::fs::read_to_string(&path).unwrap()).len(), 5);
    // A link in the job folder is never followed.
    std::os::unix::fs::symlink(d.join("elsewhere"), d.join("link.jsonl")).unwrap();
    assert!(Log::open(&d.join("link.jsonl"), LOG_CAP).is_err());
}

// ── the server, on loopback ─────────────────────────────────────────

/// An upstream that echoes upper-cased bytes, and says hello first.
async fn upstream() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let _ = s.write_all(b"hello\n").await;
                let mut b = [0u8; 4096];
                loop {
                    match s.read(&mut b).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => {
                            let up: Vec<u8> = b[..n].to_ascii_uppercase();
                            if s.write_all(&up).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            });
        }
    });
    a
}

struct Proxy {
    addr: SocketAddr,
    stop: watch::Sender<bool>,
    log_path: std::path::PathBuf,
    done: tokio::task::JoinHandle<()>,
}

impl Proxy {
    fn lines(&self) -> Vec<Decision> {
        fold::parse_lines(&std::fs::read_to_string(&self.log_path).unwrap_or_default())
    }

    async fn wait_lines(&self, n: usize) -> Vec<Decision> {
        let t0 = Instant::now();
        loop {
            let l = self.lines();
            if l.len() >= n {
                return l;
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "only {} of {n} log lines: {l:?}", l.len());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

async fn proxy(policy: Policy) -> Proxy {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let dir = crate::util::temp_dir("egress-srv");
    let log_path = dir.join("egress.jsonl");
    let log = Arc::new(Log::open(&log_path, LOG_CAP).unwrap());
    let (stop, rx) = watch::channel(false);
    let done = tokio::spawn(serve(l, Arc::new(policy), log, rx));
    Proxy { addr, stop, log_path, done }
}

/// Send a CONNECT; returns the stream and the answer head.
async fn connect(p: &Proxy, target: &str) -> (TcpStream, String) {
    let mut s = TcpStream::connect(p.addr).await.unwrap();
    s.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes()).await.unwrap();
    let head = read_answer_head(&mut s).await;
    (s, head)
}

async fn read_answer_head(s: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut b = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut b)).await.expect("an answer within 10 s").unwrap();
        assert!(n > 0, "closed before an answer head: {:?}", String::from_utf8_lossy(&buf));
        buf.push(b[0]);
    }
    String::from_utf8(buf).unwrap()
}

async fn read_n(s: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut b)).await.expect("bytes within 10 s").unwrap();
    b
}

fn upstream_policy(up: SocketAddr, answers: BTreeMap<&'static str, Vec<Vec<&'static str>>>) -> (Policy, Arc<AtomicU32>) {
    let (r, calls) = scripted(answers);
    (Policy::new(r, no_lan()).with_test_public(vec![up]), calls)
}

#[tokio::test]
async fn a_tunnel_carries_bytes_both_ways_and_logs_them() {
    let up = upstream().await;
    let (pol, _) = upstream_policy(up, BTreeMap::from([("upstream.example.com", vec![vec!["127.0.0.1"]])]));
    let p = proxy(pol).await;
    let (mut s, head) = connect(&p, &format!("upstream.example.com:{}", up.port())).await;
    assert_eq!(head, "HTTP/1.1 200 Connection Established\r\n\r\n");
    assert_eq!(read_n(&mut s, 6).await, b"hello\n");
    s.write_all(b"ping").await.unwrap();
    assert_eq!(read_n(&mut s, 4).await, b"PING");
    drop(s);
    p.wait_lines(2).await;
    // An address literal of the admitted upstream too.
    let (mut s2, head) = connect(&p, &format!("127.0.0.1:{}", up.port())).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(read_n(&mut s2, 6).await, b"hello\n");
    drop(s2);
    let _ = p.stop.send(true);
    tokio::time::timeout(Duration::from_secs(10), p.done).await.unwrap().unwrap();
    let lines = fold::parse_lines(&std::fs::read_to_string(&p.log_path).unwrap());
    let evs: Vec<(&str, &str)> = lines.iter().map(|d| (d.ev.as_str(), d.host.as_str())).collect();
    assert_eq!(evs, vec![("open", "upstream.example.com"), ("close", "upstream.example.com"), ("open", "127.0.0.1"), ("close", "127.0.0.1")]);
    assert_eq!((lines[1].bytes_out, lines[1].bytes_in, lines[1].addr.as_deref()), (4, 10, Some("127.0.0.1")));
    let l = fold::fold(&lines);
    assert_eq!((l.rows[0].conns, l.rows[0].bytes_in, l.rows[0].bytes_out), (1, 10, 4));
}

#[tokio::test]
async fn refusals_answer_at_once_and_are_logged_live() {
    let up = upstream().await;
    let (pol, calls) = upstream_policy(up, BTreeMap::from([("gitlab.example.com", vec![vec!["192.168.1.20"]])]));
    let p = proxy(pol).await;
    let (_, head) = connect(&p, "gitlab.example.com:443").await;
    assert!(head.starts_with("HTTP/1.1 403 Forbidden\r\n") && head.contains("X-K2-Refused: private_address"), "{head}");
    let (_, head) = connect(&p, "nas.local:443").await;
    assert!(head.contains("X-K2-Refused: local_name"), "{head}");
    let (_, head) = connect(&p, "10.0.0.5:22").await;
    assert!(head.contains("X-K2-Refused: private_address"), "{head}");
    // A port the test upstream isn't on: 127.0.0.1 stays loopback.
    let (_, head) = connect(&p, &format!("127.0.0.1:{}", up.port() ^ 1)).await;
    assert!(head.contains("X-K2-Refused: private_address"), "{head}");
    let mut s = TcpStream::connect(p.addr).await.unwrap();
    s.write_all(b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n").await.unwrap();
    let head = read_answer_head(&mut s).await;
    assert!(head.starts_with("HTTP/1.1 403") && head.contains("X-K2-Refused: plain_http"), "{head}");
    let mut s = TcpStream::connect(p.addr).await.unwrap();
    s.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
    assert!(read_answer_head(&mut s).await.starts_with("HTTP/1.1 400"));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "only gitlab.example.com was looked up");
    // Lines are in the log before the proxy stops (the node tails them live).
    let lines = p.lines();
    let got: Vec<(String, String, Option<String>)> = lines.iter().map(|d| (d.ev.clone(), d.host.clone(), d.code.clone())).collect();
    assert_eq!(
        got[..5],
        [
            ("refused".to_string(), "gitlab.example.com".to_string(), Some("private_address".to_string())),
            ("refused".into(), "nas.local".into(), Some("local_name".into())),
            ("refused".into(), "10.0.0.5".into(), Some("private_address".into())),
            ("refused".into(), "127.0.0.1".into(), Some("private_address".into())),
            ("refused".into(), "example.com".into(), Some("plain_http".into())),
        ]
    );
    assert_eq!(lines[0].addr.as_deref(), Some("192.168.1.20"));
    assert_eq!(fold::live_line(&lines[0]).unwrap(), "network: refused gitlab.example.com:443 → 192.168.1.20: private address (no-lan keeps jobs off the local network)");
    assert_eq!(fold::live_line(&lines[4]).unwrap(), "network: refused http://example.com/: plain HTTP isn't carried; use https");
    let _ = p.stop.send(true);
}

#[tokio::test]
async fn heads_too_big_or_too_slow_are_refused() {
    let (r, _) = scripted(BTreeMap::new());
    let p = proxy(Policy::new(r, no_lan()).with_timeouts(IDLE, Duration::from_millis(300))).await;
    let mut s = TcpStream::connect(p.addr).await.unwrap();
    let big = format!("CONNECT github.com:443 HTTP/1.1\r\nX: {}\r\n", "a".repeat(HEAD_MAX + 10));
    let _ = s.write_all(big.as_bytes()).await;
    let head = read_answer_head(&mut s).await;
    assert!(head.starts_with("HTTP/1.1 400") && head.contains("bad_request"), "{head}");
    let mut s = TcpStream::connect(p.addr).await.unwrap();
    s.write_all(b"CONNECT github.com:443 HTTP/1.1\r\n").await.unwrap();
    let t0 = Instant::now();
    let head = read_answer_head(&mut s).await;
    assert!(head.starts_with("HTTP/1.1 400"), "{head}");
    assert!(t0.elapsed() >= Duration::from_millis(250), "the head limit, not at once");
    let _ = p.stop.send(true);
}

#[tokio::test]
async fn an_idle_tunnel_closes() {
    let up = upstream().await;
    let (r, _) = scripted(BTreeMap::new());
    let p = proxy(Policy::new(r, no_lan()).with_test_public(vec![up]).with_timeouts(Duration::from_millis(400), HEAD_TIMEOUT)).await;
    let (mut s, head) = connect(&p, &up.to_string()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(read_n(&mut s, 6).await, b"hello\n");
    let mut b = [0u8; 1];
    let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut b)).await.expect("closed by the idle limit").unwrap();
    assert_eq!(n, 0);
    let _ = p.stop.send(true);
}

#[tokio::test]
async fn at_most_128_tunnels_at_once() {
    let up = upstream().await;
    let (r, _) = scripted(BTreeMap::new());
    let p = proxy(Policy::new(r, no_lan()).with_test_public(vec![up])).await;
    let mut open = Vec::new();
    for _ in 0..MAX_TUNNELS {
        let (mut s, head) = connect(&p, &up.to_string()).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(read_n(&mut s, 6).await, b"hello\n");
        open.push(s);
    }
    let (_, head) = connect(&p, &up.to_string()).await;
    assert!(head.starts_with("HTTP/1.1 429") && head.contains("too_many_tunnels"), "{head}");
    open.pop();
    // The slot frees once the proxy sees the close.
    let t0 = Instant::now();
    loop {
        let (_, head) = connect(&p, &up.to_string()).await;
        if head.starts_with("HTTP/1.1 200") {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "a closed tunnel's slot never came back: {head}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = p.stop.send(true);
}

#[tokio::test]
async fn stopping_closes_open_tunnels_with_their_bytes() {
    let up = upstream().await;
    let (r, _) = scripted(BTreeMap::new());
    let p = proxy(Policy::new(r, no_lan()).with_test_public(vec![up])).await;
    let (mut s, _) = connect(&p, &up.to_string()).await;
    assert_eq!(read_n(&mut s, 6).await, b"hello\n");
    s.write_all(b"abc").await.unwrap();
    assert_eq!(read_n(&mut s, 3).await, b"ABC");
    let _ = p.stop.send(true);
    tokio::time::timeout(Duration::from_secs(10), p.done).await.unwrap().unwrap();
    let mut b = [0u8; 1];
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), s.read(&mut b)).await.unwrap().unwrap_or(0), 0, "the tunnel closed with the proxy");
    let close = fold::parse_lines(&std::fs::read_to_string(&p.log_path).unwrap()).into_iter().find(|d| d.ev == "close").expect("a close line");
    assert_eq!((close.bytes_out, close.bytes_in), (3, 9));
}

#[tokio::test]
async fn upstream_failures_say_so() {
    // A port nothing listens on, admitted as public.
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let (r, _) = scripted(BTreeMap::new());
    let p = proxy(Policy::new(r, no_lan()).with_test_public(vec![free])).await;
    let (_, head) = connect(&p, &free.to_string()).await;
    assert!(head.starts_with("HTTP/1.1 502") && head.contains("connect_failed"), "{head}");
    let (_, head) = connect(&p, "nosuch.example.com:443").await;
    assert!(head.starts_with("HTTP/1.1 502") && head.contains("dns_failed"), "{head}");
    let lines = p.lines();
    assert_eq!(lines.iter().map(|d| d.ev.as_str()).collect::<Vec<_>>(), vec!["failed", "failed"]);
    assert!(fold::live_line(&lines[1]).unwrap().starts_with("network: couldn't reach nosuch.example.com:443: DNS lookup failed"));
    let _ = p.stop.send(true);
}

#[test]
fn the_pool_is_just_below_the_job_ports() {
    let ports: Vec<u16> = pool_ports().collect();
    assert_eq!((ports.len(), ports[0], *ports.last().unwrap()), (120, 19880, 19999));
    assert!(*ports.last().unwrap() < k2_node_proto::env::JOB_PORT_FIRST);
}
