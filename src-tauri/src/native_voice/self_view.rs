// The pure parts (placement, the frame gate) are used only by a platform
// module with a native window: macOS today.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]
//! The native self-view (0.46.17, behind `videoRoomsPreview`): your own camera
//! in a small window of its own beside the call, drawn by the OS from the
//! camera's own buffers instead of the webview's path (the `local:camera`
//! webview sink, a VP8 encode, the bridge and a WebCodecs decode).
//!
//! Sew's native sink on `local:camera` (plugin 0.9.3,
//! [`VoiceControl::open_native_sink`]) hands over each camera frame as
//! captured: on macOS the camera's own `CVPixelBuffer`, upright and never
//! mirrored, with the camera's facing. It never opens the camera itself.
//!
//! OS integration only: the window and its layer are this OS's
//! ([`platform`]: an `NSPanel` with an `AVSampleBufferDisplayLayer` on macOS,
//! nothing elsewhere yet). The rule of when it shows ([`SelfPlan`]) and what
//! each frame does ([`FrameGate`], [`desktop_mirror`]) are here, pure, so a
//! later Windows or Linux module slots in under [`platform`] without touching
//! them.
//!
//! When it shows: a K2 window that shows the call says it wants the self-view
//! while your camera is on or paused there (`voice_self_view {want}`); the
//! sink is open while some window wants it, and the window is up while the
//! sink's feed is live. It hides when the camera stops or pauses, the call
//! ends, or you close it (closed stays closed until the camera next goes
//! off). Each change goes to every webview (`self-view:state`): the page's own
//! self-view tile hides while `showing`, and stays as the fallback when the
//! native path fails, with the reason in the log.
//!
//! Opening and closing run on a thread of their own ([`self_loop`]): the
//! command and the sink's consumer only queue.
//!
//! The zero-hop preview (cargo feature `native-self-view-preview`, macOS;
//! needs tauri-plugin-sew-voice 0.9.6): on the same sink Sew also offers
//! `PreviewReady { preview, facing, generation }`, a handle on the camera's
//! own `AVCaptureSession` (kessel-video's `ApplePreview`), and
//! `PreviewGone { generation }`. The window then shows an
//! `AVCaptureVideoPreviewLayer` on that session, drawn by the system straight
//! from the camera, instead of the frames through this code; one generation
//! per camera source, so a flip or a reopen swaps the layer in one
//! main-thread step ([`PreviewSlot`], [`preview_swap`]). Until a layer is up
//! (an older Sew, no offer, or a layer that couldn't be made) the frames draw
//! the picture exactly as before ([`path_of`]); the log says which path.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_sew_voice::native::{
    CameraFeed, NativePayload, NativeSink, NativeSinkStats, NativeSinkTarget, NativeVideoConsumer, NativeVideoEvent,
};
use tauri_plugin_sew_voice::sew_call_client::kessel_video::Facing;
use tauri_plugin_sew_voice::VoiceControl;

#[cfg(target_os = "macos")]
#[path = "self_view_mac.rs"]
mod platform;

/// Other OSes have no native self-view yet: the page keeps its own.
#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{CssRect, FrameOutcome};
    use tauri_plugin_sew_voice::native::NativePayload;

    pub const SUPPORTED: bool = false;

    pub struct Display;

    impl Display {
        pub fn new<R: tauri::Runtime>(_app: tauri::AppHandle<R>) -> Display {
            Display
        }
        pub fn frame(&self, _payload: &NativePayload, _mirror: bool, _discontinuity: bool, _timestamp_us: u64) -> FrameOutcome {
            FrameOutcome::NoBuffer
        }
        pub fn flush(&self, _remove_image: bool) {}
        pub fn preview_clear(&self) {}
        pub fn show<R: tauri::Runtime>(&self, _window: &tauri::Window<R>, _anchor: Option<CssRect>) -> Result<(), String> {
            Err("unsupported".into())
        }
        pub fn hide(&self) {}
    }
}

/// Sew's self-view track (`tauri_plugin_sew_voice::video::LOCAL_CAMERA`).
pub const LOCAL_CAMERA: &str = "local:camera";

