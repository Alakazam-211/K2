//! Displacement (0.46.18, research-compute-release-transitions-v1 rev 2,
//! as Rosson trimmed it): a CI-tier job that can't start may end one of
//! this server's own running Standard jobs on the node it needs. The
//! displaced run comes back as a new generation of the SAME job (same id,
//! no retry used, `wait --follow` and `fetch` carry on to its final run).
//!
//! - **Who displaces:** only CI-tier jobs (the CI tier is already granted
//!   per workspace; no separate list).
//! - **Who is displaced:** this server's Standard jobs, at any age. Never
//!   CI or Priority, never another server's job, never a soak: Batch, or
//!   an expected run ([`store::JobRow::est_secs`]) or `--timeout` over 2 h
//!   ([`is_soak`]). An unknown run time is not a soak.
//! - **Which one:** on the node the CI job needs, the one that has run the
//!   least first; among equals (the same minute), one whose `--cache` is
//!   warm on that node goes last ([`order`]). Enough of them to fit.
//! - **Go elsewhere first:** nothing is displaced while another node that
//!   fits starts the CI job now, or should within [`ELSEWHERE_SECS`].
//! - **The displaced job:** an auto job avoids that node for
//!   [`AVOID_SECS`]; a pinned job is next on its node (it can't run
//!   elsewhere), and is displaced at most once per CI job.
//! - **Soaks stay off release nodes:** a soak is never auto-placed on a
//!   node with this server's own label `release` ([`kept_off`], read from
//!   [`k2_core::compute::server_labels`]).
//!
//! It lives on this server only (the sweeper, every 5 s): a node sees an
//! ordinary cancel, then the CI job's assign.

use std::collections::HashMap;
use std::sync::Mutex;

use k2_core::compute::attempts::next;
use k2_core::compute::proto::frames::{JobState, Offer};
use k2_core::compute::proto::tiers::Tier as JobTier;
use k2_core::compute::share;
use k2_core::compute::store::{self, Displaced, JobRow};
use k2_core::compute::{self as compute, message};

use crate::compute_place::{self, Pending, Tier as PlaceTier, Want};
use crate::compute_ws;

/// A soak: Batch, or expected to run (or allowed to run) over this long.
pub const SOAK_SECS: i64 = 2 * 3600;
/// A displaced auto job stays off that node this long.
pub const AVOID_SECS: i64 = 30 * 60;
/// Nothing is displaced while a node that fits should free within this.
pub const ELSEWHERE_SECS: u64 = 5 * 60;
/// A new CI job's first seconds: ordinary placement goes first.
pub const SETTLE_SECS: i64 = 10;
/// After a displaced run is queued again, the CI job gets this long to
/// start in the slot before anything more is displaced for it (the
/// node's next offer must show the free slot first).
pub const START_GRACE_SECS: i64 = 60;
/// The `reason` of a run its node was told to end for a CI job.
pub const DISPLACE_REQUESTED: &str = "displace_requested";
/// The reason on the cancel frame the node gets.
pub const FRAME_REASON: &str = "displaced";
/// Notes kept per job ([`store::JobCtl::displaced`]).
const MAX_NOTES: usize = 8;
/// `status`'s displacement counter window.
pub const COUNTER_SECS: i64 = 24 * 3600;

// ── the rules (pure) ─────────────────────────────────────────────────

/// A soak is never displaced (and never placed on a `release` node):
/// Batch, or an expected run or `--timeout` over 2 h. Unknown: not one.
pub fn is_soak(tier: JobTier, est_secs: Option<i64>, timeout_secs: Option<i64>) -> bool {
    tier == JobTier::Batch || est_secs.is_some_and(|e| e > SOAK_SECS) || timeout_secs.is_some_and(|t| t > SOAK_SECS)
}

pub fn soak_job(j: &JobRow) -> bool {
    is_soak(j.tier(), j.est_secs.filter(|e| *e > 0), j.ctl.timeout_secs)
}

pub fn soak_want(w: &Want) -> bool {
    is_soak(w.tier, w.est_secs.map(|e| e as i64), w.timeout_secs)
}

/// The server-private node label a release needs (`k2 compute node label
/// <node> add release`).
pub const RELEASE_LABEL: &str = "release";

