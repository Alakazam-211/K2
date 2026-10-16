//! Subscription windows for the signed-in Claude, Codex, and Grok logins.
//!
//! The daemon probes. The cache file is `k2_home()/usage/subscriptions.json`
//! and stores windows only — never an access token, refresh token, or
//! `auth.json` body. The renderer reads `GET /cli/usage/subscriptions`
//! and asks for a probe with `POST /cli/usage/subscriptions/refresh`.
//!
//! Air-gap ([`k2_core::airgap::enabled`]) stops every probe: no
//! `api.anthropic.com` GET and no `codex` / `grok` child. Both routes then
//! answer [`airgap_doc`] (`"airgap": true` plus a row per harness whose
//! status is [`STATUS_AIRGAP`]) instead of a stale number or an error.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;

/// Background probe period. The first tick waits one full interval;
/// opening the menu is the cold-start probe.
pub const PROBE_INTERVAL: Duration = Duration::from_secs(15 * 60);
const FRESH_FOR: Duration = Duration::from_secs(15);
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const ANTHROPIC_BETA: &str = "oauth-2025-04-20";
/// `expiresAt` above this is unix milliseconds. At or below, unix seconds.
const EXPIRY_UNIT_THRESHOLD: i64 = 1_000_000_000_000;
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

pub const STATUS_NOT_SIGNED_IN: &str = "Not signed in";
pub const STATUS_SIGN_IN_EXPIRED: &str = "Sign-in expired";
pub const STATUS_NO_WINDOW: &str = "No usage window";
/// API-key logins and tools with no limits API (Gemini).
pub const STATUS_NOT_SUPPORTED: &str = "Not supported";
/// Row status while air-gap is on. An older renderer prints it as-is.
pub const STATUS_AIRGAP: &str = "Off in air-gap mode";

const HARNESS_CLAUDE: &str = "claude";
const HARNESS_CODEX: &str = "codex";
const HARNESS_GROK: &str = "grok";
const PROBED_HARNESSES: [&str; 3] = [HARNESS_CLAUDE, HARNESS_CODEX, HARNESS_GROK];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub label: String,
    /// Fraction used, 0..1.
    pub used: f64,
    pub resets_at: String,
    /// Stable id: `five_hour`, `seven_day`, `seven_day_oauth_apps`,
    /// `model:<name>:<kind>`, `primary`, `secondary`, `credit`, `grok_build`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// `session` · `weekly` · `monthly` · `daily` · `other`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// Percent used, 0..100 (filled when served).
    #[serde(default)]
    pub used_percent: f64,
    /// The window's length, when the vendor says (pace mark).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_secs: Option<i64>,
    /// `resetsAt − windowSecs` (filled when served).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_start: Option<String>,
    /// `resetsAt` is past: a re-check is under way (filled when served).
    #[serde(default)]
    pub reset_passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct HarnessUsage {
    pub harness: String,
    pub plan: String,
    pub windows: Vec<UsageWindow>,
    /// When these windows were last read successfully. A failed check
    /// keeps the old time (never re-stamps old numbers as fresh); `""` =
    /// never read.
    pub checked_at: String,
    pub status: String,
    /// The last check failed: why (status code / transport, never a
    /// body). The windows are the last good reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The wallet token these numbers belong to (shared cache only; set
    /// when no switch happened during the probe).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// When the failed check ran (freshness of the 15 s cache).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempted_at: Option<String>,
    /// Why there are (no) numbers, typed (P8): `ok` · `no_window` ·
    /// `not_signed_in` · `expired` · `probe_failed` · `unsupported` ·
    /// `airgap`. Filled when served ([`derive_state`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub state: String,
    /// Where the numbers came from: `probe` · `live_statusline` ·
    /// `live_rollout` (Q5).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// The daemon is backing off this check until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_probe_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubscriptionFile {
    pub harnesses: Vec<HarnessUsage>,
}

#[derive(Debug, Clone)]
enum ClaudeLogin {
    Missing,
    Token {
        access_token: String,
        expires_at: Option<i64>,
        plan: String,
    },
}

#[derive(Debug, Clone)]
enum CodexOutcome {
    NotSignedIn,
    /// Transport or timeout. Keep still-open cached windows.
    Failed,
    Ready {
        plan: String,
        windows: Vec<UsageWindow>,
    },
}

#[derive(Debug)]
enum CodexFail {
    NotSignedIn,
    Transport(String),
}

#[derive(Debug)]
struct CodexParsed {
    plan: String,
    windows: Vec<UsageWindow>,
}

#[derive(Debug, Clone)]
enum GrokOutcome {
    NotSignedIn,
    /// Transport or timeout. Keep still-open cached windows.
    Failed,
    Ready {
        plan: String,
        windows: Vec<UsageWindow>,
    },
}

#[derive(Debug)]
enum GrokFail {
    NotSignedIn,
    Transport(String),
}

#[derive(Debug)]
struct GrokParsed {
    plan: String,
    windows: Vec<UsageWindow>,
}

trait UsageIo: Send + Sync {
    fn claude_login(&self) -> ClaudeLogin;
    fn claude_get(&self, access_token: &str) -> Result<String, String>;
    fn codex(&self) -> CodexOutcome;
    fn grok(&self) -> GrokOutcome;
}

struct LiveIo;

fn probe_hits() -> &'static AtomicUsize {
    static HITS: AtomicUsize = AtomicUsize::new(0);
    &HITS
}

fn test_io_slot() -> &'static Mutex<Option<Arc<dyn UsageIo>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<dyn UsageIo>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
fn test_airgap_slot() -> &'static Mutex<Option<bool>> {
    static SLOT: OnceLock<Mutex<Option<bool>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Air-gap gate for every probe path. Tests pin it through
/// `test_airgap_slot` so a parallel test that sets `K2_AIRGAP` cannot flip it.
fn airgap_on() -> bool {
    #[cfg(test)]
    if let Some(on) = *test_airgap_slot().lock().unwrap_or_else(|p| p.into_inner()) {
        return on;
    }
    k2_core::airgap::enabled()
}

/// The answer to both usage routes while air-gap is on. Nothing is probed
/// and the cache is neither read nor written, so no old number shows.
pub fn airgap_doc() -> Value {
    let now = Utc::now().to_rfc3339();
    let harnesses: Vec<HarnessUsage> = PROBED_HARNESSES
        .iter()
        .map(|name| HarnessUsage {
            error: None,
            account_id: None,
            attempted_at: None,
            harness: (*name).to_string(),
            plan: String::new(),
            windows: Vec::new(),
            checked_at: now.clone(),
            status: STATUS_AIRGAP.to_string(),
            ..Default::default()
        })
        .collect();
    json!({
        "airgap": true,
        "reason": STATUS_AIRGAP,
        "harnesses": harnesses,
    })
}

fn airgap_response() -> CliResponse {
    CliResponse::ok_json(airgap_doc().to_string())
}

/// prd-daemon-activity-and-thread-working-v1 A9: the usage probes start
/// real agent CLIs, so they resolve the program through the same spawn
/// guard every PTY spawn uses. Under `K2_TEST_AGENT_SHIM_DIR` only a shim
/// runs; under a temp HOME a real `codex` / `grok` is never started.
fn guarded_program(name: &str) -> Result<String, String> {
    let search = k2_core::terminal::login_path::augmented_path(
        &k2_core::terminal::login_path::process_path(),
    );
    let guard = k2_core::terminal::agent_spawn_guard::GuardEnv::from_process();
    k2_core::terminal::agent_spawn_guard::resolve_program(name, &search, &guard)
}

fn probe_denied() -> bool {
    std::env::var_os("K2_SUBSCRIPTION_PROBE").as_deref() == Some(std::ffi::OsStr::new("deny"))
}

pub fn cache_path() -> PathBuf {
    k2_core::paths::k2_home()
        .join("usage")
        .join("subscriptions.json")
}

/// `GET /cli/usage/subscriptions`. Reads the cache (never probes inline).
/// A window past its reset is marked `resetPassed` and kicks ONE
/// background re-check (P8, B18).
pub fn handle_get() -> CliResponse {
    if airgap_on() {
        return airgap_response();
    }
    let path = cache_path();
    if !path.is_file() {
        return ok_doc(&SubscriptionFile {
            harnesses: Vec::new(),
        });
    }
    let doc = match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<SubscriptionFile>(&text) {
            Ok(doc) => filter_known(doc),
            Err(_) => return CliResponse::internal_error("subscriptions cache is not valid JSON"),
        },
        Err(e) => return CliResponse::internal_error(format!("subscriptions cache: {e}")),
    };
    let doc = decorate_file(doc, Utc::now());
    if any_reset_passed(&doc) {
        kick_background_probe();
    }
    ok_doc(&doc)
}

/// `POST /cli/usage/subscriptions/refresh`.
/// A cache newer than 15 seconds is returned as-is. Otherwise the probe
/// runs here — the caller must be off the connection task (`spawn_blocking`
/// or the background loop). Concurrent callers share one probe (B19).
pub fn handle_refresh() -> CliResponse {
    if airgap_on() {
        return airgap_response();
    }
    match refresh_shared(None) {
        Ok(doc) => ok_doc(&decorate_file(doc, Utc::now())),
        Err(e) => CliResponse::internal_error(e),
    }
}

/// The shared cache, refreshed when older than 15 s (one tool, or all).
/// Single-flight. Air-gap is the caller's check.
pub fn refresh_shared(only: Option<&str>) -> Result<SubscriptionFile, String> {
    if only.is_none() {
        if let Some(doc) = fresh_cache(Utc::now()) {
            return Ok(doc);
        }
    }
    probe_singleflight(io_for_probe().as_ref(), only, Mode::Manual)
}

/// How long after boot the first probe may run (lets boot settle).
const BOOT_PROBE_DELAY: Duration = Duration::from_secs(20);

/// Headless 15-minute probe. No webview. Not the heartbeat monitor.
/// At boot it probes once when the cache is missing or older than 15
/// minutes (B18), then every 15 minutes.
pub fn spawn() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(BOOT_PROBE_DELAY).await;
        if cache_stale(Utc::now()) {
            let _ = tokio::task::spawn_blocking(background_tick).await;
        }
        let mut interval = tokio::time::interval(PROBE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        loop {
            interval.tick().await;
            let _ = tokio::task::spawn_blocking(background_tick).await;
        }
    })
}

/// Missing, unreadable, or a tool's row older than [`PROBE_INTERVAL`].
fn cache_stale(now: DateTime<Utc>) -> bool {
    let Ok(doc) = read_cache_file() else { return true };
    PROBED_HARNESSES.iter().any(|name| {
        let Some(h) = doc.harnesses.iter().find(|h| h.harness == *name) else { return true };
        let when = h.attempted_at.as_deref().unwrap_or(&h.checked_at);
        match DateTime::parse_from_rfc3339(when) {
            Ok(t) => now.signed_duration_since(t.with_timezone(&Utc)) > chrono::Duration::from_std(PROBE_INTERVAL).unwrap_or_default(),
            Err(_) => true,
        }
    })
}

/// One background fire: the default logins (skipping live-fresh and
/// backed-off ones), then idle wallet logins whose reading is stale.
/// Returns whether it probed. Air-gap skips it.
fn background_tick() -> bool {
    if airgap_on() {
        return false;
    }
    if let Err(e) = probe_singleflight(io_for_probe().as_ref(), None, Mode::Background) {
        k2_core::log_debug!("[usage] subscription probe failed: {e}");
    }
    #[cfg(not(test))]
    crate::llm_accounts_runtime::probe_idle_stale(PROBE_INTERVAL);
    true
}

// ── Single flight (B19) ─────────────────────────────────────────────

/// Who asked: the background loop honours every back-off and skips a
/// login with a fresh live reading; a person's refresh only waits out a
/// 429 `Retry-After`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Background,
    Manual,
}

type FlightResult = (Instant, Option<String>, Result<SubscriptionFile, String>);

fn flight_gate() -> &'static Mutex<()> {
    static GATE: Mutex<()> = Mutex::new(());
    &GATE
}

fn flight_last() -> &'static Mutex<Option<FlightResult>> {
    static LAST: Mutex<Option<FlightResult>> = Mutex::new(None);
    &LAST
}

/// One probe at a time. A caller that waited on a probe which finished
/// after it asked (covering what it wants) takes that result instead of
/// starting another.
fn probe_singleflight(io: &dyn UsageIo, only: Option<&str>, mode: Mode) -> Result<SubscriptionFile, String> {
    let asked = Instant::now();
    let _gate = flight_gate().lock().unwrap_or_else(|p| p.into_inner());
    if let Some((done, done_only, res)) = flight_last().lock().unwrap_or_else(|p| p.into_inner()).clone() {
        if done >= asked && (done_only.is_none() || done_only.as_deref() == only) {
            return res;
        }
    }
    let res = probe_and_write_mode(io, only, mode);
    *flight_last().lock().unwrap_or_else(|p| p.into_inner()) = Some((Instant::now(), only.map(str::to_string), res.clone()));
    res
}

fn background_running() -> &'static AtomicBool {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    &RUNNING
}

#[cfg(test)]
fn kicks() -> &'static AtomicUsize {
    static KICKS: AtomicUsize = AtomicUsize::new(0);
    &KICKS
}

/// Start one background re-check unless one is running. (Tests count the
/// kick instead of starting a thread.)
fn kick_background_probe() {
    if airgap_on() || background_running().swap(true, Ordering::SeqCst) {
        return;
    }
    #[cfg(test)]
    {
        kicks().fetch_add(1, Ordering::SeqCst);
        background_running().store(false, Ordering::SeqCst);
    }
    #[cfg(not(test))]
    {
        let spawned = std::thread::Builder::new().name("usage-recheck".into()).spawn(|| {
            if let Err(e) = probe_singleflight(io_for_probe().as_ref(), None, Mode::Background) {
                k2_core::log_debug!("[usage] re-check after a reset failed: {e}");
            }
            background_running().store(false, Ordering::SeqCst);
        });
        if spawned.is_err() {
            background_running().store(false, Ordering::SeqCst);
        }
    }
}

/// Start one background re-check of the default logins (the limits
/// document's GET found a window past its reset).
pub(crate) fn kick_recheck() {
    kick_background_probe();
}

fn any_reset_passed(doc: &SubscriptionFile) -> bool {
    doc.harnesses.iter().any(|h| h.windows.iter().any(|w| w.reset_passed))
}

// ── Back-off ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Backoff {
    failures: u32,
    until: DateTime<Utc>,
    rate_limited: bool,
}

fn backoffs() -> &'static Mutex<std::collections::HashMap<String, Backoff>> {
    static MAP: OnceLock<Mutex<std::collections::HashMap<String, Backoff>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// The longest failure back-off.
const BACKOFF_CAP: Duration = Duration::from_secs(2 * 3600);

/// `retry-after <secs>` in a probe error (the HTTP header, carried by
/// [`LiveIo::claude_get`]).
fn retry_after_secs(err: &str) -> Option<i64> {
    let lower = err.to_ascii_lowercase();
    let at = lower.find("retry-after")?;
    let digits: String = lower[at + "retry-after".len()..]
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Record a check's outcome for `key` (a harness, or `acct:<id>`): a
/// success clears the back-off; a failure doubles it from 15 minutes up to
/// two hours, and a 429 `Retry-After` wins when longer.
pub(crate) fn note_outcome(key: &str, row: &HarnessUsage, now: DateTime<Utc>) {
    let mut map = backoffs().lock().unwrap_or_else(|p| p.into_inner());
    let Some(err) = row.error.as_deref() else {
        map.remove(key);
        return;
    };
    let failures = map.get(key).map_or(0, |b| b.failures).saturating_add(1);
    let step = PROBE_INTERVAL.as_secs().saturating_mul(1u64 << (failures - 1).min(8));
    let step = step.min(BACKOFF_CAP.as_secs()) as i64;
    let mut until = now + chrono::Duration::seconds(step);
    // The vendor said when to come back: even a person's refresh waits.
    let rate_limited = err.contains("429") && retry_after_secs(err).is_some();
    if let Some(secs) = retry_after_secs(err) {
        let ra = now + chrono::Duration::seconds(secs);
        if ra > until {
            until = ra;
        }
    }
    map.insert(key.to_string(), Backoff { failures, until, rate_limited });
}

/// `(until, rate_limited)` while `key` is backing off.
pub(crate) fn backing_off(key: &str, now: DateTime<Utc>) -> Option<(DateTime<Utc>, bool)> {
    let map = backoffs().lock().unwrap_or_else(|p| p.into_inner());
    map.get(key).filter(|b| b.until > now).map(|b| (b.until, b.rate_limited))
}

/// A reading from a running session younger than `max_age`.
pub(crate) fn live_fresh(row: &HarnessUsage, now: DateTime<Utc>, max_age: Duration) -> bool {
    if !row.source.starts_with("live") {
        return false;
    }
    DateTime::parse_from_rfc3339(&row.checked_at)
        .map(|t| now.signed_duration_since(t.with_timezone(&Utc)) < chrono::Duration::from_std(max_age).unwrap_or_default())
        .unwrap_or(false)
}

// ── Cache writes ────────────────────────────────────────────────────

/// Serializes read-modify-write of the cache file (a probe's final write
/// and a live feed's row update).
fn cache_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// The cached rows (known harnesses), undecorated. Empty when missing or
/// unreadable.
pub fn cached_rows() -> Vec<HarnessUsage> {
    match load_cache() {
        Loaded::Ready(doc) => filter_known(doc).harnesses,
        _ => Vec::new(),
    }
}

/// Replace one harness's cache row with `f(old)` (`None` = keep). Returns
/// whether it wrote. An unreadable cache is left alone.
pub(crate) fn update_cache_row(harness: &str, f: impl FnOnce(Option<&HarnessUsage>) -> Option<HarnessUsage>) -> bool {
    if !is_known_harness(harness) {
        return false;
    }
    let _g = cache_lock();
    let mut doc = match load_cache() {
        Loaded::Ready(doc) => doc,
        Loaded::Missing => SubscriptionFile { harnesses: Vec::new() },
        Loaded::Unreadable(_) => return false,
    };
    let prev = doc.harnesses.iter().find(|h| h.harness == harness).cloned();
    let Some(row) = f(prev.as_ref()) else { return false };
    match doc.harnesses.iter_mut().find(|h| h.harness == harness) {
        Some(slot) => *slot = row,
        None => {
            doc.harnesses.push(row);
            doc.harnesses.sort_by_key(|h| PROBED_HARNESSES.iter().position(|n| *n == h.harness).unwrap_or(usize::MAX));
        }
    }
    match write_cache(&doc) {
        Ok(()) => true,
        Err(e) => {
            k2_core::log_debug!("[usage] live reading not cached: {e}");
            false
        }
    }
}

// ── Serving ─────────────────────────────────────────────────────────

/// Why a row has (no) numbers (P8).
pub fn derive_state(h: &HarnessUsage) -> &'static str {
    match h.status.as_str() {
        STATUS_AIRGAP => "airgap",
        STATUS_NOT_SIGNED_IN => "not_signed_in",
        STATUS_SIGN_IN_EXPIRED => "expired",
        STATUS_NOT_SUPPORTED => "unsupported",
        _ if h.error.is_some() => "probe_failed",
        _ if h.windows.is_empty() => "no_window",
        _ => "ok",
    }
}

