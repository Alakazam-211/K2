//! Agent "channel" ops — status updates, done/blocked, reservations.
//!
//! These are the lightweight per-agent signals the CLI injects during
//! a work session:
//!
//! - [`status`] — update the agent's `agent_sessions.status_message`.
//!   Also writes a `status` activity-feed entry so the UI history
//!   shows what the agent was doing at each tick.
//! - [`done`] — mark the workspace's in-flight work item complete:
//!   move the first item in `.k2so/inbox/active/` to `done/` via the
//!   unified `crate::inbox::*` primitive, flip the session to
//!   `sleeping`, log `task.done` (or `task.blocked` when the caller
//!   passes a reason).
//! - [`reserve`] — claim a set of filesystem paths for exclusive
//!   editing. Writes a JSON registry at `.k2so/reservations.json`.
//!   Callers include a comma-separated path list; the function
//!   returns `{ reserved: [...], conflicts: [...] }`.
//! - [`release`] — drop reservations held by this agent (all or a
//!   named subset).
//!
//! All four land in k2so-core so the daemon serves them headlessly.
//! Tauri keeps its `#[tauri::command]` handlers as thin forwards.

use std::fs;
use std::path::{Path, PathBuf};

use crate::db::schema::{log_activity, WorkspaceSession};
use crate::workspace::agent_identity::resolve_project_id;

fn open_project(project_path: &str) -> Result<String, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    resolve_project_id(&conn, project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))
}

/// Update the agent's status message + log a `status` activity entry.
pub fn status(
    project_path: String,
    agent: String,
    message: String,
) -> Result<serde_json::Value, String> {
    if agent.is_empty() {
        return Err("Missing 'agent' parameter".to_string());
    }
    let project_id = open_project(&project_path)?;

    let db = crate::db::shared();
    let conn = db.lock();

    WorkspaceSession::update_status_message(&conn, &project_id, &message)
        .map_err(|e| format!("Failed to update status: {}", e))?;

    log_activity(
        &conn,
        &project_id,
        Some(&agent),
        "status",
        Some(&agent),
        None,
        None,
        Some(&message),
    );

    Ok(serde_json::json!({ "success": true }))
}

/// Complete (or block) the workspace's in-flight work item. Moves
/// the first item in `.k2so/inbox/active/` to `.k2so/inbox/done/`,
/// flips the session to `sleeping`, logs an activity entry.
/// `blocked = Some(reason)` swaps the event type to `task.blocked`.
///
/// Post-Phase-2.5b the workspace IS the agent — so "the agent's
/// in-flight task" lives at workspace level in the unified inbox,
/// not in the retired per-agent `.k2so/agents/<name>/work/` tree.
/// Routes through `crate::inbox::*` so the same primitive that
/// powers `checkin` / `inbox` / scheduler reads governs the
/// completion write too.
pub fn done(
    project_path: String,
    agent: String,
    blocked: Option<String>,
) -> Result<serde_json::Value, String> {
    if agent.is_empty() {
        return Err("Missing 'agent' parameter".to_string());
    }
    let project_id = open_project(&project_path)?;

    let workspace = Path::new(&project_path);
    let active_items = crate::inbox::list_folder(workspace, "active");
    let moved_file: Option<String> = match active_items.into_iter().next() {
        Some(item) => {
            // `archive` moves the item to the standard `done/` folder —
            // creating the folder on first use. Best-effort: a move
            // failure is logged via the activity-feed entry below
            // (summary = "no active task") so the caller can see the
            // session flipped to sleeping but nothing was archived.
            match crate::inbox::archive(workspace, &item.id) {
                Ok(_) => Some(item.filename),
                Err(_) => None,
            }
        }
        None => None,
    };

    let db = crate::db::shared();
    let conn = db.lock();
    let _ = WorkspaceSession::update_status(&conn, &project_id, "sleeping");

    let event_type = if blocked.is_some() {
        "task.blocked"
    } else {
        "task.done"
    };
    let summary = moved_file.as_deref().unwrap_or("no active task");
    log_activity(
        &conn,
        &project_id,
        Some(&agent),
        event_type,
        Some(&agent),
        None,
        None,
        Some(summary),
    );

    Ok(serde_json::json!({
        "success": true,
        "event": event_type,
        "file": moved_file,
    }))
}

fn reservations_path(project_path: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path).join("reservations.json")
}

fn read_reservations(
    path: &std::path::Path,
) -> serde_json::Map<String, serde_json::Value> {
    if path.exists() {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        serde_json::Map::new()
    }
}

/// Claim a comma-separated list of paths for exclusive editing.
/// Existing reservations by the same agent are idempotent; those
/// held by another agent are reported as conflicts without being
/// overwritten. `now` is taken from the local clock as ISO-8601.
pub fn reserve(
    project_path: String,
    agent: String,
    paths_str: String,
) -> Result<serde_json::Value, String> {
    if agent.is_empty() || paths_str.is_empty() {
        return Err("Missing 'agent' or 'paths' parameter".to_string());
    }
    let project_id = open_project(&project_path)?;

    let k2so_dir = crate::workspace_dot_dir(&project_path);
    fs::create_dir_all(&k2so_dir).ok();

    let path = reservations_path(&project_path);
    let mut reservations = read_reservations(&path);

    let now = chrono::Local::now().to_rfc3339();
    let paths: Vec<&str> = paths_str.split(',').map(|s| s.trim()).collect();
    let mut reserved = Vec::new();
    let mut conflicts = Vec::new();

    for p in &paths {
        if let Some(existing) = reservations.get(*p) {
            let existing_agent = existing
                .get("agent")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if existing_agent != agent {
                conflicts.push(serde_json::json!({
                    "path": p,
                    "heldBy": existing_agent,
                }));
                continue;
            }
        }
        reservations.insert(
            p.to_string(),
            serde_json::json!({
                "agent": agent,
                "reason": "",
                "timestamp": now,
            }),
        );
        reserved.push(p.to_string());
    }

    fs::write(
        &path,
        serde_json::to_string_pretty(&reservations).unwrap_or_default(),
    )
    .map_err(|e| format!("Failed to write reservations: {}", e))?;

    let db = crate::db::shared();
    let conn = db.lock();
    log_activity(
        &conn,
        &project_id,
        Some(&agent),
        "reserve",
        Some(&agent),
        None,
        None,
        Some(&format!("Reserved {} file(s)", reserved.len())),
    );

    Ok(serde_json::json!({
        "success": true,
        "reserved": reserved,
        "conflicts": conflicts,
    }))
}

