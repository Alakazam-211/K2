//! Home M2 — the two-daemon harness (prd-home-multi-server-client-v1 MS51,
//! vs-live MS74).
//!
//! The Home connection pool talks to several servers at once. The in-process
//! harness cannot run two daemons (HOME is process-wide), so this file spawns
//! two REAL `k2-daemon` children, each under its own temp HOME (the
//! singleton lock is per HOME), exactly as `daemon_port_stability_integration`
//! does: `CARGO_BIN_EXE_k2-daemon`, an empty `K2_TEST_AGENT_SHIM_DIR`,
//! `K2SO_WATCHDOG_DISABLED=1`, kill-on-drop, and `$HOME/.k2/daemon.port`
//! polling. The owner token is `$HOME/.k2/daemon.token`.
//!
//! What it pins, per server:
//!   - each daemon answers its own `/boot-status` with its own `instanceId`
//!     (the pool's restart and "same server" signal, MS29 / MS81);
//!   - a login minted on B works on B only: B's token and A's owner token
//!     are refused by the other server (MS32);
//!   - whoami on B reports the role (MS83) and the presence summary answers
//!     with B's login;
//!   - a steady stream of authed requests does not rewrite B's
//!     `connect-sessions.json` (MS44 b / MS82);
//!   - a kick on B closes B's event socket with 4001 "kicked", and the 5 s
//!     re-check closes a socket whose token stopped being valid with 4003
//!     "revoked" (MS44 a / MS71). A's sockets are untouched.
//!
//! Login runs over loopback: the daemon's per-IP login limit exempts
//! loopback (MS73), so the client-side budgets are tested in the renderer
//! (lib/host-pool.test.ts), not here.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

type WsClient = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One spawned daemon: its HOME, loopback port and owner token.
struct Daemon {
    _child: KillOnDrop,
    home: PathBuf,
    port: u16,
    owner: String,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // `_child` drops first (field order is declaration order, but the
        // explicit kill here makes the order not matter).
        let _ = self._child.0.kill();
        let _ = self._child.0.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn isolated_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        "k2-home-two-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".k2")).expect("create temp HOME");
    home
}

fn read_trimmed(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let t = raw.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn spawn_daemon(tag: &str) -> Daemon {
    let home = isolated_home(tag);
    let shim_dir = home.join("agent-shim-empty");
    std::fs::create_dir_all(&shim_dir).expect("create empty agent shim dir");
    let child = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
            .env("HOME", &home)
            .env("K2_TEST_AGENT_SHIM_DIR", &shim_dir)
            .env("K2SO_WATCHDOG_DISABLED", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn k2-daemon"),
    );
    let port_file = home.join(".k2").join("daemon.port");
    let token_file = home.join(".k2").join("daemon.token");
    let deadline = Instant::now() + Duration::from_secs(20);
    let (port, owner) = loop {
        let port = read_trimmed(&port_file).and_then(|p| p.parse::<u16>().ok());
        let owner = read_trimmed(&token_file);
        if let (Some(port), Some(owner)) = (port, owner) {
            break (port, owner);
        }
        assert!(Instant::now() < deadline, "daemon {tag} never published daemon.port + daemon.token");
        std::thread::sleep(Duration::from_millis(50));
    };
    let d = Daemon { _child: child, home, port, owner };
    // Wait for the readiness handshake (the pool's live / starting rule).
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(r) = try_http(d.port, "GET", "/boot-status", None) {
            if r.status == 200 {
                let v: serde_json::Value = serde_json::from_str(&r.body).expect("boot-status is JSON");
                if v["phase"] == "ready" {
                    break;
                }
            }
        }
        assert!(Instant::now() < deadline, "daemon {tag} never reached phase ready");
        std::thread::sleep(Duration::from_millis(100));
    }
    d
}

struct Resp {
    status: u16,
    body: String,
}

fn try_http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Option<Resp> {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).ok()?;
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text.split_whitespace().nth(1)?.parse::<u16>().ok()?;
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Some(Resp { status, body })
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    try_http(port, method, path_and_query, body)
        .unwrap_or_else(|| panic!("{method} {path_and_query} on :{port} got no HTTP response"))
}

