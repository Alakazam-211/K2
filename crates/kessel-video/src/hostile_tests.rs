//! Hostile input on the receive path. A decoder's input comes from the
//! network, so random bytes and mutated, truncated and spliced frames must
//! give an error or a well-formed frame: never a panic, a hang, or a frame
//! larger than the decoder allows. The H.264 helpers an app may run on
//! received data get the same treatment.
//!
//! This is a seeded random test, not a coverage-guided fuzzer: it runs the
//! same inputs every time.

use crate::h264;

/// xorshift64*: deterministic, and no dependency.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next_u64() as u8).collect()
    }
}

/// One hostile input made from `valid`: random bytes, or a valid input with
/// bits flipped, truncated, extended, its first bytes (the header) rewritten,
/// spliced with another, or claiming a random size (a VP8 keyframe header,
/// RFC 6386 section 9.1: 14 bits of width and of height, 2 of scaling).
fn mutate(rng: &mut Rng, valid: &[Vec<u8>]) -> Vec<u8> {
    let mut p = valid[rng.below(valid.len())].clone();
    match rng.below(8) {
        0 => {
            let n = rng.below(2000);
            p = rng.bytes(n);
        }
        1 => {
            for _ in 0..1 + rng.below(8) {
                if !p.is_empty() {
                    let i = rng.below(p.len());
                    p[i] ^= 1 << rng.below(8);
                }
            }
        }
        2 => {
            let n = rng.below(p.len() + 1);
            p.truncate(n);
        }
        3 => {
            let n = rng.below(1200);
            let tail = rng.bytes(n);
            p.extend(tail);
        }
        4 => {
            for _ in 0..1 + rng.below(3) {
                if !p.is_empty() {
                    let i = rng.below(p.len().min(10));
                    p[i] = rng.next_u64() as u8;
                }
            }
        }
        5 => {
            let other = valid[rng.below(valid.len())].clone();
            let cut = rng.below(p.len() + 1);
            let from = rng.below(other.len() + 1);
            p.truncate(cut);
            p.extend_from_slice(&other[from..]);
        }
        6 => {
            if p.len() >= 10 {
                // Mostly small sizes that decode; now and then any 16 bits.
                for at in [6, 8] {
                    let v = if rng.below(4) == 0 { rng.next_u64() as u16 } else { 1 + rng.below(640) as u16 };
                    p[at..at + 2].copy_from_slice(&v.to_le_bytes());
                }
            }
        }
        _ => p.clear(),
    }
    p
}

#[test]
fn h264_helpers_survive_hostile_bytes() {
    let mut rng = Rng(0x6832_3634_6b76_6964);
    // A small access unit (SPS, PPS, IDR slice) in both framings.
    let mut au = Vec::new();
    h264::push_annexb(&mut au, &[0x67, 0x42, 0xc0, 0x1f, 0xda, 0x01, 0x40]);
    h264::push_annexb(&mut au, &[0x68, 0xce, 0x3c, 0x80]);
    h264::push_annexb(&mut au, &[0x65, 0x88, 0x84, 0x00, 0x00, 0x03, 0x00, 0x21, 0x7f]);
    let valid = vec![au.clone(), h264::annexb_to_avcc(&au)];
    let mut parsed = 0;
    for _ in 0..5_000 {
        let data = mutate(&mut rng, &valid);
        let nals = h264::annexb_nals(&data);
        assert!(nals.iter().map(|n| n.len()).sum::<usize>() <= data.len());
        assert_eq!(h264::annexb_is_keyframe(&data), h264::is_keyframe(&nals));
        let _ = h264::codec_string(&nals);
        for nal in &nals {
            let _ = h264::nal_type(nal);
            let _ = h264::profile_level_id(nal);
            assert!(h264::unescape_rbsp(nal).len() <= nal.len());
        }
        let avcc = h264::annexb_to_avcc(&data);
        assert_eq!(avcc.len(), nals.iter().map(|n| n.len() + 4).sum::<usize>());
        for len_size in 0..=5 {
            if let Ok(found) = h264::avcc_nals(&data, len_size) {
                assert_eq!(found.iter().map(|n| n.len() + len_size).sum::<usize>(), data.len());
                parsed += 1;
            }
            let mut out = Vec::new();
            if h264::avcc_to_annexb(&data, len_size, &mut out).is_ok() {
                assert!(out.len() <= 4 * data.len());
            }
        }
    }
    assert!(parsed > 500, "the AVCC parser accepted only {parsed} inputs");
}

#[cfg(feature = "vp8")]
mod vp8 {
    use super::{mutate, Rng};
    use crate::api::no_wake;
    use crate::error::DecodeError;
    use crate::fake::{pattern_frame, read_frame_code};
    use crate::frame::{FrameBuffer, VideoFrame};
    use crate::kit::{wait_for, DecoderCore};
    use crate::types::{DecoderConfig, EncodedInput, VideoCodec};
    use crate::vp8::{decoder, Vp8Core};
    use std::time::Duration;

