// Terminal backend — Alacritty-based terminal emulation with grid-based IPC.

mod alacritty_backend;
pub mod event_sink;
pub mod grid_types;
pub mod reflow;
mod font_renderer;
// 0.34.0 Session Stream PTY reader (Phase 2) was retired in 0.39.0
// alongside the Kessel-T0 renderer. The Alacritty_v2 `daemon_pty`
// module below is now the only daemon-side terminal type.

// Alacritty_v2 daemon-hosted PTY + Term module (Phase A1 of the
// .k2so/prds/alacritty-v2.md plan). Minimal: no LineMux, no byte
// broadcast, no ring, no APC. Uses alacritty's built-in
// EventLoop::spawn() rather than a custom reader.
pub mod daemon_pty;
// PATH enrichment for daemon-spawned children (issue #15). Computes
// an augmented PATH (login-shell PATH ++ known install dirs ++
// inherited launchd PATH) so agent CLIs in ~/.local/bin, homebrew,
// nvm shims, etc. resolve by bare name instead of ENOENT.
pub mod login_path;
// Alacritty_v2 grid snapshot + delta wire types + serializers
// (Phase A2). Shared between the daemon's WS endpoint (A3) and
// the Tauri thin client (A5). Generic over `EventListener` so
// it's usable with any Term variant.
pub mod grid_snapshot;
// Grow-then-shrink settle watcher (2026-04-22). Every Session Stream
// spawn opens the PTY at an artificially large rows value; this
// module owns the "has the initial paint settled?" decision that
// drives the follow-up SIGWINCH shrink.
pub mod grow_settle;
// `bitmap_renderer` was deleted in 0.33.x. The DOM/grid broadcast
// protocol that shipped in 0.32.13 retired bitmap rendering; the
// module's 414 lines were dead code for a full release.

pub use alacritty_backend::TerminalManager;
pub use event_sink::TerminalEventSink;

pub use daemon_pty::{
    AlacEvent, DaemonEventListener, DaemonPtyConfig, DaemonPtySession,
    LabelSource, SCROLLBACK_CAP,
};

/// Re-export alacritty's `Dimensions` trait so daemon + Tauri
/// consumers can call `term.columns()` / `term.screen_lines()`
/// without a direct `alacritty_terminal` dependency.
pub use alacritty_terminal::grid::Dimensions;

pub use grid_snapshot::{
    build_emit, cell_to_run, drain_damage, encode_row_runs, resolve_color,
    snapshot_term, CellRun, CursorSnapshot, DamagedRow, EmitDecision,
    EmitState, TermGridDelta, TermGridSnapshot,
};

use parking_lot::Mutex;
use std::sync::{Arc, OnceLock};

/// Process-wide singleton TerminalManager. Mirrors the `db::shared()`
/// pattern so any module — src-tauri's AppState, core's companion,
/// the future daemon — gets the same handle and therefore the same
/// live HashMap of terminal instances.
static SHARED: OnceLock<Arc<Mutex<TerminalManager>>> = OnceLock::new();

/// Return (and lazy-initialize) the shared TerminalManager. Callers
/// clone the Arc freely; the Mutex serializes writes to the inner
/// HashMap. Previous design had AppState own a `Mutex<TerminalManager>`
/// directly, which meant agent_hooks and companion needed AppState
/// access to spawn or inspect terminals — a strong coupling that
/// blocked moving those modules into k2so-core. Now everyone shares
/// this singleton.
pub fn shared() -> Arc<Mutex<TerminalManager>> {
    SHARED
        .get_or_init(|| Arc::new(Mutex::new(TerminalManager::new())))
        .clone()
}

// ── Shared utilities ───────────────────────────────────────────────────────

/// Ignore SIGPIPE at process startup so writing to a dead PTY returns EPIPE
/// instead of killing the entire Tauri process.
///
/// This is intentionally global: child processes spawned via Command::new()
/// inherit their own signal mask, so git/external tools are unaffected.
/// DB writes target local files (no pipe), and reqwest uses MSG_NOSIGNAL.
/// Zed uses the same approach.
#[cfg(unix)]
pub fn ignore_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

/// Expand tilde in a path and ensure the directory exists.
pub fn resolve_cwd(cwd: &str) -> String {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
    let resolved = if cwd == "~" {
        home.to_string_lossy().to_string()
    } else if cwd.starts_with("~/") {
        cwd.replacen("~", &home.to_string_lossy(), 1)
    } else {
        cwd.to_string()
    };

    if std::path::Path::new(&resolved).exists() {
        resolved
    } else {
        log_debug!("[terminal] WARNING: CWD '{}' does not exist, falling back to home", resolved);
        home.to_string_lossy().to_string()
    }
}

/// Detect the user's default shell.
pub fn detect_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| std::path::Path::new(s).exists())
        .unwrap_or_else(|| {
            for sh in &["/bin/zsh", "/bin/bash", "/bin/sh"] {
                if std::path::Path::new(sh).exists() {
                    return sh.to_string();
                }
            }
            "/bin/sh".to_string()
        })
}
