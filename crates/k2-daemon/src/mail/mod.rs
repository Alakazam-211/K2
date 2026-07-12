//! K2 Mail — the daemon-side email-server family (prd-email-server-v1).
//!
//! K2 does NOT implement SMTP. The daemon installs, configures, and
//! supervises **Stalwart** (Rust all-in-one mail server, AGPL-3.0
//! Community) as a **sidecar process**. The boundary rules below are
//! LICENSE rules as much as architecture rules (PRD §4, pre-mortem #2)
//! — every slice built on this module must keep them:
//!
//! 1. **Stalwart SERVER code is never linked, vendored, or patched.**
//!    No stalwart-mail crate in any Cargo.toml, no copied source file,
//!    ever. Communication is EXCLUSIVELY Stalwart's public HTTP
//!    management API ([`jmap`]) + systemd ([`supervisor`]). If the
//!    mgmt API can't do something, that's a feature gap to design
//!    around — not a reason to reach into their code. (The rule is
//!    about the AGPL server: `mail-parser`, a standalone
//!    Apache-2.0/MIT LIBRARY that happens to share an author, is
//!    linked for S9's RFC 822 parsing and is fine.)
//! 2. **Binaries come from upstream at install time.** The supervisor
//!    downloads the PINNED release tarball from Stalwart's own GitHub
//!    releases (sha256-verified); K2 never redistributes the binary.
//! 3. **K2-relevant state lives in K2's DB.** Agent↔address ownership,
//!    approvals, caps, doctor history → the `mail_*` tables (migration
//!    0075). Stalwart holds only what a mail server holds: domains,
//!    accounts, messages, DKIM keys. Never mirror Stalwart state we
//!    don't need; never store K2 governance state in Stalwart.
//! 4. **The mgmt API endpoint is DISCOVERED, never hardcoded.** Read
//!    the JMAP session document at `/jmap/session` (live-verified on
//!    v0.16.10 — `/.well-known/jmap` is empty there) and REBASE the
//!    served urls onto the loopback base (normal mode serves absolute
//!    urls on the mail hostname).
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
//! - [`doctor`] — S6 deliverability doctor: the MiaB-style check table
//!   (PTR/FCrDNS, outbound-25 with provider coaching, DNSBLs, the
//!   open-relay self-test, STARTTLS/cert, disk, per-domain DNS
//!   posture via `dns_verify`), persisted `mail_doctor_runs` history,
//!   and the `direct_send_gate` that locks direct mode on a failing
//!   grade. Probes behind the `DoctorEnv` trait (+ the shared
//!   `DnsResolver`) — no real network in tests.
//! - [`config`] — S6 owner config surface: per-domain send mode
//!   (doctor-gated `direct`, relay-route push/clear through jmap's
//!   single ⚠ LIVE-BOX #7 function), kind-agnostic relay-config CRUD
//!   (secrets only ever as refs), and the D4/D6 gating settings
//!   (per-workspace + global).
//! - [`external`] — S9 external assistant inboxes (PRD §17.5): the
//!   user's OWN accounts (Gmail app-password, Fastmail, company IMAP)
//!   bound to exactly ONE workspace; agents READ them through the
//!   §17.5 seam and save reply DRAFTS into the account's real Drafts
//!   folder. No send path exists. Everything behind the `ImapOps`
//!   trait; credentials in the vault under `ext-inbox-<row-id>`.
//! - [`external_imap`] — the production `ImapOps`: the `imap` crate
//!   over rustls (verification always on), the ONLY module speaking
//!   IMAP wire protocol. Its module header lists the ⚠ LIVE-BOX
//!   functions (SPECIAL-USE-in-plain-LIST variance, SEARCH charset).
//! - `routes_*` — per-concern `/cli/mail/*` handlers, dispatched by
//!   the thin `crate::mail_routes` shim so later slices don't collide:
//!   [`routes_server`] (status/enable/disable/uninstall/config/doctor),
//!   [`routes_domains`], [`routes_addresses`], [`routes_messages`],
//!   [`routes_send`], [`routes_external`] (external CRUD + draft).

pub mod access;
pub mod addresses;
pub mod config;
pub mod dns_verify;
pub mod doctor;
pub mod domains;
pub mod external;
pub mod external_imap;
pub mod external_smtp;
pub mod graph;
pub mod jmap;
pub mod messages;
pub mod oauth;
pub mod preflight;
pub mod routes_access;
pub mod routes_addresses;
pub mod routes_domains;
pub mod routes_external;
pub mod routes_link_oauth;
pub mod routes_messages;
pub mod routes_send;
pub mod routes_server;
pub mod secrets;
pub mod send;
pub mod supervisor;
pub mod sysops;

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

/// The "not built yet" contract for non-route skeleton fns (today only
/// [`supervisor`]'s upgrade op): a structured one-line error, never a
/// panic/todo!() — a stray call in production must fail loudly AND
/// recoverably. (The route-side 501 twin retired with S6 — every
/// `/cli/mail/*` route in the partition map is real now.)
pub(crate) fn not_built_err(slice: &str, what: &str) -> String {
    format!("{what}: not built yet — mail slice {slice}")
}
