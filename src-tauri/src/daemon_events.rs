//! Tauri-side subscriber for the daemon's `/events` WebSocket.
//!
//! When the k2so-daemon is running, it publishes agent-hook events
//! (`HookEvent::AgentLifecycle`, `::AgentReply`, …) as JSON frames on
//! `ws://127.0.0.1:<port>/events?token=<t>`. This module runs a
//! background thread that connects to that stream, decodes each frame,
//! and re-emits it through the Tauri app's existing event bus so the
//! React UI sees daemon-originated events identically to ones emitted by
//! src-tauri's own agent_hooks server.
//!
//! The subscribe thread reconnects on disconnect with bounded
//! exponential backoff. If the daemon never came up (e.g. release bundle
//! not yet installed on this machine), the thread keeps retrying quietly
//! — the Tauri-side agent_hooks server handles everything in the
//! meantime, so the UI degrades gracefully.

use std::net::TcpStream;
use std::time::Duration;

use serde::Deserialize;
use tauri::{AppHandle, Emitter};
use tungstenite::client::IntoClientRequest;
use tungstenite::{client::client, Message};

use k2_core::log_debug;

/// Wire shape matches `crates/k2so-daemon/src/events.rs::WireEvent`.
#[derive(Debug, Clone, Deserialize)]
struct WireEvent {
    event: String,
    payload: serde_json::Value,
}

/// Spawn the subscribe thread. Idempotent to call once per app launch —
/// multiple invocations would just open multiple subscriptions, which is
/// harmless but wasteful. Returns immediately; the actual WS connect
/// runs on the spawned thread.
pub fn spawn_subscriber(app_handle: AppHandle) {
    std::thread::Builder::new()
        .name("daemon-events-subscriber".into())
        .spawn(move || run_subscriber(app_handle))
        .ok();
}

/// Long-lived subscriber loop with reconnect backoff.
fn run_subscriber(app_handle: AppHandle) {
    // 2s → 4s → 8s → 16s → 30s and cap. Plenty forgiving for a daemon
    // that's still booting while the UI is eager.
    let backoffs = [2u64, 4, 8, 16, 30];
    let mut attempt = 0usize;

    loop {
        match connect_once(&app_handle) {
            ConnectOutcome::Handled => {
                // Graceful close → reset backoff, reconnect.
                attempt = 0;
            }
            ConnectOutcome::NoDaemon => {
                // Daemon unavailable. Use the full backoff — no point
                // hammering when there's nothing listening.
            }
            ConnectOutcome::Error => {
                // Transient error (handshake, read, write). Same policy.
            }
        }

        let wait = backoffs[attempt.min(backoffs.len() - 1)];
        attempt = attempt.saturating_add(1);
        std::thread::sleep(Duration::from_secs(wait));
    }
}

enum ConnectOutcome {
    Handled,
    NoDaemon,
    Error,
}

/// Single connect attempt. Returns once the connection closes or an error
/// surfaces. Does NOT panic — every error path logs and returns.
fn connect_once(app_handle: &AppHandle) -> ConnectOutcome {
    let (port, token) = match read_daemon_credentials() {
        Some(p) => p,
        None => return ConnectOutcome::NoDaemon,
    };

    let url = format!("ws://127.0.0.1:{port}/events?token={token}");
    let request = match url.clone().into_client_request() {
        Ok(r) => r,
        Err(e) => {
            log_debug!("[daemon-events] bad url {url}: {e}");
            return ConnectOutcome::Error;
        }
    };

    // Open a raw TCP socket ourselves so we can set a read timeout; a
    // frozen daemon shouldn't freeze this thread.
    let tcp = match TcpStream::connect(format!("127.0.0.1:{port}")) {
        Ok(t) => t,
        Err(e) => {
            // Only log noisily the first time per app boot; otherwise
            // it's spam when the daemon isn't installed.
            log_debug!("[daemon-events] connect 127.0.0.1:{port}: {e}");
            return ConnectOutcome::NoDaemon;
        }
    };
    // Long enough to cover an idle daemon, short enough to detect a
    // truly dead process in under a minute.
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(45)));

    let (mut ws, _response) = match client(request, tcp) {
        Ok(pair) => pair,
        Err(e) => {
            log_debug!("[daemon-events] ws handshake: {e}");
            return ConnectOutcome::Error;
        }
    };
    log_debug!("[daemon-events] subscribed to daemon on port {port}");

    // Phase 2.5 fix (finding #547): broadcast a connected event so
    // renderer stores that failed their initial `settingsGet()` /
    // `daemonCliGet()` calls can re-run them. Without this signal,
    // an early-boot HTTP failure left every Zustand baseline at its
    // hard-coded defaults until the user closed and reopened the
    // app — and any UI write in the meantime would persist those
    // defaults, overwriting real settings. Empty payload — the
    // event itself is the signal; consumers re-fetch fresh state.
    if let Err(e) = app_handle.emit("daemon:connected", serde_json::Value::Null) {
        log_debug!("[daemon-events] emit daemon:connected failed: {e}");
    }

    loop {
        match ws.read() {
            Ok(Message::Text(txt)) => {
                let frame: WireEvent = match serde_json::from_str(&txt) {
                    Ok(f) => f,
                    Err(e) => {
                        log_debug!("[daemon-events] bad frame json: {e}: {txt}");
                        continue;
                    }
                };
                // Re-emit through Tauri. The event name comes from the
                // daemon side and matches HookEvent::event_name() — the
                // same strings the React frontend already listens for.
                if let Err(e) = app_handle.emit(&frame.event, frame.payload) {
                    log_debug!("[daemon-events] emit {} failed: {e}", frame.event);
                }
            }
            Ok(Message::Binary(_)) => {
                // Daemon only sends text. Ignore unexpected binary.
            }
            Ok(Message::Ping(p)) => {
                let _ = ws.send(Message::Pong(p));
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Close(_)) | Ok(Message::Frame(_)) => {
                log_debug!("[daemon-events] connection closed, will reconnect");
                return ConnectOutcome::Handled;
            }
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // Read timeout fires when there's been no traffic — that
                // just means nothing has happened on the daemon. Keep
                // the connection alive; no need to reconnect.
                continue;
            }
            Err(e) => {
                log_debug!("[daemon-events] read error: {e}");
                return ConnectOutcome::Error;
            }
        }
    }
}

/// Read `~/.k2so/daemon.port` + `~/.k2so/daemon.token`. `None` if either
/// is missing — the daemon isn't running (or not yet). Duplicated from
/// `daemon_client` only because the client's blocking reqwest semantics
/// don't fit this thread's shape; the file paths stay in lockstep.
fn read_daemon_credentials() -> Option<(u16, String)> {
    let home = dirs::home_dir()?;
    let port_path = home.join(".k2so/daemon.port");
    let token_path = home.join(".k2so/daemon.token");
    let port = std::fs::read_to_string(&port_path).ok()?.trim().parse().ok()?;
    let token = std::fs::read_to_string(&token_path).ok()?.trim().to_string();
    if token.is_empty() {
        return None;
    }
    Some((port, token))
}
