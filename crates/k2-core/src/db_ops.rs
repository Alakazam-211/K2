//! Phase 2 Unit 4 — daemon-side operations for the SQLite domains
//! previously written by Tauri commands.
//!
//! Each `pub fn` in this module is the canonical implementation of a
//! state-mutating (or non-trivial query) operation against
//! `k2_core::db::shared()`. The daemon's `/cli/*` route handlers
//! (and the Tauri commands during the transition) call these.
//!
//! Conventions
//! - All returns are `Result<T, String>` so the caller can hand the
//!   error straight to a JSON `{"error": "..."}` body.
//! - Inputs use `Option<T>` for "leave unchanged on update" semantics
//!   that match the pre-Unit-4 Tauri command signatures.
//! - No file I/O outside the DB unless explicitly noted (project_config
//!   reads / git scans).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db;
use crate::db::schema::{
    AgentPreset, FocusGroup, Project, TimeEntry, Workspace, WorkspaceSection,
};
use crate::project_config;

// ── Built-in agent presets ──────────────────────────────────────────────
//
// Lives here (instead of the daemon-side route module) because it's
// pure data — both the Tauri shim and the daemon route call the same
// `presets_reset_built_ins` here. Originally lived in
// `src-tauri/src/commands/agents.rs`; moved verbatim so the IDs +
// order still match `db::seed_agent_presets` exactly.
const BUILT_IN_PRESETS: &[(&str, &str, &str, &str, i64)] = &[
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456001", "Claude", "claude --dangerously-skip-permissions", "", 0),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456002", "Codex", "codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox", "", 1),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456014", "Grok", "grok --always-approve", "", 2),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456003", "Gemini", "gemini --yolo", "", 3),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456006", "Cursor Agent", "cursor-agent", "", 4),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456012", "Pi", "pi", "", 5),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456013", "Hermes", "hermes", "", 6),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456007", "OpenCode", "opencode", "", 7),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456011", "Goose", "goose", "", 8),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456005", "Aider", "aider", "", 9),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456009", "Ollama", "ollama run llama3.2", "", 10),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456004", "Copilot", "copilot --allow-all", "", 11),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456010", "Interpreter", "interpreter", "", 12),
];

// Workspace States ops retired with the product feature. Schema layer
// (`WorkspaceState` + `workspace_states` table) retained for migrations.

// ── Workspaces ─────────────────────────────────────────────────────────

pub fn workspaces_list(project_id: &str) -> Result<Vec<Workspace>, String> {
    let db = db::shared();
    let conn = db.lock();
    Workspace::list(&conn, project_id).map_err(|e| e.to_string())
}

pub fn workspaces_create(
    project_id: &str,
    name: &str,
    type_: Option<&str>,
    branch: Option<&str>,
    worktree_path: Option<&str>,
) -> Result<Workspace, String> {
    let db = db::shared();
    let conn = db.lock();
    let id = Uuid::new_v4().to_string();
    let type_val = type_.unwrap_or("branch");

    let existing = Workspace::list(&conn, project_id).unwrap_or_default();
    let max_order = existing.iter().map(|w| w.tab_order).max().unwrap_or(-1) + 1;

    Workspace::create(
        &conn,
        &id,
        project_id,
        None,
        type_val,
        branch,
        name,
        max_order,
        worktree_path,
    )
    .map_err(|e| e.to_string())?;

    Workspace::get(&conn, &id).map_err(|e| e.to_string())
}

pub fn workspaces_delete(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    Workspace::delete(&conn, id).map_err(|e| e.to_string())
}

