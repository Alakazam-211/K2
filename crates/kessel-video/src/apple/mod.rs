//! The Apple backend, for macOS and iOS.
//!
//! Shared by both: AVFoundation cameras ([`camera`]), CoreVideo pixel
//! buffers ([`pixel`]), VideoToolbox H.264 encoding ([`vt_enc`]) and
//! decoding ([`vt_dec`]), the FFI and Objective-C helpers, and thermal and
//! low-power events. macOS only: ScreenCaptureKit screens and windows
//! ([`screen`]). iOS only: the app's foreground and background state, the
//! phone's orientation and the capture session's interruptions ([`ios`]);
//! screen sharing on iOS needs a broadcast upload extension, so `open_screen`
//! answers `Unsupported` there.

mod camera;
mod ffi;
#[cfg(target_os = "ios")]
mod ios;
mod objc;
mod pixel;
#[cfg(target_os = "macos")]
mod screen;
mod vt_dec;
mod vt_enc;

use crate::api::{
    Done, EventHub, EventRx, FrameSource, VideoBackend, VideoDecoder, VideoEncoder, Wake,
};
use crate::error::{unsupported, VideoError};
use crate::frame::{PixelFormat, VideoFrame};
use crate::kit::{lock, now_us, ThreadedDecoder, ThreadedEncoder};
use crate::types::*;
use objc2::rc::autoreleasepool;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send, sel};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

pub(crate) use objc::os_name;

/// iOS has no screen capture here: sharing the screen needs a broadcast
/// upload extension, which runs in its own process.
#[cfg(target_os = "ios")]
mod screen {
    use crate::api::{Done, FrameSource, Wake};
    use crate::error::{unsupported, VideoError};
    use crate::types::*;

    const WHY: &str = "screen capture on iOS needs a broadcast upload extension; \
                       kessel-video doesn't capture screens on iOS";

    pub(crate) fn supported() -> bool {
        false
    }

    pub(crate) fn picker_supported() -> bool {
        false
    }

    pub(crate) fn permission() -> Permission {
        Permission::Denied
    }

    pub(crate) fn request_permission(done: Done<Permission>) {
        done(Permission::Denied)
    }

    pub(crate) fn sources() -> Result<Vec<ScreenSourceInfo>, VideoError> {
        Ok(Vec::new())
    }

    pub(crate) fn pick(_opts: PickOptions, done: Done<Result<ScreenSelection, VideoError>>) {
        done(Err(unsupported(WHY)))
    }

    pub(crate) fn open(
        _source: &ScreenSelection,
        _c: CaptureConstraints,
        _wake: Wake,
    ) -> Result<Box<dyn FrameSource>, VideoError> {
        Err(unsupported(WHY))
    }
}

/// The Apple backend.
pub(crate) fn backend() -> Result<Arc<dyn VideoBackend>, VideoError> {
    let inner = Arc::new(Inner {
        events: EventHub::default(),
        shut: AtomicBool::new(false),
    });
    let weak = Arc::downgrade(&inner);
    let monitor = std::thread::Builder::new()
        .name("kessel-video-monitor".into())
        .spawn(move || monitor(weak))
        .map_err(|e| VideoError::os(e.raw_os_error().unwrap_or(0) as i64, format!("spawn: {e}")))?;
    // iOS: the app going to the background and back, as backend events.
    #[cfg(target_os = "ios")]
    let app_state = {
        let weak = Arc::downgrade(&inner);
        let listener: Arc<ios::Listener> = Arc::new(move |background: bool| {
            if let Some(i) = weak.upgrade() {
                if !i.shut.load(Ordering::Acquire) {
                    i.events.publish(BackendEvent::Background(background));
                }
            }
        });
        ios::follow_app_state(&listener);
        listener
    };
    Ok(Arc::new(AppleBackend {
        inner,
        monitor: Mutex::new(Some(monitor)),
        #[cfg(target_os = "ios")]
        _app_state: app_state,
    }))
}

struct Inner {
    events: EventHub,
    shut: AtomicBool,
}

struct AppleBackend {
    inner: Arc<Inner>,
    monitor: Mutex<Option<JoinHandle<()>>>,
    /// Kept alive while the backend lives (the app-state list holds it
    /// weakly).
    #[cfg(target_os = "ios")]
    _app_state: Arc<ios::Listener>,
}

