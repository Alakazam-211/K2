//! The node queue (prd-compute-shared-queue-v1 §7): one fair queue per
//! node over every server (tenant) it serves. Pure and deterministic: the
//! node (`k2-node/src/fairq.rs`) feeds it a snapshot and acts on the
//! answer. Solo nodes never use it (the server's `scheduler::decide`
//! places their jobs, as today).
//!
//! **Order** (the first rule that separates two candidates wins):
//! 1. `gate` first (only where the share allows it);
//! 2. tenants within today's soft CPU-hours before those over it;
//! 3. `interactive` before `batch` (a batch job queued 60 min counts as
//!    interactive);
//! 4. fair share: lowest `decayed usage / weight`;
//! 5. tie: rotate after the tenant served last.
//!
//! Inside a tenant: the best class first, then rotate across workspaces,
//! then sessions, then FIFO by the time *the node* received the job. While
//! building the order each pick is charged its estimated CPU, so the order
//! interleaves tenants by weight instead of draining the cheapest one.
//!
//! **Hard limits wait, never fail**: the tenant's `max_running`, the
//! workspace's `max_running`, the node's parallel/CPU/memory caps, one
//! `gate` job at a time, an exclusive job running (or waiting for an idle
//! node), a node-wide block (paused, foreign lock, low disk, an install),
//! a tenant-wide block (share paused, disk full, wall missing).
//!
//! **Backfill**: when the first job held up only by capacity can't fit,
//! later jobs that fit may start, until it has waited 20 min; then nothing
//! else starts until it fits.
//!
//! **ETAs** simulate the queue: running jobs end at their p50 (or p90)
//! remaining time when known, then queued jobs start in order. Unknown
//! durations give no ETA. ETAs are durations, never clock times.
//!
//! No floats on the wire; usage decay uses `f64` internally only.

use std::collections::{BTreeMap, BTreeSet};

use crate::frames::JobState;
use crate::share::{self, blocked, codes, JobClass, Quote, QueueEntry, QueueView, ShareLimits, Slot, TenantTotals};

/// Knobs (§7 "Defaults").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Usage half-life.
    pub half_life_secs: i64,
    /// A batch job queued this long counts as interactive.
    pub batch_aging_secs: i64,
    /// The backfill hold: once the capacity-blocked head waited this long,
    /// nothing else starts until it fits.
    pub backfill_hold_secs: i64,
    /// What an unknown job is charged while ordering (CPU × this).
    pub default_est_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self { half_life_secs: 24 * 3600, batch_aging_secs: 3600, backfill_hold_secs: 20 * 60, default_est_secs: 600 }
    }
}

/// The node's capacity for queued work right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caps {
    pub cpu_millis: u64,
    pub mem_bytes: u64,
    pub parallel_max: u32,
    /// A node-wide reason nothing starts (`paused`, `foreign_lock`,
    /// `disk_low`, `installing_tools`, a refusal code).
    pub blocked: Option<String>,
}

impl Caps {
    /// What a job that names no CPU (memory) is counted as: an even share,
    /// like the node's `used()`.
    pub fn default_cpu(&self) -> u64 {
        self.cpu_millis / self.parallel_max.max(1) as u64
    }
    pub fn default_mem(&self) -> u64 {
        self.mem_bytes / self.parallel_max.max(1) as u64
    }
}

/// One server's standing on this node.
#[derive(Debug, Clone, PartialEq)]
pub struct Tenant {
    pub slot: Slot,
    pub limits: ShareLimits,
    /// Decayed CPU usage on this node, in millicore-seconds.
    pub usage: u64,
    /// CPU-seconds used today (the soft quota).
    pub cpu_secs_today: u64,
    /// A tenant-wide reason its jobs wait (`share_paused`,
    /// `tenant_disk_full`, `tenant_wall_missing`, …).
    pub blocked: Option<String>,
}

impl Tenant {
    fn over_quota(&self) -> bool {
        self.cpu_secs_today >= u64::from(self.limits.cpu_hours_day) * 3600
    }
}

/// A running job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub slot: Slot,
    pub job_id: String,
    pub generation: u32,
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub class: JobClass,
    pub cpu_millis: u64,
    pub mem_bytes: u64,
    pub exclusive: bool,
    /// Expected seconds left (p50 / p90); `None` = unknown.
    pub left_p50: Option<u64>,
    pub left_p90: Option<u64>,
}

/// A job waiting in the node queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queued {
    pub slot: Slot,
    pub job_id: String,
    pub generation: u32,
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub class: JobClass,
    /// `None` = an even share of the node ([`Caps::default_cpu`]).
    pub cpu_millis: Option<u64>,
    pub mem_bytes: Option<u64>,
    pub exclusive: bool,
    /// Node clock when the node received it.
    pub enqueued_at: i64,
    /// Arrival order within its server (FIFO among jobs received in the
    /// same second; the ledger's insert order).
    pub arrival: u64,
    /// Expected run time (p50 / p90); `None` = unknown.
    pub est_p50: Option<u64>,
    pub est_p90: Option<u64>,
    /// The server's per-workspace `max_parallel` (`Plan.ws_max_running`).
    pub ws_max_running: Option<u32>,
    /// May start: its server's session is up and listed it (V47).
    pub eligible: bool,
}

/// Who was served last (the tie and the inside-tenant rotations).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rotation {
    pub last_slot: Option<Slot>,
    pub last_ws: BTreeMap<Slot, String>,
    pub last_session: BTreeMap<(Slot, String), String>,
}

