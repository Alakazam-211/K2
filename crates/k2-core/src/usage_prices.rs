//! Estimated API-equivalent cost for the token ledger (Usage v2, Q2).
//!
//! Rates come from LiteLLM's `model_prices_and_context_window.json`
//! (MIT, github.com/BerriAI/litellm), the table T3 Code and ccusage price
//! against. A trimmed snapshot (Anthropic / OpenAI / xAI chat models: four
//! per-token rates) ships inside the binary; the daemon refreshes it once a
//! day into `~/.k2/usage/prices.json` ([`store_refreshed`]). No secrets go
//! in or out: the file is public.
//!
//! The figure is an **estimate, not the user's bill** (subscriptions are a
//! flat fee). A provider-reported cost (Grok `costUsdTicks`) always wins
//! over the table; a model the table doesn't know is "unpriced", never 0.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::Serialize;
use serde_json::Value;

/// The bundled snapshot (trimmed LiteLLM table).
const BUNDLED: &str = include_str!("../data/llm_prices.json");

/// Where the daily refresh fetches the full table.
pub const LITELLM_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// A refresh is due when the cached table is older than this.
pub const REFRESH_EVERY_SECS: i64 = 24 * 60 * 60;

/// Providers kept from the full table.
const PROVIDERS: &[&str] = &["anthropic", "openai", "xai", "text-completion-openai"];

/// Names that are never priced: Claude's locally made messages, and bare
/// family names that are ambiguous across generations.
const UNPRICEABLE: &[&str] = &["", "<synthetic>", "synthetic", "opus", "sonnet", "haiku", "fable", "grok", "gpt"];

/// Provider prefixes tried when a bare name isn't in the table.
const PREFIXES: &[&str] = &["xai/", "openai/", "anthropic/"];

/// USD per token.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

#[derive(Debug, Clone)]
pub struct PriceTable {
    models: HashMap<String, Rates>,
    /// `bundled` or `cached`.
    pub source: &'static str,
    pub fetched_at: Option<String>,
}

/// What a report says about its price table.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PriceInfo {
    pub source: String,
    pub fetched_at: Option<String>,
    pub models: usize,
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite() && *n >= 0.0)
}

impl PriceTable {
    /// Parse a trimmed snapshot (`{"fetched_at", "models": {name: [in, out, cr|null, cw|null]}}`).
    pub fn from_snapshot(text: &str, source: &'static str) -> Result<PriceTable, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("price table: {e}"))?;
        let models = v.get("models").and_then(|m| m.as_object()).ok_or("price table: no models")?;
        let mut out = HashMap::new();
        for (name, row) in models {
            let Some(a) = row.as_array() else { continue };
            let (Some(input), Some(output)) = (a.first().and_then(num), a.get(1).and_then(num)) else {
                continue;
            };
            let cache_read = a.get(2).and_then(num).unwrap_or(input);
            let cache_write = a.get(3).and_then(num).unwrap_or(input);
            out.insert(name.trim().to_ascii_lowercase(), Rates { input, output, cache_read, cache_write });
        }
        if out.is_empty() {
            return Err("price table: no priced models".into());
        }
        Ok(PriceTable {
            models: out,
            source,
            fetched_at: v.get("fetched_at").and_then(|s| s.as_str()).map(str::to_string),
        })
    }

