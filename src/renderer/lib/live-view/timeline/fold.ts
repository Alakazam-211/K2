// The lanes model of a book: tracks in list order, labelled and grouped by
// the pack's derived values (never counted); each track's items in time
// order (sorted on read: the list's order means nothing); markers placed in
// record time. Pure, and cached per book (books never change in place).

import { addrOf } from './adapter'
import { CONT, MED, MKR, PROPS, TLR, UNIT, VIEW, type Book, type KV, type TrackDerived } from './schema'
import { big, durText, framesOf, pairOf, PER_S, rateOf, tcText, type Rate } from './time'

export interface TimelineHead {
  name: string
  rate: Rate | null
  tc: bigint
  doc: KV
  format: KV
  endFrom: string | null
  stacking: string | null
  dropFrame: boolean
  marks: { t: KV; i: KV; k: KV; m: KV }
  /** No rate yet (before the first page): bars sit at a provisional 24 fps. */
  provisional: boolean
}

export interface Marker {
  key: string
  n: number
  name: string
  note: string
  color: string
  dur: bigint
  at: bigint | null
  owner: 'timeline' | 'track' | 'item' | 'gone'
  ownerKey: string | null
  /** Record time (ticks); an item's marker is media time, placed at start + (at − src). */
  rec: bigint | null
  f: number | null
}

export interface Item {
  key: string
  n: number
  kind: string
  name: string
  track: string
  s: bigint
  d: bigint
  e: bigint
  f0: number
  f1: number
  geom: KV | undefined
  src: bigint | null
  off: boolean
  edit: string | null
  media: string | null
  rate: string | null
  retimed: boolean
  tr: [bigint, bigint] | null
  sortAt: bigint
  sortF: number
  markers: Marker[]
}

export interface Track {
  key: string
  position: number
  kind: string
  kindFrom: 'derived' | 'list'
  title: string
  label: string
  labelFrom: 'derived' | null
  listKind: KV
  nid: KV | undefined
  muted: boolean
  locked: boolean
  enabled: boolean
  items: Item[]
  markers: Marker[]
  sortFs: number[]
  maxAfter: number
  maxBefore: number
  clips: number
}

export interface TimelineModel {
  B: Book
  T: TimelineHead
  /** Ticks in a frame. */
  FR: bigint
  fr: (t: bigint | null | undefined) => number
  tracks: Track[]
  byT: Record<string, Track>
  items: Record<string, Item>
  markers: Marker[]
  media: Record<string, Record<string, KV>>
  end: bigint
  endF: number
  count: number
  /** The timeline's timecode at a tick, on the frame grid only. */
  tc: (t: bigint | null | undefined) => string | null
  /** The timecode at a frame (a provisional "12.5s" before the rate is known). */
  tcF: (f: number) => string
  dur: (f: number) => string
}

const CACHE = new WeakMap<Book, TimelineModel>()
const LETTER: Record<string, string> = { video: 'V', audio: 'A' }

/** Stands in for the pack's derived label and kind on books built by hand (the large sample): the view itself never counts. */
export function packDerived(B: Book): Record<string, TrackDerived> {
  const cnt: Record<string, number> = {}
  const out: Record<string, TrackDerived> = {}
  for (const [a, kind] of B.lists[CONT] || []) {
    const L = LETTER[kind ?? ''] || 'O'
    cnt[L] = (cnt[L] || 0) + 1
    out[a] = { kind_label: L + cnt[L], kind: kind ?? undefined }
  }
  return out
}

