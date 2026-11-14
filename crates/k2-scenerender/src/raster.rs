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
//! **Faces:** `double_sided == Some(false)` culls back faces (glTF's default); unknown or true
//! draws both sides, lighting the back with the flipped normal (two-sided lighting). Winding
//! follows glTF: counter-clockwise is front, flipped when the object transform mirrors.
//!
//! **Lighting:** base colour x (ambient + key + fill), two directional lights fixed in view space
//! (so every turntable frame is lit the same way). Smooth (Gouraud, perspective-correct) when the
//! mesh has normals, flat otherwise.

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
    pub raster_us: u64,
}

#[derive(Clone, Copy)]
struct Cam {
    fx: f32,
    fy: f32,
    near: f32,
    sx: f32,
    sy: f32,
}

struct Obj {
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

/// Render the active objects: opaque RGB8 on the background colour, or RGBA8 when transparent.
pub fn render(scene: &Scene, active: &[u32], frame: &Frame, p: &Params) -> (Image, Stats) {
    let s = p.ssaa.clamp(1, 4) as i64;
    let ws = p.width as i64 * s;
    let hs = p.height as i64 * s;
    let cam = Cam { fx: frame.fx, fy: frame.fy, near: frame.near, sx: (ws * ONE) as f32, sy: (hs * ONE) as f32 };
    let mut st = Stats::default();

    let t0 = Instant::now();
    let (objs, vpos, vnor, vfix) = vertex_stage(scene, active, frame, &cam, p.threads);
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
    let bands = bin(&ctx, p.threads, &mut st);
    let t2 = Instant::now();
    st.bin_us = (t2 - t1).as_micros() as u64;

    let lut = srgb::encode_table();
    let bg = [srgb::to_linear(p.background_srgb[0]), srgb::to_linear(p.background_srgb[1]), srgb::to_linear(p.background_srgb[2])];
    let w = p.width as usize;
    let ch = if p.transparent { 4 } else { 3 };
    let mut pixels = vec![0u8; w * p.height as usize * ch];
    let covered = AtomicU64::new(0);
    {
        let items: Vec<(usize, &mut [u8])> = pixels.chunks_mut(BAND_PX as usize * w * ch).enumerate().collect();
        let n = (ws * band_rows) as usize;
        par_each(
            p.threads,
            items,
            || Band { color: vec![[0.0; 3]; n], depth: vec![0.0; n] },
            |buf, (b, out)| {
                let c = raster_band(&ctx, b, &bands[b], buf, out, bg, &lut, s as usize, w, p.transparent);
                covered.fetch_add(c, Ordering::Relaxed);
            },
        );
    }
    st.covered_px = covered.load(Ordering::Relaxed);
    st.raster_us = t2.elapsed().as_micros() as u64;
    (Image { width: p.width, height: p.height, alpha: p.transparent, pixels }, st)
}

type VertexOut = (Vec<Obj>, Vec<V3>, Vec<V3>, Vec<[i32; 2]>);

fn vertex_stage(scene: &Scene, active: &[u32], frame: &Frame, cam: &Cam, threads: usize) -> VertexOut {
    let mut objs = Vec::with_capacity(active.len());
    let mut vb = 0usize;
    let mut tb = 0u64;
    let mut any_smooth = false;
    for &oi in active {
        let o = &scene.objects[oi as usize];
        let m = &scene.meshes[o.mesh as usize];
        let smooth = m.normals.is_some();
        any_smooth |= smooth;
        objs.push(Obj {
            vbase: vb,
            tbase: tb,
            tris: m.indices.len() as u64,
            mesh: o.mesh as usize,
            color: [o.base_color[0], o.base_color[1], o.base_color[2]],
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
    let x = v[0] * cam.fx;
    let y = v[1] * cam.fy;
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

#[inline]
fn clip_of(v: V3, cam: &Cam) -> [f32; 3] {
    [v[0] * cam.fx, v[1] * cam.fy, -v[2]]
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
}

#[derive(Clone, Copy)]
struct SV {
    x: i64,
    y: i64,
    iw: f32,
    c: [f32; 3],
}

#[allow(clippy::too_many_arguments)]
fn raster_band(ctx: &Ctx, b: usize, refs: &[u32], buf: &mut Band, out: &mut [u8], bg: [f32; 3], lut: &[u8], s: usize, w: usize, transparent: bool) -> u64 {
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
    // Only depth is cleared: a sample's colour is read only where its depth was written.
    let n = (y_hi - y_lo) as usize * ws;
    buf.depth[..n].fill(0.0);
    let mut oi = 0usize;
    for &g in refs {
        let g = g as u64;
        while g >= ctx.objs[oi].tbase + ctx.objs[oi].tris {
            oi += 1;
        }
        let ob = &ctx.objs[oi];
        draw_tri(ctx, ob, tri_vertices(ctx, ob, g), buf, y_lo, y_hi);
    }
    if transparent {
        resolve_rgba(buf, (y_hi - y_lo) as usize, ws, s, out, w, lut)
    } else {
        resolve(buf, (y_hi - y_lo) as usize, ws, s, out, w, lut, bg)
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

fn draw_tri(ctx: &Ctx, ob: &Obj, v: [usize; 3], buf: &mut Band, y_lo: i64, y_hi: i64) {
    let f = [ctx.vfix[v[0]], ctx.vfix[v[1]], ctx.vfix[v[2]]];
    let fast = f.iter().all(|p| p[0] != NEEDS_CLIP);
    let flat = !ob.smooth;
    if fast {
        let area = edge(f[0], f[1], f[2]);
        if area == 0 {
            return;
        }
        let front = (area < 0) != ob.mirror;
        if ob.cull && !front {
            return;
        }
        let col = shade(ctx, ob, v, front);
        let sv = |k: usize| SV { x: f[k][0] as i64, y: f[k][1] as i64, iw: 1.0 / -ctx.vpos[v[k]][2], c: col[k] };
        raster_tri(buf, [sv(0), sv(1), sv(2)], flat, y_lo, y_hi, ctx.ws);
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
        let col = shade(ctx, ob, v, front);
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
            raster_tri(buf, [sv[0], sv[k], sv[k + 1]], flat, y_lo, y_hi, ctx.ws);
        }
    }
}

/// Fill one triangle's samples in rows [y_lo, y_hi) (sample space).
fn raster_tri(buf: &mut Band, v: [SV; 3], flat: bool, y_lo: i64, y_hi: i64, ws: i64) {
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
            ea += sxa;
            eb += sxb;
            ec += sxc;
        }
        ra += sya;
        rb += syb;
        rc += syc;
    }
}

/// Box-filter the band's samples into its output rows; returns how many pixels any triangle covered.
#[allow(clippy::too_many_arguments)]
fn resolve(buf: &Band, rows_s: usize, ws: usize, s: usize, out: &mut [u8], w: usize, lut: &[u8], bg: [f32; 3]) -> u64 {
    let rows = rows_s / s;
    let inv = 1.0 / (s * s) as f32;
    let mut covered = 0u64;
    for r in 0..rows {
        for px in 0..w {
            let mut acc = [0f32; 3];
            let mut hit = false;
            for sy in 0..s {
                let base = (r * s + sy) * ws + px * s;
                for sx in 0..s {
                    let c = if buf.depth[base + sx] > 0.0 {
                        hit = true;
                        buf.color[base + sx]
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
/// in the same order as [`resolve`] (so a fully covered pixel has the opaque colour).
fn resolve_rgba(buf: &Band, rows_s: usize, ws: usize, s: usize, out: &mut [u8], w: usize, lut: &[u8]) -> u64 {
    let rows = rows_s / s;
    let total = (s * s) as u32;
    let mut covered = 0u64;
    for r in 0..rows {
        for px in 0..w {
            let mut acc = [0f32; 3];
            let mut n = 0u32;
            for sy in 0..s {
                let base = (r * s + sy) * ws + px * s;
                for sx in 0..s {
                    if buf.depth[base + sx] > 0.0 {
                        let c = buf.color[base + sx];
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

    #[test]
    fn degenerate_scenes_draw_without_a_crash() {
        for (name, s) in testgen::degenerate_scenes() {
            let active: Vec<u32> = (0..s.objects.len() as u32).collect();
            let specs = if s.cameras.is_empty() { vec![ViewSpec::Auto, ViewSpec::Turntable(5)] } else { vec![ViewSpec::Camera(0)] };
            for spec in specs {
                if let Ok(f) = frame(&s, &active, spec, 4.0 / 3.0) {
                    for transparent in [false, true] {
                        let (img, _) = render(&s, &active, &f, &Params { width: 40, height: 30, ssaa: 2, threads: 2, background_srgb: BG, transparent });
                        assert_eq!(img.pixels.len(), 40 * 30 * if transparent { 4 } else { 3 }, "{name}");
                    }
                }
            }
        }
    }
}
