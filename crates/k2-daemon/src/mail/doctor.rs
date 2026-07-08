//! Deliverability doctor — SKELETON (built in mail slice S6).
//!
//! When built (PRD §9): SPF/DKIM/DMARC evaluation via the `mail-auth`
//! crate (Apache/MIT — linked into the daemon; NOT a Stalwart crate),
//! DNS + TCP probes (outbound :25, PTR/FCrDNS, MX, cert/STARTTLS),
//! DNSBL checks, the open-relay self-test (pre-mortem #3: activation
//! fails CLOSED if this can't run), and the end-to-end probe to the
//! K2-operated seed mailbox — the ONLY external send development is
//! ever allowed to make (pre-mortem #1). Results persist to
//! `mail_doctor_runs` with a `pass|warn|fail` direct-send grade that
//! GATES enabling direct mode (§8.3).

/// S6 — run the full check table for `domain` (None = server-level
/// checks only: network, PTR, blocklists). Persists a
/// `mail_doctor_runs` row and returns the MiaB-style ✓/✖/? results +
/// grade as JSON.
#[allow(dead_code)] // S6 wires this into /cli/mail/doctor + nightly.
pub fn run(domain: Option<&str>) -> Result<serde_json::Value, String> {
    let _ = domain;
    Err(super::not_built_err("S6", "mail deliverability doctor"))
}

/// S6 — the most recent stored run for `domain` (None = server-level),
/// read from `mail_doctor_runs`. The Settings card + the direct-mode
/// gate read this; they never trigger a live run themselves.
#[allow(dead_code)] // S6 wires this into routes + the direct-mode gate.
pub fn latest_run(domain: Option<&str>) -> Result<serde_json::Value, String> {
    let _ = domain;
    Err(super::not_built_err("S6", "mail doctor history read"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skeletons_return_structured_not_built_errors() {
        assert!(run(None).unwrap_err().contains("not built yet — mail slice S6"));
        assert!(run(Some("acme.dev"))
            .unwrap_err()
            .contains("not built yet — mail slice S6"));
        assert!(latest_run(None)
            .unwrap_err()
            .contains("not built yet — mail slice S6"));
    }
}
