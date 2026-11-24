//! The order of the kit's write locks: who waiting for one goes in next.
//!
//! Two locks are held across a change's durable commit: a doc's session lock (the whole edit)
//! and the store's writer turn (one write transaction at a time, across every doc in the
//! store). Left to `std::sync::Mutex`, or to the store library's own writer lock (a mutex and a
//! condition variable), whoever takes a lock first once it is free goes in: a writer back to
//! back takes it again before a woken waiter runs, and a person, the tick or another agent can
//! be passed over without bound. A strict queue fixes that but makes every waiter wait a full
//! round, and each hand-off wakes a sleeping thread instead of letting the running one go on
//! (at saturation it about halved the agents' edits a second).
//!
//! [`FairMutex`] keeps the cheap path and bounds the waits:
//!
//! - **Free for all, while nobody is due.** A caller takes a free lock at once, the running
//!   thread included; one waiter at a time is woken to compete.
//! - **A bound per waiter.** Each later arrival let in ahead of a waiting call counts as one
//!   overtake. A waiter overtaken [`OVERTAKES_MAX`] times is due: no one else may take the
//!   lock, and the next release hands it to the waiter directly. Due waiters go in arrival
//!   order, so no waiter is overtaken more than the bound plus the prompt lane's turns.
//! - **The prompt lane.** A person's call and the manager's tick ([`Lane::Prompt`]) are due
//!   from the moment they wait: the next release hands the lock to the first of them, ahead of
//!   agents. Such a call waits behind the call holding the lock and the other prompt callers
//!   ahead of it, never behind an agent arriving after it. The lane is the calling thread's
//!   while it holds a prompt lock, so the store's writer turn taken inside a person's edit (or
//!   the tick) is in the lane too.
//! - **Passed down to the holder.** A prompt call waiting for a doc's session lock waits for
//!   the agent holding it, and that agent may itself be waiting for the store's writer turn
//!   behind other docs' agents. While a prompt call waits for a session lock, the holder's
//!   store write goes in the prompt lane, so the prompt call waits for one agent edit, not for
//!   that edit and a queue at the store.
//!
//! No clock: the bound is in calls let in, so the order is the same on every machine and in
//! every test. A time bound shorter than one round of agent calls would make every waiter due
//! at saturation (a strict queue again); a longer one never fires before the count does.
//!
//! Test builds: [`probe::Overtakes`] numbers each call on arrival (in the same step as it joins
//! the queue) and counts, at each admission of an agents'-lane call, the calls still waiting
//! that arrived before it, so a row can bound how often a waiter is passed over by agents
//! without reading a clock (`session/fair_lock_tests.rs`).
//! Plants (`MPC_BETA_PLANT`): `session-lock-barging` and `store-gate-barging` (that lock's
//! order skipped: whoever is first, as before), `fair-lane-skipped` (every call in the agents'
//! lane). Measurement modes (test builds, `MPC_FAIR_MODE`): `barging` (both locks as before),
//! `fifo` (a strict queue); `MPC_FAIR_OVERTAKES` sets the bound.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

/// How many later arrivals may be let in ahead of a waiting call before it is due. At
/// saturation every waiter's count rises together, so after this many free-for-all calls the
/// waiters go in turn, one hand-off each: about one call in `(waiters / (128 + waiters))` is a
/// hand-off (one in five with 31 agents), and an agent waits at most about 128 + 31 calls.
pub(crate) const OVERTAKES_MAX: u32 = 128;

/// Which lane a call waits in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Lane {
    /// Agents and everything not known to be a person's: bounded among themselves.
    Agent,
    /// A person's call, or the manager's tick: handed the lock ahead of agents.
    Prompt,
}

thread_local! {
    /// The calling thread's lane: raised to [`Lane::Prompt`] while it holds a prompt lock, or
    /// inside [`in_prompt_lane`].
    static LANE: Cell<Lane> = const { Cell::new(Lane::Agent) };
    /// The prompt-waiter counts of the session locks this thread holds: while one is above
    /// zero, the thread's store writes go in the prompt lane.
    static HELD: RefCell<Vec<Arc<AtomicUsize>>> = const { RefCell::new(Vec::new()) };
}

/// The calling thread's lane now.
pub(crate) fn current_lane() -> Lane {
    LANE.with(Cell::get)
}

/// Test builds: how many session locks the calling thread holds (their prompt-waiter counts).
#[cfg(test)]
pub(crate) fn held_for_tests() -> usize {
    HELD.with(|h| h.borrow().len())
}

