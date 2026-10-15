//! Never stuck, smart retries (research-distributed-compute-gaps G3/G4).
//!
//! - **Placement for `--any`**: one candidate list (grant, features,
//!   os/arch/labels, set-aside nodes, controller pause, queue caps) and
//!   one choice rule: a node that can start the job now (warm slot first,
//!   then free CPU), else the least-loaded online node, else (at submit
//!   only) the most recently seen offline one. [`plan_for_node`] rebuilds
//!   the plan for whichever node it lands on.
//! - **The sweeper** (the compute ticker, every 5 s): queued jobs past
//!   their queue timeout fail loudly (`queue_timeout`); queued `--any`
//!   jobs leave a node that went offline, is paused, draining or set
//!   aside, or move to an idle node while theirs is busy (late binding);
//!   running jobs on a node offline for `node_lost_secs` end as
//!   `interrupted/node_lost`.
//! - **Retries**: after each attempt [`after_attempt`] retries
//!   infrastructure failures only (never a non-zero exit, a timeout, a
//!   stall or a cancel), with backoff, on another node when the job is
//!   `--any`. A node with `quarantine_after` infra failures in a row is
//!   set aside for `quarantine_secs`.
//!
//! The controller is often a laptop: after it sleeps, nobody is "lost"
//! or "timed out" until nodes had a chance to reconnect (`awake_since`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

use k2_core::compute::proto::frames::{JobState, Offer, Plan, FEATURE_STALL};
use k2_core::compute::proto::{needs, refusal, results};
use k2_core::compute::retry::{self, NodeHealth};
use k2_core::compute::store::{self, AnyFilter, AttemptNote, GrantRow, JobCtl, JobRow, NodeRow};
use k2_core::compute::{self as compute, scheduler};

use crate::compute_ws;

// ── controller awake time ────────────────────────────────────────────

static AWAKE_SINCE: AtomicI64 = AtomicI64::new(0);
static LAST_TICK: AtomicI64 = AtomicI64::new(0);

/// Record a ticker pass. A gap of over a minute means this machine slept
/// (or the daemon just started): grace periods restart from now.
pub fn note_tick(now: i64) {
    let last = LAST_TICK.swap(now, Ordering::Relaxed);
    if last == 0 || now - last > 60 {
        AWAKE_SINCE.store(now, Ordering::Relaxed);
    }
}

fn awake_since(now: i64) -> i64 {
    match AWAKE_SINCE.load(Ordering::Relaxed) {
        0 => now,
        t => t,
    }
}

/// Since when a node has been offline, as far as this controller can tell.
fn offline_since(n: &NodeRow, now: i64) -> i64 {
    n.last_seen.unwrap_or(n.enrolled_at).max(awake_since(now))
}

// ── node health (set aside) ──────────────────────────────────────────

static HEALTH: Mutex<Option<HashMap<String, NodeHealth>>> = Mutex::new(None);

