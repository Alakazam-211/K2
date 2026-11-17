//! The mesh read's rows, each returning `Err("<row>, <step>: ...")` so a planted bug says which
//! row and step caught it; shared by `tests/meshread.rs` and the planted bugs in
//! `tests/mutants.rs`. Scenes and meshes are built here, never taken from another program.
#![allow(dead_code)]

use k2_mpc_blend::meshread::check::check;
use k2_mpc_blend::meshread::k2scene::{
    write, Hidden, LeftOut, Scene, SceneCamera, SceneMesh, SceneObject, SensorFit,
};
use k2_mpc_blend::meshread::{MeshReadError, MeshReadLimits};

pub type Row = Result<(), String>;

/// `Err("<row>, <step>: <what>")` unless `ok`.
pub fn step(row: &str, step: &str, ok: bool, what: impl std::fmt::Display) -> Row {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}, {step}: {what}"))
    }
}

/// The golden's bytes: `tests/meshread_golden/scene.k2scene.hex`, written by hand from the
/// layout (comments, `text` lines taken as UTF-8, and hex bytes).
pub fn golden_bytes() -> Vec<u8> {
    let text = include_str!("../meshread_golden/scene.k2scene.hex");
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if let Some(t) = line.strip_prefix("text ") {
            out.extend_from_slice(t.as_bytes());
            continue;
        }
        for h in line.split_whitespace() {
            out.push(u8::from_str_radix(h, 16).expect("a hex byte"));
        }
    }
    out
}

pub const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// An object `id` of key `@obj:<n>` drawing mesh `mesh`, at the identity.
pub fn object(id: &str, n: u32, mesh: u32) -> SceneObject {
    SceneObject {
        id: id.into(),
        key: format!("@obj:{n}"),
        mesh,
        base_color: [0.8, 0.8, 0.8, 1.0],
        double_sided: None,
        transform: IDENTITY,
        modifiers: Vec::new(),
        modifiers_not_applied: false,
        transform_not_evaluated: false,
        hidden: None,
    }
}

/// A camera `id` of key `@cam:<n>`.
pub fn camera(id: &str, n: u32) -> SceneCamera {
    SceneCamera {
        id: id.into(),
        key: format!("@cam:{n}"),
        position: [0.0, -10.0, 5.0],
        target: None,
        forward: Some([0.0, 1.0, -0.5]),
        up: [0.0, 0.0, 1.0],
        yfov: 0.5,
        aspect: Some(1.5),
        znear: Some(0.1),
        active: Some(true),
        lens_mm: Some(50.0),
        sensor_mm: Some([36.0, 24.0]),
        sensor_fit: Some(SensorFit::Auto),
    }
}

/// The golden's scene: a triangle and a quad (two triangles), three objects (two sharing the
/// triangle), one camera.
pub fn golden_scene() -> Scene {
    let mut quad = object("Quad \"q\"\\\u{e9}\u{1}", 3, 1);
    quad.double_sided = Some(false);
    quad.transform = [
        0.5, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, -1.0, 1.0,
    ];
    quad.modifiers = vec!["Subsurf".into(), "Mirror".into()];
    quad.modifiers_not_applied = true;
    quad.transform_not_evaluated = true;
    let mut moved = object("Tri.001", 2, 0);
    moved.transform[12] = 2.0;
    Scene {
        meshes: vec![
            SceneMesh {
                positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                triangles: vec![[0, 1, 2]],
            },
            SceneMesh {
                positions: vec![[0.0; 3], [2.0, 0.0, 0.0], [2.0, 2.0, 0.0], [0.0, 2.0, 0.0]],
                triangles: vec![[0, 1, 2], [0, 2, 3]],
            },
        ],
        objects: vec![object("Tri", 1, 0), moved, quad],
        cameras: vec![camera("Camera", 4)],
        cameras_left_out: Vec::new(),
    }
}

/// More scenes for the round trip: empty; a camera by target with no display fields; floats
/// that need many digits, and the extremes of `f32`.
pub fn more_scenes() -> Vec<Scene> {
    let mut by_target = camera("Target", 3);
    by_target.target = Some([0.0; 3]);
    by_target.forward = None;
    by_target.aspect = None;
    by_target.znear = None;
    by_target.active = None;
    by_target.lens_mm = None;
    by_target.sensor_mm = None;
    by_target.sensor_fit = None;
    let mut odd = object("Odd", 1, 0);
    odd.hidden = Some(Hidden::Collection);
    let mut own = object("Own", 2, 0);
    own.hidden = Some(Hidden::Itself);
    odd.transform = [
        0.1,
        1.0 / 3.0,
        -0.0,
        f32::MIN_POSITIVE,
        f32::MAX,
        f32::MIN,
        1e-45,
        123456.79,
        std::f32::consts::PI,
        -2.5e-38,
        7e30,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ];
    let mut vertical = camera("Vertical", 4);
    vertical.sensor_fit = Some(SensorFit::Vertical);
    vertical.active = Some(false);
    let mut horizontal = camera("Horizontal", 5);
    horizontal.sensor_fit = Some(SensorFit::Horizontal);
    horizontal.active = None;
    vec![
        Scene::default(),
        Scene {
            meshes: vec![SceneMesh {
                positions: vec![[1e-7, -3.4e38, 0.333_333_34], [0.0; 3], [1.0; 3]],
                triangles: vec![[2, 1, 0], [0, 0, 0]],
            }],
            objects: vec![odd, own],
            cameras: vec![by_target, vertical, horizontal, camera("Active", 6)],
            cameras_left_out: vec![
                LeftOut {
                    name: "Side".into(),
                    key: Some("@cam:9".into()),
                    reason: "camera_not_perspective".into(),
                },
                LeftOut {
                    name: "Ünïcode \"quoted\"".into(),
                    key: None,
                    reason: "camera_sensor_fit_unknown".into(),
                },
            ],
        },
    ]
}

/// Writes `scene` and reads it back through the checker: the same scene, up +Z, the data where
/// the header's counts put it.
fn round_trip_one(row: &str, scene: &Scene) -> Row {
    let limits = MeshReadLimits::default();
    let bytes = write(scene, &limits).map_err(|e| format!("{row}, write: {e}"))?;
    let back = check(&bytes, &limits);
    step(
        row,
        "the checker accepts",
        back.is_ok(),
        format!("{back:?}"),
    )?;
    let back = back.unwrap();
    step(row, "up is z", back.up == "z", &back.up)?;
    step(
        row,
        "the scene reads back",
        back.scene == *scene,
        format!("{:?}", back.scene),
    )
}

/// The writer's output reads back through the strict checker as the scene written, for the
/// golden's scene and the others.
pub fn row_round_trip() -> Row {
    let row = "round trip";
    let mut scenes = vec![golden_scene()];
    scenes.extend(more_scenes());
    let mut triangles = 0;
    for s in &scenes {
        round_trip_one(row, s)?;
        triangles += s.meshes.iter().map(|m| m.triangles.len()).sum::<usize>();
    }
    step(row, "scenes exercised", scenes.len() == 3, scenes.len())?;
    step(row, "triangles exercised", triangles == 5, triangles)?;
    println!("round trip: {} scenes, {triangles} triangles", scenes.len());
    Ok(())
}

/// The golden: the writer's bytes are the hand-written ones, and the checker reads them as the
/// golden's scene.
pub fn row_golden() -> Row {
    let row = "golden";
    let want = golden_bytes();
    let got = write(&golden_scene(), &MeshReadLimits::default())
        .map_err(|e| format!("{row}, write: {e}"))?;
    step(
        row,
        "length",
        got.len() == want.len(),
        format!("{} vs {}", got.len(), want.len()),
    )?;
    let first = got.iter().zip(&want).position(|(a, b)| a != b);
    step(
        row,
        "bytes",
        first.is_none(),
        format!("first difference at {first:?}"),
    )?;
    let back =
        check(&want, &MeshReadLimits::default()).map_err(|e| format!("{row}, check: {e}"))?;
    step(
        row,
        "the golden reads",
        back.scene == golden_scene(),
        "another scene",
    )?;
    println!("golden: {} bytes", want.len());
    Ok(())
}

/// Limits with every count at `scene`'s own (so it just passes).
fn exact_limits(scene: &Scene, header: u64, output: u64) -> MeshReadLimits {
    let v: u64 = scene.meshes.iter().map(|m| m.positions.len() as u64).sum();
    let t: u64 = scene.meshes.iter().map(|m| m.triangles.len() as u64).sum();
    let (mut dv, mut dt) = (0u64, 0u64);
    for o in &scene.objects {
        let m = &scene.meshes[o.mesh as usize];
        dv += m.positions.len() as u64;
        dt += m.triangles.len() as u64;
    }
    let mut l = MeshReadLimits::default();
    l.max_meshes = scene.meshes.len() as u64;
    l.max_objects = scene.objects.len() as u64;
    l.max_cameras = scene.cameras.len() as u64;
    l.max_vertices = v;
    l.max_triangles = t;
    l.max_drawn_vertices = dv;
    l.max_drawn_triangles = dt;
    l.max_header_bytes = header;
    l.max_output_bytes = output;
    l
}

/// Takes one off a limit.
type Under = fn(&mut MeshReadLimits);

/// Each limit: the golden's scene passes at it and is refused, typed and naming it, one under.
pub fn row_limits() -> Row {
    let row = "limits";
    let scene = golden_scene();
    let full = write(&scene, &MeshReadLimits::default()).map_err(|e| format!("{row}: {e}"))?;
    step(row, "a header", full.len() >= 12, full.len())?;
    let hlen = u64::from(u32::from_le_bytes(full[8..12].try_into().unwrap()));
    let at = exact_limits(&scene, hlen, full.len() as u64);
    let ok = write(&scene, &at);
    step(
        row,
        "at every limit",
        ok.as_ref() == Ok(&full),
        format!("{ok:?}"),
    )?;
    let cases: [(&str, Under); 9] = [
        ("meshes", |l| l.max_meshes -= 1),
        ("objects", |l| l.max_objects -= 1),
        ("cameras", |l| l.max_cameras -= 1),
        ("vertices", |l| l.max_vertices -= 1),
        ("triangles", |l| l.max_triangles -= 1),
        ("drawn vertices", |l| l.max_drawn_vertices -= 1),
        ("drawn triangles", |l| l.max_drawn_triangles -= 1),
        ("header bytes", |l| l.max_header_bytes -= 1),
        ("output bytes", |l| l.max_output_bytes -= 1),
    ];
    for (what, under) in cases {
        let mut l = at;
        under(&mut l);
        let got = write(&scene, &l);
        let named = matches!(&got, Err(MeshReadError::TooLarge { what: w, .. }) if *w == what);
        step(row, what, named, format!("{got:?}"))?;
        step(
            row,
            "the code",
            got.as_ref().err().map(|e| e.code()) == Some("too_large"),
            what,
        )?;
    }
    println!("limits: {} limits, each at and one under", cases.len());
    Ok(())
}

