//! Garden data budgets (contract §3.6; PRD §5.3, §6.3). Owner: B1.
//!
//! Token buckets, in memory: writes 60/s burst 120 per placement and per
//! agent per space; reads 60/s burst 120 per placement; saves 1,200 per
//! 10 min per placement; a transaction commit costs `ceil(ops / 20)` write
//! tokens plus its bytes against 4 MiB per minute; 30 grant changes per
//! minute per delegate. The runaway guard pauses a placement after 100
//! identical writes (same key and value hash) in 10 minutes
//! (`PauseReason::Writes`; Resume is person-only).

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use k2_core::garden_data::blobs::{UPLOADS_PER_MIN, UPLOAD_BYTES_PER_10_MIN};
use k2_core::garden_data::{
    DataError, COMMIT_BYTES_PER_MIN, GRANT_CHANGES_PER_MIN, READS_PER_SEC, RUNAWAY_SAME_WRITES, RUNAWAY_WINDOW_SECS,
    SAVES_PER_10_MIN, TX_OPS_PER_TOKEN, WRITES_PER_SEC, WRITE_BURST,
};

/// Who a budget belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BudgetKey {
    Placement { garden: String, placement: String },
    Agent { project_id: String, space: String },
    Delegate { project_id: String },
}

/// What is being spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spend {
    Read,
    Write,
    Save,
    /// A commit of this many ops and bytes.
    Commit { ops: usize, bytes: usize },
    GrantChange,
    /// A new blob upload (prd-garden-board-needs-v1 §3.3): 30 a minute.
    UploadStart,
    /// Uploaded bytes: 128 MiB per 10 minutes.
    UploadBytes { bytes: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Read,
    Write,
    Save,
    Bytes,
    Grant,
    Upload,
    UploadBytes,
}

impl Kind {
    /// (capacity, refill per second)
    fn shape(self) -> (f64, f64) {
        match self {
            Kind::Read => (f64::from(WRITE_BURST), f64::from(READS_PER_SEC)),
            Kind::Write => (f64::from(WRITE_BURST), f64::from(WRITES_PER_SEC)),
            Kind::Save => (f64::from(SAVES_PER_10_MIN), f64::from(SAVES_PER_10_MIN) / 600.0),
            Kind::Bytes => (COMMIT_BYTES_PER_MIN as f64, COMMIT_BYTES_PER_MIN as f64 / 60.0),
            Kind::Grant => (f64::from(GRANT_CHANGES_PER_MIN), f64::from(GRANT_CHANGES_PER_MIN) / 60.0),
            Kind::Upload => (f64::from(UPLOADS_PER_MIN), f64::from(UPLOADS_PER_MIN) / 60.0),
            Kind::UploadBytes => (UPLOAD_BYTES_PER_10_MIN as f64, UPLOAD_BYTES_PER_10_MIN as f64 / 600.0),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: f64,
}

/// The buckets, with an explicit clock (seconds) so tests can drive time.
#[derive(Default)]
pub struct Buckets {
    map: HashMap<(BudgetKey, Kind), Bucket>,
}

impl Buckets {
    fn take(&mut self, key: &BudgetKey, kind: Kind, cost: f64, now: f64) -> Result<(), DataError> {
        let (cap, rate) = kind.shape();
        let b = self.map.entry((key.clone(), kind)).or_insert(Bucket { tokens: cap, at: now });
        b.tokens = (b.tokens + (now - b.at).max(0.0) * rate).min(cap);
        b.at = now;
        // A single spend bigger than the bucket can never fit; let it through
        // from a full bucket (it drains it) so large commits aren't stuck.
        let cost = cost.min(cap);
        if b.tokens + 1e-9 >= cost {
            b.tokens -= cost;
            Ok(())
        } else {
            let secs = ((cost - b.tokens) / rate).ceil().max(1.0);
            Err(DataError::RateLimited { retry_after_secs: secs as u32 })
        }
    }

    /// Spend `what` for `key` at `now` (seconds), or `rate_limited`.
    pub fn spend(&mut self, key: &BudgetKey, what: Spend, now: f64) -> Result<(), DataError> {
        match what {
            Spend::Read => self.take(key, Kind::Read, 1.0, now),
            Spend::Write => self.take(key, Kind::Write, 1.0, now),
            Spend::Save => self.take(key, Kind::Save, 1.0, now),
            Spend::GrantChange => self.take(key, Kind::Grant, 1.0, now),
            Spend::UploadStart => self.take(key, Kind::Upload, 1.0, now),
            Spend::UploadBytes { bytes } => self.take(key, Kind::UploadBytes, bytes as f64, now),
            Spend::Commit { ops, bytes } => {
                let tokens = ops.div_ceil(TX_OPS_PER_TOKEN).max(1) as f64;
                // Check both before spending either.
                let mut probe = self.map.clone();
                let mut trial = Buckets { map: std::mem::take(&mut probe) };
                trial.take(key, Kind::Write, tokens, now)?;
                trial.take(key, Kind::Bytes, bytes as f64, now)?;
                self.map = trial.map;
                Ok(())
            }
        }
    }

    /// Forget buckets that have refilled completely (idle callers).
    pub fn prune(&mut self, now: f64) {
        self.map.retain(|(_, kind), b| {
            let (cap, rate) = kind.shape();
            b.tokens + (now - b.at).max(0.0) * rate < cap
        });
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

fn clock() -> f64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn buckets() -> &'static Mutex<Buckets> {
    static B: OnceLock<Mutex<Buckets>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(Buckets::default()))
}

/// Spend one unit (or a commit's cost), or answer `rate_limited` with
/// `retryAfterSecs`.
pub fn spend(key: &BudgetKey, what: Spend) -> Result<(), DataError> {
    buckets().lock().unwrap_or_else(|e| e.into_inner()).spend(key, what, clock())
}

/// Drop idle buckets and runaway windows (the minute ticker).
pub fn prune() {
    let now = clock();
    buckets().lock().unwrap_or_else(|e| e.into_inner()).prune(now);
    runaways().lock().unwrap_or_else(|e| e.into_inner()).prune(now);
}

// ── the runaway guard ─────────────────────────────────────────────────

/// Identical writes per placement: `(key, value hash)` → write times.
#[derive(Default)]
pub struct Runaway {
    map: HashMap<(String, String, String, String), VecDeque<f64>>,
}

impl Runaway {
    /// Record one write; true when this placement must now pause.
    pub fn record(&mut self, garden: &str, placement: &str, key: &str, value_hash: &str, now: f64) -> bool {
        let q = self
            .map
            .entry((garden.to_string(), placement.to_string(), key.to_string(), value_hash.to_string()))
            .or_default();
        let window = RUNAWAY_WINDOW_SECS as f64;
        while q.front().is_some_and(|t| now - *t > window) {
            q.pop_front();
        }
        q.push_back(now);
        q.len() >= RUNAWAY_SAME_WRITES as usize
    }

    pub fn prune(&mut self, now: f64) {
        let window = RUNAWAY_WINDOW_SECS as f64;
        self.map.retain(|_, q| q.back().is_some_and(|t| now - *t <= window));
    }
}

fn runaways() -> &'static Mutex<Runaway> {
    static R: OnceLock<Mutex<Runaway>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Runaway::default()))
}

