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
    let mut rng = crate::testkit::Rng::new(77);
    let x = crate::testkit::talker_track(&crate::testkit::TALKERS[0], &mut rng, 48_000 * 5);
    let mut apm = Apm::new(&ProcessingConfig::default());
    let mut enc = OpusEnc::new(EncoderCfg::default()).unwrap();
    let mut dec = OpusDec::new().unwrap();
    let mut out = [0.0f32; BLOCK_SAMPLES];
    let mut pcm = [0.0f32; FRAME_SAMPLES];
    let packets: Vec<Vec<u8>> = x
        .chunks_exact(FRAME_SAMPLES)
        .map(|f| enc.encode(f).unwrap().packet.to_vec())
        .collect();
    // Warm-up done above for the encoder; measure 5 s of each.
    let ((), apm_n) = allocations(|| {
        for b in x.chunks_exact(BLOCK_SAMPLES) {
            apm.render(b);
            apm.capture(b, &mut out);
        }
    });
    let ((), enc_n) = allocations(|| {
        for f in x.chunks_exact(FRAME_SAMPLES) {
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
