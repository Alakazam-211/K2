//! macOS and iOS: the QoS class, Mach's time-constraint policy and OS
//! workgroups, through libSystem and AudioToolbox. Declarations only: no C
//! is built.
//!
//! `os_workgroup_join`, `os_workgroup_leave`, the `os_workgroup_interval_*`
//! calls and AudioToolbox's `AudioWorkIntervalCreate` are new in macOS 11
//! and iOS 14, later than the oldest systems Rust builds for, so they are
//! looked up at run time (`dlsym`; AudioToolbox is `dlopen`ed, since a build
//! without an audio backend doesn't link it) rather than linked: on an older
//! system there is no workgroup to join, and the thread stays on the
//! time-constraint policy.
#![allow(unsafe_code)]

use super::{DspTimeConstraint, SchedOs};
use std::cell::UnsafeCell;
use std::ffi::{c_char, c_int, c_void};
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
    fn mach_absolute_time() -> u64;
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

/// `OS_CLOCK_MACH_ABSOLUTE_TIME` (`<os/clock.h>`): the clock a work
/// interval's timestamps are in.
pub(crate) const OS_CLOCK_MACH_ABSOLUTE_TIME: u32 = 32;

/// AudioToolbox, where `AudioWorkIntervalCreate` lives.
const AUDIO_TOOLBOX: &std::ffi::CStr =
    c"/System/Library/Frameworks/AudioToolbox.framework/AudioToolbox";

type CreateFn =
    unsafe extern "C" fn(name: *const c_char, clock: u32, attr: *mut c_void) -> *mut c_void;
type StartFn =
    unsafe extern "C" fn(wg: *mut c_void, start: u64, deadline: u64, data: *mut c_void) -> c_int;
type UpdateFn = unsafe extern "C" fn(wg: *mut c_void, deadline: u64, data: *mut c_void) -> c_int;
type FinishFn = unsafe extern "C" fn(wg: *mut c_void, data: *mut c_void) -> c_int;

/// The work interval calls, if this system has all of them.
#[derive(Clone, Copy)]
struct IntervalFns {
    create: CreateFn,
    start: StartFn,
    update: UpdateFn,
    finish: FinishFn,
}

fn interval_fns() -> Option<IntervalFns> {
    static FNS: OnceLock<Option<IntervalFns>> = OnceLock::new();
    *FNS.get_or_init(|| {
        // SAFETY: dlopen of a system framework by its absolute path with a
        // NUL-terminated name; the handle is kept for the process's life
        // (never dlclosed), so the symbols looked up through it stay valid.
        // dlsym with NUL-terminated names only looks symbols up; a null
        // result means the system lacks them.
        let (create, start, update, finish) = unsafe {
            let toolbox = libc::dlopen(AUDIO_TOOLBOX.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL);
            if toolbox.is_null() {
                return None;
            }
            (
                libc::dlsym(toolbox, c"AudioWorkIntervalCreate".as_ptr()),
                libc::dlsym(libc::RTLD_DEFAULT, c"os_workgroup_interval_start".as_ptr()),
                libc::dlsym(libc::RTLD_DEFAULT, c"os_workgroup_interval_update".as_ptr()),
                libc::dlsym(libc::RTLD_DEFAULT, c"os_workgroup_interval_finish".as_ptr()),
            )
        };
        if create.is_null() || start.is_null() || update.is_null() || finish.is_null() {
            return None;
        }
        // SAFETY: the symbols are these C functions (AudioToolbox's
        // `<AudioToolbox/AudioWorkInterval.h>`: `os_workgroup_interval_t
        // AudioWorkIntervalCreate(const char *name, os_clockid_t clock,
        // os_workgroup_attr_t attr)`; libSystem's
        // `<os/workgroup_interval.h>`: `int
        // os_workgroup_interval_start(os_workgroup_interval_t, uint64_t
        // start, uint64_t deadline, os_workgroup_interval_data_t)`, `int
        // os_workgroup_interval_update(os_workgroup_interval_t, uint64_t
        // deadline, os_workgroup_interval_data_t)` and `int
        // os_workgroup_interval_finish(os_workgroup_interval_t,
        // os_workgroup_interval_data_t)`); `os_clockid_t` is a `uint32_t`
        // enum, and the workgroup, attribute and data types are pointers.
        unsafe {
            Some(IntervalFns {
                create: std::mem::transmute::<*mut c_void, CreateFn>(create),
                start: std::mem::transmute::<*mut c_void, StartFn>(start),
                update: std::mem::transmute::<*mut c_void, UpdateFn>(update),
                finish: std::mem::transmute::<*mut c_void, FinishFn>(finish),
            })
        }
    })
}

