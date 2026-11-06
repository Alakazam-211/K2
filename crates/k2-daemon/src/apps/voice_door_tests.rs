//! The call door on an App's addresses: the gate, and the real App path
//! (`apps::router`) against a fake Sew and a fake App on loopback ports.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use super::*;

fn facts(voice: bool, sew_api: bool, suspended: bool, ceiling: AppChatCeiling) -> Facts {
    Facts { voice, sew_api, suspended, ceiling }
}

fn declared(caps: &[&str]) -> AppChatCeiling {
    AppChatCeiling::Declared(caps.iter().map(|c| c.to_string()).collect())
}

#[test]
fn the_gate_needs_voice_sew_api_and_voice_join() {
    let open = declared(&["chat:read", "voice:join"]);
    assert!(open_of(&facts(true, true, false, open.clone())), "every fact holds");
    assert!(open_of(&facts(true, true, false, AppChatCeiling::NotDeclared)), "no chat caps asked (Hearth, Meet): the roles and Sew decide");
    assert!(!open_of(&facts(false, true, false, open.clone())), "no voice on this box");
    assert!(!open_of(&facts(true, false, false, open.clone())), "sew-api not approved");
    assert!(!open_of(&facts(true, true, true, open.clone())), "suspended");
    assert!(!open_of(&facts(true, true, true, AppChatCeiling::NotDeclared)), "suspended, no chat caps");
    assert!(!open_of(&facts(true, true, false, declared(&["chat:read", "chat:post"]))), "chat caps without voice:join");
    assert!(!open_of(&facts(true, true, false, declared(&[]))), "chat caps waiting for approval");
    assert!(!open_of(&facts(true, true, false, AppChatCeiling::NotManaged)), "not a runtime App");
}

#[test]
fn lane_head_finds_only_the_door() {
    let (head, len) = lane_head(b"POST /_k2/voice/sew/api/v1/me/voice/stats HTTP/1.1\r\nHost: a\r\nContent-Length: 2\r\n\r\n{}").expect("door");
    assert!(head.starts_with("POST /_k2/voice/sew/api/v1/me/voice/stats ") && head.ends_with("\r\n\r\n"), "{head}");
    assert_eq!(len, head.len(), "the head, without the body");
    assert!(lane_head(b"GET /_k2/voice/sew/api/v1/gateway?v=1 HTTP/1.1\r\n\r\n").is_some(), "a query is fine");
    assert!(lane_head(b"GET /_k2/voice/sewx/api HTTP/1.1\r\n\r\n").is_none());
    assert!(lane_head(b"GET /_k2/voice/sew HTTP/1.1\r\n\r\n").is_none(), "the bare prefix is the App's");
    assert!(lane_head(b"GET /x/_k2/voice/sew/ HTTP/1.1\r\n\r\n").is_none());
    assert!(lane_head(b"GET /_k2/voice/sew/api/v1/me/voice HTTP/1.1\r\nHost: a\r\n").is_none(), "an incomplete head");
    assert!(lane_head(b"").is_none());
}

/// A fake Sew: hands each request's raw bytes to the test and answers
/// `reply`. With `upgrade`, it then echoes what it reads.
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
                    assert!(n > 0, "the door closed before a whole request");
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

/// A fake App that answers every request `app` and counts its dials.
async fn fake_app() -> (u16, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = l.local_addr().expect("addr").port();
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
                        if s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\napp").await.is_err() {
                            return;
                        }
                        let _ = s.shutdown().await;
                        return;
                    }
                }
            });
        }
    });
    (port, n)
}

/// One connection on the App's path (what the E2E listener, the App TLS door
/// and the front port all run), as the App's front port serves it.
async fn via_app(app_id: &str) -> TcpStream {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = l.local_addr().expect("addr");
    let id = app_id.to_string();
    tokio::spawn(async move {
        let (mut s, peer) = l.accept().await.expect("accept");
        crate::apps::router::serve_app_conn(&mut s, &id, Some(peer)).await;
    });
    TcpStream::connect(addr).await.expect("connect")
}

async fn read_all(mut s: TcpStream) -> String {
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out)).await.expect("closed in time");
    String::from_utf8_lossy(&out).to_string()
}

async fn ask(app_id: &str, req: &str) -> String {
    let mut c = via_app(app_id).await;
    c.write_all(req.as_bytes()).await.expect("write");
    read_all(c).await
}

async fn nothing_reached(seen: &mut mpsc::UnboundedReceiver<String>, what: &str) {
    if let Ok(Some(got)) = tokio::time::timeout(Duration::from_millis(150), seen.recv()).await {
        panic!("{what} reached Sew: {got}");
    }
}

fn post(path: &str, extra: &str, body: &str) -> String {
    format!("POST /_k2/voice/sew{path} HTTP/1.1\r\nHost: jobs.acme.k2.dev\r\nContent-Length: {}\r\n{extra}\r\n{body}", body.len())
}

const BEARER: &str = "Authorization: Bearer sewcc_vdsel.vdsecret\r\n";