/// A soak is never auto-placed on a node carrying this server's own label
/// `release` (`labels`: the node's, [`k2_core::compute::server_labels`]):
/// the machines a release needs never hold work it can't take back.
pub fn kept_off(w: &Want, labels: &std::collections::BTreeSet<String>) -> bool {
    w.auto && soak_want(w) && labels.contains(RELEASE_LABEL)
}

/// The nodes a displaced auto job still avoids (the [`AVOID_SECS`] after
/// each displacement). A pinned job avoids nothing: it is next there.
pub fn avoiding(ctl: &store::JobCtl, now: i64) -> Vec<String> {
    ctl.displaced.iter().filter(|d| !d.pinned && now - d.at < AVOID_SECS).map(|d| d.node_id.clone()).collect()
}

/// May `ci_id` end this running job? This server's Standard job, not a
/// soak, running (not finishing, not in a node's queue), not already
/// being ended or asked for, and not displaced by this CI job before.
pub fn eligible(v: &JobRow, ci_id: &str) -> bool {
    v.id != ci_id
        && v.tier() == JobTier::Normal
        && !soak_job(v)
        && matches!(v.state.as_str(), "assigned" | "preparing" | "running")
        && !v.node_queued
        && v.ctl.shards.is_empty()
        && !matches!(v.reason.as_deref(), Some("cancel_requested") | Some(DISPLACE_REQUESTED))
        && !share::pending::any(v.reason.as_deref())
        && !v.ctl.displaced.iter().any(|d| d.by == ci_id)
}

/// One job the CI job may end, as the choice reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Victim {
    pub id: String,
    pub ran_secs: i64,
    /// Its `--cache` lives on this node (a cold rebuild loses more).
    pub warm: bool,
    pub cpu_millis: u64,
    pub mem_bytes: u64,
}

/// Least time run first; in the same minute, a warm `--cache` last.
pub fn order(v: &mut [Victim]) {
    v.sort_by(|a, b| (a.ran_secs / 60, a.warm, &a.id).cmp(&(b.ran_secs / 60, b.warm, &b.id)));
}

/// What a node lacks for the CI job to start there now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Need {
    pub slots: u32,
    pub cpu_millis: u64,
    pub mem_bytes: u64,
    /// An exclusive CI job: every running job must go.
    pub idle: bool,
}

impl Need {
    pub fn none(&self) -> bool {
        self.slots == 0 && self.cpu_millis == 0 && self.mem_bytes == 0 && !self.idle
    }
}

/// [`Need`] on a node from its live offer (with what was sent since).
pub fn need_on(offer: &Offer, pend: &Pending, cpu: Option<u64>, mem_on: Option<u64>, exclusive: bool) -> Need {
    let running = offer.running + pend.jobs;
    let mut slots = if offer.parallel_max > 0 && running >= offer.parallel_max { running + 1 - offer.parallel_max } else { 0 };
    // A shared node: this server's own share cap too.
    if let Some(v) = share::view(offer).filter(|v| v.shared && v.limits.max_running > 0) {
        if v.running >= v.limits.max_running {
            slots = slots.max(v.running + 1 - v.limits.max_running);
        }
    }
    let mem_free = offer.free.mem_bytes.saturating_sub(pend.mem_bytes);
    let cpu_free = offer.free.cpu_millis.saturating_sub(pend.cpu_millis);
    Need { slots, cpu_millis: cpu.unwrap_or(0).saturating_sub(cpu_free), mem_bytes: mem_on.unwrap_or(0).saturating_sub(mem_free), idle: exclusive && running > 0 }
}

/// The victims (in [`order`]) that cover `need`, or `None` when the
/// eligible ones can't. `running`: every job running there (any server).
pub fn pick(need: &Need, victims: &[Victim], running: u32) -> Option<Vec<String>> {
    if need.none() || victims.is_empty() {
        return None;
    }
    if need.idle {
        return (victims.len() as u32 >= running).then(|| victims.iter().map(|v| v.id.clone()).collect());
    }
    let (mut s, mut c, mut m) = (0u32, 0u64, 0u64);
    let mut out = Vec::new();
    for v in victims {
        if s >= need.slots && c >= need.cpu_millis && m >= need.mem_bytes {
            break;
        }
        out.push(v.id.clone());
        s += 1;
        c += v.cpu_millis;
        m += v.mem_bytes;
    }
    (s >= need.slots && c >= need.cpu_millis && m >= need.mem_bytes).then_some(out)
}

