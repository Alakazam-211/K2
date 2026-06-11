//! Multi-heartbeat CRUD + tick evaluation + audit stamping.
//!
//! This is the piece that makes the persistent-agents feature real:
//! when launchd wakes the laptop and fires the heartbeat plist, the
//! daemon calls [`k2so_agents_heartbeat_tick`] to find eligible
//! heartbeats, runs them, and stamps audit rows so
//! `k2so heartbeat status <name>` can show what happened.
//!
//! The entire surface is Tauri-free. src-tauri keeps `#[tauri::command]`
//! wrappers around these functions so the existing UI frontend keeps
//! working unchanged; the daemon calls them directly over its HTTP
//! routes (`/cli/heartbeat/*`).
//!
//! See `.k2so/prds/multi-schedule-heartbeat.md` for the data-model
//! decisions behind this (per-heartbeat folder + `WAKEUP.md`,
//! workspace-relative `wakeup_path`, `heartbeat_fires` audit table).

use std::fs;

use serde::Serialize;

use crate::workspace::agent_identity::{agent_dir, find_primary_agent, resolve_project_id};
use crate::db::schema::{AgentHeartbeat, HeartbeatFire};
use crate::log_debug;
use crate::scheduler::should_project_fire;

// Phase 2.5c: cron schedule parsing + next-fire computation.
pub mod cron;
// Phase 2.5c: launchd plist scaffolding + crontab installer.
pub mod install;
// Phase 2.5d: per-agent heartbeat control (ensure_agent_wakeup +
// get/set + noop/action). Extracted from `agents/commands.rs`.
pub mod control;

/// Create a new heartbeat row + scaffold its `WAKEUP.md` file.
///
/// `frequency` is the scheduler mode name (e.g. `"heartbeat"`,
/// `"daily"`, `"weekly"`, `"ordinal-weekday"`) and `spec_json` is the
/// mode-specific JSON payload (interval seconds, cron-ish spec, etc.).
/// Stores the `WAKEUP.md` path as workspace-relative so project moves
/// don't break rows.
pub fn k2so_heartbeat_add(
    project_path: String,
    name: String,
    frequency: String,
    spec_json: String,
) -> Result<serde_json::Value, String> {
    AgentHeartbeat::validate_name(&name).map_err(|e| e.to_string())?;
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;

    // 0.37.0: heartbeats are workspace-level (.k2so/heartbeats/<sched>/),
    // independent of which agent owns them.
    //
    // 0.38.10 hotfix: validate against `projects.agent_mode` (the
    // workspace-level declaration), not `find_primary_agent` (which
    // probes `.k2so/agent/AGENT.md` for a `name:` field). Pre-0.38.10
    // the disk probe rejected workspaces that had been mode-flipped
    // to custom/manager/k2so-agent BEFORE AGENT.md was written (or
    // whose AGENT.md lacked a `name:` frontmatter) — the DB knew
    // they were bots but the disk hadn't caught up yet. The
    // workspace-mode column is the authoritative signal; if the user
    // said "this is an agent workspace," we trust that.
    let mode: Option<String> = conn
        .query_row(
            "SELECT agent_mode FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();
    let mode_str = mode.unwrap_or_default();
    if !matches!(mode_str.as_str(), "custom" | "manager" | "k2so-agent") {
        return Err(
            "Workspace is not configured as an agent. Set mode to Custom, Workspace Manager, or K2SO Agent first (Settings → Workspaces or `k2so mode <type>`)."
                .to_string(),
        );
    }

    // Create heartbeat folder and scaffold wakeup.md at the
    // workspace-level path the runtime reads from.
    let hb_dir = crate::workspace::agent_identity::workspace_heartbeats_dir(&project_path)
        .join(&name);
    fs::create_dir_all(&hb_dir)
        .map_err(|e| format!("Failed to create heartbeat folder: {}", e))?;
    let wakeup_file = hb_dir.join("WAKEUP.md");
    if !wakeup_file.exists() {
        // Empty body by design. WAKEUP.md is sent verbatim (frontmatter
        // stripped) on every fire — Launch button or cron — so any
        // placeholder text would become noise in the actual wake
        // message. The HTML comment below is markdown-comment syntax
        // that ALSO gets stripped from the wake send (see
        // wake::strip_frontmatter), so it serves as a hint to the user
        // viewing the file in the editor without polluting fires.
        // The optional `description:` frontmatter is shown in other
        // wakeups' cross-context display when set; left blank here so
        // the user can fill it in.
        let _ = name; // template is name-agnostic now
        let template = "---\ndescription:\n---\n\n";
        fs::write(&wakeup_file, template)
            .map_err(|e| format!("Failed to write wakeup.md: {}", e))?;
    }

    // Store workspace-relative path so project moves don't break rows
    let workspace_relative = wakeup_file
        .strip_prefix(&project_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| wakeup_file.to_string_lossy().to_string());

    let id = uuid::Uuid::new_v4().to_string();
    AgentHeartbeat::insert(
        &conn,
        &id,
        &project_id,
        &name,
        &frequency,
        &spec_json,
        &workspace_relative,
        true,
    )
    .map_err(|e| format!("Failed to insert heartbeat: {}", e))?;

    // Drop the DB lock before the cron-install path runs — it shells
    // out to launchctl which can be slow on first install.
    drop(conn);

    // Daemon-first cron bootstrap: ensure ~/.k2so/heartbeat.sh + the
    // launchd plist (or crontab) are installed so this heartbeat
    // actually fires on schedule. Idempotent — a no-op when the
    // infrastructure is already in place. Errors are logged, not
    // returned: we don't want to fail the user's heartbeat add over
    // a launchctl quirk; they can re-apply Settings → Wake Scheduler
    // to recover.
    match crate::heartbeats::install::ensure_cron_installed() {
        Ok(true) => log_debug!("[heartbeat-add] cron infrastructure installed for first time"),
        Ok(false) => {}
        Err(e) => log_debug!("[heartbeat-add] WARN: ensure_cron_installed: {e}"),
    }

    Ok(serde_json::json!({
        "id": id,
        "name": name,
        "wakeupPath": workspace_relative,
        "wakeupAbs": wakeup_file.to_string_lossy(),
    }))
}

/// List active (non-archived) heartbeat rows for a workspace,
/// enabled + disabled. Archived rows are hidden — they appear only in
/// the sidebar's Archived collapsed section, sourced from
/// `k2so_heartbeat_list_archived`.
///
/// Pre-0.36.0 this returned every row; the post-archive filter went in
/// when soft-archive replaced hard-delete.
pub fn k2so_heartbeat_list(project_path: String) -> Result<Vec<AgentHeartbeat>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    AgentHeartbeat::list_active(&conn, &project_id).map_err(|e| e.to_string())
}

