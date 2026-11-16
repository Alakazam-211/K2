// Frames and docs the sample makes itself, by the pack's rules, each marked
// `_constructed`: a placement for the large scene, several placements at
// once (with a driven child riding along, approx: "driven"), a rename (name
// and nid in one change), a verb_refused frame, and the generated large,
// too-large and empty scenes (the pack's measurement shape, big_scene).

import { clone, f32, ftext, obFileKey, parse9, placementFrom, xformText } from './math'
import { CONT, PROPS, UNITS } from './fold'
import { ACTORS, ATLAS, BROOK } from './wording'
import type { Doc, Frame, Placement, Refusal, UnitDelta } from './types'

export function bigDoc(colls: number, per: number, tier: string): Doc {
  const D: Doc = {
    rev: 12,
    lists: { [CONT]: ['@coll:1'], [UNITS]: [] },
    kinds: { '@coll:1': 'scene' },
    props: { '@coll:1': { name: 'Scene', nid: '@sc:Scene', hidden: '0' } },
    scene: { version: '5.01', header: 'large', pointer_size: '8', endian: 'little', storage: 'zstd', name_max: '255', unit_scale: '1', 'mark/c': String(colls + 1), 'mark/o': String(colls * per) },
    units: {},
    space: { length_unit: 'scene-unit', metres_per_unit: 1, up: 'z', handed: 'right' },
    tier,
    removed: {},
    added: {},
    generated: true,
  }
  let o = 0
  for (let c = 0; c < colls; c++) {
    const ca = '@coll:' + (c + 2)
    D.lists[CONT].push(ca)
    D.kinds[ca] = 'collection'
    D.props[ca] = { name: 'Coll_' + c, nid: '@gr:Coll_' + c, parent: '@coll:1', hidden: '0' }
    for (let i = 0; i < per; i++) {
      const a = '@obj:' + ++o
      const name = `Obj_${c}_${i}`
      const xf = ftext([(c % 10) * 3 + (i % 4) * 0.6, Math.floor(c / 10) * 3.6 + Math.floor(i / 4) * 0.6, 0.125, 0, 0, 0.1 * i, 1, 1, 1].map(f32))
      const p = { coll: ca, name, nid: obFileKey(name), data: 'mesh @me:Shared', xform: xf, rotmode: 'xyz', hidden: '0', noselect: '0', norender: '0', edit: '1' }
      D.lists[UNITS].push(a)
      D.kinds[a] = 'mesh'
      D.props[a] = p
      D.units[a] = { derived: { transform: xformText(p.xform) as string, data: p.data }, placement: placementFrom(p) }
    }
  }
  return D
}

export function emptyDoc(): Doc {
  return {
    rev: 3,
    lists: { [CONT]: ['@coll:1', '@coll:2'], [UNITS]: [] },
    kinds: { '@coll:1': 'scene', '@coll:2': 'collection' },
    props: { '@coll:1': { name: 'Scene', nid: '@sc:Scene', hidden: '0' }, '@coll:2': { name: 'Collection', nid: '@gr:Collection', parent: '@coll:1', hidden: '0' } },
    scene: { version: '4.02', unit_scale: '1', 'mark/c': '2', 'mark/o': '0' },
    units: {},
    space: { length_unit: 'scene-unit', metres_per_unit: 1, up: 'z', handed: 'right' },
    tier: 'live',
    removed: {},
    added: {},
    generated: true,
  }
}

const unitDelta = (key: string, container: string, ops: string[], fields: string[], placement_after?: Placement | null): UnitDelta => {
  const d: UnitDelta = { key, container, ops, geom_before: null, geom_after: null, fields, hidden_after: null, placeholder_type: null }
  if (placement_after !== undefined) d.placement_after = placement_after
  return d
}

