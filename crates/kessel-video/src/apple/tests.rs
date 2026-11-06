//! Apple backend tests, for macOS and iOS. The codec tests run on any Mac
//! (VideoToolbox has a software encoder when there's no hardware one) and in
//! the iOS simulator. Real cameras and screens need a person (permission
//! prompts) and hardware, so those run by hand.

use super::*;
use crate::api::no_wake;
use crate::error::DecodeError;
use crate::fake::{pattern_frame, read_frame_code};
use crate::h264::{annexb_nals, nal_type, profile_level_id, NAL_IDR, NAL_PPS, NAL_SPS};
use crate::kit::wait_for;

fn encode_one(
    enc: &mut Box<dyn VideoEncoder>,
    frame: &VideoFrame,
    opts: EncodeOptions,
) -> Option<EncodedFrame> {
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
    assert!(
        f.data.starts_with(&[0, 0, 0, 1]),
        "starts with a 4-byte start code"
    );
    let threes = f.data.windows(3).filter(|w| *w == [0, 0, 1]).count();
    let fours = f.data.windows(4).filter(|w| *w == [0, 0, 0, 1]).count();
    assert_eq!(threes, fours, "only 4-byte start codes");
    let nals = annexb_nals(&f.data);
    let types: Vec<u8> = nals.iter().filter_map(|n| nal_type(n)).collect();
    assert_eq!(
        f.keyframe,
        types.contains(&NAL_IDR),
        "keyframe flag matches the IDR: {types:?}"
    );
    if f.keyframe {
        let sps = types
            .iter()
            .position(|t| *t == NAL_SPS)
            .expect("SPS in band");
        let pps = types
            .iter()
            .position(|t| *t == NAL_PPS)
            .expect("PPS in band");
        let idr = types.iter().position(|t| *t == NAL_IDR).expect("IDR");
        assert!(sps < pps && pps < idr, "SPS, PPS, then IDR: {types:?}");
    }
}

#[test]
fn videotoolbox_h264_follows_the_bitstream_rules() {
    let b = backend().expect("backend");
    let caps = b.capabilities();
    eprintln!(
        "[apple] {} codecs: {:?}",
        caps.os,
        caps.codecs
            .iter()
            .map(|c| (&c.name, c.implementation))
            .collect::<Vec<_>>()
    );
    let best = caps
        .best(VideoCodec::H264, CodecDirection::Encode)
        .expect("an H.264 encoder");
    let mut enc = b
        .encoder(EncoderConfig::h264(640, 360, 30, 800_000), no_wake())
        .expect("encoder");
    let info = enc.info();
    eprintln!(
        "[apple] encoder: {} ({:?}); best listed {:?}",
        info.name, info.implementation, best.implementation
    );

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
    assert!(
        outs.len() >= 36,
        "rate control dropped too much: {} of 40",
        outs.len()
    );
    for (n, e) in &outs {
        check_bitstream(e);
        assert_eq!(
            e.capture_us,
            10_000 + *n as u64 * 33_333,
            "capture_us passes through"
        );
        assert_eq!((e.width, e.height), (640, 360), "scaled to the coded size");
    }
    let (first_n, first) = &outs[0];
    assert_eq!(*first_n, 0);
    assert!(first.keyframe, "the first output is a keyframe");
    let sps = annexb_nals(&first.data)
        .into_iter()
        .find(|n| nal_type(n) == Some(NAL_SPS))
        .expect("SPS");
    let pli = profile_level_id(sps).expect("profile-level-id");
    assert!(pli.starts_with("42"), "Constrained Baseline: {pli}");
    assert!(
        u8::from_str_radix(&pli[2..4], 16).expect("hex") & 0x40 != 0,
        "constraint_set1 on: {pli}"
    );
    let keys: Vec<u32> = outs
        .iter()
        .filter(|(_, e)| e.keyframe)
        .map(|(n, _)| *n)
        .collect();
    assert!(
        !keys.iter().any(|n| (10..=12).contains(n)),
        "set_rates forced a keyframe: {keys:?}"
    );
    assert!(
        keys.iter().any(|n| (20..=21).contains(n)),
        "no keyframe within 2 outputs of the request: {keys:?}"
    );
    assert_eq!(
        keys.len(),
        2,
        "only the first and the requested keyframes: {keys:?}"
    );
    let st = enc.stats();
    assert_eq!(st.frames_in, 40);
    assert_eq!(st.keyframes, 2);
}

#[test]
fn videotoolbox_resizes_and_takes_every_input_format() {
    let b = backend().expect("backend");
    let mut enc = b
        .encoder(EncoderConfig::h264(320, 180, 30, 300_000), no_wake())
        .expect("encoder");
    for (n, fmt) in [
        PixelFormat::I420,
        PixelFormat::Nv12,
        PixelFormat::Bgra,
        PixelFormat::Rgba,
    ]
    .into_iter()
    .enumerate()
    {
        let src = pattern_frame(640, 360, n as u32, n as u64 * 33_333).expect("frame");
        let f = crate::convert::convert(&src, 640, 360, fmt).expect("convert");
        let e = encode_one(&mut enc, &f, EncodeOptions::default()).expect("not dropped");
        check_bitstream(&e);
    }
    enc.set_size(480, 270).expect("set_size");
    let e = encode_one(
        &mut enc,
        &pattern_frame(480, 270, 9, 999_999).expect("frame"),
        EncodeOptions::default(),
    )
    .expect("out");
    assert!(e.keyframe, "a new size starts with a keyframe");
    assert_eq!((e.width, e.height), (480, 270));
    let k = encode_one(
        &mut enc,
        &pattern_frame(480, 270, 10, 1_033_332).expect("frame"),
        EncodeOptions::keyframe(),
    )
    .expect("out");
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
    let worst = sizes[30..]
        .windows(30)
        .map(|w| w.iter().sum::<u64>() * 8)
        .max()
        .expect("windows");
    eprintln!("[apple] worst 1 s window {worst} bit/s against a 600000 ceiling");
    assert!(
        worst <= 660_000,
        "a one-second window went over the ceiling by more than 10%: {worst}"
    );
}

#[test]
fn the_encoder_answers_busy_instead_of_blocking() {
    let b = backend().expect("backend");
    let mut enc = b
        .encoder(EncoderConfig::h264(1280, 720, 30, 2_000_000), no_wake())
        .expect("encoder");
    let f = pattern_frame(1280, 720, 1, 1).expect("frame");
    let t = std::time::Instant::now();
    let mut busy = 0;
    for _ in 0..50 {
        if enc.submit(&f, EncodeOptions::default()) == Err(VideoError::Busy) {
            busy += 1;
        }
    }
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "submit blocked: {:?}",
        t.elapsed()
    );
    assert!(
        busy > 0,
        "50 quick submits with nobody polling were never busy"
    );
}

#[test]
fn pixel_buffers_round_trip_nv12() {
    let f = pattern_frame(64, 48, 0xa5a5_5a5a, 0).expect("frame");
    let nv12 = crate::convert::convert(&f, 64, 48, PixelFormat::Nv12).expect("nv12");
    let planes = nv12.planes().expect("planes").clone();
    let attrs = ffi::pixel_buffer_attributes(ffi::PIX_420V, None);
    let mut pb: ffi::CVPixelBufferRef = std::ptr::null_mut();
    // SAFETY: valid attributes and an out pointer.
    let st = unsafe {
        ffi::CVPixelBufferCreate(std::ptr::null(), 64, 48, ffi::PIX_420V, attrs.0, &mut pb)
    };
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
    assert_eq!(
        read_frame_code(&native),
        Some(0xa5a5_5a5a),
        "a native frame reads back through to_planes"
    );
    drop(owned);

    // A native frame goes into the encoder without a copy.
    let b = backend().expect("backend");
    let mut enc = b
        .encoder(EncoderConfig::h264(64, 48, 30, 100_000), no_wake())
        .expect("encoder");
    let e = encode_one(&mut enc, &native, EncodeOptions::default()).expect("out");
    check_bitstream(&e);
    assert_eq!(e.capture_us, 8);
}