/// List archived heartbeat rows for a workspace, newest archive first.
/// Powers the sidebar Heartbeats panel's collapsed Archived section so
/// past chat threads remain auditable after a heartbeat is retired.
pub fn k2so_heartbeat_list_archived(
    project_path: String,
) -> Result<Vec<AgentHeartbeat>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    AgentHeartbeat::list_archived(&conn, &project_id).map_err(|e| e.to_string())
}

/// Soft-archive a heartbeat. Sets `archived_at` to the current
/// timestamp; the row is then hidden from `k2so_heartbeat_list` and
/// excluded from `list_enabled` so the scheduler-tick evaluator stops
/// firing it. Idempotent — re-archiving an already-archived row is a
/// no-op (timestamp preserved).
///
/// Replaces the previous "Remove" delete in the Settings UI from
/// 0.36.0 onward; users who want a real delete can use
/// `k2so_heartbeat_remove` (kept for power-user flows).
pub fn k2so_heartbeat_archive(
    project_path: String,
    name: String,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    AgentHeartbeat::archive(&conn, &project_id, &name)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Restore a soft-archived heartbeat. Reserved for a future
/// "Restore from Archive" UI affordance — no caller in 0.36.0.
pub fn k2so_heartbeat_unarchive(
    project_path: String,
    name: String,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    AgentHeartbeat::unarchive(&conn, &project_id, &name)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Delete a heartbeat row + best-effort remove its `WAKEUP.md` folder.
/// Row delete is the source of truth; folder cleanup is advisory.
pub fn k2so_heartbeat_remove(project_path: String, name: String) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    // 0.38.10: dropped the find_primary_agent disk probe — see add path
    // for rationale. Remove is a row+folder cleanup; the workspace's
    // agent name doesn't influence it. We trust the heartbeat row's
    // existence as proof the workspace was once configured to schedule.

    AgentHeartbeat::delete(&conn, &project_id, &name).map_err(|e| e.to_string())?;
    // 0.37.0: heartbeats live at .k2so/heartbeats/<sched>/ now.
    // 0.37.6: route to recycle bin — heartbeat dir contains the
    // user-edited WAKEUP.md + history files; recoverable on change-of-mind.
    //
    // SAFETY: routes through `scratch_safe_trash` so test scratch
    // paths under temp_dir() skip the trash crate (avoids macOS
    // Touch ID prompts during cargo test).
    let hb_dir = crate::workspace::agent_identity::workspace_heartbeats_dir(&project_path)
        .join(&name);
    if hb_dir.exists() {
        let _ = crate::safe_delete_scratch::scratch_safe_trash(&hb_dir);
    }
    Ok(())
}

/// Toggle a heartbeat's `enabled` flag. Disabled rows are skipped by
/// the tick evaluator regardless of schedule eligibility.
pub fn k2so_heartbeat_set_enabled(
    project_path: String,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    AgentHeartbeat::set_enabled(&conn, &project_id, &name, enabled)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 0.37.8 — flip the per-heartbeat opt-in to deliver WAKEUP.md into
/// the workspace's pinned chat session. When enabled,
/// `heartbeat_launch::smart_launch` skips the heartbeat's own
/// cascade and routes through `workspace_msg::deliver_live` instead.
/// See migration 0043.
pub fn k2so_heartbeat_set_use_workspace_session(
    project_path: String,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    AgentHeartbeat::set_use_workspace_session(&conn, &project_id, &name, enabled)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Replace a heartbeat row's `frequency` + `spec_json` in place. Used
/// when the user edits the schedule via the Settings UI.
pub fn k2so_heartbeat_edit(
    project_path: String,
    name: String,
    frequency: String,
    spec_json: String,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    AgentHeartbeat::update_schedule(&conn, &project_id, &name, &frequency, &spec_json)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Result of a multi-heartbeat tick — one entry per heartbeat eligible
/// to fire right now. Caller is responsible for locking, spawning, and
/// stamping `last_fired` on success.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatFireCandidate {
    pub name: String,
    pub agent_name: String,
    pub wakeup_path_abs: String,
    pub wakeup_path_rel: String,
}

/// Iterate enabled `workspace_heartbeats` rows for a project and return the
/// subset whose schedules are due to fire now.
///
/// Does NOT lock, spawn, or stamp — those are the caller's
/// responsibility. Writes audit rows into `heartbeat_fires` for each
/// evaluated candidate (`fired_multi` / `skipped_schedule` /
/// `wakeup_file_missing`) so `k2so heartbeat status <name>` can show
/// what happened.
///
/// Auto-disables a heartbeat whose `WAKEUP.md` has been deleted from
/// disk — filesystem tampering recovery so the user notices.
pub fn k2so_agents_heartbeat_tick(project_path: &str) -> Vec<HeartbeatFireCandidate> {
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(project_id) = resolve_project_id(&conn, project_path) else {
        return vec![];
    };
    let heartbeats = AgentHeartbeat::list_enabled(&conn, &project_id).unwrap_or_default();
    if heartbeats.is_empty() {
        return vec![];
    }
    let Some(agent_name) = find_primary_agent(project_path) else {
        return vec![];
    };

    let tick_start = std::time::Instant::now();
    let mut candidates = Vec::new();
    for hb in heartbeats {
        let eligible = should_project_fire(
            &hb.frequency,
            Some(&hb.spec_json),
            hb.last_fired.as_deref(),
        );
        if !eligible {
            let _ = HeartbeatFire::insert_with_schedule(
                &conn,
                &project_id,
                Some(&agent_name),
                Some(&hb.name),
                &hb.frequency,
                "skipped_schedule",
                Some("window not open"),
                None,
                None,
                Some(tick_start.elapsed().as_millis() as i64),
            );
            continue;
        }

        // 0.38.2: cron-shaped scheduling via the croner crate. `is_due`
        // asks "is now at or past this heartbeat's next scheduled time?"
        // — backed by `croner` for daily/weekly/monthly/yearly and by
        // a direct `last_fired + every_seconds` for hourly. No deadline
        // grace, no skip-because-late. Long pauses recover automatically.
        //
        // Replaces the pre-0.38.2 `is_past_deadline` whose K8s-CronJob-
        // inspired `starting_deadline_secs` window left heartbeats dark
        // for 22+ days after any pause longer than the grace (~600s).
        // Observed live in production. See `cron_schedule::is_due`.
        if !crate::heartbeats::cron::is_due(&hb) {
            let _ = HeartbeatFire::insert_with_schedule(
                &conn,
                &project_id,
                Some(&agent_name),
                Some(&hb.name),
                &hb.frequency,
                "not_due",
                Some("next scheduled time not yet reached"),
                None,
                None,
                Some(tick_start.elapsed().as_millis() as i64),
            );
            continue;
        }

        let wakeup_abs = std::path::Path::new(project_path).join(&hb.wakeup_path);
        if !wakeup_abs.exists() {
            let _ = AgentHeartbeat::set_enabled(&conn, &project_id, &hb.name, false);
            let _ = HeartbeatFire::insert_with_schedule(
                &conn,
                &project_id,
                Some(&agent_name),
                Some(&hb.name),
                &hb.frequency,
                "wakeup_file_missing",
                Some(&format!(
                    "auto-disabled: {} not found",
                    hb.wakeup_path
                )),
                None,
                None,
                Some(tick_start.elapsed().as_millis() as i64),
            );
            log_debug!(
                "[heartbeat-tick] {} wakeup file missing ({}), auto-disabled",
                hb.name,
                hb.wakeup_path
            );
            continue;
        }

        candidates.push(HeartbeatFireCandidate {
            name: hb.name,
            agent_name: agent_name.clone(),
            wakeup_path_abs: wakeup_abs.to_string_lossy().to_string(),
            wakeup_path_rel: hb.wakeup_path,
        });
    }
    candidates
}

/// Stamp `last_fired` on a heartbeat row. Called AFTER `spawn_wake_pty`
/// succeeds. Silent no-op if the row is gone (heartbeat removed
/// mid-run) — audit rows survive independently.
pub fn stamp_heartbeat_fired(project_path: &str, heartbeat_name: &str) {
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(project_id) = resolve_project_id(&conn, project_path) else {
        return;
    };
    let _ = AgentHeartbeat::stamp_last_fired(&conn, &project_id, heartbeat_name);
}

/// Rename a heartbeat — renames the row AND moves the filesystem
/// folder so `wakeup_path` stays in sync. Lets users swap the
/// migration-reserved `default` name for something meaningful without
/// losing audit history.
///
/// Schedule-name on `heartbeat_fires` is denormalized on purpose —
/// audit survives without a cascade (fires referring to the old name
/// stay pointing at the old value, as designed).
pub fn k2so_heartbeat_rename(
    project_path: String,
    old_name: String,
    new_name: String,
) -> Result<(), String> {
    AgentHeartbeat::validate_name(&new_name).map_err(|e| e.to_string())?;
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    let hb = AgentHeartbeat::get_by_name(&conn, &project_id, &old_name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Heartbeat '{}' not found", old_name))?;
    if AgentHeartbeat::get_by_name(&conn, &project_id, &new_name)
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err(format!("Heartbeat '{}' already exists", new_name));
    }

    // 0.38.10: rename touches only the heartbeat row's name + its
    // wakeup folder on disk; agent identity isn't part of either.
    // Dropped the legacy find_primary_agent probe (see add path).
    // 0.37.0: rename within the workspace-level heartbeats dir.
    let hb_parent = crate::workspace::agent_identity::workspace_heartbeats_dir(&project_path);
    let old_dir = hb_parent.join(&old_name);
    let new_dir = hb_parent.join(&new_name);

    // Tolerate already-moved state for reruns.
    if old_dir.exists() && !new_dir.exists() {
        fs::rename(&old_dir, &new_dir)
            .map_err(|e| format!("Failed to rename heartbeat folder: {}", e))?;
    }

    let new_wakeup = new_dir.join("WAKEUP.md");
    let workspace_relative = new_wakeup
        .strip_prefix(&project_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| new_wakeup.to_string_lossy().to_string());

    conn.execute(
        "UPDATE workspace_heartbeats SET name = ?1, wakeup_path = ?2 \
         WHERE project_id = ?3 AND name = ?4",
        rusqlite::params![new_name, workspace_relative, project_id, old_name],
    )
    .map_err(|e| format!("Failed to rename row: {}", e))?;

    log_debug!(
        "[heartbeat-rename] {} → {} ({})",
        old_name,
        new_name,
        hb.wakeup_path
    );
    Ok(())
}

/// Return the most recent `limit` fire rows for a workspace. Powers
/// the History panel on the Workspaces Settings page. Newest first.
pub fn k2so_heartbeat_fires_list(
    project_path: String,
    limit: Option<i64>,
) -> Result<Vec<HeartbeatFire>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    HeartbeatFire::list_by_project(&conn, &project_id, limit.unwrap_or(50))
        .map_err(|e| e.to_string())
}

/// 0.38.3 — most recent heartbeat fire records across ALL projects,
/// joined with the project name. Powers the universal audit log on
/// the system-wide Heartbeats settings page (`WakeSchedulerSection`).
/// Default limit 100 fires; bump for deeper investigation.
///
/// Hand-builds the JSON in `camelCase` to match the renderer's
/// existing shape and tack on `projectName` for the join.
pub fn k2so_heartbeat_fires_list_all(
    limit: Option<i64>,
) -> Result<Vec<serde_json::Value>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let rows =
        HeartbeatFire::list_all_recent_with_project(&conn, limit.unwrap_or(100))
            .map_err(|e| format!("list_all_recent: {}", e))?;
    let out: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(fire, project_name)| {
            serde_json::json!({
                "id": fire.id,
                "projectId": fire.project_id,
                "projectName": project_name,
                "agentName": fire.agent_name,
                "scheduleName": fire.schedule_name,
                "firedAt": fire.fired_at,
                "mode": fire.mode,
                "decision": fire.decision,
                "reason": fire.reason,
                "inboxPriority": fire.inbox_priority,
                "inboxCount": fire.inbox_count,
                "durationMs": fire.duration_ms,
            })
        })
        .collect();
    Ok(out)
}

