//! Daemon-side `/cli/review-checklist/*` route handlers (Phase 2 Unit 6).

use std::collections::HashMap;

use serde::Deserialize;

use crate::cli_response::CliResponse;
use k2_core::review_checklist as rc;

pub fn handle_read(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match params
        .get("workspace_path")
        .or_else(|| params.get("workspace"))
        .filter(|s| !s.is_empty())
    {
        Some(p) => p.clone(),
        None => return CliResponse::bad_request("Missing 'workspace_path' parameter"),
    };
    match rc::read(&workspace) {
        Ok(items) => CliResponse::ok_json(
            serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct WriteBody {
    workspace_path: String,
    items: Vec<rc::ChecklistItem>,
    agent_name: String,
    branch: String,
}

pub fn handle_write(body: &[u8]) -> CliResponse {
    let parsed: WriteBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match rc::write(
        &parsed.workspace_path,
        &parsed.items,
        &parsed.agent_name,
        &parsed.branch,
    ) {
        Ok(()) => {
            // #677.2 — checklist mutation is a review change. ONE event
            // (ReviewChanged) covers review detail + checklist.
            crate::misc_routes::emit_review_changed(
                &parsed.workspace_path,
                Some(&parsed.agent_name),
            );
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct ToggleBody {
    workspace_path: String,
    index: usize,
    agent_name: String,
    branch: String,
}

pub fn handle_toggle(body: &[u8]) -> CliResponse {
    let parsed: ToggleBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match rc::toggle(
        &parsed.workspace_path,
        parsed.index,
        &parsed.agent_name,
        &parsed.branch,
    ) {
        Ok(items) => {
            // #677.2 — checklist toggle is a review change.
            crate::misc_routes::emit_review_changed(
                &parsed.workspace_path,
                Some(&parsed.agent_name),
            );
            CliResponse::ok_json(
                serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string()),
            )
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_init(body: &[u8]) -> CliResponse {
    let parsed: WriteBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match rc::init(
        &parsed.workspace_path,
        &parsed.items,
        &parsed.agent_name,
        &parsed.branch,
    ) {
        Ok(()) => {
            // #677.2 — checklist init is a review change.
            crate::misc_routes::emit_review_changed(
                &parsed.workspace_path,
                Some(&parsed.agent_name),
            );
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}
