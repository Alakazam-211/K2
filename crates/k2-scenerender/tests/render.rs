//! The real worker, through the host: renders of generated scenes equal the
//! in-process renders byte for byte; several views in one process equal one
//! view per process; the bytes are the same across runs and thread counts;
//! hostile and degenerate inputs end typed; a mesh near the caps renders.
//! Scenes are generated in code (k2_scenerender::testgen): no model files.

use std::path::Path;
use std::time::{Duration, Instant};

use k2_scenerender::camera::ViewSpec;
use k2_scenerender::host::{self, Limits, SceneRequest};
use k2_scenerender::proto::{FailureCode, ImageFormat, Request, View};
use k2_scenerender::render;
use k2_scenerender::scene::{encode, InputLimits};
use k2_scenerender::testgen;

fn bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_k2-scenerender"))
}

/// Unoptimised test builds are slower than release: a generous wall.
fn thumb() -> Limits {
    Limits { wall: Duration::from_secs(60), ..Limits::thumbnail() }
}

fn still() -> Limits {
    Limits { wall: Duration::from_secs(60), ..Limits::still() }
}

#[test]
fn the_worker_draws_what_the_library_draws() {
    let input = encode(&testgen::camera_scene());
    let views = vec![View::new(ViewSpec::Auto, 320), View::new(ViewSpec::Camera(0), 320), View::new(ViewSpec::Turntable(5), 200)];
    for format in [ImageFormat::Webp, ImageFormat::Png] {
        let mut req = SceneRequest::new(&input, views.clone());
        req.format = format;
        let started = Instant::now();
        let r = host::render(bin(), &req, &thumb()).expect("render");
        eprintln!("[scenerender] 3 views ({}) in {} ms: {:?}", format.as_str(), started.elapsed().as_millis(), r.timings);
        assert_eq!(r.format, format);
        assert_eq!(r.views.len(), 3);
        let mut lib = Request::new(views.clone());
        lib.format = format;
        lib.threads = 1;
        let (o, _) = render::render_bytes(&input, &lib, &thumb().args(1).caps).expect("in process");
        for (i, (w, l)) in r.views.iter().zip(&o.views).enumerate() {
            assert_eq!(w.image, l.1, "view {i} ({})", format.as_str());
            assert_eq!(w.meta.height, l.0.height);
        }
        assert_eq!((r.views[0].meta.width, r.views[0].meta.height), (320, 240));
        assert_eq!((r.views[1].meta.width, r.views[1].meta.height), (320, 180), "the camera's 16:9");
        assert!(r.views[0].meta.covered_px > 320 * 240 / 20);
    }
}

#[test]
fn a_turntable_is_one_process_with_the_bytes_of_eight() {
    let input = encode(&testgen::grid_scene(4, true));
    let views: Vec<View> = (0..8).map(|k| View::new(ViewSpec::Turntable(k), 160)).collect();
    let all = host::render(bin(), &SceneRequest::new(&input, views.clone()), &thumb()).expect("turntable");
    assert_eq!(all.views.len(), 8);
    for (k, v) in views.into_iter().enumerate() {
        let one = host::render(bin(), &SceneRequest::new(&input, vec![v]), &thumb()).expect("one frame");
        assert_eq!(one.views[0].image, all.views[k].image, "frame {k}");
    }
    let distinct: std::collections::BTreeSet<&[u8]> = all.views.iter().map(|v| v.image.as_slice()).collect();
    assert!(distinct.len() > 4, "the frames differ: {}", distinct.len());
}

#[test]
fn the_same_bytes_across_runs_and_thread_counts() {
    let input = encode(&testgen::grid_scene(12, false));
    let views = vec![View::new(ViewSpec::Auto, 1280), View::new(ViewSpec::Turntable(1), 1280)];
    let first = host::render(bin(), &SceneRequest::new(&input, views.clone()), &still()).expect("first");
    for threads in [1, 2, 3, 4] {
        let l = Limits { threads, ..still() };
        let again = host::render(bin(), &SceneRequest::new(&input, views.clone()), &l).expect("again");
        for (a, b) in first.views.iter().zip(&again.views) {
            assert!(a.image == b.image, "threads {threads} differ");
        }
    }
}

