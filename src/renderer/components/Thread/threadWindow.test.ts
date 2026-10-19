// The chat render window's rules (threadWindow.ts, 0.46.2): which rows are
// drawn, how the window grows and drops, what the spacers stand in for.
import { describe, expect, it } from 'vitest'
import {
  growWindow,
  pinWindow,
  resolveWindow,
  spacerAbove,
  spacerBelow,
  WINDOW_EST_ROW_PX,
  WINDOW_MAX_ROWS,
  WINDOW_ROWS,
  WINDOW_STEP,
} from './threadWindow'

const keys = (n: number): string[] => Array.from({ length: n }, (_, i) => `k${i}`)

describe('the window sizes (Rosson 2026-10-18, as on the phone)', () => {
  it('opens at 30, grows by 20, at most 50', () => {
    expect([WINDOW_ROWS, WINDOW_STEP, WINDOW_MAX_ROWS]).toEqual([30, 20, 50])
  })
})

describe('resolveWindow / pinWindow', () => {
  it('no window: the newest 30 (all of a short chat)', () => {
    expect(resolveWindow(keys(500), null)).toEqual({ lo: 470, hi: 500 })
    expect(resolveWindow(keys(12), null)).toEqual({ lo: 0, hi: 12 })
    expect(resolveWindow([], null)).toEqual({ lo: 0, hi: 0 })
  })

  it('is pinned by key: arrivals and an older page do not move it', () => {
    const k = keys(100)
    const win = pinWindow(k, { lo: 40, hi: 70 })
    expect(win).toEqual({ lo: 'k40', hi: 'k69' })
    // Five arrivals at the end, ten older rows at the start.
    const grown = [...Array.from({ length: 10 }, (_, i) => `old${i}`), ...k, 'n1', 'n2', 'n3', 'n4', 'n5']
    expect(resolveWindow(grown, win)).toEqual({ lo: 50, hi: 80 })
  })

  it('null ends follow the first row in memory and the newest', () => {
    const k = keys(40)
    const win = pinWindow(k, { lo: 0, hi: 40 })
    expect(win).toEqual({ lo: null, hi: null })
    // An older page draws at once; so do arrivals.
    expect(resolveWindow(['a', 'b', ...k, 'z'], win)).toEqual({ lo: 0, hi: 43 })
  })

  it('a key no longer in memory falls back to the newest 30', () => {
    expect(resolveWindow(keys(100), { lo: 'gone', hi: null })).toEqual({ lo: 70, hi: 100 })
    expect(resolveWindow(keys(100), { lo: 'k50', hi: 'k10' })).toEqual({ lo: 70, hi: 100 })
  })
})

describe('growWindow', () => {
  it('grows the top by a step and, past 50, drops rows at the bottom', () => {
    expect(growWindow({ lo: 470, hi: 500 }, 500, 'top', { lo: 470, hi: 480 })).toEqual({ lo: 450, hi: 500 })
    expect(growWindow({ lo: 450, hi: 500 }, 500, 'top', { lo: 450, hi: 460 })).toEqual({ lo: 430, hi: 480 })
  })

  it('never drops a row on screen (the window may go a little over 50)', () => {
    expect(growWindow({ lo: 450, hi: 500 }, 500, 'top', { lo: 470, hi: 495 })).toEqual({ lo: 430, hi: 495 })
  })

  it('grows the bottom and drops at the top', () => {
    expect(growWindow({ lo: 100, hi: 150 }, 500, 'bottom', { lo: 140, hi: 150 })).toEqual({ lo: 120, hi: 170 })
  })

  it('nothing to add at an edge returns the same range', () => {
    const r = { lo: 0, hi: 30 }
    expect(growWindow(r, 30, 'top', r)).toBe(r)
    expect(growWindow(r, 30, 'bottom', r)).toBe(r)
  })
})

describe('spacers', () => {
  it('above: the measured rows; a row never drawn counts 0', () => {
    const k = keys(6)
    const h = new Map([
      ['k0', 110],
      ['k2', 50],
    ])
    expect(spacerAbove(k, 3, h)).toBe(160)
    expect(spacerAbove(k, 0, h)).toBe(0)
  })

  it('below: measured, else the estimate', () => {
    const k = keys(6)
    const h = new Map([['k5', 200]])
    expect(spacerBelow(k, 3, h)).toBe(WINDOW_EST_ROW_PX * 2 + 200)
    expect(spacerBelow(k, 6, h)).toBe(0)
  })
})
