//! The host-wide cut queue (the native engine Beta): which native doc's cut runs next, and when.
//!
//! A cut is background work that competes with editing, so the host runs at most one at a
//! time, only in idle windows and within a time budget. The queue decides; it never cuts. Its
//! owner (the [`super::SessionManager`], one per process) asks [`CutQueue::next`] for a job,
//! runs it, and tells the queue how long it took and how it ended ([`CutQueue::ran`]).
//!
//! - **Entries:** one per doc (a second enqueue updates it), each with why it is due
//!   ([`CutWhy`]), its tail (changes since its last cut) and when it was first queued.
//! - **Order:** forced entries first, then the largest tail, then the oldest.
//! - **One at a time:** while a job handed out hasn't been reported, no other is.
//! - **Idle:** an entry that isn't forced runs only after `idle_ms` with no native commit.
//! - **The duty:** after a cut that took `d` ms, no cut starts for `d * (100 / duty_pct - 1)`
//!   ms ([`CutQueueSettings::duty_gap_ms`]), forced ones included.
//! - **Refused:** a refused cut stays queued, retried after the duty gap.
//!
//! Time comes in as plain milliseconds from the host's clock, and a cut's wall time is the
//! caller's measurement: the queue reads no clock. It runs native cuts once the native engine's
//! single-replica mode is vendored; until then nothing queues.

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::HashMap;
use std::hash::Hash;

use super::config::CutQueueSettings;
use crate::beta_plant::planted;

/// Why a doc is due a cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CutWhy {
    /// Its changes since the last cut reached its cadence.
    Cadence,
    /// A clean unload: the doc is leaving memory, its cut still due.
    Unload,
    /// Forced: twice its cadence, or its bytes past the hard trigger. It runs while editing goes
    /// on (it needs no idle window), still one at a time and within the duty.
    Forced,
}

impl CutWhy {
    /// A second enqueue keeps the stronger reason: forced over unload over cadence.
    fn rank(self) -> u8 {
        match self {
            CutWhy::Cadence => 0,
            CutWhy::Unload => 1,
            CutWhy::Forced => 2,
        }
    }
}

/// How a cut handed out by [`CutQueue::next`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CutOutcome {
    /// The cut was made: the entry leaves the queue.
    Done,
    /// The cut was refused (pending records, another live instance): the entry stays, retried
    /// after the duty gap.
    Refused,
}

/// What a cut cost, for the host's report ([`CutQueue::ran`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CutRan<K> {
    /// The doc.
    pub(crate) key: K,
    /// Its measured wall time, ms.
    pub(crate) ms: u64,
    /// Its tail when it was handed out: changes since its last cut.
    pub(crate) tail: u64,
    /// Why it was due.
    pub(crate) why: CutWhy,
    /// How it ended.
    pub(crate) outcome: CutOutcome,
    /// Over [`CutQueueSettings::slow_ms`].
    pub(crate) slow: bool,
}

/// A queued doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    why: CutWhy,
    tail: u64,
    queued_ms: u64,
    /// Enqueue order, the last tie-break.
    seq: u64,
}

/// The job handed out and not yet reported.
#[derive(Debug, Clone)]
struct Running<K> {
    key: K,
    started_ms: u64,
    why: CutWhy,
    tail: u64,
}

/// The host-wide cut queue (module docs). `K` is the doc's key.
#[derive(Debug)]
pub(crate) struct CutQueue<K> {
    settings: CutQueueSettings,
    entries: HashMap<K, Entry>,
    seq: u64,
    running: Option<Running<K>>,
    /// No cut starts before this (the duty gap after the last measured cut).
    gap_until_ms: u64,
}

impl<K: Clone + Eq + Hash> CutQueue<K> {
    pub(crate) fn new(settings: CutQueueSettings) -> Self {
        Self {
            settings,
            entries: HashMap::new(),
            seq: 0,
            running: None,
            gap_until_ms: 0,
        }
    }

