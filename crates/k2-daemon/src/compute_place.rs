//! Never stuck, smart retries (research-distributed-compute-gaps G3/G4).
//!
//! - **Auto-placement** (no node named, or `--any`;
//!   prd-compute-auto-placement-v1): one candidate list (granted nodes
//!   only, features, os/arch/labels, set-aside nodes, controller pause,
//!   queue caps, `--timeout` within the grant, the share's limits) and one
//!   choice rule, switched by `computePlacement`: v2 (default) ranks nodes
//!   that start the job now by load band, a warm slot worth one band, and
//!   spreads bursts; v1 is the 0.46.5 order (warm slot first, then free
//!   CPU). Else the shortest quoted wait, else (at submit only) the most
//!   recently seen offline node. Every placement is recorded on the job
//!   (`JobCtl.placement`). [`plan_for_node`] rebuilds the plan for
//!   whichever node it lands on.
//! - **The sweeper** (the compute ticker, every 5 s): queued jobs past
//!   their queue timeout fail loudly (`queue_timeout`); queued `--any`
//!   jobs leave a node that went offline, is paused, draining or set
//!   aside, or move to an idle node while theirs is busy (late binding;
//!   on a shared node through a withdraw, with the target's place held
//!   while the node answers and a back-off only after a bounce);
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

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
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

pub use crate::compute_gpu_ask::{gpu_mem_needs_update_hint, gpu_refusal, gpu_skip};
use crate::compute_ws;

/// The native tools' need states (android-ndk, cargo-ndk).
#[path = "compute_place_native.rs"]
mod native;

/// The line already waiting on a node: its slots and the wait (0.46.15).
#[path = "compute_place_backlog.rs"]
mod backlog;

/// Why an auto job waits where it is; a shared node's own slots.
#[path = "compute_why_not.rs"]
pub mod why_not;

/// Placement's reads stay cheap over a long job history (0312).
#[cfg(test)]
#[path = "compute_place_lock_tests.rs"]
mod lock_tests;

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
    /// Auto-placement (no node named, or `--any`; prd-compute-auto-placement-v1
    /// D1, CP-A3): only nodes this workspace holds a grant on, whoever the
    /// caller is. The owner/Admin bypass is for a named node only.
    pub auto: bool,
    pub filter: AnyFilter,
    pub needs: Vec<String>,
    pub artifacts: bool,
    pub exclusive: bool,
    pub needs_git: bool,
    pub cpu_millis: Option<u64>,
    pub mem_bytes: Option<u64>,
    /// No `--mem`: `mem_bytes` is the default, clamped to a share's per-job
    /// memory at submit (so a share's memory limit never rules a node out).
    pub mem_defaulted: bool,
    /// An explicit `--timeout` (seconds). A node whose grant allows less
    /// is not a candidate (F10, CP-A4); without one the job gets each
    /// grant's own max, which always fits.
    pub timeout_secs: Option<i64>,
    /// An explicit `--stall-timeout` (the node must enforce it).
    pub stall: bool,
    /// Compute secret-env (SE6): the vault secret ids the job gets. A node
    /// must offer `secret-env` and be covered by a grant for each.
    pub secret_ids: Vec<String>,
    /// The job's tier (prd-compute-job-tiers-v1). `--any --tier ci` keeps
    /// only nodes where this workspace may send CI: the share's `gate` and
    /// a grant with `allow_ci` (or an owner/Admin caller) (T12).
    pub tier: JobTier,
    /// `--cache NAME` (protocol 6): the node must offer `job-cache`; the
    /// node that ran this workspace's last job with it ranks first (CE-6).
    pub cache: Option<String>,
    /// `--with` (protocol 6): the node must offer `multi-repo`.
    pub multi_repo: bool,
    /// Protocol 9: the plan asks for receipt v2 (`Plan.receipt_v = 2`);
    /// the node must offer `receipt-v2`.
    pub receipt_v2: bool,
    /// GPU jobs (0.46.11, `--gpu`): only a node offering `gpu` to this
    /// server with a GPU of that vendor open to it, and a grant that says
    /// `--gpu` (or an owner/Admin caller).
    pub gpu: Option<k2_core::compute::proto::gpu::GpuAsk>,
    /// GPU jobs S6 (`--gpu-mem`, bytes, only with `gpu`): the job's GPU
    /// memory budget. Only nodes offering `gpu-mem` take an explicit one;
    /// on a unified GPU it (default 2G) counts against the node's memory
    /// beside `--mem` ([`Want::mem_on`]).
    pub gpu_mem: Option<u64>,
    /// 0.46.15 (`--network none`, `--netns-dummy`): only nodes offering
    /// what it needs ([`crate::compute_job_network`]).
    pub network: Option<k2_core::compute::proto::jobnet::JobNet>,
    /// 0.46.18 (`--userns`): the job may make user namespaces. Only a node
    /// this server owns, offering `job-userns`, where the workspace's grant
    /// says `--userns` ([`crate::compute_userns_ask`]); no owner/Admin
    /// bypass.
    pub userns: bool,
}

impl Want {
    /// GPU jobs S6: the job's memory on the node behind `offer`: its
    /// `--mem`, plus its GPU memory when the GPU it would get there is
    /// unified (an APU's GPU buffers are system RAM outside `--mem`). The
    /// node counts the same (`k2-node`'s `plan_mem`).
    pub fn mem_on(&self, offer: &Offer) -> Option<u64> {
        mem_with_gpu(self.mem_bytes, self.gpu.as_ref(), self.gpu_mem, Some(offer))
    }

    /// [`Want::probe`] on the node behind `offer` (its memory per
    /// [`Want::mem_on`]).
    fn probe_on(&self, offer: &Offer) -> scheduler::QueuedJob {
        scheduler::QueuedJob { mem_bytes: self.mem_on(offer), ..self.probe() }
    }

    pub fn of_job(j: &JobRow, plan: &Plan) -> Self {
        Want {
            ws_id: j.workspace_id.clone(),
            admin: j.ctl.admin,
            auto: j.ctl.any.is_some(),
            filter: j.ctl.any.clone().unwrap_or_default(),
            needs: plan.needs.clone(),
            artifacts: !plan.artifacts.is_empty(),
            exclusive: plan.exclusive,
            needs_git: plan.src.is_some(),
            cpu_millis: plan.limits.cpu_millis,
            mem_bytes: plan.limits.mem_bytes,
            mem_defaulted: j.ctl.mem_defaulted,
            timeout_secs: j.ctl.timeout_secs,
            stall: j.ctl.stall_secs.is_some_and(|s| s > 0),
            secret_ids: j.ctl.secrets.iter().map(|p| p.secret_id.clone()).collect(),
            tier: j.tier(),
            cache: plan.cache.clone(),
            multi_repo: !plan.with.is_empty(),
            receipt_v2: plan.receipt_v.is_some(),
            gpu: plan.gpu,
            gpu_mem: plan.gpu_mem,
            network: plan.network.clone(),
            userns: plan.userns,
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
            tier: self.tier, submitter: None,
            gpus: self.gpu.map_or(0, |g| g.count),
        }
    }

    /// The command needs a system package (`--needs sys:…` or a profile).
    fn wants_sys(&self) -> bool {
        self.needs.iter().any(|n| n.starts_with("sys:") || n.starts_with("profile:"))
    }
}

/// GPU jobs S6: `mem` (a job's `--mem`) on the node behind `offer`, plus
/// its GPU memory (`gpu_mem`, default 2G) when the GPU `gpu` would get
/// there is unified. No offer: as if discrete (nothing added).
pub fn mem_with_gpu(mem: Option<u64>, gpu: Option<&k2_core::compute::proto::gpu::GpuAsk>, gpu_mem: Option<u64>, offer: Option<&Offer>) -> Option<u64> {
    use k2_core::compute::proto::gpu as pgpu;
    let Some(ask) = gpu else { return mem };
    let unified = offer.and_then(|o| o.host.as_ref()).is_some_and(|h| pgpu::unified_for(&h.gpus, ask));
    pgpu::with_gpu_mem(mem, pgpu::counted_bytes(Some(ask), gpu_mem, unified))
}

/// [`mem_with_gpu`] for a queued job's plan.
pub fn plan_mem_on(plan: &Plan, offer: Option<&Offer>) -> Option<u64> {
    mem_with_gpu(plan.limits.mem_bytes, plan.gpu.as_ref(), plan.gpu_mem, offer)
}

/// May `w`'s workspace send CI to the node behind `offer` with `grant`?
/// The node owner's share must allow `gate` (no share: this server owns
/// the node), and the grant must allow CI unless the caller is the owner
/// or an Admin (prd-compute-job-tiers-v1 §5.1, T12).
pub fn ci_allowed(admin: bool, grant: Option<&GrantRow>, offer: Option<&Offer>) -> bool {
    share::share_allows_ci(offer) && (admin || grant.is_some_and(|g| g.allow_ci))
}

/// F10 (CP-A4): an explicit `--timeout` within the grant's max per job.
pub fn timeout_fits(w: &Want, grant: Option<&GrantRow>) -> bool {
    let max = grant.map(|g| g.max_job_secs).unwrap_or(compute::DEFAULT_MAX_JOB_SECS);
    w.timeout_secs.is_none_or(|t| t <= max)
}

/// F11 (CP-A5): on a shared node, this server's share allows the ask
/// (per-job memory, CPUs and time, exclusive, class) — the same check
/// submit makes, so auto never picks a node submit would then refuse.
pub fn share_fits(w: &Want, grant: Option<&GrantRow>, offer: &Offer) -> bool {
    let Some(v) = offer.share.as_ref().filter(|_| share::is_shared(offer)) else { return true };
    let max_secs = w.timeout_secs.unwrap_or_else(|| grant.map(|g| g.max_job_secs).unwrap_or(compute::DEFAULT_MAX_JOB_SECS));
    let ask = share::ShareAsk {
        cpu_millis: w.cpu_millis,
        mem_bytes: w.mem_bytes.filter(|_| !w.mem_defaulted),
        max_secs: max_secs.max(0) as u64,
        exclusive: w.exclusive,
        class: share::clamp_class(&v.limits, w.tier.class()),
    };
    share::share_refusal(&v.limits, &ask).is_none()
}

// ── pending asks and recent placements (in memory, CP-A7) ────────────

/// Memory and CPU of jobs sent to a node that its offers don't show yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pending {
    pub mem_bytes: u64,
    pub cpu_millis: u64,
    pub jobs: u32,
    /// Of `jobs`, the `--any` mover's holds ([`note_hold`]).
    pub holds: u32,
}

#[derive(Debug, Clone)]
struct PendAsk {
    job: String,
    mem: u64,
    cpu: u64,
    at: i64,
    /// The node's offer revision when the job was sent.
    rev: u64,
}

/// A job sent to a node stays pending this long at most.
pub const PENDING_SECS: i64 = 60;
/// Placements this recent spread a burst (§4.1 step 5).
pub const RECENT_SECS: i64 = 30;

static PENDING: Mutex<Option<HashMap<String, Vec<PendAsk>>>> = Mutex::new(None);
static RECENT: Mutex<Option<HashMap<String, Vec<i64>>>> = Mutex::new(None);

