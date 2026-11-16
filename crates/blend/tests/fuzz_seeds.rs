//! The fuzz properties (`k2_mpc_blend::fuzzing`) on seed files built here and on their
//! truncations and one-byte changes: a deterministic slice of what the fuzz target in `fuzz/`
//! explores.

mod support;

use k2_mpc_blend::fuzzing::properties;
use k2_mpc_blend::testgen::{gzip, zstd_frames, SceneBuilder};
use k2_mpc_blend::HeaderForm;
use support::{spec, studio, Studio};

/// Every `step`-th prefix of `seed`, and every `step`-th byte replaced by a few others.
fn mutations(seed: &[u8], step: usize) {
    mutations_of(seed, step, properties);
}

fn mutations_of(seed: &[u8], step: usize, f: fn(&[u8])) {
    f(seed);
    for n in (0..seed.len()).step_by(step) {
        f(&seed[..n]);
    }
    for i in (0..seed.len()).step_by(step) {
        for b in [0u8, 1, 0x7f, 0xff] {
            if seed[i] == b {
                continue;
            }
            let mut m = seed.to_vec();
            m[i] = b;
            f(&m);
        }
    }
}

#[test]
fn the_writer_keeps_its_properties_on_seeds_and_their_mutations() {
    use k2_mpc_blend::testgen::ObjectSpec;
    // a seed whose objects rotate by Euler angles, so every kind of edit can land
    let mut b = SceneBuilder::new(HeaderForm::Classic, 8, false, 405, false);
    b.scene("Scene", &["A", "B"], &[], None);
    for name in ["A", "B"] {
        b.object(
            name,
            &ObjectSpec {
                rotmode: 1,
                restrict: 1024,
                ..spec(None)
            },
        );
    }
    let seed = b.bytes();
    k2_mpc_blend::fuzzing::write(&seed);
    mutations_of(&seed, 41, k2_mpc_blend::fuzzing::write);
    mutations_of(&zstd_frames(&seed, 700), 97, k2_mpc_blend::fuzzing::write);
}

fn older_shape() -> Vec<u8> {
    let mut b = SceneBuilder::new(HeaderForm::Classic, 4, true, 273, true);
    b.library("lib", "//lib.blend");
    b.scene("Scene", &["Cube"], &[], None);
    b.collection("Set", &["Cube", "Loose"], &[]);
    b.object("Cube", &spec(Some("ME:Cube")));
    b.object("Loose", &spec(None));
    b.mesh("Cube", &[None], 2);
    b.material("M", [1.0, 1.0, 1.0], Some("lib"));
    b.bytes()
}

fn newer_form() -> Vec<u8> {
    let mut b = SceneBuilder::new(HeaderForm::Large, 8, false, 501, false);
    b.scene("Scene", &["A"], &["C"], Some("A"));
    b.collection("C", &["B"], &[]);
    b.object("A", &spec(Some("CA:Cam")));
    b.object("B", &spec(None));
    b.camera("Cam", 35.0);
    b.bytes()
}

#[test]
fn the_seeds_themselves_read_and_index() {
    for seed in [studio(&Studio::default()), older_shape(), newer_form()] {
        let draft = k2_mpc_blend::index_blend(&mut &seed[..], &support::limits())
            .expect("check: a seed indexes");
        assert!(!draft.nodes.is_empty());
    }
}

#[test]
fn a_modern_seed_and_its_mutations_keep_the_properties() {
    mutations(&studio(&Studio::default()), 2);
}

#[test]
fn an_older_seed_and_its_mutations_keep_the_properties() {
    mutations(&older_shape(), 2);
}

#[test]
fn a_newer_form_seed_and_its_mutations_keep_the_properties() {
    mutations(&newer_form(), 2);
}

#[test]
fn compressed_seeds_and_their_mutations_keep_the_properties() {
    let plain = studio(&Studio::default());
    mutations(&gzip(&plain), 5);
    mutations(&zstd_frames(&plain, 777), 5);
}

#[test]
fn the_triangulator_keeps_its_properties_on_seeds_and_their_mutations() {
    let seeds = k2_mpc_blend::fuzzing::triangulate_seeds();
    assert_eq!(seeds.len(), 10, "every shape seeded");
    for (name, seed) in &seeds {
        let (positions, faces) = k2_mpc_blend::fuzzing::triangulate_input(seed);
        assert!(
            !positions.is_empty() && !faces.is_empty(),
            "{name} reads back"
        );
        mutations_of(seed, 3, k2_mpc_blend::fuzzing::triangulate);
    }
}
