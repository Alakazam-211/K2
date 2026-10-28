//! The per-node fair queue (§10.2, lifted from FICC's fair scheduler).
//! Pure: the daemon feeds it the queue, what's running and the node's
//! last offer, and it answers which job to place next (one placement per
//! call) plus, for display, each queued job's position and reason. Solo
//! nodes only: a shared node keeps its own queue (`k2_node_proto::queue`).
//!
//! Order (the airport: prd-compute-job-tiers-v2 + Rosson 10-28;
//! [`super::proto::tiers`]):
//! - **CI** first, strictly by `created_at`;
//! - then **Priority** and **Standard** alternate (the one that didn't get
//!   the last placement first, [`Turns::last_line`]), so Standard never
//!   starves;
//! - then **Batch**, only on an idle node (nothing running, nothing else
//!   placed), one at a time;
//! - inside each line but CI: jobs waiting an hour or more first by
//!   `created_at` (they skip the turns, never the line); then round-robin
//!   across workspaces (after the last placed), then across submitters in
//!   each workspace (after the last placed there), strict FIFO by
//!   `created_at` per (workspace, submitter) for Standard and Batch;
//! - limits: the node's `parallel_max` and room, each grant's
//!   `max_parallel`, and the grant's per-submitter cap when set (L3; CI
//!   never counts);
//! - **next free slot**: a CI or Priority job that waits only for room on
//!   the node (`parallel_full`, `local_resource_limit`, `waiting_for_idle`,
//!   `exclusive_running`) holds every start ordered after it. Any other
//!   refusal (the grant's cap, a missing tool, a per-job disk ask) never
//!   holds (T3). A Standard head never holds, except an exclusive job at
//!   the head of the order waiting for an idle node, so a stream of small
//!   jobs can't starve it. Batch never holds.
//!
//! Tiers never stop a running job (R3): this only ever places queued ones.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use super::proto::frames::{Offer, Refusal};
use super::proto::refusal::{self, JobAsk};
use super::proto::tiers::{self, Line, Tier};

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
    /// The job's tier as the server stored it (CI only with `allow_ci`,
    /// checked at submit).
    pub tier: Tier,
    /// Who sent it (`compute_jobs.submitter`): the turns inside a
    /// workspace and the per-submitter cap. `None` = an old row.
    pub submitter: Option<String>,
}

impl QueuedJob {
    fn sub(&self) -> &str {
        self.submitter.as_deref().unwrap_or("")
    }
}

/// What is on the node right now (from the controller registry).
#[derive(Debug, Clone, Default)]
pub struct Load {
    pub running_by_workspace: HashMap<String, u32>,
    pub exclusive_running: bool,
    /// Running non-CI jobs per (workspace, submitter) (L3).
    pub running_by_submitter: HashMap<(String, String), u32>,
}

/// Who got the last placements on a node (the engine keeps one per node).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Turns {
    pub last_ws: Option<String>,
    /// Per workspace: the submitter placed last there.
    pub last_submitter: HashMap<String, String>,
    /// Priority or Standard: which got the last placement.
    pub last_line: Option<Line>,
}

impl Turns {
    /// Only the workspace rotation (0.46.5's state).
    pub fn ws(last: Option<&str>) -> Self {
        Self { last_ws: last.map(str::to_string), ..Default::default() }
    }

    /// `j` was placed.
    pub fn served(&mut self, j: &QueuedJob) {
        self.note(&j.workspace_id, j.submitter.as_deref(), j.tier);
    }

    /// A job of `workspace_id`, `submitter`, `tier` was placed (or sent).
    pub fn note(&mut self, workspace_id: &str, submitter: Option<&str>, tier: Tier) {
        self.last_ws = Some(workspace_id.to_string());
        self.last_submitter.insert(workspace_id.to_string(), submitter.unwrap_or("").to_string());
        let line = tier.line();
        if matches!(line, Line::Priority | Line::Standard) {
            self.last_line = Some(line);
        }
    }
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

/// Has `j` waited long enough to skip its line's turns (server clock)?
pub fn aged(j: &QueuedJob, now: i64) -> bool {
    tiers::aged(now - j.created_at, tiers::AGE_SECS)
}

fn fifo(a: &&QueuedJob, b: &&QueuedJob) -> std::cmp::Ordering {
    a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id))
}

