//! The line already waiting on a node (0.46.15, a child of `compute_place`).
//!
//! The field case (K2 Engine, 11-16): a test node, shared, with 2 slots,
//! ran 1 job and had 20 of 24 CPUs unreserved, while 9 auto jobs waited in
//! its queue for up to 10 minutes and other Linux nodes had room. The
//! queued jobs were one workspace's, held by its grant there (1 at a
//! time). The node's quote is for a job of a workspace with nothing there
//! (k2-node's `queue::quote`), so it said "starts now", and placement took
//! that as the job's wait: 0. Every job of that workspace that no node
//! could start now went to that test node (the shortest wait), and a `--cache`
//! job whose cache lived there waited there (a wait under 15 min), then
//! wasn't moved for 15 min.
//!
//! - A shared node where this job's workspace is at its grant's limit: the
//!   wait is at least the workspace's own line there ([`est_wait`]: its
//!   jobs queued there, per grant slot, after the first of its running
//!   ones ends), whatever the quote says ([`wait`]).
//! - A shared node with this server's jobs still held here for it (not in
//!   its offer or this server's running count yet): they go first, so the
//!   node starts a new job now only with a slot free for each of them too
//!   ([`Cand::slot_free_behind_line`]).
//!
//! A solo node keeps its own estimate ([`super::SoloWaits::eta`]) and its
//! rule (it starts nothing new while a job waits there).

use k2_core::compute::proto::frames::Offer;
use k2_core::compute::share;
use k2_core::compute::store::{GrantRow, JobRow};

use super::{scheduler, solo_eta, Cand, SoloWaits, Want};

impl Cand {
    /// A node slot free for this job behind this server's jobs already
    /// held here for the node (`queued`; they go first), and a running
    /// slot of this server's share for each of them too.
    pub(crate) fn slot_free_behind_line(&self) -> bool {
        let line = self.queued as u32;
        if line == 0 {
            return self.node_slot_free();
        }
        let node = self.offer.parallel_max == 0 || self.running_now() + line < self.offer.parallel_max;
        node && self.shared_view().is_none_or(|v| v.running + self.pending.jobs + line < v.limits.max_running)
    }
}

/// This server's estimate of the wait for workspace `ws_id`'s next job on
/// the node `node_id` (its offer `offer`): a solo node's line
/// ([`SoloWaits::eta`], as before); on a shared node, only while the
/// workspace runs its grant's most jobs there, its own line there (the
/// node's quote can't know that limit). `None`: the quote stands.
///
/// `node_jobs`: the node's queued and active jobs, read with the rest of
/// placement's snapshot (one query for every node, never a lock per node).
#[allow(clippy::too_many_arguments)]
pub(super) fn est_wait<'a>(
    waits: &SoloWaits,
    node_id: &str,
    offer: &Offer,
    grant: Option<&GrantRow>,
    load: &scheduler::Load,
    ws_id: &str,
    node_jobs: impl IntoIterator<Item = &'a JobRow>,
    now: i64,
) -> Option<u64> {
    if !share::is_shared(offer) {
        return Some(waits.eta(node_id, offer.parallel_max));
    }
    let cap = grant.map_or(1, |g| g.max_parallel.max(1) as u32);
    if load.running_by_workspace.get(ws_id).copied().unwrap_or(0) < cap {
        return None;
    }
    Some(ws_line(node_jobs.into_iter().filter(|j| j.workspace_id == ws_id).map(line_entry), cap, now))
}

/// One job of the line: (waiting to start, started at, its estimate).
pub(super) type Entry = (bool, Option<i64>, Option<u64>);

fn line_entry(j: &JobRow) -> Entry {
    (j.state == "queued" || j.node_queued, j.started_at, j.est_secs.filter(|e| *e > 0).map(|e| e as u64))
}

/// A workspace's line on one node, `cap` of its jobs at a time: the
/// estimates of its waiting jobs ÷ `cap`, plus the least time left on one
/// of its running jobs ([`solo_eta`]; an unknown estimate counts 15 min).
pub(super) fn ws_line(jobs: impl IntoIterator<Item = Entry>, cap: u32, now: i64) -> u64 {
    let (mut ahead, mut left) = (Vec::new(), Vec::new());
    for (waiting, started, est) in jobs {
        if waiting {
            ahead.push(est);
            continue;
        }
        left.push(match (started, est) {
            (Some(s), Some(e)) => Some((s + e as i64 - now).max(0) as u64),
            (None, e) => e,
            (_, None) => None,
        });
    }
    solo_eta(&ahead, &left, cap)
}

