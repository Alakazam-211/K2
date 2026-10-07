//! `GET /cli/activity/snapshot[?workspace=]`
//! (prd-daemon-activity-and-thread-working-v1 S4: RL3, §7.4).
//!
//! Every activity row (§7.2), every workspace rollup (RL1), the `seq` of
//! the last `activity_changed` frame these rows are at least as new as,
//! the daemon `instanceId`, `serverNow` and `staleAfterSecs`. A client
//! pulls it when its events socket opens or re-opens, when the hello's
//! `instance_id` changes (a daemon restart), and when a frame's `seq` is
//! not `last + 1` (RL4); the snapshot replaces all its activity state for
//! this server.
//!
//! `?workspace=<id | handle | path>` narrows it to one workspace: that
//! workspace's rows and its rollup (idle with zero counts when it has no
//! live session).
//!
//! A plain authenticated read (Member floor): the owner token or a live
//! Connect login. App passes (`k2skn_`) are refused here; their guest
//! projection behind `activity:read` is S8 (AP3).

use std::collections::HashMap;

use crate::activity_events::{self, WorkspaceKey};
use crate::cli_response::CliResponse;

/// The workspace `token` names: a registered project id or handle, or a
/// path (the registered project at exactly that path, else the path
/// itself, for sessions outside any project).
fn workspace_key(token: &str) -> Result<WorkspaceKey, CliResponse> {
    let token = token.trim();
    if token.starts_with('/') {
        let path = token.trim_end_matches('/');
        let path = if path.is_empty() { "/" } else { path };
        let db = k2_core::db::shared();
        let conn = db.lock();
        let project_id = conn
            .query_row(
                "SELECT id FROM projects WHERE path = ?1 OR path = ?2",
                rusqlite::params![path, format!("{path}/")],
                |r| r.get::<_, String>(0),
            )
            .ok();
        return Ok(WorkspaceKey { project_id, workspace_path: Some(path.to_string()) });
    }
    let by_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT path FROM projects WHERE id = ?1",
            rusqlite::params![token],
            |r| r.get::<_, String>(0),
        )
        .ok()
    };
    if let Some(path) = by_id {
        return Ok(WorkspaceKey { project_id: Some(token.to_string()), workspace_path: Some(path) });
    }
    // A handle (or a uuid-shaped id the lookup above already missed).
    let ws = crate::fs_routes::resolve_owner_workspace(token)?;
    Ok(WorkspaceKey { project_id: Some(ws.project_id), workspace_path: Some(ws.path) })
}

/// The route body. The dispatcher has already refused non-GET and
/// unauthenticated callers.
pub fn handle_snapshot(params: &HashMap<String, String>) -> CliResponse {
    let key = match params.get("workspace").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(token) => match workspace_key(token) {
            Ok(k) => Some(k),
            Err(r) => return r,
        },
        None => None,
    };
    let body = activity_events::snapshot(key.as_ref());
    match serde_json::to_string(&body) {
        Ok(s) => CliResponse::ok_json(s),
        Err(e) => CliResponse::internal_error(format!("serialize activity snapshot: {e}")),
    }
}