impl Rotation {
    fn served(&mut self, q: &Queued) {
        self.last_slot = Some(q.slot);
        self.last_ws.insert(q.slot, q.workspace_id.clone());
        if let Some(s) = &q.session_id {
            self.last_session.insert((q.slot, q.workspace_id.clone()), s.clone());
        }
    }
}

/// One snapshot of the node.
#[derive(Debug, Clone)]
pub struct Input<'a> {
    pub now: i64,
    pub cfg: &'a Config,
    pub caps: &'a Caps,
    pub tenants: &'a [Tenant],
    pub running: &'a [Running],
    pub queued: &'a [Queued],
    pub rotation: &'a Rotation,
}

/// What the queue says about one queued job.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    /// 1-based position in the node's queue (start order).
    pub pos: u32,
    pub starts_now: bool,
    pub eta_secs: Option<u64>,
    pub eta_p90_secs: Option<u64>,
    pub blocked_by: Option<String>,
}

/// The queue's answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decision {
    /// Indices into `queued`, in start order (eligible first, then the
    /// ones whose server is away).
    pub order: Vec<usize>,
    /// Indices to start now, in order.
    pub start: Vec<usize>,
    /// Per `queued` index.
    pub info: Vec<Info>,
    /// The rotation after the starts.
    pub rotation: Rotation,
}

// ── order ────────────────────────────────────────────────────────────

fn tenant_of(tenants: &[Tenant], slot: Slot) -> Option<&Tenant> {
    tenants.iter().find(|t| t.slot == slot)
}

/// The class the order uses: an unallowed `gate` counts as interactive
/// (the node refuses those at assign anyway); old batch counts as
/// interactive.
fn effective_class(q: &Queued, t: &Tenant, now: i64, cfg: &Config) -> JobClass {
    match q.class {
        JobClass::Gate if t.limits.allows_class(JobClass::Gate) => JobClass::Gate,
        JobClass::Gate => JobClass::Interactive,
        JobClass::Batch if now - q.enqueued_at >= cfg.batch_aging_secs => JobClass::Interactive,
        c => c,
    }
}

/// The next value after `last` in `keys`' order (wrapping), or the first.
fn rotate_after<'a>(keys: &BTreeSet<&'a str>, last: Option<&str>) -> Option<&'a str> {
    let last = match last {
        Some(l) => l,
        None => return keys.iter().next().copied(),
    };
    keys.iter().find(|k| **k > last).or_else(|| keys.iter().next()).copied()
}

/// A tenant's next job: its best class, then the workspace after the last
/// served, then the session after the last served there, then FIFO.
fn tenant_head(pending: &[usize], input: &Input<'_>, t: &Tenant, rot: &Rotation) -> usize {
    let q = input.queued;
    let best = pending.iter().map(|i| effective_class(&q[*i], t, input.now, input.cfg)).max().expect("non-empty");
    let mine: Vec<usize> = pending.iter().copied().filter(|i| effective_class(&q[*i], t, input.now, input.cfg) == best).collect();
    let wss: BTreeSet<&str> = mine.iter().map(|i| q[*i].workspace_id.as_str()).collect();
    let ws = rotate_after(&wss, rot.last_ws.get(&t.slot).map(String::as_str)).expect("a workspace");
    let in_ws: Vec<usize> = mine.iter().copied().filter(|i| q[*i].workspace_id == ws).collect();
    let sessions: BTreeSet<&str> = in_ws.iter().map(|i| q[*i].session_id.as_deref().unwrap_or("")).collect();
    let last_s = rot.last_session.get(&(t.slot, ws.to_string())).map(String::as_str);
    let s = rotate_after(&sessions, last_s).expect("a session");
    in_ws
        .into_iter()
        .filter(|i| q[*i].session_id.as_deref().unwrap_or("") == s)
        .min_by(|a, b| (q[*a].enqueued_at, q[*a].arrival, &q[*a].job_id).cmp(&(q[*b].enqueued_at, q[*b].arrival, &q[*b].job_id)))
        .expect("a job")
}

/// The start order of the eligible jobs (then the rest, FIFO).
pub fn order(input: &Input<'_>) -> Vec<usize> {
    let q = input.queued;
    let mut usage: BTreeMap<Slot, u128> = input.tenants.iter().map(|t| (t.slot, u128::from(t.usage))).collect();
    let mut today: BTreeMap<Slot, u64> = input.tenants.iter().map(|t| (t.slot, t.cpu_secs_today)).collect();
    let mut rot = input.rotation.clone();
    let mut pending: BTreeMap<Slot, Vec<usize>> = BTreeMap::new();
    let mut away: Vec<usize> = Vec::new();
    for (i, e) in q.iter().enumerate() {
        if e.eligible && tenant_of(input.tenants, e.slot).is_some() {
            pending.entry(e.slot).or_default().push(i);
        } else {
            away.push(i);
        }
    }
    let mut out = Vec::with_capacity(q.len());
    while !pending.is_empty() {
        // Each tenant's head, with its sort key.
        let mut best: Option<(usize, Slot)> = None;
        for (slot, list) in &pending {
            let t = tenant_of(input.tenants, *slot).expect("pending only for known tenants");
            let head = tenant_head(list, input, t, &rot);
            best = Some(match best {
                None => (head, *slot),
                Some((b, bs)) => {
                    if before(input, (head, *slot), (b, bs), &usage, &today, &rot) {
                        (head, *slot)
                    } else {
                        (b, bs)
                    }
                }
            });
        }
        let (pick, slot) = best.expect("a pick");
        out.push(pick);
        let e = &q[pick];
        let cpu = e.cpu_millis.unwrap_or_else(|| input.caps.default_cpu());
        let est = e.est_p50.unwrap_or(input.cfg.default_est_secs);
        *usage.entry(slot).or_default() += u128::from(cpu) * u128::from(est);
        *today.entry(slot).or_default() += cpu.saturating_mul(est) / 1000;
        rot.served(e);
        let list = pending.get_mut(&slot).expect("listed");
        list.retain(|i| *i != pick);
        if list.is_empty() {
            pending.remove(&slot);
        }
    }
    away.sort_by(|a, b| (q[*a].enqueued_at, q[*a].arrival, &q[*a].job_id).cmp(&(q[*b].enqueued_at, q[*b].arrival, &q[*b].job_id)));
    out.extend(away);
    out
}

