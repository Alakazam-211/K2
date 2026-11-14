//! The worker's side of a request, also run in-process by the tests and the
//! fuzz targets: plan every view against the header (before any mesh data is
//! read), estimate the memory, then draw each view and encode it. Every
//! failure is a [`Failure`] with its [`FailureCode`].

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Instant;

use crate::camera::{self, Framing, ViewSpec, TURNTABLE_FRAMES};
use crate::contents::{self, MAX_LEFT_OUT_LISTED};
use crate::proto::{CameraMeta, FailureCode, FramingMeta, ImageFormat, Request, Timings, ViewMeta, DEFAULT_ASPECT, DEFAULT_BACKGROUND};
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
}

/// The images of a request.
#[derive(Debug)]
pub struct Output {
    pub format: ImageFormat,
    pub views: Vec<(ViewMeta, Vec<u8>)>,
    pub objects_without_modifiers: Vec<u32>,
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
/// triangles) per mesh.
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
            None => (0..objects.len() as u32).collect(),
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
        plans.push(Plan { view: v.view, active, width: v.width, height, aspect, ssaa, vertices: dv, triangles: dt, without_modifiers: without, transparent: req.transparent, framing: v.framing, ao: req.ao.is_some() });
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
/// each thread's band
/// buffers, every encoded image kept until the response (4 B per pixel, 5
/// transparent; at most the output cap), and 16 MiB of base.
pub fn estimate_bytes(data_bytes: u64, header_bytes: u64, plans: &[Plan], threads: u32, max_output: u64) -> u64 {
    let mut peak = 0u64;
    let mut out = 0u64;
    for p in plans {
        let (w, h, s) = (p.width as u64, p.height as u64, p.ssaa as u64);
        let band = w * s * BAND_PX as u64 * s * 16;
        let (image, kept) = if p.transparent { (12, 5) } else { (9, 4) };
        let image = if p.ao { image + AO_BYTES_PER_PX } else { image };
        let view = 32 * p.vertices + 10 * p.triangles + image * w * h + threads as u64 * band;
        peak = peak.max(view);
        out = out.saturating_add(kept * w * h);
    }
    data_bytes + 4 * header_bytes + peak + out.min(max_output) + (16 << 20)
}

/// Ambient occlusion's own buffers, per pixel: the depth image, the raw occlusion and the
/// factors (an f32 each).
pub const AO_BYTES_PER_PX: u64 = 12;

fn ms(since: Instant) -> f64 {
    (since.elapsed().as_secs_f64() * 1e6).round() / 1e3
}

/// Draw and encode every planned view of a decoded scene.
pub fn render_planned(scene: &Scene, plans: &[Plan], req: &Request, caps: &Caps) -> Result<Output, Failure> {
    let threads = threads(req, caps) as usize;
    let background = req.background.unwrap_or(DEFAULT_BACKGROUND);
    let mut views = Vec::with_capacity(plans.len());
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
        let drawn = catch_unwind(AssertUnwindSafe(|| raster::render_shaded(scene, &p.active, &frame, &params, clay.as_ref(), ao.as_ref())))
            .map_err(|_| Failure::new(FailureCode::RendererError, format!("view {vi}: the rasterizer panicked")))?;
        let (img, stats) = drawn;
        let t1 = Instant::now();
        let camera = CameraMeta { view: frame.view_f32(), proj: frame.projection(), fx: frame.fx, fy: frame.fy, near: frame.near, shift_x: frame.shift_x, shift_y: frame.shift_y };
        let bytes = encode::encode(&img, req.format).map_err(|e| Failure::new(FailureCode::RendererError, format!("view {vi}: {e}")))?;
        drop(img);
        total = total.saturating_add(bytes.len() as u64);
        if total > caps.max_output_bytes {
            return Err(Failure::new(FailureCode::TooLarge, format!("the images are over {} bytes", caps.max_output_bytes)));
        }
        let meta = ViewMeta {
            width: p.width,
            height: p.height,
            ssaa: p.ssaa,
            image_len: bytes.len() as u64,
            camera: Some(camera).filter(CameraMeta::is_finite),
            framing,
            objects: stats.objects,
            triangles: stats.triangles,
            covered_px: stats.covered_px,
            without_modifiers: p.without_modifiers,
            vertex_ms: stats.vertex_us as f64 / 1e3,
            bin_ms: stats.bin_us as f64 / 1e3,
            raster_ms: stats.raster_us as f64 / 1e3,
            encode_ms: ms(t1),
        };
        views.push((meta, bytes));
    }
    let objects_without_modifiers =
        scene.objects.iter().enumerate().filter(|(_, o)| o.modifiers_not_applied).map(|(i, _)| i as u32).collect();
    Ok(Output { format: req.format, views, objects_without_modifiers })
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
        let p = |w: u32, t: u64| Plan { view: ViewSpec::Auto, active: vec![], width: w, height: w * 3 / 4, aspect: 4.0 / 3.0, ssaa: 4, vertices: t, triangles: t, without_modifiers: false, transparent: false, framing: Framing::Bounds, ao: false };
        let small = estimate_bytes(1000, 100, &[p(320, 10)], 1, 1 << 30);
        let big = estimate_bytes(1000, 100, &[p(320, 1_000_000)], 1, 1 << 30);
        let two = estimate_bytes(1000, 100, &[p(320, 10), p(320, 10)], 1, 1 << 30);
        let rgba = estimate_bytes(1000, 100, &[Plan { transparent: true, ..p(320, 10) }], 1, 1 << 30);
        assert!(small < big && small < two && small < rgba);
        // Ambient occlusion counts its depth image and factors.
        let ao = estimate_bytes(1000, 100, &[Plan { ao: true, ..p(320, 10) }], 1, 1 << 30);
        assert_eq!(ao - small, AO_BYTES_PER_PX * 320 * 240);
    }
}
