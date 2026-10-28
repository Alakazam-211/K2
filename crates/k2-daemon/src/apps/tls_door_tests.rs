//! The App TLS door, end to end on loopback (0.46.6): a real rustls
//! client → (front443 →) the door → the App path → an HTTP/1.1 App.
//!
//! Every test runs on the current-thread runtime, so the door's DB reads
//! see the test's scoped database.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use crate::apps::custom_domains::test_util::runtime_app;
use crate::front443::routes::{Backend, Kind, RouteTable};

const HOST: &str = "app.example.com";

struct Pem {
    chain: String,
    key: String,
    der: CertificateDer<'static>,
}

fn self_signed(host: &str) -> Pem {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec![host.to_string()]).unwrap().self_signed(&key).unwrap();
    Pem { chain: cert.pem(), key: key.serialize_pem(), der: cert.der().clone() }
}

/// Bind `host` to App `app` in `state`, with `pem` in the box store.
fn bind(app: &str, host: &str, state: &str, pem: Option<&Pem>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    domains::upsert_binding(&conn, "example.com", None, false).unwrap();
    domains::upsert_name(&conn, host, "example.com", &format!("app:{app}")).unwrap();
    domains::set_app_state(&conn, host, state, None).unwrap();
    drop(conn);
    if let Some(p) = pem {
        crate::domains::store::install(host, &p.chain, &p.key).unwrap();
    }
}

/// A tiny HTTP/1.1 App: answers each request with what K2 forwarded
/// (`xff=…;xri=…;k2=…;body=<bytes>`); a WebSocket upgrade gets `101` and
/// then echoes raw bytes until the peer closes.
async fn app_backend() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            tokio::spawn(serve_app(s));
        }
    });
    port
}

async fn serve_app(s: TcpStream) {
    let mut r = BufReader::new(s);
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let mut headers: Vec<(String, String)> = Vec::new();
        loop {
            let mut h = String::new();
            r.read_line(&mut h).await.unwrap();
            let h = h.trim_end().to_string();
            if h.is_empty() {
                break;
            }
            let (k, v) = h.split_once(':').unwrap();
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
        let get = |k: &str| headers.iter().filter(|(n, _)| n == k).map(|(_, v)| v.clone()).collect::<Vec<_>>().join("|");
        if get("upgrade").eq_ignore_ascii_case("websocket") {
            r.get_mut().write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").await.unwrap();
            let mut buf = vec![0u8; 4096];
            loop {
                let n = match r.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                if r.get_mut().write_all(&buf[..n]).await.is_err() {
                    return;
                }
            }
        }
        let mut body = 0usize;
        let cl = get("content-length");
        if !cl.is_empty() {
            let mut left: usize = cl.parse().unwrap();
            let mut buf = vec![0u8; 64 * 1024];
            while left > 0 {
                let n = r.read(&mut buf[..left.min(64 * 1024)]).await.unwrap();
                assert!(n > 0, "body cut short");
                left -= n;
                body += n;
            }
        } else if get("transfer-encoding").eq_ignore_ascii_case("chunked") {
            loop {
                let mut sz = String::new();
                r.read_line(&mut sz).await.unwrap();
                let n = usize::from_str_radix(sz.trim().split(';').next().unwrap(), 16).unwrap();
                if n == 0 {
                    let mut end = String::new();
                    r.read_line(&mut end).await.unwrap();
                    break;
                }
                let mut chunk = vec![0u8; n + 2];
                r.read_exact(&mut chunk).await.unwrap();
                body += n;
            }
        }
        let text = format!("xff={};xri={};k2={};body={body}", get("x-forwarded-for"), get("x-real-ip"), get("x-k2-client-ip"));
        let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{text}", text.len());
        r.get_mut().write_all(resp.as_bytes()).await.unwrap();
    }
}

/// The door on an ephemeral loopback port, serving on THIS runtime.
async fn door() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let _ = serve_conn(tcp, server_config()).await;
            });
        }
    });
    addr
}

fn client(trust: &[&CertificateDer<'static>], alpn: &[&[u8]]) -> tokio_rustls::TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    for c in trust {
        roots.add((*c).clone()).unwrap();
    }
    let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    cfg.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
    tokio_rustls::TlsConnector::from(Arc::new(cfg))
}

