//! `dev.k2.power-helper` — the macOS root door for waking the Mac and
//! (S6) keeping it awake with the lid closed.
//!
//! Heartbeat S2 (D11, `research-heartbeat-wake-privilege-v1.md` §5.1):
//! the same pattern as `k2-mail-helper` (a tiny argv-allowlisted root
//! binary the daemon runs with `sudo -n`), installed by ONE
//! `osascript … with administrator privileges` dialog the first time
//! someone turns on "Wake this computer for heartbeats". No resident
//! root process, no `SMAppService`, no `.pkg`.
//!
//! - Binary: `/Library/PrivilegedHelperTools/dev.k2.power-helper`,
//!   `root:wheel 0755` (a copy of `k2-power-helper` from the app bundle;
//!   a path inside the user-writable app would let any process swap it).
//! - Sudoers: `/etc/sudoers.d/dev-k2-power`, `0440`, one line letting the
//!   console user run exactly that path without a password.
//!
//! Verbs (exact argv; anything else exits non-zero before any change):
//! - `version` — prints [`PROTOCOL_VERSION`].
//! - `status` — prints JSON: our scheduled wake events and whether sleep
//!   is disabled.
//! - `wake-set <unix-seconds>` — cancels every wake event this helper
//!   scheduled (`IOPMCopyScheduledPowerEvents`, owner
//!   [`WAKE_OWNER`]) and schedules one `IOPMSchedulePowerEvent` wake.
//!   The time is clamped to now+30 s … now+31 days.
//! - `wake-clear` — cancels our wake events.
//! - `awake-hold --pid <pid> --max <secs>` — S6: `pmset -a disablesleep 1`
//!   plus a detached root watcher that sets it back to 0 when `pid`
//!   exits or `max` (≤ 4 h) passes. The daemon renews (calls it again)
//!   every minute with a 3-minute `max`; each call starts a new
//!   generation and the older watcher steps aside. A daemon that dies or
//!   stops renewing gets sleep back within `max` ([`watch_loop`]).
//! - `awake-release` — S6: `pmset -a disablesleep 0` now.
//! - `awake-watch --pid <pid> --max <secs> --gen <n>` — the watcher the
//!   hold forks. Internal.
//!
//! The helper runs no agent code and reads no K2 data. Tests never exec
//! it, never run sudo, and never touch powerd: they cover the argv
//! allowlist, the clamp, and the install script text.

#![allow(dead_code)]

pub const PROTOCOL_VERSION: u32 = 1;
pub const HELPER_PATH: &str = "/Library/PrivilegedHelperTools/dev.k2.power-helper";
pub const SUDOERS_PATH: &str = "/etc/sudoers.d/dev-k2-power";
pub const SUDO_PATH: &str = "/usr/bin/sudo";
pub const PMSET_PATH: &str = "/usr/bin/pmset";
/// Owner string on our powerd wake events (`pmset -g sched` shows it).
pub const WAKE_OWNER: &str = "dev.k2.heartbeat";
/// Root-owned generation file for the S6 lid-closed hold.
pub const HOLD_STATE_PATH: &str = "/var/run/dev.k2.power-helper.hold";
/// Name of the helper binary next to `k2-daemon` in the app bundle.
pub const BUNDLED_HELPER_NAME: &str = "k2-power-helper";

pub const MIN_LEAD_SECS: i64 = 30;
pub const MAX_LEAD_SECS: i64 = 31 * 24 * 3600;
pub const MAX_HOLD_SECS: u64 = 4 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperCommand {
    Version,
    Status,
    WakeSet { unix: i64 },
    WakeClear,
    AwakeHold { pid: u32, max_secs: u64 },
    AwakeRelease,
    AwakeWatch { pid: u32, max_secs: u64, gen: u64 },
}

fn parse_num<T: std::str::FromStr>(s: &str, what: &str) -> Result<T, String> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("power helper: {what} must be a plain number"));
    }
    s.parse().map_err(|_| format!("power helper: {what} out of range"))
}

