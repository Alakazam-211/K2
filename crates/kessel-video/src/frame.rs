//! Raw frames: pixel formats, CPU planes and OS-native buffers.

use crate::error::{invalid, VideoError};
use bytes::Bytes;
use std::any::Any;
use std::sync::Arc;

/// Pixel layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PixelFormat {
    /// Planar 4:2:0, BT.601 limited range unless [`VideoFrame::color`] says
    /// otherwise.
    I420,
    /// Semi-planar 4:2:0 (Y plane, interleaved UV).
    Nv12,
    /// 8-bit BGRA (screen capture on Windows and macOS).
    Bgra,
    /// 8-bit RGBA (the decoded-frame fallback to the webview).
    Rgba,
    /// An OS buffer ([`NativeBuffer`]); see [`NativeFrame::format`].
    Native,
}

impl PixelFormat {
    /// The number of CPU planes (0 for `Native`).
    pub fn plane_count(&self) -> usize {
        match self {
            PixelFormat::I420 => 3,
            PixelFormat::Nv12 => 2,
            PixelFormat::Bgra | PixelFormat::Rgba => 1,
            PixelFormat::Native => 0,
        }
    }

    /// Bytes per row and rows of plane `i` of a `width` x `height` frame
    /// (chroma rounds up).
    pub fn plane_size(&self, i: usize, width: u32, height: u32) -> (usize, usize) {
        let (w, h) = (width as usize, height as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        match (self, i) {
            (PixelFormat::I420 | PixelFormat::Nv12, 0) => (w, h),
            (PixelFormat::I420, 1 | 2) => (cw, ch),
            (PixelFormat::Nv12, 1) => (cw * 2, ch),
            (PixelFormat::Bgra | PixelFormat::Rgba, 0) => (w * 4, h),
            _ => (0, 0),
        }
    }

    /// The bytes of a tightly packed frame.
    pub fn frame_len(&self, width: u32, height: u32) -> usize {
        (0..self.plane_count())
            .map(|i| {
                let (r, n) = self.plane_size(i, width, height);
                r * n
            })
            .sum()
    }
}

/// Colour description (v1 is BT.601/709 limited range).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ColorSpace {
    /// BT.601, limited range (16-235). The default.
    #[default]
    Bt601Limited,
    /// BT.709, limited range.
    Bt709Limited,
    /// BT.709, full range (0-255).
    Bt709Full,
}

/// CPU planes. `data` holds every plane; plane `i` starts at `offsets[i]`
/// with row stride `strides[i]`. Unused planes have stride 0 (NV12 uses 2;
/// BGRA and RGBA use 1).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Planes {
    /// The layout.
    pub format: PixelFormat,
    /// Every plane.
    pub data: Bytes,
    /// Bytes per row of each plane.
    pub strides: [u32; 3],
    /// Where each plane starts in `data`.
    pub offsets: [usize; 3],
}

impl Planes {
    /// Planes with explicit layout, checked against a `width` x `height`
    /// frame.
    pub fn new(
        format: PixelFormat,
        width: u32,
        height: u32,
        data: impl Into<Bytes>,
        strides: [u32; 3],
        offsets: [usize; 3],
    ) -> Result<Planes, VideoError> {
        let p = Planes {
            format,
            data: data.into(),
            strides,
            offsets,
        };
        p.validate(width, height)?;
        Ok(p)
    }

    /// Tightly packed planes (one after another, no row padding).
    pub fn tight(
        format: PixelFormat,
        width: u32,
        height: u32,
        data: impl Into<Bytes>,
    ) -> Result<Planes, VideoError> {
        let data = data.into();
        let want = format.frame_len(width, height);
        if data.len() != want || format == PixelFormat::Native {
            return Err(invalid(format!(
                "a tight {format:?} {width}x{height} frame is {want} bytes, not {}",
                data.len()
            )));
        }
        let (strides, offsets) = tight_layout(format, width, height);
        Planes::new(format, width, height, data, strides, offsets)
    }

