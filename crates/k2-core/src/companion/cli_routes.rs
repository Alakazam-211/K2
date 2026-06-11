//! DB-backed sub-handlers for the `/cli/companion/*` route family.
//!
//! The mobile companion issues these to discover globally-scoped
//! state (agent presets, registered workspaces, etc.) before the
//! project-specific calls. Both the daemon and the Tauri app can
//! answer; the DB is the source of truth.
//!
//! Scope here: `presets`, `projects`. The richer `sessions` and
//! `projects-summary` endpoints require a live terminal-manager
//! handle to count running agents, which is process-local (Tauri
//! and daemon each have their own). Those stay split until the
//! daemon becomes the sole PTY owner.

/// Global list of user-enabled agent presets. Mirrors the existing
/// `/cli/companion/presets` JSON shape the companion app expects.
pub fn list_presets() -> Result<String, String> {
    let db = crate::db::shared();
    let conn = db.lock();

    let mut stmt = conn
        .prepare(
            "SELECT id, label, command, icon FROM agent_presets WHERE enabled = 1 ORDER BY sort_order ASC, label ASC",
        )
        .map_err(|e| e.to_string())?;

    let presets: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "command": row.get::<_, String>(2)?,
                "icon": row.get::<_, Option<String>>(3)?,
            }))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(serde_json::to_string(&presets).unwrap_or_else(|_| "[]".to_string()))
}

/// All registered workspaces (projects), joined with focus-group
/// metadata. Global — doesn't filter by project_path. Shape matches
/// what the existing `/cli/companion/projects` emits.
pub fn list_projects() -> Result<String, String> {
    let db = crate::db::shared();
    let conn = db.lock();

    // Skip sentinel rows (`_orphan`, `_broadcast`) — audit buckets seeded by
    // `db::seed_audit_sentinels`, not real workspaces. They'd otherwise clutter
    // the companion app's workspace drawer with non-clickable entries.
    let mut stmt = conn
        .prepare(
            "SELECT p.id, p.name, p.path, p.color, p.icon_url, p.agent_mode, p.pinned, \
             p.tab_order, p.focus_group_id, fg.name, fg.color \
             FROM projects p \
             LEFT JOIN focus_groups fg ON p.focus_group_id = fg.id \
             WHERE p.id NOT IN ('_orphan', '_broadcast') \
             ORDER BY p.pinned DESC, p.tab_order ASC, p.name ASC",
        )
        .map_err(|e| e.to_string())?;

    let projects: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            let fg_id: Option<String> = row.get(8)?;
            let fg_name: Option<String> = row.get(9)?;
            let fg_color: Option<String> = row.get(10)?;
            let focus_group = if let (Some(id), Some(name)) = (&fg_id, &fg_name) {
                serde_json::json!({ "id": id, "name": name, "color": fg_color })
            } else {
                serde_json::Value::Null
            };

            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "path": row.get::<_, String>(2)?,
                "color": row.get::<_, String>(3)?,
                "iconUrl": row.get::<_, Option<String>>(4)?,
                "agentMode": row.get::<_, String>(5)?,
                "pinned": row.get::<_, bool>(6)?,
                "tabOrder": row.get::<_, i32>(7)?,
                "focusGroup": focus_group,
            }))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(serde_json::to_string(&projects).unwrap_or_else(|_| "[]".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ensure_project(id: &str, path: &str, name: &str) {
        let db = crate::db::shared();
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
        let db = crate::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id]);
    }

    #[test]
    fn list_projects_excludes_audit_sentinels() {
        crate::db::init_for_tests();
        // init_for_tests already seeds `_orphan` + `_broadcast`. Add one
        // real workspace so the response isn't empty either way.
        ensure_project("real-ws-cli", "/tmp/k2so-cli-routes-test", "RealOne");

        let body = list_projects().expect("list_projects ok");
        let parsed: Vec<serde_json::Value> =
            serde_json::from_str(&body).expect("response is JSON array");
        let ids: Vec<&str> = parsed
            .iter()
            .filter_map(|p| p.get("id").and_then(|v| v.as_str()))
            .collect();

        assert!(
            !ids.contains(&"_orphan"),
            "/cli/companion/projects must not leak the _orphan audit sentinel: {ids:?}"
        );
        assert!(
            !ids.contains(&"_broadcast"),
            "/cli/companion/projects must not leak the _broadcast audit sentinel: {ids:?}"
        );
        assert!(
            ids.contains(&"real-ws-cli"),
            "real workspace should still appear: {ids:?}"
        );

        delete_project("real-ws-cli");
    }
}
