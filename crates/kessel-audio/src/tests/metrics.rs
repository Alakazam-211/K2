//! Metric tests mirroring the baseline measurement (sonora vs the C++ WebRTC
//! library): far-end speech is encoded, received, decoded and played; the
//! playback goes through a synthetic laptop echo path to the microphone;
//! capture runs echo cancellation (and NS / AGC where stated), Opus
//! encode, and is decoded again by a remote decoder. Every bar compares
//! metrics (ERLE, levels, times, CPU), never samples.
//!
//! The baseline's sonora means are quoted next to each bar. The bars sit below them:
//! the material here is synthetic speech (no corpus download), and the
//! measured path includes the Opus codec.

use super::*;
use crate::apm::NoiseSuppressionStrength;
use crate::testkit::*;

const SYS_DELAY: usize = 60 * 48; // 60 ms
const ECHO_DB: f64 = -24.0;
const NEAR_DB: f64 = -26.0;
const FLOOR_DB: f64 = -72.0;

struct Case {
    rendered: Vec<f32>,
    sched: Vec<(u64, Packet)>,
}

/// Far-end talker over `far_range` seconds of a `total`-second call,
/// received with a fixed 30 ms network delay; returns what gets played.
fn far_case(cfg: &CoreConfig, seed: u64, total: f64, far_range: (f64, f64)) -> Case {
    let mut rng = Rng::new(seed);
    let n = secs(total);
    let mut far = vec![0.0f32; n];
    let t = speech_track(FAR_POOL, &mut rng, secs(far_range.1 - far_range.0));
    far[secs(far_range.0)..secs(far_range.0) + t.len()].copy_from_slice(&t);
    let packets = encode_remote(&far, EncoderCfg::default());
    let sched = schedule(
        &packets,
        Net {
            base_ms: 30.0,
            ..Net::default()
        },
    );
    let run = run_core(cfg, &sched, n, None);
    Case {
        rendered: run.rendered,
        sched,
    }
}

fn mic_from(parts: &[&[f32]], rng: &mut Rng) -> Vec<f32> {
    let n = parts[0].len();
    let mut m = floor_noise(rng, n, FLOOR_DB);
    for p in parts {
        add_into(&mut m, p);
    }
    m
}

/// Run the capture side against a fixed playback; checks the playback is
/// reproduced exactly (the pipeline is deterministic).
fn duplex(cfg: &CoreConfig, case: &Case, mic: &[f32]) -> CallRun {
    let run = run_core(cfg, &case.sched, mic.len(), Some(mic));
    assert_eq!(
        run.rendered, case.rendered,
        "playback differs between runs: the pipeline must be deterministic"
    );
    run
}

fn report(name: &str, values: &[(String, f64)]) {
    let line: Vec<String> = values.iter().map(|(k, v)| format!("{k}={v:.2}")).collect();
    eprintln!("[metrics] {name}: {}", line.join(" "));
}

