//! macOS and iOS: the QoS class, Mach's time-constraint policy and OS
//! workgroups, through libSystem. Declarations only: no C is built.
//!
//! `os_workgroup_join` and `os_workgroup_leave` are new in macOS 11 and
//! iOS 14, later than the oldest systems Rust builds for, so they are looked
//! up at run time (`dlsym`) rather than linked: on an older system there is
//! no workgroup to join, and the thread stays on the time-constraint policy.
#![allow(unsafe_code)]

use super::{RtParams, SchedOs};
use std::cell::UnsafeCell;
use std::ffi::{c_int, c_void};
use std::ptr::NonNull;
use std::sync::OnceLock;
use std::time::Duration;

/// `QOS_CLASS_USER_INTERACTIVE` (`<sys/qos.h>`).
pub(crate) const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;

/// `mach_timebase_info_data_t` (`<mach/mach_time.h>`).
#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

extern "C" {
    // `qos_class_t` is an `unsigned int` in C.
    fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: c_int) -> c_int;
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> c_int;
    // In libSystem since macOS 10.10 and iOS 8.
    fn os_release(object: *mut c_void);
}

/// `os_workgroup_join_token_s` (`<os/workgroup_base.h>`): a `uint32_t` and
/// 36 opaque bytes on 64-bit systems (40 bytes). Room for 64, aligned
/// for any of its fields, in case a later system grows it.
#[repr(C, align(8))]
struct JoinToken {
    _bytes: [u8; 64],
}

type JoinFn = unsafe extern "C" fn(wg: *mut c_void, token_out: *mut JoinToken) -> c_int;
type LeaveFn = unsafe extern "C" fn(wg: *mut c_void, token: *mut JoinToken);

/// The workgroup calls, if this system has them.
#[derive(Clone, Copy)]
struct WorkgroupFns {
    join: JoinFn,
    leave: LeaveFn,
}

fn workgroup_fns() -> Option<WorkgroupFns> {
    static FNS: OnceLock<Option<WorkgroupFns>> = OnceLock::new();
    *FNS.get_or_init(|| {
        // SAFETY: dlsym with RTLD_DEFAULT and NUL-terminated names only
        // looks symbols up; a null result means the system lacks them.
        let (join, leave) = unsafe {
            (
                libc::dlsym(libc::RTLD_DEFAULT, c"os_workgroup_join".as_ptr()),
                libc::dlsym(libc::RTLD_DEFAULT, c"os_workgroup_leave".as_ptr()),
            )
        };
        if join.is_null() || leave.is_null() {
            return None;
        }
        // SAFETY: both symbols are libSystem's functions with these C
        // signatures (`<os/workgroup_object.h>`: `int
        // os_workgroup_join(os_workgroup_t, os_workgroup_join_token_t)` and
        // `void os_workgroup_leave(os_workgroup_t,
        // os_workgroup_join_token_t)`); `os_workgroup_t` is a pointer and the
        // token a pointer to the client's token struct.
        unsafe {
            Some(WorkgroupFns {
                join: std::mem::transmute::<*mut c_void, JoinFn>(join),
                leave: std::mem::transmute::<*mut c_void, LeaveFn>(leave),
            })
        }
    })
}

/// An OS workgroup (`os_workgroup_t`) this holds a reference to, with room
/// for one join token. It is boxed when handed to the processing thread, so
/// the token stays put between the join and the leave.
pub(crate) struct Workgroup {
    wg: NonNull<c_void>,
    token: UnsafeCell<JoinToken>,
}

// SAFETY: an os_workgroup is a reference-counted OS object that may be used
// and released from any thread. The token is only touched by the thread
// that joins and leaves (the processing thread; `DspSched` is `!Send`), and
// `UnsafeCell` keeps `Workgroup` `!Sync`.
unsafe impl Send for Workgroup {}

