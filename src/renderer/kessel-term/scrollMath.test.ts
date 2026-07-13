import { describe, expect, it } from 'vitest'

import {
  SCROLL_OVERSCAN_ROWS,
  anchorScrollPx,
  clampScrollPx,
  computeScrollbarThumb,
  computeStripLayout,
  maxScrollPx,
  scrollPxFromThumbTopFrac,
} from './scrollMath'

// Canonical fixture: 76 scrollback rows + 24 grid rows (= 100 total),
// 24-row viewport, 20px cells. maxScrollPx = 76 * 20 = 1520.
const CELL_H = 20
const SCROLLBACK = 76
const VIEWPORT = 24
const TOTAL = SCROLLBACK + VIEWPORT

describe('computeStripLayout', () => {
  it('at bottom (scrollPx=0): exact last-viewport window, fraction 0, top overscan only', () => {
    const s = computeStripLayout(0, TOTAL, VIEWPORT, CELL_H)
    expect(s.firstVisibleRow).toBe(TOTAL - VIEWPORT) // 76
    expect(s.fraction).toBe(0)
    expect(s.overscanTop).toBe(SCROLL_OVERSCAN_ROWS)
    expect(s.stripStart).toBe(TOTAL - VIEWPORT - SCROLL_OVERSCAN_ROWS) // 73
    // No rows exist below the live grid — bottom overscan clamps away.
    expect(s.stripStart + s.rowCount).toBe(TOTAL)
    expect(s.rowCount).toBe(VIEWPORT + SCROLL_OVERSCAN_ROWS)
    expect(s.translateY).toBe(-SCROLL_OVERSCAN_ROWS * CELL_H)
  })

  it('whole-line scroll positions are exact (fraction 0, no drift)', () => {
    const s = computeStripLayout(5 * CELL_H, TOTAL, VIEWPORT, CELL_H)
    expect(s.firstVisibleRow).toBe(TOTAL - VIEWPORT - 5)
    expect(s.fraction).toBe(0)
    expect(s.translateY).toBe(-s.overscanTop * CELL_H)
  })

  it('mid-scroll fractional: partial row at top, +1 visible row, translate compensates', () => {
    // 1.5 rows up from the bottom.
    const s = computeStripLayout(1.5 * CELL_H, TOTAL, VIEWPORT, CELL_H)
    expect(s.firstVisibleRow).toBe(74)
    expect(s.fraction).toBe(10)
    expect(s.overscanTop).toBe(3)
    expect(s.stripStart).toBe(71)
    // visible 25 rows (74..98) + partial bottom row 99; overscan below
    // clamps at the buffer end (row 100 doesn't exist).
    expect(s.stripStart + s.rowCount).toBe(TOTAL)
    expect(s.translateY).toBe(-(10 + 3 * CELL_H))
  })

  it('top-of-buffer clamp: scrollPx=max shows row 0 with zero top overscan', () => {
    const s = computeStripLayout(
      maxScrollPx(SCROLLBACK, CELL_H),
      TOTAL,
      VIEWPORT,
      CELL_H,
    )
    expect(s.firstVisibleRow).toBe(0)
    expect(s.fraction).toBe(0)
    expect(s.overscanTop).toBe(0)
    expect(s.stripStart).toBe(0)
    expect(s.rowCount).toBe(VIEWPORT + SCROLL_OVERSCAN_ROWS)
    expect(s.translateY).toBe(0)
  })

  it('near-top: overscan shrinks to the rows that actually exist above', () => {
    // 2 rows above the viewport top → overscanTop 2, not 3.
    const s = computeStripLayout(
      maxScrollPx(SCROLLBACK, CELL_H) - 2 * CELL_H,
      TOTAL,
      VIEWPORT,
      CELL_H,
    )
    expect(s.firstVisibleRow).toBe(2)
    expect(s.overscanTop).toBe(2)
    expect(s.stripStart).toBe(0)
    expect(s.translateY).toBe(-2 * CELL_H)
  })

  it('buffer shorter than viewport: negative firstVisibleRow, blank filler above, translateY 0', () => {
    // 10 real rows in a 24-row viewport (fresh session, no scrollback).
    const s = computeStripLayout(0, 10, VIEWPORT, CELL_H)
    expect(s.firstVisibleRow).toBe(10 - VIEWPORT) // -14
    expect(s.fraction).toBe(0)
    expect(s.overscanTop).toBe(0)
    expect(s.stripStart).toBe(-14)
    // Rows -14..-1 are the caller's blank filler; 0..9 are real.
    expect(s.stripStart + s.rowCount).toBe(10)
    expect(s.rowCount).toBe(VIEWPORT)
    expect(s.translateY).toBe(0)
  })

  it('degenerate inputs (no metrics yet / zero viewport) render nothing', () => {
    expect(computeStripLayout(0, TOTAL, VIEWPORT, 0).rowCount).toBe(0)
    expect(computeStripLayout(0, TOTAL, 0, CELL_H).rowCount).toBe(0)
  })
})

