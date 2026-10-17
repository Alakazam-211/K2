//! Garden v3 data sources, daemon half (prd-garden-v3-external-data-cards-v1;
//! build contract `contract-sources-v1` §3.1, §3.4).
//!
//! The daemon owns the poller, the network, secret binding and trigger
//! wakes; `k2_core::sources` owns the pure maths. Routes are in
//! `crate::sources_routes`.
//!
//! [`Engine`]: one scheduler thread ([`scheduler::Sched`]) and a pool of
//! `MAX_CONCURRENT_FETCHES` worker threads that run fetches. Nothing runs
//! until [`start`] (daemon boot); before that the route-facing calls are
//! no-ops, so the routes work headless in tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::log_debug;
use k2_core::sources::{FetchPlan, SourceRow, Transition, MAX_CONCURRENT_FETCHES};
use parking_lot::ReentrantMutex;
use rusqlite::Connection;

use crate::cli_response::CliResponse;

pub mod approve_routes;
pub mod budget;
pub mod fetch;
pub mod scheduler;
pub mod secrets;
pub mod tinker_routes;
pub mod trigger_routes;
pub mod wake;

#[cfg(test)]
mod manage_tests;

/// Set by the 0.46.0 hand-off when present [GV63]: no fetch starts and
/// fires are suppressed with `draining` while it is up.
pub static DRAINING: AtomicBool = AtomicBool::new(false);

/// A refusal from a sources handler: the error envelope
/// `{"error": {"code", "hint", ...extra}}` with its HTTP status.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceErr {
    pub code: &'static str,
    pub status: u16,
    pub hint: String,
    /// Extra envelope fields (`field`, `specCode`, `untilAt`,
    /// `retryAfterSecs`, `removedAt`, `role`, `rootKey`, …).
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl SourceErr {
    pub fn new(code: &'static str, status: u16, hint: impl Into<String>) -> SourceErr {
        SourceErr { code, status, hint: hint.into(), extra: serde_json::Map::new() }
    }

    /// Add one extra envelope field.
    pub fn with(mut self, key: &str, value: impl Into<serde_json::Value>) -> SourceErr {
        self.extra.insert(key.to_string(), value.into());
        self
    }

    pub fn body(&self) -> serde_json::Value {
        let mut e = serde_json::Map::new();
        e.insert("code".into(), self.code.into());
        e.insert("hint".into(), self.hint.clone().into());
        for (k, v) in &self.extra {
            e.insert(k.clone(), v.clone());
        }
        serde_json::json!({ "error": e })
    }

    pub fn response(&self) -> CliResponse {
        CliResponse { status: status_line(self.status), content_type: "application/json", body: self.body().to_string() }
    }
}

impl From<rusqlite::Error> for SourceErr {
    fn from(e: rusqlite::Error) -> SourceErr {
        log_debug!("[sources] db error: {e}");
        SourceErr::new("db_error", 500, "the server's database refused this change")
    }
}

/// 501 `not_built`: C0 stubs only; never in a release.
pub fn not_built() -> SourceErr {
    SourceErr::new("not_built", 501, "this part of data sources is not built yet")
}

pub(crate) fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        410 => "410 Gone",
        413 => "413 Payload Too Large",
        423 => "423 Locked",
        429 => "429 Too Many Requests",
        501 => "501 Not Implemented",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

type Db = Arc<ReentrantMutex<Connection>>;
type FetchFn = dyn Fn(&FetchPlan) -> fetch::Fetched + Send + Sync;

/// Facts for `GET /cli/sources/status`.
#[derive(Debug, Clone, Default)]
pub struct EngineStatus {
    pub running: bool,
    pub in_flight: usize,
    pub behind_secs: i64,
    pub parked: Vec<(String, budget::Park)>,
    pub last_gap: Option<(k2_core::sources::GapKind, i64, i64)>,
}

thread_local! {
    /// Set while the trigger pipeline runs for one poll: whether that poll
    /// was the first after a gap (B2: `FireContext::in_gap`).
    static IN_GAP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the hard transitions being handed to the trigger pipeline right
/// now came from the first poll after a gap (sleep or restart). Triggers
/// suppress those with `gap` [GV57].
pub fn transitions_in_gap() -> bool {
    IN_GAP.with(|g| g.get())
}

/// The poller handle.
pub struct Engine {
    sched: Mutex<Option<scheduler::Sched>>,
    cv: Condvar,
    db: Mutex<Option<Db>>,
    fetch: Box<FetchFn>,
    jobs: Mutex<Option<mpsc::Sender<scheduler::Job>>>,
    notify: Mutex<Notify>,
}

/// `sources_changed` coalescing: at most one per 500 ms, the last rev wins.
#[derive(Default)]
struct Notify {
    last: Option<Instant>,
    pending: Option<i64>,
    slow: Vec<(Instant, i64)>,
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

pub fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| Engine::new(Box::new(fetch::fetch_detailed)))
}

