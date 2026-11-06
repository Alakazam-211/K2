//! The Android backend over a scripted [`Platform`]: cameras, permission,
//! upright frames, the app going to the background and back, cameras taken
//! by other apps, MediaProjection screens (sized, turned, stopped, and not
//! paused in the background), frame-in sources, and VP8 through the
//! backend. Runs on any host.

use super::meta::{camera_result, camera_status, describe, mark_default, RawCharacteristics};
use super::*;
use crate::api::no_wake;
use crate::kit::{wait_for, CoreFrame};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(5);

/// A camera that is running: its sink, to push images and losses.
struct Opened {
    id: String,
    mode: CaptureMode,
    fps_range: Option<(u32, u32)>,
    sink: Arc<dyn ImageSink>,
    alive: Arc<AtomicBool>,
}

/// The sizes a virtual display was given, in order.
type Sizes = Arc<Mutex<Vec<(u32, u32, u32)>>>;

/// A virtual display that is running: its sink, the sizes it was given,
/// and whether it was released.
struct Projected {
    projection: usize,
    sizes: Sizes,
    sink: Arc<dyn ScreenSink>,
    alive: Arc<AtomicBool>,
}

#[derive(Default)]
struct Script {
    /// The next opens fail with these, in order.
    open_errors: Vec<VideoError>,
    opened: Vec<Opened>,
    projected: Vec<Projected>,
}

struct MockProjection {
    sizes: Sizes,
    alive: Arc<AtomicBool>,
}

impl ProjectionCapture for MockProjection {
    fn resize(&mut self, width: u32, height: u32, dpi: u32) -> Result<(), VideoError> {
        lock(&self.sizes).push((width, height, dpi));
        Ok(())
    }
}

impl Drop for MockProjection {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}

struct Mock {
    cams: Vec<CamDesc>,
    permission: Permission,
    script: Mutex<Script>,
    /// What the AVC probe finds (a phone with a hardware encoder by
    /// default).
    avc: Mutex<AvcProbe>,
    /// How many times the backend probed.
    probes: std::sync::atomic::AtomicUsize,
    /// What the next H.264 encoders turn out to be, in order (then
    /// `Hardware`), and the preferences they were opened with.
    encoders: Mutex<Vec<Implementation>>,
    opened_prefs: Mutex<Vec<ImplPreference>>,
}

struct MockCapture {
    alive: Arc<AtomicBool>,
}

impl Capture for MockCapture {}

impl Drop for MockCapture {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}

/// A fake H.264 core: one byte per frame, the keyframe flag kept, and the
/// implementation the script gave it.
struct MockH264(Implementation);

impl EncoderCore for MockH264 {
    fn input_format(&self) -> PixelFormat {
        PixelFormat::Nv12
    }
    fn encode(&mut self, _f: &VideoFrame, key: bool) -> Result<Option<CoreFrame>, VideoError> {
        Ok(Some(CoreFrame::new(
            vec![0, 0, 0, 1, if key { 0x65 } else { 0x41 }],
            key,
        )))
    }
    fn set_rates(&mut self, _r: Rates) -> Result<(), VideoError> {
        Ok(())
    }
    fn set_size(&mut self, _w: u32, _h: u32) -> Result<(), VideoError> {
        Ok(())
    }
    fn implementation(&self) -> Implementation {
        self.0
    }
    fn name(&self) -> String {
        "mock MediaCodec".into()
    }
}

impl Platform for Mock {
    fn os_name(&self) -> String {
        "Android 15 (API 35)".into()
    }

    fn cameras(&self) -> Result<Vec<CamDesc>, VideoError> {
        Ok(self.cams.clone())
    }

    fn camera_permission(&self) -> Permission {
        self.permission
    }

    fn open_camera(
        &self,
        cam: &CamDesc,
        mode: CaptureMode,
        fps_range: Option<(u32, u32)>,
        sink: Arc<dyn ImageSink>,
    ) -> Result<Box<dyn Capture>, VideoError> {
        let mut s = lock(&self.script);
        if !s.open_errors.is_empty() {
            return Err(s.open_errors.remove(0));
        }
        if self.permission == Permission::Denied {
            // What ACameraManager_openCamera answers without the permission.
            camera_result(
                camera_status::PERMISSION_DENIED,
                "ACameraManager_openCamera",
            )?;
        }
        let alive = Arc::new(AtomicBool::new(true));
        s.opened.push(Opened {
            id: cam.info.id.clone(),
            mode,
            fps_range,
            sink,
            alive: alive.clone(),
        });
        Ok(Box::new(MockCapture { alive }))
    }

    fn h264_probe(&self) -> AvcProbe {
        self.probes.fetch_add(1, Ordering::SeqCst);
        lock(&self.avc).clone()
    }

    fn h264_encoder(&self, cfg: &EncoderConfig) -> Result<Box<dyn EncoderCore>, VideoError> {
        lock(&self.opened_prefs).push(cfg.implementation);
        let mut next = lock(&self.encoders);
        let imp = if next.is_empty() {
            Implementation::Hardware
        } else {
            next.remove(0)
        };
        Ok(Box::new(MockH264(imp)))
    }

