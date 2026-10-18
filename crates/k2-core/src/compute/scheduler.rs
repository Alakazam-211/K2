//! The per-node fair queue (§10.2, lifted from FICC's fair scheduler).
//! Pure: the daemon feeds it the queue, what's running and the node's
//! last offer, and it answers which job to place next (one placement per
//! call) plus, for display, each queued job's position and reason.
//!
//! Rules: round-robin across workspaces (rotating after the workspace
//! that got the last placement), FIFO by `created_at` within a
//! workspace, the node's `parallel_max` and each grant's `max_parallel`.
//! An exclusive job waits for an idle node; when it is the OLDEST queued
//! job it also holds every other start, so a stream of small jobs can't
//! starve it.

use std::collections::{BTreeMap, HashMap};

use super::proto::frames::{Offer, Refusal};
use super::proto::refusal::{self, JobAsk};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedJob {
    pub id: String,
    pub workspace_id: String,
    pub created_at: i64,
    pub exclusive: bool,
    pub cpu_millis: Option<u64>,
    pub mem_bytes: Option<u64>,
    pub disk_bytes: u64,
    pub needs_git: bool,
}

/// What is on the node right now (from the controller registry).
#[derive(Debug, Clone, Default)]
pub struct Load {
    pub running_by_workspace: HashMap<String, u32>,
    pub exclusive_running: bool,
}

impl Load {
    pub fn total(&self) -> u32 {
        self.running_by_workspace.values().sum()
    }
}

/// The scheduler's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The job to assign now, if any.
    pub place: Option<String>,
    /// Every queued job's 1-based position and why it waits.
    pub waiting: BTreeMap<String, (usize, Refusal)>,
}

/// Grant cap reached for a workspace.
pub const GRANT_PARALLEL_FULL: &str = "grant_parallel_full";
/// The controller owner paused or is draining the node.
pub const CONTROLLER_PAUSED: &str = "controller_paused";

/// The job asks for more CPU or memory than the node's caps (its policy
/// totals), so it can never start there. Never queued: refused at submit,
/// failed if already queued, and never blocks the jobs behind it.
pub const EXCEEDS_NODE_CAPS: &str = "exceeds_node_caps";

/// `Some` when `job` can never start on the node behind `offer`, whatever
/// else is running: its `--cpus` / `--mem` exceed the node's caps. A cap of
/// 0 means the node didn't say, and never refuses.
pub fn never_fits(offer: &Offer, job: &QueuedJob) -> Option<Refusal> {
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    if let Some(m) = job.mem_bytes.filter(|m| offer.caps.mem_bytes > 0 && *m > offer.caps.mem_bytes) {
        return Some(Refusal::new(
            EXCEEDS_NODE_CAPS,
            format!("the job asks for {:.1} GB of memory; the node allows at most {:.1} GB", gb(m), gb(offer.caps.mem_bytes)),
        ));
    }
    if let Some(c) = job.cpu_millis.filter(|c| offer.caps.cpu_millis > 0 && *c > offer.caps.cpu_millis) {
        return Some(Refusal::new(
            EXCEEDS_NODE_CAPS,
            format!("the job asks for {:.2} CPUs; the node allows at most {:.2}", c as f64 / 1000.0, offer.caps.cpu_millis as f64 / 1000.0),
        ));
    }
    None
}

/// A `local_resource_limit` refusal with the numbers in its message
/// (display only; the code is unchanged): what the job asks for, what is
/// free under the node's caps, and how many jobs run. Other refusals pass
/// through as they are.
pub fn with_numbers(r: Refusal, offer: &Offer, job: &QueuedJob, running: u32) -> Refusal {
    if r.code != refusal::LOCAL_RESOURCE_LIMIT {
        return r;
    }
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    let cpus = |m: u64| m as f64 / 1000.0;
    let mut asks = Vec::new();
    if let Some(m) = job.mem_bytes {
        asks.push(format!("{:.1} GB", gb(m)));
    }
    if let Some(c) = job.cpu_millis {
        asks.push(format!("{} CPUs", fmt_num(cpus(c))));
    }
    Refusal::new(
        refusal::LOCAL_RESOURCE_LIMIT,
        format!(
            "asks {}; {:.1} GB of {:.0} GB and {} of {} CPUs free; {running} running",
            asks.join(" and "),
            gb(offer.free.mem_bytes),
            gb(offer.caps.mem_bytes),
            fmt_num(cpus(offer.free.cpu_millis)),
            fmt_num(cpus(offer.caps.cpu_millis)),
        ),
    )
}

