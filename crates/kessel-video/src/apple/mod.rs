//! The macOS backend: AVFoundation cameras, ScreenCaptureKit screens and
//! windows, VideoToolbox H.264 encoding, and thermal / low-power events.
//! iOS will share the AVFoundation and VideoToolbox parts.

mod camera;
mod ffi;
mod objc;
mod pixel;
mod screen;
mod vt_enc;

use crate::api::{
    Done, EventHub, EventRx, FrameSource, VideoBackend, VideoDecoder, VideoEncoder, Wake,
};
use crate::error::{unsupported, VideoError};
use crate::frame::{PixelFormat, VideoFrame};
use crate::kit::{lock, now_us, ThreadedEncoder};
use crate::types::*;
use objc2::rc::autoreleasepool;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send, sel};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

pub(crate) use objc::os_name;

/// The macOS backend.
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
    Ok(Arc::new(AppleBackend {
        inner,
        monitor: Mutex::new(Some(monitor)),
    }))
}

struct Inner {
    events: EventHub,
    shut: AtomicBool,
}

struct AppleBackend {
    inner: Arc<Inner>,
    monitor: Mutex<Option<JoinHandle<()>>>,
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
        // for first (macOS 12+).
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

/// Whether this Mac has a hardware H.264 encoder (probed once).
fn hardware_h264() -> bool {
    static HW: OnceLock<bool> = OnceLock::new();
    *HW.get_or_init(|| {
        let mut cfg = EncoderConfig::h264(1280, 720, 30, 1_000_000);
        cfg.implementation = ImplPreference::HardwareOnly;
        vt_enc::VtEncoder::open(&cfg).is_ok()
    })
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
                4096,
                2304,
                60,
            );
            c.h264 = h264();
            c.formats = formats.clone();
            caps.codecs.push(c);
        }
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
                let core = vt_enc::VtEncoder::open(&config)?;
                Ok(Box::new(ThreadedEncoder::start(
                    Box::new(core),
                    config,
                    wake,
                )?))
            }
            _ => Err(unsupported(format!(
                "no {:?} encoder on this Mac yet",
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
        Err(unsupported(format!(
            "no {:?} decoder on this Mac yet (the webview's WebCodecs decodes)",
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