    fn open_projection(
        &self,
        projection: usize,
        size: (u32, u32, u32),
        sink: Arc<dyn ScreenSink>,
    ) -> Result<Box<dyn ProjectionCapture>, VideoError> {
        let mut s = lock(&self.script);
        if !s.open_errors.is_empty() {
            return Err(s.open_errors.remove(0));
        }
        let sizes = Arc::new(Mutex::new(vec![size]));
        let alive = Arc::new(AtomicBool::new(true));
        s.projected.push(Projected {
            projection,
            sizes: sizes.clone(),
            sink,
            alive: alive.clone(),
        });
        Ok(Box::new(MockProjection { sizes, alive }))
    }
}

/// A phone: back camera "0" (sensor at 90) and front camera "1" (at 270),
/// both 1280x720 and 640x480 at up to 30 fps.
fn phone(permission: Permission) -> Arc<Mock> {
    let yuv = meta::FORMAT_YUV_420_888;
    let configs = [yuv, 1280, 720, 0, yuv, 640, 480, 0];
    let ranges = [15, 30, 30, 30];
    let mut cams: Vec<CamDesc> = [(1u8, 90, "0"), (0u8, 270, "1")]
        .iter()
        .map(|&(facing, orientation, id)| {
            describe(
                id,
                &RawCharacteristics {
                    facing: Some(facing),
                    orientation: Some(orientation),
                    stream_configs: &configs,
                    min_durations: &[],
                    fps_ranges: &ranges,
                },
            )
            .expect("a YUV camera")
        })
        .collect();
    mark_default(&mut cams);
    Arc::new(Mock {
        cams,
        permission,
        script: Mutex::new(Script::default()),
        avc: Mutex::new(AvcProbe {
            default_name: Some("c2.acme.avc.encoder".into()),
            software: Some("c2.android.avc.encoder".into()),
        }),
        probes: std::sync::atomic::AtomicUsize::new(0),
        encoders: Mutex::new(Vec::new()),
        opened_prefs: Mutex::new(Vec::new()),
    })
}

fn start(mock: &Arc<Mock>) -> (Arc<dyn VideoBackend>, AndroidContext) {
    let ctx = AndroidContext::default();
    let platform: Arc<dyn Platform> = mock.clone();
    (AndroidBackend::start(platform, ctx.clone()), ctx)
}

/// A sensor-sized image (landscape `w` x `h`): luma rises left to right,
/// planar chroma.
struct TestImage {
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    w: u32,
    h: u32,
}

impl TestImage {
    fn new(w: u32, h: u32) -> TestImage {
        let (cw, ch) = ((w as usize).div_ceil(2), (h as usize).div_ceil(2));
        let y = (0..h as usize)
            .flat_map(|_| (0..w as usize).map(move |x| (x * 255 / (w as usize - 1)) as u8))
            .collect();
        TestImage {
            y,
            u: vec![100; cw * ch],
            v: vec![150; cw * ch],
            w,
            h,
        }
    }

    fn view(&self) -> Yuv420<'_> {
        fn plane(data: &[u8], stride: u32) -> image::Plane<'_> {
            image::Plane {
                data,
                row_stride: stride as usize,
                pixel_stride: 1,
            }
        }
        Yuv420 {
            width: self.w,
            height: self.h,
            planes: [
                plane(&self.y, self.w),
                plane(&self.u, self.w.div_ceil(2)),
                plane(&self.v, self.w.div_ceil(2)),
            ],
        }
    }
}

fn sink_of(mock: &Mock, i: usize) -> Arc<dyn ImageSink> {
    lock(&mock.script).opened[i].sink.clone()
}

fn next_frame(src: &mut Box<dyn FrameSource>) -> VideoFrame {
    wait_for(WAIT, || src.try_recv()).expect("a frame")
}

fn events(src: &mut Box<dyn FrameSource>) -> Vec<SourceEvent> {
    std::iter::from_fn(|| src.try_event()).collect()
}

#[test]
fn cameras_list_with_the_front_one_as_default() {
    let mock = phone(Permission::Granted);
    let (b, _ctx) = start(&mock);
    let cams = b.cameras();
    assert_eq!(cams.len(), 2);
    assert_eq!((cams[0].facing, cams[0].is_default), (Facing::Back, false));
    assert_eq!((cams[1].facing, cams[1].is_default), (Facing::Front, true));
    let caps = b.capabilities();
    assert_eq!(caps.backend, "android");
    assert_eq!(caps.os, "Android 15 (API 35)");
    assert!(caps.camera);
    assert!(!caps.screen.supported && !caps.screen.system_picker);
    let h264 = caps
        .best(VideoCodec::H264, CodecDirection::Encode)
        .expect("H.264 encode");
    assert_eq!(h264.implementation, Implementation::Hardware);
    assert!(
        caps.best(VideoCodec::H264, CodecDirection::Decode)
            .is_none(),
        "no H.264 decoder yet"
    );
    #[cfg(feature = "vp8")]
    {
        assert!(caps.best(VideoCodec::Vp8, CodecDirection::Encode).is_some());
        assert!(caps.best(VideoCodec::Vp8, CodecDirection::Decode).is_some());
    }
}