    pub fn bundled() -> PriceTable {
        PriceTable::from_snapshot(BUNDLED, "bundled").expect("bundled price table parses")
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    pub fn info(&self) -> PriceInfo {
        PriceInfo { source: self.source.to_string(), fetched_at: self.fetched_at.clone(), models: self.models.len() }
    }

    /// The rates for a transcript model id, or `None` (unpriced).
    pub fn lookup(&self, model: &str) -> Option<Rates> {
        let mut key = model.trim().to_ascii_lowercase();
        if let Some(i) = key.find('[') {
            key.truncate(i);
        }
        let key = key.trim().to_string();
        let bare = key.rsplit('/').next().unwrap_or(&key).to_string();
        if UNPRICEABLE.contains(&bare.as_str()) {
            return None;
        }
        if let Some(r) = self.models.get(&key) {
            return Some(*r);
        }
        if let Some(r) = self.models.get(&bare) {
            return Some(*r);
        }
        for p in PREFIXES {
            if let Some(r) = self.models.get(&format!("{p}{bare}")) {
                return Some(*r);
            }
        }
        None
    }

    /// Cost of `input` (uncached) + cache read + cache write + output tokens.
    pub fn cost(&self, model: &str, input: i64, output: i64, cache_read: i64, cache_write: i64) -> Option<f64> {
        let r = self.lookup(model)?;
        Some(
            input.max(0) as f64 * r.input
                + output.max(0) as f64 * r.output
                + cache_read.max(0) as f64 * r.cache_read
                + cache_write.max(0) as f64 * r.cache_write,
        )
    }
}

/// Trim a full LiteLLM document into the snapshot shape (stamped
/// `fetched_at`). Errors when nothing usable is in it, so a broken
/// download never replaces a good table.
pub fn trim_litellm(text: &str, fetched_at: &str) -> Result<String, String> {
    let doc: Value = serde_json::from_str(text).map_err(|e| format!("litellm json: {e}"))?;
    let obj = doc.as_object().ok_or("litellm json: not an object")?;
    let mut models = serde_json::Map::new();
    let mut names: Vec<&String> = obj.keys().collect();
    names.sort();
    for name in names {
        let e = &obj[name];
        if name == "sample_spec" || !e.is_object() {
            continue;
        }
        let provider = e.get("litellm_provider").and_then(|p| p.as_str()).unwrap_or("");
        if !PROVIDERS.contains(&provider) {
            continue;
        }
        let mode = e.get("mode").and_then(|m| m.as_str());
        if !matches!(mode, None | Some("chat") | Some("responses")) {
            continue;
        }
        let (Some(i), Some(o)) = (
            e.get("input_cost_per_token").and_then(num),
            e.get("output_cost_per_token").and_then(num),
        ) else {
            continue;
        };
        let cr = e.get("cache_read_input_token_cost").and_then(num);
        let cw = e.get("cache_creation_input_token_cost").and_then(num);
        models.insert(name.trim().to_ascii_lowercase(), serde_json::json!([i, o, cr, cw]));
    }
    if models.len() < 10 {
        return Err(format!("litellm json: only {} priced models", models.len()));
    }
    let out = serde_json::json!({
        "source": "LiteLLM model_prices_and_context_window.json (MIT, github.com/BerriAI/litellm)",
        "fetched_at": fetched_at,
        "fields": ["input_per_token", "output_per_token", "cache_read_per_token", "cache_write_per_token"],
        "models": models,
    });
    serde_json::to_string(&out).map_err(|e| e.to_string())
}

/// `~/.k2/usage/prices.json`.
pub fn cache_path() -> PathBuf {
    crate::paths::k2_home().join("usage").join("prices.json")
}

fn slot() -> &'static RwLock<Option<Arc<PriceTable>>> {
    static SLOT: RwLock<Option<Arc<PriceTable>>> = RwLock::new(None);
    &SLOT
}

fn load_from_disk() -> PriceTable {
    match std::fs::read_to_string(cache_path()) {
        Ok(text) => match PriceTable::from_snapshot(&text, "cached") {
            Ok(t) => t,
            Err(e) => {
                crate::log_debug!("[usage-prices] cached table unreadable ({e}); using the bundled one");
                PriceTable::bundled()
            }
        },
        Err(_) => PriceTable::bundled(),
    }
}

/// The current table: the daily cache when it parses, else the bundled one.
pub fn table() -> Arc<PriceTable> {
    if let Some(t) = slot().read().unwrap_or_else(|p| p.into_inner()).clone() {
        return t;
    }
    let t = Arc::new(load_from_disk());
    *slot().write().unwrap_or_else(|p| p.into_inner()) = Some(t.clone());
    t
}

/// Forget the loaded table (after a refresh, or in tests).
pub fn reset() {
    *slot().write().unwrap_or_else(|p| p.into_inner()) = None;
}

/// Is the cached table missing or older than a day?
pub fn refresh_due(now: chrono::DateTime<chrono::Utc>) -> bool {
    let Ok(text) = std::fs::read_to_string(cache_path()) else {
        return true;
    };
    let fetched = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("fetched_at").and_then(|s| s.as_str()).map(str::to_string))
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok());
    match fetched {
        Some(t) => (now - t.with_timezone(&chrono::Utc)).num_seconds() >= REFRESH_EVERY_SECS,
        None => true,
    }
}

