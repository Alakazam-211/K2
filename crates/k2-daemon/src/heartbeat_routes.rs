//! Consolidated daemon-owned `/cli/heartbeat/*` route handlers.
//!
//! Pre-0.39.0 the heartbeat HTTP surface was split across two files
//! (`main.rs` carried the CRUD dispatcher + the heartbeat-log endpoint,
//! `heartbeat_launchd_routes.rs` carried the launchd-install handlers)
//! plus a chunk of POST routes that landed inline in `handle_connection`.
//! That split made it hard to see what the daemon actually exposes for
//! heartbeats. 0.39.0 collapses every heartbeat HTTP entry point into
//! this one module so the surface area is auditable in one place.
//!
//! ## Read-side (GET, dispatched via `cli::dispatch`)
//!
//! - `/cli/heartbeat/add` — schedule a new heartbeat (query string).
//! - `/cli/heartbeat/list` / `/cli/heartbeat/list-archived` — list rows.
//! - `/cli/heartbeat/archive` / `/cli/heartbeat/unarchive` — soft delete.
//! - `/cli/heartbeat/remove` — hard delete.
//! - `/cli/heartbeat/enable` / `/cli/heartbeat/set-use-workspace-session`
//!   — flip per-row flags.
//! - `/cli/heartbeat/set-session` — pick the delivery session
//!   (pinned chat / own-auto / a specific saved provider session).
//! - `/cli/heartbeat/edit` / `/cli/heartbeat/rename` — mutate the row.
//! - `/cli/heartbeat/status` — last-N fires for one schedule.
//! - `/cli/heartbeat/fires-list` — recent fires across the project.
//! - `/cli/heartbeat/active-session` — what live session this heartbeat
//!   is currently bound to (used by the chat tab + companion).
//! - `/cli/heartbeat/fire` and `/cli/heartbeat/launch` — manual single
//!   fire (does NOT consult the schedule window).
//! - `/cli/heartbeat-log` — recent fires across every schedule.
//!
//! ## Write-side (POST, dispatched via the routes dispatcher)
//!
//! - `/cli/heartbeat/install-launchd` — install the macOS launchd plist
//!   (or Linux crontab entry) that drives the heartbeat tick.
//! - `/cli/heartbeat/uninstall-launchd` — undo the above.
//! - `/cli/heartbeat/apply-wake-scheduler` — re-apply the user's wake
//!   scheduler settings (off / on_demand / heartbeat).
//!
//! The actual install/uninstall logic lives in
//! `k2_core::heartbeats::install` so headless callers (CLI, daemon
//! boot, future CLI verb) share one implementation. The dispatcher in
//! `routes::dispatcher` provides the POST method gate before this
//! module sees the call.

use std::collections::HashMap;

use serde::Deserialize;

use k2_core::heartbeats as hb;
use k2_core::heartbeats::install as heartbeat_install;

use crate::cli_response::CliResponse;

// ──────────────────────────────────────────────────────────────────────
// POST: launchd installer / wake-scheduler routes
// ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct InstallLaunchdBody {
    /// Seconds between heartbeat fires. Defaults to 60 if missing.
    interval_seconds: Option<u32>,
    /// Whether the plist should set `WakeSystem=true`. Defaults to false.
    wake_system: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ApplyWakeSchedulerBody {
    /// One of "off", "on_demand", "heartbeat".
    mode: Option<String>,
    /// Minutes between fires (in "heartbeat" mode). Defaults to 5.
    interval_minutes: Option<u32>,
    /// WakeSystem flag (in "heartbeat" mode). Defaults to false.
    wake_system: Option<bool>,
}