/// 0.38.3 — list every active (non-archived) heartbeat across ALL
/// workspaces, with the parent project's name + path joined in. Used
/// by the system-wide Heartbeats settings page (`WakeSchedulerSection`)
/// so the operator can see and toggle every heartbeat from one place.
///
/// JSON is hand-built so the camelCase shape matches the per-workspace
/// `k2so_heartbeat_list` payload the renderer already understands —
/// plus two extra fields (`projectName`, `projectPath`) for the join.
pub fn k2so_heartbeat_list_all() -> Result<Vec<serde_json::Value>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let rows = AgentHeartbeat::list_all_active_with_project(&conn)
        .map_err(|e| format!("list_all_active: {}", e))?;
    let out: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(hb, project_name, project_path)| {
            serde_json::json!({
                "id": hb.id,
                "projectId": hb.project_id,
                "name": hb.name,
                "frequency": hb.frequency,
                "specJson": hb.spec_json,
                "wakeupPath": hb.wakeup_path,
                "enabled": hb.enabled,
                "lastFired": hb.last_fired,
                "lastSessionId": hb.last_session_id,
                "createdAt": hb.created_at,
                "concurrencyPolicy": hb.concurrency_policy,
                "startingDeadlineSecs": hb.starting_deadline_secs,
                "activeDeadlineSecs": hb.active_deadline_secs,
                "useWorkspaceSession": hb.use_workspace_session,
                "projectName": project_name,
                "projectPath": project_path,
            })
        })
        .collect();
    Ok(out)
}

