//! Receive path under loss, reordering and jitter, with real Opus packets.

use super::*;
use crate::frame::dbov;
use crate::testkit::*;

fn far(seed: u64, n: usize) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    talker_track(&TALKERS[0], &mut rng, n)
}

fn active_level(x: &[f32], mask_src: &[f32]) -> f64 {
    let m = active_frames(mask_src);
    level_on(x, &m, 100, m.len())
}

fn participant_stats(run: &CallRun) -> crate::ParticipantStats {
    run.stats
        .participants
        .iter()
        .find(|(id, _)| *id == FAR_ID)
        .map(|(_, s)| s.clone())
        .expect("participant stats")
}

#[test]
fn clean_network_plays_everything() {
    let n = secs(10.0);
    let x = far(1, n);
    let sched = schedule(
        &encode_remote(&x, EncoderCfg::default()),
        Net {
            base_ms: 25.0,
            jitter_ms: 5.0,
            seed: 1,
            ..Net::default()
        },
    );
    let run = run_core(&core_cfg(ProcessingConfig::off()), &sched, n + secs(0.5), None);
    let st = participant_stats(&run);
    assert_eq!(st.loss_pct, 0.0, "{st:?}");
    assert_eq!(st.frames_concealed, 0, "{st:?}");
    assert_eq!(st.packets_late, 0, "{st:?}");
    // Played level matches the talker's.
    let lx = active_level(&x, &x);
    let ly = active_level(&run.rendered[..n], &x);
    // The playback is delayed by the network and jitter buffer; the
    // active-frame mask tolerates that over speech segments of seconds.
    assert!((lx - ly).abs() < 2.0, "talker {lx:.1} dBFS played {ly:.1} dBFS");
}