fn with_map<V, T>(m: &Mutex<Option<HashMap<String, V>>>, f: impl FnOnce(&mut HashMap<String, V>) -> T) -> T {
    let mut g = m.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

/// A job was assigned (solo) or sent (shared) to `node_id`, whose offer
/// was at revision `rev`: until an offer with a newer revision arrives (or
/// [`PENDING_SECS`] pass), its ask counts against the node's free memory
/// and CPU. Closes the burst race: ten submits in one second all saw the
/// same free memory until the next offer.
pub fn note_pending(node_id: &str, job_id: &str, mem: Option<u64>, cpu: Option<u64>, rev: u64, now: i64) {
    with_map(&PENDING, |m| {
        let v = m.entry(node_id.to_string()).or_default();
        v.retain(|p| p.job != job_id);
        v.push(PendAsk { job: job_id.to_string(), mem: mem.unwrap_or(0), cpu: cpu.unwrap_or(0), at: now, rev });
    });
}

/// The revision a withdraw hold carries: no offer is newer, so only time
/// ([`PENDING_SECS`]) or [`drop_hold`] ends it.
const HOLD_REV: u64 = u64::MAX;

/// The `--any` mover asked a node to give `job_id` back so it can start on
/// `node_id`: hold that node's memory, CPU and a slot for it while the
/// answer travels, so a new submit or another move doesn't take the place
/// first (the job would then bounce back to the end of its old queue).
/// Unlike a sent job's pending ask, a newer offer doesn't drop it.
pub fn note_hold(node_id: &str, job_id: &str, mem: Option<u64>, cpu: Option<u64>, now: i64) {
    note_pending(node_id, job_id, mem, cpu, HOLD_REV, now);
}

/// The job's withdraw hold ends (it moved, bounced, or the withdraw wasn't
/// sent): its place on the target is free again.
pub fn drop_hold(job_id: &str) {
    with_map(&PENDING, |m| m.values_mut().for_each(|v| v.retain(|p| !(p.rev == HOLD_REV && p.job == job_id))));
}

/// Keep only the holds of jobs whose withdraw is still in flight (a node
/// that started the job instead answered its real state, V57).
fn keep_holds(in_flight: &HashSet<String>) {
    with_map(&PENDING, |m| m.values_mut().for_each(|v| v.retain(|p| p.rev != HOLD_REV || in_flight.contains(&p.job))));
}

/// What is pending on `node_id`, given its current offer revision (`None`
/// = no live offer: only time drops entries).
pub fn pending_for(node_id: &str, rev: Option<u64>, now: i64) -> Pending {
    with_map(&PENDING, |m| {
        let Some(v) = m.get_mut(node_id) else { return Pending::default() };
        v.retain(|p| now - p.at < PENDING_SECS && rev.is_none_or(|r| r <= p.rev));
        v.iter().fold(Pending::default(), |a, p| Pending {
            mem_bytes: a.mem_bytes + p.mem,
            cpu_millis: a.cpu_millis + p.cpu,
            jobs: a.jobs + 1,
            holds: a.holds + u32::from(p.rev == HOLD_REV),
        })
    })
}

/// This controller placed a job on `node_id` (node ids and times only, S3).
pub fn note_placed(node_id: &str, now: i64) {
    with_map(&RECENT, |m| m.entry(node_id.to_string()).or_default().push(now));
}

/// Placements on `node_id` in the last [`RECENT_SECS`].
pub fn recent_for(node_id: &str, now: i64) -> u32 {
    with_map(&RECENT, |m| {
        let Some(v) = m.get_mut(node_id) else { return 0 };
        v.retain(|t| now - *t < RECENT_SECS);
        v.len() as u32
    })
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
    /// Asks sent there that its last offer doesn't show yet (CP-A7).
    pub pending: Pending,
    /// This controller's placements there in the last 30 s.
    pub recent: u32,
    /// This workspace's last job with the same `--cache NAME` ran here
    /// (warmth 3, CE-6, CE-A15).
    pub holds_cache: bool,
    /// This server's estimate of the wait there (P2, [`solo_eta`]); on a
    /// shared node only at the workspace's grant limit ([`backlog::est_wait`]).
    pub est_wait: Option<u64>,
    /// Shared node (0.46.8): this server's jobs still waiting HERE for it
    /// (the `any_hold` minute, not yet in the node's queue, so not in its
    /// quote), as seconds of work per slot ([`SoloWaits::held_eta`]). Added
    /// to the quote, so a burst stops piling onto the node with the lowest
    /// quote (11-03: 15 jobs on k2n10 while others had room soon).
    pub held_wait: u64,
    /// B2 (§4.1.6): this server's `trusted` cache heads on the node,
    /// `(project key, head commit)`, from its `lineage_status`.
    pub gen_heads: Vec<(String, String)>,
    /// The workspace whose repo answers ancestry for [`gen_warmth`].
    pub ws_id: String,
}

impl Cand {
    /// This server's share, when the node is shared (protocol 3).
    pub fn shared_view(&self) -> Option<&ShareView> {
        share::is_shared(&self.offer).then(|| self.offer.share.as_ref()).flatten()
    }

    /// The node's ETA for this ask (shared nodes with a quote; `None` =
    /// unknown, as every solo node). Placement v1 ranks by this only.
    pub fn eta(&self, w: &Want) -> Option<u64> {
        self.shared_view().and_then(|v| share::eta_for(v, w.cpu_millis, w.mem_bytes))
    }

    /// The wait for this ask and its basis: the node's quote (`quote`),
    /// else (or when longer) this server's estimate (`estimate`, P2, [`backlog::wait`]).
    pub fn wait_eta(&self, w: &Want) -> Option<(u64, &'static str)> {
        backlog::wait(self, w)
    }

    /// Each need's state here (CE-1), profiles expanded.
    pub fn needs(&self, w: &Want) -> Vec<NeedStatus> {
        need_states(&self.offer, &expand_profiles(&w.needs))
    }

    /// A `sys:` need this node can't install (macOS): ranked last.
    fn unsupported(&self, w: &Want) -> bool {
        self.needs(w).iter().any(|s| s.state == NeedState::UnsupportedOnOs)
    }

    /// Node-wide free memory and CPU: the last offer's (it counts every
    /// server's running jobs) minus the asks pending since (§3).
    pub fn free_now(&self) -> (u64, u64) {
        (self.offer.free.mem_bytes.saturating_sub(self.pending.mem_bytes), self.offer.free.cpu_millis.saturating_sub(self.pending.cpu_millis))
    }

    /// The job's `--mem` (with its GPU memory on a unified GPU, S6) and
    /// `--cpus` fit in [`Cand::free_now`].
    fn fits_free(&self, w: &Want) -> bool {
        let (mem, cpu) = self.free_now();
        w.mem_on(&self.offer).is_none_or(|m| m <= mem) && w.cpu_millis.is_none_or(|c| c <= cpu)
    }

    /// Jobs running there now, as far as anyone here knows (node-wide).
    fn running_now(&self) -> u32 {
        self.offer.running.max(self.load.total()) + self.pending.jobs
    }

    fn grant_cap(&self) -> u32 {
        self.grant.as_ref().map(|g| g.max_parallel.max(1) as u32).unwrap_or(1)
    }

    fn ws_running(&self, w: &Want) -> u32 {
        self.load.running_by_workspace.get(&w.ws_id).copied().unwrap_or(0)
    }

    /// Starts the job now (as far as the last offer, the pending asks and
    /// our registry say; on a shared node, as far as its quote says, and
    /// node-wide free memory and CPU allow, CP-A6).
    pub fn starts_now(&self, w: &Want) -> bool {
        if let Some(v) = self.shared_view() {
            // A10: a shared node's quote doesn't know its GPU is held, so
            // a GPU job also needs a free GPU in the offer. A node its owner
            // paused or drains starts nothing, whatever its quote says
            // (11-19: a job "started now" on a draining node and sat there
            // 3 h): `usable` checks the node's control and this server's
            // share, as a solo node's `offer_refusal` does below.
            return self.usable()
                && share::starts_now(v, w.cpu_millis, w.mem_bytes)
                // The node's own slots, every server's jobs (the offer is newer than a
                // quote: why_not), behind ours held here for it (backlog).
                && self.slot_free_behind_line()
                // Jobs sent (or a mover's hold) since the offer take this
                // server's running slots there too.
                && (self.pending.jobs == 0 || v.running + self.pending.jobs < v.limits.max_running)
                && w.gpu.is_none_or(|g| self.offer.free.gpus >= g.count)
                && self.fits_free(w)
                && !(w.exclusive && self.running_now() > 0)
                && self.ws_running(w) < self.grant_cap();
        }
        let probe = w.probe_on(&self.offer);
        let ask = refusal::JobAsk {
            workspace_id: &w.ws_id,
            cpu_millis: probe.cpu_millis,
            mem_bytes: probe.mem_bytes,
            disk_bytes: 0,
            exclusive: probe.exclusive,
            needs_git: probe.needs_git,
            gpus: probe.gpus,
        };
        self.online
            && self.queued == 0
            && refusal::offer_refusal(&self.offer, self.load.total() + self.pending.jobs, self.load.exclusive_running, &ask).is_none()
            && self.fits_free(w)
            && self.ws_running(w) < self.grant_cap()
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
        let probe = refusal::JobAsk { workspace_id: "", cpu_millis: None, mem_bytes: None, disk_bytes: 0, exclusive: false, needs_git: false, gpus: 0 };
        // With nothing running, only a node-wide refusal is left.
        refusal::offer_refusal(&self.offer, 0, false, &probe).is_none()
    }

    /// Nothing running or queued there (from any server, as far as we know).
    fn idle(&self) -> bool {
        self.running_now() == 0 && self.queued == 0 && self.shared_view().is_none_or(|v| v.queued == 0)
    }

    /// Utilisation after placing the job, in thousandths (§4.1 step 1):
    /// the larger of memory and CPU used node-wide, and (solo nodes)
    /// running slots.
    fn load_after(&self, w: &Want) -> u32 {
        let (mem, cpu) = self.free_now();
        let used = |free: u64, ask: u64, cap: u64| if cap == 0 { 0 } else { 1000 - (free.saturating_sub(ask).saturating_mul(1000) / cap).min(1000) as u32 };
        let um = used(mem, w.mem_on(&self.offer).unwrap_or(0), self.offer.caps.mem_bytes);
        let uc = used(cpu, w.cpu_millis.unwrap_or(0), self.offer.caps.cpu_millis);
        let ur = if self.shared_view().is_none() && self.offer.parallel_max > 0 { ((self.running_now() + 1) * 1000 / self.offer.parallel_max).min(1000) } else { 0 };
        um.max(uc).max(ur)
    }

    /// The load band after placing the job: 0 idle, 1 light (≤ half
    /// used), 2 heavy.
    pub fn band(&self, w: &Want) -> u8 {
        if self.idle() {
            0
        } else if self.load_after(w) <= 500 {
            1
        } else {
            2
        }
    }
}

// ── rust components (CE-16, CE-A10) ──────────────────────────────────

/// Offer feature (k2-node 0.46.7): the node installs the Rust components a
/// need names (`rust@stable+clippy+rustfmt`). An older node is not a
/// candidate for such a need (`node_too_old`), so the token never reaches
/// a node that would refuse it. Same string as
/// `k2_node_proto::needs::FEATURE_RUST_COMPONENTS` (a test pins it; k2-node-proto
/// is only a dev-dependency of the daemon).
pub const FEATURE_RUST_COMPONENTS: &str = "rust-components";

/// The components `rust@<channel>+<component>` may name.
pub const RUST_COMPONENTS: &[&str] = &["clippy", "rustfmt", "rust-src", "llvm-tools", "rust-analyzer", "miri"];

/// Offer feature (protocol 7, §8): a bare `rust@<channel>` brings clippy
/// and rustfmt there; `+minimal` opts out (and needs this feature).
pub const FEATURE_RUST_DEFAULT_COMPONENTS: &str = needs::FEATURE_RUST_DEFAULT_COMPONENTS;

/// rustup's older names fold to the ones a need uses (`llvm-tools-preview`
/// → `llvm-tools`, `clippy-preview` → `clippy`), as k2-node folds them.
pub fn fold_component(c: &str) -> &str {
    c.strip_suffix("-preview").filter(|b| RUST_COMPONENTS.contains(b)).unwrap_or(c)
}

/// One need token split into its base and its Rust components:
/// `rust@stable+clippy+rustfmt` → (`rust@stable`, [clippy, rustfmt]);
/// `llvm-tools-preview` reads as `llvm-tools`; a component named twice
/// counts once. Only a Rust need takes components (`+` stays part of any
/// other token, e.g. `sys:g++`). A comma would split the needs list, hence
/// `+`. Targets (`+target:<triple>`) are checked and left out here: see
/// [`split_rust_parts`].
pub fn split_rust_components(token: &str) -> Result<(String, Vec<String>), String> {
    split_rust_parts(token).map(|(b, c, _)| (b, c))
}

/// [`split_rust_components`] plus the Rust compile targets:
/// `rust@1.95.0+clippy+target:wasm32-unknown-unknown` → (`rust@1.95.0`,
/// [clippy], [wasm32-unknown-unknown]); each list sorted and unique, at
/// most `needs::MAX_RUST_TARGETS` targets.
pub fn split_rust_parts(token: &str) -> Result<(String, Vec<String>, Vec<String>), String> {
    let t = token.trim();
    let Some((head, tail)) = t.split_once('+') else { return Ok((t.to_string(), Vec::new(), Vec::new())) };
    if !matches!(needs::parse(head), Ok(needs::Need::Rust { .. })) {
        return Ok((t.to_string(), Vec::new(), Vec::new()));
    }
    let mut comps: Vec<String> = Vec::new();
    let mut targets: Vec<String> = Vec::new();
    for c in tail.split('+') {
        if let Some(x) = c.strip_prefix(needs::TARGET_PREFIX) {
            if !needs::valid_target(x) {
                return Err(format!(
                    "{t:?}: {x:?} isn't a Rust target triple (a-z 0-9 _ - ., e.g. rust@1.95.0+target:wasm32-unknown-unknown; see rustup target list)"
                ));
            }
            if !targets.iter().any(|y| y == x) {
                targets.push(x.to_string());
            }
            continue;
        }
        let c = fold_component(c);
        if !RUST_COMPONENTS.contains(&c) && c != needs::RUST_MINIMAL {
            return Err(format!(
                "{t:?}: Rust components are {} joined to the channel with + (e.g. rust@stable+clippy+rustfmt; a compile target is +target:<triple>); {c:?} isn't one",
                RUST_COMPONENTS.join(", ")
            ));
        }
        if !comps.iter().any(|x| x == c) {
            comps.push(c.to_string());
        }
    }
    if targets.len() > needs::MAX_RUST_TARGETS {
        return Err(format!("{t:?}: at most {} targets", needs::MAX_RUST_TARGETS));
    }
    // Sorted, as k2-node's `Need::token()` writes them: one canonical token.
    comps.sort();
    targets.sort();
    Ok((head.to_string(), comps, targets))
}

/// Check a needs list as the node will (`needs::parse_all`: supported,
/// no duplicates, at most 32), Rust components included (CE-16). Returns
/// the canonical tokens (`nodejs@v22` → `node@22`,
/// `rust+clippy` → `rust@stable+clippy`).
pub fn check_needs<S: AsRef<str>>(raw: &[S]) -> Result<Vec<String>, needs::NeedError> {
    let mut bases = Vec::with_capacity(raw.len());
    let mut extras = Vec::with_capacity(raw.len());
    for t in raw {
        let (b, c, x) = split_rust_parts(t.as_ref()).map_err(|message| needs::NeedError { code: needs::NEED_INVALID, message })?;
        bases.push(b);
        extras.push((c, x));
    }
    let parsed = needs::parse_all(&bases)?;
    // Only a Rust need has extras: its token is k2-node's own (components,
    // then `target:` entries).
    Ok(parsed
        .into_iter()
        .zip(extras)
        .map(|(n, (components, targets))| match n {
            needs::Need::Rust { channel, .. } => needs::Need::Rust { channel, components, targets }.token(),
            other => other.token(),
        })
        .collect())
}

/// The Rust compile targets a (canonical) need token names; empty for any
/// other.
pub fn need_targets(token: &str) -> Vec<String> {
    split_rust_parts(token).map(|(_, _, t)| t).unwrap_or_default()
}

/// Any need names a Rust target (the node must offer
/// `needs::FEATURE_RUST_TARGETS`, and have rustup).
pub fn wants_targets(needs: &[String]) -> bool {
    needs.iter().any(|t| !need_targets(t).is_empty())
}

/// The node gets Rust from a system package with no rustup (its offer's
/// `rust-installer` is `system`): no target can be added.
pub fn system_rust(offer: &Offer) -> bool {
    offer.tools.get(needs::TOOLS_RUST_INSTALLER).is_some_and(|v| v == "system")
}

/// The Rust components a (canonical) need token names; empty for any other.
pub fn need_components(token: &str) -> Vec<String> {
    split_rust_components(token).map(|(_, c)| c).unwrap_or_default()
}

/// Any need names Rust components (the node must offer
/// [`FEATURE_RUST_COMPONENTS`]).
pub fn wants_components(needs: &[String]) -> bool {
    needs.iter().any(|t| need_components(t).iter().any(|c| c != needs::RUST_MINIMAL))
}

/// A need says `rust@…+minimal` (the node must offer
/// [`FEATURE_RUST_DEFAULT_COMPONENTS`]).
pub fn wants_minimal(needs: &[String]) -> bool {
    needs.iter().any(|t| need_components(t).iter().any(|c| c == needs::RUST_MINIMAL))
}

/// A need only protocol 7 nodes read: a `profile:` or a native
/// `sys:<manager>:<name>` (feature `pkg-map`).
pub fn wants_pkg_map(needs: &[String]) -> bool {
    needs.iter().any(|t| t.starts_with("profile:") || t.strip_prefix("sys:").is_some_and(|p| p.contains(':')))
}

/// Every system name a needs list asks for, canonical (or
/// `<manager>:<name>`), its profiles expanded, sorted and unique: the
/// plan's `packages` for a `pkg-map` node, and what placement checks.
pub fn plan_packages(needs: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in needs {
        if let Some(p) = t.strip_prefix("profile:") {
            if let Ok(pr) = k2_core::compute::proto::profiles::resolve(p) {
                out.extend(pr.all_sys());
            }
        } else if let Some(p) = t.strip_prefix("sys:") {
            out.push(p.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The needs with every `profile:` replaced by its tools and `sys:` names
/// (an explicit need for the same tool wins), for placement and `--plan`.
pub fn expand_profiles(needs: &[String]) -> Vec<String> {
    let mut out: Vec<String> = needs.iter().filter(|t| !t.starts_with("profile:")).cloned().collect();
    let key = |t: &str| t.split(['@', '+']).next().unwrap_or(t).to_string();
    for t in needs {
        let Some(p) = t.strip_prefix("profile:") else { continue };
        let Ok(pr) = k2_core::compute::proto::profiles::resolve(p) else { continue };
        for tool in pr.all_tools() {
            if !out.iter().any(|o| key(o) == key(&tool)) {
                out.push(tool);
            }
        }
        for s in pr.all_sys() {
            let tok = format!("sys:{s}");
            if !out.contains(&tok) {
                out.push(tok);
            }
        }
    }
    out
}

/// Offer feature (k2-node 0.46.7, CE-18 cap): the node keeps its tool
/// cache under `toolchain_budget_gb`. `k2 compute prewarm` says when a
/// node has no cap (an older k2-node). Same string as
/// `k2_node_proto::needs::FEATURE_TOOLCHAIN_BUDGET` (a test pins it).
pub const FEATURE_TOOLCHAIN_BUDGET: &str = "toolchain-budget";

// ── the tools a node has (CE-1, CE-2) ────────────────────────────────

/// What a node has for one need, from its offer's `tools` map (re-probed
/// every 10 min; the node installs what's missing either way).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeedState {
    /// Installed: a `mise:<tool>` version matching the need's version
    /// prefix (`latest`/`lts`: any), the Rust channel in `rust-toolchains`
    /// with every component it names, `cargo-nextest` present.
    Ready,
    /// The node can install it, but it isn't there yet (also: every need
    /// on an older node without a tools map).
    Install,
    /// A `sys:` package on macOS (no system packages there). Auto-placement
    /// skips the node (protocol 7, §5.3); a job pinned to a Mac runs and
    /// fails with the `brew install` line unless it's there already.
    UnsupportedOnOs,
    /// Protocol 7: mappable, but this server's package policy on the node
    /// doesn't cover it: a request to the node's owner (a card).
    Ask,
    /// Protocol 7: no name for this node's package manager.
    NoMapping,
    /// Protocol 7: on K2's deny list (never installed by K2).
    Denied,
    /// The node can't provide it at all (a Rust target on a node
    /// whose Rust is a system package with no rustup). Auto placement
    /// leaves the node out; a job pinned there is refused
    /// (`rust_target_unsupported`).
    Unsupported,
}

impl NeedState {
    pub fn as_str(self) -> &'static str {
        match self {
            NeedState::Ready => "ready",
            NeedState::Install => "install",
            NeedState::UnsupportedOnOs => "unsupported_on_os",
            NeedState::Ask => "ask",
            NeedState::NoMapping => "no_mapping",
            NeedState::Denied => "denied",
            NeedState::Unsupported => "unsupported",
        }
    }
}

/// One need against one node (`--plan` `needs`, CE-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedStatus {
    pub need: String,
    pub state: NeedState,
    /// The version (or Rust toolchain) found, when ready.
    pub have: Option<String>,
    /// A system name as this node installs it (`openssl` for
    /// `sys:libssl-dev` on Arch), protocol 7 nodes only.
    pub native: Option<String>,
}

impl NeedStatus {
    /// `{"need","state","have"}` (stable keys; `have` null when absent),
    /// plus `native` from a protocol 7 node only.
    pub fn json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({"need": self.need, "state": self.state.as_str(), "have": self.have});
        if let Some(n) = &self.native {
            v["native"] = n.clone().into();
        }
        v
    }
}

/// `22.23.3` matches `22` and `22.23` (on a dot), `latest`/`lts` match any.
fn version_matches(have: &str, want: &str) -> bool {
    matches!(want, "latest" | "lts") || have == want || have.strip_prefix(want).is_some_and(|r| r.starts_with('.'))
}

/// A rustup toolchain dir (`stable-aarch64-apple-darwin`,
/// `1.99.1-x86_64-unknown-linux-gnu`) is `channel` for that host. Never
/// `nightly` for `nightly-2026-09-01-…`, nor `1.99` for `1.99.1-…`.
fn toolchain_is(entry: &str, channel: &str) -> bool {
    entry == channel
        || entry
            .strip_prefix(channel)
            .and_then(|r| r.strip_prefix('-'))
            .and_then(|host| host.split('-').next())
            .is_some_and(|arch| !arch.is_empty() && !arch.chars().all(|c| c.is_ascii_digit()))
}

/// The offer's OS is macOS (`os` label, spellings folded).
fn offer_is_macos(o: &Offer) -> bool {
    label_matches("os", "macos", o.labels.get("os").map(String::as_str))
}

/// One `sys:` name on a node (§5.3): an older node (no package status)
/// reads `install`, as before; a protocol 7 node says whether it is
/// installed, covered by this server's policy, to be asked for, or not
/// satisfiable there.
fn sys_state(offer: &Offer, token: &str, package: &str) -> NeedStatus {
    use k2_core::compute::proto::{pkgmap, pkgpolicy};
    let st = |state: NeedState, native: Option<String>| NeedStatus { need: token.to_string(), state, have: None, native };
    let (Some(pkg), Some(host)) = (offer.pkg.as_ref(), offer.host.as_ref()) else { return st(NeedState::Install, None) };
    let Some(mgr) = pkgmap::Mgr::parse(&pkg.manager) else { return st(NeedState::Install, None) };
    let Some(natives) = pkgmap::SysName::parse(package).and_then(|n| n.native(mgr, &host.distro, &host.distro_version)) else {
        return st(NeedState::NoMapping, None);
    };
    let native = Some(natives.join(" "));
    if pkg.installed.iter().any(|i| i == package) {
        return NeedStatus { need: token.to_string(), state: NeedState::Ready, have: Some("installed".into()), native };
    }
    match pkgpolicy::coverage(&pkg.policy(), &pkg.approved, package, &natives) {
        pkgpolicy::Coverage::Denied(_) => st(NeedState::Denied, native),
        pkgpolicy::Coverage::Ask => st(NeedState::Ask, native),
        _ => st(NeedState::Install, native),
    }
}

/// The `sys:` names a node can't give without asking, and the ones it
/// can't give at all (no map row, denied).
pub fn package_gaps(states: &[NeedStatus]) -> (Vec<String>, Vec<String>) {
    let name = |s: &NeedStatus| s.need.strip_prefix("sys:").unwrap_or(&s.need).to_string();
    let ask = states.iter().filter(|s| s.state == NeedState::Ask).map(name).collect();
    let never = states.iter().filter(|s| matches!(s.state, NeedState::NoMapping | NeedState::Denied)).map(name).collect();
    (ask, never)
}

/// Each need's state on the node behind `offer` (CE-1). Tool keys the node
/// reports: `mise:<tool>` (installed versions, comma list),
/// `rust-toolchains` (toolchain dirs), `rust-components:<toolchain>`
/// (k2-node 0.46.7, CE-A10: installed components), `cargo-nextest`.
pub fn need_states(offer: &Offer, needs: &[String]) -> Vec<NeedStatus> {
    let list = |k: &str| -> Vec<String> { offer.tools.get(k).map(|v| v.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect()).unwrap_or_default() };
    let mac = offer_is_macos(offer);
    needs
        .iter()
        .map(|token| {
            let st = |state: NeedState, have: Option<String>| NeedStatus { need: token.clone(), state, have, native: None };
            let Ok((base, comps, targets)) = split_rust_parts(token) else { return st(NeedState::Install, None) };
            match needs::parse(&base) {
                Ok(needs::Need::System { .. }) if mac => st(NeedState::UnsupportedOnOs, None),
                Ok(needs::Need::System { package }) => sys_state(offer, token, &package),
                Ok(needs::Need::Profile { .. }) => st(NeedState::Install, None),
                Ok(needs::Need::Nextest) => match offer.tools.get("cargo-nextest") {
                    Some(v) => st(NeedState::Ready, Some(v.clone())),
                    None => st(NeedState::Install, None),
                },
                Ok(needs::Need::Rust { .. }) if !targets.is_empty() && system_rust(offer) => st(NeedState::Unsupported, None),
                Ok(needs::Need::Rust { channel, .. }) => {
                    let Some(tc) = list("rust-toolchains").into_iter().find(|e| toolchain_is(e, &channel)) else { return st(NeedState::Install, None) };
                    let have_comps: Vec<String> = offer
                        .tools
                        .iter()
                        .find(|(k, _)| k.strip_prefix("rust-components:").is_some_and(|x| x == tc || toolchain_is(x, &channel)))
                        .map(|(_, v)| v.split(',').map(|c| fold_component(c.trim()).to_string()).collect())
                        .unwrap_or_default();
                    // Protocol 7 (§8): a bare need brings clippy and
                    // rustfmt on nodes that install them by default.
                    let mut comps: Vec<String> = comps.into_iter().filter(|c| c != needs::RUST_MINIMAL).collect();
                    if offer.features.iter().any(|f| f == FEATURE_RUST_DEFAULT_COMPONENTS) && !need_components(token).iter().any(|c| c == needs::RUST_MINIMAL) {
                        for d in needs::DEFAULT_RUST_COMPONENTS {
                            if !comps.iter().any(|c| c == d) {
                                comps.push(d.to_string());
                            }
                        }
                    }
                    // Each target it names, from `rust-targets:<tc>`.
                    let have_targets: Vec<String> = offer
                        .tools
                        .iter()
                        .find(|(k, _)| k.strip_prefix("rust-targets:").is_some_and(|x| x == tc || toolchain_is(x, &channel)))
                        .map(|(_, v)| v.split(',').map(|x| x.trim().to_string()).collect())
                        .unwrap_or_default();
                    if comps.iter().all(|c| have_comps.contains(c)) && targets.iter().all(|x| have_targets.contains(x)) {
                        st(NeedState::Ready, Some(tc))
                    } else {
                        st(NeedState::Install, None)
                    }
                }
                Ok(needs::Need::Tool { tool, version }) if needs::NATIVE_TOOLS.contains(&tool.as_str()) => {
                    let (state, have) = native::need_state(offer, &tool, &version);
                    st(state, have)
                }
                Ok(needs::Need::Tool { tool, version }) => match list(&format!("mise:{tool}")).into_iter().rfind(|v| version_matches(v, &version)) {
                    Some(v) => st(NeedState::Ready, Some(v)),
                    None => st(NeedState::Install, None),
                },
                Err(_) => st(NeedState::Install, None),
            }
        })
        .collect()
}

/// Every need is ready there (and there is at least one).
pub fn tools_ready(states: &[NeedStatus]) -> bool {
    !states.is_empty() && states.iter().all(|s| s.state == NeedState::Ready)
}

/// "tools ready" · "installs rust@stable, cmake@3 first (~minutes)" ·
/// "sys:pkg-config: no system packages on macOS" (CE-2); `None` with no
/// needs.
pub fn tools_words(states: &[NeedStatus]) -> Option<String> {
    if states.is_empty() {
        return None;
    }
    let of = |s: NeedState| states.iter().filter(|x| x.state == s).map(|x| x.need.as_str()).collect::<Vec<_>>();
    let (install, unsupported) = (of(NeedState::Install), of(NeedState::UnsupportedOnOs));
    let (ask, never) = (of(NeedState::Ask), [of(NeedState::NoMapping), of(NeedState::Denied)].concat());
    let cannot = of(NeedState::Unsupported);
    let mut parts = Vec::new();
    if install.is_empty() && unsupported.is_empty() && ask.is_empty() && never.is_empty() && cannot.is_empty() {
        parts.push("tools ready".to_string());
    }
    if !ask.is_empty() {
        parts.push(format!("{}: the node's owner must approve (a request)", ask.join(", ")));
    }
    if !never.is_empty() {
        parts.push(format!("{}: not installable there", never.join(", ")));
    }
    if !cannot.is_empty() {
        parts.push(format!("{}: this node's Rust is a system package with no rustup (no Rust targets here)", cannot.join(", ")));
    }
    if !install.is_empty() {
        parts.push(format!("installs {} first (~minutes)", install.join(", ")));
    }
    if !unsupported.is_empty() {
        parts.push(format!("{}: no system packages on macOS (fails unless already there)", unsupported.join(", ")));
    }
    Some(parts.join(" · "))
}

/// The everyday tools a job's script leans on, as a node reports them in
/// its offer (k2-node 0.46.17 probes them with the job env).
pub const HOST_TOOLS: [&str; 3] = ["python3", "curl", "bash"];

/// "python3 3.12.4 · curl 8.5.0 · bash 5.2.21" from a node's offer, so the
/// placement line shows how nodes differ before a script trips on it;
/// `None` when the node reports none of them (an older k2-node).
pub fn host_tools_words(offer: &Offer) -> Option<String> {
    let parts: Vec<String> = HOST_TOOLS.iter().filter_map(|k| offer.tools.get(*k).map(|v| format!("{k} {v}"))).collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The tools part of a placement line and `--plan`'s `toolsLine`: the
/// needs' state ([`tools_words`]), then the node's everyday tools.
pub fn tools_line(states: &[NeedStatus], offer: Option<&Offer>) -> Option<String> {
    match (tools_words(states), offer.and_then(host_tools_words)) {
        (Some(a), Some(b)) => Some(format!("{a} · {b}")),
        (a, b) => a.or(b),
    }
}

/// Features a plan needs from the node's k2-node.
pub fn required_features(w: &Want) -> Vec<&'static str> {
    let mut f = Vec::new();
    if !w.needs.is_empty() {
        f.push(needs::FEATURE_NEEDS);
    }
    if wants_components(&w.needs) {
        f.push(FEATURE_RUST_COMPONENTS);
    }
    if wants_minimal(&w.needs) {
        f.push(FEATURE_RUST_DEFAULT_COMPONENTS);
    }
    if wants_pkg_map(&w.needs) {
        f.push(k2_core::compute::proto::pkgmap::FEATURE_PKG_MAP);
    }
    if wants_targets(&w.needs) {
        f.push(needs::FEATURE_RUST_TARGETS);
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
    if w.cache.is_some() {
        f.push(k2_core::compute::proto::cache::FEATURE_JOB_CACHE);
    }
    if w.multi_repo {
        f.push(frames::FEATURE_MULTI_REPO);
    }
    if w.receipt_v2 {
        f.push(k2_core::compute::proto::ci::FEATURE_RECEIPT_V2);
    }
    if w.gpu.is_some() {
        f.push(k2_core::compute::proto::gpu::FEATURE_GPU);
    }
    if w.gpu_mem.is_some() {
        f.push(k2_core::compute::proto::gpu::FEATURE_GPU_MEM);
    }
    // 0.46.15: `job-network`, and `netns-dummy` for dummy interfaces.
    f.extend(k2_core::compute::proto::jobnet::plan_features(
        w.network.as_ref(),
    ));
    // 0.46.18: `--userns` only where the node runs it.
    if w.userns {
        f.push(k2_core::compute::proto::userns::FEATURE_JOB_USERNS);
    }
    f
}

/// The stall timeout a plan for a node with `features` carries: the asked
/// one, else the default — only on nodes that enforce it, and only when
/// shorter than the wall-clock limit.
pub fn stall_for(ctl: &JobCtl, tier: JobTier, features: &[String], max_secs: u64) -> Option<u64> {
    let s = match ctl.stall_secs {
        Some(0) => return None,
        Some(n) => n,
        None => default_stall_secs(tier),
    };
    (s > 0 && (s as u64) < max_secs && features.iter().any(|f| f == FEATURE_STALL)).then_some(s as u64)
}

/// The stall timeout a job gets with no `--stall-timeout` (and no preset
/// `stall_timeout`): 2 h for Priority and CI, 30 min for the rest.
pub fn default_stall_secs(tier: JobTier) -> i64 {
    let t = retry::tuning();
    if matches!(tier, JobTier::Priority | JobTier::Ci) {
        t.stall_priority_secs
    } else {
        t.stall_secs
    }
}

/// Why a granted node was left out (PRD §2; the "skipped" counts).
pub mod skip {
    pub const PAUSED: &str = "paused";
    pub const SET_ASIDE: &str = "set_aside";
    pub const QUEUE_FULL: &str = "queue_full";
    pub const OFFLINE: &str = "offline";
    pub const NODE_TOO_OLD: &str = "node_too_old";
    pub const OS_ARCH_LABEL: &str = "os_arch_label";
    pub const TOO_SMALL: &str = "too_small";
    pub const SECRET_NOT_COVERED: &str = "secret_not_covered";
    pub const CI_NOT_ALLOWED: &str = "ci_not_allowed";
    pub const TIMEOUT_OVER_GRANT: &str = "timeout_over_grant";
    pub const SHARE_LIMIT: &str = "share_limit";
    /// `--exclusive` where the grant doesn't allow CI (CE-13).
    pub const EXCLUSIVE_NEEDS_GRANT: &str = "exclusive_needs_grant";
    /// Online but paused, draining, outside its window, disk low, locked.
    pub const UNAVAILABLE: &str = "unavailable";
    /// Granted, but off the workspace's node list (0.46.8,
    /// `crate::compute_allow`).
    pub const NOT_ALLOWED: &str = crate::compute_allow::SKIP;
    /// Granted, but the job says `--avoid` it (0.46.8).
    pub const AVOIDED: &str = "avoided";
    /// Protocol 7: another distro than `--distro` (or an older node that
    /// doesn't say).
    pub const DISTRO: &str = "distro";
    /// Protocol 7: a `sys:` name with no name for its package manager, or
    /// one on K2's deny list.
    pub const NO_PACKAGE_MAPPING: &str = "no_package_mapping";
    /// Protocol 7: a `sys:` name its owner must approve first, while
    /// another node can take the job without asking.
    pub const PACKAGE_ASK: &str = "package_ask";
    /// A `sys:` need on a Mac (auto-placement only).
    pub const UNSUPPORTED_ON_OS: &str = "unsupported_on_os";
    /// Not on the job's `--only` list (0.46.9).
    pub const NOT_ONLY: &str = "not_only";
    /// The node takes only jobs that name it
    /// (`k2_core::compute::pinned_only`); auto-placement only.
    pub const PINNED_ONLY: &str = "pinned_only";
    /// GPU jobs (0.46.11): `--gpu` where the node runs no GPU jobs for
    /// this server (or none of the asked vendor).
    pub const NO_GPU: &str = "no_gpu";
    /// GPU jobs (0.46.11): the node has a GPU for this server, the grant
    /// doesn't say `--gpu`.
    pub const GPU_NEEDS_GRANT: &str = "gpu_needs_grant";
    /// GPU jobs S6: `--gpu-mem` on a GPU node whose k2-node doesn't offer
    /// `gpu-mem` (older than 0.46.11, or its executor is).
    pub const GPU_MEM_NEEDS_UPDATE: &str = k2_core::compute::proto::gpu::codes::GPU_MEM_NEEDS_UPDATE;
    /// A need it can't provide at all (a Rust target where Rust is
    /// a system package with no rustup).
    pub const TOOLS_UNSUPPORTED: &str = "tools_unsupported";
    /// 0.46.15: a node that can't run the job's `--network` ask (k2-node
    /// or its executor older than 0.46.15, no job wall, or a Mac for
    /// `--netns-dummy`).
    pub const JOB_NETWORK: &str = "job_network";
    /// 0.46.18 (`--userns`): a node where this server is a tenant (only
    /// the node owner's own server's jobs may make user namespaces).
    pub const USERNS_OWNER_ONLY: &str = k2_core::compute::proto::userns::codes::USERNS_OWNER_ONLY;
    /// 0.46.18: a node this server owns whose k2-node doesn't offer
    /// `job-userns` (older than 0.46.18, or no job wall).
    pub const USERNS_NEEDS_UPDATE: &str = k2_core::compute::proto::userns::codes::USERNS_NEEDS_UPDATE;
    /// 0.46.18: the node runs `--userns` jobs, the workspace's grant there
    /// doesn't say `--userns`.
    pub const USERNS_NOT_GRANTED: &str = k2_core::compute::proto::userns::codes::USERNS_NOT_GRANTED;
}

/// One spelling per OS and arch, so `--arch aarch64` finds a Mac that
/// reports `arm64` and `--os darwin` a node that says `macos`. Nodes report
/// `macos`/`linux` and `arm64` (Macs), `aarch64` (Linux ARM) or `x86_64`.
fn canon<'a>(key: &str, v: &'a str) -> std::borrow::Cow<'a, str> {
    let lower = v.trim().to_ascii_lowercase();
    let c = match (key, lower.as_str()) {
        ("arch", "aarch64" | "arm64") => "arm64",
        ("arch", "x86_64" | "amd64" | "x64" | "x86-64") => "x86_64",
        ("os", "macos" | "darwin" | "mac" | "osx") => "macos",
        ("os", "linux") => "linux",
        _ => return std::borrow::Cow::Borrowed(v),
    };
    std::borrow::Cow::Borrowed(c)
}

