//! Granting the Kessel and Chief of Staff starter sets
//! (`k2_core::hire::starter_permissions`; prd-setup-checklist-v1 §10.2).
//!
//! **Standing switches only**, each to its own workspace, once, when the
//! agent is made. Two kinds of grantor:
//! - **A person** (the owner token, an Owner or Admin login) making Kessel:
//!   each switch goes through **the Settings route's own entry** with that
//!   person as the caller (`sidecar_routes::access_entry`,
//!   `misc_routes::connections_switch_entry`), so the route's floor and its
//!   `agent-access-set` audit line are exactly Settings'. An Owner-floor
//!   switch (`organization.manage`) is left out for an Admin: Kessel asks an
//!   Owner on a card.
//! - **Kessel** hiring the Chief of Staff (the seat-only route): it passes on
//!   only the standing switches it holds itself, never an Owner-floor one.
//!   `/cli/agent-access/set` is not an agent verb, so these are written
//!   in-process through the same single writers, and audited with Kessel as the
//!   actor.
//!
//! The permission-cards switch ("Let this agent ask for permission") is
//! turned on the same two ways ([`cards_on`]).

use k2_core::hire::starter_permissions::{owner_floor, standing_switch};
use serde_json::json;

use crate::permission::actions::Answerer;

/// Who grants.
#[derive(Debug, Clone)]
pub(crate) enum Grantor {
    Person(Answerer),
    /// Kessel's workspace id (the seat's), hiring for its person.
    Agent { workspace_id: String },
}

impl Grantor {
    fn audit_user(&self) -> String {
        match self {
            Grantor::Person(Answerer::OwnerToken) => "owner".to_string(),
            Grantor::Person(Answerer::Login { user, .. }) => format!("login:{user}"),
            Grantor::Agent { workspace_id } => format!("agent:{}", handle_of(workspace_id)),
        }
    }

    /// The owner token or an Owner login (an Owner-floor switch needs one).
    fn is_owner(&self) -> bool {
        match self {
            Grantor::Person(Answerer::OwnerToken) => true,
            Grantor::Person(Answerer::Login { user, .. }) => {
                k2_core::connect_users::role_for_user(user) == Some(k2_core::connect_users::Role::Owner)
            }
            Grantor::Agent { .. } => false,
        }
    }
}

/// The new agent's workspace.
#[derive(Debug, Clone)]
pub(crate) struct Target {
    pub id: String,
    pub path: String,
    pub handle: String,
}

/// What was granted and what was left for a card, with why.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Granted {
    pub granted: Vec<String>,
    /// `{action, reason}`: `owner_floor`, `hirer_lacks`, or the route's
    /// refusal code.
    pub left: Vec<serde_json::Value>,
}

fn handle_of(id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(id).unwrap_or_else(|_| id.to_string())
}

fn path_of(id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::Project::get(&conn, id).ok().map(|p| p.path)
}

/// Is `switch` on for workspace `id` (its own switch, not a server master)?
pub(crate) fn switch_on(id: &str, switch: &str) -> bool {
    use k2_core::workspace::settings as ws;
    match switch {
        "agents_manage" => ws::agents_can_manage_agents(id),
        "projects_manage" => ws::agents_can_manage_projects(id),
        "agents_can_create_connections" => path_of(id).is_some_and(|p| ws::get_agents_can_create_connections(&p)),
        "org_chart_manage" => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::org_chart::store::agents_can_manage(&conn, id)
        }
        "compute" => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::compute::store::agents_can_use_compute(&conn, id)
        }
        "chat_refresh" => ws::agents_can_refresh_chat(id),
        "gardens_manage" => ws::agents_can_manage_gardens(id),
        "permission_cards" => ws::agents_can_request_permissions(id),
        _ => false,
    }
}

/// Write one switch on, in-process (the agent-hirer path): the same single
/// writers `handle_access` and `/cli/agents-create-connections` use.
fn write_on(t: &Target, switch: &str, by: &str) -> Result<(), String> {
    use k2_core::workspace::settings as ws;
    match switch {
        "agents_manage" => ws::set_agents_can_manage_agents(&t.id, true)?,
        "projects_manage" => ws::set_agents_can_manage_projects(&t.id, true)?,
        "agents_can_create_connections" => ws::update_project_setting(&t.path, "agents_can_create_connections", "1")?,
        "compute" => {
            crate::compute_routes::set_agent_compute(&t.id, &t.handle, true, &k2_core::compute::store::GrantLimits::default(), by)?;
        }
        "chat_refresh" => ws::set_agents_can_refresh_chat(&t.id, true)?,
        "gardens_manage" => ws::set_agents_can_manage_gardens(&t.id, true)?,
        "permission_cards" => {
            ws::set_agents_can_request_permissions(&t.id, true)?;
            k2_core::workspace::skill_regen::sync_permission_teaching(&t.path);
        }
        other => return Err(format!("K2 doesn't grant {other} this way")),
    }
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncProjects, serde_json::Value::Null);
    Ok(())
}

fn audit(by: &str, outcome: String) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new("agent-access-set", by, outcome, "first-run", "-", "cli"));
}

