//! Phase 2 Unit 4 — `/cli/*` route handlers for the SQLite-writing
//! domains that moved from Tauri (`commands/states.rs`,
//! `commands/workspaces.rs`, `commands/focus_groups.rs`,
//! `commands/workspace_sections.rs`, `commands/workspace_layouts.rs`,
//! `commands/timer.rs`, `commands/agents.rs` presets DELETE,
//! `commands/projects.rs` minus pick_folder, `window.rs` window_state,
//! and the workspace_layouts startup migration in `src-tauri/src/lib.rs`).
//!
//! Method-gate strategy: every POST route is added to the
//! `post_allowed` allowlist in `main.rs::handle_connection` and
//! dispatched by `dispatch_unit4_post`. GET routes are added to
//! `cli::dispatch` directly.
//!
//! `read_dir`, `enable_worktrees`, and the project add flows can do
//! filesystem walks + git invocations that take noticeable time on
//! large repos. The route is wrapped in `tokio::task::spawn_blocking`
//! in `main.rs` (F5) so the accept loop stays free.

use std::collections::HashMap;

use serde::Deserialize;

use crate::cli_response::CliResponse;

use k2_core::db_ops as dops;
use k2_core::projects_ops as pops;

// ── Helpers ────────────────────────────────────────────────────────────

fn ok_serialized<T: serde::Serialize>(value: T) -> CliResponse {
    match serde_json::to_string(&value) {
        Ok(s) => CliResponse::ok_json(s),
        Err(e) => CliResponse::internal_error(format!("serialize response: {e}")),
    }
}

