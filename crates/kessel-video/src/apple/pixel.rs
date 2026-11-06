//! CVPixelBuffers: as native frames, and copies to and from CPU planes.

use super::ffi::*;
use crate::convert::{PlaneView, SourceView, ViewFn};
use crate::error::{invalid, VideoError};
use crate::frame::{
    ColorSpace, FrameBuffer, NativeBuffer, NativeFrame, PixelFormat, Planes, VideoFrame,
};
use crate::kit::lock;
use std::any::Any;
use std::sync::{Arc, Mutex};

autolink!("CoreVideo", "CoreFoundation");

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

/// Fill an NV12 pixel buffer from NV12 or I420 planes of the same size
/// (I420's chroma planes are interleaved on the way in).
pub(crate) fn fill_nv12(
    pb: CVPixelBufferRef,
    planes: &Planes,
    width: u32,
    height: u32,
) -> Result<(), VideoError> {
    if !matches!(planes.format, PixelFormat::Nv12 | PixelFormat::I420) {
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
                let dst = std::slice::from_raw_parts_mut(base.add(y * bpr), row);
                if i == 1 && planes.format == PixelFormat::I420 {
                    let u = planes.row(1, y, width, height);
                    let v = planes.row(2, y, width, height);
                    for (pair, (u, v)) in dst.chunks_exact_mut(2).zip(u.iter().zip(v)) {
                        pair[0] = *u;
                        pair[1] = *v;
                    }
                } else {
                    dst.copy_from_slice(planes.row(i, y, width, height));
                }
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

impl CvFrame {
    /// Hand the buffer's planes to `f` while it is locked for reading
    /// (no copy): NV12 (`420v`, `420f`) as Y and interleaved UV, BGRA as
    /// one packed plane. Other pixel formats are `Unsupported`.
    pub(crate) fn read_view(&self, f: ViewFn<'_>) -> Result<VideoFrame, VideoError> {
        let pb = self.raw();
        let format = format_of(pb);
        let (w, h) = self.size();
        if format == PixelFormat::Native {
            // SAFETY: a live pixel buffer.
            let t = unsafe { CVPixelBufferGetPixelFormatType(pb) };
            return Err(VideoError::Unsupported(format!(
                "pixel format {:?}",
                t.to_be_bytes()
            )));
        }
        // SAFETY: a live pixel buffer, locked read-only until the end of
        // this block. Each slice covers a plane's reported rows times its
        // bytes per row, which the lock keeps mapped, and no slice outlives
        // the lock: `f` only borrows them for its call.
        unsafe {
            if CVPixelBufferLockBaseAddress(pb, LOCK_READ_ONLY) != 0 {
                return Err(VideoError::os(0, "CVPixelBufferLockBaseAddress failed"));
            }
            let plane = |i: Option<usize>| locked_plane(self, i);
            let r = if format == PixelFormat::Bgra {
                match plane(None) {
                    Some(p) => f(
                        &SourceView::Rgb {
                            plane: PlaneView::new(p, CVPixelBufferGetBytesPerRow(pb), 4),
                            bgra: true,
                        },
                        w,
                        h,
                    ),
                    None => Err(VideoError::os(0, "a BGRA pixel buffer with no memory")),
                }
            } else {
                match (plane(Some(0)), plane(Some(1))) {
                    (Some(y), Some(uv)) if uv.len() > 1 => {
                        let (ys, uvs) = (
                            CVPixelBufferGetBytesPerRowOfPlane(pb, 0),
                            CVPixelBufferGetBytesPerRowOfPlane(pb, 1),
                        );
                        f(
                            &SourceView::Yuv([
                                PlaneView::new(y, ys, 1),
                                PlaneView::new(uv, uvs, 2),
                                PlaneView::new(&uv[1..], uvs, 2),
                            ]),
                            w,
                            h,
                        )
                    }
                    _ => Err(VideoError::os(0, "an NV12 pixel buffer plane is missing")),
                }
            };
            CVPixelBufferUnlockBaseAddress(pb, LOCK_READ_ONLY);
            r
        }
    }
}

/// A locked pixel buffer's plane `i` (`None`: the one plane of a packed
/// buffer), as its rows times its bytes per row.
///
/// # Safety
/// `cv`'s buffer must be locked for reading while the slice is used.
unsafe fn locked_plane(cv: &CvFrame, i: Option<usize>) -> Option<&[u8]> {
    let pb = cv.raw();
    let (base, bpr, rows) = match i {
        Some(i) => (
            CVPixelBufferGetBaseAddressOfPlane(pb, i) as *const u8,
            CVPixelBufferGetBytesPerRowOfPlane(pb, i),
            CVPixelBufferGetHeightOfPlane(pb, i),
        ),
        None => (
            CVPixelBufferGetBaseAddress(pb) as *const u8,
            CVPixelBufferGetBytesPerRow(pb),
            CVPixelBufferGetHeight(pb),
        ),
    };
    (!base.is_null() && bpr > 0 && rows > 0).then(|| std::slice::from_raw_parts(base, bpr * rows))
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
/// VideoToolbox decoder's output asked as `PixelFormat::Native`, a
/// camera's or screen's native frame, or any frame the backend's
/// `convert` turned into `PixelFormat::Native`: an IOSurface-backed NV12
/// buffer from a pool), for handing to Apple APIs, such as
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

/// A frame around a CVPixelBuffer the host already has (from ReplayKit or
/// its own capture code), for a [`crate::frame_in()`] source or an encoder.
/// The buffer is retained (the caller keeps its own reference) and its
/// colour description is read from its attachments. An Apple encoder takes
/// it without a copy; anything else reads it back to CPU planes.
///
/// NV12 (`420v`, `420f`) and BGRA buffers only; other formats are
/// `Unsupported`.
///
/// # Safety
///
/// `pixel_buffer` must be a live `CVPixelBufferRef`.
pub unsafe fn pixel_buffer_frame(
    pixel_buffer: *mut std::ffi::c_void,
    capture_us: u64,
) -> Result<VideoFrame, VideoError> {
    let pb = pixel_buffer as CVPixelBufferRef;
    if pb.is_null() {
        return Err(invalid("a null pixel buffer"));
    }
    if format_of(pb) == PixelFormat::Native {
        // SAFETY: a live pixel buffer (the caller's promise).
        let t = unsafe { CVPixelBufferGetPixelFormatType(pb) };
        return Err(VideoError::Unsupported(format!(
            "pixel format {:?}",
            t.to_be_bytes()
        )));
    }
    // SAFETY: as above.
    Ok(unsafe { native_frame(pb, capture_us) })
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

/// How many pixel-buffer pools [`Pools`] keeps: one per size and range in
/// use, so a few remote tracks of different sizes don't rebuild a pool on
/// every frame.
const POOLS_KEPT: usize = 4;

/// A pool's size and CoreVideo pixel format (`420v` or `420f`).
pub(crate) type PoolKey = (u32, u32, u32);

/// The NV12 pixel-buffer pools behind `convert(.., PixelFormat::Native)`,
/// most recently used first. Each pool hands out IOSurface-backed,
/// Metal-compatible buffers, and reuses a buffer once every frame holding
/// it is dropped.
#[derive(Default)]
pub(crate) struct Pools {
    pools: Mutex<Vec<(PoolKey, Arc<Cf>)>>,
}

impl Pools {
    /// The pool for `key`, made on first use. A new pool (a size change)
    /// also lets the others release their idle buffers.
    fn pool(&self, key: PoolKey) -> Result<Arc<Cf>, VideoError> {
        let mut pools = lock(&self.pools);
        if let Some(i) = pools.iter().position(|(k, _)| *k == key) {
            let entry = pools.remove(i);
            let pool = entry.1.clone();
            pools.insert(0, entry);
            return Ok(pool);
        }
        let (w, h, pix) = key;
        let attrs = pixel_buffer_attributes(pix, Some((w, h)));
        set_named(&attrs, c"kCVPixelBufferMetalCompatibilityKey", yes());
        let mut pool: CVPixelBufferPoolRef = std::ptr::null_mut();
        // SAFETY: a valid attributes dictionary (no pool attributes) and an
        // out pointer.
        let st = unsafe {
            CVPixelBufferPoolCreate(std::ptr::null(), std::ptr::null(), attrs.0, &mut pool)
        };
        if st != 0 || pool.is_null() {
            return Err(VideoError::os(
                st as i64,
                format!("CVPixelBufferPoolCreate for {w}x{h}"),
            ));
        }
        let pool = Arc::new(Cf(pool as CFTypeRef));
        for (_, old) in pools.iter() {
            // SAFETY: a live pool we hold.
            unsafe { CVPixelBufferPoolFlush(old.0 as CVPixelBufferPoolRef, POOL_FLUSH_EXCESS) };
        }
        pools.insert(0, (key, pool.clone()));
        pools.truncate(POOLS_KEPT);
        Ok(pool)
    }

    /// `frame` at `width` x `height` as a native frame. A CVPixelBuffer
    /// frame of that size passes through (BGRA too). Anything else is
    /// scaled and converted on the CPU when it needs to be, then copied
    /// into an NV12 buffer from a pool and tagged with the frame's colour
    /// (`420f` for full range, `420v` otherwise).
    pub(crate) fn native(
        &self,
        frame: &VideoFrame,
        width: u32,
        height: u32,
    ) -> Result<VideoFrame, VideoError> {
        if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
            return Err(invalid(format!("convert to {width}x{height}")));
        }
        let same_size = (width, height) == (frame.width, frame.height);
        let converted;
        let planes = match &frame.buffer {
            FrameBuffer::Native(n) if same_size && n.0.as_any().is::<CvFrame>() => {
                return Ok(frame.clone());
            }
            FrameBuffer::Planes(p)
                if same_size && matches!(p.format, PixelFormat::I420 | PixelFormat::Nv12) =>
            {
                p
            }
            _ => {
                converted = crate::convert::convert(frame, width, height, PixelFormat::Nv12)?;
                match &converted.buffer {
                    FrameBuffer::Planes(p) => p,
                    FrameBuffer::Native(_) => return Err(invalid("convert gave a native buffer")),
                }
            }
        };
        let pix = if frame.color == ColorSpace::Bt709Full {
            PIX_420F
        } else {
            PIX_420V
        };
        let pool = self.pool((width, height, pix))?;
        let mut pb: CVPixelBufferRef = std::ptr::null_mut();
        // SAFETY: a live pool; an out pointer.
        let st = unsafe {
            CVPixelBufferPoolCreatePixelBuffer(
                std::ptr::null(),
                pool.0 as CVPixelBufferPoolRef,
                &mut pb,
            )
        };
        if st != 0 || pb.is_null() {
            return Err(VideoError::os(st as i64, "no pixel buffer from the pool"));
        }
        let owned = Cf(pb as CFTypeRef);
        fill_nv12(pb, planes, width, height)?;
        tag_color(pb, frame.color);
        // SAFETY: a live pixel buffer, held by `owned` until the frame has
        // its own retain.
        let mut out = unsafe { native_frame(pb, frame.capture_us) };
        drop(owned);
        out.color = frame.color;
        Ok(out)
    }

    /// The pools held, most recently used first (tests).
    #[cfg(test)]
    pub(crate) fn held(&self) -> Vec<(PoolKey, CFTypeRef)> {
        lock(&self.pools).iter().map(|(k, p)| (*k, p.0)).collect()
    }
}