/// Release reservations held by this agent. Empty `paths_str`
/// releases all; otherwise releases the comma-separated subset that
/// the agent currently holds.
pub fn release(
    project_path: String,
    agent: String,
    paths_str: String,
) -> Result<serde_json::Value, String> {
    if agent.is_empty() {
        return Err("Missing 'agent' parameter".to_string());
    }
    let project_id = open_project(&project_path)?;

    let path = reservations_path(&project_path);
    if !path.exists() {
        return Ok(serde_json::json!({"success": true, "released": 0}));
    }

    let mut reservations = read_reservations(&path);

    let specific_paths: Vec<&str> = if paths_str.is_empty() {
        vec![]
    } else {
        paths_str.split(',').map(|s| s.trim()).collect()
    };

    let keys_to_remove: Vec<String> = reservations
        .iter()
        .filter(|(key, val)| {
            let held_by = val
                .get("agent")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if held_by != agent {
                return false;
            }
            if specific_paths.is_empty() {
                return true;
            }
            specific_paths.contains(&key.as_str())
        })
        .map(|(key, _)| key.clone())
        .collect();

    let released = keys_to_remove.len();
    for key in &keys_to_remove {
        reservations.remove(key);
    }

    fs::write(
        &path,
        serde_json::to_string_pretty(&reservations).unwrap_or_default(),
    )
    .map_err(|e| format!("Failed to write reservations: {}", e))?;

    let db = crate::db::shared();
    let conn = db.lock();
    log_activity(
        &conn,
        &project_id,
        Some(&agent),
        "release",
        Some(&agent),
        None,
        None,
        Some(&format!("Released {} file(s)", released)),
    );

    Ok(serde_json::json!({
        "success": true,
        "released": released,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_tmp() -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "k2so-channel-test-{}-{}",
            std::process::id(),
            n
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn read_reservations_missing_file_returns_empty_map() {
        let tmp = unique_tmp();
        let map = read_reservations(&tmp.join("no-such-file.json"));
        assert!(map.is_empty());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_reservations_parses_existing_file() {
        let tmp = unique_tmp();
        let path = tmp.join("reservations.json");
        fs::write(
            &path,
            r#"{"src/foo.rs":{"agent":"backend","reason":"","timestamp":"2026-04-19T00:00:00-07:00"}}"#,
        )
        .unwrap();
        let map = read_reservations(&path);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("src/foo.rs"));
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Phase 2.5b regression: `done()` must move the in-flight item
    /// from `.k2so/inbox/active/` to `.k2so/inbox/done/`, NOT the
    /// retired `.k2so/agents/<name>/work/{active,done}/` tree.
    /// Reproduces the post-2.5b workspace shape where `.k2so/agents/`
    /// is absent entirely.
    #[test]
    fn done_moves_from_workspace_inbox_active_not_legacy_agents_dir() {
        let _ = crate::db::init_for_tests();
        let tmp = unique_tmp();
        let project_path = tmp.to_string_lossy().to_string();

        // Register the project row so `open_project` resolves.
        let project_id =
            format!("proj-channel-done-{}-{}", std::process::id(), TEST_COUNTER.fetch_add(1, Ordering::SeqCst));
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT OR REPLACE INTO projects \
                 (id, path, name, color, agent_mode, pinned, tab_order) \
                 VALUES (?1, ?2, ?3, '#123456', 'manager', 0, 0)",
                rusqlite::params![project_id, project_path, "test"],
            )
            .unwrap();
        }

        // Post-Phase-2.5b shape: workspace inbox only. Explicitly no
        // `.k2so/agents/` directory.
        let inbox_root = tmp.join(".k2so").join("inbox");
        fs::create_dir_all(inbox_root.join("active")).unwrap();
        fs::write(
            inbox_root.join("active").join("ship-it.md"),
            "---\ntitle: Ship It\npriority: high\n---\n\nBody.",
        )
        .unwrap();
        assert!(
            !tmp.join(".k2so").join("agents").exists(),
            "regression precondition: .k2so/agents/ must NOT exist",
        );

        let result = done(project_path.clone(), "any-agent".to_string(), None)
            .expect("done() should succeed");

        // File reported in response.
        assert_eq!(
            result.get("file").and_then(|f| f.as_str()),
            Some("ship-it.md"),
            "response should name the moved file; got: {result}",
        );

        // The item now lives in `.k2so/inbox/done/`, not in any
        // `.k2so/agents/*/work/done/` path.
        assert!(
            inbox_root.join("done").join("ship-it.md").exists(),
            "ship-it.md should have landed in workspace inbox done/",
        );
        assert!(
            !inbox_root.join("active").join("ship-it.md").exists(),
            "ship-it.md should no longer be in active/",
        );
        assert!(
            !tmp.join(".k2so").join("agents").exists(),
            "done() must NOT recreate the legacy .k2so/agents/ tree",
        );

        let _ = fs::remove_dir_all(&tmp);
    }
}
