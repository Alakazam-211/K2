//! macOS backend tests. The codec tests run on any Mac (VideoToolbox has a
//! software encoder when there's no hardware one). Real cameras and screens
//! need a person (permission prompts) and hardware, so those run by hand.

use super::*;
use crate::api::no_wake;
use crate::fake::{pattern_frame, read_frame_code};
use crate::h264::{annexb_nals, nal_type, profile_level_id, NAL_IDR, NAL_PPS, NAL_SPS};
use crate::kit::wait_for;

fn encode_one(enc: &mut Box<dyn VideoEncoder>, frame: &VideoFrame, opts: EncodeOptions) -> Option<EncodedFrame> {
    let dropped = enc.stats().dropped_by_rc;
    enc.submit(frame, opts).expect("submit");
    let r = wait_for(Duration::from_secs(5), || match enc.poll() {
        Some(r) => Some(Some(r)),
        None if enc.stats().dropped_by_rc > dropped => Some(None),
        None => None,
    })
    .expect("an output or a rate-control drop within 5 s");
    match r {
        Some(Ok(f)) => Some(f),
        Some(Err(e)) => panic!("encode failed: {e}"),
        None => None,
    }
}

/// Every Annex B start code is the 4-byte form, and a keyframe is SPS, PPS,
/// then the IDR (SEI allowed), with nothing but parameter sets before SPS.
fn check_bitstream(f: &EncodedFrame) {
    assert!(f.data.starts_with(&[0, 0, 0, 1]), "starts with a 4-byte start code");
    let threes = f.data.windows(3).filter(|w| *w == [0, 0, 1]).count();
    let fours = f.data.windows(4).filter(|w| *w == [0, 0, 0, 1]).count();
    assert_eq!(threes, fours, "only 4-byte start codes");
    let nals = annexb_nals(&f.data);
    let types: Vec<u8> = nals.iter().filter_map(|n| nal_type(n)).collect();
    assert_eq!(f.keyframe, types.contains(&NAL_IDR), "keyframe flag matches the IDR: {types:?}");
    if f.keyframe {
        let sps = types.iter().position(|t| *t == NAL_SPS).expect("SPS in band");
        let pps = types.iter().position(|t| *t == NAL_PPS).expect("PPS in band");
        let idr = types.iter().position(|t| *t == NAL_IDR).expect("IDR");
        assert!(sps < pps && pps < idr, "SPS, PPS, then IDR: {types:?}");
    }
}

#[test]
fn videotoolbox_h264_follows_the_bitstream_rules() {
    let b = backend().expect("backend");
    let caps = b.capabilities();
    eprintln!("[apple] {} codecs: {:?}", caps.os, caps.codecs.iter().map(|c| (&c.name, c.implementation)).collect::<Vec<_>>());
    let best = caps.best(VideoCodec::H264, CodecDirection::Encode).expect("an H.264 encoder");
    let mut enc = b.encoder(EncoderConfig::h264(640, 360, 30, 800_000), no_wake()).expect("encoder");
    let info = enc.info();
    eprintln!("[apple] encoder: {} ({:?}); best listed {:?}", info.name, info.implementation, best.implementation);

    let mut outs = Vec::new();
    for n in 0..40u32 {
        let f = pattern_frame(1280, 720, n, 10_000 + n as u64 * 33_333).expect("frame");
        if n == 20 {
            enc.request_keyframe();
        }
        if n == 10 {
            enc.set_rates(Rates::new(400_000, 30)).expect("rates");
        }
        if let Some(e) = encode_one(&mut enc, &f, EncodeOptions::default()) {
            outs.push((n, e));
        }
    }
    assert!(outs.len() >= 36, "rate control dropped too much: {} of 40", outs.len());
    for (n, e) in &outs {
        check_bitstream(e);
        assert_eq!(e.capture_us, 10_000 + *n as u64 * 33_333, "capture_us passes through");
        assert_eq!((e.width, e.height), (640, 360), "scaled to the coded size");
    }
    let (first_n, first) = &outs[0];
    assert_eq!(*first_n, 0);
    assert!(first.keyframe, "the first output is a keyframe");
    let sps = annexb_nals(&first.data).into_iter().find(|n| nal_type(n) == Some(NAL_SPS)).expect("SPS");
    let pli = profile_level_id(sps).expect("profile-level-id");
    assert!(pli.starts_with("42"), "Constrained Baseline: {pli}");
    assert!(u8::from_str_radix(&pli[2..4], 16).expect("hex") & 0x40 != 0, "constraint_set1 on: {pli}");
    let keys: Vec<u32> = outs.iter().filter(|(_, e)| e.keyframe).map(|(n, _)| *n).collect();
    assert!(!keys.iter().any(|n| (10..=12).contains(n)), "set_rates forced a keyframe: {keys:?}");
    assert!(keys.iter().any(|n| (20..=21).contains(n)), "no keyframe within 2 outputs of the request: {keys:?}");
    assert_eq!(keys.len(), 2, "only the first and the requested keyframes: {keys:?}");
    let st = enc.stats();
    assert_eq!(st.frames_in, 40);
    assert_eq!(st.keyframes, 2);
}

