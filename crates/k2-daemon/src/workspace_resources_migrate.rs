//! One-shot backfill: pinned file-viewer tabs → `workspace_resources`.
//!
//! Reuses [`crate::project_group_routes::pinned_html_paths`] (name is a
//! lie — it is not HTML-only). INSERT OR IGNORE. Never unpins tabs.
//! Never DELETEs because a tab was unpinned.

use k2_core::log_debug;
use k2_core::workspace_resources;

use crate::project_group_routes::pinned_html_paths;

const MIGRATION_ID: &str = "workspace-resources-from-pinned-html-v1";

/// Run the one-shot backfill. Idempotent — gated by `code_migrations`.
pub fn run_once() {
    let db = k2_core::db::shared();
    let conn = db.lock();

    if k2_core::db::has_code_migration_applied(&conn, MIGRATION_ID) {
        return;
    }

    log_debug!("[daemon/boot] running {MIGRATION_ID} pass over workspace_layouts");

    let mut rows: Vec<(String, String)> = Vec::new();
    match conn.prepare("SELECT project_id, layout_json FROM workspace_layouts") {
        Ok(mut stmt) => {
            let mapped = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            });
            if let Ok(it) = mapped {
                for r in it.flatten() {
                    rows.push(r);
                }
            }
        }
        Err(e) => {
            log_debug!("[daemon/boot] {MIGRATION_ID}: SELECT failed: {e}");
            return;
        }
    }

    let mut inserted = 0usize;
    let mut scanned = 0usize;
    for (project_id, blob) in rows {
        scanned += 1;
        for file_path in pinned_html_paths(&blob) {
            match workspace_resources::insert_ignore(&conn, &project_id, &file_path) {
                Ok(()) => inserted += 1,
                Err(e) => {
                    log_debug!(
                        "[daemon/boot] {MIGRATION_ID}: insert skip project={project_id}: {e}"
                    );
                }
            }
        }
    }

    let notes = format!("layouts_scanned={scanned} insert_attempts={inserted}");
    k2_core::db::mark_code_migration_applied(&conn, MIGRATION_ID, Some(&notes));
    log_debug!("[daemon/boot] {MIGRATION_ID} complete: {notes}");
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "DELETE FROM code_migrations WHERE id = ?1",
        rusqlite::params![MIGRATION_ID],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned_tab(path: &str) -> serde_json::Value {
        serde_json::json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "isPinnedFile": true,
            "paneGroups": {
                "pg": {
                    "id": "pg",
                    "items": [{ "id": "i", "type": "file-viewer", "filePath": path }],
                    "activeItemIndex": 0
                }
            }
        })
    }

    #[test]
    fn migration_inserts_pinned_file_viewer_and_unpin_does_not_delete() {
        reset_for_tests();
        let project_id = uuid::Uuid::new_v4().to_string();
        let ws_id = uuid::Uuid::new_v4().to_string();
        let path = format!("/tmp/wsres-mig-{project_id}.csv");
        let layout = serde_json::json!({
            "version": 2,
            "tabs": [pinned_tab(&path)],
        });
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    project_id,
                    format!("mig-{project_id}"),
                    format!("/tmp/mig-{project_id}")
                ],
            )
            .expect("project");
            conn.execute(
                "INSERT INTO workspaces (id, project_id, name) VALUES (?1, ?2, 'main')",
                rusqlite::params![ws_id, project_id],
            )
            .expect("workspace");
            conn.execute(
                "INSERT INTO workspace_layouts (id, project_id, workspace_id, layout_json, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, 1000)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    project_id,
                    ws_id,
                    layout.to_string()
                ],
            )
            .expect("layout");
        }

        run_once();
        run_once(); // idempotent

        let n: i64 = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT COUNT(*) FROM workspace_resources WHERE workspace_id = ?1 AND file_path = ?2",
                rusqlite::params![project_id, path],
                |r| r.get(0),
            )
            .expect("count")
        };
        assert_eq!(n, 1, "pinned file-viewer becomes one resource row");

        // Simulated unpin: rewrite layout without the pinned tab. Migration
        // must not DELETE, and a second run (already stamped) must not either.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE workspace_layouts SET layout_json = ?1 WHERE project_id = ?2",
                rusqlite::params![r#"{"version":2,"tabs":[]}"#, project_id],
            )
            .expect("unpin layout");
        }
        run_once();
        let n2: i64 = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT COUNT(*) FROM workspace_resources WHERE workspace_id = ?1 AND file_path = ?2",
                rusqlite::params![project_id, path],
                |r| r.get(0),
            )
            .expect("count after unpin")
        };
        assert_eq!(n2, 1, "unpin does not delete the resource row");
    }

    #[test]
    fn html_docs_after_migrate_returns_docs_not_empty() {
        reset_for_tests();
        let project_id = uuid::Uuid::new_v4().to_string();
        let ws_id = uuid::Uuid::new_v4().to_string();
        let path = format!("/tmp/wsres-htmldocs-{project_id}.html");
        let group_name = format!("htmldocs-mig-{}", uuid::Uuid::new_v4());
        let layout = serde_json::json!({
            "version": 2,
            "tabs": [pinned_tab(&path)],
        });
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    project_id,
                    format!("htmldocs-{project_id}"),
                    format!("/tmp/htmldocs-{project_id}")
                ],
            )
            .expect("project");
            conn.execute(
                "INSERT INTO workspaces (id, project_id, name) VALUES (?1, ?2, 'main')",
                rusqlite::params![ws_id, project_id],
            )
            .expect("workspace");
            conn.execute(
                "INSERT INTO workspace_layouts (id, project_id, workspace_id, layout_json, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, 1000)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    project_id,
                    ws_id,
                    layout.to_string()
                ],
            )
            .expect("layout");
        }

        let created = crate::project_group_routes::dispatch_post(
            "/cli/project-group/create",
            serde_json::json!({ "name": group_name }).to_string().as_bytes(),
            "owner",
        );
        assert_eq!(created.status, "200 OK", "body={}", created.body);
        let g: serde_json::Value = serde_json::from_str(&created.body).expect("json");
        let gid = g["id"].as_str().expect("id");
        let added = crate::project_group_routes::dispatch_post(
            "/cli/project-group/add-member",
            serde_json::json!({ "group": gid, "workspace": project_id })
                .to_string()
                .as_bytes(),
            "owner",
        );
        assert_eq!(added.status, "200 OK", "body={}", added.body);

        run_once();

        let mut params = std::collections::HashMap::new();
        params.insert("group".to_string(), gid.to_string());
        let resp = crate::project_group_routes::dispatch("/cli/project-group/html-docs", &params)
            .expect("html-docs claimed");
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        let docs = v["docs"].as_array().expect("docs");
        assert!(
            !docs.is_empty(),
            "html-docs after migrate must not be []: {v}"
        );
        assert_eq!(docs[0]["filePath"], path.as_str());
        assert_eq!(docs[0]["workspaceId"], project_id.as_str());
    }
}
