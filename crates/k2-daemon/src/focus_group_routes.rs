//! Who may change focus groups: `POST /cli/focus-groups/create`, `update`
//! and `assign` (Settings → Workspaces, the sidebar's group menu, and
//! `k2 focus-group create|rename|add|remove`, `k2 agent set --focus-group`).
//!
//! **Who.** A person: the owner token, a Connect login (the route's Member
//! floor, `route_policy`), or a passport from a plain shell tab (a person
//! typing in K2). An agent passport never changes a group itself: it gets
//! `403 focus_group_needs_person`, naming the group and the workspace, and
//! with permission cards on that refusal ends with the card to ask with
//! (`permission::actions::focus_groups`: `focus-groups.create`,
//! `focus-groups.rename`, `focus-groups.assign`). The owner or an admin
//! taps Allow and the card runs these same routes as them.
//!
//! The list (`/cli/focus-groups/list`) stays open to agents
//! (`passport_routes`, a shared read). `delete` and `reconcile` keep the
//! owner token or a login, as before.
//!
//! Over the cell socket these three routes are agent verbs only so the
//! passport reaches TCP: the socket answers them "not served", the CLI
//! sends them again over TCP with the same passport, and the TCP arm
//! classifies it (`sidecar_routes::caller_from_tcp`).

use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;
use k2_core::{cli_name, product_name};

pub const CREATE: &str = "/cli/focus-groups/create";
pub const UPDATE: &str = "/cli/focus-groups/update";
pub const ASSIGN: &str = "/cli/focus-groups/assign";

/// An agent asked to change a focus group.
pub const NEEDS_PERSON: &str = "focus_group_needs_person";

/// The permission-card actions an agent asks with.
pub const CREATE_ACTION: &str = "focus-groups.create";
pub const RENAME_ACTION: &str = "focus-groups.rename";
pub const ASSIGN_ACTION: &str = "focus-groups.assign";

/// The group value that means "in no group" (`k2 agent set --focus-group none`).
pub const NONE: &str = "none";

/// The three routes that change a group or who is in it.
pub fn is_change(path: &str) -> bool {
    matches!(path, CREATE | UPDATE | ASSIGN)
}

/// The answer for one of [`is_change`]'s routes, by who is asking.
pub fn handle_as(path: &str, body: &[u8], caller: &Caller) -> CliResponse {
    match caller {
        Caller::Owner | Caller::Login { .. } | Caller::Shell { .. } => crate::db_routes::dispatch_unit4_post(path, body),
        Caller::Agent(_) => needs_person(path, body),
        Caller::WrongCredential | Caller::Invalid => invalid_token(),
    }
}

/// Today's answer to a request with no credential these routes take.
pub fn invalid_token() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: r#"{"error":"invalid or missing token"}"#.to_string(),
    }
}

/// A group's name by its id, or the id itself when no group has it.
pub(crate) fn group_name(id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::FocusGroup::get(&conn, id).map(|g| g.name).unwrap_or_else(|_| id.to_string())
}

/// The one group a selector names: its id, its name (any case), or the
/// start of exactly one name (two or more is ambiguous, and names them).
pub(crate) fn resolve_group(sel: &str) -> Result<k2_core::db::schema::FocusGroup, String> {
    let sel = sel.trim();
    let groups = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::FocusGroup::list(&conn).map_err(|e| e.to_string())?
    };
    let lower = sel.to_lowercase();
    let mut hits: Vec<_> = groups.iter().filter(|g| g.id == sel || g.name.to_lowercase() == lower).cloned().collect();
    if hits.is_empty() {
        hits = groups.iter().filter(|g| g.name.to_lowercase().starts_with(&lower)).cloned().collect();
    }
    match hits.len() {
        1 => Ok(hits.remove(0)),
        0 => Err(format!("no focus group matches '{sel}'")),
        _ => {
            let mut names: Vec<String> = hits.into_iter().map(|g| g.name).collect();
            names.sort();
            Err(format!("'{sel}' matches more than one focus group: {}", names.join(", ")))
        }
    }
}

/// A workspace's address name by its id, or the id itself.
pub(crate) fn workspace_handle(id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(id).unwrap_or_else(|_| id.to_string())
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// The refusal an agent gets. It names what the request would change, so
/// the walls map can teach the card with those values: `name` (create),
/// `group` and `to` (a rename), `target` and `group` (assign; `none` for
/// out of every group). A colour or order change carries no `to`, so it
/// teaches no card.
pub(crate) fn needs_person(path: &str, body: &[u8]) -> CliResponse {
    let req: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let mut error = serde_json::Map::new();
    error.insert("code".into(), json!(NEEDS_PERSON));
    let person = concat!(
        "the owner, an admin or a person signed in to this server does it (Settings → Workspaces / Agents, or ",
        cli_name!(),
        " focus-group from a terminal outside ",
        product_name!(),
        ")"
    );
    let hint = match path {
        CREATE => {
            let name = text(&req, "name").unwrap_or_default();
            error.insert("name".into(), json!(name));
            format!("Creating focus group '{name}' is a person's step: {person}.")
        }
        UPDATE => {
            let group = text(&req, "id").map(|id| group_name(&id)).unwrap_or_default();
            error.insert("group".into(), json!(group));
            match text(&req, "name") {
                Some(to) => {
                    error.insert("to".into(), json!(to));
                    format!("Renaming focus group '{group}' to '{to}' is a person's step: {person}.")
                }
                None => format!("Changing focus group '{group}' is a person's step: {person}."),
            }
        }
        _ => {
            let target = text(&req, "projectId").map(|id| workspace_handle(&id)).unwrap_or_default();
            let group = text(&req, "focusGroupId").map(|id| group_name(&id)).unwrap_or_else(|| NONE.to_string());
            error.insert("target".into(), json!(target));
            error.insert("group".into(), json!(group));
            if group == NONE {
                format!("Taking {target} out of its focus group is a person's step: {person}.")
            } else {
                format!("Putting {target} in focus group '{group}' is a person's step: {person}.")
            }
        }
    };
    error.insert("hint".into(), json!(hint));
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: json!({ "error": Value::Object(error) }).to_string(),
    }
}

#[cfg(test)]
#[path = "focus_group_routes_tests.rs"]
mod tests;