#[test]
fn front_back_and_id_choose_the_camera_and_mode() {
    let mock = phone(Permission::Granted);
    let (b, _ctx) = start(&mock);
    let _default = b
        .open_camera(None, CaptureConstraints::new(640, 480, 30), no_wake())
        .expect("default");
    let mut back = CaptureConstraints::new(1280, 720, 30);
    back.facing = Some(Facing::Back);
    let _back = b.open_camera(None, back, no_wake()).expect("back");
    // A portrait box picks the same landscape sensor mode.
    let _by_id = b
        .open_camera(Some("1"), CaptureConstraints::new(720, 1280, 15), no_wake())
        .expect("by id");
    let s = lock(&mock.script);
    // (id, mode width, mode height, AE range)
    type Open<'a> = (&'a str, u32, u32, Option<(u32, u32)>);
    let got: Vec<Open<'_>> = s
        .opened
        .iter()
        .map(|o| (o.id.as_str(), o.mode.width, o.mode.height, o.fps_range))
        .collect();
    assert_eq!(
        got,
        vec![
            ("1", 640, 480, Some((30, 30))),
            ("0", 1280, 720, Some((30, 30))),
            ("1", 1280, 720, Some((15, 30))),
        ]
    );
    drop(s);
    assert_eq!(
        b.open_camera(Some("9"), CaptureConstraints::default(), no_wake())
            .err(),
        Some(VideoError::NotFound)
    );
}

#[test]
fn a_denied_permission_is_a_clear_error() {
    let mock = phone(Permission::Denied);
    let (b, _ctx) = start(&mock);
    assert_eq!(b.camera_permission(), Permission::Denied);
    let (tx, rx) = std::sync::mpsc::channel();
    b.request_camera_permission(Box::new(move |p| {
        let _ = tx.send(p);
    }));
    assert_eq!(
        rx.recv_timeout(WAIT).expect("an answer"),
        Permission::Denied
    );
    let err = b
        .open_camera(None, CaptureConstraints::default(), no_wake())
        .err()
        .expect("refused");
    assert_eq!(err, VideoError::NoPermission);
    assert_eq!(err.code(), "no_permission");
}

#[test]
fn frames_come_upright_and_fit_the_box_in_any_orientation() {
    let mock = phone(Permission::Granted);
    let (b, ctx) = start(&mock);
    let mut back = CaptureConstraints::new(640, 360, 30);
    back.facing = Some(Facing::Back);
    let mut src = b.open_camera(None, back, no_wake()).expect("back");
    let sink = sink_of(&mock, 0);
    let img = TestImage::new(1280, 720);

    // Held upright: the back sensor's 90 turns the landscape image
    // portrait, and the landscape box fits it as 360x640.
    sink.image(&img.view());
    let f = next_frame(&mut src);
    assert_eq!((f.width, f.height), (360, 640));
    assert_eq!(f.format(), PixelFormat::I420);
    // Turned 90: the sensor's left edge (dark) is now the top.
    let p = f.planes().expect("planes");
    let top = p.row(0, 0, f.width, f.height)[180];
    let bottom = p.row(0, 639, f.width, f.height)[180];
    assert!(top < 10 && bottom > 245, "top {top}, bottom {bottom}");
    let ev = events(&mut src);
    assert!(
        ev.contains(&SourceEvent::Started {
            width: 360,
            height: 640,
            fps: 30
        }),
        "{ev:?}"
    );

    // Turned to landscape: the image is upright as it comes, 640x360.
    ctx.set_device_orientation(270);
    std::thread::sleep(Duration::from_millis(40));
    sink.image(&img.view());
    let f = next_frame(&mut src);
    assert_eq!((f.width, f.height), (640, 360));

    // A new box applies from the next frame.
    src.set_constraints(CaptureConstraints::new(320, 180, 30))
        .expect("constraints");
    std::thread::sleep(Duration::from_millis(40));
    sink.image(&img.view());
    assert_eq!(next_frame(&mut src).width, 320);

    // Paused: images are released unread.
    src.set_paused(true);
    assert!(sink.idle());
    src.set_paused(false);
    assert!(!sink.idle());

    // A bad image is an error event; the camera keeps running.
    let mut bad = img.view();
    bad.planes[0].data = &img.y[..10];
    sink.image(&bad);
    let ev = events(&mut src);
    assert!(
        ev.iter()
            .any(|e| matches!(e, SourceEvent::Error(VideoError::Os { .. }))),
        "{ev:?}"
    );
    assert!(!ev.iter().any(|e| matches!(e, SourceEvent::Ended(_))));
}

