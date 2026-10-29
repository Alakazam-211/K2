//! Runtime stage H — a daemon restart (hand-off) keeps App connections.
//!
//! The real daemon binary, headless, in a scratch HOME (no supervisor):
//!
//! 1. A published service (`/cli/publish/run`, a Python WebSocket echo +
//!    slow-stream server) behind the E2E TLS listener as the nested App
//!    host `app.hof.k2.dev` (routing map from `tunnel-routes.json`;
//!    self-signed cert), and an frpc stand-in the tunnel supervises.
//! 2. 200 WebSockets through the TLS listener, plus one in-flight HTTP
//!    stream, plus a prober that keeps opening new connections.
//! 3. `POST /cli/daemon/handoff`: a fresh daemon takes the sockets over.
//! 4. Every WebSocket still echoes, the stream completes byte-exact, the
//!    prober saw ZERO failed connections, the successor adopted the SAME
//!    frpc and the SAME App process, and the old daemon exits once its
//!    last splice closes. The App survives the old daemon (its stdout is
//!    no longer a pipe into it) and keeps serving through the successor.
//!
//! Spawn guards: scratch HOME, private XDG_RUNTIME_DIR (frpc config + its
//! `pkill -f` pattern can't touch another daemon's), `SHELL=/bin/false`
//! (PATH stays ours: the box may have a real frpc), an empty agent shim
//! dir, the control plane pointed at a closed port, no systemd/launchd
//! vars. Needs `python3`.
// The daemon hand-off passes listening sockets and the lock by fd (Unix);
// Windows restart hand-off comes with the ptyd/restart port (0.47).
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcp;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

const N_WS: usize = 200;
const SUB: &str = "hof";
const APP_HOST: &str = "app.hof.k2.dev";
const STREAM_CHUNKS: usize = 80;
const STREAM_MS: u64 = 100;
const CHUNK: usize = 16;

const APP_PY: &str = r#"
import asyncio, base64, hashlib, os, struct, sys
GUID = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

def say(*a):
    # An App that dies on a broken stdout (Node's default for EPIPE):
    # before stage H its stdout was a pipe into the daemon.
    try:
        print(*a, flush=True)
    except BrokenPipeError:
        os._exit(3)

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
    say("req", path)
    if hdr.get("upgrade", "").lower() == "websocket":
        acc = base64.b64encode(hashlib.sha1(hdr["sec-websocket-key"].encode() + GUID).digest()).decode()
        w.write(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                 "Sec-WebSocket-Accept: %s\r\n\r\n" % acc).encode())
        await w.drain()
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
                    w.write(bytes([0x88, 0])); await w.drain(); break
                if op == 9:
                    op = 10
                if ln < 126:
                    h = bytes([0x80 | op, ln])
                else:
                    h = bytes([0x80 | op, 126]) + struct.pack(">H", ln)
                w.write(h + bytes(data)); await w.drain()
        except Exception:
            pass
    elif path.startswith("/stream"):
        n, ms = int(sys.argv[2]), int(sys.argv[3])
        w.write(("HTTP/1.1 200 OK\r\nContent-Length: %d\r\nConnection: close\r\n\r\n" % (n * 16)).encode())
        await w.drain()
        for i in range(n):
            w.write(("chunk-%09d\n" % i).encode())
            await w.drain()
            await asyncio.sleep(ms / 1000.0)
    else:
        w.write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
        await w.drain()
    w.close()

async def main():
    srv = await asyncio.start_server(handle, "127.0.0.1", int(sys.argv[1]), backlog=1024)
    async with srv:
        await srv.serve_forever()

asyncio.run(main())
"#;

/// Everything the test started, killed on the way out (pass or fail).
#[derive(Default)]
struct Reaper {
    child: Option<Child>,
    pids: Vec<i32>,
}

