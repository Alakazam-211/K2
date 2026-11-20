//! The stage clocks where they differ: a call the OS preempts (wall time,
//! no CPU time) is slow on the wall clock and not on the thread's CPU time;
//! a call that works is slow on both; a starved call (wall time past the
//! cutoff) is slow on both, toward the 2-in-a-row rule only.
//!
//! First on a manual clock (exact, whatever the machine's load), then on the
//! OS's own clocks: real sleeps and real CPU work.

use super::stages::{Script, Scripted};
use crate::frame::BLOCK_SAMPLES;
use crate::stage::{
    CaptureStageStats, StageBypassReason, StageClock, StageRunner, StageTiming, TimeSource,
    DEFAULT_STAGE_STARVATION_CUTOFF,
};
use std::sync::atomic::Ordering;
use std::time::Duration;

const B: usize = BLOCK_SAMPLES;
const BOTH: [StageClock; 2] = [StageClock::ThreadCpu, StageClock::Wall];

fn timing(clock: StageClock, starvation_cutoff: Duration) -> StageTiming {
    StageTiming {
        grace: Duration::ZERO,
        clock,
        starvation_cutoff,
    }
}

/// A scripted stage on its script's clock.
fn scripted(clock: StageClock, cutoff: Duration) -> (StageRunner, std::sync::Arc<Script>) {
    let (stage, script) = Scripted::new();
    let r = StageRunner::with_source(Box::new(stage), script.clock(), timing(clock, cutoff))
        .expect("stage accepted");
    (r, script)
}

/// Run `blocks` blocks; `before(b)` runs before block `b`. Returns the
/// stats after each block.
fn run(
    r: &mut StageRunner,
    blocks: usize,
    mut before: impl FnMut(usize),
) -> Vec<CaptureStageStats> {
    let mut block = [0.0f32; B];
    (0..blocks)
        .map(|b| {
            before(b);
            r.process(&mut block);
            r.stats()
        })
        .collect()
}

/// The number of blocks run when the first strike landed, if any.
fn struck_after(seen: &[CaptureStageStats]) -> Option<usize> {
    seen.iter().position(|s| s.strikes > 0).map(|i| i + 1)
}

// ---- On a manual clock ----

#[test]
fn a_preempted_stage_strikes_on_the_wall_clock_and_not_on_thread_cpu() {
    // 0.5 ms of work, 12 ms preempted: 12.5 ms on the wall, under the
    // 20 ms starvation cutoff.
    for clock in BOTH {
        let (mut r, script) = scripted(clock, DEFAULT_STAGE_STARVATION_CUTOFF);
        script.cost_us.store(500, Ordering::Relaxed);
        script.preempt_us.store(12_000, Ordering::Relaxed);
        let seen = run(&mut r, 200, |_| {});
        let s = *seen.last().expect("ran");
        assert_eq!(s.clock, clock, "{s:?}");
        assert_eq!(s.max_wall_us, 12_500, "{clock:?}: {s:?}");
        match clock {
            StageClock::ThreadCpu => {
                assert_eq!(struck_after(&seen), None, "{s:?}");
                assert!(s.active && s.blocks_processed == 200, "{s:?}");
                assert_eq!(s.max_block_us, 500, "{s:?}");
                assert_eq!(s.p99_block_us, 500, "{s:?}");
                // Nothing hidden: every preempted call is counted.
                assert_eq!(s.preempted_over_budget, 200, "{s:?}");
                assert_eq!(s.starved, 0, "{s:?}");
            }
            StageClock::Wall => {
                assert_eq!(struck_after(&seen), Some(2), "{s:?}");
                assert_eq!(seen[1].bypassed, Some(StageBypassReason::Budget));
                assert_eq!(s.max_block_us, 12_500, "{s:?}");
                assert_eq!((s.preempted_over_budget, s.starved), (0, 0), "{s:?}");
            }
        }
    }
}

#[test]
fn a_stage_that_works_strikes_on_both_clocks() {
    for clock in BOTH {
        // 9 ms of work each call: the hard rule, on the second call.
        let (mut r, script) = scripted(clock, DEFAULT_STAGE_STARVATION_CUTOFF);
        script.cost_us.store(9_000, Ordering::Relaxed);
        let seen = run(&mut r, 10, |_| {});
        assert_eq!(struck_after(&seen), Some(2), "{clock:?}: {:?}", seen[1]);
        assert_eq!(seen[1].bypassed, Some(StageBypassReason::Budget));
        assert_eq!(seen[1].preempted_over_budget, 0, "{:?}", seen[1]);
        assert_eq!(seen[1].max_block_us, 9_000, "{:?}", seen[1]);

        // 7 ms of work on 2 calls in 100: the percentile rule.
        let (mut r, script) = scripted(clock, DEFAULT_STAGE_STARVATION_CUTOFF);
        let seen = run(&mut r, 100, |b| {
            let us = if b == 30 || b == 60 { 7_000 } else { 100 };
            script.cost_us.store(us, Ordering::Relaxed);
        });
        assert_eq!(struck_after(&seen), Some(100), "{clock:?}: {:?}", seen[99]);
    }
}

