//! Out of usage (0.46.17, Rosson): an agent's LLM login ran out (a
//! subscription limit, or an API key's credit), and K2 says so on the
//! agent's Thread as its own system line, instead of the agent going quiet.
//!
//! This module is the pure part: what each harness's signal looks like,
//! the notice's words and buttons, and the one-notice-per-event rule. The
//! daemon (`llm_out_notice`) finds the agent and the login and posts it.
//!
//! **Signals, per harness** (the most reliable first):
//! - **Claude:** the `StopFailure` hook's `error` (`rate_limit`: the
//!   subscription limit, after Claude Code's own retries;
//!   `billing_error`: the API key's credit is gone)
//!   ([`claude_stop_failure`]); the statusLine's `rate_limits` at 100%
//!   ([`claude_statusline`]); the transcript's API-error record
//!   (`isApiErrorMessage`, "usage limit reached|<reset>") for a session
//!   with no hooks ([`transcript_line`]).
//! - **Codex:** the rollout's `token_count` event with `rate_limits`
//!   `primary` / `secondary` at 100% (Codex writes the 429's snapshot
//!   there), or an `error` event that says the usage limit was hit
//!   ([`transcript_line`]).
//! - **Grok, Gemini:** no per-session signal yet.
//!
//! **Never a secret.** A signal carries a kind, a reset time and when it
//! was recorded; the notice names the login by its label only.

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset, TimeZone};
use serde_json::{json, Value};

use crate::overlay::{NoticeAction, SystemNotice};

/// What ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutKind {
    /// A subscription's usage limit (until it resets).
    Usage,
    /// An API key's credit or quota.
    Credit,
}

/// Where a [`Hit`] came from (logs and tests).
pub const SOURCE_STOP_FAILURE: &str = "claude_stop_failure";
pub const SOURCE_STATUSLINE: &str = "claude_statusline";
pub const SOURCE_CLAUDE_TRANSCRIPT: &str = "claude_transcript";
pub const SOURCE_CODEX_ROLLOUT: &str = "codex_rollout";

/// One "this session is out" signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub kind: OutKind,
    /// When usage is back (unix seconds), when the signal says.
    pub resets_at: Option<i64>,
    /// When the record was written (unix seconds), for a transcript line
    /// (an old line read on catch-up is history, not news).
    pub recorded_at: Option<i64>,
    pub source: &'static str,
}

/// A transcript record older than this is history: no notice.
pub const MAX_RECORD_AGE_SECS: i64 = 15 * 60;
/// With no reset time, one notice per agent and login per hour.
pub const QUIET_WITHOUT_RESET_SECS: i64 = 60 * 60;
/// Never two notices for one agent and login closer than this.
pub const MIN_QUIET_SECS: i64 = 10 * 60;

/// [`NoticeAction::id`]: switch the server default to the named login
/// (`POST /cli/llm/accounts/switch {id}`, Admin and above).
pub const ACTION_SWITCH: &str = "llm_switch";
/// [`NoticeAction::id`]: the client opens its add-a-login sheet for
/// `body.tool` (the person signs in; nothing is posted).
pub const ACTION_ADD_LOGIN: &str = "llm_add_login";
/// The route [`ACTION_SWITCH`] posts to.
pub const SWITCH_ROUTE: &str = "/cli/llm/accounts/switch";

/// [`SystemNotice::code`]s.
pub const CODE_SWITCHED: &str = "llm.out_of_usage.switched";
pub const CODE_SWITCH_AVAILABLE: &str = "llm.out_of_usage.switch_available";
pub const CODE_NO_LOGIN: &str = "llm.out_of_usage.no_login";

/// At or over the limit (a percent, 0..100).
fn at_limit(pct: f64) -> bool {
    pct.is_finite() && pct >= 100.0 - 1e-9
}