impl Drop for Reaper {
    fn drop(&mut self) {
        for p in &self.pids {
            unsafe { libc::kill(*p, libc::SIGKILL) };
        }
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn scratch_home() -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        "k2-rth-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(home.join(".k2")).unwrap();
    home
}

fn read_trimmed(p: &Path) -> String {
    std::fs::read_to_string(p)
        .unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        .trim()
        .to_string()
}

/// A port the App binds later: unique, outside the ephemeral range (not
/// `bind(:0)`-then-drop, which another test can be handed meanwhile).
fn free_port() -> u16 {
    k2_core::test_env::loopback_test_port()
}

/// Whether any thread of `pid` still runs (a zombie, or a process that is
/// gone, is not alive).
///
/// Linux: every thread counts, not just the leader. When a multithreaded
/// process is killed, its leader can turn zombie (`/proc/<pid>/stat` says
/// `Z`) while another thread is still exiting, and that thread still holds
/// the shared fd table: its flocks are not released until it finishes. The
/// daemon's `disk-scan` thread runs at nice 19, so on a busy box it can
/// finish tens of ms after the leader. A thread that has reached `X` (or
/// left `/proc/<pid>/task`) has closed its files.
///
/// Observed through a pidfd, never `/proc/<pid>`: the daemons are
/// non-dumpable (the vault), so under K2 compute's job wall
/// (`ProtectProc=invisible`) their `/proc` entries are hidden. A pidfd
/// polls readable only once the whole thread group has exited (the kernel
/// delays the leader's exit notification until its last thread is gone),
/// which is exactly "every thread finished, its files closed".
fn pid_alive(pid: i32) -> bool {
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: pidfd_open(pid, 0) returns a new fd or -1.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
        if fd < 0 {
            let e = std::io::Error::last_os_error();
            assert_eq!(e.raw_os_error(), Some(libc::ESRCH), "pidfd_open({pid}): {e}");
            return false; // reaped since the kill(0)
        }
        let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd, zero timeout; then close our fd.
        let n = unsafe { libc::poll(&mut p, 1, 0) };
        unsafe { libc::close(fd) };
        assert!(n >= 0, "poll(pidfd {pid}): {}", std::io::Error::last_os_error());
        n == 0 // readable = the thread group has exited
    }
    #[cfg(not(target_os = "linux"))]
    {
        let out = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        !String::from_utf8_lossy(&out.stdout).trim_start().starts_with('Z')
    }
}

/// One HTTP/1.1 request to the daemon's main listener; (status, body).
fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut s = StdTcp::connect(("127.0.0.1", port)).unwrap_or_else(|e| panic!("connect daemon: {e}"));
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let body = body.unwrap_or("");
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

mod any_cert {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, SignatureScheme};

    /// The daemon serves a self-signed cert here (K2_E2E_SELF_SIGNED);
    /// what this test checks is the splice, not the chain.
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

fn tls_connector() -> tokio_rustls::TlsConnector {
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(any_cert::AnyCert))
    .with_no_client_auth();
    tokio_rustls::TlsConnector::from(Arc::new(cfg))
}

type Tls = tokio_rustls::client::TlsStream<TcpStream>;
type Ws = tokio_tungstenite::WebSocketStream<Tls>;

async fn tls(port: u16) -> Result<Tls, String> {
    let tcp = tokio::time::timeout(Duration::from_secs(30), TcpStream::connect(("127.0.0.1", port)))
        .await
        .map_err(|_| "tcp connect timed out".to_string())?
        .map_err(|e| format!("tcp connect: {e}"))?;
    let name = rustls::pki_types::ServerName::try_from(APP_HOST.to_string()).unwrap();
    tokio::time::timeout(Duration::from_secs(30), tls_connector().connect(name, tcp))
        .await
        .map_err(|_| "tls handshake timed out".to_string())?
        .map_err(|e| format!("tls handshake: {e}"))
}

