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
                    let v = if rng.below(4) == 0 {
                        rng.next_u64() as u16
                    } else {
                        1 + rng.below(640) as u16
                    };
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
    h264::push_annexb(
        &mut au,
        &[0x65, 0x88, 0x84, 0x00, 0x00, 0x03, 0x00, 0x21, 0x7f],
    );
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
                assert_eq!(
                    found.iter().map(|n| n.len() + len_size).sum::<usize>(),
                    data.len()
                );
                parsed += 1;
            }
            let mut out = Vec::new();
            if h264::avcc_to_annexb(&data, len_size, &mut out).is_ok() {
                assert!(out.len() <= 4 * data.len());
            }
        }
    }
    assert!(
        parsed > 500,
        "the AVCC parser accepted only {parsed} inputs"
    );
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
                let f = core
                    .decode(&key)
                    .expect("a valid keyframe decodes")
                    .expect("and is shown");
                check(&f);
            }
            let data = if rng.below(4) == 0 {
                valid[rng.below(valid.len())].clone()
            } else {
                mutate(&mut rng, &valid)
            };
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
        assert!(
            frames > 200 && errors > 200,
            "both paths reached: {frames} frames, {errors} errors"
        );
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
        dec.submit(EncodedInput::new(
            VideoCodec::Vp8,
            valid[0].clone(),
            true,
            7,
        ))
        .expect("submit");
        let f = wait_for(Duration::from_secs(10), || dec.poll())
            .expect("a frame within 10 s: the decoder thread is alive")
            .expect("the valid keyframe decodes");
        assert_eq!((f.width, f.height, f.capture_us), (W, H, 7));
        assert_eq!(read_frame_code(&f), Some(0));
        assert!(
            dec.stats().errors > 0,
            "some hostile frames failed: {:?}",
            dec.stats()
        );
    }
}

/// File mode's receive path: a file's access units come from an untrusted
/// upload, so truncated and garbage access units, times that can't be in
/// order, absurd sizes and a flood past the output queue must give typed
/// errors or well-formed frames in order: never a panic, a hang, a dropped
/// frame or a frame out of order. Through the fake backend's structural
/// decoder, which holds frames back as an OS decoder does.
mod file_mode {
    use super::{mutate, Rng};
    use crate::api::{no_wake, VideoBackend, VideoDecoder};
    use crate::error::{DecodeError, VideoError};
    use crate::fake::tests::b_frame_stream;
    use crate::fake::{read_frame_code, FakeBackend, FakeConfig};
    use crate::frame::VideoFrame;
    use crate::h264::{self, test_sps};
    use crate::kit::{wait_for, FILE_OUTPUT_QUEUE};
    use crate::types::{DecoderConfig, EncodedInput, VideoCodec};
    use std::time::Duration;

    fn decoder(queue: usize) -> Box<dyn VideoDecoder> {
        let b = FakeBackend::new(FakeConfig::default());
        let mut cfg = DecoderConfig::file(VideoCodec::H264);
        cfg.output_queue = queue;
        b.decoder(cfg, no_wake()).expect("a File-mode decoder")
    }

    /// Everything out until `EndOfStream` (after a flush): frames, and the
    /// errors before it.
    fn drain(dec: &mut Box<dyn VideoDecoder>) -> (Vec<VideoFrame>, Vec<DecodeError>) {
        dec.flush().expect("flush");
        let (mut frames, mut errors) = (Vec::new(), Vec::new());
        wait_for(Duration::from_secs(10), || loop {
            match dec.poll() {
                Some(Ok(f)) => frames.push(f),
                Some(Err(DecodeError::EndOfStream)) => return Some(()),
                Some(Err(e)) => errors.push(e),
                None => return None,
            }
        })
        .expect("EndOfStream within 10 s: the decoder thread is alive");
        (frames, errors)
    }

