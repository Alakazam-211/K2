//! The capture stage on the processing core: where it sits, the bypass
//! (the echo canceller's output, delayed by the stage's latency, bit for
//! bit), the budget rules, faults, retries and resets.

use super::stages::{PassThrough, Scripted};
use super::{aec_only, core_cfg};
use crate::apm::{
    Apm, ApmSettings, NoiseSuppressionStrength, ProcessingConfig, STEP_DOWN_NS_DELAY,
};
use crate::error::ErrorCode;
use crate::frame::{EncoderCfg, BLOCK_SAMPLES};
use crate::pipeline::{Core, CoreConfig};
use crate::stage::{CaptureStage, CaptureStageStats, StageBypassReason, RETRY_BLOCKS};
use crate::stats::Stats;
use crate::testkit::{add_into, delay, floor_noise, scale, talker_track, Rng, TALKERS};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};

const B: usize = BLOCK_SAMPLES;
/// Enough for two retries and then some.
const BLOCKS: usize = 2_500;

/// Far-end speech and a microphone with its echo (60 ms, -12 dB), a second
/// talker and a noise floor. Built once.
fn signals() -> &'static (Vec<f32>, Vec<f32>) {
    static S: OnceLock<(Vec<f32>, Vec<f32>)> = OnceLock::new();
    S.get_or_init(|| {
        let n = BLOCKS * B;
        let mut rng = Rng::new(9100);
        let far = talker_track(&TALKERS[0], &mut rng, n);
        let mut near = talker_track(&TALKERS[1], &mut rng, n);
        scale(&mut near, 0.5);
        let mut echo = delay(&far, 60 * 48);
        scale(&mut echo, 0.25);
        let mut mic = floor_noise(&mut rng, n, -72.0);
        add_into(&mut mic, &echo);
        add_into(&mut mic, &near);
        (far, mic)
    })
}

/// Run `blocks` blocks through `apm` (render first, as the engine does);
/// `before(b, apm)` runs before block `b`.
fn run(apm: &mut Apm, blocks: usize, mut before: impl FnMut(usize, &mut Apm)) -> Vec<f32> {
    let (far, mic) = signals();
    let mut out = vec![0.0f32; blocks * B];
    for b in 0..blocks {
        before(b, apm);
        let r = b * B..(b + 1) * B;
        apm.render(&far[r.clone()]);
        apm.capture(&mic[r.clone()], &mut out[r]);
    }
    out
}

/// The same run with no stage: today's output.
fn plain(settings: ApmSettings, blocks: usize) -> Vec<f32> {
    run(&mut Apm::new(settings), blocks, |_, _| {})
}

/// `staged` is `plain`, `latency` samples later, bit for bit.
fn assert_delayed(staged: &[f32], plain: &[f32], latency: usize, what: &str) {
    assert_eq!(staged.len(), plain.len());
    for (i, v) in staged.iter().enumerate() {
        let want = if i >= latency {
            plain[i - latency]
        } else {
            0.0
        };
        assert!(
            v.to_bits() == want.to_bits(),
            "{what}: sample {i} (block {}) is {v}, the delayed echo canceller output is {want}",
            i / B
        );
    }
}

fn stage_stats(apm: &mut Apm) -> CaptureStageStats {
    apm.stage_stats().expect("a stage is set")
}

fn staged(settings: ApmSettings, stage: impl CaptureStage) -> Apm {
    Apm::with_stage(settings, Box::new(stage)).expect("stage accepted")
}

/// No processing, latency 0.
struct Identity;

impl CaptureStage for Identity {
    fn process(&mut self, _: &mut [f32; B]) {}
    fn latency_samples(&self) -> usize {
        0
    }
    fn reset(&mut self) {}
}

/// Records what it is given; no processing, latency 0.
struct Probe(Arc<Mutex<Vec<f32>>>);

