//! One running Camera2 capture: an `AImageReader` (YUV_420_888) as the
//! only output of a repeating `TEMPLATE_RECORD` request. Images arrive on
//! the reader's thread and go to the backend's sink.

use super::ffi::*;
use crate::android::image::{Plane, Yuv420};
use crate::android::meta::{camera_result, device_error, media_result, Lost, FORMAT_YUV_420_888};
use crate::android::{Capture, ImageSink};
use crate::error::VideoError;
use crate::types::CaptureMode;
use std::ffi::{c_int, c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

/// What the NDK's callbacks get as their context.
struct CallbackCtx {
    sink: Arc<dyn ImageSink>,
}

/// The reader holds three images: the one being read and two in flight.
const MAX_IMAGES: i32 = 3;

/// A running capture. Every field is null until made; drop releases what
/// was made, in the order the NDK needs.
pub(crate) struct NdkCapture {
    reader: *mut AImageReader,
    device: *mut ACameraDevice,
    output: *mut ACaptureSessionOutput,
    container: *mut ACaptureSessionOutputContainer,
    target: *mut ACameraOutputTarget,
    request: *mut ACaptureRequest,
    session: *mut ACameraCaptureSession,
    /// Boxed so their addresses stay put while the NDK holds them.
    listener: Box<AImageReader_ImageListener>,
    device_callbacks: Box<ACameraDevice_StateCallbacks>,
    session_callbacks: Box<ACameraCaptureSession_stateCallbacks>,
    /// The callbacks' context, freed after the reader and device are gone.
    ctx: *mut CallbackCtx,
}

// SAFETY: the handles are used only from the thread that owns the capture
// (open and drop); the NDK's own threads reach only `ctx`, whose sink is
// Send + Sync.
unsafe impl Send for NdkCapture {}

impl Capture for NdkCapture {}

impl Drop for NdkCapture {
    fn drop(&mut self) {
        // SAFETY: each handle was made by `open` (or is null) and is
        // released once, session before device before reader. Closing the
        // device and deleting the reader stop their callback threads, so
        // `ctx` is unused when it is freed last.
        unsafe {
            if !self.session.is_null() {
                ACameraCaptureSession_stopRepeating(self.session);
                ACameraCaptureSession_close(self.session);
            }
            if !self.device.is_null() {
                ACameraDevice_close(self.device);
            }
            if !self.request.is_null() {
                ACaptureRequest_free(self.request);
            }
            if !self.target.is_null() {
                ACameraOutputTarget_free(self.target);
            }
            if !self.container.is_null() {
                ACaptureSessionOutputContainer_free(self.container);
            }
            if !self.output.is_null() {
                ACaptureSessionOutput_free(self.output);
            }
            if !self.reader.is_null() {
                AImageReader_setImageListener(self.reader, std::ptr::null_mut());
                AImageReader_delete(self.reader);
            }
            drop(Box::from_raw(self.ctx));
        }
    }
}

/// Deletes an acquired image.
struct ImageGuard(*mut AImage);

impl Drop for ImageGuard {
    fn drop(&mut self) {
        // SAFETY: acquired from the reader, deleted once.
        unsafe { AImage_delete(self.0) };
    }
}

/// Read one image's planes and hand them to the sink.
///
/// # Safety
///
/// `img` must be a live image of the reader.
unsafe fn read_image(img: *const AImage, sink: &dyn ImageSink) -> Result<(), VideoError> {
    let (mut w, mut h) = (0i32, 0i32);
    // SAFETY: `img` is live (the caller's promise); the out pointers are
    // valid.
    unsafe {
        media_result(AImage_getWidth(img, &mut w), "AImage_getWidth")?;
        media_result(AImage_getHeight(img, &mut h), "AImage_getHeight")?;
    }
    let mut planes: [Plane<'_>; 3] = [Plane {
        data: &[],
        row_stride: 0,
        pixel_stride: 0,
    }; 3];
    for (i, plane) in planes.iter_mut().enumerate() {
        let (mut data, mut len, mut rs, mut ps) = (std::ptr::null_mut(), 0 as c_int, 0i32, 0i32);
        // SAFETY: as above; `i` is 0 to 2, the planes of a YUV_420_888
        // image.
        unsafe {
            media_result(
                AImage_getPlaneData(img, i as c_int, &mut data, &mut len),
                "AImage_getPlaneData",
            )?;
            media_result(
                AImage_getPlaneRowStride(img, i as c_int, &mut rs),
                "AImage_getPlaneRowStride",
            )?;
            media_result(
                AImage_getPlanePixelStride(img, i as c_int, &mut ps),
                "AImage_getPlanePixelStride",
            )?;
        }
        if data.is_null() || len <= 0 || rs <= 0 || ps <= 0 {
            return Err(VideoError::os(
                0,
                format!("camera image plane {i} is empty"),
            ));
        }
        // SAFETY: the NDK says `data` holds `len` bytes, valid until the
        // image is deleted, which is after the sink returns.
        let bytes = unsafe { std::slice::from_raw_parts(data, len as usize) };
        *plane = Plane {
            data: bytes,
            row_stride: rs as usize,
            pixel_stride: ps as usize,
        };
    }
    sink.image(&Yuv420 {
        width: w.max(0) as u32,
        height: h.max(0) as u32,
        planes,
    });
    Ok(())
}

