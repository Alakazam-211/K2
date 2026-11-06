//! The producer as a plain `extern "C"` API, for an app extension in Swift
//! (or any language) that links this crate as a static library. No C is
//! built: these are Rust functions with the C calling convention. The
//! declarations to put in a bridging header are in the README.
//!
//! Return codes: 0 written (or done), 1 dropped because the consumer
//! doesn't want frames, 2 dropped because it came too soon; below 0 an
//! error: -1 invalid argument, -2 no ring (the app isn't in a call), -3
//! another producer is attached, -4 not a matching ring, -5 an OS error,
//! -6 an internal error (a bug in this crate, caught at the boundary).
//!
//! **Threads:** a producer is not thread-safe. Make every call on one
//! producer from one serial queue (or otherwise one at a time), and call
//! [`kfr_producer_finish`] last, after every other call on it has
//! returned. ReplayKit may call `processSampleBuffer` and
//! `broadcastFinished` on different queues, so hop to one serial queue.
//!
//! **Panics:** no argument should make these functions panic, and each
//! one catches a panic and returns its error value instead of unwinding
//! into the caller. A build with `panic = "abort"` aborts instead.

use crate::{Color, EndReason, Nv12Frame, Producer, Pushed};
use std::ffi::{c_char, CStr};
use std::os::unix::ffi::OsStrExt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

/// Written (or done).
pub const KFR_OK: i32 = 0;
/// Dropped: the consumer doesn't want frames now (or is away).
pub const KFR_NOT_WANTED: i32 = 1;
/// Dropped: sooner than the ring's rate allows.
pub const KFR_TOO_SOON: i32 = 2;
/// A bad argument.
pub const KFR_ERR_INVALID: i32 = -1;
/// An internal error: a bug in this crate, caught before it reached the
/// caller.
pub const KFR_ERR_INTERNAL: i32 = -6;

/// Run `f`, turning a panic into `fallback`: a panic must never unwind
/// into C or Swift.
fn guard<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

/// Attach to the ring at `path` (a NUL-terminated file path). On failure
/// returns null and, if `error` isn't null, writes the code there.
///
/// # Safety
///
/// `path` must be null or a NUL-terminated string; `error` null or valid
/// for one write.
#[no_mangle]
pub unsafe extern "C" fn kfr_producer_attach(
    path: *const c_char,
    error: *mut i32,
) -> *mut Producer {
    let (p, code) = guard((std::ptr::null_mut(), KFR_ERR_INTERNAL), || {
        if path.is_null() {
            return (std::ptr::null_mut(), KFR_ERR_INVALID);
        }
        // SAFETY: NUL-terminated (the caller's promise).
        let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
        let path = Path::new(std::ffi::OsStr::from_bytes(bytes));
        match Producer::attach(path) {
            Ok(p) => (Box::into_raw(Box::new(p)), KFR_OK),
            Err(e) => (std::ptr::null_mut(), e.code()),
        }
    });
    if !error.is_null() {
        // SAFETY: valid for one write (the caller's promise).
        unsafe { *error = code };
    }
    p
}

/// Say the frames are starting. Call once, before the first push.
///
/// # Safety
///
/// `p` must be null or come from [`kfr_producer_attach`] and not be
/// finished, with no other call on it running (see the module's threads).
#[no_mangle]
pub unsafe extern "C" fn kfr_producer_start(p: *mut Producer) -> i32 {
    // SAFETY: the caller's promise.
    let Some(p) = (unsafe { p.as_ref() }) else {
        return KFR_ERR_INVALID;
    };
    guard(KFR_ERR_INTERNAL, || {
        p.start();
        KFR_OK
    })
}

