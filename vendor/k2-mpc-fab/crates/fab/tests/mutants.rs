//! The planted bugs (`--features mutants`): each plants one bug in a reader or the index, and
//! the row written for it must pass without it and fail under it. A plant no row catches fails
//! the run, so these rows are shown to be able to fail.
#![cfg(feature = "mutants")]

mod support;

use k2_mpc_fab::mutant;
use support::*;

type Row = fn() -> Result<(), String>;

const PLANTS: &[(&str, Row)] = &[
    // a missing format statement guessed (2.4, as legacy readers did)
    ("FormatGuessed", row_missing_format_refused),
    ("UnitGuessed", row_missing_unit_refused),
    // deprecated constructs read outside what the deprecated section allows
    ("IncrementalAccepted", row_incremental_refused),
    ("SingleQuadrantGuessed", row_single_quadrant_arc_refused),
    // rules the current revision states
    ("OpenContourAccepted", row_open_contour_refused),
    ("AnyAperturePlots", row_plot_with_macro_refused),
    ("ApertureRedefinable", row_aperture_twice_refused),
    // an unknown command skipped silently, the file reported as conforming
    ("UnknownCommandAccepted", row_unknown_command_reported),
    ("ExpansionUnbounded", row_expansion_bounded),
    ("BoundsSkipped", row_index_bounds),
    ("DrillUnitGuessed", row_drill_missing_unit_refused),
    ("HitOutsideDrillMode", row_drill_hit_needs_drill_mode),
    // the common-export drill reading: guessing implied decimals, accepting a construct the
    // document doesn't list, and a common read reported as conforming
    (
        "ImpliedDecimalsGuessed",
        row_common_implied_decimals_refused,
    ),
    (
        "UndocumentedConstructAccepted",
        row_common_undocumented_refused,
    ),
    (
        "UndocumentedConstructAccepted",
        row_common_m17_only_before_the_end,
    ),
    (
        "CommonReportedConforming",
        row_common_reported_not_conforming,
    ),
    // keys by number, which a re-export reissues: a definition's key names another one
    ("KeyByNumber", saves::row_reuse_save_keeps_each_definition),
];

/// A plant's row under the plant: `Err` with what caught it, a row's error or a check's panic.
fn under(name: &'static str, row: Row) -> Result<(), String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mutant::with(name, row))) {
        Ok(r) => r,
        Err(p) => Err(p
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .map(|m| {
                format!(
                    "a check stopped it: {}",
                    m.chars().take(300).collect::<String>()
                )
            })
            .unwrap_or_else(|| "a check stopped it".to_owned())),
    }
}

#[test]
fn every_plant_is_caught_by_its_row() {
    let mut missed = Vec::new();
    for (name, row) in PLANTS {
        if let Err(e) = row() {
            panic!("{name}'s row fails without the plant: {e}");
        }
        match under(name, *row) {
            Err(e) => println!("plant {name}: caught ({e})"),
            Ok(()) => missed.push(*name),
        }
    }
    assert!(missed.is_empty(), "plants no row caught: {missed:?}");
    println!("mutants: {} planted, {} caught", PLANTS.len(), PLANTS.len());
}

/// The reuse row's plant alone in its run (`cargo test --features mutants --test mutants
/// key_by_number_alone`): the row passes without it, fails under it, and says so.
#[test]
fn key_by_number_alone() {
    let row: Row = saves::row_reuse_save_keeps_each_definition;
    if let Err(e) = row() {
        panic!("the reuse row fails without the plant: {e}");
    }
    match under("KeyByNumber", row) {
        Err(e) => println!("one plant: KeyByNumber; rows run: 1; caught ({e})"),
        Ok(()) => panic!("KeyByNumber: the reuse row passed under it"),
    }
}
