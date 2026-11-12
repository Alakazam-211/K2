//! Idle cores: `--cpus N+M` (prd-compute-blacksmith-class-v1 R2, §4.4.4).
//!
//! A job asked with `--cpus 4+8` is admitted and placed on its 4 (the
//! guarantee; `limits.cpu_millis`) and may use up to 8 (`Plan.cpus_burst`)
//! while cores are free. Linux: its unit's `CPUQuota` (or its leaf's
//! `cpu.max`) is the 8 and its `CPUWeight` comes from the 4 alone, 100 per
//! core ([`crate::admin::jobs::job_cpu_weight`]), never from its tier
//! (K2, 11-11: `--cpus` is a real guarantee). With the sum of guarantees
//! within the node's CPUs, the kernel's weighted sharing gives every job at
//! least its own `--cpus` when the node is full, and shares spare cores the
//! same way when it isn't; there is no re-limiting loop. Tiers decide only
//! queue order. macOS has no CPU limit (jobs already use idle cores there),
//! so only the env applies.
//!
//! Only for a server that speaks protocol 10 (`cpu-burst`): an older
//! server's jobs keep today's unit, leaf and env byte for byte. The node's
//! owner turns it off with `cpu_burst = false` (then every job is held to
//! its guarantee), and a root executor that can't set a unit's weight
//! (older than this k2-node) holds them too: a burst without weights could
//! take another job's guarantee.

use k2_node_proto::frames::Plan;

use super::{Node, Tenant};

/// How one job's CPUs are set here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuPlan {
    /// The ceiling, thousandths of a core: the unit's `CPUQuota` / the
    /// leaf's `cpu.max`. The burst when it applies, else the guarantee;
    /// `None` without `--cpus` (no limit, as today).
    pub quota_millis: Option<u64>,
    /// The burst in force (thousandths of a core), when it applies.
    pub burst_millis: Option<u64>,
    /// `CPUWeight` / `cpu.weight`: a protocol-10 server's jobs only.
    pub weight: Option<u64>,
    /// `K2_JOB_CPUS_MAX`: a protocol-10 server's jobs only (the burst in
    /// whole CPUs, else the same as `K2_JOB_CPUS`).
    pub max_cpus: Option<u64>,
    /// Why a burst the job asked for doesn't apply here (a log line).
    pub held: Option<String>,
}

/// The rule, without a node. `gens`: the server speaks protocol 10.
/// `policy_on`: the owner's `cpu_burst`. `weights`: where this job gets a
/// CPU limit, the limit can carry a weight (a unit needs root's
/// `cpu_weight`; a leaf always can; no limit at all needs none).
/// `job_cpus`: the job's CPUs as `K2_JOB_CPUS` says (its `--cpus` rounded
/// up, else the node's default share). `cap_millis`: the node's CPU cap
/// for this server.
pub fn cpu_plan(plan: &Plan, gens: bool, policy_on: bool, weights: bool, job_cpus: u64, cap_millis: u64) -> CpuPlan {
    let guarantee = plan.limits.cpu_millis.filter(|m| *m > 0);
    // A burst only from a protocol-10 server, and only above its
    // guarantee (the server checks it too; a bad one is ignored here).
    let asked = plan.cpus_burst.filter(|b| gens && k2_node_proto::gens::check_cpus_burst(guarantee, Some(*b)).is_ok());
    let cores = |m: u64| m.div_ceil(1000).max(1);
    let (burst, held) = match (asked, guarantee) {
        (Some(_), _) if !policy_on => (None, Some("this node's owner turned idle cores off (cpu_burst = false)".to_string())),
        (Some(_), _) if !weights => (None, Some("this node's root executor is older than its k2-node (no per-job CPU weight; k2 compute node update)".to_string())),
        (Some(b), Some(g)) if b.min(cap_millis) <= g => (None, Some(format!("the node's CPU cap for this server is {}", cores(cap_millis)))),
        (Some(b), Some(_)) => (Some(b.min(cap_millis)), None),
        _ => (None, None),
    };
    CpuPlan {
        quota_millis: burst.or(guarantee),
        burst_millis: burst,
        weight: gens.then(|| crate::admin::jobs::job_cpu_weight(guarantee.unwrap_or(job_cpus.max(1) * 1000))),
        max_cpus: gens.then(|| burst.map(cores).unwrap_or(job_cpus.max(1))),
        held,
    }
}

