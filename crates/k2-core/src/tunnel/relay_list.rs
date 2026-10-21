//! Relay list refresh (0.45.7) — the daemon keeps `tunnel.json` `relays`
//! current from k2.dev so every device can fail over.
//!
//! On 2026-10-15 relay k2e-01 was down ~3 h. Devices whose `tunnel.json`
//! listed both relays failed over by themselves; desktops and older boxes
//! had only `server_addr`, so [`TunnelConfig::relay_list`] was one element
//! and they stayed down. Now the daemon asks k2.dev for the list:
//!
//! `GET https://k2.dev/api/tunnel/relays` with `Authorization: Bearer
//! <tunnel token>` →
//! `{"v":1,"relays":[{"host":"…","port":7000,"role":"primary"},…],"refresh_s":21600,"ts":…}`.
//! Index 0 is the primary; the order is kept exactly as received. Served
//! from k2.dev (Vercel), not the relays, so it answers during a relay
//! outage. Any non-200 (401 bad/missing token, 503 lookup failed, …) keeps
//! the old list.
//!
//! **Live relay (0.46.3, k2-dev-web #20)** — additive under `v:1`: each
//! relay may carry `"active": true|false` (true = the `*.k2.dev` wildcard
//! points at that relay's host right now) plus a top-level
//! `"active_checked_at"`. When k2.dev's DNS check is unknown both are
//! omitted. Exactly one `true` among OUR listed relays is the only "known"
//! answer; omitted, all `false`, two `true` or a non-boolean flag all mean
//! unknown. The answer is kept in memory only (never in `tunnel.json`) and
//! read by the tunnel failover through [`dns_hint`], which never does I/O;
//! the failover asks for a fresh fetch via [`request_refresh`] at most
//! every [`DNS_HINT_REQUEST_EVERY`] (k2.dev caches its check for 15 s).
//! Unknown fields stay ignored (no `deny_unknown_fields`), so older
//! daemons read the new body unchanged.
//!
//! **When** — at daemon start (after the vault holder is installed, before
//! the tunnel auto-start), every `refresh_s` (default 6 h, ± 10 %), right
//! after any relay switch ([`request_refresh`] from the connector), and
//! right after a token change through `/cli/tunnel/config` or a subdomain
//! switch (pairing). Never with air-gap on, with the tunnel disabled or
//! released, or without a token. The loop is a plain daemon thread
//! ([`Refresher`]): a headless daemon does it with no client attached.
//!
//! **Token** — the same one the connector renders into frpc.toml:
//! [`config::load`] resolves it from the K2 vault when `tunnel.json`'s own
//! `token` field is empty (desktops since 0.45.4).
//!
//! **Persist** — `relays` (plus `server_addr`/`server_port` = the primary,
//! so the legacy pair stays coherent) are written only when they changed,
//! atomically and 0600 ([`config::update_if`]). A running tunnel picks the
//! new list up at its next reconnect, never as a live restart (see the
//! connector's supervisor).
//!
//! **Validation** — `v == 1`, 1..=[`MAX_RELAYS`] relays, each host a valid
//! IP or hostname, port 1–65535, no duplicates; `refresh_s` clamped to
//! [`MIN_REFRESH`]..=[`MAX_REFRESH`]. Anything else keeps the old list and
//! logs once (the same error is not logged again until it changes).

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::config::{self, RelayEndpoint, TunnelConfig};

/// The k2.dev relay-list endpoint (K2 Web owns it).
pub const DEFAULT_RELAY_LIST_URL: &str = "https://k2.dev/api/tunnel/relays";

/// Env var overriding [`DEFAULT_RELAY_LIST_URL`] (staging / tests), like
/// `K2_CERT_BROKER_URL` for the cert broker.
pub const RELAY_LIST_URL_ENV: &str = "K2_RELAY_LIST_URL";

/// Longest list accepted.
pub const MAX_RELAYS: usize = 8;

/// Refresh interval when the server gives none.
pub const DEFAULT_REFRESH: Duration = Duration::from_secs(6 * 3600);
/// Lower clamp for the server's `refresh_s`.
pub const MIN_REFRESH: Duration = Duration::from_secs(3600);
/// Upper clamp for the server's `refresh_s`.
pub const MAX_REFRESH: Duration = Duration::from_secs(24 * 3600);
/// Retry gap after a network / server error (not a 401 or a bad list).
pub const RETRY_AFTER_ERROR: Duration = Duration::from_secs(15 * 60);

/// Whole-request budget. Short: the boot fetch runs before the tunnel
/// auto-start.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// A relay list is tiny; anything bigger is not one.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// The endpoint URL: [`RELAY_LIST_URL_ENV`] when set (non-blank), else
/// [`DEFAULT_RELAY_LIST_URL`]. In k2-core's own unit tests there is NO
/// default — a test that forgot the env can never reach k2.dev.
pub fn relay_list_url() -> Option<String> {
    match std::env::var(RELAY_LIST_URL_ENV) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ if cfg!(test) => None,
        _ => Some(DEFAULT_RELAY_LIST_URL.to_string()),
    }
}

/// `User-Agent` on the fetch, so K2 Web can tell daemon versions apart.
pub fn user_agent() -> String {
    format!("k2-daemon/{} (relay-list)", env!("CARGO_PKG_VERSION"))
}

// ── Wire shape + validation ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct WireList {
    #[serde(default)]
    v: Option<i64>,
    #[serde(default)]
    relays: Option<Vec<WireRelay>>,
    #[serde(default)]
    refresh_s: Option<i64>,
    /// 0.46.3 (k2-dev-web #20): unix seconds of k2.dev's DNS check behind
    /// the `active` flags. Omitted when the check is unknown. Loose type:
    /// a malformed value only drops the hint, never the list.
    #[serde(default)]
    active_checked_at: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct WireRelay {
    #[serde(default)]
    host: Option<String>,
    /// Wide on purpose so an out-of-range port is a validation error with
    /// a clear message rather than a parse error.
    #[serde(default)]
    port: Option<i64>,
    /// 0.46.3 (k2-dev-web #20): true when the `*.k2.dev` wildcard points
    /// at this relay right now. Omitted on every relay when unknown. Loose
    /// type for the same reason as `active_checked_at`.
    #[serde(default)]
    active: Option<serde_json::Value>,
}

/// A list that passed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidRelayList {
    /// Ordered as received; index 0 = primary.
    pub relays: Vec<RelayEndpoint>,
    /// `refresh_s`, clamped.
    pub refresh: Duration,
    /// 0.46.3: the relay `*.k2.dev` points at right now, per k2.dev's DNS
    /// check — `Some` only when exactly one listed relay says
    /// `"active": true` (see [`active_relay`]). `None` = unknown (an old
    /// server, the check failed, none or several marked active).
    pub active: Option<RelayEndpoint>,
    /// `active_checked_at` (unix seconds) when `active` is known.
    pub active_checked_at: Option<u64>,
}

/// The relay the `active` flags name, or `None` (unknown) unless exactly
/// one flag is `true` and every flag present is a boolean. Omitted on all
/// relays (today's server, or k2.dev's DNS check failed), all `false`
/// (the wildcard points at neither relay) and two `true` all count as
/// unknown; the failover then falls back to its 150 s rule.
fn active_relay(flags: &[Option<serde_json::Value>], relays: &[RelayEndpoint]) -> Option<RelayEndpoint> {
    let mut found = None;
    for (flag, relay) in flags.iter().zip(relays) {
        match flag {
            None | Some(serde_json::Value::Bool(false)) => {}
            Some(serde_json::Value::Bool(true)) => {
                if found.is_some() {
                    return None; // two marked active
                }
                found = Some(relay.clone());
            }
            Some(_) => return None, // not a boolean: don't guess
        }
    }
    found
}

