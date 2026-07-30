import { describe, expect, it } from 'vitest'

import {
  FALLBACK_SPAWN_COLS,
  FALLBACK_SPAWN_ROWS,
  measurePaneFit,
  MIN_FIT_COLS,
  MIN_FIT_ROWS,
} from './measurePaneFit'

// Canonical cell size used by TerminalPane tests / DOM probe defaults.
const CW = 8
const CH = 16

describe('measurePaneFit', () => {
  it('matches ResizeObserver math: (width-4)/cw × (height-4)/ch floored', () => {
    // avail 796×636 → 99×39
    const fit = measurePaneFit({ width: 800, height: 640 }, CW, CH)
    expect(fit).toEqual({ cols: 99, rows: 39 })
  })

  it('returns a realistic full-pane fit (not toy 120×40)', () => {
    // ~full window: 1556×996 with 8×16 cells → 194×62
    const fit = measurePaneFit({ width: 1556, height: 996 }, CW, CH)
    expect(fit).not.toBeNull()
    expect(fit!.cols).toBe(Math.floor((1556 - 4) / CW))
    expect(fit!.rows).toBe(Math.floor((996 - 4) / CH))
    expect(fit).not.toEqual({ cols: 120, rows: 40 })
  })

  it('returns null for zero-size rect (unmeasurable pane)', () => {
    expect(measurePaneFit({ width: 0, height: 0 }, CW, CH)).toBeNull()
    expect(measurePaneFit({ width: 800, height: 0 }, CW, CH)).toBeNull()
    expect(measurePaneFit({ width: 0, height: 640 }, CW, CH)).toBeNull()
  })

  it('returns null for null/undefined rect', () => {
    expect(measurePaneFit(null, CW, CH)).toBeNull()
    expect(measurePaneFit(undefined, CW, CH)).toBeNull()
  })

  it('returns null when cell metrics are not ready', () => {
    expect(measurePaneFit({ width: 800, height: 640 }, 0, CH)).toBeNull()
    expect(measurePaneFit({ width: 800, height: 640 }, CW, 0)).toBeNull()
    expect(measurePaneFit({ width: 800, height: 640 }, -1, CH)).toBeNull()
  })

  it('returns null when fit is below MIN_FIT_COLS / MIN_FIT_ROWS', () => {
    // cols = floor((80-4)/8) = 9 < 10
    expect(measurePaneFit({ width: 80, height: 640 }, CW, CH)).toBeNull()
    // rows = floor((40-4)/16) = 2 < 3
    expect(measurePaneFit({ width: 800, height: 40 }, CW, CH)).toBeNull()
  })

  it('accepts the exact MIN_FIT boundary', () => {
    // cols = 10, rows = 3 exactly
    const width = MIN_FIT_COLS * CW + 4
    const height = MIN_FIT_ROWS * CH + 4
    expect(measurePaneFit({ width, height }, CW, CH)).toEqual({
      cols: MIN_FIT_COLS,
      rows: MIN_FIT_ROWS,
    })
  })

  it('subtracts the 4px padding before flooring', () => {
    // Without -4: floor(100/8)=12; with -4: floor(96/8)=12 — pick a
    // size where the pad changes the result: width 99 → floor(95/8)=11,
    // floor(99/8)=12 without pad.
    const fit = measurePaneFit({ width: 99, height: 100 }, CW, CH)
    expect(fit).toEqual({
      cols: Math.floor((99 - 4) / CW),
      rows: Math.floor((100 - 4) / CH),
    })
    expect(fit!.cols).toBe(11)
  })

  it('documents the unmeasurable fallback constants (VT 80×24, not 120×40)', () => {
    expect(FALLBACK_SPAWN_COLS).toBe(80)
    expect(FALLBACK_SPAWN_ROWS).toBe(24)
    expect({ cols: FALLBACK_SPAWN_COLS, rows: FALLBACK_SPAWN_ROWS }).not.toEqual({
      cols: 120,
      rows: 40,
    })
  })
})
