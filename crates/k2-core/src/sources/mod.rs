//! Garden v3 data sources (prd-garden-v3-external-data-cards-v1; build
//! contract `contract-sources-v1`).
//!
//! A source is a daemon-owned HTTP JSON check: the daemon polls it,
//! projects a few fields out of the response, rates each item ok / warn /
//! crit, keeps a short history, and wakes an agent on a hard change. Garden
//! cards and the CLI read the result; they never fetch on their own.
//!
//! This module is the pure half (types, specs, state maths, the card view,
//! the SQLite repos). The daemon half (`k2_daemon::sources`) owns the
//! poller, the network and the routes; it reaches the outside world only
//! through the traits below so the maths here is testable without it.
//!
//! Times are integer seconds since the Unix epoch (UTC); durations are
//! integer seconds. JSON is camelCase on the wire, snake_case in SQL.

use std::fmt;

pub mod extract;
pub mod note;
pub mod presets_cards;
pub mod spec;
pub mod state;
pub mod store;
pub mod trigger_store;
pub mod triggers;
pub mod try_tree;
pub mod view;

pub use extract::Projection;
pub use spec::{
    AgentAccess, Extract, FetchTargets, ItemsSpec, Keep, Maker, OnTargetError, PauseReason, Rule, SimulateState,
    SourceKind, SourceSpec, SourceStatus, SpecError, SpecWarning, Target, Validated, Viewers,
};
pub use state::{
    FetchOutcome, GapKind, Health, ItemState, Phase, PollApplied, PollInput, Reason, SourceState, Transition,
    TransitionKind,
};
pub use store::SourceRow;
pub use view::{SourceResult, Viewer};

// ── limits (contract §3.6) ───────────────────────────────────────────

pub const MAX_SOURCES: usize = 64;
pub const MAX_TARGETS_PER_SOURCE: usize = 50;
pub const MAX_TARGETS_PER_SERVER: usize = 500;
pub const MAX_CONCURRENT_FETCHES: usize = 8;
pub const MIN_EVERY_SECS: u32 = 30;
pub const DEFAULT_TIMEOUT_SECS: u32 = 10;
pub const FANOUT_TIMEOUT_SECS: u32 = 5;
pub const MAX_TIMEOUT_SECS: u32 = 30;
pub const DEFAULT_MAX_BYTES: u32 = 1 << 20;
pub const MAX_MAX_BYTES: u32 = 5 << 20;
pub const MAX_ITEMS: usize = 200;
pub const MAX_STRING: usize = 200;
pub const MAX_PROJECTION_BYTES: usize = 64 * 1024;
pub const MAX_TREND_SERIES: usize = 8;
pub const REFRESH_MIN_SECS: u32 = 15;
/// One request every 2 s per origin.
pub const ORIGIN_RATE_PER_SEC: f64 = 0.5;
pub const ORIGIN_BURST: u32 = 4;
pub const PARK_MAX_SECS: u32 = 3600;
/// `wall_jump::Detector` threshold.
pub const GAP_THRESHOLD_SECS: i64 = 120;
pub const TOMBSTONE_SECS: i64 = 7 * 86400;
pub const PERSON_MUTE_MAX_SECS: u32 = 86400;
pub const AGENT_MUTE_MAX_SECS: u32 = 7200;
pub const AGENT_SIMULATE_MAX_SECS: u32 = 3600;
pub const SIMULATE_DEFAULT_SECS: u32 = 3600;
pub const NOTE_MAX_BYTES: usize = 4096;
pub const AGENT_WAKES_PER_HOUR: u16 = 30;
pub const TEST_WAKES_PER_HOUR: u16 = 5;
/// Renderer constant, mirrored for docs.
pub const REMOTE_POLL_SECS: u32 = 20;
/// Renderer constant (this computer's daemon not counted).
pub const MAX_SOURCE_SERVERS_PER_GARDEN: usize = 8;

// ── names shared with the renderer and the Garden bridge ─────────────

/// The app-level event that says "a source changed" (contract §5).
pub const SOURCES_CHANGED_KIND: &str = "sources_changed";

/// The widget cap that lets a card read sources (contract §6).
pub const SOURCES_READ_CAP: &str = "sources:read";

/// The Garden bridge verbs (contract §6). The catalog rows are B3's.
pub const BRIDGE_VERBS: &[&str] =
    &["sources.list", "sources.read", "sources.history", "sources.subscribe", "sources.refresh", "sources.ask"];

/// The vault confirm actions sources use (`vault/confirm/start`).
pub const CONFIRM_ACTION_SECRET: &str = "source-secret";
pub const CONFIRM_ACTION_BATCH: &str = "source-batch";

