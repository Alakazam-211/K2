//! The write locks' liveness and bounds under random load, and a holder that unwinds
//! (`fair.rs`).
//!
//! - `the_locks_stay_live_and_bounded_under_random_load`: per seed, 16 slots in which
//!   participants come and go (each a new thread, in a random lane, for a random number of
//!   calls), on 1 or 4 session locks over one store turn, with the tick in the prompt lane;
//!   random hold times, zero included; calls nested as an edit's are (a session lock, then the
//!   store turn inside it), or on one lock alone. A watchdog fails the run (it stops the process)
//!   when no call completes for 30,000 polls of 1 ms; every call completes, each lock is held by
//!   one call at a time, prompt calls are overtaken by at most one agent call and agent calls by
//!   at most the bound (1, 4 or 128 by seed). Seeds: `MPC_FAIR_STRESS_SEEDS` (default 6),
//!   from `MPC_FAIR_STRESS_FROM` (default 0).
//! - `a_holder_unwinding_poisons_the_lock_as_std_does_and_leaves_the_queue_clear`: callers
//!   after a holder that unwinds are told the lock is poisoned, as with std's mutex, and none is
//!   left waiting, due or in the lane.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use crate::fair::probe::{self, Lock};
use crate::fair::{held_for_tests, in_prompt_lane, FairMutex, Lane, Which};

