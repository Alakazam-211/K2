//! front443 router tests: the ClientHello peek against real captured
//! hellos, PROXY v2 golden bytes (checked with Sew's own parser), and the
//! front end to end on loopback (routing, closes, slowloris, the cap).
//!
//! Fixtures (`fixtures/*.bin`) are real ClientHellos captured 2026-10-17
//! on macOS 27.0.1 by a listener that recorded what each client sent:
//! - `chrome-pq.bin`: Chrome 155 headless, X25519MLKEM768 key share
//!   (2019 bytes: over one Ethernet MSS), SNI `turn-box.k2.dev`;
//! - `firefox.bin`: Firefox 136 headless, PQ key share, SNI `localhost`;
//! - `apple-urlsession.bin`: Apple's URLSession / Network.framework (the
//!   TLS stack Safari 27 uses), SNI `localhost`;
//! - `curl-macos.bin`: curl 8.7.1 (LibreSSL 3.3.6), SNI `turn-box.k2.dev`;
//! - `openssl-pq.bin`: OpenSSL 3.6 `s_client -groups X25519MLKEM768`, SNI
//!   `skin.acme.dev`;
//! - `openssl-tls12.bin`: OpenSSL `-tls1_2`, SNI `mail.acme.dev`;
//! - `openssl-nosni.bin`: OpenSSL `-noservername` (no SNI).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::client_hello::{host_name, peek, Peek, MAX_HELLO_BYTES};
use super::proxy_v2;
use super::routes::{Backend, Kind, RouteTable};
use super::server::{accept_loop, Front};

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/front443/fixtures").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

const FIXTURES: [(&str, Option<&str>); 7] = [
    ("chrome-pq.bin", Some("turn-box.k2.dev")),
    ("firefox.bin", Some("localhost")),
    ("apple-urlsession.bin", Some("localhost")),
    ("curl-macos.bin", Some("turn-box.k2.dev")),
    ("openssl-pq.bin", Some("skin.acme.dev")),
    ("openssl-tls12.bin", Some("mail.acme.dev")),
    ("openssl-nosni.bin", None),
];

fn done(p: Peek) -> super::client_hello::Hello {
    match p {
        Peek::Done(h) => h,
        other => panic!("expected Done, got {other:?}"),
    }
}

// ── ClientHello peek ────────────────────────────────────────────────────

#[test]
fn every_captured_client_hello_yields_its_server_name() {
    for (name, sni) in FIXTURES {
        let b = fixture(name);
        let h = done(peek(&b));
        assert_eq!(h.sni.as_deref(), sni, "{name}");
        assert_eq!(h.consumed, b.len(), "{name}: the whole capture is the ClientHello");
        assert_eq!(h.records, 1, "{name}");
    }
    // The post-quantum ones really are multi-segment sized, and carry the
    // X25519MLKEM768 group (0x11ec).
    for name in ["chrome-pq.bin", "firefox.bin", "openssl-pq.bin"] {
        let b = fixture(name);
        assert!(b.len() > 1460, "{name} is {} bytes", b.len());
        assert!(b.windows(2).any(|w| w == [0x11, 0xec]), "{name}: no X25519MLKEM768");
    }
    // ALPN is read too (Chrome offers h2 + http/1.1).
    assert_eq!(done(peek(&fixture("chrome-pq.bin"))).alpn, vec!["h2".to_string(), "http/1.1".to_string()]);
}

/// Every prefix short of the full hello is NeedMore (a hello split at ANY
/// byte across TCP segments), and the full one is Done.
#[test]
fn a_hello_split_at_every_byte_needs_more_until_complete() {
    for (name, sni) in FIXTURES {
        let b = fixture(name);
        for cut in 0..b.len() {
            assert_eq!(peek(&b[..cut]), Peek::NeedMore, "{name} cut at {cut}");
        }
        assert_eq!(done(peek(&b)).sni.as_deref(), sni, "{name}");
    }
}

/// Re-frame a single-record hello's handshake bytes into records of at
/// most `size` bytes (RFC 8446 §5.1 lets a sender fragment a handshake
/// message across records).
fn refragment(b: &[u8], size: usize) -> Vec<u8> {
    assert_eq!(b[0], 0x16);
    let len = u16::from_be_bytes([b[3], b[4]]) as usize;
    let hs = &b[5..5 + len];
    let mut out = Vec::new();
    for chunk in hs.chunks(size) {
        out.extend_from_slice(&[0x16, b[1], b[2]]);
        out.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
        out.extend_from_slice(chunk);
    }
    out
}