/// One node that fits a waiting CI job, as the decision reads it.
#[derive(Debug, Clone, Default)]
pub struct Look {
    pub node_id: String,
    pub name: String,
    /// It starts the CI job now (ordinary placement sends it).
    pub starts_now: bool,
    /// When it should free a slot (this server's estimate, a shared
    /// node's quote).
    pub soon: Option<u64>,
    pub need: Need,
    /// Eligible jobs there, in [`order`].
    pub victims: Vec<Victim>,
    pub running: u32,
}

/// What the sweeper does for one waiting CI job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Nothing to displace: it starts now, or waits for a reason a free
    /// slot wouldn't fix.
    Placing,
    /// It waits; the words say why.
    Wait(String),
    /// End these jobs on this node.
    Displace { node_id: String, victims: Vec<String> },
}

/// Go elsewhere first, then the first node (best first) whose eligible
/// jobs cover what it lacks. `looks` are best first.
pub fn decide(looks: &[Look]) -> Step {
    if looks.iter().any(|l| l.starts_now) {
        return Step::Placing;
    }
    let lacking: Vec<&Look> = looks.iter().filter(|l| !l.need.none()).collect();
    if lacking.is_empty() {
        return Step::Placing;
    }
    if let Some(l) = looks.iter().find(|l| l.soon.is_some_and(|s| s <= ELSEWHERE_SECS)) {
        return Step::Wait(format!(
            "waiting: {} should free a slot within {} (nothing displaced)",
            l.name,
            share::human_secs(l.soon.unwrap_or(0))
        ));
    }
    for l in &lacking {
        if let Some(victims) = pick(&l.need, &l.victims, l.running) {
            return Step::Displace { node_id: l.node_id.clone(), victims };
        }
    }
    let names: Vec<&str> = lacking.iter().map(|l| l.name.as_str()).collect();
    Step::Wait(format!("waiting: no displaceable job on {}", names.join(", ")))
}

// ── the sweep ────────────────────────────────────────────────────────

/// The words of each waiting CI job the last sweep looked at.
static WORDS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn words<T>(f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
    let mut g = WORDS.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(Default::default))
}

/// What a waiting CI job's displacement says now (`get`, `wait`).
pub fn ci_words(job_id: &str) -> Option<String> {
    words(|m| m.get(job_id).cloned())
}

/// Who sent a job, for the words: its submitter, else its requester.
fn who_of(j: &JobRow) -> String {
    j.submitter.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| j.requested_by.clone())
}

/// One pass (the compute sweeper, after its moves). Blocking.
pub fn sweep(now: i64) {
    let (queued, active) = k2_core::db::read(|conn| (store::queued_jobs(conn), store::active_jobs(conn)));
    let waiting: Vec<&JobRow> = queued
        .iter()
        .chain(active.iter().filter(|j| j.node_queued))
        .filter(|j| j.tier() == JobTier::Ci && j.ctl.shards.is_empty() && j.ctl.not_before.is_none_or(|nb| nb <= now))
        .filter(|j| {
            let since = if j.ctl.queued_at > 0 { j.ctl.queued_at } else { j.created_at };
            now - since >= SETTLE_SECS
        })
        .collect();
    let mut said = HashMap::new();
    for ci in waiting {
        // Already ending jobs for it, or one just came back: wait for the
        // node to end it and the CI job to take the slot.
        let ending = active.iter().find(|v| v.reason.as_deref() == Some(DISPLACE_REQUESTED) && v.ctl.displaced.last().is_some_and(|d| d.by == ci.id));
        if let Some(v) = ending {
            said.insert(ci.id.clone(), format!("displacing {} on {}: waiting for it to end", message::short(&v.id), node_name(&v.node_id)));
            continue;
        }
        let back = queued.iter().chain(active.iter()).find(|v| v.ctl.displaced.iter().any(|d| d.by == ci.id && d.requeued_at.is_some_and(|t| now - t < START_GRACE_SECS)));
        if let Some(v) = back {
            let node = v.ctl.displaced.iter().rev().find(|d| d.by == ci.id).map(|d| node_name(&d.node_id)).unwrap_or_default();
            said.insert(ci.id.clone(), format!("displaced {} on {node}: starting there", message::short(&v.id)));
            continue;
        }
        let looks = looks_for(ci, &active, now);
        match decide(&looks) {
            Step::Placing => {}
            Step::Wait(w) => {
                said.insert(ci.id.clone(), w);
            }
            Step::Displace { node_id, victims } => {
                let n = displace(ci, &node_id, &victims, now);
                if n > 0 {
                    // An auto CI job queued for another node waits for this
                    // one now: it is first in its line there.
                    if ci.ctl.any.is_some() && !ci.node_queued && ci.node_id != node_id {
                        let others: Vec<String> = looks.iter().filter(|l| l.node_id != node_id).map(|l| l.node_id.clone()).collect();
                        compute_place::try_move(ci, &others, true, "displace");
                    }
                    said.insert(ci.id.clone(), format!("displacing {n} job(s) on {}", node_name(&node_id)));
                }
            }
        }
    }
    words(|m| *m = said);
}

