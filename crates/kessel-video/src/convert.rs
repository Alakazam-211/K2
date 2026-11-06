//! CPU colour conversion and scaling between I420, NV12, BGRA and RGBA.
//!
//! Everything goes through I420 at the source size, is scaled there
//! (bilinear; area-averaged when shrinking by 2 or more), then packed into
//! the target layout. RGB uses the frame's [`ColorSpace`].
//!
//! To I420 or NV12, the source is read where it lies: CPU planes, or an OS
//! buffer of the backend locked for reading (a CVPixelBuffer, an
//! AHardwareBuffer). Its planes are scaled straight into the output, whose
//! memory comes from a [`Pool`] and goes back to it when the frame's last
//! clone drops. A steady stream of frames allocates no pixel memory.

use crate::error::{invalid, VideoError};
use crate::frame::{tight_layout, ColorSpace, FrameBuffer, PixelFormat, Planes, VideoFrame};
use crate::kit::lock;
use bytes::Bytes;
use std::sync::{Arc, Mutex, OnceLock};

/// I420 planes, tightly packed, at one size.
pub(crate) struct Yuv {
    pub(crate) w: usize,
    pub(crate) h: usize,
    pub(crate) y: Vec<u8>,
    pub(crate) u: Vec<u8>,
    pub(crate) v: Vec<u8>,
}

impl Yuv {
    fn cw(&self) -> usize {
        self.w.div_ceil(2)
    }
    fn ch(&self) -> usize {
        self.h.div_ceil(2)
    }

    fn empty() -> Yuv {
        Yuv {
            w: 0,
            h: 0,
            y: Vec::new(),
            u: Vec::new(),
            v: Vec::new(),
        }
    }

    /// Resize to `w` x `h`, keeping the memory it has.
    fn reset(&mut self, w: usize, h: usize) {
        self.w = w;
        self.h = h;
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        self.y.resize(w * h, 0);
        self.u.resize(cw * ch, 0);
        self.v.resize(cw * ch, 0);
    }

    fn views(&self) -> [PlaneView<'_>; 3] {
        let (w, cw) = (self.w, self.cw());
        [
            PlaneView::new(&self.y, w, 1),
            PlaneView::new(&self.u, cw, 1),
            PlaneView::new(&self.v, cw, 1),
        ]
    }
}

// ───────────────────────────── sources ─────────────────────────────

/// One plane of a picture held elsewhere (CPU planes, or an OS buffer
/// locked for reading): `data` starts at its first sample; rows are
/// `stride` bytes apart and samples `step` bytes apart (2 for interleaved
/// chroma, 4 for packed RGB).
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlaneView<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) stride: usize,
    pub(crate) step: usize,
}

impl<'a> PlaneView<'a> {
    pub(crate) fn new(data: &'a [u8], stride: usize, step: usize) -> PlaneView<'a> {
        PlaneView { data, stride, step }
    }

    /// Does it hold `cols` x `rows` samples of `width` bytes each?
    fn holds(&self, cols: usize, rows: usize, width: usize) -> bool {
        if cols == 0 || rows == 0 || self.step == 0 {
            return false;
        }
        (rows - 1)
            .checked_mul(self.stride)
            .zip((cols - 1).checked_mul(self.step))
            .and_then(|(r, c)| r.checked_add(c)?.checked_add(width))
            .is_some_and(|end| end <= self.data.len())
    }
}

/// A picture to read in place.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SourceView<'a> {
    /// 4:2:0: Y at full size, U and V at half size rounded up (they may
    /// interleave: NV12, NV21, Android's YUV_420_888 with pixel stride 2).
    Yuv([PlaneView<'a>; 3]),
    /// Packed 8-bit RGB with a fourth byte: BGRA, or RGBA (RGBX).
    Rgb { plane: PlaneView<'a>, bgra: bool },
}

impl SourceView<'_> {
    /// Check that every sample of a `w` x `h` picture is in the views.
    fn check(&self, w: usize, h: usize) -> Result<(), VideoError> {
        let ok = match self {
            SourceView::Yuv([y, u, v]) => {
                let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
                y.holds(w, h, 1) && u.holds(cw, ch, 1) && v.holds(cw, ch, 1)
            }
            SourceView::Rgb { plane, .. } => plane.step == 4 && plane.holds(w, h, 4),
        };
        if ok {
            Ok(())
        } else {
            Err(invalid(format!(
                "a {w}x{h} source whose planes don't cover it"
            )))
        }
    }

    /// Views of tight or padded CPU planes (checked by the caller).
    fn of_planes(p: &Planes) -> SourceView<'_> {
        let plane = |i: usize, extra: usize, step: usize| {
            PlaneView::new(&p.data[p.offsets[i] + extra..], p.strides[i] as usize, step)
        };
        match p.format {
            PixelFormat::I420 => SourceView::Yuv([plane(0, 0, 1), plane(1, 0, 1), plane(2, 0, 1)]),
            PixelFormat::Nv12 => SourceView::Yuv([plane(0, 0, 1), plane(1, 0, 2), plane(1, 1, 2)]),
            PixelFormat::Bgra => SourceView::Rgb {
                plane: plane(0, 0, 4),
                bgra: true,
            },
            _ => SourceView::Rgb {
                plane: plane(0, 0, 4),
                bgra: false,
            },
        }
    }
}

/// Hands a picture's views to a conversion while it is readable (an OS
/// buffer stays locked for the call): the views, and the picture's width
/// and height.
pub(crate) type ViewFn<'f> =
    &'f mut dyn FnMut(&SourceView<'_>, u32, u32) -> Result<VideoFrame, VideoError>;