/** A constructed placement frame for the large scene. */
export function placeFrame(D: Doc, a: string, loc: number[], rev: number, actor: string): Frame {
  const p = D.props[a]
  const v = parse9(p.xform) as number[]
  const after = ftext(loc.concat(v.slice(3)).map(f32))
  const np = { ...p, xform: after }
  return {
    _constructed: true, type: 'ops_change', seq: 900, rev, at_ms: 400000, change_kind: 'edit', actor: clone(ACTORS[actor]), client: 4711, via: 'cli', on_behalf_of: null, turn: 't:51f0c2aa9e7d3b16.9', verb: 'object place', claim: null,
    ops: [{ root: PROPS[UNITS], kind: 'map', op: 'set', key: a, field: 'xform', before: p.xform, after }],
    units: [unitDelta(a, p.coll, ['place'], ['xform'], placementFrom(np))],
    containers_changed: [], containers: [p.coll], derived: { units: { [a]: { transform: xformText(after) } } }, render_hashes: null,
  }
}

export type PlaceChange = [string, number[] | null, number[] | null]

/** A constructed frame placing several objects at once: [address, loc or null, scale or null] each.
 *  `rideApprox`: children that ride along, their place op marked approx: "driven". */
export function multiPlaceFrame(D: Doc, changes: PlaceChange[], rev: number, actor: string, rideApprox?: string[]): Frame {
  const ops: Frame['ops'] = []
  const units: UnitDelta[] = []
  const derived: Record<string, Record<string, string | null>> = {}
  const file = ACTORS[actor].kind === 'file'
  for (const [a, loc, scale] of changes) {
    const p = D.props[a]
    const v = parse9(p.xform) as number[]
    const after = ftext((loc ?? v.slice(0, 3)).concat(v.slice(3, 6), scale ?? v.slice(6, 9)).map(f32))
    ops.push({ root: PROPS[UNITS], kind: 'map', op: 'set', key: a, field: 'xform', before: p.xform, after })
    units.push(unitDelta(a, p.coll, ['place'], ['xform'], placementFrom({ ...p, xform: after })))
    derived[a] = { transform: xformText(after) }
  }
  for (const a of rideApprox ?? []) {
    const p = D.props[a]
    const pl = placementFrom(p) as Placement
    pl.approx = 'driven'
    units.push({ ...unitDelta(a, p.coll, [], [], pl), _note: 'rides on its parent; the place op marks it approx: driven (K2 Cores: a real field), so its box is dashed from the first move' })
  }
  return {
    _constructed: true, type: 'ops_change', seq: 902, rev, at_ms: null, change_kind: file ? 'external' : 'edit', actor: clone(ACTORS[actor]), client: file ? 0 : 4711, via: file ? 'file' : 'cli', on_behalf_of: null,
    turn: file ? null : 't:7b3e91c04d2a58f6.6', verb: file ? null : 'object place', claim: null, ops, units, containers_changed: [], containers: [...new Set(units.map((u) => u.container))], derived: { units: derived }, render_hashes: null,
  }
}

/** A constructed rename frame, by the pack's rule: name and nid in one change. */
export function renameFrame(D: Doc, a: string, to: string, rev: number, actor: string, turn?: string): Frame {
  const p = D.props[a]
  return {
    _constructed: true, type: 'ops_change', seq: 901, rev, at_ms: null, change_kind: 'edit', actor: clone(ACTORS[actor]), client: 4712, via: 'cli', on_behalf_of: null, turn: turn || null, verb: 'object rename', claim: null,
    ops: [
      { root: PROPS[UNITS], kind: 'map', op: 'set', key: a, field: 'name', before: p.name, after: to },
      { root: PROPS[UNITS], kind: 'map', op: 'set', key: a, field: 'nid', before: p.nid, after: obFileKey(to) },
    ],
    units: [unitDelta(a, p.coll, ['props'], ['name', 'nid'])],
    containers_changed: [], containers: [p.coll], derived: null, render_hashes: null,
  }
}

/** A verb_refused frame for a case (kit c23d2349): {at_ms, actor, client, turn, verb, unit, code, holder, claim}. */
export const REF = (who: string, verb: string, unit: string, code: string, holder?: string | null, claim?: string | null, turn?: string | null): Refusal => ({
  _constructed: true, type: 'verb_refused', at_ms: null, actor: clone(ACTORS[who]), client: who === BROOK ? 4712 : 4711, turn: turn || null, verb, unit, code, holder: holder || null, claim: claim || null,
})

export { ATLAS }
