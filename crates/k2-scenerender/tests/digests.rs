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

use k2_scenerender::camera::{Framing, ViewSpec};
use k2_scenerender::proto::{Ao, Clay, ImageFormat, Request, View};
use k2_scenerender::render::render_bytes;
use k2_scenerender::sandbox::Caps;
use k2_scenerender::scene::{encode, InputLimits, Scene};
use k2_scenerender::sha256::{digest, hex};
use k2_scenerender::testgen;

/// Made on x86_64 Linux (k2n10-vin, Intel, job 71bc4b0f) and matched, all 30, on aarch64
/// macOS (k2n1-m4, Apple M4, job c4090b9e). The 27 transparent ones: made on k2n10-vin
/// (job 9919a84e), the 30 above unchanged there. The 11 content-framing ones: made on
/// k2-arch (x86_64, job d8bddba8). The 13 clay ones: made on k2-arch (job 20f420f1). All 81
/// matched on aarch64 macOS (k2n1-m4, job ffe2988a). The 15 ambient occlusion ones: made on
/// k2n10-vin (job ac55097d), all 96 matched on aarch64 macOS (k2n1-m4, job e76bf3a5). The 15
/// object id ones (stills and id images): made on k2n10-vin (job feb4c66f), the 96 above unchanged
/// there.
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
    // Transparent (RGBA, coverage alpha).
    ("cube-webp-320-alpha/k2scene1", "c891d86464fe48e6c3b4e5aae359e3124474982f275171081455b3dd0332837d"),
    ("cube-webp-320-alpha/0", "a04366f927a5eed7fc48d8e7a51a2a7fc1dcd8c475b10f058722cce93a3c64a4"),
    ("cube-png-320-alpha/k2scene1", "c891d86464fe48e6c3b4e5aae359e3124474982f275171081455b3dd0332837d"),
    ("cube-png-320-alpha/0", "edf1b99d8183550ebcdf1b246e9882ed067e1f056a5655575be881c0753058ee"),
    ("sphere-alpha/k2scene1", "e37daa01f64f7d8d383a48c62d7825dcca8bd012f90cece165f0454828f3f724"),
    ("sphere-alpha/0", "61f71a3817527e5beae22e1776e78d965518d24d18ca43a25f7ff0e7a81a0112"),
    ("sphere-alpha/1", "b4e092768f1768f072f05c7e4064dd0b6ec1f1ade044a6947a33a020f10bb9bf"),
    ("grid-z-turntable-alpha/k2scene1", "488744bce54658e039e1e408b4f5762342fcc49e1de756dbbe829e5a30fbeca6"),
    ("grid-z-turntable-alpha/0", "4f6cb6d30c9919962875f1170c7b2c5ed3a499a276710a3f0f44e8a0a0a2e242"),
    ("grid-z-turntable-alpha/1", "204e4c91397f02b3bae784f658825179f91dd33502bbdcc3006eabb3fa8f0e25"),
    ("grid-z-turntable-alpha/2", "2ec2cf94b1e3338f3bdecac405508f17f91664947a710c530ba49b573ed8df8d"),
    ("grid-z-turntable-alpha/3", "50af17a95b9636223ce27de9613ee74e7ac6d6e94be325e39360a1ee36526226"),
    ("grid-z-turntable-alpha/4", "ee351752d2cba09a3a6758d1ea292590bb1e0f5976eb9420264edf2bd62bd849"),
    ("grid-z-turntable-alpha/5", "a34192fdc94add2a929ba871fd7b8f25e38fbd84d9d09dd6009c664de45f0d23"),
    ("grid-z-turntable-alpha/6", "29a0275876003521cdea2772fdaaebf086de6d44b1f93cc62f711a531da6d63f"),
    ("grid-z-turntable-alpha/7", "a891f0b7ff55cf35536ed79f5c4391d768a9e98e60b6b6f9ac28fa15dfded656"),
    ("mirrored-alpha/k2scene1", "5273ec44c510c8c019fe9119f4d60aa0a13913c9e17c902b9e19162858ce800a"),
    ("mirrored-alpha/0", "c6257341b18a7e1cb70bb3fd2f7e436ecbde072cbb54d333e7329733f958b73b"),
    ("camera-alpha/k2scene1", "01d2b2cbae4b6915c7959e561d2f82fd3fd2e8dbb56d4e853e0b5e3b2992b7d0"),
    ("camera-alpha/0", "c5994980f7bdab9e3a5665b5b4abae47140db8099432b7fcee46bcc624fd4f21"),
    ("camera-alpha/1", "44cccab5007974c2e26a9a16160e63fd039e601db942c5659bc86bb26e2b0f70"),
    ("degenerate-collapsed-alpha/k2scene1", "ec7e6131aa9c80b9dd85320678fca7b8db042b92d65326dc11ead5f66a722cfa"),
    ("degenerate-collapsed-alpha/0", "a9cae8308b95c674e9e7df997b26a53eabe2223169521d0770718e0a40341c5d"),
    ("degenerate-camera-inside-alpha/k2scene1", "69518e6050c7c508f5cf4b3ccd1bcd22553d694174b544a3e74ec24655b7cdba"),
    ("degenerate-camera-inside-alpha/0", "f65d38be709861b700b7206a81b687b119e49bb1d0bf8467b76e349a740eb482"),
    ("terrain-200k-alpha/k2scene1", "9fe86ae3af59abeafd74a17458ecea65357a7d40a54549eb605c534094a0e789"),
    ("terrain-200k-alpha/0", "8446abc6adb8ac39056a7d8103b281e2b7e04a5fd947f41774d9ce0f784a6208"),
    // Content framing (auto views fitted to the contents, a lens shift), on the interior scenes.
    ("interior/k2scene1", "b485a4d405aec8ff19e4258b4697b96fb3f74d5e9e402f70925223586302e2f2"),
    ("interior/0", "0ed4575e66103f2ddb39a7a6b8d336c8cc178ee22bfa954cf35bb146b1041004"),
    ("interior/1", "5604afb144e8e0ff199a89781a87e972807e13ea167dac5928487ceb229c1410"),
    ("interior/2", "109d6ad23854809bf21b5b7e0dd64e6fd325b76c96a3cf8e8bf24f88e172d921"),
    ("interior-alpha/k2scene1", "b485a4d405aec8ff19e4258b4697b96fb3f74d5e9e402f70925223586302e2f2"),
    ("interior-alpha/0", "9d8f9f72213be3d23108ad467d263966f583a1422acd4573a405b2a3e827e936"),
    ("empty-room/k2scene1", "5c87521c765dda93a8341896f22ac8f27ed0cdd2586dabad17e031b044377efc"),
    ("empty-room/0", "951187c8b46deed6344fbd83ddfd3d9689446d27c5ec1561e0707774b80f7889"),
    ("empty-room/1", "951187c8b46deed6344fbd83ddfd3d9689446d27c5ec1561e0707774b80f7889"),
    ("small-object/k2scene1", "25c2e806f1c3b53fa889bd36b059c299f268474f77be3a63bbb0b3cd9a8441f6"),
    ("small-object/0", "4069b84340122fecbe49a99b012dd150790f7827c8b7972ade287ed10d9153cb"),
    // Clay shading (two tones by class), bounds- and contents-framed, opaque and transparent.
    ("interior-clay/k2scene1", "b485a4d405aec8ff19e4258b4697b96fb3f74d5e9e402f70925223586302e2f2"),
    ("interior-clay/0", "396a83e58c4a54a92f70a14a1837948ff2a0d3bffb26c06a22e0722cccaf069c"),
    ("interior-clay/1", "a645e958de4a677d3768dea51fb8c648184cd5dff5e7096ba59dc9086c5d0e35"),
    ("interior-clay/2", "d051fce83a2bd47b7e75f84d06f292281e6d1019843efbebbd26316cbffc6203"),
    ("interior-clay-alpha/k2scene1", "b485a4d405aec8ff19e4258b4697b96fb3f74d5e9e402f70925223586302e2f2"),
    ("interior-clay-alpha/0", "4d2167db9532dfd44f31efef817076ed548013ddfe5225d8ce856352ec5d269c"),
    ("interior-clay-alpha/1", "fde8e942f28bc4164d8e1367d22b4ee44ef4a8430421fca56dc9f116dda46c7f"),
    ("interior-clay-tones/k2scene1", "b485a4d405aec8ff19e4258b4697b96fb3f74d5e9e402f70925223586302e2f2"),
    ("interior-clay-tones/0", "ee3aa1b905e185a746619760dd2f7776f3f4a1ad778965dc2a7b586c16e5f08b"),
    ("empty-room-clay/k2scene1", "5c87521c765dda93a8341896f22ac8f27ed0cdd2586dabad17e031b044377efc"),
    ("empty-room-clay/0", "00ddd81edbb0ef3cf1d7add18a31cda44366710913db0f6b85855877eb55425c"),
    ("small-object-clay/k2scene1", "25c2e806f1c3b53fa889bd36b059c299f268474f77be3a63bbb0b3cd9a8441f6"),
    ("small-object-clay/0", "615441da2daffda32ea9249726a5e2231d15e3e750f6f895f0f202eca78aa0f9"),    // Ambient occlusion (opaque and transparent, clay and base colours, default and own values).
    ("interior-ao/k2scene1", "b485a4d405aec8ff19e4258b4697b96fb3f74d5e9e402f70925223586302e2f2"),
    ("interior-ao/0", "a9bb7ffb1f4b79b197fa05e8c7c8bb5ab2dcf42531f1adf60e716023a3e279de"),
    ("interior-ao/1", "d7e313f7429abad3dcdf1955581e0fd870a5adff442db01b4230c8aa3f8b14d4"),
    ("interior-ao/2", "764c58410764e866b1680a5a16bb4efb89bc2cdc9472139da62d2daf94889928"),
    ("interior-clay-ao/k2scene1", "b485a4d405aec8ff19e4258b4697b96fb3f74d5e9e402f70925223586302e2f2"),
    ("interior-clay-ao/0", "4e7d22dbb3efd5ac5f79ba54d7144999898b71a013e40da42209800b8f0c86bf"),
    ("interior-clay-ao/1", "47b0ef31e1e59fc05d8a189c83a685b716315a1716210542c7424e1b75d0da46"),
    ("interior-clay-ao-alpha/k2scene1", "b485a4d405aec8ff19e4258b4697b96fb3f74d5e9e402f70925223586302e2f2"),
    ("interior-clay-ao-alpha/0", "b7821a95db8fa64689e7fceb4476845844468404f955accb248494e359bc6104"),
    ("interior-ao-alpha/k2scene1", "b485a4d405aec8ff19e4258b4697b96fb3f74d5e9e402f70925223586302e2f2"),
    ("interior-ao-alpha/0", "83349303973eff69737a3bb70d2204a1bb2898141afd9dcde0f4424495fd42bc"),
    ("interior-clay-ao-own/k2scene1", "b485a4d405aec8ff19e4258b4697b96fb3f74d5e9e402f70925223586302e2f2"),
    ("interior-clay-ao-own/0", "1966f900a5b0a75e9dc1ba17b9c9bb1b6dd9b3a82bc79b84a6ef281b49085b9a"),
    ("small-object-clay-ao/k2scene1", "25c2e806f1c3b53fa889bd36b059c299f268474f77be3a63bbb0b3cd9a8441f6"),
    ("small-object-clay-ao/0", "a1f73af4c1cb4badf01a9f2b939f1eeae2c750f7c0d08a9977738ad7332d12c4"),
    // Object ids: each view's still (the same digest as without ids) and its id image.
    ("interior-ids/k2scene1", "b485a4d405aec8ff19e4258b4697b96fb3f74d5e9e402f70925223586302e2f2"),
    ("interior-ids/0", "0ed4575e66103f2ddb39a7a6b8d336c8cc178ee22bfa954cf35bb146b1041004"),
    ("interior-ids/0/ids", "869df1c0821202f5e9c7697de2027a3c49f971cbe530975c4a697fab7e2363c0"),
    ("interior-ids/1", "5604afb144e8e0ff199a89781a87e972807e13ea167dac5928487ceb229c1410"),
    ("interior-ids/1/ids", "2251c9ac83286ce4d2e0951dc33150a65e09c29c653447e310babb59a3fdb329"),
    ("interior-ids/2", "077ee9c670b1110eef37d7bfbe35fe200dd1bba0bf56ba09b6cfeb94eefb0bb1"),
    ("interior-ids/2/ids", "774b7c5918640351a290895b3d550c7d41b41337e35336e5f1c14b75080570a8"),
    ("interior-ids/3", "564c63a628b41ce4a20f2e8ce4b1116686a712082f2b1c7c4a26b3b0c265681c"),
    ("interior-ids/3/ids", "970321f1ed76268c6d5760116414826a41aeb61639577ab53378afe00306c875"),
    ("interior-clay-ao-ids-alpha/k2scene1", "b485a4d405aec8ff19e4258b4697b96fb3f74d5e9e402f70925223586302e2f2"),
    ("interior-clay-ao-ids-alpha/0", "b7821a95db8fa64689e7fceb4476845844468404f955accb248494e359bc6104"),
    ("interior-clay-ao-ids-alpha/0/ids", "2251c9ac83286ce4d2e0951dc33150a65e09c29c653447e310babb59a3fdb329"),
    ("interior-clay-ao-ids-1920/k2scene1", "b485a4d405aec8ff19e4258b4697b96fb3f74d5e9e402f70925223586302e2f2"),
    ("interior-clay-ao-ids-1920/0", "f1b67dc5de403a0a5992dc67e10eb9747a517b61542509c2e1601b9b1fcab120"),
    ("interior-clay-ao-ids-1920/0/ids", "480a871d5e95e86757e7e5b994c2afc3193dca0449c01c27d32363cd5cf73416"),
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
    case_with(out, name, s, views, format, threads, false)
}