impl CaptureStage for Probe {
    fn process(&mut self, block: &mut [f32; B]) {
        self.0
            .lock()
            .expect("probe log")
            .extend_from_slice(&block[..]);
    }
    fn latency_samples(&self) -> usize {
        0
    }
    fn reset(&mut self) {}
}

/// `p` without the step-down suppressor: a bypass gives exactly the delayed
/// echo canceller output (the step-down has its own tests below).
fn settings(p: ProcessingConfig) -> ApmSettings {
    ApmSettings {
        stage_fallback_ns: None,
        ..p.settings()
    }
}

#[test]
fn the_stage_gets_the_signal_after_echo_cancellation_and_before_gain_control() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let full = settings(ProcessingConfig::default());
    let mut apm = staged(full, Probe(log.clone()));
    let _ = run(&mut apm, 300, |_, _| {});
    // The high-pass filter, echo cancellation and noise suppression; no AGC2.
    let before_agc = plain(
        ApmSettings {
            gain_control: false,
            ..full
        },
        300,
    );
    let got = log.lock().expect("probe log");
    assert_eq!(got.len(), before_agc.len());
    let same = got
        .iter()
        .zip(&before_agc)
        .all(|(a, b)| a.to_bits() == b.to_bits());
    assert!(same, "the stage's input is not the pre-AGC2 signal");
}

#[test]
fn a_pass_through_stage_gives_the_echo_canceller_output_delayed() {
    let mut apm = staged(settings(aec_only()), PassThrough::new());
    let out = run(&mut apm, 500, |_, _| {});
    assert_delayed(&out, &plain(settings(aec_only()), 500), B, "pass-through");
    let s = stage_stats(&mut apm);
    assert!(s.active && s.bypassed.is_none() && s.strikes == 0, "{s:?}");
    assert_eq!(s.latency_samples, 480);
    assert_eq!(s.blocks_processed, 500);
    assert_eq!(s.blocks_bypassed, 0);
}

#[test]
fn a_stage_with_no_latency_and_no_change_changes_nothing() {
    let mut apm = staged(settings(aec_only()), Identity);
    let out = run(&mut apm, 300, |_, _| {});
    assert_delayed(&out, &plain(settings(aec_only()), 300), 0, "identity");
}

#[test]
fn a_9_ms_stage_is_bypassed_at_once_retried_twice_then_bypassed_for_good() {
    let (stage, script) = Scripted::new();
    let mut apm = staged(settings(aec_only()), stage);
    let retry = RETRY_BLOCKS as usize;
    // Block 100 is the first slow one; the stage stays slow.
    let (s1, s2, s3) = (100, 100 + retry, 100 + 2 * retry);
    let mut seen: Vec<(usize, CaptureStageStats)> = Vec::new();
    let out = run(&mut apm, BLOCKS, |b, apm| {
        if b == s1 {
            script.sleep_us.store(9_000, Ordering::Relaxed);
        }
        if [s1, s1 + 1, s2 - 1, s2 + 1, s3 + 1].contains(&b) {
            seen.push((b, stage_stats(apm)));
        }
    });
    assert_delayed(&out, &plain(settings(aec_only()), BLOCKS), B, "9 ms stage");

    let at = |b: usize| seen.iter().find(|(x, _)| *x == b).expect("reading").1;
    assert!(at(s1).active && at(s1).strikes == 0, "{:?}", at(s1));
    // The bypass flag is set by the slow block itself.
    let a = at(s1 + 1);
    assert!(
        !a.active && a.bypassed == Some(StageBypassReason::Budget) && a.strikes == 1,
        "{a:?}"
    );
    assert!(a.max_block_us >= 9_000, "{a:?}");
    assert!(!a.permanent);
    let a = at(s2 - 1);
    assert!(
        !a.active && a.strikes == 1,
        "still waiting for the retry: {a:?}"
    );
    let a = at(s2 + 1);
    assert!(!a.active && a.strikes == 2 && !a.permanent, "{a:?}");
    let a = at(s3 + 1);
    assert!(!a.active && a.strikes == 3 && a.permanent, "{a:?}");

    // 100 good calls, then one per strike; reset once before the first
    // call and before each retry.
    assert_eq!(script.calls.load(Ordering::Relaxed), 103);
    assert_eq!(script.resets.load(Ordering::Relaxed), 3);
    let s = stage_stats(&mut apm);
    assert_eq!(s.blocks_processed, 103);
    assert_eq!(s.blocks_bypassed, BLOCKS as u64 - 100);
}

