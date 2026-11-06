//! The Android backend: Camera2 cameras through the NDK (`libcamera2ndk`,
//! with `AImageReader` from `libmediandk`), VP8 from the Rust crates, and
//! H.264 encoding through `AMediaCodec`. Screen capture runs once the app
//! hands over a MediaProjection ([`AndroidContext::set_media_projection`]):
//! a `VirtualDisplay` into an `AImageReader`, through JNI. The projection's
//! consent, its foreground service and its stop callback are the app's
//! (Java or Kotlin).
//!
//! The backend logic here runs over a [`Platform`]: the NDK glue on a
//! device (`ndk`), a scripted one in the tests, so everything but the FFI
//! is tested on any host.

// On a host, only the tests use the backend.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

mod codec;
mod image;
mod meta;
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
mod ndk;
mod screen;
#[cfg(test)]
mod tests;

use crate::api::{
    Done, EventHub, EventRx, FrameSource, VideoBackend, VideoDecoder, VideoEncoder, Wake,
};
use crate::convert::{convert, fit_within};
use crate::error::{unsupported, VideoError};
use crate::frame::{PixelFormat, Planes, VideoFrame};
use crate::kit::{
    lock, now_us, EncoderCore, QueuedSource, SourceControl, SourceShared, ThreadedEncoder,
};
use crate::platform::{
    AndroidContext, BackgroundListener, HostState, ScreenChange, ScreenListener,
};
use crate::types::*;
use image::{oriented_box, to_i420, upright_rotation, Yuv420};
use meta::{CamDesc, Lost};
use screen::ScreenImage;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// What screen capture answers on Android before the app hands over a
/// projection.
const NO_SCREEN: &str = "screen capture on Android needs a MediaProjection from the app: \
                         AndroidContext::set_media_projection";

/// The one screen source while a projection is there.
pub(crate) const PROJECTION_ID: &str = "media-projection";

/// A frame around an AHardwareBuffer the host already has (from its own
/// capture or rendering), for a [`crate::frame_in()`] source or an encoder.
/// The buffer is acquired (the caller keeps its own reference) and released
/// when the frame's last clone drops. Encoders read it back to CPU planes
/// (`AHardwareBuffer_lock`): RGBA and RGBX buffers on API 26+, YUV 4:2:0
/// buffers on API 29+. Other formats, or an older device, are
/// `Unsupported`.
///
/// # Safety
///
/// `buffer` must be a live `AHardwareBuffer*`.
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
pub unsafe fn hardware_buffer_frame(
    buffer: *mut std::ffi::c_void,
    capture_us: u64,
) -> Result<VideoFrame, VideoError> {
    // SAFETY: the caller's promise.
    unsafe { ndk::hardware_buffer_frame(buffer, capture_us) }
}

/// The Android backend for the host app's `ctx`.
#[cfg(target_os = "android")]
pub(crate) fn backend(ctx: AndroidContext) -> Result<Arc<dyn VideoBackend>, VideoError> {
    let platform = ndk::Ndk::new(ctx.jni())?;
    Ok(AndroidBackend::start(Arc::new(platform), ctx))
}

/// The Android release and API level, for logs.
#[cfg(target_os = "android")]
pub(crate) fn os_name() -> String {
    ndk::os_name()
}

/// The OS side of the backend: the NDK on a device, a script in tests.
pub(crate) trait Platform: Send + Sync + 'static {
    /// For logs.
    fn os_name(&self) -> String;
    /// The cameras with a YUV output, default marked.
    fn cameras(&self) -> Result<Vec<CamDesc>, VideoError>;
    /// The CAMERA permission. Never prompts.
    fn camera_permission(&self) -> Permission;
    /// Start `cam` in `mode`, at `fps_range` when given, handing images to
    /// `sink`. Dropping the result closes the camera.
    fn open_camera(
        &self,
        cam: &CamDesc,
        mode: CaptureMode,
        fps_range: Option<(u32, u32)>,
        sink: Arc<dyn ImageSink>,
    ) -> Result<Box<dyn Capture>, VideoError>;
    /// The H.264 encoder entry, if the device has one.
    fn h264_encode_capability(&self) -> Option<CodecCapability>;
    /// An H.264 encoder core for `cfg` (already validated).
    fn h264_encoder(&self, cfg: &EncoderConfig) -> Result<Box<dyn EncoderCore>, VideoError>;
    /// A virtual display of `width` x `height` at `dpi` from the host's
    /// MediaProjection (a JNI global reference, as an address), mirroring
    /// the screen into a reader whose images go to `sink`. Dropping the
    /// result releases the display.
    fn open_projection(
        &self,
        projection: usize,
        size: (u32, u32, u32),
        sink: Arc<dyn ScreenSink>,
    ) -> Result<Box<dyn ProjectionCapture>, VideoError>;
}