/// What a renderer refuses is refused, typed, by the writer.
pub fn row_writer_refuses() -> Row {
    let row = "writer refuses";
    let l = MeshReadLimits::default();
    let mut cases: Vec<(&str, Scene)> = Vec::new();
    let mut s = golden_scene();
    s.meshes[0].positions[1][0] = f32::NAN;
    cases.push(("a position isn't finite", s));
    let mut s = golden_scene();
    s.meshes[1].triangles[1][2] = 4;
    cases.push(("an index is past its mesh's vertices", s));
    let mut s = golden_scene();
    s.objects[0].mesh = 2;
    cases.push(("an object names no mesh", s));
    let mut s = golden_scene();
    s.objects[1].transform[3] = f32::INFINITY;
    cases.push(("an object's transform or colour isn't finite", s));
    let mut s = golden_scene();
    s.objects[0].id = "x".repeat(1025);
    cases.push(("an object's id is too long", s));
    let mut s = golden_scene();
    s.objects[2].modifiers.push("m".repeat(129));
    cases.push(("a modifier name is too long", s));
    let mut s = golden_scene();
    s.cameras.push(camera("Second", 5));
    cases.push(("a second active camera", s));
    let mut s = golden_scene();
    s.cameras[0].yfov = std::f32::consts::PI;
    cases.push(("a camera's yfov is outside (0, pi)", s));
    let mut s = golden_scene();
    s.cameras[0].forward = None;
    cases.push(("a camera has no target or forward", s));
    let mut s = golden_scene();
    s.cameras[0].sensor_mm = Some([36.0, 0.0]);
    cases.push(("a camera's sensor isn't positive", s));
    let n = cases.len();
    for (what, s) in cases {
        let got = write(&s, &l);
        step(
            row,
            what,
            got == Err(MeshReadError::Invalid { what }),
            format!("{got:?}"),
        )?;
    }
    let mut s = golden_scene();
    s.objects[2].modifiers = vec!["m".into(); 257];
    let got = write(&s, &l);
    step(
        row,
        "more than 256 modifiers",
        matches!(
            got,
            Err(MeshReadError::TooLarge {
                what: "modifiers of an object",
                ..
            })
        ),
        format!("{got:?}"),
    )?;
    println!("writer refuses: {} cases", n + 1);
    Ok(())
}

