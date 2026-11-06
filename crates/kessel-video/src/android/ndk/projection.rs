//! A MediaProjection's virtual display into an `AImageReader`: the reader's
//! window becomes a Java `Surface` (`ANativeWindow_toSurface`, API 26), the
//! host's projection makes a `VirtualDisplay` onto it through JNI, and
//! images arrive on the reader's thread. Resizing makes a new reader of the
//! new size, moves the display onto it (`resize`, `setSurface`), then
//! deletes the old one.

use super::ffi::*;
use super::jni::{Env, JObject, JValue};
use crate::android::meta::{media_result, FORMAT_YUV_420_888};
use crate::android::screen::ScreenImage;
use crate::android::{ProjectionCapture, ScreenSink};
use crate::error::{unsupported, VideoError};
use crate::yuv::{Plane, Yuv420};
use std::ffi::{c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

/// `AIMAGE_FORMAT_RGBA_8888` and `AIMAGE_FORMAT_RGBX_8888`.
const FORMAT_RGBA_8888: i32 = 1;
const FORMAT_RGBX_8888: i32 = 2;
/// `DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR`: mirror the screen.
const FLAG_AUTO_MIRROR: i32 = 16;
/// The reader holds three images: the one being read and two in flight.
const MAX_IMAGES: i32 = 3;

const CREATE: &std::ffi::CStr = c"createVirtualDisplay";
const CREATE_SIG: &std::ffi::CStr = c"(Ljava/lang/String;IIIILandroid/view/Surface;Landroid/hardware/display/VirtualDisplay$Callback;Landroid/os/Handler;)Landroid/hardware/display/VirtualDisplay;";

struct CallbackCtx {
    sink: Arc<dyn ScreenSink>,
}

/// One `AImageReader` with its listener. Dropping it stops the callbacks
/// (the NDK waits for one in flight) and frees their context.
struct Reader {
    reader: *mut AImageReader,
    listener: Box<AImageReader_ImageListener>,
    ctx: *mut CallbackCtx,
}

impl Reader {
    fn new(width: u32, height: u32, sink: Arc<dyn ScreenSink>) -> Result<Reader, VideoError> {
        let ctx = Box::into_raw(Box::new(CallbackCtx { sink }));
        let mut r = Reader {
            reader: std::ptr::null_mut(),
            listener: Box::new(AImageReader_ImageListener {
                context: ctx as *mut c_void,
                onImageAvailable: Some(on_image),
            }),
            ctx,
        };
        // SAFETY: valid sizes and out pointer; the boxed listener outlives
        // the reader (dropped after it in `Drop`). An early return drops
        // `r`, which releases what was made.
        unsafe {
            media_result(
                AImageReader_new(
                    width as i32,
                    height as i32,
                    FORMAT_RGBA_8888,
                    MAX_IMAGES,
                    &mut r.reader,
                ),
                "AImageReader_new (RGBA_8888)",
            )?;
            media_result(
                AImageReader_setImageListener(r.reader, &mut *r.listener),
                "AImageReader_setImageListener",
            )?;
        }
        Ok(r)
    }

    fn window(&self) -> Result<*mut ANativeWindow, VideoError> {
        let mut window: *mut ANativeWindow = std::ptr::null_mut();
        // SAFETY: a live reader and a valid out pointer; the window lives
        // as long as the reader.
        unsafe {
            media_result(
                AImageReader_getWindow(self.reader, &mut window),
                "AImageReader_getWindow",
            )?;
        }
        Ok(window)
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: made in `new` (or null), released once; deleting the
        // reader waits for its callback thread, so `ctx` is unused when it
        // is freed.
        unsafe {
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

/// One plane of `img`: its bytes, row stride and pixel stride.
///
/// # Safety
///
/// `img` must be a live image; the slice lives until it is deleted.
unsafe fn plane<'a>(img: *const AImage, i: c_int) -> Result<Plane<'a>, VideoError> {
    let (mut data, mut len, mut rs, mut ps) = (std::ptr::null_mut(), 0 as c_int, 0i32, 0i32);
    // SAFETY: `img` is live; the out pointers are valid.
    unsafe {
        media_result(
            AImage_getPlaneData(img, i, &mut data, &mut len),
            "AImage_getPlaneData",
        )?;
        media_result(
            AImage_getPlaneRowStride(img, i, &mut rs),
            "AImage_getPlaneRowStride",
        )?;
        media_result(
            AImage_getPlanePixelStride(img, i, &mut ps),
            "AImage_getPlanePixelStride",
        )?;
    }
    if data.is_null() || len <= 0 || rs <= 0 || ps <= 0 {
        return Err(VideoError::os(
            0,
            format!("screen image plane {i} is empty"),
        ));
    }
    Ok(Plane {
        // SAFETY: the NDK says `data` holds `len` bytes, valid until the
        // image is deleted (the caller's promise).
        data: unsafe { std::slice::from_raw_parts(data, len as usize) },
        row_stride: rs as usize,
        pixel_stride: ps as usize,
    })
}

/// Read one image and hand it to the sink.
///
/// # Safety
///
/// `img` must be a live image of the reader.
unsafe fn read_image(img: *const AImage, sink: &dyn ScreenSink) -> Result<(), VideoError> {
    let (mut w, mut h, mut format) = (0i32, 0i32, 0i32);
    // SAFETY: `img` is live; the out pointers are valid.
    unsafe {
        media_result(AImage_getWidth(img, &mut w), "AImage_getWidth")?;
        media_result(AImage_getHeight(img, &mut h), "AImage_getHeight")?;
        media_result(AImage_getFormat(img, &mut format), "AImage_getFormat")?;
    }
    let (width, height) = (w.max(0) as u32, h.max(0) as u32);
    match format {
        FORMAT_RGBA_8888 | FORMAT_RGBX_8888 => {
            // SAFETY: plane 0 of a live RGBA image.
            let p = unsafe { plane(img, 0)? };
            sink.image(&ScreenImage::Rgba {
                data: p.data,
                width,
                height,
                row_stride: p.row_stride,
                pixel_stride: p.pixel_stride,
            });
        }
        FORMAT_YUV_420_888 => {
            // SAFETY: planes 0 to 2 of a live YUV_420_888 image.
            let planes = unsafe { [plane(img, 0)?, plane(img, 1)?, plane(img, 2)?] };
            sink.image(&ScreenImage::Yuv(Yuv420 {
                width,
                height,
                planes,
            }));
        }
        other => {
            return Err(unsupported(format!("screen image format {other:#x}")));
        }
    }
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

/// A Java `Surface` for `window`, as a local reference of `env`.
fn surface(env: &Env, window: *mut ANativeWindow) -> Result<JObject, VideoError> {
    let to_surface = late()
        .to_surface
        .ok_or_else(|| unsupported("screen capture needs Android 8 (API 26) or newer"))?;
    // SAFETY: `env` is this thread's env and `window` a live window (its
    // reader is alive); the function returns a local reference or null.
    let s = unsafe { to_surface(env.raw() as *mut c_void, window) };
    if s.is_null() {
        return Err(VideoError::os(0, "ANativeWindow_toSurface returned null"));
    }
    Ok(s)
}

/// A running virtual display.
pub(crate) struct NdkProjection {
    vm: usize,
    /// A global reference to the `VirtualDisplay`.
    display: JObject,
    reader: Reader,
    sink: Arc<dyn ScreenSink>,
}

// SAFETY: the display is a global reference (usable from any thread
// attached to the VM), the reader handle is used only by the thread that
// owns this value, and the NDK's thread reaches only the callback context,
// whose sink is Send + Sync.
unsafe impl Send for NdkProjection {}

impl NdkProjection {
    /// Make a `width` x `height` virtual display at `dpi` from the host's
    /// projection.
    ///
    /// # Safety
    ///
    /// `vm` must be the process's `JavaVM*` and `projection` a live global
    /// reference to a `MediaProjection`.
    pub(crate) unsafe fn open(
        vm: usize,
        projection: usize,
        (width, height, dpi): (u32, u32, u32),
        sink: Arc<dyn ScreenSink>,
    ) -> Result<NdkProjection, VideoError> {
        let reader = Reader::new(width, height, sink.clone())?;
        // SAFETY: the caller's promise.
        let env = unsafe { Env::attach(vm)? };
        let surface = surface(&env, reader.window()?)?;
        let name = match env.new_string(c"kessel-video screen") {
            Ok(n) => n,
            Err(e) => {
                env.delete_local(surface);
                return Err(e);
            }
        };
        let projection = projection as JObject;
        // SAFETY: `projection` is a live global reference to a
        // MediaProjection (the caller's promise); the arguments match
        // createVirtualDisplay(String, int, int, int, int, Surface,
        // VirtualDisplay.Callback, Handler); the local refs are deleted
        // after the call.
        let display = unsafe {
            let made = env.method(projection, CREATE, CREATE_SIG).and_then(|m| {
                env.call_object(
                    projection,
                    m,
                    &[
                        JValue { l: name },
                        JValue { i: width as i32 },
                        JValue { i: height as i32 },
                        JValue { i: dpi as i32 },
                        JValue {
                            i: FLAG_AUTO_MIRROR,
                        },
                        JValue { l: surface },
                        JValue {
                            l: std::ptr::null_mut(),
                        },
                        JValue {
                            l: std::ptr::null_mut(),
                        },
                    ],
                    "MediaProjection.createVirtualDisplay (on Android 14+, register a \
                     MediaProjection.Callback first; a projection makes one display)",
                )
            });
            env.delete_local(name);
            env.delete_local(surface);
            let local = made?;
            if local.is_null() {
                return Err(VideoError::os(0, "createVirtualDisplay returned null"));
            }
            let global = env.new_global(local);
            env.delete_local(local);
            global
        };
        if display.is_null() {
            return Err(VideoError::os(0, "NewGlobalRef failed"));
        }
        Ok(NdkProjection {
            vm,
            display,
            reader,
            sink,
        })
    }
}

impl ProjectionCapture for NdkProjection {
    fn resize(&mut self, width: u32, height: u32, dpi: u32) -> Result<(), VideoError> {
        let reader = Reader::new(width, height, self.sink.clone())?;
        // SAFETY: `vm` was the process's JavaVM at open and stays so.
        let env = unsafe { Env::attach(self.vm)? };
        let surface = surface(&env, reader.window()?)?;
        // SAFETY: `display` is our live global reference to a
        // VirtualDisplay; the arguments match resize(int, int, int) and
        // setSurface(Surface); the Surface's local ref is deleted after.
        let done = unsafe {
            env.method(self.display, c"resize", c"(III)V")
                .and_then(|m| {
                    env.call_void(
                        self.display,
                        m,
                        &[
                            JValue { i: width as i32 },
                            JValue { i: height as i32 },
                            JValue { i: dpi as i32 },
                        ],
                        "VirtualDisplay.resize",
                    )
                })
                .and_then(|()| {
                    env.method(self.display, c"setSurface", c"(Landroid/view/Surface;)V")
                })
                .and_then(|m| {
                    env.call_void(
                        self.display,
                        m,
                        &[JValue { l: surface }],
                        "VirtualDisplay.setSurface",
                    )
                })
        };
        env.delete_local(surface);
        done?;
        // The display draws into the new reader now; the old one goes.
        self.reader = reader;
        Ok(())
    }
}

impl Drop for NdkProjection {
    fn drop(&mut self) {
        // SAFETY: as in `resize`; release() takes no arguments, and the
        // global reference is deleted once, after the last use. The reader
        // drops after this, once the display no longer draws into it.
        unsafe {
            if let Ok(env) = Env::attach(self.vm) {
                if let Ok(m) = env.method(self.display, c"release", c"()V") {
                    let _ = env.call_void(self.display, m, &[], "VirtualDisplay.release");
                }
                env.delete_global(self.display);
            }
        }
    }
}
