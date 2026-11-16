// SAMPLE ONLY. The stand-in still: a small z-buffered rasteriser that draws
// the sample's shapes the way K2 Render draws a clay still (ported from the
// mock's still.js): flat Lambert in linear light with K2 Render's lights
// (ambient 0.22, a key of 0.70 and a fill of 0.28, fixed in view space),
// its two clay tones (shell sRGB 138,134,128; contents 206,202,194), its
// screen-space AO, no background (alpha 0), and an id image beside it
// (0 background, index + 1 per object, as `layer=ids`). Pure: RGBA and
// values in typed arrays, no DOM, so it runs in tests.

import { worldOf } from '../fold'
import { norm, toCamera, type Basis } from '../project'
import type { Doc } from '../types'
import { SHELL_SHAPES, drawable, shapeKey, trisOf, unitsOf, worldPoints, xf } from './shapes'

const AMBIENT = 0.22
const KEY = 0.7
const FILL = 0.28
const KEY_DIR = norm([-0.4, 0.6, 0.7])
const FILL_DIR = norm([0.6, -0.1, 0.5])
/** K2 Render's clay as linear albedo (through its sRGB table). */
const SHELL = [0.2541521, 0.23839757, 0.2158605]
const CONTENTS = [0.6172066, 0.59061885, 0.5394795]
const NEAR = 0.05

const srgbEnc = (c: number): number => {
  const v = Math.max(0, Math.min(1, c))
  return Math.round(255 * (v <= 0.0031308 ? 12.92 * v : 1.055 * Math.pow(v, 1 / 2.4) - 0.055))
}

export interface Raster {
  width: number
  height: number
  /** RGBA, alpha 0 where nothing is drawn. */
  rgba: Uint8ClampedArray
  /** 0 background, else the object's index + 1 (K2 Render's id image). */
  ids: Uint32Array
  /** The id table: index → address. */
  objects: { index: number; id: string }[]
  ms: number
}

