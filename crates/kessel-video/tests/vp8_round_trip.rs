//! VP8 end to end through the public API on this machine's own backend:
//! frames from the fake camera, encoded and decoded by `system()`'s VP8, the
//! frame code read back from every decoded frame. Simulcast too: three
//! encoders fed the same captures at their own sizes.

#![cfg(all(feature = "system", feature = "vp8"))]

use kessel_video::fake::{read_frame_code, FakeBackend, FakeConfig};
use kessel_video::{
    no_wake, system, CaptureConstraints, CodecConfig, CodecDirection, DecoderConfig, EncodeOptions, EncodedFrame, EncodedInput, EncoderConfig,
    FrameSource, Implementation, PixelFormat, SystemConfig, VideoBackend, VideoCodec, VideoEncoder, VideoFrame,
};
use std::time::{Duration, Instant};

fn wait<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(t) = f() {
            return Some(t);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn next_frame(src: &mut Box<dyn FrameSource>) -> VideoFrame {
    wait(Duration::from_secs(5), || src.try_recv()).expect("a camera frame within 5 s")
}

fn encode(enc: &mut Box<dyn VideoEncoder>, f: &VideoFrame) -> Option<EncodedFrame> {
    let skipped = enc.stats().dropped_by_rc;
    enc.submit(f, EncodeOptions::default()).expect("submit");
    wait(Duration::from_secs(10), || match enc.poll() {
        Some(r) => Some(Some(r.expect("encode"))),
        None if enc.stats().dropped_by_rc > skipped => Some(None),
        None => None,
    })
    .expect("an output or a skip within 10 s")
}

#[test]
fn vp8_simulcast_round_trip_on_the_system_backend() {
    let backend = system(SystemConfig::default()).expect("this OS's backend");
    let caps = backend.capabilities();
    let enc_cap = caps.best(VideoCodec::Vp8, CodecDirection::Encode).expect("a VP8 encoder");
    assert_eq!(enc_cap.implementation, Implementation::Software);
    assert!(caps.best(VideoCodec::Vp8, CodecDirection::Decode).is_some(), "a VP8 decoder");
    eprintln!("[vp8] {} on {}: {}", caps.backend, caps.os, enc_cap.name);

    let camera = FakeBackend::new(FakeConfig::default());
    let mut src = camera.open_camera(None, CaptureConstraints::new(1280, 720, 30), no_wake()).expect("fake camera");
    // q, h, f: one encoder per layer, all fed the same captures.
    let layers = [(320, 180, 250_000), (640, 360, 700_000), (1280, 720, 2_000_000)];
    let mut encoders: Vec<Box<dyn VideoEncoder>> = layers
        .iter()
        .map(|&(w, h, bps)| {
            let mut c = EncoderConfig::new(CodecConfig::Vp8, w, h, 30, bps);
            c.temporal_layers = 2;
            backend.encoder(c, no_wake()).expect("VP8 encoder")
        })
        .collect();
    let mut decoders: Vec<_> = layers
        .iter()
        .map(|_| {
            let mut c = DecoderConfig::new(VideoCodec::Vp8);
            c.output = PixelFormat::Rgba;
            backend.decoder(c, no_wake()).expect("VP8 decoder")
        })
        .collect();

    let mut checked = [0u32; 3];
    for _ in 0..30 {
        let frame = next_frame(&mut src);
        let code = read_frame_code(&frame).expect("a coded capture");
        for (i, enc) in encoders.iter_mut().enumerate() {
            let Some(e) = encode(enc, &frame) else { continue };
            assert_eq!(e.capture_us, frame.capture_us, "every layer keeps the capture time");
            assert_eq!((e.width, e.height), (layers[i].0, layers[i].1));
            decoders[i]
                .submit(EncodedInput::new(VideoCodec::Vp8, e.data, e.keyframe, e.capture_us))
                .expect("decode submit");
            let d = wait(Duration::from_secs(10), || decoders[i].poll()).expect("decoded").expect("decode");
            assert_eq!(d.format(), PixelFormat::Rgba);
            assert_eq!((d.width, d.height), (layers[i].0, layers[i].1));
            assert_eq!(read_frame_code(&d), Some(code), "layer {i} lost the frame code");
            checked[i] += 1;
        }
    }
    assert!(checked.iter().all(|&n| n >= 25), "frames checked per layer: {checked:?}");
    for enc in &encoders {
        assert_eq!(enc.stats().keyframes, 1, "one keyframe per layer, no surprise ones");
    }
}
