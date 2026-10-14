//! Trigger → wake (contract §9). Owner: B2.
//!
//! The engine calls [`handle_transitions`] after every state commit,
//! [`handle_unmute`] when a mute or simulate ends, and [`start`] once at
//! daemon start (workers, the 30 s tick for `stale_for` and window expiry,
//! and the boot + 60 s re-delivery of `pending` fires).
//!
//! Pipeline per hard transition: evaluate each active trigger on the
//! source; log a `suppressed` row, or write a `pending` fire (the
//! once-per-incident index dedupes) and queue it for its agent. Delivery
//! runs on two `sources-wake-N` threads with **one delivery per agent at a
//! time** [GV64]: fires that arrive for an agent while one is in flight
//! wait, and go out together as one coalesced note.
//!
//! Delivery reuses the `k2 msg` wake path, `workspace_msg::
//! deliver_live_with_via(project_id, note, "source:<name>", "", true,
//! DEFAULT_WAKE_TIMEOUT, "source")`, with the agent's inbox as the
//! fallback (the compute pattern, `compute_admin.rs`) [GV29, GV37].

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use k2_core::sources::note::{self, NoteEvent, NoteInput};
use k2_core::sources::state::{Phase, TransitionKind};
use k2_core::sources::trigger_store::{self as ts, Delivery, NewFire};
use k2_core::sources::triggers::{self, FireContext, FireDecision, TriggerOn, TriggerStatus};
use k2_core::sources::{store, SourceRow, SourceState, Transition, WakeOutcome, WakeRequest, Waker};
use rusqlite::Connection;

// ── the waker ────────────────────────────────────────────────────────

/// The daemon's [`Waker`]: `workspace_msg::deliver_live_with_via` with
/// `via = "source"`, falling back to the agent's inbox.
pub struct DaemonWaker;

impl Waker for DaemonWaker {
    fn deliver(&self, req: &WakeRequest) -> WakeOutcome {
        #[cfg(test)]
        if let Some(seams) = test_seams::current() {
            return deliver_with(req, &*seams.live, &*seams.ws_path, &*seams.inbox);
        }
        deliver_with(
            req,
            &|ws, text, from| {
                let r = crate::workspace_msg::deliver_live_with_via(
                    ws,
                    text,
                    from,
                    "",
                    true,
                    crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
                    "source",
                );
                LiveResult { success: r.success, session_id: r.target_session_id, woke: r.woke, reason: r.reason }
            },
            &|id| crate::compute_routes::ws_by_id(id).map(|w| w.path),
            &|path, title, body, from| {
                k2_core::inbox::compose(std::path::Path::new(path), title, body, None, Some("source"), Some(from))
                    .map(|_| ())
            },
        )
    }
}

/// What the live path answered (the parts the pipeline keeps).
pub struct LiveResult {
    pub success: bool,
    pub session_id: Option<String>,
    pub woke: bool,
    /// `MsgReason` code.
    pub reason: Option<String>,
}

type LiveFn<'a> = dyn Fn(&str, &str, &str) -> LiveResult + 'a;
type PathFn<'a> = dyn Fn(&str) -> Option<String> + 'a;
type InboxFn<'a> = dyn Fn(&str, &str, &str, &str) -> Result<(), String> + 'a;

/// The delivery decision (contract §9 `DaemonWaker::deliver`): the project
/// must exist; live delivery by project id; `no_agent_mode` /
/// `workspace_not_found` are undeliverable; anything else falls back to
/// the inbox.
pub fn deliver_with(req: &WakeRequest, live: &LiveFn, ws_path: &PathFn, inbox: &InboxFn) -> WakeOutcome {
    let Some(path) = ws_path(&req.project_id) else {
        return WakeOutcome::Undeliverable { code: "workspace_not_found".into() };
    };
    let r = live(&req.project_id, &req.note, &req.from);
    if r.success {
        return WakeOutcome::Delivered { session_id: r.session_id, woke: r.woke };
    }
    if let Some(code) = r.reason.as_deref().filter(|c| *c == "no_agent_mode" || *c == "workspace_not_found") {
        return WakeOutcome::Undeliverable { code: code.to_string() };
    }
    match inbox(&path, &req.title, &req.note, &req.from) {
        Ok(()) => WakeOutcome::Inbox,
        Err(_) => WakeOutcome::Undeliverable { code: "inbox_failed".into() },
    }
}

// ── the queue ────────────────────────────────────────────────────────

/// One fire on its way to an agent.
#[derive(Debug, Clone)]
pub struct Job {
    pub fire_id: String,
    pub trigger_id: String,
    pub source_id: String,
    pub transition: Transition,
    pub at: i64,
}

#[derive(Default)]
struct Q {
    /// Projects with a batch waiting and nobody delivering to them.
    ready: VecDeque<String>,
    /// Waiting fires per project (the next note).
    waiting: HashMap<String, Vec<Job>>,
    /// Projects with a delivery in flight.
    in_flight: HashSet<String>,
}

/// The per-agent queue: one delivery in flight per agent; later fires for
/// it coalesce into the next note.
pub struct Pipe {
    q: Mutex<Q>,
    cv: Condvar,
    waker: Box<dyn Waker>,
}

/// A batch taken for delivery: the lead fire carries the note; the rest are
/// logged `coalesced` into it.
pub struct Batch {
    pub project_id: String,
    pub jobs: Vec<Job>,
}

