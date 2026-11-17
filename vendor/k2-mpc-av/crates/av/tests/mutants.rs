//! The planted bugs (`--features mutants`): each plants one bug in the set's reading or the
//! configuration, and the row written for it must pass without it and fail under it.
#![cfg(feature = "mutants")]
// the rows are the other test files, included by path: each brings its own copy of `support`
#![allow(clippy::duplicate_mod)]

#[path = "config.rs"]
mod config;
#[path = "set.rs"]
mod set;

use k2_mpc_cad_model::mutant;

type Row = fn();

const PLANTS: &[(&str, Row)] = &[
    ("DevicePerSheet", set::devices_across_sheets),
    ("CrossingIgnored", set::pulls_from_crossings),
    ("PullsAllCables", set::pulls_from_crossings),
    ("CablesOnlyPulls", set::cables_with_ends_and_labels),
    ("JunctionBeforeDevice", set::cables_with_ends_and_labels),
    ("FlagBeforeDevice", set::cables_end_at_flags),
    ("LabelShapeIgnored", set::cables_with_ends_and_labels),
    ("LeaderLabelIgnored", set::cables_labelled_by_leaders),
    ("MleaderFirstLineOnly", set::cables_labelled_by_leaders),
    ("PlacedUnrotated", set::cables_drawn_as_blocks),
    ("SnapIgnored", set::cable_findings),
    ("FlagByOrder", set::flags_paired),
    ("DynamicAsAnonymous", set::flags_paired),
    ("OneKindByOrder", set::flags_of_one_kind),
    ("FirstRevision", set::latest_revision),
    ("XrefByStem", set::title_block_by_reference),
    ("ReferenceByXrefOnly", set::reference_drawings),
    (
        "SheetTagFirstOnly",
        set::sheet_numbers_by_any_configured_tag,
    ),
    (
        "OwnTitleInsideBlocks",
        set::title_rows_from_the_referenced_drawing,
    ),
    ("EndsToInsertionOnly", set::runs_end_at_device_symbols),
    ("FlagLabelFromKind", set::flags_as_labelled_ends),
    ("LayoutAnyName", set::sheet_numbers_from_layout_or_file),
    ("UnknownKeyAccepted", config::config_reads_and_refuses),
];

fn under(name: &'static str, row: Row) -> Result<(), String> {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mutant::with(name, row)));
    match r {
        Ok(()) => Ok(()),
        Err(p) => Err(p
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .map(|m| m.lines().next().unwrap_or("").chars().take(200).collect())
            .unwrap_or_else(|| "a check stopped it".to_owned())),
    }
}

#[test]
fn every_plant_is_caught_by_its_row() {
    let mut missed = Vec::new();
    for (name, row) in PLANTS {
        row();
        match under(name, *row) {
            Ok(()) => missed.push(*name),
            Err(why) => println!("plant {name}: caught: {why}"),
        }
    }
    println!(
        "plants: {} of {} caught",
        PLANTS.len() - missed.len(),
        PLANTS.len()
    );
    assert!(missed.is_empty(), "plants no row caught: {missed:?}");
}
