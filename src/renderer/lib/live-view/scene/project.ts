// Where a point of the scene lands on a still: K2 Render sends the camera
// that drew each still (`camera {view, proj, fx, fy, near, shift_x,
// shift_y}`: a column-major world-to-view matrix and an OpenGL projection
// with an infinite far plane, crates/k2-daemon/src/docs/preview/extras.rs),
// so the tab lays its boxes and tags over the picture with the same maths.
// Also the compass (the world axes as this camera sees them). Pure; S4's
// boxes and tags use `projectPx`.

/** Sixteen numbers, column-major (m[12..14] is the translation). */
export type Mat4 = readonly number[]

/** The camera that drew a still, as K2 Render sends it. */
export interface StillCamera {
  view: Mat4
  proj: Mat4
  /** Display values K2 Render adds; projection reads `view` and `proj` only. */
  fx?: number
  fy?: number
  near?: number
  shift_x?: number
  shift_y?: number
}

export interface Projected {
  /** Normalised device coordinates: -1 left/bottom, 1 right/top. */
  nx: number
  ny: number
  /** Distance in front of the camera (view-space -z). */
  depth: number
  /** In front of the near plane: only then do nx and ny mean anything. */
  inFront: boolean
}

const NEAR_DEFAULT = 1e-4

/** A world point through the still's camera, to normalised device coordinates. */
export function project(p: readonly number[], cam: StillCamera): Projected {
  const v = cam.view
  const x = v[0] * p[0] + v[4] * p[1] + v[8] * p[2] + v[12]
  const y = v[1] * p[0] + v[5] * p[1] + v[9] * p[2] + v[13]
  const z = v[2] * p[0] + v[6] * p[1] + v[10] * p[2] + v[14]
  const w = v[3] * p[0] + v[7] * p[1] + v[11] * p[2] + v[15]
  const P = cam.proj
  const cx = P[0] * x + P[4] * y + P[8] * z + P[12] * w
  const cy = P[1] * x + P[5] * y + P[9] * z + P[13] * w
  const cw = P[3] * x + P[7] * y + P[11] * z + P[15] * w
  const depth = -z / (w || 1)
  const inFront = cw > (cam.near ?? NEAR_DEFAULT) * 0.5 && depth > 0
  return { nx: inFront ? cx / cw : NaN, ny: inFront ? cy / cw : NaN, depth, inFront }
}

/** Normalised device coordinates to the still's pixels (y down). */
export const toPx = (q: { nx: number; ny: number }, size: { width: number; height: number }): { x: number; y: number } => ({
  x: (q.nx + 1) * 0.5 * size.width,
  y: (1 - q.ny) * 0.5 * size.height,
})

/** A world point to the still's pixels, with its depth. */
export function projectPx(p: readonly number[], cam: StillCamera, size: { width: number; height: number }): { x: number; y: number; depth: number; inFront: boolean } {
  const q = project(p, cam)
  return { ...toPx(q, size), depth: q.depth, inFront: q.inFront }
}

export type Axis = 'x' | 'y' | 'z'
export interface CompassAxis {
  axis: Axis
  /** On screen, from the compass's centre: right and down, -1 to 1. */
  dx: number
  dy: number
  /** How far it points into the picture (1 straight away from you): far axes draw faded, under the near ones. */
  away: number
}

/** The world's axes as this camera sees them, farthest first (the draw order). */
export function compassAxes(cam: StillCamera): CompassAxis[] {
  const v = cam.view
  // The view matrix's rows are the camera's right, up and back in world space.
  const right = [v[0], v[4], v[8]]
  const up = [v[1], v[5], v[9]]
  const back = [v[2], v[6], v[10]]
  const axes: [Axis, number][] = [['x', 0], ['y', 1], ['z', 2]]
  return axes.map(([axis, i]) => ({ axis, dx: right[i], dy: -up[i], away: -back[i] })).sort((a, b) => b.away - a.away)
}

// ── Making a camera: a basis (eye, forward, up hint) and a lens, as K2
// Render frames a view, to the same matrices. The stand-in still and the
// "no picture yet" origin dots use it; a real still brings its own. ──

type V3 = [number, number, number]
const sub = (a: readonly number[], b: readonly number[]): V3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
const dot = (a: readonly number[], b: readonly number[]): number => a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
const cross = (a: readonly number[], b: readonly number[]): V3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
export const norm = (a: readonly number[]): V3 => {
  const l = Math.hypot(a[0], a[1], a[2]) || 1
  return [a[0] / l, a[1] / l, a[2] / l]
}

