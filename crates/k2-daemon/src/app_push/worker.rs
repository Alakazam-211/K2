//! The bounded push worker (prd-custom-app-push-v1 CP22, A9).
//!
//! Sends never run on a route thread and never one thread per send (today's
//! Companion dispatch is a detached thread each; App push must not copy
//! that, or one App's fan-out could start thousands). A fixed pool of
//! [`WORKERS`] threads drains a queue per App, taken round-robin ([`Fair`]),
//! so one App's backlog never delays another App's sends (review B-L3);
//! each App may hold at most [`MAX_PENDING_PER_APP`] pending device sends
//! (`429 push_queue_full` beyond, all or nothing). Right before calling
//! Apple or Google a worker checks again that the App's push is on and the
//! row still holds that token, with its guest's grant enabled and sign-in
//! live (review B-L1); otherwise the send is `dropped`. Nothing survives a
//! restart: pending sends are dropped with the process, like the Companion
//! `PushTarget` contract (`push/mod.rs:39-42`); the in-memory send log goes
//! with them.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
#[cfg(test)]
use std::time::Duration;

use super::payload::{Notification, Stamps};
use super::sender::{self, Outcome, Target};
use super::sendlog;

/// Worker threads (each blocks on one HTTP call at a time).
pub const WORKERS: usize = 4;
/// Pending device sends per App (CP22).
pub const MAX_PENDING_PER_APP: usize = 5_000;

/// One device send.
pub struct Job {
    pub target: Target,
    pub notification: Arc<Notification>,
    pub stamps: Stamps,
}

/// One queue per key, served round-robin: each pop takes the next key's
/// oldest item, then that key goes to the back of the ring.
pub struct Fair<T> {
    queues: HashMap<String, VecDeque<T>>,
    ring: VecDeque<String>,
}

impl<T> Default for Fair<T> {
    fn default() -> Self {
        Fair { queues: HashMap::new(), ring: VecDeque::new() }
    }
}

impl<T> Fair<T> {
    pub fn push_all(&mut self, key: &str, items: impl IntoIterator<Item = T>) {
        let q = self.queues.entry(key.to_string()).or_default();
        let was_empty = q.is_empty();
        q.extend(items);
        if was_empty && !q.is_empty() {
            self.ring.push_back(key.to_string());
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        while let Some(key) = self.ring.pop_front() {
            let Some(q) = self.queues.get_mut(&key) else { continue };
            let Some(item) = q.pop_front() else {
                self.queues.remove(&key);
                continue;
            };
            if q.is_empty() {
                self.queues.remove(&key);
            } else {
                self.ring.push_back(key);
            }
            return Some(item);
        }
        None
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}

/// A queued job. In tests it carries the enqueuing thread's DB (a test's
/// scoped k2.db is thread-local; the worker threads must read the same one).
struct Queued {
    job: Job,
    #[cfg(test)]
    db: std::sync::Arc<parking_lot::ReentrantMutex<rusqlite::Connection>>,
}

struct State {
    queue: Fair<Queued>,
    pending: HashMap<String, usize>,
    in_flight: usize,
}

struct Queue {
    state: Mutex<State>,
    work: Condvar,
    idle: Condvar,
}

fn queue() -> &'static Queue {
    static Q: OnceLock<Queue> = OnceLock::new();
    Q.get_or_init(|| {
        for i in 0..WORKERS {
            let spawned = std::thread::Builder::new().name(format!("app-push-{i}")).spawn(run);
            if spawned.is_err() {
                sender::log("a push worker thread didn't start");
            }
        }
        Queue { state: Mutex::new(State { queue: Fair::default(), pending: HashMap::new(), in_flight: 0 }), work: Condvar::new(), idle: Condvar::new() }
    })
}

/// Queue `jobs` for one App, all or nothing. `Err(pending)` = over the
/// App's ceiling.
pub fn enqueue(app: &str, jobs: Vec<Job>) -> Result<usize, usize> {
    let q = queue();
    let mut st = q.state.lock().unwrap_or_else(|e| e.into_inner());
    let pending = st.pending.get(app).copied().unwrap_or(0);
    if pending + jobs.len() > MAX_PENDING_PER_APP {
        return Err(pending);
    }
    let n = jobs.len();
    *st.pending.entry(app.to_string()).or_insert(0) += n;
    #[cfg(test)]
    let db = k2_core::db::shared();
    st.queue.push_all(
        app,
        jobs.into_iter().map(|job| Queued {
            job,
            #[cfg(test)]
            db: db.clone(),
        }),
    );
    drop(st);
    q.work.notify_all();
    Ok(n)
}

/// Pending device sends for one App.
pub fn pending(app: &str) -> usize {
    queue().state.lock().unwrap_or_else(|e| e.into_inner()).pending.get(app).copied().unwrap_or(0)
}

fn run() {
    let q = queue();
    loop {
        let job = {
            let mut st = q.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(q) = st.queue.pop() {
                    st.in_flight += 1;
                    break q;
                }
                st = q.work.wait(st).unwrap_or_else(|e| e.into_inner());
            }
        };
        let run = |job: &Job| {
            if deliverable(&job.target) {
                sender::global().deliver(&job.target, &job.notification, &job.stamps)
            } else {
                sender::log(&format!("{} send {} device {}: dropped (the phone, the guest's access or the App's push changed before it went out)", job.target.app, job.stamps.send_id, job.target.id));
                Outcome::Dropped
            }
        };
        #[cfg(test)]
        let outcome = k2_core::db::with_scoped_handle(job.db.clone(), || run(&job.job));
        #[cfg(not(test))]
        let outcome = run(&job.job);
        let job = job.job;
        sendlog::record(&job.target.app, &job.stamps.send_id, &outcome);
        if let Outcome::Failed(code) = &outcome {
            sender::log(&format!("{} send {} device {}: failed ({code})", job.target.app, job.stamps.send_id, job.target.id));
        }
        let mut st = q.state.lock().unwrap_or_else(|e| e.into_inner());
        st.in_flight -= 1;
        if let Some(p) = st.pending.get_mut(&job.target.app) {
            *p = p.saturating_sub(1);
            if *p == 0 {
                st.pending.remove(&job.target.app);
            }
        }
        if st.queue.is_empty() && st.in_flight == 0 {
            q.idle.notify_all();
        }
    }
}

