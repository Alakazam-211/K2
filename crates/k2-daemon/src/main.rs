//! K2SO daemon entry point.
//!
//! Launched by launchd (`~/Library/LaunchAgents/dev.k2.daemon.plist`,
//! `KeepAlive: true`), this process owns the persistent-agent runtime —
//! SQLite, the heartbeat scheduler, the companion WebSocket + ngrok tunnel,
//! the agent_hooks HTTP server — so that agents keep running while the
//! Tauri app is quit and the laptop lid is closed.
//!
//! On Windows release builds: `windows_subsystem = "windows"` so auto-start
//! from the thin client does not open a bare console window on the desktop
//! (k2-daemon is a CUI binary by default; the GUI app is already WINDOWS).

// Hide the console window for release Windows builds. Debug/dev builds keep
// a console so `cargo run -p k2-daemon` still shows logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//!
//! # Tokio runtime
//!
//! The binary is async-first: a multi-thread `#[tokio::main]` runtime hosts
//! the HTTP accept loop and (as more modules migrate in) the scheduler
//! ticks, companion WS, and the daemon→Tauri event channel. Each inbound
//! connection is handled by its own `tokio::spawn` task so a slow or
//! long-lived connection (future WS upgrades, streaming responses) never
//! stalls the accept loop.
//!
//! # Scaffolding pass (0.33.0-dev)
//!
//! Binds a loopback TCP listener on a random port and publishes the
//! port + freshly-generated auth token through four filesystem
//! channels:
//!
//! - `~/.k2so/daemon.port` / `~/.k2so/daemon.token` — daemon-specific
//!   addresses used by Tauri's `DaemonClient` and by
//!   `k2so daemon status` to reach the daemon's control-plane
//!   endpoints regardless of who owns the CLI-facing HTTP surface.
//! - `~/.k2so/heartbeat.port` / `~/.k2so/heartbeat.token` — **the**
//!   CLI-facing surface since Phase 4 H7. Pre-Phase-4 this file was
//!   owned by Tauri's agent_hooks HTTP server; H7 retires that
//!   listener and makes the daemon the sole writer. The CLI
//!   (`cli/k2so`) + every filesystem hook script reads these files
//!   to discover the server on every request, so a daemon restart
//!   (which rotates the random port) propagates instantly without
//!   any running consumer needing to be restarted itself.

// The whole daemon lives in the library (one module graph, one copy of
// every static, each unit test compiled and run once). See
// `k2_daemon::daemon_main`.
fn main() {
    k2_daemon::daemon_main::run();
}
