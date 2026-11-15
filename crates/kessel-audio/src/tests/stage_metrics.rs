//! The capture stage's bars on real speech, each in three cases:
//!
//! - **none**: today's chain (AEC3 + NS + AGC2), no stage;
//! - **pass**: the same chain with a pass-through stage (a pure 480-sample
//!   delay), which measures the hook alone;
//! - **rnn**: the RNNoise-like test stage with sonora's NS off, as a caller
//!   with a denoising stage would run it (see `stages.rs`: RNNoise's shape
//!   and cost, untrained weights).
//!
//! Outputs are moved back by the stage's latency before any metric, so
//! every case is compared on the microphone's timeline.

use super::metrics::{far_case, mic_from, report, Case, ECHO_DB, FLOOR_DB, NEAR_DB, SYS_DELAY};
use super::stages::{PassThrough, RnnLike};
use super::*;
use crate::apm::Apm;
use crate::stage::CaptureStage;
use crate::testkit::*;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    None,
    Pass,
    Rnn,
    /// A pass-through stage the way a denoising caller configures it: NS
    /// off, the step-down suppressor (on by default) running beside it.
    /// For the CPU bar.
    #[cfg_attr(all(debug_assertions, not(feature = "cpu-bars")), allow(dead_code))]
    Floor,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::None => "none",
            Kind::Pass => "pass",
            Kind::Rnn => "rnn",
            Kind::Floor => "floor",
        }
    }

    fn processing(self) -> ProcessingConfig {
        match self {
            Kind::None | Kind::Pass => ProcessingConfig::default(),
            Kind::Rnn | Kind::Floor => ProcessingConfig::default().with_noise_suppression(None),
        }
    }

    fn stage(self) -> Option<Box<dyn CaptureStage>> {
        match self {
            Kind::None => None,
            Kind::Pass | Kind::Floor => Some(Box::new(PassThrough::new())),
            Kind::Rnn => Some(Box::new(RnnLike::new(77).0)),
        }
    }

    fn latency(self) -> usize {
        match self {
            Kind::None => 0,
            Kind::Pass | Kind::Rnn | Kind::Floor => BLOCK_SAMPLES,
        }
    }
}

/// One duplex run; the output aligned to the microphone. A stage that was
/// bypassed would measure the bypass, not the stage: that fails.
fn staged(kind: Kind, cfg: &CoreConfig, case: &Case, mic: &[f32]) -> (Vec<f32>, CallRun) {
    let run = run_core_staged(cfg, kind.stage(), &case.sched, mic.len(), Some(mic));
    assert_eq!(
        run.rendered, case.rendered,
        "playback differs between runs: the pipeline must be deterministic"
    );
    match (kind, run.stats.capture_stage) {
        (Kind::None, None) => {}
        (Kind::None, Some(s)) => panic!("a stage without asking: {s:?}"),
        (_, None) => panic!("{kind:?}: no stage statistics"),
        (_, Some(s)) => assert!(
            s.bypassed.is_none() && s.active,
            "{kind:?}: the stage was bypassed: {s:?}"
        ),
    }
    (align(&run.out, kind.latency()), run)
}

/// The processing alone (echo cancellation, the stage, AGC2; no codec) on a
/// fixed playback, block by block as the engine runs it (render first);
/// the output aligned to the microphone.
fn processed(kind: Kind, rendered: &[f32], mic: &[f32]) -> Vec<f32> {
    let settings = kind.processing().settings();
    let mut apm = match kind.stage() {
        None => Apm::new(settings),
        Some(s) => Apm::with_stage_clock(settings, s, super::stages::still_clock()).expect("stage"),
    };
    let mut out = vec![0.0f32; mic.len()];
    let blocks = rendered
        .as_chunks::<BLOCK_SAMPLES>()
        .0
        .iter()
        .zip(mic.as_chunks::<BLOCK_SAMPLES>().0);
    for (b, (r, m)) in blocks.enumerate() {
        apm.render(r);
        apm.capture(m, &mut out[b * BLOCK_SAMPLES..(b + 1) * BLOCK_SAMPLES]);
    }
    if let Some(s) = apm.stage_stats() {
        assert!(
            s.bypassed.is_none() && s.active,
            "{kind:?}: the stage was bypassed: {s:?}"
        );
    }
    align(&out, kind.latency())
}