/// Read `frame` in place, if its pixels can be: CPU planes, or a native
/// buffer of this platform's backend. `None`: read it back first.
fn with_view(frame: &VideoFrame, f: ViewFn<'_>) -> Option<Result<VideoFrame, VideoError>> {
    match &frame.buffer {
        FrameBuffer::Planes(p) => Some(
            p.validate(frame.width, frame.height)
                .and_then(|()| f(&SourceView::of_planes(p), frame.width, frame.height)),
        ),
        #[allow(unused_variables)]
        FrameBuffer::Native(n) => {
            #[cfg(all(feature = "apple", any(target_os = "macos", target_os = "ios")))]
            if let Some(cv) = n.0.as_any().downcast_ref::<crate::apple::pixel::CvFrame>() {
                return Some(cv.read_view(f));
            }
            #[cfg(all(feature = "android", target_os = "android"))]
            if let Some(hw) = crate::android::read_native_view(n, f) {
                return Some(hw);
            }
            None
        }
    }
}

// ───────────────────────────── the output pool ─────────────────────────────

/// How many idle output buffers a [`Pool`] keeps, and their most bytes.
const POOL_KEEP: usize = 8;
const POOL_KEEP_BYTES: usize = 64 << 20;
/// How many I420 scratch pictures (for RGB sources) it keeps.
const SCRATCH_KEEP: usize = 2;

/// Where converted frames' memory comes from and goes back to.
#[derive(Default)]
pub(crate) struct Pool {
    free: Mutex<Vec<Vec<u8>>>,
    scratch: Mutex<Vec<Yuv>>,
}

/// A pooled buffer inside a frame's `Bytes`: back to its pool on drop.
struct Pooled {
    buf: Vec<u8>,
    pool: Arc<Pool>,
}

impl AsRef<[u8]> for Pooled {
    fn as_ref(&self) -> &[u8] {
        &self.buf
    }
}

impl Drop for Pooled {
    fn drop(&mut self) {
        self.pool.give(std::mem::take(&mut self.buf));
    }
}

impl Pool {
    /// A buffer of `len` bytes: the smallest idle one that fits, or a new
    /// one. Its contents are whatever they were.
    fn take(&self, len: usize) -> Vec<u8> {
        let mut free = lock(&self.free);
        let fit = free
            .iter()
            .enumerate()
            .filter(|(_, v)| v.capacity() >= len)
            .min_by_key(|(_, v)| v.capacity())
            .map(|(i, _)| i);
        let mut v = match fit {
            Some(i) => free.swap_remove(i),
            None => Vec::new(),
        };
        drop(free);
        if v.len() >= len {
            v.truncate(len);
        } else {
            v.resize(len, 0);
        }
        v
    }

    /// Keep `v` for a later frame; past the limits, the smallest go.
    fn give(&self, v: Vec<u8>) {
        if v.capacity() == 0 {
            return;
        }
        let mut free = lock(&self.free);
        free.push(v);
        while free.len() > POOL_KEEP
            || free.iter().map(Vec::capacity).sum::<usize>() > POOL_KEEP_BYTES
        {
            let Some(i) = (0..free.len()).min_by_key(|&i| free[i].capacity()) else {
                break;
            };
            free.swap_remove(i);
        }
    }

    fn take_scratch(&self) -> Yuv {
        lock(&self.scratch).pop().unwrap_or_else(Yuv::empty)
    }

    fn give_scratch(&self, y: Yuv) {
        let mut s = lock(&self.scratch);
        if s.len() < SCRATCH_KEEP {
            s.push(y);
        }
    }

    /// Idle buffers held (tests).
    #[cfg(test)]
    pub(crate) fn idle(&self) -> Vec<*const u8> {
        lock(&self.free).iter().map(|v| v.as_ptr()).collect()
    }
}

/// The pool behind [`convert`].
pub(crate) fn pool() -> &'static Arc<Pool> {
    static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
    POOL.get_or_init(Arc::default)
}

/// `src` (`sw` x `sh`) scaled into a pooled I420 or NV12 frame of `dw` x
/// `dh`.
#[allow(clippy::too_many_arguments)]
fn pooled_frame(
    pool: &Arc<Pool>,
    src: &SourceView<'_>,
    (sw, sh): (u32, u32),
    (dw, dh): (u32, u32),
    format: PixelFormat,
    color: ColorSpace,
    capture_us: u64,
) -> Result<VideoFrame, VideoError> {
    let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
    src.check(sw, sh)?;
    let mut scratch = None;
    let yuv = match src {
        SourceView::Yuv(planes) => *planes,
        SourceView::Rgb { plane, bgra } => {
            let s = scratch.insert(pool.take_scratch());
            rgb_into(*plane, *bgra, sw, sh, color, s);
            s.views()
        }
    };
    let mut buf = pool.take(format.frame_len(dw as u32, dh as u32));
    let (luma, chroma) = buf.split_at_mut(dw * dh);
    let (scw, sch, cw, ch) = (
        sw.div_ceil(2),
        sh.div_ceil(2),
        dw.div_ceil(2),
        dh.div_ceil(2),
    );
    scale_view(yuv[0], sw, sh, luma, dw, 1, dw, dh);
    if format == PixelFormat::I420 {
        let (u, v) = chroma.split_at_mut(cw * ch);
        scale_view(yuv[1], scw, sch, u, cw, 1, cw, ch);
        scale_view(yuv[2], scw, sch, v, cw, 1, cw, ch);
    } else {
        scale_view(yuv[1], scw, sch, chroma, cw * 2, 2, cw, ch);
        scale_view(yuv[2], scw, sch, &mut chroma[1..], cw * 2, 2, cw, ch);
    }
    if let Some(s) = scratch {
        pool.give_scratch(s);
    }
    let (strides, offsets) = tight_layout(format, dw as u32, dh as u32);
    let planes = Planes {
        format,
        data: Bytes::from_owner(Pooled {
            buf,
            pool: pool.clone(),
        }),
        strides,
        offsets,
    };
    let mut out = VideoFrame::from_planes(planes, dw as u32, dh as u32, capture_us)?;
    out.color = color;
    Ok(out)
}

// ───────────────────────────── convert ─────────────────────────────

