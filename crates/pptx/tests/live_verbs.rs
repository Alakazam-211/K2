//! The agent's verbs on a live deck, through the kit's session: one change event per `shape move`,
//! each verb's ops and typed refusals, the agent's base, a new shape's id after a group, `slide
//! add`'s ids, a watcher, the differential oracle against the file verbs, bounded reads and the
//! limits. The rows live in `common/live_rows.rs`, so the planted faults in `tests/mutants.rs` run
//! the same ones.

mod common;

use common::live_rows;

#[test]
fn shape_move_one_event() {
    live_rows::row_shape_move_one_event().unwrap();
}

#[test]
fn each_verb() {
    live_rows::row_each_verb().unwrap();
}

#[test]
fn slide_add_ids_as_the_file_verb() {
    live_rows::row_slide_add_ids_as_the_file_verb().unwrap();
}

#[test]
fn new_shape_after_a_group() {
    live_rows::row_new_shape_after_a_group().unwrap();
}

#[test]
fn base_required_and_checked() {
    live_rows::row_base_required_and_checked().unwrap();
}

#[test]
fn watch_sees_each_change() {
    live_rows::row_watch_sees_each_change().unwrap();
}

#[test]
fn differential_oracle() {
    live_rows::row_differential_oracle().unwrap();
}
