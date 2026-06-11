//! Unified `/cli/*` route dispatch — thin per-domain delegator.
//!
//! Every authenticated request whose path starts with `/cli/` lands in
//! [`dispatch`]. The dispatcher front door (`routes::dispatcher`) has
//! already parsed query params + validated the bearer token; this fn
//! just routes the path to the right domain module:
//!
//! - `agents_routes::dispatch`    — `/cli/agents/*`, `/cli/agent/*`,
//!   triage + scheduler-tick.
//! - `workspace_routes::dispatch` — `/cli/workspace/*`, `/cli/checkin`.
//! - `misc_routes::dispatch`      — the long tail (mode/settings/reviews/
//!   companion/connections/states/agent-channel/skills/feed/commit/
//!   heartbeat/terminal/sessions/onboarding/whats_new + the Phase 2
//!   Unit 4/6 GET delegators to db / git / fs / chat / themes / inbox …).
//!
//! Each domain `dispatch` returns `Option<CliResponse>` (`None` = not my
//! path); the first `Some` wins, else 404. Task #578 extracted the
//! former ~141-arm god-match here into those modules — paths, params,
//! method-gating, and auth are byte-for-byte unchanged; only the code
//! location moved.
//!
//! Routes that require a `project` / `project_path` query parameter
//! accept EITHER — see [`need_project`]. The shared param/respond
//! helpers ([`respond`], [`respond_unit`], [`need_project`],
//! [`str_param`], [`opt_param`], [`bool_param`]) are `pub` so the
//! domain modules reuse them rather than re-defining them.

use std::collections::HashMap;

// CliResponse is shared with lib-side handler modules
// (terminal_routes, etc.) via the top-level cli_response module.
pub use crate::cli_response::CliResponse;

/// Serialize a `Result<T, String>` from core into either a 200 JSON
/// body or a 400 `{"error": "..."}`. The single biggest shape for
/// `/cli/*` handlers.
///
/// Shared with the domain `*_routes.rs` modules (task #578 extraction)
/// — they call `crate::cli::respond(...)` rather than re-defining it.
pub fn respond<T: serde::Serialize>(r: Result<T, String>) -> CliResponse {
    match r {
        Ok(v) => CliResponse::ok_json(
            serde_json::to_string(&v).unwrap_or_else(|e| format!("{{\"error\":\"{}\"}}", e)),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Wrap `Ok(())` success into `{"success":true}` JSON.
pub fn respond_unit(r: Result<(), String>) -> CliResponse {
    match r {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Extract project path from `project` or `project_path` query
/// params; returns 400 response if missing/empty.
pub fn need_project(params: &HashMap<String, String>) -> Result<String, CliResponse> {
    for key in &["project_path", "project"] {
        if let Some(v) = params.get(*key) {
            if !v.is_empty() {
                return Ok(v.clone());
            }
        }
    }
    Err(CliResponse::bad_request(
        "Missing project (or project_path) parameter",
    ))
}

pub fn str_param(params: &HashMap<String, String>, key: &str) -> String {
    params.get(key).cloned().unwrap_or_default()
}

pub fn opt_param(params: &HashMap<String, String>, key: &str) -> Option<String> {
    params.get(key).cloned().filter(|s| !s.is_empty())
}

pub fn bool_param(params: &HashMap<String, String>, key: &str) -> bool {
    matches!(
        params.get(key).map(|v| v.as_str()),
        Some("1") | Some("true") | Some("on")
    )
}

// ── Main dispatch ─────────────────────────────────────────────────────

/// Route a single `/cli/*` path to its handler. Assumes the caller
/// has already validated the bearer token.
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> CliResponse {
    // ── Domain dispatch (task #578: per-domain *_routes::dispatch) ───
    if let Some(resp) = crate::agents_routes::dispatch(path, params) {
        return resp;
    }
    if let Some(resp) = crate::workspace_routes::dispatch(path, params) {
        return resp;
    }

    if let Some(resp) = crate::misc_routes::dispatch(path, params) {
        return resp;
    }
    CliResponse::not_found()
}

