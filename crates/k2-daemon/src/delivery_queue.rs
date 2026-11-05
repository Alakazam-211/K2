//! Per-agent delivery lanes (send-delay fix 2, 0.46.8;
//! `.k2/prds/research-remote-thread-load-v1.md` §4 fix H).
//!
//! A Thread or project chat post used to answer only after its line was
//! typed into the agent's terminal (`workspace_msg::deliver_live*`): about
//! 520 ms of fixed pauses, up to 8 s behind another inject into the same
//! session, up to 20 s waking a sleeping one. Now the post answers once it
//! is stored and its frame is out, and the delivery runs here.
//!
//! - **One lane per key** (the agent the line goes to): jobs on a lane run
//!   one at a time, in the order they were queued, so two quick posts
//!   reach the agent in order. Different agents never wait on each other.
//! - **Daemon-owned, headless-safe.** A lane is a named std thread entered
//!   into the daemon's Tokio runtime ([`crate::rt::spawn`]; a wake spawns a
//!   session, which needs one). It belongs to no HTTP connection, so a
//!   client that hangs up (or none at all) never stops a delivery.
//! - A lane's thread lives only while it has work, and exits when its
//!   queue is empty. A job that panics is logged and the lane goes on.
//! - The delivery itself is unchanged (`deliver_live*` keeps its wake,
//!   inject lock and retries); only who waits for it changed.
//!
//! Callers that need the outcome (an agent's `k2 project msg`, Design
//! notes) use [`run_in_order`]: their job still takes its place in the
//! lane, so it never jumps ahead of a person's queued post.
//!
//! The queue itself is in memory. Since 0.46.9 the Thread and project chat
//! callers also write each job to the delivery journal
//! ([`crate::delivery_journal`], SQLite), and boot queues again what a
//! restart left behind. A job cut off mid-delivery is never typed in
//! twice: its Thread is told instead.

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Queued jobs per lane key. A key is present exactly while its lane's
/// thread runs (inserted with the first job, removed by the thread when
/// it finds the queue empty, under this lock).
static LANES: LazyLock<Mutex<HashMap<String, VecDeque<Job>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn lanes() -> std::sync::MutexGuard<'static, HashMap<String, VecDeque<Job>>> {
    // A panicking job runs outside this lock, so poison only means a bug
    // in this module; keep the map either way.
    LANES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Queue `job` on `key`'s lane and return at once. It runs after every job
/// already queued on that lane. When the daemon has no runtime to start a
/// lane on (before boot), it runs on the caller's thread instead: a
/// delivery is never dropped.
pub(crate) fn run_later(key: &str, job: impl FnOnce() + Send + 'static) {
    let job: Job = Box::new(job);
    let mut map = lanes();
    if let Some(queue) = map.get_mut(key) {
        queue.push_back(job);
        return;
    }
    map.insert(key.to_string(), VecDeque::from([job]));
    let lane = key.to_string();
    match crate::rt::spawn("k2-delivery", move || drain(&lane)) {
        Ok(_) => {}
        Err(e) => {
            // Nothing else was queued (the lock is still held): run it here.
            let job = map.remove(key).and_then(|mut q| q.pop_front());
            drop(map);
            k2_core::log_debug!("[delivery] no lane thread for {key} ({e}); delivering inline");
            if let Some(job) = job {
                run_one(key, job);
            }
        }
    }
}

/// Queue `job` on `key`'s lane and wait (at most `max_wait`) for its
/// result. `None` when it did not finish in time (it still runs later) or
/// panicked.
pub(crate) fn run_in_order<T: Send + 'static>(
    key: &str,
    max_wait: Duration,
    job: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    run_later(key, move || {
        let _ = tx.send(job());
    });
    rx.recv_timeout(max_wait).ok()
}

/// How long a caller waits for its own delivery through [`run_in_order`]:
/// longer than one delivery's worst case (8 s inject lock + 20 s wake +
/// retries) plus a little queue ahead of it.
pub(crate) const WAIT_FOR_OWN_DELIVERY: Duration = Duration::from_secs(90);

fn drain(key: &str) {
    loop {
        let job = {
            let mut map = lanes();
            match map.get_mut(key).and_then(VecDeque::pop_front) {
                Some(job) => job,
                None => {
                    map.remove(key);
                    return;
                }
            }
        };
        run_one(key, job);
    }
}

