// No picture yet (K2 can't read the file's shapes): the tab still shows what
// it knows, where each object sits, from the kit's placements alone. A
// ground grid and one dot per object's origin, through a 3/4 camera fitted
// to the origins. Pure: lines and dots in the picture's pixels.

import { UNITS, objOf, worldOf } from './fold'
import { SCENE_DIR, fitBasis, stillCamera } from './project'
import { projectPx, type StillCamera } from './project'
import type { Doc } from './types'

export interface Ground {
  camera: StillCamera
  lines: { x1: number; y1: number; x2: number; y2: number }[]
  dots: { id: string; x: number; y: number }[]
}

/** The grid and origin dots at W × H. */
export function groundOf(D: Doc, W: number, H: number): Ground {
  const objs = (D.lists[UNITS] ?? []).filter((a) => {
    const k = objOf(D, a).kind
    return k !== 'camera' && k !== 'light'
  })
  const origins: { id: string; p: number[] }[] = []
  for (const a of objs) {
    const w = worldOf(D, a)
    if (w) origins.push({ id: a, p: [w[12], w[13], w[14]] })
  }
  const camera = stillCamera(fitBasis(origins.map((o) => o.p), SCENE_DIR, 0.27, 0.8, W / H))
  const size = { width: W, height: H }
  const lines: Ground['lines'] = []
  for (let i = -4; i <= 4; i++) {
    for (const [a, b] of [[[i, -4, 0], [i, 4, 0]], [[-4, i, 0], [4, i, 0]]]) {
      const p = projectPx(a, camera, size)
      const q = projectPx(b, camera, size)
      if (p.inFront && q.inFront) lines.push({ x1: p.x, y1: p.y, x2: q.x, y2: q.y })
    }
  }
  const dots = origins.flatMap((o) => {
    const q = projectPx(o.p, camera, size)
    return q.inFront ? [{ id: o.id, x: q.x, y: q.y }] : []
  })
  return { camera, lines, dots }
}
