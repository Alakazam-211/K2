//! The real worker, through the host: renders of generated scenes equal the
//! in-process renders byte for byte; several views in one process equal one
//! view per process; the bytes are the same across runs and thread counts;
//! hostile and degenerate inputs end typed; a mesh near the caps renders.
//! Scenes are generated in code (k2_scenerender::testgen): no model files.

use std::path::Path;
use std::time::{Duration, Instant};

use k2_scenerender::camera::{self, ViewSpec};
use k2_scenerender::host::{self, Limits, SceneRequest};
use k2_scenerender::proto::{CameraMeta, FailureCode, ImageFormat, Request, View};
use k2_scenerender::render;
use k2_scenerender::scene::{encode, InputLimits, Scene};
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

/// A PNG's pixels: (width, height, samples per pixel, bytes).
fn decode_png(b: &[u8]) -> (u32, u32, usize, Vec<u8>) {
    let mut r = png::Decoder::new(std::io::Cursor::new(b)).read_info().expect("a png");
    let mut buf = vec![0; r.output_buffer_size().expect("its size")];
    let info = r.next_frame(&mut buf).expect("its frame");
    buf.truncate(info.buffer_size());
    (info.width, info.height, info.color_type.samples(), buf)
}

fn mul(m: &[f32; 16], p: [f64; 4]) -> [f64; 4] {
    let mut o = [0.0; 4];
    for (r, v) in o.iter_mut().enumerate() {
        *v = (0..4).map(|c| m[c * 4 + r] as f64 * p[c]).sum();
    }
    o
}

/// The drawn objects' mesh boxes (8 local corners each, through the object's
/// transform), projected to pixels with a view's returned matrices: min x,
/// min y, max x, max y (continuous, y down).
fn projected_box(s: &Scene, objects: &[u32], cam: &CameraMeta, w: u32, h: u32) -> [f64; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for &oi in objects {
        let o = &s.objects[oi as usize];
        let Some((lo, hi)) = camera::mesh_bounds(&s.meshes[o.mesh as usize].positions) else { continue };
        for k in 0..8 {
            let p = [if k & 1 == 0 { lo[0] } else { hi[0] }, if k & 2 == 0 { lo[1] } else { hi[1] }, if k & 4 == 0 { lo[2] } else { hi[2] }, 1.0];
            let clip = mul(&cam.proj, mul(&cam.view, mul(&o.transform, p)));
            assert!(clip[3] > 0.0, "a corner behind the eye");
            let (nx, ny) = (clip[0] / clip[3], clip[1] / clip[3]);
            let (px, py) = ((nx * 0.5 + 0.5) * w as f64, (0.5 - ny * 0.5) * h as f64);
            b = [b[0].min(px), b[1].min(py), b[2].max(px), b[3].max(py)];
        }
    }
    b
}