fn wall_now() -> i64 {
    chrono::Utc::now().timestamp()
}

impl Engine {
    fn new(fetch: Box<FetchFn>) -> Engine {
        Engine {
            sched: Mutex::new(None),
            cv: Condvar::new(),
            db: Mutex::new(None),
            fetch,
            jobs: Mutex::new(None),
            notify: Mutex::new(Notify::default()),
        }
    }

    fn with_sched<T>(&self, f: impl FnOnce(&mut scheduler::Sched) -> T) -> Option<T> {
        let mut g = self.sched.lock().unwrap_or_else(|e| e.into_inner());
        g.as_mut().map(f)
    }

    /// Poll a source now (Refresh, approve, edit). Returns `nextAt`.
    pub fn poll_now(&self, source_id: &str) -> Result<i64, SourceErr> {
        self.with_sched(|s| s.poll_now(source_id));
        self.cv.notify_all();
        Ok(wall_now())
    }

    /// A definition changed: reload the schedule.
    pub fn reload(&self) {
        self.with_sched(|s| s.mark_dirty());
        self.cv.notify_all();
    }

    /// Manual refresh limit (one per 15 s per source): `Err(seconds)`.
    pub fn refresh_gate(&self, source_id: &str) -> Result<(), u32> {
        self.with_sched(|s| s.refresh_gate(source_id, Instant::now())).unwrap_or(Ok(()))
    }

    pub fn status(&self) -> EngineStatus {
        let now = Instant::now();
        self.with_sched(|s| EngineStatus {
            running: true,
            in_flight: s.in_flight(),
            behind_secs: s.behind_secs(),
            parked: s.budget.parks(now),
            last_gap: s.last_gap,
        })
        .unwrap_or_default()
    }

    /// Hard transitions after a state commit: hands them to the trigger
    /// pipeline (B2).
    pub fn on_transitions(&self, source: &SourceRow, ts: &[Transition]) {
        wake::handle_transitions(source, ts, wall_now());
    }

    fn run_post(&self, post: scheduler::Post) {
        for rev in post.notify {
            self.notify(rev);
        }
        if !post.notify_slow.is_empty() {
            let at = Instant::now() + Duration::from_secs(20);
            let mut n = self.notify.lock().unwrap_or_else(|e| e.into_inner());
            n.slow.extend(post.notify_slow.into_iter().map(|r| (at, r)));
        }
        for (row, ts, in_gap) in post.transitions {
            IN_GAP.with(|g| g.set(in_gap));
            wake::handle_transitions(&row, &ts, wall_now());
            IN_GAP.with(|g| g.set(false));
        }
        for row in post.unmuted {
            wake::handle_unmute(&row, wall_now());
        }
        if post.redeliver {
            wake::redeliver_pending(wall_now());
        }
    }

