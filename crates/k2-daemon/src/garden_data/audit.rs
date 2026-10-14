//! Garden data audit (contract §10; PRD §6.4, GL43). C0 → B1.
//!
//! Data reads and writes are **counted**: one `zen.data.counts` line per
//! placement or agent per minute with activity. Grants, revokes,
//! delegations, space delete/move/import/export, refusals, pauses and
//! signing are logged one by one. Bodies, keys and values are never logged.

/// One counted data call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counted {
    Read,
    Write { ops: usize, bytes: usize },
    Delete,
    Conflict,
}

/// Count one call for `actor` (audit label) on `space`. `made_by` is the
/// widget's maker for widget calls. C0: dropped.
pub fn count(_space: &str, _actor: &str, _made_by: Option<&str>, _what: Counted) {}

/// One line now (`zen.data.grant`, `zen.data.revoke`, refusals, …).
/// `detail` names ids, prefixes and codes, never values. C0: dropped.
pub fn line(_event: &str, _actor: &str, _detail: &str) {}

/// Write the per-minute count lines (the ticker calls this). C0: nothing.
pub fn flush() {}
