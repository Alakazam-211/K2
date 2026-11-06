//! CVPixelBuffers: as native frames, and copies to and from CPU planes.

use super::ffi::*;
use crate::error::{invalid, VideoError};
use crate::frame::{ColorSpace, NativeBuffer, NativeFrame, PixelFormat, Planes, VideoFrame};
use std::any::Any;
use std::sync::Arc;

/// A retained CVPixelBuffer (NV12 `420v`/`420f` or BGRA).
pub struct CvFrame {
    pub(crate) pb: Cf,
}

impl CvFrame {
    /// Retain `pb`.
    ///
    /// # Safety
    /// `pb` must be a live CVPixelBuffer.
    pub(crate) unsafe fn retain(pb: CVPixelBufferRef) -> CvFrame {
        CvFrame {
            pb: Cf::retain(pb as CFTypeRef),
        }
    }

    pub(crate) fn raw(&self) -> CVPixelBufferRef {
        self.pb.0 as CVPixelBufferRef
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        // SAFETY: a live pixel buffer.
        unsafe {
            (
                CVPixelBufferGetWidth(self.raw()) as u32,
                CVPixelBufferGetHeight(self.raw()) as u32,
            )
        }
    }
}

fn format_of(pb: CVPixelBufferRef) -> PixelFormat {
    // SAFETY: a live pixel buffer.
    match unsafe { CVPixelBufferGetPixelFormatType(pb) } {
        PIX_420V | PIX_420F => PixelFormat::Nv12,
        PIX_BGRA => PixelFormat::Bgra,
        _ => PixelFormat::Native,
    }
}

/// The colour description from a pixel buffer's attachments.
pub(crate) fn color_of(pb: CVPixelBufferRef) -> ColorSpace {
    // SAFETY: a live pixel buffer.
    let full = unsafe { CVPixelBufferGetPixelFormatType(pb) } == PIX_420F;
    let matrix = cf_const(c"kCVImageBufferYCbCrMatrixKey")
        // SAFETY: a live buffer and a framework key; the value is borrowed.
        .map(|k| unsafe { CVBufferGetAttachment(pb, k, std::ptr::null_mut()) })
        .unwrap_or(std::ptr::null());
    let is_709 = !matrix.is_null()
        && cf_const(c"kCVImageBufferYCbCrMatrix_ITU_R_709_2")
            // SAFETY: two live CF objects.
            .is_some_and(|m709| unsafe { CFEqual(matrix, m709) } != 0);
    match (is_709, full) {
        (true, true) => ColorSpace::Bt709Full,
        (true, false) => ColorSpace::Bt709Limited,
        _ => ColorSpace::Bt601Limited,
    }
}

/// Copy a pixel buffer's planes out (NV12 or BGRA, tight).
pub(crate) fn planes_from(pb: CVPixelBufferRef) -> Result<(Planes, u32, u32), VideoError> {
    let format = format_of(pb);
    if format == PixelFormat::Native {
        // SAFETY: a live pixel buffer.
        let t = unsafe { CVPixelBufferGetPixelFormatType(pb) };
        return Err(VideoError::Unsupported(format!(
            "pixel format {:?}",
            t.to_be_bytes()
        )));
    }
    // SAFETY: a live pixel buffer, locked read-only while we copy rows within
    // each plane's reported size.
    unsafe {
        let (w, h) = (
            CVPixelBufferGetWidth(pb) as u32,
            CVPixelBufferGetHeight(pb) as u32,
        );
        if CVPixelBufferLockBaseAddress(pb, LOCK_READ_ONLY) != 0 {
            return Err(VideoError::os(0, "CVPixelBufferLockBaseAddress failed"));
        }
        let mut out = Vec::with_capacity(format.frame_len(w, h));
        let mut ok = true;
        for i in 0..format.plane_count() {
            let (row, rows) = format.plane_size(i, w, h);
            let (base, bpr) = if format == PixelFormat::Bgra {
                (
                    CVPixelBufferGetBaseAddress(pb) as *const u8,
                    CVPixelBufferGetBytesPerRow(pb),
                )
            } else {
                (
                    CVPixelBufferGetBaseAddressOfPlane(pb, i) as *const u8,
                    CVPixelBufferGetBytesPerRowOfPlane(pb, i),
                )
            };
            if base.is_null() || bpr < row {
                ok = false;
                break;
            }
            for y in 0..rows {
                out.extend_from_slice(std::slice::from_raw_parts(base.add(y * bpr), row));
            }
        }
        CVPixelBufferUnlockBaseAddress(pb, LOCK_READ_ONLY);
        if !ok {
            return Err(VideoError::os(
                0,
                "a pixel buffer plane is missing or too narrow",
            ));
        }
        Ok((Planes::tight(format, w, h, out)?, w, h))
    }
}

