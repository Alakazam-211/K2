//! The capture stage: a caller's own processing (a noise-removal model, for
//! example) between echo cancellation and gain control, with a time budget
//! and a bypass that never shifts the timing.

use crate::error::{AudioError, ErrorCode};
use crate::frame::BLOCK_SAMPLES;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

/// A caller's own processing on the capture path, run once per 10 ms block
/// after the high-pass filter and echo cancellation and before gain control
/// (see [`crate::ProcessingConfig::capture_stage`]).
///
/// The pipeline with a stage set:
///
/// ```text
/// mic → 48 kHz → high-pass → echo cancellation (→ noise suppression, if set)
///     → [stage] → gain control + limiter → levels / VAD → Opus
/// ```
///
/// Rules for implementations:
///
/// - **`process`** runs on the engine's processing thread, synchronously,
///   once per block: 480 samples, 48 kHz mono, nominal range [-1, 1],
///   processed in place. It must not allocate, take a lock it could wait
///   on, block or panic, and it must return finite samples.
/// - **`latency_samples`** is read once, when the engine starts, and must
///   not change for the stage's life. It is the delay the stage adds (an
///   overlap-add denoiser with 10 ms hops reports 480). At most
///   [`MAX_STAGE_LATENCY_SAMPLES`].
/// - **`reset`** is called on the processing thread: **once before the
///   first `process`** (when the engine builds its processing core, before
///   any audio flows; a stage may allocate here, for example to plan its
///   FFTs), when the capture stream restarts (a device change or reopen)
///   and after a capture gap of more than a second (a suspend, for
///   example), and before the stage is retried after a bypass. It is not
///   called again if `process` hasn't run since the last reset. Clear the
///   state that carried the old audio. Only the first call may allocate.
///
/// The engine enforces a time budget (see [`StageBypassReason`]): a strike
/// is 2 consecutive calls of 9 ms or more, or a 99th percentile over 6 ms
/// across 100 calls; it is retried after 10 s of capture, and 3 strikes
/// within 60 s of capture make the bypass permanent (otherwise the count
/// decays). A panic or a non-finite sample is a fault: bypassed for good.
/// A stage that runs too long or misbehaves is bypassed, and the call goes on with
/// the echo canceller's output, delayed by `latency_samples` so the timing
/// never shifts; with
/// [`stage_fallback_noise_suppression`](crate::ProcessingConfig::stage_fallback_noise_suppression)
/// set (the default), sonora's noise suppression stands in while it is
/// bypassed. A mode switch inside the stage (an atomic it reads in
/// `process`) needs nothing from the engine.
pub trait CaptureStage: Send + 'static {
    /// Process one 10 ms block in place: 480 samples, 48 kHz mono.
    fn process(&mut self, block: &mut [f32; BLOCK_SAMPLES]);
    /// The delay the stage adds, in samples at 48 kHz. Fixed for the
    /// stage's life.
    fn latency_samples(&self) -> usize;
    /// Clear the stage's state: the stream restarted, or the stage is about
    /// to be retried.
    fn reset(&mut self);
}

/// The longest stage latency the engine accepts: one second.
pub const MAX_STAGE_LATENCY_SAMPLES: usize = 48_000;

/// Two consecutive `process` calls at or above this are a strike (one
/// alone is not: a single late block is usually the OS preempting the
/// thread, not a slow stage).
pub(crate) const HARD_BUDGET: Duration = Duration::from_millis(9);
/// Consecutive calls at or above [`HARD_BUDGET`] that make a strike.
pub(crate) const HARD_RUN: u32 = 2;
/// A strike when the 99th percentile of the last [`WINDOW`] calls is
/// above this (nearest rank: two of the hundred calls above it).
pub(crate) const P99_BUDGET: Duration = Duration::from_millis(6);
/// The sliding window for the percentile rule, in calls.
pub(crate) const WINDOW: usize = 100;
/// A bypassed stage is retried after this many blocks of capture (10 s).
pub(crate) const RETRY_BLOCKS: u64 = 1_000;
/// This many budget strikes within [`STRIKE_SPAN_BLOCKS`] make the bypass
/// permanent.
pub(crate) const MAX_STRIKES: usize = 3;
/// The span the strikes must fall within, in blocks of capture (60 s).
/// Older strikes no longer count toward a permanent bypass.
pub(crate) const STRIKE_SPAN_BLOCKS: u64 = 6_000;

