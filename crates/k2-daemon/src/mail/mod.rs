//! K2 Mail — the daemon-side email-server family (prd-email-server-v1).
//!
//! K2 does NOT implement SMTP. The daemon installs, configures, and
//! supervises **Stalwart** (Rust all-in-one mail server, AGPL-3.0
//! Community) as a **sidecar process**. The boundary rules below are
//! LICENSE rules as much as architecture rules (PRD §4, pre-mortem #2)
//! — every slice built on this module must keep them:
//!
//! 1. **Stalwart code is never linked, vendored, or patched.** No
//!    Stalwart crate in any Cargo.toml, no copied source file, ever.
//!    Communication is EXCLUSIVELY Stalwart's public HTTP management
//!    API ([`jmap`]) + systemd ([`supervisor`]). If the mgmt API can't
//!    do something, that's a feature gap to design around — not a
//!    reason to reach into their code.
//! 2. **Binaries come from upstream at install time.** The supervisor
//!    downloads the PINNED release tarball from Stalwart's own GitHub
//!    releases (sha256-verified); K2 never redistributes the binary.
//! 3. **K2-relevant state lives in K2's DB.** Agent↔address ownership,
//!    approvals, caps, doctor history → the `mail_*` tables (migration
//!    0072). Stalwart holds only what a mail server holds: domains,
//!    accounts, messages, DKIM keys. Never mirror Stalwart state we
//!    don't need; never store K2 governance state in Stalwart.
//! 4. **The mgmt API endpoint is DISCOVERED, never hardcoded.** Read
//!    the JMAP session document at `/.well-known/jmap` (PRD §4.1) —
//!    upstream docs disagree about `/api` vs `/jmap`.
//! 5. **Linux-only at runtime, compiled everywhere.** The whole module
//!    compiles + unit-tests on macOS; [`supervisor::mail_supported`]
//!    (a RUNTIME `cfg!` check) is the single capability gate the
//!    routes and the Mac UI read (pre-mortem #15).
//!
//! Module map (PRD §4):
//! - [`supervisor`] — install / bootstrap / health / upgrade /
//!   disable / uninstall of the Stalwart sidecar (S1). The ONLY module
//!   that knows Stalwart exists as a process.
//! - [`jmap`] — the typed client for Stalwart's management API.
//! - [`domains`] — S2 domain onboarding: zone-file parsing, the DNS
//!   record table, SPF split-config, add/remove/list/show ops (behind
//!   the `DomainEngine` trait so tests never touch a network).
//! - [`dns_verify`] — S2 DNS verification: the `DnsResolver` trait
//!   (production: hickory system resolver; tests: canned answers),
//!   per-record Valid/Missing/Wrong classification with expected-vs-
//!   live diffs, and the background re-verification poller
//!   (`dns_verify::spawn`, registered in main.rs next to the other
//!   background loops).
//! - [`doctor`] — deliverability checks (S6; `mail-auth` crate,
//!   DNS/TCP probes, blocklists, the direct-send readiness grade).
//! - `routes_*` — per-concern `/cli/mail/*` handlers, dispatched by
//!   the thin `crate::mail_routes` shim so later slices don't collide:
//!   [`routes_server`] (status/enable/disable/uninstall/config/doctor),
//!   [`routes_domains`], [`routes_addresses`], [`routes_messages`],
//!   [`routes_send`].

pub mod dns_verify;
pub mod doctor;
pub mod domains;
pub mod jmap;
pub mod routes_addresses;
pub mod routes_domains;
pub mod routes_messages;
pub mod routes_send;
pub mod routes_server;
pub mod supervisor;

use crate::cli_response::CliResponse;

/// The structured "not built yet" reply every stub handler returns —
/// 501 with a stable machine code so callers (and later the CLI) can
/// tell "route reserved for a future slice" from a real 404.
pub(crate) fn not_built(slice: &str, what: &str) -> CliResponse {
    CliResponse {
        status: "501 Not Implemented",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": "not_built",
                "hint": format!("{what}: not built yet — mail slice {slice}"),
            },
        })
        .to_string(),
    }
}

/// The same "not built yet" contract for non-route skeleton fns
/// ([`supervisor`], [`doctor`], the typed [`jmap`] calls): a structured
/// one-line error, never a panic/todo!() — a stray call in production
/// must fail loudly AND recoverably.
pub(crate) fn not_built_err(slice: &str, what: &str) -> String {
    format!("{what}: not built yet — mail slice {slice}")
}
