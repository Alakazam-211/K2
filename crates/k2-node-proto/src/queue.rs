//! The node queue (prd-compute-shared-queue-v1 §7): one fair queue per
//! node over every server (tenant) it serves. Pure and deterministic: the
//! node (`k2-node/src/fairq.rs`) feeds it a snapshot and acts on the
//! answer. Solo nodes never use it (the server's `scheduler::decide`
//! places their jobs, as today).
//!
//! **Order** (the first rule that separates two candidates wins;
//! prd-compute-job-tiers-v1 §3.4 with vs-live T5, T22):
//! 1. the tier band, highest first: CI (only where the share allows
//!    `gate`), Priority (only where Priority counts for that server),
//!    Normal waiting an hour or more, then fresh Normal
//!    ([`crate::tiers::band`]);
//! 2. tenants within today's soft CPU-hours before those over it (inside a
//!    band: an over-quota server's CI still goes before anyone's Normal);
//! 3. fair share: lowest `decayed usage / weight`;
//! 4. tie: rotate after the tenant served last.
//!
//! Inside a tenant: the best band first. CI, Priority and aged Normal go
//! strictly by the time *the node* received them; fresh Normal rotates
//! across workspaces, then sessions, then FIFO by node arrival. While
//! building the order each pick is charged its estimated CPU, so the order
//! interleaves tenants by weight instead of draining the cheapest one.
//!
//! **Tiers only order the queue**: no slot is reserved and nothing here
//! ever stops a running job (R2, R3). The 0.46.4 `gate_reserved_slots` and
//! "one gate job at a time" are gone (G3, G6).
//!
//! **Hard limits wait, never fail**: the tenant's `max_running`, the
//! workspace's `max_running`, the node's parallel/CPU/memory caps, an
//! exclusive job running (or waiting for an idle node), a node-wide block
//! (paused, foreign lock, low disk, an install), a tenant-wide block (share
//! paused, disk full, wall missing).
//!
//! **Next free slot / backfill**: when the first job held up by capacity is
//! CI or Priority, nothing ordered after it starts (hold 0): the node
//! drains toward it, nothing is killed. For a Normal head, later jobs that
//! fit may start until it has waited 20 min; then nothing else starts until
//! it fits. A CI or Priority job held by the secret-env isolation rule
//! (SE26) counts as waiting for capacity (T4); a tenant's own limits never
//! hold other tenants.
//!
//! **ETAs** simulate the queue: running jobs end at their p50 (or p90)
//! remaining time when known, then queued jobs start in order. Unknown
//! durations give no ETA. ETAs are durations, never clock times.
//!
//! No floats on the wire; usage decay uses `f64` internally only.

use std::collections::{BTreeMap, BTreeSet};

use crate::frames::JobState;
use crate::share::{self, blocked, codes, JobClass, Quote, QueueEntry, QueueView, ShareLimits, Slot, TenantTotals};
use crate::tiers::{self, Band, Tier};

/// Knobs (§7 "Defaults").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Usage half-life.
    pub half_life_secs: i64,
    /// A Normal job queued this long ranks above fresh Normal work, below
    /// Priority ([`tiers::AGE_SECS`]; T5).
    pub aging_secs: i64,
    /// The backfill hold: once the capacity-blocked head waited this long,
    /// nothing else starts until it fits.
    pub backfill_hold_secs: i64,
    /// What an unknown job is charged while ordering (CPU × this).
    pub default_est_secs: u64,
    /// Every slot's jobs share one uid (macOS); else one uid per slot
    /// (Linux). The secret-env isolation rule (SE26) reads it.
    pub one_uid: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { half_life_secs: 24 * 3600, aging_secs: tiers::AGE_SECS, backfill_hold_secs: 20 * 60, default_est_secs: 600, one_uid: false }
    }
}

/// Secret-env same-uid isolation (SE26): may a job of `ws` in `slot`
/// (`secret`: it gets vault secrets) start beside `running` = `(slot,
/// workspace, secret)`? Never beside another workspace's job under the
/// same uid when either of the two is a secret job; same-workspace jobs
/// may share it (the grant is per workspace anyway).
pub fn isolation_blocks<'a>(one_uid: bool, slot: Slot, ws: &str, secret: bool, running: impl IntoIterator<Item = (Slot, &'a str, bool)>) -> bool {
    running.into_iter().any(|(s, w, sec)| (one_uid || s == slot) && w != ws && (secret || sec))
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
    /// Its Priority counts here (`queue.toml`; default: the owner yes,
    /// guests no). Otherwise its Priority jobs run as Normal (T7).
    pub priority: bool,
}

