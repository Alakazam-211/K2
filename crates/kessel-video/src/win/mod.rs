//! The Windows backend: Media Foundation cameras and Windows Graphics
//! Capture screens and windows. OS components through the `windows` crate;
//! no codec library. Phase 1 publishes VP8, so there is no H.264 here yet
//! (the Media Foundation H.264 MFTs come in phase 2).

mod camera;
mod screen;

use crate::api::{Done, EventHub, EventRx, FrameSource, VideoBackend, VideoDecoder, VideoEncoder, Wake};
use crate::error::{unsupported, VideoError};
use crate::frame::{PixelFormat, VideoFrame};
use crate::kit::{lock, now_us};
use crate::types::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

/// The Windows backend.
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
    Ok(Arc::new(WinBackend {
        inner,
        monitor: Mutex::new(Some(monitor)),
    }))
}

/// The OS and version, for logs: "Windows 10.0 26100".
pub(crate) fn os_name() -> String {
    use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
    let mut v = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: a correctly sized OSVERSIONINFOW that RtlGetVersion fills.
    let st = unsafe { windows::Wdk::System::SystemServices::RtlGetVersion(&mut v) };
    if st.0 >= 0 {
        format!("Windows {}.{} {}", v.dwMajorVersion, v.dwMinorVersion, v.dwBuildNumber)
    } else {
        "Windows".to_string()
    }
}

struct Inner {
    events: EventHub,
    shut: AtomicBool,
}

struct WinBackend {
    inner: Arc<Inner>,
    monitor: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for WinBackend {
    fn drop(&mut self) {
        self.inner.shut.store(true, Ordering::Release);
        if let Some(t) = lock(&self.monitor).take() {
            let _ = t.join();
        }
    }
}

/// Polls the camera list once a second while someone is subscribed and
/// publishes changes. Windows sends no thermal or low-power events.
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

impl WinBackend {
    fn live(&self) -> Result<(), VideoError> {
        if self.inner.shut.load(Ordering::Acquire) {
            Err(VideoError::ShutDown)
        } else {
            Ok(())
        }
    }
}

impl VideoBackend for WinBackend {
    fn capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::new("windows", os_name());
        caps.camera = true;
        let wgc = screen::supported();
        // No system picker yet: GraphicsCapturePicker needs an owner HWND
        // from the app (IInitializeWithWindow) and a UI thread. No window
        // exclusion yet: WDA_EXCLUDEFROMCAPTURE hides our windows from every
        // capture and screenshot, not just ours.
        caps.screen = ScreenCapability {
            supported: wgc,
            system_picker: false,
            list: wgc,
            windows: wgc,
            cursor: wgc,
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
        // Windows has no in-app prompt for desktop apps: the person changes
        // it in Settings > Privacy > Camera. Answer with the state now.
        done(camera::permission())
    }

    fn open_camera(&self, id: Option<&str>, c: CaptureConstraints, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
        self.live()?;
        camera::open(id, c, wake)
    }

    fn screen_permission(&self) -> Permission {
        Permission::Granted
    }

    fn request_screen_permission(&self, done: Done<Permission>) {
        done(Permission::Granted)
    }

    fn screen_sources(&self) -> Result<Vec<ScreenSourceInfo>, VideoError> {
        self.live()?;
        screen::sources()
    }

    fn pick_screen(&self, _opts: PickOptions, done: Done<Result<ScreenSelection, VideoError>>) {
        if self.live().is_err() {
            return done(Err(VideoError::ShutDown));
        }
        done(Err(unsupported("no system screen picker on Windows yet; use screen_sources")))
    }

    fn open_screen(&self, source: &ScreenSelection, c: CaptureConstraints, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
        self.live()?;
        screen::open(source, c, wake)
    }

    fn encoder(&self, config: EncoderConfig, _wake: Wake) -> Result<Box<dyn VideoEncoder>, VideoError> {
        self.live()?;
        Err(unsupported(format!("no {:?} encoder on Windows yet", config.codec.codec())))
    }

