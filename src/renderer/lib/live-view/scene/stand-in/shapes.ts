// SAMPLE ONLY. The stand-in still's shapes: boxes, cylinders and lathes
// that stand in for the sample file's meshes, ported from the mock
// (mockup-live-scene-v2.html, still.js). The real tab never sees a mesh:
// K2 Render draws the still. Retirement ledger: goes when real stills are
// proven (build-live-scene.md §6).

import { UNITS, objOf, worldOf } from '../fold'
import type { Doc } from '../types'

type Prim = { box: number[] } | { cyl: number[]; caps: 'both' | 'top' | 'bottom' | 'none' }

const B = (x0: number, y0: number, z0: number, x1: number, y1: number, z1: number): Prim => ({ box: [x0, y0, z0, x1, y1, z1] })
const C = (cx: number, cy: number, z0: number, z1: number, r0: number, r1: number, n = 18, caps: 'both' | 'top' | 'bottom' | 'none' = 'both'): Prim => ({ cyl: [cx, cy, z0, z1, r0, r1, n], caps })
/** A lathe: stacked rings, capped only at the bottom and the top. */
const lathe = (prof: number[][], n = 26): Prim[] => prof.slice(1).map((p, i) => C(0, 0, prof[i][0], p[0], prof[i][1], p[1], n, i === 0 ? 'bottom' : i === prof.length - 2 ? 'top' : 'none'))
const legs = (x: number, y: number, h: number, s: number): Prim[] => [[-x, -y], [x, -y], [-x, y], [x, y]].map(([a, b]) => B(a - s, b - s, 0, a + s, b + s, h))
const shelfBoards = [0.1, 0.62, 1.12, 1.62, 1.8].map((t) => B(-0.47, -0.17, t - 0.03, 0.47, 0.15, t))

