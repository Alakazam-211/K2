//! The live BLEND pack on the kit's session (`k2_mpc_blend::live`): the store and its floats,
//! the verb and the watcher, the byte-exact row, the splice (a placement, an app's save that adds
//! an object, a rename chain, a restart, the differential oracle), outside saves keeping both,
//! what is refused (driven transforms, visibility on an object with an animated bit, a rename
//! without `--break-links`, a pairing that doesn't fit), the declines, and bounded reads. The rows are in `live_support/rows.rs`, shared with
//! the planted bugs (`tests/mutants.rs`).
//!
//! `public_corpus_live` reads the public corpus (`BLEND_PUBLIC_CORPUS`, `scripts/fetch-corpus.sh`;
//! it fails when that is unset): every file opens live or is declined typed, the ones that open
//! pass the kit's byte-exact row (`carrier_judge`), and every listed object's `xform` holds its
//! file's floats bit for bit.

mod live_support;

use std::sync::Arc;

use k2_mpc_base::session::test_support::{carrier_judge, CarrierVerdict};
use k2_mpc_base::session::Pack;
use k2_mpc_blend::live::{model, BlendLive, LiveLimits, OBJ_PROPS};
use live_support::rows;

macro_rules! row_tests {
    ($($name:ident => $row:path,)*) => {
        $(
            #[test]
            fn $name() {
                // one line naming the row and its step, so a log shows where it failed
                if let Err(e) = $row() {
                    panic!("row failed at its step: {}", e.replace('\n', " | "));
                }
            }
        )*
    };
}

row_tests! {
    opens_live_with_every_field => rows::row_opens_live_with_every_field,
    view_index_is_the_outliner => rows::row_view_index_is_the_outliner,
    place_one_event => rows::row_place_one_event,
    untouched_writes_back_byte_for_byte => rows::row_untouched_writes_back_byte_for_byte,
    place_patches_only_its_fields => rows::row_place_patches_only_its_fields,
    pack_save_adds_before_then_edit_patches_right_object => rows::row_pack_save_adds_before_then_edit_patches_right_object,
    save_after_kit_write_keeps_both => rows::row_save_after_kit_write_keeps_both,
    save_on_unwritten_changes_keeps_both => rows::row_save_on_unwritten_changes_keeps_both,
    xform_round_trips_bits => rows::row_xform_round_trips_bits,
    rename_keeps_key_through_app_save => rows::row_rename_keeps_key_through_app_save,
    driven_and_unwritable_refused => rows::row_driven_and_unwritable_refused,
    live_visibility_refused_on_animated_bits => rows::row_live_visibility_refused_on_animated_bits,
    rename_needs_break_links => rows::row_rename_needs_break_links,
    rename_chain_writes_each_name => rows::row_rename_chain_writes_each_name,
    restart_then_verb_writes_exactly => rows::row_restart_then_verb_writes_exactly,
    live_verbs_equal_file_verbs => rows::row_live_verbs_equal_file_verbs,
    pairing_mismatch_refused_typed => rows::row_pairing_mismatch_refused_typed,
    declines_are_typed => rows::row_declines_are_typed,
    reads_are_bounded_and_paged => rows::row_reads_are_bounded_and_paged,
    view_places_every_object => rows::row_view_places_every_object,
    parent_inverse_composes => rows::row_parent_inverse_composes,
    euler_order_words => rows::row_euler_order_words,
    placement_arrives_on_add => rows::row_placement_arrives_on_add,
    place_verb_moves_its_placement => rows::row_place_verb_moves_its_placement,
    move_keeps_placements => rows::row_move_keeps_placements,
    space_metric_scale => rows::row_space_metric_scale,
    space_system_zero_unknown => rows::row_space_system_zero_unknown,
    space_mixed_systems_unknown => rows::row_space_mixed_systems_unknown,
    driven_first_place_delta_is_approx => rows::row_driven_first_place_delta_is_approx,
    collection_tree_is_the_files => rows::row_collection_tree_is_the_files,
    collection_move_carries_parent_after => rows::row_collection_move_carries_parent_after,
    view_mesh_hash_is_the_mesh_read_s => live_support::view_rows::row_view_mesh_hash,
    view_hidden_by_follows_the_collections => live_support::view_rows::row_view_hidden_by,
    view_current_scene_is_the_one_drawn => live_support::view_rows::row_view_current_scene,
}

