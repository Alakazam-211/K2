//! Project commands.
//!
//! Phase 2 Unit 4 — every body became a thin proxy into the daemon's
//! `/cli/projects/*` routes, except:
//! - `projects_pick_folder` / `pick_local_files` keep their Tauri
//!   dialog code (folder/file picker is a HOST surface).
//!   (`projects_upload_icon` had no renderer caller and decoded the
//!   image with k2-core in process; removed in 0.46.4.)
//! - `projects_open_focus_window` keeps its `WebviewWindowBuilder`
//!   call — opening a new Tauri window is host-only.
//!
//! `sync:projects` events stay on the Tauri shim so existing renderer
//! `listen('sync:projects', ...)` subscribers keep firing.

use serde_json::json;
use tauri::Manager;

use crate::daemon_client::DaemonClient;
use crate::editors::EditorInfo;

/// The two `/cli/projects/list` row fields the focus window needs (the
/// daemon's `db::schema::Project`, camelCase on the wire; other fields are
/// ignored).
#[derive(serde::Deserialize)]
struct ProjectRow {
    id: String,
    name: String,
}

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

/// HOST: native multi-file picker. WKWebView `<input type=file>` has no
/// `File.path` and ignores `.click()` on `display:none` — compose attach
/// uses this so the draft gets real filesystem paths.
#[tauri::command]
pub async fn pick_local_files(app: tauri::AppHandle) -> Result<Option<Vec<String>>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title("Attach a file or image")
        .pick_files(move |paths| {
            let mapped = paths.map(|ps| ps.into_iter().map(|p| p.to_string()).collect::<Vec<_>>());
            let _ = tx.send(mapped);
        });
    rx.recv().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn projects_open_in_finder(path: String) -> Result<(), String> {
    daemon()?
        .cli_post_json("/cli/projects/open-in-finder", &json!({ "path": path }))
        .map(|_| ())
}

/// HOST: show a folder on this machine in the OS file manager.
///
/// Replaces the webview's `opener:allow-open-path` permission
/// (prd-security-sweep-0464-v1 SS16): that one let the renderer open
/// any path with its default app, which for a file means running it.
/// This command opens only an existing directory given by an absolute
/// path, and on macOS refuses an `.app` bundle (a directory that the
/// OS would launch rather than show).
#[tauri::command]
pub fn open_folder(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let dir = folder_to_open(&path)?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("open_folder: {e}"))
}

fn folder_to_open(path: &str) -> Result<std::path::PathBuf, String> {
    let p = std::path::Path::new(path);
    if path.is_empty() || !p.is_absolute() {
        return Err(format!("open_folder: needs an absolute folder path, got {path:?}"));
    }
    let meta = std::fs::metadata(p).map_err(|e| format!("open_folder: {path}: {e}"))?;
    if !meta.is_dir() {
        return Err(format!("open_folder: not a folder: {path}"));
    }
    if cfg!(target_os = "macos")
        && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("app"))
    {
        return Err(format!("open_folder: refusing an app bundle: {path}"));
    }
    Ok(p.to_path_buf())
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
///
/// **Sync** (same as `window_new` / `open_new_window`). This handler
/// uses `reqwest::blocking` (`daemon().cli_get_json`) and
/// `WebviewWindowBuilder::build()`. Running those on the async
/// command runtime panics: "Cannot drop a runtime in a context where
/// blocking is not allowed" — menu/hotkey then look like no-ops
/// (renderer only `console.warn`s the invoke error).
#[tauri::command]
pub fn projects_open_focus_window(
    app: tauri::AppHandle,
    project_id: String,
) -> Result<serde_json::Value, String> {
    // Look up the project name via the daemon (the only DB reader now).
    let projects: Vec<ProjectRow> = daemon()?.cli_get_json("/cli/projects/list", &[])?;
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

    let fragment = format!("focus={project_id}");
    let webview_url = crate::k2_app_window::k2_app_webview_url(&app, Some(&fragment));
    let builder = crate::k2_app_window::k2_app_window_builder(&app, &label, webview_url)
        .title(&project_name)
        .inner_size(1200.0, 800.0)
        .min_inner_size(600.0, 400.0);
    // hidden_title / TitleBarStyle::Overlay are macOS-only builder methods.
    #[cfg(target_os = "macos")]
    let builder = builder
        .hidden_title(true)
        .title_bar_style(tauri::TitleBarStyle::Overlay);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let builder = builder.decorations(false);
    let _window = builder
        .build()
        .map_err(|e| e.to_string())?;

    Ok(serde_json::json!({ "opened": true }))
}

#[cfg(test)]
mod tests {
    use super::folder_to_open;

    #[test]
    fn open_folder_takes_only_an_existing_absolute_directory() {
        let d = tempfile::tempdir().expect("tempdir");
        let dir = d.path().to_str().expect("utf8").to_string();
        assert_eq!(folder_to_open(&dir).expect("a folder opens"), d.path());

        let file = d.path().join("run-me.command");
        std::fs::write(&file, "#!/bin/sh\n").expect("write file");
        let err = folder_to_open(file.to_str().expect("utf8")).expect_err("a file is refused");
        assert!(err.contains("not a folder"), "{err}");

        let err = folder_to_open(d.path().join("missing").to_str().expect("utf8")).expect_err("missing is refused");
        assert!(err.contains("open_folder:"), "{err}");
        assert!(folder_to_open("relative/dir").expect_err("relative is refused").contains("absolute"));
        assert!(folder_to_open("").expect_err("empty is refused").contains("absolute"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn open_folder_refuses_an_app_bundle_on_macos() {
        let d = tempfile::tempdir().expect("tempdir");
        let app = d.path().join("Evil.APP");
        std::fs::create_dir_all(&app).expect("mkdir bundle");
        let err = folder_to_open(app.to_str().expect("utf8")).expect_err("a bundle is refused");
        assert!(err.contains("app bundle"), "{err}");
    }
}