#[test]
fn the_background_closes_cameras_and_the_foreground_reopens_them() {
    let mock = phone(Permission::Granted);
    let (b, ctx) = start(&mock);
    let events_rx = b.subscribe();
    let mut src = b
        .open_camera(None, CaptureConstraints::new(640, 480, 30), no_wake())
        .expect("camera");
    let first = lock(&mock.script).opened[0].alive.clone();
    assert!(first.load(Ordering::Acquire));

    ctx.set_background(true);
    assert!(!first.load(Ordering::Acquire), "closed in the background");
    assert_eq!(
        events(&mut src),
        vec![SourceEvent::Interrupted(Interruption::Background)]
    );
    assert_eq!(events_rx.try_recv(), Some(BackendEvent::Background(true)));
    assert_eq!(
        b.open_camera(None, CaptureConstraints::default(), no_wake())
            .err(),
        Some(VideoError::ShutDown),
        "no new cameras in the background"
    );
    // A loss the OS reports while backgrounded is expected, not an end.
    sink_of(&mock, 0).lost(Lost::Disconnected);
    assert!(events(&mut src).is_empty());

    ctx.set_background(false);
    assert_eq!(events_rx.try_recv(), Some(BackendEvent::Background(false)));
    let ev = wait_for(WAIT, || src.try_event()).expect("resumed");
    assert_eq!(ev, SourceEvent::Resumed);
    let s = lock(&mock.script);
    assert_eq!(s.opened.len(), 2, "reopened");
    assert_eq!(s.opened[1].mode, s.opened[0].mode, "in the same mode");
    assert!(s.opened[1].alive.load(Ordering::Acquire));
    drop(s);

    // Frames flow again through the new capture.
    sink_of(&mock, 1).image(&TestImage::new(640, 480).view());
    let f = next_frame(&mut src);
    assert_eq!((f.width, f.height), (480, 640), "the front sensor's 270");

    // Dropping the source closes the device.
    let second = lock(&mock.script).opened[1].alive.clone();
    drop(src);
    assert!(!second.load(Ordering::Acquire));
}

#[test]
fn a_camera_taken_by_another_app_comes_back() {
    let mock = phone(Permission::Granted);
    let (b, ctx) = start(&mock);
    let mut src = b
        .open_camera(None, CaptureConstraints::new(640, 480, 30), no_wake())
        .expect("camera");
    sink_of(&mock, 0).lost(Lost::InUse);
    sink_of(&mock, 0).lost(Lost::Disconnected);
    assert_eq!(
        events(&mut src),
        vec![SourceEvent::Interrupted(Interruption::InUseByAnother)],
        "one interruption, not an end"
    );
    // Still busy when the caller retries by unpausing.
    lock(&mock.script).open_errors.push(VideoError::DeviceBusy);
    src.set_paused(true);
    src.set_paused(false);
    assert_eq!(lock(&mock.script).opened.len(), 1);
    assert!(events(&mut src).is_empty(), "no second interruption");
    // Free again: unpausing reopens it.
    src.set_paused(true);
    src.set_paused(false);
    assert_eq!(events(&mut src), vec![SourceEvent::Resumed]);
    assert_eq!(lock(&mock.script).opened.len(), 2);

    // Revoked in the background: the source ends when the app comes back.
    ctx.set_background(true);
    lock(&mock.script)
        .open_errors
        .push(VideoError::NoPermission);
    ctx.set_background(false);
    let ev = wait_for(WAIT, || {
        let e = src.try_event()?;
        matches!(e, SourceEvent::Ended(_)).then_some(e)
    })
    .expect("ended");
    assert_eq!(ev, SourceEvent::Ended(EndReason::Revoked));
}

#[test]
fn losses_that_end_the_source() {
    let mock = phone(Permission::Granted);
    let (b, _ctx) = start(&mock);
    let mut a = b
        .open_camera(Some("0"), CaptureConstraints::default(), no_wake())
        .expect("a");
    sink_of(&mock, 0).lost(Lost::Disabled);
    assert_eq!(events(&mut a), vec![SourceEvent::Ended(EndReason::Revoked)]);
    a.set_paused(false);
    assert_eq!(
        lock(&mock.script).opened.len(),
        1,
        "an ended camera stays shut"
    );

    let mut c = b
        .open_camera(Some("1"), CaptureConstraints::default(), no_wake())
        .expect("c");
    sink_of(&mock, 1).lost(Lost::Failed(4));
    let ev = events(&mut c);
    assert_eq!(ev.len(), 2, "{ev:?}");
    assert!(matches!(
        ev[0],
        SourceEvent::Error(VideoError::Os { code: 4, .. })
    ));
    assert_eq!(ev[1], SourceEvent::Ended(EndReason::DeviceLost));
}

#[test]
fn screens_are_unsupported_and_shutdown_ends_everything() {
    let mock = phone(Permission::Granted);
    let (b, _ctx) = start(&mock);
    assert_eq!(b.screen_permission(), Permission::NotDetermined);
    assert!(b.screen_sources().expect("list").is_empty());
    let err = b
        .open_screen(
            &ScreenSelection::Listed("display:0".into()),
            CaptureConstraints::default(),
            no_wake(),
        )
        .err()
        .expect("unsupported");
    assert_eq!(err.code(), "unsupported");
    assert!(err.to_string().contains("set_media_projection"), "{err}");
    let (tx, rx) = std::sync::mpsc::channel();
    b.pick_screen(
        PickOptions::default(),
        Box::new(move |r| {
            let _ = tx.send(r);
        }),
    );
    assert_eq!(
        rx.recv_timeout(WAIT)
            .expect("an answer")
            .err()
            .map(|e| e.code()),
        Some("unsupported")
    );

    let mut src = b
        .open_camera(None, CaptureConstraints::default(), no_wake())
        .expect("camera");
    let alive = lock(&mock.script).opened[0].alive.clone();
    b.shutdown();
    assert!(!alive.load(Ordering::Acquire));
    assert_eq!(
        events(&mut src),
        vec![SourceEvent::Ended(EndReason::Closed)]
    );
    assert_eq!(
        b.open_camera(None, CaptureConstraints::default(), no_wake())
            .err(),
        Some(VideoError::ShutDown)
    );
    assert_eq!(b.screen_sources().err(), Some(VideoError::ShutDown));
}

