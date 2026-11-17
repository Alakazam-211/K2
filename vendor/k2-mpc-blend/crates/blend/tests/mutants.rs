//! The mutants (`--features mutants`): each plants one bug in the writer, the merge or the pack,
//! and the leg written for it must pass without it and fail under it. A mutant no leg catches
//! fails the run, so these rows are shown to be able to fail.
#![cfg(feature = "mutants")]

mod builds_support;
mod check_edit_support;
mod driven_support;
mod edit_hook_support;
mod live_support;
mod merged_view_support;
mod meshread_support;
mod newer_support;
mod reuse_support;
mod scenes_support;
mod support;

use k2_mpc_base::session::{BlobMerge, BlobPack};
use k2_mpc_blend::merge::{merge3, ConflictWhy, Merged, Outcome};
use k2_mpc_blend::mutant;
use k2_mpc_blend::testgen::{ObjectSpec, SceneBuilder};
use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{
    merge_known_base, read_blend, read_scene, write, BlendPack, Change, Edit, ReadLimits,
    WriteError,
};
use live_support::rows;

type Leg = fn() -> Result<(), String>;

/// A (mesh data A) and B; `a` renames A; `shift` moves everything in memory (an app's save).
fn scene(a: &str, a_loc: [f32; 3], shift: u64) -> Vec<u8> {
    let mut b = SceneBuilder::modern();
    b.shift = shift;
    b.scene("Scene", &[a, "B"], &[], None);
    b.object(
        a,
        &ObjectSpec {
            data: Some("ME:A".into()),
            loc: a_loc,
            rotmode: 1,
            ..ObjectSpec::default()
        },
    );
    b.object(
        "B",
        &ObjectSpec {
            rotmode: 1,
            ..ObjectSpec::default()
        },
    );
    b.mesh("A", &[], 3);
    b.bytes()
}

fn base() -> Vec<u8> {
    scene("A", [0.0; 3], 0)
}

fn l() -> ReadLimits {
    ReadLimits::default()
}

fn agent_loc(v: f32) -> Vec<u8> {
    write(
        &base(),
        &[Edit::new("@ob:A", Change::Location([v; 3]))],
        &l(),
    )
    .expect("the agent's write")
    .bytes
}

fn merged(ours: &[u8], theirs: &[u8]) -> Result<Merged, String> {
    match merge3(&base(), ours, theirs, &l()) {
        Outcome::Merged(m) => Ok(m),
        other => Err(format!("{other:?}").chars().take(200).collect()),
    }
}

fn loc_of(bytes: &[u8], key: &str) -> Result<String, String> {
    let (_, raw) = decompress(bytes, &l()).map_err(|e| e.to_string())?;
    let f = read_blend(&mut &raw[..], &l()).map_err(|e| e.to_string())?;
    let m = read_scene(&f).map_err(|e| e.to_string())?;
    let id = m.by_key(key).ok_or("no such object")?;
    id.fields
        .iter()
        .find(|(n, _)| n == "loc")
        .map(|(_, v)| v.clone())
        .ok_or_else(|| "no loc".into())
}

/// A file newer than the pinned versions: its values stay numbers, nothing is named or hidden.
fn leg_names_newer() -> Result<(), String> {
    let mut b = SceneBuilder::new(k2_mpc_blend::HeaderForm::Classic, 8, false, 504, false);
    b.scene("Scene", &["A"], &[], None);
    b.object(
        "A",
        &ObjectSpec {
            data: Some("ME:A".into()),
            rotmode: 1,
            ..ObjectSpec::default()
        },
    );
    b.mesh("A", &[], 3);
    let (_, raw) = decompress(&b.bytes(), &l()).map_err(|e| e.to_string())?;
    let f = read_blend(&mut &raw[..], &l()).map_err(|e| e.to_string())?;
    let m = read_scene(&f).map_err(|e| e.to_string())?;
    let a = m.by_key("@ob:A").ok_or("no object")?;
    if !a.named.is_empty() || a.hidden.is_some() {
        return Err(format!("named past the pinned versions: {:?}", a.named));
    }
    Ok(())
}

/// Both moved A: the person's location stays, the agent's is held.
fn leg_held() -> Result<(), String> {
    let m = merged(&agent_loc(1.0), &scene("A", [9.0; 3], 0x40_0000))?;
    if loc_of(&m.bytes, "@ob:A")? != "9,9,9" {
        return Err("the person's location isn't kept".into());
    }
    match m.conflicts.as_slice() {
        [c] if c.why == ConflictWhy::BothChanged => Ok(()),
        other => Err(format!("held {other:?}")),
    }
}

