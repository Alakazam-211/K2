//! How the OS schedules the processing thread (`kessel-audio-dsp`).
//!
//! What the thread asks for is [`DspSchedConfig`]
//! ([`crate::EngineConfig::dsp_sched`]); its [`DspSchedMode`] picks one of
//! four set-ups on macOS and iOS:
//!
//! - [`DspSchedMode::Workgroup`] (the default, 0.2.4's behaviour), in this
//!   order, keeping the best it got:
//!   1. the QoS class `QOS_CLASS_USER_INTERACTIVE`
//!      (`pthread_set_qos_class_self_np`), the least it should run at;
//!   2. then Mach's time-constraint policy (`THREAD_TIME_CONSTRAINT_POLICY`),
//!      which replaces the QoS class (Darwin then reports none for the
//!      thread): the real-time band, with [`DspSchedConfig::time_constraint`]
//!      (by default a 10 ms period, the block; 6 ms of computation, the
//!      stage's percentile budget; a 9 ms constraint, its hard budget);
//!   3. once the phone backend's audio unit runs (iOS 14 or later), the
//!      unit's I/O thread's OS workgroup
//!      (`kAudioOutputUnitProperty_OSWorkgroup`, `os_workgroup_join`), so the
//!      OS schedules the processing thread with the I/O thread it works in
//!      step with. Only a time-constraint thread joins. The thread leaves
//!      when the unit closes, and before it exits.
//! - [`DspSchedMode::TimeConstraintOnly`]: steps 1 and 2, never a workgroup.
//! - [`DspSchedMode::None`]: nothing; the OS's default scheduling (before
//!   0.2.4).
//! - [`DspSchedMode::OwnWorkgroup`]: steps 1 and 2, then an audio work
//!   interval of the thread's own (`AudioWorkIntervalCreate`, macOS 11 and
//!   iOS 14 or later), joined at once. Each pass with capture audio is one
//!   interval: `os_workgroup_interval_start` before the first block, with a
//!   deadline [`DspSchedConfig::interval_deadline`] (3 ms by default) after
//!   it starts; `os_workgroup_interval_update` before each further block of
//!   a backlog, moving the deadline on; `os_workgroup_interval_finish` after
//!   the last.
//!
//! In [`DspSchedMode::Workgroup`] each step the OS refuses leaves the thread
//! on the step before. In the other modes a set-up step the OS refuses (the
//! time-constraint policy; creating, joining or running the work interval)
//! falls back to [`DspSchedMode::Workgroup`], and [`DspSchedStats`] says
//! why. [`DspScheduling`] reports what the OS granted, in
//! [`crate::Stats::dsp_scheduling`] and
//! [`crate::CaptureStageStats::dsp_scheduling`]; [`DspSchedStats`] (in
//! [`crate::Stats::dsp_sched`]) the mode asked for, the mode in effect, and
//! the figures asked for.
//!
//! Elsewhere (Linux, Android, Windows) nothing changes: the thread keeps the
//! OS's default scheduling and polls, and the mode in effect is
//! [`DspSchedMode::Unsupported`].
//!
//! The order of the steps is plain code over [`SchedOs`], so the fallbacks
//! are tested with a fake on any OS; the OS calls are in `apple`.

use crate::error::{AudioError, ErrorCode};
use std::marker::PhantomData;
use std::str::FromStr;
use std::time::Duration;

