//! The planted bugs (`--features mutants`): each plants one bug in the live pack or the
//! structure index (`index_support`), or, where a pack's verb can't reach, in the test host's
//! side of a call (`HostMirrorsToPeople` and `HostMovesOnRefusal`,
//! `live_support::host_after_call`) or in the watched turn's recorder (`TurnRefusalDropped`,
//! `turn_support`: the refusal frame isn't kept, which the golden comparison must catch), or in the
//! XML readers' attribute lookup (`AttrByLocalName`, `xml_support`); the row
//! written for it must pass without it and fail under it. A plant no row catches fails the run, so these rows are shown
//! to be able to fail.
#![cfg(feature = "mutants")]

mod index_support;
mod live_support;
mod turn_support;
mod xml_support;

use k2_mpc_timeline::mutant;
use live_support::rows;

type Row = fn() -> Result<(), String>;

const PLANTS: &[(&str, Row)] = &[
    ("TickRounded", rows::row_declines_are_typed),
    ("ReadUnbounded", rows::row_reads_are_bounded_and_paged),
    ("TicksAsDouble", rows::row_time_of_day_round_trips),
    ("OverlapAllowed", rows::row_refusals_are_typed),
    ("OffGridSnapped", rows::row_refusals_are_typed),
    ("MintBelowMark", rows::row_mint_past_the_mark),
    ("ListOrderTrusted", rows::row_list_is_time_ordered),
    (
        "EncodeReserialises",
        rows::row_untouched_writes_back_byte_for_byte,
    ),
    ("GapNotRebuilt", rows::row_move_rewrites_only_its_gaps),
    ("KeyNotInMetadata", rows::row_stamp_pairs_a_resave),
    ("StampFirstWins", rows::row_copied_stamp_pairs_neither),
    ("RateTokenRewritten", rows::row_rate_token_kept),
    ("RateGridIgnored", rows::row_rate_grid_refused),
    (
        "ReadBackSkipped",
        rows::row_read_back_refuses_an_unwritten_field,
    ),
    ("DiffStaleUnit", rows::row_stamp_pairs_a_resave),
    ("IndexKeptStale", rows::row_move_then_list_sees_new_order),
    ("IndexKeptStale", move_then_list_with_the_kits_checks_off),
    ("IdentityFirstWins", rows::row_moved_duplicate_never_swaps),
    (
        "IdentityFirstWins",
        rows::row_duplicate_identity_never_swaps,
    ),
    (
        "PairByPosition",
        rows::row_pack_save_adds_before_then_edit_lands_right,
    ),
    ("TrimOutAtStart", rows::row_agent_playhead_follows_a_trim),
    (
        "HostMirrorsToPeople",
        rows::row_person_playhead_never_moved_by_a_verb,
    ),
    ("TimeUnitDropped", rows::row_view_time_unit_is_tick),
    ("SchemaExtraKey", rows::row_older_reader_ignores_time_unit),
    (
        "HostMovesOnRefusal",
        rows::row_agent_playhead_at_each_edit_point,
    ),
    (
        "TcWithoutStartTimecode",
        rows::row_each_clip_carries_its_start_tc,
    ),
    ("TcAtWholeRate", rows::row_each_clip_carries_its_start_tc),
    ("TcFromFileBytes", rows::row_edit_frame_carries_the_new_tc),
    ("TcNotRederived", edit_frame_with_the_kits_checks_off),
    (
        "TcStartChangeIgnored",
        start_tc_change_with_the_kits_checks_off,
    ),
    (
        "DropFrameAtAnyRate",
        index_support::row_df_at_25_fps_is_non_drop,
    ),
    ("DropFrameIgnored", rows::row_drop_frame_labels_drop_frame),
    (
        "DropFrameAtAnyRate",
        rows::row_drop_frame_entry_read_defensively,
    ),
    (
        "KindLabelDropped",
        rows::row_each_track_carries_its_label_and_kind,
    ),
    (
        "AudioKindAsVideo",
        rows::row_each_track_carries_its_label_and_kind,
    ),
    (
        "TracksByLabel",
        rows::row_save_adding_a_bottom_track_keeps_every_key,
    ),
    (
        "TrackNameWithoutKind",
        rows::row_save_pairs_a_name_within_its_kind,
    ),
    (turn_support::PLANT, turn_support::row_turn_is_the_golden),
    (
        "AttrByLocalName",
        xml_support::row_attributes_by_plain_name_only,
    ),
];

/// The same row with the kit's checks of the kept state off on this thread: the row's own
/// read must see a stale index.
fn move_then_list_with_the_kits_checks_off() -> Result<(), String> {
    let was = k2_mpc_base::differential::set_view_checks(false);
    let r = rows::row_move_then_list_sees_new_order();
    k2_mpc_base::differential::set_view_checks(was);
    r
}

/// The frame row with the kit's checks of the kept view index off on this thread: a pack's
/// derive by change that leaves what a change touched underived is the kit's to catch first (it
/// compares the kept index with a whole build after every change, in test builds only); off, as
/// in a host build, the row's own read of the change frame must catch it.
fn edit_frame_with_the_kits_checks_off() -> Result<(), String> {
    let was = k2_mpc_base::differential::set_view_checks(false);
    let r = rows::row_edit_frame_carries_the_new_tc();
    k2_mpc_base::differential::set_view_checks(was);
    r
}

/// The start timecode row with the kit's checks of the kept view index off on this thread, as
/// [`edit_frame_with_the_kits_checks_off`].
fn start_tc_change_with_the_kits_checks_off() -> Result<(), String> {
    let was = k2_mpc_base::differential::set_view_checks(false);
    let r = rows::row_start_tc_change_moves_every_tc();
    k2_mpc_base::differential::set_view_checks(was);
    r
}

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
            Err(e) => println!("{name}: caught ({e})"),
            Ok(()) => missed.push(*name),
        }
    }
    assert!(missed.is_empty(), "plants no row caught: {missed:?}");
    println!("mutants: {} planted, {} caught", PLANTS.len(), PLANTS.len());
}