/// The event every webview hears on each change ([`SelfViewState`]).
pub const STATE_EVENT: &str = "self-view:state";

/// How often an open sink notes its counts in the log.
pub const STATS_EVERY: Duration = Duration::from_secs(10);

/// The longest the self-view's thread waits for the main thread to show the
/// window before it gives up (and the page keeps its tile).
pub const SHOW_TIMEOUT: Duration = Duration::from_secs(3);

/// The window's size in points (16:9).
pub const PANEL_W: f64 = 224.0;
pub const PANEL_H: f64 = 126.0;
/// Room between the window and the edges it sits by.
pub const PANEL_MARGIN: f64 = 12.0;
/// With no call view to sit by: below the top bar.
pub const PANEL_TOP_INSET: f64 = 48.0;

/// A rect in a window's content, in CSS pixels from its top left (the page's
/// `getBoundingClientRect`).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct CssRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A rect on screen in points, from the bottom left (AppKit's).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Where the window goes: at the top right of the call view (`anchor`, in the
/// parent's content), inset by [`PANEL_MARGIN`], or at the content's top right
/// below the top bar with no anchor. Always inside the content when it fits
/// (the floating call tile's default corner, bottom right, stays clear).
pub fn panel_frame(content: ScreenRect, anchor: Option<CssRect>) -> ScreenRect {
    let (w, h) = (PANEL_W, PANEL_H);
    // Top right in CSS terms (from the content's top left).
    let (right, top) = match anchor.filter(|a| a.width > 0.0 && a.height > 0.0) {
        Some(a) => (a.x + a.width - PANEL_MARGIN, a.y + PANEL_MARGIN),
        None => (content.w - PANEL_MARGIN, PANEL_TOP_INSET + PANEL_MARGIN),
    };
    let left = (right - w).clamp(0.0, (content.w - w).max(0.0));
    let top = top.clamp(0.0, (content.h - h).max(0.0));
    ScreenRect { x: content.x + left, y: content.y + content.h - top - h, w, h }
}

/// Whether the self-view shows the frame mirrored. Desktop convention (as the
/// page's own self-view, which always mirrors): every camera facing you is a
/// mirror, the built-in one, a plugged-in one and one that doesn't say; only
/// a camera facing away (a phone's back camera through Continuity Camera)
/// shows as the world sees it.
pub fn desktop_mirror(facing: Facing) -> bool {
    !matches!(facing, Facing::Back)
}

/// µs → the 90 kHz clock the sample buffers carry.
pub fn pts_90k(us: u64) -> i64 {
    i64::try_from(us.saturating_mul(9) / 100).unwrap_or(i64::MAX)
}

/// The camera's feed as the plan reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Feed {
    #[default]
    Off,
    Live,
    Paused,
}

impl Feed {
    pub fn of(feed: &CameraFeed) -> Feed {
        match feed {
            CameraFeed::Live => Feed::Live,
            CameraFeed::Paused => Feed::Paused,
            _ => Feed::Off,
        }
    }
}

/// What [`self_loop`] hears, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum ToSelf {
    /// A K2 window (`window`, its label) shows the call with your camera on
    /// or paused (`want`), or no longer; `anchor` is the call view's rect in
    /// that window, when it has one.
    Want { window: String, want: bool, anchor: Option<CssRect> },
    /// The sink's camera feed changed.
    Feed(Feed),
    /// A camera frame came without an OS buffer: nothing to draw natively.
    NoBuffer,
    /// The person closed the self-view's window.
    Dismissed,
}

/// What every webview hears ([`STATE_EVENT`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct SelfViewState {
    /// This build draws the self-view natively on this OS.
    pub available: bool,
    /// The native self-view is up: the page hides its own self-view tile.
    pub showing: bool,
    /// Why it isn't up while a window wants it: `closed` (the person closed
    /// it), `open_refused:<code>` (Sew refused the sink), `no_pixel_buffer`
    /// (the camera's frames had no OS buffer), `window:<why>` (the window
    /// couldn't be shown). Null otherwise.
    pub reason: Option<String>,
}

