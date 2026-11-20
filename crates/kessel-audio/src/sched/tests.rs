//! The processing thread's scheduling: the fallback order over a fake OS,
//! what the engine reports, and on macOS and iOS what the OS actually gave
//! the thread.

use super::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    Qos,
    TimeConstraint(RtParams),
    Join(u32),
    /// The workgroup and the token its join handed back.
    Leave(u32, u64),
}

/// What the fake OS grants.
#[derive(Clone, Copy)]
struct Grants {
    qos: bool,
    time_constraint: bool,
    join: bool,
}

const ALL: Grants = Grants {
    qos: true,
    time_constraint: true,
    join: true,
};

/// A fake OS: workgroups are numbers, a join's token is the workgroup
/// times 100 plus the join's count, so a leave shows which join it ends.
struct FakeOs {
    grants: Grants,
    joins: u64,
    calls: Rc<RefCell<Vec<Call>>>,
}

impl SchedOs for FakeOs {
    type Workgroup = u32;
    type Token = u64;
    fn set_qos(&mut self) -> bool {
        self.calls.borrow_mut().push(Call::Qos);
        self.grants.qos
    }
    fn set_time_constraint(&mut self, params: RtParams) -> bool {
        self.calls.borrow_mut().push(Call::TimeConstraint(params));
        self.grants.time_constraint
    }
    fn join(&mut self, wg: &u32) -> Option<u64> {
        self.calls.borrow_mut().push(Call::Join(*wg));
        self.joins += 1;
        self.grants
            .join
            .then_some(u64::from(*wg) * 100 + self.joins)
    }
    fn leave(&mut self, wg: &u32, token: u64) {
        self.calls.borrow_mut().push(Call::Leave(*wg, token));
    }
}

fn promote(grants: Grants) -> (DspSched<FakeOs>, Rc<RefCell<Vec<Call>>>) {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let os = FakeOs {
        grants,
        joins: 0,
        calls: calls.clone(),
    };
    (DspSched::promote(os, RT_PARAMS), calls)
}

fn calls(c: &Rc<RefCell<Vec<Call>>>) -> Vec<Call> {
    c.borrow().clone()
}

#[test]
fn qos_first_then_time_constraint() {
    let (s, c) = promote(ALL);
    assert_eq!(s.mode(), DspScheduling::TimeConstraint);
    assert_eq!(calls(&c), [Call::Qos, Call::TimeConstraint(RT_PARAMS)]);
}

#[test]
fn a_refused_time_constraint_falls_back_to_qos() {
    let (s, _) = promote(Grants {
        time_constraint: false,
        ..ALL
    });
    assert_eq!(s.mode(), DspScheduling::Qos);
}

#[test]
fn a_refused_qos_still_asks_for_time_constraint() {
    let (s, c) = promote(Grants { qos: false, ..ALL });
    assert_eq!(s.mode(), DspScheduling::TimeConstraint);
    assert_eq!(calls(&c), [Call::Qos, Call::TimeConstraint(RT_PARAMS)]);
}

#[test]
fn everything_refused_is_none() {
    let (s, _) = promote(Grants {
        qos: false,
        time_constraint: false,
        join: false,
    });
    assert_eq!(s.mode(), DspScheduling::None);
}

#[test]
fn joins_the_workgroup_and_leaves_it_when_the_unit_closes() {
    let (mut s, c) = promote(ALL);
    let mut discarded = Vec::new();
    s.set_workgroup(Some(Box::new(7)), |w| discarded.push(*w));
    assert_eq!(s.mode(), DspScheduling::Workgroup);
    assert!(discarded.is_empty(), "a joined workgroup is kept");
    s.set_workgroup(None, |w| discarded.push(*w));
    assert_eq!(s.mode(), DspScheduling::TimeConstraint);
    assert_eq!(discarded, [7], "the left workgroup goes to be released");
    assert_eq!(calls(&c)[2..], [Call::Join(7), Call::Leave(7, 701)]);
}

