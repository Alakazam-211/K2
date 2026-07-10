use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};

// ── Focus Groups ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusGroup {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub tab_order: i64,
    pub created_at: i64,
}

impl FocusGroup {
    pub fn list(conn: &Connection) -> Result<Vec<FocusGroup>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, color, tab_order, created_at FROM focus_groups ORDER BY tab_order",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(FocusGroup {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                tab_order: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn get(conn: &Connection, id: &str) -> Result<FocusGroup> {
        conn.query_row(
            "SELECT id, name, color, tab_order, created_at FROM focus_groups WHERE id = ?1",
            params![id],
            |row| {
                Ok(FocusGroup {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    tab_order: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )
    }

    pub fn create(conn: &Connection, id: &str, name: &str, color: Option<&str>, tab_order: i64) -> Result<()> {
        conn.execute(
            "INSERT INTO focus_groups (id, name, color, tab_order) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, color, tab_order],
        )?;
        Ok(())
    }

    pub fn update(conn: &Connection, id: &str, name: Option<&str>, color: Option<&str>, tab_order: Option<i64>) -> Result<()> {
        if let Some(n) = name {
            conn.execute("UPDATE focus_groups SET name = ?1 WHERE id = ?2", params![n, id])?;
        }
        if let Some(c) = color {
            conn.execute("UPDATE focus_groups SET color = ?1 WHERE id = ?2", params![c, id])?;
        }
        if let Some(t) = tab_order {
            conn.execute("UPDATE focus_groups SET tab_order = ?1 WHERE id = ?2", params![t, id])?;
        }
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute("DELETE FROM focus_groups WHERE id = ?1", params![id])?;
        Ok(())
    }
}

// ── Projects ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub color: String,
    pub tab_order: i64,
    pub last_opened_at: Option<i64>,
    pub worktree_mode: i64,
    pub icon_url: Option<String>,
    pub focus_group_id: Option<String>,
    pub pinned: i64,
    pub manually_active: i64,
    pub last_interaction_at: Option<i64>,
    pub created_at: i64,
    pub agent_enabled: i64,
    pub heartbeat_enabled: i64,
    pub agent_mode: String,
    pub state_id: Option<String>,
    pub heartbeat_mode: String,
    pub heartbeat_schedule: Option<String>,
    pub heartbeat_last_fire: Option<String>,
    /// #67 — per-workspace remote-instruct opt-in (migration 0054).
    /// 1 = connect-users (role >= Member) may instruct this workspace's
    /// agent via the composer; 0 (default) = deny. The owner is always
    /// allowed regardless; the app-level `allowRemoteInstruct` is a
    /// global master OR'd on top (back-compat). Fail-closed: default 0.
    pub allow_remote_instruct: i64,
    /// Agent de-generalization S1 — per-workspace default agent
    /// (migration 0063). An `agent_presets` preset id (UUID string);
    /// readers must also tolerate a legacy command token like "claude".
    /// `None` = inherit the global `AppSettings.default_agent` at
    /// resolve time. Stamped with the current global default when the
    /// row is created (non-retroactive for pre-existing rows).
    pub default_agent: Option<String>,
}

impl Project {
    pub fn list(conn: &Connection) -> Result<Vec<Project>> {
        // `heartbeat_enabled` is computed LIVE as a true aggregate — "does this
        // workspace have at least one enabled, non-archived heartbeat?" — rather
        // than read from the stale legacy `projects.heartbeat_enabled` column
        // (which was derived from `heartbeat_mode` and drifted out of sync with
        // the per-heartbeat `enabled` flags in `workspace_heartbeats`). The
        // `enabled = 1 AND archived_at IS NULL` predicate mirrors the scheduler's
        // notion of a heartbeat that can actually fire (triage.rs). This keeps the
        // Active-bar autonomous badge + age-out keep-warm gate honest.
        let mut stmt = conn.prepare(
            "SELECT id, name, path, color, tab_order, last_opened_at, worktree_mode, icon_url, focus_group_id, pinned, manually_active, last_interaction_at, created_at, agent_enabled, \
             (EXISTS(SELECT 1 FROM workspace_heartbeats wh WHERE wh.project_id = projects.id AND wh.enabled = 1 AND wh.archived_at IS NULL)) AS heartbeat_enabled, \
             agent_mode, tier_id, heartbeat_mode, heartbeat_schedule, heartbeat_last_fire, allow_remote_instruct, default_agent \
             FROM projects ORDER BY tab_order",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                color: row.get(3)?,
                tab_order: row.get(4)?,
                last_opened_at: row.get(5)?,
                worktree_mode: row.get(6)?,
                icon_url: row.get(7)?,
                focus_group_id: row.get(8)?,
                pinned: row.get(9)?,
                manually_active: row.get(10)?,
                last_interaction_at: row.get(11)?,
                created_at: row.get(12)?,
                agent_enabled: row.get(13)?,
                heartbeat_enabled: row.get(14)?,
                agent_mode: row.get::<_, String>(15).unwrap_or_else(|_| "off".to_string()),
                state_id: row.get(16).ok(),
                heartbeat_mode: row.get::<_, String>(17).unwrap_or_else(|_| "off".to_string()),
                heartbeat_schedule: row.get(18).ok().flatten(),
                heartbeat_last_fire: row.get(19).ok().flatten(),
                allow_remote_instruct: row.get(20).unwrap_or(0),
                default_agent: row.get(21).ok().flatten(),
            })
        })?;
        rows.collect()
    }

    pub fn get(conn: &Connection, id: &str) -> Result<Project> {
        // `heartbeat_enabled` computed live — see `Project::list` for rationale.
        conn.query_row(
            "SELECT id, name, path, color, tab_order, last_opened_at, worktree_mode, icon_url, focus_group_id, pinned, manually_active, last_interaction_at, created_at, agent_enabled, \
             (EXISTS(SELECT 1 FROM workspace_heartbeats wh WHERE wh.project_id = projects.id AND wh.enabled = 1 AND wh.archived_at IS NULL)) AS heartbeat_enabled, \
             agent_mode, tier_id, heartbeat_mode, heartbeat_schedule, heartbeat_last_fire, allow_remote_instruct, default_agent \
             FROM projects WHERE id = ?1",
            params![id],
            |row| {
                Ok(Project {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    color: row.get(3)?,
                    tab_order: row.get(4)?,
                    last_opened_at: row.get(5)?,
                    worktree_mode: row.get(6)?,
                    icon_url: row.get(7)?,
                    focus_group_id: row.get(8)?,
                    pinned: row.get(9)?,
                    manually_active: row.get(10)?,
                    last_interaction_at: row.get(11)?,
                    created_at: row.get(12)?,
                    agent_enabled: row.get(13)?,
                    heartbeat_enabled: row.get(14)?,
                    agent_mode: row.get::<_, String>(15).unwrap_or_else(|_| "off".to_string()),
                    state_id: row.get(16).ok(),
                    heartbeat_mode: row.get::<_, String>(17).unwrap_or_else(|_| "off".to_string()),
                    heartbeat_schedule: row.get(18).ok().flatten(),
                    heartbeat_last_fire: row.get(19).ok().flatten(),
                    allow_remote_instruct: row.get(20).unwrap_or(0),
                    default_agent: row.get(21).ok().flatten(),
                })
            },
        )
    }

    pub fn create(
        conn: &Connection,
        id: &str,
        name: &str,
        path: &str,
        color: &str,
        tab_order: i64,
        worktree_mode: i64,
        icon_url: Option<&str>,
        focus_group_id: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO projects (id, name, path, color, tab_order, worktree_mode, icon_url, focus_group_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, name, path, color, tab_order, worktree_mode, icon_url, focus_group_id],
        )?;
        Ok(())
    }

    pub fn update(
        conn: &Connection,
        id: &str,
        name: Option<&str>,
        path: Option<&str>,
        color: Option<&str>,
        tab_order: Option<i64>,
        worktree_mode: Option<i64>,
        icon_url: Option<Option<&str>>,
        focus_group_id: Option<Option<&str>>,
        pinned: Option<i64>,
        manually_active: Option<i64>,
        agent_enabled: Option<i64>,
        heartbeat_enabled: Option<i64>,
        agent_mode: Option<String>,
        state_id: Option<Option<&str>>,
        heartbeat_mode: Option<String>,
        heartbeat_schedule: Option<Option<&str>>,
        default_agent: Option<Option<&str>>,
    ) -> Result<()> {
        // Wrap in transaction so all field updates succeed or fail atomically.
        // Without this, agent_mode and agent_enabled can diverge if the process crashes mid-update.
        let tx = conn.unchecked_transaction()?;
        if let Some(v) = name {
            tx.execute("UPDATE projects SET name = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = path {
            tx.execute("UPDATE projects SET path = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = color {
            tx.execute("UPDATE projects SET color = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = tab_order {
            tx.execute("UPDATE projects SET tab_order = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = worktree_mode {
            tx.execute("UPDATE projects SET worktree_mode = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = icon_url {
            tx.execute("UPDATE projects SET icon_url = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = focus_group_id {
            tx.execute("UPDATE projects SET focus_group_id = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = pinned {
            tx.execute("UPDATE projects SET pinned = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = manually_active {
            tx.execute("UPDATE projects SET manually_active = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = agent_enabled {
            tx.execute("UPDATE projects SET agent_enabled = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = heartbeat_enabled {
            tx.execute("UPDATE projects SET heartbeat_enabled = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(ref v) = agent_mode {
            tx.execute("UPDATE projects SET agent_mode = ?1 WHERE id = ?2", params![v, id])?;
            // Keep agent_enabled in sync for backward compat
            let enabled = if v == "off" { 0i64 } else { 1i64 };
            tx.execute("UPDATE projects SET agent_enabled = ?1 WHERE id = ?2", params![enabled, id])?;
        }
        if let Some(v) = state_id {
            match v {
                Some(sid) => tx.execute("UPDATE projects SET tier_id = ?1 WHERE id = ?2", params![sid, id])?,
                None => tx.execute("UPDATE projects SET tier_id = NULL WHERE id = ?1", params![id])?,
            };
        }
        if let Some(ref v) = heartbeat_mode {
            tx.execute("UPDATE projects SET heartbeat_mode = ?1 WHERE id = ?2", params![v, id])?;
            // Keep heartbeat_enabled in sync for backward compat
            let enabled = if v == "off" { 0i64 } else { 1i64 };
            tx.execute("UPDATE projects SET heartbeat_enabled = ?1 WHERE id = ?2", params![enabled, id])?;
        }
        if let Some(v) = heartbeat_schedule {
            tx.execute("UPDATE projects SET heartbeat_schedule = ?1 WHERE id = ?2", params![v, id])?;
        }
        // 0063 — per-workspace default agent. `Some(Some(v))` sets it
        // (preset id or legacy command token — stored as given, shape is
        // NOT validated); `Some(None)` clears back to NULL = inherit the
        // global default.
        if let Some(v) = default_agent {
            tx.execute("UPDATE projects SET default_agent = ?1 WHERE id = ?2", params![v, id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        Ok(())
    }

    #[allow(dead_code)] // API surface — covered by tests, not yet wired from UI
    pub fn update_last_opened(conn: &Connection, id: &str) -> Result<()> {
        conn.execute(
            "UPDATE projects SET last_opened_at = unixepoch() WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn touch_interaction(conn: &Connection, id: &str) -> Result<()> {
        conn.execute(
            "UPDATE projects SET last_interaction_at = unixepoch() WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn clear_interaction(conn: &Connection, id: &str) -> Result<()> {
        conn.execute(
            "UPDATE projects SET last_interaction_at = NULL WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }
}

// ── Workspace Sections ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSection {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub color: Option<String>,
    pub is_collapsed: i64,
    pub tab_order: i64,
    pub created_at: i64,
}

impl WorkspaceSection {
    pub fn list(conn: &Connection, project_id: &str) -> Result<Vec<WorkspaceSection>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, name, color, is_collapsed, tab_order, created_at \
             FROM workspace_sections WHERE project_id = ?1 ORDER BY tab_order",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(WorkspaceSection {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                color: row.get(3)?,
                is_collapsed: row.get(4)?,
                tab_order: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn get(conn: &Connection, id: &str) -> Result<WorkspaceSection> {
        conn.query_row(
            "SELECT id, project_id, name, color, is_collapsed, tab_order, created_at \
             FROM workspace_sections WHERE id = ?1",
            params![id],
            |row| {
                Ok(WorkspaceSection {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    name: row.get(2)?,
                    color: row.get(3)?,
                    is_collapsed: row.get(4)?,
                    tab_order: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
    }

    pub fn create(
        conn: &Connection,
        id: &str,
        project_id: &str,
        name: &str,
        color: Option<&str>,
        tab_order: i64,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO workspace_sections (id, project_id, name, color, tab_order) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, project_id, name, color, tab_order],
        )?;
        Ok(())
    }

    pub fn update(
        conn: &Connection,
        id: &str,
        name: Option<&str>,
        color: Option<&str>,
        is_collapsed: Option<i64>,
        tab_order: Option<i64>,
    ) -> Result<()> {
        if let Some(v) = name {
            conn.execute("UPDATE workspace_sections SET name = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = color {
            conn.execute("UPDATE workspace_sections SET color = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = is_collapsed {
            conn.execute("UPDATE workspace_sections SET is_collapsed = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = tab_order {
            conn.execute("UPDATE workspace_sections SET tab_order = ?1 WHERE id = ?2", params![v, id])?;
        }
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute("DELETE FROM workspace_sections WHERE id = ?1", params![id])?;
        Ok(())
    }
}

// ── Workspaces ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub project_id: String,
    pub section_id: Option<String>,
    #[serde(rename = "type")]
    pub type_: String,
    pub branch: Option<String>,
    pub name: String,
    pub tab_order: i64,
    pub worktree_path: Option<String>,
    pub nav_visible: i64,
    pub created_at: i64,
}

impl Workspace {
    pub fn list(conn: &Connection, project_id: &str) -> Result<Vec<Workspace>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, section_id, type, branch, name, tab_order, worktree_path, nav_visible, created_at \
             FROM workspaces WHERE project_id = ?1 ORDER BY tab_order",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(Workspace {
                id: row.get(0)?,
                project_id: row.get(1)?,
                section_id: row.get(2)?,
                type_: row.get(3)?,
                branch: row.get(4)?,
                name: row.get(5)?,
                tab_order: row.get(6)?,
                worktree_path: row.get(7)?,
                nav_visible: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?;
        rows.collect()
    }

    pub fn get(conn: &Connection, id: &str) -> Result<Workspace> {
        conn.query_row(
            "SELECT id, project_id, section_id, type, branch, name, tab_order, worktree_path, nav_visible, created_at \
             FROM workspaces WHERE id = ?1",
            params![id],
            |row| {
                Ok(Workspace {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    section_id: row.get(2)?,
                    type_: row.get(3)?,
                    branch: row.get(4)?,
                    name: row.get(5)?,
                    tab_order: row.get(6)?,
                    worktree_path: row.get(7)?,
                    nav_visible: row.get(8)?,
                    created_at: row.get(9)?,
                })
            },
        )
    }

    pub fn create(
        conn: &Connection,
        id: &str,
        project_id: &str,
        section_id: Option<&str>,
        type_: &str,
        branch: Option<&str>,
        name: &str,
        tab_order: i64,
        worktree_path: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO workspaces (id, project_id, section_id, type, branch, name, tab_order, worktree_path) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, project_id, section_id, type_, branch, name, tab_order, worktree_path],
        )?;
        Ok(())
    }

    pub fn update(
        conn: &Connection,
        id: &str,
        section_id: Option<Option<&str>>,
        type_: Option<&str>,
        branch: Option<Option<&str>>,
        name: Option<&str>,
        tab_order: Option<i64>,
        worktree_path: Option<Option<&str>>,
    ) -> Result<()> {
        if let Some(v) = section_id {
            conn.execute("UPDATE workspaces SET section_id = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = type_ {
            conn.execute("UPDATE workspaces SET type = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = branch {
            conn.execute("UPDATE workspaces SET branch = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = name {
            conn.execute("UPDATE workspaces SET name = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = tab_order {
            conn.execute("UPDATE workspaces SET tab_order = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = worktree_path {
            conn.execute("UPDATE workspaces SET worktree_path = ?1 WHERE id = ?2", params![v, id])?;
        }
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute("DELETE FROM workspaces WHERE id = ?1", params![id])?;
        Ok(())
    }
}

// ── Agent Presets ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPreset {
    pub id: String,
    pub label: String,
    pub command: String,
    pub icon: Option<String>,
    pub enabled: i64,
    pub sort_order: i64,
    pub is_built_in: i64,
    pub created_at: i64,
    /// Migration 0070 — RAW JSON string array of this preset's own
    /// dangerous auto-approve flags. NULL = legacy/unknown (consumers
    /// fail closed; see `workspace::agent_resolve`).
    pub danger_flags: Option<String>,
    /// Migration 0070 — RAW JSON string→string object merged into the
    /// child env at spawn. NULL = no preset env. Values may hold
    /// credentials: NEVER log them.
    pub env: Option<String>,
    /// Migration 0070 — readiness class for the wake/injection path:
    /// 'bracketed-paste' | 'settle:<ms>'. NULL = unknown (default
    /// injection profile).
    pub readiness: Option<String>,
}

impl AgentPreset {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentPreset> {
        Ok(AgentPreset {
            id: row.get(0)?,
            label: row.get(1)?,
            command: row.get(2)?,
            icon: row.get(3)?,
            enabled: row.get(4)?,
            sort_order: row.get(5)?,
            is_built_in: row.get(6)?,
            created_at: row.get(7)?,
            danger_flags: row.get(8)?,
            env: row.get(9)?,
            readiness: row.get(10)?,
        })
    }

    pub fn list(conn: &Connection) -> Result<Vec<AgentPreset>> {
        let mut stmt = conn.prepare(
            "SELECT id, label, command, icon, enabled, sort_order, is_built_in, created_at, \
                    danger_flags, env, readiness \
             FROM agent_presets ORDER BY sort_order",
        )?;
        let rows = stmt.query_map([], AgentPreset::from_row)?;
        rows.collect()
    }

    pub fn get(conn: &Connection, id: &str) -> Result<AgentPreset> {
        conn.query_row(
            "SELECT id, label, command, icon, enabled, sort_order, is_built_in, created_at, \
                    danger_flags, env, readiness \
             FROM agent_presets WHERE id = ?1",
            params![id],
            AgentPreset::from_row,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create(
        conn: &Connection,
        id: &str,
        label: &str,
        command: &str,
        icon: Option<&str>,
        enabled: i64,
        sort_order: i64,
        is_built_in: i64,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, label, command, icon, enabled, sort_order, is_built_in],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        conn: &Connection,
        id: &str,
        label: Option<&str>,
        command: Option<&str>,
        icon: Option<Option<&str>>,
        enabled: Option<i64>,
        sort_order: Option<i64>,
    ) -> Result<()> {
        if let Some(v) = label {
            conn.execute("UPDATE agent_presets SET label = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = command {
            conn.execute("UPDATE agent_presets SET command = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = icon {
            conn.execute("UPDATE agent_presets SET icon = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = enabled {
            conn.execute("UPDATE agent_presets SET enabled = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = sort_order {
            conn.execute("UPDATE agent_presets SET sort_order = ?1 WHERE id = ?2", params![v, id])?;
        }
        Ok(())
    }

    /// Write the migration-0070 metadata columns. Outer `None` = leave
    /// unchanged; inner `None` = clear to NULL (back to legacy/unknown,
    /// which every consumer fail-closes on). Validation of the JSON /
    /// readiness grammar is `db_ops`' job — this is the raw column write.
    pub fn update_metadata(
        conn: &Connection,
        id: &str,
        danger_flags: Option<Option<&str>>,
        env: Option<Option<&str>>,
        readiness: Option<Option<&str>>,
    ) -> Result<()> {
        if let Some(v) = danger_flags {
            conn.execute(
                "UPDATE agent_presets SET danger_flags = ?1 WHERE id = ?2",
                params![v, id],
            )?;
        }
        if let Some(v) = env {
            conn.execute("UPDATE agent_presets SET env = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = readiness {
            conn.execute(
                "UPDATE agent_presets SET readiness = ?1 WHERE id = ?2",
                params![v, id],
            )?;
        }
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute("DELETE FROM agent_presets WHERE id = ?1", params![id])?;
        Ok(())
    }
}

// ── Time Entries ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeEntry {
    pub id: String,
    pub project_id: Option<String>,
    pub start_time: i64,
    pub end_time: i64,
    pub duration_seconds: i64,
    pub memo: Option<String>,
    pub created_at: i64,
}

impl TimeEntry {
    pub fn list(
        conn: &Connection,
        start: Option<i64>,
        end: Option<i64>,
        project_id: Option<&str>,
    ) -> Result<Vec<TimeEntry>> {
        let mut sql = String::from(
            "SELECT id, project_id, start_time, end_time, duration_seconds, memo, created_at \
             FROM time_entries WHERE 1=1",
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1;

        if let Some(s) = start {
            sql.push_str(&format!(" AND start_time >= ?{}", idx));
            param_values.push(Box::new(s));
            idx += 1;
        }
        if let Some(e) = end {
            sql.push_str(&format!(" AND start_time <= ?{}", idx));
            param_values.push(Box::new(e));
            idx += 1;
        }
        if let Some(pid) = project_id {
            sql.push_str(&format!(" AND project_id = ?{}", idx));
            param_values.push(Box::new(pid.to_string()));
        }
        sql.push_str(" ORDER BY start_time DESC");

        let mut stmt = conn.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), |row| {
            Ok(TimeEntry {
                id: row.get(0)?,
                project_id: row.get(1)?,
                start_time: row.get(2)?,
                end_time: row.get(3)?,
                duration_seconds: row.get(4)?,
                memo: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn create(
        conn: &Connection,
        id: &str,
        project_id: Option<&str>,
        start_time: i64,
        end_time: i64,
        duration_seconds: i64,
        memo: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO time_entries (id, project_id, start_time, end_time, duration_seconds, memo) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, project_id, start_time, end_time, duration_seconds, memo],
        )?;
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute("DELETE FROM time_entries WHERE id = ?1", params![id])?;
        Ok(())
    }
}

// ── Workspace Tab Sessions ──────────────────────────────────────────────────
//
// 0.38.5 — daemon-side persistence of per-pane session metadata so Cmd+T
// terminal tabs survive daemon restart. See
// `0045_workspace_tab_sessions.sql` for the full rationale. Replaces the
// stale `TerminalTab` + `TerminalPane` stubs that were created in
// migration 0000 for a renderer-normalized layout design that never
// shipped — the underlying tables stayed at 0 rows for the entire
// lifetime of K2SO and were dropped by the 0045 migration.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTabSession {
    pub project_id: String,
    pub pane_group_id: String,
    pub agent_name: String,
    pub session_id: Option<String>,
    pub command: Option<String>,
    /// JSON-serialized `Vec<String>` so it round-trips through serde
    /// without a custom column type.
    pub args_json: Option<String>,
    pub cwd: Option<String>,
    pub last_seen_at: i64,
    /// S7a pin-to-size (migration 0065): the pinned grid geometry.
    /// Both `Some` + nonzero = pinned; anything else = unpinned.
    /// Written ONLY by `set_pinned_size` — the registration `upsert`
    /// deliberately never touches these, so a re-register can't
    /// silently unpin a session.
    pub pinned_cols: Option<u16>,
    pub pinned_rows: Option<u16>,
    /// Attribution for the pin: "owner" or the connect-user's
    /// daemon-resolved username. `None` when unpinned.
    pub pinned_set_by: Option<String>,
}

impl WorkspaceTabSession {
    /// Upsert the row for `(project_id, pane_group_id)`. Called by
    /// `v2_session_map::register` on every PTY registration so the
    /// daemon's restart-time recovery picks up the most recent spawn
    /// args. `session_id` is updated separately via `stamp_session_id`
    /// once the CLI tool (claude / codex) reports it; first-time
    /// upserts come through as `None` here.
    ///
    /// S7a: the pin columns ride the INSERT (fresh rows are unpinned —
    /// callers pass `None`) but are NOT in the conflict-UPDATE set, so
    /// a re-register never clobbers a live pin. Pins are written only
    /// via [`Self::set_pinned_size`].
    pub fn upsert(
        conn: &Connection,
        row: &WorkspaceTabSession,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO workspace_tab_sessions \
                (project_id, pane_group_id, agent_name, session_id, command, args_json, cwd, last_seen_at, \
                 pinned_cols, pinned_rows, pinned_set_by) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, unixepoch(), ?8, ?9, ?10) \
             ON CONFLICT(project_id, pane_group_id) DO UPDATE SET \
                agent_name = excluded.agent_name, \
                session_id = COALESCE(excluded.session_id, workspace_tab_sessions.session_id), \
                command = excluded.command, \
                args_json = excluded.args_json, \
                cwd = excluded.cwd, \
                last_seen_at = unixepoch()",
            params![
                row.project_id,
                row.pane_group_id,
                row.agent_name,
                row.session_id,
                row.command,
                row.args_json,
                row.cwd,
                row.pinned_cols,
                row.pinned_rows,
                row.pinned_set_by,
            ],
        )?;
        Ok(())
    }

    /// S7a pin-to-size (migration 0065) — persist or clear the pinned
    /// grid geometry for `(project_id, pane_group_id)`.
    ///
    /// Upserting (rather than a bare UPDATE) covers the canonical
    /// pinned chat, whose identity deliberately does NOT keep a tab
    /// row (GH#24): pinning it creates a PIN-ONLY row (command/args/
    /// session_id all NULL, which restart-recovery already treats as
    /// "no saved launch" and routes to the canonical resume resolver),
    /// so the pin still survives a daemon restart without re-creating
    /// the double-booked identity GH#24 removed.
    ///
    /// `pin = None` clears all three columns (unpinned).
    pub fn set_pinned_size(
        conn: &Connection,
        project_id: &str,
        pane_group_id: &str,
        agent_name: &str,
        cwd: Option<&str>,
        pin: Option<(u16, u16, &str)>,
    ) -> Result<()> {
        let (cols, rows, set_by) = match pin {
            Some((c, r, by)) => (Some(c), Some(r), Some(by.to_string())),
            None => (None, None, None),
        };
        conn.execute(
            "INSERT INTO workspace_tab_sessions \
                (project_id, pane_group_id, agent_name, session_id, command, args_json, cwd, last_seen_at, \
                 pinned_cols, pinned_rows, pinned_set_by) \
             VALUES (?1, ?2, ?3, NULL, NULL, NULL, ?4, unixepoch(), ?5, ?6, ?7) \
             ON CONFLICT(project_id, pane_group_id) DO UPDATE SET \
                pinned_cols = excluded.pinned_cols, \
                pinned_rows = excluded.pinned_rows, \
                pinned_set_by = excluded.pinned_set_by, \
                last_seen_at = unixepoch()",
            params![project_id, pane_group_id, agent_name, cwd, cols, rows, set_by],
        )?;
        Ok(())
    }

    /// Look up the saved session for a `(project_id, pane_group_id)`
    /// pair. Returns `None` if no row exists — the daemon then spawns
    /// fresh (the pre-0.38.5 default behavior).
    pub fn get(
        conn: &Connection,
        project_id: &str,
        pane_group_id: &str,
    ) -> Result<Option<WorkspaceTabSession>> {
        let mut stmt = conn.prepare(
            "SELECT project_id, pane_group_id, agent_name, session_id, \
                    command, args_json, cwd, last_seen_at, \
                    pinned_cols, pinned_rows, pinned_set_by \
             FROM workspace_tab_sessions \
             WHERE project_id = ?1 AND pane_group_id = ?2",
        )?;
        let mut rows = stmt.query_map(params![project_id, pane_group_id], |r| {
            Ok(WorkspaceTabSession {
                project_id: r.get(0)?,
                pane_group_id: r.get(1)?,
                agent_name: r.get(2)?,
                session_id: r.get(3)?,
                command: r.get(4)?,
                args_json: r.get(5)?,
                cwd: r.get(6)?,
                last_seen_at: r.get(7)?,
                pinned_cols: r.get(8)?,
                pinned_rows: r.get(9)?,
                pinned_set_by: r.get(10)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Look up by canonical `agent_name`. The daemon's v2_session_map
    /// is keyed by agent_name (which is `tab-<pane_group_id>` for
    /// Cmd+T tabs, bare project_id for pinned chat, heartbeat name for
    /// heartbeats) and v2_spawn often only has the agent_name in hand,
    /// not the pane_group_id. The DB key uses `(project_id, pane_group_id)`
    /// for normality with the renderer; this lookup bridges the two.
    pub fn get_by_agent_name(
        conn: &Connection,
        project_id: &str,
        agent_name: &str,
    ) -> Result<Option<WorkspaceTabSession>> {
        let mut stmt = conn.prepare(
            "SELECT project_id, pane_group_id, agent_name, session_id, \
                    command, args_json, cwd, last_seen_at, \
                    pinned_cols, pinned_rows, pinned_set_by \
             FROM workspace_tab_sessions \
             WHERE project_id = ?1 AND agent_name = ?2",
        )?;
        let mut rows = stmt.query_map(params![project_id, agent_name], |r| {
            Ok(WorkspaceTabSession {
                project_id: r.get(0)?,
                pane_group_id: r.get(1)?,
                agent_name: r.get(2)?,
                session_id: r.get(3)?,
                command: r.get(4)?,
                args_json: r.get(5)?,
                cwd: r.get(6)?,
                last_seen_at: r.get(7)?,
                pinned_cols: r.get(8)?,
                pinned_rows: r.get(9)?,
                pinned_set_by: r.get(10)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Stamp the CLI tool's session id after the renderer (or daemon)
    /// has detected it. Separate from `upsert` because the session id
    /// arrives later than the spawn — claude emits it on first turn,
    /// not at process start. Idempotent if the value is unchanged.
    pub fn stamp_session_id(
        conn: &Connection,
        project_id: &str,
        pane_group_id: &str,
        session_id: &str,
    ) -> Result<()> {
        conn.execute(
            "UPDATE workspace_tab_sessions \
             SET session_id = ?3, last_seen_at = unixepoch() \
             WHERE project_id = ?1 AND pane_group_id = ?2",
            params![project_id, pane_group_id, session_id],
        )?;
        Ok(())
    }
}

// ── Workspace States ─────────────────────────────────────────────────────

/// A workspace state defines what agents are allowed to do automatically.
/// Each capability has three levels: "auto", "gated", "off".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceState {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub is_built_in: i64,
    /// Features: new functionality, enhancements
    pub cap_features: String,
    /// Issues: bug fixes from submitted issues
    pub cap_issues: String,
    /// Crashes: automatic crash report fixes
    pub cap_crashes: String,
    /// Security: automatic security patches
    pub cap_security: String,
    /// Audits: scheduled code reviews
    pub cap_audits: String,
    /// Whether the heartbeat scheduler is active
    pub heartbeat: i64,
    pub sort_order: i64,
}

impl WorkspaceState {
    pub fn list(conn: &Connection) -> Result<Vec<WorkspaceState>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, description, is_built_in, cap_features, cap_issues, cap_crashes, cap_security, cap_audits, heartbeat, sort_order \
             FROM workspace_states ORDER BY sort_order"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(WorkspaceState {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                is_built_in: row.get(3)?,
                cap_features: row.get(4)?,
                cap_issues: row.get(5)?,
                cap_crashes: row.get(6)?,
                cap_security: row.get(7)?,
                cap_audits: row.get(8)?,
                heartbeat: row.get(9)?,
                sort_order: row.get(10)?,
            })
        })?;
        rows.collect()
    }

    pub fn get(conn: &Connection, id: &str) -> Result<WorkspaceState> {
        conn.query_row(
            "SELECT id, name, description, is_built_in, cap_features, cap_issues, cap_crashes, cap_security, cap_audits, heartbeat, sort_order \
             FROM workspace_states WHERE id = ?1",
            params![id],
            |row| {
                Ok(WorkspaceState {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    is_built_in: row.get(3)?,
                    cap_features: row.get(4)?,
                    cap_issues: row.get(5)?,
                    cap_crashes: row.get(6)?,
                    cap_security: row.get(7)?,
                    cap_audits: row.get(8)?,
                    heartbeat: row.get(9)?,
                    sort_order: row.get(10)?,
                })
            },
        )
    }

    pub fn create(
        conn: &Connection,
        id: &str,
        name: &str,
        description: Option<&str>,
        cap_features: &str,
        cap_issues: &str,
        cap_crashes: &str,
        cap_security: &str,
        cap_audits: &str,
        heartbeat: bool,
    ) -> Result<()> {
        // Wrap in transaction to prevent race condition on sort_order
        // (Zed pattern: savepoint-wrapped mutations for atomicity)
        let tx = conn.unchecked_transaction()?;
        let max_order: i64 = tx.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM workspace_states", [], |r| r.get(0)
        )?;
        tx.execute(
            "INSERT INTO workspace_states (id, name, description, is_built_in, cap_features, cap_issues, cap_crashes, cap_security, cap_audits, heartbeat, sort_order) \
             VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![id, name, description, cap_features, cap_issues, cap_crashes, cap_security, cap_audits, heartbeat as i64, max_order],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn update(
        conn: &Connection,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        cap_features: Option<&str>,
        cap_issues: Option<&str>,
        cap_crashes: Option<&str>,
        cap_security: Option<&str>,
        cap_audits: Option<&str>,
        heartbeat: Option<bool>,
    ) -> Result<()> {
        // Wrap in transaction so all updates succeed or fail together
        // (Zed pattern: atomic multi-field updates prevent partial state)
        let tx = conn.unchecked_transaction()?;
        if let Some(v) = name { tx.execute("UPDATE workspace_states SET name = ?1 WHERE id = ?2", params![v, id])?; }
        if let Some(v) = description { tx.execute("UPDATE workspace_states SET description = ?1 WHERE id = ?2", params![v, id])?; }
        if let Some(v) = cap_features { tx.execute("UPDATE workspace_states SET cap_features = ?1 WHERE id = ?2", params![v, id])?; }
        if let Some(v) = cap_issues { tx.execute("UPDATE workspace_states SET cap_issues = ?1 WHERE id = ?2", params![v, id])?; }
        if let Some(v) = cap_crashes { tx.execute("UPDATE workspace_states SET cap_crashes = ?1 WHERE id = ?2", params![v, id])?; }
        if let Some(v) = cap_security { tx.execute("UPDATE workspace_states SET cap_security = ?1 WHERE id = ?2", params![v, id])?; }
        if let Some(v) = cap_audits { tx.execute("UPDATE workspace_states SET cap_audits = ?1 WHERE id = ?2", params![v, id])?; }
        if let Some(v) = heartbeat { tx.execute("UPDATE workspace_states SET heartbeat = ?1 WHERE id = ?2", params![v as i64, id])?; }
        tx.commit()?;
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        // Don't delete built-in states — explicit check instead of unwrap_or(1) which
        // silently treats "not found" as "built-in"
        let is_built_in = conn.query_row(
            "SELECT is_built_in FROM workspace_states WHERE id = ?1", params![id], |r| r.get::<_, i64>(0)
        );
        match is_built_in {
            Ok(1) => return Ok(()), // Built-in — don't delete
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(()), // Not found — nothing to delete
            Err(e) => return Err(e),
            Ok(_) => {} // Custom state — proceed with delete
        }
        // Wrap cascade + delete in transaction for atomicity
        let tx = conn.unchecked_transaction()?;
        // Clear tier_id on projects using this state
        tx.execute("UPDATE projects SET tier_id = NULL WHERE tier_id = ?1", params![id])?;
        tx.execute("DELETE FROM workspace_states WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Get the capability state for a given work item source type.
    /// Returns "auto", "gated", or "off".
    pub fn capability_for_source(&self, source: &str) -> &str {
        match source {
            "feature" => &self.cap_features,
            "issue" => &self.cap_issues,
            "crash" => &self.cap_crashes,
            "security" => &self.cap_security,
            "audit" => &self.cap_audits,
            _ => "gated", // Unknown source → require approval
        }
    }
}

// ── Sandbox Sessions (fs-mirror PRD §5 — host bridge index) ────────────
//
// One row per workspace-scoped MIRROR sandbox session (migration 0061).
// The host bridge between the real host paths and the cell's relative
// resolution: the LIST reads it (audit), the RESUME path reads it (which
// sandbox home + `/work` layer to re-mount). Distinct from the canonical
// `workspace_sessions` (off-limits, 1-per-workspace).

/// A workspace-scoped sandbox session's host-side index row.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxSession {
    /// The forced `SessionId` == `K2_SESSION_ID` == the `.jsonl` key.
    pub session_id: String,
    /// The URL slug the session was addressed under (e.g. `ai`).
    pub workspace_slug: String,
    /// The REAL workspace path == the in-cell cwd (mirror).
    pub workspace_path: String,
    /// Host: `~/.k2/sandbox-homes/<ws>/.claude` (the per-ws sandbox home).
    pub sandbox_home_path: String,
    /// `<sandbox_home_path>/projects/<slug>/<session_id>.jsonl`.
    pub jsonl_path: String,
    /// Host: `~/.k2/sandbox-overlays/<ws>/<sid>/work-scratch` (the `/work` layer).
    pub layer_path: String,
    /// The claude project slug = `workspace_path` with `/`→`-`.
    pub slug: String,
    pub created_at: i64,
    pub last_active_at: i64,
}

impl SandboxSession {
    /// Upsert the row for a sandbox session (keyed by `session_id`). Called after
    /// a successful workspace-scoped spawn. On conflict (a resume) it refreshes
    /// `last_active_at` + the paths (idempotent), never duplicating the session.
    pub fn upsert(conn: &Connection, row: &SandboxSession) -> Result<()> {
        conn.execute(
            "INSERT INTO sandbox_sessions \
                (session_id, workspace_slug, workspace_path, sandbox_home_path, \
                 jsonl_path, layer_path, slug, created_at, last_active_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, unixepoch(), unixepoch()) \
             ON CONFLICT(session_id) DO UPDATE SET \
                workspace_slug = excluded.workspace_slug, \
                workspace_path = excluded.workspace_path, \
                sandbox_home_path = excluded.sandbox_home_path, \
                jsonl_path = excluded.jsonl_path, \
                layer_path = excluded.layer_path, \
                slug = excluded.slug, \
                last_active_at = unixepoch()",
            params![
                row.session_id,
                row.workspace_slug,
                row.workspace_path,
                row.sandbox_home_path,
                row.jsonl_path,
                row.layer_path,
                row.slug,
            ],
        )?;
        Ok(())
    }

    /// List a workspace's sandbox sessions, newest first (per-workspace audit).
    pub fn list_for_workspace(
        conn: &Connection,
        workspace_slug: &str,
    ) -> Result<Vec<SandboxSession>> {
        let mut stmt = conn.prepare(
            "SELECT session_id, workspace_slug, workspace_path, sandbox_home_path, \
                    jsonl_path, layer_path, slug, created_at, last_active_at \
             FROM sandbox_sessions WHERE workspace_slug = ?1 \
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![workspace_slug], |r| {
            Ok(SandboxSession {
                session_id: r.get(0)?,
                workspace_slug: r.get(1)?,
                workspace_path: r.get(2)?,
                sandbox_home_path: r.get(3)?,
                jsonl_path: r.get(4)?,
                slug: r.get(6)?,
                layer_path: r.get(5)?,
                created_at: r.get(7)?,
                last_active_at: r.get(8)?,
            })
        })?;
        rows.collect()
    }

    /// Look up a single sandbox session by id (the resume re-mount lookup).
    pub fn get(conn: &Connection, session_id: &str) -> Result<Option<SandboxSession>> {
        let mut stmt = conn.prepare(
            "SELECT session_id, workspace_slug, workspace_path, sandbox_home_path, \
                    jsonl_path, layer_path, slug, created_at, last_active_at \
             FROM sandbox_sessions WHERE session_id = ?1",
        )?;
        let mut rows = stmt.query_map(params![session_id], |r| {
            Ok(SandboxSession {
                session_id: r.get(0)?,
                workspace_slug: r.get(1)?,
                workspace_path: r.get(2)?,
                sandbox_home_path: r.get(3)?,
                jsonl_path: r.get(4)?,
                layer_path: r.get(5)?,
                slug: r.get(6)?,
                created_at: r.get(7)?,
                last_active_at: r.get(8)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }
}

// ── Workspace Sessions ─────────────────────────────────────────────────
//
// One row per `project_id`. The product invariant ("a workspace IS its
// agent") is the schema constraint via `UNIQUE(project_id)` enforced
// in migration 0039. The legacy `agent_name` column is gone — every
// method that used to take it now keys purely by `project_id`.

/// DB-tracked workspace agent session. Single source of truth — the
/// legacy `.lock` and `.last_session` filesystem tracking was retired.
/// `owner` distinguishes system-managed sessions (safe to inject) from
/// user interactive sessions (never inject).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSession {
    pub id: String,
    pub project_id: String,
    pub terminal_id: Option<String>,
    pub session_id: Option<String>,
    pub harness: String,
    pub owner: String,
    pub status: String,
    pub status_message: Option<String>,
    pub last_activity_at: Option<i64>,
    pub created_at: i64,
    /// Daemon-side session_id of the live PTY currently attached to
    /// this workspace's session, or NULL when no PTY is alive. Mirrors
    /// `workspace_heartbeats.active_terminal_id` (migration 0037). Stamped
    /// by `v2_spawn::handle_v2_spawn` after registering, cleared by
    /// `v2_session_map::unregister`'s child-exit hook. Distinct from
    /// `terminal_id` (renderer-scoped UUID like
    /// `agent-chat:<projId>:<agent>`) and `session_id` (Claude's
    /// conversation UUID for `--resume`). See migration 0037.
    pub active_terminal_id: Option<String>,
}

impl WorkspaceSession {
    /// Insert or replace the session for a workspace. Schema-level
    /// `UNIQUE(project_id)` guarantees at most one row per workspace.
    pub fn upsert(
        conn: &Connection,
        id: &str,
        project_id: &str,
        terminal_id: Option<&str>,
        session_id: Option<&str>,
        harness: &str,
        owner: &str,
        status: &str,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, terminal_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, unixepoch()) \
             ON CONFLICT(project_id) DO UPDATE SET \
               terminal_id = ?3, session_id = COALESCE(?4, workspace_sessions.session_id), \
               harness = ?5, owner = ?6, status = ?7, last_activity_at = unixepoch()",
            params![id, project_id, terminal_id, session_id, harness, owner, status],
        )?;
        Ok(())
    }

    /// Find the session row whose `terminal_id` matches — used by the
    /// hook handler to resolve which workspace_sessions row a fired
    /// event belongs to without the caller needing to know the project.
    pub fn get_by_terminal_id(conn: &Connection, terminal_id: &str) -> Result<Option<WorkspaceSession>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, terminal_id, session_id, harness, owner, status, status_message, last_activity_at, created_at, active_terminal_id \
             FROM workspace_sessions WHERE terminal_id = ?1 LIMIT 1"
        )?;
        let mut rows = stmt.query_map(params![terminal_id], |row| {
            Ok(WorkspaceSession {
                id: row.get(0)?,
                project_id: row.get(1)?,
                terminal_id: row.get(2)?,
                session_id: row.get(3)?,
                harness: row.get(4)?,
                owner: row.get(5)?,
                status: row.get(6)?,
                status_message: row.get(7)?,
                last_activity_at: row.get(8)?,
                created_at: row.get(9)?,
                active_terminal_id: row.get(10)?,
            })
        })?;
        match rows.next() {
            Some(Ok(s)) => Ok(Some(s)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    /// Read the workspace's session row by project_id. Returns `None`
    /// when the workspace has never spawned a session.
    pub fn get(conn: &Connection, project_id: &str) -> Result<Option<WorkspaceSession>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, terminal_id, session_id, harness, owner, status, status_message, last_activity_at, created_at, active_terminal_id \
             FROM workspace_sessions WHERE project_id = ?1"
        )?;
        let mut rows = stmt.query_map(params![project_id], |row| {
            Ok(WorkspaceSession {
                id: row.get(0)?,
                project_id: row.get(1)?,
                terminal_id: row.get(2)?,
                session_id: row.get(3)?,
                harness: row.get(4)?,
                owner: row.get(5)?,
                status: row.get(6)?,
                status_message: row.get(7)?,
                last_activity_at: row.get(8)?,
                created_at: row.get(9)?,
                active_terminal_id: row.get(10)?,
            })
        })?;
        match rows.next() {
            Some(Ok(s)) => Ok(Some(s)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    pub fn update_status(conn: &Connection, project_id: &str, status: &str) -> Result<usize> {
        // Fires on every agent state transition — cached.
        let mut stmt = conn.prepare_cached(
            "UPDATE workspace_sessions SET status = ?1, last_activity_at = unixepoch() WHERE project_id = ?2",
        )?;
        stmt.execute(params![status, project_id])
    }

    /// Try to atomically acquire the "running" lock for the workspace's
    /// session. Returns `Ok(true)` if this call took the lock (caller
    /// proceeds to spawn the PTY); `Ok(false)` if the workspace was
    /// already running (caller must NOT spawn).
    ///
    /// Replaces the pre-0.32.9 `is_locked → spawn → upsert` sequence,
    /// which had a TOCTOU race: two heartbeats firing simultaneously
    /// could both observe `is_locked=false` and both spawn, producing
    /// duplicate PTYs and a stale row. `BEGIN IMMEDIATE` takes the DB
    /// write lock before any reads so concurrent callers serialize.
    // TODO(resilience-followup): 0.32.9 introduced this CAS helper
    // but the production spawn path in `commands/k2so_agents.rs` still
    // uses the pre-CAS `is_locked → spawn → upsert` sequence.
    #[allow(dead_code)]
    pub fn try_acquire_running(
        conn: &Connection,
        session_id: &str,
        project_id: &str,
        terminal_id: Option<&str>,
        harness: &str,
        owner: &str,
    ) -> Result<bool> {
        conn.execute_batch("BEGIN IMMEDIATE;")?;

        let existing: Option<String> = conn.query_row(
            "SELECT status FROM workspace_sessions WHERE project_id = ?1",
            params![project_id],
            |row| row.get::<_, String>(0),
        ).ok();

        if matches!(existing.as_deref(), Some("running")) {
            conn.execute_batch("ROLLBACK;")?;
            return Ok(false);
        }

        let result = conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, terminal_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, NULL, ?4, ?5, 'running', unixepoch()) \
             ON CONFLICT(project_id) DO UPDATE SET \
               terminal_id = excluded.terminal_id, \
               harness = excluded.harness, \
               owner = excluded.owner, \
               status = 'running', \
               last_activity_at = unixepoch()",
            params![session_id, project_id, terminal_id, harness, owner],
        );

        match result {
            Ok(_) => {
                conn.execute_batch("COMMIT;")?;
                Ok(true)
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    pub fn update_status_message(conn: &Connection, project_id: &str, message: &str) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_sessions SET status_message = ?1, last_activity_at = unixepoch() WHERE project_id = ?2",
            params![message, project_id],
        )
    }

    pub fn update_session_id(conn: &Connection, project_id: &str, session_id: &str) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_sessions SET session_id = ?1, last_activity_at = unixepoch() WHERE project_id = ?2",
            params![session_id, project_id],
        )
    }

    /// Update the session id AND the harness (provider) that owns it,
    /// in one write. Slice 3 (agent de-generalization) makes `harness`
    /// load-bearing: the pinned-chat resolver reads it to pick the
    /// ProviderResume adapter, so every site that adopts a session id
    /// for a known provider must stamp the provider alongside it
    /// (`set-chat-session` with a `provider` param; the post-hoc
    /// adoption helper). [`Self::update_session_id`] stays for callers
    /// that don't know the provider (keeps the existing harness).
    pub fn update_session_id_and_harness(
        conn: &Connection,
        project_id: &str,
        session_id: &str,
        harness: &str,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_sessions SET session_id = ?1, harness = ?2, last_activity_at = unixepoch() \
             WHERE project_id = ?3",
            params![session_id, harness, project_id],
        )
    }

    pub fn clear_session_id(conn: &Connection, project_id: &str) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_sessions SET session_id = NULL WHERE project_id = ?1",
            params![project_id],
        )
    }

    /// Toggle the per-workspace "surfaced" boolean. `true` = the session
    /// has a live tab in the renderer; `false` = the session is running
    /// headless (heartbeat default). See migration 0036.
    pub fn set_surfaced(
        conn: &Connection,
        project_id: &str,
        surfaced: bool,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_sessions SET surfaced = ?1, last_activity_at = unixepoch() \
             WHERE project_id = ?2",
            params![surfaced as i64, project_id],
        )
    }

    /// Read the current surfaced flag. `false` if the row doesn't
    /// exist (no session yet → can't be surfaced).
    pub fn is_surfaced(conn: &Connection, project_id: &str) -> Result<bool> {
        conn.query_row(
            "SELECT surfaced FROM workspace_sessions WHERE project_id = ?1",
            params![project_id],
            |row| {
                let v: i64 = row.get(0)?;
                Ok(v != 0)
            },
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            other => Err(other),
        })
    }

    pub fn delete(conn: &Connection, project_id: &str) -> Result<usize> {
        conn.execute(
            "DELETE FROM workspace_sessions WHERE project_id = ?1",
            params![project_id],
        )
    }

    /// Atomically increment the "wakes since last /compact" counter and
    /// return the new value. Used by the heartbeat wake path to decide
    /// whether to prepend `/compact` to the wake message every N wakes.
    pub fn bump_wake_counter(conn: &Connection, project_id: &str) -> Result<i64> {
        conn.execute(
            "UPDATE workspace_sessions SET wakes_since_compact = wakes_since_compact + 1 \
             WHERE project_id = ?1",
            params![project_id],
        )?;
        let val: i64 = conn.query_row(
            "SELECT wakes_since_compact FROM workspace_sessions WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        ).unwrap_or(0);
        Ok(val)
    }

    pub fn reset_wake_counter(conn: &Connection, project_id: &str) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_sessions SET wakes_since_compact = 0 WHERE project_id = ?1",
            params![project_id],
        )
    }

    /// Stamp the daemon session_id of the live PTY currently attached
    /// to this workspace. Mirror of `AgentHeartbeat::save_active_terminal_id`.
    /// Called by `v2_spawn::handle_v2_spawn` after registering a fresh
    /// (or reusing an existing) v2 session.
    pub fn save_active_terminal_id(
        conn: &Connection,
        project_id: &str,
        terminal_id: &str,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_sessions SET active_terminal_id = ?1 \
             WHERE project_id = ?2",
            params![terminal_id, project_id],
        )
    }

    /// Null out `active_terminal_id`. Called when the chat tab's lazy
    /// re-attach observes a stale id (`/cli/sessions/lookup-by-agent`
    /// finds the recorded session no longer registered in either
    /// session map).
    #[allow(dead_code)]
    pub fn clear_active_terminal_id(conn: &Connection, project_id: &str) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_sessions SET active_terminal_id = NULL WHERE project_id = ?1",
            params![project_id],
        )
    }

    /// Null out `active_terminal_id` for every row whose recorded id
    /// matches the given value. Daemon's PTY-exit observer knows the
    /// terminal_id that died but not which row pointed at it — one
    /// UPDATE handles the lookup. Mirrors
    /// `AgentHeartbeat::clear_active_terminal_id_by_terminal`.
    pub fn clear_active_terminal_id_by_terminal(
        conn: &Connection,
        terminal_id: &str,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_sessions SET active_terminal_id = NULL \
             WHERE active_terminal_id = ?1",
            params![terminal_id],
        )
    }
}

// ── Agent Heartbeats (multi-heartbeat architecture) ────────────────────
//
// Replaces the legacy single-slot projects.heartbeat_schedule. Each row
// is one named heartbeat with its own frequency + wakeup path. Scheduler
// loop iterates enabled rows per workspace, evaluates fire eligibility,
// spawns using the row's wakeup_path. See
// .k2so/prds/multi-schedule-heartbeat.md for full design.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHeartbeat {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub frequency: String,
    pub spec_json: String,
    pub wakeup_path: String,
    pub enabled: bool,
    pub last_fired: Option<String>,
    /// Claude session id from the most recent successful spawn for this
    /// heartbeat. The next fire passes this to `--resume` so the
    /// heartbeat keeps its own dedicated chat thread instead of
    /// reusing the agent's global session.
    pub last_session_id: Option<String>,
    /// RFC3339 timestamp set when the user archives the heartbeat from
    /// Settings. NULL = active. Archived rows are hidden from the
    /// Settings list but appear in the sidebar's collapsed Archived
    /// section so chat history stays auditable.
    pub archived_at: Option<String>,
    pub created_at: i64,
    /// `forbid` | `allow` | `replace`. Mirrors K8s CronJob's
    /// concurrencyPolicy. `forbid` (default) skips a fire if the
    /// previous spawn is still in flight. See migration 0035.
    pub concurrency_policy: String,
    /// Skip-if-late window in seconds. If `now() - scheduled_fire_at`
    /// exceeds this, the tick is logged `skipped_deadline` and not
    /// spawned. Default 600s. Mirrors K8s `startingDeadlineSeconds`.
    pub starting_deadline_secs: i64,
    /// Per-spawn timeout in seconds. The async wrapper around
    /// smart_launch wraps each call in tokio::time::timeout. Default 30s
    /// — covers spawn only, not the long-running session that results.
    pub active_deadline_secs: i64,
    /// RFC3339 lease timestamp. Set by `try_acquire_heartbeat` on
    /// entry, cleared by `stamp_heartbeat_fired` on completion.
    /// Boot-time sweep clears stale leases (>5min old). NULL = idle.
    pub in_flight_started_at: Option<String>,
    /// Daemon-side terminal id of the live PTY currently running this
    /// heartbeat's claude session, or NULL when no PTY is alive.
    /// Replaces args-matching with explicit FK-style data — see
    /// migration 0036 + the heartbeat-active-session PRD.
    pub active_terminal_id: Option<String>,
    /// 0.37.8 — when true, `heartbeat_launch::smart_launch` skips its
    /// own three-branch cascade and calls
    /// `workspace_msg::deliver_live(project_path, prompt)` instead, so
    /// the WAKEUP.md prompt lands in the workspace's pinned chat
    /// session (the same JSONL the chat tab is reading) rather than
    /// the heartbeat's own saved session. The heartbeat's
    /// `last_session_id` / `active_terminal_id` stay in the DB
    /// untouched — they're just no longer targeted on new fires.
    /// Un-checking the flag restores the original behavior with the
    /// historical session intact. Default false. See migration 0043.
    pub use_workspace_session: bool,
    /// 0062 — count of consecutive failed fire-attempts (spawn error,
    /// inject error, watchdog-released hang). Reset to 0 by any
    /// successful fire (`stamp_fired_and_release`) and by a manual
    /// enable/disable toggle. At 5 the row is auto-disabled with
    /// `disabled_reason='failures'`.
    pub consecutive_failures: i64,
    /// 0062 — RFC3339 earliest time the scheduler may retry this row
    /// after a failure (exponential backoff: 1/2/4/8 min). NULL = no
    /// backoff pending. Manual launches bypass this gate.
    pub next_retry_at: Option<String>,
    /// 0062 — why `enabled` is 0, when the system (not the user)
    /// flipped it: `failures` (backoff exhaustion) or `wakeup_missing`
    /// (WAKEUP.md deleted). NULL for user-disabled rows. Cleared by
    /// any manual enable/disable so re-enabling resets the state.
    pub disabled_reason: Option<String>,
    /// 0062 — human-readable reason the evaluator can't parse this
    /// row's `frequency`/`spec_json` (a schedule that can never fire).
    /// Set on the first tick that observes the problem, cleared when
    /// the spec evaluates cleanly again or the schedule is edited.
    /// Pre-0062 this state was invisible: enabled-but-dark forever.
    pub schedule_error: Option<String>,
    /// 0073 — provider key (`claude` | `grok` | `codex` | …, the
    /// `ProviderResume` table vocabulary) that owns `last_session_id`
    /// when the user pinned this heartbeat to a SPECIFIC saved
    /// session via the delivery drop-down / `k2 heartbeat session
    /// --set`. NULL = the workspace default agent (the pre-0073
    /// behavior): the fire path probes + resumes with the default
    /// agent's adapter. Cleared alongside `last_session_id` by the
    /// self-heal path and by delivery mode `auto`.
    pub session_provider: Option<String>,
}

impl AgentHeartbeat {
    /// Validate a heartbeat name. Enforced at every insert/write path so
    /// users can't get into a weird state. See PRD § Name validation.
    pub fn validate_name(name: &str) -> Result<()> {
        if name.is_empty() {
            return Err(rusqlite::Error::InvalidParameterName(
                "heartbeat name cannot be empty".into(),
            ));
        }
        let reserved = ["default", "legacy"];
        if reserved.contains(&name) {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "heartbeat name '{}' is reserved",
                name
            )));
        }
        if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(rusqlite::Error::InvalidParameterName(
                "heartbeat name must be lowercase letters, digits, and hyphens only".into(),
            ));
        }
        if name.starts_with('-') || name.ends_with('-') {
            return Err(rusqlite::Error::InvalidParameterName(
                "heartbeat name cannot start or end with a hyphen".into(),
            ));
        }
        Ok(())
    }

    pub fn insert(
        conn: &Connection,
        id: &str,
        project_id: &str,
        name: &str,
        frequency: &str,
        spec_json: &str,
        wakeup_path: &str,
        enabled: bool,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO workspace_heartbeats \
             (id, project_id, name, frequency, spec_json, wakeup_path, enabled, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, unixepoch())",
            params![id, project_id, name, frequency, spec_json, wakeup_path, enabled as i64],
        )?;
        Ok(())
    }

    /// Column list for SELECTs. Centralised so adding a new column means
    /// updating one constant + `from_row`, not five query strings.
    const COLS: &'static str = "id, project_id, name, frequency, spec_json, wakeup_path, enabled, last_fired, last_session_id, archived_at, created_at, concurrency_policy, starting_deadline_secs, active_deadline_secs, in_flight_started_at, active_terminal_id, use_workspace_session, consecutive_failures, next_retry_at, disabled_reason, schedule_error, session_provider";

    pub fn get_by_name(conn: &Connection, project_id: &str, name: &str) -> Result<Option<AgentHeartbeat>> {
        let sql = format!(
            "SELECT {} FROM workspace_heartbeats WHERE project_id = ?1 AND name = ?2",
            Self::COLS,
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query_map(params![project_id, name], Self::from_row)?;
        match rows.next() {
            Some(Ok(h)) => Ok(Some(h)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    pub fn list_by_project(conn: &Connection, project_id: &str) -> Result<Vec<AgentHeartbeat>> {
        let sql = format!(
            "SELECT {} FROM workspace_heartbeats WHERE project_id = ?1 ORDER BY name",
            Self::COLS,
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![project_id], Self::from_row)?;
        rows.collect()
    }

    /// Active (non-archived) rows across ALL projects, with the project
    /// name + path joined in. Used by the system-wide Heartbeats settings
    /// page (0.38.3) so the operator can see + toggle every configured
    /// heartbeat from one place rather than walking workspace-by-workspace.
    pub fn list_all_active_with_project(
        conn: &Connection,
    ) -> Result<Vec<(AgentHeartbeat, String, String)>> {
        let sql = format!(
            "SELECT {}, p.name AS project_name, p.path AS project_path \
             FROM workspace_heartbeats h \
             JOIN projects p ON p.id = h.project_id \
             WHERE h.archived_at IS NULL \
             ORDER BY p.name, h.name",
            // Qualify each column with the heartbeat alias so the JOIN
            // doesn't collide on the duplicate `name` column.
            Self::COLS
                .split(", ")
                .map(|c| format!("h.{}", c))
                .collect::<Vec<_>>()
                .join(", "),
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
            let hb = Self::from_row(row)?;
            // Two extra columns appended in the SELECT above. Their
            // indices follow the AgentHeartbeat fields (which Self::COLS
            // produced) — 22 fields + project_name (22) + project_path (23).
            let project_name: String = row.get(22)?;
            let project_path: String = row.get(23)?;
            Ok((hb, project_name, project_path))
        })?;
        rows.collect()
    }

    /// Active (non-archived) rows for a project. The Settings list and the
    /// sidebar's Live/Resumable/Scheduled sections both use this.
    pub fn list_active(conn: &Connection, project_id: &str) -> Result<Vec<AgentHeartbeat>> {
        let sql = format!(
            "SELECT {} FROM workspace_heartbeats \
             WHERE project_id = ?1 AND archived_at IS NULL ORDER BY name",
            Self::COLS,
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![project_id], Self::from_row)?;
        rows.collect()
    }

    /// Archived rows for a project. The sidebar's collapsed Archived
    /// section uses this; ordered by archive recency (newest first).
    pub fn list_archived(conn: &Connection, project_id: &str) -> Result<Vec<AgentHeartbeat>> {
        let sql = format!(
            "SELECT {} FROM workspace_heartbeats \
             WHERE project_id = ?1 AND archived_at IS NOT NULL \
             ORDER BY archived_at DESC",
            Self::COLS,
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![project_id], Self::from_row)?;
        rows.collect()
    }

    pub fn list_enabled(conn: &Connection, project_id: &str) -> Result<Vec<AgentHeartbeat>> {
        // Tick-time evaluator. Skip archived heartbeats — they no longer
        // fire on schedule even if `enabled` was never flipped before
        // archiving.
        let sql = format!(
            "SELECT {} FROM workspace_heartbeats \
             WHERE project_id = ?1 AND enabled = 1 AND archived_at IS NULL \
             ORDER BY name",
            Self::COLS,
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![project_id], Self::from_row)?;
        rows.collect()
    }

    pub fn set_enabled(conn: &Connection, project_id: &str, name: &str, enabled: bool) -> Result<usize> {
        // 0062 — a manual toggle is an operator statement of intent, so
        // it clears the failure-backoff state: re-enabling a row that was
        // auto-disabled after repeated failures gives it a clean slate
        // (counter, retry window, and the disabled_reason badge all reset).
        conn.execute(
            "UPDATE workspace_heartbeats \
             SET enabled = ?1, disabled_reason = NULL, \
                 consecutive_failures = 0, next_retry_at = NULL \
             WHERE project_id = ?2 AND name = ?3",
            params![enabled as i64, project_id, name],
        )
    }

    /// 0062 — system-initiated disable (backoff exhaustion, missing
    /// WAKEUP.md). Unlike [`Self::set_enabled`] this RECORDS why via
    /// `disabled_reason` so the UI can render a distinct badge instead
    /// of a silently-flipped toggle. A manual re-enable clears it.
    pub fn auto_disable(
        conn: &Connection,
        project_id: &str,
        name: &str,
        reason: &str,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_heartbeats SET enabled = 0, disabled_reason = ?1 \
             WHERE project_id = ?2 AND name = ?3",
            params![reason, project_id, name],
        )
    }

    /// 0062 — record one failed fire-attempt: bump the consecutive
    /// counter and stamp the backoff window. Returns the NEW counter
    /// value so the caller can decide whether the auto-disable
    /// threshold was crossed. All writes on the shared connection are
    /// serialized by the process-wide mutex, so bump-then-read is safe.
    pub fn note_fire_failure(
        conn: &Connection,
        project_id: &str,
        name: &str,
        next_retry_at: Option<&str>,
    ) -> Result<i64> {
        conn.execute(
            "UPDATE workspace_heartbeats \
             SET consecutive_failures = consecutive_failures + 1, next_retry_at = ?1 \
             WHERE project_id = ?2 AND name = ?3",
            params![next_retry_at, project_id, name],
        )?;
        conn.query_row(
            "SELECT consecutive_failures FROM workspace_heartbeats \
             WHERE project_id = ?1 AND name = ?2",
            params![project_id, name],
            |r| r.get(0),
        )
    }

    /// 0062 — set (Some) or clear (None) the visible schedule-error
    /// state. The tick evaluator writes this on the transition into /
    /// out of "spec unparseable" so an impossible schedule shows up in
    /// Settings instead of sitting enabled-but-dark forever.
    pub fn set_schedule_error(
        conn: &Connection,
        project_id: &str,
        name: &str,
        error: Option<&str>,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_heartbeats SET schedule_error = ?1 \
             WHERE project_id = ?2 AND name = ?3",
            params![error, project_id, name],
        )
    }

    pub fn update_schedule(
        conn: &Connection,
        project_id: &str,
        name: &str,
        frequency: &str,
        spec_json: &str,
    ) -> Result<usize> {
        // 0062 — editing the schedule clears any recorded schedule_error;
        // the next tick re-evaluates the new spec and re-flags if it's
        // still unparseable.
        conn.execute(
            "UPDATE workspace_heartbeats \
             SET frequency = ?1, spec_json = ?2, schedule_error = NULL \
             WHERE project_id = ?3 AND name = ?4",
            params![frequency, spec_json, project_id, name],
        )
    }

    pub fn update_wakeup_path(
        conn: &Connection,
        project_id: &str,
        name: &str,
        wakeup_path: &str,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_heartbeats SET wakeup_path = ?1 WHERE project_id = ?2 AND name = ?3",
            params![wakeup_path, project_id, name],
        )
    }

    /// Stamp last_fired. Only called on *successful* spawn — lock-skips
    /// deliberately do NOT stamp, so the heartbeat stays eligible for
    /// the next tick. See PRD § last_fired semantics.
    pub fn stamp_last_fired(conn: &Connection, project_id: &str, name: &str) -> Result<usize> {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE workspace_heartbeats SET last_fired = ?1 WHERE project_id = ?2 AND name = ?3",
            params![now, project_id, name],
        )
    }

    /// Record the Claude session id from a successful heartbeat spawn.
    /// The next fire's `--resume` target. Called by `spawn_wake_headless`
    /// alongside the existing `workspace_sessions::save_session_id` write.
    pub fn save_session_id(
        conn: &Connection,
        project_id: &str,
        name: &str,
        session_id: &str,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_heartbeats SET last_session_id = ?1 \
             WHERE project_id = ?2 AND name = ?3",
            params![session_id, project_id, name],
        )
    }

    /// Null out `last_session_id`. Called by the smart_launch self-heal
    /// path when the saved session_id points at a JSONL that no longer
    /// exists on disk (daemon-restart-during-spawn race) — clearing
    /// here lets the next fire fall through to fresh_fire and pick a
    /// new pinned UUID instead of looping on `claude --resume <ghost>`.
    /// 0073: also nulls `session_provider` — a ghost id's provider
    /// pin is meaningless once the id itself is gone, and leaving it
    /// behind would make the NEXT saved id probe the wrong store.
    pub fn clear_session_id(
        conn: &Connection,
        project_id: &str,
        name: &str,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_heartbeats \
             SET last_session_id = NULL, session_provider = NULL \
             WHERE project_id = ?1 AND name = ?2",
            params![project_id, name],
        )
    }

    /// 0073 — set (or clear, both `None`) the heartbeat's delivery
    /// session: `last_session_id` + the provider that owns it, in one
    /// statement so a reader never observes a half-written pair.
    /// Used by `k2so_heartbeat_set_session` (mode `session` writes
    /// both; mode `auto` clears both).
    pub fn set_session(
        conn: &Connection,
        project_id: &str,
        name: &str,
        session_id: Option<&str>,
        provider: Option<&str>,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_heartbeats \
             SET last_session_id = ?1, session_provider = ?2 \
             WHERE project_id = ?3 AND name = ?4",
            params![session_id, provider, project_id, name],
        )
    }

    /// Record the daemon-side terminal id of the live PTY currently
    /// attached to this heartbeat. NULL when no PTY is alive (cold
    /// heartbeat, post-exit, post-daemon-restart). Replaces the
    /// args-matching `find_live_for_resume` heuristic with explicit
    /// data — see migration 0036 + the heartbeat-active-session PRD.
    pub fn save_active_terminal_id(
        conn: &Connection,
        project_id: &str,
        name: &str,
        terminal_id: &str,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_heartbeats SET active_terminal_id = ?1 \
             WHERE project_id = ?2 AND name = ?3",
            params![terminal_id, project_id, name],
        )
    }

    /// Null out `active_terminal_id`. Called when the PTY exits
    /// (child-exit observer), the watchdog kills the session, or when
    /// `openHeartbeatTab`'s lazy cleanup observes the terminal_id no
    /// longer exists in the daemon's session map.
    pub fn clear_active_terminal_id(
        conn: &Connection,
        project_id: &str,
        name: &str,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_heartbeats SET active_terminal_id = NULL \
             WHERE project_id = ?1 AND name = ?2",
            params![project_id, name],
        )
    }

    /// Null out `active_terminal_id` for every row whose terminal id
    /// matches the given value. Used by the daemon-side `PtyExited`
    /// listener — we know the terminal_id that died but not which
    /// heartbeat row pointed at it. One UPDATE handles the lookup.
    pub fn clear_active_terminal_id_by_terminal(
        conn: &Connection,
        terminal_id: &str,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_heartbeats SET active_terminal_id = NULL \
             WHERE active_terminal_id = ?1",
            params![terminal_id],
        )
    }

    /// List heartbeats with non-NULL `active_terminal_id`. Used by the
    /// boot-time sweep to reconcile column state with `v2_session_map`
    /// after a daemon restart wipes the in-memory map.
    pub fn list_with_active_terminal(conn: &Connection) -> Result<Vec<(String, String, String)>> {
        let mut stmt = conn.prepare(
            "SELECT project_id, name, active_terminal_id FROM workspace_heartbeats \
             WHERE active_terminal_id IS NOT NULL",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// 0.39.39 (#677.1) — find the (project_id, name) of every heartbeat
    /// row currently pointing at `terminal_id`. The daemon's PTY-exit
    /// chokepoint (`v2_session_map::unregister`) knows the terminal_id
    /// that died but not which heartbeat owned it; this resolves the
    /// identity so it can broadcast `HeartbeatStateChanged{live:false}`
    /// BEFORE nulling the column. Usually 0 or 1 rows.
    pub fn find_by_active_terminal(
        conn: &Connection,
        terminal_id: &str,
    ) -> Result<Vec<(String, String)>> {
        let mut stmt = conn.prepare(
            "SELECT project_id, name FROM workspace_heartbeats \
             WHERE active_terminal_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![terminal_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Soft-archive: set archived_at to now. Idempotent — re-archiving an
    /// already-archived row is a no-op (timestamp unchanged). Called by
    /// the Settings "Archive" button (replaced the previous hard-delete
    /// "Remove" behaviour in 0.36.0).
    pub fn archive(conn: &Connection, project_id: &str, name: &str) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_heartbeats SET archived_at = ?1 \
             WHERE project_id = ?2 AND name = ?3 AND archived_at IS NULL",
            params![chrono::Utc::now().to_rfc3339(), project_id, name],
        )
    }

    /// Restore a soft-archived heartbeat. Reserved for a future "Restore
    /// from Archive" UI affordance — no caller in 0.36.0.
    pub fn unarchive(conn: &Connection, project_id: &str, name: &str) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_heartbeats SET archived_at = NULL \
             WHERE project_id = ?1 AND name = ?2",
            params![project_id, name],
        )
    }

    pub fn delete(conn: &Connection, project_id: &str, name: &str) -> Result<usize> {
        conn.execute(
            "DELETE FROM workspace_heartbeats WHERE project_id = ?1 AND name = ?2",
            params![project_id, name],
        )
    }

    /// Atomic claim of a heartbeat row's in-flight lease.
    ///
    /// Returns `true` if this caller won the race and should proceed to
    /// spawn; `false` if the row is already in-flight (under `forbid`)
    /// and this fire should be skipped.
    ///
    /// Mirrors `WorkspaceSession::try_acquire_running` — `BEGIN IMMEDIATE`
    /// upgrades the connection to a write lock at BEGIN time so two
    /// concurrent readers can't both think they can proceed. This is
    /// the load-bearing fix for the pre-existing TOCTOU between the
    /// scheduler's `is_agent_locked` check and the spawn that follows.
    ///
    /// Honors `concurrency_policy`:
    /// - `forbid` (default): refuse if `in_flight_started_at IS NOT NULL`
    /// - `allow`: always succeed (still leases for sweep semantics)
    /// - `replace`: same as `allow` here; the caller is responsible for
    ///   killing the prior spawn before this returns. (P5.5 wires up
    ///   the kill side; until then `replace` behaves as `allow`.)
    ///
    /// Stale leases (left behind by a daemon crash mid-spawn) are
    /// handled by the boot-time sweep in `sweep_stale_leases`, not
    /// here — keeping the CAS path tiny.
    pub fn try_acquire_heartbeat(
        conn: &Connection,
        project_id: &str,
        name: &str,
    ) -> Result<bool> {
        conn.execute_batch("BEGIN IMMEDIATE;")?;

        let row: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT concurrency_policy, in_flight_started_at \
                 FROM workspace_heartbeats \
                 WHERE project_id = ?1 AND name = ?2",
                params![project_id, name],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .ok();

        let (policy, in_flight) = match row {
            Some(r) => r,
            None => {
                conn.execute_batch("ROLLBACK;")?;
                return Ok(false);
            }
        };

        if policy == "forbid" && in_flight.is_some() {
            conn.execute_batch("ROLLBACK;")?;
            return Ok(false);
        }

        let now = chrono::Utc::now().to_rfc3339();
        let result = conn.execute(
            "UPDATE workspace_heartbeats SET in_flight_started_at = ?1 \
             WHERE project_id = ?2 AND name = ?3",
            params![now, project_id, name],
        );

        match result {
            Ok(_) => {
                conn.execute_batch("COMMIT;")?;
                Ok(true)
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Release the in-flight lease without stamping `last_fired`.
    ///
    /// Called when smart_launch returned an error — the heartbeat
    /// stays eligible for the next tick. Symmetric counterpart to
    /// `stamp_heartbeat_fired` for the failure path.
    pub fn release_heartbeat_lease(
        conn: &Connection,
        project_id: &str,
        name: &str,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_heartbeats SET in_flight_started_at = NULL \
             WHERE project_id = ?1 AND name = ?2",
            params![project_id, name],
        )
    }

    /// Combined success path: stamp `last_fired` AND clear the lease in
    /// a single statement, so a tick never observes the row in a
    /// half-finished state. 0062: a success also resets the
    /// consecutive-failure counter + backoff window.
    pub fn stamp_fired_and_release(
        conn: &Connection,
        project_id: &str,
        name: &str,
    ) -> Result<usize> {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE workspace_heartbeats \
             SET last_fired = ?1, in_flight_started_at = NULL, \
                 consecutive_failures = 0, next_retry_at = NULL \
             WHERE project_id = ?2 AND name = ?3",
            params![now, project_id, name],
        )
    }

    /// Boot-time recovery sweep. Clears `in_flight_started_at` on rows
    /// whose lease is older than `older_than_secs` — these were left
    /// behind by a daemon that crashed between lease acquisition and
    /// completion. Without this, a crashed-mid-spawn row would stay
    /// locked forever under `concurrency_policy='forbid'`.
    ///
    /// River + Oban use the same pattern. Threshold passed in (rather
    /// than hardcoded) so callers can tune it to their longest
    /// expected `active_deadline_secs`.
    pub fn sweep_stale_leases(conn: &Connection, older_than_secs: i64) -> Result<usize> {
        let cutoff = (chrono::Utc::now()
            - chrono::Duration::seconds(older_than_secs))
        .to_rfc3339();
        conn.execute(
            "UPDATE workspace_heartbeats SET in_flight_started_at = NULL \
             WHERE in_flight_started_at IS NOT NULL \
               AND in_flight_started_at < ?1",
            params![cutoff],
        )
    }

    /// 0062 — conditional single-row lease release for the spawn-timeout
    /// watchdog. Clears `in_flight_started_at` ONLY if the lease is
    /// older than `older_than_secs`, so a lease re-acquired by a LATER
    /// fire attempt (or a spawn that completed and re-fired in the
    /// interim) is never clobbered. Returns the number of rows cleared
    /// (0 = the hung spawn actually finished and handled its own lease;
    /// 1 = the watchdog un-wedged the row).
    pub fn release_lease_if_stale(
        conn: &Connection,
        project_id: &str,
        name: &str,
        older_than_secs: i64,
    ) -> Result<usize> {
        let cutoff = (chrono::Utc::now()
            - chrono::Duration::seconds(older_than_secs))
        .to_rfc3339();
        conn.execute(
            "UPDATE workspace_heartbeats SET in_flight_started_at = NULL \
             WHERE project_id = ?1 AND name = ?2 \
               AND in_flight_started_at IS NOT NULL \
               AND in_flight_started_at < ?3",
            params![project_id, name, cutoff],
        )
    }

    fn from_row(row: &rusqlite::Row<'_>) -> Result<AgentHeartbeat> {
        Ok(AgentHeartbeat {
            id: row.get(0)?,
            project_id: row.get(1)?,
            name: row.get(2)?,
            frequency: row.get(3)?,
            spec_json: row.get(4)?,
            wakeup_path: row.get(5)?,
            enabled: row.get::<_, i64>(6)? == 1,
            last_fired: row.get(7)?,
            last_session_id: row.get(8)?,
            archived_at: row.get(9)?,
            created_at: row.get(10)?,
            concurrency_policy: row.get(11)?,
            starting_deadline_secs: row.get(12)?,
            active_deadline_secs: row.get(13)?,
            in_flight_started_at: row.get(14)?,
            active_terminal_id: row.get(15)?,
            use_workspace_session: row.get::<_, i64>(16)? == 1,
            consecutive_failures: row.get(17)?,
            next_retry_at: row.get(18)?,
            disabled_reason: row.get(19)?,
            schedule_error: row.get(20)?,
            session_provider: row.get(21)?,
        })
    }

    /// 0.37.8 — flip the per-heartbeat opt-in to deliver WAKEUP.md
    /// into the workspace's pinned chat session via
    /// `workspace_msg::deliver_live`. See migration 0043 + the field
    /// doc on `AgentHeartbeat::use_workspace_session`.
    pub fn set_use_workspace_session(
        conn: &Connection,
        project_id: &str,
        name: &str,
        enabled: bool,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE workspace_heartbeats SET use_workspace_session = ?1 \
             WHERE project_id = ?2 AND name = ?3",
            params![enabled as i64, project_id, name],
        )
    }
}

// ── Workspace Relations ─────────────────────────────────────────────────

/// Cross-workspace relationship. A custom agent workspace can oversee
/// one or more workspace manager workspaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRelation {
    pub id: String,
    pub source_project_id: String,
    pub target_project_id: String,
    pub relation_type: String,
    pub created_at: i64,
}

impl WorkspaceRelation {
    pub fn create(
        conn: &Connection,
        id: &str,
        source_project_id: &str,
        target_project_id: &str,
        relation_type: &str,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO workspace_relations (id, source_project_id, target_project_id, relation_type, created_at) \
             VALUES (?1, ?2, ?3, ?4, unixepoch())",
            params![id, source_project_id, target_project_id, relation_type],
        )?;
        Ok(())
    }

    /// Workspaces that this project oversees (source → targets).
    pub fn list_for_source(conn: &Connection, project_id: &str) -> Result<Vec<WorkspaceRelation>> {
        let mut stmt = conn.prepare(
            "SELECT id, source_project_id, target_project_id, relation_type, created_at \
             FROM workspace_relations WHERE source_project_id = ?1 ORDER BY created_at"
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(WorkspaceRelation {
                id: row.get(0)?,
                source_project_id: row.get(1)?,
                target_project_id: row.get(2)?,
                relation_type: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Custom agents that oversee this project (target ← sources).
    pub fn list_for_target(conn: &Connection, project_id: &str) -> Result<Vec<WorkspaceRelation>> {
        let mut stmt = conn.prepare(
            "SELECT id, source_project_id, target_project_id, relation_type, created_at \
             FROM workspace_relations WHERE target_project_id = ?1 ORDER BY created_at"
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(WorkspaceRelation {
                id: row.get(0)?,
                source_project_id: row.get(1)?,
                target_project_id: row.get(2)?,
                relation_type: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<usize> {
        conn.execute("DELETE FROM workspace_relations WHERE id = ?1", params![id])
    }
}

// ── Workspace Remote Connections (GAP #3) ───────────────────────────────

/// A CROSS-DAEMON connection: a local source workspace connected to a
/// remote `<agent>@<full-host>` (e.g. `ai@rpm.k2.dev`). Unlike
/// [`WorkspaceRelation`] (LOCAL project→project, same daemon), the peer
/// here lives on a DIFFERENT daemon and has no `projects` row locally —
/// it's addressed by host. This is the gate for agent-initiated
/// cross-server sends (see `connections::is_remote_connection` +
/// `federation::handle_send`). Storage migration 0055.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRemoteConnection {
    pub id: String,
    pub source_project_id: String,
    /// Canonical `<agent>@<host>` address.
    pub remote_addr: String,
    pub host: String,
    pub agent: String,
    /// Paired peer's fingerprint when known; `None` until resolved.
    pub peer_fingerprint: Option<String>,
    pub created_at: i64,
}

impl WorkspaceRemoteConnection {
    /// Insert a remote connection. Idempotent at the storage layer via the
    /// `UNIQUE(source_project_id, remote_addr)` constraint — callers should
    /// still check [`exists`](Self::exists) first to surface a friendly
    /// no-op rather than relying on the constraint error.
    pub fn create(
        conn: &Connection,
        id: &str,
        source_project_id: &str,
        remote_addr: &str,
        host: &str,
        agent: &str,
        peer_fingerprint: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO workspace_remote_connections \
             (id, source_project_id, remote_addr, host, agent, peer_fingerprint, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())",
            params![id, source_project_id, remote_addr, host, agent, peer_fingerprint],
        )?;
        Ok(())
    }

    /// All remote connections for a source workspace, oldest first.
    pub fn list_for_source(
        conn: &Connection,
        source_project_id: &str,
    ) -> Result<Vec<WorkspaceRemoteConnection>> {
        let mut stmt = conn.prepare(
            "SELECT id, source_project_id, remote_addr, host, agent, peer_fingerprint, created_at \
             FROM workspace_remote_connections WHERE source_project_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![source_project_id], |row| {
            Ok(WorkspaceRemoteConnection {
                id: row.get(0)?,
                source_project_id: row.get(1)?,
                remote_addr: row.get(2)?,
                host: row.get(3)?,
                agent: row.get(4)?,
                peer_fingerprint: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    /// Whether `(source_project_id, remote_addr)` is already connected.
    /// Backs the cross-daemon send gate — keep it cheap (COUNT, no row
    /// materialization).
    ///
    /// The `remote_addr` (`<agent>@<host>`) match is CASE-INSENSITIVE
    /// (`LOWER(...) = LOWER(...)`): hostnames are case-insensitive, and the
    /// agent token can differ in case between the folder-basename-derived
    /// reverse row (e.g. `Cortana@host`) and the agent's real display name
    /// (`cortana@host`). New rows are stored lowercased; pre-existing capital
    /// rows still match here with no migration needed.
    pub fn exists(conn: &Connection, source_project_id: &str, remote_addr: &str) -> Result<bool> {
        conn.query_row(
            "SELECT COUNT(*) > 0 FROM workspace_remote_connections \
             WHERE source_project_id = ?1 AND LOWER(remote_addr) = LOWER(?2)",
            params![source_project_id, remote_addr],
            |row| row.get(0),
        )
    }

    /// Delete the `(source_project_id, remote_addr)` connection. Returns the
    /// number of rows removed (0 when there was nothing to remove). The
    /// `remote_addr` match is CASE-INSENSITIVE, matching [`exists`](Self::exists).
    pub fn delete(
        conn: &Connection,
        source_project_id: &str,
        remote_addr: &str,
    ) -> Result<usize> {
        conn.execute(
            "DELETE FROM workspace_remote_connections \
             WHERE source_project_id = ?1 AND LOWER(remote_addr) = LOWER(?2)",
            params![source_project_id, remote_addr],
        )
    }
}

// ── Activity Feed ───────────────────────────────────────────────────────

/// Audit trail entry for workspace agent communications and lifecycle
/// events. Post-0.37.0: `actor` is a free-form string (`agent`, `user`,
/// `heartbeat`, `cli`, `sms-bridge`, an external workspace path) so
/// cross-workspace events from external systems can land here without
/// a fake "agent name" placeholder. `from_workspace` / `to_workspace`
/// are workspace path/id strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityFeedEntry {
    pub id: i64,
    pub project_id: String,
    pub actor: Option<String>,
    pub event_type: String,
    pub from_workspace: Option<String>,
    pub to_workspace: Option<String>,
    pub to_project_id: Option<String>,
    pub summary: Option<String>,
    pub metadata: Option<String>,
    pub created_at: i64,
}

impl ActivityFeedEntry {
    pub fn insert(
        conn: &Connection,
        project_id: &str,
        actor: Option<&str>,
        event_type: &str,
        from_workspace: Option<&str>,
        to_workspace: Option<&str>,
        to_project_id: Option<&str>,
        summary: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<i64> {
        // prepare_cached keeps the compiled statement in rusqlite's per-
        // connection LRU cache (default 16 slots). activity_feed appends
        // fire on every agent event; criterion bench at P1.3 showed ~25%
        // speedup vs rebuilding the statement each call.
        let mut stmt = conn.prepare_cached(
            "INSERT INTO activity_feed (project_id, actor, event_type, from_workspace, to_workspace, to_project_id, summary, metadata, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch())",
        )?;
        stmt.execute(params![project_id, actor, event_type, from_workspace, to_workspace, to_project_id, summary, metadata])?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_by_project(conn: &Connection, project_id: &str, limit: i64, offset: i64) -> Result<Vec<ActivityFeedEntry>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, actor, event_type, from_workspace, to_workspace, to_project_id, summary, metadata, created_at \
             FROM activity_feed WHERE project_id = ?1 ORDER BY created_at DESC LIMIT ?2 OFFSET ?3"
        )?;
        let rows = stmt.query_map(params![project_id, limit, offset], |row| {
            Ok(ActivityFeedEntry {
                id: row.get(0)?,
                project_id: row.get(1)?,
                actor: row.get(2)?,
                event_type: row.get(3)?,
                from_workspace: row.get(4)?,
                to_workspace: row.get(5)?,
                to_project_id: row.get(6)?,
                summary: row.get(7)?,
                metadata: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?;
        rows.collect()
    }

    pub fn list_by_actor(conn: &Connection, project_id: &str, actor: &str, limit: i64) -> Result<Vec<ActivityFeedEntry>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, actor, event_type, from_workspace, to_workspace, to_project_id, summary, metadata, created_at \
             FROM activity_feed WHERE project_id = ?1 AND (actor = ?2 OR from_workspace = ?2 OR to_workspace = ?2) \
             ORDER BY created_at DESC LIMIT ?3"
        )?;
        let rows = stmt.query_map(params![project_id, actor, limit], |row| {
            Ok(ActivityFeedEntry {
                id: row.get(0)?,
                project_id: row.get(1)?,
                actor: row.get(2)?,
                event_type: row.get(3)?,
                from_workspace: row.get(4)?,
                to_workspace: row.get(5)?,
                to_project_id: row.get(6)?,
                summary: row.get(7)?,
                metadata: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?;
        rows.collect()
    }
}

/// Convenience function to log an activity feed entry.
/// Used by CLI route handlers in agent_hooks.rs.
pub fn log_activity(
    conn: &Connection,
    project_id: &str,
    actor: Option<&str>,
    event_type: &str,
    from_workspace: Option<&str>,
    to_workspace: Option<&str>,
    to_project_id: Option<&str>,
    summary: Option<&str>,
) {
    let _ = ActivityFeedEntry::insert(conn, project_id, actor, event_type, from_workspace, to_workspace, to_project_id, summary, None);
}

/// Get unread messages addressed to a workspace identifier.
///
/// Match condition: a row belongs to this workspace's inbox when its
/// `to_workspace` equals the caller-supplied target (typically the
/// workspace's primary agent name resolved via `find_primary_agent`)
/// AND either `project_id` or `to_project_id` matches the workspace.
///
/// 0.39.0f: the pre-unification fallback that treated
/// `to_workspace IS NULL` as a synonym for `'__lead__'` is gone —
/// migration 0049 rewrote every `'__lead__'` row to the workspace's
/// primary agent name (or NULL for orphans), so the SQL no longer
/// needs to special-case the legacy routing sentinel.
pub fn get_unread_messages(
    conn: &Connection,
    project_id: &str,
    workspace_target: &str,
) -> Result<Vec<ActivityFeedEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, actor, event_type, from_workspace, to_workspace, to_project_id, summary, metadata, created_at \
         FROM activity_feed \
         WHERE to_workspace = ?1 \
         AND (project_id = ?2 OR to_project_id = ?2) \
         AND event_type IN ('message.sent', 'message.delivered') \
         AND read = 0 \
         ORDER BY created_at ASC"
    )?;
    let rows = stmt.query_map(params![workspace_target, project_id], |row| {
        Ok(ActivityFeedEntry {
            id: row.get(0)?,
            project_id: row.get(1)?,
            actor: row.get(2)?,
            event_type: row.get(3)?,
            from_workspace: row.get(4)?,
            to_workspace: row.get(5)?,
            to_project_id: row.get(6)?,
            summary: row.get(7)?,
            metadata: row.get(8)?,
            created_at: row.get(9)?,
        })
    })?;
    rows.collect()
}

/// Mark messages addressed to a workspace target as read.
///
/// 0.39.0f: dropped the `to_workspace IS NULL AND ?1 = '__lead__'`
/// fallback for the same reason as `get_unread_messages` above —
/// migration 0049 rewrote any pre-unification rows.
pub fn mark_messages_read(
    conn: &Connection,
    project_id: &str,
    workspace_target: &str,
) -> Result<usize> {
    conn.execute(
        "UPDATE activity_feed SET read = 1 \
         WHERE to_workspace = ?1 \
         AND (project_id = ?2 OR to_project_id = ?2) \
         AND event_type IN ('message.sent', 'message.delivered') \
         AND read = 0",
        params![workspace_target, project_id],
    )
}

// ── Heartbeat audit log ────────────────────────────────────────────────

/// One row per scheduler decision. Written on every tick — both for
/// agents that were launched and for agents that were skipped — so users
/// can see exactly why each agent did or didn't wake.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatFire {
    pub id: i64,
    pub project_id: String,
    pub agent_name: Option<String>,
    pub schedule_name: Option<String>,
    pub fired_at: String,
    pub mode: String,
    pub decision: String,
    pub reason: Option<String>,
    pub inbox_priority: Option<String>,
    pub inbox_count: Option<i64>,
    pub duration_ms: Option<i64>,
}

impl HeartbeatFire {
    /// Insert an audit row. `schedule_name` is the multi-heartbeat name
    /// (the `workspace_heartbeats.name`); None for legacy fires that predate
    /// the multi-heartbeat system or aren't tied to a specific heartbeat.
    pub fn insert(
        conn: &Connection,
        project_id: &str,
        agent_name: Option<&str>,
        mode: &str,
        decision: &str,
        reason: Option<&str>,
        inbox_priority: Option<&str>,
        inbox_count: Option<i64>,
        duration_ms: Option<i64>,
    ) -> Result<i64> {
        Self::insert_with_schedule(
            conn, project_id, agent_name, None,
            mode, decision, reason, inbox_priority, inbox_count, duration_ms,
        )
    }

    /// Insert an audit row with an explicit schedule_name — used by the
    /// multi-heartbeat tick so `k2so heartbeat status <name>` can filter
    /// cleanly. schedule_name is denormalized TEXT (NOT a FK to
    /// workspace_heartbeats.name) so audit rows survive heartbeat deletion.
    pub fn insert_with_schedule(
        conn: &Connection,
        project_id: &str,
        agent_name: Option<&str>,
        schedule_name: Option<&str>,
        mode: &str,
        decision: &str,
        reason: Option<&str>,
        inbox_priority: Option<&str>,
        inbox_count: Option<i64>,
        duration_ms: Option<i64>,
    ) -> Result<i64> {
        // Fires on every heartbeat tick — high-volume INSERT, cached.
        let mut stmt = conn.prepare_cached(
            "INSERT INTO heartbeat_fires \
             (project_id, agent_name, fired_at, mode, decision, reason, inbox_priority, inbox_count, duration_ms, schedule_name) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        stmt.execute(params![
            project_id,
            agent_name,
            chrono::Local::now().to_rfc3339(),
            mode,
            decision,
            reason,
            inbox_priority,
            inbox_count,
            duration_ms,
            schedule_name,
        ])?;
        Ok(conn.last_insert_rowid())
    }

    /// Return the most recent `limit` fire rows across **all** projects
    /// joined with project name. Used by the system-wide Heartbeats
    /// settings page (0.38.3) as the rightmost universal audit log.
    pub fn list_all_recent_with_project(
        conn: &Connection,
        limit: i64,
    ) -> Result<Vec<(HeartbeatFire, String)>> {
        let mut stmt = conn.prepare(
            "SELECT h.id, h.project_id, h.agent_name, h.schedule_name, h.fired_at, h.mode, \
                    h.decision, h.reason, h.inbox_priority, h.inbox_count, h.duration_ms, \
                    p.name AS project_name \
             FROM heartbeat_fires h JOIN projects p ON p.id = h.project_id \
             ORDER BY h.fired_at DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            let fire = HeartbeatFire {
                id: row.get(0)?,
                project_id: row.get(1)?,
                agent_name: row.get(2)?,
                schedule_name: row.get(3)?,
                fired_at: row.get(4)?,
                mode: row.get(5)?,
                decision: row.get(6)?,
                reason: row.get(7)?,
                inbox_priority: row.get(8)?,
                inbox_count: row.get(9)?,
                duration_ms: row.get(10)?,
            };
            let project_name: String = row.get(11)?;
            Ok((fire, project_name))
        })?;
        rows.collect()
    }

    /// Return the most recent `limit` fire rows for a project.
    pub fn list_by_project(
        conn: &Connection,
        project_id: &str,
        limit: i64,
    ) -> Result<Vec<HeartbeatFire>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, agent_name, schedule_name, fired_at, mode, decision, reason, \
                    inbox_priority, inbox_count, duration_ms \
             FROM heartbeat_fires WHERE project_id = ?1 \
             ORDER BY fired_at DESC LIMIT ?2"
        )?;
        let rows = stmt.query_map(params![project_id, limit], |row| {
            Ok(HeartbeatFire {
                id: row.get(0)?,
                project_id: row.get(1)?,
                agent_name: row.get(2)?,
                schedule_name: row.get(3)?,
                fired_at: row.get(4)?,
                mode: row.get(5)?,
                decision: row.get(6)?,
                reason: row.get(7)?,
                inbox_priority: row.get(8)?,
                inbox_count: row.get(9)?,
                duration_ms: row.get(10)?,
            })
        })?;
        rows.collect()
    }

    /// Filter fire rows by schedule_name — powers `k2so heartbeat status <name>`.
    pub fn list_by_schedule_name(
        conn: &Connection,
        project_id: &str,
        schedule_name: &str,
        limit: i64,
    ) -> Result<Vec<HeartbeatFire>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, agent_name, schedule_name, fired_at, mode, decision, reason, \
                    inbox_priority, inbox_count, duration_ms \
             FROM heartbeat_fires WHERE project_id = ?1 AND schedule_name = ?2 \
             ORDER BY fired_at DESC LIMIT ?3"
        )?;
        let rows = stmt.query_map(params![project_id, schedule_name, limit], |row| {
            Ok(HeartbeatFire {
                id: row.get(0)?,
                project_id: row.get(1)?,
                agent_name: row.get(2)?,
                schedule_name: row.get(3)?,
                fired_at: row.get(4)?,
                mode: row.get(5)?,
                decision: row.get(6)?,
                reason: row.get(7)?,
                inbox_priority: row.get(8)?,
                inbox_count: row.get(9)?,
                duration_ms: row.get(10)?,
            })
        })?;
        rows.collect()
    }

    /// Delete fire rows older than the given RFC3339 timestamp. Returns
    /// the number of rows removed. 0062 wires this into daemon boot
    /// with a 90-day cutoff so `heartbeat_fires` stops growing
    /// unboundedly (pre-0062 it had zero callers).
    pub fn prune_before(conn: &Connection, cutoff: &str) -> Result<usize> {
        conn.execute(
            "DELETE FROM heartbeat_fires WHERE fired_at < ?1",
            params![cutoff],
        )
    }
}

// ── Scheduler meta (daemon-wide KV) ────────────────────────────────────

/// 0062 — tiny daemon-owned key/value store for scheduler bookkeeping
/// that is per-daemon, not per-heartbeat. First key: `last_tick_at`,
/// stamped on every `/cli/scheduler-tick`, which makes tick-transport
/// gaps (sleep, daemon downtime, a silently-unloaded launchd agent)
/// measurable — the single biggest observability hole the misfire
/// study found.
pub struct SchedulerMeta;

impl SchedulerMeta {
    /// The RFC3339 timestamp of the most recent scheduler tick.
    pub const LAST_TICK_AT: &'static str = "last_tick_at";

    pub fn get(conn: &Connection, key: &str) -> Option<String> {
        conn.query_row(
            "SELECT value FROM scheduler_meta WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .ok()
    }

    pub fn set(conn: &Connection, key: &str, value: &str) -> Result<usize> {
        conn.execute(
            "INSERT INTO scheduler_meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
    }
}

// ── Subdomain → workspace attribution (0074) ───────────────────────────

/// 0074 — which WORKSPACE a nested K2 Connect subdomain label belongs
/// to. Daemon-local overlay on the control-plane-owned routing map
/// (`tunnel::subdomains`): `label` (lowercase nested label, e.g.
/// `staging`) → `projects.id`. Written by the `k2 publish subdomain
/// create/point/claim` seams, removed by `rm`/`unclaim`. Labels are
/// normalized lowercase to match `SubdomainMap::from_rows`; no FK on
/// purpose — a label may predate/outlive the workspace registry, and
/// readers treat a dangling project_id as unattributed.
pub struct SubdomainWorkspace;

impl SubdomainWorkspace {
    /// The full `label → project_id` attribution map.
    pub fn map(conn: &Connection) -> Result<std::collections::HashMap<String, String>> {
        let mut stmt = conn.prepare("SELECT label, project_id FROM subdomain_workspaces")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect()
    }

    /// Attribute `label` to `project_id` (upsert — re-claiming an
    /// already-attributed label repoints it, the PK guarantees one
    /// owner per label). Errors on a blank label/project_id: an empty
    /// attribution row is always a caller bug, never valid state.
    pub fn claim(conn: &Connection, label: &str, project_id: &str) -> Result<()> {
        let label = label.trim().to_ascii_lowercase();
        let project_id = project_id.trim();
        if label.is_empty() || project_id.is_empty() {
            return Err(rusqlite::Error::InvalidParameterName(
                "subdomain claim needs a non-empty label and project_id".to_string(),
            ));
        }
        conn.execute(
            "INSERT OR REPLACE INTO subdomain_workspaces (label, project_id) VALUES (?1, ?2)",
            params![label, project_id],
        )?;
        Ok(())
    }

    /// Remove `label`'s attribution. Returns whether a row was
    /// actually deleted (false = the label wasn't attributed — callers
    /// surface that honestly instead of pretending a delete happened).
    pub fn unclaim(conn: &Connection, label: &str) -> Result<bool> {
        let label = label.trim().to_ascii_lowercase();
        let n = conn.execute(
            "DELETE FROM subdomain_workspaces WHERE label = ?1",
            params![label],
        )?;
        Ok(n > 0)
    }
// ── K2 Mail (0072, prd-email-server-v1 §12) ────────────────────────────
//
// Row structs for the mail tables. Serialize camelCase — the wire
// shape the `/cli/mail/*` routes return (same convention as the
// feedback structs in `crate::feedback`). These are the K2-side
// records ONLY: agent ownership, approvals, caps, doctor history.
// Stalwart's own state (accounts, messages, DKIM keys) stays in
// Stalwart, reached exclusively over its JMAP management API.

/// The `mail_server` SINGLETON row (id = 1). "not-installed" is the
/// ABSENCE of the row — `status` only covers installed lifecycles
/// (`installing|running|degraded|stopped|disabled|error`). The
/// `*_secret_ref` fields reference the daemon's secret storage, never
/// secrets themselves.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailServer {
    pub id: i64,
    pub status: String,
    pub pinned_version: String,
    pub installed_version: Option<String>,
    pub hostname: Option<String>,
    /// `tls-alpn` | `dns-01` | `http-01` (PRD §5.3 detect-and-adapt).
    pub port_plan: Option<String>,
    pub api_url: Option<String>,
    pub admin_secret_ref: Option<String>,
    pub api_key_ref: Option<String>,
    pub installed_at: Option<i64>,
    pub updated_at: i64,
    /// S1 (0073): per-step completion state of the resumable enable
    /// machine (`{"steps":{"download":{…}},…}`), polled by
    /// GET /cli/mail/status; re-enable resumes from it.
    pub enable_progress_json: Option<String>,
    /// S1 (0073): most recent supervisor error (enable-step failure /
    /// health degradation detail), surfaced verbatim in status.
    pub last_error: Option<String>,
}

/// One `mail_domains` row. `domain` is ALWAYS the normalized form
/// (lowercase punycode A-label, no trailing dot —
/// [`crate::mail_domain::normalize_mail_domain`]); display-decode at
/// the edge. `dns_status_json` = per-record Valid/Missing/Wrong state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailDomain {
    pub id: String,
    pub domain: String,
    pub stalwart_domain_id: Option<String>,
    /// `direct` | `relay` | `receive-only` (PRD §8.3 / D1).
    pub send_mode: String,
    pub relay_config_id: Option<String>,
    /// `pending` | `verified` | `error`.
    pub status: String,
    pub dns_status_json: Option<String>,
    pub verified_at: Option<i64>,
    pub last_checked_at: Option<i64>,
    pub created_at: i64,
}

/// One `mail_relay_configs` row (smart-host outbound, PRD §8.3).
/// `kind` is `smtp` in V1; the provider kinds exist in the schema so
/// nothing may assume `kind == smtp`. `secret_ref` references the
/// daemon's secret storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailRelayConfig {
    pub id: String,
    pub kind: String,
    pub host: Option<String>,
    pub port: Option<i64>,
    pub username: Option<String>,
    pub secret_ref: Option<String>,
    /// `implicit` | `starttls`.
    pub tls_kind: Option<String>,
    pub spf_include: Option<String>,
    pub config_json: Option<String>,
    pub created_at: i64,
}

/// One `mail_addresses` row — the binding of a Stalwart account to its
/// OWNING workspace (PRD §7.1). `owner_project_id` = `projects.id`,
/// resolved server-side from the calling token, never from a body.
/// `client_id` powers idempotent minting (`k2 mail create --id`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailAddress {
    pub id: String,
    pub address: String,
    pub domain_id: String,
    pub stalwart_account_id: Option<String>,
    pub owner_project_id: String,
    pub client_id: Option<String>,
    /// `active` | `retired`.
    pub status: String,
    pub created_at: i64,
    pub retired_at: Option<i64>,
}

/// One `mail_outbound` row — the approval queue AND the send audit log
/// (PRD §8.4). A row is written BEFORE any hand-off to Stalwart in
/// every gating mode (pre-mortem #11: no row, no send). `body_ref` /
/// `attachments_ref` point at daemon-managed storage — message bodies
/// are never inlined here and never logged.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailOutbound {
    pub id: String,
    pub owner_project_id: String,
    pub agent_name: String,
    pub from_address: String,
    /// JSON array of recipient addresses.
    pub to_json: String,
    pub cc_json: Option<String>,
    pub subject: String,
    pub body_ref: Option<String>,
    pub attachments_ref: Option<String>,
    /// `pending` | `approved` | `denied` | `sent` | `failed`.
    pub status: String,
    pub decided_by: Option<String>,
    pub note: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub decided_at: Option<i64>,
    pub sent_at: Option<i64>,
}

/// One `mail_doctor_runs` row (PRD §9). `domain_id` is `None` for
/// server-level runs (network/PTR/blocklist checks have no domain).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailDoctorRun {
    pub id: String,
    pub domain_id: Option<String>,
    pub results_json: String,
    /// `pass` | `warn` | `fail` — the direct-send readiness grade.
    pub grade: String,
    pub ran_at: i64,
}

/// One `mail_external_inboxes` row (0074, PRD §17.5) — the user's OWN
/// external email account (Gmail app-password, Fastmail, company
/// IMAP), bound to exactly ONE workspace at add time. Agents in that
/// workspace read the inbox and save reply DRAFTS into the account's
/// real Drafts folder; there is NO send path from an external account
/// in V1. The password lives in the daemon vault under the
/// deterministic key `ext-inbox-<id>` — never a field here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailExternalInbox {
    pub id: String,
    /// The bound workspace (`projects.id`) — the ONLY workspace whose
    /// agents can see this inbox (masked `not_found` everywhere else).
    pub owner_project_id: String,
    /// Normalized (lowercase local part + punycode A-label domain),
    /// UNIQUE — the §17.5 `backend_for_address` seam key.
    pub email_address: String,
    pub display_name: Option<String>,
    /// `imap` in V1; `jmap` / `gmail-api` (OAuth2) anticipated in the
    /// CHECK — nothing may assume `kind == 'imap'`.
    pub kind: String,
    pub host: String,
    pub port: i64,
    /// `implicit-tls` | `starttls` — TLS is never optional.
    pub tls: String,
    pub username: String,
    /// `None` = autodetect (LIST SPECIAL-USE `\Drafts`, then common
    /// names); a value overrides detection.
    pub drafts_folder: Option<String>,
    /// `connected` | `error` (last add/draft outcome).
    pub status: String,
    pub last_checked_at: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
}

#[cfg(test)]
mod unit_tests {
    //! Per-struct CRUD + invariant coverage for schema.rs. Each test
    //! uses `crate::db::isolated_test_connection()` to get a fresh
    //! in-memory SQLite with the full migration + seed sequence
    //! applied — so tests can assert on specific row counts or state
    //! transitions without worrying about pollution from sibling
    //! tests.
    //!
    //! Coverage target: every public method on every schema struct
    //! has at least a round-trip test (write → read → assert). Edge
    //! cases (unique constraint violations, name validation, enabled
    //! filter semantics) have dedicated tests.
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> Connection {
        crate::db::isolated_test_connection()
    }

    /// 0072 (K2 Mail): the migration applies — every mail table exists
    /// and round-trips a row; the per-workspace override columns landed
    /// on `projects`.
    #[test]
    fn mail_migration_applies_and_roundtrips() {
        let conn = fresh();
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, updated_at) \
             VALUES (1, 'installing', '0.16.0', 100)",
            [],
        )
        .expect("mail_server insert");
        conn.execute(
            "INSERT INTO mail_domains (id, domain, created_at) VALUES ('d1', 'acme.dev', 100)",
            [],
        )
        .expect("mail_domains insert");
        conn.execute(
            "INSERT INTO mail_relay_configs (id, created_at) VALUES ('r1', 100)",
            [],
        )
        .expect("mail_relay_configs insert");
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, owner_project_id, created_at) \
             VALUES ('a1', 'scout@acme.dev', 'd1', 'p1', 100)",
            [],
        )
        .expect("mail_addresses insert");
        conn.execute(
            "INSERT INTO mail_outbound (id, owner_project_id, agent_name, from_address, \
             to_json, subject, created_at, updated_at) \
             VALUES ('o1', 'p1', 'scout', 'scout@acme.dev', '[\"x@example.com\"]', 's', 100, 100)",
            [],
        )
        .expect("mail_outbound insert");
        conn.execute(
            "INSERT INTO mail_doctor_runs (id, results_json, grade, ran_at) \
             VALUES ('dr1', '{}', 'warn', 100)",
            [],
        )
        .expect("mail_doctor_runs insert");

        // Defaults land per §12: receive-only, pending, smtp, active, pending.
        let (send_mode, status): (String, String) = conn
            .query_row(
                "SELECT send_mode, status FROM mail_domains WHERE id = 'd1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("domain read");
        assert_eq!(send_mode, "receive-only");
        assert_eq!(status, "pending");
        let kind: String = conn
            .query_row("SELECT kind FROM mail_relay_configs WHERE id = 'r1'", [], |r| r.get(0))
            .expect("relay read");
        assert_eq!(kind, "smtp");
        let addr_status: String = conn
            .query_row("SELECT status FROM mail_addresses WHERE id = 'a1'", [], |r| r.get(0))
            .expect("address read");
        assert_eq!(addr_status, "active");
        let out_status: String = conn
            .query_row("SELECT status FROM mail_outbound WHERE id = 'o1'", [], |r| r.get(0))
            .expect("outbound read");
        assert_eq!(out_status, "pending");

        // The per-workspace override columns exist on projects and
        // backfill to NULL (inherit-global).
        let pid = make_project_row(&conn, "/tmp/mail-mig-proj");
        let (send, cap): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT mail_agent_send, mail_address_cap FROM projects WHERE id = ?1",
                params![pid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("projects override read");
        assert!(send.is_none() && cap.is_none(), "overrides backfill to NULL");
    }

    /// 0072 (K2 Mail): the CHECK-constrained enums reject invalid
    /// values loudly, and the singleton/uniqueness constraints hold.
    #[test]
    fn mail_migration_check_constraints_enforced() {
        let conn = fresh();
        // mail_server is a singleton: id must be 1.
        assert!(
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, updated_at) \
                 VALUES (2, 'running', '0.16.0', 1)",
                [],
            )
            .is_err(),
            "mail_server id != 1 must be rejected"
        );
        // Bad enum values are rejected on every CHECKed column.
        for (label, sql) in [
            (
                "mail_server.status",
                "INSERT INTO mail_server (id, status, pinned_version, updated_at) \
                 VALUES (1, 'exploded', '0.16.0', 1)",
            ),
            (
                "mail_domains.send_mode",
                "INSERT INTO mail_domains (id, domain, send_mode, created_at) \
                 VALUES ('dx', 'x.dev', 'carrier-pigeon', 1)",
            ),
            (
                "mail_domains.status",
                "INSERT INTO mail_domains (id, domain, status, created_at) \
                 VALUES ('dy', 'y.dev', 'maybe', 1)",
            ),
            (
                "mail_relay_configs.kind",
                "INSERT INTO mail_relay_configs (id, kind, created_at) \
                 VALUES ('rx', 'sendmail', 1)",
            ),
            (
                "mail_relay_configs.tls_kind",
                "INSERT INTO mail_relay_configs (id, tls_kind, created_at) \
                 VALUES ('ry', 'plaintext', 1)",
            ),
            (
                "mail_addresses.status",
                "INSERT INTO mail_addresses (id, address, domain_id, owner_project_id, status, created_at) \
                 VALUES ('ax', 'x@x.dev', 'd', 'p', 'zombie', 1)",
            ),
            (
                "mail_outbound.status",
                "INSERT INTO mail_outbound (id, owner_project_id, agent_name, from_address, to_json, subject, status, created_at, updated_at) \
                 VALUES ('ox', 'p', 'a', 'f@x.dev', '[]', 's', 'maybe', 1, 1)",
            ),
            (
                "mail_doctor_runs.grade",
                "INSERT INTO mail_doctor_runs (id, results_json, grade, ran_at) \
                 VALUES ('dx', '{}', 'A+', 1)",
            ),
        ] {
            assert!(conn.execute(sql, []).is_err(), "{label}: bad enum must be rejected");
        }
        // Duplicate domain / address / (owner, client_id) are rejected.
        conn.execute(
            "INSERT INTO mail_domains (id, domain, created_at) VALUES ('d1', 'acme.dev', 1)",
            [],
        )
        .expect("first domain");
        assert!(
            conn.execute(
                "INSERT INTO mail_domains (id, domain, created_at) VALUES ('d2', 'acme.dev', 1)",
                [],
            )
            .is_err(),
            "duplicate domain must be rejected"
        );
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, owner_project_id, client_id, created_at) \
             VALUES ('a1', 'bot@acme.dev', 'd1', 'p1', 'signup-1', 1)",
            [],
        )
        .expect("first address");
        assert!(
            conn.execute(
                "INSERT INTO mail_addresses (id, address, domain_id, owner_project_id, client_id, created_at) \
                 VALUES ('a2', 'bot2@acme.dev', 'd1', 'p1', 'signup-1', 1)",
                [],
            )
            .is_err(),
            "duplicate (owner, client_id) must be rejected (idempotent minting)"
        );
        // ...but the same client_id under ANOTHER workspace is fine.
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, owner_project_id, client_id, created_at) \
             VALUES ('a3', 'bot3@acme.dev', 'd1', 'p2', 'signup-1', 1)",
            [],
        )
        .expect("same client_id, different owner");
    }

    /// 0074 (K2 Mail S9): `mail_external_inboxes` applies — defaults
    /// land (`imap` / `implicit-tls` / `connected`), the address is
    /// UNIQUE, the CHECKed enums reject invalid values (there is no
    /// plaintext TLS kind, by construction), and NO credential column
    /// exists on the table.
    #[test]
    fn mail_external_inboxes_migration_defaults_and_constraints() {
        let conn = fresh();
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, \
             host, port, username, created_at) \
             VALUES ('x1', 'p1', 'rosson@example.com', 'imap.example.com', 993, \
             'rosson@example.com', 100)",
            [],
        )
        .expect("external inbox insert");
        let (kind, tls, status): (String, String, String) = conn
            .query_row(
                "SELECT kind, tls, status FROM mail_external_inboxes WHERE id = 'x1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("external inbox read");
        assert_eq!((kind.as_str(), tls.as_str(), status.as_str()), ("imap", "implicit-tls", "connected"));

        // One row per account — even bound to a DIFFERENT workspace.
        assert!(
            conn.execute(
                "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, \
                 host, port, username, created_at) \
                 VALUES ('x2', 'p2', 'rosson@example.com', 'imap.example.com', 993, 'u', 1)",
                [],
            )
            .is_err(),
            "duplicate email_address must be rejected"
        );
        for (label, sql) in [
            (
                "mail_external_inboxes.kind",
                "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, kind, \
                 host, port, username, created_at) \
                 VALUES ('xk', 'p', 'k@example.com', 'pop3', 'h', 993, 'u', 1)",
            ),
            (
                "mail_external_inboxes.tls",
                "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, tls, \
                 host, port, username, created_at) \
                 VALUES ('xt', 'p', 't@example.com', 'plaintext', 'h', 143, 'u', 1)",
            ),
            (
                "mail_external_inboxes.status",
                "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, status, \
                 host, port, username, created_at) \
                 VALUES ('xs', 'p', 's@example.com', 'maybe', 'h', 993, 'u', 1)",
            ),
        ] {
            assert!(conn.execute(sql, []).is_err(), "{label}: bad enum must be rejected");
        }
        // The vault, not the table, holds the secret: no password-ish
        // column may ever exist here.
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('mail_external_inboxes')")
            .expect("table info");
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .expect("cols")
            .filter_map(Result::ok)
            .collect();
        for banned in ["password", "secret", "secret_ref", "pass"] {
            assert!(
                !cols.iter().any(|c| c.contains(banned)),
                "credential-shaped column '{banned}' must not exist: {cols:?}"
            );
        }
    }

    fn make_project_row(conn: &Connection, path: &str) -> String {
        // Every test that touches session/heartbeat/fire tables needs
        // a projects row because of the FK. This matches make_project
        // in concurrency_tests but is duplicated here to keep the two
        // test modules independent.
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            params![id, "test", path],
        )
        .expect("insert project");
        id
    }

    // ── FocusGroup ────────────────────────────────────────────────
    #[test]
    fn focus_group_create_list_get_update_delete() {
        let conn = fresh();
        FocusGroup::create(&conn, "fg1", "Work", Some("#ff0000"), 0).unwrap();
        FocusGroup::create(&conn, "fg2", "Personal", None, 1).unwrap();

        let list = FocusGroup::list(&conn).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Work");
        assert_eq!(list[1].name, "Personal");

        let fg = FocusGroup::get(&conn, "fg1").unwrap();
        assert_eq!(fg.color.as_deref(), Some("#ff0000"));

        FocusGroup::update(&conn, "fg1", Some("Work Rebranded"), None, None).unwrap();
        let fg = FocusGroup::get(&conn, "fg1").unwrap();
        assert_eq!(fg.name, "Work Rebranded");

        FocusGroup::delete(&conn, "fg2").unwrap();
        assert_eq!(FocusGroup::list(&conn).unwrap().len(), 1);
    }

    // ── Project ───────────────────────────────────────────────────
    #[test]
    fn project_create_list_get_delete_roundtrip() {
        let conn = fresh();
        // Baseline accounts for the `_orphan` + `_broadcast`
        // sentinel rows seeded by `db::seed_audit_sentinels` — they
        // exist so egress audit never fails FK when a signal's
        // workspace id doesn't match a real project.
        let baseline = Project::list(&conn).unwrap().len();

        let id = make_project_row(&conn, "/tmp/proj-cr");
        let all = Project::list(&conn).unwrap();
        assert_eq!(all.len(), baseline + 1);
        assert!(
            all.iter().any(|p| p.path == "/tmp/proj-cr"),
            "inserted project should appear in list"
        );

        let p = Project::get(&conn, &id).unwrap();
        assert_eq!(p.id, id);
        assert_eq!(p.name, "test");

        Project::delete(&conn, &id).unwrap();
        assert_eq!(Project::list(&conn).unwrap().len(), baseline);
    }

    #[test]
    fn project_heartbeat_enabled_is_live_aggregate_not_legacy_mode() {
        // Regression: `projects.heartbeat_enabled` used to be derived from the
        // legacy `heartbeat_mode` string and drifted out of sync with the
        // per-heartbeat `enabled` flags (the "Sarah" bug: mode='scheduled' but
        // every heartbeat disabled → flag wrongly reported 1, which lit the
        // autonomous badge and kept the session warm forever). `Project::list`
        // / `get` now compute it live as "any enabled, non-archived heartbeat".
        let conn = fresh();
        let id = make_project_row(&conn, "/tmp/proj-hb-agg");

        // Force the legacy drift directly: mode='scheduled' with the stale
        // stored column set to 1 (exactly what Project::update used to write).
        conn.execute(
            "UPDATE projects SET heartbeat_mode = 'scheduled', heartbeat_enabled = 1 WHERE id = ?1",
            params![id],
        )
        .unwrap();
        let stored: i64 = conn
            .query_row("SELECT heartbeat_enabled FROM projects WHERE id = ?1", params![id], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, 1, "legacy stored column should be 1 (the drift we're defending against)");

        // No heartbeats yet → live aggregate must be 0 despite mode='scheduled'.
        assert_eq!(Project::get(&conn, &id).unwrap().heartbeat_enabled, 0);

        // Two heartbeats, both disabled → still 0 (the exact Sarah state).
        AgentHeartbeat::insert(&conn, "hb-default", &id, "default", "daily", "{}", "wakeup.md", false).unwrap();
        AgentHeartbeat::insert(&conn, "hb-triage", &id, "triage", "hourly", "{}", "wakeup.md", false).unwrap();
        assert_eq!(
            Project::get(&conn, &id).unwrap().heartbeat_enabled,
            0,
            "all heartbeats disabled → aggregate 0"
        );
        // And via list() (the renderer's actual source).
        let from_list = Project::list(&conn).unwrap().into_iter().find(|p| p.id == id).unwrap();
        assert_eq!(from_list.heartbeat_enabled, 0);

        // Enable one → aggregate flips to 1.
        AgentHeartbeat::set_enabled(&conn, &id, "triage", true).unwrap();
        assert_eq!(Project::get(&conn, &id).unwrap().heartbeat_enabled, 1);

        // Archive the only enabled one → back to 0 (archived ≠ live, matches scheduler).
        AgentHeartbeat::archive(&conn, &id, "triage").unwrap();
        assert_eq!(Project::get(&conn, &id).unwrap().heartbeat_enabled, 0, "archived heartbeat must not count");
    }

    #[test]
    fn project_default_agent_is_null_after_migration_and_bare_insert() {
        // 0063 semantics: the column is nullable with NO default, so a row
        // created without an explicit value (and, by the same ALTER TABLE
        // backfill rule, every pre-migration row) reads None = "inherit the
        // global default at resolve time". Non-retroactivity hangs off this.
        let conn = fresh();
        let id = make_project_row(&conn, "/tmp/proj-da-null");
        let p = Project::get(&conn, &id).unwrap();
        assert_eq!(
            p.default_agent, None,
            "bare insert must leave default_agent NULL (inherit global)"
        );
        let from_list = Project::list(&conn)
            .unwrap()
            .into_iter()
            .find(|p| p.id == id)
            .expect("row must appear in list");
        assert_eq!(from_list.default_agent, None, "list() must read the same NULL");
    }

    #[test]
    fn project_default_agent_update_roundtrip_set_and_clear() {
        let conn = fresh();
        let id = make_project_row(&conn, "/tmp/proj-da-rt");

        // Set to a preset id (the canonical value shape).
        let preset_id = "0f9a1c2e-1111-4222-8333-444455556666";
        Project::update(
            &conn,
            &id,
            None, None, None, None, None, None, None, None, None, None, None, None, None, None,
            None,
            Some(Some(preset_id)),
        )
        .unwrap();
        assert_eq!(
            Project::get(&conn, &id).unwrap().default_agent.as_deref(),
            Some(preset_id),
            "preset-id value must round-trip"
        );

        // Legacy command token must be stored verbatim — shape is NOT
        // validated on write (Slice 0 defines tolerant matching read-side).
        Project::update(
            &conn,
            &id,
            None, None, None, None, None, None, None, None, None, None, None, None, None, None,
            None,
            Some(Some("claude")),
        )
        .unwrap();
        assert_eq!(
            Project::get(&conn, &id).unwrap().default_agent.as_deref(),
            Some("claude"),
            "legacy command token must be stored as given"
        );

        // Some(None) clears back to NULL = inherit global.
        Project::update(
            &conn,
            &id,
            None, None, None, None, None, None, None, None, None, None, None, None, None, None,
            None,
            Some(None),
        )
        .unwrap();
        assert_eq!(
            Project::get(&conn, &id).unwrap().default_agent,
            None,
            "Some(None) must clear the override back to NULL"
        );

        // None leaves the value untouched.
        Project::update(
            &conn,
            &id,
            None, None, None, None, None, None, None, None, None, None, None, None, None, None,
            None,
            Some(Some(preset_id)),
        )
        .unwrap();
        Project::update(
            &conn,
            &id,
            Some("renamed"),
            None, None, None, None, None, None, None, None, None, None, None, None, None, None,
            None,
        )
        .unwrap();
        let p = Project::get(&conn, &id).unwrap();
        assert_eq!(p.name, "renamed");
        assert_eq!(
            p.default_agent.as_deref(),
            Some(preset_id),
            "an unrelated update must not touch default_agent"
        );
    }

    #[test]
    fn project_path_unique_constraint_rejects_duplicate() {
        let conn = fresh();
        let path = "/tmp/proj-dup";
        make_project_row(&conn, path);
        // Second insert with same path must fail.
        let id2 = uuid::Uuid::new_v4().to_string();
        let err = conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            params![id2, "other", path],
        );
        assert!(err.is_err(), "duplicate path should violate unique index");
    }

    #[test]
    fn project_touch_and_clear_interaction() {
        let conn = fresh();
        let id = make_project_row(&conn, "/tmp/proj-touch");
        Project::touch_interaction(&conn, &id).unwrap();
        let p = Project::get(&conn, &id).unwrap();
        assert!(p.last_interaction_at.is_some(), "touch should set timestamp");

        Project::clear_interaction(&conn, &id).unwrap();
        let p = Project::get(&conn, &id).unwrap();
        assert!(p.last_interaction_at.is_none(), "clear should null timestamp");
    }

    #[test]
    fn project_update_last_opened_sets_timestamp() {
        let conn = fresh();
        let id = make_project_row(&conn, "/tmp/proj-opened");
        Project::update_last_opened(&conn, &id).unwrap();
        let p = Project::get(&conn, &id).unwrap();
        assert!(p.last_opened_at.is_some());
    }

    // ── WorkspaceSession ─────────────────────────────────────────
    //
    // Post-0.37.0: one row per project_id, enforced by schema-level
    // UNIQUE(project_id) (migration 0039). Tests assert the post-state
    // invariant directly — no `unwrap_or` fallbacks that would mask a
    // regression to the old multi-row shape.
    #[test]
    fn workspace_session_upsert_then_get() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as1");
        WorkspaceSession::upsert(
            &conn, "sess-1", &pid, Some("term-7"), None, "claude", "manager", "sleeping",
        )
        .unwrap();

        let s = WorkspaceSession::get(&conn, &pid)
            .unwrap()
            .expect("session exists");
        assert_eq!(s.id, "sess-1");
        assert_eq!(s.terminal_id.as_deref(), Some("term-7"));
        assert_eq!(s.status, "sleeping");
    }

    #[test]
    fn workspace_session_upsert_updates_existing_row() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as2");
        WorkspaceSession::upsert(
            &conn, "s1", &pid, Some("t1"), None, "claude", "manager", "sleeping",
        )
        .unwrap();
        WorkspaceSession::upsert(
            &conn, "s2", &pid, Some("t2"), Some("scid"), "codex", "user", "running",
        )
        .unwrap();

        // Same project_id — second upsert replaces the row's payload.
        let s = WorkspaceSession::get(&conn, &pid).unwrap().unwrap();
        assert_eq!(s.terminal_id.as_deref(), Some("t2"));
        assert_eq!(s.harness, "codex");
        assert_eq!(s.status, "running");
        assert_eq!(s.session_id.as_deref(), Some("scid"));
    }

    #[test]
    fn workspace_session_unique_constraint_on_project_id() {
        // The product invariant ("a workspace IS its agent") is the
        // schema constraint. Two raw inserts with the same project_id
        // must be rejected by SQLite — no application code can violate
        // the one-row-per-workspace rule.
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as3");
        WorkspaceSession::upsert(
            &conn, "s1", &pid, None, None, "claude", "manager", "sleeping",
        )
        .unwrap();
        let err = conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, harness, owner, status) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["sX", &pid, "claude", "manager", "sleeping"],
        );
        assert!(err.is_err(), "UNIQUE(project_id) must reject");
    }

    #[test]
    fn workspace_session_get_by_terminal_id() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as-term");
        WorkspaceSession::upsert(
            &conn, "sa", &pid, Some("terminal-99"), None, "claude", "manager", "running",
        )
        .unwrap();
        let s = WorkspaceSession::get_by_terminal_id(&conn, "terminal-99")
            .unwrap()
            .unwrap();
        assert_eq!(s.project_id, pid);
        let none = WorkspaceSession::get_by_terminal_id(&conn, "no-such").unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn workspace_session_update_status_and_message() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as-sm");
        WorkspaceSession::upsert(
            &conn, "s", &pid, None, None, "claude", "manager", "sleeping",
        )
        .unwrap();

        let n = WorkspaceSession::update_status(&conn, &pid, "running").unwrap();
        assert_eq!(n, 1);
        WorkspaceSession::update_status_message(&conn, &pid, "spawning PTY").unwrap();
        let s = WorkspaceSession::get(&conn, &pid).unwrap().unwrap();
        assert_eq!(s.status, "running");
        assert_eq!(s.status_message.as_deref(), Some("spawning PTY"));
    }

    #[test]
    fn workspace_session_session_id_set_and_clear() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as-sid");
        WorkspaceSession::upsert(
            &conn, "s", &pid, None, None, "claude", "manager", "sleeping",
        )
        .unwrap();
        WorkspaceSession::update_session_id(&conn, &pid, "claude-abcd").unwrap();
        assert_eq!(
            WorkspaceSession::get(&conn, &pid).unwrap().unwrap().session_id.as_deref(),
            Some("claude-abcd")
        );
        WorkspaceSession::clear_session_id(&conn, &pid).unwrap();
        assert!(WorkspaceSession::get(&conn, &pid).unwrap().unwrap().session_id.is_none());
    }

    #[test]
    fn workspace_session_wake_counter_increments_and_resets() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as-wc");
        WorkspaceSession::upsert(
            &conn, "s", &pid, None, None, "claude", "manager", "sleeping",
        )
        .unwrap();
        assert_eq!(WorkspaceSession::bump_wake_counter(&conn, &pid).unwrap(), 1);
        assert_eq!(WorkspaceSession::bump_wake_counter(&conn, &pid).unwrap(), 2);
        assert_eq!(WorkspaceSession::bump_wake_counter(&conn, &pid).unwrap(), 3);
        WorkspaceSession::reset_wake_counter(&conn, &pid).unwrap();
        assert_eq!(WorkspaceSession::bump_wake_counter(&conn, &pid).unwrap(), 1);
    }

    #[test]
    fn workspace_session_delete_removes_row() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as-del");
        WorkspaceSession::upsert(
            &conn, "s", &pid, None, None, "claude", "manager", "sleeping",
        )
        .unwrap();
        assert!(WorkspaceSession::get(&conn, &pid).unwrap().is_some());
        let n = WorkspaceSession::delete(&conn, &pid).unwrap();
        assert_eq!(n, 1);
        assert!(WorkspaceSession::get(&conn, &pid).unwrap().is_none());
    }

    // ── AgentHeartbeat ────────────────────────────────────────────
    #[test]
    fn heartbeat_validate_name_rejects_empty() {
        assert!(AgentHeartbeat::validate_name("").is_err());
    }

    #[test]
    fn heartbeat_validate_name_rejects_reserved() {
        assert!(AgentHeartbeat::validate_name("default").is_err());
        assert!(AgentHeartbeat::validate_name("legacy").is_err());
    }

    #[test]
    fn heartbeat_validate_name_rejects_uppercase() {
        assert!(AgentHeartbeat::validate_name("MyHeartbeat").is_err());
    }

    #[test]
    fn heartbeat_validate_name_rejects_leading_trailing_hyphen() {
        assert!(AgentHeartbeat::validate_name("-foo").is_err());
        assert!(AgentHeartbeat::validate_name("foo-").is_err());
    }

    #[test]
    fn heartbeat_validate_name_accepts_valid() {
        assert!(AgentHeartbeat::validate_name("nightly").is_ok());
        assert!(AgentHeartbeat::validate_name("morning-1").is_ok());
        assert!(AgentHeartbeat::validate_name("h1").is_ok());
    }

    #[test]
    fn heartbeat_insert_list_get_delete() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-c");
        AgentHeartbeat::insert(
            &conn, "hb1", &pid, "nightly", "60m", "{}", "agents/foo/heartbeats/nightly/WAKEUP.md", true,
        )
        .unwrap();
        AgentHeartbeat::insert(
            &conn, "hb2", &pid, "morning", "30m", "{}", "agents/foo/heartbeats/morning/WAKEUP.md", false,
        )
        .unwrap();

        let list = AgentHeartbeat::list_by_project(&conn, &pid).unwrap();
        assert_eq!(list.len(), 2);
        // list is ORDER BY name — morning < nightly
        assert_eq!(list[0].name, "morning");
        assert_eq!(list[1].name, "nightly");

        let enabled_only = AgentHeartbeat::list_enabled(&conn, &pid).unwrap();
        assert_eq!(enabled_only.len(), 1);
        assert_eq!(enabled_only[0].name, "nightly");

        let h = AgentHeartbeat::get_by_name(&conn, &pid, "nightly").unwrap().unwrap();
        assert_eq!(h.frequency, "60m");

        let n = AgentHeartbeat::delete(&conn, &pid, "morning").unwrap();
        assert_eq!(n, 1);
        assert_eq!(AgentHeartbeat::list_by_project(&conn, &pid).unwrap().len(), 1);
    }

    #[test]
    fn heartbeat_set_enabled_toggles() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-en");
        AgentHeartbeat::insert(
            &conn, "hb1", &pid, "weekly", "7d", "{}", "agents/foo/heartbeats/weekly/WAKEUP.md", false,
        )
        .unwrap();
        AgentHeartbeat::set_enabled(&conn, &pid, "weekly", true).unwrap();
        let h = AgentHeartbeat::get_by_name(&conn, &pid, "weekly").unwrap().unwrap();
        assert!(h.enabled);
        AgentHeartbeat::set_enabled(&conn, &pid, "weekly", false).unwrap();
        assert!(!AgentHeartbeat::get_by_name(&conn, &pid, "weekly").unwrap().unwrap().enabled);
    }

    #[test]
    fn heartbeat_update_schedule_and_wakeup_path() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-upd");
        AgentHeartbeat::insert(
            &conn, "hb1", &pid, "pulse", "60m", "{\"x\":1}", "path1", true,
        )
        .unwrap();
        AgentHeartbeat::update_schedule(&conn, &pid, "pulse", "30m", "{\"x\":2}").unwrap();
        let h = AgentHeartbeat::get_by_name(&conn, &pid, "pulse").unwrap().unwrap();
        assert_eq!(h.frequency, "30m");
        assert_eq!(h.spec_json, "{\"x\":2}");

        AgentHeartbeat::update_wakeup_path(&conn, &pid, "pulse", "new/path").unwrap();
        let h = AgentHeartbeat::get_by_name(&conn, &pid, "pulse").unwrap().unwrap();
        assert_eq!(h.wakeup_path, "new/path");
    }

    #[test]
    fn heartbeat_stamp_last_fired_sets_rfc3339() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-fire");
        AgentHeartbeat::insert(
            &conn, "hb1", &pid, "hb", "60m", "{}", "p", true,
        )
        .unwrap();
        AgentHeartbeat::stamp_last_fired(&conn, &pid, "hb").unwrap();
        let h = AgentHeartbeat::get_by_name(&conn, &pid, "hb").unwrap().unwrap();
        let ts = h.last_fired.expect("last_fired set");
        // RFC3339 sanity — "YYYY-MM-DDTHH:MM:SS..."
        assert!(ts.contains('T'), "expected RFC3339 timestamp, got: {}", ts);
        assert!(chrono::DateTime::parse_from_rfc3339(&ts).is_ok(), "parseable RFC3339: {}", ts);
    }

    // ── 0.36.0 fields: last_session_id + archived_at ──────────────

    #[test]
    fn heartbeat_save_session_id_writes_value() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-sid");
        AgentHeartbeat::insert(
            &conn, "hb1", &pid, "triage", "60m", "{}", "p", true,
        )
        .unwrap();
        let pre = AgentHeartbeat::get_by_name(&conn, &pid, "triage").unwrap().unwrap();
        assert!(pre.last_session_id.is_none(), "fresh row has no session id");

        let n = AgentHeartbeat::save_session_id(&conn, &pid, "triage", "claude-xyz").unwrap();
        assert_eq!(n, 1);

        let h = AgentHeartbeat::get_by_name(&conn, &pid, "triage").unwrap().unwrap();
        assert_eq!(h.last_session_id.as_deref(), Some("claude-xyz"));
    }

    // ── 0036: active_terminal_id + surfaced flag (heartbeat-active-session PRD) ──

    #[test]
    fn heartbeat_active_terminal_round_trip() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-active");
        AgentHeartbeat::insert(&conn, "hb1", &pid, "daily", "60m", "{}", "p", true).unwrap();

        // Fresh row has no live PTY.
        let pre = AgentHeartbeat::get_by_name(&conn, &pid, "daily").unwrap().unwrap();
        assert!(pre.active_terminal_id.is_none());

        // Stamp a terminal id.
        let n = AgentHeartbeat::save_active_terminal_id(&conn, &pid, "daily", "wake-cortana-abc").unwrap();
        assert_eq!(n, 1);
        let mid = AgentHeartbeat::get_by_name(&conn, &pid, "daily").unwrap().unwrap();
        assert_eq!(mid.active_terminal_id.as_deref(), Some("wake-cortana-abc"));

        // Clear it (e.g., child-exit observer).
        let n = AgentHeartbeat::clear_active_terminal_id(&conn, &pid, "daily").unwrap();
        assert_eq!(n, 1);
        let cleared = AgentHeartbeat::get_by_name(&conn, &pid, "daily").unwrap().unwrap();
        assert!(cleared.active_terminal_id.is_none());
    }

    #[test]
    fn heartbeat_clear_active_by_terminal_id_targets_matching_rows() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-clear-by-tid");
        // Two heartbeats, two distinct terminal ids.
        AgentHeartbeat::insert(&conn, "hb1", &pid, "morning", "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::insert(&conn, "hb2", &pid, "evening", "60m", "{}", "p2", true).unwrap();
        AgentHeartbeat::save_active_terminal_id(&conn, &pid, "morning", "term-1").unwrap();
        AgentHeartbeat::save_active_terminal_id(&conn, &pid, "evening", "term-2").unwrap();

        // Simulate term-1 exiting — should null morning's column only.
        let n = AgentHeartbeat::clear_active_terminal_id_by_terminal(&conn, "term-1").unwrap();
        assert_eq!(n, 1);
        assert!(AgentHeartbeat::get_by_name(&conn, &pid, "morning").unwrap().unwrap().active_terminal_id.is_none());
        assert_eq!(
            AgentHeartbeat::get_by_name(&conn, &pid, "evening").unwrap().unwrap().active_terminal_id.as_deref(),
            Some("term-2"),
        );
    }

    #[test]
    fn heartbeat_list_with_active_terminal_returns_only_non_null() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-list-active");
        AgentHeartbeat::insert(&conn, "hb1", &pid, "with-term", "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::insert(&conn, "hb2", &pid, "no-term", "60m", "{}", "p2", true).unwrap();
        AgentHeartbeat::save_active_terminal_id(&conn, &pid, "with-term", "term-x").unwrap();

        let rows = AgentHeartbeat::list_with_active_terminal(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "with-term");
        assert_eq!(rows[0].2, "term-x");
    }

    #[test]
    fn agent_session_surfaced_round_trip() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/sess-surfaced");

        // Fresh upsert — surfaced defaults to 0.
        WorkspaceSession::upsert(
            &conn, "s1", &pid,
            Some("term-1"), None, "claude", "system", "running",
        )
        .unwrap();
        assert!(!WorkspaceSession::is_surfaced(&conn, &pid).unwrap());

        // Flip on.
        let n = WorkspaceSession::set_surfaced(&conn, &pid, true).unwrap();
        assert_eq!(n, 1);
        assert!(WorkspaceSession::is_surfaced(&conn, &pid).unwrap());

        // Flip off.
        WorkspaceSession::set_surfaced(&conn, &pid, false).unwrap();
        assert!(!WorkspaceSession::is_surfaced(&conn, &pid).unwrap());
    }

    #[test]
    fn workspace_session_surfaced_default_false_for_missing_row() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/sess-missing");
        // No row inserted — query should return false, not error.
        assert!(!WorkspaceSession::is_surfaced(&conn, &pid).unwrap());
    }

    #[test]
    fn heartbeat_archive_sets_timestamp_and_is_idempotent() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-arch");
        AgentHeartbeat::insert(
            &conn, "hb1", &pid, "weekly", "7d", "{}", "p", true,
        )
        .unwrap();

        // First archive — sets the timestamp.
        let n1 = AgentHeartbeat::archive(&conn, &pid, "weekly").unwrap();
        assert_eq!(n1, 1, "first archive updates one row");
        let archived_first = AgentHeartbeat::get_by_name(&conn, &pid, "weekly")
            .unwrap()
            .unwrap()
            .archived_at
            .expect("archived_at set after archive");

        // Second archive — no-op (the WHERE clause excludes already-archived rows).
        let n2 = AgentHeartbeat::archive(&conn, &pid, "weekly").unwrap();
        assert_eq!(n2, 0, "re-archive of an archived row is a no-op");

        let archived_second = AgentHeartbeat::get_by_name(&conn, &pid, "weekly")
            .unwrap()
            .unwrap()
            .archived_at
            .expect("archived_at preserved after no-op re-archive");
        assert_eq!(
            archived_first, archived_second,
            "archived_at timestamp must NOT change on re-archive"
        );
    }

    #[test]
    fn heartbeat_clear_session_id_nulls_only_target_row() {
        // Self-heal (smart_launch) calls clear_session_id when the
        // saved session_id points at a JSONL that no longer exists
        // on disk — daemon-restart-during-spawn race. The clear must
        // be scoped to (project_id, name) and leave other heartbeats
        // and other workspaces untouched.
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-clear-sid");
        AgentHeartbeat::insert(
            &conn, "hb-target", &pid, "fast-test", "hourly", "{}", "p1", true,
        )
        .unwrap();
        AgentHeartbeat::insert(
            &conn, "hb-other", &pid, "other-hb", "hourly", "{}", "p2", true,
        )
        .unwrap();
        // Seed both heartbeats with a session_id; clearing the target
        // must leave the sibling intact.
        AgentHeartbeat::save_session_id(&conn, &pid, "fast-test", "ghost-uuid")
            .unwrap();
        AgentHeartbeat::save_session_id(&conn, &pid, "other-hb", "alive-uuid")
            .unwrap();

        let n = AgentHeartbeat::clear_session_id(&conn, &pid, "fast-test").unwrap();
        assert_eq!(n, 1, "exactly one row should be cleared");

        let cleared = AgentHeartbeat::get_by_name(&conn, &pid, "fast-test")
            .unwrap()
            .unwrap();
        assert!(
            cleared.last_session_id.is_none(),
            "fast-test last_session_id must be NULL after clear"
        );

        let untouched = AgentHeartbeat::get_by_name(&conn, &pid, "other-hb")
            .unwrap()
            .unwrap();
        assert_eq!(
            untouched.last_session_id.as_deref(),
            Some("alive-uuid"),
            "sibling heartbeat's session_id must be preserved"
        );
    }

    #[test]
    fn heartbeat_unarchive_clears_timestamp() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-un");
        AgentHeartbeat::insert(
            &conn, "hb1", &pid, "monthly", "30d", "{}", "p", true,
        )
        .unwrap();
        AgentHeartbeat::archive(&conn, &pid, "monthly").unwrap();
        assert!(AgentHeartbeat::get_by_name(&conn, &pid, "monthly").unwrap().unwrap().archived_at.is_some());

        let n = AgentHeartbeat::unarchive(&conn, &pid, "monthly").unwrap();
        assert_eq!(n, 1);
        assert!(AgentHeartbeat::get_by_name(&conn, &pid, "monthly").unwrap().unwrap().archived_at.is_none());
    }

    #[test]
    fn heartbeat_list_active_excludes_archived() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-lact");
        AgentHeartbeat::insert(&conn, "hb1", &pid, "alpha", "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::insert(&conn, "hb2", &pid, "beta",  "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::insert(&conn, "hb3", &pid, "gamma", "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::archive(&conn, &pid, "beta").unwrap();

        let active = AgentHeartbeat::list_active(&conn, &pid).unwrap();
        let names: Vec<&str> = active.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "gamma"], "archived row must not appear in list_active");
    }

    #[test]
    fn heartbeat_list_archived_returns_only_archived_rows() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-larc");
        AgentHeartbeat::insert(&conn, "hb1", &pid, "alpha", "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::insert(&conn, "hb2", &pid, "beta",  "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::archive(&conn, &pid, "beta").unwrap();

        let archived = AgentHeartbeat::list_archived(&conn, &pid).unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].name, "beta");
        assert!(archived[0].archived_at.is_some());
    }

    #[test]
    fn heartbeat_list_enabled_excludes_archived_even_when_enabled() {
        // The scheduler-tick path uses list_enabled; archiving must
        // stop a heartbeat from firing even if enabled was never
        // toggled off before archive.
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-len");
        AgentHeartbeat::insert(&conn, "hb1", &pid, "live",     "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::insert(&conn, "hb2", &pid, "retired",  "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::archive(&conn, &pid, "retired").unwrap();

        let enabled = AgentHeartbeat::list_enabled(&conn, &pid).unwrap();
        let names: Vec<&str> = enabled.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["live"],
            "archived heartbeat must be skipped by the tick evaluator"
        );
    }

    #[test]
    fn migration_0034_default_show_heartbeat_sessions_is_zero() {
        // Bare projects row — no explicit show_heartbeat_sessions value.
        // Migration 0034 sets DEFAULT 0, so freshly-inserted rows must
        // have it as 0 (silent autonomous mode is the v2-headless default).
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/proj-0034");
        let v: i64 = conn
            .query_row(
                "SELECT show_heartbeat_sessions FROM projects WHERE id = ?1",
                params![pid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, 0, "show_heartbeat_sessions must default to 0 (off)");
    }

    // ── ActivityFeedEntry ─────────────────────────────────────────
    #[test]
    fn activity_feed_insert_and_list_by_project() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/af");
        let id1 = ActivityFeedEntry::insert(
            &conn, &pid, Some("alice"), "wake.start", None, None, None, Some("kick"), None,
        )
        .unwrap();
        let id2 = ActivityFeedEntry::insert(
            &conn, &pid, Some("alice"), "wake.end", None, None, None, Some("done"), None,
        )
        .unwrap();
        assert!(id2 > id1);

        let rows = ActivityFeedEntry::list_by_project(&conn, &pid, 10, 0).unwrap();
        assert_eq!(rows.len(), 2);
        // ORDER BY created_at DESC — newest first. Matching timestamps
        // (unixepoch() resolves to seconds) can tie; we only assert
        // both rows came back.
        assert!(rows.iter().any(|r| r.event_type == "wake.start"));
        assert!(rows.iter().any(|r| r.event_type == "wake.end"));
    }

    #[test]
    fn activity_feed_list_by_actor_matches_actor_from_or_to() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/af-b");
        ActivityFeedEntry::insert(&conn, &pid, Some("alice"), "x", None, None, None, None, None).unwrap();
        ActivityFeedEntry::insert(&conn, &pid, None, "y", Some("alice"), Some("bob"), None, None, None).unwrap();
        ActivityFeedEntry::insert(&conn, &pid, None, "z", Some("carol"), Some("alice"), None, None, None).unwrap();
        ActivityFeedEntry::insert(&conn, &pid, None, "w", Some("bob"), Some("carol"), None, None, None).unwrap();

        let alice_rows = ActivityFeedEntry::list_by_actor(&conn, &pid, "alice", 10).unwrap();
        // 3 rows: actor=alice, from_workspace=alice, to_workspace=alice.
        assert_eq!(alice_rows.len(), 3);
    }

    #[test]
    fn activity_feed_unread_messages_filtered_and_marked() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/af-unread");
        ActivityFeedEntry::insert(
            &conn, &pid, None, "message.sent", Some("alice"), Some("bob"), None, Some("hello"), None,
        )
        .unwrap();
        ActivityFeedEntry::insert(
            &conn, &pid, None, "wake.start", Some("alice"), Some("bob"), None, None, None,
        )
        .unwrap();
        // wake.start should NOT show up in unread messages (filter
        // limits to message.sent/message.delivered).
        let unread = super::get_unread_messages(&conn, &pid, "bob").unwrap();
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0].event_type, "message.sent");

        let n = super::mark_messages_read(&conn, &pid, "bob").unwrap();
        assert_eq!(n, 1);
        let unread_after = super::get_unread_messages(&conn, &pid, "bob").unwrap();
        assert!(unread_after.is_empty(), "after mark_read, no unread should remain");
    }

    // ── HeartbeatFire ─────────────────────────────────────────────
    #[test]
    fn heartbeat_fire_insert_and_list_by_project() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hf");
        HeartbeatFire::insert(
            &conn, &pid, Some("alice"), "agent", "fired", Some("inbox has work"), Some("normal"), Some(2), Some(150),
        )
        .unwrap();
        HeartbeatFire::insert(
            &conn, &pid, Some("bob"), "agent", "skipped", Some("already running"), None, None, None,
        )
        .unwrap();

        let rows = HeartbeatFire::list_by_project(&conn, &pid, 10).unwrap();
        assert_eq!(rows.len(), 2);
        // DESC by fired_at — may or may not tie. Just assert presence.
        let decisions: Vec<&str> = rows.iter().map(|r| r.decision.as_str()).collect();
        assert!(decisions.contains(&"fired"));
        assert!(decisions.contains(&"skipped"));
    }

    #[test]
    fn heartbeat_fire_insert_with_schedule_persists_schedule_name() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hf-sched");
        HeartbeatFire::insert_with_schedule(
            &conn, &pid, Some("alice"), Some("nightly"),
            "agent", "fired", Some("nightly tick"), None, None, Some(42),
        )
        .unwrap();
        let rows = HeartbeatFire::list_by_schedule_name(&conn, &pid, "nightly", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].schedule_name.as_deref(), Some("nightly"));
        assert_eq!(rows[0].duration_ms, Some(42));
    }

    #[test]
    fn heartbeat_fire_prune_before_removes_old_rows() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hf-prune");
        // Insert one row with a known-old timestamp and one fresh.
        conn.execute(
            "INSERT INTO heartbeat_fires (project_id, mode, decision, fired_at) VALUES (?1, 'agent', 'fired', '2020-01-01T00:00:00Z')",
            params![pid],
        )
        .unwrap();
        HeartbeatFire::insert(&conn, &pid, None, "agent", "fired", None, None, None, None).unwrap();

        let removed = HeartbeatFire::prune_before(&conn, "2021-01-01T00:00:00Z").unwrap();
        assert_eq!(removed, 1, "prune should remove 1 old row");
        let remaining = HeartbeatFire::list_by_project(&conn, &pid, 10).unwrap();
        assert_eq!(remaining.len(), 1, "fresh row remains");
    }

    // ── WorkspaceRelation ─────────────────────────────────────────
    #[test]
    fn workspace_relation_create_list_for_source_and_target_delete() {
        let conn = fresh();
        let src = make_project_row(&conn, "/tmp/ws-src");
        let tgt = make_project_row(&conn, "/tmp/ws-tgt");

        WorkspaceRelation::create(&conn, "rel-1", &src, &tgt, "manages").unwrap();

        let from_src = WorkspaceRelation::list_for_source(&conn, &src).unwrap();
        assert_eq!(from_src.len(), 1);
        assert_eq!(from_src[0].target_project_id, tgt);

        let from_tgt = WorkspaceRelation::list_for_target(&conn, &tgt).unwrap();
        assert_eq!(from_tgt.len(), 1);
        assert_eq!(from_tgt[0].source_project_id, src);

        let n = WorkspaceRelation::delete(&conn, "rel-1").unwrap();
        assert_eq!(n, 1);
        assert!(WorkspaceRelation::list_for_source(&conn, &src).unwrap().is_empty());
    }

    // ── WorkspaceRemoteConnection (GAP #3) ─────────────────────────
    #[test]
    fn workspace_remote_connection_create_list_exists_delete() {
        let conn = fresh();
        let src = make_project_row(&conn, "/tmp/remote-src");

        WorkspaceRemoteConnection::create(
            &conn,
            "rc-1",
            &src,
            "ai@rpm.k2.dev",
            "rpm.k2.dev",
            "ai",
            Some("fp-abc"),
        )
        .unwrap();

        // Round-trip read.
        let rows = WorkspaceRemoteConnection::list_for_source(&conn, &src).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].remote_addr, "ai@rpm.k2.dev");
        assert_eq!(rows[0].host, "rpm.k2.dev");
        assert_eq!(rows[0].agent, "ai");
        assert_eq!(rows[0].peer_fingerprint.as_deref(), Some("fp-abc"));

        // exists() — the gate query.
        assert!(WorkspaceRemoteConnection::exists(&conn, &src, "ai@rpm.k2.dev").unwrap());
        assert!(!WorkspaceRemoteConnection::exists(&conn, &src, "ai@other.k2.dev").unwrap());

        // delete() by (source, addr).
        let n = WorkspaceRemoteConnection::delete(&conn, &src, "ai@rpm.k2.dev").unwrap();
        assert_eq!(n, 1);
        assert!(WorkspaceRemoteConnection::list_for_source(&conn, &src).unwrap().is_empty());
        assert!(!WorkspaceRemoteConnection::exists(&conn, &src, "ai@rpm.k2.dev").unwrap());
    }

    // ── AgentPreset seed ──────────────────────────────────────────
    #[test]
    fn agent_preset_seed_populates_built_ins() {
        // isolated_test_connection runs seed_agent_presets — so the
        // built-in presets should be present. Spot-check that Claude
        // and at least one local LLM are there.
        let conn = fresh();
        let presets = AgentPreset::list(&conn).unwrap();
        let labels: Vec<&str> = presets.iter().map(|p| p.label.as_str()).collect();
        assert!(labels.contains(&"Claude"), "Claude preset missing: {:?}", labels);
        assert!(labels.contains(&"Ollama"), "Ollama preset missing: {:?}", labels);
        assert!(presets.len() >= 11, "expected >=11 built-in presets, got {}", presets.len());
    }

    #[test]
    fn agent_preset_seed_is_idempotent_on_reapply() {
        let conn = fresh();
        let before = AgentPreset::list(&conn).unwrap().len();
        // The isolated_test_connection already seeded. A second seed
        // must be a no-op (INSERT OR IGNORE).
        crate::db::seed_agent_presets(&conn).unwrap();
        let after = AgentPreset::list(&conn).unwrap().len();
        assert_eq!(before, after, "re-seed must not duplicate rows");
    }

    // ── SubdomainWorkspace (0074) ─────────────────────────────────
    #[test]
    fn subdomain_workspace_claim_repoint_unclaim_roundtrip() {
        let conn = fresh();
        assert!(SubdomainWorkspace::map(&conn).unwrap().is_empty());

        // Claim normalizes the label lowercase (matching SubdomainMap).
        SubdomainWorkspace::claim(&conn, " Staging ", "proj-1").unwrap();
        let map = SubdomainWorkspace::map(&conn).unwrap();
        assert_eq!(map.get("staging").map(String::as_str), Some("proj-1"));

        // Re-claim by another workspace REPOINTS (PK upsert, one owner).
        SubdomainWorkspace::claim(&conn, "staging", "proj-2").unwrap();
        let map = SubdomainWorkspace::map(&conn).unwrap();
        assert_eq!(map.len(), 1, "re-claim must not duplicate the label row");
        assert_eq!(map.get("staging").map(String::as_str), Some("proj-2"));

        // Unclaim reports whether a row actually existed.
        assert!(SubdomainWorkspace::unclaim(&conn, "STAGING").unwrap());
        assert!(!SubdomainWorkspace::unclaim(&conn, "staging").unwrap());
        assert!(SubdomainWorkspace::map(&conn).unwrap().is_empty());
    }

    #[test]
    fn subdomain_workspace_claim_rejects_blank_inputs() {
        let conn = fresh();
        assert!(SubdomainWorkspace::claim(&conn, "  ", "proj-1").is_err());
        assert!(SubdomainWorkspace::claim(&conn, "staging", "  ").is_err());
        assert!(SubdomainWorkspace::map(&conn).unwrap().is_empty());
    }
}

