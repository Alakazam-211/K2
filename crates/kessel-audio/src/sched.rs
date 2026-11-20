//! How the OS schedules the processing thread (`kessel-audio-dsp`).
//!
//! On macOS and iOS the thread asks for real-time scheduling when it starts,
//! in this order, and keeps the best it got:
//!
//! 1. the QoS class `QOS_CLASS_USER_INTERACTIVE`
//!    (`pthread_set_qos_class_self_np`), the least it should run at;
//! 2. on top of that, Mach's time-constraint policy
//!    (`THREAD_TIME_CONSTRAINT_POLICY`): the real-time band, with
//!    [`RT_PARAMS`] (a 10 ms period, the block; 6 ms of computation, the
//!    stage's percentile budget; a 9 ms constraint, its hard budget);
//! 3. once the phone backend's audio unit runs (iOS 14 or later), the unit's
//!    I/O thread's OS workgroup (`kAudioOutputUnitProperty_OSWorkgroup`,
//!    `os_workgroup_join`), so the OS schedules the processing thread with
//!    the I/O thread it works in step with: the I/O callback wakes it, and its
//!    work is due within the same cycle. Only a time-constraint thread joins.
//!    The thread leaves when the unit closes, and before it exits.
//!
//! Each step that the OS refuses (an older OS without workgroups, a refused
//! policy) leaves the thread on the step before. [`DspScheduling`] reports
//! where it ended up, in [`crate::Stats::dsp_scheduling`] and
//! [`crate::CaptureStageStats::dsp_scheduling`].
//!
//! Elsewhere (Linux, Android, Windows) nothing changes in this release: the
//! thread keeps the OS's default scheduling and polls.
//!
//! The order of the steps is plain code over [`SchedOs`], so the fallbacks
//! are tested with a fake on any OS; the OS calls are in `apple`.

use std::marker::PhantomData;
use std::time::Duration;

/// How the OS schedules the processing thread (`kessel-audio-dsp`): the best
/// of the requests it made that the OS granted. In
/// [`crate::Stats::dsp_scheduling`] and
/// [`crate::CaptureStageStats::dsp_scheduling`], so call logs show it.
///
/// On macOS and iOS the thread asks for each in turn, from
/// [`DspScheduling::Qos`] up; each one the OS refuses leaves it on the one
/// before. On other platforms it asks for none of them
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
    /// (`THREAD_TIME_CONSTRAINT_POLICY`), on top of
    /// [`DspScheduling::Qos`]: a 10 ms period, 6 ms of computation and a
    /// 9 ms constraint (the block, and the capture stage's two budgets).
    TimeConstraint,
    /// Only the QoS class `QOS_CLASS_USER_INTERACTIVE`: the OS refused the
    /// time-constraint policy.
    Qos,
    /// The OS's default scheduling: on Linux, Android and Windows (no change
    /// in this release), or where the OS refused everything.
    #[default]
    None,
}

impl DspScheduling {
    /// A short name for logs: `"workgroup"`, `"time_constraint"`, `"qos"`
    /// or `"none"`.
    pub const fn as_str(self) -> &'static str {
        match self {
            DspScheduling::Workgroup => "workgroup",
            DspScheduling::TimeConstraint => "time_constraint",
            DspScheduling::Qos => "qos",
            DspScheduling::None => "none",
        }
    }
}

/// The time-constraint policy's parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// Only macOS and iOS read them (and the tests).
#[cfg_attr(
    not(any(test, target_os = "macos", target_os = "ios")),
    allow(dead_code)
)]
pub(crate) struct RtParams {
    /// How often the thread has work: one 10 ms block.
    pub(crate) period: Duration,
    /// The CPU time it needs per period before the OS may preempt it for
    /// another real-time thread.
    pub(crate) computation: Duration,
    /// The time from the start of the period by which that work must be
    /// done.
    pub(crate) constraint: Duration,
}