/// `frame` scaled to `width` x `height` and converted to `format` (I420,
/// NV12, BGRA or RGBA), as tight planes. The same size and format: a cheap
/// clone.
///
/// To I420 or NV12, a native buffer of the backend (a CVPixelBuffer, an
/// AHardwareBuffer) is read in place while locked, and the output's memory
/// is pooled: it is reused once the returned frame (and every clone of it)
/// is dropped. Other native buffers, and the RGB targets, read the frame
/// back first.
pub fn convert(
    frame: &VideoFrame,
    width: u32,
    height: u32,
    format: PixelFormat,
) -> Result<VideoFrame, VideoError> {
    convert_in(pool(), frame, width, height, format)
}

/// [`convert`] with the output memory from `pool`.
pub(crate) fn convert_in(
    pool: &Arc<Pool>,
    frame: &VideoFrame,
    width: u32,
    height: u32,
    format: PixelFormat,
) -> Result<VideoFrame, VideoError> {
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        return Err(invalid(format!("convert to {width}x{height}")));
    }
    if format == PixelFormat::Native {
        return Err(invalid(
            "convert to Native: only a backend makes native buffers",
        ));
    }
    let same_size = (width, height) == (frame.width, frame.height);
    if let FrameBuffer::Planes(p) = &frame.buffer {
        if same_size && p.format == format {
            p.validate(frame.width, frame.height)?;
            let mut out = frame.clone();
            out.buffer = FrameBuffer::Planes(p.to_tight(width, height));
            return Ok(out);
        }
    }
    if matches!(format, PixelFormat::I420 | PixelFormat::Nv12) {
        let mut to_pool = |src: &SourceView<'_>, sw: u32, sh: u32| {
            if (sw, sh) != (frame.width, frame.height) {
                return Err(invalid(format!(
                    "a {}x{} frame around a {sw}x{sh} buffer",
                    frame.width, frame.height
                )));
            }
            pooled_frame(
                pool,
                src,
                (sw, sh),
                (width, height),
                format,
                frame.color,
                frame.capture_us,
            )
        };
        if let Some(r) = with_view(frame, &mut to_pool) {
            return r;
        }
    }
    let src = cpu_planes(frame, format)?;
    if same_size && src.format == format {
        let mut out = frame.clone();
        out.buffer = FrameBuffer::Planes(src.to_tight(width, height));
        return Ok(out);
    }
    let yuv = to_yuv(&src, frame.width, frame.height, frame.color)?;
    let yuv = if same_size {
        yuv
    } else {
        scale_yuv(&yuv, width as usize, height as usize)
    };
    let planes = from_yuv(&yuv, format, frame.color);
    let mut out = VideoFrame::from_planes(planes, width, height, frame.capture_us)?;
    out.color = frame.color;
    Ok(out)
}

/// The frame's CPU planes, checked (a native buffer is read back, in
/// `prefer` when it can).
fn cpu_planes(frame: &VideoFrame, prefer: PixelFormat) -> Result<Planes, VideoError> {
    let p = match &frame.buffer {
        FrameBuffer::Planes(p) => p.clone(),
        FrameBuffer::Native(n) => {
            let want = if prefer == PixelFormat::Native {
                n.0.format()
            } else {
                prefer
            };
            n.0.to_planes(want)?
        }
    };
    p.validate(frame.width, frame.height)?;
    Ok(p)
}

fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// RGB → (Y, U, V) in a colour space (Android's screen frames, and tests).
#[cfg(any(all(feature = "android", target_os = "android"), test))]
pub(crate) fn rgb_to_yuv(cs: ColorSpace, r: i32, g: i32, b: i32) -> (u8, u8, u8) {
    let k = RgbCoeffs::of(cs);
    let (u, v) = k.chroma(r, g, b);
    (k.luma(r, g, b), u, v)
}

fn yuv_to_rgb(cs: ColorSpace, y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let (d, e) = (u as i32 - 128, v as i32 - 128);
    match cs {
        ColorSpace::Bt709Limited => {
            let c = 298 * (y as i32 - 16);
            (
                clamp((c + 459 * e + 128) >> 8),
                clamp((c - 55 * d - 136 * e + 128) >> 8),
                clamp((c + 541 * d + 128) >> 8),
            )
        }
        ColorSpace::Bt709Full => {
            let c = 256 * y as i32;
            (
                clamp((c + 403 * e + 128) >> 8),
                clamp((c - 48 * d - 120 * e + 128) >> 8),
                clamp((c + 475 * d + 128) >> 8),
            )
        }
        _ => {
            let c = 298 * (y as i32 - 16);
            (
                clamp((c + 409 * e + 128) >> 8),
                clamp((c - 100 * d - 208 * e + 128) >> 8),
                clamp((c + 516 * d + 128) >> 8),
            )
        }
    }
}

pub(crate) fn to_yuv(
    p: &Planes,
    width: u32,
    height: u32,
    cs: ColorSpace,
) -> Result<Yuv, VideoError> {
    p.validate(width, height)?;
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = Yuv {
        w,
        h,
        y: vec![0; w * h],
        u: vec![0; cw * ch],
        v: vec![0; cw * ch],
    };
    match p.format {
        PixelFormat::I420 | PixelFormat::Nv12 => {
            for y in 0..h {
                out.y[y * w..][..w].copy_from_slice(p.row(0, y, width, height));
            }
            for y in 0..ch {
                if p.format == PixelFormat::I420 {
                    out.u[y * cw..][..cw].copy_from_slice(p.row(1, y, width, height));
                    out.v[y * cw..][..cw].copy_from_slice(p.row(2, y, width, height));
                } else {
                    let r = p.row(1, y, width, height);
                    for x in 0..cw {
                        out.u[y * cw + x] = r[2 * x];
                        out.v[y * cw + x] = r[2 * x + 1];
                    }
                }
            }
        }
        PixelFormat::Bgra | PixelFormat::Rgba => {
            let SourceView::Rgb { plane, bgra } = SourceView::of_planes(p) else {
                return Err(invalid("RGB planes read as YUV"));
            };
            rgb_into(plane, bgra, w, h, cs, &mut out);
        }
        PixelFormat::Native => return Err(invalid("CPU planes can't be Native")),
    }
    Ok(out)
}

