//! Daemon-side `/cli/workspace/*` + `/cli/checkin` route handlers
//! (task #578 extraction).
//!
//! These were inline arms in `cli::dispatch`. They cover workspace
//! lifecycle (create / open / cleanup / remove), the pinned-chat
//! session resolvers, the agent display-name read/write pair, the
//! canonical-session ensurance route, workspace token resolution, the
//! live `msg` delivery endpoint, and the aggregated agent check-in.
//!
//! Behavior is byte-for-byte preserved; only the code location moved.
//! Shared param/respond helpers live in `crate::cli`.

use std::collections::HashMap;

use crate::cli::{need_project, opt_param, str_param};
use crate::cli_response::CliResponse;

/// Workspace-domain GET dispatch. Returns `Some(resp)` for a handled
/// path, `None` if the path isn't a workspace-domain route.
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let resp = match path {
        // ── Aggregated agent check-in ───────────────────────────────
        "/cli/checkin" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                match k2_core::workspace::checkin::checkin(&p, &agent) {
                    Ok(body) => CliResponse::ok_json(body),
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },

        // ── Workspace lifecycle ─────────────────────────────────────
        "/cli/workspace/create" => {
            let target = str_param(params, "path");
            match k2_core::workspace::lifecycle::create_workspace(&target) {
                Ok(body) => {
                    // 0.39.45 (GH #18/#26): broadcast so every client
                    // re-fetches its project list — CLI-created
                    // workspaces used to stay invisible until a manual
                    // window reload.
                    let _ = crate::session_events::emit(
                        crate::session_events::SessionEvent::ProjectsChanged {},
                    );
                    CliResponse::ok_json(body)
                }
                Err(e) => CliResponse::bad_request(e),
            }
        }
        "/cli/workspace/open" => {
            let target = str_param(params, "path");
            match k2_core::workspace::lifecycle::open_workspace(&target) {
                Ok(body) => CliResponse::ok_json(body),
                Err(e) => CliResponse::bad_request(e),
            }
        }
        "/cli/workspace/cleanup" => {
            match k2_core::workspace::lifecycle::cleanup_stale_workspaces() {
                Ok(body) => CliResponse::ok_json(body),
                Err(e) => CliResponse::bad_request(e),
            }
        }

        // 0.37.5: resolve the resume-chat launch args for a thin
        // client opening / refreshing a workspace's pinned chat tab.
        // Returns the canonical `claude` command + args, either:
        //   - `claude --resume <id>` when workspace_sessions.session_id
        //     points at an on-disk JSONL (continues the existing
        //     conversation), or
        //   - `claude --session-id <new>` after pre-allocating a fresh
        //     UUID and persisting it to SQL (next refresh resumes).
        //
        // Daemon-first: every thin client (Tauri pinned tab, mobile
        // companion, future MCP, CLI) hits this route. No client
        // duplicates the SQL lookup + JSONL existence check + fresh
        // pre-allocate logic — it lives in `k2_core::workspace::resume_chat`
        // and is callable purely.
        "/cli/workspace/resume-chat-args" => match need_project(params) {
            Ok(p) => match k2_core::workspace::resume_chat::resolve_resume_chat_args(&p) {
                Ok(out) => CliResponse::ok_json(out.to_json().to_string()),
                Err(e) => CliResponse::bad_request(e),
            },
            Err(r) => r,
        },

        // 0.37.12 — set the pinned chat tab's canonical Claude session
        // id for a workspace. Powers AgentChatPane's chat-history
        // dropdown (escape hatch for orphaned/deleted sessions).
        // After this returns, the renderer is expected to refresh
        // the pinned chat (close v2 session by canonical agent_name,
        // re-mount AgentChatPane) so the live PTY swaps.
        //
        // Query params:
        //   project=<workspace-path>
        //   session_id=<claude-uuid-to-set>
        "/cli/workspace/set-chat-session" => match need_project(params) {
            Ok(p) => {
                let session_id = str_param(params, "session_id");
                if session_id.is_empty() {
                    return Some(CliResponse::bad_request("Missing session_id parameter"));
                }
                let db = k2_core::db::shared();
                let conn = db.lock();
                let project_id = match k2_core::workspace::agent_identity::resolve_project_id(&conn, &p) {
                    Some(pid) => pid,
                    None => return Some(CliResponse::bad_request(
                        format!("project not registered: {p}"),
                    )),
                };
                match k2_core::db::schema::WorkspaceSession::update_session_id(
                    &conn, &project_id, &session_id,
                ) {
                    Ok(rows) => CliResponse::ok_json(
                        serde_json::json!({
                            "success": true,
                            "projectId": project_id,
                            "sessionId": session_id,
                            "rowsUpdated": rows,
                        }).to_string(),
                    ),
                    Err(e) => CliResponse::bad_request(format!("update failed: {e}")),
                }
            }
            Err(r) => r,
        },

        // 0.37.4: read the workspace's primary-agent display name.
        // Reads `.k2so/agent/AGENT.md` frontmatter — first
        // `display_name:` (the user-editable label), then `name:`
        // (the technical agent name), then `projects.name`. Total —
        // always returns a string. Mtime-cached, so render-path
        // callers can hit this freely.
        "/cli/workspace/agent-display-name" => match need_project(params) {
            Ok(p) => CliResponse::ok_json(
                serde_json::json!({
                    "display_name": k2_core::workspace::display::agent_display_name(&p),
                })
                .to_string(),
            ),
            Err(r) => r,
        },

        // 0.37.4: write the workspace's primary-agent display name.
        // Atomically rewrites AGENT.md frontmatter `display_name:`,
        // creating the file with a stub frontmatter if absent. Does
        // NOT touch the technical agent name (`name:` field), the
        // `v2_session_map` keys, or `workspace_sessions.terminal_id`
        // — those stay stable so live PTYs aren't dropped. Emits
        // `SyncProjects` so subscribed renderer surfaces re-fetch.
        //
        // Phase B: also updates the live canonical session's label
        // (if one exists) so the change propagates to subscribed
        // tabs in real time without a renderer round-trip — the
        // canonical session was spawned with `LabelSource::Locked`,
        // so PTY title events can't undo this.
        "/cli/workspace/set-agent-display-name" => match need_project(params) {
            Ok(p) => {
                let new_name = str_param(params, "name");
                if new_name.is_empty() {
                    return Some(CliResponse::bad_request("Missing name"));
                }
                match k2_core::workspace::display::set_agent_display_name(&p, &new_name) {
                    Ok(()) => {
                        // Phase B: live-session label propagation.
                        // The canonical workspace+agent session is
                        // keyed `<project_id>:<agent_name>` in
                        // v2_session_map. Look it up via the
                        // primary-agent helper + the project_id
                        // resolver and push the new label.
                        let project_id_opt = {
                            let db = k2_core::db::shared();
                            let conn = db.lock();
                            conn.query_row(
                                "SELECT id FROM projects WHERE path = ?1",
                                rusqlite::params![p],
                                |r| r.get::<_, String>(0),
                            )
                            .ok()
                        };
                        if let Some(project_id) = project_id_opt {
                            if let Some(agent_name) =
                                k2_core::workspace::agent_identity::resolve_agent_name(&p)
                            {
                                let canonical_key = format!("{project_id}:{agent_name}");
                                if let Some(session) =
                                    crate::v2_session_map::lookup_by_agent_name(&canonical_key)
                                {
                                    session.set_label(new_name.clone(), true);
                                }
                            }
                        }
                        k2_core::agent_hooks::emit(
                            k2_core::agent_hooks::HookEvent::SyncProjects,
                            serde_json::Value::Null,
                        );
                        CliResponse::ok_json(
                            serde_json::json!({
                                "success": true,
                                "display_name": new_name,
                            })
                            .to_string(),
                        )
                    }
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },

        // 0.37.2: explicit caller-driven canonical-session ensurance.
        // Replaces the SMS-bridge `agents launch <name>` workaround
        // — semantically correct, returns the canonical IDs the
        // caller can use for follow-up inject/wake. Idempotent: if
        // canonical session is already alive, returns reused=true
        // with the existing IDs. See `canonical_session.rs` module
        // doc for the full flow + the race it solves.
        "/cli/workspace/ensure-canonical-session" => match need_project(params) {
            Ok(p) => match crate::canonical_session::ensure_canonical_session(&p) {
                Ok(out) => CliResponse::ok_json(
                    serde_json::json!({
                        "success": true,
                        "session_id": out.session_id,
                        "agent": out.agent_name,
                        "project_id": out.project_id,
                        "reused": out.reused,
                        "pending_drained": out.pending_drained,
                    })
                    .to_string(),
                ),
                Err(e) => CliResponse::bad_request(e),
            },
            Err(r) => r,
        },

        // 0.37.0 simplified messaging — `k2so msg <workspace> "text"`.
        // Resolves a workspace token (name | absolute path | UUID) to
        // the canonical project path. Used by the CLI to detect whether
        // a `msg` first-arg is a workspace (new flow) or an agent name
        // (legacy flow + deprecation warning).
        "/cli/workspace/resolve" => {
            // Param name: `q` (query). Avoids collision with the auth
            // token URL param (`token=<auth>`) the cli_request helper
            // injects on every call.
            let q = str_param(params, "q");
            if q.is_empty() {
                return Some(CliResponse::bad_request("Missing q (workspace name | path | UUID)"));
            }
            match crate::workspace_msg::resolve_workspace(&q) {
                Some(path) => CliResponse::ok_json(
                    serde_json::json!({ "path": path }).to_string(),
                ),
                None => CliResponse::bad_request(format!("workspace not found: {q}")),
            }
        }
        // 0.38.6: `k2so msg <workspace>` is strictly live-or-fail.
        // The endpoint accepts a workspace token (name | path | UUID),
        // the message text, and a `from` sender identity (auto-derived
        // by the CLI from the sender's workspace; defaults to "external"
        // if empty). Returns the canonical [`MsgResponse`] JSON shape.
        //
        // The pre-0.38.6 `delivery=inbox` branch is retired — `msg`
        // never silently writes to inbox now. Callers that want queued
        // delivery use `k2so work send` (separate endpoint).
        "/cli/workspace/msg" => {
            let workspace = str_param(params, "workspace");
            let text = str_param(params, "text");
            let from = opt_param(params, "from").unwrap_or_default();
            // 0.39.25: optional slash-command prepended at the very front
            // of the delivered payload (before the `[from <name>]`
            // prefix). Empty/absent → unchanged delivery. An older CLI
            // simply never sends this param.
            let command = opt_param(params, "command").unwrap_or_default();
            // Issue #9 — wake gating. `wake=true` auto-wakes a dormant
            // canonical session (resume/spawn → wait READY → deliver).
            // DEFAULT ON: this preserves the LEGACY wake-on-message UX
            // (PRD D7 — bare `k2 msg`, party→agent companion sends, and
            // agent→agent pings all wake a sleeping canonical session) and
            // matches Rosson's directive that "msg should wake." Callers
            // opt OUT explicitly with `wake=false` (`k2 msg --no-wake`).
            // The state machine still distinguishes no-agent (never spawns)
            // from dormant (wakes) — the #9 fix is the disambiguation, not
            // the removal of auto-wake.
            let wake = opt_param(params, "wake")
                .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
                .unwrap_or(true);
            // Optional per-call ceiling for the post-wake readiness wait.
            let wake_timeout = opt_param(params, "wake_timeout")
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|s| *s > 0)
                .map(std::time::Duration::from_secs)
                .unwrap_or(crate::workspace_msg::DEFAULT_WAKE_TIMEOUT);
            if workspace.is_empty() {
                return Some(CliResponse::bad_request("Missing workspace"));
            }
            if text.is_empty() {
                return Some(CliResponse::bad_request("Missing text"));
            }
            let resp = crate::workspace_msg::deliver_live(
                &workspace, &text, &from, &command, wake, wake_timeout,
            );
            let body = serde_json::to_string(&resp)
                .unwrap_or_else(|_| "{\"success\":false}".to_string());
            CliResponse::ok_json(body)
        }
        "/cli/workspace/remove" => {
            // Teardown modes (keep_current / restore_original) still
            // live in src-tauri because they depend on
            // HARNESS_WORKSPACE_FILES + find_latest_archive. The
            // daemon serves the DB-only path; callers that pass a
            // `mode` get a 400 telling them to run from the Tauri
            // app until that helper is migrated.
            if params.contains_key("mode") {
                return Some(CliResponse::bad_request(
                    "Workspace teardown modes (keep_current/restore_original) must be run from the Tauri app — daemon serves DB-only remove.",
                ));
            }
            let target = str_param(params, "path");
            match k2_core::workspace::lifecycle::remove_workspace_db_only(&target) {
                Ok(body) => CliResponse::ok_json(body),
                Err(e) => CliResponse::bad_request(e),
            }
        }

        // 0.40.24 S2 — `/cli/workspace/set` is POST-only (it mutates
        // the projects row). A GET landing here via the read dispatch
        // chain gets an explicit 405 instead of a confusing 404 — the
        // `feedback_post_only_route_guards` house rule.
        "/cli/workspace/set" => CliResponse::method_not_allowed(),

        _ => return None,
    };
    Some(resp)
}

