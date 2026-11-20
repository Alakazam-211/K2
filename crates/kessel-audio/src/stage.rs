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
///
/// **The clock.** By default ([`StageClock::ThreadCpu`], where the platform
/// has it) a call is timed by the processing thread's own CPU time, so time
/// the OS spends running other threads (the camera, a video encoder) while
/// the stage waits doesn't count against the stage. Real starvation still
/// does: a call that takes the
/// [starvation cutoff](crate::ProcessingConfig::stage_starvation_cutoff)
/// (20 ms by default) or more on the wall clock counts as slow toward the
/// 2-in-a-row rule, whatever its CPU time. [`StageClock::Wall`] times calls
/// by the wall clock, as 0.2.2 and earlier did
/// ([`stage_clock`](crate::ProcessingConfig::stage_clock)).
///
/// **The grace window.** OS work the stage doesn't control (the call
/// starting, the audio session activating, a route change, the camera
/// starting, the app changing scene) can make a few calls slow. For a
/// short window after such an event (1.5 s of capture by default,
/// [`stage_grace`](crate::ProcessingConfig::stage_grace)) slow calls don't
/// count toward a strike. They are still timed and reported
/// ([`CaptureStageStats::grace_over_budget`], `max_block_us`,
/// `p99_block_us`, and an [`Event::CaptureStage`](crate::Event::CaptureStage)
/// when a window that excused any closes). The host opens the window with
/// [`AudioEngine::transient_event`](crate::AudioEngine::transient_event); the
/// engine opens it itself when the stage starts and whenever the capture
/// stream starts or restarts (a device opening or reopening, the end of a
/// gap). Opening it again while it is open extends it. The grace covers
/// only the time budget: a fault bypasses the stage at once, in the window
/// or not.
///
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

/// The default grace window
/// ([`ProcessingConfig::stage_grace`](crate::ProcessingConfig::stage_grace)):
/// 1.5 s of capture after a [`TransientEvent`], the stage's start or a
/// capture stream (re)start, during which slow calls don't count toward a
/// strike.
///
/// Long enough to cover one burst of OS work (an audio unit or stream
/// reopening, a scene change), short enough that a stage that really is too
/// slow is still bypassed about 1.5 s later: at most 150 slow blocks get
/// through, against the 10 s of bypass a needless strike costs.
pub const DEFAULT_STAGE_GRACE: Duration = Duration::from_millis(1_500);

/// The longest grace window the engine accepts: 10 s, the retry period.
pub const MAX_STAGE_GRACE: Duration = Duration::from_secs(10);

/// The default starvation cutoff
/// ([`ProcessingConfig::stage_starvation_cutoff`](crate::ProcessingConfig::stage_starvation_cutoff)):
/// with [`StageClock::ThreadCpu`], a call that takes 20 ms or more on the
/// wall clock counts as slow toward the 2-in-a-row rule, whatever its CPU
/// time.
///
/// Two blocks of real time, more than twice the 9 ms hard budget: a
/// preempted call that still finishes within two blocks is left alone, and
/// two in a row past it (a strike) are 40 ms of real time for 20 ms of
/// audio, more than the 30 ms of playout queued by default. That is a
/// thread that can't keep up, whoever is to blame.
pub const DEFAULT_STAGE_STARVATION_CUTOFF: Duration = Duration::from_millis(20);

/// What times the capture stage's calls for its time budget
/// ([`ProcessingConfig::stage_clock`](crate::ProcessingConfig::stage_clock);
/// see [`CaptureStage`]).
///
/// The default ([`StageClock::default`]) is [`StageClock::ThreadCpu`] where
/// the platform supports it (macOS, iOS, Linux and Android), and
/// [`StageClock::Wall`] elsewhere. Asking for `ThreadCpu` where it isn't
/// supported times by the wall clock; [`CaptureStageStats::clock`] says
/// which clock is in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StageClock {
    /// The processing thread's own CPU time
    /// (`clock_gettime(CLOCK_THREAD_CPUTIME_ID)`), read around each call:
    /// time the OS gives other threads while the stage is preempted doesn't
    /// count. A call that takes the
    /// [starvation cutoff](crate::ProcessingConfig::stage_starvation_cutoff)
    /// or more on the wall clock still counts as slow toward the 2-in-a-row
    /// rule, so a thread that really is starved is still caught.
    ///
    /// Supported on macOS, iOS, Linux and Android. Not on Windows:
    /// `GetThreadTimes` counts in clock-interrupt ticks (15.6 ms by
    /// default), coarser than the 6 and 9 ms budgets, so Windows times by
    /// the wall clock.
    ThreadCpu,
    /// The wall clock ([`std::time::Instant`]): everything between the
    /// call's start and end counts, the OS running other threads included.
    /// 0.2.2 and earlier always timed this way.
    Wall,
}

