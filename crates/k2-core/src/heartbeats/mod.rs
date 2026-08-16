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

use crate::workspace::agent_identity::{resolve_agent_name, resolve_project_id};
use crate::db::schema::{AgentHeartbeat, HeartbeatFire};

// Phase 2.5c: cron schedule parsing + next-fire computation.
pub mod cron;
// Phase 2.5c: launchd plist scaffolding + crontab installer.
pub mod install;
// Phase 2.5d: wakeup scaffolding (ensure_agent_wakeup). Extracted from
// `agents/commands.rs`; the legacy per-agent get/set/noop/action
// control API was deleted in 0.40.31.
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
    // GH#27: server-side spec validation. The CLI validates too, but
    // stale CLIs exist — same defense-in-depth as the rest of the add
    // gate. Rejects garbage weekdays/months/days-of-month HERE instead
    // of storing a row that later evaluates `schedule_invalid`.
    validate_spec_json(&frequency, &spec_json)?;
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;

    // Heartbeats are workspace-level (.k2/heartbeats/<sched>/). Workspace
    // types (custom / manager / k2) are retired — any registered
    // workspace can add a heartbeat.

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
    .map_err(|e| friendly_heartbeat_insert_error(&name, &e.to_string()))?;

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

    refresh_agents_md_if_heartbeats_roster(&project_path);

    Ok(serde_json::json!({
        "id": id,
        "name": name,
        "wakeupPath": workspace_relative,
        "wakeupAbs": wakeup_file.to_string_lossy(),
    }))
}

/// GH#27 — translate the raw sqlite error from a heartbeat insert into
/// a human-readable message. A duplicate name violates the
/// `(project_id, name)` UNIQUE constraint on `workspace_heartbeats`;
/// pre-fix the CLI printed the raw
/// `UNIQUE constraint failed: workspace_heartbeats.project_id,
/// workspace_heartbeats.name` SQL error verbatim.
fn friendly_heartbeat_insert_error(name: &str, raw: &str) -> String {
    if raw.contains("UNIQUE constraint failed") && raw.contains("workspace_heartbeats") {
        format!("a heartbeat named '{}' already exists in this workspace", name)
    } else {
        format!("Failed to insert heartbeat: {}", raw)
    }
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
        .map_err(|e| e.to_string())?;
    drop(conn);
    refresh_agents_md_if_heartbeats_roster(&project_path);
    Ok(())
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
        .map_err(|e| e.to_string())?;
    drop(conn);
    refresh_agents_md_if_heartbeats_roster(&project_path);
    Ok(())
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
    drop(conn);
    refresh_agents_md_if_heartbeats_roster(&project_path);
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
        .map_err(|e| e.to_string())?;
    drop(conn);
    refresh_agents_md_if_heartbeats_roster(&project_path);
    Ok(())
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

/// 0073 — set a heartbeat's DELIVERY SESSION: where a fire's wake
/// message lands. Three modes (the Settings drop-down / `k2 heartbeat
/// session` vocabulary):
///
/// - `"pinned"` — deliver into the workspace's pinned chat session
///   (`use_workspace_session = 1`). The heartbeat's own
///   `last_session_id`/`session_provider` stay untouched so switching
///   back restores the historical thread — same contract as
///   [`k2so_heartbeat_set_use_workspace_session`].
/// - `"auto"` — the heartbeat's own session, minted fresh on the next
///   fire: `use_workspace_session = 0`, `last_session_id = NULL`,
///   `session_provider = NULL`.
/// - `"session"` — a SPECIFIC saved session. Requires `session_id` +
///   `provider`; validates (loudly, per test discipline):
///   (a) the provider is known to the `ProviderResume` adapter table;
///   (b) the session is NOT the workspace's pinned chat session
///       (that one is reserved — use mode `pinned`);
///   (c) the provider's session file exists on disk for this project.
///   Then `use_workspace_session = 0` + both columns set atomically.
///
/// Returns `{"success":true,"mode":...}` (+ `sessionId`/`provider`
/// for mode `session`) so the route can echo the applied state.
/// Resolve `--set` to a provider conversation id.
/// Accepts a raw session UUID, `sales/reviewer`, or a bare handle (`1` / `reviewer`)
/// in this workspace. Infers provider from the tab row / Chats name when possible.
fn resolve_set_session_target(
    project_path: &str,
    project_id: &str,
    raw: &str,
    explicit_provider: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let token = raw.trim();
    let handle = if let Some((ws, handle)) =
        crate::workspace_session_handles::split_workspace_handle(token)
    {
        let here = {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT path FROM projects WHERE id = ?1",
                rusqlite::params![project_id],
                |r| r.get::<_, String>(0),
            )
            .ok()
        };
        let named = workspace_name_to_path(ws);
        if let (Some(here), Some(named)) = (here, named) {
            if here != named {
                return Err(format!("handle '{token}' is not in this workspace"));
            }
        }
        let _ = project_path;
        handle.to_string()
    } else if crate::workspace_session_handles::is_uuid_shape(token)
        || explicit_provider.is_some()
    {
        // UUID, or legacy `--set <opaque-id> --provider <p>`.
        return Ok((token.to_string(), infer_provider_for_session(token)));
    } else {
        token.to_string()
    };

    let key = {
        let db = crate::db::shared();
        let conn = db.lock();
        let key = match crate::workspace_session_handles::resolve_handle(
            &conn, project_id, &handle,
        ) {
            Ok(key) => key,
            Err(_) => {
                // Not a known handle — treat as a raw session id and let
                // the provider / disk-probe gates fail loud.
                return Ok((token.to_string(), infer_provider_for_session(token)));
            }
        };
        // Prefer the provider conversation id on the tab row when the
        // handle table is still keyed on pane_group_id.
        crate::db::schema::WorkspaceTabSession::get_by_session_id(&conn, &key)
            .ok()
            .flatten()
            .and_then(|t| t.session_id)
            .or_else(|| {
                crate::db::schema::WorkspaceTabSession::get(&conn, project_id, &key)
                    .ok()
                    .flatten()
                    .and_then(|t| t.session_id)
            })
            .unwrap_or(key)
    };
    let provider = infer_provider_for_session(&key);
    Ok((key, provider))
}