/// A running camera. Dropping it closes the device.
pub(crate) trait Capture: Send {}

/// A running virtual display. Dropping it releases the display and its
/// reader.
pub(crate) trait ProjectionCapture: Send {
    /// A new size (and density): a new reader of that size, then the
    /// display resized onto it.
    fn resize(&mut self, width: u32, height: u32, dpi: u32) -> Result<(), VideoError>;
}

/// Where a virtual display's images go. Called from the NDK's threads; must
/// not block on the backend.
pub(crate) trait ScreenSink: Send + Sync {
    /// Nothing wants frames now: the image can be released unread.
    fn idle(&self) -> bool;
    /// One image.
    fn image(&self, img: &ScreenImage<'_>);
    /// One image couldn't be read (the display keeps running).
    fn error(&self, e: VideoError);
}

/// Where a running camera's images and losses go. Called from the NDK's
/// threads; must not block on the backend.
pub(crate) trait ImageSink: Send + Sync {
    /// Nothing wants frames now: the image can be released unread.
    fn idle(&self) -> bool;
    /// One image.
    fn image(&self, img: &Yuv420<'_>);
    /// One image couldn't be read (the camera keeps running).
    fn error(&self, e: VideoError);
    /// The device stopped.
    fn lost(&self, why: Lost);
}

// ───────────────────────────── the backend ─────────────────────────────

struct Inner {
    platform: Arc<dyn Platform>,
    host: Arc<HostState>,
    events: EventHub,
    shut: AtomicBool,
    cams: Mutex<Vec<Weak<Cam>>>,
    screens: Mutex<Vec<Weak<Screen>>>,
}

pub(crate) struct AndroidBackend {
    inner: Arc<Inner>,
    // Kept alive while the backend lives; the host holds only weak refs.
    _listener: Arc<BackgroundListener>,
    _screen_listener: Arc<ScreenListener>,
}

impl AndroidBackend {
    pub(crate) fn start(platform: Arc<dyn Platform>, ctx: AndroidContext) -> Arc<dyn VideoBackend> {
        let inner = Arc::new(Inner {
            platform,
            host: ctx.host().clone(),
            events: EventHub::default(),
            shut: AtomicBool::new(false),
            cams: Mutex::new(Vec::new()),
            screens: Mutex::new(Vec::new()),
        });
        let weak = Arc::downgrade(&inner);
        let listener: Arc<BackgroundListener> = Arc::new(move |background| {
            if let Some(i) = weak.upgrade() {
                i.set_background(background);
            }
        });
        ctx.host().listen(&listener);
        let weak = Arc::downgrade(&inner);
        let screen_listener: Arc<ScreenListener> = Arc::new(move |change| {
            if let Some(i) = weak.upgrade() {
                i.screen_changed(change);
            }
        });
        ctx.host().listen_screen(&screen_listener);
        Arc::new(AndroidBackend {
            inner,
            _listener: listener,
            _screen_listener: screen_listener,
        })
    }

    fn live(&self) -> Result<(), VideoError> {
        if self.inner.shut.load(Ordering::Acquire) {
            Err(VideoError::ShutDown)
        } else {
            Ok(())
        }
    }
}

impl Inner {
    fn live_cams(&self) -> Vec<Arc<Cam>> {
        let mut cams = lock(&self.cams);
        cams.retain(|w| w.strong_count() > 0);
        cams.iter().filter_map(Weak::upgrade).collect()
    }

    fn live_screens(&self) -> Vec<Arc<Screen>> {
        let mut screens = lock(&self.screens);
        screens.retain(|w| w.strong_count() > 0);
        screens.iter().filter_map(Weak::upgrade).collect()
    }

