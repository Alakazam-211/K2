//! The bounded push worker (prd-custom-app-push-v1 CP22, A9).
//!
//! Sends never run on a route thread and never one thread per send (today's
//! Companion dispatch is a detached thread each; App push must not copy
//! that, or one App's fan-out could start thousands). A fixed pool of
//! [`WORKERS`] threads drains one queue; each App may hold at most
//! [`MAX_PENDING_PER_APP`] pending device sends (`429 push_queue_full`
//! beyond, all or nothing). Nothing survives a restart: pending sends are
//! dropped with the process, like the Companion `PushTarget` contract
//! (`push/mod.rs:39-42`); the in-memory send log goes with them.

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

struct State {
    queue: VecDeque<Job>,
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
        Queue { state: Mutex::new(State { queue: VecDeque::new(), pending: HashMap::new(), in_flight: 0 }), work: Condvar::new(), idle: Condvar::new() }
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
    st.queue.extend(jobs);
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
                if let Some(j) = st.queue.pop_front() {
                    st.in_flight += 1;
                    break j;
                }
                st = q.work.wait(st).unwrap_or_else(|e| e.into_inner());
            }
        };
        let outcome = sender::global().deliver(&job.target, &job.notification, &job.stamps);
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