// ── addresses ─────────────────────────────────────────────────────────

/// A source address as a widget or CLI names it: `name` or `name::host`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Address {
    pub name: String,
    pub host: Option<String>,
}

/// `[a-z0-9-]{1,40}`.
pub fn valid_name(name: &str) -> bool {
    (1..=40).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn valid_host(host: &str) -> bool {
    let (name, port) = match host.rsplit_once(':') {
        Some((n, p)) => (n, Some(p)),
        None => (host, None),
    };
    if let Some(p) = port {
        if !p.bytes().all(|b| b.is_ascii_digit()) || !p.parse::<u32>().is_ok_and(|v| (1..=65535).contains(&v)) {
            return false;
        }
    }
    !name.is_empty()
        && name.len() <= 253
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.contains("..")
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

impl Address {
    /// `name` must match `[a-z0-9-]{1,40}`; `host` is a DNS name or
    /// `host:port` (no scheme).
    pub fn parse(s: &str) -> Result<Address, SpecError> {
        let bad = |hint: &str| SpecError { code: "bad_name", field: "address".into(), hint: hint.into() };
        let (name, host) = match s.split_once("::") {
            Some((n, h)) => (n, Some(h)),
            None => (s, None),
        };
        if !valid_name(name) {
            return Err(bad("a source name is 1-40 characters: a-z, 0-9 and -"));
        }
        let host = match host {
            None => None,
            Some(h) => {
                let h = h.to_ascii_lowercase();
                if !valid_host(&h) {
                    return Err(bad("the part after :: is a server name like acme.k2.dev, with no scheme"));
                }
                Some(h)
            }
        };
        Ok(Address { name: name.to_string(), host })
    }
}

/// `fleet` or `fleet::acme.k2.dev`.
impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.host {
            Some(h) => write!(f, "{}::{}", self.name, h),
            None => f.write_str(&self.name),
        }
    }
}

// ── daemon boundaries (contract §3.4) ─────────────────────────────────

/// Runs one request. Never returns a body that wasn't redacted, never
/// panics on network errors.
pub trait Fetcher: Send + Sync {
    fn fetch(&self, plan: &FetchPlan) -> FetchOutcome;
}

/// Hands a trigger note to an agent.
pub trait Waker: Send + Sync {
    fn deliver(&self, req: &WakeRequest) -> WakeOutcome;
}

pub trait Clock: Send + Sync {
    fn wall_secs(&self) -> i64;
    fn mono(&self) -> std::time::Instant;
}

/// The daemon's impl emits `SessionEvent::SourcesChanged { rev }`.
pub trait ChangeSink: Send + Sync {
    fn changed(&self, rev: i64);
}

/// One request the poller (or Try) wants made.
#[derive(Debug, Clone)]
pub struct FetchPlan {
    /// None for Try.
    pub source_id: Option<String>,
    pub target_key: Option<String>,
    pub url: UrlRef,
    /// `scheme://host:port`; also the allowlist entry.
    pub origin: String,
    /// `sources.lan = 1` (an Owner typed that exact IP).
    pub lan_exact_ip: bool,
    /// Spec headers plus If-None-Match / If-Modified-Since.
    pub headers: Vec<(String, String)>,
    /// Vault secret name (resolved by the daemon).
    pub secret: Option<String>,
    pub timeout_secs: u32,
    pub max_bytes: u32,
}

#[derive(Debug, Clone)]
pub enum UrlRef {
    Plain(String),
    Secret { name: String },
}

pub struct WakeRequest {
    pub project_id: String,
    pub note: String,
    /// `source:<name>`.
    pub from: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeOutcome {
    Delivered { session_id: Option<String>, woke: bool },
    Inbox,
    Undeliverable { code: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_parses_bare_and_hosted_names() {
        assert_eq!(Address::parse("fleet").unwrap(), Address { name: "fleet".into(), host: None });
        let a = Address::parse("fleet::ACME.k2.dev").unwrap();
        assert_eq!(a.host.as_deref(), Some("acme.k2.dev"));
        assert_eq!(a.to_string(), "fleet::acme.k2.dev");
        assert_eq!(Address::parse("fleet::box.local:8443").unwrap().to_string(), "fleet::box.local:8443");
        for bad in ["", "Fleet", "a_b", "fleet::", "fleet::https://x", "fleet::a..b", "fleet::x:0", "fleet::x:99999"] {
            assert!(Address::parse(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(Address::parse(&"a".repeat(41)).is_err());
    }
}
