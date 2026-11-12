//! Seed scenes for the fuzz targets (scripts/check-fuzz.sh), generated in
//! code (k2_scenerender::testgen), so the fuzzer starts from inputs the
//! decoder takes. `cargo run -p k2-scenerender --example fuzz_seeds -- <dir>`.

use k2_scenerender::scene::encode;
use k2_scenerender::testgen;

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "fuzz-seeds".into());
    std::fs::create_dir_all(&dir).expect("seed folder");
    let mut scenes = vec![
        ("cube", testgen::cube_scene()),
        ("white-cube", testgen::white_cube_scene()),
        ("sphere", { let mut s = testgen::sphere_scene(); s.meshes[0] = testgen::sphere(8, 4); s }),
        ("grid", testgen::grid_scene(3, true)),
        ("mirrored", testgen::mirrored_scene()),
        ("camera", testgen::camera_scene()),
        ("terrain", testgen::large_scene(200)),
    ];
    let mut modded = testgen::grid_scene(2, false);
    modded.objects[0].modifiers = vec!["SUBSURF".into(), "MIRROR".into()];
    modded.objects[0].modifiers_not_applied = true;
    scenes.push(("modifiers", modded));
    for (name, s) in testgen::degenerate_scenes() {
        scenes.push((name, s));
    }
    for (name, s) in scenes {
        let path = std::path::Path::new(&dir).join(format!("{name}.k2scene"));
        std::fs::write(&path, encode(&s)).expect("write a seed");
    }
}
