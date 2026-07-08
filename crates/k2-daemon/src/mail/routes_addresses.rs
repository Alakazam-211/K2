//! `/cli/mail/address/*` — ADDRESS-concern handlers (built in mail
//! slice S3: minting, ownership, caps, idempotent `--id`, retire).
//!
//! Dispatched by the `crate::mail_routes` shim. AUTH/GATING contract
//! (PRD §10 / §7): create/delete = WORKSPACE token (`token_ok` —
//! agents mint their own addresses), POST-only (`require_post` +
//! `post_allowed`, house rule feedback_post_only_route_guards). The
//! OWNING workspace (`owner_project_id`) is resolved SERVER-SIDE from
//! the caller's context — never trusted from the body. Cap enforcement
//! reads `k2_core::workspace::settings::mail_address_cap_for_path`
//! (0 = unlimited); over cap → the PRD's exact error text with the
//! Settings pointer.

use std::collections::HashMap;

use crate::cli_response::CliResponse;

/// POST `/cli/mail/address/create` — S3: mint on a Verified domain,
/// subject to the per-workspace cap; idempotent via optional
/// `clientId` (same (owner, clientId) returns the existing address).
pub fn handle_address_create(_body: &[u8]) -> CliResponse {
    super::not_built("S3", "POST /cli/mail/address/create")
}

/// POST `/cli/mail/address/delete` — S3: retire an address the caller
/// owns (Stalwart account disabled, row → `retired`, data kept for
/// the retention window).
pub fn handle_address_delete(_body: &[u8]) -> CliResponse {
    super::not_built("S3", "POST /cli/mail/address/delete")
}

/// GET `/cli/mail/address/list` — S3: the caller's addresses + unread
/// counts + cap usage (`k2 mail list`).
pub fn handle_address_list(_params: &HashMap<String, String>) -> CliResponse {
    super::not_built("S3", "GET /cli/mail/address/list")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stubs_answer_structured_501() {
        for resp in [
            handle_address_create(b"{}"),
            handle_address_delete(b"{}"),
            handle_address_list(&HashMap::new()),
        ] {
            assert_eq!(resp.status, "501 Not Implemented");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "not_built");
            assert!(v["error"]["hint"]
                .as_str()
                .expect("hint")
                .contains("not built yet — mail slice S3"));
        }
    }
}
