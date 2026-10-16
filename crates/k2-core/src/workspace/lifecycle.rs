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

/// Canonical refuse-duplicate copy (prd-workspace-already-exists-copy-v1 C2).
/// `{name}` is [`crate::workspace::display::agent_display_name`] — display
/// if set, else folder / `projects.name`. Never the raw path.
pub fn workspace_already_exists_error(path: &str) -> String {
    format!(
        "workspace already exists {}",
        crate::workspace::display::agent_display_name(path)
    )
}

/// Hire/idempotent match: old register copy OR C2 (prd C3).
pub fn is_already_registered(err: &str) -> bool {
    err.contains("already registered") || err.contains("workspace already exists")
}

/// UNIQUE `projects.path` races map onto C2 instead of a constraint dump.
pub fn map_projects_path_unique(path: &str, err: impl std::fmt::Display) -> String {
    let s = err.to_string();
    if s.contains("UNIQUE constraint failed") && s.contains("projects.path") {
        workspace_already_exists_error(path)
    } else {
        s
    }
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
        return Err(workspace_already_exists_error(path));
    }

    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let project_id = uuid::Uuid::new_v4().to_string();
    let workspace_id = uuid::Uuid::new_v4().to_string();

    let branch =
        run_git(&["rev-parse", "--abbrev-ref", "HEAD"], path).unwrap_or_else(|| "main".to_string());

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
        .map_err(|e| {
            let mapped = map_projects_path_unique(path, &e);
            if is_already_registered(&mapped) {
                mapped
            } else {
                format!("Failed to create project: {}", e)
            }
        })?;

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
            crate::cli_folder_trust::trust_cli_folder(path);
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
    create_workspace_dir(path)?;
    register_workspace_ex(path, seed_wiki, seed_agents_md, fanout)
}

/// Make a new workspace folder, missing parents included, as the
/// daemon's own user (no root helper, no extra mode bits beyond the
/// umask). The path must be absolute: a relative one would land in the
/// daemon's cwd, never where the caller meant.
///
/// A permission error names the nearest existing folder the daemon could
/// not write to, so a bad path (e.g. `/hearth` instead of
/// `/home/k2/ai/hearth`) reads as "you can't write there", not a bare
/// `Permission denied (os error 13)`.
pub fn create_workspace_dir(path: &str) -> Result<(), String> {
    let target = Path::new(path);
    if !target.is_absolute() {
        return Err(format!("Workspace path must be absolute: {path}"));
    }
    match fs::create_dir_all(target) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let blocker = target
                .ancestors()
                .skip(1)
                .find(|a| a.exists())
                .unwrap_or_else(|| Path::new("/"));
            let user = std::env::var("USER")
                .or_else(|_| std::env::var("LOGNAME"))
                .unwrap_or_else(|_| "the daemon's user".to_string());
            let home = std::env::var("HOME").unwrap_or_default();
            let hint = if home.is_empty() {
                String::new()
            } else {
                format!(" Pick a folder you can write to, e.g. under {home}.")
            };
            Err(format!(
                "Cannot create workspace {path}: {} is not writable by {user} (permission denied).{hint}",
                blocker.display()
            ))
        }
        Err(e) => Err(format!("Failed to create directory {path}: {e}")),
    }
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

    fn count_projects(path: &str) -> i64 {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM projects WHERE path = ?1",
            rusqlite::params![path],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn count_workspaces(path: &str) -> i64 {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM workspaces w JOIN projects p ON w.project_id = p.id WHERE p.path = ?1",
            rusqlite::params![path],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn is_already_registered_matches_old_copy_and_c2() {
        assert!(is_already_registered(
            "Workspace already registered: /tmp/x"
        ));
        assert!(is_already_registered("workspace already exists Cortana"));
        assert!(!is_already_registered("Directory not found: /tmp/x"));
        assert!(!is_already_registered("Failed to add workspace"));
        assert!(!is_already_registered("Directory already exists: /tmp/x"));
    }

    #[test]
    fn register_twice_is_c2_copy_one_row() {
        crate::db::init_for_tests();
        let dir = unique_dir("already-exists");
        let path = dir.to_string_lossy().into_owned();
        register_workspace_ex(&path, false, false, false).expect("first register");
        crate::workspace::display::set_agent_display_name(&path, "Cortana")
            .expect("pin display name");
        let err = register_workspace_ex(&path, false, false, false).unwrap_err();
        assert_eq!(err, "workspace already exists Cortana");
        assert!(!err.contains("Failed to add workspace"));
        assert!(!err.contains(&path), "C2 must not use the raw path: {err}");
        assert_eq!(count_projects(&path), 1, "still one projects row");
        assert_eq!(count_workspaces(&path), 1, "still one workspaces row");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_workspace_makes_missing_nested_parents() {
        crate::db::init_for_tests();
        let root = unique_dir("missing-parents");
        // Field report 2026-10-16: /home/k2/ai did not exist yet.
        let target = root.join("ai").join("team").join("hearth");
        let path = target.to_string_lossy().into_owned();
        assert!(!root.join("ai").exists(), "precondition: parent missing");
        let body = create_workspace_ex(&path, false, false, false)
            .expect("create under a missing nested parent must succeed");
        assert!(target.is_dir(), "workspace dir not created: {path}");
        let v: serde_json::Value = serde_json::from_str(&body).expect("json body");
        assert_eq!(v["success"], serde_json::Value::Bool(true));
        assert_eq!(v["path"], serde_json::Value::String(path.clone()));
        assert_eq!(count_projects(&path), 1, "registered once");
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn create_workspace_dir_rejects_relative_path() {
        let err = create_workspace_dir("ai/hearth").unwrap_err();
        assert_eq!(err, "Workspace path must be absolute: ai/hearth");
        assert!(
            !Path::new("ai").exists(),
            "relative path must not be created in the cwd"
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_workspace_dir_unwritable_root_names_the_blocker() {
        use std::os::unix::fs::PermissionsExt;
        let root = unique_dir("unwritable");
        let locked = root.join("locked");
        std::fs::create_dir(&locked).expect("mkdir locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555))
            .expect("chmod 0555");
        let target = locked.join("ai").join("hearth");
        let path = target.to_string_lossy().into_owned();
        let res = create_workspace_dir(&path);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
            .expect("chmod back");
        let err = res.expect_err("create under a read-only folder must fail");
        let blocker = locked.to_string_lossy().into_owned();
        assert!(
            err.starts_with(&format!(
                "Cannot create workspace {path}: {blocker} is not writable by "
            )),
            "error must name the unwritable folder, got: {err}"
        );
        assert!(err.contains("(permission denied)"), "got: {err}");
        assert!(!err.contains("os error 13"), "no bare errno copy: {err}");
        assert!(!locked.join("ai").exists(), "nothing created");
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn open_twice_is_c2_copy_one_row() {
        crate::db::init_for_tests();
        let dir = unique_dir("open-twice");
        let path = dir.to_string_lossy().into_owned();
        open_workspace_ex(&path, false, false, false).expect("first open");
        crate::workspace::display::set_agent_display_name(&path, "Cortana")
            .expect("pin display name");
        let err = open_workspace_ex(&path, false, false, false).unwrap_err();
        assert_eq!(err, "workspace already exists Cortana");
        assert_eq!(count_projects(&path), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