/// Read the workspace's `show_heartbeat_sessions` flag.
///
/// `0` (default) = silent autonomous mode; heartbeat fires never open
/// tabs. Audit via the sidebar Heartbeats panel on demand.
/// `1` = each scheduled heartbeat fire opens a background tab in the
/// Tauri window. Tab persists until the user closes it.
pub fn k2so_workspace_get_show_heartbeat_sessions(
    project_path: String,
) -> Result<bool, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let v: i64 = conn
        .query_row(
            "SELECT show_heartbeat_sessions FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |r| r.get(0),
        )
        .map_err(|e| format!("workspace not found: {e}"))?;
    Ok(v != 0)
}

/// Flip the workspace's `show_heartbeat_sessions` flag.
pub fn k2so_workspace_set_show_heartbeat_sessions(
    project_path: String,
    enabled: bool,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let rows = conn
        .execute(
            "UPDATE projects SET show_heartbeat_sessions = ?1 WHERE path = ?2",
            rusqlite::params![enabled as i64, project_path],
        )
        .map_err(|e| format!("workspace update failed: {e}"))?;
    if rows == 0 {
        return Err(format!("workspace not found: {project_path}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Behaviour lives in src-tauri's integration tests today —
    //! `src-tauri/src/commands/k2so_agents.rs` has 30+ tests that
    //! exercise these same functions under their original call sites.
    //! Once the commands module itself moves into core the tests can
    //! come along.
}
