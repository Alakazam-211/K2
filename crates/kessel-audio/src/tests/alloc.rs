//! Allocation checks. A counting allocator (this test binary only) counts
//! allocations made by the current thread while a check runs.
//!
//! Asserted: the device-thread callback bodies, the jitter buffer (in-order
//! packets), the level meters and the resamplers allocate nothing.
//! Reported: allocations per second of audio in the codec and echo
//! canceller crates, which run on the processing thread.

#![allow(unsafe_code)]

use super::*;
use crate::apm::Apm;
use crate::device::{pop_playback, push_capture, IoCounters};
use crate::jitter::{JitterBuffer, Next};
use crate::level::LevelMeter;
use crate::resample::{FromPipeline, ToPipeline};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

fn bump() {
    let _ = ON.try_with(|on| {
        if on.get() {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        }
    });
}

// SAFETY: forwards every call to the system allocator unchanged; only
// counts calls on threads that opted in.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Run `f` and return how many allocations this thread made in it.
fn allocations<R>(f: impl FnOnce() -> R) -> (R, u64) {
    COUNT.with(|c| c.set(0));
    ON.with(|on| on.set(true));
    let r = f();
    ON.with(|on| on.set(false));
    (r, COUNT.with(|c| c.get()))
}

#[test]
fn the_counter_counts() {
    let (_, n) = allocations(|| std::hint::black_box(vec![1u8; 100]));
    assert!(n >= 1);
}

#[test]
fn device_callbacks_do_not_allocate() {
    let (mut tx, mut rx) = rtrb::RingBuffer::<f32>::new(48_000);
    let (mut ptx, mut prx) = rtrb::RingBuffer::<f32>::new(48_000);
    let counters = IoCounters::default();
    let capture: Vec<i16> = (0..1024).map(|i| (i * 7) as i16).collect();
    let mut playback = vec![0.0f32; 1024];
    for _ in 0..10_000 {
        let _ = ptx.push(0.1);
    }
    let (_, n) = allocations(|| {
        for _ in 0..200 {
            push_capture(&mut tx, &capture, 2, |s| s as f32 / 32768.0, &counters);
            while rx.pop().is_ok() {}
            pop_playback(&mut prx, &mut playback, 2, |v| v, &counters);
        }
    });
    assert_eq!(n, 0, "device callbacks allocated {n} times");
}

#[test]
fn jitter_buffer_and_meters_do_not_allocate_in_steady_state() {
    let mut jb = JitterBuffer::new(crate::jitter::JitterConfig::default());
    let mut meter = LevelMeter::new(50, -45.0, 300);
    let payloads: Vec<Bytes> = (0..500u16)
        .map(|i| Bytes::from(i.to_be_bytes().to_vec()))
        .collect();
    let t0 = Instant::now();
    let pcm = [0.1f32; FRAME_SAMPLES];
    let ((), n) = allocations(|| {
        for (i, p) in payloads.into_iter().enumerate() {
            let at = t0 + Duration::from_millis(i as u64 * 20 + 5);
            jb.insert(i as u16, (i * FRAME_SAMPLES) as u32, p, at);
            let next = jb.next(t0 + Duration::from_millis(i as u64 * 20));
            assert!(!matches!(next, Next::Fec(_)));
            meter.feed(&pcm);
        }
    });
    assert_eq!(n, 0, "jitter buffer / meter allocated {n} times");
}

#[test]
fn resamplers_do_not_allocate_per_block() {
    let mut to = ToPipeline::new(44_100).unwrap();
    let mut from = FromPipeline::new(44_100).unwrap();
    let input = vec![0.1f32; to.input_max()];
    let mut block = [0.0f32; BLOCK_SAMPLES];
    let ((), n) = allocations(|| {
        for _ in 0..500 {
            let need = to.input_needed();
            to.process(&input[..need], &mut block).unwrap();
            let _ = from.process(&block).unwrap();
        }
    });
    assert_eq!(n, 0, "resamplers allocated {n} times");
}