#[test]
fn a_hello_fragmented_across_many_records_is_reassembled() {
    for (name, sni) in FIXTURES {
        let b = fixture(name);
        for size in [1usize, 3, 4, 5, 64, 512, 1000] {
            let f = refragment(&b, size);
            let h = done(peek(&f));
            assert_eq!(h.sni.as_deref(), sni, "{name} in {size}-byte records");
            assert_eq!(h.records, (b.len() - 5).div_ceil(size), "{name} {size}");
            assert_eq!(h.consumed, f.len());
            // ...and split mid-record as well.
            for cut in [1, 6, f.len() / 2, f.len() - 1] {
                assert_eq!(peek(&f[..cut]), Peek::NeedMore, "{name} {size} cut {cut}");
            }
        }
    }
    // 1-byte records of a 2 KB hello: ~10 KB of records, still inside the cap.
    assert!(refragment(&fixture("chrome-pq.bin"), 1).len() <= MAX_HELLO_BYTES);
}

/// A minimal ClientHello with the given extensions block.
fn hello_with(exts: &[u8]) -> Vec<u8> {
    let mut body = vec![0x03, 0x03];
    body.extend_from_slice(&[0x11; 32]); // random
    body.push(0); // session id
    body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]); // one suite
    body.extend_from_slice(&[0x01, 0x00]); // compression: null
    body.extend_from_slice(&(exts.len() as u16).to_be_bytes());
    body.extend_from_slice(exts);
    let mut hs = vec![0x01];
    hs.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    hs.extend_from_slice(&body);
    let mut rec = vec![0x16, 0x03, 0x01];
    rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
    rec.extend_from_slice(&hs);
    rec
}

fn sni_ext(names: &[(u8, &[u8])]) -> Vec<u8> {
    let mut list = Vec::new();
    for (t, n) in names {
        list.push(*t);
        list.extend_from_slice(&(n.len() as u16).to_be_bytes());
        list.extend_from_slice(n);
    }
    let mut data = (list.len() as u16).to_be_bytes().to_vec();
    data.extend_from_slice(&list);
    let mut e = vec![0x00, 0x00];
    e.extend_from_slice(&(data.len() as u16).to_be_bytes());
    e.extend_from_slice(&data);
    e
}