/// An `--os`/`--arch`/`--label` ask against a node's label, aliases folded
/// for `os` and `arch` (other labels match exactly).
pub fn label_matches(key: &str, want: &str, have: Option<&str>) -> bool {
    have.is_some_and(|h| canon(key, want) == canon(key, h))
}

/// The candidate list and what was left out.
#[derive(Debug, Clone, Default)]
pub struct CandReport {
    pub cands: Vec<Cand>,
    /// Granted (visible) nodes left out, by [`skip`] code.
    pub skipped: BTreeMap<&'static str, u32>,
    /// Nodes this workspace may see at all (granted; or every node for a
    /// pinned owner/Admin caller).
    pub visible: u32,
    /// GPU jobs: nodes skipped [`skip::GPU_NEEDS_GRANT`], by name (the
    /// refusal names one for the owner's grant and the agent's card).
    pub gpu_needs_grant: Vec<String>,
    /// `--userns`: nodes skipped [`skip::USERNS_NOT_GRANTED`], by name.
    pub userns_not_granted: Vec<String>,
    /// `--userns`: nodes skipped [`skip::USERNS_OWNER_ONLY`] (this server
    /// is a tenant there), by name.
    pub userns_owner_only: Vec<String>,
}

/// Every node that could take the job, except `exclude`.
pub fn candidates(w: &Want, exclude: &[String], now: i64) -> Vec<Cand> {
    candidates_report(w, exclude, now).cands
}