#[test]
fn loss_reordering_and_jitter_are_concealed() {
    let n = secs(20.0);
    let x = far(2, n);
    let packets = encode_remote(&x, EncoderCfg::default());
    let net = Net {
        base_ms: 30.0,
        jitter_ms: 30.0,
        loss: 0.10,
        reorder: 0.05,
        seed: 2,
    };
    let sched = schedule(&packets, net);
    let lossy = run_core(&core_cfg(ProcessingConfig::off()), &sched, n + secs(0.5), None);
    let clean = run_core(
        &core_cfg(ProcessingConfig::off()),
        &schedule(&packets, Net { base_ms: 30.0, ..Net::default() }),
        n + secs(0.5),
        None,
    );
    let st = participant_stats(&lossy);
    eprintln!("[receive] 10% loss, 5% reorder, 30 ms jitter: {st:?}");
    assert!((6.0..=14.0).contains(&st.loss_pct), "loss {:.1}%", st.loss_pct);
    assert!(st.frames_fec > 0, "FEC never used: {st:?}");
    assert!(st.concealed_pct < 15.0, "concealed {:.1}%", st.concealed_pct);
    assert!(lossy.rendered.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
    let l_lossy = active_level(&lossy.rendered[..n], &x);
    let l_clean = active_level(&clean.rendered[..n], &x);
    assert!(
        (l_lossy - l_clean).abs() < 2.5,
        "lossy playback {l_lossy:.1} dBFS vs clean {l_clean:.1} dBFS"
    );
}

#[test]
fn fec_off_conceals_every_loss() {
    let n = secs(10.0);
    let x = far(3, n);
    let cfg = EncoderCfg {
        fec: false,
        ..EncoderCfg::default()
    };
    let sched = schedule(
        &encode_remote(&x, cfg),
        Net {
            base_ms: 30.0,
            loss: 0.10,
            seed: 3,
            ..Net::default()
        },
    );
    let run = run_core(&core_cfg(ProcessingConfig::off()), &sched, n + secs(0.5), None);
    let st = participant_stats(&run);
    assert_eq!(st.frames_fec, 0, "{st:?}");
    assert!(st.frames_concealed > 0, "{st:?}");
}

#[test]
fn fec_beats_plc_on_lost_frames() {
    // The same loss pattern with and without FEC in the stream: the frames
    // that were lost come out closer to the clean decode with FEC.
    let n = secs(10.0);
    let x = far(4, n);
    let net = Net {
        base_ms: 30.0,
        loss: 0.08,
        seed: 4,
        ..Net::default()
    };
    let err_db = |fec: bool| -> f64 {
        let cfg = EncoderCfg {
            fec,
            ..EncoderCfg::default()
        };
        let packets = encode_remote(&x, cfg);
        let lossy = run_core(&core_cfg(ProcessingConfig::off()), &schedule(&packets, net), n, None);
        let clean = run_core(
            &core_cfg(ProcessingConfig::off()),
            &schedule(&packets, Net { base_ms: 30.0, ..Net::default() }),
            n,
            None,
        );
        let (mut s, mut e) = (0.0f64, 0.0f64);
        for (a, b) in clean.rendered.iter().zip(lossy.rendered.iter()) {
            s += (*a as f64).powi(2);
            e += (*a as f64 - *b as f64).powi(2);
        }
        10.0 * (s / e.max(1e-12)).log10()
    };
    let with_fec = err_db(true);
    let without = err_db(false);
    eprintln!("[receive] SNR vs clean decode at 8% loss: FEC {with_fec:.1} dB, PLC only {without:.1} dB");
    assert!(
        with_fec > without + 1.0,
        "FEC ({with_fec:.1} dB) is not better than PLC alone ({without:.1} dB)"
    );
}

#[test]
fn levels_follow_the_talker() {
    let cfg = core_cfg(ProcessingConfig::off());
    let n = secs(10.0);
    let mut x = far(5, n);
    // A long silence in the middle.
    x[secs(4.0)..secs(7.0)].fill(0.0);
    let sched = schedule(&encode_remote(&x, EncoderCfg::default()), Net { base_ms: 20.0, ..Net::default() });
    let mut core = Core::new(cfg.clone()).unwrap();
    core.add(Participant::new(FAR_ID, &cfg).unwrap()).unwrap_or_else(|_| panic!("add"));
    let t0 = Instant::now();
    let mut next = 0;
    let mut out = [0.0f32; BLOCK_SAMPLES];
    let mut levels = crate::events::Levels::default();
    let mut speaking_when_loud = (0usize, 0usize);
    let mut speaking_when_silent = (0usize, 0usize);
    for b in 0..n / BLOCK_SAMPLES {
        let now_us = b as u64 * 10_000;
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
        core.render_block(t0 + Duration::from_micros(now_us), &mut out);
        if core.take_levels_due() {
            core.fill_levels(&mut levels);
            let (_, l) = levels.participants[0];
            // Judge by what was played over the last 50 ms: the talker
            // about 70 ms earlier (20 ms network + the jitter buffer).
            let s = (b + 1) * BLOCK_SAMPLES;
            let lag = 3360;
            if s < 48_000 + lag + 2400 {
                continue;
            }
            let loud = dbov(&x[s - lag - 2400..s - lag]) > -35.0;
            let quiet_for_long = dbov(&x[s - lag - 48_000..s - lag]) < -100.0;
            if loud {
                speaking_when_loud.0 += l.speaking as usize;
                speaking_when_loud.1 += 1;
            }
            if quiet_for_long {
                speaking_when_silent.0 += l.speaking as usize;
                speaking_when_silent.1 += 1;
            }
            assert_eq!(levels.local.level_dbov, -127.0, "nothing captured yet");
        }
    }
    assert!(speaking_when_loud.1 > 20);
    assert!(speaking_when_silent.1 > 20, "the silence must be long enough to judge");
    let frac = speaking_when_loud.0 as f64 / speaking_when_loud.1 as f64;
    assert!(frac > 0.8, "speaking in only {:.0}% of loud windows", frac * 100.0);
    assert_eq!(speaking_when_silent.0, 0, "speaking during long silence");
}

#[test]
fn deafen_silences_playback_but_not_levels() {
    let cfg = core_cfg(ProcessingConfig::off());
    let n = secs(4.0);
    let x = far(6, n);
    let sched = schedule(&encode_remote(&x, EncoderCfg::default()), Net { base_ms: 20.0, ..Net::default() });
    let mut core = Core::new(cfg.clone()).unwrap();
    core.add(Participant::new(FAR_ID, &cfg).unwrap()).unwrap_or_else(|_| panic!("add"));
    core.set_deafened(true);
    let t0 = Instant::now();
    let mut next = 0;
    let mut out = [0.0f32; BLOCK_SAMPLES];
    let mut levels = crate::events::Levels::default();
    let mut max_level = -127.0f32;
    for b in 0..n / BLOCK_SAMPLES {
        let now_us = b as u64 * 10_000;
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
        core.render_block(t0 + Duration::from_micros(now_us), &mut out);
        assert!(out.iter().all(|&v| v == 0.0), "deafened output must be silent");
        if core.take_levels_due() {
            core.fill_levels(&mut levels);
            max_level = max_level.max(levels.participants[0].1.level_dbov);
        }
    }
    assert!(max_level > -40.0, "levels must stay live while deafened: {max_level}");
}

#[test]
fn unknown_participants_are_counted_not_played() {
    let cfg = core_cfg(ProcessingConfig::off());
    let mut core = Core::new(cfg).unwrap();
    core.insert(RecvFrame {
        participant: 99,
        payload: Bytes::from_static(&[0x08, 0x00]),
        seq: 1,
        ts48k: 0,
        arrival: Instant::now(),
    });
    let mut s = Stats::default();
    core.fill_stats(&mut s);
    assert_eq!(s.unknown_participant_frames, 1);
    assert!(s.participants.is_empty());
}