#[test]
fn a_refused_join_stays_on_time_constraint() {
    let (mut s, c) = promote(Grants { join: false, ..ALL });
    let mut discarded = Vec::new();
    s.set_workgroup(Some(Box::new(7)), |w| discarded.push(*w));
    assert_eq!(s.mode(), DspScheduling::TimeConstraint);
    assert_eq!(discarded, [7], "the refused workgroup goes to be released");
    drop(s);
    assert_eq!(calls(&c)[2..], [Call::Join(7)], "nothing to leave");
}

#[test]
fn a_new_unit_swaps_workgroups_leaving_first() {
    let (mut s, c) = promote(ALL);
    let mut discarded = Vec::new();
    s.set_workgroup(Some(Box::new(7)), |w| discarded.push(*w));
    s.set_workgroup(Some(Box::new(8)), |w| discarded.push(*w));
    assert_eq!(s.mode(), DspScheduling::Workgroup);
    assert_eq!(discarded, [7]);
    assert_eq!(
        calls(&c)[2..],
        [Call::Join(7), Call::Leave(7, 701), Call::Join(8)]
    );
}

#[test]
fn only_a_time_constraint_thread_joins() {
    let (mut s, c) = promote(Grants {
        time_constraint: false,
        ..ALL
    });
    let mut discarded = Vec::new();
    s.set_workgroup(Some(Box::new(7)), |w| discarded.push(*w));
    assert_eq!(s.mode(), DspScheduling::Qos);
    assert_eq!(discarded, [7]);
    assert!(
        !calls(&c).contains(&Call::Join(7)),
        "a thread without the real-time policy doesn't join"
    );
}

#[test]
fn the_thread_leaves_its_workgroup_on_drop() {
    let (mut s, c) = promote(ALL);
    s.set_workgroup(Some(Box::new(7)), |_| panic!("kept"));
    drop(s);
    assert_eq!(calls(&c).last(), Some(&Call::Leave(7, 701)));
}

#[test]
fn the_parameters_are_the_block_and_the_stage_budgets() {
    assert_eq!(RT_PARAMS.period, Duration::from_millis(10));
    assert_eq!(RT_PARAMS.computation, Duration::from_millis(6));
    assert_eq!(RT_PARAMS.constraint, Duration::from_millis(9));
}

#[test]
fn modes_have_log_names_and_default_to_none() {
    assert_eq!(DspScheduling::default(), DspScheduling::None);
    let names: Vec<&str> = [
        DspScheduling::Workgroup,
        DspScheduling::TimeConstraint,
        DspScheduling::Qos,
        DspScheduling::None,
    ]
    .iter()
    .map(|m| m.as_str())
    .collect();
    assert_eq!(names, ["workgroup", "time_constraint", "qos", "none"]);
}

/// What the processing thread gets on the platform the tests run on.
const PLATFORM_MODE: DspScheduling = if cfg!(any(target_os = "macos", target_os = "ios")) {
    DspScheduling::TimeConstraint
} else {
    DspScheduling::None
};

/// A pass-through stage, so the stats carry a `capture_stage`.
struct Passthrough;

impl crate::CaptureStage for Passthrough {
    fn process(&mut self, _: &mut [f32; 480]) {}
    fn latency_samples(&self) -> usize {
        0
    }
    fn reset(&mut self) {}
}

fn staged_engine() -> crate::AudioEngine {
    let cfg = crate::EngineConfig::null(crate::NullDevice::default())
        .with_processing(crate::ProcessingConfig::off())
        .with_capture_stage(Box::new(Passthrough));
    crate::AudioEngine::start(cfg).expect("start")
}

