//! Capture and encode (PRD remote desktop §3.5, slices D5 and D8b): one OS
//! capture per desktop, one encode per layer, protocol-4 units out through
//! the frames pipe. Platform-neutral: the OS end is an [`Opener`] (macOS:
//! ScreenCaptureKit through kessel-video, `mac::capture`; Linux: X11
//! through kessel-video, `x11::capture`), the encoders come from a
//! kessel-video backend (VideoToolbox H.264 on macOS, software VP8 on
//! Linux) or, for JPEG, from [`crate::jpeg`], and the tests drive it with
//! fakes on any OS.
//!
//! **Codecs** are the platform's set, given at spawn ([`H264_ONLY`] on
//! macOS, [`VP8_JPEG`] on Linux) and listed in `hello.codecs`. A layer in
//! another codec fails at once (`layer_failed`, `unsupported`). The
//! software codecs keep their ceilings whatever the layer asks
//! ([`capped_width`], [`capped_fps`]).
//!
//! **Layers** come from the daemon's `layers` request (the engine's hub:
//! the `full` layer for this machine and the LAN, the `remote` one for
//! Connect, each with its id, codec, width, rate and bitrate). A layer
//! missing from the list, or `on: false`, is paused: its encoder closes.
//! With no layer on there is no capture at all.
//!
//! **One capture** (the v1 rule, K2 2026-11-10): the display deskd captures
//! is a property of the desktop, so every layer shows the same display. The
//! capture runs at the [`Plan`]: the widest layer (at most the display's
//! own width) and the highest layer rate, so ScreenCaptureKit's
//! `minimumFrameInterval` comes from the layer rate (R3). A change of plan
//! reopens the capture (its first frame is fresh); a bitrate change never
//! does.
//!
//! **Pacing.** Each layer keeps the newest frame it hasn't encoded and
//! encodes it once its interval has passed, so a 30 fps layer under a
//! 60 fps capture takes every other frame and never ends on a stale one.
//!
//! **Encode only on change** (R7). The OS gives a frame only when the
//! screen changed (ScreenCaptureKit drops idle frames), so a still screen
//! encodes nothing. The one exception: a layer that owes a keyframe (it
//! started, or the daemon asked) on a still screen encodes the last frame
//! again after [`REPEAT_AFTER_US`], stamped with the time of the repeat (the
//! picture is current: nothing changed).
//!
//! **Keyframes** (R4) only on a layer's start, a size or display change and
//! a `keyframe` request, never on a timer. A request's `since_ns` puts the
//! keyframe on the first frame captured at or after it (VideoToolbox would
//! otherwise key an older frame still in the pipe). A layer's first unit
//! after it starts, after a size change or after a display switch is a key
//! unit with `source start`; deltas before it, and units of a display the
//! layer has left, are dropped here, never sent.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use kessel_video::{fit_within, CodecConfig, ContentHint, EncodeOptions, EncoderConfig, Implementation, Rates, SourceEvent, VideoBackend, VideoEncoder, VideoError, VideoFrame, Wake};

use k2_deskd_proto::link::{Codec, Encoder as LinkEncoder, LayerSpec, UnitHeader};
use k2_deskd_proto::msg::{codes, Event};

pub use kessel_video::FrameSource;

/// A layer that owes a keyframe on a still screen encodes the last frame
/// again after this long without a fresh one.
pub const REPEAT_AFTER_US: u64 = 50_000;
/// A capture that failed to open, or ended, is tried again after this.
pub const RETRY_US: u64 = 2_000_000;
/// The highest capture rate deskd asks for.
pub const MAX_FPS: u32 = 120;
/// How long the capture thread sleeps with nothing to do (commands, frames
/// and encoder output wake it sooner).
const IDLE_STEP: Duration = Duration::from_millis(250);
/// With units in an encoder, look again at least this often (in case a
/// wake was missed).
const INFLIGHT_STEP: Duration = Duration::from_millis(20);
/// A layer whose encoder dies this many times in a row is failed.
const MAX_ENCODER_FAILURES: u32 = 2;
/// An encoder that answered busy is tried again at its next output, or
/// after this at the latest (never in a tight loop).
pub const BUSY_BACKOFF_US: u64 = 4_000;

/// deskd's clock in µs: kessel-video's (every frame's `capture_us`).
pub fn now_us() -> u64 {
    kessel_video::now_us()
}

/// deskd's clock in ns (`hello.clock_ns`, every unit's times).
pub fn clock_ns() -> u64 {
    now_us().saturating_mul(1000)
}

