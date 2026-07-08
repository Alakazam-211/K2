//! `/cli/mail/*` route shim — K2 Mail (prd-email-server-v1 §11).
//!
//! THIN dispatcher only: every handler lives in a per-concern file
//! under `crate::mail` so later slices (S1 supervisor, S2 domains,
//! S3 addresses, S4 read/wait, S5 send/approvals, S6 doctor) never
//! collide in one giant routes file. The path → file partition map —
//! FROZEN for later slices:
//!
//! | path                              | handler file            |
//! |-----------------------------------|-------------------------|
//! | GET  /cli/mail/status  (REAL)     | mail/routes_server.rs   |
//! | POST /cli/mail/server/enable      | mail/routes_server.rs   |
//! | POST /cli/mail/server/disable     | mail/routes_server.rs   |
//! | POST /cli/mail/server/uninstall   | mail/routes_server.rs   |
//! | GET  /cli/mail/config             | mail/routes_server.rs   |
//! | POST /cli/mail/config/set         | mail/routes_server.rs   |
//! | GET  /cli/mail/doctor             | mail/routes_server.rs   |
//! | POST /cli/mail/domain/add         | mail/routes_domains.rs  |
//! | POST /cli/mail/domain/remove      | mail/routes_domains.rs  |
//! | POST /cli/mail/domain/check       | mail/routes_domains.rs  |
//! | GET  /cli/mail/domain/list        | mail/routes_domains.rs  |
//! | GET  /cli/mail/domain/show        | mail/routes_domains.rs  |
//! | POST /cli/mail/address/create     | mail/routes_addresses.rs|
//! | POST /cli/mail/address/delete     | mail/routes_addresses.rs|
//! | GET  /cli/mail/address/list       | mail/routes_addresses.rs|
//! | GET  /cli/mail/messages           | mail/routes_messages.rs |
//! | GET  /cli/mail/read               | mail/routes_messages.rs |
//! | GET  /cli/mail/attachments        | mail/routes_messages.rs |
//! | GET  /cli/mail/wait               | mail/routes_messages.rs |
//! | POST /cli/mail/send               | mail/routes_send.rs     |
//! | POST /cli/mail/reply              | mail/routes_send.rs     |
//! | GET  /cli/mail/outbox             | mail/routes_send.rs     |
//! | GET  /cli/mail/approvals/list     | mail/routes_send.rs     |
//! | POST /cli/mail/approvals/approve  | mail/routes_send.rs     |
//! | POST /cli/mail/approvals/deny     | mail/routes_send.rs     |
//!
//! (Family name is `mail`, deliberately NOT `inbox` — that collides
//! with K2's internal `/cli/inbox/*` queue, PRD §11.)
//!
//! Method gating follows the house rules (feedback_post_only_route_
//! guards): mutations are JSON-bodied POSTs listed in the dispatcher's
//! `post_allowed` allowlist + handled by [`dispatch_post`] behind
//! `require_post` + `token_ok` (+ `token_is_owner_or_admin` for the
//! owner-level paths — see the dispatcher's `/cli/mail/` arm); reads
//! are GETs through `crate::cli::dispatch` → [`dispatch`], which also
//! answers 405 for mutations reached via the GET chain.
//!
//! Only `/cli/mail/status` is REAL in the foundation slice — it is the
//! capability-gating seam (`supported: bool`) the Settings→Email page
//! reads (pre-mortem #15). Every other route returns the structured
//! `not_built` 501 from its per-concern file.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::{
    routes_addresses, routes_domains, routes_messages, routes_send, routes_server,
};

