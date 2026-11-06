//! The Windows backend: Media Foundation cameras, Windows Graphics Capture
//! screens and windows, and H.264 through Media Foundation's encoder and
//! decoder MFTs (the GPU driver's hardware ones, or Microsoft's software
//! ones that ship with Windows). OS components through the `windows` crate;
//! no codec library. Media Foundation's DLLs are loaded at run time, so an
//! N or KN edition without the Media Feature Pack runs with no cameras and
//! no H.264 instead of failing to start.

mod camera;
mod h264_dec;
mod h264_enc;
mod mfdll;
mod mft;
mod screen;

use crate::api::{
    Done, EventHub, EventRx, FrameSource, VideoBackend, VideoDecoder, VideoEncoder, Wake,
};
use crate::error::{unsupported, VideoError};
use crate::frame::{PixelFormat, VideoFrame};
use crate::kit::{lock, now_us, EncoderCore, ThreadedDecoder, ThreadedEncoder};
use crate::types::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
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
        format!(
            "Windows {}.{} {}",
            v.dwMajorVersion, v.dwMinorVersion, v.dwBuildNumber
        )
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

/// The H.264 MFTs that work on this machine, found once: a kind is listed
/// only when a session of it opened (and, for encoders, encoded a frame),
/// never because Windows is Windows.
#[derive(Debug, Default)]
struct H264Probe {
    /// The friendly names of the first working MFT of each kind.
    hardware_encoder: Option<String>,
    software_encoder: Option<String>,
    hardware_decoder: Option<String>,
    software_decoder: Option<String>,
}

/// Does `found` encode a 720p frame?
fn encodes(found: &mft::Found) -> bool {
    let cfg = EncoderConfig::h264(1280, 720, 30, 1_000_000);
    let Ok(mut enc) = h264_enc::MfEncoder::open_one(found, &cfg) else {
        return false;
    };
    let Ok(frame) = VideoFrame::filled(PixelFormat::Nv12, 1280, 720, [128, 128, 128], 0) else {
        return false;
    };
    matches!(enc.encode(&frame, true), Ok(Some(f)) if f.keyframe)
}

fn probe_now() -> H264Probe {
    let mut p = H264Probe::default();
    // Unsupported (no Media Foundation) leaves every kind out.
    let enc = mft::find(mft::Role::Encoder, ImplPreference::PreferHardware).unwrap_or_default();
    p.hardware_encoder = enc
        .iter()
        .find(|f| f.hardware && encodes(f))
        .map(|f| f.name.clone());
    p.software_encoder = enc
        .iter()
        .find(|f| !f.hardware && encodes(f))
        .map(|f| f.name.clone());
    let dec = mft::find(mft::Role::Decoder, ImplPreference::PreferHardware).unwrap_or_default();
    let decodes = |f: &&mft::Found| h264_dec::open_one(f).is_ok();
    p.hardware_decoder = dec
        .iter()
        .find(|f| f.hardware && decodes(f))
        .map(|f| f.name.clone());
    p.software_decoder = dec
        .iter()
        .find(|f| !f.hardware && decodes(f))
        .map(|f| f.name.clone());
    p
}

fn h264_probe() -> &'static H264Probe {
    static PROBE: OnceLock<H264Probe> = OnceLock::new();
    PROBE.get_or_init(|| mft::on_com_thread(probe_now))
}