#[test]
fn a_bypassed_stage_comes_back_after_10_s_without_a_seam() {
    let (stage, script) = Scripted::new();
    let mut apm = staged(settings(aec_only()), stage);
    let retry = RETRY_BLOCKS as usize;
    let mut seen: Vec<(usize, CaptureStageStats)> = Vec::new();
    let blocks = 100 + retry + 300;
    let out = run(&mut apm, blocks, |b, apm| {
        match b {
            100 => script.sleep_us.store(9_000, Ordering::Relaxed),
            101 => script.sleep_us.store(0, Ordering::Relaxed),
            _ => {}
        }
        if b == 101 || b == 100 + retry || b == 100 + retry + 1 {
            seen.push((b, stage_stats(apm)));
        }
    });
    // Before, during and after the bypass, and through the warm-up after the
    // retry: always the echo canceller's output, 480 samples late.
    assert_delayed(
        &out,
        &plain(settings(aec_only()), blocks),
        B,
        "retried stage",
    );
    let (_, a) = seen[0];
    assert!(
        !a.active && a.bypassed == Some(StageBypassReason::Budget),
        "{a:?}"
    );
    let (_, a) = seen[1];
    assert!(
        !a.active && a.blocks_processed == 101,
        "not retried yet: {a:?}"
    );
    // The retry block resets the stage and runs it to fill its latency
    // (the delayed input goes on for that block); its output is used from
    // the next block.
    let (_, a) = seen[2];
    assert_eq!(a.blocks_processed, 102);
    assert!(a.active, "back: {a:?}");
    assert_eq!(
        a.bypassed,
        Some(StageBypassReason::Budget),
        "the flag is sticky"
    );
    assert_eq!(a.strikes, 1);
    assert!(!a.permanent);
    // Before the first block, then before the retry.
    assert_eq!(script.resets.load(Ordering::Relaxed), 2);
}

#[test]
fn two_calls_over_6_ms_in_100_are_a_strike_and_one_is_not() {
    let (stage, script) = Scripted::new();
    script.spin_every.store(50, Ordering::Relaxed);
    script.spin_us.store(6_300, Ordering::Relaxed);
    let mut apm = staged(settings(ProcessingConfig::off()), stage);
    let mut first_strike = None;
    let _ = run(&mut apm, 300, |b, apm| {
        if first_strike.is_none() && stage_stats(apm).strikes > 0 {
            first_strike = Some(b);
        }
    });
    // Calls 50 and 100 are slow; the window is full at call 100 (block 99),
    // so the strike shows before block 100.
    assert_eq!(first_strike, Some(100));
    let s = stage_stats(&mut apm);
    assert_eq!(s.bypassed, Some(StageBypassReason::Budget));
    assert_eq!(s.strikes, 1);

    let (stage, script) = Scripted::new();
    script.spin_every.store(100, Ordering::Relaxed);
    script.spin_us.store(6_300, Ordering::Relaxed);
    let mut apm = staged(settings(ProcessingConfig::off()), stage);
    let _ = run(&mut apm, 450, |_, _| {});
    let s = stage_stats(&mut apm);
    assert!(s.active && s.strikes == 0, "one slow call in 100: {s:?}");
    assert!(s.max_block_us >= 6_300, "{s:?}");
    // Nearest rank over 100 calls: one outlier is not the 99th percentile.
    assert!(s.p99_block_us < 6_000, "{s:?}");
}

