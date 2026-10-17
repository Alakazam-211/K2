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
    detect_project_icon_file(project_path).map(|(_, data_url)| data_url)
}

/// [`detect_project_icon`] plus the file the icon was read from (the
/// [`project_icon_rev`] cache stamps that file's mtime + length).
fn detect_project_icon_file(project_path: &str) -> Option<(std::path::PathBuf, String)> {
    let path = Path::new(project_path);
    if let Some(pkg_icon) = check_package_json_icon(path) {
        if let Some(data_url) = read_icon_as_data_url(&pkg_icon) {
            return Some((pkg_icon, data_url));
        }
    }
    if let Some(manifest_icon) = check_manifest_icons(path) {
        if let Some(data_url) = read_icon_as_data_url(&manifest_icon) {
            return Some((manifest_icon, data_url));
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
                return Some((full_path, data_url));
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
            return Some((found.swap_remove(0), data_url));
        }
    }
    None
}

// ── Icon revision (`iconRev` / Thread push `avatarRev`) ────────────────

/// Hex characters in an icon rev.
pub const ICON_REV_LEN: usize = 16;

/// How long a "this repo has no icon file" answer is reused before the
/// repo is walked again.
const NO_ICON_RECHECK: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// The rev of one icon string: the first [`ICON_REV_LEN`] lowercase hex
/// characters of the SHA-256 of its UTF-8 bytes; `""` for no icon or an
/// empty one.
pub fn icon_rev_of(icon: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    match icon {
        Some(s) if !s.is_empty() => {
            let digest = Sha256::digest(s.as_bytes());
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            hex[..ICON_REV_LEN].to_string()
        }
        _ => String::new(),
    }
}

/// The ONE icon rev for a workspace, shared by `GET /cli/projects/list`
/// (`iconRev`) and the Thread push (`data.avatarRev`) so the phone can
/// compare the two directly.
///
/// It hashes exactly the string `/cli/projects/get-icon` returns for the
/// workspace: the stored `projects.icon_url` (a data URL or an http(s)
/// URL, hashed as text, never fetched) when the row has one, else the data
/// URL detected from the repo's icon file. Detection is cached per path
/// and re-read only when the icon file's mtime or length changes (a "no
/// icon" answer is reused for [`NO_ICON_RECHECK`]), so a list call costs
/// one `stat` per file-backed row. A detected icon and the same icon
/// after get-icon has cached it into the row have the same rev.
pub fn project_icon_rev(icon_url: Option<&str>, path: &str) -> String {
    match icon_url {
        // get-icon returns a stored value as is (even an empty one).
        Some(stored) => icon_rev_of(Some(stored)),
        None => detected_icon_rev(path),
    }
}

/// A workspace's look for a Thread push: `(agentColor, avatarRev)` —
/// `projects.color` as the projects list shows it, and
/// [`project_icon_rev`] of the row. Both are `""` when the row is gone.
/// The DB lock is released before the icon rev is computed.
pub fn project_push_look(project_id: &str) -> (String, String) {
    let row: Option<(String, Option<String>, String)> = {
        let db = db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT color, icon_url, path FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()
    };
    match row {
        Some((color, icon_url, path)) => (color, project_icon_rev(icon_url.as_deref(), &path)),
        None => (String::new(), String::new()),
    }
}

type FileStamp = (Option<std::time::SystemTime>, u64);

struct DetectedRev {
    /// The icon file and its stamp at detection; `None` = no icon found.
    file: Option<(std::path::PathBuf, FileStamp)>,
    rev: String,
    checked: std::time::Instant,
}

fn detected_rev_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, DetectedRev>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, DetectedRev>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok(), meta.len()))
}

fn detected_icon_rev(project_path: &str) -> String {
    // get-icon refuses an empty path; never walk the daemon's cwd.
    if project_path.is_empty() {
        return String::new();
    }
    {
        let cache = detected_rev_cache().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = cache.get(project_path) {
            match &entry.file {
                Some((file, stamp)) if file_stamp(file).as_ref() == Some(stamp) => {
                    return entry.rev.clone();
                }
                None if entry.checked.elapsed() < NO_ICON_RECHECK => return String::new(),
                _ => {}
            }
        }
    }
    // Walk + read outside the cache lock.
    let entry = match detect_project_icon_file(project_path) {
        Some((file, data_url)) => {
            let rev = icon_rev_of(Some(&data_url));
            let Some(stamp) = file_stamp(&file) else {
                // The file went away mid-read: answer, but don't cache.
                return rev;
            };
            DetectedRev { file: Some((file, stamp)), rev, checked: std::time::Instant::now() }
        }
        None => DetectedRev { file: None, rev: String::new(), checked: std::time::Instant::now() },
    };
    let rev = entry.rev.clone();
    detected_rev_cache()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(project_path.to_string(), entry);
    rev
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
    projects_list_on(&conn)
}

