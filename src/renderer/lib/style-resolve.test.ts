import { describe, expect, it } from 'vitest'
import { STYLES, getStyle, type StyleMeta } from '@/styles.generated'
import {
  DEFAULT_SELECTION,
  STYLE_ID_ALIASES,
  canonicalStyleSelection,
  paletteSlots,
  parseSchemeMode,
  resolvePalette,
  resolveScheme,
  resolveStyleSelection,
  toKesselColors,
  type StyleSelection,
} from './style-resolve'

// A synthetic style exercising every fallback edge independently of
// whatever first-party styles happen to ship in the registry. Only the
// fields resolvePalette touches need to be real.
const terminalStub = {
  foreground: 0xe0e0e0,
  background: 0x0a0a0a,
  palette: Array.from({ length: 16 }, (_, i) => i),
  cursor: { text: null, cursor: 0xe0e0e0 },
  selection: { text: null, background: 0x444444 },
}
const swatchStub = {
  bg: '#000',
  surface: '#111',
  elevated: '#222',
  accent: '#33f',
  textPrimary: '#eee',
  border: '#333',
}
const fake: StyleMeta = {
  id: 'fake',
  name: 'Fake',
  author: 'test',
  description: '',
  defaultPalette: 'ink',
  defaultPalettes: { dark: 'ink', light: 'chalk' },
  capabilities: { gaps: true, backdrop: false, schemes: ['dark', 'light'] },
  gapPresets: ['regular', 'spacious'],
  dials: [],
  palettes: [
    { id: 'ink', name: 'Ink', schemes: ['dark'], swatch: swatchStub, terminal: terminalStub },
    { id: 'chalk', name: 'Chalk', schemes: ['light'], swatch: swatchStub, terminal: terminalStub },
    { id: 'dusk', name: 'Dusk', schemes: ['dark'], swatch: swatchStub, terminal: terminalStub },
  ],
}

describe('resolveScheme', () => {
  it('passes explicit modes through regardless of the OS signal', () => {
    expect(resolveScheme('dark', true)).toBe('dark')
    expect(resolveScheme('dark', false)).toBe('dark')
    expect(resolveScheme('light', true)).toBe('light')
    expect(resolveScheme('light', false)).toBe('light')
  })

  it('auto follows the OS appearance', () => {
    expect(resolveScheme('auto', true)).toBe('light')
    expect(resolveScheme('auto', false)).toBe('dark')
  })
})

describe('parseSchemeMode', () => {
  it('accepts the three modes and defaults everything else to dark', () => {
    expect(parseSchemeMode('light')).toBe('light')
    expect(parseSchemeMode('auto')).toBe('auto')
    expect(parseSchemeMode('dark')).toBe('dark')
    expect(parseSchemeMode('neon')).toBe('dark')
    expect(parseSchemeMode('')).toBe('dark')
    expect(parseSchemeMode(null)).toBe('dark')
    expect(parseSchemeMode(undefined)).toBe('dark')
  })
})

describe('resolvePalette fallback matrix', () => {
  it('keeps the chosen palette when it supports the resolved scheme', () => {
    expect(resolvePalette(fake, 'dusk', 'dark').id).toBe('dusk')
    expect(resolvePalette(fake, 'chalk', 'light').id).toBe('chalk')
  })

  it('falls back to the per-scheme default when the chosen palette lacks the scheme', () => {
    // dusk is dark-only; light resolution jumps to defaultPalettes.light
    expect(resolvePalette(fake, 'dusk', 'light').id).toBe('chalk')
    // chalk is light-only; dark resolution jumps to defaultPalettes.dark
    expect(resolvePalette(fake, 'chalk', 'dark').id).toBe('ink')
  })

  it('falls back to defaultPalette when the per-scheme default is missing', () => {
    const noSchemeDefaults: StyleMeta = { ...fake, defaultPalettes: {} }
    expect(resolvePalette(noSchemeDefaults, 'chalk', 'dark').id).toBe('ink')
  })

  it('falls back to the first palette when even defaultPalette is bogus', () => {
    const broken: StyleMeta = { ...fake, defaultPalette: 'nope', defaultPalettes: {} }
    expect(resolvePalette(broken, 'nope-too', 'dark').id).toBe('ink')
  })

  it('unknown palette ids resolve to the per-scheme default', () => {
    expect(resolvePalette(fake, 'does-not-exist', 'dark').id).toBe('ink')
    expect(resolvePalette(fake, 'does-not-exist', 'light').id).toBe('chalk')
  })
})