fn with_health<T>(f: impl FnOnce(&mut HashMap<String, NodeHealth>) -> T) -> T {
    let mut g = HEALTH.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

/// Until when `node_id` is set aside (consecutive infra failures).
pub fn set_aside_until(node_id: &str, now: i64) -> Option<i64> {
    with_health(|m| m.get(node_id).and_then(|h| h.until).filter(|u| *u > now))
}

/// Owner/admin: put a set-aside node back at once.
pub fn clear_set_aside(node_id: &str) {
    with_health(|m| m.remove(node_id));
}

// ── candidates ───────────────────────────────────────────────────────

/// What a job asks of a node.
#[derive(Debug, Clone)]
pub struct Want {
    pub ws_id: String,
    pub admin: bool,
    pub filter: AnyFilter,
    pub needs: Vec<String>,
    pub artifacts: bool,
    pub exclusive: bool,
    pub needs_git: bool,
    pub cpu_millis: Option<u64>,
    pub mem_bytes: Option<u64>,
    /// An explicit `--stall-timeout` (the node must enforce it).
    pub stall: bool,
}

impl Want {
    pub fn of_job(j: &JobRow, plan: &Plan) -> Self {
        Want {
            ws_id: j.workspace_id.clone(),
            admin: j.ctl.admin,
            filter: j.ctl.any.clone().unwrap_or_default(),
            needs: plan.needs.clone(),
            artifacts: !plan.artifacts.is_empty(),
            exclusive: plan.exclusive,
            needs_git: plan.src.is_some(),
            cpu_millis: plan.limits.cpu_millis,
            mem_bytes: plan.limits.mem_bytes,
            stall: j.ctl.stall_secs.is_some_and(|s| s > 0),
        }
    }

    fn probe(&self) -> scheduler::QueuedJob {
        scheduler::QueuedJob {
            id: "probe".into(),
            workspace_id: self.ws_id.clone(),
            created_at: compute::now(),
            exclusive: self.exclusive,
            cpu_millis: self.cpu_millis,
            mem_bytes: self.mem_bytes,
            disk_bytes: 0,
            needs_git: self.needs_git,
        }
    }
}

/// One node that could take the job.
#[derive(Debug, Clone)]
pub struct Cand {
    pub node: NodeRow,
    pub online: bool,
    pub offer: Offer,
    pub load: scheduler::Load,
    /// Jobs queued on it (all workspaces).
    pub queued: usize,
    pub grant: Option<GrantRow>,
}

impl Cand {
    /// Starts the job now (as far as the last offer and our registry say).
    pub fn starts_now(&self, w: &Want) -> bool {
        let probe = w.probe();
        let ask = refusal::JobAsk {
            workspace_id: &w.ws_id,
            cpu_millis: probe.cpu_millis,
            mem_bytes: probe.mem_bytes,
            disk_bytes: 0,
            exclusive: probe.exclusive,
            needs_git: probe.needs_git,
        };
        let cap = self.grant.as_ref().map(|g| g.max_parallel.max(1) as u32).unwrap_or(1);
        self.online
            && self.queued == 0
            && refusal::offer_refusal(&self.offer, self.load.total(), self.load.exclusive_running, &ask).is_none()
            && self.load.running_by_workspace.get(&w.ws_id).copied().unwrap_or(0) < cap
    }

    /// Online and only busy (no pause, window, lock, disk or upgrade refusal).
    fn usable(&self) -> bool {
        if !self.online {
            return false;
        }
        let probe = refusal::JobAsk { workspace_id: "", cpu_millis: None, mem_bytes: None, disk_bytes: 0, exclusive: false, needs_git: false };
        // With nothing running, only a node-wide refusal is left.
        refusal::offer_refusal(&self.offer, 0, false, &probe).is_none()
    }
}

/// Features a plan needs from the node's k2-node.
pub fn required_features(w: &Want) -> Vec<&'static str> {
    let mut f = Vec::new();
    if !w.needs.is_empty() {
        f.push(needs::FEATURE_NEEDS);
    }
    if w.artifacts {
        f.push(results::FEATURE_ARTIFACTS);
    }
    if w.stall {
        f.push(FEATURE_STALL);
    }
    f
}

/// The stall timeout a plan for a node with `features` carries: the asked
/// one, else the default — only on nodes that enforce it, and only when
/// shorter than the wall-clock limit.
pub fn stall_for(ctl: &JobCtl, features: &[String], max_secs: u64) -> Option<u64> {
    let s = match ctl.stall_secs {
        Some(0) => return None,
        Some(n) => n,
        None => retry::tuning().stall_secs,
    };
    (s > 0 && (s as u64) < max_secs && features.iter().any(|f| f == FEATURE_STALL)).then_some(s as u64)
}

/// Every node that could take the job, except `exclude`.
pub fn candidates(w: &Want, exclude: &[String], now: i64) -> Vec<Cand> {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_nodes(&conn, false)
    };
    let feats = required_features(w);
    let mut out = Vec::new();
    for n in rows.into_iter().filter(|n| n.state == "active" && n.controller_pause.is_none()) {
        if exclude.contains(&n.id) || set_aside_until(&n.id, now).is_some() {
            continue;
        }
        let (grant, queued_ws, queued_all, load) = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let g = store::grant(&conn, &n.id, &w.ws_id);
            let (q, load) = compute_ws::queued_view(&conn, &n.id);
            (g, store::queued_count(&conn, &n.id, &w.ws_id), q.len(), load)
        };
        if grant.is_none() && !w.admin {
            continue;
        }
        if grant.as_ref().is_some_and(|g| queued_ws >= g.max_queued) {
            continue;
        }
        let (online, _, live) = compute_ws::live_view(&n.id);
        let Some(offer) = live.filter(|_| online).or_else(|| n.last_offer.clone()) else { continue };
        if feats.iter().any(|f| !offer.features.iter().any(|x| x == f)) {
            continue;
        }
        let label = |k: &str| offer.labels.get(k).map(String::as_str);
        if w.filter.os.as_deref().is_some_and(|o| label("os") != Some(o)) || w.filter.arch.as_deref().is_some_and(|a| label("arch") != Some(a)) {
            continue;
        }
        if w.filter.labels.iter().any(|(k, x)| label(k) != Some(x.as_str())) {
            continue;
        }
        out.push(Cand { node: n, online, offer, load, queued: queued_all, grant });
    }
    out
}