/// The first published statistics (they carry the stage's once published).
fn published_stats(engine: &crate::AudioEngine) -> crate::Stats {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let s = engine.stats();
        if s.capture_stage.is_some() {
            return s;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no statistics published within 5 s"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_engine_reports_the_processing_threads_scheduling() {
    let engine = staged_engine();
    let s = published_stats(&engine);
    assert_eq!(s.dsp_scheduling, PLATFORM_MODE);
    let stage = s.capture_stage.expect("published");
    assert_eq!(stage.dsp_scheduling, PLATFORM_MODE);
    engine.shutdown();
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple {
    #![allow(unsafe_code)]

    use super::super::apple::{time_constraint_policy, AppleOs, QOS_CLASS_USER_INTERACTIVE};
    use super::*;
    use std::ffi::{c_char, c_int, c_void};

    extern "C" {
        fn pthread_get_qos_class_np(
            thread: libc::pthread_t,
            qos_class: *mut u32,
            relative_priority: *mut c_int,
        ) -> c_int;
        fn os_workgroup_cancel(wg: *mut c_void);
    }

    #[link(name = "AudioToolbox", kind = "framework")]
    extern "C" {
        fn AudioWorkIntervalCreate(
            name: *const c_char,
            clock: u32,
            attr: *mut c_void,
        ) -> *mut c_void;
    }

    /// `QOS_CLASS_UNSPECIFIED` (`<sys/qos.h>`).
    const QOS_CLASS_UNSPECIFIED: u32 = 0;

    /// `OS_CLOCK_MACH_ABSOLUTE_TIME` (`<os/clock.h>`).
    const OS_CLOCK_MACH_ABSOLUTE_TIME: u32 = 32;

    /// A thread's QoS class, read back from the OS.
    fn qos_of(thread: libc::pthread_t) -> u32 {
        let (mut qos, mut rel) = (0u32, 0 as c_int);
        // SAFETY: `thread` is a live thread of this process (the caller's
        // own, or the engine's processing thread while the engine runs);
        // both out-pointers are valid for one write.
        let r = unsafe { pthread_get_qos_class_np(thread, &mut qos, &mut rel) };
        assert_eq!(r, 0, "pthread_get_qos_class_np failed");
        qos
    }

    /// A thread's time-constraint policy, read back from the OS, or `None`
    /// if it has the default policy.
    fn time_constraint_of(thread: libc::pthread_t) -> Option<(u32, u32, u32)> {
        let mut p = libc::thread_time_constraint_policy {
            period: 0,
            computation: 0,
            constraint: 0,
            preemptible: 0,
        };
        let mut count = libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT;
        let mut get_default: libc::boolean_t = 0;
        // SAFETY: the port is the live thread's (no new reference); `p` has
        // room for `count` integers; both out-pointers are valid.
        let r = unsafe {
            libc::thread_policy_get(
                libc::pthread_mach_thread_np(thread),
                libc::THREAD_TIME_CONSTRAINT_POLICY as libc::thread_policy_flavor_t,
                (&mut p as *mut libc::thread_time_constraint_policy).cast(),
                &mut count,
                &mut get_default,
            )
        };
        assert_eq!(r, libc::KERN_SUCCESS, "thread_policy_get failed");
        (get_default == 0).then_some((p.period, p.computation, p.constraint))
    }

    fn expected_policy() -> (u32, u32, u32) {
        let p = time_constraint_policy(RT_PARAMS).expect("timebase");
        (p.period, p.computation, p.constraint)
    }

    fn on_a_new_thread<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
        std::thread::spawn(f).join().expect("test thread panicked")
    }

    #[test]
    fn a_new_thread_has_neither() {
        // So the next tests show the promotion, not the harness.
        on_a_new_thread(|| {
            // SAFETY: the calling thread.
            let me = unsafe { libc::pthread_self() };
            assert_ne!(qos_of(me), QOS_CLASS_USER_INTERACTIVE);
            assert_eq!(time_constraint_of(me), None);
        });
    }

    #[test]
    fn promote_sets_the_time_constraint_policy() {
        on_a_new_thread(|| {
            let s = promote_current();
            assert_eq!(s.mode(), DspScheduling::TimeConstraint);
            // SAFETY: the calling thread.
            let me = unsafe { libc::pthread_self() };
            assert_eq!(time_constraint_of(me), Some(expected_policy()));
            // Darwin reports no QoS class for a thread with a Mach policy of
            // its own: the time-constraint policy replaces the QoS class set
            // before it (which is there for when the policy is refused).
            assert_eq!(qos_of(me), QOS_CLASS_UNSPECIFIED);
        });
    }

    /// The real OS, with the time-constraint policy refused.
    struct RefuseTimeConstraint(AppleOs);

    impl SchedOs for RefuseTimeConstraint {
        type Workgroup = Workgroup;
        type Token = ();
        fn set_qos(&mut self) -> bool {
            self.0.set_qos()
        }
        fn set_time_constraint(&mut self, _: RtParams) -> bool {
            false
        }
        fn join(&mut self, wg: &Workgroup) -> Option<()> {
            self.0.join(wg)
        }
        fn leave(&mut self, wg: &Workgroup, token: ()) {
            self.0.leave(wg, token)
        }
    }

    #[test]
    fn a_refused_time_constraint_leaves_user_interactive_qos() {
        on_a_new_thread(|| {
            let mut s = DspSched::promote(RefuseTimeConstraint(AppleOs::new()), RT_PARAMS);
            assert_eq!(s.mode(), DspScheduling::Qos);
            // SAFETY: the calling thread.
            let me = unsafe { libc::pthread_self() };
            assert_eq!(qos_of(me), QOS_CLASS_USER_INTERACTIVE);
            assert_eq!(time_constraint_of(me), None);
            // And a thread that isn't real-time doesn't join a workgroup.
            let mut discarded = 0;
            s.set_workgroup(Some(work_interval()), |_| discarded += 1);
            assert_eq!(s.mode(), DspScheduling::Qos);
            assert_eq!(discarded, 1);
        });
    }

    #[test]
    fn the_engines_processing_thread_is_promoted() {
        let engine = staged_engine();
        assert_eq!(
            published_stats(&engine).dsp_scheduling,
            DspScheduling::TimeConstraint
        );
        // Read back from the OS: the real-time policy, on the engine's own
        // processing thread.
        let dsp = engine.dsp_pthread();
        assert_eq!(time_constraint_of(dsp), Some(expected_policy()));
        // The policy replaced the QoS class (see the test above).
        assert_eq!(qos_of(dsp), QOS_CLASS_UNSPECIFIED);
        engine.shutdown();
    }

    /// A real audio workgroup (an audio work interval), +1.
    fn work_interval() -> Box<Workgroup> {
        // SAFETY: a NUL-terminated name, a valid clock id and no attributes;
        // the result is a +1 workgroup (OS_WORKGROUP_RETURNS_RETAINED), which
        // `from_retained` takes over.
        let wg = unsafe {
            Workgroup::from_retained(AudioWorkIntervalCreate(
                c"kessel-audio-test".as_ptr(),
                OS_CLOCK_MACH_ABSOLUTE_TIME,
                std::ptr::null_mut(),
            ))
        };
        Box::new(wg.expect("AudioWorkIntervalCreate returned null"))
    }

    #[test]
    fn joins_and_leaves_a_real_workgroup() {
        on_a_new_thread(|| {
            let mut s = DspSched::promote(AppleOs::new(), RT_PARAMS);
            let mut discarded = 0;
            s.set_workgroup(Some(work_interval()), |_| discarded += 1);
            assert_eq!(s.mode(), DspScheduling::Workgroup);
            assert_eq!(discarded, 0);
            s.set_workgroup(None, |_| discarded += 1);
            assert_eq!(s.mode(), DspScheduling::TimeConstraint);
            assert_eq!(discarded, 1);
            // Joined again, then left on drop (leaving is required before
            // the thread exits).
            s.set_workgroup(Some(work_interval()), |_| discarded += 1);
            assert_eq!(s.mode(), DspScheduling::Workgroup);
        });
    }

    #[test]
    fn a_cancelled_workgroup_is_refused_and_the_thread_stays_real_time() {
        on_a_new_thread(|| {
            let mut s = DspSched::promote(AppleOs::new(), RT_PARAMS);
            let wg = work_interval();
            // SAFETY: a live workgroup this holds a reference to.
            unsafe { os_workgroup_cancel(wg.as_ptr()) };
            let mut discarded = 0;
            s.set_workgroup(Some(wg), |_| discarded += 1);
            assert_eq!(s.mode(), DspScheduling::TimeConstraint);
            assert_eq!(discarded, 1);
            // SAFETY: the calling thread.
            let me = unsafe { libc::pthread_self() };
            assert_eq!(time_constraint_of(me), Some(expected_policy()));
        });
    }
}
