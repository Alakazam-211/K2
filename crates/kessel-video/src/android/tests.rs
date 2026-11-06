//! The Android backend over a scripted [`Platform`]: cameras, permission,
//! upright frames, the app going to the background and back, cameras taken
//! by other apps, screens, and VP8 through the backend. Runs on any host.

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

#[derive(Default)]
struct Script {
    /// The next opens fail with these, in order.
    open_errors: Vec<VideoError>,
    opened: Vec<Opened>,
}

struct Mock {
    cams: Vec<CamDesc>,
    permission: Permission,
    script: Mutex<Script>,
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

/// A fake H.264 core: one byte per frame, the keyframe flag kept.
struct MockH264;

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
        Implementation::Hardware
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

    fn h264_encode_capability(&self) -> Option<CodecCapability> {
        Some(CodecCapability::new(
            VideoCodec::H264,
            CodecDirection::Encode,
            Implementation::Hardware,
            "mock MediaCodec",
            1920,
            1080,
            30,
        ))
    }

    fn h264_encoder(&self, _cfg: &EncoderConfig) -> Result<Box<dyn EncoderCore>, VideoError> {
        Ok(Box::new(MockH264))
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
    assert_eq!(b.screen_permission(), Permission::Denied);
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
    assert!(err.to_string().contains("MediaProjection"), "{err}");
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
