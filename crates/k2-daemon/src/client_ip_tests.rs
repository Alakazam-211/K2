//! Tests for `client_ip`: the PROXY v2 preamble, the hop table and the
//! HTTP/1.1 rewrite (keep-alive, pipelining, chunked, upgrades, refusals).
//! The end-to-end run through the E2E listener lives in
//! `tunnel_tls_listener` tests.

use super::*;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CLIENT: &str = "203.0.113.7";

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn sa(s: &str) -> SocketAddr {
    s.parse().unwrap()
}

/// A connected loopback pair: (client side, accepted server side, peer).
async fn pair() -> (TcpStream, TcpStream, SocketAddr) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let c = TcpStream::connect(l.local_addr().unwrap()).await.unwrap();
    let (s, peer) = l.accept().await.unwrap();
    (c, s, peer)
}

fn deadline() -> tokio::time::Instant {
    tokio::time::Instant::now() + Duration::from_secs(5)
}

// ── PROXY v2 preamble ────────────────────────────────────────────────

#[tokio::test]
async fn preamble_from_frpc_names_the_client_and_leaves_the_tls_bytes() {
    let (mut c, mut s, peer) = pair().await;
    let mut bytes = proxy_v2::header(sa("203.0.113.7:50123"), sa("127.0.0.1:443"));
    bytes.extend_from_slice(b"\x16\x03\x01TLS");
    c.write_all(&bytes).await.unwrap();
    let got = read_tunnel_preamble(&mut s, peer, deadline()).await;
    assert_eq!(got, Ok(Some(ip(CLIENT))));
    let mut rest = [0u8; 6];
    s.read_exact(&mut rest).await.unwrap();
    assert_eq!(&rest, b"\x16\x03\x01TLS", "the bytes after the header are the TLS stream, untouched");
}

#[tokio::test]
async fn preamble_v6_client_and_mapped_v4() {
    let (mut c, mut s, peer) = pair().await;
    c.write_all(&proxy_v2::header(sa("[2001:db8::9]:1"), sa("[::1]:443"))).await.unwrap();
    assert_eq!(read_tunnel_preamble(&mut s, peer, deadline()).await, Ok(Some(ip("2001:db8::9"))));
    let (mut c, mut s, peer) = pair().await;
    c.write_all(&proxy_v2::header(sa("[::ffff:198.51.100.3]:1"), sa("[::1]:443"))).await.unwrap();
    assert_eq!(read_tunnel_preamble(&mut s, peer, deadline()).await, Ok(Some(ip("198.51.100.3"))));
}

#[tokio::test]
async fn no_header_old_frpc_config_reads_nothing() {
    let (mut c, mut s, peer) = pair().await;
    c.write_all(b"\x16\x03\x01hello").await.unwrap();
    assert_eq!(read_tunnel_preamble(&mut s, peer, deadline()).await, Ok(None));
    let mut rest = [0u8; 8];
    s.read_exact(&mut rest).await.unwrap();
    assert_eq!(&rest, b"\x16\x03\x01hello", "nothing consumed");
}

#[tokio::test]
async fn old_relay_loopback_source_and_local_are_no_client() {
    for h in [
        proxy_v2::header(sa("127.0.0.1:40000"), sa("127.0.0.1:8444")),
        proxy_v2::header(sa("[::1]:40000"), sa("[::1]:8444")),
        proxy_v2::header(sa("0.0.0.0:0"), sa("127.0.0.1:8444")),
        proxy_v2::local_header(),
    ] {
        let (mut c, mut s, peer) = pair().await;
        c.write_all(&h).await.unwrap();
        assert_eq!(read_tunnel_preamble(&mut s, peer, deadline()).await, Ok(None), "{h:02x?}");
    }
}