/// [`candidates`] with the skipped counts. A node this workspace holds no
/// grant on is never listed or counted in auto mode (S2): an agent never
/// learns it exists.
pub fn candidates_report(w: &Want, exclude: &[String], now: i64) -> CandReport {
    let mut out = candidates_report_raw(w, exclude, now);
    if w.auto {
        skip_package_gaps(w, &mut out);
    }
    out
}

/// prd-compute-blacksmith-class-v1 R1 (`--mem auto`): the most memory one
/// job may ask on any node this job could otherwise run on (its caps, and a
/// shared node's per-job limit for this server). `None`: no such node
/// reports a cap (placement then says why it can't run).
pub fn largest_job_mem(w: &Want, now: i64) -> Option<u64> {
    let probe = Want { mem_bytes: None, mem_defaulted: true, ..w.clone() };
    candidates_report_raw(&probe, &[], now)
        .cands
        .iter()
        .filter_map(|c| {
            let node = (c.offer.caps.mem_bytes > 0).then_some(c.offer.caps.mem_bytes);
            let share = c.offer.share.as_ref().filter(|v| v.shared && v.limits.max_mem_gb > 0).map(|v| u64::from(v.limits.max_mem_gb) << 30);
            match (node, share) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
        })
        .max()
}

/// [`candidates_report`] without the package step (prewarm reports every
/// node's package states itself and files one batched request).
pub fn candidates_report_raw(w: &Want, exclude: &[String], now: i64) -> CandReport {
    // One pooled read for every node: the node list, this workspace's
    // grants, and every queued and active job, in one snapshot. Never the
    // main lock (a read never waits behind a writer, nor holds one up),
    // never a lock per node.
    let snap = k2_core::db::read(|conn| PlaceSnapshot::read(conn, w));
    let PlaceSnapshot { nodes: rows, cache_at, permit, pinned_only, mut grants, queued, active } = snap;
    let waits = SoloWaits::of(&queued, &active, now);
    let by_node = |list: &[JobRow], id: &str| -> Vec<JobRow> { list.iter().filter(|j| j.node_id == id).cloned().collect() };
    let feats = required_features(w);
    let mut out = CandReport::default();
    let skipped = |out: &mut CandReport, code: &'static str| *out.skipped.entry(code).or_insert(0) += 1;
    for n in rows.into_iter().filter(|n| n.state == "active") {
        if exclude.contains(&n.id) {
            continue;
        }
        // The node's live state (the engine lock), with no database lock held.
        let (online, _, live) = compute_ws::live_view(&n.id);
        let node_queued = by_node(&queued, &n.id);
        let node_active = by_node(&active, &n.id);
        let (grant, queued_ws, queued_all, load) = {
            let g = grants.remove(&n.id);
            let (q, load) = compute_ws::queued_view_of(&node_queued, &node_active, live.as_ref());
            // As `store::queued_count`: queued, or assigned and in the node's queue.
            let queued_ws = node_queued
                .iter()
                .chain(node_active.iter())
                .filter(|j| j.workspace_id == w.ws_id && (j.state == "queued" || (j.state == "assigned" && j.node_queued)))
                .count() as i64;
            (g, queued_ws, q.len(), load)
        };
        // F2 (D1): auto never crosses grants; the owner/Admin bypass is for
        // a node they name.
        if grant.is_none() && (w.auto || !w.admin) {
            continue;
        }
        out.visible += 1;
        // `--avoid` (0.46.8): never this node, for this job, in any move.
        if w.auto && w.filter.avoids(&n.id, &n.name) {
            skipped(&mut out, skip::AVOIDED);
            continue;
        }
        // `--only` (0.46.9): never a node off the job's own list.
        if w.auto && !w.filter.only.is_empty() && !w.filter.only.contains(&n.id) {
            skipped(&mut out, skip::NOT_ONLY);
            continue;
        }
        if !permit.allows(&n.id) {
            skipped(&mut out, skip::NOT_ALLOWED);
            continue;
        }
        if n.controller_pause.is_some() {
            skipped(&mut out, skip::PAUSED);
            continue;
        }
        // A pinned-only node is never chosen for a job: only a job that
        // names it goes there, and that caller is not `auto`.
        if w.auto && pinned_only.holds(&n.id) {
            skipped(&mut out, skip::PINNED_ONLY);
            continue;
        }
        if set_aside_until(&n.id, now).is_some() {
            skipped(&mut out, skip::SET_ASIDE);
            continue;
        }
        if grant.as_ref().is_some_and(|g| queued_ws >= g.max_queued) {
            skipped(&mut out, skip::QUEUE_FULL);
            continue;
        }
        let Some(offer) = live.filter(|_| online).or_else(|| n.last_offer.clone()) else {
            skipped(&mut out, skip::OFFLINE);
            continue;
        };
        // GPU jobs (0.46.11) and the `--network` ask (0.46.15): before the
        // feature check, so a node without GPU jobs for this server says
        // `no_gpu`, one that can't run the job's network `job_network`,
        // not `node_too_old`.
        // 0.46.18: and the `--userns` ask (a node this server owns, with
        // `job-userns`, and the grant's `allow_userns`).
        if let Some(code) = crate::compute_job_network::ask_skip(w, grant.as_ref(), &offer, crate::compute_userns_ask::owns(&n)) {
            if code == skip::GPU_NEEDS_GRANT {
                out.gpu_needs_grant.push(n.name.clone());
            }
            if code == skip::USERNS_NOT_GRANTED {
                out.userns_not_granted.push(n.name.clone());
            }
            if code == skip::USERNS_OWNER_ONLY {
                out.userns_owner_only.push(n.name.clone());
            }
            skipped(&mut out, code);
            continue;
        }
        if feats.iter().any(|f| !offer.features.iter().any(|x| x == f)) {
            skipped(&mut out, skip::NODE_TOO_OLD);
            continue;
        }
        let label = |k: &str| offer.labels.get(k).map(String::as_str);
        if w.filter.os.as_deref().is_some_and(|o| !label_matches("os", o, label("os")))
            || w.filter.arch.as_deref().is_some_and(|a| !label_matches("arch", a, label("arch")))
            || w.filter.labels.iter().any(|(k, x)| !label_matches(k, x, label(k)))
        {
            skipped(&mut out, skip::OS_ARCH_LABEL);
            continue;
        }
        // `--distro` (protocol 7): ID or ID_LIKE, the version on a dot.
        if let Some(spec) = w.filter.distro.as_deref().and_then(|d| k2_core::compute::proto::host::parse_distro(d).ok()) {
            if !offer.host.as_ref().is_some_and(|h| h.matches(&spec)) {
                skipped(&mut out, skip::DISTRO);
                continue;
            }
        }
        // Over the node's caps (--cpus/--mem): it would never start there
        // (compute DX slice A: never queued where it can't ever fit).
        if scheduler::never_fits(&offer, &w.probe_on(&offer)).is_some() {
            skipped(&mut out, skip::TOO_SMALL);
            continue;
        }
        // Secret-env (SE6): only nodes a grant covers for every secret
        // (`*` covers only nodes this server owns, Q2), and where the lane's
        // rules let it be sealed (C3b, V25).
        if !w.secret_ids.is_empty() && !crate::compute_secrets::placeable(&w.ws_id, &n, Some(&offer), &w.secret_ids, w.exclusive) {
            skipped(&mut out, skip::SECRET_NOT_COVERED);
            continue;
        }
        // `--any --tier ci`: only where this workspace may send CI (T12).
        if w.tier == JobTier::Ci && !ci_allowed(w.admin, grant.as_ref(), Some(&offer)) {
            skipped(&mut out, skip::CI_NOT_ALLOWED);
            continue;
        }
        // `--exclusive` (CE-13, D3): only where the grant allows CI (or
        // the caller is the owner or an Admin).
        if w.exclusive && !(w.admin || grant.as_ref().is_some_and(|g| g.allow_ci)) {
            skipped(&mut out, skip::EXCLUSIVE_NEEDS_GRANT);
            continue;
        }
        if !timeout_fits(w, grant.as_ref()) {
            skipped(&mut out, skip::TIMEOUT_OVER_GRANT);
            continue;
        }
        if !share_fits(w, grant.as_ref(), &offer) {
            skipped(&mut out, skip::SHARE_LIMIT);
            continue;
        }
        // A need it can never provide (a Rust target on system
        // Rust): never placed there.
        if need_states(&offer, &w.needs).iter().any(|s| s.state == NeedState::Unsupported) {
            skipped(&mut out, skip::TOOLS_UNSUPPORTED);
            continue;
        }
        let pending = pending_for(&n.id, online.then_some(offer.revision), now);
        let recent = recent_for(&n.id, now);
        let holds_cache = cache_at.as_deref() == Some(n.id.as_str());
        let est_wait = online.then(|| backlog::est_wait(&waits, &n.id, &offer, grant.as_ref(), &load, &w.ws_id, node_queued.iter().chain(node_active.iter()), now)).flatten();
        let held_wait = if share::is_shared(&offer) { waits.held_eta(&n.id, offer.parallel_max) } else { 0 };
        let gen_heads = if online { crate::compute_ws::trusted_heads(&n.id) } else { Vec::new() };
        out.cands.push(Cand { node: n, online, offer, load, queued: queued_all, grant, pending, recent, holds_cache, est_wait, held_wait, gen_heads, ws_id: w.ws_id.clone() });
    }
    out
}

