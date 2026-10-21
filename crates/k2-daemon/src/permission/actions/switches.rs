//! E1 — the Agent tab's switches (PRD rev 3.1 §5.2): compute.use, compute.operate, compute.manage, agents.manage, projects.manage, apps.build, apps.manage-full, apps.core, chat.manage, vault.manage, gardens.manage, dns.manage, mail.manage, mail.send-mode, db.agent-access, and each switch's `to=off` twin. Table-driven over `sidecar_routes::access_entry`, plus the new entries in `misc_routes.rs` (dns), `mail_routes.rs` and `workspace_routes.rs` (§6.4).
//!
//! P0 stub: no action registered yet. Each spec passes
//! `k2_core::permission::catalog::validate` and the floor-parity test.

use k2_core::permission::catalog::NotCardableSpec;

use super::Registered;

pub(crate) fn actions() -> Vec<Registered> {
    Vec::new()
}

/// This area's rows that aren't card-able in v1 (never, later, dropped or
/// covered), so their refusals say "It can't be requested." or name the
/// covering action.
pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}