/// What [`self_loop`] does, so tests can stand in.
pub trait SelfOps {
    /// Open Sew's `local:camera` sink. `Err`: Sew's refusal code.
    fn open_sink(&mut self) -> Result<(), String>;
    /// Drop the sink.
    fn close_sink(&mut self);
    /// Show the window on `window`'s, by `anchor`.
    fn show(&mut self, window: &str, anchor: Option<CssRect>) -> Result<(), String>;
    /// Hide the window.
    fn hide(&mut self);
    /// Tell every webview.
    fn tell(&mut self, state: &SelfViewState);
    /// Every [`STATS_EVERY`] while the sink is open: the log's counts.
    fn tick(&mut self);
}

/// Whether the sink is open and the window is up, and whether they should be.
#[derive(Debug, Default)]
pub struct SelfPlan {
    /// Windows that want it, oldest first: the last is the window it sits on.
    wants: Vec<(String, Option<CssRect>)>,
    feed: Feed,
    sink_open: bool,
    shown_on: Option<String>,
    /// The person closed it: until no window wants it any more.
    dismissed: bool,
    /// The native path failed: until no window wants it any more.
    failed: Option<String>,
    told: Option<SelfViewState>,
}

impl SelfPlan {
    pub fn state(&self) -> SelfViewState {
        let reason = if self.wants.is_empty() {
            None
        } else if self.dismissed {
            Some("closed".to_string())
        } else {
            self.failed.clone()
        };
        SelfViewState { available: true, showing: self.shown_on.is_some(), reason }
    }

    pub fn is_open(&self) -> bool {
        self.sink_open
    }

    fn parent(&self) -> Option<&(String, Option<CssRect>)> {
        self.wants.last()
    }

    /// One message, then whatever it takes to match: the sink, the window,
    /// and the webviews told.
    pub fn handle(&mut self, msg: ToSelf, ops: &mut impl SelfOps) {
        let mut moved = false;
        let mut always_tell = false;
        match msg {
            ToSelf::Want { window, want, anchor } => {
                always_tell = true;
                let was_parent = self.parent().map(|(w, _)| w.clone());
                self.wants.retain(|(w, _)| *w != window);
                if want {
                    self.wants.push((window, anchor));
                }
                if self.wants.is_empty() {
                    self.dismissed = false;
                    self.failed = None;
                }
                moved = was_parent != self.parent().map(|(w, _)| w.clone());
            }
            ToSelf::Feed(feed) => {
                if !self.sink_open {
                    return;
                }
                self.feed = feed;
            }
            ToSelf::NoBuffer => {
                if !self.sink_open {
                    return;
                }
                if self.failed.is_none() {
                    log_debug!("[native-voice] camera self_view no_pixel_buffer: the page keeps its own");
                }
                self.failed = Some("no_pixel_buffer".into());
            }
            ToSelf::Dismissed => {
                if self.shown_on.is_none() {
                    return;
                }
                log_debug!("[native-voice] camera self_view closed by the person");
                self.dismissed = true;
            }
        }
        // A window that moved to another K2 window goes there.
        if moved && self.shown_on.is_some() {
            ops.hide();
            self.shown_on = None;
        }
        self.settle(ops);
        let state = self.state();
        if always_tell || self.told.as_ref() != Some(&state) {
            ops.tell(&state);
            self.told = Some(state);
        }
    }