/// Unix seconds from a number (seconds or milliseconds), a numeric
/// string, or an RFC 3339 string.
pub fn epoch_secs(v: &Value) -> Option<i64> {
    let from_num = |n: f64| -> Option<i64> {
        if !n.is_finite() || n <= 0.0 {
            return None;
        }
        let n = n as i64;
        Some(if n > 100_000_000_000 { n / 1000 } else { n })
    };
    match v {
        Value::Number(n) => from_num(n.as_f64()?),
        Value::String(s) => {
            let t = s.trim();
            if let Ok(n) = t.parse::<f64>() {
                return from_num(n);
            }
            DateTime::parse_from_rfc3339(t).ok().map(|d| d.timestamp())
        }
        _ => None,
    }
}

/// The reset in Claude's old limit text, `Claude AI usage limit
/// reached|1763600000`.
pub fn reset_from_text(text: &str) -> Option<i64> {
    let (_, tail) = text.rsplit_once('|')?;
    let digits: String = tail.trim().chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 9 {
        return None;
    }
    epoch_secs(&Value::String(digits))
}

/// What a vendor's error words say ran out, if anything.
pub fn kind_from_words(text: &str) -> Option<OutKind> {
    let t = text.to_ascii_lowercase();
    const CREDIT: [&str; 4] = ["credit balance", "insufficient_quota", "insufficient quota", "billing"];
    const USAGE: [&str; 5] = ["usage limit", "limit reached", "hit your limit", "rate limit reached for your plan", "usage_limit"];
    if CREDIT.iter().any(|w| t.contains(w)) {
        return Some(OutKind::Credit);
    }
    if USAGE.iter().any(|w| t.contains(w)) {
        return Some(OutKind::Usage);
    }
    None
}

/// Claude's `StopFailure` hook: its `error` (`rate_limit`,
/// `billing_error`, ...) and any reset its details carried.
pub fn claude_stop_failure(error: Option<&str>, resets_at: Option<i64>) -> Option<Hit> {
    let kind = match error?.trim() {
        "rate_limit" => OutKind::Usage,
        "billing_error" => OutKind::Credit,
        _ => return None,
    };
    Some(Hit { kind, resets_at, recorded_at: None, source: SOURCE_STOP_FAILURE })
}

/// The latest reset among windows at the limit (the time usage is back).
fn latest_reset(resets: impl Iterator<Item = Option<i64>>) -> Option<i64> {
    resets.flatten().max()
}

/// Claude's statusLine `rate_limits`: out when the session or the weekly
/// window is at 100%.
pub fn claude_statusline(rate_limits: &Value) -> Option<Hit> {
    let mut any = false;
    let mut resets = Vec::new();
    for key in ["five_hour", "seven_day"] {
        let Some(w) = rate_limits.get(key).filter(|w| w.is_object()) else { continue };
        let pct = w.get("used_percentage").or_else(|| w.get("utilization")).and_then(Value::as_f64);
        if pct.is_some_and(at_limit) {
            any = true;
            resets.push(w.get("resets_at").and_then(epoch_secs));
        }
    }
    any.then(|| Hit {
        kind: OutKind::Usage,
        resets_at: latest_reset(resets.into_iter()),
        recorded_at: None,
        source: SOURCE_STATUSLINE,
    })
}

/// A Codex `rate_limits` snapshot: out when `primary` or `secondary` is at
/// 100%. A model-specific snapshot describes another allowance (as the
/// token ledger reads it).
pub fn codex_rate_limits(snapshot: &Value, recorded_at: Option<i64>) -> Option<Hit> {
    let limit_id = snapshot.get("limit_id").or_else(|| snapshot.get("limitId")).and_then(Value::as_str);
    if limit_id.is_some_and(|id| id != "codex") {
        return None;
    }
    let mut any = false;
    let mut resets = Vec::new();
    for key in ["primary", "secondary"] {
        let Some(w) = snapshot.get(key).filter(|w| w.is_object()) else { continue };
        let pct = w.get("used_percent").or_else(|| w.get("usedPercent")).and_then(Value::as_f64);
        if !pct.is_some_and(at_limit) {
            continue;
        }
        any = true;
        let reset = w.get("resets_at").or_else(|| w.get("resetsAt")).and_then(epoch_secs).or_else(|| {
            let secs = w.get("resets_in_seconds").or_else(|| w.get("resetsInSeconds")).and_then(Value::as_i64)?;
            recorded_at.map(|at| at + secs)
        });
        resets.push(reset);
    }
    any.then(|| Hit {
        kind: OutKind::Usage,
        resets_at: latest_reset(resets.into_iter()),
        recorded_at,
        source: SOURCE_CODEX_ROLLOUT,
    })
}