/// Does candidate `a` go before `b`? (§7 rules 1–5.)
fn before(input: &Input<'_>, a: (usize, Slot), b: (usize, Slot), usage: &BTreeMap<Slot, u128>, today: &BTreeMap<Slot, u64>, rot: &Rotation) -> bool {
    let (ta, tb) = (tenant_of(input.tenants, a.1).expect("known"), tenant_of(input.tenants, b.1).expect("known"));
    let (qa, qb) = (&input.queued[a.0], &input.queued[b.0]);
    let (ca, cb) = (effective_class(qa, ta, input.now, input.cfg), effective_class(qb, tb, input.now, input.cfg));
    // 1. gate first.
    if (ca == JobClass::Gate) != (cb == JobClass::Gate) {
        return ca == JobClass::Gate;
    }
    // 2. within quota first.
    let over = |t: &Tenant| today.get(&t.slot).copied().unwrap_or(0) >= u64::from(t.limits.cpu_hours_day) * 3600;
    if over(ta) != over(tb) {
        return !over(ta);
    }
    // 3. interactive before batch.
    if ca != cb {
        return ca > cb;
    }
    // 4. lowest usage / weight (cross-multiplied, no floats).
    let (ua, ub) = (usage.get(&a.1).copied().unwrap_or(0), usage.get(&b.1).copied().unwrap_or(0));
    let (wa, wb) = (u128::from(ta.limits.weight.max(1)), u128::from(tb.limits.weight.max(1)));
    if ua * wb != ub * wa {
        return ua * wb < ub * wa;
    }
    // 5. rotate after the tenant served last.
    let dist = |s: Slot| match rot.last_slot {
        Some(l) => (s as i32 - l as i32 - 1).rem_euclid(share::MAX_SLOTS as i32),
        None => s as i32,
    };
    dist(a.1) < dist(b.1)
}

// ── starts and the simulation ────────────────────────────────────────

/// A job the simulation runs.
#[derive(Debug, Clone)]
struct SimRun {
    slot: Slot,
    ws: String,
    gate: bool,
    cpu: u64,
    mem: u64,
    exclusive: bool,
    /// Simulated end time; `None` = unknown.
    end: Option<u64>,
}

/// Why a job can't start in this state: `Wait` lets later jobs go
/// (tenant or workspace limits, one gate); `Capacity` is the node being
/// full (the backfill hold applies); `Never` can't fit even on an idle
/// node.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Fit {
    Now,
    Wait,
    Capacity,
    Never,
}

fn fit(input: &Input<'_>, e: &Queued, t: &Tenant, run: &[SimRun]) -> Fit {
    let caps = input.caps;
    let cpu = e.cpu_millis.unwrap_or_else(|| caps.default_cpu());
    let mem = e.mem_bytes.unwrap_or_else(|| caps.default_mem());
    if cpu > caps.cpu_millis || mem > caps.mem_bytes {
        return Fit::Never;
    }
    let mine = run.iter().filter(|r| r.slot == e.slot).count() as u32;
    if mine >= t.limits.max_running {
        return Fit::Wait;
    }
    if let Some(m) = e.ws_max_running {
        if run.iter().filter(|r| r.slot == e.slot && r.ws == e.workspace_id).count() as u32 >= m.max(1) {
            return Fit::Wait;
        }
    }
    let gate = effective_class(e, t, input.now, input.cfg) == JobClass::Gate;
    if gate && run.iter().any(|r| r.gate) {
        return Fit::Wait;
    }
    let used_cpu: u64 = run.iter().map(|r| r.cpu).sum();
    let used_mem: u64 = run.iter().map(|r| r.mem).sum();
    let excl_running = run.iter().any(|r| r.exclusive);
    if excl_running || (e.exclusive && !run.is_empty()) || run.len() as u32 >= caps.parallel_max {
        return Fit::Capacity;
    }
    if used_cpu.saturating_add(cpu) > caps.cpu_millis || used_mem.saturating_add(mem) > caps.mem_bytes {
        return Fit::Capacity;
    }
    Fit::Now
}

struct Sim {
    /// Per queued index: start time (secs from now) if simulated.
    start_at: Vec<Option<u64>>,
    blocked: Vec<Option<String>>,
}