impl Workgroup {
    /// Take over a +1 reference to an `os_workgroup_t` (what
    /// `AudioUnitGetProperty(kAudioOutputUnitProperty_OSWorkgroup)` returns).
    /// `None` for a null pointer.
    ///
    /// # Safety
    ///
    /// `wg` is null or a valid `os_workgroup_t` whose reference the caller
    /// owns and gives up; it is released when this is dropped.
    #[cfg_attr(
        not(any(test, all(feature = "os-voice", target_os = "ios"))),
        allow(dead_code)
    )]
    pub(crate) unsafe fn from_retained(wg: *mut c_void) -> Option<Workgroup> {
        Some(Workgroup {
            wg: NonNull::new(wg)?,
            token: UnsafeCell::new(JoinToken { _bytes: [0; 64] }),
        })
    }

    /// The `os_workgroup_t`.
    #[cfg(test)]
    pub(crate) fn as_ptr(&self) -> *mut c_void {
        self.wg.as_ptr()
    }
}

impl Drop for Workgroup {
    fn drop(&mut self) {
        // SAFETY: `wg` holds the reference `from_retained` took over, released
        // once, here.
        unsafe { os_release(self.wg.as_ptr()) }
    }
}

/// The calling thread's scheduling, through libSystem.
pub(crate) struct AppleOs;

impl AppleOs {
    pub(crate) fn new() -> Self {
        // Look the workgroup calls up now, as the thread starts, not when
        // the first device opens.
        let _ = workgroup_fns();
        AppleOs
    }
}

/// `d` in Mach absolute-time units, or `None` if it doesn't fit a `u32`.
fn mach_ticks(d: Duration, tb: &MachTimebaseInfo) -> Option<u32> {
    if tb.numer == 0 || tb.denom == 0 {
        return None;
    }
    u32::try_from(d.as_nanos() * u128::from(tb.denom) / u128::from(tb.numer)).ok()
}

/// The time-constraint policy for `params` on this machine's timebase.
pub(crate) fn time_constraint_policy(
    params: RtParams,
) -> Option<libc::thread_time_constraint_policy> {
    let mut tb = MachTimebaseInfo { numer: 0, denom: 0 };
    // SAFETY: `tb` is valid for writes of one `mach_timebase_info_data_t`.
    if unsafe { mach_timebase_info(&mut tb) } != libc::KERN_SUCCESS {
        return None;
    }
    Some(libc::thread_time_constraint_policy {
        period: mach_ticks(params.period, &tb)?,
        computation: mach_ticks(params.computation, &tb)?,
        constraint: mach_ticks(params.constraint, &tb)?,
        preemptible: 1,
    })
}

impl SchedOs for AppleOs {
    type Workgroup = Workgroup;
    type Token = ();

    fn set_qos(&mut self) -> bool {
        // SAFETY: changes only the calling thread's QoS class; takes no
        // pointers.
        unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) == 0 }
    }

    fn set_time_constraint(&mut self, params: RtParams) -> bool {
        let Some(mut policy) = time_constraint_policy(params) else {
            return false;
        };
        // SAFETY: `pthread_mach_thread_np(pthread_self())` is the calling
        // thread's port (no new reference to release); `policy` is a valid
        // `thread_time_constraint_policy_data_t`, which is
        // THREAD_TIME_CONSTRAINT_POLICY_COUNT integers, and is only read.
        let r = unsafe {
            libc::thread_policy_set(
                libc::pthread_mach_thread_np(libc::pthread_self()),
                libc::THREAD_TIME_CONSTRAINT_POLICY as libc::thread_policy_flavor_t,
                (&mut policy as *mut libc::thread_time_constraint_policy).cast(),
                libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT,
            )
        };
        r == libc::KERN_SUCCESS
    }

    fn join(&mut self, wg: &Workgroup) -> Option<()> {
        let fns = workgroup_fns()?;
        // SAFETY: `wg.wg` is a live workgroup (`Workgroup` holds a
        // reference); the token is `Workgroup`'s own, large enough for
        // `os_workgroup_join_token_s`, and only this thread touches it (see
        // the `Send` impl). The call is real-time safe.
        let r = unsafe { (fns.join)(wg.wg.as_ptr(), wg.token.get()) };
        (r == 0).then_some(())
    }

    fn leave(&mut self, wg: &Workgroup, _: ()) {
        // A join succeeded, so the calls exist.
        if let Some(fns) = workgroup_fns() {
            // SAFETY: this thread joined `wg` with the token in `wg.token`
            // (`DspSched` only leaves what it joined, on the joining thread),
            // and `wg` is still live. Real-time safe.
            unsafe { (fns.leave)(wg.wg.as_ptr(), wg.token.get()) }
        }
    }
}