/// RGB to Y, U and V in integers (x256), as [`rgb_to_yuv`] has them per
/// colour space, so a loop can take them out of its match.
#[derive(Clone, Copy)]
struct RgbCoeffs {
    y: [i32; 3],
    y_off: i32,
    u: [i32; 3],
    v: [i32; 3],
}

impl RgbCoeffs {
    fn of(cs: ColorSpace) -> RgbCoeffs {
        match cs {
            ColorSpace::Bt709Limited => RgbCoeffs {
                y: [47, 157, 16],
                y_off: 16,
                u: [-26, -87, 112],
                v: [112, -102, -10],
            },
            ColorSpace::Bt709Full => RgbCoeffs {
                y: [54, 183, 18],
                y_off: 0,
                u: [-29, -99, 128],
                v: [128, -116, -12],
            },
            _ => RgbCoeffs {
                y: [66, 129, 25],
                y_off: 16,
                u: [-38, -74, 112],
                v: [112, -94, -18],
            },
        }
    }

    fn luma(&self, r: i32, g: i32, b: i32) -> u8 {
        clamp(((self.y[0] * r + self.y[1] * g + self.y[2] * b + 128) >> 8) + self.y_off)
    }

    fn chroma(&self, r: i32, g: i32, b: i32) -> (u8, u8) {
        (
            clamp(((self.u[0] * r + self.u[1] * g + self.u[2] * b + 128) >> 8) + 128),
            clamp(((self.v[0] * r + self.v[1] * g + self.v[2] * b + 128) >> 8) + 128),
        )
    }
}

/// Packed RGB (`w` x `h`, checked by the caller) into `out` as I420 at the
/// same size: luma per pixel, chroma from each 2x2 block's average.
fn rgb_into(p: PlaneView<'_>, bgra: bool, w: usize, h: usize, cs: ColorSpace, out: &mut Yuv) {
    out.reset(w, h);
    let (cw, ch) = (out.cw(), out.ch());
    let (ri, bi) = if bgra { (2, 0) } else { (0, 2) };
    let k = RgbCoeffs::of(cs);
    let row = |y: usize| &p.data[y * p.stride..][..w * 4];
    for y in 0..h {
        let luma = &mut out.y[y * w..][..w];
        for (o, px) in luma.iter_mut().zip(row(y).chunks_exact(4)) {
            *o = k.luma(px[ri] as i32, px[1] as i32, px[bi] as i32);
        }
    }
    for y in 0..ch {
        let r0 = row(2 * y);
        let r1 = (2 * y + 1 < h).then(|| row(2 * y + 1));
        for x in 0..cw {
            let (mut r, mut g, mut b, mut n) = (0, 0, 0, 0);
            for rr in [Some(r0), r1].into_iter().flatten() {
                for xx in 2 * x..(2 * x + 2).min(w) {
                    let px = &rr[4 * xx..];
                    r += px[ri] as i32;
                    g += px[1] as i32;
                    b += px[bi] as i32;
                    n += 1;
                }
            }
            let (u, v) = k.chroma(r / n, g / n, b / n);
            out.u[y * cw + x] = u;
            out.v[y * cw + x] = v;
        }
    }
}

pub(crate) fn from_yuv(yuv: &Yuv, format: PixelFormat, cs: ColorSpace) -> Planes {
    let (w, h, cw) = (yuv.w, yuv.h, yuv.cw());
    let mut data = Vec::with_capacity(format.frame_len(w as u32, h as u32));
    match format {
        PixelFormat::I420 => {
            data.extend_from_slice(&yuv.y);
            data.extend_from_slice(&yuv.u);
            data.extend_from_slice(&yuv.v);
        }
        PixelFormat::Nv12 => {
            data.extend_from_slice(&yuv.y);
            for (u, v) in yuv.u.iter().zip(&yuv.v) {
                data.push(*u);
                data.push(*v);
            }
        }
        _ => {
            let bgra = format == PixelFormat::Bgra;
            for y in 0..h {
                for x in 0..w {
                    let c = (y / 2) * cw + x / 2;
                    let (r, g, b) = yuv_to_rgb(cs, yuv.y[y * w + x], yuv.u[c], yuv.v[c]);
                    if bgra {
                        data.extend_from_slice(&[b, g, r, 255]);
                    } else {
                        data.extend_from_slice(&[r, g, b, 255]);
                    }
                }
            }
        }
    }
    let (strides, offsets) = crate::frame::tight_layout(format, w as u32, h as u32);
    Planes {
        format,
        data: data.into(),
        strides,
        offsets,
    }
}

pub(crate) fn scale_yuv(src: &Yuv, w: usize, h: usize) -> Yuv {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = Yuv {
        w,
        h,
        y: vec![0; w * h],
        u: vec![0; cw * ch],
        v: vec![0; cw * ch],
    };
    scale_plane(&src.y, src.w, src.h, &mut out.y, w, h);
    scale_plane(&src.u, src.cw(), src.ch(), &mut out.u, cw, ch);
    scale_plane(&src.v, src.cw(), src.ch(), &mut out.v, cw, ch);
    out
}

fn scale_plane(src: &[u8], sw: usize, sh: usize, dst: &mut [u8], dw: usize, dh: usize) {
    scale_view(PlaneView::new(src, sw, 1), sw, sh, dst, dw, 1, dw, dh);
}