/// Real-time media on the calling thread: user-interactive QoS on macOS
/// (kessel-video's encoder threads run at user-initiated), so the system
/// doesn't coalesce its timed waits or put it on the efficiency cores'
/// background tier. Nothing elsewhere.
pub fn realtime_qos() {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        /// `QOS_CLASS_USER_INTERACTIVE` from `<sys/qos.h>`.
        const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
        // SAFETY: sets the calling thread's own QoS class; plain integers.
        // On failure the thread keeps the class it started with.
        let _ = unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
    }
}

/// What one capture runs at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// The display (CG display id on macOS).
    pub display: u32,
    /// The widest picture any layer wants, in pixels (at most the
    /// display's own width).
    pub width: u32,
    /// The highest layer rate (ScreenCaptureKit's `minimumFrameInterval`).
    pub fps: u32,
}

/// The OS end: a display's capture.
pub trait Opener: Send + 'static {
    /// The display captured when nobody chose one (the main display), or
    /// `None` with no display at all.
    fn default_display(&mut self) -> Option<u32>;
    /// A display's own size in pixels; `None` = no such display now.
    fn native_size(&mut self, display: u32) -> Option<(u32, u32)>;
    /// Starts capturing `plan.display`, at most `plan.width` wide, at
    /// `plan.fps`, giving frames only when the screen changed.
    fn open(&mut self, plan: &Plan, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError>;
}

/// Where the engine's output goes: units to the frames pipe, events to the
/// control link.
pub trait Out: Send + Sync + 'static {
    /// `false` when there is no link (the unit is dropped).
    fn unit(&self, h: &UnitHeader, data: &[u8]) -> bool;
    fn event(&self, e: &Event);
}

/// macOS: VideoToolbox H.264 (the default set).
pub const H264_ONLY: &[Codec] = &[Codec::H264];
/// Linux X11: software VP8, and JPEG for viewers that decode no video.
pub const VP8_JPEG: &[Codec] = &[Codec::Vp8, Codec::Jpeg];

/// The widest picture the software codecs make (RD §3.5).
pub const SOFTWARE_MAX_WIDTH: u32 = 1920;
/// VP8's rate above 720 lines, and at or below them.
pub const VP8_FPS_ABOVE_720: u32 = 15;
pub const VP8_FPS_UP_TO_720: u32 = 30;
/// JPEG's highest rate.
pub const JPEG_MAX_FPS: u32 = 15;

/// Does the default (macOS) set encode `codec`?
pub fn encodes(codec: Codec) -> bool {
    H264_ONLY.contains(&codec)
}

/// A layer's widest picture (0 = as captured) within its codec's ceiling:
/// VP8 and JPEG at most [`SOFTWARE_MAX_WIDTH`]; H.264 as asked.
pub fn capped_width(codec: Codec, max_width: u32) -> u32 {
    match codec {
        Codec::H264 => max_width,
        Codec::Vp8 | Codec::Jpeg if max_width == 0 => SOFTWARE_MAX_WIDTH,
        Codec::Vp8 | Codec::Jpeg => max_width.min(SOFTWARE_MAX_WIDTH),
    }
}

/// A layer's rate within its codec's ceiling for pictures `height` lines
/// tall (0 = not known yet: the larger picture's ceiling).
pub fn capped_fps(codec: Codec, fps: u32, height: u32) -> u32 {
    let cap = match codec {
        Codec::H264 => MAX_FPS,
        Codec::Vp8 if height == 0 || height > 720 => VP8_FPS_ABOVE_720,
        Codec::Vp8 => VP8_FPS_UP_TO_720,
        Codec::Jpeg => JPEG_MAX_FPS,
    };
    fps.clamp(1, cap)
}

/// The capture for `layers` on `display` (own size `native`) with the
/// default (macOS) codecs: see [`capture_plan_for`].
pub fn capture_plan(layers: &[LayerSpec], display: u32, native: (u32, u32)) -> Option<Plan> {
    capture_plan_for(layers, H264_ONLY, display, native)
}

/// The capture for `layers` on `display` (own size `native`): the widest
/// layer, at most the display's width, and the highest rate a layer runs
/// at. `None` when no layer in `codecs` is on.
pub fn capture_plan_for(layers: &[LayerSpec], codecs: &[Codec], display: u32, native: (u32, u32)) -> Option<Plan> {
    let on: Vec<&LayerSpec> = layers.iter().filter(|l| l.on && codecs.contains(&l.codec)).collect();
    let nw = native.0.max(16);
    let width = on
        .iter()
        .map(|l| match capped_width(l.codec, l.max_width) {
            0 => nw,
            w => w.min(nw),
        })
        .max()?
        .max(16);
    // Each layer's rate within its codec's ceiling at the size it will
    // encode: the capture never reads faster than some layer encodes.
    let fps = on.iter().map(|l| capped_fps(l.codec, l.fps, layer_size(native, capped_width(l.codec, l.max_width)).1)).max()?.clamp(1, MAX_FPS);
    Some(Plan { display, width, fps })
}

