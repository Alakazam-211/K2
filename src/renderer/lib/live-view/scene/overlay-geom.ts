// The geometry under the overlay: an object's box (its local bounds'
// eight corners through its world matrix, or its origin when it has no
// bounds), projected through the still's own camera to the still's pixels;
// the slide between two places; where a selection's label hangs (the box's
// lowest visible bottom edge: Sam's floor is labelled on its front edge,
// never on a wall); which edge something off the picture lies past; and
// the display-only direction arrows (an arrow for a move, an arc for a
// turn, outward arrows for a resize). Pure; in the still's pixels.

import { worldOf } from './fold'
import { xformParts, same } from './math'
import { projectPx, type StillCamera } from './project'
import type { ObjectBounds } from './picture-source'
import type { Rect, Side } from './labels'
import type { Doc } from './types'

export type BoundsOf = (D: Doc, a: string) => ObjectBounds | null

/** The still the overlay is drawn over: its camera and its size in pixels. */
export interface StillView {
  camera: StillCamera
  width: number
  height: number
}

type V3 = number[]
/** World points: a box's eight corners (bit 0 x, bit 1 y, bit 2 z: 0–3 are the bottom face), or an origin. */
export type WorldBox = { corners: V3[] } | { origin: V3 }

const xf = (m: ArrayLike<number>, p: ArrayLike<number>): V3 => [m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12], m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13], m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14]]
const corners = (b: { min: number[]; max: number[] }): V3[] => [0, 1, 2, 3, 4, 5, 6, 7].map((i) => [i & 1 ? b.max[0] : b.min[0], i & 2 ? b.max[1] : b.min[1], i & 4 ? b.max[2] : b.min[2]])

/** An object's box in the world at a doc, or null when it has no place there. */
export function worldBox(D: Doc, a: string, bounds: BoundsOf): WorldBox | null {
  if (!D.units[a]) return null
  const w = worldOf(D, a)
  if (!w) return null
  const b = bounds(D, a)
  return b ? { corners: corners(b).map((p) => xf(w, p)) } : { origin: [w[12], w[13], w[14]] }
}

const lerp3 = (a: V3, b: V3, t: number): V3 => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
/** Part way from one place to the next (the slide). */
export function lerpBox(from: WorldBox | null, to: WorldBox, t: number): WorldBox {
  if (!from || t >= 1) return to
  if ('corners' in to && 'corners' in from) return { corners: to.corners.map((c, i) => lerp3(from.corners[i], c, t)) }
  if ('origin' in to && 'origin' in from) return { origin: lerp3(from.origin, to.origin, t) }
  return to
}

export const centreOf = (b: WorldBox): V3 => ('origin' in b ? b.origin : [0, 1, 2].map((k) => b.corners.reduce((s, q) => s + q[k], 0) / 8))

export interface Geom {
  /** The SVG path: the box's twelve edges, or a ring and cross for an origin. */
  d: string
  rect: Rect
  /** The projected corners (empty for an origin). */
  pts: { x: number; y: number }[]
  origin: { x: number; y: number } | null
}

const EDGES = [[0, 1], [2, 3], [4, 5], [6, 7], [0, 2], [1, 3], [4, 6], [5, 7], [0, 4], [1, 5], [2, 6], [3, 7]]
const n1 = (v: number): string => v.toFixed(1)

/** A box through the still's camera; null when any of it is behind the camera. */
export function geomOf(b: WorldBox, sv: StillView): Geom | null {
  const size = { width: sv.width, height: sv.height }
  const k = sv.width / 1920
  if ('origin' in b) {
    const q = projectPx(b.origin, sv.camera, size)
    if (!q.inFront) return null
    const r = 11 * k
    const c = 18 * k
    return { d: `M${n1(q.x - r)} ${n1(q.y)}a${n1(r)} ${n1(r)} 0 1 0 ${n1(2 * r)} 0a${n1(r)} ${n1(r)} 0 1 0 ${n1(-2 * r)} 0M${n1(q.x - c)} ${n1(q.y)}h${n1(2 * c)}M${n1(q.x)} ${n1(q.y - c)}v${n1(2 * c)}`, rect: { x0: q.x - 14 * k, y0: q.y - 14 * k, x1: q.x + 14 * k, y1: q.y + 14 * k }, pts: [], origin: { x: q.x, y: q.y } }
  }
  const P = b.corners.map((c) => projectPx(c, sv.camera, size))
  if (P.some((p) => !p.inFront)) return null
  // Each edge clipped to a band one still wide round the picture: a corner close to the camera
  // lands tens of thousands of pixels out, and a dashed stroke that long stalls the browser.
  const band = bandOf(sv)
  const segs = EDGES.map(([i, j]) => clipSeg(P[i], P[j], band)).filter((s): s is Seg => !!s)
  if (!segs.length) return null
  const xs = segs.flatMap((s) => [s[0].x, s[1].x])
  const ys = segs.flatMap((s) => [s[0].y, s[1].y])
  return { d: segs.map(([p, q]) => `M${n1(p.x)} ${n1(p.y)}L${n1(q.x)} ${n1(q.y)}`).join(''), rect: { x0: Math.min(...xs), y0: Math.min(...ys), x1: Math.max(...xs), y1: Math.max(...ys) }, pts: P.map((p) => ({ x: p.x, y: p.y })), origin: null }
}

