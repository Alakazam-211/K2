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
    /// The workspace agent's display name
    /// ([`crate::workspace::display::agent_display_name`] of the row's
    /// path): the same string `GET /cli/workspace/agent-display-name`
    /// gives, so a client listing folders needs no call per folder.
    pub agent_display_name: String,
}

/// Same sentinel filter as [`projects_list`], plus a nested `workspaces`
/// array per row. Internal callers keep using the flat [`projects_list`].
///
/// A fixed number of statements however many projects there are: the
/// projects read and ONE workspaces read ([`project_rows_with_workspaces`]),
/// joined in memory. `iconRev` and the display name are computed after the
/// lock is released (they may stat or read files); the display name falls
/// back to the row's own name, never a lookup per row.
pub fn projects_list_with_workspaces() -> Result<Vec<ProjectWithWorkspaces>, String> {
    let rows = {
        let db = db::shared();
        let conn = db.lock();
        project_rows_with_workspaces(&conn)?
    };
    Ok(finish_project_rows(rows))
}

/// [`projects_list_with_workspaces`] read on a read-only connection
/// ([`db::read`]): `GET /cli/projects/list`, which writes nothing.
pub fn projects_list_with_workspaces_read() -> Result<Vec<ProjectWithWorkspaces>, String> {
    let rows = db::read(project_rows_with_workspaces)?;
    Ok(finish_project_rows(rows))
}

fn finish_project_rows(rows: Vec<(Project, Vec<Workspace>)>) -> Vec<ProjectWithWorkspaces> {
    rows
        .into_iter()
        .map(|(project, workspaces)| {
            let icon_rev = project_icon_rev(project.icon_url.as_deref(), &project.path);
            let agent_display_name =
                crate::workspace::display::agent_display_name_with_name(&project.path, &project.name);
            ProjectWithWorkspaces { project, workspaces, icon_rev, agent_display_name }
        })
        .collect()
}

