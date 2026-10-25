//! Never stuck, smart retries (research-distributed-compute-gaps G3/G4).
//!
//! Pure policy, so the daemon's sweeper and tests agree:
//! - which ends are infrastructure failures (retried, preferably on
//!   another node) and which are results (never retried: a failing test,
//!   a timeout, a cancel, a job over its memory cap);
//! - the backoff between attempts;
//! - which failures count against a node (quarantine).
//!
//! Delivery rule (say it to agents): a job starts at most once per
//! attempt; a retry is a new attempt (new generation); make side effects
//! idempotent.

use super::proto::frames::JobState;

/// Knobs. Defaults are the product values; tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tuning {
    /// A queued `--any` job leaves a node that has been offline this long.
    pub replace_after_secs: i64,
    /// A running job whose node has been offline this long is `interrupted/node_lost`.
    pub node_lost_secs: i64,
    /// First retry delay; doubles per retry, capped.
    pub retry_base_secs: i64,
    pub retry_cap_secs: i64,
    /// Consecutive infra failures before a node is set aside.
    pub quarantine_after: u32,
    pub quarantine_secs: i64,
    /// Default queue timeouts.
    pub queue_timeout_attached_secs: i64,
    pub queue_timeout_detached_secs: i64,
    /// Default stall timeout (no output for this long ends the job).
    pub stall_secs: i64,
    /// The controller's compute ticker period.
    pub tick_millis: u64,
    /// `--any` (0.46.3, V71): when no node can start the job now, it waits
    /// on the server this long before it goes into a shared node's queue.
    pub any_hold_secs: i64,
    /// Owner side: how often a node's server list is mirrored (§11.2).
    pub servers_refresh_secs: i64,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            replace_after_secs: 60,
            node_lost_secs: 600,
            retry_base_secs: 30,
            retry_cap_secs: 600,
            quarantine_after: 3,
            quarantine_secs: 1800,
            queue_timeout_attached_secs: 1800,
            queue_timeout_detached_secs: 86_400,
            stall_secs: 1800,
            tick_millis: 5000,
            any_hold_secs: 60,
            servers_refresh_secs: 300,
        }
    }
}

static TUNING: std::sync::RwLock<Option<Tuning>> = std::sync::RwLock::new(None);

/// The knobs in force.
pub fn tuning() -> Tuning {
    let mut t = TUNING.read().ok().and_then(|g| *g).unwrap_or_default();
    let knobs = std::env::var(TEST_KNOBS_ENV).ok().is_some_and(|v| v.trim() == "1");
    if let Some(s) = node_lost_override(std::env::var(NODE_LOST_ENV).ok().as_deref(), cfg!(debug_assertions), knobs) {
        t.node_lost_secs = s;
    }
    t
}

/// Test knob (the headless two-server harness): a short `node_lost_secs`.
pub const NODE_LOST_ENV: &str = "K2_COMPUTE_NODE_LOST_SECS";
/// Required as well (`=1`) for [`NODE_LOST_ENV`] to count in a release
/// build: a stray variable alone never shortens a real install's timeouts.
pub const TEST_KNOBS_ENV: &str = "K2_COMPUTE_TEST_KNOBS";

/// [`NODE_LOST_ENV`] as read: honoured only in a debug (test/dev) build or
/// with [`TEST_KNOBS_ENV`]`=1`, and only 5–600 s (never longer than the
/// product default, never a hair trigger).
pub fn node_lost_override(value: Option<&str>, debug_build: bool, test_knobs: bool) -> Option<i64> {
    if !(debug_build || test_knobs) {
        return None;
    }
    value?.trim().parse::<i64>().ok().filter(|s| (5..=600).contains(s))
}

/// Tests only: shorten the waits for this process.
pub fn set_tuning_for_tests(t: Tuning) {
    if let Ok(mut g) = TUNING.write() {
        *g = Some(t);
    }
}

/// Most retries a job may ask for.
pub const MAX_RETRIES: u32 = 5;

/// `Failed` reasons that are the job's own doing (same on every node):
/// never retried, never held against a node.
const JOB_FAILED_REASONS: &[&str] = &[
    "cwd_invalid",
    "env_refused",
    "tool_unsupported",
    "need_invalid",
    "mem_cap",
    "wrong_node",
    "queue_timeout",
    // Over the node's caps (compute DX slice A): the job asks too much.
    "exceeds_node_caps",
    // Compute secret-env (SE11, SE20): the grant, the secret or the ask
    // decides these, the same on every node.
    super::proto::secret::codes::NOT_GRANTED,
    super::proto::secret::codes::NOT_ACTIVE,
    super::proto::secret::codes::ATTEMPT_STALE,
    super::proto::secret::codes::ALREADY_RUNNING,
    super::proto::secret::codes::TOO_LARGE,
    super::proto::secret::codes::PAYLOAD_MISMATCH,
];