/// The person renamed A: the agent's move lands on the renamed object.
fn leg_carried() -> Result<(), String> {
    let m = merged(&agent_loc(2.0), &scene("Hero", [0.0; 3], 0x40_0000))?;
    if !m.conflicts.is_empty() {
        return Err(format!("held {:?}", m.conflicts));
    }
    if loc_of(&m.bytes, "@ob:Hero")? != "2,2,2" {
        return Err("the agent's move didn't land".into());
    }
    Ok(())
}

/// An agent version that is a whole other save: a whole-file conflict.
fn leg_more_than_fields() -> Result<(), String> {
    let ours = scene("A", [1.0; 3], 0x10_0000);
    match merge3(&base(), &ours, &scene("A", [0.0; 3], 0x40_0000), &l()) {
        Outcome::Conflict(_) => Ok(()),
        other => Err(format!("{other:?}").chars().take(100).collect()),
    }
}

/// The pack's guard: with no base known to be the app's, a save keeps both, whether the
/// base is ours (a lease held) or an earlier kit write with the next one waiting.
fn leg_stale() -> Result<(), String> {
    let pack = BlendPack::default();
    let theirs = scene("A", [0.0; 3], 0x40_0000);
    let earlier = agent_loc(1.0);
    let next = write(
        &earlier,
        &[Edit::new("@ob:B", Change::Location([5.0; 3]))],
        &l(),
    )
    .map_err(|e| e.to_string())?
    .bytes;
    for (base, ours) in [(base(), base()), (earlier, next)] {
        match pack.merge3_bytes(&base, &ours, &theirs) {
            BlobMerge::Conflict => {}
            other => return Err(format!("{other:?}").chars().take(100).collect()),
        }
    }
    Ok(())
}

/// Both moved A: the pack keeps both, a whole-file conflict, not the agent's side in memory.
fn leg_clash_whole_file() -> Result<(), String> {
    let theirs = scene("A", [9.0; 3], 0x40_0000);
    match merge_known_base(&base(), &agent_loc(1.0), &theirs, &l()) {
        BlobMerge::Conflict => Ok(()),
        other => Err(format!("{other:?}").chars().take(100).collect()),
    }
}

/// The writer puts a location where the schema says.
fn leg_offset() -> Result<(), String> {
    let w = write(
        &base(),
        &[Edit::new("@ob:A", Change::Location([4.0; 3]))],
        &l(),
    )
    .map_err(|e| e.to_string())?;
    if loc_of(&w.bytes, "@ob:A")? != "4,4,4" {
        return Err("the location didn't read back".into());
    }
    Ok(())
}

/// A name another object has is refused.
fn leg_unique() -> Result<(), String> {
    match write(
        &base(),
        &[Edit::new("@ob:A", Change::Name("B".into()))],
        &l(),
    ) {
        Err(WriteError::Name { .. }) => Ok(()),
        Err(e) => Err(e.to_string()),
        Ok(_) => Err("a taken name was written".into()),
    }
}

/// The writer's newest version in [`in_gap`]'s rows: 5.01, so 5.02 and 5.03 are read but not
/// written.
const GAP: u16 = 501;

/// `row` with the writer's range set back to [`GAP`] (`mutant::in_gap`): no release read today
/// sits past the ones written, so the rows of that state run in this gap.
fn in_gap(row: Leg) -> Result<(), String> {
    mutant::in_gap(GAP, row)
}

/// The mesh read draws 5.02 and 5.03 in the gap: reading doesn't stop at the writer's range.
fn gap_mesh_read() -> Result<(), String> {
    in_gap(newer_support::row_mesh_read_newer)
}

/// 5.02 and 5.03 named in the gap: the names don't stop at the writer's range.
fn gap_names() -> Result<(), String> {
    in_gap(newer_support::row_names_newer)
}

/// A doc of 5.02 and 5.03 in the gap is read-only, every edit refused by its code.
fn gap_live_read_only() -> Result<(), String> {
    in_gap(newer_support::row_live_read_only)
}