/// Best-effort provider for a conversation key (tab command, else chat_session_names).
fn infer_provider_for_session(session_id: &str) -> Option<String> {
    let db = crate::db::shared();
    let conn = db.lock();
    if let Ok(Some(tab)) =
        crate::db::schema::WorkspaceTabSession::get_by_session_id(&conn, session_id)
    {
        if let Some(cmd) = tab.command.as_deref() {
            if let Some(adapter) =
                crate::workspace::provider_resume::provider_resume_for_command(cmd)
            {
                return Some(adapter.provider.to_string());
            }
        }
    }
    conn.query_row(
        "SELECT provider FROM chat_session_names WHERE session_id = ?1 \
         AND TRIM(provider) != '' ORDER BY updated_at DESC LIMIT 1",
        rusqlite::params![session_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// Avoid a daemon↔core cycle: resolve a workspace name to path via projects.
fn workspace_name_to_path(token: &str) -> Option<String> {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT path FROM projects WHERE name = ?1 COLLATE NOCASE ORDER BY rowid LIMIT 1",
        rusqlite::params![token],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

pub fn k2so_heartbeat_set_session(
    project_path: String,
    name: String,
    mode: String,
    session_id: Option<String>,
    provider: Option<String>,
) -> Result<serde_json::Value, String> {
    // ── Resolve + validate under the lock ──────────────────────────
    let (project_id, pinned_session_id) = {
        let db = crate::db::shared();
        let conn = db.lock();
        let project_id = resolve_project_id(&conn, &project_path)
            .ok_or_else(|| format!("Project not found: {}", project_path))?;
        // Loud failure on a bogus name — a silent 0-row UPDATE would
        // let the UI think the drop-down took effect.
        AgentHeartbeat::get_by_name(&conn, &project_id, &name)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Heartbeat '{}' not found", name))?;
        let pinned = crate::db::schema::WorkspaceSession::get(&conn, &project_id)
            .ok()
            .flatten()
            .and_then(|ws| ws.session_id);
        (project_id, pinned)
    };

    match mode.as_str() {
        "pinned" => {
            let db = crate::db::shared();
            let conn = db.lock();
            AgentHeartbeat::set_use_workspace_session(&conn, &project_id, &name, true)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::json!({ "success": true, "mode": "pinned" }))
        }
        "auto" => {
            let db = crate::db::shared();
            let conn = db.lock();
            AgentHeartbeat::set_use_workspace_session(&conn, &project_id, &name, false)
                .map_err(|e| e.to_string())?;
            AgentHeartbeat::set_session(&conn, &project_id, &name, None, None)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::json!({ "success": true, "mode": "auto" }))
        }
        "session" => {
            let raw = session_id
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "mode 'session' requires a 'session_id' parameter".to_string())?;
            let (session_id, inferred_provider) =
                resolve_set_session_target(&project_path, &project_id, &raw, provider.as_deref())?;
            let provider = provider
                .filter(|s| !s.is_empty())
                .or(inferred_provider)
                .ok_or_else(|| {
                    "mode 'session' requires --provider <p> (or pass a sidecar handle like sales/reviewer)"
                        .to_string()
                })?;
            // (a) provider must be a known ProviderResume adapter —
            // an unknown provider could never be probed or resumed.
            let adapter = crate::workspace::provider_resume::provider_resume_for_provider(
                &provider,
            )
            .ok_or_else(|| {
                format!(
                    "unknown provider '{}' — no resume adapter; known providers: \
                     claude, grok, cursor, gemini, pi, codex, hermes",
                    provider
                )
            })?;
            // (b) the pinned chat session is the workspace chat tab's
            // lane; pointing a heartbeat at it directly would collide
            // with the deliver_live cascade. Mode `pinned` exists for
            // exactly that intent.
            if pinned_session_id.as_deref() == Some(session_id.as_str()) {
                return Err(
                    "the pinned chat session is reserved; choose mode=pinned instead"
                        .to_string(),
                );
            }
            // (c) the session must actually exist on disk — a typo'd
            // or deleted id fails HERE (loudly), not silently at the
            // next 3am fire. Disk probe runs without the DB lock.
            if !adapter.session_file_exists(&session_id, &project_path) {
                return Err(format!(
                    "no {} session '{}' found on disk for this workspace — \
                     cannot deliver a heartbeat into a session that does not exist",
                    provider, session_id
                ));
            }
            let db = crate::db::shared();
            let conn = db.lock();
            AgentHeartbeat::set_use_workspace_session(&conn, &project_id, &name, false)
                .map_err(|e| e.to_string())?;
            AgentHeartbeat::set_session(
                &conn,
                &project_id,
                &name,
                Some(&session_id),
                Some(&provider),
            )
            .map_err(|e| e.to_string())?;
            Ok(serde_json::json!({
                "success": true,
                "mode": "session",
                "sessionId": session_id,
                "provider": provider,
            }))
        }
        other => Err(format!(
            "unknown mode '{}' — expected 'pinned', 'auto', or 'session'",
            other
        )),
    }
}

/// GH#27 — server-side schedule-spec validation shared by the add and
/// edit paths (`/cli/heartbeat/add`, `/cli/heartbeat/edit`). The CLI
/// validates the same fields client-side, but stale CLIs exist; without
/// this gate a `--weekly --days foobar` row is stored verbatim and only
/// surfaces as `schedule_invalid` at the next tick.
///
/// Checks (tolerant of unknown fields/frequencies — only the fields the
/// cron translator consumes are policed):
/// - weekly `days` entries ∈ mon|tue|wed|thu|fri|sat|sun (case-insensitive)
/// - yearly `months` entries ∈ jan..dec (case-insensitive)
/// - monthly/yearly `days_of_month` (and singular `day_of_month`) ∈ 1–31
///
/// An empty spec is accepted unchanged (legacy rows / frequency-only
/// edits); a non-empty spec that isn't valid JSON is rejected loudly.
pub fn validate_spec_json(frequency: &str, spec_json: &str) -> Result<(), String> {
    if spec_json.trim().is_empty() {
        return Ok(());
    }
    let v: serde_json::Value = serde_json::from_str(spec_json)
        .map_err(|e| format!("schedule spec is not valid JSON: {e}"))?;

    const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun",
        "jul", "aug", "sep", "oct", "nov", "dec",
    ];

    if frequency == "weekly" {
        if let Some(arr) = v.get("days").and_then(|d| d.as_array()) {
            for entry in arr {
                let ok = entry
                    .as_str()
                    .map(|s| WEEKDAYS.contains(&s.to_ascii_lowercase().as_str()))
                    .unwrap_or(false);
                if !ok {
                    return Err(format!(
                        "invalid weekly day {} — expected one of mon|tue|wed|thu|fri|sat|sun",
                        entry
                    ));
                }
            }
        }
    }
    if frequency == "yearly" {
        if let Some(arr) = v.get("months").and_then(|d| d.as_array()) {
            for entry in arr {
                let ok = entry
                    .as_str()
                    .map(|s| MONTHS.contains(&s.to_ascii_lowercase().as_str()))
                    .unwrap_or(false);
                if !ok {
                    return Err(format!(
                        "invalid yearly month {} — expected one of jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec",
                        entry
                    ));
                }
            }
        }
    }
    if frequency == "monthly" || frequency == "yearly" {
        if let Some(arr) = v.get("days_of_month").and_then(|d| d.as_array()) {
            for entry in arr {
                let ok = entry
                    .as_i64()
                    .map(|d| (1..=31).contains(&d))
                    .unwrap_or(false);
                if !ok {
                    return Err(format!(
                        "invalid day of month {} — expected an integer 1-31",
                        entry
                    ));
                }
            }
        }
        if let Some(d) = v.get("day_of_month").and_then(|d| d.as_i64()) {
            if !(1..=31).contains(&d) {
                return Err(format!("invalid day of month {d} — expected an integer 1-31"));
            }
        }
    }
    Ok(())
}

