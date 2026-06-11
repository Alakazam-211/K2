//! H7 of Phase 4 — daemon owns `heartbeat.port` + `heartbeat.token`
//! eagerly at startup.
//!
//! Before H7, the Tauri app's `agent_hooks` HTTP listener was the
//! primary writer of these files; the daemon only claimed them
//! when the 30-second watchdog noticed nobody else was listening.
//! After H7, the daemon writes them eagerly so the CLI + every
//! launchd hook script has a single authoritative source of
//! truth — independent of whether the desktop app is running.
//!
//! Spinning up the daemon from a test binary is heavy (it binds
//! a TCP listener, starts the scheduler + watchdog, opens the
//! shared DB), so instead this test spawns the compiled binary
//! as a subprocess and asserts on the filesystem side-effects.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// Per-test HOME override so parallel runs (and reruns after
/// panic) don't clobber each other's port files. Returns the
/// absolute path to the test's `.k2so` dir.
fn isolated_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        "k2so-h7-home-{}-{}-{}",
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

/// Locate the daemon binary cargo compiled for this test run.
/// `$CARGO_BIN_EXE_k2-daemon` is set automatically when a test
/// is compiled in the k2-daemon crate.
fn daemon_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_k2-daemon"))
}

#[tokio::test(flavor = "current_thread")]
async fn daemon_writes_heartbeat_port_eagerly_on_startup() {
    let home = isolated_home("eager");
    let k2so_dir = home.join(".k2");

    // Spawn the daemon as a child process with $HOME redirected
    // so its writes land in our scratch dir, not the real
    // ~/.k2so.
    let mut child = Command::new(daemon_binary())
        .env("HOME", &home)
        // Set K2SO_WATCHDOG_DISABLED so the harness watchdog
        // doesn't add log noise to this specific test.
        .env("K2SO_WATCHDOG_DISABLED", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn daemon");

    // Poll for up to 5 seconds waiting for heartbeat.port to
    // appear. Success should be much faster than that; the
    // generous timeout is just for slow CI.
    let heartbeat_port = k2so_dir.join("heartbeat.port");
    let heartbeat_token = k2so_dir.join("heartbeat.token");
    let daemon_port = k2so_dir.join("daemon.port");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_hb_port = false;
    let mut saw_hb_token = false;
    let mut saw_daemon_port = false;
    while Instant::now() < deadline {
        if heartbeat_port.exists() {
            saw_hb_port = true;
        }
        if heartbeat_token.exists() {
            saw_hb_token = true;
        }
        if daemon_port.exists() {
            saw_daemon_port = true;
        }
        if saw_hb_port && saw_hb_token && saw_daemon_port {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Kill the daemon before asserting so a failed run doesn't
    // leak a process.
    let _ = child.kill();
    let _ = child.wait();

    assert!(saw_hb_port, "daemon did not write heartbeat.port");
    assert!(saw_hb_token, "daemon did not write heartbeat.token");
    // H7's scope note: daemon.port + daemon.token stay around
    // for Tauri's DaemonClient internal use.
    assert!(saw_daemon_port, "daemon did not write daemon.port");

    // Port values in daemon.port and heartbeat.port should be
    // identical (the daemon writes the same `port` int into both).
    let hb = std::fs::read_to_string(&heartbeat_port)
        .unwrap()
        .trim()
        .to_string();
    let dp = std::fs::read_to_string(&daemon_port)
        .unwrap()
        .trim()
        .to_string();
    assert_eq!(hb, dp, "heartbeat.port ({hb}) != daemon.port ({dp})");
    assert!(hb.parse::<u16>().is_ok(), "heartbeat.port not a u16: {hb}");

    // Clean up the scratch home.
    let _ = std::fs::remove_dir_all(&home);
}
