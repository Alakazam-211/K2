//! workspace_sessions + workspace_relations DB accessors.
//!
//! Phase 2.5d: extracted from the monolithic `agents/commands.rs`.
//! Pre-Phase-2-Unit-7d these used `state.db.lock()` (Tauri's
//! `AppState`); post-7d they use the shared `db::shared()` handle
//! directly so the daemon serves them without the Tauri state
//! container. The function bodies are unchanged from the agents/
//! commands.rs origin.

use crate::db::schema::WorkspaceSession;

/// Fetch the workspace_sessions row for a project, if one exists.
/// Returns `None` for projects with no pinned chat session yet.
pub fn workspace_session_get(
    project_id: String,
) -> Result<Option<WorkspaceSession>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    WorkspaceSession::get(&conn, &project_id).map_err(|e| e.to_string())
}

/// List every workspace_relations row where the given project is the
/// SOURCE (this workspace "oversees" / "depends-on" the targets).
pub fn workspace_relations_list(
    project_id: String,
) -> Result<Vec<crate::db::schema::WorkspaceRelation>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    crate::db::schema::WorkspaceRelation::list_for_source(&conn, &project_id)
        .map_err(|e| e.to_string())
}

/// List every workspace_relations row where the given project is the
/// TARGET (other workspaces "oversee" this one).
pub fn workspace_relations_list_incoming(
    project_id: String,
) -> Result<Vec<crate::db::schema::WorkspaceRelation>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    crate::db::schema::WorkspaceRelation::list_for_target(&conn, &project_id)
        .map_err(|e| e.to_string())
}

/// Create a new workspace_relations row.
pub fn workspace_relations_create(
    source_project_id: String,
    target_project_id: String,
    relation_type: Option<String>,
) -> Result<crate::db::schema::WorkspaceRelation, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let rel_type = relation_type.unwrap_or_else(|| "oversees".to_string());
    crate::db::schema::WorkspaceRelation::create(
        &conn,
        &id,
        &source_project_id,
        &target_project_id,
        &rel_type,
    )
    .map_err(|e| e.to_string())?;
    Ok(crate::db::schema::WorkspaceRelation {
        id,
        source_project_id,
        target_project_id,
        relation_type: rel_type,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
    })
}

/// Delete a workspace_relations row by id.
pub fn workspace_relations_delete(id: String) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    crate::db::schema::WorkspaceRelation::delete(&conn, &id).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Phase 2 Tier 2.1 coverage for the workspace_sessions +
    //! workspace_relations wrappers. These exercise the shared in-memory
    //! test DB initialized lazily by `db::shared()` under `cfg(test)`.
    //!
    //! Each test inserts its OWN project rows with random UUIDs + unique
    //! paths so sibling tests sharing the same in-memory DB don't
    //! collide. The schema-level WorkspaceRelation CRUD already has its
    //! own coverage in `db/schema.rs`; these tests pin the wrapper
    //! contract (default relation_type, target_type lists, delete-by-
    //! id round trip, None for missing session).
    use super::*;
    use uuid::Uuid;

    fn unique_path(label: &str) -> String {
        format!(
            "/tmp/k2so-relations-test-{}-{}-{}",
            label,
            std::process::id(),
            Uuid::new_v4(),
        )
    }

    fn insert_project(path: &str) -> String {
        let db = crate::db::shared();
        let conn = db.lock();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, "relations-test", path],
        )
        .expect("insert project");
        id
    }

    #[test]
    fn workspace_session_get_returns_none_when_unset() {
        let pid = insert_project(&unique_path("session-none"));
        let got = workspace_session_get(pid).expect("query ok");
        assert!(
            got.is_none(),
            "fresh project should have no workspace_sessions row, got {got:?}",
        );
    }

    #[test]
    fn workspace_relations_create_defaults_relation_type_to_oversees() {
        let src = insert_project(&unique_path("rel-default-src"));
        let tgt = insert_project(&unique_path("rel-default-tgt"));

        // Pass None — the wrapper should default to "oversees".
        let rel = workspace_relations_create(src.clone(), tgt.clone(), None)
            .expect("create relation");
        assert_eq!(rel.relation_type, "oversees");
        assert_eq!(rel.source_project_id, src);
        assert_eq!(rel.target_project_id, tgt);
        // The returned id is the freshly minted UUID; non-empty + parseable.
        assert!(
            Uuid::parse_str(&rel.id).is_ok(),
            "wrapper should return a valid uuid, got {:?}",
            rel.id,
        );

        // Clean up.
        workspace_relations_delete(rel.id).ok();
    }

    #[test]
    fn workspace_relations_create_honors_explicit_relation_type() {
        let src = insert_project(&unique_path("rel-explicit-src"));
        let tgt = insert_project(&unique_path("rel-explicit-tgt"));

        let rel = workspace_relations_create(
            src.clone(),
            tgt.clone(),
            Some("collaborates".to_string()),
        )
        .expect("create explicit");
        assert_eq!(rel.relation_type, "collaborates");
        workspace_relations_delete(rel.id).ok();
    }

    #[test]
    fn workspace_relations_list_finds_outgoing_and_incoming() {
        let src = insert_project(&unique_path("rel-list-src"));
        let tgt = insert_project(&unique_path("rel-list-tgt"));

        let rel = workspace_relations_create(src.clone(), tgt.clone(), None)
            .expect("create");

        let outgoing = workspace_relations_list(src.clone()).expect("list source");
        assert!(
            outgoing.iter().any(|r| r.id == rel.id),
            "source-side list should surface the new row, got {outgoing:?}",
        );

        let incoming = workspace_relations_list_incoming(tgt.clone()).expect("list target");
        assert!(
            incoming.iter().any(|r| r.id == rel.id),
            "target-side list should surface the new row, got {incoming:?}",
        );

        // Outgoing for the TARGET project should NOT include the row
        // (asymmetry confirms list_for_source vs list_for_target are
        // wired to the correct columns).
        let backwards = workspace_relations_list(tgt.clone()).expect("list backwards");
        assert!(
            !backwards.iter().any(|r| r.id == rel.id),
            "target's outgoing list should not include a row where it's the target",
        );

        workspace_relations_delete(rel.id).ok();
    }

    #[test]
    fn workspace_relations_delete_removes_the_row() {
        let src = insert_project(&unique_path("rel-delete-src"));
        let tgt = insert_project(&unique_path("rel-delete-tgt"));

        let rel = workspace_relations_create(src.clone(), tgt.clone(), None)
            .expect("create");
        let rel_id = rel.id.clone();

        workspace_relations_delete(rel_id.clone()).expect("delete");

        let after = workspace_relations_list(src.clone()).expect("list");
        assert!(
            !after.iter().any(|r| r.id == rel_id),
            "delete must remove the row from list_for_source results",
        );
    }
}
