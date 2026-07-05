//! Daemon-side `/cli/project-group/*` route handlers — Projects V1 P2
//! (prd-projects-v1 §4).
//!
//! NOT the legacy `/cli/projects/*` surface (that is the workspace
//! REGISTRY, see the PRD's §2b naming-collision note): a *project
//! group* is a named set of workspaces with exactly one Point of
//! Contact (PoC), one chat stream, and canonical shared dashboards.
//! Thin wrappers over `k2_core::project_groups` (daemon-first).
//!
//! Wired exactly like `feedback_routes.rs`: reads (`list`, `show`,
//! `messages`) are GET query-string routes on `crate::cli::dispatch`'s
//! domain chain; mutations are JSON-bodied POSTs via an isolated arm in
//! `routes::dispatcher` with `token_ok` auth (owner AND connect-user
//! sessions — connect users see projects too). GET-chain hits on POST
//! paths return 405 (`feedback_post_only_route_guards` house rule).
//! `dashboard/save-layout` additionally requires the owner-or-admin
//! bit (PRD §6.3 resolved Q2 — viewers see but cannot save), enforced
//! in the dispatcher arm where the query string lives.
//!
//! Error contract: `{"ok":false,"error":{"code","hint"}}` with stable
//! codes — `not_found` / `ambiguous_id` (CLI exit 4; the ambiguous hint
//! lists the candidate names), `usage` (exit 2), plus the
//! project-specific `name_taken` (409), `poc_successor_required` (409)
//! and `not_a_member` (403). Core errors arrive as `"<code>: <hint>"`
//! strings (project_groups.rs convention) and are split on the first
//! `": "`.
//!
//! Events (PRD §4.2) ride the existing `/events` WireEvent broadcast
//! via `agent_hooks::emit`:
//! - `project-group:message-created` `{groupId, groupName, messageId,
//!   author}` — every STORED chat message, emitted BEFORE the
//!   injection so an open drawer never waits on a slow wake.
//! - `project-group:members-changed` `{groupId}` — add/remove member.
//! - `project-group:poc-changed` `{groupId, pocWorkspaceId}` —
//!   `set-poc` + the first-member auto-PoC on `add-member`.
//! - `project-group:layout-changed` `{groupId, dashboardId, revision}`
//!   — `dashboard/save-layout` (freshness on next open, never a live
//!   rearrange).
//! - `project-group:groups-changed` `{groupId?}` — create / rename /
//!   delete / pin / sort (nav-list liveness).
//!
//! PoC injection (§4.3, trigger matrix frozen): every stored chat
//! message from anyone EXCEPT the PoC best-effort injects into the PoC
//! workspace's CANONICAL session as `[project:<name>] <body>` via
//! `workspace_msg::deliver_live` (wake=true — a dormant canonical
//! agent is woken; `deliver_live` adds its own `[from <author>]`
//! framing). Store → emit → inject: a delivery failure never loses the
//! message; the outcome rides the response as `delivered` /
//! `deliveryReason` / `deliveredSessionId`. The PoC's own posts never
//! self-inject (`deliveryReason: "author-is-poc"` — a NORMAL success);
//! a memberless group has no PoC yet (`"no_poc"`). Membership is
//! enforced HERE (core's `post_message` stores what it is told): the
//! human `owner` is always allowed; any other author must resolve to a
//! MEMBER workspace or the post is refused (`not_a_member`) without
//! storing. Membership / PoC / layout / rename mutations never inject.
//!
//! This module also owns [`poc_removal_block`] — the §4.5 workspace-
//! removal guard wired into every removal path (`/cli/projects/delete`,
//! `/cli/workspace/remove`, `/cli/agent/retire`).

use std::collections::HashMap;

use crate::cli::{opt_param, str_param};
use crate::cli_response::CliResponse;
use k2_core::project_groups::{self, ResolveError};

/// Project-group-domain GET dispatch. Returns `Some(resp)` for a
/// handled path, `None` if the path isn't a project-group route.
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let resp = match path {
        // ── Reads ───────────────────────────────────────────────────
        // GET /cli/project-group/list
        // All groups + member counts + poc + pin/sort metadata, nav
        // order (pinned first, then sort_order, then name).
        "/cli/project-group/list" => match project_groups::list_groups() {
            Ok(groups) => CliResponse::ok_json(
                serde_json::json!({ "ok": true, "groups": groups }).to_string(),
            ),
            Err(e) => core_error(e),
        },

        // GET /cli/project-group/show?group=<id|name|prefix>
        // One group: members enriched with (workspaceId, name, path,
        // agentName), poc, dashboards list.
        "/cli/project-group/show" => {
            let selector = str_param(params, "group");
            if selector.is_empty() {
                return Some(usage_error("missing group (a project name, id, or unique prefix)"));
            }
            let group_id = match project_groups::resolve_group(&selector) {
                Ok(id) => id,
                Err(e) => return Some(resolve_error_response(&selector, e)),
            };
            match build_show_json(&group_id) {
                Ok(v) => CliResponse::ok_json(v.to_string()),
                Err(resp) => resp,
            }
        }

        // GET /cli/project-group/messages?group=&after=<ts>&limit=<n>
        // The single chat stream, oldest-first. No `after` → the
        // latest N (default 20); `after` is strictly-greater (powers
        // CLI `read --after` + the drawer's incremental fetch).
        "/cli/project-group/messages" => {
            let selector = str_param(params, "group");
            if selector.is_empty() {
                return Some(usage_error("missing group (a project name, id, or unique prefix)"));
            }
            let group_id = match project_groups::resolve_group(&selector) {
                Ok(id) => id,
                Err(e) => return Some(resolve_error_response(&selector, e)),
            };
            let after = match opt_param(params, "after") {
                Some(raw) => match raw.parse::<i64>() {
                    Ok(ts) => Some(ts),
                    Err(_) => {
                        return Some(usage_error(format!(
                            "invalid after '{raw}' — expected a unix-seconds timestamp"
                        )))
                    }
                },
                None => None,
            };
            let limit = match opt_param(params, "limit") {
                Some(raw) => match raw.parse::<i64>() {
                    Ok(n) => Some(n),
                    Err(_) => {
                        return Some(usage_error(format!(
                            "invalid limit '{raw}' — expected a positive integer"
                        )))
                    }
                },
                None => None,
            };
            match project_groups::list_messages(&group_id, after, limit) {
                Ok(page) => CliResponse::ok_json(
                    serde_json::json!({
                        "ok": true,
                        "groupId": group_id,
                        "messages": page.messages,
                        "truncated": page.truncated,
                    })
                    .to_string(),
                ),
                Err(e) => core_error(e),
            }
        }

        // ── POST-only mutations reached via the GET chain → 405 ─────
        // (feedback_post_only_route_guards house rule.)
        "/cli/project-group/create"
        | "/cli/project-group/rename"
        | "/cli/project-group/delete"
        | "/cli/project-group/pin"
        | "/cli/project-group/sort"
        | "/cli/project-group/add-member"
        | "/cli/project-group/remove-member"
        | "/cli/project-group/set-poc"
        | "/cli/project-group/msg"
        | "/cli/project-group/dashboard/save-layout" => CliResponse::method_not_allowed(),

        _ => return None,
    };
    Some(resp)
}