/// Run the queue forward. `p90` picks the slow estimates.
fn simulate(input: &Input<'_>, ord: &[usize], p90: bool) -> Sim {
    let q = input.queued;
    let mut start_at = vec![None; q.len()];
    let mut blocked: Vec<Option<String>> = vec![None; q.len()];
    let mut run: Vec<SimRun> = input
        .running
        .iter()
        .map(|r| SimRun {
            slot: r.slot,
            ws: r.workspace_id.clone(),
            gate: r.class == JobClass::Gate,
            cpu: r.cpu_millis,
            mem: r.mem_bytes,
            exclusive: r.exclusive,
            end: if p90 { r.left_p90.or(r.left_p50) } else { r.left_p50 },
        })
        .collect();
    // Who can never start here (the whole run).
    let mut pending: Vec<usize> = Vec::new();
    for i in ord {
        let e = &q[*i];
        let Some(t) = tenant_of(input.tenants, e.slot) else {
            blocked[*i] = Some(codes::SERVER_NOT_APPROVED.into());
            continue;
        };
        if !e.eligible {
            blocked[*i] = Some(blocked::SERVER_OFFLINE.into());
        } else if let Some(b) = &input.caps.blocked {
            blocked[*i] = Some(b.clone());
        } else if let Some(b) = &t.blocked {
            blocked[*i] = Some(b.clone());
        } else {
            pending.push(*i);
        }
    }
    let mut now: u64 = 0;
    for _round in 0..(4 * q.len() + 8) {
        // Start pass at `now`, in order, with the backfill hold.
        let mut held = false;
        let mut capacity_head_seen = false;
        let mut started = Vec::new();
        for i in &pending {
            let e = &q[*i];
            let t = tenant_of(input.tenants, e.slot).expect("checked");
            match fit(input, e, t, &run) {
                Fit::Now if !held => {
                    let cpu = e.cpu_millis.unwrap_or_else(|| input.caps.default_cpu());
                    let mem = e.mem_bytes.unwrap_or_else(|| input.caps.default_mem());
                    let est = if p90 { e.est_p90.or(e.est_p50) } else { e.est_p50 };
                    run.push(SimRun {
                        slot: e.slot,
                        ws: e.workspace_id.clone(),
                        gate: effective_class(e, t, input.now, input.cfg) == JobClass::Gate,
                        cpu,
                        mem,
                        exclusive: e.exclusive,
                        end: est.map(|d| now + d.max(1)),
                    });
                    start_at[*i] = Some(now);
                    started.push(*i);
                }
                Fit::Capacity if !capacity_head_seen => {
                    capacity_head_seen = true;
                    let waited = input.now - e.enqueued_at + now as i64;
                    if waited >= input.cfg.backfill_hold_secs {
                        held = true;
                    }
                }
                Fit::Never => blocked[*i] = Some(blocked::EXCEEDS_NODE_CAPS.into()),
                _ => {}
            }
        }
        pending.retain(|i| !started.contains(i) && blocked[*i].is_none());
        if pending.is_empty() {
            break;
        }
        // Advance to the next known end and retire everything over by
        // then (each round retires at least one job, so this ends).
        let Some(next) = run.iter().filter_map(|r| r.end).min() else {
            break; // nothing ends at a known time: the rest have no ETA
        };
        now = next.max(now);
        run.retain(|r| r.end.is_none_or(|e| e > now));
    }
    Sim { start_at, blocked }
}

/// The whole answer for a snapshot: order, starts, positions and ETAs.
pub fn decide(input: &Input<'_>) -> Decision {
    let ord = order(input);
    let fast = simulate(input, &ord, false);
    let slow = simulate(input, &ord, true);
    let mut info = vec![Info::default(); input.queued.len()];
    let mut start = Vec::new();
    let mut rotation = input.rotation.clone();
    for (pos, i) in ord.iter().enumerate() {
        let e = &input.queued[*i];
        let now = fast.start_at[*i] == Some(0);
        if now {
            start.push(*i);
            rotation.served(e);
        }
        let mut blocked_by = fast.blocked[*i].clone();
        if blocked_by.is_none() && !now {
            if let Some(t) = tenant_of(input.tenants, e.slot) {
                if t.over_quota() {
                    blocked_by = Some(blocked::OVER_QUOTA.into());
                }
            }
        }
        info[*i] = Info {
            pos: pos as u32 + 1,
            starts_now: now,
            eta_secs: fast.start_at[*i],
            eta_p90_secs: slow.start_at[*i].or(fast.start_at[*i]),
            blocked_by,
        };
    }
    Decision { order: ord, start, info, rotation }
}

/// A quote (V53): would a job of this size from `slot` start now, and if
/// not, roughly when? Rounded for display to other servers' precision.
pub fn quote(input: &Input<'_>, slot: Slot, cpu_millis: u64, mem_bytes: u64) -> Quote {
    let mut queued: Vec<Queued> = input.queued.to_vec();
    queued.push(Queued {
        slot,
        job_id: "\u{0}quote".into(),
        generation: 0,
        workspace_id: "\u{0}quote".into(),
        session_id: None,
        class: JobClass::Interactive,
        // 0 = the plan named no limit: an even share, like an unsized job.
        cpu_millis: (cpu_millis > 0).then_some(cpu_millis),
        mem_bytes: (mem_bytes > 0).then_some(mem_bytes),
        exclusive: false,
        enqueued_at: input.now,
        arrival: u64::MAX,
        est_p50: None,
        est_p90: None,
        ws_max_running: None,
        eligible: true,
    });
    let probe = Input { queued: &queued, ..input.clone() };
    let d = decide(&probe);
    let i = &d.info[queued.len() - 1];
    Quote {
        cpu_millis,
        mem_bytes,
        starts_now: i.starts_now,
        eta_secs: (!i.starts_now).then_some(i.eta_secs).flatten().map(share::round_eta),
        eta_p90_secs: (!i.starts_now).then_some(i.eta_p90_secs).flatten().map(share::round_eta),
        blocked_by: i.blocked_by.clone(),
    }
}

// ── what each viewer sees ────────────────────────────────────────────

/// Who is looking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer<'a> {
    pub slot: Slot,
    /// The owner server: also other tenants' labels and per-tenant totals.
    pub owner: bool,
    pub labels: &'a BTreeMap<Slot, String>,
}