    fn settle(&mut self, ops: &mut impl SelfOps) {
        // The sink.
        let want_sink = !self.wants.is_empty() && !self.dismissed && self.failed.is_none();
        if want_sink && !self.sink_open {
            match ops.open_sink() {
                Ok(()) => {
                    self.sink_open = true;
                    self.feed = Feed::Off;
                    log_debug!("[native-voice] camera self_view open");
                }
                Err(code) => {
                    log_debug!("[native-voice] camera self_view refused {code}: the page keeps its own");
                    self.failed = Some(format!("open_refused:{code}"));
                }
            }
        }
        let want_sink = want_sink && self.failed.is_none();
        if !want_sink && self.sink_open {
            if self.shown_on.take().is_some() {
                ops.hide();
            }
            ops.close_sink();
            self.sink_open = false;
            self.feed = Feed::Off;
            log_debug!("[native-voice] camera self_view closed");
        }
        // The window: up while the feed is live.
        let want_shown = if self.sink_open && self.feed == Feed::Live { self.parent().cloned() } else { None };
        match (want_shown, self.shown_on.is_some()) {
            (Some((window, anchor)), false) => match ops.show(&window, anchor) {
                Ok(()) => self.shown_on = Some(window),
                Err(why) => {
                    log_debug!("[native-voice] camera self_view window failed ({why}): the page keeps its own");
                    self.failed = Some(format!("window:{why}"));
                    ops.close_sink();
                    self.sink_open = false;
                    self.feed = Feed::Off;
                }
            },
            (None, true) => {
                ops.hide();
                self.shown_on = None;
            }
            _ => {}
        }
    }
}

/// The self-view's own thread, until every sender is gone (then an open sink
/// is closed and the window hidden).
pub fn self_loop(ops: &mut impl SelfOps, rx: mpsc::Receiver<ToSelf>, every: Duration) {
    let mut plan = SelfPlan::default();
    loop {
        match rx.recv_timeout(every) {
            Ok(msg) => plan.handle(msg, ops),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if plan.is_open() {
                    ops.tick();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if plan.shown_on.take().is_some() {
                    ops.hide();
                }
                if plan.is_open() {
                    ops.close_sink();
                }
                return;
            }
        }
    }
}

// ── One frame ───────────────────────────────────────────────────────────

/// What a display does with one camera frame ([`FrameGate::frame`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Not shown now (the window is hidden, or a mirror change is under way).
    Drop,
    /// The camera turned round: flush the picture, set the view's mirror on
    /// the main thread, then [`FrameGate::mirrored`]. This frame is dropped
    /// so none shows the wrong way round.
    Mirror(bool),
    /// To the layer (`flush` first after a discontinuity).
    Push { flush: bool },
}

/// The display's frame rule, on the sink's thread.
#[derive(Debug, Default)]
pub struct FrameGate {
    showing: bool,
    mirrored: bool,
    mirroring: bool,
}

impl FrameGate {
    pub fn frame(&mut self, mirror: bool, discontinuity: bool) -> Gate {
        if !self.showing || self.mirroring {
            return Gate::Drop;
        }
        if mirror != self.mirrored {
            self.mirrored = mirror;
            self.mirroring = true;
            return Gate::Mirror(mirror);
        }
        Gate::Push { flush: discontinuity }
    }

    /// The view's mirror is set: frames go on.
    pub fn mirrored(&mut self) {
        self.mirroring = false;
    }

    /// The window is up (frames go to the layer) or hidden.
    pub fn set_showing(&mut self, on: bool) {
        self.showing = on;
    }

    #[cfg(test)]
    pub fn is_mirrored(&self) -> bool {
        self.mirrored
    }
}

/// What a display did with a camera frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOutcome {
    Shown,
    Dropped,
    /// No OS buffer in it (CPU planes): this display can't draw it.
    NoBuffer,
    /// The zero-hop preview draws the picture: the frame isn't needed.
    Preview,
}

// ── The zero-hop preview ────────────────────────────────────────────────

/// Which picture the window draws: the camera's frames through this code
/// (the fallback, and the only path before Sew offers a preview), or the
/// system's preview straight from the camera's session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfPath {
    Buffers,
    Preview { generation: u64 },
}

/// The path the frames see: the preview only once its layer is up (until
/// then the frames keep the picture going).
pub fn path_of(installed: Option<u64>) -> SelfPath {
    match installed {
        Some(generation) => SelfPath::Preview { generation },
        None => SelfPath::Buffers,
    }
}

/// The preview Sew offers now (`PreviewReady` / `PreviewGone`), one per
/// camera source (`generation`). Updated on the sink's thread, read on the
/// main thread, which makes the layer ([`preview_swap`]).
#[derive(Debug)]
pub struct PreviewSlot<P> {
    offered: Option<(u64, P)>,
    /// The newest generation heard of: an older one is stale.
    latest: Option<u64>,
}