/// Fill the served fields: `state`, `source`, `usedPercent`,
/// `windowStart`, `resetPassed`, `nextProbeAt`.
pub fn decorate_row(mut h: HarnessUsage, now: DateTime<Utc>, backoff_key: &str) -> HarnessUsage {
    h.state = derive_state(&h).to_string();
    if h.source.is_empty() && !h.checked_at.is_empty() {
        h.source = "probe".into();
    }
    h.next_probe_at = backing_off(backoff_key, now).map(|(t, _)| t.to_rfc3339());
    for w in &mut h.windows {
        w.used_percent = (w.used * 10_000.0).round() / 100.0;
        let reset = DateTime::parse_from_rfc3339(&w.resets_at).ok().map(|t| t.with_timezone(&Utc));
        w.reset_passed = reset.is_some_and(|t| t <= now);
        w.window_start = match (reset, w.window_secs) {
            (Some(t), Some(secs)) if secs > 0 => Some((t - chrono::Duration::seconds(secs)).to_rfc3339()),
            _ => None,
        };
    }
    h
}

fn decorate_file(doc: SubscriptionFile, now: DateTime<Utc>) -> SubscriptionFile {
    SubscriptionFile {
        harnesses: doc.harnesses.into_iter().map(|h| {
            let key = h.harness.clone();
            decorate_row(h, now, &key)
        }).collect(),
    }
}

fn io_for_probe() -> Arc<dyn UsageIo> {
    if let Some(io) = test_io_slot()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
    {
        return io;
    }
    Arc::new(LiveIo)
}

fn ok_doc(doc: &SubscriptionFile) -> CliResponse {
    match serde_json::to_string(doc) {
        Ok(body) => CliResponse::ok_json(body),
        Err(e) => CliResponse::internal_error(format!("subscriptions json: {e}")),
    }
}

fn is_known_harness(name: &str) -> bool {
    PROBED_HARNESSES.contains(&name)
}

fn filter_known(doc: SubscriptionFile) -> SubscriptionFile {
    SubscriptionFile {
        harnesses: doc
            .harnesses
            .into_iter()
            .filter(|h| is_known_harness(&h.harness))
            .collect(),
    }
}

fn fresh_cache(now: DateTime<Utc>) -> Option<SubscriptionFile> {
    let doc = read_cache_file().ok()?;
    if !is_fresh(&doc, now) {
        return None;
    }
    Some(filter_known(doc))
}

fn is_fresh(doc: &SubscriptionFile, now: DateTime<Utc>) -> bool {
    for name in PROBED_HARNESSES {
        let Some(h) = doc.harnesses.iter().find(|h| h.harness == name) else {
            return false;
        };
        // A failed check is "fresh" by when it was tried, so a harness
        // that keeps failing doesn't make every refresh probe again.
        let when = h.attempted_at.as_deref().unwrap_or(&h.checked_at);
        let Ok(checked) = DateTime::parse_from_rfc3339(when) else {
            return false;
        };
        let age = now.signed_duration_since(checked.with_timezone(&Utc));
        if age > chrono::Duration::from_std(FRESH_FOR).unwrap_or(chrono::Duration::seconds(15)) {
            return false;
        }
    }
    true
}

enum Loaded {
    Missing,
    Ready(SubscriptionFile),
    Unreadable(String),
}

fn load_cache() -> Loaded {
    let path = cache_path();
    if !path.exists() {
        return Loaded::Missing;
    }
    if !path.is_file() {
        return Loaded::Unreadable("subscriptions cache is not a file".into());
    }
    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<SubscriptionFile>(&text) {
            Ok(doc) => Loaded::Ready(doc),
            Err(e) => Loaded::Unreadable(format!("subscriptions cache: {e}")),
        },
        Err(e) => Loaded::Unreadable(format!("subscriptions cache: {e}")),
    }
}

fn read_cache_file() -> Result<SubscriptionFile, String> {
    match load_cache() {
        Loaded::Ready(doc) => Ok(doc),
        Loaded::Missing => Err("missing".into()),
        Loaded::Unreadable(e) => Err(e),
    }
}

/// Probe Claude, Codex, and Grok, then atomic-replace the cache.
/// A failed read of an existing file does not truncate it.
#[cfg(test)]
fn probe_and_write(io: &dyn UsageIo) -> Result<SubscriptionFile, String> {
    probe_and_write_only(io, None)
}

/// Which wallet token is in a tool's live store, and since when. Read
/// under the DB lock: a switch holds that lock from its live write to its
/// pointer update, so the same answer before and after a probe means no
/// switch happened in between. `None` without a wallet entry.
fn wallet_generation(harness: &str) -> Option<(String, i64)> {
    use k2_core::llm_accounts as w;
    let tool = w::Tool::parse(harness)?;
    let db = k2_core::db::try_shared()?;
    let conn = db.lock();
    let owner = w::live_owner(&conn, tool).ok()??;
    let info = w::active_info(&conn, tool).ok()??;
    Some((owner, info.switched_at))
}

/// The last reading of a wallet token (its own row, never another
/// token's): what a failed check keeps.
pub(crate) fn wallet_row(account_id: &str) -> Option<HarnessUsage> {
    let db = k2_core::db::try_shared()?;
    let conn = db.lock();
    let e = k2_core::llm_accounts::get(&conn, account_id).ok()??;
    serde_json::from_str(e.usage_json.as_deref()?).ok()
}

/// Store a reading on the wallet token it belongs to (Settings → LLMs
/// shows each token's own numbers and their age). A failed check keeps the
/// token's `usage_checked_at`.
pub(crate) fn record_on_wallet(account_id: &str, row: &HarnessUsage) {
    let Some(db) = k2_core::db::try_shared() else { return };
    let Ok(text) = serde_json::to_string(row) else { return };
    let conn = db.lock();
    let checked = if row.error.is_none() {
        DateTime::parse_from_rfc3339(&row.checked_at).ok().map(|d| d.timestamp())
    } else {
        None
    };
    let res = k2_core::llm_accounts::update_meta(
        &conn,
        account_id,
        &k2_core::llm_accounts::MetaUpdate { usage_json: Some(text), usage_checked_at: checked, ..Default::default() },
    );
    if let Err(e) = res {
        k2_core::log_debug!("[usage] could not record usage on {account_id}: {e}");
    }
}

/// Probe now, ignoring the 15 s cache: one tool (after a switch) or all.
pub fn probe_now(harness: Option<&str>) -> Result<SubscriptionFile, String> {
    if airgap_on() {
        return Err(k2_core::airgap::TEACHING.to_string());
    }
    probe_singleflight(io_for_probe().as_ref(), harness, Mode::Manual)
}

/// `only`: probe that harness and keep the others' rows as they are.
#[cfg(test)]
fn probe_and_write_only(io: &dyn UsageIo, only: Option<&str>) -> Result<SubscriptionFile, String> {
    probe_and_write_mode(io, only, Mode::Manual)
}

/// Why a harness is not probed this time (its cached row is kept).
fn skip_reason(cached: Option<&HarnessUsage>, owner: Option<&str>, name: &str, mode: Mode, now: DateTime<Utc>) -> Option<String> {
    let row = cached?;
    // Only skip when the cached row is this token's own.
    if row.account_id.as_deref() != owner {
        return None;
    }
    if let Some((until, rate_limited)) = backing_off(name, now) {
        if mode == Mode::Background || rate_limited {
            return Some(format!("backing off until {}", until.to_rfc3339()));
        }
    }
    let fresh_for = match mode {
        Mode::Background => PROBE_INTERVAL,
        Mode::Manual => Duration::from_secs(60),
    };
    // A window past its reset needs a new reading even if a session
    // reported lately (the re-check a GET kicks, B18).
    let reset_passed = row.windows.iter().any(|w| {
        DateTime::parse_from_rfc3339(&w.resets_at).is_ok_and(|t| t.with_timezone(&Utc) <= now)
    });
    if !reset_passed && live_fresh(row, now, fresh_for) {
        return Some("a running session reported it".into());
    }
    None
}

fn probe_and_write_mode(io: &dyn UsageIo, only: Option<&str>, mode: Mode) -> Result<SubscriptionFile, String> {
    // Belt for any caller that missed the route/loop gate.
    if airgap_on() {
        return Err(k2_core::airgap::TEACHING.to_string());
    }
    probe_hits().fetch_add(1, Ordering::SeqCst);
    let now_wall = SystemTime::now();
    let now = Utc::now();
    let previous = match load_cache() {
        Loaded::Missing => SubscriptionFile {
            harnesses: Vec::new(),
        },
        Loaded::Ready(doc) => doc,
        Loaded::Unreadable(e) => return Err(e),
    };
    let cached = |name: &str| previous.harnesses.iter().find(|h| h.harness == name).cloned();
    let mut rows = Vec::new();
    for name in PROBED_HARNESSES {
        if only.is_some_and(|o| o != name) {
            if let Some(row) = cached(name) {
                rows.push(row);
            }
            continue;
        }
        let gen_before = wallet_generation(name);
        let owner = gen_before.as_ref().map(|g| g.0.as_str());
        if let Some(why) = skip_reason(cached(name).as_ref(), owner, name, mode, now) {
            k2_core::log_debug!("[usage] {name}: not probed ({why})");
            if let Some(row) = cached(name) {
                rows.push(row);
            }
            continue;
        }
        // A failed check keeps THIS token's last reading: the token's own
        // row when the wallet knows which token is live, else (no wallet
        // entry) the cache row, only if it isn't some other token's.
        let prev = match &gen_before {
            Some((owner, _)) => wallet_row(owner).or_else(|| cached(name).filter(|r| r.account_id.as_deref() == Some(owner.as_str()))),
            None => cached(name).filter(|r| r.account_id.is_none()),
        };
        let mut row = match name {
            HARNESS_CLAUDE => probe_claude(io.claude_login(), |token| io.claude_get(token), prev.as_ref(), now_wall, now),
            HARNESS_CODEX => probe_codex(io.codex(), prev.as_ref(), now),
            _ => probe_grok(io.grok(), prev.as_ref(), now),
        };
        row.source = "probe".into();
        note_outcome(name, &row, now);
        let gen_after = wallet_generation(name);
        match (&gen_before, &gen_after) {
            (Some(b), Some(a)) if a == b => {
                row.account_id = Some(b.0.clone());
                record_on_wallet(&b.0, &row);
            }
            (Some(_), _) => {
                // A switch ran during the probe: the reading may be either
                // token's. Show nothing as fresh; the switch re-checks.
                k2_core::log_debug!("[usage] {name}: the server default changed during the check; reading dropped");
                row = preserved(name, None, "", &row.plan, now, Some("the server default changed during the check".into()));
            }
            _ => {}
        }
        rows.push(row);
    }
    let mut doc = SubscriptionFile { harnesses: rows };
    {
        // A live feed may have written a newer reading while the probe ran
        // (it held no lock): keep that one.
        let _g = cache_lock();
        if let Loaded::Ready(current) = load_cache() {
            for row in &mut doc.harnesses {
                let Some(cur) = current.harnesses.iter().find(|h| h.harness == row.harness) else { continue };
                let cur_t = DateTime::parse_from_rfc3339(&cur.checked_at).ok();
                let row_t = DateTime::parse_from_rfc3339(&row.checked_at).ok();
                let newer = matches!((cur_t, row_t), (Some(c), Some(r)) if c > r) || (cur_t.is_some() && row_t.is_none());
                if cur.source.starts_with("live") && newer && cur.account_id == row.account_id {
                    *row = cur.clone();
                }
            }
        }
        write_cache(&doc)?;
    }
    crate::session_events::emit_subscription_usage_changed(only, None, "probe");
    Ok(doc)
}

fn probe_claude(
    login: ClaudeLogin,
    mut get: impl FnMut(&str) -> Result<String, String>,
    prev: Option<&HarnessUsage>,
    now_wall: SystemTime,
    now: DateTime<Utc>,
) -> HarnessUsage {
    let ClaudeLogin::Token {
        access_token,
        expires_at,
        plan,
    } = login
    else {
        return preserved(HARNESS_CLAUDE, prev, STATUS_NOT_SIGNED_IN, "", now, None);
    };
    if access_token.is_empty() {
        return preserved(HARNESS_CLAUDE, prev, STATUS_NOT_SIGNED_IN, &plan, now, None);
    }
    if !access_token_usable(expires_at, now_wall) {
        return preserved(
            HARNESS_CLAUDE,
            prev,
            STATUS_SIGN_IN_EXPIRED,
            &plan,
            now,
            Some("the access token expired before the check".into()),
        );
    }
    match get(&access_token) {
        Ok(body) => match parse_claude_usage(&body) {
            Ok(windows) => {
                let status = if windows.is_empty() {
                    STATUS_NO_WINDOW.to_string()
                } else {
                    String::new()
                };
                HarnessUsage {
                    error: None,
                    account_id: None,
                    attempted_at: None,
                    harness: HARNESS_CLAUDE.into(),
                    plan,
                    windows,
                    checked_at: now.to_rfc3339(),
                    status,
                    ..Default::default()
                }
            }
            Err(_) => preserved(HARNESS_CLAUDE, prev, prev_status(prev), &plan, now, Some("the usage answer was unreadable".into())),
        },
        Err(e) => preserved(HARNESS_CLAUDE, prev, prev_status(prev), &plan, now, Some(safe_error(&e))),
    }
}

/// A probe error as shown to people: status codes and transport words
/// only (the callers never put a body in it), capped.
fn safe_error(e: &str) -> String {
    let s: String = e.chars().filter(|c| !c.is_control()).take(120).collect();
    if k2_core::llm_accounts::looks_like_secret(&s) {
        "the check failed".into()
    } else {
        s
    }
}

fn probe_codex(
    outcome: CodexOutcome,
    prev: Option<&HarnessUsage>,
    now: DateTime<Utc>,
) -> HarnessUsage {
    match outcome {
        CodexOutcome::NotSignedIn => HarnessUsage {
            error: None,
            account_id: None,
            attempted_at: None,
            harness: HARNESS_CODEX.into(),
            plan: String::new(),
            windows: Vec::new(),
            checked_at: now.to_rfc3339(),
            status: STATUS_NOT_SIGNED_IN.into(),
            ..Default::default()
        },
        CodexOutcome::Failed => preserved(HARNESS_CODEX, prev, prev_status(prev), "", now, Some("codex did not answer".into())),
        CodexOutcome::Ready { plan, windows } => {
            let status = if windows.is_empty() {
                STATUS_NO_WINDOW.to_string()
            } else {
                String::new()
            };
            HarnessUsage {
                error: None,
                account_id: None,
                attempted_at: None,
                harness: HARNESS_CODEX.into(),
                plan,
                windows,
                checked_at: now.to_rfc3339(),
                status,
                ..Default::default()
            }
        }
    }
}