/// `Failed` reasons that are about the job's needs on this node (a
/// package the owner hasn't approved, a tool that won't install): worth
/// another node, but not the node's fault.
const NEEDS_FAILED_REASONS: &[&str] = &[
    "package_not_approved",
    "package_missing",
    "tool_install_failed",
    "relay_cap",
    // Secret-env refused at assign by this node (SE17): no wall, not a
    // `trust own` server, or an older executor. Another node may serve it.
    super::proto::secret::codes::NEEDS_WALL,
    super::proto::secret::codes::UNTRUSTED_SERVER,
    super::proto::secret::codes::NEEDS_UPDATE,
    super::proto::secret::codes::LINUX_ONLY,
];

/// Is this end an infrastructure failure (the command never got a fair
/// run)? `Done` (any exit code), `Timeout`, `Cancelled` are results.
pub fn is_infra(state: JobState, reason: Option<&str>) -> bool {
    match state {
        JobState::Interrupted | JobState::Unknown => true,
        JobState::Failed => !reason.is_some_and(|r| JOB_FAILED_REASONS.contains(&r)),
        _ => false,
    }
}

/// Only worth retrying on ANOTHER node (the job's needs on this node):
/// a job pinned to one node doesn't retry these.
pub fn retry_elsewhere_only(reason: Option<&str>) -> bool {
    reason.is_some_and(|r| NEEDS_FAILED_REASONS.contains(&r))
}

/// Ends that are infrastructure (retried) but never the node's fault:
/// `interrupted/node_update`, a shared node's drained update (V55).
/// Secret-env (SE11): the link dropped or the vault was locked during the
/// release: infrastructure, retried, never the node's fault.
const NO_STRIKE_REASONS: &[&str] = &[
    super::proto::share::codes::NODE_UPDATE,
    super::proto::secret::codes::UNAVAILABLE,
    super::proto::secret::codes::VAULT_LOCKED,
];

/// Does this end count against the node (quarantine)?
pub fn counts_against_node(state: JobState, reason: Option<&str>) -> bool {
    is_infra(state, reason) && !reason.is_some_and(|r| NEEDS_FAILED_REASONS.contains(&r) || NO_STRIKE_REASONS.contains(&r))
}

/// Delay before retry number `n` (1-based): base · 2^(n-1), capped, with
/// ±20 % jitter from `seed` so a batch of failed shards doesn't stampede.
pub fn backoff_secs(n: u32, t: &Tuning, seed: u64) -> i64 {
    let exp = n.saturating_sub(1).min(20);
    let raw = t.retry_base_secs.saturating_mul(1i64 << exp).min(t.retry_cap_secs).max(0);
    if raw == 0 {
        return 0;
    }
    let span = (raw / 5).max(0);
    if span == 0 {
        return raw;
    }
    let jitter = (seed % (2 * span as u64 + 1)) as i64 - span;
    (raw + jitter).clamp(0, t.retry_cap_secs.max(raw))
}

/// Consecutive node failures (in memory on the controller).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeHealth {
    pub consecutive: u32,
    /// Set aside until this time.
    pub until: Option<i64>,
}

impl NodeHealth {
    /// Record one attempt's end. Returns true when this end set the node aside.
    pub fn record(&mut self, state: JobState, reason: Option<&str>, now: i64, t: &Tuning) -> bool {
        if counts_against_node(state, reason) {
            self.consecutive += 1;
            if self.consecutive >= t.quarantine_after.max(1) && !self.is_set_aside(now) {
                self.until = Some(now + t.quarantine_secs);
                self.consecutive = 0;
                return true;
            }
        } else if state == JobState::Done {
            // The node ran a command to its end: it works.
            self.consecutive = 0;
        }
        false
    }

