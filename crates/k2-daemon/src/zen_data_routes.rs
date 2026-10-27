//! `/cli/zen/data/*` and `/cli/zen/call`: Garden data routes
//! (prd-gardens-local-services-v1 1A; contract-garden-data-v1 §4).
//! C0 → B1.
//!
//! Two doors end in the same store calls:
//! - **`POST /cli/zen/call`**: the widget door. The app's renderer, owner
//!   token only, Origin-gated; a widget's data calls run with its maker's
//!   rights (`garden_data::call`, B2). Never on the cell socket.
//! - **`/cli/zen/data/*`**: the agent and person door (`k2 store --widget`,
//!   `k2 zen data`, Settings). Agents and K2 shell tabs (agent rules) are
//!   checked against `zen_data_grants` on every call; person routes refuse
//!   a passport with 403 `person_only`. Never `/cli/store/*` (GL28): app
//!   guest sessions read `/cli/store/*`, and a widget keyspace must never
//!   reach the App gateway.
//!
//! Every policy row is `NoLogin`: a Connect login never reaches Garden
//! data (Gardens belong to this computer's OS user). App passes, API keys,
//! stream tokens and sandboxed/API cells are 403 `apps_not_allowed`.
//! Both transports refuse a GET to a POST row with 405 before the handler.

use std::collections::HashMap;

use k2_core::garden_data::blobs::MAX_BLOB_BODY;
use k2_core::garden_data::{Actor, DataError, MAX_BODY, MAX_IMPORT_BYTES};

use crate::cli_response::CliResponse;
use crate::garden_data::{self as gd, refused};
use crate::sidecar_routes::Caller;

/// The widget door.
pub const CALL_ROUTE: &str = "/cli/zen/call";

/// The import route (the one body over 64 KiB).
pub const IMPORT_ROUTE: &str = "/cli/zen/data/import";

/// The widget door's byte routes (prd-garden-board-needs-v1 GB13): owner
/// token and Origin gate, like `/cli/zen/call`; never on the cell socket.
pub const WIDGET_DOOR: &[&str] = &[CALL_ROUTE, "/cli/zen/blob/get", "/cli/zen/blob/put"];

/// Every Garden data route (contract §4.3).
pub const ROUTES: &[&str] = &[
    "/cli/zen/blob/get",
    "/cli/zen/blob/put",
    "/cli/zen/call",
    "/cli/zen/data/admin",
    "/cli/zen/data/admin/revoke",
    "/cli/zen/data/batch",
    "/cli/zen/data/blob/get",
    "/cli/zen/data/blob/list",
    "/cli/zen/data/blob/put",
    "/cli/zen/data/changes",
    "/cli/zen/data/delete",
    "/cli/zen/data/export",
    "/cli/zen/data/get",
    "/cli/zen/data/grant",
    "/cli/zen/data/grants",
    "/cli/zen/data/import",
    "/cli/zen/data/list",
    "/cli/zen/data/patch",
    "/cli/zen/data/put",
    "/cli/zen/data/revoke",
    "/cli/zen/data/show",
    "/cli/zen/data/space/delete",
    "/cli/zen/data/space/move",
    "/cli/zen/data/spaces",
    "/cli/zen/data/tx/abort",
    "/cli/zen/data/tx/add",
    "/cli/zen/data/tx/begin",
    "/cli/zen/data/tx/commit",
];

/// POST-only rows (405 on GET).
pub const POST_ROUTES: &[&str] = &[
    "/cli/zen/blob/put",
    "/cli/zen/call",
    "/cli/zen/data/admin",
    "/cli/zen/data/admin/revoke",
    "/cli/zen/data/batch",
    "/cli/zen/data/blob/put",
    "/cli/zen/data/delete",
    "/cli/zen/data/grant",
    "/cli/zen/data/import",
    "/cli/zen/data/patch",
    "/cli/zen/data/put",
    "/cli/zen/data/revoke",
    "/cli/zen/data/space/delete",
    "/cli/zen/data/space/move",
    "/cli/zen/data/tx/abort",
    "/cli/zen/data/tx/add",
    "/cli/zen/data/tx/begin",
    "/cli/zen/data/tx/commit",
];

/// Person-only rows: an agent or shell passport gets 403 `person_only`.
pub const PERSON_ROUTES: &[&str] = &[
    "/cli/zen/data/admin",
    "/cli/zen/data/admin/revoke",
    "/cli/zen/data/import",
    "/cli/zen/data/show",
    "/cli/zen/data/space/delete",
    "/cli/zen/data/space/move",
    "/cli/zen/data/spaces",
];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

