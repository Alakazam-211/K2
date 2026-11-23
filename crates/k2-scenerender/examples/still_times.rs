//! Timed 3D tab stills: generated scenes from a furnished room up to two
//! million triangles, each drawn the way the host asks for the scene view
//! (WebP lossless, transparent, clay, ambient occlusion on its defaults,
//! ids, framed on the contents) at 1,920 px in both still shapes, 16:9 and
//! 4:3, with ambient occlusion on and off. Each still is one worker process,
//! through the host, `runs` times per configuration (the first is the cold
//! run). Prints one JSON line per render, then one per scene and shape with
//! the occlusion passes timed alone (in process, the same drawing), then the
//! occlusion kernel alone on a fully covered depth image of each shape, at
//! the full width and at half of it, and a depth-aware upsample of a half
//! size occlusion image back to the full size.
//! `cargo run --release -p k2-scenerender --example still_times -- <worker binary> [runs] [scene filter] [threads]`:
//! the filter is a comma-separated list of parts of scene names (`all` for
//! every scene); the threads are a count, or `host` (the default) for the
//! host's limits for a 1,920 px still with its id image (4 threads, 10 s).

use std::path::Path;
use std::time::{Duration, Instant};

use k2_scenerender::camera::{self, Framing, ViewSpec};
use k2_scenerender::contents;
use k2_scenerender::host::{self, Limits, SceneRequest};
use k2_scenerender::proto::{Ao, Clay, ImageFormat, View};
use k2_scenerender::raster;
use k2_scenerender::scene::{encode, Scene};
use k2_scenerender::testgen::{self, object, placed};
use serde_json::json;

const WIDTH: u32 = 1920;

fn main() {
    let mut args = std::env::args().skip(1);
    let bin = args.next().expect("the worker binary's path");
    let runs: usize = args.next().and_then(|r| r.parse().ok()).unwrap_or(12).max(2);
    let only: Option<Vec<String>> = args.next().filter(|f| f != "all").map(|f| f.split(',').map(String::from).collect());
    let bin = Path::new(&bin);
    // The host's still limits, its output cap doubled for the id image; or a thread count, with a
    // wall long enough to time the heaviest scene.
    let host_limits = Limits { max_output_bytes: 8 << 20, ..Limits::still() };
    let limits = match args.next().as_deref() {
        None | Some("host") => host_limits,
        Some(n) => Limits { threads: n.parse().expect("a thread count or host"), wall: Duration::from_secs(60), ..host_limits },
    };
    let threads = limits.threads as usize;
    println!("{}", json!({"kind": "limits", "threads": limits.threads, "wall_s": limits.wall.as_secs_f64(), "memory_mib": limits.memory_bytes >> 20, "max_output_mib": limits.max_output_bytes >> 20}));
    let shapes = [("wide", 16.0f32 / 9.0), ("tall", 4.0f32 / 3.0)];
    for (name, scene) in corpus() {
        if only.as_ref().is_some_and(|o| !o.iter().any(|part| name.contains(part.as_str()))) {
            continue;
        }
        let input = encode(&scene);
        let triangles: u64 = scene.objects.iter().map(|o| scene.meshes[o.mesh as usize].indices.len() as u64).sum();
        println!("{}", json!({"kind": "scene", "scene": name, "objects": scene.objects.len(), "triangles": triangles, "input_bytes": input.len()}));
        for (shape, aspect) in shapes {
            for ao in [true, false] {
                for run in 0..runs {
                    let mut req = SceneRequest::new(&input, vec![View { aspect: Some(aspect), framing: Framing::Contents, ..View::new(ViewSpec::Auto, WIDTH) }]);
                    req.format = ImageFormat::Webp;
                    req.background = None;
                    req.transparent = true;
                    req.clay = Some(Clay::default());
                    req.ao = ao.then(Ao::default);
                    req.ids = true;
                    let started = Instant::now();
                    let r = host::render(bin, &req, &limits);
                    let wall_ms = started.elapsed().as_secs_f64() * 1e3;
                    match r {
                        Ok(r) => {
                            let v = &r.views[0].meta;
                            let t = &r.timings;
                            println!(
                                "{}",
                                json!({
                                    "kind": "render", "scene": name, "shape": shape, "ao": ao, "run": run, "cold": run == 0,
                                    "width": v.width, "height": v.height, "ssaa": v.ssaa, "triangles": v.triangles, "covered_px": v.covered_px,
                                    "host_ms": round(wall_ms), "worker_ms": round(t.header_ms + t.data_ms + t.views_ms),
                                    "header_ms": t.header_ms, "data_ms": t.data_ms, "views_ms": t.views_ms,
                                    "vertex_ms": v.vertex_ms, "bin_ms": v.bin_ms, "raster_ms": v.raster_ms, "encode_ms": v.encode_ms,
                                    "image_bytes": v.image_len, "ids_bytes": v.ids.as_ref().map_or(0, |i| i.image_len),
                                    "contents_framed": v.framing.as_ref().is_some_and(|f| f.contents), "estimate_mib": r.estimate_bytes >> 20,
                                })
                            );
                        }
                        Err(f) => println!("{}", json!({"kind": "failed", "scene": name, "shape": shape, "ao": ao, "run": run, "host_ms": round(wall_ms), "code": f.code.as_str(), "detail": f.detail})),
                    }
                }
            }
            occlusion_split(name, &scene, shape, aspect, threads);
        }
    }
    for (shape, aspect) in shapes {
        for t in [1, threads] {
            for width in [WIDTH, WIDTH / 2] {
                kernel_alone(shape, aspect, width, t);
            }
            upsample_alone(shape, aspect, t);
        }
    }
}

