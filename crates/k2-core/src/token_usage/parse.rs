//! Transcript line parsers (Usage v2, P2/P4/P6).
//!
//! One [`Draft`] = one model request, as finely as the log allows. Every
//! harness is normalised at ingest so the four counts mean the same thing
//! everywhere:
//!
//! - `input` = **uncached** input. Claude already reports it that way;
//!   Codex and Grok report input *including* cache, so cache read (and
//!   Grok cache write) is subtracted (T3 does the same).
//! - `output` includes reasoning. `reasoning` is a subset, kept only for
//!   the token mix (Grok reasoning is never added to output: B7b).
//! - `cache_read` / `cache_write` as reported.
//!
//! Parsers never copy message content: only ids, model names, cwd, counts
//! and timestamps leave a line.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One request's usage, ready to upsert.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub turn_key: String,
    /// The transcript cwd (stored canonical; attribution happens at query).
    pub workspace: String,
    pub model: String,
    pub session_id: String,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub reasoning: i64,
    /// The raw timestamp text (kept verbatim).
    pub recorded_at: String,
    pub recorded_ms: Option<i64>,
    /// Provider-reported cost in USD (Grok `costUsdTicks`).
    pub provider_cost_usd: Option<f64>,
}

/// A Codex `rate_limits` snapshot seen in a session log (live limits, Q5).
#[derive(Debug, Clone, PartialEq)]
pub struct CodexLimits {
    pub session_id: String,
    pub recorded_ms: i64,
    pub snapshot: Value,
}

/// Parser state carried between lines and across incremental reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Carry {
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub model: String,
}

/// Grok reports cost in integer ticks: 1 USD = 10^10 ticks.
pub const GROK_TICKS_PER_USD: f64 = 10_000_000_000.0;

fn json_i64(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    if let Some(u) = v.as_u64() {
        return i64::try_from(u).ok();
    }
    let f = v.as_f64()?;
    if f.is_finite() && f.fract() == 0.0 && (i64::MIN as f64..=i64::MAX as f64).contains(&f) {
        Some(f as i64)
    } else {
        None
    }
}

fn field(obj: &Value, key: &str) -> Option<i64> {
    obj.get(key).and_then(json_i64)
}

fn nn(v: Option<i64>) -> i64 {
    v.unwrap_or(0).max(0)
}

pub(crate) fn timestamp_of(v: &Value) -> String {
    match v.get("timestamp") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// `true` when the four counts are all zero (nothing to record).
fn empty(d: &Draft) -> bool {
    d.input == 0 && d.output == 0 && d.cache_read == 0 && d.cache_write == 0
}

// ── Claude ──────────────────────────────────────────────────────────

fn claude_usage(v: &Value) -> Option<&Value> {
    let has = |u: &Value| {
        ["input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"]
            .iter()
            .any(|k| field(u, k).is_some())
    };
    if let Some(u) = v.get("message").and_then(|m| m.get("usage")).filter(|u| has(u)) {
        return Some(u);
    }
    v.get("usage").filter(|u| has(u))
}

/// One Claude transcript line. `file_session` is the file stem (the
/// conversation id) used when the line has no `sessionId`.
pub fn claude(v: &Value, carry: &mut Carry, file_session: &str) -> Option<Draft> {
    if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()).filter(|s| !s.is_empty()) {
        carry.cwd = cwd.to_string();
    }
    if let Some(sid) = v.get("sessionId").and_then(|c| c.as_str()).filter(|s| !s.is_empty()) {
        carry.session_id = sid.to_string();
    }
    let usage = claude_usage(v)?;
    // Provider message id only: a forked or resumed copy is the same row.
    let turn_key = v.get("message").and_then(|m| m.get("id")).and_then(|id| id.as_str()).filter(|s| !s.is_empty())?;
    let model = v.get("message").and_then(|m| m.get("model")).and_then(|m| m.as_str()).unwrap_or("");
    let recorded_at = timestamp_of(v);
    let session_id = if carry.session_id.is_empty() { file_session.to_string() } else { carry.session_id.clone() };
    let d = Draft {
        turn_key: turn_key.to_string(),
        workspace: carry.cwd.clone(),
        model: model.to_string(),
        session_id,
        // Claude input is already the uncached part.
        input: nn(field(usage, "input_tokens")),
        output: nn(field(usage, "output_tokens")),
        cache_read: nn(field(usage, "cache_read_input_tokens")),
        cache_write: nn(field(usage, "cache_creation_input_tokens")),
        reasoning: 0,
        recorded_ms: super::epoch_ms(&recorded_at),
        recorded_at,
        provider_cost_usd: None,
    };
    if empty(&d) {
        return None;
    }
    Some(d)
}

