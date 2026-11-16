//! Polygons to triangles, the file's corner order kept, so every triangle keeps its polygon's
//! winding (counter-clockwise seen from the front).
//!
//! **Always `n - 2` triangles** for a polygon of `n >= 3` corners, whatever its shape, so a
//! mesh's triangle count (`corners - 2 * faces`) is known from its counts alone, before any is
//! made. None is dropped; a degenerate one is kept (it draws nothing).
//!
//! Per polygon, in `f64`, with a fixed order of operations:
//!
//! - **3 corners:** the triangle itself.
//! - **The plane:** the polygon's normal by Newell's sums. When it is zero or not finite (every
//!   corner on one line or one point), the polygon is fanned from its first corner (counted
//!   `degenerate`). Otherwise the polygon is projected onto the axis plane the normal is most
//!   along (ties: X, then Y, then Z), oriented so it turns counter-clockwise there.
//! - **Convex** (every turn the normal's way, or straight): a fan from the first corner.
//! - **Concave:** ear clipping. An ear is a corner turning the normal's way whose triangle with
//!   its two neighbours holds no other remaining corner that doesn't (a corner on an edge counts
//!   as inside). The search starts at the first corner and, after each ear, goes on from the
//!   corner before it. When a whole round finds no ear (a polygon crossing itself), the corners
//!   left are fanned from the first of them (counted `ear_clip_failed`).
//! - **Over [`MeshReadLimits::ear_clip_max_corners`](super::MeshReadLimits) corners:** fanned
//!   (counted `large_face_fanned`): ear clipping takes time growing with the square of the
//!   corners, and one huge polygon must not take a read's budget.
//!
//! A corner index past `positions` reads as a point that isn't finite (a degenerate polygon):
//! callers check indices first, and nothing here panics on any input.

use crate::mutant;

/// What triangulating counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TriCounts {
    /// Polygons triangulated.
    pub faces: u64,
    /// Triangles made.
    pub triangles: u64,
    /// Polygons fanned as convex.
    pub convex: u64,
    /// Polygons ear clipped whole.
    pub ear_clipped: u64,
    /// Polygons with no plane (collinear or coincident corners), fanned.
    pub degenerate: u64,
    /// Polygons where a round found no ear (they cross themselves); the rest fanned.
    pub ear_clip_failed: u64,
    /// Polygons over the ear-clipping limit, fanned.
    pub large_face_fanned: u64,
    /// Ears tested: at most `n * n` for a polygon of `n` corners.
    pub ear_tests: u64,
}

const NAN3: [f32; 3] = [f32::NAN; 3];

fn point(positions: &[[f32; 3]], i: u32) -> [f64; 3] {
    let p = positions.get(i as usize).unwrap_or(&NAN3);
    [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
}

/// Appends `(a, b, c)`, in that order (the face's).
fn emit(out: &mut Vec<[u32; 3]>, a: u32, b: u32, c: u32) {
    if mutant::active("WindingReversed") {
        out.push([a, c, b]);
    } else {
        out.push([a, b, c]);
    }
}

/// A fan of `corners` from the first.
fn fan(out: &mut Vec<[u32; 3]>, corners: &[u32]) {
    for w in corners[1..].windows(2) {
        emit(out, corners[0], w[0], w[1]);
    }
}

/// `(b - a) x (c - b)` in the plane: positive for a counter-clockwise turn.
fn turn(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0])
}

