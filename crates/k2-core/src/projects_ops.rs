//! Phase 2 Unit 4 — daemon-side operations for `projects` and related
//! tables (workspaces auto-created, focus-group reconciliation, icon
//! detection + caching).
//!
//! Moved verbatim from `src-tauri/src/commands/projects.rs` — bodies
//! were re-keyed to call into `k2_core::db::shared()` instead of
//! Tauri's `State<AppState>`. The `tauri::AppHandle::emit("sync:*")`
//! event emission was removed from these bodies; the daemon's
//! event broadcast (`/events` WS) is the new fan-out path. Tauri-side
//! proxies that wrap these calls keep emitting `sync:projects` from
//! the shim so the renderer's existing listeners keep working during
//! the transition.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use uuid::Uuid;

use crate::db;
use crate::db::schema::{FocusGroup, Project, Workspace};
use crate::project_config;

// ── Icon detection ──────────────────────────────────────────────────────

const ICON_BASENAMES: &[&str] = &["favicon", "icon", "logo", "app-icon"];
const ICON_EXTENSIONS: &[&str] = &[".svg", ".png", ".ico", ".jpg", ".jpeg", ".icns"];

fn extension_priority(ext: &str) -> u32 {
    match ext.to_lowercase().as_str() {
        ".svg" => 0,
        ".png" => 1,
        ".ico" => 2,
        ".jpg" | ".jpeg" => 3,
        ".icns" => 4,
        _ => 99,
    }
}

fn is_icon_filename(name: &str) -> bool {
    let lower = name.to_lowercase();
    let ext = Path::new(&lower)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let base = lower.strip_suffix(&ext).unwrap_or(&lower);
    ICON_BASENAMES.contains(&base) && ICON_EXTENSIONS.contains(&ext.as_str())
}

fn find_icon_files(dir: &Path, max_depth: u32, current_depth: u32) -> Vec<std::path::PathBuf> {
    let mut results = Vec::new();
    if current_depth > max_depth {
        return results;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return results,
    };
    let skip_dirs = [
        "node_modules", ".git", ".next", "dist", "out", "coverage", ".cache",
    ];
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        if let Ok(ft) = entry.file_type() {
            if ft.is_file() && is_icon_filename(&name) {
                results.push(path);
            } else if ft.is_dir()
                && current_depth < max_depth
                && !skip_dirs.contains(&name.as_str())
            {
                results.extend(find_icon_files(&path, max_depth, current_depth + 1));
            }
        }
    }
    results
}

pub fn read_icon_as_data_url(file_path: &Path) -> Option<String> {
    use base64::Engine;

    let ext = file_path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    if ext == "svg" {
        let svg = std::fs::read_to_string(file_path).ok()?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(svg.as_bytes());
        return Some(format!("data:image/svg+xml;base64,{}", encoded));
    }

    let data = std::fs::read(file_path).ok()?;
    let img = image::load_from_memory(&data).ok()?;
    let resized = img.resize_exact(48, 48, image::imageops::FilterType::Lanczos3);
    let mut buf = std::io::Cursor::new(Vec::new());
    resized.write_to(&mut buf, image::ImageFormat::Png).ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(buf.into_inner());
    Some(format!("data:image/png;base64,{}", encoded))
}

fn check_package_json_icon(project_path: &Path) -> Option<std::path::PathBuf> {
    let pkg_path = project_path.join("package.json");
    if !pkg_path.exists() {
        return None;
    }
    let raw = std::fs::read_to_string(&pkg_path).ok()?;
    let pkg: serde_json::Value = serde_json::from_str(&raw).ok()?;
    if let Some(icon) = pkg.get("icon").and_then(|v| v.as_str()) {
        let icon_path = project_path.join(icon);
        if icon_path.exists() {
            return Some(icon_path);
        }
    }
    if let Some(icon) = pkg
        .get("build")
        .and_then(|b| b.get("icon"))
        .and_then(|v| v.as_str())
    {
        let icon_path = project_path.join(icon);
        if icon_path.exists() {
            return Some(icon_path);
        }
    }
    None
}

