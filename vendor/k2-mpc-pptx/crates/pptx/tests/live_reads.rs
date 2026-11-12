//! Declines and bounded reads on a live deck: `slide show` pages of at most 64 shapes with
//! text cut at 1,024 bytes, `shape show` cut at 16,384, `slide list` paged; growth past the
//! keyed cap refused (the kit's `MPC-E822`) and text over the field limit refused.

mod common;

use common::live_rows;

#[test]
fn reads_bounded_and_paged() {
    live_rows::row_reads_bounded_and_paged().unwrap();
}

#[test]
fn limits_refuse_growth_and_long_text() {
    live_rows::row_limits_refuse_growth_and_long_text().unwrap();
}
