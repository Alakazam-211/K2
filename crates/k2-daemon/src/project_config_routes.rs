//! Daemon-side `/cli/project-config/*` route handlers (Phase 2 Unit 6).

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use k2_core::project_config as pc;

pub fn handle_get(params: &HashMap<String, String>) -> CliResponse {
    let path = match params
        .get("project")
        .or_else(|| params.get("path"))
        .filter(|s| !s.is_empty())
    {
        Some(p) => p.clone(),
        None => return CliResponse::bad_request("Missing 'project' parameter"),
    };
    let config = pc::get_project_config(&path);
    CliResponse::ok_json(
        serde_json::to_string(&config).unwrap_or_else(|_| "{}".to_string()),
    )
}

pub fn handle_has_run_command(params: &HashMap<String, String>) -> CliResponse {
    let path = match params
        .get("project")
        .or_else(|| params.get("path"))
        .filter(|s| !s.is_empty())
    {
        Some(p) => p.clone(),
        None => return CliResponse::bad_request("Missing 'project' parameter"),
    };
    let has = pc::has_run_command(&path);
    CliResponse::ok_json(serde_json::json!({ "hasRunCommand": has }).to_string())
}

/// Returns the configured run command (if any) for the project.
/// Errors with 400 when no run command is set — matches the
/// pre-Phase-2 Tauri command's error shape so the renderer can detect
/// "no command configured" by inspecting the error string.
pub fn handle_run_command(params: &HashMap<String, String>) -> CliResponse {
    let path = match params
        .get("project")
        .or_else(|| params.get("path"))
        .filter(|s| !s.is_empty())
    {
        Some(p) => p.clone(),
        None => return CliResponse::bad_request("Missing 'project' parameter"),
    };
    let config = pc::get_project_config(&path);
    match config.run_command {
        Some(cmd) if !cmd.is_empty() => CliResponse::ok_json(
            serde_json::json!({ "command": cmd }).to_string(),
        ),
        _ => CliResponse::bad_request("No run command configured for this project"),
    }
}
