// The transport under the monitor, pure: whose playhead it is in words
// (or "Skipped to …", the speed while shuttling), where the agent's edits
// start (↑ and ↓, previous and next edit), and the scrub bar of the whole
// edit: each change as a span, timeline markers, everyone's playhead and
// the monitor's own.

import type { TimelineModel } from './fold'
import { bracketsOf, playheadsOf } from './lanes-view'
import type { LocalPlay, MonitorView } from './monitor-view'
import type { SimState } from './sim'
import { VERB_WORD } from './wording'

/** The scrub bar spans the whole edit (at least 2 s). */
export const scrubSpan = (M: TimelineModel): number => Math.max(48, M.endF)

/** A frame as a share of the scrub bar (0..1). */
export const scrubAt = (M: TimelineModel, f: number): number => Math.max(0, Math.min(1, f / scrubSpan(M)))

/** The frame under a point on the scrub bar (x from its left, w its width). */
export const scrubFrame = (M: TimelineModel, x: number, w: number): number => Math.max(0, Math.min(Math.max(0, M.endF - 1), Math.round((x / Math.max(1, w)) * scrubSpan(M))))

export interface ScrubMarks {
  changes: { f0: number; f1: number; colour: string; title: string }[]
  markers: { key: string; f: number; color: string; title: string }[]
  heads: { id: string; f: number; colour: string; title: string }[]
  head: number
}

export function scrubMarks(S: SimState, M: TimelineModel, v: MonitorView, now: number): ScrubMarks {
  const minW = scrubSpan(M) / 300
  return {
    changes: bracketsOf(S).map((b) => ({ f0: b.f0, f1: Math.max(b.f1, b.f0 + minW), colour: b.colour, title: b.line })),
    markers: M.markers.filter((m) => m.owner === 'timeline' && m.f != null).map((m) => ({ key: m.key, f: m.f as number, color: m.color, title: m.name })),
    heads: playheadsOf(S, M, now)
      .filter((h) => h.id !== v.head.id && !h.away)
      .map((h) => ({ id: h.id, f: h.f, colour: h.colour, title: `${h.name} · ${M.tcF(h.f)}` })),
    head: v.head.f,
  }
}

/** Where the agent's edits start, in order (↑ and ↓, the previous and next edit buttons). */
export function editStarts(S: SimState, agent: string | null): number[] {
  if (!agent) return []
  return [...new Set(bracketsOf(S).filter((b) => b.who === agent).map((b) => b.f0))].sort((a, b) => a - b)
}

/** The transport's left line: whose playhead the monitor shows, or what just happened to it. */
export function whoLine(S: SimState, v: MonitorView, local: LocalPlay | null): string {
  const h = v.head
  if (local?.loop) return `Looping ${local.loop.who}’s ${VERB_WORD[local.loop.verb ?? ''] ?? 'change'} · Esc to stop`
  if (local?.skipped) return `Skipped to ${local.skipped}`
  if (!local && S.local?.skipped && S.local.at === S.i) return `Skipped to ${v.picture?.it?.name ?? 'the next clip'}`
  const speed = local && Math.abs(local.rate) > 1 ? ` · ${local.rate > 0 ? '' : '−'}${Math.abs(local.rate)}×` : ''
  if (!h.self) return `${h.name}’s playhead${h.playing ? ' · playing' : ''}${speed}`
  if (v.state === 'ended') return 'Your playhead · end of the edit'
  return `Your playhead${speed}`
}
