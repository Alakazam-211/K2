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
//! - [`preflight`] — the §5.1 read-only checklist (S1); pure logic
//!   over an injected environment trait.
//! - [`sysops`] — every fs/systemd/download/journald effect behind one
//!   trait (S1) so the whole install flow unit-tests as a sequence.
//! - [`secrets`] — the 0600-file secret store behind the
//!   `mail_server.*_secret_ref` columns (S1).
//! - [`jmap`] — the typed client for Stalwart's management API.
//! - [`domains`] — S2 domain onboarding: zone-file parsing, the DNS
//!   record table, SPF split-config, add/remove/list/show ops (behind
//!   the `DomainEngine` trait so tests never touch a network).
//! - [`addresses`] — S3 agent address minting: local-part rule, caps
//!   (ACTIVE-counted, §11.1.5), idempotent `--id`, retire-with-
//!   retention, Stalwart account lifecycle behind the `AddressEngine`
//!   trait (compensating destroy on late mint failure — no orphans).
//! - [`dns_verify`] — S2 DNS verification: the `DnsResolver` trait
//!   (production: hickory system resolver; tests: canned answers),
//!   per-record Valid/Missing/Wrong classification with expected-vs-
//!   live diffs, and the background re-verification poller
//!   (`dns_verify::spawn`, registered in main.rs next to the other
//!   background loops).
//! - [`messages`] — S4 read + wait ops: the §17.5 provider seam
//!   (`backend_for_address` + the `ReadBackend` trait — routes never
//!   assume local Stalwart), §8.1 message shaping with the untrusted-
//!   content markers, the HTML-strip fallback, auth-verdict parsing,
//!   workspace-jailed attachment output paths, and the §8.2 wait loop
//!   with an injected clock/poller.
//! - [`send`] — S5 outbound ops: the D4 fail-closed gate
//!   (off/approval/on), the `mail_outbound` audit-then-maybe-submit
//!   pipeline (pre-mortem #11: no row, no send), always-on rate limits,
//!   send-mode enforcement (receive-only refusal + use-time relay
//!   validation), §8.4 reply guardrails, approve/deny transitions, the
//!   `--wait` decision poll — all behind `OutboundStore`/
//!   `SubmitBackend` traits with injected fakes/clocks in tests.
//! - [`doctor`] — deliverability checks (S6; `mail-auth` crate,
//!   DNS/TCP probes, blocklists, the direct-send readiness grade).
//! - `routes_*` — per-concern `/cli/mail/*` handlers, dispatched by
//!   the thin `crate::mail_routes` shim so later slices don't collide:
//!   [`routes_server`] (status/enable/disable/uninstall/config/doctor),
//!   [`routes_domains`], [`routes_addresses`], [`routes_messages`],
//!   [`routes_send`].

pub mod addresses;
pub mod dns_verify;
pub mod doctor;
pub mod domains;
pub mod jmap;
pub mod messages;
pub mod preflight;
pub mod routes_addresses;
pub mod routes_domains;
pub mod routes_messages;
pub mod routes_send;
pub mod routes_server;
pub mod secrets;
pub mod send;
pub mod supervisor;
pub mod sysops;

use crate::cli_response::CliResponse;

/// Serializes every test that touches the SINGLETON `mail_server` row
/// (the process-global shared test DB makes concurrent row writers
/// race — the tree has been bitten by exactly this class of flake).
/// Every test that inserts/updates/deletes `mail_server` MUST hold
/// this guard for its whole body.
#[cfg(test)]
pub(crate) fn mail_server_test_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

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