/// Auto-placement and system packages (protocol 7, §5.3): a node that can
/// never give a `sys:` name (no map row, denied, a Mac) is skipped; a node
/// whose owner must approve one is skipped while another node can take the
/// job without asking. When every node must ask, they stay: submit files
/// the request on the best one and refuses (`package_not_approved`).
fn skip_package_gaps(w: &Want, out: &mut CandReport) {
    if !w.wants_sys() {
        return;
    }
    let mut keep: Vec<(Cand, bool)> = Vec::new();
    for c in std::mem::take(&mut out.cands) {
        let states = c.needs(w);
        if states.iter().any(|s| s.state == NeedState::UnsupportedOnOs) {
            *out.skipped.entry(skip::UNSUPPORTED_ON_OS).or_insert(0) += 1;
            continue;
        }
        let (ask, never) = package_gaps(&states);
        if !never.is_empty() {
            *out.skipped.entry(skip::NO_PACKAGE_MAPPING).or_insert(0) += 1;
            continue;
        }
        keep.push((c, !ask.is_empty()));
    }
    let any_free = keep.iter().any(|(_, ask)| !ask);
    for (c, ask) in keep {
        if ask && any_free {
            *out.skipped.entry(skip::PACKAGE_ASK).or_insert(0) += 1;
        } else {
            out.cands.push(c);
        }
    }
}

// ── resume affinity and solo waits (CE-6, P2) ────────────────────────

/// CE-6, CE-A15: the node that holds this workspace's cache `name`: where
/// its jobs with that cache built it up ([`store::cache_home`]). An
/// auto-placed job that ran elsewhere with an empty cache (its node was
/// busy) doesn't move it; a pinned job does. Only this workspace's rows
/// (S3). `None` when no job used it.
pub fn cache_node(conn: &rusqlite::Connection, ws_id: &str, name: &str) -> Option<String> {
    store::cache_home(conn, ws_id, name)
}

/// CE-6: a `--cache` job waits on the busy node that holds its cache when
/// that wait is at most this long (else it goes elsewhere), and the
/// sweeper doesn't move it for being busy until it has waited this long.
pub const CACHE_QUEUE_SECS: u64 = 900;

/// The placement code of a job queued on the node that holds its cache.
pub const QUEUED_FOR_CACHE: &str = "queued_for_cache";

/// Queued for its cache, and still inside [`CACHE_QUEUE_SECS`]: only a
/// hard reason (offline, paused, refused) moves it.
fn held_for_cache(j: &JobRow, since: i64, now: i64) -> bool {
    j.ctl.placement.as_ref().is_some_and(|p| p.code == QUEUED_FOR_CACHE) && now - since < CACHE_QUEUE_SECS as i64
}

/// A job's estimate when it has none (P2: 15 min per unknown).
pub const UNKNOWN_EST_SECS: u64 = 900;

/// The wait on a solo node (prd-compute-auto-placement-v1 §4.2, P2): the
/// estimates of the jobs ahead ÷ its parallel slots, plus the least time
/// left on a running job; an unknown estimate counts 15 min. Pure, so
/// `get`'s `wait.etaSecs` can use it for one queued job (Lane B).
pub fn solo_eta(ahead: &[Option<u64>], running_left: &[Option<u64>], parallel_max: u32) -> u64 {
    let sum: u64 = ahead.iter().map(|e| e.unwrap_or(UNKNOWN_EST_SECS)).sum();
    let first = running_left.iter().map(|l| l.unwrap_or(UNKNOWN_EST_SECS)).min().unwrap_or(0);
    sum / u64::from(parallel_max.max(1)) + first
}

/// This server's queued and running jobs per node, for [`solo_eta`].
#[derive(Debug, Default)]
pub struct SoloWaits {
    ahead: HashMap<String, Vec<Option<u64>>>,
    left: HashMap<String, Vec<Option<u64>>>,
    /// Only the jobs still queued on this server (not in a node's queue).
    held: HashMap<String, Vec<Option<u64>>>,
}

impl SoloWaits {
    /// From rows already read: every queued job ([`store::queued_jobs`])
    /// and every active one ([`store::active_jobs`]).
    pub fn of(queued: &[JobRow], active: &[JobRow], now: i64) -> Self {
        let est = |j: &JobRow| j.est_secs.filter(|e| *e > 0).map(|e| e as u64);
        let mut w = SoloWaits::default();
        for j in queued {
            w.ahead.entry(j.node_id.clone()).or_default().push(est(j));
            w.held.entry(j.node_id.clone()).or_default().push(est(j));
        }
        for j in active {
            if j.node_queued {
                w.ahead.entry(j.node_id.clone()).or_default().push(est(j));
                continue;
            }
            let left = match (j.started_at, est(j)) {
                (Some(s), Some(e)) => Some((s + e as i64 - now).max(0) as u64),
                (None, Some(e)) => Some(e),
                (_, None) => None,
            };
            w.left.entry(j.node_id.clone()).or_default().push(left);
        }
        w
    }

    /// A shared node: this server's jobs held here for `node_id` (not in
    /// its quote yet), their estimates (15 min each unknown) per slot.
    pub fn held_eta(&self, node_id: &str, parallel_max: u32) -> u64 {
        let sum: u64 = self.held.get(node_id).map(|v| v.iter().map(|e| e.unwrap_or(UNKNOWN_EST_SECS)).sum()).unwrap_or(0);
        sum / u64::from(parallel_max.max(1))
    }

    /// The estimated wait for a new job on `node_id`.
    pub fn eta(&self, node_id: &str, parallel_max: u32) -> u64 {
        let none = Vec::new();
        solo_eta(self.ahead.get(node_id).unwrap_or(&none), self.left.get(node_id).unwrap_or(&none), parallel_max)
    }
}

/// What placement reads of the database for one report, in one read
/// transaction ([`candidates_report_raw`]).
pub struct PlaceSnapshot {
    pub nodes: Vec<store::NodeRow>,
    /// The node that holds the job's `--cache` ([`cache_node`]).
    pub cache_at: Option<String>,
    pub permit: k2_core::compute::allow::Permit,
    pub pinned_only: k2_core::compute::pinned_only::Pinned,
    /// This workspace's grants, by node.
    pub grants: HashMap<String, store::GrantRow>,
    /// Every queued job, oldest first ([`store::queued_jobs`]).
    pub queued: Vec<JobRow>,
    /// Every job in an active state, oldest first ([`store::active_jobs`]).
    pub active: Vec<JobRow>,
}

impl PlaceSnapshot {
    pub fn read(conn: &rusqlite::Connection, w: &Want) -> Self {
        PlaceSnapshot {
            cache_at: w.cache.as_deref().and_then(|name| cache_node(conn, &w.ws_id, name)),
            // The workspace's node list (0.46.8): for every caller, named
            // node or auto, and every move.
            permit: k2_core::compute::allow::permit(conn, &w.ws_id),
            // The nodes that take only jobs that name them: for auto only.
            pinned_only: k2_core::compute::pinned_only::all(conn),
            nodes: store::list_nodes(conn, false),
            grants: store::grants(conn, None, Some(&w.ws_id)).into_iter().map(|g| (g.node_id.clone(), g)).collect(),
            queued: store::queued_jobs(conn),
            active: store::active_jobs(conn),
        }
    }
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

impl Cand {
    /// Now, Busy or Offline; `None` = online but paused, draining or
    /// unavailable (never a target).
    pub fn tier(&self, w: &Want) -> Option<Tier> {
        if self.starts_now(w) {
            Some(Tier::Now)
        } else if self.usable() {
            Some(Tier::Busy)
        } else if !self.online && self.offer.control == frames::Control::Active {
            // Offline, last seen taking jobs (one last seen paused or
            // draining is no target either).
            Some(Tier::Offline)
        } else {
            None
        }
    }
}

/// How warm a node's free slots are for this repo: 2 = one built this
/// exact commit, 1 = one built this repo (incremental), 0 = cold. With
/// cache generations (B2) the node's `trusted` head counts the same way:
/// 2 when it is this commit, 1 when it is an ancestor within
/// [`k2_core::compute::proto::gens::WARM_ANCESTRY_COMMITS`] commits.
pub fn warmth(c: &Cand, project_key: Option<&str>, commit: Option<&str>) -> u8 {
    slot_warmth(c, project_key, commit).max(gen_warmth(c, project_key, commit))
}

/// B2 (§4.1.6, BK-17): the node's `trusted` head for this project, scored.
/// The ancestry is asked of the workspace's repo, once per distinct
/// (head, commit) ([`ancestry_near`] memoizes); a head the repo doesn't
/// have counts as cold. A failed slot never scores here (BK-20): only a
/// generation a passing trusted check saved.
pub fn gen_warmth(c: &Cand, project_key: Option<&str>, commit: Option<&str>) -> u8 {
    let (Some(pk), Some(commit)) = (project_key, commit) else { return 0 };
    let mut best = 0;
    for (_, head) in c.gen_heads.iter().filter(|(k, _)| k == pk) {
        if head == commit {
            return 2;
        }
        if ancestry_near(&c.ws_id, head, commit) {
            best = 1;
        }
    }
    best
}

/// Is `head` an ancestor of `commit` within the warm distance, in the
/// workspace's repo? Memoized (a placement asks per candidate per tick).
pub fn ancestry_near(ws_id: &str, head: &str, commit: &str) -> bool {
    use std::collections::HashMap;
    static MEMO: std::sync::Mutex<Option<HashMap<(String, String, String), bool>>> = std::sync::Mutex::new(None);
    let key = (ws_id.to_string(), head.to_string(), commit.to_string());
    if let Some(v) = MEMO.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).get(&key) {
        return *v;
    }
    let near = (|| {
        if !k2_core::ci::is_sha(head) || !k2_core::ci::is_sha(commit) {
            return Some(false);
        }
        let ws = crate::compute_routes::ws_by_id(ws_id)?;
        let root = k2_core::compute::sync::repo_root(std::path::Path::new(&ws.path)).ok()?;
        let anc = k2_core::git_safe::git(&root).args(["merge-base", "--is-ancestor", head, commit]).output().ok()?;
        if !anc.status.success() {
            return Some(false);
        }
        let n = k2_core::git_safe::git(&root).args(["rev-list", "--count", &format!("{head}..{commit}")]).output().ok()?;
        let n: u32 = String::from_utf8_lossy(&n.stdout).trim().parse().ok()?;
        Some(n <= k2_core::compute::proto::gens::WARM_ANCESTRY_COMMITS)
    })()
    .unwrap_or(false);
    let mut g = MEMO.lock().unwrap_or_else(|p| p.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    if m.len() > 4096 {
        m.clear();
    }
    m.insert(key, near);
    near
}

fn slot_warmth(c: &Cand, project_key: Option<&str>, commit: Option<&str>) -> u8 {
    project_key
        .map(|pk| {
            let slots: Vec<_> = c.offer.slots.iter().filter(|s| s.project_key == pk && !s.busy).collect();
            if commit.is_some() && slots.iter().any(|s| s.last_sha.as_deref() == commit) {
                2
            } else {
                u8::from(!slots.is_empty())
            }
        })
        .unwrap_or(0)
}

/// Placement v2's warmth (§4.1 step 4, CE-1, CE-6): 3 = this workspace's
/// last job with the same `--cache NAME` ran here; 2/1 = a warm slot
/// ([`warmth`]); a node where every need is ready is worth at least 1 (a
/// warm slot's band, CE-1). v1 keeps [`warmth`].
pub fn warmth_v2(c: &Cand, w: &Want, project_key: Option<&str>, commit: Option<&str>) -> u8 {
    if c.holds_cache && w.cache.is_some() {
        return 3;
    }
    warmth(c, project_key, commit).max(u8::from(tools_ready(&c.needs(w))))
}

/// Pick one candidate with the ranking `computePlacement` names.
pub fn choose_by<'a>(
    policy: compute::PlacementPolicy,
    cands: &'a [Cand],
    w: &Want,
    prefer_not: &[String],
    project_key: Option<&str>,
    commit: Option<&str>,
) -> Option<(&'a Cand, Tier)> {
    rank(policy, cands, w, prefer_not, project_key, commit).into_iter().next()
}