/// Why the capture stage was bypassed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StageBypassReason {
    /// Over the time budget, a strike: 2 consecutive calls of 9 ms or more,
    /// or a 99th percentile over 6 ms across the last 100 calls. The stage
    /// is retried after 10 s of capture (it is reset first, and its output
    /// is used again once its latency has filled with live audio). 3
    /// strikes within 60 s of capture make the bypass permanent; otherwise
    /// the count decays (a strike older than 60 s no longer counts).
    Budget,
    /// The stage panicked (in `process` or `reset`) or returned NaN or
    /// infinite samples. It is never called again. (A panic is only caught
    /// when the build unwinds; with `panic = "abort"` it ends the process.)
    Fault,
}

/// The capture stage's state ([`crate::Stats::capture_stage`], and
/// [`crate::Event::CaptureStage`] whenever `active`, `bypassed`, `strikes`
/// or `permanent` change).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CaptureStageStats {
    /// The stage's output is what goes on: false while it is bypassed, and
    /// for the blocks it is warming up after a retry.
    pub active: bool,
    /// Sticky: why the stage was last bypassed, kept after a successful
    /// retry (`active` says whether it is running now). `None` if it never
    /// was.
    pub bypassed: Option<StageBypassReason>,
    /// Bypassed for good: a fault, or 3 strikes within 60 s of capture.
    pub permanent: bool,
    /// sonora's noise suppression is standing in for the bypassed stage
    /// right now (see
    /// [`crate::ProcessingConfig::stage_fallback_noise_suppression`]).
    pub fallback_noise_suppression: bool,
    /// Budget strikes and faults so far, all of them (only budget strikes
    /// within the last 60 s of capture count toward `permanent`).
    pub strikes: u32,
    /// The stage's latency, samples at 48 kHz. Not included in
    /// [`crate::EncodedFrame::ts48k`]: the audio in a frame is this much
    /// older than its timestamp says.
    pub latency_samples: u32,
    /// Blocks the stage processed (including the ones that struck).
    pub blocks_processed: u64,
    /// Blocks that went on as the delayed echo canceller output instead.
    pub blocks_bypassed: u64,
    /// The longest single `process` call, microseconds.
    pub max_block_us: u32,
    /// The 99th percentile (nearest rank) of the last 100 calls, or of all
    /// calls since the last retry or reset if fewer, microseconds.
    pub p99_block_us: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// The stage's output is used.
    Active,
    /// After a retry: the stage runs, the delayed input goes on, for this
    /// many more blocks.
    Warming(u32),
    /// The delayed input goes on. `Some(block)`: retry at that block count.
    Bypassed(Option<u64>),
}

/// What times the stage's calls: the wall clock, or (tests) a counter the
/// test advances, so budget behaviour doesn't depend on machine load.
#[derive(Clone, Debug)]
pub(crate) enum StageClock {
    /// [`Instant`].
    Wall,
    /// Nanoseconds, advanced by the test (a stage that "takes" time adds
    /// to it).
    #[cfg(test)]
    Manual(std::sync::Arc<std::sync::atomic::AtomicU64>),
}

impl StageClock {
    /// Time since `base`.
    fn now(&self, base: Instant) -> Duration {
        match self {
            StageClock::Wall => base.elapsed(),
            #[cfg(test)]
            StageClock::Manual(ns) => {
                Duration::from_nanos(ns.load(std::sync::atomic::Ordering::Relaxed))
            }
        }
    }
}

/// Runs a [`CaptureStage`] on the processing thread: the budget, the
/// bypass delay line and the counters. Everything is allocated up front.
pub(crate) struct StageRunner {
    clock: StageClock,
    epoch: Instant,
    /// Consecutive calls at or above [`HARD_BUDGET`].
    slow_run: u32,
    /// The blocks of the last budget strikes (a ring of [`MAX_STRIKES`]).
    strike_blocks: [u64; MAX_STRIKES],
    budget_strikes: usize,
    stage: Box<dyn CaptureStage>,
    latency: usize,
    /// The bypass delay line: the input, `latency` samples late.
    delay: Box<[f32]>,
    pos: usize,
    work: [f32; BLOCK_SAMPLES],
    /// Call times, microseconds (a ring of the last `win_len`).
    window: [u32; WINDOW],
    sorted: [u32; WINDOW],
    win_len: usize,
    win_pos: usize,
    over_p99: usize,
    mode: Mode,
    blocks: u64,
    stats: CaptureStageStats,
    changed: bool,
    /// `process` ran since the last reset.
    dirty: bool,
    /// This block's output was the stage's.
    used_stage: bool,
}