#[test]
fn report_dependency_allocations_per_second() {
    let _cpu = crate::tests::shared_cpu();
    let mut rng = crate::testkit::Rng::new(77);
    let x = crate::testkit::talker_track(&crate::testkit::TALKERS[0], &mut rng, 48_000 * 5);
    let mut apm = Apm::new(ProcessingConfig::default());
    let mut enc = OpusEnc::new(EncoderCfg::default()).unwrap();
    let mut dec = OpusDec::new().unwrap();
    let mut out = [0.0f32; BLOCK_SAMPLES];
    let mut pcm = [0.0f32; FRAME_SAMPLES];
    let packets: Vec<Vec<u8>> = x
        .as_chunks::<FRAME_SAMPLES>()
        .0
        .iter()
        .map(|f| enc.encode(f).unwrap().packet.to_vec())
        .collect();
    // Warm-up done above for the encoder; measure 5 s of each.
    let ((), apm_n) = allocations(|| {
        for b in x.as_chunks::<BLOCK_SAMPLES>().0.iter() {
            apm.render(b);
            apm.capture(b, &mut out);
        }
    });
    let ((), enc_n) = allocations(|| {
        for f in x.as_chunks::<FRAME_SAMPLES>().0.iter() {
            let _ = enc.encode(f).unwrap();
        }
    });
    let ((), dec_n) = allocations(|| {
        for p in packets.iter() {
            dec.decode(p, &mut pcm);
        }
    });
    eprintln!(
        "[alloc] per second of audio: sonora render+capture {:.0}, opus encode {:.0}, opus decode {:.0}",
        apm_n as f64 / 5.0,
        enc_n as f64 / 5.0,
        dec_n as f64 / 5.0
    );
}

/// The capture stage's own path (the runner, its delay line and timing, the
/// stage) allocates nothing after construction, from the first block: 60 s
/// of the RNNoise-like stage (which plans its FFTs in its first `reset`,
/// done when the core is built) with resets every 5 s and mode flips every
/// 3 s, then a scripted stage through a budget bypass, a retry and a fault.
/// Counting starts right after construction. No sonora here, so any
/// allocation counted would be the hook's.
#[test]
fn the_capture_stage_path_does_not_allocate() {
    use crate::tests::stages::{RnnLike, Scripted};
    use std::sync::atomic::Ordering;
    let _cpu = crate::tests::shared_cpu();
    let mut rng = crate::testkit::Rng::new(78);
    let x = crate::testkit::talker_track(&crate::testkit::TALKERS[1], &mut rng, 48_000 * 61);
    let blocks = x.as_chunks::<BLOCK_SAMPLES>().0;
    let mut out = [0.0f32; BLOCK_SAMPLES];
    let bare = || ProcessingConfig::off().with_stage_fallback_noise_suppression(None);

    let (rnn, on) = RnnLike::new(3);
    let mut apm = Apm::with_stage_clock(bare(), Box::new(rnn), crate::tests::stages::still_clock())
        .expect("stage");
    let ((), n) = allocations(|| {
        for (i, b) in blocks[100..].iter().enumerate() {
            if i % 500 == 499 {
                apm.reset_stage();
            }
            if i % 300 == 299 {
                on.fetch_xor(true, Ordering::Relaxed);
            }
            apm.capture(b, &mut out);
            if i % 5 == 0 {
                std::hint::black_box(apm.stage_stats());
                std::hint::black_box(apm.take_stage_changed());
            }
        }
    });
    let s = apm.stage_stats().expect("stage stats");
    assert!(s.active && s.bypassed.is_none(), "{s:?}");
    assert_eq!(n, 0, "the RNNoise-like stage path allocated {n} times");

    // A budget strike, a retry, then a fault; with the step-down suppressor
    // (`floor`) and without.
    let cycle = |apm: &mut Apm, script: &crate::tests::stages::Script, out: &mut [f32]| {
        for (i, b) in blocks[..2_600].iter().enumerate() {
            match i {
                100 => script.cost_us.store(9_000, Ordering::Relaxed),
                102 => script.cost_us.store(0, Ordering::Relaxed),
                2_500 => {
                    let next = script.calls.load(Ordering::Relaxed) + 1;
                    script.nan_on.store(next, Ordering::Relaxed);
                }
                _ => {}
            }
            apm.capture(b, out);
            std::hint::black_box(apm.stage_stats());
        }
    };
    let (stage, script) = Scripted::new();
    let mut apm = Apm::with_stage_clock(bare(), Box::new(stage), script.clock()).expect("stage");
    let ((), n) = allocations(|| cycle(&mut apm, &script, &mut out));
    let s = apm.stage_stats().expect("stage stats");
    assert_eq!(
        s.strikes, 2,
        "a budget strike, a retry, then a fault: {s:?}"
    );
    assert!(s.permanent, "{s:?}");
    // Once before the first block, once for the retry.
    assert_eq!(script.resets.load(Ordering::Relaxed), 2);
    assert_eq!(
        n, 0,
        "bypass, retry and fault allocated {n} times on the audio path"
    );

    // The step-down floor: its suppressor is a sonora instance, which
    // allocates on its own as sonora's noise suppression always has. The
    // switching adds nothing: the count equals the same suppressor's alone
    // on the same blocks.
    let (stage, script) = Scripted::new();
    let mut apm = Apm::with_stage_clock(ProcessingConfig::off(), Box::new(stage), script.clock())
        .expect("stage");
    let ((), floor) = allocations(|| cycle(&mut apm, &script, &mut out));
    let s = apm.stage_stats().expect("stage stats");
    assert!(s.permanent && s.fallback_noise_suppression, "{s:?}");
    let mut ns = Apm::new(crate::apm::ApmSettings {
        noise_suppression: Some(crate::apm::NoiseSuppressionStrength::Moderate),
        ..bare().settings()
    });
    let ((), alone) = allocations(|| {
        for b in blocks[..2_600].iter() {
            ns.capture(b, &mut out);
        }
    });
    eprintln!(
        "[alloc] step-down suppressor, 26 s: with the stage and its switching {floor}, sonora's noise suppression alone {alone} ({:.0}/s)",
        alone as f64 / 26.0
    );
    assert_eq!(
        floor,
        alone,
        "the step-down switching allocated {} times",
        floor as i64 - alone as i64
    );
}