fn node_name(node_id: &str) -> String {
    k2_core::db::read(|conn| store::node_by_id(conn, node_id)).map(|n| n.name).unwrap_or_else(|| node_id.to_string())
}

/// The nodes that fit a waiting CI job, best first: an auto job's
/// candidates in placement order; a pinned job's own node.
fn looks_for(ci: &JobRow, active: &[JobRow], now: i64) -> Vec<Look> {
    let Some(plan) = ci.plan() else { return Vec::new() };
    let w = Want::of_job(ci, &plan);
    if w.auto {
        let report = compute_place::candidates_report(&w, &[], now);
        let pk = plan.src.as_ref().map(|s| s.project_key.clone());
        let commit = plan.src.as_ref().map(|s| s.commit.clone());
        return compute_place::rank(compute::placement_policy(), &report.cands, &w, &[], pk.as_deref(), commit.as_deref())
            .into_iter()
            .filter(|(_, t)| *t != PlaceTier::Offline)
            .map(|(c, t)| {
                let soon = share::view(&c.offer).filter(|v| v.shared).and_then(|v| share::eta_for(v, w.cpu_millis, w.mem_bytes)).or(c.est_wait);
                look_of(ci, &w, &c.node.id, &c.node.name, &c.offer, &c.pending, t == PlaceTier::Now, soon, active, now)
            })
            .collect();
    }
    let (online, _, live) = compute_ws::live_view(&ci.node_id);
    let Some(offer) = live.filter(|_| online) else { return Vec::new() };
    let on_node: Vec<JobRow> = active.iter().filter(|j| j.node_id == ci.node_id && !j.node_queued).cloned().collect();
    let soon = match share::view(&offer).filter(|v| v.shared) {
        Some(v) => share::eta_for(v, w.cpu_millis, w.mem_bytes),
        None => compute_ws::queued_view_of(&[], &on_node, Some(&offer)).1.shadow(),
    };
    let pend = compute_place::pending_for(&ci.node_id, Some(offer.revision), now);
    vec![look_of(ci, &w, &ci.node_id, &node_name(&ci.node_id), &offer, &pend, false, soon, active, now)]
}