#[tokio::test]
async fn malformed_header_is_an_error() {
    let good = proxy_v2::header(sa("203.0.113.7:1"), sa("127.0.0.1:443"));
    let mut bad_version = good.clone();
    bad_version[12] = 0x31;
    let mut too_long = good.clone();
    too_long[14..16].copy_from_slice(&4096u16.to_be_bytes());
    let mut bad_sig = good.clone();
    bad_sig[5] = 0x99;
    for (what, bytes) in [("version", bad_version), ("length", too_long), ("signature", bad_sig)] {
        let (mut c, mut s, peer) = pair().await;
        c.write_all(&bytes).await.unwrap();
        let got = read_tunnel_preamble(&mut s, peer, deadline()).await;
        assert!(got.is_err(), "bad {what} must be refused, got {got:?}");
    }
    // Truncated: the sender stops mid-header and closes.
    let (mut c, mut s, peer) = pair().await;
    c.write_all(&good[..20]).await.unwrap();
    c.shutdown().await.unwrap();
    let got = read_tunnel_preamble(&mut s, peer, deadline()).await;
    assert!(got.is_err(), "a truncated header must be refused, got {got:?}");
}

#[tokio::test]
async fn header_from_a_non_loopback_peer_is_never_read() {
    let (mut c, mut s, _peer) = pair().await;
    let h = proxy_v2::header(sa("203.0.113.7:1"), sa("127.0.0.1:443"));
    c.write_all(&h).await.unwrap();
    // The same bytes, but the accepted peer claims to be a LAN address.
    let got = read_tunnel_preamble(&mut s, sa("192.168.1.50:5555"), deadline()).await;
    assert_eq!(got, Ok(None));
    let mut first = vec![0u8; h.len()];
    s.read_exact(&mut first).await.unwrap();
    assert_eq!(first, h, "nothing consumed: the TLS handshake will fail on it");
}

#[tokio::test]
async fn silent_peer_times_out_at_the_deadline() {
    let (_c, mut s, peer) = pair().await;
    let started = std::time::Instant::now();
    let got = read_tunnel_preamble(&mut s, peer, tokio::time::Instant::now() + Duration::from_millis(300)).await;
    let err = got.expect_err("silence must time out");
    assert!(err.contains("timed out"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(3));
}

// ── hops ────────────────────────────────────────────────────────────

#[tokio::test]
async fn hop_maps_the_upstream_local_address_while_alive() {
    let (c, s, peer) = pair().await;
    assert_eq!(peer, c.local_addr().unwrap());
    assert_eq!(hop_client_ip(peer), None);
    {
        let _hop = register_hop(&c, Some(ip(CLIENT)));
        assert_eq!(hop_client_ip(peer), Some(ip(CLIENT)), "the accepting side finds it by peer");
        assert_eq!(hop_client_ip(s.local_addr().unwrap()), None, "only that address");
    }
    assert_eq!(hop_client_ip(peer), None, "forgotten when the guard drops");
    let _none = register_hop(&c, None);
    assert_eq!(hop_client_ip(peer), None, "no client, nothing recorded");
}

// ── HTTP/1.1 rewrite ─────────────────────────────────────────────────

/// An App fixture: answers every request with its exact received head as
/// the body (keep-alive, Content-Length bodies and chunked bodies are
/// read). `Upgrade: websocket` → 101, then echoes raw bytes; with
/// `X-Refuse-Upgrade` → 403 and keep-alive. Returns its port.
pub(crate) async fn echo_app() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            tokio::spawn(echo_conn(s));
        }
    });
    port
}

