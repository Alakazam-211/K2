//! The public frame interface, end to end on the null device: real threads,
//! real queues, no audio hardware.

use kessel_audio::bytes::Bytes;
use kessel_audio::{
    AudioEngine, CaptureSource, DeviceChoice, DeviceSelection, EncodedFrame, EngineConfig,
    ErrorCode, Event, FrameRx, NullDevice, ProcessingConfig, RecvFrame, FRAME_SAMPLES,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A voiced buzz with a syllable envelope, `on` seconds of every `period`.
fn talker(rate: u32, on: f64, period: f64) -> impl FnMut(&mut [f32]) + Send {
    let mut n = 0u64;
    let mut phase = 0.0f64;
    move |buf: &mut [f32]| {
        for v in buf.iter_mut() {
            let t = n as f64 / rate as f64;
            n += 1;
            let f0 = 140.0 + 30.0 * (2.0 * std::f64::consts::PI * 0.7 * t).sin();
            phase = (phase + f0 / rate as f64).fract();
            let active = (t % period) < on;
            let env = if active {
                0.6 + 0.4 * (2.0 * std::f64::consts::PI * 4.0 * t).sin()
            } else {
                0.0
            };
            let mut s = 0.0;
            for h in 1..10 {
                s += (2.0 * std::f64::consts::PI * phase * h as f64).sin() / h as f64;
            }
            *v = (0.08 * env * s) as f32;
        }
    }
}

fn fast_null(input: Option<CaptureSource>) -> NullDevice {
    NullDevice {
        // 4x real time.
        block_period: Duration::from_micros(2_500),
        input,
        ..NullDevice::default()
    }
}

fn realtime_null(input: Option<CaptureSource>) -> NullDevice {
    NullDevice {
        input,
        ..NullDevice::default()
    }
}

fn config(dev: NullDevice) -> EngineConfig {
    EngineConfig {
        processing: ProcessingConfig::off(),
        processing_poll: Duration::from_micros(500),
        ..EngineConfig::null(dev)
    }
}

fn collect(rx: &FrameRx, n: usize, timeout: Duration) -> Vec<EncodedFrame> {
    let deadline = Instant::now() + timeout;
    let mut v = Vec::new();
    while v.len() < n {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(
            !left.is_zero(),
            "only {} of {n} frames before the timeout",
            v.len()
        );
        if let Some(f) = rx.recv_timeout(left) {
            v.push(f);
        }
    }
    v
}

#[test]
fn handles_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<AudioEngine>();
    check::<FrameRx>();
    check::<kessel_audio::EventRx>();
}

#[test]
fn capture_frames_step_by_960() {
    let engine = AudioEngine::start(config(fast_null(Some(Box::new(talker(
        48_000, 10.0, 10.0,
    ))))))
    .expect("start");
    let frames = collect(&engine.capture_frames(), 100, Duration::from_secs(10));
    for w in frames.windows(2) {
        assert_eq!(w[1].ts48k.wrapping_sub(w[0].ts48k), FRAME_SAMPLES as u32);
    }
    assert_eq!(frames[0].ts48k % FRAME_SAMPLES as u32, 0);
    assert!(frames.iter().all(|f| !f.payload.is_empty()));
    let voiced = frames.iter().filter(|f| f.vad).count();
    assert!(
        voiced > 80,
        "only {voiced} of 100 speech frames had VAD set"
    );
    assert!(frames[50].level_dbov > -40.0 && frames[50].level_dbov < 0.0);
}

#[test]
fn ts48k_wraps_around() {
    let start = u32::MAX - 5 * FRAME_SAMPLES as u32 + 1;
    let cfg = EngineConfig {
        initial_ts48k: start,
        ..config(fast_null(Some(Box::new(talker(48_000, 10.0, 10.0)))))
    };
    let engine = AudioEngine::start(cfg).expect("start");
    let frames = collect(&engine.capture_frames(), 20, Duration::from_secs(10));
    assert_eq!(frames[0].ts48k, start);
    for w in frames.windows(2) {
        assert_eq!(w[1].ts48k.wrapping_sub(w[0].ts48k), FRAME_SAMPLES as u32);
    }
    assert!(
        frames[19].ts48k < frames[0].ts48k,
        "the timestamp should have wrapped"
    );
}

#[test]
fn dtx_gaps_advance_ts48k() {
    // 1 s of speech, 2 s of silence, repeating.
    let engine = AudioEngine::start(config(fast_null(Some(Box::new(talker(48_000, 1.0, 3.0))))))
        .expect("start");
    let rx = engine.capture_frames();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut frames = Vec::new();
    // Two full cycles at 4x real time take 1.5 s.
    while Instant::now() < deadline && frames.len() < 400 {
        if let Some(f) = rx.recv_timeout(Duration::from_millis(50)) {
            frames.push(f);
        }
        if frames
            .last()
            .is_some_and(|f: &EncodedFrame| f.ts48k >= 48_000 * 7)
        {
            break;
        }
    }
    let biggest_gap = frames
        .windows(2)
        .map(|w| w[1].ts48k.wrapping_sub(w[0].ts48k))
        .max()
        .expect("frames");
    assert!(
        biggest_gap > 10 * FRAME_SAMPLES as u32,
        "no DTX gap: largest step {biggest_gap}"
    );
    assert!(frames
        .windows(2)
        .all(|w| w[1].ts48k.wrapping_sub(w[0].ts48k) % FRAME_SAMPLES as u32 == 0));
    // About 1 s of 3 is sent, plus comfort-noise updates.
    let sent_per_cycle = frames.iter().filter(|f| f.ts48k < 48_000 * 6).count() as f64 / 2.0;
    assert!(
        sent_per_cycle < 100.0,
        "{sent_per_cycle} frames per 3 s cycle: DTX isn't suppressing silence"
    );
    assert!(
        frames.iter().any(|f| f.dtx),
        "no comfort-noise (dtx) frames"
    );
}

