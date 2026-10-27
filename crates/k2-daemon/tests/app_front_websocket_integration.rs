//! 0.46.3 regression: WebSockets through an App's doors, end to end.
//!
//! The real dispatcher in-process on a temp `$HOME`, a real App installed
//! and deployed on the process engine (a python3 WebSocket server), then:
//!
//! 1. **The front port** (`apps::router::ensure_front`, the stable loopback
//!    listener the control plane targets): a WebSocket upgrade answers
//!    `101`, text echoes, ping gets its pong, and several sockets run side
//!    by side.
//! 2. **The gateway** (the E2E TLS listener, `tunnel_tls_listener`, with
//!    the App's nested host `wsapp.<sub>.k2.dev` routed through the App
//!    override): an `/rtc`-style long-lived WebSocket — the server pushes a
//!    tick every 100 ms, the client talks over it, it sits idle past 2 s
//!    gaps, and nothing cuts it for the whole run. Then it closes cleanly.
//!
//! Hermetic: self-signed leaf (`K2_E2E_SELF_SIGNED`), no frpc (the
//! listener comes up through the same ensure hook a tunnel start uses),
//! the control plane pointed at a 503 server, no real `~/.k2`.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::sync::Arc;
use std::time::{Duration, Instant};

use app_fixture::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

const APP: &str = "wsapp";
const SUB: &str = "wsx";

/// A python3 App: `GET /healthz` (and anything else) answers 200; a
/// WebSocket upgrade echoes text and answers pings; on `/rtc` it also
/// pushes `tick:<n>` every 100 ms until the socket closes.
const WS_APP_PY: &str = r#"
import asyncio, base64, hashlib, os, struct
GUID = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

def frame(op, data):
    n = len(data)
    if n < 126:
        h = bytes([0x80 | op, n])
    elif n < 65536:
        h = bytes([0x80 | op, 126]) + struct.pack(">H", n)
    else:
        h = bytes([0x80 | op, 127]) + struct.pack(">Q", n)
    return h + data

async def ticker(w):
    n = 0
    try:
        while True:
            await asyncio.sleep(0.1)
            n += 1
            w.write(frame(1, ("tick:%d" % n).encode()))
            await w.drain()
    except Exception:
        pass

async def handle(r, w):
    try:
        head = await r.readuntil(b"\r\n\r\n")
    except Exception:
        w.close(); return
    lines = head.decode().split("\r\n")
    path = lines[0].split(" ")[1]
    hdr = {}
    for l in lines[1:]:
        if ":" in l:
            k, v = l.split(":", 1)
            hdr[k.strip().lower()] = v.strip()
    if hdr.get("upgrade", "").lower() != "websocket":
        w.write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
        await w.drain(); w.close(); return
    acc = base64.b64encode(hashlib.sha1(hdr["sec-websocket-key"].encode() + GUID).digest()).decode()
    w.write(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
             "Sec-WebSocket-Accept: %s\r\n\r\n" % acc).encode())
    await w.drain()
    tick = asyncio.ensure_future(ticker(w)) if path.startswith("/rtc") else None
    try:
        while True:
            b0, b1 = await r.readexactly(2)
            op = b0 & 0x0F
            ln = b1 & 0x7F
            if ln == 126:
                ln = struct.unpack(">H", await r.readexactly(2))[0]
            elif ln == 127:
                ln = struct.unpack(">Q", await r.readexactly(8))[0]
            mask = await r.readexactly(4) if b1 & 0x80 else b"\0\0\0\0"
            data = bytearray(await r.readexactly(ln))
            for i in range(ln):
                data[i] ^= mask[i % 4]
            if op == 8:
                w.write(frame(8, b"")); await w.drain(); break
            w.write(frame(10 if op == 9 else op, bytes(data)))
            await w.drain()
    except Exception:
        pass
    if tick:
        tick.cancel()
    w.close()

async def main():
    srv = await asyncio.start_server(handle, "127.0.0.1", int(os.environ["PORT"]))
    print("ws fixture listening on %s" % os.environ["PORT"], flush=True)
    async with srv:
        await srv.serve_forever()