impl Pipe {
    pub fn new(waker: Box<dyn Waker>) -> Pipe {
        Pipe { q: Mutex::new(Q::default()), cv: Condvar::new(), waker }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Q> {
        self.q.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Queue a fire for its agent.
    pub fn enqueue(&self, project_id: &str, job: Job) {
        let mut q = self.lock();
        let w = q.waiting.entry(project_id.to_string()).or_default();
        w.push(job);
        if w.len() == 1 && !q.in_flight.contains(project_id) {
            q.ready.push_back(project_id.to_string());
            self.cv.notify_one();
        }
    }

    /// Take the next batch without waiting (`None` when nothing is ready).
    pub fn try_take(&self) -> Option<Batch> {
        let mut q = self.lock();
        Self::take_locked(&mut q)
    }

    fn take_locked(q: &mut Q) -> Option<Batch> {
        while let Some(p) = q.ready.pop_front() {
            if q.in_flight.contains(&p) {
                continue;
            }
            let jobs = q.waiting.remove(&p).unwrap_or_default();
            if jobs.is_empty() {
                continue;
            }
            q.in_flight.insert(p.clone());
            return Some(Batch { project_id: p, jobs });
        }
        None
    }

    /// Block until a batch is ready.
    fn take_wait(&self) -> Batch {
        let mut q = self.lock();
        loop {
            if let Some(b) = Self::take_locked(&mut q) {
                return b;
            }
            q = self.cv.wait(q).unwrap_or_else(|p| p.into_inner());
        }
    }

    /// The delivery to `project_id` ended; release it and queue whatever
    /// waited.
    pub fn finish(&self, project_id: &str) {
        let mut q = self.lock();
        q.in_flight.remove(project_id);
        if q.waiting.get(project_id).is_some_and(|w| !w.is_empty()) {
            q.ready.push_back(project_id.to_string());
        }
        self.cv.notify_all();
    }

    /// Hold `project_id` for a delivery outside the queue (a test wake),
    /// waiting up to `timeout` for the one in flight to end.
    pub fn acquire(&self, project_id: &str, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let mut q = self.lock();
        while q.in_flight.contains(project_id) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            q = self.cv.wait_timeout(q, left).unwrap_or_else(|p| p.into_inner()).0;
        }
        q.in_flight.insert(project_id.to_string());
        true
    }

    /// Deliver one batch: one note for every fire in it, oldest first.
    pub fn deliver_batch(&self, batch: &Batch, now: i64) {
        deliver_batch(&*self.waker, batch, now);
        self.finish(&batch.project_id);
    }

    pub fn waker(&self) -> &dyn Waker {
        &*self.waker
    }
}

static PIPE: OnceLock<Arc<Pipe>> = OnceLock::new();

fn pipe() -> Arc<Pipe> {
    #[cfg(test)]
    if let Some(p) = test_seams::pipe() {
        return p;
    }
    PIPE.get_or_init(|| Arc::new(Pipe::new(Box::new(DaemonWaker)))).clone()
}

static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Start the two wake workers, the 30 s trigger tick and the boot + 60 s
/// re-delivery. Idempotent. The engine's `start()` calls it.
pub fn start() {
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let p = pipe();
    for n in 0..2 {
        let p = p.clone();
        let _ = std::thread::Builder::new().name(format!("sources-wake-{n}")).spawn(move || loop {
            let b = p.take_wait();
            p.deliver_batch(&b, now());
        });
    }
    let _ = std::thread::Builder::new().name("sources-trigger-tick".into()).spawn(|| {
        let boot = std::time::Instant::now();
        let mut redelivered = false;
        let mut last = now();
        loop {
            std::thread::sleep(Duration::from_secs(30));
            let t = now();
            if !redelivered && boot.elapsed() >= Duration::from_secs(60) {
                redeliver_pending(t);
                redelivered = true;
            }
            tick(last, t);
            last = t;
        }
    });
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The server this check runs on, for `Runs on <host>`.
pub fn this_host() -> String {
    let primary = k2_core::tunnel::subdomains::current().primary.clone();
    let p = primary.trim();
    if p.is_empty() {
        "this server".into()
    } else if p.contains('.') {
        p.to_string()
    } else {
        format!("{p}.k2.dev")
    }
}

fn changed(conn: &Connection) {
    if let Ok(rev) = store::bump_rev(conn) {
        super::notify_changed(rev);
    }
}

// ── evaluation ───────────────────────────────────────────────────────

/// Which re-judging path this is (the tick and unmute paths re-judge the
/// same open incident many times and log only its first outcome).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Path {
    Transition,
    Rejudge,
}

/// Judge `ts` against every active trigger on `source`, log, queue.
/// Returns how many fires were queued.
fn judge(conn: &Connection, source: &SourceRow, ts_in: &[Transition], now: i64, path: Path, only: Option<&str>) -> usize {
    let hard: Vec<&Transition> = ts_in.iter().filter(|t| t.hard).collect();
    if hard.is_empty() {
        return 0;
    }
    let Ok(triggers) = ts::list(conn, Some(&source.id)) else {
        return 0;
    };
    let state = ts::load_state(conn, &source.id).ok().flatten().map(|(s, _)| s).unwrap_or_else(|| SourceState::new(&source.id));
    let in_gap = ts_in.iter().any(|t| t.kind == TransitionKind::Gap)
        || (state.phase == Phase::Paused && state.gap_kind.is_some());
    let draining = super::DRAINING.load(std::sync::atomic::Ordering::SeqCst);
    let mut queued = 0;
    let mut wrote = false;
    for trow in triggers.iter().filter(|t| t.status == TriggerStatus::Active) {
        if only.is_some_and(|id| id != trow.id) {
            continue;
        }
        let spec = trow.spec();
        for tr in &hard {
            let base = FireContext {
                source,
                state: &state,
                now,
                last_fire_at: trow.last_fire_at,
                fires_today: ts::fires_since(conn, &trow.id, triggers::utc_day_start(now)).unwrap_or(0),
                agent_fires_last_hour: ts::agent_fires_since(conn, &trow.target_project_id, now - 3600).unwrap_or(0),
                already_fired_incident: false,
                in_gap,
                draining,
            };
            let (incident, item) = match triggers::evaluate(&spec, tr, &base) {
                FireDecision::Ignore => continue,
                FireDecision::Fire { incident_id, item_key } | FireDecision::Suppress { incident_id, item_key, .. } => {
                    (incident_id, item_key)
                }
            };
            let already = ts::incident_fired(conn, &trow.id, &incident, item.as_deref()).unwrap_or(false);
            let cx = FireContext { already_fired_incident: already, ..base };
            match triggers::evaluate(&spec, tr, &cx) {
                FireDecision::Ignore => {}
                FireDecision::Suppress { reason, .. }
                    if path == Path::Rejudge
                        && (reason == triggers::SuppressReason::Deduped
                            || ts::incident_logged(conn, &trow.id, &incident, item.as_deref()).unwrap_or(true)) => {}
                FireDecision::Suppress { reason, incident_id, item_key } => {
                    log_suppressed(conn, trow, &incident_id, item_key.as_deref(), reason.code(), now);
                    wrote = true;
                }
                FireDecision::Fire { incident_id, item_key } => {
                    let fire = ts::insert_fire(
                        conn,
                        &NewFire {
                            trigger_id: &trow.id,
                            source_id: &source.id,
                            target_project_id: &trow.target_project_id,
                            incident_id: &incident_id,
                            item_key: item_key.as_deref(),
                            at: now,
                            outcome: "pending",
                            suppress_reason: None,
                        },
                    );
                    match fire {
                        Ok(Some(fire_id)) => {
                            let _ = ts::set_last_fire(conn, &trow.id, now);
                            let _ = ts::audit(
                                conn,
                                now,
                                "daemon",
                                "daemon",
                                "fire",
                                Some(&source.id),
                                Some(&trow.id),
                                &serde_json::json!({"incident": incident_id, "item": item_key, "on": trow.on.code()}),
                            );
                            pipe().enqueue(
                                &trow.target_project_id,
                                Job {
                                    fire_id,
                                    trigger_id: trow.id.clone(),
                                    source_id: source.id.clone(),
                                    transition: (*tr).clone(),
                                    at: now,
                                },
                            );
                            queued += 1;
                        }
                        Ok(None) => log_suppressed(conn, trow, &incident_id, item_key.as_deref(), "deduped", now),
                        Err(e) => k2_core::log_debug!("[sources] fire row for trigger {}: {e}", trow.id),
                    }
                    wrote = true;
                }
            }
        }
    }
    if wrote {
        changed(conn);
    }
    queued
}

fn log_suppressed(conn: &Connection, t: &ts::TriggerRow, incident: &str, item: Option<&str>, reason: &str, now: i64) {
    let r = ts::insert_fire(
        conn,
        &NewFire {
            trigger_id: &t.id,
            source_id: &t.source_id,
            target_project_id: &t.target_project_id,
            incident_id: incident,
            item_key: item,
            at: now,
            outcome: "suppressed",
            suppress_reason: Some(reason),
        },
    );
    if let Err(e) = r {
        k2_core::log_debug!("[sources] suppression row for trigger {}: {e}", t.id);
        return;
    }
    let _ = ts::audit(
        conn,
        now,
        "daemon",
        "daemon",
        "suppress",
        Some(&t.source_id),
        Some(&t.id),
        &serde_json::json!({"reason": reason, "incident": incident, "item": item}),
    );
}

/// Evaluate every active trigger on `source` against `ts`, write fire rows,
/// queue deliveries. The engine calls it after the state commit with the
/// cycle's transitions (soft ones are ignored).
pub fn handle_transitions(source: &SourceRow, ts_in: &[Transition], now: i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    judge(&conn, source, ts_in, now, Path::Transition, None);
}

/// A mute or simulate on `source` ended: an incident that is open now and
/// never fired gets one fire (cooldown and caps still apply). Pass the row
/// as it is after the change.
pub fn handle_unmute(source: &SourceRow, now: i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    rejudge_open(&conn, source, now, None);
}

fn rejudge_open(conn: &Connection, source: &SourceRow, now: i64, only: Option<&str>) -> usize {
    let Some((state, _)) = ts::load_state(conn, &source.id).ok().flatten() else {
        return 0;
    };
    let open = triggers::open_incidents(&state);
    judge(conn, source, &open, now, Path::Rejudge, only)
}

/// The 30 s tick: `stale_for` triggers whose source has been stale long
/// enough, and mutes/simulates that expired since `last`.
pub fn tick(last: i64, now: i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(sources) = store::list_live(&conn) else {
        return;
    };
    let stale_triggers: Vec<ts::TriggerRow> = ts::list_by_status(&conn, TriggerStatus::Active)
        .unwrap_or_default()
        .into_iter()
        .filter(|t| matches!(t.on, TriggerOn::StaleFor { .. }))
        .collect();
    for s in &sources {
        let expired = |until: Option<i64>| until.is_some_and(|u| u > last && u <= now);
        if expired(s.mute_until) || expired(s.simulate_until) {
            rejudge_open(&conn, s, now, None);
        }
        for t in stale_triggers.iter().filter(|t| t.source_id == s.id) {
            rejudge_open(&conn, s, now, Some(&t.id));
        }
    }
}

/// Re-queue `pending` fires older than 60 s whose incident is still open;
/// closed ones are marked `suppressed`/`deduped` [GV63].
pub fn redeliver_pending(now: i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(rows) = ts::pending_before(&conn, now - 60) else {
        return;
    };
    let mut wrote = false;
    for (fire_id, trigger_id, incident, item) in rows {
        let job = (|| {
            let t = ts::get(&conn, &trigger_id).ok()??;
            let (state, _) = ts::load_state(&conn, &t.source_id).ok()??;
            let open = triggers::open_incidents(&state);
            let tr = open.into_iter().find(|tr| {
                triggers::matches(&t.on, tr)
                    && tr.scope_item == item
                    && (tr.incident_id.as_deref() == Some(incident.as_str())
                        || (tr.incident_id.is_none() && triggers::stale_incident_id(&t.source_id, &state) == incident))
            })?;
            let at = ts::get_fire(&conn, &fire_id).ok()??.at;
            Some((t, Job { fire_id: fire_id.clone(), trigger_id: trigger_id.clone(), source_id: String::new(), transition: tr, at }))
        })();
        match job {
            Some((t, mut j)) => {
                j.source_id = t.source_id.clone();
                pipe().enqueue(&t.target_project_id, j);
            }
            None => {
                let _ = ts::mark_suppressed(&conn, &fire_id, "deduped");
                wrote = true;
            }
        }
    }
    if wrote {
        changed(&conn);
    }
}

// ── delivery ─────────────────────────────────────────────────────────

fn outcome_parts(o: &WakeOutcome) -> (&'static str, Option<&str>, Option<bool>, Option<&str>) {
    match o {
        WakeOutcome::Delivered { session_id, woke } => ("delivered", session_id.as_deref(), Some(*woke), None),
        WakeOutcome::Inbox => ("inbox", None, None, None),
        WakeOutcome::Undeliverable { code } => ("undeliverable", None, None, Some(code.as_str())),
    }
}

/// Build one note for a batch and hand it to the waker. Fires after the
/// lead are logged `coalesced` into it first.
fn deliver_batch(waker: &dyn Waker, batch: &Batch, now: i64) {
    let Some(lead) = batch.jobs.first() else {
        return;
    };
    // Gather everything under the lock, deliver without it (a wake can take
    // the whole wake timeout).
    let gathered = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for j in &batch.jobs[1..] {
            let _ = ts::mark_coalesced(&conn, &j.fire_id, &lead.fire_id);
        }
        let mut evs = Vec::new();
        for j in &batch.jobs {
            let Some(src) = store::get_by_id(&conn, &j.source_id).ok().flatten() else { continue };
            let (state, proj) = ts::load_state(&conn, &j.source_id)
                .ok()
                .flatten()
                .unwrap_or_else(|| (SourceState::new(&j.source_id), None));
            let owner_note = ts::get(&conn, &j.trigger_id).ok().flatten().and_then(|t| t.note);
            evs.push((src, state, proj, j.transition.clone(), j.at, owner_note));
        }
        evs
    };
    if gathered.is_empty() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = ts::mark_suppressed(&conn, &lead.fire_id, "deduped");
        return;
    }
    let host = this_host();
    let text = note::build(&NoteInput {
        events: gathered
            .iter()
            .map(|(src, st, proj, tr, at, n)| NoteEvent {
                source: src,
                transition: tr,
                state: st,
                projection: proj.as_ref(),
                at: *at,
                owner_note: n.as_deref(),
            })
            .collect(),
        host: &host,
    });
    let (src0, st0, _, tr0, at0, _) = &gathered[0];
    let req = WakeRequest {
        project_id: batch.project_id.clone(),
        note: text.clone(),
        from: format!("source:{}", src0.name),
        title: note::title(&src0.name, &note::phrase(tr0, src0, st0, *at0)),
    };
    let out = waker.deliver(&req);
    let (outcome, session, woke, detail) = outcome_parts(&out);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = ts::finish_fire(
        &conn,
        &lead.fire_id,
        &Delivery {
            outcome,
            delivered_at: now,
            session_id: session,
            woke,
            note_bytes: Some(text.len() as i64),
            detail,
        },
    );
    if matches!(&out, WakeOutcome::Undeliverable { code } if code == "workspace_not_found") {
        let _ = ts::mark_undeliverable_for_project(&conn, &batch.project_id, now);
    }
    changed(&conn);
}

