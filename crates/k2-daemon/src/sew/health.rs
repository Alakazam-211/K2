//! Sew's readiness contract (Sew 0.3.1, confirmed by Sew 2026-10-27).
//!
//! `GET /api/v1/health` on Sew's loopback client listener answers:
//! - 200 `{"ok": true, "ready": true}` — ready;
//! - 503 with Sew's error envelope plus `ready: false`, `reason` and
//!   `since` — not ready. Sew checks, in order: `upgrading` → `stopping` →
//!   `db` → `migrations`; `stopping` carries no `since`. Any other reason is
//!   plain not-ready (no strict enum here).
//!
//! What K2 does with it (S32 Q2, Sew's design; provisional until Rosson's
//! answer, kept in these constants so it is easy to change):
//! - **Waits** only on the reasons in [`WAIT_REASONS`] (`upgrading`): the
//!   chat-pass and owner-handoff mints wait up to [`MINT_WAIT_MS`];
//!   `k2 hostchat --wait` waits as long as its own loop does.
//! - **Never waits on `stopping`** (a stop, disable or restart is under way)
//!   and refuses at once with that hint; the same for `db`, `migrations`
//!   and unknown reasons.
//! - **Never starts Sew** from a mint (a person or Manage-Sew agent runs
//!   `k2 hostchat restart`/`enable`).
//! - An older Sew (0.3.0) answers 200 `{"ok": true}` with no `ready`: that
//!   reads as ready.
//!
//! The refusal is 503 `sew_unavailable` with `{state, since, retryAfterMs,
//! hint}` (the details a BFF needs to back off).

use serde_json::{json, Value};

use super::supervisor::{no, Env, Fail};

/// Reasons K2 waits through (Sew is coming back by itself).
pub const WAIT_REASONS: &[&str] = &["upgrading"];
/// How long a mint waits on a [`WAIT_REASONS`] state.
pub const MINT_WAIT_MS: u64 = 10_000;
/// Poll interval while waiting.
pub const POLL_MS: u64 = 500;
/// Gate switches per path (S32 Q2 is still open with Rosson): the owner
/// handoff mint and the chat-pass mint both probe Sew first.
pub const GATE_HANDOFF: bool = true;
pub const GATE_PASS: bool = true;

/// One reading of `/api/v1/health`.
#[derive(Debug, Clone, PartialEq)]
pub struct Health {
    pub ready: bool,
    /// Sew's reason when not ready (`upgrading`, `stopping`, `db`,
    /// `migrations`, or anything newer), `down` when nothing answered.
    pub reason: Option<String>,
    /// Since when (Sew's value, passed through: absent on `stopping`).
    pub since: Option<Value>,
    /// The HTTP status, `None` when nothing answered.
    pub status: Option<u16>,
    /// Transport error or Sew's error hint, for logs and `detail`.
    pub detail: Option<String>,
}

impl Health {
    pub fn waits(&self) -> bool {
        !self.ready && self.reason.as_deref().is_some_and(|r| WAIT_REASONS.contains(&r))
    }

    pub fn json(&self) -> Value {
        json!({
            "ready": self.ready,
            "reason": self.reason,
            "since": self.since,
            "status": self.status,
            "detail": self.detail,
        })
    }
}