/// 3 → "3", 2.5 → "2.5" (one decimal at most).
fn fmt_num(x: f64) -> String {
    let s = format!("{x:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

fn ask(j: &QueuedJob) -> JobAsk<'_> {
    JobAsk {
        workspace_id: &j.workspace_id,
        cpu_millis: j.cpu_millis,
        mem_bytes: j.mem_bytes,
        disk_bytes: j.disk_bytes,
        exclusive: j.exclusive,
        needs_git: j.needs_git,
    }
}

/// Decide one placement.
///
/// - `queue`: queued jobs on this node (any order).
/// - `last_workspace`: who got the previous placement on this node.
/// - `grant_max_parallel`: the grant cap per workspace.
/// - `offer`: the node's last offer (`None` = offline).
/// - `controller_pause`: `Some("paused"|"draining")` when the controller
///   owner stopped new assignments.
pub fn decide(
    queue: &[QueuedJob],
    last_workspace: Option<&str>,
    load: &Load,
    grant_max_parallel: &dyn Fn(&str) -> u32,
    offer: Option<&Offer>,
    controller_pause: Option<&str>,
) -> Decision {
    let mut jobs: Vec<&QueuedJob> = queue.iter().collect();
    jobs.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
    let position: HashMap<&str, usize> = jobs.iter().enumerate().map(|(i, j)| (j.id.as_str(), i + 1)).collect();
    let mut waiting: BTreeMap<String, (usize, Refusal)> = BTreeMap::new();
    let wait_all = |why: Refusal, waiting: &mut BTreeMap<String, (usize, Refusal)>| {
        for j in &jobs {
            waiting.insert(j.id.clone(), (position[j.id.as_str()], why.clone()));
        }
    };
    if jobs.is_empty() {
        return Decision { place: None, waiting };
    }
    if let Some(p) = controller_pause {
        wait_all(Refusal::new(CONTROLLER_PAUSED, format!("the server's owner {p} this node")), &mut waiting);
        return Decision { place: None, waiting };
    }
    let Some(offer) = offer else {
        wait_all(Refusal::new("node_offline", "the node is offline; the job starts when it reconnects"), &mut waiting);
        return Decision { place: None, waiting };
    };
    // A job over the node's caps can never start: report it (the daemon
    // fails it) and leave it out, so it never holds the jobs behind it.
    jobs.retain(|j| match never_fits(offer, j) {
        Some(r) => {
            waiting.insert(j.id.clone(), (position[j.id.as_str()], r));
            false
        }
        None => true,
    });
    if jobs.is_empty() {
        return Decision { place: None, waiting };
    }

    // Workspace rotation over a STABLE cyclic order (sorted ids), starting
    // just after the last-served workspace, so every workspace with work
    // waits at most (workspaces − 1) placements.
    let mut order: Vec<&str> = jobs.iter().map(|j| j.workspace_id.as_str()).collect();
    order.sort_unstable();
    order.dedup();
    if let Some(last) = last_workspace {
        let start = order.iter().position(|w| *w > last).unwrap_or(0);
        order.rotate_left(start);
    }

    // The oldest job overall is exclusive: hold everything else until it runs.
    let oldest = jobs[0];
    let mut place = None;
    if oldest.exclusive {
        let r = refusal::offer_refusal(offer, load.total(), load.exclusive_running, &ask(oldest));
        match r {
            None if within_grant(oldest, load, grant_max_parallel) => place = Some(oldest.id.clone()),
            None => {
                waiting.insert(oldest.id.clone(), (1, grant_full()));
            }
            Some(r) => {
                waiting.insert(oldest.id.clone(), (1, with_numbers(r, offer, oldest, load.total())));
            }
        }
        for j in jobs.iter().skip(1) {
            waiting.insert(
                j.id.clone(),
                (position[j.id.as_str()], Refusal::new(refusal::WAITING_FOR_IDLE, "an older exclusive job goes first")),
            );
        }
        return Decision { place, waiting };
    }

    for ws in &order {
        // FIFO within the workspace: only its head may start.
        let Some(head) = jobs.iter().find(|j| j.workspace_id == *ws) else { continue };
        let refused = refusal::offer_refusal(offer, load.total(), load.exclusive_running, &ask(head));
        let verdict = match refused {
            Some(r) => Err(with_numbers(r, offer, head, load.total())),
            None if !within_grant(head, load, grant_max_parallel) => Err(grant_full()),
            None => Ok(()),
        };
        match verdict {
            Ok(()) if place.is_none() => place = Some(head.id.clone()),
            Ok(()) => {
                waiting.insert(head.id.clone(), (position[head.id.as_str()], Refusal::new("next_in_turn", "next in turn")));
            }
            Err(r) => {
                waiting.insert(head.id.clone(), (position[head.id.as_str()], r));
            }
        }
    }
    for j in &jobs {
        if Some(&j.id) != place.as_ref() && !waiting.contains_key(&j.id) {
            waiting.insert(
                j.id.clone(),
                (position[j.id.as_str()], Refusal::new("behind_older_job", "an older job from this workspace goes first")),
            );
        }
    }
    Decision { place, waiting }
}

fn within_grant(j: &QueuedJob, load: &Load, cap: &dyn Fn(&str) -> u32) -> bool {
    load.running_by_workspace.get(&j.workspace_id).copied().unwrap_or(0) < cap(&j.workspace_id).max(1)
}

fn grant_full() -> Refusal {
    Refusal::new(GRANT_PARALLEL_FULL, "this workspace already runs as many jobs on the node as its grant allows")
}

/// `--any`: among candidate nodes whose offer takes the job now, prefer
/// the one with a warm slot for `project_key` at `commit`, then any slot
/// for the project, then the most free CPU. Pure.
pub fn pick_any<'a>(
    candidates: &'a [(String, Offer, Load)],
    project_key: Option<&str>,
    commit: Option<&str>,
    job: &QueuedJob,
) -> Option<&'a str> {
    let mut best: Option<(&str, (u8, u64))> = None;
    for (id, offer, load) in candidates {
        if refusal::offer_refusal(offer, load.total(), load.exclusive_running, &ask(job)).is_some() {
            continue;
        }
        let score = (warmth(offer, project_key, commit), offer.free.cpu_millis);
        if best.is_none_or(|(_, b)| score > b) {
            best = Some((id.as_str(), score));
        }
    }
    best.map(|(id, _)| id)
}

