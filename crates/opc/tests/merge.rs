//! The package merge's rows, each on its own: colliding adds by format (re-minted, references
//! rewritten), content types and relationships merged by key, deletes, refusals to guess,
//! the soundness check, the identities.

mod common;

use common::merges;

fn run(f: fn() -> Result<(), String>) {
    if let Err(e) = f() {
        panic!("{e}");
    }
}

#[test]
fn docx_two_saves_each_adding_an_image_merge() {
    run(merges::two_images);
}

#[test]
fn pptx_two_saves_each_adding_a_slide_merge() {
    run(merges::two_slides);
}

#[test]
fn xlsx_two_saves_each_adding_a_sheet_merge() {
    run(merges::two_sheets);
}

#[test]
fn the_same_addition_on_both_sides_is_one() {
    run(merges::same_image_twice);
}

#[test]
fn content_types_merge_by_key() {
    run(merges::content_types);
}

#[test]
fn relationships_merge_by_id() {
    run(merges::relationship_changes);
}

#[test]
fn deletes_merge_and_conflict_with_changes() {
    run(merges::deletes);
}

#[test]
fn the_package_relationships_re_mint_too() {
    run(merges::package_relationships);
}

#[test]
fn an_id_that_is_not_only_a_reference_is_a_conflict() {
    run(merges::unknown_reference);
}

#[test]
fn a_part_both_sides_changed_is_a_conflict_without_a_pack() {
    run(merges::both_changed_part);
}

#[test]
fn the_identities_hold() {
    run(merges::identities);
}

#[test]
fn an_unsound_merge_is_a_conflict() {
    run(merges::soundness);
}

#[test]
fn edits_apart_merge_on_every_format() {
    run(merges::self_merges);
}
