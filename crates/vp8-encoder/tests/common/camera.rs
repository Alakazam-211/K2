//! Synthetic camera-like content for the encoder's tests and examples: a
//! panning scene with detail at every scale, a moving object and sensor
//! noise. Deterministic, so every run sees the same frames.

/// A small xorshift generator: the content must be the same on every run.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

fn lattice(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393)
        ^ (y as u32).wrapping_mul(668_265_263)
        ^ seed.wrapping_mul(2_246_822_519);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^= h >> 16;
    (h & 0xffff) as f32 / 65535.0 * 2.0 - 1.0
}

/// Smooth value noise in -1..1 with features about `period` pixels apart.
fn value_noise(x: f32, y: f32, period: f32, seed: u32) -> f32 {
    let (fx, fy) = (x / period, y / period);
    let (ix, iy) = (fx.floor() as i32, fy.floor() as i32);
    let (tx, ty) = (fx - ix as f32, fy - iy as f32);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let top = lattice(ix, iy, seed) * (1.0 - sx) + lattice(ix + 1, iy, seed) * sx;
    let bottom = lattice(ix, iy + 1, seed) * (1.0 - sx) + lattice(ix + 1, iy + 1, seed) * sx;
    top * (1.0 - sy) + bottom * sy
}

/// A still "scene" larger than the frame, with detail at every scale (a
/// 1/f-like spectrum, as in natural pictures) and some hard edges.
struct Scene {
    w: usize,
    luma: Vec<f32>,
    cb: Vec<f32>,
    cr: Vec<f32>,
}

impl Scene {
    fn new(w: usize, h: usize, seed: u32) -> Scene {
        // Scale features with the frame, so 360p and 720p show the same
        // scene at two resolutions (the 720p one has more fine detail).
        let s = h as f32 / 360.0;
        let octaves = [
            (96.0, 46.0),
            (48.0, 30.0),
            (24.0, 20.0),
            (12.0, 13.0),
            (6.0, 8.0),
            (3.0, 5.0),
        ];
        let mut luma = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32, y as f32);
                let mut v = 118.0;
                for (i, &(period, amp)) in octaves.iter().enumerate() {
                    v += amp * value_noise(fx, fy, period * s, seed + i as u32);
                }
                // Hard-edged shapes: shelves, window frames, text-like marks.
                if value_noise(fx, fy, 40.0 * s, seed + 99) > 0.35 {
                    v += 45.0;
                }
                if value_noise(fx, fy, 7.0 * s, seed + 77) > 0.6 {
                    v -= 35.0;
                }
                luma[y * w + x] = v;
            }
        }
        let (cw, ch) = (w / 2, h / 2);
        let mut cb = vec![0f32; cw * ch];
        let mut cr = vec![0f32; cw * ch];
        for y in 0..ch {
            for x in 0..cw {
                let (fx, fy) = (x as f32, y as f32);
                cb[y * cw + x] = 128.0
                    + 18.0 * value_noise(fx, fy, 40.0 * s, seed + 200)
                    + 5.0 * value_noise(fx, fy, 6.0 * s, seed + 201);
                cr[y * cw + x] = 128.0
                    + 14.0 * value_noise(fx, fy, 50.0 * s, seed + 300)
                    + 4.0 * value_noise(fx, fy, 5.0 * s, seed + 301);
            }
        }
        Scene { w, luma, cb, cr }
    }
}

fn bilinear(plane: &[f32], stride: usize, x: f32, y: f32) -> f32 {
    let (ix, iy) = (x as usize, y as usize);
    let (tx, ty) = (x - ix as f32, y - iy as f32);
    let p = |xx: usize, yy: usize| plane[yy * stride + xx];
    let top = p(ix, iy) * (1.0 - tx) + p(ix + 1, iy) * tx;
    let bottom = p(ix, iy + 1) * (1.0 - tx) + p(ix + 1, iy + 1) * tx;
    top * (1.0 - ty) + bottom * ty
}

/// Camera-like frames: the camera pans slowly across the scene (sub-pixel
/// motion), a textured object moves on its own path, and every frame has
/// a little sensor noise.
pub struct Camera {
    w: usize,
    h: usize,
    scene: Scene,
}

impl Camera {
    /// `frames`: how many frames the pan must cover.
    pub fn new(w: usize, h: usize, frames: usize) -> Camera {
        let s = h as f32 / 360.0;
        let pan = (frames as f32 * 1.6 * s) as usize + 8;
        Camera {
            w,
            h,
            scene: Scene::new(w + pan, h + pan / 2, 7),
        }
    }

    /// Frame `t` as tightly packed I420.
    pub fn frame(&self, t: usize) -> Vec<u8> {
        let (w, h) = (self.w, self.h);
        let s = h as f32 / 360.0;
        let (ox, oy) = (t as f32 * 1.3 * s, t as f32 * 0.45 * s);
        let (cx, cy) = (
            w as f32 * (0.5 + 0.3 * (t as f32 / 23.0).sin()),
            h as f32 * (0.5 + 0.2 * (t as f32 / 17.0).cos()),
        );
        let r = h as f32 / 6.0;
        let mut rng = Rng((t as u32).wrapping_mul(2_654_435_761) | 1);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut out = vec![0u8; w * h + 2 * cw * ch];
        let sc = &self.scene;
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let v = if dx * dx + dy * dy < r * r {
                    // The object: the scene's texture from far away, brighter.
                    30.0 + bilinear(
                        &sc.luma,
                        sc.w,
                        (dx + r + 3.0).max(0.0) * 1.7 + 2.0,
                        (dy + r + 3.0).max(0.0) * 1.1 + 2.0,
                    )
                } else {
                    bilinear(&sc.luma, sc.w, x as f32 + ox, y as f32 + oy)
                };
                // Triangular noise of about 1.2 levels standard deviation.
                let n = (rng.next() % 4) as f32 + (rng.next() % 4) as f32 - 3.0;
                out[y * w + x] = (v + n).round().clamp(0.0, 255.0) as u8;
            }
        }
        let csw = sc.w / 2;
        for y in 0..ch {
            for x in 0..cw {
                let (sx, sy) = (x as f32 + ox / 2.0, y as f32 + oy / 2.0);
                out[w * h + y * cw + x] =
                    bilinear(&sc.cb, csw, sx, sy).round().clamp(0.0, 255.0) as u8;
                out[w * h + cw * ch + y * cw + x] =
                    bilinear(&sc.cr, csw, sx, sy).round().clamp(0.0, 255.0) as u8;
            }
        }
        out
    }
}
