//! The pack's view index (the rendering-data contract r1.4, Item 1) against the contract's
//! hand-made goldens: geometry (gap 4), fonts, field kinds and generic names (gap 6), layouts
//! and placeholder types (gap 28). The rows live in `common/view_rows.rs`, so the planted
//! faults in `tests/mutants.rs` run the same ones.

mod common;

use common::view_rows::{self, DISAGREE};

#[test]
fn index_rev40() {
    view_rows::row_index_rev40().unwrap();
}

#[test]
fn case_d_approx_box() {
    view_rows::row_case_d().unwrap();
}

#[test]
fn case_d2_one_group_flip() {
    view_rows::row_case_d2().unwrap();
}

#[test]
fn case_d3_approx_inherited() {
    view_rows::row_case_d3().unwrap();
}

#[test]
fn case_d3_counter_turned_approx_inherited() {
    view_rows::row_case_d3_counter().unwrap();
}

#[test]
fn disagreements_with_the_goldens_are_listed() {
    for (case, field, golden, pack, why) in DISAGREE {
        println!("golden disagreement: case {case}, {field}: golden {golden}, pack {pack}: {why}");
    }
}
