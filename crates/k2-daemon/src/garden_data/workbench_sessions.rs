//! The Workbench's one read of "which agent sessions does a workspace have"
//! (0.46.7). Every Workbench list (`work.list`, `GET /cli/workbench/agents`,
//! an address's sidecar) goes through this file and nothing else.
//!
//! Today it reads the sidecar rows (`k2_core::sidecar::list_rows`). Lane
//! `k2/workspace-sessions` is building one shared daemon list of a
//! workspace's sessions, live and past, for the CLI, `k2 connections` and the
//! Workbench (`.k2/prds/workspace-sessions-contract.md`). When that contract
//! lands, switch these two functions to it; nothing else in the Workbench
//! reads sessions.

use k2_core::sidecar::SidecarRow;

/// A workspace on this computer: `(project id, name, path)`.
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// Every workspace on this computer.
pub fn workspaces() -> Result<Vec<Workspace>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    Ok(k2_core::db::schema::Project::list(&conn)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|p| Workspace { id: p.id, name: p.name, path: p.path })
        .collect())
}

/// A workspace's sidecars (its main agent is not one).
pub fn sidecars(project_id: &str) -> Result<Vec<SidecarRow>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::sidecar::list_rows(&conn, project_id)
}