impl StageClock {
    /// Whether this clock works on the platform the crate was built for.
    /// [`StageClock::Wall`] always does; [`StageClock::ThreadCpu`] on macOS,
    /// iOS, Linux and Android.
    pub const fn is_supported(self) -> bool {
        match self {
            StageClock::ThreadCpu => thread_cpu::SUPPORTED,
            StageClock::Wall => true,
        }
    }
}

impl Default for StageClock {
    /// [`StageClock::ThreadCpu`] where it is supported, else
    /// [`StageClock::Wall`].
    fn default() -> Self {
        if thread_cpu::SUPPORTED {
            StageClock::ThreadCpu
        } else {
            StageClock::Wall
        }
    }
}

/// The calling thread's CPU time.
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
))]
pub(crate) mod thread_cpu {
    use std::mem::MaybeUninit;
    use std::time::Duration;

    pub(crate) const SUPPORTED: bool = true;

    /// The CPU time the calling thread has used, or `None` if the OS
    /// refused (it never should).
    #[allow(unsafe_code)]
    pub(crate) fn now() -> Option<Duration> {
        let mut ts = MaybeUninit::<libc::timespec>::uninit();
        // SAFETY: `ts` is valid for writes of one `timespec`, which is all
        // clock_gettime writes, and is read only after the call succeeded,
        // when it has been written.
        let ts = unsafe {
            if libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, ts.as_mut_ptr()) != 0 {
                return None;
            }
            ts.assume_init()
        };
        Some(Duration::new(
            u64::try_from(ts.tv_sec).ok()?,
            u32::try_from(ts.tv_nsec).ok()?,
        ))
    }
}

/// No thread CPU clock here: [`StageClock::ThreadCpu`] times by the wall
/// clock.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
pub(crate) mod thread_cpu {
    use std::time::Duration;

    pub(crate) const SUPPORTED: bool = false;

    pub(crate) fn now() -> Option<Duration> {
        None
    }
}

/// How the capture stage is timed: the grace window, the clock and the
/// starvation cutoff (from [`crate::ProcessingConfig`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StageTiming {
    /// [`crate::ProcessingConfig::stage_grace`].
    pub(crate) grace: Duration,
    /// [`crate::ProcessingConfig::stage_clock`].
    pub(crate) clock: StageClock,
    /// [`crate::ProcessingConfig::stage_starvation_cutoff`].
    pub(crate) starvation_cutoff: Duration,
}

impl Default for StageTiming {
    fn default() -> Self {
        StageTiming {
            grace: DEFAULT_STAGE_GRACE,
            clock: StageClock::default(),
            starvation_cutoff: DEFAULT_STAGE_STARVATION_CUTOFF,
        }
    }
}

