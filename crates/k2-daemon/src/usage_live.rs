//! Usage v2 limits (prd-usage-v2 §3): live feeds from running sessions
//! (Rosson Q5) and the every-login limits document.
//!
//! **Live feeds.** Running sessions already learn their own limits:
//! - Claude pipes `rate_limits` to its statusLine on every turn; K2's
//!   `~/.k2/hooks/statusline.sh` (installed only into an empty statusLine
//!   slot) forwards it through `notify.sh` as event `StatusLine`
//!   ([`ingest_statusline_hook`]).
//! - Codex writes `rate_limits` on its session log's `token_count` events
//!   (the snapshot its app-server sends as `account/rateLimits/updated`);
//!   the token-ledger scan hands the newest per login to
//!   [`ingest_codex_rollout`].
//!
//! A live reading is written onto the wallet login it belongs to and, when
//! that login is the tool's default, onto the shared cache row. It never
//! replaces a newer reading. The 15-minute poll then skips that login
//! (`subscription_usage::live_fresh`). Grok has no live feed (its
//! `_x.ai/billing` probe stays); Gemini has no limits API.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use k2_core::llm_accounts::{self as wallet, store, Tool};

use crate::subscription_usage::{self as su, HarnessUsage, UsageWindow};

pub const SOURCE_STATUSLINE: &str = "live_statusline";
pub const SOURCE_ROLLOUT: &str = "live_rollout";

/// The hook event name `statusline.sh` sends.
pub const STATUSLINE_EVENT: &str = "StatusLine";

fn reset_rfc3339(v: Option<&Value>) -> String {
    match v {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .and_then(|secs| {
                let ms = if secs > 1_000_000_000_000 { secs } else { secs.saturating_mul(1000) };
                DateTime::<Utc>::from_timestamp_millis(ms)
            })
            .map(|d| d.to_rfc3339())
            .unwrap_or_default(),
        Some(Value::String(s)) => {
            let t = s.trim();
            match t.parse::<i64>() {
                Ok(n) => reset_rfc3339(Some(&json!(n))),
                Err(_) => t.to_string(),
            }
        }
        _ => String::new(),
    }
}

/// Claude statusLine `rate_limits` → windows. `used_percentage` (or the
/// OAuth-shaped `utilization`) is a percent, 0..100.
pub fn claude_statusline_windows(rate_limits: &Value) -> Vec<UsageWindow> {
    let mut out = Vec::new();
    for (key, label, kind, secs) in
        [("five_hour", "Session", "session", 5 * 3600), ("seven_day", "Weekly", "weekly", 7 * 86_400)]
    {
        let Some(w) = rate_limits.get(key).filter(|w| w.is_object()) else { continue };
        let pct = w
            .get("used_percentage")
            .or_else(|| w.get("utilization"))
            .and_then(|v| v.as_f64())
            .filter(|n| n.is_finite() && *n >= 0.0);
        let Some(pct) = pct else { continue };
        out.push(su::window(key, label, kind, (pct / 100.0).clamp(0.0, 1.0), reset_rfc3339(w.get("resets_at")), Some(secs)));
    }
    out
}

/// The wallet slot a home folder is (`~/.k2/llm-accounts/<tool>/<id>`).
pub fn slot_account(tool: Tool, home: &str) -> Option<String> {
    let home = home.trim().trim_end_matches('/');
    if home.is_empty() {
        return None;
    }
    let canon = |p: &Path| dunce::canonicalize(&p).unwrap_or_else(|_| p.to_path_buf());
    let dir = canon(Path::new(home));
    let parent = dir.parent()?;
    if parent != canon(&store::tool_dir(tool)) {
        return None;
    }
    let id = dir.file_name()?.to_str()?.to_string();
    (!id.starts_with('.')).then_some(id)
}

fn live_owner(tool: Tool) -> Option<String> {
    let db = k2_core::db::try_shared()?;
    let conn = db.lock();
    wallet::live_owner(&conn, tool).ok().flatten()
}

fn wallet_known(id: &str) -> bool {
    let Some(db) = k2_core::db::try_shared() else { return false };
    let conn = db.lock();
    wallet::get(&conn, id).ok().flatten().is_some_and(|e| e.removed_at.is_none())
}

