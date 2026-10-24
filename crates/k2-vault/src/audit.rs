//! Audit: an append-only MAC chain in vault.db (§16.2, VS8). Never a value.
//!
//! High-volume uses (fetch, proxy, system resolves) roll up in memory per
//! (secret, consumer, host, action) and are appended as one row when the
//! window closes (hour change) or every 5 minutes. Rows are never updated.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::crypto::{hmac, Enc};
use crate::secret::Key32;

pub const ROLLUP_EVERY: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub secret_id: Option<String>,
    pub version: Option<i64>,
    pub action: String,
    pub actor: String,
    pub consumer: Option<String>,
    pub target_host: Option<String>,
    pub outcome: String,
    pub count: u64,
}

impl Entry {
    pub fn new(action: &str, actor: &str, outcome: &str) -> Entry {
        Entry {
            secret_id: None,
            version: None,
            action: action.into(),
            actor: actor.into(),
            consumer: None,
            target_host: None,
            outcome: outcome.into(),
            count: 1,
        }
    }

    pub fn secret(mut self, id: &str, version: Option<u32>) -> Entry {
        self.secret_id = Some(id.into());
        self.version = version.map(i64::from);
        self
    }

    pub fn consumer(mut self, c: impl Into<String>) -> Entry {
        self.consumer = Some(c.into());
        self
    }

    pub fn host(mut self, h: impl Into<String>) -> Entry {
        self.target_host = Some(h.into());
        self
    }
}

pub fn row_mac(key: &Key32, seq: u64, ts: &str, e: &Entry, prev: &[u8]) -> [u8; 32] {
    let mut enc = Enc::new("audit");
    enc.u64(seq)
        .str(ts)
        .opt_str(e.secret_id.as_deref())
        .opt_i64(e.version)
        .str(&e.action)
        .str(&e.actor)
        .opt_str(e.consumer.as_deref())
        .opt_str(e.target_host.as_deref())
        .str(&e.outcome)
        .u64(e.count)
        .bytes(prev);
    hmac(key, enc.finish())
}

#[derive(Hash, PartialEq, Eq, Clone)]
struct RollKey {
    secret_id: String,
    version: Option<i64>,
    action: String,
    actor: String,
    consumer: Option<String>,
    host: Option<String>,
    outcome: String,
}

pub struct Rollups {
    window_start: Instant,
    window_hour: i64,
    counts: HashMap<RollKey, u64>,
}

impl Default for Rollups {
    fn default() -> Self {
        Rollups { window_start: Instant::now(), window_hour: current_hour(), counts: HashMap::new() }
    }
}

fn current_hour() -> i64 {
    chrono::Utc::now().timestamp() / 3600
}

/// Distinct roll-up keys held in one window. Past it, a new key folds into
/// one coarse row per (action, outcome) with no secret, actor, consumer or
/// host, so a caller inventing hosts or consumers can't grow memory.
pub const ROLLUP_MAX_KEYS: usize = 4096;

impl Rollups {
    pub fn add(&mut self, e: &Entry) {
        let mut k = RollKey {
            secret_id: e.secret_id.clone().unwrap_or_default(),
            version: e.version,
            action: e.action.clone(),
            actor: e.actor.clone(),
            consumer: e.consumer.clone(),
            host: e.target_host.clone(),
            outcome: e.outcome.clone(),
        };
        if self.counts.len() >= ROLLUP_MAX_KEYS && !self.counts.contains_key(&k) {
            k = RollKey {
                secret_id: String::new(),
                version: None,
                action: e.action.clone(),
                actor: "rollup-overflow".into(),
                consumer: None,
                host: None,
                outcome: e.outcome.clone(),
            };
        }
        *self.counts.entry(k).or_insert(0) += e.count;
    }

    pub fn due(&self) -> bool {
        !self.counts.is_empty() && (self.window_start.elapsed() >= ROLLUP_EVERY || current_hour() != self.window_hour)
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// Drain into rows and start a new window.
    pub fn drain(&mut self) -> Vec<Entry> {
        let mut out: Vec<Entry> = self
            .counts
            .drain()
            .map(|(k, n)| Entry {
                secret_id: if k.secret_id.is_empty() { None } else { Some(k.secret_id) },
                version: k.version,
                action: k.action,
                actor: k.actor,
                consumer: k.consumer,
                target_host: k.host,
                outcome: k.outcome,
                count: n,
            })
            .collect();
        out.sort_by(|a, b| (&a.secret_id, &a.action, &a.consumer).cmp(&(&b.secret_id, &b.action, &b.consumer)));
        self.window_start = Instant::now();
        self.window_hour = current_hour();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollup_counts_and_drains() {
        let mut r = Rollups::default();
        let e = Entry::new("fetch", "agent:w", "ok").secret("s1", Some(1)).host("https://api.stripe.com:443");
        r.add(&e);
        r.add(&e);
        r.add(&Entry::new("fetch", "agent:w", "ok").secret("s2", Some(1)));
        assert!(!r.due(), "window not closed yet");
        let rows = r.drain();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.iter().find(|x| x.secret_id.as_deref() == Some("s1")).expect("s1").count, 2);
        assert!(r.is_empty());
    }

    #[test]
    fn rollup_keys_are_capped() {
        let mut r = Rollups::default();
        for i in 0..(ROLLUP_MAX_KEYS + 500) {
            r.add(&Entry::new("sign", "s", "deny:forwarded").secret("s1", Some(1)).host(format!("ssh:SHA256:{i}")));
        }
        assert!(r.counts.len() <= ROLLUP_MAX_KEYS + 1, "{}", r.counts.len());
        let rows = r.drain();
        assert_eq!(rows.iter().map(|x| x.count).sum::<u64>(), (ROLLUP_MAX_KEYS + 500) as u64, "every hit still counted");
        assert!(rows.iter().any(|x| x.actor == "rollup-overflow" && x.count == 500));
    }

    #[test]
    fn mac_chains_prev() {
        let k = Key32::random();
        let e = Entry::new("create", "owner", "ok");
        let a = row_mac(&k, 1, "t", &e, &[0u8; 32]);
        let b = row_mac(&k, 1, "t", &e, &[1u8; 32]);
        assert_ne!(a, b);
    }
}