/// The processing thread's parameters: the block period, the capture stage's
/// percentile budget as the computation (the share of a block the thread may
/// use at the 99th percentile) and its hard budget as the constraint (a block
/// that takes longer strikes the stage).
pub(crate) const RT_PARAMS: RtParams = RtParams {
    period: Duration::from_millis(10),
    computation: crate::stage::P99_BUDGET,
    constraint: crate::stage::HARD_BUDGET,
};

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
    /// Set the QoS class to user-interactive. True if the OS took it.
    fn set_qos(&mut self) -> bool;
    /// Set the time-constraint policy. True if the OS took it.
    fn set_time_constraint(&mut self, params: RtParams) -> bool;
    /// Join `wg`. `None` if the OS refused.
    fn join(&mut self, wg: &Self::Workgroup) -> Option<Self::Token>;
    /// Leave `wg`, joined with `token`.
    fn leave(&mut self, wg: &Self::Workgroup, token: Self::Token);
}

/// The processing thread's scheduling state. It lives on that thread (it
/// is `!Send`): a workgroup is left by the thread that joined it, and it is
/// left on drop.
pub(crate) struct DspSched<O: SchedOs> {
    os: O,
    /// What the thread got before any workgroup.
    base: DspScheduling,
    joined: Option<(Box<O::Workgroup>, O::Token)>,
    /// Joins and leaves are per thread.
    _not_send: PhantomData<*const ()>,
}

impl<O: SchedOs> DspSched<O> {
    /// Ask for the QoS class, then the time-constraint policy, for the
    /// calling thread. The QoS class goes first: Darwin refuses a QoS change
    /// once a thread has a policy of its own.
    pub(crate) fn promote(mut os: O, params: RtParams) -> Self {
        let qos = os.set_qos();
        let time_constraint = os.set_time_constraint(params);
        let base = if time_constraint {
            DspScheduling::TimeConstraint
        } else if qos {
            DspScheduling::Qos
        } else {
            DspScheduling::None
        };
        DspSched {
            os,
            base,
            joined: None,
            _not_send: PhantomData,
        }
    }

    /// The scheduling the thread has now.
    pub(crate) fn mode(&self) -> DspScheduling {
        if self.joined.is_some() {
            DspScheduling::Workgroup
        } else {
            self.base
        }
    }

    /// The audio I/O thread's workgroup changed: leave the one joined, if
    /// any, and join `wg`, if any. Only a time-constraint thread joins (a
    /// workgroup is for real-time threads). Workgroups left or not joined go
    /// to `discard`, to be released off this thread.
    pub(crate) fn set_workgroup(
        &mut self,
        wg: Option<Box<O::Workgroup>>,
        mut discard: impl FnMut(Box<O::Workgroup>),
    ) {
        if let Some((old, token)) = self.joined.take() {
            self.os.leave(&old, token);
            discard(old);
        }
        let Some(wg) = wg else {
            return;
        };
        if self.base != DspScheduling::TimeConstraint {
            discard(wg);
            return;
        }
        match self.os.join(&wg) {
            Some(token) => self.joined = Some((wg, token)),
            None => discard(wg),
        }
    }
}

impl<O: SchedOs> Drop for DspSched<O> {
    fn drop(&mut self) {
        // A thread must leave its workgroups before it exits.
        if let Some((wg, token)) = self.joined.take() {
            self.os.leave(&wg, token);
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) mod apple;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) use apple::{AppleOs as PlatformOs, Workgroup};

/// No scheduling requests on this platform (no change in this release).
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
    fn set_qos(&mut self) -> bool {
        false
    }
    fn set_time_constraint(&mut self, _: RtParams) -> bool {
        false
    }
    fn join(&mut self, wg: &Workgroup) -> Option<()> {
        match *wg {}
    }
    fn leave(&mut self, wg: &Workgroup, _: ()) {
        match *wg {}
    }
}

/// Promote the calling thread (the processing thread, as it starts) with
/// this platform's calls.
pub(crate) fn promote_current() -> DspSched<PlatformOs> {
    DspSched::promote(PlatformOs::new(), RT_PARAMS)
}

#[cfg(test)]
mod tests;