/// The job's log line for a burst (`None` when it asked for none).
pub fn burst_line(plan: &Plan, p: &CpuPlan) -> Option<String> {
    plan.cpus_burst?;
    let g = plan.limits.cpu_millis.unwrap_or(0).div_ceil(1000);
    Some(match (&p.held, p.max_cpus) {
        (Some(why), _) => format!("cpus: held to {g} ({why}); K2_JOB_CPUS_MAX={g}"),
        (None, Some(m)) => format!("cpus: {g} guaranteed, up to {m} while cores are free (K2_JOB_CPUS_MAX={m}; build with -j $K2_JOB_CPUS_MAX)"),
        (None, None) => return None,
    })
}

impl Node {
    /// R2: this job's CPU ceiling, weight and `K2_JOB_CPUS_MAX` here.
    /// `unit_mode`: it runs as a unit of root's executor.
    pub(crate) fn cpu_plan(&self, t: &Tenant, a: &k2_node_proto::frames::Assign, unit_mode: bool) -> CpuPlan {
        let job_cpus = self.job_cpus(t, a);
        let (cap, policy_on) = {
            let l = self.local.lock().unwrap();
            (self.caps(t, &l).0, l.policy.cpu_burst)
        };
        let weights = !unit_mode || self.exec_cpu_weight.load(std::sync::atomic::Ordering::SeqCst);
        cpu_plan(&a.plan, t.speaks_gens(), policy_on, weights, job_cpus, cap)
    }