impl StageRunner {
    /// Build before any audio flows (it allocates the delay line), on the
    /// thread that will run it: it resets the stage once, so a stage that
    /// prepares itself in `reset` does so here, not in the first block.
    pub(crate) fn new(stage: Box<dyn CaptureStage>) -> Result<StageRunner, AudioError> {
        Self::with_clock(stage, StageClock::Wall)
    }

    /// [`StageRunner::new`] timed by `clock`.
    pub(crate) fn with_clock(
        stage: Box<dyn CaptureStage>,
        clock: StageClock,
    ) -> Result<StageRunner, AudioError> {
        let latency = stage.latency_samples();
        if latency > MAX_STAGE_LATENCY_SAMPLES {
            return Err(AudioError::new(
                ErrorCode::InvalidConfig,
                format!(
                    "capture stage latency {latency} samples is over {MAX_STAGE_LATENCY_SAMPLES}"
                ),
            ));
        }
        let mut runner = StageRunner {
            clock,
            epoch: Instant::now(),
            slow_run: 0,
            strike_blocks: [0; MAX_STRIKES],
            budget_strikes: 0,
            stage,
            latency,
            delay: vec![0.0; latency].into_boxed_slice(),
            pos: 0,
            work: [0.0; BLOCK_SAMPLES],
            window: [0; WINDOW],
            sorted: [0; WINDOW],
            win_len: 0,
            win_pos: 0,
            over_p99: 0,
            mode: Mode::Active,
            blocks: 0,
            stats: CaptureStageStats {
                active: true,
                latency_samples: latency as u32,
                ..CaptureStageStats::default()
            },
            changed: false,
            dirty: false,
            used_stage: false,
        };
        // The one reset before the first block. A panic here is a fault,
        // like any other.
        runner.reset_stage();
        Ok(runner)
    }

    /// The stage's latency, samples.
    pub(crate) fn latency(&self) -> usize {
        self.latency
    }

    /// Whether this block's output (the last `process`) was the stage's.
    pub(crate) fn used_stage(&self) -> bool {
        self.used_stage
    }

    /// Note whether sonora's noise suppression stands in (for the stats).
    pub(crate) fn set_fallback(&mut self, on: bool) {
        self.stats.fallback_noise_suppression = on;
    }

    /// Process one block in place: the stage's output, or (bypassed) the
    /// input delayed by the stage's latency.
    pub(crate) fn process(&mut self, block: &mut [f32; BLOCK_SAMPLES]) {
        self.blocks += 1;
        self.used_stage = false;
        self.work.copy_from_slice(block);
        self.delay_in_place(block);

        if let Mode::Bypassed(retry) = self.mode {
            match retry {
                Some(at) if self.blocks >= at => {
                    if !self.reset_stage() {
                        self.stats.blocks_bypassed += 1;
                        return;
                    }
                    self.clear_window();
                    self.mode = match self.warm_blocks() {
                        0 => Mode::Active,
                        n => Mode::Warming(n),
                    };
                    self.stats.active = self.mode == Mode::Active;
                    self.changed = true;
                }
                _ => {
                    self.stats.blocks_bypassed += 1;
                    return;
                }
            }
        }

        let t = self.clock.now(self.epoch);
        let r = catch_unwind(AssertUnwindSafe(|| self.stage.process(&mut self.work)));
        let took = self.clock.now(self.epoch).saturating_sub(t);
        self.dirty = true;
        self.stats.blocks_processed += 1;
        let us = took.as_micros().min(u32::MAX as u128) as u32;
        self.stats.max_block_us = self.stats.max_block_us.max(us);

        if r.is_err() || !self.work.iter().all(|v| v.is_finite()) {
            self.strike(StageBypassReason::Fault);
            self.stats.blocks_bypassed += 1;
            return;
        }
        self.push_time(us);
        // One late call alone is usually the thread being preempted: it
        // takes two in a row, or the percentile, to strike.
        self.slow_run = if took >= HARD_BUDGET {
            self.slow_run + 1
        } else {
            0
        };
        if self.slow_run >= HARD_RUN || (self.win_len == WINDOW && self.over_p99 >= 2) {
            self.strike(StageBypassReason::Budget);
            self.stats.blocks_bypassed += 1;
            return;
        }
        match self.mode {
            Mode::Active => {
                block.copy_from_slice(&self.work);
                self.used_stage = true;
            }
            Mode::Warming(n) => {
                self.stats.blocks_bypassed += 1;
                if n <= 1 {
                    self.mode = Mode::Active;
                    self.stats.active = true;
                    self.changed = true;
                } else {
                    self.mode = Mode::Warming(n - 1);
                }
            }
            Mode::Bypassed(_) => unreachable!("handled above"),
        }
    }

