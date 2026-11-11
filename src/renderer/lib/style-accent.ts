// Accent colour choice (per client, per palette) — pure.
//
// The user's accent for a palette lives in localStorage
// (`k2.accent.<styleId>.<paletteId>`): `yellow`, a `#rrggbb` custom colour,
// or nothing for that palette's own accent. Each palette keeps its own, so
// Obsidian, Veil and Yellow never share one. This module turns that untrusted raw string
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

/** localStorage key for a palette's accent choice. */
export function accentStorageKey(styleId: string, paletteId: string): string {
  return `k2.accent.${styleId}.${paletteId}`
}

/** The old key: one accent for every palette of a style. Read only to move
 *  it per palette, once (lib/style-id-migration migrateSharedAccentKeys). */
export function legacyAccentStorageKey(styleId: string): string {
  return `k2.accent.${styleId}`
}

/** The values the one-time 0.46.11 switch wrote to `k2.accent.glass`: the
 *  word, or Yellow's hex on a dark or light palette. Any case. */
export function isSwitchYellow(raw: string | null): boolean {
  if (raw === null) return false
  const v = raw.trim().toLowerCase()
  return v === 'yellow' || v === YELLOW.dark.accent || v === YELLOW.light.accent
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
  '--color-accent-fill',
  '--color-accent-fill-hover',
  '--color-accent-edge',
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
/** A filled control's label: AA with room for a 90% hover over a white card. */
const FILL_LABEL_CONTRAST = 5
/** A filled control against the page around it (WCAG 1.4.11). */
const FILL_EDGE_CONTRAST = 3

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
/** The palette's grounds (page, surface, elevated) as opaque colours; plain
 *  white or black when none can be read. */
function paletteGrounds(palette: StylePaletteMeta, scheme: StyleScheme): Rgb[] {
  const base = surfaceColor(palette.swatch.bg, null)
  const grounds = [palette.swatch.bg, palette.swatch.surface, palette.swatch.elevated]
    .map((s) => surfaceColor(s, base))
    .filter((c): c is Rgb => c !== null)
  if (grounds.length === 0) grounds.push(scheme === 'light' ? WHITE : BLACK)
  return grounds
}

export function accentTextFor(hex: string, palette: StylePaletteMeta, scheme: StyleScheme): string {
  const accent = parseHex(hex)
  if (!accent) return hex
  const grounds = paletteGrounds(palette, scheme)
  const reads = (c: Rgb): boolean => grounds.every((g) => contrast(c, g) >= TEXT_CONTRAST)
  const target = scheme === 'light' ? BLACK : WHITE
  for (let t = 0; t <= 1; t += 0.02) {
    const candidate = mix(accent, target, t)
    if (reads(candidate)) return toHex(candidate)
  }
  return toHex(target)
}

/** The palette's own accent as #rrggbb. Glass's is a var() reference to
 *  its one blue, which an override repaints, so it is named here. */
export function defaultAccentHex(palette: StylePaletteMeta): string {
  return parseHex(palette.swatch.accent) ? palette.swatch.accent : '#0a84ff'
}

/**
 * The accent as the fill under on-accent text (filled buttons, the selected
 * segment, count badges): unchanged when its label already reads, else
 * deepened toward black until white text clears AA with room to spare. The
 * hue stays; only a pale-enough colour to carry white text gets darker.
 */
export function accentFillFor(hex: string, onAccent: string): string {
  const accent = parseHex(hex)
  const ink = parseHex(onAccent)
  if (!accent || !ink) return hex
  // A dark label on a bright accent (onAccentFor's ink): the accent itself.
  if (luminance(ink) < luminance(accent)) return toHex(accent)
  for (let t = 0; t <= 1; t += 0.02) {
    const candidate = mix(accent, BLACK, t)
    if (contrast(candidate, ink) >= FILL_LABEL_CONTRAST) return toHex(candidate)
  }
  return toHex(BLACK)
}

/** How far a filled control's hover moves from its resting fill: the
 *  palettes' 78% deepening clears 1.3:1, so a custom accent matches it. */
const FILL_HOVER_STEP = 1.35

/**
 * The hover of a filled accent control: a visible step (1.35:1) from the
 * resting fill, moving away from the label so the label reads better on it.
 * A white label's fill deepens toward black; a dark ink's (a bright accent)
 * lightens toward white. A fill with no room that way (pure white under ink)
 * steps toward the label instead, as far as the label still reads at 4.5:1.
 * Black under white has no room either way and stays put.
 */
export function accentFillHoverFor(fillHex: string, onAccent: string): string {
  const fill = parseHex(fillHex)
  const ink = parseHex(onAccent)
  if (!fill || !ink) return fillHex
  const away = luminance(ink) > luminance(fill) ? BLACK : WHITE
  for (const toward of [away, away === BLACK ? WHITE : BLACK]) {
    for (let i = 1; i <= 50; i++) {
      const candidate = mix(fill, toward, i / 50)
      if (contrast(candidate, ink) < TEXT_CONTRAST) break
      if (contrast(candidate, fill) >= FILL_HOVER_STEP) return toHex(candidate)
    }
  }
  return toHex(fill)
}

/**
 * The edge of a filled accent control (the accent Button's border): the fill
 * itself when the fill stands out at 3:1 on every ground of the palette, else
 * the accent as text (`text`, from accentTextFor), which reads at 4.5:1 on all
 * of them. So a pale accent on a light palette (yellow on Paper) or a dark one
 * on a dark palette (navy on Charcoal) still has a visible boundary. Without
 * a custom accent the var is unset and the Button falls back to the fill.
 */
export function accentEdgeFor(fill: string, text: string, palette: StylePaletteMeta, scheme: StyleScheme): string {
  const c = parseHex(fill)
  if (!c) return fill
  const stands = paletteGrounds(palette, scheme).every((g) => contrast(c, g) >= FILL_EDGE_CONTRAST)
  return stands ? toHex(c) : text
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
  const onAccent = own.onAccent ?? onAccentFor(accent)
  const text = accentTextFor(accent, palette, scheme)
  const fill = accentFillFor(accent, onAccent)
  return {
    '--color-accent': accent,
    '--color-accent-hover': own.hover ?? toHex(mix(base, toward, 0.16)),
    '--color-accent-active': toHex(mix(base, BLACK, scheme === 'light' ? 0.28 : 0.16)),
    '--color-accent-soft': own.soft ?? toHex(mix(base, WHITE, 0.42)),
    '--color-accent-text': text,
    '--color-accent-fill': fill,
    '--color-accent-fill-hover': accentFillHoverFor(fill, onAccent),
    '--color-accent-edge': accentEdgeFor(fill, text, palette, scheme),
    '--color-on-accent': onAccent,
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
