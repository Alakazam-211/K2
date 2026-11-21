//! The processing thread's scheduling: each mode's calls and fallbacks over
//! a fake OS, the config knob, what the engine reports, and on macOS and iOS
//! what the OS actually gave the thread.

use super::*;
use std::cell::RefCell;
use std::rc::Rc;

/// The fake's own work interval.
const OWN: u32 = 99;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    Qos,
    TimeConstraint(DspTimeConstraint),
    Join(u32),
    /// The workgroup and the token its join handed back.
    Leave(u32, u64),
    Create,
    Start(u32, Duration),
    Update(u32, Duration),
    Finish(u32),
}

/// What the fake OS grants.
#[derive(Clone, Copy)]
struct Grants {
    qos: bool,
    time_constraint: bool,
    join: bool,
    /// Joins of the own work interval (`join` is for the others).
    join_own: bool,
    create: bool,
    start: bool,
    update: bool,
    finish: bool,
}

const ALL: Grants = Grants {
    qos: true,
    time_constraint: true,
    join: true,
    join_own: true,
    create: true,
    start: true,
    update: true,
    finish: true,
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
    fn set_time_constraint(&mut self, params: DspTimeConstraint) -> bool {
        self.calls.borrow_mut().push(Call::TimeConstraint(params));
        self.grants.time_constraint
    }
    fn join(&mut self, wg: &u32) -> Option<u64> {
        self.calls.borrow_mut().push(Call::Join(*wg));
        self.joins += 1;
        let granted = if *wg == OWN {
            self.grants.join_own
        } else {
            self.grants.join
        };
        granted.then_some(u64::from(*wg) * 100 + self.joins)
    }
    fn leave(&mut self, wg: &u32, token: u64) {
        self.calls.borrow_mut().push(Call::Leave(*wg, token));
    }
    fn create_interval(&mut self) -> Option<Box<u32>> {
        self.calls.borrow_mut().push(Call::Create);
        self.grants.create.then(|| Box::new(OWN))
    }
    fn interval_start(&mut self, wg: &u32, deadline: Duration) -> bool {
        self.calls.borrow_mut().push(Call::Start(*wg, deadline));
        self.grants.start
    }
    fn interval_update(&mut self, wg: &u32, deadline: Duration) -> bool {
        self.calls.borrow_mut().push(Call::Update(*wg, deadline));
        self.grants.update
    }
    fn interval_finish(&mut self, wg: &u32) -> bool {
        self.calls.borrow_mut().push(Call::Finish(*wg));
        self.grants.finish
    }
}

type Calls = Rc<RefCell<Vec<Call>>>;

fn promote_with(grants: Grants, cfg: DspSchedConfig) -> (DspSched<FakeOs>, Calls) {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let os = FakeOs {
        grants,
        joins: 0,
        calls: calls.clone(),
    };
    (DspSched::promote(os, cfg), calls)
}

/// The default mode (`Workgroup`).
fn promote(grants: Grants) -> (DspSched<FakeOs>, Calls) {
    promote_with(grants, DspSchedConfig::default())
}

fn promote_mode(grants: Grants, mode: DspSchedMode) -> (DspSched<FakeOs>, Calls) {
    promote_with(grants, DspSchedConfig::new(mode))
}

fn calls(c: &Calls) -> Vec<Call> {
    c.borrow().clone()
}

/// The calls since the first `n`.
fn calls_from(c: &Calls, n: usize) -> Vec<Call> {
    c.borrow()[n..].to_vec()
}

const DEADLINE: Duration = DspSchedConfig::DEFAULT_INTERVAL_DEADLINE;

// ---- Workgroup (the default): 0.2.4's behaviour ----

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
    assert_eq!(
        s.stats().fallback_reason,
        None,
        "Workgroup is the fallback: it degrades step by step, as in 0.2.4"
    );
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
        ..ALL
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
    assert_eq!(calls_from(&c, 2), [Call::Join(7), Call::Leave(7, 701)]);
}

