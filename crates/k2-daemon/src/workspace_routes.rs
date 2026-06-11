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
                                k2_core::workspace::agent_identity::find_primary_agent(&p)
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
            if workspace.is_empty() {
                return Some(CliResponse::bad_request("Missing workspace"));
            }
            if text.is_empty() {
                return Some(CliResponse::bad_request("Missing text"));
            }
            let resp = crate::workspace_msg::deliver_live(&workspace, &text, &from, &command);
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

        _ => return None,
    };
    Some(resp)
}