/// S1 (baseline criterion 1): far-end speech only. Baseline, sonora (AEC3 + HPF):
/// steady ERLE 50.4 dB mean, 40.0 worst; first 2 s 33.5 dB.
#[test]
fn s1_far_end_only_erle() {
    let cfg = core_cfg(aec_only());
    let mut steady = Vec::new();
    for (ri, room) in ROOMS.iter().enumerate() {
        let seed = 1000 + ri as u64 * 100;
        let case = far_case(&cfg, seed, 30.0, (0.0, 30.0));
        let mut rng = Rng::new(seed + 1);
        let pose = laptop_pose(room, &mut rng);
        let path = echo_path(room, &pose, &mut rng);
        let echo = make_echo(&case.rendered, &path, SYS_DELAY, ECHO_DB);
        let mic = mic_from(&[echo.as_slice()], &mut rng);
        let run = duplex(&cfg, &case, &mic);
        let far = active_frames(&echo);
        let e_steady = erle(&mic, &run.out, &far, 500, 3000).expect("enough far-end frames");
        let e_first = erle(&mic, &run.out, &far, 0, 200).expect("enough far-end frames");
        let t20 = convergence_s(&mic, &run.out, &far, 0, 3000, 20.0);
        report(
            &format!("S1 {}", room.name),
            &[
                ("erle_5_30s_db".into(), e_steady),
                ("erle_first_2s_db".into(), e_first),
                ("t_to_20db_s".into(), t20.unwrap_or(f64::NAN)),
            ],
        );
        let aec_delay = run.stats.aec_delay_ms.expect("the echo canceller reports its delay");
        report(&format!("S1 {}", room.name), &[("aec_delay_ms".into(), aec_delay as f64)]);
        // The echo path's delay is 60 ms (the render reference is fed in the
        // same 10 ms block it is played).
        assert!((45..=75).contains(&aec_delay), "{}: AEC delay {aec_delay} ms", room.name);
        assert!(e_steady >= 30.0, "{}: steady ERLE {e_steady:.1} dB < 30", room.name);
        let t20 = t20.unwrap_or_else(|| panic!("{}: never reached 20 dB ERLE", room.name));
        assert!(t20 <= 1.0, "{}: {t20:.2} s to 20 dB ERLE", room.name);
        steady.push(e_steady);
    }
    let m = mean(&steady);
    report("S1 mean", &[("erle_5_30s_db".into(), m)]);
    assert!(m >= 35.0, "mean steady ERLE {m:.1} dB < 35");
}

struct DoubleTalk {
    near_gain_db: f64,
    gap_erle_db: f64,
    after_far_db: f64,
}

/// Far 0-30 s, near 10-40 s (baseline S2). Compares against the same near-end
/// alone through the same chain.
fn double_talk(processing: ProcessingConfig, ri: usize, seed: u64) -> DoubleTalk {
    let cfg = core_cfg(processing);
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

    let run = duplex(&cfg, &case, &mic);
    let silent = Case {
        rendered: vec![0.0; case.rendered.len()],
        sched: Vec::new(),
    };
    let reference = duplex(&cfg, &silent, &mic_ref);

    let near_m = active_frames(&near);
    let far_m = active_frames(&echo);
    let dt = mask_and(&near_m, &far_m);
    let near_gain_db = level_on(&run.out, &dt, 1200, 3000) - level_on(&reference.out, &dt, 1200, 3000);
    let gaps = mask_and_not(&far_m, &near_m);
    let gap_erle_db = erle(&mic, &run.out, &gaps, 1200, 3000).expect("far-only gaps");
    let after_far_db =
        level_on(&run.out, &near_m, 3100, 4000) - level_on(&reference.out, &near_m, 3100, 4000);
    DoubleTalk {
        near_gain_db,
        gap_erle_db,
        after_far_db,
    }
}

/// S2 (baseline criterion 2): double talk, AEC3 + HPF. Baseline, sonora: near-end gain
/// in double talk -4.0 dB (worst -5.8); ERLE in far-only gaps 42.1 dB
/// (worst 30.2).
#[test]
fn s2_double_talk_keeps_the_near_end() {
    for (ri, seed) in [(0usize, 2000u64), (2, 2100)] {
        let r = double_talk(aec_only(), ri, seed);
        report(
            &format!("S2 aec {}", ROOMS[ri].name),
            &[
                ("near_gain_dt_db".into(), r.near_gain_db),
                ("gap_erle_db".into(), r.gap_erle_db),
                ("near_after_far_db".into(), r.after_far_db),
            ],
        );
        assert!(r.near_gain_db >= -9.0, "near end ducked {:.1} dB in double talk", r.near_gain_db);
        assert!(r.gap_erle_db >= 25.0, "ERLE in far-only gaps {:.1} dB", r.gap_erle_db);
        assert!(r.after_far_db.abs() <= 2.0, "near end after far stops off by {:.1} dB", r.after_far_db);
    }
}