/// True for an IP literal (v4 or v6) or a DNS hostname (labels 1–63 of
/// `A-Z a-z 0-9 -`, not starting/ending with `-`, ≤ 253 total, no
/// trailing dot). An all-numeric dotted name must be a valid IPv4.
pub fn valid_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 || host.trim() != host {
        return false;
    }
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    let label_ok = |l: &&str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if !labels.iter().all(label_ok) {
        return false;
    }
    // "999.1.1.1" is not a hostname: numeric names must parse as an IP.
    !labels.iter().all(|l| l.bytes().all(|b| b.is_ascii_digit()))
}

/// Parse + validate a response body. `Err` names the first problem.
pub fn parse_and_validate(body: &str) -> Result<ValidRelayList, String> {
    let wire: WireList =
        serde_json::from_str(body).map_err(|e| format!("malformed relay list: {e}"))?;
    match wire.v {
        Some(1) => {}
        Some(v) => return Err(format!("unsupported relay list version v={v} (want 1)")),
        None => return Err("relay list has no version (want v=1)".to_string()),
    }
    let wire_relays = wire.relays.unwrap_or_default();
    if wire_relays.is_empty() {
        return Err("relay list is empty".to_string());
    }
    if wire_relays.len() > MAX_RELAYS {
        return Err(format!(
            "relay list has {} relays (at most {MAX_RELAYS})",
            wire_relays.len()
        ));
    }
    let mut seen = HashSet::new();
    let mut relays = Vec::with_capacity(wire_relays.len());
    let mut flags = Vec::with_capacity(wire_relays.len());
    for (i, r) in wire_relays.into_iter().enumerate() {
        flags.push(r.active);
        let host = r.host.unwrap_or_default();
        if !valid_host(&host) {
            return Err(format!("relay #{i}: invalid host {host:?}"));
        }
        let port = match r.port {
            Some(p) if (1..=65535).contains(&p) => p as u16,
            Some(p) => return Err(format!("relay #{i}: port {p} out of range 1-65535")),
            None => return Err(format!("relay #{i}: missing port")),
        };
        if !seen.insert((host.to_ascii_lowercase(), port)) {
            return Err(format!("relay #{i}: duplicate {host}:{port}"));
        }
        relays.push(RelayEndpoint { host, port });
    }
    let refresh = match wire.refresh_s {
        Some(s) if s > 0 => Duration::from_secs(s as u64).clamp(MIN_REFRESH, MAX_REFRESH),
        _ => DEFAULT_REFRESH,
    };
    let active = active_relay(&flags, &relays);
    let active_checked_at = match (&active, wire.active_checked_at) {
        (Some(_), Some(serde_json::Value::Number(n))) => n.as_u64(),
        _ => None,
    };
    Ok(ValidRelayList {
        relays,
        refresh,
        active,
        active_checked_at,
    })
}

// ── Fetch ─────────────────────────────────────────────────────────────

/// Why a fetch produced no usable list. Every variant keeps the old list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// 401: missing / bad / non-tunnel token.
    Unauthorized,
    /// Any other non-200 status.
    Http(u16),
    /// Could not reach the endpoint (DNS, connect, TLS, timeout, read).
    Network(String),
    /// 200, but the body is not a valid list.
    Invalid(String),
    /// The list was fine but writing `tunnel.json` failed.
    Persist(String),
    /// The vault-held tunnel token could not be read (0.45.7); nothing was
    /// fetched or written, retried like a network error.
    TokenUnreadable(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Unauthorized => write!(f, "401 unauthorized (tunnel token refused)"),
            FetchError::Http(c) => write!(f, "HTTP {c}"),
            FetchError::Network(e) => write!(f, "network: {e}"),
            FetchError::Invalid(e) => write!(f, "invalid list: {e}"),
            FetchError::Persist(e) => write!(f, "save tunnel.json: {e}"),
            FetchError::TokenUnreadable(e) => {
                write!(f, "tunnel token unreadable from the K2 vault: {e}")
            }
        }
    }
}

/// One `GET <url>` with the bearer token, validated. Never logs the token.
/// Redirects are not followed (the bearer must not travel elsewhere; a
/// 3xx is just a non-200).
pub fn fetch(url: &str, token: &str) -> Result<ValidRelayList, FetchError> {
    let token = token.trim();
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(user_agent())
        .build()
        .map_err(|e| FetchError::Network(format!("http client: {e}")))?;
    let resp = client
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .send()
        .map_err(|e| FetchError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    if status == 401 {
        return Err(FetchError::Unauthorized);
    }
    if status != 200 {
        return Err(FetchError::Http(status));
    }
    let body = resp
        .text()
        .map_err(|e| FetchError::Network(format!("read body: {e}")))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(FetchError::Invalid(format!("body is {} bytes", body.len())));
    }
    parse_and_validate(&body).map_err(FetchError::Invalid)
}

// ── Persist ───────────────────────────────────────────────────────────

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Write `list` as `relays` (and the primary as `server_addr` /
/// `server_port`) ONLY when it differs from what is on disk. `token` is
/// the token the list was fetched with: when the config's token changed
/// or vanished meanwhile (re-pair, release) nothing is written — a release
/// deletes `tunnel.json`, and a late write must not bring it back.
/// Returns whether the file was written.
pub fn persist_if_changed(list: &[RelayEndpoint], token: &str) -> Result<bool, String> {
    let primary = list
        .first()
        .ok_or_else(|| "refusing to persist an empty relay list".to_string())?
        .clone();
    let mut wrote = false;
    config::update_if(|c| {
        if c.token.trim().is_empty() || c.token.trim() != token.trim() {
            return false;
        }
        let same = c.relays.as_slice() == list
            && c.server_addr == primary.host
            && c.server_port == primary.port;
        if same {
            return false;
        }
        c.relays = list.to_vec();
        c.server_addr = primary.host.clone();
        c.server_port = primary.port;
        c.relays_fetched_at = Some(now_unix());
        wrote = true;
        true
    })?;
    Ok(wrote)
}

// ── One refresh ───────────────────────────────────────────────────────

/// What one [`refresh_once`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Not attempted (air-gap, disabled, released, no token, no URL).
    Skipped(String),
    /// Fetched; the list on disk already matched.
    Unchanged,
    /// Fetched and written (applies at the next reconnect).
    Updated(Vec<RelayEndpoint>),
    /// No usable list; the old one stays.
    Failed(FetchError),
}

#[derive(Debug, Clone, Default)]
struct RefreshState {
    /// Unix seconds of the last successful fetch in this process.
    checked_at: Option<u64>,
    /// The list that fetch returned.
    checked_list: Option<Vec<RelayEndpoint>>,
    /// The server's (clamped) refresh interval from that fetch.
    refresh: Option<Duration>,
    /// The last error, cleared by a success. A 401 is not an error here
    /// (see `refused_at`).
    last_error: Option<String>,
    /// Unix seconds of the last 401 from k2.dev, cleared by a success.
    /// 0.46.3: k2.dev serves the list only for a token it holds as an
    /// active subdomain row, while the relays also take seeded and
    /// freshly provisioned tokens. So a tunnel can run fine on a token
    /// k2.dev does not list: that is a normal state (the saved list stays
    /// in use), reported as a notice, never as "refresh failed".
    refused_at: Option<u64>,
    /// Last error text that was logged (log-once).
    logged_error: Option<String>,
    /// Attempts made (tests / diagnostics).
    attempts: u64,
    /// 0.46.3: what the last successful fetch said about where
    /// `*.k2.dev` points — (when, in this process's monotonic clock; the
    /// active relay or `None` for unknown; k2.dev's `active_checked_at`).
    /// A failed fetch leaves it to age out ([`dns_hint`]).
    dns: Option<(std::time::Instant, Option<RelayEndpoint>, Option<u64>)>,
}

fn state() -> &'static Mutex<RefreshState> {
    static S: OnceLock<Mutex<RefreshState>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(RefreshState::default()))
}

fn lock_state() -> std::sync::MutexGuard<'static, RefreshState> {
    state().lock().unwrap_or_else(|p| p.into_inner())
}

/// Serializes refreshes (boot, timer, switch and pairing can coincide).
fn refresh_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