/// Something the host knows is happening that can stall the processing
/// thread for a moment, for
/// [`AudioEngine::transient_event`](crate::AudioEngine::transient_event):
/// it opens (or extends) the capture stage's grace window, so slow calls
/// right after it don't count toward a strike (see [`CaptureStage`]).
///
/// The kind only says why the window opened (it is kept in
/// [`CaptureStageStats::last_transient`]); every kind opens the same window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TransientEvent {
    /// The call is starting (joining, connecting media).
    CallStart,
    /// The OS's audio session became active (on iOS,
    /// `provider(_:didActivate:)`). On the phone backend,
    /// `AudioEngine::session_activated` reports it by itself.
    AudioSessionActivated,
    /// The audio route changed (headphones, Bluetooth, the speaker). On the
    /// phone backend, `AudioEngine::route_changed` reports it by itself.
    RouteChange,
    /// The camera is starting.
    CameraStart,
    /// The app changed scene: to or from the background, another app in
    /// front, picture in picture.
    AppSceneChange,
    /// Anything else the host knows to be a short burst of OS work.
    Other,
}

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
    /// The longest single `process` call, microseconds, on the stage's
    /// [`clock`](Self::clock) (CPU time with [`StageClock::ThreadCpu`]; the
    /// wall-clock reading is [`max_wall_us`](Self::max_wall_us)).
    pub max_block_us: u32,
    /// The 99th percentile (nearest rank) of the last 100 calls, or of all
    /// calls since the last retry or reset if fewer, microseconds, on the
    /// stage's [`clock`](Self::clock). Calls in a grace window are included.
    pub p99_block_us: u32,
    /// A grace window is open now: slow calls don't count toward a strike
    /// (see [`CaptureStage`]).
    pub grace: bool,
    /// Blocks the stage processed inside a grace window, all of them.
    pub grace_blocks: u64,
    /// Of those, calls over the budget (over 6 ms) that didn't count toward
    /// a strike because of the grace window, all of them. They are in
    /// `max_block_us` and `p99_block_us` as usual.
    pub grace_over_budget: u64,
    /// The last [`TransientEvent`] the host reported, if any.
    pub last_transient: Option<TransientEvent>,
    /// The clock timing the stage's calls: the configured
    /// [`stage_clock`](crate::ProcessingConfig::stage_clock), or
    /// [`StageClock::Wall`] where [`StageClock::ThreadCpu`] isn't supported.
    pub clock: StageClock,
    /// The longest single `process` call on the wall clock, microseconds,
    /// whatever the [`clock`](Self::clock).
    pub max_wall_us: u32,
    /// With [`StageClock::ThreadCpu`]: calls over the budget (over 6 ms) on
    /// the wall clock that weren't over it in CPU time, so didn't count as
    /// slow (the thread was preempted; the stage wasn't slow), all of them.
    /// Starved calls are included. Always 0 with [`StageClock::Wall`].
    pub preempted_over_budget: u64,
    /// With [`StageClock::ThreadCpu`]: calls that took the
    /// [starvation cutoff](crate::ProcessingConfig::stage_starvation_cutoff)
    /// or more on the wall clock, which count as slow whatever their CPU
    /// time, all of them (in a grace window too). Always 0 with
    /// [`StageClock::Wall`].
    pub starved: u64,
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

/// Where the stage's call times come from: the OS's clocks, or (tests)
/// counters the test advances, so budget behaviour doesn't depend on
/// machine load.
#[derive(Clone, Debug)]
pub(crate) enum TimeSource {
    /// [`Instant`] and the thread's CPU time ([`thread_cpu::now`]).
    Os,
    /// See [`ManualTime`].
    #[cfg(test)]
    Manual(std::sync::Arc<ManualTime>),
}

/// A test clock: a stage that "takes" time advances it. Work advances CPU
/// and wall time; preemption ([`ManualTime::preempt_us`]) only wall time.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct ManualTime {
    cpu_ns: std::sync::atomic::AtomicU64,
    wall_ns: std::sync::atomic::AtomicU64,
}

#[cfg(test)]
impl ManualTime {
    /// The thread works for `us`: CPU and wall time both advance.
    pub(crate) fn work_us(&self, us: u64) {
        use std::sync::atomic::Ordering::Relaxed;
        self.cpu_ns.fetch_add(us * 1_000, Relaxed);
        self.wall_ns.fetch_add(us * 1_000, Relaxed);
    }

    /// The thread is preempted (or sleeps) for `us`: only wall time
    /// advances.
    pub(crate) fn preempt_us(&self, us: u64) {
        self.wall_ns
            .fetch_add(us * 1_000, std::sync::atomic::Ordering::Relaxed);
    }
}

/// One reading of a [`TimeSource`].
#[derive(Clone, Copy)]
struct Reading {
    wall: Duration,
    /// `None` when not asked for, or the OS refused.
    cpu: Option<Duration>,
}

