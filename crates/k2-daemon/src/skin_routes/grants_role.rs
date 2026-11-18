//! `POST /cli/skin/grants/role` (`k2 app grant role <grant-id> --role <r>`)
//! by an App's own agent (0.46.16, Rosson 2026-11-18).
//!
//! An agent with "Let this agent manage its Apps' users" moves one login of
//! its OWN App onto another role in place, as `k2 app user role` gives a guest
//! a role: the same rule ([`super::agent_may_touch`] on
//! `/cli/skin/roles/assign`: the person wholly inside its workspace, never a
//! box login, never another workspace's test guest), never an agent's login,
//! and the role is that App's or a box-wide one inside its workspace. By grant
//! id, so a person who signs in to several of its Apps moves on the one App
//! named (`assign_login_role` asks for this then). The grant keeps its id, its
//! on/off and its live sessions ([`super::handle_grants_role`]). Any other
//! grant (another workspace's App, the box login, a chat or mail grant) stays
//! the owner's. Without the switch the dispatcher's gate answers first
//! (`app_users_off`, whose Ask-with line asks for the switch).
//!
//! The owner (`agent_ws` = `None`) is as before: the handler alone.

use k2_core::skin;

use super::{agent_may_touch, agent_test_guest_refused, app_of_workspace, outside_workspace, owner_only_with_hint, str_field};
use crate::cli_response::CliResponse;

pub const GRANTS_ROLE: &str = "/cli/skin/grants/role";

/// `/cli/skin/roles/unassign` and [`GRANTS_ROLE`] share one dispatcher arm
/// (the same gate): each goes to its own handler.
pub fn unassign_or_role(agent_ws: Option<&str>, path: &str, body: &[u8], actor: &str) -> CliResponse {
    if path != GRANTS_ROLE {
        return super::agent_scoped(agent_ws, "/cli/skin/roles/unassign", body, actor, super::handle_roles_unassign);
    }
    let Some(ws) = agent_ws else { return super::handle_grants_role(body, actor) };
    let app = match agent_may_rerole(ws, body, actor) {
        Ok(app) => app,
        Err(r) => {
            k2_core::log_debug!("[skin] actor={actor} refused outside its workspace on {GRANTS_ROLE}");
            return r;
        }
    };
    let r = skin::guest_rooms::written_by(skin::guest_rooms::WrittenBy::Agent(actor.to_string()), || super::handle_grants_role(body, actor));
    super::audit_write(GRANTS_ROLE, body, actor, Some(ws), app.into_iter().collect(), &r);
    r
}

/// The grant `id` (trimmed), if there is one.
fn grant_named(id: &str) -> Result<Option<skin::SkinGrant>, CliResponse> {
    let id = id.trim();
    skin::list_grants().map(|gs| gs.into_iter().find(|g| g.id == id)).map_err(CliResponse::internal_error)
}

/// May agent workspace `ws` re-role the grant `body` names? `Ok(app)`: yes,
/// on that App (`None`: a body or grant the handler answers itself).
fn agent_may_rerole(ws: &str, body: &[u8], actor: &str) -> Result<Option<String>, CliResponse> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else { return Ok(None) };
    let Some(id) = str_field(&v, &["id", "grantId", "grant_id"]) else { return Ok(None) };
    let Some(g) = grant_named(id)? else { return Ok(None) };
    if g.kind != "app" || g.subject_kind != "principal" || g.target_id == skin::HOST_APP_ID || !app_of_workspace(&g.target_id, ws) {
        return Err(outside_workspace(format!("grant {} isn't a login on one of your Apps", g.id)));
    }
    let Some(p) = skin::list_principals().map_err(CliResponse::internal_error)?.into_iter().find(|p| p.id == g.subject_id) else {
        return Ok(None);
    };
    if skin::principal_agent_workspace(&p.id).map_err(CliResponse::internal_error)?.is_some() {
        return Err(owner_only_with_hint(&format!("'{}' is an agent's login: only the owner changes an agent's access to an App", p.username)));
    }
    if let Some(r) = agent_test_guest_refused(actor, &p.username) {
        return Err(r);
    }
    let role = str_field(&v, &["role", "roleId", "role_id"]);
    if let Some(r) = role.map(skin::find_role).transpose().map_err(CliResponse::internal_error)?.flatten() {
        if r.app_id.as_deref().is_some_and(|a| a != g.target_id) {
            return Err(outside_workspace(format!("role '{}' belongs to another App", r.name)));
        }
    }
    // The rest is giving that person a role (`k2 app user role`).
    let assign = serde_json::json!({ "username": p.username, "role": role }).to_string();
    agent_may_touch(ws, "/cli/skin/roles/assign", assign.as_bytes())?;
    Ok(Some(g.target_id))
}
