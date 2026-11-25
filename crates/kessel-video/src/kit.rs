//! Building blocks for backends: they turn a synchronous codec core into the
//! submit + poll [`VideoEncoder`] / [`VideoDecoder`] with threads of their own,
//! bounded queues and wake-ups, and give capture code one place to queue
//! frames and events. The OS backends here use them; codec crates and phone
//! backends can too.

use crate::api::{FrameSource, VideoDecoder, VideoEncoder, Wake};
use crate::convert::{convert, fit_within};
use crate::error::{DecodeError, VideoError};
use crate::frame::{PixelFormat, VideoFrame};
use crate::types::*;
use bytes::Bytes;
use crossbeam_queue::ArrayQueue;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Encoder output queue depth (never dropped; `submit` answers `Busy`).
pub const ENCODER_QUEUE: usize = 8;
/// Decoder input queue depth.
pub const DECODER_INPUT_QUEUE: usize = 4;
/// Decoder output queue depth (oldest dropped).
pub const DECODER_OUTPUT_QUEUE: usize = 2;
/// [`DecodeMode::File`]: the output queue [`DecoderConfig::file`] asks for
/// (never dropped; `submit` answers `Busy`).
pub const FILE_OUTPUT_QUEUE: usize = 8;
/// [`DecodeMode::File`]: the largest [`DecoderConfig::output_queue`]. A
/// decoded 4K frame is 12 MB, so 64 of them are most of a gigabyte.
pub const FILE_OUTPUT_QUEUE_MAX: usize = 64;
/// [`DecodeMode::File`]: the most decoded frames held back for
/// presentation order (H.264's decoded picture buffer holds at most 16).
pub const FILE_REORDER_WINDOW: usize = 16;
/// Capture queue depth (oldest dropped).
pub const CAPTURE_QUEUE: usize = 2;

static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Microseconds on this process's monotonic clock (the `capture_us` base of
/// every backend here).
pub fn now_us() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_micros() as u64
}

// ───────────────────────────── encoder ─────────────────────────────

/// One encoded frame from a core.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CoreFrame {
    /// The compressed frame, framed as the codec says.
    pub data: Bytes,
    /// A keyframe.
    pub keyframe: bool,
    /// Temporal layer.
    pub layer: LayerInfo,
}

impl CoreFrame {
    /// A base-layer frame.
    pub fn new(data: impl Into<Bytes>, keyframe: bool) -> CoreFrame {
        CoreFrame {
            data: data.into(),
            keyframe,
            layer: LayerInfo::new(0, true),
        }
    }
}

/// A synchronous encoder: one frame in, at most one out, before it returns.
/// [`ThreadedEncoder`] runs it on a thread of its own.
pub trait EncoderCore: Send + 'static {
    /// The layout it wants CPU frames in.
    fn input_format(&self) -> PixelFormat;
    /// Can it take this native frame directly (a buffer of its own
    /// backend)? Others are read back to CPU planes first.
    fn accepts_native(&self, _frame: &VideoFrame) -> bool {
        false
    }
    /// Encode one frame of the coded size (CPU planes in
    /// [`EncoderCore::input_format`], or a native buffer it accepted).
    /// `Ok(None)`: rate control skipped it. An error kills the encoder.
    fn encode(
        &mut self,
        frame: &VideoFrame,
        keyframe: bool,
    ) -> Result<Option<CoreFrame>, VideoError>;
    /// New rates, without a keyframe.
    fn set_rates(&mut self, rates: Rates) -> Result<(), VideoError>;
    /// A new coded size (may reopen the session).
    fn set_size(&mut self, width: u32, height: u32) -> Result<(), VideoError>;
    /// What the open session runs, read back from the OS (`Unknown` when
    /// the OS doesn't say). Asked again after `set_size`.
    fn implementation(&self) -> Implementation;
    /// For logs.
    fn name(&self) -> String;
}

struct EncState {
    input: Option<(VideoFrame, bool)>,
    busy: bool,
    keyframe: bool,
    rates: Option<Rates>,
    size: Option<(u32, u32)>,
    config: EncoderConfig,
    implementation: Implementation,
    name: String,
    dead: Option<VideoError>,
    stop: bool,
    stats: EncoderStats,
}

struct EncShared {
    state: Mutex<EncState>,
    cv: Condvar,
    out: ArrayQueue<Result<EncodedFrame, VideoError>>,
    wake: Wake,
}

/// A [`VideoEncoder`] around an [`EncoderCore`] on its own thread. At most
/// one frame encodes while one more waits, so a requested keyframe comes out
/// within two outputs.
pub struct ThreadedEncoder {
    shared: Arc<EncShared>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for ThreadedEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadedEncoder").finish_non_exhaustive()
    }
}