/// Mail-domain GET dispatch. Returns `Some(resp)` for a handled path,
/// `None` if the path isn't a mail route. Claims the WHOLE
/// `/cli/mail/` prefix: unknown sub-paths 404 here (clearer than the
/// top-level catch-all for a reserved family).
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    if !path.starts_with("/cli/mail/") {
        return None;
    }
    let resp = match path {
        // ── Reads ───────────────────────────────────────────────────
        "/cli/mail/status" => routes_server::handle_status(params),
        "/cli/mail/config" => routes_server::handle_config_get(params),
        "/cli/mail/doctor" => routes_server::handle_doctor(params),
        "/cli/mail/domain/list" => routes_domains::handle_domain_list(params),
        "/cli/mail/domain/show" => routes_domains::handle_domain_show(params),
        "/cli/mail/address/list" => routes_addresses::handle_address_list(params),
        "/cli/mail/messages" => routes_messages::handle_messages(params),
        "/cli/mail/read" => routes_messages::handle_read(params),
        "/cli/mail/attachments" => routes_messages::handle_attachments(params),
        "/cli/mail/wait" => routes_messages::handle_wait(params),
        "/cli/mail/outbox" => routes_send::handle_outbox(params),
        "/cli/mail/approvals/list" => routes_send::handle_approvals_list(params),

        // ── POST-only mutations reached via the GET chain → 405 ─────
        // (feedback_post_only_route_guards house rule.)
        "/cli/mail/server/enable"
        | "/cli/mail/server/disable"
        | "/cli/mail/server/uninstall"
        | "/cli/mail/config/set"
        | "/cli/mail/domain/add"
        | "/cli/mail/domain/remove"
        | "/cli/mail/domain/check"
        | "/cli/mail/address/create"
        | "/cli/mail/address/delete"
        | "/cli/mail/send"
        | "/cli/mail/reply"
        | "/cli/mail/approvals/approve"
        | "/cli/mail/approvals/deny" => CliResponse::method_not_allowed(),

        _ => CliResponse::not_found(),
    };
    Some(resp)
}

/// Dispatch a `/cli/mail/*` POST body to its per-concern handler.
/// Exact-match paths; unknown paths 404 (mirrors
/// `feedback_routes::dispatch_post`). The caller (the dispatcher's
/// `/cli/mail/` arm) has already enforced require_post + token_ok +
/// the owner-or-admin gate for owner-level paths.
pub fn dispatch_post(path: &str, body: &[u8]) -> CliResponse {
    match path {
        "/cli/mail/server/enable" => routes_server::handle_server_enable(body),
        "/cli/mail/server/disable" => routes_server::handle_server_disable(body),
        "/cli/mail/server/uninstall" => routes_server::handle_server_uninstall(body),
        "/cli/mail/config/set" => routes_server::handle_config_set(body),
        "/cli/mail/domain/add" => routes_domains::handle_domain_add(body),
        "/cli/mail/domain/remove" => routes_domains::handle_domain_remove(body),
        "/cli/mail/domain/check" => routes_domains::handle_domain_check(body),
        "/cli/mail/address/create" => routes_addresses::handle_address_create(body),
        "/cli/mail/address/delete" => routes_addresses::handle_address_delete(body),
        "/cli/mail/send" => routes_send::handle_send(body),
        "/cli/mail/reply" => routes_send::handle_reply(body),
        "/cli/mail/approvals/approve" => routes_send::handle_approvals_approve(body),
        "/cli/mail/approvals/deny" => routes_send::handle_approvals_deny(body),
        _ => CliResponse::not_found(),
    }
}