    /// Check that every plane of a `width` x `height` frame fits in `data`.
    pub fn validate(&self, width: u32, height: u32) -> Result<(), VideoError> {
        if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
            return Err(invalid(format!("a {width}x{height} frame")));
        }
        if self.format == PixelFormat::Native {
            return Err(invalid("CPU planes can't be Native"));
        }
        for i in 0..self.format.plane_count() {
            let (row, rows) = self.format.plane_size(i, width, height);
            let stride = self.strides[i] as usize;
            if stride < row {
                return Err(invalid(format!(
                    "plane {i}: stride {stride} is shorter than its {row}-byte row"
                )));
            }
            // Public fields: any offset or stride can arrive here, so the
            // end is computed without overflow.
            let end = stride
                .checked_mul(rows.saturating_sub(1))
                .and_then(|n| n.checked_add(self.offsets[i]))
                .and_then(|n| n.checked_add(row));
            match end {
                Some(end) if end <= self.data.len() => {}
                _ => {
                    return Err(invalid(format!(
                        "plane {i} (offset {}, stride {stride}) runs past the {}-byte buffer",
                        self.offsets[i],
                        self.data.len()
                    )))
                }
            }
        }
        Ok(())
    }

    /// Row `y` of plane `i`, without padding. Call [`Planes::validate`]
    /// first; panics if the row is out of the buffer.
    pub fn row(&self, i: usize, y: usize, width: u32, height: u32) -> &[u8] {
        let (row, _) = self.format.plane_size(i, width, height);
        &self.data[self.offsets[i] + y * self.strides[i] as usize..][..row]
    }

    /// The same pixels, tightly packed (a cheap clone when they already are).
    pub fn to_tight(&self, width: u32, height: u32) -> Planes {
        let (strides, offsets) = tight_layout(self.format, width, height);
        if strides == self.strides
            && offsets == self.offsets
            && self.data.len() == self.format.frame_len(width, height)
        {
            return self.clone();
        }
        let mut out = Vec::with_capacity(self.format.frame_len(width, height));
        for i in 0..self.format.plane_count() {
            let (_, rows) = self.format.plane_size(i, width, height);
            for y in 0..rows {
                out.extend_from_slice(self.row(i, y, width, height));
            }
        }
        Planes {
            format: self.format,
            data: out.into(),
            strides,
            offsets,
        }
    }
}

/// Strides and offsets of tightly packed planes.
pub fn tight_layout(format: PixelFormat, width: u32, height: u32) -> ([u32; 3], [usize; 3]) {
    let mut strides = [0u32; 3];
    let mut offsets = [0usize; 3];
    let mut at = 0usize;
    for i in 0..format.plane_count() {
        let (row, rows) = format.plane_size(i, width, height);
        strides[i] = row as u32;
        offsets[i] = at;
        at += row * rows;
    }
    (strides, offsets)
}

/// An OS-native frame buffer (CVPixelBuffer/IOSurface, an IMFSample or
/// D3D11 texture, an AHardwareBuffer, a DMA-BUF). Opaque to the caller: hand
/// it from a source of a backend to an encoder of the same backend (zero
/// copy), or ask for CPU planes.
pub trait NativeFrame: Send + Sync + 'static {
    /// The layout behind it (NV12 or BGRA in practice).
    fn format(&self) -> PixelFormat;
    /// A CPU copy in `format` (I420, NV12, BGRA or RGBA). May be slow (a
    /// GPU readback).
    fn to_planes(&self, format: PixelFormat) -> Result<Planes, VideoError>;
    /// For the backend's own downcast.
    fn as_any(&self) -> &dyn Any;
}

/// A shared native buffer.
#[derive(Clone)]
pub struct NativeBuffer(pub Arc<dyn NativeFrame>);

impl std::fmt::Debug for NativeBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("NativeBuffer")
            .field(&self.0.format())
            .finish()
    }
}

/// A frame's pixels.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum FrameBuffer {
    /// CPU planes.
    Planes(Planes),
    /// An OS buffer.
    Native(NativeBuffer),
}

/// The host clock a [`HostTime`] counts on. The codes
/// ([`HostClock::as_str`]) are API: one is never renamed or reused for
/// another clock, and a new clock is a new variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HostClock {
    /// macOS and iOS: `mach_absolute_time()`, in nanoseconds through
    /// `mach_timebase_info` (`ticks * numer / denom`). It stops while the
    /// machine sleeps. The same clock as
    /// `clock_gettime_nsec_np(CLOCK_UPTIME_RAW)`, `CACurrentMediaTime()`
    /// (in seconds), `CMClockGetHostTimeClock()`, a `CADisplayLink`'s
    /// `timestamp` and Swift's `DispatchTime.now().uptimeNanoseconds`, so a
    /// delay from capture to any of those is one subtraction.
    MachAbsoluteTime,
}

