//! Windows power layer. Thin, no admin, no UAC.
//!
//! - Keep awake: `PowerCreateRequest` + `PowerSetRequest(SystemRequired)`;
//!   `PowerClearRequest` + `CloseHandle` on drop (and the handle dies
//!   with the process). The power plan's lid action still wins.
//! - Wake: one waitable timer set with `fResume = TRUE`. Windows honors
//!   it only when the plan's "Allow wake timers" is enabled. K2 reads
//!   that setting (`powercfg /q SCHEME_CURRENT SUB_SLEEP RTCWAKE`) and
//!   shows the steps; it never changes the plan (D14).

use std::ffi::c_void;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;

use super::{LidAccess, LidFacts, LidRefusal, PowerOs, PowerSource, WakeSupport};

type Handle = *mut c_void;

#[repr(C)]
struct ReasonContext {
    version: u32,
    flags: u32,
    /// Union in the SDK; with `POWER_REQUEST_CONTEXT_SIMPLE_STRING` only
    /// the first pointer is read. Padded to the union's full size.
    simple_reason: *mut u16,
    _pad: [usize; 2],
}

#[repr(C)]
struct SystemPowerStatus {
    ac_line_status: u8,
    battery_flag: u8,
    battery_life_percent: u8,
    system_status_flag: u8,
    battery_life_time: u32,
    battery_full_life_time: u32,
}

const POWER_REQUEST_CONTEXT_VERSION: u32 = 0;
const POWER_REQUEST_CONTEXT_SIMPLE_STRING: u32 = 0x1;
const POWER_REQUEST_SYSTEM_REQUIRED: i32 = 1;
const CREATE_WAITABLE_TIMER_MANUAL_RESET: u32 = 0x1;
const TIMER_ALL_ACCESS: u32 = 0x001F_0003;
/// 100-ns intervals between 1601-01-01 and 1970-01-01.
const FILETIME_UNIX_OFFSET_SECS: i64 = 11_644_473_600;

#[link(name = "kernel32")]
extern "system" {
    fn PowerCreateRequest(context: *const ReasonContext) -> Handle;
    fn PowerSetRequest(h: Handle, kind: i32) -> i32;
    fn PowerClearRequest(h: Handle, kind: i32) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn CreateWaitableTimerExW(attrs: *const c_void, name: *const u16, flags: u32, access: u32) -> Handle;
    fn SetWaitableTimer(
        h: Handle,
        due: *const i64,
        period: i32,
        completion: *const c_void,
        arg: *const c_void,
        resume: i32,
    ) -> i32;
    fn CancelWaitableTimer(h: Handle) -> i32;
    fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    fn GetLastError() -> u32;
}

struct PowerRequest {
    h: Handle,
    _reason: Vec<u16>,
}

// The handle is a kernel object handle, usable from any thread.
unsafe impl Send for PowerRequest {}

impl Drop for PowerRequest {
    fn drop(&mut self) {
        unsafe {
            PowerClearRequest(self.h, POWER_REQUEST_SYSTEM_REQUIRED);
            CloseHandle(self.h);
        }
    }
}

struct Timer(Handle);
unsafe impl Send for Timer {}
unsafe impl Sync for Timer {}

/// D14 — the steps K2 shows; it never changes the plan itself.
pub(crate) const WAKE_TIMER_STEPS: &str = "Control Panel → Hardware and Sound → Power Options → Change plan settings → Change advanced power settings → Sleep → Allow wake timers → set \"Plugged in\" (and \"On battery\" if you want) to Enable.";

/// D14 — the lid steps K2 shows; it never changes the plan itself.
pub(crate) const LID_ACTION_STEPS: &str = "Control Panel → Hardware and Sound → Power Options → Choose what closing the lid does → When I close the lid → set \"Plugged in\" (and \"On battery\" if you want) to Do nothing → Save changes.";

pub struct WindowsPowerOs {
    timer: Mutex<Option<Timer>>,
}

impl WindowsPowerOs {
    pub fn new() -> Self {
        let h = unsafe {
            CreateWaitableTimerExW(
                std::ptr::null(),
                std::ptr::null(),
                CREATE_WAITABLE_TIMER_MANUAL_RESET,
                TIMER_ALL_ACCESS,
            )
        };
        Self { timer: Mutex::new(if h.is_null() { None } else { Some(Timer(h)) }) }
    }

    fn wake_timers_allowed(&self) -> (Option<u32>, Option<u32>) {
        match std::process::Command::new("powercfg")
            .args(["/q", "SCHEME_CURRENT", "SUB_SLEEP", "RTCWAKE"])
            .output()
        {
            Ok(o) if o.status.success() => super::parse::windows_rtcwake(&String::from_utf8_lossy(&o.stdout)),
            _ => (None, None),
        }
    }
}