const SHAPES: Record<string, Prim[]> = {
  '@me:Plane': [B(-1, -1, -0.02, 1, 1, 0)],
  '@me:Wall': [B(-1, -1, 0, 1, 1, 0.06)],
  '@me:Window': [B(-1, 0.86, 0, 1, 1, 0.07), B(-1, -1, 0, 1, -0.86, 0.07), B(-1, -1, 0, -0.88, 1, 0.07), B(0.88, -1, 0, 1, 1, 0.07), B(-0.03, -0.86, 0, 0.03, 0.86, 0.05), B(-0.88, 0.0, 0, 0.88, 0.05, 0.04), B(-0.88, -0.86, 0, 0.88, 0.86, 0.012), B(-1.1, -1.16, 0, 1.1, -1.0, 0.16)],
  '@me:Rug': [B(-1, -1, 0, 1, 1, 0.014)],
  '@me:Fan': [C(0, 0, -0.1, 0, 0.1, 0.08, 16), C(0, 0, 0, 0.3, 0.018, 0.018, 8), B(0.08, -0.07, -0.07, 0.62, 0.07, -0.055), B(-0.62, -0.07, -0.07, -0.08, 0.07, -0.055), B(-0.07, 0.08, -0.07, 0.07, 0.62, -0.055), B(-0.07, -0.62, -0.07, 0.07, -0.08, -0.055)],
  '@me:Table': [B(-0.72, -0.42, 0.69, 0.72, 0.42, 0.77), B(-0.62, -0.32, 0.6, 0.62, 0.32, 0.69), ...legs(0.6, 0.3, 0.69, 0.045)],
  '@me:Chair': [B(-0.22, -0.22, 0.42, 0.22, 0.22, 0.48), ...legs(0.18, 0.18, 0.42, 0.03), B(-0.22, -0.23, 0.48, 0.22, -0.16, 0.94), B(-0.2, -0.16, 0.48, 0.2, -0.12, 0.6)],
  '@me:Sofa': [B(-1, -0.45, 0.1, 1, 0.45, 0.42), B(-0.86, -0.42, 0.42, -0.01, 0.26, 0.58), B(0.01, -0.42, 0.42, 0.86, 0.26, 0.58), B(-1, 0.24, 0.42, 1, 0.45, 0.88), B(-0.84, 0.16, 0.58, -0.02, 0.26, 0.82), B(0.02, 0.16, 0.58, 0.84, 0.26, 0.82), B(-1, -0.45, 0.42, -0.86, 0.24, 0.68), B(0.86, -0.45, 0.42, 1, 0.24, 0.68), ...legs(0.92, 0.38, 0.1, 0.04)],
  '@me:Shelf': [B(-0.5, -0.17, 0, -0.47, 0.17, 1.82), B(0.47, -0.17, 0, 0.5, 0.17, 1.82), B(-0.5, 0.15, 0, 0.5, 0.17, 1.82), ...shelfBoards],
  '@me:Book': [B(-0.02, -0.11, 0, 0.02, 0.11, 0.27)],
  '@me:Lamp': [C(0, 0, 0, 0.04, 0.18, 0.16, 24), C(0, 0, 0.04, 1.42, 0.03, 0.03, 12), C(0, 0, 1.36, 1.7, 0.25, 0.15, 28)],
  '@me:Vase': lathe([[0, 0.055], [0.03, 0.08], [0.1, 0.1], [0.17, 0.09], [0.24, 0.055], [0.3, 0.038], [0.36, 0.042], [0.38, 0.056]]),
  '@me:Mug': [C(0, 0, 0, 0.1, 0.045, 0.045, 16), B(0.04, -0.008, 0.025, 0.075, 0.008, 0.08)],
  '@me:Hand': [B(-0.01, -0.02, 0.012, 0.01, 0.12, 0.024), C(0, 0, -0.01, 0.012, 0.16, 0.16, 32)],
  '@me:Stool': [C(0, 0, 0.42, 0.47, 0.19, 0.19, 20), ...[0, 2.09, 4.19].map((a) => B(Math.cos(a) * 0.13 - 0.018, Math.sin(a) * 0.13 - 0.018, 0, Math.cos(a) * 0.13 + 0.018, Math.sin(a) * 0.13 + 0.018, 0.42))],
  '@me:Shared': [B(-0.18, -0.18, 0, 0.18, 0.18, 0.26), B(-0.12, -0.12, 0.26, 0.12, 0.12, 0.34)],
  // The pinned test scenes' empties: a small three-axis marker each.
  '@me:Empty': [B(-0.16, -0.025, -0.025, 0.16, 0.025, 0.025), B(-0.025, -0.16, -0.025, 0.025, 0.16, 0.025), B(-0.025, -0.025, -0.02, 0.025, 0.025, 0.26)],
  '@me:Cube': [B(-0.25, -0.25, -0.25, 0.25, 0.25, 0.25)],
  '@me:Ball': lathe([[-0.2, 0.001], [-0.17, 0.11], [-0.1, 0.17], [0, 0.2], [0.1, 0.17], [0.17, 0.11], [0.2, 0.001]], 24).map((p) => ('cyl' in p ? { ...p, cyl: [0.7, ...p.cyl.slice(1)] } : p)),
}

/** The floor and walls take the shell's clay; everything else the contents'. */
export const SHELL_SHAPES = new Set(['@me:Plane', '@me:Wall', '@me:Window'])

export interface Tris {
  /** Flat triangles in local space, three vertices each, xyz. */
  f: Float32Array
  n: number
  min: number[]
  max: number[]
}

