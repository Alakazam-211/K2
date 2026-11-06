//! AHardwareBuffers from the host as native frames: acquired while a frame
//! holds them, read back to CPU planes by locking them (`AHardwareBuffer_lock`
//! for RGBA and RGBX, API 26; `AHardwareBuffer_lockPlanes` for YUV 4:2:0,
//! API 29), through the functions [`late`] finds.

use super::ffi::*;
use crate::android::screen::{rgba_tight, to_nv12, ScreenImage};
use crate::convert::{PlaneView, SourceView, ViewFn};
use crate::error::{invalid, unsupported, VideoError};
use crate::frame::{FrameBuffer, NativeBuffer, NativeFrame, PixelFormat, Planes, VideoFrame};
use crate::yuv::{Plane, Yuv420};
use std::any::Any;
use std::ffi::c_void;
use std::sync::Arc;

/// An acquired AHardwareBuffer, released on drop.
pub(crate) struct HwFrame {
    buffer: *mut AHardwareBuffer,
    desc: AHardwareBuffer_Desc,
    ahb: Ahb,
}

// SAFETY: AHardwareBuffers are reference-counted and may be used, locked and
// released from any thread.
unsafe impl Send for HwFrame {}
// SAFETY: as above; `&self` only reads the descriptor and locks for reading.
unsafe impl Sync for HwFrame {}

impl Drop for HwFrame {
    fn drop(&mut self) {
        // SAFETY: acquired in `hardware_buffer_frame`, released once.
        unsafe { (self.ahb.release)(self.buffer) };
    }
}

/// Unlocks a locked buffer.
struct Locked<'a>(&'a HwFrame);

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        // SAFETY: locked by this frame; no fence wanted (null).
        unsafe { (self.0.ahb.unlock)(self.0.buffer, std::ptr::null_mut()) };
    }
}

fn lock_failed(what: &str, status: i32) -> VideoError {
    VideoError::os(status as i64, format!("{what} failed"))
}

impl HwFrame {
    fn rgba(&self) -> bool {
        matches!(
            self.desc.format,
            AHB_FORMAT_R8G8B8A8_UNORM | AHB_FORMAT_R8G8B8X8_UNORM
        )
    }

    /// Hand the buffer's planes to `f` while it is locked for reading (no
    /// copy): RGBA and RGBX as one packed plane, YUV 4:2:0 as its three
    /// planes with their row and pixel strides (planar, NV12 or NV21).
    pub(crate) fn read_view(&self, f: ViewFn<'_>) -> Result<VideoFrame, VideoError> {
        let (w, h) = (self.desc.width, self.desc.height);
        if self.rgba() {
            let mut addr: *mut c_void = std::ptr::null_mut();
            // SAFETY: a live, acquired buffer; CPU read; no fence; the
            // whole buffer; a valid out pointer.
            let st = unsafe {
                (self.ahb.lock)(
                    self.buffer,
                    AHB_USAGE_CPU_READ_OFTEN,
                    -1,
                    std::ptr::null(),
                    &mut addr,
                )
            };
            if st != 0 || addr.is_null() {
                return Err(lock_failed("AHardwareBuffer_lock", st));
            }
            let _unlock = Locked(self);
            let row_stride = self.desc.stride as usize * 4;
            let len = row_stride * (h as usize - 1) + w as usize * 4;
            // SAFETY: a locked RGBA buffer of `stride` pixels per row holds
            // at least `len` bytes, valid until it is unlocked (after `f`).
            let data = unsafe { std::slice::from_raw_parts(addr as *const u8, len) };
            let plane = PlaneView::new(data, row_stride, 4);
            return f(&SourceView::Rgb { plane, bgra: false }, w, h);
        }
        let planes = self.lock_yuv()?;
        let _unlock = Locked(self);
        let views = self.yuv_views(&planes)?;
        let [y, u, v] = views.map(|p| PlaneView::new(p.data, p.row_stride, p.pixel_stride));
        f(&SourceView::Yuv([y, u, v]), w, h)
    }

    /// Lock a YUV 4:2:0 buffer's planes for reading (API 29). The caller
    /// unlocks it (`Locked`).
    fn lock_yuv(&self) -> Result<AHardwareBuffer_Planes, VideoError> {
        if self.desc.format != AHB_FORMAT_Y8CB8CR8_420 {
            return Err(unsupported(format!(
                "AHardwareBuffer format {:#x}",
                self.desc.format
            )));
        }
        let lock_planes = self
            .ahb
            .lock_planes
            .ok_or_else(|| unsupported("YUV AHardwareBuffers need Android 10 (API 29)"))?;
        let mut planes = AHardwareBuffer_Planes {
            planeCount: 0,
            planes: [AHardwareBuffer_Plane {
                data: std::ptr::null_mut(),
                pixelStride: 0,
                rowStride: 0,
            }; 4],
        };
        // SAFETY: a live, acquired buffer; CPU read; no fence; the whole
        // buffer; a valid planes out pointer.
        let st = unsafe {
            lock_planes(
                self.buffer,
                AHB_USAGE_CPU_READ_OFTEN,
                -1,
                std::ptr::null(),
                &mut planes,
            )
        };
        if st != 0 {
            return Err(lock_failed("AHardwareBuffer_lockPlanes", st));
        }
        Ok(planes)
    }

