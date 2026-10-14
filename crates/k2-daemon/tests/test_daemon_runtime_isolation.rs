//! A spawned test daemon's private runtime dir (the rendered frpc.toml)
//! lives inside its own temp root, never the real user runtime dir.
//!
//! 2026-10-14 incident: test daemons spawned with a temp `$HOME` but the
//! inherited `TMPDIR` / `XDG_RUNTIME_DIR` resolved the REAL
//! `$TMPDIR/k2-runtime` (macOS) or `/run/user/<uid>/k2` (Linux). A tunnel
//! release there deleted the production frpc.toml and the stop path
//! `pkill`ed the production frpc by config path, so the production tunnel
//! crash-looped for two hours. `k2_core::test_env::DaemonRuntime` gives
//! every spawned daemon its own TMPDIR / XDG_RUNTIME_DIR; this proves it
//! through the daemon's own `/cli/tunnel/status` (`frpc_config`).

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

struct Daemon {
    child: std::process::Child,
    root: PathBuf,
    port: u16,
    owner: String,
    _rt: k2_core::test_env::DaemonRuntime,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn read_trimmed(p: &Path) -> Option<String> {
    let t = std::fs::read_to_string(p).ok()?.trim().to_string();
    (!t.is_empty()).then_some(t)
}

fn get(port: u16, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect daemon");
    s.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
    write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").expect("write");
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).expect("HTTP status");
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

fn spawn_daemon() -> Daemon {
    let root = k2_core::test_env::unique_temp_path("rt-isolation");
    std::fs::create_dir_all(root.join(".k2")).expect("temp HOME");
    let shim = root.join("agent-shim-empty");
    std::fs::create_dir_all(&shim).expect("shim dir");
    let rt = k2_core::test_env::DaemonRuntime::new(&root);
    let child = Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
        .env("HOME", &root)
        .envs(rt.envs())
        .env("K2_TEST_AGENT_SHIM_DIR", &shim)
        .env("K2SO_WATCHDOG_DISABLED", "1")
        .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
        .env("K2_SUBSCRIPTION_PROBE", "deny")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn k2-daemon");
    let deadline = Instant::now() + Duration::from_secs(20);
    let (port, owner) = loop {
        let port = read_trimmed(&root.join(".k2/daemon.port")).and_then(|p| p.parse::<u16>().ok());
        if let (Some(port), Some(owner)) = (port, read_trimmed(&root.join(".k2/daemon.token"))) {
            break (port, owner);
        }
        assert!(Instant::now() < deadline, "daemon never published daemon.port + daemon.token");
        std::thread::sleep(Duration::from_millis(50));
    };
    let d = Daemon { child, root, port, owner, _rt: rt };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (code, body) = get(d.port, "/boot-status");
        if code == 200 && body.contains("\"ready\"") {
            break;
        }
        assert!(Instant::now() < deadline, "daemon never reached phase ready: {code} {body}");
        std::thread::sleep(Duration::from_millis(100));
    }
    d
}

#[test]
fn a_spawned_test_daemon_keeps_its_runtime_dir_inside_its_temp_root() {
    let d = spawn_daemon();
    let (code, body) = get(d.port, &format!("/cli/tunnel/status?token={}", d.owner));
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|e| panic!("JSON ({e}): {body}"));
    let cfg = PathBuf::from(v["frpc_config"].as_str().unwrap_or_else(|| panic!("no frpc_config: {v}")));

    let root = std::fs::canonicalize(&d.root).expect("canonical root");
    let parent = std::fs::canonicalize(cfg.parent().expect("config parent"))
        .unwrap_or_else(|e| panic!("runtime dir {} was not created: {e}", cfg.display()));
    assert!(
        parent.starts_with(&root),
        "the test daemon's runtime dir {} must be inside its temp root {}",
        parent.display(),
        root.display()
    );
    // The private runtime dir proper (TMPDIR/k2-runtime on macOS,
    // XDG_RUNTIME_DIR/k2 on Linux), not the ~/.k2 legacy fallback.
    #[cfg(target_os = "macos")]
    assert_eq!(parent, root.join("tmp").join("k2-runtime"));
    #[cfg(target_os = "linux")]
    assert_eq!(parent, root.join("run").join("k2"));
    for real in k2_core::runtime_dir::real_runtime_dirs() {
        assert!(!parent.starts_with(&real), "{} is the real runtime dir {}", parent.display(), real.display());
    }
}
