//! `/cli/dns/*` route shim — DNS K1 (control-plane proxy).
//!
//! Thin dispatcher only: handlers live in [`crate::dns::routes`]. Method
//! gating mirrors mail: mutations are JSON-bodied POSTs listed in the
//! dispatcher's `post_allowed` allowlist + handled by [`dispatch_post`]
//! behind `require_post` + token_ok OR scoped `require_hook`; reads are
//! GETs through `crate::cli::dispatch` → [`dispatch`].
//!
//! | path                         | concern                          |
//! |------------------------------|----------------------------------|
//! | GET/POST /cli/dns/access     | capability + zones summary       |
//! | GET/POST /cli/dns/zones      | list zones                       |
//! | GET/POST /cli/dns/records    | list records (zone|domain)       |
//! | POST /cli/dns/records/add    | insert record (envelope-checked) |
//! | POST /cli/dns/records/remove | delete record by id              |
//! | POST /cli/dns/verify         | on-demand delegation check       |
//! | POST /cli/dns/zones/create   | owner-only local reject          |
//! | POST /cli/dns/zones/delete   | owner-only local reject          |

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::dns::routes;

/// DNS GET dispatch. Returns `Some(resp)` for a handled path, `None` if
/// the path isn't under `/cli/dns/`. Claims the whole prefix.
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    if !path.starts_with("/cli/dns/") {
        return None;
    }
    let resp = match path {
        "/cli/dns/access" => routes::handle_access(params),
        "/cli/dns/zones" => routes::handle_zones(params),
        "/cli/dns/records" => routes::handle_records(params),

        // POST-only mutations reached via GET chain → 405
        "/cli/dns/records/add"
        | "/cli/dns/records/remove"
        | "/cli/dns/verify"
        | "/cli/dns/zones/create"
        | "/cli/dns/zones/delete" => CliResponse::method_not_allowed(),

        _ => CliResponse::not_found(),
    };
    Some(resp)
}

/// Dispatch a `/cli/dns/*` POST body to its handler.
pub fn dispatch_post(path: &str, body: &[u8]) -> CliResponse {
    match path {
        "/cli/dns/access" => routes::handle_access_post(body),
        "/cli/dns/zones" => routes::handle_zones_post(body),
        "/cli/dns/records" => routes::handle_records_post(body),
        "/cli/dns/records/add" => routes::handle_record_add_post(body),
        "/cli/dns/records/remove" => routes::handle_record_remove_post(body),
        "/cli/dns/verify" => routes::handle_verify_post(body),
        "/cli/dns/zones/create" => routes::handle_zones_create(&HashMap::new()),
        "/cli/dns/zones/delete" => routes::handle_zones_delete(&HashMap::new()),
        _ => CliResponse::not_found(),
    }
}

/// Owner-only zone lifecycle paths — agents must never drive these via
/// scoped tokens (`is_agent_verb` denylist). The HTTP arm still rejects
/// them with a teaching 403 if somehow reached.
#[allow(dead_code)] // classifier for future owner-or-admin arm parity with mail
pub fn is_owner_level_mutation(path: &str) -> bool {
    path == "/cli/dns/zones/create" || path == "/cli/dns/zones/delete"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_mutations_405_on_get_and_post_404_unknown() {
        let params = HashMap::new();
        for route in [
            "/cli/dns/records/add",
            "/cli/dns/records/remove",
            "/cli/dns/verify",
            "/cli/dns/zones/create",
            "/cli/dns/zones/delete",
        ] {
            let resp = dispatch(route, &params).expect("claimed");
            assert_eq!(resp.status, "405 Method Not Allowed", "route={route}");
        }
        assert_eq!(
            dispatch_post("/cli/dns/unknown", b"{}").status,
            "404 Not Found"
        );
        assert_eq!(
            dispatch("/cli/dns/unknown", &params)
                .expect("prefix claimed")
                .status,
            "404 Not Found"
        );
        assert!(dispatch("/cli/mail/status", &params).is_none());
        assert!(dispatch("/cli/dns", &params).is_none(), "no bare prefix");
    }

    #[test]
    fn owner_level_classifier() {
        assert!(is_owner_level_mutation("/cli/dns/zones/create"));
        assert!(is_owner_level_mutation("/cli/dns/zones/delete"));
        assert!(!is_owner_level_mutation("/cli/dns/records/add"));
        assert!(!is_owner_level_mutation("/cli/dns/access"));
    }
}