    /// The locked planes as views, each covering the bytes its pixels
    /// span. Valid while the buffer stays locked.
    fn yuv_views<'a>(
        &self,
        planes: &'a AHardwareBuffer_Planes,
    ) -> Result<[Plane<'a>; 3], VideoError> {
        let (w, h) = (self.desc.width, self.desc.height);
        if planes.planeCount < 3 {
            return Err(VideoError::os(0, format!("{} planes", planes.planeCount)));
        }
        let (cw, ch) = ((w as usize).div_ceil(2), (h as usize).div_ceil(2));
        let mut views = [Plane {
            data: &[],
            row_stride: 0,
            pixel_stride: 0,
        }; 3];
        for (i, v) in views.iter_mut().enumerate() {
            let p = planes.planes[i];
            let (cols, rows) = if i == 0 {
                (w as usize, h as usize)
            } else {
                (cw, ch)
            };
            if p.data.is_null() || p.pixelStride == 0 || cols == 0 || rows == 0 {
                return Err(VideoError::os(0, format!("plane {i} is empty")));
            }
            let len = (rows - 1) * p.rowStride as usize + (cols - 1) * p.pixelStride as usize + 1;
            *v = Plane {
                // SAFETY: a locked plane of `rows` rows of `rowStride` bytes
                // holds the `len` bytes its pixels span, valid while the
                // buffer stays locked (the caller's `Locked` outlives every
                // use of the views).
                data: unsafe { std::slice::from_raw_parts(p.data as *const u8, len) },
                row_stride: p.rowStride as usize,
                pixel_stride: p.pixelStride as usize,
            };
        }
        Ok(views)
    }

    /// The pixels as tight CPU planes in their own layout: RGBA, or NV12
    /// for YUV.
    fn read(&self) -> Result<Planes, VideoError> {
        let (w, h) = (self.desc.width, self.desc.height);
        if self.rgba() {
            let mut addr: *mut c_void = std::ptr::null_mut();
            // SAFETY: a live, acquired buffer; CPU read; no fence; the
            // whole buffer; a valid out pointer.
            let st = unsafe {
                (self.ahb.lock)(
                    self.buffer,
                    AHB_USAGE_CPU_READ_OFTEN,
                    -1,
                    std::ptr::null(),
                    &mut addr,
                )
            };
            if st != 0 || addr.is_null() {
                return Err(lock_failed("AHardwareBuffer_lock", st));
            }
            let _unlock = Locked(self);
            let row_stride = self.desc.stride as usize * 4;
            let len = row_stride * (h as usize - 1) + w as usize * 4;
            // SAFETY: a locked RGBA buffer of `stride` pixels per row holds
            // at least `len` bytes, valid until it is unlocked (after this
            // copy).
            let data = unsafe { std::slice::from_raw_parts(addr as *const u8, len) };
            let opaque = self.desc.format == AHB_FORMAT_R8G8B8X8_UNORM;
            let tight = rgba_tight(data, w, h, row_stride, opaque)?;
            return Planes::tight(PixelFormat::Rgba, w, h, tight);
        }
        let planes = self.lock_yuv()?;
        let _unlock = Locked(self);
        let views = self.yuv_views(&planes)?;
        let f = to_nv12(
            &ScreenImage::Yuv(Yuv420 {
                width: w,
                height: h,
                planes: views,
            }),
            0,
        )?;
        match f.buffer {
            FrameBuffer::Planes(p) => Ok(p),
            FrameBuffer::Native(_) => Err(invalid("NV12 copy gave a native buffer")),
        }
    }
}

impl NativeFrame for HwFrame {
    fn format(&self) -> PixelFormat {
        if self.rgba() {
            PixelFormat::Rgba
        } else {
            PixelFormat::Nv12
        }
    }

    fn to_planes(&self, format: PixelFormat) -> Result<Planes, VideoError> {
        let planes = self.read()?;
        if planes.format == format {
            return Ok(planes);
        }
        let (w, h) = (self.desc.width, self.desc.height);
        let f = VideoFrame::from_planes(planes, w, h, 0)?;
        match crate::convert::convert(&f, w, h, format)?.buffer {
            FrameBuffer::Planes(p) => Ok(p),
            FrameBuffer::Native(_) => Err(invalid("convert gave a native buffer")),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// See [`crate::hardware_buffer_frame`].
///
/// # Safety
///
/// `buffer` must be a live `AHardwareBuffer*`.
pub(crate) unsafe fn hardware_buffer_frame(
    buffer: *mut c_void,
    capture_us: u64,
) -> Result<VideoFrame, VideoError> {
    if buffer.is_null() {
        return Err(invalid("a null AHardwareBuffer"));
    }
    let ahb = late()
        .ahb
        .ok_or_else(|| unsupported("AHardwareBuffer needs Android 8 (API 26) or newer"))?;
    let buffer = buffer as *mut AHardwareBuffer;
    let mut desc = AHardwareBuffer_Desc::default();
    // SAFETY: a live buffer (the caller's promise) and a valid out pointer.
    unsafe { (ahb.describe)(buffer, &mut desc) };
    let known = matches!(
        desc.format,
        AHB_FORMAT_R8G8B8A8_UNORM | AHB_FORMAT_R8G8B8X8_UNORM | AHB_FORMAT_Y8CB8CR8_420
    );
    if !known {
        return Err(unsupported(format!(
            "AHardwareBuffer format {:#x}",
            desc.format
        )));
    }
    if desc.width == 0 || desc.height == 0 || desc.width > 16_384 || desc.height > 16_384 {
        return Err(invalid(format!("a {}x{} buffer", desc.width, desc.height)));
    }
    // SAFETY: a live buffer; released once when the frame drops.
    unsafe { (ahb.acquire)(buffer) };
    let (w, h) = (desc.width, desc.height);
    let frame = HwFrame { buffer, desc, ahb };
    Ok(VideoFrame::from_native(
        NativeBuffer(Arc::new(frame)),
        w,
        h,
        capture_us,
    ))
}
