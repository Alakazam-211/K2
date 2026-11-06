//! Frame-in: a [`FrameSource`] that the host app feeds itself.
//!
//! Some frames don't come from a device this crate opens: an iPhone's
//! screen comes from a broadcast upload extension (another process, which
//! hands frames to the app), and a host may already hold frames from its
//! own capture code. [`frame_in()`] makes a source and a [`FrameInSender`];
//! the host pushes frames into the sender, and the source behaves like any
//! camera or screen source: a queue of 2 (oldest dropped), the asked rate
//! and size, `Started` on the first frame and on every change of size or
//! rate, and `Interrupted`, `Resumed` and `Ended` when the host says so.
//!
//! A frame-in source never pauses by itself when the app goes to the
//! background: on a phone, a screen share keeps going while the person uses
//! other apps. Only the host's own calls stop it.

use crate::api::{FrameSource, Wake};
use crate::convert::convert;
use crate::error::{invalid, VideoError};
use crate::frame::{ColorSpace, FrameBuffer, PixelFormat, Planes, VideoFrame};
use crate::kit::{QueuedSource, SourceControl, SourceShared};
use crate::types::*;
use crate::yuv::{to_i420, Plane, Yuv420};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

/// How far a pushed frame must turn clockwise to stand upright.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Rotation {
    /// Already upright.
    #[default]
    Deg0,
    /// A quarter turn clockwise.
    Deg90,
    /// Half a turn.
    Deg180,
    /// Three quarters clockwise (a quarter turn counter-clockwise).
    Deg270,
}

impl Rotation {
    /// From degrees clockwise: 0, 90, 180 or 270 (360 and more wrap).
    /// Anything else is `Invalid`.
    pub fn from_degrees(degrees: u32) -> Result<Rotation, VideoError> {
        match degrees % 360 {
            0 => Ok(Rotation::Deg0),
            90 => Ok(Rotation::Deg90),
            180 => Ok(Rotation::Deg180),
            270 => Ok(Rotation::Deg270),
            _ => Err(invalid(format!("a rotation of {degrees} degrees"))),
        }
    }

    /// Degrees clockwise: 0, 90, 180 or 270.
    pub fn degrees(self) -> u32 {
        match self {
            Rotation::Deg0 => 0,
            Rotation::Deg90 => 90,
            Rotation::Deg180 => 180,
            Rotation::Deg270 => 270,
        }
    }
}

/// Settings for [`frame_in()`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FrameInOptions {
    /// The size and rate the source delivers at most. The size box is
    /// orientation-free: a 1280 x 720 box also takes a 720 x 1280 portrait
    /// frame. Larger frames are scaled down; frames that come faster than
    /// `fps` are dropped evenly.
    pub constraints: CaptureConstraints,
    /// About how often the host pushes (the rate reported when the
    /// constraints ask for none). Default 30.
    pub fps: u32,
}

impl Default for FrameInOptions {
    fn default() -> FrameInOptions {
        FrameInOptions {
            constraints: CaptureConstraints::default(),
            fps: 30,
        }
    }
}

impl FrameInOptions {
    /// At most `max_width` x `max_height` (either way round) at `fps`.
    pub fn new(max_width: u32, max_height: u32, fps: u32) -> FrameInOptions {
        FrameInOptions {
            constraints: CaptureConstraints::new(max_width, max_height, fps),
            fps,
        }
    }
}