/// Run `f` in the prompt lane (the manager's tick): every write lock it takes is handed to it
/// ahead of agents.
pub(crate) fn in_prompt_lane<R>(f: impl FnOnce() -> R) -> R {
    let _lane = LaneRaised::to(Lane::Prompt);
    f()
}

/// The thread's lane raised for a scope; put back when it drops.
struct LaneRaised(Option<Lane>);

impl LaneRaised {
    fn to(lane: Lane) -> Self {
        match lane {
            Lane::Prompt => LaneRaised(Some(LANE.with(|l| l.replace(Lane::Prompt)))),
            Lane::Agent => LaneRaised(None),
        }
    }
}

impl Drop for LaneRaised {
    fn drop(&mut self) {
        if let Some(prev) = self.0 {
            LANE.with(|l| l.set(prev));
        }
    }
}

/// Which of the kit's write locks this is (its plant and its probe).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Which {
    Session,
    Store,
}

/// How a lock lets waiters in (test builds choose; a build always uses `Bounded`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Free for all, with the per-waiter bound and the prompt lane.
    Bounded { max: u32, lanes: bool },
    /// Whoever takes it first (the plants, and the `barging` measurement).
    #[cfg_attr(not(test), allow(dead_code))]
    Barging,
}

#[cfg(test)]
fn mode(which: Which) -> Mode {
    use std::sync::OnceLock;
    static MEASURE: OnceLock<(Option<String>, Option<u32>)> = OnceLock::new();
    let (m, n) = MEASURE.get_or_init(|| {
        (
            std::env::var("MPC_FAIR_MODE").ok(),
            std::env::var("MPC_FAIR_OVERTAKES")
                .ok()
                .and_then(|v| v.parse().ok()),
        )
    });
    let plant = match which {
        Which::Session => "session-lock-barging",
        Which::Store => "store-gate-barging",
    };
    if m.as_deref() == Some("barging") || crate::beta_plant::planted(plant) {
        return Mode::Barging;
    }
    if m.as_deref() == Some("fifo") {
        return Mode::Bounded {
            max: 0,
            lanes: false,
        };
    }
    Mode::Bounded {
        max: n.unwrap_or(OVERTAKES_MAX),
        lanes: !crate::beta_plant::planted("fair-lane-skipped"),
    }
}

#[cfg(not(test))]
#[inline]
fn mode(_: Which) -> Mode {
    Mode::Bounded {
        max: OVERTAKES_MAX,
        lanes: true,
    }
}

/// The bound in effect (test builds: `MPC_FAIR_OVERTAKES`, else [`OVERTAKES_MAX`]).
#[cfg(test)]
pub(crate) fn overtakes_max() -> u32 {
    match mode(Which::Session) {
        Mode::Bounded { max, .. } => max,
        Mode::Barging => OVERTAKES_MAX,
    }
}

/// A mutex with a bounded wait for every caller and a prompt lane, over a `std::sync::Mutex`
/// that keeps the data and its poisoning: a holder that unwinds poisons it as before, and the
/// next caller is told.
pub(crate) struct FairMutex<T> {
    which: Which,
    /// How many prompt calls wait for this lock now (read by the holder's store writes).
    prompt_waiting: Arc<AtomicUsize>,
    queue: Mutex<Queue>,
    data: Mutex<T>,
    #[cfg(test)]
    probe: probe::Overtakes,
    /// Test builds: a bound of this lock's own (the stress rows), else the mode's.
    #[cfg(test)]
    bound: Option<u32>,
}

/// Who holds the lock and who waits for it (in arrival order).
struct Queue {
    held: bool,
    next: u64,
    waiting: VecDeque<Waiter>,
    /// The waiter the lock was handed to, until it runs.
    handed: Option<u64>,
    /// Whether that waiter was handed it in the prompt lane (its own or passed down).
    handed_prompt: bool,
    /// The waiter woken to compete for a free lock, until it runs.
    woken: Option<u64>,
}

struct Waiter {
    arrival: u64,
    lane: Lane,
    overtaken: u32,
    wake: Arc<Condvar>,
    /// A store write's: the prompt-waiter counts of the session locks its thread holds.
    boost: Vec<Arc<AtomicUsize>>,
}

impl Waiter {
    /// In the prompt lane: its own, or passed down from a prompt call waiting for a session
    /// lock its thread holds.
    fn prompt(&self) -> bool {
        self.lane == Lane::Prompt || self.boost.iter().any(|b| b.load(Ordering::Relaxed) > 0)
    }
}