#[test]
fn a_refused_join_stays_on_time_constraint() {
    let (mut s, c) = promote(Grants { join: false, ..ALL });
    let mut discarded = Vec::new();
    s.set_workgroup(Some(Box::new(7)), |w| discarded.push(*w));
    assert_eq!(s.mode(), DspScheduling::TimeConstraint);
    assert!(discarded.is_empty(), "held until the unit closes");
    s.set_workgroup(None, |w| discarded.push(*w));
    assert_eq!(discarded, [7], "the refused workgroup goes to be released");
    drop(s);
    assert_eq!(calls_from(&c, 2), [Call::Join(7)], "nothing to leave");
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
        calls_from(&c, 2),
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
    s.set_workgroup(None, |w| discarded.push(*w));
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
fn workgroup_mode_runs_no_intervals() {
    let (mut s, c) = promote(ALL);
    s.set_workgroup(Some(Box::new(7)), |_| panic!("kept"));
    let before = calls(&c).len();
    s.block_start();
    s.pass_end();
    assert_eq!(calls_from(&c, before), Vec::<Call>::new());
    assert_eq!(s.stats().intervals, 0);
}

#[test]
fn workgroup_stats_are_the_request_and_the_figures() {
    let (s, _) = promote(ALL);
    let st = s.stats();
    assert_eq!(st.mode_requested, DspSchedMode::Workgroup);
    assert_eq!(st.mode_effective, DspSchedMode::Workgroup);
    assert_eq!(st.fallback_reason, None);
    assert_eq!(st.time_constraint, Some(RT_PARAMS));
    assert_eq!(st.interval_deadline, None, "only OwnWorkgroup has one");
    assert_eq!(st.intervals, 0);
}

#[test]
fn the_parameters_are_the_block_and_the_stage_budgets() {
    assert_eq!(RT_PARAMS.period, Duration::from_millis(10));
    assert_eq!(RT_PARAMS.computation, Duration::from_millis(6));
    assert_eq!(RT_PARAMS.constraint, Duration::from_millis(9));
    assert_eq!(DspTimeConstraint::default(), RT_PARAMS);
}

// ---- TimeConstraintOnly ----

#[test]
fn time_constraint_only_never_joins_the_units_workgroup() {
    let (mut s, c) = promote_mode(ALL, DspSchedMode::TimeConstraintOnly);
    let mut discarded = Vec::new();
    s.set_workgroup(Some(Box::new(7)), |w| discarded.push(*w));
    assert_eq!(s.mode(), DspScheduling::TimeConstraint);
    assert_eq!(calls(&c), [Call::Qos, Call::TimeConstraint(RT_PARAMS)]);
    s.set_workgroup(None, |w| discarded.push(*w));
    assert_eq!(discarded, [7], "held, not joined, then released");
    let st = s.stats();
    assert_eq!(st.mode_requested, DspSchedMode::TimeConstraintOnly);
    assert_eq!(st.mode_effective, DspSchedMode::TimeConstraintOnly);
    assert_eq!(st.fallback_reason, None);
    assert_eq!(st.time_constraint, Some(RT_PARAMS));
    assert_eq!(st.interval_deadline, None);
}

#[test]
fn time_constraint_only_asks_for_the_configured_figures() {
    let tc = DspTimeConstraint::new(
        Duration::from_millis(10),
        Duration::from_millis(2),
        Duration::from_millis(5),
    );
    let cfg = DspSchedConfig::new(DspSchedMode::TimeConstraintOnly).with_time_constraint(tc);
    let (s, c) = promote_with(ALL, cfg);
    assert_eq!(calls(&c), [Call::Qos, Call::TimeConstraint(tc)]);
    assert_eq!(s.stats().time_constraint, Some(tc));
}

#[test]
fn a_refused_time_constraint_only_falls_back_to_workgroup() {
    let (mut s, c) = promote_mode(
        Grants {
            time_constraint: false,
            ..ALL
        },
        DspSchedMode::TimeConstraintOnly,
    );
    assert_eq!(s.mode(), DspScheduling::Qos);
    let st = s.stats();
    assert_eq!(st.mode_requested, DspSchedMode::TimeConstraintOnly);
    assert_eq!(st.mode_effective, DspSchedMode::Workgroup);
    assert_eq!(
        st.fallback_reason,
        Some(DspSchedFallback::TimeConstraintRefused)
    );
    // Workgroup without the real-time policy joins nothing.
    s.set_workgroup(Some(Box::new(7)), |_| {});
    assert!(!calls(&c).contains(&Call::Join(7)));
}

// ---- None ----

#[test]
fn none_asks_for_nothing() {
    let (mut s, c) = promote_mode(ALL, DspSchedMode::None);
    s.set_workgroup(Some(Box::new(7)), |_| {});
    s.block_start();
    s.pass_end();
    assert_eq!(s.mode(), DspScheduling::None);
    assert_eq!(
        calls(&c),
        Vec::<Call>::new(),
        "a plain thread: no QoS, no policy, no join"
    );
    let st = s.stats();
    assert_eq!(st.mode_requested, DspSchedMode::None);
    assert_eq!(st.mode_effective, DspSchedMode::None);
    assert_eq!(st.fallback_reason, None);
    assert_eq!(st.time_constraint, None);
    assert_eq!(st.interval_deadline, None);
}

// ---- OwnWorkgroup ----

#[test]
fn own_workgroup_makes_and_joins_its_interval() {
    let (s, c) = promote_mode(ALL, DspSchedMode::OwnWorkgroup);
    assert_eq!(s.mode(), DspScheduling::OwnWorkgroup);
    assert_eq!(
        calls(&c),
        [
            Call::Qos,
            Call::TimeConstraint(RT_PARAMS),
            Call::Create,
            Call::Join(OWN)
        ]
    );
    let st = s.stats();
    assert_eq!(st.mode_requested, DspSchedMode::OwnWorkgroup);
    assert_eq!(st.mode_effective, DspSchedMode::OwnWorkgroup);
    assert_eq!(st.fallback_reason, None);
    assert_eq!(st.time_constraint, Some(RT_PARAMS));
    assert_eq!(st.interval_deadline, Some(Duration::from_millis(3)));
}

#[test]
fn each_pass_is_one_interval_and_a_backlog_moves_the_deadline() {
    let (mut s, c) = promote_mode(ALL, DspSchedMode::OwnWorkgroup);
    let n = calls(&c).len();
    // One block.
    s.block_start();
    s.pass_end();
    // Three blocks in one pass.
    s.block_start();
    s.block_start();
    s.block_start();
    s.pass_end();
    // A pass with no capture audio runs no interval.
    s.pass_end();
    assert_eq!(
        calls_from(&c, n),
        [
            Call::Start(OWN, DEADLINE),
            Call::Finish(OWN),
            Call::Start(OWN, DEADLINE),
            Call::Update(OWN, DEADLINE),
            Call::Update(OWN, DEADLINE),
            Call::Finish(OWN),
        ]
    );
    assert_eq!(s.stats().intervals, 2);
    assert_eq!(s.mode(), DspScheduling::OwnWorkgroup);
}

#[test]
fn the_interval_deadline_is_configurable() {
    let d = Duration::from_micros(2_500);
    let cfg = DspSchedConfig::new(DspSchedMode::OwnWorkgroup).with_interval_deadline(d);
    let (mut s, c) = promote_with(ALL, cfg);
    s.block_start();
    s.block_start();
    s.pass_end();
    assert_eq!(
        calls_from(&c, 4),
        [Call::Start(OWN, d), Call::Update(OWN, d), Call::Finish(OWN)]
    );
    assert_eq!(s.stats().interval_deadline, Some(d));
}

#[test]
fn own_workgroup_ignores_the_units_workgroup() {
    let (mut s, c) = promote_mode(ALL, DspSchedMode::OwnWorkgroup);
    let mut discarded = Vec::new();
    s.set_workgroup(Some(Box::new(7)), |w| discarded.push(*w));
    assert_eq!(s.mode(), DspScheduling::OwnWorkgroup);
    assert!(!calls(&c).contains(&Call::Join(7)));
    s.set_workgroup(None, |w| discarded.push(*w));
    assert_eq!(discarded, [7]);
    assert_eq!(s.mode(), DspScheduling::OwnWorkgroup);
}

#[test]
fn no_work_intervals_falls_back_to_workgroup_and_joins_the_unit() {
    let (mut s, c) = promote_mode(
        Grants {
            create: false,
            ..ALL
        },
        DspSchedMode::OwnWorkgroup,
    );
    assert_eq!(s.mode(), DspScheduling::TimeConstraint);
    let st = s.stats();
    assert_eq!(st.mode_effective, DspSchedMode::Workgroup);
    assert_eq!(
        st.fallback_reason,
        Some(DspSchedFallback::OwnWorkgroupUnavailable)
    );
    // As Workgroup, it joins the unit's workgroup when the unit opens.
    s.set_workgroup(Some(Box::new(7)), |_| panic!("kept"));
    assert_eq!(s.mode(), DspScheduling::Workgroup);
    s.block_start();
    s.pass_end();
    assert_eq!(
        calls_from(&c, 2),
        [Call::Create, Call::Join(7)],
        "no intervals after the fallback"
    );
}

#[test]
fn a_refused_own_join_falls_back_to_workgroup() {
    let (mut s, c) = promote_mode(
        Grants {
            join_own: false,
            ..ALL
        },
        DspSchedMode::OwnWorkgroup,
    );
    assert_eq!(s.mode(), DspScheduling::TimeConstraint);
    assert_eq!(
        s.stats().fallback_reason,
        Some(DspSchedFallback::OwnWorkgroupJoinRefused)
    );
    assert_eq!(s.stats().mode_effective, DspSchedMode::Workgroup);
    s.block_start();
    assert!(
        !calls(&c).iter().any(|c| matches!(c, Call::Start(..))),
        "never an interval on a workgroup the thread isn't in"
    );
    drop(s);
    assert!(
        !calls(&c).iter().any(|c| matches!(c, Call::Leave(..))),
        "nothing joined, nothing to leave"
    );
}

#[test]
fn a_refused_own_workgroup_without_time_constraint_makes_none() {
    let (s, c) = promote_mode(
        Grants {
            time_constraint: false,
            ..ALL
        },
        DspSchedMode::OwnWorkgroup,
    );
    assert_eq!(s.mode(), DspScheduling::Qos);
    assert_eq!(
        s.stats().fallback_reason,
        Some(DspSchedFallback::TimeConstraintRefused)
    );
    assert!(!calls(&c).contains(&Call::Create));
}

#[test]
fn a_refused_start_leaves_the_interval_and_joins_the_unit() {
    let (mut s, c) = promote_mode(
        Grants {
            start: false,
            ..ALL
        },
        DspSchedMode::OwnWorkgroup,
    );
    s.set_workgroup(Some(Box::new(7)), |_| panic!("kept"));
    let n = calls(&c).len();
    s.block_start();
    assert_eq!(
        calls_from(&c, n),
        [
            Call::Start(OWN, DEADLINE),
            Call::Leave(OWN, 9901),
            Call::Join(7)
        ]
    );
    assert_eq!(s.mode(), DspScheduling::Workgroup);
    let st = s.stats();
    assert_eq!(st.mode_effective, DspSchedMode::Workgroup);
    assert_eq!(st.fallback_reason, Some(DspSchedFallback::IntervalRefused));
    // No more intervals.
    let n = calls(&c).len();
    s.block_start();
    s.pass_end();
    assert_eq!(calls_from(&c, n), Vec::<Call>::new());
    assert_eq!(st.intervals, 0);
}

#[test]
fn a_refused_update_finishes_then_leaves() {
    let (mut s, c) = promote_mode(
        Grants {
            update: false,
            ..ALL
        },
        DspSchedMode::OwnWorkgroup,
    );
    let n = calls(&c).len();
    s.block_start();
    s.block_start();
    s.pass_end();
    assert_eq!(
        calls_from(&c, n),
        [
            Call::Start(OWN, DEADLINE),
            Call::Update(OWN, DEADLINE),
            Call::Finish(OWN),
            Call::Leave(OWN, 9901),
        ]
    );
    assert_eq!(
        s.stats().fallback_reason,
        Some(DspSchedFallback::IntervalRefused)
    );
    assert_eq!(s.mode(), DspScheduling::TimeConstraint);
}

#[test]
fn a_refused_finish_falls_back() {
    let (mut s, c) = promote_mode(
        Grants {
            finish: false,
            ..ALL
        },
        DspSchedMode::OwnWorkgroup,
    );
    let n = calls(&c).len();
    s.block_start();
    s.pass_end();
    assert_eq!(
        calls_from(&c, n),
        [
            Call::Start(OWN, DEADLINE),
            Call::Finish(OWN),
            Call::Leave(OWN, 9901),
        ]
    );
    assert_eq!(
        s.stats().fallback_reason,
        Some(DspSchedFallback::IntervalRefused)
    );
    assert_eq!(s.stats().intervals, 0);
}

#[test]
fn drop_mid_interval_finishes_then_leaves() {
    let (mut s, c) = promote_mode(ALL, DspSchedMode::OwnWorkgroup);
    s.block_start();
    drop(s);
    let all = calls(&c);
    assert_eq!(
        all[all.len() - 2..],
        [Call::Finish(OWN), Call::Leave(OWN, 9901)]
    );
}

// ---- Platforms without the calls ----

/// A platform with none of the calls (Linux, Android, Windows).
struct NoOs;

impl SchedOs for NoOs {
    type Workgroup = u32;
    type Token = ();
    const SUPPORTED: bool = false;
    fn set_qos(&mut self) -> bool {
        panic!("called on an unsupported platform")
    }
    fn set_time_constraint(&mut self, _: DspTimeConstraint) -> bool {
        panic!("called on an unsupported platform")
    }
    fn join(&mut self, _: &u32) -> Option<()> {
        panic!("called on an unsupported platform")
    }
    fn leave(&mut self, _: &u32, _: ()) {
        panic!("called on an unsupported platform")
    }
    fn create_interval(&mut self) -> Option<Box<u32>> {
        panic!("called on an unsupported platform")
    }
    fn interval_start(&mut self, _: &u32, _: Duration) -> bool {
        panic!("called on an unsupported platform")
    }
    fn interval_update(&mut self, _: &u32, _: Duration) -> bool {
        panic!("called on an unsupported platform")
    }
    fn interval_finish(&mut self, _: &u32) -> bool {
        panic!("called on an unsupported platform")
    }
}

#[test]
fn every_mode_is_unsupported_without_the_calls() {
    for mode in [
        DspSchedMode::Workgroup,
        DspSchedMode::TimeConstraintOnly,
        DspSchedMode::None,
        DspSchedMode::OwnWorkgroup,
    ] {
        let mut s = DspSched::promote(NoOs, DspSchedConfig::new(mode));
        s.block_start();
        s.pass_end();
        assert_eq!(s.mode(), DspScheduling::None);
        let st = s.stats();
        assert_eq!(st.mode_requested, mode);
        assert_eq!(st.mode_effective, DspSchedMode::Unsupported, "{mode:?}");
        assert_eq!(st.fallback_reason, None);
        assert_eq!(st.time_constraint, None);
        assert_eq!(st.interval_deadline, None);
    }
}

// ---- Names, parsing and the config knob ----

#[test]
fn scheduling_has_log_names_and_defaults_to_none() {
    assert_eq!(DspScheduling::default(), DspScheduling::None);
    let names: Vec<&str> = [
        DspScheduling::Workgroup,
        DspScheduling::OwnWorkgroup,
        DspScheduling::TimeConstraint,
        DspScheduling::Qos,
        DspScheduling::None,
    ]
    .iter()
    .map(|m| m.as_str())
    .collect();
    assert_eq!(
        names,
        [
            "workgroup",
            "own_workgroup",
            "time_constraint",
            "qos",
            "none"
        ]
    );
}

const MODES: [DspSchedMode; 4] = [
    DspSchedMode::Workgroup,
    DspSchedMode::TimeConstraintOnly,
    DspSchedMode::None,
    DspSchedMode::OwnWorkgroup,
];

#[test]
fn modes_round_trip_through_their_names() {
    assert_eq!(DspSchedMode::default(), DspSchedMode::Workgroup);
    for m in MODES {
        assert_eq!(m.as_str().parse::<DspSchedMode>(), Ok(m));
    }
    assert_eq!(
        " Own-Workgroup ".parse::<DspSchedMode>(),
        Ok(DspSchedMode::OwnWorkgroup)
    );
    assert_eq!(
        "time_constraint".parse::<DspSchedMode>(),
        Ok(DspSchedMode::TimeConstraintOnly)
    );
    assert_eq!(DspSchedMode::Unsupported.as_str(), "unsupported");
    for bad in ["unsupported", "", "realtime"] {
        let e = bad.parse::<DspSchedMode>().expect_err(bad);
        assert_eq!(e.code, ErrorCode::InvalidConfig, "{bad:?}");
    }
}

#[test]
fn fallbacks_have_log_names() {
    let names: Vec<&str> = [
        DspSchedFallback::TimeConstraintRefused,
        DspSchedFallback::OwnWorkgroupUnavailable,
        DspSchedFallback::OwnWorkgroupJoinRefused,
        DspSchedFallback::IntervalRefused,
    ]
    .iter()
    .map(|f| f.as_str())
    .collect();
    assert_eq!(
        names,
        [
            "time_constraint_refused",
            "own_workgroup_unavailable",
            "own_workgroup_join_refused",
            "interval_refused"
        ]
    );
}

fn lookup(pairs: &[(&str, &str)]) -> impl FnMut(&str) -> Option<String> {
    let pairs: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |key| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

#[test]
fn nothing_set_is_the_default() {
    let cfg = DspSchedConfig::from_lookup(lookup(&[])).expect("defaults");
    assert_eq!(cfg, DspSchedConfig::default());
    assert_eq!(cfg.mode, DspSchedMode::Workgroup);
    assert_eq!(cfg.time_constraint, RT_PARAMS);
    assert_eq!(cfg.interval_deadline, Duration::from_millis(3));
    // An empty value counts as unset.
    let cfg = DspSchedConfig::from_lookup(lookup(&[(DspSchedConfig::ENV_MODE, " ")]))
        .expect("empty is unset");
    assert_eq!(cfg, DspSchedConfig::default());
}

#[test]
fn the_lookup_sets_the_mode_deadline_and_figures() {
    let cfg = DspSchedConfig::from_lookup(lookup(&[
        (DspSchedConfig::ENV_MODE, "own_workgroup"),
        (DspSchedConfig::ENV_INTERVAL_DEADLINE_US, "2500"),
        (DspSchedConfig::ENV_PERIOD_US, "10000"),
        (DspSchedConfig::ENV_COMPUTATION_US, "1500"),
        (DspSchedConfig::ENV_CONSTRAINT_US, "5000"),
    ]))
    .expect("valid");
    assert_eq!(cfg.mode, DspSchedMode::OwnWorkgroup);
    assert_eq!(cfg.interval_deadline, Duration::from_micros(2_500));
    assert_eq!(
        cfg.time_constraint,
        DspTimeConstraint::new(
            Duration::from_millis(10),
            Duration::from_micros(1_500),
            Duration::from_millis(5)
        )
    );
}

#[test]
fn a_bad_value_names_its_key() {
    let cases: [(&str, &str); 4] = [
        (DspSchedConfig::ENV_MODE, "fast"),
        (DspSchedConfig::ENV_INTERVAL_DEADLINE_US, "3ms"),
        (DspSchedConfig::ENV_COMPUTATION_US, "-1"),
        (DspSchedConfig::ENV_PERIOD_US, "1.5"),
    ];
    for (key, value) in cases {
        let e = DspSchedConfig::from_lookup(lookup(&[(key, value)])).expect_err(key);
        assert_eq!(e.code, ErrorCode::InvalidConfig);
        assert!(e.detail.contains(key), "{key}: {}", e.detail);
    }
}

#[test]
fn the_lookup_checks_the_figures() {
    // Computation above the constraint.
    let e = DspSchedConfig::from_lookup(lookup(&[(DspSchedConfig::ENV_COMPUTATION_US, "9500")]))
        .expect_err("computation > constraint");
    assert_eq!(e.code, ErrorCode::InvalidConfig);
    // A deadline beyond the period.
    let e = DspSchedConfig::from_lookup(lookup(&[(
        DspSchedConfig::ENV_INTERVAL_DEADLINE_US,
        "11000",
    )]))
    .expect_err("deadline > period");
    assert_eq!(e.code, ErrorCode::InvalidConfig);
}

#[test]
fn validate_rejects_impossible_figures() {
    let ms = Duration::from_millis;
    let ok = DspSchedConfig::default();
    assert_eq!(ok.validate(), Ok(()));
    let bad = [
        ok.with_mode(DspSchedMode::Unsupported),
        ok.with_time_constraint(DspTimeConstraint::new(ms(10), Duration::ZERO, ms(9))),
        ok.with_time_constraint(DspTimeConstraint::new(ms(10), ms(7), ms(6))),
        ok.with_time_constraint(DspTimeConstraint::new(ms(10), ms(6), ms(11))),
        ok.with_time_constraint(DspTimeConstraint::new(ms(1_001), ms(6), ms(9))),
        ok.with_interval_deadline(Duration::ZERO),
        ok.with_interval_deadline(ms(11)),
    ];
    for cfg in bad {
        let e = cfg.validate().expect_err(&format!("{cfg:?}"));
        assert_eq!(e.code, ErrorCode::InvalidConfig, "{cfg:?}");
    }
    // The period's bounds themselves are fine.
    assert_eq!(
        ok.with_time_constraint(DspTimeConstraint::new(ms(10), ms(10), ms(10)))
            .with_interval_deadline(ms(10))
            .validate(),
        Ok(())
    );
}

#[test]
fn the_engine_refuses_a_bad_dsp_sched() {
    let cfg = crate::EngineConfig::null(crate::NullDevice::default())
        .with_dsp_sched(DspSchedConfig::default().with_interval_deadline(Duration::ZERO));
    let e = crate::AudioEngine::start(cfg).expect_err("refused");
    assert_eq!(e.code, ErrorCode::InvalidConfig);
}

// ---- The engine ----

/// What the processing thread gets in the default mode on the platform the
/// tests run on.
const PLATFORM_MODE: DspScheduling = if cfg!(any(target_os = "macos", target_os = "ios")) {
    DspScheduling::TimeConstraint
} else {
    DspScheduling::None
};

const APPLE: bool = cfg!(any(target_os = "macos", target_os = "ios"));

/// A pass-through stage, so the stats carry a `capture_stage`.
struct Passthrough;

impl crate::CaptureStage for Passthrough {
    fn process(&mut self, _: &mut [f32; 480]) {}
    fn latency_samples(&self) -> usize {
        0
    }
    fn reset(&mut self) {}
}

fn staged_engine_with(sched: DspSchedConfig) -> crate::AudioEngine {
    let cfg = crate::EngineConfig::null(crate::NullDevice::default())
        .with_processing(crate::ProcessingConfig::off())
        .with_capture_stage(Box::new(Passthrough))
        .with_dsp_sched(sched);
    crate::AudioEngine::start(cfg).expect("start")
}

fn staged_engine() -> crate::AudioEngine {
    staged_engine_with(DspSchedConfig::default())
}

/// The first published statistics that pass `ok` (they carry the stage's
/// once published).
fn stats_where(engine: &crate::AudioEngine, ok: impl Fn(&crate::Stats) -> bool) -> crate::Stats {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let s = engine.stats();
        if s.capture_stage.is_some() && ok(&s) {
            return s;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no matching statistics published within 5 s: {s:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn published_stats(engine: &crate::AudioEngine) -> crate::Stats {
    stats_where(engine, |_| true)
}

#[test]
fn the_engine_reports_the_processing_threads_scheduling() {
    let engine = staged_engine();
    let s = published_stats(&engine);
    assert_eq!(s.dsp_scheduling, PLATFORM_MODE);
    let stage = s.capture_stage.expect("published");
    assert_eq!(stage.dsp_scheduling, PLATFORM_MODE);
    assert_eq!(s.dsp_sched.mode_requested, DspSchedMode::Workgroup);
    let effective = if APPLE {
        DspSchedMode::Workgroup
    } else {
        DspSchedMode::Unsupported
    };
    assert_eq!(s.dsp_sched.mode_effective, effective);
    assert_eq!(s.dsp_sched.time_constraint, APPLE.then_some(RT_PARAMS));
    engine.shutdown();
}

#[test]
fn the_engine_takes_the_mode_from_its_config() {
    let engine = staged_engine_with(DspSchedConfig::new(DspSchedMode::None));
    let s = published_stats(&engine);
    assert_eq!(s.dsp_scheduling, DspScheduling::None);
    assert_eq!(s.dsp_sched.mode_requested, DspSchedMode::None);
    let effective = if APPLE {
        DspSchedMode::None
    } else {
        DspSchedMode::Unsupported
    };
    assert_eq!(s.dsp_sched.mode_effective, effective);
    assert_eq!(s.dsp_sched.time_constraint, None);
    engine.shutdown();
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple {
    #![allow(unsafe_code)]

    use super::super::apple::{time_constraint_policy, AppleOs, QOS_CLASS_USER_INTERACTIVE};
    use super::*;
    use std::ffi::{c_int, c_void};

    extern "C" {
        fn pthread_get_qos_class_np(
            thread: libc::pthread_t,
            qos_class: *mut u32,
            relative_priority: *mut c_int,
        ) -> c_int;
        fn os_workgroup_cancel(wg: *mut c_void);
    }

    /// `QOS_CLASS_UNSPECIFIED` (`<sys/qos.h>`).
    const QOS_CLASS_UNSPECIFIED: u32 = 0;

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

    fn policy_of(tc: DspTimeConstraint) -> (u32, u32, u32) {
        let p = time_constraint_policy(tc).expect("timebase");
        (p.period, p.computation, p.constraint)
    }

    fn expected_policy() -> (u32, u32, u32) {
        policy_of(RT_PARAMS)
    }

    fn me() -> libc::pthread_t {
        // SAFETY: the calling thread.
        unsafe { libc::pthread_self() }
    }

    fn on_a_new_thread<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
        std::thread::spawn(f).join().expect("test thread panicked")
    }

    fn real(mode: DspSchedMode) -> DspSched<AppleOs> {
        DspSched::promote(AppleOs::new(), DspSchedConfig::new(mode))
    }

    #[test]
    fn a_new_thread_has_neither() {
        // So the next tests show the promotion, not the harness.
        on_a_new_thread(|| {
            assert_ne!(qos_of(me()), QOS_CLASS_USER_INTERACTIVE);
            assert_eq!(time_constraint_of(me()), None);
        });
    }

    #[test]
    fn promote_sets_the_time_constraint_policy() {
        on_a_new_thread(|| {
            let s = promote_current(DspSchedConfig::default());
            assert_eq!(s.mode(), DspScheduling::TimeConstraint);
            assert_eq!(time_constraint_of(me()), Some(expected_policy()));
            // Darwin reports no QoS class for a thread with a Mach policy of
            // its own: the time-constraint policy replaces the QoS class set
            // before it (which is there for when the policy is refused).
            assert_eq!(qos_of(me()), QOS_CLASS_UNSPECIFIED);
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
        fn set_time_constraint(&mut self, _: DspTimeConstraint) -> bool {
            false
        }
        fn join(&mut self, wg: &Workgroup) -> Option<()> {
            self.0.join(wg)
        }
        fn leave(&mut self, wg: &Workgroup, token: ()) {
            self.0.leave(wg, token)
        }
        fn create_interval(&mut self) -> Option<Box<Workgroup>> {
            self.0.create_interval()
        }
        fn interval_start(&mut self, wg: &Workgroup, d: Duration) -> bool {
            self.0.interval_start(wg, d)
        }
        fn interval_update(&mut self, wg: &Workgroup, d: Duration) -> bool {
            self.0.interval_update(wg, d)
        }
        fn interval_finish(&mut self, wg: &Workgroup) -> bool {
            self.0.interval_finish(wg)
        }
    }

    #[test]
    fn a_refused_time_constraint_leaves_user_interactive_qos() {
        on_a_new_thread(|| {
            let mut s = DspSched::promote(
                RefuseTimeConstraint(AppleOs::new()),
                DspSchedConfig::default(),
            );
            assert_eq!(s.mode(), DspScheduling::Qos);
            assert_eq!(qos_of(me()), QOS_CLASS_USER_INTERACTIVE);
            assert_eq!(time_constraint_of(me()), None);
            // And a thread that isn't real-time doesn't join a workgroup.
            let mut discarded = 0;
            s.set_workgroup(Some(work_interval()), |_| discarded += 1);
            assert_eq!(s.mode(), DspScheduling::Qos);
            s.set_workgroup(None, |_| discarded += 1);
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

    /// A real audio work interval, +1, made the way OwnWorkgroup makes its
    /// own (`AudioWorkIntervalCreate`, looked up at run time).
    fn work_interval() -> Box<Workgroup> {
        AppleOs::new()
            .create_interval()
            .expect("AudioWorkIntervalCreate (macOS 11 or later)")
    }

    #[test]
    fn joins_and_leaves_a_real_workgroup() {
        on_a_new_thread(|| {
            let mut s = real(DspSchedMode::Workgroup);
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
            let mut s = real(DspSchedMode::Workgroup);
            let wg = work_interval();
            // SAFETY: a live workgroup this holds a reference to.
            unsafe { os_workgroup_cancel(wg.as_ptr()) };
            s.set_workgroup(Some(wg), |_| panic!("held until the unit closes"));
            assert_eq!(s.mode(), DspScheduling::TimeConstraint);
            assert_eq!(time_constraint_of(me()), Some(expected_policy()));
        });
    }

    #[test]
    fn time_constraint_only_sets_the_policy_and_joins_nothing() {
        on_a_new_thread(|| {
            let tc = DspTimeConstraint::new(
                Duration::from_millis(10),
                Duration::from_millis(3),
                Duration::from_millis(6),
            );
            let mut s = DspSched::promote(
                AppleOs::new(),
                DspSchedConfig::new(DspSchedMode::TimeConstraintOnly).with_time_constraint(tc),
            );
            assert_eq!(s.mode(), DspScheduling::TimeConstraint);
            assert_eq!(time_constraint_of(me()), Some(policy_of(tc)));
            s.set_workgroup(Some(work_interval()), |_| {});
            assert_eq!(s.mode(), DspScheduling::TimeConstraint);
            assert_eq!(s.stats().fallback_reason, None);
        });
    }

    #[test]
    fn none_leaves_a_plain_thread() {
        on_a_new_thread(|| {
            let s = real(DspSchedMode::None);
            assert_eq!(s.mode(), DspScheduling::None);
            assert_ne!(qos_of(me()), QOS_CLASS_USER_INTERACTIVE);
            assert_eq!(time_constraint_of(me()), None);
        });
    }

    #[test]
    fn own_workgroup_runs_real_intervals() {
        on_a_new_thread(|| {
            let mut s = real(DspSchedMode::OwnWorkgroup);
            assert_eq!(s.mode(), DspScheduling::OwnWorkgroup);
            assert_eq!(s.stats().fallback_reason, None);
            assert_eq!(time_constraint_of(me()), Some(expected_policy()));
            for blocks in [1, 3, 1] {
                for _ in 0..blocks {
                    s.block_start();
                    std::thread::sleep(Duration::from_micros(200));
                }
                s.pass_end();
            }
            let st = s.stats();
            assert_eq!(st.fallback_reason, None, "the OS took every interval");
            assert_eq!(st.intervals, 3);
            assert_eq!(st.mode_effective, DspSchedMode::OwnWorkgroup);
            // Dropped mid-interval: finished, then left, before the thread
            // exits.
            s.block_start();
        });
    }

    #[test]
    fn a_cancelled_own_interval_falls_back_to_workgroup() {
        on_a_new_thread(|| {
            let mut s = real(DspSchedMode::OwnWorkgroup);
            let own = s.own().expect("made").as_ptr();
            // SAFETY: a live workgroup `s` holds a reference to.
            unsafe { os_workgroup_cancel(own) };
            s.set_workgroup(Some(work_interval()), |_| panic!("kept"));
            s.block_start();
            let st = s.stats();
            assert_eq!(st.fallback_reason, Some(DspSchedFallback::IntervalRefused));
            assert_eq!(st.mode_effective, DspSchedMode::Workgroup);
            // As Workgroup it joined the unit's workgroup it was holding.
            assert_eq!(s.mode(), DspScheduling::Workgroup);
            assert_eq!(time_constraint_of(me()), Some(expected_policy()));
        });
    }

    #[test]
    fn the_engine_runs_own_workgroup_intervals() {
        let engine = staged_engine_with(DspSchedConfig::new(DspSchedMode::OwnWorkgroup));
        // The null device delivers capture audio, so passes run intervals.
        let s = stats_where(&engine, |s| s.dsp_sched.intervals >= 5);
        assert_eq!(s.dsp_scheduling, DspScheduling::OwnWorkgroup);
        assert_eq!(
            s.capture_stage.expect("published").dsp_scheduling,
            DspScheduling::OwnWorkgroup
        );
        assert_eq!(s.dsp_sched.mode_requested, DspSchedMode::OwnWorkgroup);
        assert_eq!(s.dsp_sched.mode_effective, DspSchedMode::OwnWorkgroup);
        assert_eq!(s.dsp_sched.fallback_reason, None);
        assert_eq!(
            s.dsp_sched.interval_deadline,
            Some(Duration::from_millis(3))
        );
        assert_eq!(
            time_constraint_of(engine.dsp_pthread()),
            Some(expected_policy())
        );
        engine.shutdown();
    }

    #[test]
    fn the_engine_runs_time_constraint_only() {
        let engine = staged_engine_with(DspSchedConfig::new(DspSchedMode::TimeConstraintOnly));
        let s = published_stats(&engine);
        assert_eq!(s.dsp_scheduling, DspScheduling::TimeConstraint);
        assert_eq!(s.dsp_sched.mode_effective, DspSchedMode::TimeConstraintOnly);
        assert_eq!(
            time_constraint_of(engine.dsp_pthread()),
            Some(expected_policy())
        );
        engine.shutdown();
    }

    #[test]
    fn the_engine_runs_none_as_a_plain_thread() {
        let engine = staged_engine_with(DspSchedConfig::new(DspSchedMode::None));
        let _ = published_stats(&engine);
        assert_eq!(time_constraint_of(engine.dsp_pthread()), None);
        assert_ne!(qos_of(engine.dsp_pthread()), QOS_CLASS_USER_INTERACTIVE);
        engine.shutdown();
    }
}
