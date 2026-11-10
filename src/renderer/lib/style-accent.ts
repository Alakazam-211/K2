// Accent colour choice (per client, per style) — pure.
//
// The user's accent for a style lives in localStorage
// (`k2.accent.<styleId>`): `yellow`, a `#rrggbb` custom colour, or nothing
// for the palette's own accent. This module turns that untrusted raw string
// into the concrete CSS values the store writes onto <html>. Pure
// functions, no DOM or storage access: stores/style.ts owns the side effects.
//
// Every derived value is plain #rrggbb, so index.html's pre-paint stamp can
// check each one before using it.

import type { StylePaletteMeta, StyleScheme } from '@/styles.generated'

/** `yellow` (Bezel's gold) or a #rrggbb custom colour. */
export type AccentChoice = string

interface OwnColors {
  accent: string
  hover: string
  soft: string
  onAccent: string
}

/** Bezel's gold, slot for slot, for a light or dark palette. The picker
 *  doesn't offer it by name: it is what a Porcelain user keeps on Veil
 *  (lib/style-id-migration). */
const YELLOW: Readonly<Record<StyleScheme, OwnColors>> = {
  dark: { accent: '#ffc700', hover: '#ffd54d', soft: '#ffe38a', onAccent: '#1a1400' },
  light: { accent: '#e6b400', hover: '#c79c00', soft: '#8a6d00', onAccent: '#1a1400' },
}

/** localStorage key for a style's accent choice. */
export function accentStorageKey(styleId: string): string {
  return `k2.accent.${styleId}`
}

/** Per-scheme pre-paint stamp: `{style, palette, vars}`, written by the store,
 *  read by index.html's bootstrap. Absent when the accent is the default. */
export const lsAccentStampFor = (scheme: StyleScheme): string => `k2.accent.stamp.${scheme}`

/** Every property an accent choice writes; all are cleared before stamping. */
export const ACCENT_VARS = [
  '--color-accent',
  '--color-accent-hover',
  '--color-accent-active',
  '--color-accent-soft',
  '--color-accent-text',
  '--color-on-accent',
  '--glass-accent',
] as const

type Rgb = readonly [number, number, number]

const WHITE: Rgb = [255, 255, 255]
const BLACK: Rgb = [0, 0, 0]
/** Dark text and knobs on a bright accent. */
const INK = '#141414'
/** AA for small text. */
const TEXT_CONTRAST = 4.5
/** White stays on a colour down to 3:1, the band K2's shipped blues sit in
 *  (#3b82f6 is 3.7:1, #0a84ff is 3.6:1). */
const WHITE_ON_ACCENT_MIN = 3

function parseHex(raw: string): Rgb | null {
  const m = /^#([0-9a-f]{6})$/i.exec(raw.trim())
  if (!m) return null
  return [0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16)) as unknown as Rgb
}

const toHex = (c: Rgb): string =>
  `#${c.map((v) => Math.round(v).toString(16).padStart(2, '0')).join('')}`

/** `amount` of `to` mixed into `from`, in sRGB. */
function mix(from: Rgb, to: Rgb, amount: number): Rgb {
  return from.map((v, i) => v + (to[i] - v) * amount) as unknown as Rgb
}

function luminance(c: Rgb): number {
  const [r, g, b] = c.map((v) => {
    const s = v / 255
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

function contrast(a: Rgb, b: Rgb): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return (hi + 0.05) / (lo + 0.05)
}

/** Validate an untrusted stored value. Anything else means "the default". */
export function parseAccentChoice(raw: string | null | undefined): AccentChoice | null {
  if (raw == null) return null
  if (raw === 'yellow') return raw
  return parseHex(raw) ? raw.trim().toLowerCase() : null
}

/** White or ink, whichever a filled accent should carry. */
export function onAccentFor(hex: string): string {
  const c = parseHex(hex)
  if (!c) return INK
  return contrast(c, WHITE) >= WHITE_ON_ACCENT_MIN ? '#ffffff' : INK
}

/** A palette surface as an opaque colour: hex directly, rgba() over `base`,
 *  and null for anything else (a var() or color-mix() swatch). */
function surfaceColor(raw: string, base: Rgb | null): Rgb | null {
  const hex = parseHex(raw)
  if (hex) return hex
  const m = /^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([\d.]+)\s*)?\)$/.exec(raw.trim())
  if (!m) return null
  const c: Rgb = [Number(m[1]), Number(m[2]), Number(m[3])]
  const alpha = m[4] === undefined ? 1 : Number(m[4])
  return base ? mix(base, c, alpha) : c
}

/**
 * The colour to use when the accent is small text: unchanged if it already
 * reads on the palette's grounds, else moved toward black (light palettes) or
 * white (dark) until it clears AA on all of them.
 */
export function accentTextFor(hex: string, palette: StylePaletteMeta, scheme: StyleScheme): string {
  const accent = parseHex(hex)
  if (!accent) return hex
  const base = surfaceColor(palette.swatch.bg, null)
  const grounds = [palette.swatch.bg, palette.swatch.surface, palette.swatch.elevated]
    .map((s) => surfaceColor(s, base))
    .filter((c): c is Rgb => c !== null)
  if (grounds.length === 0) grounds.push(scheme === 'light' ? WHITE : BLACK)
  const reads = (c: Rgb): boolean => grounds.every((g) => contrast(c, g) >= TEXT_CONTRAST)
  const target = scheme === 'light' ? BLACK : WHITE
  for (let t = 0; t <= 1; t += 0.02) {
    const candidate = mix(accent, target, t)
    if (reads(candidate)) return toHex(candidate)
  }
  return toHex(target)
}

/** The palette's own accent as #rrggbb. glass-next's is a var() reference to
 *  its one blue, which an override repaints, so it is named here. */
export function defaultAccentHex(palette: StylePaletteMeta): string {
  return parseHex(palette.swatch.accent) ? palette.swatch.accent : '#0a84ff'
}

/** The CSS values for an accent colour on a palette. */
export function deriveAccentVars(
  hex: string,
  palette: StylePaletteMeta,
  scheme: StyleScheme,
  own: Partial<OwnColors> = {},
): Record<string, string> {
  const base = parseHex(hex)
  if (!base) return {}
  const accent = toHex(base)
  // Same direction the palettes take: hover lifts on dark, deepens on light.
  const toward = scheme === 'light' ? BLACK : WHITE
  return {
    '--color-accent': accent,
    '--color-accent-hover': own.hover ?? toHex(mix(base, toward, 0.16)),
    '--color-accent-active': toHex(mix(base, BLACK, scheme === 'light' ? 0.28 : 0.16)),
    '--color-accent-soft': own.soft ?? toHex(mix(base, WHITE, 0.42)),
    '--color-accent-text': accentTextFor(accent, palette, scheme),
    '--color-on-accent': own.onAccent ?? onAccentFor(accent),
    '--glass-accent': accent,
  }
}

/** The vars for a stored choice on a palette; empty for the palette's own. */
export function accentVarsFor(
  choice: AccentChoice | null,
  palette: StylePaletteMeta,
  scheme: StyleScheme,
): Record<string, string> {
  if (choice === null) return {}
  if (choice === 'yellow') return deriveAccentVars(YELLOW[scheme].accent, palette, scheme, YELLOW[scheme])
  return deriveAccentVars(choice, palette, scheme)
}

/** The colour the accent well shows for a choice. */
export function accentFillHex(
  choice: AccentChoice | null,
  palette: StylePaletteMeta,
  scheme: StyleScheme,
): string {
  if (choice === null) return defaultAccentHex(palette)
  return choice === 'yellow' ? YELLOW[scheme].accent : choice
}
