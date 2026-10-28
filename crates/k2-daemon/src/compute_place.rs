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

use k2_core::compute::proto::frames::{self, JobState, Offer, Plan, FEATURE_STALL};
use k2_core::compute::proto::share::{self as pshare, ShareView};
use k2_core::compute::proto::tiers::{Tier as JobTier, FEATURE_TIERS};
use k2_core::compute::proto::{needs, refusal, results};
use k2_core::compute::retry::{self, NodeHealth};
use k2_core::compute::share::{self, pending};
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
    /// Compute secret-env (SE6): the vault secret ids the job gets. A node
    /// must offer `secret-env` and be covered by a grant for each.
    pub secret_ids: Vec<String>,
    /// The job's tier (prd-compute-job-tiers-v1). `--any --tier ci` keeps
    /// only nodes where this workspace may send CI: the share's `gate` and
    /// a grant with `allow_ci` (or an owner/Admin caller) (T12).
    pub tier: JobTier,
    /// `--cache NAME` (protocol 6): the node must offer `job-cache`.
    pub cache: bool,
    /// `--with` (protocol 6): the node must offer `multi-repo`.
    pub multi_repo: bool,
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
            secret_ids: j.ctl.secrets.iter().map(|p| p.secret_id.clone()).collect(),
            tier: j.tier(),
            cache: plan.cache.is_some(),
            multi_repo: !plan.with.is_empty(),
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
            tier: self.tier,
        }
    }
}