/// Exact-argv allowlist. Unknown or extra argv is an error.
pub fn parse_argv(args: &[&str]) -> Result<HelperCommand, String> {
    match args {
        ["version"] => Ok(HelperCommand::Version),
        ["status"] => Ok(HelperCommand::Status),
        ["wake-set", ts] => Ok(HelperCommand::WakeSet { unix: parse_num(ts, "wake time")? }),
        ["wake-clear"] => Ok(HelperCommand::WakeClear),
        ["awake-hold", "--pid", pid, "--max", max] => {
            let max_secs: u64 = parse_num(max, "max")?;
            if max_secs == 0 || max_secs > MAX_HOLD_SECS {
                return Err(format!("power helper: max must be 1..={MAX_HOLD_SECS}"));
            }
            Ok(HelperCommand::AwakeHold { pid: parse_num(pid, "pid")?, max_secs })
        }
        ["awake-release"] => Ok(HelperCommand::AwakeRelease),
        ["awake-watch", "--pid", pid, "--max", max, "--gen", gen] => {
            let max_secs: u64 = parse_num(max, "max")?;
            if max_secs == 0 || max_secs > MAX_HOLD_SECS {
                return Err(format!("power helper: max must be 1..={MAX_HOLD_SECS}"));
            }
            Ok(HelperCommand::AwakeWatch {
                pid: parse_num(pid, "pid")?,
                max_secs,
                gen: parse_num(gen, "gen")?,
            })
        }
        _ => Err(format!("power helper: unknown command {args:?}")),
    }
}

/// Clamp a requested wake time to now+30 s … now+31 days.
pub fn clamp_wake(unix: i64, now_unix: i64) -> i64 {
    unix.clamp(now_unix + MIN_LEAD_SECS, now_unix + MAX_LEAD_SECS)
}

/// A console username we are willing to write into sudoers.
pub fn valid_username(user: &str) -> bool {
    !user.is_empty()
        && user.len() <= 64
        && user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
        && !user.starts_with('-')
}

/// The shell script the one admin dialog runs (D11). Pure, so the exact
/// text is tested. `src` is the bundled helper next to `k2-daemon`.
pub fn install_script(user: &str, src: &str) -> Result<String, String> {
    if !valid_username(user) {
        return Err(format!("refusing to write sudoers for username {user:?}"));
    }
    if src.is_empty() || src.contains('\'') || src.contains('\n') || !src.starts_with('/') {
        return Err(format!("refusing helper source path {src:?}"));
    }
    Ok(format!(
        "set -e; \
         /bin/mkdir -p /Library/PrivilegedHelperTools; \
         /usr/bin/install -o root -g wheel -m 0755 '{src}' {HELPER_PATH}; \
         tmp=$(/usr/bin/mktemp /tmp/dev-k2-power.XXXXXX); \
         /usr/bin/printf '%s\\n' '{user} ALL=(root) NOPASSWD: {HELPER_PATH}' 'Defaults:{user} !requiretty' > \"$tmp\"; \
         /usr/sbin/visudo -cf \"$tmp\"; \
         /usr/bin/install -o root -g wheel -m 0440 \"$tmp\" {SUDOERS_PATH}; \
         /bin/rm -f \"$tmp\""
    ))
}

/// The AppleScript for the dialog: `do shell script … with
/// administrator privileges` and K2's own prompt text.
pub fn install_applescript(script: &str) -> String {
    let escaped = script.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "do shell script \"{escaped}\" with prompt \"K2 needs to install a small helper so it can wake this Mac for heartbeats and keep it awake with the lid closed.\" with administrator privileges"
    )
}

/// Is the helper installed (binary and sudoers rule both present)?
pub fn installed() -> bool {
    std::path::Path::new(HELPER_PATH).exists() && std::path::Path::new(SUDOERS_PATH).exists()
}