fn newer_than(prev: Option<&HarnessUsage>, observed: DateTime<Utc>) -> bool {
    prev.and_then(|p| DateTime::parse_from_rfc3339(&p.checked_at).ok())
        .is_some_and(|t| t.with_timezone(&Utc) > observed)
}

/// A live reading merged over the previous one: the live windows, plus
/// any previous window the feed doesn't carry (Claude's model-scoped
/// weeks) while it is still open.
fn merged(
    tool: Tool,
    prev: Option<&HarnessUsage>,
    windows: &[UsageWindow],
    plan: &str,
    observed: DateTime<Utc>,
    source: &str,
    account: Option<&str>,
) -> HarnessUsage {
    let mut out: Vec<UsageWindow> = windows.to_vec();
    if let Some(p) = prev {
        for w in &p.windows {
            let covered = out.iter().any(|l| (!w.id.is_empty() && l.id == w.id) || l.label == w.label);
            let open = DateTime::parse_from_rfc3339(&w.resets_at).map(|t| t.with_timezone(&Utc) > observed).unwrap_or(false);
            if !covered && open {
                out.push(w.clone());
            }
        }
    }
    HarnessUsage {
        harness: tool.as_str().to_string(),
        plan: if plan.is_empty() { prev.map(|p| p.plan.clone()).unwrap_or_default() } else { plan.to_string() },
        windows: out,
        checked_at: observed.to_rfc3339(),
        status: String::new(),
        error: None,
        account_id: account.map(str::to_string),
        attempted_at: None,
        source: source.to_string(),
        ..Default::default()
    }
}

/// Write a live reading for `account` (or the tool's default login when
/// `None`). Returns whether anything was written.
pub fn apply_live(
    tool: Tool,
    account: Option<String>,
    windows: Vec<UsageWindow>,
    plan: &str,
    observed: DateTime<Utc>,
    source: &str,
) -> bool {
    if k2_core::airgap::enabled() || windows.is_empty() {
        return false;
    }
    let live = live_owner(tool);
    let account = account.filter(|a| !a.is_empty()).or_else(|| live.clone());
    let mut wrote = false;
    if let Some(id) = account.as_deref().filter(|id| wallet_known(id)) {
        let prev = su::wallet_row(id);
        if !newer_than(prev.as_ref(), observed) {
            let row = merged(tool, prev.as_ref(), &windows, plan, observed, source, Some(id));
            su::record_on_wallet(id, &row);
            wrote = true;
        }
    }
    // The shared cache holds the default login's row (or the CLI's own
    // login when the wallet has none).
    if account == live {
        wrote |= su::update_cache_row(tool.as_str(), |prev| {
            // Never write over another login's row.
            let other = match (prev.and_then(|p| p.account_id.as_deref()), account.as_deref()) {
                (Some(theirs), Some(ours)) => theirs != ours,
                _ => false,
            };
            if other || newer_than(prev, observed) {
                return None;
            }
            Some(merged(tool, prev, &windows, plan, observed, source, account.as_deref()))
        });
    }
    if wrote {
        crate::session_events::emit_subscription_usage_changed(Some(tool.as_str()), account.as_deref(), source);
    }
    wrote
}

/// `/hook/event` with event `StatusLine` (from `statusline.sh`): body
/// `{"k2_config_dir": "…", "statusline": <Claude's statusLine JSON>}`.
/// The transport already proved the session passport for this pane.
/// Returns whether a reading was written.
pub fn ingest_statusline_hook(body: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return false };
    let Some(limits) = v.pointer("/statusline/rate_limits").filter(|r| r.is_object()) else { return false };
    let windows = claude_statusline_windows(limits);
    let account = v
        .get("k2_config_dir")
        .and_then(|d| d.as_str())
        .and_then(|d| slot_account(Tool::Claude, d));
    apply_live(Tool::Claude, account, windows, "", Utc::now(), SOURCE_STATUSLINE)
}