/// Scale one plane: `src` is `sw` x `sh` samples; `dst` gets `dw` x `dh`,
/// rows `dstride` bytes apart and samples `dstep` apart. A copy at the same
/// size; when shrinking by 2 or more on either axis, each output sample is
/// its source box's rounded average (exactly 2:1 has its own loop, with the
/// same result); otherwise bilinear. Both sides are checked by the caller.
#[allow(clippy::too_many_arguments)]
fn scale_view(
    src: PlaneView<'_>,
    sw: usize,
    sh: usize,
    dst: &mut [u8],
    dstride: usize,
    dstep: usize,
    dw: usize,
    dh: usize,
) {
    let (ss, sp) = (src.stride, src.step);
    if (sw, sh) == (dw, dh) {
        for y in 0..dh {
            let s = &src.data[y * ss..];
            let d = &mut dst[y * dstride..];
            if sp == 1 && dstep == 1 {
                d[..dw].copy_from_slice(&s[..dw]);
            } else {
                for x in 0..dw {
                    d[x * dstep] = s[x * sp];
                }
            }
        }
        return;
    }
    if sw == 2 * dw && sh == 2 * dh {
        for y in 0..dh {
            let r0 = &src.data[2 * y * ss..];
            let r1 = &src.data[(2 * y + 1) * ss..];
            let d = &mut dst[y * dstride..];
            for x in 0..dw {
                let a = 2 * x * sp;
                let b = a + sp;
                let sum = r0[a] as u32 + r0[b] as u32 + r1[a] as u32 + r1[b] as u32;
                d[x * dstep] = ((sum + 2) / 4) as u8;
            }
        }
        return;
    }
    let (fx, fy) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
    let get = |x: usize, y: usize| src.data[y * ss + x * sp] as u32;
    if fx >= 2.0 || fy >= 2.0 {
        // Shrinking a lot: average each source box, so edges don't alias.
        for y in 0..dh {
            let y1 = (((y + 1) as f32 * fy) as usize).clamp(1, sh);
            let y0 = ((y as f32 * fy) as usize).min(y1 - 1);
            for x in 0..dw {
                let x1 = (((x + 1) as f32 * fx) as usize).clamp(1, sw);
                let x0 = ((x as f32 * fx) as usize).min(x1 - 1);
                let mut sum = 0u32;
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        sum += get(xx, yy);
                    }
                }
                let n = ((x1 - x0) * (y1 - y0)) as u32;
                dst[y * dstride + x * dstep] = ((sum + n / 2) / n) as u8;
            }
        }
        return;
    }
    for y in 0..dh {
        let sy = ((y as f32 + 0.5) * fy - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ay) = (sy as usize, sy - sy.floor());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..dw {
            let sx = ((x as f32 + 0.5) * fx - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, ax) = (sx as usize, sx - sx.floor());
            let x1 = (x0 + 1).min(sw - 1);
            let top = get(x0, y0) as f32 * (1.0 - ax) + get(x1, y0) as f32 * ax;
            let bottom = get(x0, y1) as f32 * (1.0 - ax) + get(x1, y1) as f32 * ax;
            dst[y * dstride + x * dstep] = (top * (1.0 - ay) + bottom * ay + 0.5) as u8;
        }
    }
}

/// The size that fits `width` x `height` inside `box_w` x `box_h` keeping
/// the aspect ratio, rounded to even numbers (at least 2).
pub fn fit_within(width: u32, height: u32, box_w: u32, box_h: u32) -> (u32, u32) {
    if width <= box_w && height <= box_h {
        return (width, height);
    }
    let s = (box_w as f64 / width as f64).min(box_h as f64 / height as f64);
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    (even(width as f64 * s), even(height as f64 * s))
}

/// The luma PSNR between two frames of the same size, in dB (100 for
/// identical frames). For tests and quality checks.
pub fn psnr_y(a: &VideoFrame, b: &VideoFrame) -> Result<f64, VideoError> {
    if (a.width, a.height) != (b.width, b.height) {
        return Err(invalid(format!(
            "{}x{} vs {}x{}",
            a.width, a.height, b.width, b.height
        )));
    }
    let ya = to_yuv(
        &cpu_planes(a, PixelFormat::I420)?,
        a.width,
        a.height,
        a.color,
    )?;
    let yb = to_yuv(
        &cpu_planes(b, PixelFormat::I420)?,
        b.width,
        b.height,
        b.color,
    )?;
    let sse: u64 =
        ya.y.iter()
            .zip(&yb.y)
            .map(|(p, q)| ((*p as i64 - *q as i64).pow(2)) as u64)
            .sum();
    if sse == 0 {
        return Ok(100.0);
    }
    let mse = sse as f64 / ya.y.len() as f64;
    Ok(10.0 * (255.0 * 255.0 / mse).log10())
}

