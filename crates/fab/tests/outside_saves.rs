//! The photoplot pack through outside saves in a kit session kept in sync with the file
//! (`support::saves`): a no-change save, a churn save that renumbers every aperture, a reuse
//! save that swaps two apertures' numbers, and a person's write through the kit between two
//! outside saves.

mod support;

use support::saves;

#[test]
fn a_no_change_save_makes_no_version_and_moves_no_blame() {
    saves::row_no_change_save().unwrap();
}

#[test]
fn a_churn_save_keeps_every_key_and_only_renames() {
    saves::row_churn_save_keeps_every_key().unwrap();
}

#[test]
fn a_reuse_save_keeps_each_definition_on_its_key() {
    saves::row_reuse_save_keeps_each_definition().unwrap();
}

#[test]
fn blame_credits_a_kit_write_between_two_outside_saves() {
    saves::row_kit_write_between_outside_saves().unwrap();
}
