// The ruler (56 px, filmcraft's rows, v2.1 to v2.7b): along the top, the
// playhead name tags and the marker flags, sharing one 12px tag style (a tag
// wins over a flag; flags closer than 14 px merge; a marker label is its
// whole name with a capital, or no label); timecode labels under them; the
// change band; ticks rising from the bottom; a 2 px readiness line. Pure:
// the component paints what this places. Only the window it's asked for is
// worked out (a long edit draws a screen either side of what shows).

import type { TimelineModel } from './fold'
import { rulerSteps } from './time'

export const RULER_H = 56
/** The top row's tags: 18 px tall from y = 1. */
export const TAG_H = 18
/** A tag's padding and edge, both sides together (6 px each, and a 1 px edge on a marker). */
export const TAG_PAD = 14
/** Flags closer than this merge into one (every name in its tooltip). */
export const FLAG_MERGE_PX = 14
/** The least room between two things on the top row. */
export const ROW_GAP = 3

export interface RulerTick {
  f: number
  x: number
  major: boolean
  /** The timecode at a major tick; null at a minor one. */
  label: string | null
}

/** Ticks by the density rule (labels ≥ 110 px apart, minor ticks ≥ 12 px) in frames f0..f1. */
export function rulerTicks(M: TimelineModel, ppf: number, f0: number, f1: number): RulerTick[] {
  const { minor, label } = rulerSteps(ppf, M.T.rate ? M.T.rate.base : 24)
  const out: RulerTick[] = []
  for (let f = Math.floor(Math.max(0, f0) / minor) * minor; f <= f1; f += minor) {
    const major = f % label === 0
    out.push({ f, x: f * ppf, major, label: major ? M.tcF(f) : null })
  }
  return out
}

export interface RulerHead {
  id: string
  name: string
  f: number
  colour: string
  self?: boolean
  away?: boolean
}

export interface RulerMarker {
  key: string
  name: string
  f: number
  color: string
  note?: string
}

export interface Span {
  x0: number
  x1: number
}

export interface PlacedTag {
  head: RulerHead
  x0: number
  w: number
}

export interface PlacedFlag {
  x: number
  /** The first and any merged into it. */
  markers: RulerMarker[]
  label: { text: string; x0: number; w: number } | null
}

/** A marker's label: its whole name with a capital. */
export const markerText = (name: string): string => name.charAt(0).toUpperCase() + name.slice(1)

/**
 * The top row: tags first (each centred on its playhead, stepping right past
 * anything in the way, hidden when it can't), then flags (merged when closer
 * than 14 px; left off where a tag is; labelled only where the whole name
 * fits). `reserved` is room taken already (the off-screen stacks); `width`
 * the row's end. `measure` is the painted width of a 12px label.
 */
export function layoutRulerTop(
  heads: readonly RulerHead[],
  markers: readonly RulerMarker[],
  ppf: number,
  measure: (text: string) => number,
  reserved: readonly Span[] = [],
  width = Infinity,
): { tags: PlacedTag[]; flags: PlacedFlag[]; hidden: RulerHead[] } {
  const boxes: Span[] = reserved.map((r) => ({ ...r }))
  const hit = (x0: number, x1: number): Span | undefined => boxes.find((b) => b.x0 < x1 + ROW_GAP && b.x1 > x0 - ROW_GAP)
  const tags: PlacedTag[] = []
  const hidden: RulerHead[] = []
  for (const h of [...heads].sort((a, b) => a.f - b.f)) {
    const w = Math.ceil(measure(h.name)) + TAG_PAD
    let x0 = Math.max(0, h.f * ppf - w / 2)
    for (let k = 0; k < 12; k++) {
      const b = hit(x0, x0 + w)
      if (!b) break
      x0 = b.x1 + ROW_GAP + 1
    }
    if (hit(x0, x0 + w) || x0 + w > width) {
      hidden.push(h)
      continue
    }
    boxes.push({ x0, x1: x0 + w })
    tags.push({ head: h, x0, w })
  }
  const flags: PlacedFlag[] = []
  for (const m of [...markers].sort((a, b) => a.f - b.f)) {
    const x = m.f * ppf
    const last = flags[flags.length - 1]
    if (last && x - last.x < FLAG_MERGE_PX) {
      last.markers.push(m)
      last.label = null
      continue
    }
    if (hit(x - 4, x + 4) || x + 4 > width) continue
    const text = markerText(m.name)
    const w = Math.ceil(measure(text)) + TAG_PAD
    const fits = text.length > 0 && !hit(x - 4, x + 6 + w) && x + 6 + w <= width
    boxes.push({ x0: x - 4, x1: fits ? x + 6 + w : x + 4 })
    flags.push({ x, markers: [m], label: fits ? { text, x0: x + 6, w } : null })
  }
  return { tags, flags, hidden }
}