/// May `w`'s workspace send CI to the node behind `offer` with `grant`?
/// The node owner's share must allow `gate` (no share: this server owns
/// the node), and the grant must allow CI unless the caller is the owner
/// or an Admin (prd-compute-job-tiers-v1 §5.1, T12).
pub fn ci_allowed(admin: bool, grant: Option<&GrantRow>, offer: Option<&Offer>) -> bool {
    share::share_allows_ci(offer) && (admin || grant.is_some_and(|g| g.allow_ci))
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
    /// This server's share, when the node is shared (protocol 3).
    pub fn shared_view(&self) -> Option<&ShareView> {
        share::is_shared(&self.offer).then(|| self.offer.share.as_ref()).flatten()
    }

    /// The node's ETA for this ask (shared nodes with a quote; `None` =
    /// unknown, as every solo node).
    pub fn eta(&self, w: &Want) -> Option<u64> {
        self.shared_view().and_then(|v| share::eta_for(v, w.cpu_millis, w.mem_bytes))
    }

    /// Starts the job now (as far as the last offer and our registry say;
    /// on a shared node, as far as its quote says).
    pub fn starts_now(&self, w: &Want) -> bool {
        if let Some(v) = self.shared_view() {
            let cap = self.grant.as_ref().map(|g| g.max_parallel.max(1) as u32).unwrap_or(1);
            return self.online
                && share::starts_now(v, w.cpu_millis, w.mem_bytes)
                && self.load.running_by_workspace.get(&w.ws_id).copied().unwrap_or(0) < cap;
        }
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
        if let Some(v) = self.shared_view() {
            // A shared node queues: busy is never a refusal there; only a
            // paused node or share is.
            return v.state == pshare::ServerStatus::Active && self.offer.control == frames::Control::Active;
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
    if !w.secret_ids.is_empty() {
        f.push(k2_core::compute::proto::secret::FEATURE_SECRET_ENV);
    }
    if w.cache {
        f.push(k2_core::compute::proto::cache::FEATURE_JOB_CACHE);
    }
    if w.multi_repo {
        f.push(frames::FEATURE_MULTI_REPO);
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
        // Over the node's caps (--cpus/--mem): it would never start there
        // (compute DX slice A: never queued where it can't ever fit).
        if scheduler::never_fits(&offer, &w.probe()).is_some() {
            continue;
        }
        // Secret-env (SE6): only nodes a grant covers for every secret
        // (`*` covers only nodes this server owns, Q2).
        if !w.secret_ids.is_empty() && !crate::compute_secrets::covers_all(&w.ws_id, &n, &w.secret_ids) {
            continue;
        }
        // `--any --tier ci`: only where this workspace may send CI (T12).
        if w.tier == JobTier::Ci && !ci_allowed(w.admin, grant.as_ref(), Some(&offer)) {
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
    let mut best: Option<(&Cand, (Tier, bool, i64, u8, i64, u64))> = None;
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
        // Busy everywhere: the shared node quoting the lowest ETA first
        // (§6 step 3). Solo nodes have no ETA, so among them nothing
        // changes; a quoted ETA beats an unknown one.
        let eta = match (tier, c.eta(w)) {
            (Tier::Busy, Some(e)) => -(e.min(i64::MAX as u64 / 4) as i64),
            (Tier::Busy, None) => -(i64::MAX / 2),
            _ => 0,
        };
        let score = (tier, !prefer_not.contains(&c.node.id), eta, warmth, busy, tie);
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
    if features.iter().any(|f| f == pshare::FEATURE_QUEUE) {
        p.ws_max_running = Some(grant.map(|g| g.max_parallel.max(1) as u32).unwrap_or(1));
    } else {
        // Only to nodes offering `queue`: an older node would drop them and
        // the plan's digest wouldn't match (§10.2).
        strip_queue_fields(&mut p);
    }
    // The tier only to nodes offering `tiers` (T11), the same rule as the
    // queue fields; `set_queue_fields` then sets the job's own tier.
    p.tier = offers_tiers(features).then(|| base.tier_or_class());
    p
}

/// Does a node with these features take `Plan.tier` (protocol 5)?
pub fn offers_tiers(features: &[String]) -> bool {
    features.iter().any(|f| f == FEATURE_TIERS)
}

/// The stored plan fitted to what the node understands right now (T11):
/// a tier only for a node offering `tiers`, the protocol-3 queue fields
/// only for one offering `queue`, a stall timeout only for one offering
/// `stall`. A plan stored for a newer node and sent to one rolled back to
/// an older k2-node would otherwise carry fields that node drops, so its
/// digest wouldn't match and it would fail `plan_mismatch`; a node updated
/// since gets the tier. `Some(plan)` when it changed; the caller rewrites
/// the queued row (JSON and digest) before sending.
pub fn fit_plan(plan: &Plan, j: &JobRow, features: &[String]) -> Option<Plan> {
    let has = |f: &str| features.iter().any(|x| x == f);
    let mut p = plan.clone();
    p.tier = offers_tiers(features).then(|| j.tier());
    if !has(pshare::FEATURE_QUEUE) {
        strip_queue_fields(&mut p);
    }
    if !has(FEATURE_STALL) {
        p.stall_secs = None;
    }
    (p != *plan).then_some(p)
}

/// The protocol 3 plan fields a node keeps its queue by (§10.2), for a
/// node offering `queue`: from the job row (class, estimate) and its
/// session.
pub fn set_queue_fields(p: &mut Plan, j: &JobRow) {
    p.class = Some(pshare::JobClass::parse(&j.class).unwrap_or_default());
    if p.tier.is_some() {
        p.tier = Some(j.tier());
    }
    p.session_id = j.session_id.clone();
    p.est_secs = j.est_secs.filter(|e| *e > 0).map(|e| e as u64);
}

pub fn strip_queue_fields(p: &mut Plan) {
    p.class = None;
    p.session_id = None;
    p.est_secs = None;
    p.est_p90_secs = None;
    p.ws_max_running = None;
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
        // A shared node (protocol 3): still only busy.
        "any_hold",
        pshare::codes::TENANT_QUEUE_FULL,
        pshare::codes::FAIR_SHARE_WAIT,
        // Secret-env SE26: another workspace's job runs as the same user.
        k2_core::compute::proto::secret::codes::ISOLATION_WAIT,
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
    let mut new_plan = plan_for_node(&plan, &j.ctl, &c.node, c.grant.as_ref(), &c.offer.features);
    if share::queue_on(&c.offer) {
        set_queue_fields(&mut new_plan, j);
    }
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
        if j.node_queued && j.reason.as_deref() != Some("cancel_requested") {
            // V56: it never started there. A new generation (the node, if
            // it comes back, drops the stale row: V47), no retry used, no
            // health strike; `--any` moves now, a named job waits for the
            // node. A no-force cancel or a queue timeout that was waiting
            // for the node's answer ends here instead.
            requeue_lost(&j, n, mins, now);
            continue;
        }
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let detail = format!("{} has been offline for {mins} min; if it comes back, it is told to stop this attempt", n.name);
            // A job someone asked to cancel ends `cancelled` (what was asked).
            if j.reason.as_deref() == Some("cancel_requested") {
                let detail = format!("cancelled as asked; {detail}");
                let _ = store::set_job_state(&conn, &j.id, JobState::Cancelled, Some("cancelled"), Some(&detail), now);
            } else {
                let _ = store::set_job_state(&conn, &j.id, JobState::Interrupted, Some("node_lost"), Some(&detail), now);
            }
            store::record_event(&conn, "controller", "job_lost", Some(&n.id), Some(&j.workspace_id), Some(&j.id), serde_json::json!({"offlineMins": mins}));
        }
        // A job someone asked to cancel is never retried.
        if j.reason.as_deref() == Some("cancel_requested") || !after_attempt(&j.id) {
            compute_ws::finalize(&j.id);
        }
    }

    // Jobs a shared node holds in its queue: deadlines, then moves (V46,
    // V71: only after the node confirms a withdraw).
    sweep_node_queued(now, awake, &nodes);

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
                    // Compare-and-set (as cancel and assign, slice A): a job
                    // placed or cancelled since the list was read keeps that.
                    if store::set_job_state_if(&conn, &j.id, "queued", JobState::Failed, Some("queue_timeout"), Some(&detail), now) != Ok(true) {
                        continue;
                    }
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

/// A lost node held this job in its queue (never started): requeue it
/// under a new generation with no retry and no strike (V56).
fn requeue_lost(j: &JobRow, n: &NodeRow, mins: i64, now: i64) {
    match j.reason.as_deref() {
        Some(pending::CANCEL) => {
            // The user's cancel: the node can't start it any more (V47).
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = store::set_job_state(&conn, &j.id, JobState::Cancelled, Some("cancelled"), None, now);
                store::clear_node_queued(&conn, &j.id);
            }
            compute_ws::finalize(&j.id);
            return;
        }
        Some(pending::TIMEOUT) => {
            fail_queue_timeout(j, &n.name, now);
            return;
        }
        _ => {}
    }
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if store::requeue_new_generation(&conn, &j.id, j.generation, "node_lost") != Ok(true) {
            return;
        }
        store::record_event(
            &conn,
            "controller",
            "job_requeue",
            Some(&n.id),
            Some(&j.workspace_id),
            Some(&j.id),
            serde_json::json!({"why": "node_lost", "nodeQueued": true, "offlineMins": mins, "newGeneration": j.generation + 1}),
        );
        store::job_by_id(&conn, &j.id)
    };
    if let Some(r) = row.filter(|r| r.ctl.any.is_some()) {
        try_move(&r, &[], true, "node_lost");
    }
    compute_ws::notify_change();
}

/// `--queue-timeout` on a job that waited in a node's queue.
fn fail_queue_timeout(j: &JobRow, node_name: &str, now: i64) {
    let limit = j.ctl.queue_timeout_secs.unwrap_or(0);
    let detail = format!(
        "not started within {} (it waited in {node_name}'s queue{})",
        compute::message::duration(limit),
        j.node_blocked_by.as_deref().map(|b| format!(": {}", share::blocked_words(b))).unwrap_or_default()
    );
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = store::set_job_state(&conn, &j.id, JobState::Failed, Some("queue_timeout"), Some(&detail), now);
        store::clear_node_queued(&conn, &j.id);
        store::record_event(&conn, "controller", "job_queue_timeout", Some(&j.node_id), Some(&j.workspace_id), Some(&j.id), serde_json::json!({"nodeQueued": true}));
    }
    compute_ws::finalize(&j.id);
}

