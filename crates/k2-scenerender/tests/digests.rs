//! The determinism gate: a fixed set of generated scenes, rendered in
//! process, against SHA-256 digests checked in here. The same digests on
//! x86_64 Linux and on an aarch64 Mac are what K2 asked for before landing:
//! the render path calls no libm (sRGB is a table, a camera's tan is ours),
//! uses no fused multiply-add, and splits threads by fixed bands, so the bytes
//! can't depend on the CPU, the OS or the thread count.
//!
//! A change to any pixel changes a digest here: that is a renderer change
//! (bump the crate's version so caches don't serve the old images), then
//! update the table from this test's `[digest]` lines.

use k2_scenerender::camera::ViewSpec;
use k2_scenerender::proto::{ImageFormat, Request, View};
use k2_scenerender::render::render_bytes;
use k2_scenerender::sandbox::Caps;
use k2_scenerender::scene::{encode, InputLimits, Scene};
use k2_scenerender::sha256::{digest, hex};
use k2_scenerender::testgen;

/// Made on x86_64 Linux (k2n10-vin, Intel, job 71bc4b0f) and matched, all 30, on aarch64
/// macOS (k2n1-m4, Apple M4, job c4090b9e).
const WANT: &[(&str, &str)] = &[
    ("cube-webp-320/k2scene1", "c891d86464fe48e6c3b4e5aae359e3124474982f275171081455b3dd0332837d"),
    ("cube-webp-320/0", "5a8e7a89bef92a33281af63a775f1c297cd8dc99090f88b80d190b43eb91c9bc"),
    ("cube-png-320/k2scene1", "c891d86464fe48e6c3b4e5aae359e3124474982f275171081455b3dd0332837d"),
    ("cube-png-320/0", "9e5cf73e1442bc5f12234aea5831ffba5d4903b50a4cb4416613d9c0a4b158b9"),
    ("white-cube-1920/k2scene1", "4943831d7023a5004e64f2eab6bf0b6359a8e0ab21eeb173abe57b4196026010"),
    ("white-cube-1920/0", "1ea707379589263e015b49cce235ff280f8f7edf6dedc0ab222bc3f1d396aaf8"),
    ("sphere/k2scene1", "e37daa01f64f7d8d383a48c62d7825dcca8bd012f90cece165f0454828f3f724"),
    ("sphere/0", "63fe46a09ce6d2598fc2f9a2f7151e71d094784ba09f923e47f6b6037d0afc79"),
    ("sphere/1", "eaff0600290a32d1ebf1df3dc2ad3aba2f496e4b5c8be8173c617c1081b0f711"),
    ("grid-z-turntable/k2scene1", "488744bce54658e039e1e408b4f5762342fcc49e1de756dbbe829e5a30fbeca6"),
    ("grid-z-turntable/0", "9222edbecdac60bedfbb238b5a94d6b151b8e2d3f958a504b1e1f8e2144cfc43"),
    ("grid-z-turntable/1", "c415eba4329db5f97c853c37f352d80a469490b6016ec8cda50c09b7ae52ad4f"),
    ("grid-z-turntable/2", "8722765f106d8a7b7e2a1537c6ff4b79416de3760cba3e59b79f14cd0b26a60c"),
    ("grid-z-turntable/3", "2f55a04778b54e530e972d47524e3b6157401683131cc5fecfb4fb07e9103a22"),
    ("grid-z-turntable/4", "cc2a17c4842b6c2a4014e43693dc76d499aec84d21cd865903322f11643345b2"),
    ("grid-z-turntable/5", "e3c509e4027cbe7b303987cdd596c493d8ad74b1d2d9ae21c37820969b4fa555"),
    ("grid-z-turntable/6", "c9b0ec92c39d9e592a526f7bf82e1b4d1a4e9dd80ac65f355bde95c8d560a783"),
    ("grid-z-turntable/7", "e6a0b881f7ed8480d9fd42936b363a58b14a569d22c03e915216db190d52f84c"),
    ("mirrored/k2scene1", "5273ec44c510c8c019fe9119f4d60aa0a13913c9e17c902b9e19162858ce800a"),
    ("mirrored/0", "36fe57a5e6284cb50c07eb48230806cb4a0c6a819467e02370332e7527ad3226"),
    ("camera/k2scene1", "01d2b2cbae4b6915c7959e561d2f82fd3fd2e8dbb56d4e853e0b5e3b2992b7d0"),
    ("camera/0", "8681a56f803a2001ca15d5a4edc8e778499a13ccd311845ea1f48fed18a7ab4b"),
    ("camera/1", "d10b29b7b5cc6d1c84b4f7bb91a43404faed53d897bb07192fa080fca3b48360"),
    ("degenerate-collapsed/k2scene1", "ec7e6131aa9c80b9dd85320678fca7b8db042b92d65326dc11ead5f66a722cfa"),
    ("degenerate-collapsed/0", "c66e24cd2681b8b7d8f8edaa5e561864208157df8fd92e6414388751b068cf7f"),
    ("degenerate-camera-inside/k2scene1", "69518e6050c7c508f5cf4b3ccd1bcd22553d694174b544a3e74ec24655b7cdba"),
    ("degenerate-camera-inside/0", "cd2c4fe620258c1f7bfe956cb4c7d529e247ea357dc938b5271f5f473d83e519"),
    ("terrain-200k/k2scene1", "9fe86ae3af59abeafd74a17458ecea65357a7d40a54549eb605c534094a0e789"),
    ("terrain-200k/0", "3db9bdae1bd0fa53c3d77635bbc468c87af4caabc5168773f268c6451c363885"),
    ("terrain-200k/1", "2c4e9e223945e65df1ea816d18d68aa49577dd06f5f75198981a37ee117170be"),
];