fn json(r: &Resp) -> serde_json::Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body))
}

fn add_user(d: &Daemon, username: &str, password: &str) {
    let r = http(
        d.port,
        "POST",
        &format!("/cli/users/add?token={}", d.owner),
        Some(&serde_json::json!({ "username": username, "password": password }).to_string()),
    );
    assert_eq!(r.status, 200, "users/add: {}", r.body);
}

fn login(d: &Daemon, username: &str, password: &str) -> Resp {
    http(
        d.port,
        "POST",
        "/cli/auth/login",
        Some(&serde_json::json!({ "username": username, "password": password }).to_string()),
    )
}

fn login_token(d: &Daemon, username: &str, password: &str) -> String {
    let r = login(d, username, password);
    assert_eq!(r.status, 200, "login on :{}: {}", d.port, r.body);
    json(&r)["token"].as_str().expect("login token").to_string()
}

async fn connect_events_ws(port: u16, token: &str) -> WsClient {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?path=&token={token}");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url).await.expect("events WS connect");
    let msg = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for hello")
        .expect("stream closed before hello")
        .expect("ws message Ok");
    match msg {
        Message::Text(t) => {
            let hello: serde_json::Value = serde_json::from_str(&t).expect("hello is JSON");
            assert_eq!(hello["kind"], "hello", "first frame must be hello: {hello}");
        }
        other => panic!("expected the hello frame, got {other:?}"),
    }
    ws
}

/// Wait for the server's Close frame and return its (code, reason).
async fn expect_close(ws: &mut WsClient, within: Duration, name: &str) -> (u16, String) {
    let deadline = Instant::now() + within;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "{name}: no close frame within {within:?}");
        match timeout(remaining, ws.next()).await {
            Ok(Some(Ok(Message::Close(Some(frame))))) => {
                return (u16::from(frame.code), frame.reason.to_string());
            }
            Ok(Some(Ok(Message::Close(None)))) => panic!("{name}: closed with no status code"),
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => panic!("{name}: socket error before a close frame: {e}"),
            Ok(None) => panic!("{name}: stream ended with no close frame"),
            Err(_) => panic!("{name}: still open after {within:?}"),
        }
    }
}

/// The socket is still open: nothing but event frames for `quiet`.
async fn expect_open(ws: &mut WsClient, quiet: Duration, name: &str) {
    let deadline = Instant::now() + quiet;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        match timeout(remaining, ws.next()).await {
            Err(_) => return,
            Ok(Some(Ok(Message::Close(f)))) => panic!("{name} was closed: {f:?}"),
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => panic!("{name} errored: {e}"),
            Ok(None) => panic!("{name} ended"),
        }
    }
}

