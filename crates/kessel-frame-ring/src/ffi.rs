//! The producer as a plain `extern "C"` API, for an app extension in Swift
//! (or any language) that links this crate as a static library. No C is
//! built: these are Rust functions with the C calling convention. The
//! declarations to put in a bridging header are in the README.
//!
//! Return codes: 0 written (or done), 1 dropped because the consumer
//! doesn't want frames, 2 dropped because it came too soon; below 0 an
//! error: -1 invalid argument, -2 no ring (the app isn't in a call), -3
//! another producer is attached, -4 not a matching ring, -5 an OS error.

use crate::{Color, EndReason, Nv12Frame, Producer, Pushed};
use std::ffi::{c_char, CStr};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Written (or done).
pub const KFR_OK: i32 = 0;
/// Dropped: the consumer doesn't want frames now (or is away).
pub const KFR_NOT_WANTED: i32 = 1;
/// Dropped: sooner than the ring's rate allows.
pub const KFR_TOO_SOON: i32 = 2;
/// A bad argument.
pub const KFR_ERR_INVALID: i32 = -1;

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
    let fail = |code: i32| {
        if !error.is_null() {
            // SAFETY: valid for one write (the caller's promise).
            unsafe { *error = code };
        }
        std::ptr::null_mut()
    };
    if path.is_null() {
        return fail(KFR_ERR_INVALID);
    }
    // SAFETY: NUL-terminated (the caller's promise).
    let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
    let path = Path::new(std::ffi::OsStr::from_bytes(bytes));
    match Producer::attach(path) {
        Ok(p) => {
            if !error.is_null() {
                // SAFETY: as above.
                unsafe { *error = KFR_OK };
            }
            Box::into_raw(Box::new(p))
        }
        Err(e) => fail(e.code()),
    }
}

/// Say the frames are starting. Call once, before the first push.
///
/// # Safety
///
/// `p` must come from [`kfr_producer_attach`] and not be finished.
#[no_mangle]
pub unsafe extern "C" fn kfr_producer_start(p: *mut Producer) -> i32 {
    // SAFETY: the caller's promise.
    let Some(p) = (unsafe { p.as_ref() }) else {
        return KFR_ERR_INVALID;
    };
    p.start();
    KFR_OK
}

/// Push one NV12 frame: the Y plane (`y_len` bytes, rows `y_stride` apart)
/// and the interleaved UV plane, as a locked `CVPixelBuffer`'s planes 0 and
/// 1 give them (`y_len` = bytes per row x height of plane 0). `rotation`
/// is how far it must turn clockwise to stand upright (0, 90, 180, 270);
/// `color` 0 BT.601 limited, 1 BT.709 limited (`420v`), 2 BT.709 full
/// (`420f`); `timestamp_ns` on [`kfr_monotonic_ns`]'s clock, or 0 for now.
/// Frames larger than the ring's box are scaled down. Never blocks. Returns
/// 0, 1 or 2 (see the module), or an error.
///
/// # Safety
///
/// `p` from [`kfr_producer_attach`], not finished; `y` and `uv` valid for
/// reads of `y_len` and `uv_len` bytes for the call.
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
    if y.is_null() || uv.is_null() {
        return KFR_ERR_INVALID;
    }
    // SAFETY: valid for these reads during the call (the caller's promise).
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
}

/// Does the app want frames now (in a call, looking, not suspended)? When
/// false, skip locking and copying pixel buffers.
///
/// # Safety
///
/// `p` from [`kfr_producer_attach`], not finished.
#[no_mangle]
pub unsafe extern "C" fn kfr_producer_wants_frames(p: *const Producer) -> bool {
    // SAFETY: the caller's promise.
    unsafe { p.as_ref() }.is_some_and(Producer::wants_frames)
}

/// Has the app asked the broadcast to stop (the call ended)? Then finish
/// the broadcast (`finishBroadcastWithError`) and call
/// [`kfr_producer_finish`].
///
/// # Safety
///
/// `p` from [`kfr_producer_attach`], not finished.
#[no_mangle]
pub unsafe extern "C" fn kfr_producer_stop_requested(p: *const Producer) -> bool {
    // SAFETY: the caller's promise.
    unsafe { p.as_ref() }.is_some_and(Producer::stop_requested)
}

/// Stop for good and free the producer: `reason` 1 the person stopped
/// (`broadcastFinished`), 2 closed for another reason, 3 failed. The app
/// sees the reason. `p` is invalid afterwards.
///
/// # Safety
///
/// `p` from [`kfr_producer_attach`], finished at most once (null is
/// ignored).
#[no_mangle]
pub unsafe extern "C" fn kfr_producer_finish(p: *mut Producer, reason: u32) {
    if p.is_null() {
        return;
    }
    // SAFETY: made by Box::into_raw in attach, freed once (the caller's
    // promise).
    let p = unsafe { Box::from_raw(p) };
    p.finish(EndReason::from_u32(reason).unwrap_or(EndReason::SourceClosed));
}

/// Nanoseconds on the clock frame timestamps use (mach absolute time on
/// Apple, the same as a `CMSampleBuffer`'s host-clock presentation time).
#[no_mangle]
pub extern "C" fn kfr_monotonic_ns() -> u64 {
    crate::monotonic_ns()
}
