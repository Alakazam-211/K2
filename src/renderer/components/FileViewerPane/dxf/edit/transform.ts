// Turning and scaling a selection, in drawing coordinates (y up, angles
// anticlockwise):
//   - both go about the selection's centre: the middle of the box around
//     everything selected;
//   - the box has a handle above its top edge to turn it and one on each
//     corner to scale it, a fixed number of screen pixels out from the box;
//   - dragging the turn handle turns by the angle the pointer sweeps about
//     the centre (Shift snaps to 15°); dragging a corner scales by how far
//     the pointer is from the centre against where the drag began, the same
//     in both directions (Shift snaps to 0.25 steps).

import type { Box4, ModelRow, XY } from '../model/types'

/** The box around every row given, or null when none has one. */
export function selectionBox(rows: readonly Pick<ModelRow, 'bbox'>[]): Box4 | null {
  let out: Box4 | null = null
  for (const { bbox: b } of rows) {
    if (!b) continue
    out = out ? [Math.min(out[0], b[0]), Math.min(out[1], b[1]), Math.max(out[2], b[2]), Math.max(out[3], b[3])] : [...b]
  }
  return out
}

export const centreOf = (b: Box4): XY => [(b[0] + b[2]) / 2, (b[1] + b[3]) / 2]

/** An angle in degrees as the verb takes it: 0 up to 360, without float noise. */
export function normDeg(d: number): number {
  const r = Number((((d % 360) + 360) % 360).toFixed(9))
  return r === 360 || r === 0 ? 0 : r
}

const deg = (v: XY, c: XY): number => (Math.atan2(v[1] - c[1], v[0] - c[0]) * 180) / Math.PI

/** The turn (degrees, anticlockwise, -180 to 180) a drag from `a` to `b` sweeps about `c`; Shift snaps to 15°, else to a tenth of a degree. */
export function turnDeg(c: XY, a: XY, b: XY, shift: boolean): number {
  let d = deg(b, c) - deg(a, c)
  while (d > 180) d -= 360
  while (d <= -180) d += 360
  const step = shift ? 15 : 0.1
  const r = Number((Math.round(d / step) * step).toFixed(1))
  return r === 0 || r === -180 ? Math.abs(r) : r
}

/** The factor a drag from `a` to `b` scales by about `c`; Shift snaps to 0.25 steps, else to hundredths; never 0. */
export function sizeFactor(c: XY, a: XY, b: XY, shift: boolean): number {
  const from = Math.hypot(a[0] - c[0], a[1] - c[1])
  const f = from ? Math.hypot(b[0] - c[0], b[1] - c[1]) / from : 1
  const step = shift ? 0.25 : 0.01
  return Math.max(step, Number((Math.round(f / step) * step).toFixed(2)))
}

/** `p` turned by `d` degrees (anticlockwise) about `c`. */
export function turnPoint(p: XY, c: XY, d: number): XY {
  const t = (d * Math.PI) / 180
  const [x, y] = [p[0] - c[0], p[1] - c[1]]
  return [c[0] + x * Math.cos(t) - y * Math.sin(t), c[1] + x * Math.sin(t) + y * Math.cos(t)]
}

/** `p` scaled by `f` about `c`. */
export const sizePoint = (p: XY, c: XY, f: number): XY => [c[0] + (p[0] - c[0]) * f, c[1] + (p[1] - c[1]) * f]

/** How far (screen pixels) the box's outline sits outside the selection, the turn handle above it, and how near a press must be. */
export const PAD_PX = 5
export const TURN_PX = 24
export const GRAB_PX = 8

export type Handles = { turn: XY; corners: [XY, XY, XY, XY] }

/** Where the handles sit for a selection box, `k` drawing units to a screen pixel. */
export function handlePoints(b: Box4, k: number): Handles {
  const p = PAD_PX * k
  const [x0, y0, x1, y1] = [b[0] - p, b[1] - p, b[2] + p, b[3] + p]
  return { turn: [(x0 + x1) / 2, y1 + TURN_PX * k], corners: [[x0, y0], [x1, y0], [x1, y1], [x0, y1]] }
}

export type HandleHit = { kind: 'turn'; at: XY } | { kind: 'size'; corner: number; at: XY }

/** The handle a press at `p` grabs, the turn handle first, or null. */
export function handleAt(b: Box4, p: XY, k: number): HandleHit | null {
  const h = handlePoints(b, k)
  const near = (q: XY) => Math.hypot(p[0] - q[0], p[1] - q[1]) <= GRAB_PX * k
  if (near(h.turn)) return { kind: 'turn', at: h.turn }
  const i = h.corners.findIndex(near)
  return i >= 0 ? { kind: 'size', corner: i, at: h.corners[i] } : null
}