fn fault(script_setup: impl FnOnce(&super::stages::Script), what: &str) {
    let (stage, script) = Scripted::new();
    script_setup(&script);
    let mut apm = staged(settings(aec_only()), stage);
    let out = run(&mut apm, 300, |_, _| {});
    assert_delayed(&out, &plain(settings(aec_only()), 300), B, what);
    let s = stage_stats(&mut apm);
    assert!(
        !s.active && s.bypassed == Some(StageBypassReason::Fault) && s.permanent,
        "{what}: {s:?}"
    );
    assert_eq!(s.strikes, 1, "{what}");
    assert_eq!(s.blocks_bypassed, 300 - 49, "{what}");
    assert_eq!(
        script.calls.load(Ordering::Relaxed),
        50,
        "{what}: the stage is never called again"
    );
}

#[test]
fn a_panic_is_a_fault_with_no_retry() {
    fault(|s| s.panic_on.store(50, Ordering::Relaxed), "panic");
}

#[test]
fn a_nan_is_a_fault_with_no_retry() {
    fault(|s| s.nan_on.store(50, Ordering::Relaxed), "NaN");
}

#[test]
fn a_panic_in_reset_is_a_fault() {
    let (stage, script) = Scripted::new();
    let mut apm = staged(settings(aec_only()), stage);
    // After the first reset (the one before any audio), which went well.
    script.panic_on_reset.store(true, Ordering::Relaxed);
    let out = run(&mut apm, 200, |b, apm| {
        if b == 100 {
            apm.reset_stage();
        }
    });
    let s = stage_stats(&mut apm);
    assert!(
        s.bypassed == Some(StageBypassReason::Fault) && s.permanent,
        "{s:?}"
    );
    assert_eq!(script.calls.load(Ordering::Relaxed), 100);
    // The reset cleared the delay line too, so block 100 is silence and the
    // delayed output resumes after it.
    let p = plain(settings(aec_only()), 200);
    assert!(out[100 * B..101 * B].iter().all(|&v| v == 0.0));
    assert_delayed(&out[101 * B..], &p[100 * B..199 * B], 0, "after reset");
}

#[test]
fn a_reset_clears_the_stage_and_it_runs_on() {
    let (stage, script) = Scripted::new();
    let mut apm = staged(settings(aec_only()), stage);
    let out = run(&mut apm, 300, |b, apm| {
        if b == 100 {
            apm.reset_stage();
        }
    });
    assert_eq!(script.resets.load(Ordering::Relaxed), 2);
    assert_eq!(script.calls.load(Ordering::Relaxed), 300);
    let s = stage_stats(&mut apm);
    assert!(s.active && s.strikes == 0, "{s:?}");
    let p = plain(settings(aec_only()), 300);
    assert!(out[100 * B..101 * B].iter().all(|&v| v == 0.0));
    assert_delayed(&out[101 * B..], &p[100 * B..299 * B], 0, "after reset");
}

#[test]
fn a_reset_of_a_bypassed_stage_waits_for_the_retry() {
    let (stage, script) = Scripted::new();
    let mut apm = staged(settings(ProcessingConfig::off()), stage);
    let _ = run(&mut apm, 200, |b, apm| match b {
        50 => script.sleep_us.store(9_000, Ordering::Relaxed),
        51 => script.sleep_us.store(0, Ordering::Relaxed),
        100 => apm.reset_stage(),
        _ => {}
    });
    // Only the one before the first block.
    assert_eq!(script.resets.load(Ordering::Relaxed), 1);
    assert!(!stage_stats(&mut apm).active);
}