/// Connect to `addr`, optionally writing a PROXY v2 header for `client_ip`
/// first (what front443 does), then TLS for `host`.
async fn connect(
    addr: SocketAddr,
    proxy_from: Option<SocketAddr>,
    host: &str,
    c: &tokio_rustls::TlsConnector,
) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let mut tcp = TcpStream::connect(addr).await?;
    if let Some(src) = proxy_from {
        tcp.write_all(&crate::front443::proxy_v2::header(src, addr)).await?;
    }
    c.connect(ServerName::try_from(host.to_string()).unwrap(), tcp).await
}

/// Read one HTTP/1.1 response (Content-Length body).
async fn response<S: AsyncRead + Unpin>(s: &mut BufReader<S>) -> (String, String) {
    let mut status = String::new();
    s.read_line(&mut status).await.unwrap();
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        s.read_line(&mut h).await.unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap();
        }
    }
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).await.unwrap();
    (status.trim_end().to_string(), String::from_utf8(body).unwrap())
}

async fn write_all<S: AsyncWrite + Unpin>(s: &mut S, b: &[u8]) {
    s.write_all(b).await.unwrap();
    s.flush().await.unwrap();
}

fn rig_app(app: &str, ws: &str) {
    runtime_app(ws, app);
}

/// NSI requirement: the door SETS and OVERWRITES `X-Forwarded-For` and
/// `X-Real-IP` (and `X-K2-Client-IP`) from the address front443 saw; a
/// client's own copies never reach the App.
#[tokio::test]
async fn a_live_name_reaches_its_app_and_the_client_ip_headers_are_k2s() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    rig_app("jobs", "ws-a");
    let pem = self_signed(HOST);
    bind("jobs", HOST, domains::STATE_LIVE, Some(&pem));
    let port = app_backend().await;
    crate::apps::router::switch_with("jobs", port, true);
    let r = reload();
    assert_eq!(r.routed, vec![HOST.to_string()]);
    assert!(r.without_cert.is_empty(), "{r:?}");
    assert_eq!(app_for_host("App.Example.com."), Some("jobs".to_string()));
    let addr = door().await;
    let c = client(&[&pem.der], &[b"http/1.1"]);

    let tls = connect(addr, Some("203.0.113.7:51000".parse().unwrap()), HOST, &c).await.expect("handshake with the name's own cert");
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(&b"http/1.1"[..]));
    let mut s = BufReader::new(tls);
    write_all(
        s.get_mut(),
        b"GET / HTTP/1.1\r\nHost: app.example.com\r\nX-Forwarded-For: 6.6.6.6\r\nX-Real-IP: 6.6.6.6\r\nX-K2-Client-IP: 6.6.6.6\r\nForwarded: for=6.6.6.6\r\n\r\n",
    )
    .await;
    let (status, body) = response(&mut s).await;
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(body, "xff=203.0.113.7;xri=203.0.113.7;k2=203.0.113.7;body=0", "K2's address only, exactly once each");

    // Keep-alive: a second request on the same connection is rewritten too.
    write_all(s.get_mut(), b"GET /again HTTP/1.1\r\nHost: app.example.com\r\nX-Forwarded-For: 7.7.7.7, 8.8.8.8\r\n\r\n").await;
    let (_, body) = response(&mut s).await;
    assert_eq!(body, "xff=203.0.113.7;xri=203.0.113.7;k2=203.0.113.7;body=0");

    // No PROXY header (or a loopback source): K2 knows no client, so the
    // App gets none of these headers, never the client's.
    let tls = connect(addr, None, HOST, &c).await.unwrap();
    let mut s = BufReader::new(tls);
    write_all(s.get_mut(), b"GET / HTTP/1.1\r\nHost: app.example.com\r\nX-Forwarded-For: 6.6.6.6\r\nX-Real-IP: 6.6.6.6\r\n\r\n").await;
    let (_, body) = response(&mut s).await;
    assert_eq!(body, "xff=;xri=;k2=;body=0");
    crate::apps::router::clear("jobs");
}

