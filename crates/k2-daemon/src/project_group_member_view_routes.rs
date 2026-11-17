//! One agent's own view on a project (0302), split out of
//! `project_group_routes.rs` (the size ratchet): the
//! `POST /cli/project-group/set-member-default-view` route, and the
//! per-member fields `GET /cli/project-group/show` adds — `sessionAlive` /
//! `sessionId` (the answer `sessions/lookup-by-agent` gives, so the nav
//! needs no call per agent) and `defaultView` / `ownDefaultView`.
//!
//! The route rides the project-group POST arm: `dispatch_post_as` applies
//! the "Manage K2 projects" switch (`is_gated_mutation`) and the
//! owner-or-admin rule (`is_admin_only`, with [`refusal_or`]'s hint), the
//! same rule as `set-default-view`: it changes the view everyone's tile for
//! that agent opens in. It emits `project-group:members-changed`. The store
//! is `k2_core::project_groups` (`member_default_views`,
//! `set_member_default_view`).

use crate::cli_response::CliResponse;
use crate::project_group_routes::{
    core_error, emit, error_response, parse_member_body, resolve_error_response, usage_error,
};
use k2_core::project_groups::{self, ResolveError};

/// `POST /cli/project-group/set-member-default-view` (0302): the view one
/// agent's tile opens in on the project's dashboards, for everyone.
pub const MEMBER_VIEW_PATH: &str = "/cli/project-group/set-member-default-view";

/// Whether `path` is this module's route.
pub fn is_path(path: &str) -> bool {
    path == MEMBER_VIEW_PATH
}

/// The project-group GET chain's fallback: 405 for this POST-only route
/// (feedback_post_only_route_guards), `None` (unclaimed) for anything else.
pub fn get_chain(path: &str) -> Option<CliResponse> {
    is_path(path).then(CliResponse::method_not_allowed)
}

/// The project-group POST dispatch's fallback: this route, else 404.
pub fn dispatch_post(path: &str, body: &[u8]) -> CliResponse {
    if is_path(path) {
        handle_set_member_default_view(body)
    } else {
        CliResponse::not_found()
    }
}

/// The refusal a non-admin login gets: this route's own hint, else
/// `otherwise` (the dashboards-and-appearance one).
pub fn refusal_or(path: &str, otherwise: CliResponse) -> CliResponse {
    if !is_path(path) {
        return otherwise;
    }
    error_response(
        "403 Forbidden",
        "forbidden",
        "only the owner, an admin, or an agent with 'Let this agent manage K2 projects' on can change \
         the view an agent opens in on a project (it changes it for everyone)",
    )
}

/// `show`'s `members` array: each member as built by `build_show_json`,
/// plus `sessionAlive` / `sessionId` and `defaultView` / `ownDefaultView`.
pub fn show_fields(
    group_id: &str,
    project_view: &str,
    mut members: Vec<serde_json::Value>,
) -> Result<serde_json::Value, CliResponse> {
    let own_views = project_groups::member_default_views(group_id).map_err(core_error)?;
    for m in members.iter_mut() {
        let Some(workspace_id) = m["workspaceId"].as_str().map(str::to_string) else { continue };
        // The same answer `GET /cli/sessions/lookup-by-agent?agent=<id>`
        // gives (the canonical session is keyed by the workspace id), so
        // the nav needs no call per agent. A snapshot: no event moves it.
        let live = crate::session_lookup::lookup_any(&workspace_id);
        let own_view = own_views.get(&workspace_id);
        m["sessionAlive"] = serde_json::json!(live.is_some());
        m["sessionId"] = serde_json::json!(live.map(|l| l.session_id().to_string()));
        // 0302: the view this agent's tile opens in (its own, else the
        // project's), and its own (null = follows the project).
        m["defaultView"] = serde_json::json!(own_view.map(String::as_str).unwrap_or(project_view));
        m["ownDefaultView"] = serde_json::json!(own_view);
    }
    Ok(serde_json::Value::Array(members))
}

/// Handler for `POST /cli/project-group/set-member-default-view` (0302):
/// `{group, workspace, view}`. `workspace` is a member's name, path or id;
/// `view` is `terminal`, `thread` or `chatter` (that agent's own view, for
/// everyone) or `null` (it follows the project's `defaultView` again). The
/// key is required, so a client that forgets it never clears a view by
/// accident. Same rule as `set-default-view` (checked in
/// `dispatch_post_as`). Answers `{ok, groupId, workspaceId, defaultView,
/// ownDefaultView}` (the member's `show` fields) and emits
/// `project-group:members-changed`. Never injects.
pub fn handle_set_member_default_view(body: &[u8]) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    let view = match v.get("view") {
        None => {
            return usage_error(
                "missing 'view' (terminal, thread or chatter; null makes the agent follow the project's view)",
            )
        }
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(other) => {
            return usage_error(format!("view must be terminal, thread or chatter, got {other}"))
        }
    };
    let (group_id, workspace_id, _) = match parse_member_body(body) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    if let Err(e) = project_groups::set_member_default_view(&group_id, &workspace_id, view.as_deref()) {
        return core_error(e);
    }
    let Some(group) = project_groups::get_group_by_id(&group_id) else {
        return resolve_error_response(&group_id, ResolveError::NotFound);
    };
    emit(
        k2_core::agent_hooks::HookEvent::ProjectGroupMembersChanged,
        serde_json::json!({ "groupId": group_id }),
    );
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "groupId": group_id,
            "workspaceId": workspace_id,
            "defaultView": view.as_deref().unwrap_or(&group.default_view),
            "ownDefaultView": view,
        })
        .to_string(),
    )
}

#[cfg(test)]
#[path = "project_group_member_view_routes_tests.rs"]
mod tests;
