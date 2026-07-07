import { describe, expect, it } from 'vitest'
import type { StyleMeta } from '@/styles.generated'
import {
  DEFAULT_SELECTION,
  parseSchemeMode,
  resolvePalette,
  resolveScheme,
  resolveStyleSelection,
  toKesselColors,
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
    expect(r.style.id).toBe('square')
    expect(r.resolvedScheme).toBe('dark')
    expect(r.resolvedPalette.id).toBe('charcoal')
    expect(r.gapsPreset).toBe('')
  })

  it('auto + OS-light flips square to its light default palette', () => {
    const r = resolveStyleSelection({ ...DEFAULT_SELECTION, schemeMode: 'auto' }, true)
    expect(r.resolvedScheme).toBe('light')
    expect(r.resolvedPalette.id).toBe('paper')
  })

  it('unknown style ids fall back to the default style', () => {
    const r = resolveStyleSelection({ ...DEFAULT_SELECTION, styleId: 'ghost' }, false)
    expect(r.style.id).toBe('square')
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
