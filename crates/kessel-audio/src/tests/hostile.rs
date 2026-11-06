//! Hostile input on the receive path: random bytes, mutated and truncated
//! Opus packets, and nonsense sequence numbers, timestamps and arrival
//! times. The receive path must not panic, and what it plays must stay
//! finite and within the limiter's bounds.
//!
//! This is a seeded random test, not a coverage-guided fuzzer: it runs the
//! same few thousand inputs every time.

use super::{core_cfg, encode_remote, FAR_ID};
use crate::apm::ProcessingConfig;
use crate::codec::{has_lbrr, OpusDec};
use crate::frame::{EncoderCfg, RecvFrame, BLOCK_SAMPLES, FRAME_SAMPLES};
use crate::pipeline::{Core, Participant};
use crate::stats::Stats;
use crate::testkit::Rng;
use bytes::Bytes;
use std::time::{Duration, Instant};

/// Valid Opus packets of speech-like audio, to mutate.
fn valid_packets() -> Vec<Bytes> {
    let x: Vec<f32> = (0..48_000 * 2)
        .map(|i| {
            let t = i as f32 / 48_000.0;
            0.2 * (2.0 * std::f32::consts::PI * 180.0 * t).sin()
                * (0.6 + 0.4 * (2.0 * std::f32::consts::PI * 3.0 * t).sin())
        })
        .collect();
    let cfg = EncoderCfg { dtx: false, ..EncoderCfg::default() };
    encode_remote(&x, cfg).into_iter().map(|p| p.payload).collect()
}

/// One hostile payload: random bytes, or a valid packet with bits flipped,
/// truncated, extended or with a rewritten first (TOC) byte.
fn hostile_payload(rng: &mut Rng, valid: &[Bytes]) -> Bytes {
    let base = &valid[rng.below(valid.len())];
    let mut p = base.to_vec();
    match rng.below(6) {
        0 => {
            let n = rng.below(1500);
            p = (0..n).map(|_| rng.next_u64() as u8).collect();
        }
        1 => {
            for _ in 0..1 + rng.below(8) {
                if !p.is_empty() {
                    let i = rng.below(p.len());
                    p[i] ^= 1 << rng.below(8);
                }
            }
        }
        2 => p.truncate(rng.below(p.len() + 1)),
        3 => p.extend((0..rng.below(1200)).map(|_| rng.next_u64() as u8)),
        4 => {
            if !p.is_empty() {
                p[0] = rng.next_u64() as u8;
            }
        }
        _ => p.clear(),
    }
    Bytes::from(p)
}

#[test]
fn the_decoder_survives_hostile_packets() {
    let valid = valid_packets();
    let mut rng = Rng::new(0xbad_cafe);
    let mut dec = OpusDec::new().expect("decoder");
    let mut out = vec![0.0f32; FRAME_SAMPLES];
    for i in 0..3_000 {
        let p = hostile_payload(&mut rng, &valid);
        let _ = has_lbrr(&p);
        match i % 3 {
            0 => {
                dec.decode(&p, &mut out);
            }
            1 => {
                dec.decode_fec(&p, &mut out);
            }
            _ => dec.conceal(&mut out),
        }
        assert!(out.iter().all(|v| v.is_finite()), "packet {i} decoded to a non-finite sample");
    }
    // And it still decodes a good packet afterwards.
    assert_eq!(dec.decode(&valid[10], &mut out), crate::codec::Decoded::Packet);
}

#[test]
fn the_receive_path_survives_hostile_frames() {
    let valid = valid_packets();
    let mut rng = Rng::new(0x5eed);
    let cfg = core_cfg(ProcessingConfig::off());
    let mut core = Core::new(cfg.clone()).expect("core");
    core.add(Participant::new(FAR_ID, &cfg).expect("participant"))
        .unwrap_or_else(|_| panic!("add participant"));
    let t0 = Instant::now();
    let mut seq = 0u16;
    let mut ts = 0u32;
    let mut out = [0.0f32; BLOCK_SAMPLES];
    let mut inserted = 0u64;
    for b in 0..1_500u64 {
        let now = t0 + Duration::from_millis(b * 10);
        // Zero to three frames per 10 ms block, every field suspect.
        for _ in 0..rng.below(4) {
            match rng.below(5) {
                // Sequence numbers and timestamps anywhere.
                0 => {
                    seq = rng.next_u64() as u16;
                    ts = rng.next_u64() as u32;
                }
                // Backwards, repeated or wrapping.
                1 => {
                    seq = seq.wrapping_sub(rng.below(40) as u16);
                    ts = ts.wrapping_sub((rng.below(40) * FRAME_SAMPLES) as u32);
                }
                // Timestamps that disagree with the sequence numbers.
                2 => {
                    seq = seq.wrapping_add(1);
                    ts = ts.wrapping_add(rng.next_u64() as u32 % 100_000);
                }
                _ => {
                    seq = seq.wrapping_add(1);
                    ts = ts.wrapping_add(FRAME_SAMPLES as u32);
                }
            }
            // Arrivals from the past, the present or the future.
            let skew_ms = rng.below(4_000) as i64 - 2_000;
            let arrival = if skew_ms < 0 {
                now.checked_sub(Duration::from_millis(-skew_ms as u64)).unwrap_or(now)
            } else {
                now + Duration::from_millis(skew_ms as u64)
            };
            let payload = if rng.below(3) == 0 {
                valid[rng.below(valid.len())].clone()
            } else {
                hostile_payload(&mut rng, &valid)
            };
            core.insert(RecvFrame { participant: FAR_ID, payload, seq, ts48k: ts, arrival });
            inserted += 1;
            if rng.below(20) == 0 {
                core.lost(FAR_ID, rng.next_u64() as u16);
            }
        }
        core.render_block(now, &mut out);
        assert!(
            out.iter().all(|v| v.is_finite() && v.abs() <= 1.0),
            "block {b} played a sample out of bounds"
        );
    }
    assert!(inserted > 1_000, "only {inserted} frames inserted");
    let mut stats = Stats::default();
    core.fill_stats(&mut stats);
    let (_, p) = stats
        .participants
        .iter()
        .find(|(id, _)| *id == FAR_ID)
        .expect("participant stats");
    for (name, v) in [
        ("jitter_ms", p.jitter_ms),
        ("loss_pct", p.loss_pct),
        ("concealed_pct", p.concealed_pct),
        ("jb_delay_ms", p.jb_delay_ms),
        ("jb_target_ms", p.jb_target_ms),
    ] {
        assert!(v.is_finite(), "{name} = {v}");
    }
    assert!(p.packets_received > 0, "nothing was received");
}
