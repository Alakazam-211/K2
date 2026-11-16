//! The worker's side of a request, also run in-process by the tests and the
//! fuzz targets: plan every view against the header (before any mesh data is
//! read), estimate the memory, then draw each view and encode it. Every
//! failure is a [`Failure`] with its [`FailureCode`].

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Instant;

use crate::camera::{self, Framing, ViewSpec, TURNTABLE_FRAMES};
use crate::contents::{self, MAX_LEFT_OUT_LISTED};
use crate::proto::{CameraMeta, FailureCode, FramingMeta, IdEntry, IdsMeta, ImageFormat, Request, Timings, ViewMeta, DEFAULT_ASPECT, DEFAULT_BACKGROUND, MAX_IDS_LISTED_BYTES};
use crate::raster::{self, BAND_PX};
use crate::sandbox::Caps;
use crate::encode;
use crate::scene::{self, Camera, Header, Object, Scene};

/// A typed failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: FailureCode,
    pub detail: String,
}

impl Failure {
    pub fn new(code: FailureCode, detail: impl Into<String>) -> Failure {
        Failure { code, detail: detail.into() }
    }

    fn input(e: &scene::InputError) -> Failure {
        Failure::new(FailureCode::from_input(e), e.to_string())
    }
}

/// One view, checked and sized.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub view: ViewSpec,
    pub active: Vec<u32>,
    pub width: u32,
    pub height: u32,
    pub aspect: f64,
    pub ssaa: u32,
    /// Drawn vertices and triangles (instancing expanded).
    pub vertices: u64,
    pub triangles: u64,
    pub without_modifiers: bool,
    /// RGBA (the request's `transparent`).
    pub transparent: bool,
    /// How an auto view is fitted (bounds for every other view).
    pub framing: Framing,
    /// Ambient occlusion (the request's `ao`): its buffers are in the estimate.
    pub ao: bool,
    /// The id image too (the request's `ids`): its buffers are in the estimate.
    pub ids: bool,
}

/// The images of a request.
#[derive(Debug)]
pub struct Output {
    pub format: ImageFormat,
    pub views: Vec<(ViewMeta, Vec<u8>)>,
    /// Each view's id image, in the same order, when the request asked for ids; else empty.
    pub ids: Vec<Vec<u8>>,
    pub objects_without_modifiers: Vec<u32>,
    /// The scene's objects hidden in the file.
    pub objects_hidden: Vec<u32>,
    /// The scene's objects drawn at their base transform.
    pub objects_not_evaluated: Vec<u32>,
}

/// The request's own checks (no scene needed): views, threads.
pub fn check_request(req: &Request, caps: &Caps) -> Result<(), Failure> {
    if req.views.is_empty() {
        return Err(Failure::new(FailureCode::InvalidInput, "the request has no views"));
    }
    if req.views.len() as u64 > caps.max_views as u64 {
        return Err(Failure::new(FailureCode::TooLarge, format!("{} views, at most {}", req.views.len(), caps.max_views)));
    }
    if req.input_len > caps.input.max_input_bytes {
        return Err(Failure::new(FailureCode::TooLarge, format!("input bytes: {}, at most {}", req.input_len, caps.input.max_input_bytes)));
    }
    if req.transparent && req.background.is_some() {
        return Err(Failure::new(FailureCode::InvalidInput, "a transparent image has no background"));
    }
    if let Some(ao) = &req.ao {
        ao.check().map_err(|m| Failure::new(FailureCode::InvalidInput, m))?;
    }
    Ok(())
}

/// Plan every view of `req` against a scene's header: object and camera
/// indices, sizes, SSAA, drawn counts. `mesh_counts` is (vertices,
/// triangles) per mesh. A view that names no objects draws every object not
/// hidden in the file; one that names them draws those, hidden or not.
pub fn plan(mesh_counts: &[(u64, u64)], objects: &[Object], cameras: &[Camera], req: &Request, caps: &Caps) -> Result<Vec<Plan>, Failure> {
    check_request(req, caps)?;
    let mut plans = Vec::with_capacity(req.views.len());
    for (vi, v) in req.views.iter().enumerate() {
        let bad = |m: String| Failure::new(FailureCode::InvalidInput, format!("view {vi}: {m}"));
        let big = |m: String| Failure::new(FailureCode::TooLarge, format!("view {vi}: {m}"));
        match v.view {
            ViewSpec::Camera(i) if i as usize >= cameras.len() => return Err(bad(format!("camera {i} doesn't exist ({} cameras)", cameras.len()))),
            ViewSpec::Turntable(k) if k >= TURNTABLE_FRAMES => return Err(bad(format!("turntable frame {k}: there are {TURNTABLE_FRAMES}"))),
            _ => {}
        }
        if v.framing == Framing::Contents && v.view != ViewSpec::Auto {
            return Err(bad("content framing is for the auto view only".into()));
        }
        let active: Vec<u32> = match &v.objects {
            Some(list) => {
                if list.len() > objects.len() {
                    return Err(bad(format!("{} objects listed, the scene has {}", list.len(), objects.len())));
                }
                if let Some(&o) = list.iter().find(|&&o| o as usize >= objects.len()) {
                    return Err(bad(format!("object {o} doesn't exist ({} objects)", objects.len())));
                }
                list.clone()
            }
            None => (0..objects.len() as u32).filter(|&o| objects[o as usize].hidden.is_none()).collect(),
        };
        let (mut dv, mut dt, mut without) = (0u64, 0u64, false);
        for &o in &active {
            let ob = &objects[o as usize];
            let (nv, nt) = mesh_counts.get(ob.mesh as usize).copied().unwrap_or((0, 0));
            dv += nv;
            dt += nt;
            without |= ob.modifiers_not_applied;
        }
        if dv > caps.input.max_drawn_vertices || dt > caps.input.max_drawn_triangles || dt > u32::MAX as u64 {
            return Err(big(format!("{dv} vertices and {dt} triangles drawn, at most {} and {}", caps.input.max_drawn_vertices, caps.input.max_drawn_triangles)));
        }
        let aspect = match (v.aspect, v.view) {
            (Some(a), _) => a as f64,
            (None, ViewSpec::Camera(i)) => cameras[i as usize].aspect.map(f64::from).unwrap_or(DEFAULT_ASPECT),
            (None, _) => DEFAULT_ASPECT,
        };
        if !(aspect.is_finite() && aspect > 0.0) {
            return Err(bad("the aspect must be positive".into()));
        }
        if v.width == 0 {
            return Err(bad("the width is 0".into()));
        }
        if v.width > caps.max_width {
            return Err(big(format!("{} px wide, at most {}", v.width, caps.max_width)));
        }
        let h = (v.width as f64 / aspect).round().max(1.0);
        if !(h.is_finite() && h <= caps.max_side as f64) || v.width > caps.max_side {
            return Err(big(format!("{} x {h} px, at most {} a side", v.width, caps.max_side)));
        }
        let height = h as u32;
        if v.width as u64 * height as u64 > caps.max_pixels {
            return Err(big(format!("{} x {height} px, at most {} pixels", v.width, caps.max_pixels)));
        }
        let ssaa = caps.ssaa_for(req.ssaa, v.width);
        if !(1..=4).contains(&ssaa) {
            return Err(bad(format!("ssaa {ssaa}: 1 to 4")));
        }
        if ssaa > caps.max_ssaa {
            return Err(big(format!("ssaa {ssaa}, at most {}", caps.max_ssaa)));
        }
        plans.push(Plan { view: v.view, active, width: v.width, height, aspect, ssaa, vertices: dv, triangles: dt, without_modifiers: without, transparent: req.transparent, framing: v.framing, ao: req.ao.is_some(), ids: req.ids });
    }
    Ok(plans)
}