// ── 0.40.24 S2 — the settings plane's write door ─────────────────────
//
// `POST /cli/workspace/set` — multi-field per-workspace settings write
// wrapping `k2_core::workspace::settings::update_project_setting`'s
// allowlist. This is the DB half of the "two doors" decision in
// `prd-agent-cli-0.40.24.md` §2.5: DB settings go through here
// (validated allowlist, loud errors); the persona/charter file goes
// through `/cli/agents/save-agent-md`. Powers `k2 agent set`.

/// `POST /cli/workspace/set` body. `project` accepts a workspace NAME,
/// absolute path, or project UUID (resolved via
/// [`crate::workspace_msg::resolve_workspace`]); `fields` maps
/// allowlisted `projects` column names to their new values.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct WorkspaceSetBody {
    project: String,
    fields: serde_json::Map<String, serde_json::Value>,
}

/// Shared 404 shape for agent/workspace addressing misses — a stable
/// `not_found` code plus a did-you-mean hint so the CLI can exit 4 with
/// the contract's `{error:{code, hint}}` stderr JSON.
pub(crate) fn workspace_not_found_response(q: &str) -> CliResponse {
    let suggestion = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::connections::suggest_project_name(&conn, q)
    };
    let hint = match suggestion {
        Some(s) => format!(
            "no agent/workspace named '{q}' — did you mean '{s}'? Run `k2 workspace list` for registered names, or pass a full path."
        ),
        None => format!(
            "no agent/workspace named '{q}'. Run `k2 workspace list` for registered names, or pass a full path."
        ),
    };
    CliResponse {
        status: "404 Not Found",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": "not_found", "hint": hint },
        })
        .to_string(),
    }
}