impl Queue {
    /// The waiter a release hands the lock to: the first prompt waiter, else the first waiter
    /// overtaken `max` times.
    fn due(&self, max: u32, lanes: bool) -> Option<usize> {
        if lanes {
            if let Some(i) = self.waiting.iter().position(Waiter::prompt) {
                return Some(i);
            }
        }
        self.waiting.iter().position(|w| w.overtaken >= max)
    }

    /// Whether the call that arrived as `me` (a waiter, or `None` for a newcomer) may take the
    /// lock now: it is free, not handed, and no other waiter is due.
    fn may_take(&self, me: Option<u64>, max: u32, lanes: bool) -> bool {
        if self.held || self.handed.is_some() {
            return false;
        }
        match self.due(max, lanes) {
            None => true,
            Some(i) => Some(self.waiting[i].arrival) == me,
        }
    }

    /// The call that arrived as `a` takes the lock: every waiter that arrived before it is
    /// overtaken once more.
    fn take(&mut self, a: u64) {
        self.held = true;
        for w in self.waiting.iter_mut().filter(|w| w.arrival < a) {
            w.overtaken = w.overtaken.saturating_add(1);
        }
    }

    fn remove(&mut self, a: u64) -> Option<Waiter> {
        let i = self.waiting.iter().position(|w| w.arrival == a)?;
        self.waiting.remove(i)
    }

    /// A free lock that the call that arrived as `me` (if any) is not taking: handed to the
    /// waiter that is due, if one is, else the first other waiter is woken to compete. Called
    /// on every release and by every caller that finds the lock free but doesn't take it, so a
    /// free lock never sits beside a waiter that is due (a waiter can become due while the lock
    /// is free: a prompt call starts waiting for a session lock its thread holds).
    fn settle(&mut self, me: Option<u64>, max: u32, lanes: bool) {
        if self.held || self.handed.is_some() {
            return;
        }
        if let Some(i) = self.due(max, lanes) {
            if let Some(w) = self.waiting.remove(i) {
                self.take(w.arrival);
                self.handed = Some(w.arrival);
                self.handed_prompt = lanes && w.prompt();
                w.wake.notify_one();
            }
            return;
        }
        if self.woken.is_none() {
            let first = self.waiting.iter().find(|w| Some(w.arrival) != me);
            if let Some((a, wake)) = first.map(|w| (w.arrival, w.wake.clone())) {
                self.woken = Some(a);
                wake.notify_one();
            }
        }
    }
}

/// The data's guard; letting go hands the lock on.
pub(crate) struct FairGuard<'a, T> {
    lock: &'a FairMutex<T>,
    data: Option<MutexGuard<'a, T>>,
    /// This guard holds the queue's lock (false when the order is skipped: a plant, or the
    /// `barging` measurement).
    queued: bool,
    /// The thread's lane, raised while a prompt call holds the lock.
    _lane: LaneRaised,
}

/// The lock was poisoned: a holder unwound while holding it. It holds the lock and the data
/// (as std's `PoisonError` holds its guard); dropped, the lock passes on.
pub(crate) struct Poisoned<'a, T>(FairGuard<'a, T>);

impl<'a, T> Poisoned<'a, T> {
    /// The guard, for a caller whose data can't be left half-changed.
    pub(crate) fn into_guard(self) -> FairGuard<'a, T> {
        self.0
    }
}

impl<T> FairMutex<T> {
    /// A doc's session lock.
    pub(crate) fn session(data: T) -> Self {
        Self::new(data, Which::Session)
    }

    /// The store's writer turn.
    pub(crate) fn store(data: T) -> Self {
        Self::new(data, Which::Store)
    }

    /// Test builds: a lock with its own bound (the stress rows).
    #[cfg(test)]
    pub(crate) fn with_bound(data: T, which: Which, bound: u32) -> Self {
        Self {
            bound: Some(bound),
            ..Self::new(data, which)
        }
    }

    fn new(data: T, which: Which) -> Self {
        Self {
            which,
            prompt_waiting: Arc::default(),
            queue: Mutex::new(Queue {
                held: false,
                next: 0,
                waiting: VecDeque::new(),
                handed: None,
                handed_prompt: false,
                woken: None,
            }),
            data: Mutex::new(data),
            #[cfg(test)]
            probe: probe::Overtakes::new(match which {
                Which::Session => probe::Lock::Session,
                Which::Store => probe::Lock::Store,
            }),
            #[cfg(test)]
            bound: None,
        }
    }

