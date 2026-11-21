//! The central hold (0.46.17, prd-compute-agent-feedback-v1 "node_queued
//! jobs can't be moved").
//!
//! An auto-placed job (no node named) stays on this server until a node
//! that fits it (tier, `--needs`, `--mem`, `--cpus`, OS, `--avoid`) has a
//! free slot for it now; only then is it sent. A node's own queue only
//! ever holds jobs pinned to it on purpose. Before this, a Standard auto
//! job waited here one minute and then went into the queue of the shared
//! node with the shortest wait, where it could sit behind that node's long
//! jobs while other nodes went idle (it could only be withdrawn when
//! another node started it now, then bounce).
//!
//! - **Where the hold is.** A shared node's send pass
//!   ([`crate::compute_ws`]'s `shared_pass`) keeps an auto job back
//!   ([`any_hold`]) unless [`starts_now`] says the node starts it at once.
//!   A solo node's queue is this server's own already (the scheduler
//!   assigns only into a free slot).
//! - **Where it goes.** The sweeper (every 5 s) moves a held job to any
//!   node that can start it now, in [`dispatch_order`]: CI, then Priority,
//!   then Standard, then Batch, oldest first in each tier. A tier's
//!   per-workspace caps (at most 2 Priority) are checked at submit as
//!   before.
//! - **What it says.** A held job's wait is `held: waiting for a node with
//!   a free slot (fits: node-a, node-b)` ([`held_words`]) in `get`, `jobs`
//!   and `wait`, under the reason `held`, so an agent waits on it instead
//!   of polling.
//! - **Old nodes.** Nothing new goes to a node: the hold is this server's
//!   choice of when to send the same assign, and `k2 compute move`'s pull
//!   back uses the protocol-3 withdraw every shared node answers.

use k2_core::compute::proto::frames::{self, Offer};
use k2_core::compute::proto::refusal;
use k2_core::compute::proto::share::{self as pshare, ServerStatus, ShareView};
use k2_core::compute::proto::tiers::Tier;
use k2_core::compute::{scheduler, share, store};

use crate::compute_place::Pending;

/// The waiting code of an auto job held on this server (the mover counts
/// it as only busy, so it still moves to a node that can start it now).
pub const ANY_HOLD: &str = "any_hold";
/// The wait reason `get`, `jobs` and `wait` show for a held auto job.
pub const HELD: &str = "held";

/// Waiting codes that only mean "busy": the mover may take the job to a
/// node that starts it now ([`crate::compute_place`]'s `leave_reason`), and
/// an auto job waiting with one is shown as held ([`is_held`]).
pub const BUSY: &[&str] = &[
    refusal::PARALLEL_FULL,
    refusal::EXCLUSIVE_RUNNING,
    refusal::WAITING_FOR_IDLE,
    refusal::LOCAL_RESOURCE_LIMIT,
    scheduler::GRANT_PARALLEL_FULL,
    "next_in_turn",
    "behind_older_job",
    "retry_backoff",
    // A shared node (protocol 3): still only busy.
    ANY_HOLD,
    pshare::codes::TENANT_QUEUE_FULL,
    pshare::codes::FAIR_SHARE_WAIT,
    // Secret-env SE26: another workspace's job runs as the same user.
    k2_core::compute::proto::secret::codes::ISOLATION_WAIT,
];

/// What one send pass has sent so far (the node's offer doesn't show it).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sent {
    pub jobs: u32,
    pub mem_bytes: u64,
    pub cpu_millis: u64,
}

impl Sent {
    pub fn add(&mut self, cpu_millis: Option<u64>, mem_bytes: Option<u64>) {
        self.jobs += 1;
        self.mem_bytes += mem_bytes.unwrap_or(0);
        self.cpu_millis += cpu_millis.unwrap_or(0);
    }
}

/// Does a shared node start this auto job the moment it arrives? Its
/// share is active, a slot is free node-wide (every server's running jobs,
/// plus `pend`: asks sent since its offer or held by the mover, plus
/// `sent`: this pass), its free CPU and memory cover the ask, and the
/// node's quote says "starts now". After a send the node hasn't counted
/// yet, its quote is stale: then this server's own count of its share
/// stands in (a running place free, nothing of ours queued there). A Batch
/// job only on an idle node. `quote_mem` is the ask the node quotes
/// (`--mem`); `mem_on` the memory it holds there (with its GPU memory on a
/// unified GPU).
#[allow(clippy::too_many_arguments)]
pub fn starts_now(offer: &Offer, view: &ShareView, pend: &Pending, sent: &Sent, tier: Tier, cpu_millis: Option<u64>, quote_mem: Option<u64>, mem_on: Option<u64>) -> bool {
    if view.state != ServerStatus::Active || offer.control != frames::Control::Active {
        return false;
    }
    let extra = pend.jobs + sent.jobs;
    if offer.parallel_max > 0 && offer.running + extra >= offer.parallel_max {
        return false;
    }
    if tier == Tier::Batch && offer.running + extra > 0 {
        return false;
    }
    let mem_free = offer.free.mem_bytes.saturating_sub(pend.mem_bytes + sent.mem_bytes);
    let cpu_free = offer.free.cpu_millis.saturating_sub(pend.cpu_millis + sent.cpu_millis);
    if mem_on.is_some_and(|m| m > mem_free) || cpu_millis.is_some_and(|c| c > cpu_free) {
        return false;
    }
    if extra == 0 {
        return share::starts_now(view, cpu_millis, quote_mem);
    }
    view.running + extra < view.limits.max_running && view.queued == 0
}