/// `keys` (sorted, unique) rotated to start just after `last`.
fn rotated<'a>(mut keys: Vec<&'a str>, last: Option<&str>) -> Vec<&'a str> {
    keys.sort_unstable();
    keys.dedup();
    if let Some(last) = last {
        let start = keys.iter().position(|k| *k > last).unwrap_or(0);
        keys.rotate_left(start);
    }
    keys
}

/// Round-robin over `groups` in `keys` order until all are empty.
fn round_robin<'a, 'k>(keys: &[&'k str], mut groups: HashMap<&'k str, VecDeque<&'a QueuedJob>>) -> Vec<&'a QueuedJob> {
    let mut out = Vec::new();
    loop {
        let mut any = false;
        for k in keys {
            if let Some(j) = groups.get_mut(k).and_then(VecDeque::pop_front) {
                out.push(j);
                any = true;
            }
        }
        if !any {
            return out;
        }
    }
}

/// One line's order: aged jobs by `created_at` (they skip the turns); then
/// workspaces round-robin (after the last placed), each workspace's own
/// sequence round-robin over its submitters (after the last placed
/// there), FIFO per submitter.
fn line_order<'a>(now: i64, jobs: Vec<&'a QueuedJob>, turns: &Turns) -> Vec<&'a QueuedJob> {
    let (mut old, mut fresh): (Vec<&QueuedJob>, Vec<&QueuedJob>) = jobs.into_iter().partition(|j| aged(j, now));
    old.sort_by(fifo);
    fresh.sort_by(fifo);
    let wss = rotated(fresh.iter().map(|j| j.workspace_id.as_str()).collect(), turns.last_ws.as_deref());
    let mut per_ws: HashMap<&str, VecDeque<&QueuedJob>> = HashMap::new();
    for w in &wss {
        let mine: Vec<&QueuedJob> = fresh.iter().copied().filter(|j| j.workspace_id == *w).collect();
        let subs = rotated(mine.iter().map(|j| j.sub()).collect(), turns.last_submitter.get(*w).map(String::as_str));
        let mut by_sub: HashMap<&str, VecDeque<&QueuedJob>> = HashMap::new();
        for j in mine {
            by_sub.entry(j.sub()).or_default().push_back(j);
        }
        per_ws.insert(w, round_robin(&subs, by_sub).into());
    }
    let mut out = old;
    out.extend(round_robin(&wss, per_ws));
    out
}

/// The start order at `now`: CI by `created_at`; then Priority and
/// Standard alternating (after `turns.last_line`); then Batch. Each line
/// but CI by [`line_order`]. Positions follow this order.
pub fn order_with<'a>(now: i64, queue: &'a [QueuedJob], turns: &Turns) -> Vec<&'a QueuedJob> {
    let of = |line: Line| queue.iter().filter(|j| j.tier.line() == line).collect::<Vec<&QueuedJob>>();
    let mut out = of(Line::Ci);
    out.sort_by(fifo);
    let mut pri: VecDeque<&QueuedJob> = line_order(now, of(Line::Priority), turns).into();
    let mut std: VecDeque<&QueuedJob> = line_order(now, of(Line::Standard), turns).into();
    let mut next = tiers::alternate_after(turns.last_line);
    while !pri.is_empty() || !std.is_empty() {
        let (a, b) = if next == Line::Priority { (&mut pri, &mut std) } else { (&mut std, &mut pri) };
        if let Some(j) = a.pop_front().or_else(|| b.pop_front()) {
            out.push(j);
        }
        next = if next == Line::Priority { Line::Standard } else { Line::Priority };
    }
    out.extend(line_order(now, of(Line::Batch), turns));
    out
}