/// A VideoToolbox-decoded native frame hands out its CVPixelBuffer (for an
/// AVSampleBufferDisplayLayer); another kind of native buffer doesn't.
#[test]
fn decoded_native_frames_hand_out_their_pixel_buffer() {
    let b = backend().expect("backend");
    let mut enc = b
        .encoder(EncoderConfig::h264(320, 240, 30, 400_000), no_wake())
        .expect("encoder");
    let mut cfg = DecoderConfig::new(VideoCodec::H264);
    cfg.output = PixelFormat::Native;
    let mut dec = b.decoder(cfg, no_wake()).expect("decoder");
    let f = pattern_frame(320, 240, 0x00c0_ffee, 1_000).expect("frame");
    let e = encode_one(&mut enc, &f, EncodeOptions::default()).expect("a keyframe");
    let out = decode_one(&mut dec, h264_input(&e));
    let crate::frame::FrameBuffer::Native(native) = &out.buffer else {
        panic!("asked for a native frame, got {:?}", out.format());
    };
    let pb = crate::apple_pixel_buffer(native).expect("a CVPixelBuffer");
    assert!(!pb.is_null());
    // SAFETY: `pb` is live while `native` is.
    let (w, h) = unsafe {
        (
            ffi::CVPixelBufferGetWidth(pb),
            ffi::CVPixelBufferGetHeight(pb),
        )
    };
    assert_eq!((w, h), (320, 240));
    assert_eq!(read_frame_code(&out), Some(0x00c0_ffee));

    struct Other;
    impl crate::frame::NativeFrame for Other {
        fn format(&self) -> PixelFormat {
            PixelFormat::Nv12
        }
        fn to_planes(&self, _: PixelFormat) -> Result<crate::frame::Planes, VideoError> {
            Err(VideoError::Unsupported("test".into()))
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }
    let other = crate::frame::NativeBuffer(std::sync::Arc::new(Other));
    assert_eq!(crate::apple_pixel_buffer(&other), None);
}

/// A pixel buffer the host already has, pushed through a frame-in source:
/// upright it stays native and VideoToolbox takes it without a copy;
/// turned, it is read back and turned; VP8 takes either.
#[test]
fn host_pixel_buffers_go_through_frame_in_to_the_encoders() {
    let upright = pattern_frame(64, 48, 0x0bad_cafe, 0).expect("frame");
    let make = |f: &VideoFrame| {
        let nv12 = crate::convert::convert(f, f.width, f.height, PixelFormat::Nv12).expect("nv12");
        let attrs = ffi::pixel_buffer_attributes(ffi::PIX_420V, None);
        let mut pb: ffi::CVPixelBufferRef = std::ptr::null_mut();
        // SAFETY: valid attributes and an out pointer.
        let st = unsafe {
            ffi::CVPixelBufferCreate(
                std::ptr::null(),
                f.width as usize,
                f.height as usize,
                ffi::PIX_420V,
                attrs.0,
                &mut pb,
            )
        };
        assert_eq!(st, 0, "CVPixelBufferCreate");
        pixel::fill_nv12(pb, nv12.planes().expect("planes"), f.width, f.height).expect("fill");
        ffi::Cf(pb as ffi::CFTypeRef)
    };
    let straight = make(&upright);
    let sideways = make(&crate::frame_in::turn(&upright, 270).expect("turn"));
    // SAFETY: live pixel buffers, held by `straight` and `sideways`.
    let (a, b2) = unsafe {
        (
            pixel::pixel_buffer_frame(straight.0 as *mut std::ffi::c_void, 1_000).expect("frame"),
            pixel::pixel_buffer_frame(sideways.0 as *mut std::ffi::c_void, 40_000).expect("frame"),
        )
    };
    // SAFETY: a null pointer is refused before any use.
    assert!(unsafe { pixel::pixel_buffer_frame(std::ptr::null_mut(), 0) }.is_err());
    drop((straight, sideways));

    let (tx, mut rx) =
        crate::frame_in(crate::FrameInOptions::default(), no_wake()).expect("frame_in");
    tx.push(a, crate::Rotation::Deg0).expect("push");
    let native = wait_for(Duration::from_secs(1), || rx.try_recv()).expect("frame");
    assert_eq!(native.format(), PixelFormat::Native, "upright stays native");
    tx.push(b2, crate::Rotation::Deg90).expect("push");
    let turned = wait_for(Duration::from_secs(1), || rx.try_recv()).expect("frame");
    assert_eq!((turned.width, turned.height), (64, 48));
    assert_eq!(read_frame_code(&turned), Some(0x0bad_cafe));

    let be = backend().expect("backend");
    let mut h264 = be
        .encoder(EncoderConfig::h264(64, 48, 30, 100_000), no_wake())
        .expect("encoder");
    let e = encode_one(&mut h264, &native, EncodeOptions::default()).expect("out");
    check_bitstream(&e);
    assert_eq!(e.capture_us, 1_000);
    #[cfg(feature = "vp8")]
    {
        let mut vp8 = be
            .encoder(
                EncoderConfig::new(CodecConfig::Vp8, 64, 48, 30, 200_000),
                no_wake(),
            )
            .expect("vp8");
        let e = encode_one(&mut vp8, &turned, EncodeOptions::default()).expect("out");
        assert!(e.keyframe && e.capture_us == 40_000);
    }
}

#[test]
fn listing_and_permissions_never_prompt() {
    let b = backend().expect("backend");
    let cams = b.cameras();
    eprintln!(
        "[apple] {} cameras: {:?}",
        cams.len(),
        cams.iter()
            .map(|c| (&c.name, c.facing, c.modes.len()))
            .collect::<Vec<_>>()
    );
    for c in &cams {
        assert!(!c.id.is_empty());
    }
    let p = b.camera_permission();
    eprintln!(
        "[apple] camera permission {p:?}, screen permission {:?}",
        b.screen_permission()
    );
    // A test binary has no NSCameraUsageDescription, so it can't ask: never
    // NotDetermined (asking would end the process).
    assert_ne!(p, Permission::NotDetermined);
    let caps = b.capabilities();
    let os = if cfg!(target_os = "ios") {
        "iOS "
    } else {
        "macOS "
    };
    assert!(caps.os.starts_with(os), "{}", caps.os);
    assert!(caps.camera);
    let events = b.subscribe();
    assert!(events.try_recv().is_none());
    b.shutdown();
    assert_eq!(b.screen_sources().err(), Some(VideoError::ShutDown));
    assert!(matches!(
        b.decoder(DecoderConfig::new(VideoCodec::H264), no_wake())
            .err(),
        Some(VideoError::ShutDown)
    ));
}

/// By hand on a Mac with a camera, from a binary with
/// NSCameraUsageDescription and the camera allowed:
/// `cargo test -p kessel-video real_camera -- --ignored --nocapture`.
#[test]
#[ignore = "needs a camera and the camera permission"]
fn real_camera_delivers_frames() {
    let b = backend().expect("backend");
    assert_eq!(
        b.camera_permission(),
        Permission::Granted,
        "allow the camera first"
    );
    let mut src = b
        .open_camera(None, CaptureConstraints::new(1280, 720, 30), no_wake())
        .expect("camera");
    let f = wait_for(Duration::from_secs(5), || src.try_recv()).expect("a frame in 5 s");
    eprintln!(
        "[apple] camera frame {}x{} {:?}",
        f.width,
        f.height,
        f.format()
    );
    let ev = wait_for(Duration::from_secs(1), || src.try_event()).expect("Started");
    assert!(matches!(ev, SourceEvent::Started { .. }), "{ev:?}");
    let mut enc = b
        .encoder(EncoderConfig::h264(640, 360, 30, 800_000), no_wake())
        .expect("encoder");
    check_bitstream(&encode_one(&mut enc, &f, EncodeOptions::default()).expect("out"));
}

/// By hand on a Mac with Screen Recording allowed for the test binary.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "needs a GUI session and the Screen Recording permission"]
fn real_screen_delivers_frames() {
    let b = backend().expect("backend");
    let list = b.screen_sources().expect("sources");
    eprintln!(
        "[apple] screens: {:?}",
        list.iter()
            .map(|s| (&s.id, &s.title, s.width, s.height))
            .collect::<Vec<_>>()
    );
    let display = list
        .iter()
        .find(|s| s.kind == ScreenKind::Display)
        .expect("a display");
    let mut c = CaptureConstraints::new(1280, 720, 15);
    c.exclude_own_windows = true;
    let mut src = b
        .open_screen(&ScreenSelection::Listed(display.id.clone()), c, no_wake())
        .expect("screen");
    let f = wait_for(Duration::from_secs(5), || src.try_recv()).expect("a frame in 5 s");
    assert!(f.width <= 1280 && f.height <= 720);
}

// ───────────────────────────── decoding ─────────────────────────────

fn h264_input(e: &EncodedFrame) -> EncodedInput {
    EncodedInput::new(VideoCodec::H264, e.data.clone(), e.keyframe, e.capture_us)
}

/// Submit one input and wait for its decoded frame.
fn decode_one(dec: &mut Box<dyn VideoDecoder>, input: EncodedInput) -> VideoFrame {
    dec.submit(input).expect("decode submit");
    wait_for(Duration::from_secs(5), || dec.poll())
        .expect("a decoded frame within 5 s")
        .expect("decode")
}

#[test]
fn videotoolbox_h264_round_trips_through_the_decoder() {
    let b = backend().expect("backend");
    let caps = b.capabilities();
    let d = caps
        .best(VideoCodec::H264, CodecDirection::Decode)
        .expect("an H.264 decoder is listed");
    assert_eq!(d.name, "VideoToolbox");
    let mut enc = b
        .encoder(EncoderConfig::h264(640, 360, 30, 1_000_000), no_wake())
        .expect("encoder");
    let mut dec = b
        .decoder(DecoderConfig::new(VideoCodec::H264), no_wake())
        .expect("decoder");
    let mut decoded = 0;
    for n in 0..30u32 {
        if n == 15 {
            enc.request_keyframe();
        }
        let f = pattern_frame(640, 360, n, 5_000 + n as u64 * 33_333).expect("frame");
        let Some(e) = encode_one(&mut enc, &f, EncodeOptions::default()) else {
            continue;
        };
        let out = decode_one(&mut dec, h264_input(&e));
        assert_eq!((out.width, out.height), (640, 360));
        assert_eq!(out.format(), PixelFormat::I420, "the configured output");
        assert_eq!(out.capture_us, e.capture_us, "capture_us passes through");
        assert_eq!(
            read_frame_code(&out),
            Some(n),
            "the frame code survives H.264 at frame {n}"
        );
        decoded += 1;
    }
    assert!(decoded >= 27, "only {decoded} of 30 decoded");
    let st = dec.stats();
    assert_eq!(st.frames_out, decoded);
    assert_eq!(st.errors, 0);
    eprintln!("[apple] decoder: {:?}", st.implementation);

    // A new size: the keyframe's new SPS and PPS reopen the session.
    enc.set_size(480, 270).expect("set_size");
    let f = pattern_frame(480, 270, 77, 2_000_000).expect("frame");
    let e = encode_one(&mut enc, &f, EncodeOptions::default()).expect("out");
    assert!(e.keyframe);
    let out = decode_one(&mut dec, h264_input(&e));
    assert_eq!((out.width, out.height), (480, 270));
    assert_eq!(read_frame_code(&out), Some(77));
}

#[test]
fn the_h264_decoder_gates_reports_and_recovers() {
    let b = backend().expect("backend");
    let mut enc = b
        .encoder(EncoderConfig::h264(320, 180, 30, 400_000), no_wake())
        .expect("encoder");
    let mut cfg = DecoderConfig::new(VideoCodec::H264);
    cfg.output = PixelFormat::Native;
    let mut dec = b.decoder(cfg, no_wake()).expect("decoder");

    let key = encode_one(
        &mut enc,
        &pattern_frame(320, 180, 1, 100).expect("frame"),
        EncodeOptions::default(),
    )
    .expect("out");
    assert!(key.keyframe);
    let delta = encode_one(
        &mut enc,
        &pattern_frame(320, 180, 2, 200).expect("frame"),
        EncodeOptions::default(),
    )
    .expect("out");
    assert!(!delta.keyframe);

    // A delta first is refused at the door.
    assert_eq!(
        dec.submit(h264_input(&delta)),
        Err(DecodeError::NeedKeyframe)
    );
    // An IDR with no SPS and PPS before it is corrupt.
    let bare: Vec<u8> = vec![0, 0, 0, 1, 0x65, 0x88, 0x84, 0x00, 0x33, 0xff];
    dec.submit(EncodedInput::new(VideoCodec::H264, bare, true, 1))
        .expect("queued");
    let r = wait_for(Duration::from_secs(5), || dec.poll()).expect("an answer");
    assert!(matches!(r, Err(DecodeError::Corrupt(_))), "{r:?}");
    assert_eq!(
        dec.submit(h264_input(&delta)),
        Err(DecodeError::NeedKeyframe),
        "after an error it waits for a keyframe"
    );

    // Parameter sets alone set up the format and give no frame.
    let params: Vec<u8> = annexb_nals(&key.data)
        .into_iter()
        .filter(|n| matches!(nal_type(n), Some(NAL_SPS | NAL_PPS)))
        .flat_map(|n| [&[0u8, 0, 0, 1][..], n].concat())
        .collect();
    dec.submit(EncodedInput::new(VideoCodec::H264, params, true, 3))
        .expect("queued");
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        dec.poll().is_none(),
        "parameter sets alone decode to nothing"
    );

    dec.reset();
    let out = decode_one(&mut dec, h264_input(&key));
    assert_eq!(out.format(), PixelFormat::Native, "the OS buffer, as asked");
    assert_eq!(
        read_frame_code(&out),
        Some(1),
        "it reads back through to_planes"
    );
    let out = decode_one(&mut dec, h264_input(&delta));
    assert_eq!(read_frame_code(&out), Some(2));
    assert_eq!(out.capture_us, 200);

    // A decoded native frame goes back into an encoder of this backend.
    let again = encode_one(&mut enc, &out, EncodeOptions::keyframe()).expect("out");
    check_bitstream(&again);
}