/// The routes the per-cell socket serves (every one but the widget door).
pub fn is_agent_route(p: &str) -> bool {
    is_route(p) && !WIDGET_DOOR.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

/// The body cap for a route.
pub fn max_body(p: &str) -> usize {
    if p == IMPORT_ROUTE {
        MAX_IMPORT_BYTES
    } else if p == crate::garden_data::blob_routes::WIDGET_PUT || p == crate::garden_data::blob_routes::AGENT_PUT {
        MAX_BLOB_BODY
    } else {
        MAX_BODY
    }
}

/// Who called (contract §4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataWho {
    /// This daemon's owner token (a terminal outside K2, or the app).
    Owner,
    /// A harness session's passport.
    Agent { project_id: String },
    /// A K2 shell tab's passport: agent rules (GL42).
    Shell { project_id: String },
    /// A Connect login (refused: `zen_local_only`).
    Login,
    /// App pass, API key, stream token, sandboxed or API cell.
    Wrong,
    /// No valid credential.
    Invalid,
}

impl DataWho {
    /// The workspace whose grants apply (agent and shell).
    pub fn project_id(&self) -> Option<&str> {
        match self {
            DataWho::Agent { project_id } | DataWho::Shell { project_id } => Some(project_id),
            _ => None,
        }
    }

    /// The stored writer for this caller.
    pub fn actor(&self) -> Option<Actor> {
        match self {
            DataWho::Owner => Some(Actor::Owner),
            DataWho::Agent { project_id } | DataWho::Shell { project_id } => {
                Some(Actor::Agent { project_id: project_id.clone() })
            }
            _ => None,
        }
    }

    pub fn is_owner(&self) -> bool {
        matches!(self, DataWho::Owner)
    }

    /// `owner` | `agent:<handle>` | `shell:<handle>` for audit lines.
    pub fn audit_label(&self) -> String {
        match self {
            DataWho::Owner => "owner".into(),
            DataWho::Agent { project_id } => format!("agent:{}", crate::compute_routes::ws_handle(project_id)),
            DataWho::Shell { project_id } => format!("shell:{}", crate::compute_routes::ws_handle(project_id)),
            DataWho::Login => "login".into(),
            DataWho::Wrong => "wrong-credential".into(),
            DataWho::Invalid => "-".into(),
        }
    }
}

fn from_caller(c: Caller) -> DataWho {
    match c {
        Caller::Owner => DataWho::Owner,
        Caller::Login { .. } => DataWho::Login,
        Caller::Shell { project_id } => DataWho::Shell { project_id },
        Caller::Agent(a) if a.sandboxed => DataWho::Wrong,
        Caller::Agent(a) => DataWho::Agent { project_id: a.project_id },
        Caller::WrongCredential => DataWho::Wrong,
        Caller::Invalid => DataWho::Invalid,
    }
}

/// Classify a TCP caller (a passport is classified even on `/cli/zen/call`,
/// so the refusal can say `owner_only`).
pub fn who_from_tcp(path: &str, query: &str, bearer: Option<&str>, owner_token: &str) -> DataWho {
    from_caller(crate::zen_routes::zen_caller(path, query, bearer, owner_token))
}

/// Classify a per-cell socket caller (the bound session's passport).
pub fn who_from_cell(session_id: &str, principal: &crate::session_token::HookPrincipal) -> DataWho {
    from_caller(crate::sidecar_routes::caller_from_session(session_id, principal))
}

/// 413 for a body over [`max_body`].
pub fn too_large(p: &str) -> CliResponse {
    gd::response(&DataError::TooLarge(format!("request bodies here are at most {} KiB", max_body(p) / 1024)))
}

/// 405 for a GET on a POST row.
pub fn method_not_allowed() -> CliResponse {
    gd::response(&refused("method_not_allowed", 405, "this route takes POST"))
}