impl TimeSource {
    /// Read the wall clock (since `base`) and, if `cpu`, the thread's CPU
    /// time. At a call's `start` the wall clock is read first, at its end
    /// last, so the wall interval holds the CPU one.
    fn read(&self, base: Instant, cpu: bool, start: bool) -> Reading {
        match self {
            TimeSource::Os => {
                let cpu_now = || if cpu { thread_cpu::now() } else { None };
                if start {
                    let wall = base.elapsed();
                    Reading {
                        wall,
                        cpu: cpu_now(),
                    }
                } else {
                    let cpu = cpu_now();
                    Reading {
                        wall: base.elapsed(),
                        cpu,
                    }
                }
            }
            #[cfg(test)]
            TimeSource::Manual(t) => {
                use std::sync::atomic::Ordering::Relaxed;
                Reading {
                    wall: Duration::from_nanos(t.wall_ns.load(Relaxed)),
                    cpu: cpu.then(|| Duration::from_nanos(t.cpu_ns.load(Relaxed))),
                }
            }
        }
    }

    /// The clock to use for `want`: [`StageClock::ThreadCpu`] only if this
    /// source can read the thread's CPU time.
    fn resolve(&self, want: StageClock) -> StageClock {
        match want {
            StageClock::ThreadCpu if self.read(Instant::now(), true, true).cpu.is_some() => {
                StageClock::ThreadCpu
            }
            _ => StageClock::Wall,
        }
    }
}

/// Runs a [`CaptureStage`] on the processing thread: the budget, the
/// bypass delay line and the counters. Everything is allocated up front.
pub(crate) struct StageRunner {
    time: TimeSource,
    /// Timed by the thread's CPU time ([`StageClock::ThreadCpu`], resolved).
    cpu_clock: bool,
    /// With `cpu_clock`: a call this long or longer on the wall clock counts
    /// as slow toward the 2-in-a-row rule.
    starvation_cutoff: Duration,
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
    /// Which of `window`'s calls were in a grace window (they don't count
    /// toward the percentile rule's strike).
    graced: [bool; WINDOW],
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
    /// The grace window's length, blocks.
    grace_len: u64,
    /// The grace window covers the blocks up to this one (`blocks` counts
    /// from 1).
    grace_until: u64,
    /// `grace_over_budget` when the open window opened.
    grace_mark: u64,
}

impl StageRunner {
    /// Build before any audio flows (it allocates the delay line), on the
    /// thread that will run it: it resets the stage once, so a stage that
    /// prepares itself in `reset` does so here, not in the first block.
    /// `timing` holds the grace window's length (the window opens here, for
    /// the stage's first blocks), the clock and the starvation cutoff.
    pub(crate) fn new(
        stage: Box<dyn CaptureStage>,
        timing: StageTiming,
    ) -> Result<StageRunner, AudioError> {
        Self::with_source(stage, TimeSource::Os, timing)
    }

