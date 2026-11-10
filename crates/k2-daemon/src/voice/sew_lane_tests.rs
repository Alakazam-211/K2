//! The lane against a fake Sew on a loopback port: what crosses and what doesn't.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use super::*;

const TOKEN: &str = "sewcc_selA.secretA";

/// A fake Sew: hands each request's raw bytes to the test and answers `reply`.
/// With `upgrade`, it answers 101 and then echoes what it reads.
async fn fake_sew(reply: &'static str, upgrade: bool) -> (String, mpsc::UnboundedReceiver<String>) {
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
                loop {
                    let n = s.read(&mut b).await.expect("read");
                    got.extend_from_slice(&b[..n]);
                    let text = String::from_utf8_lossy(&got).to_string();
                    if let Some(i) = text.find("\r\n\r\n") {
                        let len = text
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().expect("len")))
                            .unwrap_or(0);
                        if got.len() >= i + 4 + len {
                            tx.send(text).expect("send");
                            break;
                        }
                    }
                    assert!(n > 0, "the lane closed before a whole request");
                }
                s.write_all(reply.as_bytes()).await.expect("reply");
                if !upgrade {
                    return;
                }
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

/// One request through the lane; returns what the client read.
async fn through(addr: &str, preview: bool, req: &str) -> String {
    let head_len = req.find("\r\n\r\n").expect("head") + 4;
    let head = req[..head_len].to_string();
    let (mut client, mut lane) = tokio::io::duplex(64 * 1024);
    client.write_all(req.as_bytes()).await.expect("write");
    let addr = addr.to_string();
    let task = tokio::spawn(async move {
        serve_to(&mut lane, &head, head_len, preview, &addr).await;
        lane.shutdown().await.expect("shutdown");
    });
    let mut out = Vec::new();
    client.read_to_end(&mut out).await.expect("read");
    task.await.expect("lane task");
    String::from_utf8_lossy(&out).to_string()
}

fn post(path: &str, extra: &str, body: &str) -> String {
    format!("POST /_k2/voice/sew{path} HTTP/1.1\r\nHost: lab.k2.dev\r\nContent-Length: {}\r\n{extra}\r\n{body}", body.len())
}

#[test]
fn only_sews_allowlist_passes() {
    for (m, p) in [
        ("POST", "/api/v1/devices"),
        ("POST", "/api/v1/devices/d_1/renew"),
        ("GET", "/api/v1/me/devices"),
        ("GET", "/api/v1/devices/crl"),
        ("POST", "/api/v1/me/voice/keys/key_packages"),
        ("POST", "/api/v1/me/voice/keys/join"),
        ("POST", "/api/v1/calls/r_abc/keys/state"),
        ("POST", "/api/v1/calls/r_abc/keys/commit"),
        ("POST", "/api/v1/calls/r_abc/keys/ready"),
        ("POST", "/api/v1/calls/r_abc/keys/invalid"),
        ("POST", "/api/v1/calls/r_abc/keys/key_package"),
        ("POST", "/api/v1/calls/r_abc/keys/release"),
        ("POST", "/api/v1/calls/r_abc/keys/rekey"),
        ("POST", "/api/v1/calls/r_abc/keys/report"),
        ("GET", "/api/v1/me/voice"),
        ("POST", "/api/v1/me/voice/state"),
        ("POST", "/api/v1/me/voice/leave"),
        ("POST", "/api/v1/me/voice/stats"),
        ("POST", "/api/v1/me/voice/credential"),
    ] {
        assert_eq!(allowed(m, p), Some(Route::Http), "{m} {p}");
    }
    assert_eq!(allowed("GET", "/api/v1/gateway"), Some(Route::Gateway));
    for (m, p) in [
        ("GET", "/api/v1/gateway/ticket"),
        ("POST", "/api/v1/gateway/ticket"),
        ("POST", "/api/v1/gateway"),
        ("GET", "/api/v1/devices"),
        ("POST", "/api/v1/me/voice"),
        ("GET", "/api/v1/me/voice/leave"),
        ("POST", "/api/v1/auth/k2"),
        ("GET", "/api/v1/spaces"),
        ("GET", "/api/v1/users/@me"),
        ("POST", "/api/v1/channels/1/voice/join"),
        ("POST", "/api/v1/calls/r_abc/keys/other"),
        ("POST", "/api/v1/calls/../keys/state"),
        ("POST", "/api/v1/calls/a%2fb/keys/state"),
        ("POST", "/api/v1/devices/../renew"),
        ("POST", "/api/v1/me/voice/keys"),
        ("GET", "/mgmt/v1/local/users"),
        ("GET", "/"),
        ("GET", ""),
    ] {
        assert_eq!(allowed(m, p), None, "{m} {p}");
    }
}