/// Why a refresh must not run right now, if it must not.
fn skip_reason(cfg: &TunnelConfig) -> Option<String> {
    // An unreadable token is not "not paired": refresh_once fails it softly.
    if cfg.token.trim().is_empty() && !cfg.token_unreadable() {
        return Some("no tunnel token (not paired)".to_string());
    }
    if !cfg.enabled {
        return Some("tunnel is disabled on this device".to_string());
    }
    if super::unpair::identity_released(cfg).is_some() {
        return Some("this device's tunnel identity was released".to_string());
    }
    None
}

/// Fetch, validate and (when changed) persist the relay list once. Uses
/// the token the connector uses ([`config::load`] resolves the vault).
/// Every failure keeps the old list.
pub fn refresh_once() -> RefreshOutcome {
    let _g = refresh_lock().lock().unwrap_or_else(|p| p.into_inner());
    if crate::airgap::enabled() {
        return RefreshOutcome::Skipped("air-gap is on".to_string());
    }
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => return RefreshOutcome::Skipped(format!("tunnel.json unreadable: {e}")),
    };
    if let Some(why) = skip_reason(&cfg) {
        return RefreshOutcome::Skipped(why);
    }
    let Some(url) = relay_list_url() else {
        return RefreshOutcome::Skipped(format!("{RELAY_LIST_URL_ENV} is not set"));
    };
    let token = cfg.token.trim().to_string();
    // Vault unreadable: keep the old list, log once, retry on the error
    // interval. Never a fetch with an empty bearer, never a write.
    let result = match cfg.token_read_error {
        Some(e) => Err(FetchError::TokenUnreadable(e)),
        None => {
            lock_state().attempts += 1;
            fetch(&url, &token).and_then(|list| {
                // 0.46.3: the DNS hint counts even if saving the list fails.
                lock_state().dns = Some((
                    std::time::Instant::now(),
                    list.active.clone(),
                    list.active_checked_at,
                ));
                persist_if_changed(&list.relays, &token)
                    .map(|wrote| (list, wrote))
                    .map_err(FetchError::Persist)
            })
        }
    };
    let mut st = lock_state();
    match result {
        Ok((list, wrote)) => {
            st.checked_at = Some(now_unix());
            st.refresh = Some(list.refresh);
            st.checked_list = Some(list.relays.clone());
            st.last_error = None;
            st.refused_at = None;
            st.logged_error = None;
            if wrote {
                crate::log_debug!(
                    "[tunnel/relays] relay list updated from k2.dev: {} (applies at the next reconnect)",
                    list.relays
                        .iter()
                        .map(|r| r.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                RefreshOutcome::Updated(list.relays)
            } else {
                RefreshOutcome::Unchanged
            }
        }
        Err(e) => {
            let msg = e.to_string();
            if st.logged_error.as_deref() != Some(msg.as_str()) {
                crate::log_debug!("[tunnel/relays] relay list refresh failed, keeping the old list: {msg}");
                st.logged_error = Some(msg.clone());
            }
            if e == FetchError::Unauthorized {
                st.refused_at = Some(now_unix());
                st.last_error = None;
            } else {
                st.last_error = Some(msg);
            }
            RefreshOutcome::Failed(e)
        }
    }
}

// ── Status ────────────────────────────────────────────────────────────

/// [`RelayListStatus::notice`] text after a 401 from k2.dev.
pub const NOTICE_NOT_SERVED: &str =
    "k2.dev does not hand out a relay list for this tunnel token; the saved list stays in use";

/// Where the list on disk came from.
pub const SOURCE_FETCHED: &str = "fetched";
pub const SOURCE_PROVISIONED: &str = "provisioned";
pub const SOURCE_DEFAULT: &str = "default";

/// The relay-list part of `/cli/tunnel/status` (`relay_list`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RelayListStatus {
    /// `fetched` (from k2.dev), `provisioned` (written by hand / the
    /// provisioner) or `default` (no list: the single `server_addr`).
    pub source: String,
    /// The effective ordered list (`host:port`, index 0 = primary).
    pub relays: Vec<String>,
    /// Unix seconds of the last successful fetch by this daemon process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<u64>,
    /// Unix seconds when a fetched list was last WRITTEN (it changed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_at: Option<u64>,
    /// The last refresh error (cleared by the next success). Never a 401:
    /// that is [`RelayListStatus::notice`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// 0.46.3: set when k2.dev answered the last refresh with 401 — it
    /// does not serve a relay list for this tunnel token (a seeded or
    /// freshly provisioned token the relays accept). Not a failure: the
    /// saved list stays in use. Cleared by the next success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    /// The refresh interval in use, seconds.
    pub refresh_s: u64,
    /// 0.46.3: the relay `*.k2.dev` pointed at per the last successful
    /// fetch (`host:port`). Absent when k2.dev did not say (today's
    /// server, its DNS check failed, none or two marked active).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// 0.46.3: k2.dev's `active_checked_at` behind `active` (unix s).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_checked_at: Option<u64>,
}

/// 0.46.3: how old a k2.dev answer about the live relay may be and still
/// steer the failover. The connector asks for a fresh one at most every
/// [`DNS_HINT_REQUEST_EVERY`] while it is deciding.
pub const DNS_HINT_MAX_AGE: Duration = Duration::from_secs(60);

/// 0.46.3: the fastest the failover asks k2.dev for a fresh list. k2.dev
/// caches its DNS check for 15 s; asking faster gains nothing.
pub const DNS_HINT_REQUEST_EVERY: Duration = Duration::from_secs(20);

/// 0.46.3: what the last successful fetch, if no older than `max_age`,
/// said about where `*.k2.dev` points. Never does I/O: the fetch runs on
/// the refresher thread ([`request_refresh`]), so a slow or failed k2.dev
/// never blocks the tunnel's reconnect loop; the last known value is used
/// meanwhile and ages out to [`DnsHint::Unknown`].
pub fn dns_hint(max_age: Duration) -> super::failover::DnsHint {
    let st = lock_state();
    match &st.dns {
        Some((at, Some(relay), _)) if at.elapsed() <= max_age => {
            super::failover::DnsHint::Active(relay.clone())
        }
        _ => super::failover::DnsHint::Unknown,
    }
}

/// Plant a DNS answer as if a fetch had just returned it (tests).
#[cfg(test)]
pub(crate) fn set_dns_for_test(active: Option<RelayEndpoint>, age: Duration) {
    let at = std::time::Instant::now()
        .checked_sub(age)
        .expect("test age fits the monotonic clock");
    lock_state().dns = Some((at, active, None));
}

/// Build the status block for `cfg` (no I/O beyond the in-memory state).
pub fn status_for(cfg: &TunnelConfig) -> RelayListStatus {
    let st = lock_state().clone();
    let effective = cfg.relay_list();
    let source = if cfg.relays.is_empty() {
        SOURCE_DEFAULT
    } else if cfg.relays_fetched_at.is_some()
        || st.checked_list.as_deref() == Some(cfg.relays.as_slice())
    {
        SOURCE_FETCHED
    } else {
        SOURCE_PROVISIONED
    };
    RelayListStatus {
        source: source.to_string(),
        relays: effective.iter().map(|r| r.to_string()).collect(),
        checked_at: st.checked_at,
        changed_at: cfg.relays_fetched_at,
        last_error: st.last_error,
        notice: st.refused_at.map(|_| NOTICE_NOT_SERVED.to_string()),
        refresh_s: st.refresh.unwrap_or(DEFAULT_REFRESH).as_secs(),
        active: st
            .dns
            .as_ref()
            .and_then(|(_, a, _)| a.as_ref())
            .map(|r| r.to_string()),
        active_checked_at: st.dns.as_ref().and_then(|(_, a, c)| a.as_ref().and(*c)),
    }
}

/// Forget the in-memory refresh state (tests).
#[cfg(test)]
pub(crate) fn reset_state_for_test() {
    *lock_state() = RefreshState::default();
}

/// Fetch attempts made so far (tests).
#[cfg(test)]
pub(crate) fn attempts_for_test() -> u64 {
    lock_state().attempts
}