/// Why an auto job waits on this server rather than in a shared node's
/// queue: `None` when it starts there now (it goes) or it is pinned (a
/// pinned job goes into its node's queue at once, as before).
pub fn any_hold(auto: bool, starts_now: bool) -> Option<frames::Refusal> {
    (auto && !starts_now).then(|| frames::Refusal::new(ANY_HOLD, "held on this server until a node that fits has a free slot (never into a node's queue)"))
}

/// Tier rank for [`dispatch_order`]: CI, Priority, Standard, Batch.
fn rank(t: Tier) -> u8 {
    match t {
        Tier::Ci => 0,
        Tier::Priority => 1,
        Tier::Batch => 3,
        _ => 2,
    }
}

/// The order the sweeper offers held jobs to nodes that free up: by tier
/// (CI, Priority, Standard, Batch), then first queued first (the sweep's
/// list is oldest first; the sort is stable).
pub fn dispatch_order(jobs: &mut [store::JobRow]) {
    jobs.sort_by_key(|j| rank(j.tier()));
}

/// An auto job waiting on this server only because no node that fits has
/// a free slot for it now (`code`: its waiting code). A retry waiting out
/// its back-off keeps its own words.
pub fn is_held(j: &store::JobRow, code: &str) -> bool {
    j.ctl.any.is_some() && j.state == "queued" && !j.node_queued && j.ctl.shards.is_empty() && code != "retry_backoff" && BUSY.contains(&code)
}

/// How many node names [`held_words`] shows before "+N more".
const SHOWN: usize = 4;

/// `held: waiting for a node with a free slot (fits: node-a, node-b, +3 more)`.
pub fn held_words(fits: &[String]) -> String {
    let list = if fits.is_empty() {
        "none online now".to_string()
    } else {
        let mut s = fits.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
        if fits.len() > SHOWN {
            s.push_str(&format!(", +{} more", fits.len() - SHOWN));
        }
        s
    };
    format!("held: waiting for a node with a free slot (fits: {list})")
}

/// The granted nodes that fit a held job (by name, sorted): the ones the
/// sweeper may send it to once one has a free slot.
pub fn fits(j: &store::JobRow, now: i64) -> Vec<String> {
    let Some(plan) = j.plan() else { return Vec::new() };
    let w = crate::compute_place::Want::of_job(j, &plan);
    let mut names: Vec<String> = crate::compute_place::candidates(&w, &[], now).into_iter().map(|c| c.node.name).collect();
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_words_name_the_nodes_that_fit() {
        assert_eq!(held_words(&[]), "held: waiting for a node with a free slot (fits: none online now)");
        let two = vec!["node-a".to_string(), "node-b".to_string()];
        assert_eq!(held_words(&two), "held: waiting for a node with a free slot (fits: node-a, node-b)");
        let six: Vec<String> = ["node-a", "node-b", "node-c", "node-d", "node-e", "node-f"].iter().map(|s| s.to_string()).collect();
        assert_eq!(held_words(&six), "held: waiting for a node with a free slot (fits: node-a, node-b, node-c, node-d, +2 more)");
    }

    #[test]
    fn every_auto_job_holds_until_a_node_starts_it_and_a_pinned_one_never() {
        let r = any_hold(true, false).expect("auto, not now: held");
        assert_eq!(r.code, ANY_HOLD);
        assert!(BUSY.contains(&r.code.as_str()), "the mover counts it as only busy");
        assert!(any_hold(true, true).is_none(), "starts now: it goes");
        assert!(any_hold(false, false).is_none(), "pinned: into its node's queue as before");
    }

    #[test]
    fn dispatch_order_is_tier_then_first_queued() {
        let job = |id: &str, tier: &str, at: i64| store::JobRow { id: id.into(), tier: tier.into(), created_at: at, ..Default::default() };
        let mut v = vec![job("s1", "normal", 1), job("b1", "batch", 2), job("p1", "priority", 3), job("s2", "normal", 4), job("c1", "ci", 5), job("p2", "priority", 6)];
        dispatch_order(&mut v);
        assert_eq!(v.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(), vec!["c1", "p1", "p2", "s1", "s2", "b1"]);
    }

    #[test]
    fn a_held_job_is_an_auto_job_waiting_here_for_a_busy_reason() {
        let auto = store::JobRow { state: "queued".into(), ctl: store::JobCtl { any: Some(Default::default()), ..Default::default() }, ..Default::default() };
        assert!(is_held(&auto, ANY_HOLD));
        assert!(is_held(&auto, refusal::PARALLEL_FULL));
        assert!(!is_held(&auto, "retry_backoff"), "a retry's back-off keeps its words");
        assert!(!is_held(&auto, "node_too_old_for_receipt_v2"), "a feature wait isn't a slot wait");
        let pinned = store::JobRow { ctl: store::JobCtl::default(), ..auto.clone() };
        assert!(!is_held(&pinned, ANY_HOLD));
        let in_queue = store::JobRow { state: "assigned".into(), node_queued: true, ..auto };
        assert!(!is_held(&in_queue, ANY_HOLD));
    }
}