impl Drop for AppleBackend {
    fn drop(&mut self) {
        self.inner.shut.store(true, Ordering::Release);
        if let Some(t) = lock(&self.monitor).take() {
            let _ = t.join();
        }
    }
}

fn thermal_now() -> ThermalLevel {
    autoreleasepool(|_| {
        // SAFETY: NSProcessInfo methods with documented signatures.
        let raw: isize = unsafe {
            let info: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
            msg_send![info, thermalState]
        };
        match raw {
            0 => ThermalLevel::Nominal,
            1 => ThermalLevel::Fair,
            2 => ThermalLevel::Serious,
            _ => ThermalLevel::Critical,
        }
    })
}

fn low_power_now() -> bool {
    autoreleasepool(|_| {
        // SAFETY: NSProcessInfo methods; isLowPowerModeEnabled is checked
        // for first (macOS 12+, iOS 9+).
        unsafe {
            let info: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
            if !objc::responds(info, sel!(isLowPowerModeEnabled)) {
                return false;
            }
            let b: Bool = msg_send![info, isLowPowerModeEnabled];
            b.as_bool()
        }
    })
}

/// Polls the camera list, thermal state and low-power mode once a second
/// and publishes changes. Only checks while someone is subscribed.
fn monitor(inner: Weak<Inner>) {
    let mut cams: Option<Vec<CameraInfo>> = None;
    let mut thermal: Option<ThermalLevel> = None;
    let mut low: Option<bool> = None;
    loop {
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(10));
            match inner.upgrade() {
                Some(i) if !i.shut.load(Ordering::Acquire) => {}
                _ => return,
            }
        }
        let Some(i) = inner.upgrade() else { return };
        if i.events.subscribers() == 0 {
            continue;
        }
        let now = camera::cameras();
        if cams.as_ref().is_some_and(|c| *c != now) {
            i.events.publish(BackendEvent::Cameras(now.clone()));
        }
        cams = Some(now);
        let t = thermal_now();
        if thermal.is_some_and(|x| x != t) {
            i.events.publish(BackendEvent::Thermal(t));
        }
        thermal = Some(t);
        let l = low_power_now();
        if low.is_some_and(|x| x != l) {
            i.events.publish(BackendEvent::LowPower(l));
        }
        low = Some(l);
    }
}

/// Whether there is a hardware H.264 encoder (probed once on macOS; every
/// iPhone and iPad has one, and probing from the background would fail).
fn hardware_h264() -> bool {
    if cfg!(target_os = "ios") {
        return true;
    }
    static HW: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HW.get_or_init(|| {
        let mut cfg = EncoderConfig::h264(1280, 720, 30, 1_000_000);
        cfg.implementation = ImplPreference::HardwareOnly;
        vt_enc::VtEncoder::open(&cfg).is_ok()
    })
}

#[cfg(target_os = "ios")]
use ios::in_background;

/// Is the app in the background? Never on macOS.
#[cfg(not(target_os = "ios"))]
fn in_background() -> bool {
    false
}

impl AppleBackend {
    fn live(&self) -> Result<(), VideoError> {
        if self.inner.shut.load(Ordering::Acquire) {
            Err(VideoError::ShutDown)
        } else {
            Ok(())
        }
    }
}

/// The largest H.264 size listed: what Apple silicon Macs and recent
/// iPhones encode and decode in hardware.
const H264_MAX: (u32, u32) = if cfg!(target_os = "ios") {
    (3840, 2160)
} else {
    (4096, 2304)
};