#[test]
fn a_background_is_a_request_parameter() {
    let input = encode(&testgen::cube_scene());
    let mut req = SceneRequest::new(&input, vec![View::new(ViewSpec::Auto, 64)]);
    req.format = ImageFormat::Png;
    let grey = host::render(bin(), &req, &thumb()).expect("default");
    req.background = Some([20, 30, 40]);
    let dark = host::render(bin(), &req, &thumb()).expect("dark");
    assert_ne!(grey.views[0].image, dark.views[0].image);
    let mut lib = Request::new(vec![View::new(ViewSpec::Auto, 64)]);
    lib.format = ImageFormat::Png;
    lib.background = Some([240, 240, 240]);
    let explicit = render::render_bytes(&input, &lib, &thumb().args(1).caps).expect("explicit").0;
    assert_eq!(explicit.views[0].1, grey.views[0].image, "the default is sRGB 240");
}

#[test]
fn hostile_inputs_end_typed() {
    let good = encode(&testgen::cube_scene());
    let one = || vec![View::new(ViewSpec::Auto, 64)];
    let code = |bytes: &[u8], limits: &Limits| host::render(bin(), &SceneRequest::new(bytes, one()), limits).expect_err("refused");
    // Garbage, truncated, trailing bytes.
    assert_eq!(code(b"garbage, not a scene at all", &thumb()).code, FailureCode::InvalidInput);
    assert_eq!(code(b"", &thumb()).code, FailureCode::InvalidInput);
    assert_eq!(code(&good[..good.len() - 5], &thumb()).code, FailureCode::InvalidInput);
    assert_eq!(code(&good[..20], &thumb()).code, FailureCode::InvalidInput);
    let mut more = good.clone();
    more.extend_from_slice(&[0; 8]);
    assert_eq!(code(&more, &thumb()).code, FailureCode::InvalidInput);
    // Oversized counts in the header, with no data behind them: refused from the header.
    let j = r#"{"version":1,"meshes":[{"vertices":3,"triangles":4000000000,"normals":false}],"objects":[{"id":"a","mesh":0,"base_color":[1,1,1,1],"transform":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]}]}"#;
    let mut b = b"K2SCENE1".to_vec();
    b.extend_from_slice(&(j.len() as u32).to_le_bytes());
    b.extend_from_slice(j.as_bytes());
    let f = code(&b, &thumb());
    assert_eq!(f.code, FailureCode::TooLarge, "{}", f.detail);
    // A huge header length.
    let mut h = b"K2SCENE1".to_vec();
    h.extend_from_slice(&u32::MAX.to_le_bytes());
    h.extend_from_slice(&[b' '; 64]);
    assert_eq!(code(&h, &thumb()).code, FailureCode::TooLarge);
    // The worker's input cap (the host's own check is skipped by giving it a larger one).
    let tight = Limits { input: InputLimits { max_drawn_triangles: 6, max_drawn_vertices: 6, ..InputLimits::default() }, ..thumb() };
    assert_eq!(code(&good, &tight).code, FailureCode::TooLarge);
    // A view naming what isn't there.
    let f = host::render(bin(), &SceneRequest::new(&good, vec![View::new(ViewSpec::Camera(3), 64)]), &thumb()).expect_err("no camera");
    assert_eq!(f.code, FailureCode::InvalidInput);
    // Memory: the estimate refuses before the data is read.
    let big = encode(&testgen::large_scene(400_000));
    let small = Limits { memory_bytes: 24 << 20, ..thumb() };
    let started = Instant::now();
    let f = code(&big, &small);
    assert_eq!(f.code, FailureCode::TooLarge, "{}", f.detail);
    assert!(f.detail.contains("memory"), "{}", f.detail);
    eprintln!("[scenerender] memory refusal after {} ms: {}", started.elapsed().as_millis(), f.detail);
}

