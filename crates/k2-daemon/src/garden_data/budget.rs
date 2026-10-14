//! Garden data budgets (contract §3.6; PRD §5.3, §6.3). C0 → B1.
//!
//! Token buckets, in memory: writes 60/s burst 120 per placement and per
//! agent per space; reads 60/s per placement; saves 1,200 per 10 min per
//! placement; a transaction commit costs `ceil(ops / 20)` write tokens plus
//! its bytes against 4 MiB per minute; 30 grant changes per minute per
//! delegate. The runaway guard pauses a placement after 100 identical
//! writes (same key and value hash) in 10 minutes (`PauseReason::Writes`;
//! Resume is person-only).

use k2_core::garden_data::DataError;

/// Who a budget belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BudgetKey {
    Placement { garden: String, placement: String },
    Agent { project_id: String, space: String },
    Delegate { project_id: String },
}

/// What is being spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spend {
    Read,
    Write,
    Save,
    /// A commit of this many ops and bytes.
    Commit { ops: usize, bytes: usize },
    GrantChange,
}

/// Spend one unit, or answer `rate_limited` with `retryAfterSecs`.
/// C0: always allows.
pub fn spend(_key: &BudgetKey, _what: Spend) -> Result<(), DataError> {
    Ok(())
}

/// Record one write for the runaway guard. Answers true when the placement
/// must now be paused. C0: never.
pub fn runaway(_garden: &str, _placement: &str, _key: &str, _value_hash: &str) -> bool {
    false
}
