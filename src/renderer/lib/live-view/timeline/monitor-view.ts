// What the monitor shows, worked out from the folded state, whose playhead
// it follows and your own playback: one state from the video player design
// section (playing, paused on the exact frame or the nearest picture,
// seeking, loading the first frame, waiting mid-play, the end, preparing
// video, stills only for you, and the cards: no preview, offline, nest, not
// a video, the gap), the one quiet note bottom-left, the clip's label, the
// people watching the same spot, what to paint, and the transport's mode
// word and Play. Pure: Monitor.tsx draws it; T8 swaps the sample's facts
// for K2 Render's.

import type { TimelineModel } from './fold'
import type { MonitorPaint } from './monitor-source'
import { DEFAULT_PV, pictureAt, playReady, prepOf, sameSpot, stillAt, stillNote, type MediaFacts, type Picture, type PreviewState, type Prep } from './picture'
import { headOf } from './playheads'
import type { SimState } from './sim'
import { plural } from './wording'

/** Your own playback once you take over (play, a key, a scrub): the frame and the rate (0 paused; negative plays back). */
export interface LocalPlay {
  f: number
  rate: number
  /** Playback ran into the end of the edit. */
  ended?: boolean
  /** "Skipped to <clip>" after a card's Skip. */
  skipped?: string | null
  /** Looping a change: from 1 s before its span to 1 s after (loopRange), over and over. */
  loop?: { f0: number; f1: number; who: string; verb: string | null } | null
}

export type MonitorState = 'none' | 'loading' | 'playing' | 'paused' | 'seeking' | 'waiting' | 'ended' | 'preparing' | 'stills' | 'no_preview' | 'offline' | 'nest' | 'nomedia' | 'gap'

export interface MonitorHead {
  /** Whose playhead: you, or the person you follow. */
  id: string
  name: string
  colour: string
  self: boolean
  f: number
  playing: boolean
  rate: number
}

export interface MonitorNote {
  text: string
  /** Fades in after 300 ms (anything shorter shows nothing new). */
  late: boolean
  spin: boolean
  /** The end's "Play from the start". */
  restart: boolean
}

export interface MonitorCard {
  kind: 'preparing' | 'no_preview' | 'offline' | 'nest' | 'nomedia'
  title: string
  line: string
  /** Not a video: "Image" or "Audio". */
  kindWord?: string
  /** Skip to the next clip (a frame), where there is one. */
  next: number | null
}

export interface MonitorView {
  state: MonitorState
  /** The paused frame is the exact one (no note). */
  exact: boolean
  head: MonitorHead
  picture: Picture | null
  label: { lane: string; name: string } | null
  note: MonitorNote | null
  /** Preparing video: the count, the hold when playback reaches a clip that isn't ready (the design section's "Preparing this clip…"), and the clips that can't be previewed. */
  prep: { text: string; done: boolean; hold: string | null; cant: { key: string; name: string; f: number }[] } | null
  card: MonitorCard | null
  gap: { line: string; next: number | null } | null
  /** The small spinner in the picture's centre while the first frame loads. */
  spinner: boolean
  paint: MonitorPaint
  spots: { id: string; name: string; colour: string }[]
  mode: { word: 'Plays cuts only' | 'Stills'; title: string }
  /** The monitor is playing (yours, the scenario's, or the followed playhead's). */
  playing: boolean
  canPlay: boolean
  playTip: string
  pv: PreviewState
  prepState: Prep | null
}

export const CUTS_TITLE = 'The preview plays the top video track and switches clips at the cuts: no titles, effects or transitions.'
export const STILLS_DECODE = 'This computer can’t play the preview video, so you see stills.'
export const STILLS_NOPLAYER = 'There’s no player yet, so you see stills.'
export const PREP_PLAY_TIP = 'Playback starts when the next part of the video is ready'

const NONE: MonitorPaint = { how: 'none', mi: null, srcF: null, inF: null }