/// A person's test wake: one `[TEST]` note to the trigger's agent, now, in
/// line with any delivery already in flight to that agent. Returns the
/// outcome code and whether the agent was woken.
pub fn test_wake(trigger: &ts::TriggerRow, source: &SourceRow, now: i64) -> (&'static str, bool) {
    let fire_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ts::insert_fire(
            &conn,
            &NewFire {
                trigger_id: &trigger.id,
                source_id: &source.id,
                target_project_id: &trigger.target_project_id,
                incident_id: &format!("test:{}", uuid::Uuid::new_v4()),
                item_key: None,
                at: now,
                outcome: "test",
                suppress_reason: None,
            },
        )
        .ok()
        .flatten()
    };
    let p = pipe();
    let text = note::build_test(source, &this_host(), trigger.note.as_deref());
    let held = p.acquire(&trigger.target_project_id, Duration::from_secs(90));
    let out = if held {
        let o = p.waker().deliver(&WakeRequest {
            project_id: trigger.target_project_id.clone(),
            note: text.clone(),
            from: format!("source:{}", source.name),
            title: note::title(&source.name, "test"),
        });
        p.finish(&trigger.target_project_id);
        o
    } else {
        WakeOutcome::Undeliverable { code: "busy".into() }
    };
    let (outcome, session, woke, detail) = outcome_parts(&out);
    if let Some(id) = fire_id {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = ts::finish_fire(
            &conn,
            &id,
            &Delivery {
                outcome: "test",
                delivered_at: now,
                session_id: session,
                woke,
                note_bytes: Some(text.len() as i64),
                detail: Some(detail.unwrap_or(outcome)),
            },
        );
        if matches!(&out, WakeOutcome::Undeliverable { code } if code == "workspace_not_found") {
            let _ = ts::mark_undeliverable_for_project(&conn, &trigger.target_project_id, now);
        }
        changed(&conn);
    }
    (outcome, woke.unwrap_or(false))
}

