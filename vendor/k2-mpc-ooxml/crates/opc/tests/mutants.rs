//! Each planted bug must be caught, by the check that exists to catch it. Each row names the
//! leg it holds to, on its own, so a change to one leg can't hide behind another. Run with
//! `--features mutants`.
#![cfg(feature = "mutants")]

mod common;

use common::damage::refusals_hold;
use common::merges;
use common::{fixtures_hold, plans_hold};
use k2_mpc_opc::mutant::with;

type Leg = fn() -> Result<(), String>;

/// (mutant, the leg, the leg's name).
const ROWS: &[(&str, Leg, &str)] = &[
    // the zip layer and the package checks
    ("zip_no_crc", refusals_hold, "refusals"),
    ("zip_no_duplicates", refusals_hold, "refusals"),
    ("zip_no_tiling", refusals_hold, "refusals"),
    ("doctype_skip", refusals_hold, "refusals"),
    ("zip_recompress_all", fixtures_hold, "fixtures"),
    // the write as ranges of the file
    ("opc_plan_skips_gap", plans_hold, "plans"),
    // the merge: renaming, re-minting, rewriting, merging by key
    ("opc_no_rename", merges::two_images, "two images"),
    ("opc_no_rename", merges::two_slides, "two slides"),
    ("opc_no_remint", merges::two_images, "two images"),
    ("opc_no_remint", merges::two_sheets, "two sheets"),
    (
        "opc_no_remint",
        merges::package_relationships,
        "package rels",
    ),
    ("opc_no_ref_rewrite", merges::two_images, "two images"),
    ("opc_no_ref_rewrite", merges::two_slides, "two slides"),
    ("opc_types_ours", merges::content_types, "content types"),
    ("opc_rels_drop_theirs", merges::two_images, "two images"),
    // the soundness check
    ("opc_no_validate", merges::soundness, "soundness"),
    ("opc_validate_all", merges::soundness, "soundness"),
    ("opc_no_target_check", merges::soundness, "soundness"),
];

#[test]
fn the_legs_pass_without_a_mutant() {
    refusals_hold().expect("refusals");
    fixtures_hold().expect("fixtures");
    plans_hold().expect("plans");
    merges::named().expect("merge rows");
}

#[test]
fn each_mutant_is_caught_by_its_leg() {
    let mut missed = Vec::new();
    for &(m, leg, name) in ROWS {
        match with(m, leg) {
            Err(e) => println!("{m} / {name}: {}", e.lines().next().unwrap_or("")),
            Ok(()) => {
                println!("{m} / {name}: NOT CAUGHT");
                missed.push(format!("{m} / {name}"));
            }
        }
    }
    assert!(missed.is_empty(), "missed: {missed:?}");
    println!("{} rows, every mutant caught", ROWS.len());
}