impl<P> Default for PreviewSlot<P> {
    fn default() -> Self {
        PreviewSlot { offered: None, latest: None }
    }
}

impl<P: Clone> PreviewSlot<P> {
    /// `PreviewReady`: true when the wanted layer changed. A generation no
    /// newer than one already heard of (a repeat, or one that went or
    /// failed) changes nothing.
    pub fn ready(&mut self, generation: u64, preview: P) -> bool {
        if self.latest.is_some_and(|l| generation <= l) {
            return false;
        }
        self.latest = Some(generation);
        self.offered = Some((generation, preview));
        true
    }

    /// `PreviewGone`: true when it was the one offered (its layer comes down).
    pub fn gone(&mut self, generation: u64) -> bool {
        self.latest = Some(self.latest.map_or(generation, |l| l.max(generation)));
        self.take_if(generation)
    }

    /// Its layer couldn't be made: the frames draw the picture until the
    /// next generation. True when it was the one offered.
    pub fn failed(&mut self, generation: u64) -> bool {
        self.take_if(generation)
    }

    /// The sink closed: nothing offered, and the next sink starts afresh.
    pub fn clear(&mut self) {
        *self = PreviewSlot::default();
    }

    /// The layer wanted now, if any.
    pub fn wanted(&self) -> Option<(u64, P)> {
        self.offered.clone()
    }

    pub fn wanted_generation(&self) -> Option<u64> {
        self.offered.as_ref().map(|(g, _)| *g)
    }

    fn take_if(&mut self, generation: u64) -> bool {
        if self.wanted_generation() == Some(generation) {
            self.offered = None;
            true
        } else {
            false
        }
    }
}

/// On the main thread: from the layer that is up to the one wanted. A
/// replace takes the old layer down in the same step as the new one goes up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewSwap {
    Keep,
    Install(u64),
    Remove(u64),
    Replace { old: u64, new: u64 },
}

pub fn preview_swap(installed: Option<u64>, wanted: Option<u64>) -> PreviewSwap {
    match (installed, wanted) {
        (None, None) => PreviewSwap::Keep,
        (Some(a), Some(b)) if a == b => PreviewSwap::Keep,
        (None, Some(new)) => PreviewSwap::Install(new),
        (Some(old), None) => PreviewSwap::Remove(old),
        (Some(old), Some(new)) => PreviewSwap::Replace { old, new },
    }
}

// ── The log's counts ────────────────────────────────────────────────────

/// What the consumer did, for the log.
#[derive(Debug, Default)]
pub struct SelfCounts {
    pub shown: AtomicU64,
    pub dropped: AtomicU64,
    pub no_buffer: AtomicU64,
    pub discontinuities: AtomicU64,
    pub mirrored: AtomicU64,
    /// Frames not needed: the zero-hop preview drew the picture.
    pub previewed: AtomicU64,
    /// The feed now: 0 off, 1 live, 2 paused.
    pub feed: AtomicU8,
}

impl SelfCounts {
    fn feed_name(&self) -> &'static str {
        match self.feed.load(Ordering::Relaxed) {
            1 => "live",
            2 => "paused",
            _ => "off",
        }
    }
}

/// The log's line: Sew's sink counters and ours (counts only).
pub fn stats_line(stats: &NativeSinkStats, counts: &SelfCounts) -> String {
    let n = |a: &AtomicU64| a.load(Ordering::Relaxed);
    format!(
        "camera self_view feed={} frames_in={} frames_out={} fps={:.1} dropped_queue_full={} consumer_us_avg={} shown={} dropped={} no_buffer={} discontinuities={} mirrored={} previewed={}",
        counts.feed_name(),
        stats.frames_in,
        stats.frames_out,
        stats.fps,
        stats.dropped_queue_full,
        stats.consumer_us_avg,
        n(&counts.shown),
        n(&counts.dropped),
        n(&counts.no_buffer),
        n(&counts.discontinuities),
        n(&counts.mirrored),
        n(&counts.previewed),
    )
}