#[test]
fn the_bearer_must_be_a_call_credential() {
    let h = |v: &str| format!("GET / HTTP/1.1\r\nAuthorization: {v}\r\n\r\n");
    assert_eq!(sewcc_bearer(&h("Bearer sewcc_sel.secret")), Some("sewcc_sel.secret"));
    assert_eq!(sewcc_bearer(&h("Bearer owner-token")), None);
    assert_eq!(sewcc_bearer(&h("Bearer k2skn_abc")), None);
    assert_eq!(sewcc_bearer(&h("Bearer sewcc_")), None);
    assert_eq!(sewcc_bearer(&h("Basic sewcc_sel.secret")), None);
    assert_eq!(sewcc_bearer(&h(&format!("Bearer sewcc_{}", "x".repeat(600)))), None);
    assert_eq!(sewcc_bearer("GET / HTTP/1.1\r\nCookie: k2_session=x\r\n\r\n"), None);
    assert_eq!(selector("sewcc_sel.secret"), "sel");
}

#[tokio::test]
async fn forwards_with_the_prefix_stripped_and_nothing_else_crosses() {
    let (addr, mut seen) = fake_sew("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}", false).await;
    let req = post(
        "/api/v1/me/voice/stats?x=1",
        &format!("Authorization: Bearer {TOKEN}\r\nCookie: k2_session=abc\r\nContent-Type: application/json\r\nX-K2-Whatever: y\r\nX-Forwarded-For: 1.2.3.4\r\n"),
        r#"{"a":1}"#,
    );
    let out = through(&addr, true, &req).await;
    assert!(out.starts_with("HTTP/1.1 200 OK"), "{out}");
    assert!(out.ends_with("{}"), "{out}");
    let got = seen.recv().await.expect("Sew saw it");
    assert!(got.starts_with("POST /api/v1/me/voice/stats?x=1 HTTP/1.1\r\n"), "{got}");
    assert!(got.contains(&format!("Authorization: Bearer {TOKEN}\r\n")), "{got}");
    assert!(got.contains("content-type: application/json\r\n"), "{got}");
    assert!(got.ends_with(r#"{"a":1}"#), "{got}");
    let lower = got.to_ascii_lowercase();
    for gone in ["cookie", "x-k2-whatever", "x-forwarded-for", "lab.k2.dev"] {
        assert!(!lower.contains(gone), "{gone} crossed: {got}");
    }
    assert_eq!(lower.matches("authorization:").count(), 1, "{got}");
}

#[tokio::test]
async fn refusals_are_answered_here_and_never_reach_sew() {
    let (addr, mut seen) = fake_sew("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", false).await;
    let auth = format!("Authorization: Bearer {TOKEN}\r\n");

    let out = through(&addr, false, &post("/api/v1/me/voice/stats", &auth, "{}")).await;
    assert!(out.starts_with("HTTP/1.1 404") && out.contains("voice_rooms_off"), "preview off: {out}");
    let out = through(&addr, true, &post("/api/v1/auth/k2", &auth, "{}")).await;
    assert!(out.starts_with("HTTP/1.1 404"), "off the list: {out}");
    let get_ticket = format!("GET /_k2/voice/sew/api/v1/gateway/ticket HTTP/1.1\r\n{auth}\r\n");
    assert!(through(&addr, true, &get_ticket).await.starts_with("HTTP/1.1 404"));
    let out = through(&addr, true, &post("/api/v1/me/voice/stats", "Authorization: Bearer owner-token\r\n", "{}")).await;
    assert!(out.starts_with("HTTP/1.1 401") && !out.contains("owner-token"), "not sewcc_: {out}");
    let out = through(&addr, true, &post("/api/v1/me/voice/stats", "Cookie: k2_session=abc\r\n", "{}")).await;
    assert!(out.starts_with("HTTP/1.1 401"), "cookie only: {out}");
    let big = format!("POST /_k2/voice/sew/api/v1/me/voice/stats HTTP/1.1\r\n{auth}Content-Length: {}\r\n\r\n", MAX_BODY + 1);
    assert!(through(&addr, true, &big).await.starts_with("HTTP/1.1 413"));
    // Rev 2 point 2: the gateway without an upgrade is not a route.
    for target in ["/_k2/voice/sew/api/v1/gateway", "/_k2/voice/sew/api/v1/gateway?v=1&encoding=json"] {
        let plain_get = format!("GET {target} HTTP/1.1\r\n{auth}\r\n");
        let out = through(&addr, true, &plain_get).await;
        assert!(out.starts_with("HTTP/1.1 404") && out.contains(r#""code":"not_found""#), "gateway without upgrade: {out}");
    }

    assert!(tokio::time::timeout(Duration::from_millis(100), seen.recv()).await.is_err(), "something reached Sew");
}

#[tokio::test]
async fn sew_down_is_a_clean_answer_with_no_credential_in_it() {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = l.local_addr().expect("addr").to_string();
    drop(l);
    let out = through(&addr, true, &post("/api/v1/me/voice/state", "Authorization: Bearer sewcc_down.secretdown\r\n", "{}")).await;
    assert!(out.starts_with("HTTP/1.1 502") && out.contains("voice_unavailable"), "{out}");
    assert!(!out.contains("secretdown"), "{out}");
}

#[tokio::test]
async fn never_relays_off_this_box() {
    let out = through("192.0.2.1:8470", true, &post("/api/v1/me/voice/state", "Authorization: Bearer sewcc_far.secret\r\n", "{}")).await;
    assert!(out.starts_with("HTTP/1.1 502"), "{out}");
}

#[test]
fn the_rate_limit_is_per_credential_and_refills() {
    let t0 = Instant::now();
    let burst = RATE_BURST as usize;
    for i in 0..burst {
        assert!(take_token("rate-a", t0), "request {i} of the burst");
    }
    assert!(!take_token("rate-a", t0), "past the burst");
    assert!(take_token("rate-b", t0), "another credential has its own budget");
    assert!(take_token("rate-a", t0 + Duration::from_secs(1)), "a second later it refilled");
}

#[tokio::test]
async fn past_the_rate_limit_answers_429_and_nothing_crosses() {
    let (addr, mut seen) = fake_sew("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", false).await;
    let now = Instant::now();
    while take_token("rate-http", now) {}
    let out = through(&addr, true, &post("/api/v1/me/voice/state", "Authorization: Bearer sewcc_rate-http.secret\r\n", "{}")).await;
    assert!(out.starts_with("HTTP/1.1 429") && out.contains("Retry-After: 2"), "{out}");
    assert!(tokio::time::timeout(Duration::from_millis(100), seen.recv()).await.is_err());
}

/// Open a gateway socket through the lane and return the client end, after the
/// handshake reply.
async fn open_gateway(addr: &str, cred: &str) -> (tokio::io::DuplexStream, tokio::task::JoinHandle<()>) {
    let req = format!(
        "GET /_k2/voice/sew/api/v1/gateway?v=1&encoding=json HTTP/1.1\r\nHost: lab.k2.dev\r\nAuthorization: Bearer {cred}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: abc==\r\nSec-WebSocket-Version: 13\r\nCookie: k2_session=x\r\n\r\n"
    );
    let head_len = req.len();
    let (mut client, mut lane) = tokio::io::duplex(64 * 1024);
    client.write_all(req.as_bytes()).await.expect("write");
    let addr = addr.to_string();
    let task = tokio::spawn(async move {
        serve_to(&mut lane, &req, head_len, true, &addr).await;
    });
    let mut got = Vec::new();
    let mut b = [0u8; 1024];
    while !got.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = client.read(&mut b).await.expect("handshake");
        assert!(n > 0, "closed in the handshake");
        got.extend_from_slice(&b[..n]);
    }
    assert!(got.starts_with(b"HTTP/1.1 101"), "{}", String::from_utf8_lossy(&got));
    (client, task)
}

#[tokio::test]
async fn the_gateway_splices_and_a_second_socket_on_one_credential_passes_too() {
    let (addr, mut seen) = fake_sew("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n", true).await;
    let cred = "sewcc_ws-a.secret";
    let (mut first, first_task) = open_gateway(&addr, cred).await;
    let got = seen.recv().await.expect("Sew saw the handshake");
    assert!(got.starts_with("GET /api/v1/gateway?v=1&encoding=json HTTP/1.1\r\n"), "{got}");
    assert!(got.contains("sec-websocket-key: abc==\r\n"), "{got}");
    assert!(!got.to_ascii_lowercase().contains("cookie"), "{got}");

    first.write_all(b"ping").await.expect("frame");
    let mut b = [0u8; 4];
    first.read_exact(&mut b).await.expect("echo");
    assert_eq!(&b, b"ping");

    // Rev 2 point 4: a second socket with the same credential reaches Sew
    // too, and K2 cuts neither: the takeover (and its 4011) is Sew's.
    let (mut second, second_task) = open_gateway(&addr, cred).await;
    let got = seen.recv().await.expect("Sew saw the second handshake");
    assert!(got.starts_with("GET /api/v1/gateway?v=1&encoding=json HTTP/1.1\r\n"), "{got}");
    second.write_all(b"pong").await.expect("frame");
    second.read_exact(&mut b).await.expect("echo");
    assert_eq!(&b, b"pong");
    first.write_all(b"more").await.expect("frame");
    tokio::time::timeout(Duration::from_secs(5), first.read_exact(&mut b)).await.expect("in time").expect("the first is still open");
    assert_eq!(&b, b"more");

    drop(first);
    tokio::time::timeout(Duration::from_secs(5), first_task).await.expect("closed").expect("task");
    second.write_all(b"last").await.expect("frame");
    second.read_exact(&mut b).await.expect("the second outlives the first");
    assert_eq!(&b, b"last");
    drop(second);
    tokio::time::timeout(Duration::from_secs(5), second_task).await.expect("closed").expect("task");
}

/// K2 PRD prd-k2-bobbin-in-apps-v1 §8 K-f (pin: passes today): the call
/// door splices Sew's gateway byte for byte, so Sew 0.9.0's
/// `recording_updated` (with `trigger` and `required`) reaches the engine
/// unchanged, with no K2 change.
#[tokio::test]
async fn a_recording_updated_frame_crosses_the_gateway_splice() {
    let payload = br#"{"op":0,"t":"recording_updated","s":7,"d":{"recording":{"id":"77","channel_id":"10","trigger":"required","required":true}}}"#;
    let mut frame = vec![0x81u8, 126, (payload.len() >> 8) as u8, (payload.len() & 0xff) as u8];
    frame.extend_from_slice(payload);
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = l.local_addr().expect("addr").to_string();
    let sent = frame.clone();
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.expect("accept");
        let mut got = Vec::new();
        let mut b = [0u8; 4096];
        while !got.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = s.read(&mut b).await.expect("read");
            assert!(n > 0, "closed in the handshake");
            got.extend_from_slice(&b[..n]);
        }
        s.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").await.expect("101");
        // The client says it has the 101 (a byte through the splice), then
        // Sew's frame follows.
        let n = s.read(&mut b).await.expect("the client's go");
        assert!(n > 0, "the client's go");
        s.write_all(&sent).await.expect("frame");
        let _ = s.read(&mut b).await;
    });
    let (mut client, task) = open_gateway(&addr, "sewcc_rec-a.secret").await;
    client.write_all(b"go").await.expect("go");
    let mut got = vec![0u8; frame.len()];
    tokio::time::timeout(Duration::from_secs(5), client.read_exact(&mut got)).await.expect("in time").expect("the frame");
    assert_eq!(got, frame, "byte for byte");
    drop(client);
    tokio::time::timeout(Duration::from_secs(5), task).await.expect("closed").expect("task");
}