#[allow(clippy::too_many_arguments)]
fn look_of(ci: &JobRow, w: &Want, node_id: &str, name: &str, offer: &Offer, pend: &Pending, starts_now: bool, soon: Option<u64>, active: &[JobRow], now: i64) -> Look {
    let per_slot_cpu = if offer.parallel_max > 0 { offer.caps.cpu_millis / u64::from(offer.parallel_max) } else { 0 };
    let mut victims: Vec<Victim> = k2_core::db::read(|conn| {
        active
            .iter()
            .filter(|v| v.node_id == node_id && eligible(v, &ci.id))
            .map(|v| {
                let plan = v.plan();
                let warm = plan.as_ref().and_then(|p| p.cache.clone()).is_some_and(|name| compute_place::cache_node(conn, &v.workspace_id, &name).as_deref() == Some(node_id));
                Victim {
                    id: v.id.clone(),
                    ran_secs: (now - v.started_at.or(v.assigned_at).unwrap_or(now)).max(0),
                    warm,
                    cpu_millis: plan.as_ref().and_then(|p| p.limits.cpu_millis).unwrap_or(per_slot_cpu),
                    mem_bytes: plan.as_ref().and_then(|p| compute_place::plan_mem_on(p, Some(offer))).unwrap_or(0),
                }
            })
            .collect()
    });
    order(&mut victims);
    Look {
        node_id: node_id.to_string(),
        name: name.to_string(),
        starts_now,
        soon,
        need: need_on(offer, pend, w.cpu_millis, w.mem_on(offer), w.exclusive),
        victims,
        running: offer.running,
    }
}

/// Tell `node_id` to end these runs for `ci` (each re-checked under the
/// placement lock). Returns how many were asked.
pub fn displace(ci: &JobRow, node_id: &str, victims: &[String], now: i64) -> usize {
    let by_who = who_of(ci);
    let ci_short = message::short(&ci.id).to_string();
    let node = node_name(node_id);
    compute_ws::with_kick_lock(|| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut n = 0;
        for id in victims {
            let Some(v) = store::job_by_id(&conn, id) else { continue };
            if v.node_id != node_id || !eligible(&v, &ci.id) {
                continue;
            }
            let ran = (now - v.started_at.or(v.assigned_at).unwrap_or(now)).max(0);
            let mut ctl = v.ctl.clone();
            ctl.displaced.push(Displaced {
                by: ci.id.clone(),
                by_who: by_who.clone(),
                node_id: node_id.to_string(),
                at: now,
                ran_secs: ran,
                generation: v.generation,
                pinned: v.ctl.any.is_none(),
                requeued_at: None,
            });
            while ctl.displaced.len() > MAX_NOTES {
                ctl.displaced.remove(0);
            }
            if store::set_job_ctl(&conn, &v.id, &ctl).is_err() {
                continue;
            }
            let detail = format!("displaced by {ci_short} ({by_who}) on {node}: this run ends and the same job is queued again (not a failure, no retry used)");
            store::set_job_reason(&conn, &v.id, Some(DISPLACE_REQUESTED), Some(&detail));
            let sent = send_displace(node_id, &v.id, v.generation);
            store::record_event(
                &conn,
                "controller",
                "job_displaced",
                Some(node_id),
                Some(&v.workspace_id),
                Some(&v.id),
                serde_json::json!({"by": ci.id, "byWho": by_who, "ranSecs": ran, "pinned": v.ctl.any.is_none(), "sent": sent}),
            );
            n += 1;
        }
        n
    })
}

/// The node's stop for a displaced run: an ordinary cancel to the node (it
/// ends `cancelled/displaced`); this server queues the same job again.
pub fn send_displace(node_id: &str, job_id: &str, generation: i64) -> bool {
    use k2_core::compute::proto::frames::{Cancel, Frame};
    compute_ws::send_to(node_id, Frame::Cancel(Cancel { job_id: job_id.into(), generation: generation as u32, reason: FRAME_REASON.into() }))
}

/// A run its node was told to end for a CI job (`asked`, the row's
/// reason) reports `state`: is it queued again ([`requeue`])? A stop, an
/// interruption or a lost record: yes. A run that finished on its own
/// first (done, failed, timed out) keeps that result.
pub fn owes_requeue(asked: Option<&str>, state: JobState) -> bool {
    asked == Some(DISPLACE_REQUESTED) && matches!(state, JobState::Cancelled | JobState::Interrupted | JobState::Unknown)
}

