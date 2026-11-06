//! The signalling door: which heads are the native engine's, what crosses to
//! the engine, and what never does.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use super::*;

const JWT: &str = "eyJhbGciOiJFUzI1NiJ9.eyJzdWIiOiJhY21lIn0.c2lnbmF0dXJl";
const QUERY: &str = "join_request=Q2dFeE1qTT0";

fn native(extra: &str) -> String {
    format!(
        "GET /rtc/v1?{QUERY} HTTP/1.1\r\nHost: jobs.acme.k2.dev\r\nAuthorization: Bearer {JWT}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: abc==\r\nSec-WebSocket-Version: 13\r\n{extra}\r\n"
    )
}

#[test]
fn only_the_native_engines_socket_is_the_doors() {
    assert_eq!(classify(&native("")), Class::Native);
    assert_eq!(classify(&native("").replace("Bearer", "bearer")), Class::Native, "the scheme is case-insensitive");
    assert_eq!(classify(&native("").replace(&format!("?{QUERY}"), "")), Class::Native, "a query is optional");

    let cases: Vec<(&str, String)> = vec![
        ("a browser's cookie", native("").replace(&format!("Authorization: Bearer {JWT}\r\n"), "Cookie: app_session=abc\r\n")),
        (
            "a browser's query token",
            native("").replace(&format!("Authorization: Bearer {JWT}\r\n"), "").replace(QUERY, "access_token=tok"),
        ),
        ("an App pass", native("").replace(JWT, "k2skn_pass.secret")),
        ("a Sew session", native("").replace(JWT, "sews_sess.secret")),
        ("a call credential", native("").replace(JWT, "sewcc_sel.secret")),
        ("another scheme", native("").replace("Bearer ", "Basic ")),
        ("an empty bearer", native("").replace(&format!("Bearer {JWT}"), "Bearer ")),
        ("a bearer with a space", native("").replace(JWT, "abc def")),
        ("a token too long", native("").replace(JWT, &"a".repeat(MAX_TOKEN + 1))),
        ("two Authorization headers", native(&format!("Authorization: Bearer {JWT}\r\n"))),
        ("no upgrade", native("").replace("Upgrade: websocket\r\n", "")),
        ("a POST", native("").replacen("GET ", "POST ", 1)),
        ("/rtc", native("").replacen("/rtc/v1", "/rtc", 1)),
        ("/rtc/validate", native("").replacen("/rtc/v1", "/rtc/validate", 1)),
        ("/rtc/v1/validate", native("").replacen("/rtc/v1", "/rtc/v1/validate", 1)),
        ("/rtc/v1/", native("").replacen("/rtc/v1", "/rtc/v1/", 1)),
        ("/rtc/v1x", native("").replacen("/rtc/v1", "/rtc/v1x", 1)),
        ("a prefix", native("").replacen("/rtc/v1", "/app/rtc/v1", 1)),
        ("no request line", String::new()),
    ];
    for (what, head) in cases {
        assert_eq!(classify(&head), Class::App, "{what} is the App's: {head}");
    }
    assert_eq!(classify(&native("").replace(JWT, "k2sknx.secret")), Class::Native, "only the exact K2 prefixes are K2's");
}

#[test]
fn native_head_needs_a_whole_head() {
    let req = native("");
    let (head, len) = native_head(req.as_bytes()).expect("the door's");
    assert_eq!((head.as_str(), len), (req.as_str(), req.len()));
    assert!(native_head(&req.as_bytes()[..req.len() - 2]).is_none(), "an incomplete head");
    assert!(native_head(b"GET / HTTP/1.1\r\n\r\n").is_none());
}