/// Luma at (`x`, `y`) of a CPU frame (tests and the fake's frame code).
pub(crate) fn luma_plane(frame: &VideoFrame) -> Result<(Vec<u8>, usize), VideoError> {
    let yuv = to_yuv(
        &cpu_planes(frame, PixelFormat::I420)?,
        frame.width,
        frame.height,
        frame.color,
    )?;
    Ok((yuv.y, yuv.w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> VideoFrame {
        let (cw, ch) = ((w as usize).div_ceil(2), (h as usize).div_ceil(2));
        let mut buf = Vec::new();
        for y in 0..h as usize {
            for x in 0..w as usize {
                buf.push((40 + x * 2 + y) as u8);
            }
        }
        for y in 0..ch {
            for x in 0..cw {
                buf.push((100 + x + y % 2) as u8);
            }
        }
        for y in 0..ch {
            for x in 0..cw {
                buf.push((140 + y / 2 + x % 3) as u8);
            }
        }
        VideoFrame::from_planes(
            Planes::tight(PixelFormat::I420, w, h, buf).expect("planes"),
            w,
            h,
            5,
        )
        .expect("frame")
    }

    #[test]
    fn nv12_and_i420_convert_both_ways_exactly() {
        for (w, h) in [(16, 8), (7, 5)] {
            let a = gradient(w, h);
            let n = convert(&a, w, h, PixelFormat::Nv12).expect("nv12");
            assert_eq!(n.format(), PixelFormat::Nv12);
            let back = convert(&n, w, h, PixelFormat::I420).expect("i420");
            assert_eq!(back.planes(), a.planes());
            assert_eq!(back.capture_us, 5);
        }
    }

    #[test]
    fn rgb_round_trips_closely() {
        let a = gradient(32, 16);
        for f in [PixelFormat::Bgra, PixelFormat::Rgba] {
            let rgb = convert(&a, 32, 16, f).expect("rgb");
            let back = convert(&rgb, 32, 16, PixelFormat::I420).expect("back");
            let p = psnr_y(&a, &back).expect("psnr");
            assert!(p > 40.0, "{f:?}: {p:.1} dB");
        }
        // Pure colours land where BT.601 puts them.
        let white = VideoFrame::filled(PixelFormat::Bgra, 4, 4, [235, 128, 128], 0).expect("white");
        assert!(white.planes().expect("p").data.iter().all(|&b| b == 255));
        let black = VideoFrame::filled(PixelFormat::Rgba, 4, 4, [16, 128, 128], 0).expect("black");
        assert!(black
            .planes()
            .expect("p")
            .data
            .chunks(4)
            .all(|px| px == [0, 0, 0, 255]));
    }

    #[test]
    fn scaling_keeps_flat_colour_and_follows_gradients() {
        let flat = VideoFrame::filled(PixelFormat::Nv12, 64, 48, [100, 90, 160], 9).expect("flat");
        for (w, h) in [(32, 24), (16, 12), (96, 72), (31, 17)] {
            let s = convert(&flat, w, h, PixelFormat::Nv12).expect("scale");
            assert_eq!((s.width, s.height, s.capture_us), (w, h, 9));
            let p = s.planes().expect("planes");
            assert!(p.data[..(w * h) as usize].iter().all(|&v| v == 100));
            assert_eq!(&p.data[(w * h) as usize..][..2], &[90, 160]);
            let back = convert(&s, 64, 48, PixelFormat::I420).expect("back");
            assert_eq!(psnr_y(&flat, &back).expect("psnr"), 100.0);
        }
        let g = gradient(64, 64);
        let half = convert(&g, 32, 32, PixelFormat::I420).expect("half");
        let up = convert(&half, 64, 64, PixelFormat::I420).expect("up");
        let p = psnr_y(&g, &up).expect("psnr");
        assert!(
            p > 25.0,
            "a smooth gradient survives half and back: {p:.1} dB"
        );
        assert!(convert(&g, 0, 4, PixelFormat::I420).is_err());
        assert!(convert(&g, 4, 4, PixelFormat::Native).is_err());
    }

    #[test]
    fn fit_keeps_the_aspect_and_even_sizes() {
        assert_eq!(fit_within(1280, 720, 640, 640), (640, 360));
        assert_eq!(fit_within(1280, 720, 320, 100), (178, 100));
        assert_eq!(
            fit_within(320, 240, 640, 480),
            (320, 240),
            "never scales up"
        );
    }

    #[test]
    fn psnr_needs_the_same_size() {
        let a = VideoFrame::filled(PixelFormat::I420, 8, 8, [16, 128, 128], 0).expect("a");
        let b = VideoFrame::filled(PixelFormat::I420, 8, 8, [26, 128, 128], 0).expect("b");
        let p = psnr_y(&a, &b).expect("psnr");
        assert!((p - 28.13).abs() < 0.01, "{p}");
        let small = convert(&a, 4, 4, PixelFormat::I420).expect("s");
        assert!(psnr_y(&a, &small).is_err());
    }

    /// What `convert` gave before it read sources in place: everything
    /// through tight I420 at the source size, scaled, then packed.
    fn reference(frame: &VideoFrame, w: u32, h: u32, format: PixelFormat) -> Planes {
        let src = frame.planes().expect("cpu planes");
        let yuv = to_yuv(src, frame.width, frame.height, frame.color).expect("yuv");
        let yuv = if (w, h) == (frame.width, frame.height) {
            yuv
        } else {
            scale_yuv(&yuv, w as usize, h as usize)
        };
        from_yuv(&yuv, format, frame.color)
    }

    /// A busy picture: luma and chroma vary in both directions, so any
    /// misplaced sample shows.
    fn busy(w: u32, h: u32) -> VideoFrame {
        let (cw, ch) = ((w as usize).div_ceil(2), (h as usize).div_ceil(2));
        let mut buf = Vec::new();
        for y in 0..h as usize {
            for x in 0..w as usize {
                buf.push(((x * 7 + y * 13) ^ (x * y)) as u8);
            }
        }
        for y in 0..ch {
            for x in 0..cw {
                buf.push((60 + (x * 3 + y * 5) % 140) as u8);
            }
        }
        for y in 0..ch {
            for x in 0..cw {
                buf.push((200 - (x * 5 + y * 2) % 150) as u8);
            }
        }
        VideoFrame::from_planes(
            Planes::tight(PixelFormat::I420, w, h, buf).expect("planes"),
            w,
            h,
            42,
        )
        .expect("frame")
    }

    #[test]
    fn reading_in_place_gives_the_same_pixels_as_before() {
        // (source size, target size): camera sizes to the VP8 backup's
        // 360p and 180p, portrait, the same size, odd sizes, bilinear and
        // upscaling.
        let sizes = [
            ((1280, 720), (640, 360)),
            ((1280, 720), (320, 180)),
            ((1280, 720), (1280, 720)),
            ((720, 1280), (360, 640)),
            ((720, 1280), (180, 320)),
            ((1920, 1080), (640, 360)),
            ((64, 48), (48, 36)),
            ((33, 17), (16, 9)),
            ((7, 5), (8, 6)),
            ((7, 5), (7, 5)),
        ];
        for ((sw, sh), (dw, dh)) in sizes {
            let i420 = busy(sw, sh);
            for src_format in [
                PixelFormat::I420,
                PixelFormat::Nv12,
                PixelFormat::Bgra,
                PixelFormat::Rgba,
            ] {
                let mut src = if src_format == PixelFormat::I420 {
                    i420.clone()
                } else {
                    VideoFrame::from_planes(reference(&i420, sw, sh, src_format), sw, sh, 42)
                        .expect("src")
                };
                src.color = ColorSpace::Bt709Limited;
                for to in [PixelFormat::I420, PixelFormat::Nv12] {
                    let out = convert(&src, dw, dh, to).expect("convert");
                    assert_eq!((out.width, out.height, out.capture_us), (dw, dh, 42));
                    assert_eq!(out.color, ColorSpace::Bt709Limited);
                    assert_eq!(
                        out.planes().expect("planes"),
                        &reference(&src, dw, dh, to),
                        "{src_format:?} {sw}x{sh} -> {to:?} {dw}x{dh}"
                    );
                }
            }
        }
    }

    /// Android's YUV_420_888 layouts (and an AHardwareBuffer's locked
    /// planes): padded rows, planar chroma or interleaved (NV12 and NV21).
    #[test]
    fn strided_and_interleaved_views_read_like_tight_planes() {
        let (w, h) = (1280usize, 720usize);
        let src = busy(w as u32, h as u32);
        let p = src.planes().expect("planes");
        let (cw, ch) = (w / 2, h / 2);
        let plane = |i: usize, row: usize| -> Vec<u8> {
            (0..if i == 0 { h } else { ch })
                .flat_map(|y| p.row(i, y, w as u32, h as u32)[..row].to_vec())
                .collect()
        };
        let (y, u, v) = (plane(0, w), plane(1, cw), plane(2, cw));
        let pad = 64;
        let mut ys = vec![0u8; (w + pad) * h];
        for r in 0..h {
            ys[r * (w + pad)..][..w].copy_from_slice(&y[r * w..][..w]);
        }
        // Planar chroma with padded rows.
        let mut us = vec![0u8; (cw + pad) * ch];
        let mut vs = vec![0u8; (cw + pad) * ch];
        // Interleaved V then U (NV21), padded rows.
        let mut vu = vec![0u8; (w + pad) * ch];
        for r in 0..ch {
            us[r * (cw + pad)..][..cw].copy_from_slice(&u[r * cw..][..cw]);
            vs[r * (cw + pad)..][..cw].copy_from_slice(&v[r * cw..][..cw]);
            for x in 0..cw {
                vu[r * (w + pad) + 2 * x] = v[r * cw + x];
                vu[r * (w + pad) + 2 * x + 1] = u[r * cw + x];
            }
        }
        let y_view = PlaneView::new(&ys, w + pad, 1);
        let layouts = [
            SourceView::Yuv([
                y_view,
                PlaneView::new(&us, cw + pad, 1),
                PlaneView::new(&vs, cw + pad, 1),
            ]),
            // Android's views of NV21 memory stop one byte short.
            SourceView::Yuv([
                y_view,
                PlaneView::new(&vu[1..vu.len() - 1], w + pad, 2),
                PlaneView::new(&vu[..vu.len() - 1], w + pad, 2),
            ]),
        ];
        let pool = Arc::new(Pool::default());
        for (n, view) in layouts.iter().enumerate() {
            for ((dw, dh), to) in [
                ((640, 360), PixelFormat::I420),
                ((320, 180), PixelFormat::I420),
                ((1280, 720), PixelFormat::Nv12),
            ] {
                let out = pooled_frame(
                    &pool,
                    view,
                    (w as u32, h as u32),
                    (dw, dh),
                    to,
                    ColorSpace::Bt601Limited,
                    9,
                )
                .expect("pooled");
                assert_eq!(
                    out.planes().expect("planes"),
                    &reference(&src, dw, dh, to),
                    "layout {n} -> {to:?} {dw}x{dh}"
                );
            }
        }
        // Views short of the picture are refused, never read past.
        let short = SourceView::Yuv([
            PlaneView::new(&ys[..ys.len() - pad - 1], w + pad, 1),
            PlaneView::new(&us, cw + pad, 1),
            PlaneView::new(&vs, cw + pad, 1),
        ]);
        let r = pooled_frame(
            &pool,
            &short,
            (w as u32, h as u32),
            (640, 360),
            PixelFormat::I420,
            ColorSpace::Bt601Limited,
            0,
        );
        assert!(matches!(r, Err(VideoError::Invalid(_))), "{r:?}");
        let rgb = SourceView::Rgb {
            plane: PlaneView::new(&ys, w * 4, 4),
            bgra: true,
        };
        let r = pooled_frame(
            &pool,
            &rgb,
            (w as u32, h as u32),
            (640, 360),
            PixelFormat::I420,
            ColorSpace::Bt601Limited,
            0,
        );
        assert!(matches!(r, Err(VideoError::Invalid(_))), "{r:?}");
    }

    #[test]
    fn output_memory_is_reused_once_frames_drop() {
        let pool = Arc::new(Pool::default());
        let src = busy(1280, 720);
        let first = convert_in(&pool, &src, 640, 360, PixelFormat::I420).expect("first");
        let at = first.planes().expect("planes").data.as_ptr();
        assert!(pool.idle().is_empty(), "held by the frame");
        let clone = first.clone();
        drop(first);
        assert!(pool.idle().is_empty(), "a clone still holds it");
        drop(clone);
        assert_eq!(pool.idle(), vec![at], "back once the last clone drops");
        for _ in 0..30 {
            let f = convert_in(&pool, &src, 640, 360, PixelFormat::I420).expect("again");
            assert_eq!(f.planes().expect("planes").data.as_ptr(), at, "reused");
        }
        // Two alive at once: two buffers, both kept after.
        let a = convert_in(&pool, &src, 640, 360, PixelFormat::Nv12).expect("a");
        let b = convert_in(&pool, &src, 320, 180, PixelFormat::I420).expect("b");
        assert_ne!(
            a.planes().expect("a").data.as_ptr(),
            b.planes().expect("b").data.as_ptr()
        );
        drop((a, b));
        assert_eq!(pool.idle().len(), 2);
        // A smaller frame takes the smallest buffer that fits.
        let small = convert_in(&pool, &src, 320, 180, PixelFormat::I420).expect("small");
        assert_eq!(pool.idle().len(), 1);
        drop(small);
        // At most POOL_KEEP idle buffers; the smallest go first.
        let held: Vec<VideoFrame> = (0..POOL_KEEP as u32 + 3)
            .map(|i| convert_in(&pool, &src, 64 + 2 * i, 36, PixelFormat::I420).expect("many"))
            .collect();
        drop(held);
        assert_eq!(pool.idle().len(), POOL_KEEP);
        // The same size and format is still a cheap clone, not a copy.
        let same = convert_in(&pool, &src, 1280, 720, PixelFormat::I420).expect("same");
        assert_eq!(
            same.planes().expect("same").data.as_ptr(),
            src.planes().expect("src").data.as_ptr()
        );
    }

    /// The scaler as it was before the in-place reads: every fast path
    /// (same size, 2:1) and every view must land on its bytes.
    fn old_scale_plane(src: &[u8], sw: usize, sh: usize, dst: &mut [u8], dw: usize, dh: usize) {
        let (fx, fy) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
        let get = |x: usize, y: usize| src[y * sw + x] as u32;
        if fx >= 2.0 || fy >= 2.0 {
            for y in 0..dh {
                let y1 = (((y + 1) as f32 * fy) as usize).clamp(1, sh);
                let y0 = ((y as f32 * fy) as usize).min(y1 - 1);
                for x in 0..dw {
                    let x1 = (((x + 1) as f32 * fx) as usize).clamp(1, sw);
                    let x0 = ((x as f32 * fx) as usize).min(x1 - 1);
                    let mut sum = 0u32;
                    for yy in y0..y1 {
                        for xx in x0..x1 {
                            sum += get(xx, yy);
                        }
                    }
                    let n = ((x1 - x0) * (y1 - y0)) as u32;
                    dst[y * dw + x] = ((sum + n / 2) / n) as u8;
                }
            }
            return;
        }
        for y in 0..dh {
            let sy = ((y as f32 + 0.5) * fy - 0.5).clamp(0.0, (sh - 1) as f32);
            let (y0, ay) = (sy as usize, sy - sy.floor());
            let y1 = (y0 + 1).min(sh - 1);
            for x in 0..dw {
                let sx = ((x as f32 + 0.5) * fx - 0.5).clamp(0.0, (sw - 1) as f32);
                let (x0, ax) = (sx as usize, sx - sx.floor());
                let x1 = (x0 + 1).min(sw - 1);
                let top = get(x0, y0) as f32 * (1.0 - ax) + get(x1, y0) as f32 * ax;
                let bottom = get(x0, y1) as f32 * (1.0 - ax) + get(x1, y1) as f32 * ax;
                dst[y * dw + x] = (top * (1.0 - ay) + bottom * ay + 0.5) as u8;
            }
        }
    }

    #[test]
    fn every_scaling_path_matches_the_original_arithmetic() {
        let sizes = [
            ((64, 36), (64, 36)),
            ((64, 36), (32, 18)),
            ((64, 36), (16, 9)),
            ((60, 36), (20, 12)),
            ((64, 36), (32, 36)),
            ((64, 36), (64, 12)),
            ((63, 35), (21, 7)),
            ((64, 36), (48, 27)),
            ((64, 36), (30, 17)),
            ((33, 17), (16, 9)),
            ((7, 5), (8, 6)),
            ((5, 3), (1, 1)),
        ];
        for ((sw, sh), (dw, dh)) in sizes {
            let src: Vec<u8> = (0..sw * sh)
                .map(|i| ((i * 37) ^ (i / sw * 11)) as u8)
                .collect();
            let mut want = vec![0u8; dw * dh];
            old_scale_plane(&src, sw, sh, &mut want, dw, dh);
            let mut got = vec![0u8; dw * dh];
            scale_plane(&src, sw, sh, &mut got, dw, dh);
            assert_eq!(got, want, "{sw}x{sh} -> {dw}x{dh}");
            // Through a padded, interleaved view into an interleaved output.
            let mut wide = vec![0u8; (sw * 2 + 6) * sh];
            for y in 0..sh {
                for x in 0..sw {
                    wide[y * (sw * 2 + 6) + 2 * x + 1] = src[y * sw + x];
                }
            }
            let mut out = vec![0u8; dw * dh * 2];
            scale_view(
                PlaneView::new(&wide[1..], sw * 2 + 6, 2),
                sw,
                sh,
                &mut out,
                dw * 2,
                2,
                dw,
                dh,
            );
            let picked: Vec<u8> = out.iter().step_by(2).copied().collect();
            assert_eq!(picked, want, "{sw}x{sh} -> {dw}x{dh}, interleaved");
        }
    }

    /// RGB to YUV as the formulas were first written out.
    fn old_rgb_to_yuv(cs: ColorSpace, r: i32, g: i32, b: i32) -> (u8, u8, u8) {
        match cs {
            ColorSpace::Bt709Limited => (
                clamp(((47 * r + 157 * g + 16 * b + 128) >> 8) + 16),
                clamp(((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128),
                clamp(((112 * r - 102 * g - 10 * b + 128) >> 8) + 128),
            ),
            ColorSpace::Bt709Full => (
                clamp((54 * r + 183 * g + 18 * b + 128) >> 8),
                clamp(((-29 * r - 99 * g + 128 * b + 128) >> 8) + 128),
                clamp(((128 * r - 116 * g - 12 * b + 128) >> 8) + 128),
            ),
            _ => (
                clamp(((66 * r + 129 * g + 25 * b + 128) >> 8) + 16),
                clamp(((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128),
                clamp(((112 * r - 94 * g - 18 * b + 128) >> 8) + 128),
            ),
        }
    }

    #[test]
    fn rgb_coefficients_match_rgb_to_yuv() {
        for cs in [
            ColorSpace::Bt601Limited,
            ColorSpace::Bt709Limited,
            ColorSpace::Bt709Full,
        ] {
            let k = RgbCoeffs::of(cs);
            for r in (0..256).step_by(15) {
                for g in (0..256).step_by(17) {
                    for b in (0..256).step_by(13) {
                        let (y, u, v) = old_rgb_to_yuv(cs, r, g, b);
                        assert_eq!(rgb_to_yuv(cs, r, g, b), (y, u, v));
                        assert_eq!(k.luma(r, g, b), y, "{cs:?} {r} {g} {b}");
                        assert_eq!(k.chroma(r, g, b), (u, v), "{cs:?} {r} {g} {b}");
                    }
                }
            }
        }
    }
}
