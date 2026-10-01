//! Subscription windows for the signed-in Claude, Codex, and Grok logins.
//!
//! The daemon probes. The cache file is `k2_home()/usage/subscriptions.json`
//! and stores windows only — never an access token, refresh token, or
//! `auth.json` body. The renderer reads `GET /cli/usage/subscriptions`
//! and asks for a probe with `POST /cli/usage/subscriptions/refresh`.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
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

const HARNESS_CLAUDE: &str = "claude";
const HARNESS_CODEX: &str = "codex";
const HARNESS_GROK: &str = "grok";
const PROBED_HARNESSES: [&str; 3] = [HARNESS_CLAUDE, HARNESS_CODEX, HARNESS_GROK];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub label: String,
    pub used: f64,
    pub resets_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HarnessUsage {
    pub harness: String,
    pub plan: String,
    pub windows: Vec<UsageWindow>,
    pub checked_at: String,
    pub status: String,
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

fn probe_denied() -> bool {
    std::env::var_os("K2_SUBSCRIPTION_PROBE").as_deref() == Some(std::ffi::OsStr::new("deny"))
}

pub fn cache_path() -> PathBuf {
    k2_core::paths::k2_home()
        .join("usage")
        .join("subscriptions.json")
}

/// `GET /cli/usage/subscriptions`. Reads the cache. Writes nothing.
pub fn handle_get() -> CliResponse {
    let path = cache_path();
    if !path.is_file() {
        return ok_doc(&SubscriptionFile {
            harnesses: Vec::new(),
        });
    }
    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<SubscriptionFile>(&text) {
            Ok(doc) => ok_doc(&filter_known(doc)),
            Err(_) => CliResponse::internal_error("subscriptions cache is not valid JSON"),
        },
        Err(e) => CliResponse::internal_error(format!("subscriptions cache: {e}")),
    }
}

/// `POST /cli/usage/subscriptions/refresh`.
/// A cache newer than 15 seconds is returned as-is. Otherwise the probe
/// runs here — the caller must be off the connection task (`spawn_blocking`
/// or the background loop).
pub fn handle_refresh() -> CliResponse {
    let now = Utc::now();
    if let Some(doc) = fresh_cache(now) {
        return ok_doc(&doc);
    }
    let io = io_for_probe();
    match probe_and_write(io.as_ref()) {
        Ok(doc) => ok_doc(&doc),
        Err(e) => CliResponse::internal_error(e),
    }
}

/// Headless 15-minute probe. No webview. Not the heartbeat monitor.
/// The first fire waits one interval; the menu POST covers a cold cache.
pub fn spawn() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(PROBE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        loop {
            interval.tick().await;
            let _ = tokio::task::spawn_blocking(|| {
                if let Err(e) = probe_and_write(io_for_probe().as_ref()) {
                    k2_core::log_debug!("[usage] subscription probe failed: {e}");
                }
            })
            .await;
        }
    })
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
        let Ok(checked) = DateTime::parse_from_rfc3339(&h.checked_at) else {
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
fn probe_and_write(io: &dyn UsageIo) -> Result<SubscriptionFile, String> {
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
    let claude = probe_claude(
        io.claude_login(),
        |token| io.claude_get(token),
        previous
            .harnesses
            .iter()
            .find(|h| h.harness == HARNESS_CLAUDE),
        now_wall,
        now,
    );
    let codex = probe_codex(
        io.codex(),
        previous
            .harnesses
            .iter()
            .find(|h| h.harness == HARNESS_CODEX),
        now,
    );
    let grok = probe_grok(
        io.grok(),
        previous
            .harnesses
            .iter()
            .find(|h| h.harness == HARNESS_GROK),
        now,
    );
    let doc = SubscriptionFile {
        harnesses: vec![claude, codex, grok],
    };
    write_cache(&doc)?;
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
        return preserved(HARNESS_CLAUDE, prev, STATUS_NOT_SIGNED_IN, "", now);
    };
    if access_token.is_empty() {
        return preserved(HARNESS_CLAUDE, prev, STATUS_NOT_SIGNED_IN, &plan, now);
    }
    if !access_token_usable(expires_at, now_wall) {
        return preserved(HARNESS_CLAUDE, prev, STATUS_SIGN_IN_EXPIRED, &plan, now);
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
                    harness: HARNESS_CLAUDE.into(),
                    plan,
                    windows,
                    checked_at: now.to_rfc3339(),
                    status,
                }
            }
            Err(_) => preserved(HARNESS_CLAUDE, prev, prev_status(prev), &plan, now),
        },
        Err(_) => preserved(HARNESS_CLAUDE, prev, prev_status(prev), &plan, now),
    }
}