// ── Background refresher ──────────────────────────────────────────────

/// `base` ± up to `frac` of it, at random.
fn jittered(base: Duration, frac: f64) -> Duration {
    let span = (base.as_millis() as f64 * frac) as u64;
    if span == 0 {
        return base;
    }
    let mut b = [0u8; 8];
    let r = match getrandom::getrandom(&mut b) {
        Ok(()) => u64::from_le_bytes(b),
        Err(_) => now_unix().wrapping_mul(0x9E37_79B9_7F4A_7C15),
    };
    let off = r % (2 * span + 1);
    (base + Duration::from_millis(off)).saturating_sub(Duration::from_millis(span))
}

/// How long to wait after `outcome` before the next timed refresh.
pub fn next_wait(outcome: &RefreshOutcome) -> Duration {
    let refresh = lock_state().refresh.unwrap_or(DEFAULT_REFRESH);
    match outcome {
        RefreshOutcome::Failed(FetchError::Network(_))
        | RefreshOutcome::Failed(FetchError::Http(_))
        | RefreshOutcome::Failed(FetchError::Persist(_))
        | RefreshOutcome::Failed(FetchError::TokenUnreadable(_)) => {
            jittered(RETRY_AFTER_ERROR, 0.1)
        }
        _ => jittered(refresh, 0.1),
    }
}

#[derive(Default)]
struct Wake {
    /// A requested refresh and why.
    pending: Option<String>,
    stop: bool,
}

struct Shared {
    wake: Mutex<Wake>,
    cv: Condvar,
}

fn active() -> &'static Mutex<Weak<Shared>> {
    static A: OnceLock<Mutex<Weak<Shared>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(Weak::new()))
}

/// The running refresh loop. Dropping it stops and joins the thread.
pub struct Refresher {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Refresher {
    /// Start the loop; its first timed refresh is `first_wait` out.
    /// Registers it as the target of [`request_refresh`].
    pub fn spawn(first_wait: Duration) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            wake: Mutex::new(Wake::default()),
            cv: Condvar::new(),
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("k2-tunnel-relays".to_string())
            .spawn(move || run_loop(s, first_wait))
            .map_err(|e| format!("spawn relay-list refresher: {e}"))?;
        *active().lock().unwrap_or_else(|p| p.into_inner()) = Arc::downgrade(&shared);
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }
}

impl Drop for Refresher {
    fn drop(&mut self) {
        {
            let mut w = self.shared.wake.lock().unwrap_or_else(|p| p.into_inner());
            w.stop = true;
        }
        self.shared.cv.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let mut a = active().lock().unwrap_or_else(|p| p.into_inner());
        if a.upgrade().is_some_and(|s| Arc::ptr_eq(&s, &self.shared)) {
            *a = Weak::new();
        }
    }
}

fn run_loop(shared: Arc<Shared>, first_wait: Duration) {
    let mut wait = first_wait;
    loop {
        let reason = {
            let deadline = std::time::Instant::now() + wait;
            let mut w = shared.wake.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                if w.stop {
                    return;
                }
                if let Some(r) = w.pending.take() {
                    break r;
                }
                let now = std::time::Instant::now();
                if now >= deadline {
                    break "timer".to_string();
                }
                w = shared
                    .cv
                    .wait_timeout(w, deadline - now)
                    .unwrap_or_else(|p| p.into_inner())
                    .0;
            }
        };
        let outcome = refresh_once();
        if let RefreshOutcome::Skipped(why) = &outcome {
            if reason != "timer" {
                crate::log_debug!("[tunnel/relays] refresh ({reason}) skipped: {why}");
            }
        }
        wait = next_wait(&outcome);
    }
}

/// Ask the running loop for a refresh now (relay switch, token change).
/// Async and best-effort: returns at once; false when no loop runs (the
/// daemon starts one at boot; unit contexts have none).
pub fn request_refresh(reason: &str) -> bool {
    let Some(shared) = active().lock().unwrap_or_else(|p| p.into_inner()).upgrade() else {
        return false;
    };
    {
        let mut w = shared.wake.lock().unwrap_or_else(|p| p.into_inner());
        w.pending = Some(reason.to_string());
    }
    shared.cv.notify_all();
    true
}

