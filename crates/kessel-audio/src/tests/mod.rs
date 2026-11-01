//! Offline pipeline tests: the processing core driven block by block with a
//! simulated clock, fed by synthetic talkers through the baseline's echo model.

mod alloc;
mod metrics;
mod receive;

use crate::apm::ProcessingConfig;
use crate::codec::{OpusDec, OpusEnc};
use crate::frame::{EncoderCfg, RecvFrame, BLOCK_SAMPLES, FRAME_SAMPLES};
use crate::jitter::JitterConfig;
use crate::pipeline::{Core, CoreConfig, Participant};
use crate::stats::Stats;
use crate::testkit::Rng;
use bytes::Bytes;
use std::time::{Duration, Instant};

pub(crate) const FAR_ID: u64 = 1;

/// CPU-heavy tests share this lock; the CPU measurement takes it
/// exclusively so it times the pipeline, not the other tests.
static HEAVY: std::sync::RwLock<()> = std::sync::RwLock::new(());

pub(crate) fn shared_cpu() -> std::sync::RwLockReadGuard<'static, ()> {
    HEAVY.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn exclusive_cpu() -> std::sync::RwLockWriteGuard<'static, ()> {
    HEAVY.write().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn core_cfg(processing: ProcessingConfig) -> CoreConfig {
    CoreConfig {
        processing,
        encoder: EncoderCfg::default(),
        jitter: JitterConfig::default(),
        initial_ts48k: 0,
        levels_interval_ms: 50,
        speaking_threshold_dbov: -45.0,
        max_participants: 8,
    }
}

/// Echo cancellation and the high-pass filter only (the baseline's main config).
pub(crate) fn aec_only() -> ProcessingConfig {
    ProcessingConfig {
        echo_cancellation: true,
        noise_suppression: None,
        gain_control: false,
        high_pass: true,
    }
}

#[derive(Clone)]
pub(crate) struct Packet {
    pub(crate) seq: u16,
    pub(crate) ts: u32,
    pub(crate) payload: Bytes,
}

/// Encode a far-end signal the way a remote sender would: 20 ms frames,
/// DTX frames with nothing to send are skipped (sequence numbers count
/// sent packets only).
pub(crate) fn encode_remote(x: &[f32], cfg: EncoderCfg) -> Vec<Packet> {
    let mut enc = OpusEnc::new(cfg).expect("encoder");
    let mut seq = 0u16;
    let mut out = Vec::new();
    for (i, f) in x.as_chunks::<FRAME_SAMPLES>().0.iter().enumerate() {
        let e = enc.encode(f).expect("encode");
        if cfg.dtx && e.is_empty_dtx() {
            continue;
        }
        out.push(Packet {
            seq,
            ts: (i * FRAME_SAMPLES) as u32,
            payload: Bytes::copy_from_slice(e.packet),
        });
        seq = seq.wrapping_add(1);
    }
    out
}

/// Network conditions for a packet schedule.
#[derive(Clone, Copy, Default)]
pub(crate) struct Net {
    pub(crate) base_ms: f64,
    pub(crate) jitter_ms: f64,
    pub(crate) loss: f64,
    pub(crate) reorder: f64,
    pub(crate) seed: u64,
}

/// Arrival times (µs after the call starts), sorted by arrival.
pub(crate) fn schedule(packets: &[Packet], net: Net) -> Vec<(u64, Packet)> {
    let mut rng = Rng::new(net.seed);
    let mut v: Vec<(u64, Packet)> = Vec::new();
    for p in packets {
        if rng.uniform() < net.loss {
            continue;
        }
        let mut ms = p.ts as f64 / 48.0 + net.base_ms + rng.uniform() * net.jitter_ms;
        if rng.uniform() < net.reorder {
            // Held back by one to two packet times.
            ms += rng.range(20.0, 40.0);
        }
        v.push(((ms * 1000.0) as u64, p.clone()));
    }
    v.sort_by_key(|(t, _)| *t);
    v
}

pub(crate) struct CallRun {
    /// What was played (and fed to the echo canceller).
    pub(crate) rendered: Vec<f32>,
    /// Our encoded capture, decoded by a remote decoder, on the capture
    /// timeline (DTX gaps concealed as a receiver would).
    pub(crate) out: Vec<f32>,
    /// Wall time per 10 ms block (render + capture).
    pub(crate) block_ns: Vec<u64>,
    pub(crate) stats: Stats,
}

/// Drive a core: the far-end schedule is received as participant FAR_ID;
/// `mic` (if any) is captured block by block, render first, as the engine
/// does.
pub(crate) fn run_core(
    cfg: &CoreConfig,
    sched: &[(u64, Packet)],
    n_samples: usize,
    mic: Option<&[f32]>,
) -> CallRun {
    run_core_inner(cfg, sched, n_samples, mic)
}

/// [`run_core`]'s body (the CPU test calls it while holding the lock).
pub(crate) fn run_core_inner(
    cfg: &CoreConfig,
    sched: &[(u64, Packet)],
    n_samples: usize,
    mic: Option<&[f32]>,
) -> CallRun {
    let mut core = Core::new(cfg.clone()).expect("core");
    core.add(Participant::new(FAR_ID, cfg).expect("participant"))
        .unwrap_or_else(|_| panic!("add participant"));
    let t0 = Instant::now();
    let n_blocks = n_samples / BLOCK_SAMPLES;
    let mut rendered = vec![0.0f32; n_blocks * BLOCK_SAMPLES];
    let mut frames: Vec<(u32, Bytes)> = Vec::new();
    let mut next = 0usize;
    let mut block_ns = Vec::with_capacity(n_blocks);
    let mut out_block = [0.0f32; BLOCK_SAMPLES];
    for b in 0..n_blocks {
        let now_us = b as u64 * 10_000;
        let now = t0 + Duration::from_micros(now_us);
        while next < sched.len() && sched[next].0 <= now_us {
            let (at, p) = &sched[next];
            core.insert(RecvFrame {
                participant: FAR_ID,
                payload: p.payload.clone(),
                seq: p.seq,
                ts48k: p.ts,
                arrival: t0 + Duration::from_micros(*at),
            });
            next += 1;
        }
        let r = b * BLOCK_SAMPLES..(b + 1) * BLOCK_SAMPLES;
        let t = Instant::now();
        core.render_block(now, &mut out_block);
        rendered[r.clone()].copy_from_slice(&out_block);
        if let Some(m) = mic {
            if let Some(f) = core.capture_block(&m[r]) {
                frames.push((f.ts48k, f.payload));
            }
        }
        block_ns.push(t.elapsed().as_nanos() as u64);
    }
    let mut stats = Stats::default();
    core.fill_stats(&mut stats);
    let out = decode_capture(&frames, n_blocks * BLOCK_SAMPLES);
    CallRun {
        rendered,
        out,
        block_ns,
        stats,
    }
}

/// Decode captured frames onto the capture timeline, concealing gaps.
pub(crate) fn decode_capture(frames: &[(u32, Bytes)], n: usize) -> Vec<f32> {
    let mut dec = OpusDec::new().expect("decoder");
    let mut out = vec![0.0f32; n];
    let mut pos = 0usize;
    for (ts, p) in frames {
        let ts = *ts as usize;
        while pos + FRAME_SAMPLES <= ts.min(n) {
            dec.conceal(&mut out[pos..pos + FRAME_SAMPLES]);
            pos += FRAME_SAMPLES;
        }
        if ts + FRAME_SAMPLES > n {
            break;
        }
        dec.decode(p, &mut out[ts..ts + FRAME_SAMPLES]);
        pos = ts + FRAME_SAMPLES;
    }
    out
}
