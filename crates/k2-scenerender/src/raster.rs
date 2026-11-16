//! The rasterizer.
//!
//! Pipeline, all on the CPU:
//! 1. **Vertex:** object-to-view per object (the matrices composed in f64, applied in f32), view
//!    normals through the cofactor matrix, and each vertex snapped to 24.8 fixed point in sample
//!    space; a vertex behind the near plane or outside the guard band (|x|, |y| > 2w) is marked
//!    for clipping instead.
//! 2. **Bin:** each triangle, in global order (objects in order, triangles in order), is tested
//!    (facing, culling, any sample covered) and its index pushed to every band of rows it touches.
//!    Bands are 16 output rows. Triangles with a marked vertex go through the homogeneous clipper
//!    (near plane + guard band, Sutherland-Hodgman).
//! 3. **Raster:** each band draws its triangles in global order into its own sample buffers
//!    (linear RGB f32 + 1/w depth, strict `>` test: the earliest triangle wins a tie), then
//!    box-filters the SSAA samples into its rows of the image (linear -> sRGB through the fixed
//!    table of [`crate::srgb`]: no libm on the render path).
//!
//! **Transparent** ([`Params::transparent`]): RGBA8 instead of RGB8 on the background. Alpha is
//! the pixel's covered samples over its samples, rounded half up in integers; the colour is
//! straight (not premultiplied), the mean of the covered samples only; an uncovered pixel is
//! 0, 0, 0, 0. A fully covered pixel has the opaque image's colour exactly.
//!
//! **Determinism:** work is split by fixed chunks (vertices, triangles) and fixed bands, never by
//! thread count; each unit's result depends only on its input, and merges are in index order. So
//! the bytes are the same for any thread count. Edge functions are exact integers (i64), with a
//! top-left fill rule (watertight). Floats use only `+ - * / sqrt` in a fixed order, no FMA.
//!
//! **Lens shift** ([`Frame::shift_x`], a content-framed view's off-axis projection): clip x is
//! `fx * x + shift_x * w` (y likewise), everywhere clip x is formed (projection, guard band,
//! clipping), so the clipper and the fill rule see one frustum. Unshifted, the shift terms are
//! not evaluated at all: the arithmetic is the same as before the shift existed.
//!
//! **Faces:** `double_sided == Some(false)` culls back faces (glTF's default); unknown or true
//! draws both sides, lighting the back with the flipped normal (two-sided lighting). Winding
//! follows glTF: counter-clockwise is front, flipped when the object transform mirrors.
//!
//! **Lighting:** base colour x (ambient + key + fill), two directional lights fixed in view space
//! (so every turntable frame is lit the same way). Smooth (Gouraud, perspective-correct) when the
//! mesh has normals, flat otherwise.
//!
//! **Clay shading** ([`render_with`] and a [`Clay`]): each object's base colour is replaced by
//! one of two tones, by whether it is shell; nothing else changes (the lights, the coverage
//! alpha, the order of every operation).
//!
//! **Ambient occlusion** ([`render_shaded`] and an [`Ao`]), off unless asked for: before the colour
//! pass, a depth pass draws the same bands depth only and keeps one sample of each pixel (1/w at a
//! fixed place in it; 0 where nothing is drawn there) for the whole frame. Each covered pixel then
//! gets a factor in [0, 1] from that depth image ([`occlusion`]: above thumbnail widths worked out
//! on a half grid, every other pixel of every other row, and brought back to every pixel without
//! crossing a depth edge), and the colour pass multiplies every covered sample's linear colour by
//! its pixel's factor before the box filter: the whole light (ambient, key and fill) is darkened,
//! the background and the coverage alpha are not. A factor of exactly 1 leaves the sample's bits as
//! they were, so where nothing occludes, the pixels are the plain image's.
//!
//! **Object ids** ([`render_with_ids`]), off unless asked for: the colour pass also writes, into
//! each sample it fills, the drawn object's index in the scene's objects plus 1, under the same
//! depth test (so where two objects overlap, the nearer one's, the earliest on a tie). Each
//! pixel then keeps the value of one sample, the one the depth pass keeps ([`Lens::sample_at`]),
//! 0 where nothing is drawn there: an id image at the output size, the object the occlusion's
//! depth is of. The colour, the depth and the order of every float operation are untouched, so
//! the image's bytes are the same with ids or without. A pixel is on an edge between objects
//! when a 4-neighbour holds another value (or 0): anything that wants the edges derives them so.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::camera::Frame;
use crate::math::*;
use crate::scene::Scene;
use crate::srgb::{self, q16};

const ONE: i64 = 256;
const HALF: i64 = 128;
const GUARD: f32 = 2.0;
/// Output rows per band.
pub const BAND_PX: u32 = 16;
const CHUNK_TRIS: u64 = 1 << 16;
const CHUNK_VERTS: usize = 1 << 16;
const NEEDS_CLIP: i32 = i32::MIN;
const MAX_POLY: usize = 16;

/// The clay look (v1): base colour x (ambient + key + fill), the lights fixed in view space.
pub const AMBIENT: f32 = 0.22;
pub const KEY: f32 = 0.70;
pub const FILL: f32 = 0.28;
/// View-space directions toward the key light (upper left, in front) and the fill light
/// (right, slightly below); normalized when used.
pub const KEY_DIR: [f32; 3] = [-0.4, 0.6, 0.7];
pub const FILL_DIR: [f32; 3] = [0.6, -0.1, 0.5];

#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub width: u32,
    pub height: u32,
    /// Samples per axis per pixel (1-4): 2 = 4 samples per pixel.
    pub ssaa: u32,
    /// Worker threads (1: the calling thread only). Doesn't change the output.
    pub threads: usize,
    /// The opaque image's background (unused when transparent).
    pub background_srgb: [u8; 3],
    /// RGBA with coverage alpha instead of RGB on the background.
    pub transparent: bool,
}

pub struct Image {
    pub width: u32,
    pub height: u32,
    /// RGBA8 (straight alpha) when transparent, else RGB8.
    pub alpha: bool,
    /// The pixels, rows top to bottom.
    pub pixels: Vec<u8>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stats {
    pub objects: u64,
    pub vertices: u64,
    pub triangles: u64,
    /// Band references (a triangle in k bands counts k).
    pub refs: u64,
    pub clipped: u64,
    pub culled: u64,
    /// Off screen, degenerate, or covering no sample centre.
    pub skipped: u64,
    pub covered_px: u64,
    pub vertex_us: u64,
    pub bin_us: u64,
    /// The colour pass, and with ambient occlusion its depth and occlusion passes too.
    pub raster_us: u64,
    /// The depth and occlusion passes alone (0 without ambient occlusion).
    pub ao_us: u64,
}

#[derive(Clone, Copy)]
struct Cam {
    fx: f32,
    fy: f32,
    near: f32,
    sx: f32,
    sy: f32,
    /// The lens shift, and whether there is one.
    shift: [f32; 2],
    shifted: bool,
}

struct Obj {
    /// The object's index in the scene's objects.
    index: u32,
    vbase: usize,
    tbase: u64,
    tris: u64,
    mesh: usize,
    color: [f32; 3],
    cull: bool,
    mirror: bool,
    smooth: bool,
}

struct Ctx<'a> {
    scene: &'a Scene,
    objs: Vec<Obj>,
    vpos: Vec<V3>,
    vnor: Vec<V3>,
    vfix: Vec<[i32; 2]>,
    cam: Cam,
    ws: i64,
    hs: i64,
    band_rows: i64,
    nbands: usize,
    l1: V3,
    l2: V3,
}

/// Run `f` over `items` on up to `threads` threads, each with its own `init()` state. Which
/// thread takes which item varies; the caller makes each item's result independent of that.
fn par_each<T: Send, S, I, F>(threads: usize, items: Vec<T>, init: I, f: F)
where
    I: Fn() -> S + Sync,
    F: Fn(&mut S, T) + Sync,
{
    let n = threads.min(items.len());
    if n <= 1 {
        let mut s = init();
        for it in items {
            f(&mut s, it);
        }
        return;
    }
    let q = Mutex::new(items.into_iter());
    std::thread::scope(|sc| {
        for _ in 0..n {
            sc.spawn(|| {
                let mut s = init();
                loop {
                    let it = q.lock().unwrap().next();
                    match it {
                        Some(it) => f(&mut s, it),
                        None => break,
                    }
                }
            });
        }
    });
}

/// Clay shading for [`render_with`]: one tone for the shell, one for the contents, in place of
/// each object's base colour.
#[derive(Clone, Copy, Debug)]
pub struct Clay<'a> {
    /// Per entry of `active`: shell (`true`) or not.
    pub shell: &'a [bool],
    /// The tones, sRGB (made linear through [`srgb::to_linear`]).
    pub shell_srgb: [u8; 3],
    pub contents_srgb: [u8; 3],
}

/// Ambient occlusion for [`render_shaded`] (see [`occlusion`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ao {
    /// How far an occluder reaches, in world units: full weight within it, none past twice it.
    pub radius: f32,
    /// How dark full occlusion gets, 0 to 1: the light factor is `1 - strength * occlusion`.
    pub strength: f32,
}

/// Render the active objects: opaque RGB8 on the background colour, or RGBA8 when transparent.
pub fn render(scene: &Scene, active: &[u32], frame: &Frame, p: &Params) -> (Image, Stats) {
    render_shaded(scene, active, frame, p, None, None)
}

/// [`render`], with each object in a clay tone instead of its base colour when `clay` is given.
pub fn render_with(scene: &Scene, active: &[u32], frame: &Frame, p: &Params, clay: Option<&Clay>) -> (Image, Stats) {
    render_shaded(scene, active, frame, p, clay, None)
}

/// The vertex and bin stages: the context every pass draws from, and each band's triangles.
fn prepare<'a>(scene: &'a Scene, active: &[u32], frame: &Frame, p: &Params, clay: Option<&Clay>, st: &mut Stats) -> (Ctx<'a>, Vec<Vec<u32>>) {
    let s = p.ssaa.clamp(1, 4) as i64;
    let ws = p.width as i64 * s;
    let hs = p.height as i64 * s;
    let cam = Cam {
        fx: frame.fx,
        fy: frame.fy,
        near: frame.near,
        sx: (ws * ONE) as f32,
        sy: (hs * ONE) as f32,
        shift: [frame.shift_x, frame.shift_y],
        shifted: frame.shift_x != 0.0 || frame.shift_y != 0.0,
    };

    let t0 = Instant::now();
    let (objs, vpos, vnor, vfix) = vertex_stage(scene, active, frame, &cam, p.threads, clay);
    st.objects = objs.len() as u64;
    st.vertices = vpos.len() as u64;
    st.triangles = objs.iter().map(|o| o.tris).sum();
    let t1 = Instant::now();
    st.vertex_us = (t1 - t0).as_micros() as u64;

    let band_rows = BAND_PX as i64 * s;
    let nbands = p.height.div_ceil(BAND_PX) as usize;
    let ctx = Ctx {
        scene,
        objs,
        vpos,
        vnor,
        vfix,
        cam,
        ws,
        hs,
        band_rows,
        nbands,
        l1: normalize(KEY_DIR),
        l2: normalize(FILL_DIR),
    };
    let bands = bin(&ctx, p.threads, st);
    st.bin_us = t1.elapsed().as_micros() as u64;
    (ctx, bands)
}