asyncio.run(main())
"#;

fn ws_app_folder(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        format!(
            r#"{{"schema":1,"kind":"app","id":"{APP}","name":"{APP}","version":"1.0.0",
"caps":["thread:read"],
"host":{{"mode":"cmd","start":["python3","server.py"],"health":{{"path":"/healthz"}},"drainSecs":2}}}}"#
        ),
    )
    .unwrap();
    std::fs::write(dir.join("server.py"), WS_APP_PY).unwrap();
    dunce::canonicalize(&dir).unwrap()
}

mod any_cert {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, SignatureScheme};

    /// The listener serves a self-signed leaf here; what this test checks
    /// is the WebSocket through it, not the chain.
    #[derive(Debug)]
    pub struct AnyCert;
    impl ServerCertVerifier for AnyCert {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            rustls::crypto::aws_lc_rs::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }
}

async fn tls_to(port: u16, sni: &str) -> tokio_rustls::client::TlsStream<TcpStream> {
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(any_cert::AnyCert))
    .with_no_client_auth();
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.expect("tcp connect to the TLS listener");
    let name = rustls::pki_types::ServerName::try_from(sni.to_string()).unwrap();
    tokio::time::timeout(Duration::from_secs(30), tokio_rustls::TlsConnector::from(Arc::new(cfg)).connect(name, tcp))
        .await
        .expect("tls handshake timed out")
        .expect("tls handshake")
}

/// Next data message (text), answering nothing; fails loudly on a close,
/// an error or 10 s of silence.
async fn next_text<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>, what: &str) -> String
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => return t.to_string(),
            Ok(Some(Ok(Message::Pong(_)))) | Ok(Some(Ok(Message::Ping(_)))) => continue,
            other => panic!("{what}: expected a text message, got {other:?}"),
        }
    }
}

