//! `GET /cli/workspace/agent-display-name` (`k2 workspace profile`): the
//! owner token and a person's login, as on the `/cli/` catch-all, or an
//! agent passport for its own workspace only.
//!
//! The path is an agent verb (`session_token` ALLOW_EXACT), so the cell
//! socket serves it with the passport stamped (`project=` is its own
//! workspace, the request's claim kept under `CLIENT_PROJECT_KEY`). Over
//! TCP a passport takes the dispatcher's passport arm, which stamps the
//! same way (`passport_routes`, D1); the owner and logins take the `/cli/`
//! catch-all. The handler then refuses a passport whose claim names any
//! other workspace: [`other_workspace_refusal`].

use std::collections::HashMap;

use crate::caller_workspace::{CLIENT_PROJECT_KEY, PRINCIPAL_BOUND_KEY};
use crate::cli_response::CliResponse;
use crate::workspace_msg::{resolve_workspace_detailed, WorkspaceResolve};

/// A passport (stamped params) reads its own workspace's profile only:
/// `Some(403 own_workspace_only)` when its `project=` claim names any other
/// workspace (or one K2 doesn't know), or its own can't be resolved. `None`
/// for the owner and logins, and for a passport naming its own workspace
/// or none.
pub(crate) fn other_workspace_refusal(params: &HashMap<String, String>) -> Option<CliResponse> {
    if params.get(PRINCIPAL_BOUND_KEY).map(String::as_str) != Some("1") {
        return None;
    }
    let found = |token: &str| match resolve_workspace_detailed(token) {
        WorkspaceResolve::Found(path) => Some(path),
        _ => None,
    };
    let Some(own) = params.get("project").map(|s| s.trim()).filter(|s| !s.is_empty()).and_then(found) else {
        return Some(refusal());
    };
    match params.get(CLIENT_PROJECT_KEY).map(|s| s.trim()).filter(|s| !s.is_empty()) {
        None => None,
        Some(claim) if found(claim).as_deref() == Some(own.as_str()) => None,
        Some(_) => Some(refusal()),
    }
}

fn refusal() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "error": {
                "code": "own_workspace_only",
                "hint": "an agent reads its own workspace's profile only; a person can read another's",
            }
        })
        .to_string(),
    }
}
