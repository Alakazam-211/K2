// Change frames the sample builds itself (the watched edit, the large
// timeline, the empty one, outside saves): the diff of two books in the
// kit's ops_change shape, each touched clip's timecode on the frame as the
// pack sends it. Each built frame remembers the books either side.

import { actorOf } from './actors'
import { clone } from './adapter'
import { modelOf } from './fold'
import { LIST_OF, PROPS, UNIT, type Actor, type Book, type TimelineFrame, type TimelineOp, type TimelineUnitDelta } from './schema'

const BUILT = new WeakMap<TimelineFrame, { before: Book; after: Book }>()

/** The books either side of a frame the sample built; undefined for a frame from the data. */
export const builtOf = (f: TimelineFrame): { before: Book; after: Book } | undefined => BUILT.get(f)

export function bookDiff(B0: Book, B1: Book): { ops: TimelineOp[]; units: TimelineUnitDelta[]; derived: TimelineFrame['derived'] } {
  const ops: TimelineOp[] = []
  const units: TimelineUnitDelta[] = []
  const tcs: Record<string, { tc: string | null }> = {}
  for (const root of Object.keys(B1.lists)) {
    const a = (B0.lists[root] || []).map((x) => x[0])
    const b = B1.lists[root].map((x) => x[0])
    const cur = a.slice()
    const sa = new Set(a)
    const sb = new Set(b)
    for (const k of a)
      if (!sb.has(k)) {
        const i = cur.indexOf(k)
        cur.splice(i, 1)
        ops.push({ root, kind: 'list', op: 'delete', key: k, index_before: i, index_after: null, before: B0.lists[root].find((x) => x[0] === k)![1], effect: 'visible' })
      }
    b.forEach((k, i) => {
      if (!sa.has(k)) {
        cur.splice(i, 0, k)
        ops.push({ root, kind: 'list', op: 'insert', key: k, index_before: null, index_after: i, after: B1.lists[root][i][1], effect: 'visible' })
      }
    })
  }
  for (const root of Object.keys(B1.maps)) {
    const M0 = B0.maps[root] || {}
    const M1 = B1.maps[root]
    const list = LIST_OF[root]
    const live = list ? new Set(B1.lists[list].map((x) => x[0])) : null
    for (const k of new Set([...Object.keys(M0), ...Object.keys(M1)])) {
      const p0 = M0[k] || {}
      const p1 = M1[k] || {}
      if (live && !live.has(k)) continue // a deleted element keeps its fields
      for (const f of new Set([...Object.keys(p0), ...Object.keys(p1)])) {
        if (p0[f] === p1[f]) continue
        const field = list ? f : null
        if (f in p1) ops.push({ root, kind: 'map', op: 'set', key: k, field, ...(f in p0 ? { before: p0[f] } : {}), after: p1[f] })
        else ops.push({ root, kind: 'map', op: 'delete', key: k, field, before: p0[f] })
      }
    }
  }
  const M0 = modelOf(B0)
  const M1 = modelOf(B1)
  const touched = new Set(ops.filter((o) => o.root === UNIT || o.root === PROPS[UNIT]).map((o) => o.key))
  for (const k of touched) {
    const a = M0.items[k]
    const b = M1.items[k]
    const base = { key: k, hidden_after: null, placeholder_type: null }
    if (b && !a) {
      units.push({ ...base, container: b.track, ops: ['add'], geom_before: null, geom_after: b.geom ?? null, fields: [] })
      tcs[k] = { tc: M1.tc(b.s) }
      continue
    }
    if (a && !b) {
      units.push({ ...base, container: a.track, ops: ['delete'], geom_before: a.geom ?? null, geom_after: null, fields: [] })
      continue
    }
    const ch = ops.filter((o) => o.root === PROPS[UNIT] && o.key === k).map((o) => (o.kind === 'map' ? o.field || '' : ''))
    const o: string[] = []
    if (ch.includes('geom') || ch.includes('track')) o.push('move_resize')
    if (ch.includes('name')) o.push('text')
    if (ch.includes('off')) o.push('hide_show')
    const geom = ch.includes('geom')
    units.push({ ...base, container: b.track, ops: o, geom_before: geom ? (a.geom ?? null) : null, geom_after: geom ? (b.geom ?? null) : null, fields: ch.sort(), hidden_after: ch.includes('off') ? b.off : null })
    tcs[k] = { tc: M1.tc(b.s) } // every touched clip's timecode rides on the frame (ruled)
  }
  return { ops, units, derived: Object.keys(tcs).length ? { units: tcs } : null }
}

/** A change frame from `B0` by `mutate`, from `actor` (a handle). */
export function mkFrame(B0: Book, rev: number, at: number, actor: string, verb: string | null, mutate: (B: Book) => void, o?: Partial<TimelineFrame>): TimelineFrame {
  const B1 = clone(B0)
  B1.rev = rev
  mutate(B1)
  const d = bookDiff(B0, B1)
  const a = actorOf(actor)
  const file = a.kind === 'file'
  const wire: Actor = { id: a.id, name: a.name, kind: a.kind, colour: file ? null : a.colour }
  const f: TimelineFrame = {
    _constructed: true,
    type: 'ops_change',
    seq: rev,
    rev,
    at_ms: at,
    change_kind: file ? 'external' : 'edit',
    actor: wire,
    client: 0,
    via: file ? 'external' : a.kind === 'agent' ? 'cli' : 'ui',
    on_behalf_of: null,
    turn: null,
    verb,
    claim: null,
    ops: d.ops,
    units: d.units,
    containers_changed: [],
    containers: [...new Set(d.units.map((u) => u.container as string))],
    derived: d.derived,
    render_hashes: null,
    ...(o || {}),
  }
  BUILT.set(f, { before: B0, after: B1 })
  return f
}

/** Set a clip's geom ("start,duration" in ticks). */
export const geomSet = (B: Book, k: string, s: string, d: string): void => {
  B.maps[PROPS[UNIT]][k].geom = `${s},${d}`
}