/// How the OS schedules the processing thread (`kessel-audio-dsp`): the best
/// of the requests it made that the OS granted. In
/// [`crate::Stats::dsp_scheduling`] and
/// [`crate::CaptureStageStats::dsp_scheduling`], so call logs show it.
///
/// On macOS and iOS the thread asks for each in turn, from
/// [`DspScheduling::Qos`] up, as its [`DspSchedMode`] says; each one the OS
/// refuses leaves it on the one before. On other platforms, and in
/// [`DspSchedMode::None`], it asks for none of them
/// ([`DspScheduling::None`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DspScheduling {
    /// Joined the audio unit's I/O thread's OS workgroup
    /// (`kAudioOutputUnitProperty_OSWorkgroup`, iOS 14 or later), on top of
    /// [`DspScheduling::TimeConstraint`]: the OS schedules the thread with
    /// the audio I/O thread. Only the phone backend on iOS joins, and only
    /// while its audio unit runs; the thread is back on
    /// [`DspScheduling::TimeConstraint`] when the unit closes.
    Workgroup,
    /// Mach's real-time time-constraint policy
    /// (`THREAD_TIME_CONSTRAINT_POLICY`), asked for after the QoS class and
    /// replacing it, with [`DspSchedConfig::time_constraint`] (by default a
    /// 10 ms period, 6 ms of computation and a 9 ms constraint: the block,
    /// and the capture stage's two budgets).
    TimeConstraint,
    /// Only the QoS class `QOS_CLASS_USER_INTERACTIVE`: the OS refused the
    /// time-constraint policy.
    Qos,
    /// The OS's default scheduling: on Linux, Android and Windows, in
    /// [`DspSchedMode::None`], or where the OS refused everything.
    #[default]
    None,
    /// Joined the thread's own audio work interval
    /// ([`DspSchedMode::OwnWorkgroup`]), on top of
    /// [`DspScheduling::TimeConstraint`]: each pass with capture audio is an
    /// interval with its own deadline. New in 0.2.5, and last, so the 0.2.4
    /// variants keep their discriminants.
    OwnWorkgroup,
}

impl DspScheduling {
    /// A short name for logs: `"workgroup"`, `"time_constraint"`,
    /// `"qos"`, `"none"` or `"own_workgroup"`.
    pub const fn as_str(self) -> &'static str {
        match self {
            DspScheduling::Workgroup => "workgroup",
            DspScheduling::TimeConstraint => "time_constraint",
            DspScheduling::Qos => "qos",
            DspScheduling::None => "none",
            DspScheduling::OwnWorkgroup => "own_workgroup",
        }
    }
}

/// Which scheduling the processing thread asks the OS for
/// ([`DspSchedConfig::mode`]). Only macOS and iOS act on it; elsewhere the
/// mode in effect is [`DspSchedMode::Unsupported`].
///
/// Parse one from a setting with [`str::parse`] (the [`DspSchedMode::as_str`]
/// names, any case, `-` or `_`), or read the environment with
/// [`DspSchedConfig::from_env`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DspSchedMode {
    /// The QoS class, then the time-constraint policy, then the audio unit's
    /// I/O workgroup while the phone backend's unit runs: 0.2.4's behaviour,
    /// and the default. The other modes fall back to this one when the OS
    /// refuses their set-up.
    #[default]
    Workgroup,
    /// The QoS class and the time-constraint policy
    /// (`thread_policy_set(THREAD_TIME_CONSTRAINT_POLICY)`) with
    /// [`DspSchedConfig::time_constraint`], and no workgroup.
    TimeConstraintOnly,
    /// No requests: the OS's default scheduling, as before 0.2.4. The
    /// thread is still woken by the capture callback.
    None,
    /// The QoS class and the time-constraint policy, then an audio work
    /// interval of the thread's own (`AudioWorkIntervalCreate`, macOS 11 and
    /// iOS 14 or later), joined, instead of the audio unit's I/O workgroup.
    /// Each pass with capture audio runs as one interval, due
    /// [`DspSchedConfig::interval_deadline`] after it starts.
    OwnWorkgroup,
    /// Only reported, in [`DspSchedStats::mode_effective`]: this platform
    /// has none of these calls (Linux, Android, Windows). As a request it is
    /// [`ErrorCode::InvalidConfig`].
    Unsupported,
}

impl DspSchedMode {
    /// A short name for logs and settings: `"workgroup"`,
    /// `"time_constraint_only"`, `"none"`, `"own_workgroup"` or
    /// `"unsupported"`.
    pub const fn as_str(self) -> &'static str {
        match self {
            DspSchedMode::Workgroup => "workgroup",
            DspSchedMode::TimeConstraintOnly => "time_constraint_only",
            DspSchedMode::None => "none",
            DspSchedMode::OwnWorkgroup => "own_workgroup",
            DspSchedMode::Unsupported => "unsupported",
        }
    }

    /// Whether the mode asks for the QoS class and the time-constraint
    /// policy.
    const fn wants_time_constraint(self) -> bool {
        matches!(
            self,
            DspSchedMode::Workgroup | DspSchedMode::TimeConstraintOnly | DspSchedMode::OwnWorkgroup
        )
    }
}

impl FromStr for DspSchedMode {
    type Err = AudioError;