/// Every usable candidate, best first, by the ranking `computePlacement`
/// names (`--plan`'s `ranking`, P2). The first is what [`choose_by`] picks.
pub fn rank<'a>(
    policy: compute::PlacementPolicy,
    cands: &'a [Cand],
    w: &Want,
    prefer_not: &[String],
    project_key: Option<&str>,
    commit: Option<&str>,
) -> Vec<(&'a Cand, Tier)> {
    match policy {
        compute::PlacementPolicy::V1 => rank_v1(cands, w, prefer_not, project_key, commit),
        compute::PlacementPolicy::V2 => rank_v2(cands, w, prefer_not, project_key, commit),
    }
}

/// Placement v1 (the 0.46.5 order, kept behind `computePlacement=v1`):
/// a node that can start the job now (warm slot first, then free CPU),
/// else the least-loaded online node, else the most recently seen offline
/// one. `prefer_not` (sibling shards, failed nodes) loses ties.
pub fn choose<'a>(cands: &'a [Cand], w: &Want, prefer_not: &[String], project_key: Option<&str>, commit: Option<&str>) -> Option<(&'a Cand, Tier)> {
    rank_v1(cands, w, prefer_not, project_key, commit).into_iter().next()
}

/// v1, best first. A stable sort on the score, highest first: among equal
/// scores the earlier candidate stays first (the old loop's `>`).
fn rank_v1<'a>(cands: &'a [Cand], w: &Want, prefer_not: &[String], project_key: Option<&str>, commit: Option<&str>) -> Vec<(&'a Cand, Tier)> {
    let mut scored: Vec<(&Cand, (Tier, bool, bool, i64, u8, i64, u64))> = Vec::new();
    for c in cands {
        let Some(tier) = c.tier(w) else { continue };
        let warmth = warmth(c, project_key, commit);
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
        // A node held for a moving job is the last place to wait.
        let not_held = tier != Tier::Busy || c.pending.holds == 0;
        scored.push((c, (tier, not_held, !prefer_not.contains(&c.node.id), eta, warmth, busy, tie)));
    }
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.into_iter().map(|(c, s)| (c, s.0)).collect()
}

/// The v2 sort key (smaller wins), PRD §4.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum V2Key {
    /// Starts now: not the node that holds the `--cache` (CE-6: that one
    /// first), a `sys:` need it can't install (CE-1: last), band (warmth
    /// worth one band), `prefer_not`, warmth, recent placements, this
    /// workspace's running jobs, load, free CPU, `sys:` support, name.
    Now(bool, bool, u8, bool, Reverse<u8>, u32, u32, u32, Reverse<u64>, bool, String),
    /// Busy everywhere: a node held for a job the mover is bringing there
    /// last (a job queued there would start first and bounce it),
    /// `prefer_not`, a `sys:` need it can't install, a known ETA (`(0,
    /// secs)`, quoted or estimated, lower first) before an unknown one
    /// (`(1, 0)`), fewer jobs there, warmth, name.
    Busy(bool, bool, bool, (u8, u64), u64, Reverse<u8>, String),
    /// Only offline: `prefer_not`, the most recently seen, name.
    Offline(bool, Reverse<i64>, String),
}

fn v2_key(c: &Cand, tier: Tier, w: &Want, prefer_not: &[String], project_key: Option<&str>, commit: Option<&str>) -> V2Key {
    let avoid = prefer_not.contains(&c.node.id);
    let warm = warmth_v2(c, w, project_key, commit);
    let name = c.node.name.clone();
    let unsupported = c.unsupported(w);
    match tier {
        Tier::Now => {
            let band = c.band(w).saturating_sub(u8::from(warm >= 1));
            let no_sys = w.wants_sys() && !c.offer.features.iter().any(|f| f == needs::FEATURE_SYSTEM_PACKAGES);
            let cache_elsewhere = !(c.holds_cache && w.cache.is_some());
            V2Key::Now(cache_elsewhere, unsupported, band, avoid, Reverse(warm), c.recent, c.ws_running(w), c.load_after(w), Reverse(c.free_now().1), no_sys, name)
        }
        Tier::Busy => {
            let eta = c.wait_eta(w).map(|(e, _)| (0, e)).unwrap_or((1, 0));
            V2Key::Busy(c.pending.holds > 0, avoid, unsupported, eta, u64::from(c.running_now()) + c.queued as u64, Reverse(warm), name)
        }
        Tier::Offline => V2Key::Offline(avoid, Reverse(c.node.last_seen.unwrap_or(0)), name),
    }
}

/// Placement v2 (prd-compute-auto-placement-v1 §4, the default): a node
/// that starts the job now, by load band after placing it (idle, light,
/// heavy) where a warm slot is worth one band (D2), then `prefer_not`,
/// warmth, this controller's recent placements there and this
/// workspace's running jobs (bursts and one builder's jobs spread), load,
/// free CPU, `sys:` support and name; else the shortest quoted wait; else
/// the most recently seen offline node.
pub fn choose_v2<'a>(cands: &'a [Cand], w: &Want, prefer_not: &[String], project_key: Option<&str>, commit: Option<&str>) -> Option<(&'a Cand, Tier)> {
    rank_v2(cands, w, prefer_not, project_key, commit).into_iter().next()
}

/// v2, best first (a stable sort: the first minimum is [`choose_v2`]'s).
fn rank_v2<'a>(cands: &'a [Cand], w: &Want, prefer_not: &[String], project_key: Option<&str>, commit: Option<&str>) -> Vec<(&'a Cand, Tier)> {
    let mut keyed: Vec<(&Cand, Tier, V2Key)> = cands.iter().filter_map(|c| c.tier(w).map(|t| (c, t, v2_key(c, t, w, prefer_not, project_key, commit)))).collect();
    keyed.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
    keyed.into_iter().map(|(c, t, _)| (c, t)).collect()
}