/** Draw the doc's drawable objects through a camera at W × H. */
export function rasterise(D: Doc, cam: Basis, W: number, H: number): Raster {
  const t0 = typeof performance !== 'undefined' ? performance.now() : Date.now()
  const rgba = new Uint8ClampedArray(W * H * 4)
  const px = new Uint32Array(rgba.buffer)
  const zb = new Float32Array(W * H)
  const ids = new Uint32Array(W * H)
  const objs = unitsOf(D).filter((a) => drawable(D, a))
  const objects = objs.map((id, index) => ({ index, id }))

  const raster = (a: { x: number; y: number; iz: number }, b: { x: number; y: number; iz: number }, c: { x: number; y: number; iz: number }, cw: number, v: number): void => {
    const area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
    if (Math.abs(area) < 1e-9) return
    const x0 = Math.max(0, Math.floor(Math.min(a.x, b.x, c.x)))
    const x1 = Math.min(W - 1, Math.ceil(Math.max(a.x, b.x, c.x)))
    const y0 = Math.max(0, Math.floor(Math.min(a.y, b.y, c.y)))
    const y1 = Math.min(H - 1, Math.ceil(Math.max(a.y, b.y, c.y)))
    if (x0 > x1 || y0 > y1) return
    const inv = 1 / area
    const e0x = (b.y - c.y) * inv
    const e1x = (c.y - a.y) * inv
    for (let y = y0; y <= y1; y++) {
      const py = y + 0.5
      let w0 = ((b.x - (x0 + 0.5)) * (c.y - py) - (b.y - py) * (c.x - (x0 + 0.5))) * inv
      let w1 = ((c.x - (x0 + 0.5)) * (a.y - py) - (c.y - py) * (a.x - (x0 + 0.5))) * inv
      let row = y * W + x0
      for (let x = x0; x <= x1; x++, row++, w0 += e0x, w1 += e1x) {
        const w2 = 1 - w0 - w1
        if (w0 < -1e-6 || w1 < -1e-6 || w2 < -1e-6) continue
        const iz = w0 * a.iz + w1 * b.iz + w2 * c.iz
        if (iz > zb[row]) {
          zb[row] = iz
          px[row] = cw
          ids[row] = v
        }
      }
    }
  }

  objs.forEach((a, oi) => {
    const w = worldOf(D, a)
    const key = shapeKey(D, a)
    const T = trisOf(key)
    if (!w || !T) return
    const alb = key && SHELL_SHAPES.has(key) ? SHELL : CONTENTS
    const f = T.f
    for (let t = 0; t < T.n * 3; t += 3) {
      const P = [0, 1, 2].map((k) => xf(w, [f[(t + k) * 3], f[(t + k) * 3 + 1], f[(t + k) * 3 + 2]]))
      const e1 = [P[1][0] - P[0][0], P[1][1] - P[0][1], P[1][2] - P[0][2]]
      const e2 = [P[2][0] - P[0][0], P[2][1] - P[0][1], P[2][2] - P[0][2]]
      let n = norm([e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]])
      const toEye = [cam.eye[0] - P[0][0], cam.eye[1] - P[0][1], cam.eye[2] - P[0][2]]
      if (n[0] * toEye[0] + n[1] * toEye[1] + n[2] * toEye[2] < 0) n = [-n[0], -n[1], -n[2]]
      // The normal in view space (x right, y up, z toward the viewer), then K2 Render's two lights.
      const nv = [n[0] * cam.r[0] + n[1] * cam.r[1] + n[2] * cam.r[2], n[0] * cam.u[0] + n[1] * cam.u[1] + n[2] * cam.u[2], -(n[0] * cam.f[0] + n[1] * cam.f[1] + n[2] * cam.f[2])]
      const lit = AMBIENT + KEY * Math.max(0, nv[0] * KEY_DIR[0] + nv[1] * KEY_DIR[1] + nv[2] * KEY_DIR[2]) + FILL * Math.max(0, nv[0] * FILL_DIR[0] + nv[1] * FILL_DIR[1] + nv[2] * FILL_DIR[2])
      const col = alb.map((c) => srgbEnc(c * lit))
      const cw = ((255 << 24) | (col[2] << 16) | (col[1] << 8) | col[0]) >>> 0
      // Camera space, clipped on the near plane.
      let poly = P.map((p) => toCamera(cam, p) as number[])
      if (poly.some((v) => v[2] < NEAR)) {
        const out: number[][] = []
        for (let i = 0; i < poly.length; i++) {
          const A = poly[i]
          const Bv = poly[(i + 1) % poly.length]
          const ia = A[2] >= NEAR
          const ib = Bv[2] >= NEAR
          if (ia) out.push(A)
          if (ia !== ib) {
            const s = (NEAR - A[2]) / (Bv[2] - A[2])
            out.push([A[0] + (Bv[0] - A[0]) * s, A[1] + (Bv[1] - A[1]) * s, NEAR])
          }
        }
        poly = out
        if (poly.length < 3) continue
      }
      const S2 = poly.map((v) => ({ x: (v[0] / (v[2] * cam.fy * cam.aspect) - cam.sx + 1) * 0.5 * W, y: (1 - (v[1] / (v[2] * cam.fy) - cam.sy)) * 0.5 * H, iz: 1 / v[2] }))
      for (let k = 1; k + 1 < S2.length; k++) raster(S2[0], S2[k], S2[k + 1], cw, oi + 1)
    }
  })

  if (W >= 640 && objs.length) ambientOcclusion(D, objs, cam, W, H, px, zb, ids)
  const ms = Math.round((typeof performance !== 'undefined' ? performance.now() : Date.now()) - t0)
  return { width: W, height: H, rgba, ids, objects, ms }
}

