//! H5 of Phase 4 — daemon-side `/cli/agents/launch` +
//! `/cli/agents/delegate`.
//!
//! Both endpoints used to live in Tauri's `agent_hooks.rs` and
//! called Tauri's `spawn_wake_pty` (which owns a
//! `TerminalManager::create` under the legacy alacritty path). H5
//! replaces the spawn side with daemon-owned Session Stream —
//! `spawn::spawn_agent_session` — so the new session shows up in
//! `session_map` and is reachable by every route that already
//! knows how to find daemon sessions (H1-H4).
//!
//! The heavy lifting (decision tree for launch, worktree + task
//! CLAUDE.md for delegate) is already in k2so-core:
//! - `k2_core::workspace::agent_launch::k2so_agents_build_launch`
//!   walks the three wake branches (resume active / delegate from
//!   inbox / fresh launch) and returns the launch JSON.
//! - `k2_core::deprecated::delegate::k2so_agents_delegate` creates
//!   the worktree + moves the inbox item + writes CLAUDE.md and
//!   returns the launch JSON.
//!
//! Each handler:
//!   1. Calls the core entry point to build the launch JSON.
//!   2. Parses `cwd`, `command`, `args` out of that JSON.
//!   3. Hands them to `spawn::spawn_agent_session` so the PTY is
//!      daemon-owned from the start (no Tauri `TerminalManager`).
//!   4. Emits the same HookEvent the Tauri path emitted so
//!      attached UIs see the same wire format.
//!   5. Returns JSON whose shape matches the legacy endpoints.

use std::collections::HashMap;

use serde::Deserialize;

use k2_core::agent_hooks::{emit, HookEvent};

use crate::cli_response::CliResponse;
use crate::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};

/// H6: spawn a wake PTY via the Session Stream pipeline (same
/// shape as `crate::wake_headless::spawn_wake_headless` but
/// daemon-owned — the resulting session lands in `session_map`
/// and is reachable by every /cli/* route that looks up by agent
/// name). Caller decides which backend to use based on the
/// project's `use_session_stream` setting.
///
/// Mirrors the side-effects of the legacy helper:
///   1. spawn_agent_session (PTY + dual-emit reader + archive).
///   2. Lock the agent in `agent_sessions` so scheduler skips it
///      on the next tick.
///   3. Emit `CliTerminalSpawnBackground` so any attached UI sees
///      the new session.
///
/// Returns the session id (as a String) on success.
// `heartbeat_name`: when Some, the wake is on behalf of a specific
// scheduled heartbeat. Per-heartbeat session save is currently
// handled by the v2 session-stream itself (the saved session_id is
// the v2 session UUID, not Claude's resume id), so this parameter
// is reserved for symmetry with `spawn_wake_headless` and a future
// hook that mirrors the per-heartbeat resume contract for v2 wakes.
// 0.37.0: retired from the heartbeat fire path. Every daemon-
// driven wake spawn now flows through `wake_headless::spawn_wake_headless`
// (v2). This function survives only as dead code reachable via the
// explicit `/cli/sessions/spawn` Kessel-T0 endpoint, which is opt-in
// for users who select Kessel as their renderer in settings.
#[allow(dead_code)]
pub fn spawn_wake_via_session_stream(
    agent_name: &str,
    project_path: &str,
    wake_prompt: &str,
    heartbeat_name: Option<&str>,
) -> Result<String, String> {
    // Pre-allocate Claude's session id (P6 fix). Without this, two
    // concurrent fires in the same project root attach to the same
    // claude session via implicit "continue most recent" behavior,
    // and both heartbeat rows end up stamped with the same id.
    // Pinning at spawn time gives each fire a deterministic, unique
    // session — see matching comment in `wake::spawn_wake_headless`.
    let pinned_session_id = uuid::Uuid::new_v4().to_string();

    // --print so claude delivers + exits (no lingering daemon PTY
    // that competes with the user's tab in find_live_for_resume).
    // See longer rationale in wake::spawn_wake_headless.
    let args = vec![
        "--dangerously-skip-permissions".to_string(),
        "--print".to_string(),
        "--session-id".to_string(),
        pinned_session_id.clone(),
        wake_prompt.to_string(),
    ];
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
    };
    let outcome = spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: agent_name.to_string(),
        project_id: project_id.clone(),
        cwd: project_path.to_string(),
        command: Some("claude".to_string()),
        args: Some(args),
        cols: 120,
        rows: 38,
        canonical_key: None,
    })?;

    let _ = k2_core::workspace::session::k2so_agents_lock(
        project_path.to_string(),
        agent_name.to_string(),
        Some(outcome.session_id.to_string()),
        Some("system".to_string()),
    );

    // Synchronous per-heartbeat session stamp.
    if let Some(hb_name) = heartbeat_name {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(project_id) =
            k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
        {
            let _ = k2_core::db::schema::AgentHeartbeat::save_session_id(
                &conn, &project_id, hb_name, &pinned_session_id,
            );
        }
    }

    emit(
        HookEvent::CliTerminalSpawnBackground,
        serde_json::json!({
            "terminalId": outcome.session_id.to_string(),
            "command": "claude",
            "cwd": project_path,
            "projectPath": project_path,
            "agentName": agent_name,
            "heartbeatName": heartbeat_name,
        }),
    );

    Ok(outcome.session_id.to_string())
}

