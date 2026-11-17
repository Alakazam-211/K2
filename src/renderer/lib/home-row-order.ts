// Home groups: the order a group's agents show in, and what ⌘1–9 opens.
//
//   1. Pinned rows first, in pin order (at most MAX_PINS, like iMessage).
//   2. Then the rest: in the group's manual order, or, with the group's
//      "Most recent at the top" on, newest send first (`lastMessagedAt`,
//      stamped only when the person sends to that agent), the rows never
//      sent to after them in manual order.
//
// ⌘1–9 opens the pinned rows in order. A group with no pins keeps the old
// behaviour: ⌘N opens the Nth row as shown (and ⌘0 the tenth).
//
// Pure, over the Homes store's shapes. Pins and sends sync on the row,
// "Most recent at the top" on the group (homes_sync, migration 0305).

import { assignOrders, compareOrder } from '@/lib/fractional-index'

/** The most pinned rows one group may hold (the daemon's `MAX_PINS`). */
export const MAX_PINS = 9

interface RowLike {
  address: string
  pinOrder?: string | null
  lastMessagedAt?: number | null
}

interface HomeLike<R extends RowLike> {
  rows: readonly R[]
  recentFirst?: boolean
}

export function isPinned(r: RowLike): boolean {
  return typeof r.pinOrder === 'string' && r.pinOrder.length > 0
}

/** The pinned rows, in pin order. */
export function pinnedRows<R extends RowLike>(home: HomeLike<R>): R[] {
  return home.rows
    .filter(isPinned)
    .sort((a, b) => compareOrder(a.pinOrder as string, b.pinOrder as string) || compareOrder(a.address, b.address))
}

/** The rows that are not pinned: manual order, or newest send first. */
export function unpinnedRows<R extends RowLike>(home: HomeLike<R>): R[] {
  const rest = home.rows.filter((r) => !isPinned(r))
  if (!home.recentFirst) return rest
  // Array.prototype.sort is stable: equal stamps (and no stamp) keep manual order.
  return rest.sort((a, b) => (b.lastMessagedAt ?? 0) - (a.lastMessagedAt ?? 0))
}

/** Every row, in the order the nav shows them. */
export function displayRows<R extends RowLike>(home: HomeLike<R>): R[] {
  return [...pinnedRows(home), ...unpinnedRows(home)]
}

/** The rows ⌘1–9 open, in order: the pinned ones, or (no pins) all as shown. */
export function shortcutRows<R extends RowLike>(home: HomeLike<R>): R[] {
  const pins = pinnedRows(home)
  return pins.length > 0 ? pins : displayRows(home)
}

/** An index past every chord: the number column draws its muted dash. */
export const NO_SHORTCUT_INDEX = 99

/** The ⌘ number a row shown at `displayIndex` carries (0-based, as the
 *  row's number column takes it): with pins, only the pinned rows have one. */
export function shortcutIndexFor<R extends RowLike>(home: HomeLike<R>, displayIndex: number): number {
  const pins = home.rows.filter(isPinned).length
  if (pins === 0) return displayIndex
  return displayIndex < pins ? displayIndex : NO_SHORTCUT_INDEX
}

/**
 * A drag in the nav, in shown positions: the row at `from` dropped to land
 * at `to`. Pinned rows move among the pins; unpinned rows move among the
 * unpinned ones in manual order (with "Most recent at the top" on, their
 * order is automatic, so nothing moves). Returns the new pin order, or the
 * new full manual order of the group's rows, or null.
 */
export function planRowDrop<R extends RowLike>(
  home: HomeLike<R>,
  from: number,
  to: number,
): { pins: string[] } | { rows: string[] } | null {
  const pins = pinnedRows(home).map((r) => r.address)
  const shown = displayRows(home)
  if (from < 0 || from >= shown.length || from === to) return null
  if (from < pins.length) {
    const target = Math.min(Math.max(to, 0), pins.length - 1)
    if (target === from) return null
    const next = [...pins]
    const [moved] = next.splice(from, 1)
    next.splice(target, 0, moved)
    return { pins: next }
  }
  if (home.recentFirst) return null
  const unpinned = shown.slice(pins.length).map((r) => r.address)
  const u = from - pins.length
  const v = Math.min(Math.max(to - pins.length, 0), unpinned.length - 1)
  if (u === v) return null
  const reordered = [...unpinned]
  const [moved] = reordered.splice(u, 1)
  reordered.splice(v, 0, moved)
  // The unpinned rows take their new order in the slots they already hold
  // in the group's manual order; pinned rows keep theirs.
  let k = 0
  return { rows: home.rows.map((r) => (isPinned(r) ? r.address : reordered[k++])) }
}

/**
 * Where the nav's drop line goes for the row at `from` with the pointer
 * over gap `gap` (0..rows, shown positions): the gap the drop really lands
 * in, kept to the row's own section (a pin among the pins, an unpinned row
 * among the unpinned), as `planRowDrop` clamps it. Null where the drop
 * would move nothing: its own place, or an unpinned row while "Most recent
 * at the top" is on.
 */
export function rowDropLanding<R extends RowLike>(home: HomeLike<R>, from: number, gap: number): number | null {
  const pins = pinnedRows(home).length
  const n = home.rows.length
  if (from < 0 || from >= n) return null
  if (from >= pins && home.recentFirst) return null
  const [lo, hi] = from < pins ? [0, pins] : [pins, n]
  const at = Math.min(Math.max(gap, lo), hi)
  return at === from || at === from + 1 ? null : at
}

/** Pin keys for `addresses` in this order, keeping every key that is
 *  still in order (`fractional-index`'s assignOrders). */
export function pinKeys(addresses: readonly string[], current: (address: string) => string | null): Map<string, string> {
  const keys = assignOrders(addresses.length, (i) => current(addresses[i]))
  return new Map(addresses.map((a, i) => [a, keys[i]]))
}