/// `--any` when no candidate takes the job now: the node to queue it on.
/// Only a node whose caps can fit the job (see [`never_fits`]) — a job
/// queued where it can never start would hold every later job from its
/// workspace there. Warmest first, then the biggest CPU cap. `None` = no
/// candidate could ever run it. Pure.
pub fn pick_any_queue<'a>(
    candidates: &'a [(String, Offer, Load)],
    project_key: Option<&str>,
    commit: Option<&str>,
    job: &QueuedJob,
) -> Option<&'a str> {
    let mut best: Option<(&str, (u8, u64))> = None;
    for (id, offer, _) in candidates {
        if never_fits(offer, job).is_some() {
            continue;
        }
        let score = (warmth(offer, project_key, commit), offer.caps.cpu_millis);
        if best.is_none_or(|(_, b)| score > b) {
            best = Some((id.as_str(), score));
        }
    }
    best.map(|(id, _)| id)
}

/// 2 = an idle slot for the project at `commit`, 1 = an idle slot for the
/// project, 0 = none.
fn warmth(offer: &Offer, project_key: Option<&str>, commit: Option<&str>) -> u8 {
    let Some(pk) = project_key else { return 0 };
    let slots: Vec<_> = offer.slots.iter().filter(|s| s.project_key == pk && !s.busy).collect();
    if commit.is_some() && slots.iter().any(|s| s.last_sha.as_deref() == commit) {
        2
    } else if !slots.is_empty() {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::proto::frames::{Availability, Caps, Control, Free, SlotInfo};

    fn offer(parallel: u32) -> Offer {
        Offer {
            revision: 1,
            control: Control::Active,
            control_by: None,
            control_at: None,
            availability: Availability { ok: true, reasons: vec![] },
            caps: Caps { cpu_millis: 8000, mem_bytes: 16 << 30, disk_budget_bytes: 100 << 30, hard: true },
            free: Free { cpu_millis: 8000, mem_bytes: 16 << 30, disk_free_bytes: 500 << 30 },
            slots: vec![],
            running: 0,
            parallel_max: parallel,
            refusal: None,
            labels: BTreeMap::new(),
            tools: BTreeMap::from([("git".into(), "2".into())]),
            protocol: crate::compute::proto::PROTOCOL,
            boot_id: "b".into(),
            ledger_id: "l".into(),
            node_version: "0.1.0".into(),
            features: Vec::new(),
        }
    }

    fn job(id: &str, ws: &str, at: i64) -> QueuedJob {
        QueuedJob { id: id.into(), workspace_id: ws.into(), created_at: at, exclusive: false, cpu_millis: None, mem_bytes: None, disk_bytes: 0, needs_git: true }
    }

    fn cap1(_: &str) -> u32 {
        1
    }

    #[test]
    fn fifo_within_a_workspace_and_round_robin_across() {
        let q = vec![job("a1", "A", 1), job("a2", "A", 2), job("b1", "B", 3)];
        let d = decide(&q, None, &Load::default(), &cap1, Some(&offer(4)), None);
        assert_eq!(d.place.as_deref(), Some("a1"));
        // A just got a placement → B is next even though a2 is older.
        let q2 = vec![job("a2", "A", 2), job("b1", "B", 3)];
        let d = decide(&q2, Some("A"), &Load::default(), &cap1, Some(&offer(4)), None);
        assert_eq!(d.place.as_deref(), Some("b1"));
        assert_eq!(d.waiting["a2"].1.code, "next_in_turn");
    }

    #[test]
    fn caps_and_reasons() {
        let q = vec![job("a1", "A", 1), job("b1", "B", 2)];
        let mut load = Load::default();
        load.running_by_workspace.insert("A".into(), 1);
        // Node parallel 2, A at its grant cap of 1 → B goes.
        let d = decide(&q, None, &load, &cap1, Some(&offer(2)), None);
        assert_eq!(d.place.as_deref(), Some("b1"));
        assert_eq!(d.waiting["a1"].1.code, GRANT_PARALLEL_FULL);
        // Node full.
        load.running_by_workspace.insert("B".into(), 1);
        let d = decide(&q, None, &load, &|_| 5, Some(&offer(2)), None);
        assert_eq!(d.place, None);
        assert_eq!(d.waiting["a1"].1.code, refusal::PARALLEL_FULL);
        assert_eq!(d.waiting["b1"].0, 2, "position is global FIFO order");
    }

    #[test]
    fn resource_limit_message_has_numbers() {
        let mut big = job("a1", "A", 1);
        big.mem_bytes = Some(6 << 30);
        big.cpu_millis = Some(4000);
        let mut o = offer(4);
        o.caps = Caps { cpu_millis: 6000, mem_bytes: 12 << 30, disk_budget_bytes: 100 << 30, hard: true };
        o.free = Free { cpu_millis: 3000, mem_bytes: (42 << 30) / 10, disk_free_bytes: 500 << 30 };
        let mut load = Load::default();
        load.running_by_workspace.insert("B".into(), 1);
        let d = decide(&[big.clone()], None, &load, &|_| 4, Some(&o), None);
        assert_eq!(d.place, None);
        let (pos, r) = &d.waiting["a1"];
        assert_eq!((*pos, r.code.as_str()), (1, refusal::LOCAL_RESOURCE_LIMIT));
        assert_eq!(r.message, "asks 6.0 GB and 4 CPUs; 4.2 GB of 12 GB and 3 of 6 CPUs free; 1 running");
        // Memory only: just that in the ask.
        let mut mem_only = job("m1", "A", 1);
        mem_only.mem_bytes = Some(5 << 30);
        let r = with_numbers(Refusal::new(refusal::LOCAL_RESOURCE_LIMIT, "x"), &o, &mem_only, 0);
        assert!(r.message.starts_with("asks 5.0 GB; "), "{}", r.message);
        // Other refusals are untouched.
        let r = with_numbers(Refusal::new(refusal::PARALLEL_FULL, "full"), &o, &big, 1);
        assert_eq!((r.code.as_str(), r.message.as_str()), (refusal::PARALLEL_FULL, "full"));
    }

    #[test]
    fn offline_paused_and_node_refusals_pass_through() {
        let q = vec![job("a1", "A", 1)];
        assert_eq!(decide(&q, None, &Load::default(), &cap1, None, None).waiting["a1"].1.code, "node_offline");
        assert_eq!(
            decide(&q, None, &Load::default(), &cap1, Some(&offer(1)), Some("paused")).waiting["a1"].1.code,
            CONTROLLER_PAUSED
        );
        let mut o = offer(1);
        o.control = Control::Draining;
        assert_eq!(decide(&q, None, &Load::default(), &cap1, Some(&o), None).waiting["a1"].1.code, refusal::LOCAL_DRAINING);
        let mut o = offer(1);
        o.refusal = Some(Refusal::new(refusal::FOREIGN_LOCK, "sew"));
        assert_eq!(decide(&q, None, &Load::default(), &cap1, Some(&o), None).waiting["a1"].1.code, refusal::FOREIGN_LOCK);
    }

    #[test]
    fn oldest_exclusive_job_holds_the_node_until_idle() {
        let mut ex = job("x", "A", 1);
        ex.exclusive = true;
        let q = vec![ex.clone(), job("b1", "B", 2)];
        let mut load = Load::default();
        load.running_by_workspace.insert("C".into(), 1);
        let d = decide(&q, None, &load, &|_| 5, Some(&offer(4)), None);
        assert_eq!(d.place, None, "nothing starts while the exclusive job waits for idle");
        assert_eq!(d.waiting["x"].1.code, refusal::WAITING_FOR_IDLE);
        assert_eq!(d.waiting["b1"].1.code, refusal::WAITING_FOR_IDLE);
        let d = decide(&q, None, &Load::default(), &|_| 5, Some(&offer(4)), None);
        assert_eq!(d.place.as_deref(), Some("x"));
        // While it runs, nothing else starts.
        let mut running = Load::default();
        running.running_by_workspace.insert("A".into(), 1);
        running.exclusive_running = true;
        let d = decide(&[job("b1", "B", 2)], Some("A"), &running, &|_| 5, Some(&offer(4)), None);
        assert_eq!(d.place, None);
        assert_eq!(d.waiting["b1"].1.code, refusal::EXCLUSIVE_RUNNING);
    }

    /// No starvation: 10k random submissions across 5 workspaces with one
    /// slot; every workspace's wait (in placements) stays bounded by the
    /// number of active workspaces, and every job eventually runs in FIFO
    /// order within its workspace.
    #[test]
    fn no_starvation_over_random_submissions() {
        let mut seed: u64 = 0x9e3779b97f4a7c15;
        let mut rand = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut queue: Vec<QueuedJob> = Vec::new();
        let mut last: Option<String> = None;
        let mut placed_order: HashMap<String, Vec<i64>> = HashMap::new();
        let mut since_served: HashMap<String, u32> = HashMap::new();
        let mut t = 0i64;
        let mut submitted = 0;
        let mut placed = 0;
        while submitted < 10_000 || !queue.is_empty() {
            // Up to 3 submissions per step, skewed toward workspace 0.
            if submitted < 10_000 {
                for _ in 0..(rand() % 4) {
                    let w = match rand() % 10 {
                        0..=5 => 0,
                        n => (n - 5) as usize,
                    };
                    t += 1;
                    queue.push(job(&format!("j{t}"), &format!("w{w}"), t));
                    submitted += 1;
                }
            }
            let d = decide(&queue, last.as_deref(), &Load::default(), &cap1, Some(&offer(1)), None);
            let Some(id) = d.place else { continue };
            let i = queue.iter().position(|j| j.id == id).unwrap();
            let j = queue.remove(i);
            placed += 1;
            placed_order.entry(j.workspace_id.clone()).or_default().push(j.created_at);
            // Count placements each workspace with queued work sat through.
            let waiting_ws: std::collections::HashSet<String> = queue.iter().map(|q| q.workspace_id.clone()).collect();
            for w in 0..5 {
                let w = format!("w{w}");
                if w == j.workspace_id || !waiting_ws.contains(&w) {
                    since_served.insert(w, 0);
                } else {
                    *since_served.entry(w).or_insert(0) += 1;
                }
            }
            for (ws, waited) in &since_served {
                assert!(*waited <= 4, "workspace {ws} waited {waited} placements with work queued");
            }
            last = Some(j.workspace_id);
        }
        assert_eq!(placed, submitted);
        for (ws, order) in placed_order {
            assert!(order.windows(2).all(|w| w[0] < w[1]), "{ws} not FIFO");
        }
    }

    #[test]
    fn any_prefers_warm_slot_then_free_cpu() {
        let mut warm = offer(2);
        warm.free.cpu_millis = 1000;
        warm.slots = vec![SlotInfo { project_key: "pk".into(), slot: 1, busy: false, last_sha: Some("c1".into()) }];
        let mut big = offer(2);
        big.free.cpu_millis = 32000;
        let mut paused = offer(2);
        paused.control = Control::Paused;
        let cands = vec![
            ("big".to_string(), big, Load::default()),
            ("warm".to_string(), warm, Load::default()),
            ("paused".to_string(), paused, Load::default()),
        ];
        let j = job("j", "A", 1);
        assert_eq!(pick_any(&cands, Some("pk"), Some("c1"), &j), Some("warm"));
        assert_eq!(pick_any(&cands, None, None, &j), Some("big"));
        assert_eq!(pick_any(&cands[2..], None, None, &j), None);
    }

    fn small(mem_gb: u64) -> Offer {
        let mut o = offer(2);
        o.caps.mem_bytes = mem_gb << 30;
        o.free.mem_bytes = mem_gb << 30;
        o
    }

    fn mem_job(id: &str, ws: &str, at: i64, gb: u64) -> QueuedJob {
        let mut j = job(id, ws, at);
        j.mem_bytes = Some(gb << 30);
        j
    }

    /// k2n1-m4 (2 GB caps) took a --mem 4G job from --any and held it at
    /// position 1 forever; every later job from the workspace waited behind it.
    #[test]
    fn never_fits_compares_against_the_node_caps_not_free() {
        assert_eq!(never_fits(&small(2), &mem_job("j", "A", 1, 4)).expect("over caps").code, EXCEEDS_NODE_CAPS);
        assert_eq!(never_fits(&small(6), &mem_job("j", "A", 1, 4)), None);
        // Busy (no free memory now) but within caps: it can start later.
        let mut busy = small(6);
        busy.free.mem_bytes = 0;
        assert_eq!(never_fits(&busy, &mem_job("j", "A", 1, 4)), None);
        let mut cpu = job("j", "A", 1);
        cpu.cpu_millis = Some(16_000);
        assert_eq!(never_fits(&offer(2), &cpu).expect("over cpu caps").code, EXCEEDS_NODE_CAPS);
        // A node that reports no caps never refuses on them.
        let mut unknown = small(0);
        unknown.caps.cpu_millis = 0;
        assert_eq!(never_fits(&unknown, &cpu), None);
        assert_eq!(never_fits(&unknown, &mem_job("j", "A", 1, 4)), None);
    }

    #[test]
    fn any_queues_only_where_the_job_can_ever_fit() {
        let mut busy6 = small(6);
        busy6.free.mem_bytes = 1 << 30;
        let cands = vec![
            ("k2n1-m4".to_string(), small(2), Load::default()),
            ("k2n2-m1".to_string(), busy6, Load::default()),
        ];
        let j = mem_job("j", "A", 1, 4);
        assert_eq!(pick_any(&cands, None, None, &j), None, "nothing takes it now");
        assert_eq!(pick_any_queue(&cands, None, None, &j), Some("k2n2-m1"));
        assert_eq!(pick_any_queue(&cands[..1], None, None, &j), None, "no node could ever run it");
        // A job that fits now still goes where it runs now.
        let fits = vec![("k2n1-m4".to_string(), small(2), Load::default()), ("k2n3-m1".to_string(), small(6), Load::default())];
        assert_eq!(pick_any(&fits, None, None, &j), Some("k2n3-m1"));
    }

    #[test]
    fn a_job_over_the_node_caps_never_blocks_the_jobs_behind_it() {
        let q = vec![mem_job("big", "A", 1, 4), job("a2", "A", 2), job("a3", "A", 3)];
        let d = decide(&q, None, &Load::default(), &cap1, Some(&small(2)), None);
        assert_eq!(d.place.as_deref(), Some("a2"));
        let (pos, why) = d.waiting.get("big").expect("big is reported");
        assert_eq!((*pos, why.code.as_str()), (1, EXCEEDS_NODE_CAPS));
        assert_eq!(d.waiting.get("a3").expect("a3 waits").1.code, "behind_older_job");
        // Same for an exclusive job over the caps.
        let mut ex = mem_job("ex", "A", 1, 4);
        ex.exclusive = true;
        let d = decide(&[ex, job("b1", "B", 2)], None, &Load::default(), &cap1, Some(&small(2)), None);
        assert_eq!(d.place.as_deref(), Some("b1"));
        assert_eq!(d.waiting.get("ex").expect("ex is reported").1.code, EXCEEDS_NODE_CAPS);
    }
}