/// [`render_with`], darkened by ambient occlusion when `ao` is given.
pub fn render_shaded(scene: &Scene, active: &[u32], frame: &Frame, p: &Params, clay: Option<&Clay>, ao: Option<&Ao>) -> (Image, Stats) {
    let (img, _, st) = draw(scene, active, frame, p, clay, ao, false);
    (img, st)
}

/// [`render_shaded`], and the id image: for each pixel, row-major at the output size, the index
/// in the scene's objects plus 1 of the object drawn at its depth sample, 0 where nothing is (see
/// the module's notes). The image is the same, byte for byte, as [`render_shaded`]'s.
pub fn render_with_ids(scene: &Scene, active: &[u32], frame: &Frame, p: &Params, clay: Option<&Clay>, ao: Option<&Ao>) -> (Image, Vec<u32>, Stats) {
    let (img, ids, st) = draw(scene, active, frame, p, clay, ao, true);
    (img, ids.unwrap_or_default(), st)
}

fn draw(scene: &Scene, active: &[u32], frame: &Frame, p: &Params, clay: Option<&Clay>, ao: Option<&Ao>, ids: bool) -> (Image, Option<Vec<u32>>, Stats) {
    let mut st = Stats::default();
    let (ctx, bands) = prepare(scene, active, frame, p, clay, &mut st);
    let s = p.ssaa.clamp(1, 4) as i64;
    let ws = ctx.ws;
    let band_rows = ctx.band_rows;

    let t2 = Instant::now();
    let w = p.width as usize;
    let factors = ao.map(|a| {
        let depth = depth_pass(&ctx, &bands, p.threads, s as usize, w, p.height as usize);
        let f = occlusion(&depth, w, p.height as usize, &Lens::of(frame, p.ssaa), a, p.threads);
        st.ao_us = t2.elapsed().as_micros() as u64;
        f
    });
    let lut = srgb::encode_table();
    let bg = [srgb::to_linear(p.background_srgb[0]), srgb::to_linear(p.background_srgb[1]), srgb::to_linear(p.background_srgb[2])];
    let ch = if p.transparent { 4 } else { 3 };
    let mut pixels = vec![0u8; w * p.height as usize * ch];
    let mut id_px = if ids { vec![0u32; w * p.height as usize] } else { Vec::new() };
    let covered = AtomicU64::new(0);
    {
        let rows = BAND_PX as usize * w;
        let out = pixels.chunks_mut(rows * ch);
        let items: Vec<(usize, &mut [u8], Option<&mut [u32]>)> = if ids {
            out.zip(id_px.chunks_mut(rows)).enumerate().map(|(b, (o, i))| (b, o, Some(i))).collect()
        } else {
            out.enumerate().map(|(b, o)| (b, o, None)).collect()
        };
        let n = (ws * band_rows) as usize;
        par_each(
            p.threads,
            items,
            || Band { color: vec![[0.0; 3]; n], depth: vec![0.0; n], id: if ids { vec![0; n] } else { Vec::new() } },
            |buf, (b, out, id_out)| {
                let k = factors.as_deref().map(|f| &f[b * rows..(b * rows + rows).min(f.len())]);
                let c = raster_band(&ctx, b, &bands[b], buf, out, bg, &lut, s as usize, w, p.transparent, k, id_out);
                covered.fetch_add(c, Ordering::Relaxed);
            },
        );
    }
    st.covered_px = covered.load(Ordering::Relaxed);
    st.raster_us = t2.elapsed().as_micros() as u64;
    (Image { width: p.width, height: p.height, alpha: p.transparent, pixels }, ids.then_some(id_px), st)
}

type VertexOut = (Vec<Obj>, Vec<V3>, Vec<V3>, Vec<[i32; 2]>);

fn vertex_stage(scene: &Scene, active: &[u32], frame: &Frame, cam: &Cam, threads: usize, clay: Option<&Clay>) -> VertexOut {
    let mut objs = Vec::with_capacity(active.len());
    let mut vb = 0usize;
    let mut tb = 0u64;
    let mut any_smooth = false;
    let lin = |c: [u8; 3]| [srgb::to_linear(c[0]), srgb::to_linear(c[1]), srgb::to_linear(c[2])];
    let tones = clay.map(|c| (c.shell, lin(c.shell_srgb), lin(c.contents_srgb)));
    for (k, &oi) in active.iter().enumerate() {
        let o = &scene.objects[oi as usize];
        let m = &scene.meshes[o.mesh as usize];
        let smooth = m.normals.is_some();
        any_smooth |= smooth;
        objs.push(Obj {
            index: oi,
            vbase: vb,
            tbase: tb,
            tris: m.indices.len() as u64,
            mesh: o.mesh as usize,
            color: match tones {
                Some((shell, s, c)) => {
                    if shell.get(k).copied().unwrap_or(false) {
                        s
                    } else {
                        c
                    }
                }
                None => [o.base_color[0], o.base_color[1], o.base_color[2]],
            },
            cull: o.double_sided == Some(false),
            mirror: m3_det(&m4_from_f32(&o.transform)) < 0.0,
            smooth,
        });
        vb += m.positions.len();
        tb += m.indices.len() as u64;
    }
    let mut vpos = vec![[0f32; 3]; vb];
    let mut vnor = if any_smooth { vec![[0f32; 3]; vb] } else { Vec::new() };
    let mut vfix = vec![[0i32; 2]; vb];
    {
        struct Item<'s> {
            obj: usize,
            start: usize,
            pos: &'s mut [V3],
            nor: &'s mut [V3],
            fix: &'s mut [[i32; 2]],
        }
        let mut items = Vec::new();
        let mut prest: &mut [V3] = &mut vpos;
        let mut nrest: &mut [V3] = &mut vnor;
        let mut frest: &mut [[i32; 2]] = &mut vfix;
        for (k, ob) in objs.iter().enumerate() {
            let n = scene.meshes[ob.mesh].positions.len();
            let mut start = 0;
            while start < n {
                let len = (n - start).min(CHUNK_VERTS);
                let (pos, pr) = std::mem::take(&mut prest).split_at_mut(len);
                prest = pr;
                let (fix, fr) = std::mem::take(&mut frest).split_at_mut(len);
                frest = fr;
                let nor: &mut [V3] = if any_smooth {
                    let (a, b) = std::mem::take(&mut nrest).split_at_mut(len);
                    nrest = b;
                    a
                } else {
                    &mut []
                };
                items.push(Item { obj: k, start, pos, nor, fix });
                start += len;
            }
        }
        let objs = &objs;
        par_each(
            threads,
            items,
            || (),
            |_, it| {
                let ob = &objs[it.obj];
                let o = &scene.objects[active[it.obj] as usize];
                let mesh = &scene.meshes[ob.mesh];
                let mv = m4_mul(&frame.view, &m4_from_f32(&o.transform));
                let m = m4_to_f32(&mv);
                let nmd = normal_matrix(&mv);
                let nm = [
                    [nmd[0][0] as f32, nmd[0][1] as f32, nmd[0][2] as f32],
                    [nmd[1][0] as f32, nmd[1][1] as f32, nmd[1][2] as f32],
                    [nmd[2][0] as f32, nmd[2][1] as f32, nmd[2][2] as f32],
                ];
                for i in 0..it.pos.len() {
                    let p = mesh.positions[it.start + i];
                    let v = [
                        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
                        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
                        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
                    ];
                    it.pos[i] = v;
                    it.fix[i] = project(v, cam);
                }
                if ob.smooth {
                    let normals = mesh.normals.as_ref().unwrap();
                    for i in 0..it.nor.len() {
                        let n = normals[it.start + i];
                        it.nor[i] = normalize([
                            nm[0][0] * n[0] + nm[1][0] * n[1] + nm[2][0] * n[2],
                            nm[0][1] * n[0] + nm[1][1] * n[1] + nm[2][1] * n[2],
                            nm[0][2] * n[0] + nm[1][2] * n[1] + nm[2][2] * n[2],
                        ]);
                    }
                }
            },
        );
    }
    (objs, vpos, vnor, vfix)
}

/// Sample-space 24.8 fixed point of a clip-space point (x, y over w), the one rounding used everywhere.
#[inline]
fn fixed_of(nx: f32, ny: f32, cam: &Cam) -> [i32; 2] {
    let px = (nx * 0.5 + 0.5) * cam.sx;
    let py = (0.5 - ny * 0.5) * cam.sy;
    [px.round() as i32, py.round() as i32]
}

#[inline]
// `!(w >= near)`: a NaN depth takes the clip path too.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn project(v: V3, cam: &Cam) -> [i32; 2] {
    let w = -v[2];
    if !(w >= cam.near) {
        return [NEEDS_CLIP, 0];
    }
    let (x, y) = clip_xy(v, w, cam);
    let g = GUARD * w;
    if !(x.abs() <= g && y.abs() <= g) {
        return [NEEDS_CLIP, 0];
    }
    let inv = 1.0 / w;
    fixed_of(x * inv, y * inv, cam)
}

#[inline]
fn fixed_clip(p: &CV, cam: &Cam) -> [i32; 2] {
    let inv = 1.0 / p[2];
    fixed_of(p[0] * inv, p[1] * inv, cam)
}

/// E_ab(p): twice the signed area of (a, b, p) in y-down sample space.
#[inline]
fn edge(a: [i32; 2], b: [i32; 2], p: [i32; 2]) -> i64 {
    (b[0] as i64 - a[0] as i64) * (p[1] as i64 - a[1] as i64) - (b[1] as i64 - a[1] as i64) * (p[0] as i64 - a[0] as i64)
}

#[inline]
fn floor_div(a: i64, b: i64) -> i64 {
    a.div_euclid(b)
}

#[inline]
fn ceil_div(a: i64, b: i64) -> i64 {
    -((-a).div_euclid(b))
}

/// Sample rows and columns whose centres a box of fixed-point points could cover, clamped.
fn sample_bbox(pts: &[[i32; 2]], ws: i64, hs: i64) -> Option<(i64, i64, i64, i64)> {
    let mut minx = i64::MAX;
    let mut maxx = i64::MIN;
    let mut miny = i64::MAX;
    let mut maxy = i64::MIN;
    for p in pts {
        minx = minx.min(p[0] as i64);
        maxx = maxx.max(p[0] as i64);
        miny = miny.min(p[1] as i64);
        maxy = maxy.max(p[1] as i64);
    }
    let x0 = ceil_div(minx - HALF, ONE).max(0);
    let x1 = floor_div(maxx - HALF, ONE).min(ws - 1);
    let y0 = ceil_div(miny - HALF, ONE).max(0);
    let y1 = floor_div(maxy - HALF, ONE).min(hs - 1);
    if x0 > x1 || y0 > y1 {
        None
    } else {
        Some((x0, x1, y0, y1))
    }
}

/// A clip-space vertex: x, y, w, then linear r, g, b.
type CV = [f32; 6];

/// Clip x and y of a view-space point whose clip w is `w` (= -z): the one place they are formed.
#[inline]
fn clip_xy(v: V3, w: f32, cam: &Cam) -> (f32, f32) {
    let x = v[0] * cam.fx;
    let y = v[1] * cam.fy;
    if cam.shifted {
        (x + cam.shift[0] * w, y + cam.shift[1] * w)
    } else {
        (x, y)
    }
}

