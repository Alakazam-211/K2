//! Windows: `k2-daemon --keep-alive` is launchd's KeepAlive. A daemon that
//! dies is started again; a daemon asked to stop (`/cli/daemon/stop`, a
//! quit) ends the supervisor too.
//!
//! Boots the REAL daemon binary under a throwaway HOME (never the real
//! `~/.k2`), with an empty agent shim dir so nothing can spawn a real CLI.

#![cfg(windows)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Ends the supervisor and everything under it on every exit path.
struct KillTree(Child);

impl Drop for KillTree {
    fn drop(&mut self) {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &self.0.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.0.wait();
    }
}

fn http(port: u16, method: &str, path: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect daemon");
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(s, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 0\r\n\r\n").unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

/// The daemon's port once its `/boot-status` answers ready.
fn wait_ready(k2: &Path, what: &str) -> u16 {
    let end = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(port) = std::fs::read_to_string(k2.join("daemon.port")).ok().and_then(|s| s.trim().parse::<u16>().ok()) {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() && http(port, "GET", "/boot-status").contains("\"ready\"") {
                return port;
            }
        }
        assert!(Instant::now() < end, "{what}: the daemon never came up ready");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The supervisor's one daemon child.
fn daemon_child(supervisor: u32, what: &str) -> u32 {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        let kids = k2_core::win_proc::children_of(supervisor);
        if let [one] = kids[..] {
            return one;
        }
        assert!(Instant::now() < end, "{what}: expected one daemon under the supervisor, got {kids:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_killed_daemon_comes_back_and_a_stop_ends_the_supervisor() {
    let home: PathBuf = std::env::temp_dir().join(format!("k2-keepalive-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    let k2 = home.join(".k2");
    std::fs::create_dir_all(&k2).unwrap();
    let shim = home.join("agent-shim-empty");
    std::fs::create_dir_all(&shim).unwrap();
    let rt = k2_core::test_env::DaemonRuntime::new(&home);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_k2-daemon"));
    cmd.arg("--keep-alive")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .envs(rt.envs())
        .env("K2_TEST_AGENT_SHIM_DIR", &shim)
        .env("K2SO_WATCHDOG_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut sup = KillTree(cmd.spawn().expect("spawn the supervisor"));
    let sup_pid = sup.0.id();

    let port = wait_ready(&k2, "first boot");
    let first = daemon_child(sup_pid, "first boot");

    // A crash: the daemon dies without a stop. The supervisor starts it again.
    assert!(k2_core::win_proc::terminate(first), "kill the daemon");
    let end = Instant::now() + Duration::from_secs(30);
    let second = loop {
        let kids = k2_core::win_proc::children_of(sup_pid);
        if let [one] = kids[..] {
            if one != first {
                break one;
            }
        }
        assert!(Instant::now() < end, "the killed daemon was not started again: {kids:?}");
        std::thread::sleep(Duration::from_millis(100));
    };
    let port2 = wait_ready(&k2, "after the kill");
    assert_eq!(port2, port, "the restarted daemon takes its port back");
    assert!(sup.0.try_wait().unwrap().is_none(), "the supervisor keeps running");

    // A quit: /cli/daemon/stop (the owner's token) ends the daemon AND the
    // supervisor, with exit code 0, and nothing comes back.
    let token = std::fs::read_to_string(k2.join("daemon.token")).expect("daemon.token");
    let r = http(port2, "POST", &format!("/cli/daemon/stop?token={}", token.trim()));
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    let end = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(st) = sup.0.try_wait().unwrap() {
            break st;
        }
        assert!(Instant::now() < end, "the supervisor did not end after a stop");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(status.code(), Some(0), "a quit ends supervision cleanly");
    assert!(!k2_core::win_proc::pid_alive(second), "the stopped daemon is gone");
    std::thread::sleep(Duration::from_secs(2));
    assert!(TcpStream::connect(("127.0.0.1", port2)).is_err(), "nothing started it again");
    drop(rt);
    let _ = std::fs::remove_dir_all(&home);
}