fn probe_grok(
    outcome: GrokOutcome,
    prev: Option<&HarnessUsage>,
    now: DateTime<Utc>,
) -> HarnessUsage {
    match outcome {
        GrokOutcome::NotSignedIn => HarnessUsage {
            error: None,
            account_id: None,
            attempted_at: None,
            harness: HARNESS_GROK.into(),
            plan: String::new(),
            windows: Vec::new(),
            checked_at: now.to_rfc3339(),
            status: STATUS_NOT_SIGNED_IN.into(),
            ..Default::default()
        },
        GrokOutcome::Failed => preserved(HARNESS_GROK, prev, prev_status(prev), "", now, Some("grok did not answer".into())),
        GrokOutcome::Ready { plan, windows } => {
            let status = if windows.is_empty() {
                STATUS_NO_WINDOW.to_string()
            } else {
                String::new()
            };
            HarnessUsage {
                error: None,
                account_id: None,
                attempted_at: None,
                harness: HARNESS_GROK.into(),
                plan,
                windows,
                checked_at: now.to_rfc3339(),
                status,
                ..Default::default()
            }
        }
    }
}

fn prev_status(prev: Option<&HarnessUsage>) -> &str {
    prev.map(|p| p.status.as_str()).unwrap_or("")
}

/// The previous reading's still-open windows. `error: None` = a state K2
/// knows now (not signed in): stamped now. `error: Some` = the check
/// failed: the last good reading keeps ITS time and says why, so old
/// numbers are never shown as just checked.
fn preserved(
    harness: &str,
    prev: Option<&HarnessUsage>,
    status: &str,
    plan: &str,
    now: DateTime<Utc>,
    error: Option<String>,
) -> HarnessUsage {
    let windows = prev
        .map(|p| {
            p.windows
                .iter()
                .filter(|w| window_still_open(w, now))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let plan = if !plan.is_empty() {
        plan.to_string()
    } else {
        prev.map(|p| p.plan.clone()).unwrap_or_default()
    };
    let (checked_at, attempted_at) = match &error {
        None => (now.to_rfc3339(), None),
        Some(_) => (prev.map(|p| p.checked_at.clone()).unwrap_or_default(), Some(now.to_rfc3339())),
    };
    HarnessUsage {
        error,
        account_id: None,
        attempted_at,
        harness: harness.to_string(),
        plan,
        windows,
        checked_at,
        status: status.to_string(),
        ..Default::default()
    }
}

fn window_still_open(window: &UsageWindow, now: DateTime<Utc>) -> bool {
    if window.resets_at.is_empty() {
        return true;
    }
    match DateTime::parse_from_rfc3339(&window.resets_at) {
        Ok(t) => t.with_timezone(&Utc) > now,
        Err(_) => true,
    }
}

/// Still-valid access token. Millisecond expiries compare to milliseconds.
/// The five-minute expiring buffer is still valid and may GET. Never refreshes.
fn access_token_usable(expires_at: Option<i64>, now: SystemTime) -> bool {
    let Some(exp) = expires_at else {
        return true;
    };
    if exp <= 0 {
        return false;
    }
    let now_secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let now_ms = now_secs.saturating_mul(1000);
    if exp > EXPIRY_UNIT_THRESHOLD {
        exp > now_ms
    } else {
        exp > now_secs
    }
}

fn plan_label(tier: &str, subscription: &str) -> String {
    let lower = tier.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("max_") {
        if let Some(digits) = rest.strip_suffix('x') {
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                return format!("Max {rest}");
            }
        }
    }
    if !subscription.is_empty() {
        let mut chars = subscription.chars();
        if let Some(first) = chars.next() {
            return format!("{}{}", first.to_uppercase(), chars.as_str());
        }
    }
    if !tier.is_empty() {
        return tier.to_string();
    }
    String::new()
}

fn parse_claude_login(raw: &str) -> ClaudeLogin {
    let json: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return ClaudeLogin::Missing,
    };
    let Some(oauth) = json.get("claudeAiOauth") else {
        return ClaudeLogin::Missing;
    };
    let access = oauth
        .get("accessToken")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if access.is_empty() {
        return ClaudeLogin::Missing;
    }
    let expires_at = oauth.get("expiresAt").and_then(|v| v.as_i64());
    let tier = oauth
        .get("rateLimitTier")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let sub = oauth
        .get("subscriptionType")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    ClaudeLogin::Token {
        access_token: access.to_string(),
        expires_at,
        plan: plan_label(tier, sub),
    }
}

/// Keychain first on macOS, then `CLAUDE_CONFIG_DIR` when set, then
/// `~/.claude/.credentials.json`. An empty access token is not a login.
/// A missing refresh token does not reject a present access token.
fn login_from_sources(keychain: Option<&str>, files: &[Option<&str>]) -> ClaudeLogin {
    if let Some(raw) = keychain {
        let login = parse_claude_login(raw);
        if matches!(login, ClaudeLogin::Token { .. }) {
            return login;
        }
    }
    for raw in files.iter().flatten() {
        let login = parse_claude_login(raw);
        if matches!(login, ClaudeLogin::Token { .. }) {
            return login;
        }
    }
    ClaudeLogin::Missing
}

fn credential_file_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            paths.push(PathBuf::from(dir).join(".credentials.json"));
        }
    }
    if let Some(home) = dirs::home_dir() {
        let fallback = home.join(".claude").join(".credentials.json");
        if !paths.iter().any(|p| p == &fallback) {
            paths.push(fallback);
        }
    }
    paths
}

/// Window lengths, for the pace mark.
const FIVE_HOURS_SECS: i64 = 5 * 3600;
const WEEK_SECS: i64 = 7 * 86_400;
const MONTH_SECS: i64 = 30 * 86_400;
const DAY_SECS: i64 = 86_400;

/// A window with its id, kind and length.
pub fn window(id: &str, label: &str, kind: &str, used: f64, resets_at: String, window_secs: Option<i64>) -> UsageWindow {
    UsageWindow {
        id: id.to_string(),
        label: label.to_string(),
        kind: kind.to_string(),
        used,
        resets_at,
        window_secs,
        ..Default::default()
    }
}

fn kind_secs(kind: &str) -> Option<i64> {
    match kind {
        "session" => Some(FIVE_HOURS_SECS),
        "weekly" => Some(WEEK_SECS),
        "monthly" => Some(MONTH_SECS),
        "daily" => Some(DAY_SECS),
        _ => None,
    }
}

/// Claude `api/oauth/usage` (P7): `utilization` is always a percent
/// (B5: 0.4 is 0.4 %, never 40 %). Weekly is `seven_day`, the all-models
/// week `/usage` shows (B6); the third-party-app allowance
/// `seven_day_oauth_apps` is its own row, only when it is above 0.
fn parse_claude_usage(body: &str) -> Result<Vec<UsageWindow>, String> {
    let payload: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let mut windows = Vec::new();
    let buckets = [
        ("five_hour", "Session", "session", false),
        ("seven_day", "Weekly", "weekly", false),
        ("seven_day_oauth_apps", "Weekly (apps)", "weekly", true),
    ];
    for (key, label, kind, only_when_used) in buckets {
        let Some(bucket) = object_bucket(&payload, key) else { continue };
        let Some(used) = normalize_used(util_number(bucket.get("utilization"))) else { continue };
        if only_when_used && used <= 0.0 {
            continue;
        }
        windows.push(window(key, label, kind, used, reset_of(bucket), kind_secs(kind)));
    }
    let limits = payload.get("limits").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let mut seen = std::collections::BTreeSet::new();
    for entry in &limits {
        let Some(name) = scoped_model_name(entry) else {
            continue;
        };
        let kind = entry.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if !seen.insert((name.clone(), kind.to_string())) {
            continue;
        }
        let Some(used) = normalize_used(util_number(entry.get("percent").or_else(|| entry.get("utilization")))) else {
            continue;
        };
        let k = scoped_kind(kind);
        windows.push(window(
            &format!("model:{}:{}", name.to_ascii_lowercase(), if k.is_empty() { "other" } else { k }),
            &scoped_title(&name, kind),
            if k.is_empty() { "other" } else { k },
            used,
            reset_of(entry),
            kind_secs(k),
        ));
    }
    Ok(windows)
}

fn object_bucket<'a>(payload: &'a Value, key: &str) -> Option<&'a Value> {
    payload.get(key).filter(|v| v.is_object())
}

fn util_number(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    if let Some(n) = value.as_f64() {
        return Some(n);
    }
    let text = value.as_str()?.trim().trim_end_matches('%');
    text.parse::<f64>().ok()
}

/// A vendor percent (0..100) as a fraction. Never guesses the scale.
fn normalize_used(raw: Option<f64>) -> Option<f64> {
    let n = raw?;
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    Some((n / 100.0).clamp(0.0, 1.0))
}

fn scoped_model_name(entry: &Value) -> Option<String> {
    let model = entry.pointer("/scope/model")?;
    let name = model
        .get("display_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            model
                .get("id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
        })?;
    Some(name.trim().to_string())
}

fn scoped_kind(kind: &str) -> &'static str {
    let text = kind.to_ascii_lowercase();
    if text.contains("month") {
        "monthly"
    } else if text.contains("week") || text.contains("day") {
        "weekly"
    } else if text.contains("hour") || text.contains("session") {
        "session"
    } else {
        ""
    }
}

fn scoped_title(name: &str, kind: &str) -> String {
    let window = match scoped_kind(kind) {
        "monthly" => "Monthly",
        "weekly" => "Weekly",
        "session" => "Session",
        _ => "",
    };
    if window.is_empty() {
        name.to_string()
    } else {
        format!("{name} {window}")
    }
}

fn reset_of(bucket: &Value) -> String {
    let value = bucket
        .get("resets_at")
        .or_else(|| bucket.get("resetsAt"))
        .unwrap_or(&Value::Null);
    reset_to_rfc3339(value)
}

fn reset_to_rfc3339(value: &Value) -> String {
    if let Some(n) = value.as_i64() {
        return unix_to_rfc3339(n);
    }
    if let Some(n) = value.as_f64() {
        if n.is_finite() {
            return unix_to_rfc3339(n as i64);
        }
    }
    if let Some(text) = value.as_str() {
        let text = text.trim();
        if text.is_empty() {
            return String::new();
        }
        if let Ok(n) = text.parse::<i64>() {
            return unix_to_rfc3339(n);
        }
        return text.to_string();
    }
    String::new()
}

fn unix_to_rfc3339(n: i64) -> String {
    let ms = if n > EXPIRY_UNIT_THRESHOLD {
        n
    } else {
        n.saturating_mul(1000)
    };
    DateTime::<Utc>::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

fn codex_label(mins: i64) -> String {
    if mins == 10080 {
        "Weekly".into()
    } else if mins > 0 && mins % 60 == 0 {
        format!("{}h", mins / 60)
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        "Limit".into()
    }
}

fn kind_of_secs(secs: i64) -> &'static str {
    if secs >= MONTH_SECS - DAY_SECS {
        "monthly"
    } else if secs >= WEEK_SECS {
        "weekly"
    } else if secs >= DAY_SECS {
        "daily"
    } else if secs > 0 {
        "session"
    } else {
        "other"
    }
}

/// One Codex window. Reads the app-server shape (`usedPercent`,
/// `windowDurationMins`, `resetsAt`) and the session-log shape
/// (`used_percent`, `window_minutes`, `resets_at` or `resets_in_seconds`
/// from `observed_ms`).
pub(crate) fn codex_window(id: &str, window: &Value, observed_ms: Option<i64>) -> Option<UsageWindow> {
    if !window.is_object() {
        return None;
    }
    let used_percent = window.get("usedPercent").or_else(|| window.get("used_percent"))?.as_f64()?;
    let mins = window
        .get("windowDurationMins")
        .or_else(|| window.get("window_minutes"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let mut resets_at = window
        .get("resetsAt")
        .or_else(|| window.get("resets_at"))
        .map(reset_to_rfc3339)
        .unwrap_or_default();
    if resets_at.is_empty() {
        if let (Some(secs), Some(at)) = (
            window.get("resets_in_seconds").or_else(|| window.get("resetsInSeconds")).and_then(|v| v.as_i64()),
            observed_ms,
        ) {
            resets_at = DateTime::<Utc>::from_timestamp_millis(at + secs * 1000).map(|d| d.to_rfc3339()).unwrap_or_default();
        }
    }
    let secs = (mins > 0).then_some(mins * 60);
    Some(window_of(id, &codex_label(mins), kind_of_secs(mins * 60), (used_percent / 100.0).clamp(0.0, 1.0), resets_at, secs))
}

fn window_of(id: &str, label: &str, kind: &str, used: f64, resets_at: String, secs: Option<i64>) -> UsageWindow {
    window(id, label, kind, used, resets_at, secs)
}

/// The `primary` / `secondary` windows and plan of a Codex rate-limit
/// snapshot (app-server `rateLimits` or a session log's `rate_limits`).
pub(crate) fn codex_windows(snapshot: &Value, observed_ms: Option<i64>) -> (String, Vec<UsageWindow>) {
    let plan = snapshot
        .get("planType")
        .or_else(|| snapshot.get("plan_type"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let windows = ["primary", "secondary"]
        .iter()
        .filter_map(|key| snapshot.get(*key).and_then(|w| codex_window(key, w, observed_ms)))
        .collect();
    (plan, windows)
}

fn parse_codex_results(account_msg: &Value, limits_msg: &Value) -> Result<CodexParsed, CodexFail> {
    if account_msg.get("result").is_none() && account_msg.get("error").is_some() {
        return Err(CodexFail::NotSignedIn);
    }
    let account = account_msg.pointer("/result/account");
    let signed_out = match account {
        None | Some(Value::Null) => true,
        Some(_) => false,
    };
    if limits_msg.get("result").is_none() && limits_msg.get("error").is_some() {
        if signed_out {
            return Err(CodexFail::NotSignedIn);
        }
        return Err(CodexFail::Transport("codex rate limits unavailable".into()));
    }
    let limits = limits_msg.pointer("/result/rateLimits");
    if signed_out {
        return Err(CodexFail::NotSignedIn);
    }
    let plan = limits
        .and_then(|v| v.get("planType"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            account
                .and_then(|v| v.get("planType"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .to_string();
    let windows = limits.map(|l| codex_windows(l, None).1).unwrap_or_default();
    Ok(CodexParsed { plan, windows })
}

trait CodexTransport {
    fn write_line(&mut self, line: &str) -> Result<(), String>;
    fn read_matching(&mut self, id: i64, timeout: Duration) -> Result<Value, String>;
}

fn rpc_line(id: i64, method: &str, params: Value) -> Result<String, String> {
    serde_json::to_string(&json!({
        "method": method,
        "id": id,
        "params": params,
    }))
    .map_err(|e| e.to_string())
}

fn codex_line(id: i64, method: &str, params: Value) -> Result<String, CodexFail> {
    rpc_line(id, method, params).map_err(CodexFail::Transport)
}

/// One-shot app-server conversation. No resume, no session id, no login refresh.
fn codex_exchange(io: &mut dyn CodexTransport) -> Result<CodexParsed, CodexFail> {
    let timeout = Duration::from_secs(8);
    io.write_line(&codex_line(
        1,
        "initialize",
        json!({
            "clientInfo": {"name": "k2", "title": "K2", "version": "0"}
        }),
    )?)
    .map_err(CodexFail::Transport)?;
    let _init = io.read_matching(1, timeout).map_err(CodexFail::Transport)?;
    let initialized = serde_json::to_string(&json!({"method": "initialized", "params": {}}))
        .map_err(|e| CodexFail::Transport(e.to_string()))?;
    io.write_line(&initialized).map_err(CodexFail::Transport)?;
    io.write_line(&codex_line(
        2,
        "account/read",
        json!({"refreshToken": false}),
    )?)
    .map_err(CodexFail::Transport)?;
    let account = match io.read_matching(2, timeout) {
        Ok(msg) => msg,
        // A signed-out Codex exits app-server before account/read returns.
        Err(err) if err.contains("closed") => return Err(CodexFail::NotSignedIn),
        Err(err) => return Err(CodexFail::Transport(err)),
    };
    io.write_line(&codex_line(3, "account/rateLimits/read", json!({}))?)
        .map_err(CodexFail::Transport)?;
    let limits = io.read_matching(3, timeout).map_err(CodexFail::Transport)?;
    parse_codex_results(&account, &limits)
}

fn codex_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.arg("app-server");
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());
    cmd
}

struct ProcessTransport {
    stdin: std::process::ChildStdin,
    rx: std::sync::mpsc::Receiver<String>,
}

impl CodexTransport for ProcessTransport {
    fn write_line(&mut self, line: &str) -> Result<(), String> {
        self.stdin
            .write_all(line.as_bytes())
            .map_err(|e| e.to_string())?;
        self.stdin.write_all(b"\n").map_err(|e| e.to_string())?;
        self.stdin.flush().map_err(|e| e.to_string())
    }

    fn read_matching(&mut self, id: i64, timeout: Duration) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("codex app-server timed out".into());
            }
            match self.rx.recv_timeout(left) {
                Ok(line) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let msg: Value = match serde_json::from_str(&line) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if msg.get("id").and_then(|v| v.as_i64()) == Some(id) {
                        return Ok(msg);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    return Err("codex app-server timed out".into());
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("codex app-server closed".into());
                }
            }
        }
    }
}

struct ChildGuard(Option<std::process::Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn map_spawn_err(err: std::io::Error) -> CodexFail {
    if err.kind() == std::io::ErrorKind::NotFound {
        CodexFail::NotSignedIn
    } else {
        CodexFail::Transport(err.to_string())
    }
}

fn run_codex_process(program: &str) -> Result<CodexParsed, CodexFail> {
    run_codex_process_in(program, None, codex_exchange)
}

/// `codex app-server` pointed at a wallet slot (`CODEX_HOME=<slot>`, file
/// credential store so the OS keyring is never involved). `None` = the
/// live home (today's probe).
fn codex_command_in(program: &str, slot: Option<&Path>) -> Command {
    let mut cmd = codex_command(program);
    if let Some(slot) = slot {
        cmd.args(["-c", "cli_auth_credentials_store=\"file\""]);
        cmd.env("CODEX_HOME", slot);
    }
    cmd
}

fn run_codex_process_in<T>(
    program: &str,
    slot: Option<&Path>,
    exchange: fn(&mut dyn CodexTransport) -> Result<T, CodexFail>,
) -> Result<T, CodexFail> {
    let mut child = codex_command_in(program, slot).spawn().map_err(map_spawn_err)?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| CodexFail::Transport("codex stdin missing".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| CodexFail::Transport("codex stdout missing".into()))?;
    let guard = ChildGuard(Some(child));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let mut transport = ProcessTransport { stdin, rx };
    let parsed = exchange(&mut transport);
    drop(transport);
    drop(guard);
    parsed
}

trait GrokTransport {
    fn write_line(&mut self, line: &str) -> Result<(), String>;
    fn read_matching(&mut self, id: i64, timeout: Duration) -> Result<Value, String>;
    fn close_stdin(&mut self) -> Result<(), String>;
}

fn grok_line(id: i64, method: &str, params: Value) -> Result<String, GrokFail> {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .map_err(|e| GrokFail::Transport(e.to_string()))
}

fn grok_notification(method: &str) -> Result<String, GrokFail> {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "method": method,
    }))
    .map_err(|e| GrokFail::Transport(e.to_string()))
}

/// One-shot `grok agent stdio` exchange. Initialize, then `_x.ai/billing`.
/// Stdin closes after that request. No login refresh and no billing URL.
///
/// grok 1.0.41 rejects `clientInfo` alone (`Invalid params`) and rejects
/// `--no-leader`. The handshake that returns `creditUsagePercent` is ACP
/// `protocolVersion` 1, then `notifications/initialized`, then billing.
fn grok_exchange(io: &mut dyn GrokTransport) -> Result<GrokParsed, GrokFail> {
    let timeout = Duration::from_secs(8);
    io.write_line(&grok_line(
        1,
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": {"readTextFile": false, "writeTextFile": false},
                "terminal": false
            },
            "clientInfo": {"name": "k2", "title": "K2", "version": "0"}
        }),
    )?)
    .map_err(GrokFail::Transport)?;
    let init = io.read_matching(1, timeout).map_err(GrokFail::Transport)?;
    if is_grok_auth_message(&init) {
        return Err(GrokFail::NotSignedIn);
    }
    if init.get("error").is_some() {
        return Err(GrokFail::Transport("grok initialize failed".into()));
    }
    io.write_line(&grok_notification("notifications/initialized")?)
        .map_err(GrokFail::Transport)?;
    io.write_line(&grok_line(2, "_x.ai/billing", json!({}))?)
        .map_err(GrokFail::Transport)?;
    // Read the billing result before closing stdin. grok 1.0.41 exits on
    // stdin EOF and drops the reply if the pipe closes first.
    let billing = io.read_matching(2, timeout);
    io.close_stdin().map_err(GrokFail::Transport)?;
    parse_grok_billing(&billing.map_err(GrokFail::Transport)?)
}