#[test]
fn h264_goes_to_the_platform_encoder() {
    let mock = phone(Permission::Granted);
    let (b, _ctx) = start(&mock);
    let mut enc = b
        .encoder(EncoderConfig::h264(640, 360, 30, 600_000), no_wake())
        .expect("H.264");
    assert_eq!(enc.info().implementation, Implementation::Hardware);
    let f = VideoFrame::filled(PixelFormat::I420, 640, 360, [80, 90, 100], 7).expect("frame");
    enc.submit(&f, EncodeOptions::default()).expect("submit");
    let out = wait_for(WAIT, || enc.poll())
        .expect("output")
        .expect("frame");
    assert!(out.keyframe, "the first frame is a keyframe");
    assert_eq!(out.capture_us, 7);
    assert!(matches!(
        b.decoder(DecoderConfig::new(VideoCodec::H264), no_wake())
            .err(),
        Some(VideoError::Unsupported(_))
    ));
}

/// The H.264 entries `capabilities()` lists, with their names.
fn h264_entries(caps: &Capabilities) -> Vec<(Implementation, String)> {
    caps.codecs
        .iter()
        .filter(|c| c.codec == VideoCodec::H264)
        .map(|c| {
            assert_eq!(c.direction, CodecDirection::Encode, "no H.264 decoder");
            (c.implementation, c.name.clone())
        })
        .collect()
}

#[test]
fn h264_is_listed_as_hardware_only_after_a_real_probe_kept_once() {
    let mock = phone(Permission::Granted);
    let (b, _ctx) = start(&mock);
    assert_eq!(
        mock.probes.load(Ordering::SeqCst),
        0,
        "nothing probed before asked"
    );
    let caps = b.capabilities();
    assert_eq!(
        h264_entries(&caps),
        vec![
            (
                Implementation::Hardware,
                "MediaCodec c2.acme.avc.encoder".to_string()
            ),
            (
                Implementation::Software,
                "MediaCodec c2.android.avc.encoder".to_string()
            ),
        ]
    );
    let best = caps
        .best(VideoCodec::H264, CodecDirection::Encode)
        .expect("H.264");
    assert_eq!(best.implementation, Implementation::Hardware);
    for _ in 0..3 {
        b.capabilities();
    }
    let mut hw = EncoderConfig::h264(640, 360, 30, 600_000);
    hw.implementation = ImplPreference::HardwareOnly;
    let enc = b.encoder(hw, no_wake()).expect("hardware");
    assert_eq!(enc.implementation(), Implementation::Hardware);
    assert_eq!(mock.probes.load(Ordering::SeqCst), 1, "probed once, kept");
    #[cfg(feature = "vp8")]
    assert!(
        caps.best(VideoCodec::Vp8, CodecDirection::Encode).is_some(),
        "VP8 as before"
    );
}

#[test]
fn a_software_only_or_missing_avc_encoder_is_never_listed_as_hardware() {
    // What video/avc resolves to, the platform's software one, the entries.
    type Case = (
        Option<&'static str>,
        Option<&'static str>,
        Vec<Implementation>,
    );
    let cases: [Case; 6] = [
        // The default is the platform's own: software, listed once.
        (
            Some("c2.android.avc.encoder"),
            Some("c2.android.avc.encoder"),
            vec![Implementation::Software],
        ),
        // An older platform's OMX software encoder.
        (
            Some("OMX.google.h264.encoder"),
            None,
            vec![Implementation::Software],
        ),
        // The emulator's.
        (
            Some("c2.goldfish.h264.encoder"),
            Some("c2.android.avc.encoder"),
            vec![Implementation::Software],
        ),
        // A device that can't name its codec (before API 28): not hardware.
        (
            None,
            Some("OMX.google.h264.encoder"),
            vec![Implementation::Software],
        ),
        // No AVC encoder at all: VP8 only.
        (None, None, vec![]),
        // A vendor component marked software.
        (
            Some("OMX.acme.video.encoder.avc.sw"),
            None,
            vec![Implementation::Software],
        ),
    ];
    for (default_name, software, want) in cases {
        let mock = phone(Permission::Granted);
        *lock(&mock.avc) = AvcProbe {
            default_name: default_name.map(Into::into),
            software: software.map(Into::into),
        };
        let (b, _ctx) = start(&mock);
        let caps = b.capabilities();
        let got: Vec<Implementation> = h264_entries(&caps).into_iter().map(|e| e.0).collect();
        assert_eq!(got, want, "{default_name:?} / {software:?}");
        // HardwareOnly is refused before any codec is created.
        let mut hw = EncoderConfig::h264(640, 360, 30, 600_000);
        hw.implementation = ImplPreference::HardwareOnly;
        assert!(
            matches!(
                b.encoder(hw, no_wake()).err(),
                Some(VideoError::Unsupported(_))
            ),
            "{default_name:?}: no hardware to require"
        );
        assert!(lock(&mock.opened_prefs).is_empty());
        #[cfg(feature = "vp8")]
        assert!(caps.best(VideoCodec::Vp8, CodecDirection::Encode).is_some());
    }
}