/// Every view's meta carries the camera it was drawn through: the
/// rasterizer's own f32 values, through the worker's response. Projecting each
/// drawn object's box with them bounds the covered pixels (within 1 px), and a
/// cube's (a convex box) fits them tightly.
#[test]
fn every_still_carries_the_camera_that_drew_it() {
    const MAGENTA: [u8; 3] = [255, 0, 255];
    let cases: Vec<(&str, Scene, View, bool)> = vec![
        ("cube", testgen::cube_scene(), View::new(ViewSpec::Auto, 320), true),
        ("white-cube-turntable-7", testgen::white_cube_scene(), View::new(ViewSpec::Turntable(7), 200), true),
        ("camera", testgen::camera_scene(), View::new(ViewSpec::Camera(0), 320), true),
        ("camera-object", testgen::camera_scene(), View { view: ViewSpec::Auto, objects: Some(vec![0]), width: 160, aspect: Some(1.0) }, true),
        ("grid-turntable-2", testgen::grid_scene(3, true), View::new(ViewSpec::Turntable(2), 200), false),
        ("grid-turntable-5", testgen::grid_scene(3, false), View::new(ViewSpec::Turntable(5), 200), false),
        ("grid-one-object", testgen::grid_scene(3, false), View { view: ViewSpec::Auto, objects: Some(vec![4]), width: 120, aspect: None }, false),
        ("mirrored", testgen::mirrored_scene(), View::new(ViewSpec::Auto, 256), false),
        ("sphere", testgen::sphere_scene(), View::new(ViewSpec::Auto, 200), false),
    ];
    for (name, s, view, tight) in cases {
        let input = encode(&s);
        let mut req = SceneRequest::new(&input, vec![view.clone()]);
        req.format = ImageFormat::Png;
        req.background = Some(MAGENTA);
        let r = host::render(bin(), &req, &thumb()).unwrap_or_else(|f| panic!("{name}: {} {}", f.code, f.detail));
        let m = &r.views[0].meta;
        let cam = m.camera.unwrap_or_else(|| panic!("{name}: no camera"));
        // The rasterizer's values, bit for bit (not recomputed from the matrices).
        let active: Vec<u32> = view.objects.clone().unwrap_or_else(|| (0..s.objects.len() as u32).collect());
        let plan = render::plan(&render::scene_counts(&s), &s.objects, &s.cameras, &Request::new(vec![view.clone()]), &thumb().args(1).caps).expect("the plan");
        let f = camera::frame(&s, &active, view.view, plan[0].aspect).expect("the frame");
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<u32>>();
        assert_eq!(bits(&[cam.fx, cam.fy, cam.near, cam.shift_x, cam.shift_y]), bits(&[f.fx, f.fy, f.near, 0.0, 0.0]), "{name}");
        assert_eq!(bits(&cam.view), bits(&f.view_f32()), "{name}");
        assert_eq!(bits(&cam.proj), bits(&f.projection()), "{name}");
        assert_eq!((cam.proj[0], cam.proj[5], cam.proj[11], cam.proj[14], cam.proj[15]), (cam.fx, cam.fy, -1.0, -2.0 * cam.near, 0.0), "{name}");
        let (w, h, n, px) = decode_png(&r.views[0].image);
        assert_eq!((w, h, n), (m.width, m.height, 3));
        let (mut x0, mut y0, mut x1, mut y1, mut count) = (u32::MAX, u32::MAX, 0u32, 0u32, 0u64);
        for y in 0..h {
            for x in 0..w {
                let o = ((y * w + x) * 3) as usize;
                if px[o..o + 3] != MAGENTA {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
                    count += 1;
                }
            }
        }
        assert!(count > 0 && count <= m.covered_px, "{name}: {count} pixels drawn, {} covered", m.covered_px);
        let b = projected_box(&s, &active, &cam, w, h);
        eprintln!("[scenerender] camera {name}: covered [{x0}, {y0}, {x1}, {y1}], projected box [{:.2}, {:.2}, {:.2}, {:.2}]", b[0], b[1], b[2], b[3]);
        let (x0, y0, x1, y1) = (x0 as f64, y0 as f64, x1 as f64, y1 as f64);
        assert!(x0 >= b[0] - 1.0 && y0 >= b[1] - 1.0 && x1 <= b[2] + 1.0 && y1 <= b[3] + 1.0, "{name}: the covered pixels leave the projected box");
        if tight {
            assert!(x0 <= b[0].max(0.0) + 2.0 && y0 <= b[1].max(0.0) + 2.0 && x1 >= b[2].min(w as f64) - 2.0 && y1 >= b[3].min(h as f64) - 2.0, "{name}: a box's covered pixels fill its projection");
        }
    }
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
            Ok(r) => {
                assert_eq!(r.views.len(), 1, "{name}");
                // A camera with a value past f32's range (the huge scene) isn't sent.
                assert!(r.views[0].meta.camera.is_none_or(|c| c.is_finite()), "{name}");
            }
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