/// A frame-in source and the sender that feeds it.
///
/// Push from any thread; [`FrameInSender::push`] never blocks. Drop the
/// source to stop: pushes then answer `ShutDown`. Drop every sender (or call
/// [`FrameInSender::end`]) to end the source: it reports
/// `Ended(SourceClosed)` when the last sender goes without saying why.
pub fn frame_in(opts: FrameInOptions, wake: Wake) -> Result<(FrameInSender, FrameIn), VideoError> {
    opts.constraints.validate()?;
    if !(1..=240).contains(&opts.fps) {
        return Err(invalid(format!("{} fps", opts.fps)));
    }
    let shared = SourceShared::new(opts.constraints, opts.fps, wake);
    // A phone's screen turns; the box turns with the frame.
    shared.set_box_follows_frame(true);
    let alive = Arc::new(());
    let sender = FrameInSender {
        inner: Arc::new(SenderInner {
            shared: shared.clone(),
            alive: Arc::downgrade(&alive),
            interrupted: AtomicBool::new(false),
        }),
    };
    let source = FrameIn {
        queue: QueuedSource::new(shared, NoControl),
        _alive: alive,
    };
    Ok((sender, source))
}

struct NoControl;

impl SourceControl for NoControl {}

/// The source side of [`frame_in()`]: an ordinary [`FrameSource`].
pub struct FrameIn {
    queue: QueuedSource<NoControl>,
    /// The senders hold it weakly: once the source is dropped, pushes fail.
    _alive: Arc<()>,
}

impl std::fmt::Debug for FrameIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameIn")
            .field("stats", &self.queue.stats())
            .finish_non_exhaustive()
    }
}

impl FrameSource for FrameIn {
    fn try_recv(&mut self) -> Option<VideoFrame> {
        self.queue.try_recv()
    }

    fn try_event(&mut self) -> Option<SourceEvent> {
        self.queue.try_event()
    }

    fn set_constraints(&mut self, c: CaptureConstraints) -> Result<(), VideoError> {
        self.queue.set_constraints(c)
    }

    fn set_paused(&mut self, paused: bool) {
        self.queue.set_paused(paused)
    }

    fn stats(&self) -> SourceStats {
        self.queue.stats()
    }
}

struct SenderInner {
    shared: Arc<SourceShared>,
    alive: Weak<()>,
    interrupted: AtomicBool,
}

impl Drop for SenderInner {
    fn drop(&mut self) {
        // The last sender went without a reason: the producer is gone.
        // (`event` sends `Ended` once; a reason given before wins.)
        self.shared
            .event(SourceEvent::Ended(EndReason::SourceClosed));
    }
}

/// The host's side of a [`frame_in()`] source. Clone it freely; the source
/// ends when the last clone is dropped.
#[derive(Clone)]
pub struct FrameInSender {
    inner: Arc<SenderInner>,
}

impl std::fmt::Debug for FrameInSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameInSender")
            .field("open", &self.is_open())
            .finish_non_exhaustive()
    }
}

impl FrameInSender {
    /// Hand over one frame, `rotation` short of upright. Never blocks.
    ///
    /// - CPU frames in any format, or a native buffer (a CVPixelBuffer on
    ///   Apple through `pixel_buffer_frame`, an AHardwareBuffer on Android
    ///   through `hardware_buffer_frame`, or any [`crate::NativeFrame`]).
    ///   An upright native frame passes through untouched, so an encoder of
    ///   the same backend takes it without a copy; a turned one is read
    ///   back and turned on the CPU.
    /// - `capture_us` is on [`crate::now_us`]'s clock (the backend's), and
    ///   must not go backwards.
    /// - Full-range YUV (`ColorSpace::Bt709Full`) on the CPU is mapped to
    ///   limited range, which every encoder here writes.
    ///
    /// `Ok` means the source took it; it may still be dropped to keep the
    /// asked rate, or as the oldest in a full queue (counted in the
    /// source's stats), or because the source is paused (see
    /// [`FrameInSender::wants_frames`]). `ShutDown` once the source was
    /// dropped or has ended: stop capturing.
    pub fn push(&self, frame: VideoFrame, rotation: Rotation) -> Result<(), VideoError> {
        if !self.is_open() {
            return Err(VideoError::ShutDown);
        }
        if self.inner.interrupted.swap(false, Ordering::AcqRel) {
            self.inner.shared.event(SourceEvent::Resumed);
        }
        if self.inner.shared.idle() {
            return Ok(());
        }
        let frame = prepare(frame, rotation)?;
        self.inner.shared.deliver(frame);
        Ok(())
    }