    /// A mode by its [`DspSchedMode::as_str`] name, ignoring case and
    /// surrounding space, with `-` for `_`; `"time_constraint"` is also
    /// [`DspSchedMode::TimeConstraintOnly`]. `"unsupported"` (only
    /// reported) and anything else is [`ErrorCode::InvalidConfig`].
    fn from_str(s: &str) -> Result<Self, AudioError> {
        let name = s.trim().to_ascii_lowercase().replace('-', "_");
        match name.as_str() {
            "workgroup" => Ok(DspSchedMode::Workgroup),
            "time_constraint_only" | "time_constraint" => Ok(DspSchedMode::TimeConstraintOnly),
            "none" => Ok(DspSchedMode::None),
            "own_workgroup" => Ok(DspSchedMode::OwnWorkgroup),
            _ => Err(AudioError::new(
                ErrorCode::InvalidConfig,
                format!(
                    "unknown DSP scheduling mode {s:?} (workgroup, time_constraint_only, none or own_workgroup)"
                ),
            )),
        }
    }
}

/// The time-constraint policy's figures (`thread_time_constraint_policy`),
/// as durations; the thread converts them to Mach absolute-time units.
///
/// `#[non_exhaustive]`: start from [`DspTimeConstraint::default`] or
/// [`DspTimeConstraint::new`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct DspTimeConstraint {
    /// How often the thread has work: one 10 ms block by default.
    pub period: Duration,
    /// The CPU time it needs per period before the OS may preempt it for
    /// another real-time thread: by default 6 ms, the capture stage's
    /// percentile budget. Darwin raises a computation under half the
    /// constraint to half the constraint (`<mach/thread_policy.h>`), and
    /// refuses one outside its real-time quantum bounds (the thread then
    /// reports the refusal; see [`DspSchedFallback::TimeConstraintRefused`]).
    pub computation: Duration,
    /// The time from the start of the period by which that work must be
    /// done: by default 9 ms, the stage's hard budget (a block that takes
    /// longer strikes the stage).
    pub constraint: Duration,
}

impl DspTimeConstraint {
    /// The figures 0.2.4 asks for: a 10 ms period, 6 ms of computation and
    /// a 9 ms constraint.
    pub const DEFAULT: DspTimeConstraint = DspTimeConstraint {
        period: Duration::from_millis(10),
        computation: crate::stage::P99_BUDGET,
        constraint: crate::stage::HARD_BUDGET,
    };

    /// These figures. [`crate::AudioEngine::start`] checks them: more than
    /// zero computation, computation at most the constraint, the constraint
    /// at most the period, and the period at most 1 s.
    pub const fn new(period: Duration, computation: Duration, constraint: Duration) -> Self {
        DspTimeConstraint {
            period,
            computation,
            constraint,
        }
    }

    fn validate(&self) -> Result<(), AudioError> {
        if self.computation.is_zero()
            || self.computation > self.constraint
            || self.constraint > self.period
            || self.period > Duration::from_secs(1)
        {
            return Err(AudioError::new(
                ErrorCode::InvalidConfig,
                format!(
                    "dsp_sched.time_constraint needs 0 < computation <= constraint <= period <= 1 s (got {:?} / {:?} / {:?})",
                    self.computation, self.constraint, self.period
                ),
            ));
        }
        Ok(())
    }
}

impl Default for DspTimeConstraint {
    fn default() -> Self {
        DspTimeConstraint::DEFAULT
    }
}

/// What the processing thread asks the OS for
/// ([`crate::EngineConfig::dsp_sched`]): the mode, the time-constraint
/// figures and, for [`DspSchedMode::OwnWorkgroup`], each interval's
/// deadline. Read once, as the thread starts.
///
/// `#[non_exhaustive]`: start from [`DspSchedConfig::default`] (0.2.4's
/// behaviour), [`DspSchedConfig::new`], [`DspSchedConfig::from_env`] or
/// [`DspSchedConfig::from_lookup`], and chain the `with_` methods.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct DspSchedConfig {
    /// Which set-up to ask for. [`DspSchedMode::Workgroup`] by default.
    pub mode: DspSchedMode,
    /// The time-constraint policy's figures, in every mode that asks for
    /// the policy. [`DspTimeConstraint::DEFAULT`] by default.
    pub time_constraint: DspTimeConstraint,
    /// [`DspSchedMode::OwnWorkgroup`] only: how long after an interval
    /// starts (the thread starts a block as soon as the capture callback
    /// wakes it) its work is due. 3 ms by default; more than zero and at
    /// most the time-constraint period. Other modes ignore it.
    pub interval_deadline: Duration,
}

