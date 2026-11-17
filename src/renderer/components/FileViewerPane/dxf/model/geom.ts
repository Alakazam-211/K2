// SVG path data from the model's geometry, in drawing coordinates (Y up; the
// view flips the whole drawing once). Arcs are counter-clockwise, so they use
// SVG sweep 1. Ellipses are flattened as the viewer's own reading does (72
// steps a turn), so both paths draw the same curve.

import type { XY } from './types'

export const num = (v: unknown, d = 0): number => (typeof v === 'number' && Number.isFinite(v) ? v : d)
export const fmt = (v: number): string => {
  const r = Math.round(v * 1e4) / 1e4
  return Object.is(r, -0) ? '0' : String(r)
}
export const xy = (v: unknown): XY | null =>
  Array.isArray(v) && v.length >= 2 && typeof v[0] === 'number' && typeof v[1] === 'number' ? [v[0], v[1]] : null

const TAU = 2 * Math.PI

/** A counter-clockwise arc from a0 to a1 (radians); a full turn is drawn as two halves. */
export function arcD(c: XY, r: number, a0: number, a1: number, move = true): string {
  let span = a1 - a0
  while (span <= 0) span += TAU
  while (span > TAU) span -= TAU
  const at = (a: number) => `${fmt(c[0] + r * Math.cos(a))} ${fmt(c[1] + r * Math.sin(a))}`
  const head = move ? `M${at(a0)}` : ''
  if (span > TAU - 1e-9) return `${head} A${fmt(r)} ${fmt(r)} 0 0 1 ${at(a0 + Math.PI)} A${fmt(r)} ${fmt(r)} 0 0 1 ${at(a0)}`
  return `${head} A${fmt(r)} ${fmt(r)} 0 ${span > Math.PI ? 1 : 0} 1 ${at(a0 + span)}`
}

export function ellipsePoints(c: XY, major: XY, ratio: number, p0: number, p1: number): { pts: XY[]; full: boolean } {
  let t1 = p1
  while (t1 <= p0) t1 += TAU
  const full = t1 - p0 >= TAU - 1e-6
  const steps = Math.max(8, Math.ceil((72 * (t1 - p0)) / TAU))
  const pts: XY[] = []
  for (let i = 0; i <= steps; i++) {
    const t = p0 + ((t1 - p0) * i) / steps
    pts.push([c[0] + major[0] * Math.cos(t) - ratio * major[1] * Math.sin(t), c[1] + major[1] * Math.cos(t) + ratio * major[0] * Math.sin(t)])
  }
  return { pts, full }
}

export function polyD(pts: readonly XY[], closed: boolean): string {
  if (pts.length < 2) return ''
  const d = pts.map((p, i) => `${i ? 'L' : 'M'}${fmt(p[0])} ${fmt(p[1])}`).join(' ')
  return closed ? `${d} Z` : d
}

/** A bulge segment's SVG arc (the bulge is tan of a quarter of the included angle; positive is counter-clockwise). */
function bulgeArc(a: XY, b: XY, bulge: number): string {
  const chord = Math.hypot(b[0] - a[0], b[1] - a[1])
  if (!bulge || chord === 0) return ` L${fmt(b[0])} ${fmt(b[1])}`
  const theta = 4 * Math.atan(Math.abs(bulge))
  const r = chord / (2 * Math.sin(theta / 2))
  return ` A${fmt(r)} ${fmt(r)} 0 ${theta > Math.PI ? 1 : 0} ${bulge > 0 ? 1 : 0} ${fmt(b[0])} ${fmt(b[1])}`
}

export type Vert = { p: XY; bulge?: number }

export function bulgeD(verts: readonly Vert[], closed: boolean): string {
  if (verts.length < 2) return ''
  let d = `M${fmt(verts[0].p[0])} ${fmt(verts[0].p[1])}`
  const n = closed ? verts.length : verts.length - 1
  for (let i = 0; i < n; i++) d += bulgeArc(verts[i].p, verts[(i + 1) % verts.length].p, num(verts[i].bulge))
  return closed ? `${d} Z` : d
}

/** The model's vertex list (`{p, bulge}` rows), read defensively. */
export function vertsOf(v: unknown): Vert[] {
  if (!Array.isArray(v)) return []
  const out: Vert[] = []
  for (const r of v) {
    const p = xy((r as { p?: unknown })?.p)
    if (p) out.push({ p, bulge: num((r as { bulge?: unknown }).bulge) })
  }
  return out
}

export const pointsOf = (v: unknown): XY[] => (Array.isArray(v) ? v.map(xy).filter((p): p is XY => p !== null) : [])
