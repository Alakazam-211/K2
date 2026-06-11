//! Daemon-side `/cli/themes/*` route handlers (Phase 2 Unit 6).

use std::collections::HashMap;

use serde::Deserialize;

use crate::cli_response::CliResponse;
use k2_core::themes;

pub fn handle_list(_params: &HashMap<String, String>) -> CliResponse {
    match themes::list_custom() {
        Ok(list) => CliResponse::ok_json(
            serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_get_dir(_params: &HashMap<String, String>) -> CliResponse {
    match themes::get_dir() {
        Ok(p) => CliResponse::ok_json(serde_json::json!({ "path": p }).to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_ensure_dir(_params: &HashMap<String, String>) -> CliResponse {
    match themes::ensure_dir() {
        Ok(p) => CliResponse::ok_json(serde_json::json!({ "path": p }).to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct CreateTemplateBody {
    /// JSON of an existing theme to seed the new file with. Empty
    /// string -> seed with the built-in default.
    #[serde(default)]
    base_theme_json: String,
}

pub fn handle_create_template(body: &[u8]) -> CliResponse {
    let parsed: CreateTemplateBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match themes::create_template(&parsed.base_theme_json) {
        Ok(p) => CliResponse::ok_json(serde_json::json!({ "path": p }).to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct DeleteBody {
    path: String,
}

pub fn handle_delete(body: &[u8]) -> CliResponse {
    let parsed: DeleteBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match themes::delete(&parsed.path) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}