#[test]
fn degenerate_scenes_render_or_refuse_typed() {
    for (name, s) in testgen::degenerate_scenes() {
        let input = encode(&s);
        let view = if s.cameras.is_empty() { ViewSpec::Auto } else { ViewSpec::Camera(0) };
        match host::render(bin(), &SceneRequest::new(&input, vec![View::new(view, 48)]), &thumb()) {
            Ok(r) => assert_eq!(r.views.len(), 1, "{name}"),
            Err(f) => assert_ne!(f.code, FailureCode::RendererError, "{name}: {} | {}", f.detail, f.stderr),
        }
    }
}

#[test]
fn a_mesh_near_the_caps_renders() {
    // 7.9M triangles, 4M vertices: under the 8M caps, about 150 MB of K2SCENE1.
    let s = testgen::large_scene(7_900_000);
    let tris = s.meshes[0].indices.len();
    assert!(tris > 7_800_000 && tris <= 8_000_000, "{tris}");
    let input = encode(&s);
    let started = Instant::now();
    let r = host::render(bin(), &SceneRequest::new(&input, vec![View::new(ViewSpec::Auto, 320)]), &Limits { wall: Duration::from_secs(120), ..Limits::thumbnail() });
    let r = r.unwrap_or_else(|f| panic!("{}: {} | {}", f.code, f.detail, f.stderr));
    eprintln!(
        "[scenerender] near the caps: {tris} triangles, {} bytes in, {} ms total, estimate {} MiB, {:?}, view {:?}",
        input.len(),
        started.elapsed().as_millis(),
        r.estimate_bytes >> 20,
        r.timings,
        r.views[0].meta
    );
    assert!(r.views[0].meta.covered_px > 1000);
    // One more triangle drawn than the cap: refused from the header.
    let mut over = testgen::large_scene(7_900_000);
    let extra = over.meshes[0].indices[0];
    while over.meshes[0].indices.len() <= 8_000_000 {
        over.meshes[0].indices.push(extra);
    }
    let f = host::render(bin(), &SceneRequest::new(&encode(&over), vec![View::new(ViewSpec::Auto, 320)]), &thumb()).expect_err("over the cap");
    assert_eq!(f.code, FailureCode::TooLarge, "{}", f.detail);
}

/// Measured times (printed; no assertion on speed): a few generated scenes,
/// thumbnail and still, one process each.
#[test]
fn render_times_on_generated_scenes() {
    let scenes = [("cube", testgen::cube_scene()), ("sphere", testgen::sphere_scene()), ("grid-30", testgen::grid_scene(30, true)), ("terrain-1m", testgen::large_scene(1_000_000))];
    for (name, s) in scenes {
        let input = encode(&s);
        for (w, l) in [(320u32, thumb()), (1920, still())] {
            let started = Instant::now();
            let r = host::render(bin(), &SceneRequest::new(&input, vec![View::new(ViewSpec::Auto, w)]), &l).expect("render");
            let v = &r.views[0].meta;
            eprintln!(
                "[scenerender] time {name} {w} px: {} ms end to end, {} triangles, header {:.1} ms, data {:.1} ms, vertex {:.1}, bin {:.1}, raster {:.1}, encode {:.1} ms, {} bytes",
                started.elapsed().as_millis(),
                v.triangles,
                r.timings.header_ms,
                r.timings.data_ms,
                v.vertex_ms,
                v.bin_ms,
                v.raster_ms,
                v.encode_ms,
                v.image_len
            );
        }
    }
}

#[test]
fn version_and_usage() {
    let out = std::process::Command::new(bin()).arg("--version").output().expect("run");
    let v = String::from_utf8_lossy(&out.stdout);
    assert!(v.starts_with("k2-scenerender ") && v.contains("proto 1"), "{v}");
    // Without the host's argv the worker reads nothing and says how it is run.
    let out = std::process::Command::new(bin()).stdin(std::process::Stdio::null()).output().expect("run");
    assert_eq!(out.status.code(), Some(k2_scenerender::proto::EXIT_USAGE));
    assert!(out.stdout.is_empty());
}