/// The H.264 capability entries the probe found.
fn h264_capabilities() -> Vec<CodecCapability> {
    let p = h264_probe();
    let formats = vec![
        PixelFormat::Nv12,
        PixelFormat::I420,
        PixelFormat::Bgra,
        PixelFormat::Rgba,
    ];
    let entry = |dir, implementation, name: &str, (w, h): (u32, u32), level| {
        let mut c = CodecCapability::new(
            VideoCodec::H264,
            dir,
            implementation,
            format!("Media Foundation: {name}"),
            w,
            h,
            30,
        );
        c.h264 = Some(H264Capability::new(
            vec![
                H264Profile::ConstrainedBaseline,
                H264Profile::ConstrainedHigh,
            ],
            level,
        ));
        c.formats = formats.clone();
        c
    };
    let mut out = Vec::new();
    // Hardware: what level 5.1 allows; the GPU may refuse a size, and the
    // open says so. Microsoft's software encoder: 1080p in real time.
    let kinds = [
        (
            &p.hardware_encoder,
            CodecDirection::Encode,
            Implementation::Hardware,
            (4096, 2304),
            H264Level::L5_1,
        ),
        (
            &p.software_encoder,
            CodecDirection::Encode,
            Implementation::Software,
            (1920, 1080),
            H264Level::L4_0,
        ),
        (
            &p.hardware_decoder,
            CodecDirection::Decode,
            Implementation::Hardware,
            (4096, 2304),
            H264Level::L5_1,
        ),
        (
            &p.software_decoder,
            CodecDirection::Decode,
            Implementation::Software,
            (4096, 2304),
            H264Level::L5_1,
        ),
    ];
    for (name, dir, imp, size, level) in kinds {
        if let Some(name) = name {
            out.push(entry(dir, imp, name, size, level));
        }
    }
    out
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
        crate::rust_codecs::add_capabilities(&mut caps);
        caps.codecs.extend(h264_capabilities());
        // Media Foundation is missing on an N or KN edition without the
        // Media Feature Pack.
        caps.camera = mfdll::mfplat().is_ok() && mfdll::mfcapture().is_ok();
        let wgc = screen::supported();
        // The system picker (GraphicsCapturePicker) needs the app's HWND:
        // PickOptions::with_parent_window. No window exclusion: Windows' only
        // way (SetWindowDisplayAffinity with WDA_EXCLUDEFROMCAPTURE) hides
        // our windows from every capture and screenshot on the machine, not
        // just from this one.
        caps.screen = ScreenCapability {
            supported: wgc,
            system_picker: wgc,
            list: wgc,
            windows: wgc,
            cursor: wgc,
            exclude_own_windows: false,
            frame_in: false,
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
        Permission::Granted
    }

    fn request_screen_permission(&self, done: Done<Permission>) {
        done(Permission::Granted)
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
                let core = mft::on_com_thread(|| h264_enc::MfEncoder::open(&config))?;
                Ok(Box::new(ThreadedEncoder::start(
                    Box::new(core),
                    config,
                    wake,
                )?))
            }
            _ => Err(unsupported(format!(
                "no {:?} encoder on Windows",
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
                let core = mft::on_com_thread(|| h264_dec::MfDecoder::open(&config))?;
                Ok(Box::new(ThreadedDecoder::start(
                    Box::new(core),
                    config,
                    wake,
                )?))
            }
            _ => Err(unsupported(format!(
                "no {:?} decoder on Windows",
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

/// A Windows error as a [`VideoError`], mapping the codes callers act on.
pub(crate) fn map_error(e: &windows::core::Error, what: &str) -> VideoError {
    use windows::Win32::Foundation::E_ACCESSDENIED;
    use windows::Win32::Media::MediaFoundation::{
        MF_E_HW_MFT_FAILED_START_STREAMING, MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED,
        MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED,
    };
    /// `HRESULT_FROM_WIN32(ERROR_BUSY)`.
    const E_BUSY: i32 = 0x8007_00AA_u32 as i32;
    let code = e.code();
    if code == E_ACCESSDENIED {
        VideoError::NoPermission
    } else if code == MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED {
        VideoError::DeviceLost
    } else if code == MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED
        || code == MF_E_HW_MFT_FAILED_START_STREAMING
        || code.0 == E_BUSY
    {
        VideoError::DeviceBusy
    } else {
        VideoError::os(code.0 as i64, format!("{what}: {}", e.message()))
    }
}

#[cfg(test)]
mod h264_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::no_wake;

    #[test]
    fn errors_map_to_what_callers_act_on() {
        use windows::Win32::Foundation::E_ACCESSDENIED;
        use windows::Win32::Media::MediaFoundation::{
            MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED, MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED,
        };
        let e = |h| windows::core::Error::new(h, "test");
        assert_eq!(map_error(&e(E_ACCESSDENIED), "x"), VideoError::NoPermission);
        assert_eq!(
            map_error(&e(MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED), "x"),
            VideoError::DeviceLost
        );
        assert_eq!(
            map_error(&e(MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED), "x"),
            VideoError::DeviceBusy
        );
        assert!(matches!(
            map_error(&e(windows::core::HRESULT(0x8000_4005_u32 as i32)), "x"),
            VideoError::Os { .. }
        ));
    }

    #[test]
    fn the_backend_starts_and_answers_without_hardware() {
        let b = backend().expect("backend");
        let caps = b.capabilities();
        assert_eq!(caps.backend, "windows");
        assert!(caps.os.starts_with("Windows"), "{}", caps.os);
        // H.264 is listed exactly when an encoder opens (the probe and the
        // open agree).
        let listed = caps
            .codecs
            .iter()
            .any(|c| c.codec == VideoCodec::H264 && c.direction == CodecDirection::Encode);
        let opened = b.encoder(EncoderConfig::h264(640, 360, 30, 500_000), no_wake());
        match &opened {
            Ok(_) => assert!(listed, "an H.264 encoder opened but none is listed"),
            Err(VideoError::Unsupported(_)) => assert!(!listed, "listed but none opened"),
            Err(e) => panic!("{e}"),
        }
        drop(opened);
        assert_eq!(caps.screen.system_picker, caps.screen.supported);
        assert!(!caps.screen.exclude_own_windows);
        let (tx, rx) = std::sync::mpsc::channel();
        b.pick_screen(
            PickOptions::new(),
            Box::new(move |r| {
                let _ = tx.send(r.err());
            }),
        );
        let refused = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("answered at once");
        assert!(
            matches!(
                refused,
                Some(VideoError::Invalid(_)) | Some(VideoError::Unsupported(_))
            ),
            "{refused:?}"
        );
        let cams = b.cameras();
        for c in &cams {
            assert!(!c.id.is_empty());
        }
        b.shutdown();
        assert_eq!(b.screen_sources().err(), Some(VideoError::ShutDown));
    }

    /// By hand on a Windows machine with a camera:
    /// `cargo test -p kessel-video real_camera -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs a camera"]
    fn real_camera_delivers_frames() {
        let b = backend().expect("backend");
        let mut src = b
            .open_camera(None, CaptureConstraints::new(1280, 720, 30), no_wake())
            .expect("camera");
        let f = crate::kit::wait_for(Duration::from_secs(5), || src.try_recv())
            .expect("a frame in 5 s");
        assert_eq!(f.format(), PixelFormat::Nv12);
    }

    /// By hand on a Windows desktop session.
    #[test]
    #[ignore = "needs a desktop session"]
    fn real_screen_delivers_frames() {
        let b = backend().expect("backend");
        let list = b.screen_sources().expect("sources");
        let display = list
            .iter()
            .find(|s| s.kind == ScreenKind::Display)
            .expect("a display");
        let mut src = b
            .open_screen(
                &ScreenSelection::Listed(display.id.clone()),
                CaptureConstraints::new(1280, 720, 15),
                no_wake(),
            )
            .expect("screen");
        let f = crate::kit::wait_for(Duration::from_secs(5), || src.try_recv())
            .expect("a frame in 5 s");
        assert!(f.width <= 1280 && f.height <= 720);
    }

    /// By hand from a terminal on a Windows desktop: the picker opens over
    /// the terminal's window; pick a display or a window within 2 minutes.
    /// `cargo test -p kessel-video --lib real_picker -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a desktop session and a person to pick"]
    fn real_picker_delivers_frames() {
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        let b = backend().expect("backend");
        // SAFETY: a plain query.
        let hwnd = unsafe { GetForegroundWindow() };
        assert!(!hwnd.0.is_null(), "no foreground window to own the picker");
        let (tx, rx) = std::sync::mpsc::channel();
        b.pick_screen(
            PickOptions::new().with_parent_window(hwnd.0 as usize as u64),
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        );
        let sel = rx
            .recv_timeout(Duration::from_secs(120))
            .expect("an answer within 2 minutes")
            .expect("a pick");
        let mut src = b
            .open_screen(&sel, CaptureConstraints::new(1280, 720, 15), no_wake())
            .expect("screen");
        let f = crate::kit::wait_for(Duration::from_secs(5), || src.try_recv())
            .expect("a frame in 5 s");
        assert!(f.width <= 1280 && f.height <= 720);
    }
}