/// `--plan`'s `ranking` (P2, CE-2): the top three candidates, each with
/// its state, band, warmth, wait and needs. Granted node names and
/// node-wide numbers only (S2).
pub fn ranking_json(ranked: &[(&Cand, Tier)], w: &Want, project_key: Option<&str>, commit: Option<&str>) -> serde_json::Value {
    ranked
        .iter()
        .take(3)
        .map(|(c, t)| {
            let eta = c.wait_eta(w);
            serde_json::json!({
                "node": c.node.name,
                "state": match t { Tier::Now => "now", Tier::Busy => "busy", Tier::Offline => "offline" },
                "band": (*t == Tier::Now).then(|| c.band(w)),
                "warmth": warmth_v2(c, w, project_key, commit),
                "etaSecs": eta.map(|e| e.0),
                "etaBasis": eta.map(|e| e.1).unwrap_or("unknown"),
                "needs": c.needs(w).iter().map(NeedStatus::json).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>()
        .into()
}

// ── the placement record (§7) ────────────────────────────────────────

/// What a placement was made from, for its record.
pub struct Explain<'a> {
    pub policy: compute::PlacementPolicy,
    pub cands: &'a [Cand],
    pub skipped: &'a BTreeMap<&'static str, u32>,
    pub project_key: Option<&'a str>,
    pub commit: Option<&'a str>,
}

fn gb(b: u64) -> u64 {
    b >> 30
}

/// "24G" (whole GiB) or "1.5G".
pub fn mem_words(b: u64) -> String {
    if b % (1 << 30) == 0 {
        format!("{}G", b >> 30)
    } else {
        format!("{:.1}G", b as f64 / (1u64 << 30) as f64)
    }
}

/// "~6 min" / "~40s".
pub fn wait_words(secs: u64) -> String {
    if secs < 90 {
        format!("~{secs}s")
    } else {
        format!("~{} min", secs.div_ceil(60))
    }
}

/// "1 offline", "2 too small for --mem 24G", … (granted nodes only).
pub fn skip_words(code: &str, n: u32, w: &Want) -> String {
    let what = match code {
        skip::PAUSED => "paused".to_string(),
        skip::SET_ASIDE => "set aside after failures".to_string(),
        skip::QUEUE_FULL => "with your queue there full".to_string(),
        skip::OFFLINE => "offline".to_string(),
        skip::NODE_TOO_OLD => "with a k2-node too old for this job".to_string(),
        skip::OS_ARCH_LABEL => "of another --os/--arch/--label".to_string(),
        skip::TOO_SMALL => match (w.mem_bytes.filter(|_| !w.mem_defaulted), w.cpu_millis) {
            (Some(m), _) => format!("too small for --mem {}", mem_words(m)),
            (None, Some(c)) => format!("too small for --cpus {}", c as f64 / 1000.0),
            (None, None) => format!("too small for the default {}", mem_words(w.mem_bytes.unwrap_or(compute::DEFAULT_MEM_BYTES))),
        },
        skip::SECRET_NOT_COVERED => "not covered by the secret's grant (or its lane's rules: consent, Linux, isolation)".to_string(),
        skip::CI_NOT_ALLOWED => "without CI for you".to_string(),
        skip::TIMEOUT_OVER_GRANT => {
            format!("with a grant shorter than --timeout {}", w.timeout_secs.map(compute::message::duration).unwrap_or_default())
        }
        skip::SHARE_LIMIT => "over this server's share limits".to_string(),
        skip::EXCLUSIVE_NEEDS_GRANT => "without CI (which --exclusive needs) for you".to_string(),
        skip::UNAVAILABLE => "unavailable now (paused, draining, window, disk or lock)".to_string(),
        skip::NOT_ALLOWED => "off this workspace's node list".to_string(),
        skip::AVOIDED => "you avoid (--avoid)".to_string(),
        skip::DISTRO => format!("of another --distro{}", w.filter.distro.as_deref().map(|d| format!(" than {d}")).unwrap_or_default()),
        skip::NO_PACKAGE_MAPPING => "that can't install a sys: package you need (no name for their distro, or K2 never installs it)".to_string(),
        skip::PACKAGE_ASK => "that need their owner's approval for a sys: package".to_string(),
        skip::UNSUPPORTED_ON_OS => "Macs (no system packages)".to_string(),
        skip::NOT_ONLY => "off your --only list".to_string(),
        skip::PINNED_ONLY => "pinned-only (they run only jobs that name them)".to_string(),
        skip::NO_GPU => "with no GPU jobs for you (--gpu)".to_string(),
        skip::GPU_NEEDS_GRANT => "with a GPU, but your grant there doesn't say --gpu".to_string(),
        skip::GPU_MEM_NEEDS_UPDATE => "with GPU jobs, but a k2-node too old for --gpu-mem".to_string(),
        skip::TOOLS_UNSUPPORTED => "that can't provide a --needs (system Rust: no Rust targets)".to_string(),
        skip::JOB_NETWORK => crate::compute_job_network::skip_words(w),
        skip::USERNS_OWNER_ONLY => "where this server is a tenant (--userns is only for nodes it owns)".to_string(),
        skip::USERNS_NEEDS_UPDATE => "with a k2-node too old for --userns".to_string(),
        skip::USERNS_NOT_GRANTED => "where your grant doesn't say --userns".to_string(),
        other => other.to_string(),
    };
    format!("{n} {what}")
}

/// "6 nodes fit, 2 skipped (1 offline, 1 too small for --mem 24G)".
pub fn fits_words(considered: u32, skipped: &BTreeMap<String, u32>, w: &Want) -> String {
    let mut s = if considered == 1 { "1 node fits".to_string() } else { format!("{considered} nodes fit") };
    let total: u32 = skipped.values().sum();
    if total > 0 {
        let parts: Vec<String> = skipped.iter().map(|(k, n)| skip_words(k, *n, w)).collect();
        s.push_str(&format!(", {total} skipped ({})", parts.join(", ")));
    }
    s
}

/// "46/110 GB, 8/12 CPUs free" (node-wide, as `k2 compute nodes` shows).
fn free_words(c: &Cand) -> String {
    let (mem, cpu) = c.free_now();
    let mut parts = Vec::new();
    if c.offer.caps.mem_bytes > 0 {
        parts.push(format!("{}/{} GB", gb(mem), gb(c.offer.caps.mem_bytes)));
    }
    if c.offer.caps.cpu_millis > 0 {
        parts.push(format!("{}/{} CPUs", cpu / 1000, c.offer.caps.cpu_millis / 1000));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("{} free", parts.join(", "))
    }
}

/// The record of an auto placement on `c` (`tier`), from what it was
/// chosen among. Only granted node names, node-wide numbers and this
/// workspace's own slots and jobs (S2).
pub fn placement_record(c: &Cand, tier: Tier, w: &Want, ex: &Explain<'_>, now: i64) -> store::Placement {
    let unavailable = ex.cands.iter().filter(|x| x.tier(w).is_none()).count() as u32;
    let mut skipped: BTreeMap<String, u32> = ex.skipped.iter().map(|(k, n)| (k.to_string(), *n)).collect();
    if unavailable > 0 {
        *skipped.entry(skip::UNAVAILABLE.to_string()).or_insert(0) += unavailable;
    }
    let considered = ex.cands.len() as u32 - unavailable;
    let warm = warmth(c, ex.project_key, ex.commit);
    let free = free_words(c);
    // CE-1/CE-2: "tools ready" or what it installs first; then the node's
    // python3, curl and bash (0.46.17).
    let tools = tools_line(&c.needs(w), Some(&c.offer));
    // A Batch job on a node that isn't idle: why it waits, first (0.46.17).
    let batch = (w.tier == JobTier::Batch && !c.idle()).then(|| share::batch_wait_words(Some(c.running_now())));
    let with_tools = |s: String| match &tools {
        Some(t) => format!("{s} · {t}"),
        None => s,
    };
    let with_free = |s: String| {
        let s = with_tools(s);
        if free.is_empty() { s } else { format!("{s} · {free}") }
    };
    // A wait: "no node free now" says why, in each usable node's own
    // numbers (other servers' jobs included), so the wait can be trusted.
    let with_elsewhere = |s: String| {
        let usable: Vec<Cand> = ex.cands.iter().filter(|x| x.tier(w).is_some()).cloned().collect();
        match why_not::elsewhere_words(&usable, w, false) {
            Some(e) => s.replacen("no node free now", &e, 1),
            None => s,
        }
    };
    let (code, why, eta) = match tier {
        Tier::Now => {
            let eff = |x: &Cand| x.band(w).saturating_sub(u8::from(warmth_v2(x, w, ex.project_key, ex.commit) >= 1));
            let spread = ex.cands.iter().any(|x| {
                x.node.id != c.node.id && x.tier(w) == Some(Tier::Now) && eff(x) == eff(c) && (x.recent > c.recent || x.ws_running(w) > c.ws_running(w))
            });
            let cache_here = w.cache.as_deref().filter(|_| c.holds_cache);
            let (code, what) = if let Some(name) = cache_here {
                ("starts_now_cache", format!("holds cache {name}"))
            } else if warm >= 2 {
                ("starts_now_warm", "warm slot (this commit)".to_string())
            } else if warm == 1 {
                ("starts_now_warm", "warm slot (this repo: incremental)".to_string())
            } else if c.idle() {
                ("starts_now_idle", "idle".to_string())
            } else if spread {
                ("starts_now_spread", "spread (fewer of the latest jobs there)".to_string())
            } else {
                ("starts_now_least_loaded", "least loaded".to_string())
            };
            (code, with_free(format!("starts now · {what}")), None)
        }
        Tier::Busy => {
            let (code, why, eta) = match c.wait_eta(w) {
                Some((e, "quote")) => ("queued_quoted_eta", format!("no node free now; shortest wait {} (quoted) · it moves if another frees up first", wait_words(e)), Some(e)),
                Some((e, _)) => ("queued_est_wait", format!("no node free now; shortest wait {} (estimate) · it moves if another frees up first", wait_words(e)), Some(e)),
                None => ("queued_least_busy", format!("no node free now; least busy ({} running, {} queued) · it moves if another frees up first", c.running_now(), c.queued), None),
            };
            (code, with_elsewhere(with_tools(why)), eta)
        }
        Tier::Offline => ("queued_offline_only", "no matching node is online; queued on the one seen most recently · it moves to the first that comes online".to_string(), None),
    };
    let why = match batch {
        Some(b) => format!("{b} · {why}"),
        None => why,
    };
    store::Placement {
        mode: "auto".into(),
        policy: ex.policy.as_wire().into(),
        node: c.node.name.clone(),
        code: code.into(),
        fits: fits_words(considered, &skipped, w),
        why,
        starts_now: Some(tier == Tier::Now),
        eta_secs: eta,
        considered,
        skipped,
        at: now,
    }
}

/// The record of a job pinned to a named node.
pub fn pinned_record(node_name: &str, now: i64) -> store::Placement {
    store::Placement {
        mode: "pinned".into(),
        policy: compute::placement_policy().as_wire().into(),
        node: node_name.to_string(),
        code: "pinned".into(),
        why: "you named this node".into(),
        at: now,
        ..Default::default()
    }
}

// ── CPU-count hints (CE-15) ──────────────────────────────────────────

/// Env names K2 sets to the job's CPUs, unless the caller set them
/// (CE-15). GNU `nproc` honours `OMP_NUM_THREADS`. The PRD's
/// `K2_JOB_CPUS` needs k2-node: a node refuses any `K2_` name in a plan's
/// env, so the controller can't send it.
pub const CPU_HINT_VARS: &[&str] = &["CARGO_BUILD_JOBS", "RUST_TEST_THREADS", "NEXTEST_TEST_THREADS", "OMP_NUM_THREADS"];

/// The job's CPUs: `--cpus` rounded up, else the node's default
/// reservation (its CPUs ÷ its parallel slots, as k2-node reserves for a
/// job without `--cpus`), at least 1. `None`: no offer to read.
pub fn job_cpus(cpu_millis: Option<u64>, offer: Option<&Offer>) -> Option<u64> {
    match cpu_millis {
        Some(m) => Some(m.div_ceil(1000).max(1)),
        None => offer.filter(|o| o.caps.cpu_millis > 0).map(|o| (o.caps.cpu_millis / u64::from(o.parallel_max.max(1))).div_ceil(1000).max(1)),
    }
}

/// Set the hints in `env`: first drop `prev` (hints set for another
/// node), then each [`CPU_HINT_VARS`] name `env` and `skip` don't have.
/// Returns the names set.
pub fn set_cpu_hints(env: &mut BTreeMap<String, String>, prev: &[String], skip: &[String], cpus: Option<u64>) -> Vec<String> {
    for n in prev {
        env.remove(n);
    }
    let Some(c) = cpus else { return Vec::new() };
    let mut set = Vec::new();
    for n in CPU_HINT_VARS {
        if !env.contains_key(*n) && !skip.iter().any(|s| s == n) {
            env.insert(n.to_string(), c.to_string());
            set.push(n.to_string());
        }
    }
    set
}

/// The plan for `node`: the base plan's argv/env/source/needs/artifacts,
/// with this node's id, this node's grant limits, (when it enforces one)
/// the stall timeout, and the CPU hints for this node (CE-15, CE-A14).
pub fn plan_for_node(base: &Plan, ctl: &JobCtl, tier: JobTier, node: &NodeRow, grant: Option<&GrantRow>, offer: &Offer) -> Plan {
    let features = &offer.features;
    let max_secs = grant.map(|g| g.max_job_secs).unwrap_or(compute::DEFAULT_MAX_JOB_SECS);
    let disk_gb = grant.map(|g| g.max_disk_gb).unwrap_or(compute::DEFAULT_MAX_DISK_GB);
    let mut p = base.clone();
    p.node_id = node.id.clone();
    if !ctl.cpu_hints.is_empty() {
        let cpus = job_cpus(p.limits.cpu_millis, Some(offer));
        set_cpu_hints(&mut p.env, &ctl.cpu_hints, &p.secret_env, cpus);
    }
    p.limits.max_secs = ctl.timeout_secs.unwrap_or(max_secs).clamp(1, max_secs.max(1)) as u64;
    p.limits.disk_bytes = (disk_gb.max(1) as u64) << 30;
    p.stall_secs = stall_for(ctl, tier, features, p.limits.max_secs);
    p.provenance = crate::compute_provenance::ask(features);
    if features.iter().any(|f| f == pshare::FEATURE_QUEUE) {
        p.ws_max_running = Some(grant.map(|g| g.max_parallel.max(1) as u32).unwrap_or(1));
    } else {
        // Only to nodes offering `queue`: an older node would drop them and
        // the plan's digest wouldn't match (§10.2).
        strip_queue_fields(&mut p);
    }
    // The tier only to nodes offering `tiers` (T11), the same rule as the
    // queue fields; `set_queue_fields` then sets the job's own tier. Batch
    // only to a node offering `tiers2` (T27); else Normal.
    let tiers2 = offers_tiers2(features);
    p.tier = offers_tiers(features).then(|| base.tier_or_class().for_peer(tiers2));
    set_submitter_fields(&mut p, features, grant.map(|g| g.submitter_max_running.max(0) as u32).unwrap_or(0));
    // Protocol 7: the expanded system names only to a `pkg-map` node (an
    // older one would compute another digest and refuse).
    p.packages = if features.iter().any(|f| f == k2_core::compute::proto::pkgmap::FEATURE_PKG_MAP) { plan_packages(&p.needs) } else { Vec::new() };
    p
}

/// Does a node with these features take `Plan.tier` (protocol 5)?
pub fn offers_tiers(features: &[String]) -> bool {
    features.iter().any(|f| f == FEATURE_TIERS)
}

/// Does a node with these features take Batch and the submitter fields
/// (`tiers2`, protocol 6, prd-compute-job-tiers-v2)?
pub fn offers_tiers2(features: &[String]) -> bool {
    features.iter().any(|f| f == k2_core::compute::proto::tiers::FEATURE_TIERS2)
}

/// `Plan.submitter_max_running` only to a `tiers2` node with a queue (L3;
/// 0 = no cap, so absent); `Plan.submitter` stays as the plan has it there
/// and goes for any other node (an older one would refuse the digest).
fn set_submitter_fields(p: &mut Plan, features: &[String], submitter_cap: u32) {
    if offers_tiers2(features) && features.iter().any(|f| f == pshare::FEATURE_QUEUE) {
        p.submitter_max_running = (submitter_cap > 0).then_some(submitter_cap);
    } else {
        p.submitter = None;
        p.submitter_max_running = None;
    }
}

/// The grant's caps a plan carries to a node's queue, read at every send
/// (prd-compute-job-tiers-v2 L5, T23/T30).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrantCaps {
    /// `max_parallel` → `Plan.ws_max_running`.
    pub ws: u32,
    /// `submitter_max_running` → `Plan.submitter_max_running` (0 = off).
    pub submitter: u32,
}

impl GrantCaps {
    pub fn of(g: &GrantRow) -> Self {
        Self { ws: g.max_parallel.max(1) as u32, submitter: g.submitter_max_running.max(0) as u32 }
    }
}

/// The stored plan fitted to what the node understands right now (T11):
/// a tier only for a node offering `tiers`, the protocol-3 queue fields
/// only for one offering `queue`, a stall timeout only for one offering
/// `stall`. A plan stored for a newer node and sent to one rolled back to
/// an older k2-node would otherwise carry fields that node drops, so its
/// digest wouldn't match and it would fail `plan_mismatch`; a node updated
/// since gets the tier. `Some(plan)` when it changed; the caller rewrites
/// the queued row (JSON and digest) before sending.
///
/// `caps`: the workspace's grant on that node right now
/// (prd-compute-job-tiers-v2 L5, T23/T30). The plan carries its
/// `max_parallel` to a node offering `queue` (`Plan.ws_max_running`) and
/// its per-submitter cap to one offering `tiers2`; both are refreshed
/// before every send, so raising a grant reaches jobs submitted before it
/// (10-27: jobs kept a cap of 1 from submit). `None` leaves the plan's
/// values. The tier is the job row's (a retier reaches the plan here);
/// Batch only to a `tiers2` node (T27), and the submitter likewise.
pub fn fit_plan(plan: &Plan, j: &JobRow, features: &[String], caps: Option<GrantCaps>) -> Option<Plan> {
    let has = |f: &str| features.iter().any(|x| x == f);
    let mut p = plan.clone();
    // B2: cache generations only on a node offering them (an older node
    // would compute another digest); a secret job's fork (`none`) only on
    // a node that gives secret jobs their own uid.
    let gens_ok = has(k2_core::compute::proto::gens::FEATURE_CACHE_GENS)
        && (p.cache_commit != Some(k2_core::compute::proto::gens::CacheCommit::None) || has(k2_core::compute::proto::ci::FEATURE_JOB_UID));
    if !gens_ok {
        p.cache_commit = None;
        p.cache_ref = None;
    }
    let tiers2 = offers_tiers2(features);
    p.tier = offers_tiers(features).then(|| j.tier().for_peer(tiers2));
    if !has(pshare::FEATURE_QUEUE) {
        strip_queue_fields(&mut p);
    } else {
        // The class old nodes order by is the row's (a retier rewrites it,
        // clamped to the share's top class like at submit).
        p.class = Some(pshare::JobClass::parse(&j.class).unwrap_or_else(|| j.tier().class()));
        if let Some(c) = caps {
            p.ws_max_running = Some(c.ws.max(1));
        }
    }
    if tiers2 && has(pshare::FEATURE_QUEUE) {
        p.submitter = j.submitter.clone();
        if let Some(c) = caps {
            p.submitter_max_running = (c.submitter > 0).then_some(c.submitter);
        }
    } else {
        p.submitter = None;
        p.submitter_max_running = None;
    }
    if !has(FEATURE_STALL) {
        p.stall_secs = None;
    }
    // prd-compute-blacksmith-class-v1 R2 (§4.4.4): `--cpus N+M`'s M only to
    // a node offering `cpu-burst`; any other node keeps N (its plan and
    // digest as before). From the row, so a move to a newer node gets it.
    p.cpus_burst = j.ctl.cpus_burst.filter(|_| has(k2_core::compute::proto::gens::FEATURE_CPU_BURST));
    // Build statements (prd-compute-build-attestations-v1 §4.4): asked of
    // a node offering `provenance-v1`, gone for any other (its digest as
    // before, `gens::missing_feature` clear).
    p.provenance = crate::compute_provenance::ask(features);
    (p != *plan).then_some(p)
}

/// The protocol 3 plan fields a node keeps its queue by (§10.2), for a
/// node offering `queue`: from the job row (class, estimate) and its
/// session.
pub fn set_queue_fields(p: &mut Plan, j: &JobRow) {
    p.class = Some(pshare::JobClass::parse(&j.class).unwrap_or_default());
    if p.tier.is_some() {
        // A Batch tier stays only where the plan already carries one (a
        // `tiers2` node: `plan_for_node` lowered it to Normal elsewhere).
        let t = j.tier();
        p.tier = Some(if p.tier == Some(JobTier::Normal) && t == JobTier::Batch { JobTier::Normal } else { t });
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
    // `--avoid` names the node it waits on (never placed there; only a
    // release that races a placement can leave it so): leave for any.
    if j.ctl.any.as_ref().is_some_and(|f| f.avoids(&node.id, &node.name)) {
        return Some(("node_avoided", true));
    }
    if !compute_ws::is_online(&node.id) {
        return (now - offline_since(node, now) >= t.replace_after_secs).then_some(("node_offline", true));
    }
    // Its owner paused or drains it: a hard reason, as offline (a drain
    // for an update can take hours).
    if let Some(why) = node_held(&node.id) {
        return Some((why, true));
    }
    let d = decisions.entry(node.id.clone()).or_insert_with(|| compute_ws::decision_for(&node.id));
    let (_, why) = d.waiting.get(&j.id)?;
    if crate::compute_hold::BUSY.contains(&why.code.as_str()) {
        Some(("node_busy", false))
    } else {
        Some(("node_refused", true))
    }
}

/// The node is online but its owner paused, drains or stopped it (its
/// live offer's `control`): `node_paused`, `node_draining` or
/// `node_stopped`. `None` when it takes jobs, or isn't online.
pub fn node_held(node_id: &str) -> Option<&'static str> {
    let (online, _, live) = compute_ws::live_view(node_id);
    live.filter(|_| online).and_then(|o| held_code(o.control))
}

/// [`node_held`]'s code for a node's control.
pub fn held_code(c: frames::Control) -> Option<&'static str> {
    match c {
        frames::Control::Active => None,
        frames::Control::Paused => Some("node_paused"),
        frames::Control::Draining => Some("node_draining"),
        frames::Control::Stopped => Some("node_stopped"),
    }
}

/// Why nothing of this server's goes to a node now because of its
/// owner's control (paused, draining, stopped), as the node itself would
/// refuse it; the drain's author when the offer names one. A job pinned
/// to the node waits with this; an `--any` job leaves it.
pub fn control_wait(offer: &Offer) -> Option<frames::Refusal> {
    let mut r = refusal::control_refusal(offer.control)?;
    if let Some(by) = offer.control_by.as_deref().filter(|b| !b.is_empty()) {
        r.message = format!("{} (by {by})", r.message);
    }
    Some(r)
}

/// A node the job may move to at all: not one it avoids or keeps off its
/// `--only` list, and one that can give its `--needs` (the k2-node
/// features they need, no `sys:` name it can never install there).
/// [`candidates_report`] already leaves such nodes out; the mover checks
/// again so its decision stands on its own.
fn movable(c: &Cand, w: &Want) -> bool {
    if w.filter.avoids(&c.node.id, &c.node.name) || (!w.filter.only.is_empty() && !w.filter.only.contains(&c.node.id)) {
        return false;
    }
    if required_features(w).iter().any(|f| !c.offer.features.iter().any(|x| x == f)) {
        return false;
    }
    let states = c.needs(w);
    !states.iter().any(|s| s.state == NeedState::UnsupportedOnOs) && package_gaps(&states).1.is_empty()
}

/// The `--any` mover's choice for a waiting job (`cands`: every other
/// node): the best [`movable`] node, only when it can start the job now.
/// A pinned job never moves. `None`: it stays where it waits.
pub fn move_target<'a>(
    pinned: bool,
    policy: compute::PlacementPolicy,
    cands: &'a [Cand],
    w: &Want,
    prefer_not: &[String],
    project_key: Option<&str>,
    commit: Option<&str>,
) -> Option<&'a Cand> {
    if pinned {
        return None;
    }
    leave_target(false, policy, cands, w, prefer_not, project_key, commit)
}

