//! `k2-deskd` ("K2 Screen"): the remote desktop helper
//! (`prd-remote-desktop-tab-v1` §3.2, slices D3 and D4).
//!
//! - **macOS:** a background-only app nested in K2.app
//!   (`Contents/Helpers/K2 Screen.app`, `dev.k2.app.screen`), started by its
//!   own LaunchAgent (`dev.k2.deskd`) so launchd makes it its own
//!   responsible process. It is the only K2 process that holds Screen
//!   Recording and Accessibility. It listens on `~/.k2/run/deskd.sock`
//!   (0600) and serves only K2's signed daemon (a peer code check: ordinary
//!   hygiene; same-user code is accepted for v1).
//! - **Linux:** the daemon's child over pipes; X11 input through XTEST.
//!
//! It takes no arguments that change what it does (`--version` and
//! `--tcc-probe` only print), and never writes frames to disk.
//!
//! Capture and encode (D5, D8b) plug into [`server::Deskd::send_frame`].

pub mod hold;
pub mod inject;
pub mod server;

#[cfg(unix)]
pub mod signals;
#[cfg(unix)]
pub mod sock;

#[cfg(target_os = "macos")]
pub mod mac;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub mod notify;
#[cfg(target_os = "linux")]
pub mod x11;

/// `$HOME`, else the passwd entry's home.
pub fn home_dir() -> std::path::PathBuf {
    if let Some(h) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        return h.into();
    }
    #[cfg(unix)]
    // SAFETY: getpwuid's static result is read before any other call.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_dir.is_null() {
            return std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned().into();
        }
    }
    std::path::PathBuf::from(".")
}
