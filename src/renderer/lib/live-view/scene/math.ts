// The pack's rules, re-done for the tab: addresses, f32 text and the
// transform text (model.rs, view.rs), placements (place.rs), and the kit's
// placement math (client/src/placement.ts at v0.18.0): trs and matrix
// steps, Euler and quaternion, world = parent world · parent inverse · own.
// Every reference in a view is an address (@coll:3, @obj:16), never an
// internal key (K2 Cores, 2026-11-13): the tab reads references as sent.

import type { Placement, PlacementStep, Rot } from './types'

const ADDR = /^@[a-z]+:(0|[1-9]\d*)$/
export const OBJ = '@obj:'
export const COLL = '@coll:'

export const isAddr = (s: unknown, pre?: string): s is string => typeof s === 'string' && ADDR.test(s) && (!pre || s.startsWith(pre))
/** Any kit address: @kind:id, or a scene path form (@scene, @scene/turntable#k). */
export const isAddrAny = (s: unknown): boolean => typeof s === 'string' && /^@[a-z]+(:(0|[1-9]\d*))?(\/[a-z]+(#\d+)?)?$/.test(s)

/** The shortest decimal that rounds to the same f32. */
export function f32(x: number): number {
  const f = Math.fround(x)
  if (f === 0) return 0
  for (let p = 1; p <= 9; p++) {
    const s = Number(f.toPrecision(p))
    if (Math.fround(s) === f) return s
  }
  return f
}

export function parse9(s: unknown): number[] | null {
  if (typeof s !== 'string') return null
  const v = s.split(',').map(Number)
  return v.length === 9 && v.every(Number.isFinite) ? v : null
}

export const ftext = (v: readonly number[]): string => v.map((n) => String(n)).join(',')

export function xformText(x: string | null | undefined): string | null {
  const v = parse9(x)
  if (!v) return null
  const t = (i: number): string => ftext(v.slice(i, i + 3))
  return `loc ${t(0)} rot ${t(3)} scale ${t(6)}`
}

export function xformParts(s: string | null | undefined): { loc: number[]; rot: number[]; scale: number[] } | null {
  const v = parse9(s)
  return v ? { loc: v.slice(0, 3), rot: v.slice(3, 6), scale: v.slice(6, 9) } : null
}

export const ID16 = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]

/** place.rs placement(): own fields only; a parent inverse from the carrier (identity in the sample). */
export function placementFrom(p: Record<string, string>, parentInv?: number[]): Placement | null {
  const x = parse9(p.xform)
  if (!x || !p.rotmode) return null
  const rot: Rot | null = p.rotmode === 'quaternion' ? (p.quat ? { quat: p.quat.split(',').map(Number) } : null) : p.rotmode === 'axis_angle' ? null : { euler: p.rotmode.toUpperCase(), angles: x.slice(3, 6) }
  if (!rot) return null
  const trs: PlacementStep = { trs: { loc: x.slice(0, 3), rot, scale: x.slice(6, 9) } }
  const par = p.parent && isAddr(p.parent, OBJ) ? p.parent : null
  if (p.parent && !par) return null // a parent that isn't a listed object: no placement, never a guess
  const out: Placement = { relative_to: par, steps: par ? [{ matrix: (parentInv ?? ID16).slice() }, trs] : [trs] }
  if (p.edit === 'a') out.approx = 'driven'
  return out
}

/** scene.rs escape_name: whitespace, %, / and control characters as %XX. */
export const escName = (name: string): string =>
  [...name]
    .map((c) =>
      /\s/.test(c) || c === '%' || c === '/' || c.charCodeAt(0) < 32
        ? [...new TextEncoder().encode(c)].map((b) => '%' + b.toString(16).toUpperCase().padStart(2, '0')).join('')
        : c,
    )
    .join('')
export const obFileKey = (name: string): string => '@ob:' + escName(name)

export type M4 = Float64Array

export function mul(a: ArrayLike<number>, b: ArrayLike<number>): M4 {
  const out = new Float64Array(16)
  for (let c = 0; c < 4; c++)
    for (let r = 0; r < 4; r++) {
      let s = 0
      for (let k = 0; k < 4; k++) s += a[k * 4 + r] * b[c * 4 + k]
      out[c * 4 + r] = s
    }
  return out
}

function ident(): M4 {
  const m = new Float64Array(16)
  m[0] = m[5] = m[10] = m[15] = 1
  return m
}

type M3 = number[][]
const m3mul = (a: M3, b: M3): M3 => [0, 1, 2].map((r) => [0, 1, 2].map((c) => a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c]))
function basic(axis: number, a: number): M3 {
  const s = Math.sin(a)
  const c = Math.cos(a)
  if (axis === 0) return [[1, 0, 0], [0, c, -s], [0, s, c]]
  if (axis === 1) return [[c, 0, s], [0, 1, 0], [-s, 0, c]]
  return [[c, -s, 0], [s, c, 0], [0, 0, 1]]
}
const ORDERS = ['XYZ', 'XZY', 'YXZ', 'YZX', 'ZXY', 'ZYX']

