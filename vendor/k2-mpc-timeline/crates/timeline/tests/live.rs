//! The live OTIO pack on the kit's session (`k2_mpc_timeline::live`): the store and its times,
//! the view, the declines and bounded reads; the verbs and the watcher, what they refuse, time
//! order and minted keys; the write-back (untouched byte for byte, a move's gaps, our stamp, the
//! rate token, a time of day, a pairing that doesn't fit, a restart, a delete, the differential
//! oracle); outside saves (one that adds a clip before a live-edited one, keeping both); the
//! agent's playhead (each edit verb leaves it at its edit point, a person's is never moved) and
//! the view's time unit; each clip's start timecode (`tc`) in the view index, the snapshot and
//! the change frames, and the rate and start timecode in the snapshot; a drop-frame timeline's
//! timecodes labelled drop-frame, a non-drop one's as before, and the drop-frame entry read
//! defensively; each track's label and kind (`kind_label`, `kind`) in the view index, the
//! snapshot and the change frames (a rename moves neither; a move or an added track does);
//! which track is which after an outside save (by name and kind, then by order where a name
//! can't decide), a claim on a track kept through one. The rows are in
//! `live_support/rows.rs`, shared with the planted bugs (`tests/mutants.rs`).
//!
//! `public_corpus_live` reads the public corpus (`TIMELINE_PUBLIC_CORPUS`,
//! `scripts/fetch-corpus.sh`; it fails when that is unset; scripts/public-corpus.sh runs it): every
//! OTIO file opens live or is declined typed, and the ones that open pass the kit's byte-exact row
//! (`carrier_judge`).

mod live_support;

use std::sync::Arc;

use k2_mpc_base::session::test_support::{carrier_judge, CarrierVerdict};
use k2_mpc_base::session::Pack;
use k2_mpc_timeline::live::TimelineLive;
use live_support::rows;

macro_rules! row_tests {
    ($($name:ident => $row:path,)*) => {
        $(
            #[test]
            fn $name() {
                if let Err(e) = $row() {
                    panic!("row failed at its step: {e}");
                }
            }
        )*
    };
}