/** A camera as a basis: eye, forward (into the picture), right, up; `fy` the tangent of half the vertical angle; a lens shift in NDC. */
export interface Basis {
  eye: V3
  f: V3
  r: V3
  u: V3
  fy: number
  aspect: number
  sx: number
  sy: number
}

export function basis(eye: readonly number[], forward: readonly number[], upHint: readonly number[], fy: number, aspect: number): Basis {
  const f = norm(forward)
  const r = norm(cross(f, upHint))
  const u = cross(r, f)
  return { eye: [eye[0], eye[1], eye[2]], f, r, u, fy, aspect, sx: 0, sy: 0 }
}

/** A basis in camera space (x right, y up, z into the picture), for the rasteriser. */
export function toCamera(b: Basis, p: readonly number[]): V3 {
  const d = sub(p, b.eye)
  return [dot(d, b.r), dot(d, b.u), dot(d, b.f)]
}

/** A basis as K2 Render's matrices: world-to-view and an infinite-far OpenGL projection with the shift. */
export function stillCamera(b: Basis, near = 0.05): StillCamera {
  const { r, u, f, eye } = b
  const view = [r[0], u[0], -f[0], 0, r[1], u[1], -f[1], 0, r[2], u[2], -f[2], 0, -dot(r, eye), -dot(u, eye), dot(f, eye), 1]
  const proj = [1 / (b.fy * b.aspect), 0, 0, 0, 0, 1 / b.fy, 0, 0, b.sx, b.sy, -1, -1, 0, 0, -2 * near, 0]
  return { view, proj, near, shift_x: b.sx, shift_y: b.sy }
}

/** The nearest camera along `dir` (from the box's centre outward) that keeps every point within `margin` of the frame (NDC), with a shift that centres them. */
export function fitBasis(pts: readonly (readonly number[])[], dir: readonly number[], fy: number, margin: number, aspect: number): Basis {
  let mn = [Infinity, Infinity, Infinity]
  let mx = [-Infinity, -Infinity, -Infinity]
  for (const p of pts) for (let k = 0; k < 3; k++) {
    mn[k] = Math.min(mn[k], p[k])
    mx[k] = Math.max(mx[k], p[k])
  }
  if (!pts.length) {
    mn = [-1, -1, 0]
    mx = [1, 1, 1]
  }
  const ctr = [(mn[0] + mx[0]) / 2, (mn[1] + mx[1]) / 2, (mn[2] + mx[2]) / 2]
  const rad = Math.max(0.4, Math.hypot(mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]) / 2)
  const cs = pts.length > 1 ? pts : [mn, mx]
  const back = [-dir[0], -dir[1], -dir[2]]
  const ext = (d: number): { b: Basis; k: number; sx: number; sy: number } => {
    const b = basis([ctr[0] + dir[0] * d, ctr[1] + dir[1] * d, ctr[2] + dir[2] * d], back, [0, 0, 1], fy, aspect)
    let x0 = 9, x1 = -9, y0 = 9, y1 = -9
    for (const p of cs) {
      const c = toCamera(b, p)
      if (c[2] <= 0.05) return { b, k: 99, sx: 0, sy: 0 }
      const x = c[0] / (c[2] * fy * aspect)
      const y = c[1] / (c[2] * fy)
      x0 = Math.min(x0, x)
      x1 = Math.max(x1, x)
      y0 = Math.min(y0, y)
      y1 = Math.max(y1, y)
    }
    return { b, k: Math.max((x1 - x0) / 2, (y1 - y0) / 2), sx: (x0 + x1) / 2, sy: (y0 + y1) / 2 }
  }
  let lo = rad * 0.2
  let hi = rad * 40
  for (let it = 0; it < 32; it++) {
    const mid = (lo + hi) / 2
    if (ext(mid).k > margin) lo = mid
    else hi = mid
  }
  const e = ext(hi)
  e.b.sx = e.sx
  e.b.sy = e.sy
  return e.b
}

/** A point through a basis, to NDC (with its shift), and its depth. */
export function basisNdc(b: Basis, p: readonly number[]): { nx: number; ny: number; z: number } {
  const c = toCamera(b, p)
  return { nx: c[0] / (c[2] * b.fy * b.aspect) - b.sx, ny: c[1] / (c[2] * b.fy) - b.sy, z: c[2] }
}

/** The 3/4 view K2 Render's Scene takes, and the turntable's eight directions round it (a little higher, to see over walls). */
export const SCENE_DIR: V3 = norm([1.0, -1.32, 0.66])
export function turnDir(k: number): V3 {
  const a = (k * Math.PI) / 4
  const [x, y] = SCENE_DIR
  return norm([x * Math.cos(a) - y * Math.sin(a), x * Math.sin(a) + y * Math.cos(a), 0.95])
}
