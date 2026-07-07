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

    // Agent-degeneralization S2: resolve the workspace/global default
    // agent instead of hardcoding claude.
    let (resolved, project_id) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (
            k2_core::workspace::agent_resolve::resolve_agent_command(&conn, project_path),
            k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path),
        )
    };

    // --print so claude delivers + exits (no lingering daemon PTY
    // that competes with the user's tab in find_live_for_resume).
    // See longer rationale in wake::spawn_wake_headless.
    //
    // Slice 3b: `--print` + the positional prompt stay Claude grammar
    // (byte-identical argv). A non-claude default routes through the
    // ProviderResume adapter: premint providers (grok) pin the
    // pre-allocated uuid with their own flag; self-minting providers
    // spawn bare and adopt the discovered id post-hoc; unknown
    // providers spawn bare with no invented flags. The wake prompt is
    // still only deliverable positionally (claude) on this legacy
    // path — non-claude spawns receive no prompt (Kessel-T0 opt-in
    // surface; per-agent prompt delivery is Slice 5).
    let adapter = k2_core::workspace::provider_resume::provider_resume_for_command(
        &resolved.command,
    );
    let (args, session_id_pinned) = if resolved.is_claude() {
        let mut args = resolved.args.clone();
        k2_core::workspace::agent_resolve::ensure_flag(
            &mut args,
            "--dangerously-skip-permissions",
        );
        args.extend([
            "--print".to_string(),
            "--session-id".to_string(),
            pinned_session_id.clone(),
            wake_prompt.to_string(),
        ]);
        (args, true)
    } else {
        match adapter.and_then(|a| a.premint_args(&resolved.args, &pinned_session_id)) {
            Some(args) => (args, true),
            None => (resolved.args.clone(), false),
        }
    };
    let outcome = spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: agent_name.to_string(),
        project_id: project_id.clone(),
        cwd: project_path.to_string(),
        command: Some(resolved.command.clone()),
        args: Some(args),
        cols: 120,
        rows: 38,
        canonical_key: None,
        // W2: the resolved preset's migration-0070 env.
        env: resolved.env_map(),
    })?;

    let _ = k2_core::workspace::session::k2so_agents_lock(
        project_path.to_string(),
        agent_name.to_string(),
        Some(outcome.session_id.to_string()),
        Some("system".to_string()),
    );

    // Synchronous per-heartbeat session stamp. Slice 3b: the pinned id
    // was only actually passed when a premint flag carried it (claude
    // `--session-id`, grok `--session-id`); don't stamp a ghost id for
    // a bare self-minting spawn.
    if let Some(hb_name) = heartbeat_name {
        if session_id_pinned {
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
    }

    // Slice 3b: self-minting providers (pi/codex/gemini/cursor) get
    // their on-disk id adopted into the workspace_sessions SSOT a beat
    // after spawn. No-op for premint providers (id already pinned) and
    // unknown commands (nothing to discover).
    if !session_id_pinned {
        if let Some(a) = adapter {
            k2_core::workspace::provider_resume::defer_adopt_discovered_session(
                a.provider.to_string(),
                project_path.to_string(),
            );
        }
    }

    emit(
        HookEvent::CliTerminalSpawnBackground,
        serde_json::json!({
            "terminalId": outcome.session_id.to_string(),
            "command": resolved.command.as_str(),
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

    // Agent-degeneralization S2: only when the caller passed NO explicit
    // command, resolve the workspace/global default agent
    // (projects.default_agent → AppSettings.default_agent → claude)
    // instead of letting build_launch default to a hardcoded claude.
    // An explicit `command` param keeps exact legacy behavior.
    let resolved = if cli_command.is_none() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        Some(k2_core::workspace::agent_resolve::resolve_agent_command(
            &conn,
            project_path,
        ))
    } else {
        None
    };
    let effective_command =
        cli_command.or_else(|| resolved.as_ref().map(|r| r.command.clone()));

    let launch_info = match k2_core::workspace::agent_launch::k2so_agents_build_launch(
        project_path.to_string(),
        agent.clone(),
        effective_command,
        None,
        None,
        None, // /cli/agents/launch is a manual launch — use the per-agent global session
    ) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("build_launch failed: {e}")),
    };

    let mut command = str_field(&launch_info, "command", "claude").to_string();
    let mut cwd = str_field(&launch_info, "cwd", project_path).to_string();
    let mut args = str_array(&launch_info, "args");
    // W2: the resolved preset's migration-0070 env rides into the child
    // env. An explicit `command` param resolved nothing → no preset env
    // (legacy behavior). Values never logged.
    let mut spawn_env: std::collections::HashMap<String, String> = resolved
        .as_ref()
        .map(|r| r.env_map())
        .unwrap_or_default();

    // Slice 3b: build_launch composes Claude flag grammar
    // (--dangerously-skip-permissions / --append-system-prompt /
    // --resume / --fork-session + a positional wake message) — kept
    // byte-identical for a claude default (and for an explicit
    // `command` param). When the RESOLVED default is NOT claude, the
    // launch routes through the canonical resume resolver instead: the
    // stored harness / workspace default picks the agent, the
    // ProviderResume adapter supplies its resume/premint grammar, and
    // self-minting providers spawn bare with post-hoc id adoption. A
    // provider unknown to the adapter table still spawns the preset's
    // own command+args bare (no invented flags). The kickoff prompt
    // stays claude-only (per-agent prompt delivery is Slice 5).
    if let Some(r) = resolved.as_ref().filter(|r| !r.is_claude()) {
        let routed = k2_core::workspace::provider_resume::provider_resume_for_command(
            &r.command,
        )
        .and_then(|_| {
            k2_core::workspace::resume_chat::resolve_resume_chat_args(project_path).ok()
        });
        match routed {
            Some(rr) => {
                command = rr.command.clone();
                args = rr.args.clone();
                cwd = rr.cwd.clone();
                // W2: the resume resolver picked the governing preset
                // (the stored harness can differ from the workspace
                // default) — carry ITS env, not the outrun default's.
                spawn_env = rr
                    .env
                    .clone()
                    .map(|m| m.into_iter().collect())
                    .unwrap_or_default();
                if rr.pending_session_discovery {
                    k2_core::workspace::provider_resume::defer_adopt_discovered_session(
                        rr.provider.clone(),
                        project_path.to_string(),
                    );
                }
            }
            None => {
                // Unknown provider (or resolver error): bare preset
                // spawn in the branch-selected cwd — Slice-2 parity.
                command = r.command.clone();
                args = r.args.clone();
            }
        }
    }

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
        env: spawn_env,
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

    #[allow(deprecated)] // deprecated-but-live delegate seam (Phase 2.1 PRD A23)
    let launch_info = match k2_core::deprecated::delegate::k2so_agents_delegate(
        project_path.to_string(),
        target.clone(),
        file.clone(),
    ) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("delegate failed: {e}")),
    };

    let cwd = str_field(&launch_info, "cwd", project_path).to_string();
    let agent_name = str_field(&launch_info, "agentName", &target).to_string();
    let mut args = str_array(&launch_info, "args");

    // Agent-degeneralization S2: `k2so_agents_delegate` stamps
    // `command: "claude"` today, so this fallback is defensive-only —
    // but if the delegate JSON ever omits the command, the
    // workspace/global default agent fills it instead of a bare
    // literal. Slice 3b judgment call: a delegated worktree is a FRESH,
    // unregistered workspace (project_id deliberately None below) with
    // nothing to resume and no workspace_sessions row to premint/adopt
    // into — so a non-claude resolved default deliberately spawns the
    // preset's own command+args bare (no invented flags, no task-prompt
    // injection; delegate kickoff grammar per agent is Slice 5).
    // W2: the resolved preset's migration-0070 env (only the resolved-
    // default arm has a preset; an explicit AGENT.md command has none).
    let mut spawn_env: std::collections::HashMap<String, String> = Default::default();
    let command = match launch_info.get("command").and_then(|v| v.as_str()) {
        Some(c) => c.to_string(),
        None => {
            let resolved = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::workspace::agent_resolve::resolve_agent_command(
                    &conn,
                    project_path,
                )
            };
            if !resolved.is_claude() {
                args = resolved.args.clone();
            }
            spawn_env = resolved.env_map();
            resolved.command
        }
    };

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
        env: spawn_env,
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