#[test]
fn mute_stops_frames_and_time_keeps_running() {
    let engine = AudioEngine::start(config(fast_null(Some(Box::new(talker(
        48_000, 100.0, 100.0,
    ))))))
    .expect("start");
    let rx = engine.capture_frames();
    let before = collect(&rx, 10, Duration::from_secs(5));
    engine.set_capture(false);
    std::thread::sleep(Duration::from_millis(100));
    while rx.try_recv().is_some() {}
    std::thread::sleep(Duration::from_millis(300));
    assert!(rx.try_recv().is_none(), "frames while muted");
    let muted_levels = engine.levels();
    assert!(!muted_levels.local.speaking);
    engine.set_capture(true);
    let after = collect(&rx, 1, Duration::from_secs(5));
    let gap = after[0].ts48k.wrapping_sub(before[9].ts48k);
    // 400 ms of wall time at 4x is 1.6 s of audio.
    assert!(gap >= 48_000, "ts48k only advanced {gap} across the mute");
}

/// A device far faster than any processor keeps the processing thread behind
/// for good. A mute must still land: it can't wait for a backlog that never
/// clears.
#[test]
fn mute_lands_while_the_processing_thread_is_behind() {
    let device = NullDevice {
        block_period: Duration::from_micros(100),
        input: Some(Box::new(talker(48_000, 100.0, 100.0))),
        ..NullDevice::default()
    };
    let engine = AudioEngine::start(config(device)).expect("start");
    let rx = engine.capture_frames();
    collect(&rx, 10, Duration::from_secs(10));
    engine.set_capture(false);
    std::thread::sleep(Duration::from_millis(200));
    while rx.try_recv().is_some() {}
    std::thread::sleep(Duration::from_millis(300));
    assert!(rx.try_recv().is_none(), "frames while muted");
}

#[test]
fn a_call_between_two_engines() {
    // Engine A talks; its frames are delivered to engine B as participant 7.
    // Both run in real time, so arrival times match the timestamps.
    let a = AudioEngine::start(config(realtime_null(Some(Box::new(talker(
        48_000, 100.0, 100.0,
    ))))))
    .expect("start a");
    let played: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = played.clone();
    let b = AudioEngine::start(config(NullDevice {
        output: Some(Box::new(move |x: &[f32]| {
            sink.lock().unwrap().extend_from_slice(x)
        })),
        ..realtime_null(None)
    }))
    .expect("start b");
    b.add_participant(7);
    let events = b.events();
    let rx = a.capture_frames();
    let t0 = Instant::now();
    let mut seq = 0u16;
    while t0.elapsed() < Duration::from_millis(1500) {
        if let Some(f) = rx.recv_timeout(Duration::from_millis(20)) {
            b.push(RecvFrame {
                participant: 7,
                payload: f.payload,
                seq,
                ts48k: f.ts48k,
                arrival: Instant::now(),
            });
            seq = seq.wrapping_add(1);
        }
    }
    let stats = b.stats();
    let p = &stats
        .participants
        .iter()
        .find(|(id, _)| *id == 7)
        .expect("stats for 7")
        .1;
    assert!(p.packets_received > 50, "{p:?}");
    assert_eq!(p.loss_pct, 0.0, "{p:?}");
    assert!(p.frames_decoded > 40, "{p:?}");
    let levels = b.levels();
    let l = levels
        .participants
        .iter()
        .find(|(id, _)| *id == 7)
        .expect("level for 7")
        .1;
    assert!(l.speaking, "participant 7 should be speaking: {l:?}");
    let mut saw_levels = 0;
    while let Some(e) = events.try_recv() {
        if matches!(e, Event::Levels(_)) {
            saw_levels += 1;
        }
    }
    assert!(saw_levels >= 5, "only {saw_levels} level events");
    let energy: f64 = {
        let v = played.lock().unwrap();
        let tail = &v[v.len().saturating_sub(24_000)..];
        tail.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / tail.len().max(1) as f64
    };
    assert!(
        energy > 1e-5,
        "nothing audible was played (mean square {energy})"
    );

    // Deafen: playback goes silent.
    b.set_deafened(true);
    std::thread::sleep(Duration::from_millis(200));
    played.lock().unwrap().clear();
    let t1 = Instant::now();
    while t1.elapsed() < Duration::from_millis(300) {
        if let Some(f) = rx.recv_timeout(Duration::from_millis(20)) {
            b.push(RecvFrame {
                participant: 7,
                payload: f.payload,
                seq,
                ts48k: f.ts48k,
                arrival: Instant::now(),
            });
            seq = seq.wrapping_add(1);
        }
    }
    let v = played.lock().unwrap();
    assert!(!v.is_empty());
    assert!(v.iter().all(|&s| s == 0.0), "deafened output is not silent");
}

