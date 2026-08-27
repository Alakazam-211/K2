//! `/cli/db/*` and `/cli/store/*` route shim.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::sql::routes;

pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    if !(path.starts_with("/cli/db/") || path.starts_with("/cli/store/")) {
        return None;
    }
    let resp = match path {
        "/cli/db/status" => routes::handle_status(params),
        "/cli/db/doctor" => routes::handle_doctor(params),
        "/cli/db/list" => routes::handle_list(params),
        "/cli/db/dsn" => routes::handle_dsn(params),
        "/cli/store/list" => routes::handle_store_list(params),
        "/cli/store/get" => routes::handle_store_get(params),
        "/cli/store/query" => routes::handle_store_query(params),
        "/cli/db/server/enable"
        | "/cli/db/server/disable"
        | "/cli/db/server/uninstall"
        | "/cli/db/create"
        | "/cli/db/migrate"
        | "/cli/db/dump"
        | "/cli/db/restore"
        | "/cli/db/drop"
        | "/cli/db/grant"
        | "/cli/db/revoke"
        | "/cli/db/bind"
        | "/cli/store/create"
        | "/cli/store/put"
        | "/cli/store/rm"
        | "/cli/store/drop" => CliResponse::method_not_allowed(),
        _ => CliResponse::not_found(),
    };
    Some(resp)
}

pub fn dispatch_post(path: &str, body: &[u8]) -> CliResponse {
    match path {
        "/cli/db/server/enable" => routes::handle_server_enable(body),
        "/cli/db/server/disable" => routes::handle_server_disable(body),
        "/cli/db/server/uninstall" => routes::handle_server_uninstall(body),
        "/cli/db/doctor" => routes::handle_doctor_run(body),
        "/cli/db/create" => routes::handle_create(body),
        "/cli/db/migrate" => routes::handle_migrate(body),
        "/cli/db/dump" => routes::handle_dump(body),
        "/cli/db/restore" => routes::handle_restore(body),
        "/cli/db/drop" => routes::handle_drop(body),
        "/cli/db/grant" => routes::handle_grant(body),
        "/cli/db/revoke" => routes::handle_revoke(body),
        "/cli/db/bind" => routes::handle_bind(body),
        "/cli/store/create" => routes::handle_store_create(body),
        "/cli/store/put" => routes::handle_store_put(body),
        "/cli/store/rm" => routes::handle_store_rm(body),
        "/cli/store/drop" => routes::handle_store_drop(body),
        _ => CliResponse::not_found(),
    }
}

pub fn is_owner_level_mutation(path: &str) -> bool {
    path.starts_with("/cli/db/server/") || path == "/cli/db/doctor" || path == "/cli/db/bind"
}

pub fn is_sql_owner_surface(path: &str) -> bool {
    is_owner_level_mutation(path)
}

pub fn owner_only_response() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": "owner_only",
                "hint": "requires owner/admin — ask your human (k2 db enable/disable/uninstall/doctor/bind are owner surfaces)",
            },
        })
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_only_response_has_stable_code_and_403() {
        let r = owner_only_response();
        assert_eq!(r.status, "403 Forbidden");
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["error"]["code"], "owner_only");
    }

    #[test]
    fn get_create_405() {
        let r = dispatch("/cli/db/create", &HashMap::new()).unwrap();
        assert_eq!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn get_grant_405() {
        let r = dispatch("/cli/db/grant", &HashMap::new()).unwrap();
        assert_eq!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn get_bind_405() {
        let r = dispatch("/cli/db/bind", &HashMap::new()).unwrap();
        assert_eq!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn get_restore_405() {
        let r = dispatch("/cli/db/restore", &HashMap::new()).unwrap();
        assert_eq!(r.status, "405 Method Not Allowed");
    }
}