/// [`order_with`] with only a workspace rotation (0.46.5's call).
pub fn order<'a>(now: i64, queue: &'a [QueuedJob], last_workspace: Option<&str>) -> Vec<&'a QueuedJob> {
    order_with(now, queue, &Turns::ws(last_workspace))
}

/// Why everything after a held head waits.
fn hold_reason(j: &QueuedJob, line: Line) -> Refusal {
    match line {
        Line::Ci => Refusal::new(tiers::codes::TIER_HOLD, "a CI job ahead of it takes the next free slot"),
        Line::Priority => Refusal::new(tiers::codes::TIER_HOLD, "a Priority job ahead of it takes the next free slot"),
        _ if j.exclusive => Refusal::new(refusal::WAITING_FOR_IDLE, "an exclusive job ahead of it waits for an idle node"),
        _ => Refusal::new(tiers::codes::TIER_HOLD, "a job ahead of it takes the next free slot"),
    }
}

/// [`decide_with`] with only a workspace rotation and no per-submitter cap
/// (0.46.5's call).
pub fn decide(
    now: i64,
    queue: &[QueuedJob],
    last_workspace: Option<&str>,
    load: &Load,
    grant_max_parallel: &dyn Fn(&str) -> u32,
    offer: Option<&Offer>,
    controller_pause: Option<&str>,
) -> Decision {
    decide_with(now, queue, &Turns::ws(last_workspace), load, &Limits { grant_max_parallel, submitter_max_running: &|_| 0 }, offer, controller_pause)
}

/// The grant's caps, per workspace.
pub struct Limits<'a> {
    /// The grant's `max_parallel`.
    pub grant_max_parallel: &'a dyn Fn(&str) -> u32,
    /// The grant's `submitter_max_running` (0 = off; L3).
    pub submitter_max_running: &'a dyn Fn(&str) -> u32,
}

/// Decide one placement at `now` (server clock).
///
/// - `queue`: queued jobs on this node (any order).
/// - `turns`: who got the previous placements on this node.
/// - `caps`: the grant caps per workspace.
/// - `offer`: the node's last offer (`None` = offline).
/// - `controller_pause`: `Some("paused"|"draining")` when the controller
///   owner stopped new assignments.
pub fn decide_with(now: i64, queue: &[QueuedJob], turns: &Turns, load: &Load, caps: &Limits<'_>, offer: Option<&Offer>, controller_pause: Option<&str>) -> Decision {
    let grant_max_parallel = caps.grant_max_parallel;
    let jobs = order_with(now, queue, turns);
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
    let mut place: Option<String> = None;
    let mut hold: Option<Refusal> = None;
    let mut head_seen = false;
    // (workspace, submitter) pairs whose oldest Standard or Batch job was
    // already looked at (strict FIFO per submitter inside a workspace).
    let mut seen: HashSet<(&str, &str, Line)> = HashSet::new();
    for j in &jobs {
        let pos = position[j.id.as_str()];
        // A job over the node's caps can never start: report it (the
        // daemon fails it) and leave it out, so it never holds the jobs
        // behind it.
        if let Some(r) = never_fits(offer, j) {
            waiting.insert(j.id.clone(), (pos, r));
            continue;
        }
        let is_head = !std::mem::replace(&mut head_seen, true);
        if let Some(h) = &hold {
            waiting.insert(j.id.clone(), (pos, h.clone()));
            continue;
        }
        let line = j.tier.line();
        if matches!(line, Line::Standard | Line::Batch) && !seen.insert((j.workspace_id.as_str(), j.sub(), line)) {
            waiting.insert(j.id.clone(), (pos, Refusal::new("behind_older_job", "an older job from the same submitter goes first")));
            continue;
        }
        let verdict = match refusal::offer_refusal(offer, load.total(), load.exclusive_running, &ask(j)) {
            Some(r) => Err(with_numbers(r, offer, j, load.total())),
            None if !within_grant(j, load, grant_max_parallel) => Err(grant_full()),
            None if !within_submitter_cap(j, load, caps.submitter_max_running) => Err(submitter_full(j, caps.submitter_max_running)),
            // L1: Batch only on an idle node, and only when nothing else
            // is placed now (one at a time).
            None if line == Line::Batch && (load.total() > 0 || place.is_some()) => Err(Refusal::new(
                super::proto::share::blocked::BATCH_WINDOW,
                "Batch: it starts when the node is idle (nothing running, nothing else ready)",
            )),
            None => Ok(()),
        };
        match verdict {
            Ok(()) if place.is_none() => place = Some(j.id.clone()),
            Ok(()) => {
                waiting.insert(j.id.clone(), (pos, Refusal::new("next_in_turn", "next in turn")));
            }
            Err(r) => {
                // Next free slot: only while it waits for room, never for a
                // refusal that could last (T3); CI and Priority, and an
                // exclusive job at the head of the order.
                if tiers::waits_for_room(&r.code) && (line.holds() || (is_head && j.exclusive && line != Line::Batch)) {
                    hold = Some(hold_reason(j, line));
                }
                waiting.insert(j.id.clone(), (pos, r));
            }
        }
    }
    Decision { place, waiting }
}