/// Read one answer (`Ok((status, body))` or a transport error).
pub fn parse(answer: Result<(u16, Value), String>) -> Health {
    match answer {
        Err(e) => Health { ready: false, reason: Some("down".into()), since: None, status: None, detail: Some(e) },
        Ok((code, body)) => {
            let ready = match body.get("ready").and_then(Value::as_bool) {
                Some(r) => r,
                // Sew 0.3.0: no `ready` field; `ok: true` on a 200 is ready.
                None => (200..300).contains(&code) && body.get("ok").and_then(Value::as_bool) == Some(true),
            };
            if ready && (200..300).contains(&code) {
                return Health { ready: true, reason: None, since: None, status: Some(code), detail: None };
            }
            let reason = body
                .get("reason")
                .or_else(|| body.pointer("/error/details/reason"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| Some(if code == 503 { "not_ready".to_string() } else { format!("http_{code}") }));
            let since = body.get("since").or_else(|| body.pointer("/error/details/since")).filter(|v| !v.is_null()).cloned();
            let detail = body
                .pointer("/error/hint")
                .or_else(|| body.pointer("/error/message"))
                .and_then(Value::as_str)
                .map(str::to_string);
            Health { ready: false, reason, since, status: Some(code), detail }
        }
    }
}

/// One probe through the box's ops.
pub fn read(env: &Env) -> Health {
    parse(env.ops.api_health())
}

/// Probe; while the reason is one K2 waits through, poll until ready or
/// `max_ms` passes. Returns the last reading.
pub fn wait_ready(env: &Env, max_ms: u64) -> Health {
    let mut h = read(env);
    let mut waited = 0u64;
    while h.waits() && waited < max_ms {
        env.ops.sleep_ms(POLL_MS);
        waited += POLL_MS;
        h = read(env);
    }
    h
}

/// The person-facing words for a not-ready reason.
pub fn hint(reason: &str) -> String {
    match reason {
        "upgrading" => "Sew is upgrading; it comes back by itself — try again in a moment".into(),
        "stopping" => "Sew is stopping (a stop, disable or restart is under way); K2 won't wait — try again once \
                       k2 hostchat status says running, or run k2 hostchat restart"
            .into(),
        "db" => "Sew can't reach its database (sew-db); k2 hostchat restart brings the units back in order".into(),
        "migrations" => "Sew is applying database migrations; try again shortly (k2 hostchat status)".into(),
        "down" => "Sew is not answering on this box (k2 hostchat status; k2 hostchat restart)".into(),
        other => format!("Sew is not ready ({other}); k2 hostchat status shows why"),
    }
}

/// How long a client should wait before trying again.
pub fn retry_after_ms(reason: &str) -> u64 {
    match reason {
        "upgrading" | "migrations" => 5_000,
        "stopping" => 15_000,
        _ => 30_000,
    }
}

/// The refusal body's details.
pub fn unavailable_details(h: &Health) -> Value {
    let reason = h.reason.clone().unwrap_or_else(|| "not_ready".into());
    json!({
        "state": reason,
        "since": h.since,
        "retryAfterMs": retry_after_ms(&reason),
        "hint": hint(&reason),
    })
}

/// 503 `sew_unavailable` (supervisor routes).
pub fn unavailable(h: &Health) -> Fail {
    let d = unavailable_details(h);
    let mut f = no("sew_unavailable", 503, d["hint"].as_str().unwrap_or("Sew is not ready").to_string());
    f.extra = Some(d);
    f
}

/// The mint gate: no Sew install (or a disabled/removed one) is not this
/// gate's business (the mint's own install-id check answers); otherwise
/// Sew must be ready, waiting up to [`MINT_WAIT_MS`] on an upgrade.
pub fn mint_gate(env: &Env) -> Result<(), Value> {
    let Some(row) = super::store::load() else { return Ok(()) };
    if matches!(row.status.as_str(), "removed" | "disabled" | "installing") {
        return Ok(());
    }
    let h = wait_ready(env, MINT_WAIT_MS);
    if h.ready {
        Ok(())
    } else {
        Err(unavailable_details(&h))
    }
}

// ── doctor mapping (Sew 0.3.1) ──────────────────────────────────────────

/// K2's verdict over `sew doctor`'s checks: `failed` when any check fails;
/// `degraded` on a warn (Sew 2026-10-27: `media.reachability` warn is
/// degraded, never failed); `unknown`
/// checks (non-root creds, sysctl) never fail or degrade anything — they
/// are listed. Also lifts `upgrade.state` (a stale upgrade file: phase,
/// age, writer gone) so status can show it.
pub fn doctor_verdict(report: &Value) -> Value {
    let checks = report.get("checks").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut failed = Vec::new();
    let mut degraded = Vec::new();
    let mut unknown = Vec::new();
    for c in &checks {
        let id = c.get("id").and_then(Value::as_str).unwrap_or("?").to_string();
        match c.get("status").and_then(Value::as_str).unwrap_or("") {
            "fail" => failed.push(id),
            "warn" => degraded.push(id),
            "unknown" => unknown.push(id),
            _ => {}
        }
    }
    let verdict = if !failed.is_empty() {
        "failed"
    } else if !degraded.is_empty() {
        "degraded"
    } else {
        "ok"
    };
    let upgrade = checks.iter().find(|c| c.get("id").and_then(Value::as_str) == Some("upgrade.state")).cloned();
    json!({"verdict": verdict, "failed": failed, "degraded": degraded, "unknown": unknown, "upgradeState": upgrade})
}

/// `sew media status --json`'s `turn {tier, ice_urls, source}` (Sew
/// 0.3.1), mapped for `/cli/sew/status` `voice.turnEngine`. `None` when the
/// engine did not report it (older Sew).
pub fn media_turn(engine: Option<&Value>) -> Option<Value> {
    let t = engine?.get("turn")?;
    Some(json!({
        "tier": t.get("tier"),
        "iceUrls": t.get("ice_urls").cloned().unwrap_or(Value::Null),
        "source": t.get("source"),
    }))
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
