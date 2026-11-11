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
import type { AccentChoice } from '@/lib/style-accent'
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
  /** What each OS appearance shows under Auto, any of the style's palettes
   *  (`k2.palette.dark` / `k2.palette.light`). */
  autoPalettes?: Partial<Record<StyleScheme, string>>
}

/** Fully resolved selection — everything the DOM/bridge needs. */
export interface ResolvedStyle {
  style: StyleMeta
  resolvedScheme: StyleScheme
  resolvedPalette: StylePaletteMeta
  /** Validated against `style.gapPresets`; '' = base density. */
  gapsPreset: string
}

/**
 * The look for a client that has never picked one (Rosson, 2026-11-11):
 * Glass under Auto, Obsidian when the system is dark and Veil when it is
 * light, each palette with DEFAULT_PALETTE_ACCENTS. A saved choice is never
 * replaced by it. Keep index.html's pre-paint bootstrap in step.
 */
export const DEFAULT_SELECTION: Readonly<StyleSelection> = Object.freeze({
  styleId: 'glass',
  paletteId: 'obsidian',
  schemeMode: 'auto',
  gapsPreset: '',
  autoPalettes: Object.freeze({ dark: 'obsidian', light: 'veil' }),
})

/**
 * THE ONE PLACE that decides the default look's accents, per palette of the
 * default style (Glass). The default look and the one-time 0.46.11 switch
 * write exactly these, each to its own palette's key; a palette left out
 * keeps its own accent. Values are lib/style-accent choices: `yellow` or a
 * #rrggbb colour.
 *
 *   {}                      no accent written: every palette its own (now)
 *   { obsidian: 'yellow' }  Obsidian with Yellow, Veil and Yellow their own
 *
 * Change it here, then update `defaultAccent` in index.html's pre-paint
 * bootstrap to match: stores/style-accent.test.ts derives the values from
 * this constant and fails until they agree.
 */
export const DEFAULT_PALETTE_ACCENTS: Readonly<Partial<Record<string, AccentChoice>>> = Object.freeze({})

/** What the daemon's StyleSettings holds when nobody ever set it
 *  (app_settings.rs). Its echo is not a choice. */
export const DAEMON_STYLE_DEFAULT = { id: 'square', palette: 'charcoal', scheme: 'dark', gaps: '' } as const

/**
 * Retired style ids and where they went. The ONE alias table: every
 * incoming id goes through canonicalStyleSelection. `keepsTuning` says the
 * old style's density, dials and text weight mean the same thing on the
 * new one, so they carry over.
 *
 * On the retirement ledger (prd-themes-terminal-glass-v1 §7): remove once
 * Terminal is proven, together with lib/style-id-migration.ts and the copy
 * of this table in index.html's pre-paint bootstrap. styles/glass-classic is
 * the old Glass, compiled for one release as a fallback; its alias is what
 * makes it impossible to select, from the UI, the daemon or the CLI. Delete
 * the package and the two Glass aliases the release after new Glass is proven.
 */
export const STYLE_ID_ALIASES: ReadonlyMap<string, StyleAlias> = new Map<string, StyleAlias>([
  ['square', { to: 'terminal', keepsTuning: true }],
  ['bezel', { to: 'glass', keepsTuning: false, palettes: { dark: 'yellow', light: 'veil' }, accentOnLight: 'yellow' }],
  ['glass-next', { to: 'glass', keepsTuning: true }],
  ['glass-classic', { to: 'glass', keepsTuning: true }],
])

export interface StyleAlias {
  to: string
  keepsTuning: boolean
  /** Where a retired style's palettes went, by scheme. */
  palettes?: Record<StyleScheme, string>
  /** The accent that keeps a retired light palette's colour; set once, for
   *  someone who used that palette (light or auto), by lib/style-id-migration. */
  accentOnLight?: string
}

/** A selection with any retired style id replaced. A palette the new style
 *  doesn't have falls to that style's default for the scheme, the scheme
 *  mode always carries over, and gaps carry over only under `keepsTuning`.
 *  A selection on a current id comes back untouched. */
export function canonicalStyleSelection(sel: StyleSelection): StyleSelection {
  const alias = STYLE_ID_ALIASES.get(sel.styleId)
  if (!alias) return sel
  const target = getStyle(alias.to)
  const paletteExists = target?.palettes.some((p) => p.id === sel.paletteId) ?? false
  const schemeHint: StyleScheme = sel.schemeMode === 'light' ? 'light' : 'dark'
  const autoPalettes = alias.palettes ?? sel.autoPalettes
  return {
    styleId: alias.to,
    paletteId:
      alias.palettes?.[schemeHint] ??
      (paletteExists
        ? sel.paletteId
        : (target?.defaultPalettes[schemeHint] ?? target?.defaultPalette ?? sel.paletteId)),
    schemeMode: sel.schemeMode,
    gapsPreset: alias.keepsTuning ? sel.gapsPreset : '',
    ...(autoPalettes ? { autoPalettes } : {}),
  }
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

/**
 * What each OS appearance shows, for the per-scheme keys. Under Auto these are
 * the person's two picks. Under a fixed mode, that mode's side is the chosen
 * palette and the other side is left as it was, so Auto comes back unchanged;
 * a dark palette picked for the light side (or the reverse) also survives.
 */
export function paletteSlots(style: StyleMeta, sel: StyleSelection): Record<StyleScheme, string> {
  const slot = (scheme: StyleScheme): string => {
    const saved = style.palettes.find((p) => p.id === sel.autoPalettes?.[scheme])
    if (sel.schemeMode === scheme && !(saved && !saved.schemes.includes(scheme))) {
      return resolvePalette(style, sel.paletteId, scheme).id
    }
    return (saved ?? resolvePalette(style, sel.paletteId, scheme)).id
  }
  return { dark: slot('dark'), light: slot('light') }
}

/** Resolve a full selection. Retired style ids map through the alias
 *  table; unknown style ids fall back to the default style;
 *  unknown/unsupported palettes and gap presets fall back per the chains
 *  above. `osPrefersLight` is injected so this stays pure. */
export function resolveStyleSelection(
  selection: StyleSelection,
  osPrefersLight: boolean,
): ResolvedStyle {
  const sel = canonicalStyleSelection(selection)
  const style = getStyle(sel.styleId) ?? getStyle(DEFAULT_STYLE_ID)!
  const wanted = resolveScheme(sel.schemeMode, osPrefersLight)
  const autoPick =
    sel.schemeMode === 'auto' ? style.palettes.find((p) => p.id === sel.autoPalettes?.[wanted]) : undefined
  const resolvedPalette = autoPick ?? resolvePalette(style, sel.paletteId, wanted)
  // Auto can show a dark palette on a light OS: the palette decides.
  const resolvedScheme = resolvedPalette.schemes.includes(wanted) ? wanted : resolvedPalette.schemes[0]
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
