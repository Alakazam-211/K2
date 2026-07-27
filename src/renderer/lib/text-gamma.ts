// WebGL text-weight (coverage gamma) helpers.
//
// The WebGL painter thins/fattens glyph edges with `pow(coverage, gamma)`
// in the fragment shader. The right value depends on polarity:
//   - dark terminal bg  → ~0.7 (fatten; light-on-dark reads thin)
//   - light terminal bg → ~1.05 (thin; dark-on-light reads bold)
//
// User overrides are PER-STYLE / PER-SCHEME (like style dials), stored
// under localStorage `k2.textGamma.<styleId>.<scheme>`. The Styles
// Settings section owns the UI; switching styles restores that style's
// saved weight (or the polarity preset if never adjusted).
//
// Precedence at paint time (highest first):
//   1. localStorage.K2SO_WEBGL_TEXT_GAMMA  — dev escape hatch
//   2. style-store textGamma               — effective value for the
//      active style (loaded from per-style storage or preset)
//
// See PLAN-text-gamma-per-style.md / docs/learnings/LEARNINGS-webgl-scroll.md §chonky.

import type { StyleMeta, StylePaletteMeta, StyleScheme } from '@/styles.generated'

/** Dark-theme preset (fatten). Not the clamp floor (0.5) — 0.7 is the feel default. */
export const TEXT_GAMMA_DARK = 0.7
/** Light-theme preset (thin). */
export const TEXT_GAMMA_LIGHT = 1.05
/** Inclusive clamp for store writes and paint-time resolution. */
export const TEXT_GAMMA_MIN = 0.5
export const TEXT_GAMMA_MAX = 3

export function clampTextGamma(v: number): number {
  if (!Number.isFinite(v)) return TEXT_GAMMA_LIGHT
  return Math.min(TEXT_GAMMA_MAX, Math.max(TEXT_GAMMA_MIN, v))
}

/** localStorage key for a user override of WebGL text weight.
 *  One value per (style, resolved scheme) — dark and light keep
 *  independent tweaks, matching polarity-dependent presets. */
export function textGammaStorageKey(styleId: string, scheme: StyleScheme): string {
  return `k2.textGamma.${styleId}.${scheme}`
}

/** Read a stored override; null when absent / unreadable / non-finite. */
export function readStoredTextGamma(styleId: string, scheme: StyleScheme): number | null {
  try {
    if (typeof localStorage === 'undefined') return null
    const raw = localStorage.getItem(textGammaStorageKey(styleId, scheme))
    if (raw == null || raw.trim() === '') return null
    const n = Number(raw)
    if (!Number.isFinite(n)) return null
    return clampTextGamma(n)
  } catch {
    return null
  }
}

/** Persist a per-style / per-scheme override. */
export function writeStoredTextGamma(
  styleId: string,
  scheme: StyleScheme,
  value: number,
): void {
  try {
    if (typeof localStorage === 'undefined') return
    localStorage.setItem(textGammaStorageKey(styleId, scheme), String(clampTextGamma(value)))
  } catch {
    // Privacy-mode / full storage — live value still applies via the store.
  }
}

/** Drop the override so the next resolve falls back to the preset. */
export function clearStoredTextGamma(styleId: string, scheme: StyleScheme): void {
  try {
    if (typeof localStorage === 'undefined') return
    localStorage.removeItem(textGammaStorageKey(styleId, scheme))
  } catch {
    // Best-effort.
  }
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

/** Rosson's calibration: dark bg → 0.7, light bg → 1.05. */
export function defaultTextGammaFor(bg: string | number): number {
  return relativeLuminance(bg) < 0.5 ? TEXT_GAMMA_DARK : TEXT_GAMMA_LIGHT
}

/**
 * Resolve the PRESET gamma for a style + its resolved palette
 * (ignoring user localStorage). Optional `style.terminalTextGamma`
 * wins; otherwise derive from the palette's terminal background.
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

/**
 * Effective gamma for a fully resolved style selection:
 * stored per-style/scheme override, else polarity/style preset.
 */
export function resolveEffectiveTextGamma(
  style: StyleMeta & { terminalTextGamma?: number },
  palette: StylePaletteMeta,
  scheme: StyleScheme,
): number {
  const stored = readStoredTextGamma(style.id, scheme)
  if (stored != null) return stored
  return resolveTextGammaPreset(style, palette)
}
