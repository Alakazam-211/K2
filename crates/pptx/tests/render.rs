//! The render hash on the golden sample deck: the golden turn and its variants. The rows live
//! in `common/hash_rows.rs`, so the planted faults in `tests/mutants.rs` run the same ones.

mod common;

use common::hash_rows;

#[test]
fn turn_revs_41_to_46() {
    hash_rows::row_turn().unwrap();
}

#[test]
fn v1_slide_added_first() {
    hash_rows::row_v1_slide_added_first().unwrap();
}

#[test]
fn v2_table_style() {
    hash_rows::row_v2_table_style().unwrap();
}

#[test]
fn v3_one_layout() {
    hash_rows::row_v3_one_layout().unwrap();
}

#[test]
fn v5_theme() {
    hash_rows::row_v5_theme().unwrap();
}

#[test]
fn v6_linked_slide() {
    hash_rows::row_v6_linked_slide().unwrap();
}