    /// Queue `key`, due for `why` with `tail` changes since its last cut, at `now_ms`. Already
    /// queued: its entry is updated (the newer tail, the stronger reason, the first queued time
    /// kept), never a second entry.
    pub(crate) fn enqueue(&mut self, key: K, why: CutWhy, tail: u64, now_ms: u64) {
        if let Some(e) = self.entries.get_mut(&key) {
            e.tail = tail;
            if why.rank() > e.why.rank() {
                e.why = why;
            }
            return;
        }
        self.seq += 1;
        self.entries.insert(
            key,
            Entry {
                why,
                tail,
                queued_ms: now_ms,
                seq: self.seq,
            },
        );
    }

    /// The next doc to cut at `now_ms`, the host's last native commit at
    /// `last_native_commit_ms`, or `None`: at most one job per call, and none while a job
    /// handed out is unreported, inside the duty gap, or (unless the entry is forced) before
    /// the host is idle. The caller runs it and reports it with [`CutQueue::ran`].
    pub(crate) fn next(&mut self, now_ms: u64, last_native_commit_ms: u64) -> Option<K> {
        // the plant: a second job handed out while the first runs
        if self.running.is_some() && !planted("queue-runs-two") {
            return None;
        }
        // the plant: the duty gap ignored
        if now_ms < self.gap_until_ms && !planted("duty-ignored") {
            return None;
        }
        // the plant: every entry runs as if idle
        let idle = now_ms.saturating_sub(last_native_commit_ms) >= self.settings.idle_ms()
            || planted("idle-ignored");
        let running = self.running.as_ref().map(|r| &r.key);
        let (key, e) = self
            .entries
            .iter()
            .filter(|(k, e)| {
                // the plant: a forced entry waits for idle like any other
                Some(*k) != running
                    && (idle || (e.why == CutWhy::Forced && !planted("forced-waits-for-idle")))
            })
            .min_by_key(|(_, e)| {
                (
                    std::cmp::Reverse(e.why == CutWhy::Forced),
                    std::cmp::Reverse(e.tail),
                    e.queued_ms,
                    e.seq,
                )
            })?;
        let key = key.clone();
        self.running = Some(Running {
            key: key.clone(),
            started_ms: now_ms,
            why: e.why,
            tail: e.tail,
        });
        Some(key)
    }

    /// The job [`CutQueue::next`] handed out for `key` ended after `measured_ms` (its wall time,
    /// the caller's measurement), with `outcome`: the duty gap starts at its end
    /// ([`CutQueueSettings::duty_gap_ms`]); done, its entry leaves the queue; refused, it stays.
    /// Returns its report, or `None` (nothing changed) when `key` isn't the job handed out.
    pub(crate) fn ran(
        &mut self,
        key: K,
        measured_ms: u64,
        outcome: CutOutcome,
    ) -> Option<CutRan<K>> {
        if self.running.as_ref().map(|r| &r.key) != Some(&key) {
            return None;
        }
        let r = self.running.take()?;
        let end = r.started_ms.saturating_add(measured_ms);
        self.gap_until_ms = end.saturating_add(self.settings.duty_gap_ms(measured_ms));
        // the plant: a refused cut dropped from the queue
        if outcome == CutOutcome::Done || planted("refused-dropped") {
            self.entries.remove(&key);
        }
        Some(CutRan {
            key,
            ms: measured_ms,
            tail: r.tail,
            why: r.why,
            outcome,
            slow: measured_ms > self.settings.slow_ms(),
        })
    }

    /// How many docs are queued.
    pub(crate) fn queued(&self) -> usize {
        self.entries.len()
    }

    /// `key`'s entry: why it is due, its tail and when it was first queued.
    pub(crate) fn entry(&self, key: &K) -> Option<(CutWhy, u64, u64)> {
        self.entries.get(key).map(|e| (e.why, e.tail, e.queued_ms))
    }
}