#[test]
fn the_opened_encoder_reports_what_was_created() {
    let mock = phone(Permission::Granted);
    let (b, _ctx) = start(&mock);
    // video/avc resolves to the software codec (the hardware one is busy),
    // then to one the device can't name.
    lock(&mock.encoders).extend([
        Implementation::Software,
        Implementation::Unknown,
        Implementation::Software,
        Implementation::Unknown,
    ]);
    let mut hw = EncoderConfig::h264(640, 360, 30, 600_000);
    hw.implementation = ImplPreference::HardwareOnly;
    assert_eq!(
        b.encoder(hw, no_wake()).err(),
        Some(VideoError::NoResources)
    );
    assert_eq!(
        b.encoder(hw, no_wake()).err(),
        Some(VideoError::NoResources)
    );
    // Preferring hardware takes what came, and says so.
    let cfg = EncoderConfig::h264(640, 360, 30, 600_000);
    let enc = b.encoder(cfg, no_wake()).expect("software");
    assert_eq!(enc.info().implementation, Implementation::Software);
    assert_eq!(enc.implementation(), Implementation::Software);
    let enc = b.encoder(cfg, no_wake()).expect("unnamed");
    assert_eq!(enc.implementation(), Implementation::Unknown);
    let enc = b.encoder(hw, no_wake()).expect("hardware again");
    assert_eq!(enc.implementation(), Implementation::Hardware);
    assert_eq!(
        *lock(&mock.opened_prefs),
        vec![
            ImplPreference::HardwareOnly,
            ImplPreference::HardwareOnly,
            ImplPreference::PreferHardware,
            ImplPreference::PreferHardware,
            ImplPreference::HardwareOnly,
        ]
    );
}

#[test]
fn codec_names_classify_conservatively() {
    use super::codec::{implementation_of, software_codec_name};
    for sw in [
        "c2.android.avc.encoder",
        "C2.Android.AVC.Encoder",
        "OMX.google.h264.encoder",
        "c2.google.avc.encoder",
        "OMX.ffmpeg.h264.encoder",
        "c2.goldfish.h264.encoder",
        "OMX.acme.video.encoder.avc.sw",
        "c2.acme.avc.encoder.sw",
        "acme.h264.encoder",
        "",
    ] {
        assert!(software_codec_name(sw), "{sw}");
        assert_eq!(
            implementation_of(Some(sw)),
            Implementation::Software,
            "{sw}"
        );
    }
    for hw in [
        "c2.acme.avc.encoder",
        "OMX.acme.video.encoder.avc",
        "c2.globex.h264.encoder",
    ] {
        assert!(!software_codec_name(hw), "{hw}");
        assert_eq!(
            implementation_of(Some(hw)),
            Implementation::Hardware,
            "{hw}"
        );
    }
    assert_eq!(implementation_of(None), Implementation::Unknown);
}

#[cfg(feature = "vp8")]
#[test]
fn camera_frames_round_trip_through_vp8() {
    let mock = phone(Permission::Granted);
    let (b, _ctx) = start(&mock);
    let mut src = b
        .open_camera(None, CaptureConstraints::new(640, 480, 30), no_wake())
        .expect("camera");
    sink_of(&mock, 0).image(&TestImage::new(640, 480).view());
    let frame = next_frame(&mut src);
    let (w, h) = (frame.width, frame.height);
    let mut enc = b
        .encoder(
            EncoderConfig::new(CodecConfig::Vp8, w, h, 30, 800_000),
            no_wake(),
        )
        .expect("VP8 encoder");
    enc.submit(&frame, EncodeOptions::default())
        .expect("submit");
    let encoded = wait_for(WAIT, || enc.poll())
        .expect("output")
        .expect("frame");
    assert!(encoded.keyframe);
    let mut dec = b
        .decoder(DecoderConfig::new(VideoCodec::Vp8), no_wake())
        .expect("VP8 decoder");
    dec.submit(EncodedInput::new(
        VideoCodec::Vp8,
        encoded.data.clone(),
        true,
        encoded.capture_us,
    ))
    .expect("decode");
    let decoded = wait_for(WAIT, || dec.poll())
        .expect("decoded")
        .expect("frame");
    assert_eq!((decoded.width, decoded.height), (w, h));
    let psnr = crate::convert::psnr_y(&frame, &decoded).expect("psnr");
    assert!(psnr > 30.0, "PSNR {psnr:.1} dB");
}

// ───────────────────────────── screens ─────────────────────────────

/// A phone screen of 1080 x 2400 at 420 dpi, handed over as projection
/// "handle" 0x5150 (the mock never dereferences it).
#[allow(unsafe_code)]
fn project(ctx: &AndroidContext) {
    // SAFETY: the scripted platform never uses the handle as a reference.
    unsafe { ctx.set_media_projection(0x5150 as *mut std::ffi::c_void, 1080, 2400, 420) };
}

fn projected(mock: &Mock, i: usize) -> (Arc<dyn ScreenSink>, Sizes, Arc<AtomicBool>) {
    let s = lock(&mock.script);
    let p = &s.projected[i];
    (p.sink.clone(), p.sizes.clone(), p.alive.clone())
}