describe('resolveStyleSelection', () => {
  it('resolves the registry default selection to itself (dark)', () => {
    const r = resolveStyleSelection(DEFAULT_SELECTION, false)
    expect(r.style.id).toBe('terminal')
    expect(r.resolvedScheme).toBe('dark')
    expect(r.resolvedPalette.id).toBe('charcoal')
    expect(r.gapsPreset).toBe('')
  })

  it('auto + OS-light flips terminal to its light default palette', () => {
    const r = resolveStyleSelection({ ...DEFAULT_SELECTION, schemeMode: 'auto' }, true)
    expect(r.resolvedScheme).toBe('light')
    expect(r.resolvedPalette.id).toBe('paper')
  })

  it('unknown style ids fall back to the default style', () => {
    const r = resolveStyleSelection({ ...DEFAULT_SELECTION, styleId: 'ghost' }, false)
    expect(r.style.id).toBe('terminal')
  })

  it('validates gap presets against the style and drops unknown ones', () => {
    const ok = resolveStyleSelection({ ...DEFAULT_SELECTION, gapsPreset: 'regular' }, false)
    expect(ok.gapsPreset).toBe('regular')
    const bad = resolveStyleSelection({ ...DEFAULT_SELECTION, gapsPreset: 'gigantic' }, false)
    expect(bad.gapsPreset).toBe('')
  })
})

describe('toKesselColors', () => {
  it('maps registry terminal colors onto the Kessel shape with a 16-entry palette', () => {
    const c = toKesselColors(terminalStub)
    expect(c.foreground).toBe(0xe0e0e0)
    expect(c.background).toBe(0x0a0a0a)
    expect(c.palette).toHaveLength(16)
    expect(c.palette[15]).toBe(15)
    expect(c.cursor).toEqual({ text: null, cursor: 0xe0e0e0 })
    expect(c.selection).toEqual({ text: null, background: 0x444444 })
  })

  it('pads short palettes to 16 entries', () => {
    const c = toKesselColors({ ...terminalStub, palette: [1, 2, 3] })
    expect(c.palette).toHaveLength(16)
    expect(c.palette[2]).toBe(3)
    expect(c.palette[3]).toBe(0)
  })
})

