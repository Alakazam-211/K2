//! Terminal grid reflow (re-wrap a screen at another width).
//!
//! The algorithm lives in `k2-term-grid` (to be published as kessel-grid).
//! This wrapper keeps the `k2_core::terminal::reflow::reflow_grid` path and
//! adds K2's perf histogram around each call.

use super::grid_types::GridUpdate;

/// Reflow a GridUpdate from its original column width to a target width.
/// Returns a new GridUpdate with lines re-wrapped at `target_cols`.
pub fn reflow_grid(grid: &GridUpdate, target_cols: u16, target_rows: u16) -> GridUpdate {
    let _h = crate::perf_hist!("reflow");
    k2_term_grid::reflow::reflow_grid(grid, target_cols, target_rows)
}
