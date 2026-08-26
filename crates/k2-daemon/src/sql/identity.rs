//! SQL-side wrapper over [`crate::caller_workspace`] — copy of mail identity.

use std::collections::HashMap;

use crate::caller_workspace::{
    resolve_caller_workspace_claim, resolve_caller_workspace_from_params, ResolveCallerError,
};
use crate::cli_response::CliResponse;

fn error_response(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint },
        })
        .to_string(),
    }
}

fn map_err(err: ResolveCallerError) -> CliResponse {
    match err {
        ResolveCallerError::MissingClaim => {
            error_response("400 Bad Request", "usage", &err.hint())
        }
        ResolveCallerError::NotFound(ref q) => {
            crate::workspace_routes::workspace_not_found_response(q)
        }
        ResolveCallerError::Unregistered(ref path) => error_response(
            "404 Not Found",
            "not_found",
            &format!("workspace not registered: {path}"),
        ),
        ResolveCallerError::UnresolvablePrincipal => {
            error_response("403 Forbidden", "forbidden", &err.hint())
        }
    }
}

pub fn resolve_caller(project_claim: &str) -> Result<(String, String), CliResponse> {
    let claim = project_claim.trim();
    let claim = if claim.is_empty() { None } else { Some(claim) };
    resolve_caller_workspace_claim(claim)
        .map(|ws| (ws.path, ws.workspace_uuid))
        .map_err(map_err)
}

pub fn resolve_caller_params(
    params: &HashMap<String, String>,
) -> Result<(String, String), CliResponse> {
    resolve_caller_workspace_from_params(params)
        .map(|ws| (ws.path, ws.workspace_uuid))
        .map_err(map_err)
}
