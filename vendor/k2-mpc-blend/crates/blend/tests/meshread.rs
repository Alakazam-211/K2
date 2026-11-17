//! The mesh read: the K2SCENE1 writer against the strict checker and a golden written by hand,
//! its limits and refusals; the triangulator; the mesh layouts; the objects of the scene drawn
//! (the scene's choice, sharing, hidden objects, world matrices, what is left out, modifiers),
//! the read's determinism and its verb; the objects' keys, the cameras, the heap limit, the
//! vertex limits after compaction, the meshes' content hashes, and the hidden-by rule against
//! its shared vectors. The rows are in `meshread_support/mod.rs`, shared with the planted bugs
//! in `tests/mutants.rs`.

mod meshread_support;

use meshread_support as rows;

fn run(r: rows::Row) {
    if let Err(e) = r {
        panic!("{e}");
    }
}

#[test]
fn k2scene_round_trips_through_the_checker() {
    run(rows::row_round_trip());
}

#[test]
fn k2scene_is_the_golden() {
    run(rows::row_golden());
}

#[test]
fn k2scene_limits_are_typed_at_each_cap() {
    run(rows::row_limits());
}

#[test]
fn k2scene_writer_refuses_what_a_renderer_refuses() {
    run(rows::row_writer_refuses());
}

#[test]
fn k2scene_checker_refuses_each_break() {
    run(rows::row_checker_refuses());
}

#[test]
fn triangles_keep_the_winding() {
    run(rows::row_winding());
}

#[test]
fn concave_polygons_are_ear_clipped() {
    run(rows::row_concave());
}

#[test]
fn every_polygon_gives_n_minus_2_triangles() {
    run(rows::row_counts());
}

#[test]
fn each_mesh_layout_reads() {
    run(rows::row_layouts());
}

#[test]
fn mesh_layouts_agree() {
    run(rows::row_layouts_agree());
}

#[test]
fn mesh_counts_are_checked_against_their_blocks() {
    run(rows::row_counts_checked());
}

#[test]
fn loose_vertices_are_dropped() {
    run(rows::row_loose_vertices());
}

#[test]
fn mesh_breaks_are_typed_skips() {
    run(rows::row_mesh_skips());
}

#[test]
fn the_scene_drawn_is_the_current_one() {
    run(rows::row_scene_choice());
}

#[test]
fn shared_meshes_are_written_once() {
    run(rows::row_instancing());
}

#[test]
fn hidden_objects_stay_in_marked() {
    run(rows::row_hidden());
}

#[test]
fn world_matrices_are_composed() {
    run(rows::row_world());
}

#[test]
fn world_matrices_equal_an_independent_composition() {
    run(rows::row_world_composed());
}

#[test]
fn objects_left_out_are_named_with_reasons() {
    run(rows::row_left_out());
}

#[test]
fn modifiers_and_driven_transforms_are_marked() {
    run(rows::row_modifiers_and_driven());
}

#[test]
fn the_read_is_deterministic() {
    run(rows::row_determinism());
}

#[test]
fn scene_meshes_answers_with_the_report() {
    run(rows::row_verb());
}

#[test]
fn objects_are_keyed_minted_or_paired() {
    run(rows::row_keys());
}

#[test]
fn cameras_are_placed_and_framed() {
    run(rows::row_cameras());
}

#[test]
fn the_heap_is_checked_before_the_meshes() {
    run(rows::row_heap_limit());
}

#[test]
fn vertex_limits_count_what_is_written() {
    run(rows::row_limits_after_compaction());
}

#[test]
fn meshes_carry_their_content_hash() {
    run(rows::row_mesh_hash());
}

#[test]
fn hidden_by_meets_its_shared_vectors() {
    run(rows::row_hidden_by_vectors());
}