/// Node-queued jobs (shared nodes): the queue timeout withdraws, then
/// fails (§6 "Never-stuck"); an `--any` job moves to a node that can start
/// it now only after the node confirms the withdraw (V46), at most twice
/// and not again within 5 min (V71). A job the node already started stays
/// (the node answers its real state, V57).
fn sweep_node_queued(now: i64, awake: i64, nodes: &HashMap<String, NodeRow>) {
    let t = retry::tuning();
    let held = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_queued_jobs(&conn)
    };
    for j in held {
        let Some(n) = nodes.get(&j.node_id) else { continue };
        let online = compute_ws::is_online(&n.id);
        if pending::any(j.reason.as_deref()) || j.reason.as_deref() == Some("cancel_requested") {
            continue; // waiting for the node's answer
        }
        let since = if j.ctl.queued_at > 0 { j.ctl.queued_at } else { j.created_at };
        if let Some(limit) = j.ctl.queue_timeout_secs.filter(|l| *l > 0) {
            if now - since >= limit && now - awake >= 2 * t.replace_after_secs {
                if !online {
                    // The node can't start it while we're not attached, and
                    // drops it on the next attach (V47): fail it now.
                    fail_queue_timeout(&j, &n.name, now);
                } else {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    store::set_job_reason(&conn, &j.id, Some(pending::TIMEOUT), j.detail.as_deref());
                    drop(conn);
                    compute_ws::send_cancel_if_not_started(&n.id, &j.id, j.generation, frames::CANCEL_WITHDRAW);
                }
                continue;
            }
        }
        if j.ctl.any.is_none() || !online || !share::may_withdraw(j.withdraws, j.withdrawn_at, now) {
            continue;
        }
        let Some(plan) = j.plan() else { continue };
        let w = Want::of_job(&j, &plan);
        let cands = candidates(&w, std::slice::from_ref(&j.node_id), now);
        let pk = plan.src.as_ref().map(|s| s.project_key.clone());
        let commit = plan.src.as_ref().map(|s| s.commit.clone());
        if !matches!(choose(&cands, &w, &[], pk.as_deref(), commit.as_deref()), Some((_, Tier::Now))) {
            continue;
        }
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::set_job_reason(&conn, &j.id, Some(pending::WITHDRAW), None);
            store::note_withdraw(&conn, &j.id, now);
            store::record_event(&conn, "controller", "withdraw_asked", Some(&n.id), Some(&j.workspace_id), Some(&j.id), serde_json::json!({"generation": j.generation, "withdraws": j.withdraws + 1}));
        }
        if !compute_ws::send_cancel_if_not_started(&n.id, &j.id, j.generation, frames::CANCEL_WITHDRAW) {
            // Not sent (the node just dropped): no withdraw in flight.
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::set_job_reason(&conn, &j.id, Some(pshare::codes::NODE_QUEUED), None);
        }
    }
}