/// Owner-level `/cli/mail/*` mutations — the paths the dispatcher's
/// POST arm additionally gates with `token_is_owner_or_admin` (PRD
/// §10: server enable/disable/uninstall + domain add/remove/check +
/// mode/relay config + approvals = owner-or-admin; address
/// create/delete + send/reply stay workspace-token so agents can act).
pub fn is_owner_level_mutation(path: &str) -> bool {
    path.starts_with("/cli/mail/server/")
        || path.starts_with("/cli/mail/domain/")
        || path.starts_with("/cli/mail/config/")
        || path.starts_with("/cli/mail/approvals/")
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — shim wiring
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// GET on every POST-only mutation answers an explicit 405 through
    /// the read dispatch chain (feedback_post_only_route_guards), the
    /// POST dispatcher 404s unknown paths, and non-mail paths are not
    /// claimed.
    #[test]
    fn mail_mutations_405_on_get_and_post_404_unknown() {
        let params = HashMap::new();
        for route in [
            "/cli/mail/server/enable",
            "/cli/mail/server/disable",
            "/cli/mail/server/uninstall",
            "/cli/mail/config/set",
            "/cli/mail/domain/add",
            "/cli/mail/domain/remove",
            "/cli/mail/domain/check",
            "/cli/mail/address/create",
            "/cli/mail/address/delete",
            "/cli/mail/send",
            "/cli/mail/reply",
            "/cli/mail/approvals/approve",
            "/cli/mail/approvals/deny",
        ] {
            let resp = dispatch(route, &params).expect("route claimed by GET chain");
            assert_eq!(resp.status, "405 Method Not Allowed", "route={route}");
            assert!(resp.body.contains("POST required"), "body={}", resp.body);
        }
        assert_eq!(dispatch_post("/cli/mail/unknown", b"{}").status, "404 Not Found");
        assert_eq!(
            dispatch("/cli/mail/unknown", &params).expect("prefix claimed").status,
            "404 Not Found"
        );
        assert!(dispatch("/cli/feedback/list", &params).is_none(), "not our prefix");
        assert!(dispatch("/cli/mailbox", &params).is_none(), "no bare-prefix match");
    }

    /// Every reserved route answers 501 not_built (never 404) so later
    /// slices can rely on the partition map; status is the one real
    /// route and answers 200.
    #[test]
    fn reserved_routes_501_and_status_is_real() {
        let params = HashMap::new();
        for route in [
            "/cli/mail/config",
            "/cli/mail/doctor",
            "/cli/mail/domain/list",
            "/cli/mail/domain/show",
            "/cli/mail/address/list",
            "/cli/mail/messages",
            "/cli/mail/read",
            "/cli/mail/attachments",
            "/cli/mail/wait",
            "/cli/mail/outbox",
            "/cli/mail/approvals/list",
        ] {
            let resp = dispatch(route, &params).expect("claimed");
            assert_eq!(resp.status, "501 Not Implemented", "route={route}");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "not_built", "route={route}");
        }
        for route in [
            "/cli/mail/server/enable",
            "/cli/mail/server/disable",
            "/cli/mail/server/uninstall",
            "/cli/mail/config/set",
            "/cli/mail/domain/add",
            "/cli/mail/domain/remove",
            "/cli/mail/domain/check",
            "/cli/mail/address/create",
            "/cli/mail/address/delete",
            "/cli/mail/send",
            "/cli/mail/reply",
            "/cli/mail/approvals/approve",
            "/cli/mail/approvals/deny",
        ] {
            let resp = dispatch_post(route, b"{}");
            assert_eq!(resp.status, "501 Not Implemented", "route={route}");
        }
        let resp = dispatch("/cli/mail/status", &params).expect("claimed");
        assert_eq!(resp.status, "200 OK", "status is REAL from day one");
    }

    /// The owner-or-admin classification the dispatcher arm relies on.
    #[test]
    fn owner_level_mutation_classification() {
        for owner_path in [
            "/cli/mail/server/enable",
            "/cli/mail/server/disable",
            "/cli/mail/server/uninstall",
            "/cli/mail/config/set",
            "/cli/mail/domain/add",
            "/cli/mail/domain/remove",
            "/cli/mail/domain/check",
            "/cli/mail/approvals/approve",
            "/cli/mail/approvals/deny",
        ] {
            assert!(is_owner_level_mutation(owner_path), "{owner_path}");
        }
        for agent_path in [
            "/cli/mail/address/create",
            "/cli/mail/address/delete",
            "/cli/mail/send",
            "/cli/mail/reply",
        ] {
            assert!(!is_owner_level_mutation(agent_path), "{agent_path}");
        }
    }
}