export function modelOf(B: Book): TimelineModel {
  const hit = CACHE.get(B)
  if (hit) return hit
  const TM = B.maps[TLR] || {}
  const g = (k: string): KV => (TM[k] || {})[''] ?? null
  const rate = rateOf(g('rate'))
  const T: TimelineHead = {
    name: g('name') || '',
    rate,
    tc: big(g('tc')) || 0n,
    doc: g('doc'),
    format: g('format'),
    endFrom: g('end_from') || null,
    stacking: g('stacking') || null,
    dropFrame: g('drop_frame') === '1',
    marks: { t: g('mark/t'), i: g('mark/i'), k: g('mark/k'), m: g('mark/m') },
    provisional: !(rate && rate.frame),
  }
  const FR = T.provisional ? PER_S / 24n : (rate!.frame as bigint)
  const fr = (t: bigint | null | undefined): number => framesOf(t, FR)
  const tp = B.maps[PROPS[CONT]] || {}
  const ip = B.maps[PROPS[UNIT]] || {}
  const kp = B.maps[PROPS[MKR]] || {}
  const mp = B.maps[PROPS[MED]] || {}
  const DC = B.derived?.containers || {}
  const tracks: Track[] = (B.lists[CONT] || []).map(([a, kind], i) => {
    const p = tp[a] || {}
    const d = DC[a] || {}
    return {
      key: a,
      position: i + 1,
      kind: d.kind || kind || '',
      kindFrom: d.kind ? 'derived' : 'list',
      title: p[VIEW.container_title] || '',
      label: d.kind_label || '',
      labelFrom: d.kind_label ? 'derived' : null,
      listKind: kind,
      nid: p.nid,
      muted: p.muted === '1',
      locked: p.locked === '1',
      enabled: p.enabled !== '0',
      items: [],
      markers: [],
      sortFs: [],
      maxAfter: 0,
      maxBefore: 0,
      clips: 0,
    }
  })
  const byT: Record<string, Track> = Object.fromEntries(tracks.map((t) => [t.key, t]))
  const items: Record<string, Item> = {}
  let end = 0n
  ;(B.lists[UNIT] || []).forEach(([a, kind], n) => {
    const p = ip[a] || {}
    const gm = pairOf(p[VIEW.geometry])
    const tr = pairOf(p.tr)
    const ed = VIEW.editable_text
    const s = gm ? gm[0] : 0n
    const d = gm ? gm[1] : 0n
    const it: Item = {
      key: a,
      n,
      kind: kind || '',
      name: p.name || '',
      track: addrOf(CONT, p[VIEW.unit_container_field]),
      s,
      d,
      e: s + d,
      f0: fr(s),
      f1: fr(s + d),
      geom: p.geom,
      src: big(p.src),
      off: p[VIEW.hidden] === '1',
      edit: ed.values[p[ed.field] ?? ''] || null,
      media: p.media ? addrOf(MED, p.media) : null,
      rate: p.rate || null,
      retimed: p.retimed === '1',
      tr,
      sortAt: kind === 'transition' && tr ? s + tr[0] : s,
      sortF: 0,
      markers: [],
    }
    it.sortF = fr(it.sortAt)
    items[a] = it
    const t = byT[it.track]
    if (t) t.items.push(it)
    if (it.e > end) end = it.e
  })
  const tz = (x: Item): number => (x.kind === 'transition' ? 0 : 1)
  for (const t of tracks) {
    t.items.sort((x, y) => (x.sortAt < y.sortAt ? -1 : x.sortAt > y.sortAt ? 1 : tz(x) - tz(y) || x.n - y.n))
    t.sortFs = t.items.map((x) => x.sortF)
    t.maxAfter = t.items.reduce((m, x) => Math.max(m, x.f1 - x.sortF), 0)
    t.maxBefore = t.items.reduce((m, x) => Math.max(m, x.sortF - x.f0), 0)
    t.clips = t.items.filter((x) => x.kind !== 'transition').length
  }
  const markers: Marker[] = (B.lists[MKR] || []).map(([a], n) => {
    const p = kp[a] || {}
    const at = big(p.at)
    const o = p.owner || ''
    let owner: Marker['owner'] = 'timeline'
    let ownerKey: string | null = null
    let rec: bigint | null = at
    if (o) {
      const ti = addrOf(CONT, o)
      const ii = addrOf(UNIT, o)
      if (byT[ti]) {
        owner = 'track'
        ownerKey = ti
      } else if (items[ii]) {
        owner = 'item'
        ownerKey = ii
        const it = items[ii]
        rec = at != null && it.src != null ? it.s + (at - it.src) : null
      } else {
        owner = 'gone'
        rec = null
      }
    }
    const m: Marker = { key: a, n, name: p.name || '', note: p.note || '', color: p.color || '', dur: big(p.dur) || 0n, at, owner, ownerKey, rec, f: rec == null ? null : fr(rec) }
    if (owner === 'item' && ownerKey) items[ownerKey].markers.push(m)
    else if (owner === 'track' && ownerKey) byT[ownerKey].markers.push(m)
    return m
  })
  const media = Object.fromEntries((B.lists[MED] || []).map(([a, kind]) => [a, { kind, ...(mp[a] || {}) }]))
  const M: TimelineModel = {
    B,
    T,
    FR,
    fr,
    tracks,
    byT,
    items,
    markers,
    media,
    end,
    endF: fr(end),
    count: Object.keys(items).length,
    tc: (t) => (T.provisional ? null : tcText(t, T.rate, T.tc)),
    tcF: (f) => (T.provisional ? `${(f / 24).toFixed(f % 24 ? 1 : 0)}s` : (tcText(BigInt(Math.round(f)) * FR, T.rate, T.tc) as string)),
    dur: (f) => durText(f, T.rate ? T.rate.base : 24),
  }
  CACHE.set(B, M)
  return M
}