type Pt = { x: number; y: number }
type Seg = [Pt, Pt]
/** One still's width and height beyond each edge of the picture. */
const bandOf = (sv: StillView): Rect => ({ x0: -sv.width, y0: -sv.height, x1: 2 * sv.width, y1: 2 * sv.height })
const inBand = (p: Pt, r: Rect): boolean => p.x >= r.x0 && p.x <= r.x1 && p.y >= r.y0 && p.y <= r.y1

/** A segment clipped to a rectangle (Liang–Barsky), or null when none of it is inside. */
export function clipSeg(a: Pt, b: Pt, r: Rect): Seg | null {
  const dx = b.x - a.x
  const dy = b.y - a.y
  let t0 = 0
  let t1 = 1
  for (const [p, q] of [[-dx, a.x - r.x0], [dx, r.x1 - a.x], [-dy, a.y - r.y0], [dy, r.y1 - a.y]]) {
    if (p === 0) {
      if (q < 0) return null
      continue
    }
    const t = q / p
    if (p < 0) t0 = Math.max(t0, t)
    else t1 = Math.min(t1, t)
    if (t0 > t1) return null
  }
  return [{ x: a.x + t0 * dx, y: a.y + t0 * dy }, { x: a.x + t1 * dx, y: a.y + t1 * dy }]
}

/** Where a selection's label hangs: the lowest visible point of the box's bottom face (its corners and edge midpoints). */
export function edgeAnchor(g: Geom, sv: StillView): { x: number; y: number } {
  const k = sv.width / 1920
  const W = sv.width
  const H = sv.height
  if (g.origin) return { x: g.origin.x + 16 * k, y: g.origin.y + 10 * k }
  const c = g.pts.slice(0, 4)
  const mids = [[0, 1], [1, 3], [3, 2], [2, 0]].map(([i, j]) => ({ x: (c[i].x + c[j].x) / 2, y: (c[i].y + c[j].y) / 2 }))
  const inside = c.concat(mids).filter((p) => p.x > 80 * k && p.x < W - 240 * k && p.y > 80 * k && p.y < H - 120 * k).sort((a, b) => b.y - a.y)
  if (inside[0]) return { x: inside[0].x, y: inside[0].y + 12 * k }
  return { x: Math.max(160 * k, Math.min(W - 240 * k, (g.rect.x0 + g.rect.x1) / 2)), y: Math.min(g.rect.y1, H - 130 * k) }
}

/** Which edge of the picture something lies past (null when its centre is on the picture), and the point on that edge nearest it. */
export function offPicture(b: WorldBox, sv: StillView): { side: Side; at: { x: number; y: number } } | null {
  const W = sv.width
  const H = sv.height
  const c = centreOf(b)
  const q = projectPx(c, sv.camera, { width: W, height: H })
  if (q.inFront && q.x >= 0 && q.x <= W && q.y >= 0 && q.y <= H) return null
  let dx: number
  let dy: number
  if (q.inFront) {
    dx = (q.x - W / 2) / (W / 2)
    dy = (q.y - H / 2) / (H / 2)
  } else {
    // Behind the camera: which way it lies, in the camera's own right and up.
    const v = sv.camera.view
    dx = v[0] * c[0] + v[4] * c[1] + v[8] * c[2] + v[12]
    dy = -(v[1] * c[0] + v[5] * c[1] + v[9] * c[2] + v[13])
  }
  const clampX = (x: number): number => (Number.isFinite(x) && q.inFront ? Math.max(0, Math.min(W, x)) : W / 2)
  const clampY = (y: number): number => (Number.isFinite(y) && q.inFront ? Math.max(0, Math.min(H, y)) : H / 2)
  if (Math.abs(dx) >= Math.abs(dy)) return dx < 0 ? { side: 'left', at: { x: 0, y: clampY(q.y) } } : { side: 'right', at: { x: W, y: clampY(q.y) } }
  return dy < 0 ? { side: 'top', at: { x: clampX(q.x), y: 0 } } : { side: 'bottom', at: { x: clampX(q.x), y: H } }
}