/// A node's offer changed this server's share (tenant side notes, §4.1,
/// §4.3): it became shared, or its share was paused, revoked or ended.
pub fn share_changed(before: &NodeRow, after: Option<&ShareView>) {
    let Some(v) = after else { return };
    let was = before.share.as_ref();
    let until = v.expires_in_secs.map(|s| format!(" until {}", clock_day(compute::now() + s))).unwrap_or_default();
    if v.shared && !was.is_some_and(|w| w.shared) {
        let text = match (v.role, v.owner_label.as_deref()) {
            (pshare::Role::Tenant, Some(owner)) => format!("{} is now shared with this server by {owner}{until}.", before.name),
            _ => format!("{} is now shared with {} server(s){until}.", before.name, v.servers.saturating_sub(1)),
        };
        crate::compute_admin::share_note(&before.id, &format!("shared:{}", v.servers), &text);
    }
    if was.is_some_and(|w| w.state != v.state) {
        let text = match v.state {
            pshare::ServerStatus::Paused => format!("{}: the node's owner paused this server's share; queued jobs wait, running ones finish.", before.name),
            pshare::ServerStatus::Revoked => format!("{}: the node's owner revoked this server; its queued jobs are cancelled.", before.name),
            pshare::ServerStatus::Expired => format!("{}: this server's share has ended; its queued jobs are cancelled.", before.name),
            pshare::ServerStatus::Active => format!("{}: this server's share is active again.", before.name),
            _ => return,
        };
        crate::compute_admin::share_note(&before.id, &format!("state:{}:{}", v.state.as_str(), compute::now() / 60), &text);
    }
}

