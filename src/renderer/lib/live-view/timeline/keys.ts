// The timeline's watching keys (the mock's map; no editing keys): Space
// plays and pauses; J, K and L go back, stop and forward, again for 2×, 4×
// and 8×; ← and → step a frame; ↑ and ↓ jump to the agent's edits; Home
// and End; = and - zoom; \ fits; Esc ends a loop. Pure: what a key does to
// your own playback. Any of them takes the monitor over to your playhead
// (T5: that leaves Follow).

import type { LocalPlay } from './monitor-view'

export type WatchKey = 'play' | 'stop' | 'forward' | 'back' | 'frame-' | 'frame+' | 'edit-' | 'edit+' | 'start' | 'end' | 'zoom+' | 'zoom-' | 'fit' | 'escape'

export const KEY_MAP: Readonly<Record<string, WatchKey>> = {
  ' ': 'play',
  k: 'stop',
  K: 'stop',
  l: 'forward',
  L: 'forward',
  j: 'back',
  J: 'back',
  ArrowLeft: 'frame-',
  ArrowRight: 'frame+',
  ArrowUp: 'edit-',
  ArrowDown: 'edit+',
  Home: 'start',
  End: 'end',
  '=': 'zoom+',
  '+': 'zoom+',
  '-': 'zoom-',
  '\\': 'fit',
  Escape: 'escape',
}

/** The quiet key hint's words (its tooltip and accessible name). */
export const keysHint = (agent: string): string =>
  `Keys: Space play or pause · J K L back, stop, forward (again for 2×, 4×, 8×) · ← → a frame · ↑ ↓ ${agent}’s edits · Home End · = - zoom · \\ fit · Esc stop a loop`

export interface KeyContext {
  /** The monitor's frame and whether it's playing now. */
  f: number
  playing: boolean
  /** Play is allowed (not stills only; while preparing, once the next 30 s are ready). */
  canPlay: boolean
  /** The monitor is at the end of the edit. */
  ended: boolean
  endF: number
  /** Where the agent's edits start, in order. */
  edits: readonly number[]
}

/** Your playback after a playback key; null when the key does nothing here (zoom, fit and Esc are the view's). */
export function playbackKey(k: WatchKey, cur: LocalPlay | null, c: KeyContext): LocalPlay | null {
  const f = cur ? cur.f : c.f
  const rate = cur && !cur.ended ? cur.rate : c.playing ? 1 : 0
  const at = (o: Partial<LocalPlay>): LocalPlay => ({ f, rate: 0, ...o })
  const last = Math.max(0, Math.ceil(c.endF) - 1)
  switch (k) {
    case 'play':
      if (c.ended && !rate) return c.canPlay ? at({ f: 0, rate: 1 }) : null // at the end, Space plays again from the start
      if (!rate && !c.canPlay) return null // preparing: nothing until the next 30 s are ready
      return at({ rate: rate ? 0 : 1 })
    case 'stop':
      return at({ rate: 0 })
    case 'forward':
      if (!c.canPlay) return null
      return at({ f: c.ended ? 0 : f, rate: rate > 0 ? Math.min(8, rate * 2) : 1 })
    case 'back':
      if (!c.canPlay) return null
      return at({ rate: rate < 0 ? Math.max(-8, rate * 2) : -1 })
    case 'frame-':
      return at({ f: Math.max(0, Math.round(f) - 1) })
    case 'frame+':
      return at({ f: Math.min(last, Math.round(f) + 1) })
    case 'start':
      return at({ f: 0 })
    case 'end':
      return at({ f: last })
    case 'edit-': {
      const p = c.edits.filter((x) => x < f - 0.5).pop()
      return p == null ? null : at({ f: p })
    }
    case 'edit+': {
      const n = c.edits.find((x) => x > f + 0.5)
      return n == null ? null : at({ f: n })
    }
    default:
      return null
  }
}

/** One step of playback: `dt` seconds at the rate, stopping at either end (the end says so). */
export function advance(cur: LocalPlay, dt: number, fps: number, endF: number): LocalPlay {
  if (!cur.rate || cur.ended) return cur
  const f = cur.f + dt * fps * cur.rate
  if (cur.loop && f >= cur.loop.f1) return { ...cur, f: cur.loop.f0 }
  if (f >= endF - 1) return { ...cur, f: Math.max(0, endF - 1), rate: 0, ended: true }
  if (f <= 0) return { ...cur, f: 0, rate: 0 }
  return { ...cur, f }
}
