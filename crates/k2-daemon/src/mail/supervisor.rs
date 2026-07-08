//! Stalwart sidecar supervisor — SKELETON (built in mail slice S1).
//!
//! The ONLY module that knows Stalwart exists as a process (PRD §4.1).
//! Responsibilities when built: preflight, pinned install (download
//! from upstream GitHub releases + sha256 verify — never build, never
//! link, pre-mortem #2), systemd unit + K2's hardening drop-in (§10),
//! bootstrap (capture one-time admin password, mint the least-privilege
//! `k2-daemon` ApiKey, disable IMAP/POP3/ManageSieve/CalDAV + the :8080
//! setup listener), health, explicit pinned upgrades with snapshot +
//! auto-rollback (pre-mortem #8), disable vs uninstall (§4.1).
//!
//! Every fn below is an unimplemented-on-purpose skeleton returning the
//! structured [`crate::mail::not_built_err`] error — callable, loud,
//! recoverable. Signatures are the S1 contract; bodies land in S1.

/// The PINNED Stalwart version the supervisor installs and manages.
///
/// PLACEHOLDER "0.16.0" — verify the latest v0.16.x at S1-build time
/// and update (plus the baked-in release-tarball sha256 checksums that
/// land next to it in S1). Pinning is load-bearing: v0.16 removed the
/// whole REST API relative to earlier lines, upstream has no mgmt-API
/// stability policy, and the config format churns between minors
/// (PRD §4, §16, pre-mortem #8). Upgrades are explicit, K2-shipped,
/// tested operations — the supervisor must REFUSE to manage an
/// unrecognized on-disk version (clear error, no writes), and the
/// daemon updating itself never touches the Stalwart version.
pub const STALWART_PINNED_VERSION: &str = "0.16.0";

/// The single capability gate for the whole mail family (D3 +
/// pre-mortem #15): V1 runs on **Linux deployments only**.
///
/// RUNTIME `cfg!`, deliberately NOT a compile-time `#[cfg]` on the
/// module — the module compiles and unit-tests on macOS, and the
/// `/cli/mail/status` route reports `supported: false` so the Mac
/// Settings→Email page renders its example-only state from the
/// DAEMON's answer (never from `navigator.platform`: a Mac app driving
/// a remote Linux daemon must see the REAL page).
pub fn mail_supported() -> bool {
    cfg!(target_os = "linux")
}

/// S1 — read-only preflight (PRD §5.1): OS check, existing-MTA check,
/// port bindability (25/465/587), :443-free probe (picks the TLS plan,
/// §5.3), public IP + rDNS, outbound-:25 probe (pre-mortem #6), disk/
/// RAM. Returns the MiaB-style checklist as JSON when built.
#[allow(dead_code)] // S1 wires this into the enable flow.
pub fn preflight() -> Result<serde_json::Value, String> {
    Err(super::not_built_err("S1", "mail supervisor preflight"))
}

/// S1 — download the pinned release from upstream GitHub, verify
/// sha256 against checksums baked into the daemon at build time,
/// install to /usr/local/bin/stalwart, create the `stalwart` system
/// user, write the systemd unit + hardening drop-in (PRD §10), enable.
/// Idempotent/resumable; each step surfaces its real error.
#[allow(dead_code)] // S1 wires this into the enable flow.
pub fn install(hostname: &str) -> Result<(), String> {
    let _ = hostname;
    Err(super::not_built_err("S1", "mail supervisor install"))
}

/// S1 — first-run bootstrap (PRD §4.1): capture the one-time admin
/// password from journald, set hostname + listeners per the port plan,
/// disable IMAP/POP3/ManageSieve/CalDAV, enable spam filter, mint the
/// least-privilege localhost-allowlisted `k2-daemon` ApiKey, rotate the
/// admin password into the daemon's secret store, and DISABLE the
/// :8080 setup listener (pre-mortem #13: mgmt binds localhost only).
#[allow(dead_code)] // S1 wires this into the enable flow.
pub fn bootstrap() -> Result<(), String> {
    Err(super::not_built_err("S1", "mail supervisor bootstrap"))
}

/// S1 — systemd state + authenticated API ping, mapped onto the
/// `mail_server.status` lifecycle (`running`/`degraded`/`stopped`/…).
/// Runs on the existing heartbeat cadence; failures raise the standard
/// daemon event → app notification.
#[allow(dead_code)] // S1 wires this into the heartbeat cadence.
pub fn health_check() -> Result<serde_json::Value, String> {
    Err(super::not_built_err("S1", "mail supervisor health check"))
}

/// Post-S1 — explicit pinned upgrade: snapshot config + data dir,
/// swap the verified binary, health-check, auto-rollback on failure
/// (pre-mortem #8). NEVER called from any auto-update path.
#[allow(dead_code)] // wired when the first pin bump ships.
pub fn upgrade(to_version: &str) -> Result<(), String> {
    let _ = to_version;
    Err(super::not_built_err("S1", "mail supervisor upgrade"))
}

/// S1 — disable: stop + disable the unit, KEEP data. Domains stay
/// verified while MX points at a dead port — the route layer warns
/// loudly per PRD §4.1.
#[allow(dead_code)] // S1 wires this into /cli/mail/server/disable.
pub fn disable() -> Result<(), String> {
    Err(super::not_built_err("S1", "mail supervisor disable"))
}

/// S1 — uninstall: disable + optional explicit purge of
/// /var/lib/stalwart (double-confirmed route-side; `purge_data` is
/// only honored after the hostname-typed confirmation).
#[allow(dead_code)] // S1 wires this into /cli/mail/server/uninstall.
pub fn uninstall(purge_data: bool) -> Result<(), String> {
    let _ = purge_data;
    Err(super::not_built_err("S1", "mail supervisor uninstall"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The capability gate matches the compile target — on macOS dev
    /// boxes this asserts FALSE (and the module still compiled + ran,
    /// which is the point of the runtime cfg!).
    #[test]
    fn mail_supported_matches_target_os() {
        assert_eq!(mail_supported(), cfg!(target_os = "linux"));
    }

    /// Skeletons fail loudly with the structured not-built error —
    /// never panic, never silently succeed.
    #[test]
    fn skeletons_return_structured_not_built_errors() {
        for (label, err) in [
            ("preflight", preflight().unwrap_err()),
            ("install", install("mail.acme.dev").unwrap_err()),
            ("bootstrap", bootstrap().unwrap_err()),
            ("health", health_check().unwrap_err()),
            ("upgrade", upgrade("0.16.1").unwrap_err()),
            ("disable", disable().unwrap_err()),
            ("uninstall", uninstall(false).unwrap_err()),
        ] {
            assert!(
                err.contains("not built yet — mail slice S1"),
                "{label}: {err}"
            );
        }
    }
}