#[tokio::test]
async fn a_refused_handshake_passes_through_and_opens_no_socket() {
    let (addr, _seen) = fake_sew("HTTP/1.1 403 Forbidden\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}", false).await;
    let req = "GET /_k2/voice/sew/api/v1/gateway HTTP/1.1\r\nAuthorization: Bearer sewcc_ws-refused.secret\r\nUpgrade: websocket\r\n\r\n";
    let out = through(&addr, true, req).await;
    assert!(out.starts_with("HTTP/1.1 403"), "{out}");
}

/// Rev 2 point 1: `{id}` and `{call_id}` are one segment of
/// `[A-Za-z0-9_-]{1,128}`; `..`, any `%`-escape and empty segments never pass.
#[test]
fn path_parameters_are_one_plain_segment() {
    let ok = "a".repeat(128);
    assert_eq!(allowed("POST", &format!("/api/v1/devices/{ok}/renew")), Some(Route::Http));
    assert_eq!(allowed("POST", &format!("/api/v1/calls/r_{}/keys/state", "b".repeat(126))), Some(Route::Http));
    assert_eq!(allowed("POST", "/api/v1/calls/r_Abc-9_x/keys/commit"), Some(Route::Http));
    let long = "a".repeat(129);
    for (m, p) in [
        ("POST", format!("/api/v1/devices/{long}/renew")),
        ("POST", format!("/api/v1/calls/{long}/keys/state")),
        ("POST", "/api/v1/devices/../renew".to_string()),
        ("POST", "/api/v1/devices/a..b/renew".to_string()),
        ("POST", "/api/v1/devices/%2e%2e/renew".to_string()),
        ("POST", "/api/v1/devices/a%2Fb/renew".to_string()),
        ("POST", "/api/v1/calls/r_1%00/keys/state".to_string()),
        ("POST", "/api/v1/devices//renew".to_string()),
        ("POST", "/api/v1/calls//keys/state".to_string()),
        ("POST", "/api/v1/calls/r_1/keys//state".to_string()),
        ("POST", "/api/v1//devices".to_string()),
        ("POST", "/api/v1/devices/".to_string()),
        ("GET", "/api/v1/gateway/".to_string()),
        ("POST", "/api/v1/calls/r_1/keys/state/".to_string()),
        ("POST", "/api/v1/calls/r.1/keys/state".to_string()),
    ] {
        assert_eq!(allowed(m, &p), None, "{m} {p}");
    }
}