#[test]
fn vp8_round_trips_through_the_apple_backend() {
    let b = backend().expect("backend");
    let mut enc = b
        .encoder(
            EncoderConfig::new(CodecConfig::Vp8, 320, 180, 30, 400_000),
            no_wake(),
        )
        .expect("VP8 encoder");
    let mut dec = b
        .decoder(DecoderConfig::new(VideoCodec::Vp8), no_wake())
        .expect("VP8 decoder");
    for n in 0..10u32 {
        let f = pattern_frame(640, 360, n, n as u64 * 33_333).expect("frame");
        let Some(e) = encode_one(&mut enc, &f, EncodeOptions::default()) else {
            continue;
        };
        assert_eq!(e.codec, VideoCodec::Vp8);
        let out = decode_one(
            &mut dec,
            EncodedInput::new(VideoCodec::Vp8, e.data.clone(), e.keyframe, e.capture_us),
        );
        assert_eq!((out.width, out.height), (320, 180));
        assert_eq!(read_frame_code(&out), Some(n));
    }
}

// ─────────────────────── convert to native buffers ───────────────────────

/// The CVPixelBuffer inside a native frame from this backend.
fn pixel_buffer_of(f: &VideoFrame) -> ffi::CVPixelBufferRef {
    let crate::frame::FrameBuffer::Native(n) = &f.buffer else {
        panic!("a native frame, not {:?}", f.format());
    };
    let pb = crate::apple_pixel_buffer(n).expect("a CVPixelBuffer");
    assert!(!pb.is_null());
    pb
}

/// A pixel buffer's size, CoreVideo format and IOSurface (or null).
fn describe(pb: ffi::CVPixelBufferRef) -> (usize, usize, u32, *mut std::ffi::c_void) {
    // SAFETY: a live pixel buffer (its frame is held by the caller).
    unsafe {
        (
            ffi::CVPixelBufferGetWidth(pb),
            ffi::CVPixelBufferGetHeight(pb),
            ffi::CVPixelBufferGetPixelFormatType(pb),
            ffi::CVPixelBufferGetIOSurface(pb),
        )
    }
}

/// `convert(.., Native)` turns CPU planes into an IOSurface-backed NV12
/// CVPixelBuffer holding the same pixels, with the frame's range and
/// matrix; scales first when the size changes; and passes a pixel buffer
/// of the right size through.
#[test]
fn convert_to_native_gives_iosurface_nv12_pixel_buffers() {
    use crate::frame::ColorSpace;
    let b = backend().expect("backend");
    let i420 = pattern_frame(320, 240, 0x1234_5678, 77).expect("frame");
    let nv12 = crate::convert::convert(&i420, 320, 240, PixelFormat::Nv12).expect("nv12");

    for src in [&i420, &nv12] {
        let n = b
            .convert(src, 320, 240, PixelFormat::Native)
            .expect("to native");
        assert_eq!(n.format(), PixelFormat::Native);
        assert_eq!((n.width, n.height, n.capture_us), (320, 240, 77));
        assert_eq!(n.color, ColorSpace::Bt601Limited);
        let pb = pixel_buffer_of(&n);
        let (w, h, pix, surface) = describe(pb);
        assert_eq!((w, h), (320, 240));
        assert_eq!(pix, ffi::PIX_420V, "NV12, video range");
        assert!(!surface.is_null(), "IOSurface-backed");
        // SAFETY: a live pixel buffer, held by `n`.
        let back = unsafe { pixel::cpu_frame(pb, 0) }.expect("read back");
        assert_eq!(back.planes(), nv12.planes(), "the same pixels");
        assert_eq!(back.color, ColorSpace::Bt601Limited);
        assert_eq!(crate::convert::psnr_y(src, &n).expect("psnr"), 100.0);
        assert_eq!(read_frame_code(&n), Some(0x1234_5678));
    }

    // Odd sizes: chroma rounds up.
    let odd = pattern_frame(33, 17, 5, 0).expect("odd");
    let n = b.convert(&odd, 33, 17, PixelFormat::Native).expect("odd");
    // SAFETY: a live pixel buffer, held by `n`.
    let back = unsafe { pixel::cpu_frame(pixel_buffer_of(&n), 0) }.expect("read back");
    let want = crate::convert::convert(&odd, 33, 17, PixelFormat::Nv12).expect("nv12");
    assert_eq!(back.planes(), want.planes());

    // RGB goes through the CPU conversion first.
    let bgra = crate::convert::convert(&i420, 320, 240, PixelFormat::Bgra).expect("bgra");
    let n = b
        .convert(&bgra, 320, 240, PixelFormat::Native)
        .expect("bgra");
    let pb = pixel_buffer_of(&n);
    assert_eq!(describe(pb).2, ffi::PIX_420V);
    // SAFETY: a live pixel buffer, held by `n`.
    let back = unsafe { pixel::cpu_frame(pb, 0) }.expect("read back");
    let want = crate::convert::convert(&bgra, 320, 240, PixelFormat::Nv12).expect("nv12");
    assert_eq!(back.planes(), want.planes(), "the CPU conversion's pixels");
    assert_eq!(read_frame_code(&n), Some(0x1234_5678));

    // Range and matrix: full range is 420f; the matrix is an attachment.
    for (color, want_pix) in [
        (ColorSpace::Bt709Full, ffi::PIX_420F),
        (ColorSpace::Bt709Limited, ffi::PIX_420V),
        (ColorSpace::Bt601Limited, ffi::PIX_420V),
    ] {
        let mut f = i420.clone();
        f.color = color;
        let n = b
            .convert(&f, 320, 240, PixelFormat::Native)
            .expect("native");
        assert_eq!(n.color, color);
        let pb = pixel_buffer_of(&n);
        assert_eq!(describe(pb).2, want_pix, "{color:?}");
        assert_eq!(pixel::color_of(pb), color, "read back from the buffer");
    }

    // Another size: scaled on the CPU, then wrapped.
    let half = b
        .convert(&i420, 160, 120, PixelFormat::Native)
        .expect("half");
    let pb = pixel_buffer_of(&half);
    assert_eq!(describe(pb).0, 160);
    assert_eq!(describe(pb).1, 120);
    // SAFETY: a live pixel buffer, held by `half`.
    let back = unsafe { pixel::cpu_frame(pb, 0) }.expect("read back");
    let want = crate::convert::convert(&i420, 160, 120, PixelFormat::Nv12).expect("scaled");
    assert_eq!(back.planes(), want.planes());

    // Native to native: the same size passes through; another size is a
    // new, scaled buffer.
    let n = b
        .convert(&i420, 320, 240, PixelFormat::Native)
        .expect("native");
    let same = b.convert(&n, 320, 240, PixelFormat::Native).expect("same");
    assert_eq!(
        pixel_buffer_of(&same),
        pixel_buffer_of(&n),
        "passed through"
    );
    let small = b.convert(&n, 160, 120, PixelFormat::Native).expect("small");
    assert_ne!(pixel_buffer_of(&small), pixel_buffer_of(&n));
    let (w, h, pix, surface) = describe(pixel_buffer_of(&small));
    assert_eq!((w, h, pix), (160, 120, ffi::PIX_420V));
    assert!(!surface.is_null());
    assert_eq!(read_frame_code(&small), read_frame_code(&half));

    // A native frame from convert goes into VideoToolbox without a copy.
    let mut enc = b
        .encoder(EncoderConfig::h264(320, 240, 30, 400_000), no_wake())
        .expect("encoder");
    let e = encode_one(&mut enc, &n, EncodeOptions::default()).expect("out");
    check_bitstream(&e);
    assert_eq!(e.capture_us, 77);

    assert!(matches!(
        b.convert(&i420, 0, 240, PixelFormat::Native),
        Err(VideoError::Invalid(_))
    ));
    // The other formats still come back as CPU planes.
    let p = b.convert(&n, 320, 240, PixelFormat::I420).expect("i420");
    assert_eq!(p.format(), PixelFormat::I420);
    assert_eq!(read_frame_code(&p), Some(0x1234_5678));
}