#[test]
fn a_starved_call_counts_on_the_thread_cpu_clock() {
    // 0.1 ms of work, 25 ms preempted: past the 20 ms cutoff.
    let (mut r, script) = scripted(StageClock::ThreadCpu, DEFAULT_STAGE_STARVATION_CUTOFF);
    script.cost_us.store(100, Ordering::Relaxed);
    script.preempt_us.store(25_000, Ordering::Relaxed);
    let seen = run(&mut r, 10, |_| {});
    assert_eq!(struck_after(&seen), Some(2), "{:?}", seen[1]);
    let s = seen[1];
    assert_eq!(s.bypassed, Some(StageBypassReason::Budget), "{s:?}");
    assert_eq!(s.starved, 2, "{s:?}");
    assert_eq!(s.preempted_over_budget, 2, "{s:?}");
    assert_eq!((s.max_block_us, s.max_wall_us), (100, 25_100), "{s:?}");

    // The cutoff is the caller's: raised past the stall, no strike...
    let (mut r, script) = scripted(StageClock::ThreadCpu, Duration::from_millis(30));
    script.cost_us.store(100, Ordering::Relaxed);
    script.preempt_us.store(25_000, Ordering::Relaxed);
    let seen = run(&mut r, 200, |_| {});
    assert_eq!(struck_after(&seen), None, "{:?}", seen[199]);
    assert_eq!(seen[199].starved, 0);
    // ...lowered under it, a strike: 12 ms on the wall against 10 ms.
    let (mut r, script) = scripted(StageClock::ThreadCpu, Duration::from_millis(10));
    script.cost_us.store(100, Ordering::Relaxed);
    script.preempt_us.store(11_900, Ordering::Relaxed);
    let seen = run(&mut r, 10, |_| {});
    assert_eq!(struck_after(&seen), Some(2), "{:?}", seen[1]);
    // At the cutoff counts: exactly 10 ms.
    let (mut r, script) = scripted(StageClock::ThreadCpu, Duration::from_millis(10));
    script.preempt_us.store(10_000, Ordering::Relaxed);
    let seen = run(&mut r, 10, |_| {});
    assert_eq!(struck_after(&seen), Some(2), "{:?}", seen[1]);
}

#[test]
fn single_starved_calls_are_no_strike_and_skip_the_percentile() {
    // A 25 ms stall every 10th call: 20 starved calls in 200, never two in a
    // row. On the thread's CPU time no strike (starvation counts toward the
    // 2-in-a-row rule only); on the wall clock the percentile strikes.
    for clock in BOTH {
        let (mut r, script) = scripted(clock, DEFAULT_STAGE_STARVATION_CUTOFF);
        script.cost_us.store(100, Ordering::Relaxed);
        let seen = run(&mut r, 200, |b| {
            let stall = if b % 10 == 5 { 25_000 } else { 0 };
            script.preempt_us.store(stall, Ordering::Relaxed);
        });
        match clock {
            StageClock::ThreadCpu => {
                let s = seen[199];
                assert_eq!(struck_after(&seen), None, "{s:?}");
                assert_eq!((s.starved, s.preempted_over_budget), (20, 20), "{s:?}");
            }
            StageClock::Wall => {
                assert_eq!(struck_after(&seen), Some(100), "{:?}", seen[99]);
            }
        }
    }
}

#[test]
fn the_grace_window_excuses_starved_calls_and_still_counts_them() {
    let (stage, script) = Scripted::new();
    let mut r = StageRunner::with_source(
        Box::new(stage),
        script.clock(),
        StageTiming {
            grace: Duration::from_millis(500),
            ..timing(StageClock::ThreadCpu, DEFAULT_STAGE_STARVATION_CUTOFF)
        },
    )
    .expect("stage accepted");
    script.preempt_us.store(25_000, Ordering::Relaxed);
    // 50 blocks in the window: excused, counted.
    let seen = run(&mut r, 52, |_| {});
    let s = seen[49];
    assert!(s.grace && s.active && s.strikes == 0, "{s:?}");
    assert_eq!((s.starved, s.grace_over_budget), (50, 50), "{s:?}");
    // After it, the 2-in-a-row rule as usual.
    assert_eq!(struck_after(&seen), Some(52), "{:?}", seen[51]);
}

