// What the monitor and the lanes' pictures show, worked out from the model
// and what K2 Render holds per source (`MediaFacts`, by file name): the
// cut-only picture at a record time, where the player switches source,
// still-at-playhead, lane tiles, preparing video, loops, and who's watching
// the same spot. Pure: T4 draws it, T8 swaps the facts for K2 Render's.

import { lanesOf, visibleItems, type Item, type TimelineModel, type Track } from './fold'
import { ptrF } from './playheads'
import type { KV, Pointer } from './schema'
import { big, dfTc, PER_S, tcText } from './time'

/** K2 Render's facts for one source. */
export interface SourceFacts {
  kind: string
  hash?: string
  proxy?: { ladder_px?: number[]; audio?: string } | null
  poster?: { px: number } | null
  filmstrip?: { tile_px: number; every_s: number } | null
  [k: string]: unknown
}
export type MediaFacts = Record<string, SourceFacts>

/** The filmstrip's grid (K2 Render: 160 px tiles every 2 s). */
export const FILMSTRIP_EVERY_S = 2

/** The viewer's preview state (local to this client, never on the wire). */
export interface PreviewState {
  player: 'cut' | 'still'
  mode: string
  rung: number
  pending: string[]
  nopreview: string[]
  offline: string[]
  prep?: { ready: string[] | 'all'; cant?: string[]; fresh?: string[]; hl?: boolean; done?: boolean }
  why?: string | null
  quick?: boolean
  lanes?: 'S' | 'M' | 'L'
  pic?: string
  scroll?: 'page' | 'centre'
  at?: number
}
export const DEFAULT_PV: PreviewState = { player: 'cut', mode: 'pause', rung: 1920, pending: [], nopreview: [], offline: [] }

export interface MediaInfo extends SourceFacts {
  addr: string
  file: KV
  name: KV
  origin: KV
  dropFrame: boolean
  mrate: KV
  mstart: KV
}

export function mediaInfo(M: TimelineModel, addr: string | null | undefined, facts: MediaFacts): MediaInfo | null {
  const md = addr ? M.media[addr] : undefined
  if (!md) return null
  return { addr: addr as string, file: md.file ?? null, name: md.name ?? null, origin: md.origin || null, dropFrame: md.drop_frame === '1', mrate: md.rate || null, mstart: md.start || null, ...(facts[md.file ?? ''] || { kind: 'unknown' }) }
}

/** The source frame at a record frame: the clip's src in frames plus the offset into the clip. */
export const srcFrameAt = (M: TimelineModel, it: Item, f: number): number | null => (it.src == null ? null : Number(it.src / M.FR) + Math.floor(f - it.f0 + 1e-9))
export const inSrcF = (M: TimelineModel, it: Item): number | null => (it.src == null ? null : Number(it.src / M.FR))
export const gridF = (M: TimelineModel): number => FILMSTRIP_EVERY_S * (M.T.rate ? M.T.rate.base : 24)
export const srcTc = (M: TimelineModel, sf: number | null): string => (sf == null ? '' : tcText(BigInt(sf) * M.FR, M.T.rate, 0n) || '')

export interface Picture {
  kind: 'clip' | 'gap' | 'nest' | 'nomedia' | 'offline' | 'nopreview'
  f: number
  skipped: { it: Item; why: 'off' | 'title' }[]
  it?: Item
  t?: Track
  mkind?: string
  mi?: MediaInfo | null
  media?: string | null
  srcF?: number | null
  inF?: number | null
}

/** Cut-only: the top enabled video track wins; titles and turned-off clips are passed over; a nest shows a card. */
export function pictureAt(M: TimelineModel, f: number, pv: PreviewState | null | undefined, facts: MediaFacts): Picture {
  const p = pv || DEFAULT_PV
  const skipped: Picture['skipped'] = []
  for (const t of lanesOf(M).filter((x) => x.kind === 'video')) {
    if (t.enabled === false) continue
    const it = visibleItems(t, f, f + 1e-6).find((x) => x.kind !== 'transition' && x.f0 <= f && f < x.f1)
    if (!it) continue
    if (it.off) {
      skipped.push({ it, why: 'off' })
      continue
    }
    if (it.kind === 'generator') {
      skipped.push({ it, why: 'title' })
      continue
    }
    const base = { it, t, skipped, f }
    if (it.kind === 'nest') return { kind: 'nest', ...base }
    const mi = mediaInfo(M, it.media, facts)
    if (!mi || mi.kind !== 'video') return { kind: 'nomedia', mkind: mi && mi.kind === 'image' ? 'Image' : mi && mi.kind === 'audio' ? 'Audio' : 'No picture', mi, ...base }
    const out = { media: it.media, mi, srcF: srcFrameAt(M, it, f), inF: inSrcF(M, it), ...base }
    if ((p.offline || []).includes(it.media as string)) return { kind: 'offline', ...out }
    if ((p.nopreview || []).includes(it.media as string)) return { kind: 'nopreview', ...out }
    return { kind: 'clip', ...out }
  }
  return { kind: 'gap', skipped, f }
}