impl Default for DspSchedConfig {
    fn default() -> Self {
        DspSchedConfig::new(DspSchedMode::Workgroup)
    }
}

impl DspSchedConfig {
    /// The default interval deadline for [`DspSchedMode::OwnWorkgroup`]:
    /// 3 ms after the interval starts.
    pub const DEFAULT_INTERVAL_DEADLINE: Duration = Duration::from_millis(3);

    /// The environment variable [`DspSchedConfig::from_env`] reads the mode
    /// from: a [`DspSchedMode`] name, such as `own_workgroup`.
    pub const ENV_MODE: &'static str = "KESSEL_AUDIO_DSP_SCHED";
    /// The environment variable for [`DspSchedConfig::interval_deadline`],
    /// in whole microseconds.
    pub const ENV_INTERVAL_DEADLINE_US: &'static str = "KESSEL_AUDIO_DSP_INTERVAL_DEADLINE_US";
    /// The environment variable for [`DspTimeConstraint::period`], in whole
    /// microseconds.
    pub const ENV_PERIOD_US: &'static str = "KESSEL_AUDIO_DSP_PERIOD_US";
    /// The environment variable for [`DspTimeConstraint::computation`], in
    /// whole microseconds.
    pub const ENV_COMPUTATION_US: &'static str = "KESSEL_AUDIO_DSP_COMPUTATION_US";
    /// The environment variable for [`DspTimeConstraint::constraint`], in
    /// whole microseconds.
    pub const ENV_CONSTRAINT_US: &'static str = "KESSEL_AUDIO_DSP_CONSTRAINT_US";

    /// `mode`, with the default figures and deadline.
    pub const fn new(mode: DspSchedMode) -> Self {
        DspSchedConfig {
            mode,
            time_constraint: DspTimeConstraint::DEFAULT,
            interval_deadline: Self::DEFAULT_INTERVAL_DEADLINE,
        }
    }

