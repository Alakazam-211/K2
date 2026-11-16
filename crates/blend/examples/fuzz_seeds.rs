//! Writes seed files for the fuzz targets into a folder: scenes built here (`testgen`) in every
//! header form, pointer size, byte order and shape, plain, gzip and zstd. Test tooling only.
//!
//!     cargo run --example fuzz_seeds --features testgen,fuzzing -- DIR [TRIANGULATE_DIR]
//!
//! With a second folder, the triangulator target's seeds go there (`fuzzing::triangulate_seeds`).

use k2_mpc_blend::testgen::{gzip, zstd_frames, ObjectSpec, SceneBuilder};
use k2_mpc_blend::HeaderForm;

fn scene(form: HeaderForm, ps: u8, big: bool, version: u16, legacy: bool) -> Vec<u8> {
    let mut b = SceneBuilder::new(form, ps, big, version, legacy);
    let with = |data: Option<&str>| ObjectSpec {
        data: data.map(str::to_owned),
        ..ObjectSpec::default()
    };
    b.ui();
    b.library("lib", "//lib.blend");
    b.scene("Scene", &["Cam"], &["Set"], Some("Cam"));
    b.collection("Set", &["Cube", "Child"], &[]);
    b.object("Cam", &with(Some("CA:Lens")));
    b.object(
        "Cube",
        &ObjectSpec {
            materials: vec![Some("Red".into()), None],
            ..with(Some("ME:Cube"))
        },
    );
    b.object(
        "Child",
        &ObjectSpec {
            parent: Some("Cube".into()),
            restrict: 1,
            ..with(Some("LA:Lamp"))
        },
    );
    b.camera("Lens", 50.0);
    b.light("Lamp", 10.0);
    b.mesh("Cube", &[Some("Blue".into())], 3);
    b.material("Red", [1.0, 0.0, 0.0], None);
    b.material("Blue", [0.0, 0.0, 1.0], Some("lib"));
    b.world("World");
    b.bytes()
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: fuzz_seeds DIR");
    std::fs::create_dir_all(&dir).expect("the seed folder");
    let shapes = [
        (
            "classic8le",
            scene(HeaderForm::Classic, 8, false, 403, false),
        ),
        ("classic4be", scene(HeaderForm::Classic, 4, true, 279, true)),
        (
            "classic8be-legacy",
            scene(HeaderForm::Classic, 8, true, 273, true),
        ),
        ("large", scene(HeaderForm::Large, 8, false, 501, false)),
    ];
    let mut n = 0;
    for (name, plain) in &shapes {
        for (how, bytes) in [
            ("plain", plain.clone()),
            ("gzip", gzip(plain)),
            ("zstd", zstd_frames(plain, 1500)),
        ] {
            std::fs::write(format!("{dir}/{name}-{how}.blend"), bytes).expect("a seed");
            n += 1;
        }
    }
    println!("fuzz seeds: {n} files in {dir}");
    if let Some(tri) = std::env::args().nth(2) {
        std::fs::create_dir_all(&tri).expect("the triangulator's seed folder");
        let seeds = k2_mpc_blend::fuzzing::triangulate_seeds();
        for (name, bytes) in &seeds {
            std::fs::write(format!("{tri}/{name}.bin"), bytes).expect("a seed");
        }
        println!("triangulator seeds: {} files in {tri}", seeds.len());
    }
}
