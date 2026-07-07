// Style System P3 — pure selection→resolution logic.
//
// A user's Style selection is (styleId, paletteId, schemeMode, gapsPreset).
// This module resolves it against the generated style registry into the
// concrete (style, palette, scheme, gaps) tuple that gets stamped on
// <html> and pushed into the Kessel terminal bridge. Pure functions,
// no DOM/store access — unit-tested in style-resolve.test.ts.

import {
  DEFAULT_STYLE_ID,
  getStyle,
  type StyleMeta,
  type StylePaletteMeta,
  type StyleScheme,
} from '@/styles.generated'
import type { KesselColorsConfig } from '@/kessel/config'
import type { StyleTerminalColors } from '@/styles.generated'

/** The user's scheme MODE — 'auto' follows the OS appearance. */
export type SchemeMode = 'dark' | 'light' | 'auto'

/** The persisted/chosen selection (pre-resolution). `paletteId` is the
 *  palette the user PICKED — it may not support the resolved scheme, in
 *  which case resolution falls back per-style. `gapsPreset` is '' for
 *  the style's base density or one of `style.gapPresets`. */
export interface StyleSelection {
  styleId: string
  paletteId: string
  schemeMode: SchemeMode
  gapsPreset: string
}

/** Fully resolved selection — everything the DOM/bridge needs. */
export interface ResolvedStyle {
  style: StyleMeta
  resolvedScheme: StyleScheme
  resolvedPalette: StylePaletteMeta
  /** Validated against `style.gapPresets`; '' = base density. */
  gapsPreset: string
}

export const DEFAULT_SELECTION: StyleSelection = {
  styleId: 'square',
  paletteId: 'charcoal',
  schemeMode: 'dark',
  gapsPreset: '',
}

/** Parse an untrusted string (daemon settings / localStorage) into a
 *  SchemeMode, defaulting to 'dark'. */
export function parseSchemeMode(raw: string | null | undefined): SchemeMode {
  return raw === 'light' || raw === 'auto' ? raw : 'dark'
}

/** 'auto' → whatever the OS prefers; explicit modes pass through. */
export function resolveScheme(mode: SchemeMode, osPrefersLight: boolean): StyleScheme {
  if (mode === 'auto') return osPrefersLight ? 'light' : 'dark'
  return mode
}

/** Palette fallback chain: the chosen palette if it supports the
 *  resolved scheme → the style's per-scheme default → the style's
 *  overall default → the style's first palette. */
export function resolvePalette(
  style: StyleMeta,
  paletteId: string,
  resolvedScheme: StyleScheme,
): StylePaletteMeta {
  const find = (id: string | undefined): StylePaletteMeta | undefined =>
    id ? style.palettes.find((p) => p.id === id) : undefined

  const chosen = find(paletteId)
  if (chosen && chosen.schemes.includes(resolvedScheme)) return chosen

  const schemeDefault = find(style.defaultPalettes[resolvedScheme])
  if (schemeDefault) return schemeDefault

  return find(style.defaultPalette) ?? style.palettes[0]
}

/** Resolve a full selection. Unknown style ids fall back to the default
 *  style; unknown/unsupported palettes and gap presets fall back per the
 *  chains above. `osPrefersLight` is injected so this stays pure. */
export function resolveStyleSelection(
  sel: StyleSelection,
  osPrefersLight: boolean,
): ResolvedStyle {
  const style = getStyle(sel.styleId) ?? getStyle(DEFAULT_STYLE_ID)!
  const resolvedScheme = resolveScheme(sel.schemeMode, osPrefersLight)
  const resolvedPalette = resolvePalette(style, sel.paletteId, resolvedScheme)
  const gapsPreset =
    sel.gapsPreset && style.capabilities.gaps && style.gapPresets.includes(sel.gapsPreset)
      ? sel.gapsPreset
      : ''
  return { style, resolvedScheme, resolvedPalette, gapsPreset }
}

/** Registry terminal colors → KesselColorsConfig. The shapes match
 *  field-for-field except `palette`: the registry emits a plain
 *  readonly number[] while Kessel wants a 16-tuple, so we normalize
 *  (pad/truncate to exactly 16 entries — the generator always emits
 *  16, this is belt-and-braces for hand-edited community styles). */
export function toKesselColors(t: StyleTerminalColors): KesselColorsConfig {
  const palette = Array.from({ length: 16 }, (_, i) => t.palette[i] ?? 0)
  return {
    foreground: t.foreground,
    background: t.background,
    palette: palette as unknown as KesselColorsConfig['palette'],
    cursor: { text: t.cursor.text, cursor: t.cursor.cursor },
    selection: { text: t.selection.text, background: t.selection.background },
  }
}