impl HostClock {
    /// A stable snake_case code for logs and wire formats:
    /// `"mach_absolute_time"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            HostClock::MachAbsoluteTime => "mach_absolute_time",
        }
    }
}

impl std::fmt::Display for HostClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// When a frame was captured: nanoseconds on a named host clock (see
/// [`VideoFrame::host_time`]). The shape is API: a `u64` count of
/// nanoseconds and the [`HostClock`] it counts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct HostTime {
    /// Nanoseconds on `clock`.
    pub nanos: u64,
    /// The clock.
    pub clock: HostClock,
}

impl HostTime {
    /// `nanos` on `clock`.
    pub fn new(nanos: u64, clock: HostClock) -> HostTime {
        HostTime { nanos, clock }
    }
}

/// Mach ticks to nanoseconds with a `mach_timebase_info` (`numer` /
/// `denom`), with no overflow on the way: `None` for a zero `denom` or a
/// result past `u64`.
#[cfg_attr(
    not(all(feature = "apple", any(target_os = "macos", target_os = "ios"))),
    allow(dead_code)
)]
pub(crate) fn mach_ticks_to_nanos(ticks: u64, numer: u32, denom: u32) -> Option<u64> {
    if denom == 0 {
        return None;
    }
    u64::try_from(ticks as u128 * numer as u128 / denom as u128).ok()
}

/// One raw video frame. Always upright: the backend applies the camera's
/// rotation before handing it out.
#[derive(Clone)]
#[non_exhaustive]
pub struct VideoFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Capture time in µs on the backend's monotonic clock
    /// ([`crate::VideoBackend::now_us`]). Encoders and decoders pass it
    /// through unchanged.
    pub capture_us: u64,
    /// Colour description.
    pub color: ColorSpace,
    /// The pixels.
    pub buffer: FrameBuffer,
    /// When the sensor captured the frame, in nanoseconds on a named host
    /// clock: for measuring the delay from capture to anything else on the
    /// same machine (a display layer, an encoder's output, a send). It
    /// doesn't replace `capture_us`, which stays the backend's own clock and
    /// the codecs' timestamp.
    ///
    /// - **macOS and iOS cameras** set it to the sample buffer's
    ///   presentation time (which is on the capture session's
    ///   synchronization clock), moved to the host time clock with
    ///   `CMSyncConvertTime`, taken to mach ticks with
    ///   `CMClockConvertHostTimeToSystemUnits` and to nanoseconds with
    ///   `mach_timebase_info`: [`HostClock::MachAbsoluteTime`]. It is
    ///   `None` for a sample with no valid presentation time, and while the
    ///   session has no clock.
    /// - **Everything else** leaves it `None`: the other platforms' cameras,
    ///   every screen source, decoders, and the fake and null backends. A
    ///   frame-in host may set it on the frames it pushes.
    /// - kessel-video's own scaling, conversion and turning keep it
    ///   ([`crate::convert`], [`crate::VideoBackend::convert`], a source's
    ///   fit to its box, a frame-in turn). Encoders don't carry it: an
    ///   [`crate::EncodedFrame`] has `capture_us` only.
    pub host_time: Option<HostTime>,
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("capture_us", &self.capture_us)
            .field("color", &self.color)
            .field("host_time", &self.host_time)
            .field("format", &self.format())
            .finish()
    }
}

impl VideoFrame {
    /// A frame from CPU planes, checked.
    pub fn from_planes(
        planes: Planes,
        width: u32,
        height: u32,
        capture_us: u64,
    ) -> Result<VideoFrame, VideoError> {
        planes.validate(width, height)?;
        Ok(VideoFrame {
            width,
            height,
            capture_us,
            color: ColorSpace::default(),
            buffer: FrameBuffer::Planes(planes),
            host_time: None,
        })
    }

    /// A frame from an OS buffer.
    pub fn from_native(
        buffer: NativeBuffer,
        width: u32,
        height: u32,
        capture_us: u64,
    ) -> VideoFrame {
        VideoFrame {
            width,
            height,
            capture_us,
            color: ColorSpace::default(),
            buffer: FrameBuffer::Native(buffer),
            host_time: None,
        }
    }

