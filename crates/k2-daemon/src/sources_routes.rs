//! `/cli/sources/*` HTTP routes (prd-garden-v3-external-data-cards-v1;
//! contract-sources-v1 §4).
//!
//! Who may do what (contract §4.1):
//! - `role_gate` (route_policy floors) stops Connect logins below the row.
//! - [`who_from_tcp`] / [`who_from_cell`] classify the caller with the
//!   compute classifier (`compute_routes::Who`): app passes, API keys and
//!   stream tokens are 403 `apps_not_allowed`; no valid credential is 401
//!   `invalid_token`.
//! - Both transports refuse a GET to a POST-only path with 405 before the
//!   handler runs ([`is_post_route`]).
//! - The per-cell socket serves EVERY `/cli/sources/*` path for the bound
//!   session (agent or K2 shell tab); handlers refuse what agents can't do
//!   with 403 `person_only`, never 404 [GV52]. `shell` follows the agent
//!   rules.
//!
//! The handlers are blocking (DB); the dispatcher runs [`handle`] on a
//! blocking thread. B1 owns this file and its source handlers; the
//! trigger, approval and Try/simulate paths delegate to B2's modules under
//! `crate::sources`.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::sources::{approve_routes, not_built, tinker_routes, trigger_routes, SourceErr};

/// Largest request body (256 KiB).
pub const MAX_BODY: usize = 256 * 1024;

/// Every sources route.
pub const ROUTES: &[&str] = &[
    "/cli/sources/add",
    "/cli/sources/approve",
    "/cli/sources/batch-approve",
    "/cli/sources/changes",
    "/cli/sources/edit",
    "/cli/sources/export",
    "/cli/sources/history",
    "/cli/sources/list",
    "/cli/sources/mute",
    "/cli/sources/pause",
    "/cli/sources/pending",
    "/cli/sources/pin",
    "/cli/sources/propose",
    "/cli/sources/read",
    "/cli/sources/refresh",
    "/cli/sources/remove",
    "/cli/sources/resume",
    "/cli/sources/secret",
    "/cli/sources/show",
    "/cli/sources/simulate",
    "/cli/sources/status",
    "/cli/sources/triggers/add",
    "/cli/sources/triggers/approve",
    "/cli/sources/triggers/edit",
    "/cli/sources/triggers/list",
    "/cli/sources/triggers/log",
    "/cli/sources/triggers/pause",
    "/cli/sources/triggers/propose",
    "/cli/sources/triggers/remove",
    "/cli/sources/triggers/resume",
    "/cli/sources/triggers/show",
    "/cli/sources/triggers/test",
    "/cli/sources/try",
    "/cli/sources/unmute",
    "/cli/sources/unpin",
];

/// POST-only rows (405 on GET).
pub const POST_ROUTES: &[&str] = &[
    "/cli/sources/add",
    "/cli/sources/approve",
    "/cli/sources/batch-approve",
    "/cli/sources/edit",
    "/cli/sources/mute",
    "/cli/sources/pause",
    "/cli/sources/pin",
    "/cli/sources/propose",
    "/cli/sources/refresh",
    "/cli/sources/remove",
    "/cli/sources/resume",
    "/cli/sources/secret",
    "/cli/sources/simulate",
    "/cli/sources/triggers/add",
    "/cli/sources/triggers/approve",
    "/cli/sources/triggers/edit",
    "/cli/sources/triggers/pause",
    "/cli/sources/triggers/propose",
    "/cli/sources/triggers/remove",
    "/cli/sources/triggers/resume",
    "/cli/sources/triggers/test",
    "/cli/sources/try",
    "/cli/sources/unmute",
    "/cli/sources/unpin",
];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

/// Classify a TCP caller (the dispatcher's effective auth query).
pub fn who_from_tcp(query: &str, bearer: Option<&str>, owner_token: &str) -> Who {
    crate::compute_routes::who_from_tcp(query, bearer, owner_token)
}

/// Classify a per-cell socket caller (the bound session's passport).
pub fn who_from_cell(session_id: &str, principal: &crate::session_token::HookPrincipal) -> Who {
    crate::compute_routes::who_from_cell(session_id, principal)
}

/// 413 for a body over [`MAX_BODY`].
pub fn too_large() -> CliResponse {
    SourceErr::new("bad_request", 413, format!("request bodies here are at most {} KiB", MAX_BODY / 1024)).response()
}

/// 405 for a GET on a POST-only path.
pub fn method_not_allowed() -> CliResponse {
    SourceErr::new("method_not_allowed", 405, "this route takes POST").response()
}