async fn ws(port: u16) -> Result<Ws, String> {
    let t = tls(port).await?;
    let (ws, _) = tokio::time::timeout(
        Duration::from_secs(30),
        tokio_tungstenite::client_async(format!("ws://{APP_HOST}/ws"), t),
    )
    .await
    .map_err(|_| "ws upgrade timed out".to_string())?
    .map_err(|e| format!("ws upgrade: {e}"))?;
    Ok(ws)
}

async fn echo(ws: &mut Ws, msg: &str) -> Result<(), String> {
    ws.send(Message::Text(msg.to_string()))
        .await
        .map_err(|e| format!("send: {e}"))?;
    match tokio::time::timeout(Duration::from_secs(30), ws.next()).await {
        Ok(Some(Ok(Message::Text(t)))) if t == msg => Ok(()),
        other => Err(format!("echo of {msg:?}: {other:?}")),
    }
}

/// Wait for `f` up to `secs`, panicking with `what` on timeout.
async fn wait_for(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handoff_keeps_200_websockets_and_a_stream_and_adopts_frpc() {
    let py = Command::new("python3").arg("--version").output();
    assert!(
        py.map(|o| o.status.success()).unwrap_or(false),
        "python3 is required for the App fixture"
    );
    let python = String::from_utf8(
        Command::new("sh").args(["-c", "command -v python3"]).output().unwrap().stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let home = scratch_home();
    let k2 = home.join(".k2");
    let bin = home.join("bin");
    let run = home.join("run");
    let tmp = home.join("tmp");
    let shim = home.join("agent-shim-empty");
    let ws_dir = home.join("ws-app");
    for d in [&bin, &run, &tmp, &shim, &ws_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    // The App and its routing entry.
    let app_py = ws_dir.join("app.py");
    std::fs::write(&app_py, APP_PY).unwrap();
    let app_port = free_port();
    std::fs::write(
        k2.join("tunnel-routes.json"),
        serde_json::json!({ "primary": SUB, "targets": { "app": format!("localhost:{app_port}") } })
            .to_string(),
    )
    .unwrap();

    // The frpc stand-in: logs a proxy registration, then idles (argv stays
    // `<sh> <bin>/frpc -c <cfg>`, so it can be adopted).
    let frpc = bin.join("frpc");
    std::fs::write(
        &frpc,
        format!("#!/bin/sh\necho '[I] [proxy_wrapper.go:204] [x] [k2so-{SUB}] start proxy success'\nwhile :; do sleep 1; done\n"),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&frpc, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(
        k2.join("tunnel.json"),
        serde_json::json!({
            "server_addr": "127.0.0.1",
            "server_port": 1,
            "token": "tok-rth",
            "subdomain": SUB,
            "auto_start": true,
            "e2e": true,
        })
        .to_string(),
    )
    .unwrap();

    let log_path = home.join("daemon.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let path_env = k2_core::test_env::path_list_prepend(&bin, std::env::var("PATH").unwrap());
    let mut reaper = Reaper::default();
    reaper.child = Some(
        Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
            .env("HOME", &home)
            .env("PATH", &path_env)
            .env("SHELL", "/bin/false")
            .env("XDG_RUNTIME_DIR", &run)
            .env("TMPDIR", &tmp)
            .env("K2_E2E_SELF_SIGNED", "1")
            .env("K2_TUNNEL_LEASE", "off")
            .env("K2_CONNECT_BASE", "http://127.0.0.1:1")
            .env("K2_CERT_BROKER_URL", "http://127.0.0.1:1")
            .env("K2_HANDOFF_DRAIN_SECS", "120")
            .env("K2_HANDOFF_DROPIN", "off")
            .env("K2_TEST_AGENT_SHIM_DIR", &shim)
            .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
            .env("K2SO_WATCHDOG_DISABLED", "1")
            .env_remove("INVOCATION_ID")
            .env_remove("NOTIFY_SOCKET")
            .env_remove("XPC_SERVICE_NAME")
            .env_remove("K2_AIRGAP")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("spawn daemon"),
    );
    let old_pid = reaper.child.as_ref().unwrap().id() as i32;
    let dump = |why: &str| -> String {
        format!(
            "{why}\n--- daemon.log (tail) ---\n{}",
            std::fs::read_to_string(&log_path)
                .unwrap_or_default()
                .lines()
                .rev()
                .take(80)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    // Ready.
    wait_for(60, "daemon.port + daemon.token", || {
        k2.join("daemon.port").exists() && k2.join("daemon.token").exists()
    })
    .await;
    let dport: u16 = read_trimmed(&k2.join("daemon.port")).parse().unwrap();
    let token = read_trimmed(&k2.join("daemon.token"));
    wait_for(60, "boot ready", || {
        http(dport, "GET", &format!("/cli/daemon/handoff/status?token={token}"), None).0 == 200
    })
    .await;

    // The App: a published service the daemon spawned.
    let wsp = ws_dir.to_string_lossy().to_string();
    let (st, body) = http(
        dport,
        "POST",
        &format!("/cli/projects/add-without-git?token={token}"),
        Some(&serde_json::json!({ "path": wsp, "seedWiki": false, "seedAgentsMd": false }).to_string()),
    );
    assert_eq!(st, 200, "{}", dump(&format!("workspace add: {body}")));
    let cmd = format!(
        "exec {python} {} {app_port} {STREAM_CHUNKS} {STREAM_MS}",
        app_py.display()
    );
    let (st, body) = http(
        dport,
        "POST",
        &format!("/cli/publish/run?token={token}"),
        Some(
            &serde_json::json!({
                "project": wsp, "name": "app", "cmd": cmd, "port": app_port, "noTunnel": true
            })
            .to_string(),
        ),
    );
    assert_eq!(st, 200, "{}", dump(&format!("publish run: {body}")));
    let svc: serde_json::Value = serde_json::from_str(&body).unwrap();
    let app_pid = svc["pid"].as_i64().unwrap_or_else(|| panic!("publish pid: {body}")) as i32;
    reaper.pids.push(app_pid);

    // The tunnel: TLS listener up, frpc supervised.
    wait_for(60, "tunnel-https.port", || k2.join("tunnel-https.port").exists()).await;
    let tls_port: u16 = read_trimmed(&k2.join("tunnel-https.port")).parse().unwrap();
    // Beside the rendered frpc config, in the private runtime dir.
    let pid_rec = if cfg!(target_os = "linux") {
        run.join("k2").join("frpc.pid")
    } else {
        tmp.join("k2-runtime").join("frpc.pid")
    };
    let mut seen = false;
    for _ in 0..600 {
        if pid_rec.exists() {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(seen, "{}", dump(&format!("no {}", pid_rec.display())));
    let frpc_rec: serde_json::Value = serde_json::from_str(&read_trimmed(&pid_rec)).unwrap();
    let frpc_pid = frpc_rec["pid"].as_i64().unwrap() as i32;
    reaper.pids.push(frpc_pid);
    assert!(pid_alive(frpc_pid));

    // 200 WebSockets through the E2E listener to the App.
    let mut sockets = Vec::with_capacity(N_WS);
    for i in 0..N_WS {
        let mut w = ws(tls_port)
            .await
            .unwrap_or_else(|e| panic!("{}", dump(&format!("ws {i} before the hand-off: {e}"))));
        echo(&mut w, &format!("before-{i}"))
            .await
            .unwrap_or_else(|e| panic!("{}", dump(&e)));
        sockets.push(w);
    }

    // One in-flight HTTP stream (80 × 100 ms ≈ 8 s).
    let mut stream = tls(tls_port).await.expect("stream tls");
    stream
        .write_all(
            format!("GET /stream HTTP/1.1\r\nHost: {APP_HOST}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    while !got.windows(4).any(|w| w == b"\r\n\r\n") || got.len() < 200 {
        let n = stream.read(&mut buf).await.expect("stream head");
        assert!(n > 0, "stream closed early");
        got.extend_from_slice(&buf[..n]);
    }

    // A prober: new connections all through the switch.
    let stop = Arc::new(AtomicBool::new(false));
    let ok_n = Arc::new(AtomicUsize::new(0));
    let fails = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let prober = {
        let (stop, ok_n, fails) = (stop.clone(), ok_n.clone(), fails.clone());
        tokio::spawn(async move {
            let mut i = 0;
            while !stop.load(Ordering::SeqCst) {
                i += 1;
                match ws(tls_port).await {
                    Ok(mut w) => match echo(&mut w, &format!("probe-{i}")).await {
                        Ok(()) => {
                            ok_n.fetch_add(1, Ordering::SeqCst);
                            let _ = w.close(None).await;
                        }
                        Err(e) => fails.lock().unwrap().push(e),
                    },
                    Err(e) => fails.lock().unwrap().push(e),
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ── The hand-off ──────────────────────────────────────────────────
    let t0 = Instant::now();
    let (st, body) = http(dport, "POST", &format!("/cli/daemon/handoff?token={token}"), Some(""));
    assert_eq!(st, 202, "{}", dump(&format!("handoff: {body}")));
    let lock = k2.join("daemon.lock");
    wait_for(60, "the successor to own daemon.lock", || {
        std::fs::read_to_string(&lock)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .is_some_and(|p| p != old_pid)
    })
    .await;
    let new_pid: i32 = read_trimmed(&lock).parse().unwrap();
    reaper.pids.push(new_pid);
    wait_for(60, "the successor's token", || read_trimmed(&k2.join("daemon.token")) != token).await;
    let new_token = read_trimmed(&k2.join("daemon.token"));
    wait_for(60, "the successor ready on the same port", || {
        let (s, b) = http(dport, "GET", &format!("/cli/daemon/handoff/status?token={new_token}"), None);
        s == 200 && b.contains("\"bootedFromHandoff\":true")
    })
    .await;
    let switched_in = t0.elapsed();
    assert!(pid_alive(old_pid), "the old daemon must still be draining its splices");
    // That the draining daemon let go of daemon.lock is checked at the
    // second hand-off, where its successor can be killed (below).

    // Every old WebSocket still works (the old daemon still pumps them).
    for (i, w) in sockets.iter_mut().enumerate() {
        echo(w, &format!("after-{i}"))
            .await
            .unwrap_or_else(|e| panic!("{}", dump(&format!("ws {i} after the hand-off: {e}"))));
    }

    // The stream completes, byte-exact.
    loop {
        let n = stream.read(&mut buf).await.expect("stream body");
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buf[..n]);
    }
    let body_start = got.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let body = &got[body_start..];
    assert_eq!(body.len(), STREAM_CHUNKS * CHUNK, "the in-flight stream was cut");
    for (i, line) in body.chunks(CHUNK).enumerate() {
        assert_eq!(line, format!("chunk-{i:09}\n").as_bytes(), "chunk {i}");
    }

    // The prober saw no refusal and no drop across the switch.
    tokio::time::sleep(Duration::from_millis(500)).await;
    stop.store(true, Ordering::SeqCst);
    prober.await.unwrap();
    let fails = fails.lock().unwrap().clone();
    assert!(fails.is_empty(), "{}", dump(&format!("{} prober failure(s): {fails:?}", fails.len())));
    assert!(ok_n.load(Ordering::SeqCst) > 10, "the prober ran");

    // frpc was adopted, not restarted.
    let rec: serde_json::Value = serde_json::from_str(&read_trimmed(&pid_rec)).unwrap();
    assert_eq!(rec["pid"].as_i64().unwrap() as i32, frpc_pid, "frpc must be adopted");
    assert!(pid_alive(frpc_pid));
    let (st, body) = http(dport, "GET", &format!("/cli/tunnel/status?token={new_token}"), None);
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"running\":true"), "{}", dump(&format!("tunnel status: {body}")));
    assert_eq!(
        std::fs::read_to_string(home.join(".k2/frpc.log"))
            .unwrap()
            .matches("start proxy success")
            .count(),
        1,
        "exactly one frpc ever started"
    );
    assert_eq!(
        read_trimmed(&k2.join("tunnel-https.port")).parse::<u16>().unwrap(),
        tls_port,
        "the tunnel TLS port is stable"
    );

    // The old daemon exits once its last splice closes.
    for mut w in sockets.drain(..) {
        let _ = w.close(None).await;
    }
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(st) = reaper.child.as_mut().unwrap().try_wait().unwrap() {
            assert!(st.success(), "{}", dump(&format!("old daemon exit: {st:?}")));
            break;
        }
        assert!(Instant::now() < deadline, "{}", dump("the old daemon never finished draining"));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // The App outlived the old daemon (same pid, still serving through the
    // successor), and the successor still supervises it.
    assert!(pid_alive(app_pid), "{}", dump("the App died with the old daemon"));
    let mut w = ws(tls_port).await.unwrap_or_else(|e| panic!("{}", dump(&format!("ws after the old daemon exited: {e}"))));
    echo(&mut w, "post-exit").await.unwrap_or_else(|e| panic!("{}", dump(&e)));
    let _ = w.close(None).await;
    drop(w); // closes the TCP connection: the splice ends with it
    let (st, body) = http(
        dport,
        "GET",
        &format!("/cli/publish/list?token={new_token}&project={}", urlencode(&wsp)),
        None,
    );
    assert_eq!(st, 200, "{body}");
    assert!(
        body.contains(&format!("\"pid\":{app_pid}")),
        "{}",
        dump(&format!("the successor must have re-attached the App: {body}"))
    );
    assert!(pid_alive(frpc_pid), "frpc survives the old daemon's exit");

    // The successor can hand off again (its own listeners registered, the
    // lock and the frpc record carried forward).
    let mut held_ws = ws(tls_port).await.expect("ws before the second hand-off");
    echo(&mut held_ws, "second-before").await.expect("echo");
    let (st, body) = http(dport, "POST", &format!("/cli/daemon/handoff?token={new_token}"), Some(""));
    assert_eq!(st, 202, "{}", dump(&format!("second handoff: {body}")));
    wait_for(60, "the third daemon to own daemon.lock", || {
        std::fs::read_to_string(&lock)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .is_some_and(|p| p != new_pid)
    })
    .await;
    let third_pid: i32 = read_trimmed(&lock).parse().unwrap();
    reaper.pids.push(third_pid);
    echo(&mut held_ws, "second-after")
        .await
        .unwrap_or_else(|e| panic!("{}", dump(&format!("ws across the second hand-off: {e}"))));
    let mut fresh = ws(tls_port).await.expect("ws via the third daemon");
    echo(&mut fresh, "third").await.expect("echo");

    // The draining daemon let go of the singleton lock: a successor that
    // dies during the drain must not leave daemon.lock held by it (the
    // supervisor's fresh daemon would be refused for the whole drain).
    // The predecessor and successor share one open file description, so
    // kill the successor while the second daemon still drains `held_ws`;
    // then a NEW open file description can take the flock only if no
    // process still holds the shared one. Behavioural, so it needs no
    // root: the daemon is non-dumpable (vault Q18), and its /proc/<pid>/fd
    // is unreadable to the same non-root user.
    // "Gone" means every thread of it (see `pid_alive`): a still-exiting
    // thread of the killed daemon holds the shared description too.
    unsafe { libc::kill(third_pid, libc::SIGKILL) };
    wait_for(20, "the killed third daemon to be gone", || !pid_alive(third_pid)).await;
    assert!(pid_alive(new_pid), "the second daemon must still be draining held_ws");
    {
        use std::os::unix::io::AsRawFd;
        let probe = std::fs::OpenOptions::new().read(true).write(true).open(&lock).expect("open daemon.lock");
        let rc = unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        let err = std::io::Error::last_os_error();
        assert_eq!(rc, 0, "{}", dump(&format!("the draining daemon (pid {new_pid}) still holds daemon.lock: {err}")));
        unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_UN) };
    }

    let _ = held_ws.close(None).await;
    drop(held_ws);
    let _ = fresh.close(None).await;
    drop(fresh);
    wait_for(60, "the second daemon to drain and exit", || !pid_alive(new_pid)).await;
    let rec: serde_json::Value = serde_json::from_str(&read_trimmed(&pid_rec)).unwrap();
    assert_eq!(rec["pid"].as_i64().unwrap() as i32, frpc_pid, "still the same frpc");

    eprintln!(
        "[rt-h] hand-off: {N_WS} websockets + 1 stream kept, {} new connections during the \
         switch, 0 failed; successor ready in {switched_in:?}",
        ok_n.load(Ordering::SeqCst)
    );

    // The last daemon was killed for the lock check; the reaper stops frpc
    // and the App.
    drop(reaper);
    let _ = std::fs::remove_dir_all(&home);
}

/// `pid_alive` (the hand-off test's "the killed successor is gone" check)
/// must not call a process gone while one of its threads still holds its
/// files. A Python child takes a flock, starts a thread that keeps running,
/// then ends only its main thread (raw `SYS_exit`): the leader is a zombie
/// while the process still holds the lock. Checking the leader alone (the
/// old helper) said "gone" here, and the lock probe after it failed on
/// every Linux node.
#[cfg(target_os = "linux")]
#[test]
fn pid_alive_counts_a_zombie_leader_whose_thread_still_holds_a_flock() {
    use std::io::BufRead;
    use std::os::unix::io::AsRawFd;

    let dir = std::env::temp_dir().join(format!("k2-rth-zl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let lock = dir.join("held.lock");
    let script = r#"
import ctypes, fcntl, os, platform, sys, threading, time
f = open(sys.argv[1], "a+")
fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
def hold():
    time.sleep(float(sys.argv[2]))
    os._exit(0)
threading.Thread(target=hold).start()
print("locked", flush=True)
# End the main thread only (SYS_exit, not exit_group): the leader becomes a
# zombie while `hold` keeps the process, and its fd table, alive.
ctypes.CDLL(None).syscall({"x86_64": 60, "aarch64": 93}[platform.machine()], 0)
"#;
    let mut child = Command::new("python3")
        .args(["-c", script, lock.to_str().unwrap(), "3"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .expect("python3 is required");
    let pid = child.id() as i32;
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "locked");

    // Wait until the leader is a zombie (what the old check looked at).
    let leader_state = || {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("child stat");
        stat[stat.rfind(')').unwrap() + 1..].split_whitespace().next().unwrap().to_string()
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while leader_state() != "Z" {
        assert!(Instant::now() < deadline, "the leader never became a zombie");
        std::thread::sleep(Duration::from_millis(5));
    }

    let probe = std::fs::OpenOptions::new().read(true).write(true).open(&lock).unwrap();
    let try_lock = || unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    assert!(!try_lock(), "the child's live thread must still hold the flock");
    assert!(
        pid_alive(pid),
        "a zombie leader whose thread still holds the flock is not gone"
    );

    // Once every thread has exited, it is gone and the lock is free.
    let deadline = Instant::now() + Duration::from_secs(20);
    while pid_alive(pid) {
        assert!(Instant::now() < deadline, "the child never exited");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(try_lock(), "gone, yet the flock is still held");
    assert!(child.wait().unwrap().success());
    let _ = std::fs::remove_dir_all(&dir);
}