fn caps() -> Caps {
    Caps {
        max_width: 1920,
        max_side: 4096,
        max_pixels: 16 << 20,
        max_ssaa: 4,
        max_threads: 4,
        max_views: 64,
        max_output_bytes: 256 << 20,
        input: InputLimits::default(),
    }
}

fn case(out: &mut Vec<(String, String)>, name: &str, s: &Scene, views: Vec<View>, format: ImageFormat, threads: u32) {
    let input = encode(s);
    out.push((format!("{name}/k2scene1"), hex(&digest(&input))));
    let mut req = Request::new(views);
    req.format = format;
    req.threads = threads;
    let (o, _) = render_bytes(&input, &req, &caps()).unwrap_or_else(|f| panic!("{name}: {} {}", f.code, f.detail));
    for (i, (_, img)) in o.views.iter().enumerate() {
        out.push((format!("{name}/{i}"), hex(&digest(img))));
    }
}

#[test]
fn the_digests_are_the_checked_in_ones() {
    let mut got = Vec::new();
    let auto = |w: u32| View::new(ViewSpec::Auto, w);
    case(&mut got, "cube-webp-320", &testgen::cube_scene(), vec![auto(320)], ImageFormat::Webp, 1);
    case(&mut got, "cube-png-320", &testgen::cube_scene(), vec![auto(320)], ImageFormat::Png, 2);
    case(&mut got, "white-cube-1920", &testgen::white_cube_scene(), vec![auto(1920)], ImageFormat::Webp, 4);
    case(&mut got, "sphere", &testgen::sphere_scene(), vec![auto(320), auto(1280)], ImageFormat::Webp, 3);
    let turntable: Vec<View> = (0..8).map(|k| View::new(ViewSpec::Turntable(k), 160)).collect();
    case(&mut got, "grid-z-turntable", &testgen::grid_scene(5, true), turntable, ImageFormat::Webp, 2);
    case(&mut got, "mirrored", &testgen::mirrored_scene(), vec![auto(400)], ImageFormat::Webp, 1);
    case(&mut got, "camera", &testgen::camera_scene(), vec![View::new(ViewSpec::Camera(0), 640), View { view: ViewSpec::Auto, objects: Some(vec![0]), width: 200, aspect: Some(1.0) }], ImageFormat::Webp, 4);
    for (name, s) in testgen::degenerate_scenes() {
        if name == "camera-inside" || name == "collapsed" {
            case(&mut got, &format!("degenerate-{name}"), &s, vec![View::new(if s.cameras.is_empty() { ViewSpec::Auto } else { ViewSpec::Camera(0) }, 120)], ImageFormat::Webp, 2);
        }
    }
    case(&mut got, "terrain-200k", &testgen::large_scene(200_000), vec![auto(640), View::new(ViewSpec::Turntable(6), 1920)], ImageFormat::Webp, 4);
    for (k, v) in &got {
        eprintln!("[digest] (\"{k}\", \"{v}\"),");
    }
    eprintln!("[digest] {} digests on {} {}", got.len(), std::env::consts::ARCH, std::env::consts::OS);
    let want: Vec<(String, String)> = WANT.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let differ: Vec<&str> = got.iter().zip(&want).filter(|(g, w)| g != w).map(|(g, _)| g.0.as_str()).collect();
    assert!(differ.is_empty() && got.len() == want.len(), "{} of {} differ ({} checked in): {differ:?}", differ.len(), got.len(), want.len());
}