#[tokio::test]
async fn an_app_host_with_the_door_relays_the_allowlist_and_refuses_the_rest() {
    let _home = crate::test_support::TempHome::new();
    let (sew, mut seen) = fake_sew("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}", false).await;
    let (app_port, dials) = fake_app().await;
    let id = "voice-door-open";
    let _ = crate::apps::router::switch(id, app_port);
    set_test_door(id, true, &sew);

    // Allowed: stripped of the prefix, the bearer and the body cross; the
    // cookie doesn't; the App is never dialed. The body comes in a later
    // write than the head.
    let mut c = via_app(id).await;
    let req = post("/api/v1/me/voice/stats?x=1", &format!("{BEARER}Cookie: app_session=abc\r\nContent-Type: application/json\r\n"), "");
    let req = req.replace("Content-Length: 0", "Content-Length: 7");
    c.write_all(req.as_bytes()).await.expect("head");
    tokio::time::sleep(Duration::from_millis(50)).await;
    c.write_all(br#"{"a":1}"#).await.expect("body");
    let out = read_all(c).await;
    assert!(out.starts_with("HTTP/1.1 200 OK") && out.ends_with("{}"), "{out}");
    let got = seen.recv().await.expect("Sew saw it");
    assert!(got.starts_with("POST /api/v1/me/voice/stats?x=1 HTTP/1.1\r\n"), "{got}");
    assert!(got.contains("Authorization: Bearer sewcc_vdsel.vdsecret\r\n") && got.ends_with(r#"{"a":1}"#), "{got}");
    let lower = got.to_ascii_lowercase();
    assert!(!lower.contains("cookie") && !lower.contains("jobs.acme.k2.dev"), "{got}");

    // A GET with no Origin (a native client sends none) passes too.
    let out = ask(id, &format!("GET /_k2/voice/sew/api/v1/devices/crl HTTP/1.1\r\nHost: jobs.acme.k2.dev\r\n{BEARER}\r\n")).await;
    assert!(out.starts_with("HTTP/1.1 200 OK"), "{out}");
    assert!(seen.recv().await.expect("Sew saw it").starts_with("GET /api/v1/devices/crl HTTP/1.1\r\n"));

    // Off the allowlist: K2's own 404, with the frame headers.
    let out = ask(id, &format!("GET /_k2/voice/sew/api/v1/spaces HTTP/1.1\r\nHost: jobs.acme.k2.dev\r\n{BEARER}\r\n")).await;
    assert!(out.starts_with("HTTP/1.1 404") && out.contains(r#""code":"not_found""#), "{out}");
    assert!(out.contains(k2_core::frame_policy::CONNECT_HEADER_LINES), "K2's own answer carries the frame headers: {out}");
    let out = ask(id, &format!("GET /_k2/voice/sew/api/v1/gateway/ticket HTTP/1.1\r\nHost: a\r\n{BEARER}\r\n")).await;
    assert!(out.starts_with("HTTP/1.1 404"), "the ticket route stays shut: {out}");
    nothing_reached(&mut seen, "an off-list route").await;

    // Only a call credential counts.
    for (what, auth) in [
        ("an App pass", "Authorization: Bearer k2skn_pass.secret\r\n"),
        ("a Sew session", "Authorization: Bearer sews_session.secret\r\n"),
        ("a cookie", "Cookie: k2_session=abc; app_session=def\r\n"),
        ("nothing", ""),
    ] {
        let out = ask(id, &post("/api/v1/me/voice/state", auth, "{}")).await;
        assert!(out.starts_with("HTTP/1.1 401") && out.contains("call_credential_required"), "{what}: {out}");
        assert!(!out.contains("k2skn_pass") && !out.contains("sews_session"), "{what}: {out}");
    }
    nothing_reached(&mut seen, "a request without a call credential").await;
    assert_eq!(dials.load(Ordering::SeqCst), 0, "the door never dials the App");

    // Anything else is the App's, unchanged.
    let out = ask(id, "GET /_k2/voice/sewx HTTP/1.1\r\nHost: jobs.acme.k2.dev\r\n\r\n").await;
    assert!(out.starts_with("HTTP/1.1 200 OK") && out.ends_with("app"), "{out}");
    assert_eq!(dials.load(Ordering::SeqCst), 1);
    nothing_reached(&mut seen, "the App's own request").await;

    // The App down: the door still serves (the call's keys don't need it).
    crate::apps::router::clear(id);
    let out = ask(id, &post("/api/v1/me/voice/leave", BEARER, "{}")).await;
    assert!(out.starts_with("HTTP/1.1 200 OK"), "{out}");
    assert!(seen.recv().await.expect("Sew saw it").starts_with("POST /api/v1/me/voice/leave HTTP/1.1\r\n"));
    let out = ask(id, "GET / HTTP/1.1\r\nHost: jobs.acme.k2.dev\r\n\r\n").await;
    assert!(out.starts_with("HTTP/1.1 503"), "the rest is the down App's 503: {out}");
}

#[tokio::test]
async fn an_app_without_the_door_gets_404_and_sew_hears_nothing() {
    let _home = crate::test_support::TempHome::new();
    let (sew, mut seen) = fake_sew("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", false).await;
    let (app_port, dials) = fake_app().await;
    let id = "voice-door-shut";
    let _ = crate::apps::router::switch(id, app_port);
    set_test_door(id, false, &sew);
    let out = ask(id, &post("/api/v1/me/voice/stats", BEARER, "{}")).await;
    assert!(out.starts_with("HTTP/1.1 404") && out.contains(crate::voice::sew_lane::APP_DOOR_OFF), "{out}");
    assert!(out.contains(k2_core::frame_policy::CONNECT_HEADER_LINES), "{out}");
    let upgrade = format!("GET /_k2/voice/sew/api/v1/gateway HTTP/1.1\r\nHost: a\r\n{BEARER}Connection: Upgrade\r\nUpgrade: websocket\r\n\r\n");
    assert!(ask(id, &upgrade).await.starts_with("HTTP/1.1 404"), "no socket either");
    nothing_reached(&mut seen, "a shut door's request").await;
    assert_eq!(dials.load(Ordering::SeqCst), 0, "a door path is never the App's on a voice box");
    let out = ask(id, "GET / HTTP/1.1\r\nHost: jobs.acme.k2.dev\r\n\r\n").await;
    assert!(out.ends_with("app"), "the App itself is untouched: {out}");
    crate::apps::router::clear(id);
}

#[tokio::test]
async fn the_gateway_upgrade_passes_through_on_an_app_host() {
    let _home = crate::test_support::TempHome::new();
    let (sew, mut seen) = fake_sew("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n", true).await;
    let (app_port, dials) = fake_app().await;
    let id = "voice-door-socket";
    let _ = crate::apps::router::switch(id, app_port);
    set_test_door(id, true, &sew);
    let mut c = via_app(id).await;
    let req = "GET /_k2/voice/sew/api/v1/gateway?v=1&encoding=json HTTP/1.1\r\nHost: jobs.acme.k2.dev\r\nAuthorization: Bearer sewcc_vdws.secret\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: abc==\r\nSec-WebSocket-Version: 13\r\nCookie: app_session=x\r\n\r\n";
    c.write_all(req.as_bytes()).await.expect("handshake");
    let mut got = Vec::new();
    let mut b = [0u8; 1024];
    while !got.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(5), c.read(&mut b)).await.expect("in time").expect("read");
        assert!(n > 0, "closed in the handshake");
        got.extend_from_slice(&b[..n]);
    }
    assert!(got.starts_with(b"HTTP/1.1 101"), "{}", String::from_utf8_lossy(&got));
    let up = seen.recv().await.expect("Sew saw the handshake");
    assert!(up.starts_with("GET /api/v1/gateway?v=1&encoding=json HTTP/1.1\r\n"), "{up}");
    assert!(up.contains("sec-websocket-key: abc==\r\n") && !up.to_ascii_lowercase().contains("cookie"), "{up}");
    c.write_all(b"ping").await.expect("frame");
    let mut f = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), c.read_exact(&mut f)).await.expect("in time").expect("echo");
    assert_eq!(&f, b"ping", "frames cross both ways");
    assert_eq!(dials.load(Ordering::SeqCst), 0);
    drop(c);
    crate::apps::router::clear(id);
}

/// 0.46.9: the transfer door on an App's address is K2's, whatever the call
/// door's gate says: a door head is answered here (no token: 401) and never
/// reaches the App; anything else is replayed to the App untouched.
#[tokio::test]
async fn the_transfer_door_is_answered_on_the_app_path() {
    let (mut client, mut server) = tokio::io::duplex(64 * 1024);
    let head = "PUT /_k2/transfers/uploads/0123456789abcdef0123456789abcdef HTTP/1.1\r\nHost: a\r\nContent-Range: bytes 0-1/2\r\nContent-Length: 2\r\n\r\nab";
    client.write_all(head.as_bytes()).await.expect("write");
    let out = first_request_with(&mut server, None, || panic!("the call door's gate is never asked"), "127.0.0.1:1").await;
    assert!(matches!(out, FirstRequest::Answered), "K2 answers the transfer door");
    drop(server);
    let mut got = String::new();
    client.read_to_string(&mut got).await.expect("read");
    assert!(got.starts_with("HTTP/1.1 401"), "{got}");
    assert!(got.contains("transfer_token_required") && got.contains("Connection: close"), "{got}");

    let (mut client, mut server) = tokio::io::duplex(64 * 1024);
    client.write_all(b"GET /api/x HTTP/1.1\r\nHost: a\r\n\r\n").await.expect("write");
    match first_request_with(&mut server, None, || false, "127.0.0.1:1").await {
        FirstRequest::Pass(bytes) => assert!(bytes.starts_with(b"GET /api/x ")),
        FirstRequest::Answered => panic!("an App path goes to the App"),
    }
}