/// Daemon boot: one synchronous refresh (so a tunnel auto-start right
/// after dials a fresh list), then the background loop for the life of
/// the process. Call once, off the async runtime (blocking HTTP).
pub fn start_daemon_refresher() -> RefreshOutcome {
    static DAEMON: OnceLock<Refresher> = OnceLock::new();
    let outcome = refresh_once();
    match &outcome {
        RefreshOutcome::Skipped(why) => {
            crate::log_debug!("[tunnel/relays] boot refresh skipped: {why}")
        }
        RefreshOutcome::Unchanged => {
            crate::log_debug!("[tunnel/relays] boot refresh: relay list unchanged")
        }
        _ => {}
    }
    if DAEMON.get().is_none() {
        match Refresher::spawn(next_wait(&outcome)) {
            Ok(r) => {
                let _ = DAEMON.set(r);
            }
            Err(e) => crate::log_debug!("[tunnel/relays] WARN: {e}"),
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;
    use std::io::{Read, Write};
    use std::sync::mpsc;

    const GOOD: &str = r#"{"v":1,"relays":[{"host":"178.156.232.105","port":7000,"role":"primary"},{"host":"40.160.53.25","port":7000,"role":"standby"}],"refresh_s":21600,"ts":1792060000}"#;

    fn ep(host: &str, port: u16) -> RelayEndpoint {
        RelayEndpoint { host: host.to_string(), port }
    }

    fn both() -> Vec<RelayEndpoint> {
        vec![ep("178.156.232.105", 7000), ep("40.160.53.25", 7000)]
    }

    /// A loopback fake of the endpoint: answers every request with
    /// `(status, body)` and reports each request's head on the channel.
    struct FakeServer {
        url: String,
        rx: mpsc::Receiver<String>,
        addr: std::net::SocketAddr,
        stop: Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl FakeServer {
        fn start(status: u16, body: &str) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake");
            let addr = listener.local_addr().expect("addr");
            let (tx, rx) = mpsc::channel();
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stop_t = stop.clone();
            let body = body.to_string();
            let thread = std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop_t.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut sock) = stream else { continue };
                    let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 2048];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes());
                }
            });
            Self {
                url: format!("http://{addr}/api/tunnel/relays"),
                rx,
                addr,
                stop,
                thread: Some(thread),
            }
        }

        fn next_request(&self) -> String {
            self.rx
                .recv_timeout(Duration::from_secs(10))
                .expect("the fake relay-list server must have seen a request")
        }

        fn no_request(&self) {
            assert!(
                self.rx.recv_timeout(Duration::from_millis(300)).is_err(),
                "no request may reach the relay-list endpoint here"
            );
        }
    }

    impl Drop for FakeServer {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = std::net::TcpStream::connect(self.addr);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    fn seed(token: &str) {
        config::save(&TunnelConfig {
            token: token.to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed tunnel.json");
    }

    fn mtime() -> SystemTime {
        std::fs::metadata(config::config_path())
            .expect("stat tunnel.json")
            .modified()
            .expect("mtime")
    }

    // ── validation ──

    #[test]
    fn valid_list_parses_in_order_with_refresh() {
        let l = parse_and_validate(GOOD).expect("the agreed contract body must validate");
        assert_eq!(l.relays, both(), "order kept exactly as received");
        assert_eq!(l.refresh, Duration::from_secs(21600));
    }

    #[test]
    fn hostnames_and_ipv6_are_valid_hosts() {
        for h in ["k2e-01.k2.dev", "relay", "2001:db8::1", "10.0.0.1"] {
            assert!(valid_host(h), "{h} must be valid");
        }
        for h in ["", " 1.2.3.4", "-bad.k2.dev", "bad-.k2.dev", "a..b", "999.1.1.1", "x:7000",
                  "http://k2.dev", "k2.dev.", "a b", "under_score.dev"] {
            assert!(!valid_host(h), "{h:?} must be invalid");
        }
    }

    #[test]
    fn empty_list_is_rejected() {
        let e = parse_and_validate(r#"{"v":1,"relays":[],"refresh_s":21600}"#)
            .expect_err("empty must be rejected");
        assert!(e.contains("empty"), "{e}");
        let e = parse_and_validate(r#"{"v":1}"#).expect_err("missing list must be rejected");
        assert!(e.contains("empty"), "{e}");
    }

    #[test]
    fn malformed_host_is_rejected() {
        let e = parse_and_validate(r#"{"v":1,"relays":[{"host":"evil host;rm","port":7000}]}"#)
            .expect_err("malformed host");
        assert!(e.contains("invalid host"), "{e}");
        let e = parse_and_validate(r#"{"v":1,"relays":[{"port":7000}]}"#).expect_err("no host");
        assert!(e.contains("invalid host"), "{e}");
    }

    #[test]
    fn bad_port_is_rejected() {
        for p in ["0", "65536", "-1"] {
            let body = format!(r#"{{"v":1,"relays":[{{"host":"1.2.3.4","port":{p}}}]}}"#);
            let e = parse_and_validate(&body).expect_err("bad port");
            assert!(e.contains("out of range"), "port {p}: {e}");
        }
        let e = parse_and_validate(r#"{"v":1,"relays":[{"host":"1.2.3.4"}]}"#).expect_err("missing port");
        assert!(e.contains("missing port"), "{e}");
        let e = parse_and_validate(r#"{"v":1,"relays":[{"host":"1.2.3.4","port":"7000"}]}"#)
            .expect_err("string port");
        assert!(e.contains("malformed"), "{e}");
    }

    #[test]
    fn duplicate_relay_is_rejected() {
        let e = parse_and_validate(
            r#"{"v":1,"relays":[{"host":"K2.dev","port":7000},{"host":"k2.dev","port":7000}]}"#,
        )
        .expect_err("duplicate");
        assert!(e.contains("duplicate"), "{e}");
        // Same host, other port: two relays, fine.
        let ok = parse_and_validate(
            r#"{"v":1,"relays":[{"host":"k2.dev","port":7000},{"host":"k2.dev","port":7001}]}"#,
        )
        .expect("same host on two ports is two relays");
        assert_eq!(ok.relays.len(), 2);
    }

    #[test]
    fn too_long_list_is_rejected() {
        let relays: Vec<String> = (1..=MAX_RELAYS + 1)
            .map(|i| format!(r#"{{"host":"10.0.0.{i}","port":7000}}"#))
            .collect();
        let body = format!(r#"{{"v":1,"relays":[{}]}}"#, relays.join(","));
        let e = parse_and_validate(&body).expect_err("too long");
        assert!(e.contains("at most"), "{e}");
        let ok: Vec<String> = (1..=MAX_RELAYS)
            .map(|i| format!(r#"{{"host":"10.0.0.{i}","port":7000}}"#))
            .collect();
        let body = format!(r#"{{"v":1,"relays":[{}]}}"#, ok.join(","));
        assert_eq!(parse_and_validate(&body).expect("max is fine").relays.len(), MAX_RELAYS);
    }

    #[test]
    fn bad_version_is_rejected() {
        let e = parse_and_validate(r#"{"v":2,"relays":[{"host":"1.2.3.4","port":7000}]}"#)
            .expect_err("v2");
        assert!(e.contains("v=2"), "{e}");
        let e = parse_and_validate(r#"{"relays":[{"host":"1.2.3.4","port":7000}]}"#)
            .expect_err("no v");
        assert!(e.contains("no version"), "{e}");
        let e = parse_and_validate("<html>").expect_err("not json");
        assert!(e.contains("malformed"), "{e}");
    }

    #[test]
    fn refresh_s_is_clamped_and_defaulted() {
        let r = |s: &str| {
            parse_and_validate(&format!(
                r#"{{"v":1,"relays":[{{"host":"1.2.3.4","port":7000}}]{s}}}"#
            ))
            .expect("valid")
            .refresh
        };
        assert_eq!(r(r#","refresh_s":5"#), MIN_REFRESH);
        assert_eq!(r(r#","refresh_s":999999"#), MAX_REFRESH);
        assert_eq!(r(r#","refresh_s":7200"#), Duration::from_secs(7200));
        assert_eq!(r(""), DEFAULT_REFRESH);
        assert_eq!(r(r#","refresh_s":-3"#), DEFAULT_REFRESH);
    }

    // ── 0.46.3: the live-relay signal (k2-dev-web #20, merged 25f816f) ──

    /// The "known" body, verbatim from the live k2.dev code.
    const KNOWN: &str = r#"{"v":1,"relays":[{"host":"178.156.232.105","port":7000,"role":"primary","active":true},{"host":"40.160.53.25","port":7000,"role":"standby","active":false}],"refresh_s":21600,"ts":1792606947,"active_checked_at":1792606947}"#;

    /// The "unknown" variant: the same body with no `active` on either
    /// relay and no `active_checked_at` (DoH check failed / resolvers
    /// disagree).
    const UNKNOWN: &str = r#"{"v":1,"relays":[{"host":"178.156.232.105","port":7000,"role":"primary"},{"host":"40.160.53.25","port":7000,"role":"standby"}],"refresh_s":21600,"ts":1792606947}"#;

    fn with_flags(a: &str, b: &str, extra: &str) -> String {
        format!(
            r#"{{"v":1,"relays":[{{"host":"178.156.232.105","port":7000,"role":"primary"{a}}},{{"host":"40.160.53.25","port":7000,"role":"standby"{b}}}],"refresh_s":21600,"ts":1792606947{extra}}}"#
        )
    }

    #[test]
    fn known_fixture_names_the_primary_as_active() {
        let l = parse_and_validate(KNOWN).expect("the live known body is valid");
        assert_eq!(l.relays, both(), "the list itself is unchanged by the new fields");
        assert_eq!(l.active, Some(ep("178.156.232.105", 7000)));
        assert_eq!(l.active_checked_at, Some(1_792_606_947));
    }

    #[test]
    fn known_fixture_with_the_standby_active() {
        let body = with_flags(r#","active":false"#, r#","active":true"#, r#","active_checked_at":1792606947"#);
        let l = parse_and_validate(&body).expect("valid");
        assert_eq!(l.active, Some(ep("40.160.53.25", 7000)));
    }

    #[test]
    fn unknown_fixture_is_unknown() {
        let l = parse_and_validate(UNKNOWN).expect("the unknown body is valid");
        assert_eq!(l.relays, both());
        assert_eq!(l.active, None);
        assert_eq!(l.active_checked_at, None);
    }

    #[test]
    fn old_server_body_is_unknown() {
        // Today's (pre-#20) body: no `active`, no `active_checked_at`.
        let l = parse_and_validate(GOOD).expect("valid");
        assert_eq!(l.relays, both());
        assert_eq!(l.active, None);
    }

    #[test]
    fn two_relays_marked_active_is_unknown() {
        let body = with_flags(r#","active":true"#, r#","active":true"#, r#","active_checked_at":1792606947"#);
        let l = parse_and_validate(&body).expect("the list stays valid");
        assert_eq!(l.relays, both());
        assert_eq!(l.active, None, "two active relays: don't guess");
        assert_eq!(l.active_checked_at, None);
    }

    #[test]
    fn every_relay_inactive_is_unknown() {
        // The wildcard points at neither relay.
        let body = with_flags(r#","active":false"#, r#","active":false"#, r#","active_checked_at":1792606947"#);
        let l = parse_and_validate(&body).expect("the list stays valid");
        assert_eq!(l.active, None);
        assert_eq!(l.active_checked_at, None);
    }

    #[test]
    fn a_non_boolean_active_flag_is_unknown_and_keeps_the_list() {
        let body = with_flags(r#","active":"yes""#, "", "");
        let l = parse_and_validate(&body).expect("a bad hint never drops the list");
        assert_eq!(l.relays, both());
        assert_eq!(l.active, None);
    }

    #[test]
    fn active_on_a_relay_not_in_our_list_never_becomes_a_target() {
        // k2.dev lists a relay this box doesn't run: the parsed hint names
        // it, but the failover only moves to relays in ITS OWN list.
        let body = r#"{"v":1,"relays":[{"host":"178.156.232.105","port":7000,"role":"primary","active":false},{"host":"203.0.113.9","port":7000,"role":"standby","active":true}],"refresh_s":21600,"ts":1792606947,"active_checked_at":1792606947}"#;
        let l = parse_and_validate(body).expect("valid");
        assert_eq!(l.active, Some(ep("203.0.113.9", 7000)));
        let mut sel = crate::tunnel::failover::RelaySelector::with_policy_seeded(
            both(),
            3,
            crate::tunnel::failover::FailbackPolicy::DNS_AWARE,
            5,
        )
        .with_dns_gate(crate::tunnel::failover::DEFAULT_DNS_MIN_OUTAGE);
        let hint = crate::tunnel::failover::DnsHint::Active(l.active.clone().expect("named"));
        let t0 = std::time::Instant::now();
        for s in 0..140u64 {
            let d = sel.on_failure_at(t0 + Duration::from_secs(s), &hint, &mut |_| true);
            assert!(
                !matches!(d, crate::tunnel::failover::FailureDecision::Switch(..)),
                "an unlisted active relay must not trigger a move ({s}s): {d:?}"
            );
        }
        assert_eq!(sel.current(), &ep("178.156.232.105", 7000));
    }

    #[test]
    fn a_fetched_known_list_feeds_the_failover_hint_and_status() {
        with_temp_home(|| {
            reset_state_for_test();
            seed("k2c_good");
            assert_eq!(dns_hint(DNS_HINT_MAX_AGE), crate::tunnel::failover::DnsHint::Unknown);
            let srv = FakeServer::start(200, KNOWN);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            let out = refresh_once();
            assert_eq!(out, RefreshOutcome::Updated(both()));
            assert_eq!(
                dns_hint(DNS_HINT_MAX_AGE),
                crate::tunnel::failover::DnsHint::Active(ep("178.156.232.105", 7000))
            );
            let cfg = config::load().expect("load");
            let st = status_for(&cfg);
            assert_eq!(st.active.as_deref(), Some("178.156.232.105:7000"));
            assert_eq!(st.active_checked_at, Some(1_792_606_947));
            // The on-disk list never carries the hint.
            let raw = std::fs::read_to_string(config::config_path()).expect("read tunnel.json");
            assert!(!raw.contains("active"), "{raw}");
        });
    }

    #[test]
    fn a_fetched_unknown_list_clears_the_hint() {
        with_temp_home(|| {
            reset_state_for_test();
            seed("k2c_good");
            set_dns_for_test(Some(ep("178.156.232.105", 7000)), Duration::ZERO);
            let srv = FakeServer::start(200, UNKNOWN);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            assert_eq!(refresh_once(), RefreshOutcome::Updated(both()));
            assert_eq!(dns_hint(DNS_HINT_MAX_AGE), crate::tunnel::failover::DnsHint::Unknown);
            assert_eq!(status_for(&config::load().expect("load")).active, None);
        });
    }

    #[test]
    fn the_hint_ages_out_to_unknown() {
        with_temp_home(|| {
            reset_state_for_test();
            set_dns_for_test(Some(ep("40.160.53.25", 7000)), Duration::from_secs(30));
            assert_eq!(
                dns_hint(DNS_HINT_MAX_AGE),
                crate::tunnel::failover::DnsHint::Active(ep("40.160.53.25", 7000)),
                "a 30 s old answer is still used"
            );
            set_dns_for_test(Some(ep("40.160.53.25", 7000)), Duration::from_secs(61));
            assert_eq!(
                dns_hint(DNS_HINT_MAX_AGE),
                crate::tunnel::failover::DnsHint::Unknown,
                "a stale answer must not steer the failover"
            );
            reset_state_for_test();
        });
    }

    #[test]
    fn a_failed_fetch_keeps_the_last_known_hint_until_it_ages_out() {
        with_temp_home(|| {
            reset_state_for_test();
            seed("k2c_good");
            set_dns_for_test(Some(ep("178.156.232.105", 7000)), Duration::from_secs(5));
            let srv = FakeServer::start(503, r#"{"error":"Lookup failed."}"#);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            assert!(matches!(refresh_once(), RefreshOutcome::Failed(FetchError::Http(503))));
            assert_eq!(
                dns_hint(DNS_HINT_MAX_AGE),
                crate::tunnel::failover::DnsHint::Active(ep("178.156.232.105", 7000)),
                "the last known value is used meanwhile"
            );
            reset_state_for_test();
        });
    }

    #[test]
    fn fetch_timeouts_cover_k2devs_slow_dns_check() {
        // A cache miss on k2.dev can take ~1.5 s while it runs its DNS
        // check; the fetch runs off the reconnect loop (refresher thread).
        assert!(HTTP_TIMEOUT >= Duration::from_secs(5), "{HTTP_TIMEOUT:?}");
        assert!(CONNECT_TIMEOUT >= Duration::from_secs(5), "{CONNECT_TIMEOUT:?}");
        assert!(DNS_HINT_REQUEST_EVERY >= Duration::from_secs(15), "k2.dev caches 15 s");
    }

    #[test]
    fn a_slow_k2dev_never_blocks_reading_the_hint() {
        // A fake k2.dev that never answers: a refresh on another thread
        // hangs on it, while dns_hint() (what the reconnect loop calls)
        // returns at once with the last known value.
        with_temp_home(|| {
            reset_state_for_test();
            seed("k2c_good");
            set_dns_for_test(Some(ep("178.156.232.105", 7000)), Duration::ZERO);
            let silent = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            let url = format!("http://{}/api/tunnel/relays", silent.local_addr().expect("addr"));
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &url);
            let t = std::thread::spawn(refresh_once);
            std::thread::sleep(Duration::from_millis(300));
            assert!(!t.is_finished(), "setup: the fetch must still be hanging");
            let started = std::time::Instant::now();
            let hint = dns_hint(DNS_HINT_MAX_AGE);
            assert!(started.elapsed() < Duration::from_millis(100), "dns_hint must not wait on I/O");
            assert_eq!(
                hint,
                crate::tunnel::failover::DnsHint::Active(ep("178.156.232.105", 7000))
            );
            let out = t.join().expect("refresh thread");
            assert!(matches!(out, RefreshOutcome::Failed(FetchError::Network(_))), "{out:?}");
            drop(silent);
            reset_state_for_test();
        });
    }

    // ── refresh_once against a fake server ──

    #[test]
    fn fetch_writes_the_list_and_keeps_server_addr_coherent() {
        with_temp_home(|| {
            reset_state_for_test();
            seed("k2c_good");
            let srv = FakeServer::start(200, GOOD);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            let out = refresh_once();
            assert_eq!(out, RefreshOutcome::Updated(both()));
            let req = srv.next_request().to_ascii_lowercase();
            assert!(req.starts_with("get /api/tunnel/relays "), "{req}");
            assert!(req.contains("authorization: bearer k2c_good"), "{req}");
            assert!(req.contains("user-agent: k2-daemon/"), "{req}");
            let cfg = config::load().expect("load");
            assert_eq!(cfg.relays, both());
            assert_eq!(cfg.server_addr, "178.156.232.105");
            assert_eq!(cfg.server_port, 7000);
            assert!(cfg.relays_fetched_at.is_some(), "fetched lists are stamped");
            let st = status_for(&cfg);
            assert_eq!(st.source, SOURCE_FETCHED);
            assert_eq!(st.relays, vec!["178.156.232.105:7000", "40.160.53.25:7000"]);
            assert!(st.checked_at.is_some());
            assert_eq!(st.last_error, None);
            assert_eq!(st.refresh_s, 21600);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(config::config_path())
                    .expect("stat")
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o600, "the rewritten tunnel.json must stay 0600");
            }
        });
    }

    #[test]
    fn writes_only_when_the_list_changed() {
        with_temp_home(|| {
            reset_state_for_test();
            seed("k2c_good");
            let srv = FakeServer::start(200, GOOD);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            assert_eq!(refresh_once(), RefreshOutcome::Updated(both()));
            let raw_before = std::fs::read(config::config_path()).expect("read");
            let m0 = mtime();
            std::thread::sleep(Duration::from_millis(1100));
            assert_eq!(refresh_once(), RefreshOutcome::Unchanged, "same list → no write");
            assert_eq!(mtime(), m0, "an unchanged list must not touch tunnel.json");
            assert_eq!(std::fs::read(config::config_path()).expect("read"), raw_before);
        });
    }

    #[test]
    fn provisioned_identical_list_is_not_rewritten() {
        with_temp_home(|| {
            reset_state_for_test();
            config::save(&TunnelConfig {
                server_addr: "178.156.232.105".to_string(),
                relays: both(),
                token: "k2c_good".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed provisioned");
            let m0 = mtime();
            let srv = FakeServer::start(200, GOOD);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            assert_eq!(refresh_once(), RefreshOutcome::Unchanged);
            assert_eq!(mtime(), m0);
            let cfg = config::load().expect("load");
            assert_eq!(cfg.relays_fetched_at, None, "no write → no stamp");
            assert_eq!(
                status_for(&cfg).source,
                SOURCE_FETCHED,
                "the list on disk matches what k2.dev said"
            );
        });
    }

    fn assert_kept_old(status: u16, body: &str, want: fn(&FetchError) -> bool) {
        with_temp_home(|| {
            reset_state_for_test();
            config::save(&TunnelConfig {
                relays: vec![ep("10.9.9.1", 7000), ep("10.9.9.2", 7000)],
                server_addr: "10.9.9.1".to_string(),
                token: "k2c_good".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed old list");
            let raw_before = std::fs::read(config::config_path()).expect("read");
            let srv = FakeServer::start(status, body);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            match refresh_once() {
                RefreshOutcome::Failed(e) => assert!(want(&e), "status {status}: wrong error {e:?}"),
                other => panic!("status {status} body {body}: expected Failed, got {other:?}"),
            }
            srv.next_request();
            assert_eq!(
                std::fs::read(config::config_path()).expect("read"),
                raw_before,
                "the old list must be kept byte-for-byte"
            );
            let st = status_for(&config::load().expect("load"));
            assert_eq!(st.source, SOURCE_PROVISIONED);
            if status == 401 {
                // 0.46.3: a 401 is a notice (k2.dev does not list this
                // token), never "refresh failed".
                assert_eq!(st.last_error, None, "a 401 must not read as a failure");
                assert_eq!(st.notice.as_deref(), Some(NOTICE_NOT_SERVED));
            } else {
                assert!(st.last_error.is_some(), "status must carry the error");
                assert_eq!(st.notice, None, "only a 401 sets the notice");
            }
        });
    }

    #[test]
    fn unauthorized_keeps_the_old_list() {
        assert_kept_old(401, r#"{"code":"tunnel_only"}"#, |e| *e == FetchError::Unauthorized);
    }

    /// 0.46.3: a later success clears the 401 notice, and the status JSON
    /// never says "failed" for it.
    #[test]
    fn unauthorized_notice_is_cleared_by_the_next_success() {
        with_temp_home(|| {
            reset_state_for_test();
            seed("k2c_good");
            {
                let srv = FakeServer::start(401, r#"{"code":"tunnel_only"}"#);
                let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
                assert_eq!(refresh_once(), RefreshOutcome::Failed(FetchError::Unauthorized));
                srv.next_request();
            }
            let st = status_for(&config::load().expect("load"));
            let wire = serde_json::to_string(&st).expect("serialize");
            assert!(wire.contains(r#""notice":"#), "{wire}");
            assert!(!wire.contains("last_error"), "a 401 must not be a last_error: {wire}");
            assert!(!wire.to_ascii_lowercase().contains("fail"), "{wire}");

            let srv = FakeServer::start(200, GOOD);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            assert_eq!(refresh_once(), RefreshOutcome::Updated(both()));
            srv.next_request();
            let st = status_for(&config::load().expect("load"));
            assert_eq!(st.notice, None, "a success clears the notice");
            assert_eq!(st.last_error, None);
        });
    }

    #[test]
    fn server_error_keeps_the_old_list() {
        assert_kept_old(503, "", |e| *e == FetchError::Http(503));
    }

    #[test]
    fn invalid_body_keeps_the_old_list() {
        assert_kept_old(200, r#"{"v":1,"relays":[]}"#, |e| matches!(e, FetchError::Invalid(_)));
        assert_kept_old(200, r#"{"v":1,"relays":[{"host":"1.2.3.4","port":0}]}"#, |e| {
            matches!(e, FetchError::Invalid(_))
        });
    }

    #[test]
    fn unreachable_endpoint_keeps_the_old_list() {
        with_temp_home(|| {
            reset_state_for_test();
            seed("k2c_good");
            let closed = crate::test_env::ClosedPort::new();
            let _u = crate::test_env::EnvVar::set(
                RELAY_LIST_URL_ENV,
                format!("http://127.0.0.1:{}/api/tunnel/relays", closed.port()),
            );
            match refresh_once() {
                RefreshOutcome::Failed(FetchError::Network(_)) => {}
                other => panic!("expected a network failure, got {other:?}"),
            }
            assert!(config::load().expect("load").relays.is_empty(), "nothing written");
        });
    }

    #[test]
    fn never_fetches_without_a_token() {
        with_temp_home(|| {
            reset_state_for_test();
            seed("");
            let srv = FakeServer::start(200, GOOD);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            match refresh_once() {
                RefreshOutcome::Skipped(why) => assert!(why.contains("no tunnel token"), "{why}"),
                other => panic!("expected Skipped, got {other:?}"),
            }
            srv.no_request();
            assert_eq!(attempts_for_test(), 0);
        });
    }

    #[test]
    fn never_fetches_when_the_tunnel_is_disabled() {
        with_temp_home(|| {
            reset_state_for_test();
            config::save(&TunnelConfig {
                token: "k2c_good".to_string(),
                enabled: false,
                e2e: false,
                ..Default::default()
            })
            .expect("seed disabled");
            let srv = FakeServer::start(200, GOOD);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            match refresh_once() {
                RefreshOutcome::Skipped(why) => assert!(why.contains("disabled"), "{why}"),
                other => panic!("expected Skipped, got {other:?}"),
            }
            srv.no_request();
        });
    }

    #[test]
    fn never_fetches_when_released() {
        with_temp_home(|| {
            reset_state_for_test();
            seed("k2c_old");
            super::super::unpair::save(&super::super::unpair::UnpairTombstone {
                released_at: "2026-10-15T00:00:00+00:00".to_string(),
                subdomain: "rosson".to_string(),
                token_sha256: super::super::unpair::token_fingerprint("k2c_old"),
                upstream_reported: true,
                ..Default::default()
            })
            .expect("tombstone");
            let srv = FakeServer::start(200, GOOD);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            match refresh_once() {
                RefreshOutcome::Skipped(why) => assert!(why.contains("released"), "{why}"),
                other => panic!("expected Skipped, got {other:?}"),
            }
            srv.no_request();
        });
    }

    #[test]
    fn never_fetches_under_air_gap() {
        with_temp_home(|| {
            reset_state_for_test();
            seed("k2c_good");
            let srv = FakeServer::start(200, GOOD);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            let _a = crate::test_env::EnvVar::set(crate::airgap::ENV_VAR, "1");
            match refresh_once() {
                RefreshOutcome::Skipped(why) => assert!(why.contains("air-gap"), "{why}"),
                other => panic!("expected Skipped, got {other:?}"),
            }
            srv.no_request();
        });
    }

    #[test]
    fn unit_tests_have_no_default_url() {
        let _g = crate::test_env::lock();
        let _u = crate::test_env::EnvVar::remove(RELAY_LIST_URL_ENV);
        assert_eq!(relay_list_url(), None, "k2-core tests can never reach k2.dev");
        let _s = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, " http://127.0.0.1:1/x ");
        assert_eq!(relay_list_url().as_deref(), Some("http://127.0.0.1:1/x"));
    }

    #[test]
    fn an_unreadable_vault_token_fetches_nothing_writes_nothing_and_retries() {
        with_temp_home(|| {
            reset_state_for_test();
            let vault = config::MemTokenVault::holding(None);
            let _v = config::TestTokenVault::install(vault.clone());
            seed("k2c_kept");
            assert_eq!(vault.stored().as_deref(), Some("k2c_kept"), "setup: token in the vault");
            let before = std::fs::read(config::config_path()).expect("read tunnel.json");
            let srv = FakeServer::start(200, GOOD);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);

            vault.set_fail_reads(true);
            let outcome = refresh_once();
            match &outcome {
                RefreshOutcome::Failed(FetchError::TokenUnreadable(e)) => {
                    assert!(e.contains("read failed"), "{e}")
                }
                other => panic!("expected Failed(TokenUnreadable), got {other:?}"),
            }
            srv.no_request();
            let w = next_wait(&outcome);
            assert!(
                w >= RETRY_AFTER_ERROR.mul_f64(0.9) && w <= RETRY_AFTER_ERROR.mul_f64(1.1),
                "an unreadable token retries on the error interval, got {w:?}"
            );
            assert_eq!(std::fs::read(config::config_path()).expect("read"), before, "nothing written");
            assert_eq!(vault.stored().as_deref(), Some("k2c_kept"), "the vault token is untouched");
            let st = status_for(&config::load().expect("load"));
            assert!(
                st.last_error.as_deref().is_some_and(|e| e.contains("unreadable")),
                "the status names the unreadable token: {st:?}"
            );

            // The vault reads again: the next refresh fetches with the kept token.
            vault.set_fail_reads(false);
            assert_eq!(refresh_once(), RefreshOutcome::Updated(both()));
            let req = srv.next_request().to_ascii_lowercase();
            assert!(req.contains("authorization: bearer k2c_kept"), "{req}");
        });
    }

    #[test]
    fn fetch_uses_the_connectors_resolved_token_from_the_vault() {
        with_temp_home(|| {
            reset_state_for_test();
            let _v = config::TestTokenVault::install(config::MemTokenVault::holding(None));
            // Save through the vault: the file keeps everything but the token.
            seed("k2c_from_vault");
            let raw = std::fs::read_to_string(config::config_path()).expect("read");
            let on_disk: TunnelConfig = serde_json::from_str(&raw).expect("parse");
            assert_eq!(on_disk.token, "", "setup: tunnel.json's own token must be empty");
            // The connector's frpc.toml renders the resolved token...
            let rendered = super::super::render_config(4242).expect("render");
            assert!(rendered.contains("token = \"k2c_from_vault\""), "{rendered}");
            // ...and the relay fetch sends that same token.
            let srv = FakeServer::start(200, GOOD);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            assert_eq!(refresh_once(), RefreshOutcome::Updated(both()));
            let req = srv.next_request().to_ascii_lowercase();
            assert!(req.contains("authorization: bearer k2c_from_vault"), "{req}");
            // The write kept the token in the vault, not the file.
            let raw = std::fs::read_to_string(config::config_path()).expect("read");
            assert!(!raw.contains("k2c_from_vault"), "token must not land in tunnel.json\n{raw}");
            assert_eq!(config::load().expect("load").token, "k2c_from_vault");
        });
    }

    #[test]
    fn a_token_change_during_the_fetch_writes_nothing() {
        with_temp_home(|| {
            seed("k2c_new");
            let wrote = persist_if_changed(&both(), "k2c_old").expect("persist");
            assert!(!wrote, "a list fetched with an old token must not be written");
            assert!(config::load().expect("load").relays.is_empty());
        });
    }

    #[test]
    fn a_release_during_the_fetch_does_not_recreate_tunnel_json() {
        with_temp_home(|| {
            assert!(!config::config_path().exists());
            let wrote = persist_if_changed(&both(), "k2c_old").expect("persist");
            assert!(!wrote);
            assert!(!config::config_path().exists(), "no token on disk → no file");
        });
    }

    // ── triggers ──

    #[test]
    fn request_refresh_without_a_loop_is_a_no_op() {
        let _g = crate::test_env::lock();
        assert!(!request_refresh("test"), "no loop → nothing to wake");
    }

    #[test]
    fn pairing_a_new_token_triggers_a_fetch() {
        with_temp_home(|| {
            reset_state_for_test();
            let _lease = crate::test_env::EnvVar::set(super::super::lease::LEASE_ENV_VAR, "off");
            let srv = FakeServer::start(200, GOOD);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            seed("");
            // The daemon's loop, with no timed refresh during the test.
            let loop_ = Refresher::spawn(Duration::from_secs(3600)).expect("spawn");
            srv.no_request();
            // Desktop pairing: POST /cli/tunnel/config with the token.
            let (_view, _report) = super::super::identity::apply_config(
                super::super::TunnelConfigUpdate {
                    token: Some("k2c_paired".to_string()),
                    ..Default::default()
                },
                57839,
                &super::super::FrpcBinary::Explicit(std::path::PathBuf::from("/no/frpc")),
            )
            .expect("pairing must succeed whatever the fetch does");
            let req = srv.next_request().to_ascii_lowercase();
            assert!(req.contains("authorization: bearer k2c_paired"), "{req}");
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while config::load().expect("load").relays != both() {
                assert!(std::time::Instant::now() < deadline, "the fetched list was never written");
                std::thread::sleep(Duration::from_millis(50));
            }
            drop(loop_);
            assert!(!request_refresh("after stop"), "a dropped loop is unregistered");
        });
    }

    #[test]
    fn pairing_succeeds_when_the_fetch_fails() {
        with_temp_home(|| {
            reset_state_for_test();
            let _lease = crate::test_env::EnvVar::set(super::super::lease::LEASE_ENV_VAR, "off");
            let srv = FakeServer::start(401, r#"{"code":"tunnel_only"}"#);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            seed("");
            let loop_ = Refresher::spawn(Duration::from_secs(3600)).expect("spawn");
            let (view, _) = super::super::identity::apply_config(
                super::super::TunnelConfigUpdate {
                    token: Some("k2c_paired".to_string()),
                    ..Default::default()
                },
                57839,
                &super::super::FrpcBinary::Explicit(std::path::PathBuf::from("/no/frpc")),
            )
            .expect("pairing must not fail on a failed relay fetch");
            assert!(view.token_set);
            srv.next_request();
            drop(loop_);
            assert!(config::load().expect("load").relays.is_empty());
        });
    }

    #[test]
    fn a_non_token_config_write_does_not_trigger_a_fetch() {
        with_temp_home(|| {
            reset_state_for_test();
            let srv = FakeServer::start(200, GOOD);
            let _u = crate::test_env::EnvVar::set(RELAY_LIST_URL_ENV, &srv.url);
            seed("k2c_same");
            let loop_ = Refresher::spawn(Duration::from_secs(3600)).expect("spawn");
            super::super::identity::apply_config(
                super::super::TunnelConfigUpdate {
                    device_id: Some("dev-2".to_string()),
                    ..Default::default()
                },
                57839,
                &super::super::FrpcBinary::Explicit(std::path::PathBuf::from("/no/frpc")),
            )
            .expect("apply");
            srv.no_request();
            drop(loop_);
        });
    }

    #[test]
    fn jitter_stays_within_ten_percent() {
        for _ in 0..200 {
            let j = jittered(DEFAULT_REFRESH, 0.1);
            assert!(j >= DEFAULT_REFRESH.mul_f64(0.9) && j <= DEFAULT_REFRESH.mul_f64(1.1), "{j:?}");
        }
    }
}
