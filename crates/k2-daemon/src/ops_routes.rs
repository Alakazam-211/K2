//! `/cli/ops/*` — read-only Observability / Agent-Ops aggregate API.
//!
//! Implements Phase A + B of `.k2/prds/prd-observability-agent-ops.md`: a
//! thin read/aggregate plane over the **existing** capture planes. It
//! **persists nothing of its own** and adds no capture engine — it reads:
//!
//!   * Phase A — `GET /cli/ops/activity` over the persistent `activity_feed`
//!     SQLite table (the one durable audit surface that had no HTTP route).
//!   * Phase B — `GET /cli/ops/overview` over the live `v2_session_map` +
//!     the canonical Active set (`compute_active_project_ids`) + the cached
//!     `AgentStatusChanged` state — one snapshot of every live agent.
//!
//! Both routes are **read-only GET**. They reach this module through the
//! unified `/cli/*` dispatch (`cli::dispatch`), which gates every request on
//! `token_ok` (owner OR connect-user session) BEFORE this code runs — same
//! gate as every other `/cli/...` read route (e.g. `/cli/projects/active`,
//! `/cli/terminal/active-count`). There is no mutation surface here, so
//! there is nothing to 405-guard.
//!
//! Phases C (WS multiplex), D (renderer), and E (`/boot-status` capability)
//! are out of this slice.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::cli_response::CliResponse;

/// Sane default + hard cap on `?limit=` for `/cli/ops/activity` so a client
/// can't ask the daemon to serialize an unbounded history slice.
const ACTIVITY_DEFAULT_LIMIT: i64 = 100;
const ACTIVITY_MAX_LIMIT: i64 = 1000;

/// Domain dispatch for `/cli/ops/*`. Returns `None` for non-ops paths so
/// `cli::dispatch` can fall through to the next domain. Mirrors the
/// `agents_routes` / `misc_routes` `dispatch(path, params) -> Option<…>`
/// convention.
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    match path {
        // OBS-SEAM: when the §4 forensic ledger lands, add ?session=<sid>
        // served from the ledger (same endpoint, additive param) — this is
        // the ledger team's read surface; do not mint a parallel one.
        "/cli/ops/activity" => Some(handle_activity(params)),
        "/cli/ops/overview" => Some(handle_overview()),
        _ => None,
    }
}

/// `GET /cli/ops/activity?project=<id>&since=<ts>&limit=<n>&actor=<name>`.
///
/// Read-only over the persistent `activity_feed` (`ActivityFeedEntry`). The
/// **persistent recent-history** surface — survives daemon restart. With
/// `actor` set, filters via `list_by_actor` (actor matches `actor` /
/// `from_workspace` / `to_workspace`); otherwise `list_by_project`. `since`
/// (unix seconds, inclusive) and `limit` (default 100, hard max 1000) are
/// honored. Returns a JSON array of the rows, newest first.
fn handle_activity(params: &HashMap<String, String>) -> CliResponse {
    let project = match params.get("project") {
        Some(p) if !p.is_empty() => p.as_str(),
        _ => return CliResponse::bad_request("missing 'project' query param"),
    };

    // limit: default 100, clamped to [1, 1000]. A malformed value is a
    // client error, not a silent default — fail loud.
    let limit = match params.get("limit") {
        None => ACTIVITY_DEFAULT_LIMIT,
        Some(raw) => match raw.parse::<i64>() {
            Ok(n) if n >= 1 => n.min(ACTIVITY_MAX_LIMIT),
            _ => return CliResponse::bad_request("'limit' must be a positive integer"),
        },
    };

    // since: optional unix-seconds lower bound (inclusive). Malformed → 400.
    let since = match params.get("since") {
        None => None,
        Some(raw) => match raw.parse::<i64>() {
            Ok(ts) => Some(ts),
            Err(_) => return CliResponse::bad_request("'since' must be a unix timestamp"),
        },
    };

    let actor = params.get("actor").map(String::as_str).filter(|s| !s.is_empty());

    let db = k2_core::db::shared();
    let conn = db.lock();
    let rows = match actor {
        Some(a) => k2_core::db::schema::ActivityFeedEntry::list_by_actor(&conn, project, a, limit),
        None => k2_core::db::schema::ActivityFeedEntry::list_by_project(&conn, project, limit, 0),
    };
    drop(conn);

    let rows = match rows {
        Ok(r) => r,
        Err(e) => return CliResponse::internal_error(format!("activity_feed query: {e}")),
    };

    // `since` is applied after the query (the existing core queries take no
    // lower bound). Rows are already newest-first; keep that order.
    let filtered: Vec<_> = match since {
        Some(ts) => rows.into_iter().filter(|r| r.created_at >= ts).collect(),
        None => rows,
    };

    match serde_json::to_string(&filtered) {
        Ok(json) => CliResponse::ok_json(json),
        Err(e) => CliResponse::internal_error(format!("serialize activity: {e}")),
    }
}

