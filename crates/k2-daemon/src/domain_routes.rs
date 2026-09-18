//! `/cli/domains` + `/cli/certs` route shim.
//!
//! Copy DNS K1: GET list exact; GET of mutating paths → 405; POSTs on
//! `post_allowed` + `require_post`. Dual GET+POST on exact `/cli/domains`
//! is list vs attach.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::domains::routes;

/// GET dispatch. Claims `/cli/domains`, `/cli/domains/*`, `/cli/certs`,
/// `/cli/certs/*`.
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    if !is_domains_or_certs_path(path) {
        return None;
    }
    let resp = match path {
        "/cli/domains" => routes::handle_list(params),
        "/cli/certs" => routes::handle_certs_list(params),
        "/cli/domains/remove"
        | "/cli/domains/names"
        | "/cli/domains/names/remove"
        | "/cli/certs/issue"
        | "/cli/certs/renew"
        | "/cli/certs/upload" => CliResponse::method_not_allowed(),
        _ => CliResponse::not_found(),
    };
    Some(resp)
}

pub fn dispatch_post(path: &str, body: &[u8]) -> CliResponse {
    match path {
        "/cli/domains" => routes::handle_attach_post(body),
        "/cli/domains/remove" => routes::handle_remove_post(body),
        "/cli/domains/names" => routes::handle_names_add_post(body),
        "/cli/domains/names/remove" => routes::handle_names_remove_post(body),
        _ => CliResponse::not_found(),
    }
}

pub fn is_domains_or_certs_path(path: &str) -> bool {
    path == "/cli/domains"
        || path.starts_with("/cli/domains/")
        || path == "/cli/certs"
        || path.starts_with("/cli/certs/")
}

/// GET of these paths is always 405 (before auth so agents see 405 not 403).
pub fn is_mutating_get_path(path: &str) -> bool {
    matches!(
        path,
        "/cli/domains/remove"
            | "/cli/domains/names"
            | "/cli/domains/names/remove"
            | "/cli/certs/issue"
            | "/cli/certs/renew"
            | "/cli/certs/upload"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutating_gets_are_405() {
        let params = HashMap::new();
        for route in [
            "/cli/domains/remove",
            "/cli/domains/names",
            "/cli/domains/names/remove",
            "/cli/certs/issue",
            "/cli/certs/renew",
        ] {
            let resp = dispatch(route, &params).expect("claimed");
            assert_eq!(resp.status, "405 Method Not Allowed", "route={route}");
        }
        assert!(dispatch("/cli/dns/zones", &params).is_none());
        assert!(is_mutating_get_path("/cli/domains/remove"));
        assert!(!is_mutating_get_path("/cli/domains"));
        assert!(!is_mutating_get_path("/cli/certs"));
    }

    #[test]
    fn post_unknown_is_404() {
        assert_eq!(
            dispatch_post("/cli/domains/nope", b"{}").status,
            "404 Not Found"
        );
    }
}
