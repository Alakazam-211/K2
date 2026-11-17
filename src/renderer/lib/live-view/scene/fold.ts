// The scene doc, folded: the keyed state (the kit's list and map roots,
// elements by address) plus the view snapshot's per-unit derived text and
// placements, with each ops_change frame applied in turn. Read through the
// view schema (which root holds collections and objects, which field names
// a title), so a pack with other names reads the same. Pure: a frame makes
// a new doc and the old one stays (the picture's still may lag a rev).

import { GOLD } from './samples'
import { clone, f32, ftext, isAddr, OBJ, COLL, parse9, worldMatrix, type M4 } from './math'
import type { Doc, Frame, IndexJson, Placement, Rot } from './types'

export const VS = GOLD.schema.view
export const CONT = VS.containers
export const UNITS = VS.units
export const UF = VS.unit_fields
export const CF = VS.container_fields
export const PROPS: Record<string, string> = Object.fromEntries((GOLD.schema.keyed.composite ?? []).map((c) => [c.list, c.map]))
/** Collection nesting is a schema relation, generic in the kit (its name is provisional): the tab reads the relation, not a field it knows by name. */
export const CREL = (VS.relations ?? []).find((r) => r.from === CONT && r.to === CONT) ?? null
/** The kit's keyed limit (config.rs): about 2,000 objects. */
export const KEYED_MAX = 25000

export function doc0(): Doc {
  const D: Doc = { rev: GOLD.state.upto, lists: {}, kinds: {}, props: {}, scene: {}, units: {}, space: clone(GOLD.index.space ?? null), tier: GOLD.index.tier, removed: {}, added: {} }
  for (const r of GOLD.state.roots) {
    if (r.kind === 'list') {
      D.lists[r.name] = (r.live ?? []).map((x) => x[0])
      for (const [a, v] of r.live ?? []) D.kinds[a] = v
    } else if (r.composite_of) {
      for (const [a, f, v] of r.entries ?? []) (D.props[a] = D.props[a] ?? {})[f as string] = v
    } else for (const [k, , v] of r.entries ?? []) D.scene[k] = v
  }
  for (const u of GOLD.index.units) D.units[u.key] = { derived: clone(u.derived ?? {}), placement: u.placement ? clone(u.placement) : null }
  return D
}

export function applyFrame(D0: Doc, f: Frame): Doc {
  const D = clone(D0)
  D.rev = f.rev
  D.added = {}
  for (const op of f.ops) {
    if (op.kind === 'list') {
      const L = (D.lists[op.root] = D.lists[op.root] ?? [])
      const i = L.indexOf(op.key)
      if (op.op === 'insert' || op.op === 'restore') {
        if (i < 0) L.splice(op.index_after ?? L.length, 0, op.key)
        D.kinds[op.key] = op.after as string
        D.added[op.key] = f.rev
      } else if (op.op === 'delete') {
        if (i >= 0) L.splice(i, 1)
        D.removed[op.key] = { kind: D.kinds[op.key], props: clone(D.props[op.key] ?? {}), rev: f.rev, at: i }
      } else if (op.op === 'move') {
        if (i >= 0) L.splice(i, 1)
        L.splice(op.index_after ?? L.length, 0, op.key)
      } else if (op.op === 'set') D.kinds[op.key] = op.after as string
    } else if (op.field == null) {
      if (op.op === 'set') D.scene[op.key] = op.after as string
      else delete D.scene[op.key]
    } else {
      const e = (D.props[op.key] = D.props[op.key] ?? {})
      if (op.op === 'set') e[op.field] = op.after as string
      else delete e[op.field]
    }
  }
  for (const d of f.units ?? []) {
    if (d.ops.includes('delete')) {
      delete D.units[d.key]
      continue
    }
    const u = (D.units[d.key] = D.units[d.key] ?? { derived: {}, placement: null })
    if ('placement_after' in d) u.placement = d.placement_after ? clone(d.placement_after) : null
  }
  for (const [a, vals] of Object.entries(f.derived?.units ?? {})) {
    const u = D.units[a]
    if (!u) continue
    for (const n of D.unitDerived ?? VS.unit_derived ?? []) {
      if (!(n in vals)) continue
      if (vals[n] === null) delete u.derived[n]
      else u.derived[n] = vals[n] as string
    }
  }
  if (f.view_space) D.space = clone(f.view_space)
  return D
}

/** The turn's docs by rev: the snapshot, then each frame applied. */
export const DOCS: Record<number, Doc> = { [GOLD.state.upto]: doc0() }
for (const f of GOLD.turn.frames) DOCS[f.rev] = applyFrame(DOCS[f.rev - 1], f)

export interface SceneObj {
  addr: string
  kind: string
  name: string
  hidden: boolean
  noselect: boolean
  norender: boolean
  container: string
  also: string[]
  parent: string | null
  parentListed: boolean
  nid: string
  data: string | null
  /** The pack's edit mark: 1 editable, a driven, t quaternion (no turn), 0 linked. */
  edit: string
  rotmode: string
  xform: string | null
  quat: string | null
  placement: Placement | null
  derived: Record<string, string>
}