/// Extract a top-level string field from a launch-info JSON object,
/// falling back to `default` if the field is missing or not a string.
fn str_field<'a>(v: &'a serde_json::Value, key: &str, default: &'a str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or(default)
}

/// Extract a top-level string-array field, turning each element into
/// an owned String. Returns an empty Vec if the field is absent or
/// not an array.
fn str_array(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Handler for `GET /cli/agents/launch?project=<path>&agent=<name>[&command=<cmd>]`.
///
/// Walks the three wake branches in core
/// (`k2so_agents_build_launch`) and spawns the resolved command in
/// the resolved `cwd` as a Session Stream session tagged with the
/// agent name. Emits `CliTerminalSpawnBackground` — matches the
/// legacy Tauri path so UI subscribers render the pane the same
/// way.
pub fn handle_agents_launch(
    params: &HashMap<String, String>,
    project_path: &str,
) -> CliResponse {
    let agent = params.get("agent").cloned().unwrap_or_default();
    if agent.is_empty() {
        return CliResponse::bad_request("missing agent param");
    }
    let cli_command = params.get("command").cloned().filter(|s| !s.is_empty());

    let launch_info = match k2_core::workspace::agent_launch::k2so_agents_build_launch(
        project_path.to_string(),
        agent.clone(),
        cli_command,
        None,
        None,
        None, // /cli/agents/launch is a manual launch — use the per-agent global session
    ) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("build_launch failed: {e}")),
    };

    let command = str_field(&launch_info, "command", "claude").to_string();
    let cwd = str_field(&launch_info, "cwd", project_path).to_string();
    let args = str_array(&launch_info, "args");

    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
    };
    let outcome = match spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: agent.clone(),
        project_id,
        cwd: cwd.clone(),
        command: Some(command.clone()),
        args: if args.is_empty() { None } else { Some(args) },
        cols: 120,
        rows: 38,
        canonical_key: None,
    }) {
        Ok(o) => o,
        Err(e) => return CliResponse::bad_request(format!("spawn failed: {e}")),
    };

    // Mark the session `running` in `agent_sessions` so the
    // scheduler skips the agent on subsequent ticks. Best-effort —
    // the PTY is already live and will keep running if the DB
    // write fails.
    let _ = k2_core::workspace::session::k2so_agents_lock(
        project_path.to_string(),
        agent.clone(),
        Some(outcome.session_id.to_string()),
        Some("system".to_string()),
    );

    // Observational event for any UI on the /events WS. Shape
    // matches what src-tauri's spawn_wake_pty emits today so the
    // frontend's listener doesn't need to branch on origin.
    emit(
        HookEvent::CliTerminalSpawnBackground,
        serde_json::json!({
            "terminalId": outcome.session_id.to_string(),
            "command": command,
            "cwd": cwd,
            "projectPath": project_path,
            "agentName": &agent,
        }),
    );

    CliResponse::ok_json(
        serde_json::json!({
            "success": true,
            "terminalId": outcome.session_id.to_string(),
            "agentName": agent,
            "pendingDrained": outcome.pending_drained,
            "note": "Agent session launched by daemon",
        })
        .to_string(),
    )
}