/// The queue as `viewer` may see it (§7): its own jobs in full; others'
/// as "another server" with class, state, size and a rounded ETA only.
pub fn view(input: &Input<'_>, d: &Decision, viewer: &Viewer<'_>, request_id: &str) -> QueueView {
    let mut entries = Vec::new();
    let mut pos = 0u32;
    let mut push = |mine: bool, slot: Slot, mut e: QueueEntry, job: (&str, u32, &str, Option<&String>)| {
        pos += 1;
        e.pos = pos;
        if mine {
            e.job_id = Some(job.0.to_string());
            e.generation = Some(job.1);
            e.workspace_id = Some(job.2.to_string());
            e.session_id = job.3.cloned();
        } else {
            e.eta_secs = e.eta_secs.map(share::round_eta);
            e.eta_p90_secs = e.eta_p90_secs.map(share::round_eta);
            if viewer.owner {
                e.server_label = viewer.labels.get(&slot).cloned();
            }
        }
        entries.push(e);
    };
    for r in input.running {
        let mine = r.slot == viewer.slot;
        let e = QueueEntry {
            pos: 0,
            mine,
            class: r.class,
            state: JobState::Running,
            cpu_millis: r.cpu_millis,
            mem_bytes: r.mem_bytes,
            eta_secs: r.left_p50,
            eta_p90_secs: r.left_p90.or(r.left_p50),
            blocked_by: None,
            job_id: None,
            generation: None,
            workspace_id: None,
            session_id: None,
            server_label: None,
        };
        push(mine, r.slot, e, (&r.job_id, r.generation, &r.workspace_id, r.session_id.as_ref()));
    }
    for i in &d.order {
        let q = &input.queued[*i];
        let info = &d.info[*i];
        let mine = q.slot == viewer.slot;
        let e = QueueEntry {
            pos: 0,
            mine,
            class: q.class,
            state: JobState::Queued,
            cpu_millis: q.cpu_millis.unwrap_or_else(|| input.caps.default_cpu()),
            mem_bytes: q.mem_bytes.unwrap_or_else(|| input.caps.default_mem()),
            eta_secs: info.eta_secs,
            eta_p90_secs: info.eta_p90_secs,
            blocked_by: info.blocked_by.clone(),
            job_id: None,
            generation: None,
            workspace_id: None,
            session_id: None,
            server_label: None,
        };
        push(mine, q.slot, e, (&q.job_id, q.generation, &q.workspace_id, q.session_id.as_ref()));
    }
    let totals = if viewer.owner {
        input
            .tenants
            .iter()
            .map(|t| TenantTotals {
                slot: t.slot,
                label: viewer.labels.get(&t.slot).cloned().unwrap_or_default(),
                running: input.running.iter().filter(|r| r.slot == t.slot).count() as u32,
                queued: input.queued.iter().filter(|q| q.slot == t.slot).count() as u32,
                cpu_secs_today: t.cpu_secs_today,
            })
            .collect()
    } else {
        Vec::new()
    };
    QueueView { request_id: request_id.to_string(), at: input.now, entries, totals }
}

// ── usage ────────────────────────────────────────────────────────────

