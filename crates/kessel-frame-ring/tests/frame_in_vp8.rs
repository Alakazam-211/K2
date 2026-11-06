//! The whole phone screen-share path on one machine: a producer (standing in
//! for the broadcast upload extension) writes sideways NV12 frames into the
//! ring; the app's side reads them into a kessel-video frame-in source
//! (`pump`, the glue a call engine needs), which turns them upright; VP8
//! encodes and decodes them, and each frame's code reads back. Then the
//! producer stops and the source ends with the person's reason.

#![cfg(unix)]

use kessel_frame_ring::{
    monotonic_ns, Color, Consumer, EndReason as RingEnd, Layout, Nv12Frame, Producer,
    ProducerState, Pushed,
};
use kessel_video::fake::{pattern_frame, read_frame_code, FakeBackend, FakeConfig};
use kessel_video::{
    frame_in, no_wake, now_us, CodecConfig, ColorSpace, DecoderConfig, EncodeOptions, EncodedInput,
    EncoderConfig, EndReason, FrameInOptions, FrameInSender, FrameSource, PixelFormat, Planes,
    Rotation, SourceEvent, VideoBackend, VideoCodec, VideoFrame,
};
use std::time::{Duration, Instant};

/// The app's glue, called every few milliseconds while sharing: the
/// newest ring frame into the frame-in source, and the producer's end into
/// the source's end. False once the source is done.
fn pump(ring: &mut Consumer, tx: &FrameInSender, buf: &mut Vec<u8>) -> bool {
    let state = ring.producer().expect("producer state");
    ring.set_wanted(tx.wants_frames());
    if let Some(info) = ring.try_read(buf) {
        // The ring's clock is the machine's; the source's is this process's.
        let age_us = monotonic_ns().saturating_sub(info.timestamp_ns) / 1_000;
        let capture_us = now_us().saturating_sub(age_us);
        let planes = Planes::new(
            PixelFormat::Nv12,
            info.width,
            info.height,
            std::mem::take(buf),
            [info.y_stride() as u32, info.uv_stride() as u32, 0],
            [0, info.uv_offset(), 0],
        )
        .expect("ring frames are tight NV12");
        let mut frame =
            VideoFrame::from_planes(planes, info.width, info.height, capture_us).expect("frame");
        frame.color = match info.color {
            Color::Bt601Limited => ColorSpace::Bt601Limited,
            Color::Bt709Limited => ColorSpace::Bt709Limited,
            Color::Bt709Full => ColorSpace::Bt709Full,
        };
        let rotation = Rotation::from_degrees(info.rotation).expect("a quarter turn");
        if tx.push(frame, rotation).is_err() {
            // The call dropped the source: ask the extension to stop.
            ring.request_stop();
            return false;
        }
    }
    match state {
        ProducerState::Ended(RingEnd::UserStopped) => {
            tx.end(EndReason::UserStopped);
            false
        }
        ProducerState::Ended(_) | ProducerState::Gone => {
            tx.end(EndReason::SourceClosed);
            false
        }
        ProducerState::Waiting | ProducerState::Running => true,
    }
}

/// Pattern frame `n`, turned a quarter counter-clockwise (as a phone held
/// sideways hands it over), as tight NV12 planes: (Y, UV, width, height).
fn sideways(width: u32, height: u32, n: u32) -> (Vec<u8>, Vec<u8>, u32, u32) {
    let up = pattern_frame(width, height, n, 0).expect("pattern");
    let p = up.planes().expect("planes").to_tight(width, height);
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = (w / 2, h / 2);
    // Out is h wide and w tall: out(x, y) = in(w - 1 - y, x).
    let turn = |src: &[u8], sw: usize, sh: usize| -> Vec<u8> {
        let mut out = vec![0u8; sw * sh];
        for y in 0..sw {
            for x in 0..sh {
                out[y * sh + x] = src[x * sw + (sw - 1 - y)];
            }
        }
        out
    };
    let y = turn(&p.data[..w * h], w, h);
    let u = turn(&p.data[w * h..w * h + cw * ch], cw, ch);
    let v = turn(&p.data[w * h + cw * ch..], cw, ch);
    let uv = u.iter().zip(&v).flat_map(|(u, v)| [*u, *v]).collect();
    (y, uv, height, width)
}

fn wait<T>(mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(t) = f() {
            return t;
        }
        assert!(Instant::now() < deadline, "nothing within 10 s");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn ring_frames_go_through_frame_in_and_vp8() {
    let path = std::env::temp_dir().join(format!("kfr-e2e-{}", std::process::id()));
    let mut ring = Consumer::create(&path, Layout::hd720()).expect("ring");
    let (tx, mut src) = frame_in(FrameInOptions::new(1280, 720, 15), no_wake()).expect("frame_in");
    let mut extension = Producer::attach(&path).expect("attach");
    extension.start();

    let backend = FakeBackend::new(FakeConfig::default());
    let mut enc = backend
        .encoder(
            EncoderConfig::new(CodecConfig::Vp8, 320, 240, 15, 600_000),
            no_wake(),
        )
        .expect("VP8 encoder");
    let mut dec = backend
        .decoder(DecoderConfig::new(VideoCodec::Vp8), no_wake())
        .expect("VP8 decoder");

    let mut buf = Vec::new();
    let mut decoded = 0;
    for n in 0..6u32 {
        let code = 0x7e00_0000 + n;
        let (y, uv, w, h) = sideways(320, 240, code);
        let mut f = Nv12Frame::new(&y, w as usize, &uv, w as usize, w, h);
        f.rotation = 90;
        f.color = Color::Bt709Limited;
        f.timestamp_ns = monotonic_ns();
        assert_eq!(
            extension.push(&f).expect("push"),
            Pushed::Written {
                frame_no: n as u64 + 1,
                width: 240,
                height: 320
            }
        );
        assert!(pump(&mut ring, &tx, &mut buf), "still sharing");
        let frame = wait(|| src.try_recv());
        assert_eq!((frame.width, frame.height), (320, 240), "upright");
        assert_eq!(frame.color, ColorSpace::Bt709Limited);
        assert!(frame.capture_us <= now_us());
        let skipped = enc.stats().dropped_by_rc;
        enc.submit(&frame, EncodeOptions::default())
            .expect("submit");
        let out = wait(|| match enc.poll() {
            Some(r) => Some(Some(r.expect("encoded"))),
            None if enc.stats().dropped_by_rc > skipped => Some(None),
            None => None,
        });
        std::thread::sleep(Duration::from_millis(70));
        // Rate control may skip a frame; a skipped one has nothing to check.
        let Some(out) = out else { continue };
        assert_eq!(out.capture_us, frame.capture_us);
        dec.submit(EncodedInput::new(
            VideoCodec::Vp8,
            out.data.clone(),
            out.keyframe,
            out.capture_us,
        ))
        .expect("decode");
        let back = wait(|| dec.poll()).expect("decoded");
        assert_eq!(read_frame_code(&back), Some(code), "frame {n}");
        decoded += 1;
    }
    assert!(decoded >= 4, "decoded {decoded} of 6");

    // The person stops the broadcast: the source ends with their reason.
    extension.finish(RingEnd::UserStopped);
    assert!(!pump(&mut ring, &tx, &mut buf));
    let events: Vec<SourceEvent> = std::iter::from_fn(|| src.try_event()).collect();
    assert_eq!(
        events.last(),
        Some(&SourceEvent::Ended(EndReason::UserStopped)),
        "{events:?}"
    );
    assert!(!tx.is_open());
    drop(ring);
    assert!(!path.exists());
}