/// Queue a displaced run again: a new generation of the same job, no
/// retry used, its record saying what took the node. An auto job goes
/// elsewhere (it avoids the node for [`AVOID_SECS`]); a pinned one is
/// next on its node. Then the node is kicked, so the CI job takes the slot.
pub fn requeue(job_id: &str) -> bool {
    let now = compute::now();
    let (row, node_id) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(j) = store::job_by_id(&conn, job_id) else { return false };
        if j.reason.as_deref() != Some(DISPLACE_REQUESTED) || j.is_terminal() {
            return false;
        }
        // The record of the ended generation keeps the words (who took
        // the node), under the reason `displaced`.
        let words = j.ctl.displaced.iter().rev().find(|d| d.generation == j.generation).map(|d| {
            let who = if d.by_who.is_empty() { String::new() } else { format!(" ({})", d.by_who) };
            format!("displaced by {}{who} after {}", message::short(&d.by), share::human_secs(d.ran_secs.max(0) as u64))
        });
        store::set_job_reason(&conn, &j.id, Some(next::DISPLACED), words.as_deref());
        if store::requeue_new_generation(&conn, &j.id, j.generation, next::DISPLACED) != Ok(true) {
            return false;
        }
        let Some(mut r) = store::job_by_id(&conn, &j.id) else { return false };
        let mut ctl = r.ctl.clone();
        if let Some(d) = ctl.displaced.iter_mut().rev().find(|d| d.generation == j.generation) {
            d.requeued_at = Some(now);
        }
        // Its queue wait starts again (a `--queue-timeout` counts from here);
        // its place in its line keeps its first arrival.
        ctl.queued_at = now;
        let _ = store::set_job_ctl(&conn, &r.id, &ctl);
        r.ctl = ctl;
        store::record_event(
            &conn,
            "controller",
            "job_requeue",
            Some(&j.node_id),
            Some(&j.workspace_id),
            Some(&j.id),
            serde_json::json!({"why": next::DISPLACED, "newGeneration": j.generation + 1, "by": r.ctl.displaced.last().map(|d| d.by.clone())}),
        );
        (r, j.node_id.clone())
    };
    if row.ctl.any.is_some() && compute_place::try_move(&row, std::slice::from_ref(&node_id), true, next::DISPLACED).is_none() {
        // No other node fits it at all: it waits for this one.
        compute_ws::kick(&row.node_id);
    }
    compute_ws::kick(&node_id);
    compute_ws::notify_change();
    true
}

/// A pinned job queued again after a displacement, not started since:
/// it goes first in its line on that node (the scheduler reads it as
/// queued at the epoch). Its place among CI jobs is unchanged.
pub fn next_on_node(j: &JobRow) -> bool {
    j.state == "queued"
        && j.ctl.any.is_none()
        && j.ctl.displaced.last().is_some_and(|d| d.pinned && d.requeued_at.is_some() && d.generation < j.generation && d.node_id == j.node_id)
}

// ── what it says ─────────────────────────────────────────────────────

/// A job queued again after a displacement, not started since:
/// `displaced by 9c1e2f3a (k2/ci), re-queued as the same job; next on
/// node-x` (pinned) or `…; avoiding node-x for 24m` (auto).
pub fn queued_words(j: &JobRow, now: i64) -> Option<String> {
    if j.state != "queued" && !j.node_queued {
        return None;
    }
    let d = j.ctl.displaced.last().filter(|d| d.requeued_at.is_some() && d.generation < j.generation)?;
    let node = node_name(&d.node_id);
    let who = if d.by_who.is_empty() { String::new() } else { format!(" ({})", d.by_who) };
    let tail = if d.pinned {
        format!("; next on {node}")
    } else if now - d.at < AVOID_SECS {
        format!("; avoiding {node} for {}", share::human_secs((AVOID_SECS - (now - d.at)).max(0) as u64))
    } else {
        String::new()
    };
    Some(format!("displaced by {}{who}, re-queued as the same job{tail}", message::short(&d.by)))
}

/// A run being ended for a CI job: `ending: displaced by 9c1e2f3a (k2/ci)`.
fn ending_words(j: &JobRow) -> Option<String> {
    if j.reason.as_deref() != Some(DISPLACE_REQUESTED) {
        return None;
    }
    let d = j.ctl.displaced.last()?;
    let who = if d.by_who.is_empty() { String::new() } else { format!(" ({})", d.by_who) };
    Some(format!("ending: displaced by {}{who}", message::short(&d.by)))
}