/// The checker refuses each break of the layout, made to a good output: so it can fail.
pub fn row_checker_refuses() -> Row {
    let row = "checker refuses";
    let l = MeshReadLimits::default();
    let good = golden_bytes();
    step(
        row,
        "the good one passes",
        check(&good, &l).is_ok(),
        "refused",
    )?;
    let hlen = u32::from_le_bytes(good[8..12].try_into().unwrap()) as usize;
    let data = (12 + hlen + 3) & !3;
    let header = std::str::from_utf8(&good[12..12 + hlen])
        .unwrap()
        .to_owned();
    let with_header = |h: &str| {
        let mut b = b"K2SCENE1".to_vec();
        b.extend((h.len() as u32).to_le_bytes());
        b.extend(h.as_bytes());
        while !b.len().is_multiple_of(4) {
            b.push(0);
        }
        b.extend(&good[data..]);
        b
    };
    let mut cases: Vec<(&str, Vec<u8>, &str)> = Vec::new();
    let mut b = good.clone();
    b.push(0);
    cases.push(("a trailing byte", b, "after the data"));
    cases.push((
        "one byte short",
        good[..good.len() - 1].to_vec(),
        "truncated",
    ));
    let mut b = good.clone();
    b[12 + hlen] = 1;
    cases.push(("non-zero padding", b, "padding"));
    let mut b = good.clone();
    b[0] = b'X';
    cases.push(("the magic", b, "not a K2SCENE1"));
    let mut b = good.clone();
    let last = b.len() - 4;
    b[last..].copy_from_slice(&4u32.to_le_bytes());
    cases.push(("an index past its mesh", b, "index 4"));
    let mut b = good.clone();
    b[data..data + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    cases.push(("a position that isn't finite", b, "not finite"));
    cases.push((
        "a count off by one",
        with_header(&header.replacen("\"triangles\":1", "\"triangles\":2", 1)),
        "truncated",
    ));
    cases.push((
        "a duplicate key",
        with_header(&header.replacen("\"up\":\"z\"", "\"up\":\"z\",\"up\":\"z\"", 1)),
        "duplicate",
    ));
    cases.push((
        "an unknown key",
        with_header(&header.replacen("\"up\":\"z\"", "\"up\":\"z\",\"extra\":1", 1)),
        "unknown key",
    ));
    cases.push((
        "two active cameras",
        with_header(&header.replacen(
            "\"cameras\":[",
            "\"cameras\":[{\"key\":\"@cam:9\",\"position\":[0,0,0],\"forward\":[0,1,0],\"up\":[0,0,1],\"yfov\":1,\"active\":true},",
            1,
        )),
        "second active",
    ));
    cases.push((
        "a version",
        with_header(&header.replacen("\"version\":1", "\"version\":2", 1)),
        "version 2",
    ));
    cases.push((
        "a whole number written with a fraction",
        with_header(&header.replacen("\"mesh\":0", "\"mesh\":0.0", 1)),
        "not a whole number",
    ));
    // keys: each object and camera has one, an address, once
    cases.push((
        "an object without its key",
        with_header(&header.replacen("\"key\":\"@obj:1\",", "", 1)),
        "no key",
    ));
    cases.push((
        "a camera without its key",
        with_header(&header.replacen("\"key\":\"@cam:4\",", "", 1)),
        "no key",
    ));
    cases.push((
        "a key with a leading zero",
        with_header(&header.replacen("@obj:1\"", "@obj:01\"", 1)),
        "isn't @obj:N",
    ));
    cases.push((
        "a key of another kind",
        with_header(&header.replacen("@obj:2\"", "@cam:2\"", 1)),
        "isn't @obj:N",
    ));
    cases.push((
        "a key twice",
        with_header(&header.replacen("@obj:2\"", "@obj:1\"", 1)),
        "given twice",
    ));
    cases.push((
        "a camera's number an object's",
        with_header(&header.replacen("@cam:4\"", "@cam:3\"", 1)),
        "given twice",
    ));
    // the meshes' hashes: there, in lowercase hex, and the data's
    let first = header
        .find("\"hash\":\"")
        .map(|i| i + 8)
        .ok_or_else(|| format!("{row}: the golden has no hash"))?;
    let h0 = header[first..first + 64].to_owned();
    cases.push((
        "a mesh without its hash",
        with_header(&header.replacen(&format!(",\"hash\":\"{h0}\""), "", 1)),
        "no hash",
    ));
    cases.push((
        "a hash in capitals",
        with_header(&header.replacen(&h0, &h0.to_uppercase(), 1)),
        "lowercase hex",
    ));
    let mut b = good.clone();
    b[data..data + 4].copy_from_slice(&0.5f32.to_le_bytes());
    cases.push(("a position its hash doesn't hold", b, "its hash isn't"));
    let mut b = good.clone();
    let swap = data + 3 * 12;
    b[swap..swap + 4].copy_from_slice(&1u32.to_le_bytes());
    b[swap + 4..swap + 8].copy_from_slice(&0u32.to_le_bytes());
    cases.push(("a triangle its hash doesn't hold", b, "its hash isn't"));
    let n = cases.len();
    for (what, b, says) in cases {
        let got = check(&b, &l);
        let refused = matches!(&got, Err(e) if e.contains(says));
        step(row, what, refused, format!("{:?}", got.map(|_| "accepted")))?;
    }
    println!("checker refuses: {n} breaks");
    Ok(())
}

// ---- the triangulator ------------------------------------------------------------------------

use k2_mpc_blend::meshread::tri::{triangulate, TriCounts};

/// A mesh's polygons, flat: positions, corners and offsets (polygon `i`'s corners are
/// `corners[offsets[i]..offsets[i + 1]]`).
#[derive(Debug, Clone, Default)]
pub struct Polys {
    pub positions: Vec<[f32; 3]>,
    pub corners: Vec<u32>,
    pub offsets: Vec<u32>,
}

impl Polys {
    pub fn new() -> Self {
        Self {
            offsets: vec![0],
            ..Self::default()
        }
    }

    /// A polygon of new vertices at `points`, in their order.
    pub fn add(&mut self, points: &[[f32; 3]]) {
        for p in points {
            self.corners.push(self.positions.len() as u32);
            self.positions.push(*p);
        }
        self.offsets.push(self.corners.len() as u32);
    }

    pub fn faces(&self) -> usize {
        self.offsets.len() - 1
    }

    pub fn face(&self, i: usize) -> &[u32] {
        &self.corners[self.offsets[i] as usize..self.offsets[i + 1] as usize]
    }

    /// Triangulated with `max_corners`: the triangles and the counts.
    pub fn triangulated(&self, max_corners: u32) -> (Vec<[u32; 3]>, TriCounts) {
        let mut out = Vec::new();
        let mut counts = TriCounts::default();
        triangulate(
            &self.positions,
            &self.corners,
            &self.offsets,
            max_corners,
            &mut out,
            &mut counts,
        );
        (out, counts)
    }
}

type V3 = [f64; 3];

fn v(p: [f32; 3]) -> V3 {
    [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}

/// A triangle's normal, length twice its area.
fn tri_normal(p: &[[f32; 3]], t: [u32; 3]) -> V3 {
    let (a, b, c) = (
        v(p[t[0] as usize]),
        v(p[t[1] as usize]),
        v(p[t[2] as usize]),
    );
    cross(sub(b, a), sub(c, a))
}

/// A polygon's normal (Newell), length twice its area.
fn poly_normal(p: &[[f32; 3]], face: &[u32]) -> V3 {
    let mut n = [0f64; 3];
    for (&i, &j) in face.iter().zip(face.iter().cycle().skip(1)) {
        let (c, d) = (v(p[i as usize]), v(p[j as usize]));
        n[0] += (c[1] - d[1]) * (c[2] + d[2]);
        n[1] += (c[2] - d[2]) * (c[0] + d[0]);
        n[2] += (c[0] - d[0]) * (c[1] + d[1]);
    }
    n
}

/// `q` inside the planar polygon `face` (even-odd), in the polygon's plane seen along `n`.
fn in_polygon(p: &[[f32; 3]], face: &[u32], n: V3, q: V3) -> bool {
    // two axes across the normal
    let a = if n[0].abs() < 0.9 * len(n) {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = cross(n, a);
    let w = cross(n, u);
    let at = |x: V3| (dot(x, u), dot(x, w));
    let (qx, qy) = at(q);
    let mut inside = false;
    for (&i, &j) in face.iter().zip(face.iter().cycle().skip(1)) {
        let (ax, ay) = at(v(p[i as usize]));
        let (bx, by) = at(v(p[j as usize]));
        if (ay > qy) != (by > qy) && qx < ax + (qy - ay) * (bx - ax) / (by - ay) {
            inside = !inside;
        }
    }
    inside
}

/// The unit cube's eight corners and six quads, each counter-clockwise seen from outside.
pub fn cube() -> Polys {
    let c = [
        [-1.0, -1.0, -1.0],
        [1.0, -1.0, -1.0],
        [1.0, 1.0, -1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
    ];
    let quads = [
        [0, 3, 2, 1],
        [4, 5, 6, 7],
        [0, 1, 5, 4],
        [3, 7, 6, 2],
        [0, 4, 7, 3],
        [1, 2, 6, 5],
    ];
    let mut p = Polys::new();
    p.positions = c.to_vec();
    for q in quads {
        p.corners.extend(q);
        p.offsets.push(p.corners.len() as u32);
    }
    p
}

/// The concave set, counter-clockwise in the plane: an L (started where a fan from the first
/// corner leaves it), an arrow, a ten-pointed star started at an inner corner, a comb.
pub fn concave_2d() -> Vec<(&'static str, Vec<[f32; 2]>)> {
    let star = (0..10)
        .map(|k| {
            let r = if k % 2 == 0 { 0.8 } else { 2.0 };
            let a = k as f64 * std::f64::consts::PI / 5.0;
            [(r * a.cos()) as f32, (r * a.sin()) as f32]
        })
        .collect();
    let comb = [
        [4.0, 1.0],
        [3.0, 1.0],
        [3.0, 3.0],
        [2.0, 3.0],
        [2.0, 1.0],
        [1.0, 1.0],
        [1.0, 3.0],
        [0.0, 3.0],
        [0.0, 0.0],
        [5.0, 0.0],
        [5.0, 3.0],
        [4.0, 3.0],
    ];
    vec![
        (
            "L",
            vec![
                [2.0, 0.0],
                [2.0, 1.0],
                [1.0, 1.0],
                [1.0, 2.0],
                [0.0, 2.0],
                [0.0, 0.0],
            ],
        ),
        (
            "arrow",
            vec![[0.0, 0.0], [2.0, 1.0], [0.0, 2.0], [1.0, 1.0]],
        ),
        ("star", star),
        ("comb", comb.to_vec()),
    ]
}

/// The concave set in three planes: flat (normal +Z), tilted (its normal mostly -Y, so the
/// projection flips), and flat with the corner order reversed (normal -Z).
pub fn concave() -> (Polys, Vec<String>) {
    let mut p = Polys::new();
    let mut names = Vec::new();
    for (name, pts) in concave_2d() {
        let flat: Vec<[f32; 3]> = pts.iter().map(|q| [q[0], q[1], 0.0]).collect();
        let tilted: Vec<[f32; 3]> = pts
            .iter()
            .map(|q| [q[0] + 1.0, q[1] * 0.6 + 3.0, q[1] * 0.8 - 2.0])
            .collect();
        let mut reversed = flat.clone();
        reversed.reverse();
        for (how, pts) in [("flat", flat), ("tilted", tilted), ("reversed", reversed)] {
            p.add(&pts);
            names.push(format!("{name} {how}"));
        }
    }
    (p, names)
}

/// Every triangle of `face` (its slice of `tris`) faces the polygon's way, the areas add up to
/// the polygon's, and every triangle's centroid is inside it.
fn covers(row: &str, name: &str, p: &Polys, face: &[u32], tris: &[[u32; 3]]) -> Row {
    let n = poly_normal(&p.positions, face);
    let area = len(n) / 2.0;
    let mut sum = 0.0;
    for &t in tris {
        let tn = tri_normal(&p.positions, t);
        let d = dot(tn, n);
        step(
            row,
            &format!("{name} faces its way"),
            d >= -1e-9 * area * area,
            format!("{t:?}: {d}"),
        )?;
        sum += len(tn) / 2.0;
        let c = t.iter().fold([0f64; 3], |acc, &i| {
            let q = v(p.positions[i as usize]);
            [
                acc[0] + q[0] / 3.0,
                acc[1] + q[1] / 3.0,
                acc[2] + q[2] / 3.0,
            ]
        });
        if len(tn) > 1e-9 {
            step(
                row,
                &format!("{name} inside"),
                in_polygon(&p.positions, face, n, c),
                format!("{t:?}"),
            )?;
        }
    }
    step(
        row,
        &format!("{name} area"),
        (sum - area).abs() <= 1e-6 * area.max(1.0),
        format!("{sum} vs {area}"),
    )
}

/// The cube: 12 triangles, every one facing out.
pub fn row_winding() -> Row {
    let row = "winding";
    let p = cube();
    let (tris, _) = p.triangulated(4096);
    step(row, "12 triangles", tris.len() == 12, tris.len())?;
    for t in &tris {
        let n = tri_normal(&p.positions, *t);
        let c = t.iter().fold([0f64; 3], |acc, &i| {
            let q = v(p.positions[i as usize]);
            [acc[0] + q[0], acc[1] + q[1], acc[2] + q[2]]
        });
        step(
            row,
            "every triangle faces out",
            dot(n, c) > 0.0,
            format!("{t:?}"),
        )?;
    }
    println!("winding: {} triangles, each facing out", tris.len());
    Ok(())
}

/// The concave set, flat, tilted and reversed: `n - 2` triangles each, covering the polygon
/// exactly, each facing the polygon's way.
pub fn row_concave() -> Row {
    let row = "concave";
    let (p, names) = concave();
    let (tris, counts) = p.triangulated(4096);
    let mut at = 0;
    for (i, name) in names.iter().enumerate() {
        let face = p.face(i);
        let k = face.len() - 2;
        let mine = tris
            .get(at..at + k)
            .ok_or_else(|| format!("{row}, {name} n - 2: too few triangles"))?;
        covers(row, name, &p, face, mine)?;
        at += k;
    }
    step(
        row,
        "all triangles",
        at == tris.len(),
        format!("{at} vs {}", tris.len()),
    )?;
    step(
        row,
        "ear clipped",
        counts.ear_clipped == names.len() as u64,
        format!("{counts:?}"),
    )?;
    println!(
        "concave: {} polygons, {} triangles, {counts:?}",
        names.len(),
        tris.len()
    );
    Ok(())
}

/// Exactly `n - 2` triangles per polygon whatever its shape (regular, concave, on one line, on
/// one point, crossing itself, over the ear-clipping limit), every index one of its polygon's,
/// and the same output twice.
pub fn row_counts() -> Row {
    let row = "counts";
    let mut p = Polys::new();
    for n in 3..=12 {
        let pts: Vec<[f32; 3]> = (0..n)
            .map(|k| {
                let a = k as f64 * std::f64::consts::TAU / n as f64;
                [a.cos() as f32, a.sin() as f32, 0.5]
            })
            .collect();
        p.add(&pts);
    }
    let (c, _) = concave();
    for i in 0..c.faces() {
        let pts: Vec<[f32; 3]> = c.face(i).iter().map(|&k| c.positions[k as usize]).collect();
        p.add(&pts);
    }
    // on one line; on one point; crossing itself (no ear in a whole round)
    p.add(&[[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0]]);
    p.add(&[[1.0; 3]; 5]);
    p.add(&[
        [3.0, 3.0, 0.0],
        [0.0, 2.0, 0.0],
        [0.0, 3.0, 0.0],
        [1.0, 0.0, 0.0],
        [2.0, 3.0, 0.0],
    ]);
    // over the limit: 40 corners, a limit of 32
    let big: Vec<[f32; 3]> = (0..40)
        .map(|k| {
            let a = k as f64 * std::f64::consts::TAU / 40.0;
            let r = if k % 2 == 0 { 1.0 } else { 0.5 };
            [(r * a.cos()) as f32, (r * a.sin()) as f32, 0.0]
        })
        .collect();
    p.add(&big);
    let (tris, counts) = p.triangulated(32);
    let mut at = 0;
    for i in 0..p.faces() {
        let face = p.face(i);
        let k = face.len() - 2;
        let mine = tris
            .get(at..at + k)
            .ok_or_else(|| format!("{row}, n - 2 for polygon {i}: too few triangles"))?;
        for t in mine {
            step(
                row,
                "indices of the polygon",
                t.iter().all(|x| face.contains(x)),
                format!("{i}: {t:?}"),
            )?;
        }
        at += k;
    }
    step(
        row,
        "n - 2 in all",
        at == tris.len(),
        format!("{at} vs {}", tris.len()),
    )?;
    let want = p.corners.len() - 2 * p.faces();
    step(
        row,
        "corners - 2 * faces",
        tris.len() == want,
        format!("{} vs {want}", tris.len()),
    )?;
    step(
        row,
        "counted",
        counts.triangles == want as u64 && counts.faces == p.faces() as u64,
        format!("{counts:?}"),
    )?;
    step(
        row,
        "degenerate",
        counts.degenerate == 2,
        format!("{counts:?}"),
    )?;
    step(
        row,
        "ear clip failed",
        counts.ear_clip_failed == 1,
        format!("{counts:?}"),
    )?;
    step(
        row,
        "large fanned",
        counts.large_face_fanned == 1,
        format!("{counts:?}"),
    )?;
    let (again, _) = p.triangulated(32);
    step(row, "the same twice", again == tris, "differs")?;
    println!(
        "counts: {} polygons, {} triangles, {counts:?}",
        p.faces(),
        tris.len()
    );
    Ok(())
}

// ---- the mesh layouts ------------------------------------------------------------------------

use k2_mpc_blend::meshread::mesh::{self, Layout, MeshSkip};
use k2_mpc_blend::testgen::{MeshLayouts, MeshSpec, SceneBuilder};
use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{read_blend, BlendFile, ReadLimits};

/// A file at `version` whose schema has `schema`'s layouts, holding `meshes` (each written in
/// its own layouts).
pub fn mesh_file(
    version: u16,
    schema: MeshLayouts,
    meshes: &[(&str, MeshSpec, MeshLayouts)],
) -> Vec<u8> {
    let mut b = SceneBuilder::meshes(version, schema);
    for (name, spec, write) in meshes {
        b.mesh_with(name, spec, *write);
    }
    b.bytes()
}

/// The file read, its decompressed bytes, and its mesh blocks in file order.
pub fn read_meshes(bytes: &[u8]) -> (BlendFile, Vec<u8>, Vec<u32>) {
    let (_, raw) = decompress(bytes, &ReadLimits::default()).expect("check: decompresses");
    let file = read_blend(&mut &raw[..], &ReadLimits::default()).expect("check: reads");
    let me = file
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| &b.head.code == b"ME\0\0")
        .map(|(i, _)| i as u32)
        .collect();
    (file, raw, me)
}

/// The cube as a mesh spec.
pub fn cube_spec() -> MeshSpec {
    let c = cube();
    MeshSpec {
        positions: c.positions.clone(),
        faces: (0..c.faces()).map(|i| c.face(i).to_vec()).collect(),
        ..MeshSpec::default()
    }
}

/// Each layout and the version that writes it alone in the corpus (`PROVENANCE.md`).
pub const LAYOUTS: [(Layout, MeshLayouts, u16); 3] = [
    (Layout::Layers, MeshLayouts::LAYERS, 403),
    (Layout::Structs, MeshLayouts::STRUCTS, 306),
    (Layout::Store, MeshLayouts::STORE, 501),
];

/// The mesh read of a one-mesh file in layout `l` at `version`.
fn read_one(
    version: u16,
    l: MeshLayouts,
    spec: &MeshSpec,
) -> Result<(Layout, SceneMesh), MeshSkip> {
    let bytes = mesh_file(version, l, &[("M", spec.clone(), l)]);
    let (file, raw, me) = read_meshes(&bytes);
    let mut counts = TriCounts::default();
    mesh::read_mesh(&file, &raw, me[0], 4096, &mut counts)
}

/// The cube in each layout alone: found as that layout only, read as 8 positions bit for bit
/// and 12 triangles facing out.
pub fn row_layouts() -> Row {
    let row = "layouts";
    let spec = cube_spec();
    for (layout, l, version) in LAYOUTS {
        let w = layout.word();
        let bytes = mesh_file(version, l, &[("Cube", spec.clone(), l)]);
        let (file, raw, me) = read_meshes(&bytes);
        step(row, &format!("{w}: one mesh"), me.len() == 1, me.len())?;
        let found = mesh::layouts(&file, &raw, me[0]);
        step(
            row,
            &format!("{w}: found alone"),
            found == vec![layout],
            format!("{found:?}"),
        )?;
        let c = mesh::counts(&file, &raw, me[0]);
        step(
            row,
            &format!("{w}: counts"),
            c.map(|c| (c.vertices, c.faces, c.corners)) == Ok((8, 6, 24)),
            format!("{c:?}"),
        )?;
        let got = read_one(version, l, &spec);
        let (lay, m) = got.map_err(|e| format!("{row}, {w}: read: {e:?}"))?;
        step(
            row,
            &format!("{w}: layout"),
            lay == layout,
            format!("{lay:?}"),
        )?;
        let bits = |v: &[[f32; 3]]| v.iter().flatten().map(|x| x.to_bits()).collect::<Vec<_>>();
        step(
            row,
            &format!("{w}: positions"),
            bits(&m.positions) == bits(&spec.positions),
            format!("{:?}", m.positions),
        )?;
        step(
            row,
            &format!("{w}: 12 triangles"),
            m.triangles.len() == 12,
            m.triangles.len(),
        )?;
        for t in &m.triangles {
            let n = tri_normal(&m.positions, *t);
            let c = t.iter().fold([0f64; 3], |acc, &i| {
                let q = v(m.positions[i as usize]);
                [acc[0] + q[0], acc[1] + q[1], acc[2] + q[2]]
            });
            step(
                row,
                &format!("{w}: faces out"),
                dot(n, c) > 0.0,
                format!("{t:?}"),
            )?;
        }
    }
    println!(
        "layouts: {} layouts, the cube read from each",
        LAYOUTS.len()
    );
    Ok(())
}

/// A mesh written in both the attribute layers and the structs (a file saved for older readers
/// too): both found, both read alike, the layers taken; and a concave mesh reads the same from
/// every layout.
pub fn row_layouts_agree() -> Row {
    let row = "layouts agree";
    let both = MeshLayouts {
        layers: true,
        structs: true,
        store: false,
    };
    let bytes = mesh_file(306, both, &[("Cube", cube_spec(), both)]);
    let (file, raw, me) = read_meshes(&bytes);
    let found = mesh::layouts(&file, &raw, me[0]);
    step(
        row,
        "both found",
        found == vec![Layout::Layers, Layout::Structs],
        format!("{found:?}"),
    )?;
    let a = mesh::read_geometry(&file, &raw, me[0], Layout::Layers)
        .map_err(|e| format!("{row}: {e:?}"))?;
    let b = mesh::read_geometry(&file, &raw, me[0], Layout::Structs)
        .map_err(|e| format!("{row}: {e:?}"))?;
    step(
        row,
        "the same geometry",
        (&a.positions, &a.corners, &a.faces) == (&b.positions, &b.corners, &b.faces),
        format!("{a:?} vs {b:?}"),
    )?;
    let mut counts = TriCounts::default();
    let got = mesh::read_mesh(&file, &raw, me[0], 4096, &mut counts);
    step(
        row,
        "the layers taken",
        matches!(got, Ok((Layout::Layers, _))),
        format!("{got:?}"),
    )?;
    // a concave mesh: the same from every layout
    let (c, _) = concave();
    let spec = MeshSpec {
        positions: c.positions.clone(),
        faces: (0..c.faces()).map(|i| c.face(i).to_vec()).collect(),
        ..MeshSpec::default()
    };
    let mut first: Option<SceneMesh> = None;
    for (layout, l, version) in LAYOUTS {
        let (_, m) =
            read_one(version, l, &spec).map_err(|e| format!("{row}, {}: {e:?}", layout.word()))?;
        let want = c.corners.len() - 2 * c.faces();
        step(
            row,
            "n - 2 each",
            m.triangles.len() == want,
            m.triangles.len(),
        )?;
        if let Some(f) = &first {
            step(
                row,
                &format!("{} as the first", layout.word()),
                *f == m,
                "differs",
            )?;
        }
        if first.is_none() {
            first = Some(m);
        }
    }
    println!(
        "layouts agree: the cube in two layouts, {} concave polygons in three",
        c.faces()
    );
    Ok(())
}

/// A count the arrays don't hold is refused before anything is read: a mesh claiming 9
/// vertices with 8 stored, in each layout.
pub fn row_counts_checked() -> Row {
    let row = "counts checked";
    let spec = MeshSpec {
        claimed_vertices: Some(9),
        ..cube_spec()
    };
    for (layout, l, version) in LAYOUTS {
        let got = read_one(version, l, &spec);
        step(
            row,
            &format!("{}: array length", layout.word()),
            got == Err(MeshSkip::ArrayLength("positions")),
            format!("{:?}", got.map(|(l, m)| (l, m.positions.len()))),
        )?;
    }
    println!("counts checked: {} layouts", LAYOUTS.len());
    Ok(())
}

/// A loose vertex isn't written: the cube with one more vertex no polygon uses reads as the
/// cube's 8, renumbered in file order, its triangles at the same points; a mesh of loose
/// vertices only has nothing to draw.
pub fn row_loose_vertices() -> Row {
    let row = "loose vertices";
    let cube = cube_spec();
    let mut spec = cube.clone();
    spec.positions.insert(4, [9.0, 9.0, 9.0]);
    for f in &mut spec.faces {
        for i in f.iter_mut() {
            if *i >= 4 {
                *i += 1;
            }
        }
    }
    for (layout, l, version) in LAYOUTS {
        let w = layout.word();
        let (_, m) = read_one(version, l, &spec).map_err(|e| format!("{row}, {w}: {e:?}"))?;
        step(
            row,
            &format!("{w}: 8 vertices"),
            m.positions == cube.positions,
            format!("{:?}", m.positions),
        )?;
        let (_, plain) = read_one(version, l, &cube).map_err(|e| format!("{row}, {w}: {e:?}"))?;
        step(
            row,
            &format!("{w}: the same triangles"),
            m.triangles == plain.triangles,
            format!("{:?}", m.triangles),
        )?;
        let only = MeshSpec {
            positions: cube.positions.clone(),
            ..MeshSpec::default()
        };
        let got = read_one(version, l, &only);
        step(
            row,
            &format!("{w}: nothing to draw"),
            got == Err(MeshSkip::NoTriangles),
            format!("{got:?}"),
        )?;
    }
    println!("loose vertices: {} layouts", LAYOUTS.len());
    Ok(())
}

/// Each break of a mesh is a typed skip, in each layout: a corner past the vertices, a polygon
/// of 2 corners, offsets that don't add up, a position that isn't finite, positions at an
/// address no block has, no layout at all.
pub fn row_mesh_skips() -> Row {
    let row = "mesh skips";
    let cube = cube_spec();
    let mut n = 0;
    for (layout, l, version) in LAYOUTS {
        let w = layout.word();
        let mut cases: Vec<(&str, MeshSpec, MeshSkip)> = Vec::new();
        let mut s = cube.clone();
        s.faces[2][1] = 8;
        cases.push(("a corner past the vertices", s, MeshSkip::CornerOutOfRange));
        let mut s = cube.clone();
        s.faces.push(vec![0, 1]);
        cases.push(("a polygon of 2 corners", s, MeshSkip::FaceTooSmall));
        let mut s = cube.clone();
        s.offsets = Some(vec![0, 4, 8, 12, 16, 20, 23]);
        cases.push(("offsets short of the corners", s, MeshSkip::Offsets));
        let mut s = cube.clone();
        s.positions[3][1] = f32::INFINITY;
        cases.push((
            "a position that isn't finite",
            s,
            MeshSkip::PositionNotFinite,
        ));
        let s = MeshSpec {
            dangling_positions: true,
            ..cube.clone()
        };
        cases.push((
            "positions nowhere",
            s,
            MeshSkip::ArrayUnresolved("positions"),
        ));
        for (what, spec, want) in cases {
            let got = read_one(version, l, &spec);
            step(
                row,
                &format!("{w}: {what}"),
                got == Err(want),
                format!("{:?}", got.map(|(l, _)| l)),
            )?;
            step(
                row,
                &format!("{w}: {what}, its word"),
                want.word().starts_with("mesh_invalid:"),
                want.word(),
            )?;
            n += 1;
        }
        let bytes = mesh_file(version, l, &[("M", cube.clone(), MeshLayouts::NONE)]);
        let (file, raw, me) = read_meshes(&bytes);
        let got = mesh::read_mesh(&file, &raw, me[0], 4096, &mut TriCounts::default());
        step(
            row,
            &format!("{w}: no layout"),
            got == Err(MeshSkip::LayoutUnknown),
            format!("{got:?}"),
        )?;
        n += 1;
    }
    println!("mesh skips: {n} cases");
    Ok(())
}

// ---- objects and the read --------------------------------------------------------------------

use k2_mpc_blend::meshread::check::Checked;
use k2_mpc_blend::meshread::{mesh_read, MeshRead};
use k2_mpc_blend::testgen::{gzip, zstd_frames, Driven, ObjectSpec};
use k2_mpc_blend::{write as write_file, Change, Edit};

/// A mesh object of mesh `data`, rotating by XYZ Euler angles.
pub fn mesh_obj(data: &str) -> ObjectSpec {
    ObjectSpec {
        data: Some(format!("ME:{data}")),
        ty: 1,
        rotmode: 1,
        ..ObjectSpec::default()
    }
}

/// A builder for files with meshes (attribute layers, 4.03).
pub fn objects_builder() -> SceneBuilder {
    SceneBuilder::meshes(403, MeshLayouts::LAYERS)
}

/// The read of `bytes`, which must pass the strict checker.
pub fn read_ok(row: &str, bytes: &[u8]) -> Result<(MeshRead, Checked), String> {
    let r =
        mesh_read(bytes, &MeshReadLimits::default()).map_err(|e| format!("{row}, read: {e}"))?;
    let c = check(&r.k2scene, &MeshReadLimits::default())
        .map_err(|e| format!("{row}, the checker: {e}"))?;
    Ok((r, c))
}

fn ids(c: &Checked) -> Vec<&str> {
    c.scene.objects.iter().map(|o| o.id.as_str()).collect()
}

/// Two scenes, `A` holding `a` and `B` holding `b`; `global` names the current one (as
/// [`SceneBuilder::file_global`]: `Some("")` an address nothing has), `None` writes no global
/// block.
fn two_scenes(global: Option<Option<&str>>) -> Vec<u8> {
    let mut b = objects_builder();
    if let Some(g) = global {
        b.file_global(g);
    }
    b.scene("A", &["a"], &[], None);
    b.scene("B", &["b"], &[], None);
    b.object("a", &mesh_obj("Cube"));
    b.object("b", &mesh_obj("Cube"));
    b.mesh_with("Cube", &cube_spec(), MeshLayouts::LAYERS);
    b.bytes()
}

/// The scene drawn is the file's current one; without one (no global block, a null or
/// dangling pointer), the first in file order, and the report says which.
pub fn row_scene_choice() -> Row {
    let row = "scene choice";
    let cases = [
        ("current B", Some(Some("B")), "B", "current", "b"),
        ("current A", Some(Some("A")), "A", "current", "a"),
        ("no global block", None, "A", "first", "a"),
        ("a null current", Some(None), "A", "first", "a"),
        ("a dangling current", Some(Some("")), "A", "first", "a"),
    ];
    for (what, g, scene, from, obj) in cases {
        let (r, c) = read_ok(row, &two_scenes(g))?;
        let got = (r.report.scene.as_deref(), r.report.scene_from, ids(&c));
        step(
            row,
            what,
            got == (Some(scene), from, vec![obj]),
            format!("{got:?}"),
        )?;
    }
    println!("scene choice: {} cases", cases.len());
    Ok(())
}

/// Three objects sharing one mesh and one with its own: two meshes, in the meshes' file order
/// (the own mesh's block first), each written once; the objects in file order.
pub fn row_instancing() -> Row {
    let row = "instancing";
    let mut b = objects_builder();
    b.file_global(Some("S"));
    b.scene("S", &["s1", "s2", "s3", "own"], &[], None);
    for o in ["s1", "s2", "s3"] {
        b.object(o, &mesh_obj("Shared"));
    }
    b.object("own", &mesh_obj("Own"));
    let mut tri = cube_spec();
    tri.faces.truncate(1);
    b.mesh_with("Own", &tri, MeshLayouts::LAYERS);
    b.mesh_with("Shared", &cube_spec(), MeshLayouts::LAYERS);
    let (r, c) = read_ok(row, &b.bytes())?;
    step(
        row,
        "two meshes",
        c.scene.meshes.len() == 2,
        c.scene.meshes.len(),
    )?;
    let m: Vec<u32> = c.scene.objects.iter().map(|o| o.mesh).collect();
    step(
        row,
        "the mesh of each",
        m == vec![1, 1, 1, 0],
        format!("{m:?}"),
    )?;
    step(
        row,
        "the objects",
        ids(&c) == vec!["s1", "s2", "s3", "own"],
        format!("{:?}", ids(&c)),
    )?;
    let t: Vec<usize> = c.scene.meshes.iter().map(|m| m.triangles.len()).collect();
    step(
        row,
        "each mesh's triangles",
        t == vec![2, 12],
        format!("{t:?}"),
    )?;
    step(
        row,
        "the report",
        (r.report.meshes, r.report.objects) == (2, 4),
        format!("{:?}", r.report),
    )?;
    println!("instancing: 4 objects on 2 meshes");
    Ok(())
}

/// Hidden objects stay in, marked: by their own flag (`self`), or held only through hidden
/// collections (`collection`); one held through a shown collection too is shown.
pub fn row_hidden() -> Row {
    let row = "hidden";
    let mut b = objects_builder();
    b.file_global(Some("S"));
    b.scene("S", &["Vis", "Own"], &["Off", "On"], None);
    b.collection_flagged("Off", &["InOff", "Both"], &["Inner"], 1);
    b.collection("Inner", &["Deep"], &[]);
    b.collection("On", &["Both"], &[]);
    b.object("Vis", &mesh_obj("Cube"));
    b.object(
        "Own",
        &ObjectSpec {
            restrict: 1,
            ..mesh_obj("Cube")
        },
    );
    b.object("InOff", &mesh_obj("Cube"));
    b.object("Both", &mesh_obj("Cube"));
    b.object("Deep", &mesh_obj("Cube"));
    b.mesh_with("Cube", &cube_spec(), MeshLayouts::LAYERS);
    let (r, c) = read_ok(row, &b.bytes())?;
    let got: Vec<(&str, Option<Hidden>)> = c
        .scene
        .objects
        .iter()
        .map(|o| (o.id.as_str(), o.hidden))
        .collect();
    let want = vec![
        ("Vis", None),
        ("Own", Some(Hidden::Itself)),
        ("InOff", Some(Hidden::Collection)),
        ("Both", None),
        ("Deep", Some(Hidden::Collection)),
    ];
    step(
        row,
        "each object, in file order",
        got == want,
        format!("{got:?}"),
    )?;
    step(row, "counted", r.report.hidden == 3, r.report.hidden)?;
    println!("hidden: {} objects, {} hidden", got.len(), r.report.hidden);
    Ok(())
}

fn translation(t: &[f32; 16]) -> [f32; 3] {
    [t[12], t[13], t[14]]
}

/// World matrices are composed from the stored location, rotation and scale through parents
/// and parent inverses, never taken from the world matrix the file caches (garbage here); a
/// location written since reads back composed.
pub fn row_world() -> Row {
    let row = "world";
    let garbage = Some([7.0f32; 16]);
    let mut inverse = IDENTITY;
    inverse[12] = -10.0;
    let mut b = objects_builder();
    b.file_global(Some("S"));
    b.scene(
        "S",
        &["Moved", "Parent", "Child", "Child2", "Turned"],
        &[],
        None,
    );
    b.object(
        "Moved",
        &ObjectSpec {
            loc: [1.0, 2.0, 3.0],
            obmat: garbage,
            ..mesh_obj("Cube")
        },
    );
    b.object(
        "Parent",
        &ObjectSpec {
            loc: [10.0, 0.0, 0.0],
            obmat: garbage,
            ..mesh_obj("Cube")
        },
    );
    b.object(
        "Child",
        &ObjectSpec {
            loc: [0.0, 1.0, 0.0],
            parent: Some("Parent".into()),
            parentinv: inverse,
            obmat: garbage,
            ..mesh_obj("Cube")
        },
    );
    b.object(
        "Child2",
        &ObjectSpec {
            loc: [0.0, 1.0, 0.0],
            parent: Some("Parent".into()),
            obmat: garbage,
            ..mesh_obj("Cube")
        },
    );
    b.object(
        "Turned",
        &ObjectSpec {
            rot: [0.0, 0.0, std::f32::consts::FRAC_PI_2],
            scale: [2.0, 2.0, 2.0],
            obmat: garbage,
            ..mesh_obj("Cube")
        },
    );
    b.mesh_with("Cube", &cube_spec(), MeshLayouts::LAYERS);
    let bytes = b.bytes();
    let (_, c) = read_ok(row, &bytes)?;
    let t: Vec<[f32; 3]> = c
        .scene
        .objects
        .iter()
        .map(|o| translation(&o.transform))
        .collect();
    let want = vec![
        [1.0, 2.0, 3.0],
        [10.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [10.0, 1.0, 0.0],
        [0.0, 0.0, 0.0],
    ];
    step(row, "translations", t == want, format!("{t:?}"))?;
    let turned = c.scene.objects[4].transform;
    let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
    // Z by 90 degrees, scale 2: X goes to 2Y, Y to -2X
    let ok = close(turned[0], 0.0)
        && close(turned[1], 2.0)
        && close(turned[4], -2.0)
        && close(turned[5], 0.0)
        && close(turned[10], 2.0)
        && turned[15] == 1.0;
    step(row, "rotation and scale", ok, format!("{turned:?}"))?;
    // a location written since: the cached matrix is as stale as before
    let moved = write_file(
        &bytes,
        &[Edit::new("@ob:Moved", Change::Location([4.0, 5.0, 6.0]))],
        &ReadLimits::default(),
    )
    .map_err(|e| format!("{row}, the write: {e}"))?;
    let (_, c) = read_ok(row, &moved.bytes)?;
    let t = translation(&c.scene.objects[0].transform);
    step(row, "after a write", t == [4.0, 5.0, 6.0], format!("{t:?}"))?;
    println!("world: 5 objects composed, one after a write");
    Ok(())
}

/// Mesh objects the read can't draw honestly are left out and named, in file order; others of
/// the scene (a light) and objects of another scene aren't listed.
pub fn row_left_out() -> Row {
    let row = "left out";
    let mut b = objects_builder();
    b.file_global(Some("S"));
    b.library("lib", "//lib.blend");
    let names = [
        "Fine", "Linked", "Delta", "Bone", "BadMesh", "NoFaces", "Lamp", "Other",
    ];
    b.scene("S", &names[..7], &[], None);
    b.scene("T", &["Other"], &[], None);
    b.object("Fine", &mesh_obj("Cube"));
    b.object(
        "Linked",
        &ObjectSpec {
            lib: Some("lib".into()),
            ..mesh_obj("Cube")
        },
    );
    b.object(
        "Delta",
        &ObjectSpec {
            delta_loc: [0.0, 0.0, 1.0],
            ..mesh_obj("Cube")
        },
    );
    b.object(
        "Bone",
        &ObjectSpec {
            parent: Some("Fine".into()),
            partype: 7,
            ..mesh_obj("Cube")
        },
    );
    b.object("BadMesh", &mesh_obj("Bad"));
    b.object("NoFaces", &mesh_obj("Loose"));
    b.object(
        "Lamp",
        &ObjectSpec {
            data: Some("LA:L".into()),
            ty: 10,
            ..ObjectSpec::default()
        },
    );
    b.object("Other", &mesh_obj("Cube"));
    b.light("L", 1.0);
    b.mesh_with("Cube", &cube_spec(), MeshLayouts::LAYERS);
    let mut bad = cube_spec();
    bad.faces[0][0] = 99;
    b.mesh_with("Bad", &bad, MeshLayouts::LAYERS);
    let loose = MeshSpec {
        positions: cube_spec().positions,
        ..MeshSpec::default()
    };
    b.mesh_with("Loose", &loose, MeshLayouts::LAYERS);
    let (r, c) = read_ok(row, &b.bytes())?;
    step(
        row,
        "drawn",
        ids(&c) == vec!["Fine"],
        format!("{:?}", ids(&c)),
    )?;
    let got: Vec<(&str, &str)> = r
        .report
        .skipped
        .iter()
        .map(|s| (s.name.as_str(), s.reason))
        .collect();
    let want = vec![
        ("Linked", "linked_object"),
        ("Delta", "delta_transform"),
        ("Bone", "parent_not_object"),
        ("BadMesh", "mesh_invalid:corner_out_of_range"),
        ("NoFaces", "no_triangles"),
    ];
    step(row, "the reasons", got == want, format!("{got:?}"))?;
    println!("left out: {} objects, each with its reason", got.len());
    Ok(())
}

/// Modifiers by the file's own words, in order, `modifiers_not_applied` with them; at most 256
/// (the rest counted); none: absent. Driven transforms are marked not evaluated.
pub fn row_modifiers_and_driven() -> Row {
    let row = "modifiers";
    let mut b = objects_builder();
    b.file_global(Some("S"));
    b.scene("S", &["Two", "Many", "None", "Walker"], &[], None);
    b.object(
        "Two",
        &ObjectSpec {
            modifiers: vec!["SubsurfModifierData".into(), "MirrorModifierData".into()],
            ..mesh_obj("Cube")
        },
    );
    b.object(
        "Many",
        &ObjectSpec {
            modifiers: vec!["ArrayModifierData".into(); 300],
            ..mesh_obj("Cube")
        },
    );
    b.object("None", &mesh_obj("Cube"));
    b.object(
        "Walker",
        &ObjectSpec {
            driven: Driven::Action,
            ..mesh_obj("Cube")
        },
    );
    b.action();
    b.mesh_with("Cube", &cube_spec(), MeshLayouts::LAYERS);
    let (r, c) = read_ok(row, &b.bytes())?;
    let o = &c.scene.objects;
    step(row, "four objects", o.len() == 4, o.len())?;
    step(
        row,
        "two, in order",
        o[0].modifiers == vec!["Subsurf", "Mirror"] && o[0].modifiers_not_applied,
        format!("{:?}", o[0]),
    )?;
    step(
        row,
        "300, cut at 256",
        o[1].modifiers.len() == 256
            && o[1].modifiers.iter().all(|m| m == "Array")
            && r.report.modifiers_truncated == 1,
        format!("{} {}", o[1].modifiers.len(), r.report.modifiers_truncated),
    )?;
    step(
        row,
        "none",
        o[2].modifiers.is_empty() && !o[2].modifiers_not_applied,
        format!("{:?}", o[2]),
    )?;
    let flags: Vec<bool> = o.iter().map(|o| o.transform_not_evaluated).collect();
    step(
        row,
        "driven",
        flags == vec![false, false, false, true],
        format!("{flags:?}"),
    )?;
    println!("modifiers: 2 named, 300 cut to 256, none absent; 1 driven");
    Ok(())
}

/// The same bytes read twice, and the same file stored plain, gzip and zstd, give the same
/// output byte for byte.
pub fn row_determinism() -> Row {
    let row = "determinism";
    let mut b = objects_builder();
    b.file_global(Some("S"));
    b.scene("S", &["A", "B"], &["C"], None);
    b.collection_flagged("C", &["D"], &[], 1);
    b.object(
        "A",
        &ObjectSpec {
            loc: [0.1, 0.2, 0.3],
            rot: [0.4, 0.5, 0.6],
            modifiers: vec!["SubsurfModifierData".into()],
            ..mesh_obj("Cube")
        },
    );
    b.object(
        "B",
        &ObjectSpec {
            parent: Some("A".into()),
            ..mesh_obj("Concave")
        },
    );
    b.object("D", &mesh_obj("Cube"));
    b.mesh_with("Cube", &cube_spec(), MeshLayouts::LAYERS);
    let (c, _) = concave();
    let spec = MeshSpec {
        positions: c.positions.clone(),
        faces: (0..c.faces()).map(|i| c.face(i).to_vec()).collect(),
        ..MeshSpec::default()
    };
    b.mesh_with("Concave", &spec, MeshLayouts::LAYERS);
    let plain = b.bytes();
    let (first, _) = read_ok(row, &plain)?;
    let (again, _) = read_ok(row, &plain)?;
    step(row, "twice", first == again, "differs")?;
    for (how, bytes) in [("gzip", gzip(&plain)), ("zstd", zstd_frames(&plain, 1000))] {
        let (r, _) = read_ok(row, &bytes)?;
        step(row, how, r.k2scene == first.k2scene, "differs")?;
    }
    step(
        row,
        "objects",
        first.report.objects == 3,
        first.report.objects,
    )?;
    println!(
        "determinism: {} bytes, the same 4 ways",
        first.k2scene.len()
    );
    Ok(())
}

/// `scene meshes`: listed in the registry, and its answer the report's lines.
pub fn row_verb() -> Row {
    use k2_mpc_blend::verbs::{meshes, registry, VERBS};
    let row = "verb";
    step(
        row,
        "listed",
        VERBS.iter().any(|v| v.name == "scene meshes"),
        "no scene meshes",
    )?;
    registry().map_err(|e| format!("{row}, the registry: {e:?}"))?;
    let r = meshes(&two_scenes(Some(Some("B"))), &MeshReadLimits::default())
        .map_err(|e| format!("{row}: {e}"))?;
    let lines = r.report.lines();
    let want = vec![
        "scene B (current): 1 meshes, 1 objects, 0 cameras, 0 hidden, 0 left out, 0 collection \
         instances not drawn"
            .to_owned(),
        "object b\t@obj:2\tmesh 0\tvertices 8\ttriangles 12".to_owned(),
    ];
    step(row, "the lines", lines == want, format!("{lines:?}"))?;
    println!("verb: {} lines", lines.len());
    Ok(())
}

// ---- keys, cameras, the heap, counts after compaction, mesh hashes, hidden by -----------------

use k2_mpc_base::elem::Key;
use k2_mpc_base::index::Digest;
use k2_mpc_base::session::Pairing;
use k2_mpc_blend::meshread::check::{parse_json, J};
use k2_mpc_blend::meshread::hidden::{HiddenBy, HiddenWalk};
use k2_mpc_blend::meshread::mesh_read_paired;
use k2_mpc_blend::testgen::{CameraSpec, Render};

/// A pairing of `objs` keys only.
fn pairing(keys: &[&str]) -> Pairing {
    Pairing {
        roots: vec![(
            "objs".to_owned(),
            keys.iter().map(|k| Key::new(*k).expect("a key")).collect(),
        )],
    }
}

/// A file whose drawn objects aren't the first: a light, `A`, an object of another scene, `B`.
fn keyed_file() -> Vec<u8> {
    let mut b = objects_builder();
    b.file_global(Some("S"));
    b.scene("S", &["Lamp", "A", "B"], &[], None);
    b.scene("T", &["Elsewhere"], &[], None);
    b.object(
        "Lamp",
        &ObjectSpec {
            data: Some("LA:L".into()),
            ty: 10,
            ..ObjectSpec::default()
        },
    );
    b.object("A", &mesh_obj("Cube"));
    b.object("Elsewhere", &mesh_obj("Cube"));
    b.object("B", &mesh_obj("Cube"));
    b.light("L", 1.0);
    b.mesh_with("Cube", &cube_spec(), MeshLayouts::LAYERS);
    b.bytes()
}

fn keys_of(c: &Checked) -> Vec<&str> {
    c.scene.objects.iter().map(|o| o.key.as_str()).collect()
}

/// Every object is written with its address: the live key an import mints (`o1`.. in file
/// order, the light and the other scene's object counted), or the pairing's; a pairing that
/// doesn't fit is refused, typed, with no key guessed.
pub fn row_keys() -> Row {
    let row = "keys";
    let bytes = keyed_file();
    let (r, c) = read_ok(row, &bytes)?;
    step(
        row,
        "control: two drawn",
        ids(&c) == vec!["A", "B"],
        format!("{:?}", ids(&c)),
    )?;
    step(
        row,
        "minted",
        keys_of(&c) == vec!["@obj:2", "@obj:4"],
        format!("{:?}", keys_of(&c)),
    )?;
    let reported: Vec<&str> = r.report.drawn.iter().map(|d| d.key.as_str()).collect();
    step(
        row,
        "reported",
        reported == keys_of(&c),
        format!("{reported:?}"),
    )?;
    let l = MeshReadLimits::default();
    let p = pairing(&["o7", "o3", "o9", "o12"]);
    let paired = mesh_read_paired(&bytes, Some(&p), &l).map_err(|e| format!("{row}: {e}"))?;
    let pc = check(&paired.k2scene, &l).map_err(|e| format!("{row}, paired: {e}"))?;
    step(
        row,
        "paired",
        keys_of(&pc) == vec!["@obj:3", "@obj:12"],
        format!("{:?}", keys_of(&pc)),
    )?;
    let same = mesh_read_paired(&bytes, None, &l).map_err(|e| format!("{row}: {e}"))?;
    step(row, "no pairing is the minted keys", same == r, "differs")?;
    let mut refused = 0;
    for (what, keys) in [
        ("too few keys", vec!["o1", "o2", "o3"]),
        ("too many keys", vec!["o1", "o2", "o3", "o4", "o5"]),
        ("a key that isn't oN", vec!["o1", "c2", "o3", "o4"]),
        ("a key twice", vec!["o1", "o2", "o2", "o4"]),
    ] {
        let got = mesh_read_paired(&bytes, Some(&pairing(&keys)), &l);
        step(
            row,
            what,
            matches!(&got, Err(MeshReadError::Pairing { .. }))
                && got.as_ref().err().map(|e| e.code()) == Some("pairing_mismatch"),
            format!("{:?}", got.map(|r| r.report.drawn.len())),
        )?;
        refused += 1;
    }
    println!("keys: 2 objects minted and paired, {refused} pairings refused");
    Ok(())
}

/// A file with a mesh object first, then cameras: `Wide` (auto, at 1,2,3 turned a quarter
/// about X: looking along +Y, up +Z), `Tall` (vertical, the scene's camera), `Flat`
/// (horizontal), an orthographic one, one of an unknown sensor fit, and one of another scene.
fn camera_file(render: Render) -> Vec<u8> {
    let mut b = objects_builder();
    b.render = render;
    b.file_global(Some("S"));
    let names = ["Mesh", "Wide", "Tall", "Flat", "Ortho", "Odd"];
    b.scene("S", &names, &[], Some("Tall"));
    b.scene("T", &["Elsewhere"], &[], None);
    b.object("Mesh", &mesh_obj("Cube"));
    let cam = |data: &str| ObjectSpec {
        data: Some(format!("CA:{data}")),
        ty: 11,
        rotmode: 1,
        ..ObjectSpec::default()
    };
    b.object(
        "Wide",
        &ObjectSpec {
            loc: [1.0, 2.0, 3.0],
            rot: [std::f32::consts::FRAC_PI_2, 0.0, 0.0],
            ..cam("Wide")
        },
    );
    for n in ["Tall", "Flat", "Ortho", "Odd", "Elsewhere"] {
        b.object(n, &cam(n));
    }
    let spec = |lens: f32, fit: u8, ty: u8| CameraSpec {
        lens,
        fit,
        ty,
        ..CameraSpec::default()
    };
    b.camera_with("Wide", &spec(50.0, 0, 0));
    b.camera_with("Tall", &spec(35.0, 2, 0));
    b.camera_with("Flat", &spec(85.0, 1, 0));
    b.camera_with("Ortho", &spec(50.0, 0, 1));
    b.camera_with("Odd", &spec(50.0, 7, 0));
    b.camera_with("Elsewhere", &spec(50.0, 0, 0));
    b.mesh_with("Cube", &cube_spec(), MeshLayouts::LAYERS);
    b.bytes()
}

fn near(a: f32, b: f64) -> bool {
    (f64::from(a) - b).abs() <= 1e-6 * b.abs().max(1.0)
}

fn near3(a: [f32; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| near(*x, y))
}

/// Cameras: perspective ones of the scene drawn, in file order, keyed by their objects' live
/// keys; where each is and looks (composed like an object); the vertical field of view from the
/// lens, the sensor, its fit and the render size with its pixel aspect; the scene's own camera
/// active; the others left out and named. A portrait render takes the sensor's width as the
/// height under the automatic fit.
pub fn row_cameras() -> Row {
    let row = "cameras";
    let (r, c) = read_ok(row, &camera_file(Render::default()))?;
    let cams = &c.scene.cameras;
    let names: Vec<&str> = cams.iter().map(|c| c.id.as_str()).collect();
    step(
        row,
        "the cameras",
        names == vec!["Wide", "Tall", "Flat"],
        format!("{names:?}"),
    )?;
    let keys: Vec<&str> = cams.iter().map(|c| c.key.as_str()).collect();
    step(
        row,
        "their keys",
        keys == vec!["@cam:2", "@cam:3", "@cam:4"],
        format!("{keys:?}"),
    )?;
    let left: Vec<(&str, &str)> = r
        .report
        .skipped
        .iter()
        .map(|s| (s.name.as_str(), s.reason))
        .collect();
    step(
        row,
        "left out",
        left == vec![
            ("Ortho", "camera_not_perspective"),
            ("Odd", "camera_sensor_fit_unknown"),
        ],
        format!("{left:?}"),
    )?;
    let listed: Vec<(&str, Option<&str>, &str)> = c
        .scene
        .cameras_left_out
        .iter()
        .map(|l| (l.name.as_str(), l.key.as_deref(), l.reason.as_str()))
        .collect();
    step(
        row,
        "left out, listed in the output",
        listed
            == vec![
                ("Ortho", Some("@cam:5"), "camera_not_perspective"),
                ("Odd", Some("@cam:6"), "camera_sensor_fit_unknown"),
            ],
        format!("{listed:?}"),
    )?;
    let aspect: f64 = 1920.0 / 1080.0;
    let want = [
        2.0 * (36.0 / (2.0 * 50.0 * aspect)).atan(),
        2.0 * (24.0 / (2.0 * 35.0f64)).atan(),
        2.0 * (36.0 / (2.0 * 85.0 * aspect)).atan(),
    ];
    for (cam, w) in cams.iter().zip(want) {
        step(
            row,
            &format!("{}: yfov", cam.id),
            near(cam.yfov, w),
            format!("{} vs {w}", cam.yfov),
        )?;
        step(
            row,
            &format!("{}: aspect", cam.id),
            cam.aspect.is_some_and(|a| near(a, aspect)),
            format!("{:?}", cam.aspect),
        )?;
    }
    let wide = &cams[0];
    step(
        row,
        "Wide: where it is and looks",
        near3(wide.position, [1.0, 2.0, 3.0])
            && wide.forward.is_some_and(|f| near3(f, [0.0, 1.0, 0.0]))
            && near3(wide.up, [0.0, 0.0, 1.0])
            && wide.target.is_none(),
        format!("{wide:?}"),
    )?;
    let active: Vec<Option<bool>> = cams.iter().map(|c| c.active).collect();
    step(
        row,
        "the scene's camera active",
        active == vec![Some(false), Some(true), Some(false)],
        format!("{active:?}"),
    )?;
    let fits: Vec<Option<SensorFit>> = cams.iter().map(|c| c.sensor_fit).collect();
    step(
        row,
        "the fits",
        fits == vec![
            Some(SensorFit::Auto),
            Some(SensorFit::Vertical),
            Some(SensorFit::Horizontal),
        ],
        format!("{fits:?}"),
    )?;
    step(
        row,
        "lens, sensor, near clip",
        cams[1].lens_mm == Some(35.0)
            && cams[1].sensor_mm == Some([36.0, 24.0])
            && cams[1].znear == Some(0.1),
        format!("{:?}", cams[1]),
    )?;
    let lines = r.report.lines();
    step(
        row,
        "a line per camera",
        lines
            .iter()
            .any(|l| l.starts_with("camera Tall\t@cam:3\t") && l.ends_with("\tactive")),
        format!("{lines:?}"),
    )?;
    // a portrait render with wide pixels: the sensor's width across the height
    let portrait = Render {
        xsch: 1080,
        ysch: 1920,
        xasp: 2.0,
        yasp: 1.0,
    };
    let (_, c) = read_ok(row, &camera_file(portrait))?;
    let a = 1080.0 * 2.0 / 1920.0;
    let w = &c.scene.cameras[0];
    step(
        row,
        "portrait: the aspect with the pixel aspect",
        w.aspect.is_some_and(|x| near(x, a)),
        format!("{:?}", w.aspect),
    )?;
    let p = 1080.0 / (1920.0 * 2.0);
    let portrait = Render {
        xsch: 1080,
        ysch: 1920,
        xasp: 1.0,
        yasp: 2.0,
    };
    let (_, c) = read_ok(row, &camera_file(portrait))?;
    let w = &c.scene.cameras[0];
    let want = 2.0 * (36.0 / (2.0 * 50.0f64)).atan();
    step(
        row,
        "portrait: auto takes the width as the height",
        near(w.yfov, want) && w.aspect.is_some_and(|x| near(x, p)),
        format!("{} vs {want}, {:?}", w.yfov, w.aspect),
    )?;
    println!("cameras: 3 written, 2 left out, a portrait render twice");
    Ok(())
}

/// The heap a read needs is worked out before any mesh array or the output: a read whose
/// figure is over the limit is refused (`too_large`, `heap`), one at it is read.
pub fn row_heap_limit() -> Row {
    let row = "heap limit";
    let bytes = keyed_file();
    let (r, _) = read_ok(row, &bytes)?;
    let need = r.report.heap;
    step(
        row,
        "control: a figure at least the file",
        need >= bytes.len() as u64,
        need,
    )?;
    let mut l = MeshReadLimits::default();
    l.max_heap_bytes = need - 1;
    let got = mesh_read(&bytes, &l);
    step(
        row,
        "refused one under",
        matches!(
            &got,
            Err(MeshReadError::TooLarge { what: "heap", got: g, max: m }) if *g == need && *m == need - 1
        ) && got.as_ref().err().map(|e| e.code()) == Some("too_large"),
        format!("{:?}", got.map(|r| r.report.heap)),
    )?;
    l.max_heap_bytes = need;
    let at = mesh_read(&bytes, &l);
    step(
        row,
        "read at it",
        at.as_ref().map(|a| &a.k2scene) == Ok(&r.k2scene),
        format!("{:?}", at.map(|a| a.report.heap)),
    )?;
    println!("heap limit: {need} bytes worked out, refused one under, read at it");
    Ok(())
}

/// The vertex limits count what is written, after the loose vertices are dropped: a cube with
/// a loose vertex (9 stored, 8 used) is read at a limit of 8, and refused at 7.
pub fn row_limits_after_compaction() -> Row {
    let row = "limits after compaction";
    let mut spec = cube_spec();
    spec.positions.push([9.0, 9.0, 9.0]);
    let mut b = objects_builder();
    b.file_global(Some("S"));
    b.scene("S", &["C"], &[], None);
    b.object("C", &mesh_obj("Cube"));
    b.mesh_with("Cube", &spec, MeshLayouts::LAYERS);
    let bytes = b.bytes();
    let mut l = MeshReadLimits::default();
    l.max_vertices = 8;
    l.max_drawn_vertices = 8;
    let got = mesh_read(&bytes, &l);
    step(
        row,
        "8 used of 9 stored, at a limit of 8",
        got.as_ref()
            .is_ok_and(|r| r.report.drawn.first().map(|d| d.vertices) == Some(8)),
        format!("{:?}", got.map(|r| r.report.drawn)),
    )?;
    l.max_vertices = 7;
    let got = mesh_read(&bytes, &l);
    step(
        row,
        "refused at 7",
        matches!(
            got,
            Err(MeshReadError::TooLarge {
                what: "vertices",
                got: 8,
                max: 7
            })
        ),
        format!("{:?}", got.map(|r| r.report.drawn)),
    )?;
    l.max_vertices = 8;
    l.max_drawn_vertices = 7;
    let got = mesh_read(&bytes, &l);
    step(
        row,
        "drawn: refused at 7",
        matches!(
            got,
            Err(MeshReadError::TooLarge {
                what: "drawn vertices",
                got: 8,
                max: 7
            })
        ),
        format!("{:?}", got.map(|r| r.report.drawn)),
    )?;
    println!("limits after compaction: read at 8, refused at 7 stored and drawn");
    Ok(())
}

/// SHA-256 of a mesh's counts and its data, as the layout says, worked out here.
fn hash_of(m: &SceneMesh) -> String {
    let mut b = Vec::new();
    b.extend((m.positions.len() as u32).to_le_bytes());
    b.extend((m.triangles.len() as u32).to_le_bytes());
    for p in m.positions.iter().flatten() {
        b.extend(p.to_le_bytes());
    }
    for i in m.triangles.iter().flatten() {
        b.extend(i.to_le_bytes());
    }
    Digest::of(&b)
        .as_bytes()
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// Each mesh's hash is SHA-256 of its counts and its data as written; objects sharing a mesh
/// share it, the report gives it with each object; the same geometry in another file (another
/// version and layout, other names, saved elsewhere in memory) has the same hash, other
/// geometry another.
pub fn row_mesh_hash() -> Row {
    let row = "mesh hash";
    let mut b = objects_builder();
    b.file_global(Some("S"));
    b.scene("S", &["s1", "s2", "own"], &[], None);
    b.object("s1", &mesh_obj("Shared"));
    b.object("s2", &mesh_obj("Shared"));
    b.object("own", &mesh_obj("Own"));
    let mut tri = cube_spec();
    tri.faces.truncate(1);
    b.mesh_with("Own", &tri, MeshLayouts::LAYERS);
    b.mesh_with("Shared", &cube_spec(), MeshLayouts::LAYERS);
    let (r, c) = read_ok(row, &b.bytes())?;
    let mine: Vec<String> = c.scene.meshes.iter().map(hash_of).collect();
    step(
        row,
        "the counts' and data's",
        c.hashes == mine,
        format!("{:?} vs {mine:?}", c.hashes),
    )?;
    let drawn: Vec<&str> = r
        .report
        .drawn
        .iter()
        .map(|d| d.mesh_hash.as_str())
        .collect();
    step(
        row,
        "shared by instances",
        drawn.len() == 3 && drawn[0] == drawn[1] && drawn[0] != drawn[2],
        format!("{drawn:?}"),
    )?;
    step(
        row,
        "each object's mesh's",
        drawn[0] == c.hashes[1] && drawn[2] == c.hashes[0],
        format!("{drawn:?} vs {:?}", c.hashes),
    )?;
    let mut b = SceneBuilder::meshes(306, MeshLayouts::STRUCTS);
    b.shift = 0x40_0000;
    b.file_global(Some("Other"));
    b.scene("Other", &["Lone"], &[], None);
    b.object(
        "Lone",
        &ObjectSpec {
            loc: [5.0, 0.0, 0.0],
            ..mesh_obj("Box")
        },
    );
    b.mesh_with("Box", &cube_spec(), MeshLayouts::STRUCTS);
    let (_, other) = read_ok(row, &b.bytes())?;
    step(
        row,
        "the same geometry elsewhere",
        other.hashes == vec![c.hashes[1].clone()],
        format!("{:?}", other.hashes),
    )?;
    let mut moved = cube_spec();
    moved.positions[0][0] = -1.5;
    let mut b = objects_builder();
    b.file_global(Some("S"));
    b.scene("S", &["s1"], &[], None);
    b.object("s1", &mesh_obj("Shared"));
    b.mesh_with("Shared", &moved, MeshLayouts::LAYERS);
    let (_, changed) = read_ok(row, &b.bytes())?;
    step(
        row,
        "other geometry",
        changed.hashes.len() == 1 && changed.hashes[0] != c.hashes[1],
        format!("{:?}", changed.hashes),
    )?;
    println!("mesh hash: 2 meshes, 3 objects, the same cube in another file");
    Ok(())
}

/// The hidden-by rule against the shared vectors (`tests/data/hidden-by.json`): every case,
/// every object's value.
pub fn row_hidden_by_vectors() -> Row {
    let row = "hidden by";
    let text = include_str!("../data/hidden-by.json");
    let top = parse_json(text).map_err(|e| format!("{row}: the vectors: {e}"))?;
    let Some(J::Arr(cases)) = top.get("cases") else {
        return Err(format!("{row}: the vectors: no cases"));
    };
    let s = |j: Option<&J>| match j {
        Some(J::Str(x)) => Ok(x.clone()),
        _ => Err(format!("{row}: the vectors: a string expected")),
    };
    let list = |j: Option<&J>| match j {
        Some(J::Arr(v)) => Ok(v.clone()),
        _ => Err(format!("{row}: the vectors: a list expected")),
    };
    let mut objects = 0;
    let mut moved = 0;
    for case in cases {
        let name = s(case.get("name"))?;
        let colls = list(case.get("collections"))?;
        let keys: Vec<String> = colls
            .iter()
            .map(|c| s(c.get("key")))
            .collect::<Result<_, _>>()?;
        let at = |k: &str| keys.iter().position(|x| x == k);
        let mut children = Vec::new();
        let mut hidden = Vec::new();
        for c in &colls {
            let kids = list(c.get("children"))?
                .iter()
                .map(|k| s(Some(k)).map(|k| at(&k).unwrap_or(usize::MAX)))
                .collect::<Result<Vec<_>, _>>()?;
            children.push(kids);
            hidden.push(c.get("hidden") == Some(&J::Bool(true)));
        }
        // a collection moved live: out from under every parent, last under its new one
        if let Some(J::Arr(moves)) = case.get("moves") {
            for m in moves {
                let c = at(&s(m.get("collection"))?)
                    .ok_or(format!("{row}, {name}: a move's collection"))?;
                let to = at(&s(m.get("to"))?).ok_or(format!("{row}, {name}: a move's parent"))?;
                for kids in children.iter_mut() {
                    kids.retain(|&k| k != c);
                }
                children[to].push(c);
                moved += 1;
            }
        }
        let master = at(&s(case.get("master"))?).ok_or(format!("{row}, {name}: no master"))?;
        let walk = HiddenWalk::new(master, &children, &|c| hidden[c]);
        for o in list(case.get("objects"))? {
            let key = s(o.get("key"))?;
            let own = o.get("hidden") == Some(&J::Bool(true));
            let holders: Vec<usize> = list(o.get("in"))?
                .iter()
                .map(|k| s(Some(k)).map(|k| at(&k).unwrap_or(usize::MAX)))
                .collect::<Result<_, _>>()?;
            let got = match walk.of(own, &holders) {
                None => None,
                Some(HiddenBy::Itself) => Some("self".to_owned()),
                Some(HiddenBy::Collection(c)) => Some(format!("collection:{}", keys[c])),
            };
            let want = match case.get("expect").and_then(|e| e.get(&key)) {
                Some(J::Null) => None,
                Some(J::Str(x)) => Some(x.clone()),
                _ => return Err(format!("{row}, {name}: no expected value for {key}")),
            };
            step(
                row,
                &format!("{name}: {key}"),
                got == want,
                format!("{got:?} vs {want:?}"),
            )?;
            objects += 1;
        }
    }
    step(row, "every case", cases.len() == 13, cases.len())?;
    step(row, "every object", objects == 25, objects)?;
    step(row, "every move", moved == 2, moved)?;
    println!(
        "hidden by: {} cases, {objects} objects, {moved} moves",
        cases.len()
    );
    Ok(())
}

// ---------- world matrices composed in the test alone ----------

/// `a * b`, column-major 4x4, in `f64`.
fn mul64(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

const IDENTITY64: [f64; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// A rotation of `angle` radians about axis `axis` (0 x, 1 y, 2 z), column-major.
fn about(axis: usize, angle: f64) -> [f64; 16] {
    let (sn, cs) = angle.sin_cos();
    let mut m = IDENTITY64;
    let (i, j) = ((axis + 1) % 3, (axis + 2) % 3);
    m[i * 4 + i] = cs;
    m[i * 4 + j] = sn;
    m[j * 4 + i] = -sn;
    m[j * 4 + j] = cs;
    m
}

/// The rotation of a unit quaternion `w, x, y, z`, column-major.
fn quat64(q: [f32; 4]) -> [f64; 16] {
    let [w, x, y, z] = q.map(f64::from);
    let mut m = IDENTITY64;
    m[0] = 1.0 - 2.0 * (y * y + z * z);
    m[1] = 2.0 * (x * y + w * z);
    m[2] = 2.0 * (x * z - w * y);
    m[4] = 2.0 * (x * y - w * z);
    m[5] = 1.0 - 2.0 * (x * x + z * z);
    m[6] = 2.0 * (y * z + w * x);
    m[8] = 2.0 * (x * z + w * y);
    m[9] = 2.0 * (y * z - w * x);
    m[10] = 1.0 - 2.0 * (x * x + y * y);
    m
}

/// One object of [`row_world_composed`]: name, parent, location, rotation mode (`rotmode`),
/// Euler angles, quaternion, scale.
type Posed = (
    &'static str,
    Option<&'static str>,
    [f32; 3],
    i16,
    [f32; 3],
    [f32; 4],
    [f32; 3],
);

/// A chain `A` (XYZ) holding `B` (ZXY) holding `C` (a quaternion), and `D` (YZX) alone.
const POSED: [Posed; 4] = [
    (
        "A",
        None,
        [1.0, 2.0, 3.0],
        1,
        [0.3, -0.7, 1.1],
        [1.0, 0.0, 0.0, 0.0],
        [1.0, 2.0, 0.5],
    ),
    (
        "B",
        Some("A"),
        [0.5, -1.0, 0.25],
        5,
        [0.4, 0.9, -1.3],
        [1.0, 0.0, 0.0, 0.0],
        [0.5, 0.5, 2.0],
    ),
    (
        "C",
        Some("B"),
        [-2.0, 0.0, 1.5],
        0,
        [0.0; 3],
        [0.8, 0.2, -0.4, 0.4],
        [1.5, 1.0, 1.0],
    ),
    (
        "D",
        None,
        [4.0, -3.0, 0.0],
        4,
        [-0.6, 0.2, 2.4],
        [1.0, 0.0, 0.0, 0.0],
        [1.0, 1.0, 3.0],
    ),
];

/// The world matrix of `name` composed in `f64` from [`POSED`], in this row alone: translation,
/// then rotation (Euler in the order its mode names, the first letter applied first; or the
/// quaternion), then scale, under the parent's (identity parent inverses).
fn posed_world(name: &str) -> [f64; 16] {
    let p = POSED.iter().find(|p| p.0 == name).expect("posed");
    let (_, parent, loc, mode, rot, quat, scale) = *p;
    let mut m = IDENTITY64;
    m[12] = f64::from(loc[0]);
    m[13] = f64::from(loc[1]);
    m[14] = f64::from(loc[2]);
    let order: &[usize] = match mode {
        1 => &[0, 1, 2],
        2 => &[0, 2, 1],
        3 => &[1, 0, 2],
        4 => &[1, 2, 0],
        5 => &[2, 0, 1],
        6 => &[2, 1, 0],
        _ => &[],
    };
    if order.is_empty() {
        m = mul64(&m, &quat64(quat));
    }
    for &axis in order.iter().rev() {
        m = mul64(&m, &about(axis, f64::from(rot[axis])));
    }
    let mut s = IDENTITY64;
    for k in 0..3 {
        s[k * 4 + k] = f64::from(scale[k]);
    }
    m = mul64(&m, &s);
    match parent {
        Some(pn) => mul64(&posed_world(pn), &m),
        None => m,
    }
}

/// The mesh read's world matrices at 5.03 equal a composition in this row alone, in `f64`: Euler
/// rotations in three orders (XYZ, ZXY, YZX), a quaternion, scales that differ by axis, and a
/// two-level parent chain.
pub fn row_world_composed() -> Row {
    let row = "world composed";
    let mut b = SceneBuilder::meshes(503, MeshLayouts::STORE);
    b.file_global(Some("S"));
    b.scene("S", &["A", "B", "C", "D"], &[], None);
    for (name, parent, loc, mode, rot, quat, scale) in POSED {
        b.object(
            name,
            &ObjectSpec {
                parent: parent.map(str::to_owned),
                loc,
                rot,
                quat,
                scale,
                rotmode: mode,
                ..mesh_obj("Cube")
            },
        );
    }
    b.mesh_with("Cube", &cube_spec(), MeshLayouts::STORE);
    let (_, c) = read_ok(row, &b.bytes())?;
    let mut n = 0;
    for o in &c.scene.objects {
        let want = posed_world(&o.id);
        for (i, (&a, &w)) in o.transform.iter().zip(&want).enumerate() {
            let close = (f64::from(a) - w).abs() <= 1e-5 * f64::from(a).abs().max(w.abs()).max(1.0);
            step(row, &o.id, close, format!("[{i}]: read {a}, composed {w}"))?;
        }
        n += 1;
    }
    step(row, "every object", n == POSED.len(), n)?;
    println!("world composed: {n} objects");
    Ok(())
}