#[test]
fn defaults_and_stats_say_which_clock() {
    assert_eq!(DEFAULT_STAGE_STARVATION_CUTOFF, Duration::from_millis(20));
    let p = crate::ProcessingConfig::default();
    assert_eq!(p.stage_clock, StageClock::default());
    assert_eq!(p.stage_starvation_cutoff, DEFAULT_STAGE_STARVATION_CUTOFF);
    let p = crate::ProcessingConfig::off()
        .with_stage_clock(StageClock::Wall)
        .with_stage_starvation_cutoff(Duration::from_millis(40));
    assert_eq!(p.stage_clock, StageClock::Wall);
    assert_eq!(p.stage_starvation_cutoff, Duration::from_millis(40));
    assert!(StageClock::Wall.is_supported());
    for clock in BOTH {
        let (mut r, _) = scripted(clock, DEFAULT_STAGE_STARVATION_CUTOFF);
        assert_eq!(r.stats().clock, clock, "a manual clock has both");
    }
}

#[test]
fn a_starvation_cutoff_under_the_hard_budget_is_refused() {
    use crate::{AudioEngine, EngineConfig, ErrorCode, NullDevice};
    let cfg = |cutoff: Duration| {
        let mut c = EngineConfig::null(NullDevice::default());
        c.processing.stage_starvation_cutoff = cutoff;
        c
    };
    match AudioEngine::start(cfg(Duration::from_micros(8_999))) {
        Ok(_) => panic!("a starvation cutoff under 9 ms was accepted"),
        Err(e) => assert_eq!(e.code, ErrorCode::InvalidConfig, "{e:?}"),
    }
    cfg(Duration::from_millis(9))
        .validate()
        .expect("9 ms is accepted");
    cfg(Duration::MAX)
        .validate()
        .expect("no cutoff is accepted");
}

// ---- On the OS's clocks ----

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
))]
mod os {
    use super::*;
    use crate::stage::CaptureStage;
    use std::time::Instant;

    /// How far one thread's CPU time may read ahead of the wall clock over
    /// the same interval: the two come from different clock sources, and the
    /// stats round each to whole microseconds.
    const SLACK: Duration = Duration::from_micros(200);

    /// What [`Real`] does in each call.
    #[derive(Clone, Copy, Debug)]
    enum Act {
        /// Sleep: wall time, next to no CPU time.
        Sleep(Duration),
        /// Spin until the thread has used this much CPU time.
        Burn(Duration),
    }

    /// A stage that sleeps or works for real.
    struct Real(Act);

    impl CaptureStage for Real {
        fn process(&mut self, block: &mut [f32; B]) {
            match self.0 {
                Act::Sleep(d) => std::thread::sleep(d),
                Act::Burn(d) => burn(d),
            }
            block[0] = std::hint::black_box(block[0]);
        }
        fn latency_samples(&self) -> usize {
            0
        }
        fn reset(&mut self) {}
    }

    /// Spin until the thread's CPU time has advanced by `d`. Panics if that
    /// takes more than 10 s of wall time (a CPU clock that doesn't move).
    fn burn(d: Duration) {
        let cpu = || crate::stage::thread_cpu::now().expect("thread CPU time");
        let (c0, w0) = (cpu(), Instant::now());
        let mut x = 1u64;
        while cpu().saturating_sub(c0) < d {
            for _ in 0..1_000 {
                x = std::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
            }
            assert!(
                w0.elapsed() < Duration::from_secs(10),
                "the thread CPU clock didn't reach {d:?} in 10 s"
            );
        }
    }

    fn real(act: Act, clock: StageClock, cutoff: Duration) -> StageRunner {
        StageRunner::with_source(Box::new(Real(act)), TimeSource::Os, timing(clock, cutoff))
            .expect("stage accepted")
    }

    #[test]
    fn thread_cpu_is_the_default_here() {
        assert!(StageClock::ThreadCpu.is_supported());
        assert_eq!(StageClock::default(), StageClock::ThreadCpu);
        let mut r = real(
            Act::Sleep(Duration::ZERO),
            StageClock::ThreadCpu,
            DEFAULT_STAGE_STARVATION_CUTOFF,
        );
        assert_eq!(r.stats().clock, StageClock::ThreadCpu);
    }