async fn echo_conn(mut s: TcpStream) {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        // One head.
        let end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            let mut b = [0u8; 4096];
            match s.read(&mut b).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&b[..n]),
            }
        };
        let head: Vec<u8> = buf.drain(..end).collect();
        let text = String::from_utf8_lossy(&head).to_string();
        let lower = text.to_ascii_lowercase();
        let header = |name: &str| {
            lower.lines().find_map(|l| l.strip_prefix(&format!("{name}: ")).map(|v| v.trim().to_string()))
        };
        if let Some(n) = header("content-length").and_then(|v| v.parse::<usize>().ok()) {
            while buf.len() < n {
                let mut b = [0u8; 4096];
                match s.read(&mut b).await {
                    Ok(0) | Err(_) => return,
                    Ok(k) => buf.extend_from_slice(&b[..k]),
                }
            }
            buf.drain(..n);
        } else if header("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
            // Read through the last chunk's empty trailer line.
            loop {
                if let Some(i) = buf.windows(5).position(|w| w == b"0\r\n\r\n") {
                    buf.drain(..i + 5);
                    break;
                }
                let mut b = [0u8; 4096];
                match s.read(&mut b).await {
                    Ok(0) | Err(_) => return,
                    Ok(k) => buf.extend_from_slice(&b[..k]),
                }
            }
        }
        if header("upgrade").as_deref() == Some("websocket") {
            if header("x-refuse-upgrade").is_some() {
                let body = "no";
                let r = format!("HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\n\r\n{body}", body.len());
                if s.write_all(r.as_bytes()).await.is_err() {
                    return;
                }
                continue;
            }
            let r = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nX-Handshake-Len: {}\r\n\r\n", head.len());
            if s.write_all(r.as_bytes()).await.is_err() {
                return;
            }
            // Echo the head we got first, then raw bytes forever.
            let _ = s.write_all(&head).await;
            if !buf.is_empty() {
                let _ = s.write_all(&buf).await;
            }
            let (mut r, mut w) = s.split();
            let _ = tokio::io::copy(&mut r, &mut w).await;
            return;
        }
        if text.starts_with("HEAD ") {
            let r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", head.len());
            if s.write_all(r.as_bytes()).await.is_err() {
                return;
            }
            continue;
        }
        let r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", head.len());
        if s.write_all(r.as_bytes()).await.is_err() || s.write_all(&head).await.is_err() {
            return;
        }
    }
}

/// The test client: a socket plus whatever it has read past the last
/// response (pipelined responses can arrive in one read).
struct Client {
    s: TcpStream,
    buf: Vec<u8>,
}

impl Client {
    async fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.s.write_all(bytes).await
    }

    async fn more(&mut self) {
        let mut b = [0u8; 4096];
        let n = tokio::time::timeout(TEST_WAIT, self.s.read(&mut b)).await.expect("bytes in time").unwrap();
        assert!(n > 0, "connection closed; have {:?}", String::from_utf8_lossy(&self.buf));
        self.buf.extend_from_slice(&b[..n]);
    }

    /// Exactly `n` bytes.
    async fn take(&mut self, n: usize) -> Vec<u8> {
        while self.buf.len() < n {
            self.more().await;
        }
        self.buf.drain(..n).collect()
    }

    /// One response: (head, body). `no_body`: a HEAD / 1xx answer.
    async fn response_with(&mut self, no_body: bool) -> (String, String) {
        let end = loop {
            if let Some(i) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            self.more().await;
        };
        let head = String::from_utf8_lossy(&self.take(end).await).to_string();
        if no_body || head.starts_with("HTTP/1.1 1") {
            return (head, String::new());
        }
        let len: usize = values(&head, "content-length").first().map(|v| v.parse().unwrap()).unwrap_or(0);
        let body = String::from_utf8_lossy(&self.take(len).await).to_string();
        (head, body)
    }

    async fn response(&mut self) -> (String, String) {
        self.response_with(false).await
    }

    /// Everything until the other side closes.
    async fn rest(&mut self) -> String {
        let mut out = std::mem::take(&mut self.buf);
        tokio::time::timeout(TEST_WAIT, self.s.read_to_end(&mut out)).await.expect("closed in time").unwrap();
        String::from_utf8_lossy(&out).to_string()
    }
}

/// A client whose connection is forwarded (in `mode`) to `app`.
async fn via(app: u16, mode: Mode) -> Client {
    let (client, server_side, _) = pair().await;
    tokio::spawn(async move {
        let mut up = TcpStream::connect(("127.0.0.1", app)).await.unwrap();
        let mut down = server_side;
        forward(&mut down, &mut up, mode).await
    });
    Client { s: client, buf: Vec::new() }
}

/// A client forwarded to a fresh echo App.
async fn through(mode: Mode) -> Client {
    via(echo_app().await, mode).await
}

