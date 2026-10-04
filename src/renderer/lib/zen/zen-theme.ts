// prd-zen-mode-v1 Z20, Z21, Z22, Z26, Z44 — the Zen theme layer's seam.
//
// Zen is fully separate from Styles: its tokens are `--zen-*` custom
// properties set on the Zen root only (never `<html>`), with
// `data-zen-scheme` on that root. Zen never reads `--color-*`, `--radius-*`,
// `--material-*`, `--motion-*` or `--inset-window`.
//
// The default engine draws K2's default Zen template theme (`zen-tokens.ts`,
// `zen-motion.ts`): it is what safe mode always uses (safe mode never reads
// the user's files). The S5 engine (`zen-theme-engine.ts`: tokens from
// `zen.toml`, scheme, type, shape, bezier and animation) plugs in with
// `registerZenThemeEngine`; the root calls `zenThemeFor(...)` and applies
// whatever comes back. Reduced motion and reduced transparency are inputs,
// and both win over the theme.

import { create } from 'zustand'
import type { ZenResolvedPage } from './zen-page'
import { buildZenTheme } from './zen-theme-engine'
import type { ZenFont, ZenTerminalPalette } from './zen-tokens'

export type ZenScheme = 'light' | 'dark'

export interface ZenThemeResult {
  scheme: ZenScheme
  vars: Record<string, string>
  /** Keys the engine refused (unknown token names), for the console and tests. */
  rejected?: string[]
  /** The active theme bundle's name, when the daemon names one. */
  name?: string | null
  /** The one font token (UI and terminals). */
  font?: ZenFont
  /** The terminal palette for terminals shown in Zen (CSS colours). */
  terminal?: ZenTerminalPalette
  /** The page background image under a canvas scrim (`dim` 0.5–0.95), or
   *  null (none, or reduced transparency). Only `data:image` URLs. */
  background?: { src: string; dim: number } | null
}

/** What a theme engine gets: the resolved page (null in safe mode), this
 *  computer's `prefers-color-scheme` (Z26: independent of the Style), and
 *  its reduced-motion / reduced-transparency preferences. */
export interface ZenThemeInput {
  page: ZenResolvedPage | null
  systemScheme: ZenScheme
  reducedMotion?: boolean
  reducedTransparency?: boolean
}

export type ZenThemeEngine = (input: ZenThemeInput) => ZenThemeResult

/** K2's default Zen theme, ignoring the page. Stateless. */
export const defaultZenThemeEngine: ZenThemeEngine = (input) => buildZenTheme({ ...input, page: null })

let engine: ZenThemeEngine = defaultZenThemeEngine

/** S5 plug-in point. Returns the unregister (back to the default). */
export function registerZenThemeEngine(next: ZenThemeEngine): () => void {
  engine = next
  return () => {
    if (engine === next) engine = defaultZenThemeEngine
  }
}

/** The theme for the Zen root. `safe` always gets K2's default. */
export function zenThemeFor(
  page: ZenResolvedPage | null,
  systemScheme: ZenScheme,
  safe: boolean,
  prefs: { reducedMotion?: boolean; reducedTransparency?: boolean } = {},
): ZenThemeResult {
  if (safe) return defaultZenThemeEngine({ page: null, systemScheme, ...prefs })
  return engine({ page, systemScheme, ...prefs })
}

export function systemZenScheme(): ZenScheme {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return 'light'
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

/** `matchMedia(query).matches`, false where there is no matchMedia. */
export function zenMediaMatches(query: string): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false
  return window.matchMedia(query).matches
}

export const REDUCED_MOTION_QUERY = '(prefers-reduced-motion: reduce)'
export const REDUCED_TRANSPARENCY_QUERY = '(prefers-reduced-transparency: reduce)'

/**
 * The theme bundle this window's Zen root applied last (Omarchy 1 and 5),
 * for anything in Zen that can't read CSS variables: a terminal shown in Zen
 * takes `terminal` as its palette and `terminalFont` as its face. Null when
 * Zen isn't shown. Set by the Zen root only.
 */
export interface ZenAppliedTheme {
  name: string | null
  scheme: ZenScheme
  terminal: ZenTerminalPalette
  /** CSS font stack for terminals (the one `font` token, kept fixed-width). */
  terminalFont: string
}

export const useZenAppliedThemeStore = create<{ applied: ZenAppliedTheme | null }>(() => ({ applied: null }))