/// The timebase, read once.
fn timebase() -> Option<MachTimebaseInfo> {
    static TB: OnceLock<Option<(u32, u32)>> = OnceLock::new();
    let (numer, denom) = (*TB.get_or_init(|| {
        let mut tb = MachTimebaseInfo { numer: 0, denom: 0 };
        // SAFETY: `tb` is valid for writes of one `mach_timebase_info_data_t`.
        let ok = unsafe { mach_timebase_info(&mut tb) } == libc::KERN_SUCCESS;
        (ok && tb.numer != 0 && tb.denom != 0).then_some((tb.numer, tb.denom))
    }))?;
    Some(MachTimebaseInfo { numer, denom })
}

/// `deadline` from now in Mach absolute time, as `(now, now + deadline)`.
fn now_and_deadline(deadline: Duration) -> Option<(u64, u64)> {
    let tb = timebase()?;
    let ticks = u64::try_from(deadline.as_nanos() * u128::from(tb.denom) / u128::from(tb.numer))
        .ok()
        // The deadline must be after the start.
        .filter(|&t| t > 0)?;
    // SAFETY: takes no arguments; reads the clock. Real-time safe.
    let now = unsafe { mach_absolute_time() };
    Some((now, now.checked_add(ticks)?))
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

/// The calling thread's scheduling, through libSystem and AudioToolbox.
pub(crate) struct AppleOs;

impl AppleOs {
    pub(crate) fn new() -> Self {
        // Look the workgroup calls and the timebase up now, as the thread
        // starts, not when the first device opens or the first block runs.
        let _ = workgroup_fns();
        let _ = interval_fns();
        let _ = timebase();
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
    params: DspTimeConstraint,
) -> Option<libc::thread_time_constraint_policy> {
    let tb = timebase()?;
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

    fn set_time_constraint(&mut self, params: DspTimeConstraint) -> bool {
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

    fn create_interval(&mut self) -> Option<Box<Workgroup>> {
        // The interval calls must all be there, or an interval is no use.
        let fns = interval_fns()?;
        // SAFETY: a NUL-terminated name, a valid clock id and no attributes
        // (null is the default); the result is null or a +1
        // `os_workgroup_interval_t` (an `os_workgroup_t`; the header marks it
        // OS_WORKGROUP_RETURNS_RETAINED), which `from_retained` takes over
        // and releases.
        unsafe {
            let wg = (fns.create)(
                c"kessel-audio-dsp".as_ptr(),
                OS_CLOCK_MACH_ABSOLUTE_TIME,
                std::ptr::null_mut(),
            );
            Workgroup::from_retained(wg).map(Box::new)
        }
    }

    // `DspSched` calls the three interval calls only while this thread is a
    // member of `wg` (`<os/workgroup_interval.h>`: a call from a
    // non-member returns an error, which would count as a refusal), and only
    // from this one thread.

    fn interval_start(&mut self, wg: &Workgroup, deadline: Duration) -> bool {
        let (Some(fns), Some((now, due))) = (interval_fns(), now_and_deadline(deadline)) else {
            return false;
        };
        // SAFETY: `wg.wg` is a live work interval (made by `create_interval`;
        // `Workgroup` holds a reference) that this thread has joined; both
        // timestamps are on its clock (Mach absolute time), `due` after
        // `now`; no interval data (null, as the header asks). Real-time safe.
        unsafe { (fns.start)(wg.wg.as_ptr(), now, due, std::ptr::null_mut()) == 0 }
    }

    fn interval_update(&mut self, wg: &Workgroup, deadline: Duration) -> bool {
        let (Some(fns), Some((_, due))) = (interval_fns(), now_and_deadline(deadline)) else {
            return false;
        };
        // SAFETY: as in `interval_start`, for the interval this thread
        // started on `wg`.
        unsafe { (fns.update)(wg.wg.as_ptr(), due, std::ptr::null_mut()) == 0 }
    }

    fn interval_finish(&mut self, wg: &Workgroup) -> bool {
        let Some(fns) = interval_fns() else {
            return false;
        };
        // SAFETY: as in `interval_start`, for the interval this thread
        // started on `wg`.
        unsafe { (fns.finish)(wg.wg.as_ptr(), std::ptr::null_mut()) == 0 }
    }
}