    /// Set [`mode`](Self::mode).
    pub fn with_mode(mut self, mode: DspSchedMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set [`time_constraint`](Self::time_constraint).
    pub fn with_time_constraint(mut self, tc: DspTimeConstraint) -> Self {
        self.time_constraint = tc;
        self
    }

    /// Set [`interval_deadline`](Self::interval_deadline).
    pub fn with_interval_deadline(mut self, deadline: Duration) -> Self {
        self.interval_deadline = deadline;
        self
    }

    /// The defaults, with whatever of these the environment sets:
    /// [`ENV_MODE`](Self::ENV_MODE) (a [`DspSchedMode`] name),
    /// [`ENV_INTERVAL_DEADLINE_US`](Self::ENV_INTERVAL_DEADLINE_US),
    /// [`ENV_PERIOD_US`](Self::ENV_PERIOD_US),
    /// [`ENV_COMPUTATION_US`](Self::ENV_COMPUTATION_US) and
    /// [`ENV_CONSTRAINT_US`](Self::ENV_CONSTRAINT_US) (whole microseconds).
    /// An empty variable counts as unset. The engine never reads the
    /// environment itself: pass the result to
    /// [`crate::EngineConfig::with_dsp_sched`].
    ///
    /// A value that doesn't parse, or figures that don't pass the checks in
    /// [`DspTimeConstraint::new`] and on
    /// [`interval_deadline`](Self::interval_deadline), are
    /// [`ErrorCode::InvalidConfig`], naming the variable.
    pub fn from_env() -> Result<Self, AudioError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Like [`DspSchedConfig::from_env`], but each key
    /// ([`ENV_MODE`](Self::ENV_MODE) and the others) is looked up with
    /// `lookup`: for an app's own settings store (a debug menu, a remote
    /// flag), so a test build can switch modes without a rebuild.
    pub fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Result<Self, AudioError> {
        let mut get = |key: &str| lookup(key).filter(|v| !v.trim().is_empty());
        let us = |key: &str, v: String| -> Result<Duration, AudioError> {
            v.trim()
                .parse::<u64>()
                .map(Duration::from_micros)
                .map_err(|_| {
                    AudioError::new(
                        ErrorCode::InvalidConfig,
                        format!("{key}={v:?}: expected whole microseconds"),
                    )
                })
        };
        let mut cfg = DspSchedConfig::default();
        if let Some(v) = get(Self::ENV_MODE) {
            cfg.mode = v.parse().map_err(|e: AudioError| {
                AudioError::new(e.code, format!("{}: {}", Self::ENV_MODE, e.detail))
            })?;
        }
        if let Some(v) = get(Self::ENV_INTERVAL_DEADLINE_US) {
            cfg.interval_deadline = us(Self::ENV_INTERVAL_DEADLINE_US, v)?;
        }
        if let Some(v) = get(Self::ENV_PERIOD_US) {
            cfg.time_constraint.period = us(Self::ENV_PERIOD_US, v)?;
        }
        if let Some(v) = get(Self::ENV_COMPUTATION_US) {
            cfg.time_constraint.computation = us(Self::ENV_COMPUTATION_US, v)?;
        }
        if let Some(v) = get(Self::ENV_CONSTRAINT_US) {
            cfg.time_constraint.constraint = us(Self::ENV_CONSTRAINT_US, v)?;
        }
        cfg.validate()?;
        Ok(cfg)
    }

    pub(crate) fn validate(&self) -> Result<(), AudioError> {
        if self.mode == DspSchedMode::Unsupported {
            return Err(AudioError::new(
                ErrorCode::InvalidConfig,
                "dsp_sched.mode Unsupported is only reported, not asked for",
            ));
        }
        self.time_constraint.validate()?;
        if self.interval_deadline.is_zero() || self.interval_deadline > self.time_constraint.period
        {
            return Err(AudioError::new(
                ErrorCode::InvalidConfig,
                format!(
                    "dsp_sched.interval_deadline must be above 0 and at most the period {:?} (got {:?})",
                    self.time_constraint.period, self.interval_deadline
                ),
            ));
        }
        Ok(())
    }
}

/// Why the processing thread fell back to [`DspSchedMode::Workgroup`]
/// ([`DspSchedStats::fallback_reason`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DspSchedFallback {
    /// The OS refused the time-constraint policy
    /// ([`DspSchedMode::TimeConstraintOnly`] or
    /// [`DspSchedMode::OwnWorkgroup`]). A workgroup is for real-time threads,
    /// so the fallback joins none either: the thread is on the QoS class
    /// ([`DspScheduling::Qos`]) or nothing.
    TimeConstraintRefused,
    /// In [`DspSchedMode::OwnWorkgroup`], the system has no audio work
    /// intervals (`AudioWorkIntervalCreate` or the interval calls are
    /// missing: before macOS 11 or iOS 14), or creating one failed.
    OwnWorkgroupUnavailable,
    /// In [`DspSchedMode::OwnWorkgroup`], `os_workgroup_join` refused the
    /// thread's own work interval.
    OwnWorkgroupJoinRefused,
    /// In [`DspSchedMode::OwnWorkgroup`], `os_workgroup_interval_start`,
    /// `_update` or `_finish` refused an interval during the call. The
    /// thread left its own work interval.
    IntervalRefused,
}

impl DspSchedFallback {
    /// A short name for logs: `"time_constraint_refused"`,
    /// `"own_workgroup_unavailable"`, `"own_workgroup_join_refused"` or
    /// `"interval_refused"`.
    pub const fn as_str(self) -> &'static str {
        match self {
            DspSchedFallback::TimeConstraintRefused => "time_constraint_refused",
            DspSchedFallback::OwnWorkgroupUnavailable => "own_workgroup_unavailable",
            DspSchedFallback::OwnWorkgroupJoinRefused => "own_workgroup_join_refused",
            DspSchedFallback::IntervalRefused => "interval_refused",
        }
    }
}