/// The wait for `w` on `c` and its basis ([`Cand::wait_eta`]): a shared
/// node's quote plus this server's jobs still held here for it (`quote`),
/// raised to this workspace's own line there when that is longer
/// (`estimate`); else (a solo node, or a shared one without a quote) this
/// server's estimate, when it has one.
pub(super) fn wait(c: &Cand, w: &Want) -> Option<(u64, &'static str)> {
    match (c.eta(w).map(|e| e + c.held_wait), c.est_wait) {
        (Some(q), Some(line)) if line > q => Some((line, "estimate")),
        (Some(q), _) => Some((q, "quote")),
        (None, e) => e.map(|e| (e, "estimate")),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{cand, shared, want};
    use super::super::{choose_by, rank, Tier, UNKNOWN_EST_SECS};
    use super::*;
    use k2_core::compute::PlacementPolicy::{V1, V2};

    const GB: u64 = 1 << 30;

    /// A 2 GB job that needs cmake, ready on both nodes below (the tools
    /// don't decide between them).
    fn job() -> Want {
        Want { needs: vec!["cmake@3".into()], ..want(2) }
    }

    fn tooled(mut c: Cand) -> Cand {
        c.offer.tools.insert("mise:cmake".into(), "3.31.1".into());
        c.offer.features.push(k2_core::compute::proto::needs::FEATURE_NEEDS.into());
        c
    }

    /// The field case's node: 24 CPUs, 64 GB, 2 slots, shared, 1 job
    /// running; its quote says a 2 GB job starts now (it quotes for a
    /// workspace with nothing there).
    fn big() -> Cand {
        let mut c = tooled(shared(cand("a-big", 64, 64, 1, 2), Some((true, None))));
        c.offer.caps.cpu_millis = 24_000;
        c.offer.free.cpu_millis = 20_000;
        let v = c.offer.share.as_mut().expect("share");
        v.limits.max_running = 2;
        v.running = 1;
        c
    }

    /// A smaller idle solo node that can start the job now.
    fn small() -> Cand {
        tooled(cand("b-small", 16, 16, 0, 2))
    }

    fn order<'a>(policy: k2_core::compute::PlacementPolicy, cands: &'a [Cand]) -> Vec<(&'a str, Tier)> {
        rank(policy, cands, &job(), &[], None, None).into_iter().map(|(c, t)| (c.node.id.as_str(), t)).collect()
    }

    /// A big node whose slots are full once this server's jobs held for it
    /// count (1 running + 3 waiting, 2 slots) is not "now", however much
    /// CPU it has free; the smaller idle node starts the job.
    #[test]
    fn a_node_with_a_line_ranks_behind_an_idle_one() {
        let w = job();
        let mut b = big();
        assert!(b.starts_now(&w), "nothing waits: a free slot and 20 free CPUs");
        let both = [b.clone(), small()];
        let (first, _) = choose_by(V2, &both, &w, &[], None, None).expect("a pick");
        assert_eq!(first.node.id, "a-big", "with nothing waiting there it wins on load");
        b.queued = 3;
        assert!(!b.slot_free_behind_line());
        assert!(!b.starts_now(&w), "3 jobs of this server wait for it first");
        assert_eq!(b.tier(&w), Some(Tier::Busy), "it still queues there");
        let why = super::super::why_not::reason(&b, &w).expect("a reason");
        assert_eq!(why, (super::super::why_not::QUEUED_THERE, "3 job(s) of this server already wait for it".to_string()));
        for policy in [V1, V2] {
            assert_eq!(order(policy, &[b.clone(), small()])[0], ("b-small", Tier::Now), "{policy:?}");
        }
        // Room for every held job and this one: it starts.
        let mut roomy = big();
        roomy.offer.parallel_max = 6;
        roomy.offer.share.as_mut().expect("share").limits.max_running = 6;
        roomy.queued = 3;
        assert!(roomy.starts_now(&w), "1 running + 3 held + this one fit in 6");
        // The share's own limit counts the held ones too.
        roomy.offer.share.as_mut().expect("share").limits.max_running = 4;
        assert!(!roomy.starts_now(&w), "this server's share runs 4 at most: 1 + 3 held");
    }

    /// A workspace at its grant's limit on a shared node waits for its own
    /// line there: 3 queued (15 min each, unknown) one at a time, after
    /// its running job (unknown: 15 min).
    #[test]
    fn a_capped_workspace_waits_for_its_own_line() {
        let now = 10_000;
        let line = ws_line([(false, None, None), (true, None, None), (true, None, None), (true, None, None)], 1, now);
        assert_eq!(line, 4 * UNKNOWN_EST_SECS);
        // Known estimates; two at a time; a running job 100 s from its end.
        assert_eq!(ws_line([(false, Some(now - 500), Some(600)), (true, None, Some(400)), (true, None, Some(200))], 2, now), 300 + 100);
        assert_eq!(ws_line(std::iter::empty(), 1, now), 0);
    }

    /// The quote says "starts now" (0 s), but this workspace's line there
    /// is an hour: the wait is the line, and a busy node with a shorter
    /// wait ranks first. Without a line (not at the grant's limit), the
    /// quote stands, as before.
    #[test]
    fn the_line_raises_a_quote_that_cant_be_this_jobs() {
        let w = job();
        let mut b = big();
        b.load.running_by_workspace.insert(w.ws_id.clone(), 4);
        b.grant.as_mut().expect("grant").max_parallel = 1;
        assert_eq!(b.tier(&w), Some(Tier::Busy), "its grant there runs 1 at a time");
        assert_eq!(b.wait_eta(&w), Some((0, "quote")), "no line known: the quote");
        b.est_wait = Some(4 * UNKNOWN_EST_SECS);
        assert_eq!(b.wait_eta(&w), Some((3600, "estimate")));
        // Busy everywhere: the small node's 15 min beats the big node's
        // hour (v2; v1 ranks busy nodes by their quotes only, as before).
        let mut s = tooled(cand("b-small", 16, 16, 1, 1));
        s.est_wait = Some(UNKNOWN_EST_SECS);
        assert_eq!(s.tier(&w), Some(Tier::Busy));
        assert_eq!(order(V2, &[b.clone(), s.clone()]), vec![("b-small", Tier::Busy), ("a-big", Tier::Busy)]);
        // A quote longer than the line wins (it knows other servers' jobs).
        let mut slow = big();
        slow.offer.share.as_mut().expect("share").quotes[0] =
            k2_core::compute::proto::share::Quote { cpu_millis: 0, mem_bytes: 2 * GB, starts_now: false, eta_secs: Some(7200), eta_p90_secs: None, blocked_by: None };
        slow.est_wait = Some(3600);
        assert_eq!(slow.wait_eta(&w), Some((7200, "quote")));
        // A solo node: its estimate, as before.
        assert_eq!(s.wait_eta(&w), Some((UNKNOWN_EST_SECS, "estimate")));
    }
}
