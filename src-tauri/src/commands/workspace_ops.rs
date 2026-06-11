use tauri::{AppHandle, Emitter};

#[tauri::command]
pub fn workspace_split_pane(
    app: AppHandle,
    tab_id: String,
    pane_id: String,
    direction: String,
) -> Result<(), String> {
    if direction != "horizontal" && direction != "vertical" {
        return Err(format!("Invalid direction '{}': must be 'horizontal' or 'vertical'", direction));
    }

    app.emit(
        "workspace:split-pane",
        serde_json::json!({
            "tabId": tab_id,
            "paneId": pane_id,
            "direction": direction,
        }),
    )
    .map_err(|e| format!("Failed to emit workspace:split-pane: {}", e))
}

#[tauri::command]
pub fn workspace_close_pane(
    app: AppHandle,
    tab_id: String,
    pane_id: String,
) -> Result<(), String> {
    app.emit(
        "workspace:close-pane",
        serde_json::json!({
            "tabId": tab_id,
            "paneId": pane_id,
        }),
    )
    .map_err(|e| format!("Failed to emit workspace:close-pane: {}", e))
}

#[tauri::command]
pub fn workspace_open_document(
    app: AppHandle,
    tab_id: String,
    pane_id: String,
    file_path: String,
) -> Result<(), String> {
    if file_path.is_empty() {
        return Err("file_path cannot be empty".to_string());
    }

    app.emit(
        "workspace:open-document",
        serde_json::json!({
            "tabId": tab_id,
            "paneId": pane_id,
            "filePath": file_path,
        }),
    )
    .map_err(|e| format!("Failed to emit workspace:open-document: {}", e))
}

#[tauri::command]
pub fn workspace_open_terminal(
    app: AppHandle,
    tab_id: String,
    pane_id: Option<String>,
    cwd: String,
    command: Option<String>,
) -> Result<(), String> {
    if cwd.is_empty() {
        return Err("cwd cannot be empty".to_string());
    }

    app.emit(
        "workspace:open-terminal",
        serde_json::json!({
            "tabId": tab_id,
            "paneId": pane_id,
            "cwd": cwd,
            "command": command,
        }),
    )
    .map_err(|e| format!("Failed to emit workspace:open-terminal: {}", e))
}

#[tauri::command]
pub fn workspace_new_tab(
    app: AppHandle,
    cwd: String,
) -> Result<(), String> {
    if cwd.is_empty() {
        return Err("cwd cannot be empty".to_string());
    }

    app.emit(
        "workspace:new-tab",
        serde_json::json!({
            "cwd": cwd,
        }),
    )
    .map_err(|e| format!("Failed to emit workspace:new-tab: {}", e))
}

#[tauri::command]
pub fn workspace_close_tab(
    app: AppHandle,
    tab_id: String,
) -> Result<(), String> {
    app.emit(
        "workspace:close-tab",
        serde_json::json!({
            "tabId": tab_id,
        }),
    )
    .map_err(|e| format!("Failed to emit workspace:close-tab: {}", e))
}

#[tauri::command]
pub fn workspace_arrange(
    app: AppHandle,
    layout: serde_json::Value,
) -> Result<(), String> {
    if !layout.is_object() {
        return Err("layout must be a JSON object".to_string());
    }

    app.emit("workspace:arrange", layout)
        .map_err(|e| format!("Failed to emit workspace:arrange: {}", e))
}