/// How good a pick is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Offline (submit only: it starts when the node is back or moves).
    Offline,
    /// Online and only busy: the job waits its turn there.
    Busy,
    /// Starts now.
    Now,
}

/// Pick one candidate. `prefer_not` (sibling shards, failed nodes) loses
/// ties. `project_key`/`commit` prefer a warm slot.
pub fn choose<'a>(cands: &'a [Cand], w: &Want, prefer_not: &[String], project_key: Option<&str>, commit: Option<&str>) -> Option<(&'a Cand, Tier)> {
    let mut best: Option<(&Cand, (Tier, bool, u8, i64, u64))> = None;
    for c in cands {
        let tier = if c.starts_now(w) {
            Tier::Now
        } else if c.usable() {
            Tier::Busy
        } else if !c.online {
            Tier::Offline
        } else {
            continue; // online but paused/draining/unavailable: never a target
        };
        let warmth = project_key
            .map(|pk| {
                let slots: Vec<_> = c.offer.slots.iter().filter(|s| s.project_key == pk && !s.busy).collect();
                if commit.is_some() && slots.iter().any(|s| s.last_sha.as_deref() == commit) {
                    2
                } else {
                    u8::from(!slots.is_empty())
                }
            })
            .unwrap_or(0);
        let busy = -((c.load.total() as i64) + c.queued as i64);
        let fresh = c.node.last_seen.unwrap_or(0) as u64;
        let tie = match tier {
            Tier::Offline => fresh,
            _ => c.offer.free.cpu_millis,
        };
        let score = (tier, !prefer_not.contains(&c.node.id), warmth, busy, tie);
        if best.as_ref().is_none_or(|(_, b)| score > *b) {
            best = Some((c, score));
        }
    }
    best.map(|(c, s)| (c, s.0))
}

/// The plan for `node`: the base plan's argv/env/source/needs/artifacts,
/// with this node's id, this node's grant limits and (when it enforces
/// one) the stall timeout.
pub fn plan_for_node(base: &Plan, ctl: &JobCtl, node: &NodeRow, grant: Option<&GrantRow>, features: &[String]) -> Plan {
    let max_secs = grant.map(|g| g.max_job_secs).unwrap_or(compute::DEFAULT_MAX_JOB_SECS);
    let disk_gb = grant.map(|g| g.max_disk_gb).unwrap_or(compute::DEFAULT_MAX_DISK_GB);
    let mut p = base.clone();
    p.node_id = node.id.clone();
    p.limits.max_secs = ctl.timeout_secs.unwrap_or(max_secs).clamp(1, max_secs.max(1)) as u64;
    p.limits.disk_bytes = (disk_gb.max(1) as u64) << 30;
    p.stall_secs = stall_for(ctl, features, p.limits.max_secs);
    p
}

/// Why a queued job may leave its node, and whether that's a hard reason
/// (leave for any online node) or only busy (leave only for a node that
/// starts it now).
fn leave_reason(j: &JobRow, node: Option<&NodeRow>, decisions: &mut HashMap<String, scheduler::Decision>, now: i64) -> Option<(&'static str, bool)> {
    let t = retry::tuning();
    let Some(node) = node else { return Some(("node_removed", true)) };
    if node.state != "active" {
        return Some(("node_removed", true));
    }
    if set_aside_until(&node.id, now).is_some() {
        return Some(("node_set_aside", true));
    }
    if node.controller_pause.is_some() {
        return Some((scheduler::CONTROLLER_PAUSED, true));
    }
    if !compute_ws::is_online(&node.id) {
        return (now - offline_since(node, now) >= t.replace_after_secs).then_some(("node_offline", true));
    }
    let d = decisions.entry(node.id.clone()).or_insert_with(|| compute_ws::decision_for(&node.id));
    let (_, why) = d.waiting.get(&j.id)?;
    let busy = [
        refusal::PARALLEL_FULL,
        refusal::EXCLUSIVE_RUNNING,
        refusal::WAITING_FOR_IDLE,
        refusal::LOCAL_RESOURCE_LIMIT,
        scheduler::GRANT_PARALLEL_FULL,
        "next_in_turn",
        "behind_older_job",
        "retry_backoff",
    ];
    if busy.contains(&why.code.as_str()) {
        Some(("node_busy", false))
    } else {
        Some(("node_refused", true))
    }
}