/// NSI requirement: a 20 MB single request and a 100 MB chunked upload pass
/// whole (no body limit at the door).
#[tokio::test]
async fn big_uploads_pass_whole() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    rig_app("upl", "ws-a");
    let host = "upl.example.com";
    let pem = self_signed(host);
    bind("upl", host, domains::STATE_LIVE, Some(&pem));
    let port = app_backend().await;
    crate::apps::router::switch_with("upl", port, true);
    reload();
    let addr = door().await;
    let c = client(&[&pem.der], &[b"http/1.1"]);
    let tls = connect(addr, Some("198.51.100.4:40000".parse().unwrap()), host, &c).await.unwrap();
    let mut s = BufReader::new(tls);

    const MB: usize = 1024 * 1024;
    let body = vec![b'x'; 20 * MB];
    write_all(s.get_mut(), format!("POST /up HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes()).await;
    write_all(s.get_mut(), &body).await;
    let (status, got) = response(&mut s).await;
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(got.ends_with(&format!("body={}", 20 * MB)), "{got}");

    write_all(s.get_mut(), format!("POST /chunks HTTP/1.1\r\nHost: {host}\r\nTransfer-Encoding: chunked\r\n\r\n").as_bytes()).await;
    let chunk = vec![b'y'; MB];
    for _ in 0..100 {
        write_all(s.get_mut(), format!("{:x}\r\n", chunk.len()).as_bytes()).await;
        write_all(s.get_mut(), &chunk).await;
        write_all(s.get_mut(), b"\r\n").await;
    }
    write_all(s.get_mut(), b"0\r\n\r\n").await;
    let (status, got) = response(&mut s).await;
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(got.ends_with(&format!("body={}", 100 * MB)), "{got}");
    crate::apps::router::clear("upl");
}

/// A WebSocket through the door: upgraded, then raw both ways after an
/// idle gap of `idle`.
async fn websocket_survives(idle: Duration) {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    rig_app("ws", "ws-a");
    let host = "ws.example.com";
    let pem = self_signed(host);
    bind("ws", host, domains::STATE_LIVE, Some(&pem));
    let port = app_backend().await;
    crate::apps::router::switch_with("ws", port, true);
    reload();
    let addr = door().await;
    let c = client(&[&pem.der], &[b"http/1.1"]);
    let tls = connect(addr, Some("198.51.100.9:40001".parse().unwrap()), host, &c).await.unwrap();
    let mut s = BufReader::new(tls);
    write_all(
        s.get_mut(),
        format!(
            "GET /api/stream HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        .as_bytes(),
    )
    .await;
    let mut status = String::new();
    s.read_line(&mut status).await.unwrap();
    assert!(status.starts_with("HTTP/1.1 101"), "{status}");
    loop {
        let mut h = String::new();
        s.read_line(&mut h).await.unwrap();
        if h.trim_end().is_empty() {
            break;
        }
    }
    for round in 0..2 {
        tokio::time::sleep(idle).await;
        let msg = format!("frame-{round}-after-{}s", idle.as_secs());
        write_all(s.get_mut(), msg.as_bytes()).await;
        let mut back = vec![0u8; msg.len()];
        tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut back)).await.expect("echo in time").expect("still open");
        assert_eq!(back, msg.as_bytes(), "round {round}");
    }
    crate::apps::router::clear("ws");
}

#[tokio::test]
async fn a_websocket_upgrades_and_survives_an_idle_gap() {
    websocket_survives(Duration::from_secs(3)).await;
}

/// NSI requirement: a 10+ minute WebSocket through the door. Long, so it
/// runs on request: `cargo test -p k2-daemon --lib door_websocket_eleven -- --ignored`.
#[tokio::test]
#[ignore = "11 minutes; run with --ignored (NSI long-lived WebSocket check)"]
async fn door_websocket_eleven_idle_minutes() {
    websocket_survives(Duration::from_secs(11 * 60)).await;
}