// ── Codex ───────────────────────────────────────────────────────────

/// One Codex rollout line. `offset` is the line's byte offset in the file
/// and `file_key` the file path: the fallback key when a `token_count`
/// carries no cumulative totals (older Codex).
pub fn codex(v: &Value, carry: &mut Carry, offset: u64, file_key: &str) -> (Option<Draft>, Option<CodexLimits>) {
    let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let payload = v.get("payload");
    match kind {
        "session_meta" => {
            if let Some(p) = payload {
                if carry.session_id.is_empty() {
                    if let Some(id) = p.get("id").and_then(|s| s.as_str()) {
                        carry.session_id = id.to_string();
                    }
                }
                if carry.cwd.is_empty() {
                    if let Some(cwd) = p.get("cwd").and_then(|s| s.as_str()) {
                        carry.cwd = cwd.to_string();
                    }
                }
            }
            return (None, None);
        }
        "turn_context" => {
            if let Some(p) = payload {
                if let Some(m) = p.get("model").and_then(|s| s.as_str()).filter(|s| !s.is_empty()) {
                    carry.model = m.to_string();
                }
                if let Some(cwd) = p.get("cwd").and_then(|s| s.as_str()).filter(|s| !s.is_empty()) {
                    carry.cwd = cwd.to_string();
                }
            }
            return (None, None);
        }
        "event_msg" => {}
        _ => return (None, None),
    }
    let Some(payload) = payload else { return (None, None) };
    if payload.get("type").and_then(|t| t.as_str()) != Some("token_count") {
        return (None, None);
    }
    let recorded_at = timestamp_of(v);
    let recorded_ms = super::epoch_ms(&recorded_at);
    let limits = payload
        .get("rate_limits")
        .filter(|r| r.is_object())
        .filter(|r| {
            // Model-specific snapshots describe another allowance (T3).
            r.get("limit_id").or_else(|| r.get("limitId")).and_then(|s| s.as_str()).map_or(true, |id| id == "codex")
        })
        .and_then(|r| {
            recorded_ms.map(|ms| CodexLimits { session_id: carry.session_id.clone(), recorded_ms: ms, snapshot: r.clone() })
        });
    let Some(last) = payload.pointer("/info/last_token_usage").filter(|l| l.is_object()) else {
        return (None, limits);
    };
    let raw_input = nn(field(last, "input_tokens"));
    let cached = nn(field(last, "cached_input_tokens"));
    let output = nn(field(last, "output_tokens"));
    let reasoning = nn(field(last, "reasoning_output_tokens")).min(output);
    // One row per model request. The cumulative totals identify the
    // request: an identical re-emit, or a forked copy of the history,
    // lands on the same key instead of counting twice (B2).
    let turn_key = match payload.pointer("/info/total_token_usage").filter(|t| t.is_object()) {
        Some(total) => format!(
            "codex:{}:{}:{}:{}:{}:{}:{}",
            nn(field(total, "input_tokens")),
            nn(field(total, "cached_input_tokens")),
            nn(field(total, "output_tokens")),
            nn(field(total, "reasoning_output_tokens")),
            nn(field(total, "total_tokens")),
            raw_input,
            output
        ),
        None => {
            let scope = if carry.session_id.is_empty() { file_key } else { carry.session_id.as_str() };
            format!("codex:{scope}@{offset}")
        }
    };
    let d = Draft {
        turn_key,
        workspace: carry.cwd.clone(),
        model: carry.model.clone(),
        session_id: carry.session_id.clone(),
        input: (raw_input - cached).max(0),
        output,
        cache_read: cached,
        cache_write: 0,
        reasoning,
        recorded_at,
        recorded_ms,
        provider_cost_usd: None,
    };
    if empty(&d) {
        return (None, limits);
    }
    (Some(d), limits)
}

// ── Grok ────────────────────────────────────────────────────────────