impl VideoBackend for AppleBackend {
    fn capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::new("apple", os_name());
        crate::rust_codecs::add_capabilities(&mut caps);
        caps.camera = true;
        caps.screen = ScreenCapability {
            supported: screen::supported(),
            system_picker: screen::picker_supported(),
            list: screen::supported(),
            windows: screen::supported(),
            cursor: screen::supported(),
            exclude_own_windows: screen::supported(),
        };
        let h264 = || {
            Some(H264Capability::new(
                vec![
                    H264Profile::ConstrainedBaseline,
                    H264Profile::ConstrainedHigh,
                ],
                H264Level(52),
            ))
        };
        let formats = vec![
            PixelFormat::Nv12,
            PixelFormat::I420,
            PixelFormat::Bgra,
            PixelFormat::Rgba,
            PixelFormat::Native,
        ];
        if hardware_h264() {
            let mut c = CodecCapability::new(
                VideoCodec::H264,
                CodecDirection::Encode,
                Implementation::Hardware,
                "VideoToolbox",
                H264_MAX.0,
                H264_MAX.1,
                60,
            );
            c.h264 = h264();
            c.formats = formats.clone();
            caps.codecs.push(c);
        }
        // macOS also has Apple's software encoder; iOS doesn't offer one.
        if cfg!(target_os = "macos") {
            let mut c = CodecCapability::new(
                VideoCodec::H264,
                CodecDirection::Encode,
                Implementation::Software,
                "VideoToolbox (software)",
                1920,
                1080,
                30,
            );
            c.h264 = h264();
            c.formats = formats.clone();
            caps.codecs.push(c);
        }
        let mut c = CodecCapability::new(
            VideoCodec::H264,
            CodecDirection::Decode,
            Implementation::Hardware,
            "VideoToolbox",
            H264_MAX.0,
            H264_MAX.1,
            60,
        );
        c.h264 = h264();
        c.formats = formats;
        caps.codecs.push(c);
        caps
    }

    fn now_us(&self) -> u64 {
        now_us()
    }

    fn cameras(&self) -> Vec<CameraInfo> {
        camera::cameras()
    }

    fn camera_permission(&self) -> Permission {
        camera::permission()
    }

    fn request_camera_permission(&self, done: Done<Permission>) {
        camera::request_permission(done)
    }

    fn open_camera(
        &self,
        id: Option<&str>,
        c: CaptureConstraints,
        wake: Wake,
    ) -> Result<Box<dyn FrameSource>, VideoError> {
        self.live()?;
        camera::open(id, c, wake)
    }

    fn screen_permission(&self) -> Permission {
        screen::permission()
    }

    fn request_screen_permission(&self, done: Done<Permission>) {
        screen::request_permission(done)
    }

    fn screen_sources(&self) -> Result<Vec<ScreenSourceInfo>, VideoError> {
        self.live()?;
        screen::sources()
    }

    fn pick_screen(&self, opts: PickOptions, done: Done<Result<ScreenSelection, VideoError>>) {
        if self.live().is_err() {
            return done(Err(VideoError::ShutDown));
        }
        screen::pick(opts, done)
    }

    fn open_screen(
        &self,
        source: &ScreenSelection,
        c: CaptureConstraints,
        wake: Wake,
    ) -> Result<Box<dyn FrameSource>, VideoError> {
        self.live()?;
        screen::open(source, c, wake)
    }

    fn encoder(
        &self,
        config: EncoderConfig,
        wake: Wake,
    ) -> Result<Box<dyn VideoEncoder>, VideoError> {
        self.live()?;
        if let Some(r) = crate::rust_codecs::encoder(&config, &wake) {
            return r;
        }
        config.validate()?;
        match config.codec {
            CodecConfig::H264 { .. } => {
                // iOS gives VideoToolbox's encoder to apps in front only.
                if in_background() {
                    return Err(VideoError::ShutDown);
                }
                let core = vt_enc::VtEncoder::open(&config)?;
                Ok(Box::new(ThreadedEncoder::start(
                    Box::new(core),
                    config,
                    wake,
                )?))
            }
            _ => Err(unsupported(format!(
                "no {:?} encoder here yet",
                config.codec.codec()
            ))),
        }
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
        match config.codec {
            VideoCodec::H264 => {
                let core = vt_dec::VtDecoder::open(&config)?;
                Ok(Box::new(ThreadedDecoder::start(
                    Box::new(core),
                    config,
                    wake,
                )?))
            }
            _ => Err(unsupported(format!(
                "no {:?} decoder here yet",
                config.codec
            ))),
        }
    }

    fn convert(
        &self,
        frame: &VideoFrame,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<VideoFrame, VideoError> {
        self.live()?;
        crate::convert::convert(frame, width, height, format)
    }

    fn subscribe(&self) -> EventRx {
        self.inner.events.subscribe()
    }

    fn shutdown(&self) {
        self.inner.shut.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