row_tests! {
    opens_live_with_every_field => rows::row_opens_live_with_every_field,
    view_index_is_the_lanes => rows::row_view_index_is_the_lanes,
    declines_are_typed => rows::row_declines_are_typed,
    reads_are_bounded_and_paged => rows::row_reads_are_bounded_and_paged,
    move_one_event => rows::row_move_one_event,
    refusals_are_typed => rows::row_refusals_are_typed,
    list_is_time_ordered => rows::row_list_is_time_ordered,
    mint_past_the_mark => rows::row_mint_past_the_mark,
    untouched_writes_back_byte_for_byte => rows::row_untouched_writes_back_byte_for_byte,
    move_rewrites_only_its_gaps => rows::row_move_rewrites_only_its_gaps,
    stamp_pairs_a_resave => rows::row_stamp_pairs_a_resave,
    copied_stamp_pairs_neither => rows::row_copied_stamp_pairs_neither,
    rate_token_kept => rows::row_rate_token_kept,
    time_of_day_round_trips => rows::row_time_of_day_round_trips,
    pairing_mismatch_refused_typed => rows::row_pairing_mismatch_refused_typed,
    save_adds_before_then_edit_lands_right => rows::row_save_adds_before_then_edit_lands_right,
    pack_save_adds_before_then_edit_lands_right => rows::row_pack_save_adds_before_then_edit_lands_right,
    save_after_kit_write_keeps_both => rows::row_save_after_kit_write_keeps_both,
    save_on_unwritten_changes_keeps_both => rows::row_save_on_unwritten_changes_keeps_both,
    restart_then_verb_writes_exactly => rows::row_restart_then_verb_writes_exactly,
    live_verbs_equal_model_edits => rows::row_live_verbs_equal_model_edits,
    delete_lifts_with_markers => rows::row_delete_lifts_with_markers,
    rate_grid_refused => rows::row_rate_grid_refused,
    hand_edit_writes_back => rows::row_hand_edit_writes_back,
    save_pairs_by_live_identity => rows::row_save_pairs_by_live_identity,
    duplicate_identity_never_swaps => rows::row_duplicate_identity_never_swaps,
    moved_duplicate_never_swaps => rows::row_moved_duplicate_never_swaps,
    read_back_refuses_an_unwritten_field => rows::row_read_back_refuses_an_unwritten_field,
    move_then_list_sees_new_order => rows::row_move_then_list_sees_new_order,
    info_length_equals_the_scan => rows::row_info_length_equals_the_scan,
    tracks_stack_last_on_top => rows::row_tracks_stack_last_on_top,
    track_enabled_and_muted_read_only => rows::row_track_enabled_and_muted_read_only,
    muted_lane_stays_in_the_view => rows::row_muted_lane_stays_in_the_view,
    end_is_the_latest_item_end => rows::row_end_is_the_latest_item_end,
    media_start_maps_source_exactly => rows::row_media_start_maps_source_exactly,
    retimed_is_its_own_field => rows::row_retimed_is_its_own_field,
    agent_playhead_follows_a_trim => rows::row_agent_playhead_follows_a_trim,
    agent_playhead_at_each_edit_point => rows::row_agent_playhead_at_each_edit_point,
    person_playhead_never_moved_by_a_verb => rows::row_person_playhead_never_moved_by_a_verb,
    view_time_unit_is_tick => rows::row_view_time_unit_is_tick,
    older_reader_ignores_time_unit => rows::row_older_reader_ignores_time_unit,
    each_clip_carries_its_start_tc => rows::row_each_clip_carries_its_start_tc,
    edit_frame_carries_the_new_tc => rows::row_edit_frame_carries_the_new_tc,
    start_tc_change_moves_every_tc => rows::row_start_tc_change_moves_every_tc,
    snapshot_carries_the_rate_and_start_tc => rows::row_snapshot_carries_the_rate_and_start_tc,
    drop_frame_labels_drop_frame => rows::row_drop_frame_labels_drop_frame,
    non_drop_is_unchanged => rows::row_non_drop_is_unchanged,
    drop_frame_entry_read_defensively => rows::row_drop_frame_entry_read_defensively,
    each_track_carries_its_label_and_kind => rows::row_each_track_carries_its_label_and_kind,
    snapshot_carries_track_label_and_kind => rows::row_snapshot_carries_track_label_and_kind,
    rename_keeps_track_label_and_kind => rows::row_rename_keeps_track_label_and_kind,
    track_label_shift_rides_the_frame => rows::row_track_label_shift_rides_the_frame,
    save_adding_a_bottom_track_keeps_every_key => rows::row_save_adding_a_bottom_track_keeps_every_key,
    save_rename_and_move_pair_by_name_first => rows::row_save_rename_and_move_pair_by_name_first,
    save_duplicate_names_fall_back_to_their_order => rows::row_save_duplicate_names_fall_back_to_their_order,
    save_pairs_a_name_within_its_kind => rows::row_save_pairs_a_name_within_its_kind,
    claim_on_a_track_stays_through_a_save => rows::row_claim_on_a_track_stays_through_a_save,
    hold_on_a_track_renamed_outside_ends_gone_outside_save => rows::row_hold_on_a_track_renamed_outside_ends_gone_outside_save,
    save_deleting_a_track_keeps_the_others => rows::row_save_deleting_a_track_keeps_the_others,
}

/// The public corpus (the folder `TIMELINE_PUBLIC_CORPUS` names; unset, it fails): each OTIO
/// file opens live or is declined typed (listed); the ones that open are byte exact through the kit
/// (`carrier_judge`), and `timeline info`'s length equals a scan of every item. FCPXML and
/// PRPROJ files have no live pack yet: listed, not opened.
#[test]
#[ignore = "the public corpus: scripts/public-corpus.sh runs it"]
fn public_corpus_live() {
    let dir = std::env::var_os("TIMELINE_PUBLIC_CORPUS")
        .map(std::path::PathBuf::from)
        .expect("TIMELINE_PUBLIC_CORPUS: the public corpus's folder (scripts/fetch-corpus.sh)");
    let manifest = include_str!("corpus/manifest.tsv");
    let pack = TimelineLive::default();
    let mut live: Vec<(String, Vec<u8>)> = Vec::new();
    let mut declined = Vec::new();
    let mut other = 0;
    for line in manifest
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let name = line.split('\t').next().unwrap();
        if !name.ends_with(".otio") {
            other += 1;
            continue;
        }
        let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        match pack.decode(&bytes) {
            Ok(_) => {
                rows::info_equals_scan_on(name, &bytes).unwrap_or_else(|e| panic!("{e}"));
                live.push((name.to_owned(), bytes))
            }
            Err(d) => {
                println!("corpus live: declined {name}: {}", d.code);
                declined.push(name.to_owned());
            }
        }
    }
    assert!(!live.is_empty(), "no public OTIO file opens live");
    let refs: Vec<(&str, &[u8])> = live
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    match carrier_judge(Arc::new(TimelineLive::default()), &refs) {
        Ok(CarrierVerdict::ByteExact { files }) => println!(
            "corpus live: {files} OTIO files byte exact through the kit, {} declined, {other} \
             FCPXML and PRPROJ files not live",
            declined.len()
        ),
        other => panic!("corpus live: {other:?}"),
    }
}