impl Tenant {
    fn over_quota(&self) -> bool {
        self.cpu_secs_today >= u64::from(self.limits.cpu_hours_day) * 3600
    }

    /// The tier this tenant's job counts as here: CI only with the share's
    /// `gate`, Priority only where Priority counts for it (T7, T9).
    pub fn effective_tier(&self, tier: Tier) -> Tier {
        tiers::effective(tier, self.limits.allows_class(JobClass::Gate), self.priority)
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
    /// The tier it was sent with (shown in views; a running job is never
    /// stopped or reordered by its tier).
    pub tier: Tier,
    pub cpu_millis: u64,
    pub mem_bytes: u64,
    pub exclusive: bool,
    /// Expected seconds left (p50 / p90); `None` = unknown.
    pub left_p50: Option<u64>,
    pub left_p90: Option<u64>,
    /// It got vault secrets (SE26).
    pub secret: bool,
}

/// A job waiting in the node queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queued {
    pub slot: Slot,
    pub job_id: String,
    pub generation: u32,
    pub workspace_id: String,
    pub session_id: Option<String>,
    /// The plan's class (old peers; views).
    pub class: JobClass,
    /// The plan's tier (`Plan::tier_or_class`); what counts here is
    /// [`Tenant::effective_tier`].
    pub tier: Tier,
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
    /// It gets vault secrets at spawn (`Plan.secret_env`, SE26).
    pub secret: bool,
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

/// The band the order uses: the tier the tenant may use here (CI without
/// the share's `gate`, or Priority where it doesn't count, is Normal), with
/// an hour-old Normal job aged above fresh Normal (T5).
fn band_of(q: &Queued, t: &Tenant, now: i64, cfg: &Config) -> Band {
    tiers::band(t.effective_tier(q.tier), now - q.enqueued_at, cfg.aging_secs)
}

/// The next value after `last` in `keys`' order (wrapping), or the first.
fn rotate_after<'a>(keys: &BTreeSet<&'a str>, last: Option<&str>) -> Option<&'a str> {
    let last = match last {
        Some(l) => l,
        None => return keys.iter().next().copied(),
    };
    keys.iter().find(|k| **k > last).or_else(|| keys.iter().next()).copied()
}

/// Node arrival order: when the node received it, then its insert order.
fn arrival_key(q: &Queued) -> (i64, u64, &str) {
    (q.enqueued_at, q.arrival, q.job_id.as_str())
}

/// A tenant's next job: its best band; CI, Priority and aged Normal by
/// node arrival; fresh Normal by the workspace after the last served, then
/// the session after the last served there, then FIFO.
fn tenant_head(pending: &[usize], input: &Input<'_>, t: &Tenant, rot: &Rotation) -> usize {
    let q = input.queued;
    let best = pending.iter().map(|i| band_of(&q[*i], t, input.now, input.cfg)).max().expect("non-empty");
    let mine: Vec<usize> = pending.iter().copied().filter(|i| band_of(&q[*i], t, input.now, input.cfg) == best).collect();
    if best.by_arrival() {
        return mine.into_iter().min_by(|a, b| arrival_key(&q[*a]).cmp(&arrival_key(&q[*b]))).expect("a job");
    }
    let wss: BTreeSet<&str> = mine.iter().map(|i| q[*i].workspace_id.as_str()).collect();
    let ws = rotate_after(&wss, rot.last_ws.get(&t.slot).map(String::as_str)).expect("a workspace");
    let in_ws: Vec<usize> = mine.iter().copied().filter(|i| q[*i].workspace_id == ws).collect();
    let sessions: BTreeSet<&str> = in_ws.iter().map(|i| q[*i].session_id.as_deref().unwrap_or("")).collect();
    let last_s = rot.last_session.get(&(t.slot, ws.to_string())).map(String::as_str);
    let s = rotate_after(&sessions, last_s).expect("a session");
    in_ws
        .into_iter()
        .filter(|i| q[*i].session_id.as_deref().unwrap_or("") == s)
        .min_by(|a, b| arrival_key(&q[*a]).cmp(&arrival_key(&q[*b])))
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
    away.sort_by(|a, b| arrival_key(&q[*a]).cmp(&arrival_key(&q[*b])));
    out.extend(away);
    out
}