/// Still sendable right now (review B-L1, L1): the App's push state is on
/// (the box switch, the owner's switch and the `push:send` approval or the
/// owner's on), and the row exists with this exact token, its guest's grant
/// enabled and its sign-in live. A store error is "no": never send on a
/// doubt.
pub(crate) fn deliverable(t: &Target) -> bool {
    super::state(&t.app).is_ok() && k2_core::skin::app_push::still_deliverable(&t.app, &t.id, &t.token).unwrap_or(false)
}

/// Tests: wait until one App's sends are done (or `max` passes).
#[cfg(test)]
pub(crate) fn wait_app(app: &str, max: Duration) -> bool {
    let q = queue();
    let deadline = std::time::Instant::now() + max;
    let mut st = q.state.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if st.pending.get(app).copied().unwrap_or(0) == 0 {
            return true;
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return false;
        }
        let (s, _) = q.idle.wait_timeout(st, left.min(Duration::from_millis(20))).unwrap_or_else(|e| e.into_inner());
        st = s;
    }
}

#[cfg(test)]
mod tests {
    use super::Fair;

    #[test]
    fn apps_are_served_round_robin_so_one_backlog_never_starves_another() {
        let mut f: Fair<String> = Fair::default();
        f.push_all("big", (0..5).map(|i| format!("big{i}")));
        f.push_all("small", ["small0".to_string()]);
        f.push_all("big", ["big5".to_string()]);
        let order: Vec<String> = std::iter::from_fn(|| f.pop()).collect();
        assert_eq!(order, ["big0", "small0", "big1", "big2", "big3", "big4", "big5"], "small goes second, not after big's backlog");
        assert!(f.is_empty());
        f.push_all("a", ["a0".to_string(), "a1".to_string()]);
        f.push_all("b", ["b0".to_string(), "b1".to_string()]);
        f.push_all("c", Vec::<String>::new());
        let order: Vec<String> = std::iter::from_fn(|| f.pop()).collect();
        assert_eq!(order, ["a0", "b0", "a1", "b1"]);
    }
}
