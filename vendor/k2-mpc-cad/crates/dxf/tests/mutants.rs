//! The planted bugs (`--features mutants`): each plants one bug in the tokenizer, the model, the
//! index, a read or an edit verb, and the row written for it must pass without it and fail under it. A plant
//! no row catches fails the run, so every row is shown to be able to fail. One plant at a time,
//! each named in the output with what stopped it.
#![cfg(feature = "mutants")]
// the rows are the other test files, included by path: each brings its own copy of `support`
#![allow(clippy::duplicate_mod)]

#[path = "draw.rs"]
mod draw;
#[path = "edit.rs"]
mod edit;
#[path = "edit_session.rs"]
mod edit_session;
#[path = "generic.rs"]
mod generic;
#[path = "index.rs"]
mod index;
#[path = "merge.rs"]
mod merge;
#[path = "probe.rs"]
mod probe;
#[path = "references.rs"]
mod references;
#[path = "tokenize.rs"]
mod tokenize;
#[path = "verbs.rs"]
mod verbs;
#[path = "writer.rs"]
mod writer;

use k2_mpc_dxf::mutant;

type Row = fn();

const PLANTS: &[(&str, Row)] = &[
    // the tokenizer: what it must carry exactly, and what it must refuse
    ("LineEndingNormalised", tokenize::tokenize_reemit_identical),
    ("CodePaddingDropped", tokenize::tokenize_reemit_identical),
    ("CommentDropped", tokenize::tokenize_reemit_identical),
    ("TailDropped", tokenize::bom_and_tail_are_carried),
    ("LenientTokenizer", tokenize::not_pairs_refused),
    // the model and the index
    ("HandleNotNormalised", index::handles_normalised),
    ("HmaxFromSeed", index::hmax_from_every_object),
    ("KeyFromPosition", index::no_handles_minted),
    ("FollowersAsNodes", index::followers_in_parent),
    ("CountOnLayerNode", index::attrs_are_own_facts),
    ("PointInName", index::move_is_not_rename),
    ("LayerInContentHash", index::layer_change_is_a_field),
    ("HeaderCopiedWhole", index::lastsavedby_not_indexed),
    ("BulgeIgnored", index::bbox_exact_kinds),
    ("OcsIgnored", index::bbox_exact_kinds),
    ("DefaultNotFilled", index::compact_attrs_read_back_the_same),
    // the generic reads
    ("BoxNotPolygon", generic::inside_and_crossing),
    ("LineDiagonalFlipped", generic::inside_and_crossing),
    ("AttribValueAsText", generic::attrib_find_typed),
    ("AttribsOutOfOrder", generic::block_show_attribs),
    ("ByLayerUnresolved", generic::ltype_resolved),
    ("NearIgnoresDistance", generic::text_near),
    // the structure probe
    (
        "ProbeValueLeaks",
        probe::probe_names_and_counts_never_values,
    ),
    ("PairingCountsRepeats", probe::probe_set_pairs_by_structure),
    // block references and leaders, resolved
    (
        "DynamicAsAnonymous",
        references::dynamic_blocks_by_real_name,
    ),
    ("PlacedUnrotated", references::placed_axis),
    (
        "PlacedCountsTemplates",
        references::placed_geometry_leaves_out_attribute_definitions,
    ),
    ("LeaderPointsReversed", references::leaders_arrow_first),
    (
        "MleaderFirstLineOnly",
        references::multileader_every_line_and_plain_text,
    ),
    // what we write
    ("SubclassDropped", writer::subclass_markers_in_order),
    // the merge
    ("MergeTakesTheirs", merge::merge_disjoint_entities),
    ("FieldsAsWhole", merge::merge_same_entity_different_fields),
    ("LastWriterWins", merge::merge_same_field_conflicts),
    ("FrameGuessed", merge::merge_frame_never_guessed),
    ("AddDropped", merge::merge_adds_and_deletes),
    ("OursStyleLeaks", merge::merge_keeps_theirs_style),
    // the reads
    ("UnitsDropped", verbs::units_on_geometry_reads),
    ("ExtentsFromHeader", verbs::extents_header_vs_computed),
    ("NestedCountsDirectOnly", verbs::block_counts_nested),
    ("CheckSkipsBlocks", verbs::check_findings),
    ("WindowAsCrossing", verbs::entity_find_window_and_crossing),
    // the drawing-model read
    ("DrawByLayerUnresolved", draw::draw_style_resolved),
    ("ByBlockResolvedInBlock", draw::draw_block_scope_insert),
    ("SortentsIgnored", draw::draw_order_sortents),
    (
        "HandleOrderWithoutTable",
        draw::draw_order_file_without_table,
    ),
    ("PageOverBudget", draw::draw_pages_bounded),
    ("StaleCursorServed", draw::draw_stale_cursor_refused),
    ("BulgeDropped", draw::draw_polylines),
    ("FrameVertexDrawn", draw::draw_polylines),
    ("InsertExpanded", draw::draw_inserts_are_references),
    ("BasePointIgnored", draw::draw_inserts_are_references),
    ("AlignAnchorWrong", draw::draw_text_placed),
    ("PercentCodeLeaks", draw::draw_text_placed),
    ("MtextRawCodes", draw::draw_mtext_runs),
    ("SplineTolLoose", draw::draw_splines_within_tolerance),
    ("HatchUnclipped", draw::draw_hatch_pattern_clipped),
    ("HatchOverCap", draw::draw_hatch_dense),
    ("SolidCornerOrder", draw::draw_other_kinds),
    ("WhyOutsideSet", draw::draw_whys_closed_set),
    ("MtextBoxIgnoresAttach", draw::draw_mtext_box_attachment),
    (
        "MtextBoxIgnoresAttach",
        index::index_text_box_follows_alignment,
    ),
    ("MtextBoxIgnoresRuns", draw::draw_mtext_box_wraps_and_grows),
    ("EmbeddedCodesAsMain", draw::draw_mtext_embedded_object),
    (
        "AttribEmbeddedCodesAsMain",
        draw::draw_attdef_embedded_object,
    ),
    ("AttribTypeAsGeneration", draw::draw_attdef_embedded_object),
    ("EmbeddedCodesAsMain", index::index_mtext_embedded_object),
    ("TextBoxExact", draw::draw_text_boxes_are_approx),
    ("TextBoxExact", index::index_text_boxes_are_approx),
    ("DrawFloatsUnrounded", draw::draw_floats_platform_identical),
    ("DrawNoiseKept", draw::draw_noise_snapped_to_zero),
    // the edit verbs: what they write, and what they refuse that they shouldn't
    ("SpanCoversCodeLine", edit::untouched_units_exact),
    ("EditStyleLeaks", edit::style_kept),
    ("FloatDriftOnMove", edit::move_decimal_exact),
    ("VectorMovedAsPoint", edit::point_table),
    ("AttribsLeftBehind", edit::point_table),
    ("CodepageIgnored", edit::attrib_value_written),
    ("TrueColourLeft", edit::set_props),
    ("PairOrderIgnored", edit::set_props),
    ("HmaxOwnedOnly", edit::adds_mint_above_hmax),
    ("HandseedLeft", edit::adds_mint_above_hmax),
    ("FollowersLeft", edit::delete_cleans_known_refs),
    ("SortentsLeft", edit::delete_cleans_known_refs),
    ("SelfCheckSkipped", edit::self_check_catches),
    ("DynblockAttribRefused", edit::honest_edits_land),
    ("LockedReadAsFrozen", edit::honest_edits_land),
    ("ProxyWritten", edit::refusals_named),
    ("MergeFieldSplitDiffers", edit_session::merge_and_base_agree),
    // agents change a drawing through the verbs only: the whole-file write refused, the verbs
    // admitted
    (
        "AgentWritesAllowed",
        edit_session::agent_writes_refused_verbs_land,
    ),
    (
        "AgentWritesRefused",
        edit_session::agent_writes_refused_verbs_land,
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
