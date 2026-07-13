// WebGL text-weight (coverage gamma) helpers.
//
// The WebGL painter thins/fattens glyph edges with `pow(coverage, gamma)`
// in the fragment shader. The right value depends on polarity:
//   - dark terminal bg  → ~0.7 (fatten; light-on-dark reads thin)
//   - light terminal bg → ~1.2 (thin; dark-on-light reads bold)
//
// Precedence at paint time (highest first):
//   1. localStorage.K2SO_WEBGL_TEXT_GAMMA  — dev escape hatch
//   2. terminal-settings.textGamma         — user value (style selection writes this)
//
// See PLAN-text-gamma-per-style.md / LEARNINGS-webgl-scroll.md §chonky.

import type { StyleMeta, StylePaletteMeta } from '@/styles.generated'

/** Dark-theme preset (fatten). */
export const TEXT_GAMMA_DARK = 0.7
/** Light-theme preset (thin). Also the store default for fresh installs. */
export const TEXT_GAMMA_LIGHT = 1.2
/** Inclusive clamp for both store writes and paint-time resolution. */
export const TEXT_GAMMA_MIN = 0.5
export const TEXT_GAMMA_MAX = 3

export function clampTextGamma(v: number): number {
  if (!Number.isFinite(v)) return TEXT_GAMMA_LIGHT
  return Math.min(TEXT_GAMMA_MAX, Math.max(TEXT_GAMMA_MIN, v))
}

/** Relative luminance of a 0xRRGGBB (or CSS hex) terminal background.
 *  sRGB → linear → WCAG relative luminance. Dark < 0.5 → fatten preset. */
export function relativeLuminance(bg: string | number): number {
  let r8: number
  let g8: number
  let b8: number
  if (typeof bg === 'number') {
    const n = bg >>> 0
    r8 = (n >> 16) & 0xff
    g8 = (n >> 8) & 0xff
    b8 = n & 0xff
  } else {
    const s = bg.trim()
    const hex = s.startsWith('#') ? s.slice(1) : s
    if (hex.length === 3) {
      r8 = parseInt(hex[0] + hex[0], 16)
      g8 = parseInt(hex[1] + hex[1], 16)
      b8 = parseInt(hex[2] + hex[2], 16)
    } else if (hex.length === 6) {
      r8 = parseInt(hex.slice(0, 2), 16)
      g8 = parseInt(hex.slice(2, 4), 16)
      b8 = parseInt(hex.slice(4, 6), 16)
    } else {
      return 0 // treat unparseable as dark
    }
    if (![r8, g8, b8].every((c) => Number.isFinite(c))) return 0
  }
  const lin = (c: number): number => {
    const s = c / 255
    return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4)
  }
  return 0.2126 * lin(r8) + 0.7152 * lin(g8) + 0.0722 * lin(b8)
}

/** Rosson's calibration: dark bg → 0.7, light bg → 1.2. */
export function defaultTextGammaFor(bg: string | number): number {
  return relativeLuminance(bg) < 0.5 ? TEXT_GAMMA_DARK : TEXT_GAMMA_LIGHT
}

/**
 * Resolve the preset gamma for a style + its resolved palette.
 * Optional `style.terminalTextGamma` (if the style registry ever stamps
 * one) wins; otherwise derive from the palette's terminal background.
 */
export function resolveTextGammaPreset(
  style: StyleMeta & { terminalTextGamma?: number },
  palette: StylePaletteMeta,
): number {
  const explicit = style.terminalTextGamma
  if (typeof explicit === 'number' && Number.isFinite(explicit)) {
    return clampTextGamma(explicit)
  }
  return defaultTextGammaFor(palette.terminal.background)
}