unsafe extern "C" fn on_image(ctx: *mut c_void, reader: *mut AImageReader) {
    // An unwind can't cross into the NDK: catch it and drop the image.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `ctx` is the CallbackCtx set with this listener; it lives
        // until after the reader is deleted, which waits for this callback.
        let ctx = unsafe { &*(ctx as *const CallbackCtx) };
        let mut img = std::ptr::null_mut();
        // SAFETY: `reader` is the live reader calling us.
        let status = unsafe { AImageReader_acquireLatestImage(reader, &mut img) };
        if status != 0 || img.is_null() {
            return;
        }
        let guard = ImageGuard(img);
        if ctx.sink.idle() {
            // Released unread: the reader needs its buffers back.
            return;
        }
        // SAFETY: `guard` keeps the image live until after the read.
        if let Err(e) = unsafe { read_image(guard.0, &*ctx.sink) } {
            ctx.sink.error(e);
        }
    }));
}

unsafe extern "C" fn on_disconnected(ctx: *mut c_void, _device: *mut ACameraDevice) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the CallbackCtx given with these callbacks; it lives until
        // after the device is closed, which waits for this callback.
        let ctx = unsafe { &*(ctx as *const CallbackCtx) };
        ctx.sink.lost(Lost::Disconnected);
    }));
}

unsafe extern "C" fn on_error(ctx: *mut c_void, _device: *mut ACameraDevice, error: c_int) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: as in `on_disconnected`.
        let ctx = unsafe { &*(ctx as *const CallbackCtx) };
        ctx.sink.lost(device_error(error));
    }));
}

unsafe extern "C" fn on_session_state(_ctx: *mut c_void, _session: *mut ACameraCaptureSession) {}

/// Open camera `id` in `mode` and start it repeating into a reader.
pub(crate) fn open(
    manager: *mut ACameraManager,
    id: &CStr,
    mode: CaptureMode,
    fps_range: Option<(u32, u32)>,
    sink: Arc<dyn ImageSink>,
) -> Result<NdkCapture, VideoError> {
    let ctx = Box::into_raw(Box::new(CallbackCtx { sink })) as *mut c_void;
    let mut cap = NdkCapture {
        reader: std::ptr::null_mut(),
        device: std::ptr::null_mut(),
        output: std::ptr::null_mut(),
        container: std::ptr::null_mut(),
        target: std::ptr::null_mut(),
        request: std::ptr::null_mut(),
        session: std::ptr::null_mut(),
        listener: Box::new(AImageReader_ImageListener {
            context: ctx,
            onImageAvailable: Some(on_image),
        }),
        device_callbacks: Box::new(ACameraDevice_StateCallbacks {
            context: ctx,
            onDisconnected: Some(on_disconnected),
            onError: Some(on_error),
        }),
        session_callbacks: Box::new(ACameraCaptureSession_stateCallbacks {
            context: ctx,
            onClosed: Some(on_session_state),
            onReady: Some(on_session_state),
            onActive: Some(on_session_state),
        }),
        ctx: ctx as *mut CallbackCtx,
    };
    // From here an early return drops `cap`, which releases what was made.
    // SAFETY (the block): every pointer passed is live (made above, or the
    // caller's manager), every out pointer is a field of `cap`, and the
    // boxed callback structs outlive the handles that use them.
    unsafe {
        media_result(
            AImageReader_new(
                mode.width as i32,
                mode.height as i32,
                FORMAT_YUV_420_888,
                MAX_IMAGES,
                &mut cap.reader,
            ),
            "AImageReader_new",
        )?;
        media_result(
            AImageReader_setImageListener(cap.reader, &mut *cap.listener),
            "AImageReader_setImageListener",
        )?;
        let mut window: *mut ANativeWindow = std::ptr::null_mut();
        media_result(
            AImageReader_getWindow(cap.reader, &mut window),
            "AImageReader_getWindow",
        )?;
        camera_result(
            ACameraManager_openCamera(
                manager,
                id.as_ptr(),
                &mut *cap.device_callbacks,
                &mut cap.device,
            ),
            "ACameraManager_openCamera",
        )?;
        camera_result(
            ACaptureSessionOutputContainer_create(&mut cap.container),
            "ACaptureSessionOutputContainer_create",
        )?;
        camera_result(
            ACaptureSessionOutput_create(window, &mut cap.output),
            "ACaptureSessionOutput_create",
        )?;
        camera_result(
            ACaptureSessionOutputContainer_add(cap.container, cap.output),
            "ACaptureSessionOutputContainer_add",
        )?;
        camera_result(
            ACameraDevice_createCaptureRequest(cap.device, TEMPLATE_RECORD, &mut cap.request),
            "ACameraDevice_createCaptureRequest",
        )?;
        camera_result(
            ACameraOutputTarget_create(window, &mut cap.target),
            "ACameraOutputTarget_create",
        )?;
        camera_result(
            ACaptureRequest_addTarget(cap.request, cap.target),
            "ACaptureRequest_addTarget",
        )?;
        if let Some((lo, hi)) = fps_range {
            let range = [lo as i32, hi as i32];
            camera_result(
                ACaptureRequest_setEntry_i32(
                    cap.request,
                    ACAMERA_CONTROL_AE_TARGET_FPS_RANGE,
                    2,
                    range.as_ptr(),
                ),
                "ACaptureRequest_setEntry_i32 (AE target FPS range)",
            )?;
        }
        camera_result(
            ACameraDevice_createCaptureSession(
                cap.device,
                cap.container,
                &*cap.session_callbacks,
                &mut cap.session,
            ),
            "ACameraDevice_createCaptureSession",
        )?;
        camera_result(
            ACameraCaptureSession_setRepeatingRequest(
                cap.session,
                std::ptr::null_mut(),
                1,
                &mut cap.request,
                std::ptr::null_mut(),
            ),
            "ACameraCaptureSession_setRepeatingRequest",
        )?;
    }
    Ok(cap)
}
