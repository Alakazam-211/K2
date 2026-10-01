//! Heartbeat S6 — the Keep awake routes on a real headless daemon.
//!
//! Spawns the real `k2-daemon` binary under a temp HOME with
//! `K2_HEARTBEAT_NO_SELF_HEAL=1`, so the power layer is the no-op backend:
//! no assertion, no helper, no `pmset`, no real power setting is touched.
//! No client is attached. Pins:
//!   - `GET /cli/power/status` answers with the daemon's own state;
//!   - `POST /cli/power/keep-awake` takes each mode, saves it to the
//!     daemon's settings, and reports what is really held (the no-op
//!     backend has no lid support, so Always is "lid open only");
//!   - a bad mode is a 400 and changes nothing; GET on the POST route is
//!     405; a Member login may use both (route_policy Member).

#![cfg(unix)]

use std::io::{Read, Write};
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

struct Resp {
    status: u16,
    body: String,
}

fn try_http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Option<Resp> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
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

fn spawn_daemon() -> Daemon {
    let home = std::env::temp_dir().join(format!(
        "k2-keep-awake-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".k2")).expect("temp HOME");
    let shim_dir = home.join("agent-shim-empty");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    let child = Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
        .env("HOME", &home)
        .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
        .env("K2_TEST_AGENT_SHIM_DIR", &shim_dir)
        .env("K2SO_WATCHDOG_DISABLED", "1")
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
        if let Some(r) = try_http(d.port, "GET", "/boot-status", None) {
            if r.status == 200 && json(&r)["phase"] == "ready" {
                break;
            }
        }
        assert!(Instant::now() < deadline, "daemon never reached phase ready");
        std::thread::sleep(Duration::from_millis(100));
    }
    d
}

fn set(d: &Daemon, token: &str, body: serde_json::Value) -> Resp {
    http(d.port, "POST", &format!("/cli/power/keep-awake?token={token}"), Some(&body.to_string()))
}

fn saved_mode(d: &Daemon) -> String {
    let raw = std::fs::read_to_string(d.home.join(".k2").join("settings.json")).expect("settings.json written");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("settings JSON");
    v["keepAwake"]["mode"].as_str().expect("keepAwake.mode saved").to_string()
}

#[test]
fn keep_awake_routes_on_a_headless_daemon() {
    let d = spawn_daemon();
    let owner = d.owner.clone();

    let r = http(d.port, "GET", &format!("/cli/power/status?token={owner}"), None);
    assert_eq!(r.status, 200, "{}", r.body);
    let v = json(&r);
    assert_eq!(v["keepAwake"]["mode"], "off");
    assert_eq!(v["keepAwake"]["state"], "off");
    assert_eq!(v["keepAwake"]["held"], false);
    assert_eq!(v["keepAwake"]["batteryFloorPercent"], 20);

    // Always: the no-op backend takes the lid-open hold and has no lid
    // support, so the honest answer is "lid open only".
    let r = set(&d, &owner, serde_json::json!({ "mode": "always" }));
    assert_eq!(r.status, 200, "{}", r.body);
    let v = json(&r);
    assert_eq!(v["success"], true);
    assert_eq!(v["keepAwake"]["mode"], "always");
    assert_eq!(v["keepAwake"]["state"], "lid_open_only", "{v}");
    assert_eq!(v["keepAwake"]["label"], "Awake (lid open only)");
    assert_eq!(v["keepAwake"]["held"], true);
    assert_eq!(v["keepAwake"]["lidHeld"], false);
    let detail = v["keepAwake"]["detail"].as_str().expect("detail");
    assert!(detail.starts_with("Lid closed will still sleep"), "{detail}");
    assert_eq!(saved_mode(&d), "always");

    // Working with no agent working: not held.
    let r = set(&d, &owner, serde_json::json!({ "mode": "working" }));
    assert_eq!(r.status, 200, "{}", r.body);
    let v = json(&r);
    assert_eq!(v["keepAwake"]["state"], "waiting", "{v}");
    assert_eq!(v["keepAwake"]["held"], false);
    assert_eq!(v["keepAwake"]["workingSessions"], 0);
    assert_eq!(saved_mode(&d), "working");

    // A bad mode is refused and changes nothing.
    let r = set(&d, &owner, serde_json::json!({ "mode": "sometimes" }));
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains("off|working|always"), "{}", r.body);
    assert_eq!(saved_mode(&d), "working");

    // POST-only.
    let r = http(d.port, "GET", &format!("/cli/power/keep-awake?token={owner}"), None);
    assert_eq!(r.status, 405, "{}", r.body);

    // No token: refused.
    let r = http(d.port, "GET", "/cli/power/status", None);
    assert_eq!(r.status, 403, "{}", r.body);

    // A Member login may read and set it (route_policy Member).
    let r = http(
        d.port,
        "POST",
        &format!("/cli/users/add?token={owner}"),
        Some(&serde_json::json!({ "username": "anna", "password": "correct horse 6" }).to_string()),
    );
    assert_eq!(r.status, 200, "users/add: {}", r.body);
    let r = http(
        d.port,
        "POST",
        "/cli/auth/login",
        Some(&serde_json::json!({ "username": "anna", "password": "correct horse 6" }).to_string()),
    );
    assert_eq!(r.status, 200, "login: {}", r.body);
    let member = json(&r)["token"].as_str().expect("login token").to_string();
    let r = http(d.port, "GET", &format!("/cli/power/status?token={member}"), None);
    assert_eq!(r.status, 200, "{}", r.body);
    let r = set(&d, &member, serde_json::json!({ "mode": "off" }));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(json(&r)["keepAwake"]["state"], "off");
    assert_eq!(saved_mode(&d), "off");
}
