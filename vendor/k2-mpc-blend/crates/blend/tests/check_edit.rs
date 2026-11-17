//! A live change's rules (`k2_mpc_blend::live::check_edit`), the write-back's and a host's
//! check of a person's edit before it applies: each rule refuses with its code, the first one
//! broken wins, and on generated files the verdict is the write-back's for each candidate edit.
//! The rows are in `check_edit_support/mod.rs`, shared with the planted bugs
//! (`tests/mutants.rs`).

mod check_edit_support;
mod driven_support;
mod live_support;

use check_edit_support as rows;

fn pass(row: fn() -> Result<(), String>) {
    // one line naming the row and its step, so a log shows where it failed
    if let Err(e) = row() {
        panic!("row failed at its step: {}", e.replace('\n', " | "));
    }
}

#[test]
fn takes_what_live_writes() {
    pass(rows::row_check_edit_takes_what_live_writes);
}

#[test]
fn refuses_list_ops() {
    pass(rows::row_check_edit_refuses_list_ops);
}

#[test]
fn refuses_collection_fields() {
    pass(rows::row_check_edit_refuses_collection_fields);
}

#[test]
fn refuses_a_driven_transform() {
    pass(rows::row_check_edit_refuses_driven_xform);
}

#[test]
fn refuses_a_rotation_on_quaternion() {
    pass(rows::row_check_edit_refuses_rotation_on_quaternion);
}

#[test]
fn refuses_an_object_nothing_writes() {
    pass(rows::row_check_edit_refuses_never);
}

#[test]
fn refuses_visibility_with_an_animated_bit() {
    pass(rows::row_check_edit_refuses_animated_visibility);
}

#[test]
fn name_rules() {
    pass(rows::row_check_edit_name_rules);
}

#[test]
fn refuses_other_fields() {
    pass(rows::row_check_edit_refuses_other_fields);
}

#[test]
fn first_rule_broken_wins() {
    pass(rows::row_check_edit_first_rule);
}

#[test]
fn agrees_with_the_write_back() {
    pass(rows::row_check_edit_agrees_with_write_back);
}

#[test]
fn the_file_verb_writes_a_constrained_transform_live_refuses_it() {
    pass(rows::row_file_verb_writes_constrained_live_refuses);
}

#[test]
fn the_file_verb_writes_an_undriven_component_live_refuses_it() {
    pass(rows::row_file_verb_writes_undriven_component_live_refuses);
}