    /// Screens follow the display; a stopped projection ends them. They
    /// don't care whether the app is in the background.
    fn screen_changed(&self, change: ScreenChange) {
        if self.shut.load(Ordering::Acquire) {
            return;
        }
        match change {
            ScreenChange::Display => {
                let display = self.host.screen().display;
                for s in self.live_screens() {
                    s.refit(display);
                }
            }
            ScreenChange::Stopped => {
                for s in self.live_screens() {
                    s.close();
                    s.sink
                        .shared
                        .event(SourceEvent::Ended(EndReason::SourceClosed));
                }
                self.events.publish(BackendEvent::CapabilitiesChanged);
            }
            ScreenChange::Given => self.events.publish(BackendEvent::CapabilitiesChanged),
        }
    }

    fn projected(&self) -> bool {
        self.host.screen().projection.is_some()
    }

    fn set_background(&self, background: bool) {
        if self.shut.load(Ordering::Acquire) {
            return;
        }
        self.events.publish(BackendEvent::Background(background));
        let cams = self.live_cams();
        if background {
            // Close now: Android expects the camera released in onPause.
            for c in cams {
                c.suspend(Interruption::Background);
            }
        } else if !cams.is_empty() {
            // Reopening takes a while; keep it off the caller's (UI) thread.
            let spawned = std::thread::Builder::new()
                .name("kessel-video-resume".into())
                .spawn(move || {
                    for c in cams {
                        c.resume();
                    }
                });
            if let Err(e) = spawned {
                let e = VideoError::os(e.raw_os_error().unwrap_or(0) as i64, format!("spawn: {e}"));
                for c in self.live_cams() {
                    c.sink.shared.event(SourceEvent::Error(e.clone()));
                }
            }
        }
    }
}

impl VideoBackend for AndroidBackend {
    fn capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::new("android", self.inner.platform.os_name());
        crate::rust_codecs::add_capabilities(&mut caps);
        caps.codecs
            .extend(self.inner.platform.h264_encode_capability());
        caps.camera = true;
        // One screen, while the app's MediaProjection lasts.
        let projected = self.inner.projected();
        caps.screen = ScreenCapability {
            supported: projected,
            system_picker: false,
            list: projected,
            windows: false,
            cursor: false,
            exclude_own_windows: false,
            frame_in: false,
        };
        caps
    }

    fn now_us(&self) -> u64 {
        now_us()
    }

    fn cameras(&self) -> Vec<CameraInfo> {
        // The trait's list can't carry an error; a failing camera service
        // shows as no cameras, and open says why.
        self.inner
            .platform
            .cameras()
            .map(|c| c.into_iter().map(|d| d.info).collect())
            .unwrap_or_default()
    }

    fn camera_permission(&self) -> Permission {
        self.inner.platform.camera_permission()
    }

    fn request_camera_permission(&self, done: Done<Permission>) {
        // Asking needs an Activity and its result callback, in Java: the
        // app asks (in Tauri, its permission API). This reports the state.
        done(self.inner.platform.camera_permission())
    }

