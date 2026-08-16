//! Workspace lifecycle DB ops — create / open / cleanup.
//!
//! These power the `/cli/workspace/{create,open,cleanup}` routes. They
//! insert / delete rows in the `projects` + `workspaces` tables and
//! emit `HookEvent::SyncProjects` so the Tauri UI refreshes when it's
//! running.
//!
//! Not included here: `/cli/workspace/remove` with a teardown mode.
//! That path depends on `teardown_workspace_harness_files` (symlink
//! freeze/restore of the HARNESS_WORKSPACE_FILES list + .aider.conf.yml
//! archive resolution) which still lives in src-tauri. Remove-with-
//! teardown stays Tauri-served until that helper moves to core.

use std::fs;
use std::path::Path;

use crate::agent_hooks::{emit, HookEvent};

fn run_git(args: &[&str], cwd: &str) -> Option<String> {
    std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
}

/// Register an existing folder as a K2SO workspace — inserts rows in
/// `projects` + `workspaces` under a transaction. Errors if the path
/// is already registered.
///
/// Shared between `/cli/workspace/create` (which just wraps this with
/// a directory-exists precheck + `fs::create_dir_all`) and
/// `/cli/workspace/open` (which pre-checks `is_dir`).
pub fn register_workspace(path: &str) -> Result<String, String> {
    register_workspace_ex(path, true, true, false)
}

