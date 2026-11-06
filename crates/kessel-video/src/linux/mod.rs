//! The Linux backend: V4L2 cameras through raw ioctls. Screen capture (the
//! xdg-desktop-portal ScreenCast with PipeWire) comes next; Linux has no OS
//! H.264, so codecs come from the VP8 crates.

#[cfg(not(target_pointer_width = "64"))]
compile_error!("the V4L2 structs here are laid out for 64-bit targets");

mod camera;
mod v4l2;

use crate::api::{Done, EventHub, EventRx, FrameSource, VideoBackend, VideoDecoder, VideoEncoder, Wake};
use crate::error::{unsupported, VideoError};
use crate::frame::{PixelFormat, VideoFrame};
use crate::kit::{lock, now_us};
use crate::types::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

/// The Linux backend.
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
    Ok(Arc::new(LinuxBackend {
        inner,
        monitor: Mutex::new(Some(monitor)),
    }))
}

/// The OS and version, for logs: the distribution's pretty name and the
/// kernel release.
pub(crate) fn os_name() -> String {
    let pretty = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string()))
        })
        .unwrap_or_else(|| "Linux".into());
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").map(|s| s.trim().to_string()).unwrap_or_default();
    if kernel.is_empty() {
        pretty
    } else {
        format!("{pretty} (kernel {kernel})")
    }
}

struct Inner {
    events: EventHub,
    shut: AtomicBool,
}

struct LinuxBackend {
    inner: Arc<Inner>,
    monitor: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for LinuxBackend {
    fn drop(&mut self) {
        self.inner.shut.store(true, Ordering::Release);
        if let Some(t) = lock(&self.monitor).take() {
            let _ = t.join();
        }
    }
}

/// Polls the camera list once a second while someone is subscribed.
fn monitor(inner: Weak<Inner>) {
    let mut cams: Option<Vec<CameraInfo>> = None;
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
    }
}

impl LinuxBackend {
    fn live(&self) -> Result<(), VideoError> {
        if self.inner.shut.load(Ordering::Acquire) {
            Err(VideoError::ShutDown)
        } else {
            Ok(())
        }
    }
}

impl VideoBackend for LinuxBackend {
    fn capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::new("linux", os_name());
        caps.camera = true;
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

    fn open_camera(&self, id: Option<&str>, c: CaptureConstraints, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
        self.live()?;
        camera::open(id, c, wake)
    }

    fn screen_permission(&self) -> Permission {
        // The portal asks each time a capture starts.
        Permission::NotDetermined
    }

    fn request_screen_permission(&self, done: Done<Permission>) {
        done(Permission::NotDetermined)
    }

    fn screen_sources(&self) -> Result<Vec<ScreenSourceInfo>, VideoError> {
        self.live()?;
        // Only the portal's picker can choose a screen on Linux.
        Ok(Vec::new())
    }

    fn pick_screen(&self, _opts: PickOptions, done: Done<Result<ScreenSelection, VideoError>>) {
        done(Err(unsupported("Linux screen capture (the ScreenCast portal) isn't built yet")))
    }

    fn open_screen(&self, _s: &ScreenSelection, _c: CaptureConstraints, _wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
        self.live()?;
        Err(unsupported("Linux screen capture (the ScreenCast portal) isn't built yet"))
    }

    fn encoder(&self, config: EncoderConfig, _wake: Wake) -> Result<Box<dyn VideoEncoder>, VideoError> {
        self.live()?;
        Err(unsupported(format!("no {:?} encoder on Linux yet", config.codec.codec())))
    }

    fn decoder(&self, config: DecoderConfig, _wake: Wake) -> Result<Box<dyn VideoDecoder>, VideoError> {
        self.live()?;
        Err(unsupported(format!("no {:?} decoder on Linux yet", config.codec)))
    }

    fn convert(&self, frame: &VideoFrame, width: u32, height: u32, format: PixelFormat) -> Result<VideoFrame, VideoError> {
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
mod tests {
    use super::*;
    use crate::api::no_wake;

    #[test]
    fn the_backend_starts_and_reports_itself() {
        let b = backend().expect("backend");
        let caps = b.capabilities();
        eprintln!("[linux] {}", caps.os);
        assert_eq!(caps.backend, "linux");
        assert!(caps.camera);
        assert!(caps.codecs.iter().all(|c| c.codec != VideoCodec::H264), "no H.264 on Linux");
        assert!(matches!(b.encoder(EncoderConfig::h264(640, 360, 30, 500_000), no_wake()).err(), Some(VideoError::Unsupported(_))));
        assert!(b.screen_sources().expect("list").is_empty());
        b.shutdown();
        assert_eq!(b.screen_sources().err(), Some(VideoError::ShutDown));
    }
}