#[test]
fn videotoolbox_resizes_and_takes_every_input_format() {
    let b = backend().expect("backend");
    let mut enc = b.encoder(EncoderConfig::h264(320, 180, 30, 300_000), no_wake()).expect("encoder");
    for (n, fmt) in [PixelFormat::I420, PixelFormat::Nv12, PixelFormat::Bgra, PixelFormat::Rgba].into_iter().enumerate() {
        let src = pattern_frame(640, 360, n as u32, n as u64 * 33_333).expect("frame");
        let f = crate::convert::convert(&src, 640, 360, fmt).expect("convert");
        let e = encode_one(&mut enc, &f, EncodeOptions::default()).expect("not dropped");
        check_bitstream(&e);
    }
    enc.set_size(480, 270).expect("set_size");
    let e = encode_one(&mut enc, &pattern_frame(480, 270, 9, 999_999).expect("frame"), EncodeOptions::default()).expect("out");
    assert!(e.keyframe, "a new size starts with a keyframe");
    assert_eq!((e.width, e.height), (480, 270));
    let k = encode_one(&mut enc, &pattern_frame(480, 270, 10, 1_033_332).expect("frame"), EncodeOptions::keyframe()).expect("out");
    assert!(k.keyframe, "EncodeOptions::keyframe");
    assert_eq!(enc.info().config.width, 480);
}

#[test]
fn videotoolbox_holds_the_bitrate_ceiling() {
    let b = backend().expect("backend");
    let mut cfg = EncoderConfig::h264(640, 360, 30, 500_000);
    cfg.max_bitrate_bps = 600_000;
    let mut enc = b.encoder(cfg, no_wake()).expect("encoder");
    let mut sizes = Vec::new();
    for n in 0..150u32 {
        // A busy picture: the bar moves fast and the code changes.
        let f = pattern_frame(640, 360, n * 7, n as u64 * 33_333).expect("frame");
        if let Some(e) = encode_one(&mut enc, &f, EncodeOptions::default()) {
            sizes.push(e.data.len() as u64);
        } else {
            sizes.push(0);
        }
    }
    // Skip the first second (the keyframe and rate control settling).
    let worst = sizes[30..].windows(30).map(|w| w.iter().sum::<u64>() * 8).max().expect("windows");
    eprintln!("[apple] worst 1 s window {worst} bit/s against a 600000 ceiling");
    assert!(worst <= 660_000, "a one-second window went over the ceiling by more than 10%: {worst}");
}

#[test]
fn the_encoder_answers_busy_instead_of_blocking() {
    let b = backend().expect("backend");
    let mut enc = b.encoder(EncoderConfig::h264(1280, 720, 30, 2_000_000), no_wake()).expect("encoder");
    let f = pattern_frame(1280, 720, 1, 1).expect("frame");
    let t = std::time::Instant::now();
    let mut busy = 0;
    for _ in 0..50 {
        if enc.submit(&f, EncodeOptions::default()) == Err(VideoError::Busy) {
            busy += 1;
        }
    }
    assert!(t.elapsed() < Duration::from_millis(500), "submit blocked: {:?}", t.elapsed());
    assert!(busy > 0, "50 quick submits with nobody polling were never busy");
}

