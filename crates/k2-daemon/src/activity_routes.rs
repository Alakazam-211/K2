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
//! Connect login.
//!
//! App passes (`k2skn_`, S8: AP3) get only the guest projection, and only
//! with `?workspace=<handle | id>` naming a room on the pass where it has
//! `activity:read` (else 403 `skin_room` / `missing capability
//! activity:read`; no `workspace=` is 400):
//! `{workspace, status, since, serverNow, sessions:[{agentName,
//! paneGroupId, status, since}]}`. The words and `since` rule are the
//! room socket's (`activity_events_ws`); the room `status` is the
//! highest-ranked display among its sessions (RL1) and `since` the
//! earliest turn start among its working ones. No path, id, reason, tool
//! or count.

use std::collections::HashMap;

use k2_core::activity::Display;
use k2_core::skin::SkinPass;

use crate::activity_events::{self, rank, WorkspaceKey};
use crate::activity_events_ws::{guest_status, room_rows, GuestSession};
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

/// The route body for an app pass (AP3). The dispatcher has already
/// refused non-GET and passes the door does not admit.
pub fn handle_snapshot_gated(params: &HashMap<String, String>, skin: Option<SkinPass>) -> CliResponse {
    let Some(pass) = skin else {
        return handle_snapshot(params);
    };
    let Some(token) = params.get("workspace").map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        return CliResponse::bad_request("missing workspace");
    };
    let room = match crate::fs_routes::resolve_skin_workspace(&pass, token) {
        Ok(r) => r,
        Err(r) => return r,
    };
    if !pass.has_cap_in_room(&room.project_id, crate::skin_routes::ACTIVITY_READ) {
        return crate::skin_routes::missing_cap_response(crate::skin_routes::ACTIVITY_READ);
    }
    let wire_workspace = if room.handle.is_empty() { token.to_string() } else { room.handle.clone() };
    let body = guest_snapshot(&wire_workspace, &room.project_id, chrono::Utc::now().timestamp_millis());
    match serde_json::to_string(&body) {
        Ok(s) => CliResponse::ok_json(s),
        Err(e) => CliResponse::internal_error(format!("serialize activity snapshot: {e}")),
    }
}

/// AP3 / §7.7: the room's guest projection from the store.
pub fn guest_snapshot(wire_workspace: &str, project_id: &str, server_now: i64) -> serde_json::Value {
    let rows = room_rows(project_id);
    let mut display = Display::Idle;
    let mut since: Option<i64> = None;
    for v in &rows {
        if rank(v.display) > rank(display) {
            display = v.display;
        }
        if v.display == Display::Working {
            if let Some(t) = v.turn_started_at {
                since = Some(since.map_or(t, |s| s.min(t)));
            }
        }
    }
    let mut sessions: Vec<GuestSession> = rows.iter().map(GuestSession::of).collect();
    sessions.sort_by(|a, b| a.agent_name.cmp(&b.agent_name));
    serde_json::json!({
        "workspace": wire_workspace,
        "status": guest_status(display),
        "since": since,
        "serverNow": server_now,
        "sessions": sessions.iter().map(GuestSession::snapshot_json).collect::<Vec<_>>(),
    })
}