/// Every header line named `name` (case-insensitive) in an echoed head.
fn values(head: &str, name: &str) -> Vec<String> {
    head.lines()
        .filter_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
        .collect()
}

const SPOOFS: &str = "X-Forwarded-For: 6.6.6.6\r\nX-Real-IP: 6.6.6.6\r\nForwarded: for=6.6.6.6\r\nX-K2-Client-IP: 6.6.6.6\r\nx_forwarded_for: 6.6.6.6\r\nX-K2_Client-IP: 6.6.6.6\r\nTrue-Client-IP: 6.6.6.6\r\nCF-Connecting-IP: 6.6.6.6\r\nX-Client-IP: 6.6.6.6\r\n";

fn assert_k2_only(seen: &str, want: Option<&str>) {
    assert!(!seen.contains("6.6.6.6"), "a client copy reached the App:\n{seen}");
    for h in ["forwarded", "true-client-ip", "cf-connecting-ip", "x-client-ip", "x_forwarded_for", "x-k2_client-ip"] {
        assert!(values(seen, h).is_empty(), "{h} must be gone:\n{seen}");
    }
    match want {
        Some(ip) => {
            assert_eq!(values(seen, "x-k2-client-ip"), vec![ip.to_string()], "{seen}");
            assert_eq!(values(seen, "x-forwarded-for"), vec![ip.to_string()], "single-value XFF:\n{seen}");
            assert_eq!(values(seen, "x-real-ip"), vec![ip.to_string()], "{seen}");
        }
        None => {
            for h in ["x-k2-client-ip", "x-forwarded-for", "x-real-ip"] {
                assert!(values(seen, h).is_empty(), "no client IP: {h} must be absent:\n{seen}");
            }
        }
    }
}

#[tokio::test]
async fn keep_alive_every_request_is_rewritten() {
    let mut c = through(Mode::Tunnel(Some(ip(CLIENT)))).await;
    for i in 0..3 {
        let req = format!("GET /r{i} HTTP/1.1\r\nHost: app.example.k2.dev\r\n{SPOOFS}Accept: */*\r\n\r\n");
        c.send(req.as_bytes()).await.unwrap();
        let (head, seen) = c.response().await;
        assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
        assert!(seen.starts_with(&format!("GET /r{i} HTTP/1.1\r\n")), "{seen}");
        assert_eq!(values(&seen, "host"), vec!["app.example.k2.dev"]);
        assert_eq!(values(&seen, "accept"), vec!["*/*"], "other headers kept");
        assert_k2_only(&seen, Some(CLIENT));
    }
}

#[tokio::test]
async fn no_client_ip_means_no_ip_headers_at_all() {
    let mut c = through(Mode::Tunnel(None)).await;
    c.send(format!("GET / HTTP/1.1\r\nHost: a\r\n{SPOOFS}\r\n").as_bytes()).await.unwrap();
    let (_, seen) = c.response().await;
    assert_k2_only(&seen, None);
}

#[tokio::test]
async fn pipelined_requests_in_one_write_are_each_rewritten() {
    let mut c = through(Mode::Tunnel(Some(ip(CLIENT)))).await;
    let one = format!("GET /a HTTP/1.1\r\nHost: a\r\n{SPOOFS}\r\n");
    let two = format!("POST /b HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\n{SPOOFS}\r\nhello");
    let three = format!("GET /c HTTP/1.1\r\nHost: a\r\n{SPOOFS}\r\n");
    c.send(format!("{one}{two}{three}").as_bytes()).await.unwrap();
    for path in ["GET /a", "POST /b", "GET /c"] {
        let (_, seen) = c.response().await;
        assert!(seen.starts_with(path), "{path}: {seen}");
        assert_k2_only(&seen, Some(CLIENT));
    }
}