#[test]
fn push_never_blocks_and_counts_drops() {
    let cfg = EngineConfig {
        recv_queue_frames: 8,
        ..config(fast_null(None))
    };
    let engine = AudioEngine::start(cfg).expect("start");
    engine.add_participant(1);
    let t0 = Instant::now();
    for i in 0..20_000u32 {
        engine.push(RecvFrame {
            participant: 1,
            payload: Bytes::from_static(&[0xf8, 0xff, 0xfe]),
            seq: i as u16,
            ts48k: i * FRAME_SAMPLES as u32,
            arrival: Instant::now(),
        });
    }
    assert!(t0.elapsed() < Duration::from_secs(2), "push blocked");
    std::thread::sleep(Duration::from_millis(100));
    assert!(engine.stats().recv_queue_drops > 0);
}

#[test]
fn unknown_participants_are_counted() {
    let engine = AudioEngine::start(config(fast_null(None))).expect("start");
    engine.push(RecvFrame {
        participant: 42,
        payload: Bytes::from_static(&[0xf8, 0xff, 0xfe]),
        seq: 0,
        ts48k: 0,
        arrival: Instant::now(),
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while engine.stats().unknown_participant_frames == 0 {
        assert!(Instant::now() < deadline, "the frame was never counted");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_frame_queue_is_bounded() {
    let cfg = EngineConfig {
        capture_queue_frames: 5,
        ..config(fast_null(Some(Box::new(talker(48_000, 100.0, 100.0)))))
    };
    let engine = AudioEngine::start(cfg).expect("start");
    std::thread::sleep(Duration::from_millis(500));
    let rx = engine.capture_frames();
    assert!(rx.len() <= 5);
    let deadline = Instant::now() + Duration::from_secs(5);
    while engine.stats().capture.frames_dropped == 0 {
        assert!(
            Instant::now() < deadline,
            "no drops counted with a full queue"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_44k1_device_still_makes_50_frames_a_second() {
    let dev = NullDevice {
        sample_rate: 44_100,
        block_period: Duration::from_millis(10),
        input: Some(Box::new(talker(44_100, 100.0, 100.0))),
        output: None,
    };
    let engine = AudioEngine::start(config(dev)).expect("start");
    let rx = engine.capture_frames();
    let _ = collect(&rx, 5, Duration::from_secs(5));
    let t0 = Instant::now();
    let frames = collect(&rx, 50, Duration::from_secs(5));
    let secs = t0.elapsed().as_secs_f64();
    assert!(secs > 0.6 && secs < 1.6, "50 frames took {secs:.2} s");
    for w in frames.windows(2) {
        assert_eq!(w[1].ts48k.wrapping_sub(w[0].ts48k), FRAME_SAMPLES as u32);
    }
}

#[test]
fn null_devices_and_set_device() {
    let engine = AudioEngine::start(config(fast_null(None))).expect("start");
    let list = engine.devices();
    assert_eq!(list.inputs.len(), 1);
    assert_eq!(list.outputs.len(), 1);
    assert!(list.inputs[0].is_default);
    let in_use = engine.devices_in_use();
    assert_eq!(
        in_use.input.as_ref().map(|d| d.id.clone()),
        Some(list.inputs[0].id.clone())
    );
    engine
        .set_device(DeviceSelection {
            input: Some(DeviceChoice::Default),
            output: Some(DeviceChoice::Id(list.outputs[0].id.clone())),
        })
        .expect("switch to the same devices");
    let err = engine
        .set_device(DeviceSelection {
            input: Some(DeviceChoice::Id("no-such-device".into())),
            output: None,
        })
        .expect_err("unknown device");
    assert_eq!(err.code, ErrorCode::NoDevice);
    assert_eq!(err.code.as_str(), "no_device");
}

#[test]
fn invalid_config_is_refused() {
    let cfg = EngineConfig {
        playout_buffer_ms: 5,
        ..config(fast_null(None))
    };
    let err = AudioEngine::start(cfg).expect_err("5 ms playout buffer");
    assert_eq!(err.code, ErrorCode::InvalidConfig);
}

#[test]
fn error_codes_have_stable_names() {
    assert_eq!(ErrorCode::DeviceLost.as_str(), "device_lost");
    assert_eq!(ErrorCode::DeviceBusy.as_str(), "device_busy");
    assert_eq!(ErrorCode::NoPermission.as_str(), "no_permission");
}

#[test]
fn shutdown_is_prompt() {
    let engine = AudioEngine::start(config(fast_null(None))).expect("start");
    let t0 = Instant::now();
    drop(engine);
    assert!(t0.elapsed() < Duration::from_secs(1));
}
