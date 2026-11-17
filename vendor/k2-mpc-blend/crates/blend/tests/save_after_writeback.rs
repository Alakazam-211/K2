//! An app's save of the session's own written-back version (the app reopened what the kit wrote
//! and saved it): kept both with no version noted, nothing when it is the same bytes, merged
//! when the host noted the version the app opened; and the pack's check of a merged doc, which
//! keeps both for a merge the format forbids. The rows are in `merged_view_support`, shared
//! with the planted bugs (`tests/mutants.rs`).

mod live_support;
mod merged_view_support;

fn pass(row: fn() -> Result<(), String>) {
    if let Err(e) = row() {
        panic!("{e}");
    }
}

#[test]
fn a_resave_with_no_version_noted_keeps_both() {
    pass(merged_view_support::row_resave_unpinned_keeps_both);
}

#[test]
fn the_same_bytes_saved_again_are_nothing() {
    pass(merged_view_support::row_resave_identical_unchanged);
}

#[test]
fn a_resave_of_the_noted_version_merges() {
    pass(merged_view_support::row_pinned_resave_merges);
}

#[test]
fn a_merge_giving_two_objects_one_name_keeps_both() {
    pass(merged_view_support::row_pinned_duplicate_name_keeps_both);
}

#[test]
fn a_merged_doc_is_checked_by_the_packs_rules() {
    pass(merged_view_support::row_merged_view_rules);
}