#[derive(Clone, Copy)]
struct DoubleTalk {
    /// After the stage and AGC2 (the bar).
    near_gain_db: f64,
    gap_erle_db: f64,
    after_far_db: f64,
    /// The same ERLE after the Opus round trip (reported): DTX and the
    /// 20 ms framing, which a stage's 10 ms latency shifts, move it by about
    /// a dB either way.
    gap_erle_codec_db: f64,
}

/// The crate's S2 double-talk call (far 0-30 s, near 10-40 s) for one case.
fn double_talk(kind: Kind, ri: usize, seed: u64) -> DoubleTalk {
    let cfg = core_cfg(kind.processing());
    let room = &ROOMS[ri];
    let case = far_case(&cfg, seed, 40.0, (0.0, 30.0));
    let mut rng = Rng::new(seed + 7);
    let pose = laptop_pose(room, &mut rng);
    let path = echo_path(room, &pose, &mut rng);
    let echo = make_echo(&case.rendered, &path, SYS_DELAY, ECHO_DB);
    let n = echo.len();
    let mut near_dry = vec![0.0f32; n];
    let nt = speech_track(NEAR_POOL, &mut rng, n - secs(10.0));
    near_dry[secs(10.0)..secs(10.0) + nt.len()].copy_from_slice(&nt);
    let near = near_at_mic(&near_dry, room, &pose, NEAR_DB);
    let floor = floor_noise(&mut rng, n, FLOOR_DB);
    let mut mic = floor.clone();
    add_into(&mut mic, &echo);
    add_into(&mut mic, &near);
    let mut mic_ref = floor;
    add_into(&mut mic_ref, &near);

    let out = processed(kind, &case.rendered, &mic);
    let silence = vec![0.0f32; case.rendered.len()];
    let reference = processed(kind, &silence, &mic_ref);
    let (coded, _) = staged(kind, &cfg, &case, &mic);

    let near_m = active_frames(&near);
    let far_m = active_frames(&echo);
    let dt = mask_and(&near_m, &far_m);
    let gaps = mask_and_not(&far_m, &near_m);
    DoubleTalk {
        near_gain_db: level_on(&out, &dt, 1200, 3000) - level_on(&reference, &dt, 1200, 3000),
        gap_erle_db: erle(&mic, &out, &gaps, 1200, 3000).expect("far-only gaps"),
        after_far_db: level_on(&out, &near_m, 3100, 4000)
            - level_on(&reference, &near_m, 3100, 4000),
        gap_erle_codec_db: erle(&mic, &coded, &gaps, 1200, 3000).expect("far-only gaps"),
    }
}

/// Bar 1: ERLE in double talk: the far-only gaps of the double-talk call,
/// measured after AEC3, the stage and AGC2 (before the codec, as the bar
/// defines it), in all three rooms: no more than 1 dB below today's chain,
/// and at least 35 dB wherever today's chain reaches 35 dB. The living room
/// is the crate's full-chain reference (S2), where today's chain must reach
/// it too. In the office room today's chain itself measures about 31 dB
/// after the codec on this material, so 35 there would test sonora, not the
/// hook. The same ERLE after the Opus round trip is reported alongside.
#[test]
fn bar1_stage_keeps_erle_in_double_talk() {
    let _cpu = crate::tests::shared_cpu();
    for (ri, seed) in [(0usize, 2000u64), (1, 2200), (2, 2100)] {
        let today = double_talk(Kind::None, ri, seed);
        for kind in [Kind::None, Kind::Pass, Kind::Rnn] {
            let r = if kind == Kind::None {
                today
            } else {
                double_talk(kind, ri, seed)
            };
            report(
                &format!("bar1 {} {}", kind.name(), ROOMS[ri].name),
                &[
                    ("gap_erle_db".into(), r.gap_erle_db),
                    ("vs_today_db".into(), r.gap_erle_db - today.gap_erle_db),
                    ("near_gain_dt_db".into(), r.near_gain_db),
                    ("near_after_far_db".into(), r.after_far_db),
                    ("codec_gap_erle_db".into(), r.gap_erle_codec_db),
                    (
                        "codec_vs_today_db".into(),
                        r.gap_erle_codec_db - today.gap_erle_codec_db,
                    ),
                ],
            );
            if ri == 1 || today.gap_erle_db >= 35.0 {
                assert!(
                    r.gap_erle_db >= 35.0,
                    "{} {}: ERLE {:.1} dB < 35 (today's: {:.1})",
                    kind.name(),
                    ROOMS[ri].name,
                    r.gap_erle_db,
                    today.gap_erle_db
                );
            }
            assert!(
                r.gap_erle_db >= today.gap_erle_db - 1.0,
                "{} {}: ERLE {:.1} dB, more than 1 dB below today's {:.1}",
                kind.name(),
                ROOMS[ri].name,
                r.gap_erle_db,
                today.gap_erle_db
            );
            assert!(
                r.near_gain_db >= -7.0,
                "{} {}: near end ducked {:.1} dB",
                kind.name(),
                ROOMS[ri].name,
                r.near_gain_db
            );
        }
    }
}

