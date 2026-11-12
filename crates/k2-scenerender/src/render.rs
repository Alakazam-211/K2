//! The worker's side of a request, also run in-process by the tests and the
//! fuzz targets: plan every view against the header (before any mesh data is
//! read), estimate the memory, then draw each view and encode it. Every
//! failure is a [`Failure`] with its [`FailureCode`].

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Instant;

use crate::camera::{self, ViewSpec, TURNTABLE_FRAMES};
use crate::proto::{FailureCode, ImageFormat, Request, Timings, ViewMeta, DEFAULT_ASPECT, DEFAULT_BACKGROUND};
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
        plans.push(Plan { view: v.view, active, width: v.width, height, aspect, ssaa, vertices: dv, triangles: dt, without_modifiers: without });
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
/// the encoder's copies (9 B per pixel), each thread's band buffers, every
/// encoded image kept until the response (at most the output cap), and 16 MiB
/// of base.
pub fn estimate_bytes(data_bytes: u64, header_bytes: u64, plans: &[Plan], threads: u32, max_output: u64) -> u64 {
    let mut peak = 0u64;
    let mut out = 0u64;
    for p in plans {
        let (w, h, s) = (p.width as u64, p.height as u64, p.ssaa as u64);
        let band = w * s * BAND_PX as u64 * s * 16;
        let view = 32 * p.vertices + 10 * p.triangles + 9 * w * h + threads as u64 * band;
        peak = peak.max(view);
        out = out.saturating_add(4 * w * h);
    }
    data_bytes + 4 * header_bytes + peak + out.min(max_output) + (16 << 20)
}

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
        let frame = camera::frame(scene, &p.active, p.view, p.aspect).map_err(|e| Failure::new(FailureCode::InvalidInput, format!("view {vi}: {e}")))?;
        let params = raster::Params { width: p.width, height: p.height, ssaa: p.ssaa, threads, background_srgb: background };
        let drawn = catch_unwind(AssertUnwindSafe(|| raster::render(scene, &p.active, &frame, &params)))
            .map_err(|_| Failure::new(FailureCode::RendererError, format!("view {vi}: the rasterizer panicked")))?;
        let (img, stats) = drawn;
        let t1 = Instant::now();
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
    use crate::proto::View;
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
            View { view: ViewSpec::Auto, objects: Some(vec![0]), width: 50, aspect: Some(1.0) },
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
            View { view: ViewSpec::Auto, objects: Some(vec![0]), width: 32, aspect: None },
            View { view: ViewSpec::Auto, objects: Some(vec![1]), width: 32, aspect: None },
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
        assert_eq!(code(vec![View { view: ViewSpec::Auto, objects: Some(vec![5]), width: 32, aspect: None }]), FailureCode::InvalidInput);
        assert_eq!(code(vec![View::new(ViewSpec::Auto, 0)]), FailureCode::InvalidInput);
        assert_eq!(code(vec![View::new(ViewSpec::Auto, 4000)]), FailureCode::TooLarge);
        assert_eq!(code(vec![View { view: ViewSpec::Auto, objects: None, width: 1000, aspect: Some(0.01) }]), FailureCode::TooLarge);
        assert_eq!(code(vec![View { view: ViewSpec::Auto, objects: None, width: 10, aspect: Some(f32::NAN) }]), FailureCode::InvalidInput);
        assert_eq!(code(vec![View::new(ViewSpec::Auto, 8); 65]), FailureCode::TooLarge, "too many views");
        let mut r = Request::new(vec![View::new(ViewSpec::Auto, 32)]);
        r.ssaa = Some(5);
        assert_eq!(render_bytes(&input, &r, &c).expect_err("ssaa").code, FailureCode::InvalidInput);
        let tight = Caps { max_ssaa: 2, ..c };
        r.ssaa = Some(4);
        assert_eq!(render_bytes(&input, &r, &tight).expect_err("ssaa cap").code, FailureCode::TooLarge);
        let small_out = Caps { max_output_bytes: 10, ..c };
        assert_eq!(render_bytes(&input, &Request::new(vec![View::new(ViewSpec::Auto, 32)]), &small_out).expect_err("out").code, FailureCode::TooLarge);
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
    }

    #[test]
    fn the_estimate_grows_with_the_work() {
        let p = |w: u32, t: u64| Plan { view: ViewSpec::Auto, active: vec![], width: w, height: w * 3 / 4, aspect: 4.0 / 3.0, ssaa: 4, vertices: t, triangles: t, without_modifiers: false };
        let small = estimate_bytes(1000, 100, &[p(320, 10)], 1, 1 << 30);
        let big = estimate_bytes(1000, 100, &[p(320, 1_000_000)], 1, 1 << 30);
        let two = estimate_bytes(1000, 100, &[p(320, 10), p(320, 10)], 1, 1 << 30);
        assert!(small < big && small < two);
    }
}
