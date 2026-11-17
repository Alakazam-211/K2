//! A still after a live edit without a whole-file read: a live state's K2SCENE1 written over the
//! carrier's mesh read (`live::reuse`), byte for byte the whole read of a version that holds
//! the state; the carrier id, the shared data section, no decode, the three lists and the
//! declines. The rows are in `reuse_support`, shared with the planted bugs (`tests/mutants.rs`).

mod live_support;
mod reuse_support;

fn pass(row: fn() -> Result<(), String>) {
    if let Err(e) = row() {
        panic!("{e}");
    }
}

#[test]
fn the_output_is_the_whole_read_of_a_version_holding_the_state() {
    pass(reuse_support::row_reuse_parity);
}

#[test]
fn headers_of_every_pad_stay_byte_identical() {
    pass(reuse_support::row_reuse_header_lengths);
}

#[test]
fn the_carrier_id_is_the_data_sections_hash() {
    pass(reuse_support::row_reuse_carrier_id);
}

#[test]
fn calls_on_one_carrier_share_its_data() {
    pass(reuse_support::row_reuse_shared_data);
}

#[test]
fn no_call_decodes_a_file() {
    pass(reuse_support::row_reuse_no_decode);
}

#[test]
fn the_lists_name_what_changed() {
    pass(reuse_support::row_reuse_key_lists);
}

#[test]
fn each_decline_is_typed() {
    pass(reuse_support::row_reuse_declines);
}