/// A layer's coded size for a captured frame: the frame scaled down to the
/// layer's width (0 = as captured), aspect kept, even, at least 16.
pub fn layer_size(frame: (u32, u32), max_width: u32) -> (u32, u32) {
    let (w, h) = if max_width == 0 || max_width >= frame.0 { frame } else { fit_within(frame.0, frame.1, max_width, u32::MAX) };
    ((w & !1).max(16), (h & !1).max(16))
}

/// The protocol-4 name for what a session runs on.
pub fn link_encoder(i: Implementation) -> LinkEncoder {
    match i {
        Implementation::Hardware => LinkEncoder::Hardware,
        Implementation::Software => LinkEncoder::Software,
        _ => LinkEncoder::Unknown,
    }
}

/// Opens a small H.264 session to see what the OS gives (`hello.encoder`,
/// R9), and closes it. `Unknown` when none opens.
pub fn probe_encoder(backend: &dyn VideoBackend) -> LinkEncoder {
    probe_encoder_for(backend, Codec::H264)
}

/// [`probe_encoder`] for `codec`'s encoder (JPEG: deskd's own, software).
pub fn probe_encoder_for(backend: &dyn VideoBackend, codec: Codec) -> LinkEncoder {
    match open_encoder(backend, codec, (1280, 720), 30, 2_000_000, None, kessel_video::no_wake()) {
        Ok(e) => link_encoder(e.implementation()),
        Err(_) => LinkEncoder::Unknown,
    }
}

fn encoder_config(codec: Codec, size: (u32, u32), fps: u32, bitrate: u32) -> EncoderConfig {
    let (fps, bitrate) = (fps.clamp(1, MAX_FPS), bitrate.max(10_000));
    let mut c = match codec {
        Codec::Vp8 => EncoderConfig::new(CodecConfig::Vp8, size.0, size.1, fps, bitrate),
        _ => EncoderConfig::h264(size.0, size.1, fps, bitrate),
    };
    // Text and UI: keep detail, lower the frame rate first.
    c.content = ContentHint::Screen;
    c
}

/// A layer's encoder: JPEG is deskd's own ([`crate::jpeg`]); video comes
/// from the backend.
fn open_encoder(backend: &dyn VideoBackend, codec: Codec, size: (u32, u32), fps: u32, bitrate: u32, quality: Option<u8>, wake: Wake) -> Result<Box<dyn VideoEncoder>, VideoError> {
    match codec {
        Codec::Jpeg => crate::jpeg::encoder(size.0, size.1, fps, quality.unwrap_or(crate::jpeg::DEFAULT_QUALITY), wake),
        video => backend.encoder(encoder_config(video, size, fps, bitrate), wake),
    }
}

fn error_code(e: &VideoError) -> &'static str {
    match e {
        VideoError::NoPermission => codes::NO_SCREEN_PERMISSION,
        VideoError::NotFound | VideoError::DeviceLost => codes::NO_SUCH_DISPLAY,
        _ => codes::CAPTURE_FAILED,
    }
}

/// What the engine has done (the live test's numbers).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Captures opened.
    pub opens: u64,
    /// Frames the OS gave.
    pub frames: u64,
    /// Frames submitted to an encoder (all layers).
    pub submitted: u64,
    /// Of those, repeats of the last frame on a still screen.
    pub repeats: u64,
    /// Units sent.
    pub units: u64,
    /// Of those, key units.
    pub keys: u64,
    /// Encoder outputs not sent (a delta before a layer's start key, or a
    /// display the layer left).
    pub dropped: u64,
    /// Submits an encoder refused as busy (the frame waits for the next
    /// step, or a newer one replaces it).
    pub busy: u64,
    /// Frames the running encoders' rate control skipped (kessel-video's
    /// `dropped_by_rc`, summed over the layers running now).
    pub rc_dropped: u64,
}

