//! Sub-fix A of Phase 2.5 finding #547 — daemon port stays the same
//! across restarts.
//!
//! Pre-fix the daemon called `TcpListener::bind("127.0.0.1:0")` on
//! every boot, so a `launchctl kickstart -k` or app-update restart
//! minted a new ephemeral port. Renderer-side caches (the
//! `daemon_ws_url` promise in `kessel/daemon-ws.ts`) held onto the
//! previous port and silently sent HTTP traffic to a dead socket
//! until they hit ECONNREFUSED.
//!
//! Post-fix the daemon reads `~/.k2so/daemon.port` first, attempts
//! to bind that exact port, and falls back to ephemeral only when
//! it's taken. This test spawns the compiled daemon binary twice
//! in a row (with a between-runs grace window for SO_REUSEADDR
//! recycling) and verifies the published port is identical across
//! both invocations.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// Per-test HOME override so parallel runs don't clobber each
/// other's port files. Matches the pattern in
/// `heartbeat_port_claim_integration.rs`.
fn isolated_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        "k2so-port-stab-home-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".k2")).unwrap();
    home
}

fn daemon_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_k2-daemon"))
}

/// Kill + reap the daemon child on every exit path (a panic between
/// spawn and the explicit kill must not leak a real `k2-daemon`).
struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawn the daemon under the given HOME, wait for it to publish
/// `daemon.port`, return the published port, then kill the child.
///
/// Spawn guard (2026-09-12): this boots the REAL daemon binary under a
/// temp HOME. It needs no agent, so `K2_TEST_AGENT_SHIM_DIR` points at an
/// EMPTY dir — any spawn attempt inside the child fails loudly instead of
/// resolving the real `~/.local/bin/claude` (which opened a browser OAuth
/// login under a fresh HOME). `Command` inherits env, so the child sees it.
fn boot_once_and_read_port(home: &PathBuf) -> u16 {
    let k2so_dir = home.join(".k2");
    let daemon_port = k2so_dir.join("daemon.port");
    let shim_dir = home.join("agent-shim-empty");
    std::fs::create_dir_all(&shim_dir).unwrap();

    let child = KillOnDrop(
        Command::new(daemon_binary())
            .env("HOME", home)
            .env("K2_TEST_AGENT_SHIM_DIR", &shim_dir)
            .env("K2SO_WATCHDOG_DISABLED", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn daemon"),
    );

    // Poll for the port file to appear and stabilize.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last_port: Option<u16> = None;
    while Instant::now() < deadline {
        if daemon_port.exists() {
            if let Ok(raw) = std::fs::read_to_string(&daemon_port) {
                if let Ok(p) = raw.trim().parse::<u16>() {
                    last_port = Some(p);
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Tear down before asserting so a failed run can't leak a process.
    drop(child);

    last_port.expect("daemon never published a parseable daemon.port")
}

#[test]
fn daemon_reuses_published_port_across_restarts() {
    let home = isolated_home("reuse");

    let first_port = boot_once_and_read_port(&home);
    assert!(first_port > 0, "first boot port must be non-zero");

    // Give the kernel a moment to release the bound socket. macOS
    // honors SO_REUSEADDR for loopback bind-after-close immediately,
    // but a small sleep here makes the test robust to slow CI.
    std::thread::sleep(Duration::from_millis(200));

    let second_port = boot_once_and_read_port(&home);
    assert_eq!(
        first_port, second_port,
        "daemon did not reuse published port across restarts \
         (finding #547 regression): first={first_port}, second={second_port}"
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&home);
}