#[inline]
fn clip_of(v: V3, cam: &Cam) -> [f32; 3] {
    let w = -v[2];
    let (x, y) = clip_xy(v, w, cam);
    [x, y, w]
}

/// The homogeneous determinant of (x, y, w): > 0 when the triangle is counter-clockwise as seen
/// from the eye, also when some vertices are behind it.
#[inline]
fn hdet(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])
}

fn trivially_out(p: &[[f32; 3]; 3], near: f32) -> bool {
    p.iter().all(|v| v[2] < near)
        || p.iter().all(|v| v[0] > v[2])
        || p.iter().all(|v| v[0] < -v[2])
        || p.iter().all(|v| v[1] > v[2])
        || p.iter().all(|v| v[1] < -v[2])
}

#[inline]
fn plane_dist(v: &CV, plane: usize, near: f32) -> f32 {
    match plane {
        0 => v[2] - near,
        1 => GUARD * v[2] + v[0],
        2 => GUARD * v[2] - v[0],
        3 => GUARD * v[2] + v[1],
        _ => GUARD * v[2] - v[1],
    }
}

/// Sutherland-Hodgman against the near plane and the guard band. Returns the polygon and its
/// vertex count (0: nothing left).
fn clip_poly(tri: [CV; 3], near: f32) -> ([CV; MAX_POLY], usize) {
    let mut a = [[0f32; 6]; MAX_POLY];
    let mut b = [[0f32; 6]; MAX_POLY];
    a[..3].copy_from_slice(&tri);
    let mut n = 3;
    for plane in 0..5 {
        let mut m = 0;
        for i in 0..n {
            let p = a[i];
            let q = a[if i + 1 == n { 0 } else { i + 1 }];
            let dp = plane_dist(&p, plane, near);
            let dq = plane_dist(&q, plane, near);
            if m + 2 > MAX_POLY {
                return (a, 0);
            }
            if dp >= 0.0 {
                b[m] = p;
                m += 1;
            }
            if (dp >= 0.0) != (dq >= 0.0) {
                // Always from the inside vertex, so two triangles sharing the edge get the same point.
                let (i, o, di, do_) = if dp >= 0.0 { (p, q, dp, dq) } else { (q, p, dq, dp) };
                let t = di / (di - do_);
                let mut r = [0f32; 6];
                for k in 0..6 {
                    r[k] = i[k] + t * (o[k] - i[k]);
                }
                b[m] = r;
                m += 1;
            }
        }
        std::mem::swap(&mut a, &mut b);
        n = m;
        if n < 3 {
            return (a, 0);
        }
    }
    (a, n)
}

struct BinOut {
    bands: Vec<Vec<u32>>,
    clipped: u64,
    culled: u64,
    skipped: u64,
}

fn bin(ctx: &Ctx, threads: usize, st: &mut Stats) -> Vec<Vec<u32>> {
    let total = st.triangles;
    let nchunks = total.div_ceil(CHUNK_TRIS) as usize;
    let mut outs: Vec<Option<BinOut>> = (0..nchunks).map(|_| None).collect();
    {
        let items: Vec<(usize, &mut Option<BinOut>)> = outs.iter_mut().enumerate().collect();
        par_each(
            threads,
            items,
            || (),
            |_, (ci, slot)| {
                let lo = ci as u64 * CHUNK_TRIS;
                let hi = (lo + CHUNK_TRIS).min(total);
                *slot = Some(bin_chunk(ctx, lo, hi));
            },
        );
    }
    let outs: Vec<BinOut> = outs.into_iter().map(|o| o.expect("every chunk binned")).collect();
    let mut bands: Vec<Vec<u32>> = Vec::with_capacity(ctx.nbands);
    for b in 0..ctx.nbands {
        let n: usize = outs.iter().map(|o| o.bands[b].len()).sum();
        let mut v = Vec::with_capacity(n);
        for o in &outs {
            v.extend_from_slice(&o.bands[b]);
        }
        st.refs += n as u64;
        bands.push(v);
    }
    for o in &outs {
        st.clipped += o.clipped;
        st.culled += o.culled;
        st.skipped += o.skipped;
    }
    bands
}

#[inline]
fn tri_vertices(ctx: &Ctx, ob: &Obj, g: u64) -> [usize; 3] {
    let t = ctx.scene.meshes[ob.mesh].indices[(g - ob.tbase) as usize];
    [ob.vbase + t[0] as usize, ob.vbase + t[1] as usize, ob.vbase + t[2] as usize]
}

fn bin_chunk(ctx: &Ctx, lo: u64, hi: u64) -> BinOut {
    let mut out = BinOut { bands: vec![Vec::new(); ctx.nbands], clipped: 0, culled: 0, skipped: 0 };
    if lo >= hi {
        return out;
    }
    let mut oi = ctx.objs.partition_point(|o| o.tbase + o.tris <= lo);
    for g in lo..hi {
        while g >= ctx.objs[oi].tbase + ctx.objs[oi].tris {
            oi += 1;
        }
        let ob = &ctx.objs[oi];
        let v = tri_vertices(ctx, ob, g);
        let f = [ctx.vfix[v[0]], ctx.vfix[v[1]], ctx.vfix[v[2]]];
        let bbox = if f.iter().all(|p| p[0] != NEEDS_CLIP) {
            let area = edge(f[0], f[1], f[2]);
            if area == 0 {
                out.skipped += 1;
                continue;
            }
            let front = (area < 0) != ob.mirror;
            if ob.cull && !front {
                out.culled += 1;
                continue;
            }
            sample_bbox(&f, ctx.ws, ctx.hs)
        } else {
            let c = [clip_of(ctx.vpos[v[0]], &ctx.cam), clip_of(ctx.vpos[v[1]], &ctx.cam), clip_of(ctx.vpos[v[2]], &ctx.cam)];
            if trivially_out(&c, ctx.cam.near) {
                out.skipped += 1;
                continue;
            }
            let d = hdet(c[0], c[1], c[2]);
            if d == 0.0 || !d.is_finite() {
                out.skipped += 1;
                continue;
            }
            let front = (d > 0.0) != ob.mirror;
            if ob.cull && !front {
                out.culled += 1;
                continue;
            }
            let cv = |k: usize| [c[k][0], c[k][1], c[k][2], 0.0, 0.0, 0.0];
            let (poly, n) = clip_poly([cv(0), cv(1), cv(2)], ctx.cam.near);
            if n < 3 {
                out.skipped += 1;
                continue;
            }
            let mut pts = [[0i32; 2]; MAX_POLY];
            for k in 0..n {
                pts[k] = fixed_clip(&poly[k], &ctx.cam);
            }
            out.clipped += 1;
            sample_bbox(&pts[..n], ctx.ws, ctx.hs)
        };
        let Some((_, _, y0, y1)) = bbox else {
            out.skipped += 1;
            continue;
        };
        for b in (y0 / ctx.band_rows)..=(y1 / ctx.band_rows) {
            out.bands[b as usize].push(g as u32);
        }
    }
    out
}

struct Band {
    color: Vec<[f32; 3]>,
    depth: Vec<f32>,
    /// Each sample's object (index + 1), written with its depth; empty without ids.
    id: Vec<u32>,
}

#[derive(Clone, Copy)]
struct SV {
    x: i64,
    y: i64,
    iw: f32,
    c: [f32; 3],
}

#[allow(clippy::too_many_arguments)]
fn raster_band(ctx: &Ctx, b: usize, refs: &[u32], buf: &mut Band, out: &mut [u8], bg: [f32; 3], lut: &[u8], s: usize, w: usize, transparent: bool, ao: Option<&[f32]>, ids: Option<&mut [u32]>) -> u64 {
    let y_lo = b as i64 * ctx.band_rows;
    let y_hi = (y_lo + ctx.band_rows).min(ctx.hs);
    let ws = ctx.ws as usize;
    if refs.is_empty() && transparent {
        out.fill(0);
        return 0;
    }
    if refs.is_empty() {
        // Nothing here: the background, without touching the sample buffers.
        let px = [lut[q16(bg[0])], lut[q16(bg[1])], lut[q16(bg[2])]];
        for o in out.as_chunks_mut::<3>().0 {
            *o = px;
        }
        return 0;
    }
    // Only depth is cleared: a sample's colour (and id) is read only where its depth was written.
    let n = (y_hi - y_lo) as usize * ws;
    buf.depth[..n].fill(0.0);
    let mut oi = 0usize;
    for &g in refs {
        let g = g as u64;
        while g >= ctx.objs[oi].tbase + ctx.objs[oi].tris {
            oi += 1;
        }
        let ob = &ctx.objs[oi];
        if ids.is_some() {
            draw_tri::<true, true>(ctx, ob, tri_vertices(ctx, ob, g), buf, y_lo, y_hi);
        } else {
            draw_tri::<true, false>(ctx, ob, tri_vertices(ctx, ob, g), buf, y_lo, y_hi);
        }
    }
    if let Some(ids) = ids {
        // The depth pass's sample of each pixel (see `depth_pass`).
        let c = s / 2;
        for r in 0..(y_hi - y_lo) as usize / s {
            let row = (r * s + c) * ws + c;
            for px in 0..w {
                let i = row + px * s;
                ids[r * w + px] = if buf.depth[i] > 0.0 { buf.id[i] } else { 0 };
            }
        }
    }
    if transparent {
        resolve_rgba(buf, (y_hi - y_lo) as usize, ws, s, out, w, lut, ao)
    } else {
        resolve(buf, (y_hi - y_lo) as usize, ws, s, out, w, lut, bg, ao)
    }
}

fn light(base: [f32; 3], n: V3, ctx: &Ctx) -> [f32; 3] {
    let k = AMBIENT + KEY * dot(n, ctx.l1).max(0.0) + FILL * dot(n, ctx.l2).max(0.0);
    [base[0] * k, base[1] * k, base[2] * k]
}

fn shade(ctx: &Ctx, ob: &Obj, v: [usize; 3], front: bool) -> [[f32; 3]; 3] {
    if ob.smooth {
        let mut out = [[0f32; 3]; 3];
        for k in 0..3 {
            let n = ctx.vnor[v[k]];
            out[k] = light(ob.color, if front { n } else { neg(n) }, ctx);
        }
        out
    } else {
        let a = ctx.vpos[v[0]];
        let mut n = normalize(cross(sub(ctx.vpos[v[1]], a), sub(ctx.vpos[v[2]], a)));
        // Two-sided: the face normal that points at the eye (the origin of view space).
        if dot(n, a) > 0.0 {
            n = neg(n);
        }
        let c = light(ob.color, n, ctx);
        [c; 3]
    }
}

