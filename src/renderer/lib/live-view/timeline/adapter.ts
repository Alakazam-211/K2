// The one adapter between where a timeline's state comes from and the view's
// book (schema.ts). Three sources, one book:
//
//   - `bookFromKeyedState`: the kit's KeyedWatch (`watch.state`, with
//     `watch.containerDerived`): the real path (T7). K2 Cores' timeline pack
//     frames drop in here, and each ops_change goes through `applyFrame`;
//   - `bookFromPinned`: the pack's pinned goldens (crates/timeline/tests/
//     kit_view_golden), read exactly as pinned;
//   - `bookFromPages`: the sample's snapshot pages (`{upto, roots}`).
//
// Nothing past this file knows which one it was.

import type { KeyedState } from '@k2/mpc-client'
import type { Book, KV, TimelineFrame, TimelineOp, TrackDerived } from './schema'

export const clone = <T>(o: T): T => (o === undefined ? o : (JSON.parse(JSON.stringify(o)) as T))

/** Addresses (view.rs address_of / key_of): fields hold keys (t1), the wire names elements by address (@trk:1). */
const ADDR: Record<string, [string, string]> = { tracks: ['t', '@trk:'], items: ['i', '@item:'], markers: ['k', '@mk:'], media: ['m', '@med:'] }
const NUM = /^(0|[1-9]\d*)$/

export function addrOf(root: string, key: string | null | undefined): string {
  const a = ADDR[root]
  if (!a || typeof key !== 'string' || key[0] !== a[0] || !NUM.test(key.slice(1))) return key ?? ''
  return a[1] + key.slice(1)
}

export function keyOf(root: string, ad: unknown): string | null {
  const a = ADDR[root]
  if (!a || typeof ad !== 'string' || !ad.startsWith(a[1])) return null
  const n = ad.slice(a[1].length)
  return NUM.test(n) ? a[0] + n : null
}

export interface PagedRoot {
  name: string
  kind: string
  live?: [string, KV][]
  hidden?: [string, KV][]
  entries?: [string, string | null, KV][]
}

export interface IndexContainer {
  key: string
  derived?: TrackDerived | null
}

/** The sample's snapshot pages, in the schema's root order, with the index's derived values. */
export function bookFromPages(st: { upto: number; roots: PagedRoot[] }, containers?: IndexContainer[] | null): Book {
  const B: Book = { rev: st.upto, lists: {}, hidden: {}, maps: {}, derived: { containers: {} } }
  for (const c of containers || []) if (c.derived) B.derived.containers[c.key] = clone(c.derived)
  for (const r of st.roots) {
    if (r.kind === 'list') {
      B.lists[r.name] = (r.live || []).map((x) => [x[0], x[1]])
      B.hidden[r.name] = (r.hidden || []).map((x) => [x[0], x[1]])
    } else {
      const m: Record<string, Record<string, KV>> = {}
      for (const [k, f, v] of r.entries || []) (m[k] = m[k] || {})[f == null ? '' : f] = v
      B.maps[r.name] = m
    }
  }
  return B
}

/** The pinned golden's shape: `{tracks, items, markers, media: [[address, kind]], *_props: {"<address> <field>": value}, timeline: {field: value}}`. */
export interface PinnedSnapshot {
  index: { rev: number; containers?: IndexContainer[] }
  state: Record<string, unknown>
}

export function bookFromPinned(G: PinnedSnapshot): Book {
  const st = G.state as Record<string, unknown>
  const B: Book = { rev: G.index.rev, lists: {}, hidden: {}, maps: {}, derived: { containers: {} } }
  for (const r of ['tracks', 'items', 'markers', 'media']) {
    B.lists[r] = ((st[r] as [string, KV][] | undefined) || []).map((x) => [x[0], x[1]])
    B.hidden[r] = []
  }
  for (const r of ['track_props', 'item_props', 'marker_props', 'media_props']) {
    const mm: Record<string, Record<string, KV>> = {}
    for (const [k, v] of Object.entries((st[r] as Record<string, KV> | undefined) || {})) {
      const i = k.indexOf(' ')
      ;(mm[k.slice(0, i)] = mm[k.slice(0, i)] || {})[k.slice(i + 1)] = v
    }
    B.maps[r] = mm
  }
  B.maps.timeline = Object.fromEntries(Object.entries((st.timeline as Record<string, KV> | undefined) || {}).map(([f, v]) => [f, { '': v }]))
  for (const c of G.index.containers || []) if (c.derived) B.derived.containers[c.key] = { kind_label: c.derived.kind_label, kind: c.derived.kind }
  // Only where the data has none: the track's name is its label.
  for (const [a, kind] of B.lists.tracks) {
    if (!B.derived.containers[a]) B.derived.containers[a] = { kind_label: String((B.maps.track_props[a] || {}).name || ''), kind: kind ?? undefined }
  }
  return B
}