export const picKey = (p: Picture): string => p.kind + ':' + (p.it ? p.it.key : '')

/** Where cut-only playback switches source between f0 and f1: every place the picture changes clip. */
export function cutsIn(M: TimelineModel, f0: number, f1: number, pv: PreviewState | null | undefined, facts: MediaFacts): number[] {
  const at = new Set<number>()
  for (const t of M.tracks.filter((x) => x.kind === 'video'))
    for (const x of visibleItems(t, f0, f1)) {
      if (x.kind === 'transition') continue
      for (const b of [x.f0, x.f1]) if (b > f0 && b < f1) at.add(b)
    }
  return [...at].sort((a, b) => a - b).filter((b) => picKey(pictureAt(M, b - 0.5, pv, facts)) !== picKey(pictureAt(M, b, pv, facts)))
}

export interface Tile {
  i: number
  srcF: number
  key: string
  head?: boolean
  tail?: boolean
}

/** Lane tiles: the head decoded at the in-point (keyed by media hash, source time, width); the rest the source's filmstrip on its 2 s grid. */
export function laneTiles(M: TimelineModel, it: Item, tileF: number, width: number, facts: MediaFacts): Tile[] {
  const mi = mediaInfo(M, it.media, facts)
  if (!mi || mi.kind !== 'video' || it.src == null) return []
  const g = gridF(M)
  const s0 = inSrcF(M, it) as number
  const out: Tile[] = [{ i: 0, srcF: s0, head: true, key: `${mi.hash}|${s0 * Number(M.FR)}|${width}` }]
  const n = Math.ceil((it.f1 - it.f0) / tileF)
  for (let i = 1; i < n; i++) {
    const s = Math.floor((s0 + i * tileF) / g) * g
    out.push({ i, srcF: Math.max(s, s0), head: s < s0, key: s < s0 ? out[0].key : `${mi.hash}|${s * Number(M.FR)}|160` })
  }
  return out
}

/** Lane heights S, M, L (video / audio px). */
export const LANE_SIZES = { S: { video: 30, audio: 26 }, M: { video: 58, audio: 48 }, L: { video: 92, audio: 72 } } as const
export const PIC_MIN = 28
export const STRIP_MIN = 50
export const WAVE_MIN = 30

/** Clip pictures by lane height: auto is a filmstrip from 50 px, the head from 28 px, none below; a filmstrip on a short lane falls back to the head. */
export function picModeFor(h: number, pref?: string | null): string {
  if (pref === 'off' || h < PIC_MIN) return 'off'
  const m = pref === 'auto' || !pref ? (h < STRIP_MIN ? 'head' : 'strip') : pref
  return h < STRIP_MIN && m === 'strip' ? 'head' : m
}

/** The tiles a clip shows in a mode; the tail is decoded at the out-point; a clip narrower than two tiles shows its head only. */
export function pictureTiles(M: TimelineModel, it: Item, mode: string, tileF: number, width: number, facts: MediaFacts): Tile[] {
  if (mode === 'off') return []
  const all = laneTiles(M, it, tileF, width, facts)
  if (!all.length) return []
  if (mode === 'strip') return all
  if (mode === 'head' || it.f1 - it.f0 < 2 * tileF) return [all[0]]
  const mi = mediaInfo(M, it.media, facts) as MediaInfo
  const sT = (inSrcF(M, it) as number) + Math.max(0, Math.round(it.f1 - it.f0) - 1)
  return [all[0], { i: -1, srcF: sT, tail: true, key: `${mi.hash}|${sT * Number(M.FR)}|${width}` }]
}

export type Still = Picture & { from: 'tile' | 'head' | 'poster' | null; stillF?: number | null; behind?: number | null }

/** Still at playhead: the filmstrip tile at or before the playhead's source time; the head tile before the first grid tile; the poster only until the head tile exists. */
export function stillAt(M: TimelineModel, f: number, pv: PreviewState | null | undefined, facts: MediaFacts): Still {
  const p = pictureAt(M, f, pv, facts)
  if (p.kind !== 'clip') return { from: null, ...p }
  const g = gridF(M)
  const sf = p.srcF as number
  const inF = p.inF as number
  const s = Math.floor(sf / g) * g
  if (s >= inF) return { ...p, from: 'tile', stillF: s, behind: sf - s }
  const headReady = !(pv?.pending || []).includes((p.it as Item).key)
  return headReady ? { ...p, from: 'head', stillF: inF, behind: sf - inF } : { ...p, from: 'poster', stillF: null, behind: null }
}

