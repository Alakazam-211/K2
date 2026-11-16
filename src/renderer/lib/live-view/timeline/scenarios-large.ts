// The large timeline: 10 tracks × 220 items, seeded, then an agent's verbs
// across it. Lanes virtualise; the overview marks where changes land.

import { builtOf, geomSet, mkFrame } from './diff'
import { modelOf, packDerived, type Item, type Track } from './fold'
import type { Book, TimelineFrame } from './schema'
import { ensurePeer, step, type Scenario, type SimState } from './sim'
import { ATLAS, blank, FRT, ptr, SAM, SELF, vpt } from './world'

function mulberry(a: number): () => number {
  return () => {
    a |= 0
    a = (a + 0x6d2b79f5) | 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

export const PER_TRACK = 220
const LETTER: Record<string, string> = { video: 'V', audio: 'A' }

function buildLargeBook(): Book {
  const R = mulberry(2026)
  const T = (f: number): string => String(BigInt(f) * FRT)
  const pad3 = (n: number): string => String(n).padStart(3, '0')
  const B: Book = { rev: 500, lists: { tracks: [], items: [], markers: [], media: [] }, hidden: { tracks: [], items: [], markers: [], media: [] }, maps: { track_props: {}, item_props: {}, marker_props: {}, media_props: {}, timeline: {} }, derived: { containers: {} } }
  const TR = [
    ['video', 'Interviews', 'Interview'],
    ['video', 'B-roll', 'B-roll'],
    ['video', 'B-roll 2', 'Cutaway'],
    ['video', 'Graphics', 'Graphic'],
    ['video', 'Titles', 'Title'],
    ['video', 'Fixes', 'Fix'],
    ['audio', 'Dialogue', 'Dialogue'],
    ['audio', 'Room tone', 'Room tone'],
    ['audio', 'Effects', 'Effect'],
    ['audio', 'Music', 'Music cue'],
  ]
  const cnt: Record<string, number> = {}
  TR.forEach(([kind, name], i) => {
    const L = LETTER[kind]
    cnt[L] = (cnt[L] || 0) + 1
    const a = '@trk:' + (i + 1)
    B.lists.tracks.push([a, kind])
    B.maps.track_props[a] = { name, muted: '0', locked: '0', edit: '1', nid: L + cnt[L] }
  })
  for (let m = 1; m <= 60; m++) {
    const a = '@med:' + m
    B.lists.media.push([a, 'file'])
    B.maps.media_props[a] = { name: `Reel ${pad3(m)}.mov`, file: `Reel ${pad3(m)}.mov`, nid: 'm:reel' + pad3(m), start: '0', dur: T(48000), rate: '24/1' }
  }
  let n = 0
  TR.forEach(([, , word], ti) => {
    let pos = Math.floor(R() * 48)
    for (let j = 1; j <= PER_TRACK; j++) {
      const d = 36 + Math.floor(R() * R() * 560)
      const a = '@item:' + ++n
      const gen = ti === 4
      B.lists.items.push([a, gen ? 'generator' : 'clip'])
      B.maps.item_props[a] = { track: 't' + (ti + 1), geom: `${T(pos)},${T(d)}`, name: `${word} ${pad3(j)}`, off: R() < 0.03 ? '1' : '0', edit: '1', src: T(Math.floor(R() * 40000)), media: 'm' + (1 + Math.floor(R() * 60)), rate: '24/1' }
      pos += d + (R() < 0.4 ? 12 + Math.floor(R() * 96) : 0)
    }
  })
  for (let k = 1; k <= 120; k++) {
    const a = '@mk:' + k
    B.lists.markers.push([a, 'marker'])
    B.maps.marker_props[a] = { owner: '', at: T(k * 420 + Math.floor(R() * 200)), dur: '0', name: `Note ${k}`, note: '', color: ['RED', 'GREEN', 'BLUE', 'YELLOW', 'PURPLE'][k % 5] }
  }
  B.derived = { containers: packDerived(B) }
  const head: Record<string, string> = { name: 'Long edit', tc: T(86400), format: 'otio', version: 'Timeline.1', rate: '24/1', doc: '9e2f41c08b7d5a36', 'mark/t': '10', 'mark/i': String(n), 'mark/k': '120', 'mark/m': '60' }
  for (const [k, v] of Object.entries(head)) B.maps.timeline[k] = { '': v }
  return B
}

export interface Large {
  B0: Book
  frames: TimelineFrame[]
  P0: number
  picks: { mv: Item; tr: Item; dl: Item; gp: Item; of: Item; nk: string }
}

function buildLargeWorld(): Large {
  const B0 = buildLargeBook()
  const M = modelOf(B0)
  const frames: TimelineFrame[] = []
  const after0 = (t: Track, f0: number, gap: number): Item | undefined => t.items.find((x, i) => x.kind !== 'transition' && x.f0 >= f0 && t.items[i + 1] && t.items[i + 1].f0 - x.f1 >= gap)
  const after = (t: Track, f0: number, gap: number): Item => (after0(t, f0, gap) || after0(t, 0, gap)) as Item
  const P0 = Math.round(M.endF * 0.5)
  const last = (k: string): number => M.byT[k].items[M.byT[k].items.length - 1].f0
  const ticks = (f: number): string => String(BigInt(Math.round(f)) * FRT)
  let B = B0
  let rev = 500
  const push = (verb: string, mut: (X: Book) => void): void => {
    rev++
    const f = mkFrame(B, rev, 400000 + (rev - 500) * 2500, ATLAS, verb, mut, { turn: 't:3c90de11a4b7f250.2', client: 4711 })
    frames.push(f)
    B = builtOf(f)!.after
  }
  const mv = after(M.byT['@trk:2'], P0, 60)
  push('clip move', (X) => geomSet(X, mv.key, ticks(mv.f0 + 48), String(mv.d)))
  const tr = after(M.byT['@trk:7'], P0 + 600, 0)
  push('clip trim', (X) => geomSet(X, tr.key, String(tr.s), ticks(tr.f1 - tr.f0 - 12)))
  const dl = after(M.byT['@trk:4'], P0 + 1200, 0)
  push('clip delete', (X) => {
    X.lists.items = X.lists.items.filter((x) => x[0] !== dl.key)
    X.hidden.items.push([dl.key, 'clip'])
  })
  const gp = after(M.byT['@trk:6'], P0 - 400, 100)
  const nk = '@item:' + (M.count + 1)
  push('clip add', (X) => {
    X.lists.items.push([nk, 'clip'])
    X.maps.item_props[nk] = { track: 't6', geom: `${ticks(gp.f1 + 12)},${ticks(72)}`, name: 'Fix 221', off: '0', edit: '1', src: ticks(1200), media: 'm7', rate: '24/1' }
    X.maps.timeline['mark/i'][''] = String(M.count + 1)
  })
  const of = after(M.byT['@trk:3'], Math.round(last('@trk:3') * 0.92), 0)
  push('clip enable', (X) => {
    X.maps.item_props[of.key].off = '1'
  })
  return { B0, frames, P0, picks: { mv, tr, dl, gp, of, nk } }
}

export const LARGE = buildLargeWorld()

export function buildLarge(): Scenario {
  const M = modelOf(LARGE.B0)
  const fr = (f: TimelineFrame) => ({ kind: 'frame' as const, frame: f })
  const base = (): SimState => {
    const S = blank(LARGE.B0)
    ensurePeer(S, SELF)
    ensurePeer(S, ATLAS)
    Object.assign(S.peers[SELF], { pointer: ptr(LARGE.P0 + 300, null) })
    Object.assign(ensurePeer(S, SAM), { viewport: vpt(Math.round(LARGE.P0 * 0.4), Math.round(LARGE.P0 * 0.4) + 1400, '@trk:3', '@trk:8'), pointer: ptr(Math.round(LARGE.P0 * 0.4) + 500, '@trk:2') })
    Object.assign(S.peers[ATLAS], { focus: { root: 'tracks', key: '@trk:2' } })
    return S
  }
  const c = LARGE.picks.mv.f0
  return {
    id: 'large',
    group: 'Large timeline',
    title: `Large timeline: ${M.count.toLocaleString('en')} items on ${M.tracks.length} tracks, virtualised`,
    base,
    steps: [
      step('fit', [{ kind: 'zoom', fit: true }], `The whole ${M.tcF(M.endF).slice(0, 8)} edit in view. Lanes draw only what the window shows.`),
      step('zoom in', [{ kind: 'zoom', ppf: 1.1, center: c }], 'Zoomed to Atlas’s lane. A few dozen bars are drawn.'),
      step('move', [fr(LARGE.frames[0])], 'Atlas moves a B-roll clip 2 s later.'),
      step('trim', [fr(LARGE.frames[1])], 'Atlas trims a dialogue clip’s end.'),
      step('lift', [fr(LARGE.frames[2])], 'Atlas removes a graphic: a lift, leaving a gap.'),
      step('add', [fr(LARGE.frames[3])], 'Atlas adds a clip into a gap on V6: the key is minted past the mark.'),
      step('far away', [fr(LARGE.frames[4])], 'Atlas turns off a clip further along. Following scrolls there.'),
      step('fit again', [{ kind: 'zoom', fit: true }], 'Zoomed out: the overview shows where the five changes landed.'),
    ],
  }
}
