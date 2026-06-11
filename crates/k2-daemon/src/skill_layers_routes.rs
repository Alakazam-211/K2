//! Daemon-side `/cli/skill-layers/*` route handlers (Phase 2 Unit 6).

use std::collections::HashMap;

use serde::Deserialize;

use crate::cli_response::CliResponse;
use k2_core::skill_layers as sl;

pub fn handle_list(params: &HashMap<String, String>) -> CliResponse {
    let tier = match params.get("tier").filter(|s| !s.is_empty()) {
        Some(t) => t.clone(),
        None => return CliResponse::bad_request("Missing 'tier' parameter"),
    };
    match sl::list(&tier) {
        Ok(layers) => CliResponse::ok_json(
            serde_json::to_string(&layers).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_get_content(params: &HashMap<String, String>) -> CliResponse {
    let tier = params.get("tier").cloned().unwrap_or_default();
    let filename = params.get("filename").cloned().unwrap_or_default();
    if tier.is_empty() || filename.is_empty() {
        return CliResponse::bad_request("Missing 'tier' or 'filename' parameter");
    }
    match sl::get_content(&tier, &filename) {
        Ok(content) => CliResponse::ok_json(
            serde_json::json!({ "content": content }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct CreateBody {
    tier: String,
    name: String,
}

pub fn handle_create(body: &[u8]) -> CliResponse {
    let parsed: CreateBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match sl::create(&parsed.tier, &parsed.name) {
        Ok(layer) => CliResponse::ok_json(
            serde_json::to_string(&layer).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct DeleteBody {
    tier: String,
    filename: String,
}

pub fn handle_delete(body: &[u8]) -> CliResponse {
    let parsed: DeleteBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match sl::delete(&parsed.tier, &parsed.filename) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}