pub fn workspace_set_nav_visible(id: &str, visible: bool) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE workspaces SET nav_visible = ?1 WHERE id = ?2",
        rusqlite::params![if visible { 1 } else { 0 }, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Focus groups ───────────────────────────────────────────────────────

pub fn focus_groups_list() -> Result<Vec<FocusGroup>, String> {
    let db = db::shared();
    let conn = db.lock();
    FocusGroup::list(&conn).map_err(|e| e.to_string())
}

pub fn focus_groups_create(name: &str, color: Option<&str>) -> Result<FocusGroup, String> {
    let db = db::shared();
    let conn = db.lock();
    let id = Uuid::new_v4().to_string();

    let existing = FocusGroup::list(&conn).unwrap_or_default();
    let max_order = existing.iter().map(|g| g.tab_order).max().unwrap_or(-1) + 1;

    FocusGroup::create(&conn, &id, name, color, max_order).map_err(|e| e.to_string())?;
    FocusGroup::get(&conn, &id).map_err(|e| e.to_string())
}

pub fn focus_groups_update(
    id: &str,
    name: Option<&str>,
    color: Option<&str>,
    tab_order: Option<i64>,
) -> Result<FocusGroup, String> {
    let db = db::shared();
    let conn = db.lock();
    FocusGroup::update(&conn, id, name, color, tab_order).map_err(|e| e.to_string())?;
    FocusGroup::get(&conn, id).map_err(|e| e.to_string())
}

pub fn focus_groups_delete(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    FocusGroup::delete(&conn, id).map_err(|e| e.to_string())
}

pub fn focus_groups_assign_project(
    project_id: &str,
    focus_group_id: Option<&str>,
) -> Result<Project, String> {
    let db = db::shared();
    let conn = db.lock();

    Project::update(
        &conn,
        project_id,
        None, None, None, None, None, None,
        Some(focus_group_id),
        None, None, None, None, None, None, None, None, None,
    )
    .map_err(|e| e.to_string())?;

    // Write the focus group name to .k2so/config.json
    let project = Project::get(&conn, project_id).map_err(|e| e.to_string())?;
    let group_name = match focus_group_id {
        Some(gid) => FocusGroup::get(&conn, gid).ok().map(|g| g.name),
        None => None,
    };

    project_config::set_project_config_value(
        &project.path,
        "focusGroupName",
        group_name.as_deref(),
    )
    .ok();

    Project::get(&conn, project_id).map_err(|e| e.to_string())
}

pub fn focus_groups_reconcile_project(project_id: &str) -> Result<Project, String> {
    let db = db::shared();
    let conn = db.lock();
    let project = Project::get(&conn, project_id).map_err(|e| e.to_string())?;
    let config = project_config::get_project_config(&project.path);
    let config_group_name = config.focus_group_name;

    match config_group_name {
        None => {
            if project.focus_group_id.is_some() {
                Project::update(
                    &conn,
                    project_id,
                    None, None, None, None, None, None,
                    Some(None),
                    None, None, None, None, None, None, None, None, None,
                )
                .map_err(|e| e.to_string())?;
            }
        }
        Some(ref group_name) => {
            let groups = FocusGroup::list(&conn).map_err(|e| e.to_string())?;
            let existing = groups.iter().find(|g| &g.name == group_name);

            let group_id = if let Some(g) = existing {
                g.id.clone()
            } else {
                let new_id = Uuid::new_v4().to_string();
                let max_order = groups.iter().map(|g| g.tab_order).max().unwrap_or(-1) + 1;
                FocusGroup::create(&conn, &new_id, group_name, None, max_order)
                    .map_err(|e| e.to_string())?;
                new_id
            };

            if project.focus_group_id.as_deref() != Some(&group_id) {
                Project::update(
                    &conn,
                    project_id,
                    None, None, None, None, None, None,
                    Some(Some(group_id.as_str())),
                    None, None, None, None, None, None, None, None, None,
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }

    Project::get(&conn, project_id).map_err(|e| e.to_string())
}

// ── Workspace sections ─────────────────────────────────────────────────

pub fn sections_list(project_id: &str) -> Result<Vec<WorkspaceSection>, String> {
    let db = db::shared();
    let conn = db.lock();
    WorkspaceSection::list(&conn, project_id).map_err(|e| e.to_string())
}

pub fn sections_create(
    project_id: &str,
    name: &str,
    color: Option<&str>,
) -> Result<WorkspaceSection, String> {
    let db = db::shared();
    let conn = db.lock();
    let id = Uuid::new_v4().to_string();
    let existing = WorkspaceSection::list(&conn, project_id).unwrap_or_default();
    let max_order = existing.iter().map(|s| s.tab_order).max().unwrap_or(-1) + 1;

    WorkspaceSection::create(&conn, &id, project_id, name, color, max_order)
        .map_err(|e| e.to_string())?;
    WorkspaceSection::get(&conn, &id).map_err(|e| e.to_string())
}

pub fn sections_update(
    id: &str,
    name: Option<&str>,
    color: Option<&str>,
    is_collapsed: Option<i64>,
    tab_order: Option<i64>,
) -> Result<WorkspaceSection, String> {
    let db = db::shared();
    let conn = db.lock();
    WorkspaceSection::update(&conn, id, name, color, is_collapsed, tab_order)
        .map_err(|e| e.to_string())?;
    WorkspaceSection::get(&conn, id).map_err(|e| e.to_string())
}

pub fn sections_delete(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    WorkspaceSection::delete(&conn, id).map_err(|e| e.to_string())
}

pub fn sections_reorder(ids: &[String]) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    for (i, id) in ids.iter().enumerate() {
        WorkspaceSection::update(&conn, id, None, None, None, Some(i as i64))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn sections_assign_workspace(
    workspace_id: &str,
    section_id: Option<&str>,
) -> Result<Workspace, String> {
    let db = db::shared();
    let conn = db.lock();
    Workspace::update(
        &conn,
        workspace_id,
        Some(section_id),
        None,
        None,
        None,
        None,
        None,
    )
    .map_err(|e| e.to_string())?;
    Workspace::get(&conn, workspace_id).map_err(|e| e.to_string())
}

// ── Workspace layouts ─────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLayout {
    pub project_id: String,
    pub workspace_id: String,
    pub layout_json: String,
}

pub fn workspace_layout_save(
    project_id: &str,
    workspace_id: &str,
    layout_json: &str,
) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    let id = format!("{}:{}", project_id, workspace_id);

    conn.execute(
        "INSERT INTO workspace_layouts (id, project_id, workspace_id, layout_json, updated_at)
         VALUES (?1, ?2, ?3, ?4, unixepoch())
         ON CONFLICT(project_id, workspace_id)
         DO UPDATE SET layout_json = excluded.layout_json, updated_at = unixepoch()",
        rusqlite::params![id, project_id, workspace_id, layout_json],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// 0.39.39 (#677.3) — save a workspace layout AND bump its monotonic
/// `revision`, returning the NEW revision. The revision is the
/// deterministic last-write-wins token concurrent clients use to drop
/// stale tab-order writes (`updated_at` is second-granular and collides
/// under burst writes; an explicit incrementing integer never does).
///
/// On INSERT the row starts at revision 1; on UPDATE the stored
/// revision is incremented by 1. The new value is read back inside the
/// same locked connection so callers always observe their own write's
/// revision (no read-after-write race).
pub fn workspace_layout_save_with_revision(
    project_id: &str,
    workspace_id: &str,
    layout_json: &str,
) -> Result<i64, String> {
    // 0.39.45 (#27): heal pinned-tab identity BEFORE persisting. The
    // renderer's workspace-switch race can stamp a SIBLING workspace's
    // agentName/projectPath into the system-agent tab; pre-0.39.45 the
    // daemon stored that layout verbatim, making the corruption the
    // SSOT (the renderer-side reconcile heals only its own memory).
    // NOTE: must run before taking the DB lock — the identity resolver
    // takes its own scoped locks.
    let healed = heal_system_agent_tab_identity(project_id, layout_json);
    let layout_json = healed.as_deref().unwrap_or(layout_json);
    // 2026-07-02: prune leaked bare-terminal tabs at save time too, so
    // a client still holding a poisoned in-memory layout (the pre-
    // b339c70 re-mint loop's output) can't re-persist hundreds of dead
    // tabs over the healed row. Same scoped-lock rule as above.
    let pruned = prune_leaked_bare_tabs(project_id, layout_json);
    let layout_json = pruned.as_deref().unwrap_or(layout_json);

    let db = db::shared();
    let conn = db.lock();
    let id = format!("{}:{}", project_id, workspace_id);

    conn.execute(
        "INSERT INTO workspace_layouts (id, project_id, workspace_id, layout_json, updated_at, revision)
         VALUES (?1, ?2, ?3, ?4, unixepoch(), 1)
         ON CONFLICT(project_id, workspace_id)
         DO UPDATE SET layout_json = excluded.layout_json,
                       updated_at = unixepoch(),
                       revision = workspace_layouts.revision + 1",
        rusqlite::params![id, project_id, workspace_id, layout_json],
    )
    .map_err(|e| e.to_string())?;

    let revision: i64 = conn
        .query_row(
            "SELECT revision FROM workspace_layouts WHERE project_id = ?1 AND workspace_id = ?2",
            rusqlite::params![project_id, workspace_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    Ok(revision)
}

pub fn workspace_layout_load(
    project_id: &str,
    workspace_id: &str,
) -> Result<Option<String>, String> {
    let result = {
        let db = db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT layout_json FROM workspace_layouts WHERE project_id = ?1 AND workspace_id = ?2",
            rusqlite::params![project_id, workspace_id],
            |row| row.get::<_, String>(0),
        )
    };
    match result {
        // 0.39.45 (#27): read-repair — serve a healed view of layouts
        // whose pinned tab carries a foreign workspace's identity
        // (rows corrupted before the save-side heal existed). Not
        // persisted here (a read must not bump the revision); the next
        // save persists the healed form. 2026-07-02: compose the
        // leaked-bare-tab prune the same way — rows poisoned by the
        // pre-b339c70 tab re-mint loop otherwise make every workspace
        // restore O(hundreds of dead panes).
        Ok(json) => {
            let json = heal_system_agent_tab_identity(project_id, &json).unwrap_or(json);
            let json = prune_leaked_bare_tabs(project_id, &json).unwrap_or(json);
            Ok(Some(json))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// 0.39.45 (GH #27) — heal the system-agent (pinned Chat/Inbox) tab's
/// identity inside a layout JSON against the OWNING project row.
///
/// The renderer serializes pinned agent items with `agentName` +
/// `projectPath`; a workspace-switch race (#9, recurring on the
/// daemon-owned path as #27) can stamp a SIBLING workspace's identity
/// into them. The daemon knows the truth — `project_id` keys the
/// layout row — so any system-agent item whose `projectPath` differs
/// from the owning project's path is rewritten in place (and its
/// `sessionId` dropped: that session belonged to the wrong workspace).
/// `agentName` is re-stamped from the same resolver the tab headers
/// use ([`crate::workspace::display::agent_display_name`]).
///
/// Returns `Some(healed_json)` when anything changed; `None` when the
/// layout is already correct, unparseable (the caller keeps the
/// original bytes — healing must never eat a layout), or the project
/// row is gone.
pub fn heal_system_agent_tab_identity(project_id: &str, layout_json: &str) -> Option<String> {
    // Owning identity — scoped lock, dropped before agent_display_name
    // (which takes its own DB lock on the fallback path).
    let path: String = {
        let db = db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT path FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |r| r.get::<_, String>(0),
        )
        .ok()?
    };
    if path.is_empty() {
        return None;
    }
    let agent_name = crate::workspace::display::agent_display_name(&path);

    let mut layout: serde_json::Value = serde_json::from_str(layout_json).ok()?;
    let mut changed = false;
    heal_tabs_array(layout.get_mut("tabs"), &agent_name, &path, &mut changed);
    if let Some(groups) = layout.get_mut("extraGroups").and_then(|v| v.as_array_mut()) {
        for g in groups {
            heal_tabs_array(g.get_mut("tabs"), &agent_name, &path, &mut changed);
        }
    }
    if changed {
        crate::log_debug!(
            "[layout-heal] #27 healed system-agent tab identity for project={} → name='{}' path='{}'",
            project_id,
            agent_name,
            path
        );
        serde_json::to_string(&layout).ok()
    } else {
        None
    }
}

/// Walk one `tabs` array, fixing agent items inside tabs flagged
/// `isSystemAgent`. Non-system tabs are user content — a user may
/// legitimately pin another workspace's agent there — so they are
/// deliberately left alone.
fn heal_tabs_array(
    tabs: Option<&mut serde_json::Value>,
    agent_name: &str,
    path: &str,
    changed: &mut bool,
) {
    let Some(tabs) = tabs.and_then(|v| v.as_array_mut()) else {
        return;
    };
    for tab in tabs {
        if tab.get("isSystemAgent").and_then(|v| v.as_bool()) != Some(true) {
            continue;
        }
        let Some(pgs) = tab.get_mut("paneGroups").and_then(|v| v.as_object_mut()) else {
            continue;
        };
        for pg in pgs.values_mut() {
            let Some(items) = pg.get_mut("items").and_then(|v| v.as_array_mut()) else {
                continue;
            };
            for item in items {
                if item.get("type").and_then(|v| v.as_str()) != Some("agent") {
                    continue;
                }
                let path_wrong = item
                    .get("projectPath")
                    .and_then(|v| v.as_str())
                    .map(|p| p != path)
                    .unwrap_or(true);
                if path_wrong {
                    item["projectPath"] = serde_json::Value::String(path.to_string());
                    // The stored chat sessionId belonged to the wrong
                    // workspace — resuming it would re-bind the foreign
                    // conversation. Drop it; the resume resolver
                    // re-derives the right one (GH #24 machinery).
                    if let Some(obj) = item.as_object_mut() {
                        obj.remove("sessionId");
                    }
                    *changed = true;
                }
                let name_wrong = item
                    .get("agentName")
                    .and_then(|v| v.as_str())
                    .map(|n| n != agent_name)
                    .unwrap_or(true);
                if name_wrong {
                    item["agentName"] = serde_json::Value::String(agent_name.to_string());
                    *changed = true;
                }
            }
        }
    }
}

/// How many prunable bare-terminal tabs a layout may plausibly hold
/// because a HUMAN opened them. Above this, the count itself is the
/// tell: the re-mint leak produced identical session-less "Terminal N"
/// tabs by the hundreds, humans open a handful — so a pathological
/// layout is pruned to ZERO prunable tabs, not capped.
///
/// Why 16, and why it must NOT be `K2_V2_BARE_TAB_CAP` (32):
///   - the first version of this heal kept the newest `cap` (32) bare
///     tabs, so a once-healed poisoned row now sits at EXACTLY 32
///     all-bare tabs — still 32 doomed spawn POSTs (each refused by
///     the spawn cap) on every workspace entry, the 3-4s
///     workspace-switch hang. The threshold must sit strictly BELOW
///     the spawn cap so those rows read as pathological on the next
///     load and finish healing.
///   - generous headroom above real usage: dev-box layouts never held
///     more than single-digit genuine unnamed bare shells (renamed
///     tabs are `locked` and protected regardless).
///   - the cost asymmetry favors pruning: a prunable tab is
///     never-attached AND session-less, so restoring it yields a
///     brand-new empty shell — pruning one loses nothing but a tab
///     stub (one Cmd+T to recreate), while KEEPING debris costs
///     O(count) refused spawn round-trips on EVERY entry. And a LIVE
///     bare shell can never be lost at all: the daemon reconcile
///     re-adopts any live `tab-*` PTY missing from the layout.
fn leaked_bare_tab_plausible_max() -> usize {
    std::env::var("K2_BARE_TAB_PRUNE_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16)
}

/// 2026-07-02 leaked-tab layout heal — the persistence-side companion
/// to v2_spawn's bare-tab cap.
///
/// The split-pane restore re-mint loop (client bug b339c70; shipped
/// broken since 0.39.39, so released clients still carry it) appended a
/// fresh "Terminal N" tab to the saved layout every layout-echo cycle.
/// One workspace on the dev box accumulated 488 tabs / 450 dead bare
/// shells; the spawn cap stopped the PTY exhaustion, but the POISONED
/// LAYOUT persisted — and every workspace-view remount (leaving
/// Settings, workspace switch, boot) mounted all ~450 panes and fired
/// ~450 doomed spawn POSTs. That is the "leaving Settings hangs"
/// regression: the exit path is O(saved tabs).
///
/// Scope is deliberately the leak's exact shape, mirroring the spawn
/// cap's predicate. A tab is PRUNABLE only when ALL hold:
///   - not `isSystemAgent`, not `isPinnedFile`, not `locked`
///     (a user rename is a statement the tab matters);
///   - every pane item is a plain `terminal` with none of the
///     heartbeat / surfaced / attach / sandbox markers;
///   - no pane group has a RESUMABLE `workspace_tab_sessions` row
///     (`session_id IS NOT NULL`) — those restore real CLI sessions.
///
/// Policy (2026-07-03, the workspace-switch latency fix): the prunable
/// COUNT decides. At or under [`leaked_bare_tab_plausible_max`] the
/// tabs are plausibly a human's open shells and the layout round-trips
/// untouched. Above it the layout is leak-poisoned and ALL prunable
/// tabs drop — keeping a "newest N" remnant (the first version of this
/// heal capped at the spawn cap, 32) just left N doomed spawn POSTs on
/// every workspace entry. `None` = nothing to prune, plausible count,
/// or unparseable (healing must never eat a layout).
pub fn prune_leaked_bare_tabs(project_id: &str, layout_json: &str) -> Option<String> {
    let mut layout: serde_json::Value = serde_json::from_str(layout_json).ok()?;

    // Pane groups with a resumable saved session — their tabs restore
    // real work (claude --resume …) and are never pruned. One query,
    // scoped lock (same idiom as the identity heal above).
    let resumable: std::collections::HashSet<String> = {
        let db = db::shared();
        let conn = db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT pane_group_id FROM workspace_tab_sessions \
                 WHERE project_id = ?1 AND session_id IS NOT NULL",
            )
            .ok()?;
        let rows = stmt
            .query_map(rusqlite::params![project_id], |r| r.get::<_, String>(0))
            .ok()?;
        rows.filter_map(|r| r.ok()).collect()
    };

    // Is this tab the leak's shape? (See the doc comment's predicate.)
    let is_prunable = |tab: &serde_json::Value| -> bool {
        for flag in ["isSystemAgent", "isPinnedFile", "locked"] {
            if tab.get(flag).and_then(|v| v.as_bool()) == Some(true) {
                return false;
            }
        }
        let Some(pgs) = tab.get("paneGroups").and_then(|v| v.as_object()) else {
            return false;
        };
        if pgs.is_empty() {
            return false;
        }
        if pgs.keys().any(|pg_id| resumable.contains(pg_id)) {
            return false;
        }
        pgs.values().all(|pg| {
            pg.get("items")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items.iter().all(|item| {
                        item.get("type").and_then(|v| v.as_str()) == Some("terminal")
                            && ["heartbeatName", "surfacedAgentName", "attachAgentName", "sandbox"]
                                .iter()
                                .all(|k| item.get(*k).map_or(true, |v| v.is_null()))
                    })
                })
                .unwrap_or(false)
        })
    };

    // Count prunable tabs across the whole layout (main strip +
    // extraGroups); the count is what classifies the layout.
    let count_in = |tabs: Option<&serde_json::Value>| -> usize {
        tabs.and_then(|v| v.as_array())
            .map(|a| a.iter().filter(|t| is_prunable(t)).count())
            .unwrap_or(0)
    };
    let mut total_prunable = count_in(layout.get("tabs"));
    if let Some(groups) = layout.get("extraGroups").and_then(|v| v.as_array()) {
        for g in groups {
            total_prunable += count_in(g.get("tabs"));
        }
    }
    let plausible = leaked_bare_tab_plausible_max();
    if total_prunable <= plausible {
        return None;
    }

    // Pathological — drop EVERY prunable tab. Protected / marked /
    // resumable tabs pass `is_prunable == false` and always survive.
    let drop_in = |tabs: Option<&mut serde_json::Value>| {
        let Some(tabs) = tabs.and_then(|v| v.as_array_mut()) else {
            return;
        };
        tabs.retain(|t| !is_prunable(t));
    };
    drop_in(layout.get_mut("tabs"));
    if let Some(groups) = layout.get_mut("extraGroups").and_then(|v| v.as_array_mut()) {
        for g in groups {
            drop_in(g.get_mut("tabs"));
        }
    }

    crate::log_debug!(
        "[layout-heal] pruned ALL {} leaked bare-terminal tab(s) for project={} (count exceeded plausible max {})",
        total_prunable,
        project_id,
        plausible
    );
    serde_json::to_string(&layout).ok()
}

pub fn workspace_layout_load_all() -> Result<Vec<WorkspaceLayout>, String> {
    let db = db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare("SELECT project_id, workspace_id, layout_json FROM workspace_layouts")
        .map_err(|e| e.to_string())?;
    let layouts = stmt
        .query_map([], |row| {
            Ok(WorkspaceLayout {
                project_id: row.get(0)?,
                workspace_id: row.get(1)?,
                layout_json: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(layouts)
}

pub fn workspace_layout_delete(
    project_id: &str,
    workspace_id: Option<&str>,
) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    if let Some(ws_id) = workspace_id {
        conn.execute(
            "DELETE FROM workspace_layouts WHERE project_id = ?1 AND workspace_id = ?2",
            rusqlite::params![project_id, ws_id],
        )
        .map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "DELETE FROM workspace_layouts WHERE project_id = ?1",
            rusqlite::params![project_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ── Tab titles (0.39.39 #676, daemon-canonical) ────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabTitle {
    pub project_id: String,
    pub tab_id: String,
    pub title: String,
    /// When true, the user explicitly renamed this tab — program-
    /// generated PTY titles must NOT overwrite it. Stored as INTEGER
    /// 0/1 (0053).
    pub locked: bool,
}

/// Upsert a daemon-canonical tab title keyed by (project_id, tab_id).
/// Replaces the renderer-local-only `setTabTitle`. The route layer
/// broadcasts `TabTitleChanged` after this returns so other clients
/// converge.
pub fn tab_title_set(
    project_id: &str,
    tab_id: &str,
    title: &str,
    locked: bool,
) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO tab_titles (project_id, tab_id, title, locked, updated_at)
         VALUES (?1, ?2, ?3, ?4, unixepoch())
         ON CONFLICT(project_id, tab_id)
         DO UPDATE SET title = excluded.title, locked = excluded.locked, updated_at = unixepoch()",
        rusqlite::params![project_id, tab_id, title, if locked { 1_i64 } else { 0_i64 }],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// All tab titles for a project. The renderer hydrates its tab labels
/// from this on workspace load instead of from local layout state.
pub fn tab_titles_for_project(project_id: &str) -> Result<Vec<TabTitle>, String> {
    let db = db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT project_id, tab_id, title, locked FROM tab_titles WHERE project_id = ?1 \
             ORDER BY tab_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![project_id], |row| {
            Ok(TabTitle {
                project_id: row.get(0)?,
                tab_id: row.get(1)?,
                title: row.get(2)?,
                locked: row.get::<_, i64>(3)? != 0,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

// ── Time entries (timer) ───────────────────────────────────────────────

pub fn timer_entries_list(
    start: Option<i64>,
    end: Option<i64>,
    project_id: Option<&str>,
) -> Result<Vec<TimeEntry>, String> {
    let db = db::shared();
    let conn = db.lock();
    TimeEntry::list(&conn, start, end, project_id).map_err(|e| e.to_string())
}

pub fn timer_entry_create(
    id: &str,
    project_id: Option<&str>,
    start_time: i64,
    end_time: i64,
    duration_seconds: i64,
    memo: Option<&str>,
) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    TimeEntry::create(
        &conn,
        id,
        project_id,
        start_time,
        end_time,
        duration_seconds,
        memo,
    )
    .map_err(|e| e.to_string())
}

pub fn timer_entry_delete(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    TimeEntry::delete(&conn, id).map_err(|e| e.to_string())
}

pub fn timer_entries_export(
    format: &str,
    start: Option<i64>,
    end: Option<i64>,
    project_id: Option<&str>,
) -> Result<String, String> {
    let entries = timer_entries_list(start, end, project_id)?;
    match format {
        "csv" => {
            let mut csv =
                String::from("id,project_id,start_time,end_time,duration_seconds,memo,created_at\n");
            for e in &entries {
                csv.push_str(&format!(
                    "{},{},{},{},{},{},{}\n",
                    e.id,
                    e.project_id.as_deref().unwrap_or(""),
                    e.start_time,
                    e.end_time,
                    e.duration_seconds,
                    csv_escape(e.memo.as_deref().unwrap_or("")),
                    e.created_at,
                ));
            }
            Ok(csv)
        }
        _ => serde_json::to_string_pretty(&entries).map_err(|e| e.to_string()),
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// ── Agent presets ──────────────────────────────────────────────────────

pub fn presets_list() -> Result<Vec<AgentPreset>, String> {
    let db = db::shared();
    let conn = db.lock();
    AgentPreset::list(&conn).map_err(|e| e.to_string())
}

/// One preset by exact id. `Ok(None)` = no such row (the route layer
/// turns that into its uniform 404), `Err` = real DB failure.
pub fn presets_get(id: &str) -> Result<Option<AgentPreset>, String> {
    let db = db::shared();
    let conn = db.lock();
    match AgentPreset::get(&conn, id) {
        Ok(p) => Ok(Some(p)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Custom preset ids are caller-visible slugs (`k2 preset add --id
/// my-agent`): 1–64 chars of `[A-Za-z0-9._-]`, starting alphanumeric.
/// Built-in ids (UUIDs) predate this and are never re-validated.
fn validate_custom_preset_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err("preset id must be 1–64 characters".to_string());
    }
    let mut chars = id.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return Err("preset id must start with a letter or digit".to_string());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(
            "preset id may only contain letters, digits, '.', '_' and '-'".to_string(),
        );
    }
    Ok(())
}

/// Validate the migration-0070 metadata WRITE grammar. Read-side
/// consumers tolerate malformed rows (fail-closed), but a write must be
/// rejected loudly — we never store metadata we know is garbage.
///
/// - `danger_flags`: JSON array of non-empty strings.
/// - `env`: JSON string→string object; keys non-empty, no `=`/NUL.
/// - `readiness`: `bracketed-paste` | `settle:<ms>` (1..=600000 ms) —
///   the `provider_resume::InjectionProfile` vocabulary.
fn validate_preset_metadata(
    danger_flags: Option<&str>,
    env: Option<&str>,
    readiness: Option<&str>,
) -> Result<(), String> {
    if let Some(raw) = danger_flags {
        let flags: Vec<String> = serde_json::from_str(raw)
            .map_err(|e| format!("danger_flags must be a JSON array of strings: {e}"))?;
        if flags.iter().any(|f| f.trim().is_empty()) {
            return Err("danger_flags entries must be non-empty".to_string());
        }
    }
    if let Some(raw) = env {
        let map: std::collections::BTreeMap<String, String> = serde_json::from_str(raw)
            .map_err(|e| format!("env must be a JSON object of string values: {e}"))?;
        for key in map.keys() {
            if key.is_empty() || key.contains('=') || key.contains('\0') {
                return Err(format!("invalid env variable name {key:?}"));
            }
        }
    }
    if let Some(r) = readiness {
        let valid = r == "bracketed-paste"
            || r
                .strip_prefix("settle:")
                .and_then(|ms| ms.parse::<u64>().ok())
                .is_some_and(|ms| (1..=600_000).contains(&ms));
        if !valid {
            return Err(format!(
                "readiness must be 'bracketed-paste' or 'settle:<ms>' (1..=600000), got {r:?}"
            ));
        }
    }
    Ok(())
}

pub fn presets_create(
    label: &str,
    command: &str,
    icon: Option<&str>,
) -> Result<AgentPreset, String> {
    presets_create_full(None, label, command, icon, None, None, None)
}

/// Full create — W6 (0.40.30). `id` None = mint a UUID (the Settings-UI
/// path); Some(slug) = caller-chosen id (`k2 preset add --id <slug>`),
/// validated + uniqueness-checked. Metadata columns are validated by
/// [`validate_preset_metadata`]. Always creates a CUSTOM (non-built-in)
/// enabled preset appended at the end of the sort order.
pub fn presets_create_full(
    id: Option<&str>,
    label: &str,
    command: &str,
    icon: Option<&str>,
    danger_flags: Option<&str>,
    env: Option<&str>,
    readiness: Option<&str>,
) -> Result<AgentPreset, String> {
    if label.trim().is_empty() {
        return Err("preset label must not be empty".to_string());
    }
    if command.trim().is_empty() {
        return Err("preset command must not be empty".to_string());
    }
    if let Some(slug) = id {
        validate_custom_preset_id(slug)?;
    }
    validate_preset_metadata(danger_flags, env, readiness)?;

    let db = db::shared();
    let conn = db.lock();
    let id = match id {
        Some(slug) => {
            match AgentPreset::get(&conn, slug) {
                Ok(_) => return Err(format!("preset id '{slug}' already exists")),
                Err(rusqlite::Error::QueryReturnedNoRows) => {}
                Err(e) => return Err(e.to_string()),
            }
            slug.to_string()
        }
        None => Uuid::new_v4().to_string(),
    };
    let existing = AgentPreset::list(&conn).unwrap_or_default();
    let max_order = existing.iter().map(|p| p.sort_order).max().unwrap_or(-1) + 1;

    AgentPreset::create(&conn, &id, label, command, icon, 1, max_order, 0)
        .map_err(|e| e.to_string())?;
    AgentPreset::update_metadata(
        &conn,
        &id,
        Some(danger_flags),
        Some(env),
        Some(readiness),
    )
    .map_err(|e| e.to_string())?;
    AgentPreset::get(&conn, &id).map_err(|e| e.to_string())
}

pub fn presets_update(
    id: &str,
    label: Option<&str>,
    command: Option<&str>,
    icon: Option<Option<&str>>,
    enabled: Option<i64>,
    sort_order: Option<i64>,
) -> Result<AgentPreset, String> {
    presets_update_full(id, label, command, icon, enabled, sort_order, None, None, None)
}

/// Full update — W6 (0.40.30). Metadata params: outer `None` = leave
/// unchanged, inner `None` = clear to NULL (legacy/unknown; consumers
/// fail closed). Metadata IS editable on built-ins — declaring a
/// built-in's flags/env/readiness is exactly what the metadata columns
/// are for; only DELETE is built-in-guarded (`presets_delete`).
#[allow(clippy::too_many_arguments)]
pub fn presets_update_full(
    id: &str,
    label: Option<&str>,
    command: Option<&str>,
    icon: Option<Option<&str>>,
    enabled: Option<i64>,
    sort_order: Option<i64>,
    danger_flags: Option<Option<&str>>,
    env: Option<Option<&str>>,
    readiness: Option<Option<&str>>,
) -> Result<AgentPreset, String> {
    validate_preset_metadata(
        danger_flags.flatten(),
        env.flatten(),
        readiness.flatten(),
    )?;
    if matches!(label, Some(l) if l.trim().is_empty()) {
        return Err("preset label must not be empty".to_string());
    }
    if matches!(command, Some(c) if c.trim().is_empty()) {
        return Err("preset command must not be empty".to_string());
    }

    let db = db::shared();
    let conn = db.lock();
    // Existence check first so an unknown id is a clean error (the raw
    // column UPDATEs would silently no-op on 0 rows).
    match AgentPreset::get(&conn, id) {
        Ok(_) => {}
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            return Err(format!("no preset with id '{id}'"))
        }
        Err(e) => return Err(e.to_string()),
    }
    AgentPreset::update(&conn, id, label, command, icon, enabled, sort_order)
        .map_err(|e| e.to_string())?;
    AgentPreset::update_metadata(&conn, id, danger_flags, env, readiness)
        .map_err(|e| e.to_string())?;
    AgentPreset::get(&conn, id).map_err(|e| e.to_string())
}

pub fn presets_delete(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    let preset = AgentPreset::get(&conn, id).map_err(|e| e.to_string())?;
    if preset.is_built_in != 0 {
        return Err("Cannot delete built-in presets. Disable them instead.".to_string());
    }
    AgentPreset::delete(&conn, id).map_err(|e| e.to_string())
}

pub fn presets_reorder(ids: &[String]) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    for (i, id) in ids.iter().enumerate() {
        AgentPreset::update(&conn, id, None, None, None, None, Some(i as i64))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn presets_reset_built_ins() -> Result<Vec<AgentPreset>, String> {
    let db = db::shared();
    let conn = db.lock();

    conn.execute("DELETE FROM agent_presets WHERE is_built_in = 1", [])
        .map_err(|e| e.to_string())?;

    for (id, label, command, icon, sort_order) in BUILT_IN_PRESETS {
        AgentPreset::create(&conn, id, label, command, Some(icon), 1, *sort_order, 1)
            .map_err(|e| e.to_string())?;
    }
    // Freshly recreated rows carry NULL migration-0070 metadata
    // (danger_flags/env/readiness); restore the truthful built-in values
    // via the same label-keyed backfill the boot seed uses.
    db::backfill_built_in_preset_metadata(&conn).map_err(|e| e.to_string())?;
    AgentPreset::list(&conn).map_err(|e| e.to_string())
}

// ── Window state ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub is_maximized: bool,
}

pub fn window_state_get() -> Result<Option<WindowState>, String> {
    let db = db::shared();
    let conn = db.lock();
    let r = conn.query_row(
        "SELECT x, y, width, height, is_maximized FROM window_state WHERE id = 1",
        [],
        |row| {
            Ok(WindowState {
                x: row.get(0)?,
                y: row.get(1)?,
                width: row.get(2)?,
                height: row.get(3)?,
                is_maximized: row.get::<_, i32>(4)? != 0,
            })
        },
    );
    match r {
        Ok(s) => Ok(Some(s)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn window_state_set(state: &WindowState, only_maximized_flag: bool) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    if only_maximized_flag {
        conn.execute(
            "UPDATE window_state SET is_maximized = 1, updated_at = unixepoch() WHERE id = 1",
            [],
        )
        .map_err(|e| e.to_string())?;
        return Ok(());
    }
    conn.execute(
        "INSERT INTO window_state (id, x, y, width, height, is_maximized, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, unixepoch())
         ON CONFLICT(id) DO UPDATE SET
           x = excluded.x, y = excluded.y,
           width = excluded.width, height = excluded.height,
           is_maximized = excluded.is_maximized,
           updated_at = unixepoch()",
        rusqlite::params![
            state.x,
            state.y,
            state.width,
            state.height,
            state.is_maximized as i32
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Workspace layouts migration (one-shot, from lib.rs) ────────────────

/// Phase 2 Unit 4: relocated from `src-tauri/src/lib.rs`. Migrates
/// legacy `workspaceLayouts` map in `~/.k2so/settings.json` into the
/// `workspace_layouts` SQLite table. Idempotent — safe to call on
/// every boot. After moving, removes `workspaceLayouts` from
/// settings.json so the read side stops fighting the DB.
pub fn migrate_workspace_layouts_to_db() {
    let Some(home) = dirs::home_dir() else { return };
    let settings_path = home.join(".k2").join("settings.json");
    if !settings_path.exists() {
        return;
    }
    let raw = match std::fs::read_to_string(&settings_path) {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return,
    };
    let layouts = match parsed.get("workspaceLayouts") {
        Some(v) if v.is_object() && !v.as_object().unwrap().is_empty() => {
            v.as_object().unwrap().clone()
        }
        _ => return,
    };

    let db = db::shared();
    let conn = db.lock();

    let mut migrated = 0usize;
    for (key, layout_val) in &layouts {
        let parts: Vec<&str> = key.splitn(2, ':').collect();
        if parts.len() != 2 {
            continue;
        }
        let project_id = parts[0];
        let workspace_id = parts[1];
        let layout_json = match serde_json::to_string(layout_val) {
            Ok(j) => j,
            Err(_) => continue,
        };
        let id = key.clone();
        if conn
            .execute(
                "INSERT OR IGNORE INTO workspace_layouts (id, project_id, workspace_id, layout_json, updated_at) VALUES (?1, ?2, ?3, ?4, unixepoch())",
                rusqlite::params![id, project_id, workspace_id, layout_json],
            )
            .is_ok()
        {
            migrated += 1;
        }
    }
    drop(conn);

    if migrated > 0 {
        crate::log_debug!(
            "[daemon/migrations] migrated {migrated} workspace_layouts row(s) from settings.json"
        );
        if let Some(obj) = parsed.as_object_mut() {
            obj.remove("workspaceLayouts");
        }
        if let Ok(json) = serde_json::to_string_pretty(&parsed) {
            let tmp = settings_path.with_extension("json.tmp");
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, &settings_path);
            }
        }
    }
}

#[cfg(test)]
mod tab_title_and_revision_tests {
    //! 0.39.39 (#676 + #677.3) — daemon-canonical tab titles +
    //! monotonic tab-order revision.
    //!
    //! `db::shared()` is a PROCESS-GLOBAL in-memory test DB, so these
    //! tests serialize on a single mutex and use unique ids so they
    //! don't collide with each other or other modules' rows. FK
    //! enforcement is ON in the test DB, so each test seeds its
    //! `projects` (+ `workspaces`) parent rows first.

    use super::*;
    use crate::db;
    use parking_lot::Mutex as PLMutex;

    static TEST_LOCK: PLMutex<()> = PLMutex::new(());

    fn unique(suffix: &str) -> String {
        format!(
            "tt-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            suffix
        )
    }

    fn seed_project(project_id: &str) {
        let dbh = db::shared();
        let conn = dbh.lock();
        crate::db::schema::Project::create(
            &conn,
            project_id,
            "Test Project",
            &format!("/tmp/{project_id}"),
            "#fff",
            0,
            0,
            None,
            None,
        )
        .expect("seed project");
    }

    fn seed_workspace(workspace_id: &str, project_id: &str) {
        let dbh = db::shared();
        let conn = dbh.lock();
        crate::db::schema::Workspace::create(
            &conn,
            workspace_id,
            project_id,
            None,
            "default",
            None,
            "main",
            0,
            None,
        )
        .expect("seed workspace");
    }

    #[test]
    fn tab_title_set_upserts_and_lists() {
        let _g = TEST_LOCK.lock();
        let project_id = unique("p");
        seed_project(&project_id);

        // Insert.
        tab_title_set(&project_id, "tab-a", "First", false).expect("set");
        let titles = tab_titles_for_project(&project_id).expect("list");
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].tab_id, "tab-a");
        assert_eq!(titles[0].title, "First");

        // Upsert same key — title replaced, still one row.
        tab_title_set(&project_id, "tab-a", "Renamed", false).expect("upsert");
        let titles = tab_titles_for_project(&project_id).expect("list2");
        assert_eq!(titles.len(), 1, "upsert must not add a row");
        assert_eq!(titles[0].title, "Renamed");

        // Second tab — two rows now.
        tab_title_set(&project_id, "tab-b", "Second", false).expect("set b");
        let titles = tab_titles_for_project(&project_id).expect("list3");
        assert_eq!(titles.len(), 2);
    }

    #[test]
    fn tab_title_locked_flag_round_trips() {
        let _g = TEST_LOCK.lock();
        let project_id = unique("p");
        seed_project(&project_id);

        // Default insert is unlocked.
        tab_title_set(&project_id, "tab-a", "Auto", false).expect("set unlocked");
        let titles = tab_titles_for_project(&project_id).expect("list");
        assert_eq!(titles.len(), 1);
        assert!(!titles[0].locked, "default insert must be unlocked");

        // A user rename locks the tab — sticky against PTY titles.
        tab_title_set(&project_id, "tab-a", "Mine", true).expect("set locked");
        let titles = tab_titles_for_project(&project_id).expect("list2");
        assert_eq!(titles.len(), 1, "upsert must not add a row");
        assert_eq!(titles[0].title, "Mine");
        assert!(titles[0].locked, "user rename must persist locked=true");
    }

    #[test]
    fn layout_save_revision_is_monotonic_for_lww() {
        let _g = TEST_LOCK.lock();
        let project_id = unique("p");
        let workspace_id = unique("w");
        seed_project(&project_id);
        seed_workspace(&workspace_id, &project_id);

        // First save → revision 1.
        let r1 = workspace_layout_save_with_revision(&project_id, &workspace_id, r#"{"a":1}"#)
            .expect("save1");
        assert_eq!(r1, 1, "first write starts at revision 1");

        // Concurrent-write simulation: two more writes → 2, then 3.
        // A client whose base revision is r1(=1) would see r3(=3) on the
        // broadcast and drop its stale write — LWW resolves
        // deterministically because the revision strictly increases.
        let r2 = workspace_layout_save_with_revision(&project_id, &workspace_id, r#"{"a":2}"#)
            .expect("save2");
        let r3 = workspace_layout_save_with_revision(&project_id, &workspace_id, r#"{"a":3}"#)
            .expect("save3");
        assert_eq!(r2, 2);
        assert_eq!(r3, 3);
        assert!(r3 > r2 && r2 > r1, "revision must be strictly monotonic");

        // The latest layout_json is the last write's.
        let loaded = workspace_layout_load(&project_id, &workspace_id)
            .expect("load")
            .expect("present");
        assert_eq!(loaded, r#"{"a":3}"#);
    }
}

#[cfg(test)]
mod layout_heal_tests {
    //! 0.39.45 (GH #27) — daemon-side system-agent tab identity heal.
    //!
    //! The corruption shape from the issue: workspace Trey's pinned
    //! Chat/Inbox tabs carried sibling RPMqbai's agentName/projectPath
    //! in `workspace_layouts.layout_json` while every daemon SSOT table
    //! was correct. These tests lock the heal-on-save and read-repair
    //! behaviors against that exact shape.

    use super::*;
    use crate::db;

    fn unique(suffix: &str) -> String {
        format!(
            "heal-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            suffix
        )
    }

    /// Seed a `projects` row with a controlled name + path. AGENT.md is
    /// absent on these fabricated paths, so `agent_display_name` falls
    /// back to `projects.name` — making the expected healed agentName
    /// deterministic.
    fn seed_project(project_id: &str, name: &str, path: &str) {
        let dbh = db::shared();
        let conn = dbh.lock();
        crate::db::schema::Project::create(&conn, project_id, name, path, "#fff", 0, 0, None, None)
            .expect("seed project");
    }

    fn seed_workspace(workspace_id: &str, project_id: &str) {
        let dbh = db::shared();
        let conn = dbh.lock();
        crate::db::schema::Workspace::create(
            &conn,
            workspace_id,
            project_id,
            None,
            "default",
            None,
            "main",
            0,
            None,
        )
        .expect("seed workspace");
    }

    /// The #27 corruption shape: a system-agent tab whose agent item is
    /// stamped with a FOREIGN workspace's identity + stale sessionId.
    fn corrupt_layout() -> String {
        serde_json::json!({
            "version": 2,
            "tabs": [{
                "id": "tab-1",
                "title": "Chat",
                "isSystemAgent": true,
                "mosaicTree": "item-1",
                "paneGroups": {
                    "pg-1": {
                        "id": "pg-1",
                        "items": [{
                            "id": "item-1",
                            "type": "agent",
                            "agentName": "RPMqbai",
                            "projectPath": "/tmp/SIBLING/RPMqbai",
                            "section": "chat",
                            "sessionId": "stale-session-from-sibling"
                        }],
                        "activeItemIndex": 0
                    }
                }
            }]
        })
        .to_string()
    }

    #[test]
    fn heal_rewrites_foreign_identity_and_drops_session_id() {
        let pid = unique("rewrite");
        let path = format!("/tmp/{pid}");
        seed_project(&pid, "Trey", &path);

        let healed = heal_system_agent_tab_identity(&pid, &corrupt_layout())
            .expect("foreign identity must be healed");
        let v: serde_json::Value = serde_json::from_str(&healed).expect("healed JSON parses");
        let item = &v["tabs"][0]["paneGroups"]["pg-1"]["items"][0];
        assert_eq!(item["agentName"], serde_json::json!("Trey"));
        assert_eq!(item["projectPath"], serde_json::json!(path));
        assert!(
            item.get("sessionId").is_none(),
            "the sibling's sessionId must be dropped, got: {item}"
        );
        // Non-identity fields survive untouched.
        assert_eq!(item["section"], serde_json::json!("chat"));
        assert_eq!(v["tabs"][0]["title"], serde_json::json!("Chat"));
    }

    #[test]
    fn heal_returns_none_for_already_correct_layout() {
        let pid = unique("correct");
        let path = format!("/tmp/{pid}");
        seed_project(&pid, "SelfName", &path);

        let layout = serde_json::json!({
            "version": 2,
            "tabs": [{
                "id": "tab-1",
                "isSystemAgent": true,
                "paneGroups": { "pg-1": { "items": [{
                    "id": "item-1",
                    "type": "agent",
                    "agentName": "SelfName",
                    "projectPath": path,
                    "section": "chat",
                    "sessionId": "own-session"
                }], "activeItemIndex": 0 } }
            }]
        })
        .to_string();
        assert_eq!(
            heal_system_agent_tab_identity(&pid, &layout),
            None,
            "a correct layout must round-trip untouched (own sessionId preserved)"
        );
    }

    #[test]
    fn heal_leaves_non_system_tabs_alone() {
        let pid = unique("nonsys");
        let path = format!("/tmp/{pid}");
        seed_project(&pid, "NonSys", &path);

        // Same foreign-agent item but on a NON-system tab — a user may
        // legitimately pin another workspace's agent in their layout.
        let layout = serde_json::json!({
            "version": 2,
            "tabs": [{
                "id": "tab-1",
                "isSystemAgent": false,
                "paneGroups": { "pg-1": { "items": [{
                    "id": "item-1",
                    "type": "agent",
                    "agentName": "OtherWs",
                    "projectPath": "/tmp/other-ws",
                    "section": "chat"
                }], "activeItemIndex": 0 } }
            }]
        })
        .to_string();
        assert_eq!(
            heal_system_agent_tab_identity(&pid, &layout),
            None,
            "non-system tabs are user content and must not be rewritten"
        );
    }

    #[test]
    fn heal_returns_none_for_unparseable_layout() {
        let pid = unique("garbage");
        seed_project(&pid, "G", &format!("/tmp/{pid}"));
        assert_eq!(
            heal_system_agent_tab_identity(&pid, "not json at all {{{"),
            None,
            "healing must never eat a layout it can't parse"
        );
    }

    #[test]
    fn save_persists_the_healed_layout() {
        let pid = unique("save");
        let wid = unique("save-ws");
        let path = format!("/tmp/{pid}");
        seed_project(&pid, "SaveHeal", &path);
        seed_workspace(&wid, &pid);

        workspace_layout_save_with_revision(&pid, &wid, &corrupt_layout()).expect("save");

        // Raw read (bypassing load's read-repair) proves the heal
        // happened AT SAVE TIME — the stored bytes are already clean.
        let stored: String = {
            let dbh = db::shared();
            let conn = dbh.lock();
            conn.query_row(
                "SELECT layout_json FROM workspace_layouts WHERE project_id = ?1 AND workspace_id = ?2",
                rusqlite::params![pid, wid],
                |r| r.get(0),
            )
            .expect("stored row")
        };
        let v: serde_json::Value = serde_json::from_str(&stored).expect("stored JSON parses");
        let item = &v["tabs"][0]["paneGroups"]["pg-1"]["items"][0];
        assert_eq!(item["agentName"], serde_json::json!("SaveHeal"));
        assert_eq!(item["projectPath"], serde_json::json!(path));
        assert!(item.get("sessionId").is_none());
    }

    #[test]
    fn load_serves_read_repaired_view_of_a_corrupt_row() {
        let pid = unique("load");
        let wid = unique("load-ws");
        let path = format!("/tmp/{pid}");
        seed_project(&pid, "LoadHeal", &path);
        seed_workspace(&wid, &pid);

        // Plant the corruption DIRECTLY (simulating a row written
        // before the save-side heal existed).
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            conn.execute(
                "INSERT INTO workspace_layouts (id, project_id, workspace_id, layout_json, updated_at, revision)
                 VALUES (?1, ?2, ?3, ?4, unixepoch(), 1)",
                rusqlite::params![format!("{pid}:{wid}"), pid, wid, corrupt_layout()],
            )
            .expect("plant corrupt row");
        }

        let served = workspace_layout_load(&pid, &wid)
            .expect("load ok")
            .expect("row present");
        let v: serde_json::Value = serde_json::from_str(&served).expect("served JSON parses");
        let item = &v["tabs"][0]["paneGroups"]["pg-1"]["items"][0];
        assert_eq!(
            item["agentName"],
            serde_json::json!("LoadHeal"),
            "load must serve the healed view of a pre-existing corrupt row"
        );
        assert_eq!(item["projectPath"], serde_json::json!(path));
        assert!(item.get("sessionId").is_none());
    }
}

#[cfg(test)]
mod leaked_tab_prune_tests {
    //! 2026-07-02 — leaked bare-terminal tab prune (the settings-exit
    //! latency fix). The pre-b339c70 re-mint loop poisoned one dev-box
    //! workspace's saved layout with 488 tabs / 450 dead bare shells;
    //! every workspace-view remount (leaving Settings, workspace
    //! switch) then mounted all ~450 panes and fired ~450 doomed spawn
    //! POSTs (measured: 15 remounts × 450 `bare_tab_cap` refusals in
    //! the daemon log).
    //!
    //! 2026-07-03 — policy hardened for the workspace-switch latency
    //! fix: the first heal kept the newest 32 (the spawn cap), which
    //! left once-healed rows carrying EXACTLY 32 debris tabs — 32
    //! refused spawns per workspace entry, a 3-4s switch. These tests
    //! now pin the binary rule: a plausible count of genuine bare
    //! tabs round-trips untouched; a pathological count prunes to
    //! ZERO prunable tabs (protected/marked/resumable always survive).

    use super::*;
    use crate::db;

    fn unique(suffix: &str) -> String {
        format!(
            "prune-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            suffix
        )
    }

    fn seed_project(project_id: &str) {
        let dbh = db::shared();
        let conn = dbh.lock();
        crate::db::schema::Project::create(
            &conn,
            project_id,
            "Pruned",
            &format!("/tmp/{project_id}"),
            "#fff",
            0,
            0,
            None,
            None,
        )
        .expect("seed project");
    }

    fn seed_workspace(workspace_id: &str, project_id: &str) {
        let dbh = db::shared();
        let conn = dbh.lock();
        crate::db::schema::Workspace::create(
            &conn,
            workspace_id,
            project_id,
            None,
            "default",
            None,
            "main",
            0,
            None,
        )
        .expect("seed workspace");
    }

    /// The leak's exact serialized shape: a bare "Terminal N" tab whose
    /// single pane item is a plain terminal with no markers.
    fn bare_tab(n: usize) -> serde_json::Value {
        let pg = format!("pg-bare-{n}");
        serde_json::json!({
            "id": format!("tab-bare-{n}"),
            "title": format!("Terminal {n}"),
            "mosaicTree": pg,
            "paneGroups": { pg.clone(): {
                "id": pg,
                "items": [{ "id": format!("item-{n}"), "paneGroupId": format!("pg-bare-{n}"), "type": "terminal" }],
                "activeItemIndex": 0
            } }
        })
    }

    fn tab_ids(json: &str) -> Vec<String> {
        let v: serde_json::Value = serde_json::from_str(json).expect("healed JSON parses");
        v["tabs"]
            .as_array()
            .expect("tabs array")
            .iter()
            .map(|t| t["id"].as_str().expect("tab id").to_string())
            .collect()
    }

    #[test]
    fn prune_keeps_plausible_counts_and_zeroes_pathological_ones() {
        let pid = unique("cap");
        seed_project(&pid);
        let plausible = leaked_bare_tab_plausible_max();

        // A human-plausible count (10 genuine open shells) → untouched
        // (None): normal layouts never pay for this heal.
        let genuine = serde_json::json!({
            "version": 2,
            "tabs": (0..10).map(bare_tab).collect::<Vec<_>>()
        })
        .to_string();
        assert_eq!(
            prune_leaked_bare_tabs(&pid, &genuine),
            None,
            "a 10-bare-tab layout is plausibly human and must round-trip untouched"
        );

        // Exactly at the threshold → still untouched (boundary pin).
        let at_threshold = serde_json::json!({
            "version": 2,
            "tabs": (0..plausible).map(bare_tab).collect::<Vec<_>>()
        })
        .to_string();
        assert_eq!(
            prune_leaked_bare_tabs(&pid, &at_threshold),
            None,
            "a layout AT the plausible max must round-trip untouched"
        );

        // One over the threshold → pathological; ALL prunable tabs drop.
        let over = serde_json::json!({
            "version": 2,
            "tabs": (0..plausible + 1).map(bare_tab).collect::<Vec<_>>()
        })
        .to_string();
        let healed = prune_leaked_bare_tabs(&pid, &over).expect("pathological layout must be pruned");
        assert_eq!(
            tab_ids(&healed).len(),
            0,
            "a pathological layout prunes to ZERO bare tabs, not a capped remnant"
        );
    }

    #[test]
    fn prune_second_pass_cleans_a_row_the_old_cap_rule_already_healed() {
        // The live regression shape (dev-box Cortana row): the first
        // version of this heal pruned 488 → 35 by keeping the newest
        // K2_V2_BARE_TAB_CAP (32) bare tabs — which still fired 32
        // doomed spawn POSTs on every workspace entry (the 3-4s
        // switch). A second heal pass over that already-once-healed
        // shape must remove the remaining debris.
        let pid = unique("second-pass");
        seed_project(&pid);

        let mut tabs = vec![
            serde_json::json!({ "id": "tab-system", "isSystemAgent": true,
                "paneGroups": { "pg-s": { "id": "pg-s", "items": [
                    { "id": "i-s", "type": "agent", "agentName": "a", "projectPath": "/tmp/x", "section": "chat" }
                ], "activeItemIndex": 0 } } }),
            serde_json::json!({ "id": "tab-pinned", "isPinnedFile": true,
                "paneGroups": { "pg-p": { "id": "pg-p", "items": [
                    { "id": "i-p", "type": "file-viewer", "filePath": "/tmp/f.html" }
                ], "activeItemIndex": 0 } } }),
        ];
        // Exactly the old cap's remnant: 32 identical bare tabs.
        tabs.extend((461..493).map(bare_tab));
        let once_healed = serde_json::json!({ "version": 2, "tabs": tabs }).to_string();

        let healed = prune_leaked_bare_tabs(&pid, &once_healed)
            .expect("an at-old-cap all-bare row must still read as pathological");
        let ids = tab_ids(&healed);
        assert_eq!(
            ids,
            vec!["tab-system".to_string(), "tab-pinned".to_string()],
            "second pass must drop all 32 debris tabs and keep only real tabs"
        );
    }

    #[test]
    fn prune_spares_system_pinned_locked_marked_and_resumable_tabs() {
        let pid = unique("spare");
        seed_project(&pid);

        // A bare-shaped tab whose pane group has a RESUMABLE saved
        // session — restores real work, must survive.
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            crate::db::schema::WorkspaceTabSession::upsert(
                &conn,
                &crate::db::schema::WorkspaceTabSession {
                    project_id: pid.clone(),
                    pane_group_id: "pg-resumable".into(),
                    agent_name: "tab-pg-resumable".into(),
                    session_id: Some("claude-session-uuid".into()),
                    command: Some("claude".into()),
                    args_json: None,
                    cwd: None,
                    last_seen_at: 0,
                    pinned_cols: None,
                    pinned_rows: None,
                    pinned_set_by: None,
                },
            )
            .expect("seed resumable tab session");
        }
        let resumable_tab = serde_json::json!({
            "id": "tab-resumable",
            "title": "Terminal 999",
            "mosaicTree": "pg-resumable",
            "paneGroups": { "pg-resumable": {
                "id": "pg-resumable",
                "items": [{ "id": "item-r", "paneGroupId": "pg-resumable", "type": "terminal" }],
                "activeItemIndex": 0
            } }
        });
        let protected = [
            serde_json::json!({ "id": "tab-system", "isSystemAgent": true,
                "paneGroups": { "pg-s": { "id": "pg-s", "items": [
                    { "id": "i-s", "type": "agent", "agentName": "a", "projectPath": "/tmp/x", "section": "chat" }
                ], "activeItemIndex": 0 } } }),
            serde_json::json!({ "id": "tab-pinned", "isPinnedFile": true,
                "paneGroups": { "pg-p": { "id": "pg-p", "items": [
                    { "id": "i-p", "type": "file-viewer", "filePath": "/tmp/f.html" }
                ], "activeItemIndex": 0 } } }),
            serde_json::json!({ "id": "tab-locked", "locked": true,
                "paneGroups": { "pg-l": { "id": "pg-l", "items": [
                    { "id": "i-l", "paneGroupId": "pg-l", "type": "terminal" }
                ], "activeItemIndex": 0 } } }),
            serde_json::json!({ "id": "tab-heartbeat",
                "paneGroups": { "pg-h": { "id": "pg-h", "items": [
                    { "id": "i-h", "paneGroupId": "pg-h", "type": "terminal", "heartbeatName": "daily" }
                ], "activeItemIndex": 0 } } }),
            resumable_tab,
        ];

        // Protected tabs at the FRONT — where a prune-everything pass
        // would eat them if the predicate ever regressed. The bare-tab
        // count is far past the plausible max, so ALL of them drop.
        let mut tabs: Vec<serde_json::Value> = protected.to_vec();
        tabs.extend((0..leaked_bare_tab_plausible_max() + 5).map(bare_tab));
        let layout = serde_json::json!({ "version": 2, "tabs": tabs }).to_string();

        let healed = prune_leaked_bare_tabs(&pid, &layout).expect("pathological layout must be pruned");
        let ids = tab_ids(&healed);
        assert_eq!(
            ids.len(),
            protected.len(),
            "every bare tab is pruned; every protected tab survives"
        );
        for id in ["tab-system", "tab-pinned", "tab-locked", "tab-heartbeat", "tab-resumable"] {
            assert!(ids.contains(&id.to_string()), "{id} must survive the prune");
        }
    }

    #[test]
    fn prune_returns_none_for_unparseable_layout() {
        let pid = unique("garbage");
        seed_project(&pid);
        assert_eq!(
            prune_leaked_bare_tabs(&pid, "not json at all {{{"),
            None,
            "pruning must never eat a layout it can't parse"
        );
    }

    #[test]
    fn load_and_save_serve_bounded_layouts_for_a_poisoned_row() {
        let pid = unique("bounded");
        let wid = unique("bounded-ws");
        seed_project(&pid);
        seed_workspace(&wid, &pid);

        // Plant the dev-box pathology directly: 450 leaked bare tabs
        // (written before the prune existed) alongside one real tab.
        let mut poisoned_tabs = vec![serde_json::json!({
            "id": "tab-real", "isSystemAgent": true,
            "paneGroups": { "pg-real": { "id": "pg-real", "items": [
                { "id": "i-real", "type": "agent", "agentName": "a", "projectPath": "/tmp/x", "section": "chat" }
            ], "activeItemIndex": 0 } }
        })];
        poisoned_tabs.extend((0..450).map(bare_tab));
        let poisoned = serde_json::json!({
            "version": 2,
            "tabs": poisoned_tabs
        })
        .to_string();
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            conn.execute(
                "INSERT INTO workspace_layouts (id, project_id, workspace_id, layout_json, updated_at, revision)
                 VALUES (?1, ?2, ?3, ?4, unixepoch(), 1)",
                rusqlite::params![format!("{pid}:{wid}"), pid, wid, poisoned],
            )
            .expect("plant poisoned row");
        }

        // Read-repair: the served view holds ONLY real tabs — THIS is
        // what makes the workspace-entry remount O(real tabs) instead
        // of O(leak).
        let served = workspace_layout_load(&pid, &wid)
            .expect("load ok")
            .expect("row present");
        assert_eq!(
            tab_ids(&served),
            vec!["tab-real".to_string()],
            "load must serve a real-tabs-only view of a poisoned row"
        );

        // Save-side: a client re-persisting its poisoned in-memory
        // layout gets pruned before the row is written.
        workspace_layout_save_with_revision(&pid, &wid, &poisoned).expect("save");
        let stored: String = {
            let dbh = db::shared();
            let conn = dbh.lock();
            conn.query_row(
                "SELECT layout_json FROM workspace_layouts WHERE project_id = ?1 AND workspace_id = ?2",
                rusqlite::params![pid, wid],
                |r| r.get(0),
            )
            .expect("stored row")
        };
        assert_eq!(
            tab_ids(&stored),
            vec!["tab-real".to_string()],
            "save must persist the pruned form, not the poisoned one"
        );
    }
}

#[cfg(test)]
mod preset_metadata_ops_tests {
    //! W6 (0.40.30) — `presets_*` metadata CRUD: slug-id create, write-side
    //! validation, set/clear semantics, and the built-in delete guard.
    //! Uses the process-global in-memory test DB (`db::shared()` →
    //! `init_for_tests`), so every id here is uniquified.

    use super::*;

    fn uid(tag: &str) -> String {
        format!(
            "w6-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        )
    }

    #[test]
    fn create_full_with_slug_id_and_metadata_round_trips() {
        let id = uid("slug");
        let p = presets_create_full(
            Some(&id),
            "W6 Custom",
            "my-agent --model fast",
            None,
            Some(r#"["--auto-yes"]"#),
            Some(r#"{"OPENAI_BASE_URL":"http://localhost:11434/v1"}"#),
            Some("settle:2000"),
        )
        .expect("create");
        assert_eq!(p.id, id, "caller-chosen slug id must be stored verbatim");
        assert_eq!(p.is_built_in, 0);
        assert_eq!(p.enabled, 1);
        assert_eq!(p.danger_flags.as_deref(), Some(r#"["--auto-yes"]"#));
        assert_eq!(
            p.env.as_deref(),
            Some(r#"{"OPENAI_BASE_URL":"http://localhost:11434/v1"}"#)
        );
        assert_eq!(p.readiness.as_deref(), Some("settle:2000"));

        // get sees the same row; a duplicate slug is rejected loudly.
        let got = presets_get(&id).expect("get ok").expect("row present");
        assert_eq!(got.command, "my-agent --model fast");
        let dup = presets_create_full(Some(&id), "Dup", "dup-agent", None, None, None, None);
        assert!(
            dup.unwrap_err().contains("already exists"),
            "duplicate slug must be a loud error"
        );
    }

    #[test]
    fn presets_get_unknown_id_is_ok_none() {
        assert!(
            presets_get(&uid("missing")).expect("no db error").is_none(),
            "unknown id must be Ok(None), never a fabricated row"
        );
    }

    #[test]
    fn write_side_metadata_validation_rejects_garbage() {
        let mk = |df: Option<&str>, env: Option<&str>, ready: Option<&str>| {
            presets_create_full(None, "W6 Bad", "bad-agent", None, df, env, ready)
        };
        assert!(mk(Some("not-json["), None, None).is_err(), "malformed danger_flags");
        assert!(mk(Some(r#"[""]"#), None, None).is_err(), "empty flag entry");
        assert!(mk(None, Some("{broken"), None).is_err(), "malformed env");
        assert!(mk(None, Some(r#"{"A=B":"x"}"#), None).is_err(), "env key with '='");
        assert!(mk(None, None, Some("sentinel:hi")).is_err(), "unknown readiness class");
        assert!(mk(None, None, Some("settle:0")).is_err(), "settle floor");
        assert!(mk(None, None, Some("settle:9999999")).is_err(), "settle ceiling");
        // Bad slug ids too.
        assert!(
            presets_create_full(Some("-leading-dash"), "X", "x", None, None, None, None)
                .is_err(),
            "slug must start alphanumeric"
        );
        assert!(
            presets_create_full(Some("has space"), "X", "x", None, None, None, None).is_err(),
            "slug must not contain spaces"
        );
    }

    #[test]
    fn update_full_sets_and_clears_metadata() {
        let id = uid("upd");
        presets_create_full(Some(&id), "W6 Upd", "upd-agent", None, None, None, None)
            .expect("create");

        // Set all three.
        let p = presets_update_full(
            &id,
            None,
            None,
            None,
            None,
            None,
            Some(Some(r#"["--yolo-mode"]"#)),
            Some(Some(r#"{"K":"v"}"#)),
            Some(Some("bracketed-paste")),
        )
        .expect("set metadata");
        assert_eq!(p.danger_flags.as_deref(), Some(r#"["--yolo-mode"]"#));
        assert_eq!(p.env.as_deref(), Some(r#"{"K":"v"}"#));
        assert_eq!(p.readiness.as_deref(), Some("bracketed-paste"));

        // Outer None leaves untouched.
        let p = presets_update_full(
            &id, Some("Renamed"), None, None, None, None, None, None, None,
        )
        .expect("label-only update");
        assert_eq!(p.label, "Renamed");
        assert_eq!(p.danger_flags.as_deref(), Some(r#"["--yolo-mode"]"#), "metadata untouched");

        // Inner None clears back to NULL.
        let p = presets_update_full(
            &id, None, None, None, None, None,
            Some(None), Some(None), Some(None),
        )
        .expect("clear metadata");
        assert_eq!(p.danger_flags, None);
        assert_eq!(p.env, None);
        assert_eq!(p.readiness, None);

        // Invalid metadata on update is rejected before any write.
        assert!(presets_update_full(
            &id, None, None, None, None, None, None, None, Some(Some("settle:none")),
        )
        .is_err());
    }

    #[test]
    fn update_unknown_id_errors_and_built_in_delete_still_guarded() {
        let missing = uid("ghost");
        let err = presets_update_full(
            &missing, None, None, None, None, None, None, None, None,
        )
        .unwrap_err();
        assert!(err.contains("no preset"), "unknown id must be loud: {err}");

        // Built-ins: metadata EDITABLE, delete REFUSED. Find one seeded row.
        let built_in = presets_list()
            .expect("list")
            .into_iter()
            .find(|p| p.is_built_in == 1)
            .expect("seeded built-ins present in the test DB");
        let before = built_in.danger_flags.clone();
        let p = presets_update_full(
            &built_in.id, None, None, None, None, None,
            Some(Some(r#"["--w6-test-flag"]"#)), None, None,
        )
        .expect("metadata edit on a built-in is allowed");
        assert_eq!(p.danger_flags.as_deref(), Some(r#"["--w6-test-flag"]"#));
        let err = presets_delete(&built_in.id).unwrap_err();
        assert!(
            err.contains("built-in"),
            "built-in delete must stay refused: {err}"
        );
        // Restore the seeded value so other tests sharing the global DB
        // see truthful metadata.
        presets_update_full(
            &built_in.id, None, None, None, None, None,
            Some(before.as_deref()), None, None,
        )
        .expect("restore");
    }
}
