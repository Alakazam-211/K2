// SPLINE → polyline. Rational splines (group 41 weights, flag 4) are
// evaluated as NURBS, in homogeneous coordinates; without weights, or with
// all weights equal, that is the plain B-spline. Flattened as K2DRAW1 §4
// asks: each chord within 2.5e-4 of the spline's box diagonal, at most
// 4,096 points. With no usable knots, the fit points (else the control
// points) are joined.

import type { ISplineEntity } from 'dxf-parser'
import type { EntityExtras } from './parse-extras'

type Pt = [number, number]

const MAX_POINTS = 4096
const TOLERANCE = 2.5e-4

const num = (v: unknown, d = 0): number => (typeof v === 'number' && Number.isFinite(v) ? v : d)

/** de Boor at `t` on (x·w, y·w, w), then back to the plane. */
function evaluate(t: number, p: number, knots: number[], cps: Pt[], w: number[]): Pt {
  let k = p
  while (k < cps.length - 1 && knots[k + 1] <= t) k++
  const d = Array.from({ length: p + 1 }, (_, j) => {
    const i = j + k - p
    return [cps[i][0] * w[i], cps[i][1] * w[i], w[i]]
  })
  for (let r = 1; r <= p; r++) {
    for (let j = p; j >= r; j--) {
      const i = j + k - p
      const den = knots[i + p - r + 1] - knots[i]
      const a = den === 0 ? 0 : (t - knots[i]) / den
      d[j] = [0, 1, 2].map((c) => (1 - a) * d[j - 1][c] + a * d[j][c])
    }
  }
  const [x, y, h] = d[p]
  return h === 0 ? [x, y] : [x / h, y / h]
}

/** Weights to use: one positive finite weight per control point, else all 1. */
function weightsOf(e: ISplineEntity & EntityExtras, count: number): number[] {
  const w = e.weights
  if (w && w.length === count && w.every((v) => Number.isFinite(v) && v > 0)) return w
  return new Array<number>(count).fill(1)
}

export function sampleSpline(e: ISplineEntity & EntityExtras): Pt[] {
  const cps: Pt[] = (e.controlPoints ?? []).map((q) => [num(q.x), num(q.y)])
  const p = num(e.degreeOfSplineCurve, 3)
  const knots = e.knotValues ?? []
  if (cps.length >= 2 && p >= 1 && knots.length === cps.length + p + 1) {
    const lo = knots[p]
    const hi = knots[cps.length]
    if (hi > lo) {
      const w = weightsOf(e, cps.length)
      const at = (t: number): Pt => evaluate(t, p, knots, cps, w)
      const steps = Math.min(2000, Math.max(32, cps.length * 10))
      const ts = Array.from({ length: steps + 1 }, (_, s) => (s === steps ? hi : lo + ((hi - lo) * s) / steps))
      const pts = ts.map(at)
      return refine(ts, pts, at)
    }
  }
  const pts = e.fitPoints && e.fitPoints.length >= 2 ? e.fitPoints : (e.controlPoints ?? [])
  return pts.map((q) => [num(q.x), num(q.y)])
}

/** Split any span whose midpoint strays from its chord by more than the tolerance. */
function refine(ts: number[], pts: Pt[], at: (t: number) => Pt): Pt[] {
  let minX = Infinity
  let minY = Infinity
  let maxX = -Infinity
  let maxY = -Infinity
  for (const [x, y] of pts) {
    minX = Math.min(minX, x)
    minY = Math.min(minY, y)
    maxX = Math.max(maxX, x)
    maxY = Math.max(maxY, y)
  }
  const tol = TOLERANCE * Math.hypot(maxX - minX, maxY - minY)
  if (!(tol > 0)) return pts
  const out: Pt[] = [pts[0]]
  let budget = MAX_POINTS - pts.length
  const span = (t0: number, a: Pt, t1: number, b: Pt, depth: number): void => {
    const tm = (t0 + t1) / 2
    const m = at(tm)
    const off = Math.hypot(m[0] - (a[0] + b[0]) / 2, m[1] - (a[1] + b[1]) / 2)
    if (off > tol && depth < 12 && budget > 0) {
      budget--
      span(t0, a, tm, m, depth + 1)
      span(tm, m, t1, b, depth + 1)
    } else {
      out.push(b)
    }
  }
  for (let i = 1; i < pts.length; i++) span(ts[i - 1], pts[i - 1], ts[i], pts[i], 0)
  return out
}
