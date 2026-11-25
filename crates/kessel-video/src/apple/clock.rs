//! Capture times on the host clock: a capture sample's presentation time,
//! which is on its session's synchronization clock, as nanoseconds of
//! `mach_absolute_time()` ([`HostClock::MachAbsoluteTime`]).

use super::ffi::{self, CMClockRef, CMTime, CM_TIME_IMPLIED_VALUE, CM_TIME_VALID};
use crate::frame::{mach_ticks_to_nanos, HostClock, HostTime};
use std::sync::OnceLock;

/// `mach_timebase_info` as (numer, denom), read once; (0, 0) if the call
/// fails, which makes every conversion `None`.
pub(crate) fn timebase() -> (u32, u32) {
    static TIMEBASE: OnceLock<(u32, u32)> = OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = ffi::MachTimebaseInfo::default();
        // SAFETY: an out pointer to a live struct of the right layout.
        let r = unsafe { ffi::mach_timebase_info(&mut info) };
        if r != 0 || info.denom == 0 {
            (0, 0)
        } else {
            (info.numer, info.denom)
        }
    })
}

/// `mach_absolute_time()` now, in nanoseconds.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn now_nanos() -> Option<u64> {
    let (numer, denom) = timebase();
    // SAFETY: takes nothing; libSystem.
    mach_ticks_to_nanos(unsafe { ffi::mach_absolute_time() }, numer, denom)
}

/// `pts`, a time on `clock`, as a host time: moved to the host time clock
/// with `CMSyncConvertTime`, to mach ticks with
/// `CMClockConvertHostTimeToSystemUnits`, and to nanoseconds with
/// `mach_timebase_info`. `None` for a null clock, a time that isn't a
/// valid number (invalid, infinite or indefinite), or one before the host
/// clock's zero.
///
/// # Safety
/// `clock` must be null or a live CMClock or CMTimebase.
pub(crate) unsafe fn host_time_of(pts: CMTime, clock: CMClockRef) -> Option<HostTime> {
    if clock.is_null() || !is_number(pts) {
        return None;
    }
    let host = ffi::CMSyncConvertTime(pts, clock, ffi::CMClockGetHostTimeClock());
    if !is_number(host) || host.value < 0 {
        return None;
    }
    let ticks = ffi::CMClockConvertHostTimeToSystemUnits(host);
    let (numer, denom) = timebase();
    mach_ticks_to_nanos(ticks, numer, denom).map(|n| HostTime::new(n, HostClock::MachAbsoluteTime))
}

/// A valid, finite CMTime with a positive timescale.
fn is_number(t: CMTime) -> bool {
    t.flags & CM_TIME_VALID != 0 && t.flags & CM_TIME_IMPLIED_VALUE == 0 && t.timescale > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_timebase_is_real() {
        let (numer, denom) = timebase();
        assert!(numer > 0 && denom > 0, "{numer}/{denom}");
    }

    #[test]
    fn host_clock_times_are_mach_absolute_time_in_nanoseconds() {
        // SAFETY: the host time clock is a process-wide CMClock that lives
        // for the whole process.
        unsafe {
            let host = ffi::CMClockGetHostTimeClock();
            assert!(!host.is_null());
            let before = now_nanos().expect("now");
            let t = host_time_of(ffi::CMClockGetTime(host), host).expect("a host time");
            let after = now_nanos().expect("now");
            assert_eq!(t.clock, HostClock::MachAbsoluteTime);
            assert!(
                before <= t.nanos && t.nanos <= after,
                "{before} <= {} <= {after}",
                t.nanos
            );
            // An exact tick count comes back as exactly its nanoseconds.
            let ticks = ffi::mach_absolute_time();
            let (numer, denom) = timebase();
            let t = host_time_of(ffi::CMClockMakeHostTimeFromSystemUnits(ticks), host)
                .expect("a host time");
            let want = mach_ticks_to_nanos(ticks, numer, denom).expect("ns");
            // CMTime may round the tick count to its timescale: within 1 µs.
            assert!(t.nanos.abs_diff(want) < 1_000, "{} vs {want}", t.nanos);
        }
    }

    #[test]
    fn invalid_and_infinite_times_and_a_null_clock_give_none() {
        // SAFETY: as above; a null clock is allowed.
        unsafe {
            let host = ffi::CMClockGetHostTimeClock();
            assert_eq!(host_time_of(ffi::CM_TIME_INVALID, host), None);
            let mut inf = ffi::cm_time(0, 1);
            inf.flags |= 4; // kCMTimeFlags_PositiveInfinity
            assert_eq!(host_time_of(inf, host), None);
            let mut indefinite = ffi::cm_time(0, 1);
            indefinite.flags |= 16; // kCMTimeFlags_Indefinite
            assert_eq!(host_time_of(indefinite, host), None);
            assert_eq!(host_time_of(ffi::cm_time(1, 0), host), None, "no timescale");
            assert_eq!(host_time_of(ffi::cm_time(-5, 1), host), None, "before zero");
            assert_eq!(
                host_time_of(ffi::CMClockGetTime(host), std::ptr::null()),
                None
            );
        }
    }
}
