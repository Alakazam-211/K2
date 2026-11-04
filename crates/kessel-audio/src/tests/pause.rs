//! Suspend and device pauses on the processing core: no frames while
//! paused, a closing frame after speech, and a timestamp gap that a
//! receiver reads as DTX rather than loss.

use super::{core_cfg, encode_remote, schedule, Net, FAR_ID};
use crate::apm::ProcessingConfig;
use crate::frame::{EncodedFrame, EncoderCfg, RecvFrame, BLOCK_SAMPLES, FRAME_SAMPLES};
use crate::jitter::{JitterBuffer, JitterConfig};
use crate::pipeline::{gap_ts48k, Core, Participant, Pause};
use crate::stats::ParticipantStats;
use std::time::{Duration, Instant};

const BLOCK: Duration = Duration::from_millis(10);

/// A voiced buzz, loud enough that every frame is speech.
fn buzz(n: usize, from: usize) -> Vec<f32> {
    (from..from + n)
        .map(|i| {
            let t = i as f64 / 48_000.0;
            let mut s = 0.0;
            for h in 1..8 {
                s += (2.0 * std::f64::consts::PI * 150.0 * h as f64 * t).sin() / h as f64;
            }
            (0.15 * s) as f32
        })
        .collect()
}

/// DTX off: a steady buzz can read as noise to Opus, and these tests count
/// frames. (The receiver tells a pause from loss by timestamps, whatever the
/// encoder's DTX setting.)
fn core_with(dtx: bool) -> Core {
    Core::new(crate::pipeline::CoreConfig {
        encoder: EncoderCfg {
            dtx,
            ..EncoderCfg::default()
        },
        ..core_cfg(ProcessingConfig::off())
    })
    .expect("core")
}

fn core() -> Core {
    core_with(false)
}

/// Capture `blocks` 10 ms blocks of buzz starting at block `b0`.
fn capture(core: &mut Core, b0: usize, blocks: usize, out: &mut Vec<EncodedFrame>) {
    for b in b0..b0 + blocks {
        let x = buzz(BLOCK_SAMPLES, b * BLOCK_SAMPLES);
        if let Some(f) = core.capture_block(&x) {
            out.push(f);
        }
    }
}

/// When each frame reaches the far end: 30 ms after its capture ended
/// (`ts48k` started at 0 and tracks the wall clock, gaps included).
fn arrivals(frames: &[EncodedFrame]) -> Vec<u64> {
    frames.iter().map(|f| (f.ts48k / 48) as u64 + 20 + 30).collect()
}

/// Play `frames` through a receiver's jitter buffer as a transport would:
/// consecutive sequence numbers, arriving at `arrive_ms`.
fn receive(frames: &[EncodedFrame], arrive_ms: &[u64]) -> ParticipantStats {
    let mut jb = JitterBuffer::new(JitterConfig::default());
    let t0 = Instant::now();
    let end_ms = arrive_ms.last().expect("frames") + 400;
    let mut next = 0usize;
    let mut tick = 0u64;
    while tick * 20 < end_ms {
        let now_ms = tick * 20;
        while next < frames.len() && arrive_ms[next] <= now_ms {
            let f = &frames[next];
            jb.insert(next as u16, f.ts48k, f.payload.clone(), t0 + Duration::from_millis(arrive_ms[next]));
            next += 1;
        }
        let _ = jb.next(t0 + Duration::from_millis(now_ms));
        tick += 1;
    }
    assert_eq!(next, frames.len(), "every frame was delivered");
    jb.stats()
}

/// One second of speech, a two-second suspend, one more second.
fn suspended_call() -> (Vec<EncodedFrame>, usize) {
    let mut c = core();
    let t0 = Instant::now();
    let mut frames = Vec::new();
    capture(&mut c, 0, 100, &mut frames);
    assert_eq!(frames.len(), 50);
    let tail = c
        .set_paused(Pause::Caller, true, t0 + BLOCK * 100)
        .expect("a pause after speech closes the talkspurt");
    frames.push(tail);
    let before = frames.len();
    capture(&mut c, 100, 200, &mut frames);
    assert_eq!(frames.len(), before, "no frames while suspended");
    assert!(c.set_paused(Pause::Caller, false, t0 + BLOCK * 300).is_none());
    let resumed_at = frames.len();
    capture(&mut c, 300, 100, &mut frames);
    assert_eq!(frames.len(), resumed_at + 50);
    (frames, resumed_at)
}

#[test]
fn a_suspend_reads_as_dtx_at_the_far_end_not_loss() {
    let (frames, resumed_at) = suspended_call();
    let tail = &frames[resumed_at - 1];
    assert!(tail.dtx && !tail.vad, "{tail:?}");
    assert_eq!(tail.ts48k, 50 * FRAME_SAMPLES as u32);
    // The first frame after the resume: the tail's slot, plus 2 s.
    assert_eq!(
        frames[resumed_at].ts48k,
        51 * FRAME_SAMPLES as u32 + gap_ts48k(Duration::from_secs(2))
    );
    assert_eq!(gap_ts48k(Duration::from_secs(2)), 100 * FRAME_SAMPLES as u32);
    let st = receive(&frames, &arrivals(&frames));
    assert_eq!(st.loss_pct, 0.0, "{st:?}");
    assert_eq!(st.frames_concealed, 0, "{st:?}");
    assert!(st.frames_dtx >= 100, "the 2 s gap is DTX: {st:?}");
}