/// The visible projects (sentinels dropped), each with its workspaces, from
/// two reads on `conn`: [`Project::list`] and
/// [`Workspace::list_all_by_project`].
fn project_rows_with_workspaces(conn: &rusqlite::Connection) -> Result<Vec<(Project, Vec<Workspace>)>, String> {
    let projects = Project::list(conn).map_err(|e| e.to_string())?;
    let mut by_project = Workspace::list_all_by_project(conn).map_err(|e| e.to_string())?;
    Ok(projects
        .into_iter()
        .filter(|p| !db::AUDIT_SENTINEL_IDS.contains(&p.id.as_str()))
        .map(|project| {
            let workspaces = by_project.remove(&project.id).unwrap_or_default();
            (project, workspaces)
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
    // Q8: never under the workspace's Apps (see `remove_workspace_db_only`).
    crate::deploy::store::workspace_has_apps_error(&conn, id)?;
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
    // Garden access (prd-garden-access-v1 G7, migration 0219): its list,
    // ask and made rows go; its History lines stay. With the switch never
    // on there are no rows and nothing is written.
    crate::zen::garden_access::delete_for_workspace(&conn, id, chrono::Utc::now().timestamp())
        .map_err(|e| e.to_string())?;
    Project::delete(&conn, id).map_err(|e| e.to_string())?;
    for gid in &member_groups {
        crate::agent_hooks::emit(
            crate::agent_hooks::HookEvent::ProjectGroupMembersChanged,
            serde_json::json!({ "groupId": gid }),
        );
    }
    // Sew S27-A2: the UI's remove path deletes `projects` too.
    crate::tool_hook::kick_reconcile();
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
    active_project_rows_on(&conn)
}

/// [`compute_active_project_ids`] on a read-only connection ([`db::read`]):
/// `GET /cli/projects/active`, which writes nothing.
pub fn compute_active_project_ids_read(now_ms: i64, active_window_hours: u32) -> Result<Vec<String>, String> {
    let rows = db::read(active_project_rows_on)?;
    Ok(crate::active::active_project_ids(now_ms, &rows, active_window_hours))
}

fn active_project_rows_on(conn: &rusqlite::Connection) -> Result<Vec<crate::active::ProjectRow>, String> {
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

    // While git doesn't run (a Mac without the Command Line Tools), read
    // the repo with gix instead, so Add workspace never needs the git
    // binary (0.46.4).
    let git_ready = crate::git_safe::availability().is_ready();
    let git_dir = p.join(".git");
    let is_git_repo = if git_dir.exists() {
        true
    } else if !git_ready {
        crate::git_changes::repo_key_and_root(path).is_some()
    } else {
        crate::git_safe::git(path)
            .args(["rev-parse", "--show-toplevel"])
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

    // git runs before the database lock is taken (speed pass P1): a slow
    // repo must not hold every other database caller.
    let current_branch = if git_ready {
        crate::git_safe::git(path)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    } else {
        crate::git_tools::head_branch_without_cli(path)
    }
    .unwrap_or_else(|| "main".to_string());

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
    // Trimmed and checked up front (0.46.4): a name git refuses would
    // otherwise sit deferred forever on a machine without git.
    let branch_name = crate::git_tools::valid_branch(branch.filter(|b| !b.trim().is_empty()).unwrap_or("main"))?;
    // A typo'd path is refused, with or without git (0.46.5, first-run
    // review item 14): the deferred path used to create the folder and
    // register the workspace at the typo.
    crate::git_tools::require_folder(Path::new(path))?;

    // git doesn't run yet (a Mac without Apple's Command Line Tools, a box
    // with no git): add the workspace anyway and leave the identity seed,
    // `git init` and the first commit for when it does (0.46.4,
    // `crate::git_tools`). Otherwise do them now.
    let deferred = !crate::git_safe::availability().is_ready();
    if deferred {
        crate::git_tools::defer_init(Path::new(path), &branch_name)?;
    } else {
        init_and_first_commit(path, &branch_name, false)?;
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
    })
    .inspect_err(|_| {
        if deferred {
            crate::git_tools::clear_deferred(Path::new(path));
        }
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

/// Seed the git identity if missing, `git init` on `branch_name`, and the
/// empty first commit. Add workspace runs it at once when git works; the
/// deferred step (`crate::git_tools::run_deferred`) runs it later.
///
/// `unattended` (the deferred run: no one is watching): no commit signing,
/// so a signing setup can't wait on a passphrase prompt. Hooks are already
/// off for every K2 git call (`git_safe::harden_args`).
pub(crate) fn init_and_first_commit(path: &str, branch_name: &str, unattended: bool) -> Result<(), String> {
    // Before the empty initial commit: if this machine has no git author,
    // seed global user.name / user.email from the OS account. Skips when
    // already configured (Rosson 2026-08-11 — fresh Windows install pain).
    crate::git_identity::ensure_git_identity()?;
    let no_sign: &[&str] = if unattended { &["-c", "commit.gpgsign=false"] } else { &[] };

    let init_output = crate::git_safe::git(path)
        .args(no_sign)
        .args(["init", &format!("--initial-branch={}", branch_name)])
        .output()
        .map_err(|e| format!("Failed to run git init: {}", e))?;
    if !init_output.status.success() {
        let stderr = String::from_utf8_lossy(&init_output.stderr);
        return Err(format!("Failed to initialize git: {}", stderr));
    }
    let commit_output = crate::git_safe::git(path)
        .args(no_sign)
        .args(["commit", "--allow-empty", "-m", "Initial commit"])
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
    crate::git_write_guard::mark_k2_made(std::path::Path::new(path));
    Ok(())
}

// ── Worktree enablement ────────────────────────────────────────────────

pub fn projects_enable_worktrees(project_id: &str) -> Result<Project, String> {
    // `git worktree list` runs before the lock is taken (speed pass P1).
    let project_path = db::read(|conn| Project::get(conn, project_id).map(|p| p.path)).map_err(|e| e.to_string())?;
    let worktrees = crate::git::list_worktrees(&project_path);

    let db = db::shared();
    let conn = db.lock();
    let project = Project::get(&conn, project_id).map_err(|e| e.to_string())?;
    if project.path != project_path {
        return Err(format!("workspace {project_id} moved while its worktrees were read; try again"));
    }

    with_transaction(&conn, || {
        Project::update(
            &conn, project_id, None, None, None, None, Some(1), None, None, None, None, None, None,
            None, None, None, None, None, None, None,
        )
        .map_err(|e| e.to_string())?;

        let existing_workspaces = Workspace::list(&conn, project_id).unwrap_or_default();

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

    /// The SQL statements one `projects_list_with_workspaces()` runs on a
    /// database holding `projects` workspaces of 3 rows each, counted on the
    /// connection itself (every statement SQLite starts, whoever starts it).
    /// Paths are fresh per call, so the agent display name cache is cold.
    fn list_statement_count(projects: usize) -> usize {
        list_statement_count_of(projects, projects_list_with_workspaces)
    }

    /// [`list_statement_count`] for any of the list's entry points. The
    /// route's `projects_list_with_workspaces_read` runs the same reads
    /// through `db::read`; with this scoped in-memory DB (no read pool) they
    /// run on this traced connection, so they are counted, never 0.
    fn list_statement_count_of(projects: usize, list: fn() -> Result<Vec<ProjectWithWorkspaces>, String>) -> usize {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut conn = rusqlite::Connection::open(":memory:").expect("open");
        db::run_migrations(&conn).expect("migrations");
        let run = uuid::Uuid::new_v4();
        for i in 0..projects {
            let id = format!("count-{i}");
            conn.execute(
                "INSERT INTO projects (id, path, name, color, agent_mode, pinned, tab_order) \
                 VALUES (?1, ?2, ?3, '#123456', 'off', 0, ?4)",
                rusqlite::params![id, format!("/tmp/k2-list-count-{run}-{i}"), format!("p{i}"), i as i64],
            )
            .expect("project");
            for w in 0..3 {
                Workspace::create(&conn, &format!("{id}-w{w}"), &id, None, "branch", Some("main"), "w", w, None)
                    .expect("workspace");
            }
        }
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        conn.trace_v2(
            rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
            Some(move |e: rusqlite::trace::TraceEvent<'_>| {
                if let rusqlite::trace::TraceEvent::Stmt(..) = e {
                    seen.fetch_add(1, Ordering::SeqCst);
                }
            }),
        )
        .expect("trace");
        let handle = std::sync::Arc::new(db::DbLock::new(conn));
        let listed = db::with_scoped_handle(handle, || list().expect("list ok"));
        assert_eq!(listed.len(), projects, "every seeded project is listed");
        assert!(
            listed.iter().all(|p| p.workspaces.len() == 3),
            "every project carries its 3 workspaces"
        );
        count.load(Ordering::SeqCst)
    }

    /// The projects list costs the same statements for 200 projects as for
    /// 5: the projects read (its main select and the per-switch maps) plus
    /// ONE workspaces read. It was one workspaces read per project, plus a
    /// name lookup per project whose display name cache was cold.
    #[test]
    fn projects_list_statement_count_does_not_grow_with_projects() {
        let small = list_statement_count(5);
        let large = list_statement_count(200);
        assert_eq!(small, large, "statements for 5 projects ({small}) vs 200 ({large})");
        assert!(large <= 6, "the projects list runs {large} statements; want at most 6");
    }

    /// The route's read-pool entry point runs the same fixed reads.
    #[test]
    fn projects_list_read_statement_count_does_not_grow_with_projects() {
        let before = db::read_pool::read_counts_for_test();
        let small = list_statement_count_of(5, projects_list_with_workspaces_read);
        let large = list_statement_count_of(200, projects_list_with_workspaces_read);
        let after = db::read_pool::read_counts_for_test();
        assert_eq!(after.no_pool, before.no_pool + 2, "both lists went through db::read");
        assert!(small > 0, "the reads were counted");
        assert_eq!(small, large, "statements for 5 projects ({small}) vs 200 ({large})");
        assert_eq!(large, list_statement_count(200), "the same reads as the main-connection list");
    }

    /// Workspaces keep their per-project order (by `tab_order`) when read in
    /// one pass, and a project with none still gets an empty list.
    #[test]
    fn projects_list_keeps_workspace_order_per_project() {
        let _scoped_db = db::scoped_for_test();
        ensure_project("order-a", "/tmp/k2-order-a", "A");
        ensure_project("order-b", "/tmp/k2-order-b", "B");
        ensure_project("order-empty", "/tmp/k2-order-empty", "E");
        {
            let db = db::shared();
            let conn = db.lock();
            for (id, project, order) in
                [("b-2", "order-b", 2), ("a-1", "order-a", 1), ("b-0", "order-b", 0), ("a-0", "order-a", 0)]
            {
                Workspace::create(&conn, id, project, None, "branch", Some("main"), id, order, None)
                    .expect("workspace");
            }
        }
        let listed = projects_list_with_workspaces().expect("list ok");
        let ids = |pid: &str| -> Vec<String> {
            listed
                .iter()
                .find(|p| p.project.id == pid)
                .unwrap_or_else(|| panic!("{pid} listed"))
                .workspaces
                .iter()
                .map(|w| w.id.clone())
                .collect()
        };
        assert_eq!(ids("order-a"), vec!["a-0", "a-1"]);
        assert_eq!(ids("order-b"), vec!["b-0", "b-2"]);
        assert!(ids("order-empty").is_empty());
    }

    /// Every `projects/list` row carries `agentDisplayName`: the same name
    /// `GET /cli/workspace/agent-display-name` gives for its path (the
    /// persona's `display_name:`, else the folder's registry name), so a
    /// picker needs no call per folder.
    #[test]
    fn projects_list_rows_carry_the_agent_display_name() {
        let _scoped_db = db::scoped_for_test();
        let dir = std::env::temp_dir().join(format!("k2-list-agent-name-{}", uuid::Uuid::new_v4()));
        let agent_dir = crate::workspace::agent_identity::workspace_agent_path(dir.to_str().expect("utf8"));
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        std::fs::write(
            agent_dir.join("ROLE.md"),
            "---\nname: ada-tech\ndisplay_name: Ada Lovelace\n---\n\nPersona.\n",
        )
        .expect("persona");
        let named_path = dir.to_str().expect("utf8").to_string();
        ensure_project("list-name-persona", &named_path, "ada-folder");
        ensure_project("list-name-plain", "/tmp/k2-list-name-plain-nonexistent", "plain-folder");

        let listed = projects_list_with_workspaces().expect("list ok");
        let json = serde_json::to_value(&listed).expect("serialize");
        let row = |id: &str| {
            json.as_array()
                .expect("array")
                .iter()
                .find(|r| r["id"] == id)
                .unwrap_or_else(|| panic!("{id} listed"))
                .clone()
        };
        assert_eq!(row("list-name-persona")["agentDisplayName"], "Ada Lovelace", "persona display_name wins");
        assert_eq!(row("list-name-plain")["agentDisplayName"], "plain-folder", "no persona: the folder's name");
        assert_eq!(
            row("list-name-persona")["agentDisplayName"],
            crate::workspace::display::agent_display_name(&named_path).as_str(),
            "the same name the per-folder route gives"
        );
        for r in json.as_array().expect("array") {
            assert!(r["agentDisplayName"].is_string(), "every row has a name: {r}");
            assert!(r.get("agent_display_name").is_none(), "camelCase only: {r}");
        }

        delete_project("list-name-persona");
        delete_project("list-name-plain");
        std::fs::remove_dir_all(&dir).expect("cleanup");
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
            let path = format!("/tmp/k2-stamp-test/{}", Uuid::new_v4());
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
        assert!(root_body.contains("<!-- GENERATED at "));
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

    /// 0.46.5 (first-run review item 14): "Init git" on a typo'd path is
    /// refused before anything happens: no folder, no marker, no row.
    #[test]
    fn init_git_and_open_refuses_a_missing_folder_and_creates_nothing() {
        let _home = crate::test_env::TempHome::new();
        let _scoped_db = db::scoped_for_test();
        let parent = std::env::temp_dir().join(format!("k2-proj-typo-{}-{}", std::process::id(), Uuid::new_v4()));
        std::fs::create_dir_all(&parent).expect("mkdir parent");
        let typo = parent.join("sitee");
        let path = typo.to_string_lossy().into_owned();
        let err = projects_init_git_and_open_ex(&path, Some("main"), false, false, false).unwrap_err();
        assert!(err.contains("There's no folder at"), "{err}");
        assert!(!typo.exists(), "the typo'd folder was created");
        assert_eq!(count_projects_at(&path), 0, "no workspace row at the typo");
        let _ = std::fs::remove_dir_all(&parent);
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