/// [`projects_list`] on an explicit connection (one snapshot: callers
/// that compare it with `Project::list` hold one lock for both).
pub fn projects_list_on(conn: &rusqlite::Connection) -> Result<Vec<Project>, String> {
    // Filter out the internal audit sentinels (`_orphan`, `_broadcast`).
    // They're seeded into `projects` only to satisfy the activity-feed
    // FK and must never appear in the user-facing workspace sidebar.
    // The unfiltered `Project::list` below stays intact for internal
    // callers (heartbeat/agent scanning, dedup/import, migrations).
    let projects = Project::list(conn)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|p| !db::AUDIT_SENTINEL_IDS.contains(&p.id.as_str()))
        .collect();
    Ok(projects)
}

/// `GET /cli/projects/list` DTO: a `Project` plus nested workspaces.
/// Flattened so JSON is `{...projectFields, workspaces: [...]}`. Empty
/// vec still emits `"workspaces":[]` — never `skip_serializing_if`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWithWorkspaces {
    #[serde(flatten)]
    pub project: Project,
    pub workspaces: Vec<Workspace>,
    /// [`project_icon_rev`] of this row: the same value a Thread push
    /// for this workspace carries as `data.avatarRev`; `""` = no icon.
    pub icon_rev: String,
}

/// Same sentinel filter as [`projects_list`], plus a nested `workspaces`
/// array per row. One `db.lock()` covers the project+workspace join so
/// the HTTP handler does not N+1. Internal callers keep using the flat
/// [`projects_list`]. `iconRev` is computed after the lock is released
/// (it may stat or read a repo icon file).
pub fn projects_list_with_workspaces() -> Result<Vec<ProjectWithWorkspaces>, String> {
    let rows = {
        let db = db::shared();
        let conn = db.lock();
        let projects = Project::list(&conn).map_err(|e| e.to_string())?;
        let mut rows = Vec::with_capacity(projects.len());
        for project in projects {
            if db::AUDIT_SENTINEL_IDS.contains(&project.id.as_str()) {
                continue;
            }
            let workspaces = Workspace::list(&conn, &project.id).map_err(|e| e.to_string())?;
            rows.push((project, workspaces));
        }
        rows
    };
    Ok(rows
        .into_iter()
        .map(|(project, workspaces)| {
            let icon_rev = project_icon_rev(project.icon_url.as_deref(), &project.path);
            ProjectWithWorkspaces { project, workspaces, icon_rev }
        })
        .collect())
}

/// Agent de-generalization S1 (migration 0063) — stamp a freshly
/// CREATED `projects` row with the CURRENT global default agent
/// (`AppSettings.default_agent`). Non-retroactive by design: only the
/// create flows below call this, so pre-existing rows keep NULL
/// (= inherit the global at resolve time) and a new workspace keeps
/// the agent it was born with even if the global default changes
/// later. Stores the value as given (preset id today, legacy command
/// token like "claude" historically) — shape is NOT validated here;
/// readers do tolerant matching. An empty global value is not stamped
/// (NULL stays the one representation of "nothing chosen").
fn stamp_default_agent(conn: &rusqlite::Connection, project_id: &str) -> Result<(), String> {
    let global = crate::app_settings::load().default_agent;
    if global.trim().is_empty() {
        return Ok(());
    }
    conn.execute(
        "UPDATE projects SET default_agent = ?1 WHERE id = ?2",
        rusqlite::params![global, project_id],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
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

    let project = with_transaction(&conn, || {
        Project::create(&conn, &project_id, name, path, &color, tab_order, 1, None, None)
            .map_err(|e| e.to_string())?;
        stamp_default_agent(&conn, &project_id)?;
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
    })?;
    crate::cli_folder_trust::trust_cli_folder(path);
    Ok(project)
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
    default_agent: Option<Option<&str>>,
    default_model: Option<Option<&str>>,
    force_model_on_resume: Option<i64>,
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
        default_agent,
        default_model,
        force_model_on_resume,
    )
    .map_err(|e| e.to_string())?;
    Project::get(&conn, id).map_err(|e| e.to_string())
}