/// The default chain (AEC3 + NS + AGC2). Baseline, sonora: far-only ERLE 33.6 dB,
/// gaps 46.6 dB.
#[test]
fn s2_double_talk_full_chain() {
    let r = double_talk(ProcessingConfig::default(), 1, 2200);
    report(
        &format!("S2 full {}", ROOMS[1].name),
        &[
            ("near_gain_dt_db".into(), r.near_gain_db),
            ("gap_erle_db".into(), r.gap_erle_db),
            ("near_after_far_db".into(), r.after_far_db),
        ],
    );
    assert!(r.gap_erle_db >= 25.0, "ERLE in far-only gaps {:.1} dB", r.gap_erle_db);
    assert!(r.near_gain_db >= -10.0, "near end ducked {:.1} dB", r.near_gain_db);
    assert!(r.after_far_db.abs() <= 3.0, "near end after far stops off by {:.1} dB", r.after_far_db);
}

/// S3 (baseline criterion 3): the echo path changes at 20 s. (a) the laptop
/// moves and the speaker gets 3 dB louder; (b) the system delay jumps by
/// 40 ms. Baseline, sonora: time to 45 dB ERLE 0.01 s (a) and 0.32 s (b).
#[test]
fn s3_reconverges_after_an_echo_path_change() {
    let cfg = core_cfg(aec_only());
    let room = &ROOMS[1];
    let seed = 3000;
    let case = far_case(&cfg, seed, 40.0, (0.0, 40.0));
    let mut rng = Rng::new(seed + 1);
    let pose_a = laptop_pose(room, &mut rng);
    let path_a = echo_path(room, &pose_a, &mut rng);
    let pose_b = laptop_pose(room, &mut rng);
    let path_b = echo_path(room, &pose_b, &mut rng);
    let change = secs(20.0);
    let echo_a = make_echo(&case.rendered, &path_a, SYS_DELAY, ECHO_DB);
    for (label, echo_b) in [
        ("moved+3dB", make_echo(&case.rendered, &path_b, SYS_DELAY, ECHO_DB + 3.0)),
        ("delay+40ms", make_echo(&case.rendered, &path_a, SYS_DELAY + 40 * 48, ECHO_DB)),
    ] {
        let mut echo = echo_a.clone();
        echo[change..].copy_from_slice(&echo_b[change..]);
        let mic = mic_from(&[echo.as_slice()], &mut rng);
        let run = duplex(&cfg, &case, &mic);
        let far = active_frames(&echo);
        let before = erle(&mic, &run.out, &far, 1000, 2000).expect("frames");
        let after_2s = erle(&mic, &run.out, &far, 2000, 2200).expect("frames");
        let settled = erle(&mic, &run.out, &far, 3000, 4000).expect("frames");
        let t30 = convergence_s(&mic, &run.out, &far, 2000, 4000, 30.0);
        report(
            &format!("S3 {label}"),
            &[
                ("erle_before_db".into(), before),
                ("erle_first_2s_after_db".into(), after_2s),
                ("erle_settled_db".into(), settled),
                ("t_to_30db_s".into(), t30.unwrap_or(f64::NAN)),
            ],
        );
        let t30 = t30.unwrap_or_else(|| panic!("{label}: never got back to 30 dB ERLE"));
        assert!(t30 <= 2.0, "{label}: {t30:.2} s to get back to 30 dB ERLE");
        assert!(settled >= 30.0, "{label}: settled ERLE {settled:.1} dB");
        assert!(after_2s >= 15.0, "{label}: ERLE in the first 2 s after {after_2s:.1} dB");
    }
}