/** Lanes top to bottom: video stacked upward (the highest V on top), then audio downward, then anything else. */
export function lanesOf(M: TimelineModel): Track[] {
  const v = M.tracks.filter((t) => t.kind === 'video').slice().reverse()
  const a = M.tracks.filter((t) => t.kind === 'audio')
  const o = M.tracks.filter((t) => t.kind !== 'video' && t.kind !== 'audio')
  return v.concat(a, o)
}

/** The items a window of frames shows: a binary search on the time order, never a scan of the doc. */
export function visibleItems(t: Track, f0: number, f1: number): Item[] {
  const A = t.sortFs
  const lo0 = f0 - t.maxAfter
  let lo = 0
  let hi = A.length
  while (lo < hi) {
    const m = (lo + hi) >> 1
    if (A[m] < lo0) lo = m + 1
    else hi = m
  }
  const out: Item[] = []
  for (let i = lo; i < A.length && A[i] < f1 + t.maxBefore; i++) {
    const x = t.items[i]
    if (x.f0 < f1 && x.f1 > f0) out.push(x)
  }
  return out
}

/** live_overlap: two enabled items overlap on a track outside a transition. */
export function overlaps(M: TimelineModel): string[] {
  const bad: string[] = []
  for (const t of M.tracks) {
    const c = t.items.filter((x) => x.kind !== 'transition' && !x.off).sort((x, y) => (x.s < y.s ? -1 : x.s > y.s ? 1 : 0))
    for (let i = 1; i < c.length; i++) if (c[i - 1].e > c[i].s) bad.push(`${t.label}: ${c[i - 1].name} / ${c[i].name}`)
  }
  return bad
}

/**
 * Tracks either side of an outside save, paired as the kit pairs them
 * (timeline 2f12e2b0): by (name, kind), in list order. A track with no
 * partner is gone (or new); a hold on a gone track ends with
 * `gone_outside_save`, and the view names it as it was.
 */
export function pairTracks(M0: TimelineModel, M1: TimelineModel): { pairs: [string, string][]; gone: string[]; added: string[] } {
  const used = new Set<string>()
  const pairs: [string, string][] = []
  const added: string[] = []
  for (const t of M1.tracks) {
    const p = M0.tracks.find((o) => !used.has(o.key) && o.title === t.title && o.kind === t.kind)
    if (p) {
      used.add(p.key)
      pairs.push([p.key, t.key])
    } else added.push(t.key)
  }
  return { pairs, gone: M0.tracks.filter((o) => !used.has(o.key)).map((o) => o.key), added }
}

/** Keyed entries in a book (the kit's 25,000 limit). */
export function entryCount(B: Book): number {
  let n = 0
  for (const l of Object.values(B.lists)) n += l.length
  for (const m of Object.values(B.maps)) for (const e of Object.values(m)) n += Object.keys(e).length
  return n
}