fn round(ms: f64) -> f64 {
    (ms * 1e3).round() / 1e3
}

/// The scene view drawn in process as the worker draws it (clay, ids, transparent, framed on the
/// contents, on `threads`), with ambient occlusion: the depth and occlusion passes' time alone
/// next to the whole raster time. Three runs; each printed.
fn occlusion_split(name: &str, scene: &Scene, shape: &str, aspect: f32, threads: usize) {
    let active: Vec<u32> = (0..scene.objects.len() as u32).collect();
    let classes = contents::classify(scene, &active);
    let Ok((frame, _)) = camera::frame_contents(scene, &active, &classes, aspect as f64) else {
        println!("{}", json!({"kind": "split_failed", "scene": name, "shape": shape}));
        return;
    };
    let shell: Vec<bool> = (0..active.len()).map(|k| classes.is_shell(k)).collect();
    let c = Clay::default();
    let clay = raster::Clay { shell: &shell, shell_srgb: c.shell, contents_srgb: c.contents };
    let a = Ao::default();
    let ao = raster::Ao { radius: (a.radius as f64 * camera::bounding_sphere(scene, &active).1) as f32, strength: a.strength };
    let height = (WIDTH as f64 / aspect as f64).round() as u32;
    let params = raster::Params { width: WIDTH, height, ssaa: 2, threads, background_srgb: [0, 0, 0], transparent: true };
    for run in 0..3 {
        let (_, _, st) = raster::render_with_ids(scene, &active, &frame, &params, Some(&clay), Some(&ao));
        println!(
            "{}",
            json!({
                "kind": "ao_split", "scene": name, "shape": shape, "threads": threads, "run": run,
                "vertex_ms": st.vertex_us as f64 / 1e3, "bin_ms": st.bin_us as f64 / 1e3,
                "raster_ms": st.raster_us as f64 / 1e3, "ao_ms": st.ao_us as f64 / 1e3,
            })
        );
    }
}

