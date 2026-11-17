// A real scene doc from the kit's keyed watch: its keyed state (list roots
// and the composite maps of each element's fields) and its view (each
// unit's placement and derived values, each container's derived values),
// in the shape the scene's fold reads (the same shape the sample's doc
// has). The BLEND pack's references between elements are live keys (`c1`,
// `o5`); the tab speaks addresses (`@coll:1`, `@obj:5`), so they are
// spelled as addresses here and in each change frame (normFrame).
//
// The pack's derived values: per unit `mesh_hash` (a mesh's content; a
// change means the mesh changed, and a new still) and `hidden_by` (`self`,
// or `collection:@coll:N`), per container `current_scene` (the scene the
// picture draws). When the view can't pair with its file's version they are
// absent, or `hidden_by` falls back to self: the doc just carries what came.

import type { KeyedWatch } from '@k2/mpc-client'
import type { Doc, Frame } from './types'

const SEP = '\u001f'
/** Fields whose values name other elements. */
const REF_FIELDS = new Set(['coll', 'parent', 'also'])

/** A live key as an address (`c3` → `@coll:3`, `o5` → `@obj:5`); an address, or anything else, as it is. A comma list: each. */
export function addrOfRef(v: string | null | undefined): string | null | undefined {
  if (v == null) return v
  return v
    .split(',')
    .map((x) => {
      const m = /^([co])(0|[1-9]\d*)$/.exec(x.trim())
      return m ? `@${m[1] === 'c' ? 'coll' : 'obj'}:${m[2]}` : x
    })
    .join(',')
}

/** A change frame with its references as addresses. */
export function normFrame(f: Frame): Frame {
  if (!f.ops?.some((o) => o.field && REF_FIELDS.has(o.field))) return f
  return { ...f, ops: f.ops.map((o) => (o.field && REF_FIELDS.has(o.field) ? { ...o, before: addrOfRef(o.before), after: addrOfRef(o.after) } : o)) }
}

/** The doc the watch holds now, or null before its snapshot. */
export function docFromWatch(w: KeyedWatch): Doc | null {
  const st = w.state
  const view = w.view
  if (!st || !view) return null
  const D: Doc = { rev: w.rev ?? view.rev ?? 0, lists: {}, kinds: {}, props: {}, scene: {}, units: {}, space: (view.space as Doc['space']) ?? null, tier: view.tier, removed: {}, added: {} }
  const composite = new Set((st.schema.composite ?? []).map((c) => c.map))
  for (const [name, root] of st.roots) {
    if (root.kind === 'list') {
      D.lists[name] = root.live.map(([k]) => k)
      for (const [k, v] of root.live) D.kinds[k] = v ?? ''
    } else if (root.kind === 'map') {
      for (const [mk, v] of root.entries) {
        if (v == null) continue
        const at = mk.indexOf(SEP)
        if (composite.has(name) && at > 0) {
          const key = mk.slice(0, at)
          const field = mk.slice(at + 1)
          ;(D.props[key] = D.props[key] ?? {})[field] = REF_FIELDS.has(field) ? (addrOfRef(v) as string) : v
        } else D.scene[mk] = v
      }
    }
  }
  const names = view.schema.unit_derived ?? []
  for (const c of view.containers) {
    for (const u of w.units(c.key)) {
      const derived: Record<string, string> = {}
      for (const n of names) {
        const v = w.unitDerived(n, u.key)
        if (v != null) derived[n] = v
      }
      D.units[u.key] = { derived, placement: (u.placement as Doc['units'][string]['placement']) ?? null }
    }
  }
  D.unitDerived = names.slice()
  D.currentScene = view.containers.find((c) => w.containerDerived('current_scene', c.key) === 'true')?.key ?? null
  return D
}

/** After a change: each unit's derived values as the watch has them now (a frame names only what changed), and the scene drawn. */
export function withDerived(D: Doc, w: KeyedWatch): Doc {
  const names = D.unitDerived ?? []
  for (const [k, u] of Object.entries(D.units)) {
    const derived: Record<string, string> = {}
    for (const n of names) {
      const v = w.unitDerived(n, k)
      if (v != null) derived[n] = v
    }
    u.derived = derived
  }
  const cs = w.view?.containers.find((c) => w.containerDerived('current_scene', c.key) === 'true')?.key
  D.currentScene = cs ?? null
  return D
}

/** The units whose mesh changed between two docs (their `mesh_hash` moved). */
export function meshesChanged(a: Doc, b: Doc): string[] {
  return Object.keys(b.units).filter((k) => a.units[k] && a.units[k].derived.mesh_hash !== b.units[k].derived.mesh_hash && (a.units[k].derived.mesh_hash != null || b.units[k].derived.mesh_hash != null))
}