/// Trim a freshly downloaded LiteLLM document and atomically replace the
/// cache. Returns the number of priced models. A bad document leaves the
/// old cache untouched.
pub fn store_refreshed(full_json: &str, now: chrono::DateTime<chrono::Utc>) -> Result<usize, String> {
    let snapshot = trim_litellm(full_json, &now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))?;
    let table = PriceTable::from_snapshot(&snapshot, "cached")?;
    let path = cache_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("usage dir: {e}"))?;
    }
    crate::fs_atomic::atomic_write(&path, snapshot.as_bytes()).map_err(|e| format!("prices write: {e}"))?;
    let n = table.len();
    *slot().write().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(table));
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_table_prices_current_claude_codex_and_grok_models() {
        let t = PriceTable::bundled();
        assert!(t.len() > 50, "bundled table has {} models", t.len());
        assert!(t.lookup("claude-opus-5-5").is_some());
        assert!(t.lookup("claude-opus-5-5[1m]").is_some(), "bracket suffix stripped");
        assert!(t.lookup("CLAUDE-OPUS-5-5").is_some(), "case folded");
        assert!(t.lookup("gpt-5-codex").is_some());
        assert!(t.lookup("grok-code-fast-1").is_some(), "xai/ prefix tried");
        assert!(t.lookup("openai/gpt-5").is_some(), "provider prefix stripped");
        assert!(t.lookup("<synthetic>").is_none());
        assert!(t.lookup("opus").is_none());
        assert!(t.lookup("").is_none());
        assert!(t.lookup("not-a-real-model-xyz").is_none());
    }

    #[test]
    fn cost_is_rate_times_tokens_with_cache_defaults() {
        let snap = r#"{"fetched_at":"2026-10-16T00:00:00Z","models":{"m-full":[0.000001,0.00001,0.0000001,0.00000125],"m-nocache":[0.000002,0.00002,null,null]}}"#;
        let t = PriceTable::from_snapshot(snap, "cached").expect("snapshot");
        let full = t.cost("m-full", 1000, 100, 10_000, 400).expect("priced");
        let want = 1000.0 * 0.000001 + 100.0 * 0.00001 + 10_000.0 * 0.0000001 + 400.0 * 0.00000125;
        assert!((full - want).abs() < 1e-12, "{full} vs {want}");
        // Missing cache rates bill as plain input, never as free.
        let nocache = t.cost("m-nocache", 0, 0, 1000, 1000).expect("priced");
        assert!((nocache - 2000.0 * 0.000002).abs() < 1e-12);
        assert!(t.cost("unknown", 1, 1, 1, 1).is_none());
        assert_eq!(t.fetched_at.as_deref(), Some("2026-10-16T00:00:00Z"));
    }

    #[test]
    fn trim_keeps_priced_chat_models_of_three_providers_only() {
        let mut doc = serde_json::Map::new();
        doc.insert("sample_spec".into(), serde_json::json!({"input_cost_per_token": 1.0, "output_cost_per_token": 1.0}));
        for i in 0..12 {
            doc.insert(format!("claude-x-{i}"), serde_json::json!({"litellm_provider": "anthropic", "mode": "chat", "input_cost_per_token": 0.000003, "output_cost_per_token": 0.000015, "cache_read_input_token_cost": 0.0000003}));
        }
        doc.insert("embed".into(), serde_json::json!({"litellm_provider": "openai", "mode": "embedding", "input_cost_per_token": 0.1, "output_cost_per_token": 0.1}));
        doc.insert("other".into(), serde_json::json!({"litellm_provider": "bedrock", "mode": "chat", "input_cost_per_token": 0.1, "output_cost_per_token": 0.1}));
        doc.insert("half".into(), serde_json::json!({"litellm_provider": "xai", "mode": "chat", "input_cost_per_token": 0.1}));
        let text = serde_json::to_string(&Value::Object(doc)).expect("doc");
        let snap = trim_litellm(&text, "2026-10-16T00:00:00Z").expect("trim");
        let t = PriceTable::from_snapshot(&snap, "cached").expect("parse");
        assert_eq!(t.len(), 12);
        assert!(t.lookup("embed").is_none());
        assert!(t.lookup("other").is_none());
        assert!(t.lookup("half").is_none(), "a half-priced model is unpriced, not cheap");
        let r = t.lookup("claude-x-3").expect("kept");
        assert!((r.cache_read - 0.0000003).abs() < 1e-15);
        assert!((r.cache_write - 0.000003).abs() < 1e-15, "missing write rate = input rate");
        assert!(trim_litellm("{\"a\":1}", "x").is_err(), "an empty download never replaces the table");
    }
}
