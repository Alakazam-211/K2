//! Linux power layer. Thin, and no prompts (D13: no polkit rule).
//!
//! - Keep awake: `systemd-inhibit --what=sleep:handle-lid-switch
//!   --mode=block cat`, fed by a pipe the daemon holds. Closing the pipe
//!   (drop, or the daemon dying) ends `cat` and releases the lock. In a
//!   seat session logind allows it with no prompt. A headless lingering
//!   user may be refused; then K2 tries `sleep` alone, and otherwise
//!   reports `limited (no session)`.
//! - Wake: one `timerfd_create(CLOCK_BOOTTIME_ALARM)` owned by the
//!   daemon. Needs `CAP_WAKE_ALARM`, which the Arch package's install
//!   hook sets on `/usr/bin/k2-daemon` (W5). Without it, wake reports
//!   unavailable with the one `setcap` line to run.

use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Child, Command, Stdio};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;

use super::{PowerOs, PowerSource, WakeSupport};

struct Inhibitor {
    child: Child,
}

impl Drop for Inhibitor {
    fn drop(&mut self) {
        drop(self.child.stdin.take()); // EOF → `cat` exits → lock released
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_inhibitor(what: &str, why: &str) -> Result<Inhibitor, String> {
    let mut child = Command::new("systemd-inhibit")
        .args([
            &format!("--what={what}"),
            "--who=K2",
            &format!("--why={why}"),
            "--mode=block",
            "cat",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("systemd-inhibit not available: {e}"))?;
    // A refusal (polkit, no session) exits at once.
    std::thread::sleep(std::time::Duration::from_millis(200));
    match child.try_wait() {
        Ok(None) => Ok(Inhibitor { child }),
        Ok(Some(_)) => {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            Err(format!("systemd-inhibit --what={what} refused: {}", err.trim()))
        }
        Err(e) => Err(format!("wait systemd-inhibit: {e}")),
    }
}

pub struct LinuxPowerOs {
    alarm: Result<Mutex<OwnedFd>, String>,
}

impl LinuxPowerOs {
    pub fn new() -> Self {
        let fd = unsafe {
            libc::timerfd_create(libc::CLOCK_BOOTTIME_ALARM, libc::TFD_CLOEXEC | libc::TFD_NONBLOCK)
        };
        let alarm = if fd < 0 {
            let err = std::io::Error::last_os_error();
            let exe = std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "/usr/bin/k2-daemon".into());
            Err(if err.raw_os_error() == Some(libc::EPERM) {
                format!(
                    "no CAP_WAKE_ALARM on the daemon: run `sudo setcap cap_wake_alarm+ep {exe}` then restart it"
                )
            } else {
                format!("wake alarm timer unavailable: {err}")
            })
        } else {
            Ok(Mutex::new(unsafe { OwnedFd::from_raw_fd(fd) }))
        };
        Self { alarm }
    }

    fn arm(&self, secs_from_now: i64) -> Result<(), String> {
        let fd = self.alarm.as_ref().map_err(Clone::clone)?;
        let fd = fd.lock();
        let spec = libc::itimerspec {
            it_interval: libc::timespec { tv_sec: 0, tv_nsec: 0 },
            it_value: libc::timespec { tv_sec: secs_from_now.max(0) as libc::time_t, tv_nsec: 0 },
        };
        let rc = unsafe { libc::timerfd_settime(fd.as_raw_fd(), 0, &spec, std::ptr::null_mut()) };
        if rc != 0 {
            return Err(format!("timerfd_settime: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

impl PowerOs for LinuxPowerOs {
    fn hold_awake(&self, reason: &str) -> Result<Box<dyn Send>, String> {
        match spawn_inhibitor("sleep:handle-lid-switch", reason) {
            Ok(i) => Ok(Box::new(i)),
            Err(first) => match spawn_inhibitor("sleep", reason) {
                Ok(i) => {
                    k2_core::log_debug!("[power] lid lock refused, holding sleep only: {first}");
                    Ok(Box::new(i))
                }
                Err(second) => Err(format!("limited (no session): {second}")),
            },
        }
    }

    fn wake_support(&self) -> WakeSupport {
        match &self.alarm {
            Ok(_) => WakeSupport::Ready,
            Err(why) => WakeSupport::Unavailable(why.clone()),
        }
    }

    fn set_wake(&self, at: DateTime<Utc>) -> Result<(), String> {
        // Relative on the boot-time alarm clock, which keeps counting
        // through suspend. A zero value would disarm, so at least 1 s.
        self.arm((at - Utc::now()).num_seconds().max(1))
    }

    fn clear_wake(&self) -> Result<(), String> {
        self.arm(0)
    }

    fn power_source(&self) -> PowerSource {
        let mut entries = Vec::new();
        if let Ok(dir) = std::fs::read_dir("/sys/class/power_supply") {
            for e in dir.flatten() {
                let p = e.path();
                let read = |f: &str| std::fs::read_to_string(p.join(f)).ok().map(|s| s.trim().to_string());
                let kind = read("type").unwrap_or_default();
                let online = read("online").and_then(|s| s.parse().ok());
                let cap = read("capacity").and_then(|s| s.parse().ok());
                entries.push((kind, online, cap));
            }
        }
        super::parse::linux_supplies(&entries)
    }
}