struct GrokCounts {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
    cost_usd: Option<f64>,
}

fn grok_counts(obj: &Value) -> GrokCounts {
    let cache_read = nn(field(obj, "cachedReadTokens"));
    let cache_write = nn(field(obj, "cacheCreationTokens").or_else(|| field(obj, "cachedWriteTokens")));
    let output = nn(field(obj, "outputTokens"));
    GrokCounts {
        // Grok input includes the cached part (T3: provider-grok usage).
        input: (nn(field(obj, "inputTokens")) - cache_read - cache_write).max(0),
        output,
        cache_read,
        cache_write,
        // Reasoning is inside output: never added (B7b).
        reasoning: nn(field(obj, "reasoningTokens").or_else(|| field(obj, "thoughtTokens"))).min(output),
        cost_usd: obj
            .get("costUsdTicks")
            .and_then(|t| t.as_f64())
            .filter(|t| t.is_finite() && *t >= 0.0)
            .map(|t| t / GROK_TICKS_PER_USD),
    }
}

fn grok_size(c: &GrokCounts) -> i64 {
    c.input + c.output + c.cache_read + c.cache_write
}

/// One Grok `updates.jsonl` line. `carry.cwd` is the session's workspace
/// (from `summary.json`) and `carry.session_id` its folder name.
pub fn grok(v: &Value, carry: &Carry) -> Vec<Draft> {
    let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
    if method != "session/update" && method != "_x.ai/session/update" {
        return Vec::new();
    }
    let Some(update) = v.pointer("/params/update") else { return Vec::new() };
    let Some(usage) = update.get("usage").filter(|u| u.is_object()) else { return Vec::new() };
    let prompt = update
        .get("prompt_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| v.pointer("/params/_meta/eventId").and_then(|s| s.as_str()).filter(|s| !s.is_empty()));
    let Some(prompt) = prompt else { return Vec::new() };
    let session_id = v
        .pointer("/params/sessionId")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| carry.session_id.clone());
    let recorded_at = timestamp_of(v);
    let recorded_ms = v
        .pointer("/params/_meta/agentTimestampMs")
        .and_then(json_i64)
        .filter(|ms| *ms >= 1_000_000_000_000)
        .or_else(|| super::epoch_ms(&recorded_at));
    let make = |model: &str, c: &GrokCounts, cost: Option<f64>| Draft {
        turn_key: format!("{prompt}\t{model}"),
        workspace: carry.cwd.clone(),
        model: model.to_string(),
        session_id: session_id.clone(),
        input: c.input,
        output: c.output,
        cache_read: c.cache_read,
        cache_write: c.cache_write,
        reasoning: c.reasoning,
        recorded_at: recorded_at.clone(),
        recorded_ms,
        provider_cost_usd: cost,
    };
    let top = grok_counts(usage);
    let per_model: Vec<(String, GrokCounts)> = usage
        .get("modelUsage")
        .and_then(|m| m.as_object())
        .map(|m| {
            m.iter()
                .filter(|(name, body)| !name.is_empty() && body.is_object())
                .map(|(name, body)| (name.clone(), grok_counts(body)))
                .filter(|(_, c)| grok_size(c) > 0)
                .collect()
        })
        .unwrap_or_default();
    if per_model.is_empty() {
        if grok_size(&top) == 0 {
            return Vec::new();
        }
        let model = update
            .get("model")
            .and_then(|s| s.as_str())
            .or_else(|| usage.get("model").and_then(|s| s.as_str()))
            .unwrap_or("");
        return vec![make(model, &top, top.cost_usd)];
    }
    // Cost split (T3): a model's own ticks win; the rest of the top-level
    // cost is shared by token count among the models without ticks.
    let own: f64 = per_model.iter().filter_map(|(_, c)| c.cost_usd).sum();
    let unticked: i64 = per_model.iter().filter(|(_, c)| c.cost_usd.is_none()).map(|(_, c)| grok_size(c)).sum();
    let remaining = top.cost_usd.map(|t| (t - own).max(0.0));
    per_model
        .iter()
        .map(|(model, c)| {
            let cost = c.cost_usd.or_else(|| match remaining {
                Some(r) if unticked > 0 => Some(r * grok_size(c) as f64 / unticked as f64),
                _ => None,
            });
            make(model, c, cost)
        })
        .collect()
}