/// Run one Garden data route. The transport already refused a GET to a
/// POST row and an oversized body. `headers` is the request head (TCP;
/// empty on the cell socket), for `/cli/zen/call`'s Origin gate.
pub fn handle(path: &str, who: DataWho, body: &[u8], params: &HashMap<String, String>, headers: &str) -> CliResponse {
    if path == CALL_ROUTE {
        return gd::call::handle(&who, headers, body);
    }
    if gd::blob_routes::is_widget_route(path) {
        return match gd::blob_routes::widget(path, &who, headers, body, params) {
            Ok(v) => gd::ok(v),
            Err(e) => gd::response(&e),
        };
    }
    let gate = match &who {
        DataWho::Invalid => Some(refused("invalid_token", 401, "invalid or missing token")),
        DataWho::Wrong => Some(refused(
            "apps_not_allowed",
            403,
            "app passes, API keys, stream tokens and sandboxed sessions can't use Garden data",
        )),
        DataWho::Login => Some(refused(
            "zen_local_only",
            403,
            "Garden data belongs to the person on this computer; Connect logins can't reach it",
        )),
        DataWho::Agent { .. } | DataWho::Shell { .. } if PERSON_ROUTES.contains(&path) => Some(DataError::PersonOnly),
        _ => None,
    };
    if let Some(e) = gate {
        return gd::response(&e);
    }
    let r = match path {
        // B1: data and grants.
        "/cli/zen/data/get"
        | "/cli/zen/data/put"
        | "/cli/zen/data/patch"
        | "/cli/zen/data/delete"
        | "/cli/zen/data/list"
        | "/cli/zen/data/changes"
        | "/cli/zen/data/batch"
        | "/cli/zen/data/grants" => gd::agent_routes::handle(path, &who, body, params),
        "/cli/zen/data/grant"
        | "/cli/zen/data/revoke"
        | "/cli/zen/data/admin"
        | "/cli/zen/data/admin/revoke"
        | "/cli/zen/data/spaces"
        | "/cli/zen/data/show"
        | "/cli/zen/data/space/delete"
        | "/cli/zen/data/space/move" => gd::grant_routes::handle(path, &who, body, params),
        // B2: transactions.
        "/cli/zen/data/tx/begin" | "/cli/zen/data/tx/add" | "/cli/zen/data/tx/commit" | "/cli/zen/data/tx/abort" => {
            gd::tx_routes::handle(path, &who, body, params)
        }
        // prd-garden-board-needs-v1 GB15: blobs.
        p if gd::blob_routes::is_agent_route(p) => gd::blob_routes::agent(p, &who, body, params),
        // B3: export and import.
        "/cli/zen/data/export" | "/cli/zen/data/import" => gd::export_routes::handle(path, &who, body, params),
        _ => Err(DataError::NotFound("route_not_found")),
    };
    match r {
        Ok(v) => gd::ok(v),
        Err(e) => gd::response(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_and_consistent() {
        for w in ROUTES.windows(2) {
            assert!(w[0] < w[1], "ROUTES sorted: {} then {}", w[0], w[1]);
        }
        for p in POST_ROUTES.iter().chain(PERSON_ROUTES) {
            assert!(is_route(p), "{p} is a route");
        }
        for p in ROUTES {
            assert!(p.starts_with("/cli/zen/data/") || WIDGET_DOOR.contains(p), "{p}");
            let row = crate::routes::route_policy::lookup(p).unwrap_or_else(|| panic!("{p} has no policy row"));
            assert_eq!(row.post.is_some(), is_post_route(p), "{p}: policy POST row vs POST_ROUTES");
        }
        for p in WIDGET_DOOR {
            assert!(is_route(p) && !is_agent_route(p), "{p}: the widget door never reaches the cell socket");
        }
        assert_eq!(max_body(IMPORT_ROUTE), MAX_IMPORT_BYTES);
        assert_eq!(max_body("/cli/zen/data/put"), MAX_BODY);
        assert_eq!(max_body("/cli/zen/blob/put"), MAX_BLOB_BODY);
        assert_eq!(max_body("/cli/zen/data/blob/put"), MAX_BLOB_BODY);
        assert_eq!(max_body("/cli/zen/data/blob/get"), MAX_BODY);
    }

    #[test]
    fn gates_refuse_before_any_handler() {
        let p = HashMap::new();
        let code = |r: CliResponse| -> String {
            let v: serde_json::Value = serde_json::from_str(&r.body).expect("json body");
            v["error"]["code"].as_str().unwrap_or("").to_string()
        };
        assert_eq!(code(handle("/cli/zen/data/get", DataWho::Invalid, b"", &p, "")), "invalid_token");
        assert_eq!(code(handle("/cli/zen/data/get", DataWho::Wrong, b"", &p, "")), "apps_not_allowed");
        assert_eq!(code(handle("/cli/zen/data/get", DataWho::Login, b"", &p, "")), "zen_local_only");
        let agent = DataWho::Agent { project_id: "w-1".into() };
        for person in PERSON_ROUTES {
            assert_eq!(code(handle(person, agent.clone(), b"{}", &p, "")), "person_only", "{person}");
        }
        let shell = DataWho::Shell { project_id: "w-1".into() };
        assert_eq!(code(handle("/cli/zen/data/spaces", shell, b"", &p, "")), "person_only", "shell = agent rules");
    }
}