/// Does candidate `a` go before `b`? (Rules 1–4 in the module header.)
fn before(input: &Input<'_>, a: (usize, Slot), b: (usize, Slot), usage: &BTreeMap<Slot, u128>, today: &BTreeMap<Slot, u64>, rot: &Rotation) -> bool {
    let (ta, tb) = (tenant_of(input.tenants, a.1).expect("known"), tenant_of(input.tenants, b.1).expect("known"));
    let (qa, qb) = (&input.queued[a.0], &input.queued[b.0]);
    let (ba, bb) = (band_of(qa, ta, input.now, input.cfg), band_of(qb, tb, input.now, input.cfg));
    // 1. the tier band, highest first.
    if ba != bb {
        return ba > bb;
    }
    // 2. within quota first (inside a band, T22).
    let over = |t: &Tenant| today.get(&t.slot).copied().unwrap_or(0) >= u64::from(t.limits.cpu_hours_day) * 3600;
    if over(ta) != over(tb) {
        return !over(ta);
    }
    // 3. lowest usage / weight (cross-multiplied, no floats).
    let (ua, ub) = (usage.get(&a.1).copied().unwrap_or(0), usage.get(&b.1).copied().unwrap_or(0));
    let (wa, wb) = (u128::from(ta.limits.weight.max(1)), u128::from(tb.limits.weight.max(1)));
    if ua * wb != ub * wa {
        return ua * wb < ub * wa;
    }
    // 4. rotate after the tenant served last.
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
    secret: bool,
    cpu: u64,
    mem: u64,
    exclusive: bool,
    /// Simulated end time; `None` = unknown.
    end: Option<u64>,
}

