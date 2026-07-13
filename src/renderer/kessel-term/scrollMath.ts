// Pure window math for TerminalPane's pixel-precision scrolling.
//
// The scroll position is a single `scrollPx` value: distance in PIXELS
// scrolled up from the bottom of the buffer. 0 = pinned to the live
// grid (the heavy-output path), max = scrollback.length * cellHeight
// (top of history). The renderer paints a strip of
// `viewportRows + overscan` rows and positions it with a
// `translateY(...)` — a scroll that stays within one row is a pure
// compositor translation (no row mounts/unmounts), and crossing a row
// boundary re-slices only the strip's edge rows. Keeping this math
// pure (no DOM, no React) is what makes it unit-testable; the
// component is a thin consumer.

/** Rows rendered beyond each edge of the visible window so a
 *  boundary crossing reveals an already-mounted row instead of a
 *  blank flash. Clamped at the buffer edges. */
export const SCROLL_OVERSCAN_ROWS = 3

/** Upper clamp for `scrollPx`: the whole scrollback, in pixels. */
export function maxScrollPx(scrollbackLen: number, cellHeight: number): number {
  return Math.max(0, scrollbackLen * cellHeight)
}

/** Clamp a scroll position to [0, scrollback * cellHeight]. Non-finite
 *  input collapses to 0 (bottom) so a bad delta can never wedge the
 *  viewport off-buffer. */
export function clampScrollPx(
  px: number,
  scrollbackLen: number,
  cellHeight: number,
): number {
  if (!Number.isFinite(px) || px <= 0) return 0
  const max = maxScrollPx(scrollbackLen, cellHeight)
  return px > max ? max : px
}

export interface StripLayout {
  /** Absolute buffer row whose top edge is at (or above) the viewport
   *  top. May be NEGATIVE when the buffer is shorter than the viewport
   *  — those rows render blank, matching the bottom-anchored grid. */
  firstVisibleRow: number
  /** Absolute buffer row of the strip's first rendered row
   *  (`firstVisibleRow - overscanTop`). */
  stripStart: number
  /** Overscan rows actually rendered above the viewport (clamped to
   *  the rows that exist above `firstVisibleRow`). */
  overscanTop: number
  /** Rows rendered in the strip (visible + overscan, clamped so no
   *  row index reaches past the end of the buffer). */
  rowCount: number
  /** Sub-row pixel offset in [0, cellHeight): how much of
   *  `firstVisibleRow` is hidden above the viewport top. */
  fraction: number
  /** `-(fraction + overscanTop * cellHeight)` — the strip's
   *  translateY. Applying it puts `firstVisibleRow`'s top exactly
   *  `fraction` px above the viewport top. */
  translateY: number
}

/** Map a pixel scroll position onto the row strip the renderer must
 *  paint. Invariants:
 *   - scrollPx = 0 ⇒ the strip's visible window is EXACTLY the last
 *     `viewportRows` rows of the buffer (fraction 0, translateY only
 *     compensates for top overscan).
 *   - scrollPx = k * cellHeight ⇒ fraction 0 (whole-line positions
 *     are exact, no drift).
 *   - rows at absolute index < 0 are the caller's blank filler;
 *     rows ≥ totalRows are never requested. */
export function computeStripLayout(
  scrollPx: number,
  totalRows: number,
  viewportRows: number,
  cellHeight: number,
  overscan: number = SCROLL_OVERSCAN_ROWS,
): StripLayout {
  if (cellHeight <= 0 || viewportRows <= 0) {
    return {
      firstVisibleRow: 0,
      stripStart: 0,
      overscanTop: 0,
      rowCount: 0,
      fraction: 0,
      translateY: 0,
    }
  }
  // Viewport top edge in buffer-pixel space (0 = top of scrollback).
  const topPx = (totalRows - viewportRows) * cellHeight - scrollPx
  const firstVisibleRow = Math.floor(topPx / cellHeight)
  const fraction = topPx - firstVisibleRow * cellHeight
  // A fractional offset exposes a partial extra row at the bottom.
  const visibleCount = fraction > 0 ? viewportRows + 1 : viewportRows
  const overscanTop = Math.min(overscan, Math.max(0, firstVisibleRow))
  const stripStart = firstVisibleRow - overscanTop
  const stripEnd = Math.min(firstVisibleRow + visibleCount + overscan, totalRows)
  return {
    firstVisibleRow,
    stripStart,
    overscanTop,
    rowCount: Math.max(0, stripEnd - stripStart),
    fraction,
    // `|| 0` normalizes the -0 produced when both terms are 0.
    translateY: -(fraction + overscanTop * cellHeight) || 0,
  }
}

export interface ScrollbarThumb {
  /** Thumb top as a fraction of the track length, in [0, 1 - heightFrac]. */
  topFrac: number
  /** Thumb length as a fraction of the track length. */
  heightFrac: number
}

/** Proportional overlay-scrollbar thumb. Null when there is nothing
 *  to scroll (no scrollback, or the buffer fits the viewport) — the
 *  bar simply doesn't render. Fractions (not px) so the caller can
 *  position with CSS percentages and never needs a measured track
 *  length at render time. */
export function computeScrollbarThumb(
  scrollPx: number,
  scrollbackLen: number,
  totalRows: number,
  viewportRows: number,
  cellHeight: number,
  minHeightFrac = 0.05,
): ScrollbarThumb | null {
  const max = maxScrollPx(scrollbackLen, cellHeight)
  if (max <= 0 || totalRows <= 0 || viewportRows <= 0) return null
  if (totalRows <= viewportRows) return null
  const heightFrac = Math.min(1, Math.max(minHeightFrac, viewportRows / totalRows))
  const clamped = clampScrollPx(scrollPx, scrollbackLen, cellHeight)
  // scrollPx = 0 (bottom) ⇒ thumb sits at the track's bottom;
  // scrollPx = max (top of history) ⇒ thumb at the track's top.
  const topFrac = (1 - clamped / max) * (1 - heightFrac)
  return { topFrac, heightFrac }
}

/** Inverse of `computeScrollbarThumb`'s position mapping: a dragged
 *  thumb-top fraction back to a clamped scrollPx. */
export function scrollPxFromThumbTopFrac(
  topFrac: number,
  heightFrac: number,
  scrollbackLen: number,
  cellHeight: number,
): number {
  const max = maxScrollPx(scrollbackLen, cellHeight)
  const range = 1 - heightFrac
  if (max <= 0 || range <= 0) return 0
  const t = Math.min(range, Math.max(0, topFrac))
  return clampScrollPx((1 - t / range) * max, scrollbackLen, cellHeight)
}

/** Scroll anchoring: keep the viewed content stationary when rows
 *  append BELOW it. `scrollPx` measures from the buffer BOTTOM, so
 *  every appended row silently shifts the window down one row —
 *  streaming output made a scrolled-up reader's text crawl, and a k1
 *  resync snapshot (sent by the daemon exactly when acks lag, i.e.
 *  during fast scrolling) yanked the view by the whole backlog at
 *  once. Every terminal pins here (xterm's isUserScrolling, kitty's
 *  scrolled_by). At the bottom (scrollPx === 0) the viewport keeps
 *  following live output — that behavior is unchanged. */
export function anchorScrollPx(
  scrollPx: number,
  appendedRows: number,
  scrollbackLen: number,
  cellHeight: number,
): number {
  if (scrollPx <= 0 || appendedRows <= 0) return scrollPx
  return clampScrollPx(
    scrollPx + appendedRows * cellHeight,
    scrollbackLen,
    cellHeight,
  )
}