impl PowerOs for WindowsPowerOs {
    fn hold_awake(&self, reason: &str) -> Result<Box<dyn Send>, String> {
        let mut wide: Vec<u16> = reason.encode_utf16().chain(std::iter::once(0)).collect();
        let ctx = ReasonContext {
            version: POWER_REQUEST_CONTEXT_VERSION,
            flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            simple_reason: wide.as_mut_ptr(),
            _pad: [0; 2],
        };
        let h = unsafe { PowerCreateRequest(&ctx) };
        if h.is_null() || h as isize == -1 {
            return Err(format!("PowerCreateRequest failed ({})", unsafe { GetLastError() }));
        }
        if unsafe { PowerSetRequest(h, POWER_REQUEST_SYSTEM_REQUIRED) } == 0 {
            let code = unsafe { GetLastError() };
            unsafe { CloseHandle(h) };
            return Err(format!("PowerSetRequest failed ({code})"));
        }
        Ok(Box::new(PowerRequest { h, _reason: wide }))
    }

    fn wake_support(&self) -> WakeSupport {
        if self.timer.lock().is_none() {
            return WakeSupport::Unavailable("could not create a wake timer".into());
        }
        match self.wake_timers_allowed() {
            (Some(0), _) => WakeSupport::Unavailable(format!(
                "this power plan does not allow wake timers when plugged in. {WAKE_TIMER_STEPS}"
            )),
            _ => WakeSupport::Ready,
        }
    }

    fn set_wake(&self, at: DateTime<Utc>) -> Result<(), String> {
        let guard = self.timer.lock();
        let t = guard.as_ref().ok_or_else(|| "no wake timer".to_string())?;
        // Positive = absolute FILETIME (UTC, 100 ns since 1601).
        let due: i64 = (at.timestamp() + FILETIME_UNIX_OFFSET_SECS) * 10_000_000;
        let ok = unsafe { SetWaitableTimer(t.0, &due, 0, std::ptr::null(), std::ptr::null(), 1) };
        if ok == 0 {
            return Err(format!("SetWaitableTimer failed ({})", unsafe { GetLastError() }));
        }
        Ok(())
    }

    fn clear_wake(&self) -> Result<(), String> {
        if let Some(t) = self.timer.lock().as_ref() {
            unsafe { CancelWaitableTimer(t.0) };
        }
        Ok(())
    }

    fn power_source(&self) -> PowerSource {
        let mut s = SystemPowerStatus {
            ac_line_status: 255,
            battery_flag: 255,
            battery_life_percent: 255,
            system_status_flag: 0,
            battery_life_time: 0,
            battery_full_life_time: 0,
        };
        if unsafe { GetSystemPowerStatus(&mut s) } == 0 {
            return PowerSource::default();
        }
        PowerSource {
            on_ac: match s.ac_line_status {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            },
            battery_percent: if s.battery_life_percent <= 100 { Some(s.battery_life_percent) } else { None },
        }
    }

    /// S6 / D14 — lid closed keeps running only when the plan's lid
    /// action for this power source is "Do nothing". K2 reads it and
    /// shows the steps; it never changes it.
    fn lid_facts(&self, src: PowerSource) -> LidFacts {
        let (ac, dc) = match std::process::Command::new("powercfg")
            .args(["/q", "SCHEME_CURRENT", "SUB_BUTTONS", "LIDACTION"])
            .output()
        {
            Ok(o) if o.status.success() => super::parse::windows_setting_index(&String::from_utf8_lossy(&o.stdout)),
            _ => (None, None),
        };
        let current = if src.on_ac == Some(false) { dc } else { ac };
        let access = match current {
            Some(0) => LidAccess::Ready,
            Some(n) => LidAccess::Unavailable(format!(
                "the power plan's lid action is {}. {LID_ACTION_STEPS}",
                super::parse::windows_lid_action_word(n)
            )),
            None => LidAccess::Unavailable(format!("could not read the lid action. {LID_ACTION_STEPS}")),
        };
        LidFacts { access, ac_only_unless_allowed: false }
    }

    /// Nothing to take: with the lid action "Do nothing", the system
    /// request from the lid-open hold keeps it running.
    fn hold_lid_closed(&self, _reason: &str) -> Result<Box<dyn Send>, LidRefusal> {
        Ok(Box::new(()))
    }

    fn notes(&self) -> serde_json::Value {
        let (ac, dc) = self.wake_timers_allowed();
        let word = |v: Option<u32>| match v {
            Some(0) => "disabled",
            Some(1) => "enabled",
            Some(2) => "important_only",
            _ => "unknown",
        };
        serde_json::json!({
            "wakeTimers": { "pluggedIn": word(ac), "onBattery": word(dc) },
            "steps": WAKE_TIMER_STEPS,
        })
    }
}
