import { describe, expect, it } from 'vitest'
import type { StyleMeta, StylePaletteMeta } from '@/styles.generated'
import {
  TEXT_GAMMA_DARK,
  TEXT_GAMMA_LIGHT,
  TEXT_GAMMA_MAX,
  TEXT_GAMMA_MIN,
  clampTextGamma,
  defaultTextGammaFor,
  relativeLuminance,
  resolveTextGammaPreset,
} from './text-gamma'

describe('clampTextGamma', () => {
  it('clamps to [0.5, 3]', () => {
    expect(clampTextGamma(0.1)).toBe(TEXT_GAMMA_MIN)
    expect(clampTextGamma(9)).toBe(TEXT_GAMMA_MAX)
    expect(clampTextGamma(1.2)).toBe(1.2)
  })
  it('falls back to light preset for non-finite', () => {
    expect(clampTextGamma(Number.NaN)).toBe(TEXT_GAMMA_LIGHT)
    expect(clampTextGamma(Number.POSITIVE_INFINITY)).toBe(TEXT_GAMMA_LIGHT)
  })
})

describe('relativeLuminance / defaultTextGammaFor', () => {
  it('classifies pure black as dark → 0.7', () => {
    expect(relativeLuminance(0x000000)).toBe(0)
    expect(defaultTextGammaFor(0x000000)).toBe(TEXT_GAMMA_DARK)
    expect(defaultTextGammaFor('#000')).toBe(TEXT_GAMMA_DARK)
  })
  it('classifies pure white as light → 1.2', () => {
    expect(relativeLuminance(0xffffff)).toBeCloseTo(1, 5)
    expect(defaultTextGammaFor(0xffffff)).toBe(TEXT_GAMMA_LIGHT)
    expect(defaultTextGammaFor('#ffffff')).toBe(TEXT_GAMMA_LIGHT)
  })
  it('classifies a typical dark terminal bg as dark', () => {
    // square/charcoal terminal bg from the registry (~0x0a0a0a-ish)
    expect(defaultTextGammaFor(0x100f0e)).toBe(TEXT_GAMMA_DARK)
    expect(defaultTextGammaFor(1052430)).toBe(TEXT_GAMMA_DARK) // bezel graphite
  })
  it('classifies a typical light terminal bg as light', () => {
    expect(defaultTextGammaFor(0xf7f5f0)).toBe(TEXT_GAMMA_LIGHT)
    expect(defaultTextGammaFor(16250610)).toBe(TEXT_GAMMA_LIGHT) // bezel porcelain
  })
})

describe('resolveTextGammaPreset', () => {
  const darkPal = {
    id: 'ink',
    name: 'Ink',
    schemes: ['dark' as const],
    swatch: {
      bg: '#000',
      surface: '#111',
      elevated: '#222',
      accent: '#f00',
      textPrimary: '#eee',
      border: '#333',
    },
    terminal: {
      foreground: 0xe0e0e0,
      background: 0x0a0a0a,
      palette: Array.from({ length: 16 }, (_, i) => i),
      cursor: { text: null, cursor: 0xe0e0e0 },
      selection: { text: null, background: 0x444444 },
    },
  } satisfies StylePaletteMeta

  const lightPal: StylePaletteMeta = {
    ...darkPal,
    id: 'chalk',
    name: 'Chalk',
    schemes: ['light'],
    terminal: { ...darkPal.terminal, background: 0xf5f5f5 },
  }

  const style: StyleMeta = {
    id: 'fake',
    name: 'Fake',
    author: 'test',
    description: '',
    defaultPalette: 'ink',
    defaultPalettes: { dark: 'ink', light: 'chalk' },
    capabilities: { gaps: false, backdrop: false, schemes: ['dark', 'light'] },
    gapPresets: [],
    dials: [],
    palettes: [darkPal, lightPal],
  }

  it('derives from palette terminal bg when no explicit override', () => {
    expect(resolveTextGammaPreset(style, darkPal)).toBe(TEXT_GAMMA_DARK)
    expect(resolveTextGammaPreset(style, lightPal)).toBe(TEXT_GAMMA_LIGHT)
  })

  it('honors style.terminalTextGamma when finite', () => {
    const withOverride = { ...style, terminalTextGamma: 1.5 }
    expect(resolveTextGammaPreset(withOverride, darkPal)).toBe(1.5)
    expect(resolveTextGammaPreset({ ...style, terminalTextGamma: 0.1 }, darkPal)).toBe(
      TEXT_GAMMA_MIN,
    )
  })
})
