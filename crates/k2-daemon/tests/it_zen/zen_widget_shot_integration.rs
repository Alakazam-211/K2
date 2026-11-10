//! `k2 zen widget preview --dir … --shot` (0.46.3) over the REAL
//! `k2-daemon` binary under a temp HOME, no client and no Zen folder: the
//! shot route is a POST row (GET is 405), answers K2's real widget frame
//! and a host page for a staged built-in card, and writes nothing under
//! `~/.k2/zen`. Never touches the real `~/.k2`. Fail loudly.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{json, Value as J};

struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn new_home(tag: &str) -> Home {
    let home = std::env::temp_dir().join(format!("k2-zen-shot-{tag}-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(home.join(".k2")).expect("create temp HOME/.k2");
    std::fs::create_dir_all(home.join("agent-shim-empty")).expect("shim dir");
    Home(home)
}

fn request(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let s = TcpStream::connect(("127.0.0.1", port)).expect("connect daemon");
    s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let mut r = BufReader::new(s);
    let body = body.unwrap_or("");
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    r.get_mut().write_all(head.as_bytes()).expect("write request");
    let mut status = String::new();
    r.read_line(&mut status).expect("status line");
    let code: u16 = status.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("bad status {status:?}"));
    let mut len = None;
    loop {
        let mut line = String::new();
        r.read_line(&mut line).expect("header");
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = Some(v.trim().parse::<usize>().expect("content-length"));
            }
        }
    }
    let mut buf = vec![0u8; len.unwrap_or_else(|| panic!("{path}: no Content-Length"))];
    r.read_exact(&mut buf).expect("body");
    (code, String::from_utf8(buf).expect("utf-8 body"))
}

fn wait_for<T>(what: &str, within: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + within;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out after {within:?} waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn read_trimmed(p: &Path) -> Option<String> {
    let t = std::fs::read_to_string(p).ok()?.trim().to_string();
    (!t.is_empty()).then_some(t)
}

struct Daemon {
    child: std::process::Child,
    port: u16,
    owner: String,
    _rt: k2_core::test_env::DaemonRuntime,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_daemon(home: &Path) -> Daemon {
    let rt = k2_core::test_env::DaemonRuntime::new(home);
    let child = Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
        .env("HOME", home)
        .envs(rt.envs())
        .env("K2_TEST_AGENT_SHIM_DIR", home.join("agent-shim-empty"))
        .env("K2SO_WATCHDOG_DISABLED", "1")
        .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
        .env("K2_SUBSCRIPTION_PROBE", "deny")
        .env_remove("K2_HOOK_SOCK")
        .env_remove("K2_HOOK_TOKEN")
        .env_remove("K2SO_HOOK_SOCK")
        .env_remove("K2SO_HOOK_TOKEN")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn k2-daemon");
    let k2 = home.join(".k2");
    let (port, owner) = wait_for("daemon.port + daemon.token", Duration::from_secs(20), || {
        let port = read_trimmed(&k2.join("daemon.port")).and_then(|p| p.parse::<u16>().ok())?;
        Some((port, read_trimmed(&k2.join("daemon.token"))?))
    });
    let d = Daemon { child, port, owner, _rt: rt };
    wait_for("boot-status ready", Duration::from_secs(30), || {
        TcpStream::connect(("127.0.0.1", d.port)).ok()?;
        let (s, b) = request(d.port, "GET", "/boot-status", None);
        (s == 200 && serde_json::from_str::<J>(&b).ok()?["phase"] == "ready").then_some(())
    });
    d
}

fn fleet_grid_files() -> J {
    let w = k2_core::zen::builtin_widgets::BUILTIN_WIDGETS
        .iter()
        .find(|w| w.name == "fleet-grid" && w.version == 2)
        .expect("k2:fleet-grid@2");
    J::Array(w.files.iter().map(|(n, t)| json!({"name": n, "text": t})).collect())
}

#[test]
fn the_shot_route_answers_a_real_frame_and_writes_nothing() {
    let home = new_home("bin");
    let d = spawn_daemon(&home.0);
    let tok = d.owner.clone();
    let zen = home.0.join(".k2").join("zen");
    assert!(!zen.exists(), "a fresh HOME has no Zen folder");

    // A POST row: a GET never reaches the handler.
    let (s, _) = request(d.port, "GET", &format!("/cli/zen/widget/shot?token={tok}"), None);
    assert_eq!(s, 405, "GET on the shot route");

    let body = json!({"files": fleet_grid_files(), "fallback": "fleet-grid", "scheme": "light", "config": {"source": "fleet"}}).to_string();
    let (s, b) = request(d.port, "POST", &format!("/cli/zen/widget/shot?token={tok}"), Some(&body));
    assert_eq!(s, 200, "{b}");
    let v: J = serde_json::from_str(&b).expect("JSON");
    assert_eq!((v["ok"].as_bool(), v["scheme"].as_str(), v["widget"].as_str()), (Some(true), Some("light"), Some("fleet-grid")), "{v}");
    let nonce = v["nonce"].as_str().expect("nonce");
    let frame = v["frame"].as_str().expect("frame");
    assert!(frame.contains(&format!("script-src 'nonce-{nonce}' 'wasm-unsafe-eval'")), "the widget CSP with this nonce");
    assert!(frame.contains("color-scheme:light!important"), "the Garden's scheme wins");
    let host = v["host"].as_str().expect("host");
    assert!(host.contains("__K2_SHOT_FRAME__") && host.contains("\"zen-widgets-v1\""), "the host page and its hello");
    assert!(!zen.exists(), "a shot makes no Zen folder");

    // Without a token: refused.
    let (s, _) = request(d.port, "POST", "/cli/zen/widget/shot", Some(&body));
    assert_eq!(s, 403, "no token");
}
