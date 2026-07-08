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
//! | GET  /cli/mail/preflight (REAL,S1)| mail/routes_server.rs   |
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
//! REAL as of S1+S2+S3+S4+S5: `/cli/mail/status` (the capability-
//! gating seam the Settings→Email page reads, pre-mortem #15),
//! `/cli/mail/preflight`, the server lifecycle mutations
//! (enable/disable/uninstall), the S2 domain family
//! (`domain/add|remove|check|list|show`), the S3 address family
//! (`address/create|delete|list`), the S4 read family
//! (`messages|read|attachments|wait` — note: the dispatcher gives
//! those four GETs their own `spawn_blocking` arm, since they dial
//! Stalwart over blocking reqwest and `wait` holds the request up to
//! 900 s), and the S5 send family (`send|reply|outbox|approvals/*` —
//! POSTs already run in the mail POST arm's `spawn_blocking`; the
//! `approvals/list` GET has its own dispatcher clause adding the
//! owner-or-admin gate, §11.1.3: owner verbs hard-fail for agent
//! tokens server-side). Every other route (config, doctor) returns
//! the structured `not_built` 501 from its per-concern file until its
//! slice lands.

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
        // S1: read-only preflight checklist (PRD §5.1). GET-only by
        // design — it mutates nothing, so it is deliberately NOT in
        // the dispatcher's post_allowed list.
        "/cli/mail/preflight" => routes_server::handle_preflight(params),
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
    /// slices can rely on the partition map; the REAL routes (status
    /// from the foundation slice, the S2 domain family) answer their
    /// own contracts.
    #[test]
    fn reserved_routes_501_and_real_routes_answer() {
        let params = HashMap::new();
        for route in ["/cli/mail/config", "/cli/mail/doctor"] {
            let resp = dispatch(route, &params).expect("claimed");
            assert_eq!(resp.status, "501 Not Implemented", "route={route}");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "not_built", "route={route}");
        }
        let resp = dispatch_post("/cli/mail/config/set", b"{}");
        assert_eq!(resp.status, "501 Not Implemented");
        let resp = dispatch("/cli/mail/status", &params).expect("claimed");
        assert_eq!(resp.status, "200 OK", "status is REAL from day one");

        // S2 — the domain family is REAL: reads answer through the
        // shim (list 200; show without a domain = usage 400), and the
        // mutations validate their bodies (`{}` = usage 400) instead
        // of 501-ing. Deep behavior is owned by routes_domains tests.
        let resp = dispatch("/cli/mail/domain/list", &params).expect("claimed");
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["ok"], true);
        let resp = dispatch("/cli/mail/domain/show", &params).expect("claimed");
        assert_eq!(resp.status, "400 Bad Request");
        for route in [
            "/cli/mail/domain/add",
            "/cli/mail/domain/remove",
            "/cli/mail/domain/check",
        ] {
            let resp = dispatch_post(route, b"{}");
            assert_eq!(resp.status, "400 Bad Request", "route={route}");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "usage", "route={route}");
        }

        // S3 — the address family is REAL: the list read validates its
        // params through the shim (missing project = usage 400; the
        // owner table answers 200), and the mutations validate their
        // bodies (`{}` = usage 400) instead of 501-ing. Deep behavior
        // is owned by routes_addresses/mail::addresses tests.
        let resp = dispatch("/cli/mail/address/list", &params).expect("claimed");
        assert_eq!(resp.status, "400 Bad Request");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "usage");
        let all_params: HashMap<String, String> =
            HashMap::from([("all".to_string(), "true".to_string())]);
        let resp = dispatch("/cli/mail/address/list", &all_params).expect("claimed");
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["ok"], true);
        for route in ["/cli/mail/address/create", "/cli/mail/address/delete"] {
            let resp = dispatch_post(route, b"{}");
            assert_eq!(resp.status, "400 Bad Request", "route={route}");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "usage", "route={route}");
        }

        // S1 — preflight is REAL.
        let resp = dispatch("/cli/mail/preflight", &params).expect("claimed");
        assert_eq!(resp.status, "200 OK", "preflight is REAL as of S1");

        // S5 — the send family is REAL: reads validate their params
        // through the shim (outbox without a project = usage 400; the
        // owner approvals queue answers 200 — its owner gate lives in
        // the dispatcher clause), and the mutations validate their
        // bodies (`{}` = usage 400) instead of 501-ing. Deep behavior
        // is owned by routes_send/mail::send tests.
        let resp = dispatch("/cli/mail/outbox", &params).expect("claimed");
        assert_eq!(resp.status, "400 Bad Request");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "usage");
        let resp = dispatch("/cli/mail/approvals/list", &params).expect("claimed");
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["ok"], true);
        for route in [
            "/cli/mail/send",
            "/cli/mail/reply",
            "/cli/mail/approvals/approve",
            "/cli/mail/approvals/deny",
        ] {
            let resp = dispatch_post(route, b"{}");
            assert_eq!(resp.status, "400 Bad Request", "route={route}: {}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "usage", "route={route}");
        }

        // S4 — the read family is REAL: every route validates its
        // params through the shim (missing project/id = usage 400)
        // instead of 501-ing. Deep behavior is owned by
        // routes_messages/mail::messages tests.
        for route in [
            "/cli/mail/messages",
            "/cli/mail/read",
            "/cli/mail/attachments",
            "/cli/mail/wait",
        ] {
            let resp = dispatch(route, &params).expect("claimed");
            assert_eq!(resp.status, "400 Bad Request", "route={route}");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "usage", "route={route}");
        }
    }

    /// The S1-real server mutations answer through their handlers (no
    /// 404/501): enable validates its body, disable/uninstall answer
    /// the structured not-installed conflict on an empty table.
    /// (Deeper behavior is tested in mail::routes_server.)
    #[test]
    fn s1_server_mutations_are_wired_to_real_handlers() {
        let _g = crate::mail::mail_server_test_lock();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
        }
        let resp = dispatch_post("/cli/mail/server/enable", b"{}");
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        for route in ["/cli/mail/server/disable", "/cli/mail/server/uninstall"] {
            let resp = dispatch_post(route, b"{}");
            assert_eq!(resp.status, "409 Conflict", "route={route}: {}", resp.body);
            assert!(resp.body.contains("not_installed"), "{}", resp.body);
        }
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
