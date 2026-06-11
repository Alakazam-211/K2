//! Daemon-side `/cli/chat/*` route handlers (Phase 2 Unit 6).
//!
//! Wraps `k2_core::chat_history::*` for the renderer's chat-history
//! sidebar. Response shapes mirror the pre-Phase-2 Tauri commands
//! byte-for-byte so the renderer can swap endpoints without changing
//! any consumer code.

use std::collections::HashMap;

use serde::Deserialize;

use crate::cli_response::CliResponse;
use k2_core::chat_history as ch;

// ── GET handlers ──────────────────────────────────────────────────────

pub fn handle_list(params: &HashMap<String, String>) -> CliResponse {
    let project_filter = params
        .get("project_path")
        .or_else(|| params.get("project"))
        .map(String::as_str);
    match ch::list_all_sessions(project_filter) {
        Ok(sessions) => CliResponse::ok_json(
            serde_json::to_string(&sessions).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_storage_paths(params: &HashMap<String, String>) -> CliResponse {
    let project_path = match params
        .get("project_path")
        .or_else(|| params.get("project"))
        .filter(|s| !s.is_empty())
    {
        Some(p) => p.clone(),
        None => return CliResponse::bad_request("Missing project_path parameter"),
    };
    match ch::get_storage_paths(&project_path) {
        Ok(paths) => CliResponse::ok_json(
            serde_json::to_string(&paths).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_custom_names(_params: &HashMap<String, String>) -> CliResponse {
    match ch::get_custom_names() {
        Ok(map) => CliResponse::ok_json(
            serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_pinned(_params: &HashMap<String, String>) -> CliResponse {
    match ch::get_pinned() {
        Ok(list) => CliResponse::ok_json(
            serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_detect_active(params: &HashMap<String, String>) -> CliResponse {
    let provider = params.get("provider").cloned().unwrap_or_default();
    let project_path = params
        .get("project_path")
        .or_else(|| params.get("project"))
        .cloned()
        .unwrap_or_default();
    if provider.is_empty() || project_path.is_empty() {
        return CliResponse::bad_request("Missing 'provider' or 'project_path' parameter");
    }
    match ch::detect_active_session(&provider, &project_path) {
        Ok(opt) => CliResponse::ok_json(
            serde_json::json!({ "sessionId": opt }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_discover_ide(params: &HashMap<String, String>) -> CliResponse {
    let project_path = match params
        .get("project_path")
        .or_else(|| params.get("project"))
        .filter(|s| !s.is_empty())
    {
        Some(p) => p.clone(),
        None => return CliResponse::bad_request("Missing project_path parameter"),
    };
    match ch::discover_ide_sessions(&project_path) {
        Ok(sessions) => CliResponse::ok_json(
            serde_json::to_string(&sessions).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_session_exists(params: &HashMap<String, String>) -> CliResponse {
    let project_path = params
        .get("project_path")
        .or_else(|| params.get("project"))
        .cloned()
        .unwrap_or_default();
    let session_id = params.get("session_id").cloned().unwrap_or_default();
    if project_path.is_empty() || session_id.is_empty() {
        return CliResponse::bad_request(
            "Missing 'project_path' or 'session_id' parameter",
        );
    }
    let exists = ch::claude_session_file_exists(&session_id, &project_path);
    CliResponse::ok_json(serde_json::json!({ "exists": exists }).to_string())
}

// ── POST handlers ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RenameBody {
    provider: String,
    session_id: String,
    custom_name: String,
}

pub fn handle_rename(body: &[u8]) -> CliResponse {
    let parsed: RenameBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match ch::rename_session(&parsed.provider, &parsed.session_id, &parsed.custom_name) {
        Ok(()) => {
            // Mirror the Tauri-side `sync:chat-history` event so the
            // renderer's tab list refreshes after a rename. The
            // daemon-side broadcast lands on every `/events` WS
            // subscriber including `daemon_events.rs`, which re-emits
            // onto Tauri's event bus.
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::SyncChatHistory,
                serde_json::Value::Null,
            );
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct TogglePinBody {
    provider: String,
    session_id: String,
    pinned: bool,
}

pub fn handle_toggle_pin(body: &[u8]) -> CliResponse {
    let parsed: TogglePinBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match ch::toggle_pin(&parsed.provider, &parsed.session_id, parsed.pinned) {
        Ok(()) => {
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::SyncChatHistory,
                serde_json::Value::Null,
            );
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct MigrateIdeBody {
    project_path: String,
    composer_ids: Vec<String>,
}

pub fn handle_migrate_ide(body: &[u8]) -> CliResponse {
    let parsed: MigrateIdeBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match ch::migrate_ide_sessions(&parsed.project_path, &parsed.composer_ids) {
        Ok(count) => CliResponse::ok_json(
            serde_json::json!({ "migrated": count }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}