/// Rev 2 point 2: query strings pass through unchanged.
#[tokio::test]
async fn query_strings_pass_through_unchanged() {
    let (addr, mut seen) = fake_sew("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", false).await;
    for (target, q) in [
        ("/api/v1/devices/crl", "?after=1730851200"),
        ("/api/v1/devices/crl", "?after=2026-11-05T10%3A00%3A00Z&limit=50"),
        ("/api/v1/me/voice", "?v=1&encoding=json"),
    ] {
        let req = format!("GET /_k2/voice/sew{target}{q} HTTP/1.1\r\nAuthorization: Bearer sewcc_q.secret\r\n\r\n");
        let out = through(&addr, true, &req).await;
        assert!(out.starts_with("HTTP/1.1 200"), "{out}");
        let got = seen.recv().await.expect("Sew saw it");
        assert!(got.starts_with(&format!("GET {target}{q} HTTP/1.1\r\n")), "{got}");
    }
}

/// Rev 2 point 3: a call credential with a cookie is forwarded with the
/// cookie stripped; a cookie alone is the door's own 401.
#[tokio::test]
async fn a_cookie_is_stripped_never_refused_and_never_enough() {
    let (addr, mut seen) = fake_sew("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", false).await;
    let both = "Authorization: Bearer sewcc_ck.secret\r\nCookie: k2_session=abc\r\ncookie: other=1\r\n";
    let out = through(&addr, true, &post("/api/v1/me/voice/state", both, "{}")).await;
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    let got = seen.recv().await.expect("Sew saw it");
    assert!(got.starts_with("POST /api/v1/me/voice/state HTTP/1.1\r\n") && got.contains("Authorization: Bearer sewcc_ck.secret\r\n"), "{got}");
    assert!(!got.to_ascii_lowercase().contains("cookie"), "{got}");
    let out = through(&addr, true, &post("/api/v1/me/voice/state", "Cookie: k2_session=abc\r\n", "{}")).await;
    assert!(out.starts_with("HTTP/1.1 401") && out.contains("call_credential_required"), "{out}");
    assert!(tokio::time::timeout(Duration::from_millis(100), seen.recv()).await.is_err(), "a cookie alone reached Sew");
}