fn parse_grok_billing(msg: &Value) -> Result<GrokParsed, GrokFail> {
    if is_grok_auth_message(msg) {
        return Err(GrokFail::NotSignedIn);
    }
    if msg.get("error").is_some() {
        return Err(GrokFail::Transport("grok billing failed".into()));
    }
    let Some(result) = msg.get("result").filter(|v| v.is_object()) else {
        return Err(GrokFail::Transport("grok billing missing result".into()));
    };
    Ok(grok_from_result(result))
}

fn grok_from_result(result: &Value) -> GrokParsed {
    let plan = grok_plan(result);
    let config = result
        .get("config")
        .filter(|v| v.is_object())
        .unwrap_or(result);
    // The billing reply is protobuf-style JSON, which leaves out zero
    // values: right after a reset `creditUsagePercent` is simply absent.
    // A missing percent with a real current period (an object with `type`
    // or `end`) is 0% used. A missing percent with no current period is
    // "no window", not a made-up 0%.
    let has_period = config
        .get("currentPeriod")
        .and_then(|v| v.as_object())
        .is_some_and(|p| p.contains_key("type") || p.contains_key("end"));
    let credit = match util_number(config.get("creditUsagePercent")) {
        Some(n) => Some(n),
        None if has_period => Some(0.0),
        None => None,
    };
    let build = grok_build_percent(result);
    let resets_at = config
        .pointer("/currentPeriod/end")
        .map(reset_to_rfc3339)
        .unwrap_or_default();
    let started = config.pointer("/currentPeriod/start").map(reset_to_rfc3339).unwrap_or_default();
    let period_type = config
        .pointer("/currentPeriod/type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let label = grok_period_label(period_type);
    let secs = match (DateTime::parse_from_rfc3339(&started), DateTime::parse_from_rfc3339(&resets_at)) {
        (Ok(a), Ok(b)) if b > a => Some((b - a).num_seconds()),
        _ => kind_secs(grok_period_kind(period_type)),
    };
    let kind = secs.map(kind_of_secs).unwrap_or_else(|| grok_period_kind(period_type));
    let mut windows = Vec::new();
    // `credit` is None only when there is no percent and no period.
    // Percent always (B5): 0.22 is 0.22 %, never 22 %.
    if let Some(used) = normalize_used(credit) {
        windows.push(window("credit", &label, kind, used, resets_at.clone(), secs));
    }
    if credit.is_some() {
        if let Some(used) = normalize_used(build) {
            windows.push(window("grok_build", "Grok Build", kind, used, resets_at, secs));
        }
    }
    GrokParsed { plan, windows }
}

fn grok_period_kind(kind: &str) -> &'static str {
    let text = kind.to_ascii_uppercase();
    if text.contains("WEEK") || text.is_empty() {
        "weekly"
    } else if text.contains("MONTH") {
        "monthly"
    } else if text.contains("DAY") {
        "daily"
    } else if text.contains("HOUR") {
        "session"
    } else {
        "other"
    }
}

fn grok_plan(result: &Value) -> String {
    for key in ["subscription_tier", "subscriptionTier"] {
        if let Some(text) = json_string(result, key) {
            return text;
        }
    }
    if let Some(config) = result.get("config") {
        for key in ["subscription_tier", "subscriptionTier"] {
            if let Some(text) = json_string(config, key) {
                return text;
            }
        }
    }
    String::new()
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Grok Build percent. `GrokBuild` present without `usagePercent` is 0%,
/// because protobuf-style JSON leaves out zero values. No `GrokBuild` at
/// all is no Build window: we can't tell whether the plan has Build.
fn grok_build_percent(result: &Value) -> Option<f64> {
    let build = result
        .pointer("/config/productUsage/GrokBuild")
        .or_else(|| result.pointer("/productUsage/GrokBuild"))
        .filter(|v| v.is_object())?;
    match build.get("usagePercent") {
        Some(value) => util_number(Some(value)),
        None => Some(0.0),
    }
}

fn grok_period_label(kind: &str) -> String {
    let text = kind.to_ascii_uppercase();
    if text.contains("WEEK") || text.is_empty() {
        "Weekly".into()
    } else if text.contains("MONTH") {
        "Monthly".into()
    } else if text.contains("DAY") {
        "Daily".into()
    } else if text.contains("HOUR") {
        "Hourly".into()
    } else {
        "Limit".into()
    }
}

fn is_grok_auth_message(msg: &Value) -> bool {
    let Some(error) = msg.get("error") else {
        return false;
    };
    if error.is_null() {
        return false;
    }
    grok_auth_error(error)
}

fn grok_auth_error(error: &Value) -> bool {
    if let Some(text) = error.as_str() {
        return grok_auth_words(text);
    }
    if matches!(error.get("code").and_then(|c| c.as_i64()), Some(401 | 403)) {
        return true;
    }
    if error
        .get("message")
        .and_then(|m| m.as_str())
        .is_some_and(grok_auth_words)
    {
        return true;
    }
    let Some(data) = error.get("data") else {
        return false;
    };
    if data.as_str().is_some_and(grok_auth_words) {
        return true;
    }
    data.get("message")
        .and_then(|m| m.as_str())
        .is_some_and(grok_auth_words)
}

fn grok_auth_words(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("auth")
        || lower.contains("forbidden")
        || lower.contains("sign in")
        || lower.contains("signed out")
        || lower.contains("not logged")
        || lower.contains("login required")
        || lower.contains("credential")
}

fn grok_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(["agent", "stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

struct GrokProcessTransport {
    stdin: Option<std::process::ChildStdin>,
    rx: std::sync::mpsc::Receiver<String>,
}

impl GrokTransport for GrokProcessTransport {
    fn write_line(&mut self, line: &str) -> Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "grok stdin closed".to_string())?;
        stdin
            .write_all(line.as_bytes())
            .map_err(|e| e.to_string())?;
        stdin.write_all(b"\n").map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())
    }

    fn close_stdin(&mut self) -> Result<(), String> {
        self.stdin.take();
        Ok(())
    }

    fn read_matching(&mut self, id: i64, timeout: Duration) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("grok billing timed out".into());
            }
            match self.rx.recv_timeout(left) {
                Ok(line) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let msg: Value = match serde_json::from_str(&line) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if msg.get("id").and_then(|v| v.as_i64()) == Some(id) {
                        return Ok(msg);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    return Err("grok billing timed out".into());
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("grok billing closed".into());
                }
            }
        }
    }
}

/// Kills the grok process group on drop, including when a read times out.
struct GrokGuard {
    child: Option<std::process::Child>,
}

