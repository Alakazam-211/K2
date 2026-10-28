//! Windows: launchd's `KeepAlive` for the daemon.
//!
//! macOS's launchd agent starts the daemon again when it crashes or is
//! asked to restart (an update, `/cli/daemon/restart`), and lets it go
//! when the person quits. Windows has no such agent for a per-user
//! program, so `k2-daemon --keep-alive` is one: the logon Run value and
//! the app start it, and it runs the daemon as its child, reading why
//! the daemon ended from its exit code:
//!
//! - [`EXIT_QUIT`] (0): a stop (the person quit; `/cli/daemon/stop`, a
//!   stop intent): stop supervising.
//! - [`EXIT_ALREADY_RUNNING`]: another daemon holds this home's lock:
//!   check again later, so this supervisor takes over if that one ends.
//! - [`EXIT_RESTART`]: a restart (update, `/cli/daemon/restart`): start it
//!   again at once.
//! - anything else (a crash, a kill): start it again, backing off while it
//!   keeps failing fast.
//!
//! One supervisor per user and K2 home (a named mutex). An installer's
//! `taskkill /IM k2-daemon.exe /T /F` ends the supervisor with the daemon.

#![cfg_attr(not(windows), allow(dead_code))]

use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

/// The daemon argument that runs this supervisor instead of the daemon.
pub const ARG: &str = "--keep-alive";

/// The daemon's exit codes the supervisor reads (Windows).
pub const EXIT_QUIT: i32 = 0;
pub const EXIT_RESTART: i32 = 75;
pub const EXIT_ALREADY_RUNNING: i32 = 76;

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A run this long counts as healthy: the next crash starts from the
/// shortest wait again.
const HEALTHY_RUN: Duration = Duration::from_secs(60);
/// How often to look again while another daemon holds the lock.
const OTHER_DAEMON_RECHECK: Duration = Duration::from_secs(15);

/// What the supervisor does after the daemon ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    Stop,
    /// Start the daemon again after this wait.
    Start(Duration),
}

/// The decision, pure: `code` is the daemon's exit code (`None`: no code,
/// or it could not start), `ran` how long it ran, `backoff` the wait the
/// next failure will use (updated here).
pub fn next(code: Option<i32>, ran: Duration, backoff: &mut Duration) -> Next {
    match code {
        Some(EXIT_QUIT) => Next::Stop,
        Some(EXIT_ALREADY_RUNNING) => Next::Start(OTHER_DAEMON_RECHECK),
        Some(EXIT_RESTART) => {
            *backoff = BACKOFF_MIN;
            Next::Start(Duration::from_millis(200))
        }
        _ => {
            if ran >= HEALTHY_RUN {
                *backoff = BACKOFF_MIN;
            }
            let wait = *backoff;
            *backoff = (*backoff * 2).min(BACKOFF_MAX);
            Next::Start(wait)
        }
    }
}

/// Run the supervisor until the daemon quits. Returns the process exit
/// code (0; another supervisor already running is also 0).
#[cfg(windows)]
pub fn run() -> i32 {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let Some(_guard) = single_instance() else {
        k2_core::log_debug!("[keep-alive] another supervisor is running for this K2 home; leaving it to that one");
        return 0;
    };
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            k2_core::log_debug!("[keep-alive] FATAL: own path: {e}");
            return 1;
        }
    };
    let mut backoff = BACKOFF_MIN;
    loop {
        let started = Instant::now();
        let mut cmd = Command::new(&exe);
        if let Some(dir) = exe.parent() {
            cmd.current_dir(dir);
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(CREATE_NO_WINDOW);
        let code = match cmd.status() {
            Ok(st) => st.code(),
            Err(e) => {
                k2_core::log_debug!("[keep-alive] could not start {}: {e}", exe.display());
                None
            }
        };
        match next(code, started.elapsed(), &mut backoff) {
            Next::Stop => {
                k2_core::log_debug!("[keep-alive] the daemon quit; supervisor done");
                return 0;
            }
            Next::Start(wait) => {
                k2_core::log_debug!("[keep-alive] daemon ended (code {code:?}); starting it again in {wait:?}");
                std::thread::sleep(wait);
            }
        }
    }
}

/// A named mutex for this user and K2 home; `None` when another
/// supervisor holds it. Released when the process ends.
#[cfg(windows)]
fn single_instance() -> Option<Mutex> {
    use std::hash::{Hash, Hasher};
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let mut h = std::collections::hash_map::DefaultHasher::new();
    k2_core::paths::k2_home().to_string_lossy().to_lowercase().hash(&mut h);
    let name = format!(r"Local\K2DaemonKeepAlive-{:016x}", h.finish());
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: a NUL-terminated name; the handle is closed in Drop.
    unsafe {
        let m = CreateMutexW(std::ptr::null(), 0, wide.as_ptr());
        if m.is_null() {
            return None;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            windows_sys::Win32::Foundation::CloseHandle(m);
            return None;
        }
        Some(Mutex(m))
    }
}

#[cfg(windows)]
struct Mutex(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for Mutex {
    fn drop(&mut self) {
        // SAFETY: the handle CreateMutexW returned, closed once.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_stops_restart_and_crash_start_again_with_backoff() {
        let mut b = BACKOFF_MIN;
        assert_eq!(next(Some(EXIT_QUIT), Duration::from_secs(5), &mut b), Next::Stop);
        assert_eq!(next(Some(EXIT_ALREADY_RUNNING), Duration::ZERO, &mut b), Next::Start(OTHER_DAEMON_RECHECK));
        // Crashes in a row back off 1, 2, 4 … capped at 60 s.
        let waits: Vec<Duration> = (0..8).map(|_| match next(Some(101), Duration::from_secs(1), &mut b) {
            Next::Start(w) => w,
            Next::Stop => panic!("a crash never stops supervision"),
        }).collect();
        assert_eq!(waits[..4], [1, 2, 4, 8].map(Duration::from_secs));
        assert_eq!(*waits.last().unwrap(), BACKOFF_MAX);
        // A long healthy run resets it; so does a requested restart.
        assert_eq!(next(Some(1), HEALTHY_RUN, &mut b), Next::Start(BACKOFF_MIN));
        let _ = next(None, Duration::ZERO, &mut b);
        assert_eq!(next(Some(EXIT_RESTART), Duration::ZERO, &mut b), Next::Start(Duration::from_millis(200)));
        assert_eq!(b, BACKOFF_MIN);
    }
}
