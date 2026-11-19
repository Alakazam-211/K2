//! `GET /cli/workspace/agent-display-name` (`k2 workspace profile`): the
//! owner token and a person's login, as on the `/cli/` catch-all, or an
//! agent passport for its own workspace only.
//!
//! The path is an agent verb (`session_token` ALLOW_EXACT), so the cell
//! socket serves it with the passport stamped (`project=` is its own
//! workspace, the request's claim kept under `CLIENT_PROJECT_KEY`). The
//! TCP arm here stamps the same way. The handler then refuses a passport
//! whose claim names any other workspace: [`other_workspace_refusal`].

use std::collections::HashMap;

use tokio::net::TcpStream;

use crate::caller_workspace::{CLIENT_PROJECT_KEY, PRINCIPAL_BOUND_KEY};
use crate::cli_response::CliResponse;
use crate::routes::http;
use crate::workspace_msg::{resolve_workspace_detailed, WorkspaceResolve};

pub(crate) const PATH: &str = "/cli/workspace/agent-display-name";

/// The TCP dispatcher's arm for [`PATH`]. A credential that is neither the
/// owner's, a live login, nor a live passport gets the catch-all's plain
/// 403, as before.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve_tcp(
    stream: &mut TcpStream,
    buf: &mut [u8],
    is_post: bool,
    path: &str,
    query: &str,
    bearer: Option<&str>,
    owner_token: &str,
) {
    let mut params = http::read_request_params(stream, buf, is_post, path, query).await;
    // Identity keys are the server's, never the request's.
    crate::caller_workspace::strip_request_stamps(&mut params);
    let principal = if http::token_ok(query, owner_token) {
        None
    } else {
        let presented = bearer.filter(|s| !s.is_empty()).or_else(|| http::extract_token(query)).unwrap_or("");
        match crate::session_token::require_hook(presented, PATH) {
            Some(v) => Some(v.principal),
            None => {
                let r = CliResponse::forbidden();
                http::send_response(stream, r.status, r.content_type, &r.body).await;
                return;
            }
        }
    };
    if let Some(ref p) = principal {
        crate::caller_workspace::keep_client_project(&mut params);
        crate::caller_workspace::stamp_principal(&mut params, p);
    }
    let r = tokio::task::spawn_blocking(move || {
        crate::caller_workspace::with_request_principal(principal, || crate::cli::dispatch(PATH, &params))
    })
    .await
    .unwrap_or_else(|e| CliResponse::internal_error(format!("worker join: {e}")));
    http::send_response(stream, r.status, r.content_type, &r.body).await;
}

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