fn record_secs(rec: &Value) -> Option<i64> {
    rec.get("timestamp").and_then(epoch_secs)
}

fn all_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Claude's API-error record: `isApiErrorMessage`, with `error`
/// `rate_limit` / `billing_error` or the limit words.
fn claude_record(rec: &Value) -> Option<Hit> {
    if rec.get("type").and_then(Value::as_str) != Some("assistant")
        || rec.get("isApiErrorMessage").and_then(Value::as_bool) != Some(true)
        || rec.get("isSidechain").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let text = all_text(rec.pointer("/message/content"));
    let kind = match rec.get("error").and_then(Value::as_str) {
        Some("rate_limit") => Some(OutKind::Usage),
        Some("billing_error") => Some(OutKind::Credit),
        _ => kind_from_words(&text),
    }?;
    Some(Hit { kind, resets_at: reset_from_text(&text), recorded_at: record_secs(rec), source: SOURCE_CLAUDE_TRANSCRIPT })
}

/// Codex's rollout: `token_count` at the limit, or an `error` event that
/// says the limit was hit.
fn codex_record(rec: &Value) -> Option<Hit> {
    if rec.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let payload = rec.get("payload")?;
    let at = record_secs(rec);
    match payload.get("type").and_then(Value::as_str)? {
        "token_count" => codex_rate_limits(payload.get("rate_limits").filter(|r| r.is_object())?, at),
        "error" | "stream_error" => {
            let info = payload.get("codex_error_info").map(|v| v.to_string()).unwrap_or_default();
            let words = format!("{} {info}", payload.get("message").and_then(Value::as_str).unwrap_or(""));
            let kind = kind_from_words(&words)?;
            Some(Hit { kind, resets_at: None, recorded_at: at, source: SOURCE_CODEX_ROLLOUT })
        }
        _ => None,
    }
}

/// One transcript line of a session's `harness` (`claude`, `codex`), if it
/// says the login ran out. A cheap substring check first: most lines are
/// never parsed.
pub fn transcript_line(harness: &str, line: &str) -> Option<Hit> {
    let wanted = match harness {
        "claude" => line.contains("isApiErrorMessage"),
        "codex" => line.contains("rate_limits") || line.contains("\"error\""),
        _ => false,
    };
    if !wanted {
        return None;
    }
    let rec: Value = serde_json::from_str(line.trim()).ok()?;
    match harness {
        "claude" => claude_record(&rec),
        _ => codex_record(&rec),
    }
}

/// What happens next for the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Auto-rotate moved the server default to this login (label).
    Switched { label: String },
    /// Another signed-in login has room; a person can switch to it.
    Available { id: String, label: String },
    /// No other login to switch to.
    Nothing,
}

/// The notice's facts. No token, ever: the login by its label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// The agent's address (`sales`, `sales/reviewer`).
    pub agent: String,
    /// `claude`, `codex`, ... (the add-a-login sheet's tool).
    pub tool: String,
    /// The login's label.
    pub login: String,
    pub kind: OutKind,
    pub resets_at: Option<i64>,
    pub next: Next,
}

/// Markdown's marks in a name, escaped (the Thread renders markdown).
fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.trim().chars() {
        if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '~' | '|') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `17:00 (UTC-07:00)` today, `Nov 21, 17:00 (UTC-07:00)` another day, in
/// the server's time (`offset`).
pub fn when_words(at: i64, now: i64, offset: FixedOffset) -> String {
    let (Some(t), Some(n)) = (offset.timestamp_opt(at, 0).single(), offset.timestamp_opt(now, 0).single()) else {
        return String::new();
    };
    let clock = if t.date_naive() == n.date_naive() { t.format("%H:%M").to_string() } else { t.format("%b %-d, %H:%M").to_string() };
    let secs = offset.local_minus_utc();
    let zone = if secs == 0 {
        "UTC".to_string()
    } else {
        let sign = if secs < 0 { '-' } else { '+' };
        let a = secs.abs();
        format!("UTC{sign}{:02}:{:02}", a / 3600, (a % 3600) / 60)
    };
    format!("{clock} ({zone})")
}