/// (vertices, triangles) per mesh of a header.
pub fn header_counts(h: &Header) -> Vec<(u64, u64)> {
    h.meshes.iter().map(|m| (m.vertices as u64, m.triangles as u64)).collect()
}

/// (vertices, triangles) per mesh of a decoded scene.
pub fn scene_counts(s: &Scene) -> Vec<(u64, u64)> {
    s.meshes.iter().map(|m| (m.positions.len() as u64, m.indices.len() as u64)).collect()
}

/// The threads a request gets.
pub fn threads(req: &Request, caps: &Caps) -> u32 {
    req.threads.clamp(1, caps.max_threads.max(1))
}

/// Bytes the request will need at its peak, from the counts alone (checked
/// before the mesh data is read): the decoded data (the size of its bytes),
/// the header's parse (4x its JSON), the largest view's vertex stage (32 B
/// per drawn vertex) and band lists (10 B per drawn triangle), its image and
/// the encoder's copies (9 B per pixel, 12 transparent), with ambient
/// occlusion its depth image, raw occlusion and factors (12 B per pixel),
/// with ids the id image and its encoding ([`IDS_BYTES_PER_PX`]), each
/// thread's band buffers (16 B per sample, 20 with ids), every encoded image
/// kept until the response (4 B per pixel, 5 transparent, 3 more with ids;
/// at most the output cap), with ids the tables ([`IDS_TABLE_BYTES`]), and
/// 16 MiB of base.
pub fn estimate_bytes(data_bytes: u64, header_bytes: u64, plans: &[Plan], threads: u32, max_output: u64) -> u64 {
    let mut peak = 0u64;
    let mut out = 0u64;
    let mut tables = 0u64;
    for p in plans {
        let (w, h, s) = (p.width as u64, p.height as u64, p.ssaa as u64);
        let band = w * s * BAND_PX as u64 * s * if p.ids { 20 } else { 16 };
        let (image, kept) = if p.transparent { (12, 5) } else { (9, 4) };
        let image = if p.ao { image + AO_BYTES_PER_PX } else { image };
        let (image, kept) = if p.ids { (image + IDS_BYTES_PER_PX, kept + IDS_KEPT_PER_PX) } else { (image, kept) };
        let view = 32 * p.vertices + 10 * p.triangles + image * w * h + threads as u64 * band;
        peak = peak.max(view);
        out = out.saturating_add(kept * w * h);
        if p.ids {
            tables = IDS_TABLE_BYTES;
        }
    }
    data_bytes + 4 * header_bytes + peak + out.min(max_output) + tables + (16 << 20)
}

/// Ambient occlusion's own buffers, per pixel, at most: the depth image and the factors (an f32
/// each), and on the half grid its depth, raw and blurred occlusion (an f32 each per 4 pixels).
pub const AO_BYTES_PER_PX: u64 = 12;

/// The id image's own buffers, per pixel: the image (a u32), its RGB copy and the encoder's
/// filtered rows (3 B each).
pub const IDS_BYTES_PER_PX: u64 = 10;

/// An encoded id image kept until the response, per pixel, at most (PNG, 8-bit RGB).
pub const IDS_KEPT_PER_PX: u64 = 3;

/// The id tables of a request: what they hold in memory and in the response line (three times
/// [`MAX_IDS_LISTED_BYTES`]: each entry's id and its place in the list, then the line).
pub const IDS_TABLE_BYTES: u64 = 3 * MAX_IDS_LISTED_BYTES as u64;

/// The table of an id image (values: index + 1, 0 for nothing) of a scene with `objects`: how
/// many objects it shows, and those objects ascending by index, as many as fit `budget` bytes of
/// their JSON (each entry and a comma), the first that doesn't fit ending the list. Ids are
/// copied as they are; objects sharing an id are each listed under their own index.
pub fn id_table(objects: &[Object], ids: &[u32], budget: usize) -> (u64, Vec<IdEntry>) {
    let mut seen = vec![false; objects.len() + 1];
    for &v in ids {
        if let Some(s) = seen.get_mut(v as usize) {
            *s = true;
        }
    }
    let (mut count, mut used, mut open) = (0u64, 0usize, true);
    let mut listed = Vec::new();
    for (k, o) in objects.iter().enumerate() {
        if !seen[k + 1] {
            continue;
        }
        count += 1;
        if open {
            let e = IdEntry { index: k as u32, id: o.id.clone() };
            let len = serde_json::to_vec(&e).map(|j| j.len() + 1).unwrap_or(usize::MAX);
            if len <= budget - used {
                used += len;
                listed.push(e);
            } else {
                open = false;
            }
        }
    }
    (count, listed)
}

fn ms(since: Instant) -> f64 {
    (since.elapsed().as_secs_f64() * 1e6).round() / 1e3
}