/// Through the kit: an app's save that adds an object first, then a verb patches the right
/// object. Needs a kit that makes an outside save the carrier, with its pairing, before anything
/// renders the doc that took it (kit v0.21.0 does): a kit that settles the save against the
/// previous carrier and pairing fails this row, as this pack refuses that splice. The pack's
/// side of this alone is `pack_save_adds_before_then_edit_patches_right_object` above.
#[test]
fn save_adds_before_then_edit_patches_right_object() {
    rows::row_save_adds_before_then_edit_patches_right_object().unwrap();
}

/// The public corpus (the folder `BLEND_PUBLIC_CORPUS` names; unset, it fails): each file opens
/// live or is declined typed; the ones that open are byte exact through the kit (`carrier_judge`),
/// and their objects' `xform` are their files' floats bit for bit.
#[test]
#[ignore = "the public corpus: scripts/public-corpus.sh runs it"]
fn public_corpus_live() {
    let dir = std::env::var_os("BLEND_PUBLIC_CORPUS")
        .map(std::path::PathBuf::from)
        .expect("BLEND_PUBLIC_CORPUS: the public corpus's folder (scripts/fetch-corpus.sh)");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("the corpus folder")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".blend"))
        .collect();
    names.sort();
    assert!(names.len() >= 7, "the corpus holds {} files", names.len());
    let pack = BlendLive::default();
    let mut live: Vec<(String, Vec<u8>)> = Vec::new();
    let mut bits_checked = 0usize;
    for n in &names {
        let bytes = std::fs::read(dir.join(n)).expect("a corpus file");
        match pack.decode(&bytes) {
            Err(d) => {
                println!("corpus live: {n}: declined {} ({})", d.code, d.message);
                assert!(
                    [
                        "live_version",
                        "live_too_large",
                        "live_ambiguous",
                        "live_value"
                    ]
                    .contains(&d.code),
                    "{n}: an untyped decline {}",
                    d.code
                );
            }
            Ok(c) => {
                let scene = model::read(&bytes, &LiveLimits::default()).expect("reads");
                let (_, ok) = scene.minted_keys();
                for (o, k) in scene.objs.iter().zip(&ok) {
                    let Some(x) = &o.xform else { continue };
                    let text = live_support::field(&c.roots, OBJ_PROPS, k.as_str(), "xform")
                        .expect("an xform");
                    let back: Vec<u32> = text
                        .split(',')
                        .map(|v| v.parse::<f32>().expect("a float").to_bits())
                        .collect();
                    let want: Vec<u32> = x.iter().map(|v| v.to_bits()).collect();
                    assert_eq!(back, want, "{n}: {k:?}: {text}");
                    bits_checked += 1;
                }
                let edits: Vec<&str> = scene.objs.iter().map(|o| o.edit.as_str()).collect();
                println!(
                    "corpus live: {n}: opens: {} collections, {} objects, {} entries, edit 1/t/a/0 = {}/{}/{}/{}",
                    scene.colls.len(),
                    scene.objs.len(),
                    scene.entries(),
                    edits.iter().filter(|e| **e == "1").count(),
                    edits.iter().filter(|e| **e == "t").count(),
                    edits.iter().filter(|e| **e == "a").count(),
                    edits.iter().filter(|e| **e == "0").count(),
                );
                live.push((n.clone(), bytes));
            }
        }
    }
    assert!(!live.is_empty(), "no corpus file opens live");
    let refs: Vec<(&str, &[u8])> = live
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    match carrier_judge(Arc::new(BlendLive::default()), &refs) {
        Ok(CarrierVerdict::ByteExact { files }) => println!(
            "corpus live: carrier_judge ByteExact over {files} public files; xform bits checked on {bits_checked} objects"
        ),
        other => panic!("corpus live: carrier_judge: {other:?}"),
    }
}