/// The first sentence: who is out, on which login, until when.
fn first_sentence(f: &Facts, now: i64, offset: FixedOffset) -> String {
    let (agent, login) = (plain(&f.agent), plain(&f.login));
    match f.kind {
        OutKind::Credit => format!("{agent} is out of credit on {login}."),
        OutKind::Usage => match f.resets_at.filter(|r| *r > now) {
            Some(r) => format!("{agent} is out of usage on {login} until {}.", when_words(r, now, offset)),
            None => format!("{agent} is out of usage on {login}."),
        },
    }
}

/// The notice's words.
pub fn notice_text(f: &Facts, now: i64, offset: FixedOffset) -> String {
    let first = first_sentence(f, now, offset);
    match &f.next {
        Next::Switched { label } => format!("{first} Switched to {}.", plain(label)),
        Next::Available { label, .. } => format!("{first} {} has room: switch to it, or add a login.", plain(label)),
        Next::Nothing => format!("{first} Add or switch a login."),
    }
}

fn add_login_action(tool: &str) -> NoticeAction {
    NoticeAction {
        id: ACTION_ADD_LOGIN.to_string(),
        label: "Add a login".to_string(),
        route: None,
        body: Some(json!({ "tool": tool })),
        workspace: None,
        done_at: None,
        done_by: None,
    }
}

/// The notice: kind, code and buttons. Switched: an info line. Otherwise
/// an action line with a person's one-tap fix: switch to the login with
/// room, and add a login.
pub fn notice(f: &Facts) -> SystemNotice {
    match &f.next {
        Next::Switched { .. } => SystemNotice::info(CODE_SWITCHED),
        Next::Available { id, label } => {
            let mut n = SystemNotice::action(CODE_SWITCH_AVAILABLE);
            n.actions.push(NoticeAction {
                id: ACTION_SWITCH.to_string(),
                label: format!("Switch to {}", label.trim()),
                route: Some(SWITCH_ROUTE.to_string()),
                body: Some(json!({ "id": id })),
                workspace: None,
                done_at: None,
                done_by: None,
            });
            n.actions.push(add_login_action(&f.tool));
            n
        }
        Next::Nothing => {
            let mut n = SystemNotice::action(CODE_NO_LOGIN);
            n.actions.push(add_login_action(&f.tool));
            n
        }
    }
}

/// One notice per event: an agent and login that got a notice stay quiet
/// until the limit resets (at least [`MIN_QUIET_SECS`]), or for
/// [`QUIET_WITHOUT_RESET_SECS`] when the reset isn't known. Every signal
/// for the same event (the hook, the statusLine, the transcript) lands
/// inside that window.
#[derive(Debug, Default)]
pub struct Episodes {
    until: HashMap<String, i64>,
}

impl Episodes {
    /// May `key` (agent + login) get a notice now? Claims it when yes.
    pub fn admit(&mut self, key: &str, resets_at: Option<i64>, now: i64) -> bool {
        self.until.retain(|_, until| *until > now);
        if self.until.contains_key(key) {
            return false;
        }
        let until = match resets_at.filter(|r| *r > now) {
            Some(r) => r.max(now + MIN_QUIET_SECS),
            None => now + QUIET_WITHOUT_RESET_SECS,
        };
        self.until.insert(key.to_string(), until);
        true
    }

    /// Take a claim back (the notice couldn't be posted).
    pub fn release(&mut self, key: &str) {
        self.until.remove(key);
    }
}

/// Is `hit` news at `now`? A record from a catch-up read that is older
/// than [`MAX_RECORD_AGE_SECS`], or a reset already past, is not.
pub fn is_news(hit: &Hit, now: i64) -> bool {
    if hit.recorded_at.is_some_and(|at| now - at > MAX_RECORD_AGE_SECS) {
        return false;
    }
    !hit.resets_at.is_some_and(|r| r <= now)
}

#[cfg(test)]
#[path = "out_of_usage_tests.rs"]
mod tests;
