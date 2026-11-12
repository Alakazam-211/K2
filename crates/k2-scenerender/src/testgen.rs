//! Scenes generated in code, for the tests, the fuzz seeds and the daemon's
//! tests: a cube, a sphere, a grid of instances, a mirrored object, a large
//! mesh, and degenerate and hostile ones. No model file is read or shipped.

use crate::math::{normalize, IDENTITY};
use crate::scene::{Camera, Mesh, Object, Scene, UpAxis};

pub fn identity() -> [f32; 16] {
    crate::math::m4_to_f32(&IDENTITY)
}

/// Translate, then scale (column-major object-to-world).
pub fn placed(t: [f32; 3], s: [f32; 3]) -> [f32; 16] {
    let mut m = identity();
    m[0] = s[0];
    m[5] = s[1];
    m[10] = s[2];
    m[12] = t[0];
    m[13] = t[1];
    m[14] = t[2];
    m
}

pub fn object(id: &str, mesh: u32, color: [f32; 3], transform: [f32; 16], double_sided: Option<bool>) -> Object {
    Object {
        id: id.into(),
        mesh,
        base_color: [color[0], color[1], color[2], 1.0],
        double_sided,
        transform,
        hash: None,
        modifiers: Vec::new(),
        modifiers_not_applied: false,
    }
}

/// A unit cube centred on the origin, 12 triangles, counter-clockwise outward.
/// With `normals`, per-vertex normals point from the centre (smooth corners).
pub fn cube(normals: bool) -> Mesh {
    let p = [
        [-0.5, -0.5, -0.5],
        [0.5, -0.5, -0.5],
        [0.5, 0.5, -0.5],
        [-0.5, 0.5, -0.5],
        [-0.5, -0.5, 0.5],
        [0.5, -0.5, 0.5],
        [0.5, 0.5, 0.5],
        [-0.5, 0.5, 0.5],
    ];
    let idx = [
        [4, 5, 6],
        [4, 6, 7], // +z
        [1, 0, 3],
        [1, 3, 2], // -z
        [5, 1, 2],
        [5, 2, 6], // +x
        [0, 4, 7],
        [0, 7, 3], // -x
        [7, 6, 2],
        [7, 2, 3], // +y
        [0, 1, 5],
        [0, 5, 4], // -y
    ];
    let n = if normals { Some(p.iter().map(|v| normalize(*v)).collect()) } else { None };
    Mesh { positions: p.to_vec(), normals: n, indices: idx.to_vec(), hash: None }
}

/// A unit cube with a flat face per side (24 vertices, split normals): sharp edges.
pub fn cube_flat_normals() -> Mesh {
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
    ];
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    for (n, u, v) in faces {
        let base = positions.len() as u32;
        for (a, b) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            positions.push([n[0] * 0.5 + u[0] * a + v[0] * b, n[1] * 0.5 + u[1] * a + v[1] * b, n[2] * 0.5 + u[2] * a + v[2] * b]);
            normals.push(n);
        }
        indices.push([base, base + 1, base + 2]);
        indices.push([base, base + 2, base + 3]);
    }
    Mesh { positions, normals: Some(normals), indices, hash: None }
}

/// A UV sphere of radius 1: `segments` around, `rings` from pole to pole,
/// smooth normals.
pub fn sphere(segments: u32, rings: u32) -> Mesh {
    let segments = segments.max(3);
    let rings = rings.max(2);
    // Angles from a fixed rotation recurrence (no sin/cos: the generated
    // input is the same on every machine too).
    let unit = |n: u32, full: bool| -> Vec<(f64, f64)> {
        // cos and sin of k * (2 pi / n) (or pi / n), by repeated rotation from
        // a seed computed with the crate's own tan: t = tan(step / 2).
        let step_half = if full { std::f64::consts::PI / n as f64 } else { std::f64::consts::PI / (2.0 * n as f64) };
        let t = crate::trig::tan(step_half);
        let (c1, s1) = ((1.0 - t * t) / (1.0 + t * t), 2.0 * t / (1.0 + t * t));
        let mut out = Vec::with_capacity(n as usize + 1);
        let (mut c, mut s) = (1.0f64, 0.0f64);
        for _ in 0..=n {
            out.push((c, s));
            let nc = c * c1 - s * s1;
            let ns = s * c1 + c * s1;
            c = nc;
            s = ns;
        }
        out
    };
    let around = unit(segments, true);
    let down = unit(rings, false);
    let mut positions = Vec::new();
    for &(cr, sr) in &down {
        // cr = cos(polar), sr = sin(polar), polar from 0 (top) to pi.
        for &(ca, sa) in around.iter().take(segments as usize) {
            positions.push([(sr * ca) as f32, cr as f32, (sr * sa) as f32]);
        }
    }
    let normals = positions.iter().map(|p| normalize(*p)).collect();
    let mut indices = Vec::new();
    let row = segments;
    for r in 0..rings {
        for s in 0..segments {
            let a = r * row + s;
            let b = r * row + (s + 1) % segments;
            let c = (r + 1) * row + s;
            let d = (r + 1) * row + (s + 1) % segments;
            // Counter-clockwise seen from outside.
            if r > 0 {
                indices.push([a, b, c]);
            }
            if r + 1 < rings {
                indices.push([b, d, c]);
            }
        }
    }
    Mesh { positions, normals: Some(normals), indices, hash: None }
}