#[test]
fn the_engine_gets_k2s_address_and_never_a_clients() {
    let spoofed = "X-Forwarded-For: 198.51.100.66\r\nForwarded: for=198.51.100.67\r\nX-Real-IP: 198.51.100.68\r\nX-K2-Client-IP: 198.51.100.69\r\n\
                   Cookie: app_session=abc\r\nOrigin: https://evil.example\r\nSec-WebSocket-Protocol: chat\r\nSec-WebSocket-Extensions: permessage-deflate\r\nUser-Agent: acme-phone/1\r\n";
    let ip: IpAddr = "203.0.113.9".parse().expect("ip");
    let out = upstream_head(&native(spoofed), "127.0.0.1:7880", Some(ip)).expect("the door's");
    assert!(out.starts_with(&format!("GET /rtc/v1?{QUERY} HTTP/1.1\r\nHost: 127.0.0.1:7880\r\n")), "path and query unchanged: {out}");
    assert!(out.contains(&format!("\r\nAuthorization: Bearer {JWT}\r\n")), "{out}");
    for line in [
        "Connection: Upgrade",
        "Upgrade: websocket",
        "sec-websocket-key: abc==",
        "sec-websocket-version: 13",
        "sec-websocket-extensions: permessage-deflate",
        "user-agent: acme-phone/1",
        "X-Forwarded-For: 203.0.113.9",
    ] {
        assert!(out.contains(&format!("\r\n{line}\r\n")), "{line}: {out}");
    }
    assert!(out.ends_with("\r\n\r\n"), "{out}");
    assert_eq!(out.matches("X-Forwarded-For").count(), 1, "{out}");
    for gone in ["198.51.100", "forwarded:", "x-real-ip", "x-k2-client-ip", "cookie", "origin", "sec-websocket-protocol", "jobs.acme"] {
        assert!(!out.to_ascii_lowercase().contains(gone), "{gone} crossed: {out}");
    }

    let out = upstream_head(&native(spoofed), "127.0.0.1:7880", None).expect("the door's");
    assert!(!out.to_ascii_lowercase().contains("forwarded"), "no address known: none sent, never the client's: {out}");
    assert!(upstream_head(&native("").replace(JWT, "sewcc_a.b"), "127.0.0.1:7880", None).is_none(), "a K2 credential never crosses");
}

#[test]
fn a_log_line_never_carries_the_token_or_the_query() {
    let line = log_line("refused", &native("X-Forwarded-For: 198.51.100.66\r\n"));
    assert_eq!(line, "[voice/signal-lane] refused: GET /rtc/v1");
    assert!(!line.contains(JWT) && !line.contains("join_request"), "{line}");
}

/// A fake signal listener: hands the handshake to the test, answers 101, then
/// echoes.
async fn fake_engine() -> (String, mpsc::UnboundedReceiver<String>) {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = l.local_addr().expect("addr").to_string();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.expect("accept");
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut got = Vec::new();
                let mut b = [0u8; 8192];
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = s.read(&mut b).await.expect("read");
                    assert!(n > 0, "closed before a whole head");
                    got.extend_from_slice(&b[..n]);
                }
                tx.send(String::from_utf8_lossy(&got).to_string()).expect("send");
                s.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").await.expect("101");
                loop {
                    let n = match s.read(&mut b).await {
                        Ok(n) if n > 0 => n,
                        _ => return,
                    };
                    s.write_all(&b[..n]).await.expect("echo");
                }
            });
        }
    });
    (addr, rx)
}

#[tokio::test]
async fn the_splice_carries_frames_both_ways_and_a_large_server_message() {
    let (addr, mut seen) = fake_engine().await;
    let (mut client, mut server) = tokio::io::duplex(4 * 1024 * 1024);
    let req = native("X-Forwarded-For: 198.51.100.66\r\n");
    client.write_all(req.as_bytes()).await.expect("handshake");
    let ip: IpAddr = "2001:db8::7".parse().expect("ip");
    let head = req.clone();
    let task = tokio::spawn(async move { serve(&mut server, &head, head.len(), &addr, Some(ip)).await });

    let mut got = Vec::new();
    let mut b = [0u8; 1024];
    while !got.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(5), client.read(&mut b)).await.expect("in time").expect("read");
        assert!(n > 0, "closed in the handshake");
        got.extend_from_slice(&b[..n]);
    }
    assert!(got.starts_with(b"HTTP/1.1 101"), "{}", String::from_utf8_lossy(&got));
    let up = seen.recv().await.expect("the engine saw the handshake");
    assert!(up.contains("\r\nX-Forwarded-For: 2001:db8::7\r\n") && !up.contains("198.51.100.66"), "{up}");

    // 2 MiB, well above 1 MiB, crosses whole.
    let big = vec![b'x'; 2 * 1024 * 1024];
    client.write_all(&big).await.expect("big");
    let mut back = vec![0u8; big.len()];
    tokio::time::timeout(Duration::from_secs(10), client.read_exact(&mut back)).await.expect("in time").expect("echo");
    assert!(back == big, "the echo came back whole");
    drop(client);
    tokio::time::timeout(Duration::from_secs(5), task).await.expect("the splice ends when the client goes").expect("join");
}

#[tokio::test]
async fn an_unreachable_engine_is_a_502_without_the_token() {
    let free = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = free.local_addr().expect("addr").to_string();
    drop(free);
    let (mut client, mut server) = tokio::io::duplex(64 * 1024);
    let req = native("");
    client.write_all(req.as_bytes()).await.expect("handshake");
    serve(&mut server, &req, req.len(), &addr, None).await;
    drop(server);
    let mut out = String::new();
    client.read_to_string(&mut out).await.expect("read");
    assert!(out.starts_with("HTTP/1.1 502") && out.contains("voice_unavailable"), "{out}");
    assert!(!out.contains(JWT) && !out.contains("join_request"), "{out}");
}