/// PRD test 5 + CD-A10: an `acme-tls/1` hello gets ONLY a registered
/// challenge certificate, and the connection then closes; for a name with
/// no challenge pending it fails. Unknown names, `issuing` names (normal
/// hello) and names of an App that doesn't exist get no certificate.
#[tokio::test]
async fn challenge_hellos_unknown_names_and_unserved_states_are_closed() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    rig_app("jobs", "ws-a");
    let live = self_signed(HOST);
    bind("jobs", HOST, domains::STATE_LIVE, Some(&live));
    let issuing = self_signed("new.example.com");
    bind("jobs", "new.example.com", domains::STATE_ISSUING, Some(&issuing));
    bind("jobs", "wait.example.com", domains::STATE_WAITING_DNS, None);
    let ghost = self_signed("ghost.example.com");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::upsert_name(&conn, "ghost.example.com", "example.com", "app:nobody").unwrap();
        domains::set_app_state(&conn, "ghost.example.com", domains::STATE_LIVE, None).unwrap();
    }
    crate::domains::store::install("ghost.example.com", &ghost.chain, &ghost.key).unwrap();
    let r = reload();
    assert_eq!(r.routed, vec![HOST.to_string(), "new.example.com".to_string()], "live + issuing only, existing Apps only");
    assert_eq!(app_for_host("new.example.com"), None, "issuing is routed, not served");
    let addr = door().await;

    // acme-tls/1 for the live name, no challenge pending: no certificate.
    let acme = client(&[&live.der], &[ACME_TLS_ALPN]);
    assert!(connect(addr, None, HOST, &acme).await.is_err(), "no challenge → closed");
    let acme_new = client(&[&issuing.der], &[ACME_TLS_ALPN]);
    assert!(connect(addr, None, "new.example.com", &acme_new).await.is_err());

    // A challenge registered for the issuing name: that hello gets it.
    let chal = self_signed("new.example.com");
    let guard = Challenge::register("new.example.com", certified_key(&chal.chain, &chal.key).unwrap());
    assert!(challenge_pending("new.example.com"));
    let acme_chal = client(&[&chal.der], &[ACME_TLS_ALPN]);
    let mut t = connect(addr, None, "new.example.com", &acme_chal).await.expect("the challenge cert is served");
    assert_eq!(t.get_ref().1.alpn_protocol(), Some(ACME_TLS_ALPN));
    let mut buf = [0u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(5), t.read(&mut buf)).await.unwrap().unwrap_or(0);
    assert_eq!(n, 0, "a challenge connection carries no data and closes");
    // ...and a NORMAL hello for that name still gets nothing (not live).
    let normal = client(&[&chal.der, &issuing.der], &[b"http/1.1"]);
    assert!(connect(addr, None, "new.example.com", &normal).await.is_err());
    drop(guard);
    assert!(!challenge_pending("new.example.com"), "the guard clears it");
    assert!(connect(addr, None, "new.example.com", &acme_chal).await.is_err());

    // Unknown names, a waiting name and an App that doesn't exist: closed.
    let any = client(&[&live.der, &ghost.der], &[b"http/1.1"]);
    for h in ["evil.example", "wait.example.com", "ghost.example.com"] {
        assert!(connect(addr, None, h, &any).await.is_err(), "{h} must be closed");
    }
    // The live name itself works.
    assert!(connect(addr, None, HOST, &client(&[&live.der], &[b"http/1.1"])).await.is_ok());
}

/// A live name whose certificate is missing from the store is reported
/// (routed, but every handshake fails), never served with another cert.
#[tokio::test]
async fn a_live_name_without_a_stored_cert_is_reported_and_closed() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    rig_app("jobs", "ws-a");
    bind("jobs", HOST, domains::STATE_LIVE, None);
    let r = reload();
    assert_eq!(r.without_cert, vec![HOST.to_string()]);
    let addr = door().await;
    let any = self_signed(HOST);
    assert!(connect(addr, None, HOST, &client(&[&any.der], &[b"http/1.1"])).await.is_err());
}