export function objOf(D: Doc, a: string): SceneObj {
  const p = D.props[a] ?? {}
  const u = D.units[a] ?? { derived: {}, placement: null }
  const pl = p.parent
  return {
    addr: a,
    kind: D.kinds[a],
    name: p[UF.name] || '',
    hidden: p[UF.hidden] === '1',
    noselect: p.noselect === '1',
    norender: p.norender === '1',
    container: p[VS.unit_container_field],
    also: (p.also || '').split(',').filter(Boolean),
    parent: pl || null,
    parentListed: isAddr(pl, OBJ),
    nid: p.nid || '',
    data: u.derived.data || null,
    edit: p.edit || '0',
    rotmode: p.rotmode || '',
    xform: p.xform || null,
    quat: p.quat || null,
    placement: u.placement,
    derived: u.derived,
  }
}

export interface SceneColl {
  addr: string
  kind: string
  title: string | null
  hidden: boolean
  parent: string | null
  nid: string
}

export function collOf(D: Doc, a: string): SceneColl {
  const p = D.props[a] ?? {}
  const par = CREL ? p[CREL.field] : undefined
  return { addr: a, kind: D.kinds[a], title: p[CF.title] == null ? null : p[CF.title], hidden: p[CF.hidden] === '1', parent: isAddr(par, COLL) ? par : null, nid: p.nid || '' }
}

export const nameOf = (D: Doc, a: string): string => D.props[a]?.name || D.removed[a]?.props?.name || a
export function collTitle(D: Doc, a: string): string {
  const c = collOf(D, a)
  return c.title != null ? c.title : c.kind === 'none' ? 'Not in a collection' : a
}

export interface Nums {
  loc: number[]
  rot: Rot
  scale: number[]
  from: 'placement' | 'xform'
}

/** The numbers: the kit's 3D slot (the placement's last trs step) when it places the object; else the xform field. */
export function numsOf(o: SceneObj): Nums | null {
  const p = o.placement
  const st = p && p.steps && p.steps.length ? p.steps[p.steps.length - 1] : null
  if (st && st.trs) return { loc: st.trs.loc, rot: st.trs.rot, scale: st.trs.scale, from: 'placement' }
  const v = parse9(o.xform)
  if (!v) return null
  return {
    loc: v.slice(0, 3),
    rot: o.rotmode === 'quaternion' && o.quat ? { quat: o.quat.split(',').map(Number) } : { euler: (o.rotmode || 'xyz').toUpperCase(), angles: v.slice(3, 6) },
    scale: v.slice(6, 9),
    from: 'xform',
  }
}

export const metres = (D: Doc): boolean => !!D.space && D.space.metres_per_unit === 1
export const worldOf = (D: Doc, a: string): M4 | null => worldMatrix((x) => (D.units[x] ? D.units[x].placement : undefined), a)

export function liveEntries(D: Doc): number {
  let n = 0
  for (const L of Object.values(D.lists)) n += L.length
  for (const p of Object.values(D.props)) n += Object.keys(p).length
  return n + Object.keys(D.scene).length
}

/** A doc from a kit view index alone (the pinned BLEND snapshots): containers
 *  with their parent, units with placements and derived text, no keyed
 *  fields. `only` limits the units to the pages that have arrived. */
export function docFromIndex(IXP: IndexJson, only?: Set<string>): Doc {
  const D: Doc = { rev: IXP.rev, lists: { [CONT]: [], [UNITS]: [] }, kinds: {}, props: {}, scene: {}, units: {}, space: clone(IXP.space), tier: IXP.tier, removed: {}, added: {}, pinned: true }
  IXP.containers.forEach((c, i) => {
    D.lists[CONT].push(c.key)
    D.kinds[c.key] = c.key === '@coll:0' ? 'none' : i === 0 ? 'scene' : 'collection'
    D.props[c.key] = c.title == null ? { hidden: c.hidden ? '1' : '0' } : { name: c.title, hidden: c.hidden ? '1' : '0' }
    if (c.parent) D.props[c.key].parent = c.parent
  })
  for (const u of IXP.units) {
    if (only && !only.has(u.key)) continue
    const pl = u.placement || null
    const trs = pl && pl.steps.find((st) => st.trs)
    const t = trs ? trs.trs! : null
    const q = t && t.rot.quat
    const ang = t && t.rot.angles
    const x = t
      ? t.loc.concat(ang || [0, 0, 0], t.scale)
      : ((/loc (\S+) rot (\S+) scale (\S+)/.exec(u.derived?.transform || '') as unknown as string[] | null) || [0, '0,0,0', '0,0,0', '1,1,1']).slice(1).join(',').split(',').map(Number)
    D.lists[UNITS].push(u.key)
    D.kinds[u.key] = u.kind
    D.props[u.key] = { coll: u.container, name: u.name, xform: ftext(x.map(f32)), rotmode: q ? 'quaternion' : 'xyz', hidden: u.hidden ? '1' : '0', noselect: '0', norender: '0', edit: pl && pl.approx === 'driven' ? 'a' : '1' }
    if (q) D.props[u.key].quat = ftext(q)
    if (u.derived?.data) D.props[u.key].data = u.derived.data
    if (pl && pl.relative_to) D.props[u.key].parent = pl.relative_to
    D.units[u.key] = { derived: clone(u.derived || {}), placement: clone(pl) }
  }
  return D
}
