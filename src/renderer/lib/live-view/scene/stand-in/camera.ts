// SAMPLE ONLY. The stand-in still's camera per view, the way K2 Render
// frames them (ported from the mock's still.js): Scene frames the contents
// (the furniture, not the room shell) at a 3/4 angle with a lens shift, and
// the whole room when framing the furniture would cut its corners; a file
// camera keeps its own place and lens; a framed object fits it and its
// children; the turntable circles the scene in eight steps. A real still
// brings its own camera.

import { objOf, worldOf } from '../fold'
import { SCENE_DIR, basis, basisNdc, fitBasis, turnDir, type Basis } from '../project'
import type { Doc } from '../types'
import { parseView } from '../view-address'
import { objectOfView } from '../views'
import { corners, drawable, unitsOf, worldPoints } from './shapes'

/** Each file camera's lens (mm, 36 mm sensor): sample values; K2 Render sends `lens_mm` with the listing. */
const SAMPLE_LENS: Record<string, number> = { '@ca:Camera': 26, '@ca:Camera.001': 35 }

/** Leave the room shell out of Scene's framing: objects that cover a big share of the floor, or are flat. */
function contentsOf(D: Doc, list: string[]): string[] {
  const boxes = list.map((a) => ({ a, b: worldPoints(D, [a], false) })).filter((x) => x.b)
  const all = worldPoints(D, list, false)
  if (!all || boxes.length < 4) return list
  const area = (b: { min: number[]; max: number[] }): number => Math.max(1e-6, (b.max[0] - b.min[0]) * (b.max[1] - b.min[1]))
  const A = area(all)
  const keep = boxes
    .filter(({ b }) => b && area(b) < 0.25 * A && b.max[2] - b.min[2] > 0.03 && b.max[0] - b.min[0] < 0.6 * (all.max[0] - all.min[0]) && b.max[1] - b.min[1] < 0.6 * (all.max[1] - all.min[1]))
    .map((x) => x.a)
  return keep.length >= 3 ? keep : list
}

/** The stand-in camera for a view (K2 Render's address: `@scene:main`, `@cam:<id>`, `@obj:<id>`, `@turntable:<n>`) at an aspect. */
export function standInCamera(D: Doc, view: string, aspect: number): Basis {
  const all = unitsOf(D)
  const mesh = all.filter((a) => drawable(D, a))
  const p = parseView(view)
  const at = p && (p.kind === 'camera' || p.kind === 'object') ? objectOfView(D, view) : null
  if (p?.kind === 'camera' && at) {
    const w = worldOf(D, at)
    const lens = SAMPLE_LENS[(objOf(D, at).data || '').replace('camera ', '')] || 50
    if (w) return basis([w[12], w[13], w[14]], [-w[8], -w[9], -w[10]], [w[4], w[5], w[6]], 18 / lens / aspect, aspect)
  }
  if (p?.kind === 'object' && at) {
    const kids = all.filter((a) => (D.props[a] || {}).parent === at)
    const b = worldPoints(D, [at, ...kids], false)
    if (b) {
      const pad = 0.25
      const lo = [b.min[0] - pad, b.min[1] - pad, b.min[2] - 0.05]
      const hi = [b.max[0] + pad, b.max[1] + pad, b.max[2] + 0.1]
      return fitBasis(corners({ min: lo, max: hi }), SCENE_DIR, 0.27, 0.62, aspect)
    }
  }
  const list = mesh.length ? contentsOf(D, mesh) : all
  const b = worldPoints(D, list, false)
  const k = p?.kind === 'turntable' ? p.frame : null
  const dir = k !== null ? turnDir(k) : SCENE_DIR
  let c = fitBasis(b ? b.pts : [], dir, 0.27, 0.68, aspect)
  if (list !== mesh) {
    // The room must fit too: if framing the furniture cuts its corners, frame the whole room.
    const room = worldPoints(D, mesh, false)
    if (room) {
      if (room.pts.map((p) => basisNdc(c, p)).some((q) => q.z <= 0.05 || Math.abs(q.nx) > 0.97 || Math.abs(q.ny) > 0.97)) c = fitBasis(room.pts, dir, 0.27, 0.9, aspect)
      // Then centre everything drawn up and down: no empty band above a cut-down room.
      const ys = room.pts.map((p) => basisNdc(c, p)).filter((q) => q.z > 0.05).map((q) => q.ny)
      if (ys.length) {
        const y0 = Math.min(...ys)
        const y1 = Math.max(...ys)
        if ((y1 - y0) / 2 <= 0.94) c.sy += (y0 + y1) / 2
      }
    }
  }
  return c
}