/// The whole processing thread's work (render, capture, encode, decode and
/// mix) for 60 s of duplex after a second of warm-up: a stage adds no
/// allocation. sonora and Opus allocate on their own (reported above), so
/// the bar is the count with a stage against the count without.
#[test]
fn a_capture_stage_adds_no_allocations_to_the_processing_thread() {
    use crate::tests::stages::{PassThrough, RnnLike};
    let _cpu = crate::tests::shared_cpu();
    let total = 48_000 * 61;
    let mut rng = crate::testkit::Rng::new(79);
    let far = crate::testkit::talker_track(&crate::testkit::TALKERS[0], &mut rng, total);
    let near = crate::testkit::talker_track(&crate::testkit::TALKERS[1], &mut rng, total);
    let mut mic = crate::testkit::delay(&far, 60 * 48);
    crate::testkit::scale(&mut mic, 0.25);
    crate::testkit::add_into(&mut mic, &near);
    let sched = schedule(
        &encode_remote(&far, EncoderCfg::default()),
        Net {
            base_ms: 30.0,
            ..Net::default()
        },
    );
    let count = |processing: ProcessingConfig, stage: Option<Box<dyn crate::CaptureStage>>| {
        let cfg = core_cfg(processing);
        let mut core =
            Core::with_stage_clock(cfg.clone(), stage, crate::tests::stages::still_clock())
                .expect("core");
        core.add(Participant::new(FAR_ID, &cfg).expect("participant"))
            .unwrap_or_else(|_| panic!("add participant"));
        let t0 = Instant::now();
        let mut next = 0usize;
        let mut out = [0.0f32; BLOCK_SAMPLES];
        let mut frames = 0u64;
        let mut step = |core: &mut Core, b: usize| {
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
            let r = b * BLOCK_SAMPLES..(b + 1) * BLOCK_SAMPLES;
            core.capture_block(&mic[r]).is_some()
        };
        for b in 0..100 {
            frames += step(&mut core, b) as u64;
        }
        let ((), n) = allocations(|| {
            for b in 100..total / BLOCK_SAMPLES {
                frames += step(&mut core, b) as u64;
            }
        });
        let mut stats = Stats::default();
        core.fill_stats(&mut stats);
        if let Some(s) = stats.capture_stage {
            assert!(s.active && s.bypassed.is_none(), "{s:?}");
        }
        assert!(frames > 2_000, "only {frames} frames");
        n
    };
    let none = count(ProcessingConfig::default(), None);
    let pass = count(
        ProcessingConfig::default(),
        Some(Box::new(PassThrough::new())),
    );
    let rnn = count(
        ProcessingConfig::default()
            .with_noise_suppression(None)
            .with_stage_fallback_noise_suppression(None),
        Some(Box::new(RnnLike::new(4).0)),
    );
    // With the step-down suppressor running beside the stage (reported: it
    // is sonora's noise suppression, moved out of line).
    let floor = count(
        ProcessingConfig::default().with_noise_suppression(None),
        Some(Box::new(RnnLike::new(4).0)),
    );
    eprintln!(
        "[alloc] processing thread, 60 s duplex: no stage {none}, pass-through stage {pass}, RNNoise-like stage (NS off) {rnn}, the same with the step-down suppressor {floor}"
    );
    assert!(
        pass <= none,
        "a pass-through stage added {} allocations",
        pass - none
    );
    assert!(
        rnn <= none,
        "the RNNoise-like stage added {} allocations",
        rnn - none
    );
}