/// A stage that is only ready after `reset` (it prepares itself there, as
/// a stage that plans its FFTs might): the engine resets it once before the
/// first block, and not again until something was processed.
#[test]
fn the_stage_is_reset_once_before_the_first_block() {
    struct NeedsReset {
        ready: bool,
        resets: Arc<Mutex<u32>>,
    }
    impl CaptureStage for NeedsReset {
        fn process(&mut self, _: &mut [f32; B]) {
            assert!(self.ready, "process before reset");
        }
        fn latency_samples(&self) -> usize {
            0
        }
        fn reset(&mut self) {
            self.ready = true;
            *self.resets.lock().expect("count") += 1;
        }
    }
    let resets = Arc::new(Mutex::new(0));
    let mut apm = staged(
        settings(ProcessingConfig::off()),
        NeedsReset {
            ready: false,
            resets: resets.clone(),
        },
    );
    assert_eq!(*resets.lock().expect("count"), 1, "reset when built");
    // A stream start before any audio: nothing to clear.
    apm.reset_stage();
    assert_eq!(*resets.lock().expect("count"), 1);
    let _ = run(&mut apm, 50, |_, _| {});
    let s = stage_stats(&mut apm);
    assert!(s.active && s.bypassed.is_none(), "{s:?}");
    apm.reset_stage();
    assert_eq!(*resets.lock().expect("count"), 2);
}

