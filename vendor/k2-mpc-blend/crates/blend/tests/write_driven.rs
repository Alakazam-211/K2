//! The writer on animated channels: a change to a channel an action (directly, or through an
//! NLA strip) or a driver animates is refused, typed (`blend_driven`), because the program
//! evaluates the animation when it opens the file and the value written wouldn't hold
//! (`PROVENANCE.md`); a channel nothing animates is written. A visibility bit an action or a
//! driver animates is refused the same way when a change flips it; a transform written to an
//! object with constraints is written, with a warning (`blend_constrained`). The rows are in
//! `driven_support/mod.rs`, shared with the planted bugs in `tests/mutants.rs`.

mod driven_support;

use driven_support as rows;

fn run(r: rows::Row) {
    if let Err(e) = r {
        panic!("{e}");
    }
}

#[test]
fn a_patch_to_an_animated_channel_is_refused_typed() {
    run(rows::row_driven_z_refused());
}

#[test]
fn channels_nothing_animates_are_written_and_read_back() {
    run(rows::row_undriven_written());
}

#[test]
fn a_driver_drives_its_own_channel_only() {
    run(rows::row_driver_drives_its_channel());
}

#[test]
fn an_nla_strip_drives_its_actions_channels_only() {
    run(rows::row_nla_strip_drives_its_channel());
}

#[test]
fn animation_the_reader_cant_follow_drives_every_transform_channel() {
    run(rows::row_unfollowed_animation_drives_all());
}

#[test]
fn each_action_slot_drives_its_own_objects_channels() {
    run(rows::row_each_slot_drives_its_own());
}

#[test]
fn the_file_verbs_refuse_a_driven_channel() {
    run(rows::row_file_verb_refuses());
}

#[test]
fn the_live_tier_marks_driven_only_what_a_transform_channel_drives() {
    run(rows::row_live_follows_the_channels());
}

#[test]
fn the_live_tier_and_the_writer_take_one_rule() {
    run(rows::row_live_and_writer_agree());
}

#[test]
fn the_programs_saves_read_per_channel_through_bags_and_list() {
    run(rows::row_program_saves_read_per_channel());
}

#[test]
fn a_flip_of_an_animated_visibility_bit_is_refused_typed() {
    run(rows::row_driven_bit_refused());
}

#[test]
fn the_live_store_marks_exactly_the_bits_the_writer_refuses() {
    run(rows::row_live_marks_the_animated_bits());
}

#[test]
fn a_transform_written_under_constraints_comes_with_a_warning() {
    run(rows::row_constrained_written_with_warning());
}