const TRIS = new Map<string, Tris | null>()
export function trisOf(key: string | null): Tris | null {
  if (!key) return null
  if (TRIS.has(key)) return TRIS.get(key) ?? null
  const prims = SHAPES[key]
  if (!prims) {
    TRIS.set(key, null)
    return null
  }
  const out: number[][] = []
  const q = (a: number[], b: number[], c: number[], d: number[]): void => void out.push(a, b, c, a, c, d)
  for (const p of prims) {
    if ('box' in p) {
      const [x0, y0, z0, x1, y1, z1] = p.box
      const v = (i: number): number[] => [i & 1 ? x1 : x0, i & 2 ? y1 : y0, i & 4 ? z1 : z0]
      q(v(0), v(2), v(3), v(1))
      q(v(4), v(5), v(7), v(6))
      q(v(0), v(1), v(5), v(4))
      q(v(2), v(6), v(7), v(3))
      q(v(0), v(4), v(6), v(2))
      q(v(1), v(3), v(7), v(5))
    } else {
      const [cx, cy, z0, z1, r0, r1, n] = p.cyl
      for (let i = 0; i < n; i++) {
        const a = (i / n) * Math.PI * 2
        const b = ((i + 1) / n) * Math.PI * 2
        const P = (r: number, t: number, z: number): number[] => [cx + r * Math.cos(t), cy + r * Math.sin(t), z]
        q(P(r0, a, z0), P(r0, b, z0), P(r1, b, z1), P(r1, a, z1))
        if (r1 > 0.001 && (p.caps === 'both' || p.caps === 'top')) out.push([cx, cy, z1], P(r1, a, z1), P(r1, b, z1))
        if (r0 > 0.001 && (p.caps === 'both' || p.caps === 'bottom')) out.push([cx, cy, z0], P(r0, b, z0), P(r0, a, z0))
      }
    }
  }
  const f = new Float32Array(out.length * 3)
  out.forEach((v, i) => f.set(v, i * 3))
  const mn = [Infinity, Infinity, Infinity]
  const mx = [-Infinity, -Infinity, -Infinity]
  for (let i = 0; i < f.length; i += 3)
    for (let k = 0; k < 3; k++) {
      mn[k] = Math.min(mn[k], f[i + k])
      mx[k] = Math.max(mx[k], f[i + k])
    }
  const t = { f, n: out.length / 3, min: mn, max: mx }
  TRIS.set(key, t)
  return t
}

const meshKey = (data: string | null): string | null => (data && data.startsWith('mesh ') ? data.slice(5) : null)

/** The stand-in shape of an object: its mesh's, or a marker for an empty in the pinned test scenes. */
export function shapeKey(D: Doc, a: string): string | null {
  const o = objOf(D, a)
  return meshKey(o.data) || (D.pinned && o.kind === 'empty' && !o.hidden ? '@me:Empty' : null)
}

/** Drawn in the still: a mesh (or a pinned empty) that renders, isn't linked, and has a shape. */
export function drawable(D: Doc, a: string): boolean {
  const o = objOf(D, a)
  return (o.kind === 'mesh' || (!!D.pinned && o.kind === 'empty')) && !o.norender && o.edit !== '0' && !!trisOf(shapeKey(D, a))
}

/** An object's local bounds (K2 Render's listing has them for real: `bounds {min, max, approx}`). */
export function boundsOf(D: Doc, a: string): { min: number[]; max: number[] } | null {
  const t = trisOf(shapeKey(D, a))
  return t ? { min: t.min, max: t.max } : null
}

const xf = (m: ArrayLike<number>, p: ArrayLike<number>): number[] => [m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12], m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13], m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14]]
export const corners = (b: { min: number[]; max: number[] }): number[][] => [0, 1, 2, 3, 4, 5, 6, 7].map((i) => [i & 1 ? b.max[0] : b.min[0], i & 2 ? b.max[1] : b.min[1], i & 4 ? b.max[2] : b.min[2]])
export { xf }

/** The world points of some objects: their boxes' corners (or origins only). */
export function worldPoints(D: Doc, addrs: readonly string[], originsOnly: boolean): { pts: number[][]; min: number[]; max: number[] } | null {
  const pts: number[][] = []
  for (const a of addrs) {
    const w = worldOf(D, a)
    if (!w) continue
    const b = !originsOnly && drawable(D, a) ? boundsOf(D, a) : null
    if (b) for (const c of corners(b)) pts.push(xf(w, c))
    else pts.push([w[12], w[13], w[14]])
  }
  if (!pts.length) return null
  const min = [Infinity, Infinity, Infinity]
  const max = [-Infinity, -Infinity, -Infinity]
  for (const p of pts) for (let k = 0; k < 3; k++) {
    min[k] = Math.min(min[k], p[k])
    max[k] = Math.max(max[k], p[k])
  }
  return { pts, min, max }
}

export const unitsOf = (D: Doc): string[] => D.lists[UNITS] ?? []