#[test]
fn synthetic_hellos_hold_the_rules() {
    // Upper case is lowercased for routing.
    assert_eq!(done(peek(&hello_with(&sni_ext(&[(0, b"Turn-Box.K2.dev")])))).sni.as_deref(), Some("turn-box.k2.dev"));
    // Unknown name types are skipped.
    assert_eq!(done(peek(&hello_with(&sni_ext(&[(7, b"x"), (0, b"a.b")])))).sni.as_deref(), Some("a.b"));
    // No extensions at all (an old TLS 1.2 client): no SNI.
    let mut bare = hello_with(&[]);
    // drop the empty extensions length: legal pre-extension hello
    let hs_len = bare.len() - 5 - 2;
    bare.truncate(bare.len() - 2);
    bare[3..5].copy_from_slice(&(hs_len as u16).to_be_bytes());
    bare[6..9].copy_from_slice(&((hs_len - 4) as u32).to_be_bytes()[1..]);
    assert_eq!(done(peek(&bare)).sni, None);

    let bad: Vec<(&str, Vec<u8>)> = vec![
        ("two host names", hello_with(&sni_ext(&[(0, b"a.b"), (0, b"c.d")]))),
        ("space in name", hello_with(&sni_ext(&[(0, b"a b.c")]))),
        ("trailing dot", hello_with(&sni_ext(&[(0, b"a.b.")]))),
        ("empty label", hello_with(&sni_ext(&[(0, b"a..b")]))),
        ("NUL in name", hello_with(&sni_ext(&[(0, b"a\0.b")]))),
        ("empty name", hello_with(&sni_ext(&[(0, b"")]))),
        ("repeated extension", hello_with(&[sni_ext(&[(0, b"a.b")]), sni_ext(&[(0, b"c.d")])].concat())),
        ("empty server_name ext", hello_with(&[0x00, 0x00, 0x00, 0x00])),
        ("ext length overruns", hello_with(&[0x00, 0x00, 0x00, 0x09, 0x00])),
    ];
    for (why, b) in bad {
        assert!(matches!(peek(&b), Peek::Bad(_)), "{why}: {:?}", peek(&b));
    }
    // A record that isn't a handshake before the hello completed.
    let f = refragment(&fixture("curl-macos.bin"), 100);
    let mut mixed = f[..105].to_vec();
    mixed.extend_from_slice(&[0x17, 0x03, 0x03, 0x00, 0x01, 0x00]);
    assert!(matches!(peek(&mixed), Peek::Bad(_)));
    // Zero-length and over-long records.
    assert!(matches!(peek(&[0x16, 0x03, 0x01, 0x00, 0x00]), Peek::Bad(_)));
    assert!(matches!(peek(&[0x16, 0x03, 0x01, 0x40, 0x01]), Peek::Bad(_)));
    // A handshake that claims more than the front reads.
    assert!(matches!(peek(&[0x16, 0x03, 0x01, 0x00, 0x04, 0x01, 0x00, 0x50, 0x00]), Peek::Bad(_)));
    // Not a ClientHello (a ServerHello).
    let mut sh = hello_with(&sni_ext(&[(0, b"a.b")]));
    sh[5] = 0x02;
    assert!(matches!(peek(&sh), Peek::Bad(_)));
    // Not TLS at all.
    for not in [&b"GET / HTTP/1.1\r\n"[..], b"SSH-2.0-OpenSSH_9.6\r\n", &proxy_v2::local_header()] {
        assert_eq!(peek(not), Peek::NotTls);
    }
}

#[test]
fn host_names_are_strict_dns() {
    assert_eq!(host_name(b"turn-box.k2.dev").as_deref(), Some("turn-box.k2.dev"));
    assert_eq!(host_name(b"_acme.example").as_deref(), Some("_acme.example"));
    for bad in [&b""[..], b".", b"a.", b".a", b"a..b", b"a b", b"\xc3\xa9.example", &[b'a'; 64], b"a/b", b"a:443"] {
        assert_eq!(host_name(bad), None, "{:?}", String::from_utf8_lossy(bad));
    }
    let long = format!("{}.x", vec!["a".repeat(63); 4].join("."));
    assert_eq!(host_name(long.as_bytes()), None, "254 bytes");
}

// ── PROXY v2 ────────────────────────────────────────────────────────────

/// Sew's v2 parser, copied verbatim (logic) from Sew main `047667cc`
/// `crates/sew-turn/src/stream/proxy.rs` `parse_v2`, so the golden bytes
/// are checked against the parser on the other end of the splice.
mod sew_parser {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    pub const V2_SIGNATURE: [u8; 12] = [0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A];
    pub const V2_MAX: usize = 512;
    #[derive(Debug, PartialEq)]
    pub enum ProxyParse {
        NeedMore,
        Done { client: Option<SocketAddr>, consumed: usize },
        Bad,
    }
    pub fn parse_v2(buf: &[u8]) -> ProxyParse {
        let n = buf.len().min(12);
        if buf[..n] != V2_SIGNATURE[..n] {
            return ProxyParse::Bad;
        }
        if buf.len() < 16 {
            return ProxyParse::NeedMore;
        }
        let ver_cmd = buf[12];
        if ver_cmd >> 4 != 2 {
            return ProxyParse::Bad;
        }
        let cmd = ver_cmd & 0x0F;
        let fam = buf[13];
        let len = usize::from(u16::from_be_bytes([buf[14], buf[15]]));
        let total = 16 + len;
        if total > V2_MAX {
            return ProxyParse::Bad;
        }
        if buf.len() < total {
            return ProxyParse::NeedMore;
        }
        let body = &buf[16..total];
        match cmd {
            0x0 => ProxyParse::Done { client: None, consumed: total },
            0x1 => {
                let client = match fam {
                    0x11 if body.len() >= 12 => {
                        let ip = Ipv4Addr::new(body[0], body[1], body[2], body[3]);
                        let port = u16::from_be_bytes([body[8], body[9]]);
                        SocketAddr::new(IpAddr::V4(ip), port)
                    }
                    0x21 if body.len() >= 36 => {
                        let mut o = [0u8; 16];
                        o.copy_from_slice(&body[..16]);
                        let port = u16::from_be_bytes([body[32], body[33]]);
                        SocketAddr::new(IpAddr::V6(Ipv6Addr::from(o)), port)
                    }
                    _ => return ProxyParse::Bad,
                };
                ProxyParse::Done { client: Some(client), consumed: total }
            }
            _ => ProxyParse::Bad,
        }
    }
}