use crate::cli::{need_project, opt_param, respond, respond_unit, str_param};

/// Agents-domain GET dispatch. Returns `Some(resp)` for a handled path,
/// `None` if the path isn't an agents-domain route (caller falls through).
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let resp = match path {
        // ── Read-only: agent metadata ────────────────────────────────
        "/cli/agents/list" => match need_project(params) {
            Ok(p) => respond(k2_core::workspace::agent::list(p)),
            Err(r) => r,
        },
        // 0.40.24 S2 — the full per-agent settings read backing
        // `k2 agent conf/get/set`. `q` is a workspace token (name |
        // absolute path | UUID); unknown tokens 404 with a stable
        // `not_found` code + did-you-mean hint (the CLI maps that to
        // exit 4 per the agent-CLI contract).
        "/cli/agent/conf" => {
            let q = str_param(params, "q");
            if q.is_empty() {
                return Some(CliResponse::bad_request(
                    "Missing q (agent name | path | UUID)",
                ));
            }
            handle_agent_conf(&q)
        }
        // 0.40.24 S3 — the fleet view backing `k2 agent list`: every
        // registered agent (workspace == agent) with its mode, enabled
        // bit, live-session status, and path.
        "/cli/agent/list" => handle_agent_list(),
        // 0.40.24 S4 — retire is POST-only (it mutates rows + moves the
        // folder). A GET landing here via the read dispatch chain gets
        // an explicit 405 (feedback_post_only_route_guards).
        "/cli/agent/retire" => CliResponse::method_not_allowed(),
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
        // 0.40.31: the legacy per-agent adaptive-backoff heartbeat API
        // (`<agent>/heartbeat.json`, unread since the custom-agent
        // scheduler loop retired in 0.39.0d) is deleted. Route entries
        // kept as HTTP-410 so any straggler (stale generated CLAUDE.md
        // instructions, old scripts) gets a clear signal instead of a
        // silent 404 from the catch-all.
        "/cli/agents/heartbeat"
        | "/cli/agents/heartbeat/noop"
        | "/cli/agents/heartbeat/action" => CliResponse::gone(
            "per-agent heartbeat control deleted in 0.40.31; use the named workspace \
             heartbeat schedules instead — `k2 heartbeat schedule list` / the \
             `/cli/heartbeat/*` routes",
        ),

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
/// Handler for `GET /cli/agent/conf?q=<name|path|uuid>` (0.40.24 S2).
///
/// Returns the FULL per-agent settings object the `k2 agent conf`
/// mockup renders:
/// `{ok, name, path, mode, enabled, personaPath, connections[],
/// projects[], live}`.
///
/// - `mode` is display-normalized (stored `k2so`/`agent` → CLI-canonical
///   `k2`, see `settings::display_agent_mode`) so operators only ever
///   see the documented vocabulary.
/// - `connections` are `{peer, bidirectional, remote}` — local edges
///   are always mutual (one relation row per pair IS bidirectional
///   awareness, migration 0051); only cross-daemon edges carry
///   `bidirectional: false` + `remote: true`.
/// - `projects` are the workspace's project-group memberships,
///   `{id, name, isPoc}` ordered by name (Projects V1 — powers
///   `k2 agent hire --project` / `set --add-project` plan probes and
///   `k2 agent get <name> projects`).
/// - `live` reports the workspace's canonical session:
///   `{active, sessionId?, uptimeSec: null}` (uptime tracking is not
///   plumbed through `DaemonPtySession` yet; the key is emitted for
///   shape stability).
pub fn handle_agent_conf(q: &str) -> CliResponse {
    let Some(path) = crate::workspace_msg::resolve_workspace(q) else {
        return crate::workspace_routes::workspace_not_found_response(q);
    };

    // projects row: id (for the membership lookup) + raw mode +
    // enabled bit.
    let (workspace_id, mode_raw, enabled) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match conn.query_row(
            "SELECT id, agent_mode, agent_enabled FROM projects WHERE path = ?1",
            rusqlite::params![path],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?.unwrap_or_else(|| "off".to_string()),
                    r.get::<_, Option<i64>>(2)?.unwrap_or(0) == 1,
                ))
            },
        ) {
            Ok(v) => v,
            // resolve_workspace just matched this path; a miss here is
            // a mid-request removal — treat as not found.
            Err(_) => return crate::workspace_routes::workspace_not_found_response(q),
        }
    };

    // Display name (AGENT.md frontmatter display_name → name →
    // projects.name — the existing renamable-for-display invariant).
    let name = k2_core::workspace::display::agent_display_name(&path);

    // Charter file (persona). Canonical post-0.37.0 location only —
    // conf reports what `k2 agent set --persona` writes.
    let persona = k2_core::workspace_dot_dir(&path).join("agent/AGENT.md");
    let persona_path = if persona.exists() {
        serde_json::json!(persona.to_string_lossy())
    } else {
        serde_json::Value::Null
    };

    let connections = match k2_core::connections::list_conf_peers(&path) {
        Ok(list) => serde_json::to_value(list).unwrap_or_else(|_| serde_json::json!([])),
        Err(_) => serde_json::json!([]),
    };

    // Project-group memberships, {id, name, isPoc} ordered by name
    // (degrades to [] on a read error — the connections idiom above).
    let projects = match k2_core::project_groups::memberships_for_workspace(&workspace_id) {
        Ok(list) => serde_json::Value::Array(
            list.iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m.group_id,
                        "name": m.group_name,
                        "isPoc": m.is_poc,
                    })
                })
                .collect(),
        ),
        Err(_) => serde_json::json!([]),
    };

    // Canonical live session (same lookup ensure_canonical_session
    // uses for its single-flight check).
    let live = crate::canonical_session::lookup_project_id(&path)
        .and_then(|pid| {
            crate::session_lookup::lookup_any(&crate::canonical_session::canonical_key_for(&pid))
        })
        .filter(|s| s.is_child_alive())
        .map(|s| {
            serde_json::json!({
                "active": true,
                "sessionId": s.session_id().to_string(),
                "uptimeSec": serde_json::Value::Null,
            })
        })
        .unwrap_or_else(|| serde_json::json!({ "active": false }));

    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "name": name,
            "path": path,
            "mode": k2_core::workspace::settings::display_agent_mode(&mode_raw),
            "enabled": enabled,
            "personaPath": persona_path,
            "connections": connections,
            "projects": projects,
            "live": live,
        })
        .to_string(),
    )
}