/// The pools behind `convert(.., Native)`: one per size and range, reused
/// from frame to frame, a new one on a size change, at most four kept.
#[test]
fn native_pools_are_reused_and_rebuilt_on_a_size_change() {
    let pools = pixel::Pools::default();
    let frame = |w, h| pattern_frame(w, h, 9, 0).expect("frame");
    let a = pools.native(&frame(64, 48), 64, 48).expect("a");
    let held = pools.held();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].0, (64, 48, ffi::PIX_420V));
    let first = held[0].1;
    for _ in 0..5 {
        let f = pools.native(&frame(64, 48), 64, 48).expect("again");
        assert_eq!(describe(pixel_buffer_of(&f)).0, 64);
    }
    assert_eq!(
        pools.held(),
        vec![((64, 48, ffi::PIX_420V), first)],
        "reused"
    );
    drop(a);

    // A size change: a new pool, in front.
    let b = pools.native(&frame(96, 64), 96, 64).expect("b");
    assert_eq!(describe(pixel_buffer_of(&b)).0, 96);
    let held = pools.held();
    assert_eq!(held.len(), 2);
    assert_eq!(held[0].0, (96, 64, ffi::PIX_420V));
    assert_ne!(held[0].1, first, "a new pool");
    // Back to the first size: its pool again.
    pools.native(&frame(64, 48), 64, 48).expect("back");
    assert_eq!(pools.held()[0], ((64, 48, ffi::PIX_420V), first));
    // Full range is its own pool.
    let mut full = frame(64, 48);
    full.color = crate::frame::ColorSpace::Bt709Full;
    pools.native(&full, 64, 48).expect("full");
    assert_eq!(pools.held()[0].0, (64, 48, ffi::PIX_420F));
    // At most four.
    for (w, h) in [(32, 32), (48, 32), (80, 60)] {
        pools.native(&frame(w, h), w, h).expect("more");
    }
    let held = pools.held();
    assert_eq!(held.len(), 4);
    assert_eq!(held[0].0, (80, 60, ffi::PIX_420V));
    assert!(held.iter().all(|(k, _)| *k != (96, 64, ffi::PIX_420V)));
}

/// VP8 decoded on the CPU, then handed out as pixel buffers: Sew's
/// picture-in-picture path.
#[cfg(feature = "vp8")]
#[test]
fn vp8_decodes_into_native_pixel_buffers() {
    let b = backend().expect("backend");
    let mut enc = b
        .encoder(
            EncoderConfig::new(CodecConfig::Vp8, 320, 180, 30, 400_000),
            no_wake(),
        )
        .expect("VP8 encoder");
    let mut dec = b
        .decoder(DecoderConfig::new(VideoCodec::Vp8), no_wake())
        .expect("VP8 decoder");
    let mut buffers = Vec::new();
    for n in 0..10u32 {
        let f = pattern_frame(320, 180, n, n as u64 * 33_333).expect("frame");
        let Some(e) = encode_one(&mut enc, &f, EncodeOptions::default()) else {
            continue;
        };
        let out = decode_one(
            &mut dec,
            EncodedInput::new(VideoCodec::Vp8, e.data.clone(), e.keyframe, e.capture_us),
        );
        assert_eq!(out.format(), PixelFormat::I420);
        let native = b
            .convert(&out, out.width, out.height, PixelFormat::Native)
            .expect("to native");
        assert_eq!(native.capture_us, n as u64 * 33_333);
        let (w, h, pix, surface) = describe(pixel_buffer_of(&native));
        assert_eq!((w, h, pix), (320, 180, ffi::PIX_420V));
        assert!(!surface.is_null());
        assert_eq!(read_frame_code(&native), Some(n));
        // Half size for a small window.
        let small = b
            .convert(&out, 160, 90, PixelFormat::Native)
            .expect("small");
        assert_eq!(describe(pixel_buffer_of(&small)).0, 160);
        buffers.push(native);
    }
    assert!(buffers.len() >= 8, "{} of 10 frames", buffers.len());
}

/// Received H.264 goes into VideoToolbox: mutated, truncated and random
/// access units give an error or a frame, never a panic or a hang, and a
/// clean keyframe decodes after a reset. Seeded, so the same every run.
#[test]
fn the_h264_decoder_survives_hostile_input() {
    use crate::kit::DecoderCore;
    let b = backend().expect("backend");
    let mut enc = b
        .encoder(EncoderConfig::h264(320, 180, 30, 400_000), no_wake())
        .expect("encoder");
    let mut valid = Vec::new();
    for n in 0..8u32 {
        let f = pattern_frame(320, 180, n, n as u64 * 33_333).expect("frame");
        if let Some(e) = encode_one(&mut enc, &f, EncodeOptions::default()) {
            valid.push(e);
        }
    }
    assert!(valid[0].keyframe);
    let mut core = vt_dec::VtDecoder::open(&DecoderConfig::new(VideoCodec::H264)).expect("core");
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        seed.wrapping_mul(0x2545_f491_4f6c_dd1d)
    };
    let t = std::time::Instant::now();
    let (mut frames, mut errors) = (0, 0);
    for i in 0..300u64 {
        let src = &valid[(next() % valid.len() as u64) as usize];
        let mut data = src.data.to_vec();
        match next() % 4 {
            0 => {
                for _ in 0..1 + next() % 8 {
                    let at = (next() % data.len() as u64) as usize;
                    data[at] ^= 1 << (next() % 8);
                }
            }
            1 => data.truncate((next() % (data.len() as u64 + 1)) as usize),
            2 => data = (0..next() % 1500).map(|_| next() as u8).collect(),
            _ => {}
        }
        let input = EncodedInput::new(VideoCodec::H264, data, src.keyframe, i);
        match core.decode(&input) {
            Ok(Some(f)) => {
                assert!(f.width <= 8192 && f.height <= 8192, "{f:?}");
                frames += 1;
            }
            Ok(None) => {}
            Err(_) => errors += 1,
        }
    }
    eprintln!(
        "[apple] hostile H.264: {frames} frames, {errors} errors in {:?}",
        t.elapsed()
    );
    assert!(
        t.elapsed() < Duration::from_secs(60),
        "300 hostile inputs took {:?}",
        t.elapsed()
    );
    core.reset().expect("reset");
    let out = core
        .decode(&h264_input(&valid[0]))
        .expect("a clean keyframe decodes after a reset")
        .expect("a frame");
    assert_eq!(read_frame_code(&out), Some(0));
}

// ───────────────────────────── mappings ─────────────────────────────

#[test]
fn access_units_split_into_parameter_sets_and_slices() {
    let au = [
        &[0u8, 0, 0, 1, 0x09, 0xf0][..],
        &[0, 0, 0, 1, 0x67, 1, 2][..],
        &[0, 0, 1, 0x68, 3][..],
        &[0, 0, 0, 1, 0x06, 4][..],
        &[0, 0, 0, 1, 0x65, 5, 6][..],
    ]
    .concat();
    let s = vt_dec::split_access_unit(&au);
    assert_eq!(s.sps, Some(&[0x67u8, 1, 2][..]));
    assert_eq!(s.pps, Some(&[0x68u8, 3][..]));
    assert_eq!(s.nals, vec![&[0x06u8, 4][..], &[0x65u8, 5, 6][..]]);
    let none = vt_dec::split_access_unit(&[0, 0, 0, 1, 0x41, 9]);
    assert_eq!((none.sps, none.pps), (None, None));
    assert_eq!(none.nals.len(), 1);
}

