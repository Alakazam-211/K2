//! Phase 2 Unit 3 — daemon-side `TerminalEventSink`.
//!
//! Routes `k2_core::terminal::event_sink::TerminalEventSink` events
//! (`on_title`, `on_bell`, `on_exit`, `on_grid_update`) onto the
//! daemon's broadcast channel as `WireEvent` frames. Tauri's
//! `daemon_events.rs` receives the frames over `/events` WS and
//! re-emits them via `AppHandle::emit(&event_name, payload)` — the
//! same event names the renderer already listens to
//! (`terminal:grid:<id>`, `terminal:title:<id>`, etc.). The renderer
//! contract is unchanged; only the producer moved.
//!
//! # F2 tokio Handle capture
//!
//! Terminal grid/title/exit events are fired from alacritty's PTY
//! reader thread — a `std::thread::spawn`'d worker that has no
//! ambient tokio runtime. The `Sender::send` on a tokio
//! `broadcast::Sender` does not itself require a runtime context
//! (it's synchronous internally), so unlike Unit 1's companion
//! subscriber we do not need to dispatch via a captured `Handle`
//! here. We keep the handle in the struct anyway as a placeholder
//! for future paths that *do* need a runtime (e.g. forwarding to a
//! tokio mpsc with bounded backpressure or awaiting an async
//! permit). Cheap; documents the F2 pattern explicitly so the next
//! agent sees how to extend.
//!
//! # Lifecycle ownership
//!
//! Register once during daemon startup, before any
//! `TerminalManager::create()` call lands. The sink is process-wide
//! and lives for the daemon's lifetime — the broadcast channel
//! outlives it (it's owned by `main.rs`).

use std::sync::OnceLock;

use k2_core::log_debug;
use k2_core::terminal::event_sink::TerminalEventSink;
use k2_core::terminal::grid_types::GridUpdate;
use tokio::sync::broadcast;

use crate::events::WireEvent;

/// Process-wide singleton handle to the registered terminal-event
/// broadcaster. Set by `register()`; read by `shared_sink()` when
/// a terminal-route handler needs to attach a sink at `create` time.
static SHARED_TX: OnceLock<broadcast::Sender<WireEvent>> = OnceLock::new();

/// Broadcasts terminal events as `WireEvent` frames named
/// `terminal:{title,bell,exit,grid}:<id>` so the renderer's existing
/// `listen('terminal:grid:<id>')` subscribers see the same events.
pub struct DaemonTerminalEventSink {
    tx: broadcast::Sender<WireEvent>,
    /// Captured at construction (must run inside the tokio runtime).
    /// Currently unused — kept as a placeholder for future async
    /// extension paths. See module docs.
    #[allow(dead_code)]
    handle: tokio::runtime::Handle,
}

impl DaemonTerminalEventSink {
    pub fn new(tx: broadcast::Sender<WireEvent>, handle: tokio::runtime::Handle) -> Self {
        Self { tx, handle }
    }
}

impl TerminalEventSink for DaemonTerminalEventSink {
    fn on_title(&self, terminal_id: &str, title: &str) {
        let _ = self.tx.send(WireEvent {
            event: format!("terminal:title:{}", terminal_id),
            payload: serde_json::Value::String(title.to_string()),
        });
    }

    fn on_bell(&self, terminal_id: &str) {
        let _ = self.tx.send(WireEvent {
            event: format!("terminal:bell:{}", terminal_id),
            payload: serde_json::Value::Null,
        });
    }

    fn on_exit(&self, terminal_id: &str, exit_code: i32) {
        let _ = self.tx.send(WireEvent {
            event: format!("terminal:exit:{}", terminal_id),
            payload: serde_json::json!({ "exitCode": exit_code }),
        });
    }

    fn on_grid_update(&self, terminal_id: &str, update: &GridUpdate) {
        // GridUpdate already implements Serialize via core; one
        // serialize per event is unavoidable since the broadcast
        // payload is serde_json::Value.
        let payload = match serde_json::to_value(update) {
            Ok(v) => v,
            Err(e) => {
                log_debug!(
                    "[daemon/terminal-sink] serialize GridUpdate failed for {terminal_id}: {e}"
                );
                return;
            }
        };
        let _ = self.tx.send(WireEvent {
            event: format!("terminal:grid:{}", terminal_id),
            payload,
        });
    }
}

/// Register the shared broadcaster so terminal-route handlers can
/// build per-`create()` sink instances on demand. Idempotent — a
/// repeated call is a no-op (the daemon only has one event channel
/// per boot). Must run inside the tokio runtime so we can capture
/// a `Handle`.
pub fn register(event_tx: broadcast::Sender<WireEvent>) {
    let _ = tokio::runtime::Handle::current(); // assert we're in a runtime
    let _ = SHARED_TX.set(event_tx);
    log_debug!("[daemon/terminal-sink] registered shared broadcaster");
}

/// Build a fresh `DaemonTerminalEventSink` arc for use as the
/// `event_sink` argument to `TerminalManager::create`. Returns
/// `None` if `register()` hasn't run yet — terminal routes return a
/// 500 in that case rather than silently dropping events.
pub fn build_sink() -> Option<std::sync::Arc<dyn TerminalEventSink>> {
    let tx = SHARED_TX.get()?.clone();
    // Capture a Handle here. If build_sink() is called from a
    // tokio task this is the runtime; if from a worker thread that
    // already has SHARED_TX populated, we still need a runtime
    // reference — fall back to a no-op handle in that case is not
    // possible (Handle::current() panics outside a runtime). All
    // current callers (terminal routes) run inside tokio so this
    // is safe.
    let handle = tokio::runtime::Handle::current();
    Some(std::sync::Arc::new(DaemonTerminalEventSink::new(tx, handle)))
}
