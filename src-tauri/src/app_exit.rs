//! How the app process ends, once it has decided to quit (the close button
//! with keep-daemon-on-quit off, and `RunEvent::Exit` after Cmd+Q or Quit).
//!
//! Normally `_exit(0)` on unix: it skips C++ static destructors, because
//! ggml's Metal cleanup in `__cxa_finalize_ranges` races macOS's Metal
//! device teardown and aborts (SIGABRT). In relaunch mode (the frontend set
//! `RELAUNCH_MODE` before `process::relaunch`) a normal `exit(0)` instead,
//! so the process plugin can start the new process before this one ends.
//! Not unix: always a normal exit.

/// How to end the process, decided by [`how`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    /// `std::process::exit(0)`.
    Normal,
    /// `libc::_exit(0)`: no destructors.
    Immediate,
}

/// `relaunch` is `RELAUNCH_MODE`; `unix` is `cfg!(unix)` at the call site
/// (a parameter so every branch is testable on any OS).
pub(crate) fn how(relaunch: bool, unix: bool) -> Exit {
    if relaunch || !unix {
        Exit::Normal
    } else {
        Exit::Immediate
    }
}

/// End the process now, the way [`how`] says.
pub(crate) fn now(relaunch: bool) -> ! {
    match how(relaunch, cfg!(unix)) {
        Exit::Normal => {
            if relaunch {
                log_debug!("[shutdown] Relaunch mode — using normal exit");
            }
            std::process::exit(0)
        }
        Exit::Immediate => {
            #[cfg(unix)]
            unsafe {
                libc::_exit(0)
            }
            #[cfg(not(unix))]
            unreachable!("Immediate is unix only")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_skips_destructors_unless_relaunching() {
        assert_eq!(how(false, true), Exit::Immediate);
        assert_eq!(how(true, true), Exit::Normal, "relaunch lets the new process start");
    }

    #[test]
    fn other_systems_always_exit_normally() {
        assert_eq!(how(false, false), Exit::Normal);
        assert_eq!(how(true, false), Exit::Normal);
    }
}