    fn open_camera(
        &self,
        id: Option<&str>,
        c: CaptureConstraints,
        wake: Wake,
    ) -> Result<Box<dyn FrameSource>, VideoError> {
        self.live()?;
        c.validate()?;
        if self.inner.host.background() {
            return Err(VideoError::ShutDown);
        }
        let cams = self.inner.platform.cameras()?;
        let desc = match id {
            Some(id) => cams.into_iter().find(|d| d.info.id == id),
            None => {
                let want = c.facing;
                let pick = want.and_then(|f| cams.iter().position(|d| d.info.facing == f));
                let pick = pick.or_else(|| cams.iter().position(|d| d.info.is_default));
                pick.map(|i| cams[i].clone())
            }
        }
        .ok_or(VideoError::NotFound)?;
        // Modes are sensor-sized (landscape); the box is orientation-free.
        let (bw, bh) = oriented_box(c.max_width, c.max_height, false);
        let mut wanted = c;
        wanted.max_width = bw;
        wanted.max_height = bh;
        let mode = nearest_mode(&desc.info.modes, &wanted)
            .ok_or_else(|| unsupported("the camera lists no YUV mode"))?;
        let fps = c
            .fps
            .map_or(mode.max_fps.min(30), |f| f.min(mode.max_fps).max(1));
        let fps_range = meta::fps_range(&desc.fps_ranges, fps);
        // The shared queue never sees the size box: the sink fits frames
        // to it after turning them upright.
        let mut queued = c;
        queued.max_width = None;
        queued.max_height = None;
        let shared = SourceShared::new(queued, fps, wake);
        let sink = Arc::new(CamSink {
            shared,
            host: self.inner.host.clone(),
            sensor: desc.sensor_orientation,
            facing: desc.info.facing,
            size_box: Mutex::new((c.max_width, c.max_height)),
            device: AtomicU8::new(RUNNING),
        });
        let capture = self
            .inner
            .platform
            .open_camera(&desc, mode, fps_range, sink.clone())?;
        let cam = Arc::new(Cam {
            platform: self.inner.platform.clone(),
            host: self.inner.host.clone(),
            desc,
            mode,
            fps_range,
            sink: sink.clone(),
            slot: Mutex::new(Slot {
                capture: Some(capture),
                interrupted: None,
                closed: false,
            }),
        });
        lock(&self.inner.cams).push(Arc::downgrade(&cam));
        // The app may have gone to the background while this opened.
        if self.inner.host.background() {
            cam.suspend(Interruption::Background);
        }
        Ok(Box::new(AndroidCamera {
            queue: QueuedSource::new(sink.shared.clone(), CamControl { cam }),
            sink,
        }))
    }

    fn screen_permission(&self) -> Permission {
        // The consent is the projection: asking for it
        // (createScreenCaptureIntent) is the app's.
        if self.inner.projected() {
            Permission::Granted
        } else {
            Permission::NotDetermined
        }
    }

    fn request_screen_permission(&self, done: Done<Permission>) {
        done(self.screen_permission())
    }

    fn screen_sources(&self) -> Result<Vec<ScreenSourceInfo>, VideoError> {
        self.live()?;
        let st = *self.inner.host.screen();
        Ok(match st.projection {
            Some(_) => vec![ScreenSourceInfo::new(
                PROJECTION_ID,
                ScreenKind::Display,
                "Screen",
                st.display.0,
                st.display.1,
            )],
            None => Vec::new(),
        })
    }

    fn pick_screen(&self, _opts: PickOptions, done: Done<Result<ScreenSelection, VideoError>>) {
        if self.live().is_err() {
            return done(Err(VideoError::ShutDown));
        }
        // The consent dialog was the picker; the projection is the choice.
        if self.inner.projected() {
            done(Ok(ScreenSelection::Listed(PROJECTION_ID.into())))
        } else {
            done(Err(unsupported(NO_SCREEN)))
        }
    }

    fn open_screen(
        &self,
        s: &ScreenSelection,
        c: CaptureConstraints,
        wake: Wake,
    ) -> Result<Box<dyn FrameSource>, VideoError> {
        self.live()?;
        c.validate()?;
        // Held until the virtual display is made: the host's
        // media_projection_stopped waits for it.
        let st = self.inner.host.screen();
        let Some(projection) = st.projection else {
            return Err(unsupported(NO_SCREEN));
        };
        match s {
            ScreenSelection::Listed(id) if id == PROJECTION_ID => {}
            _ => return Err(VideoError::NotFound),
        }
        let (dw, dh, dpi) = st.display;
        if dw == 0 || dh == 0 || dpi == 0 {
            return Err(VideoError::Invalid(format!(
                "a {dw}x{dh} display at {dpi} dpi (set_media_projection's size)"
            )));
        }
        let (w, h) = screen::virtual_size(dw, dh, c.max_width, c.max_height);
        let shared = SourceShared::new(c, c.fps.unwrap_or(30), wake);
        shared.set_box_follows_frame(true);
        let sink = Arc::new(ScreenFrames { shared });
        let capture = self
            .inner
            .platform
            .open_projection(projection, (w, h, dpi), sink.clone())?;
        drop(st);
        let screen = Arc::new(Screen {
            sink,
            slot: Mutex::new(ScreenSlot {
                capture: Some(capture),
                size: (w, h, dpi),
                size_box: (c.max_width, c.max_height),
            }),
        });
        lock(&self.inner.screens).push(Arc::downgrade(&screen));
        Ok(Box::new(AndroidScreen {
            queue: QueuedSource::new(screen.sink.shared.clone(), ScreenControl),
            screen,
            host: self.inner.host.clone(),
        }))
    }