fn run_one(key: &str, job: Job) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
        k2_core::log_debug!("[delivery] a delivery on lane {key} panicked; the lane goes on");
    }
}

/// Jobs still queued (not counting a running one) on `key`'s lane.
#[cfg(test)]
pub(crate) fn queued(key: &str) -> usize {
    lanes().get(key).map_or(0, VecDeque::len)
}

/// True while `key`'s lane has a thread (running or queued work).
#[cfg(test)]
pub(crate) fn busy(key: &str) -> bool {
    lanes().contains_key(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::Instant;

    fn key(label: &str) -> String {
        format!("test-lane-{label}-{}", uuid::Uuid::new_v4())
    }

    fn wait_idle(key: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while busy(key) {
            assert!(Instant::now() < deadline, "lane {key} never went idle");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn run_later_returns_before_a_slow_job_finishes() {
        let k = key("slow");
        let done = Arc::new(StdMutex::new(false));
        let d = done.clone();
        let t0 = Instant::now();
        run_later(&k, move || {
            std::thread::sleep(Duration::from_millis(800));
            *d.lock().expect("done lock") = true;
        });
        let took = t0.elapsed();
        assert!(took < Duration::from_millis(200), "queueing must not wait for the job: {took:?}");
        assert!(!*done.lock().expect("done lock"), "the job is still running when run_later returns");
        wait_idle(&k);
        assert!(*done.lock().expect("done lock"), "the job ran");
    }

    #[test]
    fn one_lane_runs_its_jobs_one_at_a_time_in_order() {
        let k = key("order");
        let seen = Arc::new(StdMutex::new(Vec::new()));
        for i in 0..6u32 {
            let s = seen.clone();
            run_later(&k, move || {
                // The first job is the slowest: a later one must still wait.
                std::thread::sleep(Duration::from_millis(if i == 0 { 300 } else { 20 }));
                s.lock().expect("seen lock").push(i);
            });
        }
        wait_idle(&k);
        assert_eq!(*seen.lock().expect("seen lock"), vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn lanes_for_different_agents_do_not_wait_on_each_other() {
        let (slow, fast) = (key("a"), key("b"));
        run_later(&slow, || std::thread::sleep(Duration::from_millis(1500)));
        let t0 = Instant::now();
        let got = run_in_order(&fast, Duration::from_secs(5), || 7);
        assert_eq!(got, Some(7));
        assert!(t0.elapsed() < Duration::from_millis(1000), "another agent's slow delivery held this one: {:?}", t0.elapsed());
        assert!(busy(&slow), "the slow lane is still working");
        wait_idle(&slow);
    }

    #[test]
    fn run_in_order_waits_behind_queued_work_and_returns_the_result() {
        let k = key("wait");
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let s = seen.clone();
        run_later(&k, move || {
            std::thread::sleep(Duration::from_millis(300));
            s.lock().expect("seen lock").push("queued first");
        });
        let s = seen.clone();
        let got = run_in_order(&k, Duration::from_secs(5), move || {
            s.lock().expect("seen lock").push("waited");
            "result"
        });
        assert_eq!(got, Some("result"));
        assert_eq!(*seen.lock().expect("seen lock"), vec!["queued first", "waited"]);
        wait_idle(&k);
    }

    #[test]
    fn run_in_order_gives_up_waiting_but_the_job_still_runs() {
        let k = key("late");
        let ran = Arc::new(StdMutex::new(false));
        let r = ran.clone();
        let got = run_in_order(&k, Duration::from_millis(100), move || {
            std::thread::sleep(Duration::from_millis(400));
            *r.lock().expect("ran lock") = true;
        });
        assert_eq!(got, None, "timed out");
        wait_idle(&k);
        assert!(*ran.lock().expect("ran lock"), "the job ran after the wait gave up");
    }

    #[test]
    fn a_panicking_job_does_not_stop_its_lane() {
        let k = key("panic");
        run_later(&k, || panic!("boom (expected by the test)"));
        let got = run_in_order(&k, Duration::from_secs(5), || "after");
        assert_eq!(got, Some("after"));
        wait_idle(&k);
        assert_eq!(queued(&k), 0);
    }
}
