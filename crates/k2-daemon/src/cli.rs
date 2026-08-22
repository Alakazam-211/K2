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
//! - `ops_routes::dispatch`       — `/cli/ops/*` read-only Observability /
//!   Agent-Ops aggregate API (activity history + live overview snapshot).
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

/// Wiki seed on workspace add/open/create. Default ON; opt out with
/// `no_wiki=1` or `seed_wiki=0|false|off|no`.
pub fn seed_wiki_wanted(params: &HashMap<String, String>) -> bool {
    if bool_param(params, "no_wiki") {
        return false;
    }
    match opt_param(params, "seed_wiki").as_deref() {
        Some("0") | Some("false") | Some("off") | Some("no") => false,
        _ => true,
    }
}

/// Cwd `AGENTS.md` generate on workspace add/open/create. Default ON;
/// opt out with `no_agents_md=1` or `seed_agents_md=0|false|off|no`.
pub fn agents_md_wanted(params: &HashMap<String, String>) -> bool {
    if bool_param(params, "no_agents_md") {
        return false;
    }
    match opt_param(params, "seed_agents_md").as_deref() {
        Some("0") | Some("false") | Some("off") | Some("no") => false,
        _ => true,
    }
}

/// Leftover harness fan-out on workspace add/open/create. Default OFF;
/// opt in with `fanout=1|true|on`.
pub fn fanout_wanted(params: &HashMap<String, String>) -> bool {
    bool_param(params, "fanout")
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
    // Context management stack — `/cli/context/*` reads (layers/show/catalog) +
    // 405 guards for POST-only mutations reached via the GET chain.
    if let Some(resp) = crate::context_routes::dispatch(path, params) {
        return resp;
    }
    // Feedback F1 — `/cli/feedback/*` reads (list/show) + the 405
    // guards for its POST-only mutations reached via the GET chain.
    if let Some(resp) = crate::feedback_routes::dispatch(path, params) {
        return resp;
    }
    // Projects V1 P2 — `/cli/project-group/*` reads (list/show/
    // messages) + the 405 guards for its POST-only mutations reached
    // via the GET chain. NOT the legacy `/cli/projects/*` workspace-
    // registry surface (PRD §2b naming collision).
    if let Some(resp) = crate::project_group_routes::dispatch(path, params) {
        return resp;
    }
    // Companion C4 — `/cli/push/*` 405 guards for its POST-only
    // mutations reached via the GET chain (no push GET reads in V1).
    if let Some(resp) = crate::push_routes::dispatch(path, params) {
        return resp;
    }
    // K2 Mail — `/cli/mail/*` reads (status is REAL — the capability
    // seam; the rest 501 until their slice lands) + the 405 guards for
    // its POST-only mutations reached via the GET chain.
    if let Some(resp) = crate::mail_routes::dispatch(path, params) {
        return resp;
    }
    // DNS K1 — `/cli/dns/*` reads (access/zones/records) + 405 guards
    // for POST-only mutations. Principal-bound; toggle-gated in handlers.
    if let Some(resp) = crate::dns_routes::dispatch(path, params) {
        return resp;
    }
    // Published services — GET list/logs + 405 twins for POST-only verbs.
    if let Some(resp) = crate::publish_routes::dispatch(path, params) {
        return resp;
    }
    // 0.40.34 — `/cli/browser/open-url` 405 guard for the POST-only
    // mutation reached via the GET chain (no browser GET reads).
    if let Some(resp) = crate::browser_routes::dispatch(path, params) {
        return resp;
    }

    if let Some(resp) = crate::misc_routes::dispatch(path, params) {
        return resp;
    }
    // Observability / Agent-Ops read-only GET aggregate API (`/cli/ops/*`).
    if let Some(resp) = crate::ops_routes::dispatch(path, params) {
        return resp;
    }
    CliResponse::not_found()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn seed_wiki_wanted_defaults_on() {
        assert!(seed_wiki_wanted(&HashMap::new()));
        assert!(seed_wiki_wanted(&params(&[("path", "/tmp/x")])));
        assert!(seed_wiki_wanted(&params(&[("seed_wiki", "1")])));
    }

    #[test]
    fn seed_wiki_wanted_opts_out() {
        assert!(!seed_wiki_wanted(&params(&[("no_wiki", "1")])));
        assert!(!seed_wiki_wanted(&params(&[("no_wiki", "true")])));
        assert!(!seed_wiki_wanted(&params(&[("seed_wiki", "0")])));
        assert!(!seed_wiki_wanted(&params(&[("seed_wiki", "false")])));
        assert!(
            !seed_wiki_wanted(&params(&[("no_wiki", "1"), ("seed_wiki", "1")])),
            "no_wiki wins over seed_wiki=1"
        );
    }

    #[test]
    fn agents_md_wanted_defaults_on() {
        assert!(agents_md_wanted(&HashMap::new()));
        assert!(agents_md_wanted(&params(&[("path", "/tmp/x")])));
        assert!(agents_md_wanted(&params(&[("seed_agents_md", "1")])));
    }

    #[test]
    fn agents_md_wanted_opts_out() {
        assert!(!agents_md_wanted(&params(&[("no_agents_md", "1")])));
        assert!(!agents_md_wanted(&params(&[("no_agents_md", "true")])));
        assert!(!agents_md_wanted(&params(&[("seed_agents_md", "0")])));
        assert!(!agents_md_wanted(&params(&[("seed_agents_md", "false")])));
        assert!(
            !agents_md_wanted(&params(&[("no_agents_md", "1"), ("seed_agents_md", "1")])),
            "no_agents_md wins over seed_agents_md=1"
        );
    }

    #[test]
    fn fanout_wanted_defaults_off() {
        assert!(!fanout_wanted(&HashMap::new()));
        assert!(!fanout_wanted(&params(&[("path", "/tmp/x")])));
        assert!(fanout_wanted(&params(&[("fanout", "1")])));
        assert!(fanout_wanted(&params(&[("fanout", "true")])));
        assert!(fanout_wanted(&params(&[("fanout", "on")])));
        assert!(!fanout_wanted(&params(&[("fanout", "0")])));
    }
}