    fn encoder(
        &self,
        config: EncoderConfig,
        wake: Wake,
    ) -> Result<Box<dyn VideoEncoder>, VideoError> {
        self.live()?;
        if let CodecConfig::H264 { .. } = config.codec {
            config.validate()?;
            let core = self.inner.platform.h264_encoder(&config)?;
            return Ok(Box::new(ThreadedEncoder::start(core, config, wake)?));
        }
        if let Some(r) = crate::rust_codecs::encoder(&config, &wake) {
            return r;
        }
        Err(unsupported(format!(
            "no {:?} encoder on Android",
            config.codec.codec()
        )))
    }

    fn decoder(
        &self,
        config: DecoderConfig,
        wake: Wake,
    ) -> Result<Box<dyn VideoDecoder>, VideoError> {
        self.live()?;
        if let Some(r) = crate::rust_codecs::decoder(&config, &wake) {
            return r;
        }
        Err(unsupported(format!(
            "no {:?} decoder on Android yet",
            config.codec
        )))
    }

    fn convert(
        &self,
        frame: &VideoFrame,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<VideoFrame, VideoError> {
        self.live()?;
        convert(frame, width, height, format)
    }

    fn subscribe(&self) -> EventRx {
        self.inner.events.subscribe()
    }

    fn shutdown(&self) {
        if self.inner.shut.swap(true, Ordering::AcqRel) {
            return;
        }
        for c in self.inner.live_cams() {
            c.close();
            c.sink.shared.event(SourceEvent::Ended(EndReason::Closed));
        }
        for s in self.inner.live_screens() {
            s.close();
            s.sink.shared.event(SourceEvent::Ended(EndReason::Closed));
        }
    }
}

// ───────────────────────────── screens ─────────────────────────────

/// Turns a virtual display's images into NV12 frames and queues them.
struct ScreenFrames {
    shared: Arc<SourceShared>,
}

impl ScreenSink for ScreenFrames {
    fn idle(&self) -> bool {
        self.shared.idle()
    }

    fn image(&self, img: &ScreenImage<'_>) {
        if self.shared.idle() {
            return;
        }
        match screen::to_nv12(img, now_us()) {
            Ok(f) => self.shared.deliver(f),
            Err(e) => self.shared.event(SourceEvent::Error(e)),
        }
    }

    fn error(&self, e: VideoError) {
        self.shared.event(SourceEvent::Error(e));
    }
}

struct ScreenSlot {
    /// The virtual display; `None` once closed.
    capture: Option<Box<dyn ProjectionCapture>>,
    /// Its width, height and density now.
    size: (u32, u32, u32),
    /// The asked size box (orientation-free).
    size_box: (Option<u32>, Option<u32>),
}

/// One shared screen: the virtual display, kept at the display's shape.
struct Screen {
    sink: Arc<ScreenFrames>,
    slot: Mutex<ScreenSlot>,
}

impl Screen {
    /// Resize the virtual display for `display` (width, height, dpi) and
    /// the size box, if its size changes.
    fn refit(&self, display: (u32, u32, u32)) {
        let (dw, dh, dpi) = display;
        if dw == 0 || dh == 0 || dpi == 0 {
            return;
        }
        let mut slot = lock(&self.slot);
        let (bw, bh) = slot.size_box;
        let (w, h) = screen::virtual_size(dw, dh, bw, bh);
        let size = (w, h, dpi);
        if size == slot.size {
            return;
        }
        let Some(capture) = slot.capture.as_mut() else {
            return;
        };
        if let Err(e) = capture.resize(w, h, dpi) {
            drop(slot);
            self.sink.shared.event(SourceEvent::Error(e));
            return;
        }
        slot.size = size;
    }

    fn close(&self) {
        lock(&self.slot).capture = None;
    }
}

struct ScreenControl;

impl SourceControl for ScreenControl {}

/// The screen source: the shared queue, and the size box kept here so the
/// virtual display follows it.
struct AndroidScreen {
    queue: QueuedSource<ScreenControl>,
    screen: Arc<Screen>,
    host: Arc<HostState>,
}

impl FrameSource for AndroidScreen {
    fn try_recv(&mut self) -> Option<VideoFrame> {
        self.queue.try_recv()
    }