/// One live agent in the `/cli/ops/overview` snapshot.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OverviewSession {
    /// Daemon-side session/terminal id (mirrors `/cli/sessions/events`'s
    /// `session_id` and the `paneId` agent-status is keyed on).
    session_id: String,
    /// Absolute cwd of the session's PTY (empty when unknown).
    workspace_path: String,
    /// Canonical `v2_session_map` key — the agent's address.
    agent_address: String,
    /// Whether this session's project is in the canonical Active set
    /// (derived from `compute_active_project_ids`, the SAME source that
    /// produces the `ActiveChanged` event).
    active: bool,
    /// Normalized status: `working` (start) | `idle` (stop) | `permission`,
    /// or `null` when no `AgentStatusChanged` has been observed for this
    /// session since boot. Derived from the cached `AgentStatusChanged` —
    /// the SAME source `/cli/sessions/events` carries.
    agent_status: Option<String>,
    /// `live` when this session's PTY is a heartbeat's active terminal,
    /// else `null` (derived from `workspace_heartbeats.active_terminal_id`,
    /// the same truth `HeartbeatStateChanged` carries).
    heartbeat_state: Option<String>,
    /// Unix seconds of the last observed `AgentStatusChanged` for this
    /// session, or `null` if none seen.
    last_activity_at: Option<i64>,
}

/// Map the raw `AgentStatusChanged` bucket to the working|idle vocabulary
/// the overview exposes. Pure + deterministic, so it can't introduce
/// divergence from the cached value.
fn normalize_status(raw: &str) -> String {
    match raw {
        "start" => "working".to_string(),
        "stop" => "idle".to_string(),
        other => other.to_string(),
    }
}

fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `GET /cli/ops/overview` — one JSON snapshot of every live session on this
/// daemon, for a pane's initial render. Built by reading the live
/// `v2_session_map` and tagging each session with status derived from the
/// SAME sources `/cli/sessions/events` uses (Active set + cached
/// `AgentStatusChanged`), so the pane and the live event stream never
/// disagree.
fn handle_overview() -> CliResponse {
    // `active` — canonical Active set, the SAME function the ActiveChanged
    // broadcast (`active_reaper::recompute_and_broadcast_active`) calls.
    let window = k2_core::app_settings::load().active_window_hours;
    let active_ids: HashSet<String> =
        match k2_core::projects_ops::compute_active_project_ids(unix_now_ms(), window) {
            Ok(ids) => ids.into_iter().collect(),
            Err(e) => return CliResponse::internal_error(format!("compute active set: {e}")),
        };

    let sessions = crate::v2_session_map::snapshot();

    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut out: Vec<OverviewSession> = Vec::with_capacity(sessions.len());
    for (agent_address, session) in sessions {
        let session_id = session.session_id.to_string();
        let workspace_path = session
            .cwd
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        // active: is this session's project in the canonical Active set?
        let active = if workspace_path.is_empty() {
            false
        } else {
            k2_core::workspace::agent_identity::resolve_project_id(&conn, &workspace_path)
                .map(|pid| active_ids.contains(&pid))
                .unwrap_or(false)
        };

        // agent_status: latest cached AgentStatusChanged for this session.
        let cached = crate::session_events::agent_status_for(&session_id);
        let agent_status = cached.as_ref().map(|(raw, _)| normalize_status(raw));
        let last_activity_at = cached.as_ref().map(|(_, ts)| *ts);

        // heartbeat_state: live iff this PTY backs a heartbeat's active
        // terminal (same column HeartbeatStateChanged is derived from).
        let heartbeat_state = match k2_core::db::schema::AgentHeartbeat::find_by_active_terminal(
            &conn,
            &session_id,
        ) {
            Ok(rows) if !rows.is_empty() => Some("live".to_string()),
            _ => None,
        };

        out.push(OverviewSession {
            session_id,
            workspace_path,
            agent_address,
            active,
            agent_status,
            heartbeat_state,
            last_activity_at,
        });
    }
    drop(conn);

    match serde_json::to_string(&out) {
        Ok(json) => CliResponse::ok_json(json),
        Err(e) => CliResponse::internal_error(format!("serialize overview: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_status_maps_buckets_to_working_idle() {
        assert_eq!(normalize_status("start"), "working");
        assert_eq!(normalize_status("stop"), "idle");
        // Unknown/other buckets pass through verbatim (e.g. permission).
        assert_eq!(normalize_status("permission"), "permission");
        assert_eq!(normalize_status("whatever"), "whatever");
    }

    #[test]
    fn activity_requires_project_param() {
        let params = HashMap::new();
        let r = handle_activity(&params);
        assert_eq!(r.status, "400 Bad Request", "missing project must 400");
    }

    #[test]
    fn activity_rejects_malformed_limit_and_since() {
        let mut params = HashMap::new();
        params.insert("project".to_string(), "p".to_string());
        params.insert("limit".to_string(), "-3".to_string());
        assert_eq!(
            handle_activity(&params).status,
            "400 Bad Request",
            "negative limit must 400, not silently default"
        );

        let mut params = HashMap::new();
        params.insert("project".to_string(), "p".to_string());
        params.insert("since".to_string(), "notanumber".to_string());
        assert_eq!(
            handle_activity(&params).status,
            "400 Bad Request",
            "non-numeric since must 400"
        );
    }

    #[test]
    fn dispatch_returns_none_for_foreign_paths() {
        let params = HashMap::new();
        assert!(dispatch("/cli/projects/active", &params).is_none());
        assert!(dispatch("/cli/ops/overview", &params).is_some());
    }
}