/// S4: noise suppression, pink noise at 5 dB SNR. Baseline, sonora
/// (moderate): 11.9 dB noise cut in pauses, measured against the raw input
/// (so it includes the high-pass filter's share).
#[test]
fn s4_noise_suppression_cuts_noise_in_pauses() {
    let processing = ProcessingConfig {
        echo_cancellation: false,
        noise_suppression: Some(NoiseSuppressionStrength::Moderate),
        gain_control: false,
        high_pass: true,
    };
    let cfg = core_cfg(processing);
    let mut rng = Rng::new(4000);
    let n = secs(20.0);
    let speech = speech_track(NEAR_POOL, &mut rng, n);
    let mut noise = pink(&mut rng, n);
    let sl = active_level_db(&speech);
    let nl = db(energy(&noise) / n as f64);
    scale(&mut noise, 10f64.powf((sl - 5.0 - nl) / 20.0));
    let mut mic = speech.clone();
    add_into(&mut mic, &noise);
    let silent = Case {
        rendered: vec![0.0; n],
        sched: Vec::new(),
    };
    let run = duplex(&cfg, &silent, &mic);
    // Reference: the same input with NS off, through the same codec.
    let plain = duplex(
        &core_cfg(ProcessingConfig {
            noise_suppression: None,
            ..cfg.processing.clone()
        }),
        &silent,
        &mic,
    );
    let speech_m = active_frames(&speech);
    let pauses = mask_not(&speech_m);
    let vs_input = level_on(&mic, &pauses, 300, 2000) - level_on(&run.out, &pauses, 300, 2000);
    let ns_alone = level_on(&plain.out, &pauses, 300, 2000) - level_on(&run.out, &pauses, 300, 2000);
    let speech_change =
        level_on(&run.out, &speech_m, 300, 2000) - level_on(&plain.out, &speech_m, 300, 2000);
    report(
        "S4 pink 5 dB",
        &[
            ("noise_cut_vs_input_db".into(), vs_input),
            ("noise_cut_ns_alone_db".into(), ns_alone),
            ("speech_change_db".into(), speech_change),
        ],
    );
    assert!(vs_input >= 9.0, "noise cut in pauses {vs_input:.1} dB < 9");
    assert!(ns_alone >= 6.0, "noise suppression alone cut {ns_alone:.1} dB < 6");
    assert!(speech_change >= -3.5, "speech level changed {speech_change:.1} dB");
}

/// S6: AGC2 levelling, near-end only at -50 to -20 dBFS, through the tuned
/// AGC2. Baseline (sonora untuned / C++): -27.0/-23.8/-24.0/-20.7 and
/// -27.3/-23.1/-23.1/-20.6 dBFS; spread 6.3 / 6.8 dB.
#[test]
fn s6_agc2_levels_speech() {
    let processing = ProcessingConfig {
        echo_cancellation: false,
        noise_suppression: None,
        gain_control: true,
        high_pass: true,
    };
    let cfg = core_cfg(processing);
    let room = &ROOMS[0];
    let mut rng = Rng::new(6000);
    let pose = laptop_pose(room, &mut rng);
    let n = secs(30.0);
    let near_dry = speech_track(NEAR_POOL, &mut rng, n);
    let silent = Case {
        rendered: vec![0.0; n],
        sched: Vec::new(),
    };
    let mut levels = Vec::new();
    for lvl in [-50.0f64, -40.0, -30.0, -20.0] {
        let near = near_at_mic(&near_dry, room, &pose, lvl);
        let mask = active_frames(&near);
        let mic = mic_from(&[near.as_slice()], &mut rng);
        let run = duplex(&cfg, &silent, &mic);
        let l = level_on(&run.out, &mask, 1500, 3000);
        report(&format!("S6 input {lvl}"), &[("output_dbfs".into(), l)]);
        levels.push(l);
    }
    let spread = max(&levels) - min(&levels);
    report("S6", &[("spread_db".into(), spread)]);
    for (i, l) in levels.iter().enumerate() {
        assert!((-31.0..=-17.0).contains(l), "input {} dBFS came out at {l:.1} dBFS", -50 + 10 * i as i32);
    }
    assert!(spread <= 8.0, "spread {spread:.1} dB over a 30 dB input range");
}