/// The newest Codex `rate_limits` a ledger scan saw for one login.
pub fn ingest_codex_rollout(seen: &k2_core::token_usage::CodexLimitsSeen) -> bool {
    let Some(observed) = DateTime::<Utc>::from_timestamp_millis(seen.recorded_ms) else { return false };
    // An old session log is history, not a live reading.
    if Utc::now().signed_duration_since(observed) > chrono::Duration::hours(6) {
        return false;
    }
    let (plan, windows) = su::codex_windows(&seen.snapshot, Some(seen.recorded_ms));
    let account = (!seen.account_id.is_empty()).then(|| seen.account_id.clone());
    apply_live(Tool::Codex, account, windows, &plan, observed, SOURCE_ROLLOUT)
}

// ── The every-login document (§3.3) ─────────────────────────────────

fn login_json(row: &HarnessUsage) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("state".into(), json!(row.state));
    m.insert("status".into(), json!(row.status));
    m.insert("error".into(), json!(row.error));
    m.insert("plan".into(), json!(row.plan));
    m.insert("checkedAt".into(), json!(row.checked_at));
    m.insert("attemptedAt".into(), json!(row.attempted_at));
    m.insert("source".into(), json!(row.source));
    m.insert("nextProbeAt".into(), json!(row.next_probe_at));
    m.insert("windows".into(), serde_json::to_value(&row.windows).unwrap_or_else(|_| json!([])));
    m
}

/// Every login of every tool with its limits, default first, then pinned,
/// then idle. `kick` = start a background re-check for windows past their
/// reset (the GET does; tests don't).
pub fn limits_doc(now: DateTime<Utc>, kick: bool) -> Value {
    let airgap = k2_core::airgap::enabled();
    let shared = su::cached_rows();
    let mut tools = Vec::new();
    let mut idle_past_reset: Vec<String> = Vec::new();
    let mut shared_past_reset = false;
    let db = k2_core::db::try_shared();
    for tool in Tool::ALL {
        let supported = tool.subscription_supported();
        let mut logins: Vec<(i64, Value)> = Vec::new();
        let shared_row = shared.iter().find(|h| h.harness == tool.as_str()).cloned();
        let (entries, active, live, pins) = match &db {
            Some(db) => {
                let conn = db.lock();
                let entries = wallet::list_for(&conn, tool).unwrap_or_default();
                let active = wallet::active_id(&conn, tool).ok().flatten();
                let live = wallet::live_owner(&conn, tool).ok().flatten();
                let pins: Vec<(String, Value)> = wallet::pins::list_pins(&conn)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|p| p.tool == tool.as_str())
                    .map(|p| (p.account_id.clone(), crate::llm_accounts_routes::pin_json(&conn, &p)))
                    .collect();
                (entries, active, live, pins)
            }
            None => (Vec::new(), None, None, Vec::new()),
        };
        for e in &entries {
            let is_live = live.as_deref() == Some(e.id.as_str());
            let mut row: HarnessUsage = e
                .usage_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| HarnessUsage { harness: tool.as_str().into(), ..Default::default() });
            // The live login's freshest numbers may be on the shared row.
            if is_live {
                if let Some(s) = shared_row.as_ref().filter(|s| s.account_id.as_deref() == Some(e.id.as_str())) {
                    let newer = |a: &str, b: &str| {
                        match (DateTime::parse_from_rfc3339(a), DateTime::parse_from_rfc3339(b)) {
                            (Ok(a), Ok(b)) => a > b,
                            (Ok(_), Err(_)) => true,
                            _ => false,
                        }
                    };
                    if newer(&s.checked_at, &row.checked_at) || row.checked_at.is_empty() {
                        row = s.clone();
                    }
                }
            }
            if e.is_api_key() || !supported {
                row.windows.clear();
                row.status = su::STATUS_NOT_SUPPORTED.into();
                row.error = None;
            }
            if airgap {
                row.windows.clear();
                row.status = su::STATUS_AIRGAP.into();
                row.error = None;
            }
            let key = if is_live { tool.as_str().to_string() } else { format!("acct:{}", e.id) };
            let row = su::decorate_row(row, now, &key);
            if row.windows.iter().any(|w| w.reset_passed) {
                if is_live {
                    shared_past_reset = true;
                } else {
                    idle_past_reset.push(e.id.clone());
                }
            }
            let pinned: Vec<Value> = pins.iter().filter(|(id, _)| id == &e.id).map(|(_, v)| v.clone()).collect();
            let is_default = active.as_deref() == Some(e.id.as_str());
            let mut m = login_json(&row);
            m.insert("accountId".into(), json!(e.id));
            m.insert("label".into(), json!(e.label));
            m.insert("email".into(), json!(e.email));
            m.insert("kind".into(), json!(e.kind));
            m.insert("isDefault".into(), json!(is_default));
            m.insert("isLive".into(), json!(is_live));
            if row.plan.is_empty() {
                m.insert("plan".into(), json!(e.plan.clone().unwrap_or_default()));
            }
            let rank = if is_default { 0 } else if !pinned.is_empty() { 1 } else { 2 };
            m.insert("pinned".into(), Value::Array(pinned));
            logins.push((rank * 1_000_000 + e.position, Value::Object(m)));
        }
        // A signed-in CLI with no wallet entry: the shared cache row.
        if entries.is_empty() && supported {
            if let Some(row) = shared_row.clone() {
                let mut row = row;
                if airgap {
                    row.windows.clear();
                    row.status = su::STATUS_AIRGAP.into();
                }
                let row = su::decorate_row(row, now, tool.as_str());
                if row.windows.iter().any(|w| w.reset_passed) {
                    shared_past_reset = true;
                }
                let mut m = login_json(&row);
                m.insert("accountId".into(), Value::Null);
                m.insert("label".into(), json!(tool.display()));
                m.insert("email".into(), Value::Null);
                m.insert("kind".into(), json!("unmanaged"));
                m.insert("isDefault".into(), json!(true));
                m.insert("isLive".into(), json!(true));
                m.insert("pinned".into(), json!([]));
                logins.push((0, Value::Object(m)));
            }
        }
        logins.sort_by_key(|(rank, _)| *rank);
        tools.push(json!({
            "tool": tool.as_str(),
            "display": tool.display(),
            "supported": supported,
            "logins": logins.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        }));
    }
    if kick && !airgap {
        if shared_past_reset {
            su::kick_recheck();
        }
        if !idle_past_reset.is_empty() {
            kick_idle_recheck(idle_past_reset);
        }
    }
    json!({ "generatedAt": now.to_rfc3339(), "airgap": airgap, "tools": tools })
}