    pub fn is_set_aside(&self, now: i64) -> bool {
        self.until.is_some_and(|u| u > now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_are_never_infra() {
        assert!(!is_infra(JobState::Done, None), "a failing test is a result");
        assert!(!is_infra(JobState::Timeout, Some("timeout")));
        assert!(!is_infra(JobState::Timeout, Some("stalled")), "a hung test is a result too");
        assert!(!is_infra(JobState::Cancelled, Some("cancelled")));
        assert!(!is_infra(JobState::Failed, Some("mem_cap")));
        assert!(!is_infra(JobState::Failed, Some("tool_unsupported")));
        assert!(!is_infra(JobState::Failed, Some("queue_timeout")));
        assert!(is_infra(JobState::Interrupted, Some("node_reboot")));
        assert!(is_infra(JobState::Interrupted, Some("node_lost")));
        assert!(is_infra(JobState::Unknown, Some("not_on_node")));
        assert!(is_infra(JobState::Failed, Some("sync_failed")));
        assert!(is_infra(JobState::Failed, Some("spawn_failed")));
        assert!(is_infra(JobState::Failed, Some("package_not_approved")), "another node may have it");
        assert!(!counts_against_node(JobState::Failed, Some("package_not_approved")), "not the node's fault");
        assert!(retry_elsewhere_only(Some("package_missing")) && !retry_elsewhere_only(Some("node_lost")));
        assert!(counts_against_node(JobState::Interrupted, Some("node_lost")));
        // V55: a shared node's drained update retries, with no strike.
        assert!(is_infra(JobState::Interrupted, Some("node_update")));
        assert!(!counts_against_node(JobState::Interrupted, Some("node_update")));
    }

    /// The harness's short node_lost_secs never reaches a release build by
    /// accident: it needs a debug build or the explicit test-knobs switch.
    #[test]
    fn the_node_lost_knob_needs_a_test_build_or_the_switch() {
        assert_eq!(node_lost_override(Some("10"), false, false), None, "release build, no switch: ignored");
        assert_eq!(node_lost_override(Some("10"), true, false), Some(10));
        assert_eq!(node_lost_override(Some("10"), false, true), Some(10));
        assert_eq!(node_lost_override(Some("1"), true, true), None, "under 5 s is refused");
        assert_eq!(node_lost_override(Some("6000"), true, true), None, "never longer than the default");
        assert_eq!(node_lost_override(Some("x"), true, true), None);
        assert_eq!(node_lost_override(None, true, true), None);
    }

    #[test]
    fn node_updates_never_set_a_node_aside() {
        let t = Tuning { quarantine_after: 2, quarantine_secs: 100, ..Tuning::default() };
        let mut h = NodeHealth::default();
        for at in 1..10 {
            assert!(!h.record(JobState::Interrupted, Some("node_update"), at, &t), "update {at} set the node aside");
        }
        assert_eq!(h.consecutive, 0);
        assert!(!h.is_set_aside(10));
    }

    #[test]
    fn backoff_doubles_caps_and_jitters_within_a_fifth() {
        let t = Tuning::default();
        assert_eq!(backoff_secs(1, &t, 12), 30 + (12 % 13) as i64 - 6);
        for seed in 0..50u64 {
            let b1 = backoff_secs(1, &t, seed);
            assert!((24..=36).contains(&b1), "{b1}");
            let b2 = backoff_secs(2, &t, seed);
            assert!((48..=72).contains(&b2), "{b2}");
            let b9 = backoff_secs(9, &t, seed);
            assert!((480..=600).contains(&b9), "capped: {b9}");
        }
        let zero = Tuning { retry_base_secs: 0, ..t };
        assert_eq!(backoff_secs(3, &zero, 7), 0);
    }

    #[test]
    fn a_node_is_set_aside_after_k_infra_failures_in_a_row() {
        let t = Tuning { quarantine_after: 3, quarantine_secs: 100, ..Tuning::default() };
        let mut h = NodeHealth::default();
        assert!(!h.record(JobState::Interrupted, Some("node_lost"), 1, &t));
        assert!(!h.record(JobState::Failed, Some("sync_failed"), 2, &t));
        // A finished command resets the run of failures.
        assert!(!h.record(JobState::Done, None, 3, &t));
        assert_eq!(h.consecutive, 0);
        assert!(!h.record(JobState::Unknown, None, 4, &t));
        assert!(!h.record(JobState::Unknown, None, 5, &t));
        // Job-side failures never count.
        assert!(!h.record(JobState::Failed, Some("mem_cap"), 6, &t));
        assert!(!h.record(JobState::Timeout, Some("stalled"), 6, &t));
        assert!(h.record(JobState::Failed, Some("spawn_failed"), 7, &t), "third in a row");
        assert!(h.is_set_aside(50));
        assert!(!h.is_set_aside(107), "for a while, not forever");
    }

    /// SE11: grant/secret refusals are results; a dropped link or a locked
    /// vault is infrastructure that never counts against the node; a node
    /// that can't take secrets is worth another node only.
    #[test]
    fn secret_env_reasons_are_classified() {
        use crate::compute::proto::secret::codes as c;
        for r in [c::NOT_GRANTED, c::NOT_ACTIVE, c::ATTEMPT_STALE, c::ALREADY_RUNNING, c::TOO_LARGE, c::PAYLOAD_MISMATCH] {
            assert!(!is_infra(JobState::Failed, Some(r)), "{r} is a result");
            assert!(!counts_against_node(JobState::Failed, Some(r)), "{r}");
        }
        for r in [c::UNAVAILABLE, c::VAULT_LOCKED] {
            assert!(is_infra(JobState::Failed, Some(r)), "{r} is retried");
            assert!(!counts_against_node(JobState::Failed, Some(r)), "{r} never strikes the node");
        }
        for r in [c::NEEDS_WALL, c::UNTRUSTED_SERVER, c::NEEDS_UPDATE, c::LINUX_ONLY] {
            assert!(is_infra(JobState::Failed, Some(r)) && retry_elsewhere_only(Some(r)), "{r}");
            assert!(!counts_against_node(JobState::Failed, Some(r)), "{r}");
        }
    }
}
