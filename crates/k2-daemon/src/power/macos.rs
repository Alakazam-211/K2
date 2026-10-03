//! macOS power layer. Thin: one IOKit call per job, plus the root helper
//! for wake events.
//!
//! - Keep awake: `IOPMAssertionCreateWithName` with
//!   `PreventUserIdleSystemSleep` (idle sleep, any power source) and
//!   `PreventSystemSleep` (holds a dark wake open, AC only). No admin.
//!   powerd drops both if the daemon dies.
//! - Wake: `sudo -n dev.k2.power-helper wake-set <ts>` (D11). Without the
//!   helper, wake is unavailable and turning it on shows one dialog.
//! - Lid closed (S6 Keep awake): `sudo -n dev.k2.power-helper awake-hold
//!   --pid <daemon> --max 180`, renewed every 60 s by a thread the hold
//!   owns; `awake-release` on drop. The helper's watcher restores sleep if
//!   the daemon dies or stops renewing. A marker under `~/.k2` lets the
//!   next boot clear a hold a crash or reboot left behind.

use std::ffi::{c_char, c_void, CString};

use chrono::{DateTime, Utc};

use super::helper;
use super::{LidAccess, LidFacts, LidRefusal, PowerOs, PowerSource, WakeSupport};

/// S6 — each `awake-hold` lasts this long unless renewed.
const LID_HOLD_MAX_SECS: u64 = 180;
/// S6 — renew well inside the max.
const LID_RENEW_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

fn lid_marker() -> std::path::PathBuf {
    k2_core::paths::k2_home().join("keep-awake-lid.hold")
}

fn awake_hold_args() -> Vec<String> {
    vec![
        "awake-hold".into(),
        "--pid".into(),
        std::process::id().to_string(),
        "--max".into(),
        LID_HOLD_MAX_SECS.to_string(),
    ]
}

fn call_owned(args: &[String]) -> Result<String, String> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    helper::call(&refs)
}

/// The lid-closed hold: a renewer thread; drop stops it and releases.
struct MacLidHold {
    stop: Option<std::sync::mpsc::Sender<()>>,
    renewer: Option<std::thread::JoinHandle<()>>,
}

impl Drop for MacLidHold {
    fn drop(&mut self) {
        drop(self.stop.take()); // disconnect -> the renewer exits
        if let Some(t) = self.renewer.take() {
            let _ = t.join();
        }
        if let Err(e) = helper::call(&["awake-release"]) {
            k2_core::log_debug!(
                "[keep-awake] awake-release failed (the helper watcher restores sleep within {LID_HOLD_MAX_SECS}s): {e}"
            );
        }
        let _ = std::fs::remove_file(lid_marker());
    }
}

type CFStringRef = *const c_void;
const UTF8: u32 = 0x0800_0100;
const ASSERTION_LEVEL_ON: u32 = 255;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(alloc: *const c_void, s: *const c_char, enc: u32) -> CFStringRef;
    fn CFRelease(cf: *const c_void);
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMAssertionCreateWithName(kind: CFStringRef, level: u32, name: CFStringRef, id: *mut u32) -> i32;
    fn IOPMAssertionRelease(id: u32) -> i32;
}

fn cf_string(s: &str) -> Result<CFStringRef, String> {
    let c = CString::new(s).map_err(|_| "nul in CFString".to_string())?;
    let r = unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) };
    if r.is_null() {
        Err(format!("CFStringCreateWithCString({s}) failed"))
    } else {
        Ok(r)
    }
}

fn create_assertion(kind: &str, name: &str) -> Result<u32, String> {
    let k = cf_string(kind)?;
    let n = match cf_string(name) {
        Ok(n) => n,
        Err(e) => {
            unsafe { CFRelease(k) };
            return Err(e);
        }
    };
    let mut id: u32 = 0;
    let rc = unsafe { IOPMAssertionCreateWithName(k, ASSERTION_LEVEL_ON, n, &mut id) };
    unsafe {
        CFRelease(k);
        CFRelease(n);
    }
    if rc != 0 {
        return Err(format!("IOPMAssertionCreateWithName({kind}) returned {rc:#x}"));
    }
    Ok(id)
}

/// Both assertions; released on drop.
struct MacAssertion {
    ids: Vec<u32>,
}

impl Drop for MacAssertion {
    fn drop(&mut self) {
        for id in &self.ids {
            unsafe { IOPMAssertionRelease(*id) };
        }
    }
}

pub struct MacPowerOs;

impl PowerOs for MacPowerOs {
    fn hold_awake(&self, reason: &str) -> Result<Box<dyn Send>, String> {
        let idle = create_assertion("PreventUserIdleSystemSleep", reason)?;
        let mut ids = vec![idle];
        // Honored on AC only; keeps a scheduled dark wake up for the run.
        match create_assertion("PreventSystemSleep", reason) {
            Ok(id) => ids.push(id),
            Err(e) => k2_core::log_debug!("[power] PreventSystemSleep not held: {e}"),
        }
        Ok(Box::new(MacAssertion { ids }))
    }