fn idle_running() -> &'static std::sync::atomic::AtomicBool {
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &RUNNING
}

/// Re-check idle logins whose window reset (one background pass at a time).
fn kick_idle_recheck(ids: Vec<String>) {
    use std::sync::atomic::Ordering;
    if idle_running().swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new().name("usage-idle-recheck".into()).spawn(move || {
        for id in ids {
            let entry = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                wallet::get(&conn, &id).ok().flatten()
            };
            if let Some(e) = entry {
                if su::backing_off(&format!("acct:{id}"), Utc::now()).is_none() {
                    if let Err(err) = crate::llm_accounts_runtime::probe_idle_usage(&e) {
                        k2_core::log_debug!("[usage] re-check {id}: {err}");
                    }
                }
            }
        }
        idle_running().store(false, Ordering::SeqCst);
    });
    if spawned.is_err() {
        idle_running().store(false, Ordering::SeqCst);
    }
}

/// `POST /cli/usage/limits/refresh` body: `{tool?}` or `{accountId?}`.
/// The default login goes through the shared single-flight probe; an idle
/// login through its own slot probe. Answers the limits document.
pub fn refresh(tool: Option<&str>, account_id: Option<&str>) -> Result<Value, (u16, String)> {
    if k2_core::airgap::enabled() {
        return Ok(limits_doc(Utc::now(), false));
    }
    if let Some(id) = account_id.filter(|s| !s.is_empty()) {
        let entry = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            wallet::get(&conn, id).ok().flatten().filter(|e| e.removed_at.is_none())
        };
        let Some(e) = entry else { return Err((404, format!("no login {id} (k2 llm tokens list)"))) };
        let t = e.tool().ok_or((400, "unknown tool".to_string()))?;
        if live_owner(t).as_deref() == Some(id) {
            su::refresh_shared(Some(t.as_str())).map_err(|e| (500, e))?;
        } else {
            crate::llm_accounts_runtime::probe_idle_usage(&e).map_err(|e| (500, e))?;
        }
        return Ok(limits_doc(Utc::now(), false));
    }
    if let Some(t) = tool.filter(|s| !s.is_empty()) {
        let parsed = Tool::parse(t).filter(|t| t.subscription_supported()).ok_or((400, "tool must be claude, codex, or grok".to_string()))?;
        su::refresh_shared(Some(parsed.as_str())).map_err(|e| (500, e))?;
        return Ok(limits_doc(Utc::now(), false));
    }
    su::refresh_shared(None).map_err(|e| (500, e))?;
    Ok(limits_doc(Utc::now(), false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statusline_rate_limits_are_percent_with_window_lengths() {
        let rl = json!({"five_hour": {"used_percentage": 0.4, "resets_at": 1_800_000_000}, "seven_day": {"utilization": 31, "resets_at": "2027-01-15T08:00:00Z"}, "other": {"used_percentage": 99}});
        let w = claude_statusline_windows(&rl);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].id, "five_hour");
        assert!((w[0].used - 0.004).abs() < 1e-12, "0.4 means 0.4 %, never 40 % (B5)");
        assert_eq!(w[0].window_secs, Some(18_000));
        assert_eq!(w[0].resets_at, DateTime::<Utc>::from_timestamp(1_800_000_000, 0).expect("t").to_rfc3339());
        assert_eq!((w[1].id.as_str(), w[1].kind.as_str(), w[1].window_secs), ("seven_day", "weekly", Some(604_800)));
        assert!((w[1].used - 0.31).abs() < 1e-12);
        assert_eq!(w[1].resets_at, "2027-01-15T08:00:00Z");
        assert!(claude_statusline_windows(&json!({"five_hour": {"resets_at": 1}})).is_empty(), "no percent, no window");
    }

    fn statusline_body(dir: &str, pct: f64) -> Vec<u8> {
        let resets = (Utc::now() + chrono::Duration::hours(3)).timestamp();
        json!({"k2_config_dir": dir, "statusline": {"session_id": "s", "rate_limits": {"five_hour": {"used_percentage": pct, "resets_at": resets}}}})
            .to_string()
            .into_bytes()
    }

    #[test]
    fn a_statusline_reading_lands_on_its_login_and_the_default_row() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        let _airgap = k2_core::test_env::EnvVar::set("K2_AIRGAP", "0");
        let mut rx = crate::session_events::subscribe();
        // No wallet entry: the CLI's own login is the shared row.
        assert!(ingest_statusline_hook(&statusline_body("", 12.5)));
        let row = su::cached_rows().into_iter().find(|h| h.harness == "claude").expect("claude row");
        assert!((row.windows[0].used - 0.125).abs() < 1e-12);
        assert_eq!((row.source.as_str(), row.error.is_none()), (SOURCE_STATUSLINE, true));
        let mut saw = false;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, crate::session_events::SessionEvent::SubscriptionUsageChanged { ref source, .. } if source == SOURCE_STATUSLINE) {
                saw = true;
            }
        }
        assert!(saw, "a live reading announces itself");

        // A pinned slot: its own wallet row; the default row is untouched.
        let entry = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            wallet::create(&conn, Tool::Claude, "work", None).expect("create login")
        };
        let slot = store::slot_dir(Tool::Claude, &entry.id);
        std::fs::create_dir_all(&slot).expect("slot dir");
        assert_eq!(slot_account(Tool::Claude, &slot.to_string_lossy()).as_deref(), Some(entry.id.as_str()));
        assert_eq!(slot_account(Tool::Claude, "/somewhere/else"), None);
        assert!(ingest_statusline_hook(&statusline_body(&slot.to_string_lossy(), 40.0)));
        let w = su::wallet_row(&entry.id).expect("slot reading");
        assert!((w.windows[0].used - 0.4).abs() < 1e-12);
        assert_eq!((w.account_id.as_deref(), w.source.as_str()), (Some(entry.id.as_str()), SOURCE_STATUSLINE));
        let shared = su::cached_rows().into_iter().find(|h| h.harness == "claude").expect("claude row");
        assert!((shared.windows[0].used - 0.125).abs() < 1e-12, "the default row is not the slot's");

        // An older reading never replaces a newer one.
        let old = Utc::now() - chrono::Duration::hours(1);
        let wins = claude_statusline_windows(&json!({"five_hour": {"used_percentage": 99.0, "resets_at": 1_900_000_000}}));
        assert!(!apply_live(Tool::Claude, Some(entry.id.clone()), wins, "", old, SOURCE_STATUSLINE));
        assert!((su::wallet_row(&entry.id).expect("still").windows[0].used - 0.4).abs() < 1e-12);

        // No rate_limits (an API-key session): nothing happens.
        assert!(!ingest_statusline_hook(br#"{"k2_config_dir":"","statusline":{"model":{"id":"x"}}}"#));
        assert!(!ingest_statusline_hook(b"not json"));

        // The every-login document lists it, default first; Gemini has no limits API.
        let doc = limits_doc(Utc::now(), false);
        let claude = doc["tools"].as_array().expect("tools").iter().find(|t| t["tool"] == "claude").expect("claude").clone();
        let login = claude["logins"].as_array().expect("logins").iter().find(|l| l["accountId"] == json!(entry.id)).expect("login").clone();
        assert_eq!(login["source"], json!(SOURCE_STATUSLINE));
        assert_eq!(login["state"], json!("ok"));
        assert_eq!(login["windows"][0]["usedPercent"], json!(40.0));
        assert!(login["windows"][0]["windowStart"].is_string(), "pace needs the window start");
        let gemini = doc["tools"].as_array().expect("tools").iter().find(|t| t["tool"] == "gemini").expect("gemini").clone();
        assert_eq!(gemini["supported"], json!(false));
    }

    #[test]
    fn a_codex_session_log_snapshot_becomes_a_live_reading() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        let _airgap = k2_core::test_env::EnvVar::set("K2_AIRGAP", "0");
        let now_ms = Utc::now().timestamp_millis();
        let seen = k2_core::token_usage::CodexLimitsSeen {
            account_id: String::new(),
            recorded_ms: now_ms,
            snapshot: json!({"primary": {"used_percent": 22.0, "window_minutes": 300, "resets_in_seconds": 3600}, "secondary": {"used_percent": 5.0, "window_minutes": 10080, "resets_in_seconds": 86400}, "plan_type": "pro"}),
        };
        assert!(ingest_codex_rollout(&seen));
        let row = su::cached_rows().into_iter().find(|h| h.harness == "codex").expect("codex row");
        assert_eq!((row.source.as_str(), row.plan.as_str(), row.windows.len()), (SOURCE_ROLLOUT, "pro", 2));
        assert!((row.windows[0].used - 0.22).abs() < 1e-12);
        let stale = k2_core::token_usage::CodexLimitsSeen { recorded_ms: now_ms - 7 * 3_600_000, ..seen };
        assert!(!ingest_codex_rollout(&stale), "an old session log is history, not a live reading");
    }

    #[test]
    fn merged_keeps_open_windows_the_feed_does_not_carry() {
        let observed = Utc::now();
        let later = (observed + chrono::Duration::days(3)).to_rfc3339();
        let past = (observed - chrono::Duration::days(1)).to_rfc3339();
        let prev = HarnessUsage {
            harness: "claude".into(),
            plan: "Max 20x".into(),
            windows: vec![
                su::window("five_hour", "Session", "session", 0.9, later.clone(), Some(18_000)),
                su::window("model:fable:weekly", "Fable Weekly", "weekly", 0.5, later.clone(), Some(604_800)),
                su::window("model:old:weekly", "Old Weekly", "weekly", 0.5, past, Some(604_800)),
            ],
            ..Default::default()
        };
        let live = vec![su::window("five_hour", "Session", "session", 0.1, later, Some(18_000))];
        let m = merged(Tool::Claude, Some(&prev), &live, "", observed, SOURCE_STATUSLINE, Some("acc"));
        let ids: Vec<&str> = m.windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, vec!["five_hour", "model:fable:weekly"], "live wins; open model week kept; reset one dropped");
        assert!((m.windows[0].used - 0.1).abs() < 1e-12);
        assert_eq!(m.plan, "Max 20x");
        assert_eq!((m.source.as_str(), m.account_id.as_deref(), m.error.is_none()), (SOURCE_STATUSLINE, Some("acc"), true));
    }
}