#[test]
fn proxy_v2_golden_bytes() {
    let src: SocketAddr = "203.0.113.7:51234".parse().unwrap();
    let dst: SocketAddr = "198.51.100.1:443".parse().unwrap();
    let h = proxy_v2::header(src, dst);
    #[rustfmt::skip]
    let want: Vec<u8> = vec![
        0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A, // signature
        0x21,                   // v2, PROXY
        0x11,                   // TCP over IPv4
        0x00, 0x0C,             // 12 address bytes
        203, 0, 113, 7,         // source
        198, 51, 100, 1,        // destination
        0xC8, 0x22,             // 51234
        0x01, 0xBB,             // 443
    ];
    assert_eq!(h, want);

    let src6: SocketAddr = "[2001:db8::7]:40000".parse().unwrap();
    let dst6: SocketAddr = "[2001:db8::1]:443".parse().unwrap();
    let h6 = proxy_v2::header(src6, dst6);
    let mut want6 = want[..12].to_vec();
    want6.extend_from_slice(&[0x21, 0x21, 0x00, 0x24]);
    want6.extend_from_slice(&"2001:db8::7".parse::<std::net::Ipv6Addr>().unwrap().octets());
    want6.extend_from_slice(&"2001:db8::1".parse::<std::net::Ipv6Addr>().unwrap().octets());
    want6.extend_from_slice(&[0x9C, 0x40, 0x01, 0xBB]);
    assert_eq!(h6, want6);
    assert_eq!(h6.len(), 52);

    // A v4 client on the dual-stack [::]:443 socket arrives v4-mapped: TCP4.
    let mapped: SocketAddr = "[::ffff:203.0.113.7]:51234".parse().unwrap();
    let mapped_dst: SocketAddr = "[::ffff:198.51.100.1]:443".parse().unwrap();
    assert_eq!(proxy_v2::header(mapped, mapped_dst), want);
    // Mixed families: both as IPv6.
    let mixed = proxy_v2::header(src, dst6);
    assert_eq!(&mixed[12..16], &[0x21, 0x21, 0x00, 0x24]);

    // Sew's parser reads exactly the header and the real client.
    for (h, client) in [(&want, src), (&want6, src6), (&mixed, "[::ffff:203.0.113.7]:51234".parse().unwrap())] {
        let mut stream = h.clone();
        stream.extend_from_slice(&fixture("chrome-pq.bin"));
        assert_eq!(sew_parser::parse_v2(&stream), sew_parser::ProxyParse::Done { client: Some(client), consumed: h.len() });
        for cut in 0..h.len() {
            assert_eq!(sew_parser::parse_v2(&h[..cut.max(1)]), sew_parser::ProxyParse::NeedMore, "cut {cut}");
        }
    }
    let local = proxy_v2::local_header();
    assert_eq!(local, [&want[..12], &[0x20, 0x00, 0x00, 0x00][..]].concat());
    assert_eq!(sew_parser::parse_v2(&local), sew_parser::ProxyParse::Done { client: None, consumed: 16 });
}

// ── route table ─────────────────────────────────────────────────────────

fn be(kind: Kind, addr: SocketAddr, proxy: bool) -> Backend {
    Backend { kind, addr, proxy, refuse_loopback_clients: kind == Kind::Stalwart }
}