    fn try_event(&mut self) -> Option<SourceEvent> {
        self.queue.try_event()
    }

    fn set_constraints(&mut self, c: CaptureConstraints) -> Result<(), VideoError> {
        self.queue.set_constraints(c)?;
        lock(&self.screen.slot).size_box = (c.max_width, c.max_height);
        let display = self.host.screen().display;
        self.screen.refit(display);
        Ok(())
    }

    fn set_paused(&mut self, paused: bool) {
        // The display keeps running; its images are released unread.
        self.queue.set_paused(paused)
    }

    fn stats(&self) -> SourceStats {
        self.queue.stats()
    }
}

impl Drop for AndroidScreen {
    fn drop(&mut self) {
        self.screen.close();
    }
}

// ───────────────────────────── cameras ─────────────────────────────

/// [`CamSink::device`]: the capture runs.
const RUNNING: u8 = 0;
/// The OS stopped the device; reopening may work (resume, or unpause).
const STOPPED: u8 = 1;
/// The OS stopped the device for good; the source has ended.
const ENDED: u8 = 2;

/// Turns camera images upright, fits them to the box and queues them.
struct CamSink {
    shared: Arc<SourceShared>,
    host: Arc<HostState>,
    sensor: u32,
    facing: Facing,
    size_box: Mutex<(Option<u32>, Option<u32>)>,
    /// What the device's callbacks last said: `RUNNING`, `STOPPED` or
    /// `ENDED`.
    device: AtomicU8,
}

impl CamSink {
    fn end(&self, reason: EndReason) {
        self.device.store(ENDED, Ordering::Release);
        self.shared.event(SourceEvent::Ended(reason));
    }
}

impl CamSink {
    fn frame(&self, img: &Yuv420<'_>) -> Result<VideoFrame, VideoError> {
        let rotation = upright_rotation(self.sensor, self.facing, self.host.orientation());
        let (data, w, h) = to_i420(img, rotation)?;
        let frame = VideoFrame::from_planes(
            Planes::tight(PixelFormat::I420, w, h, data)?,
            w,
            h,
            now_us(),
        )?;
        let (bw, bh) = {
            let (mw, mh) = *lock(&self.size_box);
            oriented_box(mw, mh, h > w)
        };
        let (bw, bh) = (bw.unwrap_or(u32::MAX), bh.unwrap_or(u32::MAX));
        if w <= bw && h <= bh {
            return Ok(frame);
        }
        let (fw, fh) = fit_within(w, h, bw, bh);
        convert(&frame, fw, fh, PixelFormat::I420)
    }
}

impl ImageSink for CamSink {
    fn idle(&self) -> bool {
        self.shared.idle()
    }

    fn image(&self, img: &Yuv420<'_>) {
        if self.shared.idle() {
            return;
        }
        match self.frame(img) {
            Ok(f) => self.shared.deliver(f),
            Err(e) => self.shared.event(SourceEvent::Error(e)),
        }
    }

    fn error(&self, e: VideoError) {
        self.shared.event(SourceEvent::Error(e));
    }

    fn lost(&self, why: Lost) {
        if self.host.background() {
            // Going to the background closes it anyway; resume reopens.
            return;
        }
        match why {
            // A built-in camera taken by another app comes back on resume
            // (or when the caller unpauses).
            Lost::Disconnected if self.facing == Facing::External => {
                self.end(EndReason::DeviceLost)
            }
            Lost::Disconnected | Lost::InUse => {
                if self.device.swap(STOPPED, Ordering::AcqRel) == RUNNING {
                    self.shared
                        .event(SourceEvent::Interrupted(Interruption::InUseByAnother));
                }
            }
            Lost::Disabled => self.end(EndReason::Revoked),
            Lost::Failed(code) => {
                self.shared.event(SourceEvent::Error(VideoError::os(
                    code as i64,
                    "the camera device failed",
                )));
                self.end(EndReason::DeviceLost);
            }
        }
    }
}

struct Slot {
    capture: Option<Box<dyn Capture>>,
    /// Closed by an interruption; resume reopens it.
    interrupted: Option<Interruption>,
    /// Closed for good (shutdown).
    closed: bool,
}

/// One open camera: what reopening it needs, and the running capture.
struct Cam {
    platform: Arc<dyn Platform>,
    host: Arc<HostState>,
    desc: CamDesc,
    mode: CaptureMode,
    fps_range: Option<(u32, u32)>,
    sink: Arc<CamSink>,
    slot: Mutex<Slot>,
}

impl Cam {
    /// Close the device until [`Cam::resume`].
    fn suspend(&self, why: Interruption) {
        let mut slot = lock(&self.slot);
        if slot.closed || slot.interrupted.is_some() || self.ended() {
            return;
        }
        // Dropping the capture waits for its callbacks, which never take
        // this lock.
        slot.capture = None;
        slot.interrupted = Some(why);
        drop(slot);
        self.sink.shared.event(SourceEvent::Interrupted(why));
    }