/// Handler for `GET /cli/agents/delegate?project=<path>&target=<agent>&file=<path>`.
///
/// Creates a fresh worktree + writes the task CLAUDE.md (via
/// `agents::delegate::k2so_agents_delegate`), then spawns `claude`
/// in the worktree as a Session Stream session tagged with the
/// target agent's name. Emits `CliTerminalSpawn` +
/// `SyncProjects` — the first opens a UI pane for the new
/// session; the second tells the sidebar a new worktree appeared.
pub fn handle_agents_delegate(
    params: &HashMap<String, String>,
    project_path: &str,
) -> CliResponse {
    let target = params.get("target").cloned().unwrap_or_default();
    let file = params.get("file").cloned().unwrap_or_default();
    if target.is_empty() {
        return CliResponse::bad_request("missing target param");
    }
    if file.is_empty() {
        return CliResponse::bad_request("missing file param");
    }

    let launch_info = match k2_core::deprecated::delegate::k2so_agents_delegate(
        project_path.to_string(),
        target.clone(),
        file.clone(),
    ) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("delegate failed: {e}")),
    };

    let command = str_field(&launch_info, "command", "claude").to_string();
    let cwd = str_field(&launch_info, "cwd", project_path).to_string();
    let agent_name = str_field(&launch_info, "agentName", &target).to_string();
    let args = str_array(&launch_info, "args");

    // Delegated agents run in worktree subdirs, not the parent
    // workspace. The delegated PTY isn't bound to the parent's
    // canonical-agent slot — it has its own identity. project_id
    // intentionally None so the registration uses the worktree-
    // unique agent_name as the slot key.
    let outcome = match spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: agent_name.clone(),
        project_id: None,
        cwd: cwd.clone(),
        command: Some(command.clone()),
        args: if args.is_empty() { None } else { Some(args) },
        cols: 120,
        rows: 38,
        canonical_key: None,
    }) {
        Ok(o) => o,
        Err(e) => return CliResponse::bad_request(format!("spawn failed: {e}")),
    };

    let _ = k2_core::workspace::session::k2so_agents_lock(
        project_path.to_string(),
        agent_name.clone(),
        Some(outcome.session_id.to_string()),
        Some("delegated".to_string()),
    );

    emit(
        HookEvent::CliTerminalSpawn,
        serde_json::json!({
            "terminalId": outcome.session_id.to_string(),
            "agentName": &agent_name,
            "command": command,
            "cwd": cwd,
            "projectPath": project_path,
        }),
    );
    // Tell the sidebar a new worktree was registered (delegate
    // adds a row to the `workspaces` table).
    emit(HookEvent::SyncProjects, serde_json::Value::Null);

    // Echo back every field the legacy endpoint returned so CLI
    // clients that read `branch`, `worktreePath`, `taskFile` etc.
    // keep working. Daemon-specific additions (`terminalId`,
    // `pendingDrained`) are inserted alongside.
    let mut out = launch_info.clone();
    if let Some(obj) = out.as_object_mut() {
        obj.insert(
            "terminalId".into(),
            serde_json::Value::String(outcome.session_id.to_string()),
        );
        obj.insert(
            "pendingDrained".into(),
            serde_json::Value::Number(outcome.pending_drained.into()),
        );
        obj.insert("success".into(), serde_json::Value::Bool(true));
    }
    CliResponse::ok_json(serde_json::to_string(&out).unwrap_or_else(|_| "{}".into()))
}

// ══════════════════════════════════════════════════════════════════════
// Task #578 extraction — agents-domain GET dispatch
// ══════════════════════════════════════════════════════════════════════
//
// These handlers were inline arms in `cli::dispatch`. They cover the
// `/cli/agents/*`, `/cli/agent/*`, and the agent-scoped triage /
// scheduler routes. Behavior is byte-for-byte preserved; only the code
// location moved. The shared param/respond helpers live in
// `crate::cli` (made `pub` for this extraction).

use crate::cli::{bool_param, need_project, opt_param, respond, respond_unit, str_param};