    /// How this lock lets waiters in: `None` when the order is skipped (whoever is first),
    /// else the bound and whether the prompt lane is on.
    fn order(&self) -> Option<(u32, bool)> {
        match mode(self.which) {
            Mode::Barging => None,
            Mode::Bounded { max, lanes } => {
                #[cfg(test)]
                let max = self.bound.unwrap_or(max);
                Some((max, lanes))
            }
        }
    }

    /// Test builds: the queue as it stands (held, waiting, handed, woken, prompt waiters).
    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> (bool, usize, bool, bool, usize) {
        let q = self.queue();
        (
            q.held,
            q.waiting.len(),
            q.handed.is_some(),
            q.woken.is_some(),
            self.prompt_waiting.load(Ordering::Relaxed),
        )
    }

    fn queue(&self) -> MutexGuard<'_, Queue> {
        // held only for a few lines of bookkeeping here, which can't stop part-way
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Take the lock in the calling thread's lane ([`current_lane`]).
    pub(crate) fn lock(&self) -> Result<FairGuard<'_, T>, Poisoned<'_, T>> {
        self.lock_in(current_lane())
    }

    /// Take the lock in `lane` (or the calling thread's, if that is the prompt lane). A
    /// poisoned lock still passes on when its guard drops.
    pub(crate) fn lock_in(&self, lane: Lane) -> Result<FairGuard<'_, T>, Poisoned<'_, T>> {
        let lane = match (lane, current_lane()) {
            (Lane::Agent, Lane::Agent) => Lane::Agent,
            _ => Lane::Prompt,
        };
        let Some((max, lanes)) = self.order() else {
            #[cfg(test)]
            let _arrival = self.probe.arrive(lane);
            return self.guard(false, lane);
        };
        let lane = if lanes { lane } else { Lane::Agent };
        let mut q = self.queue();
        let me = q.next;
        q.next += 1;
        // test builds: numbered in the same step as it joins the queue; counted as let in
        // after the lock is held, in the lane it was let in by
        #[cfg(test)]
        let mut arrival = self.probe.arrive(lane);
        if q.may_take(None, max, lanes) {
            q.take(me);
        } else {
            let wake = Arc::new(Condvar::new());
            let boost = match (self.which, lanes) {
                (Which::Store, true) => HELD.with(|h| h.borrow().clone()),
                _ => Vec::new(),
            };
            let counted = lane == Lane::Prompt;
            if counted {
                self.prompt_waiting.fetch_add(1, Ordering::Relaxed);
            }
            q.waiting.push_back(Waiter {
                arrival: me,
                lane,
                overtaken: 0,
                wake: wake.clone(),
                boost,
            });
            // every pass holds the queue from its checks to its wait, and acts on the queue's
            // state, never on a wake-up alone (a wake-up may come early, late or twice)
            let mut wait_first = false;
            loop {
                if wait_first {
                    q = wake.wait(q).unwrap_or_else(PoisonError::into_inner);
                }
                if q.handed == Some(me) {
                    // handed on: already out of the queue, its overtakes counted
                    q.handed = None;
                    #[cfg(test)]
                    arrival.let_in_as(if q.handed_prompt { Lane::Prompt } else { lane });
                    if q.woken == Some(me) {
                        q.woken = None;
                    }
                    break;
                }
                if q.woken == Some(me) {
                    q.woken = None;
                }
                if q.may_take(Some(me), max, lanes) {
                    let left = q.remove(me);
                    #[cfg(test)]
                    arrival.let_in_as(match lanes && left.as_ref().is_some_and(Waiter::prompt) {
                        true => Lane::Prompt,
                        false => lane,
                    });
                    drop(left);
                    q.take(me);
                    break;
                }
                // not taking it now: a free lock goes on to whoever is due (or a waiter wakes).
                // That may be this call itself (whether a waiter is due can change between two
                // looks: a prompt-waiter count passed down moves on its own), so the next pass
                // looks at the queue again before it waits
                q.settle(Some(me), max, lanes);
                wait_first = q.handed != Some(me);
            }
            if counted {
                self.prompt_waiting.fetch_sub(1, Ordering::Relaxed);
            }
        }
        drop(q);
        let g = self.guard(true, lane);
        #[cfg(test)]
        drop(arrival);
        g
    }

    fn guard(&self, queued: bool, lane: Lane) -> Result<FairGuard<'_, T>, Poisoned<'_, T>> {
        let (data, poisoned) = match self.data.lock() {
            Ok(g) => (g, false),
            Err(e) => (e.into_inner(), true),
        };
        if let Which::Session = self.which {
            HELD.with(|h| h.borrow_mut().push(self.prompt_waiting.clone()));
        }
        let g = FairGuard {
            lock: self,
            data: Some(data),
            queued,
            _lane: LaneRaised::to(lane),
        };
        match poisoned {
            false => Ok(g),
            true => Err(Poisoned(g)),
        }
    }