#[test]
fn videotoolbox_statuses_map_to_the_api_errors() {
    assert_eq!(
        vt_dec::decode_error(ffi::VT_INVALID_SESSION, "x"),
        DecodeError::SessionLost
    );
    assert_eq!(
        vt_dec::decode_error(ffi::VT_DECODER_NOT_AVAILABLE_NOW, "x"),
        DecodeError::SessionLost
    );
    assert_eq!(
        vt_dec::decode_error(ffi::VT_DECODER_MALFUNCTION, "x"),
        DecodeError::SessionLost
    );
    assert!(matches!(
        vt_dec::decode_error(ffi::VT_BAD_DATA, "x"),
        DecodeError::Corrupt(_)
    ));
    assert_eq!(
        vt_dec::decode_error(-12345, "x"),
        DecodeError::Video(VideoError::os(-12345, "x"))
    );
    // In front (always, on a Mac) a lost encoder session is an OS error.
    assert_eq!(
        vt_enc::lost_session(ffi::VT_INVALID_SESSION).code(),
        "os",
        "not in the background"
    );
    let gone = vt_enc::lost_session(ffi::VT_ENCODER_NOT_AVAILABLE_NOW);
    if cfg!(target_os = "ios") {
        assert_eq!(
            gone,
            VideoError::ShutDown,
            "iOS: the app is in the background"
        );
    } else {
        assert_eq!(gone.code(), "os");
    }
}

#[test]
fn avfoundation_codes_map_to_the_api_errors() {
    assert_eq!(camera::av_error_code(-11852, "x"), VideoError::NoPermission);
    assert_eq!(camera::av_error_code(-11804, "x"), VideoError::DeviceBusy);
    assert_eq!(camera::av_error_code(-11814, "x"), VideoError::DeviceLost);
    assert_eq!(
        camera::av_error_code(-11819, "media services reset"),
        VideoError::os(-11819, "media services reset")
    );
    // Interruption reasons (iOS).
    assert_eq!(
        camera::interruption_for_reason(Some(1)),
        Interruption::Background
    );
    assert_eq!(
        camera::interruption_for_reason(Some(4)),
        Interruption::Background
    );
    assert_eq!(
        camera::interruption_for_reason(Some(3)),
        Interruption::InUseByAnother
    );
    assert_eq!(
        camera::interruption_for_reason(Some(2)),
        Interruption::InUseByAnother
    );
    assert_eq!(
        camera::interruption_for_reason(Some(5)),
        Interruption::Thermal
    );
    assert_eq!(
        camera::interruption_for_reason(None),
        Interruption::InUseByAnother
    );
    // UIDeviceOrientation to AVCaptureVideoOrientation.
    for o in 1..=4 {
        assert_eq!(camera::video_orientation_for(o), Some(o));
    }
    for o in [0, 5, 6, 7, -1] {
        assert_eq!(camera::video_orientation_for(o), None, "{o}");
    }
}

#[test]
fn phone_modes_are_chosen_by_the_box_turned_landscape() {
    let upright = CaptureConstraints::new(720, 1280, 30);
    let l = camera::landscape_box(upright);
    assert_eq!(
        (l.max_width, l.max_height, l.fps),
        (Some(1280), Some(720), Some(30))
    );
    let wide = CaptureConstraints::new(1280, 720, 30);
    assert_eq!(camera::landscape_box(wide), wide);
    assert_eq!(
        camera::landscape_box(CaptureConstraints::default()),
        CaptureConstraints::default()
    );
    let modes = [
        CaptureMode::new(1920, 1080, 30, PixelFormat::Nv12),
        CaptureMode::new(1280, 720, 60, PixelFormat::Nv12),
        CaptureMode::new(640, 480, 30, PixelFormat::Nv12),
    ];
    assert_eq!(
        nearest_mode(&modes, &l).map(|m| (m.width, m.height)),
        Some((1280, 720)),
        "an upright 720p box picks the 720p sensor mode"
    );
}

/// What iOS says about screens and codecs (in the simulator or on a phone).
#[cfg(target_os = "ios")]
#[test]
fn ios_screens_come_in_through_frame_in_and_h264_is_hardware_only() {
    let b = backend().expect("backend");
    let caps = b.capabilities();
    assert!(caps.screen.supported && caps.screen.frame_in);
    assert!(!caps.screen.list && !caps.screen.system_picker);
    assert_eq!(b.screen_permission(), Permission::Granted);
    assert_eq!(b.screen_sources(), Ok(Vec::new()));
    let r = b.open_screen(
        &ScreenSelection::Listed("0".into()),
        CaptureConstraints::default(),
        no_wake(),
    );
    assert!(
        matches!(&r, Err(VideoError::Unsupported(why)) if why.contains("broadcast upload extension")),
        "{:?}",
        r.err()
    );
    let (tx, rx) = std::sync::mpsc::channel();
    b.pick_screen(
        PickOptions::new(),
        Box::new(move |r| tx.send(r).expect("send")),
    );
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(1))
            .expect("answered at once"),
        Err(VideoError::Unsupported(_))
    ));
    assert!(caps
        .codecs
        .iter()
        .all(|c| c.codec != VideoCodec::H264 || c.implementation == Implementation::Hardware));
    let mut sw = EncoderConfig::h264(640, 360, 30, 500_000);
    sw.implementation = ImplPreference::SoftwareOnly;
    assert!(matches!(
        b.encoder(sw, no_wake()).err(),
        Some(VideoError::Unsupported(_))
    ));
    let mut sw = DecoderConfig::new(VideoCodec::H264);
    sw.implementation = ImplPreference::SoftwareOnly;
    assert!(matches!(
        b.decoder(sw, no_wake()).err(),
        Some(VideoError::Unsupported(_))
    ));
    // The test binary is in front as far as it knows (no UIApplication).
    let rx = b.subscribe();
    assert!(rx.try_recv().is_none());
}

/// The encoder's and decoder's threads move themselves to QoS class
/// user-initiated (their wake callbacks run on those threads).
#[cfg(feature = "vp8")]
#[test]
fn codec_threads_run_at_user_initiated_qos() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let enc_qos = Arc::new(AtomicU32::new(0));
    let dec_qos = Arc::new(AtomicU32::new(0));
    let e = enc_qos.clone();
    let mut enc = crate::vp8::encoder(
        EncoderConfig::new(CodecConfig::Vp8, 64, 64, 30, 300_000),
        Arc::new(move || e.store(current_qos(), Ordering::Relaxed)),
    )
    .expect("encoder");
    let frame = pattern_frame(64, 64, 0, 0).expect("frame");
    let key = encode_one(&mut enc, &frame, EncodeOptions::default()).expect("a keyframe");
    let d = dec_qos.clone();
    let mut dec = crate::vp8::decoder(
        DecoderConfig::new(VideoCodec::Vp8),
        Arc::new(move || d.store(current_qos(), Ordering::Relaxed)),
    )
    .expect("decoder");
    dec.submit(EncodedInput::new(VideoCodec::Vp8, key.data, true, 0))
        .expect("submit");
    wait_for(Duration::from_secs(5), || dec.poll())
        .expect("a decoded frame within 5 s")
        .expect("decode");
    for (what, seen) in [("encoder", &enc_qos), ("decoder", &dec_qos)] {
        let q = wait_for(Duration::from_secs(5), || {
            Some(seen.load(Ordering::Relaxed)).filter(|&q| q != 0)
        })
        .unwrap_or_else(|| panic!("the {what} thread never woke the caller"));
        assert_eq!(q, QOS_CLASS_USER_INITIATED, "{what} thread QoS");
    }
}

// ───────────────────── what the sessions really are ─────────────────────

/// The session's own word wins; then a binding ask (hardware required,
/// which fails the create otherwise, or disabled, which is never used);
/// then VideoToolbox's encoder list. Nothing said is `Unknown`, never a
/// guess.
#[test]
fn session_implementation_never_guesses() {
    use vt_enc::session_implementation as imp;
    let opt = [Some(true), Some(false), None];
    for asked in opt {
        for listed in opt {
            assert_eq!(imp(Some(true), asked, listed), Implementation::Hardware);
            assert_eq!(imp(Some(false), asked, listed), Implementation::Software);
        }
    }
    for listed in opt {
        assert_eq!(imp(None, Some(true), listed), Implementation::Hardware);
        assert_eq!(imp(None, Some(false), listed), Implementation::Software);
    }
    assert_eq!(imp(None, None, Some(true)), Implementation::Hardware);
    assert_eq!(imp(None, None, Some(false)), Implementation::Software);
    assert_eq!(imp(None, None, None), Implementation::Unknown);
}

