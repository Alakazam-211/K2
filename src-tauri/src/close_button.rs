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
//! Linux: by design (K2, 2026-11-11). The daemon is a systemd service and
//! runs headless, so closing a window must never stop it.
//! Windows: an open question. Should setting OFF stop the daemon there,
//! as on macOS? Left as is until that's decided.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_on_hides_the_window_on_every_os() {
        assert_eq!(action(true, true), CloseAction::Hide);
        assert_eq!(action(true, false), CloseAction::Hide);
    }

    #[test]
    fn setting_off_on_macos_stops_the_daemon_then_quits() {
        assert_eq!(action(false, true), CloseAction::Quit { stop_daemon: true });
    }

    #[test]
    fn setting_off_off_macos_quits_and_leaves_the_daemon() {
        assert_eq!(action(false, false), CloseAction::Quit { stop_daemon: false });
    }

    #[test]
    fn a_settings_file_without_the_field_hides_the_window() {
        // A settings file without the field: it defaults to on, so the
        // close button keeps the server running on every OS.
        let keep = k2_client::app_settings_view::parse("{}").keep_daemon_on_quit;
        assert_eq!(action(keep, true), CloseAction::Hide);
        assert_eq!(action(keep, false), CloseAction::Hide);
    }
}
