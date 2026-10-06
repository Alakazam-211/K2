// prd-zen-freeform-chrome FC27 / FC46 (FC-T10) — what gives way when a
// row is too narrow, and the column clamp.

import { describe, expect, it } from 'vitest'
import { zenClampedMinWidths, zenColumnMinWidthCss, zenRowOverflow, ZEN_OVERFLOW_ORDER } from './zen-overflow'

describe('a crowded row (FC27)', () => {
  // The template band: switcher 128, toggle 84 (gap included), 120 px of
  // drag space; usage 108 and theme 98 are the optional extras.
  const row = (available: number) =>
    zenRowOverflow({ available, fixed: 128 + 84, optional: { usage: 108, 'theme-picker': 98 }, dragMin: 120 })

  it('hides usage first, then the theme control, then truncates the switcher; nothing required ever hides', () => {
    expect(ZEN_OVERFLOW_ORDER).toEqual(['usage', 'theme-picker'])
    expect(row(538)).toEqual({ hide: [], compactSwitcher: false })
    expect(row(537)).toEqual({ hide: ['usage'], compactSwitcher: false })
    expect(row(430)).toEqual({ hide: ['usage'], compactSwitcher: false })
    expect(row(429)).toEqual({ hide: ['usage', 'theme-picker'], compactSwitcher: false })
    expect(row(332)).toEqual({ hide: ['usage', 'theme-picker'], compactSwitcher: false })
    expect(row(331)).toEqual({ hide: ['usage', 'theme-picker'], compactSwitcher: true })
    // The returned list only ever names optional kinds.
    for (const w of [0, 100, 300, 500, 900]) {
      for (const k of row(w).hide) expect(ZEN_OVERFLOW_ORDER).toContain(k)
    }
  })

  it('an extra the page didn’t place is skipped, not counted', () => {
    expect(zenRowOverflow({ available: 400, fixed: 212, optional: { 'theme-picker': 98 }, dragMin: 120 })).toEqual({
      hide: ['theme-picker'],
      compactSwitcher: false,
    })
    // A column edge keeps no drag space.
    expect(zenRowOverflow({ available: 300, fixed: 212, optional: { usage: 80 }, dragMin: 0 })).toEqual({
      hide: [],
      compactSwitcher: false,
    })
  })
})

describe('the column clamp (FC27, FC46)', () => {
  it('at an 800 px window, three 400 px min widths scale down so every column fits', () => {
    const mins = [400, 400, 400]
    const gap = 12
    // The layout host: 800 px minus its own side padding.
    const width = 800 - 2 * gap
    const clamped = zenClampedMinWidths(mins, width, gap)
    expect(clamped.reduce((a, b) => a + b, 0) + 2 * gap).toBeLessThanOrEqual(width)
    expect(clamped.every((w) => w > 0 && w < 400)).toBe(true)
    // Pages that fit are unchanged.
    expect(zenClampedMinWidths([240, 360], 1200, gap)).toEqual([240, 360])
    expect(zenClampedMinWidths([0, 360], 300, gap)).toEqual([0, 288])
  })

  it('the CSS is min(its px, its share of the row)', () => {
    expect(zenColumnMinWidthCss([400, 400, 400], 2)).toBe('min(400px, calc((100% - 2 * var(--zen-gap)) * 0.333333))')
    expect(zenColumnMinWidthCss([240, 360], 0)).toBe('min(240px, calc((100% - 1 * var(--zen-gap)) * 0.4))')
    expect(zenColumnMinWidthCss([240, 0], 1)).toBe(0)
  })
})
