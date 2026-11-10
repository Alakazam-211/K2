//! Local-input detection on macOS (D4, vs-live R13/R14): an event tap on
//! the session sees every mouse and keyboard event; an event that doesn't
//! carry deskd's [`INJECTED_TAG`] (and wasn't posted by deskd's pid) is the
//! person at the Mac, and starts or extends the 3 s local hold.
//!
//! **Which tap (R13).** A listen-only tap on keyboard events needs Input
//! Monitoring, a third grant. An active tap needs Accessibility only, which
//! deskd holds anyway, so this is an active tap that passes every event on
//! unchanged. macOS switches an active tap off if it stalls
//! (`kCGEventTapDisabledByTimeout`); the callback turns it back on. The
//! callback does one atomic store, so it never stalls the person's input.
//!
//! The tap needs Accessibility to exist: until it's granted, creating it
//! fails, and the thread retries every 5 s.

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::ffi::*;
use super::inject::INJECTED_TAG;
use crate::hold::LocalHold;

struct TapCtx {
    hold: Arc<LocalHold>,
    pid: i64,
    port: AtomicPtr<c_void>,
}

const fn bit(t: u32) -> u64 {
    1u64 << t
}

/// Mouse, wheel and keyboard events (not tablet/gesture noise).
pub const MASK: u64 = bit(kCGEventLeftMouseDown)
    | bit(kCGEventLeftMouseUp)
    | bit(kCGEventRightMouseDown)
    | bit(kCGEventRightMouseUp)
    | bit(kCGEventMouseMoved)
    | bit(kCGEventLeftMouseDragged)
    | bit(kCGEventRightMouseDragged)
    | bit(kCGEventKeyDown)
    | bit(kCGEventKeyUp)
    | bit(kCGEventFlagsChanged)
    | bit(kCGEventScrollWheel)
    | bit(kCGEventOtherMouseDown)
    | bit(kCGEventOtherMouseUp)
    | bit(kCGEventOtherMouseDragged);

/// Is this event deskd's own? (Pure, for tests.)
pub fn is_injected(user_data: i64, source_pid: i64, our_pid: i64) -> bool {
    user_data == INJECTED_TAG || source_pid == our_pid
}

extern "C" fn callback(_proxy: CGEventTapProxy, kind: u32, event: CGEventRef, user_info: *mut c_void) -> CGEventRef {
    // SAFETY: user_info is the leaked TapCtx below, alive for the process.
    let ctx = unsafe { &*(user_info as *const TapCtx) };
    if kind == kCGEventTapDisabledByTimeout || kind == kCGEventTapDisabledByUserInput {
        let port = ctx.port.load(Ordering::SeqCst);
        if !port.is_null() {
            // SAFETY: the port is ours and alive.
            unsafe { CGEventTapEnable(port, true) };
        }
        return event;
    }
    if !event.is_null() {
        // SAFETY: reading fields of the event the tap hands us.
        let (tag, pid) = unsafe { (CGEventGetIntegerValueField(event, kCGEventSourceUserData), CGEventGetIntegerValueField(event, kCGEventSourceUnixProcessID)) };
        if !is_injected(tag, pid, ctx.pid) {
            ctx.hold.note_local();
        }
    }
    event
}

/// Starts the tap on its own thread with its own run loop.
pub fn spawn(hold: Arc<LocalHold>) {
    let ctx: &'static TapCtx = Box::leak(Box::new(TapCtx { hold, pid: std::process::id() as i64, port: AtomicPtr::new(std::ptr::null_mut()) }));
    std::thread::Builder::new()
        .name("k2-deskd-tap".into())
        .spawn(move || loop {
            // SAFETY: creating a tap with a 'static context.
            let port = unsafe { CGEventTapCreate(kCGSessionEventTap, kCGHeadInsertEventTap, kCGEventTapOptionDefault, MASK, callback, ctx as *const TapCtx as *mut c_void) };
            if port.is_null() {
                // No Accessibility yet.
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
            ctx.port.store(port, Ordering::SeqCst);
            // SAFETY: standard run-loop wiring for a mach-port source.
            unsafe {
                let src = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, port, 0);
                CFRunLoopAddSource(CFRunLoopGetCurrent(), src, kCFRunLoopCommonModes);
                CGEventTapEnable(port, true);
                CFRunLoopRun();
            }
            // The run loop only returns if the source went away; start over.
            std::thread::sleep(Duration::from_secs(1));
        })
        .expect("spawn event tap thread");
}

/// HID idle (any input, including posted events if CGEventPost resets it:
/// D0 check 3 decides whether that is so).
pub fn hid_idle_ms() -> Option<u64> {
    // SAFETY: plain call.
    let s = unsafe { CGEventSourceSecondsSinceLastEventType(kCGEventSourceStateHIDSystemState, kCGAnyInputEventType) };
    (s.is_finite() && s >= 0.0).then(|| (s * 1000.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_tag_or_our_pid_is_injected_anything_else_is_local() {
        assert!(is_injected(INJECTED_TAG, 0, 77));
        assert!(is_injected(0, 77, 77));
        assert!(!is_injected(0, 0, 77)); // hardware: source pid 0
        assert!(!is_injected(5, 1234, 77)); // another app's synthetic input counts as local
    }

    #[test]
    fn mask_covers_keys_and_buttons() {
        assert_ne!(MASK & bit(kCGEventKeyDown), 0);
        assert_ne!(MASK & bit(kCGEventFlagsChanged), 0);
        assert_ne!(MASK & bit(kCGEventOtherMouseDragged), 0);
    }
}