/// The processing thread's scheduling set-up, for call logs
/// ([`crate::Stats::dsp_sched`]): the mode asked for and the mode in effect,
/// why they differ, and the figures asked for. What the OS granted is
/// [`crate::Stats::dsp_scheduling`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct DspSchedStats {
    /// [`DspSchedConfig::mode`], as configured.
    pub mode_requested: DspSchedMode,
    /// The mode the thread runs in: the one asked for,
    /// [`DspSchedMode::Workgroup`] after a fallback (see
    /// [`fallback_reason`](Self::fallback_reason)), or
    /// [`DspSchedMode::Unsupported`] off macOS and iOS.
    pub mode_effective: DspSchedMode,
    /// Why [`mode_effective`](Self::mode_effective) differs from
    /// [`mode_requested`](Self::mode_requested); `None` when it doesn't.
    pub fallback_reason: Option<DspSchedFallback>,
    /// The time-constraint figures asked for (period, computation and
    /// constraint), in the modes that ask for the policy; `None` in
    /// [`DspSchedMode::None`] and off macOS and iOS. Whether the OS granted
    /// them is [`crate::Stats::dsp_scheduling`].
    pub time_constraint: Option<DspTimeConstraint>,
    /// [`DspSchedMode::OwnWorkgroup`] asked for: each interval's deadline
    /// after its start ([`DspSchedConfig::interval_deadline`]); `None` in
    /// the other modes and off macOS and iOS.
    pub interval_deadline: Option<Duration>,
    /// Work intervals finished on the thread's own workgroup (one per pass
    /// with capture audio), since the engine started.
    pub intervals: u64,
}

/// The default figures (the tests read them; the engine's come from the
/// config).
#[cfg(test)]
pub(crate) const RT_PARAMS: DspTimeConstraint = DspTimeConstraint::DEFAULT;

/// Whether the device callbacks wake the processing thread (see
/// [`crate::device::IoCounters::wake_dsp`]) instead of it only polling. On
/// macOS and iOS; elsewhere it polls as before.
pub(crate) const WAKE_FROM_IO: bool = cfg!(any(target_os = "macos", target_os = "ios"));

/// The OS calls that change the calling thread's scheduling.
pub(crate) trait SchedOs {
    /// An OS workgroup to join.
    type Workgroup;
    /// What a join hands back for the leave.
    type Token;
    /// Whether this platform has these calls at all. Without them the mode
    /// in effect is [`DspSchedMode::Unsupported`] and nothing is called.
    const SUPPORTED: bool = true;
    /// Set the QoS class to user-interactive. True if the OS took it.
    fn set_qos(&mut self) -> bool;
    /// Set the time-constraint policy. True if the OS took it.
    fn set_time_constraint(&mut self, params: DspTimeConstraint) -> bool;
    /// Join `wg`. `None` if the OS refused.
    fn join(&mut self, wg: &Self::Workgroup) -> Option<Self::Token>;
    /// Leave `wg`, joined with `token`.
    fn leave(&mut self, wg: &Self::Workgroup, token: Self::Token);
    /// Make an audio work interval for this thread. `None` if the system
    /// has none (or the interval calls), or it failed.
    fn create_interval(&mut self) -> Option<Box<Self::Workgroup>>;
    /// Start an interval of `wg` (joined) now, due `deadline` from now.
    /// True if the OS took it.
    fn interval_start(&mut self, wg: &Self::Workgroup, deadline: Duration) -> bool;
    /// Move the started interval's deadline to `deadline` from now.
    fn interval_update(&mut self, wg: &Self::Workgroup, deadline: Duration) -> bool;
    /// Finish the started interval.
    fn interval_finish(&mut self, wg: &Self::Workgroup) -> bool;
}

/// Which workgroup the thread has joined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Joined {
    /// The audio unit's I/O workgroup.
    Io,
    /// The thread's own work interval.
    Own,
}

/// The processing thread's scheduling state. It lives on that thread (it
/// is `!Send`): a workgroup is left by the thread that joined it, and it is
/// left on drop.
pub(crate) struct DspSched<O: SchedOs> {
    os: O,
    cfg: DspSchedConfig,
    /// What the thread got before any workgroup.
    base: DspScheduling,
    effective: DspSchedMode,
    fallback: Option<DspSchedFallback>,
    /// The audio unit's I/O workgroup while the unit runs, joined or not
    /// (a fallback may join it later).
    io: Option<Box<O::Workgroup>>,
    /// The thread's own work interval ([`DspSchedMode::OwnWorkgroup`]),
    /// kept until the thread exits so nothing is released on it mid-call.
    own: Option<Box<O::Workgroup>>,
    joined: Option<(Joined, O::Token)>,
    /// An interval of `own` is started and not finished.
    in_interval: bool,
    intervals: u64,
    /// Joins and leaves are per thread.
    _not_send: PhantomData<*const ()>,
}