/// A height field of `n` x `n` quads (2 n^2 triangles) over [-1, 1]^2 in x
/// and z, with integer-made bumps, no normals (flat shaded).
pub fn terrain(n: u32) -> Mesh {
    let n = n.max(1);
    let mut positions = Vec::with_capacity(((n + 1) * (n + 1)) as usize);
    for j in 0..=n {
        for i in 0..=n {
            let x = -1.0 + 2.0 * i as f32 / n as f32;
            let z = -1.0 + 2.0 * j as f32 / n as f32;
            let h = ((i.wrapping_mul(7919) ^ j.wrapping_mul(104_729)) % 97) as f32 / 97.0 * 0.08 + 0.2 * (1.0 - x * x) * (1.0 - z * z);
            positions.push([x, h, z]);
        }
    }
    let mut indices = Vec::with_capacity((2 * n * n) as usize);
    let w = n + 1;
    for j in 0..n {
        for i in 0..n {
            let a = j * w + i;
            let b = a + 1;
            let c = a + w;
            let d = c + 1;
            indices.push([a, c, b]);
            indices.push([b, c, d]);
        }
    }
    Mesh { positions, normals: None, indices, hash: None }
}

fn scene(meshes: Vec<Mesh>, objects: Vec<Object>) -> Scene {
    Scene { up: UpAxis::Y, meshes, objects, cameras: Vec::new() }
}

/// One cube, back faces culled, terracotta.
pub fn cube_scene() -> Scene {
    scene(vec![cube_flat_normals()], vec![object("Cube", 0, [0.8, 0.3, 0.2], identity(), Some(false))])
}

/// One white cube with flat faces (no normals): the shades of its three
/// visible faces are fixed by the lights.
pub fn white_cube_scene() -> Scene {
    scene(vec![cube(false)], vec![object("Cube", 0, [1.0, 1.0, 1.0], identity(), Some(false))])
}

/// A smooth sphere.
pub fn sphere_scene() -> Scene {
    scene(vec![sphere(48, 24)], vec![object("Sphere", 0, [0.3, 0.5, 0.8], identity(), Some(false))])
}

/// An `n` x `n` grid of instances of two shared meshes (a cube and a sphere).
pub fn grid_scene(n: u32, z_up: bool) -> Scene {
    let mut objects = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let k = j * n + i;
            let mesh = k % 2;
            let (x, y) = (i as f32 * 1.5, j as f32 * 1.5);
            let t = if z_up { placed([x, y, 0.0], [1.0, 1.0, 1.0]) } else { placed([x, 0.0, y], [1.0, 1.0, 1.0]) };
            let c = [(i % 3) as f32 * 0.4 + 0.1, 0.6, (j % 3) as f32 * 0.3 + 0.2];
            objects.push(object(&format!("Item.{k:03}"), mesh, c, t, if k.is_multiple_of(3) { None } else { Some(false) }));
        }
    }
    let mut s = scene(vec![cube(true), sphere(16, 8)], objects);
    if z_up {
        s.up = UpAxis::Z;
    }
    s
}

