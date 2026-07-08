//! `/cli/mail/send|reply|outbox|approvals/*` — SEND-concern handlers
//! (built in mail slice S5: gating, approvals queue, outbox, reply
//! guardrails, rate limits, audit).
//!
//! Dispatched by the `crate::mail_routes` shim. AUTH/GATING contract
//! (PRD §8.4 / §10): send/reply = WORKSPACE token (`token_ok`), gated
//! by the effective `mail_agent_send` mode
//! (`k2_core::workspace::settings::mail_agent_send_for_path` — off →
//! CLI exit 3 with the Settings pointer); approvals approve/deny =
//! OWNER-OR-ADMIN (`token_is_owner_or_admin`). All mutations POST-only
//! (`require_post` + `post_allowed`, feedback_post_only_route_guards).
//!
//! FAIL-CLOSED invariants (pre-mortem #11, binding on S5):
//! - every send writes a `mail_outbound` audit row BEFORE anything is
//!   handed to Stalwart — no row, no send;
//! - an unreachable queue table NEVER bypasses approval mode;
//! - sender identity is stamped server-side (From must be an address
//!   the caller owns — never trusted from args);
//! - Stalwart's queue owns retries; `send` reports
//!   "accepted-for-delivery", never "delivered" (pre-mortem #9).

use std::collections::HashMap;

use crate::cli_response::CliResponse;

/// POST `/cli/mail/send` — S5: gated outbound (off → exit-3 error;
/// approval → pending `mail_outbound` row; on → audit row + submit).
pub fn handle_send(_body: &[u8]) -> CliResponse {
    super::not_built("S5", "POST /cli/mail/send")
}

/// POST `/cli/mail/reply` — S5: guardrailed reply (recipient locked to
/// original sender, From locked to receiving address, DMARC-fail +
/// loop caps per PRD §8.4).
pub fn handle_reply(_body: &[u8]) -> CliResponse {
    super::not_built("S5", "POST /cli/mail/reply")
}

/// GET `/cli/mail/outbox` — S5: the caller's outbound rows
/// (pending / approved+sent / denied with note / failed).
pub fn handle_outbox(_params: &HashMap<String, String>) -> CliResponse {
    super::not_built("S5", "GET /cli/mail/outbox")
}

/// GET `/cli/mail/approvals/list` — S5: the pending-outbound queue
/// (owner's Approvals tab; expires to auto-deny after 7 days).
pub fn handle_approvals_list(_params: &HashMap<String, String>) -> CliResponse {
    super::not_built("S5", "GET /cli/mail/approvals/list")
}

/// POST `/cli/mail/approvals/approve` — S5: owner-or-admin approve →
/// daemon submits via Stalwart (audit row already exists).
pub fn handle_approvals_approve(_body: &[u8]) -> CliResponse {
    super::not_built("S5", "POST /cli/mail/approvals/approve")
}

/// POST `/cli/mail/approvals/deny` — S5: owner-or-admin deny with a
/// note that flows back to the agent.
pub fn handle_approvals_deny(_body: &[u8]) -> CliResponse {
    super::not_built("S5", "POST /cli/mail/approvals/deny")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stubs_answer_structured_501() {
        for resp in [
            handle_send(b"{}"),
            handle_reply(b"{}"),
            handle_outbox(&HashMap::new()),
            handle_approvals_list(&HashMap::new()),
            handle_approvals_approve(b"{}"),
            handle_approvals_deny(b"{}"),
        ] {
            assert_eq!(resp.status, "501 Not Implemented");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "not_built");
            assert!(v["error"]["hint"]
                .as_str()
                .expect("hint")
                .contains("not built yet — mail slice S5"));
        }
    }
}