/// The capability entry comes from a real hardware-only session, probed
/// once and kept.
#[test]
fn the_hardware_h264_probe_opens_a_session_once() {
    let hw = hardware_h264();
    let probes = PROBES.load(Ordering::SeqCst);
    assert!(probes >= 1, "a real probe ran");
    for _ in 0..5 {
        assert_eq!(hardware_h264(), hw);
    }
    let b = backend().expect("backend");
    let caps = b.capabilities();
    assert_eq!(
        PROBES.load(Ordering::SeqCst),
        probes,
        "kept, not probed again"
    );
    let listed_hw = caps.codecs.iter().any(|c| {
        c.codec == VideoCodec::H264
            && c.direction == CodecDirection::Encode
            && c.implementation == Implementation::Hardware
    });
    assert_eq!(
        listed_hw, hw,
        "listed as hardware exactly when the probe found it"
    );
    // The probe's answer is what a hardware-only open gives now.
    let mut cfg = EncoderConfig::h264(1280, 720, 30, 1_000_000);
    cfg.implementation = ImplPreference::HardwareOnly;
    let open = vt_enc::VtEncoder::open(&cfg);
    assert_eq!(open.is_ok(), hw, "{:?}", open.as_ref().err());
    eprintln!("[apple] hardware H.264 encoder: {hw}");
}

#[test]
fn opened_encoders_report_the_session_videotoolbox_made() {
    let b = backend().expect("backend");
    let hw = hardware_h264();
    let mut cfg = EncoderConfig::h264(640, 360, 30, 600_000);

    cfg.implementation = ImplPreference::HardwareOnly;
    match b.encoder(cfg, no_wake()) {
        Ok(enc) => {
            assert!(hw, "a hardware-only open where the probe found none");
            assert_eq!(enc.implementation(), Implementation::Hardware);
            assert!(enc.info().name.contains("(hardware"), "{}", enc.info().name);
        }
        Err(e) => {
            assert!(!hw, "hardware-only refused where the probe found one: {e}");
            assert!(matches!(e, VideoError::Unsupported(_)), "{e:?}");
        }
    }

    // macOS has Apple's software encoder; it says so.
    #[cfg(target_os = "macos")]
    {
        cfg.implementation = ImplPreference::SoftwareOnly;
        let enc = b.encoder(cfg, no_wake()).expect("software encoder");
        assert_eq!(enc.implementation(), Implementation::Software);
        assert_eq!(enc.info().implementation, Implementation::Software);
        assert!(enc.info().name.contains("(software"), "{}", enc.info().name);
    }

    // Preferring hardware: what was made, before and after a resize.
    cfg.implementation = ImplPreference::PreferHardware;
    let mut enc = b.encoder(cfg, no_wake()).expect("encoder");
    let want = if hw {
        Implementation::Hardware
    } else {
        Implementation::Software
    };
    assert_eq!(enc.implementation(), want);
    enc.set_size(480, 270).expect("set_size");
    let f = pattern_frame(480, 270, 3, 1).expect("frame");
    encode_one(&mut enc, &f, EncodeOptions::default()).expect("out");
    assert_eq!(enc.info().config.width, 480);
    assert_eq!(enc.implementation(), want, "the reopened session's");
}

#[test]
fn opened_decoders_report_the_session_once_it_exists() {
    let b = backend().expect("backend");
    let hw = hardware_h264();
    let mut enc = b
        .encoder(EncoderConfig::h264(320, 180, 30, 400_000), no_wake())
        .expect("encoder");
    let key = encode_one(
        &mut enc,
        &pattern_frame(320, 180, 4, 10).expect("frame"),
        EncodeOptions::default(),
    )
    .expect("key");

    let prefs: &[(ImplPreference, Implementation)] = &[
        (
            ImplPreference::PreferHardware,
            if hw {
                Implementation::Hardware
            } else {
                Implementation::Software
            },
        ),
        #[cfg(target_os = "macos")]
        (ImplPreference::SoftwareOnly, Implementation::Software),
    ];
    for &(pref, want) in prefs {
        let mut cfg = DecoderConfig::new(VideoCodec::H264);
        cfg.implementation = pref;
        let mut dec = b.decoder(cfg, no_wake()).expect("decoder");
        assert_eq!(
            dec.implementation(),
            None,
            "{pref:?}: no session before a keyframe"
        );
        assert_eq!(dec.stats().implementation, None);
        let out = decode_one(&mut dec, h264_input(&key));
        assert_eq!(read_frame_code(&out), Some(4));
        assert_eq!(dec.implementation(), Some(want), "{pref:?}");
        dec.reset();
        let gone = wait_for(Duration::from_secs(5), || {
            (dec.implementation().is_none()).then_some(())
        });
        assert!(gone.is_some(), "{pref:?}: a reset closes the session");
        eprintln!("[apple] decoder {pref:?}: {want:?}");
    }
}

// ───────────────────── convert from native buffers ─────────────────────

/// An IOSurface-backed BGRA pixel buffer holding `frame`'s pixels.
fn bgra_pixel_buffer(frame: &VideoFrame) -> VideoFrame {
    let bgra =
        crate::convert::convert(frame, frame.width, frame.height, PixelFormat::Bgra).expect("bgra");
    let p = bgra.planes().expect("planes");
    let (w, h) = (frame.width as usize, frame.height as usize);
    let attrs = ffi::pixel_buffer_attributes(ffi::PIX_BGRA, Some((frame.width, frame.height)));
    let mut pb: ffi::CVPixelBufferRef = std::ptr::null_mut();
    // SAFETY: valid attributes and an out pointer; the buffer is locked
    // while its rows are written within its bytes per row, then wrapped
    // (retained) and our reference released.
    unsafe {
        let st = ffi::CVPixelBufferCreate(std::ptr::null(), w, h, ffi::PIX_BGRA, attrs.0, &mut pb);
        assert!(st == 0 && !pb.is_null(), "CVPixelBufferCreate {st}");
        let owned = ffi::Cf(pb as ffi::CFTypeRef);
        assert_eq!(ffi::CVPixelBufferLockBaseAddress(pb, 0), 0);
        let base = ffi::CVPixelBufferGetBaseAddress(pb) as *mut u8;
        let bpr = ffi::CVPixelBufferGetBytesPerRow(pb);
        assert!(!base.is_null() && bpr >= w * 4);
        for y in 0..h {
            let row = p.row(0, y, frame.width, frame.height);
            std::slice::from_raw_parts_mut(base.add(y * bpr), w * 4).copy_from_slice(row);
        }
        ffi::CVPixelBufferUnlockBaseAddress(pb, 0);
        let out = pixel::native_frame(pb, frame.capture_us);
        drop(owned);
        out
    }
}

/// CPU planes of a native frame, copied out the old way.
fn cpu_copy(native: &VideoFrame) -> VideoFrame {
    // SAFETY: a live pixel buffer, held by `native`.
    let mut f =
        unsafe { pixel::cpu_frame(pixel_buffer_of(native), native.capture_us) }.expect("read back");
    f.color = native.color;
    f
}

/// One hardware capture, two encoders: CVPixelBuffers (NV12 video and full
/// range, BGRA) convert in place to I420 or NV12 at the VP8 backup's
/// sizes, with exactly the pixels a CPU copy converts to.
#[test]
fn native_pixel_buffers_convert_in_place() {
    use crate::frame::ColorSpace;
    let b = backend().expect("backend");
    for (w, h) in [(1280, 720), (720, 1280), (33, 17)] {
        let i420 = pattern_frame(w, h, 0x0BAD_CAFE, 5).expect("frame");
        let mut full = i420.clone();
        full.color = ColorSpace::Bt709Full;
        let sources = [
            b.convert(&i420, w, h, PixelFormat::Native).expect("420v"),
            b.convert(&full, w, h, PixelFormat::Native).expect("420f"),
            bgra_pixel_buffer(&i420),
        ];
        let pix: Vec<u32> = sources
            .iter()
            .map(|s| describe(pixel_buffer_of(s)).2)
            .collect();
        assert_eq!(pix, vec![ffi::PIX_420V, ffi::PIX_420F, ffi::PIX_BGRA]);
        let (dw, dh) = crate::convert::fit_within(w, h, 640, 640);
        let (sw2, sh2) = crate::convert::fit_within(w, h, 320, 320);
        for src in &sources {
            let cpu = cpu_copy(src);
            for (tw, th) in [(dw, dh), (sw2, sh2), (w, h)] {
                for to in [PixelFormat::I420, PixelFormat::Nv12] {
                    let got = b.convert(src, tw, th, to).expect("from native");
                    let want = crate::convert::convert(&cpu, tw, th, to).expect("from cpu");
                    assert_eq!((got.width, got.height, got.capture_us), (tw, th, 5));
                    assert_eq!(got.color, src.color);
                    assert_eq!(
                        got.planes().expect("planes"),
                        want.planes().expect("planes"),
                        "{:?} {w}x{h} -> {to:?} {tw}x{th}",
                        describe(pixel_buffer_of(src)).2.to_be_bytes()
                    );
                }
            }
        }
        // The frame code survives the backup's size.
        let small = b
            .convert(&sources[0], dw, dh, PixelFormat::I420)
            .expect("small");
        assert_eq!(read_frame_code(&small), Some(0x0BAD_CAFE));
    }
    // A frame whose size doesn't match its buffer is refused.
    let mut lying = b
        .convert(
            &pattern_frame(64, 48, 1, 0).expect("frame"),
            64,
            48,
            PixelFormat::Native,
        )
        .expect("native");
    lying.width = 128;
    assert!(matches!(
        b.convert(&lying, 64, 48, PixelFormat::I420),
        Err(VideoError::Invalid(_))
    ));
}