/// Replace a heartbeat row's `frequency` + `spec_json` in place. Used
/// when the user edits the schedule via the Settings UI.
pub fn k2so_heartbeat_edit(
    project_path: String,
    name: String,
    frequency: String,
    spec_json: String,
) -> Result<(), String> {
    // GH#27: same server-side spec gate as the add path.
    validate_spec_json(&frequency, &spec_json)?;
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    AgentHeartbeat::update_schedule(&conn, &project_id, &name, &frequency, &spec_json)
        .map(|_| ())
        .map_err(|e| e.to_string())?;
    drop(conn);
    refresh_agents_md_if_heartbeats_roster(&project_path);
    Ok(())
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
    /// RFC3339 of the originally-scheduled occurrence when this fire is
    /// a CATCH-UP for a missed slot (evaluator returned `DueCatchUp`).
    /// None = on-time fire. The launcher writes `fired_catchup` (with
    /// this timestamp in the reason) instead of `fired` so the audit
    /// trail distinguishes recovered misses from on-time fires.
    pub catchup_of: Option<String>,
}

/// Iterate enabled `workspace_heartbeats` rows for a project and return the
/// subset whose schedules are due to fire now.
///
/// Reliability overhaul: `cron::evaluate` is the SINGLE due-authority —
/// the legacy `should_project_fire` calendar-position gate (which
/// silently dropped any miss that crossed a day/week/month boundary,
/// and whose once-per-day latch let a manual launch consume the day's
/// scheduled fire) no longer runs for these rows. Misses always catch
/// up, coalesced to one fire for the most recent missed occurrence.
///
/// Does NOT lock, spawn, or stamp — those are the caller's
/// responsibility. Audit hygiene: quiet non-events (`NotYet`,
/// window-holds, backoff waits) write NO rows — pre-overhaul every
/// tick wrote a `not_due`/`skipped_schedule` row per heartbeat (1,440
/// rows/heartbeat/day) and real fires drowned. Rows are written only
/// for decisions that carry information: fires (by the launcher),
/// `schedule_invalid` (on state transition), `wakeup_file_missing`.
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
    // #70: DB-canonical name (file `name:` → workspace basename when the
    // workspace is configured but has no AGENT.md) so heartbeats still fire
    // for fileless configured workspaces.
    let Some(agent_name) = resolve_agent_name(project_path) else {
        return vec![];
    };

    let now = chrono::Local::now();
    let tick_start = std::time::Instant::now();
    let mut candidates = Vec::new();
    for hb in heartbeats {
        // Failure backoff: after a failed fire-attempt the launcher
        // stamps `next_retry_at` (exponential); until then the row is
        // not retried. Quiet — the failure itself already wrote an
        // `error` audit row. Manual launches bypass this gate.
        if let Some(retry_at) = hb
            .next_retry_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        {
            if now < retry_at {
                continue;
            }
        }

        let catchup_of = match cron::evaluate(&hb) {
            cron::DueStatus::Due { .. } => None,
            cron::DueStatus::DueCatchUp { missed_at } => Some(missed_at.to_rfc3339()),
            cron::DueStatus::NotYet { .. } | cron::DueStatus::HoldWindow { .. } => {
                // Not due / holding for the firing window: quiet.
                clear_schedule_error_if_set(&conn, &project_id, &hb);
                continue;
            }
            cron::DueStatus::Invalid { reason } => {
                // A schedule that can never fire is surfaced ONCE per
                // state change: persist `schedule_error` (Settings
                // badge) + one `schedule_invalid` audit row — not a
                // row per tick.
                if hb.schedule_error.as_deref() != Some(reason.as_str()) {
                    let _ = AgentHeartbeat::set_schedule_error(
                        &conn, &project_id, &hb.name, Some(&reason),
                    );
                    let _ = HeartbeatFire::insert_with_schedule(
                        &conn,
                        &project_id,
                        Some(&agent_name),
                        Some(&hb.name),
                        &hb.frequency,
                        "schedule_invalid",
                        Some(&reason),
                        None,
                        None,
                        Some(tick_start.elapsed().as_millis() as i64),
                    );
                    log_debug!(
                        "[heartbeat-tick] {} schedule invalid: {}",
                        hb.name,
                        reason
                    );
                }
                continue;
            }
        };
        clear_schedule_error_if_set(&conn, &project_id, &hb);

        let wakeup_abs = std::path::Path::new(project_path).join(&hb.wakeup_path);
        if !wakeup_abs.exists() {
            let _ = AgentHeartbeat::auto_disable(
                &conn, &project_id, &hb.name, "wakeup_missing",
            );
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
            catchup_of,
        });
    }
    candidates
}

