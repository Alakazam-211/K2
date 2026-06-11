//! Project commands.
//!
//! Phase 2 Unit 4 — every body became a thin proxy into the daemon's
//! `/cli/projects/*` routes, except:
//! - `projects_pick_folder` / `projects_upload_icon` keep their Tauri
//!   dialog code (folder/file picker is a HOST surface).
//! - `projects_open_focus_window` keeps its `WebviewWindowBuilder`
//!   call — opening a new Tauri window is host-only.
//!
//! `sync:projects` events stay on the Tauri shim so existing renderer
//! `listen('sync:projects', ...)` subscribers keep firing.

use k2_core::db::schema::Project;
use serde_json::json;
use tauri::{Emitter, Manager};

use crate::daemon_client::DaemonClient;
use crate::editors::EditorInfo;

// ── Re-exported types ──────────────────────────────────────────────────

pub use k2_core::projects_ops::IconResult;

fn daemon() -> Result<DaemonClient, String> {
    DaemonClient::try_connect()
}

// ── Tauri Commands ─────────────────────────────────────────────────────
//
// Plan B cleanup: the DB-backed project CRUD proxies (`projects_list`,
// `projects_create`, `projects_update`, `projects_delete`,
// `projects_reorder`, `projects_add_*`, `projects_init_git_and_open`,
// `projects_enable_worktrees`, `projects_get_icon`, `projects_detect_icon`,
// `projects_clear_icon`, `projects_touch_interaction*`, and
// `workspace_set_nav_visible`) were deleted — the renderer reaches that
// data host-aware via `/cli/*` on the active daemon. Only the HOST-only
// commands below remain (native pickers, OS-integration "open in" verbs,
// editor discovery, and the focus-window opener); they still proxy
// through the LOCAL daemon via `daemon()` where they need DB lookups.

/// HOST: native folder picker dialog.
#[tauri::command]
pub async fn projects_pick_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title("Select Project Folder")
        .pick_folder(move |path| {
            let _ = tx.send(path.map(|p| p.to_string()));
        });
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_or(Ok(None), |p| Ok(Some(p)))
}

#[tauri::command]
pub fn projects_open_in_finder(path: String) -> Result<(), String> {
    daemon()?
        .cli_post_json("/cli/projects/open-in-finder", &json!({ "path": path }))
        .map(|_| ())
}

/// HOST: file picker dialog. After the user selects an image we
/// decode it into a data-URL locally (using `k2_core::projects_ops::read_icon_as_data_url`)
/// then ask the daemon to persist it.
#[tauri::command]
pub async fn projects_upload_icon(
    app: tauri::AppHandle,
    project_id: String,
) -> Result<IconResult, String> {
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title("Select Icon Image")
        .add_filter("Images", &["png", "jpg", "jpeg", "svg", "ico", "icns"])
        .pick_file(move |path| {
            let _ = tx.send(path.map(|p| p.to_string()));
        });

    let selected = rx
        .recv()
        .map_err(|e| e.to_string())?
        .map_or(Ok(None), |p| Ok::<_, String>(Some(p)))?;

    match selected {
        None => Ok(IconResult {
            found: false,
            data_url: None,
        }),
        Some(file_path) => {
            let data_url =
                k2_core::projects_ops::read_icon_as_data_url(std::path::Path::new(&file_path))
                    .ok_or("Could not read the selected image")?;
            let r: IconResult = daemon()?.cli_post_json_decode(
                "/cli/projects/set-icon",
                &json!({ "projectId": project_id, "dataUrl": data_url }),
            )?;
            let _ = app.emit("sync:projects", ());
            Ok(r)
        }
    }
}

#[tauri::command]
pub fn projects_open_in_editor(editor_id: String, path: String) -> Result<(), String> {
    daemon()?
        .cli_post_json(
            "/cli/projects/open-in-editor",
            &json!({ "editorId": editor_id, "path": path }),
        )
        .map(|_| ())
}

#[tauri::command]
pub fn projects_get_editors() -> Result<Vec<EditorInfo>, String> {
    daemon()?.cli_get_json("/cli/projects/get-editors", &[])
}

#[tauri::command]
pub fn projects_open_in_terminal(terminal_app: String, path: String) -> Result<(), String> {
    daemon()?
        .cli_post_json(
            "/cli/projects/open-in-terminal",
            &json!({ "terminalApp": terminal_app, "path": path }),
        )
        .map(|_| ())
}

#[tauri::command]
pub fn projects_get_all_editors() -> Result<Vec<EditorInfo>, String> {
    daemon()?.cli_get_json("/cli/projects/get-all-editors", &[])
}

#[tauri::command]
pub fn projects_refresh_editors() -> Result<Vec<EditorInfo>, String> {
    daemon()?.cli_post_json_decode("/cli/projects/refresh-editors", &json!({}))
}

/// HOST: opens a new Tauri WebviewWindow. Window create + management
/// is a Tauri-only API; the daemon has nothing equivalent.
#[tauri::command]
pub async fn projects_open_focus_window(
    app: tauri::AppHandle,
    project_id: String,
) -> Result<serde_json::Value, String> {
    use tauri::WebviewWindowBuilder;

    // Look up the project name via the daemon (the only DB reader now).
    let projects: Vec<Project> = daemon()?.cli_get_json("/cli/projects/list", &[])?;
    let project_name = projects
        .into_iter()
        .find(|p| p.id == project_id)
        .map(|p| p.name)
        .ok_or_else(|| format!("Project not found: {}", project_id))?;

    let label = format!("focus-{}", project_id);
    if let Some(existing) = app.get_webview_window(&label) {
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(serde_json::json!({ "focused": true }));
    }

    let webview_url = if cfg!(debug_assertions) {
        let url_str = format!("http://localhost:5173#focus={}", project_id);
        tauri::WebviewUrl::External(url::Url::parse(&url_str).map_err(|e| e.to_string())?)
    } else {
        tauri::WebviewUrl::App(format!("index.html#focus={}", project_id).into())
    };

    let _window = WebviewWindowBuilder::new(&app, &label, webview_url)
        .title(&project_name)
        .inner_size(1200.0, 800.0)
        .min_inner_size(600.0, 400.0)
        .hidden_title(true)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .build()
        .map_err(|e| e.to_string())?;

    Ok(serde_json::json!({ "opened": true }))
}