/// Baseline criterion 5: CPU per 10 ms block, here for the whole duplex path
/// (playout: jitter buffer, Opus decode, mix, AEC render analysis;
/// capture: AEC3 + NS + AGC2, Opus encode). The baseline measured the APM alone at
/// 1.55% of a core (p50 150.6 µs). Bar: median block under 5% of real
/// time (500 µs per 10 ms).
#[test]
fn cpu_full_duplex_under_five_percent_of_a_core() {
    let cfg = core_cfg(ProcessingConfig::default());
    let case = far_case(&cfg, 5000, 20.0, (0.0, 20.0));
    let mut rng = Rng::new(5001);
    let room = &ROOMS[1];
    let pose = laptop_pose(room, &mut rng);
    let path = echo_path(room, &pose, &mut rng);
    let echo = make_echo(&case.rendered, &path, SYS_DELAY, ECHO_DB);
    let mic = mic_from(&[echo.as_slice()], &mut rng);
    let run = duplex(&cfg, &case, &mic);
    let mut ns: Vec<u64> = run.block_ns.clone();
    ns.sort_unstable();
    let p50 = ns[ns.len() / 2] as f64 / 1000.0;
    let p99 = ns[ns.len() * 99 / 100] as f64 / 1000.0;
    let mean_us = ns.iter().sum::<u64>() as f64 / ns.len() as f64 / 1000.0;
    report(
        "CPU duplex",
        &[
            ("p50_us".into(), p50),
            ("p99_us".into(), p99),
            ("mean_us".into(), mean_us),
            ("pct_core_mean".into(), mean_us / 100.0),
            ("debug_build".into(), cfg!(debug_assertions) as u8 as f64),
        ],
    );
    assert!(p50 <= 500.0, "median block {p50:.0} µs (> 5% of a core)");
}

/// The AGC2 tuning holds: with the tuned headroom, speech at -40 and
/// -30 dBFS comes out louder than with WebRTC's default headroom by about
/// the 1 dB the C++ comparison asked for.
#[test]
fn s6_agc2_tuning_lifts_mid_levels_by_about_a_db() {
    use crate::apm::{Apm, AGC2_HEADROOM_DB};
    let processing = ProcessingConfig {
        echo_cancellation: false,
        noise_suppression: None,
        gain_control: true,
        high_pass: true,
    };
    let room = &ROOMS[0];
    let mut rng = Rng::new(6100);
    let pose = laptop_pose(room, &mut rng);
    let n = secs(30.0);
    let near_dry = speech_track(NEAR_POOL, &mut rng, n);
    for lvl in [-40.0f64, -30.0] {
        let near = near_at_mic(&near_dry, room, &pose, lvl);
        let mask = active_frames(&near);
        let mic = mic_from(&[near.as_slice()], &mut rng);
        let run = |headroom: f32| -> f64 {
            let mut apm = Apm::with_agc_headroom(&processing, headroom);
            let zeros = [0.0f32; BLOCK_SAMPLES];
            let mut out = vec![0.0f32; mic.len()];
            for (i, b) in mic.chunks_exact(BLOCK_SAMPLES).enumerate() {
                apm.render(&zeros);
                apm.capture(b, &mut out[i * BLOCK_SAMPLES..(i + 1) * BLOCK_SAMPLES]);
            }
            level_on(&out, &mask, 1500, 3000)
        };
        let default = run(5.0);
        let tuned = run(AGC2_HEADROOM_DB);
        report(
            &format!("S6 tuning input {lvl}"),
            &[("default_dbfs".into(), default), ("tuned_dbfs".into(), tuned)],
        );
        let lift = tuned - default;
        assert!((0.4..=1.6).contains(&lift), "input {lvl}: tuning lifted the level {lift:.2} dB");
    }
}