    /// Is the source open and not paused? When false, the host can skip
    /// the work of making frames.
    pub fn wants_frames(&self) -> bool {
        self.is_open() && !self.inner.shared.idle()
    }

    /// Is the source still there and not ended?
    pub fn is_open(&self) -> bool {
        self.inner.alive.strong_count() > 0 && !self.ended()
    }

    fn ended(&self) -> bool {
        // `idle` is paused or ended; paused alone still counts as open.
        self.inner.shared.has_ended()
    }

    /// The frames stopped for a while, not for good (`Interrupted`). The
    /// next push, or [`FrameInSender::resume`], reports `Resumed`.
    pub fn interrupt(&self, why: Interruption) {
        if !self.is_open() || self.inner.interrupted.swap(true, Ordering::AcqRel) {
            return;
        }
        self.inner.shared.event(SourceEvent::Interrupted(why));
    }

    /// Back after [`FrameInSender::interrupt`] (`Resumed`), before the next
    /// frame.
    pub fn resume(&self) {
        if self.is_open() && self.inner.interrupted.swap(false, Ordering::AcqRel) {
            self.inner.shared.event(SourceEvent::Resumed);
        }
    }

    /// The frames stopped for good: `Ended(reason)`, sent once. For a
    /// screen share, `UserStopped` when the person stopped it from the
    /// system's UI, `SourceClosed` when the producer went away.
    pub fn end(&self, reason: EndReason) {
        self.inner.shared.event(SourceEvent::Ended(reason));
    }
}

/// Turn `frame` upright and map full-range YUV to limited range.
fn prepare(frame: VideoFrame, rotation: Rotation) -> Result<VideoFrame, VideoError> {
    let full = frame.color == ColorSpace::Bt709Full;
    if let FrameBuffer::Planes(p) = &frame.buffer {
        p.validate(frame.width, frame.height)?;
    }
    if rotation == Rotation::Deg0 {
        let cpu_yuv = matches!(frame.format(), PixelFormat::I420 | PixelFormat::Nv12);
        if !(full && cpu_yuv) {
            return Ok(frame);
        }
        let planes = frame
            .planes()
            .expect("CPU YUV has planes")
            .to_tight(frame.width, frame.height);
        let mut out = VideoFrame::from_planes(
            limit_range(&planes, frame.width, frame.height),
            frame.width,
            frame.height,
            frame.capture_us,
        )?;
        out.color = ColorSpace::Bt709Limited;
        return Ok(out);
    }
    let mut turned = turn(&frame, rotation.degrees())?;
    if full {
        let planes = turned.planes().expect("a turned frame has planes").clone();
        turned = VideoFrame::from_planes(
            limit_range(&planes, turned.width, turned.height),
            turned.width,
            turned.height,
            turned.capture_us,
        )?;
        turned.color = ColorSpace::Bt709Limited;
    }
    Ok(turned)
}