/// front443 → the door, end to end: the front routes the custom name by
/// SNI with PROXY v2 to the door, the door terminates TLS and the App
/// answers; the front's other routes are untouched.
#[tokio::test]
async fn front443_routes_a_custom_name_through_the_door() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    rig_app("jobs", "ws-a");
    let pem = self_signed(HOST);
    bind("jobs", HOST, domains::STATE_LIVE, Some(&pem));
    let port = app_backend().await;
    crate::apps::router::switch_with("jobs", port, true);
    reload();
    let door_addr = door().await;
    let mut t = RouteTable::default();
    t.insert(HOST, Backend { kind: Kind::App, addr: door_addr, proxy: true, refuse_loopback_clients: false }).unwrap();
    let table = Arc::new(t);
    let front = crate::front443::server::Front::with_limits(16, Duration::from_secs(5), Duration::from_secs(2), u32::MAX, Box::new(move || Arc::clone(&table)));
    let l = Arc::new(TcpListener::bind("127.0.0.1:0").await.unwrap());
    let front_addr = l.local_addr().unwrap();
    let task = tokio::spawn(crate::front443::server::accept_loop(l, Arc::clone(&front), tokio::runtime::Handle::current()));

    let tls = connect(front_addr, None, HOST, &client(&[&pem.der], &[b"h2", b"http/1.1"])).await.expect("through the front");
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(&b"http/1.1"[..]), "never h2 at the door");
    let mut s = BufReader::new(tls);
    write_all(s.get_mut(), b"GET / HTTP/1.1\r\nHost: app.example.com\r\nX-Forwarded-For: 6.6.6.6\r\n\r\n").await;
    let (status, body) = response(&mut s).await;
    assert_eq!(status, "HTTP/1.1 200 OK");
    // The test client is loopback, so front443's PROXY header names no
    // usable client: the spoofed header is still dropped.
    assert_eq!(body, "xff=;xri=;k2=;body=0");
    assert_eq!(front.counters.app.load(std::sync::atomic::Ordering::Relaxed), 1);
    task.abort();
    crate::apps::router::clear("jobs");
}

// ── what a custom name promises the App (0.46.6, the first customer's review) ──

/// An App that answers every request with its whole request head as the
/// body; on a WebSocket upgrade it answers 101, sends the head it got
/// followed by `\n--end--\n`, then echoes raw bytes.
async fn head_echo_backend() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let mut r = BufReader::new(s);
                loop {
                    let mut head = String::new();
                    loop {
                        let mut line = String::new();
                        if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        head.push_str(&line);
                        if line == "\r\n" {
                            break;
                        }
                    }
                    if head.to_ascii_lowercase().contains("\r\nupgrade: websocket\r\n") {
                        let w = r.get_mut();
                        w.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").await.unwrap();
                        w.write_all(head.as_bytes()).await.unwrap();
                        w.write_all(b"\n--end--\n").await.unwrap();
                        let mut buf = vec![0u8; 4096];
                        loop {
                            let n = match r.read(&mut buf).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => n,
                            };
                            if r.get_mut().write_all(&buf[..n]).await.is_err() {
                                return;
                            }
                        }
                    }
                    let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{head}", head.len());
                    if r.get_mut().write_all(resp.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

fn header_values(head: &str, name: &str) -> Vec<String> {
    head.lines()
        .filter_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
        .collect()
}

/// Requirements 1–3: `Host` passes through unchanged (the App's same-host
/// Origin check and canonical-host 308 use it); exactly one
/// `X-Forwarded-Proto: https` on every request (the App's Secure cookie);
/// the door answers with the App's own response, never a redirect of K2's
/// (the App does its own 308s). The App here has the client-IP rewrite OFF
/// in its manifest (a raw App): the door rewrites anyway.
#[tokio::test]
async fn the_door_keeps_host_says_https_and_never_redirects() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    rig_app("canon", "ws-a");
    let host = "canon.example.com";
    let pem = self_signed(host);
    bind("canon", host, domains::STATE_LIVE, Some(&pem));
    let port = head_echo_backend().await;
    crate::apps::router::switch_with("canon", port, false);
    reload();
    let addr = door().await;
    let c = client(&[&pem.der], &[b"http/1.1"]);
    let tls = connect(addr, Some("203.0.113.7:51000".parse().unwrap()), host, &c).await.unwrap();
    let mut s = BufReader::new(tls);
    for path in ["/", "/login?next=%2Fhome"] {
        write_all(
            s.get_mut(),
            format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nX-Forwarded-Proto: http\r\nX-K2-Client-IP: 6.6.6.6\r\n\r\n").as_bytes(),
        )
        .await;
        let (status, seen) = response(&mut s).await;
        assert_eq!(status, "HTTP/1.1 200 OK", "the App's own answer, no K2 redirect");
        assert!(seen.starts_with(&format!("GET {path} HTTP/1.1\r\n")), "path untouched: {seen}");
        assert_eq!(header_values(&seen, "host"), vec![host.to_string()], "Host unchanged: {seen}");
        assert_eq!(header_values(&seen, "x-forwarded-proto"), vec!["https".to_string()], "{seen}");
        assert_eq!(header_values(&seen, "x-k2-client-ip"), vec!["203.0.113.7".to_string()], "{seen}");
        assert!(!seen.contains("6.6.6.6"), "{seen}");
    }
    crate::apps::router::clear("canon");
}