/// One layer's encode.
struct Run {
    spec: LayerSpec,
    enc: Option<Box<dyn VideoEncoder>>,
    /// The coded size the encoder has.
    size: Option<(u32, u32)>,
    /// The display its units show.
    display: Option<u32>,
    seq: u64,
    /// The newest frame not yet encoded, and its display.
    pending: Option<(VideoFrame, u32)>,
    /// When the last frame went into the encoder (pacing).
    kept_at_us: Option<u64>,
    /// A keyframe is owed on the first frame captured at or after this.
    want_key: Option<u64>,
    /// When the keyframe became owed (for the still-screen repeat).
    owed_at_us: u64,
    /// The next unit must be a key with `source start`.
    start_owed: bool,
    /// A key frame has gone in for the owed start.
    start_key_in: bool,
    /// Frames in the encoder, oldest first: (capture_us, display).
    inflight: VecDeque<(u64, u32)>,
    failures: u32,
    /// The encoder answered busy: no submit before this (or its next
    /// output, whichever comes first).
    busy_until_us: Option<u64>,
}

impl Run {
    fn new(spec: LayerSpec, now_us: u64) -> Run {
        Run {
            spec,
            enc: None,
            size: None,
            display: None,
            seq: 0,
            pending: None,
            kept_at_us: None,
            want_key: Some(0),
            owed_at_us: now_us,
            start_owed: true,
            start_key_in: false,
            inflight: VecDeque::new(),
            failures: 0,
            busy_until_us: None,
        }
    }

    /// The rate it runs at: the layer's, within its codec's ceiling for
    /// the size it encodes.
    fn fps(&self) -> u32 {
        capped_fps(self.spec.codec, self.spec.fps, self.size.map_or(0, |s| s.1))
    }

    fn interval_us(&self) -> u64 {
        1_000_000 / self.fps().clamp(1, MAX_FPS) as u64
    }

    fn due_at(&self) -> u64 {
        // A quarter interval of slack, as kessel-video's own pacing: a frame
        // that comes a little early isn't held a whole interval.
        let i = self.interval_us();
        self.kept_at_us.map_or(0, |t| t + i - i / 4)
    }

    fn owe_key(&mut self, since_us: u64, now_us: u64) {
        if self.want_key.is_none() {
            self.owed_at_us = now_us;
        }
        self.want_key = Some(self.want_key.map_or(since_us, |s| s.max(since_us)));
    }

    /// Back to a fresh start (a new encoder, a key with `source start`).
    fn restart(&mut self, now_us: u64) {
        self.enc = None;
        self.size = None;
        self.inflight.clear();
        self.start_owed = true;
        self.start_key_in = false;
        self.owe_key(0, now_us);
    }
}

/// The capture and its layers. [`Capture`] runs it on its own thread; the
/// tests drive it step by step.
pub struct Engine<O: Opener> {
    opener: O,
    backend: Arc<dyn VideoBackend>,
    wake: Wake,
    /// What it encodes ([`H264_ONLY`], [`VP8_JPEG`]).
    codecs: Vec<Codec>,
    runs: Vec<Run>,
    /// Layers on in the list that can't run (reported once).
    failed: HashSet<u8>,
    /// The display chosen by `display`, if any (else the main one).
    chosen: Option<u32>,
    source: Option<(Plan, Box<dyn FrameSource>)>,
    /// The last frame captured, and its display (for still-screen repeats).
    last: Option<(VideoFrame, u32)>,
    retry_at_us: Option<u64>,
    stats: Stats,
}

impl<O: Opener> Engine<O> {
    /// An engine for the default (macOS) codecs.
    pub fn new(opener: O, backend: Arc<dyn VideoBackend>, wake: Wake) -> Engine<O> {
        Engine::with_codecs(opener, backend, wake, H264_ONLY)
    }

    /// An engine that encodes `codecs`.
    pub fn with_codecs(opener: O, backend: Arc<dyn VideoBackend>, wake: Wake, codecs: &[Codec]) -> Engine<O> {
        Engine { opener, backend, wake, codecs: codecs.to_vec(), runs: Vec::new(), failed: HashSet::new(), chosen: None, source: None, last: None, retry_at_us: None, stats: Stats::default() }
    }

    /// What it encodes.
    pub fn codecs(&self) -> &[Codec] {
        &self.codecs
    }

    pub fn stats(&self) -> Stats {
        let rc_dropped = self.runs.iter().filter_map(|r| r.enc.as_ref()).map(|e| e.stats().dropped_by_rc).sum();
        Stats { rc_dropped, ..self.stats }
    }

    /// The capture running now.
    pub fn plan(&self) -> Option<Plan> {
        self.source.as_ref().map(|s| s.0)
    }

    /// The layers running now (ids).
    pub fn running(&self) -> Vec<u8> {
        self.runs.iter().map(|r| r.spec.id).collect()
    }

