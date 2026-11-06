//! Bitrate accuracy: on camera-like content the encoder lands near its
//! target at every size and rate a WebRTC simulcast ladder uses, and never
//! overshoots by more than the tolerance. On easy content (a flat test
//! pattern) it may undershoot, but only with the quantizer at its floor:
//! there is nothing left to spend the bits on.

mod common;

use common::camera::Camera;
use vp8_encoder::{Encoder, EncoderConfig, Picture, TemporalLayers};

/// The lowest quantizer index rate control picks (`rate::Q_MIN`).
const Q_FLOOR: u8 = 4;

const FPS: usize = 30;
/// Frames encoded per run; the first second (keyframe and model warm-up)
/// is not measured.
const FRAMES: usize = 240;
const WARMUP: usize = FPS;

/// Achieved bitrate within this fraction of the target, either way.
const TOLERANCE: f64 = 0.12;

/// kessel-video's fake camera pattern (`kessel_video::fake::pattern_frame`):
/// a gradient, a bar that moves with `n`, and `n` drawn as 32 black or
/// white blocks.
fn fake_pattern(w: usize, h: usize, n: usize) -> Vec<u8> {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut buf = vec![0u8; w * h + 2 * cw * ch];
    let bar_w = (w / 16).max(2);
    let bar_x = (n * (w / 64).max(1)) % w;
    for y in 0..h {
        for x in 0..w {
            let in_bar = x >= bar_x && x < bar_x + bar_w && y > h / 4;
            buf[y * w + x] = if in_bar {
                200
            } else {
                (40 + (x * 100 / w.max(1)) + (y * 60 / h.max(1))) as u8
            };
        }
    }
    let code = n as u32;
    let (bw, bh) = ((w / 16).max(1), (h / 16).max(1));
    for bit in 0..32u32 {
        let on = (code >> (31 - bit)) & 1 == 1;
        let (x0, y0) = ((bit % 8) as usize * bw, (bit / 8) as usize * bh);
        for y in y0..(y0 + bh).min(h) {
            for x in x0..(x0 + bw).min(w) {
                buf[y * w + x] = if on { 235 } else { 16 };
            }
        }
    }
    for i in 0..cw * ch {
        buf[w * h + i] = 110 + ((i % cw) * 30 / cw.max(1)) as u8;
        buf[w * h + cw * ch + i] = 140;
    }
    buf
}

struct Measured {
    kbps: f64,
    q_min: u8,
    q_median: u8,
    q_max: u8,
    dropped: usize,
}

fn measure(cfg: EncoderConfig, frame: impl Fn(usize) -> Vec<u8>) -> Measured {
    let (w, h) = (cfg.width, cfg.height);
    let mut enc = Encoder::new(cfg).expect("encoder");
    let mut bits = 0usize;
    let mut qs = Vec::new();
    let mut dropped = 0;
    for t in 0..FRAMES {
        let src = frame(t);
        let pic = Picture::from_i420(&src, w, h).expect("picture");
        let out = enc.encode(&pic, false).expect("encode");
        if t < WARMUP {
            continue;
        }
        match out {
            Some(f) => {
                bits += f.data.len() * 8;
                qs.push(f.q_index);
            }
            None => dropped += 1,
        }
    }
    assert!(!qs.is_empty(), "every measured frame was dropped");
    qs.sort_unstable();
    Measured {
        kbps: bits as f64 / ((FRAMES - WARMUP) as f64 / FPS as f64) / 1000.0,
        q_min: qs[0],
        q_median: qs[qs.len() / 2],
        q_max: qs[qs.len() - 1],
        dropped,
    }
}

fn report(what: &str, w: u32, h: u32, layers: TemporalLayers, rate: u32, m: &Measured) -> f64 {
    let target = rate as f64 / 1000.0;
    let off = m.kbps / target - 1.0;
    eprintln!(
        "[bitrate] {what} {w}x{h} {layers:?} target {target:.0} kbit/s: got {:.0} kbit/s ({:+.1}%), q {}..{} (median {}), {} dropped",
        m.kbps,
        off * 100.0,
        m.q_min,
        m.q_max,
        m.q_median,
        m.dropped
    );
    off
}

/// Camera-like content lands within the tolerance at every simulcast size
/// and rate, with and without temporal layers.
#[test]
fn camera_content_hits_the_target() {
    let mut failures = Vec::new();
    for (w, h) in [(640u32, 360u32), (1280, 720)] {
        let cam = Camera::new(w as usize, h as usize, FRAMES);
        for layers in [TemporalLayers::L1T1, TemporalLayers::L1T3] {
            for rate in [300_000u32, 600_000, 1_200_000, 2_500_000] {
                let cfg = EncoderConfig::new(w, h, FPS as u32, rate).with_temporal_layers(layers);
                let m = measure(cfg, |t| cam.frame(t));
                let off = report("camera", w, h, layers, rate, &m);
                if off.abs() > TOLERANCE {
                    failures.push(format!(
                        "{w}x{h} {layers:?} at {} kbit/s: {:.0} kbit/s ({:+.1}%)",
                        rate / 1000,
                        m.kbps,
                        off * 100.0
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "off target by more than {:.0}%:\n{}",
        TOLERANCE * 100.0,
        failures.join("\n")
    );
}

/// The fake camera's pattern is mostly flat: it may undershoot, but only
/// at the quantizer floor (best quality), and it never overshoots.
#[test]
fn easy_content_undershoots_only_at_the_quantizer_floor() {
    let (w, h) = (640u32, 360u32);
    let mut failures = Vec::new();
    for layers in [TemporalLayers::L1T1, TemporalLayers::L1T3] {
        for rate in [300_000u32, 600_000, 1_200_000, 2_500_000] {
            let cfg = EncoderConfig::new(w, h, FPS as u32, rate).with_temporal_layers(layers);
            let m = measure(cfg, |t| fake_pattern(w as usize, h as usize, t));
            let off = report("fake pattern", w, h, layers, rate, &m);
            if off > TOLERANCE {
                failures.push(format!(
                    "{layers:?} at {} kbit/s: overshoot {:+.1}%",
                    rate / 1000,
                    off * 100.0
                ));
            }
            if off < -TOLERANCE && m.q_median > Q_FLOOR {
                failures.push(format!(
                    "{layers:?} at {} kbit/s: undershoot {:+.1}% with median q {} above the floor {Q_FLOOR}",
                    rate / 1000,
                    off * 100.0,
                    m.q_median
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