/// Draw and encode every planned view of a decoded scene.
pub fn render_planned(scene: &Scene, plans: &[Plan], req: &Request, caps: &Caps) -> Result<Output, Failure> {
    let threads = threads(req, caps) as usize;
    let background = req.background.unwrap_or(DEFAULT_BACKGROUND);
    let mut views = Vec::with_capacity(plans.len());
    let mut ids_images = Vec::new();
    let mut total = 0u64;
    for (vi, p) in plans.iter().enumerate() {
        let bad_view = |e: String| Failure::new(FailureCode::InvalidInput, format!("view {vi}: {e}"));
        // The classes, once, when the framing or the shading needs them.
        let classes = (p.framing == Framing::Contents || req.clay.is_some()).then(|| contents::classify(scene, &p.active));
        let (frame, framing) = match (p.framing, &classes) {
            (Framing::Contents, Some(classes)) => {
                let (f, m) = camera::frame_contents(scene, &p.active, classes, p.aspect).map_err(bad_view)?;
                let count = m.left_out.len() as u64;
                let mut left_out = m.left_out;
                left_out.truncate(MAX_LEFT_OUT_LISTED);
                (f, Some(FramingMeta { contents: m.contents, left_out_count: count, left_out, grown: m.grown }))
            }
            _ => (camera::frame(scene, &p.active, p.view, p.aspect).map_err(bad_view)?, None),
        };
        let shell: Vec<bool> = match (&req.clay, &classes) {
            (Some(_), Some(c)) => (0..p.active.len()).map(|k| c.is_shell(k)).collect(),
            _ => Vec::new(),
        };
        let clay = req.clay.map(|c| raster::Clay { shell: &shell, shell_srgb: c.shell, contents_srgb: c.contents });
        // The radius is a fraction of what is drawn (its bounding sphere), so a world size.
        let ao = req.ao.map(|a| raster::Ao { radius: (a.radius as f64 * camera::bounding_sphere(scene, &p.active).1) as f32, strength: a.strength });
        let params = raster::Params { width: p.width, height: p.height, ssaa: p.ssaa, threads, background_srgb: background, transparent: p.transparent };
        let drawn = catch_unwind(AssertUnwindSafe(|| {
            if p.ids {
                let (img, ids, st) = raster::render_with_ids(scene, &p.active, &frame, &params, clay.as_ref(), ao.as_ref());
                (img, Some(ids), st)
            } else {
                let (img, st) = raster::render_shaded(scene, &p.active, &frame, &params, clay.as_ref(), ao.as_ref());
                (img, None, st)
            }
        }))
        .map_err(|_| Failure::new(FailureCode::RendererError, format!("view {vi}: the rasterizer panicked")))?;
        let (img, id_px, stats) = drawn;
        let t1 = Instant::now();
        let camera = CameraMeta { view: frame.view_f32(), proj: frame.projection(), fx: frame.fx, fy: frame.fy, near: frame.near, shift_x: frame.shift_x, shift_y: frame.shift_y };
        let bytes = encode::encode(&img, req.format).map_err(|e| Failure::new(FailureCode::RendererError, format!("view {vi}: {e}")))?;
        drop(img);
        total = total.saturating_add(bytes.len() as u64);
        if total > caps.max_output_bytes {
            return Err(Failure::new(FailureCode::TooLarge, format!("the images are over {} bytes", caps.max_output_bytes)));
        }
        let (ids_meta, ids_png) = match id_px {
            Some(px) => {
                let png = encode::ids_png(&px, p.width, p.height).map_err(|e| Failure::new(FailureCode::RendererError, format!("view {vi}: {e}")))?;
                total = total.saturating_add(png.len() as u64);
                if total > caps.max_output_bytes {
                    return Err(Failure::new(FailureCode::TooLarge, format!("the images and id images are over {} bytes", caps.max_output_bytes)));
                }
                // Each view gets an equal share of the response line's budget for the tables.
                let (count, objects) = id_table(&scene.objects, &px, MAX_IDS_LISTED_BYTES / plans.len().max(1));
                (Some(IdsMeta { image_len: png.len() as u64, count, objects }), Some(png))
            }
            None => (None, None),
        };
        let meta = ViewMeta {
            width: p.width,
            height: p.height,
            ssaa: p.ssaa,
            image_len: bytes.len() as u64,
            camera: Some(camera).filter(CameraMeta::is_finite),
            framing,
            ids: ids_meta,
            objects: stats.objects,
            triangles: stats.triangles,
            covered_px: stats.covered_px,
            without_modifiers: p.without_modifiers,
            base_values: p.active.iter().any(|&o| scene.objects[o as usize].transform_not_evaluated),
            hidden_left_out: hidden_left_out(&scene.objects, req.views.get(vi).is_some_and(|v| v.objects.is_some())),
            vertex_ms: stats.vertex_us as f64 / 1e3,
            bin_ms: stats.bin_us as f64 / 1e3,
            raster_ms: stats.raster_us as f64 / 1e3,
            encode_ms: ms(t1),
        };
        views.push((meta, bytes));
        ids_images.extend(ids_png);
    }
    let which = |f: fn(&Object) -> bool| -> Vec<u32> { scene.objects.iter().enumerate().filter(|(_, o)| f(o)).map(|(i, _)| i as u32).collect() };
    Ok(Output {
        format: req.format,
        views,
        ids: ids_images,
        objects_without_modifiers: which(|o| o.modifiers_not_applied),
        objects_hidden: which(|o| o.hidden.is_some()),
        objects_not_evaluated: which(|o| o.transform_not_evaluated),
    })
}

/// How many objects hidden in the file a view leaves out: all of them for a
/// view that names no objects, none for one that names them.
fn hidden_left_out(objects: &[Object], named: bool) -> u64 {
    if named {
        0
    } else {
        objects.iter().filter(|o| o.hidden.is_some()).count() as u64
    }
}