export type ArrowKind = 'move' | 'turn' | 'scale'

/** A line (or polyline) ending in an open arrowhead, in still pixels. */
function arrowPath(pts: { x: number; y: number }[], head: number): string {
  const a = pts[pts.length - 2]
  const b = pts[pts.length - 1]
  const ang = Math.atan2(b.y - a.y, b.x - a.x)
  const h1 = { x: b.x - head * Math.cos(ang - 0.5), y: b.y - head * Math.sin(ang - 0.5) }
  const h2 = { x: b.x - head * Math.cos(ang + 0.5), y: b.y - head * Math.sin(ang + 0.5) }
  return `M${pts.map((p) => `${n1(p.x)} ${n1(p.y)}`).join('L')}M${n1(h1.x)} ${n1(h1.y)}L${n1(b.x)} ${n1(b.y)}L${n1(h2.x)} ${n1(h2.y)}`
}

/** The direction arrows for one object's change from `D0` to `D1` (its xform before and after). */
export function arrowsFor(a: string, xform: { before: string | null | undefined; after: string | null | undefined }, D0: Doc, D1: Doc, bounds: BoundsOf, sv: StillView): { kind: ArrowKind; d: string }[] {
  const pa = xformParts(xform.before)
  const pb = xformParts(xform.after)
  const b0 = worldBox(D0, a, bounds)
  const b1 = worldBox(D1, a, bounds)
  if (!pa || !pb || !b0 || !b1) return []
  const k = sv.width / 1920
  const head = 18 * k
  const size = { width: sv.width, height: sv.height }
  const band = bandOf(sv)
  // An arrow whose end lies far off the picture isn't drawn (see geomOf).
  const at = (p: V3): { x: number; y: number } | null => {
    const q = projectPx(p, sv.camera, size)
    return q.inFront && inBand(q, band) ? { x: q.x, y: q.y } : null
  }
  const out: { kind: ArrowKind; d: string }[] = []
  const line = (p: V3, q: V3, kind: ArrowKind): void => {
    const P = at(p)
    const Q = at(q)
    if (!P || !Q || Math.hypot(Q.x - P.x, Q.y - P.y) < 14 * k) return
    out.push({ kind, d: arrowPath([{ x: P.x + (Q.x - P.x) * 0.12, y: P.y + (Q.y - P.y) * 0.12 }, Q], head) })
  }
  if (!same(pa.loc, pb.loc)) line(centreOf(b0), centreOf(b1), 'move')
  if (!same(pa.rot, pb.rot)) {
    const q = centreOf(b1)
    const cs = 'corners' in b1 ? b1.corners : null
    const R = (cs ? Math.max(...cs.map((v) => Math.hypot(v[0] - q[0], v[1] - q[1]))) : 0.4) * 0.95
    const top = cs ? Math.max(...cs.map((v) => v[2])) + 0.04 : q[2]
    const span = pb.rot[2] - pa.rot[2]
    const sw = Math.sign(span || 1) * Math.max(Math.abs(span), 0.9)
    const f0 = -Math.PI / 2 - sw / 2
    const pts: { x: number; y: number }[] = []
    for (let i = 0; i <= 16; i++) {
      const th = f0 + (sw * i) / 16
      const P = at([q[0] + R * Math.cos(th), q[1] + R * Math.sin(th), top])
      if (!P) break
      pts.push(P)
    }
    if (pts.length === 17) out.push({ kind: 'turn', d: arrowPath(pts, head) })
  }
  if (!same(pa.scale, pb.scale)) {
    const w0 = worldOf(D0, a)
    const w1 = worldOf(D1, a)
    const bb = bounds(D1, a)
    if (w0 && w1 && bb)
      for (const ax of [0, 1])
        if (pa.scale[ax] !== pb.scale[ax])
          for (const sgn of [1, -1]) {
            const lp = [0, 1, 2].map((j) => (j === ax ? (sgn > 0 ? bb.max[j] : bb.min[j]) : (bb.min[j] + bb.max[j]) / 2))
            line(xf(w0, lp), xf(w1, lp), 'scale')
          }
  }
  return out
}