/// `seed_wiki` / `seed_agents_md` default on; `fanout` default off.
pub fn register_workspace_ex(
    path: &str,
    seed_wiki: bool,
    seed_agents_md: bool,
    fanout: bool,
) -> Result<String, String> {
    let db = crate::db::shared();
    let conn = db.lock();

    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM projects WHERE path = ?1",
            rusqlite::params![path],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if exists {
        return Err(format!("Workspace already registered: {}", path));
    }

    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let project_id = uuid::Uuid::new_v4().to_string();
    let workspace_id = uuid::Uuid::new_v4().to_string();

    let branch = run_git(&["rev-parse", "--abbrev-ref", "HEAD"], path)
        .unwrap_or_else(|| "main".to_string());

    let tab_order: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(tab_order), -1) + 1 FROM projects",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    conn.execute_batch("BEGIN").map_err(|e| e.to_string())?;

    let insert_result = (|| -> Result<(), String> {
        // Context management stack: workspaces are always "custom" agents going
        // forward (no Off/Manager/K2 type UX). Set agent_mode=custom +
        // agent_enabled=1 on create so heartbeats / agent features work
        // without a mode radio.
        let handle = crate::workspace::handle::mint_handle_for_create(&conn, &name);
        conn.execute(
            "INSERT INTO projects (id, name, path, color, tab_order, worktree_mode, icon_url, focus_group_id, agent_mode, agent_enabled, handle) \
             VALUES (?1, ?2, ?3, '#3b82f6', ?4, 0, NULL, NULL, 'custom', 1, ?5)",
            rusqlite::params![project_id, name, path, tab_order, handle],
        )
        .map_err(|e| format!("Failed to create project: {}", e))?;

        conn.execute(
            "INSERT INTO workspaces (id, project_id, section_id, type, branch, name, tab_order, worktree_path) \
             VALUES (?1, ?2, NULL, 'branch', ?3, ?3, 0, NULL)",
            rusqlite::params![workspace_id, project_id, branch],
        )
        .map_err(|e| format!("Failed to create workspace: {}", e))?;
        Ok(())
    })();

    match insert_result {
        Ok(()) => {
            let _ = conn.execute_batch("COMMIT");
            emit(HookEvent::SyncProjects, serde_json::Value::Null);
            if seed_wiki {
                crate::wiki::seed_wiki_on_add(path);
            }
            crate::workspace::skill_regen::apply_new_workspace_agents_policy(
                path,
                seed_agents_md,
                fanout,
            );
            Ok(serde_json::json!({
                "success": true,
                "projectId": project_id,
                "workspaceId": workspace_id,
                "name": name,
                "path": path,
                "wikiSeeded": seed_wiki,
                "agentsMdSeeded": seed_agents_md,
                "fanout": fanout,
            })
            .to_string())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// `/cli/workspace/create` — create the directory if missing, then
/// [`register_workspace`].
pub fn create_workspace(path: &str) -> Result<String, String> {
    create_workspace_ex(path, true, true, false)
}

pub fn create_workspace_ex(
    path: &str,
    seed_wiki: bool,
    seed_agents_md: bool,
    fanout: bool,
) -> Result<String, String> {
    if path.is_empty() {
        return Err("Missing 'path' parameter".to_string());
    }
    if Path::new(path).exists() {
        return Err(format!("Directory already exists: {}", path));
    }
    fs::create_dir_all(path).map_err(|e| format!("Failed to create directory: {}", e))?;
    register_workspace_ex(path, seed_wiki, seed_agents_md, fanout)
}

/// `/cli/workspace/open` — verify the path is an existing directory
/// and register it.
pub fn open_workspace(path: &str) -> Result<String, String> {
    open_workspace_ex(path, true, true, false)
}

pub fn open_workspace_ex(
    path: &str,
    seed_wiki: bool,
    seed_agents_md: bool,
    fanout: bool,
) -> Result<String, String> {
    if path.is_empty() {
        return Err("Missing 'path' parameter".to_string());
    }
    if !Path::new(path).is_dir() {
        return Err(format!("Directory not found: {}", path));
    }
    register_workspace_ex(path, seed_wiki, seed_agents_md, fanout)
}

/// `/cli/workspace/cleanup` — drop `workspaces` rows whose
/// `worktree_path` no longer exists on disk. Returns the list of
/// removed paths so the UI can show which entries were stale.
pub fn cleanup_stale_workspaces() -> Result<String, String> {
    let db = crate::db::shared();
    let conn = db.lock();

    let mut stmt = conn
        .prepare(
            "SELECT id, worktree_path FROM workspaces WHERE worktree_path IS NOT NULL AND worktree_path != ''",
        )
        .map_err(|e| e.to_string())?;
    let stale: Vec<(String, String)> = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .filter(|(_, path)| !Path::new(path).exists())
        .collect();

    let removed = stale.len();
    for (id, _) in &stale {
        let _ = conn.execute(
            "DELETE FROM workspaces WHERE id = ?1",
            rusqlite::params![id],
        );
    }
    emit(HookEvent::SyncProjects, serde_json::Value::Null);
    Ok(serde_json::json!({
        "removed": removed,
        "stale": stale.iter().map(|(_, p)| p.clone()).collect::<Vec<_>>(),
    })
    .to_string())
}

/// `/cli/workspace/remove` — DB-only variant (no teardown mode). Drops
/// the project + workspace rows. Callers that need the teardown modes
/// (`keep_current` / `restore_original`) continue to use the Tauri-side
/// handler, which performs the symlink freeze/restore before delegating
/// here.
pub fn remove_workspace_db_only(path: &str) -> Result<String, String> {
    if path.is_empty() {
        return Err("Missing 'path' parameter".to_string());
    }
    let db = crate::db::shared();
    let conn = db.lock();

    let project_id: String = conn
        .query_row(
            "SELECT id FROM projects WHERE path = ?1",
            rusqlite::params![path],
            |row| row.get(0),
        )
        .map_err(|_| format!("Workspace not found: {}", path))?;

    // Project-group memberships first (no SQL FK on workspace_id —
    // 0066): callers are §4.5 PoC-guarded, so this only ever removes
    // plain memberships. Affected groups get members-changed below.
    let member_groups =
        crate::project_groups::remove_workspace_memberships_with(&conn, &project_id)?;

    conn.execute(
        "DELETE FROM workspaces WHERE project_id = ?1",
        rusqlite::params![project_id],
    )
    .map_err(|e| format!("Failed to delete workspaces: {}", e))?;
    conn.execute(
        "DELETE FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
    )
    .map_err(|e| format!("Failed to delete project: {}", e))?;

    for gid in &member_groups {
        emit(
            HookEvent::ProjectGroupMembersChanged,
            serde_json::json!({ "groupId": gid }),
        );
    }
    emit(HookEvent::SyncProjects, serde_json::Value::Null);
    Ok(serde_json::json!({
        "success": true,
        "removed": path,
        "teardown": serde_json::Value::Null,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn unique_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2-lifecycle-{}-{}-{}",
            label,
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn register_workspace_seeds_wiki_by_default() {
        crate::db::init_for_tests();
        let dir = unique_dir("seed-on");
        let path = dir.to_string_lossy().into_owned();
        register_workspace(&path).expect("register");
        assert!(
            dir.join(".k2/wiki/Home.md").is_file(),
            "default register must seed Home.md"
        );
        assert!(
            dir.join(".k2/wiki/_Index.md").is_file(),
            "default register must seed _Index.md"
        );
        assert!(
            dir.join(".k2/AGENTS.md").is_file(),
            "default register must compose .k2/AGENTS.md"
        );
        assert!(
            dir.join("AGENTS.md").is_file(),
            "default register must plant cwd AGENTS.md"
        );
        assert!(
            !dir.join("CLAUDE.md").exists(),
            "default register must not plant leftover CLAUDE.md"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn register_workspace_ex_false_skips_wiki() {
        crate::db::init_for_tests();
        let dir = unique_dir("seed-off");
        let path = dir.to_string_lossy().into_owned();
        register_workspace_ex(&path, false, true, false).expect("register");
        assert!(
            !dir.join(".k2/wiki").exists(),
            "opt-out must not create .k2/wiki"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