fn clock_day(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0).map(|d| d.format("%b %-d").to_string()).unwrap_or_default()
}

/// Tenant side: a note 24 h before this server's share on a node ends,
/// and one when it ends (§4.3). The node judges by its own clock; this
/// server only counts down from the duration the node sent.
fn sweep_share_expiry(now: i64) {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_nodes(&conn, false)
    };
    for n in rows {
        let (Some(v), Some(at)) = (n.share.as_ref(), n.share_at) else { continue };
        let Some(left) = v.expires_in_secs else { continue };
        let ends = at + left;
        if now >= ends {
            crate::compute_admin::share_note(&n.id, &format!("expired:{ends}"), &format!("{}: this server's share has ended; jobs there are cancelled.", n.name));
        } else if ends - now <= share::EXPIRY_NOTE_SECS {
            crate::compute_admin::share_note(&n.id, &format!("expiring:{ends}"), &format!("{}: this server's share ends {} (in under 24 h).", n.name, clock_day(ends)));
        }
    }
}

/// The share housekeeping the compute ticker runs (headless, §12).
pub fn sweep_shares(now: i64) {
    sweep_share_expiry(now);
    crate::compute_admin::servers_tick(now);
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

    /// Job tiers T11, T17 (mixed versions): a tiered plan sent to a node
    /// without `tiers` (an older k2-node, or one rolled back to 0.46.4)
    /// loses its tier and gets the untiered digest back, so that node takes
    /// it; a node offering `tiers` gets the job's own tier; a plan that
    /// already fits is left alone.
    #[test]
    fn the_tier_follows_the_nodes_features() {
        let plan: Plan = serde_json::from_value(serde_json::json!({
            "job_id": "j1", "node_id": "n1", "workspace_id": "w1", "workspace_label": "k2", "requested_by": "owner",
            "argv": ["true"], "env": {}, "cwd": null,
            "limits": {"max_secs": 60, "cpu_millis": null, "mem_bytes": null, "disk_bytes": 1, "log_cap_bytes": 1},
            "src": null, "exclusive": false, "created_at": 1, "class": "interactive"
        }))
        .expect("a plan");
        let job = JobRow { id: "j1".into(), tier: "priority".into(), ..Default::default() };
        let new_node = vec![FEATURE_TIERS.to_string(), pshare::FEATURE_QUEUE.to_string()];
        let old_node = vec![pshare::FEATURE_QUEUE.to_string()];
        assert_eq!(fit_plan(&plan, &job, &old_node), None, "no tier, an old node: as it is");
        let tiered = fit_plan(&plan, &job, &new_node).expect("a node with tiers gets the tier");
        assert_eq!(tiered.tier, Some(JobTier::Priority));
        assert_ne!(tiered.digest().unwrap(), plan.digest().unwrap());
        assert_eq!(fit_plan(&tiered, &job, &new_node), None, "already fits");
        let back = fit_plan(&tiered, &job, &old_node).expect("a rolled-back node loses the tier");
        assert_eq!((back.tier, back.digest().unwrap()), (None, plan.digest().unwrap()), "the untiered digest, byte for byte");
        assert!(offers_tiers(&new_node) && !offers_tiers(&old_node));
        // A v0.46.2 node (no `queue`, no `stall`): every field it doesn't
        // know goes, so its digest is the one it computes (the tiers
        // rollback drill: `plan_mismatch` before this).
        let mut full = tiered.clone();
        full.ws_max_running = Some(2);
        full.stall_secs = Some(600);
        let bare = fit_plan(&full, &job, &[]).expect("fitted");
        assert_eq!((bare.tier, bare.class, bare.ws_max_running, bare.stall_secs), (None, None, None, None));
        let mut want = plan.clone();
        want.class = None;
        assert_eq!(bare.digest().unwrap(), want.digest().unwrap(), "what a v0.46.2 node computes");
        assert_eq!(fit_plan(&bare, &job, &[]), None);
    }
}