/// A code as the log keeps it: `[a-z0-9_.:-]`, at most 48 characters.
pub fn code(s: &str) -> String {
    s.chars()
        .filter_map(|ch| match ch {
            'a'..='z' | '0'..='9' | '_' | '.' | ':' | '-' => Some(ch),
            'A'..='Z' => Some(ch.to_ascii_lowercase()),
            _ => None,
        })
        .take(48)
        .collect()
}

// ── The real thing ──────────────────────────────────────────────────────

/// The sink's events to the display, on the sink's own thread.
struct SelfViewFeed {
    display: Arc<platform::Display>,
    counts: Arc<SelfCounts>,
    tx: mpsc::Sender<ToSelf>,
}

impl NativeVideoConsumer for SelfViewFeed {
    fn on_video(&self, event: NativeVideoEvent) {
        let add = |n: &AtomicU64| {
            n.fetch_add(1, Ordering::Relaxed);
        };
        match event {
            NativeVideoEvent::SelfView { feed } => {
                let feed = Feed::of(&feed);
                self.counts.feed.store(
                    match feed {
                        Feed::Off => 0,
                        Feed::Live => 1,
                        Feed::Paused => 2,
                    },
                    Ordering::Relaxed,
                );
                if feed != Feed::Live {
                    self.display.flush(true);
                }
                let _ = self.tx.send(ToSelf::Feed(feed));
            }
            NativeVideoEvent::Frame(f) => {
                let NativePayload::Camera(cam) = &f.payload else { return };
                let mirror = desktop_mirror(cam.facing);
                if f.discontinuity {
                    add(&self.counts.discontinuities);
                }
                if mirror {
                    add(&self.counts.mirrored);
                }
                // `f` (and so its buffer) lives until this returns.
                match self.display.frame(&f.payload, mirror, f.discontinuity, f.timestamp_us) {
                    FrameOutcome::Shown => add(&self.counts.shown),
                    FrameOutcome::Dropped => add(&self.counts.dropped),
                    FrameOutcome::Preview => add(&self.counts.previewed),
                    FrameOutcome::NoBuffer => {
                        if self.counts.no_buffer.fetch_add(1, Ordering::Relaxed) == 0 {
                            let _ = self.tx.send(ToSelf::NoBuffer);
                        }
                    }
                }
            }
            NativeVideoEvent::Reset { .. } => self.display.flush(false),
            // The zero-hop preview (Sew 0.11.6, plugin 0.9.6): a layer on the
            // camera's own session, mirrored by K2's desktop rule (not
            // kessel-video's `None`, which mirrors only a front camera).
            #[cfg(all(feature = "native-self-view-preview", target_os = "macos"))]
            NativeVideoEvent::PreviewReady { preview, facing, generation, .. } => {
                self.display.preview_ready(Arc::new(preview), desktop_mirror(facing), generation);
            }
            #[cfg(all(feature = "native-self-view-preview", target_os = "macos"))]
            NativeVideoEvent::PreviewGone { generation, .. } => self.display.preview_gone(generation),
            _ => {}
        }
    }
}

/// The webview windows that asked, by label, for the window to sit on.
type Windows<R> = Arc<Mutex<HashMap<String, tauri::Window<R>>>>;

struct RealOps<R: Runtime> {
    app: AppHandle<R>,
    windows: Windows<R>,
    display: Arc<platform::Display>,
    tx: mpsc::Sender<ToSelf>,
    sink: Option<NativeSink>,
    counts: Arc<SelfCounts>,
}

impl<R: Runtime> SelfOps for RealOps<R> {
    fn open_sink(&mut self) -> Result<(), String> {
        let Some(control) = self.app.try_state::<VoiceControl>() else {
            return Err("no_engine".into());
        };
        self.counts = Arc::new(SelfCounts::default());
        let feed = SelfViewFeed { display: self.display.clone(), counts: self.counts.clone(), tx: self.tx.clone() };
        // `max_size` changes nothing for the self-view: frames are the camera's.
        match control.open_native_sink(NativeSinkTarget::Track(LOCAL_CAMERA.into()), (0, 0), Arc::new(feed)) {
            Ok(sink) => {
                self.sink = Some(sink);
                Ok(())
            }
            Err(e) => Err(code(&e.code)),
        }
    }