#[test]
fn the_table_takes_only_loopback_backends_and_valid_names() {
    let lo: SocketAddr = "127.0.0.1:5349".parse().unwrap();
    let mut t = RouteTable::default();
    t.insert("Turn-Box.K2.dev", be(Kind::Turn, lo, true)).unwrap();
    assert!(t.lookup("turn-box.k2.dev").is_some(), "stored lowercase");
    assert!(t.insert("x.example", be(Kind::Caddy, "10.0.0.5:443".parse().unwrap(), false)).is_err(), "never off-box");
    assert!(t.insert("x.example", be(Kind::Caddy, "[::1]:38473".parse().unwrap(), false)).is_ok());
    assert!(t.insert("bad name", be(Kind::Caddy, lo, false)).is_err());
    assert!(t.insert("turn-box.k2.dev", be(Kind::Stalwart, "127.0.0.1:8443".parse().unwrap(), true)).is_err(), "first route wins, the clash is reported");
    assert!(t.insert("turn-box.k2.dev", be(Kind::Turn, lo, true)).is_ok(), "the same route again is fine");
    assert_eq!(t.len(), 2);

    // The persisted copy is re-validated: a doctored entry pointing off the
    // box is dropped on load.
    let mut text = serde_json::to_value(&t).unwrap();
    text["hosts"]["evil.example"] = serde_json::json!({"kind": "caddy", "addr": "192.0.2.1:443", "proxy": false, "refuseLoopbackClients": false});
    let back = super::routes::parse_persisted(&text.to_string()).unwrap();
    assert_eq!(back, t);
    assert!(super::routes::parse_persisted("not json").is_none());
}

// ── the front on loopback ───────────────────────────────────────────────

struct Rig {
    addr: SocketAddr,
    front: Arc<Front>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn rig(cap: usize, hello: Duration, table: RouteTable) -> Rig {
    let table = Arc::new(table);
    let front = Front::with_timeouts(cap, hello, Duration::from_secs(2), Box::new(move || Arc::clone(&table)));
    let l = Arc::new(TcpListener::bind("127.0.0.1:0").await.unwrap());
    let addr = l.local_addr().unwrap();
    let task = tokio::spawn(accept_loop(l, Arc::clone(&front), tokio::runtime::Handle::current()));
    Rig { addr, front, task }
}

/// A backend that records the first `n` bytes each connection sends, then
/// answers `pong` and keeps the connection open until the peer closes.
async fn backend() -> (SocketAddr, tokio::sync::mpsc::UnboundedReceiver<(SocketAddr, Vec<u8>)>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, peer)) = l.accept().await else { return };
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut got = Vec::new();
                let mut buf = [0u8; 4096];
                // Read until the peer pauses (the front writes header +
                // hello in one go).
                while let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(200), s.read(&mut buf)).await {
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                }
                let _ = tx.send((peer, got));
                let _ = s.write_all(b"pong").await;
                let mut sink = [0u8; 64];
                while let Ok(n) = s.read(&mut sink).await {
                    if n == 0 {
                        break;
                    }
                }
            });
        }
    });
    (addr, rx)
}