#[tokio::test]
async fn smuggled_request_in_a_body_is_not_parsed_but_the_next_one_is() {
    // The body *contains* text that looks like a request with a spoof: it
    // is body bytes (Content-Length covers it), never a head.
    let mut c = through(Mode::Tunnel(Some(ip(CLIENT)))).await;
    let inner = "GET /x HTTP/1.1\r\nX-Forwarded-For: 6.6.6.6\r\n\r\n";
    let req = format!("POST /p HTTP/1.1\r\nHost: a\r\nContent-Length: {}\r\n\r\n{inner}GET /n HTTP/1.1\r\nHost: a\r\nX-Forwarded-For: 6.6.6.6\r\n\r\n", inner.len());
    c.send(req.as_bytes()).await.unwrap();
    let (_, first) = c.response().await;
    assert!(first.starts_with("POST /p"), "{first}");
    let (_, second) = c.response().await;
    assert!(second.starts_with("GET /n"), "{second}");
    assert_k2_only(&second, Some(CLIENT));
}

#[tokio::test]
async fn chunked_body_is_reframed_and_owned_trailers_dropped() {
    // Capture what the App receives byte for byte.
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let app = l.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel::<Vec<u8>>();
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let mut got = Vec::new();
        // Until the second request's head is complete.
        while !(got.windows(10).any(|w| w == b"GET /after") && got.ends_with(b"\r\n\r\n")) {
            let mut b = [0u8; 4096];
            let n = s.read(&mut b).await.unwrap();
            assert!(n > 0, "closed early: {:?}", String::from_utf8_lossy(&got));
            got.extend_from_slice(&b[..n]);
        }
        tx.send(got).unwrap();
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    });
    let mut c = via(app, Mode::Tunnel(Some(ip(CLIENT)))).await;
    c.send(
        b"POST /up HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n\
          5;ext=1\r\nhello\r\n00A\r\n0123456789\r\n0\r\nX-Forwarded-For: 6.6.6.6\r\nX-Trace: t1\r\n\r\n\
          GET /after HTTP/1.1\r\nHost: a\r\n\r\n",
    )
    .await
    .unwrap();
    let got = tokio::time::timeout(TEST_WAIT, rx).await.expect("App saw both").unwrap();
    let text = String::from_utf8_lossy(&got).to_string();
    assert_eq!(
        text,
        "POST /up HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\nX-K2-Client-IP: 203.0.113.7\r\nX-Forwarded-For: 203.0.113.7\r\nX-Real-IP: 203.0.113.7\r\n\r\n\
         5\r\nhello\r\na\r\n0123456789\r\n0\r\nX-Trace: t1\r\n\r\n\
         GET /after HTTP/1.1\r\nHost: a\r\nX-K2-Client-IP: 203.0.113.7\r\nX-Forwarded-For: 203.0.113.7\r\nX-Real-IP: 203.0.113.7\r\n\r\n"
    );
    let (h1, _) = c.response().await;
    let (h2, _) = c.response().await;
    assert!(h1.starts_with("HTTP/1.1 200") && h2.starts_with("HTTP/1.1 200"));
}

#[tokio::test]
async fn websocket_upgrade_is_rewritten_then_raw() {
    let mut c = through(Mode::Tunnel(Some(ip(CLIENT)))).await;
    let hs = format!(
        "GET /ws HTTP/1.1\r\nHost: a\r\nUpgrade: websocket\r\nConnection: Upgrade, X-K2-Client-IP\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{SPOOFS}\r\n"
    );
    c.send(hs.as_bytes()).await.unwrap();
    let (head, _) = c.response().await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    let len: usize = values(&head, "x-handshake-len")[0].parse().unwrap();
    let seen = String::from_utf8_lossy(&c.take(len).await).to_string();
    assert_k2_only(&seen, Some(CLIENT));
    assert_eq!(values(&seen, "connection"), vec!["Upgrade"], "K2's names can't be listed hop-by-hop:\n{seen}");
    assert_eq!(values(&seen, "upgrade"), vec!["websocket"]);
    // After the 101 the bytes are the App's protocol, passed through as is
    // (even ones that look like HTTP).
    let frame = b"\x81\x05hello GET / HTTP/1.1\r\nX-Forwarded-For: 1.1.1.1\r\n\r\n";
    c.send(frame).await.unwrap();
    assert_eq!(c.take(frame.len()).await, frame);
}

