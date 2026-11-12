//! The splice (`live::splice::encode_from`) at the pack level, on bytes handed to it as the
//! kit's carrier hook K1 hands them. The rows live in `common/splice_rows.rs`, so the
//! planted faults in `tests/mutants.rs` run the same ones.

mod common;

use common::splice_rows;

#[test]
fn f5_untouched_byte_for_byte() {
    splice_rows::row_f5_untouched_byte_for_byte().unwrap();
}

#[test]
fn each_change_spliced() {
    splice_rows::row_each_change_spliced().unwrap();
}

#[test]
fn all_changes_at_once() {
    splice_rows::row_all_changes_at_once().unwrap();
}

#[test]
fn guard_refuses_what_the_file_cant_hold() {
    splice_rows::row_guard_refuses_what_the_file_cant_hold().unwrap();
}

#[test]
fn carrier_content_is_decoded() {
    splice_rows::row_carrier_content_is_decoded().unwrap();
}

#[test]
fn one_pass_per_encode_from() {
    splice_rows::row_one_pass_per_encode_from().unwrap();
}
