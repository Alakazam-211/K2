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

use crate::daemon_client::{off_main, DaemonClient};
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

// The daemon-calling commands below are `async` and run the call on the
// blocking pool (`off_main`): a sync Tauri command runs ON the main thread,
// so a slow or wedged daemon (10 s reqwest timeout) froze the window. Each
// sends the same request as before; the `*_with` fns hold it for the tests.

#[tauri::command]
pub async fn projects_open_in_finder(path: String) -> Result<(), String> {
    off_main(move || open_in_finder_with(&daemon()?, &path)).await
}

fn open_in_finder_with(client: &DaemonClient, path: &str) -> Result<(), String> {
    client
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
pub async fn projects_open_in_editor(editor_id: String, path: String) -> Result<(), String> {
    off_main(move || open_in_editor_with(&daemon()?, &editor_id, &path)).await
}

fn open_in_editor_with(client: &DaemonClient, editor_id: &str, path: &str) -> Result<(), String> {
    client
        .cli_post_json(
            "/cli/projects/open-in-editor",
            &json!({ "editorId": editor_id, "path": path }),
        )
        .map(|_| ())
}

#[tauri::command]
pub async fn projects_get_editors() -> Result<Vec<EditorInfo>, String> {
    off_main(|| get_editors_with(&daemon()?)).await
}

fn get_editors_with(client: &DaemonClient) -> Result<Vec<EditorInfo>, String> {
    client.cli_get_json("/cli/projects/get-editors", &[])
}

#[tauri::command]
pub async fn projects_open_in_terminal(terminal_app: String, path: String) -> Result<(), String> {
    off_main(move || open_in_terminal_with(&daemon()?, &terminal_app, &path)).await
}

fn open_in_terminal_with(client: &DaemonClient, terminal_app: &str, path: &str) -> Result<(), String> {
    client
        .cli_post_json(
            "/cli/projects/open-in-terminal",
            &json!({ "terminalApp": terminal_app, "path": path }),
        )
        .map(|_| ())
}

#[tauri::command]
pub async fn projects_get_all_editors() -> Result<Vec<EditorInfo>, String> {
    off_main(|| get_all_editors_with(&daemon()?)).await
}

fn get_all_editors_with(client: &DaemonClient) -> Result<Vec<EditorInfo>, String> {
    client.cli_get_json("/cli/projects/get-all-editors", &[])
}

#[tauri::command]
pub async fn projects_refresh_editors() -> Result<Vec<EditorInfo>, String> {
    off_main(|| refresh_editors_with(&daemon()?)).await
}

fn refresh_editors_with(client: &DaemonClient) -> Result<Vec<EditorInfo>, String> {
    client.cli_post_json_decode("/cli/projects/refresh-editors", &json!({}))
}

/// HOST: opens a new Tauri WebviewWindow. Window create + management
/// is a Tauri-only API; the daemon has nothing equivalent.
///
/// `async`: the project-name lookup (`reqwest::blocking`) runs on the
/// blocking pool via `off_main`, so it neither holds the main thread nor
/// runs reqwest::blocking on an async worker ("Cannot drop a runtime in a
/// context where blocking is not allowed", which made menu/hotkey look
/// like no-ops when this command was first made async without it). The
/// window build then runs here, which Tauri v2 supports from async
/// commands (and recommends on Windows, where a sync command that creates a
/// window can deadlock).
#[tauri::command]
pub async fn projects_open_focus_window(
    app: tauri::AppHandle,
    project_id: String,
) -> Result<serde_json::Value, String> {
    // Look up the project name via the daemon (the only DB reader now).
    let lookup_id = project_id.clone();
    let project_name = off_main(move || project_name_with(&daemon()?, &lookup_id)).await?;

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

fn project_name_with(client: &DaemonClient, project_id: &str) -> Result<String, String> {
    let projects: Vec<ProjectRow> = client.cli_get_json("/cli/projects/list", &[])?;
    projects
        .into_iter()
        .find(|p| p.id == project_id)
        .map(|p| p.name)
        .ok_or_else(|| format!("Project not found: {}", project_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_client::test_support::{pairs, FakeDaemon};

    fn run<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
        tauri::async_runtime::block_on(off_main(f))
    }

    #[test]
    fn open_in_verbs_post_the_same_json_bodies() {
        let fake = FakeDaemon::serve(vec![(200, "{}"), (200, "{}"), (200, "{}")]);
        let client = DaemonClient::for_test(&fake.base, "tok");
        run(move || {
            open_in_finder_with(&client, "/w/a b")?;
            open_in_editor_with(&client, "cursor", "/w/a b")?;
            open_in_terminal_with(&client, "iTerm", "/w/a b")
        })
        .expect("all three succeed");
        let finder = fake.next();
        assert_eq!((finder.method.as_str(), finder.path.as_str()), ("POST", "/cli/projects/open-in-finder"));
        assert_eq!(finder.query, pairs(&[("token", "tok")]));
        assert_eq!(serde_json::from_str::<serde_json::Value>(&finder.body).expect("json"), json!({ "path": "/w/a b" }));
        let editor = fake.next();
        assert_eq!(editor.path, "/cli/projects/open-in-editor");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&editor.body).expect("json"),
            json!({ "editorId": "cursor", "path": "/w/a b" })
        );
        let terminal = fake.next();
        assert_eq!(terminal.path, "/cli/projects/open-in-terminal");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&terminal.body).expect("json"),
            json!({ "terminalApp": "iTerm", "path": "/w/a b" })
        );
    }

    #[test]
    fn open_in_finder_surfaces_a_daemon_error() {
        let fake = FakeDaemon::serve(vec![(500, "boom")]);
        let client = DaemonClient::for_test(&fake.base, "tok");
        assert_eq!(
            run(move || open_in_finder_with(&client, "/w")),
            Err("daemon /cli/projects/open-in-finder 500: boom".to_string())
        );
    }

    #[test]
    fn editor_lists_decode_from_their_routes() {
        let row = r#"[{"id":"cursor","label":"Cursor","mac_app":"Cursor","cli_command":"cursor","installed":true,"type":"editor"}]"#;
        let fake = FakeDaemon::serve(vec![(200, row), (200, row), (200, row)]);
        let client = DaemonClient::for_test(&fake.base, "tok");
        let (a, b, c) = run(move || Ok((get_editors_with(&client)?, get_all_editors_with(&client)?, refresh_editors_with(&client)?)))
            .expect("all three decode");
        for list in [&a, &b, &c] {
            assert_eq!(serde_json::to_value(list).expect("serialize"), serde_json::from_str::<serde_json::Value>(row).expect("json"));
        }
        let get = fake.next();
        assert_eq!((get.method.as_str(), get.path.as_str()), ("GET", "/cli/projects/get-editors"));
        let all = fake.next();
        assert_eq!((all.method.as_str(), all.path.as_str()), ("GET", "/cli/projects/get-all-editors"));
        let refresh = fake.next();
        assert_eq!((refresh.method.as_str(), refresh.path.as_str()), ("POST", "/cli/projects/refresh-editors"));
        assert_eq!(serde_json::from_str::<serde_json::Value>(&refresh.body).expect("json"), json!({}));
    }

    #[test]
    fn focus_window_looks_the_project_name_up_by_id() {
        let list = r#"[{"id":"p1","name":"One","path":"/w/1"},{"id":"p2","name":"Two","path":"/w/2"}]"#;
        let fake = FakeDaemon::serve(vec![(200, list), (200, list)]);
        let client = DaemonClient::for_test(&fake.base, "tok");
        let (found, missing) = run(move || Ok((project_name_with(&client, "p2"), project_name_with(&client, "nope"))))
            .expect("ran");
        assert_eq!(found, Ok("Two".to_string()));
        assert_eq!(missing, Err("Project not found: nope".to_string()));
        let seen = fake.next();
        assert_eq!((seen.method.as_str(), seen.path.as_str()), ("GET", "/cli/projects/list"));
    }

    // ── open_folder (SS16): only an existing absolute directory ──

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