/// Handler for `GET /cli/agent/list` (0.40.24 S3).
///
/// The fleet view backing `k2 agent list`: one entry per registered
/// workspace (workspace == agent), shaped
/// `{ok, agents: [{name, mode, enabled, live, path}]}` and sorted by
/// name (case-insensitive) for stable rendering. `mode` is
/// display-normalized (stored `k2so`/`agent` → `k2`) and `live` uses
/// the SAME canonical-session source as `handle_agent_conf`'s `live`
/// object, so `list` and `conf` can never disagree about liveness.
pub fn handle_agent_list() -> CliResponse {
    // Snapshot the projects table, then do the per-row display-name
    // (AGENT.md read) + live lookups WITHOUT holding the DB lock —
    // `agent_display_name` takes its own locks.
    let rows: Vec<(String, String, String, bool)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut stmt = match conn.prepare(
            "SELECT id, path, COALESCE(agent_mode, 'off'), COALESCE(agent_enabled, 0) \
             FROM projects",
        ) {
            Ok(s) => s,
            Err(e) => return CliResponse::bad_request(format!("projects query failed: {e}")),
        };
        let mapped = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)? == 1,
            ))
        });
        match mapped {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => return CliResponse::bad_request(format!("projects query failed: {e}")),
        }
    };

    let mut agents: Vec<serde_json::Value> = rows
        .into_iter()
        // The audit sentinels (`_orphan`, `_broadcast`) are internal
        // activity-feed FK rows, not agents — same filter as
        // `projects_list()`.
        .filter(|(id, _, _, _)| !k2_core::db::AUDIT_SENTINEL_IDS.contains(&id.as_str()))
        .map(|(id, path, mode_raw, enabled)| {
            let name = k2_core::workspace::display::agent_display_name(&path);
            let live = crate::session_lookup::lookup_any(
                &crate::canonical_session::canonical_key_for(&id),
            )
            .filter(|s| s.is_child_alive())
            .is_some();
            serde_json::json!({
                "name": name,
                "mode": k2_core::workspace::settings::display_agent_mode(&mode_raw),
                "enabled": enabled,
                "live": live,
                "path": path,
            })
        })
        .collect();
    agents.sort_by_key(|a| {
        a["name"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase()
    });

    CliResponse::ok_json(
        serde_json::json!({ "ok": true, "agents": agents }).to_string(),
    )
}

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

    /// 0.40.24 S3 — `/cli/agent/list` returns every registered agent
    /// (display-normalized mode, enabled bool, live flag, path), sorted
    /// case-insensitively, and NEVER leaks the internal audit sentinels
    /// (`_orphan`/`_broadcast` — activity-feed FK rows, not agents).
    #[test]
    fn agent_list_returns_fleet_and_filters_audit_sentinels() {
        let id = uuid::Uuid::new_v4();
        let name = format!("agent-list-test-{id}");
        let path = format!("/tmp/agent-list-test-{}-{id}", std::process::id());
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, agent_mode, agent_enabled) \
                 VALUES (?1, ?2, ?3, 'k2so', 1)",
                rusqlite::params![id.to_string(), name, path],
            )
            .expect("insert project row");
        }

        let resp = handle_agent_list();
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let body: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(body["ok"], true);
        let agents = body["agents"].as_array().expect("agents array");

        // Our row is present with the DISPLAY mode spelling (k2so → k2).
        let mine = agents
            .iter()
            .find(|a| a["path"] == path.as_str())
            .unwrap_or_else(|| panic!("registered agent missing from list: {}", resp.body));
        assert_eq!(mine["name"], name.as_str());
        assert_eq!(mine["mode"], "k2", "stored k2so must display as k2");
        assert_eq!(mine["enabled"], true);
        assert_eq!(mine["live"], false);

        // The audit sentinels never surface.
        assert!(
            agents.iter().all(|a| a["path"] != "_orphan" && a["path"] != "_broadcast"),
            "audit sentinels leaked into the fleet view: {}",
            resp.body
        );
    }

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