    /// Submit, polling into `out` while the queue pushes back.
    fn submit(
        dec: &mut Box<dyn VideoDecoder>,
        input: EncodedInput,
        out: &mut Vec<VideoFrame>,
    ) -> Result<(), DecodeError> {
        loop {
            match dec.submit(input.clone()) {
                Err(DecodeError::Busy) => {
                    while let Some(r) = dec.poll() {
                        if let Ok(f) = r {
                            out.push(f);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                r => return r,
            }
        }
    }

    fn strictly_increasing(frames: &[VideoFrame]) {
        for w in frames.windows(2) {
            assert!(
                w[0].capture_us < w[1].capture_us,
                "out of order: {} then {}",
                w[0].capture_us,
                w[1].capture_us
            );
        }
    }

    #[test]
    fn truncated_and_garbage_access_units_give_errors_and_frames_stay_in_order() {
        let stream = b_frame_stream(30);
        let valid: Vec<Vec<u8>> = stream.iter().map(|i| i.data.to_vec()).collect();
        let mut rng = Rng(0x6669_6c65_6d6f_6465);
        let mut dec = decoder(FILE_OUTPUT_QUEUE);
        let mut out = Vec::new();
        let mut refused = 0;
        for round in 0..600u64 {
            // Now and then the valid stream again, so frames come out too.
            let input = if round % 50 < 30 {
                let mut i = stream[(round % 50) as usize].clone();
                i.capture_us += (round / 50) * 10_000_000;
                i.dts_us = i.dts_us.map(|d| d + (round / 50) * 10_000_000);
                i
            } else {
                let data = match rng.below(3) {
                    0 => {
                        // A truncated access unit.
                        let mut p = valid[rng.below(valid.len())].clone();
                        let n = rng.below(p.len() + 1);
                        p.truncate(n);
                        p
                    }
                    1 => {
                        // Garbage NAL units behind real start codes.
                        let mut p = Vec::new();
                        for _ in 0..1 + rng.below(4) {
                            let n = rng.below(64);
                            h264::push_annexb(&mut p, &rng.bytes(n));
                        }
                        p
                    }
                    _ => mutate(&mut rng, &valid),
                };
                let pts = (round / 50) * 10_000_000 + 5_000_000 + round;
                EncodedInput::file(VideoCodec::H264, data, true, pts, None)
            };
            match submit(&mut dec, input, &mut out) {
                Ok(()) => {}
                Err(DecodeError::NeedKeyframe | DecodeError::Video(VideoError::Invalid(_))) => {
                    refused += 1
                }
                Err(e) => panic!("round {round}: {e:?}"),
            }
        }
        let (frames, _errors) = drain(&mut dec);
        out.extend(frames);
        strictly_increasing(&out);
        assert!(out.len() > 100, "only {} frames", out.len());
        let st = dec.stats();
        assert!(st.errors > 0, "some hostile inputs failed: {st:?}");
        eprintln!(
            "[hostile file] {} frames, {refused} refused, {st:?}",
            out.len()
        );
        // A clean stream after all that decodes whole and in order.
        let mut out = Vec::new();
        for i in b_frame_stream(12) {
            submit(&mut dec, i, &mut out).expect("a clean stream");
        }
        let (frames, errors) = drain(&mut dec);
        out.extend(frames);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(out.len(), 12);
        for (i, f) in out.iter().enumerate() {
            assert_eq!(f.capture_us, i as u64 * 40_000);
            assert_eq!(read_frame_code(f), Some(i as u32));
        }
    }

    #[test]
    fn times_that_cant_be_in_order_are_refused_with_invalid() {
        let stream = b_frame_stream(12);
        let mut dec = decoder(FILE_OUTPUT_QUEUE);
        let mut out = Vec::new();
        for i in stream.iter().take(9) {
            submit(&mut dec, i.clone(), &mut out).expect("valid");
        }
        let invalid = |r: Result<(), DecodeError>| {
            matches!(r, Err(DecodeError::Video(VideoError::Invalid(_))))
        };
        // The dts going backwards, and the same dts again.
        let mut back = stream[9].clone();
        back.dts_us = Some(0);
        assert!(invalid(dec.submit(back)));
        assert_eq!(
            dec.submit(stream[9].clone()),
            Err(DecodeError::NeedKeyframe),
            "after a refusal the stream goes on from a keyframe"
        );
        // A keyframe whose pts is already taken, then one over i64::MAX.
        let mut dup = stream[0].clone();
        dup.capture_us = stream[8].capture_us;
        dup.dts_us = None;
        assert!(invalid(dec.submit(dup)));
        let mut huge = stream[0].clone();
        huge.capture_us = u64::MAX;
        huge.dts_us = None;
        assert!(invalid(dec.submit(huge)));
        let (frames, _) = drain(&mut dec);
        out.extend(frames);
        strictly_increasing(&out);
        assert_eq!(out.len(), 9, "the valid ones all came out");
        // After a frame is out, a pts at or before it can't be.
        let mut first = stream[0].clone();
        first.capture_us = 1_000_000;
        first.dts_us = None;
        dec.submit(first).expect("a keyframe");
        let (frames, _) = drain(&mut dec);
        assert_eq!(frames.len(), 1);
        let mut key = stream[0].clone();
        key.capture_us = 2_000_000;
        key.dts_us = None;
        dec.submit(key.clone()).expect("a keyframe");
        let mut out = Vec::new();
        // Push more keyframes until the first is out (the reorder depth),
        // then one before it.
        let mut pts = 2_000_000;
        while out.is_empty() {
            pts += 40_000;
            let mut k = key.clone();
            k.capture_us = pts;
            submit(&mut dec, k, &mut out).expect("a keyframe");
            std::thread::sleep(Duration::from_millis(2));
            while let Some(Ok(f)) = dec.poll() {
                out.push(f);
            }
            assert!(pts < 4_000_000, "nothing came out");
        }
        let mut late = key.clone();
        late.capture_us = out[0].capture_us;
        assert!(invalid(dec.submit(late)), "a pts already out");
        let (frames, _) = drain(&mut dec);
        out.extend(frames);
        strictly_increasing(&out);
        assert_eq!(dec.stats().dropped_out, 0);
    }

    #[test]
    fn absurd_sizes_and_other_sampling_are_unsupported_not_decoded() {
        use test_sps::{sps, Spec};
        let au = |s: Spec| {
            let mut au = Vec::new();
            h264::push_annexb(&mut au, &sps(s));
            h264::push_annexb(&mut au, &[0x68, 0xce, 0x3c, 0x80]);
            h264::push_annexb(&mut au, &[0x65, 0x88, 0x84, 0x21]);
            au
        };
        let cases = [
            Spec {
                profile: 110,
                depth: 10,
                ..Spec::default()
            },
            Spec {
                profile: 122,
                chroma: 2,
                ..Spec::default()
            },
            Spec {
                profile: 244,
                chroma: 3,
                depth: 10,
                ..Spec::default()
            },
            Spec {
                width_mbs: 1024,
                height_mbs: 1024,
                ..Spec::default()
            },
            Spec {
                width_mbs: 2048,
                height_mbs: 1,
                ..Spec::default()
            },
        ];
        let mut dec = decoder(FILE_OUTPUT_QUEUE);
        for (n, s) in cases.into_iter().enumerate() {
            let input = EncodedInput::file(VideoCodec::H264, au(s), true, n as u64, None);
            dec.submit(input).expect("queued");
            let r = wait_for(Duration::from_secs(5), || dec.poll()).expect("an answer");
            assert!(
                matches!(r, Err(DecodeError::Video(VideoError::Unsupported(_)))),
                "{s:?}: {r:?}"
            );
        }
        // An SPS that names more than H.264 allows: corrupt.
        let mut bad = Vec::new();
        h264::push_annexb(
            &mut bad,
            &sps(Spec {
                width_mbs: 65_535,
                ..Spec::default()
            }),
        );
        dec.submit(EncodedInput::file(VideoCodec::H264, bad, true, 99, None))
            .expect("queued");
        let r = wait_for(Duration::from_secs(5), || dec.poll()).expect("an answer");
        assert!(matches!(r, Err(DecodeError::Corrupt(_))), "{r:?}");
        assert_eq!(dec.stats().frames_out, 0, "nothing was decoded");
        // Random SPS bytes never panic the parser.
        let mut rng = Rng(0x7370_7370_7370_7370);
        let valid = vec![
            sps(Spec::default()),
            sps(Spec {
                reorder: Some(3),
                ..Spec::default()
            }),
        ];
        for _ in 0..5_000 {
            let mut nal = mutate(&mut rng, &valid);
            if nal.is_empty() {
                continue;
            }
            nal[0] = 0x67;
            if let Ok(i) = h264::parse_sps(&nal) {
                assert!(i.width_mbs <= 2048 && i.height_mbs <= 2048);
                assert!(i.reorder_depth() <= 16);
            }
        }
    }

    #[test]
    fn a_flood_past_the_queue_is_busy_and_nothing_is_dropped() {
        let stream = b_frame_stream(40);
        let mut dec = decoder(2);
        let mut accepted = 0;
        let mut busy = 0;
        // Nobody polls: the queue fills, then every submit is pushed back.
        for i in stream.iter() {
            let mut tries = 0;
            loop {
                match dec.submit(i.clone()) {
                    Ok(()) => {
                        accepted += 1;
                        break;
                    }
                    Err(DecodeError::Busy) => {
                        busy += 1;
                        tries += 1;
                        if tries > 50 {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => panic!("{e:?}"),
                }
            }
            if tries > 50 {
                break;
            }
        }
        assert!(busy > 50, "the full queue pushed back: {busy}");
        assert!(accepted < 40, "{accepted} accepted with nobody polling");
        let (frames, errors) = drain(&mut dec);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(frames.len(), accepted, "every accepted frame came out");
        strictly_increasing(&frames);
        let st = dec.stats();
        assert_eq!(st.dropped_out, 0);
        assert_eq!(st.frames_out, accepted as u64);
    }
}