/// ms per frame for the conversions the VP8 backup needs, from a camera's
/// CVPixelBuffer. Run with `--release -- --ignored --nocapture`.
#[test]
#[ignore = "a timing run: cargo test --release, by hand"]
fn convert_timings_from_native_pixel_buffers() {
    let b = backend().expect("backend");
    let land = pattern_frame(1280, 720, 1, 0).expect("frame");
    let port = pattern_frame(720, 1280, 1, 0).expect("frame");
    let nv12_land = b
        .convert(&land, 1280, 720, PixelFormat::Native)
        .expect("420v");
    let nv12_port = b
        .convert(&port, 720, 1280, PixelFormat::Native)
        .expect("420v");
    let bgra_land = bgra_pixel_buffer(&land);
    let runs: [(&str, &VideoFrame, u32, u32); 7] = [
        ("420v 1280x720 -> I420 640x360", &nv12_land, 640, 360),
        ("420v 1280x720 -> I420 320x180", &nv12_land, 320, 180),
        ("420v 1280x720 -> I420 1280x720", &nv12_land, 1280, 720),
        ("420v 720x1280 -> I420 360x640", &nv12_port, 360, 640),
        ("BGRA 1280x720 -> I420 640x360", &bgra_land, 640, 360),
        ("BGRA 1280x720 -> I420 1280x720", &bgra_land, 1280, 720),
        ("420v 1280x720 -> I420 960x540", &nv12_land, 960, 540),
    ];
    for (what, src, w, h) in runs {
        for _ in 0..20 {
            drop(b.convert(src, w, h, PixelFormat::I420).expect("warm"));
        }
        let mut ms: Vec<f64> = (0..300)
            .map(|_| {
                let t = std::time::Instant::now();
                let out = b.convert(src, w, h, PixelFormat::I420).expect("convert");
                let e = t.elapsed().as_secs_f64() * 1000.0;
                drop(out);
                e
            })
            .collect();
        ms.sort_by(|a, b| a.total_cmp(b));
        let mean = ms.iter().sum::<f64>() / ms.len() as f64;
        eprintln!(
            "[timing] {what}: median {:.3} ms, mean {mean:.3} ms, p95 {:.3} ms",
            ms[ms.len() / 2],
            ms[ms.len() * 95 / 100]
        );
    }
}

#[test]
fn cf_values_are_read_only_as_the_type_they_are() {
    use super::ffi::{bool_value, dict, dict_value, no, num_i32, number_i64, set_named, yes};
    let key = c"kVTEncodeFrameOptionKey_ForceKeyFrame";
    let d = dict();
    assert!(set_named(&d, key, yes()));
    let k = super::ffi::cf_const(key).expect("key");
    let n = num_i32(7);
    // SAFETY: live CF objects (or null) throughout.
    unsafe {
        assert_eq!(bool_value(yes()), Some(true));
        assert_eq!(bool_value(no()), Some(false));
        assert_eq!(bool_value(std::ptr::null()), None);
        // A number where a boolean is wanted, a dictionary lookup in a
        // number: refused, not read as the wrong type.
        assert_eq!(bool_value(n.0), None);
        assert!(dict_value(n.0, k).is_null());
        assert!(dict_value(std::ptr::null(), k).is_null());
        assert_eq!(bool_value(dict_value(d.0, k)), Some(true));
    }
    assert_eq!(number_i64(n.0), Some(7));
    assert_eq!(number_i64(yes()), None);
    assert_eq!(number_i64(d.0), None);
}

// ───────────────────────────── losing the camera ─────────────────────────────

use camera::{Life, Next, Signal};

/// Feed `signals` to `life`: the events each one sent, and what came next.
fn feed(life: &mut Life, signals: Vec<Signal>) -> Vec<(Vec<SourceEvent>, Next)> {
    signals
        .into_iter()
        .map(|s| {
            let mut sent = Vec::new();
            let next = life.on(s, &mut |e| sent.push(e));
            (sent, next)
        })
        .collect()
}

fn look(connected: bool, running: bool, interrupted: bool, suspended: bool) -> Signal {
    Signal::Poll {
        connected,
        running,
        interrupted,
        suspended,
    }
}

const LOST: SourceEvent = SourceEvent::Ended(EndReason::DeviceLost);

#[test]
fn a_disconnected_device_ends_the_source_once() {
    for phone in [false, true] {
        let mut life = Life::new(phone);
        assert_eq!(
            feed(
                &mut life,
                vec![
                    look(true, true, false, false),
                    Signal::Disconnected,
                    Signal::Disconnected,
                    look(false, false, false, false),
                    Signal::RuntimeError(-11800, "late".into()),
                ]
            ),
            vec![
                (vec![], Next::Watch),
                (vec![LOST], Next::Stop),
                (vec![], Next::Stop),
                (vec![], Next::Stop),
                (vec![], Next::Stop),
            ],
            "phone: {phone}"
        );
        // The look every 250 ms catches it too.
        let mut life = Life::new(phone);
        assert_eq!(
            feed(&mut life, vec![look(false, true, false, false)]),
            vec![(vec![LOST], Next::Stop)],
            "phone: {phone}"
        );
    }
}

#[test]
fn a_runtime_error_ends_a_mac_camera_at_once() {
    // macOS: a runtime error stops the session for good (a Continuity
    // Camera that went away posts one), even before isRunning says so.
    let mut life = Life::new(false);
    assert_eq!(
        feed(
            &mut life,
            vec![
                Signal::RuntimeError(-11800, "device went away".into()),
                look(true, true, false, false),
            ]
        ),
        vec![
            (
                vec![
                    SourceEvent::Error(VideoError::os(
                        -11800,
                        "capture session: device went away"
                    )),
                    LOST
                ],
                Next::Stop
            ),
            (vec![], Next::Stop),
        ]
    );
    // A session that stopped on its own ends the source too.
    let mut life = Life::new(false);
    assert_eq!(
        feed(
            &mut life,
            vec![look(true, true, false, false), look(true, false, false, false)]
        ),
        vec![(vec![], Next::Watch), (vec![LOST], Next::Stop)]
    );
}

#[test]
fn a_phone_tells_device_lost_from_interruptions() {
    // A phone call: interrupted, then resumed, never lost, even while the
    // session isn't running.
    let mut life = Life::new(true);
    assert_eq!(
        feed(
            &mut life,
            vec![
                Signal::Interrupted(Some(3)),
                Signal::Interrupted(Some(1)),
                look(true, false, true, false),
                Signal::InterruptionEnded,
                Signal::InterruptionEnded,
                look(true, true, false, false),
            ]
        ),
        vec![
            (
                vec![SourceEvent::Interrupted(Interruption::InUseByAnother)],
                Next::Watch
            ),
            (vec![], Next::Watch),
            (vec![], Next::Watch),
            (vec![SourceEvent::Resumed], Next::Watch),
            (vec![], Next::Watch),
            (vec![], Next::Watch),
        ]
    );
    // An error that says the device is gone ends it at once.
    let mut life = Life::new(true);
    assert_eq!(
        feed(&mut life, vec![Signal::RuntimeError(-11814, "gone".into())]),
        vec![(
            vec![SourceEvent::Error(VideoError::DeviceLost), LOST],
            Next::Stop
        )]
    );
    // Any other error is reported; the session stopping without an
    // interruption then ends the source.
    let mut life = Life::new(true);
    assert_eq!(
        feed(
            &mut life,
            vec![
                Signal::RuntimeError(-11800, "x".into()),
                look(true, true, false, false),
                look(true, false, false, false),
            ]
        ),
        vec![
            (
                vec![SourceEvent::Error(VideoError::os(
                    -11800,
                    "capture session: x"
                ))],
                Next::Watch
            ),
            (vec![], Next::Watch),
            (vec![LOST], Next::Stop),
        ]
    );
}

#[test]
fn a_media_services_reset_restarts_the_session_once() {
    let reset = || {
        SourceEvent::Error(VideoError::os(
            camera::MEDIA_SERVICES_RESET,
            "capture session: reset",
        ))
    };
    let mut life = Life::new(true);
    assert_eq!(
        feed(
            &mut life,
            vec![
                Signal::RuntimeError(camera::MEDIA_SERVICES_RESET, "reset".into()),
                look(true, false, false, false),
                // Running again after the restart.
                look(true, true, false, false),
                Signal::RuntimeError(camera::MEDIA_SERVICES_RESET, "reset".into()),
                look(true, false, false, false),
                // This restart didn't take: lost.
                look(true, false, false, false),
            ]
        ),
        vec![
            (vec![reset()], Next::Watch),
            (vec![], Next::Restart),
            (vec![], Next::Watch),
            (vec![reset()], Next::Watch),
            (vec![], Next::Restart),
            (vec![LOST], Next::Stop),
        ]
    );
}

#[test]
fn a_suspended_camera_pauses_without_ending() {
    // A MacBook's lid closes and opens while its camera runs.
    let mut life = Life::new(false);
    assert_eq!(
        feed(
            &mut life,
            vec![
                look(true, true, false, true),
                look(true, true, false, true),
                look(true, true, false, false),
            ]
        ),
        vec![
            (
                vec![SourceEvent::Interrupted(Interruption::Suspended)],
                Next::Watch
            ),
            (vec![], Next::Watch),
            (vec![SourceEvent::Resumed], Next::Watch),
        ]
    );
    // On a phone, a suspension and an interruption that overlap are one
    // pause: one Interrupted, and one Resumed when both are over.
    let mut life = Life::new(true);
    assert_eq!(
        feed(
            &mut life,
            vec![
                look(true, true, false, true),
                Signal::Interrupted(Some(1)),
                look(true, false, true, false),
                Signal::InterruptionEnded,
            ]
        ),
        vec![
            (
                vec![SourceEvent::Interrupted(Interruption::Suspended)],
                Next::Watch
            ),
            (vec![], Next::Watch),
            (vec![], Next::Watch),
            (vec![SourceEvent::Resumed], Next::Watch),
        ]
    );
}

