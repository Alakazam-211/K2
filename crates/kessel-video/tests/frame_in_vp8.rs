//! Frame-in end to end through the public API: NV12 frames pushed into a
//! frame-in source (sideways, as a phone's screen can arrive), turned
//! upright, encoded as VP8, decoded, and the frame code read back from
//! every decoded frame. Then the producer stops and the source ends.

#![cfg(feature = "vp8")]

use kessel_video::fake::{pattern_frame, read_frame_code, FakeBackend, FakeConfig};
use kessel_video::{
    frame_in, no_wake, psnr_y, CodecConfig, DecoderConfig, EncodeOptions, EncodedInput,
    EncoderConfig, EndReason, FrameInOptions, FrameSource, PixelFormat, Planes, Rotation,
    SourceEvent, VideoBackend, VideoCodec, VideoFrame,
};
use std::time::{Duration, Instant};

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

/// An upright pattern frame turned a quarter counter-clockwise, as tight
/// NV12 bytes (what a sideways screen hands over), with its size.
fn sideways_nv12(width: u32, height: u32, n: u32) -> (Vec<u8>, u32, u32) {
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
    let mut nv12 = turn(&p.data[..w * h], w, h);
    let u = turn(&p.data[w * h..w * h + cw * ch], cw, ch);
    let v = turn(&p.data[w * h + cw * ch..], cw, ch);
    for (u, v) in u.iter().zip(&v) {
        nv12.push(*u);
        nv12.push(*v);
    }
    (nv12, height, width)
}

#[test]
fn nv12_frames_in_through_vp8_and_back() {
    let backend = FakeBackend::new(FakeConfig::default());
    let (tx, mut src) = frame_in(FrameInOptions::new(1280, 720, 15), no_wake()).expect("frame_in");
    let mut enc = backend
        .encoder(
            EncoderConfig::new(CodecConfig::Vp8, 320, 240, 15, 600_000),
            no_wake(),
        )
        .expect("VP8 encoder");
    let mut dec_cfg = DecoderConfig::new(VideoCodec::Vp8);
    dec_cfg.output = PixelFormat::I420;
    let mut dec = backend.decoder(dec_cfg, no_wake()).expect("VP8 decoder");

    let mut decoded = Vec::new();
    for n in 0..8u32 {
        let (bytes, w, h) = sideways_nv12(320, 240, 0x5a00_0000 + n);
        assert_eq!((w, h), (240, 320), "pushed sideways");
        let at = 1_000_000 + n as u64 * 70_000;
        let frame = VideoFrame::from_planes(
            Planes::tight(PixelFormat::Nv12, w, h, bytes).expect("planes"),
            w,
            h,
            at,
        )
        .expect("frame");
        tx.push(frame, Rotation::Deg90).expect("push");
        let upright = wait(|| src.try_recv());
        assert_eq!(
            (upright.width, upright.height),
            (320, 240),
            "turned upright"
        );
        assert_eq!(upright.capture_us, at);
        let skipped = enc.stats().dropped_by_rc;
        enc.submit(&upright, EncodeOptions::default())
            .expect("submit");
        let out = wait(|| match enc.poll() {
            Some(r) => Some(Some(r.expect("encoded"))),
            None if enc.stats().dropped_by_rc > skipped => Some(None),
            None => None,
        });
        // Rate control may skip a frame; a skipped one has nothing to check.
        let Some(out) = out else { continue };
        assert_eq!(out.capture_us, at, "the timestamp travels through VP8");
        assert_eq!(
            out.keyframe,
            decoded.is_empty(),
            "only the first is a keyframe"
        );
        dec.submit(EncodedInput::new(
            VideoCodec::Vp8,
            out.data.clone(),
            out.keyframe,
            out.capture_us,
        ))
        .expect("decode");
        let back = wait(|| dec.poll()).expect("decoded");
        assert_eq!((back.width, back.height, back.capture_us), (320, 240, at));
        assert_eq!(read_frame_code(&back), Some(0x5a00_0000 + n), "frame {n}");
        decoded.push(n);
        let psnr = psnr_y(&upright, &back).expect("psnr");
        assert!(psnr > 28.0, "frame {n}: PSNR {psnr:.1} dB");
    }
    assert!(decoded.len() >= 6, "decoded {decoded:?} of 8");

    let events: Vec<SourceEvent> = std::iter::from_fn(|| src.try_event()).collect();
    assert_eq!(
        events,
        vec![SourceEvent::Started {
            width: 320,
            height: 240,
            fps: 15
        }]
    );
    tx.end(EndReason::UserStopped);
    assert_eq!(
        src.try_event(),
        Some(SourceEvent::Ended(EndReason::UserStopped))
    );
}