impl Drop for GrokGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            kill_grok_group(child.id());
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn kill_grok_group(pid: u32) {
    #[cfg(unix)]
    unsafe {
        let pgid = pid as libc::pid_t;
        if libc::killpg(pgid, libc::SIGKILL) != 0 {
            libc::kill(pgid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

fn map_grok_spawn_err(err: std::io::Error) -> GrokFail {
    if err.kind() == std::io::ErrorKind::NotFound {
        GrokFail::NotSignedIn
    } else {
        GrokFail::Transport(err.to_string())
    }
}

fn run_grok_process(program: &str) -> Result<GrokParsed, GrokFail> {
    run_grok_process_in(program, None)
}

fn run_grok_process_in(program: &str, slot: Option<&Path>) -> Result<GrokParsed, GrokFail> {
    let mut cmd = grok_command(program);
    if let Some(slot) = slot {
        cmd.env("GROK_HOME", slot);
    }
    let mut child = cmd.spawn().map_err(map_grok_spawn_err)?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| GrokFail::Transport("grok stdin missing".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| GrokFail::Transport("grok stdout missing".into()))?;
    let guard = GrokGuard { child: Some(child) };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let mut transport = GrokProcessTransport {
        stdin: Some(stdin),
        rx,
    };
    let parsed = grok_exchange(&mut transport);
    drop(transport);
    drop(guard);
    parsed
}

fn write_cache(doc: &SubscriptionFile) -> Result<(), String> {
    let path = cache_path();
    let bytes = serde_json::to_vec_pretty(doc).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    if text.contains("accessToken")
        || text.contains("refreshToken")
        || text.contains("Authorization")
    {
        return Err("refusing to write a subscription cache that contains credentials".into());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("usage dir: {e}"))?;
    }
    k2_core::fs_atomic::atomic_write(&path, &bytes).map_err(|e| format!("usage write: {e}"))?;
    restrict_cache(&path)
}

fn restrict_cache(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = path.parent() {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("usage dir mode: {e}"))?;
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("usage file mode: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

impl UsageIo for LiveIo {
    fn claude_login(&self) -> ClaudeLogin {
        if probe_denied() {
            return ClaudeLogin::Missing;
        }
        live_claude_login()
    }

    fn claude_get(&self, access_token: &str) -> Result<String, String> {
        if probe_denied() {
            return Err("subscription probe denied".into());
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        let response = client
            .get(USAGE_URL)
            .bearer_auth(access_token)
            .header("anthropic-beta", ANTHROPIC_BETA)
            .header("Accept", "application/json")
            .send()
            .map_err(|_| "usage get failed".to_string())?;
        let status = response.status();
        if !status.is_success() {
            // A 429 says when to come back; the back-off honours it.
            let retry = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<i64>().ok());
            return Err(match retry {
                Some(secs) => format!("usage endpoint status {} retry-after {secs}", status.as_u16()),
                None => format!("usage endpoint status {}", status.as_u16()),
            });
        }
        response
            .text()
            .map_err(|_| "usage body unreadable".to_string())
    }

    fn codex(&self) -> CodexOutcome {
        if probe_denied() {
            return CodexOutcome::Failed;
        }
        let program = match guarded_program("codex") {
            Ok(p) => p,
            Err(msg) => {
                k2_core::log_debug!("[usage] codex probe refused: {msg}");
                return CodexOutcome::Failed;
            }
        };
        match run_codex_process(&program) {
            Ok(parsed) => CodexOutcome::Ready {
                plan: parsed.plan,
                windows: parsed.windows,
            },
            Err(CodexFail::NotSignedIn) => CodexOutcome::NotSignedIn,
            Err(CodexFail::Transport(err)) => {
                k2_core::log_debug!("[usage] codex probe failed: {err}");
                CodexOutcome::Failed
            }
        }
    }

    fn grok(&self) -> GrokOutcome {
        if probe_denied() {
            return GrokOutcome::Failed;
        }
        let program = match guarded_program("grok") {
            Ok(p) => p,
            Err(msg) => {
                k2_core::log_debug!("[usage] grok probe refused: {msg}");
                return GrokOutcome::Failed;
            }
        };
        match run_grok_process(&program) {
            Ok(parsed) => GrokOutcome::Ready {
                plan: parsed.plan,
                windows: parsed.windows,
            },
            Err(GrokFail::NotSignedIn) => GrokOutcome::NotSignedIn,
            Err(GrokFail::Transport(err)) => {
                k2_core::log_debug!("[usage] grok probe failed: {err}");
                GrokOutcome::Failed
            }
        }
    }
}

fn live_claude_login() -> ClaudeLogin {
    #[cfg(target_os = "macos")]
    let keychain = read_keychain();
    #[cfg(not(target_os = "macos"))]
    let keychain: Option<String> = None;
    let files: Vec<Option<String>> = credential_file_paths()
        .iter()
        .map(|path| fs::read_to_string(path).ok())
        .collect();
    let file_refs: Vec<Option<&str>> = files.iter().map(|raw| raw.as_deref()).collect();
    login_from_sources(keychain.as_deref(), &file_refs)
}

#[cfg(target_os = "macos")]
fn read_keychain() -> Option<String> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// ── Wallet slots (Settings → LLMs, `k2 llm accounts usage`) ──────────

/// The token-refresh exchange for an idle Codex slot: the CLI's own
/// `account/read {refreshToken:true}` (no model tokens spent).
fn codex_refresh_exchange(io: &mut dyn CodexTransport) -> Result<(), CodexFail> {
    let timeout = Duration::from_secs(20);
    io.write_line(&codex_line(
        1,
        "initialize",
        json!({"clientInfo": {"name": "k2", "title": "K2", "version": "0"}}),
    )?)
    .map_err(CodexFail::Transport)?;
    let _init = io.read_matching(1, timeout).map_err(CodexFail::Transport)?;
    let initialized = serde_json::to_string(&json!({"method": "initialized", "params": {}}))
        .map_err(|e| CodexFail::Transport(e.to_string()))?;
    io.write_line(&initialized).map_err(CodexFail::Transport)?;
    io.write_line(&codex_line(2, "account/read", json!({"refreshToken": true}))?)
        .map_err(CodexFail::Transport)?;
    let msg = match io.read_matching(2, timeout) {
        Ok(m) => m,
        Err(err) if err.contains("closed") => return Err(CodexFail::NotSignedIn),
        Err(err) => return Err(CodexFail::Transport(err)),
    };
    if let Some(e) = msg.get("error") {
        let text = e.get("message").and_then(|m| m.as_str()).unwrap_or("account/read error");
        return Err(CodexFail::Transport(text.chars().take(160).collect()));
    }
    match msg.pointer("/result/account") {
        Some(a) if !a.is_null() => Ok(()),
        _ => Err(CodexFail::NotSignedIn),
    }
}

/// Have Codex refresh an idle wallet slot's token itself (`CODEX_HOME`
/// = the slot). The caller holds the wallet's Codex lock.
pub fn codex_refresh_slot(slot: &Path) -> Result<(), String> {
    if airgap_on() {
        return Err(k2_core::airgap::TEACHING.to_string());
    }
    let program = guarded_program("codex")?;
    match run_codex_process_in(&program, Some(slot), codex_refresh_exchange) {
        Ok(()) => Ok(()),
        Err(CodexFail::NotSignedIn) => Err("401 not signed in (refresh token rejected)".into()),
        Err(CodexFail::Transport(e)) => Err(e),
    }
}

/// Usage for an idle wallet slot. Same row shape as the shared cache
/// (`harness` = the tool), never written to the shared cache. The caller
/// holds the wallet's lock for that tool. `claude_token` is the slot's
/// access token (Claude only), read by the wallet store. `prev` is the
/// slot's own last reading: a failed check keeps it, with its old time
/// and the error.
pub fn probe_wallet_slot(
    tool: &str,
    slot: &Path,
    claude_token: Option<(String, Option<i64>, String)>,
    prev: Option<&HarnessUsage>,
) -> HarnessUsage {
    let now = Utc::now();
    if airgap_on() {
        return HarnessUsage {
            error: None,
            account_id: None,
            attempted_at: None,
            harness: tool.to_string(),
            plan: String::new(),
            windows: Vec::new(),
            checked_at: now.to_rfc3339(),
            status: STATUS_AIRGAP.into(),
            ..Default::default()
        };
    }
    match tool {
        HARNESS_CLAUDE => {
            let login = match claude_token {
                Some((access_token, expires_at, plan)) => ClaudeLogin::Token { access_token, expires_at, plan },
                None => ClaudeLogin::Missing,
            };
            probe_claude(login, |t| LiveIo.claude_get(t), prev, SystemTime::now(), now)
        }
        HARNESS_CODEX => {
            let outcome = match guarded_program("codex") {
                Err(_) => CodexOutcome::Failed,
                Ok(program) => match run_codex_process_in(&program, Some(slot), codex_exchange) {
                    Ok(parsed) => CodexOutcome::Ready { plan: parsed.plan, windows: parsed.windows },
                    Err(CodexFail::NotSignedIn) => CodexOutcome::NotSignedIn,
                    Err(CodexFail::Transport(_)) => CodexOutcome::Failed,
                },
            };
            probe_codex(outcome, prev, now)
        }
        HARNESS_GROK => {
            let outcome = match guarded_program("grok") {
                Err(_) => GrokOutcome::Failed,
                Ok(program) => match run_grok_process_in(&program, Some(slot)) {
                    Ok(parsed) => GrokOutcome::Ready { plan: parsed.plan, windows: parsed.windows },
                    Err(GrokFail::NotSignedIn) => GrokOutcome::NotSignedIn,
                    Err(GrokFail::Transport(_)) => GrokOutcome::Failed,
                },
            };
            probe_grok(outcome, prev, now)
        }
        other => HarnessUsage {
            error: None,
            account_id: None,
            attempted_at: None,
            harness: other.to_string(),
            plan: String::new(),
            windows: Vec::new(),
            checked_at: now.to_rfc3339(),
            status: STATUS_NOT_SUPPORTED.into(),
            ..Default::default()
        },
    }
}

#[cfg(test)]
fn percent_left(used: f64) -> i64 {
    ((1.0 - used) * 100.0).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards the module's probe state (`probe_hits`, the test IO and
    /// air-gap slots), NOT the env. A test that also touches the env takes
    /// the ONE shared env lock FIRST, then this (one lock order everywhere).
    fn probe_lock() -> k2_core::test_env::SerialGuard {
        // Env lock first (`k2_core::test_env::serial`), then the probe lock.
        static LOCK: Mutex<()> = Mutex::new(());
        k2_core::test_env::serial(&LOCK)
    }

    struct ScriptIo {
        login: ClaudeLogin,
        body: Mutex<Result<String, String>>,
        gets: AtomicUsize,
        codex: Mutex<CodexOutcome>,
        grok: Mutex<GrokOutcome>,
    }

    impl UsageIo for ScriptIo {
        fn claude_login(&self) -> ClaudeLogin {
            self.login.clone()
        }
        fn claude_get(&self, _access_token: &str) -> Result<String, String> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            self.body.lock().unwrap_or_else(|p| p.into_inner()).clone()
        }
        fn codex(&self) -> CodexOutcome {
            self.codex.lock().unwrap_or_else(|p| p.into_inner()).clone()
        }
        fn grok(&self) -> GrokOutcome {
            self.grok.lock().unwrap_or_else(|p| p.into_inner()).clone()
        }
    }

    struct PanicIo;

    impl UsageIo for PanicIo {
        fn claude_login(&self) -> ClaudeLogin {
            panic!("claude login probe ran");
        }
        fn claude_get(&self, _access_token: &str) -> Result<String, String> {
            panic!("claude usage GET ran");
        }
        fn codex(&self) -> CodexOutcome {
            panic!("codex probe ran");
        }
        fn grok(&self) -> GrokOutcome {
            panic!("grok probe ran");
        }
    }

    struct MemCodex {
        lines: Vec<String>,
        account: Value,
        limits: Value,
    }

    impl CodexTransport for MemCodex {
        fn write_line(&mut self, line: &str) -> Result<(), String> {
            self.lines.push(line.to_string());
            Ok(())
        }
        fn read_matching(&mut self, id: i64, _timeout: Duration) -> Result<Value, String> {
            let last = self.lines.last().map(|s| s.as_str()).unwrap_or("");
            let sent: Value = serde_json::from_str(last).map_err(|e| e.to_string())?;
            let method = sent.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let result = match method {
                "initialize" => json!({}),
                "account/read" => self.account.clone(),
                "account/rateLimits/read" => self.limits.clone(),
                other => return Err(format!("unexpected method {other}")),
            };
            Ok(json!({"id": id, "result": result}))
        }
    }

    fn with_isolated(f: impl FnOnce()) {
        // Env lock (TempHome) first, then the module probe lock.
        let _home = crate::test_support::TempHome::new();
        let _probe = probe_lock();
        // Own wallet DB: the probe records readings on wallet tokens, and
        // another test's rows in the shared DB must not change this one.
        let _db = k2_core::db::scoped_for_test();
        probe_hits().store(0, Ordering::SeqCst);
        // Back-off and single-flight state are process-wide: start clean.
        backoffs().lock().unwrap_or_else(|p| p.into_inner()).clear();
        *flight_last().lock().unwrap_or_else(|p| p.into_inner()) = None;
        *test_io_slot().lock().unwrap_or_else(|p| p.into_inner()) = None;
        // Air-gap off unless a test says otherwise; immune to a parallel
        // test that sets K2_AIRGAP.
        set_airgap(false);
        let _deny = k2_core::test_env::EnvVar::set("K2_SUBSCRIPTION_PROBE", "deny");
        f();
        *test_io_slot().lock().unwrap_or_else(|p| p.into_inner()) = None;
        *test_airgap_slot().lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    fn install(io: Arc<dyn UsageIo>) {
        *test_io_slot().lock().unwrap_or_else(|p| p.into_inner()) = Some(io);
    }

    fn set_airgap(on: bool) {
        *test_airgap_slot().lock().unwrap_or_else(|p| p.into_inner()) = Some(on);
    }

    fn signed_in_script() -> Arc<ScriptIo> {
        Arc::new(ScriptIo {
            login: ClaudeLogin::Token {
                access_token: "LEAK-ACCESS".into(),
                expires_at: Some(future_expiry_secs()),
                plan: "Max 20x".into(),
            },
            body: Mutex::new(Ok(usage_body())),
            gets: AtomicUsize::new(0),
            codex: Mutex::new(CodexOutcome::NotSignedIn),
            grok: Mutex::new(GrokOutcome::NotSignedIn),
        })
    }

    fn assert_airgap_body(body: &str) {
        let v: Value = serde_json::from_str(body).expect("airgap body is JSON");
        assert_eq!(v["airgap"], json!(true), "{body}");
        assert_eq!(v["reason"], json!(STATUS_AIRGAP), "{body}");
        let rows = v["harnesses"].as_array().expect("harnesses array");
        let names: Vec<&str> = rows
            .iter()
            .map(|r| r["harness"].as_str().expect("harness name"))
            .collect();
        assert_eq!(names, vec![HARNESS_CLAUDE, HARNESS_CODEX, HARNESS_GROK]);
        for row in rows {
            assert_eq!(row["status"], json!(STATUS_AIRGAP), "{body}");
            assert_eq!(row["windows"], json!([]), "air-gap must not show a stale number: {body}");
        }
    }

    #[test]
    fn airgap_background_tick_makes_no_http_call_and_spawns_no_cli() {
        with_isolated(|| {
            set_airgap(true);
            // PanicIo panics on any login read, GET, codex or grok spawn.
            install(Arc::new(PanicIo));
            assert!(!background_tick(), "air-gap tick must not probe");
            assert_eq!(probe_hits().load(Ordering::SeqCst), 0);
            assert!(!cache_path().exists(), "air-gap tick must not write the cache");
        });
    }

    #[test]
    fn airgap_probe_and_write_refuses_before_any_io() {
        with_isolated(|| {
            set_airgap(true);
            let err = probe_and_write(&PanicIo).expect_err("air-gap must refuse");
            assert_eq!(err, k2_core::airgap::TEACHING);
            assert_eq!(probe_hits().load(Ordering::SeqCst), 0);
        });
    }

    #[test]
    fn airgap_refresh_route_reports_airgap_and_does_not_probe() {
        with_isolated(|| {
            set_airgap(true);
            install(Arc::new(PanicIo));
            let response = handle_refresh();
            assert_eq!(response.status, "200 OK");
            assert_airgap_body(&response.body);
            assert_eq!(probe_hits().load(Ordering::SeqCst), 0);
            assert!(!cache_path().exists());
        });
    }

    #[test]
    fn airgap_get_route_reports_airgap_not_the_stale_cache() {
        with_isolated(|| {
            let now = Utc::now();
            let doc = SubscriptionFile {
                harnesses: vec![harness(
                    HARNESS_CLAUDE,
                    vec![UsageWindow {
                        label: "Weekly".into(),
                        used: 0.42,
                        resets_at: future_reset(now),
                        ..Default::default()
                    }],
                    "",
                    &now.to_rfc3339(),
                )],
            };
            write_cache(&doc).expect("seed");
            set_airgap(true);
            let response = crate::cli::dispatch(
                "/cli/usage/subscriptions",
                &std::collections::HashMap::new(),
            );
            assert_eq!(response.status, "200 OK");
            assert_airgap_body(&response.body);
            assert!(!response.body.contains("0.42"), "{}", response.body);
        });
    }

    #[test]
    fn without_airgap_tick_and_refresh_still_probe() {
        with_isolated(|| {
            set_airgap(false);
            let io = signed_in_script();
            install(io.clone());
            assert!(background_tick(), "tick must probe without air-gap");
            assert_eq!(probe_hits().load(Ordering::SeqCst), 1);
            assert_eq!(io.gets.load(Ordering::SeqCst), 1);
            let written = read_cache_file().expect("cache written");
            assert_eq!(written.harnesses.len(), 3);

            let stale = (Utc::now() - chrono::Duration::seconds(16)).to_rfc3339();
            let mut doc = written;
            for h in &mut doc.harnesses {
                h.checked_at = stale.clone();
            }
            write_cache(&doc).expect("age cache");
            let response = handle_refresh();
            assert_eq!(response.status, "200 OK");
            assert_eq!(probe_hits().load(Ordering::SeqCst), 2);
            assert!(!response.body.contains("\"airgap\""), "{}", response.body);
            assert!(response.body.contains("Max 20x"), "{}", response.body);

            let get = handle_get();
            assert_eq!(get.status, "200 OK");
            assert!(!get.body.contains("\"airgap\""), "{}", get.body);
        });
    }

    #[test]
    fn airgap_gate_reads_the_shared_helper() {
        let head = include_str!("subscription_usage.rs")
            .split("mod tests")
            .next()
            .expect("module head");
        assert!(head.contains("k2_core::airgap::enabled()"));
        assert!(!head.contains("var(\"K2_AIRGAP\")"), "use the airgap helper, not the env var");
    }

    fn token_login(expires_at: Option<i64>) -> ClaudeLogin {
        ClaudeLogin::Token {
            access_token: "LEAK-ACCESS".into(),
            expires_at,
            plan: "Max 20x".into(),
        }
    }

    /// A seconds-shaped expiry still in the future on the machine running the test.
    fn future_expiry_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs() as i64
            + 3600
    }

    fn usage_body() -> String {
        r#"{"five_hour":{"utilization":10,"resets_at":"2026-09-26T20:00:00Z"},"seven_day":{"utilization":31,"resets_at":"2026-10-03T00:00:00Z"},"seven_day_oauth_apps":null,"limits":[{"kind":"weekly_scoped","percent":12,"resets_at":"2026-10-03T00:00:00Z","scope":{"model":{"display_name":"Opus","id":"claude-opus"}}}]}"#.into()
    }

    fn future_reset(now: DateTime<Utc>) -> String {
        (now + chrono::Duration::days(2)).to_rfc3339()
    }

    fn harness(name: &str, windows: Vec<UsageWindow>, status: &str, checked: &str) -> HarnessUsage {
        HarnessUsage {
            error: None,
            account_id: None,
            attempted_at: None,
            harness: name.into(),
            plan: "Max 20x".into(),
            windows,
            checked_at: checked.into(),
            status: status.into(),
            ..Default::default()
        }
    }

    fn wall(secs: i64) -> (SystemTime, DateTime<Utc>) {
        let now = DateTime::<Utc>::from_timestamp(secs, 0).expect("timestamp");
        let wall = UNIX_EPOCH + Duration::from_secs(secs as u64);
        (wall, now)
    }

    #[test]
    fn claude_utilization_31_stores_used_0_31_and_69_percent_left() {
        let windows = parse_claude_usage(&usage_body()).expect("usage json");
        let weekly = windows
            .iter()
            .find(|w| w.label == "Weekly")
            .expect("weekly row");
        assert_eq!((weekly.used * 100.0).round() as i64, 31);
        assert_eq!(percent_left(weekly.used), 69);
        let session = windows
            .iter()
            .find(|w| w.label == "Session")
            .expect("session row");
        assert_eq!((session.used * 100.0).round() as i64, 10);
    }

    #[test]
    fn model_scoped_limits_row_is_kept() {
        let windows = parse_claude_usage(&usage_body()).expect("usage json");
        let scoped = windows
            .iter()
            .find(|w| w.label == "Opus Weekly")
            .expect("model-scoped row");
        assert_eq!((scoped.used * 100.0).round() as i64, 12);
        assert!(windows.len() >= 3);
    }

    #[test]
    fn plan_max_20x_label() {
        assert_eq!(plan_label("max_20x", "pro"), "Max 20x");
        assert_eq!(plan_label("", "plus"), "Plus");
    }

    #[test]
    fn written_file_has_no_access_token() {
        with_isolated(|| {
            let io = ScriptIo {
                login: token_login(Some(future_expiry_secs())),
                body: Mutex::new(Ok(usage_body())),
                gets: AtomicUsize::new(0),
                codex: Mutex::new(CodexOutcome::NotSignedIn),
                grok: Mutex::new(GrokOutcome::NotSignedIn),
            };
            let doc = probe_and_write(&io).expect("write");
            assert_eq!(io.gets.load(Ordering::SeqCst), 1);
            let text = fs::read_to_string(cache_path()).expect("cache file");
            assert!(!text.contains("accessToken"), "{text}");
            assert!(!text.contains("refreshToken"), "{text}");
            assert!(!text.contains("Authorization"), "{text}");
            assert!(!text.contains("LEAK-ACCESS"), "{text}");
            assert!(!text.contains("LEAK-REFRESH"), "{text}");
            let weekly = doc
                .harnesses
                .iter()
                .find(|h| h.harness == HARNESS_CLAUDE)
                .expect("claude")
                .windows
                .iter()
                .find(|w| w.label == "Weekly")
                .expect("weekly");
            assert_eq!(percent_left(weekly.used), 69);
        });
    }

    fn wall_secs(wall: SystemTime) -> i64 {
        wall.duration_since(UNIX_EPOCH).expect("wall").as_secs() as i64
    }

    #[test]
    fn expired_millisecond_token_does_not_get_and_keeps_open_window() {
        let (wall, now) = wall(1_700_000_000);
        let expires_ms = wall_secs(wall) * 1000 - 5_000;
        assert!(expires_ms > EXPIRY_UNIT_THRESHOLD);
        let reset = future_reset(now);
        let prev = harness(
            HARNESS_CLAUDE,
            vec![UsageWindow {
                label: "Weekly".into(),
                used: 0.31,
                resets_at: reset.clone(),
                ..Default::default()
            }],
            "",
            now.to_rfc3339().as_str(),
        );
        let mut gets = 0;
        let got = probe_claude(
            token_login(Some(expires_ms)),
            |_| {
                gets += 1;
                Err("must not GET".into())
            },
            Some(&prev),
            wall,
            now,
        );
        assert_eq!(gets, 0);
        assert_eq!(got.status, STATUS_SIGN_IN_EXPIRED);
        assert_eq!(got.windows.len(), 1);
        assert_eq!(got.windows[0].label, "Weekly");
        assert_eq!(percent_left(got.windows[0].used), 69);
        assert_eq!(got.windows[0].resets_at, reset);
    }

    #[test]
    fn seconds_expires_at_in_the_future_may_get() {
        let (wall, now) = wall(1_700_000_000);
        let expires = wall_secs(wall) + 3600;
        assert!(expires < EXPIRY_UNIT_THRESHOLD);
        let mut gets = 0;
        let got = probe_claude(
            token_login(Some(expires)),
            |_| {
                gets += 1;
                Ok(usage_body())
            },
            None,
            wall,
            now,
        );
        assert_eq!(gets, 1);
        assert_eq!(got.status, "");
        let weekly = got
            .windows
            .iter()
            .find(|w| w.label == "Weekly")
            .expect("weekly");
        assert_eq!(percent_left(weekly.used), 69);
    }

    #[test]
    fn millisecond_expires_at_in_the_future_may_get() {
        let (wall, now) = wall(1_700_000_000);
        let expires_ms = (wall_secs(wall) + 3600) * 1000;
        assert!(expires_ms > EXPIRY_UNIT_THRESHOLD);
        let mut gets = 0;
        let got = probe_claude(
            token_login(Some(expires_ms)),
            |_| {
                gets += 1;
                Ok(usage_body())
            },
            None,
            wall,
            now,
        );
        assert_eq!(gets, 1);
        assert_eq!(got.status, "");
    }

    #[test]
    fn five_minute_buffer_still_gets_and_does_not_refresh() {
        let (wall, now) = wall(1_700_000_000);
        let expires = wall_secs(wall) + 60;
        let mut gets = 0;
        let got = probe_claude(
            token_login(Some(expires)),
            |_| {
                gets += 1;
                Ok(usage_body())
            },
            None,
            wall,
            now,
        );
        assert_eq!(gets, 1);
        assert_eq!(got.status, "");
        let head = include_str!("subscription_usage.rs")
            .split("mod tests")
            .next()
            .expect("module head");
        assert!(!head.contains("handle_refresh_now"));
    }

    #[test]
    fn empty_access_token_is_not_signed_in_and_does_not_get() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"","refreshToken":"LEAK-REFRESH","expiresAt":1999999999,"rateLimitTier":"max_20x"}}"#;
        let login = parse_claude_login(raw);
        assert!(matches!(login, ClaudeLogin::Missing));
        let (wall, now) = wall(1_700_000_000);
        let reset = future_reset(now);
        let prev = harness(
            HARNESS_CLAUDE,
            vec![UsageWindow {
                label: "Weekly".into(),
                used: 0.31,
                resets_at: reset,
                ..Default::default()
            }],
            "",
            &now.to_rfc3339(),
        );
        let mut gets = 0;
        let got = probe_claude(
            login,
            |_| {
                gets += 1;
                Err("must not GET".into())
            },
            Some(&prev),
            wall,
            now,
        );
        assert_eq!(gets, 0);
        assert_eq!(got.status, STATUS_NOT_SIGNED_IN);
        assert_eq!(got.windows.len(), 1);
        assert_eq!(got.windows[0].label, "Weekly");
    }

    #[test]
    fn access_token_without_refresh_token_is_signed_in() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"present","expiresAt":1999999999,"rateLimitTier":"max_20x"}}"#;
        match parse_claude_login(raw) {
            ClaudeLogin::Token { plan, .. } => assert_eq!(plan, "Max 20x"),
            ClaudeLogin::Missing => panic!("access token without refresh must still count"),
        }
    }

    #[test]
    fn keychain_fixture_beats_missing_file() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"keychain-token","refreshToken":"r","expiresAt":1700000000000,"rateLimitTier":"max_20x","subscriptionType":"max"}}"#;
        let login = login_from_sources(Some(raw), &[None]);
        match login {
            ClaudeLogin::Token {
                plan, expires_at, ..
            } => {
                assert_eq!(plan, "Max 20x");
                assert_eq!(expires_at, Some(1_700_000_000_000));
            }
            ClaudeLogin::Missing => panic!("keychain fixture must count as signed in"),
        }
    }

    #[test]
    fn keychain_wins_over_file() {
        let keychain = r#"{"claudeAiOauth":{"accessToken":"from-keychain","expiresAt":1700003600,"rateLimitTier":"max_20x"}}"#;
        let file = r#"{"claudeAiOauth":{"accessToken":"from-file","expiresAt":1700003600,"subscriptionType":"pro"}}"#;
        let login = login_from_sources(Some(keychain), &[Some(file)]);
        match login {
            ClaudeLogin::Token {
                access_token, plan, ..
            } => {
                assert_eq!(access_token, "from-keychain");
                assert_eq!(plan, "Max 20x");
            }
            ClaudeLogin::Missing => panic!("keychain login missing"),
        }
    }

    #[test]
    fn claude_config_dir_is_not_the_only_file_path() {
        // Env lock first (the guard holds it; restores on drop), then the
        // module probe lock.
        let config_dir =
            k2_core::test_env::EnvVar::set("CLAUDE_CONFIG_DIR", "/tmp/k2-claude-config-dir");
        let _probe = probe_lock();
        let paths = credential_file_paths();
        drop(config_dir);
        assert_eq!(
            paths.first().map(|p| p.as_path()),
            Some(Path::new("/tmp/k2-claude-config-dir/.credentials.json"))
        );
        assert!(
            paths.len() >= 2,
            "home credentials file must stay in the list: {paths:?}"
        );
        assert!(paths.iter().any(|p| {
            p.ends_with(".claude/.credentials.json") || p.ends_with(".claude\\.credentials.json")
        }));
        let head = include_str!("subscription_usage.rs")
            .split("mod tests")
            .next()
            .expect("module head");
        let live = head.find("fn live_claude_login").expect("live login");
        let body = &head[live..];
        let keychain = body.find("read_keychain").expect("keychain reader");
        let files = body.find("credential_file_paths()").expect("file reader");
        assert!(keychain < files, "keychain must run before the file");
    }

    #[test]
    fn codex_used_percent_18_and_10080_minutes_is_weekly() {
        let account = json!({
            "id": 2,
            "result": {
                "account": {"type": "chatgpt", "planType": "plus"},
                "requiresOpenaiAuth": true
            }
        });
        let limits = json!({
            "id": 3,
            "result": {
                "rateLimits": {
                    "planType": "plus",
                    "primary": {"usedPercent": 18, "windowDurationMins": 10080, "resetsAt": 1893456000},
                    "secondary": {"usedPercent": 4, "windowDurationMins": 300, "resetsAt": 1893456000}
                }
            }
        });
        let parsed = parse_codex_results(&account, &limits).expect("codex parse");
        assert_eq!(parsed.plan, "plus");
        let weekly = parsed
            .windows
            .iter()
            .find(|w| w.label == "Weekly")
            .expect("weekly");
        assert_eq!((weekly.used * 100.0).round() as i64, 18);
        assert!((weekly.used - 0.18).abs() < 1e-9);
        let session = parsed.windows.iter().find(|w| w.label == "5h").expect("5h");
        assert_eq!((session.used * 100.0).round() as i64, 4);
    }

    /// A9: under a temp HOME the usage probes never resolve to a real
    /// CLI; only a test shim (when a shim dir is set) may run.
    #[test]
    fn probes_resolve_through_the_spawn_guard() {
        crate::test_support::with_temp_home(|| {
            for name in ["codex", "grok"] {
                match guarded_program(name) {
                    Err(msg) => assert!(msg.contains("refusing") || msg.contains("not found"), "{msg}"),
                    Ok(p) => {
                        let shims = std::env::var_os(k2_core::terminal::agent_spawn_guard::SHIM_DIR_ENV)
                            .unwrap_or_else(|| panic!("{name} resolved to {p} with no shim dir under a temp HOME"));
                        assert!(
                            std::env::split_paths(&shims).any(|d| std::path::Path::new(&p).starts_with(&d)),
                            "{name} resolved outside the shim dirs: {p}"
                        );
                    }
                }
            }
        });
    }

    #[test]
    fn codex_argv_is_app_server_without_resume_or_session() {
        let cmd = codex_command("codex");
        assert_eq!(cmd.get_program().to_string_lossy(), "codex");
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["app-server".to_string()]);
        assert!(!args.iter().any(|a| a == "resume" || a.contains("session")));
    }

    #[test]
    fn codex_exchange_does_not_resume_or_refresh_login() {
        let mut io = MemCodex {
            lines: Vec::new(),
            account: json!({
                "account": {"type": "chatgpt", "planType": "plus"},
                "requiresOpenaiAuth": true
            }),
            limits: json!({
                "rateLimits": {
                    "planType": "plus",
                    "primary": {"usedPercent": 18, "windowDurationMins": 10080, "resetsAt": 1893456000},
                    "secondary": null
                }
            }),
        };
        let parsed = codex_exchange(&mut io).expect("exchange");
        let joined = io.lines.join("\n");
        assert!(joined.contains("\"method\":\"initialize\""));
        assert!(joined.contains("\"method\":\"initialized\""));
        assert!(joined.contains("\"method\":\"account/read\""));
        assert!(joined.contains("\"method\":\"account/rateLimits/read\""));
        assert!(joined.contains("\"refreshToken\":false"));
        assert!(!joined.contains("resume"));
        assert!(!joined.contains("sessionId"));
        assert!(!joined.contains("auth.json"));
        assert_eq!(parsed.windows[0].label, "Weekly");
        assert_eq!((parsed.windows[0].used * 100.0).round() as i64, 18);
    }

    #[test]
    fn codex_missing_login_is_not_signed_in_not_zero_percent() {
        let account = json!({
            "id": 2,
            "result": {"account": null, "requiresOpenaiAuth": true}
        });
        let limits = json!({"id": 3, "result": {"rateLimits": null}});
        let err = parse_codex_results(&account, &limits).expect_err("no login");
        assert!(matches!(err, CodexFail::NotSignedIn));
        let now = Utc::now();
        let got = probe_codex(CodexOutcome::NotSignedIn, None, now);
        assert_eq!(got.status, STATUS_NOT_SIGNED_IN);
        assert!(got.windows.is_empty());
        assert!(got
            .windows
            .iter()
            .all(|w| w.used != 0.0 || w.label.is_empty()));
        let missing = map_spawn_err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no such file",
        ));
        assert!(matches!(missing, CodexFail::NotSignedIn));
        let harness = probe_codex(CodexOutcome::NotSignedIn, None, now);
        assert_eq!(harness.status, STATUS_NOT_SIGNED_IN);
        assert!(harness.windows.is_empty());
    }

    #[test]
    fn unknown_harness_is_absent_and_probe_does_not_add_one() {
        with_isolated(|| {
            let now = Utc::now().to_rfc3339();
            let doc = SubscriptionFile {
                harnesses: vec![harness(
                    "gemini",
                    vec![UsageWindow {
                        label: "Weekly".into(),
                        used: 0.99,
                        resets_at: now.clone(),
                        ..Default::default()
                    }],
                    "",
                    &now,
                )],
            };
            write_cache(&doc).expect("seed");
            let response = handle_get();
            assert_eq!(response.status, "200 OK");
            assert!(!response.body.contains("gemini"), "{}", response.body);
            let on_disk = fs::read_to_string(cache_path()).expect("disk");
            assert!(on_disk.contains("gemini"), "GET must not rewrite the file");
            let io = ScriptIo {
                login: token_login(Some(future_expiry_secs())),
                body: Mutex::new(Ok(usage_body())),
                gets: AtomicUsize::new(0),
                codex: Mutex::new(CodexOutcome::NotSignedIn),
                grok: Mutex::new(GrokOutcome::NotSignedIn),
            };
            let written = probe_and_write(&io).expect("probe");
            assert_eq!(written.harnesses.len(), 3);
            assert!(written.harnesses.iter().all(|h| h.harness != "gemini"));
            let names: Vec<&str> = written
                .harnesses
                .iter()
                .map(|h| h.harness.as_str())
                .collect();
            assert_eq!(names, vec![HARNESS_CLAUDE, HARNESS_CODEX, HARNESS_GROK]);
        });
    }

    #[test]
    fn get_is_not_404_and_writes_nothing() {
        with_isolated(|| {
            let response = crate::cli::dispatch(
                "/cli/usage/subscriptions",
                &std::collections::HashMap::new(),
            );
            assert_eq!(response.status, "200 OK");
            assert!(
                !response.body.contains("route not found"),
                "{}",
                response.body
            );
            assert!(response.body.contains("harnesses"), "{}", response.body);
            assert!(!cache_path().exists(), "GET must not create the cache");
        });
    }

    #[test]
    fn get_on_refresh_path_is_405_and_post_refresh_exists() {
        let response = crate::cli::dispatch(
            "/cli/usage/subscriptions/refresh",
            &std::collections::HashMap::new(),
        );
        assert_eq!(response.status, "405 Method Not Allowed");
        assert!(response.body.contains("POST required"), "{}", response.body);

        let dispatcher = include_str!("routes/dispatcher.rs");
        // POST allowlist = the route policy table: only the refresh verb.
        assert!(crate::routes::route_policy::post_allowed(
            "/cli/usage/subscriptions/refresh"
        ));
        assert!(!crate::routes::route_policy::post_allowed("/cli/usage/subscriptions"));
        let arm = dispatcher
            .find("\"/cli/usage/subscriptions/refresh\" =>")
            .expect("refresh arm");
        let window = &dispatcher[arm..arm + 1600];
        assert!(window.contains("require_post"));
        assert!(window.contains("spawn_blocking"));
        assert!(window.contains("handle_refresh"));
        assert!(!window.contains("require_owner"));
    }

    #[test]
    fn refresh_fresher_than_15_seconds_does_not_probe() {
        with_isolated(|| {
            install(Arc::new(PanicIo));
            let now = Utc::now().to_rfc3339();
            let doc = SubscriptionFile {
                harnesses: vec![
                    harness(HARNESS_CLAUDE, Vec::new(), STATUS_NOT_SIGNED_IN, &now),
                    harness(HARNESS_CODEX, Vec::new(), STATUS_NOT_SIGNED_IN, &now),
                    harness(HARNESS_GROK, Vec::new(), STATUS_NOT_SIGNED_IN, &now),
                ],
            };
            write_cache(&doc).expect("seed");
            probe_hits().store(0, Ordering::SeqCst);
            let response = handle_refresh();
            assert_eq!(response.status, "200 OK");
            assert_eq!(probe_hits().load(Ordering::SeqCst), 0);
            assert!(response.body.contains(STATUS_NOT_SIGNED_IN));
        });
    }

    #[test]
    fn refresh_older_than_15_seconds_probes() {
        with_isolated(|| {
            let stale = (Utc::now() - chrono::Duration::seconds(16)).to_rfc3339();
            let doc = SubscriptionFile {
                harnesses: vec![
                    harness(HARNESS_CLAUDE, Vec::new(), STATUS_NOT_SIGNED_IN, &stale),
                    harness(HARNESS_CODEX, Vec::new(), STATUS_NOT_SIGNED_IN, &stale),
                    harness(HARNESS_GROK, Vec::new(), STATUS_NOT_SIGNED_IN, &stale),
                ],
            };
            write_cache(&doc).expect("seed");
            install(Arc::new(ScriptIo {
                login: ClaudeLogin::Token {
                    access_token: "LEAK-ACCESS".into(),
                    expires_at: Some(future_expiry_secs()),
                    plan: "Max 20x".into(),
                },
                body: Mutex::new(Ok(usage_body())),
                gets: AtomicUsize::new(0),
                codex: Mutex::new(CodexOutcome::NotSignedIn),
                grok: Mutex::new(GrokOutcome::NotSignedIn),
            }));
            probe_hits().store(0, Ordering::SeqCst);
            let response = handle_refresh();
            assert_eq!(response.status, "200 OK");
            assert_eq!(probe_hits().load(Ordering::SeqCst), 1);
            assert!(response.body.contains("Max 20x"), "{}", response.body);
            assert!(!response.body.contains("LEAK-ACCESS"), "{}", response.body);
        });
    }

    #[test]
    fn failed_probe_keeps_future_window_and_does_not_truncate_first() {
        with_isolated(|| {
            let reset = (Utc::now() + chrono::Duration::days(3)).to_rfc3339();
            let checked = (Utc::now() - chrono::Duration::seconds(30)).to_rfc3339();
            let doc = SubscriptionFile {
                harnesses: vec![harness(
                    HARNESS_CLAUDE,
                    vec![UsageWindow {
                        label: "keep-me-weekly".into(),
                        used: 0.31,
                        resets_at: reset,
                        ..Default::default()
                    }],
                    "",
                    &checked,
                )],
            };
            write_cache(&doc).expect("seed");
            let before = fs::read(cache_path()).expect("seed bytes");
            assert!(!before.is_empty());
            let io = ScriptIo {
                login: token_login(Some(future_expiry_secs())),
                body: Mutex::new(Err("usage endpoint down".into())),
                gets: AtomicUsize::new(0),
                codex: Mutex::new(CodexOutcome::Failed),
                grok: Mutex::new(GrokOutcome::Failed),
            };
            let written = probe_and_write(&io).expect("preserved write");
            assert_eq!(io.gets.load(Ordering::SeqCst), 1);
            let text = fs::read_to_string(cache_path()).expect("cache");
            assert!(text.contains("keep-me-weekly"), "{text}");
            assert!(!text.is_empty());
            let claude = written
                .harnesses
                .iter()
                .find(|h| h.harness == HARNESS_CLAUDE)
                .expect("claude");
            assert_eq!(claude.windows.len(), 1);
            assert_eq!(claude.windows[0].label, "keep-me-weekly");
            assert_eq!(percent_left(claude.windows[0].used), 69);
            // Field bug 2026-10-14: a failed check used to re-stamp the
            // old numbers as checked now.
            assert_eq!(claude.checked_at, checked, "a failed check keeps the old time");
            assert_eq!(claude.error.as_deref(), Some("usage endpoint down"));
            assert!(claude.attempted_at.is_some());
            let codex = written.harnesses.iter().find(|h| h.harness == HARNESS_CODEX).expect("codex");
            assert_eq!(codex.checked_at, "", "never read: no time");
            assert!(codex.error.is_some());
        });
    }

    /// Insert two Claude wallet tokens; `active` is the server default.
    fn seed_wallet(active: &str, idle: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for (i, id) in [active, idle].iter().enumerate() {
            conn.execute(
                "INSERT INTO llm_accounts (id, tool, label, kind, position, created_at, state) \
                 VALUES (?1, 'claude', ?1, 'subscription', ?2, 1, 'signed_in')",
                rusqlite::params![id, i as i64],
            )
            .expect("seed token");
        }
        conn.execute(
            "INSERT INTO llm_active (tool, account_id, switched_at) VALUES ('claude', ?1, 1)",
            rusqlite::params![active],
        )
        .expect("seed active");
    }

    fn wallet_usage(id: &str) -> (Option<String>, Option<i64>) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let e = k2_core::llm_accounts::get(&conn, id).expect("db").expect("token");
        (e.usage_json, e.usage_checked_at)
    }

    #[test]
    fn readings_stay_with_the_token_they_belong_to() {
        with_isolated(|| {
            seed_wallet("acc_live", "acc_old");
            // The cache still holds the PREVIOUS default's numbers (the
            // switch happened since): they belong to acc_old.
            let now = Utc::now();
            let mut old = harness(
                HARNESS_CLAUDE,
                vec![UsageWindow { label: "old-account-weekly".into(), used: 0.95, resets_at: future_reset(now), ..Default::default() }],
                "",
                &(now - chrono::Duration::minutes(40)).to_rfc3339(),
            );
            old.account_id = Some("acc_old".into());
            write_cache(&SubscriptionFile { harnesses: vec![old] }).expect("seed");
            // The check for the new default fails (429): the old account's
            // numbers must not appear as the new default's.
            let failing = ScriptIo {
                login: token_login(Some(future_expiry_secs())),
                body: Mutex::new(Err("usage endpoint status 429".into())),
                gets: AtomicUsize::new(0),
                codex: Mutex::new(CodexOutcome::NotSignedIn),
                grok: Mutex::new(GrokOutcome::NotSignedIn),
            };
            let doc = probe_and_write(&failing).expect("write");
            let claude = doc.harnesses.iter().find(|h| h.harness == HARNESS_CLAUDE).expect("claude");
            assert!(claude.windows.is_empty(), "another token's windows were shown: {claude:?}");
            assert_eq!(claude.checked_at, "", "never read for this token");
            assert_eq!(claude.error.as_deref(), Some("usage endpoint status 429"));
            assert_eq!(claude.account_id.as_deref(), Some("acc_live"));
            let (json_live, checked_live) = wallet_usage("acc_live");
            assert!(json_live.expect("failure recorded").contains("429"));
            assert_eq!(checked_live, None, "a failed check never stamps usage_checked_at");
            // A good reading lands on the live token only.
            let ok_io = signed_in_script();
            let doc = probe_and_write(ok_io.as_ref()).expect("write");
            let claude = doc.harnesses.iter().find(|h| h.harness == HARNESS_CLAUDE).expect("claude");
            assert!(claude.error.is_none());
            assert_eq!(claude.account_id.as_deref(), Some("acc_live"));
            let (json_live, checked_live) = wallet_usage("acc_live");
            let row: HarnessUsage = serde_json::from_str(&json_live.expect("reading")).expect("row");
            assert_eq!(row.windows, claude.windows);
            assert!(checked_live.is_some_and(|t| (Utc::now().timestamp() - t).abs() < 60));
            let (json_old, checked_old) = wallet_usage("acc_old");
            assert_eq!((json_old, checked_old), (None, None), "the idle token's row is untouched");
            // A later failure keeps THIS token's last good reading and time.
            *ok_io.body.lock().unwrap() = Err("usage endpoint status 500".into());
            let doc = probe_and_write(ok_io.as_ref()).expect("write");
            let claude = doc.harnesses.iter().find(|h| h.harness == HARNESS_CLAUDE).expect("claude");
            let still_open: Vec<UsageWindow> = row.windows.iter().filter(|w| window_still_open(w, Utc::now())).cloned().collect();
            assert_eq!(claude.windows, still_open, "this token's own windows (closed ones dropped)");
            assert_eq!(claude.error.as_deref(), Some("usage endpoint status 500"));
            assert_eq!(claude.checked_at, row.checked_at, "not re-stamped");
            assert_eq!(wallet_usage("acc_live").1, checked_live);
            assert!(!fs::read_to_string(cache_path()).unwrap().contains("LEAK-ACCESS"));
        });
    }

    #[test]
    fn probe_now_checks_one_tool_and_keeps_the_others() {
        with_isolated(|| {
            let now = Utc::now();
            let stamp = (now - chrono::Duration::minutes(5)).to_rfc3339();
            let mut codex = harness(HARNESS_CODEX, Vec::new(), STATUS_NO_WINDOW, &stamp);
            codex.plan = "pro".into();
            write_cache(&SubscriptionFile { harnesses: vec![codex.clone()] }).expect("seed");
            let io = signed_in_script();
            install(io.clone());
            let doc = probe_now(Some(HARNESS_CLAUDE)).expect("probe");
            assert_eq!(io.gets.load(Ordering::SeqCst), 1);
            let names: Vec<&str> = doc.harnesses.iter().map(|h| h.harness.as_str()).collect();
            assert_eq!(names, vec![HARNESS_CLAUDE, HARNESS_CODEX], "grok had no row and was not probed");
            assert_eq!(doc.harnesses[1], codex, "codex kept as it was");
        });
    }

    #[cfg(unix)]
    #[test]
    fn unix_cache_file_is_0600_and_dir_is_0700() {
        use std::os::unix::fs::PermissionsExt;
        with_isolated(|| {
            let now = Utc::now().to_rfc3339();
            let doc = SubscriptionFile {
                harnesses: vec![harness(
                    HARNESS_CLAUDE,
                    Vec::new(),
                    STATUS_NOT_SIGNED_IN,
                    &now,
                )],
            };
            write_cache(&doc).expect("write");
            let file_mode = fs::metadata(cache_path())
                .expect("file meta")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(file_mode, 0o600);
            let dir_mode = fs::metadata(cache_path().parent().expect("usage dir"))
                .expect("dir meta")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700);
        });
    }

    #[test]
    fn probe_interval_is_fifteen_minutes_and_not_the_heartbeat() {
        assert_eq!(PROBE_INTERVAL.as_secs(), 15 * 60);
        // The boot sequence lives in daemon_main.rs (main.rs is a shim).
        let main = include_str!("daemon_main.rs");
        let usage = main
            .find("subscription_usage::spawn()")
            .expect("usage loop");
        let heart = main
            .find("heartbeat_monitor::spawn()")
            .expect("heartbeat loop");
        assert_ne!(usage, heart);
        let line = main[..usage].lines().last().expect("spawn line");
        assert!(!line.contains("heartbeat"));
        let head = include_str!("subscription_usage.rs")
            .split("mod tests")
            .next()
            .expect("module head");
        let code: String = head
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//") && !trimmed.starts_with("//!")
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!code.contains("spawn_session"));
        assert!(!code.contains("v2_session_map"));
        assert!(!code.contains("auth.json"));
        assert!(!code.contains("compute_auth_state"));
        assert!(!code.contains("webview"));
        assert!(!code.contains("heartbeat_monitor"));
    }

    #[test]
    fn utilization_under_one_is_a_small_percent_not_a_fraction() {
        // B5: right after a reset every window is under 1 %; 0.4 is 0.4 %,
        // never 40 %.
        let body = r#"{"five_hour":{"utilization":0,"resets_at":"2026-09-26T20:00:00Z"},"seven_day":{"utilization":0.6,"resets_at":"2026-10-03T00:00:00Z"}}"#;
        let windows = parse_claude_usage(body).expect("usage json");
        let weekly = windows.iter().find(|w| w.label == "Weekly").expect("weekly");
        assert!((weekly.used - 0.006).abs() < 1e-12, "{}", weekly.used);
        assert_ne!((weekly.used * 100.0).round() as i64, 60);
        assert_eq!(percent_left(weekly.used), 99);
        let session = windows.iter().find(|w| w.label == "Session").expect("session");
        assert_eq!(session.used, 0.0);
    }

    #[test]
    fn weekly_is_seven_day_and_the_apps_allowance_is_its_own_row() {
        // B6: the oauth-apps bucket never stands in for the all-models week.
        let body = r#"{"five_hour":{"utilization":10,"resets_at":"2026-09-26T20:00:00Z"},"seven_day":{"utilization":31,"resets_at":"2026-10-03T00:00:00Z"},"seven_day_oauth_apps":{"utilization":0,"resets_at":"2026-10-03T00:00:00Z"}}"#;
        let windows = parse_claude_usage(body).expect("usage json");
        let weekly = windows.iter().find(|w| w.label == "Weekly").expect("weekly");
        assert_eq!((weekly.id.as_str(), (weekly.used * 100.0).round() as i64), ("seven_day", 31));
        assert!(windows.iter().all(|w| w.id != "seven_day_oauth_apps"), "a 0 % apps bucket is not shown");
        let used = r#"{"seven_day":{"utilization":31,"resets_at":"2026-10-03T00:00:00Z"},"seven_day_oauth_apps":{"utilization":12,"resets_at":"2026-10-03T00:00:00Z"}}"#;
        let windows = parse_claude_usage(used).expect("usage json");
        let apps = windows.iter().find(|w| w.id == "seven_day_oauth_apps").expect("apps row");
        assert_eq!(apps.label, "Weekly (apps)");
        assert_eq!((windows.iter().find(|w| w.label == "Weekly").expect("w").used * 100.0).round() as i64, 31);
    }

    #[test]
    fn windows_carry_ids_kinds_and_lengths_for_the_pace_mark() {
        let windows = parse_claude_usage(&usage_body()).expect("usage json");
        let get = |id: &str| windows.iter().find(|w| w.id == id).cloned().expect(id);
        assert_eq!((get("five_hour").kind.as_str(), get("five_hour").window_secs), ("session", Some(5 * 3600)));
        assert_eq!((get("seven_day").kind.as_str(), get("seven_day").window_secs), ("weekly", Some(7 * 86_400)));
        let scoped = get("model:opus:weekly");
        assert_eq!((scoped.label.as_str(), scoped.window_secs), ("Opus Weekly", Some(7 * 86_400)));
        let codex = codex_windows(
            &json!({"primary": {"usedPercent": 18.0, "windowDurationMins": 300, "resetsAt": 1_800_000_000}, "secondary": {"usedPercent": 3.0, "windowDurationMins": 10080}}),
            None,
        )
        .1;
        assert_eq!((codex[0].id.as_str(), codex[0].kind.as_str(), codex[0].window_secs), ("primary", "session", Some(18_000)));
        assert_eq!((codex[1].label.as_str(), codex[1].kind.as_str()), ("Weekly", "weekly"));
        // The session-log shape (snake case, resets_in_seconds).
        let log = codex_windows(&json!({"primary": {"used_percent": 40.0, "window_minutes": 300, "resets_in_seconds": 600}}), Some(1_800_000_000_000)).1;
        assert!((log[0].used - 0.4).abs() < 1e-12);
        assert_eq!(log[0].resets_at, DateTime::<Utc>::from_timestamp(1_800_000_600, 0).expect("t").to_rfc3339());
        let grok = parse_grok_billing(&grok_value(GROK_FIXTURE)).expect("grok");
        assert_eq!((grok.windows[0].id.as_str(), grok.windows[0].window_secs), ("credit", Some(7 * 86_400)), "period start → end");
    }

    #[test]
    fn served_rows_have_state_percent_pace_start_and_reset_flags() {
        let now = DateTime::parse_from_rfc3339("2026-10-16T12:00:00Z").expect("t").with_timezone(&Utc);
        let row = HarnessUsage {
            harness: "claude".into(),
            checked_at: "2026-10-16T11:58:00Z".into(),
            windows: vec![
                window("five_hour", "Session", "session", 0.315, "2026-10-16T14:00:00Z".into(), Some(18_000)),
                window("seven_day", "Weekly", "weekly", 0.97, "2026-10-16T11:00:00Z".into(), Some(604_800)),
            ],
            ..Default::default()
        };
        let r = decorate_row(row, now, "decorate-test-key");
        assert_eq!(r.state, "ok");
        assert_eq!(r.source, "probe");
        assert!((r.windows[0].used_percent - 31.5).abs() < 1e-9);
        assert_eq!(r.windows[0].window_start.as_deref(), Some("2026-10-16T09:00:00+00:00"));
        assert!(!r.windows[0].reset_passed);
        assert!(r.windows[1].reset_passed, "past resetsAt (B18)");
        let state = |status: &str, err: Option<&str>, wins: usize| {
            derive_state(&HarnessUsage {
                status: status.into(),
                error: err.map(str::to_string),
                windows: vec![UsageWindow::default(); wins],
                ..Default::default()
            })
        };
        assert_eq!(state(STATUS_SIGN_IN_EXPIRED, Some("x"), 1), "expired", "kept visible with a typed state (B22)");
        assert_eq!(state(STATUS_NOT_SIGNED_IN, None, 0), "not_signed_in");
        assert_eq!(state("", Some("usage endpoint status 500"), 1), "probe_failed");
        assert_eq!(state(STATUS_NOT_SUPPORTED, None, 0), "unsupported");
        assert_eq!(state(STATUS_AIRGAP, None, 0), "airgap");
        assert_eq!(state("", None, 0), "no_window");
    }

    #[test]
    fn a_get_past_a_reset_marks_it_and_kicks_one_recheck() {
        with_isolated(|| {
            let past = (Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
            write_cache(&SubscriptionFile {
                harnesses: vec![harness(HARNESS_CLAUDE, vec![window("seven_day", "Weekly", "weekly", 0.97, past, Some(604_800))], "", &Utc::now().to_rfc3339())],
            })
            .expect("cache");
            let before = kicks().load(Ordering::SeqCst);
            let resp = handle_get();
            let v: Value = serde_json::from_str(&resp.body).expect("json");
            assert_eq!(v["harnesses"][0]["windows"][0]["resetPassed"], json!(true));
            assert_eq!(v["harnesses"][0]["state"], json!("ok"));
            assert_eq!(kicks().load(Ordering::SeqCst), before + 1, "the GET started a re-check");
            assert_eq!(probe_hits().load(Ordering::SeqCst), 0, "never inline");
        });
    }

    #[test]
    fn concurrent_refreshes_share_one_probe() {
        struct SlowIo(AtomicUsize);
        impl UsageIo for SlowIo {
            fn claude_login(&self) -> ClaudeLogin {
                ClaudeLogin::Token { access_token: "T".into(), expires_at: Some(future_expiry_secs()), plan: String::new() }
            }
            fn claude_get(&self, _t: &str) -> Result<String, String> {
                self.0.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(300));
                Ok(usage_body())
            }
            fn codex(&self) -> CodexOutcome {
                CodexOutcome::NotSignedIn
            }
            fn grok(&self) -> GrokOutcome {
                GrokOutcome::NotSignedIn
            }
        }
        with_isolated(|| {
            let io = Arc::new(SlowIo(AtomicUsize::new(0)));
            install(io.clone());
            let threads: Vec<_> = (0..4).map(|_| std::thread::spawn(|| refresh_shared(None).map(|d| d.harnesses.len()))).collect();
            for t in threads {
                assert_eq!(t.join().expect("join").expect("refresh"), 3);
            }
            assert_eq!(io.0.load(Ordering::SeqCst), 1, "one GET for four callers (B19)");
            assert_eq!(probe_hits().load(Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn failures_back_off_and_a_429_retry_after_is_honoured() {
        let _probe = probe_lock();
        let now = Utc::now();
        let failed = |err: &str| HarnessUsage { error: Some(err.into()), ..Default::default() };
        let key = format!("backoff-test-{}", std::process::id());
        note_outcome(&key, &failed("usage endpoint status 500"), now);
        let (until, limited) = backing_off(&key, now).expect("backing off");
        assert_eq!((until - now).num_minutes(), 15);
        assert!(!limited);
        note_outcome(&key, &failed("usage endpoint status 500"), now);
        assert_eq!((backing_off(&key, now).expect("2nd").0 - now).num_minutes(), 30, "doubles");
        for _ in 0..10 {
            note_outcome(&key, &failed("x"), now);
        }
        assert_eq!((backing_off(&key, now).expect("cap").0 - now).num_minutes(), 120, "capped at 2 h");
        note_outcome(&key, &HarnessUsage::default(), now);
        assert!(backing_off(&key, now).is_none(), "a success clears it");
        note_outcome(&key, &failed("usage endpoint status 429 retry-after 7200"), now);
        let (until, limited) = backing_off(&key, now).expect("429");
        assert!(limited);
        assert_eq!((until - now).num_seconds(), 7200, "Retry-After wins when longer");
        assert_eq!(retry_after_secs("status 429 retry-after 90"), Some(90));
        assert_eq!(retry_after_secs("status 500"), None);
    }

    #[test]
    fn background_skips_live_fresh_and_backed_off_logins_manual_waits_only_for_429() {
        let _probe = probe_lock();
        let now = Utc::now();
        let live = HarnessUsage { source: "live_statusline".into(), checked_at: (now - chrono::Duration::minutes(5)).to_rfc3339(), ..Default::default() };
        assert!(skip_reason(Some(&live), None, "skip-test-a", Mode::Background, now).is_some(), "a 5-minute-old live reading is not polled");
        assert!(skip_reason(Some(&live), None, "skip-test-a", Mode::Manual, now).is_none(), "a person's refresh still checks");
        let other = HarnessUsage { account_id: Some("acc_other".into()), ..live.clone() };
        assert!(skip_reason(Some(&other), Some("acc_me"), "skip-test-a", Mode::Background, now).is_none(), "never keep another token's row");
        let probed = HarnessUsage { source: "probe".into(), checked_at: now.to_rfc3339(), ..Default::default() };
        let key = format!("skip-test-{}", std::process::id());
        note_outcome(&key, &HarnessUsage { error: Some("status 500".into()), ..Default::default() }, now);
        assert!(skip_reason(Some(&probed), None, &key, Mode::Background, now).is_some());
        assert!(skip_reason(Some(&probed), None, &key, Mode::Manual, now).is_none());
        note_outcome(&key, &HarnessUsage { error: Some("status 429 retry-after 60".into()), ..Default::default() }, now);
        assert!(skip_reason(Some(&probed), None, &key, Mode::Manual, now).is_some(), "a 429 with Retry-After holds a person's refresh too");
        note_outcome(&key, &HarnessUsage::default(), now);
        note_outcome(&key, &HarnessUsage { error: Some("usage endpoint status 429".into()), ..Default::default() }, now);
        assert!(skip_reason(Some(&probed), None, &key, Mode::Manual, now).is_none(), "a bare 429 only slows the loop");
        note_outcome(&key, &HarnessUsage::default(), now);
        let past = (now - chrono::Duration::minutes(1)).to_rfc3339();
        let live_reset = HarnessUsage { windows: vec![window("five_hour", "Session", "session", 0.9, past, Some(18_000))], ..live.clone() };
        assert!(skip_reason(Some(&live_reset), None, "skip-test-b", Mode::Background, now).is_none(), "a reset window is re-checked even when live");
    }

    #[test]
    fn a_probe_announces_itself_and_keeps_a_newer_live_row() {
        with_isolated(|| {
            let mut rx = crate::session_events::subscribe();
            let future = (Utc::now() + chrono::Duration::days(2)).to_rfc3339();
            let live_at = (Utc::now() + chrono::Duration::seconds(30)).to_rfc3339();
            install(signed_in_script());
            // A live row lands "after" the probe's start.
            write_cache(&SubscriptionFile {
                harnesses: vec![HarnessUsage {
                    harness: HARNESS_CODEX.into(),
                    windows: vec![window("primary", "5h", "session", 0.5, future, Some(18_000))],
                    checked_at: live_at.clone(),
                    source: "live_rollout".into(),
                    ..Default::default()
                }],
            })
            .expect("cache");
            let doc = probe_now(None).expect("probe");
            let codex = doc.harnesses.iter().find(|h| h.harness == HARNESS_CODEX).expect("codex");
            assert_eq!((codex.source.as_str(), codex.checked_at.as_str()), ("live_rollout", live_at.as_str()), "the newer live row wins");
            let mut saw = false;
            while let Ok(ev) = rx.try_recv() {
                if matches!(ev, crate::session_events::SessionEvent::SubscriptionUsageChanged { ref source, .. } if source == "probe") {
                    saw = true;
                }
            }
            assert!(saw, "subscription_usage_changed after the write (B20)");
        });
    }

    #[test]
    fn a_missing_or_old_cache_is_stale_at_boot() {
        with_isolated(|| {
            assert!(cache_stale(Utc::now()), "missing");
            let fresh = Utc::now().to_rfc3339();
            write_cache(&SubscriptionFile {
                harnesses: PROBED_HARNESSES.iter().map(|n| harness(n, Vec::new(), "", &fresh)).collect(),
            })
            .expect("cache");
            assert!(!cache_stale(Utc::now()));
            assert!(cache_stale(Utc::now() + chrono::Duration::minutes(16)), "older than 15 minutes (B18)");
        });
    }

    const GROK_FIXTURE: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"config":{"creditUsagePercent":22.0,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-09-19T00:00:00Z","end":"2026-09-26T00:00:00Z"}},"subscription_tier":"SuperGrok"}}"#;

    fn grok_value(raw: &str) -> Value {
        serde_json::from_str(raw).expect("grok json")
    }

    struct MemGrok {
        lines: Vec<String>,
        closed_after: Option<usize>,
        billing: Value,
    }

    impl GrokTransport for MemGrok {
        fn write_line(&mut self, line: &str) -> Result<(), String> {
            self.lines.push(line.to_string());
            Ok(())
        }
        fn close_stdin(&mut self) -> Result<(), String> {
            self.closed_after = Some(self.lines.len());
            Ok(())
        }
        fn read_matching(&mut self, id: i64, _timeout: Duration) -> Result<Value, String> {
            let last = self.lines.last().map(|s| s.as_str()).unwrap_or("");
            let sent: Value = serde_json::from_str(last).map_err(|e| e.to_string())?;
            let method = sent.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let result = match method {
                "initialize" => json!({}),
                "_x.ai/billing" => self.billing.clone(),
                other => return Err(format!("unexpected method {other}")),
            };
            Ok(json!({"id": id, "result": result}))
        }
    }

    #[test]
    fn grok_fixture_stores_used_0_22_supergrok_and_period_end() {
        let parsed = parse_grok_billing(&grok_value(GROK_FIXTURE)).expect("fixture");
        assert_eq!(parsed.plan, "SuperGrok");
        assert_eq!(parsed.windows.len(), 1);
        assert_eq!(parsed.windows[0].label, "Weekly");
        assert!((parsed.windows[0].used - 0.22).abs() < 1e-9);
        assert_eq!(parsed.windows[0].resets_at, "2026-09-26T00:00:00Z");
        assert!(parsed.windows.iter().all(|w| w.label != "Grok Build"));
    }

    #[test]
    fn grok_subscription_tier_camel_case_is_plan() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"result":{"config":{"creditUsagePercent":22.0,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2026-09-26T00:00:00Z"}},"subscriptionTier":"SuperGrok"}}"#;
        let parsed = parse_grok_billing(&grok_value(raw)).expect("camel tier");
        assert_eq!(parsed.plan, "SuperGrok");
        assert!((parsed.windows[0].used - 0.22).abs() < 1e-9);
    }

    #[test]
    fn grok_credit_under_one_is_a_small_percent() {
        // B5: creditUsagePercent is a percent: 0.22 is 0.22 %, never 22 %.
        let raw = r#"{"jsonrpc":"2.0","id":1,"result":{"config":{"creditUsagePercent":0.22,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2026-09-26T00:00:00Z"}},"subscription_tier":"SuperGrok"}}"#;
        let parsed = parse_grok_billing(&grok_value(raw)).expect("small percent");
        assert!((parsed.windows[0].used - 0.0022).abs() < 1e-12, "{}", parsed.windows[0].used);
    }

    #[test]
    fn grok_build_window_when_product_usage_present() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"result":{"config":{"creditUsagePercent":22.0,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2026-09-26T00:00:00Z"},"productUsage":{"GrokBuild":{"usagePercent":4.0}}},"subscription_tier":"SuperGrok"}}"#;
        let parsed = parse_grok_billing(&grok_value(raw)).expect("build");
        assert_eq!(parsed.windows.len(), 2);
        assert_eq!(parsed.windows[0].label, "Weekly");
        let build = parsed
            .windows
            .iter()
            .find(|w| w.label == "Grok Build")
            .expect("grok build");
        assert!((build.used - 0.04).abs() < 1e-9);
        assert_eq!(build.resets_at, "2026-09-26T00:00:00Z");
    }

    /// Live `_x.ai/billing` result on grok 1.0.46 just after the weekly
    /// reset. Protobuf JSON leaves out the zero `creditUsagePercent`.
    const GROK_ZERO_AFTER_RESET: &str = r#"{"jsonrpc":"2.0","id":2,"result":{"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-10-02T22:47:50.098695+00:00","end":"2026-10-09T22:47:50.098695+00:00"},"onDemandCap":{"val":0},"onDemandUsed":{"val":0},"prepaidBalance":{"val":0},"isUnifiedBillingUser":true,"billingPeriodStart":"2026-10-02T22:47:50.098695+00:00","billingPeriodEnd":"2026-10-09T22:47:50.098695+00:00"},"subscription_tier":"SuperGrok Heavy"}}"#;

    #[test]
    fn grok_omitted_zero_with_period_is_weekly_window_at_0() {
        let parsed = parse_grok_billing(&grok_value(GROK_ZERO_AFTER_RESET)).expect("zero");
        assert_eq!(parsed.plan, "SuperGrok Heavy");
        assert_eq!(parsed.windows.len(), 1);
        assert_eq!(parsed.windows[0].label, "Weekly");
        assert_eq!(parsed.windows[0].used, 0.0);
        assert_eq!(
            parsed.windows[0].resets_at,
            "2026-10-09T22:47:50.098695+00:00"
        );
        let got = probe_grok(
            GrokOutcome::Ready {
                plan: parsed.plan,
                windows: parsed.windows,
            },
            None,
            Utc::now(),
        );
        assert_eq!(got.status, "");
        assert_eq!(got.windows.len(), 1);
    }

    #[test]
    fn grok_empty_grok_build_is_build_window_at_0() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"result":{"config":{"creditUsagePercent":22.0,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2026-09-26T00:00:00Z"},"productUsage":{"GrokBuild":{}}},"subscription_tier":"SuperGrok"}}"#;
        let parsed = parse_grok_billing(&grok_value(raw)).expect("empty build");
        assert_eq!(parsed.windows.len(), 2);
        assert_eq!(parsed.windows[0].label, "Weekly");
        assert!((parsed.windows[0].used - 0.22).abs() < 1e-9);
        let build = parsed
            .windows
            .iter()
            .find(|w| w.label == "Grok Build")
            .expect("grok build");
        assert_eq!(build.used, 0.0);
        assert_eq!(build.resets_at, "2026-09-26T00:00:00Z");
    }

    #[test]
    fn grok_missing_percent_and_period_is_no_usage_window() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"result":{"config":{"productUsage":{"GrokBuild":{"usagePercent":4.0}}},"subscription_tier":"SuperGrok"}}"#;
        let parsed = parse_grok_billing(&grok_value(raw)).expect("no percent");
        assert!(parsed.windows.is_empty());
        assert_eq!(parsed.plan, "SuperGrok");
        let got = probe_grok(
            GrokOutcome::Ready {
                plan: parsed.plan,
                windows: parsed.windows,
            },
            None,
            Utc::now(),
        );
        assert_eq!(got.status, STATUS_NO_WINDOW);
        assert!(got.windows.is_empty());
    }

    #[test]
    fn grok_auth_error_is_not_signed_in() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"error":{"code":401,"message":"unauthorized"}}"#;
        let err = parse_grok_billing(&grok_value(raw)).expect_err("auth");
        assert!(matches!(err, GrokFail::NotSignedIn));
        let missing = map_grok_spawn_err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no such file",
        ));
        assert!(matches!(missing, GrokFail::NotSignedIn));
        let now = Utc::now();
        let got = probe_grok(GrokOutcome::NotSignedIn, None, now);
        assert_eq!(got.status, STATUS_NOT_SIGNED_IN);
        assert!(got.windows.is_empty());
        assert_eq!(got.harness, HARNESS_GROK);
    }

    #[test]
    fn grok_not_signed_in_leaves_claude_and_codex_rows() {
        with_isolated(|| {
            let io = ScriptIo {
                login: token_login(Some(future_expiry_secs())),
                body: Mutex::new(Ok(usage_body())),
                gets: AtomicUsize::new(0),
                codex: Mutex::new(CodexOutcome::Ready {
                    plan: "plus".into(),
                    windows: vec![UsageWindow {
                        label: "Weekly".into(),
                        used: 0.18,
                        resets_at: "2026-10-03T00:00:00Z".into(),
                        ..Default::default()
                    }],
                }),
                grok: Mutex::new(GrokOutcome::NotSignedIn),
            };
            let doc = probe_and_write(&io).expect("probe");
            let claude = doc
                .harnesses
                .iter()
                .find(|h| h.harness == HARNESS_CLAUDE)
                .expect("claude");
            assert_eq!(claude.status, "");
            assert!(claude.windows.iter().any(|w| w.label == "Weekly"));
            let codex = doc
                .harnesses
                .iter()
                .find(|h| h.harness == HARNESS_CODEX)
                .expect("codex");
            assert_eq!(codex.plan, "plus");
            assert_eq!(codex.status, "");
            assert!((codex.windows[0].used - 0.18).abs() < 1e-9);
            let grok = doc
                .harnesses
                .iter()
                .find(|h| h.harness == HARNESS_GROK)
                .expect("grok");
            assert_eq!(grok.status, STATUS_NOT_SIGNED_IN);
            assert!(grok.windows.is_empty());
        });
    }

    #[test]
    fn grok_argv_is_agent_stdio_without_reauth() {
        let cmd = grok_command("grok");
        assert_eq!(cmd.get_program().to_string_lossy(), "grok");
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["agent".to_string(), "stdio".to_string()]);
        assert!(!args.iter().any(|a| a == "--no-leader"));
        assert!(!args
            .iter()
            .any(|a| { a == "--reauth" || a.contains("auth.json") || a == "usage" }));
        let head = include_str!("subscription_usage.rs")
            .split("mod tests")
            .next()
            .expect("module head");
        assert!(head.contains("process_group(0)"));
        assert!(head.contains("killpg"));
        assert!(!head.contains("cli-chat-proxy.grok.com"));
    }

    #[test]
    fn grok_exchange_initializes_then_billing_and_closes_stdin() {
        let fixture = grok_value(GROK_FIXTURE);
        let billing = fixture.get("result").cloned().expect("result");
        let mut io = MemGrok {
            lines: Vec::new(),
            closed_after: None,
            billing,
        };
        let parsed = grok_exchange(&mut io).expect("exchange");
        assert_eq!(parsed.plan, "SuperGrok");
        assert_eq!(parsed.windows.len(), 1);
        assert!((parsed.windows[0].used - 0.22).abs() < 1e-9);
        assert_eq!(parsed.windows[0].resets_at, "2026-09-26T00:00:00Z");
        let joined = io.lines.join("\n");
        assert!(joined.contains("\"method\":\"initialize\""));
        assert!(joined.contains("\"protocolVersion\":1"));
        assert!(joined.contains("\"method\":\"notifications/initialized\""));
        assert!(joined.contains("\"method\":\"_x.ai/billing\""));
        let init_at = joined.find("\"method\":\"initialize\"").expect("init");
        let ready_at = joined
            .find("\"method\":\"notifications/initialized\"")
            .expect("initialized");
        let bill_at = joined
            .find("\"method\":\"_x.ai/billing\"")
            .expect("billing");
        assert!(init_at < ready_at && ready_at < bill_at);
        assert_eq!(io.closed_after, Some(3));
        assert!(!joined.contains("\"method\":\"initialized\""));
        assert!(!joined.contains("--reauth"));
        assert!(!joined.contains("auth.json"));
        assert!(!joined.contains("cli-chat-proxy"));
    }
}