/// The occlusion kernel alone (no depth pass) on a depth image covering every pixel: a floor
/// plane receding from the camera with a box standing on it, so the kernel finds occluders.
fn kernel_alone(shape: &str, aspect: f32, width: u32, threads: usize) {
    let (w, h) = (width as usize, (width as f64 / aspect as f64).round() as usize);
    let mut depth = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            // Inverse view depth: nearer at the bottom of the image; a box in the middle third.
            let floor = 0.05 + 0.45 * y as f32 / h as f32;
            let in_box = x > w / 3 && x < 2 * w / 3 && y > h / 3 && y < 2 * h / 3;
            depth[y * w + x] = if in_box { floor.max(0.3) } else { floor };
        }
    }
    let lens = raster::Lens { fx: 1.2, fy: 1.2 * aspect, shift: [0.0, 0.0], at: raster::Lens::sample_at(2) };
    let ao = raster::Ao { radius: 0.5, strength: Ao::default().strength };
    for run in 0..5 {
        let started = Instant::now();
        let f = raster::occlusion(&depth, w, h, &lens, &ao, threads);
        let ms = started.elapsed().as_secs_f64() * 1e3;
        println!("{}", json!({"kind": "ao_kernel", "shape": shape, "width": w, "height": h, "threads": threads, "run": run, "ms": round(ms), "check": f[w * h / 2]}));
    }
}

/// A half size occlusion image brought up to the full size, each pixel from its four nearest
/// half size pixels, bilinear and weighted by how close their depth is to its own (so occlusion
/// doesn't bleed across edges): what drawing the occlusion at half the width would add. Rows
/// split across `threads`.
fn upsample_alone(shape: &str, aspect: f32, threads: usize) {
    let (w, h) = (WIDTH as usize, (WIDTH as f64 / aspect as f64).round() as usize);
    let (lw, lh) = (w / 2, h / 2);
    let depth: Vec<f32> = (0..w * h).map(|i| 0.05 + 0.45 * (i / w) as f32 / h as f32 + if i % 97 == 0 { 0.1 } else { 0.0 }).collect();
    let low_depth: Vec<f32> = (0..lw * lh).map(|i| depth[(i / lw) * 2 * w + (i % lw) * 2]).collect();
    let low: Vec<f32> = (0..lw * lh).map(|i| 0.5 + 0.5 * ((i * 7919) % 101) as f32 / 101.0).collect();
    for run in 0..5 {
        let started = Instant::now();
        let mut out = vec![1f32; w * h];
        let rows = h.div_ceil(threads.max(1));
        std::thread::scope(|sc| {
            for (c, chunk) in out.chunks_mut(rows * w).enumerate() {
                let (depth, low_depth, low) = (&depth, &low_depth, &low);
                sc.spawn(move || {
                    for (r, row) in chunk.chunks_mut(w).enumerate() {
                        let y = c * rows + r;
                        let fy = ((y as f32 - 0.5) * 0.5).max(0.0);
                        let (y0, ty) = ((fy as usize).min(lh - 1), fy.fract());
                        let y1 = (y0 + 1).min(lh - 1);
                        for (x, o) in row.iter_mut().enumerate() {
                            let d = depth[y * w + x];
                            if d.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
                                continue;
                            }
                            let fx = ((x as f32 - 0.5) * 0.5).max(0.0);
                            let (x0, tx) = ((fx as usize).min(lw - 1), fx.fract());
                            let x1 = (x0 + 1).min(lw - 1);
                            let (mut sum, mut wsum) = (0f32, 0f32);
                            for (q, bw) in [(y0 * lw + x0, (1.0 - tx) * (1.0 - ty)), (y0 * lw + x1, tx * (1.0 - ty)), (y1 * lw + x0, (1.0 - tx) * ty), (y1 * lw + x1, tx * ty)] {
                                let wt = bw / (1e-3 + (low_depth[q] - d).abs() / d);
                                sum += wt * low[q];
                                wsum += wt;
                            }
                            if wsum > 0.0 {
                                *o = sum / wsum;
                            }
                        }
                    }
                });
            }
        });
        let ms = started.elapsed().as_secs_f64() * 1e3;
        println!("{}", json!({"kind": "upsample", "shape": shape, "width": w, "height": h, "threads": threads, "run": run, "ms": round(ms), "check": out[w * h / 2]}));
    }
}