    /// The whole layer list (replaces the last one).
    pub fn set_layers(&mut self, specs: &[LayerSpec], now_us: u64, out: &dyn Out) {
        let mut next = Vec::new();
        let mut failed = HashSet::new();
        for spec in specs.iter().filter(|s| s.on) {
            if self.failed.contains(&spec.id) {
                failed.insert(spec.id);
                continue;
            }
            if !self.codecs.contains(&spec.codec) {
                out.event(&Event::LayerFailed { layer: spec.id, code: codes::UNSUPPORTED.into(), error: format!("K2 Screen can't encode {} here", spec.codec.as_str()) });
                failed.insert(spec.id);
                continue;
            }
            // A JPEG layer's quality is set when its encoder opens: a new
            // quality is a new layer.
            match self.runs.iter().position(|r| r.spec.id == spec.id && r.spec.codec == spec.codec && r.spec.quality == spec.quality) {
                Some(i) => {
                    let mut r = self.runs.swap_remove(i);
                    if (r.spec.fps, r.spec.bitrate) != (spec.fps, spec.bitrate) {
                        let fps = capped_fps(spec.codec, spec.fps, r.size.map_or(0, |s| s.1));
                        if let Some(e) = r.enc.as_mut() {
                            let _ = e.set_rates(Rates::new(spec.bitrate.max(10_000), fps));
                        }
                    }
                    // A new width shows at the next frame (a size change: a key).
                    r.spec = *spec;
                    next.push(r);
                }
                None => next.push(Run::new(*spec, now_us)),
            }
        }
        // Layers that left the list close their encoders here.
        self.runs = next;
        self.failed = failed;
        self.reconcile(now_us, out);
    }

    /// A keyframe on `layer` on the first frame captured at or after
    /// `since_ns` (absent: the next frame).
    pub fn keyframe(&mut self, layer: u8, since_ns: Option<u64>, now_us: u64) {
        if let Some(r) = self.runs.iter_mut().find(|r| r.spec.id == layer) {
            r.owe_key(since_ns.map_or(0, |n| n / 1000), now_us);
        }
    }

    /// New rate targets for `layer`, without a keyframe.
    pub fn rate(&mut self, layer: u8, fps: u32, bitrate: u32, now_us: u64, out: &dyn Out) {
        let Some(r) = self.runs.iter_mut().find(|r| r.spec.id == layer) else { return };
        r.spec.fps = fps;
        r.spec.bitrate = bitrate;
        let fps = r.fps();
        if let Some(e) = r.enc.as_mut() {
            let _ = e.set_rates(Rates::new(bitrate.max(10_000), fps));
        }
        self.reconcile(now_us, out);
    }

    /// Captures `display` from now on (every layer moves).
    pub fn select_display(&mut self, display: u32, now_us: u64, out: &dyn Out) -> Result<u32, (&'static str, String)> {
        if self.opener.native_size(display).is_none() {
            return Err((codes::NO_SUCH_DISPLAY, format!("no display {display}")));
        }
        let showing = self.plan().map(|p| p.display);
        self.chosen = Some(display);
        if showing.is_some() && showing != Some(display) {
            for r in &mut self.runs {
                // The old display's frame never goes in after the switch.
                r.pending = None;
                r.owe_key(now_us, now_us);
            }
        }
        self.reconcile(now_us, out);
        Ok(display)
    }

    /// The link closed: every layer off, the capture closed, the display
    /// back to the main one.
    pub fn reset(&mut self) {
        self.runs.clear();
        self.failed.clear();
        self.source = None;
        self.last = None;
        self.chosen = None;
        self.retry_at_us = None;
    }

    fn display(&mut self) -> Option<u32> {
        if let Some(d) = self.chosen {
            if self.opener.native_size(d).is_some() {
                return Some(d);
            }
            // The chosen display went away: back to the main one.
            self.chosen = None;
        }
        self.opener.default_display()
    }

    /// Opens, reopens or closes the capture to match the layers.
    fn reconcile(&mut self, now_us: u64, out: &dyn Out) {
        if self.runs.is_empty() {
            self.source = None;
            self.retry_at_us = None;
            return;
        }
        let Some(display) = self.display() else {
            self.source = None;
            self.capture_error(0, codes::NO_SUCH_DISPLAY, "this computer has no display to capture".into(), now_us, out);
            return;
        };
        let Some(native) = self.opener.native_size(display) else {
            self.source = None;
            self.capture_error(display, codes::NO_SUCH_DISPLAY, format!("display {display} went away"), now_us, out);
            return;
        };
        let specs: Vec<LayerSpec> = self.runs.iter().map(|r| r.spec).collect();
        let Some(plan) = capture_plan_for(&specs, &self.codecs, display, native) else {
            self.source = None;
            return;
        };
        if self.plan() == Some(plan) {
            return;
        }
        // One capture: close the old one before the new one starts.
        self.source = None;
        if self.last.as_ref().is_some_and(|(_, d)| *d != display) {
            self.last = None;
        }
        match self.opener.open(&plan, Arc::clone(&self.wake)) {
            Ok(src) => {
                self.stats.opens += 1;
                self.source = Some((plan, src));
                self.retry_at_us = None;
            }
            Err(e) => self.capture_error(display, error_code(&e), e.to_string(), now_us, out),
        }
    }