fn sessions_file(d: &Daemon) -> PathBuf {
    d.home.join(".k2").join("connect-sessions.json")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_daemons_each_answer_for_themselves() {
    let a = spawn_daemon("a");
    let b = spawn_daemon("b");
    assert_ne!(a.port, b.port);

    // MS29 / MS81: two servers, two instanceIds; a second read of one server
    // is the same instance (no restart).
    let boot = |d: &Daemon| json(&http(d.port, "GET", "/boot-status", None));
    let (ba, bb) = (boot(&a), boot(&b));
    let ia = ba["instanceId"].as_str().expect("A instanceId").to_string();
    let ib = bb["instanceId"].as_str().expect("B instanceId").to_string();
    assert!(!ia.is_empty() && !ib.is_empty());
    assert_ne!(ia, ib, "two daemons must report two instanceIds");
    assert_eq!(boot(&a)["instanceId"].as_str(), Some(ia.as_str()));
    assert_eq!(ba["protocol"].as_u64(), Some(1));
    assert_eq!(bb["phase"], "ready");

    // A user exists on B only.
    add_user(&b, "anna", "correct horse 1");
    let tok_b = login_token(&b, "anna", "correct horse 1");
    assert_eq!(login(&a, "anna", "correct horse 1").status, 401, "anna is not a user on A");

    // MS32: tokens never cross servers.
    let who = |d: &Daemon, tok: &str| http(d.port, "GET", &format!("/cli/auth/whoami?token={tok}"), None);
    let wb = who(&b, &tok_b);
    assert_eq!(wb.status, 200, "{}", wb.body);
    assert_eq!(json(&wb)["role"], "member");
    assert_eq!(json(&wb)["owner"], false);
    assert_eq!(who(&a, &tok_b).status, 403, "B's login must be refused by A");
    assert_eq!(who(&b, &a.owner).status, 403, "A's owner token must be refused by B");
    assert_eq!(json(&who(&a, &a.owner))["role"], "owner");

    // The pool's signed-in status read, with B's own login.
    let summary = http(b.port, "GET", &format!("/cli/presence/summary?token={tok_b}"), None);
    assert_eq!(summary.status, 200, "{}", summary.body);
    assert!(json(&summary)["workspaces"].is_array());
    assert_eq!(
        http(a.port, "GET", &format!("/cli/presence/summary?token={tok_b}"), None).status,
        403
    );

    // MS44 b / MS82: authed traffic on B does not rewrite its session file.
    let file = sessions_file(&b);
    let ino_before = std::fs::metadata(&file).expect("B has a session file").ino();
    let bytes_before = std::fs::read(&file).expect("read B session file");
    for _ in 0..20 {
        assert_eq!(who(&b, &tok_b).status, 200);
    }
    assert_eq!(std::fs::metadata(&file).expect("file").ino(), ino_before, "steady traffic rewrote the file");
    assert_eq!(std::fs::read(&file).expect("read"), bytes_before);
    // A second login is a real change: written.
    let tok_b2 = login_token(&b, "anna", "correct horse 1");
    assert_ne!(std::fs::metadata(&file).expect("file").ino(), ino_before, "a new session must be saved");
    assert_eq!(who(&b, &tok_b2).status, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kick_and_revoke_close_codes_are_per_server() {
    let a = spawn_daemon("ka");
    let b = spawn_daemon("kb");
    add_user(&b, "anna", "correct horse 2");
    let tok = login_token(&b, "anna", "correct horse 2");

    let mut on_b = connect_events_ws(b.port, &tok).await;
    let mut on_a = connect_events_ws(a.port, &a.owner).await;

    // MS71 / MS44 a: B's owner kicks anna → 4001 "kicked", at once (not the
    // 5 s re-check).
    let kick = http(
        b.port,
        "POST",
        &format!("/cli/presence/kick?token={}", b.owner),
        Some(&serde_json::json!({ "username": "anna" }).to_string()),
    );
    assert_eq!(kick.status, 200, "kick: {}", kick.body);
    let (code, reason) = expect_close(&mut on_b, Duration::from_secs(4), "B events socket").await;
    assert_eq!((code, reason.as_str()), (4001, "kicked"));
    // After a revoke, whoami answers 403 (the pool's dead-login signal).
    assert_eq!(http(b.port, "GET", &format!("/cli/auth/whoami?token={tok}"), None).status, 403);
    // A is untouched by B's kick.
    expect_open(&mut on_a, Duration::from_millis(500), "A events socket").await;

    // A token that stops being valid without a kick (the user is disabled)
    // is closed by the 5 s re-check with 4003 "revoked" — not a kick.
    let tok2 = login_token(&b, "anna", "correct horse 2");
    let mut on_b2 = connect_events_ws(b.port, &tok2).await;
    let off = http(
        b.port,
        "POST",
        &format!("/cli/users/set-disabled?token={}", b.owner),
        Some(&serde_json::json!({ "username": "anna", "disabled": true }).to_string()),
    );
    assert_eq!(off.status, 200, "set-disabled: {}", off.body);
    let (code, reason) = expect_close(&mut on_b2, Duration::from_secs(8), "B events socket (re-check)").await;
    assert_eq!((code, reason.as_str()), (4003, "revoked"));
    expect_open(&mut on_a, Duration::from_millis(300), "A events socket").await;
}