#[tokio::test]
async fn refused_upgrade_keeps_rewriting_the_next_request() {
    let mut c = through(Mode::Tunnel(Some(ip(CLIENT)))).await;
    // The upgrade and a pipelined plain request in one write: the second
    // must wait for the App's answer, then be parsed and rewritten (not
    // passed through raw).
    let req = format!(
        "GET /ws HTTP/1.1\r\nHost: a\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nX-Refuse-Upgrade: 1\r\n\r\nGET /plain HTTP/1.1\r\nHost: a\r\n{SPOOFS}\r\n"
    );
    c.send(req.as_bytes()).await.unwrap();
    let (head, body) = c.response().await;
    assert!(head.starts_with("HTTP/1.1 403") && body == "no", "{head}{body}");
    let (_, seen) = c.response().await;
    assert!(seen.starts_with("GET /plain"), "{seen}");
    assert_k2_only(&seen, Some(CLIENT));
}

#[tokio::test]
async fn expect_100_continue_is_relayed() {
    // An App that sends 100 Continue before reading the body.
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let app = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let mut got = Vec::new();
        while !got.windows(4).any(|w| w == b"\r\n\r\n") {
            let mut b = [0u8; 1024];
            let n = s.read(&mut b).await.unwrap();
            got.extend_from_slice(&b[..n]);
        }
        s.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await.unwrap();
        let mut body = [0u8; 4];
        s.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"data");
        s.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok").await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
    });
    let mut c = via(app, Mode::Tunnel(Some(ip(CLIENT)))).await;
    c.send(b"PUT /f HTTP/1.1\r\nHost: a\r\nContent-Length: 4\r\nExpect: 100-continue\r\n\r\n").await.unwrap();
    let (cont, _) = c.response().await;
    assert!(cont.starts_with("HTTP/1.1 100"), "{cont}");
    c.send(b"data").await.unwrap();
    let (done, body) = c.response().await;
    assert!(done.starts_with("HTTP/1.1 201") && body == "ok", "{done}{body}");
}

#[tokio::test]
async fn head_response_has_no_body_and_the_next_response_frames() {
    let mut c = through(Mode::Tunnel(Some(ip(CLIENT)))).await;
    c.send(b"HEAD /h HTTP/1.1\r\nHost: a\r\n\r\nGET /g HTTP/1.1\r\nHost: a\r\nX-Real-IP: 6.6.6.6\r\n\r\n").await.unwrap();
    // HEAD: a Content-Length but no body.
    let (head, _) = c.response_with(true).await;
    assert!(head.starts_with("HTTP/1.1 200") && !values(&head, "content-length").is_empty(), "{head}");
    let (_, seen) = c.response().await;
    assert!(seen.starts_with("GET /g"), "{seen}");
    assert_k2_only(&seen, Some(CLIENT));
}

