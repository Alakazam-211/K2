//! The encoder's output decodes with vp8-decoder, looks like its input, and
//! follows the rate, keyframe and layer requests.

use vp8_decoder::Decoder;
use vp8_encoder::{Encoder, EncoderConfig, Picture, TemporalLayers};

/// A synthetic camera-like sequence: a textured background that pans, a
/// moving bright square and a gradient, plus a little noise.
fn frame(w: usize, h: usize, t: usize) -> Vec<u8> {
    let mut seed = (t as u32).wrapping_mul(2_654_435_761).wrapping_add(1);
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed % 5) as i32 - 2
    };
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = vec![0u8; w * h + 2 * cw * ch];
    for y in 0..h {
        for x in 0..w {
            let px = x + 2 * t;
            let tex = ((px / 8 + y / 8) % 2) as i32 * 40 + ((px * 7 + y * 3) % 31) as i32;
            let grad = (x * 100 / w) as i32;
            let sq = t * 3 % w;
            let in_sq = x >= sq && x < sq + w / 6 && y >= h / 3 && y < h / 3 + h / 5;
            let v = if in_sq { 230 } else { 40 + tex + grad } + noise();
            out[y * w + x] = v.clamp(0, 255) as u8;
        }
    }
    for y in 0..ch {
        for x in 0..cw {
            out[w * h + y * cw + x] = (100 + (x * 60 / cw) as i32 + (t % 20) as i32) as u8;
            out[w * h + cw * ch + y * cw + x] = (150 - (y * 50 / ch) as i32) as u8;
        }
    }
    out
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mse: f64 = a.iter().zip(b).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    if mse == 0.0 {
        99.0
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

struct Run {
    psnr_min: f64,
    psnr_avg: f64,
    bits: Vec<usize>,
    keyframes: Vec<usize>,
    tids: Vec<u8>,
    syncs: Vec<bool>,
}

fn run(cfg: EncoderConfig, frames: usize, mut each: impl FnMut(usize, &mut Encoder)) -> Run {
    let (w, h) = (cfg.width as usize, cfg.height as usize);
    let mut enc = Encoder::new(cfg).expect("encoder");
    let mut dec = Decoder::new();
    let mut r = Run {
        psnr_min: 99.0,
        psnr_avg: 0.0,
        bits: Vec::new(),
        keyframes: Vec::new(),
        tids: Vec::new(),
        syncs: Vec::new(),
    };
    let mut decoded = 0;
    for t in 0..frames {
        each(t, &mut enc);
        let src = frame(w, h, t);
        let pic = Picture::from_i420(&src, w as u32, h as u32).expect("picture");
        let Some(out) = enc.encode(&pic, false).expect("encode") else {
            r.bits.push(0);
            continue;
        };
        let info = vp8_decoder::frame_info(&out.data).expect("frame header");
        assert_eq!(info.keyframe, out.keyframe, "frame {t}: keyframe flag");
        if out.keyframe {
            r.keyframes.push(t);
        }
        r.bits.push(out.data.len() * 8);
        r.tids.push(out.temporal_id);
        r.syncs.push(out.layer_sync);
        let picture = dec.decode(&out.data).unwrap_or_else(|e| panic!("frame {t}: {e}")).expect("shown");
        assert_eq!((picture.width, picture.height), (w as u32, h as u32));
        let p = psnr(&picture.to_i420(), &src);
        r.psnr_min = r.psnr_min.min(p);
        r.psnr_avg += p;
        decoded += 1;
    }
    r.psnr_avg /= decoded as f64;
    r
}

#[test]
fn decodes_and_looks_like_the_input() {
    let r = run(EncoderConfig::new(320, 240, 30, 600_000), 45, |_, _| {});
    eprintln!("[roundtrip] 320x240 at 600 kbit/s: PSNR avg {:.2} dB, min {:.2} dB", r.psnr_avg, r.psnr_min);
    assert_eq!(r.keyframes, vec![0], "only the first frame is a keyframe");
    assert!(r.psnr_avg > 32.0, "average PSNR {:.2} dB", r.psnr_avg);
    assert!(r.psnr_min > 27.0, "minimum PSNR {:.2} dB", r.psnr_min);
}

#[test]
fn odd_sizes_and_tiny_frames_round_trip() {
    for (w, h) in [(1, 1), (17, 9), (33, 47), (175, 143)] {
        let r = run(EncoderConfig::new(w, h, 30, 300_000), 6, |_, _| {});
        assert!(r.psnr_min > 25.0, "{w}x{h}: minimum PSNR {:.2} dB", r.psnr_min);
    }
}

#[test]
fn hits_the_target_bitrate() {
    for (w, h, rate) in [(320u32, 240u32, 400_000u32), (640, 360, 1_000_000)] {
        let frames = 150;
        let r = run(EncoderConfig::new(w, h, 30, rate), frames, |_, _| {});
        // Skip the first second (keyframe and model warm-up).
        let tail: usize = r.bits[30..].iter().sum();
        let kbps = tail as f64 / ((frames - 30) as f64 / 30.0) / 1000.0;
        eprintln!("[rate] {w}x{h} target {} kbit/s: got {kbps:.0} kbit/s, PSNR avg {:.2} dB", rate / 1000, r.psnr_avg);
        let target = rate as f64 / 1000.0;
        assert!(kbps > target * 0.6 && kbps < target * 1.25, "{w}x{h}: {kbps:.0} kbit/s for a {target} kbit/s target");
    }
}

#[test]
fn keyframes_come_on_request_and_rate_changes_do_not_force_one() {
    let r = run(EncoderConfig::new(160, 120, 30, 200_000), 40, |t, enc| {
        if t == 10 {
            enc.set_rates(100_000, 30);
        }
        if t == 20 {
            enc.request_keyframe();
        }
    });
    assert_eq!(r.keyframes, vec![0, 20]);
}

#[test]
fn temporal_layers_follow_the_webrtc_pattern() {
    let r = run(EncoderConfig::new(160, 120, 30, 300_000).with_temporal_layers(TemporalLayers::L1T3), 12, |_, _| {});
    assert_eq!(r.tids, vec![0, 2, 1, 2, 0, 2, 1, 2, 0, 2, 1, 2]);
    assert_eq!(r.syncs[..4], [true, true, true, false]);
    let r = run(EncoderConfig::new(160, 120, 30, 300_000).with_temporal_layers(TemporalLayers::L1T2), 6, |_, _| {});
    assert_eq!(r.tids, vec![0, 1, 0, 1, 0, 1]);
}

/// A receiver that drops the upper layers still decodes every frame it gets.
#[test]
fn base_layer_alone_decodes() {
    let (w, h) = (160usize, 120usize);
    let mut enc = Encoder::new(EncoderConfig::new(w as u32, h as u32, 30, 300_000).with_temporal_layers(TemporalLayers::L1T3)).expect("encoder");
    let mut base = Decoder::new();
    let mut half = Decoder::new();
    for t in 0..24 {
        let src = frame(w, h, t);
        let pic = Picture::from_i420(&src, w as u32, h as u32).expect("picture");
        let out = enc.encode(&pic, false).expect("encode").expect("not dropped at this rate");
        if out.temporal_id == 0 {
            let p = base.decode(&out.data).expect("base layer decodes").expect("shown");
            assert!(psnr(&p.to_i420(), &src) > 25.0, "frame {t}");
        }
        if out.temporal_id <= 1 {
            let p = half.decode(&out.data).expect("layers 0 and 1 decode").expect("shown");
            assert!(psnr(&p.to_i420(), &src) > 25.0, "frame {t}");
        }
    }
}

#[test]
fn rejects_the_wrong_size() {
    let mut enc = Encoder::new(EncoderConfig::new(64, 64, 30, 100_000)).expect("encoder");
    let data = vec![0u8; 32 * 32 * 3 / 2];
    let pic = Picture::from_i420(&data, 32, 32).expect("picture");
    assert!(enc.encode(&pic, false).is_err());
}
