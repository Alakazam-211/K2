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

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};
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
            tier: self.tier, submitter: None,
        }
    }

    /// The command needs a system package (`--needs sys:…`).
    fn wants_sys(&self) -> bool {
        self.needs.iter().any(|n| n.starts_with("sys:"))
    }
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

/// What is pending on `node_id`, given its current offer revision (`None`
/// = no live offer: only time drops entries).
pub fn pending_for(node_id: &str, rev: Option<u64>, now: i64) -> Pending {
    with_map(&PENDING, |m| {
        let Some(v) = m.get_mut(node_id) else { return Pending::default() };
        v.retain(|p| now - p.at < PENDING_SECS && rev.is_none_or(|r| r <= p.rev));
        v.iter().fold(Pending::default(), |a, p| Pending { mem_bytes: a.mem_bytes + p.mem, cpu_millis: a.cpu_millis + p.cpu, jobs: a.jobs + 1 })
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

    /// Node-wide free memory and CPU: the last offer's (it counts every
    /// server's running jobs) minus the asks pending since (§3).
    pub fn free_now(&self) -> (u64, u64) {
        (self.offer.free.mem_bytes.saturating_sub(self.pending.mem_bytes), self.offer.free.cpu_millis.saturating_sub(self.pending.cpu_millis))
    }

    /// The job's `--mem` and `--cpus` fit in [`Cand::free_now`].
    fn fits_free(&self, w: &Want) -> bool {
        let (mem, cpu) = self.free_now();
        w.mem_bytes.is_none_or(|m| m <= mem) && w.cpu_millis.is_none_or(|c| c <= cpu)
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
            return self.online
                && share::starts_now(v, w.cpu_millis, w.mem_bytes)
                && self.fits_free(w)
                && !(w.exclusive && self.running_now() > 0)
                && self.ws_running(w) < self.grant_cap();
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
        let probe = refusal::JobAsk { workspace_id: "", cpu_millis: None, mem_bytes: None, disk_bytes: 0, exclusive: false, needs_git: false };
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
        let um = used(mem, w.mem_bytes.unwrap_or(0), self.offer.caps.mem_bytes);
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
    /// Online but paused, draining, outside its window, disk low, locked.
    pub const UNAVAILABLE: &str = "unavailable";
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
}

/// Every node that could take the job, except `exclude`.
pub fn candidates(w: &Want, exclude: &[String], now: i64) -> Vec<Cand> {
    candidates_report(w, exclude, now).cands
}

/// [`candidates`] with the skipped counts. A node this workspace holds no
/// grant on is never listed or counted in auto mode (S2): an agent never
/// learns it exists.
pub fn candidates_report(w: &Want, exclude: &[String], now: i64) -> CandReport {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_nodes(&conn, false)
    };
    let feats = required_features(w);
    let mut out = CandReport::default();
    let skipped = |out: &mut CandReport, code: &'static str| *out.skipped.entry(code).or_insert(0) += 1;
    for n in rows.into_iter().filter(|n| n.state == "active") {
        if exclude.contains(&n.id) {
            continue;
        }
        let (grant, queued_ws, queued_all, load) = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let g = store::grant(&conn, &n.id, &w.ws_id);
            let (q, load) = compute_ws::queued_view(&conn, &n.id);
            (g, store::queued_count(&conn, &n.id, &w.ws_id), q.len(), load)
        };
        // F2 (D1): auto never crosses grants; the owner/Admin bypass is for
        // a node they name.
        if grant.is_none() && (w.auto || !w.admin) {
            continue;
        }
        out.visible += 1;
        if n.controller_pause.is_some() {
            skipped(&mut out, skip::PAUSED);
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
        let (online, _, live) = compute_ws::live_view(&n.id);
        let Some(offer) = live.filter(|_| online).or_else(|| n.last_offer.clone()) else {
            skipped(&mut out, skip::OFFLINE);
            continue;
        };
        if feats.iter().any(|f| !offer.features.iter().any(|x| x == f)) {
            skipped(&mut out, skip::NODE_TOO_OLD);
            continue;
        }
        let label = |k: &str| offer.labels.get(k).map(String::as_str);
        if w.filter.os.as_deref().is_some_and(|o| label("os") != Some(o))
            || w.filter.arch.as_deref().is_some_and(|a| label("arch") != Some(a))
            || w.filter.labels.iter().any(|(k, x)| label(k) != Some(x.as_str()))
        {
            skipped(&mut out, skip::OS_ARCH_LABEL);
            continue;
        }
        // Over the node's caps (--cpus/--mem): it would never start there
        // (compute DX slice A: never queued where it can't ever fit).
        if scheduler::never_fits(&offer, &w.probe()).is_some() {
            skipped(&mut out, skip::TOO_SMALL);
            continue;
        }
        // Secret-env (SE6): only nodes a grant covers for every secret
        // (`*` covers only nodes this server owns, Q2).
        if !w.secret_ids.is_empty() && !crate::compute_secrets::covers_all(&w.ws_id, &n, &w.secret_ids) {
            skipped(&mut out, skip::SECRET_NOT_COVERED);
            continue;
        }
        // `--any --tier ci`: only where this workspace may send CI (T12).
        if w.tier == JobTier::Ci && !ci_allowed(w.admin, grant.as_ref(), Some(&offer)) {
            skipped(&mut out, skip::CI_NOT_ALLOWED);
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
        let pending = pending_for(&n.id, online.then_some(offer.revision), now);
        let recent = recent_for(&n.id, now);
        out.cands.push(Cand { node: n, online, offer, load, queued: queued_all, grant, pending, recent });
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

impl Cand {
    /// Now, Busy or Offline; `None` = online but paused, draining or
    /// unavailable (never a target).
    pub fn tier(&self, w: &Want) -> Option<Tier> {
        if self.starts_now(w) {
            Some(Tier::Now)
        } else if self.usable() {
            Some(Tier::Busy)
        } else if !self.online {
            Some(Tier::Offline)
        } else {
            None
        }
    }
}

/// How warm a node's free slots are for this repo: 2 = one built this
/// exact commit, 1 = one built this repo (incremental), 0 = cold.
pub fn warmth(c: &Cand, project_key: Option<&str>, commit: Option<&str>) -> u8 {
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

/// Pick one candidate with the ranking `computePlacement` names.
pub fn choose_by<'a>(
    policy: compute::PlacementPolicy,
    cands: &'a [Cand],
    w: &Want,
    prefer_not: &[String],
    project_key: Option<&str>,
    commit: Option<&str>,
) -> Option<(&'a Cand, Tier)> {
    match policy {
        compute::PlacementPolicy::V1 => choose(cands, w, prefer_not, project_key, commit),
        compute::PlacementPolicy::V2 => choose_v2(cands, w, prefer_not, project_key, commit),
    }
}

/// Placement v1 (the 0.46.5 order, kept behind `computePlacement=v1`):
/// a node that can start the job now (warm slot first, then free CPU),
/// else the least-loaded online node, else the most recently seen offline
/// one. `prefer_not` (sibling shards, failed nodes) loses ties.
pub fn choose<'a>(cands: &'a [Cand], w: &Want, prefer_not: &[String], project_key: Option<&str>, commit: Option<&str>) -> Option<(&'a Cand, Tier)> {
    let mut best: Option<(&Cand, (Tier, bool, i64, u8, i64, u64))> = None;
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
        let score = (tier, !prefer_not.contains(&c.node.id), eta, warmth, busy, tie);
        if best.as_ref().is_none_or(|(_, b)| score > *b) {
            best = Some((c, score));
        }
    }
    best.map(|(c, s)| (c, s.0))
}

/// The v2 sort key (smaller wins), PRD §4.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum V2Key {
    /// Starts now: band (a warm slot worth one band), `prefer_not`,
    /// warmth, recent placements, this workspace's running jobs, load,
    /// free CPU, `sys:` support, name.
    Now(u8, bool, Reverse<u8>, u32, u32, u32, Reverse<u64>, bool, String),
    /// Busy everywhere: `prefer_not`, a quoted ETA (`(0, secs)`, lower
    /// first) before an unknown one (`(1, 0)`), fewer jobs there, warmth,
    /// name.
    Busy(bool, (u8, u64), u64, Reverse<u8>, String),
    /// Only offline: `prefer_not`, the most recently seen, name.
    Offline(bool, Reverse<i64>, String),
}

fn v2_key(c: &Cand, tier: Tier, w: &Want, prefer_not: &[String], project_key: Option<&str>, commit: Option<&str>) -> V2Key {
    let avoid = prefer_not.contains(&c.node.id);
    let warm = warmth(c, project_key, commit);
    let name = c.node.name.clone();
    match tier {
        Tier::Now => {
            let band = c.band(w).saturating_sub(u8::from(warm >= 1));
            let no_sys = w.wants_sys() && !c.offer.features.iter().any(|f| f == needs::FEATURE_SYSTEM_PACKAGES);
            V2Key::Now(band, avoid, Reverse(warm), c.recent, c.ws_running(w), c.load_after(w), Reverse(c.free_now().1), no_sys, name)
        }
        Tier::Busy => {
            let eta = c.eta(w).map(|e| (0, e)).unwrap_or((1, 0));
            V2Key::Busy(avoid, eta, u64::from(c.running_now()) + c.queued as u64, Reverse(warm), name)
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
    cands
        .iter()
        .filter_map(|c| c.tier(w).map(|t| (c, t, v2_key(c, t, w, prefer_not, project_key, commit))))
        .min_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(&b.2)))
        .map(|(c, t, _)| (c, t))
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
fn wait_words(secs: u64) -> String {
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
        skip::SECRET_NOT_COVERED => "not covered by the secret's grant".to_string(),
        skip::CI_NOT_ALLOWED => "without CI for you".to_string(),
        skip::TIMEOUT_OVER_GRANT => {
            format!("with a grant shorter than --timeout {}", w.timeout_secs.map(compute::message::duration).unwrap_or_default())
        }
        skip::SHARE_LIMIT => "over this server's share limits".to_string(),
        skip::UNAVAILABLE => "unavailable now (paused, draining, window, disk or lock)".to_string(),
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
    let with_free = |s: String| if free.is_empty() { s } else { format!("{s} · {free}") };
    let (code, why, eta) = match tier {
        Tier::Now => {
            let eff = |x: &Cand| x.band(w).saturating_sub(u8::from(warmth(x, ex.project_key, ex.commit) >= 1));
            let spread = ex.cands.iter().any(|x| {
                x.node.id != c.node.id && x.tier(w) == Some(Tier::Now) && eff(x) == eff(c) && (x.recent > c.recent || x.ws_running(w) > c.ws_running(w))
            });
            let (code, what) = if warm >= 2 {
                ("starts_now_warm", "warm slot (this commit)")
            } else if warm == 1 {
                ("starts_now_warm", "warm slot (this repo: incremental)")
            } else if c.idle() {
                ("starts_now_idle", "idle")
            } else if spread {
                ("starts_now_spread", "spread (fewer of the latest jobs there)")
            } else {
                ("starts_now_least_loaded", "least loaded")
            };
            (code, with_free(format!("starts now · {what}")), None)
        }
        Tier::Busy => match c.eta(w) {
            Some(e) => ("queued_quoted_eta", format!("no node free now; shortest wait {} (quoted) · it moves if another frees up first", wait_words(e)), Some(e)),
            None => (
                "queued_least_busy",
                format!("no node free now; least busy ({} running, {} queued) · it moves if another frees up first", c.running_now(), c.queued),
                None,
            ),
        },
        Tier::Offline => ("queued_offline_only", "no matching node is online; queued on the one seen most recently · it moves to the first that comes online".to_string(), None),
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
    // queue fields; `set_queue_fields` then sets the job's own tier. Batch
    // only to a node offering `tiers2` (T27); else Normal.
    let tiers2 = offers_tiers2(features);
    p.tier = offers_tiers(features).then(|| base.tier_or_class().for_peer(tiers2));
    set_submitter_fields(&mut p, features, grant.map(|g| g.submitter_max_running.max(0) as u32).unwrap_or(0));
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
    let report = candidates_report(&w, std::slice::from_ref(&j.node_id), now);
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
    let mut new_plan = plan_for_node(&plan, &j.ctl, &c.node, c.grant.as_ref(), &c.offer.features);
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
    ctl.placement = Some(rec);
    let moved = {
        let db = k2_core::db::shared();
        let conn = db.lock();
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
        if !matches!(choose_by(compute::placement_policy(), &cands, &w, &[], pk.as_deref(), commit.as_deref()), Some((_, Tier::Now))) {
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

    // ── auto-placement fixtures (prd-compute-auto-placement-v1 §10) ──

    const GB: u64 = 1 << 30;
    const ME: &str = "ws-me";

    fn node_row(id: &str) -> NodeRow {
        NodeRow {
            id: id.into(),
            name: id.into(),
            fingerprint: String::new(),
            public_key_pem: String::new(),
            os: None,
            arch: None,
            labels: Default::default(),
            state: "active".into(),
            enrolled_via: "code".into(),
            enrolled_by: None,
            enrolled_at: 1,
            sas: None,
            confirmed_by: None,
            confirmed_at: None,
            revoked_by: None,
            revoked_at: None,
            controller_pause: None,
            controller_pause_by: None,
            controller_pause_at: None,
            last_seen: Some(100),
            last_route: None,
            protocol: Some(6),
            node_version: None,
            routes: vec![],
            last_offer: None,
            role: "owner".into(),
            shared: false,
            owner_label: None,
            share: None,
            share_at: None,
        }
    }

    fn grant_row(node: &str, max_job_secs: i64, max_parallel: i64) -> GrantRow {
        GrantRow {
            node_id: node.into(),
            workspace_id: ME.into(),
            scopes: String::new(),
            max_job_secs,
            max_disk_gb: 60,
            max_parallel,
            max_queued: 10,
            warm_at: None,
            expires_at: None,
            granted_by: "owner".into(),
            granted_at: 1,
            allow_ci: false,
            submitter_max_running: 0,
        }
    }

    /// A solo node: `free_gb` of `caps_gb` memory free, 8 CPUs, `running`
    /// of `parallel` slots in use (the registry agrees, another workspace's).
    fn cand(id: &str, free_gb: u64, caps_gb: u64, running: u32, parallel: u32) -> Cand {
        let offer = Offer {
            share: None,
            revision: 1,
            control: frames::Control::Active,
            control_by: None,
            control_at: None,
            availability: frames::Availability { ok: true, reasons: vec![] },
            caps: frames::Caps { cpu_millis: 8000, mem_bytes: caps_gb * GB, disk_budget_bytes: 100 * GB, hard: true },
            free: frames::Free { cpu_millis: 8000 - u64::from(running) * 1000, mem_bytes: free_gb * GB, disk_free_bytes: 500 * GB },
            slots: vec![],
            running,
            parallel_max: parallel,
            refusal: None,
            labels: [("os".to_string(), "linux".to_string())].into(),
            tools: [("git".to_string(), "2.47".to_string())].into(),
            protocol: 6,
            boot_id: "b".into(),
            ledger_id: "l".into(),
            node_version: "0.46.6".into(),
            features: vec![],
        };
        let mut load = scheduler::Load::default();
        if running > 0 {
            load.running_by_workspace.insert("ws-other-secret".into(), running);
        }
        Cand { node: node_row(id), online: true, offer, load, queued: 0, grant: Some(grant_row(id, 7200, 2)), pending: Pending::default(), recent: 0 }
    }

    /// Make `c` a node shared with another server (this server a tenant):
    /// `quote` = (starts now, ETA) for a 2 GB ask, or none.
    fn shared(mut c: Cand, quote: Option<(bool, Option<u64>)>) -> Cand {
        c.offer.features.push(pshare::FEATURE_QUEUE.to_string());
        c.offer.share = Some(ShareView {
            role: pshare::Role::Tenant,
            slot: 1,
            shared: true,
            servers: 2,
            owner_label: Some("other.k2.dev".into()),
            state: pshare::ServerStatus::Active,
            limits: pshare::ShareLimits::new_server(pshare::Trust::Own, false),
            running: 0,
            queued: 0,
            cpu_secs_today: 0,
            expires_in_secs: None,
            skew_secs: None,
            quotes: quote
                .map(|(now, eta)| vec![pshare::Quote { cpu_millis: 0, mem_bytes: 2 * GB, starts_now: now, eta_secs: eta, eta_p90_secs: None, blocked_by: None }])
                .unwrap_or_default(),
            priority: None,
        });
        c
    }

    fn warm(mut c: Cand, sha: &str) -> Cand {
        c.offer.slots.push(frames::SlotInfo { project_key: "pk".into(), slot: 0, busy: false, last_sha: Some(sha.into()) });
        c
    }

    fn want(mem_gb: u64) -> Want {
        Want {
            ws_id: ME.into(),
            admin: false,
            auto: true,
            filter: AnyFilter::default(),
            needs: vec![],
            artifacts: false,
            exclusive: false,
            needs_git: false,
            cpu_millis: None,
            mem_bytes: Some(mem_gb * GB),
            mem_defaulted: false,
            timeout_secs: None,
            stall: false,
            secret_ids: vec![],
            tier: JobTier::Normal,
            cache: false,
            multi_repo: false,
        }
    }

    fn pick(policy: compute::PlacementPolicy, cands: &[Cand], w: &Want) -> (String, Tier) {
        let (c, t) = choose_by(policy, cands, w, &[], Some("pk"), Some("c1")).expect("a pick");
        (c.node.id.clone(), t)
    }

    /// The whole order a ranking gives (pick, drop it, pick again).
    fn order(policy: compute::PlacementPolicy, cands: &[Cand], w: &Want) -> Vec<String> {
        let mut left = cands.to_vec();
        let mut out = Vec::new();
        while let Some((c, _)) = choose_by(policy, &left, w, &[], Some("pk"), Some("c1")) {
            let id = c.node.id.clone();
            out.push(id.clone());
            left.retain(|x| x.node.id != id);
        }
        out
    }

    use compute::PlacementPolicy::{V1, V2};

    /// D2 (§10 unit 1): a warm slot is worth one load band. A warm, lightly
    /// loaded node ties with a cold idle one and warmth wins; a warm,
    /// heavily loaded node loses to a cold idle one.
    #[test]
    fn a_warm_slot_is_worth_one_band() {
        let w = want(2);
        let warm_light = warm(cand("warm-light", 28, 32, 1, 4), "c1");
        let warm_heavy = warm(cand("warm-heavy", 4, 32, 1, 2), "c1");
        let cold_idle = cand("cold-idle", 32, 32, 0, 4);
        assert_eq!((warm_light.band(&w), warm_heavy.band(&w), cold_idle.band(&w)), (1, 2, 0));
        assert_eq!(pick(V2, &[cold_idle.clone(), warm_light.clone()], &w), ("warm-light".into(), Tier::Now), "warm + light ties idle; warmth wins");
        assert_eq!(pick(V2, &[warm_heavy.clone(), cold_idle.clone()], &w), ("cold-idle".into(), Tier::Now), "warm + heavy loses to idle");
        // v1 (the 0.46.5 order): any warm slot beats any load difference.
        assert_eq!(pick(V1, &[warm_heavy, cold_idle], &w).0, "warm-heavy");
    }

    /// §10 unit 2 (and §4.4): two equal idle nodes; once one took a job
    /// (a recent placement, or this workspace runs one there), the next
    /// goes to the other.
    #[test]
    fn a_burst_spreads_over_equal_nodes() {
        let w = want(2);
        let (mut a, b) = (cand("a", 32, 32, 0, 4), cand("b", 32, 32, 0, 4));
        assert_eq!(pick(V2, &[a.clone(), b.clone()], &w).0, "a", "the name breaks a full tie");
        a.recent = 1;
        a.pending = Pending { mem_bytes: 2 * GB, cpu_millis: 0, jobs: 1 };
        assert_eq!(pick(V2, &[a.clone(), b.clone()], &w).0, "b", "a just took one: the burst spreads");
        // Same band, no pending: this controller's recent placements and
        // this workspace's own running jobs still spread it.
        let (mut a, b) = (cand("a", 20, 32, 1, 4), cand("b", 20, 32, 1, 4));
        a.recent = 1;
        assert_eq!(pick(V2, &[a.clone(), b.clone()], &w).0, "b", "recent placements spread");
        a.recent = 0;
        a.load.running_by_workspace.insert(ME.into(), 1);
        assert_eq!(pick(V2, &[a, b], &w).0, "b", "one builder's jobs spread");
    }

    /// §10 unit 3 (CP-A6): a shared node whose node-wide free memory is
    /// 2 GB is not Now for a 4 GB job, even with no quote; it is for 1 GB.
    #[test]
    fn a_shared_node_checks_node_wide_free_memory() {
        let c = shared(cand("mini", 2, 8, 1, 2), None);
        assert!(!c.starts_now(&want(4)), "2 GB free node-wide: not Now for --mem 4G");
        assert!(c.starts_now(&want(1)), "1 GB fits");
        assert_eq!(c.tier(&want(4)), Some(Tier::Busy), "it still queues there");
        // A quote that says "starts now" for this size doesn't override
        // node-wide free memory (1 GB free, a 2 GB job).
        let q = shared(cand("mini", 1, 8, 1, 2), Some((true, None)));
        assert!(!q.starts_now(&want(2)));
        assert!(shared(cand("mini", 4, 8, 1, 2), Some((true, None))).starts_now(&want(2)), "with room, the quote stands");
    }

    /// A mini with little free memory is skipped for a big --mem job: the
    /// one with room starts it now.
    #[test]
    fn a_big_mem_job_skips_the_node_without_free_memory() {
        let w = want(24);
        let small = cand("mini", 6, 16, 1, 2);
        let big = cand("ryzen", 100, 110, 3, 8);
        assert!(!small.starts_now(&w) && big.starts_now(&w));
        assert_eq!(pick(V2, &[small, big], &w), ("ryzen".into(), Tier::Now));
    }

    /// §10 unit 4 (CP-A7): pending asks. Two 24 GB jobs in the same
    /// second: the second doesn't see the first one's memory as free; a
    /// newer offer revision, or 60 s, drops the entry.
    #[test]
    fn pending_asks_cover_a_burst_until_the_next_offer() {
        let node = "pending-test-node";
        let now = 1_000_000;
        assert_eq!(pending_for(node, Some(7), now), Pending::default());
        note_pending(node, "job-1", Some(24 * GB), Some(4000), 7, now);
        let p = pending_for(node, Some(7), now);
        assert_eq!(p, Pending { mem_bytes: 24 * GB, cpu_millis: 4000, jobs: 1 });
        let mut c = cand(node, 40, 64, 0, 4);
        assert!(c.starts_now(&want(24)), "before: 40 GB free");
        c.pending = p;
        assert_eq!(c.free_now(), (16 * GB, 4000));
        assert!(!c.starts_now(&want(24)), "the first job's 24 GB is not free any more");
        note_pending(node, "job-1", Some(24 * GB), Some(4000), 7, now);
        assert_eq!(pending_for(node, Some(7), now).jobs, 1, "the same job counts once");
        assert_eq!(pending_for(node, Some(8), now), Pending::default(), "a newer offer shows it: dropped");
        note_pending(node, "job-2", Some(GB), None, 9, now);
        assert_eq!(pending_for(node, None, now + PENDING_SECS - 1).jobs, 1);
        assert_eq!(pending_for(node, None, now + PENDING_SECS).jobs, 0, "dropped after 60 s");
        // Recent placements: 30 s.
        note_placed(node, now);
        note_placed(node, now + 10);
        assert_eq!(recent_for(node, now + 20), 2);
        assert_eq!(recent_for(node, now + RECENT_SECS + 5), 1);
    }

    /// §10 unit 5 (F10, CP-A4): --timeout 4h skips a 2 h grant and keeps a
    /// 6 h one; no --timeout fits every grant (it gets the grant's max).
    #[test]
    fn a_timeout_over_the_grant_skips_that_node() {
        let mut w = want(2);
        let (two, six) = (grant_row("a", 2 * 3600, 1), grant_row("b", 6 * 3600, 1));
        assert!(timeout_fits(&w, Some(&two)) && timeout_fits(&w, Some(&six)));
        w.timeout_secs = Some(4 * 3600);
        assert!(!timeout_fits(&w, Some(&two)), "4h > 2h grant");
        assert!(timeout_fits(&w, Some(&six)), "4h ≤ 6h grant");
        assert!(!timeout_fits(&w, None), "no grant: the 2h default");
    }

    /// §10 unit 6 (F11, CP-A5): a share's per-job memory of 8 GB rules the
    /// node out for --mem 12G (not for a defaulted --mem, which submit
    /// clamps); its 2 h per job rules out --timeout 3h; a solo node has no
    /// share limits.
    #[test]
    fn a_shares_limits_skip_that_node() {
        let c = shared(cand("mini", 30, 32, 0, 2), None);
        let g = grant_row("mini", 2 * 3600, 1);
        assert!(share_fits(&want(8), Some(&g), &c.offer));
        assert!(!share_fits(&want(12), Some(&g), &c.offer), "over the share's 8 GB per job");
        let mut d = want(12);
        d.mem_defaulted = true;
        assert!(share_fits(&d, Some(&g), &c.offer), "a defaulted --mem is clamped at submit");
        let mut t = want(2);
        t.timeout_secs = Some(3 * 3600);
        assert!(!share_fits(&t, Some(&g), &c.offer), "over the share's 2h per job");
        let mut x = want(2);
        x.exclusive = true;
        assert!(!share_fits(&x, Some(&g), &c.offer), "the share doesn't allow exclusive");
        assert!(share_fits(&want(12), Some(&g), &cand("solo", 30, 32, 0, 2).offer), "solo: no share limits");
    }

    /// §10 unit 8: an exclusive job is Now only on an idle node (solo or
    /// shared).
    #[test]
    fn an_exclusive_job_starts_only_on_an_idle_node() {
        let mut w = want(2);
        w.exclusive = true;
        assert!(cand("idle", 32, 32, 0, 4).starts_now(&w));
        assert!(!cand("busy", 30, 32, 1, 4).starts_now(&w));
        assert!(!shared(cand("sbusy", 30, 32, 1, 4), None).starts_now(&w));
        assert_eq!(pick(V2, &[cand("busy", 30, 32, 1, 4), cand("idle", 32, 32, 0, 4)], &w).0, "idle");
    }

    /// §10 unit 9: busy everywhere, a quoted ETA beats an unknown one and a
    /// lower ETA a higher one.
    #[test]
    fn busy_everywhere_the_shortest_quoted_wait_wins() {
        let w = want(2);
        let solo_full = cand("a-solo-full", 30, 32, 2, 2);
        let slow = shared(cand("b-slow", 30, 32, 2, 2), Some((false, Some(900))));
        let fast = shared(cand("c-fast", 30, 32, 2, 2), Some((false, Some(120))));
        let all = [solo_full, slow, fast];
        for c in &all {
            assert_eq!(c.tier(&w), Some(Tier::Busy), "{}", c.node.id);
        }
        assert_eq!(order(V2, &all, &w), vec!["c-fast", "b-slow", "a-solo-full"]);
        assert_eq!(pick(V1, &all, &w).0, "c-fast", "v1 also takes the lowest quote");
    }

    /// §10 unit 10: the v1 switch keeps the 0.46.5 order exactly (a golden
    /// over a mixed set), and v2 differs only where §4 says.
    #[test]
    fn the_v1_switch_keeps_the_old_order() {
        let w = want(2);
        let mut offline = cand("f-offline", 32, 32, 0, 4);
        offline.online = false;
        let set = [
            warm(cand("a-warm-heavy", 4, 32, 1, 2), "c1"),
            cand("b-cold-idle", 32, 32, 0, 4),
            cand("c-cold-light", 24, 32, 1, 4),
            shared(cand("d-busy-quoted", 30, 32, 2, 2), Some((false, Some(300)))),
            cand("e-busy-solo", 30, 32, 2, 2),
            offline,
        ];
        assert_eq!(
            order(V1, &set, &w),
            vec!["a-warm-heavy", "b-cold-idle", "c-cold-light", "d-busy-quoted", "e-busy-solo", "f-offline"],
            "v1 golden: warm first, then less busy, quoted waits, offline last"
        );
        assert_eq!(
            order(V2, &set, &w),
            vec!["b-cold-idle", "a-warm-heavy", "c-cold-light", "d-busy-quoted", "e-busy-solo", "f-offline"],
            "v2: the idle node before the warm heavy one; warm wins the band tie with a light one"
        );
        for p in [V1, V2] {
            assert_eq!(compute::PlacementPolicy::from_wire(p.as_wire()), Some(p));
        }
        assert_eq!(compute::PlacementPolicy::from_wire("v3"), None);
    }

    /// `sys:` packages: a node that installs them ranks first, all else
    /// equal; one that can't stays a candidate.
    #[test]
    fn sys_needs_prefer_a_node_that_installs_packages() {
        let mut w = want(2);
        w.needs = vec!["sys:pkg-config".into()];
        let mac = cand("a-mac", 32, 32, 0, 4);
        let mut linux = cand("b-linux", 32, 32, 0, 4);
        linux.offer.features.push(needs::FEATURE_SYSTEM_PACKAGES.to_string());
        assert_eq!(pick(V2, &[mac.clone(), linux], &w).0, "b-linux");
        assert_eq!(pick(V2, &[mac], &w), ("a-mac".into(), Tier::Now), "still a candidate alone");
    }

    /// §7: the record and its words. Codes per case; the line names only
    /// node names, node-wide numbers and counts — never another
    /// workspace's id (S2/S5).
    #[test]
    fn the_placement_record_says_where_and_why() {
        let w = want(24);
        let cands = [warm(cand("k2n10-vin", 46, 110, 1, 8), "c1"), cand("k2n2-m1", 10, 16, 1, 2)];
        let skipped = BTreeMap::from([(skip::OFFLINE, 1u32), (skip::TOO_SMALL, 1u32)]);
        let ex = Explain { policy: V2, cands: &cands, skipped: &skipped, project_key: Some("pk"), commit: Some("c1") };
        let (c, t) = choose_by(V2, &cands, &w, &[], Some("pk"), Some("c1")).unwrap();
        let rec = placement_record(c, t, &w, &ex, 42);
        assert_eq!((rec.mode.as_str(), rec.policy.as_str(), rec.node.as_str(), rec.code.as_str()), ("auto", "v2", "k2n10-vin", "starts_now_warm"));
        assert_eq!(rec.why, "starts now · warm slot (this commit) · 46/110 GB, 7/8 CPUs free");
        assert_eq!(rec.fits, "2 nodes fit, 2 skipped (1 offline, 1 too small for --mem 24G)");
        assert_eq!((rec.starts_now, rec.considered, rec.at), (Some(true), 2, 42));
        let text = serde_json::to_string(&rec).unwrap();
        assert!(!text.contains("ws-other-secret") && !text.contains(ME), "no workspace ids in the record: {text}");
        // Idle, least loaded, quoted wait, offline only.
        let idle = [cand("idle", 32, 32, 0, 4)];
        let ex = Explain { policy: V1, cands: &idle, skipped: &BTreeMap::new(), project_key: None, commit: None };
        let rec = placement_record(&idle[0], Tier::Now, &want(2), &ex, 1);
        assert_eq!((rec.code.as_str(), rec.policy.as_str(), rec.fits.as_str()), ("starts_now_idle", "v1", "1 node fits"));
        let q = [shared(cand("q", 30, 32, 2, 2), Some((false, Some(330))))];
        let ex = Explain { policy: V2, cands: &q, skipped: &BTreeMap::new(), project_key: None, commit: None };
        let rec = placement_record(&q[0], Tier::Busy, &want(2), &ex, 1);
        assert_eq!((rec.code.as_str(), rec.eta_secs, rec.starts_now), ("queued_quoted_eta", Some(330), Some(false)));
        assert_eq!(rec.why, "no node free now; shortest wait ~6 min (quoted) · it moves if another frees up first");
        let rec = placement_record(&q[0], Tier::Offline, &want(2), &ex, 1);
        assert_eq!(rec.code, "queued_offline_only");
        // A pinned job's record.
        let p = pinned_record("k2n2-m1", 5);
        assert_eq!((p.mode.as_str(), p.code.as_str(), p.starts_now), ("pinned", "pinned", None));
        assert_eq!(mem_words(3 * GB / 2), "1.5G");
    }

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
        assert_eq!(fit_plan(&plan, &job, &old_node, None), None, "no tier, an old node: as it is");
        let tiered = fit_plan(&plan, &job, &new_node, None).expect("a node with tiers gets the tier");
        assert_eq!(tiered.tier, Some(JobTier::Priority));
        assert_ne!(tiered.digest().unwrap(), plan.digest().unwrap());
        assert_eq!(fit_plan(&tiered, &job, &new_node, None), None, "already fits");
        let back = fit_plan(&tiered, &job, &old_node, None).expect("a rolled-back node loses the tier");
        assert_eq!((back.tier, back.digest().unwrap()), (None, plan.digest().unwrap()), "the untiered digest, byte for byte");
        assert!(offers_tiers(&new_node) && !offers_tiers(&old_node));
        // A v0.46.2 node (no `queue`, no `stall`): every field it doesn't
        // know goes, so its digest is the one it computes (the tiers
        // rollback drill: `plan_mismatch` before this).
        let mut full = tiered.clone();
        full.ws_max_running = Some(2);
        full.stall_secs = Some(600);
        let bare = fit_plan(&full, &job, &[], Some(GrantCaps { ws: 3, submitter: 0 })).expect("fitted");
        assert_eq!((bare.tier, bare.class, bare.ws_max_running, bare.stall_secs), (None, None, None, None));
        let mut want = plan.clone();
        want.class = None;
        assert_eq!(bare.digest().unwrap(), want.digest().unwrap(), "what a v0.46.2 node computes");
        assert_eq!(fit_plan(&bare, &job, &[], Some(GrantCaps { ws: 3, submitter: 0 })), None);
    }

    /// prd-compute-job-tiers-v2 L5 (T23, 10-27): the workspace cap in a
    /// plan follows the grant at every send. A plan stored with cap 1 goes
    /// out with the grant's current 3 to a node with a queue; a node
    /// without one never gets the field; the same cap again is no change.
    #[test]
    fn the_workspace_cap_is_refreshed_from_the_grant_before_every_send() {
        let mut plan: Plan = serde_json::from_value(serde_json::json!({
            "job_id": "j1", "node_id": "n1", "workspace_id": "w1", "workspace_label": "k2", "requested_by": "owner",
            "argv": ["true"], "env": {}, "cwd": null,
            "limits": {"max_secs": 60, "cpu_millis": null, "mem_bytes": null, "disk_bytes": 1, "log_cap_bytes": 1},
            "src": null, "exclusive": false, "created_at": 1, "class": "interactive"
        }))
        .expect("a plan");
        plan.ws_max_running = Some(1);
        let job = JobRow { id: "j1".into(), tier: "normal".into(), ..Default::default() };
        let queue_node = vec![FEATURE_TIERS.to_string(), pshare::FEATURE_QUEUE.to_string()];
        let fresh = fit_plan(&plan, &job, &queue_node, Some(GrantCaps { ws: 3, submitter: 0 })).expect("the stale cap is rewritten");
        assert_eq!(fresh.ws_max_running, Some(3));
        assert_ne!(fresh.digest().unwrap(), plan.digest().unwrap(), "a new digest: the row is rewritten before the send");
        assert_eq!(fit_plan(&fresh, &job, &queue_node, Some(GrantCaps { ws: 3, submitter: 0 })), None, "already current");
        let lowered = fit_plan(&fresh, &job, &queue_node, Some(GrantCaps { ws: 1, submitter: 0 })).expect("a lowered grant reaches it too");
        assert_eq!(lowered.ws_max_running, Some(1));
        let solo = fit_plan(&plan, &job, &[FEATURE_TIERS.to_string()], Some(GrantCaps { ws: 3, submitter: 0 })).expect("fitted");
        assert_eq!(solo.ws_max_running, None, "a node without a queue never gets the field");
    }

    /// prd-compute-job-tiers-v2 T27/T30: Batch and the submitter fields go
    /// only to a `tiers2` node; a `tiers`-only (0.46.5) node gets Normal
    /// and no submitter, so its parser and digest are unchanged; a 0.46.4
    /// node orders it by class `batch`.
    #[test]
    fn batch_and_the_submitter_ride_only_to_tiers2_nodes() {
        let plan: Plan = serde_json::from_value(serde_json::json!({
            "job_id": "j1", "node_id": "n1", "workspace_id": "w1", "workspace_label": "k2", "requested_by": "owner",
            "argv": ["true"], "env": {}, "cwd": null,
            "limits": {"max_secs": 60, "cpu_millis": null, "mem_bytes": null, "disk_bytes": 1, "log_cap_bytes": 1},
            "src": null, "exclusive": false, "created_at": 1, "class": "batch"
        }))
        .expect("a plan");
        let job = JobRow { id: "j1".into(), tier: "batch".into(), class: "batch".into(), submitter: Some("k2/abcd1234/builder-a".into()), ..Default::default() };
        let caps = Some(GrantCaps { ws: 3, submitter: 2 });
        let v2 = vec![FEATURE_TIERS.to_string(), pshare::FEATURE_QUEUE.to_string(), k2_core::compute::proto::tiers::FEATURE_TIERS2.to_string()];
        let p = fit_plan(&plan, &job, &v2, caps).expect("fitted");
        assert_eq!((p.tier, p.submitter.as_deref(), p.submitter_max_running), (Some(JobTier::Batch), Some("k2/abcd1234/builder-a"), Some(2)));
        let v1 = vec![FEATURE_TIERS.to_string(), pshare::FEATURE_QUEUE.to_string()];
        let p = fit_plan(&plan, &job, &v1, caps).expect("fitted");
        assert_eq!((p.tier, p.submitter.as_deref(), p.submitter_max_running), (Some(JobTier::Normal), None, None));
        let wire = serde_json::to_value(&p).unwrap();
        assert!(wire.get("submitter").is_none() && wire.get("submitter_max_running").is_none(), "{wire}");
        assert_eq!(wire["tier"], "normal", "a 0.46.5 parser never sees `batch`");
        let p = fit_plan(&plan, &job, &[pshare::FEATURE_QUEUE.to_string()], caps).unwrap_or(plan.clone());
        assert_eq!((p.tier, p.class), (None, Some(pshare::JobClass::Batch)), "0.46.4: class batch");
        // No cap on the grant: no field.
        let p = fit_plan(&plan, &job, &v2, Some(GrantCaps { ws: 3, submitter: 0 })).expect("fitted");
        assert_eq!(p.submitter_max_running, None);
    }
}
