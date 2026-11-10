//! Who is calling an Org Chart route (prd-org-chart-v1 §6.1 "Agent
//! identity", §7). Identity is never taken from the request: an agent is
//! the workspace (and session address) its passport was minted for.
//!
//! - the owner token, a K2 shell tab (a person typing in K2, noun tiers
//!   NT5) → the owner, a person;
//! - a Connect login → that person, Admin powers when Owner or Admin (its
//!   role already cleared the route floor in `route_policy::role_gate`);
//! - an agent passport, on Loopback TCP or its own cell socket → the agent;
//! - App passes, API keys, an API session, a passport from off this
//!   machine, or nothing valid → 403.

use k2_core::connect_users::Role;
use k2_core::org_chart::store::Actor;

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::session_token::HookPrincipal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgCaller {
    /// `admin` = Owner or Admin.
    Person { label: String, admin: bool },
    /// An agent: its workspace, its session and its address (the canonical
    /// agent's project id, or a sidecar's handle; read state is per
    /// address).
    Agent { project_id: String, session_id: String, agent_address: String, label: String },
}

impl OrgCaller {
    pub fn label(&self) -> &str {
        match self {
            OrgCaller::Person { label, .. } | OrgCaller::Agent { label, .. } => label,
        }
    }

    pub fn is_admin(&self) -> bool {
        matches!(self, OrgCaller::Person { admin: true, .. })
    }

    pub fn agent_id(&self) -> Option<&str> {
        match self {
            OrgCaller::Agent { project_id, .. } => Some(project_id),
            OrgCaller::Person { .. } => None,
        }
    }

    /// The store's actor.
    pub fn actor(&self) -> Actor {
        match self {
            OrgCaller::Person { label, admin } => Actor::Person { label: label.clone(), admin: *admin },
            OrgCaller::Agent { project_id, label, .. } => Actor::Agent { project_id: project_id.clone(), label: label.clone() },
        }
    }
}

pub(crate) fn refusal(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

fn handle(project_id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(project_id).unwrap_or_else(|_| project_id.to_string())
}

/// A passport's session, classified (cell socket, or Loopback TCP).
pub fn from_passport(session_id: &str, principal: &HookPrincipal) -> Result<OrgCaller, CliResponse> {
    match crate::compute_routes::who_from_cell(session_id, principal) {
        Who::Shell { .. } => Ok(OrgCaller::Person { label: "owner".into(), admin: true }),
        Who::Agent { project_id, session_id } if !project_id.trim().is_empty() => {
            let label = format!("agent:{}", handle(&project_id));
            Ok(OrgCaller::Agent { project_id, session_id, agent_address: principal.agent_address.clone(), label })
        }
        Who::Agent { .. } => Err(refusal("403 Forbidden", "forbidden", "this session's passport has no workspace")),
        _ => Err(refusal("403 Forbidden", "wrong_credential", "this credential can't use k2 organization")),
    }
}

/// A TCP caller. `query` is the dispatcher's auth query (Bearer / cookie
/// folded in); `loopback` = the request came from this machine.
pub fn from_tcp(path: &str, query: &str, bearer: Option<&str>, owner_token: &str, loopback: bool) -> Result<OrgCaller, CliResponse> {
    let presented = crate::routes::http::extract_token(query)
        .filter(|s| !s.is_empty())
        .or_else(|| bearer.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("");
    if presented.is_empty() {
        return Err(CliResponse::forbidden());
    }
    if k2_core::skin::is_skin_token(presented) || presented.starts_with("k2sk_") {
        return Err(refusal("403 Forbidden", "wrong_credential", "an App pass or API key can't use k2 organization"));
    }
    if crate::routes::http::ct_eq_token(presented, owner_token) {
        return Ok(OrgCaller::Person { label: "owner".into(), admin: true });
    }
    if let Some(username) = k2_core::connect_users::validate_session(presented) {
        // An unreadable role fails closed to the lowest.
        let role = k2_core::connect_users::role_for_user(&username).unwrap_or(Role::VoiceChat);
        if role < Role::Member {
            return Err(refusal("403 Forbidden", "role_required", "This needs a member login on this server."));
        }
        return Ok(OrgCaller::Person { label: format!("login:{username}"), admin: role >= Role::Admin });
    }
    if let Some(v) = crate::session_token::require_hook(presented, path) {
        if !loopback {
            return Err(refusal("403 Forbidden", "remote_passport", "an agent's passport works only on this machine"));
        }
        return from_passport(&v.session_id, &v.principal);
    }
    Err(CliResponse::forbidden())
}
