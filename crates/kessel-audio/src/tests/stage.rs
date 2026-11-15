//! The capture stage on the processing core: where it sits, the bypass
//! (the echo canceller's output, delayed by the stage's latency, bit for
//! bit), the budget rules, faults, retries and resets.

use super::stages::{still_clock, PassThrough, Script, Scripted};
use super::{aec_only, core_cfg};
use crate::apm::{
    Apm, ApmSettings, NoiseSuppressionStrength, ProcessingConfig, STEP_DOWN_NS_DELAY,
};
use crate::error::ErrorCode;
use crate::frame::{EncoderCfg, BLOCK_SAMPLES};
use crate::pipeline::{Core, CoreConfig};
use crate::stage::{CaptureStage, CaptureStageStats, StageBypassReason, StageClock, RETRY_BLOCKS};
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

/// The stage on a clock that never moves: never over budget.
fn staged(settings: ApmSettings, stage: impl CaptureStage) -> Apm {
    Apm::with_stage_clock(settings, Box::new(stage), still_clock()).expect("stage accepted")
}

/// A scripted stage, timed by its script's clock.
fn staged_with(settings: ApmSettings, stage: Scripted, script: &Script) -> Apm {
    Apm::with_stage_clock(settings, Box::new(stage), script.clock()).expect("stage accepted")
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
fn a_9_ms_stage_is_bypassed_on_its_second_slow_call_retried_twice_then_bypassed_for_good() {
    let (stage, script) = Scripted::new();
    let mut apm = staged_with(settings(aec_only()), stage, &script);
    // Block 100 is the first slow call; the stage stays slow. Each strike
    // takes two slow calls in a row: block 101, then (after each 10 s
    // retry, whose warm-up call is the first slow one) blocks 1102, 2103.
    let (s1, s2, s3) = (101, 1_102, 2_103);
    let mut seen: Vec<CaptureStageStats> = Vec::new();
    let out = run(&mut apm, BLOCKS, |b, apm| {
        if b == 100 {
            script.cost_us.store(9_000, Ordering::Relaxed);
        }
        seen.push(stage_stats(apm));
    });
    assert_delayed(&out, &plain(settings(aec_only()), BLOCKS), B, "9 ms stage");

    // `seen[b]` is the reading before block b.
    let a = seen[s1];
    assert!(a.active && a.strikes == 0, "one slow call is not a strike: {a:?}");
    assert!(a.max_block_us >= 9_000, "{a:?}");
    // The flag is set by the second slow call itself.
    let a = seen[s1 + 1];
    assert!(
        !a.active && a.bypassed == Some(StageBypassReason::Budget) && a.strikes == 1,
        "{a:?}"
    );
    assert!(!a.permanent);
    let a = seen[s2 - 1];
    assert!(
        !a.active && a.strikes == 1,
        "retried, one slow call so far: {a:?}"
    );
    let a = seen[s2 + 1];
    assert!(!a.active && a.strikes == 2 && !a.permanent, "{a:?}");
    let a = seen[s3 + 1];
    assert!(
        !a.active && a.strikes == 3 && a.permanent,
        "three strikes within 60 s: {a:?}"
    );

    // 100 good calls, then two per strike; reset once before the first
    // call and before each retry.
    assert_eq!(script.calls.load(Ordering::Relaxed), 106);
    assert_eq!(script.resets.load(Ordering::Relaxed), 3);
    let s = stage_stats(&mut apm);
    assert_eq!(s.blocks_processed, 106);
    // Block 100's output was the stage's (it finished, if late).
    assert_eq!(s.blocks_bypassed, BLOCKS as u64 - 101);
}

/// Strikes more than 60 s apart don't add up: the count decays.
#[test]
fn strikes_more_than_60_s_apart_never_make_the_bypass_permanent() {
    let (stage, script) = Scripted::new();
    let mut apm = staged_with(settings(ProcessingConfig::off()), stage, &script);
    // A pair of slow calls every 35 s: any three strikes span 70 s.
    let mut strikes = 0;
    let _ = run(&mut apm, 12_000, |b, apm| {
        let slow = b % 3_500 == 100 || b % 3_500 == 101;
        script
            .cost_us
            .store(if slow { 9_000 } else { 0 }, Ordering::Relaxed);
        strikes = stage_stats(apm).strikes;
    });
    let s = stage_stats(&mut apm);
    assert_eq!(strikes, 4, "{s:?}");
    assert!(!s.permanent, "{s:?}");
    assert!(s.active, "back after each retry: {s:?}");

    // The same three strikes within 60 s: permanent.
    let (stage, script) = Scripted::new();
    let mut apm = staged_with(settings(ProcessingConfig::off()), stage, &script);
    let _ = run(&mut apm, 7_000, |b, _| {
        let slow = b % 1_500 == 100 || b % 1_500 == 101;
        script
            .cost_us
            .store(if slow { 9_000 } else { 0 }, Ordering::Relaxed);
    });
    let s = stage_stats(&mut apm);
    assert!(s.permanent && s.strikes == 3, "{s:?}");
}

/// One late call now and then, as a preempted thread gives, never strikes.
#[test]
fn single_late_calls_are_not_strikes() {
    let (stage, script) = Scripted::new();
    // One call in every 100 is 20 ms late.
    script.slow_every.store(100, Ordering::Relaxed);
    script.slow_us.store(20_000, Ordering::Relaxed);
    let mut apm = staged_with(settings(ProcessingConfig::off()), stage, &script);
    let _ = run(&mut apm, 2_000, |_, _| {});
    let s = stage_stats(&mut apm);
    assert!(s.active && s.strikes == 0, "{s:?}");
    assert!(s.max_block_us >= 20_000, "{s:?}");
}

#[test]
fn a_bypassed_stage_comes_back_after_10_s_without_a_seam() {
    let (stage, script) = Scripted::new();
    let mut apm = staged_with(settings(aec_only()), stage, &script);
    let retry = RETRY_BLOCKS as usize;
    let mut seen: Vec<(usize, CaptureStageStats)> = Vec::new();
    let blocks = 100 + retry + 300;
    let out = run(&mut apm, blocks, |b, apm| {
        // Two slow calls (blocks 100 and 101): a strike on block 101, and
        // the retry on block 101 + 10 s.
        match b {
            100 => script.cost_us.store(9_000, Ordering::Relaxed),
            102 => script.cost_us.store(0, Ordering::Relaxed),
            _ => {}
        }
        if b == 102 || b == 101 + retry || b == 102 + retry {
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
        !a.active && a.blocks_processed == 102,
        "not retried yet: {a:?}"
    );
    // The retry block resets the stage and runs it to fill its latency
    // (the delayed input goes on for that block); its output is used from
    // the next block.
    let (_, a) = seen[2];
    assert_eq!(a.blocks_processed, 103);
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
    script.slow_every.store(50, Ordering::Relaxed);
    script.slow_us.store(6_300, Ordering::Relaxed);
    let mut apm = staged_with(settings(ProcessingConfig::off()), stage, &script);
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
    script.slow_every.store(100, Ordering::Relaxed);
    script.slow_us.store(6_300, Ordering::Relaxed);
    let mut apm = staged_with(settings(ProcessingConfig::off()), stage, &script);
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
    let mut apm = staged_with(settings(aec_only()), stage, &script);
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
    let mut apm = staged_with(settings(aec_only()), stage, &script);
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
    let mut apm = staged_with(settings(aec_only()), stage, &script);
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
    let mut apm = staged_with(settings(ProcessingConfig::off()), stage, &script);
    let _ = run(&mut apm, 200, |b, apm| match b {
        50 => script.cost_us.store(9_000, Ordering::Relaxed),
        52 => script.cost_us.store(0, Ordering::Relaxed),
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
    let mut apm = staged_with(step_down_settings(), stage, &script);
    let retry = RETRY_BLOCKS as usize;
    // Slow calls on blocks 100 and 101: the strike (and the crossfade) is
    // block k = 101; the retry's warm-up block is k + retry.
    let k = 101;
    let blocks = k + retry + 200;
    let mut seen: Vec<(usize, CaptureStageStats)> = Vec::new();
    let out = run(&mut apm, blocks, |b, apm| {
        match b {
            100 => script.cost_us.store(9_000, Ordering::Relaxed),
            102 => script.cost_us.store(0, Ordering::Relaxed),
            _ => {}
        }
        if [k, k + 1, k + retry, k + retry + 1, k + retry + 2].contains(&b) {
            seen.push((b, stage_stats(apm)));
        }
    });
    let at = |b: usize| seen.iter().find(|(x, _)| *x == b).expect("reading").1;
    assert!(!at(k).fallback_noise_suppression, "{:?}", at(k));
    assert!(at(k + 1).fallback_noise_suppression, "{:?}", at(k + 1));
    // The retry's warm-up block is still the suppressor's.
    assert!(at(k + retry + 1).active);
    assert!(!at(k + retry + 2).fallback_noise_suppression);

    let delayed = {
        let p = plain(settings(aec_only()), blocks);
        let mut d = vec![0.0f32; p.len()];
        d[B..].copy_from_slice(&p[..p.len() - B]);
        d
    };
    let ns = suppressed_reference(blocks);
    let block = |x: &[f32], b: usize| x[b * B..(b + 1) * B].to_vec();
    let same = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits());
    // Before: the stage (a pure delay). After the crossfade block k and
    // until the stage is back: the suppressor. From block k + retry + 2:
    // the stage again.
    for b in 0..k {
        assert!(same(&block(&out, b), &block(&delayed, b)), "block {b}");
    }
    for b in k + 1..=k + retry {
        assert!(same(&block(&out, b), &block(&ns, b)), "block {b}");
    }
    for b in k + retry + 2..blocks {
        assert!(same(&block(&out, b), &block(&delayed, b)), "block {b}");
    }
    // The two crossfade blocks lie between their two sources, sample by
    // sample, and step no further than the sources do.
    for (b, from, to) in [(k, &delayed, &ns), (k + retry + 1, &ns, &delayed)] {
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
    let mut apm = staged_with(step_down_settings(), stage, &script);
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
    let mut apm = staged_with(with_ns, stage, &script);
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

/// The wall clock, end to end (the other budget tests use a manual clock).
/// Tolerant of a loaded machine: the stage sleeps 30 ms when armed, and a
/// sleep only ever runs long, so two armed calls are always a strike; before
/// arming, load may cause a strike of its own, so this only asserts what
/// arming must do.
#[test]
fn wall_clock_smoke_a_sleeping_stage_is_bypassed() {
    use std::sync::atomic::AtomicBool;
    struct Sleepy {
        armed: Arc<AtomicBool>,
        prev: [f32; B],
    }
    impl CaptureStage for Sleepy {
        fn process(&mut self, block: &mut [f32; B]) {
            if self.armed.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            std::mem::swap(block, &mut self.prev);
        }
        fn latency_samples(&self) -> usize {
            B
        }
        fn reset(&mut self) {
            self.prev.fill(0.0);
        }
    }
    let armed = Arc::new(AtomicBool::new(false));
    let mut apm = Apm::with_stage(
        settings(ProcessingConfig::off()),
        Box::new(Sleepy {
            armed: armed.clone(),
            prev: [0.0; B],
        }),
    )
    .expect("stage");
    let out = run(&mut apm, 60, |b, _| {
        if b == 50 {
            armed.store(true, Ordering::Relaxed);
        }
    });
    let s = stage_stats(&mut apm);
    assert!(
        !s.active && s.bypassed == Some(StageBypassReason::Budget) && s.strikes >= 1,
        "{s:?}"
    );
    // Whatever happened, the output is the input 480 samples late.
    let (_, mic) = signals();
    assert_delayed(&out, &mic[..60 * B], B, "wall clock");
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
    let stamps = |stage: Option<Box<dyn CaptureStage>>, clock: StageClock| -> Vec<u32> {
        let mut core = Core::with_stage_clock(no_dtx(ProcessingConfig::off()), stage, clock)
            .expect("core");
        let mut ts = Vec::new();
        for b in mic.as_chunks::<B>().0.iter().take(400) {
            if let Some(f) = core.capture_block(b) {
                ts.push(f.ts48k);
            }
        }
        ts
    };
    let (slow, script) = Scripted::new();
    script.cost_us.store(9_000, Ordering::Relaxed);
    let none = stamps(None, still_clock());
    assert_eq!(none.len(), 200);
    assert_eq!(
        stamps(Some(Box::new(PassThrough::new())), still_clock()),
        none
    );
    assert_eq!(stamps(Some(Box::new(slow)), script.clock()), none);
    assert_eq!(script.calls.load(Ordering::Relaxed), 2, "struck on the second");
}

#[test]
fn stats_carry_the_stage() {
    let mut core = Core::with_stage_clock(
        no_dtx(ProcessingConfig::off()),
        Some(Box::new(PassThrough::new())),
        still_clock(),
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
        |s| s.cost_us.store(9_000, Ordering::Relaxed),
        |s| s.panic_on.store(3, Ordering::Relaxed),
    ];
    for setup in setups {
        let (stage, script) = Scripted::new();
        setup(&script);
        let mut core =
            Core::with_stage_clock(no_dtx(aec_only()), Some(Box::new(stage)), script.clock())
                .expect("core");
        let mut frames = 0;
        for b in mic.as_chunks::<B>().0.iter().take(400) {
            if core.capture_block(b).is_some() {
                frames += 1;
            }
        }
        assert_eq!(frames, 200);
    }
}