/// Fill an NV12 pixel buffer from NV12 planes of the same size.
pub(crate) fn fill_nv12(
    pb: CVPixelBufferRef,
    planes: &Planes,
    width: u32,
    height: u32,
) -> Result<(), VideoError> {
    if planes.format != PixelFormat::Nv12 {
        return Err(invalid(format!("fill_nv12 with {:?}", planes.format)));
    }
    planes.validate(width, height)?;
    // SAFETY: a live pixel buffer, locked while we write rows within each
    // plane's reported size, which we check covers the frame.
    unsafe {
        if CVPixelBufferLockBaseAddress(pb, 0) != 0 {
            return Err(VideoError::os(0, "CVPixelBufferLockBaseAddress failed"));
        }
        let mut res = Ok(());
        for i in 0..2 {
            let (row, rows) = PixelFormat::Nv12.plane_size(i, width, height);
            let base = CVPixelBufferGetBaseAddressOfPlane(pb, i) as *mut u8;
            let bpr = CVPixelBufferGetBytesPerRowOfPlane(pb, i);
            let have_rows = CVPixelBufferGetHeightOfPlane(pb, i);
            if base.is_null() || bpr < row || have_rows < rows {
                res = Err(VideoError::os(
                    0,
                    format!(
                        "pixel buffer plane {i} ({bpr} x {have_rows}) can't hold {row} x {rows}"
                    ),
                ));
                break;
            }
            for y in 0..rows {
                let src = planes.row(i, y, width, height);
                std::ptr::copy_nonoverlapping(src.as_ptr(), base.add(y * bpr), row);
            }
        }
        CVPixelBufferUnlockBaseAddress(pb, 0);
        res
    }
}

/// Tag a pixel buffer with its colour matrix, so the encoder writes it in
/// the stream's VUI.
pub(crate) fn tag_color(pb: CVPixelBufferRef, color: ColorSpace) {
    let (matrix, primaries, transfer) = match color {
        ColorSpace::Bt601Limited => (
            c"kCVImageBufferYCbCrMatrix_ITU_R_601_4",
            c"kCVImageBufferColorPrimaries_SMPTE_C",
            c"kCVImageBufferTransferFunction_ITU_R_709_2",
        ),
        _ => (
            c"kCVImageBufferYCbCrMatrix_ITU_R_709_2",
            c"kCVImageBufferColorPrimaries_ITU_R_709_2",
            c"kCVImageBufferTransferFunction_ITU_R_709_2",
        ),
    };
    for (key, value) in [
        (c"kCVImageBufferYCbCrMatrixKey", matrix),
        (c"kCVImageBufferColorPrimariesKey", primaries),
        (c"kCVImageBufferTransferFunctionKey", transfer),
    ] {
        if let (Some(k), Some(v)) = (cf_const(key), cf_const(value)) {
            // SAFETY: a live buffer; a framework key and value.
            unsafe { CVBufferSetAttachment(pb, k, v, ATTACH_PROPAGATE) };
        }
    }
}

impl NativeFrame for CvFrame {
    fn format(&self) -> PixelFormat {
        format_of(self.raw())
    }

    fn to_planes(&self, format: PixelFormat) -> Result<Planes, VideoError> {
        let (planes, w, h) = planes_from(self.raw())?;
        if planes.format == format {
            return Ok(planes);
        }
        let mut f = VideoFrame::from_planes(planes, w, h, 0)?;
        f.color = color_of(self.raw());
        match crate::convert::convert(&f, w, h, format)?.buffer {
            crate::frame::FrameBuffer::Planes(p) => Ok(p),
            crate::frame::FrameBuffer::Native(_) => Err(invalid("convert gave a native buffer")),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A native frame around `pb` (retained).
///
/// # Safety
/// `pb` must be a live CVPixelBuffer.
pub(crate) unsafe fn native_frame(pb: CVPixelBufferRef, capture_us: u64) -> VideoFrame {
    let f = CvFrame::retain(pb);
    let (w, h) = f.size();
    let color = color_of(pb);
    let mut v = VideoFrame::from_native(NativeBuffer(Arc::new(f)), w, h, capture_us);
    v.color = color;
    v
}

/// The `CVPixelBufferRef` inside a native frame from the Apple backend (a
/// VideoToolbox decoder's output asked as `PixelFormat::Native`, or a
/// camera's or screen's native frame), for handing to Apple APIs, such as
/// wrapping it in a `CMSampleBuffer` for an `AVSampleBufferDisplayLayer`.
/// `None` for a native buffer of another kind.
///
/// The pointer is borrowed: it stays valid while `buffer` (or any clone of
/// it) lives. To keep the pixel buffer longer, `CVBufferRetain` it and
/// `CVBufferRelease` it when done. Treat it as read-only: other clones may
/// be reading it.
pub fn apple_pixel_buffer(buffer: &NativeBuffer) -> Option<*mut std::ffi::c_void> {
    buffer
        .0
        .as_any()
        .downcast_ref::<CvFrame>()
        .map(CvFrame::raw)
}

/// A CPU copy of `pb` (NV12 or BGRA).
///
/// # Safety
/// `pb` must be a live CVPixelBuffer.
pub(crate) unsafe fn cpu_frame(
    pb: CVPixelBufferRef,
    capture_us: u64,
) -> Result<VideoFrame, VideoError> {
    let (planes, w, h) = planes_from(pb)?;
    let mut v = VideoFrame::from_planes(planes, w, h, capture_us)?;
    v.color = color_of(pb);
    Ok(v)
}