    fn decoder(&self, config: DecoderConfig, _wake: Wake) -> Result<Box<dyn VideoDecoder>, VideoError> {
        self.live()?;
        Err(unsupported(format!("no {:?} decoder on Windows yet (the webview's WebCodecs decodes)", config.codec)))
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

/// A Windows error as a [`VideoError`], mapping the codes callers act on.
pub(crate) fn map_error(e: &windows::core::Error, what: &str) -> VideoError {
    use windows::Win32::Foundation::E_ACCESSDENIED;
    use windows::Win32::Media::MediaFoundation::{MF_E_HW_MFT_FAILED_START_STREAMING, MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED, MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED};
    /// `HRESULT_FROM_WIN32(ERROR_BUSY)`.
    const E_BUSY: i32 = 0x8007_00AA_u32 as i32;
    let code = e.code();
    if code == E_ACCESSDENIED {
        VideoError::NoPermission
    } else if code == MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED {
        VideoError::DeviceLost
    } else if code == MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED || code == MF_E_HW_MFT_FAILED_START_STREAMING || code.0 == E_BUSY {
        VideoError::DeviceBusy
    } else {
        VideoError::os(code.0 as i64, format!("{what}: {}", e.message()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::no_wake;

    #[test]
    fn errors_map_to_what_callers_act_on() {
        use windows::Win32::Foundation::E_ACCESSDENIED;
        use windows::Win32::Media::MediaFoundation::{MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED, MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED};
        let e = |h| windows::core::Error::new(h, "test");
        assert_eq!(map_error(&e(E_ACCESSDENIED), "x"), VideoError::NoPermission);
        assert_eq!(map_error(&e(MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED), "x"), VideoError::DeviceLost);
        assert_eq!(map_error(&e(MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED), "x"), VideoError::DeviceBusy);
        assert!(matches!(map_error(&e(windows::core::HRESULT(0x8000_4005_u32 as i32)), "x"), VideoError::Os { .. }));
    }

    #[test]
    fn the_backend_starts_and_answers_without_hardware() {
        let b = backend().expect("backend");
        let caps = b.capabilities();
        assert_eq!(caps.backend, "windows");
        assert!(caps.os.starts_with("Windows"), "{}", caps.os);
        assert!(caps.codecs.is_empty(), "phase 1: no OS codecs");
        assert!(!caps.screen.system_picker);
        let cams = b.cameras();
        for c in &cams {
            assert!(!c.id.is_empty());
        }
        assert!(matches!(b.encoder(EncoderConfig::h264(640, 360, 30, 500_000), no_wake()).err(), Some(VideoError::Unsupported(_))));
        b.shutdown();
        assert_eq!(b.screen_sources().err(), Some(VideoError::ShutDown));
    }

    /// By hand on a Windows machine with a camera:
    /// `cargo test -p kessel-video real_camera -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs a camera"]
    fn real_camera_delivers_frames() {
        let b = backend().expect("backend");
        let mut src = b.open_camera(None, CaptureConstraints::new(1280, 720, 30), no_wake()).expect("camera");
        let f = crate::kit::wait_for(Duration::from_secs(5), || src.try_recv()).expect("a frame in 5 s");
        assert_eq!(f.format(), PixelFormat::Nv12);
    }

    /// By hand on a Windows desktop session.
    #[test]
    #[ignore = "needs a desktop session"]
    fn real_screen_delivers_frames() {
        let b = backend().expect("backend");
        let list = b.screen_sources().expect("sources");
        let display = list.iter().find(|s| s.kind == ScreenKind::Display).expect("a display");
        let mut src = b.open_screen(&ScreenSelection::Listed(display.id.clone()), CaptureConstraints::new(1280, 720, 15), no_wake()).expect("screen");
        let f = crate::kit::wait_for(Duration::from_secs(5), || src.try_recv()).expect("a frame in 5 s");
        assert!(f.width <= 1280 && f.height <= 720);
    }
}
