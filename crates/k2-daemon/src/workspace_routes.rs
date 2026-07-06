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
        //   session_id=<conversation-uuid-to-set>
        //   provider=<harness>   (OPTIONAL, Slice 3 — the provider that
        //     owns the picked session, e.g. "claude"/"grok"/"pi". When
        //     present it's persisted to workspace_sessions.harness
        //     alongside the id so the resume resolver picks the right
        //     ProviderResume adapter. Absent = keep the existing
        //     harness — backward compatible; the renderer only starts
        //     sending it in Slice 4.)
        "/cli/workspace/set-chat-session" => match need_project(params) {
            Ok(p) => {
                let session_id = str_param(params, "session_id");
                if session_id.is_empty() {
                    return Some(CliResponse::bad_request("Missing session_id parameter"));
                }
                let provider = opt_param(params, "provider");
                let db = k2_core::db::shared();
                let conn = db.lock();
                let project_id = match k2_core::workspace::agent_identity::resolve_project_id(&conn, &p) {
                    Some(pid) => pid,
                    None => return Some(CliResponse::bad_request(
                        format!("project not registered: {p}"),
                    )),
                };
                let update = match provider.as_deref() {
                    Some(prov) => k2_core::db::schema::WorkspaceSession::update_session_id_and_harness(
                        &conn, &project_id, &session_id, prov,
                    ),
                    None => k2_core::db::schema::WorkspaceSession::update_session_id(
                        &conn, &project_id, &session_id,
                    ),
                };
                match update {
                    Ok(rows) => CliResponse::ok_json(
                        serde_json::json!({
                            "success": true,
                            "projectId": project_id,
                            "sessionId": session_id,
                            "provider": provider,
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
            // Projects V1 §4.5 — the PoC removal guard: refuse to
            // deregister a workspace that is the Point of Contact of
            // any project group until a successor is named. This is
            // the `k2 workspace remove` path.
            let project_id = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::workspace::agent_identity::resolve_project_id(&conn, &target)
            };
            if let Some(project_id) = project_id {
                let display = k2_core::workspace::display::agent_display_name(&target);
                if let Some(resp) =
                    crate::project_group_routes::poc_removal_block(&project_id, &display)
                {
                    return Some(resp);
                }
            }
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

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — 0.40.24 S2 settings plane
// ──────────────────────────────────────────────────────────────────────
//
// `db::shared()` in a test context auto-inits the PROCESS-GLOBAL
// in-memory DB (`init_for_tests`), shared across every test in the
// binary — so each test inserts its own project row with a UNIQUE
// name/path (a `ws-set-` prefix) to avoid collisions.

#[cfg(test)]
mod tests {
    use super::*;

    fn unique(label: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4();
        (
            format!("ws-set-{label}-{id}"),
            format!("/tmp/ws-set-test-{label}-{}-{id}", std::process::id()),
        )
    }

    fn insert_project(name: &str, path: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, path],
        )
        .expect("insert project row");
        id
    }

    fn stored_value(path: &str, field: &str) -> Option<String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            &format!("SELECT CAST({field} AS TEXT) FROM projects WHERE path = ?1"),
            rusqlite::params![path],
            |r| r.get::<_, Option<String>>(0),
        )
        .expect("read stored value")
    }

    fn set_body(project: &str, fields: serde_json::Value) -> Vec<u8> {
        serde_json::json!({ "project": project, "fields": fields })
            .to_string()
            .into_bytes()
    }

    /// Multi-field set in ONE call: `agent_mode` + an EXPLICIT
    /// `agent_enabled` in the same batch. The mode write couples
    /// enabled (bot mode → 1), so the explicit `agent_enabled: "0"`
    /// must be applied AFTER the mode and win.
    #[test]
    fn workspace_set_multi_field_applies_mode_before_explicit_enabled() {
        let (name, path) = unique("multi");
        insert_project(&name, &path);

        let resp = handle_workspace_set(&set_body(
            &path,
            serde_json::json!({ "agent_enabled": "0", "agent_mode": "manager" }),
        ));
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let body: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(body["ok"], true);
        assert_eq!(body["changed"], true);

        // Both fields landed; the explicit enabled=0 beat the coupling.
        assert_eq!(stored_value(&path, "agent_mode").as_deref(), Some("manager"));
        assert_eq!(stored_value(&path, "agent_enabled").as_deref(), Some("0"));

        // Every requested field is reported in actions.
        let actions = body["actions"].as_array().expect("actions array");
        assert_eq!(actions.len(), 2, "actions={actions:?}");
        assert!(
            actions.iter().all(|a| a["status"] == "done"),
            "both fields should apply: {actions:?}"
        );
    }

    /// One unknown field rejects the ENTIRE batch — the valid field in
    /// the same request must NOT be applied.
    #[test]
    fn workspace_set_unknown_field_rejects_whole_batch() {
        let (name, path) = unique("unknown");
        insert_project(&name, &path);
        let before = stored_value(&path, "agent_mode");

        let resp = handle_workspace_set(&set_body(
            &path,
            serde_json::json!({ "agent_mode": "manager", "not_a_field": "x" }),
        ));
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
        assert!(
            resp.body.contains("not_a_field"),
            "error must name the offending field: {}",
            resp.body
        );
        assert!(
            resp.body.contains("agent_mode") && resp.body.contains("sandbox_fs_mode"),
            "error must list the valid fields: {}",
            resp.body
        );
        // Nothing was applied.
        assert_eq!(
            stored_value(&path, "agent_mode"),
            before,
            "a rejected batch must have zero side effects"
        );
    }

    /// W6 (0.40.30): `default_agent` is settable through the same
    /// batch door `k2 agent hire --agent` uses — stored VERBATIM
    /// (preset id or legacy command token; the resolver tolerates and
    /// falls through on stale values), and a re-run converges.
    #[test]
    fn workspace_set_writes_default_agent_verbatim_and_converges() {
        let (name, path) = unique("defagent");
        insert_project(&name, &path);
        assert_eq!(stored_value(&path, "default_agent"), None, "starts NULL");

        let resp = handle_workspace_set(&set_body(
            &path,
            serde_json::json!({ "default_agent": "my-preset-slug" }),
        ));
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let body: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(body["changed"], true, "{body}");
        assert_eq!(
            stored_value(&path, "default_agent").as_deref(),
            Some("my-preset-slug"),
            "value must be stored verbatim"
        );

        // Idempotent re-run reports skipped/changed:false.
        let resp = handle_workspace_set(&set_body(
            &path,
            serde_json::json!({ "default_agent": "my-preset-slug" }),
        ));
        let body: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(body["changed"], false, "re-run must converge: {body}");
    }

    /// GET on the POST-only route answers an explicit 405 through the
    /// read dispatch chain (feedback_post_only_route_guards).
    #[test]
    fn workspace_set_get_dispatch_is_405() {
        let params = HashMap::new();
        let resp = dispatch("/cli/workspace/set", &params)
            .expect("route must be claimed by the GET chain");
        assert_eq!(resp.status, "405 Method Not Allowed");
        assert!(resp.body.contains("POST required"), "body={}", resp.body);
    }

    /// B2: the CLI-canonical `k2` spelling stores as the legacy `k2so`
    /// (every stored-value reader matches that), displays back as `k2`,
    /// and a re-run of the same set converges with `changed:false`.
    #[test]
    fn workspace_set_normalizes_k2_spelling_and_converges() {
        let (name, path) = unique("k2norm");
        insert_project(&name, &path);

        let body = set_body(&path, serde_json::json!({ "agent_mode": "k2" }));
        let resp = handle_workspace_set(&body);
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        assert_eq!(stored_value(&path, "agent_mode").as_deref(), Some("k2so"));
        assert_eq!(
            k2_core::workspace::settings::display_agent_mode("k2so"),
            "k2",
            "stored spelling must display as the canonical vocabulary"
        );
        // Coupling: bot mode flips enabled on.
        assert_eq!(stored_value(&path, "agent_enabled").as_deref(), Some("1"));

        // Idempotent re-run: the change probe compares against the
        // NORMALIZED spelling, so this must skip, not re-apply.
        let resp2 = handle_workspace_set(&set_body(&path, serde_json::json!({ "agent_mode": "k2" })));
        let body2: serde_json::Value = serde_json::from_str(&resp2.body).expect("valid JSON");
        assert_eq!(body2["ok"], true);
        assert_eq!(body2["changed"], false, "re-run must converge: {}", resp2.body);
        assert_eq!(body2["actions"][0]["status"], "skipped");
    }

    /// Bad agent_mode VALUES are rejected loudly (no store-then-break).
    #[test]
    fn workspace_set_rejects_invalid_mode_value() {
        let (name, path) = unique("badmode");
        insert_project(&name, &path);
        let resp = handle_workspace_set(&set_body(&path, serde_json::json!({ "agent_mode": "bogus" })));
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
        assert!(
            resp.body.contains("invalid agent_mode"),
            "error should describe the invalid value: {}",
            resp.body
        );
    }

    /// `project` accepts a workspace NAME (the resolver path `k2 agent
    /// set <name>` uses); an unknown token 404s with the stable
    /// `not_found` code + hint.
    #[test]
    fn workspace_set_resolves_name_and_404s_unknown() {
        let (name, path) = unique("byname");
        insert_project(&name, &path);

        // Address by NAME, not path.
        let resp = handle_workspace_set(&set_body(&name, serde_json::json!({ "agent_mode": "custom" })));
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        assert_eq!(stored_value(&path, "agent_mode").as_deref(), Some("custom"));

        // Unknown token → 404 + not_found code.
        let resp = handle_workspace_set(&set_body(
            "no-such-agent-anywhere",
            serde_json::json!({ "agent_mode": "custom" }),
        ));
        assert_eq!(resp.status, "404 Not Found", "body={}", resp.body);
        let body: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(body["error"]["code"], "not_found", "body={}", resp.body);
    }

    /// Empty batches and bodies without a project are usage errors,
    /// not silent no-ops.
    #[test]
    fn workspace_set_requires_project_and_fields() {
        let resp = handle_workspace_set(br#"{"project":"","fields":{"agent_mode":"off"}}"#);
        assert_eq!(resp.status, "400 Bad Request");
        assert!(resp.body.contains("project"), "body={}", resp.body);

        let (name, path) = unique("nofields");
        insert_project(&name, &path);
        let resp = handle_workspace_set(&set_body(&path, serde_json::json!({})));
        assert_eq!(resp.status, "400 Bad Request");
        assert!(resp.body.contains("fields"), "body={}", resp.body);
    }

    /// `/cli/agent/conf` returns the full shape for a registered
    /// workspace (display-normalized mode, enabled bool, null persona,
    /// empty connections, inactive live) and the not-found shape for an
    /// unknown token.
    #[test]
    fn agent_conf_returns_full_shape_and_404s_unknown() {
        let (name, path) = unique("conf");
        insert_project(&name, &path);
        handle_workspace_set(&set_body(&path, serde_json::json!({ "agent_mode": "k2" })));

        let resp = crate::agents_routes::handle_agent_conf(&name);
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let body: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(body["ok"], true);
        assert_eq!(body["path"], path.as_str());
        assert_eq!(body["mode"], "k2", "stored k2so must display as k2: {}", resp.body);
        assert_eq!(body["enabled"], true);
        assert!(body["personaPath"].is_null(), "no AGENT.md on disk: {}", resp.body);
        assert_eq!(body["connections"], serde_json::json!([]));
        assert_eq!(body["live"]["active"], false);

        let resp = crate::agents_routes::handle_agent_conf("definitely-not-registered");
        assert_eq!(resp.status, "404 Not Found", "body={}", resp.body);
        let body: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(body["error"]["code"], "not_found");
    }
}
