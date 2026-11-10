// The accent choice: what a stored value means, and that the colours it
// derives stay readable on every palette.

import { describe, expect, it } from 'vitest'
import { STYLES, getStyle } from '@/styles.generated'
import {
  ACCENT_VARS,
  accentFillHex,
  accentTextFor,
  accentVarsFor,
  defaultAccentHex,
  onAccentFor,
  parseAccentChoice,
} from './style-accent'

function luminance(hex: string): number {
  const ch = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
  const [r, g, b] = ch.map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4))
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}
function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return (hi + 0.05) / (lo + 0.05)
}

const palette = (style: string, id: string) => getStyle(style)!.palettes.find((p) => p.id === id)!

describe('parseAccentChoice', () => {
  it('accepts yellow and a #rrggbb colour, lowercased', () => {
    expect(parseAccentChoice('yellow')).toBe('yellow')
    expect(parseAccentChoice('#FF00aa')).toBe('#ff00aa')
  })

  it.each([null, undefined, '', 'green', '#abc', '#12345g', 'rgb(1,2,3)', 'url(x)', '#ff00aa;color:red'])(
    'treats %s as no choice',
    (raw) => {
      expect(parseAccentChoice(raw)).toBeNull()
    },
  )
})

describe('onAccentFor', () => {
  it('keeps white on the blues K2 ships with white text', () => {
    expect(onAccentFor('#3b82f6')).toBe('#ffffff')
    expect(onAccentFor('#0a84ff')).toBe('#ffffff')
    expect(onAccentFor('#000000')).toBe('#ffffff')
  })

  it('goes dark on bright colours', () => {
    for (const c of ['#ffc700', '#e6b400', '#30d158', '#ffffff', '#f97316']) {
      expect([c, onAccentFor(c)]).toEqual([c, '#141414'])
      expect(contrast(c, onAccentFor(c))).toBeGreaterThanOrEqual(4.5)
    }
  })
})

describe('the yellow accent choice', () => {
  it("is Bezel's gold, slot for slot, on a dark palette", () => {
    const vars = accentVarsFor('yellow', palette('terminal', 'charcoal'), 'dark')
    expect(vars['--color-accent']).toBe('#ffc700')
    expect(vars['--color-accent-hover']).toBe('#ffd54d')
    expect(vars['--color-accent-soft']).toBe('#ffe38a')
    expect(vars['--color-on-accent']).toBe('#1a1400')
  })

  it("is Bezel's daylight gold on a light palette", () => {
    const vars = accentVarsFor('yellow', palette('glass', 'veil'), 'light')
    expect(vars['--color-accent']).toBe('#e6b400')
    expect(vars['--color-accent-hover']).toBe('#c79c00')
    expect(vars['--color-accent-soft']).toBe('#8a6d00')
    expect(vars['--color-on-accent']).toBe('#1a1400')
  })
})

describe('accent as small text', () => {
  const lightPalettes = STYLES.filter((s) => s.id !== 'bezel').flatMap((s) =>
    s.palettes.filter((p) => p.schemes.includes('light')).map((p) => ({ style: s.id, p })),
  )

  it('there are light palettes to check', () => {
    expect(lightPalettes.length).toBeGreaterThanOrEqual(3)
  })

  it.each(lightPalettes.map((x) => [x.style, x.p.id, x.p] as const))(
    'yellow on %s %s clears 4.5:1 on its solid grounds',
    (_s, _id, p) => {
      const text = accentVarsFor('yellow', p, 'light')['--color-accent-text']
      for (const ground of [p.swatch.bg, p.swatch.surface, p.swatch.elevated]) {
        if (ground.startsWith('#')) expect(contrast(text, ground)).toBeGreaterThanOrEqual(4.5)
      }
    },
  )

  it('leaves a colour alone when it already reads', () => {
    expect(accentTextFor('#ffc700', palette('terminal', 'charcoal'), 'dark')).toBe('#ffc700')
  })

  it('lifts a custom colour too dark for a dark palette', () => {
    const text = accentTextFor('#101030', palette('terminal', 'charcoal'), 'dark')
    expect(text).not.toBe('#101030')
    expect(contrast(text, '#1e1e1e')).toBeGreaterThanOrEqual(4.5)
  })
})

describe('accentVarsFor', () => {
  it('is empty for the default, so the palette stays in charge', () => {
    expect(accentVarsFor(null, palette('terminal', 'charcoal'), 'dark')).toEqual({})
  })

  it('writes only declared properties, each plain #rrggbb for the pre-paint check', () => {
    const vars = accentVarsFor('#ff00aa', palette('glass', 'veil'), 'light')
    for (const [name, value] of Object.entries(vars)) {
      expect(ACCENT_VARS).toContain(name)
      expect(value).toMatch(/^#[0-9a-f]{6}$/)
    }
    expect(Object.keys(vars).sort()).toEqual([...ACCENT_VARS].sort())
  })

  it('moves hover the way the palettes do: lighter on dark, darker on light', () => {
    const dark = accentVarsFor('#3366cc', palette('terminal', 'charcoal'), 'dark')
    const light = accentVarsFor('#3366cc', palette('terminal', 'paper'), 'light')
    expect(luminance(dark['--color-accent-hover'])).toBeGreaterThan(luminance('#3366cc'))
    expect(luminance(light['--color-accent-hover'])).toBeLessThan(luminance('#3366cc'))
  })
})

describe('accentFillHex', () => {
  it('shows the choice, or the palette default as a real colour', () => {
    expect(accentFillHex('#ff00aa', palette('terminal', 'charcoal'), 'dark')).toBe('#ff00aa')
    expect(accentFillHex('yellow', palette('glass', 'veil'), 'light')).toBe('#e6b400')
    expect(accentFillHex(null, palette('terminal', 'charcoal'), 'dark')).toBe('#3b82f6')
    expect(accentFillHex(null, palette('terminal', 'yellow'), 'dark')).toBe('#ffc700')
  })

  it('never hands the colour input a var() reference', () => {
    for (const s of STYLES) {
      for (const p of s.palettes) expect(defaultAccentHex(p)).toMatch(/^#[0-9a-fA-F]{6}$/)
    }
  })
})

describe('the Yellow palettes', () => {
  it.each(['terminal', 'glass', 'glass-next'])('%s has three palettes, with Yellow as a dark one', (id) => {
    const style = getStyle(id)!
    expect(style.palettes).toHaveLength(3)
    const yellow = style.palettes.find((p) => p.id === 'yellow')!
    expect(yellow.name).toBe('Yellow')
    expect(yellow.schemes).toEqual(['dark'])
    expect(yellow.swatch.accent).toBe('#ffc700')
  })
})