/// Run one helper verb with `sudo -n` (fails fast when the rule is
/// missing). Returns stdout.
pub fn call(args: &[&str]) -> Result<String, String> {
    let mut full = vec!["-n", HELPER_PATH];
    full.extend_from_slice(args);
    let out = std::process::Command::new(SUDO_PATH)
        .args(&full)
        .output()
        .map_err(|e| format!("run sudo {HELPER_PATH}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "power helper {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run the parsed command as root. macOS only.
#[cfg(target_os = "macos")]
pub fn execute(cmd: HelperCommand) -> Result<(), String> {
    match cmd {
        HelperCommand::Version => {
            println!("{PROTOCOL_VERSION}");
            Ok(())
        }
        HelperCommand::Status => {
            let events = mac::our_wake_events()?;
            let v = serde_json::json!({
                "version": PROTOCOL_VERSION,
                "wakeEvents": events,
                "holdActive": std::path::Path::new(HOLD_STATE_PATH).exists(),
            });
            println!("{v}");
            Ok(())
        }
        HelperCommand::WakeSet { unix } => {
            let now = chrono::Utc::now().timestamp();
            mac::cancel_our_wake_events()?;
            mac::schedule_wake(clamp_wake(unix, now))
        }
        HelperCommand::WakeClear => mac::cancel_our_wake_events(),
        HelperCommand::AwakeHold { pid, max_secs } => mac::awake_hold(pid, max_secs),
        HelperCommand::AwakeRelease => mac::awake_release(),
        HelperCommand::AwakeWatch { pid, max_secs, gen } => mac::awake_watch(pid, max_secs, gen),
    }
}

/// What the lid-hold watcher reads and does. The real one (macOS) reads
/// the root-owned generation file, checks the daemon pid, sleeps, and
/// runs `pmset -a disablesleep 0`; tests drive a mock with a fake clock.
pub trait WatchEnv {
    /// The current hold generation; `None` = released.
    fn current_gen(&self) -> Option<u64>;
    fn pid_alive(&self, pid: u32) -> bool;
    /// Seconds since the watcher started.
    fn elapsed_secs(&self) -> u64;
    fn sleep_secs(&mut self, secs: u64);
    /// Put sleep back (`disablesleep 0`) and clear the hold.
    fn restore_sleep(&mut self) -> Result<(), String>;
}

/// How often the watcher looks.
pub const WATCH_POLL_SECS: u64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchOutcome {
    /// Released, or a renewal started a newer generation: nothing to do.
    SteppedAside,
    /// The daemon died: sleep restored.
    RestoredDaemonGone,
    /// No renewal before `max`: sleep restored.
    RestoredNoRenewal,
}

/// The watchdog. Restores sleep when the daemon pid exits or `max_secs`
/// pass without a renewal; steps aside when its generation is replaced
/// (renewed) or released.
pub fn watch_loop(env: &mut dyn WatchEnv, pid: u32, max_secs: u64, gen: u64) -> Result<WatchOutcome, String> {
    loop {
        if env.current_gen() != Some(gen) {
            return Ok(WatchOutcome::SteppedAside);
        }
        let outcome = if !env.pid_alive(pid) {
            Some(WatchOutcome::RestoredDaemonGone)
        } else if env.elapsed_secs() >= max_secs {
            Some(WatchOutcome::RestoredNoRenewal)
        } else {
            None
        };
        if let Some(o) = outcome {
            // Re-check: a renewal may have landed while we looked.
            if env.current_gen() != Some(gen) {
                return Ok(WatchOutcome::SteppedAside);
            }
            env.restore_sleep()?;
            return Ok(o);
        }
        env.sleep_secs(WATCH_POLL_SECS);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn execute(_cmd: HelperCommand) -> Result<(), String> {
    Err("power helper: macOS only".to_string())
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::ffi::{c_char, c_void, CStr, CString};

    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFDateRef = *const c_void;
    type CFArrayRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    /// Seconds between 1970-01-01 and CFAbsoluteTime's 2001-01-01.
    const CF_EPOCH_OFFSET: f64 = 978_307_200.0;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(alloc: *const c_void, s: *const c_char, enc: u32) -> CFStringRef;
        fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, len: isize, enc: u32) -> u8;
        fn CFDateCreate(alloc: *const c_void, at: f64) -> CFDateRef;
        fn CFDateGetAbsoluteTime(d: CFDateRef) -> f64;
        fn CFArrayGetCount(a: CFArrayRef) -> isize;
        fn CFArrayGetValueAtIndex(a: CFArrayRef, i: isize) -> *const c_void;
        fn CFDictionaryGetValue(d: CFDictionaryRef, key: *const c_void) -> *const c_void;
        fn CFRelease(cf: CFTypeRef);
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMSchedulePowerEvent(time: CFDateRef, id: CFStringRef, kind: CFStringRef) -> i32;
        fn IOPMCancelScheduledPowerEvent(time: CFDateRef, id: CFStringRef, kind: CFStringRef) -> i32;
        fn IOPMCopyScheduledPowerEvents() -> CFArrayRef;
    }

    /// An owned CFString, released on drop.
    struct CfStr(CFStringRef);
    impl CfStr {
        fn new(s: &str) -> Result<Self, String> {
            let c = CString::new(s).map_err(|_| "nul in CFString".to_string())?;
            let r = unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) };
            if r.is_null() {
                return Err(format!("CFStringCreateWithCString({s}) failed"));
            }
            Ok(Self(r))
        }
    }
    impl Drop for CfStr {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0) }
        }
    }

    fn cf_string_value(s: CFStringRef) -> Option<String> {
        if s.is_null() {
            return None;
        }
        let mut buf = [0 as c_char; 512];
        let ok = unsafe { CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, UTF8) };
        if ok == 0 {
            return None;
        }
        Some(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
    }

    /// Visit every scheduled power event owned by [`WAKE_OWNER`]:
    /// `(time CFDate, event type string, unix seconds)`.
    fn for_each_ours(mut f: impl FnMut(CFDateRef, &str, i64) -> Result<(), String>) -> Result<(), String> {
        let arr = unsafe { IOPMCopyScheduledPowerEvents() };
        if arr.is_null() {
            return Ok(()); // no events at all
        }
        let k_time = CfStr::new("time")?;
        let k_owner = CfStr::new("scheduledby")?;
        let k_type = CfStr::new("eventtype")?;
        let n = unsafe { CFArrayGetCount(arr) };
        let mut result = Ok(());
        for i in 0..n {
            let dict = unsafe { CFArrayGetValueAtIndex(arr, i) };
            if dict.is_null() {
                continue;
            }
            let owner = cf_string_value(unsafe { CFDictionaryGetValue(dict, k_owner.0) });
            if owner.as_deref() != Some(WAKE_OWNER) {
                continue;
            }
            let date = unsafe { CFDictionaryGetValue(dict, k_time.0) };
            let kind = cf_string_value(unsafe { CFDictionaryGetValue(dict, k_type.0) })
                .unwrap_or_else(|| "wake".to_string());
            if date.is_null() {
                continue;
            }
            let unix = (unsafe { CFDateGetAbsoluteTime(date) } + CF_EPOCH_OFFSET) as i64;
            if let Err(e) = f(date, &kind, unix) {
                result = Err(e);
                break;
            }
        }
        unsafe { CFRelease(arr) };
        result
    }

    pub(super) fn our_wake_events() -> Result<Vec<serde_json::Value>, String> {
        let mut out = Vec::new();
        for_each_ours(|_, kind, unix| {
            out.push(serde_json::json!({ "type": kind, "at": unix }));
            Ok(())
        })?;
        Ok(out)
    }

    pub(super) fn cancel_our_wake_events() -> Result<(), String> {
        let owner = CfStr::new(WAKE_OWNER)?;
        for_each_ours(|date, kind, unix| {
            let kind_s = CfStr::new(kind)?;
            let rc = unsafe { IOPMCancelScheduledPowerEvent(date, owner.0, kind_s.0) };
            if rc != 0 {
                return Err(format!("IOPMCancelScheduledPowerEvent({unix}) returned {rc:#x}"));
            }
            Ok(())
        })
    }

    pub(super) fn schedule_wake(unix: i64) -> Result<(), String> {
        let owner = CfStr::new(WAKE_OWNER)?;
        let kind = CfStr::new("wake")?;
        let date = unsafe { CFDateCreate(std::ptr::null(), unix as f64 - CF_EPOCH_OFFSET) };
        if date.is_null() {
            return Err("CFDateCreate failed".to_string());
        }
        let rc = unsafe { IOPMSchedulePowerEvent(date, owner.0, kind.0) };
        unsafe { CFRelease(date) };
        if rc != 0 {
            return Err(format!("IOPMSchedulePowerEvent({unix}) returned {rc:#x}"));
        }
        Ok(())
    }

    fn pmset_disablesleep(on: bool) -> Result<(), String> {
        let out = std::process::Command::new(PMSET_PATH)
            .args(["-a", "disablesleep", if on { "1" } else { "0" }])
            .output()
            .map_err(|e| format!("run pmset: {e}"))?;
        if !out.status.success() {
            return Err(format!("pmset disablesleep failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(())
    }

    fn pid_alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    fn read_gen() -> Option<u64> {
        std::fs::read_to_string(HOLD_STATE_PATH).ok()?.trim().parse().ok()
    }

    /// S6: disable sleep and fork a detached watcher that restores it.
    pub(super) fn awake_hold(pid: u32, max_secs: u64) -> Result<(), String> {
        if !pid_alive(pid) {
            return Err(format!("power helper: pid {pid} is not running"));
        }
        let gen = read_gen().unwrap_or(0).wrapping_add(1);
        std::fs::write(HOLD_STATE_PATH, gen.to_string())
            .map_err(|e| format!("write {HOLD_STATE_PATH}: {e}"))?;
        pmset_disablesleep(true)?;
        let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
        let mut cmd = std::process::Command::new(exe);
        cmd.args([
            "awake-watch",
            "--pid",
            &pid.to_string(),
            "--max",
            &max_secs.to_string(),
            "--gen",
            &gen.to_string(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        if let Err(e) = cmd.spawn() {
            // No watcher = no safety net: undo at once.
            let _ = pmset_disablesleep(false);
            let _ = std::fs::remove_file(HOLD_STATE_PATH);
            return Err(format!("spawn watcher: {e}"));
        }
        Ok(())
    }

    pub(super) fn awake_release() -> Result<(), String> {
        let _ = std::fs::remove_file(HOLD_STATE_PATH);
        pmset_disablesleep(false)
    }

    struct RealWatch {
        started: std::time::Instant,
    }

    impl WatchEnv for RealWatch {
        fn current_gen(&self) -> Option<u64> {
            read_gen()
        }
        fn pid_alive(&self, pid: u32) -> bool {
            pid_alive(pid)
        }
        fn elapsed_secs(&self) -> u64 {
            self.started.elapsed().as_secs()
        }
        fn sleep_secs(&mut self, secs: u64) {
            std::thread::sleep(std::time::Duration::from_secs(secs));
        }
        fn restore_sleep(&mut self) -> Result<(), String> {
            let _ = std::fs::remove_file(HOLD_STATE_PATH);
            pmset_disablesleep(false)
        }
    }

    /// The watcher: restore sleep when `pid` exits or `max` passes,
    /// unless a newer hold (another generation) took over.
    pub(super) fn awake_watch(pid: u32, max_secs: u64, gen: u64) -> Result<(), String> {
        watch_loop(&mut RealWatch { started: std::time::Instant::now() }, pid, max_secs, gen).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_allowlist_is_exact() {
        assert_eq!(parse_argv(&["version"]), Ok(HelperCommand::Version));
        assert_eq!(parse_argv(&["wake-set", "1790000000"]), Ok(HelperCommand::WakeSet { unix: 1_790_000_000 }));
        assert_eq!(parse_argv(&["wake-clear"]), Ok(HelperCommand::WakeClear));
        assert_eq!(
            parse_argv(&["awake-hold", "--pid", "42", "--max", "600"]),
            Ok(HelperCommand::AwakeHold { pid: 42, max_secs: 600 })
        );
        for bad in [
            vec![],
            vec!["wake-set"],
            vec!["wake-set", "-5"],
            vec!["wake-set", "12; rm -rf /"],
            vec!["wake-set", "1", "2"],
            vec!["awake-hold", "--pid", "42", "--max", "0"],
            vec!["awake-hold", "--pid", "42", "--max", "999999"],
            vec!["awake-hold", "--max", "60", "--pid", "42"],
            vec!["sh", "-c", "id"],
        ] {
            let e = parse_argv(&bad).expect_err(&format!("{bad:?} must be refused"));
            assert!(e.starts_with("power helper:"), "{e}");
        }
    }

    /// A mock helper state: the generation file, the daemon pid and a
    /// fake clock. `renew_at` = seconds at which the daemon renews.
    struct MockWatch {
        now: u64,
        gen: Option<u64>,
        daemon_dies_at: Option<u64>,
        renew_at: Vec<u64>,
        restored_at: Vec<u64>,
    }

    impl WatchEnv for MockWatch {
        fn current_gen(&self) -> Option<u64> {
            self.gen
        }
        fn pid_alive(&self, _pid: u32) -> bool {
            !matches!(self.daemon_dies_at, Some(t) if self.now >= t)
        }
        fn elapsed_secs(&self) -> u64 {
            self.now
        }
        fn sleep_secs(&mut self, secs: u64) {
            let before = self.now;
            self.now += secs;
            if self.renew_at.iter().any(|&t| t > before && t <= self.now) {
                self.gen = self.gen.map(|g| g + 1);
            }
        }
        fn restore_sleep(&mut self) -> Result<(), String> {
            self.restored_at.push(self.now);
            self.gen = None;
            Ok(())
        }
    }

    fn mock(renew_at: Vec<u64>, daemon_dies_at: Option<u64>) -> MockWatch {
        MockWatch { now: 0, gen: Some(1), daemon_dies_at, renew_at, restored_at: Vec::new() }
    }

    /// S6 — renewals stop (daemon hung): the watcher restores sleep
    /// once, at the deadline, not before.
    #[test]
    fn watchdog_restores_sleep_when_renewals_stop() {
        let mut env = mock(vec![], None);
        let out = watch_loop(&mut env, 4242, 180, 1).expect("watch");
        assert_eq!(out, WatchOutcome::RestoredNoRenewal);
        assert_eq!(env.restored_at.len(), 1, "restored exactly once");
        let at = env.restored_at[0];
        assert!((180..180 + WATCH_POLL_SECS).contains(&at), "restored at {at}s for a 180s max");
    }

    /// The daemon dies: restored on the next look, long before `max`.
    #[test]
    fn watchdog_restores_sleep_when_the_daemon_dies() {
        let mut env = mock(vec![], Some(22));
        let out = watch_loop(&mut env, 4242, 180, 1).expect("watch");
        assert_eq!(out, WatchOutcome::RestoredDaemonGone);
        assert_eq!(env.restored_at, vec![25]);
    }

    /// A renewal starts a newer generation: this watcher steps aside and
    /// never touches sleep (the new watcher owns the deadline).
    #[test]
    fn watchdog_steps_aside_on_renewal() {
        let mut env = mock(vec![60], None);
        let out = watch_loop(&mut env, 4242, 180, 1).expect("watch");
        assert_eq!(out, WatchOutcome::SteppedAside);
        assert!(env.restored_at.is_empty(), "a renewed hold must not be cut");
        assert_eq!(env.gen, Some(2));
    }

    /// One watcher's view of the shared mock: it measures from its own
    /// start, like the real one.
    struct FromStart<'a>(&'a mut MockWatch, u64);

    impl WatchEnv for FromStart<'_> {
        fn current_gen(&self) -> Option<u64> {
            self.0.current_gen()
        }
        fn pid_alive(&self, pid: u32) -> bool {
            self.0.pid_alive(pid)
        }
        fn elapsed_secs(&self) -> u64 {
            self.0.now - self.1
        }
        fn sleep_secs(&mut self, secs: u64) {
            self.0.sleep_secs(secs)
        }
        fn restore_sleep(&mut self) -> Result<(), String> {
            self.0.restore_sleep()
        }
    }

    /// The whole chain: the daemon renews at 60, 120 and 180 s, then
    /// stops. Each watcher hands over to the next; the last one restores
    /// sleep one `max` after the last renewal, exactly once.
    #[test]
    fn watchdog_chain_restores_after_the_last_renewal() {
        let renewals = vec![60, 120, 180];
        let mut env = mock(renewals.clone(), None);
        let (mut gen, mut start) = (1u64, 0u64);
        let mut handovers = 0;
        loop {
            match watch_loop(&mut FromStart(&mut env, start), 4242, 180, gen).expect("watch") {
                WatchOutcome::SteppedAside => {
                    start = renewals[handovers];
                    handovers += 1;
                    gen += 1;
                }
                WatchOutcome::RestoredNoRenewal => break,
                WatchOutcome::RestoredDaemonGone => panic!("the daemon never died in this run"),
            }
        }
        assert_eq!(handovers, 3);
        assert_eq!(env.restored_at, vec![360], "last renewal 180 s + max 180 s");
    }

    #[test]
    fn wake_time_is_clamped() {
        let now = 1_790_000_000;
        assert_eq!(clamp_wake(now - 100, now), now + 30);
        assert_eq!(clamp_wake(now + 600, now), now + 600);
        assert_eq!(clamp_wake(now + 90 * 24 * 3600, now), now + 31 * 24 * 3600);
    }

    #[test]
    fn install_script_writes_one_narrow_sudoers_line() {
        let s = install_script("z3thon", "/Applications/K2.app/Contents/MacOS/k2-power-helper")
            .expect("valid inputs");
        assert!(s.contains("z3thon ALL=(root) NOPASSWD: /Library/PrivilegedHelperTools/dev.k2.power-helper"), "{s}");
        assert!(s.contains("/usr/sbin/visudo -cf"), "sudoers must be checked before install: {s}");
        assert!(s.contains("-m 0440"), "{s}");
        assert!(s.contains("-o root -g wheel -m 0755 '/Applications/K2.app/Contents/MacOS/k2-power-helper'"), "{s}");
        assert!(install_script("bad user", "/x").is_err());
        assert!(install_script("root;id", "/x").is_err());
        assert!(install_script("z3thon", "/tmp/it's").is_err());
        assert!(install_script("z3thon", "relative/path").is_err());

        let apple = install_applescript(&s);
        assert!(apple.starts_with("do shell script \""), "{apple}");
        assert!(apple.ends_with("with administrator privileges"), "{apple}");
        let bytes = apple.as_bytes();
        let bare_quotes = (0..bytes.len())
            .filter(|&i| bytes[i] == b'"' && (i == 0 || bytes[i - 1] != b'\\'))
            .count();
        assert_eq!(bare_quotes, 4, "only the script and prompt delimiters may be bare quotes: {apple}");
    }
}
