// The host's presence and claim rules for a grid (addendum r1 Items 3.4 and
// 4), for the sample source and the golden checks: what the host refuses and
// what it drops. The view never enforces them (the host does); it uses them
// only to build constructed sample events that a host would send.
//
// Why this is still ours: the kit's client (v0.17.0) checks corners
// (`cornerProblem`, used here) and refuses too many ranges on send, but the
// host's own verdicts (a watcher refused, what a change leaves of someone's
// cells, what a claim holds off) live in the kit's Rust host, not its client.

import { CELLS_RANGES_MAX, colNumber, type Cells } from '@k2/mpc-client'
import { cornerOk, cornerPlace, gridOf, inRect, rangeRect, type Book } from './workbook'
import type { ClaimRecord } from './reducer'

/** The kit's check on a `cells` value (Item 3.4): an error id, or null when accepted. */
export function checkCells(book: Book, v: Cells, o?: { watcher?: boolean }): string | null {
  if (o?.watcher) return 'MPC-E305'
  if (v.ranges.length > CELLS_RANGES_MAX) return 'MPC-E310'
  const g = gridOf(book, v.container)
  if (!g) return 'MPC-E114'
  const corners = [v.active, ...v.ranges.flatMap((r) => [r.start, r.end])].filter((c): c is NonNullable<typeof c> => c != null)
  for (const c of corners) if (!cornerOk(g, c)) return 'MPC-E114'
  if (v.active && v.ranges.length) {
    const at = cornerPlace(g, v.active)
    if (!at || !v.ranges.some((r) => { const R = rangeRect(g, r); return !!R && inRect(at, R) })) return 'MPC-E114'
  }
  return null
}

/** The kit's check on a viewport: finite, no negative size; refused for watchers. */
export function checkViewport(rect: { x: number; y: number; w: number; h: number }, o?: { watcher?: boolean }): string | null {
  if (o?.watcher) return 'MPC-E305'
  return [rect.x, rect.y, rect.w, rect.h].every((n) => Number.isFinite(n)) && rect.w >= 0 && rect.h >= 0 ? null : 'MPC-E114'
}

/**
 * After a change: what the kit leaves of a person's `cells` (CS8, CS9). A range
 * (or the caret) is dropped, never moved, when a corner's row or column stops
 * being live or its form breaks. Returns the value to send, or `undefined`
 * when nothing changed.
 */
export function dropBroken(book: Book, cells: Cells | null): Cells | null | undefined {
  if (!cells) return undefined
  const g = gridOf(book, cells.container)
  if (!g) return null
  const ranges = cells.ranges.filter((r) => cornerOk(g, r.start) && cornerOk(g, r.end))
  const active = cells.active && cornerOk(g, cells.active) ? cells.active : null
  if (ranges.length === cells.ranges.length && active === cells.active) return undefined
  // Nothing left: the value goes (CS9, r1.1).
  return active == null && ranges.length === 0 ? null : { container: cells.container, active, ranges }
}

/** CC2: what a cells claim holds off (its one range: the kit's claims on cells name exactly one), for agents only (`ClaimPolicy::Respect`), given a verb line as the goldens write it. */
export function claimVerdict(book: Book, cl: ClaimRecord, row: { who: string; whoKind: string; verb: string }): string {
  const g = gridOf(book, cl.container)
  const R = g && cl.ranges?.[0] ? rangeRect(g, cl.ranges[0]) : null
  if (!R) return 'applied'
  const own = row.who === cl.actor.id
  const agent = row.whoKind === 'agent'
  let m: RegExpExecArray | null
  if ((m = /^cell set (@sheet:\d+) ([A-Z]+)(\d+)/.exec(row.verb))) {
    const inside = m[1] === cl.container && inRect({ r: Number(m[3]), c: colNumber(m[2]) ?? 0 }, R)
    return inside && agent && !own ? 'refused MPC-E301' : 'applied'
  }
  if ((m = /^rows delete (@sheet:\d+) AT (\d+)/.exec(row.verb))) {
    const inside = m[1] === cl.container && Number(m[2]) >= R.r0 && Number(m[2]) <= R.r1
    return inside && agent && !own ? 'refused MPC-E301' : 'applied'
  }
  if ((m = /^range set (@sheet:\d+) ([A-Z]+)(\d+)/.exec(row.verb))) {
    const inside = m[1] === cl.container && inRect({ r: Number(m[3]), c: colNumber(m[2]) ?? 0 }, R)
    if (inside && own) return `applied; frame claim = ${cl.id}`
    return inside && agent ? 'refused MPC-E301' : 'applied'
  }
  // Inserts grow it; row fields and people are never refused.
  return 'applied'
}
