//! What the main window's close button (the red dot on macOS) does.
//!
//! The "Keep server running when the window is closed" setting
//! (`keepDaemonOnQuit` in `~/.k2/settings.json`, default on) decides:
//!
//!   ON  -> hide the window. The app process and the daemon stay.
//!   OFF -> quit the app. On macOS, stop the daemon first (the launchd
//!          job would respawn it otherwise). Linux and Windows quit the
//!          app only; the daemon is left alone there.
//!
//! Cmd+Q, the tray's Quit and the menu's Quit are not this path: they
//! land in `RunEvent::ExitRequested` in lib.rs, which never stops the
//! daemon. Pure so it can be tested without a window; the event closure
//! in lib.rs carries out the action.

/// The close button's action, decided by [`action`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseAction {
    /// Prevent the close and hide the window. Nothing stops.
    Hide,
    /// Let the app quit. `stop_daemon`: ask the local daemon to stop first.
    Quit { stop_daemon: bool },
}

/// `keep_daemon_on_quit` is the setting; `macos` is
/// `cfg!(target_os = "macos")` at the call site (a parameter so every
/// branch is testable on any OS).
pub(crate) fn action(keep_daemon_on_quit: bool, macos: bool) -> CloseAction {
    if keep_daemon_on_quit {
        return CloseAction::Hide;
    }
    CloseAction::Quit { stop_daemon: macos }
}
