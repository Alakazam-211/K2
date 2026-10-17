//! The poller's scheduler (PRD §5.3, §4.3; contract §3.1): a heap of due
//! `Instant`s (monotonic, so a wall-clock step never stalls it [GV38]),
//! jitter, soft re-checks and hard back-off [GV56], the per-origin budget
//! and the global in-flight cap [GV59], gap detection (sleep and restart
//! [GV57]), staleness, mute/simulate expiry, retention and the hourly
//! poll rollup [GV68].
//!
//! [`Sched`] holds no threads and never blocks on the network: `tick`
//! returns the fetch jobs to run, `complete` takes their results. The
//! engine (`sources::Engine`) runs it on a thread with a worker pool; tests
//! drive it by hand with a fake fetch.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use k2_core::sources::state::{apply_gap, apply_poll, check_stale, stale_incident, PollInput};
use k2_core::sources::store;
use k2_core::sources::{
    lint, FetchOutcome, FetchPlan, FetchTargets, GapKind, Keep, Phase, Reason, SourceRow, SourceSpec, SourceState,
    SourceStatus, Transition, TransitionKind, UrlRef, GAP_THRESHOLD_SECS, MAX_CONCURRENT_FETCHES,
};
use k2_core::log_debug;
use rusqlite::Connection;

use super::budget::{Budget, Take};
use super::fetch::Fetched;
use crate::wall_jump::Detector;

/// One target fetch waiting for (or holding) a slot.
#[derive(Debug, Clone)]
pub struct Job {
    pub source_id: String,
    pub target_key: Option<String>,
    pub plan: FetchPlan,
    pub origin: String,
    /// When its cycle was due (for "behind by").
    pub due: Instant,
    cycle: u64,
}

struct Entry {
    row: SourceRow,
    spec: SourceSpec,
    due: Instant,
}

struct Cycle {
    id: u64,
    outcomes: Vec<(Option<String>, FetchOutcome)>,
    pending: usize,
}

/// Fetch counts for the hourly rollup [GV68].
#[derive(Debug, Default, Clone)]
struct Rollup {
    fetches: u64,
    ok: u64,
    failures: BTreeMap<String, u64>,
    deferred: u64,
}

/// What the engine does after releasing the scheduler lock.
#[derive(Default)]
pub struct Post {
    /// Revisions to announce with `sources_changed`.
    pub notify: Vec<i64>,
    /// Revisions to announce after 20 s (`simulate slow`).
    pub notify_slow: Vec<i64>,
    /// Hard transitions for the trigger pipeline, with whether the poll
    /// that made them was the first after a gap.
    pub transitions: Vec<(SourceRow, Vec<Transition>, bool)>,
    /// Sources whose mute or simulation just ended.
    pub unmuted: Vec<SourceRow>,
    /// Time to re-deliver pending fires (once, 60 s after boot).
    pub redeliver: bool,
}

pub struct Sched {
    entries: HashMap<String, Entry>,
    heap: BinaryHeap<Reverse<(Instant, String)>>,
    queue: VecDeque<Job>,
    in_flight: usize,
    cycles: HashMap<String, Cycle>,
    next_cycle: u64,
    pub budget: Budget,
    detector: Detector,
    behind_secs: i64,
    pub last_gap: Option<(GapKind, i64, i64)>,
    dirty: bool,
    due_now: HashSet<String>,
    forced: HashSet<String>,
    refreshed: HashMap<String, Instant>,
    last_stale_check: Option<Instant>,
    stale_resent: HashMap<String, Instant>,
    last_prune: Instant,
    rollup: HashMap<String, Rollup>,
    started: Instant,
    redelivered: bool,
    pub draining: bool,
}

/// Uniform in [0, 1).
fn rand01() -> f64 {
    (uuid::Uuid::new_v4().as_u128() as u64 >> 11) as f64 / (1u64 << 53) as f64
}