#[tokio::test]
async fn ambiguous_or_foreign_requests_are_refused_never_passed_raw() {
    let cases: &[(&str, &[u8])] = &[
        ("TE+CL", b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"),
        ("two CLs", b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 4\r\nContent-Length: 5\r\n\r\nabcd"),
        ("CL list", b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 4, 4\r\n\r\nabcd"),
        ("signed CL", b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: +4\r\n\r\nabcd"),
        ("TE not chunked", b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: gzip\r\n\r\nxx"),
        ("double chunked", b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked, chunked\r\n\r\n0\r\n\r\n"),
        ("TE in 1.0", b"POST / HTTP/1.0\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"),
        ("underscored CL", b"POST / HTTP/1.1\r\nHost: a\r\nContent_Length: 4\r\n\r\nabcd"),
        ("space before colon", b"GET / HTTP/1.1\r\nHost: a\r\nX-Forwarded-For : 6.6.6.6\r\n\r\n"),
        ("obs-fold", b"GET / HTTP/1.1\r\nHost: a\r\nX-Note: a\r\n X-Forwarded-For: 6.6.6.6\r\n\r\n"),
        ("h2 preface", b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"),
        ("CONNECT", b"CONNECT a:443 HTTP/1.1\r\nHost: a:443\r\n\r\n"),
        ("not HTTP", b"\x00\x01binary-protocol\r\n\r\n"),
    ];
    for (what, bytes) in cases {
        let mut c = through(Mode::Tunnel(Some(ip(CLIENT)))).await;
        c.send(bytes).await.unwrap();
        let out = c.rest().await;
        assert!(out.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{what}: {out}");
        assert!(!out.contains("200 OK") && !out.contains("6.6.6.6"), "{what} must not reach the App: {out}");
    }
}

#[tokio::test]
async fn refusal_waits_for_earlier_responses() {
    let mut c = through(Mode::Tunnel(Some(ip(CLIENT)))).await;
    c.send(b"GET /ok HTTP/1.1\r\nHost: a\r\n\r\nGET / HTTP/1.1\r\nHost: a\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n").await.unwrap();
    let out = c.rest().await;
    let ok = out.find("HTTP/1.1 200 OK").expect("first answered");
    let refused = out.find("HTTP/1.1 400 Bad Request").expect("then refused");
    assert!(ok < refused, "{out}");
}

#[tokio::test]
async fn oversized_head_is_431() {
    let mut c = through(Mode::Tunnel(Some(ip(CLIENT)))).await;
    let big = format!("GET / HTTP/1.1\r\nHost: a\r\nX-Big: {}\r\n\r\n", "a".repeat(MAX_HEAD + 10));
    let _ = c.send(big.as_bytes()).await;
    let out = c.rest().await;
    assert!(out.starts_with("HTTP/1.1 431"), "{out}");
}

#[tokio::test]
async fn local_front_port_strips_only_k2s_header_unless_it_is_k2s_hop() {
    // A local caller (not K2's hop): X-K2-Client-IP dropped, a local
    // proxy's own X-Forwarded-For kept.
    let mut c = through(Mode::Local { peer: Some(sa("127.0.0.1:1")) }).await;
    c.send(b"GET / HTTP/1.1\r\nHost: a\r\nX-K2-Client-IP: 6.6.6.6\r\nX-Forwarded-For: 198.51.100.1\r\n\r\n").await.unwrap();
    let (_, seen) = c.response().await;
    assert!(values(&seen, "x-k2-client-ip").is_empty(), "{seen}");
    assert_eq!(values(&seen, "x-forwarded-for"), vec!["198.51.100.1"], "{seen}");

    // K2's own hop from the tunnel: the address it carries is set.
    let app = echo_app().await;
    let front = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let front_port = front.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (s, peer) = front.accept().await.unwrap();
        let mut up = TcpStream::connect(("127.0.0.1", app)).await.unwrap();
        let mut down = s;
        let _ = forward(&mut down, &mut up, Mode::Local { peer: Some(peer) }).await;
    });
    let hop = TcpStream::connect(("127.0.0.1", front_port)).await.unwrap();
    let _guard = register_hop(&hop, Some(ip("198.51.100.200")));
    let mut hop = Client { s: hop, buf: Vec::new() };
    hop.send(format!("GET / HTTP/1.1\r\nHost: a\r\n{SPOOFS}\r\n").as_bytes()).await.unwrap();
    let (_, seen) = hop.response().await;
    assert_k2_only(&seen, Some("198.51.100.200"));
}

#[test]
fn owned_names_cover_underscore_spellings() {
    for n in ["X_Forwarded_For", "x-real_ip", "X-K2_CLIENT_IP", "FORWARDED"] {
        assert!(is_owned(&normalized(n)), "{n}");
    }
    assert!(!is_owned(&normalized("X-Forwarded-Proto")));
    assert_eq!(chunk_size(b"1f").unwrap(), 31);
    assert_eq!(chunk_size(b"A ;x=y").unwrap(), 10);
    for bad in [&b""[..], b"0x5", b"-1", b"5 5", b"g", b"1234567890abcdef0"] {
        assert!(chunk_size(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
    }
}