const MUTANTS: &[(&str, Leg)] = &[
    ("merge_agent_wins", leg_held),
    ("merge_no_carried_keys", leg_carried),
    ("merge_trusts_ours", leg_more_than_fields),
    ("pack_no_stale_guard", leg_stale),
    ("pack_merges_clash", leg_clash_whole_file),
    ("write_wrong_offset", leg_offset),
    ("write_names_not_unique", leg_unique),
    // a change to a channel an action animates written (`driven_support/mod.rs`)
    ("DrivenChannelWritten", driven_support::row_driven_z_refused),
    // a flip of a visibility bit an action animates written; a transform written under
    // constraints with no warning (`driven_support/mod.rs`)
    ("DrivenBitWritten", driven_support::row_driven_bit_refused),
    (
        "ConstrainedUnwarned",
        driven_support::row_constrained_written_with_warning,
    ),
    // the live pack's plants, each caught by its row (`live_support/rows.rs`)
    (
        "RenameKeepsNid",
        rows::row_rename_keeps_key_through_app_save,
    ),
    ("DrivenPlaced", rows::row_driven_and_unwritable_refused),
    // a change to a driven transform taken by the live rules (`live::check_edit`)
    (
        "CheckEditPassesDriven",
        check_edit_support::row_check_edit_refuses_driven_xform,
    ),
    // the kit's edit check never asked (`BlendLive::checks_edits` false): a person's edit of a
    // driven channel taken (`newer_support/mod.rs`)
    (
        "EditCheckOff",
        newer_support::row_person_edit_of_driven_channel,
    ),
    (
        "LiveAnimatedBitFlipped",
        rows::row_live_visibility_refused_on_animated_bits,
    ),
    (
        "QuatWrittenAsEuler",
        rows::row_driven_and_unwritable_refused,
    ),
    ("RenameNotUnique", rows::row_rename_needs_break_links),
    (
        "PairByPosition",
        rows::row_pack_save_adds_before_then_edit_patches_right_object,
    ),
    ("PairByName", rows::row_rename_chain_writes_each_name),
    ("FloatRounded", rows::row_xform_round_trips_bits),
    (
        "EncodeReserialises",
        rows::row_untouched_writes_back_byte_for_byte,
    ),
    ("PairingUnchecked", rows::row_pairing_mismatch_refused_typed),
    // the placements' plants, each row's control step passing under it first
    ("ParentInverseIgnored", rows::row_parent_inverse_composes),
    ("EulerOrderReversed", rows::row_euler_order_words),
    ("PlacementDroppedOnAdd", rows::row_placement_arrives_on_add),
    // the collection tree: the view carries no parent; the store names the wrong collection
    ("CollParentDropped", rows::row_collection_tree_is_the_files),
    ("CollParentWrong", rows::row_collection_tree_is_the_files),
    (
        "UnitSystemZeroAsMetric",
        rows::row_space_system_zero_unknown,
    ),
    ("PlacementByPosition", rows::row_move_keeps_placements),
    // a newer file's values named as the pinned versions name them
    ("NamesPastPinned", leg_names_newer),
    // files of 5.02 and 5.03 written: the writer's range left at 5.01, a version past the ones
    // read written (`newer_support/mod.rs`)
    ("WritesStopAt501", newer_support::row_writer_newer),
    ("WritesPastRead", newer_support::row_writer_newer),
    // the reads past the writer's range, in a gap: read only up to the writer's versions, read
    // past 5.03, named only up to the writer's, a read-only write-back refused without its
    // code, a read-only doc's verbs applied
    ("ReadStopsAtWritten", gap_mesh_read),
    ("ReadPastRange", newer_support::row_mesh_read_newer),
    ("NamesStopAtWritten", gap_names),
    ("ReadOnlyCodeDropped", gap_live_read_only),
    ("ReadOnlyVerbsWrite", gap_live_read_only),
    // a link placeholder's kind looked up by its block's code (`ID`), not its stored name's
    (
        "PlaceholderKindByBlockCode",
        newer_support::row_placeholder_kind,
    ),
    // a kept build changed on disk: its sha256 isn't the manifest's
    ("FixtureByteChanged", builds_support::row_kept_builds),
    // a live refusal printed as text only, with no registered entry
    ("RefusalAsText", rows::row_refusal_is_its_registered_entry),
    // the scene read's cameras: a camera named by its data, not its object; two names swapped
    ("CameraNameDropped", support::row_cameras_named),
    ("CameraNamesSwapped", support::row_cameras_named),
    // the mesh read's plants, each caught by its row (`meshread_support/mod.rs`)
    ("HeaderCountOff", meshread_support::row_round_trip),
    ("WindingReversed", meshread_support::row_winding),
    ("ConcaveFanned", meshread_support::row_concave),
    ("TriangleDropped", meshread_support::row_counts),
    ("CountsTrusted", meshread_support::row_counts_checked),
    ("LooseVertexEmitted", meshread_support::row_loose_vertices),
    ("StoredMatrixUsed", meshread_support::row_world),
    ("InstancesDuplicated", meshread_support::row_instancing),
    ("HiddenDropped", meshread_support::row_hidden),
    ("SceneChoiceFirst", meshread_support::row_scene_choice),
    ("KeyFromPosition", meshread_support::row_keys),
    ("CameraKeyFromCameraIndex", meshread_support::row_cameras),
    ("SensorFitIgnored", meshread_support::row_cameras),
    ("OrthoCameraKept", meshread_support::row_cameras),
    ("LeftOutUnlisted", meshread_support::row_cameras),
    ("HeapUnchecked", meshread_support::row_heap_limit),
    (
        "LimitBeforeCompaction",
        meshread_support::row_limits_after_compaction,
    ),
    ("MeshHashOmitsCounts", meshread_support::row_mesh_hash),
    ("HiddenByInnermost", meshread_support::row_hidden_by_vectors),
    (
        "HiddenByCollectionFirst",
        meshread_support::row_hidden_by_vectors,
    ),
    // the live view's derived values from the carrier's scene
    (
        "MeshHashPerObject",
        live_support::view_rows::row_view_mesh_hash,
    ),
    (
        "HiddenByStaleOnCollection",
        live_support::view_rows::row_view_hidden_by,
    ),
    (
        "CurrentSceneFirst",
        live_support::view_rows::row_view_current_scene,
    ),
    // a merged doc: every one refused (the kit's default), every one taken
    (
        "MergedViewRefusesAll",
        merged_view_support::row_pinned_resave_merges,
    ),
    (
        "MergedViewUnchecked",
        merged_view_support::row_pinned_duplicate_name_keeps_both,
    ),
    // the reuse: the carrier's transform kept, the id from the data's length only, the file
    // decoded on every call, the data copied, every key listed as changed
    ("ReuseStaleTransform", reuse_support::row_reuse_parity),
    ("CarrierIdStale", reuse_support::row_reuse_carrier_id),
    ("ReuseRereads", reuse_support::row_reuse_no_decode),
    ("ReuseCopiesData", reuse_support::row_reuse_shared_data),
    ("ReuseChangedAll", reuse_support::row_reuse_key_lists),
    // the zstd decoder left at its own window bound, not the reader's limit
    (
        "ZstdWindowAtDecoderDefault",
        support::zstd::row_window_over_decoder_default,
    ),
    // the streaming index left at 64 GiB, whatever its limits say
    ("IndexDecodeUnbounded", support::zstd::row_index_cap),
    // the reader's own default raw cap left at 64 GiB
    ("ReadDefaultUnbounded", support::zstd::row_read_default_cap),
    // the pack's whole-file default left at 64 GiB
    (
        "WholeFileDefaultUnbounded",
        support::row_whole_file_default_bounded,
    ),
    // a generated scene's master collection written without the embedded flag
    ("EmbeddedFlagDropped", scenes_support::row_embedded_flag),
];

/// Every mutant, or with `BLEND_MUTANT_ONLY` set, only the one it names (one plant per run).
#[test]
fn every_mutant_is_caught_by_its_leg() {
    let only = std::env::var("BLEND_MUTANT_ONLY").ok();
    let chosen: Vec<&(&str, Leg)> = MUTANTS
        .iter()
        .filter(|(n, _)| only.as_deref().is_none_or(|o| o == *n))
        .collect();
    if let Some(o) = &only {
        assert_eq!(chosen.len(), 1, "BLEND_MUTANT_ONLY names one mutant: {o}");
        println!("plant: {o}");
    }
    let mut missed = Vec::new();
    for (name, leg) in chosen.iter().copied() {
        if let Err(e) = leg() {
            panic!("{name}'s leg fails without the mutant: {e}");
        }
        match mutant::with(name, *leg) {
            Err(e) => println!("{name}: caught ({e})"),
            Ok(()) => missed.push(*name),
        }
    }
    assert!(missed.is_empty(), "mutants no leg caught: {missed:?}");
    println!("mutants: {} planted, {} caught", chosen.len(), chosen.len());
}