    fn capture_error(&mut self, display: u32, code: &str, error: String, now_us: u64, out: &dyn Out) {
        out.event(&Event::CaptureError { display, code: code.into(), error });
        self.retry_at_us = Some(now_us + RETRY_US);
    }

    /// One step: the capture's frames into the layers, due frames into the
    /// encoders, encoded units out. Returns how long the caller may wait
    /// before the next step (a command, a frame or an encoder output wakes
    /// it sooner).
    pub fn step(&mut self, now_us: u64, out: &dyn Out) -> Duration {
        if self.source.is_none() && self.retry_at_us.is_some_and(|t| now_us >= t) {
            self.retry_at_us = None;
            self.reconcile(now_us, out);
        }
        self.take_frames(now_us, out);
        self.repeat_for_keys(now_us);
        for i in 0..self.runs.len() {
            self.submit_due(i, now_us, out);
        }
        for i in 0..self.runs.len() {
            self.drain(i, now_us, out);
        }
        self.runs.retain(|r| !self.failed.contains(&r.spec.id));
        self.next_wait(now_us)
    }

    fn take_frames(&mut self, now_us: u64, out: &dyn Out) {
        let Some((plan, src)) = self.source.as_mut() else { return };
        let display = plan.display;
        let mut ended = None;
        while let Some(ev) = src.try_event() {
            if let SourceEvent::Ended(reason) = ev {
                ended = Some(reason);
            }
        }
        let mut frames = Vec::new();
        while let Some(f) = src.try_recv() {
            frames.push(f);
        }
        for f in frames {
            self.stats.frames += 1;
            for r in &mut self.runs {
                r.pending = Some((f.clone(), display));
            }
            self.last = Some((f, display));
        }
        if let Some(reason) = ended {
            self.source = None;
            self.capture_error(display, codes::CAPTURE_FAILED, format!("the capture ended ({reason:?})"), now_us, out);
        }
    }

    /// A layer that owes a keyframe on a still screen gets the last frame
    /// again, stamped now (nothing changed since, so it is current).
    fn repeat_for_keys(&mut self, now_us: u64) {
        let Some(display) = self.plan().map(|p| p.display) else { return };
        let Some((last, last_display)) = self.last.as_ref() else { return };
        if *last_display != display {
            return;
        }
        for r in &mut self.runs {
            if r.want_key.is_some() && r.pending.is_none() && now_us >= r.owed_at_us + REPEAT_AFTER_US {
                let mut f = last.clone();
                f.capture_us = now_us.max(last.capture_us + 1);
                r.pending = Some((f, display));
                // Not again until this one is in.
                r.owed_at_us = now_us;
                self.stats.repeats += 1;
            }
        }
    }

    fn fail(&mut self, i: usize, code: &str, error: String, open_failed: bool, now_us: u64, out: &dyn Out) {
        let r = &mut self.runs[i];
        r.failures += 1;
        if open_failed || r.failures >= MAX_ENCODER_FAILURES {
            out.event(&Event::LayerFailed { layer: r.spec.id, code: code.into(), error });
            self.failed.insert(r.spec.id);
            r.enc = None;
            r.pending = None;
            return;
        }
        r.restart(now_us);
    }