/** The monitor's playhead: yours once you take over; else whoever `whose` names (T5: whom you follow); else yours. */
export function monitorHead(S: SimState, M: TimelineModel, whose: string | null | undefined, local: LocalPlay | null, now: number): MonitorHead {
  const me = S.peers[S.self]
  const mine = { id: S.self, name: 'You', colour: 'var(--color-accent)', self: true }
  if (local) return { ...mine, f: local.f, playing: local.rate !== 0 && !local.ended, rate: local.ended ? 0 : local.rate }
  const p = whose && whose !== S.self ? S.peers[whose] : undefined
  const h = p ? headOf(M, p, now) : null
  if (p && h) return { id: p.handle, name: p.name, colour: p.colour || '#888', self: false, f: h.f, playing: h.playing, rate: h.rate }
  const hm = headOf(M, me, now)
  return { ...mine, f: hm ? hm.f : 0, playing: false, rate: 0 }
}

const nextClipAfter = (M: TimelineModel, f: number): number | null => {
  let n: number | null = null
  for (const t of M.tracks) if (t.kind === 'video') for (const x of t.items) if (x.kind === 'clip' && x.f0 > f && (n == null || x.f0 < n)) n = x.f0
  return n
}

export interface MonitorInput {
  S: SimState
  M: TimelineModel
  facts: MediaFacts
  now: number
  /** Whose playhead the monitor shows; yours when unset (T5 wires Follow). */
  whose?: string | null
  local: LocalPlay | null
  /** Who else is in the file (the same-spot chips), with their colours as the view shows them. */
  people?: readonly { id: string; name: string; colour: string; self?: boolean }[]
}