/// `frame` as tight I420, turned clockwise by `degrees` (90, 180 or 270).
pub(crate) fn turn(frame: &VideoFrame, degrees: u32) -> Result<VideoFrame, VideoError> {
    let planes: Planes = match &frame.buffer {
        FrameBuffer::Native(n) => n.0.to_planes(PixelFormat::Nv12)?,
        FrameBuffer::Planes(p) if matches!(p.format, PixelFormat::I420 | PixelFormat::Nv12) => {
            p.clone()
        }
        FrameBuffer::Planes(_) => {
            let i420 = convert(frame, frame.width, frame.height, PixelFormat::I420)?;
            i420.planes().expect("convert gives planes").clone()
        }
    };
    planes.validate(frame.width, frame.height)?;
    let d = &planes.data[..];
    let plane = |offset: usize, stride: u32, pixel_stride: usize| Plane {
        data: &d[offset..],
        row_stride: stride as usize,
        pixel_stride,
    };
    let img = Yuv420 {
        width: frame.width,
        height: frame.height,
        planes: match planes.format {
            PixelFormat::Nv12 => [
                plane(planes.offsets[0], planes.strides[0], 1),
                plane(planes.offsets[1], planes.strides[1], 2),
                plane(planes.offsets[1] + 1, planes.strides[1], 2),
            ],
            _ => [
                plane(planes.offsets[0], planes.strides[0], 1),
                plane(planes.offsets[1], planes.strides[1], 1),
                plane(planes.offsets[2], planes.strides[2], 1),
            ],
        },
    };
    let (data, w, h) = to_i420(&img, degrees)?;
    let mut out = VideoFrame::from_planes(
        Planes::tight(PixelFormat::I420, w, h, data)?,
        w,
        h,
        frame.capture_us,
    )?;
    out.color = frame.color;
    Ok(out)
}