/// Run one sources route. The transport already refused a GET to a
/// POST-only path and an oversized body.
pub fn handle(path: &str, who: Who, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    match &who {
        Who::Invalid => return SourceErr::new("invalid_token", 401, "invalid or missing token").response(),
        Who::Wrong => {
            return SourceErr::new(
                "apps_not_allowed",
                403,
                "app passes, API keys and stream tokens can't use data sources",
            )
            .response()
        }
        _ => {}
    }
    let r: Result<serde_json::Value, SourceErr> = match path {
        // B1: sources.
        "/cli/sources/list"
        | "/cli/sources/show"
        | "/cli/sources/read"
        | "/cli/sources/history"
        | "/cli/sources/changes"
        | "/cli/sources/export"
        | "/cli/sources/status"
        | "/cli/sources/propose"
        | "/cli/sources/add"
        | "/cli/sources/edit"
        | "/cli/sources/remove"
        | "/cli/sources/pause"
        | "/cli/sources/resume"
        | "/cli/sources/mute"
        | "/cli/sources/unmute"
        | "/cli/sources/refresh" => Err(not_built()),
        // B2: Try and simulate.
        "/cli/sources/try" | "/cli/sources/simulate" => tinker_routes::handle(path, &who, body, params),
        // B2: approvals, secret binding, pins.
        "/cli/sources/pending"
        | "/cli/sources/approve"
        | "/cli/sources/batch-approve"
        | "/cli/sources/secret"
        | "/cli/sources/pin"
        | "/cli/sources/unpin" => approve_routes::handle(path, &who, body, params),
        // B2: triggers.
        "/cli/sources/triggers/list"
        | "/cli/sources/triggers/show"
        | "/cli/sources/triggers/log"
        | "/cli/sources/triggers/propose"
        | "/cli/sources/triggers/add"
        | "/cli/sources/triggers/edit"
        | "/cli/sources/triggers/approve"
        | "/cli/sources/triggers/pause"
        | "/cli/sources/triggers/resume"
        | "/cli/sources/triggers/remove"
        | "/cli/sources/triggers/test" => trigger_routes::handle(path, &who, body, params),
        _ => Err(SourceErr::new("not_found", 404, "no such sources route")),
    };
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => e.response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(r: &CliResponse) -> &'static str {
        r.status
    }

    #[test]
    fn route_lists_are_sorted_unique_and_post_rows_are_routes() {
        for w in ROUTES.windows(2) {
            assert!(w[0] < w[1], "ROUTES sorted and unique: {} then {}", w[0], w[1]);
        }
        for w in POST_ROUTES.windows(2) {
            assert!(w[0] < w[1], "POST_ROUTES sorted and unique: {} then {}", w[0], w[1]);
        }
        for p in POST_ROUTES {
            assert!(is_route(p), "{p} is a POST row but not a route");
        }
        assert_eq!(ROUTES.len(), 35);
        assert_eq!(POST_ROUTES.len(), 24);
    }

    /// The policy table agrees with these lists: every route is classified,
    /// POST-only rows have no GET form, GET rows take no POST.
    #[test]
    fn every_route_matches_its_policy_row() {
        for p in ROUTES {
            let row = crate::routes::route_policy::lookup(p).unwrap_or_else(|| panic!("{p} has no policy row"));
            if is_post_route(p) {
                assert!(row.get.is_none() && row.post.is_some(), "{p} is POST-only");
            } else {
                assert!(row.get.is_some() && row.post.is_none(), "{p} is GET-only");
            }
        }
    }

    #[test]
    fn handle_refuses_apps_and_invalid_tokens_before_any_route() {
        let params = HashMap::new();
        let r = handle("/cli/sources/list", Who::Wrong, b"", &params);
        assert_eq!(status(&r), "403 Forbidden");
        assert!(r.body.contains("apps_not_allowed"), "{}", r.body);
        let r = handle("/cli/sources/list", Who::Invalid, b"", &params);
        assert_eq!(status(&r), "401 Unauthorized");
        assert!(r.body.contains("invalid_token"), "{}", r.body);
    }

    /// Every route reaches a handler (a stub answers 501 `not_built` until
    /// its builder lands it): never this module's 404, always a JSON body.
    #[test]
    fn every_route_is_wired_to_a_handler() {
        let _home = crate::test_support::TempHome::new();
        let _ = k2_core::db::init_for_tests();
        let params = HashMap::new();
        for p in ROUTES {
            let r = handle(p, Who::Owner, b"{}", &params);
            assert!(!r.body.contains("no such sources route"), "{p} is not wired: {}", r.body);
            let _: serde_json::Value =
                serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{p}: not JSON ({e}): {}", r.body));
        }
        let r = handle("/cli/sources/nope", Who::Owner, b"", &params);
        assert_eq!(status(&r), "404 Not Found");
    }
}
