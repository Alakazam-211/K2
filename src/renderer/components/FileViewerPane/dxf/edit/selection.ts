// Selecting in a drawing, in drawing coordinates (y up):
//   - only what is drawn and shown in model space can be picked;
//   - a click picks the entity nearest the pointer as drawn, within a few pixels;
//   - a box dragged left to right is a window (what is wholly inside), right
//     to left a crossing (what it touches), as CAD apps do; both read the
//     model's boxes;
//   - Shift locks a move to its longer axis.
// And which entities the edit verbs change: the model's own reason when it
// gives one, else the CAD pack's rule by kind, then an external reference,
// a locked layer, or inside a block.

import type { ModelHeader, ModelRow, XY } from '../model/types'
import { hitDistance, SOLID } from './hit'

/** What can be selected: drawn and shown, in model space. */
export const pickable = (rows: readonly ModelRow[]): ModelRow[] => rows.filter((r) => r.owner === 'model' && r.drawn && r.visible === true)

export const boxMode = (a: XY, b: XY): 'window' | 'crossing' => (b[0] >= a[0] ? 'window' : 'crossing')

/** The handles a box from `a` to `b` selects. */
export function boxSelect(rows: readonly ModelRow[], a: XY, b: XY): string[] {
  const x0 = Math.min(a[0], b[0])
  const x1 = Math.max(a[0], b[0])
  const y0 = Math.min(a[1], b[1])
  const y1 = Math.max(a[1], b[1])
  const window = boxMode(a, b) === 'window'
  return rows
    .filter(({ bbox: q }) => {
      if (!q) return false
      return window ? q[0] >= x0 && q[2] <= x1 && q[1] >= y0 && q[3] <= y1 : q[0] <= x1 && q[2] >= x0 && q[1] <= y1 && q[3] >= y0
    })
    .map((r) => r.h)
}

/** The entity a click at `p` picks: a line or curve before a text, block or fill under it, then the nearest as drawn within `tol`, then the smaller box, then the one on top. */
export function pickAt(rows: readonly ModelRow[], p: XY, tol: number): string | null {
  let best: { h: string; loose: boolean; d: number; area: number; z: number } | null = null
  for (const r of rows) {
    // Far outside its box: not near it as drawn either.
    const q = r.bbox
    if (q && (p[0] < q[0] - tol || p[0] > q[2] + tol || p[1] < q[1] - tol || p[1] > q[3] + tol)) continue
    const d = hitDistance(r, p)
    if (!(d <= tol)) continue
    const area = q ? (q[2] - q[0]) * (q[3] - q[1]) : Infinity
    const c = { h: r.h, loose: SOLID.has(r.kind), d, area, z: r.z }
    const better =
      !best ||
      (best.loose && !c.loose) ||
      (best.loose === c.loose && (c.d < best.d || (c.d === best.d && (c.area < best.area || (c.area === best.area && c.z > best.z)))))
    if (better) best = c
  }
  return best?.h ?? null
}

export const toggle = (sel: readonly string[], h: string): string[] => (sel.includes(h) ? sel.filter((x) => x !== h) : [...sel, h])

/** Shift: keep the longer axis of a move. */
export const shiftLock = (dx: number, dy: number): [number, number] => (Math.abs(dx) >= Math.abs(dy) ? [dx, 0] : [0, dy])

/** A dragged offset rounded to the finest step a screen pixel shows (`unitsPerPx`), so a drag never sends 10.000000001. */
export function snapDelta(v: number, unitsPerPx: number): number {
  const step = 10 ** Math.floor(Math.log10(Math.max(unitsPerPx, 1e-12)))
  const out = Math.round(v / step) * step
  const digits = Math.max(0, -Math.floor(Math.log10(step)))
  const r = Number(out.toFixed(digits))
  return r === 0 ? 0 : r
}

// The CAD pack's rule by kind, for a model that doesn't give its own word.
const EDITABLE = new Set(['LINE', 'POINT', 'CIRCLE', 'ARC', 'LWPOLYLINE', 'POLYLINE', 'TEXT', 'MTEXT', 'INSERT', 'ELLIPSE', 'SPLINE', 'SOLID', 'TRACE', '3DFACE', 'ATTDEF'])
const BY_KIND: Record<string, string> = {
  HATCH: 'hatch',
  DIMENSION: 'dimension',
  ARC_DIMENSION: 'dimension',
  LEADER: 'leader',
  MLEADER: 'leader',
  MULTILEADER: 'leader',
  VIEWPORT: 'viewport',
  ACAD_PROXY_ENTITY: 'proxy',
  '3DSOLID': 'acis',
  REGION: 'acis',
  BODY: 'acis',
  SURFACE: 'acis',
  IMAGE: 'image',
  WIPEOUT: 'image',
  UNDERLAY: 'image',
  PDFUNDERLAY: 'image',
  DWFUNDERLAY: 'image',
  DGNUNDERLAY: 'image',
}

/** Why the edit verbs won't change this entity (a reason word for `whyWords`), or null when they will. */
export function editReason(r: ModelRow, header: Pick<ModelHeader, 'layers'>): string | null {
  // Inside a block definition. (A layout's entities are on paper, which the viewer doesn't show or select yet.)
  if (r.owner !== 'model') return 'block'
  const word = typeof r.edit === 'string' && r.edit && r.edit !== '1' && r.edit !== 'dynblock' ? r.edit : null
  if (word) return word
  if (!EDITABLE.has(r.kind)) return BY_KIND[r.kind] ?? 'unknown'
  if (r.kind === 'INSERT' && r.geom?.xref === true) return 'xref'
  if (header.layers.find((l) => l.name === r.layer)?.locked) return 'locked'
  return null
}