/// Agents-domain GET dispatch. Returns `Some(resp)` for a handled path,
/// `None` if the path isn't an agents-domain route (caller falls through).
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let resp = match path {
        // ── Read-only: agent metadata ────────────────────────────────
        "/cli/agents/list" => match need_project(params) {
            Ok(p) => respond(k2_core::workspace::agent::list(p)),
            Err(r) => r,
        },
        "/cli/agents/profile" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                match k2_core::workspace::agent::get_profile(p, agent) {
                    Ok(content) => CliResponse::ok_json(
                        serde_json::json!({ "content": content }).to_string(),
                    ),
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },
        // ── Read-only: workspace relations (host-aware list reads) ───
        // K2 Connect GAP: the renderer's "Connected Workspaces" panel
        // previously read relations via LOCAL Tauri invoke(), which
        // misfired against a remote host. These mirror the
        // `workspace_relations_list{,_incoming}(projectId)` Tauri commands
        // and return `WorkspaceRelation[]` (camelCase) verbatim.
        "/cli/relations/list" => handle_relations_list(str_param(params, "project_id")),
        "/cli/relations/list-incoming" => {
            handle_relations_list_incoming(str_param(params, "project_id"))
        }
        // 0.39.0f Phase 2.1b: `/cli/agents/work` retired → `/cli/inbox/list`.
        "/cli/agents/work" => CliResponse::gone(
            "agents/work route deprecated in Phase 2.1; use /cli/inbox/list — see `k2so help-deprecated`",
        ),
        // 0.39.0f Phase 2.1b: `/cli/work/inbox` retired → `/cli/inbox/list`.
        "/cli/work/inbox" => CliResponse::gone(
            "work/* routes deprecated in Phase 2.1; use /cli/inbox/* — see `k2so help-deprecated`",
        ),

        // ── State-mutating: agent CRUD ──────────────────────────────
        "/cli/agents/create" => match need_project(params) {
            Ok(p) => respond(k2_core::workspace::agent::create(
                p,
                str_param(params, "name"),
                str_param(params, "role"),
                opt_param(params, "prompt"),
                opt_param(params, "agent_type"),
            )),
            Err(r) => r,
        },
        "/cli/agents/delete" => match need_project(params) {
            Ok(p) => respond_unit(k2_core::workspace::agent::delete(
                p,
                str_param(params, "name"),
            )),
            Err(r) => r,
        },
        "/cli/agent/update" => match need_project(params) {
            Ok(p) => respond(k2_core::workspace::agent::update_field(
                p,
                str_param(params, "agent"),
                str_param(params, "field"),
                str_param(params, "value"),
            )
            .map(|content| serde_json::json!({ "success": true, "content": content }))),
            Err(r) => r,
        },

        // ── State-mutating: work queue ──────────────────────────────
        // 0.39.0f Phase 2.1b: `/cli/agents/work/*` retired → `/cli/inbox/*`.
        // Route entries kept so external callers get a clear HTTP-410 signal
        // (rather than a silent 404 from the catch-all). The body points
        // them at the new endpoint and `help-deprecated`.
        "/cli/agents/work/create" => CliResponse::gone(
            "work/* routes deprecated in Phase 2.1; use /cli/inbox/compose — see `k2so help-deprecated`",
        ),
        "/cli/agents/work/move" => CliResponse::gone(
            "work/* routes deprecated in Phase 2.1; use /cli/inbox/move — see `k2so help-deprecated`",
        ),
        // 0.39.0f Phase 2.1c: `/cli/work/inbox/create` retired →
        // POST /cli/inbox/compose?project=<target-workspace>. The
        // sole CLI caller (cmd_msg_inbox_form) was migrated in
        // Phase 2.1c; the Tauri-side caller `workspace_inbox_create`
        // and its daemon dependency `deliver_to_inbox` were deleted
        // in the Phase 2.1 wrap-up. Route entry kept as a 410-Gone
        // so any external straggler gets a clear signal.
        "/cli/work/inbox/create" => CliResponse::gone(
            "work/* routes deprecated in Phase 2.1; use /cli/inbox/compose with project=<target-workspace> — see `k2so help-deprecated`",
        ),

        // ── Agent lifecycle: lock + session ─────────────────────────
        "/cli/agents/lock" => match need_project(params) {
            Ok(p) => respond_unit(k2_core::workspace::session::k2so_agents_lock(
                p,
                str_param(params, "agent"),
                opt_param(params, "terminal_id"),
                opt_param(params, "owner"),
            )),
            Err(r) => r,
        },
        "/cli/agents/unlock" => match need_project(params) {
            Ok(p) => respond_unit(k2_core::workspace::session::k2so_agents_unlock(
                p,
                str_param(params, "agent"),
            )),
            Err(r) => r,
        },

        // ── Agent-hook channel events ───────────────────────────────
        "/cli/events" => match need_project(params) {
            Ok(p) => {
                // 0.39.0f: default the `agent` query param to the
                // workspace's primary agent name (resolved via
                // `find_primary_agent`) instead of the pre-unification
                // `__lead__` sentinel. The display-name fallback
                // catches workspaces where the primary hasn't been
                // fully scaffolded yet — `agent_display_name` is
                // total (always returns a string) so callers without
                // an explicit agent still get a routable identity.
                let agent = opt_param(params, "agent").unwrap_or_else(|| {
                    k2_core::workspace::agent_identity::find_primary_agent(&p)
                        .unwrap_or_else(|| k2_core::workspace::display::agent_display_name(&p))
                });
                let events = k2_core::workspace::events::drain_agent_events(&p, &agent);
                CliResponse::ok_json(
                    serde_json::to_string(&events).unwrap_or_else(|_| "[]".to_string()),
                )
            }
            Err(r) => r,
        },
        "/cli/agent/reply" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                let message = str_param(params, "message");
                k2_core::agent_hooks::emit(
                    k2_core::agent_hooks::HookEvent::AgentReply,
                    serde_json::json!({
                        "agentName": agent,
                        "message": message,
                        "projectPath": p,
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    }),
                );
                CliResponse::ok_json(r#"{"success":true}"#.to_string())
            }
            Err(r) => r,
        },

        // ── Per-agent heartbeat control ─────────────────────────────
        "/cli/agents/heartbeat" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                let interval = opt_param(params, "interval").and_then(|v| v.parse::<u64>().ok());
                let phase = opt_param(params, "phase");
                let mode = opt_param(params, "mode");
                let cost_budget = opt_param(params, "cost_budget");
                // If ANY mutation param is present → update; else → read.
                if interval.is_some()
                    || phase.is_some()
                    || mode.is_some()
                    || cost_budget.is_some()
                {
                    let force_wake = if params.contains_key("force_wake") {
                        Some(bool_param(params, "force_wake"))
                    } else {
                        None
                    };
                    respond(k2_core::heartbeats::control::set_heartbeat(
                        p,
                        agent,
                        interval,
                        phase,
                        mode,
                        cost_budget,
                        force_wake,
                    ))
                } else {
                    respond(k2_core::heartbeats::control::get_heartbeat(p, agent))
                }
            }
            Err(r) => r,
        },
        "/cli/agents/heartbeat/noop" => match need_project(params) {
            Ok(p) => respond(k2_core::heartbeats::control::heartbeat_noop(
                p,
                str_param(params, "agent"),
            )),
            Err(r) => r,
        },
        "/cli/agents/heartbeat/action" => match need_project(params) {
            Ok(p) => respond(k2_core::heartbeats::control::heartbeat_action(
                p,
                str_param(params, "agent"),
            )),
            Err(r) => r,
        },

        // ── Sub-agent completion ────────────────────────────────────
        "/cli/agent/complete" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                let file = str_param(params, "file");
                match k2_core::workspace::reviews::agent_complete(p, agent, file) {
                    Ok(body) => CliResponse::ok_json(body),
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },

        // ── Agent CLAUDE.md regen ───────────────────────────────────
        "/cli/agents/generate-claude-md" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                if agent.is_empty() {
                    return Some(CliResponse::bad_request("Missing 'agent' parameter"));
                }
                match k2_core::skills::content::generate_agent_claude_md_content(
                    &p, &agent, None,
                ) {
                    Ok(md) => {
                        let claude_md_path =
                            k2_core::workspace::agent_identity::agent_dir(&p, &agent).join("CLAUDE.md");
                        if let Err(e) =
                            k2_core::workspace::work_item::atomic_write(&claude_md_path, &md)
                        {
                            return Some(CliResponse::bad_request(e));
                        }
                        CliResponse::ok_json(
                            serde_json::json!({"success": true, "length": md.len()})
                                .to_string(),
                        )
                    }
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },

        // ── Agent launch + delegate (handlers above) ────────────────
        "/cli/agents/launch" => match need_project(params) {
            Ok(p) => handle_agents_launch(params, &p),
            Err(r) => r,
        },
        "/cli/agents/delegate" => match need_project(params) {
            Ok(p) => handle_agents_delegate(params, &p),
            Err(r) => r,
        },

        // ── Phase 4 H2: live-session enumeration ────────────────────
        "/cli/agents/running" => crate::terminal_routes::handle_agents_running(params),
        "/cli/agents/reap" => crate::terminal_routes::handle_agents_reap(params),

        // ── Scheduler / triage ──────────────────────────────────────
        // `/cli/agents/triage` is READ-ONLY (plain-text summary for
        // `k2so agents triage`). `/cli/scheduler-tick` is the
        // DESTRUCTIVE heartbeat fire path — `~/.k2so/heartbeat.sh`
        // invokes it on launchd's schedule and parses `"count":N`
        // to log what fired.
        "/cli/agents/triage" => match need_project(params) {
            Ok(p) => CliResponse::ok_text(crate::triage::handle_triage(&p)),
            Err(r) => r,
        },
        "/cli/scheduler-tick" => match need_project(params) {
            Ok(p) => CliResponse::ok_json(crate::triage::handle_scheduler_fire(&p)),
            Err(r) => r,
        },

        _ => return None,
    };
    Some(resp)
}