/// Draw one triangle into the band: colour and depth, or depth alone (`COLOR` false: no shading,
/// the colour buffer untouched); with `IDS`, the object's index + 1 into each sample it wins too.
fn draw_tri<const COLOR: bool, const IDS: bool>(ctx: &Ctx, ob: &Obj, v: [usize; 3], buf: &mut Band, y_lo: i64, y_hi: i64) {
    let f = [ctx.vfix[v[0]], ctx.vfix[v[1]], ctx.vfix[v[2]]];
    let fast = f.iter().all(|p| p[0] != NEEDS_CLIP);
    let flat = !ob.smooth;
    let id = ob.index.saturating_add(1);
    if fast {
        let area = edge(f[0], f[1], f[2]);
        if area == 0 {
            return;
        }
        let front = (area < 0) != ob.mirror;
        if ob.cull && !front {
            return;
        }
        let col = if COLOR { shade(ctx, ob, v, front) } else { [[0.0; 3]; 3] };
        let sv = |k: usize| SV { x: f[k][0] as i64, y: f[k][1] as i64, iw: 1.0 / -ctx.vpos[v[k]][2], c: col[k] };
        raster_tri::<COLOR, IDS>(buf, [sv(0), sv(1), sv(2)], flat, y_lo, y_hi, ctx.ws, id);
    } else {
        let c = [clip_of(ctx.vpos[v[0]], &ctx.cam), clip_of(ctx.vpos[v[1]], &ctx.cam), clip_of(ctx.vpos[v[2]], &ctx.cam)];
        if trivially_out(&c, ctx.cam.near) {
            return;
        }
        let d = hdet(c[0], c[1], c[2]);
        if d == 0.0 || !d.is_finite() {
            return;
        }
        let front = (d > 0.0) != ob.mirror;
        if ob.cull && !front {
            return;
        }
        let col = if COLOR { shade(ctx, ob, v, front) } else { [[0.0; 3]; 3] };
        let cv = |k: usize| [c[k][0], c[k][1], c[k][2], col[k][0], col[k][1], col[k][2]];
        let (poly, n) = clip_poly([cv(0), cv(1), cv(2)], ctx.cam.near);
        if n < 3 {
            return;
        }
        let mut sv = [SV { x: 0, y: 0, iw: 0.0, c: [0.0; 3] }; MAX_POLY];
        for k in 0..n {
            let p = &poly[k];
            let fx = fixed_clip(p, &ctx.cam);
            sv[k] = SV { x: fx[0] as i64, y: fx[1] as i64, iw: 1.0 / p[2], c: [p[3], p[4], p[5]] };
        }
        for k in 1..n - 1 {
            raster_tri::<COLOR, IDS>(buf, [sv[0], sv[k], sv[k + 1]], flat, y_lo, y_hi, ctx.ws, id);
        }
    }
}

/// Fill one triangle's samples in rows [y_lo, y_hi) (sample space); depth only when `COLOR` is
/// false; `id` into each sample it wins when `IDS`.
#[allow(clippy::too_many_arguments)]
fn raster_tri<const COLOR: bool, const IDS: bool>(buf: &mut Band, v: [SV; 3], flat: bool, y_lo: i64, y_hi: i64, ws: i64, id: u32) {
    let a = v[0];
    let mut b = v[1];
    let mut c = v[2];
    let mut area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if area == 0 {
        return;
    }
    if area < 0 {
        std::mem::swap(&mut b, &mut c);
        area = -area;
    }
    let minx = a.x.min(b.x).min(c.x);
    let maxx = a.x.max(b.x).max(c.x);
    let miny = a.y.min(b.y).min(c.y);
    let maxy = a.y.max(b.y).max(c.y);
    let x0 = ceil_div(minx - HALF, ONE).max(0);
    let x1 = floor_div(maxx - HALF, ONE).min(ws - 1);
    let y0 = ceil_div(miny - HALF, ONE).max(y_lo);
    let y1 = floor_div(maxy - HALF, ONE).min(y_hi - 1);
    if x0 > x1 || y0 > y1 {
        return;
    }
    let px0 = x0 * ONE + HALF;
    let py0 = y0 * ONE + HALF;
    // E_pq(s) = (q.x - p.x)(s.y - p.y) - (q.y - p.y)(s.x - p.x); inside: all three >= 0, ties
    // only on top or left edges.
    let setup = |p: SV, q: SV| -> (i64, i64, i64, i64) {
        let dx = q.x - p.x;
        let dy = q.y - p.y;
        let e = dx * (py0 - p.y) - dy * (px0 - p.x);
        let top_left = (dy == 0 && dx > 0) || dy < 0;
        (e, -dy * ONE, dx * ONE, if top_left { 0 } else { -1 })
    };
    let (mut ra, sxa, sya, ba) = setup(b, c);
    let (mut rb, sxb, syb, bb) = setup(c, a);
    let (mut rc, sxc, syc, bc) = setup(a, b);
    let inv_area = 1.0 / area as f32;
    let (ia, ib, ic) = (a.iw, b.iw, c.iw);
    let ca = [a.c[0] * ia, a.c[1] * ia, a.c[2] * ia];
    let cb = [b.c[0] * ib, b.c[1] * ib, b.c[2] * ib];
    let cc = [c.c[0] * ic, c.c[1] * ic, c.c[2] * ic];
    let wsu = ws as usize;
    for y in y0..=y1 {
        let row = (y - y_lo) as usize * wsu;
        let (mut ea, mut eb, mut ec) = (ra, rb, rc);
        for x in x0..=x1 {
            if ea + ba >= 0 && eb + bb >= 0 && ec + bc >= 0 {
                let la = ea as f32 * inv_area;
                let lb = eb as f32 * inv_area;
                let lc = ec as f32 * inv_area;
                let iw = la * ia + lb * ib + lc * ic;
                let i = row + x as usize;
                if iw > buf.depth[i] {
                    buf.depth[i] = iw;
                    if IDS {
                        buf.id[i] = id;
                    }
                    if COLOR {
                        buf.color[i] = if flat {
                            a.c
                        } else {
                            let inv = 1.0 / iw;
                            [
                                (la * ca[0] + lb * cb[0] + lc * cc[0]) * inv,
                                (la * ca[1] + lb * cb[1] + lc * cc[1]) * inv,
                                (la * ca[2] + lb * cb[2] + lc * cc[2]) * inv,
                            ]
                        };
                    }
                }
            }
            ea += sxa;
            eb += sxb;
            ec += sxc;
        }
        ra += sya;
        rb += syb;
        rc += syc;
    }
}

/// The covered sample's colour, times its pixel's occlusion factor when there is one.
#[inline]
fn darkened(c: [f32; 3], k: Option<f32>) -> [f32; 3] {
    match k {
        Some(k) => [c[0] * k, c[1] * k, c[2] * k],
        None => c,
    }
}

/// Box-filter the band's samples into its output rows; returns how many pixels any triangle covered.
/// With `ao` (the band's rows of occlusion factors), each covered sample is darkened first.
#[allow(clippy::too_many_arguments)]
fn resolve(buf: &Band, rows_s: usize, ws: usize, s: usize, out: &mut [u8], w: usize, lut: &[u8], bg: [f32; 3], ao: Option<&[f32]>) -> u64 {
    let rows = rows_s / s;
    let inv = 1.0 / (s * s) as f32;
    let mut covered = 0u64;
    for r in 0..rows {
        for px in 0..w {
            let mut acc = [0f32; 3];
            let mut hit = false;
            let k = ao.map(|a| a[r * w + px]);
            for sy in 0..s {
                let base = (r * s + sy) * ws + px * s;
                for sx in 0..s {
                    let c = if buf.depth[base + sx] > 0.0 {
                        hit = true;
                        darkened(buf.color[base + sx], k)
                    } else {
                        bg
                    };
                    acc[0] += c[0];
                    acc[1] += c[1];
                    acc[2] += c[2];
                }
            }
            let o = (r * w + px) * 3;
            out[o] = lut[q16(acc[0] * inv)];
            out[o + 1] = lut[q16(acc[1] * inv)];
            out[o + 2] = lut[q16(acc[2] * inv)];
            if hit {
                covered += 1;
            }
        }
    }
    covered
}

/// [`resolve`] for a transparent image: RGBA8, alpha = covered samples / samples (rounded half
/// up, in integers), the colour the mean of the covered samples only (straight alpha), summed
/// in the same order as [`resolve`] (so a fully covered pixel has the opaque colour). Occlusion
/// darkens the colour as in [`resolve`]; the alpha is the coverage either way.
#[allow(clippy::too_many_arguments)]
fn resolve_rgba(buf: &Band, rows_s: usize, ws: usize, s: usize, out: &mut [u8], w: usize, lut: &[u8], ao: Option<&[f32]>) -> u64 {
    let rows = rows_s / s;
    let total = (s * s) as u32;
    let mut covered = 0u64;
    for r in 0..rows {
        for px in 0..w {
            let mut acc = [0f32; 3];
            let mut n = 0u32;
            let k = ao.map(|a| a[r * w + px]);
            for sy in 0..s {
                let base = (r * s + sy) * ws + px * s;
                for sx in 0..s {
                    if buf.depth[base + sx] > 0.0 {
                        let c = darkened(buf.color[base + sx], k);
                        acc[0] += c[0];
                        acc[1] += c[1];
                        acc[2] += c[2];
                        n += 1;
                    }
                }
            }
            let o = (r * w + px) * 4;
            if n == 0 {
                out[o..o + 4].fill(0);
                continue;
            }
            let inv = 1.0 / n as f32;
            out[o] = lut[q16(acc[0] * inv)];
            out[o + 1] = lut[q16(acc[1] * inv)];
            out[o + 2] = lut[q16(acc[2] * inv)];
            out[o + 3] = ((510 * n + total) / (2 * total)) as u8;
            covered += 1;
        }
    }
    covered
}

/// The depth pass for ambient occlusion: every band drawn depth only (the colour pass's own
/// triangles, rasterization and depth test), each pixel keeping the 1/w of one sample, the same
/// one in every pixel ([`Lens::sample_at`]), 0 where it is not covered; the whole frame,
/// row-major. One fixed sample, not (say) the nearest of the pixel's: on a plane, 1/w at points a
/// whole number of pixels apart is then exactly affine, which the kernel's flat test relies on.
fn depth_pass(ctx: &Ctx, bands: &[Vec<u32>], threads: usize, s: usize, w: usize, h: usize) -> Vec<f32> {
    let mut depth = vec![0f32; w * h];
    let rows = BAND_PX as usize * w;
    let ws = ctx.ws as usize;
    let n = ws * ctx.band_rows as usize;
    let items: Vec<(usize, &mut [f32])> = depth.chunks_mut(rows).enumerate().collect();
    par_each(
        threads,
        items,
        || Band { color: Vec::new(), depth: vec![0.0; n], id: Vec::new() },
        |buf, (b, out)| {
            let refs = &bands[b];
            if refs.is_empty() {
                return;
            }
            let y_lo = b as i64 * ctx.band_rows;
            let y_hi = (y_lo + ctx.band_rows).min(ctx.hs);
            let rows_s = (y_hi - y_lo) as usize;
            buf.depth[..rows_s * ws].fill(0.0);
            let mut oi = 0usize;
            for &g in refs {
                let g = g as u64;
                while g >= ctx.objs[oi].tbase + ctx.objs[oi].tris {
                    oi += 1;
                }
                let ob = &ctx.objs[oi];
                draw_tri::<false, false>(ctx, ob, tri_vertices(ctx, ob, g), buf, y_lo, y_hi);
            }
            let c = s / 2;
            for r in 0..rows_s / s {
                let row = (r * s + c) * ws + c;
                for px in 0..w {
                    out[r * w + px] = buf.depth[row + px * s];
                }
            }
        },
    );
    depth
}