/// Move a queued job to another node now, if one fits. `hard`: any usable
/// online node; else only one that starts it now. Returns the new node.
pub fn try_move(j: &JobRow, avoid: &[String], hard: bool, why: &str) -> Option<String> {
    let plan = j.plan()?;
    let now = compute::now();
    let w = Want::of_job(j, &plan);
    let cands = candidates(&w, std::slice::from_ref(&j.node_id), now);
    let pk = plan.src.as_ref().map(|s| s.project_key.clone());
    let commit = plan.src.as_ref().map(|s| s.commit.clone());
    let (c, tier) = choose(&cands, &w, avoid, pk.as_deref(), commit.as_deref())?;
    let ok = match tier {
        Tier::Now => true,
        Tier::Busy => hard,
        Tier::Offline => false,
    };
    if !ok {
        return None;
    }
    let new_plan = plan_for_node(&plan, &j.ctl, &c.node, c.grant.as_ref(), &c.offer.features);
    let moved = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let moved = store::move_queued_job(&conn, &j.id, &new_plan, &j.ctl, &format!("moved_{why}")).unwrap_or(false);
        if moved {
            store::record_event(
                &conn,
                "controller",
                "job_move",
                Some(&c.node.id),
                Some(&j.workspace_id),
                Some(&j.id),
                serde_json::json!({"from": j.node_id, "to": c.node.id, "why": why}),
            );
        }
        moved
    };
    if !moved {
        return None;
    }
    compute_ws::kick(&c.node.id);
    compute_ws::notify_change();
    Some(c.node.id.clone())
}

// ── after an attempt ─────────────────────────────────────────────────

/// An attempt reached a terminal state (the row says which). Count it for
/// the node, then retry it when it was an infrastructure failure and the
/// job has retries left. True = requeued (no wake message yet).
pub fn after_attempt(job_id: &str) -> bool {
    let now = compute::now();
    let t = retry::tuning();
    let Some(j) = ({
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::job_by_id(&conn, job_id)
    }) else {
        return false;
    };
    let Some(state) = j.job_state() else { return false };
    let reason = j.reason.clone();
    let set_aside = with_health(|m| m.entry(j.node_id.clone()).or_default().record(state, reason.as_deref(), now, &t));
    if set_aside {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::record_event(
            &conn,
            "controller",
            "node_set_aside",
            Some(&j.node_id),
            None,
            Some(&j.id),
            serde_json::json!({"for_secs": t.quarantine_secs, "after": t.quarantine_after, "last": reason}),
        );
        drop(conn);
        k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, serde_json::json!({"compute": "node_set_aside"}));
    }
    if !retry::is_infra(state, reason.as_deref()) {
        return false;
    }
    // A missing package or tool won't appear on the same node by retrying.
    if j.ctl.any.is_none() && retry::retry_elsewhere_only(reason.as_deref()) {
        return false;
    }
    let mut ctl = j.ctl.clone();
    // Rows from before retries were options: one retry for an interruption.
    let allowed = if ctl == JobCtl::default() && j.retry_interrupted && state == JobState::Interrupted { 1 } else { ctl.retries };
    ctl.failed_attempts.push(AttemptNote { attempt: j.attempt, node_id: j.node_id.clone(), state: j.state.clone(), reason: reason.clone(), at: now });
    let db = k2_core::db::shared();
    let conn = db.lock();
    if ctl.retries_used >= allowed {
        let _ = store::set_job_ctl(&conn, &j.id, &ctl);
        if allowed > 0 {
            let names: HashMap<String, String> = store::list_nodes(&conn, true).into_iter().map(|n| (n.id, n.name)).collect();
            let tried: Vec<String> = ctl
                .failed_attempts
                .iter()
                .map(|a| format!("{} ({}{})", names.get(&a.node_id).cloned().unwrap_or_else(|| a.node_id.clone()), a.state, a.reason.as_deref().map(|r| format!("/{r}")).unwrap_or_default()))
                .collect();
            // The reason stays the last attempt's (it says what to fix);
            // the detail lists every attempt.
            let detail = format!("retries exhausted after {} attempt(s): {}", ctl.failed_attempts.len(), tried.join(", "));
            store::set_job_reason(&conn, &j.id, reason.as_deref(), Some(&detail));
        }
        return false;
    }
    ctl.retries_used += 1;
    let seed = j.id.bytes().fold(j.attempt as u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64));
    let delay = retry::backoff_secs(ctl.retries_used, &t, seed);
    ctl.not_before = (delay > 0).then_some(now + delay);
    ctl.queued_at = now + delay;
    if store::requeue_new_attempt(&conn, &j.id, "retry_backoff").is_err() {
        return false;
    }
    let _ = store::set_job_ctl(&conn, &j.id, &ctl);
    store::record_event(
        &conn,
        "controller",
        "job_retry",
        Some(&j.node_id),
        Some(&j.workspace_id),
        Some(&j.id),
        serde_json::json!({"after": j.state, "reason": reason, "retry": ctl.retries_used, "of": allowed, "delaySecs": delay}),
    );
    let row = store::job_by_id(&conn, &j.id);
    drop(conn);
    // Prefer another node (never the ones that just failed it).
    if let Some(r) = row.filter(|r| r.ctl.any.is_some()) {
        let avoid: Vec<String> = ctl.failed_attempts.iter().map(|a| a.node_id.clone()).collect();
        if try_move(&r, &avoid, true, "retry").is_none() {
            compute_ws::kick(&r.node_id);
        }
    } else {
        compute_ws::kick(&j.node_id);
    }
    compute_ws::notify_change();
    true
}