    #[test]
    fn the_thread_cpu_clock_counts_work_and_not_sleep() {
        let cpu = || crate::stage::thread_cpu::now().expect("thread CPU time");
        let c0 = cpu();
        std::thread::sleep(Duration::from_millis(50));
        let slept = cpu().saturating_sub(c0);
        assert!(
            slept < Duration::from_millis(10),
            "50 ms asleep used {slept:?} of CPU time"
        );
        // The wall interval holds the CPU one: wall read first, then last.
        let w0 = Instant::now();
        let c0 = cpu();
        burn(Duration::from_millis(30));
        let worked = cpu().saturating_sub(c0);
        let wall = w0.elapsed();
        assert!(worked >= Duration::from_millis(30), "{worked:?}");
        // One thread's CPU time can't run ahead of the wall clock (another
        // thread's, or the process's, could: the other tests run alongside),
        // give or take the two clocks' own sources (SLACK).
        assert!(worked <= wall + SLACK, "{worked:?} of CPU time in {wall:?}");
    }

    #[test]
    fn a_sleeping_stage_strikes_on_the_wall_clock_and_not_on_thread_cpu() {
        // 12 ms asleep, over the 9 ms budget. No starvation cutoff on the
        // thread's CPU time, so a loaded machine's long sleeps can't strike
        // there: only the CPU time is on trial.
        let act = Act::Sleep(Duration::from_millis(12));
        let mut r = real(act, StageClock::ThreadCpu, Duration::MAX);
        let seen = run(&mut r, 10, |_| {});
        let s = seen[9];
        assert_eq!(struck_after(&seen), None, "{s:?}");
        assert_eq!(s.clock, StageClock::ThreadCpu);
        assert!(s.max_block_us < 6_000, "{s:?}");
        assert!(s.max_wall_us >= 12_000, "{s:?}");
        assert_eq!(s.preempted_over_budget, 10, "{s:?}");

        // A sleep only runs long: two in a row always strike on the wall.
        let mut r = real(act, StageClock::Wall, DEFAULT_STAGE_STARVATION_CUTOFF);
        let seen = run(&mut r, 10, |_| {});
        assert_eq!(struck_after(&seen), Some(2), "{:?}", seen[1]);
        assert!(seen[1].max_block_us >= 12_000, "{:?}", seen[1]);
    }

    #[test]
    fn a_stage_that_burns_cpu_strikes_on_both_clocks() {
        for clock in BOTH {
            let mut r = real(
                Act::Burn(Duration::from_millis(10)),
                clock,
                DEFAULT_STAGE_STARVATION_CUTOFF,
            );
            let seen = run(&mut r, 10, |_| {});
            let s = seen[1];
            assert_eq!(struck_after(&seen), Some(2), "{clock:?}: {s:?}");
            assert_eq!(s.bypassed, Some(StageBypassReason::Budget), "{s:?}");
            assert!(s.max_block_us >= 10_000, "{clock:?}: {s:?}");
            let slack = SLACK.as_micros() as u32;
            assert!(s.max_wall_us + slack >= s.max_block_us, "{clock:?}: {s:?}");
        }
    }

    #[test]
    fn a_starved_stage_strikes_on_thread_cpu_too() {
        // 30 ms asleep, past the 20 ms cutoff: CPU time says idle, the
        // wall says the thread can't keep up.
        let mut r = real(
            Act::Sleep(Duration::from_millis(30)),
            StageClock::ThreadCpu,
            DEFAULT_STAGE_STARVATION_CUTOFF,
        );
        let seen = run(&mut r, 10, |_| {});
        let s = seen[1];
        assert_eq!(struck_after(&seen), Some(2), "{s:?}");
        assert_eq!(s.starved, 2, "{s:?}");
        assert!(s.max_block_us < 6_000 && s.max_wall_us >= 30_000, "{s:?}");
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
#[test]
fn without_a_thread_cpu_clock_the_wall_clock_times() {
    assert!(!StageClock::ThreadCpu.is_supported());
    assert_eq!(StageClock::default(), StageClock::Wall);
    let (stage, _) = Scripted::new();
    let mut r = StageRunner::with_source(
        Box::new(stage),
        TimeSource::Os,
        timing(StageClock::ThreadCpu, DEFAULT_STAGE_STARVATION_CUTOFF),
    )
    .expect("stage accepted");
    assert_eq!(r.stats().clock, StageClock::Wall);
}