impl ThreadedEncoder {
    /// Start the thread. `config` must already be checked and `core` open at
    /// its size.
    pub fn start(
        core: Box<dyn EncoderCore>,
        config: EncoderConfig,
        wake: Wake,
    ) -> Result<ThreadedEncoder, VideoError> {
        let shared = Arc::new(EncShared {
            state: Mutex::new(EncState {
                input: None,
                busy: false,
                keyframe: true,
                rates: None,
                size: None,
                config,
                implementation: core.implementation(),
                name: core.name(),
                dead: None,
                stop: false,
                stats: EncoderStats::default(),
            }),
            cv: Condvar::new(),
            out: ArrayQueue::new(ENCODER_QUEUE),
            wake,
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("kessel-video-enc".into())
            .spawn(move || encoder_main(s, core))
            .map_err(|e| {
                VideoError::os(e.raw_os_error().unwrap_or(0) as i64, format!("spawn: {e}"))
            })?;
        Ok(ThreadedEncoder {
            shared,
            thread: Some(thread),
        })
    }
}

fn encoder_main(s: Arc<EncShared>, mut core: Box<dyn EncoderCore>) {
    // Real-time media: keep the thread off a low QoS class on Apple.
    #[cfg(all(feature = "apple-codecs", any(target_os = "macos", target_os = "ios")))]
    crate::apple::set_media_thread_qos();
    let mut since_key: u64 = 0;
    loop {
        let (frame, mut key, rates, size, cfg) = {
            let mut st = lock(&s.state);
            while st.input.is_none() && st.rates.is_none() && st.size.is_none() && !st.stop {
                st = s.cv.wait(st).unwrap_or_else(PoisonError::into_inner);
            }
            if st.stop {
                return;
            }
            let input = st.input.take();
            let key = input.is_some() && std::mem::take(&mut st.keyframe);
            st.busy = input.is_some();
            (input, key, st.rates.take(), st.size.take(), st.config)
        };
        let mut fail = None;
        if let Some((w, h)) = size {
            if let Err(e) = core.set_size(w, h) {
                fail = Some(e);
            }
            key = true;
            since_key = 0;
            let mut st = lock(&s.state);
            st.keyframe = frame.is_none();
            // A new size may be a new session, and another implementation.
            st.implementation = core.implementation();
            st.name = core.name();
        }
        if let (Some(r), None) = (rates, &fail) {
            if let Err(e) = core.set_rates(r) {
                fail = Some(e);
            }
        }
        let mut out = None;
        if let (Some((frame, opt_key)), None) = (frame, &fail) {
            key |= opt_key;
            if let Some(n) = cfg.keyframe_interval {
                key |= since_key + 1 >= n as u64;
            }
            let t0 = Instant::now();
            // At the encoder's size, a native buffer the encoder takes, or
            // planes in its input format, go in as they are.
            let same_size = (frame.width, frame.height) == (cfg.width, cfg.height);
            let native = matches!(frame.buffer, crate::frame::FrameBuffer::Native(_))
                && core.accepts_native(&frame);
            let prepared = if same_size && (native || frame.format() == core.input_format()) {
                Ok(frame.clone())
            } else {
                convert(&frame, cfg.width, cfg.height, core.input_format())
            };
            match prepared.and_then(|f| core.encode(&f, key)) {
                Ok(Some(cf)) => {
                    since_key = if cf.keyframe { 0 } else { since_key + 1 };
                    let mut ef = EncodedFrame::new(
                        cfg.codec.codec(),
                        cf.data,
                        cf.keyframe,
                        frame.capture_us,
                        cfg.width,
                        cfg.height,
                    );
                    ef.layer = cf.layer;
                    ef.encode_us = t0.elapsed().as_micros().min(u32::MAX as u128) as u32;
                    out = Some(ef);
                }
                Ok(None) => {
                    since_key += 1;
                    let mut st = lock(&s.state);
                    st.stats.dropped_by_rc += 1;
                    // A skipped keyframe is still owed.
                    st.keyframe |= key;
                }
                Err(e) => fail = Some(e),
            }
        }
        let mut st = lock(&s.state);
        st.busy = false;
        if let Some(e) = fail {
            st.dead = Some(e.clone());
            st.stop = true;
            drop(st);
            // Room is reserved by submit's check, but a terminal error must
            // get through regardless.
            s.out.force_push(Err(e));
            (s.wake)();
            return;
        }
        if let Some(ef) = out {
            st.stats.frames_out += 1;
            st.stats.bytes_out += ef.data.len() as u64;
            if ef.keyframe {
                st.stats.keyframes += 1;
            }
            drop(st);
            if s.out.push(Ok(ef)).is_err() {
                // Can't happen: submit keeps the queue from filling.
                lock(&s.state).stats.busy += 1;
            }
            (s.wake)();
        }
    }
}

impl VideoEncoder for ThreadedEncoder {
    fn submit(&mut self, frame: &VideoFrame, opts: EncodeOptions) -> Result<(), VideoError> {
        let mut st = lock(&self.shared.state);
        if let Some(e) = &st.dead {
            return Err(e.clone());
        }
        let pending = st.input.is_some() as usize + st.busy as usize;
        if st.input.is_some() || self.shared.out.len() + pending >= ENCODER_QUEUE {
            st.stats.busy += 1;
            return Err(VideoError::Busy);
        }
        if let crate::frame::FrameBuffer::Planes(p) = &frame.buffer {
            p.validate(frame.width, frame.height)?;
        }
        st.input = Some((frame.clone(), opts.keyframe));
        st.stats.frames_in += 1;
        drop(st);
        self.shared.cv.notify_one();
        Ok(())
    }

    fn poll(&mut self) -> Option<Result<EncodedFrame, VideoError>> {
        self.shared.out.pop()
    }

    fn request_keyframe(&mut self) {
        lock(&self.shared.state).keyframe = true;
    }

    fn set_rates(&mut self, rates: Rates) -> Result<(), VideoError> {
        rates.validate()?;
        let mut st = lock(&self.shared.state);
        if let Some(e) = &st.dead {
            return Err(e.clone());
        }
        st.config.bitrate_bps = rates.bitrate_bps;
        st.config.max_bitrate_bps = st.config.max_bitrate_bps.max(rates.bitrate_bps);
        st.config.fps = rates.fps;
        st.rates = Some(rates);
        drop(st);
        self.shared.cv.notify_one();
        Ok(())
    }

    fn set_size(&mut self, width: u32, height: u32) -> Result<(), VideoError> {
        validate_size(width, height)?;
        let mut st = lock(&self.shared.state);
        if let Some(e) = &st.dead {
            return Err(e.clone());
        }
        st.config.width = width;
        st.config.height = height;
        st.size = Some((width, height));
        st.keyframe = true;
        drop(st);
        self.shared.cv.notify_one();
        Ok(())
    }

    fn info(&self) -> EncoderInfo {
        let st = lock(&self.shared.state);
        EncoderInfo::new(st.implementation, st.name.clone(), st.config)
    }

    fn stats(&self) -> EncoderStats {
        lock(&self.shared.state).stats
    }
}

impl Drop for ThreadedEncoder {
    fn drop(&mut self) {
        lock(&self.shared.state).stop = true;
        self.shared.cv.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ───────────────────────────── decoder ─────────────────────────────

/// A synchronous decoder: one frame in, at most one out, before it returns.
pub trait DecoderCore: Send + 'static {
    /// Decode one frame. `Ok(None)`: nothing to show (parameter sets only).
    fn decode(&mut self, input: &EncodedInput) -> Result<Option<VideoFrame>, DecodeError>;
    /// Forget all state; reopen a lost session.
    fn reset(&mut self) -> Result<(), VideoError>;
    /// What the open session runs, read back from the OS (`Unknown` when
    /// the OS doesn't say, or before a session opens).
    fn implementation(&self) -> Implementation;
    /// What the open session runs, or `None` while no session is open (a
    /// decoder that opens on its first keyframe). Asked after every decode
    /// and reset, for [`DecoderStats::implementation`].
    fn session_implementation(&self) -> Option<Implementation> {
        Some(self.implementation())
    }
    /// [`DecodeMode::File`]: decode one input, given in decode order with
    /// its pts in `capture_us`, and append to `out` every frame that is
    /// ready, each stamped with its own pts in `capture_us`, in any order:
    /// [`ThreadedDecoder`] puts them in presentation order. A core may hold
    /// frames back (the OS reordering) and give them in a later call or in
    /// [`DecoderCore::flush_file`]. An error may come after frames were
    /// appended; those still count.
    ///
    /// The default calls [`DecoderCore::decode`] (one in, at most one out,
    /// at once) and stamps its frame with the input's pts.
    fn decode_file(
        &mut self,
        input: &EncodedInput,
        out: &mut Vec<VideoFrame>,
    ) -> Result<(), DecodeError> {
        if let Some(mut f) = self.decode(input)? {
            f.capture_us = input.capture_us;
            out.push(f);
        }
        Ok(())
    }
    /// [`DecodeMode::File`], at the end of the stream: append every frame
    /// still held back, each with its pts. The default holds none.
    fn flush_file(&mut self, out: &mut Vec<VideoFrame>) -> Result<(), DecodeError> {
        let _ = out;
        Ok(())
    }
}

/// File mode's ordering: the frames held back for presentation order and
/// the times seen, under the decoder's state lock.
#[derive(Default)]
struct FileOrder {
    /// The pts of inputs accepted and not yet out (for duplicates).
    pending: BTreeSet<u64>,
    /// Decoded frames waiting for their turn, by pts.
    held: BTreeMap<u64, VideoFrame>,
    /// The pts of the last frame out.
    last_out: Option<u64>,
    /// The dts of the last input accepted.
    last_dts: Option<u64>,
    /// How many frames the stream may reorder (from its SPS; 0 for codecs
    /// without reordering).
    depth: usize,
    /// Frames out, waiting for `poll` (never dropped).
    ready: VecDeque<Result<VideoFrame, DecodeError>>,
    /// `flush` was called and the drain hasn't finished.
    draining: bool,
}

impl FileOrder {
    /// A decoded frame from the core: held for its turn, or dropped when
    /// it can't be in order (a later frame is out, or the same pts is
    /// held).
    fn arrive(&mut self, f: VideoFrame, stats: &mut DecoderStats) {
        let pts = f.capture_us;
        if self.last_out.is_some_and(|l| pts <= l) || self.held.contains_key(&pts) {
            stats.dropped_out += 1;
            return;
        }
        self.held.insert(pts, f);
    }

    /// Move held frames to `ready` while more are held than the stream
    /// may reorder (all of them when `all`).
    fn release(&mut self, all: bool, stats: &mut DecoderStats) {
        let keep = if all {
            0
        } else {
            self.depth.min(FILE_REORDER_WINDOW)
        };
        while self.held.len() > keep {
            let Some((pts, f)) = self.held.pop_first() else {
                break;
            };
            self.last_out = Some(pts);
            // Inputs at or before it can no longer come out.
            self.pending = self.pending.split_off(&pts.saturating_add(1));
            stats.frames_out += 1;
            self.ready.push_back(Ok(f));
        }
    }

    /// Forget the stream (a reset, or the end of a flush).
    fn restart(&mut self) {
        self.pending.clear();
        self.held.clear();
        self.last_out = None;
        self.last_dts = None;
    }
}

struct DecState {
    waiting_key: bool,
    reset: bool,
    fit: Option<(u32, u32)>,
    stop: bool,
    stats: DecoderStats,
    /// `Some` in [`DecodeMode::File`].
    file: Option<FileOrder>,
}

struct DecShared {
    state: Mutex<DecState>,
    cv: Condvar,
    input: ArrayQueue<EncodedInput>,
    out: ArrayQueue<Result<VideoFrame, DecodeError>>,
    output: PixelFormat,
    codec: VideoCodec,
    /// [`DecodeMode::File`].
    file_mode: bool,
    /// [`DecodeMode::File`]: `submit` answers `Busy` while this many
    /// frames wait for `poll`.
    file_queue: usize,
    wake: Wake,
}

/// A [`VideoDecoder`] around a [`DecoderCore`] on its own thread. Input
/// queue of 4 (`Busy` when full). Real time: output queue of 2 (oldest
/// dropped). [`DecodeMode::File`]: frames in presentation order, an output
/// queue of [`DecoderConfig::output_queue`] that never drops (`Busy`
/// instead), and [`VideoDecoder::flush`]. Waits for a keyframe at the
/// start, after an error, after `reset` and after a flush.
pub struct ThreadedDecoder {
    shared: Arc<DecShared>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for ThreadedDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadedDecoder").finish_non_exhaustive()
    }
}

impl ThreadedDecoder {
    /// Start the thread. `Invalid` for a config that fails
    /// [`DecoderConfig::validate`].
    pub fn start(
        core: Box<dyn DecoderCore>,
        config: DecoderConfig,
        wake: Wake,
    ) -> Result<ThreadedDecoder, VideoError> {
        config.validate()?;
        let stats = DecoderStats {
            implementation: core.session_implementation(),
            ..DecoderStats::default()
        };
        let file = (config.mode == DecodeMode::File).then(FileOrder::default);
        let shared = Arc::new(DecShared {
            state: Mutex::new(DecState {
                waiting_key: true,
                reset: false,
                fit: config.fit,
                stop: false,
                stats,
                file,
            }),
            cv: Condvar::new(),
            input: ArrayQueue::new(DECODER_INPUT_QUEUE),
            out: ArrayQueue::new(DECODER_OUTPUT_QUEUE),
            output: config.output,
            codec: config.codec,
            file_mode: config.mode == DecodeMode::File,
            file_queue: config.output_queue,
            wake,
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("kessel-video-dec".into())
            .spawn(move || decoder_main(s, core))
            .map_err(|e| {
                VideoError::os(e.raw_os_error().unwrap_or(0) as i64, format!("spawn: {e}"))
            })?;
        Ok(ThreadedDecoder {
            shared,
            thread: Some(thread),
        })
    }
}

fn shape(
    frame: VideoFrame,
    output: PixelFormat,
    fit: Option<(u32, u32)>,
) -> Result<VideoFrame, VideoError> {
    let (w, h) = match fit {
        Some((bw, bh)) => fit_within(frame.width, frame.height, bw.max(2), bh.max(2)),
        None => (frame.width, frame.height),
    };
    if output == PixelFormat::Native {
        if (w, h) == (frame.width, frame.height) {
            return Ok(frame);
        }
        return convert(&frame, w, h, PixelFormat::I420);
    }
    if (w, h) == (frame.width, frame.height) && frame.format() == output {
        return Ok(frame);
    }
    convert(&frame, w, h, output)
}

/// One File-mode step on the decoder thread: an input, or (`None`) the
/// end-of-stream drain.
fn file_step(
    s: &DecShared,
    core: &mut Box<dyn DecoderCore>,
    input: Option<EncodedInput>,
    fit: Option<(u32, u32)>,
) {
    let mut frames = Vec::new();
    let mut depth = None;
    let result = match &input {
        Some(input) => {
            let checked = if s.codec == VideoCodec::H264 {
                crate::h264::check_file_sps(&input.data)
            } else {
                Ok(None)
            };
            match checked {
                Ok(sps) => {
                    depth = sps.map(|i| i.reorder_depth() as usize);
                    core.decode_file(input, &mut frames)
                }
                Err(e) => Err(e),
            }
        }
        None => core.flush_file(&mut frames),
    };
    let mut shaped = Vec::with_capacity(frames.len());
    let mut shape_err = None;
    for f in frames {
        let pts = f.capture_us;
        match shape(f, s.output, fit) {
            Ok(mut f) => {
                f.capture_us = pts;
                shaped.push(f);
            }
            Err(e) => shape_err = Some(DecodeError::Video(e)),
        }
    }
    let mut guard = lock(&s.state);
    let st = &mut *guard;
    st.stats.implementation = core.session_implementation();
    if st.reset {
        // A reset came in while this decoded: its output is stale.
        return;
    }
    let Some(order) = st.file.as_mut() else {
        return;
    };
    if let Some(d) = depth {
        order.depth = d;
    }
    for f in shaped {
        order.arrive(f, &mut st.stats);
    }
    let error = result.err().or(shape_err);
    if let Some(e) = error {
        st.stats.errors += 1;
        st.waiting_key = true;
        if let Some(input) = &input {
            order.pending.remove(&input.capture_us);
        }
        // Queued deltas can't decode without the lost reference.
        let mut keep = Vec::new();
        while let Some(i) = s.input.pop() {
            if i.keyframe {
                keep.push(i);
            } else {
                order.pending.remove(&i.capture_us);
            }
        }
        for i in keep {
            st.waiting_key = false;
            let _ = s.input.push(i);
        }
        // What is held is still good: it goes out first, in order.
        order.release(true, &mut st.stats);
        order.ready.push_back(Err(e));
    }
    if input.is_none() {
        order.release(true, &mut st.stats);
        order.ready.push_back(Err(DecodeError::EndOfStream));
        order.restart();
        order.draining = false;
        st.waiting_key = true;
    } else {
        order.release(false, &mut st.stats);
    }
    drop(guard);
    (s.wake)();
}

fn decoder_main(s: Arc<DecShared>, mut core: Box<dyn DecoderCore>) {
    // Real-time media: keep the thread off a low QoS class on Apple.
    #[cfg(all(feature = "apple-codecs", any(target_os = "macos", target_os = "ios")))]
    crate::apple::set_media_thread_qos();
    loop {
        // Wait for a reset, an input or (File mode) a flush.
        let (input, fit, drain) = {
            let mut st = lock(&s.state);
            loop {
                if st.stop {
                    return;
                }
                if st.reset {
                    st.reset = false;
                    break (None, st.fit, false);
                }
                if let Some(i) = s.input.pop() {
                    break (Some(i), st.fit, false);
                }
                if st.file.as_ref().is_some_and(|f| f.draining) {
                    break (None, st.fit, true);
                }
                st = s.cv.wait(st).unwrap_or_else(PoisonError::into_inner);
            }
        };
        if drain || (input.is_some() && s.file_mode) {
            file_step(&s, &mut core, input, fit);
            continue;
        }
        let Some(input) = input else {
            let r = core.reset();
            let mut st = lock(&s.state);
            st.stats.implementation = core.session_implementation();
            if let Err(e) = r {
                match st.file.as_mut() {
                    Some(order) => order.ready.push_back(Err(DecodeError::Video(e))),
                    None => {
                        s.out.force_push(Err(DecodeError::Video(e)));
                    }
                }
                st.stats.errors += 1;
                drop(st);
                (s.wake)();
            }
            continue;
        };
        let result = core.decode(&input).and_then(|f| match f {
            Some(f) => shape(f, s.output, fit)
                .map(Some)
                .map_err(DecodeError::Video),
            None => Ok(None),
        });
        let mut st = lock(&s.state);
        // The session may have opened, or reopened as another kind.
        st.stats.implementation = core.session_implementation();
        if st.reset {
            // A reset came in while this decoded: its output is stale.
            continue;
        }
        match result {
            Ok(Some(mut frame)) => {
                frame.capture_us = input.capture_us;
                st.stats.frames_out += 1;
                if s.out.force_push(Ok(frame)).is_some() {
                    st.stats.dropped_out += 1;
                }
            }
            Ok(None) => continue,
            Err(e) => {
                st.stats.errors += 1;
                st.waiting_key = true;
                // Queued deltas can't decode without the lost reference.
                let mut keep = Vec::new();
                while let Some(i) = s.input.pop() {
                    if i.keyframe {
                        keep.push(i);
                    }
                }
                for i in keep {
                    st.waiting_key = false;
                    let _ = s.input.push(i);
                }
                s.out.force_push(Err(e));
            }
        }
        drop(st);
        (s.wake)();
    }
}

/// File mode's checks of an input's times, before it is queued.
fn check_file_times(order: &FileOrder, frame: &EncodedInput) -> Result<(), DecodeError> {
    let pts = frame.capture_us;
    let bad = |why: String| Err(DecodeError::Video(VideoError::Invalid(why)));
    if pts > i64::MAX as u64 {
        return bad(format!("pts {pts} µs is over i64::MAX"));
    }
    if let Some(dts) = frame.dts_us {
        if dts > i64::MAX as u64 {
            return bad(format!("dts {dts} µs is over i64::MAX"));
        }
        if let Some(last) = order.last_dts.filter(|l| dts <= *l) {
            return bad(format!("dts {dts} µs is not after the last one, {last} µs"));
        }
    }
    if let Some(last) = order.last_out.filter(|l| pts <= *l) {
        return bad(format!(
            "pts {pts} µs is not after a frame already out, {last} µs"
        ));
    }
    if order.pending.contains(&pts) || order.held.contains_key(&pts) {
        return bad(format!("pts {pts} µs twice"));
    }
    Ok(())
}

impl VideoDecoder for ThreadedDecoder {
    fn submit(&mut self, frame: EncodedInput) -> Result<(), DecodeError> {
        let mut guard = lock(&self.shared.state);
        let st = &mut *guard;
        if st.stop {
            return Err(VideoError::ShutDown.into());
        }
        if let Some(order) = &st.file {
            if order.draining {
                return Err(DecodeError::Busy);
            }
        }
        if st.waiting_key && !frame.keyframe {
            st.stats.errors += 1;
            return Err(DecodeError::NeedKeyframe);
        }
        if let Some(order) = st.file.as_mut() {
            if let Err(e) = check_file_times(order, &frame) {
                // The stream has a hole now: go on from a keyframe.
                st.stats.errors += 1;
                st.waiting_key = true;
                return Err(e);
            }
            if order.ready.len() >= self.shared.file_queue {
                return Err(DecodeError::Busy);
            }
            let (pts, dts) = (frame.capture_us, frame.dts_us);
            if self.shared.input.push(frame).is_err() {
                return Err(DecodeError::Busy);
            }
            order.pending.insert(pts);
            if dts.is_some() {
                order.last_dts = dts;
            }
        } else if self.shared.input.push(frame).is_err() {
            return Err(DecodeError::Busy);
        }
        st.waiting_key = false;
        st.stats.frames_in += 1;
        drop(guard);
        self.shared.cv.notify_one();
        Ok(())
    }

    fn poll(&mut self) -> Option<Result<VideoFrame, DecodeError>> {
        let mut st = lock(&self.shared.state);
        match st.file.as_mut() {
            Some(order) => order.ready.pop_front(),
            None => {
                drop(st);
                self.shared.out.pop()
            }
        }
    }

    fn flush(&mut self) -> Result<(), DecodeError> {
        let mut st = lock(&self.shared.state);
        if st.stop {
            return Err(VideoError::ShutDown.into());
        }
        let Some(order) = st.file.as_mut() else {
            return Err(DecodeError::Video(VideoError::Unsupported(
                "flush is for DecodeMode::File".into(),
            )));
        };
        order.draining = true;
        drop(st);
        self.shared.cv.notify_one();
        Ok(())
    }

    fn reset(&mut self) {
        let mut st = lock(&self.shared.state);
        while self.shared.input.pop().is_some() {}
        while self.shared.out.pop().is_some() {}
        if let Some(order) = st.file.as_mut() {
            order.restart();
            order.ready.clear();
            order.draining = false;
        }
        st.reset = true;
        st.waiting_key = true;
        drop(st);
        self.shared.cv.notify_one();
    }

    fn set_fit(&mut self, fit: Option<(u32, u32)>) {
        lock(&self.shared.state).fit = fit;
    }

    fn stats(&self) -> DecoderStats {
        lock(&self.shared.state).stats
    }
}

impl Drop for ThreadedDecoder {
    fn drop(&mut self) {
        lock(&self.shared.state).stop = true;
        self.shared.cv.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ───────────────────────────── capture ─────────────────────────────

/// What a capture backend shares with its OS callback: the frame and event
/// queues, pause, constraints (fps decimation and scaling to fit), and the
/// counters.
pub struct SourceShared {
    frames: ArrayQueue<VideoFrame>,
    events: ArrayQueue<SourceEvent>,
    wake: Wake,
    paused: AtomicBool,
    ended: AtomicBool,
    constraints: Mutex<CaptureConstraints>,
    stats: Mutex<SourceStats>,
    last_kept_us: AtomicU64,
    native_fps: Mutex<u32>,
    box_follows_frame: AtomicBool,
}

impl std::fmt::Debug for SourceShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceShared")
            .field("stats", &*lock(&self.stats))
            .finish_non_exhaustive()
    }
}

impl SourceShared {
    /// A source's queues. `native_fps` is what the device runs at.
    pub fn new(constraints: CaptureConstraints, native_fps: u32, wake: Wake) -> Arc<SourceShared> {
        Arc::new(SourceShared {
            frames: ArrayQueue::new(CAPTURE_QUEUE),
            events: ArrayQueue::new(32),
            wake,
            paused: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            constraints: Mutex::new(constraints),
            stats: Mutex::new(SourceStats::default()),
            last_kept_us: AtomicU64::new(u64::MAX),
            native_fps: Mutex::new(native_fps),
            box_follows_frame: AtomicBool::new(false),
        })
    }

    /// Phone cameras: turn the asked box to the frame's orientation before
    /// fitting, so a 1280 x 720 box gives 720 x 1280 frames while the phone
    /// is held upright (instead of 405 x 720). Off by default.
    pub fn set_box_follows_frame(&self, on: bool) {
        self.box_follows_frame.store(on, Ordering::Release);
    }

    /// Paused or ended: the OS callback can skip its work.
    pub fn idle(&self) -> bool {
        self.paused.load(Ordering::Acquire) || self.ended.load(Ordering::Acquire)
    }

    /// `Ended` was sent: no more frames.
    pub fn has_ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }

    /// The device's own rate changed.
    pub fn set_native_fps(&self, fps: u32) {
        *lock(&self.native_fps) = fps;
    }

    /// The constraints now.
    pub fn constraints(&self) -> CaptureConstraints {
        *lock(&self.constraints)
    }

    /// Hand over a captured frame: dropped while paused; dropped to keep
    /// the asked rate; scaled down to fit the asked size; queued (oldest
    /// dropped); `Started` when the delivered size or rate changes; wake.
    pub fn deliver(&self, frame: VideoFrame) {
        if self.idle() {
            return;
        }
        let c = self.constraints();
        let native = *lock(&self.native_fps);
        let fps = c.fps.map_or(native, |f| f.min(native.max(1)));
        let last = self.last_kept_us.load(Ordering::Acquire);
        if last != u64::MAX && fps > 0 {
            let interval = 1_000_000 / fps as u64;
            if frame.capture_us.saturating_sub(last) + interval / 4 < interval {
                lock(&self.stats).dropped += 1;
                return;
            }
        }
        self.last_kept_us.store(frame.capture_us, Ordering::Release);
        let (mut bw, mut bh) = (
            c.max_width.unwrap_or(u32::MAX),
            c.max_height.unwrap_or(u32::MAX),
        );
        if self.box_follows_frame.load(Ordering::Acquire)
            && (frame.width > frame.height) != (bw > bh)
            && frame.width != frame.height
            && bw != bh
        {
            std::mem::swap(&mut bw, &mut bh);
        }
        let frame = if frame.width > bw || frame.height > bh {
            let (w, h) = fit_within(frame.width, frame.height, bw, bh);
            let format = match frame.format() {
                PixelFormat::Native => PixelFormat::Nv12,
                f => f,
            };
            match convert(&frame, w, h, format) {
                Ok(f) => f,
                Err(e) => {
                    self.event(SourceEvent::Error(e));
                    return;
                }
            }
        } else {
            frame
        };
        let started = {
            let mut st = lock(&self.stats);
            st.frames += 1;
            let changed = (st.width, st.height, st.fps) != (frame.width, frame.height, fps);
            st.width = frame.width;
            st.height = frame.height;
            st.fps = fps;
            changed.then_some((frame.width, frame.height, fps))
        };
        if let Some((width, height, fps)) = started {
            self.events
                .force_push(SourceEvent::Started { width, height, fps });
        }
        if self.frames.force_push(frame).is_some() {
            lock(&self.stats).dropped += 1;
        }
        (self.wake)();
    }

    /// Queue an event and wake. `Ended` is sent once; frames stop after it.
    pub fn event(&self, e: SourceEvent) {
        if let SourceEvent::Ended(_) = e {
            if self.ended.swap(true, Ordering::AcqRel) {
                return;
            }
        }
        self.events.force_push(e);
        (self.wake)();
    }
}

/// The platform side of a source: device control. Dropping it closes the
/// device.
pub trait SourceControl: Send {
    /// Pause or resume at the device (optional: the queue drops frames while
    /// paused anyway).
    fn set_paused(&mut self, _paused: bool) {}
    /// Apply new constraints at the device (optional: the queue scales and
    /// drops to fit anyway).
    fn set_constraints(&mut self, _c: &CaptureConstraints) -> Result<(), VideoError> {
        Ok(())
    }
    /// macOS and iOS: the source's capture session for a self-view, as
    /// [`FrameSource::apple_preview`] (optional: `None`, the default, for
    /// a source without one).
    #[cfg(all(feature = "apple", any(target_os = "macos", target_os = "ios")))]
    fn apple_preview(&self) -> Option<crate::ApplePreview> {
        None
    }
}

/// A [`FrameSource`] made of a [`SourceShared`] and a [`SourceControl`].
pub struct QueuedSource<C: SourceControl> {
    shared: Arc<SourceShared>,
    control: C,
}

impl<C: SourceControl> std::fmt::Debug for QueuedSource<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueuedSource")
            .field("shared", &self.shared)
            .finish_non_exhaustive()
    }
}

impl<C: SourceControl> QueuedSource<C> {
    /// A source.
    pub fn new(shared: Arc<SourceShared>, control: C) -> QueuedSource<C> {
        QueuedSource { shared, control }
    }
}

impl<C: SourceControl> FrameSource for QueuedSource<C> {
    fn try_recv(&mut self) -> Option<VideoFrame> {
        self.shared.frames.pop()
    }