/// Handler for `POST /cli/heartbeat/install-launchd`.
///
/// Writes `~/.k2so/heartbeat.sh` + installs the launchd plist (macOS)
/// or crontab entry (Linux). Idempotent.
pub fn handle_install_launchd(body: &[u8]) -> CliResponse {
    let parsed: InstallLaunchdBody = if body.is_empty() {
        InstallLaunchdBody::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(b) => b,
            Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
        }
    };
    let interval_seconds = parsed.interval_seconds.unwrap_or(60).max(60);
    let wake_system = parsed.wake_system.unwrap_or(false);

    match heartbeat_install::install_heartbeat_scheduler(interval_seconds, wake_system) {
        Ok(msg) => CliResponse::ok_json(
            serde_json::json!({ "success": true, "message": msg }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/heartbeat/uninstall-launchd`.
///
/// Unloads + deletes the plist (macOS) or strips the crontab entry
/// (Linux). Idempotent — success on a host that never had the
/// scheduler installed.
pub fn handle_uninstall_launchd(_body: &[u8]) -> CliResponse {
    match heartbeat_install::uninstall_heartbeat_scheduler() {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/heartbeat/apply-wake-scheduler`.
///
/// Reads the user's Wake Scheduler settings and either installs or
/// uninstalls the heartbeat plist accordingly. Mode "off" /
/// "on_demand" uninstall, "heartbeat" installs with the user's
/// interval + wake_system.
pub fn handle_apply_wake_scheduler(body: &[u8]) -> CliResponse {
    let parsed: ApplyWakeSchedulerBody = if body.is_empty() {
        ApplyWakeSchedulerBody::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(b) => b,
            Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
        }
    };
    let mode = parsed.mode.unwrap_or_else(|| "off".to_string());
    let interval_minutes = parsed.interval_minutes.unwrap_or(5).max(1);
    let wake_system = parsed.wake_system.unwrap_or(false);

    match heartbeat_install::apply_wake_scheduler(&mode, interval_minutes, wake_system) {
        Ok(msg) => CliResponse::ok_json(
            serde_json::json!({ "success": true, "message": msg }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

// ──────────────────────────────────────────────────────────────────────
// POST: workspace heartbeat-sessions visibility flag (K2 Connect GAP)
// ──────────────────────────────────────────────────────────────────────
//
// The renderer previously flipped this via the LOCAL
// `k2so_workspace_set_show_heartbeat_sessions` Tauri command — which
// misfires when driving a REMOTE host (K2 Connect). This route wraps the
// SAME core fn so the write always lands on the daemon the renderer is
// talking to. Workspace-scoped (a `project_path` in the body), NOT
// owner-only. The dispatcher provides the POST method gate + token gate.

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct SetShowHeartbeatSessionsBody {
    project_path: String,
    enabled: bool,
}

/// Handler for `POST /cli/heartbeat/set-show-sessions`.
///
/// Wraps `k2_core::heartbeats::k2so_workspace_set_show_heartbeat_sessions`.
/// Mirrors the `k2so_workspace_set_show_heartbeat_sessions` Tauri command.
pub fn handle_set_show_heartbeat_sessions(body: &[u8]) -> CliResponse {
    let parsed: SetShowHeartbeatSessionsBody = if body.is_empty() {
        SetShowHeartbeatSessionsBody::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(b) => b,
            Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
        }
    };
    if parsed.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    match hb::k2so_workspace_set_show_heartbeat_sessions(parsed.project_path, parsed.enabled) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[cfg(test)]
mod gap_route_tests {
    use super::*;

    #[test]
    fn set_show_heartbeat_sessions_rejects_missing_project_path() {
        let r = handle_set_show_heartbeat_sessions(b"{}");
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("project_path"), "body={}", r.body);
    }

    #[test]
    fn set_show_heartbeat_sessions_rejects_garbage_body() {
        let r = handle_set_show_heartbeat_sessions(b"not json");
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("invalid body"), "body={}", r.body);
    }
}

#[cfg(test)]
mod fire_disabled_gate_tests {
    //! GH#27 — `/cli/heartbeat/fire` refuses a manual fire on a
    //! DISABLED (enabled=0, non-archived) heartbeat unless `force=1`.
    //! The `Err(..)` return surfaces through `CliResponse::bad_request`
    //! as exactly `{"error":"heartbeat '<name>' is disabled — enable it
    //! or pass --force"}` — the CLI contract a parallel agent codes
    //! against. Archived rows keep the `skipped_archived` shape.

    use super::*;
    use k2_core::db::schema::AgentHeartbeat;

    /// Seed a unique project + one heartbeat row. Returns project_path.
    fn seed(label: &str, enabled: bool, archived: bool) -> String {
        k2_core::db::init_for_tests();
        let path = std::env::temp_dir()
            .join(format!(
                "k2-gh27-fire-{label}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ))
            .to_string_lossy()
            .into_owned();
        let project_id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, 'gh27-fire', ?2)",
            rusqlite::params![project_id, path],
        )
        .unwrap();
        AgentHeartbeat::insert(
            &conn,
            &uuid::Uuid::new_v4().to_string(),
            &project_id,
            "hb",
            "daily",
            "{}",
            ".k2/heartbeats/hb/WAKEUP.md",
            enabled,
        )
        .unwrap();
        if archived {
            AgentHeartbeat::archive(&conn, &project_id, "hb").unwrap();
        }
        path
    }

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn fire_refuses_disabled_heartbeat_without_force() {
        let path = seed("disabled", false, false);
        let err = dispatch_get("/cli/heartbeat/fire", &path, &params(&[("name", "hb")]))
            .expect_err("disabled heartbeat must refuse a manual fire");
        // Exact wire contract — bad_request wraps this verbatim into
        // {"error": "..."}.
        assert_eq!(err, "heartbeat 'hb' is disabled — enable it or pass --force");
    }

    #[test]
    fn fire_force_1_bypasses_the_disabled_gate() {
        let path = seed("forced", false, false);
        let result = dispatch_get(
            "/cli/heartbeat/fire",
            &path,
            &params(&[("name", "hb"), ("force", "1")]),
        );
        // force=1 reaches smart_launch. This scratch workspace has no
        // scheduleable agent, so the launch itself reports that — the
        // point is the disabled refusal is gone.
        let body = result.expect("force=1 must not be refused by the disabled gate");
        assert!(
            !body.contains("is disabled"),
            "force=1 must bypass the disabled refusal, got: {body}"
        );
    }

    #[test]
    fn fire_archived_row_keeps_skipped_archived_shape() {
        // Archived + disabled: the archive check (in smart_launch) owns
        // this case — NOT the disabled refusal.
        let path = seed("archived", false, true);
        let body = dispatch_get("/cli/heartbeat/fire", &path, &params(&[("name", "hb")]))
            .expect("archived rows return the existing skipped_archived JSON, not a 400");
        assert!(
            body.contains("skipped_archived"),
            "archived behavior must be unchanged, got: {body}"
        );
    }

    #[test]
    fn fire_enabled_row_is_unaffected_by_the_gate() {
        let path = seed("enabled", true, false);
        let result =
            dispatch_get("/cli/heartbeat/fire", &path, &params(&[("name", "hb")]));
        let body = result.expect("enabled heartbeat must pass the gate without force");
        assert!(!body.contains("is disabled"), "got: {body}");
    }
}

// ──────────────────────────────────────────────────────────────────────
// GET: heartbeat CRUD / fires / active-session
// ──────────────────────────────────────────────────────────────────────

/// Heartbeat-drawer live-update fix — wrap a CRUD arm's result: on
/// success, broadcast `heartbeat_roster_changed` so every subscribed
/// client (sidebar drawer, Settings) re-fetches its heartbeat list.
/// Errors pass through untouched (nothing changed, nothing to announce).
/// Fire/launch and the read routes must NOT come through here — live
/// (PTY) flips already broadcast via `emit_heartbeat_live`, and reads
/// don't mutate the roster.
fn with_roster_broadcast(
    project_path: &str,
    result: Result<String, String>,
) -> Result<String, String> {
    if result.is_ok() {
        // Resolve the project_id the same way the active-session emit
        // does. A miss shouldn't happen right after a successful
        // mutation; if it does, emit with an empty projectId anyway —
        // the WS path filter still routes the nudge on workspacePath.
        let project_id = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
                .unwrap_or_default()
        };
        crate::session_events::emit_heartbeat_roster_changed(project_path, &project_id);
    }
    result
}

/// Dispatch an authenticated `/cli/heartbeat/*` request to the matching
/// core function. Returns the JSON response body on success or an error
/// message the caller turns into a 400.
///
/// Mirrors the dispatch shape in src-tauri's agent_hooks server (pre-H7)
/// so the CLI sees identical responses regardless of which process is
/// listening. Unrecognized sub-paths return an error string the caller
/// translates into a 400.
pub fn dispatch_get(
    path: &str,
    project_path: &str,
    params: &HashMap<String, String>,
) -> Result<String, String> {
    match path {
        "/cli/heartbeat/add" => {
            let name = params.get("name").cloned().unwrap_or_default();
            let frequency = params.get("frequency").cloned().unwrap_or_default();
            let spec_json = params
                .get("spec")
                .cloned()
                .unwrap_or_else(|| "{}".to_string());
            if name.is_empty() || frequency.is_empty() {
                return Err("Missing 'name' or 'frequency' parameter".to_string());
            }
            with_roster_broadcast(
                project_path,
                hb::k2so_heartbeat_add(project_path.to_string(), name, frequency, spec_json)
                    .map(|v| v.to_string()),
            )
        }
        "/cli/heartbeat/list" => hb::k2so_heartbeat_list(project_path.to_string())
            .map(|rows| serde_json::to_string(&rows).unwrap_or_default()),
        "/cli/heartbeat/list-archived" => {
            hb::k2so_heartbeat_list_archived(project_path.to_string())
                .map(|rows| serde_json::to_string(&rows).unwrap_or_default())
        }
        "/cli/heartbeat/archive" => {
            let name = params.get("name").cloned().unwrap_or_default();
            if name.is_empty() {
                return Err("Missing 'name' parameter".to_string());
            }
            with_roster_broadcast(
                project_path,
                hb::k2so_heartbeat_archive(project_path.to_string(), name)
                    .map(|_| r#"{"success":true}"#.to_string()),
            )
        }
        "/cli/heartbeat/unarchive" => {
            let name = params.get("name").cloned().unwrap_or_default();
            if name.is_empty() {
                return Err("Missing 'name' parameter".to_string());
            }
            with_roster_broadcast(
                project_path,
                hb::k2so_heartbeat_unarchive(project_path.to_string(), name)
                    .map(|_| r#"{"success":true}"#.to_string()),
            )
        }
        "/cli/heartbeat/fire" | "/cli/heartbeat/launch" => {
            // Manual single-heartbeat launch — does NOT consult schedule
            // window. Routes through the smart-launch decision tree
            // (fresh-fire / inject-into-live / resume-and-fire) so the
            // CLI, the Tauri Launch button, and the cron tick all share
            // one canonical path. `fire` kept as an alias since the
            // existing CLI verb predates `launch`.
            //
            // GH#27: a DISABLED (enabled=0, non-archived) heartbeat
            // refuses a manual fire unless `force=1` is passed — the
            // CLI's `--force` flag. Archived rows are untouched by this
            // gate; they keep the existing `skipped_archived` response
            // from smart_launch. The scheduler tick never reaches this
            // route (it iterates `list_enabled` directly), so this gate
            // only affects manual fires.
            let name = params.get("name").cloned().unwrap_or_default();
            let force = params
                .get("force")
                .map(|v| v == "1" || v == "true")
                .unwrap_or(false);
            if !force && !name.is_empty() {
                let disabled = {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
                        .and_then(|pid| {
                            k2_core::db::schema::AgentHeartbeat::get_by_name(&conn, &pid, &name)
                                .ok()
                                .flatten()
                        })
                        .map(|hb| !hb.enabled && hb.archived_at.is_none())
                        .unwrap_or(false)
                };
                if disabled {
                    return Err(format!(
                        "heartbeat '{name}' is disabled — enable it or pass --force"
                    ));
                }
            }
            Ok(crate::heartbeat_launch::smart_launch(project_path, &name).to_string())
        }
        "/cli/heartbeat/remove" => {
            let name = params.get("name").cloned().unwrap_or_default();
            if name.is_empty() {
                return Err("Missing 'name' parameter".to_string());
            }
            with_roster_broadcast(
                project_path,
                hb::k2so_heartbeat_remove(project_path.to_string(), name)
                    .map(|_| r#"{"success":true}"#.to_string()),
            )
        }
        "/cli/heartbeat/enable" => {
            let name = params.get("name").cloned().unwrap_or_default();
            let enabled = params
                .get("enabled")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true);
            if name.is_empty() {
                return Err("Missing 'name' parameter".to_string());
            }
            with_roster_broadcast(
                project_path,
                hb::k2so_heartbeat_set_enabled(project_path.to_string(), name, enabled)
                    .map(|_| r#"{"success":true}"#.to_string()),
            )
        }
        "/cli/heartbeat/set-use-workspace-session" => {
            // 0.37.8 — flip the per-heartbeat opt-in to deliver
            // WAKEUP.md into the workspace's pinned chat session
            // instead of the heartbeat's own saved session.
            let name = params.get("name").cloned().unwrap_or_default();
            let enabled = params
                .get("enabled")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false);
            if name.is_empty() {
                return Err("Missing 'name' parameter".to_string());
            }
            with_roster_broadcast(
                project_path,
                hb::k2so_heartbeat_set_use_workspace_session(
                    project_path.to_string(),
                    name,
                    enabled,
                )
                .map(|_| r#"{"success":true}"#.to_string()),
            )
        }
        "/cli/heartbeat/set-session" => {
            // 0073 — user-selectable delivery session. Modes:
            // `pinned` (workspace pinned chat), `auto` (own session,
            // new on next fire), `session` (a specific saved session;
            // requires `session_id` + `provider`, validated in core:
            // known provider, not the pinned chat session, exists on
            // disk). Success echoes the applied state:
            // {"success":true,"mode":...[,"sessionId","provider"]}.
            let name = params.get("name").cloned().unwrap_or_default();
            let mode = params.get("mode").cloned().unwrap_or_default();
            let session_id = params
                .get("session_id")
                .cloned()
                .filter(|s| !s.is_empty());
            let provider = params
                .get("provider")
                .cloned()
                .filter(|s| !s.is_empty());
            if name.is_empty() || mode.is_empty() {
                return Err("Missing 'name' or 'mode' parameter".to_string());
            }
            with_roster_broadcast(
                project_path,
                hb::k2so_heartbeat_set_session(
                    project_path.to_string(),
                    name,
                    mode,
                    session_id,
                    provider,
                )
                .map(|v| v.to_string()),
            )
        }
        "/cli/heartbeat/edit" => {
            let name = params.get("name").cloned().unwrap_or_default();
            let frequency = params.get("frequency").cloned().unwrap_or_default();
            let spec_json = params.get("spec").cloned().unwrap_or_default();
            if name.is_empty() || frequency.is_empty() {
                return Err("Missing 'name' or 'frequency' parameter".to_string());
            }
            with_roster_broadcast(
                project_path,
                hb::k2so_heartbeat_edit(project_path.to_string(), name, frequency, spec_json)
                    .map(|_| r#"{"success":true}"#.to_string()),
            )
        }
        "/cli/heartbeat/rename" => {
            let old_name = params.get("from").cloned().unwrap_or_default();
            let new_name = params.get("to").cloned().unwrap_or_default();
            if old_name.is_empty() || new_name.is_empty() {
                return Err("Missing 'from' or 'to' parameter".to_string());
            }
            with_roster_broadcast(
                project_path,
                hb::k2so_heartbeat_rename(project_path.to_string(), old_name, new_name)
                    .map(|_| r#"{"success":true}"#.to_string()),
            )
        }
        "/cli/heartbeat/status" => {
            // Last N fires for a specific schedule name.
            let name = params.get("name").cloned().unwrap_or_default();
            let limit = params
                .get("limit")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(10)
                .clamp(1, 200);
            if name.is_empty() {
                return Err("Missing 'name' parameter".to_string());
            }
            let db = k2_core::db::shared();
            let conn = db.lock();
            let project_id = k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
                .ok_or_else(|| format!("Project not found: {project_path}"))?;
            k2_core::db::schema::HeartbeatFire::list_by_schedule_name(
                &conn,
                &project_id,
                &name,
                limit,
            )
            .map(|rows| serde_json::to_string(&rows).unwrap_or_default())
            .map_err(|e| e.to_string())
        }
        "/cli/heartbeat/fires-list" => {
            // Recent fires for the whole project. Powers the Settings
            // History panel. Migrated alongside the rest of the
            // heartbeat CRUD so the daemon serves the same surface
            // src-tauri did.
            let limit = params
                .get("limit")
                .and_then(|s| s.parse::<i64>().ok());
            hb::k2so_heartbeat_fires_list(project_path.to_string(), limit)
                .map(|rows| serde_json::to_string(&rows).unwrap_or_default())
        }
        "/cli/heartbeat/active-session" => {
            // 0036 — heartbeat-active-session lookup. Reads the row's
            // `active_terminal_id` and verifies via session_lookup
            // (covers both legacy session_map and v2_session_map).
            // Returns the agent_name as well so the renderer can pass
            // it to TerminalPane's `attachAgentName` override and
            // /cli/sessions/v2/spawn returns the existing session
            // (reused=true) instead of spawning a duplicate. See
            // `.k2so/prds/heartbeat-active-session-tracking.md`.
            let name = params.get("name").cloned().unwrap_or_default();
            if name.is_empty() {
                return Err("Missing 'name' parameter".to_string());
            }
            let db = k2_core::db::shared();
            let conn = db.lock();
            let project_id = k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
                .ok_or_else(|| format!("Project not found: {project_path}"))?;
            let hb_row =
                k2_core::db::schema::AgentHeartbeat::get_by_name(&conn, &project_id, &name)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("no heartbeat '{name}'"))?;
            let mut active_id = hb_row.active_terminal_id.clone();
            // Walk both legacy + v2 session maps so we accept any
            // running PTY (heartbeat fresh-fires today land in v2;
            // legacy chat tabs may still be in session_map).
            let (mut active_agent_name, mut session_alive, mut is_v2) =
                match active_id.as_deref() {
                    Some(tid) => match k2_core::session::SessionId::parse(tid) {
                        Some(sid) => {
                            let snap = crate::session_lookup::snapshot_all();
                            let found = snap
                                .iter()
                                .find(|(_n, live)| live.session_id() == sid);
                            match found {
                                Some((nm, live)) => {
                                    (Some(nm.clone()), true, live.is_v2())
                                }
                                None => (None, false, false),
                            }
                        }
                        None => (None, false, false),
                    },
                    None => (None, false, false),
                };
            // Lazy cleanup so the next call reflects reality.
            if active_id.is_some() && !session_alive {
                let _ = k2_core::db::schema::AgentHeartbeat::clear_active_terminal_id(
                    &conn, &project_id, &name,
                );
                active_id = None;
            }
            // Fallback: stamp was null or pointed at a corpse. Scan
            // argv for any live PTY running `--resume <last_session_id>`
            // and surface it. Avoids the duplicate-claude-process
            // problem where clicking a heartbeat row spawns yet
            // another `claude --resume` against an already-running
            // session. When found, stamp the row so subsequent calls
            // go straight through the fast path above.
            if !session_alive {
                if let Some(saved) = hb_row
                    .last_session_id
                    .as_deref()
                    .filter(|s| !s.is_empty())
                {
                    let snap = crate::session_lookup::snapshot_all();
                    // Prefer `tab-*` agent names (visible UI tabs) over
                    // daemon-internal agent names. Same ranking
                    // `find_live_for_resume` uses.
                    let mut matches: Vec<&(String, crate::session_lookup::LiveSession)> =
                        snap.iter()
                            .filter(|(_n, live)| {
                                let args = live.args();
                                let mut i = 0;
                                while i + 1 < args.len() {
                                    if (args[i] == "--session-id"
                                        || args[i] == "--resume")
                                        && args[i + 1] == saved
                                    {
                                        return true;
                                    }
                                    i += 1;
                                }
                                false
                            })
                            .collect();
                    matches.sort_by_key(|(n, _)| if n.starts_with("tab-") { 0 } else { 1 });
                    if let Some((nm, live)) = matches.first() {
                        let new_tid = live.session_id().to_string();
                        let _ = k2_core::db::schema::AgentHeartbeat::save_active_terminal_id(
                            &conn, &project_id, &name, &new_tid,
                        );
                        // #677.1 — reconcile re-bound this heartbeat to a
                        // live PTY; broadcast the live flip.
                        crate::session_events::emit_heartbeat_live(
                            "", &project_id, &name, true,
                        );
                        active_id = Some(new_tid);
                        active_agent_name = Some(nm.clone());
                        session_alive = true;
                        is_v2 = live.is_v2();
                    }
                }
            }
            Ok(serde_json::json!({
                "name": hb_row.name,
                "claudeSessionId": hb_row.last_session_id,
                "activeTerminalId": if session_alive { active_id.clone() } else { None },
                "activeAgentName": active_agent_name,
                "sessionAlive": session_alive,
                "isV2": is_v2,
            })
            .to_string())
        }
        _ => Err(format!("Unknown heartbeat route: {path}")),
    }
}

/// Dispatch `/cli/heartbeat-log` (the "all recent fires" diagnostic
/// route). Same pattern as `dispatch_get` but factored out because the
/// URL sits at `/cli/heartbeat-log`, not under `/cli/heartbeat/`.
pub fn dispatch_log(
    project_path: &str,
    params: &HashMap<String, String>,
) -> Result<String, String> {
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(50)
        .clamp(1, 500);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let project_id = k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
        .ok_or_else(|| format!("Project not found: {project_path}"))?;
    k2_core::db::schema::HeartbeatFire::list_by_project(&conn, &project_id, limit)
        .map(|rows| serde_json::to_string(&rows).unwrap_or_default())
        .map_err(|e| e.to_string())
}