    /// Coalesced `sources_changed`.
    fn notify(&self, rev: i64) {
        let mut n = self.notify.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if n.last.is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_millis(500)) {
            n.last = Some(now);
            n.pending = None;
            drop(n);
            emit_changed(rev);
        } else {
            n.pending = Some(n.pending.map_or(rev, |p| p.max(rev)));
        }
    }

    fn flush_notify(&self) {
        let mut n = self.notify.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let mut due: Vec<i64> = Vec::new();
        n.slow.retain(|(at, rev)| {
            if *at <= now {
                due.push(*rev);
                false
            } else {
                true
            }
        });
        if let Some(p) = n.pending {
            if n.last.is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_millis(500)) {
                due.push(p);
                n.pending = None;
                n.last = Some(now);
            }
        }
        drop(n);
        if let Some(rev) = due.into_iter().max() {
            emit_changed(rev);
        }
    }

    /// One scheduler pass on this thread; returns the jobs to run.
    fn pass(&self, db: &Db) -> Vec<scheduler::Job> {
        let mut post = scheduler::Post::default();
        let jobs = {
            let conn = db.lock();
            let mut g = self.sched.lock().unwrap_or_else(|e| e.into_inner());
            match g.as_mut() {
                Some(s) => {
                    s.draining = DRAINING.load(Ordering::SeqCst);
                    s.tick(&conn, Instant::now(), wall_now(), &mut post)
                }
                None => Vec::new(),
            }
        };
        self.run_post(post);
        self.flush_notify();
        jobs
    }

    fn finish_job(&self, db: &Db, job: scheduler::Job, fetched: fetch::Fetched) {
        let mut post = scheduler::Post::default();
        {
            let conn = db.lock();
            let mut g = self.sched.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(s) = g.as_mut() {
                s.complete(&conn, job, fetched, Instant::now(), wall_now(), &mut post);
            }
        }
        self.run_post(post);
        self.cv.notify_all();
    }

    fn start_on(&'static self, db: Db) {
        {
            let mut g = self.sched.lock().unwrap_or_else(|e| e.into_inner());
            if g.is_some() {
                return;
            }
            let mut s = scheduler::Sched::new(Instant::now(), wall_now());
            let mut post = scheduler::Post::default();
            {
                let conn = db.lock();
                s.boot(&conn, Instant::now(), wall_now(), &mut post);
            }
            *g = Some(s);
            drop(g);
            self.run_post(post);
        }
        *self.db.lock().unwrap_or_else(|e| e.into_inner()) = Some(db.clone());
        let (tx, rx) = mpsc::channel::<scheduler::Job>();
        let rx = Arc::new(Mutex::new(rx));
        *self.jobs.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx.clone());
        for n in 0..MAX_CONCURRENT_FETCHES {
            let rx = rx.clone();
            let db = db.clone();
            let _ = std::thread::Builder::new().name(format!("sources-fetch-{n}")).spawn(move || loop {
                let job = match rx.lock().unwrap_or_else(|e| e.into_inner()).recv() {
                    Ok(j) => j,
                    Err(_) => return,
                };
                let fetched = (self.fetch)(&job.plan);
                self.finish_job(&db, job, fetched);
            });
        }
        let _ = std::thread::Builder::new().name("sources-scheduler".into()).spawn(move || loop {
            for job in self.pass(&db) {
                if tx.send(job).is_err() {
                    return;
                }
            }
            let wait = self
                .with_sched(|s| s.next_due())
                .flatten()
                .map(|d| d.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_secs(1))
                .clamp(Duration::from_millis(50), Duration::from_millis(500));
            let g = self.sched.lock().unwrap_or_else(|e| e.into_inner());
            let _ = self.cv.wait_timeout(g, wait);
        });
        log_debug!("[sources] poller started");
    }
}

/// Start the poller (daemon boot, after migrations). Idempotent.
pub fn start() {
    match k2_core::db::try_shared() {
        Some(db) => engine().start_on(db),
        None => log_debug!("[sources] no database; poller not started"),
    }
}

fn emit_changed(rev: i64) {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::SourcesChanged { rev });
}

/// Announce a change: `sources_changed {rev}` to every subscriber,
/// coalesced to one per 500 ms (the last `rev` wins).
pub fn notify_changed(rev: i64) {
    engine().notify(rev);
    engine().cv.notify_all();
}

#[cfg(test)]
pub(crate) mod test_engine {
    //! A scheduler driven by hand with a fake fetch (no threads).
    use super::*;

    pub(crate) struct Harness {
        pub sched: scheduler::Sched,
        pub fetch: Box<FetchFn>,
        pub mono: Instant,
        pub wall: i64,
        pub posts: Vec<scheduler::Post>,
    }

    impl Harness {
        pub(crate) fn new(fetch: impl Fn(&FetchPlan) -> fetch::Fetched + Send + Sync + 'static, wall: i64) -> Harness {
            let mono = Instant::now();
            Harness { sched: scheduler::Sched::new(mono, wall), fetch: Box::new(fetch), mono, wall, posts: Vec::new() }
        }

        /// Advance both clocks by `secs` (a sleep advances only the wall
        /// clock: use `sleep`).
        pub(crate) fn advance(&mut self, secs: u64) {
            self.mono += Duration::from_secs(secs);
            self.wall += secs as i64;
        }

        pub(crate) fn sleep(&mut self, secs: i64) {
            self.wall += secs;
        }

        /// One tick; run every dispatched job inline; repeat until nothing
        /// more is dispatched. Returns how many fetches ran.
        pub(crate) fn step(&mut self, conn: &Connection) -> usize {
            let mut ran = 0;
            loop {
                let mut post = scheduler::Post::default();
                let jobs = self.sched.tick(conn, self.mono, self.wall, &mut post);
                self.posts.push(post);
                if jobs.is_empty() {
                    return ran;
                }
                for job in jobs {
                    let f = (self.fetch)(&job.plan);
                    let mut post = scheduler::Post::default();
                    self.sched.complete(conn, job, f, self.mono, self.wall, &mut post);
                    self.posts.push(post);
                    ran += 1;
                }
            }
        }

        pub(crate) fn transitions(&self) -> Vec<(String, Transition, bool)> {
            self.posts
                .iter()
                .flat_map(|p| p.transitions.iter().flat_map(|(r, ts, g)| ts.iter().map(move |t| (r.name.clone(), t.clone(), *g))))
                .collect()
        }
    }
}