pub fn projects_delete(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    // Project-group memberships first (no SQL FK on workspace_id —
    // 0066): callers are §4.5 PoC-guarded (poc_removal_block), so this
    // only ever removes plain memberships.
    let member_groups = crate::project_groups::remove_workspace_memberships_with(&conn, id)?;
    conn.execute(
        "DELETE FROM workspaces WHERE project_id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    // Garden data (contract-garden-data-v1 §2.1, GL33): no FK, so the
    // workspace's grants (and every grant made under its delegations) and
    // its widget-author stamps go explicitly; its widgets become unclaimed.
    crate::garden_data::grants::delete_for_workspace(&conn, id).map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM zen_widget_authors WHERE author_project_id = ?1", rusqlite::params![id])
        .map_err(|e| e.to_string())?;
    Project::delete(&conn, id).map_err(|e| e.to_string())?;
    for gid in &member_groups {
        crate::agent_hooks::emit(
            crate::agent_hooks::HookEvent::ProjectGroupMembersChanged,
            serde_json::json!({ "groupId": gid }),
        );
    }
    Ok(())
}

pub fn projects_reorder(ids: &[String]) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    for (i, id) in ids.iter().enumerate() {
        Project::update(
            &conn,
            id,
            None, None, None, Some(i as i64), None, None, None, None, None, None, None, None, None, None, None, None, None, None,
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
    projects_add_from_path_ex(path, true, true, false)
}

pub fn projects_add_from_path_ex(
    path: &str,
    seed_wiki: bool,
    seed_agents_md: bool,
    fanout: bool,
) -> Result<AddFromPathResult, String> {
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
            return Err(crate::workspace::lifecycle::workspace_already_exists_error(path));
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
        .map_err(|e| crate::workspace::lifecycle::map_projects_path_unique(path, e))?;
        stamp_default_agent(&conn, &project_id)?;
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
    if let AddFromPathResult::Project(_) = &result {
        // A folder that still has a pre-0.40.4 `.k2so/` dot-dir gets the same
        // `.k2so/` → `.k2/` cutover boot gives registered workspaces — BEFORE
        // the seeding below writes `.k2/`. Otherwise the seeds create a fresh
        // `.k2/` beside `.k2so/`, the boot cutover then refuses the pair
        // ("left for manual merge"), and the folder's inbox, skills and agent
        // are stranded under `.k2so/`. No-op without a `.k2so/` dir or when
        // `.k2/` already exists.
        let moved = crate::workspace::dot_dir_migration::migrate_workspace_dot_dir(p);
        if moved.renamed {
            crate::log_debug!(
                "[projects/add] {path}: .k2so -> .k2 ({} symlink(s) re-pointed)",
                moved.symlinks_repointed
            );
        }
    }
    if seed_wiki {
        if let AddFromPathResult::Project(_) = &result {
            crate::wiki::seed_wiki_on_add(path);
        }
    }
    if let AddFromPathResult::Project(_) = &result {
        crate::workspace::skill_regen::apply_new_workspace_agents_policy(
            path,
            seed_agents_md,
            fanout,
        );
        crate::cli_folder_trust::trust_cli_folder(path);
    }
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
        None, None, None, None, None, None, None, None, None, None, None,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn projects_add_without_git(path: &str) -> Result<Project, String> {
    projects_add_without_git_ex(path, true, true, false)
}

pub fn projects_add_without_git_ex(
    path: &str,
    seed_wiki: bool,
    seed_agents_md: bool,
    fanout: bool,
) -> Result<Project, String> {
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());

    let db = db::shared();
    let conn = db.lock();
    let existing_projects = Project::list(&conn).unwrap_or_default();
    if existing_projects.iter().any(|ep| ep.path == path) {
        return Err(crate::workspace::lifecycle::workspace_already_exists_error(path));
    }
    let project_id = Uuid::new_v4().to_string();
    let workspace_id = Uuid::new_v4().to_string();
    let tab_order = next_tab_order(&conn);

    let project = with_transaction(&conn, || {
        Project::create(
            &conn, &project_id, &name, path, "#3b82f6", tab_order, 0, None, None,
        )
        .map_err(|e| crate::workspace::lifecycle::map_projects_path_unique(path, e))?;
        stamp_default_agent(&conn, &project_id)?;
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
    })?;
    if seed_wiki {
        crate::wiki::seed_wiki_on_add(path);
    }
    crate::workspace::skill_regen::apply_new_workspace_agents_policy(
        path,
        seed_agents_md,
        fanout,
    );
    crate::cli_folder_trust::trust_cli_folder(path);
    Ok(project)
}

pub fn projects_init_git_and_open(path: &str, branch: Option<&str>) -> Result<Project, String> {
    projects_init_git_and_open_ex(path, branch, true, true, false)
}

pub fn projects_init_git_and_open_ex(
    path: &str,
    branch: Option<&str>,
    seed_wiki: bool,
    seed_agents_md: bool,
    fanout: bool,
) -> Result<Project, String> {
    let branch_name = branch
        .filter(|b| !b.trim().is_empty())
        .unwrap_or("main")
        .to_string();

    // Before the empty initial commit: if this machine has no git author,
    // seed global user.name / user.email from the OS account. Skips when
    // already configured (Rosson 2026-08-11 — fresh Windows install pain).
    crate::git_identity::ensure_git_identity()?;

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
            // ensure_git_identity should have fixed this; surface a clearer
            // message if git still rejects the author (rare policy/env case).
            return Err(
                "Git user still not configured after auto-setup. Run:\n  git config --global user.name \"Your Name\"\n  git config --global user.email \"you@example.com\""
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

    let project = with_transaction(&conn, || {
        Project::create(
            &conn, &project_id, &name, path, "#3b82f6", tab_order, 0, None, None,
        )
        .map_err(|e| e.to_string())?;
        stamp_default_agent(&conn, &project_id)?;
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
    })?;
    if seed_wiki {
        crate::wiki::seed_wiki_on_add(path);
    }
    crate::workspace::skill_regen::apply_new_workspace_agents_policy(
        path,
        seed_agents_md,
        fanout,
    );
    crate::cli_folder_trust::trust_cli_folder(path);
    Ok(project)
}

// ── Worktree enablement ────────────────────────────────────────────────

pub fn projects_enable_worktrees(project_id: &str) -> Result<Project, String> {
    let db = db::shared();
    let conn = db.lock();
    let project = Project::get(&conn, project_id).map_err(|e| e.to_string())?;

    with_transaction(&conn, || {
        Project::update(
            &conn, project_id, None, None, None, None, Some(1), None, None, None, None, None, None,
            None, None, None, None, None, None, None,
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
                None, None, None, None, None, None, None, None, None, None, None, None,
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
            None, None, None, None, None, None, None, None, None, None, None, None,
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
        None, None, None, None, None, None, None, None, None, None, None, None,
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
        None, None, None, None, None, None, None, None, None, None, None, None,
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
        // A scoped DB seeds `_orphan` + `_broadcast` (like the shared
        // one) and holds only this test's rows. Add a real workspace so
        // the user-facing surface is non-empty either way.
        let _scoped_db = db::scoped_for_test();
        let ws_path = std::env::temp_dir().join(format!("k2-projects-ops-{}", uuid::Uuid::new_v4()));
        ensure_project("real-ws-projects-ops", &ws_path.to_string_lossy(), "RealOne");

        // Both surfaces read from ONE snapshot (one lock): two separate
        // reads would see rows another thread inserted in between.
        let db_handle = db::shared();
        let conn = db_handle.lock();

        // UI-facing surface: sentinels filtered out, real workspace present.
        let visible = projects_list_on(&conn).expect("projects_list ok");
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

    fn ensure_workspace(id: &str, project_id: &str, name: &str) {
        let db = db::shared();
        let conn = db.lock();
        Workspace::create(
            &conn,
            id,
            project_id,
            None,
            "branch",
            Some("main"),
            name,
            0,
            None,
        )
        .expect("workspace create");
    }

    fn delete_workspace(id: &str) {
        let db = db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM workspaces WHERE id = ?1", rusqlite::params![id]);
    }

    /// Nested DTO for `GET /cli/projects/list`: each visible project has a
    /// `workspaces` array (empty vec still serializes as `"workspaces":[]`,
    /// never omitted). Audit sentinels stay hidden the same way
    /// `projects_list()` hides them.
    #[test]
    fn projects_list_with_workspaces_embeds_arrays_hides_sentinels() {
        let _scoped_db = db::scoped_for_test();
        ensure_project(
            "embed-ws-proj",
            "/tmp/k2-embed-ws-proj",
            "WithWs",
        );
        ensure_project(
            "embed-empty-proj",
            "/tmp/k2-embed-empty-proj",
            "NoWs",
        );
        ensure_workspace("embed-w1", "embed-ws-proj", "main");

        let listed = projects_list_with_workspaces().expect("list ok");
        let ids: Vec<&str> = listed.iter().map(|p| p.project.id.as_str()).collect();
        assert!(
            !ids.contains(&"_orphan"),
            "projects_list_with_workspaces() must not leak _orphan: {ids:?}"
        );
        assert!(
            !ids.contains(&"_broadcast"),
            "projects_list_with_workspaces() must not leak _broadcast: {ids:?}"
        );
        assert!(
            ids.contains(&"embed-ws-proj"),
            "fixture project must appear: {ids:?}"
        );
        assert!(
            ids.contains(&"embed-empty-proj"),
            "empty-workspace fixture must appear: {ids:?}"
        );

        let with_ws = listed
            .iter()
            .find(|p| p.project.id == "embed-ws-proj")
            .expect("with-ws project");
        assert_eq!(with_ws.workspaces.len(), 1);
        assert_eq!(with_ws.workspaces[0].id, "embed-w1");

        let empty = listed
            .iter()
            .find(|p| p.project.id == "embed-empty-proj")
            .expect("empty project");
        assert!(
            empty.workspaces.is_empty(),
            "project with no workspace rows must still have an empty vec"
        );

        let json = serde_json::to_value(&listed).expect("serialize");
        let arr = json.as_array().expect("json array");
        for row in arr {
            let obj = row.as_object().expect("project object");
            assert!(
                obj.contains_key("workspaces"),
                "workspaces key must always be present (including []): {obj:?}"
            );
            assert!(
                obj["workspaces"].is_array(),
                "workspaces must be a JSON array: {obj:?}"
            );
        }
        let empty_row = arr
            .iter()
            .find(|r| r["id"] == "embed-empty-proj")
            .expect("empty json row");
        assert_eq!(
            empty_row["workspaces"],
            serde_json::json!([]),
            "empty workspaces must serialize as [] not omit the key"
        );
        assert_eq!(
            empty_row["dbAgentAccess"], "off",
            "projects/list must include create-only dbAgentAccess (default off)"
        );

        delete_workspace("embed-w1");
        delete_project("embed-ws-proj");
        delete_project("embed-empty-proj");
    }

    /// 0063 stamp-on-create: a project row CREATED through the public
    /// create flow gets `default_agent` stamped with the CURRENT global
    /// `AppSettings.default_agent` — pinning the workspace to the agent
    /// it was born with (the NULL = inherit-global fallthrough is only
    /// for pre-migration rows, covered in schema.rs). HOME is pointed at
    /// a temp dir (a `test_env::TempHome`, under the ONE crate-wide env
    /// lock, taken before the scoped DB) so the asserted value is fully
    /// deterministic and the developer's real ~/.k2/settings.json is
    /// never read or written.
    #[test]
    fn projects_create_stamps_default_agent_from_global_setting() {
        // Temp HOME with a settings.json whose default_agent is a known
        // preset-id-shaped value. Restored + removed on drop (even on panic).
        let _home = crate::test_env::TempHome::new();

        let run = || -> Project {
            let stamped_preset = "aaaabbbb-1234-4abc-9def-000011112222";
            let mut s = crate::app_settings::AppSettings::default();
            s.default_agent = stamped_preset.into();
            crate::app_settings::save(&s).expect("save global settings");

            let _scoped_db = db::scoped_for_test();
            let path = format!("/tmp/k2so-stamp-test/{}", Uuid::new_v4());
            let created =
                projects_create("StampMe", &path, None).expect("projects_create ok");
            assert_eq!(
                created.default_agent.as_deref(),
                Some(stamped_preset),
                "created row must be stamped with the global default_agent"
            );

            // The stamp must survive an unrelated projects_update (None
            // leaves it untouched) and be visible through Project::get.
            let updated = projects_update(
                &created.id,
                Some("StampMe Renamed"),
                None, None, None, None, None, None, None, None, None, None, None, None,
                None, None, None,
            )
            .expect("projects_update ok");
            assert_eq!(updated.name, "StampMe Renamed");
            assert_eq!(
                updated.default_agent.as_deref(),
                Some(stamped_preset),
                "unrelated update must not clear the stamp"
            );

            // And projects_update can override + clear it (the route's
            // Some("") → Some(None) convention lands here as Some(None)).
            let overridden = projects_update(
                &created.id,
                None, None, None, None, None, None, None, None, None, None, None, None, None,
                Some(Some("claude")),
                None, None,
            )
            .expect("projects_update set override ok");
            assert_eq!(
                overridden.default_agent.as_deref(),
                Some("claude"),
                "legacy command token must be stored as given"
            );
            let cleared = projects_update(
                &created.id,
                None, None, None, None, None, None, None, None, None, None, None, None, None,
                Some(None),
                None, None,
            )
            .expect("projects_update clear ok");
            assert_eq!(
                cleared.default_agent, None,
                "Some(None) must clear back to NULL = inherit global"
            );

            let with_model = projects_update(
                &created.id,
                None, None, None, None, None, None, None, None, None, None, None, None, None, None,
                Some(Some("opus")),
                Some(1),
            )
            .expect("projects_update set default_model ok");
            assert_eq!(with_model.default_model.as_deref(), Some("opus"));
            assert_eq!(with_model.force_model_on_resume, 1);
            let cleared_model = projects_update(
                &created.id,
                None, None, None, None, None, None, None, None, None, None, None, None, None, None,
                Some(Some("")),
                Some(0),
            )
            .expect("projects_update clear default_model ok");
            assert_eq!(cleared_model.default_model, None);
            assert_eq!(cleared_model.force_model_on_resume, 0);
            created
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));

        // Cleanup before re-raising any assertion failure (HOME itself is
        // restored by the `_home` guard when this test returns or unwinds).
        let created_id = match &result {
            Ok(p) => Some(p.id.clone()),
            Err(_) => None,
        };
        if let Some(id) = created_id {
            delete_project(&id);
        }
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    fn git_scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2-proj-add-{label}-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let status = std::process::Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(&dir)
            .status()
            .expect("run git init");
        assert!(status.success(), "git init must succeed for {label}");
        dir
    }

    /// Garden data (contract-garden-data-v1 §2.1): deleting a workspace
    /// removes its grants, the grants made under its delegations and its
    /// widget-author stamps; other workspaces' rows stay.
    #[test]
    fn projects_delete_removes_garden_data_grants_and_stamps() {
        let _home = crate::test_env::TempHome::new();
        let _scoped_db = db::scoped_for_test();
        {
            let h = db::shared();
            let conn = h.lock();
            for (id, handle) in [("w-gd-builder", "gd-builder"), ("w-gd-other", "gd-other")] {
                conn.execute(
                    "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
                    rusqlite::params![id, handle, format!("/t/{handle}")],
                )
                .unwrap();
            }
            conn.execute_batch(
                "INSERT INTO zen_data_grants (id, space, prefix, subject_kind, subject_id, level, admin, via_grant, granted_by, granted_at)
                   VALUES (1, 'data:board', '', 'workspace', 'w-gd-builder', 'write', 1, NULL, 'owner', 1),
                          (2, 'data:board', 'p/', 'workspace', 'w-gd-other', 'read', 0, 1, 'agent:w-gd-builder', 1),
                          (3, 'data:board', 'q/', 'workspace', 'w-gd-other', 'read', 0, NULL, 'owner', 1);
                 INSERT INTO zen_widget_authors (widget, hash, author_kind, author_project_id, source, caller_kind, signed_at)
                   VALUES ('board', printf('%064d', 1), 'agent', 'w-gd-builder', 'sign', 'agent', 1);",
            )
            .unwrap();
        }
        projects_delete("w-gd-builder").expect("delete");
        let h = db::shared();
        let conn = h.lock();
        let ids: Vec<i64> = conn
            .prepare("SELECT id FROM zen_data_grants ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(ids, vec![3], "the delegation and what it made are gone; the person's grant stays");
        let stamps: i64 = conn.query_row("SELECT COUNT(*) FROM zen_widget_authors", [], |r| r.get(0)).unwrap();
        assert_eq!(stamps, 0);
    }

    #[test]
    fn projects_add_from_path_ex_default_plants() {
        // Add reads ~/.k2/settings.json and writes CLI folder trust under
        // $HOME: hold the env lock with an isolated HOME (env lock first,
        // then the scoped DB).
        let _home = crate::test_env::TempHome::new();
        let _scoped_db = db::scoped_for_test();
        let dir = git_scratch("default-on");
        let path = dir.to_string_lossy().into_owned();
        match projects_add_from_path_ex(&path, true, true, false) {
            Ok(AddFromPathResult::Project(_)) => {}
            Ok(AddFromPathResult::NeedsGitInit { .. }) => {
                panic!("git-init scratch must not return NeedsGitInit")
            }
            Err(e) => panic!("projects_add_from_path_ex failed: {e}"),
        }
        let canon = crate::workspace_dot_dir(&dir).join("AGENTS.md");
        assert!(canon.is_file(), "GUI add path must compose .k2/AGENTS.md");
        let root = dir.join("AGENTS.md");
        assert!(
            root.is_file(),
            "projects_add_from_path_ex default must plant cwd AGENTS.md"
        );
        let canon_body = std::fs::read_to_string(&canon).expect("read canon");
        let root_body = std::fs::read_to_string(&root).expect("read root");
        assert_eq!(canon_body, root_body);
        assert!(root_body.contains("<!-- GENERATED by K2"));
        assert!(!dir.join("CLAUDE.md").exists(), "default must not plant leftovers");
        assert!(!dir.join("GEMINI.md").exists());
        assert!(!dir.join("AGENT.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn projects_add_from_path_ex_seed_agents_md_false_does_not_plant() {
        let _home = crate::test_env::TempHome::new();
        let _scoped_db = db::scoped_for_test();
        let dir = git_scratch("generate-off");
        let path = dir.to_string_lossy().into_owned();
        match projects_add_from_path_ex(&path, true, false, false) {
            Ok(AddFromPathResult::Project(_)) => {}
            Ok(AddFromPathResult::NeedsGitInit { .. }) => {
                panic!("git-init scratch must not return NeedsGitInit")
            }
            Err(e) => panic!("projects_add_from_path_ex failed: {e}"),
        }
        assert!(
            crate::workspace_dot_dir(&dir).join("AGENTS.md").is_file(),
            ".k2/AGENTS.md still composed"
        );
        assert!(
            !dir.join("AGENTS.md").exists(),
            "seed_agents_md=false must not plant cwd AGENTS.md"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn count_projects_at(path: &str) -> i64 {
        let db = db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM projects WHERE path = ?1",
            rusqlite::params![path],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn projects_add_from_path_twice_is_c2_copy_one_row() {
        let _home = crate::test_env::TempHome::new();
        let _scoped_db = db::scoped_for_test();
        let dir = git_scratch("add-twice");
        let path = dir.to_string_lossy().into_owned();
        match projects_add_from_path_ex(&path, false, false, false) {
            Ok(AddFromPathResult::Project(_)) => {}
            Ok(AddFromPathResult::NeedsGitInit { .. }) => {
                panic!("git-init scratch must not return NeedsGitInit")
            }
            Err(e) => panic!("first add must succeed: {e}"),
        }
        crate::workspace::display::set_agent_display_name(&path, "Cortana")
            .expect("pin display name");
        let err = match projects_add_from_path_ex(&path, false, false, false) {
            Err(e) => e,
            Ok(_) => panic!("second add must refuse"),
        };
        assert_eq!(err, "workspace already exists Cortana");
        assert!(!err.contains("Failed to add workspace"));
        assert!(!err.contains("This folder is already added"));
        assert!(!err.contains(&path), "C2 must not use the raw path: {err}");
        assert_eq!(count_projects_at(&path), 1, "still one projects row");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn projects_add_without_git_twice_is_c2_copy_one_row() {
        let _home = crate::test_env::TempHome::new();
        let _scoped_db = db::scoped_for_test();
        let dir = std::env::temp_dir().join(format!(
            "k2-proj-nogit-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.to_string_lossy().into_owned();
        projects_add_without_git_ex(&path, false, false, false).expect("first add");
        crate::workspace::display::set_agent_display_name(&path, "Cortana")
            .expect("pin display name");
        let err = projects_add_without_git_ex(&path, false, false, false).unwrap_err();
        assert_eq!(err, "workspace already exists Cortana");
        assert!(!err.contains("UNIQUE constraint"));
        assert!(!err.contains("Failed to add workspace"));
        assert_eq!(count_projects_at(&path), 1, "still one projects row");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── iconRev / avatarRev ─────────────────────────────────────────────

    fn icon_project(id: &str, path: &str, icon_url: Option<&str>, color: &str) {
        let db = db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR REPLACE INTO projects \
             (id, path, name, color, agent_mode, pinned, tab_order, icon_url) \
             VALUES (?1, ?2, ?1, ?3, 'off', 0, 0, ?4)",
            rusqlite::params![id, path, color, icon_url],
        )
        .unwrap();
    }

    fn icon_temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("k2-iconrev-{tag}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// The row's `iconRev` as `GET /cli/projects/list` serializes it.
    fn listed_icon_rev(id: &str) -> String {
        let listed = projects_list_with_workspaces().expect("list ok");
        let json = serde_json::to_value(&listed).expect("serialize");
        let row = json
            .as_array()
            .expect("array")
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("row {id} missing"))
            .clone();
        row["iconRev"].as_str().expect("iconRev is a string").to_string()
    }

    fn is_rev(s: &str) -> bool {
        s.len() == ICON_REV_LEN && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    }

    #[test]
    fn icon_rev_of_is_sha256_prefix_and_empty_for_none() {
        // sha256("abc") = ba7816bf8f01cfea…
        assert_eq!(icon_rev_of(Some("abc")), "ba7816bf8f01cfea");
        assert_eq!(icon_rev_of(Some("")), "");
        assert_eq!(icon_rev_of(None), "");
    }

    /// The list's `iconRev` and the push's `avatarRev` are the same
    /// function of the same source, and both equal the rev of exactly
    /// what `/cli/projects/get-icon` returns: data URL, URL, file-backed
    /// icon, and no icon.
    #[test]
    fn icon_rev_list_push_and_get_icon_agree_for_every_source() {
        let _scoped_db = db::scoped_for_test();

        // 1. A stored data URL.
        let data_url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==";
        let d1 = icon_temp_dir("data");
        icon_project("rev-data", &d1.to_string_lossy(), Some(data_url), "#112233");
        let got = projects_get_icon(&d1.to_string_lossy(), Some("rev-data")).expect("get-icon");
        assert_eq!(got.data_url.as_deref(), Some(data_url));
        let want = icon_rev_of(Some(data_url));
        assert!(is_rev(&want), "rev shape: {want}");
        assert_eq!(listed_icon_rev("rev-data"), want);
        assert_eq!(project_push_look("rev-data"), ("#112233".to_string(), want.clone()));

        // 2. A stored http(s) URL (hashed as text, never fetched).
        let url = "https://cdn.example.com/agents/cortana.png";
        let d2 = icon_temp_dir("url");
        icon_project("rev-url", &d2.to_string_lossy(), Some(url), "#445566");
        let got = projects_get_icon(&d2.to_string_lossy(), Some("rev-url")).expect("get-icon");
        assert_eq!(got.data_url.as_deref(), Some(url));
        let want_url = icon_rev_of(Some(url));
        assert!(is_rev(&want_url));
        assert_ne!(want_url, want, "different sources, different revs");
        assert_eq!(listed_icon_rev("rev-url"), want_url);
        assert_eq!(project_push_look("rev-url").1, want_url);

        // 3. A file-backed icon (nothing stored yet; detected from the repo).
        let d3 = icon_temp_dir("file");
        std::fs::write(d3.join("favicon.svg"), "<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
        let p3 = d3.to_string_lossy().into_owned();
        icon_project("rev-file", &p3, None, "#778899");
        let detected = detect_project_icon(&p3).expect("detects favicon.svg");
        let want_file = icon_rev_of(Some(&detected));
        assert!(is_rev(&want_file));
        assert_eq!(listed_icon_rev("rev-file"), want_file, "list before get-icon caches");
        assert_eq!(project_push_look("rev-file").1, want_file, "push before get-icon caches");

        // The file changes: the rev follows (mtime/length-keyed cache).
        std::fs::write(
            d3.join("favicon.svg"),
            "<svg xmlns='http://www.w3.org/2000/svg' width='48' height='48'/>",
        )
        .unwrap();
        let detected2 = detect_project_icon(&p3).expect("still detects");
        let want_file2 = icon_rev_of(Some(&detected2));
        assert_ne!(want_file2, want_file, "a changed icon must change the rev");
        assert_eq!(listed_icon_rev("rev-file"), want_file2);
        assert_eq!(project_push_look("rev-file").1, want_file2);

        // get-icon caches the detected data URL into the row: same rev.
        let got = projects_get_icon(&p3, Some("rev-file")).expect("get-icon");
        assert_eq!(got.data_url.as_deref(), Some(detected2.as_str()));
        assert_eq!(listed_icon_rev("rev-file"), want_file2, "caching must not move the rev");
        assert_eq!(project_push_look("rev-file").1, want_file2);

        // 4. No icon anywhere.
        let d4 = icon_temp_dir("none");
        icon_project("rev-none", &d4.to_string_lossy(), None, "#aabbcc");
        let got = projects_get_icon(&d4.to_string_lossy(), Some("rev-none")).expect("get-icon");
        assert!(!got.found);
        assert_eq!(got.data_url, None);
        assert_eq!(listed_icon_rev("rev-none"), "");
        assert_eq!(project_push_look("rev-none"), ("#aabbcc".to_string(), String::new()));

        // Setting a new avatar changes the rev; clearing it falls back.
        projects_set_icon("rev-data", "data:image/png;base64,AAAA").expect("set");
        let changed = listed_icon_rev("rev-data");
        assert_ne!(changed, want);
        assert_eq!(changed, icon_rev_of(Some("data:image/png;base64,AAAA")));
        assert_eq!(project_push_look("rev-data").1, changed);
        projects_clear_icon("rev-data").expect("clear");
        assert_eq!(listed_icon_rev("rev-data"), "", "cleared, and no icon file in the repo");

        // A gone row: empty look, never a panic.
        assert_eq!(project_push_look("rev-missing"), (String::new(), String::new()));

        for d in [d1, d2, d3, d4] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