const SEP = '\u001f'

/** The kit's KeyedWatch state (`watch.state`) at `rev`, labels and kinds from `watch.containerDerived`. */
export function bookFromKeyedState(state: KeyedState, rev: number, containerDerived?: (name: string, container: string) => string | null): Book {
  const B: Book = { rev, lists: {}, hidden: {}, maps: {}, derived: { containers: {} } }
  for (const [name, r] of state.roots) {
    if (r.kind === 'list') {
      B.lists[name] = r.live.map((x) => [x[0], x[1]])
      B.hidden[name] = [...r.hidden].map(([k, v]) => [k, v])
    } else if (r.kind === 'map') {
      const m: Record<string, Record<string, KV>> = {}
      for (const [mk, v] of r.entries) {
        const i = mk.indexOf(SEP)
        const k = i < 0 ? mk : mk.slice(0, i)
        ;(m[k] = m[k] || {})[i < 0 ? '' : mk.slice(i + 1)] = v
      }
      B.maps[name] = m
    }
  }
  if (containerDerived)
    for (const [a] of B.lists.tracks || []) {
      const kind_label = containerDerived('kind_label', a)
      const kind = containerDerived('kind', a)
      if (kind_label || kind) B.derived.containers[a] = { ...(kind_label ? { kind_label } : {}), ...(kind ? { kind } : {}) }
    }
  return B
}

/** One op, as the kit client's KeyedWatch applies it: anything that doesn't fit throws (a misfit, never guessed at). */
export function applyOp(B: Book, o: TimelineOp): void {
  if (o.kind === 'map') {
    const M = B.maps[o.root]
    if (!M) throw new Error('no root ' + o.root)
    const fld = o.field == null ? '' : o.field
    if (o.op === 'set') {
      if (!('after' in o)) throw new Error(`set of ${o.key} without a value`)
      ;(M[o.key] = M[o.key] || {})[fld] = o.after ?? null
    } else {
      if (!M[o.key] || !(fld in M[o.key])) throw new Error(`delete of ${o.key} ${fld}, absent`)
      delete M[o.key][fld]
    }
    return
  }
  const L = B.lists[o.root]
  const H = B.hidden[o.root]
  if (!L) throw new Error('no list ' + o.root)
  const at = L.findIndex((x) => x[0] === o.key)
  if (o.op === 'insert') {
    if (at >= 0 || o.index_after == null || o.index_after > L.length) throw new Error(`insert of ${o.key} at ${o.index_after}`)
    L.splice(o.index_after, 0, [o.key, o.after == null ? null : o.after])
  } else if (o.op === 'delete') {
    if (at < 0 || at !== o.index_before) throw new Error(`delete ${o.key}: not at ${o.index_before}`)
    H.push(L.splice(at, 1)[0])
  } else if (o.op === 'move') {
    if (at !== o.index_before || o.index_after == null || o.index_after >= L.length) throw new Error(`move ${o.key}`)
    const [e] = L.splice(at, 1)
    L.splice(o.index_after, 0, e)
  } else if (o.op === 'set') {
    if (at < 0) throw new Error(`set ${o.key}: not live`)
    L[at][1] = o.after ?? null
  } else throw new Error('op ' + String((o as { op: string }).op))
}

/** A change frame on a book: a new book (books are never changed in place). Misfits go to `misfits`. */
export function applyFrame(B0: Book, f: Pick<TimelineFrame, 'rev' | 'ops' | 'derived'>, misfits?: string[]): Book {
  const B = clone(B0)
  B.rev = f.rev
  for (const o of f.ops) {
    try {
      applyOp(B, o)
    } catch (e) {
      misfits?.push(`rev ${f.rev}: ${(e as Error).message}`)
    }
  }
  if (f.derived && f.derived.containers) Object.assign(B.derived.containers, clone(f.derived.containers))
  return B
}