/// Send `msg`, then read until its echo comes back; returns the ticks seen
/// on the way (the `/rtc` server keeps pushing while we talk).
async fn talk<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>, msg: &str) -> Vec<u64>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    ws.send(Message::Text(msg.to_string().into())).await.expect("send");
    let mut ticks = Vec::new();
    loop {
        let t = next_text(ws, &format!("echo of {msg}")).await;
        if t == msg {
            return ticks;
        }
        let n = t
            .strip_prefix("tick:")
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("unexpected message {t:?} while waiting for {msg:?}"));
        ticks.push(n);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websockets_through_the_app_front_port_and_a_long_lived_rtc_socket_through_the_gateway() {
    require_python3();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let env = setup(&[
        ("K2_E2E_SELF_SIGNED", "1"),
        ("K2_TUNNEL_LEASE", "off"),
    ]);
    let _e2e = k2_core::test_env::EnvVar::remove("K2_E2E");
    let _air = k2_core::test_env::EnvVar::remove("K2_AIRGAP");
    let cp = k2_core::test_env::ErrorHttpServer::start(503);
    let _cp = k2_core::test_env::EnvVar::set(k2_core::tunnel::subdomains::CONTROL_PLANE_BASE_ENV, cp.url());

    k2_daemon::tunnel_tls_listener::register_ensure_hook();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "wsws");

    // A real App, installed and deployed on the process engine.
    let r = post(d.port, "/cli/app/install", OWNER, json!({"app": APP, "workspace": ws.id, "engine": "process"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let front = r.json()["frontPort"].as_u64().expect("frontPort") as u16;
    let folder = ws_app_folder(&ws.path.join(APP));
    let r = post(d.port, "/cli/app/deploy", OWNER, json!({"app": APP, "folder": folder, "wait": 60})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["deployment"]["result"], "live", "{}", r.body);
    let backend = k2_daemon::apps::router::live(APP).expect("the deploy switched the App's override").port;
    assert_ne!(backend, front, "the front port is the daemon's, not the App's");

    // ── 1. WebSocket upgrades through the front port ────────────────────
    let mut sockets = Vec::new();
    for i in 0..5 {
        let tcp = TcpStream::connect(("127.0.0.1", front)).await.expect("connect front port");
        let (mut s, resp) = tokio::time::timeout(
            Duration::from_secs(30),
            tokio_tungstenite::client_async(format!("ws://127.0.0.1:{front}/ws"), tcp),
        )
        .await
        .expect("upgrade timed out")
        .expect("WebSocket upgrade through the front port");
        assert_eq!(resp.status().as_u16(), 101);
        assert!(talk(&mut s, &format!("front-{i}")).await.is_empty(), "no ticks off /rtc");
        sockets.push(s);
    }
    for (i, s) in sockets.iter_mut().enumerate() {
        assert!(talk(s, &format!("again-{i}")).await.is_empty());
        s.send(Message::Ping(b"p".to_vec().into())).await.unwrap();
        match tokio::time::timeout(Duration::from_secs(10), s.next()).await {
            Ok(Some(Ok(Message::Pong(p)))) => assert_eq!(&p[..], b"p"),
            other => panic!("pong through the front port: {other:?}"),
        }
    }
    for mut s in sockets {
        s.close(None).await.expect("close");
    }

    // ── 2. A long-lived /rtc socket through the gateway (E2E TLS) ───────
    k2_core::tunnel::config::save(&k2_core::tunnel::TunnelConfig {
        token: "tok-ws".to_string(),
        subdomain: SUB.to_string(),
        device_id: Some("dev-ws".to_string()),
        ..Default::default()
    })
    .expect("seed tunnel.json");
    let mut targets = std::collections::HashMap::new();
    targets.insert(APP.to_string(), format!("localhost:{front}"));
    k2_core::tunnel::subdomains::store(k2_core::tunnel::subdomains::SubdomainMap {
        primary: SUB.to_string(),
        targets,
    });
    let dport = d.port;
    let https = tokio::task::spawn_blocking(move || k2_core::tunnel::tls::ensure_https_port(dport))
        .await
        .unwrap()
        .expect("the E2E TLS listener comes up");
    let host = format!("{APP}.{SUB}.k2.dev");
    let tls = tls_to(https, &host).await;
    let (mut rtc, resp) = tokio::time::timeout(
        Duration::from_secs(30),
        tokio_tungstenite::client_async(format!("wss://{host}/rtc"), tls),
    )
    .await
    .expect("rtc upgrade timed out")
    .expect("WebSocket upgrade through the gateway");
    assert_eq!(resp.status().as_u16(), 101);

    let started = Instant::now();
    let mut last_tick = 0u64;
    let mut round = 0;
    while started.elapsed() < Duration::from_secs(8) {
        round += 1;
        let ticks = talk(&mut rtc, &format!("rtc-{round}")).await;
        for t in ticks {
            assert_eq!(t, last_tick + 1, "server pushes arrive in order, none lost");
            last_tick = t;
        }
        // Client idle for 2.5 s (only the server talks): an idle cut in any
        // splice on the way would end the socket here.
        tokio::time::sleep(Duration::from_millis(2500)).await;
    }
    // Drain the ticks that queued while we slept; they keep counting.
    let ticks = talk(&mut rtc, "rtc-last").await;
    for t in ticks {
        assert_eq!(t, last_tick + 1, "server pushes arrive in order, none lost");
        last_tick = t;
    }
    assert!(last_tick >= 60, "~8 s of 100 ms pushes reached the client, got {last_tick}");
    assert!(round >= 3, "at least three idle gaps, got {round}");
    rtc.close(None).await.expect("clean close through the gateway");
    loop {
        match tokio::time::timeout(Duration::from_secs(10), rtc.next()).await {
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => break,
            Ok(Some(Ok(_))) => continue,
            other => panic!("the close handshake through the gateway: {other:?}"),
        }
    }

    // The front port still serves plain HTTP afterwards.
    assert_eq!(http_get(front, "/healthz").map(|(s, b)| (s, b)), Some((200, "ok".to_string())));
    let r = post(d.port, "/cli/app/stop", OWNER, json!({"app": APP})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(http_get(front, "/").is_some_and(|(s, _)| s == 503), "stopped: K2 answers 503 at the front");
    k2_core::tunnel::subdomains::store(k2_core::tunnel::subdomains::SubdomainMap::default());
}