    /// A tight frame of one colour (`y`, `u`, `v`), in I420, NV12, or (as
    /// RGB from the same YUV) BGRA / RGBA.
    pub fn filled(
        format: PixelFormat,
        width: u32,
        height: u32,
        yuv: [u8; 3],
        capture_us: u64,
    ) -> Result<VideoFrame, VideoError> {
        let i420 = {
            let (cw, ch) = ((width as usize).div_ceil(2), (height as usize).div_ceil(2));
            let luma = width as usize * height as usize;
            let mut buf = vec![yuv[0]; luma + 2 * cw * ch];
            buf[luma..luma + cw * ch].fill(yuv[1]);
            buf[luma + cw * ch..].fill(yuv[2]);
            VideoFrame::from_planes(
                Planes::tight(PixelFormat::I420, width, height, buf)?,
                width,
                height,
                capture_us,
            )?
        };
        if format == PixelFormat::I420 {
            return Ok(i420);
        }
        crate::convert::convert(&i420, width, height, format)
    }

    /// The pixel layout (`Native` for an OS buffer).
    pub fn format(&self) -> PixelFormat {
        match &self.buffer {
            FrameBuffer::Planes(p) => p.format,
            FrameBuffer::Native(_) => PixelFormat::Native,
        }
    }

    /// The CPU planes, if the frame has them.
    pub fn planes(&self) -> Option<&Planes> {
        match &self.buffer {
            FrameBuffer::Planes(p) => Some(p),
            FrameBuffer::Native(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_round_chroma_up() {
        assert_eq!(PixelFormat::I420.frame_len(4, 4), 16 + 8);
        assert_eq!(PixelFormat::Nv12.frame_len(3, 3), 9 + 8);
        assert_eq!(PixelFormat::Bgra.frame_len(3, 3), 36);
        assert_eq!(PixelFormat::Nv12.plane_size(1, 3, 3), (4, 2));
        assert_eq!(PixelFormat::I420.plane_size(2, 3, 3), (2, 2));
        assert_eq!(
            tight_layout(PixelFormat::I420, 4, 4),
            ([4, 2, 2], [0, 16, 20])
        );
        assert_eq!(
            tight_layout(PixelFormat::Nv12, 4, 4),
            ([4, 4, 0], [0, 16, 0])
        );
    }

    #[test]
    fn padded_planes_read_row_by_row() {
        // 4x2 NV12 with 8-byte rows.
        let mut buf = vec![0xEEu8; 8 * 2 + 8];
        buf[..4].copy_from_slice(&[1, 2, 3, 4]);
        buf[8..12].copy_from_slice(&[5, 6, 7, 8]);
        buf[16..20].copy_from_slice(&[9, 10, 11, 12]);
        let p = Planes::new(PixelFormat::Nv12, 4, 2, buf, [8, 8, 0], [0, 16, 0]).expect("planes");
        let t = p.to_tight(4, 2);
        assert_eq!(&t.data[..], &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        assert_eq!(t.strides, [4, 4, 0]);
    }

    #[test]
    fn bad_layouts_fail_loudly() {
        assert!(Planes::tight(PixelFormat::I420, 4, 4, vec![0u8; 23]).is_err());
        assert!(Planes::new(PixelFormat::Nv12, 4, 2, vec![0u8; 12], [3, 4, 0], [0, 8, 0]).is_err());
        assert!(Planes::new(PixelFormat::Nv12, 4, 2, vec![0u8; 11], [4, 4, 0], [0, 8, 0]).is_err());
        assert!(Planes::tight(PixelFormat::I420, 0, 4, vec![]).is_err());
        assert!(Planes::tight(PixelFormat::Native, 4, 4, vec![]).is_err());
        // Offsets and strides whose sums overflow: an error, not a panic.
        let mut p = Planes::tight(PixelFormat::I420, 4, 4, vec![0u8; 24]).expect("ok");
        p.offsets[1] = usize::MAX - 1;
        assert!(p.validate(4, 4).is_err());
        let mut p = Planes::tight(PixelFormat::I420, 4, 4, vec![0u8; 24]).expect("ok");
        p.strides[0] = u32::MAX;
        assert!(p.validate(4, 16_384).is_err());
        let p = Planes::tight(PixelFormat::I420, 4, 4, vec![0u8; 24]).expect("ok");
        assert!(
            VideoFrame::from_planes(p, 8, 8, 0).is_err(),
            "planes too small for the frame"
        );
    }

    #[test]
    fn filled_frames_in_every_cpu_format() {
        for f in [
            PixelFormat::I420,
            PixelFormat::Nv12,
            PixelFormat::Bgra,
            PixelFormat::Rgba,
        ] {
            let v = VideoFrame::filled(f, 6, 4, [81, 90, 240], 7).expect("filled");
            assert_eq!((v.format(), v.width, v.height, v.capture_us), (f, 6, 4, 7));
        }
        let rgba = VideoFrame::filled(PixelFormat::Rgba, 2, 2, [81, 90, 240], 0).expect("red");
        let px = &rgba.planes().expect("planes").data[..4];
        assert!(
            px[0] > 230 && px[1] < 20 && px[2] < 20 && px[3] == 255,
            "BT.601 red: {px:?}"
        );
    }

    #[test]
    fn a_new_frame_has_no_host_time() {
        let planes = Planes::tight(PixelFormat::I420, 4, 4, vec![0u8; 24]).expect("planes");
        let f = VideoFrame::from_planes(planes, 4, 4, 9).expect("frame");
        assert_eq!(f.host_time, None);
        let f = VideoFrame::filled(PixelFormat::Nv12, 4, 4, [16, 128, 128], 9).expect("filled");
        assert_eq!(f.host_time, None);
    }

    #[test]
    fn host_time_shape_and_codes_are_stable() {
        let t = HostTime::new(1_234_567_890, HostClock::MachAbsoluteTime);
        assert_eq!(t.nanos, 1_234_567_890);
        assert_eq!(t.clock, HostClock::MachAbsoluteTime);
        // The codes are API: hosts carry them on their own wires.
        assert_eq!(HostClock::MachAbsoluteTime.as_str(), "mach_absolute_time");
        assert_eq!(
            HostClock::MachAbsoluteTime.to_string(),
            "mach_absolute_time"
        );
        // Copy, so a sink can carry it without a clone; 16 bytes or less
        // (a u64 and a one-byte clock).
        let u = t;
        assert_eq!(u, t);
        assert!(std::mem::size_of::<HostTime>() <= 16);
        assert!(std::mem::size_of::<Option<HostTime>>() <= 16);
    }

    #[test]
    fn host_time_is_in_the_debug_line() {
        let mut f = VideoFrame::filled(PixelFormat::I420, 2, 2, [16, 128, 128], 0).expect("f");
        assert!(format!("{f:?}").contains("host_time: None"), "{f:?}");
        f.host_time = Some(HostTime::new(42, HostClock::MachAbsoluteTime));
        let line = format!("{f:?}");
        assert!(
            line.contains("nanos: 42") && line.contains("MachAbsoluteTime"),
            "{line}"
        );
    }

    #[test]
    fn mach_ticks_become_nanoseconds_without_overflow() {
        // Apple silicon: 24 MHz ticks, timebase 125/3.
        assert_eq!(mach_ticks_to_nanos(24_000_000, 125, 3), Some(1_000_000_000));
        assert_eq!(mach_ticks_to_nanos(3, 125, 3), Some(125));
        // Intel Macs and the simulator there: 1/1.
        assert_eq!(mach_ticks_to_nanos(987_654_321, 1, 1), Some(987_654_321));
        // ticks * numer overflows a u64 here, but the result fits: the
        // product is taken in 128 bits.
        let ticks = 3u64 << 57;
        assert!(ticks.checked_mul(125).is_none());
        assert_eq!(mach_ticks_to_nanos(ticks, 125, 3), Some(125u64 << 57));
        assert_eq!(mach_ticks_to_nanos(u64::MAX, 125, 3), None, "past u64");
        assert_eq!(mach_ticks_to_nanos(1, 1, 0), None, "a zero denominator");
    }

    #[test]
    fn conversion_and_scaling_keep_the_host_time() {
        let t = Some(HostTime::new(5_000, HostClock::MachAbsoluteTime));
        let mut f = VideoFrame::filled(PixelFormat::I420, 8, 8, [81, 90, 240], 3).expect("f");
        f.host_time = t;
        for (w, h, format) in [
            (8, 8, PixelFormat::I420),
            (8, 8, PixelFormat::Nv12),
            (4, 4, PixelFormat::I420),
            (4, 4, PixelFormat::Nv12),
            (8, 8, PixelFormat::Bgra),
            (4, 4, PixelFormat::Rgba),
        ] {
            let out = crate::convert::convert(&f, w, h, format).expect("convert");
            assert_eq!(
                (out.host_time, out.capture_us),
                (t, 3),
                "{w}x{h} {format:?}"
            );
        }
    }
}
