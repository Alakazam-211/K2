//! Terminal event sink — decouples the alacritty backend from the Tauri
//! event system.
//!
//! Before this trait existed, `AlacrittyTerminalInstance` held a
//! `tauri::AppHandle` and called `app_handle.emit(...)` directly on every
//! title/bell/exit/grid event. That tied the whole terminal module to Tauri
//! and made it impossible to move into k2so-core (shared by the Tauri app
//! and the k2so-daemon).
//!
//! The trait is intentionally tiny and synchronous — emissions are
//! fire-and-forget from the terminal's perspective. Implementations
//! swallow their own delivery failures (dead WS clients, broken pipes,
//! backgrounded Tauri windows) rather than surfacing them.

use crate::terminal::grid_types::GridUpdate;

/// What the terminal module wants the outside world to know about.
///
/// Every variant carries the originating terminal's ID so downstream
/// consumers can route to the right UI surface (Tauri window, companion
/// WebSocket client, log file).
pub trait TerminalEventSink: Send + Sync {
    /// OSC 0/2 title escape sequence landed. Update the tab / window title.
    fn on_title(&self, terminal_id: &str, title: &str);

    /// BEL character (0x07) received. Play an audible bell or flash the tab.
    fn on_bell(&self, terminal_id: &str);

    /// Child process exited. `exit_code = -1` means unknown (signal-killed
    /// with no waitable status).
    fn on_exit(&self, terminal_id: &str, exit_code: i32);

    /// Grid state changed and the emission loop snapshotted it. Fired at
    /// up to ~60Hz from the emission thread. The update carries its own
    /// `seqno` stamp for downstream coalescing and change-detection.
    fn on_grid_update(&self, terminal_id: &str, update: &GridUpdate);
}