impl<O: SchedOs> DspSched<O> {
    /// Set up the calling thread as `cfg` asks. The QoS class goes before
    /// the time-constraint policy: Darwin refuses a QoS change once a thread
    /// has a policy of its own, and the policy replaces the class; the class
    /// is what is left if the policy is refused.
    pub(crate) fn promote(os: O, cfg: DspSchedConfig) -> Self {
        let mut s = DspSched {
            os,
            cfg,
            base: DspScheduling::None,
            effective: cfg.mode,
            fallback: None,
            io: None,
            own: None,
            joined: None,
            in_interval: false,
            intervals: 0,
            _not_send: PhantomData,
        };
        if !O::SUPPORTED {
            s.effective = DspSchedMode::Unsupported;
            return s;
        }
        if !cfg.mode.wants_time_constraint() {
            // `None` (and `Unsupported`, which `validate` keeps out).
            s.effective = DspSchedMode::None;
            return s;
        }
        let qos = s.os.set_qos();
        let time_constraint = s.os.set_time_constraint(cfg.time_constraint);
        s.base = if time_constraint {
            DspScheduling::TimeConstraint
        } else if qos {
            DspScheduling::Qos
        } else {
            DspScheduling::None
        };
        if cfg.mode != DspSchedMode::Workgroup && !time_constraint {
            s.fall_back(DspSchedFallback::TimeConstraintRefused);
            return s;
        }
        if cfg.mode == DspSchedMode::OwnWorkgroup {
            match s.os.create_interval() {
                None => s.fall_back(DspSchedFallback::OwnWorkgroupUnavailable),
                Some(wg) => {
                    let joined = s.os.join(&wg);
                    s.own = Some(wg);
                    match joined {
                        Some(token) => s.joined = Some((Joined::Own, token)),
                        None => s.fall_back(DspSchedFallback::OwnWorkgroupJoinRefused),
                    }
                }
            }
        }
        s
    }

    /// The scheduling the thread has now.
    pub(crate) fn mode(&self) -> DspScheduling {
        match self.joined {
            Some((Joined::Io, _)) => DspScheduling::Workgroup,
            Some((Joined::Own, _)) => DspScheduling::OwnWorkgroup,
            None => self.base,
        }
    }

    /// The set-up, for [`crate::Stats::dsp_sched`].
    pub(crate) fn stats(&self) -> DspSchedStats {
        let supported = O::SUPPORTED;
        DspSchedStats {
            mode_requested: self.cfg.mode,
            mode_effective: self.effective,
            fallback_reason: self.fallback,
            time_constraint: (supported && self.cfg.mode.wants_time_constraint())
                .then_some(self.cfg.time_constraint),
            interval_deadline: (supported && self.cfg.mode == DspSchedMode::OwnWorkgroup)
                .then_some(self.cfg.interval_deadline),
            intervals: self.intervals,
        }
    }

    /// The thread's own work interval, if it made one.
    #[cfg(all(test, any(target_os = "macos", target_os = "ios")))]
    pub(crate) fn own(&self) -> Option<&O::Workgroup> {
        self.own.as_deref()
    }

    /// What, when it changes, is published at once (not the interval
    /// count, which changes every block).
    pub(crate) fn state(&self) -> (DspScheduling, DspSchedMode, Option<DspSchedFallback>) {
        (self.mode(), self.effective, self.fallback)
    }

    /// Give up the requested mode for [`DspSchedMode::Workgroup`]: finish
    /// and leave the own work interval (kept until drop), and join the
    /// audio unit's workgroup if there is one.
    fn fall_back(&mut self, why: DspSchedFallback) {
        if self.in_interval {
            self.in_interval = false;
            if let Some(own) = self.own.as_deref() {
                let _ = self.os.interval_finish(own);
            }
        }
        // Only the own work interval is ever joined here (a fallback runs at
        // set-up or from an interval call); the unit's workgroup stays.
        if matches!(self.joined, Some((Joined::Own, _))) {
            if let (Some((_, token)), Some(own)) = (self.joined.take(), self.own.as_deref()) {
                self.os.leave(own, token);
            }
        }
        self.effective = DspSchedMode::Workgroup;
        self.fallback = Some(why);
        self.join_io();
    }

    /// Join the audio unit's workgroup, if the mode in effect is
    /// [`DspSchedMode::Workgroup`], the thread got the time-constraint
    /// policy (a workgroup is for real-time threads), and there is one.
    fn join_io(&mut self) {
        if self.effective != DspSchedMode::Workgroup
            || self.base != DspScheduling::TimeConstraint
            || self.joined.is_some()
        {
            return;
        }
        let Some(io) = self.io.as_deref() else {
            return;
        };
        if let Some(token) = self.os.join(io) {
            self.joined = Some((Joined::Io, token));
        }
    }