/// Test seams: a fake pipe (manual: no worker threads) and fake delivery
/// functions for [`DaemonWaker`]. Never a real CLI in tests.
#[cfg(test)]
pub mod test_seams {
    use super::*;
    use std::cell::RefCell;

    pub struct Seams {
        pub live: Box<dyn Fn(&str, &str, &str) -> LiveResult>,
        pub ws_path: Box<dyn Fn(&str) -> Option<String>>,
        pub inbox: Box<dyn Fn(&str, &str, &str, &str) -> Result<(), String>>,
    }

    thread_local! {
        static PIPE: RefCell<Option<Arc<Pipe>>> = const { RefCell::new(None) };
        static SEAMS: RefCell<Option<Arc<Seams>>> = const { RefCell::new(None) };
    }

    pub fn set_pipe(p: Option<Arc<Pipe>>) {
        PIPE.with(|s| *s.borrow_mut() = p);
    }

    pub(super) fn pipe() -> Option<Arc<Pipe>> {
        PIPE.with(|s| s.borrow().clone())
    }

    pub fn set(s: Option<Seams>) {
        SEAMS.with(|c| *c.borrow_mut() = s.map(Arc::new));
    }

    pub(super) fn current() -> Option<Arc<Seams>> {
        SEAMS.with(|c| c.borrow().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::sources::state::{Health, Reason};
    use std::sync::Mutex as StdMutex;

    /// Records every note; answers `Delivered`.
    #[derive(Default)]
    pub struct FakeWaker {
        pub calls: StdMutex<Vec<WakeRequest>>,
        pub answer: StdMutex<Option<WakeOutcome>>,
    }

    struct FakeHandle(Arc<FakeWaker>);

    impl Waker for FakeHandle {
        fn deliver(&self, req: &WakeRequest) -> WakeOutcome {
            self.0.calls.lock().unwrap().push(WakeRequest {
                project_id: req.project_id.clone(),
                note: req.note.clone(),
                from: req.from.clone(),
                title: req.title.clone(),
            });
            self.0.answer
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(WakeOutcome::Delivered { session_id: Some("sess-1".into()), woke: true })
        }
    }

    struct Env {
        _db: k2_core::db::TestDbGuard,
        fake: Arc<FakeWaker>,
        pipe: Arc<Pipe>,
    }

    impl Drop for Env {
        fn drop(&mut self) {
            test_seams::set_pipe(None);
        }
    }

    const NOW: i64 = 1_760_000_000;

    fn env() -> Env {
        let db = k2_core::db::scoped_for_test();
        let fake = Arc::new(FakeWaker::default());
        let pipe = Arc::new(Pipe::new(Box::new(FakeHandle(fake.clone()))));
        test_seams::set_pipe(Some(pipe.clone()));
        {
            let h = k2_core::db::shared();
            let c = h.lock();
            c.execute(
                "INSERT INTO sources (id, name, label, spec_json, spec_hash, origins_json, every_secs, viewers, \
                 agent_access, keep, status, made_by, rev, created_at, updated_at) VALUES ('s1', 'fleet', 'Fleet', \
                 '{}', ?1, '[\"https://a.k2.dev:443\"]', 60, 'member', 'values', 'values', 'active', 'owner', 1, 0, 0)",
                rusqlite::params!["a".repeat(64)],
            )
            .unwrap();
            c.execute(
                "INSERT INTO source_state (source_id, health, phase, updated_at, incident_id, rev) \
                 VALUES ('s1', 'ok', 'live', ?1, NULL, 1)",
                rusqlite::params![NOW - 120],
            )
            .unwrap();
        }
        Env { _db: db, fake, pipe }
    }

    fn conn_do<T>(f: impl FnOnce(&Connection) -> T) -> T {
        let h = k2_core::db::shared();
        let c = h.lock();
        f(&c)
    }

    fn add_trigger(on: TriggerOn, project: &str) -> ts::TriggerRow {
        conn_do(|c| {
            ts::insert(
                c,
                &ts::NewTrigger {
                    source_id: "s1",
                    on: &on,
                    target_project_id: project,
                    note: Some("check the relay first"),
                    cooldown_secs: 1800,
                    daily_cap: 12,
                    status: TriggerStatus::Active,
                    made_by: "owner",
                    made_by_project: None,
                    approved_by: Some("owner"),
                    now: 0,
                },
            )
            .unwrap()
        })
    }

    fn set_item(key: &str, health: &str, incident: Option<&str>) {
        conn_do(|c| {
            c.execute(
                "INSERT OR REPLACE INTO source_items (source_id, item_key, is_target, health, reason, hard_since, incident_id) \
                 VALUES ('s1', ?1, 1, ?2, 'timeout', ?3, ?4)",
                rusqlite::params![key, health, NOW - 60, incident],
            )
            .unwrap();
        });
    }

    fn row() -> SourceRow {
        conn_do(|c| store::get_by_id(c, "s1").unwrap().unwrap())
    }

    fn item_crit(key: &str, incident: &str) -> Transition {
        Transition {
            scope_item: Some(key.into()),
            from: Health::Ok,
            to: Health::Crit,
            kind: TransitionKind::WentCrit,
            incident_id: Some(incident.into()),
            reason: Some(Reason::Timeout),
            hard: true,
        }
    }

    fn drain(e: &Env, now: i64) -> usize {
        let mut n = 0;
        while let Some(b) = e.pipe.try_take() {
            e.pipe.deliver_batch(&b, now);
            n += 1;
        }
        n
    }

    fn fires(trigger: &str) -> Vec<ts::FireRow> {
        conn_do(|c| ts::list_fires(c, trigger, 200).unwrap())
    }

    #[test]
    fn fires_once_per_incident_with_a_note() {
        let e = env();
        let t = add_trigger(TriggerOn::ItemWentCrit, "p1");
        set_item("ohana", "crit", Some("inc-o"));
        let tr = item_crit("ohana", "inc-o");
        handle_transitions(&row(), &[tr.clone()], NOW);
        assert_eq!(drain(&e, NOW), 1);
        let calls = e.fake.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].project_id, "p1");
        assert_eq!(calls[0].from, "source:fleet");
        assert!(calls[0].note.starts_with("[source:fleet] ohana went red at "), "{}", calls[0].note);
        assert!(calls[0].note.contains("Owner's note: check the relay first"));
        drop(calls);
        let f = fires(&t.id);
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].outcome.as_str(), f[0].woke), ("delivered", Some(true)));
        assert!(f[0].note_bytes.unwrap() > 0);
        // The same incident again (next cycle, a re-sent transition): deduped.
        handle_transitions(&row(), &[tr], NOW + 300);
        assert_eq!(drain(&e, NOW + 300), 0);
        let f = fires(&t.id);
        assert_eq!(f[0].suppress_reason.as_deref(), Some("deduped"));
        assert_eq!(e.fake.calls.lock().unwrap().len(), 1);
        let last = conn_do(|c| ts::get(c, &t.id).unwrap().unwrap().last_fire_at);
        assert_eq!(last, Some(NOW));
    }

    #[test]
    fn escalation_fires_went_crit_once() {
        let e = env();
        let crit = add_trigger(TriggerOn::WentCrit, "p1");
        let warn = add_trigger(TriggerOn::WentWarn, "p1");
        let mut w = item_crit("x", "inc-1");
        w.scope_item = None;
        w.kind = TransitionKind::WentWarn;
        handle_transitions(&row(), &[w.clone()], NOW);
        let mut c = w.clone();
        c.kind = TransitionKind::WentCrit;
        handle_transitions(&row(), &[c.clone()], NOW + 120);
        handle_transitions(&row(), &[c], NOW + 240);
        drain(&e, NOW + 240);
        let counted = |id: &str| fires(id).iter().filter(|f| f.outcome == "delivered" || f.outcome == "coalesced").count();
        assert_eq!(counted(&warn.id), 1);
        assert_eq!(counted(&crit.id), 1);
    }

    #[test]
    fn one_in_flight_per_agent_and_later_fires_coalesce() {
        let e = env();
        let t = add_trigger(TriggerOn::ItemWentCrit, "p1");
        handle_transitions(&row(), &[item_crit("a", "i-a")], NOW);
        let first = e.pipe.try_take().expect("first batch");
        assert_eq!(first.jobs.len(), 1);
        // While a is in flight, b and c arrive: they wait, together.
        handle_transitions(&row(), &[item_crit("b", "i-b")], NOW + 2000);
        handle_transitions(&row(), &[item_crit("c", "i-c")], NOW + 4000);
        assert!(e.pipe.try_take().is_none(), "nothing else goes to p1 while one is in flight");
        e.pipe.deliver_batch(&first, NOW + 4001);
        let second = e.pipe.try_take().expect("the waiting batch");
        assert_eq!(second.jobs.len(), 2);
        e.pipe.deliver_batch(&second, NOW + 4002);
        let calls = e.fake.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        let blocks: Vec<&str> = calls[1].note.split("\n\n").collect();
        assert_eq!(blocks.len(), 2, "{}", calls[1].note);
        assert!(blocks[0].contains("b went red") && blocks[1].contains("c went red"), "oldest first");
        drop(calls);
        let f = fires(&t.id);
        assert_eq!(f.iter().filter(|f| f.outcome == "coalesced").count(), 1);
        assert_eq!(f.iter().filter(|f| f.outcome == "delivered").count(), 2);
    }

    #[test]
    fn cooldown_and_daily_cap_and_agent_cap_suppress() {
        let e = env();
        let t = add_trigger(TriggerOn::ItemWentCrit, "p1");
        handle_transitions(&row(), &[item_crit("a", "i-a")], NOW);
        handle_transitions(&row(), &[item_crit("b", "i-b")], NOW + 60);
        drain(&e, NOW + 60);
        let f = fires(&t.id);
        assert_eq!(f[0].suppress_reason.as_deref(), Some("cooldown"));
        // A different trigger to the same agent, with 30 fires already this hour.
        let other = add_trigger(TriggerOn::WentCrit, "p1");
        conn_do(|c| {
            for i in 0..30 {
                ts::insert_fire(
                    c,
                    &NewFire {
                        trigger_id: "elsewhere",
                        source_id: "s1",
                        target_project_id: "p1",
                        incident_id: &format!("x{i}"),
                        item_key: None,
                        at: NOW + 100,
                        outcome: "pending",
                        suppress_reason: None,
                    },
                )
                .unwrap();
            }
        });
        let mut src = item_crit("z", "i-src");
        src.scope_item = None;
        handle_transitions(&row(), &[src], NOW + 200);
        assert_eq!(fires(&other.id)[0].suppress_reason.as_deref(), Some("agent_cap"));
    }

    #[test]
    fn mute_suppresses_and_unmute_fires_open_incidents_once() {
        let e = env();
        let t = add_trigger(TriggerOn::ItemWentCrit, "p1");
        conn_do(|c| {
            c.execute("UPDATE sources SET mute_until = ?1, mute_by = 'owner' WHERE id = 's1'", rusqlite::params![NOW + 600])
                .unwrap();
        });
        set_item("open", "crit", Some("i-open"));
        handle_transitions(&row(), &[item_crit("open", "i-open"), item_crit("closed", "i-closed")], NOW);
        assert_eq!(drain(&e, NOW), 0);
        assert!(fires(&t.id).iter().all(|f| f.suppress_reason.as_deref() == Some("muted")));
        // Unmute: only the incident still open fires, once.
        conn_do(|c| {
            c.execute("UPDATE sources SET mute_until = NULL, mute_by = NULL WHERE id = 's1'", []).unwrap();
        });
        handle_unmute(&row(), NOW + 700);
        handle_unmute(&row(), NOW + 701);
        assert_eq!(drain(&e, NOW + 701), 1);
        let calls = e.fake.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].note.contains("open went red"));
        assert!(!calls[0].note.contains("closed"), "an incident that closed inside the window never fires");
    }

    #[test]
    fn simulated_paused_gap_and_draining_never_fire() {
        let e = env();
        let t = add_trigger(TriggerOn::ItemWentCrit, "p1");
        conn_do(|c| {
            c.execute(
                "UPDATE sources SET simulate_state = 'crit', simulate_until = ?1, simulate_by = 'owner' WHERE id = 's1'",
                rusqlite::params![NOW + 600],
            )
            .unwrap();
        });
        handle_transitions(&row(), &[item_crit("a", "i-a")], NOW);
        conn_do(|c| {
            c.execute("UPDATE sources SET simulate_state = NULL, simulate_until = NULL, status = 'paused', pause_reason = 'paused_by_person' WHERE id = 's1'", [])
                .unwrap();
        });
        handle_transitions(&row(), &[item_crit("b", "i-b")], NOW);
        conn_do(|c| {
            c.execute("UPDATE sources SET status = 'active', pause_reason = NULL WHERE id = 's1'", []).unwrap();
            c.execute("UPDATE source_state SET phase = 'paused', gap_kind = 'host_slept', gap_from = 1, gap_to = 2 WHERE source_id = 's1'", [])
                .unwrap();
        });
        handle_transitions(&row(), &[item_crit("c", "i-c")], NOW);
        conn_do(|c| {
            c.execute("UPDATE source_state SET phase = 'live', gap_kind = NULL, gap_from = NULL, gap_to = NULL WHERE source_id = 's1'", [])
                .unwrap();
        });
        crate::sources::DRAINING.store(true, std::sync::atomic::Ordering::SeqCst);
        handle_transitions(&row(), &[item_crit("d", "i-d")], NOW);
        crate::sources::DRAINING.store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(drain(&e, NOW), 0);
        let mut reasons: Vec<String> = fires(&t.id).into_iter().filter_map(|f| f.suppress_reason).collect();
        reasons.sort();
        assert_eq!(reasons, vec!["draining", "gap", "paused", "simulated"]);
    }

    #[test]
    fn stale_for_fires_once_from_the_tick_and_ignores_gaps() {
        let e = env();
        let t = add_trigger(TriggerOn::StaleFor { secs: 900 }, "p1");
        // updated 2000 s ago, every 60 → stale since NOW - 1820.
        conn_do(|c| {
            c.execute("UPDATE source_state SET phase = 'stale', updated_at = ?1 WHERE source_id = 's1'", rusqlite::params![NOW - 2000])
                .unwrap();
        });
        tick(NOW - 30, NOW);
        tick(NOW, NOW + 30);
        assert_eq!(drain(&e, NOW + 30), 1);
        assert_eq!(fires(&t.id).len(), 1, "one row per stale episode, no tick spam");
        assert!(e.fake.calls.lock().unwrap()[0].note.contains("has had no good check for"));
        // A gap (paused) is never stale.
        let t2 = add_trigger(TriggerOn::StaleFor { secs: 300 }, "p2");
        conn_do(|c| {
            c.execute("UPDATE source_state SET phase = 'paused', gap_kind = 'daemon_restarted', updated_at = 1 WHERE source_id = 's1'", [])
                .unwrap();
        });
        tick(NOW + 30, NOW + 60);
        assert!(fires(&t2.id).is_empty());
    }

    #[test]
    fn pending_fires_are_redelivered_or_closed() {
        let e = env();
        let t = add_trigger(TriggerOn::ItemWentCrit, "p1");
        set_item("open", "crit", Some("i-open"));
        let (open, closed) = conn_do(|c| {
            let mk = |inc: &str, item: &str| {
                ts::insert_fire(
                    c,
                    &NewFire {
                        trigger_id: &t.id,
                        source_id: "s1",
                        target_project_id: "p1",
                        incident_id: inc,
                        item_key: Some(item),
                        at: NOW - 600,
                        outcome: "pending",
                        suppress_reason: None,
                    },
                )
                .unwrap()
                .unwrap()
            };
            (mk("i-open", "open"), mk("i-gone", "gone"))
        });
        redeliver_pending(NOW);
        assert_eq!(drain(&e, NOW), 1);
        let get = |id: &str| conn_do(|c| ts::get_fire(c, id).unwrap().unwrap());
        assert_eq!(get(&open).outcome, "delivered");
        assert_eq!((get(&closed).outcome.as_str(), get(&closed).suppress_reason.as_deref()), ("suppressed", Some("deduped")));
    }

    #[test]
    fn missing_project_makes_the_trigger_undeliverable() {
        let e = env();
        let t = add_trigger(TriggerOn::ItemWentCrit, "gone-project");
        *e.fake.answer.lock().unwrap() = Some(WakeOutcome::Undeliverable { code: "workspace_not_found".into() });
        handle_transitions(&row(), &[item_crit("a", "i-a")], NOW);
        drain(&e, NOW);
        let f = fires(&t.id);
        assert_eq!((f[0].outcome.as_str(), f[0].detail.as_deref()), ("undeliverable", Some("workspace_not_found")));
        assert_eq!(conn_do(|c| ts::get(c, &t.id).unwrap().unwrap().status), TriggerStatus::Undeliverable);
    }

    #[test]
    fn test_wake_is_marked_and_logged_as_test() {
        let e = env();
        let t = add_trigger(TriggerOn::WentCrit, "p1");
        let (outcome, woke) = test_wake(&t, &row(), NOW);
        assert_eq!((outcome, woke), ("delivered", true));
        assert!(e.fake.calls.lock().unwrap()[0].note.starts_with("[TEST] "));
        let f = fires(&t.id);
        assert_eq!((f[0].outcome.as_str(), f[0].detail.as_deref()), ("test", Some("delivered")));
        assert_eq!(conn_do(|c| ts::fires_since(c, &t.id, 0).unwrap()), 0, "tests don't count toward caps");
    }

    /// The real waker's decision path, through the seam: live delivery by
    /// project id with via "source", else the inbox; no CLI is spawned.
    #[test]
    fn daemon_waker_uses_the_msg_wake_path_then_the_inbox() {
        use std::rc::Rc;
        let seen: Rc<RefCellLog> = Rc::new(RefCellLog::default());
        let req = WakeRequest { project_id: "p1".into(), note: "n".into(), from: "source:fleet".into(), title: "t".into() };
        let s1 = seen.clone();
        let live_ok = move |ws: &str, _t: &str, from: &str| {
            s1.push(format!("live {ws} {from}"));
            LiveResult { success: true, session_id: Some("sess".into()), woke: true, reason: None }
        };
        let out = deliver_with(&req, &live_ok, &|_| Some("/w".into()), &|_, _, _, _| Ok(()));
        assert_eq!(out, WakeOutcome::Delivered { session_id: Some("sess".into()), woke: true });
        assert_eq!(seen.take(), vec!["live p1 source:fleet".to_string()]);
        let s2 = seen.clone();
        let live_fail = |_: &str, _: &str, _: &str| LiveResult { success: false, session_id: None, woke: false, reason: Some("pty_stalled".into()) };
        let inbox = move |path: &str, title: &str, _b: &str, from: &str| {
            s2.push(format!("inbox {path} {title} {from}"));
            Ok(())
        };
        assert_eq!(deliver_with(&req, &live_fail, &|_| Some("/w".into()), &inbox), WakeOutcome::Inbox);
        assert_eq!(seen.take(), vec!["inbox /w t source:fleet".to_string()]);
        let no_agent = |_: &str, _: &str, _: &str| LiveResult { success: false, session_id: None, woke: false, reason: Some("no_agent_mode".into()) };
        assert_eq!(
            deliver_with(&req, &no_agent, &|_| Some("/w".into()), &|_, _, _, _| Ok(())),
            WakeOutcome::Undeliverable { code: "no_agent_mode".into() }
        );
        assert_eq!(
            deliver_with(&req, &live_ok, &|_| None, &|_, _, _, _| Ok(())),
            WakeOutcome::Undeliverable { code: "workspace_not_found".into() }
        );
        assert_eq!(
            deliver_with(&req, &live_fail, &|_| Some("/w".into()), &|_, _, _, _| Err("disk".into())),
            WakeOutcome::Undeliverable { code: "inbox_failed".into() }
        );
        // And DaemonWaker routes through the same function (seamed).
        let s3 = seen.clone();
        test_seams::set(Some(test_seams::Seams {
            live: Box::new(move |ws, _t, from| {
                s3.push(format!("seam {ws} {from}"));
                LiveResult { success: true, session_id: None, woke: false, reason: None }
            }),
            ws_path: Box::new(|_| Some("/w".into())),
            inbox: Box::new(|_, _, _, _| Ok(())),
        }));
        assert_eq!(DaemonWaker.deliver(&req), WakeOutcome::Delivered { session_id: None, woke: false });
        test_seams::set(None);
        assert_eq!(seen.take(), vec!["seam p1 source:fleet".to_string()]);
    }

    #[derive(Default)]
    struct RefCellLog(std::cell::RefCell<Vec<String>>);
    impl RefCellLog {
        fn push(&self, s: String) {
            self.0.borrow_mut().push(s);
        }
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.0.borrow_mut())
        }
    }
}