fn check_manifest_icons(project_path: &Path) -> Option<std::path::PathBuf> {
    let manifest_paths = [
        "manifest.json",
        "site.webmanifest",
        "public/manifest.json",
        "public/site.webmanifest",
        "src/manifest.json",
    ];
    for mp in &manifest_paths {
        let full_path = project_path.join(mp);
        if !full_path.exists() {
            continue;
        }
        let raw = match std::fs::read_to_string(&full_path) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let manifest: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if let Some(icons) = manifest.get("icons").and_then(|v| v.as_array()) {
            if icons.is_empty() {
                continue;
            }
            let mut sorted_icons: Vec<&serde_json::Value> = icons.iter().collect();
            sorted_icons.sort_by(|a, b| {
                let size_a: u32 = a
                    .get("sizes")
                    .and_then(|s| s.as_str())
                    .and_then(|s| s.split('x').next())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let size_b: u32 = b
                    .get("sizes")
                    .and_then(|s| s.as_str())
                    .and_then(|s| s.split('x').next())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                size_b.cmp(&size_a)
            });
            for icon in sorted_icons {
                if let Some(src) = icon.get("src").and_then(|v| v.as_str()) {
                    let icon_path = project_path.join(src);
                    if icon_path.exists() {
                        return Some(icon_path);
                    }
                }
            }
        }
    }
    None
}

pub fn detect_project_icon(project_path: &str) -> Option<String> {
    let path = Path::new(project_path);
    if let Some(pkg_icon) = check_package_json_icon(path) {
        if let Some(data_url) = read_icon_as_data_url(&pkg_icon) {
            return Some(data_url);
        }
    }
    if let Some(manifest_icon) = check_manifest_icons(path) {
        if let Some(data_url) = read_icon_as_data_url(&manifest_icon) {
            return Some(data_url);
        }
    }
    let static_paths = [
        "favicon.ico", "favicon.png", "favicon.svg",
        "public/favicon.ico", "public/favicon.png", "public/favicon.svg",
        "static/favicon.ico", "static/favicon.png",
        "icon.png", "icon.svg", "icon.ico",
        "app-icon.png", "logo.png", "logo.svg", "logo.ico",
        ".icon.png",
        "app/favicon.ico", "app/favicon.png", "app/icon.ico", "app/icon.png", "app/icon.svg",
        "app/public/favicon.ico", "app/public/favicon.png",
        "src/favicon.ico", "src/favicon.png",
        "src/assets/icon.png", "src/assets/icon.svg", "src/assets/logo.png", "src/assets/logo.svg",
        "src/assets/favicon.ico", "src/assets/favicon.png",
        "src/app/favicon.ico", "src/app/icon.png",
        "resources/icon.png", "resources/icon.ico",
        "build/icon.png", "build/icon.ico", "build/icon.icns",
        "buildResources/icon.png", "buildResources/icon.ico",
        "assets/icon.png", "assets/logo.png",
    ];
    let mut sorted_static: Vec<&&str> = static_paths.iter().collect();
    sorted_static.sort_by(|a, b| {
        let ext_a = Path::new(a)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let ext_b = Path::new(b)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        extension_priority(&ext_a).cmp(&extension_priority(&ext_b))
    });
    for icon_path in &sorted_static {
        let full_path = path.join(icon_path);
        if full_path.exists() {
            if let Some(data_url) = read_icon_as_data_url(&full_path) {
                return Some(data_url);
            }
        }
    }
    let mut found = find_icon_files(path, 2, 0);
    if !found.is_empty() {
        found.sort_by(|a, b| {
            let ext_a = a
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let ext_b = b
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            extension_priority(&ext_a).cmp(&extension_priority(&ext_b))
        });
        if let Some(data_url) = read_icon_as_data_url(&found[0]) {
            return Some(data_url);
        }
    }
    None
}

// ── Transaction helper ─────────────────────────────────────────────────

