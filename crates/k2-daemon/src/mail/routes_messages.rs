//! `/cli/mail/messages|read|attachments|wait` — READ-concern handlers
//! (built in mail slice S4: JMAP read proxy + the long-poll `wait`
//! verification-code primitive).
//!
//! Dispatched by the `crate::mail_routes` shim. AUTH/GATING contract
//! (PRD §8.1): reads are always allowed for the OWNING agent, scoped
//! server-side to addresses the caller owns — GETs with `token_ok`.
//! `wait` is a long-poll (UDS-eligible) with a single watcher per
//! mailbox fanned out to callers + per-agent rate limits (pre-mortem
//! #10 — wired from day one, not deferred).
//!
//! UNTRUSTED-CONTENT contract (PRD §8.1): message bodies returned
//! here are EXTERNAL UNTRUSTED input. The CLI wraps them in
//! BEGIN/END EXTERNAL EMAIL markers; nothing from a body is ever
//! executed or auto-followed by K2 itself. Attachments are never
//! auto-opened. Bodies are never logged.

use std::collections::HashMap;

use crate::cli_response::CliResponse;

/// GET `/cli/mail/messages` — S4: newest-first summaries for owned
/// addresses (`k2 mail messages`).
pub fn handle_messages(_params: &HashMap<String, String>) -> CliResponse {
    super::not_built("S4", "GET /cli/mail/messages")
}

/// GET `/cli/mail/read` — S4: one full message (parsed text, html on
/// demand, attachments index, SPF/DKIM/DMARC verdicts); marks read.
pub fn handle_read(_params: &HashMap<String, String>) -> CliResponse {
    super::not_built("S4", "GET /cli/mail/read")
}

/// GET `/cli/mail/attachments` — S4: list, or fetch one attachment's
/// bytes to a caller-named path (never auto-opened).
pub fn handle_attachments(_params: &HashMap<String, String>) -> CliResponse {
    super::not_built("S4", "GET /cli/mail/attachments")
}

/// GET `/cli/mail/wait` — S4: long-poll for a matching incoming
/// message (`--to/--from/--subject/--timeout`, 60 s grace window,
/// timeout → CLI exit 2).
pub fn handle_wait(_params: &HashMap<String, String>) -> CliResponse {
    super::not_built("S4", "GET /cli/mail/wait")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stubs_answer_structured_501() {
        for resp in [
            handle_messages(&HashMap::new()),
            handle_read(&HashMap::new()),
            handle_attachments(&HashMap::new()),
            handle_wait(&HashMap::new()),
        ] {
            assert_eq!(resp.status, "501 Not Implemented");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "not_built");
            assert!(v["error"]["hint"]
                .as_str()
                .expect("hint")
                .contains("not built yet — mail slice S4"));
        }
    }
}
