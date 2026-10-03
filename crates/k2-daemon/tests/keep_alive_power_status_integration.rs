//! 0.43.0 regression: `GET /cli/power/status` never consumed its request
//! head. The dispatcher only peeks the head before routing, so the
//! keep-alive loop peeked the same GET again and answered it again, without
//! end. Every later request on that socket read a power/status body: the
//! desktop's usage menu got `{keepAwake}` instead of `{harnesses}`, chat
//! and preset lists got an object, and the window crashed.
//!
//! This drives the real binary under a temp HOME over ONE keep-alive TCP
//! connection: power/status, then waiting-count, then usage/subscriptions,
//! and checks each response belongs to its own request. It also checks the
//! same with a Connect login session, the token a remote desktop uses.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

struct Daemon {
    child: std::process::Child,
    home: PathBuf,
    port: u16,
    owner: String,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn spawn_daemon() -> Daemon {
    let home = std::env::temp_dir().join(format!(
        "k2-keepalive-power-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(home.join(".k2")).expect("create temp HOME");
    let shim = home.join("agent-shim-empty");
    std::fs::create_dir_all(&shim).expect("create empty agent shim dir");
    let child = Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
        .env("HOME", &home)
        .env("K2_TEST_AGENT_SHIM_DIR", &shim)
        .env("K2SO_WATCHDOG_DISABLED", "1")
        .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
        .env("K2_SUBSCRIPTION_PROBE", "deny")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn k2-daemon");
    let port_file = home.join(".k2").join("daemon.port");
    let token_file = home.join(".k2").join("daemon.token");
    let deadline = Instant::now() + Duration::from_secs(20);
    let (port, owner) = loop {
        let port = read_trimmed(&port_file).and_then(|p| p.parse::<u16>().ok());
        if let (Some(port), Some(owner)) = (port, read_trimmed(&token_file)) {
            break (port, owner);
        }
        assert!(Instant::now() < deadline, "daemon never published daemon.port + daemon.token");
        std::thread::sleep(Duration::from_millis(50));
    };
    let d = Daemon { child, home, port, owner };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let mut conn = Conn::open(d.port);
        let (status, body) = conn.request("GET", "/boot-status", None);
        if status == 200 {
            let v: serde_json::Value = serde_json::from_str(&body).expect("boot-status is JSON");
            if v["phase"] == "ready" {
                break;
            }
        }
        assert!(Instant::now() < deadline, "daemon never reached phase ready");
        std::thread::sleep(Duration::from_millis(100));
    }
    d
}

/// One HTTP/1.1 keep-alive connection. Reads exactly one
/// Content-Length-framed response per request.
struct Conn {
    reader: BufReader<TcpStream>,
}

impl Conn {
    fn open(port: u16) -> Conn {
        let s = TcpStream::connect(("127.0.0.1", port)).expect("connect daemon");
        s.set_read_timeout(Some(Duration::from_secs(15))).expect("read timeout");
        Conn { reader: BufReader::new(s) }
    }

    fn request(&mut self, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        let body = body.unwrap_or("");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        self.reader.get_mut().write_all(head.as_bytes()).expect("write request");
        let mut status_line = String::new();
        self.reader.read_line(&mut status_line).expect("read status line");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .unwrap_or_else(|| panic!("no status in {status_line:?}"))
            .parse()
            .unwrap_or_else(|_| panic!("bad status in {status_line:?}"));
        let mut len: Option<usize> = None;
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("read header");
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                if k.eq_ignore_ascii_case("content-length") {
                    len = Some(v.trim().parse().expect("content-length is a number"));
                }
            }
        }
        let len = len.unwrap_or_else(|| panic!("{path}: response has no Content-Length"));
        let mut buf = vec![0u8; len];
        self.reader.read_exact(&mut buf).expect("read body");
        (status, String::from_utf8(buf).expect("utf-8 body"))
    }
}

fn json(body: &str, what: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{what}: not JSON ({e}): {body}"))
}

fn assert_each_response_is_its_own(port: u16, token: &str, who: &str) {
    let mut conn = Conn::open(port);

    let (s, b) = conn.request("GET", &format!("/cli/power/status?token={token}"), None);
    assert_eq!(s, 200, "{who}: power/status: {b}");
    assert!(json(&b, "power/status")["keepAwake"].is_object(), "{who}: power/status body: {b}");

    let (s, b) = conn.request("GET", &format!("/cli/feedback/waiting-count?token={token}"), None);
    assert_eq!(s, 200, "{who}: waiting-count: {b}");
    let v = json(&b, "waiting-count");
    assert!(
        v["count"].is_number(),
        "{who}: the request after power/status on the same socket got the wrong body: {b}"
    );

    let (s, b) = conn.request("GET", &format!("/cli/usage/subscriptions?token={token}"), None);
    assert_eq!(s, 200, "{who}: usage/subscriptions: {b}");
    assert!(
        json(&b, "usage/subscriptions")["harnesses"].is_array(),
        "{who}: usage/subscriptions got the wrong body: {b}"
    );

    let (s, b) = conn.request("GET", &format!("/cli/presets/list?token={token}"), None);
    assert_eq!(s, 200, "{who}: presets/list: {b}");
    assert!(json(&b, "presets/list").is_array(), "{who}: presets/list got the wrong body: {b}");
}

#[test]
fn power_status_answers_once_on_a_keep_alive_socket() {
    let d = spawn_daemon();

    assert_each_response_is_its_own(d.port, &d.owner, "owner token");

    // A Connect login, as a remote desktop holds.
    let mut conn = Conn::open(d.port);
    let (s, b) = conn.request(
        "POST",
        &format!("/cli/users/add?token={}", d.owner),
        Some(r#"{"username":"keepalive","password":"correct-horse-battery-9"}"#),
    );
    assert_eq!(s, 200, "users/add: {b}");
    let mut conn = Conn::open(d.port);
    let (s, b) = conn.request(
        "POST",
        &format!("/cli/users/set-role?token={}", d.owner),
        Some(r#"{"username":"keepalive","role":"owner"}"#),
    );
    assert_eq!(s, 200, "users/set-role: {b}");
    let mut conn = Conn::open(d.port);
    let (s, b) = conn.request(
        "POST",
        "/cli/auth/login",
        Some(r#"{"username":"keepalive","password":"correct-horse-battery-9"}"#),
    );
    assert_eq!(s, 200, "auth/login: {b}");
    let session = json(&b, "auth/login")["token"]
        .as_str()
        .unwrap_or_else(|| panic!("auth/login has no token: {b}"))
        .to_string();

    assert_each_response_is_its_own(d.port, &session, "login session");
}