/// Bar 2: AGC2's output level with a stage within 1.5 dB of today's, for a
/// normal and a soft (-12 dB) talker. Near-end speech only, no noise but
/// the -72 dBFS floor.
#[test]
fn bar2_stage_keeps_agc2_levels() {
    let _cpu = crate::tests::shared_cpu();
    let room = &ROOMS[0];
    let mut rng = Rng::new(6200);
    let pose = laptop_pose(room, &mut rng);
    let n = secs(30.0);
    let near_dry = speech_track(NEAR_POOL, &mut rng, n);
    let silent = Case {
        rendered: vec![0.0; n],
        sched: Vec::new(),
    };
    for (label, lvl) in [("normal", NEAR_DB), ("soft", NEAR_DB - 12.0)] {
        let near = near_at_mic(&near_dry, room, &pose, lvl);
        let mask = active_frames(&near);
        let mic = mic_from(&[near.as_slice()], &mut rng);
        let level = |kind: Kind| -> f64 {
            let cfg = core_cfg(kind.processing());
            let (out, _) = staged(kind, &cfg, &silent, &mic);
            level_on(&out, &mask, 1500, 3000)
        };
        let today = level(Kind::None);
        for kind in [Kind::Pass, Kind::Rnn] {
            let l = level(kind);
            let d = l - today;
            report(
                &format!("bar2 {label} {}", kind.name()),
                &[
                    ("input_dbfs".into(), lvl),
                    ("today_dbfs".into(), today),
                    ("stage_dbfs".into(), l),
                    ("delta_db".into(), d),
                ],
            );
            assert!(
                d.abs() <= 1.5,
                "{label} talker, {}: AGC2 output {l:.2} dBFS vs today's {today:.2} ({d:+.2} dB)",
                kind.name()
            );
        }
    }
}