// ── the sweeper ──────────────────────────────────────────────────────

/// One pass (the compute ticker). Blocking.
pub fn sweep(now: i64) {
    let t = retry::tuning();
    let awake = awake_since(now);
    let nodes: HashMap<String, NodeRow> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_nodes(&conn, true).into_iter().map(|n| (n.id.clone(), n)).collect()
    };

    // Lost nodes: running work on a node gone for `node_lost_secs`.
    let active = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::active_jobs(&conn)
    };
    for j in active {
        let Some(n) = nodes.get(&j.node_id) else { continue };
        if compute_ws::is_online(&n.id) || now - offline_since(n, now) < t.node_lost_secs {
            continue;
        }
        let mins = (now - offline_since(n, now)) / 60;
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let detail = format!("{} has been offline for {mins} min; if it comes back, it is told to stop this attempt", n.name);
            let _ = store::set_job_state(&conn, &j.id, JobState::Interrupted, Some("node_lost"), Some(&detail), now);
            store::record_event(&conn, "controller", "job_lost", Some(&n.id), Some(&j.workspace_id), Some(&j.id), serde_json::json!({"offlineMins": mins}));
        }
        if !after_attempt(&j.id) {
            compute_ws::finalize(&j.id);
        }
    }

    // Queued work: deadlines, then moves.
    let queued = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::queued_jobs(&conn)
    };
    let mut decisions: HashMap<String, scheduler::Decision> = HashMap::new();
    let mut changed = false;
    for j in queued {
        let since = if j.ctl.queued_at > 0 { j.ctl.queued_at } else { j.created_at };
        if let Some(limit) = j.ctl.queue_timeout_secs.filter(|l| *l > 0) {
            // After this machine wakes, nodes get a moment to reconnect first.
            if now - since >= limit && now - awake >= 2 * t.replace_after_secs {
                let last = j.reason.clone().unwrap_or_else(|| "waiting".into());
                let node = nodes.get(&j.node_id).map(|n| n.name.clone()).unwrap_or_default();
                let detail = format!("not started within {} (last wait on {node}: {last}{})", compute::message::duration(limit), j.detail.as_deref().map(|d| format!(": {d}")).unwrap_or_default());
                {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    let _ = store::set_job_state(&conn, &j.id, JobState::Failed, Some("queue_timeout"), Some(&detail), now);
                    store::record_event(&conn, "controller", "job_queue_timeout", Some(&j.node_id), Some(&j.workspace_id), Some(&j.id), serde_json::json!({"waitedSecs": now - since}));
                }
                compute_ws::finalize(&j.id);
                changed = true;
                continue;
            }
        }
        if j.ctl.not_before.is_some_and(|nb| nb > now) || j.ctl.any.is_none() {
            continue;
        }
        if let Some((why, hard)) = leave_reason(&j, nodes.get(&j.node_id), &mut decisions, now) {
            if try_move(&j, &[], hard, why).is_some() {
                decisions.clear();
                changed = true;
            }
        }
    }
    if changed {
        compute_ws::notify_change();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn awake_since_restarts_after_a_long_gap() {
        note_tick(1_000);
        note_tick(1_005);
        assert_eq!(awake_since(1_005), 1_000, "a normal tick keeps the start");
        note_tick(5_000);
        assert_eq!(awake_since(5_000), 5_000, "a gap over a minute = slept");
    }
}