/// `p` inside or on the counter-clockwise triangle `a b c`.
fn inside(p: [f64; 2], a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> bool {
    let side =
        |u: [f64; 2], v: [f64; 2]| (v[0] - u[0]) * (p[1] - u[1]) - (v[1] - u[1]) * (p[0] - u[0]);
    side(a, b) >= 0.0 && side(b, c) >= 0.0 && side(c, a) >= 0.0
}

/// Triangulates one polygon, appending exactly `face.len() - 2` triangles to `out` (none for
/// fewer than 3 corners). `face` holds the polygon's vertex indices, in the file's corner order.
pub fn triangulate_face(
    positions: &[[f32; 3]],
    face: &[u32],
    max_corners: u32,
    out: &mut Vec<[u32; 3]>,
    counts: &mut TriCounts,
) {
    let n = face.len();
    if n < 3 {
        return;
    }
    counts.faces += 1;
    counts.triangles += (n - 2) as u64;
    if n == 3 {
        emit(out, face[0], face[1], face[2]);
        return;
    }
    let drop_last = mutant::active("TriangleDropped");
    let start = out.len();
    clip_or_fan(positions, face, max_corners, out, counts);
    if drop_last && out.len() > start {
        out.pop();
    }
}

fn clip_or_fan(
    positions: &[[f32; 3]],
    face: &[u32],
    max_corners: u32,
    out: &mut Vec<[u32; 3]>,
    counts: &mut TriCounts,
) {
    let n = face.len();
    if n > max_corners as usize {
        counts.large_face_fanned += 1;
        fan(out, face);
        return;
    }
    // Newell's normal, summed in corner order
    let mut nrm = [0f64; 3];
    for (&i, &j) in face.iter().zip(face.iter().cycle().skip(1)) {
        let (c, d) = (point(positions, i), point(positions, j));
        nrm[0] += (c[1] - d[1]) * (c[2] + d[2]);
        nrm[1] += (c[2] - d[2]) * (c[0] + d[0]);
        nrm[2] += (c[0] - d[0]) * (c[1] + d[1]);
    }
    let a = [nrm[0].abs(), nrm[1].abs(), nrm[2].abs()];
    if !nrm.iter().all(|x| x.is_finite()) || a == [0.0; 3] {
        counts.degenerate += 1;
        fan(out, face);
        return;
    }
    // drop the axis the normal is most along; (u, v) turn counter-clockwise for a positive
    // component: X -> (y, z), Y -> (z, x), Z -> (x, y)
    let k = if a[0] >= a[1] && a[0] >= a[2] {
        0
    } else if a[1] >= a[2] {
        1
    } else {
        2
    };
    let flip = nrm[k] < 0.0;
    let pts: Vec<[f64; 2]> = face
        .iter()
        .map(|&i| {
            let p = point(positions, i);
            let (u, v) = match k {
                0 => (p[1], p[2]),
                1 => (p[2], p[0]),
                _ => (p[0], p[1]),
            };
            [u, if flip { -v } else { v }]
        })
        .collect();
    let convex = (0..n).all(|i| turn(pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]) >= 0.0);
    if convex || mutant::active("ConcaveFanned") {
        counts.convex += 1;
        fan(out, face);
        return;
    }
    // ear clipping over the corners left (local indices, in corner order)
    let mut left: Vec<usize> = (0..n).collect();
    let mut at = 0usize;
    let mut misses = 0usize;
    while left.len() > 3 {
        let m = left.len();
        let (pi, ci, ni) = (left[(at + m - 1) % m], left[at], left[(at + 1) % m]);
        counts.ear_tests += 1;
        let (p, c, q) = (pts[pi], pts[ci], pts[ni]);
        let ear = turn(p, c, q) > 0.0
            && (0..m).all(|j| {
                let v = left[j];
                if v == pi || v == ci || v == ni {
                    return true;
                }
                let reflex =
                    turn(pts[left[(j + m - 1) % m]], pts[v], pts[left[(j + 1) % m]]) <= 0.0;
                !(reflex && inside(pts[v], p, c, q))
            });
        if ear {
            emit(out, face[pi], face[ci], face[ni]);
            left.remove(at);
            at = if at == 0 { left.len() - 1 } else { at - 1 };
            misses = 0;
        } else {
            at = (at + 1) % m;
            misses += 1;
            if misses >= m {
                counts.ear_clip_failed += 1;
                let rest: Vec<u32> = left.iter().map(|&i| face[i]).collect();
                fan(out, &rest);
                return;
            }
        }
    }
    emit(out, face[left[0]], face[left[1]], face[left[2]]);
    counts.ear_clipped += 1;
}

/// Triangulates every polygon of a mesh: polygon `i`'s corners are
/// `corners[offsets[i]..offsets[i + 1]]`, each a vertex index. Appends `corners.len() - 2 *
/// faces` triangles when every polygon has at least 3 corners. Offsets out of order or past
/// `corners` end the walk there (callers check them first).
pub fn triangulate(
    positions: &[[f32; 3]],
    corners: &[u32],
    offsets: &[u32],
    max_corners: u32,
    out: &mut Vec<[u32; 3]>,
    counts: &mut TriCounts,
) {
    for w in offsets.windows(2) {
        let (s, e) = (w[0] as usize, w[1] as usize);
        let Some(face) = corners.get(s..e) else {
            return;
        };
        triangulate_face(positions, face, max_corners, out, counts);
    }
}