/// The scenes, smallest first: furnished rooms with more and denser props, several rooms side by
/// side, a grid of many small objects, and the heavy ones (a room with a dense sculpture, and
/// large terrains).
fn corpus() -> Vec<(&'static str, Scene)> {
    vec![
        ("room", testgen::interior_scene()),
        ("room-props-12k", furnished(3, 64)),
        ("room-props-50k", furnished(3, 128)),
        ("room-props-200k", furnished(3, 256)),
        ("room-props-500k", furnished(8, 256)),
        ("rooms-3", side_by_side(&furnished(3, 128), 3, 1)),
        ("rooms-2x2", side_by_side(&furnished(4, 128), 2, 2)),
        ("grid-30", testgen::grid_scene(30, false)),
        ("room-sculpture-1.5m", sculpture_room(866)),
        ("terrain-1m", testgen::large_scene(1_000_000)),
        ("terrain-2.1m", testgen::large_scene(2_100_000)),
    ]
}

/// The furnished room with `count` round props (cushions, pots, plants) of one shared sphere mesh
/// of `segments` around and half as many rings, about `segments^2` triangles each.
fn furnished(count: u32, segments: u32) -> Scene {
    let mut s = testgen::interior_scene();
    let mesh = s.meshes.len() as u32;
    s.meshes.push(testgen::sphere(segments, segments / 2));
    // Spots on the floor, the table and the sofa, round the room.
    let spots: [([f32; 3], f32); 8] = [
        ([-0.6, 0.62, -1.75], 0.16),
        ([0.6, 0.62, -1.75], 0.16),
        ([-0.3, 0.56, 0.1], 0.09),
        ([-2.4, 0.3, 1.8], 0.3),
        ([2.4, 0.3, -1.9], 0.3),
        ([1.65, 0.6, 0.85], 0.12),
        ([-2.3, 0.25, 0.4], 0.25),
        ([0.0, 0.2, 1.7], 0.2),
    ];
    for (i, (at, r)) in spots.iter().cycle().take(count as usize).enumerate() {
        let tone = [0.3 + 0.08 * i as f32, 0.5, 0.6 - 0.05 * i as f32];
        s.objects.push(object(&format!("Prop.{i:02}"), mesh, tone, placed(*at, [*r, *r, *r]), Some(false)));
    }
    s
}

/// The furnished room with one sculpture: a sphere of `segments` around and as many rings (about
/// `2 segments^2` triangles) on a plinth by the back wall.
fn sculpture_room(segments: u32) -> Scene {
    let mut s = furnished(3, 64);
    let mesh = s.meshes.len() as u32;
    s.meshes.push(testgen::sphere(segments, segments));
    s.objects.push(object("Sculpture", mesh, [0.8, 0.8, 0.75], placed([1.8, 0.6, -1.6], [0.5, 0.6, 0.5]), Some(false)));
    s
}

/// `room` repeated `nx` x `nz` times, 6.5 m apart in x and 5.5 m in z, as one scene.
fn side_by_side(room: &Scene, nx: u32, nz: u32) -> Scene {
    let mut s = Scene { up: room.up, meshes: Vec::new(), objects: Vec::new(), cameras: Vec::new(), cameras_left_out: Default::default() };
    for k in 0..nx * nz {
        let (dx, dz) = ((k % nx) as f32 * 6.5, (k / nx) as f32 * 5.5);
        let base = s.meshes.len() as u32;
        s.meshes.extend(room.meshes.iter().cloned());
        for o in &room.objects {
            let mut o = o.clone();
            o.id = format!("{}.{k}", o.id);
            o.mesh += base;
            o.transform[12] += dx;
            o.transform[14] += dz;
            s.objects.push(o);
        }
    }
    s
}
