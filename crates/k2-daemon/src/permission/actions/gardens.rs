//! E1 — garden.keep-mine widget= version= (K2-presented; GardenOwner floor via `zen::garden_owner_may`, which Settings calls too, §5.2.1).
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