/// The step-down suppressor's delay is what the code assumes: its output
/// lines up with the stage's (cross-correlation of its input and output on
/// speech, where it passes speech nearly unchanged).
#[test]
fn the_step_down_lines_up_with_the_stage() {
    let (_, mic) = signals();
    let x = &mic[..B * 600];
    let ns = ApmSettings {
        noise_suppression: Some(NoiseSuppressionStrength::Moderate),
        ..settings(ProcessingConfig::off())
    };
    let mut apm = Apm::new(ns);
    let mut y = vec![0.0f32; x.len()];
    for (i, b) in x.as_chunks::<B>().0.iter().enumerate() {
        apm.capture(b, &mut y[i * B..(i + 1) * B]);
    }
    let from = 200 * B;
    let best = (0..2_000usize)
        .map(|lag| {
            let c: f64 = (from..x.len() - lag)
                .map(|n| x[n] as f64 * y[n + lag] as f64)
                .sum();
            (lag, c)
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("lags")
        .0;
    assert_eq!(
        best, STEP_DOWN_NS_DELAY,
        "the noise suppressor's delay is {best} samples"
    );
}

/// Step-down settings: echo cancellation, no noise suppression, the
/// fallback at Moderate.
fn step_down_settings() -> ApmSettings {
    ApmSettings {
        stage_fallback_ns: Some(NoiseSuppressionStrength::Moderate),
        ..settings(aec_only())
    }
}

/// What the step-down suppressor gives: Moderate noise suppression on the
/// echo canceller's output, delayed to the stage's 480 samples.
fn suppressed_reference(blocks: usize) -> Vec<f32> {
    let aec = plain(settings(aec_only()), blocks);
    let mut ns = Apm::new(ApmSettings {
        noise_suppression: Some(NoiseSuppressionStrength::Moderate),
        ..settings(ProcessingConfig::off())
    });
    let mut y = vec![0.0f32; aec.len()];
    for (i, b) in aec.as_chunks::<B>().0.iter().enumerate() {
        ns.capture(b, &mut y[i * B..(i + 1) * B]);
    }
    let mut out = vec![0.0f32; y.len()];
    let d = B - STEP_DOWN_NS_DELAY;
    out[d..].copy_from_slice(&y[..y.len() - d]);
    out
}

/// The largest sample-to-sample step in `x`.
fn max_step(x: &[f32]) -> f32 {
    x.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// The step-down floor: a bypass turns sonora's noise suppression on
/// (crossfaded over one block), a retry that brings the stage back turns it
/// off again (crossfaded), with the stage's timing throughout.
#[test]
fn a_bypass_steps_down_to_noise_suppression_and_a_retry_steps_back() {
    let (stage, script) = Scripted::new();
    let mut apm = staged(step_down_settings(), stage);
    let retry = RETRY_BLOCKS as usize;
    let blocks = 100 + retry + 200;
    let mut seen: Vec<(usize, CaptureStageStats)> = Vec::new();
    let out = run(&mut apm, blocks, |b, apm| {
        match b {
            100 => script.sleep_us.store(9_000, Ordering::Relaxed),
            101 => script.sleep_us.store(0, Ordering::Relaxed),
            _ => {}
        }
        if [100, 101, 100 + retry, 100 + retry + 1, 100 + retry + 2].contains(&b) {
            seen.push((b, stage_stats(apm)));
        }
    });
    let at = |b: usize| seen.iter().find(|(x, _)| *x == b).expect("reading").1;
    assert!(!at(100).fallback_noise_suppression, "{:?}", at(100));
    assert!(at(101).fallback_noise_suppression, "{:?}", at(101));
    // The retry's warm-up block is still the suppressor's.
    assert!(at(100 + retry + 1).active);
    assert!(!at(100 + retry + 2).fallback_noise_suppression);

    let delayed = {
        let p = plain(settings(aec_only()), blocks);
        let mut d = vec![0.0f32; p.len()];
        d[B..].copy_from_slice(&p[..p.len() - B]);
        d
    };
    let ns = suppressed_reference(blocks);
    let block = |x: &[f32], b: usize| x[b * B..(b + 1) * B].to_vec();
    let same = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits());
    // Before: the stage (a pure delay). After the crossfade block 100 and
    // until the stage is back: the suppressor. From block 100 + retry + 2:
    // the stage again.
    for b in 0..100 {
        assert!(same(&block(&out, b), &block(&delayed, b)), "block {b}");
    }
    for b in 101..=100 + retry {
        assert!(same(&block(&out, b), &block(&ns, b)), "block {b}");
    }
    for b in 100 + retry + 2..blocks {
        assert!(same(&block(&out, b), &block(&delayed, b)), "block {b}");
    }
    // The two crossfade blocks lie between their two sources, sample by
    // sample, and step no further than the sources do.
    for (b, from, to) in [(100, &delayed, &ns), (100 + retry + 1, &ns, &delayed)] {
        for i in b * B..(b + 1) * B {
            let (lo, hi) = (from[i].min(to[i]), from[i].max(to[i]));
            assert!(
                out[i] >= lo - 1e-6 && out[i] <= hi + 1e-6,
                "sample {i} outside its sources"
            );
        }
        let around = &out[(b - 1) * B..(b + 2) * B];
        let limit =
            max_step(&from[(b - 1) * B..(b + 2) * B]).max(max_step(&to[(b - 1) * B..(b + 2) * B]));
        assert!(
            max_step(around) <= limit * 1.1 + 1e-4,
            "block {b}: a step of {} where the sources step at most {limit}",
            max_step(around)
        );
    }
    // The suppressor suppresses: quieter than the unsuppressed output over
    // the bypass.
    let e = |x: &[f32]| x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
    assert!(
        e(&out[300 * B..1000 * B]) < e(&delayed[300 * B..1000 * B]),
        "no suppression during the bypass"
    );
}

/// A fault steps down for the rest of the stream; with noise suppression
/// already on (before the stage), there is nothing to step down to.
#[test]
fn a_fault_steps_down_for_good_and_suppression_already_on_is_left_alone() {
    let (stage, script) = Scripted::new();
    script.nan_on.store(50, Ordering::Relaxed);
    let mut apm = staged(step_down_settings(), stage);
    let out = run(&mut apm, 300, |_, _| {});
    let s = stage_stats(&mut apm);
    assert!(s.permanent && s.fallback_noise_suppression, "{s:?}");
    let ns = suppressed_reference(300);
    assert!(out[60 * B..]
        .iter()
        .zip(&ns[60 * B..])
        .all(|(a, b)| a.to_bits() == b.to_bits()));

    let (stage, script) = Scripted::new();
    script.nan_on.store(50, Ordering::Relaxed);
    let with_ns = ApmSettings {
        noise_suppression: Some(NoiseSuppressionStrength::Moderate),
        ..step_down_settings()
    };
    let mut apm = staged(with_ns, stage);
    let out = run(&mut apm, 300, |_, _| {});
    let s = stage_stats(&mut apm);
    assert!(s.permanent && !s.fallback_noise_suppression, "{s:?}");
    assert_delayed(
        &out,
        &plain(
            ApmSettings {
                noise_suppression: Some(NoiseSuppressionStrength::Moderate),
                ..settings(aec_only())
            },
            300,
        ),
        B,
        "suppression before the stage",
    );
}

#[test]
fn more_than_a_second_of_latency_is_refused() {
    struct Slow;
    impl CaptureStage for Slow {
        fn process(&mut self, _: &mut [f32; B]) {}
        fn latency_samples(&self) -> usize {
            48_001
        }
        fn reset(&mut self) {}
    }
    let e = match Core::with_stage(core_cfg(ProcessingConfig::off()), Some(Box::new(Slow))) {
        Ok(_) => panic!("a 48001-sample latency was accepted"),
        Err(e) => e,
    };
    assert_eq!(e.code, ErrorCode::InvalidConfig);
}

fn no_dtx(processing: ProcessingConfig) -> CoreConfig {
    CoreConfig {
        encoder: EncoderCfg {
            dtx: false,
            ..EncoderCfg::default()
        },
        ..core_cfg(processing)
    }
}

/// `ts48k` doesn't include the stage's latency: the same stamps with and
/// without a stage, and through a bypass.
#[test]
fn the_stage_does_not_move_ts48k() {
    let (_, mic) = signals();
    let stamps = |stage: Option<Box<dyn CaptureStage>>| -> Vec<u32> {
        let mut core = Core::with_stage(no_dtx(ProcessingConfig::off()), stage).expect("core");
        let mut ts = Vec::new();
        for b in mic.as_chunks::<B>().0.iter().take(400) {
            if let Some(f) = core.capture_block(b) {
                ts.push(f.ts48k);
            }
        }
        ts
    };
    let (slow, script) = Scripted::new();
    script.sleep_us.store(9_000, Ordering::Relaxed);
    let none = stamps(None);
    assert_eq!(none.len(), 200);
    assert_eq!(stamps(Some(Box::new(PassThrough::new()))), none);
    assert_eq!(stamps(Some(Box::new(slow))), none);
}

#[test]
fn stats_carry_the_stage() {
    let mut core = Core::with_stage(
        no_dtx(ProcessingConfig::off()),
        Some(Box::new(PassThrough::new())),
    )
    .expect("core");
    let (_, mic) = signals();
    for b in mic.as_chunks::<B>().0.iter().take(20) {
        let _ = core.capture_block(b);
    }
    let mut s = Stats::default();
    core.fill_stats(&mut s);
    let st = s.capture_stage.expect("stage stats");
    assert!(st.active && st.blocks_processed == 20 && st.latency_samples == 480);

    let mut core = Core::new(no_dtx(ProcessingConfig::off())).expect("core");
    let mut s = Stats::default();
    core.fill_stats(&mut s);
    assert_eq!(s.capture_stage, None);
}

/// Capture never stops for the stage: a frame every 20 ms through a slow
/// stage and a panicking one.
#[test]
fn capture_goes_on_through_a_bypass_and_a_fault() {
    let (_, mic) = signals();
    let setups: [fn(&super::stages::Script); 2] = [
        |s| s.sleep_us.store(9_000, Ordering::Relaxed),
        |s| s.panic_on.store(3, Ordering::Relaxed),
    ];
    for setup in setups {
        let (stage, script) = Scripted::new();
        setup(&script);
        let mut core = Core::with_stage(no_dtx(aec_only()), Some(Box::new(stage))).expect("core");
        let mut frames = 0;
        for b in mic.as_chunks::<B>().0.iter().take(400) {
            if core.capture_block(b).is_some() {
                frames += 1;
            }
        }
        assert_eq!(frames, 200);
    }
}