/// What the occlusion pass needs of a view to put a pixel back in view space: the projection's
/// scales and its lens shift ([`Frame`]), and where in each pixel its depth was taken.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lens {
    pub fx: f32,
    pub fy: f32,
    pub shift: [f32; 2],
    /// The depth's sample in each pixel, from its top-left corner, in pixels (x and y alike).
    pub at: f32,
}

impl Lens {
    /// `f` drawn with `ssaa` samples per axis: the depth is sample (s/2, s/2) of each pixel.
    pub fn of(f: &Frame, ssaa: u32) -> Lens {
        Lens { fx: f.fx, fy: f.fy, shift: [f.shift_x, f.shift_y], at: Lens::sample_at(ssaa) }
    }

    /// Where sample (s/2, s/2) of `ssaa` per axis sits in its pixel: the centre for 1 and 3,
    /// 3/4 for 2, 5/8 for 4.
    pub fn sample_at(ssaa: u32) -> f32 {
        let s = ssaa.clamp(1, 4);
        ((s / 2) as f32 + 0.5) / s as f32
    }
}

/// The occlusion kernel's axes: 8 directions 22.5 degrees apart over a half turn, each sampled
/// both ways; (cos, sin) written out, so no libm.
const AO_AXES: [[f32; 2]; 8] = [
    [1.0, 0.0],
    [0.923_879_5, 0.382_683_43],
    [0.707_106_77, 0.707_106_77],
    [0.382_683_43, 0.923_879_5],
    [0.0, 1.0],
    [-0.382_683_43, 0.923_879_5],
    [-0.707_106_77, 0.707_106_77],
    [-0.923_879_5, 0.382_683_43],
];
/// Distances per axis: 3 x 8 = 24 pairs per pixel. Axis k's step j is at
/// `(8 j + AO_ORDER[k] + phase) / 24` of the reach, so the 24 distances are evenly spaced over it,
/// each used once, and neighbouring axes take distances far apart.
const AO_STEPS: usize = 3;
const AO_ORDER: [u32; 8] = [0, 4, 2, 6, 1, 5, 3, 7];
const AO_PAIRS: usize = AO_STEPS * AO_AXES.len();
/// The fixed 4 x 4 pattern that varies the kernel from pixel to pixel (no random numbers): cell
/// `c = AO_PATTERN[y % 4][x % 4]` turns the axes by `AO_TURNS[c % 4]` (quarter steps of the 22.5
/// degrees between axes) and moves the distances by a phase of `(c / 4 + 0.5) / 4` of a step. Any
/// 4 x 4 block of pixels holds every cell once, so the blur over 4 x 4 sees 32 directions and 96
/// distances instead of the 8 and 24 of one pixel: smooth gradients, not rings.
const AO_PATTERN: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
/// (cos, sin) of 0, 5.625, 11.25 and 16.875 degrees.
const AO_TURNS: [[f32; 2]; 4] = [[1.0, 0.0], [0.995_184_7, 0.098_017_14], [0.980_785_25, 0.195_090_32], [0.956_940_35, 0.290_284_66]];
/// A pair bent by less than this (gap over half-chord) counts as flat: it absorbs the rounding of
/// a plane split into triangles (vertices snapped to 1/256 of a sample, 1/w interpolated in f32).
pub const AO_BIAS: f32 = 0.05;
/// The kernel's largest reach on screen, as a fraction of the image's longer side (a surface very
/// near the eye would otherwise sample across the whole frame).
pub const AO_MAX_REACH: f32 = 0.25;
/// The blur takes a neighbour only when its depth is within this fraction of the radius of the
/// pixel's: the same surface, or one touching it; never the floor seen past a table's edge.
pub const AO_BLUR_DEPTH: f32 = 0.25;

/// The widest image whose occlusion is worked out at every pixel; wider, on the half grid. At
/// thumbnail widths the half grid's blur is too wide next to the creases, and every pixel costs
/// little there.
pub const AO_FULL_GRID_MAX_WIDTH: usize = 640;

/// Round half away from zero, without libm (NaN gives 0).
#[inline]
fn round_px(v: f32) -> i64 {
    if v >= 0.0 {
        (v + 0.5) as i64
    } else {
        -((0.5 - v) as i64)
    }
}

/// The ambient occlusion factor of each pixel of a depth image: `depth` is row-major `w` x `h`,
/// each pixel's 1/w at the same place in it (0: nothing drawn there). An uncovered pixel gets 1,
/// and so does a covered one nothing occludes; full occlusion gets `1 - strength`.
///
/// The occlusion is worked out on a grid: every pixel up to [`AO_FULL_GRID_MAX_WIDTH`] wide, else
/// a half grid, `ceil(w / 2)` x `ceil(h / 2)`, whose point (i, j) is pixel (2i, 2j), its depth
/// and its place on screen exactly (no averaging, so a plane is still a plane on the grid); then
/// brought back to every pixel.
///
/// For a covered grid point P (view-space point p at depth z = 1/w), the kernel reaches `radius`
/// world units: `radius * fx * width / 2 / z` pixels, half as many steps on the half grid (so the
/// same fraction of the image at any width), at most [`AO_MAX_REACH`] of the image's longer side.
/// It takes [`AO_PAIRS`] pairs of grid points A and B at whole-step offsets `+d` and `-d` from P
/// (fixed directions and distances, [`AO_AXES`], varied by P's cell of [`AO_PATTERN`]), so P is
/// exactly their midpoint on screen. On screen, 1/w is affine over a plane, so for A, B and P on
/// one plane P's 1/w is the mean of theirs: the pair's gap `z - 2 / (1/w_A + 1/w_B)` (how far the
/// chord AB passes in front of P, along P's ray) is 0 on any plane, at any angle, and positive only
/// where the surface bends towards the eye: a crease, a corner, an object standing on the floor.
/// The pair's occlusion is its gap over half the chord's length (1 for a right-angle corner), less
/// [`AO_BIAS`], at most 1; it counts fully while A and B are both within `radius` of p, fading to
/// nothing at twice it, so an object far in front of P casts no halo. A pair with an end off the
/// grid or on an uncovered point counts 0: the empty background never occludes. P's raw occlusion
/// is the sum of its pairs over [`AO_PAIRS`].
///
/// Then a blur on the grid: P's occlusion is the mean of the raw occlusion over the 4 x 4 grid
/// points from (i - 2, j - 2) to (i + 1, j + 1) that are covered and within [`AO_BLUR_DEPTH`] of
/// the radius of P's depth (P is always one).
///
/// Then each covered pixel takes the mean occlusion of the grid points around it (bilinear: a
/// pixel on the grid, as every pixel is on the full one, its own point; one between two points
/// both, one between four all four, each weighing the same) that are covered and within
/// [`AO_BLUR_DEPTH`] of the radius of its own depth, so occlusion never crosses a depth edge;
/// with none, it has none. The factor is `1 - strength * occlusion`; where every raw value is 0
/// (a plane, the empty background) it is exactly 1.
///
/// Only `+ - * / sqrt` in a fixed order, and integer offsets: the same bits on every machine and
/// for any thread count (rows are split by fixed bands, each pass reading only the one before).
pub fn occlusion(depth: &[f32], w: usize, h: usize, lens: &Lens, ao: &Ao, threads: usize) -> Vec<f32> {
    let mut out = vec![1f32; w * h];
    if depth.len() != w * h || w == 0 || h == 0 {
        return out;
    }
    // The grid: every pixel up to AO_FULL_GRID_MAX_WIDTH, else every other pixel of every other
    // row; its depth and its place.
    let step = if w <= AO_FULL_GRID_MAX_WIDTH { 1 } else { 2 };
    let (gw, gh) = (w.div_ceil(step), h.div_ceil(step));
    let grid: Vec<f32> = if step == 1 { depth.to_vec() } else { (0..gw * gh).map(|g| depth[(g / gw) * step * w + (g % gw) * step]).collect() };
    // View-space x / z and y / z of each grid column's and row's depth sample.
    let ax: Vec<f32> = (0..gw).map(|i| (((step * i) as f32 + lens.at) * 2.0 / w as f32 - 1.0 - lens.shift[0]) / lens.fx).collect();
    let ay: Vec<f32> = (0..gh).map(|j| (1.0 - ((step * j) as f32 + lens.at) * 2.0 / h as f32 - lens.shift[1]) / lens.fy).collect();
    let mut steps = [[[0f32; 2]; AO_PAIRS]; 16];
    for (c, cell) in steps.iter_mut().enumerate() {
        let turn = AO_TURNS[c % 4];
        let phase = ((c / 4) as f32 + 0.5) / 4.0;
        for (k, axis) in AO_AXES.iter().enumerate() {
            let dir = [turn[0] * axis[0] - turn[1] * axis[1], turn[1] * axis[0] + turn[0] * axis[1]];
            for j in 0..AO_STEPS {
                let f = ((8 * j as u32 + AO_ORDER[k]) as f32 + phase) / AO_PAIRS as f32;
                cell[k * AO_STEPS + j] = [dir[0] * f, dir[1] * f];
            }
        }
    }
    let kernel = Kernel {
        depth: &grid,
        w: gw as i64,
        h: gh as i64,
        ax: &ax,
        ay: &ay,
        steps: &steps,
        // In grid steps: on the half grid, half the pixels.
        reach: 0.5 * lens.fx * w as f32 * ao.radius / step as f32,
        max_reach: w.max(h) as f32 * AO_MAX_REACH / step as f32,
        inv_radius: 1.0 / ao.radius,
    };
    let grid_rows = BAND_PX as usize * gw;
    let mut raw = vec![0f32; gw * gh];
    {
        let items: Vec<(usize, &mut [f32])> = raw.chunks_mut(grid_rows).enumerate().collect();
        par_each(
            threads,
            items,
            || (),
            |_, (b, band)| {
                for (r, row) in band.chunks_mut(gw).enumerate() {
                    let y = (b * BAND_PX as usize + r) as i64;
                    for (x, o) in row.iter_mut().enumerate() {
                        *o = kernel.raw(x as i64, y);
                    }
                }
            },
        );
    }
    let near = AO_BLUR_DEPTH * ao.radius;
    // |1/q - 1/iw| <= near, without a division: q (covered) is on iw's surface or one touching it.
    let close = |q: f32, iw: f32| {
        let d = if q > iw { q - iw } else { iw - q };
        q > 0.0 && d <= near * q * iw
    };
    let mut blurred = vec![0f32; gw * gh];
    {
        let items: Vec<(usize, &mut [f32])> = blurred.chunks_mut(grid_rows).enumerate().collect();
        par_each(
            threads,
            items,
            || (),
            |_, (b, band)| {
                for (r, row) in band.chunks_mut(gw).enumerate() {
                    let y = b * BAND_PX as usize + r;
                    for (x, o) in row.iter_mut().enumerate() {
                        let iw = grid[y * gw + x];
                        let covered = iw > 0.0;
                        if !covered {
                            continue;
                        }
                        let (mut sum, mut n) = (0f32, 0u32);
                        for qy in y.saturating_sub(2)..(y + 2).min(gh) {
                            for qx in x.saturating_sub(2)..(x + 2).min(gw) {
                                if close(grid[qy * gw + qx], iw) {
                                    sum += raw[qy * gw + qx];
                                    n += 1;
                                }
                            }
                        }
                        if n > 0 {
                            *o = sum / n as f32;
                        }
                    }
                }
            },
        );
    }
    let rows = BAND_PX as usize * w;
    let items: Vec<(usize, &mut [f32])> = out.chunks_mut(rows).enumerate().collect();
    par_each(
        threads,
        items,
        || (),
        |_, (b, band)| {
            for (r, row) in band.chunks_mut(w).enumerate() {
                let y = b * BAND_PX as usize + r;
                // The grid rows around this row: its own on the full grid or an even row of the
                // half grid, the two either side on an odd one (the second off the grid on the
                // last row of an even height).
                let gy = [y / step, if y % step == 1 && y / step + 1 < gh { y / step + 1 } else { usize::MAX }];
                for (x, f) in row.iter_mut().enumerate() {
                    let iw = depth[y * w + x];
                    let covered = iw > 0.0;
                    if !covered {
                        continue;
                    }
                    let gx = [x / step, if x % step == 1 && x / step + 1 < gw { x / step + 1 } else { usize::MAX }];
                    let (mut sum, mut n) = (0f32, 0u32);
                    for &qy in gy.iter().filter(|&&v| v != usize::MAX) {
                        for &qx in gx.iter().filter(|&&v| v != usize::MAX) {
                            if close(grid[qy * gw + qx], iw) {
                                sum += blurred[qy * gw + qx];
                                n += 1;
                            }
                        }
                    }
                    if n > 0 {
                        *f = (1.0 - ao.strength * (sum / n as f32)).max(0.0);
                    }
                }
            }
        },
    );
    out
}