/// Decode `input` (a whole K2SCENE1) and draw every view of `req`: the
/// worker's path in one call, for the tests and the fuzz targets. Returns the
/// timings too.
pub fn render_bytes(input: &[u8], req: &Request, caps: &Caps) -> Result<(Output, Timings), Failure> {
    let t0 = Instant::now();
    check_request(req, caps)?;
    let mut d = scene::Decoder::new(input, input.len() as u64, caps.input);
    let h = d.header().map_err(|e| Failure::input(&e))?;
    let plans = plan(&header_counts(&h), &h.objects, &h.cameras, req, caps)?;
    let header_ms = ms(t0);
    let t1 = Instant::now();
    let scene = d.data(h).map_err(|e| Failure::input(&e))?;
    let data_ms = ms(t1);
    let t2 = Instant::now();
    let out = render_planned(&scene, &plans, req, caps)?;
    Ok((out, Timings { header_ms, data_ms, views_ms: ms(t2) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{Ao, Clay, View};
    use crate::scene::{encode, InputLimits};
    use crate::testgen;

    pub fn caps() -> Caps {
        Caps {
            max_width: 1920,
            max_side: 4096,
            max_pixels: 16 << 20,
            max_ssaa: 4,
            max_threads: 4,
            max_views: 64,
            max_output_bytes: 64 << 20,
            input: InputLimits::default(),
        }
    }

    #[test]
    fn several_views_in_one_request_give_the_bytes_of_one_view_each() {
        let input = encode(&testgen::camera_scene());
        let views = vec![
            View::new(ViewSpec::Auto, 96),
            View::new(ViewSpec::Turntable(3), 64),
            View::new(ViewSpec::Camera(0), 80),
            View { objects: Some(vec![0]), aspect: Some(1.0), ..View::new(ViewSpec::Auto, 50) },
        ];
        let all = render_bytes(&input, &Request::new(views.clone()), &caps()).expect("all").0;
        assert_eq!(all.views.len(), 4);
        for (i, v) in views.into_iter().enumerate() {
            let one = render_bytes(&input, &Request::new(vec![v]), &caps()).expect("one").0;
            assert_eq!(one.views[0].1, all.views[i].1, "view {i}");
            assert_eq!(one.views[0].0.height, all.views[i].0.height);
        }
        assert_eq!(all.views[2].0.height, 45, "the camera's 16:9");
        assert_eq!(all.views[3].0.height, 50, "the view's 1:1");
    }

    #[test]
    fn modifiers_not_applied_pass_through_per_object_and_per_view() {
        let mut s = testgen::grid_scene(2, true);
        s.objects[1].modifiers = vec!["SUBSURF".into()];
        s.objects[1].modifiers_not_applied = true;
        s.objects[2].modifiers = vec!["BEVEL".into()];
        let input = encode(&s);
        let views = vec![
            View::new(ViewSpec::Auto, 32),
            View { objects: Some(vec![0]), ..View::new(ViewSpec::Auto, 32) },
            View { objects: Some(vec![1]), ..View::new(ViewSpec::Auto, 32) },
        ];
        let out = render_bytes(&input, &Request::new(views), &caps()).expect("render").0;
        assert_eq!(out.objects_without_modifiers, vec![1]);
        let flags: Vec<bool> = out.views.iter().map(|v| v.0.without_modifiers).collect();
        assert_eq!(flags, [true, false, true]);
    }

    /// Four objects in a 2 x 2 grid, a small one (index 2) on top of the first
    /// and inside their box, and one far away (index 5); the camera scene's
    /// camera, turned to look at the small one.
    fn hidden_scene() -> crate::scene::Scene {
        let mut s = testgen::grid_scene(2, false);
        s.cameras = testgen::camera_scene().cameras;
        s.cameras[0].target = Some([0.0, 0.75, 0.0]);
        s.objects.insert(2, testgen::object("Top", 0, [0.9, 0.2, 0.2], testgen::placed([0.0, 0.75, 0.0], [0.4, 0.4, 0.4]), None));
        s.objects.push(testgen::object("Far", 0, [0.2, 0.2, 0.9], testgen::placed([100.0, 0.0, 0.0], [1.0, 1.0, 1.0]), None));
        s
    }

    fn with_ids(views: Vec<View>) -> Request {
        let mut r = Request::new(views);
        r.ids = true;
        r
    }

    /// The scene, a camera and a turntable frame leave a hidden object out:
    /// a far one doesn't widen the fit (the same bytes, id images and cameras
    /// as the scene without it), and a hidden one in the middle is never in
    /// the id image while every other object keeps its index + 1 wherever the
    /// hidden one didn't cover it when shown.
    #[test]
    fn scene_camera_and_turntable_views_leave_hidden_objects_out() {
        let views = vec![View::new(ViewSpec::Auto, 96), View::new(ViewSpec::Camera(0), 96), View::new(ViewSpec::Turntable(2), 96), View { framing: Framing::Contents, ..View::new(ViewSpec::Auto, 96) }];
        let mut far_hidden = hidden_scene();
        far_hidden.objects[5].hidden = Some(crate::scene::Hidden::Itself);
        let mut without_far = hidden_scene();
        without_far.objects.pop();
        let req = with_ids(views.clone());
        let (h, w) = (render_bytes(&encode(&far_hidden), &req, &caps()).expect("far hidden").0, render_bytes(&encode(&without_far), &req, &caps()).expect("without far").0);
        assert!(h.ids == w.ids, "the same id images");
        for (i, (a, b)) in h.views.iter().zip(&w.views).enumerate() {
            assert!(a.1 == b.1, "view {i}: the same still");
            assert_eq!((a.0.camera, a.0.objects, &a.0.framing, &a.0.ids), (b.0.camera, b.0.objects, &b.0.framing, &b.0.ids), "view {i}");
            assert_eq!((a.0.hidden_left_out, b.0.hidden_left_out), (1, 0), "view {i}");
        }
        assert_eq!((h.objects_hidden.clone(), w.objects_hidden.clone()), (vec![5], vec![]));
        // The small one, hidden too ("collection"): never in the id image, the rest unmoved.
        let mut both = far_hidden.clone();
        both.objects[2].hidden = Some(crate::scene::Hidden::Collection);
        let req = with_ids(views[..3].to_vec());
        let (shown, hidden) = (render_bytes(&encode(&far_hidden), &req, &caps()).expect("small shown").0, render_bytes(&encode(&both), &req, &caps()).expect("small hidden").0);
        for i in 0..3 {
            let (sm, hm) = (&shown.views[i].0, &hidden.views[i].0);
            assert_eq!(sm.camera, hm.camera, "view {i}: inside the others' box, the fit is the same");
            assert_eq!((sm.objects, hm.objects, hm.hidden_left_out), (5, 4, 2), "view {i}");
            let (sp, sset) = id_values(&shown.ids[i], sm.width, sm.height);
            let (hp, hset) = id_values(&hidden.ids[i], hm.width, hm.height);
            assert!(sset.contains(&3), "view {i}: shown, the small object is in the image");
            assert!(!hset.contains(&3) && !hset.contains(&6), "view {i}: hidden, never");
            assert!(!hset.is_empty() && hset.iter().all(|v| [1, 2, 4, 5].contains(v)), "view {i}: {hset:?}");
            assert!(sp.iter().zip(&hp).all(|(s, h)| *s == 3 || s == h), "view {i}: every other pixel keeps its value");
            assert_eq!(hm.ids.as_ref().map(|m| m.objects.iter().map(|e| e.index).collect::<Vec<_>>()), Some(hset.iter().map(|v| v - 1).collect()), "view {i}: the table lists only objects with pixels");
        }
        // Hiding every object: nothing drawn, and no failure.
        let mut all = hidden_scene();
        for o in all.objects.iter_mut() {
            o.hidden = Some(crate::scene::Hidden::Itself);
        }
        let none = render_bytes(&encode(&all), &with_ids(views.clone()), &caps()).expect("all hidden").0;
        assert!(none.views.iter().all(|v| v.0.objects == 0 && v.0.covered_px == 0 && v.0.hidden_left_out == 6));
    }

    /// A view that names a hidden object draws it alone, with its index in
    /// the id image: the same bytes as when it is shown.
    #[test]
    fn an_object_view_draws_a_hidden_object() {
        let mut s = hidden_scene();
        let one = with_ids(vec![View { objects: Some(vec![2]), ..View::new(ViewSpec::Auto, 64) }]);
        let shown = render_bytes(&encode(&s), &one, &caps()).expect("shown").0;
        for how in [crate::scene::Hidden::Itself, crate::scene::Hidden::Collection] {
            s.objects[2].hidden = Some(how);
            let o = render_bytes(&encode(&s), &one, &caps()).expect("hidden").0;
            let m = &o.views[0].0;
            assert!(m.covered_px > 0 && m.objects == 1 && m.hidden_left_out == 0, "{m:?}");
            let (_, set) = id_values(&o.ids[0], m.width, m.height);
            assert_eq!(set.into_iter().collect::<Vec<_>>(), [3]);
            assert!(o.views[0].1 == shown.views[0].1 && o.ids == shown.ids, "{how:?}");
        }
    }

    /// An object whose placement isn't evaluated is drawn at its base
    /// transform: the same pixels, flagged on each view that draws it.
    #[test]
    fn transform_not_evaluated_draws_the_same_and_says_base_values() {
        let a = hidden_scene();
        let mut b = a.clone();
        b.objects[1].transform_not_evaluated = true;
        let views = vec![View::new(ViewSpec::Auto, 64), View::new(ViewSpec::Camera(0), 64), View::new(ViewSpec::Turntable(4), 64), View { objects: Some(vec![1]), ..View::new(ViewSpec::Auto, 64) }, View { objects: Some(vec![0]), ..View::new(ViewSpec::Auto, 64) }];
        let (ra, rb) = (render_bytes(&encode(&a), &with_ids(views.clone()), &caps()).expect("a").0, render_bytes(&encode(&b), &with_ids(views), &caps()).expect("b").0);
        assert!(ra.ids == rb.ids);
        for (x, y) in ra.views.iter().zip(&rb.views) {
            assert!(x.1 == y.1);
            assert!(!x.0.base_values);
        }
        assert_eq!(rb.views.iter().map(|v| v.0.base_values).collect::<Vec<_>>(), [true, true, true, true, false]);
        assert_eq!((ra.objects_not_evaluated.clone(), rb.objects_not_evaluated.clone()), (vec![], vec![1]));
        // Hidden, it is no longer drawn by the scene: no longer flagged there.
        let mut c = b.clone();
        c.objects[1].hidden = Some(crate::scene::Hidden::Itself);
        let rc = render_bytes(&encode(&c), &Request::new(vec![View::new(ViewSpec::Auto, 32), View { objects: Some(vec![1]), ..View::new(ViewSpec::Auto, 32) }]), &caps()).expect("c").0;
        assert_eq!(rc.views.iter().map(|v| v.0.base_values).collect::<Vec<_>>(), [false, true]);
        // Off the response line when there is nothing to say; on it, they round-trip.
        let line = |r: &Output| -> Vec<u8> {
            crate::proto::Response { ok: true, code: None, detail: None, format: Some(ImageFormat::Webp), views: Some(r.views.iter().map(|v| v.0.clone()).collect()), ..crate::proto::Response::failed(FailureCode::TooLarge, "") }.line()
        };
        let la = line(&ra);
        let text = String::from_utf8_lossy(&la);
        assert!(!text.contains("base_values") && !text.contains("hidden"), "{text}");
        let lc = line(&rc);
        assert!(String::from_utf8_lossy(&lc).contains(r#""base_values":true"#));
        let back = crate::proto::Response::parse(&lc[..lc.len() - 1]).expect("parse").views.expect("views");
        assert_eq!(back.iter().map(|v| (v.base_values, v.hidden_left_out)).collect::<Vec<_>>(), [(false, 1), (true, 0)]);
    }

    #[test]
    fn bad_views_are_typed_before_any_drawing() {
        let input = encode(&testgen::cube_scene());
        let c = caps();
        let code = |views: Vec<View>| render_bytes(&input, &Request::new(views), &c).expect_err("refused").code;
        assert_eq!(code(vec![]), FailureCode::InvalidInput);
        assert_eq!(code(vec![View::new(ViewSpec::Camera(0), 32)]), FailureCode::InvalidInput, "no camera");
        assert_eq!(code(vec![View::new(ViewSpec::Turntable(8), 32)]), FailureCode::InvalidInput);
        assert_eq!(code(vec![View { objects: Some(vec![5]), ..View::new(ViewSpec::Auto, 32) }]), FailureCode::InvalidInput);
        assert_eq!(code(vec![View::new(ViewSpec::Auto, 0)]), FailureCode::InvalidInput);
        assert_eq!(code(vec![View::new(ViewSpec::Auto, 4000)]), FailureCode::TooLarge);
        assert_eq!(code(vec![View { aspect: Some(0.01), ..View::new(ViewSpec::Auto, 1000) }]), FailureCode::TooLarge);
        assert_eq!(code(vec![View { aspect: Some(f32::NAN), ..View::new(ViewSpec::Auto, 10) }]), FailureCode::InvalidInput);
        // Content framing is for the auto view: refused on a turntable frame or a camera.
        let contents = |spec: ViewSpec| View { framing: Framing::Contents, ..View::new(spec, 32) };
        assert_eq!(code(vec![contents(ViewSpec::Turntable(2))]), FailureCode::InvalidInput);
        assert_eq!(render_bytes(&encode(&testgen::camera_scene()), &Request::new(vec![contents(ViewSpec::Camera(0))]), &c).expect_err("camera").code, FailureCode::InvalidInput);
        assert!(render_bytes(&input, &Request::new(vec![contents(ViewSpec::Auto)]), &c).is_ok());
        assert_eq!(code(vec![View::new(ViewSpec::Auto, 8); 65]), FailureCode::TooLarge, "too many views");
        let mut r = Request::new(vec![View::new(ViewSpec::Auto, 32)]);
        r.ssaa = Some(5);
        assert_eq!(render_bytes(&input, &r, &c).expect_err("ssaa").code, FailureCode::InvalidInput);
        let tight = Caps { max_ssaa: 2, ..c };
        r.ssaa = Some(4);
        assert_eq!(render_bytes(&input, &r, &tight).expect_err("ssaa cap").code, FailureCode::TooLarge);
        let small_out = Caps { max_output_bytes: 10, ..c };
        assert_eq!(render_bytes(&input, &Request::new(vec![View::new(ViewSpec::Auto, 32)]), &small_out).expect_err("out").code, FailureCode::TooLarge);
        let mut t = Request::new(vec![View::new(ViewSpec::Auto, 32)]);
        t.transparent = true;
        assert!(render_bytes(&input, &t, &c).is_ok());
        assert_eq!(render_bytes(&input, &t, &small_out).expect_err("out").code, FailureCode::TooLarge, "the output cap holds for RGBA");
        t.background = Some([1, 2, 3]);
        assert_eq!(render_bytes(&input, &t, &c).expect_err("both").code, FailureCode::InvalidInput, "transparent with a background");
    }

    /// A camera's display fields (active, lens, sensor, fit) draw nothing: the
    /// same digests and the same bytes with or without them.
    #[test]
    fn camera_display_fields_change_no_digest_and_no_pixel() {
        let a = testgen::camera_scene();
        let mut b = a.clone();
        b.cameras[0].active = Some(true);
        b.cameras[0].lens_mm = Some(18.0);
        b.cameras[0].sensor_mm = Some([23.5, 15.6]);
        b.cameras[0].sensor_fit = Some(crate::scene::SensorFit::Vertical);
        let views = vec![View::new(ViewSpec::Camera(0), 160), View::new(ViewSpec::Auto, 96), View::new(ViewSpec::Turntable(2), 64)];
        let (ia, ib) = (encode(&a), encode(&b));
        let digests = |x: &[u8]| -> Vec<[u8; 32]> { crate::digest::list(x, &InputLimits::default()).expect("list").iter().map(|i| i.digest).collect() };
        assert_eq!(digests(&ia), digests(&ib));
        let (ra, rb) = (render_bytes(&ia, &Request::new(views.clone()), &caps()).expect("a").0, render_bytes(&ib, &Request::new(views), &caps()).expect("b").0);
        for (x, y) in ra.views.iter().zip(&rb.views) {
            assert_eq!(x.1, y.1);
            assert!(x.0.camera.is_some() && x.0.camera == y.0.camera);
        }
    }

    /// A portrait sensor with `sensor_fit: "auto"` draws with exactly the
    /// camera's `yfov` (fy = 1 / tan(yfov / 2)): the lens and sensor never
    /// re-derive the field of view, so the bytes are the same camera's without
    /// display fields.
    #[test]
    fn a_portrait_sensor_on_auto_fit_draws_with_exactly_its_yfov() {
        let mut plain = testgen::camera_scene();
        // What a pack would write for 50 mm on a 24 x 36 mm sensor, fitted to its longer side.
        plain.cameras[0].yfov = (2.0 * (18.0f64 / 50.0).atan()) as f32;
        plain.cameras[0].aspect = Some(2.0 / 3.0);
        let mut portrait = plain.clone();
        portrait.cameras[0].lens_mm = Some(50.0);
        portrait.cameras[0].sensor_mm = Some([24.0, 36.0]);
        portrait.cameras[0].sensor_fit = Some(crate::scene::SensorFit::Auto);
        let yfov = portrait.cameras[0].yfov;
        let want = (1.0 / crate::trig::tan(yfov as f64 * 0.5)) as f32;
        assert!((want as f64 - 1.0 / (yfov as f64 * 0.5).tan()).abs() < 1e-5, "our tan is tan");
        let f = crate::camera::frame(&portrait, &[0], ViewSpec::Camera(0), 2.0 / 3.0).expect("frame");
        assert_eq!(f.fy.to_bits(), want.to_bits());
        assert_eq!(f, crate::camera::frame(&plain, &[0], ViewSpec::Camera(0), 2.0 / 3.0).expect("frame"));
        let views = vec![View::new(ViewSpec::Camera(0), 120)];
        let a = render_bytes(&encode(&portrait), &Request::new(views.clone()), &caps()).expect("portrait").0;
        let b = render_bytes(&encode(&plain), &Request::new(views), &caps()).expect("plain").0;
        assert_eq!((a.views[0].0.width, a.views[0].0.height), (120, 180));
        assert_eq!(a.views[0].0.camera.expect("a camera").fy.to_bits(), want.to_bits());
        assert_eq!(a.views[0].1, b.views[0].1);
    }

    /// Asking for bounds framing is asking for nothing: the same bytes and the
    /// same meta as a view without the field, and no framing meta.
    #[test]
    fn bounds_framing_is_the_auto_view_as_it_was() {
        let input = encode(&testgen::interior_scene());
        let plain = render_bytes(&input, &Request::new(vec![View::new(ViewSpec::Auto, 96)]), &caps()).expect("plain").0;
        let bounds = render_bytes(&input, &Request::new(vec![View { framing: Framing::Bounds, ..View::new(ViewSpec::Auto, 96) }]), &caps()).expect("bounds").0;
        assert_eq!(plain.views[0].1, bounds.views[0].1);
        assert_eq!(plain.views[0].0.camera, bounds.views[0].0.camera);
        assert_eq!(plain.views[0].0.framing, None);
        let cam = plain.views[0].0.camera.expect("a camera");
        assert_eq!((cam.shift_x.to_bits(), cam.shift_y.to_bits()), (0, 0));
    }

    /// A content-framed view draws every object (the shell too), reports what
    /// it left out of the frame, and falls back to the bounds framing (the
    /// same bytes) when nothing is contents.
    #[test]
    fn content_framing_draws_everything_and_says_what_it_left_out() {
        let contents = |w: u32| View { framing: Framing::Contents, ..View::new(ViewSpec::Auto, w) };
        let input = encode(&testgen::interior_scene());
        let (c, b) = (
            render_bytes(&input, &Request::new(vec![contents(96)]), &caps()).expect("contents").0,
            render_bytes(&input, &Request::new(vec![View::new(ViewSpec::Auto, 96)]), &caps()).expect("bounds").0,
        );
        let (cm, bm) = (&c.views[0].0, &b.views[0].0);
        assert_ne!(c.views[0].1, b.views[0].1);
        assert_eq!(cm.objects, bm.objects, "every object drawn");
        assert_eq!(cm.framing, Some(FramingMeta { contents: true, left_out_count: 4, left_out: vec![0, 1, 2, 3], grown: false }));
        assert!(cm.covered_px > bm.covered_px, "closer in: more of the image covered");
        let cam = cm.camera.expect("a camera");
        assert!(cam.shift_x != 0.0 && cam.shift_y != 0.0);
        // One small object: framed, with the room around it.
        let small = render_bytes(&encode(&testgen::small_object_scene()), &Request::new(vec![contents(64)]), &caps()).expect("small").0;
        assert_eq!(small.views[0].0.framing.as_ref().map(|f| (f.contents, f.grown, f.left_out.len())), Some((true, true, 4)));
        // All shell, and no objects: the bounds framing's bytes, flagged.
        for s in [testgen::empty_room_scene(), crate::scene::Scene { objects: Vec::new(), ..testgen::empty_room_scene() }] {
            let input = encode(&s);
            let c = render_bytes(&input, &Request::new(vec![contents(64)]), &caps()).expect("fallback").0;
            let b = render_bytes(&input, &Request::new(vec![View::new(ViewSpec::Auto, 64)]), &caps()).expect("bounds").0;
            assert_eq!(c.views[0].1, b.views[0].1);
            assert_eq!(c.views[0].0.camera, b.views[0].0.camera);
            assert_eq!(c.views[0].0.framing, Some(FramingMeta { contents: false, left_out_count: 0, left_out: vec![], grown: false }));
        }
        // An object view asking for it frames that object's own contents.
        let one = View { objects: Some(vec![5]), ..contents(64) };
        let o = render_bytes(&input, &Request::new(vec![one]), &caps()).expect("one object").0;
        assert_eq!(o.views[0].0.objects, 1);
        assert_eq!(o.views[0].0.framing.as_ref().map(|f| f.contents), Some(true));
    }

    /// The scene with each object's base colour set to the linear value of its
    /// class's clay tone: what clay shading must draw, byte for byte.
    fn painted(s: &crate::scene::Scene, clay: Clay) -> crate::scene::Scene {
        let all: Vec<u32> = (0..s.objects.len() as u32).collect();
        let c = contents::classify(s, &all);
        let mut p = s.clone();
        for (k, o) in p.objects.iter_mut().enumerate() {
            let t = if c.is_shell(k) { clay.shell } else { clay.contents };
            o.base_color = [crate::srgb::to_linear(t[0]), crate::srgb::to_linear(t[1]), crate::srgb::to_linear(t[2]), 1.0];
        }
        p
    }

    /// Clay shading swaps each object's colour for its class's tone and
    /// changes nothing else: the same bytes as the scene painted in those
    /// tones, opaque and transparent (the same coverage alpha), on every
    /// kind of view, with the default tones and with the request's own.
    #[test]
    fn clay_is_the_scene_painted_in_two_tones() {
        let views = vec![
            View::new(ViewSpec::Auto, 96),
            View { framing: Framing::Contents, ..View::new(ViewSpec::Auto, 96) },
            View::new(ViewSpec::Turntable(3), 64),
            View { objects: Some(vec![0, 5, 6]), ..View::new(ViewSpec::Auto, 64) },
        ];
        let s = testgen::interior_scene();
        for tones in [Clay::default(), Clay { shell: [60, 70, 80], contents: [250, 240, 200] }] {
            for transparent in [false, true] {
                let mut req = Request::new(views.clone());
                req.transparent = transparent;
                let plain = render_bytes(&encode(&painted(&s, tones)), &req, &caps()).expect("painted").0;
                req.clay = Some(tones);
                let clay = render_bytes(&encode(&s), &req, &caps()).expect("clay").0;
                for (i, (a, b)) in plain.views.iter().zip(&clay.views).enumerate() {
                    assert!(a.1 == b.1, "view {i}, transparent {transparent}, {tones:?}");
                    assert_eq!((a.0.covered_px, a.0.camera, &a.0.framing), (b.0.covered_px, b.0.camera, &b.0.framing));
                }
            }
        }
        // (The object view classes its own three objects, the floor, the sofa and the table:
        // the same classes as in the whole scene, so the painted scene matches it too.)
        // Clay changes every image.
        let base = render_bytes(&encode(&s), &Request::new(views.clone()), &caps()).expect("base").0;
        let mut req = Request::new(views);
        req.clay = Some(Clay::default());
        let clay = render_bytes(&encode(&s), &req, &caps()).expect("clay").0;
        assert!(base.views.iter().zip(&clay.views).all(|(a, b)| a.1 != b.1));
    }

    /// With clay off, nothing reads the classes: a request without it draws
    /// the bytes it drew before clay existed (the checked-in digests hold
    /// that), and naming the default tones is the same as `{}`.
    #[test]
    fn clay_off_draws_base_colours_and_default_tones_are_the_default() {
        let input = encode(&testgen::interior_scene());
        let views = vec![View::new(ViewSpec::Auto, 80)];
        let mut a = Request::new(views.clone());
        a.clay = Some(Clay::default());
        let mut b = Request::new(views.clone());
        b.clay = Some(Clay { shell: crate::proto::CLAY_SHELL, contents: crate::proto::CLAY_CONTENTS });
        assert_eq!(render_bytes(&input, &a, &caps()).expect("a").0.views[0].1, render_bytes(&input, &b, &caps()).expect("b").0.views[0].1);
        let off = render_bytes(&input, &Request::new(views), &caps()).expect("off").0;
        let s = testgen::interior_scene();
        let all: Vec<u32> = (0..s.objects.len() as u32).collect();
        let f = camera::frame(&s, &all, ViewSpec::Auto, DEFAULT_ASPECT).expect("frame");
        let p = raster::Params { width: 80, height: 60, ssaa: 4, threads: 1, background_srgb: DEFAULT_BACKGROUND, transparent: false };
        let (img, _) = raster::render(&s, &all, &f, &p);
        assert_eq!(off.views[0].1, crate::encode::encode(&img, ImageFormat::Webp).expect("webp"), "the base-colour rasterizer's bytes");
    }

    /// Ambient occlusion through a request: strength 0 is the plain still
    /// byte for byte (opaque and transparent, clay or not, any view); the
    /// default changes each still, the same bytes for any thread count; out
    /// of range values are refused before any drawing.
    #[test]
    fn ambient_occlusion_is_the_requests_and_strength_zero_is_none() {
        let input = encode(&testgen::interior_scene());
        let views = vec![View::new(ViewSpec::Auto, 96), View { framing: Framing::Contents, ..View::new(ViewSpec::Auto, 96) }, View::new(ViewSpec::Turntable(5), 64)];
        for clay in [None, Some(Clay::default())] {
            for transparent in [false, true] {
                let mut req = Request::new(views.clone());
                req.transparent = transparent;
                req.clay = clay;
                let plain = render_bytes(&input, &req, &caps()).expect("plain").0;
                req.ao = Some(Ao { strength: 0.0, ..Ao::default() });
                let zero = render_bytes(&input, &req, &caps()).expect("zero").0;
                req.ao = Some(Ao::default());
                req.threads = 1;
                let ao = render_bytes(&input, &req, &caps()).expect("ao").0;
                req.threads = 4;
                let ao4 = render_bytes(&input, &req, &caps()).expect("ao, 4 threads").0;
                for i in 0..views.len() {
                    let what = format!("view {i}, clay {}, transparent {transparent}", clay.is_some());
                    assert!(plain.views[i].1 == zero.views[i].1, "{what}");
                    assert!(plain.views[i].1 != ao.views[i].1, "{what}");
                    assert!(ao.views[i].1 == ao4.views[i].1, "{what}");
                    assert_eq!((plain.views[i].0.covered_px, plain.views[i].0.camera), (ao.views[i].0.covered_px, ao.views[i].0.camera), "{what}");
                }
            }
        }
        for bad in [Ao { strength: 1.01, ..Ao::default() }, Ao { radius: 0.0, ..Ao::default() }, Ao { radius: f32::NAN, ..Ao::default() }] {
            let mut req = Request::new(views.clone());
            req.ao = Some(bad);
            assert_eq!(render_bytes(&input, &req, &caps()).expect_err("refused").code, FailureCode::InvalidInput, "{bad:?}");
        }
    }

    /// The id image's values and the set of them (0 left out).
    fn id_values(png: &[u8], w: u32, h: u32) -> (Vec<u32>, std::collections::BTreeSet<u32>) {
        let (iw, ih, px) = crate::encode::ids_from_png(png).expect("an id png");
        assert_eq!((iw, ih), (w, h), "the still's size");
        let set = px.iter().copied().filter(|&v| v != 0).collect();
        (px, set)
    }

    /// Ids through a request: every view gets its id image after its image,
    /// at the still's size, with a table of exactly the objects in it, in
    /// every look; the stills are the bytes drawn without ids; an object view
    /// shows only its objects; any thread count draws the same id image.
    #[test]
    fn ids_come_with_their_table_and_leave_the_stills_alone() {
        let s = testgen::interior_scene();
        let input = encode(&s);
        let views = vec![
            View::new(ViewSpec::Auto, 96),
            View { framing: Framing::Contents, ..View::new(ViewSpec::Auto, 96) },
            View::new(ViewSpec::Turntable(3), 64),
            View { objects: Some(vec![0, 5, 6]), ..View::new(ViewSpec::Auto, 64) },
            View { objects: Some(vec![9]), ..View::new(ViewSpec::Auto, 48) },
        ];
        let mut first: Option<Vec<Vec<u8>>> = None;
        for (clay, ao) in [(None, None), (Some(Clay::default()), None), (Some(Clay::default()), Some(Ao::default()))] {
            for transparent in [false, true] {
                let mut req = Request::new(views.clone());
                req.transparent = transparent;
                req.clay = clay;
                req.ao = ao;
                let plain = render_bytes(&input, &req, &caps()).expect("plain").0;
                assert!(plain.ids.is_empty() && plain.views.iter().all(|v| v.0.ids.is_none()));
                req.ids = true;
                let with = render_bytes(&input, &req, &caps()).expect("ids").0;
                assert_eq!(with.ids.len(), views.len());
                for (i, ((a, b), png)) in plain.views.iter().zip(&with.views).zip(&with.ids).enumerate() {
                    let what = format!("view {i}, clay {}, ao {}, transparent {transparent}", clay.is_some(), ao.is_some());
                    assert!(a.1 == b.1, "{what}: the still is the same");
                    let m = b.0.ids.as_ref().expect("the table");
                    assert_eq!(m.image_len, png.len() as u64);
                    let (_, set) = id_values(png, b.0.width, b.0.height);
                    let listed: Vec<u32> = m.objects.iter().map(|e| e.index + 1).collect();
                    assert_eq!(listed, set.iter().copied().collect::<Vec<_>>(), "{what}: the table is the objects in the image, ascending");
                    assert_eq!(m.count, set.len() as u64);
                    assert!(m.objects.iter().all(|e| e.id == s.objects[e.index as usize].id), "{what}");
                    if let Some(only) = &views[i].objects {
                        assert!(m.objects.iter().all(|e| only.contains(&e.index)), "{what}: {:?}", m.objects);
                    }
                }
                assert!(with.views[0].0.ids.as_ref().expect("ids").count >= 8);
                assert_eq!(with.views[4].0.ids.as_ref().map(|m| (m.count, m.objects[0].index)), Some((1, 9)), "the object view: that object alone");
                // The id images don't depend on the look.
                match &first {
                    None => first = Some(with.ids.clone()),
                    Some(f) => assert!(*f == with.ids),
                }
                req.threads = 4;
                assert!(render_bytes(&input, &req, &caps()).expect("4 threads").0.ids == with.ids);
            }
        }
    }

    /// Renaming objects changes the table, never the id image or the still;
    /// ids are kept byte for byte (an IFC GlobalId, case, spaces), and objects
    /// sharing an id are each listed under their own index.
    #[test]
    fn a_rename_changes_the_table_not_the_id_image() {
        let a = testgen::interior_scene();
        let mut b = a.clone();
        let global = "2O2Fr$t4X7Zf8NOew3FLOH";
        b.objects[0].id = " Floor ".into();
        b.objects[4].id = global.into();
        b.objects[5].id = global.into();
        b.objects[6].id = global.to_lowercase();
        b.objects[7].id = "Vase \"\u{e9}\"\\".into();
        let views = vec![View { framing: Framing::Contents, ..View::new(ViewSpec::Auto, 120) }, View::new(ViewSpec::Auto, 80)];
        let mut req = Request::new(views);
        req.ids = true;
        req.clay = Some(Clay::default());
        let (ra, rb) = (render_bytes(&encode(&a), &req, &caps()).expect("a").0, render_bytes(&encode(&b), &req, &caps()).expect("b").0);
        assert_eq!(ra.ids, rb.ids);
        for (x, y) in ra.views.iter().zip(&rb.views) {
            assert_eq!(x.1, y.1);
            let (tx, ty) = (x.0.ids.as_ref().expect("a"), y.0.ids.as_ref().expect("b"));
            assert_ne!(tx, ty);
            assert_eq!(tx.objects.iter().map(|e| e.index).collect::<Vec<_>>(), ty.objects.iter().map(|e| e.index).collect::<Vec<_>>());
            let id = |k: u32| ty.objects.iter().find(|e| e.index == k).map(|e| e.id.as_str());
            assert_eq!((id(4), id(5)), (Some(global), Some(global)), "a shared id, under each index");
            assert_eq!(id(6), Some("2o2fr$t4x7zf8noew3floh"));
            assert_eq!(id(0), Some(" Floor "));
            assert_eq!(id(7), Some("Vase \"\u{e9}\"\\"));
        }
        // And through the response line, byte for byte.
        let line = crate::proto::Response { ok: true, code: None, detail: None, format: Some(ImageFormat::Webp), views: Some(rb.views.iter().map(|v| v.0.clone()).collect()), ..crate::proto::Response::failed(FailureCode::TooLarge, "") }.line();
        let back = crate::proto::Response::parse(&line[..line.len() - 1]).expect("parse").views.expect("views");
        assert_eq!(back[0].ids, rb.views[0].0.ids);
    }

    /// The table lists in index order while the entries' JSON fits the
    /// budget, and counts every object; a scene of long ids keeps the
    /// response line far inside its cap.
    #[test]
    fn the_id_table_is_bounded_and_counts_everything() {
        let s = testgen::grid_scene(4, false);
        let px: Vec<u32> = vec![0, 3, 3, 1, 16, 9, 0, 9];
        let (count, all) = id_table(&s.objects, &px, usize::MAX);
        assert_eq!(count, 4);
        assert_eq!(all.iter().map(|e| e.index).collect::<Vec<_>>(), [0, 2, 8, 15]);
        // `{"index":0,"id":"Item.000"}` and a comma: 28 bytes (29 for index 15).
        assert_eq!(serde_json::to_vec(&all[0]).expect("json").len() + 1, 28);
        for (budget, n) in [(0, 0), (27, 0), (28, 1), (55, 1), (56, 2), (28 * 3 + 28, 3), (28 * 3 + 29, 4)] {
            let (c, l) = id_table(&s.objects, &px, budget);
            assert_eq!((c, l.len()), (4, n), "budget {budget}");
            assert_eq!(l[..], all[..n]);
        }
        assert_eq!(id_table(&s.objects, &[0; 10], usize::MAX), (0, vec![]));
        // 2,500 objects with 1 KiB ids of control characters (6 bytes each in JSON): cut short at
        // the budget, everything counted, the line well inside its cap.
        let mut big = testgen::grid_scene(50, false);
        for (k, o) in big.objects.iter_mut().enumerate() {
            o.id = format!("{k:04}{}", "\u{1}".repeat(crate::scene::MAX_ID_BYTES - 4));
        }
        let mut req = Request::new(vec![View::new(ViewSpec::Auto, 480)]);
        req.ids = true;
        let out = render_bytes(&encode(&big), &req, &caps()).expect("long ids").0;
        let m = out.views[0].0.ids.as_ref().expect("ids");
        let line = crate::proto::Response { ok: true, code: None, detail: None, format: Some(ImageFormat::Webp), views: Some(vec![out.views[0].0.clone()]), ..crate::proto::Response::failed(FailureCode::TooLarge, "") }.line();
        eprintln!("[ids] {} objects in the image, {} listed, a {} byte line", m.count, m.objects.len(), line.len());
        assert!(m.count > m.objects.len() as u64 && !m.objects.is_empty());
        assert!(m.objects.windows(2).all(|w| w[0].index < w[1].index));
        assert!(line.len() <= crate::proto::MAX_IDS_LISTED_BYTES + (64 << 10) && line.len() < crate::proto::MAX_RESPONSE_LINE);
        assert!(m.objects.iter().all(|e| e.id == big.objects[e.index as usize].id));
    }

    /// The id images count against the output cap with the stills: a cap
    /// the still fits but not its id image refuses the request with ids.
    #[test]
    fn id_images_count_against_the_output_cap() {
        let input = encode(&testgen::interior_scene());
        let mut req = Request::new(vec![View::new(ViewSpec::Auto, 200)]);
        let plain = render_bytes(&input, &req, &caps()).expect("plain").0;
        req.ids = true;
        let with = render_bytes(&input, &req, &caps()).expect("ids").0;
        let still = plain.views[0].1.len() as u64;
        let tight = Caps { max_output_bytes: still + with.ids[0].len() as u64 / 2, ..caps() };
        assert!(render_bytes(&input, &Request { ids: false, ..req.clone() }, &tight).is_ok());
        assert_eq!(render_bytes(&input, &req, &tight).expect_err("over").code, FailureCode::TooLarge);
        let exact = Caps { max_output_bytes: still + with.ids[0].len() as u64, ..caps() };
        assert!(render_bytes(&input, &req, &exact).is_ok());
    }

    #[test]
    fn png_and_webp_both_decode_to_a_valid_header() {
        let input = encode(&testgen::sphere_scene());
        let mut r = Request::new(vec![View::new(ViewSpec::Auto, 40)]);
        let webp = render_bytes(&input, &r, &caps()).expect("webp").0;
        assert_eq!(&webp.views[0].1[..4], b"RIFF");
        assert_eq!(&webp.views[0].1[8..12], b"WEBP");
        r.format = ImageFormat::Png;
        let png = render_bytes(&input, &r, &caps()).expect("png").0;
        assert_eq!(&png.views[0].1[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(png.views[0].1[25], 2, "colour type RGB");
        r.transparent = true;
        let rgba = render_bytes(&input, &r, &caps()).expect("png rgba").0;
        assert_eq!(rgba.views[0].1[25], 6, "colour type RGBA");
        r.format = ImageFormat::Webp;
        let wa = render_bytes(&input, &r, &caps()).expect("webp rgba").0;
        // VP8L: the alpha_is_used bit (byte 24 of the file, bit 4) is set, and not for the opaque one.
        assert_eq!((&wa.views[0].1[12..16], wa.views[0].1[24] & 0x10), (&b"VP8L"[..], 0x10));
        assert_eq!(webp.views[0].1[24] & 0x10, 0);
    }

    #[test]
    fn the_estimate_grows_with_the_work() {
        let p = |w: u32, t: u64| Plan { view: ViewSpec::Auto, active: vec![], width: w, height: w * 3 / 4, aspect: 4.0 / 3.0, ssaa: 4, vertices: t, triangles: t, without_modifiers: false, transparent: false, framing: Framing::Bounds, ao: false, ids: false };
        let small = estimate_bytes(1000, 100, &[p(320, 10)], 1, 1 << 30);
        let big = estimate_bytes(1000, 100, &[p(320, 1_000_000)], 1, 1 << 30);
        let two = estimate_bytes(1000, 100, &[p(320, 10), p(320, 10)], 1, 1 << 30);
        let rgba = estimate_bytes(1000, 100, &[Plan { transparent: true, ..p(320, 10) }], 1, 1 << 30);
        assert!(small < big && small < two && small < rgba);
        // Ambient occlusion counts its depth image and factors.
        let ao = estimate_bytes(1000, 100, &[Plan { ao: true, ..p(320, 10) }], 1, 1 << 30);
        assert_eq!(ao - small, AO_BYTES_PER_PX * 320 * 240);
        // Ids count their image, its encoding and copy kept, a u32 per band sample, the tables.
        let ids = estimate_bytes(1000, 100, &[Plan { ids: true, ..p(320, 10) }], 2, 1 << 30);
        let plain = estimate_bytes(1000, 100, &[p(320, 10)], 2, 1 << 30);
        assert_eq!(ids - plain, (IDS_BYTES_PER_PX + IDS_KEPT_PER_PX) * 320 * 240 + 2 * 320 * 4 * BAND_PX as u64 * 4 * 4 + IDS_TABLE_BYTES);
    }
}