#[test]
fn pixel_buffers_round_trip_nv12() {
    let f = pattern_frame(64, 48, 0xa5a5_5a5a, 0).expect("frame");
    let nv12 = crate::convert::convert(&f, 64, 48, PixelFormat::Nv12).expect("nv12");
    let planes = nv12.planes().expect("planes").clone();
    let attrs = ffi::pixel_buffer_attributes(ffi::PIX_420V, None);
    let mut pb: ffi::CVPixelBufferRef = std::ptr::null_mut();
    // SAFETY: valid attributes and an out pointer.
    let st = unsafe { ffi::CVPixelBufferCreate(std::ptr::null(), 64, 48, ffi::PIX_420V, attrs.0, &mut pb) };
    assert_eq!(st, 0, "CVPixelBufferCreate");
    let owned = ffi::Cf(pb as ffi::CFTypeRef);
    pixel::fill_nv12(pb, &planes, 64, 48).expect("fill");
    pixel::tag_color(pb, crate::frame::ColorSpace::Bt709Limited);
    // SAFETY: a live pixel buffer.
    let back = unsafe { pixel::cpu_frame(pb, 7) }.expect("copy out");
    assert_eq!(back.planes(), Some(&planes));
    assert_eq!(back.color, crate::frame::ColorSpace::Bt709Limited);
    // SAFETY: a live pixel buffer.
    let native = unsafe { pixel::native_frame(pb, 8) };
    assert_eq!(native.format(), PixelFormat::Native);
    assert_eq!(read_frame_code(&native), Some(0xa5a5_5a5a), "a native frame reads back through to_planes");
    drop(owned);

    // A native frame goes into the encoder without a copy.
    let b = backend().expect("backend");
    let mut enc = b.encoder(EncoderConfig::h264(64, 48, 30, 100_000), no_wake()).expect("encoder");
    let e = encode_one(&mut enc, &native, EncodeOptions::default()).expect("out");
    check_bitstream(&e);
    assert_eq!(e.capture_us, 8);
}

#[test]
fn listing_and_permissions_never_prompt() {
    let b = backend().expect("backend");
    let cams = b.cameras();
    eprintln!("[apple] {} cameras: {:?}", cams.len(), cams.iter().map(|c| (&c.name, c.facing, c.modes.len())).collect::<Vec<_>>());
    for c in &cams {
        assert!(!c.id.is_empty());
    }
    let p = b.camera_permission();
    eprintln!("[apple] camera permission {p:?}, screen permission {:?}", b.screen_permission());
    // A test binary has no NSCameraUsageDescription, so it can't ask: never
    // NotDetermined (asking would end the process).
    assert_ne!(p, Permission::NotDetermined);
    let caps = b.capabilities();
    assert!(caps.os.starts_with("macOS "), "{}", caps.os);
    assert!(caps.camera);
    let events = b.subscribe();
    assert!(events.try_recv().is_none());
    b.shutdown();
    assert_eq!(b.screen_sources().err(), Some(VideoError::ShutDown));
    assert!(matches!(b.decoder(DecoderConfig::new(VideoCodec::H264), no_wake()).err(), Some(VideoError::ShutDown)));
}

/// By hand on a Mac with a camera, from a binary with
/// NSCameraUsageDescription and the camera allowed:
/// `cargo test -p kessel-video real_camera -- --ignored --nocapture`.
#[test]
#[ignore = "needs a camera and the camera permission"]
fn real_camera_delivers_frames() {
    let b = backend().expect("backend");
    assert_eq!(b.camera_permission(), Permission::Granted, "allow the camera first");
    let mut src = b.open_camera(None, CaptureConstraints::new(1280, 720, 30), no_wake()).expect("camera");
    let f = wait_for(Duration::from_secs(5), || src.try_recv()).expect("a frame in 5 s");
    eprintln!("[apple] camera frame {}x{} {:?}", f.width, f.height, f.format());
    let ev = wait_for(Duration::from_secs(1), || src.try_event()).expect("Started");
    assert!(matches!(ev, SourceEvent::Started { .. }), "{ev:?}");
    let mut enc = b.encoder(EncoderConfig::h264(640, 360, 30, 800_000), no_wake()).expect("encoder");
    check_bitstream(&encode_one(&mut enc, &f, EncodeOptions::default()).expect("out"));
}

/// By hand on a Mac with Screen Recording allowed for the test binary.
#[test]
#[ignore = "needs a GUI session and the Screen Recording permission"]
fn real_screen_delivers_frames() {
    let b = backend().expect("backend");
    let list = b.screen_sources().expect("sources");
    eprintln!("[apple] screens: {:?}", list.iter().map(|s| (&s.id, &s.title, s.width, s.height)).collect::<Vec<_>>());
    let display = list.iter().find(|s| s.kind == ScreenKind::Display).expect("a display");
    let mut c = CaptureConstraints::new(1280, 720, 15);
    c.exclude_own_windows = true;
    let mut src = b.open_screen(&ScreenSelection::Listed(display.id.clone()), c, no_wake()).expect("screen");
    let f = wait_for(Duration::from_secs(5), || src.try_recv()).expect("a frame in 5 s");
    assert!(f.width <= 1280 && f.height <= 720);
}