    fn submit_due(&mut self, i: usize, now_us: u64, out: &dyn Out) {
        let backend = Arc::clone(&self.backend);
        let wake = Arc::clone(&self.wake);
        let r = &mut self.runs[i];
        if self.failed.contains(&r.spec.id) || r.pending.is_none() || now_us < r.due_at() || r.busy_until_us.is_some_and(|t| now_us < t) {
            return;
        }
        let Some((frame, display)) = r.pending.take() else { return };
        let size = layer_size((frame.width, frame.height), capped_width(r.spec.codec, r.spec.max_width));
        let codec = r.spec.codec;
        if r.enc.is_none() {
            let fps = capped_fps(codec, r.spec.fps, size.1);
            match open_encoder(&*backend, codec, size, fps, r.spec.bitrate, r.spec.quality, wake) {
                Ok(e) => {
                    r.enc = Some(e);
                    r.size = Some(size);
                    r.start_owed = true;
                    r.start_key_in = false;
                }
                Err(e) => return self.fail(i, codes::ENCODER_FAILED, format!("no {} encoder for {}x{}: {e}", codec.as_str(), size.0, size.1), true, now_us, out),
            }
        } else if r.size != Some(size) {
            if let Some(Err(e)) = r.enc.as_mut().map(|enc| enc.set_size(size.0, size.1)) {
                return self.fail(i, codes::ENCODER_FAILED, format!("resize to {}x{}: {e}", size.0, size.1), false, now_us, out);
            }
            r.size = Some(size);
            r.start_owed = true;
            r.start_key_in = false;
            // A new height can move the codec's rate ceiling.
            let fps = r.fps();
            if let Some(e) = r.enc.as_mut() {
                let _ = e.set_rates(Rates::new(r.spec.bitrate.max(10_000), fps));
            }
        }
        if r.display != Some(display) {
            r.display = Some(display);
            r.start_owed = true;
            r.start_key_in = false;
        }
        let key = r.want_key.is_some_and(|since| frame.capture_us >= since) || (r.start_owed && !r.start_key_in);
        let opts = if key { EncodeOptions::keyframe() } else { EncodeOptions::default() };
        let capture_us = frame.capture_us;
        let res = r.enc.as_mut().expect("opened above").submit(&frame, opts);
        match res {
            Ok(()) => {
                r.inflight.push_back((capture_us, display));
                r.kept_at_us = Some(now_us);
                if key {
                    r.want_key = None;
                    if r.start_owed {
                        r.start_key_in = true;
                    }
                }
                self.stats.submitted += 1;
            }
            Err(VideoError::Busy) => {
                // Keep it unless a newer frame comes first.
                r.pending = Some((frame, display));
                r.busy_until_us = Some(now_us + BUSY_BACKOFF_US);
                self.stats.busy += 1;
            }
            Err(e) => self.fail(i, codes::ENCODER_FAILED, format!("submit: {e}"), false, now_us, out),
        }
    }

    fn drain(&mut self, i: usize, now_us: u64, out: &dyn Out) {
        loop {
            let r = &mut self.runs[i];
            if self.failed.contains(&r.spec.id) {
                return;
            }
            let Some(res) = r.enc.as_mut().and_then(|e| e.poll()) else { return };
            // An output frees room: a busy encoder can take the next frame.
            r.busy_until_us = None;
            let ef = match res {
                Ok(ef) => ef,
                Err(e) => return self.fail(i, codes::ENCODER_FAILED, format!("the encoder failed: {e}"), false, now_us, out),
            };
            // Outputs come in submit order; frames rate control skipped are
            // the ones in front of this one.
            let mut display = None;
            while let Some((cap, d)) = r.inflight.pop_front() {
                if cap == ef.capture_us {
                    display = Some(d);
                    break;
                }
            }
            let stale = display.is_none() || display != r.display || (r.start_owed && !ef.keyframe);
            if stale {
                self.stats.dropped += 1;
                continue;
            }
            let source_start = r.start_owed;
            if source_start {
                r.start_owed = false;
                r.start_key_in = false;
            }
            r.failures = 0;
            let h = UnitHeader {
                codec: r.spec.codec,
                key: ef.keyframe,
                source_start,
                discardable: false,
                layer: r.spec.id,
                source: display.expect("checked"),
                width: ef.width,
                height: ef.height,
                seq: r.seq,
                captured_ns: ef.capture_us.saturating_mul(1000),
                emitted_ns: now_us.saturating_mul(1000).max(ef.capture_us.saturating_mul(1000)),
            };
            r.seq += 1;
            if out.unit(&h, &ef.data) {
                self.stats.units += 1;
                if h.key {
                    self.stats.keys += 1;
                }
            }
        }
    }

    fn next_wait(&self, now_us: u64) -> Duration {
        let mut wait = IDLE_STEP.as_micros() as u64;
        for r in &self.runs {
            if r.pending.is_some() {
                let at = r.due_at().max(r.busy_until_us.unwrap_or(0));
                wait = wait.min(at.saturating_sub(now_us));
            }
            if r.want_key.is_some() && r.pending.is_none() {
                wait = wait.min((r.owed_at_us + REPEAT_AFTER_US).saturating_sub(now_us));
            }
            if !r.inflight.is_empty() {
                wait = wait.min(INFLIGHT_STEP.as_micros() as u64);
            }
        }
        if let Some(t) = self.retry_at_us {
            wait = wait.min(t.saturating_sub(now_us));
        }
        Duration::from_micros(wait)
    }
}