/// Bar 3: the hook's own CPU: 60 s of duplex (far-end speech, its echo and
/// a near-end talker) with a pass-through stage against no stage, both on
/// today's chain, per 10 ms block (render + capture). Three alternating
/// rounds; the cost is the median of the rounds' differences of means. The
/// bar is 0.5% of a core: 50 µs per 10 ms block.
///
/// Also reported: the RNNoise-like stage's duplex cost and its own time per
/// block (not part of the bar: the hook's cost excludes the stage's work).
#[cfg(any(not(debug_assertions), feature = "cpu-bars"))]
#[test]
fn bar3_hook_cpu_cost() {
    let inputs = crate::tests::shared_cpu();
    let total = 60.0;
    let cfg = core_cfg(ProcessingConfig::default());
    let case = far_case(&cfg, 5100, total, (0.0, total));
    let mut rng = Rng::new(5101);
    let room = &ROOMS[1];
    let pose = laptop_pose(room, &mut rng);
    let path = echo_path(room, &pose, &mut rng);
    let echo = make_echo(&case.rendered, &path, SYS_DELAY, ECHO_DB);
    let n = echo.len();
    let mut near_dry = vec![0.0f32; n];
    let nt = speech_track(NEAR_POOL, &mut rng, n - secs(20.0));
    near_dry[secs(20.0)..secs(20.0) + nt.len()].copy_from_slice(&nt);
    let near = near_at_mic(&near_dry, room, &pose, NEAR_DB);
    let mic = mic_from(&[echo.as_slice(), near.as_slice()], &mut rng);
    drop(inputs);

    let _alone = crate::tests::exclusive_cpu();
    let mean_us = |v: &[u64]| v.iter().sum::<u64>() as f64 / v.len() as f64 / 1000.0;
    let pct = |v: &[u64], p: usize| -> f64 {
        let mut s = v.to_vec();
        s.sort_unstable();
        s[(s.len() * p / 100).min(s.len() - 1)] as f64 / 1000.0
    };
    // none, pass, floor; each round in a different order.
    let kinds = [Kind::None, Kind::Pass, Kind::Floor];
    let mut all: [Vec<u64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut diffs = Vec::new();
    let mut floor_diffs = Vec::new();
    for round in 0..3 {
        let mut means = [0.0f64; 3];
        for k in 0..3 {
            let i = (k + round) % 3;
            let kind = kinds[i];
            let cfg = core_cfg(kind.processing());
            let (_, run) = staged(kind, &cfg, &case, &mic);
            means[i] = mean_us(&run.block_ns);
            all[i].extend_from_slice(&run.block_ns);
        }
        diffs.push(means[1] - means[0]);
        floor_diffs.push(means[2] - means[0]);
        report(
            &format!("bar3 round {round}"),
            &[
                ("none_mean_us".into(), means[0]),
                ("pass_mean_us".into(), means[1]),
                ("floor_mean_us".into(), means[2]),
                ("hook_us".into(), means[1] - means[0]),
                ("floor_hook_us".into(), means[2] - means[0]),
            ],
        );
    }
    diffs.sort_by(|a, b| a.total_cmp(b));
    floor_diffs.sort_by(|a, b| a.total_cmp(b));
    let hook_us = diffs[1];
    let floor_us = floor_diffs[1];
    let [none_all, pass_all, floor_all] = all;

    let cfg = core_cfg(Kind::Rnn.processing());
    let (_, rnn_run) = staged(Kind::Rnn, &cfg, &case, &mic);
    let (mut alone, _) = RnnLike::new(5);
    alone.reset();
    let mut stage_ns = Vec::new();
    for b in mic.as_chunks::<BLOCK_SAMPLES>().0.iter() {
        let mut x = *b;
        let t = Instant::now();
        alone.process(&mut x);
        stage_ns.push(t.elapsed().as_nanos() as u64);
    }

    report(
        "bar3",
        &[
            ("none_p50_us".into(), pct(&none_all, 50)),
            ("none_p99_us".into(), pct(&none_all, 99)),
            ("none_mean_us".into(), mean_us(&none_all)),
            ("pass_p50_us".into(), pct(&pass_all, 50)),
            ("pass_p99_us".into(), pct(&pass_all, 99)),
            ("pass_mean_us".into(), mean_us(&pass_all)),
            ("hook_us_median".into(), hook_us),
            ("hook_pct_core".into(), hook_us / 100.0),
            ("floor_p50_us".into(), pct(&floor_all, 50)),
            ("floor_p99_us".into(), pct(&floor_all, 99)),
            ("floor_hook_us_median".into(), floor_us),
            ("floor_hook_pct_core".into(), floor_us / 100.0),
            ("rnn_duplex_p50_us".into(), pct(&rnn_run.block_ns, 50)),
            ("rnn_duplex_p99_us".into(), pct(&rnn_run.block_ns, 99)),
            ("rnn_duplex_mean_us".into(), mean_us(&rnn_run.block_ns)),
            ("rnn_stage_p50_us".into(), pct(&stage_ns, 50)),
            ("rnn_stage_p99_us".into(), pct(&stage_ns, 99)),
            ("debug_build".into(), cfg!(debug_assertions) as u8 as f64),
        ],
    );
    assert!(
        hook_us <= 50.0,
        "the hook costs {hook_us:.1} µs per 10 ms block (> 0.5% of a core)"
    );
    assert!(
        pct(&stage_ns, 99) < 6_000.0,
        "the RNNoise-like stage alone is over the 6 ms p99 budget"
    );
}