    fn wake_support(&self) -> WakeSupport {
        if helper::installed() {
            WakeSupport::Ready
        } else {
            WakeSupport::Unavailable(
                "wake helper not installed: turning the switch on asks once for an admin password".into(),
            )
        }
    }

    fn set_wake(&self, at: DateTime<Utc>) -> Result<(), String> {
        helper::call(&["wake-set", &at.timestamp().to_string()]).map(|_| ())
    }

    fn clear_wake(&self) -> Result<(), String> {
        helper::call(&["wake-clear"]).map(|_| ())
    }

    fn power_source(&self) -> PowerSource {
        match std::process::Command::new(helper::PMSET_PATH).args(["-g", "batt"]).output() {
            Ok(o) if o.status.success() => super::parse::pmset_batt(&String::from_utf8_lossy(&o.stdout)),
            _ => PowerSource::default(),
        }
    }

    fn wake_needs_approval(&self) -> bool {
        !helper::installed()
    }

    fn lid_helper_approved(&self) -> bool {
        helper::installed()
    }

    /// D11 — the one admin dialog. Copies the bundled `k2-power-helper`
    /// to `/Library/PrivilegedHelperTools` and writes the sudoers rule.
    /// Gives up after 2 minutes (nobody at the screen).
    fn install_wake_helper(&self) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
        let src = exe
            .parent()
            .ok_or_else(|| "daemon has no parent dir".to_string())?
            .join(helper::BUNDLED_HELPER_NAME);
        if !src.exists() {
            return Err(format!("helper binary missing at {}", src.display()));
        }
        let user = std::env::var("USER").map_err(|_| "USER is not set".to_string())?;
        let script = helper::install_script(&user, &src.to_string_lossy())?;
        let mut child = std::process::Command::new("/usr/bin/osascript")
            .args(["-e", &helper::install_applescript(&script)])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("run osascript: {e}"))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() && helper::installed() {
                        return Ok(());
                    }
                    let mut err = String::new();
                    if let Some(mut e) = child.stderr.take() {
                        use std::io::Read;
                        let _ = e.read_to_string(&mut err);
                    }
                    return Err(if err.contains("-128") || err.contains("canceled") {
                        "The admin dialog was declined.".to_string()
                    } else {
                        format!("Helper install failed: {}", err.trim())
                    });
                }
                Ok(None) if std::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("No one approved the admin dialog on this Mac within 2 minutes.".into());
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(250)),
                Err(e) => return Err(format!("wait osascript: {e}")),
            }
        }
    }

    fn lid_facts(&self, _src: PowerSource) -> LidFacts {
        LidFacts {
            access: if helper::installed() { LidAccess::Ready } else { LidAccess::NeedsApproval },
            ac_only_unless_allowed: true,
        }
    }

    fn hold_lid_closed(&self, _reason: &str) -> Result<Box<dyn Send>, LidRefusal> {
        let refuse = |reason: String| LidRefusal { no_session: false, reason };
        if !helper::installed() {
            return Err(refuse("the helper is not installed".into()));
        }
        let args = awake_hold_args();
        call_owned(&args).map_err(refuse)?;
        if let Err(e) = std::fs::write(lid_marker(), std::process::id().to_string()) {
            k2_core::log_debug!("[keep-awake] could not write the lid marker: {e}");
        }
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let renewer = std::thread::Builder::new()
            .name("k2-keep-awake-lid".into())
            .spawn(move || loop {
                match rx.recv_timeout(LID_RENEW_EVERY) {
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if let Err(e) = call_owned(&args) {
                            k2_core::log_debug!("[keep-awake] lid hold renewal failed: {e}");
                        }
                    }
                    _ => return,
                }
            })
            .map_err(|e| {
                let _ = helper::call(&["awake-release"]);
                let _ = std::fs::remove_file(lid_marker());
                refuse(format!("could not start the renewal thread: {e}"))
            })?;
        Ok(Box::new(MacLidHold { stop: Some(tx), renewer: Some(renewer) }))
    }

    fn clear_stale_lid_hold(&self) {
        let marker = lid_marker();
        if !marker.exists() {
            return;
        }
        if helper::installed() {
            match helper::call(&["awake-release"]) {
                Ok(_) => k2_core::log_debug!("[keep-awake] cleared a lid hold left by a previous run"),
                Err(e) => k2_core::log_debug!("[keep-awake] stale lid hold not cleared: {e}"),
            }
        }
        let _ = std::fs::remove_file(marker);
    }

    fn notes(&self) -> serde_json::Value {
        serde_json::json!({
            "helperInstalled": helper::installed(),
            "helperPath": helper::HELPER_PATH,
            "lidClosed": "With the lid closed a scheduled wake is a short dark wake; on AC K2 holds it for the run, on battery the Mac may sleep again within about a minute.",
        })
    }
}