    fn ended(&self) -> bool {
        self.sink.device.load(Ordering::Acquire) == ENDED
    }

    /// Stopped by the OS, or closed by an interruption: a retry may help.
    fn retryable(&self) -> bool {
        let slot = lock(&self.slot);
        !slot.closed
            && !self.ended()
            && (slot.interrupted.is_some() || self.sink.device.load(Ordering::Acquire) == STOPPED)
    }

    /// Reopen after an interruption or a stop by the OS (in the foreground
    /// only).
    fn resume(&self) {
        let mut slot = lock(&self.slot);
        let stopped = self.sink.device.load(Ordering::Acquire) == STOPPED;
        if slot.closed || self.ended() || self.host.background() {
            return;
        }
        if slot.capture.is_some() && !stopped {
            return;
        }
        // A stopped device's handles go first: the camera can't open twice.
        slot.capture = None;
        let sink: Arc<dyn ImageSink> = self.sink.clone();
        self.sink.device.store(RUNNING, Ordering::Release);
        match self
            .platform
            .open_camera(&self.desc, self.mode, self.fps_range, sink)
        {
            Ok(c) => {
                slot.capture = Some(c);
                slot.interrupted = None;
                drop(slot);
                self.sink.shared.event(SourceEvent::Resumed);
            }
            Err(VideoError::DeviceBusy) => {
                let was = slot.interrupted.replace(Interruption::InUseByAnother);
                drop(slot);
                if was != Some(Interruption::InUseByAnother) && !stopped {
                    self.sink
                        .shared
                        .event(SourceEvent::Interrupted(Interruption::InUseByAnother));
                }
            }
            Err(VideoError::NoPermission) => {
                slot.closed = true;
                drop(slot);
                self.sink.end(EndReason::Revoked);
            }
            Err(e) => {
                slot.closed = true;
                drop(slot);
                self.sink.shared.event(SourceEvent::Error(e));
                self.sink.end(EndReason::DeviceLost);
            }
        }
    }

    fn close(&self) {
        let mut slot = lock(&self.slot);
        slot.closed = true;
        slot.capture = None;
    }
}

struct CamControl {
    cam: Arc<Cam>,
}

impl SourceControl for CamControl {
    fn set_paused(&mut self, paused: bool) {
        // Unpausing is the caller's way to retry a camera another app took.
        if !paused && self.cam.retryable() {
            self.cam.resume();
        }
    }
}

impl Drop for CamControl {
    fn drop(&mut self) {
        self.cam.close();
    }
}

/// The camera source: the shared queue, with the size box kept here
/// (orientation-free) instead of in the queue.
struct AndroidCamera {
    queue: QueuedSource<CamControl>,
    sink: Arc<CamSink>,
}

impl FrameSource for AndroidCamera {
    fn try_recv(&mut self) -> Option<VideoFrame> {
        self.queue.try_recv()
    }

    fn try_event(&mut self) -> Option<SourceEvent> {
        self.queue.try_event()
    }

    fn set_constraints(&mut self, c: CaptureConstraints) -> Result<(), VideoError> {
        c.validate()?;
        let mut queued = c;
        queued.max_width = None;
        queued.max_height = None;
        self.queue.set_constraints(queued)?;
        *lock(&self.sink.size_box) = (c.max_width, c.max_height);
        Ok(())
    }

    fn set_paused(&mut self, paused: bool) {
        self.queue.set_paused(paused)
    }

    fn stats(&self) -> SourceStats {
        self.queue.stats()
    }
}
