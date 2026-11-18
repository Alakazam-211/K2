//! The planted bugs (`--features mutants`): each plants one bug in a check, a bit read, the
//! decompression, the container or the test builder, and the row written for it must pass
//! without it and fail under it. A plant no row catches fails the run. One plant at a time,
//! each named in the output with what stopped it.
#![cfg(feature = "mutants")]
// the rows are the other test files, included by path: each brings its own copy of `support`
#![allow(clippy::duplicate_mod)]

#[path = "bits.rs"]
mod bits;
#[path = "container.rs"]
mod container;
#[path = "crc.rs"]
mod crc;
#[path = "drawing.rs"]
mod drawing;
#[path = "entities.rs"]
mod entities;
#[path = "header.rs"]
mod header;
#[path = "heap.rs"]
mod heap;
#[path = "leaders.rs"]
mod leaders;
#[path = "lz77.rs"]
mod lz77;
#[path = "objects.rs"]
mod objects;
#[path = "polylines.rs"]
mod polylines;
#[path = "tables.rs"]
mod tables;

use k2_mpc_dwg::mutant;

type Row = fn();

const PLANTS: &[(&str, Row)] = &[
    // the checks
    ("Crc16SeedIgnored", crc::crc16_vectors),
    ("Crc32NoFinalXor", crc::crc32_vectors),
    ("ChecksumHalvesSwapped", crc::page_checksum_vectors),
    // the bit-coded values
    ("BsCodesSwapped", bits::fixed_and_coded_values),
    ("DdHalvesSwapped", bits::default_relative_doubles),
    ("McSignLost", bits::modular_numbers),
    ("HandleNotRelative", bits::handle_references),
    ("OtOffsetMissing", bits::colours_texts_and_types),
    ("ReadPastEndZeroFills", bits::past_the_end_refused),
    // the decompression
    ("MatchOffsetOffByOne", lz77::matches_and_literals),
    ("MatchBeforeStartAllowed", lz77::streams_refused),
    // the container
    ("HeaderCrcSkipped", container::header_and_its_copy),
    ("GapsNotAdvanced", container::page_map_with_gaps),
    (
        "SectionPagesOffByOne",
        container::sections_over_several_pages,
    ),
    ("ChecksumsSkipped", container::page_checks),
    ("EncryptionIgnored", container::encrypted_refused),
    ("VersionByPrefix", container::versions),
    ("AppInfoForeign", container::app_info_is_ours_alone),
    // the objects and classes
    ("MapDeltasNotReset", objects::object_map_blocks),
    ("ObjectMapCrcSkipped", objects::object_map_checked),
    ("VersionLayoutIgnored", objects::object_heads_by_version),
    ("ObjectCrcSkipped", objects::object_crc_checked),
    ("ClassStringsInline", objects::classes_from_a_string_stream),
    ("ClassCountOffByOne", objects::classes_inline),
    ("ClassCrcSkipped", objects::classes_checked),
    // the bounds
    ("BombBoundAtEnd", heap::bomb_within_a_small_heap),
    ("PageSizeUnbounded", heap::declared_size_bounded),
    ("TuLengthUnchecked", heap::text_length_bounded),
    ("PageCountTrusted", heap::section_page_count_bounded),
    ("SectionSizeTrusted", heap::section_size_bounded),
    ("MapEntriesUnbounded", heap::object_map_entries_bounded),
    // the entities and tables
    ("LinksBitRead", entities::common_data_by_version),
    ("LtypeBeforeLayer", entities::common_data_by_version),
    ("BllAsThreeB", entities::a_graphic_skipped),
    ("BookColourHasRgb", entities::a_book_colour),
    ("LwDdFromFirst", entities::light_polylines),
    ("MTextAnnotativeInverted", entities::texts),
    ("ImageRectAsPolygon", entities::images_and_wipeouts),
    ("MultilineEmbeddedSkipped", entities::multi_line_attributes),
    ("MTextColumnHeightRaw", entities::mtext_column_heights),
    ("InsertOwnedIgnored", entities::references_with_attributes),
    ("DataEndUnchecked", entities::data_end_checked),
    (
        "ViewportFrozenUncounted",
        entities::hatches_dimensions_viewports,
    ),
    ("SplineWeightsSkipped", entities::splines_and_rays),
    ("SplineScenarioStored", entities::splines_and_rays),
    // the multileaders
    (
        "MleaderLandingFromDirection",
        leaders::multileaders_with_text,
    ),
    ("MleaderAtFromNormal", leaders::multileaders_with_text),
    ("MleaderArrowFromContext", leaders::multileaders_with_text),
    ("MleaderLineHandlesSkipped", leaders::multileaders_with_text),
    (
        "MleaderBoundsUncounted",
        leaders::multileader_lines_and_points_bounded,
    ),
    // the old polylines
    (
        "PolylineOwnedIgnored",
        polylines::polylines_2d_and_their_vertices,
    ),
    (
        "VertexWidthSignKept",
        polylines::polylines_2d_and_their_vertices,
    ),
    ("FrameVertexKept", drawing::polyline_geometry),
    // the leaders
    ("LeaderArrowFromHook", leaders::leaders_read),
    ("LeaderHandlesSwapped", leaders::leaders_read),
    ("DstyleCodesSwapped", drawing::leader_style_overrides),
    ("BlockOwnedUncounted", tables::block_records),
    ("XrefOwnedCountRead", tables::xref_block_records),
    ("LeftOutUncounted", drawing::left_out_entries_counted),
    ("EntryXrefIndexAlways", tables::layers),
    ("LayoutCountRaw", tables::layouts),
    ("LtypeTextBitSwapped", tables::linetypes_and_styles),
    ("DbColorNamesSwapped", tables::book_colours),
    ("StyleBitsSwapped", tables::linetypes_and_styles),
    ("DimScaleAsArrow", tables::dimension_styles),
    ("DimColoursAsShorts", tables::dimension_styles),
    ("LwCountsTrusted", heap::polyline_count_bounded),
    ("HatchCountsTrusted", heap::hatch_count_bounded),
    // the drawing model
    ("RadiansKept", drawing::geometry_for_the_model),
    ("RepeatedNamesKept", drawing::repeated_block_names_numbered),
    ("BlockRepTagIgnored", drawing::dynamic_block_named),
    ("HeaderTailUnread", header::header_variables_2004),
    ("HeaderMenuNameSkipped", header::header_variables_2018),
    ("CurrentLayerUnread", header::header_variables_2018),
    ("AnnotativeEedIgnored", drawing::block_annotative_mark),
    ("AttdefsUnread", drawing::attdef_infos),
    // the whole handle stream, the 2007 entry short
    ("EntryStateAsTwoBits", tables::entry_states),
    ("EntryDependentBitIgnored", tables::entry_state_dependent),
    (
        "LayerVisualStyleSkipped",
        tables::layer_without_visual_style_refused,
    ),
    ("TrailingHandlesSkipped", tables::dimension_styles),
    (
        "EntityTrailingHandlesSkipped",
        entities::hatches_dimensions_viewports,
    ),
];

/// A plant's row under the plant: `Err` with what stopped it.
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
        // without the plant the row passes
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