/// [`move_target`] for an `--any` job: `hard` (its node is paused,
/// draining or gone) also takes a node where it only waits its turn;
/// else only one that starts it now.
pub fn leave_target<'a>(
    hard: bool,
    policy: compute::PlacementPolicy,
    cands: &'a [Cand],
    w: &Want,
    prefer_not: &[String],
    project_key: Option<&str>,
    commit: Option<&str>,
) -> Option<&'a Cand> {
    let ok: Vec<Cand> = cands.iter().filter(|c| movable(c, w)).cloned().collect();
    let (c, tier) = choose_by(policy, &ok, w, prefer_not, project_key, commit)?;
    if !(tier == Tier::Now || (hard && tier == Tier::Busy)) {
        return None;
    }
    cands.iter().find(|x| x.node.id == c.node.id)
}

/// The line a move leaves on the job ("moved from k2n7-vin: k2n12-vin can
/// start it now · …"): where it came from, and why the new node.
pub fn moved_why(from: &str, to: &str, tier: Tier, why: &str) -> String {
    match tier {
        Tier::Now => format!("moved from {from}: {to} can start it now{}", why.strip_prefix("starts now").unwrap_or(why)),
        _ => format!("moved from {from}: {why}"),
    }
}

/// Move a queued job to another node now, if one fits. `hard`: any usable
/// online node; else only one that starts it now. Returns the new node.
pub fn try_move(j: &JobRow, avoid: &[String], hard: bool, why: &str) -> Option<String> {
    let plan = j.plan()?;
    let now = compute::now();
    let w = Want::of_job(j, &plan);
    let mut report = candidates_report(&w, std::slice::from_ref(&j.node_id), now);
    report.cands.retain(|c| movable(c, &w));
    let pk = plan.src.as_ref().map(|s| s.project_key.clone());
    let commit = plan.src.as_ref().map(|s| s.commit.clone());
    let policy = compute::placement_policy();
    let (c, tier) = choose_by(policy, &report.cands, &w, avoid, pk.as_deref(), commit.as_deref())?;
    let ok = match tier {
        Tier::Now => true,
        Tier::Busy => hard,
        Tier::Offline => false,
    };
    if !ok {
        return None;
    }
    let mut new_plan = plan_for_node(&plan, &j.ctl, j.tier(), &c.node, c.grant.as_ref(), &c.offer);
    if share::queue_on(&c.offer) {
        set_queue_fields(&mut new_plan, j);
    }
    // The record follows the job (§7): `moved_<why>`, the original
    // `considered` kept.
    let mut ctl = j.ctl.clone();
    let ex = Explain { policy, cands: &report.cands, skipped: &report.skipped, project_key: pk.as_deref(), commit: commit.as_deref() };
    let mut rec = placement_record(c, tier, &w, &ex, now);
    rec.code = format!("moved_{why}");
    if let Some(before) = j.ctl.placement.as_ref() {
        rec.considered = before.considered;
    }
    let moved = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let from = store::node_by_id(&conn, &j.node_id).map(|n| n.name).unwrap_or_else(|| j.node_id.clone());
        rec.why = moved_why(&from, &c.node.name, tier, &rec.why);
        ctl.placement = Some(rec);
        let moved = store::move_queued_job(&conn, &j.id, &new_plan, &ctl, &format!("moved_{why}")).unwrap_or(false);
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
    note_placed(&c.node.id, now);
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
    // 0313: its `--env` values are gone (never run with a marker).
    if j.plan().is_some_and(|p| !store::env_dropped(&p).is_empty()) {
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
        // A pooled read: the sweep's snapshot never holds the writer's lock.
        k2_core::db::read(store::active_jobs)
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

    // Queued work: deadlines, then moves, held jobs in dispatch order
    // (CI, Priority, Standard, Batch; oldest first in each: 0.46.17).
    let mut queued = {
        // A pooled read: the sweep's snapshot never holds the writer's lock.
        k2_core::db::read(store::queued_jobs)
    };
    crate::compute_hold::dispatch_order(&mut queued);
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
            // CE-6: a job waiting for its cache stays while only busy.
            if !hard && held_for_cache(&j, since, now) {
                continue;
            }
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
        // A pull back (`k2 compute move`, 0.46.17): it is back as asked.
        Some(pending::PULL) => {
            crate::compute_move::pulled_back(&n.id, j);
            compute_ws::notify_change();
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
    // Holds of withdraws no longer in flight end here.
    keep_holds(&held.iter().filter(|j| j.reason.as_deref() == Some(pending::WITHDRAW)).map(|j| j.id.clone()).collect());
    // Order (oldest first, as `node_queued_jobs` lists them): while an
    // older job of a workspace waits out its back-off on a node, the
    // workspace's newer jobs there don't move past it.
    let mut cooling: HashSet<(String, String)> = HashSet::new();
    for j in held {
        let Some(n) = nodes.get(&j.node_id) else { continue };
        let online = compute_ws::is_online(&n.id);
        if j.reason.as_deref() == Some("cancel_requested") {
            continue; // a forced cancel: the node's end, or node-lost
        }
        if pending::any(j.reason.as_deref()) {
            // Waiting for the node's answer, never forever (0.46.17).
            chase_ask(&j, n, online, now, awake);
            continue;
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
                    store::set_pending_ask(&conn, &j.id, j.generation, pending::TIMEOUT, j.detail.as_deref(), now);
                    drop(conn);
                    compute_ws::send_cancel_if_not_started(&n.id, &j.id, j.generation, frames::CANCEL_WITHDRAW);
                }
                continue;
            }
        }
        if j.ctl.any.is_none() || !online {
            continue;
        }
        let turn = (j.workspace_id.clone(), j.node_id.clone());
        if cooling.contains(&turn) {
            continue;
        }
        // The back-off after a bounce (no lifetime cap; this supersedes
        // V71's "at most twice in all").
        if !share::may_withdraw(j.withdraws, j.withdrawn_at, now) {
            cooling.insert(turn);
            continue;
        }
        if held_for_cache(&j, since, now) {
            continue;
        }
        let Some(plan) = j.plan() else { continue };
        let w = Want::of_job(&j, &plan);
        let cands = candidates(&w, std::slice::from_ref(&j.node_id), now);
        let pk = plan.src.as_ref().map(|s| s.project_key.clone());
        let commit = plan.src.as_ref().map(|s| s.commit.clone());
        // A node its owner paused or drains starts nothing until it is
        // resumed: the job leaves for any node where it can wait its turn;
        // else only for one that starts it now.
        let hard = node_held(&n.id).is_some();
        let Some(target) = leave_target(hard, compute::placement_policy(), &cands, &w, &[], pk.as_deref(), commit.as_deref()) else { continue };
        // Hold the target's place while the node answers.
        note_hold(&target.node.id, &j.id, w.mem_on(&target.offer), w.cpu_millis, now);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::set_pending_ask(&conn, &j.id, j.generation, pending::WITHDRAW, None, now);
            store::note_withdraw(&conn, &j.id, now);
            store::record_event(
                &conn,
                "controller",
                "withdraw_asked",
                Some(&n.id),
                Some(&j.workspace_id),
                Some(&j.id),
                serde_json::json!({"generation": j.generation, "withdraws": j.withdraws + 1, "to": target.node.id}),
            );
        }
        if !compute_ws::send_cancel_if_not_started(&n.id, &j.id, j.generation, frames::CANCEL_WITHDRAW) {
            // Not sent (the node just dropped): no withdraw in flight.
            drop_hold(&j.id);
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::set_job_reason(&conn, &j.id, Some(pshare::codes::NODE_QUEUED), None);
        }
    }
}

/// A node-queued job whose node owes an answer to a "give it back" ask
/// (its `reason` is a pending marker). Before 0.46.17 the sweeper waited
/// for that answer with no limit, so a lost answer left the job `assigned`
/// forever while the node stayed connected. Now, while the node is
/// connected, the ask goes once more after [`share::ASK_RESEND_SECS`] and
/// is settled by the controller after [`share::ASK_SETTLE_SECS`]
/// ([`compute_ws::settle_unanswered`]). After this machine wakes, the wait
/// counts from the wake. An offline node is the node-lost rule's (its
/// reattach asks again and restarts the wait).
///
/// A pending ask never blocks `--queue-timeout`: when the job's queue time
/// runs out while a move, pull or re-send waits, the ask becomes the
/// timeout's (same send time), so it fails at the latest when that ask is
/// settled.
fn chase_ask(j: &JobRow, n: &NodeRow, online: bool, now: i64, awake: i64) {
    let t = retry::tuning();
    let marker = j.reason.as_deref().unwrap_or_default();
    let since = if j.ctl.queued_at > 0 { j.ctl.queued_at } else { j.created_at };
    let timeout_due = j.ctl.queue_timeout_secs.filter(|l| *l > 0).is_some_and(|limit| now - since >= limit && now - awake >= 2 * t.replace_after_secs);
    if timeout_due && matches!(marker, pending::WITHDRAW | pending::RESEND | pending::PULL) {
        if !online {
            // The node can't start it while we're not attached, and drops
            // it on the next attach (V47): fail it now.
            fail_queue_timeout(j, &n.name, now);
            return;
        }
        let at = store::NodeAsk::of(j).map_or(now, |a| a.at);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::set_job_reason(&conn, &j.id, Some(pending::TIMEOUT), j.detail.as_deref());
            store::stamp_node_ask(&conn, &j.id, j.generation, pending::TIMEOUT, at, false);
        }
        drop_hold(&j.id);
        compute_ws::send_cancel_if_not_started(&n.id, &j.id, j.generation, frames::CANCEL_WITHDRAW);
        return;
    }
    if !online {
        return;
    }
    // An ask from before 0.46.17 (or one whose stamp a write lost) is
    // counted from now.
    let Some(ask) = store::NodeAsk::of(j).cloned() else {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::stamp_node_ask(&conn, &j.id, j.generation, marker, now, false);
        return;
    };
    let waited = now - ask.at.max(awake);
    if waited >= share::ASK_SETTLE_SECS {
        compute_ws::settle_unanswered(&n.id, j, &ask, now);
    } else if waited >= share::ASK_RESEND_SECS && !ask.resent {
        compute_ws::ask_again(&n.id, j, &ask, now);
    }
}

/// A waiting job's mover state for its own workspace (jobs, get, queue):
/// pinned or auto, and for an auto job the withdraws that bounced in a row
/// and when the mover may try again. `None`: not waiting.
pub fn move_state_json(j: &JobRow, now: i64) -> Option<serde_json::Value> {
    if !(j.state == "queued" || j.node_queued) {
        return None;
    }
    // `k2 compute move` asked its node for it back (0.46.17).
    let pulling = j.reason.as_deref() == Some(pending::PULL);
    if j.ctl.any.is_none() && !pulling {
        return Some(serde_json::json!({"auto": false, "note": "pinned: it waits for the node it was named on (k2 compute move <job> --auto lets K2 move it)"}));
    }
    let wait = share::withdraw_wait(j.withdraws, j.withdrawn_at, now);
    let moving = pulling || j.reason.as_deref() == Some(pending::WITHDRAW);
    let note = if pulling {
        crate::compute_move::pulling_note(j)
    } else if moving {
        match store::NodeAsk::of(j) {
            Some(a) => format!(
                "moving: its node was asked to give it back {} ago, for a node that can start it now; {k2} takes it back itself at {}",
                compute::message::duration(now - a.at),
                k2_core::compute::attempts::ask_settle_words(),
                k2 = k2_core::brand::PRODUCT_NAME
            ),
            None => "moving: its node was asked to give it back for a node that can start it now".to_string(),
        }
    } else if j.withdraws > 0 && wait > 0 {
        format!("tried to move it {} time(s) in a row; the node it was meant for filled first; next try in {wait}s", j.withdraws)
    } else {
        "K2 moves it when another node can start it now".to_string()
    };
    Some(serde_json::json!({
        "auto": j.ctl.any.is_some(),
        "bounces": j.withdraws,
        "lastTryAt": j.withdrawn_at,
        "nextTryInSecs": wait,
        "moving": moving,
        "note": note,
    }))
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
#[path = "compute_place_tests.rs"]
mod tests;