    fn close_sink(&mut self) {
        // The sink first: no frame after the flush.
        if let Some(sink) = self.sink.take() {
            log_debug!("[native-voice] {}", stats_line(&sink.stats(), &self.counts));
            sink.close();
        }
        self.display.flush(true);
        // No events after the close: a preview still up comes down.
        self.display.preview_clear();
    }

    fn show(&mut self, window: &str, anchor: Option<CssRect>) -> Result<(), String> {
        let Some(w) = self.windows.lock().unwrap_or_else(|e| e.into_inner()).get(window).cloned() else {
            return Err("no_window".into());
        };
        let (tx, rx) = mpsc::channel();
        let display = self.display.clone();
        let main = w.clone();
        main.run_on_main_thread(move || {
            let _ = tx.send(display.show(&w, anchor));
        })
        .map_err(|_| "main_thread".to_string())?;
        rx.recv_timeout(SHOW_TIMEOUT).map_err(|_| "timeout".to_string())?
    }

    fn hide(&mut self) {
        let display = self.display.clone();
        if self.app.run_on_main_thread(move || display.hide()).is_err() {
            log_debug!("[native-voice] camera self_view hide: no main thread");
        }
    }

    fn tell(&mut self, state: &SelfViewState) {
        let _ = self.app.emit(STATE_EVENT, state);
    }

    fn tick(&mut self) {
        if let Some(sink) = &self.sink {
            log_debug!("[native-voice] {}", stats_line(&sink.stats(), &self.counts));
        }
    }
}

/// The app's self-view: the thread's sender and the windows that asked.
/// Managed as app state on an OS that has one ([`platform::SUPPORTED`]).
pub struct SelfView<R: Runtime> {
    tx: mpsc::Sender<ToSelf>,
    windows: Windows<R>,
}

/// The person closed the window (the window's own button, on the main thread).
static DISMISS: OnceLock<Mutex<mpsc::Sender<ToSelf>>> = OnceLock::new();

pub(crate) fn dismiss() {
    if let Some(tx) = DISMISS.get() {
        let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(ToSelf::Dismissed);
    }
}

/// Start the thread and manage the handle, where this OS has a native
/// self-view; nothing elsewhere (the command then answers unavailable).
pub fn init<R: Runtime>(app: &AppHandle<R>) {
    if !platform::SUPPORTED {
        return;
    }
    let (tx, rx) = mpsc::channel();
    let windows: Windows<R> = Arc::new(Mutex::new(HashMap::new()));
    let _ = DISMISS.set(Mutex::new(tx.clone()));
    let mut ops = RealOps {
        app: app.clone(),
        windows: windows.clone(),
        display: Arc::new(platform::Display::new(app.clone())),
        tx: tx.clone(),
        sink: None,
        counts: Arc::new(SelfCounts::default()),
    };
    let started = std::thread::Builder::new()
        .name("k2-self-view".into())
        .spawn(move || self_loop(&mut ops, rx, STATS_EVERY));
    if started.is_err() {
        log_debug!("[native-voice] camera self_view thread didn't start: the page keeps its own");
        return;
    }
    app.manage(SelfView { tx, windows });
}

/// `voice_self_view`: the calling window wants the self-view or no longer.
/// Answers whether this app draws it natively (false: the page keeps its own).
pub fn set_want<R: Runtime>(app: &AppHandle<R>, window: tauri::Window<R>, want: bool, anchor: Option<CssRect>) -> bool {
    let Some(sv) = app.try_state::<SelfView<R>>() else { return false };
    let label = window.label().to_string();
    {
        let mut windows = sv.windows.lock().unwrap_or_else(|e| e.into_inner());
        if want {
            windows.insert(label.clone(), window);
        } else {
            windows.remove(&label);
        }
    }
    sv.tx.send(ToSelf::Want { window: label, want, anchor }).is_ok()
}

#[cfg(test)]
#[path = "self_view_tests.rs"]
mod tests;
