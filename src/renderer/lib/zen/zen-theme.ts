// prd-zen-mode-v1 Z20, Z21, Z26, Z44 — the Zen theme layer's seam.
//
// Zen is fully separate from Styles: its tokens are `--zen-*` custom
// properties set on the Zen root only (never `<html>`), with
// `data-zen-scheme` on that root. Zen never reads `--color-*`, `--radius-*`,
// `--material-*`, `--motion-*` or `--inset-window`.
//
// S4 ships K2's default Zen theme (warm light, soft dark) and the seam. The
// S5 theme engine (tokens from `zen.toml`, scheme, type, shape, bezier and
// animation with reduced motion) plugs in with `registerZenThemeEngine`; the
// root calls `zenThemeFor(...)` and applies whatever comes back. Safe mode
// always uses the default (it never reads the user's files).

import type { ZenResolvedPage } from './zen-page'

export type ZenScheme = 'light' | 'dark'

export interface ZenThemeResult {
  scheme: ZenScheme
  vars: Record<string, string>
}

const SHARED: Record<string, string> = {
  '--zen-font-family': '-apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif',
  '--zen-font-size': '14px',
  '--zen-line-height': '1.45',
  '--zen-radius': '14px',
  '--zen-bubble-radius': '18px',
  '--zen-gap': '12px',
  '--zen-list-width': '320px',
}

/** K2's default Zen theme (Z44): solid surfaces, one accent, no glass. */
export const ZEN_DEFAULT_THEME: Record<ZenScheme, Record<string, string>> = {
  light: {
    ...SHARED,
    '--zen-canvas': '#f6f1ea',
    '--zen-surface': '#fffaf4',
    '--zen-surface-raised': '#ffffff',
    '--zen-border': '#e6ddd1',
    '--zen-text': '#2b2620',
    '--zen-text-muted': '#7a6f62',
    '--zen-accent': '#c2662d',
    '--zen-accent-text': '#ffffff',
    '--zen-bubble-me': '#c2662d',
    '--zen-bubble-me-text': '#ffffff',
    '--zen-bubble-agent': '#efe7dc',
    '--zen-bubble-agent-text': '#2b2620',
    '--zen-working': '#3f8f6b',
    '--zen-unread': '#c2662d',
    '--zen-needs-you': '#b8862b',
    '--zen-danger': '#b3412f',
  },
  dark: {
    ...SHARED,
    '--zen-canvas': '#1c1a18',
    '--zen-surface': '#242120',
    '--zen-surface-raised': '#2d2a27',
    '--zen-border': '#3a3632',
    '--zen-text': '#eee7de',
    '--zen-text-muted': '#a59a8d',
    '--zen-accent': '#e08a52',
    '--zen-accent-text': '#1c1a18',
    '--zen-bubble-me': '#e08a52',
    '--zen-bubble-me-text': '#1c1a18',
    '--zen-bubble-agent': '#2f2b28',
    '--zen-bubble-agent-text': '#eee7de',
    '--zen-working': '#6cc49b',
    '--zen-unread': '#e08a52',
    '--zen-needs-you': '#e0b25a',
    '--zen-danger': '#e0705c',
  },
}

/** What a theme engine gets: the resolved page (null in safe mode) and this
 *  computer's `prefers-color-scheme` (Z26: independent of the Style). */
export type ZenThemeEngine = (input: { page: ZenResolvedPage | null; systemScheme: ZenScheme }) => ZenThemeResult

export const defaultZenThemeEngine: ZenThemeEngine = ({ systemScheme }) => ({
  scheme: systemScheme,
  vars: { ...ZEN_DEFAULT_THEME[systemScheme] },
})

let engine: ZenThemeEngine = defaultZenThemeEngine

/** S5 plug-in point. Returns the unregister (back to the default). */
export function registerZenThemeEngine(next: ZenThemeEngine): () => void {
  engine = next
  return () => {
    if (engine === next) engine = defaultZenThemeEngine
  }
}

/** The theme for the Zen root. `safe` always gets K2's default. */
export function zenThemeFor(page: ZenResolvedPage | null, systemScheme: ZenScheme, safe: boolean): ZenThemeResult {
  if (safe) return defaultZenThemeEngine({ page: null, systemScheme })
  return engine({ page, systemScheme })
}

export function systemZenScheme(): ZenScheme {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return 'light'
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}