// ══════════════════════════════════════════════════════════════════════
// K2 Connect host-awareness GAP — POST routes
// ══════════════════════════════════════════════════════════════════════
//
// The renderer previously called the matching `k2so_agents_*` /
// `workspace_relations_*` / `k2so_session_set_surfaced` Tauri commands
// via LOCAL `invoke()`. Those run in-process against the LOCAL daemon's
// filesystem/DB, so when the renderer is driving a REMOTE host (K2
// Connect) the call misfires (wrong machine, or no Tauri backend). These
// POST routes give the renderer a host-aware HTTP surface that always
// targets the daemon it's actually talking to. Each wraps the SAME
// `k2_core` fn the Tauri command called, so local + remote stay
// identical.
//
// All are workspace-scoped (a `project_path` / `project_id` in the body),
// NOT owner-only — they're the same writes any logged-in user performs
// from the workspace UI, so they take the same auth as every other
// `/cli/*` data route (owner token OR connect-user session via
// `token_ok`). The dispatcher provides the POST method gate + token gate
// before this module sees the call.

/// Deserialize a JSON body, returning a `400` `CliResponse` on parse
/// failure. Empty bodies fall back to `Default` so a missing required
/// field surfaces as the handler's own "missing X" error rather than a
/// serde error.
fn parse_body<T: serde::de::DeserializeOwned + Default>(
    body: &[u8],
) -> Result<T, CliResponse> {
    if body.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_slice(body)
        .map_err(|e| CliResponse::bad_request(format!("invalid body: {e}")))
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ProjectPathBody {
    project_path: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct SaveAgentMdBody {
    project_path: String,
    agent_name: String,
    content: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct SaveSessionIdBody {
    project_path: String,
    agent_name: String,
    session_id: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct SetSurfacedBody {
    project_path: String,
    agent_name: String,
    surfaced: bool,
    terminal_id: Option<String>,
    command: Option<String>,
    args: Option<Vec<String>>,
    heartbeat_name: Option<String>,
    attach_agent_name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RelationCreateBody {
    source_project_id: String,
    target_project_id: String,
    relation_type: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RelationDeleteBody {
    id: String,
}

/// Handler for `POST /cli/agents/regenerate-workspace-skill`.
///
/// Wraps `k2_core::workspace::skill_regen::regenerate_workspace_skill`.
/// Returns the regenerated SKILL.md text as JSON. Mirrors the
/// `k2so_agents_regenerate_workspace_skill` Tauri command (4 renderer
/// callers).
pub fn handle_regenerate_workspace_skill(body: &[u8]) -> CliResponse {
    let b: ProjectPathBody = match parse_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    match k2_core::workspace::skill_regen::regenerate_workspace_skill(b.project_path) {
        Ok(skill) => {
            CliResponse::ok_json(serde_json::json!({ "skill": skill }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/agents/save-agent-md`.
///
/// Wraps `k2_core::workspace::agent_editor::k2so_agents_save_agent_md`.
/// Mirrors the `k2so_agents_save_agent_md` Tauri command.
///
/// DESIGN NOTE: a dedicated route (rather than reusing the generic
/// `/cli/fs/write-file`) is the correct choice — `k2so_agents_save_agent_md`
/// is NOT a plain byte write. The core fn resolves the canonical
/// `.k2so/agent/<agent>/AGENT.md` path from `(project_path, agent_name)`,
/// applies the same backup/validation the editor pipeline owns, and keeps
/// the harness mirror in sync. `/cli/fs/write-file` would require the
/// renderer to know + recompute that path itself (re-implementing core
/// logic on the client), which is exactly the host-awareness coupling we
/// are removing. So: dedicated route.
pub fn handle_save_agent_md(body: &[u8]) -> CliResponse {
    let b: SaveAgentMdBody = match parse_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    if b.agent_name.is_empty() {
        return CliResponse::bad_request("missing agent_name");
    }
    match k2_core::workspace::agent_editor::k2so_agents_save_agent_md(
        b.project_path,
        b.agent_name,
        b.content,
    ) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/agents/disable-workspace-claude-md`.
///
/// Wraps `k2_core::workspace::harness::disable_workspace_claude_md`.
/// Removes/disables the workspace SKILL.md + CLAUDE.md symlink. Mirrors
/// the `k2so_agents_disable_workspace_claude_md` Tauri command.
pub fn handle_disable_workspace_claude_md(body: &[u8]) -> CliResponse {
    let b: ProjectPathBody = match parse_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    match k2_core::workspace::harness::disable_workspace_claude_md(b.project_path) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/agents/run-workspace-ingest`.
///
/// Wraps `k2_core::workspace::harness::k2so_agents_run_workspace_ingest`.
/// Mirrors the `k2so_agents_run_workspace_ingest` Tauri command.
pub fn handle_run_workspace_ingest(body: &[u8]) -> CliResponse {
    let b: ProjectPathBody = match parse_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    match k2_core::workspace::harness::k2so_agents_run_workspace_ingest(b.project_path) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/agents/save-session-id`.
///
/// Wraps `k2_core::workspace::session::k2so_agents_save_session_id`.
/// Mirrors the `k2so_agents_save_session_id` Tauri command.
pub fn handle_save_session_id(body: &[u8]) -> CliResponse {
    let b: SaveSessionIdBody = match parse_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    if b.agent_name.is_empty() {
        return CliResponse::bad_request("missing agent_name");
    }
    match k2_core::workspace::session::k2so_agents_save_session_id(
        b.project_path,
        b.agent_name,
        b.session_id,
    ) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/session/set-surfaced`.
///
/// Wraps `k2_core::workspace::session::k2so_session_set_surfaced` (the
/// multi-arg surfaced-toggle; each arg is a body field). Mirrors the
/// `k2so_session_set_surfaced` Tauri command.
pub fn handle_session_set_surfaced(body: &[u8]) -> CliResponse {
    let b: SetSurfacedBody = match parse_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    if b.agent_name.is_empty() {
        return CliResponse::bad_request("missing agent_name");
    }
    match k2_core::workspace::session::k2so_session_set_surfaced(
        b.project_path,
        b.agent_name,
        b.surfaced,
        b.terminal_id,
        b.command,
        b.args,
        b.heartbeat_name,
        b.attach_agent_name,
    ) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/relations/create`.
///
/// Wraps `k2_core::workspace::relations::workspace_relations_create`.
/// Returns the created `WorkspaceRelation` as JSON. Mirrors the
/// `workspace_relations_create(sourceProjectId, targetProjectId,
/// relationType)` Tauri command.
///
/// DESIGN NOTE (FLAGGED): the existing `/cli/connections` route is
/// project-PATH + action-based (`?project=<path>&action=add&target=…`),
/// whereas the renderer's `workspace_relations_create` is project-ID
/// based and returns the full created row. Rather than reshape the
/// renderer onto the path/action API (higher-risk: different identity
/// model + different return shape), this adds an ID-based route that
/// directly mirrors the Tauri command 1:1 — the lower-risk option. The
/// path/action `/cli/connections` GET route is left untouched.
pub fn handle_relations_create(body: &[u8]) -> CliResponse {
    let b: RelationCreateBody = match parse_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.source_project_id.is_empty() {
        return CliResponse::bad_request("missing source_project_id");
    }
    if b.target_project_id.is_empty() {
        return CliResponse::bad_request("missing target_project_id");
    }
    match k2_core::workspace::relations::workspace_relations_create(
        b.source_project_id,
        b.target_project_id,
        b.relation_type,
    ) {
        Ok(rel) => CliResponse::ok_json(
            serde_json::to_string(&rel).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/relations/delete`.
///
/// Wraps `k2_core::workspace::relations::workspace_relations_delete`.
/// Mirrors the `workspace_relations_delete(id)` Tauri command. See the
/// FLAGGED design note on [`handle_relations_create`].
pub fn handle_relations_delete(body: &[u8]) -> CliResponse {
    let b: RelationDeleteBody = match parse_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.id.is_empty() {
        return CliResponse::bad_request("missing id");
    }
    match k2_core::workspace::relations::workspace_relations_delete(b.id) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `GET /cli/relations/list?project_id=…`.
///
/// Wraps `k2_core::workspace::relations::workspace_relations_list`.
/// Mirrors the `workspace_relations_list(projectId)` Tauri command —
/// returns the OUTGOING relations (rows where the project is the SOURCE)
/// as a JSON array of `WorkspaceRelation` (camelCase via the schema's
/// `serde(rename_all = "camelCase")`), the exact shape the renderer's
/// `WorkspaceRelation[]` deserializes. K2 Connect host-awareness GAP:
/// the renderer previously fired this via LOCAL Tauri invoke(), which
/// misfires when driving a remote host.
pub fn handle_relations_list(project_id: String) -> CliResponse {
    if project_id.is_empty() {
        return CliResponse::bad_request("missing project_id");
    }
    match k2_core::workspace::relations::workspace_relations_list(project_id) {
        Ok(rows) => CliResponse::ok_json(
            serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `GET /cli/relations/list-incoming?project_id=…`.
///
/// Wraps `k2_core::workspace::relations::workspace_relations_list_incoming`.
/// Mirrors the `workspace_relations_list_incoming(projectId)` Tauri
/// command — returns the INCOMING relations (rows where the project is
/// the TARGET) as a JSON `WorkspaceRelation[]`. See
/// [`handle_relations_list`] for the host-awareness GAP note.
pub fn handle_relations_list_incoming(project_id: String) -> CliResponse {
    if project_id.is_empty() {
        return CliResponse::bad_request("missing project_id");
    }
    match k2_core::workspace::relations::workspace_relations_list_incoming(project_id) {
        Ok(rows) => CliResponse::ok_json(
            serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[cfg(test)]
mod gap_route_tests {
    use super::*;

    #[test]
    fn regenerate_rejects_missing_project_path() {
        let r = handle_regenerate_workspace_skill(b"{}");
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("project_path"), "body={}", r.body);
    }

    #[test]
    fn save_agent_md_rejects_missing_agent_name() {
        let r = handle_save_agent_md(br#"{"project_path":"/tmp/x","content":"hi"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("agent_name"), "body={}", r.body);
    }

    #[test]
    fn save_agent_md_rejects_garbage_body() {
        let r = handle_save_agent_md(b"not json");
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("invalid body"), "body={}", r.body);
    }

    #[test]
    fn disable_workspace_claude_md_rejects_missing_project_path() {
        let r = handle_disable_workspace_claude_md(b"{}");
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("project_path"), "body={}", r.body);
    }

    #[test]
    fn run_workspace_ingest_rejects_missing_project_path() {
        let r = handle_run_workspace_ingest(b"{}");
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("project_path"), "body={}", r.body);
    }

    #[test]
    fn save_session_id_rejects_missing_agent_name() {
        let r = handle_save_session_id(br#"{"project_path":"/tmp/x","session_id":"s"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("agent_name"), "body={}", r.body);
    }

    #[test]
    fn set_surfaced_rejects_missing_agent_name() {
        let r = handle_session_set_surfaced(br#"{"project_path":"/tmp/x","surfaced":true}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("agent_name"), "body={}", r.body);
    }

    #[test]
    fn set_surfaced_parses_full_multiarg_body() {
        // Garbage project_path so the core call fails fast, but the body
        // (all 8 fields) must deserialize without a serde error first.
        let body = serde_json::json!({
            "project_path": "/nonexistent/k2so-set-surfaced-test",
            "agent_name": "agentX",
            "surfaced": true,
            "terminal_id": "tid-1",
            "command": "claude",
            "args": ["--print", "hi"],
            "heartbeat_name": "hb1",
            "attach_agent_name": "tab-1"
        })
        .to_string();
        let r = handle_session_set_surfaced(body.as_bytes());
        // Must NOT be the serde "invalid body" 400 — the body parsed.
        assert!(
            !r.body.contains("invalid body"),
            "multi-arg body should deserialize cleanly; body={}",
            r.body
        );
    }

    #[test]
    fn relations_create_rejects_missing_target() {
        let r = handle_relations_create(br#"{"source_project_id":"a"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("target_project_id"), "body={}", r.body);
    }

    #[test]
    fn relations_delete_rejects_missing_id() {
        let r = handle_relations_delete(b"{}");
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("id"), "body={}", r.body);
    }

    #[test]
    fn relations_list_rejects_missing_project_id() {
        let r = handle_relations_list(String::new());
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("project_id"), "body={}", r.body);
    }

    #[test]
    fn relations_list_incoming_rejects_missing_project_id() {
        let r = handle_relations_list_incoming(String::new());
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("project_id"), "body={}", r.body);
    }

    #[test]
    fn relations_list_routes_dispatch_as_get() {
        // Both list routes must be reachable via the agents-domain GET
        // dispatch (not 404 / None). A project with no relations returns
        // an empty JSON array — the exact `WorkspaceRelation[]` shape the
        // renderer deserializes.
        let mut params = std::collections::HashMap::new();
        params.insert(
            "project_id".to_string(),
            "k2so-relations-dispatch-test-nonexistent".to_string(),
        );
        let out = dispatch("/cli/relations/list", &params)
            .expect("/cli/relations/list must be handled by agents dispatch");
        assert_eq!(out.status, "200 OK", "body={}", out.body);
        assert_eq!(out.body, "[]", "body={}", out.body);

        let inc = dispatch("/cli/relations/list-incoming", &params)
            .expect("/cli/relations/list-incoming must be handled by agents dispatch");
        assert_eq!(inc.status, "200 OK", "body={}", inc.body);
        assert_eq!(inc.body, "[]", "body={}", inc.body);
    }
}