/// Full-range YUV (tight I420 or NV12) mapped to limited range: luma
/// 0-255 to 16-235, chroma 0-255 to 16-240 around 128.
fn limit_range(p: &Planes, width: u32, height: u32) -> Planes {
    let luma: [u8; 256] = std::array::from_fn(|v| (16 + (v as u32 * 219 + 127) / 255) as u8);
    let chroma: [u8; 256] = std::array::from_fn(|v| {
        let c = v as i32 - 128;
        (128 + (c * 224 + c.signum() * 127) / 255) as u8
    });
    let luma_len = width as usize * height as usize;
    let data: Vec<u8> = p
        .data
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            if i < luma_len {
                luma[v as usize]
            } else {
                chroma[v as usize]
            }
        })
        .collect();
    Planes::tight(p.format, width, height, data).expect("the same tight layout")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::no_wake;
    use crate::fake::{pattern_frame, read_frame_code};
    use crate::frame::NativeFrame;
    use std::any::Any;
    use std::sync::atomic::AtomicUsize;

    fn nv12(w: u32, h: u32, n: u32, at: u64) -> VideoFrame {
        let f = pattern_frame(w, h, n, at).expect("pattern");
        convert(&f, w, h, PixelFormat::Nv12).expect("nv12")
    }

    fn events(src: &mut FrameIn) -> Vec<SourceEvent> {
        std::iter::from_fn(|| src.try_event()).collect()
    }

    #[test]
    fn frames_go_through_with_started_and_the_queue_drops_the_oldest() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let (tx, mut rx) = frame_in(
            FrameInOptions::default(),
            Arc::new(move || {
                w.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .expect("frame_in");
        assert!(tx.wants_frames() && tx.is_open());
        assert!(rx.try_recv().is_none(), "nothing before a push");
        tx.push(nv12(64, 48, 1, 0), Rotation::Deg0).expect("push");
        assert_eq!(
            events(&mut rx),
            vec![SourceEvent::Started {
                width: 64,
                height: 48,
                fps: 30
            }]
        );
        let f = rx.try_recv().expect("a frame");
        assert_eq!((f.format(), f.width, f.height), (PixelFormat::Nv12, 64, 48));
        assert_eq!(read_frame_code(&f), Some(1));
        assert!(
            wakes.load(Ordering::Relaxed) >= 1,
            "the push woke the reader"
        );

        // Backpressure: five quick pushes (34 ms apart, so the rate keeps
        // them all) with nobody reading. Every push returns at once; the
        // queue keeps the newest 2.
        for n in 2..=6u32 {
            tx.push(nv12(64, 48, n, n as u64 * 34_000), Rotation::Deg0)
                .expect("push never blocks");
        }
        let kept: Vec<u32> = std::iter::from_fn(|| rx.try_recv())
            .map(|f| read_frame_code(&f).expect("code"))
            .collect();
        assert_eq!(kept, vec![5, 6], "the oldest were dropped");
        let st = rx.stats();
        assert_eq!((st.frames, st.dropped), (6, 3));
        assert!(events(&mut rx).is_empty(), "same size and rate: no Started");

        // A new size is a new Started.
        tx.push(nv12(32, 24, 7, 300_000), Rotation::Deg0)
            .expect("push");
        assert_eq!(
            events(&mut rx),
            vec![SourceEvent::Started {
                width: 32,
                height: 24,
                fps: 30
            }]
        );
    }

    #[test]
    fn the_rate_and_the_box_are_kept() {
        // At most 160 x 90 at 10 fps, while the host pushes 30.
        let (tx, mut rx) = frame_in(FrameInOptions::new(160, 90, 10), no_wake()).expect("open");
        for n in 0..9u32 {
            tx.push(nv12(320, 180, n, n as u64 * 33_333), Rotation::Deg0)
                .expect("push");
            while rx.try_recv().is_some() {}
        }
        let st = rx.stats();
        assert_eq!(st.frames, 3, "30 fps kept at 10: {st:?}");
        assert_eq!((st.width, st.height, st.fps), (160, 90, 10));
        // A portrait frame fits the box turned (box follows the frame).
        tx.push(nv12(180, 320, 9, 1_000_000), Rotation::Deg0)
            .expect("push");
        let f = rx.try_recv().expect("frame");
        assert_eq!((f.width, f.height), (90, 160));
    }

    #[test]
    fn interruptions_and_the_end_are_events() {
        let (tx, mut rx) = frame_in(FrameInOptions::default(), no_wake()).expect("open");
        tx.push(nv12(32, 32, 1, 0), Rotation::Deg0).expect("push");
        events(&mut rx);
        tx.interrupt(Interruption::Hidden);
        tx.interrupt(Interruption::Hidden);
        assert_eq!(
            events(&mut rx),
            vec![SourceEvent::Interrupted(Interruption::Hidden)],
            "once"
        );
        tx.push(nv12(32, 32, 2, 40_000), Rotation::Deg0)
            .expect("push");
        assert_eq!(events(&mut rx), vec![SourceEvent::Resumed]);
        tx.interrupt(Interruption::Background);
        tx.resume();
        tx.resume();
        assert_eq!(
            events(&mut rx),
            vec![
                SourceEvent::Interrupted(Interruption::Background),
                SourceEvent::Resumed
            ]
        );

        let other = tx.clone();
        tx.end(EndReason::UserStopped);
        assert_eq!(
            events(&mut rx),
            vec![SourceEvent::Ended(EndReason::UserStopped)]
        );
        assert!(!other.is_open() && !other.wants_frames());
        assert_eq!(
            other.push(nv12(32, 32, 3, 80_000), Rotation::Deg0).err(),
            Some(VideoError::ShutDown)
        );
        drop((tx, other));
        assert!(
            events(&mut rx).is_empty(),
            "the given reason stands; dropping the senders adds nothing"
        );
        while rx.try_recv().is_some() {}
        assert!(rx.try_recv().is_none(), "no frames after the end");
    }

    #[test]
    fn the_last_sender_going_ends_the_source_and_a_dropped_source_refuses() {
        let (tx, mut rx) = frame_in(FrameInOptions::default(), no_wake()).expect("open");
        let tx2 = tx.clone();
        drop(tx);
        assert!(events(&mut rx).is_empty(), "a clone still holds it");
        drop(tx2);
        assert_eq!(
            events(&mut rx),
            vec![SourceEvent::Ended(EndReason::SourceClosed)]
        );

        let (tx, rx) = frame_in(FrameInOptions::default(), no_wake()).expect("open");
        drop(rx);
        assert!(!tx.is_open() && !tx.wants_frames());
        assert_eq!(
            tx.push(nv12(32, 32, 1, 0), Rotation::Deg0).err(),
            Some(VideoError::ShutDown)
        );
    }

    #[test]
    fn a_paused_source_drops_pushes_and_says_so() {
        let (tx, mut rx) = frame_in(FrameInOptions::default(), no_wake()).expect("open");
        rx.set_paused(true);
        assert!(!tx.wants_frames() && tx.is_open());
        tx.push(nv12(32, 32, 1, 0), Rotation::Deg0)
            .expect("a paused source still takes pushes");
        assert!(rx.try_recv().is_none());
        rx.set_paused(false);
        assert!(tx.wants_frames());
        tx.push(nv12(32, 32, 2, 40_000), Rotation::Deg0)
            .expect("push");
        assert_eq!(read_frame_code(&rx.try_recv().expect("frame")), Some(2));
    }

    /// Turn an I420 frame counter-clockwise by a quarter (what a sensor or
    /// a sideways screen would hand over), as tight NV12.
    fn sideways_nv12(upright: &VideoFrame) -> VideoFrame {
        let i420 = turn(upright, 270).expect("turn");
        convert(&i420, i420.width, i420.height, PixelFormat::Nv12).expect("nv12")
    }

    #[test]
    fn rotation_turns_frames_upright_in_any_format() {
        let (tx, mut rx) = frame_in(FrameInOptions::default(), no_wake()).expect("open");
        let upright = pattern_frame(96, 64, 0x1234_5678, 0).expect("pattern");
        let sideways = sideways_nv12(&upright);
        assert_eq!((sideways.width, sideways.height), (64, 96));
        assert_ne!(
            read_frame_code(&sideways),
            Some(0x1234_5678),
            "sideways doesn't read"
        );
        tx.push(sideways.clone(), Rotation::Deg90).expect("push");
        let f = rx.try_recv().expect("frame");
        assert_eq!((f.format(), f.width, f.height), (PixelFormat::I420, 96, 64));
        assert_eq!(read_frame_code(&f), Some(0x1234_5678));
        // The same from RGBA, and from a padded NV12.
        let mut at = 40_000;
        let mut rgba = convert(&sideways, 64, 96, PixelFormat::Rgba).expect("rgba");
        rgba.capture_us = at;
        tx.push(rgba, Rotation::Deg90).expect("push");
        assert_eq!(
            read_frame_code(&rx.try_recv().expect("frame")),
            Some(0x1234_5678)
        );
        let p = sideways.planes().expect("planes");
        let mut padded = Vec::new();
        for y in 0..96 {
            padded.extend_from_slice(p.row(0, y, 64, 96));
            padded.extend_from_slice(&[0xEE; 16]);
        }
        let uv_at = padded.len();
        for y in 0..48 {
            padded.extend_from_slice(p.row(1, y, 64, 96));
            padded.extend_from_slice(&[0xEE; 16]);
        }
        let planes = Planes::new(
            PixelFormat::Nv12,
            64,
            96,
            padded,
            [80, 80, 0],
            [0, uv_at, 0],
        )
        .expect("padded planes");
        at += 40_000;
        let f = VideoFrame::from_planes(planes, 64, 96, at).expect("frame");
        tx.push(f, Rotation::Deg90).expect("push");
        let f = rx.try_recv().expect("frame");
        assert_eq!(f.capture_us, at, "the timestamp travels");
        assert_eq!(read_frame_code(&f), Some(0x1234_5678));
        // Half a turn twice is upright again.
        let mut half = turn(&upright, 180).expect("turn");
        half.capture_us = at + 40_000;
        tx.push(half, Rotation::Deg180).expect("push");
        assert_eq!(
            read_frame_code(&rx.try_recv().expect("frame")),
            Some(0x1234_5678)
        );
    }

    #[test]
    fn rotations_are_quarter_turns() {
        for d in [0, 90, 180, 270, 360, 450] {
            assert_eq!(Rotation::from_degrees(d).expect("ok").degrees(), d % 360);
        }
        assert!(matches!(
            Rotation::from_degrees(45),
            Err(VideoError::Invalid(_))
        ));
    }

    struct Native(VideoFrame);

    impl NativeFrame for Native {
        fn format(&self) -> PixelFormat {
            PixelFormat::Nv12
        }
        fn to_planes(&self, format: PixelFormat) -> Result<Planes, VideoError> {
            let f = convert(&self.0, self.0.width, self.0.height, format)?;
            Ok(f.planes().expect("planes").clone())
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[test]
    fn native_frames_pass_through_upright_and_turn_on_the_cpu() {
        let (tx, mut rx) = frame_in(FrameInOptions::default(), no_wake()).expect("open");
        let upright = pattern_frame(64, 48, 77, 0).expect("pattern");
        let native = VideoFrame::from_native(
            crate::frame::NativeBuffer(Arc::new(Native(upright.clone()))),
            64,
            48,
            0,
        );
        tx.push(native, Rotation::Deg0).expect("push");
        let f = rx.try_recv().expect("frame");
        assert_eq!(f.format(), PixelFormat::Native, "no copy when upright");
        let sideways = turn(&upright, 270).expect("turn");
        let native = VideoFrame::from_native(
            crate::frame::NativeBuffer(Arc::new(Native(sideways))),
            48,
            64,
            40_000,
        );
        tx.push(native, Rotation::Deg90).expect("push");
        let f = rx.try_recv().expect("frame");
        assert_eq!((f.format(), f.width, f.height), (PixelFormat::I420, 64, 48));
        assert_eq!(read_frame_code(&f), Some(77));
    }

    #[test]
    fn full_range_is_mapped_to_limited() {
        let (tx, mut rx) = frame_in(FrameInOptions::default(), no_wake()).expect("open");
        for (rotation, at) in [(Rotation::Deg0, 0), (Rotation::Deg90, 40_000)] {
            // Black, white and grey luma; chroma at both ends.
            let mut data = vec![0u8; 16];
            data[4..8].fill(255);
            data[8..12].fill(128);
            data.extend_from_slice(&[0, 255, 128, 128, 0, 255, 128, 128]);
            let mut f = VideoFrame::from_planes(
                Planes::tight(PixelFormat::Nv12, 4, 4, data).expect("planes"),
                4,
                4,
                at,
            )
            .expect("frame");
            f.color = ColorSpace::Bt709Full;
            tx.push(f, rotation).expect("push");
            let out = rx.try_recv().expect("frame");
            assert_eq!(out.color, ColorSpace::Bt709Limited);
            let p = out.planes().expect("planes").to_tight(4, 4);
            let luma = &p.data[..16];
            assert_eq!(luma.iter().min(), Some(&16), "{rotation:?}");
            assert_eq!(luma.iter().max(), Some(&235), "{rotation:?}");
            let chroma = &p.data[16..];
            assert_eq!(chroma.iter().min(), Some(&16), "{rotation:?}");
            assert_eq!(chroma.iter().max(), Some(&240), "{rotation:?}");
            assert!(chroma.contains(&128));
        }
    }

    #[test]
    fn bad_frames_and_options_fail_loudly() {
        assert!(frame_in(FrameInOptions::new(8, 8, 30), no_wake()).is_err());
        let o = FrameInOptions {
            fps: 0,
            ..FrameInOptions::default()
        };
        assert!(frame_in(o, no_wake()).is_err());
        let (tx, mut rx) = frame_in(FrameInOptions::default(), no_wake()).expect("open");
        let f = nv12(32, 32, 1, 0);
        let mut broken = f.clone();
        broken.width = 64;
        assert!(matches!(
            tx.push(broken, Rotation::Deg0),
            Err(VideoError::Invalid(_))
        ));
        assert!(rx.try_recv().is_none());
    }
}