/// Requirements 4–5: a WebSocket upgrade on the same host (the App's
/// stream and activity paths) carries the cookie, the Bearer and
/// `?session=` untouched, plus K2's `X-K2-Client-IP` and `X-Forwarded-Proto:
/// https` on the upgrade request itself (forged copies dropped); then the
/// socket stays open across an idle gap (no idle timeout).
#[tokio::test]
async fn a_websocket_upgrade_carries_k2s_client_ip_and_the_apps_credentials() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    rig_app("live", "ws-a");
    let host = "live.example.com";
    let pem = self_signed(host);
    bind("live", host, domains::STATE_LIVE, Some(&pem));
    let port = head_echo_backend().await;
    crate::apps::router::switch_with("live", port, true);
    reload();
    let addr = door().await;
    let c = client(&[&pem.der], &[b"http/1.1"]);
    for path in ["/api/stream", "/api/activity/events?session=s-123", "/overlay/ws"] {
        let tls = connect(addr, Some("198.51.100.9:40001".parse().unwrap()), host, &c).await.unwrap();
        let mut s = BufReader::new(tls);
        write_all(
            s.get_mut(),
            format!(
                "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                 Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
                 Cookie: sid=abc\r\nAuthorization: Bearer tok-1\r\nX-Forwarded-For: 6.6.6.6\r\nX-K2-Client-IP: 6.6.6.6\r\n\r\n"
            )
            .as_bytes(),
        )
        .await;
        let mut status = String::new();
        s.read_line(&mut status).await.unwrap();
        assert!(status.starts_with("HTTP/1.1 101"), "{status}");
        loop {
            let mut h = String::new();
            s.read_line(&mut h).await.unwrap();
            if h.trim_end().is_empty() {
                break;
            }
        }
        let mut seen = String::new();
        while !seen.ends_with("\n--end--\n") {
            let mut line = String::new();
            assert!(s.read_line(&mut line).await.unwrap() > 0, "closed early: {seen}");
            seen.push_str(&line);
        }
        assert!(seen.starts_with(&format!("GET {path} HTTP/1.1\r\n")), "path and ?session= untouched: {seen}");
        assert_eq!(header_values(&seen, "host"), vec![host.to_string()]);
        assert_eq!(header_values(&seen, "cookie"), vec!["sid=abc".to_string()]);
        assert_eq!(header_values(&seen, "authorization"), vec!["Bearer tok-1".to_string()]);
        assert_eq!(header_values(&seen, "x-k2-client-ip"), vec!["198.51.100.9".to_string()], "on the upgrade request: {seen}");
        assert_eq!(header_values(&seen, "x-forwarded-for"), vec!["198.51.100.9".to_string()]);
        assert_eq!(header_values(&seen, "x-forwarded-proto"), vec!["https".to_string()]);
        assert!(!seen.contains("6.6.6.6"), "{seen}");
        tokio::time::sleep(Duration::from_secs(2)).await;
        write_all(s.get_mut(), b"frame-after-idle").await;
        let mut back = [0u8; 16];
        tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut back)).await.expect("echo in time").expect("still open");
        assert_eq!(&back, b"frame-after-idle");
    }
    crate::apps::router::clear("live");
}