/// Dispatch a `/cli/project-group/*` POST body to its handler.
/// Exact-match paths; unknown paths 404 (mirrors
/// `feedback_routes::dispatch_post`).
pub fn dispatch_post(path: &str, body: &[u8]) -> CliResponse {
    match path {
        "/cli/project-group/create" => handle_create(body),
        "/cli/project-group/rename" => handle_rename(body),
        "/cli/project-group/delete" => handle_delete(body),
        "/cli/project-group/pin" => handle_pin(body),
        "/cli/project-group/sort" => handle_sort(body),
        "/cli/project-group/add-member" => handle_add_member(body),
        "/cli/project-group/remove-member" => handle_remove_member(body),
        "/cli/project-group/set-poc" => handle_set_poc(body),
        "/cli/project-group/msg" => handle_msg(body),
        "/cli/project-group/dashboard/save-layout" => handle_save_layout(body),
        _ => CliResponse::not_found(),
    }
}

// ── Shared response helpers ───────────────────────────────────────────

/// Stable usage-error shape (code `usage` → CLI exit 2).
fn usage_error(hint: impl std::fmt::Display) -> CliResponse {
    error_response("400 Bad Request", "usage", hint)
}

fn error_response(
    status: &'static str,
    code: &str,
    hint: impl std::fmt::Display,
) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint.to_string() },
        })
        .to_string(),
    }
}

/// Stable addressing-miss shapes for group-selector resolution (both →
/// CLI exit 4; the ambiguous hint lists the matching candidate names).
fn resolve_error_response(given: &str, err: ResolveError) -> CliResponse {
    match err {
        ResolveError::NotFound => error_response(
            "404 Not Found",
            "not_found",
            format!("no project matches '{given}'"),
        ),
        ResolveError::Ambiguous(candidates) => error_response(
            "404 Not Found",
            "ambiguous_id",
            format!(
                "'{given}' matches {} projects — use a longer prefix: {}",
                candidates.len(),
                candidates.join(", "),
            ),
        ),
    }
}

/// Map a core `Err(String)` onto the wire error contract. Core errors
/// carry the stable code as the FIRST TOKEN (`"<code>: <hint>"`,
/// project_groups.rs convention); anything without a recognized code
/// is a validation-shaped `usage` error.
fn core_error(e: String) -> CliResponse {
    let (code, hint) = match e.split_once(": ") {
        Some((c @ ("name_taken" | "poc_successor_required" | "not_a_member" | "usage"), h)) => {
            (c.to_string(), h.to_string())
        }
        _ => ("usage".to_string(), e),
    };
    let status = match code.as_str() {
        "name_taken" | "poc_successor_required" => "409 Conflict",
        "not_a_member" => "403 Forbidden",
        _ => "400 Bad Request",
    };
    error_response(status, &code, hint)
}

/// `ok:true` + the group's full wire fields (the shape `create` /
/// `rename` / `pin` / `sort` / `set-poc` respond with).
fn group_ok_json(group: &project_groups::ProjectGroup) -> CliResponse {
    let mut v = serde_json::to_value(group).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(map) = v.as_object_mut() {
        map.insert("ok".to_string(), serde_json::json!(true));
    }
    CliResponse::ok_json(v.to_string())
}

/// `projects.name` + `projects.path` for a workspace id (member
/// enrichment + the PoC injection target).
fn workspace_name_path(workspace_id: &str) -> (Option<String>, Option<String>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT name, path FROM projects WHERE id = ?1",
        rusqlite::params![workspace_id],
        |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
    )
    .unwrap_or((None, None))
}

/// The `show` wire shape: the group's fields flat at the top level +
/// `members` (each enriched with the workspace's registry name, path,
/// and agent display name) + `dashboards`.
fn build_show_json(group_id: &str) -> Result<serde_json::Value, CliResponse> {
    let Some(group) = project_groups::get_group_by_id(group_id) else {
        return Err(resolve_error_response(group_id, ResolveError::NotFound));
    };
    let members = project_groups::list_members(group_id).map_err(core_error)?;
    let dashboards = project_groups::list_dashboards(group_id).map_err(core_error)?;
    let members_json: Vec<serde_json::Value> = members
        .iter()
        .map(|m| {
            let (name, path) = workspace_name_path(&m.workspace_id);
            let agent_name = path
                .as_deref()
                .map(k2_core::workspace::display::agent_display_name);
            serde_json::json!({
                "workspaceId": m.workspace_id,
                "name": name,
                "path": path,
                "agentName": agent_name,
                "createdAt": m.created_at,
            })
        })
        .collect();
    let mut v = serde_json::to_value(&group).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(map) = v.as_object_mut() {
        map.insert("ok".to_string(), serde_json::json!(true));
        map.insert("members".to_string(), serde_json::json!(members_json));
        map.insert("dashboards".to_string(), serde_json::json!(dashboards));
    }
    Ok(v)
}

fn emit(event: k2_core::agent_hooks::HookEvent, payload: serde_json::Value) {
    k2_core::agent_hooks::emit(event, payload);
}

/// `project-group:groups-changed` — structural nav-list refresh signal
/// (create / rename / delete / pin / sort).
fn emit_groups_changed(group_id: &str) {
    emit(
        k2_core::agent_hooks::HookEvent::ProjectGroupsChanged,
        serde_json::json!({ "groupId": group_id }),
    );
}

/// Resolve a `workspace` body field (name | absolute path | UUID, via
/// [`crate::workspace_msg::resolve_workspace`] — the feedback `project`
/// field's resolver) to its `projects.id` + path.
fn resolve_member_workspace(token: &str) -> Result<(String, String), CliResponse> {
    let Some(path) = crate::workspace_msg::resolve_workspace(token) else {
        return Err(crate::workspace_routes::workspace_not_found_response(token));
    };
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
    };
    match project_id {
        Some(id) => Ok((id, path)),
        None => Err(error_response(
            "404 Not Found",
            "not_found",
            format!("workspace not registered: {path}"),
        )),
    }
}

// ── §4.5 — the PoC workspace-removal guard ────────────────────────────

/// The §4.5 removal guard, enforced at the TOP of every workspace-
/// removal path (`/cli/projects/delete`, `/cli/workspace/remove`,
/// `/cli/agent/retire`): a workspace that is the Point of Contact
/// anywhere cannot be removed from the server until every affected
/// project has a successor. NO auto-reassignment; NOT force-
/// overridable.
///
/// `None` = not a PoC anywhere, removal may proceed. `Some(resp)` =
/// refuse with the resolved-Q6 shape — 409, stable code
/// `poc_of_projects`, hint
/// `"X is the Point of Contact for: A, B. Reassign the PoC first."`
/// (a DB error checking the guard fails CLOSED with a loud 500 —
/// never silently allows the removal through).
pub fn poc_removal_block(workspace_id: &str, display_name: &str) -> Option<CliResponse> {
    let blocks = match project_groups::poc_blocks_for_workspace(workspace_id) {
        Ok(b) => b,
        Err(e) => {
            return Some(CliResponse::internal_error(format!(
                "PoC removal-guard check failed: {e}"
            )))
        }
    };
    if blocks.is_empty() {
        return None;
    }
    Some(error_response(
        "409 Conflict",
        "poc_of_projects",
        format!(
            "{display_name} is the Point of Contact for: {}. Reassign the PoC first.",
            blocks.join(", ")
        ),
    ))
}

// ── POST handlers ─────────────────────────────────────────────────────

/// `POST /cli/project-group/create` body.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct CreateBody {
    name: String,
}