fn unit_ok(r: Result<(), String>) -> CliResponse {
    match r {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

fn serialized<T: serde::Serialize>(r: Result<T, String>) -> CliResponse {
    match r {
        Ok(v) => ok_serialized(v),
        Err(e) => CliResponse::bad_request(e),
    }
}

fn parse_body<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, CliResponse> {
    serde_json::from_slice(body).map_err(|e| CliResponse::bad_request(format!("invalid JSON: {e}")))
}

fn str_param(params: &HashMap<String, String>, key: &str) -> String {
    params.get(key).cloned().unwrap_or_default()
}

fn opt_str(params: &HashMap<String, String>, key: &str) -> Option<String> {
    params.get(key).cloned().filter(|s| !s.is_empty())
}

fn opt_i64(params: &HashMap<String, String>, key: &str) -> Option<i64> {
    params.get(key).and_then(|v| v.parse::<i64>().ok())
}

// ══════════════════════════════════════════════════════════════════════
// GET handlers (used in cli::dispatch)
// ══════════════════════════════════════════════════════════════════════

// ── States ────────────────────────────────────────────────────────────

pub fn handle_states_list() -> CliResponse {
    serialized(dops::states_list())
}

// ── Workspaces ────────────────────────────────────────────────────────

pub fn handle_workspaces_list(params: &HashMap<String, String>) -> CliResponse {
    let project_id = str_param(params, "project_id");
    if project_id.is_empty() {
        return CliResponse::bad_request("Missing 'project_id' parameter");
    }
    serialized(dops::workspaces_list(&project_id))
}

// ── Focus groups ──────────────────────────────────────────────────────

pub fn handle_focus_groups_list() -> CliResponse {
    serialized(dops::focus_groups_list())
}

// ── Sections ──────────────────────────────────────────────────────────

pub fn handle_sections_list(params: &HashMap<String, String>) -> CliResponse {
    let project_id = str_param(params, "project_id");
    if project_id.is_empty() {
        return CliResponse::bad_request("Missing 'project_id' parameter");
    }
    serialized(dops::sections_list(&project_id))
}

// ── Workspace layouts ─────────────────────────────────────────────────

pub fn handle_layout_load(params: &HashMap<String, String>) -> CliResponse {
    let project_id = str_param(params, "project_id");
    let workspace_id = str_param(params, "workspace_id");
    if project_id.is_empty() || workspace_id.is_empty() {
        return CliResponse::bad_request("Missing 'project_id'/'workspace_id' parameter");
    }
    serialized(dops::workspace_layout_load(&project_id, &workspace_id))
}

pub fn handle_layout_load_all() -> CliResponse {
    serialized(dops::workspace_layout_load_all())
}

// ── Tab titles (0.39.39 #676, daemon-canonical) ────────────────────────

/// `GET /cli/workspace/tab-titles?project=<path>` — all daemon-canonical
/// tab titles for a workspace. The renderer hydrates its tab labels from
/// this on workspace load instead of from local layout state. Accepts a
/// project PATH (resolved to project_id); also accepts `project_id`
/// directly for callers that already hold the id.
pub fn handle_tab_titles_list(params: &HashMap<String, String>) -> CliResponse {
    let project_id = match resolve_project_id_param(params) {
        Ok(pid) => pid,
        Err(r) => return r,
    };
    serialized(dops::tab_titles_for_project(&project_id))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetTabTitleBody {
    /// Project PATH (the contract field the renderer sends). Resolved to
    /// project_id below. `project_id` is accepted as an alias.
    #[serde(default)]
    project: Option<String>,
    #[serde(default, rename = "projectId")]
    project_id: Option<String>,
    tab_id: String,
    title: String,
    /// Sticky-rename flag (0053). A user's explicit rename sends
    /// `locked: true` so program-generated PTY titles can't overwrite
    /// it. Defaults to false for callers that don't set it.
    #[serde(default)]
    locked: bool,
}

/// `POST /cli/workspace/set-tab-title { project, tabId, title }` (#676).
/// Upserts the daemon-canonical tab title + broadcasts `TabTitleChanged`
/// so a rename in window A shows up in window B + the mobile companion.
/// `project` is the workspace PATH; the handler resolves it to the
/// project_id the storage is keyed by.
pub fn handle_set_tab_title(body: &[u8]) -> CliResponse {
    let b: SetTabTitleBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if b.tab_id.is_empty() {
        return CliResponse::bad_request("Missing 'tabId'");
    }
    // Resolve project_id from either the path (`project`) or an explicit
    // `projectId`. Capture the project PATH too for the event's
    // workspace-scoped filter field.
    let (project_id, project_path) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(pid) = b.project_id.as_deref().filter(|s| !s.is_empty()) {
            let path = k2_core::db::schema::Project::get(&conn, pid)
                .ok()
                .map(|p| p.path)
                .unwrap_or_default();
            (pid.to_string(), path)
        } else if let Some(path) = b.project.as_deref().filter(|s| !s.is_empty()) {
            match k2_core::workspace::agent_identity::resolve_project_id(&conn, path) {
                Some(pid) => (pid, path.to_string()),
                None => {
                    return CliResponse::bad_request(format!(
                        "project not registered: {path}"
                    ))
                }
            }
        } else {
            return CliResponse::bad_request("Missing 'project' (or 'projectId')");
        }
    };

    match dops::tab_title_set(&project_id, &b.tab_id, &b.title, b.locked) {
        Ok(()) => {
            let _ = crate::session_events::emit(
                crate::session_events::SessionEvent::TabTitleChanged {
                    workspace_path: project_path,
                    project: project_id.clone(),
                    tab_id: b.tab_id.clone(),
                    title: b.title.clone(),
                },
            );
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Resolve a project_id from query params accepting either `project_id`
/// or a project `project`/`project_path` PATH.
fn resolve_project_id_param(params: &HashMap<String, String>) -> Result<String, CliResponse> {
    if let Some(pid) = params.get("project_id").filter(|s| !s.is_empty()) {
        return Ok(pid.clone());
    }
    let path = params
        .get("project")
        .or_else(|| params.get("project_path"))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CliResponse::bad_request("Missing 'project' or 'project_id'"))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace::agent_identity::resolve_project_id(&conn, path)
        .ok_or_else(|| CliResponse::bad_request(format!("project not registered: {path}")))
}

// ── Timer ─────────────────────────────────────────────────────────────

pub fn handle_timer_entries_list(params: &HashMap<String, String>) -> CliResponse {
    let start = opt_i64(params, "start");
    let end = opt_i64(params, "end");
    let project_id = opt_str(params, "project_id");
    serialized(dops::timer_entries_list(
        start,
        end,
        project_id.as_deref(),
    ))
}

pub fn handle_timer_entries_export(params: &HashMap<String, String>) -> CliResponse {
    let format = params
        .get("format")
        .cloned()
        .unwrap_or_else(|| "json".to_string());
    let start = opt_i64(params, "start");
    let end = opt_i64(params, "end");
    let project_id = opt_str(params, "project_id");
    match dops::timer_entries_export(&format, start, end, project_id.as_deref()) {
        Ok(s) => CliResponse::ok_text(s),
        Err(e) => CliResponse::bad_request(e),
    }
}

// ── Presets ───────────────────────────────────────────────────────────

pub fn handle_presets_list() -> CliResponse {
    serialized(dops::presets_list())
}

// ── Window state ──────────────────────────────────────────────────────

pub fn handle_window_state_get() -> CliResponse {
    serialized(dops::window_state_get())
}

// ── Projects ──────────────────────────────────────────────────────────

pub fn handle_projects_list() -> CliResponse {
    serialized(pops::projects_list())
}

pub fn handle_projects_get_icon(params: &HashMap<String, String>) -> CliResponse {
    let path = str_param(params, "path");
    if path.is_empty() {
        return CliResponse::bad_request("Missing 'path' parameter");
    }
    let project_id = opt_str(params, "project_id");
    serialized(pops::projects_get_icon(&path, project_id.as_deref()))
}

pub fn handle_projects_get_editors() -> CliResponse {
    ok_serialized(k2_core::editors::get_installed_editors())
}

pub fn handle_projects_get_all_editors() -> CliResponse {
    ok_serialized(k2_core::editors::get_all_editors())
}

pub fn handle_projects_refresh_editors() -> CliResponse {
    ok_serialized(k2_core::editors::clear_editor_cache())
}

// ══════════════════════════════════════════════════════════════════════
// POST handlers (dispatched by `dispatch_unit4_post` in main.rs)
// ══════════════════════════════════════════════════════════════════════

// ── States ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatesCreateBody {
    name: String,
    description: Option<String>,
    cap_features: String,
    cap_issues: String,
    cap_crashes: String,
    cap_security: String,
    cap_audits: String,
    heartbeat: bool,
}

pub fn handle_states_create(body: &[u8]) -> CliResponse {
    let b: StatesCreateBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(dops::states_create(
        &b.name,
        b.description.as_deref(),
        &b.cap_features,
        &b.cap_issues,
        &b.cap_crashes,
        &b.cap_security,
        &b.cap_audits,
        b.heartbeat,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatesUpdateBody {
    id: String,
    name: Option<String>,
    description: Option<String>,
    cap_features: Option<String>,
    cap_issues: Option<String>,
    cap_crashes: Option<String>,
    cap_security: Option<String>,
    cap_audits: Option<String>,
    heartbeat: Option<bool>,
}

pub fn handle_states_update(body: &[u8]) -> CliResponse {
    let b: StatesUpdateBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(dops::states_update(
        &b.id,
        b.name.as_deref(),
        b.description.as_deref(),
        b.cap_features.as_deref(),
        b.cap_issues.as_deref(),
        b.cap_crashes.as_deref(),
        b.cap_security.as_deref(),
        b.cap_audits.as_deref(),
        b.heartbeat,
    ))
}

#[derive(Deserialize)]
struct IdBody {
    id: String,
}

pub fn handle_states_delete(body: &[u8]) -> CliResponse {
    let b: IdBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(dops::states_delete(&b.id))
}

// ── Workspaces ────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspacesCreateBody {
    project_id: String,
    name: String,
    #[serde(rename = "type")]
    type_: Option<String>,
    branch: Option<String>,
    worktree_path: Option<String>,
}

pub fn handle_workspaces_create(body: &[u8]) -> CliResponse {
    let b: WorkspacesCreateBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(dops::workspaces_create(
        &b.project_id,
        &b.name,
        b.type_.as_deref(),
        b.branch.as_deref(),
        b.worktree_path.as_deref(),
    ))
}

pub fn handle_workspaces_delete(body: &[u8]) -> CliResponse {
    let b: IdBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(dops::workspaces_delete(&b.id))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceSetNavBody {
    id: String,
    visible: bool,
}

pub fn handle_workspace_set_nav_visible(body: &[u8]) -> CliResponse {
    let b: WorkspaceSetNavBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(dops::workspace_set_nav_visible(&b.id, b.visible))
}

// ── Focus groups ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct FgCreateBody {
    name: String,
    color: Option<String>,
}

pub fn handle_focus_groups_create(body: &[u8]) -> CliResponse {
    let b: FgCreateBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(dops::focus_groups_create(&b.name, b.color.as_deref()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FgUpdateBody {
    id: String,
    name: Option<String>,
    color: Option<String>,
    tab_order: Option<i64>,
}

pub fn handle_focus_groups_update(body: &[u8]) -> CliResponse {
    let b: FgUpdateBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(dops::focus_groups_update(
        &b.id,
        b.name.as_deref(),
        b.color.as_deref(),
        b.tab_order,
    ))
}

pub fn handle_focus_groups_delete(body: &[u8]) -> CliResponse {
    let b: IdBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(dops::focus_groups_delete(&b.id))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FgAssignBody {
    project_id: String,
    focus_group_id: Option<String>,
}

pub fn handle_focus_groups_assign(body: &[u8]) -> CliResponse {
    let b: FgAssignBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(dops::focus_groups_assign_project(
        &b.project_id,
        b.focus_group_id.as_deref(),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FgReconcileBody {
    project_id: String,
}

pub fn handle_focus_groups_reconcile(body: &[u8]) -> CliResponse {
    let b: FgReconcileBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(dops::focus_groups_reconcile_project(&b.project_id))
}

// ── Sections ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SectionsCreateBody {
    project_id: String,
    name: String,
    color: Option<String>,
}

pub fn handle_sections_create(body: &[u8]) -> CliResponse {
    let b: SectionsCreateBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(dops::sections_create(
        &b.project_id,
        &b.name,
        b.color.as_deref(),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SectionsUpdateBody {
    id: String,
    name: Option<String>,
    color: Option<String>,
    is_collapsed: Option<i64>,
    tab_order: Option<i64>,
}

pub fn handle_sections_update(body: &[u8]) -> CliResponse {
    let b: SectionsUpdateBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(dops::sections_update(
        &b.id,
        b.name.as_deref(),
        b.color.as_deref(),
        b.is_collapsed,
        b.tab_order,
    ))
}

pub fn handle_sections_delete(body: &[u8]) -> CliResponse {
    let b: IdBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(dops::sections_delete(&b.id))
}

#[derive(Deserialize)]
struct ReorderBody {
    ids: Vec<String>,
}

pub fn handle_sections_reorder(body: &[u8]) -> CliResponse {
    let b: ReorderBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(dops::sections_reorder(&b.ids))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SectionsAssignBody {
    workspace_id: String,
    section_id: Option<String>,
}

pub fn handle_sections_assign(body: &[u8]) -> CliResponse {
    let b: SectionsAssignBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(dops::sections_assign_workspace(
        &b.workspace_id,
        b.section_id.as_deref(),
    ))
}

// ── Workspace layouts ─────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayoutSaveBody {
    project_id: String,
    workspace_id: String,
    layout_json: String,
}

pub fn handle_layout_save(body: &[u8]) -> CliResponse {
    let b: LayoutSaveBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    // 0.39.39 (#677.3) — revision-aware save: the daemon stamps a
    // monotonic revision so concurrent tab-order writes from multiple
    // clients resolve last-write-wins deterministically. The new
    // revision rides back in the response (additive — old clients
    // ignore it) and out on the `TabOrderChanged` broadcast so every
    // OTHER client can drop a stale local write whose base is behind.
    match dops::workspace_layout_save_with_revision(
        &b.project_id,
        &b.workspace_id,
        &b.layout_json,
    ) {
        Ok(revision) => {
            // Resolve the project PATH for the workspace-scoped event
            // filter. Best-effort: if the project row is gone the event
            // still carries project_id/workspace_id (which the renderer
            // can match on directly) with an empty workspacePath.
            let workspace_path = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::db::schema::Project::get(&conn, &b.project_id)
                    .ok()
                    .map(|p| p.path)
                    .unwrap_or_default()
            };
            let _ = crate::session_events::emit(
                crate::session_events::SessionEvent::TabOrderChanged {
                    workspace_path,
                    project: b.project_id.clone(),
                    workspace: b.workspace_id.clone(),
                    revision,
                },
            );
            CliResponse::ok_json(
                serde_json::json!({ "success": true, "revision": revision }).to_string(),
            )
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayoutDeleteBody {
    project_id: String,
    workspace_id: Option<String>,
}

pub fn handle_layout_delete(body: &[u8]) -> CliResponse {
    let b: LayoutDeleteBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(dops::workspace_layout_delete(
        &b.project_id,
        b.workspace_id.as_deref(),
    ))
}

// ── Timer ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimerCreateBody {
    id: String,
    project_id: Option<String>,
    start_time: i64,
    end_time: i64,
    duration_seconds: i64,
    memo: Option<String>,
}

pub fn handle_timer_create(body: &[u8]) -> CliResponse {
    let b: TimerCreateBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(dops::timer_entry_create(
        &b.id,
        b.project_id.as_deref(),
        b.start_time,
        b.end_time,
        b.duration_seconds,
        b.memo.as_deref(),
    ))
}

pub fn handle_timer_delete(body: &[u8]) -> CliResponse {
    let b: IdBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(dops::timer_entry_delete(&b.id))
}

// ── Presets ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct PresetsCreateBody {
    label: String,
    command: String,
    icon: Option<String>,
}

pub fn handle_presets_create(body: &[u8]) -> CliResponse {
    let b: PresetsCreateBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(dops::presets_create(&b.label, &b.command, b.icon.as_deref()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresetsUpdateBody {
    id: String,
    label: Option<String>,
    command: Option<String>,
    icon: Option<String>,
    enabled: Option<i64>,
    sort_order: Option<i64>,
}

pub fn handle_presets_update(body: &[u8]) -> CliResponse {
    let b: PresetsUpdateBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(dops::presets_update(
        &b.id,
        b.label.as_deref(),
        b.command.as_deref(),
        b.icon.as_ref().map(|i| Some(i.as_str())),
        b.enabled,
        b.sort_order,
    ))
}

pub fn handle_presets_delete(body: &[u8]) -> CliResponse {
    let b: IdBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(dops::presets_delete(&b.id))
}

pub fn handle_presets_reorder(body: &[u8]) -> CliResponse {
    let b: ReorderBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(dops::presets_reorder(&b.ids))
}

pub fn handle_presets_reset_built_ins(_body: &[u8]) -> CliResponse {
    serialized(dops::presets_reset_built_ins())
}

// ── Window state ──────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowStateSetBody {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    is_maximized: bool,
    /// When true, only the `is_maximized` flag is touched in the DB —
    /// the position/size fields are left untouched. Used by Tauri's
    /// save-on-close path when the window is currently maximized so
    /// the last windowed geometry is preserved.
    #[serde(default)]
    only_maximized_flag: bool,
}

pub fn handle_window_state_set(body: &[u8]) -> CliResponse {
    let b: WindowStateSetBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let state = dops::WindowState {
        x: b.x,
        y: b.y,
        width: b.width,
        height: b.height,
        is_maximized: b.is_maximized,
    };
    unit_ok(dops::window_state_set(&state, b.only_maximized_flag))
}

// ── Projects: writes ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct ProjectsCreateBody {
    name: String,
    path: String,
    color: Option<String>,
}

/// 0.39.45 (GH #18/#26): broadcast that the registered project set
/// changed so every client — other windows, the CLI's next caller, K2
/// Connect remotes — re-fetches the project list without a manual
/// window reload. APP-LEVEL on the session-events bus.
fn emit_projects_changed() {
    let _ = crate::session_events::emit(
        crate::session_events::SessionEvent::ProjectsChanged {},
    );
}

pub fn handle_projects_create(body: &[u8]) -> CliResponse {
    let b: ProjectsCreateBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let result = pops::projects_create(&b.name, &b.path, b.color.as_deref());
    if result.is_ok() {
        emit_projects_changed();
    }
    serialized(result)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectsUpdateBody {
    id: String,
    name: Option<String>,
    color: Option<String>,
    tab_order: Option<i64>,
    worktree_mode: Option<i64>,
    pinned: Option<i64>,
    manually_active: Option<i64>,
    /// Some("data:...") sets it, Some("") clears it, None leaves unchanged.
    icon_url: Option<String>,
    agent_enabled: Option<i64>,
    heartbeat_enabled: Option<i64>,
    agent_mode: Option<String>,
    state_id: Option<String>,
    heartbeat_mode: Option<String>,
    heartbeat_schedule: Option<String>,
}

pub fn handle_projects_update(body: &[u8]) -> CliResponse {
    let b: ProjectsUpdateBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let icon_param = b.icon_url.as_ref().map(|u| {
        if u.is_empty() {
            None
        } else {
            Some(u.as_str())
        }
    });
    let state_param = b.state_id.as_ref().map(|t| {
        if t.is_empty() {
            None
        } else {
            Some(t.as_str())
        }
    });
    let hb_schedule_param = b.heartbeat_schedule.as_ref().map(|s| {
        if s.is_empty() {
            None
        } else {
            Some(s.as_str())
        }
    });
    serialized(pops::projects_update(
        &b.id,
        b.name.as_deref(),
        b.color.as_deref(),
        b.tab_order,
        b.worktree_mode,
        b.pinned,
        b.manually_active,
        icon_param,
        b.agent_enabled,
        b.heartbeat_enabled,
        b.agent_mode,
        state_param,
        b.heartbeat_mode,
        hb_schedule_param,
    ))
}

pub fn handle_projects_delete(body: &[u8]) -> CliResponse {
    let b: IdBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let result = pops::projects_delete(&b.id);
    if result.is_ok() {
        emit_projects_changed();
    }
    unit_ok(result)
}

pub fn handle_projects_reorder(body: &[u8]) -> CliResponse {
    let b: ReorderBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(pops::projects_reorder(&b.ids))
}

// ── Canonical Active set routes (task #672) ────────────────────────────

/// Current unix time in milliseconds (the renderer's native clock).
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ActiveSnapshot {
    project_ids: Vec<String>,
    active_window_hours: u32,
}

/// `GET /cli/projects/active` → `{ projectIds, activeWindowHours }`.
/// Canonical Active-set snapshot used by the renderer on connect /
/// host-switch. Reads `active_window_hours` from app_settings.
pub fn handle_projects_active() -> CliResponse {
    let window = k2_core::app_settings::load().active_window_hours;
    match pops::compute_active_project_ids(now_ms(), window) {
        Ok(ids) => ok_serialized(ActiveSnapshot {
            project_ids: ids,
            active_window_hours: window,
        }),
        Err(e) => CliResponse::internal_error(format!("compute active set: {e}")),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivateBody {
    project_id: String,
}

/// `POST /cli/projects/activate { projectId }` → bump
/// `last_interaction_at` (the workspace was opened/focused), recompute
/// the canonical Active set, broadcast `ActiveChanged`. `{ "ok": true }`.
pub fn handle_projects_activate(body: &[u8]) -> CliResponse {
    let b: ActivateBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(e) = pops::projects_touch_interaction(&b.project_id) {
        return CliResponse::bad_request(e);
    }
    crate::active_reaper::recompute_and_broadcast_active();
    CliResponse::ok_json(r#"{"ok":true}"#.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PinBody {
    project_id: String,
    pinned: bool,
}

/// `POST /cli/projects/pin { projectId, pinned }` → set
/// `manually_active` = pinned, recompute, broadcast. `{ "ok": true }`.
pub fn handle_projects_pin(body: &[u8]) -> CliResponse {
    let b: PinBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(e) = pops::projects_set_manually_active(&b.project_id, b.pinned) {
        return CliResponse::bad_request(e);
    }
    crate::active_reaper::recompute_and_broadcast_active();
    CliResponse::ok_json(r#"{"ok":true}"#.to_string())
}

/// `POST /cli/projects/dismiss { projectId }` → clear `manually_active`
/// (if set) AND arm the reaper's grace for this workspace NOW (don't
/// wait for the interaction window to expire), then recompute +
/// broadcast. `{ "ok": true }`.
pub fn handle_projects_dismiss(body: &[u8]) -> CliResponse {
    let b: ActivateBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(e) = pops::projects_set_manually_active(&b.project_id, false) {
        return CliResponse::bad_request(e);
    }
    // Arm the daemon reaper's grace immediately (PRD §4.2) — the chat is
    // reap-eligible now, gated only by `!heartbeat` + the 15s grace +
    // the fire-time re-check; re-activating within the grace cancels it.
    crate::active_reaper::arm_dismiss_grace(&b.project_id);
    crate::active_reaper::recompute_and_broadcast_active();
    CliResponse::ok_json(r#"{"ok":true}"#.to_string())
}

pub fn handle_projects_touch_interaction(body: &[u8]) -> CliResponse {
    let b: IdBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(pops::projects_touch_interaction(&b.id))
}

pub fn handle_projects_touch_interaction_clear(body: &[u8]) -> CliResponse {
    let b: IdBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(pops::projects_touch_interaction_clear(&b.id))
}

#[derive(Deserialize)]
struct PathBody {
    path: String,
}

pub fn handle_projects_add_from_path(body: &[u8]) -> CliResponse {
    let b: PathBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let result = pops::projects_add_from_path(&b.path);
    if result.is_ok() {
        emit_projects_changed();
    }
    serialized(result)
}

pub fn handle_projects_add_without_git(body: &[u8]) -> CliResponse {
    let b: PathBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let result = pops::projects_add_without_git(&b.path);
    if result.is_ok() {
        emit_projects_changed();
    }
    serialized(result)
}

#[derive(Deserialize)]
struct InitGitBody {
    path: String,
    branch: Option<String>,
}

pub fn handle_projects_init_git_and_open(body: &[u8]) -> CliResponse {
    let b: InitGitBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let result = pops::projects_init_git_and_open(&b.path, b.branch.as_deref());
    if result.is_ok() {
        emit_projects_changed();
    }
    serialized(result)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectIdBody {
    project_id: String,
}

pub fn handle_projects_enable_worktrees(body: &[u8]) -> CliResponse {
    let b: ProjectIdBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(pops::projects_enable_worktrees(&b.project_id))
}

pub fn handle_projects_detect_icon(body: &[u8]) -> CliResponse {
    let b: ProjectIdBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(pops::projects_detect_icon(&b.project_id))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectsSetIconBody {
    project_id: String,
    data_url: String,
}

pub fn handle_projects_set_icon(body: &[u8]) -> CliResponse {
    let b: ProjectsSetIconBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(pops::projects_set_icon(&b.project_id, &b.data_url))
}

pub fn handle_projects_clear_icon(body: &[u8]) -> CliResponse {
    let b: ProjectIdBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(pops::projects_clear_icon(&b.project_id))
}

#[derive(Deserialize)]
struct PathOpBody {
    path: String,
}

pub fn handle_projects_open_in_finder(body: &[u8]) -> CliResponse {
    let b: PathOpBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let r = std::process::Command::new("open")
        .arg("-R")
        .arg(&b.path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Failed to open Finder: {}", e));
    unit_ok(r)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenEditorBody {
    editor_id: String,
    path: String,
}

pub fn handle_projects_open_in_editor(body: &[u8]) -> CliResponse {
    let b: OpenEditorBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(k2_core::editors::open_in_editor(&b.editor_id, &b.path))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenTerminalBody {
    terminal_app: String,
    path: String,
}

pub fn handle_projects_open_in_terminal(body: &[u8]) -> CliResponse {
    let b: OpenTerminalBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(k2_core::editors::open_in_terminal(
        &b.terminal_app,
        &b.path,
    ))
}

// ══════════════════════════════════════════════════════════════════════
// POST dispatch — single match arm called from main.rs
// ══════════════════════════════════════════════════════════════════════

/// Phase 2 Unit 4 POST dispatch. Unknown paths return 404.
pub fn dispatch_unit4_post(path: &str, body: &[u8]) -> CliResponse {
    match path {
        // States
        "/cli/states/create" => handle_states_create(body),
        "/cli/states/update" => handle_states_update(body),
        "/cli/states/delete" => handle_states_delete(body),

        // Workspaces
        "/cli/workspaces/create" => handle_workspaces_create(body),
        "/cli/workspaces/delete" => handle_workspaces_delete(body),
        "/cli/workspaces/set-nav-visible" => handle_workspace_set_nav_visible(body),

        // Focus groups
        "/cli/focus-groups/create" => handle_focus_groups_create(body),
        "/cli/focus-groups/update" => handle_focus_groups_update(body),
        "/cli/focus-groups/delete" => handle_focus_groups_delete(body),
        "/cli/focus-groups/assign" => handle_focus_groups_assign(body),
        "/cli/focus-groups/reconcile" => handle_focus_groups_reconcile(body),

        // Sections
        "/cli/sections/create" => handle_sections_create(body),
        "/cli/sections/update" => handle_sections_update(body),
        "/cli/sections/delete" => handle_sections_delete(body),
        "/cli/sections/reorder" => handle_sections_reorder(body),
        "/cli/sections/assign" => handle_sections_assign(body),

        // Workspace layouts
        "/cli/workspace-layouts/save" => handle_layout_save(body),
        "/cli/workspace-layouts/delete" => handle_layout_delete(body),

        // Timer
        "/cli/timer/create" => handle_timer_create(body),
        "/cli/timer/delete" => handle_timer_delete(body),

        // Presets
        "/cli/presets/create" => handle_presets_create(body),
        "/cli/presets/update" => handle_presets_update(body),
        "/cli/presets/delete" => handle_presets_delete(body),
        "/cli/presets/reorder" => handle_presets_reorder(body),
        "/cli/presets/reset" => handle_presets_reset_built_ins(body),

        // Window state
        "/cli/window-state/set" => handle_window_state_set(body),

        // Projects
        "/cli/projects/create" => handle_projects_create(body),
        "/cli/projects/update" => handle_projects_update(body),
        "/cli/projects/delete" => handle_projects_delete(body),
        "/cli/projects/reorder" => handle_projects_reorder(body),
        "/cli/projects/touch-interaction" => handle_projects_touch_interaction(body),
        "/cli/projects/touch-interaction-clear" => handle_projects_touch_interaction_clear(body),
        // task #672 — canonical Active mutating routes.
        "/cli/projects/activate" => handle_projects_activate(body),
        "/cli/projects/pin" => handle_projects_pin(body),
        "/cli/projects/dismiss" => handle_projects_dismiss(body),
        "/cli/projects/add-from-path" => handle_projects_add_from_path(body),
        "/cli/projects/add-without-git" => handle_projects_add_without_git(body),
        "/cli/projects/init-git-and-open" => handle_projects_init_git_and_open(body),
        "/cli/projects/enable-worktrees" => handle_projects_enable_worktrees(body),
        "/cli/projects/detect-icon" => handle_projects_detect_icon(body),
        "/cli/projects/set-icon" => handle_projects_set_icon(body),
        "/cli/projects/clear-icon" => handle_projects_clear_icon(body),
        "/cli/projects/open-in-finder" => handle_projects_open_in_finder(body),
        "/cli/projects/open-in-editor" => handle_projects_open_in_editor(body),
        "/cli/projects/open-in-terminal" => handle_projects_open_in_terminal(body),
        "/cli/projects/refresh-editors" => {
            ok_serialized(k2_core::editors::clear_editor_cache())
        }

        _ => CliResponse::not_found(),
    }
}

#[cfg(test)]
mod set_tab_title_tests {
    //! 0.39.39 (#676) — `POST /cli/workspace/set-tab-title` persists the
    //! daemon-canonical title AND broadcasts `TabTitleChanged`.

    use super::*;
    use crate::session_events::{self, SessionEvent};
    use parking_lot::Mutex as PLMutex;

    static TEST_LOCK: PLMutex<()> = PLMutex::new(());

    fn unique(suffix: &str) -> String {
        format!(
            "stt-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            suffix
        )
    }

    fn seed_project(project_id: &str, path: &str) {
        let dbh = k2_core::db::shared();
        let conn = dbh.lock();
        k2_core::db::schema::Project::create(
            &conn, project_id, "Test", path, "#fff", 0, 0, None, None,
        )
        .expect("seed project");
    }

    #[test]
    fn set_tab_title_persists_and_emits() {
        let _g = TEST_LOCK.lock();
        let project_id = unique("p");
        let path = format!("/tmp/{project_id}");
        seed_project(&project_id, &path);

        // Subscribe BEFORE the write so we capture the emit.
        let mut rx = session_events::subscribe();

        let body = serde_json::json!({
            "project": path,
            "tabId": "tab-xyz",
            "title": "Hello Tab",
        })
        .to_string();
        let resp = handle_set_tab_title(body.as_bytes());
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        assert!(resp.body.contains("\"success\":true"));

        // Persisted under the resolved project_id.
        let titles = dops::tab_titles_for_project(&project_id).expect("list");
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].tab_id, "tab-xyz");
        assert_eq!(titles[0].title, "Hello Tab");

        // Broadcast carried the exact contract. The bus is process-
        // global, so drain until we find our probe (by tabId).
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            assert!(std::time::Instant::now() < deadline, "no TabTitleChanged seen");
            match rx.try_recv() {
                Ok(SessionEvent::TabTitleChanged {
                    workspace_path,
                    project,
                    tab_id,
                    title,
                }) if tab_id == "tab-xyz" => {
                    assert_eq!(workspace_path, path);
                    assert_eq!(project, project_id);
                    assert_eq!(title, "Hello Tab");
                    break;
                }
                Ok(_) => continue,           // contamination from another test
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(e) => panic!("recv error: {e:?}"),
            }
        }
    }

    #[test]
    fn set_tab_title_rejects_unregistered_project() {
        let _g = TEST_LOCK.lock();
        let body = serde_json::json!({
            "project": "/tmp/definitely-not-registered-xyz",
            "tabId": "t",
            "title": "x",
        })
        .to_string();
        let resp = handle_set_tab_title(body.as_bytes());
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
        assert!(resp.body.contains("not registered"));
    }

    #[test]
    fn set_tab_title_rejects_missing_tab_id() {
        let _g = TEST_LOCK.lock();
        let project_id = unique("p");
        let path = format!("/tmp/{project_id}");
        seed_project(&project_id, &path);
        let body = serde_json::json!({ "project": path, "tabId": "", "title": "x" })
            .to_string();
        let resp = handle_set_tab_title(body.as_bytes());
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
        assert!(resp.body.contains("tabId"));
    }
}