/// The same asymmetric shape twice: as is on the left, mirrored in x
/// (negative determinant) on the right, both with back faces culled.
pub fn mirrored_scene() -> Scene {
    // An L of two boxes.
    let mut m = cube_flat_normals();
    let other = cube_flat_normals();
    let base = m.positions.len() as u32;
    m.positions.extend(other.positions.iter().map(|p| [p[0] + 1.0, p[1] * 0.5 - 0.25, p[2]]));
    if let (Some(n), Some(o)) = (m.normals.as_mut(), other.normals.as_ref()) {
        n.extend(o.iter().copied());
    }
    m.indices.extend(other.indices.iter().map(|t| [t[0] + base, t[1] + base, t[2] + base]));
    let left = placed([-1.5, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let right = placed([1.5, 0.0, 0.0], [-1.0, 1.0, 1.0]);
    scene(vec![m], vec![object("L", 0, [0.7, 0.7, 0.2], left, Some(false)), object("L.mirror", 0, [0.2, 0.7, 0.7], right, Some(false))])
}

/// One mesh of about `triangles` triangles (a square terrain).
pub fn large_scene(triangles: u64) -> Scene {
    let n = ((triangles / 2) as f64).sqrt() as u32;
    scene(vec![terrain(n)], vec![object("Terrain", 0, [0.6, 0.55, 0.45], placed([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]), None)])
}

/// A scene with one camera looking at a cube from the front-right.
pub fn camera_scene() -> Scene {
    let mut s = cube_scene();
    s.cameras.push(Camera {
        id: "Camera".into(),
        position: [2.0, 1.5, 3.0],
        target: Some([0.0, 0.0, 0.0]),
        forward: None,
        up: [0.0, 1.0, 0.0],
        yfov: 0.69,
        aspect: Some(16.0 / 9.0),
        znear: Some(0.1),
    });
    s
}

/// Degenerate and hostile scenes that must render (or refuse) without a
/// crash: no objects; an empty mesh; zero-area and collapsed triangles;
/// huge coordinates; a zero-scale object; a camera inside the geometry.
pub fn degenerate_scenes() -> Vec<(&'static str, Scene)> {
    let mut out = Vec::new();
    out.push(("empty", scene(Vec::new(), Vec::new())));
    out.push((
        "empty-mesh",
        scene(vec![Mesh { positions: Vec::new(), normals: None, indices: Vec::new(), hash: None }], vec![object("E", 0, [1.0; 3], identity(), None)]),
    ));
    let collapsed = Mesh {
        positions: vec![[0.0; 3], [0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
        normals: Some(vec![[0.0; 3]; 4]),
        indices: vec![[0, 1, 2], [0, 0, 0], [0, 2, 3]],
        hash: None,
    };
    out.push(("collapsed", scene(vec![collapsed], vec![object("C", 0, [1.0; 3], identity(), None)])));
    let huge = Mesh {
        positions: vec![[-3.0e38, -3.0e38, -3.0e38], [3.0e38, -3.0e38, 0.0], [0.0, 3.0e38, 3.0e38]],
        normals: None,
        indices: vec![[0, 1, 2]],
        hash: None,
    };
    out.push(("huge", scene(vec![huge], vec![object("H", 0, [1.0; 3], placed([0.0; 3], [1e30, 1e30, 1e30]), None)])));
    out.push(("zero-scale", scene(vec![cube(true)], vec![object("Z", 0, [1.0; 3], placed([0.0; 3], [0.0, 0.0, 0.0]), None)])));
    let mut inside = scene(vec![cube(false)], vec![object("Room", 0, [0.9; 3], placed([0.0; 3], [4.0, 4.0, 4.0]), None)]);
    inside.cameras.push(Camera {
        id: "inside".into(),
        position: [0.1, 0.05, 0.2],
        target: Some([0.3, -0.2, -1.0]),
        forward: None,
        up: [0.0, 1.0, 0.0],
        yfov: 1.6,
        aspect: None,
        znear: Some(0.001),
    });
    out.push(("camera-inside", inside));
    let mut away = cube_scene();
    away.cameras.push(Camera {
        id: "away".into(),
        position: [0.0, 0.0, 5.0],
        target: None,
        forward: Some([0.0, 0.0, 1.0]),
        up: [0.0, 1.0, 0.0],
        yfov: 0.5,
        aspect: None,
        znear: None,
    });
    out.push(("camera-away", away));
    out
}