    const W: u32 = 320;
    const H: u32 = 180;

    /// A valid VP8 stream: a keyframe, deltas, and a second keyframe.
    fn valid_stream() -> Vec<Vec<u8>> {
        let cfg = vp8_encoder::EncoderConfig::new(W, H, 30, 400_000);
        let mut enc = vp8_encoder::Encoder::new(cfg).expect("encoder");
        let mut out = Vec::new();
        for n in 0..24u32 {
            let f = pattern_frame(W, H, n, u64::from(n) * 33_333).expect("frame");
            let FrameBuffer::Planes(p) = &f.buffer else {
                panic!("the pattern frame is CPU planes");
            };
            let tight = p.to_tight(W, H);
            let pic = vp8_encoder::Picture::from_i420(&tight.data, W, H).expect("picture");
            if let Some(e) = enc.encode(&pic, n == 12).expect("encode") {
                out.push(e.data);
            }
        }
        assert!(out.len() >= 20, "only {} frames", out.len());
        assert_eq!(out[0][0] & 1, 0, "the first frame is a keyframe");
        out
    }

    /// A decoded frame is well formed: a size the decoder allows, and planes
    /// that fit.
    fn check(f: &VideoFrame) {
        assert!(f.width > 0 && f.height > 0);
        assert!(
            u64::from(f.width) * u64::from(f.height) <= vp8_decoder::DEFAULT_MAX_PIXELS,
            "a {}x{} frame",
            f.width,
            f.height
        );
        let FrameBuffer::Planes(p) = &f.buffer else {
            panic!("a decoded frame is CPU planes");
        };
        p.validate(f.width, f.height).expect("the planes fit");
    }

    #[test]
    fn vp8_decoding_survives_hostile_frames() {
        let valid = valid_stream();
        let mut rng = Rng(0x6b65_7373_656c_7670);
        let mut core = Vp8Core::new();
        let (mut frames, mut errors) = (0u32, 0u32);
        for i in 0..3_000u64 {
            // A valid keyframe now and then, so mutated deltas reach the
            // inter-prediction paths with a reference in place.
            if i % 40 == 0 {
                let key = EncodedInput::new(VideoCodec::Vp8, valid[0].clone(), true, i);
                let f = core.decode(&key).expect("a valid keyframe decodes").expect("and is shown");
                check(&f);
            }
            let data = if rng.below(4) == 0 { valid[rng.below(valid.len())].clone() } else { mutate(&mut rng, &valid) };
            let keyframe = rng.below(2) == 0;
            match core.decode(&EncodedInput::new(VideoCodec::Vp8, data, keyframe, i)) {
                Ok(Some(f)) => {
                    check(&f);
                    frames += 1;
                }
                Ok(None) => {}
                Err(_) => errors += 1,
            }
        }
        assert!(frames > 200 && errors > 200, "both paths reached: {frames} frames, {errors} errors");
        // The stream decodes again from a keyframe.
        core.reset().expect("reset");
        let key = EncodedInput::new(VideoCodec::Vp8, valid[0].clone(), true, 1);
        let f = core.decode(&key).expect("decode").expect("a frame");
        assert_eq!((f.width, f.height), (W, H));
        assert_eq!(read_frame_code(&f), Some(0));
    }

    #[test]
    fn the_decoder_thread_survives_hostile_frames_and_decodes_again() {
        let valid = valid_stream();
        let mut rng = Rng(0x7468_7265_6164_7376);
        let mut dec = decoder(DecoderConfig::new(VideoCodec::Vp8), no_wake()).expect("decoder");
        for i in 0..500u64 {
            // Marked as keyframes, so none is refused for want of one.
            let input = EncodedInput::new(VideoCodec::Vp8, mutate(&mut rng, &valid), true, i);
            loop {
                match dec.submit(input.clone()) {
                    Ok(()) => break,
                    Err(DecodeError::Busy) => std::thread::sleep(Duration::from_millis(1)),
                    Err(e) => panic!("input {i} refused: {e:?}"),
                }
                while let Some(r) = dec.poll() {
                    if let Ok(f) = r {
                        check(&f);
                    }
                }
            }
            while let Some(r) = dec.poll() {
                if let Ok(f) = r {
                    check(&f);
                }
            }
        }
        // A panic would have ended the thread, and nothing would come out.
        dec.reset();
        dec.submit(EncodedInput::new(VideoCodec::Vp8, valid[0].clone(), true, 7)).expect("submit");
        let f = wait_for(Duration::from_secs(10), || dec.poll())
            .expect("a frame within 10 s: the decoder thread is alive")
            .expect("the valid keyframe decodes");
        assert_eq!((f.width, f.height, f.capture_us), (W, H, 7));
        assert_eq!(read_frame_code(&f), Some(0));
        assert!(dec.stats().errors > 0, "some hostile frames failed: {:?}", dec.stats());
    }
}