/// Why a job can't start in this state: `Wait` lets later jobs go
/// (tenant or workspace limits); `Capacity` is the node being full (the
/// hold applies: at once behind a CI or Priority head, after 20 min behind
/// a Normal one); `Never` can't fit even on an idle node.
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
    // The tenant's and the workspace's own limits: `Wait`, never a hold on
    // other tenants (§2.5 of the vs-live).
    let mine = run.iter().filter(|r| r.slot == e.slot).count() as u32;
    if mine >= t.limits.max_running {
        return Fit::Wait;
    }
    if let Some(m) = e.ws_max_running {
        if run.iter().filter(|r| r.slot == e.slot && r.ws == e.workspace_id).count() as u32 >= m.max(1) {
            return Fit::Wait;
        }
    }
    // SE26: no secret job beside another workspace's job under one uid. A
    // CI or Priority job waits for the node to drain toward it (T4: as
    // `Wait` it could starve while other workspaces keep starting).
    if isolation_blocks(input.cfg.one_uid, e.slot, &e.workspace_id, e.secret, run.iter().map(|r| (r.slot, r.ws.as_str(), r.secret))) {
        return if band_of(e, t, input.now, input.cfg).holds() { Fit::Capacity } else { Fit::Wait };
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
            secret: r.secret,
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
        // Start pass at `now`, in order, with the hold: the first job that
        // waits for capacity holds every later start at once when it is
        // CI or Priority ("next free slot"), after 20 min when it is
        // Normal. The order is band-first, so nothing of a higher band
        // ever sits behind it.
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
                    run.push(SimRun { slot: e.slot, ws: e.workspace_id.clone(), secret: e.secret, cpu, mem, exclusive: e.exclusive, end: est.map(|d| now + d.max(1)) });
                    start_at[*i] = Some(now);
                    started.push(*i);
                }
                Fit::Capacity if !capacity_head_seen => {
                    capacity_head_seen = true;
                    let waited = input.now - e.enqueued_at + now as i64;
                    // Its band as of the real now (ETAs don't age a job
                    // into a hold it doesn't have yet).
                    if band_of(e, t, input.now, input.cfg).holds() || waited >= input.cfg.backfill_hold_secs {
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
        // SE26: say why it waits when only the isolation rule holds it now.
        if blocked_by.is_none()
            && !now
            && isolation_blocks(input.cfg.one_uid, e.slot, &e.workspace_id, e.secret, input.running.iter().map(|r| (r.slot, r.workspace_id.as_str(), r.secret)))
        {
            blocked_by = Some(crate::secret::codes::ISOLATION_WAIT.into());
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
        tier: Tier::Normal,
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
        secret: false,
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
    /// It said protocol ≥ 5: entries carry their tier.
    pub tiers: bool,
}

/// The queue as `viewer` may see it (§7): its own jobs in full; others'
/// as "another server" with class, tier, state, size and a rounded ETA
/// only (never who sent them).
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
            } else {
                // Why another server's job waits (its quota, its pause, its
                // disk) is that server's business (review B1).
                e.blocked_by = None;
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
            tier: viewer.tiers.then(|| tenant_of(input.tenants, r.slot).map(|t| t.effective_tier(r.tier)).unwrap_or(Tier::Normal)),
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
            tier: viewer.tiers.then(|| tenant_of(input.tenants, q.slot).map(|t| t.effective_tier(q.tier)).unwrap_or(Tier::Normal)),
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
    use crate::share::Trust;

    fn limits(weight: u32) -> ShareLimits {
        ShareLimits { weight, max_running: 8, max_queued: 1000, cpu_hours_day: 1000, ..ShareLimits::new_server(Trust::Own, false) }
    }

    /// The owner (slot 0) has Priority by default; guests don't.
    fn tenant(slot: Slot, weight: u32) -> Tenant {
        Tenant { slot, limits: limits(weight), usage: 0, cpu_secs_today: 0, blocked: None, priority: slot == crate::share::OWNER_SLOT }
    }

    /// A tenant whose share allows CI (`gate`) and whose Priority counts.
    fn gate_tenant(slot: Slot) -> Tenant {
        let mut t = tenant(slot, 1);
        t.limits.top_class = JobClass::Gate;
        t.limits.gate = true;
        t.priority = true;
        t
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
            tier: Tier::Normal,
            cpu_millis: Some(1000),
            mem_bytes: Some(1 << 30),
            exclusive: false,
            enqueued_at: at,
            arrival: 0,
            est_p50: Some(60),
            est_p90: Some(120),
            ws_max_running: None,
            eligible: true,
            secret: false,
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
            tier: Tier::Normal,
            cpu_millis: cpu,
            mem_bytes: 1 << 30,
            exclusive: false,
            left_p50: left,
            left_p90: left.map(|l| l * 2),
            secret: false,
        }
    }

    /// SE26: a secret job never starts beside another workspace's job of
    /// the same slot (one uid on Linux), nor that job beside it; same
    /// workspace and other slots may; on macOS (one uid) other slots wait
    /// too. The waiting job says `secret_isolation_wait`.
    #[test]
    fn a_secret_job_never_shares_a_uid_with_another_workspace() {
        let mut w = World::new(4, vec![tenant(0, 1), tenant(1, 1)]);
        let mut r = running(0, "other-ws", 1000, Some(60));
        r.workspace_id = "w-other".into();
        w.running = vec![r];
        let mut sec = q(0, "sec", "w-rel", 1);
        sec.secret = true;
        let same_slot_other_ws = sec.clone();
        w.queued = vec![same_slot_other_ws];
        let d = w.decide();
        assert!(d.start.is_empty(), "waits beside another workspace's job");
        assert_eq!(d.info[0].blocked_by.as_deref(), Some(crate::secret::codes::ISOLATION_WAIT));
        // Another slot (another uid on Linux): starts.
        let mut other_slot = sec.clone();
        other_slot.slot = 1;
        w.queued = vec![other_slot.clone()];
        assert_eq!(w.decide().start, vec![0]);
        // macOS: one uid for every slot: waits.
        w.cfg.one_uid = true;
        assert!(w.decide().start.is_empty());
        w.cfg.one_uid = false;
        // Same workspace beside it: fine.
        w.running[0].workspace_id = "w-rel".into();
        w.queued = vec![sec.clone()];
        assert_eq!(w.decide().start, vec![0]);
        // A plain job of another workspace beside a running secret job: waits.
        w.running[0].secret = true;
        w.queued = vec![q(0, "plain", "w-x", 1)];
        let d = w.decide();
        assert!(d.start.is_empty());
        assert_eq!(d.info[0].blocked_by.as_deref(), Some(crate::secret::codes::ISOLATION_WAIT));
        // Two queued at once: the secret job and another workspace's job
        // never start together in one pass.
        w.running.clear();
        w.queued = vec![sec.clone(), q(0, "plain", "w-x", 2)];
        assert_eq!(w.decide().start.len(), 1);
        assert!(isolation_blocks(false, 0, "a", true, [(0u8, "b", false)]));
        assert!(!isolation_blocks(false, 0, "a", false, [(0u8, "b", false)]), "no secret anywhere: no rule");
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
            // Jobs queued at small times are under an hour old (fresh
            // Normal) unless a test ages them on purpose.
            Self { cfg: Config::default(), caps: caps(par), tenants, running: vec![], queued: vec![], rot: Rotation::default(), now: 1_000 }
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

    /// CI jumps the queue and takes every free slot (no "one gate job at a
    /// time", G6), and it never stops a running job (R3).
    #[test]
    fn ci_jumps_the_queue_takes_every_free_slot_and_never_kills() {
        let mut w = World::new(4, vec![gate_tenant(0), tenant(1, 1)]);
        w.running = vec![running(1, "r1", 1000, Some(600))];
        w.queued = vec![q(1, "n1", "w", 1), q(0, "c1", "w", 5), q(0, "c2", "w", 6)];
        w.queued[1].tier = Tier::Ci;
        w.queued[2].tier = Tier::Ci;
        assert_eq!(w.order(), vec!["c1", "c2", "n1"]);
        let d = w.decide();
        assert_eq!(w.ids(&d.start), vec!["c1", "c2", "n1"], "every free slot fills; two CI jobs run at once");
        // A full node: nothing is stopped for CI, it waits for the next end.
        w.running = (0..4).map(|i| running(1, &format!("r{i}"), 1000, Some(600))).collect();
        let d = w.decide();
        assert!(d.start.is_empty());
        assert_eq!(d.info[1].eta_secs, Some(600));
        // CI from a server whose share has no `gate` counts as Normal.
        w.running.clear();
        w.caps = caps(1);
        w.queued = vec![q(0, "n0", "w", 1), q(1, "c1", "w", 5)];
        w.queued[1].tier = Tier::Ci;
        assert_eq!(w.order(), vec!["n0", "c1"], "slot 1 can't use CI: it takes its turn as Normal");
    }

    /// R2 / G3: no slot is ever held back. With three slots and two long
    /// runs, the third goes to Normal work even where servers may send CI.
    #[test]
    fn no_slot_is_reserved_for_ci() {
        let mut w = World::new(3, vec![gate_tenant(0), tenant(1, 1)]);
        w.running = vec![running(1, "t1", 1000, Some(7200)), running(1, "t2", 1000, Some(7200))];
        w.queued = vec![q(1, "n1", "w", 1)];
        assert_eq!(w.ids(&w.decide().start), vec!["n1"]);
    }

    /// R1 "next free slot": a CI or Priority head that waits for room holds
    /// every later start, even a small job that would fit; the node drains
    /// toward it and nothing is killed. A Normal head lets it backfill.
    #[test]
    fn a_ci_or_priority_head_takes_the_next_free_slot() {
        for tier in [Tier::Ci, Tier::Priority] {
            let mut w = World::new(4, vec![gate_tenant(0), tenant(1, 1)]);
            w.running = vec![running(1, "big", 6000, Some(300))];
            let mut head = q(0, "head", "w", w.now - 5);
            head.tier = tier;
            head.cpu_millis = Some(4000);
            w.queued = vec![head, q(1, "small", "w", w.now - 600)];
            let d = w.decide();
            assert!(d.start.is_empty(), "{tier:?}: nothing starts ahead of it: {:?}", w.ids(&d.start));
            assert_eq!((d.info[0].pos, d.info[0].eta_secs), (1, Some(300)), "{tier:?}: it starts when the running job ends");
            assert_eq!(d.info[1].eta_secs, Some(300), "{tier:?}: the small job starts beside it then");
        }
        let mut w = World::new(4, vec![gate_tenant(0), tenant(1, 1)]);
        w.running = vec![running(1, "big", 6000, Some(300))];
        let mut head = q(0, "head", "w", w.now - 5);
        head.cpu_millis = Some(4000);
        w.queued = vec![head, q(1, "small", "w", w.now - 1)];
        assert_eq!(w.ids(&w.decide().start), vec!["small"], "a Normal head lets small jobs backfill (its 20 minutes)");
    }

    #[test]
    fn priority_never_goes_before_ci_and_ci_never_waits_behind_priority() {
        let mut w = World::new(1, vec![gate_tenant(0), gate_tenant(1)]);
        let mut p = q(0, "p-old", "w", 1);
        p.tier = Tier::Priority;
        let mut c = q(1, "ci-new", "w", 900);
        c.tier = Tier::Ci;
        w.queued = vec![p, c, q(0, "n-oldest", "w", 0)];
        assert_eq!(w.order(), vec!["ci-new", "p-old", "n-oldest"]);
        let d = w.decide();
        assert_eq!(w.ids(&d.start), vec!["ci-new"]);
        assert_eq!(d.info.iter().map(|i| i.pos).collect::<Vec<_>>(), vec![2, 1, 3]);
    }

    /// T5: an hour-old Normal job ranks above fresh Normal work but below
    /// Priority, so Priority keeps "next free slot" under load. The old
    /// `batch` class is just Normal (G4).
    #[test]
    fn aged_normal_ranks_between_priority_and_fresh_normal() {
        let mut w = World::new(1, vec![gate_tenant(0)]);
        w.now = 10_000;
        let aged = q(0, "aged", "w-a", w.now - 3600);
        let fresh = q(0, "fresh", "w-b", w.now - 10);
        let mut p = q(0, "p", "w-c", w.now - 1);
        p.tier = Tier::Priority;
        w.queued = vec![fresh.clone(), aged.clone(), p];
        assert_eq!(w.order(), vec!["p", "aged", "fresh"]);
        // Workspaces would take turns (w-b after w-a was served), but the
        // aged job ranks above fresh work.
        w.rot.last_ws.insert(0, "w-a".into());
        w.queued = vec![fresh, aged];
        assert_eq!(w.order(), vec!["aged", "fresh"]);
        let mut b = q(0, "old-batch", "w", w.now - 5);
        b.class = JobClass::Batch;
        w.queued = vec![b, q(0, "later", "w", w.now - 1)];
        assert_eq!(w.order(), vec!["old-batch", "later"], "a detached (batch) job is plain Normal, FIFO");
    }

    /// T7: a guest server's Priority counts only where the node's owner
    /// allows it; otherwise it runs, and shows, as Normal. Views carry the
    /// tier only for protocol-5 servers.
    #[test]
    fn a_guests_priority_counts_only_where_the_owner_allows_it() {
        let mut w = World::new(1, vec![tenant(0, 1), tenant(1, 1)]);
        let mut gp = q(1, "guest-p", "w", 2);
        gp.tier = Tier::Priority;
        w.queued = vec![q(0, "owner-n", "w", 1), gp];
        assert_eq!(w.order(), vec!["owner-n", "guest-p"], "no priority for slot 1: Normal; slot 0 wins the tie");
        w.tenants[1].priority = true;
        assert_eq!(w.order(), vec!["guest-p", "owner-n"]);
        let labels = BTreeMap::new();
        let d = w.decide();
        let v = view(&w.input(), &d, &Viewer { slot: 0, owner: true, labels: &labels, tiers: true }, "r");
        assert_eq!(v.entries.iter().map(|e| e.tier).collect::<Vec<_>>(), vec![Some(Tier::Priority), Some(Tier::Normal)]);
        w.tenants[1].priority = false;
        let d = w.decide();
        let v = view(&w.input(), &d, &Viewer { slot: 1, owner: false, labels: &labels, tiers: true }, "r");
        assert!(v.entries.iter().all(|e| e.tier == Some(Tier::Normal)), "shown as it counts: {:?}", v.entries);
        let old = view(&w.input(), &d, &Viewer { slot: 1, owner: false, labels: &labels, tiers: false }, "r");
        assert!(old.entries.iter().all(|e| e.tier.is_none()), "a protocol-4 server gets no tier field");
        let mut r = running(0, "r", 1000, Some(60));
        r.tier = Tier::Ci;
        w.tenants[0] = gate_tenant(0);
        w.running = vec![r];
        let d = w.decide();
        let v = view(&w.input(), &d, &Viewer { slot: 1, owner: false, labels: &labels, tiers: true }, "r");
        assert_eq!((v.entries[0].state, v.entries[0].tier, v.entries[0].job_id.as_deref()), (JobState::Running, Some(Tier::Ci), None));
    }

    /// T4: a CI job with secrets on a one-uid node waits for the node to
    /// drain toward it, instead of starving while other workspaces start.
    #[test]
    fn a_ci_job_with_secrets_drains_the_node_toward_it() {
        let mut w = World::new(4, vec![gate_tenant(0)]);
        w.cfg.one_uid = true;
        let mut r = running(0, "other", 1000, Some(120));
        r.workspace_id = "w-other".into();
        w.running = vec![r];
        let mut ci = q(0, "ci", "w-rel", w.now - 5);
        ci.tier = Tier::Ci;
        ci.secret = true;
        w.queued = vec![ci.clone(), q(0, "n", "w-x", w.now - 1)];
        let d = w.decide();
        assert!(d.start.is_empty(), "no other workspace starts beside the waiting CI job: {:?}", w.ids(&d.start));
        assert_eq!(d.info[0].eta_secs, Some(120));
        assert_eq!(d.info[0].blocked_by.as_deref(), Some(crate::secret::codes::ISOLATION_WAIT));
        // A Normal secret job keeps today's rule: it waits, others go.
        ci.tier = Tier::Normal;
        w.queued = vec![ci, q(0, "n", "w-x", w.now - 1)];
        assert_eq!(w.ids(&w.decide().start), vec!["n"]);
    }

    /// A server's own limits are `Wait`: a CI job held by its own server's
    /// `max_running` never holds other servers' work.
    #[test]
    fn a_tenants_own_limits_never_hold_other_tenants() {
        let mut t0 = gate_tenant(0);
        t0.limits.max_running = 1;
        let mut w = World::new(4, vec![t0, tenant(1, 1)]);
        w.running = vec![running(0, "mine", 1000, Some(100))];
        let mut ci = q(0, "ci", "w", 1);
        ci.tier = Tier::Ci;
        w.queued = vec![ci, q(1, "guest", "w", 2)];
        let d = w.decide();
        assert_eq!(w.ids(&d.start), vec!["guest"]);
        assert_eq!(d.info[0].eta_secs, Some(100));
    }

    /// CI, Priority and aged Normal go strictly by node arrival inside a
    /// server (no workspace or session turns); fresh Normal keeps them.
    #[test]
    fn urgent_tiers_go_by_arrival_inside_a_server() {
        let mut w = World::new(1, vec![gate_tenant(0)]);
        let mk = |job: &str, ws: &str, sess: &str, at: i64, tier: Tier| {
            let mut e = q(0, job, ws, at);
            e.session_id = Some(sess.into());
            e.tier = tier;
            e
        };
        w.queued = vec![mk("p1", "a", "x", 1, Tier::Priority), mk("p2", "a", "x", 2, Tier::Priority), mk("p3", "b", "y", 3, Tier::Priority)];
        w.rot.last_ws.insert(0, "a".into());
        assert_eq!(w.order(), vec!["p1", "p2", "p3"], "no workspace turns for Priority");
        for e in w.queued.iter_mut() {
            e.tier = Tier::Normal;
        }
        assert_eq!(w.order(), vec!["p3", "p1", "p2"], "fresh Normal: workspaces take turns (b after a)");
    }

    /// T22: CPU-hours sort inside a band, never across: an over-quota
    /// server's CI still goes before a within-quota server's Normal.
    #[test]
    fn quota_sorts_inside_a_band_never_across() {
        let mut a = gate_tenant(0);
        a.cpu_secs_today = 1000 * 3600; // at its soft limit
        let mut w = World::new(1, vec![a, tenant(1, 1)]);
        w.queued = vec![q(0, "a-ci", "w", 1), q(1, "b-n", "w", 2)];
        w.queued[0].tier = Tier::Ci;
        assert_eq!(w.order(), vec!["a-ci", "b-n"]);
        w.queued[0].tier = Tier::Normal;
        assert_eq!(w.order(), vec!["b-n", "a-ci"], "same band: within quota first");
        let d = w.decide();
        assert_eq!(w.ids(&d.start), vec!["b-n"]);
        assert_eq!(d.info[0].blocked_by.as_deref(), Some(blocked::OVER_QUOTA), "it waits, it doesn't fail");
        assert!(d.info[0].eta_secs.is_some(), "over quota still gets an ETA");
    }

    /// Random queues (a fixed LCG, no extra crates): the order is always
    /// band-first; whenever a CI or Priority job can't start (here only
    /// for room: no tenant limits, secrets or exclusive jobs), nothing
    /// ordered after it starts; and starts never overfill the node. A
    /// decision only ever starts jobs: running ones are input (R3).
    #[test]
    fn random_queues_keep_band_order_and_the_next_free_slot() {
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move |n: u64| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) % n
        };
        for round in 0..2000 {
            let par = 1 + next(4) as u32;
            let mut w = World::new(par, vec![gate_tenant(0), gate_tenant(1), tenant(2, 1)]);
            w.now = 20_000;
            let now = w.now;
            let nr = next(u64::from(par) + 1);
            let mut running_jobs = Vec::new();
            for i in 0..nr {
                running_jobs.push(running(next(3) as u8, &format!("r{i}"), 500 + 500 * next(4), Some(30 + 60 * next(10))));
            }
            w.running = running_jobs;
            let n = 1 + next(8);
            let mut queued = Vec::new();
            for i in 0..n {
                let mut e = q(next(3) as u8, &format!("j{i}"), &format!("w{}", next(3)), now - next(7200) as i64);
                e.tier = [Tier::Normal, Tier::Normal, Tier::Priority, Tier::Ci][next(4) as usize];
                e.cpu_millis = Some(500 + 500 * next(8));
                e.arrival = i;
                queued.push(e);
            }
            w.queued = queued;
            let d = w.decide();
            let bands: Vec<Band> = d
                .order
                .iter()
                .map(|i| {
                    let e = &w.queued[*i];
                    band_of(e, tenant_of(&w.tenants, e.slot).expect("tenant"), w.now, &w.cfg)
                })
                .collect();
            assert!(bands.windows(2).all(|p| p[0] >= p[1]), "round {round}: not band-first: {bands:?}");
            let started: BTreeSet<usize> = d.start.iter().copied().collect();
            if let Some(k) = (0..d.order.len()).find(|k| !started.contains(&d.order[*k]) && bands[*k].holds()) {
                let after: Vec<String> = d.order[k + 1..].iter().filter(|i| started.contains(i)).map(|i| w.queued[*i].job_id.clone()).collect();
                assert!(after.is_empty(), "round {round}: {after:?} started past a waiting {:?} head", bands[k]);
            }
            assert!(w.running.len() + d.start.len() <= par as usize, "round {round}: overfilled");
            let cpu: u64 = w.running.iter().map(|r| r.cpu_millis).sum::<u64>() + d.start.iter().map(|i| w.queued[*i].cpu_millis.expect("sized")).sum::<u64>();
            assert!(cpu <= w.caps.cpu_millis || (w.running.is_empty() && d.start.len() == 1), "round {round}: CPU overfilled");
        }
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
        let v = view(&w.input(), &d, &Viewer { slot: 1, owner: false, labels: &labels, tiers: false }, "r1");
        assert_eq!(v.request_id, "r1");
        let other = &v.entries[0];
        assert!(!other.mine && other.job_id.is_none() && other.workspace_id.is_none() && other.session_id.is_none() && other.server_label.is_none());
        assert_eq!((other.state, other.eta_secs), (JobState::Running, Some(420)), "rounded");
        let me = &v.entries[1];
        assert!(me.mine && me.job_id.as_deref() == Some("my-job") && me.session_id.as_deref() == Some("sess"));
        assert!(v.totals.is_empty());
        let text = crate::canonical::to_string(&v).unwrap();
        assert!(!text.contains("secret-job") && !text.contains("owner-ws") && !text.contains("rosson"), "{text}");
        let o = view(&w.input(), &d, &Viewer { slot: 0, owner: true, labels: &labels, tiers: false }, "r2");
        assert_eq!(o.entries[1].server_label.as_deref(), Some("baden.k2.dev"));
        assert!(o.entries[1].job_id.is_none(), "the owner sees labels, not other servers' jobs");
        assert_eq!(o.totals.len(), 2);

        // B1: why another server's job waits stays that server's business;
        // its own server and the owner see it.
        w.tenants[1].blocked = Some(codes::SHARE_PAUSED.into());
        let d = w.decide();
        let peer = Viewer { slot: 2, owner: false, labels: &labels, tiers: false };
        let v = view(&w.input(), &d, &peer, "r3");
        assert!(v.entries.iter().all(|e| e.blocked_by.is_none()), "{:?}", v.entries);
        let own = view(&w.input(), &d, &Viewer { slot: 1, owner: false, labels: &labels, tiers: false }, "r4");
        assert_eq!(own.entries[1].blocked_by.as_deref(), Some(codes::SHARE_PAUSED));
        let o = view(&w.input(), &d, &Viewer { slot: 0, owner: true, labels: &labels, tiers: false }, "r5");
        assert_eq!(o.entries[1].blocked_by.as_deref(), Some(codes::SHARE_PAUSED));
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