/// A `w` x `h` RGBA screen image with rows padded to `stride` bytes: the
/// top half white, the bottom half black.
fn screen_image(w: u32, h: u32, stride: usize) -> Vec<u8> {
    let mut data = vec![0x77u8; stride * h as usize];
    for y in 0..h as usize {
        let v = if y < h as usize / 2 { 255 } else { 0 };
        for x in 0..w as usize {
            data[y * stride + x * 4..][..4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    data
}

fn show(sink: &Arc<dyn ScreenSink>, data: &[u8], w: u32, h: u32, stride: usize) {
    sink.image(&ScreenImage::Rgba {
        data,
        width: w,
        height: h,
        row_stride: stride,
        pixel_stride: 4,
    });
}

#[test]
fn a_projection_makes_one_screen_and_capabilities_follow_it() {
    let mock = phone(Permission::Granted);
    let (b, ctx) = start(&mock);
    let events_rx = b.subscribe();
    let caps = b.capabilities().screen;
    assert!(!caps.supported && !caps.list && !caps.frame_in);
    assert!(!ctx.has_media_projection());

    project(&ctx);
    assert!(ctx.has_media_projection());
    assert_eq!(
        events_rx.try_recv(),
        Some(BackendEvent::CapabilitiesChanged)
    );
    let caps = b.capabilities().screen;
    assert!(caps.supported && caps.list, "{caps:?}");
    assert!(!caps.system_picker && !caps.frame_in && !caps.windows);
    assert_eq!(b.screen_permission(), Permission::Granted);
    let sources = b.screen_sources().expect("list");
    assert_eq!(sources.len(), 1);
    assert_eq!(
        (
            sources[0].id.as_str(),
            sources[0].kind,
            sources[0].width,
            sources[0].height
        ),
        (PROJECTION_ID, ScreenKind::Display, 1080, 2400)
    );
    let (tx, rx) = std::sync::mpsc::channel();
    b.pick_screen(
        PickOptions::default(),
        Box::new(move |r| tx.send(r).expect("send")),
    );
    let picked = rx
        .recv_timeout(WAIT)
        .expect("an answer at once")
        .expect("picked");
    assert!(matches!(&picked, ScreenSelection::Listed(id) if id == PROJECTION_ID));
    assert_eq!(
        b.open_screen(
            &ScreenSelection::Listed("display:0".into()),
            CaptureConstraints::default(),
            no_wake()
        )
        .err(),
        Some(VideoError::NotFound)
    );

    // The virtual display fits the asked box, turned upright.
    let mut src = b
        .open_screen(&picked, CaptureConstraints::new(1280, 720, 15), no_wake())
        .expect("screen");
    let (sink, sizes, alive) = projected(&mock, 0);
    assert_eq!(lock(&mock.script).projected[0].projection, 0x5150);
    assert_eq!(*lock(&sizes), vec![(576, 1280, 420)]);

    // Padded RGBA images become tight NV12 frames.
    let img = screen_image(576, 1280, 576 * 4 + 64);
    show(&sink, &img, 576, 1280, 576 * 4 + 64);
    let f = next_frame(&mut src);
    assert_eq!(
        (f.format(), f.width, f.height),
        (PixelFormat::Nv12, 576, 1280)
    );
    let p = f.planes().expect("planes");
    assert_eq!(p.row(0, 0, 576, 1280)[0], 235, "white on top");
    assert_eq!(p.row(0, 1279, 576, 1280)[0], 16, "black below");
    assert_eq!(
        events(&mut src),
        vec![SourceEvent::Started {
            width: 576,
            height: 1280,
            fps: 15
        }]
    );

    // The phone turns: the display follows, and frames come landscape.
    ctx.set_screen_size(2400, 1080, 420);
    assert_eq!(*lock(&sizes), vec![(576, 1280, 420), (1280, 576, 420)]);
    ctx.set_screen_size(2400, 1080, 420);
    assert_eq!(lock(&sizes).len(), 2, "no change, no resize");
    std::thread::sleep(Duration::from_millis(70));
    let img = screen_image(1280, 576, 1280 * 4);
    show(&sink, &img, 1280, 576, 1280 * 4);
    let f = next_frame(&mut src);
    assert_eq!((f.width, f.height), (1280, 576));
    assert!(events(&mut src).contains(&SourceEvent::Started {
        width: 1280,
        height: 576,
        fps: 15
    }));
    // A smaller box resizes the display too.
    src.set_constraints(CaptureConstraints::new(640, 360, 15))
        .expect("constraints");
    assert_eq!(lock(&sizes).last(), Some(&(640, 288, 420)));

    // A bad image is an error event, not an end.
    sink.image(&ScreenImage::Rgba {
        data: &img[..10],
        width: 1280,
        height: 576,
        row_stride: 1280 * 4,
        pixel_stride: 4,
    });
    assert!(matches!(
        events(&mut src).as_slice(),
        [SourceEvent::Error(VideoError::Os { .. })]
    ));

    // Dropping the source releases the display.
    drop(src);
    assert!(!alive.load(Ordering::Acquire));
}

#[test]
fn screens_keep_going_in_the_background_and_end_when_the_projection_stops() {
    let mock = phone(Permission::Granted);
    let (b, ctx) = start(&mock);
    project(&ctx);
    let mut screen = b
        .open_screen(
            &ScreenSelection::Listed(PROJECTION_ID.into()),
            CaptureConstraints::new(1280, 720, 30),
            no_wake(),
        )
        .expect("screen");
    let mut camera = b
        .open_camera(None, CaptureConstraints::new(640, 480, 30), no_wake())
        .expect("camera");
    let (sink, _sizes, alive) = projected(&mock, 0);
    let events_rx = b.subscribe();

    ctx.set_background(true);
    assert_eq!(
        events(&mut camera),
        vec![SourceEvent::Interrupted(Interruption::Background)],
        "the camera pauses"
    );
    assert!(alive.load(Ordering::Acquire), "the screen's display stays");
    let img = screen_image(576, 1280, 576 * 4);
    show(&sink, &img, 576, 1280, 576 * 4);
    let f = next_frame(&mut screen);
    assert_eq!((f.width, f.height), (576, 1280), "frames in the background");
    assert!(
        !events(&mut screen)
            .iter()
            .any(|e| matches!(e, SourceEvent::Interrupted(_))),
        "a screen share isn't interrupted by the background"
    );
    // A second screen can open in the background too.
    let second = b.open_screen(
        &ScreenSelection::Listed(PROJECTION_ID.into()),
        CaptureConstraints::default(),
        no_wake(),
    );
    assert!(second.is_ok(), "{:?}", second.err());
    drop(second);

    // The person stops sharing from the status bar: onStop.
    while events_rx.try_recv().is_some() {}
    ctx.media_projection_stopped();
    assert!(!alive.load(Ordering::Acquire), "the display is released");
    assert_eq!(
        events(&mut screen),
        vec![SourceEvent::Ended(EndReason::SourceClosed)]
    );
    assert_eq!(
        events_rx.try_recv(),
        Some(BackendEvent::CapabilitiesChanged)
    );
    assert!(!b.capabilities().screen.supported);
    assert!(!ctx.has_media_projection());
    assert!(b.screen_sources().expect("list").is_empty());
    ctx.media_projection_stopped();
    assert!(events(&mut screen).is_empty(), "ended once");
    assert_eq!(
        b.open_screen(
            &ScreenSelection::Listed(PROJECTION_ID.into()),
            CaptureConstraints::default(),
            no_wake()
        )
        .err()
        .map(|e| e.code()),
        Some("unsupported")
    );

    // A new projection works again; shutdown ends its screen as Closed.
    project(&ctx);
    let mut again = b
        .open_screen(
            &ScreenSelection::Listed(PROJECTION_ID.into()),
            CaptureConstraints::default(),
            no_wake(),
        )
        .expect("screen");
    let (_, sizes, alive) = projected(&mock, 2);
    assert_eq!(*lock(&sizes), vec![(1080, 2400, 420)], "no box: full size");
    b.shutdown();
    assert!(!alive.load(Ordering::Acquire));
    assert_eq!(
        events(&mut again),
        vec![SourceEvent::Ended(EndReason::Closed)]
    );
}

#[test]
#[allow(unsafe_code)]
fn a_failed_virtual_display_is_an_error_and_yuv_screens_work() {
    let mock = phone(Permission::Granted);
    let (b, ctx) = start(&mock);
    project(&ctx);
    lock(&mock.script)
        .open_errors
        .push(VideoError::os(0, "createVirtualDisplay threw"));
    let sel = ScreenSelection::Listed(PROJECTION_ID.into());
    assert!(matches!(
        b.open_screen(&sel, CaptureConstraints::default(), no_wake()),
        Err(VideoError::Os { .. })
    ));
    let mut src = b
        .open_screen(&sel, CaptureConstraints::default(), no_wake())
        .expect("screen");
    let (sink, _, _) = projected(&mock, 0);
    let img = TestImage::new(64, 48);
    sink.image(&ScreenImage::Yuv(img.view()));
    let f = next_frame(&mut src);
    assert_eq!((f.format(), f.width, f.height), (PixelFormat::Nv12, 64, 48));
    // A zero-sized display is a caller's mistake.
    ctx.media_projection_stopped();
    // SAFETY: the scripted platform never uses the handle.
    unsafe { ctx.set_media_projection(0x5150 as *mut std::ffi::c_void, 0, 0, 0) };
    assert!(matches!(
        b.open_screen(&sel, CaptureConstraints::default(), no_wake()),
        Err(VideoError::Invalid(_))
    ));
}

/// A frame-in source on a phone: frames keep flowing while the app is in
/// the background, which pauses the camera.
#[test]
fn frame_in_sources_ignore_the_background() {
    let mock = phone(Permission::Granted);
    let (b, ctx) = start(&mock);
    let mut camera = b
        .open_camera(None, CaptureConstraints::new(640, 480, 30), no_wake())
        .expect("camera");
    let (tx, mut rx) =
        crate::frame_in(crate::FrameInOptions::new(1280, 720, 15), no_wake()).expect("frame_in");
    ctx.set_background(true);
    assert_eq!(
        events(&mut camera),
        vec![SourceEvent::Interrupted(Interruption::Background)]
    );
    assert!(tx.wants_frames(), "the background doesn't pause frame-in");
    let f = crate::fake::pattern_frame(640, 360, 3, 0).expect("frame");
    tx.push(f, crate::Rotation::Deg0).expect("push");
    let got = rx.try_recv().expect("a frame in the background");
    assert_eq!(crate::fake::read_frame_code(&got), Some(3));
    assert!(
        !std::iter::from_fn(|| rx.try_event()).any(|e| matches!(e, SourceEvent::Interrupted(_)))
    );
    drop(b);
}