describe('clampScrollPx', () => {
  it('scrollback shrink re-clamp: a stranded offset collapses to the new max', () => {
    const before = 60 * CELL_H
    // Scrollback shrank to 10 rows (resize / restart / alt-screen).
    expect(clampScrollPx(before, 10, CELL_H)).toBe(10 * CELL_H)
    // Scrollback vanished entirely (alt screen) → pinned to bottom.
    expect(clampScrollPx(before, 0, CELL_H)).toBe(0)
  })

  it('in-range values pass through untouched (identity keeps setState bail-outs working)', () => {
    expect(clampScrollPx(123.45, SCROLLBACK, CELL_H)).toBe(123.45)
    expect(clampScrollPx(0, SCROLLBACK, CELL_H)).toBe(0)
  })

  it('negative / non-finite input collapses to the bottom', () => {
    expect(clampScrollPx(-50, SCROLLBACK, CELL_H)).toBe(0)
    expect(clampScrollPx(Number.NaN, SCROLLBACK, CELL_H)).toBe(0)
    expect(clampScrollPx(Number.POSITIVE_INFINITY, SCROLLBACK, CELL_H)).toBe(0)
  })
})

describe('computeScrollbarThumb', () => {
  it('proportional thumb pinned to the bottom at scrollPx=0', () => {
    const t = computeScrollbarThumb(0, SCROLLBACK, TOTAL, VIEWPORT, CELL_H)
    expect(t).not.toBeNull()
    expect(t!.heightFrac).toBeCloseTo(VIEWPORT / TOTAL)
    expect(t!.topFrac + t!.heightFrac).toBeCloseTo(1)
  })

  it('thumb at the top at scrollPx=max', () => {
    const t = computeScrollbarThumb(
      maxScrollPx(SCROLLBACK, CELL_H),
      SCROLLBACK,
      TOTAL,
      VIEWPORT,
      CELL_H,
    )
    expect(t!.topFrac).toBe(0)
  })

  it('null when there is nothing to scroll', () => {
    expect(computeScrollbarThumb(0, 0, VIEWPORT, VIEWPORT, CELL_H)).toBeNull()
    expect(computeScrollbarThumb(0, SCROLLBACK, TOTAL, VIEWPORT, 0)).toBeNull()
  })

  it('tiny viewport/huge history clamps to the minimum thumb height', () => {
    const t = computeScrollbarThumb(0, 100_000, 100_024, 24, CELL_H, 0.05)
    expect(t!.heightFrac).toBe(0.05)
  })
})

describe('scrollPxFromThumbTopFrac', () => {
  it('round-trips computeScrollbarThumb positions', () => {
    for (const px of [0, 137.5, 760, maxScrollPx(SCROLLBACK, CELL_H)]) {
      const t = computeScrollbarThumb(px, SCROLLBACK, TOTAL, VIEWPORT, CELL_H)!
      expect(
        scrollPxFromThumbTopFrac(t.topFrac, t.heightFrac, SCROLLBACK, CELL_H),
      ).toBeCloseTo(px)
    }
  })

  it('clamps drags past either end of the track', () => {
    const heightFrac = 0.24
    expect(scrollPxFromThumbTopFrac(-0.5, heightFrac, SCROLLBACK, CELL_H)).toBe(
      maxScrollPx(SCROLLBACK, CELL_H),
    )
    expect(scrollPxFromThumbTopFrac(2, heightFrac, SCROLLBACK, CELL_H)).toBe(0)
  })
})

describe('anchorScrollPx — pin the view while scrolled up', () => {
  // cellHeight 20, scrollback 100 rows → max scrollPx 2000.
  it('at the bottom (scrollPx 0) keeps following live output', () => {
    expect(anchorScrollPx(0, 5, 100, 20)).toBe(0)
  })

  it('scrolled up: grows by exactly the appended height', () => {
    // 3 rows append below the view → +60px keeps the same content
    // under the cursor.
    expect(anchorScrollPx(400, 3, 100, 20)).toBe(460)
  })

  it('clamps to the top of the (grown) scrollback', () => {
    // Near the top: a large resync backlog cannot push past max.
    expect(anchorScrollPx(1990, 50, 100, 20)).toBe(2000)
  })

  it('no growth → no movement', () => {
    expect(anchorScrollPx(400, 0, 100, 20)).toBe(400)
  })
})
