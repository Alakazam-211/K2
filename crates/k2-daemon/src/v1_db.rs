//! D19/D20 — `/v1/w/<ws>/db` create + migrate, GET dump.
//! API key + workspace grant + `cap_db` (fail-closed). Create is NOT on hire.

use crate::cli_response::CliResponse;
use crate::routes::http::{V1Capability, V1Principal};
use crate::sql::ops;
use crate::sql::secrets::FileSecretStore;
use crate::sql::sysops::RealSystemOps;
use crate::v1_sandboxes::{decode_and_validate_segment, resolve_authorized_workspace, uniform_ws_404};

pub(crate) fn require_db(principal: &V1Principal) -> Result<(), CliResponse> {
    if principal.has_capability(V1Capability::Db) {
        Ok(())
    } else {
        Err(uniform_ws_404())
    }
}

fn ops_err(e: ops::OpsError) -> CliResponse {
    CliResponse {
        status: e.status(),
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": e.code(), "hint": e.hint() },
        })
        .to_string(),
    }
}

fn project_id_for_path(path: &str) -> Result<String, CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace::agent_identity::resolve_project_id(&conn, path)
        .ok_or_else(uniform_ws_404)
}

/// `POST /v1/w/<ws>/db` — create (NOT on hire).
pub(crate) fn handle_v1_db_create(
    principal: &V1Principal,
    ws_raw: &str,
    body: &[u8],
) -> CliResponse {
    if let Err(resp) = require_db(principal) {
        return resp;
    }
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    let ws_path = match resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let parsed: serde_json::Value = match serde_json::from_slice(if body.is_empty() { b"{}" } else { body }) {
        Ok(v) => v,
        Err(e) => {
            return CliResponse::bad_request(format!("invalid JSON body: {e}"));
        }
    };
    let client_id = parsed["clientId"].as_str().or_else(|| parsed["id"].as_str());
    let name = parsed["name"].as_str();
    let project_id = match project_id_for_path(&ws_path) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let cap = k2_core::workspace::settings::db_active_cap_for_path(&ws_path);
    let secrets = FileSecretStore::default();
    match ops::create_database(
        &RealSystemOps,
        &secrets,
        &project_id,
        cap,
        client_id,
        name,
    ) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => ops_err(e),
    }
}

/// `POST /v1/w/<ws>/db/migrate`
pub(crate) fn handle_v1_db_migrate(
    principal: &V1Principal,
    ws_raw: &str,
    body: &[u8],
) -> CliResponse {
    if let Err(resp) = require_db(principal) {
        return resp;
    }
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    let ws_path = match resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let parsed: serde_json::Value = match serde_json::from_slice(if body.is_empty() { b"{}" } else { body }) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let dir = parsed["dir"].as_str();
    let project_id = match project_id_for_path(&ws_path) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let secrets = FileSecretStore::default();
    match ops::migrate(&RealSystemOps, &secrets, &project_id, &ws_path, dir) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => ops_err(e),
    }
}

/// `GET /v1/w/<ws>/db/dump` — org-box pull (D20).
pub(crate) fn handle_v1_db_dump(principal: &V1Principal, ws_raw: &str) -> CliResponse {
    if let Err(resp) = require_db(principal) {
        return resp;
    }
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    let ws_path = match resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match project_id_for_path(&ws_path) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let secrets = FileSecretStore::default();
    match ops::dump(&RealSystemOps, &secrets, &project_id, &ws_path, None) {
        Ok((mut v, bytes)) => {
            if let Some(b) = bytes {
                v["bytesBase64"] = serde_json::Value::String(
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b),
                );
            }
            CliResponse::ok_json(v.to_string())
        }
        Err(e) => ops_err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::api_keys::ApiCapabilities;

    fn apik_caps(id: &str, grant: Option<&str>, capabilities: ApiCapabilities) -> V1Principal {
        V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: id.to_string(),
            anthropic_key: None,
            provider: None,
            base_url: None,
            scope: "owner".to_string(),
            allowed_workspaces: grant.map(str::to_string),
            capabilities,
        })
    }

    #[test]
    fn db_cap_fail_closed_is_uniform_404() {
        k2_core::db::init_for_tests();
        let no_db = apik_caps(
            "k-db-nocap",
            Some("*"),
            ApiCapabilities {
                host_sessions: true,
                canonical_message: true,
                sandboxes: true,
                db: false,
            },
        );
        for call in [
            handle_v1_db_create(&no_db, "any", b"{}"),
            handle_v1_db_migrate(&no_db, "any", b"{}"),
            handle_v1_db_dump(&no_db, "any"),
        ] {
            assert_eq!(call.status, "404 Not Found", "body={}", call.body);
            assert!(call.body.contains("no such workspace"));
        }
    }
}
