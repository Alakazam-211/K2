//! Phase 2.1c Item 2 — Tauri command wrappers for the workspace inbox
//! primitive (`k2_core::inbox`).
//!
//! These are thin shims around `k2_core::inbox::*`. The renderer
//! invokes these via `invoke('k2so_inbox_*', ...)`. They mirror the
//! daemon-side `/cli/inbox/*` HTTP routes — same arguments, same
//! return shapes — so the React layer and the CLI see the same data.
//!
//! Daemon-first per `feedback_daemon_first.md`: all the logic lives
//! in `k2_core::inbox`. These commands hold zero business logic.

use k2_core::inbox::InboxItem;

// ── Read ───────────────────────────────────────────────────────────────

/// List items in `folder`. Empty `folder` (or omitted) → top-level
/// inbox arrivals.
#[tauri::command]
pub fn k2so_inbox_list(
    project_path: String,
    folder: Option<String>,
) -> Result<Vec<InboxItem>, String> {
    let folder = folder.unwrap_or_default();
    let workspace = std::path::PathBuf::from(&project_path);
    Ok(k2_core::inbox::list_folder(&workspace, &folder))
}

/// Lightweight count of top-level inbox items. Lets badge call sites
/// avoid pulling full payloads when they only need `.length`.
#[tauri::command]
pub fn k2so_inbox_count(project_path: String) -> Result<usize, String> {
    let workspace = std::path::PathBuf::from(&project_path);
    Ok(k2_core::inbox::list_root(&workspace).len())
}

/// List folders the workspace has under `.k2so/inbox/`. Standard
/// folders (`active`, `done`) plus any custom ones the agent created.
#[tauri::command]
pub fn k2so_inbox_folders(project_path: String) -> Result<Vec<String>, String> {
    let workspace = std::path::PathBuf::from(&project_path);
    Ok(k2_core::inbox::list_folders(&workspace))
}

/// Read the full markdown content of one item by id (filename stem).
#[tauri::command]
pub fn k2so_inbox_read(project_path: String, id: String) -> Result<String, String> {
    let workspace = std::path::PathBuf::from(&project_path);
    k2_core::inbox::read_by_id(&workspace, &id)
}

/// Substring search across all inbox items (title + filename + body).
#[tauri::command]
pub fn k2so_inbox_search(project_path: String, query: String) -> Result<Vec<InboxItem>, String> {
    let workspace = std::path::PathBuf::from(&project_path);
    Ok(k2_core::inbox::search(&workspace, &query))
}

// ── Write ──────────────────────────────────────────────────────────────

/// Compose a new item at the inbox root.
#[tauri::command]
pub fn k2so_inbox_compose(
    project_path: String,
    title: String,
    body: String,
    priority: Option<String>,
    source: Option<String>,
    from: Option<String>,
) -> Result<InboxItem, String> {
    let workspace = std::path::PathBuf::from(&project_path);
    k2_core::inbox::compose(
        &workspace,
        &title,
        &body,
        priority.as_deref(),
        source.as_deref(),
        from.as_deref(),
    )
}

/// Move an item to a folder. Empty `folder` returns to root.
#[tauri::command]
pub fn k2so_inbox_move(
    project_path: String,
    id: String,
    folder: String,
) -> Result<(), String> {
    let workspace = std::path::PathBuf::from(&project_path);
    k2_core::inbox::move_item(&workspace, &id, &folder).map(|_| ())
}

/// Move an item to the standard `done/` folder.
#[tauri::command]
pub fn k2so_inbox_archive(project_path: String, id: String) -> Result<(), String> {
    let workspace = std::path::PathBuf::from(&project_path);
    k2_core::inbox::archive(&workspace, &id).map(|_| ())
}

/// Send an item to the macOS Recycle Bin. Recoverable from Trash.
/// Uses `safe_delete::trash` per `feedback_recycle_bin_tests.md`.
#[tauri::command]
pub fn k2so_inbox_delete(project_path: String, id: String) -> Result<(), String> {
    let workspace = std::path::PathBuf::from(&project_path);
    k2_core::inbox::delete(&workspace, &id)
}

/// Append a reply block to an existing inbox item.
#[tauri::command]
pub fn k2so_inbox_respond(
    project_path: String,
    id: String,
    text: String,
) -> Result<(), String> {
    let workspace = std::path::PathBuf::from(&project_path);
    k2_core::inbox::respond(&workspace, &id, &text).map(|_| ())
}