#[cfg(test)]
mod concurrency_tests {
    //! CAS and multi-connection concurrency tests for schema-level
    //! operations. These tests use file-backed SQLite (via a temp
    //! directory) because in-memory `:memory:` databases are not
    //! shared across `Connection` handles — to actually race
    //! connections we need real disk state.
    //!
    //! The resilience review introduced `try_acquire_running` with
    //! `BEGIN IMMEDIATE` specifically to avoid a TOCTOU race between
    //! two heartbeats firing at the same time. These tests PROVE the
    //! claim by spawning N threads, each opening their own
    //! connection, and asserting exactly one wins the acquisition.
    //! Without these, "concurrency-safe" is just an assertion in the
    //! doc comment.
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Build a unique on-disk DB path and bootstrap it through the
    /// full migration + seed sequence. Caller is responsible for
    /// cleanup (directory removal after the test).
    fn scratch_db() -> (PathBuf, PathBuf) {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2so-schema-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let db_path = dir.join("k2so.db");
        crate::db::bootstrap_test_db_at(&db_path).expect("bootstrap");
        (dir, db_path)
    }

    fn open_conn(path: &std::path::Path) -> Connection {
        crate::db::open_with_resilience(path).expect("open connection")
    }

    fn make_project(conn: &Connection, project_path: &str) -> String {
        // Schema requires a projects row: workspace_sessions.project_id
        // is a FK to projects.id, and PRAGMA foreign_keys is ON.
        // Returns the generated UUID — callers pass that as the
        // project_id arg to try_acquire_running.
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            params![id, "test", project_path],
        )
        .expect("insert project");
        id
    }

    #[test]
    fn try_acquire_running_exactly_one_winner_under_parallel_contention() {
        // 20 threads race to acquire the same workspace's session
        // lock. Exactly one must return Ok(true); all others Ok(false).
        // The pre-0.32.9 is_locked() → spawn → upsert sequence had a
        // TOCTOU here; BEGIN IMMEDIATE closes it. This test is the
        // proof.
        let (dir, db_path) = scratch_db();
        let project_id = {
            let conn = open_conn(&db_path);
            make_project(&conn, "/tmp/proj-a")
        };

        let db_path = Arc::new(db_path);
        let project = Arc::new(project_id);
        let n_threads = 20usize;

        let handles: Vec<_> = (0..n_threads)
            .map(|tid| {
                let db_path = db_path.clone();
                let project = project.clone();
                std::thread::spawn(move || -> bool {
                    let conn = open_conn(&db_path);
                    WorkspaceSession::try_acquire_running(
                        &conn,
                        &format!("session-{}", tid),
                        &project,
                        None,
                        "claude",
                        "manager",
                    )
                    .expect("try_acquire_running")
                })
            })
            .collect();

        let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let winners = results.iter().filter(|&&r| r).count();
        assert_eq!(
            winners, 1,
            "expected exactly 1 winner under contention, got {}: results={:?}",
            winners, results
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_acquire_running_different_workspaces_all_succeed() {
        // The CAS is per-workspace (project_id), not global. Eight
        // different workspaces, each acquired by its own thread, all
        // should win. Replaces the pre-0.37.0 "different agents same
        // project" test, which is now unreachable — the schema-level
        // UNIQUE(project_id) makes one-row-per-workspace the law.
        let (dir, db_path) = scratch_db();
        let n_workspaces = 8usize;
        let project_ids: Vec<String> = {
            let conn = open_conn(&db_path);
            (0..n_workspaces)
                .map(|i| make_project(&conn, &format!("/tmp/proj-multi-{i}")))
                .collect()
        };

        let db_path = Arc::new(db_path);
        let handles: Vec<_> = project_ids
            .into_iter()
            .enumerate()
            .map(|(i, project)| {
                let db_path = db_path.clone();
                std::thread::spawn(move || -> bool {
                    let conn = open_conn(&db_path);
                    WorkspaceSession::try_acquire_running(
                        &conn,
                        &format!("session-w{}", i),
                        &project,
                        None,
                        "claude",
                        "manager",
                    )
                    .expect("try_acquire_running")
                })
            })
            .collect();

        let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let winners = results.iter().filter(|&&r| r).count();
        assert_eq!(winners, n_workspaces, "different workspaces should all win: {:?}", results);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_acquire_running_serializes_without_busy_errors() {
        // 5 rounds of 10 threads each. After each round the winner
        // releases the lock (status='stopped'). Next round must also
        // produce exactly one winner. Verifies that BEGIN IMMEDIATE
        // + busy_timeout doesn't surface SQLITE_BUSY as an error —
        // instead callers block on the write queue until they get
        // their turn.
        let (dir, db_path) = scratch_db();
        let project_id = {
            let conn = open_conn(&db_path);
            make_project(&conn, "/tmp/proj-c")
        };

        for round in 0..5 {
            let db_path = Arc::new(db_path.clone());
            let project = Arc::new(project_id.clone());

            let handles: Vec<_> = (0..10)
                .map(|tid| {
                    let db_path = db_path.clone();
                    let project = project.clone();
                    std::thread::spawn(move || -> bool {
                        let conn = open_conn(&db_path);
                        WorkspaceSession::try_acquire_running(
                            &conn,
                            &format!("session-r{}-t{}", round, tid),
                            &project,
                            None,
                            "claude",
                            "manager",
                        )
                        .expect("try_acquire_running should never error, only return false")
                    })
                })
                .collect();

            let winners: usize = handles
                .into_iter()
                .map(|h| h.join().unwrap() as usize)
                .sum();
            assert_eq!(winners, 1, "round {}: expected 1 winner, got {}", round, winners);

            // Release the lock so the next round has something to acquire.
            let conn = open_conn(&db_path);
            conn.execute(
                "UPDATE workspace_sessions SET status='stopped' WHERE project_id=?1",
                params![project_id],
            ).expect("release lock");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_acquire_running_reacquires_after_release() {
        // Single-threaded correctness check: acquire → release →
        // re-acquire must all return Ok(true). Catches a regression
        // where the CAS could treat a released row as still held.
        let (dir, db_path) = scratch_db();
        let conn = open_conn(&db_path);
        let project = make_project(&conn, "/tmp/proj-d");

        let first = WorkspaceSession::try_acquire_running(
            &conn, "s1", &project, None, "claude", "manager",
        )
        .unwrap();
        assert!(first, "first acquire should win");

        let second = WorkspaceSession::try_acquire_running(
            &conn, "s2", &project, None, "claude", "manager",
        )
        .unwrap();
        assert!(!second, "second acquire (already held) should lose");

        // Release (status != 'running').
        conn.execute(
            "UPDATE workspace_sessions SET status='stopped' WHERE project_id=?1",
            params![project],
        )
        .unwrap();

        let third = WorkspaceSession::try_acquire_running(
            &conn, "s3", &project, None, "claude", "manager",
        )
        .unwrap();
        assert!(third, "re-acquire after release should win");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Heartbeat lease (try_acquire_heartbeat) ──────────────────────
    //
    // Mirrors the WorkspaceSession CAS tests above. The pre-P5 production
    // path had no concurrency control on heartbeat fires — two ticks
    // overlapping (which becomes a real risk once StartInterval drops
    // from 300 to 60 in P5.7) would spawn the same heartbeat twice.
    // These tests prove the new CAS prevents that.

    fn make_heartbeat(conn: &Connection, project_id: &str, name: &str) {
        AgentHeartbeat::insert(
            conn,
            &uuid::Uuid::new_v4().to_string(),
            project_id,
            name,
            "hourly",
            r#"{"every_seconds":3600}"#,
            ".k2so/agents/x/wakeups/test/WAKEUP.md",
            true,
        )
        .expect("insert heartbeat");
    }

    #[test]
    fn try_acquire_heartbeat_exactly_one_winner_under_parallel_contention() {
        let (dir, db_path) = scratch_db();
        let project = {
            let conn = open_conn(&db_path);
            let project = make_project(&conn, "/tmp/proj-hb-a");
            make_heartbeat(&conn, &project, "test-hb");
            project
        };

        let db_path = Arc::new(db_path);
        let project = Arc::new(project);
        let n_threads = 20usize;

        let handles: Vec<_> = (0..n_threads)
            .map(|_| {
                let db_path = db_path.clone();
                let project = project.clone();
                std::thread::spawn(move || -> bool {
                    let conn = open_conn(&db_path);
                    AgentHeartbeat::try_acquire_heartbeat(&conn, &project, "test-hb")
                        .expect("try_acquire_heartbeat")
                })
            })
            .collect();

        let winners: usize = handles
            .into_iter()
            .map(|h| h.join().unwrap() as usize)
            .sum();
        assert_eq!(winners, 1, "expected exactly 1 winner under contention");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_acquire_heartbeat_release_allows_reacquire() {
        let (dir, db_path) = scratch_db();
        let conn = open_conn(&db_path);
        let project = make_project(&conn, "/tmp/proj-hb-b");
        make_heartbeat(&conn, &project, "test-hb");

        let first = AgentHeartbeat::try_acquire_heartbeat(&conn, &project, "test-hb")
            .expect("first acquire");
        assert!(first, "first acquire should win");

        let second = AgentHeartbeat::try_acquire_heartbeat(&conn, &project, "test-hb")
            .expect("second acquire");
        assert!(!second, "second acquire (lease held) should lose");

        AgentHeartbeat::release_heartbeat_lease(&conn, &project, "test-hb")
            .expect("release lease");

        let third = AgentHeartbeat::try_acquire_heartbeat(&conn, &project, "test-hb")
            .expect("third acquire");
        assert!(third, "re-acquire after release should win");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stamp_fired_and_release_clears_lease() {
        let (dir, db_path) = scratch_db();
        let conn = open_conn(&db_path);
        let project = make_project(&conn, "/tmp/proj-hb-c");
        make_heartbeat(&conn, &project, "test-hb");

        AgentHeartbeat::try_acquire_heartbeat(&conn, &project, "test-hb")
            .expect("acquire");
        AgentHeartbeat::stamp_fired_and_release(&conn, &project, "test-hb")
            .expect("stamp");

        let row = AgentHeartbeat::get_by_name(&conn, &project, "test-hb")
            .expect("get")
            .expect("row exists");
        assert!(row.in_flight_started_at.is_none(), "lease should be cleared");
        assert!(row.last_fired.is_some(), "last_fired should be stamped");

        // Re-acquire works because the lease was cleared.
        let next = AgentHeartbeat::try_acquire_heartbeat(&conn, &project, "test-hb")
            .expect("re-acquire");
        assert!(next, "post-stamp re-acquire should win");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_stale_leases_clears_old_in_flight_rows() {
        let (dir, db_path) = scratch_db();
        let conn = open_conn(&db_path);
        let project = make_project(&conn, "/tmp/proj-hb-d");
        make_heartbeat(&conn, &project, "stale-hb");
        make_heartbeat(&conn, &project, "fresh-hb");

        // Stale lease: 1 hour old.
        let stale_ts = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        conn.execute(
            "UPDATE workspace_heartbeats SET in_flight_started_at = ?1 WHERE name = 'stale-hb'",
            params![stale_ts],
        )
        .unwrap();
        // Fresh lease: just now.
        AgentHeartbeat::try_acquire_heartbeat(&conn, &project, "fresh-hb")
            .expect("acquire fresh");

        // Sweep anything older than 5 minutes.
        let cleared = AgentHeartbeat::sweep_stale_leases(&conn, 300).expect("sweep");
        assert_eq!(cleared, 1, "exactly one stale lease should be cleared");

        let stale = AgentHeartbeat::get_by_name(&conn, &project, "stale-hb")
            .unwrap()
            .unwrap();
        assert!(stale.in_flight_started_at.is_none(), "stale lease cleared");

        let fresh = AgentHeartbeat::get_by_name(&conn, &project, "fresh-hb")
            .unwrap()
            .unwrap();
        assert!(fresh.in_flight_started_at.is_some(), "fresh lease preserved");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn release_lease_if_stale_only_clears_old_lease_for_named_row() {
        let (dir, db_path) = scratch_db();
        let conn = open_conn(&db_path);
        let project = make_project(&conn, "/tmp/proj-hb-watchdog");
        make_heartbeat(&conn, &project, "hung-hb");
        make_heartbeat(&conn, &project, "other-hb");

        // A fresh lease must survive the conditional release (the
        // watchdog must never clobber a lease re-acquired by a later
        // fire attempt).
        AgentHeartbeat::try_acquire_heartbeat(&conn, &project, "hung-hb").expect("acquire");
        let cleared = AgentHeartbeat::release_lease_if_stale(&conn, &project, "hung-hb", 120)
            .expect("conditional release");
        assert_eq!(cleared, 0, "a fresh lease must not be force-released");

        // Age the lease past the cutoff → cleared, and only this row.
        let old = (chrono::Utc::now() - chrono::Duration::seconds(300)).to_rfc3339();
        conn.execute(
            "UPDATE workspace_heartbeats SET in_flight_started_at = ?1 WHERE name = 'hung-hb'",
            params![old],
        )
        .unwrap();
        AgentHeartbeat::try_acquire_heartbeat(&conn, &project, "other-hb").expect("acquire other");
        let cleared = AgentHeartbeat::release_lease_if_stale(&conn, &project, "hung-hb", 120)
            .expect("conditional release");
        assert_eq!(cleared, 1, "stale lease must be force-released");
        let other = AgentHeartbeat::get_by_name(&conn, &project, "other-hb").unwrap().unwrap();
        assert!(other.in_flight_started_at.is_some(), "unrelated row's lease untouched");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn note_fire_failure_counts_and_success_resets() {
        let (dir, db_path) = scratch_db();
        let conn = open_conn(&db_path);
        let project = make_project(&conn, "/tmp/proj-hb-failures");
        make_heartbeat(&conn, &project, "flaky-hb");

        let retry = (chrono::Utc::now() + chrono::Duration::seconds(60)).to_rfc3339();
        let n1 = AgentHeartbeat::note_fire_failure(&conn, &project, "flaky-hb", Some(&retry))
            .expect("failure 1");
        assert_eq!(n1, 1);
        let n2 = AgentHeartbeat::note_fire_failure(&conn, &project, "flaky-hb", Some(&retry))
            .expect("failure 2");
        assert_eq!(n2, 2, "consecutive failures must accumulate");
        let row = AgentHeartbeat::get_by_name(&conn, &project, "flaky-hb").unwrap().unwrap();
        assert_eq!(row.next_retry_at.as_deref(), Some(retry.as_str()));

        // A successful fire resets the counter + backoff window.
        AgentHeartbeat::stamp_fired_and_release(&conn, &project, "flaky-hb").expect("stamp");
        let row = AgentHeartbeat::get_by_name(&conn, &project, "flaky-hb").unwrap().unwrap();
        assert_eq!(row.consecutive_failures, 0, "success must reset the failure counter");
        assert!(row.next_retry_at.is_none(), "success must clear the backoff window");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_disable_records_reason_and_reenable_clears_state() {
        let (dir, db_path) = scratch_db();
        let conn = open_conn(&db_path);
        let project = make_project(&conn, "/tmp/proj-hb-disable");
        make_heartbeat(&conn, &project, "doomed-hb");
        conn.execute(
            "UPDATE workspace_heartbeats SET enabled = 1 WHERE name = 'doomed-hb'",
            [],
        )
        .unwrap();
        AgentHeartbeat::note_fire_failure(&conn, &project, "doomed-hb", None).expect("failure");

        AgentHeartbeat::auto_disable(&conn, &project, "doomed-hb", "failures")
            .expect("auto disable");
        let row = AgentHeartbeat::get_by_name(&conn, &project, "doomed-hb").unwrap().unwrap();
        assert!(!row.enabled, "auto_disable must flip enabled off");
        assert_eq!(
            row.disabled_reason.as_deref(),
            Some("failures"),
            "auto_disable must record WHY for the UI badge",
        );

        // Manual re-enable = clean slate: reason, counter, backoff all reset.
        AgentHeartbeat::set_enabled(&conn, &project, "doomed-hb", true).expect("re-enable");
        let row = AgentHeartbeat::get_by_name(&conn, &project, "doomed-hb").unwrap().unwrap();
        assert!(row.enabled);
        assert!(row.disabled_reason.is_none(), "re-enable must clear disabled_reason");
        assert_eq!(row.consecutive_failures, 0, "re-enable must reset the failure counter");
        assert!(row.next_retry_at.is_none(), "re-enable must clear the backoff window");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn schedule_error_set_clear_and_edit_clears() {
        let (dir, db_path) = scratch_db();
        let conn = open_conn(&db_path);
        let project = make_project(&conn, "/tmp/proj-hb-scherr");
        make_heartbeat(&conn, &project, "broken-hb");

        AgentHeartbeat::set_schedule_error(&conn, &project, "broken-hb", Some("bad spec"))
            .expect("set error");
        let row = AgentHeartbeat::get_by_name(&conn, &project, "broken-hb").unwrap().unwrap();
        assert_eq!(row.schedule_error.as_deref(), Some("bad spec"));

        // Editing the schedule clears the recorded error.
        AgentHeartbeat::update_schedule(&conn, &project, "broken-hb", "daily", r#"{"time":"09:00"}"#)
            .expect("edit");
        let row = AgentHeartbeat::get_by_name(&conn, &project, "broken-hb").unwrap().unwrap();
        assert!(row.schedule_error.is_none(), "editing the schedule must clear schedule_error");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scheduler_meta_roundtrips_and_upserts() {
        let (dir, db_path) = scratch_db();
        let conn = open_conn(&db_path);
        assert_eq!(SchedulerMeta::get(&conn, SchedulerMeta::LAST_TICK_AT), None);
        SchedulerMeta::set(&conn, SchedulerMeta::LAST_TICK_AT, "2026-07-02T10:00:00Z")
            .expect("set");
        SchedulerMeta::set(&conn, SchedulerMeta::LAST_TICK_AT, "2026-07-02T10:01:00Z")
            .expect("upsert");
        assert_eq!(
            SchedulerMeta::get(&conn, SchedulerMeta::LAST_TICK_AT).as_deref(),
            Some("2026-07-02T10:01:00Z"),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_acquire_heartbeat_allow_policy_skips_lease_check() {
        let (dir, db_path) = scratch_db();
        let conn = open_conn(&db_path);
        let project = make_project(&conn, "/tmp/proj-hb-e");
        make_heartbeat(&conn, &project, "allow-hb");

        // Flip policy to 'allow'.
        conn.execute(
            "UPDATE workspace_heartbeats SET concurrency_policy = 'allow' WHERE name = 'allow-hb'",
            [],
        )
        .unwrap();

        let first = AgentHeartbeat::try_acquire_heartbeat(&conn, &project, "allow-hb")
            .expect("first");
        let second = AgentHeartbeat::try_acquire_heartbeat(&conn, &project, "allow-hb")
            .expect("second");
        assert!(first && second, "allow policy permits both fires");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