fn with_transaction<T, F>(conn: &rusqlite::Connection, f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String>,
{
    conn.execute_batch("BEGIN")
        .map_err(|e| format!("Failed to begin transaction: {}", e))?;
    match f() {
        Ok(val) => {
            conn.execute_batch("COMMIT")
                .map_err(|e| format!("Failed to commit: {}", e))?;
            Ok(val)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

fn next_tab_order(conn: &rusqlite::Connection) -> i64 {
    let projects = Project::list(conn).unwrap_or_default();
    projects.iter().map(|p| p.tab_order).max().unwrap_or(-1) + 1
}

// ── Project CRUD ───────────────────────────────────────────────────────

pub fn projects_list() -> Result<Vec<Project>, String> {
    let db = db::shared();
    let conn = db.lock();
    // Filter out the internal audit sentinels (`_orphan`, `_broadcast`).
    // They're seeded into `projects` only to satisfy the activity-feed
    // FK and must never appear in the user-facing workspace sidebar.
    // The unfiltered `Project::list` below stays intact for internal
    // callers (heartbeat/agent scanning, dedup/import, migrations).
    let projects = Project::list(&conn)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|p| !db::AUDIT_SENTINEL_IDS.contains(&p.id.as_str()))
        .collect();
    Ok(projects)
}

pub fn projects_create(
    name: &str,
    path: &str,
    color: Option<&str>,
) -> Result<Project, String> {
    let db = db::shared();
    let conn = db.lock();
    let project_id = Uuid::new_v4().to_string();
    let workspace_id = Uuid::new_v4().to_string();
    let tab_order = next_tab_order(&conn);
    let color = color.unwrap_or("#3b82f6").to_string();

    with_transaction(&conn, || {
        Project::create(&conn, &project_id, name, path, &color, tab_order, 1, None, None)
            .map_err(|e| e.to_string())?;
        Workspace::create(
            &conn,
            &workspace_id,
            &project_id,
            None,
            "branch",
            Some("main"),
            "main",
            0,
            None,
        )
        .map_err(|e| e.to_string())?;
        Project::get(&conn, &project_id).map_err(|e| e.to_string())
    })
}

#[allow(clippy::too_many_arguments)]
pub fn projects_update(
    id: &str,
    name: Option<&str>,
    color: Option<&str>,
    tab_order: Option<i64>,
    worktree_mode: Option<i64>,
    pinned: Option<i64>,
    manually_active: Option<i64>,
    icon_url: Option<Option<&str>>,
    agent_enabled: Option<i64>,
    heartbeat_enabled: Option<i64>,
    agent_mode: Option<String>,
    state_id: Option<Option<&str>>,
    heartbeat_mode: Option<String>,
    heartbeat_schedule: Option<Option<&str>>,
) -> Result<Project, String> {
    // Pre-update: if agent_mode is changing, archive orphan agents for
    // the project's path BEFORE applying the swap. Uses the same in-
    // process helper the agents-archive-orphans route calls — we're
    // already inside the daemon so no need for the HTTP roundtrip the
    // Tauri shim used.
    if agent_mode.is_some() {
        let project_path_for_archive: Option<String> = {
            let db = db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT path FROM projects WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get::<_, String>(0),
            )
            .ok()
        };
        if let Some(path) = project_path_for_archive {
            let _ = crate::workspace::migrations::archive_orphan_top_tier_agents(&path);
        }
    }

    let db = db::shared();
    let conn = db.lock();
    Project::update(
        &conn,
        id,
        name,
        None,
        color,
        tab_order,
        worktree_mode,
        icon_url,
        None,
        pinned,
        manually_active,
        agent_enabled,
        heartbeat_enabled,
        agent_mode,
        state_id,
        heartbeat_mode,
        heartbeat_schedule,
    )
    .map_err(|e| e.to_string())?;
    Project::get(&conn, id).map_err(|e| e.to_string())
}

pub fn projects_delete(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    conn.execute(
        "DELETE FROM workspaces WHERE project_id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    Project::delete(&conn, id).map_err(|e| e.to_string())
}

pub fn projects_reorder(ids: &[String]) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    for (i, id) in ids.iter().enumerate() {
        Project::update(
            &conn,
            id,
            None, None, None, Some(i as i64), None, None, None, None, None, None, None, None, None, None, None,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ── Canonical Active set (task #672) ───────────────────────────────────

/// Load every project as a DB-free [`crate::active::ProjectRow`] view
/// for the canonical Active compute. Includes the audit sentinels
/// (`_orphan`/`_broadcast`) — they can never be Active (never pinned,
/// never interacted with), so they cost nothing and skipping the
/// `projects_list` filter keeps this a straight column read.
pub fn active_project_rows() -> Result<Vec<crate::active::ProjectRow>, String> {
    let db = db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare("SELECT id, manually_active, last_interaction_at FROM projects")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(crate::active::ProjectRow {
                id: row.get(0)?,
                manually_active: row.get(1)?,
                last_interaction_at_secs: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// Compute the canonical Active project-id set against the live DB.
/// `now_ms` is unix milliseconds; `active_window_hours` comes from
/// `app_settings`. Thin wrapper over [`crate::active::active_project_ids`]
/// — used by the daemon's `GET /cli/projects/active` route + the Active
/// reaper so both share one truth.
pub fn compute_active_project_ids(
    now_ms: i64,
    active_window_hours: u32,
) -> Result<Vec<String>, String> {
    let rows = active_project_rows()?;
    Ok(crate::active::active_project_ids(now_ms, &rows, active_window_hours))
}

/// Set `projects.manually_active` for a single project (the dedicated
/// pin/unpin write behind `POST /cli/projects/pin`). `pinned = true`
/// pins the workspace into the Active bar; `false` clears the pin.
/// Returns `Ok(())` even if the id doesn't exist (idempotent UPDATE).
pub fn projects_set_manually_active(id: &str, pinned: bool) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE projects SET manually_active = ?1 WHERE id = ?2",
        rusqlite::params![if pinned { 1i64 } else { 0i64 }, id],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

pub fn projects_touch_interaction(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    Project::touch_interaction(&conn, id).map_err(|e| e.to_string())
}

pub fn projects_touch_interaction_clear(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    Project::clear_interaction(&conn, id).map_err(|e| e.to_string())
}

// ── Project add flows ──────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum AddFromPathResult {
    NeedsGitInit {
        #[serde(rename = "needsGitInit")]
        needs_git_init: bool,
        path: String,
        name: String,
    },
    Project(Project),
}

pub fn projects_add_from_path(path: &str) -> Result<AddFromPathResult, String> {
    let p = Path::new(path);
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());

    let git_dir = p.join(".git");
    let is_git_repo = if git_dir.exists() {
        true
    } else {
        Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };

    if !is_git_repo {
        return Ok(AddFromPathResult::NeedsGitInit {
            needs_git_init: true,
            path: path.to_string(),
            name,
        });
    }

    if !p.exists() {
        return Err(format!("Path does not exist: {}", path));
    }

    let db = db::shared();
    let conn = db.lock();
    let existing_projects = Project::list(&conn).unwrap_or_default();
    let path_with_sep = format!("{}/", path.trim_end_matches('/'));
    for ep in &existing_projects {
        if path == ep.path {
            return Err(format!("This folder is already added as workspace '{}'.", ep.name));
        }
        let ep_with_sep = format!("{}/", ep.path.trim_end_matches('/'));
        if path_with_sep.starts_with(&ep_with_sep) || ep_with_sep.starts_with(&path_with_sep) {
            let workspaces = Workspace::list(&conn, &ep.id).unwrap_or_default();
            for ws in &workspaces {
                if let Some(ref wt_path) = ws.worktree_path {
                    let wt_with_sep = format!("{}/", wt_path.trim_end_matches('/'));
                    if path_with_sep.starts_with(&wt_with_sep)
                        || wt_with_sep.starts_with(&path_with_sep)
                    {
                        return Err(format!(
                            "This folder overlaps with a worktree in workspace '{}'. Remove it first or choose a different folder.",
                            ep.name
                        ));
                    }
                }
            }
        }
    }

    let project_id = Uuid::new_v4().to_string();
    let workspace_id = Uuid::new_v4().to_string();
    let tab_order = next_tab_order(&conn);

    let current_branch = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "main".to_string());

    let result = with_transaction(&conn, || {
        Project::create(
            &conn, &project_id, &name, path, "#3b82f6", tab_order, 0, None, None,
        )
        .map_err(|e| e.to_string())?;
        Workspace::create(
            &conn,
            &workspace_id,
            &project_id,
            None,
            "branch",
            Some(&current_branch),
            &current_branch,
            0,
            None,
        )
        .map_err(|e| e.to_string())?;
        let config = project_config::get_project_config(path);
        if let Some(ref focus_group_name) = config.focus_group_name {
            reconcile_focus_group(&conn, &project_id, focus_group_name)?;
        }
        let project = Project::get(&conn, &project_id).map_err(|e| e.to_string())?;
        Ok(AddFromPathResult::Project(project))
    })?;
    Ok(result)
}

fn reconcile_focus_group(
    conn: &rusqlite::Connection,
    project_id: &str,
    focus_group_name: &str,
) -> Result<(), String> {
    let groups = FocusGroup::list(conn).map_err(|e| e.to_string())?;
    let existing = groups.iter().find(|g| g.name == focus_group_name);
    let group_id = if let Some(g) = existing {
        g.id.clone()
    } else {
        let new_id = Uuid::new_v4().to_string();
        let max_order = groups.iter().map(|g| g.tab_order).max().unwrap_or(-1) + 1;
        FocusGroup::create(conn, &new_id, focus_group_name, None, max_order)
            .map_err(|e| e.to_string())?;
        new_id
    };
    Project::update(
        conn,
        project_id,
        None, None, None, None, None, None,
        Some(Some(group_id.as_str())),
        None, None, None, None, None, None, None, None,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn projects_add_without_git(path: &str) -> Result<Project, String> {
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());

    let db = db::shared();
    let conn = db.lock();
    let project_id = Uuid::new_v4().to_string();
    let workspace_id = Uuid::new_v4().to_string();
    let tab_order = next_tab_order(&conn);

    with_transaction(&conn, || {
        Project::create(
            &conn, &project_id, &name, path, "#3b82f6", tab_order, 0, None, None,
        )
        .map_err(|e| e.to_string())?;
        Workspace::create(
            &conn,
            &workspace_id,
            &project_id,
            None,
            "default",
            None,
            &name,
            0,
            None,
        )
        .map_err(|e| e.to_string())?;
        Project::get(&conn, &project_id).map_err(|e| e.to_string())
    })
}

pub fn projects_init_git_and_open(path: &str, branch: Option<&str>) -> Result<Project, String> {
    let branch_name = branch
        .filter(|b| !b.trim().is_empty())
        .unwrap_or("main")
        .to_string();

    let init_output = Command::new("git")
        .args(["init", &format!("--initial-branch={}", branch_name)])
        .current_dir(path)
        .output()
        .map_err(|e| format!("Failed to run git init: {}", e))?;
    if !init_output.status.success() {
        let stderr = String::from_utf8_lossy(&init_output.stderr);
        return Err(format!("Failed to initialize git: {}", stderr));
    }
    let commit_output = Command::new("git")
        .args(["commit", "--allow-empty", "-m", "Initial commit"])
        .current_dir(path)
        .output()
        .map_err(|e| format!("Failed to run git commit: {}", e))?;
    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        if stderr.contains("user.email") || stderr.contains("user.name") {
            return Err(
                "Git user not configured. Run:\n  git config --global user.name \"Your Name\"\n  git config --global user.email \"you@example.com\""
                    .to_string(),
            );
        }
        return Err(format!("Failed to create initial commit: {}", stderr));
    }

    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());

    let db = db::shared();
    let conn = db.lock();
    let project_id = Uuid::new_v4().to_string();
    let workspace_id = Uuid::new_v4().to_string();
    let tab_order = next_tab_order(&conn);

    with_transaction(&conn, || {
        Project::create(
            &conn, &project_id, &name, path, "#3b82f6", tab_order, 0, None, None,
        )
        .map_err(|e| e.to_string())?;
        Workspace::create(
            &conn,
            &workspace_id,
            &project_id,
            None,
            "branch",
            Some(&branch_name),
            &branch_name,
            0,
            None,
        )
        .map_err(|e| e.to_string())?;
        Project::get(&conn, &project_id).map_err(|e| e.to_string())
    })
}

// ── Worktree enablement ────────────────────────────────────────────────

pub fn projects_enable_worktrees(project_id: &str) -> Result<Project, String> {
    let db = db::shared();
    let conn = db.lock();
    let project = Project::get(&conn, project_id).map_err(|e| e.to_string())?;

    with_transaction(&conn, || {
        Project::update(
            &conn, project_id, None, None, None, None, Some(1), None, None, None, None, None, None,
            None, None, None, None,
        )
        .map_err(|e| e.to_string())?;

        let existing_workspaces = Workspace::list(&conn, project_id).unwrap_or_default();
        let worktrees = crate::git::list_worktrees(&project.path);

        for wt in &worktrees {
            if wt.is_main {
                if let Some(main_ws) = existing_workspaces
                    .iter()
                    .find(|ws| ws.type_ == "branch" || ws.type_ == "default")
                {
                    if main_ws.branch.as_deref() != Some(&wt.branch) {
                        conn.execute(
                            "UPDATE workspaces SET branch = ?1, name = ?2 WHERE id = ?3",
                            rusqlite::params![wt.branch, wt.branch, main_ws.id],
                        )
                        .map_err(|e| e.to_string())?;
                    }
                    if main_ws.type_ != "branch" {
                        conn.execute(
                            "UPDATE workspaces SET type = 'branch' WHERE id = ?1",
                            rusqlite::params![main_ws.id],
                        )
                        .map_err(|e| e.to_string())?;
                    }
                }
            } else {
                let already_tracked = existing_workspaces
                    .iter()
                    .any(|ws| ws.worktree_path.as_deref() == Some(&wt.path));
                if !already_tracked {
                    let ws_id = Uuid::new_v4().to_string();
                    let max_order =
                        existing_workspaces.iter().map(|w| w.tab_order).max().unwrap_or(-1) + 1;
                    Workspace::create(
                        &conn,
                        &ws_id,
                        project_id,
                        None,
                        "worktree",
                        Some(&wt.branch),
                        &wt.branch,
                        max_order,
                        Some(&wt.path),
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }
        Project::get(&conn, project_id).map_err(|e| e.to_string())
    })
}

// ── Icon commands ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IconResult {
    pub found: bool,
    #[serde(rename = "dataUrl")]
    pub data_url: Option<String>,
}

pub fn projects_get_icon(path: &str, project_id: Option<&str>) -> Result<IconResult, String> {
    if let Some(pid) = project_id {
        let db = db::shared();
        let conn = db.lock();
        if let Ok(project) = Project::get(&conn, pid) {
            if let Some(ref icon_url) = project.icon_url {
                return Ok(IconResult {
                    found: true,
                    data_url: Some(icon_url.clone()),
                });
            }
        }
    }

    if let Some(data_url) = detect_project_icon(path) {
        if let Some(pid) = project_id {
            let db = db::shared();
            let conn = db.lock();
            Project::update(
                &conn,
                pid,
                None, None, None, None, None,
                Some(Some(data_url.as_str())),
                None, None, None, None, None, None, None, None, None,
            )
            .ok();
        }
        return Ok(IconResult {
            found: true,
            data_url: Some(data_url),
        });
    }

    Ok(IconResult {
        found: false,
        data_url: None,
    })
}

pub fn projects_detect_icon(project_id: &str) -> Result<IconResult, String> {
    let db = db::shared();
    let conn = db.lock();
    let project = Project::get(&conn, project_id).map_err(|e| e.to_string())?;
    if let Some(data_url) = detect_project_icon(&project.path) {
        Project::update(
            &conn,
            project_id,
            None, None, None, None, None,
            Some(Some(data_url.as_str())),
            None, None, None, None, None, None, None, None, None,
        )
        .map_err(|e| e.to_string())?;
        Ok(IconResult {
            found: true,
            data_url: Some(data_url),
        })
    } else {
        Ok(IconResult {
            found: false,
            data_url: None,
        })
    }
}

pub fn projects_set_icon(project_id: &str, data_url: &str) -> Result<IconResult, String> {
    let db = db::shared();
    let conn = db.lock();
    Project::get(&conn, project_id).map_err(|_| "Project not found".to_string())?;
    Project::update(
        &conn,
        project_id,
        None, None, None, None, None,
        Some(Some(data_url)),
        None, None, None, None, None, None, None, None, None,
    )
    .map_err(|e| e.to_string())?;
    Ok(IconResult {
        found: true,
        data_url: Some(data_url.to_string()),
    })
}

pub fn projects_clear_icon(project_id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    Project::update(
        &conn,
        project_id,
        None, None, None, None, None,
        Some(None),
        None, None, None, None, None, None, None, None, None,
    )
    .map_err(|e| e.to_string())
}

pub fn project_name(project_id: &str) -> Result<String, String> {
    let db = db::shared();
    let conn = db.lock();
    let p = Project::get(&conn, project_id).map_err(|e| e.to_string())?;
    Ok(p.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ensure_project(id: &str, path: &str, name: &str) {
        let db = db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR REPLACE INTO projects \
             (id, path, name, color, agent_mode, pinned, tab_order) \
             VALUES (?1, ?2, ?3, '#123456', 'off', 0, 0)",
            rusqlite::params![id, path, name],
        )
        .unwrap();
    }

    fn delete_project(id: &str) {
        let db = db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id]);
    }

    /// `projects_list()` is the UI-facing wrapper. It MUST hide the
    /// internal audit sentinels (`_orphan`, `_broadcast`), while the
    /// underlying `Project::list()` — used by internal callers like
    /// heartbeat/agent scanning and dedup/import — MUST still return
    /// them. Asserting both surfaces here pins the invariant that they
    /// differ by exactly the two sentinel rows.
    #[test]
    fn projects_list_hides_audit_sentinels_but_project_list_keeps_them() {
        // init_for_tests() seeds `_orphan` + `_broadcast` (see
        // db::init_for_tests). Add a real workspace so the user-facing
        // surface is non-empty either way.
        db::init_for_tests();
        ensure_project(
            "real-ws-projects-ops",
            "/tmp/k2so-projects-ops-test",
            "RealOne",
        );

        // UI-facing surface: sentinels filtered out, real workspace present.
        let visible = projects_list().expect("projects_list ok");
        let visible_ids: Vec<&str> = visible.iter().map(|p| p.id.as_str()).collect();
        assert!(
            !visible_ids.contains(&"_orphan"),
            "projects_list() must not leak the _orphan audit sentinel: {visible_ids:?}"
        );
        assert!(
            !visible_ids.contains(&"_broadcast"),
            "projects_list() must not leak the _broadcast audit sentinel: {visible_ids:?}"
        );
        assert!(
            visible_ids.contains(&"real-ws-projects-ops"),
            "real workspace must still appear in projects_list(): {visible_ids:?}"
        );

        // Internal surface: the raw Project::list() STILL returns the
        // sentinels (internal callers depend on a complete row set).
        let db_handle = db::shared();
        let conn = db_handle.lock();
        let raw_ids: Vec<String> = Project::list(&conn)
            .expect("Project::list ok")
            .into_iter()
            .map(|p| p.id)
            .collect();
        drop(conn);
        assert!(
            raw_ids.iter().any(|id| id == "_orphan"),
            "Project::list() must still return the _orphan sentinel for internal callers: {raw_ids:?}"
        );
        assert!(
            raw_ids.iter().any(|id| id == "_broadcast"),
            "Project::list() must still return the _broadcast sentinel for internal callers: {raw_ids:?}"
        );

        // The two surfaces differ by EXACTLY the two sentinels.
        let mut only_in_raw: Vec<&String> = raw_ids
            .iter()
            .filter(|id| !visible_ids.contains(&id.as_str()))
            .collect();
        only_in_raw.sort();
        assert_eq!(
            only_in_raw,
            vec![&"_broadcast".to_string(), &"_orphan".to_string()],
            "projects_list() and Project::list() must differ by exactly the two audit sentinels"
        );

        delete_project("real-ws-projects-ops");
    }
}