export function monitorView(i: MonitorInput): MonitorView {
  const { S, M, facts, local } = i
  const head = monitorHead(S, M, i.whose, local, i.now)
  const f = head.f
  const pv0 = S.pv ?? DEFAULT_PV
  const P = prepOf(M, pv0, facts)
  const stillsMode = pv0.player === 'still'
  // Your own playback replaces the scenario's play state; the rest (the player, prep, the sources) stays.
  let mode = local ? (local.ended ? 'ended' : local.rate ? 'play' : 'pause') : pv0.mode
  let why = local ? null : (pv0.why ?? null)
  const quick = local ? false : !!pv0.quick
  const playing = !stillsMode && (local ? head.playing : mode === 'stall' || mode === 'ended' ? false : (!head.self && head.playing) || (mode === 'play' && pv0.player === 'cut'))
  const still = stillsMode || (!!P && !playing)
  const p = still && !P ? stillAt(M, f, pv0, facts) : pictureAt(M, f, pv0, facts)
  // Playing into a clip that isn't ready yet holds in "Preparing this clip…".
  if (local && playing && P && p.kind === 'clip' && p.it && !P.isReady(p.it.key)) {
    mode = 'stall'
    why = 'prep'
  }
  const pv: PreviewState = { ...pv0, mode, why: why ?? pv0.why ?? null }
  const live = S.tier === 'live' && !S.loading
  const canPlay = !stillsMode && (!P || playReady(M, f, pv0, facts) || playing)
  const playTip = stillsMode ? (pv0.why === 'decode' ? STILLS_DECODE : STILLS_NOPLAYER) : !canPlay ? PREP_PLAY_TIP : mode === 'ended' ? 'Play from the start (Space)' : playing ? 'Pause (Space or K)' : 'Play (Space or L)'
  const modeWord = stillsMode ? { word: 'Stills' as const, title: pv0.why === 'decode' ? STILLS_DECODE : STILLS_NOPLAYER } : { word: 'Plays cuts only' as const, title: CUTS_TITLE }
  const base = { head, playing: playing && mode !== 'stall', exact: false, label: null, note: null, prep: null, card: null, gap: null, spinner: false, spots: [], mode: modeWord, canPlay, playTip, pv, prepState: P }
  if (!live || !M.count) return { ...base, state: 'none', picture: null, paint: NONE, canPlay: false }

  const it = p.it
  const label = it && p.t ? { lane: p.t.label, name: it.name } : null
  const next = (x: { f1: number }): number => Math.min(M.endF - 1, x.f1)

  // What to paint (black behind a card or the gap).
  let paint: MonitorPaint = NONE
  const prepCard = !!(P && p.kind === 'clip' && it && !P.isReady(it.key))
  if (p.kind === 'clip' && !prepCard) {
    const at = { mi: p.mi ?? null, inF: p.inF ?? null }
    const s = p as ReturnType<typeof stillAt>
    if (P && !playing) paint = { how: 'poster', ...at, srcF: null }
    else if (still) paint = s.from === 'tile' || s.from === 'head' ? { how: 'tile', ...at, srcF: s.stillF ?? null } : { how: 'poster', ...at, srcF: null }
    else if (mode === 'first') paint = { how: 'dim', ...at, srcF: null }
    else if (mode === 'seek' && !quick) paint = { how: 'soft', ...at, srcF: p.srcF ?? null }
    else paint = { how: 'frame', ...at, srcF: p.srcF ?? null }
  }

  // The card in the picture's place, each with a next step where one exists.
  let card: MonitorCard | null = null
  let gap: MonitorView['gap'] = null
  if (prepCard && it) card = { kind: 'preparing', title: it.name, line: 'Preparing this clip’s preview', next: null }
  else if (p.kind === 'nopreview' && it) card = { kind: 'no_preview', title: it.name, line: 'This clip can’t be previewed.', next: next(it) }
  else if (p.kind === 'offline' && it) card = { kind: 'offline', title: it.name, line: 'This clip’s media isn’t available right now.', next: next(it) }
  else if (p.kind === 'nest' && it) card = { kind: 'nest', title: it.name, line: 'A nested timeline: the preview shows cuts at the top level only.', next: next(it) }
  else if (p.kind === 'nomedia' && it) card = { kind: 'nomedia', title: it.name, line: '', kindWord: p.mkind, next: next(it) }
  else if (p.kind === 'gap') gap = { line: 'Gap: nothing on the video tracks here', next: nextClipAfter(M, f) }

  // One quiet note, bottom-left.
  let note: MonitorNote | null = null
  const n = (text: string, late = true, spin = false, restart = false): MonitorNote | null => (text ? { text, late, spin, restart } : null)
  if (P) note = null // the prep box says it, so the corner holds one thing
  else if (mode === 'ended') note = n('End of the edit', false, false, true)
  else if (mode === 'stall') note = n(pv.why === 'prep' ? 'Preparing this clip…' : 'Waiting for video…', true, true)
  else if (stillsMode && pv0.why === 'decode') note = n('Stills only: this computer can’t play the preview', false)
  else if (still) note = p.kind === 'clip' ? n(stillNote(M, p as ReturnType<typeof stillAt>)) : null
  else if (mode === 'seek') note = quick ? null : n('Seeking…', true, true)
  else if (mode === 'first') note = p.kind === 'clip' ? n('Loading…') : null

  const prep = P
    ? {
        done: P.done && !!pv0.prep?.done,
        hold: mode === 'stall' ? 'Preparing this clip…' : null,
        text: P.done && pv0.prep?.done ? 'Video ready' : `Preparing video · ${P.n} of ${P.total} previewable clips ready`,
        cant: P.cant.map((x) => ({ key: x.key, name: x.name, f: x.f0 })),
      }
    : null

  // The end wins: the last frame holds (whatever it shows) and Play goes back to the start.
  const stateOf = (): MonitorState => {
    if (mode === 'ended') return 'ended'
    if (card && card.kind !== 'preparing') return card.kind
    if (gap) return 'gap'
    if (mode === 'stall') return 'waiting'
    if (P && !playing) return 'preparing'
    if (stillsMode && pv0.why === 'decode') return 'stills'
    if (still) return 'paused'
    if (mode === 'seek') return 'seeking'
    if (mode === 'first') return 'loading'
    return playing ? 'playing' : 'paused'
  }
  const state = stateOf()
  const exact = state === 'paused' && !still

  const skip = [S.self, head.id]
  const near = sameSpot(M, f, Object.values(S.peers), skip, pv0, facts)
  const spots = near.map((q) => {
    const v = i.people?.find((x) => x.id === q.handle)
    return { id: q.handle, name: v?.name ?? q.name, colour: v?.colour ?? q.colour ?? '#888' }
  })

  return { ...base, state, exact, picture: p, label, note, prep, card, gap, spinner: mode === 'first' && !still && p.kind === 'clip', paint, spots }
}

/** "2 other clips can’t be previewed". */
export const cantLine = (n: number): string => `${plural(n, 'other clip')} can’t be previewed`