fn case_with(out: &mut Vec<(String, String)>, name: &str, s: &Scene, views: Vec<View>, format: ImageFormat, threads: u32, transparent: bool) {
    case_clay(out, name, s, views, format, threads, transparent, None)
}

#[allow(clippy::too_many_arguments)]
fn case_clay(out: &mut Vec<(String, String)>, name: &str, s: &Scene, views: Vec<View>, format: ImageFormat, threads: u32, transparent: bool, clay: Option<Clay>) {
    case_ao(out, name, s, views, format, threads, transparent, clay, None)
}

#[allow(clippy::too_many_arguments)]
fn case_ao(out: &mut Vec<(String, String)>, name: &str, s: &Scene, views: Vec<View>, format: ImageFormat, threads: u32, transparent: bool, clay: Option<Clay>, ao: Option<Ao>) {
    case_ids(out, name, s, views, format, threads, transparent, clay, ao, false)
}

/// With `ids`, each view's id image too, as `<name>/<i>/ids`, after its still.
#[allow(clippy::too_many_arguments)]
fn case_ids(out: &mut Vec<(String, String)>, name: &str, s: &Scene, views: Vec<View>, format: ImageFormat, threads: u32, transparent: bool, clay: Option<Clay>, ao: Option<Ao>, ids: bool) {
    let input = encode(s);
    out.push((format!("{name}/k2scene1"), hex(&digest(&input))));
    let mut req = Request::new(views);
    req.format = format;
    req.threads = threads;
    req.transparent = transparent;
    req.clay = clay;
    req.ao = ao;
    req.ids = ids;
    let (o, _) = render_bytes(&input, &req, &caps()).unwrap_or_else(|f| panic!("{name}: {} {}", f.code, f.detail));
    assert_eq!(o.ids.len(), if ids { o.views.len() } else { 0 }, "{name}");
    for (i, (_, img)) in o.views.iter().enumerate() {
        out.push((format!("{name}/{i}"), hex(&digest(img))));
        if let Some(png) = o.ids.get(i) {
            out.push((format!("{name}/{i}/ids"), hex(&digest(png))));
        }
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
    case(&mut got, "camera", &testgen::camera_scene(), vec![View::new(ViewSpec::Camera(0), 640), View { objects: Some(vec![0]), aspect: Some(1.0), ..View::new(ViewSpec::Auto, 200) }], ImageFormat::Webp, 4);
    for (name, s) in testgen::degenerate_scenes() {
        if name == "camera-inside" || name == "collapsed" {
            case(&mut got, &format!("degenerate-{name}"), &s, vec![View::new(if s.cameras.is_empty() { ViewSpec::Auto } else { ViewSpec::Camera(0) }, 120)], ImageFormat::Webp, 2);
        }
    }
    case(&mut got, "terrain-200k", &testgen::large_scene(200_000), vec![auto(640), View::new(ViewSpec::Turntable(6), 1920)], ImageFormat::Webp, 4);
    // Transparent (RGBA, coverage alpha) renders of the same fixtures.
    case_with(&mut got, "cube-webp-320-alpha", &testgen::cube_scene(), vec![auto(320)], ImageFormat::Webp, 1, true);
    case_with(&mut got, "cube-png-320-alpha", &testgen::cube_scene(), vec![auto(320)], ImageFormat::Png, 2, true);
    case_with(&mut got, "sphere-alpha", &testgen::sphere_scene(), vec![auto(320), auto(1280)], ImageFormat::Webp, 3, true);
    let turntable: Vec<View> = (0..8).map(|k| View::new(ViewSpec::Turntable(k), 160)).collect();
    case_with(&mut got, "grid-z-turntable-alpha", &testgen::grid_scene(5, true), turntable, ImageFormat::Webp, 2, true);
    case_with(&mut got, "mirrored-alpha", &testgen::mirrored_scene(), vec![auto(400)], ImageFormat::Webp, 1, true);
    case_with(&mut got, "camera-alpha", &testgen::camera_scene(), vec![View::new(ViewSpec::Camera(0), 640), View { objects: Some(vec![0]), aspect: Some(1.0), ..View::new(ViewSpec::Auto, 200) }], ImageFormat::Webp, 4, true);
    for (name, s) in testgen::degenerate_scenes() {
        if name == "camera-inside" || name == "collapsed" {
            case_with(&mut got, &format!("degenerate-{name}-alpha"), &s, vec![View::new(if s.cameras.is_empty() { ViewSpec::Auto } else { ViewSpec::Camera(0) }, 120)], ImageFormat::Webp, 2, true);
        }
    }
    case_with(&mut got, "terrain-200k-alpha", &testgen::large_scene(200_000), vec![auto(640)], ImageFormat::Webp, 4, true);
    // Content framing: the interior bounds-framed, then contents-framed at 4:3 and 1:1; an
    // all-shell room (falls back: the same bytes as its bounds view); one small object.
    let contents = |w: u32| View { framing: Framing::Contents, ..View::new(ViewSpec::Auto, w) };
    case(&mut got, "interior", &testgen::interior_scene(), vec![auto(320), contents(320), View { aspect: Some(1.0), ..contents(240) }], ImageFormat::Webp, 3);
    case_with(&mut got, "interior-alpha", &testgen::interior_scene(), vec![contents(320)], ImageFormat::Webp, 2, true);
    case(&mut got, "empty-room", &testgen::empty_room_scene(), vec![auto(160), contents(160)], ImageFormat::Webp, 2);
    case(&mut got, "small-object", &testgen::small_object_scene(), vec![contents(240)], ImageFormat::Webp, 1);
    // Clay: the interior bounds-framed, contents-framed and a turntable frame; transparent; the
    // request's own tones; an all-shell room (every object the shell tone); one small object.
    let clay = Some(Clay::default());
    case_clay(&mut got, "interior-clay", &testgen::interior_scene(), vec![auto(320), contents(320), View::new(ViewSpec::Turntable(1), 200)], ImageFormat::Webp, 2, false, clay);
    case_clay(&mut got, "interior-clay-alpha", &testgen::interior_scene(), vec![auto(320), contents(320)], ImageFormat::Png, 3, true, clay);
    let tones = Some(Clay { shell: [92, 98, 110], contents: [236, 226, 204] });
    case_clay(&mut got, "interior-clay-tones", &testgen::interior_scene(), vec![contents(240)], ImageFormat::Webp, 4, false, tones);
    case_clay(&mut got, "empty-room-clay", &testgen::empty_room_scene(), vec![auto(160)], ImageFormat::Webp, 1, false, clay);
    case_clay(&mut got, "small-object-clay", &testgen::small_object_scene(), vec![contents(240)], ImageFormat::Webp, 2, false, clay);
    // Ambient occlusion: the interior in base colours (bounds- and contents-framed, 2x2 SSAA at
    // 800 px) and in clay (contents-framed, a turntable frame), transparent with and without
    // clay, the request's own strength and radius, one small object.
    let ao = Some(Ao::default());
    case_ao(&mut got, "interior-ao", &testgen::interior_scene(), vec![auto(320), contents(320), contents(800)], ImageFormat::Webp, 3, false, None, ao);
    case_ao(&mut got, "interior-clay-ao", &testgen::interior_scene(), vec![contents(320), View::new(ViewSpec::Turntable(1), 200)], ImageFormat::Webp, 2, false, clay, ao);
    case_ao(&mut got, "interior-clay-ao-alpha", &testgen::interior_scene(), vec![contents(320)], ImageFormat::Png, 4, true, clay, ao);
    case_ao(&mut got, "interior-ao-alpha", &testgen::interior_scene(), vec![contents(320)], ImageFormat::Webp, 1, true, None, ao);
    let own = Some(Ao { strength: 0.5, radius: 0.25 });
    case_ao(&mut got, "interior-clay-ao-own", &testgen::interior_scene(), vec![contents(240)], ImageFormat::Webp, 2, false, clay, own);
    case_ao(&mut got, "small-object-clay-ao", &testgen::small_object_scene(), vec![contents(240)], ImageFormat::Webp, 3, false, clay, ao);
    // Object ids: the interior's views above (bounds- and contents-framed), a turntable frame
    // and an object view; transparent in clay with occlusion; the contents-framed still at 1920
    // in clay with occlusion, as the 3D tab draws it.
    let object = |o: u32, w: u32| View { objects: Some(vec![o]), ..View::new(ViewSpec::Auto, w) };
    case_ids(&mut got, "interior-ids", &testgen::interior_scene(), vec![auto(320), contents(320), View::new(ViewSpec::Turntable(1), 200), object(5, 160)], ImageFormat::Webp, 3, false, None, None, true);
    case_ids(&mut got, "interior-clay-ao-ids-alpha", &testgen::interior_scene(), vec![contents(320)], ImageFormat::Png, 4, true, clay, ao, true);
    case_ids(&mut got, "interior-clay-ao-ids-1920", &testgen::interior_scene(), vec![contents(1920)], ImageFormat::Webp, 4, false, clay, ao, true);
    for (k, v) in &got {
        eprintln!("[digest] (\"{k}\", \"{v}\"),");
    }
    eprintln!("[digest] {} digests on {} {}", got.len(), std::env::consts::ARCH, std::env::consts::OS);
    let find = |k: &str| got.iter().find(|g| g.0 == k).map(|g| g.1.clone());
    assert_eq!(find("empty-room/0"), find("empty-room/1"), "an all-shell room falls back to the bounds framing");
    assert_ne!(find("interior/0"), find("interior/1"));
    assert_ne!(find("interior/0"), find("interior-clay/0"));
    assert_ne!(find("interior/1"), find("interior-ao/1"));
    assert_ne!(find("interior-clay/1"), find("interior-clay-ao/0"));
    // Ids leave the stills alone, and don't depend on the look.
    assert_eq!(find("interior-ids/0"), find("interior/0"));
    assert_eq!(find("interior-ids/1"), find("interior/1"));
    assert_eq!(find("interior-clay-ao-ids-alpha/0"), find("interior-clay-ao-alpha/0"));
    assert_eq!(find("interior-clay-ao-ids-alpha/0/ids"), find("interior-ids/1/ids"));
    let want: Vec<(String, String)> = WANT.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let differ: Vec<&str> = got.iter().zip(&want).filter(|(g, w)| g != w).map(|(g, _)| g.0.as_str()).collect();
    assert!(differ.is_empty() && got.len() == want.len(), "{} of {} differ ({} checked in): {differ:?}", differ.len(), got.len(), want.len());
}
