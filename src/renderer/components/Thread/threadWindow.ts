// The chat render window (0.46.2, ported from K2 Companion's
// `src/zen/threadWindow.ts`, phone-perf 5f6c158). A long chat keeps every
// message in memory (its hook, its state) but only a window of rows in the
// DOM. Two-ended:
//   - following the newest: the newest WINDOW_ROWS; as messages come in,
//     the oldest drop off once scrolling is quiet;
//   - scrolled away: rows come back WINDOW_STEP at a time before the reader
//     reaches a drawn edge, from memory first (the server's older page is
//     fetched only when memory runs out);
//   - never more than WINDOW_MAX_ROWS drawn for long: growing at one end
//     drops rows at the other (never one on screen).
// Rows that left are replaced by a spacer of their measured height, so the
// list's scrollHeight and what is on screen never move. A row never
// measured counts 0 above (the window's top is then a real edge) and an
// estimate below (out of view either way).
//
// The window is kept by row keys, not indices: messages arriving (end) or
// loaded earlier (start) do not move it. `lo: null` = from the first
// message in memory (an older page shows at once, as before the window);
// `hi: null` = through the newest (arrivals show at once).

/** Rows drawn when a chat opens (and while following the newest). */
export const WINDOW_ROWS = 30
/** The most rows drawn while reading through a chat. */
export const WINDOW_MAX_ROWS = 50
/** Rows drawn (and, at the cap, dropped at the far end) per step. */
export const WINDOW_STEP = WINDOW_MAX_ROWS - WINDOW_ROWS
/** Bring rows back when a drawn edge is within this many screens. */
export const WINDOW_RESPAWN_SCREENS = 1.5
/** Rows more than this many screens past the view may leave (once quiet). */
export const WINDOW_KEEP_SCREENS = 2
/** Height (px) assumed for a row below the window that was never drawn. */
export const WINDOW_EST_ROW_PX = 72
/** Quiet time (ms) after a scroll before rows are dropped. */
export const WINDOW_IDLE_MS = 300

export interface ChatWindow {
  /** Key of the first drawn row; null = the first in memory. */
  lo: string | null
  /** Key of the last drawn row; null = the newest. */
  hi: string | null
}

/** A drawn range [lo, hi) of indices. */
export interface WindowRange {
  lo: number
  hi: number
}

/** The drawn range of `keys` for `win` (null: the newest `size`). A key no
 *  longer in memory falls back to the newest `size`. */
export function resolveWindow(keys: readonly string[], win: ChatWindow | null, size = WINDOW_ROWS): WindowRange {
  const len = keys.length
  const newest = { lo: Math.max(0, len - size), hi: len }
  if (!win) return newest
  const lo = win.lo == null ? 0 : keys.indexOf(win.lo)
  const hiAt = win.hi == null ? len - 1 : keys.indexOf(win.hi)
  if (lo < 0 || hiAt < 0 || hiAt < lo) return newest
  return { lo, hi: hiAt + 1 }
}

/** The window for a drawn range (what `resolveWindow` turns back into it). */
export function pinWindow(keys: readonly string[], range: WindowRange): ChatWindow {
  return {
    lo: range.lo <= 0 ? null : keys[range.lo] ?? null,
    hi: range.hi >= keys.length ? null : keys[range.hi - 1] ?? null,
  }
}

/** Grow `range` by `step` rows at `edge`, keeping at most `max` drawn: past
 *  it, rows at the other end leave, but never one of `keep` (the rows on
 *  screen), which may leave the window a little over `max`. Returns
 *  `range` itself when there is nothing to add at that edge. */
export function growWindow(
  range: WindowRange,
  len: number,
  edge: 'top' | 'bottom',
  keep: WindowRange,
  step = WINDOW_STEP,
  max = WINDOW_MAX_ROWS,
): WindowRange {
  if (edge === 'top') {
    if (range.lo <= 0) return range
    const lo = Math.max(0, range.lo - step)
    const hi = Math.max(Math.min(range.hi, lo + max), Math.min(range.hi, keep.hi))
    return { lo, hi }
  }
  if (range.hi >= len) return range
  const hi = Math.min(len, range.hi + step)
  const lo = Math.min(Math.max(range.lo, hi - max), Math.max(range.lo, keep.lo))
  return { lo, hi }
}

/** Top spacer: the measured rows above the window (never measured: 0). */
export function spacerAbove(keys: readonly string[], lo: number, heights: ReadonlyMap<string, number>): number {
  let px = 0
  for (let i = 0; i < lo; i++) px += heights.get(keys[i]) ?? 0
  return px
}

/** Bottom spacer: the rows below the window, measured or estimated. */
export function spacerBelow(
  keys: readonly string[],
  hi: number,
  heights: ReadonlyMap<string, number>,
  estimate = WINDOW_EST_ROW_PX,
): number {
  let px = 0
  for (let i = hi; i < keys.length; i++) px += heights.get(keys[i]) ?? estimate
  return px
}

/** Where a row starts in the list's flow: above its own top margin. */
function outerTop(el: HTMLElement): number {
  const margin = parseFloat(getComputedStyle(el).marginTop) || 0
  return el.offsetTop - margin
}

/** What each of `rows` adds to the list's height (its margin, box and the
 *  gap after it): the distance to the next element's start. Layout
 *  offsets, so transforms (the message-in rise) do not count. */
export function measureRows(rows: readonly HTMLElement[]): number[] {
  return rows.map((el) => {
    const next = el.nextElementSibling as HTMLElement | null
    if (next) return Math.max(0, outerTop(next) - outerTop(el))
    const parent = el.parentElement
    const gap = parent ? parseFloat(getComputedStyle(parent).rowGap) || 0 : 0
    return Math.max(0, el.offsetTop + el.offsetHeight - outerTop(el) + gap)
  })
}