/// What the capture thread is asked.
enum Cmd {
    Layers(Vec<LayerSpec>),
    Keyframe { layer: u8, since_ns: Option<u64> },
    Rate { layer: u8, fps: u32, bitrate: u32 },
    Display(u32, mpsc::Sender<Result<u32, (&'static str, String)>>),
    Reset,
    Stats(mpsc::Sender<Stats>),
    /// A frame or an encoder output is queued.
    Wake,
    Stop,
}

/// The engine on its own thread (`k2-deskd-capture`). Commands, frames and
/// encoder outputs wake it; it never blocks the link reader.
pub struct Capture {
    tx: mpsc::Sender<Cmd>,
    encoder: LinkEncoder,
    codecs: Vec<Codec>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// Starts the thread for the default (macOS) codecs. `encoder` is what
    /// [`probe_encoder`] found.
    pub fn spawn<O: Opener>(opener: O, backend: Arc<dyn VideoBackend>, out: Arc<dyn Out>, encoder: LinkEncoder) -> std::io::Result<Capture> {
        Capture::spawn_with(opener, backend, out, encoder, H264_ONLY)
    }

    /// Starts the thread for `codecs` (`hello.codecs`).
    pub fn spawn_with<O: Opener>(opener: O, backend: Arc<dyn VideoBackend>, out: Arc<dyn Out>, encoder: LinkEncoder, codecs: &[Codec]) -> std::io::Result<Capture> {
        let list = codecs.to_vec();
        let (tx, rx) = mpsc::channel::<Cmd>();
        let wake_tx = tx.clone();
        let wake: Wake = Arc::new(move || {
            let _ = wake_tx.send(Cmd::Wake);
        });
        let thread = std::thread::Builder::new().name("k2-deskd-capture".into()).spawn(move || {
            realtime_qos();
            let mut engine = Engine::with_codecs(opener, backend, wake, &list);
            let mut wait = IDLE_STEP;
            loop {
                let first = match rx.recv_timeout(wait) {
                    Ok(c) => Some(c),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                };
                for cmd in first.into_iter().chain(std::iter::from_fn(|| rx.try_recv().ok())) {
                    let now = now_us();
                    match cmd {
                        Cmd::Layers(l) => engine.set_layers(&l, now, &*out),
                        Cmd::Keyframe { layer, since_ns } => engine.keyframe(layer, since_ns, now),
                        Cmd::Rate { layer, fps, bitrate } => engine.rate(layer, fps, bitrate, now, &*out),
                        Cmd::Display(d, reply) => {
                            let _ = reply.send(engine.select_display(d, now, &*out));
                        }
                        Cmd::Reset => engine.reset(),
                        Cmd::Stats(reply) => {
                            let _ = reply.send(engine.stats());
                        }
                        Cmd::Wake => {}
                        Cmd::Stop => return,
                    }
                }
                wait = engine.step(now_us(), &*out);
            }
        })?;
        Ok(Capture { tx, encoder, codecs: codecs.to_vec(), thread: Some(thread) })
    }

    /// `hello.codecs`.
    pub fn codecs(&self) -> Vec<Codec> {
        self.codecs.clone()
    }

    /// `hello.encoder`.
    pub fn encoder(&self) -> LinkEncoder {
        self.encoder
    }

    pub fn layers(&self, layers: Vec<LayerSpec>) {
        let _ = self.tx.send(Cmd::Layers(layers));
    }

    pub fn keyframe(&self, layer: u8, since_ns: Option<u64>) {
        let _ = self.tx.send(Cmd::Keyframe { layer, since_ns });
    }

    pub fn rate(&self, layer: u8, fps: u32, bitrate: u32) {
        let _ = self.tx.send(Cmd::Rate { layer, fps, bitrate });
    }

    /// Switches the capture to `display`; waits for the answer (a capture
    /// reopen takes up to ScreenCaptureKit's start).
    pub fn display(&self, display: u32) -> Result<u32, (&'static str, String)> {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Cmd::Display(display, tx)).is_err() {
            return Err((codes::CAPTURE_FAILED, "the capture thread stopped".into()));
        }
        rx.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|_| Err((codes::CAPTURE_FAILED, "the capture didn't answer in 10 s".into())))
    }

    /// Everything off (the link closed).
    pub fn reset(&self) {
        let _ = self.tx.send(Cmd::Reset);
    }

    pub fn stats(&self) -> Option<Stats> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Cmd::Stats(tx)).ok()?;
        rx.recv_timeout(Duration::from_secs(5)).ok()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
#[path = "capture_tests.rs"]
pub(crate) mod tests;
