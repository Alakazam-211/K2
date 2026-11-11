//! Named starter permission sets a template's `suggest` may name as
//! `set:<name>` (prd-agent-templates-v1 §5).
//!
//! The lists themselves are K2 Onboarding's, kept in one module so either
//! one is a one-line change (prd-setup-checklist-v1 §10.2):
//! `crates/k2-core/src/hire/starter_permissions.rs`. A set this list
//! doesn't name is unknown, and a template naming one fails to load (fail
//! closed, and the "every built-in loads" test says so).
//!
//! A set may hold one-time actions (`connections.add`, `sidecar.retire`):
//! those have no standing state, so the permissions step (T3) leaves them
//! as card asks. `chat.refresh` means its standing twin, the chat_refresh
//! toggle (AT6).

/// The ids in starter set `name`, or `None` for a set K2 doesn't know.
pub fn named_set(name: &str) -> Option<&'static [&'static str]> {
    const SETS: &[(&str, &[&str])] = &[
        ("k5-starter", crate::hire::starter_permissions::K5_STARTER_PERMISSIONS),
        ("chief-of-staff-starter", crate::hire::starter_permissions::CHIEF_OF_STAFF_STARTER_PERMISSIONS),
    ];
    SETS.iter().find(|(n, _)| *n == name).map(|(_, ids)| *ids)
}