    /// [`StageRunner::new`] with call times from `time`.
    pub(crate) fn with_source(
        stage: Box<dyn CaptureStage>,
        time: TimeSource,
        timing: StageTiming,
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
        let clock = time.resolve(timing.clock);
        let mut runner = StageRunner {
            time,
            cpu_clock: clock == StageClock::ThreadCpu,
            starvation_cutoff: timing.starvation_cutoff,
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
            graced: [false; WINDOW],
            sorted: [0; WINDOW],
            win_len: 0,
            win_pos: 0,
            over_p99: 0,
            mode: Mode::Active,
            blocks: 0,
            stats: CaptureStageStats {
                active: true,
                latency_samples: latency as u32,
                clock,
                ..CaptureStageStats::default()
            },
            changed: false,
            dirty: false,
            used_stage: false,
            grace_len: grace_blocks(timing.grace),
            grace_until: 0,
            grace_mark: 0,
        };
        // The one reset before the first block. A panic here is a fault,
        // like any other.
        runner.reset_stage();
        // The stage's own start: its first calls can be slow for reasons
        // that aren't its own (cold caches, page faults, the OS settling).
        runner.open_grace(None);
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

    /// Open the grace window, or extend the open one, for the next
    /// `grace_len` blocks of capture. `event` is the host's reason, if it
    /// gave one. A zero-length window does nothing.
    pub(crate) fn open_grace(&mut self, event: Option<TransientEvent>) {
        if event.is_some() {
            self.stats.last_transient = event;
        }
        if self.grace_len == 0 {
            return;
        }
        if !self.stats.grace {
            self.stats.grace = true;
            self.grace_mark = self.stats.grace_over_budget;
        }
        self.grace_until = self
            .grace_until
            .max(self.blocks.saturating_add(self.grace_len));
    }

    /// Process one block in place: the stage's output, or (bypassed) the
    /// input delayed by the stage's latency.
    pub(crate) fn process(&mut self, block: &mut [f32; BLOCK_SAMPLES]) {
        self.blocks += 1;
        self.used_stage = false;
        let grace = self.blocks <= self.grace_until;
        if self.stats.grace && !grace {
            // The window closed. If it excused slow calls, say so now
            // (`Event::CaptureStage`), so they aren't hidden.
            self.stats.grace = false;
            if self.stats.grace_over_budget > self.grace_mark {
                self.changed = true;
            }
        }
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

        let t0 = self.time.read(self.epoch, self.cpu_clock, true);
        let r = catch_unwind(AssertUnwindSafe(|| self.stage.process(&mut self.work)));
        let t1 = self.time.read(self.epoch, self.cpu_clock, false);
        let wall = t1.wall.saturating_sub(t0.wall);
        // What counts toward the budget: the thread's CPU time, or the wall
        // time (the wall clock, or a CPU reading the OS refused). A starved
        // call (wall time at or over the cutoff) is slow whatever its CPU
        // time.
        let (took, starved) = match (t0.cpu, t1.cpu) {
            (Some(a), Some(b)) => {
                let cpu = b.saturating_sub(a);
                if wall > P99_BUDGET && cpu <= P99_BUDGET {
                    self.stats.preempted_over_budget += 1;
                }
                let starved = wall >= self.starvation_cutoff;
                if starved {
                    self.stats.starved += 1;
                }
                (cpu, starved)
            }
            _ => (wall, false),
        };
        self.dirty = true;
        self.stats.blocks_processed += 1;
        let us = micros(took);
        self.stats.max_block_us = self.stats.max_block_us.max(us);
        self.stats.max_wall_us = self.stats.max_wall_us.max(micros(wall));
        if grace {
            self.stats.grace_blocks += 1;
            if took > P99_BUDGET || starved {
                self.stats.grace_over_budget += 1;
            }
        }

        if r.is_err() || !self.work.iter().all(|v| v.is_finite()) {
            self.strike(StageBypassReason::Fault);
            self.stats.blocks_bypassed += 1;
            return;
        }
        self.push_time(us, grace);
        // One late call alone is usually the thread being preempted: it
        // takes two in a row, or the percentile, to strike. A starved call
        // counts toward the two in a row, not the percentile: one stall is
        // the OS's, two in a row a thread that can't keep up. In a grace
        // window no call counts.
        self.slow_run = if (took >= HARD_BUDGET || starved) && !grace {
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
    /// Either way the grace window opens: a stream (re)start is OS work.
    pub(crate) fn reset(&mut self) {
        self.open_grace(None);
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

    /// Add a call's time to the window. A call in a grace window (`graced`)
    /// stays in the percentile reading but doesn't count toward its strike.
    fn push_time(&mut self, us: u32, graced: bool) {
        let limit = P99_BUDGET.as_micros() as u32;
        if self.win_len == WINDOW {
            if self.window[self.win_pos] > limit && !self.graced[self.win_pos] {
                self.over_p99 -= 1;
            }
        } else {
            self.win_len += 1;
        }
        self.window[self.win_pos] = us;
        self.graced[self.win_pos] = graced;
        if us > limit && !graced {
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

/// `d` in whole microseconds, at most `u32::MAX`.
fn micros(d: Duration) -> u32 {
    d.as_micros().min(u32::MAX as u128) as u32
}

/// A grace window's length in 10 ms blocks, rounded up.
fn grace_blocks(grace: Duration) -> u64 {
    let block = Duration::from_millis(10).as_nanos();
    grace.as_nanos().div_ceil(block).min(u64::MAX as u128) as u64
}