/// Handler for `POST /cli/workspace/set`.
///
/// Contract (mirrors the `k2 agent set` mockup):
/// - The WHOLE field batch is validated before ANYTHING is applied —
///   one unknown field rejects the request with the valid-field list
///   and zero side effects.
/// - `agent_mode` (when present) is applied FIRST: its write couples
///   `agent_enabled` (off → 0, bot-mode → 1, see
///   `update_project_setting`), so an EXPLICIT `agent_enabled` in the
///   same batch must land after it and win.
/// - Per-field change detection: a field whose stored value already
///   equals the requested value reports `skipped`, so re-running the
///   same set converges with `changed: false`.
pub fn handle_workspace_set(body: &[u8]) -> CliResponse {
    let b: WorkspaceSetBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if b.project.is_empty() {
        return CliResponse::bad_request("missing 'project' (workspace name | path | UUID)");
    }
    if b.fields.is_empty() {
        return CliResponse::bad_request(
            "missing 'fields' — nothing to set (e.g. {\"fields\":{\"agent_mode\":\"manager\"}})",
        );
    }
    let Some(path) = crate::workspace_msg::resolve_workspace(&b.project) else {
        return workspace_not_found_response(&b.project);
    };

    // Validate the ENTIRE batch up front — field names against the
    // core allowlist, values coerced to the string form the settings
    // layer stores. Nothing is applied if any entry is bad.
    let allowed = k2_core::workspace::settings::allowed_project_setting_fields();
    let mut pending: Vec<(String, String)> = Vec::with_capacity(b.fields.len());
    for (field, value) in &b.fields {
        if !allowed.contains(&field.as_str()) {
            return CliResponse::bad_request(format!(
                "unknown setting field '{}' — valid fields: {}",
                field,
                allowed.join(", "),
            ));
        }
        let v = match value {
            serde_json::Value::String(s) => s.clone(),
            // Booleans map onto the 0/1 int columns (agent_enabled,
            // pinned, worktree_mode, …).
            serde_json::Value::Bool(bv) => if *bv { "1" } else { "0" }.to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            other => {
                return CliResponse::bad_request(format!(
                    "field '{field}' value must be a scalar (string/bool/number), got {other}"
                ));
            }
        };
        if field == "agent_mode"
            && !matches!(v.as_str(), "off" | "custom" | "manager" | "k2" | "k2so" | "agent")
        {
            return CliResponse::bad_request(format!(
                "invalid agent_mode '{v}' — valid: off, custom, manager, k2",
            ));
        }
        pending.push((field.clone(), v));
    }

    // agent_mode first (its write couples agent_enabled; an explicit
    // agent_enabled in the same batch must be applied after and win),
    // then the rest in a stable alphabetical order.
    pending.sort_by(|a, b| {
        let rank = |f: &str| usize::from(f != "agent_mode");
        (rank(&a.0), a.0.as_str()).cmp(&(rank(&b.0), b.0.as_str()))
    });

    let mut actions: Vec<serde_json::Value> = Vec::with_capacity(pending.len());
    let mut changed = false;
    for (field, value) in &pending {
        // The stored form of the requested value (agent_mode's CLI
        // spelling `k2` normalizes to the stored `k2so` — B2).
        let stored_target = if field == "agent_mode" {
            k2_core::workspace::settings::stored_agent_mode_value(value).to_string()
        } else {
            value.clone()
        };
        // Per-field change probe. `field` is allowlist-validated above,
        // so interpolating the column name is safe (same rationale as
        // update_project_setting's own SQL).
        let current: Option<String> = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                &format!("SELECT CAST({field} AS TEXT) FROM projects WHERE path = ?1"),
                rusqlite::params![path],
                |r| r.get::<_, Option<String>>(0),
            )
            .unwrap_or(None)
        };
        if current.as_deref() == Some(stored_target.as_str()) {
            actions.push(serde_json::json!({
                "field": field,
                "status": "skipped",
                "reason": "already set",
                "value": value,
            }));
            continue;
        }
        match k2_core::workspace::settings::update_project_setting(&path, field, value) {
            Ok(()) => {
                changed = true;
                actions.push(serde_json::json!({
                    "field": field,
                    "status": "done",
                    "value": value,
                }));
            }
            Err(e) => {
                // Fail loudly mid-batch: report what landed and what
                // didn't so the caller can re-run to converge.
                actions.push(serde_json::json!({
                    "field": field,
                    "status": "failed",
                    "error": e,
                }));
                return CliResponse {
                    status: "400 Bad Request",
                    content_type: "application/json",
                    body: serde_json::json!({
                        "ok": false,
                        "changed": changed,
                        "path": path,
                        "actions": actions,
                        "error": format!("failed setting '{field}': {e}"),
                    })
                    .to_string(),
                };
            }
        }
    }

    if changed {
        k2_core::agent_hooks::emit(
            k2_core::agent_hooks::HookEvent::SyncProjects,
            serde_json::Value::Null,
        );
    }

    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "changed": changed,
            "path": path,
            "actions": actions,
        })
        .to_string(),
    )
}
