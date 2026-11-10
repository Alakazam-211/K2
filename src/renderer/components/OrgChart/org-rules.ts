// The line rules the canvas checks before it posts (prd-org-chart-v1 §4,
// Rosson 11-10: "top to bottom, or laterally. (never top to top)", no loops).
// The daemon enforces the same rules and is the authority; checking here
// lets a refused drag snap back with the daemon's sentence and no round trip.

import type { OrgChart, OrgLineKind, OrgRef } from './org-api'
import type { Side } from './board-geom'

export interface LineRefusal {
  code: 'line_top_to_top' | 'line_self' | 'line_exists' | 'line_loop'
  message: string
}

export const TOP_TO_TOP_MESSAGE = 'Lines go top to bottom (below) or side to side (beside), never top to top.'

/** What a drag from one card's dot to another's makes. `upper`: which end
 *  sits above for a `below` line. */
export type WireResult =
  | { ok: true; kind: 'below'; upper: 'from' | 'to' }
  | { ok: true; kind: 'beside' }
  | { ok: false; refusal: LineRefusal }

const vertical = (s: Side): boolean => s === 'top' || s === 'bottom'

/** Bottom → top (or top → bottom): one sits above the other. Side → side:
 *  beside. Anything else (top → top, bottom → bottom, top → a side) is
 *  refused: a line is only ever `below` or `beside`. */
export function wireKind(fromSide: Side, toSide: Side): WireResult {
  if (fromSide === 'bottom' && toSide === 'top') return { ok: true, kind: 'below', upper: 'from' }
  if (fromSide === 'top' && toSide === 'bottom') return { ok: true, kind: 'below', upper: 'to' }
  if (!vertical(fromSide) && !vertical(toSide)) return { ok: true, kind: 'beside' }
  return { ok: false, refusal: { code: 'line_top_to_top', message: TOP_TO_TOP_MESSAGE } }
}

const key = (r: OrgRef): string => `${r.kind}:${r.id}`
const same = (a: OrgRef, b: OrgRef): boolean => a.kind === b.kind && a.id === b.id

/** A node's name as people read it (an agent's name or an org's). */
export function refName(chart: OrgChart, r: OrgRef): string {
  if (r.kind === 'agent') return chart.agents.find((a) => a.id === r.id)?.name ?? r.id
  return chart.orgs.find((o) => o.id === r.id)?.name ?? r.id
}

/** Is `top` already above `bottom` through `below` lines (directly or down a chain)? */
function above(chart: OrgChart, top: OrgRef, bottom: OrgRef): boolean {
  const down = new Map<string, OrgRef[]>()
  for (const l of chart.lines) {
    if (l.kind !== 'below') continue
    const k = key(l.a)
    if (!down.has(k)) down.set(k, [])
    down.get(k)!.push(l.b)
  }
  const seen = new Set<string>()
  const stack = [top]
  while (stack.length) {
    const n = stack.pop()!
    for (const next of down.get(key(n)) ?? []) {
      if (same(next, bottom)) return true
      const k = key(next)
      if (!seen.has(k)) {
        seen.add(k)
        stack.push(next)
      }
    }
  }
  return false
}

/** Null when the line may be drawn; else the refusal the daemon would give.
 *  For `below`, `a` sits above `b`. */
export function checkLine(chart: OrgChart, kind: OrgLineKind, a: OrgRef, b: OrgRef): LineRefusal | null {
  if (same(a, b)) return { code: 'line_self', message: 'A line joins two different cards.' }
  // `b` already above `a` (also through a chain): drawing `a` above `b` would loop.
  if (kind === 'below' && above(chart, b, a)) {
    const A = refName(chart, a)
    const B = refName(chart, b)
    return { code: 'line_loop', message: `${B} is already above ${A}, so ${A} can't be above ${B}.` }
  }
  // One line per pair, whichever way it was drawn.
  if (chart.lines.some((l) => (same(l.a, a) && same(l.b, b)) || (same(l.a, b) && same(l.b, a)))) {
    return { code: 'line_exists', message: `${refName(chart, a)} and ${refName(chart, b)} already have a line.` }
  }
  return null
}