    /// Offer `cpu-burst` to `t` (protocol 10, the owner's `cpu_burst` on).
    pub(crate) fn offers_cpu_burst(&self, t: &Tenant) -> bool {
        t.speaks_gens() && self.local.lock().unwrap().policy.cpu_burst
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_node_proto::tiers::Tier;

    fn plan(cpus: Option<u64>, burst: Option<u64>, tier: Tier) -> Plan {
        let mut a = crate::ledger::tests::assign("j", 1);
        a.plan.limits.cpu_millis = cpus;
        a.plan.cpus_burst = burst;
        a.plan.tier = Some(tier);
        a.plan
    }

    /// §10 R2: a `--cpus 2+8` job's ceiling is 8 and its weight comes from
    /// the 2 (admission counts the 2: `limits.cpu_millis` is unchanged).
    #[test]
    fn a_burst_job_gets_its_ceiling_and_a_weight_from_its_guarantee() {
        let p = plan(Some(2000), Some(8000), Tier::Normal);
        let c = cpu_plan(&p, true, true, true, 2, 12_000);
        assert_eq!(c, CpuPlan { quota_millis: Some(8000), burst_millis: Some(8000), weight: Some(200), max_cpus: Some(8), held: None });
        assert_eq!(p.limits.cpu_millis, Some(2000), "placement and admission count the guarantee");
        assert!(burst_line(&p, &c).unwrap().contains("2 guaranteed, up to 8"));
        // The tier never changes the weight: only the guarantee does.
        for t in [Tier::Ci, Tier::Priority, Tier::Normal, Tier::Batch] {
            assert_eq!(cpu_plan(&plan(Some(4000), Some(8000), t), true, true, true, 4, 16_000).weight, Some(400), "{t:?}");
        }
        // Never past the node's cap for this server.
        let c = cpu_plan(&p, true, true, true, 2, 6_000);
        assert_eq!((c.quota_millis, c.max_cpus), (Some(6000), Some(6)));
        // A cap at the guarantee: no burst, and said.
        let c = cpu_plan(&p, true, true, true, 2, 2_000);
        assert_eq!((c.quota_millis, c.burst_millis), (Some(2000), None));
        assert!(c.held.unwrap().contains("CPU cap"));
    }

    /// K2 11-11: `--cpus` is a real guarantee. On a full node, jobs of
    /// different tiers that all want more than they get (two bursting,
    /// one held) each get at least their own `--cpus` by weight, and the
    /// spare cores go by guarantee too.
    #[test]
    fn full_node_jobs_at_different_tiers_each_get_their_guarantee() {
        use crate::admin::jobs::weighted_shares;
        let cap = 16_000;
        let cases: [&[(u64, Option<u64>, Tier)]; 4] = [
            // A Standard 8 beside a CI 8 bursting to 16.
            &[(8000, None, Tier::Normal), (8000, Some(16_000), Tier::Ci)],
            // A Batch 2, a Priority 6 bursting to 12, a CI 4 bursting to 16.
            &[(2000, None, Tier::Batch), (6000, Some(12_000), Tier::Priority), (4000, Some(16_000), Tier::Ci)],
            // Guarantees short of the node: the spare 6 cores split by guarantee.
            &[(6000, Some(16_000), Tier::Batch), (4000, Some(16_000), Tier::Ci)],
            // Fractional guarantees.
            &[(500, Some(4000), Tier::Batch), (1500, Some(8000), Tier::Ci), (14_000, None, Tier::Normal)],
        ];
        for jobs in cases {
            let plans: Vec<CpuPlan> = jobs.iter().map(|(g, b, t)| cpu_plan(&plan(Some(*g), *b, *t), true, true, true, g.div_ceil(1000), cap)).collect();
            let weights: Vec<u64> = plans.iter().map(|p| p.weight.expect("a weight")).collect();
            let shares = weighted_shares(cap, &weights);
            for ((g, _, t), share) in jobs.iter().zip(&shares) {
                assert!(*share >= *g, "{t:?} guaranteed {g} gets {share} of {cap} ({jobs:?}, weights {weights:?})");
            }
        }
        // The spare cores go by guarantee, not tier: 6 and 4 of 16 → 9.6 and 6.4.
        let w: Vec<u64> = [(6000, Tier::Batch), (4000, Tier::Ci)].iter().map(|(g, t)| cpu_plan(&plan(Some(*g), Some(16_000), *t), true, true, true, 6, cap).weight.unwrap()).collect();
        assert_eq!(weighted_shares(cap, &w), vec![9600, 6400]);
    }

    /// The owner's switch and an older root executor hold a burst job to
    /// its guarantee, and its log says why.
    #[test]
    fn off_or_an_older_executor_holds_it_to_its_guarantee() {
        let p = plan(Some(2000), Some(8000), Tier::Normal);
        let off = cpu_plan(&p, true, false, true, 2, 12_000);
        assert_eq!((off.quota_millis, off.max_cpus, off.weight), (Some(2000), Some(2), Some(200)));
        assert!(off.held.as_deref().unwrap().contains("cpu_burst = false"));
        assert!(burst_line(&p, &off).unwrap().starts_with("cpus: held to 2"));
        let old = cpu_plan(&p, true, true, false, 2, 12_000);
        assert_eq!(old.quota_millis, Some(2000));
        assert!(old.held.as_deref().unwrap().contains("root executor"));
    }

    /// A protocol-9 server's job is today's: its guarantee as the ceiling,
    /// no weight, no `K2_JOB_CPUS_MAX`, whatever its plan says.
    #[test]
    fn an_older_servers_job_is_unchanged() {
        let p = plan(Some(2000), Some(8000), Tier::Normal);
        assert_eq!(cpu_plan(&p, false, true, true, 2, 12_000), CpuPlan { quota_millis: Some(2000), burst_millis: None, weight: None, max_cpus: None, held: None });
        let none = plan(None, None, Tier::Normal);
        assert_eq!(cpu_plan(&none, false, true, true, 3, 12_000), CpuPlan { quota_millis: None, burst_millis: None, weight: None, max_cpus: None, held: None });
    }

    /// No burst asked (protocol 10): today's ceiling, a weight from the
    /// guarantee (or the default share) so bursting jobs beside it never
    /// take its share, and `K2_JOB_CPUS_MAX` = `K2_JOB_CPUS`.
    #[test]
    fn a_plain_job_keeps_its_ceiling_and_gets_a_weight() {
        let c = cpu_plan(&plan(Some(4000), None, Tier::Normal), true, true, true, 4, 12_000);
        assert_eq!(c, CpuPlan { quota_millis: Some(4000), burst_millis: None, weight: Some(400), max_cpus: Some(4), held: None });
        let d = cpu_plan(&plan(None, None, Tier::Normal), true, true, true, 3, 12_000);
        assert_eq!(d, CpuPlan { quota_millis: None, burst_millis: None, weight: Some(300), max_cpus: Some(3), held: None });
        assert_eq!(burst_line(&plan(Some(4000), None, Tier::Normal), &c), None);
        // A burst not above the guarantee (or without one) is ignored.
        assert_eq!(cpu_plan(&plan(Some(4000), Some(4000), Tier::Normal), true, true, true, 4, 12_000).quota_millis, Some(4000));
        assert_eq!(cpu_plan(&plan(None, Some(8000), Tier::Normal), true, true, true, 3, 12_000).quota_millis, None);
    }
}