/// Handler for `POST /cli/project-group/create` — group + its auto
/// 'Main' dashboard; fires `project-group:groups-changed`.
pub fn handle_create(body: &[u8]) -> CliResponse {
    let b: CreateBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.name.trim().is_empty() {
        return usage_error("create requires a non-empty project <name>");
    }
    match project_groups::create_group(&b.name) {
        Ok(group) => {
            emit_groups_changed(&group.id);
            group_ok_json(&group)
        }
        Err(e) => core_error(e),
    }
}

/// `POST /cli/project-group/rename` body.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RenameBody {
    group: String,
    name: String,
}

/// Handler for `POST /cli/project-group/rename`. Never injects.
pub fn handle_rename(body: &[u8]) -> CliResponse {
    let b: RenameBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.group.is_empty() {
        return usage_error("missing 'group' (a project name, id, or unique prefix)");
    }
    if b.name.trim().is_empty() {
        return usage_error("rename requires a non-empty new <name>");
    }
    let group_id = match project_groups::resolve_group(&b.group) {
        Ok(id) => id,
        Err(e) => return resolve_error_response(&b.group, e),
    };
    match project_groups::rename_group(&group_id, &b.name) {
        Ok(group) => {
            emit_groups_changed(&group.id);
            group_ok_json(&group)
        }
        Err(e) => core_error(e),
    }
}

/// `POST /cli/project-group/delete` body.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct DeleteBody {
    group: String,
}

/// Handler for `POST /cli/project-group/delete` — cascades the group's
/// member/message/dashboard rows only, NEVER the workspaces themselves
/// (locked default, PRD ledger §11).
pub fn handle_delete(body: &[u8]) -> CliResponse {
    let b: DeleteBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.group.is_empty() {
        return usage_error("missing 'group' (a project name, id, or unique prefix)");
    }
    let group_id = match project_groups::resolve_group(&b.group) {
        Ok(id) => id,
        Err(e) => return resolve_error_response(&b.group, e),
    };
    match project_groups::delete_group(&group_id) {
        Ok(()) => {
            emit_groups_changed(&group_id);
            CliResponse::ok_json(
                serde_json::json!({ "ok": true, "id": group_id }).to_string(),
            )
        }
        Err(e) => core_error(e),
    }
}

/// `POST /cli/project-group/pin` body. `pinned` is REQUIRED (an absent
/// flag is a usage error, never a silent unpin).
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct PinBody {
    group: String,
    pinned: Option<bool>,
}

/// Handler for `POST /cli/project-group/pin` — the canonical nav
/// Pinned-section flag (resolved Q4).
pub fn handle_pin(body: &[u8]) -> CliResponse {
    let b: PinBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.group.is_empty() {
        return usage_error("missing 'group' (a project name, id, or unique prefix)");
    }
    let Some(pinned) = b.pinned else {
        return usage_error("missing 'pinned' (true|false)");
    };
    let group_id = match project_groups::resolve_group(&b.group) {
        Ok(id) => id,
        Err(e) => return resolve_error_response(&b.group, e),
    };
    match project_groups::set_pinned(&group_id, pinned) {
        Ok(group) => {
            emit_groups_changed(&group.id);
            group_ok_json(&group)
        }
        Err(e) => core_error(e),
    }
}

/// `POST /cli/project-group/sort` body. `sortOrder` is REQUIRED.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct SortBody {
    group: String,
    sort_order: Option<i64>,
}

/// Handler for `POST /cli/project-group/sort` — nav ordering within
/// the group's section. (Additive beyond the PRD §4.1 table — the
/// write half of the `sort_order` column §6.1's drag-to-reorder needs;
/// mirrors `set_pinned`'s shape.)
pub fn handle_sort(body: &[u8]) -> CliResponse {
    let b: SortBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.group.is_empty() {
        return usage_error("missing 'group' (a project name, id, or unique prefix)");
    }
    let Some(sort_order) = b.sort_order else {
        return usage_error("missing 'sortOrder' (an integer)");
    };
    let group_id = match project_groups::resolve_group(&b.group) {
        Ok(id) => id,
        Err(e) => return resolve_error_response(&b.group, e),
    };
    match project_groups::set_sort_order(&group_id, sort_order) {
        Ok(group) => {
            emit_groups_changed(&group.id);
            group_ok_json(&group)
        }
        Err(e) => core_error(e),
    }
}

/// `POST /cli/project-group/add-member` /
/// `POST /cli/project-group/remove-member` /
/// `POST /cli/project-group/set-poc` body. `workspace` accepts a
/// workspace name, absolute path, or project UUID.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct MemberBody {
    group: String,
    workspace: String,
}

fn parse_member_body(body: &[u8]) -> Result<(String, String, String), CliResponse> {
    let b: MemberBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return Err(usage_error(format!("invalid JSON body: {e}"))),
    };
    if b.group.is_empty() {
        return Err(usage_error("missing 'group' (a project name, id, or unique prefix)"));
    }
    if b.workspace.is_empty() {
        return Err(usage_error("missing 'workspace' (name | path | UUID)"));
    }
    let group_id = match project_groups::resolve_group(&b.group) {
        Ok(id) => id,
        Err(e) => return Err(resolve_error_response(&b.group, e)),
    };
    let (workspace_id, _path) = resolve_member_workspace(&b.workspace)?;
    Ok((group_id, workspace_id, b.workspace))
}

/// Handler for `POST /cli/project-group/add-member`. The FIRST member
/// of an empty group auto-becomes the PoC (`becamePoc: true`, +
/// `project-group:poc-changed`); re-adding an existing member is an Ok
/// no-op that emits nothing.
pub fn handle_add_member(body: &[u8]) -> CliResponse {
    let (group_id, workspace_id, _) = match parse_member_body(body) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    match project_groups::add_member(&group_id, &workspace_id) {
        Ok(outcome) => {
            if !outcome.already_member {
                emit(
                    k2_core::agent_hooks::HookEvent::ProjectGroupMembersChanged,
                    serde_json::json!({ "groupId": group_id }),
                );
                if outcome.became_poc {
                    emit(
                        k2_core::agent_hooks::HookEvent::ProjectGroupPocChanged,
                        serde_json::json!({
                            "groupId": group_id,
                            "pocWorkspaceId": workspace_id,
                        }),
                    );
                }
            }
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "groupId": group_id,
                    "workspaceId": workspace_id,
                    "alreadyMember": outcome.already_member,
                    "becamePoc": outcome.became_poc,
                })
                .to_string(),
            )
        }
        Err(e) => core_error(e),
    }
}

/// Handler for `POST /cli/project-group/remove-member`. Removing the
/// current PoC is refused (`poc_successor_required`, 409) until
/// `set-poc` names a successor — no auto-reassignment.
pub fn handle_remove_member(body: &[u8]) -> CliResponse {
    let (group_id, workspace_id, _) = match parse_member_body(body) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    match project_groups::remove_member(&group_id, &workspace_id) {
        Ok(()) => {
            emit(
                k2_core::agent_hooks::HookEvent::ProjectGroupMembersChanged,
                serde_json::json!({ "groupId": group_id }),
            );
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "groupId": group_id,
                    "workspaceId": workspace_id,
                })
                .to_string(),
            )
        }
        Err(e) => core_error(e),
    }
}