fn within_grant(j: &QueuedJob, load: &Load, cap: &dyn Fn(&str) -> u32) -> bool {
    load.running_by_workspace.get(&j.workspace_id).copied().unwrap_or(0) < cap(&j.workspace_id).max(1)
}

/// L3: the per-submitter cap (0 = off); CI never counts or waits.
fn within_submitter_cap(j: &QueuedJob, load: &Load, cap: &dyn Fn(&str) -> u32) -> bool {
    let c = cap(&j.workspace_id);
    c == 0 || j.tier == Tier::Ci || load.running_by_submitter.get(&(j.workspace_id.clone(), j.sub().to_string())).copied().unwrap_or(0) < c
}

fn submitter_full(j: &QueuedJob, cap: &dyn Fn(&str) -> u32) -> Refusal {
    Refusal::new(
        super::proto::share::blocked::SUBMITTER_RUNNING_FULL,
        format!("this submitter already runs {} job(s) here (the grant's per-submitter cap); jobs behind it may start first", cap(&j.workspace_id)),
    )
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
            share: None,
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

    /// "Now" for tests whose jobs are all fresh (created at small times).
    const T0: i64 = 0;

    fn job(id: &str, ws: &str, at: i64) -> QueuedJob {
        QueuedJob { id: id.into(), workspace_id: ws.into(), created_at: at, exclusive: false, cpu_millis: None, mem_bytes: None, disk_bytes: 0, needs_git: true, tier: Tier::Normal, submitter: None }
    }

    fn tiered(id: &str, ws: &str, at: i64, tier: Tier) -> QueuedJob {
        QueuedJob { tier, ..job(id, ws, at) }
    }

    fn cap1(_: &str) -> u32 {
        1
    }

    #[test]
    fn fifo_within_a_workspace_and_round_robin_across() {
        let q = vec![job("a1", "A", 1), job("a2", "A", 2), job("b1", "B", 3)];
        let d = decide(T0, &q, None, &Load::default(), &cap1, Some(&offer(4)), None);
        assert_eq!(d.place.as_deref(), Some("a1"));
        // A just got a placement → B is next even though a2 is older.
        let q2 = vec![job("a2", "A", 2), job("b1", "B", 3)];
        let d = decide(T0, &q2, Some("A"), &Load::default(), &cap1, Some(&offer(4)), None);
        assert_eq!(d.place.as_deref(), Some("b1"));
        assert_eq!(d.waiting["a2"].1.code, "next_in_turn");
    }

    #[test]
    fn caps_and_reasons() {
        let q = vec![job("a1", "A", 1), job("b1", "B", 2)];
        let mut load = Load::default();
        load.running_by_workspace.insert("A".into(), 1);
        // Node parallel 2, A at its grant cap of 1 → B goes.
        let d = decide(T0, &q, None, &load, &cap1, Some(&offer(2)), None);
        assert_eq!(d.place.as_deref(), Some("b1"));
        assert_eq!(d.waiting["a1"].1.code, GRANT_PARALLEL_FULL);
        // Node full.
        load.running_by_workspace.insert("B".into(), 1);
        let d = decide(T0, &q, None, &load, &|_| 5, Some(&offer(2)), None);
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
        let d = decide(T0, &[big.clone()], None, &load, &|_| 4, Some(&o), None);
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
        assert_eq!(decide(T0, &q, None, &Load::default(), &cap1, None, None).waiting["a1"].1.code, "node_offline");
        assert_eq!(
            decide(T0, &q, None, &Load::default(), &cap1, Some(&offer(1)), Some("paused")).waiting["a1"].1.code,
            CONTROLLER_PAUSED
        );
        let mut o = offer(1);
        o.control = Control::Draining;
        assert_eq!(decide(T0, &q, None, &Load::default(), &cap1, Some(&o), None).waiting["a1"].1.code, refusal::LOCAL_DRAINING);
        let mut o = offer(1);
        o.refusal = Some(Refusal::new(refusal::FOREIGN_LOCK, "sew"));
        assert_eq!(decide(T0, &q, None, &Load::default(), &cap1, Some(&o), None).waiting["a1"].1.code, refusal::FOREIGN_LOCK);
    }

    #[test]
    fn oldest_exclusive_job_holds_the_node_until_idle() {
        let mut ex = job("x", "A", 1);
        ex.exclusive = true;
        let q = vec![ex.clone(), job("b1", "B", 2)];
        let mut load = Load::default();
        load.running_by_workspace.insert("C".into(), 1);
        let d = decide(T0, &q, None, &load, &|_| 5, Some(&offer(4)), None);
        assert_eq!(d.place, None, "nothing starts while the exclusive job waits for idle");
        assert_eq!(d.waiting["x"].1.code, refusal::WAITING_FOR_IDLE);
        assert_eq!(d.waiting["b1"].1.code, refusal::WAITING_FOR_IDLE);
        let d = decide(T0, &q, None, &Load::default(), &|_| 5, Some(&offer(4)), None);
        assert_eq!(d.place.as_deref(), Some("x"));
        // While it runs, nothing else starts.
        let mut running = Load::default();
        running.running_by_workspace.insert("A".into(), 1);
        running.exclusive_running = true;
        let d = decide(T0, &[job("b1", "B", 2)], Some("A"), &running, &|_| 5, Some(&offer(4)), None);
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
            let d = decide(T0, &queue, last.as_deref(), &Load::default(), &cap1, Some(&offer(1)), None);
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
        let d = decide(T0, &q, None, &Load::default(), &cap1, Some(&small(2)), None);
        assert_eq!(d.place.as_deref(), Some("a2"));
        let (pos, why) = d.waiting.get("big").expect("big is reported");
        assert_eq!((*pos, why.code.as_str()), (1, EXCEEDS_NODE_CAPS));
        assert_eq!(d.waiting.get("a3").expect("a3 waits").1.code, "behind_older_job");
        // Same for an exclusive job over the caps.
        let mut ex = mem_job("ex", "A", 1, 4);
        ex.exclusive = true;
        let d = decide(T0, &[ex, job("b1", "B", 2)], None, &Load::default(), &cap1, Some(&small(2)), None);
        assert_eq!(d.place.as_deref(), Some("b1"));
        assert_eq!(d.waiting.get("ex").expect("ex is reported").1.code, EXCEEDS_NODE_CAPS);
    }

    // ── tiers (prd-compute-job-tiers-v1, solo nodes) ──────────────────

    #[test]
    fn a_solo_node_orders_ci_then_priority_then_normal() {
        let q = vec![job("n", "A", 1), tiered("p", "B", 2, Tier::Priority), tiered("c", "C", 3, Tier::Ci)];
        let d = decide(T0, &q, None, &Load::default(), &cap1, Some(&offer(4)), None);
        assert_eq!(d.place.as_deref(), Some("c"));
        assert_eq!(d.waiting["p"].0, 2);
        assert_eq!(d.waiting["n"].0, 3, "positions follow the tier order, not created_at");
        assert_eq!(order(T0, &q, None).iter().map(|j| j.id.as_str()).collect::<Vec<_>>(), vec!["c", "p", "n"]);
    }

    /// R1 "next free slot": a CI or Priority head that waits for room holds
    /// every later start, even a small job that fits now.
    #[test]
    fn a_ci_or_priority_head_waiting_for_room_holds_the_node() {
        for tier in [Tier::Ci, Tier::Priority] {
            let mut o = offer(4);
            o.free.cpu_millis = 2000;
            let mut head = tiered("head", "A", 5, tier);
            head.cpu_millis = Some(4000);
            let mut load = Load::default();
            load.running_by_workspace.insert("C".into(), 1);
            let d = decide(T0, &[head, job("small", "B", 1)], None, &load, &|_| 5, Some(&o), None);
            assert_eq!(d.place, None, "{tier:?}: nothing starts ahead of it");
            assert_eq!(d.waiting["head"].1.code, refusal::LOCAL_RESOURCE_LIMIT);
            assert_eq!(d.waiting["small"].1.code, tiers::codes::TIER_HOLD);
        }
        // A Normal head in the same spot: the small job goes (no hold on solo).
        let mut o = offer(4);
        o.free.cpu_millis = 2000;
        let mut head = job("head", "A", 1);
        head.cpu_millis = Some(4000);
        let d = decide(T0, &[head, job("small", "B", 2)], None, &Load::default(), &|_| 5, Some(&o), None);
        assert_eq!(d.place.as_deref(), Some("small"));
    }

    /// T3: a CI head refused for anything but room (its grant's cap, a
    /// missing tool, a per-job disk ask) never holds the node.
    #[test]
    fn a_ci_head_refused_for_a_lasting_reason_never_holds_the_node() {
        let mut load = Load::default();
        load.running_by_workspace.insert("A".into(), 1);
        let d = decide(T0, &[tiered("ci", "A", 1, Tier::Ci), job("b", "B", 2)], None, &load, &cap1, Some(&offer(4)), None);
        assert_eq!(d.waiting["ci"].1.code, GRANT_PARALLEL_FULL);
        assert_eq!(d.place.as_deref(), Some("b"), "the grant's cap is the workspace's limit, not the node's room");
        let mut no_git = offer(4);
        no_git.tools.clear();
        let mut plain = job("b", "B", 2);
        plain.needs_git = false;
        let d = decide(T0, &[tiered("ci", "A", 1, Tier::Ci), plain.clone()], None, &Load::default(), &cap1, Some(&no_git), None);
        assert_eq!(d.waiting["ci"].1.code, refusal::TOOL_MISSING);
        assert_eq!(d.place.as_deref(), Some("b"), "a missing tool would hold the node forever");
        let mut disk = offer(4);
        disk.free.disk_free_bytes = 1 << 30;
        let mut p = tiered("p", "A", 1, Tier::Priority);
        p.disk_bytes = 10 << 30;
        let d = decide(T0, &[p, plain], None, &Load::default(), &cap1, Some(&disk), None);
        assert_eq!(d.waiting["p"].1.code, refusal::DISK_LOW);
        assert_eq!(d.place.as_deref(), Some("b"));
    }

    /// T5: an hour-old Normal job ranks above fresh Normal work and below
    /// Priority; on a solo node it never holds (Normal's rule), and strict
    /// FIFO inside its workspace still applies.
    #[test]
    fn aged_normal_ranks_below_priority_and_never_holds_a_solo_node() {
        let now = 10_000;
        let aged = job("aged", "A", now - 3600);
        let fresh = job("fresh", "B", now - 10);
        let p = tiered("p", "C", now - 1, Tier::Priority);
        let q = vec![fresh.clone(), aged.clone(), p];
        assert_eq!(order(now, &q, Some("A")).iter().map(|j| j.id.as_str()).collect::<Vec<_>>(), vec!["p", "aged", "fresh"]);
        let d = decide(now, &[fresh.clone(), aged.clone()], Some("A"), &Load::default(), &cap1, Some(&offer(4)), None);
        assert_eq!(d.place.as_deref(), Some("aged"), "aged beats the workspace turn");
        let mut o = offer(4);
        o.free.cpu_millis = 2000;
        let mut big = aged;
        big.cpu_millis = Some(4000);
        let d = decide(now, &[big, fresh], None, &Load::default(), &cap1, Some(&o), None);
        assert_eq!(d.place.as_deref(), Some("fresh"), "an old Normal job never drains a node");
        let mut a1 = job("a1", "A", now - 7200);
        a1.cpu_millis = Some(4000);
        let d = decide(now, &[a1, job("a2", "A", now - 3700)], None, &Load::default(), &cap1, Some(&o), None);
        assert_eq!(d.place, None);
        assert_eq!(d.waiting["a2"].1.code, "behind_older_job");
    }

    /// An exclusive Normal job waiting for idle no longer holds a CI job
    /// back: CI heads the order and the exclusive one waits behind it.
    #[test]
    fn an_exclusive_normal_job_never_blocks_ci() {
        let mut ex = job("ex", "A", 1);
        ex.exclusive = true;
        let mut load = Load::default();
        load.running_by_workspace.insert("C".into(), 1);
        let d = decide(T0, &[ex, tiered("ci", "B", 2, Tier::Ci)], None, &load, &|_| 5, Some(&offer(4)), None);
        assert_eq!(d.place.as_deref(), Some("ci"));
        assert_eq!(d.waiting["ex"].1.code, refusal::WAITING_FOR_IDLE);
        assert_eq!(d.waiting["ex"].0, 2);
    }

    fn by(id: &str, ws: &str, at: i64, sub: &str, tier: Tier) -> QueuedJob {
        QueuedJob { submitter: Some(sub.into()), tier, ..job(id, ws, at) }
    }

    fn ids(v: &[&QueuedJob]) -> Vec<String> {
        v.iter().map(|j| j.id.clone()).collect()
    }

    /// The airport (Rosson 10-28) on a solo node: CI, then Priority and
    /// Standard alternating (after the line placed last), then Batch.
    #[test]
    fn priority_and_standard_alternate_on_a_solo_node() {
        let q = vec![
            by("p1", "A", 1, "a", Tier::Priority),
            by("p2", "A", 2, "a", Tier::Priority),
            by("s1", "A", 3, "a", Tier::Normal),
            by("s2", "A", 4, "a", Tier::Normal),
            by("b1", "A", 0, "a", Tier::Batch),
            by("ci", "A", 9, "a", Tier::Ci),
        ];
        assert_eq!(ids(&order_with(T0, &q, &Turns::default())), vec!["ci", "p1", "s1", "p2", "s2", "b1"]);
        let after_p = Turns { last_line: Some(Line::Priority), ..Default::default() };
        assert_eq!(ids(&order_with(T0, &q, &after_p)), vec!["ci", "s1", "p1", "s2", "p2", "b1"]);
        // Placing follows the turns: after a Priority placement, Standard.
        let mut t = Turns::default();
        let q2: Vec<QueuedJob> = q.iter().filter(|j| j.tier != Tier::Ci).cloned().collect();
        let caps = Limits { grant_max_parallel: &|_| 9, submitter_max_running: &|_| 0 };
        let d = decide_with(T0, &q2, &t, &Load::default(), &caps, Some(&offer(4)), None);
        assert_eq!(d.place.as_deref(), Some("p1"));
        t.served(&q2[0]);
        let rest: Vec<QueuedJob> = q2.iter().filter(|j| j.id != "p1").cloned().collect();
        let mut load = Load::default();
        load.running_by_workspace.insert("A".into(), 1);
        let d = decide_with(T0, &rest, &t, &load, &caps, Some(&offer(4)), None);
        assert_eq!(d.place.as_deref(), Some("s1"), "Standard never starves behind a Priority burst");
        assert_eq!(d.waiting["b1"].1.code, "batch_window", "Batch waits for an idle node");
    }

    /// L1 on a solo node: Batch only when nothing runs and nothing else is
    /// placed; it never holds anyone.
    #[test]
    fn batch_waits_for_an_idle_solo_node() {
        let caps = Limits { grant_max_parallel: &|_| 9, submitter_max_running: &|_| 0 };
        let b = by("b", "A", 1, "a", Tier::Batch);
        let d = decide_with(T0, std::slice::from_ref(&b), &Turns::default(), &Load::default(), &caps, Some(&offer(4)), None);
        assert_eq!(d.place.as_deref(), Some("b"), "idle: it goes");
        let mut busy = Load::default();
        busy.running_by_workspace.insert("Z".into(), 1);
        let d = decide_with(T0, &[b.clone(), by("s", "A", 2, "a", Tier::Normal)], &Turns::default(), &busy, &caps, Some(&offer(4)), None);
        assert_eq!(d.place.as_deref(), Some("s"));
        assert_eq!(d.waiting["b"].1.code, "batch_window");
    }

    /// L4 on a solo node: submitters take turns inside a workspace (one
    /// builder's burst can't hold the others), FIFO per submitter; L3: the
    /// per-submitter cap, CI exempt.
    #[test]
    fn submitters_take_turns_and_the_cap_holds_one_builder() {
        let mut q: Vec<QueuedJob> = (0..4).map(|i| by(&format!("a{i}"), "W", i, "w/s/a", Tier::Normal)).collect();
        q.push(by("b0", "W", 10, "w/s/b", Tier::Normal));
        assert_eq!(ids(&order_with(T0, &q, &Turns::default()))[..3], ["a0", "b0", "a1"]);
        let mut t = Turns::ws(Some("W"));
        t.last_submitter.insert("W".into(), "w/s/a".into());
        assert_eq!(order_with(T0, &q, &t)[0].id, "b0", "after A, B's turn");
        // A runs 2; cap 2: A waits, B goes.
        let mut load = Load::default();
        load.running_by_workspace.insert("W".into(), 2);
        load.running_by_submitter.insert(("W".into(), "w/s/a".into()), 2);
        let caps = Limits { grant_max_parallel: &|_| 9, submitter_max_running: &|_| 2 };
        let d = decide_with(T0, &q, &Turns::default(), &load, &caps, Some(&offer(8)), None);
        assert_eq!(d.place.as_deref(), Some("b0"));
        assert_eq!(d.waiting["a0"].1.code, "submitter_running_full");
        // CI is never capped.
        let ci = by("ci", "W", 20, "w/s/a", Tier::Ci);
        let d = decide_with(T0, std::slice::from_ref(&ci), &Turns::default(), &load, &caps, Some(&offer(8)), None);
        assert_eq!(d.place.as_deref(), Some("ci"));
        // Off (0) by default.
        let off = Limits { grant_max_parallel: &|_| 9, submitter_max_running: &|_| 0 };
        let d = decide_with(T0, &q, &Turns::default(), &load, &off, Some(&offer(8)), None);
        assert_eq!(d.place.as_deref(), Some("a0"));
    }
}