/// `usage` after `dt` seconds of half-life decay.
pub fn decay(usage: u64, dt_secs: i64, half_life_secs: i64) -> u64 {
    if dt_secs <= 0 || usage == 0 {
        return usage;
    }
    let f = 0.5f64.powf(dt_secs as f64 / half_life_secs.max(1) as f64);
    (usage as f64 * f) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::{Network, Trust};

    fn limits(weight: u32) -> ShareLimits {
        ShareLimits { weight, max_running: 8, max_queued: 1000, cpu_hours_day: 1000, ..ShareLimits::new_server(Trust::Own, false) }
    }

    fn tenant(slot: Slot, weight: u32) -> Tenant {
        Tenant { slot, limits: limits(weight), usage: 0, cpu_secs_today: 0, blocked: None }
    }

    fn caps(par: u32) -> Caps {
        Caps { cpu_millis: 8000, mem_bytes: 32 << 30, parallel_max: par, blocked: None }
    }

    fn q(slot: Slot, job: &str, ws: &str, at: i64) -> Queued {
        Queued {
            slot,
            job_id: job.into(),
            generation: 1,
            workspace_id: ws.into(),
            session_id: None,
            class: JobClass::Interactive,
            cpu_millis: Some(1000),
            mem_bytes: Some(1 << 30),
            exclusive: false,
            enqueued_at: at,
            arrival: 0,
            est_p50: Some(60),
            est_p90: Some(120),
            ws_max_running: None,
            eligible: true,
        }
    }

    fn running(slot: Slot, job: &str, cpu: u64, left: Option<u64>) -> Running {
        Running {
            slot,
            job_id: job.into(),
            generation: 1,
            workspace_id: "w".into(),
            session_id: None,
            class: JobClass::Interactive,
            cpu_millis: cpu,
            mem_bytes: 1 << 30,
            exclusive: false,
            left_p50: left,
            left_p90: left.map(|l| l * 2),
        }
    }

    struct World {
        cfg: Config,
        caps: Caps,
        tenants: Vec<Tenant>,
        running: Vec<Running>,
        queued: Vec<Queued>,
        rot: Rotation,
        now: i64,
    }

    impl World {
        fn new(par: u32, tenants: Vec<Tenant>) -> Self {
            Self { cfg: Config::default(), caps: caps(par), tenants, running: vec![], queued: vec![], rot: Rotation::default(), now: 10_000 }
        }
        fn input(&self) -> Input<'_> {
            Input { now: self.now, cfg: &self.cfg, caps: &self.caps, tenants: &self.tenants, running: &self.running, queued: &self.queued, rotation: &self.rot }
        }
        fn ids(&self, idx: &[usize]) -> Vec<String> {
            idx.iter().map(|i| self.queued[*i].job_id.clone()).collect()
        }
        fn order(&self) -> Vec<String> {
            self.ids(&order(&self.input()))
        }
        fn decide(&self) -> Decision {
            decide(&self.input())
        }
    }

    #[test]
    fn gate_jumps_the_queue_but_only_one_runs_and_it_never_kills() {
        let mut owner = tenant(0, 2);
        owner.limits = ShareLimits::owner(8, 8, 32, 100, Network::On);
        let mut w = World::new(4, vec![owner, tenant(1, 1)]);
        w.running = vec![running(1, "r1", 1000, Some(600))];
        w.queued = vec![q(1, "b1", "w", 1), q(0, "g1", "w", 5), q(0, "g2", "w", 6)];
        w.queued[1].class = JobClass::Gate;
        w.queued[2].class = JobClass::Gate;
        assert_eq!(w.order(), vec!["g1", "g2", "b1"]);
        let d = w.decide();
        assert_eq!(w.ids(&d.start), vec!["g1", "b1"], "one gate at a time; the other waits, b1 backfills");
        assert!(!d.info[2].starts_now && d.info[2].pos == 2);
        // A gate from a tenant whose share doesn't allow it is interactive.
        w.queued[0].class = JobClass::Gate;
        w.queued[1].class = JobClass::Interactive;
        w.queued[2].class = JobClass::Interactive;
        assert_eq!(w.order()[0], "g1", "tenant 1 can't gate, so b1 doesn't jump; slot 0 wins the tie");
    }

    #[test]
    fn within_quota_goes_before_over_quota_even_against_class() {
        let mut a = tenant(0, 1);
        a.cpu_secs_today = 1000 * 3600; // at the soft limit
        let b = tenant(1, 1);
        let mut w = World::new(1, vec![a, b]);
        w.queued = vec![q(0, "a-int", "w", 1), q(1, "b-batch", "w", 2)];
        w.queued[1].class = JobClass::Batch;
        assert_eq!(w.order(), vec!["b-batch", "a-int"], "quota sits above class, so marking all interactive can't win");
        let d = w.decide();
        assert_eq!(w.ids(&d.start), vec!["b-batch"]);
        assert_eq!(d.info[0].blocked_by.as_deref(), Some(blocked::OVER_QUOTA), "it waits, it doesn't fail");
        assert!(d.info[0].eta_secs.is_some(), "over quota still gets an ETA");
    }

    #[test]
    fn interactive_before_batch_and_old_batch_counts_as_interactive() {
        let mut w = World::new(1, vec![tenant(0, 1)]);
        w.queued = vec![q(0, "batch", "w", 1), q(0, "int", "w", 5)];
        w.queued[0].class = JobClass::Batch;
        w.now = 100;
        assert_eq!(w.order(), vec!["int", "batch"]);
        w.now = 1 + 3600;
        assert_eq!(w.order(), vec!["batch", "int"], "aged batch is interactive and older");
    }

    #[test]
    fn fair_share_follows_decayed_usage_over_weight() {
        let mut a = tenant(0, 2);
        a.usage = 1_000_000;
        let mut b = tenant(1, 1);
        b.usage = 600_000; // 600k/1 > 1M/2
        let mut w = World::new(1, vec![a, b]);
        w.queued = vec![q(1, "b1", "w", 1), q(0, "a1", "w", 2)];
        assert_eq!(w.order()[0], "a1");
        w.tenants[1].usage = 400_000;
        assert_eq!(w.order()[0], "b1");
    }

    /// Weights 2:1 with equal jobs converge to 2:1 of the starts (the
    /// order charges each pick), within 10 %.
    #[test]
    fn weights_two_to_one_share_two_to_one() {
        let mut w = World::new(1, vec![tenant(0, 2), tenant(1, 1)]);
        for i in 0..300 {
            w.queued.push(q(0, &format!("a{i:03}"), "w", i));
            w.queued.push(q(1, &format!("b{i:03}"), "w", i));
        }
        let first: Vec<String> = w.order().into_iter().take(150).collect();
        let a = first.iter().filter(|j| j.starts_with('a')).count() as f64;
        let b = first.iter().filter(|j| j.starts_with('b')).count() as f64;
        let ratio = a / b;
        assert!((1.8..=2.2).contains(&ratio), "a {a} b {b} ratio {ratio}");
    }

    #[test]
    fn five_workspaces_get_no_more_than_one() {
        // Tenant 0 has five workspaces, tenant 1 one; equal weights.
        let mut w = World::new(1, vec![tenant(0, 1), tenant(1, 1)]);
        for i in 0..40 {
            w.queued.push(q(0, &format!("a{i:02}"), &format!("ws{}", i % 5), i));
            w.queued.push(q(1, &format!("b{i:02}"), "solo", i));
        }
        let first: Vec<String> = w.order().into_iter().take(40).collect();
        let a = first.iter().filter(|j| j.starts_with('a')).count();
        assert!((18..=22).contains(&a), "five workspaces got {a} of 40");
        // Inside tenant 0 the workspaces rotate.
        let a_ws: Vec<String> = first.iter().filter(|j| j.starts_with('a')).take(5).map(|j| w.queued.iter().find(|x| &x.job_id == j).unwrap().workspace_id.clone()).collect();
        let distinct: BTreeSet<&String> = a_ws.iter().collect();
        assert_eq!(distinct.len(), 5, "{a_ws:?}");
    }

    #[test]
    fn sessions_rotate_inside_a_workspace_then_fifo() {
        let mut w = World::new(1, vec![tenant(0, 1)]);
        let s = |job: &str, sess: &str, at: i64| {
            let mut e = q(0, job, "w", at);
            e.session_id = Some(sess.into());
            e
        };
        w.queued = vec![s("x1", "x", 1), s("x2", "x", 2), s("x3", "x", 3), s("y1", "y", 4)];
        assert_eq!(w.order(), vec!["x1", "y1", "x2", "x3"]);
    }

    #[test]
    fn fifo_uses_the_node_clock_not_the_servers() {
        let mut w = World::new(1, vec![tenant(0, 1)]);
        w.queued = vec![q(0, "late", "w", 50), q(0, "early", "w", 10)];
        assert_eq!(w.order(), vec!["early", "late"]);
        // Same second: arrival order, not the job ids' spelling.
        let (mut j10, mut j9) = (q(0, "j10", "w", 7), q(0, "j9", "w", 7));
        j10.arrival = 1;
        j9.arrival = 2;
        w.queued = vec![j9, j10];
        assert_eq!(w.order(), vec!["j10", "j9"]);
    }

    #[test]
    fn hard_limits_wait_and_never_fail() {
        let mut t = tenant(0, 1);
        t.limits.max_running = 1;
        let mut w = World::new(4, vec![t, tenant(1, 1)]);
        w.running = vec![running(0, "r0", 1000, Some(100))];
        w.queued = vec![q(0, "a", "w", 1), q(1, "b", "w", 2)];
        let d = w.decide();
        assert_eq!(w.ids(&d.start), vec!["b"], "a waits for its tenant's max_running; b goes");
        assert_eq!(d.info[0].eta_secs, Some(100), "a starts when r0 ends");
        // A paused share waits with a reason.
        w.tenants[1].blocked = Some(codes::SHARE_PAUSED.into());
        let d = w.decide();
        assert!(d.start.is_empty());
        assert_eq!(d.info[1].blocked_by.as_deref(), Some(codes::SHARE_PAUSED));
        // A node-wide block holds everything.
        w.tenants[1].blocked = None;
        w.caps.blocked = Some(blocked::INSTALLING_TOOLS.into());
        let d = w.decide();
        assert!(d.start.is_empty());
        assert!(d.info.iter().all(|i| i.blocked_by.as_deref() == Some(blocked::INSTALLING_TOOLS) && i.eta_secs.is_none()));
    }

    #[test]
    fn workspace_max_running_is_honoured_inside_the_tenant() {
        let mut w = World::new(4, vec![tenant(0, 1)]);
        w.running = vec![running(0, "r", 1000, Some(30))];
        w.queued = vec![q(0, "a", "w", 1), q(0, "b", "other", 2)];
        w.queued[0].ws_max_running = Some(1);
        let d = w.decide();
        assert_eq!(w.ids(&d.start), vec!["b"]);
        assert_eq!(d.info[0].eta_secs, Some(30));
    }

    #[test]
    fn backfill_starts_small_jobs_until_the_head_waited_twenty_minutes() {
        let mut w = World::new(4, vec![tenant(0, 1), tenant(1, 1)]);
        w.running = vec![running(1, "big-running", 6000, Some(300))];
        let mut big = q(0, "big", "w", w.now - 60);
        big.cpu_millis = Some(4000);
        let small = q(1, "small", "w", w.now - 30);
        w.queued = vec![big, small];
        let d = w.decide();
        assert_eq!(w.ids(&d.start), vec!["small"], "the small job backfills");
        assert_eq!(d.info[0].eta_secs, Some(300));
        // The head has now waited 20 min: nothing else starts.
        w.queued[0].enqueued_at = w.now - 20 * 60;
        let d = w.decide();
        assert!(d.start.is_empty(), "held for the head");
        assert_eq!(d.info[1].eta_secs, Some(300), "small starts with the head");
    }

    #[test]
    fn an_exclusive_head_waits_for_an_idle_node_and_holds_after_twenty_minutes() {
        let mut w = World::new(4, vec![tenant(0, 1)]);
        w.running = vec![running(0, "r", 1000, Some(50))];
        let mut ex = q(0, "ex", "w", w.now - 10);
        ex.exclusive = true;
        w.queued = vec![ex, q(0, "after", "w", w.now - 5)];
        assert_eq!(w.ids(&w.decide().start), vec!["after"]);
        w.queued[0].enqueued_at = w.now - 3600;
        let d = w.decide();
        assert!(d.start.is_empty());
        assert_eq!(d.info[0].eta_secs, Some(50));
    }

    #[test]
    fn a_job_that_never_fits_is_marked_and_holds_nobody() {
        let mut w = World::new(4, vec![tenant(0, 1)]);
        let mut huge = q(0, "huge", "w", w.now - 9999);
        huge.cpu_millis = Some(64_000);
        w.queued = vec![huge, q(0, "ok", "w", w.now)];
        let d = w.decide();
        assert_eq!(w.ids(&d.start), vec!["ok"]);
        assert_eq!(d.info[0].blocked_by.as_deref(), Some("exceeds_node_caps"));
        assert_eq!(d.info[0].eta_secs, None);
    }

    #[test]
    fn an_away_servers_jobs_wait_and_block_nobody() {
        let mut w = World::new(1, vec![tenant(0, 1), tenant(1, 1)]);
        w.queued = vec![q(0, "away", "w", 1), q(1, "here", "w", 2)];
        w.queued[0].eligible = false;
        let d = w.decide();
        assert_eq!(w.ids(&d.order), vec!["here", "away"]);
        assert_eq!(w.ids(&d.start), vec!["here"]);
        assert_eq!(d.info[0].blocked_by.as_deref(), Some(blocked::SERVER_OFFLINE));
        assert_eq!(d.info[0].eta_secs, None);
    }

    #[test]
    fn etas_simulate_the_queue_and_unknowns_stay_unknown() {
        let mut w = World::new(1, vec![tenant(0, 1)]);
        w.running = vec![running(0, "r", 1000, Some(100))];
        w.queued = vec![q(0, "a", "w", 1), q(0, "b", "w", 2), q(0, "c", "w", 3)];
        w.queued[1].est_p50 = None;
        w.queued[1].est_p90 = None;
        let d = w.decide();
        assert_eq!(d.info.iter().map(|i| i.eta_secs).collect::<Vec<_>>(), vec![Some(100), Some(160), None]);
        assert_eq!(d.info[0].eta_p90_secs, Some(200), "the running job's p90");
        assert_eq!(d.info.iter().map(|i| i.pos).collect::<Vec<_>>(), vec![1, 2, 3]);
        w.running[0].left_p50 = None;
        w.running[0].left_p90 = None;
        assert!(w.decide().info.iter().all(|i| i.eta_secs.is_none()));
    }

    #[test]
    fn the_rotation_advances_past_the_tenant_served() {
        let mut w = World::new(1, vec![tenant(0, 1), tenant(1, 1), tenant(2, 1)]);
        w.queued = vec![q(0, "a", "w", 1), q(1, "b", "w", 1), q(2, "c", "w", 1)];
        assert_eq!(w.order()[0], "a");
        w.rot.last_slot = Some(0);
        assert_eq!(w.order()[0], "b");
        w.rot.last_slot = Some(2);
        assert_eq!(w.order()[0], "a");
        let d = w.decide();
        assert_eq!(d.rotation.last_slot, Some(0), "the start is remembered");
    }

    #[test]
    fn quotes_say_start_now_or_a_rounded_eta() {
        let mut w = World::new(2, vec![tenant(0, 1), tenant(1, 1)]);
        let q0 = quote(&w.input(), 1, 1000, 1 << 30);
        assert!(q0.starts_now && q0.eta_secs.is_none());
        w.running = vec![running(0, "r0", 1000, Some(130)), running(0, "r1", 1000, Some(700))];
        let q1 = quote(&w.input(), 1, 1000, 1 << 30);
        assert!(!q1.starts_now);
        assert_eq!(q1.eta_secs, Some(180), "130 s rounds up to 3 min");
        let q2 = quote(&w.input(), 1, 64_000, 1 << 30);
        assert_eq!(q2.blocked_by.as_deref(), Some("exceeds_node_caps"));
        // An unsized job (0, 0) is quoted as an even share and keeps its key.
        let q3 = quote(&w.input(), 1, 0, 0);
        assert_eq!((q3.cpu_millis, q3.mem_bytes, q3.starts_now), (0, 0, false));
        assert_eq!(q3.eta_secs, Some(180));
    }

    #[test]
    fn others_see_only_redacted_entries_and_the_owner_sees_labels() {
        let mut w = World::new(1, vec![tenant(0, 1), tenant(1, 1)]);
        let mut r = running(0, "secret-job", 1000, Some(361));
        r.workspace_id = "owner-ws".into();
        w.running = vec![r];
        let mut mine = q(1, "my-job", "my-ws", 1);
        mine.session_id = Some("sess".into());
        w.queued = vec![mine];
        let d = w.decide();
        let labels = BTreeMap::from([(0, "rosson.k2.dev".to_string()), (1, "baden.k2.dev".to_string())]);
        let v = view(&w.input(), &d, &Viewer { slot: 1, owner: false, labels: &labels }, "r1");
        assert_eq!(v.request_id, "r1");
        let other = &v.entries[0];
        assert!(!other.mine && other.job_id.is_none() && other.workspace_id.is_none() && other.session_id.is_none() && other.server_label.is_none());
        assert_eq!((other.state, other.eta_secs), (JobState::Running, Some(420)), "rounded");
        let me = &v.entries[1];
        assert!(me.mine && me.job_id.as_deref() == Some("my-job") && me.session_id.as_deref() == Some("sess"));
        assert!(v.totals.is_empty());
        let text = crate::canonical::to_string(&v).unwrap();
        assert!(!text.contains("secret-job") && !text.contains("owner-ws") && !text.contains("rosson"), "{text}");
        let o = view(&w.input(), &d, &Viewer { slot: 0, owner: true, labels: &labels }, "r2");
        assert_eq!(o.entries[1].server_label.as_deref(), Some("baden.k2.dev"));
        assert!(o.entries[1].job_id.is_none(), "the owner sees labels, not other servers' jobs");
        assert_eq!(o.totals.len(), 2);
    }

    #[test]
    fn usage_halves_every_half_life() {
        assert_eq!(decay(1000, 0, 86400), 1000);
        assert_eq!(decay(1000, 86400, 86400), 500);
        assert_eq!(decay(1000, 2 * 86400, 86400), 250);
        assert_eq!(decay(0, 99, 86400), 0);
    }

    #[test]
    fn a_tenant_without_a_share_is_never_started() {
        let mut w = World::new(1, vec![tenant(0, 1)]);
        w.queued = vec![q(5, "ghost", "w", 1)];
        let d = w.decide();
        assert!(d.start.is_empty());
        assert_eq!(d.info[0].blocked_by.as_deref(), Some(codes::SERVER_NOT_APPROVED));
    }
}