/// Record one widget write for the runaway guard. Answers true when the
/// placement must now be paused.
pub fn runaway(garden: &str, placement: &str, key: &str, value_hash: &str) -> bool {
    runaways().lock().unwrap_or_else(|e| e.into_inner()).record(garden, placement, key, value_hash, clock())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> BudgetKey {
        BudgetKey::Agent { project_id: "w-1".into(), space: "data:board".into() }
    }

    #[test]
    fn writes_allow_a_burst_of_120_then_60_per_second() {
        let mut b = Buckets::default();
        for _ in 0..WRITE_BURST {
            b.spend(&agent(), Spend::Write, 0.0).unwrap();
        }
        let e = b.spend(&agent(), Spend::Write, 0.0).unwrap_err();
        assert_eq!(e, DataError::RateLimited { retry_after_secs: 1 });
        for _ in 0..WRITES_PER_SEC {
            b.spend(&agent(), Spend::Write, 1.0).unwrap();
        }
        assert!(b.spend(&agent(), Spend::Write, 1.0).is_err());
        // Another agent has its own bucket.
        let other = BudgetKey::Agent { project_id: "w-2".into(), space: "data:board".into() };
        b.spend(&other, Spend::Write, 1.0).unwrap();
    }

    #[test]
    fn a_commit_costs_one_token_per_20_ops_and_its_bytes() {
        let mut b = Buckets::default();
        // 1,000 ops = 50 tokens of the 120 burst.
        b.spend(&agent(), Spend::Commit { ops: 1000, bytes: 1024 * 1024 }, 0.0).unwrap();
        b.spend(&agent(), Spend::Commit { ops: 1000, bytes: 1024 * 1024 }, 0.0).unwrap();
        // 100 tokens gone; a third 50-token commit doesn't fit.
        assert!(b.spend(&agent(), Spend::Commit { ops: 1000, bytes: 1 }, 0.0).is_err());
        // Bytes: 4 MiB a minute.
        let mut b = Buckets::default();
        for _ in 0..4 {
            b.spend(&agent(), Spend::Commit { ops: 1, bytes: 1024 * 1024 }, 0.0).unwrap();
        }
        let e = b.spend(&agent(), Spend::Commit { ops: 1, bytes: 1024 * 1024 }, 0.0).unwrap_err();
        assert_eq!(e.code(), "rate_limited");
        // A refused commit spent nothing: 119 single writes still fit.
        for _ in 0..(WRITE_BURST - 4) {
            b.spend(&agent(), Spend::Write, 0.0).unwrap();
        }
    }

    #[test]
    fn grant_changes_are_30_a_minute() {
        let mut b = Buckets::default();
        let d = BudgetKey::Delegate { project_id: "w-b".into() };
        for _ in 0..GRANT_CHANGES_PER_MIN {
            b.spend(&d, Spend::GrantChange, 0.0).unwrap();
        }
        assert!(b.spend(&d, Spend::GrantChange, 0.0).is_err());
        b.spend(&d, Spend::GrantChange, 2.0).unwrap();
    }

    #[test]
    fn prune_forgets_full_buckets() {
        let mut b = Buckets::default();
        b.spend(&agent(), Spend::Write, 0.0).unwrap();
        b.prune(0.0);
        assert_eq!(b.len(), 1);
        b.prune(10.0);
        assert!(b.is_empty());
    }

    #[test]
    fn runaway_pauses_after_100_identical_writes_in_10_minutes() {
        let mut r = Runaway::default();
        for i in 0..(RUNAWAY_SAME_WRITES - 1) {
            assert!(!r.record("g", "p", "k", "h", f64::from(i)));
        }
        assert!(!r.record("g", "p", "k", "other", 100.0), "a different value is a different write");
        assert!(r.record("g", "p", "k", "h", 100.0));
        let mut r = Runaway::default();
        for i in 0..RUNAWAY_SAME_WRITES {
            assert!(!r.record("g", "p", "k", "h", f64::from(i) * 10.0), "spread over more than 10 minutes");
        }
    }
}