function rotM(r: Rot | undefined): M3 | null {
  if (!r) return null
  if (r.euler && r.angles) {
    if (ORDERS.indexOf(r.euler) < 0) return null
    const a = r.angles.map(Math.fround)
    const ax = [...r.euler].map((ch) => 'XYZ'.indexOf(ch))
    return m3mul(m3mul(basic(ax[2], a[ax[2]]), basic(ax[1], a[ax[1]])), basic(ax[0], a[ax[0]]))
  }
  if (r.quat) {
    const q = r.quat.map(Math.fround)
    const n = Math.hypot(...q)
    if (!(n > 0)) return null
    const [w, x, y, z] = q.map((v) => v / n)
    return [
      [1 - 2 * (y * y + z * z), 2 * (x * y - w * z), 2 * (x * z + w * y)],
      [2 * (x * y + w * z), 1 - 2 * (x * x + z * z), 2 * (y * z - w * x)],
      [2 * (x * z - w * y), 2 * (y * z + w * x), 1 - 2 * (x * x + y * y)],
    ]
  }
  return null
}

function stepM(s: PlacementStep): M4 | null {
  if (s.trs) {
    const r = rotM(s.trs.rot)
    if (!r) return null
    const sc = s.trs.scale.map(Math.fround)
    const l = s.trs.loc.map(Math.fround)
    const m = ident()
    for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) m[j * 4 + i] = r[i][j] * sc[j]
    m[12] = l[0]
    m[13] = l[1]
    m[14] = l[2]
    return m
  }
  if (s.matrix) {
    const m = s.matrix.map(Math.fround)
    if (m[3] !== 0 || m[7] !== 0 || m[11] !== 0 || m[15] !== 1) return null
    return Float64Array.from(m)
  }
  return null
}

export function placementMatrix(p: Placement | null | undefined): M4 | null {
  if (!p || !p.steps || !p.steps.length) return null
  let m = ident()
  for (const s of p.steps) {
    const sm = stepM(s)
    if (!sm) return null
    m = mul(m, sm)
  }
  return m.every(Number.isFinite) ? m : null
}

export function worldMatrix(lookup: (a: string) => Placement | null | undefined, address: string): M4 | null {
  const chain: Placement[] = []
  const seen = new Set<string>()
  let at: string | null = address
  while (at !== null && at !== undefined) {
    if (seen.has(at)) return null
    seen.add(at)
    const p = lookup(at)
    if (!p) return null
    chain.push(p)
    at = p.relative_to == null ? null : p.relative_to
  }
  let w = ident()
  for (let i = chain.length - 1; i >= 0; i--) {
    const m = placementMatrix(chain[i])
    if (!m) return null
    w = mul(w, m)
  }
  return w
}

// ── JSON helpers: the mock's clone, strip (drop `_` annotations) and same ──
export const clone = <T>(o: T): T => (o === undefined ? o : (JSON.parse(JSON.stringify(o)) as T))
export function strip<T>(o: T): T {
  if (Array.isArray(o)) return o.map(strip) as unknown as T
  if (o && typeof o === 'object') {
    const r: Record<string, unknown> = {}
    for (const [k, v] of Object.entries(o)) if (k[0] !== '_') r[k] = strip(v)
    return r as T
  }
  return o
}
export const same = (a: unknown, b: unknown): boolean => JSON.stringify(strip(a)) === JSON.stringify(strip(b))