/// A small deterministic generator (xorshift).
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    pub(crate) fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// A watchdog on the calling thread: returns once `done` reaches `target`; when no call has
/// completed for 30,000 polls of 1 ms, prints `dump` and stops the process, so a lock that
/// stops letting calls in fails the row instead of hanging the run.
pub(crate) fn watch_progress(done: &AtomicU64, target: u64, dump: impl Fn() -> String) {
    const STILL_POLLS: u32 = 30_000;
    let (mut last, mut still) = (u64::MAX, 0u32);
    loop {
        let now = done.load(Ordering::SeqCst);
        if now >= target {
            return;
        }
        if now == last {
            still += 1;
            if still >= STILL_POLLS {
                eprintln!(
                    "row: no call completed in {STILL_POLLS} polls ({now} of {target} done): {}",
                    dump()
                );
                std::process::abort();
            }
        } else {
            (last, still) = (now, 0);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Hold a lock for a random while: no time at all, a few spins, or a long spin with a yield.
fn hold(r: &mut Rng) {
    match r.below(4) {
        0 => {}
        1 => (0..r.below(64)).for_each(|_| std::hint::spin_loop()),
        2 => (0..r.below(4_000)).for_each(|_| std::hint::spin_loop()),
        _ => std::thread::yield_now(),
    }
}

/// One seed's locks: the session locks, the store turn, and a flag per lock that a holder
/// sets while inside (each lock is held by one call at a time).
struct Locks {
    sessions: Vec<FairMutex<u64>>,
    store: FairMutex<u64>,
    inside: Vec<AtomicBool>,
    bound: u32,
    /// Lock-free reports of what went wrong (asserted after the run).
    bad: Mutex<Vec<String>>,
}

impl Locks {
    fn report(&self, what: String) {
        let mut b = self.bad.lock().unwrap();
        if b.len() < 20 {
            b.push(what);
        }
    }

    /// Hold lock `i` (a session lock, or `sessions.len()` for the store) as one call does.
    fn enter(&self, i: usize) {
        if self.inside[i].swap(true, Ordering::SeqCst) {
            self.report(format!("lock {i} held by two calls at once"));
        }
    }

    fn leave(&self, i: usize) {
        self.inside[i].store(false, Ordering::SeqCst);
    }

    /// One call: a session lock with the store turn inside it (as an edit), a session lock
    /// alone, or the store turn alone; each checked for its overtakes after.
    fn call(&self, r: &mut Rng, lane: Lane) {
        let store_i = self.sessions.len();
        probe::take(Lock::Session);
        probe::take(Lock::Store);
        let d = r.below(self.sessions.len() as u64) as usize;
        match r.below(10) {
            0..=6 => {
                let mut g = self.sessions[d].lock_in(lane).ok().expect("not poisoned");
                self.enter(d);
                *g += 1;
                hold(r);
                {
                    let mut s = self.store.lock().ok().expect("not poisoned");
                    self.enter(store_i);
                    *s += 1;
                    hold(r);
                    self.leave(store_i);
                }
                hold(r);
                self.leave(d);
            }
            7 | 8 => {
                let mut g = self.sessions[d].lock_in(lane).ok().expect("not poisoned");
                self.enter(d);
                *g += 1;
                hold(r);
                self.leave(d);
            }
            _ => {
                let mut s = self.store.lock_in(lane).ok().expect("not poisoned");
                self.enter(store_i);
                *s += 1;
                hold(r);
                self.leave(store_i);
            }
        }
        let (sw, _) = probe::take(Lock::Session);
        let (gw, _) = probe::take(Lock::Store);
        let prompt = lane == Lane::Prompt || crate::fair::current_lane() == Lane::Prompt;
        let max = if prompt { 1 } else { u64::from(self.bound) };
        if sw > max || gw > max {
            self.report(format!(
                "a {} call was overtaken by agent calls {sw} times at a session lock and {gw} at \
                 the store (bound {max})",
                if prompt { "prompt" } else { "agent" }
            ));
        }
    }
}

/// One seed of the stress row: what it saw.
fn stress(seed: u64) -> String {
    const SLOTS: usize = 16;
    const CALLS: u64 = 12_000;
    let mut rng = Rng::new(seed);
    let docs = [1usize, 4][(seed % 2) as usize];
    let bound = [1u32, 4, 128][((seed / 2) % 3) as usize];
    let locks = Locks {
        sessions: (0..docs)
            .map(|_| FairMutex::with_bound(0, Which::Session, bound))
            .collect(),
        store: FairMutex::with_bound(0, Which::Store, bound),
        inside: (0..=docs).map(|_| AtomicBool::new(false)).collect(),
        bound,
        bad: Mutex::new(Vec::new()),
    };
    let claimed = AtomicU64::new(0);
    let done = AtomicU64::new(0);
    let finished = AtomicBool::new(false);
    let joins = AtomicU64::new(0);
    let ticks = AtomicU64::new(0);
    std::thread::scope(|sc| {
        for _ in 0..SLOTS {
            let slot_seed = rng.next();
            let (locks, claimed, done, joins) = (&locks, &claimed, &done, &joins);
            sc.spawn(move || {
                let mut r = Rng::new(slot_seed);
                while claimed.load(Ordering::SeqCst) < CALLS {
                    // a participant joins: a thread of its own, a lane, a life of some calls
                    let life = 1 + r.below(200);
                    let lane = if r.below(4) == 0 {
                        Lane::Prompt
                    } else {
                        Lane::Agent
                    };
                    let mut pr = Rng::new(r.next());
                    joins.fetch_add(1, Ordering::SeqCst);
                    std::thread::scope(|inner| {
                        inner.spawn(move || {
                            for _ in 0..life {
                                if claimed.fetch_add(1, Ordering::SeqCst) >= CALLS {
                                    break;
                                }
                                locks.call(&mut pr, lane);
                                done.fetch_add(1, Ordering::SeqCst);
                            }
                            // it leaves holding nothing, back in the agents' lane
                            if held_for_tests() != 0 || crate::fair::current_lane() != Lane::Agent {
                                locks.report("a participant left with a lane or a lock".into());
                            }
                        });
                    });
                }
            });
        }
        {
            let (locks, finished, ticks) = (&locks, &finished, &ticks);
            let mut r = Rng::new(rng.next());
            sc.spawn(move || {
                in_prompt_lane(|| {
                    while !finished.load(Ordering::SeqCst) {
                        locks.call(&mut r, Lane::Agent);
                        ticks.fetch_add(1, Ordering::SeqCst);
                    }
                })
            });
        }
        watch_progress(&done, CALLS, || {
            let mut out: Vec<String> = locks
                .sessions
                .iter()
                .map(|s| format!("session {:?}", s.snapshot()))
                .collect();
            out.push(format!("store {:?}", locks.store.snapshot()));
            out.join("; ")
        });
        finished.store(true, Ordering::SeqCst);
    });
    let bad = locks.bad.into_inner().unwrap();
    assert!(
        bad.is_empty(),
        "row: seed {seed} (docs {docs}, bound {bound}): {bad:?}"
    );
    let clear = (false, 0, false, false, 0);
    for (i, s) in locks.sessions.iter().enumerate() {
        assert_eq!(
            s.snapshot(),
            clear,
            "row: seed {seed}: session lock {i} left with a holder, waiters or a hand-off"
        );
    }
    assert_eq!(
        locks.store.snapshot(),
        clear,
        "row: seed {seed}: the store turn left with a holder, waiters or a hand-off"
    );
    let calls: u64 = locks
        .sessions
        .iter()
        .map(|s| *s.lock().ok().expect("not poisoned"))
        .sum::<u64>()
        + *locks.store.lock().ok().expect("not poisoned");
    assert!(
        calls >= CALLS,
        "control: seed {seed}: every call held its locks ({calls} lock holds for {CALLS} calls)"
    );
    format!(
        "seed {seed} docs {docs} bound {bound}: {} calls, {} participants, {} tick calls, {calls} \
         lock holds",
        done.load(Ordering::SeqCst),
        joins.load(Ordering::SeqCst),
        ticks.load(Ordering::SeqCst)
    )
}

/// Many seeds of random load: every call completes, each lock is held by one call at a time,
/// and the bounds hold; the locks end clear.
#[test]
fn the_locks_stay_live_and_bounded_under_random_load() {
    let var = |k: &str, d: u64| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let (from, seeds) = (
        var("MPC_FAIR_STRESS_FROM", 0),
        var("MPC_FAIR_STRESS_SEEDS", 6),
    );
    for seed in from..from + seeds {
        eprintln!("STRESS {}", stress(seed));
    }
    eprintln!("STRESS done: {seeds} seeds from {from}");
}

/// A holder that unwinds while three agent calls and a prompt call wait: each waiter is told
/// the lock is poisoned, exactly as std's mutex tells its waiters, none is left waiting, due or
/// in the lane, and later callers in either lane are told at once.
#[test]
fn a_holder_unwinding_poisons_the_lock_as_std_does_and_leaves_the_queue_clear() {
    for prompt_at in 0..4 {
        let m = FairMutex::with_bound(0u64, Which::Session, 1);
        let s = std::sync::Mutex::new(0u64);
        let (m, s) = (&m, &s);
        let lanes: Vec<Lane> = (0..4)
            .map(|i| {
                if i == prompt_at {
                    Lane::Prompt
                } else {
                    Lane::Agent
                }
            })
            .collect();
        std::thread::scope(|sc| {
            let (held_tx, held_rx) = mpsc::channel();
            let (go_tx, go_rx) = mpsc::channel::<()>();
            let holder = sc.spawn(move || {
                let _g = m.lock_in(Lane::Agent).ok().expect("free");
                let _s = s.lock().unwrap();
                held_tx.send(()).unwrap();
                go_rx.recv().unwrap();
                panic!("the holder unwinds");
            });
            held_rx.recv().unwrap();
            let waiters: Vec<_> = lanes
                .iter()
                .map(|&lane| sc.spawn(move || (m.lock_in(lane).is_err(), s.lock().is_err())))
                .collect();
            let queued = AtomicU64::new(0);
            let target = lanes.len() as u64;
            // the waiters are queued before the holder goes
            std::thread::scope(|w| {
                w.spawn(|| {
                    while m.snapshot().1 < lanes.len() {
                        std::thread::yield_now();
                    }
                    queued.store(target, Ordering::SeqCst);
                });
                watch_progress(&queued, target, || format!("{:?}", m.snapshot()));
            });
            go_tx.send(()).unwrap();
            assert!(holder.join().is_err(), "control: the holder unwound");
            for (i, w) in waiters.into_iter().enumerate() {
                let (fair, std) = w.join().expect("a waiter returns");
                assert!(std, "control: std's mutex told waiter {i} it is poisoned");
                assert_eq!(
                    fair, std,
                    "row: waiter {i} (prompt at {prompt_at}) is told what std's mutex tells it"
                );
            }
        });
        let clear = (false, 0, false, false, 0);
        assert_eq!(
            m.snapshot(),
            clear,
            "row: after the waiters, the queue is clear (prompt at {prompt_at})"
        );
        assert!(
            m.lock_in(Lane::Prompt).is_err() && m.lock_in(Lane::Agent).is_err(),
            "row: later callers in either lane are told the lock is poisoned"
        );
        assert_eq!(m.snapshot(), clear, "row: and the queue stays clear");
    }
}