/// One switch through the Settings route's entry, as the person.
fn person_on(who: &Answerer, t: &Target, switch: &str) -> crate::cli_response::CliResponse {
    if switch == "agents_can_create_connections" {
        return crate::misc_routes::connections_switch_entry(who, &t.path, true);
    }
    crate::sidecar_routes::access_entry(who, &json!({ "toggle": switch, "workspace": t.id, "value": true }))
}

fn refusal_code(r: &crate::cli_response::CliResponse) -> String {
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or(serde_json::Value::Null);
    v.pointer("/error/code")
        .or_else(|| v.get("error").filter(|e| e.is_string()))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("refused")
        .to_string()
}

/// Turn on one switch (`switch`, a toggle name) for `t` as `by`. `Err` is
/// the reason it stayed off.
fn grant_switch(by: &Grantor, t: &Target, switch: &str) -> Result<(), String> {
    match by {
        Grantor::Person(who) => {
            let r = person_on(who, t, switch);
            if r.status.starts_with('2') {
                Ok(())
            } else {
                Err(refusal_code(&r))
            }
        }
        Grantor::Agent { workspace_id } => {
            if !switch_on(workspace_id, switch) {
                return Err("hirer_lacks".into());
            }
            let actor = by.audit_user();
            write_on(t, switch, &actor)?;
            audit(&actor, format!("ok {} {switch}=true (starter set)", t.handle));
            Ok(())
        }
    }
}

/// A starter list plus the standing toggles Rosson's sets name that have
/// no V1 action id (`EXTRA_STANDING_TOGGLES`: `chat.refresh`'s twin,
/// "Can refresh its own pinned chat"). They are granted beside the list,
/// under the same rules, so the lists stay V1 standing ids only.
pub(crate) fn with_extras(set: &[&'static str]) -> Vec<&'static str> {
    let mut out = set.to_vec();
    out.extend(k2_core::hire::starter_permissions::EXTRA_STANDING_TOGGLES.iter().map(|(a, _)| *a));
    out
}

/// Grant `set` (starter action ids) to `t`, following the module's rules.
pub(crate) fn grant_set(set: &[&str], t: &Target, by: &Grantor) -> Granted {
    let mut out = Granted::default();
    for action in set {
        let left = |reason: &str| json!({ "action": action, "reason": reason });
        let Some(switch) = standing_switch(action) else {
            out.left.push(left("not_standing"));
            continue;
        };
        if owner_floor(action) && !by.is_owner() {
            out.left.push(left("owner_floor"));
            continue;
        }
        match grant_switch(by, t, switch) {
            Ok(()) => out.granted.push(action.to_string()),
            Err(reason) => out.left.push(left(&reason)),
        }
    }
    out
}

/// "Let this agent ask for permission" on for `t`. A person turns it on
/// through Settings' entry; Kessel turns it on for its hire (the seat-only
/// route is that authority), whether or not Kessel's own is on.
pub(crate) fn cards_on(t: &Target, by: &Grantor) -> Result<(), String> {
    match by {
        Grantor::Person(_) => grant_switch(by, t, "permission_cards"),
        Grantor::Agent { .. } => {
            let actor = by.audit_user();
            write_on(t, "permission_cards", &actor)?;
            audit(&actor, format!("ok {} permission_cards=true (starter set)", t.handle));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use k2_core::hire::starter_permissions::{CHIEF_OF_STAFF_STARTER_PERMISSIONS, EXTRA_STANDING_TOGGLES, KESSEL_STARTER_PERMISSIONS};
    use k2_core::permission::catalog::{Floor, Grant};

    /// Both starter lists are standing V1 actions in the real catalog
    /// (the agent-templates loader checks the same), each with the floor
    /// the module's Owner rule assumes; the one extra is a one-time action
    /// whose standing twin is a toggle, granted beside the lists.
    #[test]
    fn the_starter_lists_are_standing_v1_actions_with_their_floors() {
        for id in KESSEL_STARTER_PERMISSIONS.iter().chain(CHIEF_OF_STAFF_STARTER_PERMISSIONS) {
            assert!(crate::permission::walls::V1_ACTIONS.contains(id), "{id} is not a V1 action");
            let r = crate::permission::actions::find(id).unwrap_or_else(|| panic!("{id} is not registered"));
            assert_eq!(r.spec.grant, Grant::Standing, "{id} must be standing");
            assert_eq!(
                r.spec.floor == Floor::Owner,
                k2_core::hire::starter_permissions::owner_floor(id),
                "{id}: the Owner-floor rule matches the catalog"
            );
        }
        for (action, toggle) in EXTRA_STANDING_TOGGLES {
            let r = crate::permission::actions::find(action).unwrap_or_else(|| panic!("{action}"));
            assert_eq!(r.spec.grant, Grant::OneTime, "{action} is one-time; its twin {toggle} is granted instead");
            assert!(!KESSEL_STARTER_PERMISSIONS.contains(action) && !CHIEF_OF_STAFF_STARTER_PERMISSIONS.contains(action));
        }
    }
}