/** K2 Render's AO, approximated from the depth buffer: creases, inside corners and the floor round and under objects darken (colour only; alpha untouched). */
function ambientOcclusion(D: Doc, objs: string[], cam: Basis, W: number, H: number, px: Uint32Array, zb: Float32Array, id: Uint32Array): void {
  const box = worldPoints(D, objs, false)
  const R = box ? (0.1 * Math.hypot(box.max[0] - box.min[0], box.max[1] - box.min[1], box.max[2] - box.min[2])) / 2 : 0.4
  const occ = new Float32Array(W * H)
  const dirs: [number, number][] = []
  for (let k = 0; k < 12; k++) for (const f of [0.35, 0.7, 1]) dirs.push([(k * Math.PI) / 6, f])
  const step = 2
  for (let y = 0; y < H; y += step)
    for (let x = 0; x < W; x += step) {
      const i = y * W + x
      if (id[i] === 0) continue
      const z = 1 / zb[i]
      const rp = (R * (H / 2)) / (z * cam.fy)
      if (rp < 1.5) continue
      // The pixel's own surface plane (1/z is linear across a plane in screen space): only samples in front of it occlude.
      const iz = zb[i]
      const izAt = (xx: number, yy: number): number | null => {
        if (xx < 0 || yy < 0 || xx >= W || yy >= H) return null
        const k = yy * W + xx
        return id[k] === id[i] ? zb[k] : null
      }
      const a1 = izAt(x - 2, y)
      const a2 = izAt(x + 2, y)
      const b1 = izAt(x, y - 2)
      const b2 = izAt(x, y + 2)
      const gx = a1 != null && a2 != null ? (a2 - a1) / 4 : a2 != null ? (a2 - iz) / 2 : a1 != null ? (iz - a1) / 2 : 0
      const gy = b1 != null && b2 != null ? (b2 - b1) / 4 : b2 != null ? (b2 - iz) / 2 : b1 != null ? (iz - b1) / 2 : 0
      let o = 0
      const rot = (((x * 73856093) ^ (y * 19349663)) % 628) / 100
      const rf = 0.75 + ((x * 31 + y * 17) & 7) / 28
      for (const [a0, f0] of dirs) {
        const ox = Math.cos(a0 + rot) * f0 * rf * rp
        const oy = Math.sin(a0 + rot) * f0 * rf * rp
        const sx2 = Math.round(x + ox)
        const sy2 = Math.round(y + oy)
        if (sx2 < 0 || sy2 < 0 || sx2 >= W || sy2 >= H) continue
        const j2 = sy2 * W + sx2
        if (id[j2] === 0) continue
        const zs = 1 / zb[j2]
        const pi = iz + gx * ox + gy * oy
        const plane = pi > 1e-6 ? 1 / pi : 1e9
        const d = plane - zs
        const bias = 0.03 + 0.012 * z
        if (d > bias && d < R * 2.5) o += 1 - d / (R * 2.5)
      }
      const v = Math.min(1, (o / dirs.length) * 1.8)
      for (let yy = y; yy < Math.min(H, y + step); yy++) for (let xx = x; xx < Math.min(W, x + step); xx++) occ[yy * W + xx] = v
    }
  // A small box blur, so the darkening is soft.
  const tmp = new Float32Array(W * H)
  const rb = 5
  for (let y = 0; y < H; y++) {
    let acc = 0
    let n = 0
    for (let x = -rb; x < W; x++) {
      if (x + rb < W) { acc += occ[y * W + x + rb]; n++ }
      if (x - rb - 1 >= 0) { acc -= occ[y * W + x - rb - 1]; n-- }
      if (x >= 0) tmp[y * W + x] = acc / n
    }
  }
  for (let x = 0; x < W; x++) {
    let acc = 0
    let n = 0
    for (let y = -rb; y < H; y++) {
      if (y + rb < H) { acc += tmp[(y + rb) * W + x]; n++ }
      if (y - rb - 1 >= 0) { acc -= tmp[(y - rb - 1) * W + x]; n-- }
      if (y >= 0) occ[y * W + x] = acc / n
    }
  }
  for (let i = 0; i < W * H; i++) {
    if (id[i] === 0 || occ[i] < 0.01) continue
    const k = 1 - 0.55 * occ[i]
    const v = px[i]
    px[i] = ((v & 0xff000000) | (Math.round(((v >>> 16) & 255) * k) << 16) | (Math.round(((v >>> 8) & 255) * k) << 8) | Math.round((v & 255) * k)) >>> 0
  }
}