#[test]
fn a_closed_source_sends_nothing() {
    let mut life = Life::new(false);
    life.close();
    assert_eq!(
        feed(
            &mut life,
            vec![
                Signal::Disconnected,
                Signal::RuntimeError(-11800, "x".into()),
                look(true, true, false, true),
            ]
        ),
        vec![
            (vec![], Next::Stop),
            (vec![], Next::Stop),
            (vec![], Next::Stop)
        ]
    );
}

struct NoControl;
impl crate::kit::SourceControl for NoControl {}

type Obj = objc2::rc::Retained<AnyObject>;

/// An NSObject standing in for a capture session or a device: the
/// observers only match notifications by the object that posts them.
fn stand_in() -> Obj {
    // SAFETY: +[NSObject new].
    let o: Option<Obj> = unsafe { msg_send![class!(NSObject), new] };
    o.expect("an NSObject")
}

/// Post the notification `name` from `object`, as AVFoundation does
/// (synchronously, on this thread).
fn post(name: &AnyObject, object: &AnyObject, info: Option<&AnyObject>) {
    let info: *const AnyObject = info.map_or(std::ptr::null(), |i| i as *const AnyObject);
    autoreleasepool(|_| {
        // SAFETY: NSNotificationCenter's documented post method, with an
        // NSString name, a live object and a nil or live dictionary.
        unsafe {
            let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
            let _: () =
                msg_send![center, postNotificationName: name, object: object, userInfo: info];
        }
    });
}

/// A runtime error notification's userInfo: `AVCaptureSessionErrorKey` to
/// an AVFoundation NSError with `code`.
fn runtime_error_info(code: isize) -> Obj {
    let domain = objc::nsstring("AVFoundationErrorDomain").expect("an NSString");
    let key = ffi::cf_const(c"AVCaptureSessionErrorKey").expect("AVCaptureSessionErrorKey")
        as *const AnyObject;
    // SAFETY: NSError and NSDictionary class methods with live arguments.
    unsafe {
        let nil: *const AnyObject = std::ptr::null();
        let e: Option<Obj> =
            msg_send![class!(NSError), errorWithDomain: &*domain, code: code, userInfo: nil];
        let e = e.expect("an NSError");
        let d: Option<Obj> =
            msg_send![class!(NSDictionary), dictionaryWithObject: &*e, forKey: key];
        d.expect("an NSDictionary")
    }
}

/// The four notification names, as the tests' own NSStrings. Posting
/// AVFoundation's real names from a stand-in would reach AVFoundation's own
/// process-wide observers, which take the object for a real device.
struct TestNames {
    disconnected: Obj,
    runtime_error: Obj,
    interrupted: Obj,
    interruption_ended: Obj,
}

impl TestNames {
    fn new() -> TestNames {
        let n = |s: &str| objc::nsstring(s).expect("an NSString");
        TestNames {
            disconnected: n("KesselVideoTestDeviceWasDisconnected"),
            runtime_error: n("KesselVideoTestSessionRuntimeError"),
            interrupted: n("KesselVideoTestSessionWasInterrupted"),
            interruption_ended: n("KesselVideoTestSessionInterruptionEnded"),
        }
    }

    fn notices(&self) -> camera::Notices {
        let p = |o: &Obj| Some(objc2::rc::Retained::as_ptr(o));
        camera::Notices {
            disconnected: p(&self.disconnected),
            runtime_error: p(&self.runtime_error),
            interrupted: p(&self.interrupted),
            interruption_ended: p(&self.interruption_ended),
        }
    }
}

/// A camera source's queue, life and observers, on two stand-ins.
struct Observed {
    src: crate::kit::QueuedSource<NoControl>,
    life: Arc<Mutex<Life>>,
    observers: Vec<objc::Observer>,
    names: TestNames,
    session: Obj,
    device: Obj,
    /// How many times the source woke its caller.
    wakes: Arc<std::sync::atomic::AtomicUsize>,
}

fn observed() -> Observed {
    let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w = wakes.clone();
    let shared = crate::kit::SourceShared::new(
        CaptureConstraints::new(640, 360, 30),
        30,
        Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let (session, device, names) = (stand_in(), stand_in(), TestNames::new());
    let life = Arc::new(Mutex::new(Life::new(cfg!(target_os = "ios"))));
    // SAFETY: live objects (the observers only compare them by identity)
    // and live NSStrings.
    let observers =
        unsafe { camera::observers(&session, &device, &life, &shared, &names.notices()) };
    assert_eq!(observers.len(), 4, "one observer for each name");
    Observed {
        src: crate::kit::QueuedSource::new(shared, NoControl),
        life,
        observers,
        names,
        session,
        device,
        wakes,
    }
}

#[test]
fn avfoundation_names_its_notifications() {
    let n = camera::Notices::avfoundation();
    assert!(n.disconnected.is_some(), "the disconnect notification");
    assert!(n.runtime_error.is_some(), "the runtime error notification");
    let phone = cfg!(target_os = "ios");
    assert_eq!(n.interrupted.is_some(), phone, "interruptions on iOS only");
    assert_eq!(n.interruption_ended.is_some(), phone);
    assert!(
        ffi::cf_const(c"AVCaptureSessionErrorKey").is_some(),
        "the runtime error's userInfo key"
    );
}

#[test]
fn the_device_disconnected_notification_ends_the_source() {
    let mut o = observed();
    let other = stand_in();
    // Another camera's disconnection, and one posted by our session, are
    // not our device's.
    post(&o.names.disconnected, &other, None);
    post(&o.names.disconnected, &o.session, None);
    assert_eq!(o.src.try_event(), None);
    assert_eq!(o.wakes.load(Ordering::SeqCst), 0);
    post(&o.names.disconnected, &o.device, None);
    assert_eq!(o.src.try_event(), Some(LOST));
    assert_eq!(o.src.try_event(), None);
    assert_eq!(o.wakes.load(Ordering::SeqCst), 1);
    // Ended is sent once.
    post(&o.names.disconnected, &o.device, None);
    assert_eq!(o.src.try_event(), None);
    assert_eq!(o.wakes.load(Ordering::SeqCst), 1);
}

#[test]
fn the_runtime_error_notification_ends_the_source() {
    let mut o = observed();
    // AVErrorDeviceWasDisconnected: lost on macOS and iOS alike.
    let info = runtime_error_info(-11808);
    post(&o.names.runtime_error, &o.device, Some(&*info));
    assert_eq!(o.src.try_event(), None, "from the device, not the session");
    post(&o.names.runtime_error, &o.session, Some(&*info));
    assert_eq!(
        o.src.try_event(),
        Some(SourceEvent::Error(VideoError::DeviceLost))
    );
    assert_eq!(o.src.try_event(), Some(LOST));
    assert_eq!(o.src.try_event(), None);
}

#[cfg(target_os = "macos")]
#[test]
fn any_runtime_error_notification_ends_a_mac_camera() {
    let mut o = observed();
    let info = runtime_error_info(-11800);
    post(&o.names.runtime_error, &o.session, Some(&*info));
    let first = o.src.try_event();
    assert!(
        matches!(
            first,
            Some(SourceEvent::Error(VideoError::Os { code: -11800, .. }))
        ),
        "{first:?}"
    );
    assert_eq!(o.src.try_event(), Some(LOST));
}

#[test]
fn interruption_notifications_pause_and_resume() {
    let mut o = observed();
    // No reason in the userInfo: another client has the camera.
    post(&o.names.interrupted, &o.session, None);
    assert_eq!(
        o.src.try_event(),
        Some(SourceEvent::Interrupted(Interruption::InUseByAnother))
    );
    post(&o.names.interruption_ended, &o.device, None);
    assert_eq!(o.src.try_event(), None, "from the session only");
    post(&o.names.interruption_ended, &o.session, None);
    assert_eq!(o.src.try_event(), Some(SourceEvent::Resumed));
    assert_eq!(o.src.try_event(), None);
    // Still running: the next look sends nothing.
    let mut sent = Vec::new();
    let next = lock(&o.life).on(look(true, true, false, false), &mut |e| sent.push(e));
    assert_eq!((sent, next), (vec![], Next::Watch));
}

#[test]
fn no_events_after_the_source_closes() {
    let Observed {
        mut src,
        life,
        observers,
        names,
        session,
        device,
        wakes,
    } = observed();
    // What CameraControl's Drop does first: close, then remove.
    lock(&life).close();
    post(&names.disconnected, &device, None);
    drop(observers);
    post(&names.disconnected, &device, None);
    let info = runtime_error_info(-11808);
    post(&names.runtime_error, &session, Some(&*info));
    assert_eq!(src.try_event(), None);
    assert_eq!(wakes.load(Ordering::SeqCst), 0);
    // Removed, the observers let go of the life they held.
    assert_eq!(Arc::strong_count(&life), 1);
}

#[test]
fn an_object_without_is_suspended_is_not_suspended() {
    let o = stand_in();
    // SAFETY: a live object; isSuspended is only sent if it answers it.
    assert!(!unsafe { camera::is_suspended(&*o) });
}
