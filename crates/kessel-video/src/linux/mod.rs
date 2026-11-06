//! The Linux backend: V4L2 cameras through raw ioctls, and screens through
//! the xdg-desktop-portal ScreenCast (zbus) with a PipeWire stream (the
//! system's libpipewire, loaded at run time). Linux has no OS H.264, so
//! codecs come from the VP8 crates.

#[cfg(not(target_pointer_width = "64"))]
compile_error!("the V4L2 and SPA structs here are laid out for 64-bit targets");

mod camera;
mod pipewire;
mod portal;
mod screen;
mod v4l2;

use crate::api::{
    Done, EventHub, EventRx, FrameSource, VideoBackend, VideoDecoder, VideoEncoder, Wake,
};
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
            s.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| "Linux".into());
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
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
        crate::rust_codecs::add_capabilities(&mut caps);
        caps.camera = true;
        let screens = screen::supported();
        caps.screen = ScreenCapability {
            supported: screens,
            // The portal is the only way to choose a screen on Linux.
            system_picker: screens,
            list: false,
            windows: screen::windows_supported(),
            // Embedded in the frames when the portal offers it.
            cursor: screens,
            // The portal shares what the person picks; it can't leave our
            // own windows out of a display.
            exclude_own_windows: false,
        };
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
        // Nothing to grant ahead: the portal asks the person at each pick.
        Permission::Granted
    }

    fn request_screen_permission(&self, done: Done<Permission>) {
        done(Permission::Granted)
    }

    fn screen_sources(&self) -> Result<Vec<ScreenSourceInfo>, VideoError> {
        self.live()?;
        // Only the portal's picker can choose a screen on Linux.
        Ok(Vec::new())
    }

    fn pick_screen(&self, opts: PickOptions, done: Done<Result<ScreenSelection, VideoError>>) {
        if self.live().is_err() {
            return done(Err(VideoError::ShutDown));
        }
        screen::pick(opts, done)
    }

    fn open_screen(
        &self,
        s: &ScreenSelection,
        c: CaptureConstraints,
        wake: Wake,
    ) -> Result<Box<dyn FrameSource>, VideoError> {
        self.live()?;
        screen::open(s, c, wake)
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
        Err(unsupported(format!(
            "no {:?} encoder on Linux yet",
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
            "no {:?} decoder on Linux yet",
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
        assert!(
            caps.codecs.iter().all(|c| c.codec != VideoCodec::H264),
            "no H.264 on Linux"
        );
        assert!(matches!(
            b.encoder(EncoderConfig::h264(640, 360, 30, 500_000), no_wake())
                .err(),
            Some(VideoError::Unsupported(_))
        ));
        assert!(b.screen_sources().expect("list").is_empty());
        assert!(!caps.screen.list && !caps.screen.exclude_own_windows);
        eprintln!("[linux] screen capture here: {}", caps.screen.supported);
        assert_eq!(
            b.open_screen(
                &ScreenSelection::Listed("display:0".into()),
                CaptureConstraints::default(),
                no_wake()
            )
            .err(),
            Some(VideoError::NotFound),
            "Linux has no list: only the portal picks"
        );
        b.shutdown();
        assert_eq!(b.screen_sources().err(), Some(VideoError::ShutDown));
    }

    /// By hand on a Linux desktop (GNOME, KDE, Hyprland...) with
    /// xdg-desktop-portal and PipeWire: scripts/try-linux-screen.sh. The
    /// portal's picker appears; pick a screen or a window.
    #[test]
    #[ignore = "needs a desktop session with the ScreenCast portal and a person to pick"]
    fn real_portal_screen_delivers_frames() {
        let b = backend().expect("backend");
        assert!(
            b.capabilities().screen.supported,
            "no portal or no libpipewire here"
        );
        let (tx, rx) = std::sync::mpsc::channel();
        b.pick_screen(
            PickOptions::default(),
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        );
        let sel = rx
            .recv_timeout(Duration::from_secs(120))
            .expect("an answer within 2 minutes")
            .expect("a pick");
        let mut c = CaptureConstraints::new(1280, 720, 15);
        c.cursor = true;
        let mut src = b.open_screen(&sel, c, no_wake()).expect("screen");
        let mut frames = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while frames < 30 && std::time::Instant::now() < deadline {
            if let Some(f) = src.try_recv() {
                assert!(
                    f.width <= 1280 && f.height <= 720,
                    "{}x{}",
                    f.width,
                    f.height
                );
                frames += 1;
            }
            while let Some(e) = src.try_event() {
                eprintln!("[linux] event {e:?}");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        eprintln!("[linux] {frames} frames, stats {:?}", src.stats());
        assert!(
            frames >= 5,
            "the stream delivered only {frames} frames in 10 s"
        );
        assert!(
            b.open_screen(&sel, c, no_wake()).is_err(),
            "a pick opens once"
        );
    }
}