    /// The stream restarted or had a gap: clear the delay line and reset a
    /// running stage. A bypassed stage is reset when it is retried; a stage
    /// that hasn't processed anything since its last reset isn't reset again.
    pub(crate) fn reset(&mut self) {
        self.delay.fill(0.0);
        self.pos = 0;
        match self.mode {
            Mode::Active | Mode::Warming(_) if self.dirty => {
                if self.reset_stage() {
                    self.clear_window();
                    // Stage and delay line both start from silence, so the
                    // stage's output lines up at once.
                    if self.mode != Mode::Active {
                        self.mode = Mode::Active;
                        self.stats.active = true;
                        self.changed = true;
                    }
                }
            }
            Mode::Active | Mode::Warming(_) | Mode::Bypassed(_) => {}
        }
    }

    /// The counters, with the percentile over the current window.
    pub(crate) fn stats(&mut self) -> CaptureStageStats {
        let mut s = self.stats;
        s.p99_block_us = self.p99_us();
        s
    }

    /// True once after the state (active, bypassed, strikes) changed.
    pub(crate) fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Blocks to run a retried stage before its output is used: until its
    /// latency has filled with live audio.
    fn warm_blocks(&self) -> u32 {
        self.latency.div_ceil(BLOCK_SAMPLES) as u32
    }

    /// Reset the stage; a panic is a fault. Returns whether it went well.
    fn reset_stage(&mut self) -> bool {
        if catch_unwind(AssertUnwindSafe(|| self.stage.reset())).is_err() {
            self.strike(StageBypassReason::Fault);
            false
        } else {
            self.dirty = false;
            true
        }
    }

    fn strike(&mut self, why: StageBypassReason) {
        self.stats.strikes += 1;
        self.stats.bypassed = Some(why);
        self.stats.active = false;
        // Permanent on a fault, or on the third budget strike within 60 s
        // of capture: the oldest of the last three is recent enough.
        let mut permanent = why == StageBypassReason::Fault;
        if why == StageBypassReason::Budget {
            self.strike_blocks[self.budget_strikes % MAX_STRIKES] = self.blocks;
            self.budget_strikes += 1;
            if self.budget_strikes >= MAX_STRIKES {
                let oldest = self.strike_blocks[self.budget_strikes % MAX_STRIKES];
                permanent = self.blocks - oldest <= STRIKE_SPAN_BLOCKS;
            }
        }
        self.stats.permanent = permanent;
        self.mode = Mode::Bypassed((!permanent).then_some(self.blocks + RETRY_BLOCKS));
        self.clear_window();
        self.changed = true;
    }

    /// `block` in, the same samples `latency` later out.
    fn delay_in_place(&mut self, block: &mut [f32; BLOCK_SAMPLES]) {
        let l = self.latency;
        if l == 0 {
            return;
        }
        for v in block.iter_mut() {
            std::mem::swap(&mut self.delay[self.pos], v);
            self.pos += 1;
            if self.pos == l {
                self.pos = 0;
            }
        }
    }

    fn push_time(&mut self, us: u32) {
        let limit = P99_BUDGET.as_micros() as u32;
        if self.win_len == WINDOW {
            if self.window[self.win_pos] > limit {
                self.over_p99 -= 1;
            }
        } else {
            self.win_len += 1;
        }
        self.window[self.win_pos] = us;
        if us > limit {
            self.over_p99 += 1;
        }
        self.win_pos = (self.win_pos + 1) % WINDOW;
    }

    /// Start the timing rules afresh (after a strike, a retry or a reset).
    fn clear_window(&mut self) {
        self.win_len = 0;
        self.win_pos = 0;
        self.over_p99 = 0;
        self.slow_run = 0;
    }

    fn p99_us(&mut self) -> u32 {
        let n = self.win_len;
        if n == 0 {
            return 0;
        }
        // The ring's live entries are its first `n` slots until it fills.
        let s = &mut self.sorted[..n];
        s.copy_from_slice(&self.window[..n]);
        let rank = (n * 99).div_ceil(100).max(1) - 1;
        *s.select_nth_unstable(rank).1
    }
}