fn secs(d: f64) -> Duration {
    Duration::from_secs_f64(d.max(0.0))
}

/// First poll (and after a gap): a random point within `min(every, 60 s)`.
pub fn first_delay(every: u32) -> Duration {
    secs(f64::from(every.min(60)) * rand01())
}

/// The next poll after a cycle [GV56]: soft re-checks at `min(every, 60)`,
/// hard fetch failures back off `every × 2^k` (capped at `max(every,
/// 15 min)`), otherwise `every × U(0.9, 1.1)`.
pub fn next_delay(st: &SourceState, spec: &SourceSpec) -> Duration {
    let every = f64::from(spec.every_secs);
    if st.has_soft() {
        return secs(every.min(60.0));
    }
    if st.fetch_failures >= u32::from(spec.fail_after.max(1)) {
        let cap = every.max(900.0);
        let k = st.backoff_k.min(20);
        return secs((every * f64::from(1u32 << k)).min(cap));
    }
    secs(every * (0.9 + 0.2 * rand01()))
}

impl Sched {
    pub fn new(mono: Instant, wall: i64) -> Sched {
        Sched {
            entries: HashMap::new(),
            heap: BinaryHeap::new(),
            queue: VecDeque::new(),
            in_flight: 0,
            cycles: HashMap::new(),
            next_cycle: 0,
            budget: Budget::new(),
            detector: Detector::starting_at(GAP_THRESHOLD_SECS, mono, wall),
            behind_secs: 0,
            last_gap: None,
            dirty: true,
            due_now: HashSet::new(),
            forced: HashSet::new(),
            refreshed: HashMap::new(),
            last_stale_check: None,
            stale_resent: HashMap::new(),
            last_prune: mono,
            rollup: HashMap::new(),
            started: mono,
            redelivered: false,
            draining: false,
        }
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    pub fn behind_secs(&self) -> i64 {
        self.behind_secs
    }

    pub fn active_count(&self) -> usize {
        self.entries.len()
    }

    /// A definition changed: reload on the next tick.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Poll `id` on the next tick (Refresh, approve, edit); a source parked
    /// after a vault denial polls again.
    pub fn poll_now(&mut self, id: &str) {
        self.dirty = true;
        self.due_now.insert(id.to_string());
        self.forced.insert(id.to_string());
    }

    /// Manual refresh: at most once per `REFRESH_MIN_SECS` per source.
    /// `Err(seconds to wait)`.
    pub fn refresh_gate(&mut self, id: &str, mono: Instant) -> Result<(), u32> {
        let min = Duration::from_secs(u64::from(k2_core::sources::REFRESH_MIN_SECS));
        if let Some(t) = self.refreshed.get(id) {
            let since = mono.saturating_duration_since(*t);
            if since < min {
                return Err((min - since).as_secs().max(1) as u32);
            }
        }
        self.refreshed.insert(id.to_string(), mono);
        Ok(())
    }

    fn schedule(&mut self, id: &str, at: Instant) {
        if let Some(e) = self.entries.get_mut(id) {
            e.due = at;
            self.heap.push(Reverse((at, id.to_string())));
        }
    }

    /// The next instant something is due (for the engine's wait).
    pub fn next_due(&self) -> Option<Instant> {
        self.heap.peek().map(|r| r.0 .0)
    }

    fn reload(&mut self, conn: &Connection, mono: Instant) {
        let rows = match store::list_live(conn) {
            Ok(r) => r,
            Err(e) => {
                log_debug!("[sources] reload failed: {e}");
                return;
            }
        };
        let mut seen = HashSet::new();
        for row in rows.into_iter().filter(|r| r.status == SourceStatus::Active) {
            let spec = match store::spec_of(&row) {
                Ok(s) => s,
                Err(e) => {
                    log_debug!("[sources] {} has an unreadable definition: {e}", row.name);
                    continue;
                }
            };
            seen.insert(row.id.clone());
            match self.entries.get_mut(&row.id) {
                Some(e) => {
                    e.row = row;
                    e.spec = spec;
                }
                None => {
                    let id = row.id.clone();
                    let at = mono + first_delay(spec.every_secs);
                    self.entries.insert(id.clone(), Entry { row, spec, due: at });
                    self.heap.push(Reverse((at, id)));
                }
            }
        }
        self.entries.retain(|id, _| seen.contains(id));
        self.cycles.retain(|id, _| seen.contains(id));
        self.queue.retain(|j| seen.contains(&j.source_id));
    }

    /// Boot after a restart: every source that ran before is in a
    /// `daemon_restarted` gap until its first poll [GV57].
    pub fn boot(&mut self, conn: &Connection, mono: Instant, wall: i64, out: &mut Post) {
        self.reload(conn, mono);
        self.dirty = false;
        let ids: Vec<String> = self.entries.keys().cloned().collect();
        let mut earliest: Option<i64> = None;
        for id in ids {
            let Ok(Some((st, _))) = store::load_state(conn, &id) else { continue };
            let Some(from) = st.checked_at else { continue };
            earliest = Some(earliest.map_or(from, |e: i64| e.min(from)));
            let g = apply_gap(&st, GapKind::DaemonRestarted, from, wall);
            if let Some(rev) = self.save_gap(conn, &id, &g, from, wall) {
                out.notify.push(rev);
            }
        }
        if let Some(from) = earliest {
            self.last_gap = Some((GapKind::DaemonRestarted, from, wall));
        }
    }

    fn save_gap(&self, conn: &Connection, id: &str, st: &SourceState, from: i64, to: i64) -> Option<i64> {
        let e = self.entries.get(id)?;
        let trend = e.spec.extract.trend.clone();
        let keep_values = e.spec.keep == Keep::Values;
        store::tx(conn, |c| {
            let rev = store::save_state(c, st, None)?;
            let t = TransitionKind::Gap;
            store::insert_events(
                c,
                id,
                to,
                &[Transition {
                    scope_item: None,
                    from: st.health,
                    to: st.health,
                    kind: t,
                    incident_id: None,
                    reason: Some(match st.gap_kind {
                        Some(GapKind::HostSlept) => Reason::HostSlept,
                        _ => Reason::DaemonRestarted,
                    }),
                    hard: false,
                }],
                false,
            )?;
            if keep_values && to - from > 1 {
                let marks: Vec<(String, Option<f64>)> = trend.iter().map(|s| (s.clone(), None)).collect();
                store::insert_points(c, id, from + 1, &marks)?;
            }
            Ok(rev)
        })
        .map_err(|e| log_debug!("[sources] gap save failed: {e}"))
        .ok()
    }

    /// A sleep was detected: gap every source, poll them all soon.
    fn gap_all(&mut self, conn: &Connection, kind: GapKind, from: i64, to: i64, mono: Instant, out: &mut Post) {
        self.last_gap = Some((kind, from, to));
        let ids: Vec<String> = self.entries.keys().cloned().collect();
        for id in ids {
            if let Ok(Some((st, _))) = store::load_state(conn, &id) {
                if st.checked_at.is_some() {
                    let g = apply_gap(&st, kind, from, to);
                    if let Some(rev) = self.save_gap(conn, &id, &g, from, to) {
                        out.notify.push(rev);
                    }
                }
            }
            let every = self.entries.get(&id).map_or(60, |e| e.spec.every_secs);
            self.schedule(&id, mono + first_delay(every));
        }
    }

    /// One scheduler pass. Returns the fetches to run now.
    pub fn tick(&mut self, conn: &Connection, mono: Instant, wall: i64, out: &mut Post) -> Vec<Job> {
        if let Some(j) = self.detector.observe_at(mono, wall) {
            log_debug!("[sources] host slept {}s; gapping sources", j.wall_delta - j.mono_delta);
            if self.dirty {
                self.reload(conn, mono);
                self.dirty = false;
            }
            self.gap_all(conn, GapKind::HostSlept, j.from, j.to, mono, out);
        }
        if self.dirty {
            self.reload(conn, mono);
            self.dirty = false;
        }
        for id in std::mem::take(&mut self.due_now) {
            self.schedule(&id, mono);
        }
        self.expire_overlays(conn, wall, out);
        if self.last_stale_check.is_none_or(|t| mono.saturating_duration_since(t) >= Duration::from_secs(15)) {
            self.last_stale_check = Some(mono);
            self.stale_checks(conn, mono, wall, out);
        }
        if mono.saturating_duration_since(self.last_prune) >= Duration::from_secs(3600) {
            self.last_prune = mono;
            self.hourly(conn, wall);
        }
        if !self.redelivered && mono.saturating_duration_since(self.started) >= Duration::from_secs(60) {
            self.redelivered = true;
            out.redeliver = true;
        }
        while let Some(Reverse((due, id))) = self.heap.peek().cloned() {
            if due > mono {
                break;
            }
            self.heap.pop();
            let current = self.entries.get(&id).map(|e| e.due);
            if current != Some(due) || self.cycles.contains_key(&id) {
                continue;
            }
            self.start_cycle(conn, &id, due, mono, wall, out);
        }
        self.dispatch(conn, mono, wall, out)
    }

    fn expire_overlays(&mut self, conn: &Connection, wall: i64, out: &mut Post) {
        let mut changed = false;
        for e in self.entries.values() {
            let mute_over = e.row.mute_until.is_some_and(|u| u <= wall);
            let sim_over = e.row.simulate_until.is_some_and(|u| u <= wall);
            if !mute_over && !sim_over {
                continue;
            }
            let r = store::tx(conn, |c| {
                let mut rev = 0;
                if mute_over {
                    rev = store::set_mute(c, &e.row.id, None, None, None, wall)?;
                    store::audit(c, wall, "daemon", "daemon", "unmute", Some(&e.row.id), None, &serde_json::json!({"expired": true}))?;
                }
                if sim_over {
                    rev = store::set_simulate(c, &e.row.id, None, None, None, wall)?;
                    store::audit(c, wall, "daemon", "daemon", "simulate_off", Some(&e.row.id), None, &serde_json::json!({"expired": true}))?;
                }
                Ok(rev)
            });
            match r {
                Ok(rev) => {
                    out.notify.push(rev);
                    out.unmuted.push(e.row.clone());
                    changed = true;
                }
                Err(err) => log_debug!("[sources] expiry write failed: {err}"),
            }
        }
        if changed {
            self.reload(conn, Instant::now());
        }
    }

    fn stale_checks(&mut self, conn: &Connection, mono: Instant, wall: i64, out: &mut Post) {
        let ids: Vec<String> = self.entries.keys().filter(|id| !self.cycles.contains_key(*id)).cloned().collect();
        for id in ids {
            let Some(e) = self.entries.get(&id) else { continue };
            let Ok(Some((st, _))) = store::load_state(conn, &id) else { continue };
            if let Some((s, t)) = check_stale(&st, &e.spec, wall) {
                let saved = store::tx(conn, |c| {
                    let rev = store::save_state(c, &s, None)?;
                    store::insert_events(c, &id, wall, std::slice::from_ref(&t), false)?;
                    Ok(rev)
                });
                if let Ok(rev) = saved {
                    out.notify.push(rev);
                    out.transitions.push((e.row.clone(), vec![t], false));
                    self.stale_resent.insert(id.clone(), mono);
                }
            } else if st.phase == Phase::Stale
                && self.stale_resent.get(&id).is_none_or(|t| mono.saturating_duration_since(*t) >= Duration::from_secs(60))
            {
                // `stale_for` triggers judge the stale episode's age; they
                // see it again every minute (deduped by its id).
                self.stale_resent.insert(id.clone(), mono);
                let t = Transition {
                    scope_item: None,
                    from: st.health,
                    to: st.health,
                    kind: TransitionKind::WentStale,
                    incident_id: Some(stale_incident(&id, st.updated_at)),
                    reason: st.reason,
                    hard: true,
                };
                out.transitions.push((e.row.clone(), vec![t], false));
            }
        }
    }

    fn hourly(&mut self, conn: &Connection, wall: i64) {
        for (id, r) in std::mem::take(&mut self.rollup) {
            if r.fetches == 0 && r.deferred == 0 {
                continue;
            }
            let detail = serde_json::json!({"fetches": r.fetches, "ok": r.ok, "failures": r.failures, "deferred": r.deferred});
            let _ = store::audit(conn, wall, "daemon", "daemon", "poll_rollup", Some(&id), None, &detail);
        }
        if let Err(e) = store::prune(conn, wall) {
            log_debug!("[sources] retention failed: {e}");
        }
    }

    fn plans(e: &Entry, st: &SourceState) -> Vec<(Option<String>, FetchPlan, String)> {
        let spec = &e.spec;
        let conditional = |etag: &Option<String>, lm: &Option<String>| {
            let mut h = spec.headers.clone();
            if let Some(t) = etag {
                h.push(("If-None-Match".into(), t.clone()));
            }
            if let Some(m) = lm {
                h.push(("If-Modified-Since".into(), m.clone()));
            }
            h
        };
        let base = |url: UrlRef, origin: String, headers: Vec<(String, String)>, key: Option<String>| FetchPlan {
            source_id: Some(e.row.id.clone()),
            target_key: key,
            url,
            origin,
            lan_exact_ip: e.row.lan,
            headers,
            secret: spec.secret.clone(),
            secret_workspace: e.row.secret_workspace.clone(),
            timeout_secs: spec.timeout_secs,
            max_bytes: spec.max_bytes,
        };
        match &spec.fetch {
            FetchTargets::Url(u) => {
                let origin = e.row.origins.first().cloned().unwrap_or_default();
                vec![(None, base(UrlRef::Plain(u.clone()), origin.clone(), conditional(&st.etag, &st.last_modified), None), origin)]
            }
            FetchTargets::UrlSecret { name, origin } => {
                let o = lint::origin_of(origin, "", true).map(|p| p.origin).unwrap_or_else(|_| origin.clone());
                vec![(None, base(UrlRef::Secret { name: name.clone() }, o.clone(), conditional(&st.etag, &st.last_modified), None), o)]
            }
            FetchTargets::Targets(ts) => ts
                .iter()
                .map(|t| {
                    let o = lint::origin_of(&t.url, "", true).map(|p| p.origin).unwrap_or_default();
                    let (etag, lm) = st.items.get(&t.key).map(|i| (i.etag.clone(), i.last_modified.clone())).unwrap_or((None, None));
                    let p = base(UrlRef::Plain(t.url.clone()), o.clone(), conditional(&etag, &lm), Some(t.key.clone()));
                    (Some(t.key.clone()), p, o)
                })
                .collect(),
        }
    }

    fn start_cycle(&mut self, conn: &Connection, id: &str, due: Instant, mono: Instant, _wall: i64, out: &mut Post) {
        let Some(e) = self.entries.get(id) else { return };
        let every = Duration::from_secs(u64::from(e.spec.every_secs));
        if self.draining {
            self.schedule(id, mono + every);
            return;
        }
        let Ok(Some((mut st, _))) = store::load_state(conn, id) else {
            self.schedule(id, mono + every);
            return;
        };
        // Air-gap: only Owner-typed exact-IP sources run [GV18].
        if k2_core::airgap::enabled() && !e.row.lan {
            if st.phase != Phase::Paused || st.reason != Some(Reason::Airgap) {
                st.phase = Phase::Paused;
                st.reason = Some(Reason::Airgap);
                if let Ok(rev) = store::save_state(conn, &st, None) {
                    out.notify.push(rev);
                }
            }
            self.schedule(id, mono + every);
            return;
        }
        let forced = self.forced.remove(id);
        if st.vault_denied && !forced {
            self.schedule(id, mono + every);
            return;
        }
        let plans = Self::plans(e, &st);
        if plans.is_empty() {
            self.schedule(id, mono + every);
            return;
        }
        self.next_cycle += 1;
        let cycle = self.next_cycle;
        self.cycles.insert(id.to_string(), Cycle { id: cycle, outcomes: Vec::new(), pending: plans.len() });
        for (key, plan, origin) in plans {
            self.queue.push_back(Job { source_id: id.to_string(), target_key: key, plan, origin, due, cycle });
        }
    }

    fn dispatch(&mut self, conn: &Connection, mono: Instant, wall: i64, out: &mut Post) -> Vec<Job> {
        let mut run = Vec::new();
        let mut keep = VecDeque::new();
        let mut skipped: Vec<Job> = Vec::new();
        while let Some(job) = self.queue.pop_front() {
            if self.draining || self.in_flight >= MAX_CONCURRENT_FETCHES {
                keep.push_back(job);
                continue;
            }
            match self.budget.try_take(&job.origin, mono) {
                Take::Go => {
                    self.in_flight += 1;
                    self.rollup.entry(job.source_id.clone()).or_default().fetches += 1;
                    run.push(job);
                }
                Take::Wait(_) => keep.push_back(job),
                Take::Parked(_) => skipped.push(job),
            }
        }
        self.queue = keep;
        self.behind_secs =
            self.queue.iter().map(|j| mono.saturating_duration_since(j.due).as_secs() as i64).max().unwrap_or(0);
        for job in skipped {
            self.rollup.entry(job.source_id.clone()).or_default().deferred += 1;
            self.settle(conn, &job, None, mono, wall, out);
        }
        run
    }

    /// A finished fetch.
    pub fn complete(&mut self, conn: &Connection, job: Job, fetched: Fetched, mono: Instant, wall: i64, out: &mut Post) {
        self.in_flight = self.in_flight.saturating_sub(1);
        if let Some(s) = fetched.park_secs {
            self.budget.park(&job.origin, s, Reason::RateLimited, mono, wall);
        }
        let r = self.rollup.entry(job.source_id.clone()).or_default();
        let new_reason = match &fetched.outcome {
            FetchOutcome::Ok { .. } => {
                r.ok += 1;
                None
            }
            FetchOutcome::Err { reason, .. } => {
                let n = r.failures.entry(reason.code().to_string()).or_default();
                *n += 1;
                (*n == 1).then_some(*reason)
            }
        };
        if let Some(reason) = new_reason {
            let _ = store::audit(
                conn,
                wall,
                "daemon",
                "daemon",
                "new_failure_reason",
                Some(&job.source_id),
                None,
                &serde_json::json!({"reason": reason.code(), "target": job.target_key}),
            );
        }
        self.settle(conn, &job, Some(fetched.outcome), mono, wall, out);
    }

    /// Record one target's result (or a skip) in its cycle; finish the
    /// cycle when every target is in.
    fn settle(&mut self, conn: &Connection, job: &Job, outcome: Option<FetchOutcome>, mono: Instant, wall: i64, out: &mut Post) {
        let Some(c) = self.cycles.get_mut(&job.source_id) else { return };
        if c.id != job.cycle {
            return;
        }
        if let Some(o) = outcome {
            c.outcomes.push((job.target_key.clone(), o));
        }
        c.pending = c.pending.saturating_sub(1);
        if c.pending == 0 {
            let c = self.cycles.remove(&job.source_id).expect("present");
            self.finish(conn, &job.source_id, c.outcomes, mono, wall, out);
        }
    }

    fn finish(&mut self, conn: &Connection, id: &str, outcomes: Vec<(Option<String>, FetchOutcome)>, mono: Instant, wall: i64, out: &mut Post) {
        let Some(e) = self.entries.get(id) else { return };
        let Ok(Some((prev, prev_proj))) = store::load_state(conn, id) else { return };
        if outcomes.is_empty() {
            // Every target was parked: show why, poll when the park lifts.
            let until = e.row.origins.iter().filter_map(|o| self.budget.parked(o, mono)).map(|p| p.until).max();
            let mut st = prev.clone();
            if st.reason != Some(Reason::RateLimited) && st.health == k2_core::sources::Health::Ok {
                st.reason = Some(Reason::RateLimited);
                if let Ok(rev) = store::save_state(conn, &st, None) {
                    out.notify.push(rev);
                }
            }
            let at = until.unwrap_or(mono + Duration::from_secs(u64::from(e.spec.every_secs)));
            self.schedule(id, at);
            return;
        }
        // Every target stopped at the vault: not a check of the site. The
        // source pauses with the reason; a denial parks it until the secret,
        // the grant or the source changes, or a Refresh [GV68].
        let vault_reason = |o: &FetchOutcome| match o {
            FetchOutcome::Err { reason, .. }
                if matches!(reason, Reason::SecretMissing | Reason::SecretDisabled | Reason::VaultUnavailable) =>
            {
                Some(*reason)
            }
            _ => None,
        };
        if let Some(reason) = outcomes.iter().map(|(_, o)| vault_reason(o)).collect::<Option<Vec<_>>>().and_then(|v| v.first().copied()) {
            let mut st = prev.clone();
            st.phase = Phase::Paused;
            st.reason = Some(reason);
            st.checked_at = Some(wall);
            let park = reason != Reason::VaultUnavailable;
            let newly = park && !st.vault_denied;
            st.vault_denied = park;
            let every = Duration::from_secs(u64::from(e.spec.every_secs));
            let saved = store::tx(conn, |c| {
                let rev = store::save_state(c, &st, None)?;
                if newly {
                    store::audit(c, wall, "daemon", "daemon", "vault_denied_parked", Some(id), None, &serde_json::json!({"reason": reason.code()}))?;
                }
                Ok(rev)
            });
            if let Ok(rev) = saved {
                out.notify.push(rev);
            }
            self.schedule(id, mono + every);
            return;
        }
        let applied = apply_poll(
            &prev,
            PollInput { spec: &e.spec, outcomes, now: wall, simulated: false, prev_projection: prev_proj.as_ref() },
        );
        let mut st = applied.state;
        let delay = next_delay(&st, &e.spec);
        st.next_at = Some(wall + delay.as_secs() as i64);
        let simulated = e.row.simulate_state.is_some() && e.row.simulate_until.is_some_and(|u| u > wall);
        let write_points = !simulated && e.spec.keep == Keep::Values && !applied.points.is_empty();
        let points: Vec<(String, Option<f64>)> = applied.points.iter().map(|(s, v)| (s.clone(), Some(*v))).collect();
        let saved = store::tx(conn, |c| {
            let rev = store::save_state(c, &st, applied.projection.as_ref())?;
            store::insert_events(c, id, wall, &applied.transitions, simulated)?;
            if write_points {
                store::insert_points(c, id, wall, &points)?;
            }
            Ok(rev)
        });
        let row = e.row.clone();
        self.schedule(id, mono + delay);
        match saved {
            Ok(rev) => {
                if row.simulate_state == Some(k2_core::sources::SimulateState::Slow) && simulated {
                    out.notify_slow.push(rev);
                } else {
                    out.notify.push(rev);
                }
            }
            Err(err) => {
                log_debug!("[sources] state write failed for {}: {err}", row.name);
                return;
            }
        }
        let hard: Vec<Transition> = applied.transitions.into_iter().filter(|t| t.hard).collect();
        if !hard.is_empty() {
            out.transitions.push((row, hard, prev.in_gap()));
        }
    }
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
