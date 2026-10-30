//! Windows power layer. Thin, no admin, no UAC.
//!
//! - Keep awake: `PowerCreateRequest` + `PowerSetRequest(SystemRequired)`;
//!   `PowerClearRequest` + `CloseHandle` on drop (and the handle dies
//!   with the process). The power plan's lid action still wins.
//! - Wake: one waitable timer set with `fResume = TRUE`. Windows honors
//!   it only when the plan's "Allow wake timers" (RTCWAKE) is enabled, so
//!   a scheduled heartbeat can wake a sleeping PC only then. K2 reads that
//!   setting and, when it is off, reports wake as unavailable with the
//!   steps; it never changes the plan (D14).
//! - Plan settings (RTCWAKE, the lid action) are read straight from the
//!   active power scheme with powrprof (`PowerGetActiveScheme` +
//!   `PowerReadACValueIndex` / `PowerReadDCValueIndex`): a registry read,
//!   no child process. The scheduler re-plans every tick, and starting
//!   `powercfg.exe` that often opened a console window each time.

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
    fn LocalFree(mem: *mut c_void) -> *mut c_void;
}

/// A Win32 `GUID`.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// `GUID_SLEEP_SUBGROUP` (powercfg alias SUB_SLEEP).
const SUB_SLEEP: Guid =
    Guid { data1: 0x238c_9fa8, data2: 0x0aad, data3: 0x41ed, data4: [0x83, 0xf4, 0x97, 0xbe, 0x24, 0x2c, 0x8f, 0x20] };
/// "Allow wake timers" (alias RTCWAKE): 0 disable, 1 enable, 2 important only.
const RTCWAKE: Guid =
    Guid { data1: 0xbd3b_718a, data2: 0x0680, data3: 0x4d9d, data4: [0x8a, 0xb2, 0xe1, 0xd2, 0xb4, 0xac, 0x80, 0x6d] };
/// `GUID_SYSTEM_BUTTON_SUBGROUP` (alias SUB_BUTTONS).
const SUB_BUTTONS: Guid =
    Guid { data1: 0x4f97_1e89, data2: 0xeebd, data3: 0x4455, data4: [0xa8, 0xde, 0x9e, 0x59, 0x04, 0x0e, 0x73, 0x47] };
/// "Lid close action" (alias LIDACTION): 0 do nothing, 1 sleep, 2 hibernate, 3 shut down.
const LIDACTION: Guid =
    Guid { data1: 0x5ca8_3367, data2: 0x6e45, data3: 0x459f, data4: [0xa2, 0x7b, 0x47, 0x6b, 0x1d, 0x01, 0xc9, 0x36] };

#[link(name = "powrprof")]
extern "system" {
    fn PowerGetActiveScheme(user_root: *mut c_void, active: *mut *mut Guid) -> u32;
    fn PowerReadACValueIndex(root: *mut c_void, scheme: *const Guid, sub: *const Guid, setting: *const Guid, value: *mut u32) -> u32;
    fn PowerReadDCValueIndex(root: *mut c_void, scheme: *const Guid, sub: *const Guid, setting: *const Guid, value: *mut u32) -> u32;
    fn PowerReadSettingAttributes(sub: *const Guid, setting: *const Guid) -> u32;
}

/// `POWER_ATTRIBUTE_HIDE`: the setting doesn't apply to this machine (the
/// lid action on a PC with no lid); `powercfg /q` leaves it out.
const POWER_ATTRIBUTE_HIDE: u32 = 0x1;

/// The active power scheme's (plugged in, on battery) index for one
/// setting, as `powercfg /q SCHEME_CURRENT <sub> <setting>` shows it.
/// `None` for a side that can't be read, and for a hidden setting (which
/// powercfg doesn't list either).
fn plan_setting(sub: &Guid, setting: &Guid) -> (Option<u32>, Option<u32>) {
    // SAFETY: two valid GUID pointers.
    if unsafe { PowerReadSettingAttributes(sub, setting) } & POWER_ATTRIBUTE_HIDE != 0 {
        return (None, None);
    }
    let mut scheme: *mut Guid = std::ptr::null_mut();
    // SAFETY: the API allocates the GUID; it is freed once with LocalFree.
    if unsafe { PowerGetActiveScheme(std::ptr::null_mut(), &mut scheme) } != 0 || scheme.is_null() {
        return (None, None);
    }
    let read = |f: unsafe extern "system" fn(*mut c_void, *const Guid, *const Guid, *const Guid, *mut u32) -> u32| {
        let mut v = 0u32;
        // SAFETY: valid GUID pointers and our own out-param.
        (unsafe { f(std::ptr::null_mut(), scheme, sub, setting, &mut v) } == 0).then_some(v)
    };
    let out = (read(PowerReadACValueIndex), read(PowerReadDCValueIndex));
    // SAFETY: the block PowerGetActiveScheme returned, freed once.
    unsafe { LocalFree(scheme.cast()) };
    out
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

    /// "Allow wake timers" (plugged in, on battery).
    fn wake_timers_allowed(&self) -> (Option<u32>, Option<u32>) {
        plan_setting(&SUB_SLEEP, &RTCWAKE)
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
        let (ac, dc) = plan_setting(&SUB_BUTTONS, &LIDACTION);
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

    /// The power plan's lid action decides; K2 only reports it (D14).
    fn lid_switch_applies(&self) -> bool {
        false
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `powercfg /q` single-setting output -> (AC, DC) current index.
    fn powercfg_index(text: &str) -> (Option<u32>, Option<u32>) {
        let read = |label: &str| {
            text.lines()
                .find(|l| l.contains(label))
                .and_then(|l| l.rsplit("0x").next())
                .and_then(|hex| u32::from_str_radix(hex.trim(), 16).ok())
        };
        (read("Current AC Power Setting Index"), read("Current DC Power Setting Index"))
    }

    fn powercfg(sub: &str, setting: &str) -> (Option<u32>, Option<u32>) {
        let o = k2_core::bg_proc::command("powercfg").args(["/q", "SCHEME_CURRENT", sub, setting]).output().expect("powercfg runs");
        assert!(o.status.success(), "{o:?}");
        let text = String::from_utf8_lossy(&o.stdout);
        let got = powercfg_index(&text);
        eprintln!("powercfg {sub} {setting}: {got:?}");
        got
    }

    #[test]
    fn powercfg_text_parses() {
        let text = "    Power Setting GUID: bd3b718a-0680-4d9d-8ab2-e1d2b4ac806d  (Allow wake timers)\n      Possible Setting Index: 000\n    Current AC Power Setting Index: 0x00000001\n    Current DC Power Setting Index: 0x00000000\n";
        assert_eq!(powercfg_index(text), (Some(1), Some(0)));
        assert_eq!(powercfg_index("garbage"), (None, None));
    }

    /// The powrprof reads give what `powercfg /q` prints for the active
    /// plan (the test is the one place K2 still runs powercfg), including
    /// "nothing" for a hidden setting such as the lid on a PC with no lid.
    #[test]
    fn plan_reads_match_powercfg() {
        let rtc = plan_setting(&SUB_SLEEP, &RTCWAKE);
        assert!(rtc.0.is_some() && rtc.1.is_some(), "{rtc:?}");
        assert_eq!(rtc, powercfg("SUB_SLEEP", "RTCWAKE"));
        assert_eq!(plan_setting(&SUB_BUTTONS, &LIDACTION), powercfg("SUB_BUTTONS", "LIDACTION"));
    }
}