/// Read until EOF (or 3 s): what the front sent back before closing.
async fn drain(c: &mut TcpStream) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        match tokio::time::timeout(Duration::from_secs(3), c.read(&mut buf)).await {
            Ok(Ok(0)) | Ok(Err(_)) => return out,
            Ok(Ok(n)) => out.extend_from_slice(&buf[..n]),
            Err(_) => panic!("the front kept the connection open"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_routed_name_gets_proxy_v2_then_the_untouched_client_bytes() {
    let (be_addr, mut rx) = backend().await;
    let mut t = RouteTable::default();
    t.insert("turn-box.k2.dev", be(Kind::Turn, be_addr, true)).unwrap();
    t.insert("skin.acme.dev", be(Kind::Caddy, be_addr, false)).unwrap();
    let r = rig(16, Duration::from_secs(5), t).await;

    // Chrome's post-quantum hello, in three segments with pauses.
    let hello = fixture("chrome-pq.bin");
    let mut c = TcpStream::connect(r.addr).await.unwrap();
    c.set_nodelay(true).unwrap();
    let client_addr = c.local_addr().unwrap();
    for part in [&hello[..700], &hello[700..1400], &hello[1400..]] {
        c.write_all(part).await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    let (_peer, got) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
    let header = proxy_v2::header(client_addr, r.addr);
    assert_eq!(&got[..header.len()], &header[..], "PROXY v2 first");
    assert_eq!(&got[header.len()..], &hello[..], "then the ClientHello, byte for byte");
    assert_eq!(sew_parser::parse_v2(&got), sew_parser::ProxyParse::Done { client: Some(client_addr), consumed: header.len() });
    // Both directions splice.
    let mut pong = [0u8; 4];
    c.read_exact(&mut pong).await.unwrap();
    assert_eq!(&pong, b"pong");

    // A route without PROXY: the backend sees only the client's bytes.
    let pq = fixture("openssl-pq.bin");
    let mut c2 = TcpStream::connect(r.addr).await.unwrap();
    c2.write_all(&pq).await.unwrap();
    let (_p, got2) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
    assert_eq!(got2, pq);
    let c = &r.front.counters;
    assert_eq!(c.turn.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(c.caddy.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_names_no_sni_non_tls_and_loopback_mail_clients_are_closed_without_a_byte() {
    use std::sync::atomic::Ordering::Relaxed;
    let (be_addr, mut rx) = backend().await;
    let mut t = RouteTable::default();
    t.insert("skin.acme.dev", be(Kind::Caddy, be_addr, true)).unwrap();
    // Stalwart's route refuses loopback clients (the test client is one).
    t.insert("mail.acme.dev", be(Kind::Stalwart, be_addr, true)).unwrap();
    // A route whose backend is down.
    t.insert("localhost", be(Kind::Caddy, "127.0.0.1:9".parse().unwrap(), true)).unwrap();
    let r = rig(16, Duration::from_secs(5), t).await;

    let cases: [(&str, Vec<u8>); 5] = [
        ("unknown SNI", fixture("chrome-pq.bin")), // turn-box.k2.dev is not routed here
        ("no SNI", fixture("openssl-nosni.bin")),
        ("not TLS", b"GET / HTTP/1.1\r\nHost: skin.acme.dev\r\n\r\n".to_vec()),
        ("loopback mail client", fixture("openssl-tls12.bin")),
        ("backend down", fixture("firefox.bin")),
    ];
    for (why, bytes) in cases {
        let mut c = TcpStream::connect(r.addr).await.unwrap();
        c.write_all(&bytes).await.unwrap();
        assert!(drain(&mut c).await.is_empty(), "{why}: the front wrote nothing back");
    }
    assert!(tokio::time::timeout(Duration::from_millis(600), rx.recv()).await.is_err(), "no backend saw a connection");
    let k = &r.front.counters;
    assert_eq!(k.unknown_sni.load(Relaxed), 1);
    assert_eq!(k.no_sni.load(Relaxed), 1);
    assert_eq!(k.not_tls.load(Relaxed), 1);
    assert_eq!(k.loopback_refused.load(Relaxed), 1);
    assert_eq!(k.backend_failed.load(Relaxed), 1);
    assert_eq!(k.stalwart.load(Relaxed), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slowloris_hello_is_cut_at_the_deadline_not_reset_per_byte() {
    use std::sync::atomic::Ordering::Relaxed;
    let (be_addr, mut rx) = backend().await;
    let mut t = RouteTable::default();
    t.insert("turn-box.k2.dev", be(Kind::Turn, be_addr, true)).unwrap();
    let r = rig(16, Duration::from_millis(800), t).await;
    let hello = fixture("chrome-pq.bin");

    // Stalls after 10 bytes.
    let mut a = TcpStream::connect(r.addr).await.unwrap();
    a.write_all(&hello[..10]).await.unwrap();
    // Dribbles a byte every 100 ms (would never finish within the deadline).
    let mut b = TcpStream::connect(r.addr).await.unwrap();
    let start = std::time::Instant::now();
    let dribble = tokio::spawn(async move {
        for byte in hello.iter() {
            if b.write_all(&[*byte]).await.is_err() {
                return b;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        b
    });
    assert!(drain(&mut a).await.is_empty());
    let mut b = dribble.await.unwrap();
    assert!(drain(&mut b).await.is_empty());
    let took = start.elapsed();
    assert!(took < Duration::from_secs(3), "cut at ~0.8 s, took {took:?}");
    assert_eq!(r.front.counters.hello_timeout.load(Relaxed), 2);
    assert!(tokio::time::timeout(Duration::from_millis(300), rx.recv()).await.is_err());
}

/// The front's own cap: 2,000 spliced connections held open, the 2,001st
/// is closed at once, and a freed slot is usable again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_thousand_connections_fill_the_cap_and_the_next_is_refused() {
    use std::sync::atomic::Ordering::Relaxed;
    const CAP: usize = 2000;
    k2_core::raise_nofile_limit();
    let soft = k2_core::nofile_soft_limit().expect("RLIMIT_NOFILE");
    // client + front-accepted + front-to-backend + backend-accepted, per
    // connection, plus slack.
    assert!(soft >= (4 * CAP + 512) as u64, "soft RLIMIT_NOFILE {soft} too low for the 2,000-connection cap test");

    let (be_addr, mut rx) = backend().await;
    let mut t = RouteTable::default();
    t.insert("turn-box.k2.dev", be(Kind::Turn, be_addr, true)).unwrap();
    let r = rig(CAP, Duration::from_secs(10), t).await;
    let hello = fixture("curl-macos.bin");

    let mut clients = Vec::with_capacity(CAP);
    for i in 0..CAP {
        // Stay well inside the listen backlog (macOS somaxconn is 128): a
        // busy test box must not turn a slow accept into a reset.
        while (i as u64).saturating_sub(r.front.counters.accepted.load(Relaxed)) > 64 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let mut c = TcpStream::connect(r.addr).await.unwrap();
        c.write_all(&hello).await.unwrap();
        clients.push(c);
    }
    let mut seen = 0;
    while seen < CAP {
        tokio::time::timeout(Duration::from_secs(20), rx.recv()).await.expect("backend saw every connection").unwrap();
        seen += 1;
    }
    assert_eq!(r.front.counters.live.load(Relaxed), CAP as u64);

    // The next one is closed without being read.
    let mut over = TcpStream::connect(r.addr).await.unwrap();
    let _ = over.write_all(&hello).await;
    assert!(drain(&mut over).await.is_empty());
    assert_eq!(r.front.counters.over_cap.load(Relaxed), 1);
    assert!(tokio::time::timeout(Duration::from_millis(300), rx.recv()).await.is_err());

    // Free one slot: the next connection is routed.
    drop(clients.pop());
    for _ in 0..100 {
        if r.front.counters.live.load(Relaxed) < CAP as u64 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut again = TcpStream::connect(r.addr).await.unwrap();
    again.write_all(&hello).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("routed after a slot freed").unwrap();
    let mut pong = [0u8; 4];
    again.read_exact(&mut pong).await.unwrap();
    assert_eq!(&pong, b"pong");
    assert_eq!(r.front.counters.over_cap.load(Relaxed), 1);
}

/// Sew's order (front up -> TURN routed -> reconfigure): the probe passes
/// only when a ClientHello for the TURN name went through the front to a
/// listening backend; a missing route or a dead backend fails it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_turn_route_probe_proves_the_front_reaches_the_backend() {
    let (be_addr, mut rx) = backend().await;
    let mut t = RouteTable::default();
    t.insert("turn-box.k2.dev", be(Kind::Turn, be_addr, true)).unwrap();
    t.insert("turn-dead.k2.dev", be(Kind::Turn, "127.0.0.1:9".parse().unwrap(), true)).unwrap();
    let r = rig(16, Duration::from_secs(5), t).await;
    let (front, addr) = (Arc::clone(&r.front), r.addr);
    let ok = tokio::task::spawn_blocking(move || super::probe_through(&front, addr, "turn-box.k2.dev")).await.unwrap();
    assert_eq!(ok, Ok(()));
    let (_peer, got) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
    let hello = super::client_hello::synthetic("turn-box.k2.dev");
    assert!(got.ends_with(&hello), "PROXY v2 then the probe hello");
    assert_eq!(&got[..12], &proxy_v2::SIGNATURE);
    assert_eq!(done(peek(&hello)).sni.as_deref(), Some("turn-box.k2.dev"));

    let (front, addr) = (Arc::clone(&r.front), r.addr);
    let unrouted = tokio::task::spawn_blocking(move || super::probe_through(&front, addr, "turn-other.k2.dev")).await.unwrap();
    assert!(unrouted.unwrap_err().contains("did not route"));
    let (front, addr) = (Arc::clone(&r.front), r.addr);
    let dead = tokio::task::spawn_blocking(move || super::probe_through(&front, addr, "turn-dead.k2.dev")).await.unwrap();
    assert!(dead.unwrap_err().contains("nothing answers"), "a dead backend is named");
}