/** The note a still carries: none for the exact frame; "Nearest picture: 1.4 s before"; "Clip’s poster". */
export function stillNote(M: TimelineModel, s: Still | null): string {
  if (!s || s.kind !== 'clip') return ''
  if (s.from === 'poster') return 'Clip’s poster'
  if (!s.behind) return ''
  return `Nearest picture: ${(s.behind / (M.T.rate ? M.T.rate.base : 24)).toFixed(1)} s before`
}

export interface LoopSpan {
  f0: number
  f1: number
  who?: string | null
  verb?: string | null
}

/** A loop over a change: from 1 s before its span to 1 s after (the lead-in shaded). */
export function loopRange(M: TimelineModel, span: LoopSpan): { f0: number; f1: number; s0: number; s1: number; who: string | null; verb: string | null } {
  const sec = M.T.rate ? M.T.rate.base : 24
  return { f0: Math.max(0, span.f0 - sec), f1: Math.min(M.endF, span.f1 + sec), s0: span.f0, s1: span.f1, who: span.who || null, verb: span.verb || null }
}

/** The video clips that can be previewed. */
export const vclips = (M: TimelineModel, facts: MediaFacts): Item[] =>
  lanesOf(M)
    .filter((t) => t.kind === 'video')
    .flatMap((t) => t.items.filter((x) => x.kind === 'clip' && mediaInfo(M, x.media, facts)?.kind === 'video'))

export interface Prep {
  all: Item[]
  pool: Item[]
  cant: Item[]
  ready: Item[]
  n: number
  total: number
  done: boolean
  isReady: (k: string) => boolean
  fresh: string[]
}

/** Preparing video: per-clip readiness; clips that can't be previewed never count against ready. */
export function prepOf(M: TimelineModel, pv: PreviewState | null | undefined, facts: MediaFacts): Prep | null {
  if (!pv || !pv.prep) return null
  const P = pv.prep
  const all = vclips(M, facts)
  const cantK = P.cant || []
  const cant = all.filter((x) => cantK.includes(x.key))
  const pool = all.filter((x) => !cantK.includes(x.key))
  const isIn = (k: string): boolean => P.ready === 'all' || (P.ready || []).includes(k)
  const ready = pool.filter((x) => isIn(x.key))
  return { all, pool, cant, ready, n: ready.length, total: pool.length, done: ready.length === pool.length, isReady: (k) => isIn(k) || cantK.includes(k), fresh: P.fresh || [] }
}

/** Play enables once the next 30 s of record time after the playhead is ready. */
export function playReady(M: TimelineModel, f: number, pv: PreviewState | null | undefined, facts: MediaFacts): boolean {
  const P = prepOf(M, pv, facts)
  if (!P) return true
  const sec = M.T.rate ? M.T.rate.base : 24
  const f1 = Math.min(M.endF, f + 30 * sec)
  const cuts = [f, ...cutsIn(M, f, f1, pv, facts), f1]
  for (let i = 0; i < cuts.length - 1; i++) {
    const p = pictureAt(M, (cuts[i] + cuts[i + 1]) / 2, pv, facts)
    if (p.kind === 'clip' && !P.isReady((p.it as Item).key)) return false
  }
  return true
}

export interface SpotPeer {
  handle: string
  pointer: Pointer | null
  away: boolean
}

/** Who's watching the same spot as the monitor: a playhead on the same picture, or within a second of it. */
export function sameSpot<P extends SpotPeer>(M: TimelineModel, f: number, peers: P[], skip: string[], pv: PreviewState | null | undefined, facts: MediaFacts): P[] {
  const k = picKey(pictureAt(M, f, pv, facts))
  const sec = M.T.rate ? M.T.rate.base : 24
  return peers
    .filter((p) => !skip.includes(p.handle) && p.pointer && !p.away)
    .filter((p) => {
      const g = ptrF(M, p.pointer)
      return g != null && (Math.abs(g - f) <= sec || (k !== 'gap:' && picKey(pictureAt(M, g, pv, facts)) === k))
    })
}

/** A drop-frame source's timecode at the picture: the source start plus the time into the clip, at 30000/1001, in the ";" form. */
export function dfSrcTc(M: TimelineModel, p: Picture): string | null {
  const mi = p.mi
  if (!mi || !mi.dropFrame) return null
  const st = big(mi.mstart)
  const startS = st != null ? Number(st) / Number(PER_S) : 0
  const intoS = (p.srcF || 0) / (M.T.rate ? M.T.rate.fps : 24)
  return dfTc(Math.floor(((startS + intoS) * 30000) / 1001))
}
