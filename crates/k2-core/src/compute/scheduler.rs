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
                waiting.insert(oldest.id.clone(), (1, r));
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
            Some(r) => Err(r),
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

/// `--any`: among candidate nodes whose offer takes the job, prefer the
/// one with a warm slot for `project_key` at `commit`, then any slot for
/// the project, then the most free CPU. Pure.
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
        let warmth = match project_key {
            Some(pk) => {
                let slots: Vec<_> = offer.slots.iter().filter(|s| s.project_key == pk && !s.busy).collect();
                if commit.is_some() && slots.iter().any(|s| s.last_sha.as_deref() == commit) {
                    2
                } else if !slots.is_empty() {
                    1
                } else {
                    0
                }
            }
            None => 0,
        };
        let score = (warmth, offer.free.cpu_millis);
        if best.is_none_or(|(_, b)| score > b) {
            best = Some((id.as_str(), score));
        }
    }
    best.map(|(id, _)| id)
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
}