#[test]
fn without_the_timestamp_gap_the_same_pause_is_concealment() {
    // The control: what the far end would hear if the timestamp didn't jump.
    let (mut frames, resumed_at) = suspended_call();
    let at = arrivals(&frames);
    let jump = gap_ts48k(Duration::from_secs(2));
    for f in &mut frames[resumed_at..] {
        f.ts48k = f.ts48k.wrapping_sub(jump);
    }
    let st = receive(&frames, &at);
    assert!(st.frames_concealed >= 50, "a stall is concealment: {st:?}");
}

#[test]
fn the_gap_rounds_up_to_whole_frames() {
    assert_eq!(gap_ts48k(Duration::ZERO), 0);
    assert_eq!(gap_ts48k(Duration::from_millis(1)), 960);
    assert_eq!(gap_ts48k(Duration::from_millis(20)), 960);
    assert_eq!(gap_ts48k(Duration::from_millis(21)), 1920);
}

#[test]
fn no_closing_frame_when_muted_or_already_silent() {
    let t0 = Instant::now();
    let mut c = core();
    let mut frames = Vec::new();
    capture(&mut c, 0, 50, &mut frames);
    c.set_capture(false);
    capture(&mut c, 50, 2, &mut frames);
    assert!(c.set_paused(Pause::Caller, true, t0).is_none(), "muted");

    let mut c = core_with(true);
    // DTX: two seconds of silence and the encoder sends nothing.
    for _ in 0..200 {
        let _ = c.capture_block(&[0.0; BLOCK_SAMPLES]);
    }
    assert!(c.set_paused(Pause::Caller, true, t0).is_none(), "already in DTX");

    let mut c = core();
    for _ in 0..4 {
        let _ = c.capture_block(&[0.0; BLOCK_SAMPLES]);
    }
    assert!(c.set_paused(Pause::Caller, true, t0).is_none(), "silence, not speech");
}

#[test]
fn a_pause_lasts_while_any_reason_holds() {
    let t0 = Instant::now();
    let mut c = core();
    let mut frames = Vec::new();
    capture(&mut c, 0, 20, &mut frames);
    let ts_end = 10 * FRAME_SAMPLES as u32;
    assert!(c.set_paused(Pause::Device, true, t0 + BLOCK * 20).is_some());
    assert!(c.set_paused(Pause::Caller, true, t0 + BLOCK * 30).is_none());
    assert!(c.set_paused(Pause::Device, false, t0 + BLOCK * 40).is_none());
    assert!(c.is_paused());
    let n = frames.len();
    capture(&mut c, 40, 20, &mut frames);
    assert_eq!(frames.len(), n, "still suspended");
    assert!(c.set_paused(Pause::Caller, false, t0 + BLOCK * 60).is_none());
    assert!(!c.is_paused());
    frames.clear();
    capture(&mut c, 60, 2, &mut frames);
    // Measured from the first pause (block 20) to the last resume (block
    // 60): 400 ms, after the tail's slot.
    assert_eq!(
        frames[0].ts48k,
        ts_end + FRAME_SAMPLES as u32 + gap_ts48k(Duration::from_millis(400))
    );
}

#[test]
fn nothing_captured_yet_means_no_gap() {
    // A deferred start: paused from the beginning, then the device opens.
    let t0 = Instant::now();
    let mut c = core();
    assert!(c.set_paused(Pause::Device, true, t0).is_none());
    assert!(c.set_paused(Pause::Device, false, t0 + Duration::from_secs(5)).is_none());
    let mut frames = Vec::new();
    capture(&mut c, 500, 2, &mut frames);
    assert_eq!(frames[0].ts48k, 0, "the first frame keeps initial_ts48k");
}

#[test]
fn suspend_silences_playback_and_a_device_pause_does_not() {
    let cfg = core_cfg(ProcessingConfig::off());
    let mut c = Core::new(cfg.clone()).expect("core");
    c.add(Participant::new(FAR_ID, &cfg).expect("participant"))
        .unwrap_or_else(|_| panic!("add"));
    let far = buzz(48_000 * 4, 0);
    let enc = EncoderCfg {
        dtx: false,
        ..EncoderCfg::default()
    };
    let sched = schedule(&encode_remote(&far, enc), Net { base_ms: 30.0, ..Net::default() });
    let t0 = Instant::now();
    let mut next = 0;
    let mut energy = |c: &mut Core, b0: usize, blocks: usize| -> f64 {
        let mut e = 0.0f64;
        let mut out = [0.0f32; BLOCK_SAMPLES];
        for b in b0..b0 + blocks {
            let now_us = b as u64 * 10_000;
            while next < sched.len() && sched[next].0 <= now_us {
                let (at, p) = &sched[next];
                c.insert(RecvFrame {
                    participant: FAR_ID,
                    payload: p.payload.clone(),
                    seq: p.seq,
                    ts48k: p.ts,
                    arrival: t0 + Duration::from_micros(*at),
                });
                next += 1;
            }
            c.render_block(t0 + Duration::from_micros(now_us), &mut out);
            e += out.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>();
        }
        e
    };
    assert!(energy(&mut c, 0, 100) > 1.0, "the far end plays");
    let _ = c.set_paused(Pause::Caller, true, t0 + BLOCK * 100);
    assert_eq!(energy(&mut c, 100, 50), 0.0, "suspended: silence");
    let mut levels = crate::events::Levels::default();
    c.fill_levels(&mut levels);
    assert!(levels.participants[0].1.level_dbov > -60.0, "still decoded: {levels:?}");
    assert_eq!(levels.local.level_dbov, -127.0);
    let _ = c.set_paused(Pause::Caller, false, t0 + BLOCK * 150);
    let _ = c.set_paused(Pause::Device, true, t0 + BLOCK * 150);
    assert!(energy(&mut c, 150, 50) > 1.0, "a device pause still plays");
}