/// Handler for `POST /cli/project-group/set-poc`. The target must
/// already be a member (`not_a_member`, 403). Never injects.
pub fn handle_set_poc(body: &[u8]) -> CliResponse {
    let (group_id, workspace_id, _) = match parse_member_body(body) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    match project_groups::set_poc(&group_id, &workspace_id) {
        Ok(group) => {
            emit(
                k2_core::agent_hooks::HookEvent::ProjectGroupPocChanged,
                serde_json::json!({
                    "groupId": group.id,
                    "pocWorkspaceId": workspace_id,
                }),
            );
            group_ok_json(&group)
        }
        Err(e) => core_error(e),
    }
}

// ── The chat post + §4.3 PoC injection ────────────────────────────────

/// `POST /cli/project-group/msg` body. `author` defaults to `owner`
/// (the app drawer posts author-less); member agents pass their own
/// workspace name via the CLI.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct MsgBody {
    group: String,
    body: String,
    author: Option<String>,
}

/// The injected payload (§4.3): `[project:<name>] <body>`.
/// `deliver_live` adds its own `[from <author>]` sender framing on
/// delivery, so the author is not duplicated here.
fn project_payload(group_name: &str, body: &str) -> String {
    format!("[project:{group_name}] {body}")
}

/// Handler for `POST /cli/project-group/msg` — store → emit →
/// best-effort PoC injection (§4.3, trigger matrix frozen):
///
/// | actor                    | stored | emits | injected into PoC |
/// |--------------------------|--------|-------|-------------------|
/// | human (`owner`)          | yes    | yes   | yes (wake=true)   |
/// | member agent, not PoC    | yes    | yes   | yes (wake=true)   |
/// | the PoC itself           | yes    | yes   | NO — never self-inject (`author-is-poc`) |
/// | non-member workspace     | NO (`not_a_member`) | no | no       |
///
/// Membership is enforced here (core stores what it is told): `owner`
/// is always allowed; any other author must resolve to a MEMBER
/// workspace. PoC identity is read at store time — a post racing a
/// reassignment injects into whoever is PoC when the message lands. A
/// delivery failure never fails the store; the outcome rides the
/// response (`delivered` / `deliveryReason` / `deliveredSessionId`).
pub fn handle_msg(body: &[u8]) -> CliResponse {
    let b: MsgBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.group.is_empty() {
        return usage_error("missing 'group' (a project name, id, or unique prefix)");
    }
    if b.body.trim().is_empty() {
        return usage_error("msg requires non-empty <text>");
    }
    let group_id = match project_groups::resolve_group(&b.group) {
        Ok(id) => id,
        Err(e) => return resolve_error_response(&b.group, e),
    };
    let Some(group) = project_groups::get_group_by_id(&group_id) else {
        return resolve_error_response(&b.group, ResolveError::NotFound);
    };

    // Author attribution + route-level membership enforcement (§3):
    // author ∈ {owner, a member workspace}. Core's post_message stores
    // what it is told — the gate lives HERE.
    let author_given = b.author.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let author = author_given.unwrap_or("owner");
    let author_workspace_id: Option<String> = if author == "owner" {
        None // the human — always allowed.
    } else {
        let Some(path) = crate::workspace_msg::resolve_workspace(author) else {
            return error_response(
                "403 Forbidden",
                "not_a_member",
                format!(
                    "author '{author}' is not a registered workspace — only 'owner' or a \
                     member workspace's agent can post to '{}'",
                    group.name
                ),
            );
        };
        let workspace_id = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
        };
        let Some(workspace_id) = workspace_id else {
            return error_response(
                "403 Forbidden",
                "not_a_member",
                format!("author workspace not registered: {path}"),
            );
        };
        let members = match project_groups::list_members(&group_id) {
            Ok(m) => m,
            Err(e) => return core_error(e),
        };
        if !members.iter().any(|m| m.workspace_id == workspace_id) {
            return error_response(
                "403 Forbidden",
                "not_a_member",
                format!(
                    "author '{author}' is not a member of project '{}' — add it first",
                    group.name
                ),
            );
        }
        Some(workspace_id)
    };

    // Store.
    let msg = match project_groups::post_message(&group_id, author, &b.body) {
        Ok(m) => m,
        Err(e) => return core_error(e),
    };

    // Emit BEFORE the injection so an open drawer / badge never waits
    // on a slow wake (the feedback_routes ordering).
    emit(
        k2_core::agent_hooks::HookEvent::ProjectGroupMessageCreated,
        serde_json::json!({
            "groupId": group_id,
            "groupName": group.name,
            "messageId": msg.id,
            "author": msg.author,
        }),
    );

    // §4.3 — best-effort PoC injection. PoC identity is read AT STORE
    // TIME (re-read after the insert, not from the pre-store snapshot).
    let poc = match project_groups::get_poc(&group_id) {
        Ok(p) => p,
        Err(_) => None, // group deleted mid-post: message stored; nothing to inject into.
    };
    let (delivered, delivery_reason, delivered_session) = match poc.as_deref() {
        // A memberless group has no PoC yet — a stored-but-undelivered
        // post is a NORMAL success.
        None => (false, Some("no_poc".to_string()), None),
        // The PoC's own posts never self-inject (frozen matrix).
        Some(poc_id) if author_workspace_id.as_deref() == Some(poc_id) => {
            (false, Some("author-is-poc".to_string()), None)
        }
        Some(poc_id) => {
            let (_, path) = workspace_name_path(poc_id);
            match path {
                None => (false, Some("workspace_not_found".to_string()), None),
                Some(path) => {
                    let payload = project_payload(&group.name, &msg.body);
                    // An owner post is framed with the owner's display
                    // name (the composer's server-side resolution, cf.
                    // feedback D3); agents are framed as themselves.
                    let from = if author == "owner" {
                        crate::workspace_msg::resolve_owner_from()
                    } else {
                        author.to_string()
                    };
                    let resp = crate::workspace_msg::deliver_live(
                        &path,
                        &payload,
                        &from,
                        "",
                        true,
                        crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
                    );
                    (resp.success, resp.reason, resp.target_session_id)
                }
            }
        }
    };

    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "id": msg.id,
            "groupId": group_id,
            "groupName": group.name,
            "author": msg.author,
            "body": msg.body,
            "createdAt": msg.created_at,
            "delivered": delivered,
            "deliveryReason": delivery_reason,
            "deliveredSessionId": delivered_session,
        })
        .to_string(),
    )
}

// ── Dashboards ────────────────────────────────────────────────────────

/// `POST /cli/project-group/dashboard/save-layout` body. `layoutJson`
/// accepts the layout as a JSON value (the natural renderer shape) or
/// a pre-serialized JSON string.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct SaveLayoutBody {
    group: String,
    dashboard_id: String,
    layout_json: serde_json::Value,
}