/// Clear a stale `schedule_error` once the row evaluates cleanly again
/// (spec fixed via edit, or a transient parse issue resolved). Quiet —
/// the recovery needs no audit row.
fn clear_schedule_error_if_set(
    conn: &rusqlite::Connection,
    project_id: &str,
    hb: &AgentHeartbeat,
) {
    if hb.schedule_error.is_some() {
        let _ = AgentHeartbeat::set_schedule_error(conn, project_id, &hb.name, None);
    }
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
    drop(conn);
    refresh_agents_md_if_heartbeats_roster(&project_path);
    Ok(())
}

fn refresh_agents_md_if_heartbeats_roster(project_path: &str) {
    crate::workspace::context_layers::refresh_roster_after_live_kind_change(
        &[project_path],
        crate::workspace::context_layers::LiveKind::Heartbeats,
    );
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
                "sessionProvider": hb.session_provider,
                "createdAt": hb.created_at,
                "concurrencyPolicy": hb.concurrency_policy,
                "startingDeadlineSecs": hb.starting_deadline_secs,
                "activeDeadlineSecs": hb.active_deadline_secs,
                "useWorkspaceSession": hb.use_workspace_session,
                "consecutiveFailures": hb.consecutive_failures,
                "disabledReason": hb.disabled_reason,
                "scheduleError": hb.schedule_error,
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
    //! Legacy CRUD behaviour lives in src-tauri's integration tests —
    //! `src-tauri/src/commands/k2so_agents.rs` has 30+ tests that
    //! exercise those functions under their original call sites.
    //!
    //! `k2so_heartbeat_set_session` (0073) is tested HERE — it was
    //! born in core, so its mode/validation matrix has no other home.

    use super::*;

    /// Scratch-$HOME guard for the on-disk session probe (mode
    /// `session` validation (c)). Same pattern as
    /// `workspace::provider_resume::tests::HomeGuard`.
    struct HomeGuard {
        original: Option<std::ffi::OsString>,
        home: std::path::PathBuf,
        _lock: parking_lot::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn new(label: &str) -> Self {
            let lock = crate::themes::HOME_LOCK.lock();
            let home = std::env::temp_dir().join(format!(
                "k2-hb-set-session-{label}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&home).unwrap();
            let original = std::env::var_os("HOME");
            std::env::set_var("HOME", &home);
            Self { original, home, _lock: lock }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    /// Register a project + one heartbeat row named `hb`. Returns the
    /// project_id. Project path must be unique per test (shared DB).
    fn seed_project_and_heartbeat(project_path: &str) -> String {
        crate::db::init_for_tests();
        let db = crate::db::shared();
        let conn = db.lock();
        let project_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, 'hb-session-test', ?2)",
            rusqlite::params![project_id, project_path],
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
            true,
        )
        .unwrap();
        project_id
    }

    fn hb_row(project_id: &str) -> AgentHeartbeat {
        let db = crate::db::shared();
        let conn = db.lock();
        AgentHeartbeat::get_by_name(&conn, project_id, "hb")
            .unwrap()
            .expect("heartbeat row exists")
    }

    /// Seed a saved delivery session directly (bypassing the disk
    /// probe) so mode transitions can be asserted.
    #[test]
    fn heartbeat_add_does_not_require_workspace_type() {
        crate::db::init_for_tests();
        let dir = std::env::temp_dir().join(format!(
            "k2-hb-add-no-mode-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().to_string();
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, agent_mode) VALUES (?1, 'off-ws', ?2, 'off')",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), path],
            )
            .unwrap();
        }
        let out = k2so_heartbeat_add(
            path.clone(),
            "daily-check".into(),
            "daily".into(),
            "{}".into(),
        )
        .expect("add must succeed when workspace type is off / unset");
        assert_eq!(out["name"], "daily-check");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn seed_saved_session(project_id: &str, session_id: &str, provider: &str) {
        let db = crate::db::shared();
        let conn = db.lock();
        AgentHeartbeat::set_session(
            &conn, project_id, "hb", Some(session_id), Some(provider),
        )
        .unwrap();
    }

    #[test]
    fn new_heartbeat_insert_is_pinned_and_preexisting_auto_stays_auto() {
        crate::db::init_for_tests();
        let path = format!(
            "/fixture/hb-d22-insert-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        );
        let db = crate::db::shared();
        let conn = db.lock();
        let project_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, 'd22', ?2)",
            rusqlite::params![project_id, path],
        )
        .expect("seed project");
        conn.execute(
            "INSERT INTO workspace_heartbeats \
             (id, project_id, name, frequency, spec_json, wakeup_path, enabled, created_at, use_workspace_session) \
             VALUES (?1, ?2, 'legacy-auto', 'daily', '{}', '.k2/heartbeats/legacy-auto/WAKEUP.md', 1, unixepoch(), 0)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), project_id],
        )
        .expect("seed pre-existing auto");
        AgentHeartbeat::insert(
            &conn,
            &uuid::Uuid::new_v4().to_string(),
            &project_id,
            "fresh-pinned",
            "daily",
            "{}",
            ".k2/heartbeats/fresh-pinned/WAKEUP.md",
            true,
        )
        .expect("new insert");
        let legacy = AgentHeartbeat::get_by_name(&conn, &project_id, "legacy-auto")
            .expect("q")
            .expect("legacy row");
        assert!(
            !legacy.use_workspace_session,
            "D22: must not UPDATE existing auto rows onto pinned"
        );
        let fresh = AgentHeartbeat::get_by_name(&conn, &project_id, "fresh-pinned")
            .expect("q")
            .expect("fresh row");
        assert!(
            fresh.use_workspace_session,
            "D22: new INSERT has use_workspace_session == true"
        );
    }

    #[test]
    fn set_session_pinned_flips_flag_and_preserves_saved_session() {
        let path = format!("/fixture/hb-set-session-pinned-{}", uuid::Uuid::new_v4());
        let project_id = seed_project_and_heartbeat(&path);
        seed_saved_session(&project_id, "historic-sid", "grok");

        let v = k2so_heartbeat_set_session(path, "hb".into(), "pinned".into(), None, None)
            .expect("pinned mode succeeds");
        assert_eq!(v["success"], true);
        assert_eq!(v["mode"], "pinned");

        let hb = hb_row(&project_id);
        assert!(hb.use_workspace_session, "pinned sets the flag");
        assert_eq!(
            hb.last_session_id.as_deref(),
            Some("historic-sid"),
            "pinned must NOT clear the historical session id"
        );
        assert_eq!(
            hb.session_provider.as_deref(),
            Some("grok"),
            "pinned must NOT clear the historical provider"
        );
    }

    #[test]
    fn set_session_auto_clears_both_fields_and_flag() {
        let path = format!("/fixture/hb-set-session-auto-{}", uuid::Uuid::new_v4());
        let project_id = seed_project_and_heartbeat(&path);
        seed_saved_session(&project_id, "old-sid", "claude");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            AgentHeartbeat::set_use_workspace_session(&conn, &project_id, "hb", true).unwrap();
        }

        let v = k2so_heartbeat_set_session(path, "hb".into(), "auto".into(), None, None)
            .expect("auto mode succeeds");
        assert_eq!(v["mode"], "auto");

        let hb = hb_row(&project_id);
        assert!(!hb.use_workspace_session, "auto clears the pinned flag");
        assert_eq!(hb.last_session_id, None, "auto clears last_session_id");
        assert_eq!(hb.session_provider, None, "auto clears session_provider");
    }

    #[test]
    fn set_session_session_mode_requires_id_and_provider() {
        let path = format!("/fixture/hb-set-session-req-{}", uuid::Uuid::new_v4());
        let _pid = seed_project_and_heartbeat(&path);

        let err = k2so_heartbeat_set_session(
            path.clone(), "hb".into(), "session".into(), None, Some("claude".into()),
        )
        .unwrap_err();
        assert!(err.contains("session_id"), "err={err}");

        let err = k2so_heartbeat_set_session(
            path, "hb".into(), "session".into(), Some("sid-1".into()), None,
        )
        .unwrap_err();
        assert!(err.contains("provider"), "err={err}");
    }

    #[test]
    fn set_session_unknown_provider_errors() {
        let path = format!("/fixture/hb-set-session-unk-{}", uuid::Uuid::new_v4());
        let _pid = seed_project_and_heartbeat(&path);
        let err = k2so_heartbeat_set_session(
            path,
            "hb".into(),
            "session".into(),
            Some("sid-1".into()),
            Some("aider".into()),
        )
        .unwrap_err();
        assert!(err.contains("unknown provider 'aider'"), "err={err}");
    }

    #[test]
    fn set_session_pinned_chat_session_is_reserved() {
        let path = format!("/fixture/hb-set-session-resv-{}", uuid::Uuid::new_v4());
        let project_id = seed_project_and_heartbeat(&path);
        {
            let db = crate::db::shared();
            let conn = db.lock();
            crate::db::schema::WorkspaceSession::upsert(
                &conn,
                &uuid::Uuid::new_v4().to_string(),
                &project_id,
                None,
                Some("the-pinned-sid"),
                "claude",
                "user",
                "running",
            )
            .unwrap();
        }
        let err = k2so_heartbeat_set_session(
            path,
            "hb".into(),
            "session".into(),
            Some("the-pinned-sid".into()),
            Some("claude".into()),
        )
        .unwrap_err();
        assert_eq!(
            err,
            "the pinned chat session is reserved; choose mode=pinned instead"
        );
    }

    #[test]
    fn set_session_missing_session_file_errors_loudly() {
        let _guard = HomeGuard::new("missing-file");
        let path = format!("/fixture/hb-set-session-miss-{}", uuid::Uuid::new_v4());
        let project_id = seed_project_and_heartbeat(&path);
        let err = k2so_heartbeat_set_session(
            path,
            "hb".into(),
            "session".into(),
            Some("ghost-sid".into()),
            Some("claude".into()),
        )
        .unwrap_err();
        assert!(
            err.contains("claude") && err.contains("ghost-sid"),
            "error must name the provider and the id: {err}"
        );
        // Nothing written on the failure path.
        let hb = hb_row(&project_id);
        assert_eq!(hb.last_session_id, None);
        assert_eq!(hb.session_provider, None);
    }

    #[test]
    fn set_session_session_mode_sets_both_fields_when_file_exists() {
        let guard = HomeGuard::new("happy");
        let path = format!("/fixture/hb-set-session-ok-{}", uuid::Uuid::new_v4());
        let project_id = seed_project_and_heartbeat(&path);
        let sid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeee0073";
        // Claude on-disk fixture the probe accepts.
        let hash = crate::chat_history::claude_project_hash(&path);
        let dir = guard.home.join(".claude").join("projects").join(&hash);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{sid}.jsonl")), b"{\"cwd\":\"/x\"}\n").unwrap();
        // Flag ON beforehand so we can assert mode `session` clears it.
        {
            let db = crate::db::shared();
            let conn = db.lock();
            AgentHeartbeat::set_use_workspace_session(&conn, &project_id, "hb", true).unwrap();
        }

        let v = k2so_heartbeat_set_session(
            path,
            "hb".into(),
            "session".into(),
            Some(sid.into()),
            Some("claude".into()),
        )
        .expect("session mode succeeds with the file on disk");
        assert_eq!(v["success"], true);
        assert_eq!(v["mode"], "session");
        assert_eq!(v["sessionId"], sid);
        assert_eq!(v["provider"], "claude");

        let hb = hb_row(&project_id);
        assert!(!hb.use_workspace_session, "session mode clears the pinned flag");
        assert_eq!(hb.last_session_id.as_deref(), Some(sid));
        assert_eq!(hb.session_provider.as_deref(), Some("claude"));
    }

    #[test]
    fn set_session_accepts_sidecar_handle_and_infers_provider() {
        let guard = HomeGuard::new("handle-set");
        let path = format!("/fixture/hb-set-handle-{}", uuid::Uuid::new_v4());
        let project_id = seed_project_and_heartbeat(&path);
        let sid = uuid::Uuid::new_v4().to_string();
        let hash = crate::chat_history::claude_project_hash(&path);
        let dir = guard.home.join(".claude").join("projects").join(&hash);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{sid}.jsonl")), b"{\"cwd\":\"/x\"}\n").unwrap();
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO workspace_tab_sessions \
                 (project_id, pane_group_id, agent_name, session_id, command, last_seen_at) \
                 VALUES (?1, ?2, ?3, ?4, 'claude', unixepoch())",
                rusqlite::params![project_id, "pane-rev", format!("tab-pane-rev"), sid],
            )
            .expect("tab");
            conn.execute(
                "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
                 VALUES ('claude', ?1, 'Reviewer', 0, unixepoch())",
                rusqlite::params![sid],
            )
            .expect("name");
            crate::workspace_session_handles::allocate_ordinal(&conn, &project_id, &sid)
                .expect("ord");
        }
        let v = k2so_heartbeat_set_session(
            path,
            "hb".into(),
            "session".into(),
            Some("reviewer".into()),
            None,
        )
        .expect("handle --set without --provider");
        assert_eq!(v["sessionId"], sid);
        assert_eq!(v["provider"], "claude");
    }

    #[test]
    fn set_session_unknown_mode_and_missing_heartbeat_error() {
        let path = format!("/fixture/hb-set-session-bad-{}", uuid::Uuid::new_v4());
        let _pid = seed_project_and_heartbeat(&path);
        let err = k2so_heartbeat_set_session(
            path.clone(), "hb".into(), "banana".into(), None, None,
        )
        .unwrap_err();
        assert!(err.contains("unknown mode 'banana'"), "err={err}");

        let err = k2so_heartbeat_set_session(
            path, "no-such-hb".into(), "auto".into(), None, None,
        )
        .unwrap_err();
        assert!(err.contains("'no-such-hb' not found"), "err={err}");
    }

    /// The self-heal `clear_session_id` must clear BOTH the ghost id
    /// and its provider pin — a leftover provider would make the next
    /// saved id probe the wrong store.
    #[test]
    fn clear_session_id_clears_the_provider_pin_too() {
        let path = format!("/fixture/hb-clear-both-{}", uuid::Uuid::new_v4());
        let project_id = seed_project_and_heartbeat(&path);
        seed_saved_session(&project_id, "ghost", "grok");

        let db = crate::db::shared();
        let conn = db.lock();
        AgentHeartbeat::clear_session_id(&conn, &project_id, "hb").unwrap();
        let hb = AgentHeartbeat::get_by_name(&conn, &project_id, "hb")
            .unwrap()
            .unwrap();
        assert_eq!(hb.last_session_id, None);
        assert_eq!(hb.session_provider, None, "self-heal must clear session_provider");
    }

    // ── GH#27: duplicate-name error mapping ──────────────────────────

    /// The mapping must fire on the REAL sqlite error a duplicate
    /// (project_id, name) insert produces — not just a hand-written
    /// string — so the test performs the double insert itself.
    #[test]
    fn duplicate_heartbeat_insert_error_is_human_readable() {
        let path = format!("/fixture/hb-dup-name-{}", uuid::Uuid::new_v4());
        let project_id = seed_project_and_heartbeat(&path); // seeds hb named "hb"
        let db = crate::db::shared();
        let conn = db.lock();
        let raw = AgentHeartbeat::insert(
            &conn,
            &uuid::Uuid::new_v4().to_string(),
            &project_id,
            "hb", // duplicate name in the same workspace
            "daily",
            "{}",
            ".k2/heartbeats/hb/WAKEUP.md",
            true,
        )
        .expect_err("duplicate (project_id, name) must violate UNIQUE")
        .to_string();
        assert!(
            raw.contains("UNIQUE constraint failed"),
            "precondition: raw sqlite error shape changed? raw={raw}"
        );
        assert_eq!(
            friendly_heartbeat_insert_error("hb", &raw),
            "a heartbeat named 'hb' already exists in this workspace",
        );
        // Non-duplicate errors keep the raw detail.
        let other = friendly_heartbeat_insert_error("hb", "database is locked");
        assert_eq!(other, "Failed to insert heartbeat: database is locked");
    }

    // ── GH#27: server-side schedule-spec validation ──────────────────

    #[test]
    fn validate_spec_rejects_bad_weekly_days() {
        let err = validate_spec_json("weekly", r#"{"time":"09:00","days":["foobar"]}"#)
            .expect_err("garbage weekday must be rejected");
        assert!(err.contains("invalid weekly day"), "err={err}");
        assert!(err.contains("mon|tue|wed|thu|fri|sat|sun"), "err={err}");
        // Non-string entries are equally invalid.
        assert!(validate_spec_json("weekly", r#"{"days":[3]}"#).is_err());
    }

    #[test]
    fn validate_spec_accepts_valid_weekly_days_case_insensitive() {
        validate_spec_json("weekly", r#"{"time":"09:00","days":["mon","WED","Fri"]}"#)
            .expect("valid weekdays in any case must pass");
    }

    #[test]
    fn validate_spec_rejects_bad_yearly_months() {
        let err = validate_spec_json(
            "yearly",
            r#"{"time":"09:00","months":["janx"],"days_of_month":[1]}"#,
        )
        .expect_err("garbage month must be rejected");
        assert!(err.contains("invalid yearly month"), "err={err}");
        validate_spec_json("yearly", r#"{"months":["JAN","dec"],"days_of_month":[1]}"#)
            .expect("valid months in any case must pass");
    }

    #[test]
    fn validate_spec_rejects_out_of_range_days_of_month() {
        for freq in ["monthly", "yearly"] {
            assert!(
                validate_spec_json(freq, r#"{"days_of_month":[0]}"#).is_err(),
                "{freq}: day 0 must be rejected"
            );
            assert!(
                validate_spec_json(freq, r#"{"days_of_month":[32]}"#).is_err(),
                "{freq}: day 32 must be rejected"
            );
            validate_spec_json(freq, r#"{"days_of_month":[1,15,31]}"#)
                .expect("in-range days must pass");
        }
        // Singular legacy shape is policed too.
        assert!(validate_spec_json("monthly", r#"{"day_of_month":42}"#).is_err());
        validate_spec_json("monthly", r#"{"day_of_month":15}"#).unwrap();
    }

    #[test]
    fn validate_spec_tolerates_empty_spec_and_unknown_frequencies() {
        // Legacy rows / frequency-only edits send an empty spec.
        validate_spec_json("weekly", "").unwrap();
        validate_spec_json("daily", "{}").unwrap();
        // Unknown frequencies aren't this gate's business.
        validate_spec_json("hourly", r#"{"every_seconds":3600}"#).unwrap();
        // But non-empty garbage that isn't JSON fails loudly.
        assert!(validate_spec_json("weekly", "not-json").is_err());
    }
}