    fn try_event(&mut self) -> Option<SourceEvent> {
        self.shared.events.pop()
    }

    fn set_constraints(&mut self, c: CaptureConstraints) -> Result<(), VideoError> {
        c.validate()?;
        self.control.set_constraints(&c)?;
        *lock(&self.shared.constraints) = c;
        Ok(())
    }

    fn set_paused(&mut self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Release);
        if paused {
            while self.shared.frames.pop().is_some() {}
        }
        self.control.set_paused(paused);
    }

    #[cfg(all(feature = "apple", any(target_os = "macos", target_os = "ios")))]
    fn apple_preview(&self) -> Option<crate::ApplePreview> {
        self.control.apple_preview()
    }

    fn stats(&self) -> SourceStats {
        *lock(&self.shared.stats)
    }
}

/// Wait until `f` gives `Some` or `timeout` passes (tests and blocking
/// helpers).
pub fn wait_for<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(t) = f() {
            return Some(t);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::no_wake;
    use crate::fake::pattern_frame;

    #[test]
    fn the_box_turns_with_the_frame_only_when_asked() {
        let s = SourceShared::new(CaptureConstraints::new(1280, 720, 30), 30, no_wake());
        let upright = |n: u32| pattern_frame(1080, 1920, n, n as u64 * 40_000).expect("frame");
        s.deliver(upright(0));
        let f = s.frames.pop().expect("a frame");
        assert_eq!(f.height, 720, "fitted to the box as given");
        assert!(f.width < 720, "{}x{}", f.width, f.height);

        s.set_box_follows_frame(true);
        s.deliver(upright(1));
        let f = s.frames.pop().expect("a frame");
        assert_eq!((f.width, f.height), (720, 1280), "the box turned upright");
        s.deliver(pattern_frame(1920, 1080, 2, 80_000).expect("frame"));
        let f = s.frames.pop().expect("a frame");
        assert_eq!(
            (f.width, f.height),
            (1280, 720),
            "a landscape frame keeps the box"
        );
    }

    #[test]
    fn a_frame_fitted_to_the_box_keeps_its_host_time() {
        use crate::frame::{HostClock, HostTime};
        let s = SourceShared::new(CaptureConstraints::new(640, 360, 30), 30, no_wake());
        let t = Some(HostTime::new(77_000, HostClock::MachAbsoluteTime));
        let mut big = pattern_frame(1280, 720, 1, 0).expect("frame");
        big.host_time = t;
        s.deliver(big);
        let f = s.frames.pop().expect("a frame");
        assert_eq!((f.width, f.height), (640, 360), "scaled to the box");
        assert_eq!(f.host_time, t);
        // One that fits passes through as it is.
        let mut small = pattern_frame(320, 180, 2, 40_000).expect("frame");
        small.host_time = t;
        s.deliver(small);
        assert_eq!(s.frames.pop().expect("a frame").host_time, t);
    }

    #[test]
    fn a_fake_camera_frame_has_no_host_time() {
        use crate::api::VideoBackend;
        let backend = crate::fake::FakeBackend::new(crate::fake::FakeConfig::default());
        let mut camera = backend
            .open_camera(None, Default::default(), no_wake())
            .expect("open");
        let f = wait_for(Duration::from_secs(5), || camera.try_recv()).expect("a frame");
        assert_eq!(f.host_time, None);
    }
}