    /// The audio I/O thread's workgroup changed: leave the one joined, if
    /// any, and take `wg`, if any, joining it in
    /// [`DspSchedMode::Workgroup`]. Workgroups let go go to `discard`, to be
    /// released off this thread.
    pub(crate) fn set_workgroup(
        &mut self,
        wg: Option<Box<O::Workgroup>>,
        mut discard: impl FnMut(Box<O::Workgroup>),
    ) {
        if matches!(self.joined, Some((Joined::Io, _))) {
            if let (Some((_, token)), Some(io)) = (self.joined.take(), self.io.as_deref()) {
                self.os.leave(io, token);
            }
        }
        if let Some(old) = self.io.take() {
            discard(old);
        }
        self.io = wg;
        self.join_io();
    }

    /// A capture block is about to be processed: in
    /// [`DspSchedMode::OwnWorkgroup`], start an interval due
    /// [`DspSchedConfig::interval_deadline`] from now, or, for a further
    /// block in the same pass, move its deadline there. A refusal falls
    /// back to [`DspSchedMode::Workgroup`].
    pub(crate) fn block_start(&mut self) {
        if !matches!(self.joined, Some((Joined::Own, _))) {
            return;
        }
        let Some(own) = self.own.as_deref() else {
            return;
        };
        let deadline = self.cfg.interval_deadline;
        let ok = if self.in_interval {
            self.os.interval_update(own, deadline)
        } else {
            self.os.interval_start(own, deadline)
        };
        if ok {
            self.in_interval = true;
        } else {
            self.fall_back(DspSchedFallback::IntervalRefused);
        }
    }

    /// The pass's capture blocks are done: finish the interval, if one is
    /// started. A refusal falls back to [`DspSchedMode::Workgroup`].
    pub(crate) fn pass_end(&mut self) {
        if !self.in_interval {
            return;
        }
        self.in_interval = false;
        let finished = self
            .own
            .as_deref()
            .is_some_and(|own| self.os.interval_finish(own));
        if finished {
            self.intervals += 1;
        } else {
            self.fall_back(DspSchedFallback::IntervalRefused);
        }
    }
}

impl<O: SchedOs> Drop for DspSched<O> {
    fn drop(&mut self) {
        if self.in_interval {
            self.in_interval = false;
            if let Some(own) = self.own.as_deref() {
                let _ = self.os.interval_finish(own);
            }
        }
        // A thread must leave its workgroups before it exits.
        if let Some((joined, token)) = self.joined.take() {
            let wg = match joined {
                Joined::Io => self.io.as_deref(),
                Joined::Own => self.own.as_deref(),
            };
            if let Some(wg) = wg {
                self.os.leave(wg, token);
            }
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) mod apple;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) use apple::{AppleOs as PlatformOs, Workgroup};

/// No scheduling requests on this platform.
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub(crate) struct PlatformOs;

/// No OS workgroups on this platform: never constructed.
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub(crate) enum Workgroup {}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
impl PlatformOs {
    pub(crate) fn new() -> Self {
        PlatformOs
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
impl SchedOs for PlatformOs {
    type Workgroup = Workgroup;
    type Token = ();
    const SUPPORTED: bool = false;
    fn set_qos(&mut self) -> bool {
        false
    }
    fn set_time_constraint(&mut self, _: DspTimeConstraint) -> bool {
        false
    }
    fn join(&mut self, wg: &Workgroup) -> Option<()> {
        match *wg {}
    }
    fn leave(&mut self, wg: &Workgroup, _: ()) {
        match *wg {}
    }
    fn create_interval(&mut self) -> Option<Box<Workgroup>> {
        None
    }
    fn interval_start(&mut self, wg: &Workgroup, _: Duration) -> bool {
        match *wg {}
    }
    fn interval_update(&mut self, wg: &Workgroup, _: Duration) -> bool {
        match *wg {}
    }
    fn interval_finish(&mut self, wg: &Workgroup) -> bool {
        match *wg {}
    }
}

/// Set up the calling thread (the processing thread, as it starts) with
/// this platform's calls.
pub(crate) fn promote_current(cfg: DspSchedConfig) -> DspSched<PlatformOs> {
    DspSched::promote(PlatformOs::new(), cfg)
}

#[cfg(test)]
mod tests;