/// Handler for `POST /cli/project-group/dashboard/save-layout` —
/// last-write-wins, revision++, fires `project-group:layout-changed`
/// (freshness on NEXT open/switch — never a live rearrange).
/// Dashboard-id-addressed from day one (V1.1 multi-dashboards needs no
/// migration); the dashboard must belong to the addressed group.
pub fn handle_save_layout(body: &[u8]) -> CliResponse {
    let b: SaveLayoutBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.group.is_empty() {
        return usage_error("missing 'group' (a project name, id, or unique prefix)");
    }
    if b.dashboard_id.is_empty() {
        return usage_error("missing 'dashboardId'");
    }
    let layout = match &b.layout_json {
        serde_json::Value::Null => return usage_error("missing 'layoutJson'"),
        serde_json::Value::String(s) => s.clone(),
        v => v.to_string(),
    };
    let group_id = match project_groups::resolve_group(&b.group) {
        Ok(id) => id,
        Err(e) => return resolve_error_response(&b.group, e),
    };
    let Some(dashboard) = project_groups::get_dashboard(&b.dashboard_id) else {
        return error_response(
            "404 Not Found",
            "not_found",
            format!("no dashboard with id {}", b.dashboard_id),
        );
    };
    if dashboard.group_id != group_id {
        return error_response(
            "404 Not Found",
            "not_found",
            format!("dashboard {} does not belong to project '{}'", b.dashboard_id, b.group),
        );
    }
    match project_groups::save_dashboard_layout(&b.dashboard_id, &layout) {
        Ok(saved) => {
            emit(
                k2_core::agent_hooks::HookEvent::ProjectGroupLayoutChanged,
                serde_json::json!({
                    "groupId": group_id,
                    "dashboardId": saved.id,
                    "revision": saved.revision,
                }),
            );
            let mut v = serde_json::to_value(&saved).unwrap_or_else(|_| serde_json::json!({}));
            if let Some(map) = v.as_object_mut() {
                map.insert("ok".to_string(), serde_json::json!(true));
            }
            CliResponse::ok_json(v.to_string())
        }
        Err(e) => core_error(e),
    }
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — Projects V1 P2 routes
// ──────────────────────────────────────────────────────────────────────
//
// Mirrors feedback_routes' test module: `db::shared()` auto-inits the
// PROCESS-GLOBAL in-memory DB shared across every test in the binary —
// each test inserts its own rows with UNIQUE (uuid-embedding) names and
// paths, and any name PREFIX a test resolves embeds that test's own
// uuid so sibling tests' rows can never collide with it. Event
// assertions go through the crate-wide shared capture sink
// (`crate::test_support`) and filter by their own group id.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{event_mark, install_capture_sink};

    /// Events since `mark` whose payload `groupId` matches (emission
    /// order preserved).
    fn events_for_group(mark: usize, group_id: &str) -> Vec<(String, serde_json::Value)> {
        crate::test_support::events_since(mark)
            .into_iter()
            .filter(|(_, p)| p["groupId"] == group_id)
            .collect()
    }

    /// A per-test unique project-group name embedding a fresh uuid.
    fn gname(label: &str) -> String {
        format!("pgr-{label}-{}", uuid::Uuid::new_v4())
    }

    /// Insert a workspace-registry row (`projects`) with a unique
    /// name/path; returns `(id, name, path)`.
    fn insert_workspace(label: &str) -> (String, String, String) {
        let id = uuid::Uuid::new_v4().to_string();
        let name = format!("pgr-ws-{label}-{id}");
        let path = format!("/tmp/pgr-ws-{label}-{}-{id}", std::process::id());
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, path],
        )
        .expect("insert project row");
        (id, name, path)
    }

    fn post_json(path: &str, body: serde_json::Value) -> CliResponse {
        dispatch_post(path, body.to_string().as_bytes())
    }

    fn ok_json(resp: CliResponse) -> serde_json::Value {
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        serde_json::from_str(&resp.body).expect("valid JSON body")
    }

    fn create_group_via_route(name: &str) -> serde_json::Value {
        ok_json(post_json(
            "/cli/project-group/create",
            serde_json::json!({ "name": name }),
        ))
    }

    fn get_params(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn show(selector: &str) -> CliResponse {
        dispatch("/cli/project-group/show", &get_params(&[("group", selector)]))
            .expect("show route claimed")
    }

    fn messages(selector: &str, extra: &[(&str, &str)]) -> CliResponse {
        let mut pairs = vec![("group", selector)];
        pairs.extend_from_slice(extra);
        dispatch("/cli/project-group/messages", &get_params(&pairs))
            .expect("messages route claimed")
    }

    fn msg_as(group: &str, author: Option<&str>, text: &str) -> CliResponse {
        let mut body = serde_json::json!({ "group": group, "body": text });
        if let Some(a) = author {
            body["author"] = serde_json::json!(a);
        }
        post_json("/cli/project-group/msg", body)
    }

    /// Full lifecycle through the route layer: create (auto 'Main'
    /// dashboard) → list → rename → pin → sort → delete, with the
    /// groups-changed event on every structural mutation.
    #[test]
    fn project_group_route_lifecycle() {
        install_capture_sink();
        let name = gname("lifecycle");

        let mark = event_mark();
        let created = create_group_via_route(&name);
        assert_eq!(created["ok"], true);
        assert_eq!(created["name"], name.as_str());
        assert!(created["pocWorkspaceId"].is_null(), "memberless → no PoC");
        assert_eq!(created["memberCount"], 0);
        let gid = created["id"].as_str().expect("id").to_string();
        let events = events_for_group(mark, &gid);
        assert_eq!(
            events.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["project-group:groups-changed"],
            "create emits groups-changed only: {events:?}"
        );

        // show: the auto 'Main' dashboard rides along.
        let shown = ok_json(show(&name));
        assert_eq!(shown["id"], gid.as_str());
        let dashboards = shown["dashboards"].as_array().expect("dashboards");
        assert_eq!(dashboards.len(), 1);
        assert_eq!(dashboards[0]["name"], "Main");
        assert_eq!(dashboards[0]["revision"], 0);
        assert_eq!(shown["members"].as_array().expect("members").len(), 0);

        // list contains the group.
        let listed = ok_json(
            dispatch("/cli/project-group/list", &get_params(&[])).expect("list claimed"),
        );
        assert!(
            listed["groups"]
                .as_array()
                .expect("groups")
                .iter()
                .any(|g| g["id"] == gid.as_str()),
            "created group listed"
        );

        // rename → groups-changed; the new name resolves.
        let new_name = gname("lifecycle-renamed");
        let mark = event_mark();
        let renamed = ok_json(post_json(
            "/cli/project-group/rename",
            serde_json::json!({ "group": gid, "name": new_name }),
        ));
        assert_eq!(renamed["name"], new_name.as_str());
        assert_eq!(
            events_for_group(mark, &gid)
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            vec!["project-group:groups-changed"]
        );
        assert_eq!(ok_json(show(&new_name))["id"], gid.as_str());

        // pin + sort round-trip, each emitting groups-changed.
        let mark = event_mark();
        let pinned = ok_json(post_json(
            "/cli/project-group/pin",
            serde_json::json!({ "group": gid, "pinned": true }),
        ));
        assert_eq!(pinned["pinned"], true);
        let sorted = ok_json(post_json(
            "/cli/project-group/sort",
            serde_json::json!({ "group": gid, "sortOrder": 5 }),
        ));
        assert_eq!(sorted["sortOrder"], 5);
        assert_eq!(sorted["pinned"], true, "sort must not clobber pin");
        assert_eq!(
            events_for_group(mark, &gid)
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            vec!["project-group:groups-changed", "project-group:groups-changed"]
        );

        // Missing-flag validation is loud.
        let resp = post_json("/cli/project-group/pin", serde_json::json!({ "group": gid }));
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
        let resp = post_json("/cli/project-group/sort", serde_json::json!({ "group": gid }));
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);

        // delete → groups-changed; the group stops resolving.
        let mark = event_mark();
        let deleted = ok_json(post_json(
            "/cli/project-group/delete",
            serde_json::json!({ "group": gid }),
        ));
        assert_eq!(deleted["id"], gid.as_str());
        assert_eq!(
            events_for_group(mark, &gid)
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            vec!["project-group:groups-changed"]
        );
        let resp = show(&new_name);
        assert_eq!(resp.status, "404 Not Found", "body={}", resp.body);
    }

    /// Addressing misses: unknown selector → 404 `not_found`;
    /// ambiguous name prefix → 404 `ambiguous_id` with every candidate
    /// name in the hint — on the GET chain AND a POST mutation.
    #[test]
    fn project_group_resolve_errors() {
        let base = gname("resolve");
        let a = format!("{base}-alpha");
        let b_name = format!("{base}-beta");
        create_group_via_route(&a);
        create_group_via_route(&b_name);

        // Ambiguous prefix (embeds this test's uuid → no sibling noise).
        let ambiguous = format!("{base}-");
        for resp in [
            show(&ambiguous),
            post_json(
                "/cli/project-group/rename",
                serde_json::json!({ "group": ambiguous, "name": gname("x") }),
            ),
        ] {
            assert_eq!(resp.status, "404 Not Found", "body={}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "ambiguous_id", "body={}", resp.body);
            let hint = v["error"]["hint"].as_str().expect("hint");
            assert!(
                hint.contains(&a) && hint.contains(&b_name),
                "candidates must be in the hint: {hint}"
            );
        }

        // Unknown → not_found on GET and POST alike.
        let unknown = format!("zzz-no-such-{}", uuid::Uuid::new_v4());
        for resp in [
            show(&unknown),
            messages(&unknown, &[]),
            post_json(
                "/cli/project-group/delete",
                serde_json::json!({ "group": unknown }),
            ),
            post_json(
                "/cli/project-group/msg",
                serde_json::json!({ "group": unknown, "body": "x" }),
            ),
        ] {
            assert_eq!(resp.status, "404 Not Found", "body={}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "not_found", "body={}", resp.body);
        }

        // Missing group param → usage on every group-taking route.
        for resp in [
            show(""),
            messages("", &[]),
            post_json("/cli/project-group/create", serde_json::json!({ "name": " " })),
            post_json("/cli/project-group/msg", serde_json::json!({ "body": "x" })),
        ] {
            assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "usage", "body={}", resp.body);
        }

        // Duplicate name → the stable name_taken code, 409.
        let resp = post_json("/cli/project-group/create", serde_json::json!({ "name": a }));
        assert_eq!(resp.status, "409 Conflict", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "name_taken");
    }

    /// Membership + PoC through the routes: first member auto-becomes
    /// PoC (members-changed + poc-changed), removing the PoC is 409
    /// `poc_successor_required`, set-poc requires membership (403
    /// `not_a_member`), unknown workspace tokens 404.
    #[test]
    fn project_group_members_and_poc_routes() {
        install_capture_sink();
        let g = create_group_via_route(&gname("members"));
        let gid = g["id"].as_str().expect("id").to_string();
        let (w1_id, w1_name, _) = insert_workspace("m1");
        let (w2_id, _, w2_path) = insert_workspace("m2");

        // First member (addressed by NAME) auto-becomes PoC.
        let mark = event_mark();
        let added = ok_json(post_json(
            "/cli/project-group/add-member",
            serde_json::json!({ "group": gid, "workspace": w1_name }),
        ));
        assert_eq!(added["workspaceId"], w1_id.as_str());
        assert_eq!(added["becamePoc"], true);
        assert_eq!(added["alreadyMember"], false);
        let events = events_for_group(mark, &gid);
        assert_eq!(
            events.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["project-group:members-changed", "project-group:poc-changed"],
            "first add emits members-changed then poc-changed: {events:?}"
        );
        assert_eq!(events[1].1["pocWorkspaceId"], w1_id.as_str());

        // Second member (addressed by PATH) does not steal the PoC.
        let mark = event_mark();
        let added = ok_json(post_json(
            "/cli/project-group/add-member",
            serde_json::json!({ "group": gid, "workspace": w2_path }),
        ));
        assert_eq!(added["workspaceId"], w2_id.as_str());
        assert_eq!(added["becamePoc"], false);
        let events = events_for_group(mark, &gid);
        assert_eq!(
            events.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["project-group:members-changed"]
        );

        // Re-add: Ok no-op, nothing emitted.
        let mark = event_mark();
        let readd = ok_json(post_json(
            "/cli/project-group/add-member",
            serde_json::json!({ "group": gid, "workspace": w1_id }),
        ));
        assert_eq!(readd["alreadyMember"], true);
        assert!(events_for_group(mark, &gid).is_empty(), "no-op must not emit");

        // show enriches members with registry name/path + agent name.
        let shown = ok_json(show(&gid));
        assert_eq!(shown["pocWorkspaceId"], w1_id.as_str());
        let members = shown["members"].as_array().expect("members");
        assert_eq!(members.len(), 2);
        let m1 = members
            .iter()
            .find(|m| m["workspaceId"] == w1_id.as_str())
            .expect("w1 in members");
        assert_eq!(m1["name"], w1_name.as_str());
        assert!(
            m1["agentName"].as_str().is_some_and(|s| !s.is_empty()),
            "agentName enriched: {m1}"
        );

        // Removing the PoC is refused with the stable code — 409.
        let resp = post_json(
            "/cli/project-group/remove-member",
            serde_json::json!({ "group": gid, "workspace": w1_id }),
        );
        assert_eq!(resp.status, "409 Conflict", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "poc_successor_required");

        // set-poc to a NON-member → 403 not_a_member; PoC unchanged.
        let (stranger_id, ..) = insert_workspace("stranger");
        let resp = post_json(
            "/cli/project-group/set-poc",
            serde_json::json!({ "group": gid, "workspace": stranger_id }),
        );
        assert_eq!(resp.status, "403 Forbidden", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "not_a_member");

        // Successor named → poc-changed; then the ex-PoC removes fine.
        let mark = event_mark();
        let updated = ok_json(post_json(
            "/cli/project-group/set-poc",
            serde_json::json!({ "group": gid, "workspace": w2_id }),
        ));
        assert_eq!(updated["pocWorkspaceId"], w2_id.as_str());
        let events = events_for_group(mark, &gid);
        assert_eq!(
            events.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["project-group:poc-changed"]
        );
        assert_eq!(events[0].1["pocWorkspaceId"], w2_id.as_str());
        let mark = event_mark();
        let removed = ok_json(post_json(
            "/cli/project-group/remove-member",
            serde_json::json!({ "group": gid, "workspace": w1_id }),
        ));
        assert_eq!(removed["workspaceId"], w1_id.as_str());
        assert_eq!(
            events_for_group(mark, &gid)
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            vec!["project-group:members-changed"]
        );

        // Unknown workspace token → the shared 404 not_found shape.
        let resp = post_json(
            "/cli/project-group/add-member",
            serde_json::json!({ "group": gid, "workspace": "no-such-ws-anywhere-zz" }),
        );
        assert_eq!(resp.status, "404 Not Found", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "not_found");
    }

    /// §4.3 — the frozen trigger matrix through the msg route. Every
    /// test workspace is dead (no agent, no live session), so an
    /// injection ATTEMPT reports `delivered:false` with the wake
    /// path's `no_agent_mode` — the point is which actors trigger an
    /// attempt at all, and that the store + emit always survive.
    #[test]
    fn project_group_msg_injection_matrix() {
        install_capture_sink();
        let name = gname("inject");
        let g = create_group_via_route(&name);
        let gid = g["id"].as_str().expect("id").to_string();
        let (poc_id, poc_name, _) = insert_workspace("poc");
        let (_member_id, member_name, _) = insert_workspace("member");
        ok_json(post_json(
            "/cli/project-group/add-member",
            serde_json::json!({ "group": gid, "workspace": poc_id }),
        ));
        ok_json(post_json(
            "/cli/project-group/add-member",
            serde_json::json!({ "group": gid, "workspace": member_name }),
        ));

        // Human (author-less → owner): stored + emitted + ATTEMPTED
        // (delivery fails against the dead PoC workspace, but the
        // attempt is reported and the store survives).
        let mark = event_mark();
        let posted = ok_json(msg_as(&gid, None, "kickoff note"));
        assert_eq!(posted["author"], "owner");
        assert_eq!(posted["delivered"], false);
        assert_eq!(
            posted["deliveryReason"], "no_agent_mode",
            "owner post must ATTEMPT injection into the PoC: {posted}"
        );
        let events = events_for_group(mark, &gid);
        assert_eq!(
            events.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["project-group:message-created"],
            "msg emits message-created only: {events:?}"
        );
        assert_eq!(events[0].1["groupName"], name.as_str());
        assert_eq!(events[0].1["messageId"], posted["id"]);
        assert_eq!(events[0].1["author"], "owner");

        // Member agent (NOT the PoC): attempted too.
        let posted = ok_json(msg_as(&gid, Some(&member_name), "progress: half done"));
        assert_eq!(posted["author"], member_name.as_str());
        assert_eq!(posted["delivered"], false);
        assert_eq!(posted["deliveryReason"], "no_agent_mode");

        // The PoC itself: stored + emitted, NEVER attempted.
        let mark = event_mark();
        let posted = ok_json(msg_as(&gid, Some(&poc_name), "I hear you all"));
        assert_eq!(posted["delivered"], false);
        assert_eq!(
            posted["deliveryReason"], "author-is-poc",
            "the PoC never self-injects: {posted}"
        );
        assert_eq!(
            events_for_group(mark, &gid)
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            vec!["project-group:message-created"],
            "a PoC post still stores + emits"
        );

        // Non-member workspace: refused, NOT stored, nothing emitted.
        let (_stranger_id, stranger_name, _) = insert_workspace("stranger");
        let before = ok_json(messages(&gid, &[]))["messages"]
            .as_array()
            .expect("messages")
            .len();
        let mark = event_mark();
        let resp = msg_as(&gid, Some(&stranger_name), "let me in");
        assert_eq!(resp.status, "403 Forbidden", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "not_a_member");
        assert!(
            events_for_group(mark, &gid).is_empty(),
            "a refused post must not emit"
        );
        // An unresolvable author is refused the same way.
        let resp = msg_as(&gid, Some("no-such-workspace-zz"), "hi");
        assert_eq!(resp.status, "403 Forbidden", "body={}", resp.body);
        let after = ok_json(messages(&gid, &[]))["messages"]
            .as_array()
            .expect("messages")
            .len();
        assert_eq!(before, after, "refused posts must never store");

        // Store-survives-delivery-failure: all three accepted posts
        // are in the stream, oldest-first.
        let page = ok_json(messages(&gid, &[]));
        let bodies: Vec<&str> = page["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|m| m["body"].as_str().expect("body"))
            .collect();
        assert_eq!(
            bodies,
            vec!["kickoff note", "progress: half done", "I hear you all"],
            "every failed-delivery message is stored, in order"
        );

        // No-PoC-yet: a memberless group stores the owner's post with
        // deliveryReason no_poc (a NORMAL success) and never attempts.
        let empty = create_group_via_route(&gname("inject-empty"));
        let empty_id = empty["id"].as_str().expect("id");
        let posted = ok_json(msg_as(empty_id, None, "anyone home?"));
        assert_eq!(posted["delivered"], false);
        assert_eq!(posted["deliveryReason"], "no_poc");
        assert!(posted["deliveredSessionId"].is_null());

        // The §4.3 payload framing (deliver_live adds its own
        // `[from …]`, so the payload carries only the project tag).
        assert_eq!(
            project_payload("release-41", "ship it"),
            "[project:release-41] ship it"
        );
    }

    /// Messages route paging params: after (strictly greater) + limit
    /// + truncated riding the wire; bad params are loud usage errors.
    #[test]
    fn project_group_messages_route_paging() {
        let g = create_group_via_route(&gname("paging"));
        let gid = g["id"].as_str().expect("id").to_string();
        // 25 rows at controlled timestamps 1000..=1024.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            for i in 0..25i64 {
                conn.execute(
                    "INSERT INTO project_group_messages (id, group_id, author, body, created_at) \
                     VALUES (?1, ?2, 'owner', ?3, ?4)",
                    rusqlite::params![
                        uuid::Uuid::new_v4().to_string(),
                        gid,
                        format!("m-{i}"),
                        1000 + i
                    ],
                )
                .expect("insert message");
            }
        }

        // Default: the latest 20, oldest-first, truncated.
        let page = ok_json(messages(&gid, &[]));
        let msgs = page["messages"].as_array().expect("messages");
        assert_eq!(msgs.len(), 20);
        assert_eq!(page["truncated"], true);
        assert_eq!(msgs[0]["createdAt"], 1005);
        assert_eq!(msgs[19]["createdAt"], 1024);

        // after is strictly greater; limit caps + truncates.
        let page = ok_json(messages(&gid, &[("after", "1010")]));
        let msgs = page["messages"].as_array().expect("messages");
        assert_eq!(msgs[0]["createdAt"], 1011);
        assert_eq!(page["truncated"], false);
        let page = ok_json(messages(&gid, &[("after", "999"), ("limit", "10")]));
        assert_eq!(page["messages"].as_array().expect("messages").len(), 10);
        assert_eq!(page["truncated"], true);

        // Bad params fail loudly.
        for (k, v) in [("after", "yesterday"), ("limit", "many"), ("limit", "0")] {
            let resp = messages(&gid, &[(k, v)]);
            assert_eq!(resp.status, "400 Bad Request", "{k}={v} body={}", resp.body);
            let parsed: serde_json::Value =
                serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(parsed["error"]["code"], "usage", "{k}={v}");
        }
    }

    /// Dashboard save-layout: revision++, layout-changed event with
    /// {groupId, dashboardId, revision}, object-or-string layoutJson,
    /// group-ownership check, invalid-JSON refusal.
    #[test]
    fn project_group_dashboard_save_layout_route() {
        install_capture_sink();
        let g = create_group_via_route(&gname("dash"));
        let gid = g["id"].as_str().expect("id").to_string();
        let dash_id = ok_json(show(&gid))["dashboards"][0]["id"]
            .as_str()
            .expect("dashboard id")
            .to_string();

        // Object-shaped layoutJson (the natural renderer shape).
        let mark = event_mark();
        let saved = ok_json(post_json(
            "/cli/project-group/dashboard/save-layout",
            serde_json::json!({
                "group": gid,
                "dashboardId": dash_id,
                "layoutJson": { "version": 1, "columns": [] },
            }),
        ));
        assert_eq!(saved["revision"], 1);
        let events = events_for_group(mark, &gid);
        assert_eq!(
            events.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["project-group:layout-changed"]
        );
        assert_eq!(events[0].1["dashboardId"], dash_id.as_str());
        assert_eq!(events[0].1["revision"], 1);

        // String-shaped layoutJson; revision keeps climbing.
        let saved = ok_json(post_json(
            "/cli/project-group/dashboard/save-layout",
            serde_json::json!({
                "group": gid,
                "dashboardId": dash_id,
                "layoutJson": r#"{"version":1,"columns":[{"widthPct":100}]}"#,
            }),
        ));
        assert_eq!(saved["revision"], 2);

        // Unparseable string layout is refused; the blob is untouched.
        let resp = post_json(
            "/cli/project-group/dashboard/save-layout",
            serde_json::json!({ "group": gid, "dashboardId": dash_id, "layoutJson": "{nope" }),
        );
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
        let shown = ok_json(show(&gid));
        assert_eq!(shown["dashboards"][0]["revision"], 2, "failed save must not bump");

        // A dashboard belonging to ANOTHER group 404s under this group.
        let other = create_group_via_route(&gname("dash-other"));
        let other_dash = ok_json(show(other["id"].as_str().expect("id")))["dashboards"][0]["id"]
            .as_str()
            .expect("id")
            .to_string();
        let resp = post_json(
            "/cli/project-group/dashboard/save-layout",
            serde_json::json!({
                "group": gid,
                "dashboardId": other_dash,
                "layoutJson": { "version": 1 },
            }),
        );
        assert_eq!(resp.status, "404 Not Found", "body={}", resp.body);

        // Unknown dashboard id → 404; missing fields → usage.
        let resp = post_json(
            "/cli/project-group/dashboard/save-layout",
            serde_json::json!({ "group": gid, "dashboardId": "nope", "layoutJson": {} }),
        );
        assert_eq!(resp.status, "404 Not Found", "body={}", resp.body);
        let resp = post_json(
            "/cli/project-group/dashboard/save-layout",
            serde_json::json!({ "group": gid, "dashboardId": dash_id }),
        );
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
    }

    /// GET on the POST-only mutations answers an explicit 405 through
    /// the read dispatch chain (feedback_post_only_route_guards), and
    /// the POST dispatcher 404s unknown paths.
    #[test]
    fn project_group_mutations_405_on_get_and_post_404_unknown() {
        let params = std::collections::HashMap::new();
        for route in [
            "/cli/project-group/create",
            "/cli/project-group/rename",
            "/cli/project-group/delete",
            "/cli/project-group/pin",
            "/cli/project-group/sort",
            "/cli/project-group/add-member",
            "/cli/project-group/remove-member",
            "/cli/project-group/set-poc",
            "/cli/project-group/msg",
            "/cli/project-group/dashboard/save-layout",
        ] {
            let resp = dispatch(route, &params).expect("route claimed by GET chain");
            assert_eq!(resp.status, "405 Method Not Allowed", "route={route}");
            assert!(resp.body.contains("POST required"), "body={}", resp.body);
        }
        let resp = dispatch_post("/cli/project-group/unknown", b"{}");
        assert_eq!(resp.status, "404 Not Found");
        // Unclaimed paths stay unclaimed on the GET chain.
        assert!(dispatch("/cli/project-group/unknown", &params).is_none());
    }

    // ── §4.5 — the PoC workspace-removal guard ────────────────────────

    /// A workspace that is PoC in TWO groups is refused by every
    /// removal path with the exact resolved-Q6 shape; after
    /// reassignment (or for a plain member) removal proceeds.
    #[test]
    fn poc_removal_guard_blocks_every_removal_path() {
        // Two groups, one PoC workspace + a bystander member. Names
        // sort deterministically (poc_blocks_for_workspace orders by
        // name): use an explicit -a / -b suffix on a shared uuid stem.
        let stem = gname("guard");
        let name_a = format!("{stem}-a");
        let name_b = format!("{stem}-b");
        let ga = create_group_via_route(&name_a);
        let gb = create_group_via_route(&name_b);
        let ga_id = ga["id"].as_str().expect("id").to_string();
        let gb_id = gb["id"].as_str().expect("id").to_string();
        let (poc_id, _, poc_path) = insert_workspace("guard-poc");
        let (member_id, ..) = insert_workspace("guard-member");
        for gid in [&ga_id, &gb_id] {
            ok_json(post_json(
                "/cli/project-group/add-member",
                serde_json::json!({ "group": gid, "workspace": poc_id }),
            ));
            ok_json(post_json(
                "/cli/project-group/add-member",
                serde_json::json!({ "group": gid, "workspace": member_id }),
            ));
        }
        let display = k2_core::workspace::display::agent_display_name(&poc_path);
        let expected_hint = format!(
            "{display} is the Point of Contact for: {name_a}, {name_b}. Reassign the PoC first."
        );

        // The guard helper itself.
        let resp = poc_removal_block(&poc_id, &display).expect("PoC must be blocked");
        assert_eq!(resp.status, "409 Conflict", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "poc_of_projects");
        assert_eq!(v["error"]["hint"], expected_hint.as_str());

        // Path 1 — the UI's `/cli/projects/delete` route.
        let resp = crate::db_routes::handle_projects_delete(
            serde_json::json!({ "id": poc_id }).to_string().as_bytes(),
        );
        assert_eq!(resp.status, "409 Conflict", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "poc_of_projects");
        assert_eq!(v["error"]["hint"], expected_hint.as_str());

        // Path 2 — `k2 workspace remove` → `/cli/workspace/remove`.
        let params: std::collections::HashMap<String, String> =
            [("path".to_string(), poc_path.clone())].into_iter().collect();
        let resp = crate::workspace_routes::dispatch("/cli/workspace/remove", &params)
            .expect("remove route claimed");
        assert_eq!(resp.status, "409 Conflict", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "poc_of_projects");
        assert_eq!(v["error"]["hint"], expected_hint.as_str());

        // Path 3 — `k2 agent retire` → `/cli/agent/retire` (guard
        // fires BEFORE the git/secrets guards; force must NOT override).
        for force in [false, true] {
            let resp = crate::agent_retire::handle_agent_retire(
                serde_json::json!({ "q": poc_path, "force": force })
                    .to_string()
                    .as_bytes(),
            );
            assert_eq!(resp.status, "409 Conflict", "force={force} body={}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "poc_of_projects", "force={force}");
            assert_eq!(v["error"]["hint"], expected_hint.as_str(), "force={force}");
        }

        // Nothing was removed by any refused path.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let survives: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM projects WHERE id = ?1",
                    rusqlite::params![poc_id],
                    |r| r.get(0),
                )
                .expect("count");
            assert_eq!(survives, 1, "refused removal must change nothing");
        }

        // Reassign BOTH groups' PoC away → the block lifts.
        for gid in [&ga_id, &gb_id] {
            ok_json(post_json(
                "/cli/project-group/set-poc",
                serde_json::json!({ "group": gid, "workspace": member_id }),
            ));
        }
        assert!(
            poc_removal_block(&poc_id, &display).is_none(),
            "reassigned ex-PoC must be removable"
        );
        // …and the real removal path now succeeds (the ex-PoC is
        // still a plain MEMBER of both groups — plain membership never
        // blocks; §4.5 guards the PoC role only).
        let resp = crate::db_routes::handle_projects_delete(
            serde_json::json!({ "id": poc_id }).to_string().as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);

        // A workspace that is PoC nowhere passes the guard untouched.
        let (free_id, _, free_path) = insert_workspace("guard-free");
        assert!(poc_removal_block(&free_id, "free").is_none());
        let params: std::collections::HashMap<String, String> =
            [("path".to_string(), free_path)].into_iter().collect();
        let resp = crate::workspace_routes::dispatch("/cli/workspace/remove", &params)
            .expect("remove route claimed");
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
    }
}