fn probe_codex(
    outcome: CodexOutcome,
    prev: Option<&HarnessUsage>,
    now: DateTime<Utc>,
) -> HarnessUsage {
    match outcome {
        CodexOutcome::NotSignedIn => HarnessUsage {
            harness: HARNESS_CODEX.into(),
            plan: String::new(),
            windows: Vec::new(),
            checked_at: now.to_rfc3339(),
            status: STATUS_NOT_SIGNED_IN.into(),
        },
        CodexOutcome::Failed => preserved(HARNESS_CODEX, prev, prev_status(prev), "", now),
        CodexOutcome::Ready { plan, windows } => {
            let status = if windows.is_empty() {
                STATUS_NO_WINDOW.to_string()
            } else {
                String::new()
            };
            HarnessUsage {
                harness: HARNESS_CODEX.into(),
                plan,
                windows,
                checked_at: now.to_rfc3339(),
                status,
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
            harness: HARNESS_GROK.into(),
            plan: String::new(),
            windows: Vec::new(),
            checked_at: now.to_rfc3339(),
            status: STATUS_NOT_SIGNED_IN.into(),
        },
        GrokOutcome::Failed => preserved(HARNESS_GROK, prev, prev_status(prev), "", now),
        GrokOutcome::Ready { plan, windows } => {
            let status = if windows.is_empty() {
                STATUS_NO_WINDOW.to_string()
            } else {
                String::new()
            };
            HarnessUsage {
                harness: HARNESS_GROK.into(),
                plan,
                windows,
                checked_at: now.to_rfc3339(),
                status,
            }
        }
    }
}

fn prev_status(prev: Option<&HarnessUsage>) -> &str {
    prev.map(|p| p.status.as_str()).unwrap_or("")
}

fn preserved(
    harness: &str,
    prev: Option<&HarnessUsage>,
    status: &str,
    plan: &str,
    now: DateTime<Utc>,
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
    HarnessUsage {
        harness: harness.to_string(),
        plan,
        windows,
        checked_at: now.to_rfc3339(),
        status: status.to_string(),
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

fn parse_claude_usage(body: &str) -> Result<Vec<UsageWindow>, String> {
    let payload: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let session = object_bucket(&payload, "five_hour");
    let weekly = object_bucket(&payload, "seven_day_oauth_apps")
        .or_else(|| object_bucket(&payload, "seven_day"));
    let limits = payload
        .get("limits")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut raw_values = Vec::new();
    if let Some(bucket) = session {
        raw_values.push(util_number(bucket.get("utilization")));
    }
    if let Some(bucket) = weekly {
        raw_values.push(util_number(bucket.get("utilization")));
    }
    for entry in &limits {
        raw_values.push(util_number(
            entry.get("percent").or_else(|| entry.get("utilization")),
        ));
    }
    let percent_scale = raw_values.iter().flatten().any(|n| *n >= 1.0);

    let mut windows = Vec::new();
    if let Some(bucket) = session {
        if let Some(used) = normalize_used(util_number(bucket.get("utilization")), percent_scale) {
            windows.push(UsageWindow {
                label: "Session".into(),
                used,
                resets_at: reset_of(bucket),
            });
        }
    }
    if let Some(bucket) = weekly {
        if let Some(used) = normalize_used(util_number(bucket.get("utilization")), percent_scale) {
            windows.push(UsageWindow {
                label: "Weekly".into(),
                used,
                resets_at: reset_of(bucket),
            });
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for entry in &limits {
        let Some(name) = scoped_model_name(entry) else {
            continue;
        };
        let kind = entry.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if !seen.insert((name.clone(), kind.to_string())) {
            continue;
        }
        let Some(used) = normalize_used(
            util_number(entry.get("percent").or_else(|| entry.get("utilization"))),
            percent_scale,
        ) else {
            continue;
        };
        windows.push(UsageWindow {
            label: scoped_title(&name, kind),
            used,
            resets_at: reset_of(entry),
        });
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

fn normalize_used(raw: Option<f64>, percent_scale: bool) -> Option<f64> {
    let n = raw?;
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    let used = if percent_scale || n > 1.0 {
        n / 100.0
    } else {
        n
    };
    Some(used.clamp(0.0, 1.0))
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

fn scoped_title(name: &str, kind: &str) -> String {
    let text = kind.to_ascii_lowercase();
    let window = if text.contains("month") {
        "Monthly"
    } else if text.contains("week") || text.contains("day") {
        "Weekly"
    } else if text.contains("hour") || text.contains("session") {
        "Session"
    } else {
        ""
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

fn codex_window(window: &Value) -> Option<UsageWindow> {
    if !window.is_object() {
        return None;
    }
    let used_percent = window.get("usedPercent")?.as_f64()?;
    let mins = window
        .get("windowDurationMins")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let resets_at = window
        .get("resetsAt")
        .or_else(|| window.get("resets_at"))
        .map(reset_to_rfc3339)
        .unwrap_or_default();
    Some(UsageWindow {
        label: codex_label(mins),
        used: (used_percent / 100.0).clamp(0.0, 1.0),
        resets_at,
    })
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
    let mut windows = Vec::new();
    if let Some(limits) = limits {
        for key in ["primary", "secondary"] {
            if let Some(window) = limits.get(key).and_then(codex_window) {
                windows.push(window);
            }
        }
    }
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
    let mut child = codex_command(program).spawn().map_err(map_spawn_err)?;
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
    let parsed = codex_exchange(&mut transport);
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
    let credit = util_number(config.get("creditUsagePercent"));
    let build = grok_build_percent(result);
    let percent_scale = [credit, build].into_iter().flatten().any(|n| n >= 1.0);
    let resets_at = config
        .pointer("/currentPeriod/end")
        .map(reset_to_rfc3339)
        .unwrap_or_default();
    let label = grok_period_label(
        config
            .pointer("/currentPeriod/type")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    let mut windows = Vec::new();
    // A missing credit percent is "no window", not a made-up 0%.
    if let Some(used) = normalize_used(credit, percent_scale) {
        windows.push(UsageWindow {
            label,
            used,
            resets_at: resets_at.clone(),
        });
    }
    if credit.is_some() {
        if let Some(used) = normalize_used(build, percent_scale) {
            windows.push(UsageWindow {
                label: "Grok Build".into(),
                used,
                resets_at,
            });
        }
    }
    GrokParsed { plan, windows }
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

fn grok_build_percent(result: &Value) -> Option<f64> {
    let value = result
        .pointer("/config/productUsage/GrokBuild/usagePercent")
        .or_else(|| result.pointer("/productUsage/GrokBuild/usagePercent"));
    util_number(value)
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
    let mut child = grok_command(program).spawn().map_err(map_grok_spawn_err)?;
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
            return Err(format!("usage endpoint status {}", status.as_u16()));
        }
        response
            .text()
            .map_err(|_| "usage body unreadable".to_string())
    }

    fn codex(&self) -> CodexOutcome {
        if probe_denied() {
            return CodexOutcome::Failed;
        }
        match run_codex_process("codex") {
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
        match run_grok_process("grok") {
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

#[cfg(test)]
fn percent_left(used: f64) -> i64 {
    ((1.0 - used) * 100.0).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    fn probe_lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|p| p.into_inner())
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
        let _probe = probe_lock();
        let _home = crate::test_support::TempHome::new();
        probe_hits().store(0, Ordering::SeqCst);
        *test_io_slot().lock().unwrap_or_else(|p| p.into_inner()) = None;
        let prev_deny = std::env::var_os("K2_SUBSCRIPTION_PROBE");
        std::env::set_var("K2_SUBSCRIPTION_PROBE", "deny");
        f();
        *test_io_slot().lock().unwrap_or_else(|p| p.into_inner()) = None;
        match prev_deny {
            Some(v) => std::env::set_var("K2_SUBSCRIPTION_PROBE", v),
            None => std::env::remove_var("K2_SUBSCRIPTION_PROBE"),
        }
    }

    fn install(io: Arc<dyn UsageIo>) {
        *test_io_slot().lock().unwrap_or_else(|p| p.into_inner()) = Some(io);
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
            harness: name.into(),
            plan: "Max 20x".into(),
            windows,
            checked_at: checked.into(),
            status: status.into(),
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
        let _probe = probe_lock();
        let prev = std::env::var_os("CLAUDE_CONFIG_DIR");
        std::env::set_var("CLAUDE_CONFIG_DIR", "/tmp/k2-claude-config-dir");
        let paths = credential_file_paths();
        match prev {
            Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
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
        let main = include_str!("main.rs");
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
    fn fraction_utilization_stays_a_fraction() {
        let body = r#"{"five_hour":{"utilization":0.31,"resets_at":"2026-09-26T20:00:00Z"},"seven_day":{"utilization":0.2,"resets_at":"2026-10-03T00:00:00Z"}}"#;
        let windows = parse_claude_usage(body).expect("usage json");
        let session = windows
            .iter()
            .find(|w| w.label == "Session")
            .expect("session");
        assert!((session.used - 0.31).abs() < 1e-9);
        assert_eq!(percent_left(session.used), 69);
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
    fn grok_fraction_credit_stays_a_fraction() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"result":{"config":{"creditUsagePercent":0.22,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2026-09-26T00:00:00Z"}},"subscription_tier":"SuperGrok"}}"#;
        let parsed = parse_grok_billing(&grok_value(raw)).expect("fraction");
        assert!((parsed.windows[0].used - 0.22).abs() < 1e-9);
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

    #[test]
    fn grok_missing_percent_is_no_usage_window() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"result":{"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-09-19T00:00:00Z","end":"2026-09-26T00:00:00Z"},"productUsage":{"GrokBuild":{"usagePercent":4.0}}},"subscription_tier":"SuperGrok"}}"#;
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