struct Kernel<'a> {
    depth: &'a [f32],
    w: i64,
    h: i64,
    ax: &'a [f32],
    ay: &'a [f32],
    /// Each pattern cell's pair offsets, in units of the reach.
    steps: &'a [[[f32; 2]; AO_PAIRS]; 16],
    /// Pixels the radius spans at 1/w = 1.
    reach: f32,
    max_reach: f32,
    inv_radius: f32,
}

impl Kernel<'_> {
    #[inline]
    fn at(&self, x: i64, y: i64, iw: f32) -> V3 {
        let z = 1.0 / iw;
        [self.ax[x as usize] * z, self.ay[y as usize] * z, z]
    }

    /// The raw occlusion at (x, y), 0 to 1.
    fn raw(&self, x: i64, y: i64) -> f32 {
        let iw = self.depth[(y * self.w + x) as usize];
        if !(iw > 0.0) {
            return 0.0;
        }
        let p = self.at(x, y, iw);
        let reach = (self.reach * iw).min(self.max_reach);
        let mut sum = 0f32;
        for st in &self.steps[AO_PATTERN[(y & 3) as usize][(x & 3) as usize] as usize] {
            let dx = round_px(st[0] * reach);
            let dy = round_px(st[1] * reach);
            if dx == 0 && dy == 0 {
                continue;
            }
            let (xa, ya, xb, yb) = (x + dx, y + dy, x - dx, y - dy);
            if xa < 0 || xb < 0 || ya < 0 || yb < 0 || xa >= self.w || xb >= self.w || ya >= self.h || yb >= self.h {
                continue;
            }
            let wa = self.depth[(ya * self.w + xa) as usize];
            let wb = self.depth[(yb * self.w + xb) as usize];
            if !(wa > 0.0 && wb > 0.0) {
                continue;
            }
            let gap = p[2] - 2.0 / (wa + wb);
            if !(gap > 0.0) {
                continue;
            }
            let a = self.at(xa, ya, wa);
            let b = self.at(xb, yb, wb);
            let (ea, eb) = (sub(a, p), sub(b, p));
            let (la, lb) = (dot(ea, ea), dot(eb, eb));
            let far = if la > lb { la } else { lb };
            let weight = (2.0 - far.sqrt() * self.inv_radius).min(1.0);
            if !(weight > 0.0) {
                continue;
            }
            let ab = sub(a, b);
            let half = 0.5 * dot(ab, ab).sqrt();
            if !(half > 0.0) {
                continue;
            }
            let o = ((gap / half - AO_BIAS) / (1.0 - AO_BIAS)).min(1.0);
            if o > 0.0 {
                sum += o * weight;
            }
        }
        sum / AO_PAIRS as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{frame, ViewSpec};
    use crate::scene::{Camera, Mesh, Object, Scene, UpAxis};
    use crate::testgen;

    const BG: [u8; 3] = [240, 240, 240];

    fn draw(s: &Scene, w: u32, h: u32, ssaa: u32, threads: usize, spec: ViewSpec) -> (Image, Stats, Frame) {
        let active: Vec<u32> = (0..s.objects.len() as u32).collect();
        let f = frame(s, &active, spec, w as f64 / h as f64).expect("a frame");
        let (img, st) = render(s, &active, &f, &Params { width: w, height: h, ssaa, threads, background_srgb: BG, transparent: false });
        (img, st, f)
    }

    fn px(img: &Image, x: u32, y: u32) -> [u8; 3] {
        let o = ((y * img.width + x) * 3) as usize;
        [img.pixels[o], img.pixels[o + 1], img.pixels[o + 2]]
    }

    #[test]
    fn the_white_cubes_three_faces_have_the_lights_shades() {
        let s = testgen::white_cube_scene();
        let (w, h) = (160u32, 120u32);
        let (img, st, f) = draw(&s, w, h, 4, 1, ViewSpec::Auto);
        assert_eq!(st.culled, 6, "three faces culled (6 triangles)");
        let lut = srgb::encode_table();
        let l1 = normalize(KEY_DIR);
        let l2 = normalize(FILL_DIR);
        let mut shades = Vec::new();
        for n in [[1.0f64, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            // The face's normal in view space and its centre on screen.
            let nv = m4_dir(&f.view, n);
            let c = m4_point(&f.view, [n[0] * 0.5, n[1] * 0.5, n[2] * 0.5]);
            let wv = -c[2];
            let sx = ((c[0] * f.fx as f64 / wv) * 0.5 + 0.5) * w as f64;
            let sy = (0.5 - (c[1] * f.fy as f64 / wv) * 0.5) * h as f64;
            let k = AMBIENT as f64
                + KEY as f64 * (nv[0] * l1[0] as f64 + nv[1] * l1[1] as f64 + nv[2] * l1[2] as f64).max(0.0)
                + FILL as f64 * (nv[0] * l2[0] as f64 + nv[1] * l2[1] as f64 + nv[2] * l2[2] as f64).max(0.0);
            let want = lut[q16(k as f32)] as i32;
            let got = px(&img, sx as u32, sy as u32);
            assert!(got[0] == got[1] && got[1] == got[2], "grey: {got:?}");
            assert!((got[0] as i32 - want).abs() <= 1, "face {n:?} at ({sx:.1}, {sy:.1}): {got:?}, want {want}");
            shades.push(got[0]);
        }
        // The top is brightest, then the front (+z, on the left), then the right (+x).
        assert!(shades[1] > shades[2] && shades[2] > shades[0], "{shades:?}");
        assert_eq!(px(&img, 0, 0), BG, "a corner is background");
    }

    #[test]
    fn same_bytes_for_any_thread_count() {
        // Enough triangles and rows to span several chunks and bands.
        let s = testgen::grid_scene(30, false);
        let (a, _, _) = draw(&s, 200, 150, 2, 1, ViewSpec::Turntable(2));
        for t in [2, 3, 8] {
            let (b, _, _) = draw(&s, 200, 150, 2, t, ViewSpec::Turntable(2));
            assert!(a.pixels == b.pixels, "threads {t} differ");
        }
    }

    #[test]
    fn transparent_alpha_is_coverage_and_colour_is_straight() {
        let s = testgen::sphere_scene();
        let active = [0u32];
        let (w, h) = (96u32, 72u32);
        let f = frame(&s, &active, ViewSpec::Auto, w as f64 / h as f64).expect("a frame");
        let p = Params { width: w, height: h, ssaa: 4, threads: 1, background_srgb: BG, transparent: false };
        let (opaque, so) = render(&s, &active, &f, &p);
        let (clear, sc) = render(&s, &active, &f, &Params { transparent: true, ..p });
        assert!(clear.alpha && !opaque.alpha);
        assert_eq!(so.covered_px, sc.covered_px);
        let (mut full, mut part, mut none) = (0, 0, 0);
        for i in 0..(w * h) as usize {
            let (o, c) = (&opaque.pixels[i * 3..i * 3 + 3], &clear.pixels[i * 4..i * 4 + 4]);
            match c[3] {
                0 => {
                    assert_eq!(c, [0, 0, 0, 0]);
                    none += 1;
                }
                255 => {
                    assert_eq!(&c[..3], o, "a covered pixel has the opaque colour");
                    full += 1;
                }
                a => {
                    // 16 samples: alpha is round(255 n / 16) for some n in 1..=15.
                    assert!((1..16).any(|n| (510 * n + 16) / 32 == a as u32), "{a}");
                    part += 1;
                }
            }
        }
        assert!(full > 100 && part > 20 && none > 100, "{full} {part} {none}");
        assert_eq!((full + part) as u64, sc.covered_px);
        // The same bytes for any thread count.
        for t in [2, 5] {
            assert!(render(&s, &active, &f, &Params { transparent: true, threads: t, ..p }).0.pixels == clear.pixels, "threads {t}");
        }
    }

    #[test]
    fn camera_inside_a_box_sees_walls_everywhere() {
        // Near-plane and guard-band clipping: the eye inside a double-sided box.
        let mut s = testgen::white_cube_scene();
        s.objects[0].double_sided = None;
        s.cameras.push(Camera {
            id: "inside".into(),
            position: [0.1, 0.05, 0.2],
            target: Some([0.3, -0.2, -1.0]),
            forward: None,
            up: [0.0, 1.0, 0.0],
            yfov: 1.6,
            aspect: None,
            znear: Some(0.001),
            active: None,
            lens_mm: None,
            sensor_mm: None,
            sensor_fit: None,
            key: None,
        });
        let (img, st, _) = draw(&s, 80, 60, 2, 2, ViewSpec::Camera(0));
        assert_eq!(st.covered_px, 80 * 60, "every pixel shows a wall");
        assert!(st.clipped > 0);
        assert_ne!(px(&img, 40, 30), BG);
    }

    #[test]
    fn culling_follows_double_sided_and_mirroring() {
        // One triangle facing away from the auto camera (it faces -Z; the camera is on the +Z side).
        let tri = Mesh { positions: vec![[-1.0, -1.0, 0.0], [0.0, 1.0, 0.0], [1.0, -1.0, 0.0]], normals: None, indices: vec![[0, 1, 2]], hash: None };
        let one = |ds: Option<bool>, t: [f32; 16]| Scene {
            up: UpAxis::Y,
            meshes: vec![tri.clone()],
            objects: vec![testgen::object("t", 0, [0.8, 0.3, 0.2], t, ds)],
            cameras: vec![],
        };
        assert_eq!(draw(&one(Some(false), testgen::identity()), 32, 32, 2, 1, ViewSpec::Auto).1.covered_px, 0);
        assert!(draw(&one(None, testgen::identity()), 32, 32, 2, 1, ViewSpec::Auto).1.covered_px > 50);
        assert!(draw(&one(Some(true), testgen::identity()), 32, 32, 2, 1, ViewSpec::Auto).1.covered_px > 50);
        // Mirrored in z: the same positions, but the front side now faces the camera.
        assert!(draw(&one(Some(false), testgen::placed([0.0; 3], [1.0, 1.0, -1.0])), 32, 32, 2, 1, ViewSpec::Auto).1.covered_px > 50);
    }

    #[test]
    fn a_mirrored_object_draws_as_the_mirror_image() {
        // The L and its mirror (negative determinant, back faces culled) both draw solid, the same
        // area, mirrored about the image's centre column when seen straight on.
        let mut s = testgen::mirrored_scene();
        s.cameras.push(Camera {
            id: "front".into(),
            position: [0.0, 0.0, 12.0],
            target: Some([0.0, 0.0, 0.0]),
            forward: None,
            up: [0.0, 1.0, 0.0],
            yfov: 0.6,
            aspect: Some(2.0),
            znear: None,
            active: None,
            lens_mm: None,
            sensor_mm: None,
            sensor_fit: None,
            key: None,
        });
        let (img, st, _) = draw(&s, 200, 100, 2, 1, ViewSpec::Camera(0));
        assert!(st.culled > 0);
        // Mirrored coverage, up to a few edge pixels (the fill rule isn't symmetric on an edge).
        let (mut left, mut right, mut differ) = (0u32, 0u32, 0u32);
        for y in 0..100 {
            for x in 0..100 {
                let l = px(&img, x, y) != BG;
                let r = px(&img, 199 - x, y) != BG;
                left += l as u32;
                right += r as u32;
                differ += (l != r) as u32;
            }
        }
        assert!(left > 200 && differ <= 4 && left.abs_diff(right) <= 4, "{left} {right} {differ}");
    }

    #[test]
    fn nearer_triangle_wins_in_either_order() {
        let quad = |z: f32| Mesh {
            positions: vec![[-1.0, -1.0, z], [1.0, -1.0, z], [1.0, 1.0, z], [-1.0, 1.0, z]],
            normals: None,
            indices: vec![[0, 1, 2], [0, 2, 3]],
            hash: None,
        };
        let obj = |mesh: u32, color: [f32; 3]| -> Object { testgen::object(&format!("q{mesh}"), mesh, color, testgen::identity(), None) };
        let cam = Camera {
            id: "front".into(),
            position: [0.0, 0.0, 5.0],
            target: Some([0.0, 0.0, 0.0]),
            forward: None,
            up: [0.0, 1.0, 0.0],
            yfov: 0.8,
            aspect: None,
            znear: None,
            active: None,
            lens_mm: None,
            sensor_mm: None,
            sensor_fit: None,
            key: None,
        };
        for order in [[0u32, 1], [1, 0]] {
            let s = Scene {
                up: UpAxis::Y,
                meshes: vec![quad(0.5), quad(-0.5)],
                objects: order.iter().map(|&m| obj(m, if m == 0 { [1.0, 0.0, 0.0] } else { [0.0, 0.0, 1.0] })).collect(),
                cameras: vec![cam.clone()],
            };
            let (img, _, _) = draw(&s, 40, 40, 1, 1, ViewSpec::Camera(0));
            let c = px(&img, 20, 20);
            assert!(c[0] > 0 && c[2] == 0, "the nearer (red) quad must win: {c:?}");
        }
    }

    /// The id image of the objects `active` of `s` through `spec`, with the image drawn alongside
    /// it and the image drawn without ids.
    fn ids_of(s: &Scene, active: &[u32], spec: ViewSpec, p: &Params, clay: Option<&Clay>, ao: Option<&Ao>) -> (Vec<u32>, Image, Image, Stats) {
        let f = frame(s, active, spec, p.width as f64 / p.height as f64).expect("a frame");
        let (img, ids, st) = render_with_ids(s, active, &f, p, clay, ao);
        assert_eq!(ids.len(), (p.width * p.height) as usize);
        let (plain, _) = render_shaded(s, active, &f, p, clay, ao);
        (ids, img, plain, st)
    }

    #[test]
    fn ids_name_the_object_under_each_pixel() {
        // The white cube: its index + 1 at the centre, 0 in the corners; at one sample per pixel
        // the covered pixels are exactly the ones with an id.
        let s = testgen::white_cube_scene();
        let p = Params { width: 160, height: 120, ssaa: 1, threads: 2, background_srgb: BG, transparent: false };
        let (ids, _, _, st) = ids_of(&s, &[0], ViewSpec::Auto, &p, None, None);
        assert_eq!(ids[60 * 160 + 80], 1);
        assert_eq!((ids[0], ids[159], ids[119 * 160], ids[119 * 160 + 159]), (0, 0, 0, 0));
        assert_eq!(ids.iter().filter(|&&i| i != 0).count() as u64, st.covered_px);
        // Two quads one behind the other: the nearer one's index wins, in either order.
        let quad = |z: f32| Mesh {
            positions: vec![[-1.0, -1.0, z], [1.0, -1.0, z], [1.0, 1.0, z], [-1.0, 1.0, z]],
            normals: None,
            indices: vec![[0, 1, 2], [0, 2, 3]],
            hash: None,
        };
        for (order, nearer) in [([0u32, 1], 1u32), ([1, 0], 2)] {
            let s = Scene {
                up: UpAxis::Y,
                meshes: vec![quad(0.5), quad(-0.5)],
                objects: order.iter().map(|&m| testgen::object(&format!("q{m}"), m, [0.5; 3], testgen::identity(), None)).collect(),
                cameras: vec![looking([0.0, 0.0, 5.0], [0.0, 0.0, 0.0], 0.8)],
            };
            for ssaa in [1, 2, 3, 4] {
                let p = Params { width: 40, height: 40, ssaa, threads: 1, background_srgb: BG, transparent: false };
                let (ids, _, _, _) = ids_of(&s, &[0, 1], ViewSpec::Camera(0), &p, None, None);
                assert_eq!(ids[20 * 40 + 20], nearer, "order {order:?}, ssaa {ssaa}");
                assert_eq!(ids[0], 0, "the background");
                assert!(ids.iter().all(|&i| i == 0 || i == nearer), "the farther quad is hidden everywhere");
                // Drawn alone, the farther one shows.
                let far = if nearer == 1 { 1 } else { 0 };
                let (alone, _, _, _) = ids_of(&s, &[far], ViewSpec::Camera(0), &p, None, None);
                assert_eq!(alone[20 * 40 + 20], far + 1);
            }
        }
    }

    #[test]
    fn ids_are_the_same_for_any_thread_count_and_leave_the_image_alone() {
        let s = testgen::grid_scene(30, false);
        let all: Vec<u32> = (0..s.objects.len() as u32).collect();
        let p = Params { width: 200, height: 150, ssaa: 2, threads: 1, background_srgb: BG, transparent: false };
        let (one, img, plain, _) = ids_of(&s, &all, ViewSpec::Turntable(2), &p, None, None);
        assert!(img.pixels == plain.pixels);
        let distinct: std::collections::BTreeSet<u32> = one.iter().copied().collect();
        assert!(distinct.len() > 100, "{}", distinct.len());
        for t in [2, 3, 8] {
            assert!(ids_of(&s, &all, ViewSpec::Turntable(2), &Params { threads: t, ..p }, None, None).0 == one, "threads {t}");
        }
        // The interior in clay, with occlusion, opaque and transparent: the image is the one drawn
        // without ids, and the ids don't depend on the look.
        let s = testgen::interior_scene();
        let all: Vec<u32> = (0..s.objects.len() as u32).collect();
        let shell: Vec<bool> = (0..all.len()).map(|k| k < 4).collect();
        let clay = Clay { shell: &shell, shell_srgb: [138, 134, 128], contents_srgb: [206, 202, 194] };
        let ao = Ao { radius: 0.4, strength: 1.0 };
        let p = Params { width: 240, height: 180, ssaa: 2, threads: 3, background_srgb: BG, transparent: false };
        let (base, _, _, _) = ids_of(&s, &all, ViewSpec::Auto, &p, None, None);
        for transparent in [false, true] {
            for (c, a) in [(None, None), (Some(&clay), None), (None, Some(&ao)), (Some(&clay), Some(&ao))] {
                let p = Params { transparent, ..p };
                let (ids, img, plain, _) = ids_of(&s, &all, ViewSpec::Auto, &p, c, a);
                assert!(img.pixels == plain.pixels, "transparent {transparent}");
                assert!(ids == base);
                for t in [1, 5] {
                    assert!(ids_of(&s, &all, ViewSpec::Auto, &Params { threads: t, ..p }, c, a).0 == base, "threads {t}");
                }
            }
        }
        let seen: std::collections::BTreeSet<u32> = base.iter().copied().collect();
        assert!(seen.len() >= 8 && seen.iter().all(|&i| i as usize <= all.len()), "{seen:?}");
    }

    #[test]
    fn ids_are_where_the_occlusions_depth_is() {
        // Covered at the depth sample, and only there; an object view holds that object alone.
        let s = testgen::interior_scene();
        let all: Vec<u32> = (0..s.objects.len() as u32).collect();
        for (ssaa, spec) in [(1u32, ViewSpec::Auto), (2, ViewSpec::Turntable(3)), (3, ViewSpec::Auto), (4, ViewSpec::Turntable(6))] {
            let (w, h) = (160u32, 120u32);
            let p = Params { width: w, height: h, ssaa, threads: 2, background_srgb: BG, transparent: true };
            let f = frame(&s, &all, spec, w as f64 / h as f64).expect("a frame");
            let mut st = Stats::default();
            let (ctx, bands) = prepare(&s, &all, &f, &p, None, &mut st);
            let depth = depth_pass(&ctx, &bands, 2, ssaa as usize, w as usize, h as usize);
            let (_, ids, _) = render_with_ids(&s, &all, &f, &p, None, None);
            assert!(depth.iter().zip(&ids).all(|(d, i)| (*d > 0.0) == (*i != 0)), "ssaa {ssaa}");
        }
        let p = Params { width: 120, height: 90, ssaa: 2, threads: 2, background_srgb: BG, transparent: false };
        let (ids, _, _, _) = ids_of(&s, &[5], ViewSpec::Auto, &p, None, None);
        assert!(ids.iter().all(|&i| i == 0 || i == 6) && ids.contains(&6));
    }

    /// The depth image and occlusion factors of every object of `s` through `spec` (strength 1).
    fn ao_factors(s: &Scene, spec: ViewSpec, w: u32, h: u32, ssaa: u32, radius: f32, threads: usize) -> (Vec<f32>, Vec<f32>, Frame) {
        let active: Vec<u32> = (0..s.objects.len() as u32).collect();
        let f = frame(s, &active, spec, w as f64 / h as f64).expect("a frame");
        let p = Params { width: w, height: h, ssaa, threads, background_srgb: BG, transparent: false };
        let mut st = Stats::default();
        let (ctx, bands) = prepare(s, &active, &f, &p, None, &mut st);
        let depth = depth_pass(&ctx, &bands, threads, ssaa as usize, w as usize, h as usize);
        let k = occlusion(&depth, w as usize, h as usize, &Lens::of(&f, ssaa), &Ao { radius, strength: 1.0 }, threads);
        (depth, k, f)
    }

    /// The pixel a world point lands in.
    fn pixel_of(f: &Frame, p: [f64; 3], w: u32, h: u32) -> usize {
        let v = m4_point(&f.view, p);
        let z = -v[2];
        let x = ((v[0] * f.fx as f64 / z + f.shift_x as f64) * 0.5 + 0.5) * w as f64;
        let y = (0.5 - (v[1] * f.fy as f64 / z + f.shift_y as f64) * 0.5) * h as f64;
        assert!(x >= 0.0 && y >= 0.0 && x < w as f64 && y < h as f64, "{p:?} is off the image: {x} {y}");
        y as usize * w as usize + x as usize
    }

    fn looking(position: [f32; 3], target: [f32; 3], yfov: f32) -> Camera {
        Camera {
            id: "look".into(),
            position,
            target: Some(target),
            forward: None,
            up: [0.0, 1.0, 0.0],
            yfov,
            aspect: None,
            znear: None,
            active: None,
            lens_mm: None,
            sensor_mm: None,
            sensor_fit: None,
            key: None,
        }
    }

    /// A flat n x n grid of quads (2 n^2 triangles) on y = 0, `size` across.
    fn flat_grid(n: u32, size: f32) -> Mesh {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                positions.push([(i as f32 / n as f32 - 0.5) * size, 0.0, (j as f32 / n as f32 - 0.5) * size]);
            }
        }
        for j in 0..n {
            for i in 0..n {
                let a = j * (n + 1) + i;
                indices.push([a, a + n + 1, a + 1]);
                indices.push([a + 1, a + n + 1, a + n + 2]);
            }
        }
        Mesh { positions, normals: None, indices, hash: None }
    }

    #[test]
    fn occlusion_leaves_a_flat_plane_alone() {
        // A plane cut into 512 triangles, seen at a slant from off to one side, through the
        // horizon (its far edge against the empty background): no pixel is occluded at all.
        let s = Scene {
            up: UpAxis::Y,
            meshes: vec![flat_grid(16, 20.0)],
            objects: vec![testgen::object("plane", 0, [0.8, 0.8, 0.8], testgen::identity(), None)],
            cameras: vec![looking([3.0, 2.0, 7.0], [-1.0, 0.0, -2.0], 0.9)],
        };
        for (w, h, ssaa) in [(160u32, 120u32, 4u32), (640, 480, 2)] {
            let (depth, k, _) = ao_factors(&s, ViewSpec::Camera(0), w, h, ssaa, 1.0, 2);
            let covered = depth.iter().filter(|&&d| d > 0.0).count();
            assert!(covered > (w * h / 3) as usize && covered < (w * h) as usize, "{covered}");
            let occluded = k.iter().filter(|&&f| f != 1.0).count();
            assert_eq!(occluded, 0, "{w} x {h}: a plane is never occluded");
        }
    }

    #[test]
    fn occlusion_darkens_an_inside_corner_and_only_near_it() {
        // A floor slab and a wall standing on it at z = -3: dark along the crease, untouched a
        // couple of radii away from it, on the floor and up the wall.
        let s = Scene {
            up: UpAxis::Y,
            meshes: vec![testgen::boxes(&[([-3.0, -0.1, -3.0], [3.0, 0.0, 3.0])]), testgen::boxes(&[([-3.0, 0.0, -3.0], [3.0, 3.0, -2.9])])],
            objects: vec![
                testgen::object("floor", 0, [0.8, 0.8, 0.8], testgen::identity(), Some(false)),
                testgen::object("wall", 1, [0.8, 0.8, 0.8], testgen::identity(), Some(false)),
            ],
            cameras: vec![looking([1.0, 2.0, 5.0], [0.0, 0.5, -3.0], 0.9)],
        };
        let (w, h) = (320u32, 240u32);
        let (_, k, f) = ao_factors(&s, ViewSpec::Camera(0), w, h, 2, 0.4, 3);
        let at = |p: [f64; 3]| k[pixel_of(&f, p, w, h)];
        let near_floor = at([0.0, 0.0, -2.85]);
        let near_wall = at([0.0, 0.05, -2.9]);
        let darkest = k.iter().fold(1f32, |m, &f| m.min(f));
        eprintln!("[ao] corner: {near_floor} on the floor, {near_wall} on the wall, {darkest} at the darkest");
        assert!(near_floor < 0.85 && near_wall < 0.85 && darkest < 0.75, "the crease: {near_floor} {near_wall} {darkest}");
        // Farther along the floor, lighter; past twice the radius, nothing.
        assert!(at([0.0, 0.0, -2.6]) > near_floor, "{} {near_floor}", at([0.0, 0.0, -2.6]));
        assert_eq!(at([0.0, 0.0, -1.0]), 1.0);
        assert_eq!(at([0.0, 0.0, 1.5]), 1.0);
        assert_eq!(at([0.0, 1.5, -2.9]), 1.0);
        assert!(k.iter().all(|&f| (0.0..=1.0).contains(&f)));
    }

    #[test]
    fn occlusion_of_the_empty_background_is_none() {
        // Nothing drawn: every factor 1.
        let lens = Lens { fx: 1.5, fy: 2.0, shift: [0.0, 0.0], at: 0.5 };
        assert!(occlusion(&[0.0; 64 * 48], 64, 48, &lens, &Ao { radius: 0.5, strength: 1.0 }, 2).iter().all(|&f| f == 1.0));
        // Convex objects on the empty background: their edges bend away from the eye and the
        // background around them never occludes, so they are untouched too.
        for s in [testgen::white_cube_scene(), testgen::sphere_scene()] {
            let (depth, k, _) = ao_factors(&s, ViewSpec::Auto, 200, 150, 2, 0.3, 1);
            assert!(depth.iter().filter(|&&d| d > 0.0).count() > 2000);
            assert!(k.iter().all(|&f| f == 1.0));
        }
    }

    #[test]
    fn occlusion_never_crosses_a_depth_edge() {
        // A box floating well in front of a wall (more than twice the radius): the wall around
        // its outline and the box's own faces stay untouched, though the half grid's points on
        // the box sit next to pixels of the wall and the other way round (both widths are above
        // AO_FULL_GRID_MAX_WIDTH, one odd).
        let s = Scene {
            up: UpAxis::Y,
            meshes: vec![testgen::boxes(&[([-8.0, -6.0, -3.0], [8.0, 6.0, -2.9])]), testgen::boxes(&[([-0.37, -0.41, -1.0], [0.43, 0.39, -0.2])])],
            objects: vec![
                testgen::object("wall", 0, [0.8, 0.8, 0.8], testgen::identity(), Some(false)),
                testgen::object("box", 1, [0.8, 0.8, 0.8], testgen::identity(), Some(false)),
            ],
            cameras: vec![looking([0.6, 0.5, 3.0], [0.0, 0.0, -2.9], 0.9)],
        };
        for (w, h) in [(961u32, 721u32), (1280, 960)] {
            let (depth, k, _) = ao_factors(&s, ViewSpec::Camera(0), w, h, 2, 0.3, 3);
            assert!(depth.iter().filter(|&&d| d > 0.0).count() == (w * h) as usize, "{w} x {h}: the wall fills the frame");
            assert!(k.iter().all(|&f| f == 1.0), "{w} x {h}: nothing occluded");
        }
    }

    #[test]
    fn occlusion_is_the_same_for_any_thread_count_and_alike_at_any_width() {
        let s = testgen::interior_scene();
        let (_, one, _) = ao_factors(&s, ViewSpec::Auto, 300, 225, 2, 0.4, 1);
        for t in [2, 5] {
            assert!(ao_factors(&s, ViewSpec::Auto, 300, 225, 2, 0.4, t).1 == one, "threads {t}");
        }
        // The radius is a world size: the same share of the picture is darkened, as much, at any
        // width (the mean darkening over the covered pixels within a tenth).
        let mean = |w: u32, h: u32, ssaa: u32| -> f64 {
            let (depth, k, _) = ao_factors(&s, ViewSpec::Auto, w, h, ssaa, 0.4, 4);
            let (mut sum, mut n) = (0f64, 0u32);
            for (d, f) in depth.iter().zip(&k) {
                if *d > 0.0 {
                    sum += 1.0 - *f as f64;
                    n += 1;
                }
            }
            sum / n as f64
        };
        let (small, large) = (mean(320, 240, 4), mean(1280, 960, 2));
        eprintln!("[ao] mean darkening: {small:.4} at 320 px, {large:.4} at 1280 px");
        assert!(small > 0.01 && (small - large).abs() < 0.1 * large, "{small} {large}");
    }

    #[test]
    fn occlusion_darkens_the_colour_never_the_coverage() {
        let s = testgen::interior_scene();
        let active: Vec<u32> = (0..s.objects.len() as u32).collect();
        let (w, h) = (200u32, 150u32);
        let f = frame(&s, &active, ViewSpec::Turntable(1), w as f64 / h as f64).expect("a frame");
        let ao = Ao { radius: 0.4, strength: 0.8 };
        for transparent in [false, true] {
            let p = Params { width: w, height: h, ssaa: 2, threads: 2, background_srgb: BG, transparent };
            let (plain, sp) = render(&s, &active, &f, &p);
            // Strength 0: every factor exactly 1, so the plain image's bytes.
            let (zero, _) = render_shaded(&s, &active, &f, &p, None, Some(&Ao { strength: 0.0, ..ao }));
            assert!(zero.pixels == plain.pixels, "transparent {transparent}");
            let (dark, sd) = render_shaded(&s, &active, &f, &p, None, Some(&ao));
            assert_eq!(sp.covered_px, sd.covered_px);
            assert!(sd.ao_us > 0 && sp.ao_us == 0);
            let ch = if transparent { 4 } else { 3 };
            let mut darker = 0;
            for (a, b) in plain.pixels.chunks(ch).zip(dark.pixels.chunks(ch)) {
                assert!(b[..3].iter().zip(&a[..3]).all(|(d, p)| d <= p), "{a:?} {b:?}");
                if transparent {
                    assert_eq!(a[3], b[3], "the alpha is the coverage");
                }
                darker += (a[..3] != b[..3]) as u32;
            }
            assert!(darker > 500, "{darker}");
            // The same bytes for any thread count.
            for t in [1, 4] {
                assert!(render_shaded(&s, &active, &f, &Params { threads: t, ..p }, None, Some(&ao)).0.pixels == dark.pixels, "threads {t}");
            }
        }
    }

    #[test]
    fn degenerate_scenes_draw_without_a_crash() {
        for (name, s) in testgen::degenerate_scenes() {
            let active: Vec<u32> = (0..s.objects.len() as u32).collect();
            let specs = if s.cameras.is_empty() { vec![ViewSpec::Auto, ViewSpec::Turntable(5)] } else { vec![ViewSpec::Camera(0)] };
            for spec in specs {
                if let Ok(f) = frame(&s, &active, spec, 4.0 / 3.0) {
                    for transparent in [false, true] {
                        let p = Params { width: 40, height: 30, ssaa: 2, threads: 2, background_srgb: BG, transparent };
                        let (img, _) = render(&s, &active, &f, &p);
                        assert_eq!(img.pixels.len(), 40 * 30 * if transparent { 4 } else { 3 }, "{name}");
                        let (img, _) = render_shaded(&s, &active, &f, &p, None, Some(&Ao { radius: 0.5, strength: 1.0 }));
                        assert_eq!(img.pixels.len(), 40 * 30 * if transparent { 4 } else { 3 }, "{name} with occlusion");
                    }
                }
            }
        }
    }
}