    /// The lock is let go: handed to the waiter that is due, if one is, else free for all
    /// with the first waiter woken to compete.
    fn release(&self) {
        let Some((max, lanes)) = self.order() else {
            return;
        };
        let mut q = self.queue();
        q.held = false;
        q.settle(None, max, lanes);
    }
}

impl<T> Drop for FairGuard<'_, T> {
    fn drop(&mut self) {
        // the data first (an unwinding holder poisons it here), then the lock passes on
        drop(self.data.take());
        if let Which::Session = self.lock.which {
            HELD.with(|h| {
                let mut h = h.borrow_mut();
                if let Some(i) = h
                    .iter()
                    .rposition(|b| Arc::ptr_eq(b, &self.lock.prompt_waiting))
                {
                    h.remove(i);
                }
            });
        }
        if self.queued {
            self.lock.release();
        }
    }
}

impl<T> Deref for FairGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.data.as_deref().expect("held until drop")
    }
}

impl<T> DerefMut for FairGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.data.as_deref_mut().expect("held until drop")
    }
}

#[cfg(test)]
pub(crate) mod probe {
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::sync::{Mutex, PoisonError};

    use super::Lane;

    /// Which lock a probe counts for.
    #[derive(Clone, Copy, Debug)]
    pub(crate) enum Lock {
        /// A doc's session lock.
        Session = 0,
        /// The store's writer turn.
        Store = 1,
    }

    /// The calls waiting for one lock, by arrival number, each with the times an agents'-lane
    /// call has overtaken it so far.
    pub(crate) struct Overtakes {
        lock: Lock,
        waiting: Mutex<Waiting>,
    }

    #[derive(Default)]
    struct Waiting {
        next: u64,
        waiting: BTreeMap<u64, u64>,
    }

    thread_local! {
        /// Per lock, the most times one of this thread's calls was overtaken since the last
        /// take.
        static WORST: [Cell<u64>; 2] = const { [Cell::new(0), Cell::new(0)] };
        /// Per lock, the times this thread's calls were overtaken in all since the last take.
        static TOTAL: [Cell<u64>; 2] = const { [Cell::new(0), Cell::new(0)] };
    }

    impl Overtakes {
        pub(crate) fn new(lock: Lock) -> Self {
            Self {
                lock,
                waiting: Mutex::default(),
            }
        }

        /// A call in `lane` arrives at the lock (before it waits): its arrival, which counts
        /// the call as let in when it drops (the caller keeps it until it holds the lock).
        pub(crate) fn arrive(&self, lane: Lane) -> Arrival<'_> {
            let mut w = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
            let a = w.next;
            w.next += 1;
            w.waiting.insert(a, 0);
            Arrival { of: self, a, lane }
        }

        /// The call that arrived as `a` is let in: if it is an agents'-lane call, every call
        /// still waiting that arrived before it is overtaken once more; the times `a` itself was
        /// overtaken go to this thread's worst and total for the lock.
        fn admitted(&self, a: u64, lane: Lane) {
            let mut w = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
            let mine = w.waiting.remove(&a).unwrap_or(0);
            if lane == Lane::Agent {
                for (_, n) in w.waiting.range_mut(..a) {
                    *n += 1;
                }
            }
            let i = self.lock as usize;
            WORST.with(|c| c[i].set(c[i].get().max(mine)));
            TOTAL.with(|c| c[i].set(c[i].get() + mine));
        }
    }

    /// A call's arrival at the lock ([`Overtakes::arrive`]).
    pub(crate) struct Arrival<'a> {
        of: &'a Overtakes,
        a: u64,
        lane: Lane,
    }

    impl Arrival<'_> {
        /// The call was let in in `lane` (a waiter in the agents' lane handed the lock in the
        /// prompt lane passed down to it counts as a prompt call).
        pub(crate) fn let_in_as(&mut self, lane: Lane) {
            self.lane = lane;
        }
    }

    impl Drop for Arrival<'_> {
        fn drop(&mut self) {
            self.of.admitted(self.a, self.lane);
        }
    }

    /// The most times one of this thread's calls to `lock` was overtaken since the last take,
    /// and the times they were overtaken in all; resets both.
    pub(crate) fn take(lock: Lock) -> (u64, u64) {
        let i = lock as usize;
        (
            WORST.with(|c| c[i].replace(0)),
            TOTAL.with(|c| c[i].replace(0)),
        )
    }
}