/// Push one NV12 frame: the Y plane (`y_len` bytes, rows `y_stride` apart)
/// and the interleaved UV plane, as a locked `CVPixelBuffer`'s planes 0 and
/// 1 give them (`y_len` = bytes per row x height of plane 0). `rotation`
/// is how far it must turn clockwise to stand upright (0, 90, 180, 270);
/// `color` 0 BT.601 limited, 1 BT.709 limited (`420v`), 2 BT.709 full
/// (`420f`); `timestamp_ns` on [`kfr_monotonic_ns`]'s clock, or 0 for now.
/// Frames larger than the ring's box are scaled down. Never blocks. Returns
/// 0, 1 or 2 (see the module), or an error: -1 for any argument that
/// doesn't describe a frame (sizes, strides, lengths, rotation, colour).
///
/// # Safety
///
/// `p` null or from [`kfr_producer_attach`], not finished, with no other
/// call on it running; `y` and `uv` valid for reads of `y_len` and
/// `uv_len` bytes for the call.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn kfr_producer_push_nv12(
    p: *mut Producer,
    y: *const u8,
    y_len: usize,
    y_stride: usize,
    uv: *const u8,
    uv_len: usize,
    uv_stride: usize,
    width: u32,
    height: u32,
    rotation: u32,
    color: u32,
    timestamp_ns: u64,
) -> i32 {
    // SAFETY: the caller's promise.
    let Some(p) = (unsafe { p.as_mut() }) else {
        return KFR_ERR_INVALID;
    };
    let Some(color) = Color::from_u32(color) else {
        return KFR_ERR_INVALID;
    };
    // No buffer is longer than isize::MAX bytes; a slice may not be either.
    let too_long = |len: usize| isize::try_from(len).is_err();
    if y.is_null() || uv.is_null() || too_long(y_len) || too_long(uv_len) {
        return KFR_ERR_INVALID;
    }
    guard(KFR_ERR_INTERNAL, || {
        // SAFETY: valid for these reads during the call (the caller's
        // promise), and each length is at most isize::MAX.
        let (y, uv) = unsafe {
            (
                std::slice::from_raw_parts(y, y_len),
                std::slice::from_raw_parts(uv, uv_len),
            )
        };
        let mut f = Nv12Frame::new(y, y_stride, uv, uv_stride, width, height);
        f.rotation = rotation;
        f.color = color;
        f.timestamp_ns = timestamp_ns;
        match p.push(&f) {
            Ok(Pushed::Written { .. }) => KFR_OK,
            Ok(Pushed::NotWanted) => KFR_NOT_WANTED,
            Ok(Pushed::TooSoon) => KFR_TOO_SOON,
            Err(e) => e.code(),
        }
    })
}

/// Does the app want frames now (in a call, looking, not suspended)? When
/// false, skip locking and copying pixel buffers.
///
/// # Safety
///
/// `p` null or from [`kfr_producer_attach`], not finished, with no other
/// call on it running.
#[no_mangle]
pub unsafe extern "C" fn kfr_producer_wants_frames(p: *const Producer) -> bool {
    // SAFETY: the caller's promise.
    let Some(p) = (unsafe { p.as_ref() }) else {
        return false;
    };
    guard(false, || p.wants_frames())
}

/// Has the app asked the broadcast to stop (the call ended)? Then finish
/// the broadcast (`finishBroadcastWithError`) and call
/// [`kfr_producer_finish`].
///
/// # Safety
///
/// `p` null or from [`kfr_producer_attach`], not finished, with no other
/// call on it running.
#[no_mangle]
pub unsafe extern "C" fn kfr_producer_stop_requested(p: *const Producer) -> bool {
    // SAFETY: the caller's promise.
    let Some(p) = (unsafe { p.as_ref() }) else {
        return false;
    };
    guard(false, || p.stop_requested())
}

/// Stop for good and free the producer: `reason` 1 the person stopped
/// (`broadcastFinished`), 2 closed for another reason, 3 failed. The app
/// sees the reason. `p` is invalid afterwards.
///
/// # Safety
///
/// `p` from [`kfr_producer_attach`], finished at most once (null is
/// ignored), after every other call on it has returned.
#[no_mangle]
pub unsafe extern "C" fn kfr_producer_finish(p: *mut Producer, reason: u32) {
    if p.is_null() {
        return;
    }
    // SAFETY: made by Box::into_raw in attach, freed once (the caller's
    // promise).
    let p = unsafe { Box::from_raw(p) };
    guard((), || {
        p.finish(EndReason::from_u32(reason).unwrap_or(EndReason::SourceClosed))
    });
}

/// Nanoseconds on the clock frame timestamps use (mach absolute time on
/// Apple, the same as a `CMSampleBuffer`'s host-clock presentation time).
#[no_mangle]
pub extern "C" fn kfr_monotonic_ns() -> u64 {
    guard(0, crate::monotonic_ns)
}
