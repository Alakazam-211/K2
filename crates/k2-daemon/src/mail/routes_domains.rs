//! `/cli/mail/domain/*` — DOMAIN-concern handlers (built in mail
//! slice S2: add/remove/check + the dnsZoneFile record table + the
//! verification poller).
//!
//! Dispatched by the `crate::mail_routes` shim. AUTH/GATING contract
//! (PRD §10), enforced in the dispatcher's `/cli/mail/` POST arm and
//! re-asserted per-handler as slices land: domain add/remove/check =
//! OWNER-OR-ADMIN (`token_is_owner_or_admin`), POST-only (`require_post`
//! + `post_allowed`, house rule feedback_post_only_route_guards); the
//! list/show reads = any authed token (`token_ok`, GET).
//!
//! NORMALIZATION contract (pre-mortem #14): every handler here runs
//! its `domain` input through
//! `k2_core::mail_domain::normalize_mail_domain` BEFORE any lookup or
//! store — only the lowercase punycode A-label form ever reaches the
//! DB or Stalwart.

use std::collections::HashMap;

use crate::cli_response::CliResponse;

/// POST `/cli/mail/domain/add` — S2: normalize → Stalwart
/// `Domain/set create` (auto-DKIM) → store → return the DNS record
/// table from `dnsZoneFile`.
pub fn handle_domain_add(_body: &[u8]) -> CliResponse {
    super::not_built("S2", "POST /cli/mail/domain/add")
}

/// POST `/cli/mail/domain/remove` — S2: retire all its addresses
/// (count shown), destroy the Stalwart domain after explicit confirm;
/// data purge is a separate checkbox (PRD §6.6).
pub fn handle_domain_remove(_body: &[u8]) -> CliResponse {
    super::not_built("S2", "POST /cli/mail/domain/remove")
}

/// POST `/cli/mail/domain/check` — S2: force re-verification of every
/// record; per-record Valid/Missing/Wrong with expected-vs-live diff
/// (never a bare "missing", pre-mortem #4).
pub fn handle_domain_check(_body: &[u8]) -> CliResponse {
    super::not_built("S2", "POST /cli/mail/domain/check")
}

/// GET `/cli/mail/domain/list` — S2: all domains + status chips.
pub fn handle_domain_list(_params: &HashMap<String, String>) -> CliResponse {
    super::not_built("S2", "GET /cli/mail/domain/list")
}

/// GET `/cli/mail/domain/show` — S2: one domain's full record table.
pub fn handle_domain_show(_params: &HashMap<String, String>) -> CliResponse {
    super::not_built("S2", "GET /cli/mail/domain/show")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stubs_answer_structured_501() {
        for resp in [
            handle_domain_add(b"{}"),
            handle_domain_remove(b"{}"),
            handle_domain_check(b"{}"),
            handle_domain_list(&HashMap::new()),
            handle_domain_show(&HashMap::new()),
        ] {
            assert_eq!(resp.status, "501 Not Implemented");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "not_built");
            assert!(v["error"]["hint"]
                .as_str()
                .expect("hint")
                .contains("not built yet — mail slice S2"));
        }
    }
}