/// Every displacement of `j`, for `get --json`.
pub fn shown(j: &JobRow) -> serde_json::Value {
    j.ctl
        .displaced
        .iter()
        .map(|d| {
            let who = if d.by_who.is_empty() { String::new() } else { format!(" ({})", d.by_who) };
            serde_json::json!({
                "by": message::short(&d.by),
                "byWho": d.by_who,
                "node": node_name(&d.node_id),
                "at": d.at,
                "ranSecs": d.ran_secs,
                "generation": d.generation,
                "pinned": d.pinned,
                "requeuedAt": d.requeued_at,
                "line": format!("displaced by {}{who} on {} after {} (not a failure, no retry used)", message::short(&d.by), node_name(&d.node_id), share::human_secs(d.ran_secs.max(0) as u64)),
            })
        })
        .collect::<Vec<_>>()
        .into()
}

/// The one line `get`, `jobs` and `wait` show for a job now, if any: a
/// displaced job queued again, a run being ended, or a waiting CI job's
/// displacement words.
pub fn line(j: &JobRow, now: i64) -> Option<String> {
    queued_words(j, now).or_else(|| ending_words(j)).or_else(|| (j.tier() == JobTier::Ci && !j.is_terminal()).then(|| ci_words(&j.id)).flatten())
}

/// Put the line into a queue view's `line` (what `wait` prints).
pub fn decorate(j: &JobRow, q: &mut serde_json::Value) {
    let Some(words) = line(j, compute::now()) else { return };
    for key in ["line", "message"] {
        // A shared node's queue line is `message` too (`get` prints that).
        if key == "message" && q.get("onNode").and_then(|v| v.as_bool()) != Some(true) {
            continue;
        }
        if let Some(l) = q.get(key).and_then(|l| l.as_str()).map(str::to_string) {
            q[key] = format!("{words} · {l}").into();
        }
    }
    q["displacement"] = words.into();
}

/// `status`'s counter: the last 24 h's displacements and the minutes they
/// cost (each run's time before it was ended, plus a finished re-run's
/// time over its usual run when that is known).
pub fn counter(now: i64) -> serde_json::Value {
    let since = now - COUNTER_SECS;
    let rows: Vec<(String, i64)> = k2_core::db::read(|conn| {
        let Ok(mut stmt) = conn.prepare("SELECT job_id, detail_json FROM compute_events WHERE kind = 'job_displaced' AND at >= ?1 ORDER BY id DESC LIMIT 2000") else {
            return Vec::new();
        };
        stmt.query_map([since], |r| {
            let id: Option<String> = r.get(0)?;
            let d: String = r.get(1)?;
            Ok((id.unwrap_or_default(), serde_json::from_str::<serde_json::Value>(&d).ok().and_then(|v| v.get("ranSecs").and_then(|x| x.as_i64())).unwrap_or(0)))
        })
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default()
    });
    let mut lost: i64 = rows.iter().map(|(_, r)| (*r).max(0)).sum();
    let mut seen = std::collections::HashSet::new();
    for (id, _) in &rows {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(j) = k2_core::db::read(|conn| store::job_by_id(conn, id)) else { continue };
        lost += rerun_extra(&j);
    }
    let count = rows.len();
    let mins = lost / 60;
    let line = if count == 0 {
        "displaced (24 h): none".to_string()
    } else {
        format!("displaced (24 h): {count} run(s) ended for CI jobs, ~{mins} min lost (time run before, plus re-runs over their usual time)")
    };
    serde_json::json!({"count": count, "lostSecs": lost, "minutesLost": mins, "windowSecs": COUNTER_SECS, "line": line})
}

/// A finished re-run's time over its usual run (0 when unknown).
pub fn rerun_extra(j: &JobRow) -> i64 {
    if j.state != "done" {
        return 0;
    }
    match (j.started_at.zip(j.ended_at), j.est_secs.filter(|e| *e > 0)) {
        (Some((s, e)), Some(est)) => ((e - s) - est).max(0),
        _ => 0,
    }
}

/// `status` rows (a node's running and queued jobs): the line, if any.
pub fn entry(r: &JobRow, now: i64, e: &mut serde_json::Value) {
    if let Some(w) = line(r, now) {
        e["displacement"] = w.into();
    }
}

#[cfg(test)]
#[path = "compute_displace_tests.rs"]
mod tests;