describe('retired style ids (the alias table)', () => {
  const sel = (over: Partial<StyleSelection>): StyleSelection => ({ ...DEFAULT_SELECTION, ...over })

  it('every alias points at a style that ships and is not hidden', () => {
    expect(STYLE_ID_ALIASES.size).toBeGreaterThan(0)
    for (const { to } of STYLE_ID_ALIASES.values()) {
      expect(getStyle(to)?.hidden, to).toBeUndefined()
      expect(getStyle(to), to).toBeDefined()
    }
  })

  it('square becomes terminal and keeps palette, scheme and gaps', () => {
    const r = resolveStyleSelection(
      sel({ styleId: 'square', paletteId: 'paper', schemeMode: 'light', gapsPreset: 'spacious' }),
      false,
    )
    expect([r.style.id, r.resolvedPalette.id, r.resolvedScheme, r.gapsPreset]).toEqual([
      'terminal',
      'paper',
      'light',
      'spacious',
    ])
  })

  it('Graphite lands on Glass Yellow, Porcelain on Glass Veil', () => {
    const dark = resolveStyleSelection(sel({ styleId: 'bezel', paletteId: 'graphite', schemeMode: 'dark' }), false)
    expect([dark.style.id, dark.resolvedPalette.id, dark.resolvedScheme]).toEqual(['glass', 'yellow', 'dark'])
    const light = resolveStyleSelection(sel({ styleId: 'bezel', paletteId: 'porcelain', schemeMode: 'light' }), false)
    expect([light.style.id, light.resolvedPalette.id, light.resolvedScheme]).toEqual(['glass', 'veil', 'light'])
  })

  it('bezel on auto stays auto and follows the OS', () => {
    const c = canonicalStyleSelection(sel({ styleId: 'bezel', paletteId: 'graphite', schemeMode: 'auto' }))
    expect(c.schemeMode).toBe('auto')
    expect(resolveStyleSelection(c, true).resolvedPalette.id).toBe('veil')
    expect(resolveStyleSelection(c, false).resolvedPalette.id).toBe('yellow')
  })

  it('bezel drops gaps, even a preset Glass would accept', () => {
    const c = canonicalStyleSelection(sel({ styleId: 'bezel', paletteId: 'graphite', gapsPreset: 'spacious' }))
    expect(c.gapsPreset).toBe('')
    expect(resolveStyleSelection(sel({ styleId: 'bezel', gapsPreset: 'spacious' }), false).gapsPreset).toBe('')
  })

  it('a current id comes back as the same object', () => {
    const s = sel({ styleId: 'glass', paletteId: 'obsidian' })
    expect(canonicalStyleSelection(s)).toBe(s)
  })

  it('Bezel still compiles, hidden, but a selection can never resolve to it', () => {
    expect(getStyle('bezel')?.hidden).toBe(true)
    expect(STYLES.filter((s) => s.hidden).map((s) => s.id)).toEqual(['bezel', 'glass-classic'])
    expect(resolveStyleSelection(sel({ styleId: 'bezel' }), false).style.id).toBe('glass')
  })

  it('old Glass still compiles, hidden, but a selection can never resolve to it', () => {
    expect(getStyle('glass-classic')?.hidden).toBe(true)
    expect(resolveStyleSelection(sel({ styleId: 'glass-classic', paletteId: 'veil', schemeMode: 'light' }), false).style.id).toBe('glass')
    expect(canonicalStyleSelection(sel({ styleId: 'glass-classic' })).styleId).toBe('glass')
  })

  it('a saved glass-next is Glass, with its palette and scheme', () => {
    const r = resolveStyleSelection(sel({ styleId: 'glass-next', paletteId: 'yellow', schemeMode: 'dark' }), false)
    expect([r.style.id, r.resolvedPalette.id, r.resolvedScheme]).toEqual(['glass', 'yellow', 'dark'])
    expect(getStyle('glass-next')).toBeUndefined()
  })
})

describe('Auto picks (any two of a style\'s palettes)', () => {
  const sel = (o: Partial<StyleSelection>): StyleSelection => ({ ...DEFAULT_SELECTION, ...o })
  const auto = (dark: string, light: string): StyleSelection =>
    sel({ styleId: 'terminal', paletteId: 'charcoal', schemeMode: 'auto', autoPalettes: { dark, light } })

  it('shows the pick for each OS appearance', () => {
    const s = auto('yellow', 'paper')
    expect(resolveStyleSelection(s, false).resolvedPalette.id).toBe('yellow')
    expect(resolveStyleSelection(s, true).resolvedPalette.id).toBe('paper')
  })

  it('a dark palette on a light OS renders as dark', () => {
    const r = resolveStyleSelection(auto('charcoal', 'yellow'), true)
    expect([r.resolvedPalette.id, r.resolvedScheme]).toEqual(['yellow', 'dark'])
  })

  it('a pick this style does not have falls back to its default', () => {
    const r = resolveStyleSelection(auto('obsidian', 'veil'), true)
    expect(r.resolvedPalette.id).toBe('paper')
  })

  it('a fixed mode ignores the picks and leaves the other side alone', () => {
    const style = getStyle('terminal')!
    const s = sel({ styleId: 'terminal', paletteId: 'yellow', schemeMode: 'dark', autoPalettes: { dark: 'charcoal', light: 'charcoal' } })
    expect(resolveStyleSelection(s, true).resolvedPalette.id).toBe('yellow')
    expect(paletteSlots(style, s)).toEqual({ dark: 'yellow', light: 'charcoal' })
  })

  it('a dark palette picked for the light side survives a fixed Light', () => {
    const style = getStyle('terminal')!
    const s = sel({ styleId: 'terminal', paletteId: 'paper', schemeMode: 'light', autoPalettes: { dark: 'charcoal', light: 'yellow' } })
    expect(resolveStyleSelection(s, false).resolvedPalette.id).toBe('paper')
    expect(paletteSlots(style, s)).toEqual({ dark: 'charcoal', light: 'yellow' })
  })
})
