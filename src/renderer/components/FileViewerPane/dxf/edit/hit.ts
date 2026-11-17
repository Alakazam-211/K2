// How far a point is from an entity as drawn, in drawing units: its lines
// and curves for lines, circles, arcs, ellipses, polylines (bulges as short
// chords), splines and points; anywhere on a text, a block or a filled
// shape counts as on it (inside its box), though a line or curve under the
// pointer wins over it; anything else by its box's outline.

import { ellipsePoints, num, pointsOf, vertsOf, xy } from '../model/geom'
import type { ModelRow, XY } from '../model/types'

const TAU = 2 * Math.PI

function segment(p: XY, a: XY, b: XY): number {
  const vx = b[0] - a[0]
  const vy = b[1] - a[1]
  const len2 = vx * vx + vy * vy
  const t = len2 ? Math.max(0, Math.min(1, ((p[0] - a[0]) * vx + (p[1] - a[1]) * vy) / len2)) : 0
  return Math.hypot(p[0] - (a[0] + t * vx), p[1] - (a[1] + t * vy))
}

function path(p: XY, pts: readonly XY[], closed: boolean): number {
  if (pts.length === 1) return Math.hypot(p[0] - pts[0][0], p[1] - pts[0][1])
  let d = Infinity
  for (let i = 1; i < pts.length; i++) d = Math.min(d, segment(p, pts[i - 1], pts[i]))
  if (closed && pts.length > 2) d = Math.min(d, segment(p, pts[pts.length - 1], pts[0]))
  return d
}

/** A bulge segment as a few points along its arc. */
function bulgePoints(a: XY, b: XY, bulge: number): XY[] {
  if (!bulge) return [a, b]
  const theta = 4 * Math.atan(bulge)
  const chord = Math.hypot(b[0] - a[0], b[1] - a[1])
  if (!chord) return [a, b]
  const r = chord / (2 * Math.sin(Math.abs(theta) / 2))
  const mx = (a[0] + b[0]) / 2
  const my = (a[1] + b[1]) / 2
  const h = Math.sqrt(Math.max(r * r - (chord / 2) ** 2, 0)) * (Math.abs(theta) > Math.PI ? -1 : 1) * Math.sign(bulge)
  const c: XY = [mx - (h * (b[1] - a[1])) / chord, my + (h * (b[0] - a[0])) / chord]
  const a0 = Math.atan2(a[1] - c[1], a[0] - c[0])
  const out: XY[] = []
  for (let i = 0; i <= 8; i++) {
    const t = a0 + (theta * i) / 8
    out.push([c[0] + r * Math.cos(t), c[1] + r * Math.sin(t)])
  }
  return out
}

function boxEdge(p: XY, q: [number, number, number, number], solid: boolean): number {
  const dx = Math.max(q[0] - p[0], 0, p[0] - q[2])
  const dy = Math.max(q[1] - p[1], 0, p[1] - q[3])
  const out = Math.hypot(dx, dy)
  if (out > 0 || solid) return out
  return Math.min(p[0] - q[0], q[2] - p[0], p[1] - q[1], q[3] - p[1])
}

/** Picked anywhere on their box: a line or curve under the pointer wins over these. */
export const SOLID = new Set(['TEXT', 'MTEXT', 'INSERT', 'HATCH', 'SOLID', 'TRACE', '3DFACE', 'IMAGE', 'WIPEOUT', 'DIMENSION', 'MULTILEADER', 'MLEADER'])

export function hitDistance(r: ModelRow, p: XY): number {
  const g = r.geom ?? {}
  const pts = pointsOf(g.path)
  switch (r.kind) {
    case 'LINE': {
      const a = xy(g.p1)
      const b = xy(g.p2)
      if (a && b) return segment(p, a, b)
      break
    }
    case 'POINT': {
      const a = xy(g.p)
      if (a) return Math.hypot(p[0] - a[0], p[1] - a[1])
      break
    }
    case 'CIRCLE':
    case 'ARC': {
      const c = xy(g.centre)
      const rad = num(g.r, NaN)
      if (!c || !Number.isFinite(rad)) break
      const d = Math.hypot(p[0] - c[0], p[1] - c[1])
      if (r.kind === 'CIRCLE') return Math.abs(d - rad)
      const a0 = num(g.a0)
      let span = num(g.a1) - a0
      while (span <= 0) span += TAU
      let at = Math.atan2(p[1] - c[1], p[0] - c[0]) - a0
      while (at < 0) at += TAU
      if (at <= span) return Math.abs(d - rad)
      const end = (a: number): number => Math.hypot(p[0] - (c[0] + rad * Math.cos(a)), p[1] - (c[1] + rad * Math.sin(a)))
      return Math.min(end(a0), end(a0 + span))
    }
    case 'ELLIPSE': {
      const c = xy(g.centre)
      const major = xy(g.major)
      if (!c || !major) break
      const e = ellipsePoints(c, major, num(g.ratio, 1), num(g.p0), num(g.p1, TAU))
      return path(p, e.pts, e.full)
    }
    case 'LWPOLYLINE':
    case 'POLYLINE': {
      const v = vertsOf(g.verts)
      if (v.length < 2) break
      const closed = g.closed === true
      const all: XY[] = []
      const n = closed ? v.length : v.length - 1
      for (let i = 0; i < n; i++) all.push(...bulgePoints(v[i].p, v[(i + 1) % v.length].p, num(v[i].bulge)))
      return path(p, all, false)
    }
  }
  if (pts.length >= 2 && !SOLID.has(r.kind)) return path(p, pts, g.closed === true)
  return r.bbox ? boxEdge(p, r.bbox, SOLID.has(r.kind)) : Infinity
}
