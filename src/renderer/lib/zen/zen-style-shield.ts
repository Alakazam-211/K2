// prd-zen-mode-v1 Z20 — the Styles shield: Zen never shows a Styles value.
//
// Zen reuses a few app components (the Thread's markdown body, its choice
// and secret cards, the app menu, the window buttons). Those read the
// Styles variables (`--color-*`, `--font-ui`, `--radius-*`, `--term-*`),
// so inside Zen they used to paint with the user's Style: with a dark
// Style, `.chat-markdown p { color: var(--color-text-primary) }` drew
// near-white text on Zen's light agent bubble.
//
// The fix is one mapping, not per-component patches: the Zen root re-points
// every Styles variable at a Zen token (`ZEN_STYLE_SHIELD`), and each
// message bubble re-points the text ones again at that bubble's own text
// colour (`zenBubbleShield`). Whatever a reused component reads inside the
// Zen subtree is then a Zen value, in both Zen schemes and under any Style.
//
// `zen-text-contrast.test.tsx` holds every `:root` variable of
// `styles.generated.css` to this table, so a new Styles token can't leak.

import { ZEN_TERMINAL_ANSI_KEYS, zenTerminalVar } from './zen-tokens'

const zen = (token: string): string => `var(--zen-${token})`
const mix = (token: string, pct: number): string => `color-mix(in srgb, var(--zen-${token}) ${pct}%, transparent)`

const TERM_VARS: Record<string, string> = {
  '--term-fg': `var(${zenTerminalVar('foreground')})`,
  '--term-bg': `var(${zenTerminalVar('background')})`,
  '--term-cursor': `var(${zenTerminalVar('cursor')})`,
  '--term-selection': `var(${zenTerminalVar('selection')})`,
  ...Object.fromEntries(ZEN_TERMINAL_ANSI_KEYS.map((k, i) => [`--term-ansi-${i}`, `var(${zenTerminalVar(k)})`])),
}

/**
 * Every Styles variable, as a Zen value. Set on the Zen root, under the
 * theme's own `--zen-*` values.
 */
export const ZEN_STYLE_SHIELD: Readonly<Record<string, string>> = {
  // Surfaces
  '--color-bg': zen('canvas'),
  '--color-bg-canvas': zen('canvas'),
  '--color-bg-surface': zen('surface'),
  '--color-bg-elevated': zen('surface-raised'),
  '--color-bg-inset': zen('surface'),
  '--color-bg-stripe': mix('text', 4),
  '--color-bg-hover': zen('surface-raised'),
  '--color-border': zen('border'),
  '--color-border-strong': mix('text', 25),
  // Text
  '--color-text': zen('text'),
  '--color-text-primary': zen('text'),
  '--color-text-secondary': zen('text'),
  '--color-text-muted': zen('text-muted'),
  '--color-code-inline': zen('text'),
  // Accent
  '--color-accent': zen('accent'),
  '--color-accent-hover': zen('accent'),
  '--color-accent-soft': zen('accent'),
  '--color-on-accent': zen('accent-text'),
  // Status
  '--color-status-working': zen('working'),
  '--color-status-working-soft': zen('working'),
  '--color-status-ok': zen('working'),
  '--color-status-ok-soft': zen('working'),
  '--color-status-ok-hover': zen('working'),
  '--color-status-success': zen('working'),
  '--color-status-success-soft': zen('working'),
  '--color-success': zen('working'),
  '--color-good': zen('working'),
  '--color-status-warn': zen('needs-you'),
  '--color-status-warn-soft': zen('needs-you'),
  '--color-status-warn-text': zen('needs-you'),
  '--color-status-warn-amber': zen('needs-you'),
  '--color-status-warn-amber-soft': zen('needs-you'),
  '--color-status-warn-amber-bright': zen('needs-you'),
  '--color-status-warning-soft': zen('needs-you'),
  '--color-status-error': zen('danger'),
  '--color-status-error-soft': zen('danger'),
  '--color-status-error-bright': zen('danger'),
  '--color-status-error-text': zen('danger'),
  '--color-danger': zen('danger'),
  '--color-danger-hover': zen('danger'),
  '--color-danger-active': zen('danger'),
  '--color-danger-muted': zen('danger'),
  '--color-bad': zen('danger'),
  '--color-neutral': zen('idle'),
  '--color-control-track-off': zen('border'),
  '--color-diff-add-text': zen('working'),
  '--color-diff-remove-text': zen('danger'),
  '--color-diff-modified-border': zen('working'),
  // Washes, overlays, scrollbars
  '--color-wash-1': mix('text', 5),
  '--color-wash-2': mix('text', 10),
  '--color-wash-3': mix('text', 20),
  '--color-overlay-soft-bg': mix('text', 6),
  '--color-overlay-soft-border': mix('text', 8),
  '--color-scrim': 'rgba(0, 0, 0, 0.32)',
  '--color-scrollbar-thumb': mix('text-muted', 30),
  '--color-scrollbar-thumb-hover': mix('text-muted', 50),
  '--color-scrollbar-thumb-strong': mix('text-muted', 45),
  '--color-scrollbar-thumb-strong-hover': mix('text-muted', 65),
  // Rings (Bezel's edge lines)
  '--color-ring-edge': zen('border'),
  '--color-ring-edge-strong': zen('border'),
  '--color-ring-gap': zen('canvas'),
  '--color-ring-hairline': zen('border'),
  '--color-ring-hairline-soft': zen('border'),
  '--color-ring-hairline-outer': zen('border'),
  '--color-ring-key-line': zen('border'),
  '--color-ring-key-shade': 'transparent',
  '--color-ring-shadow': 'transparent',
  '--divider-color': zen('border'),
  // Type and shape
  '--font-ui': zen('font-family'),
  '--font-display': zen('font-family'),
  '--radius-box': 'calc(var(--zen-radius) / 2)',
  '--radius-field': 'calc(var(--zen-radius) / 2)',
  '--radius-selector': 'calc(var(--zen-radius) / 2)',
  // Terminals
  ...TERM_VARS,
}

/** Which side of the conversation a bubble is on. */
export type ZenBubbleSide = 'me' | 'agent'

/**
 * The text variables again, for one message bubble: its own text colour
 * (`bubble-me-text` / `bubble-agent-text`) for body, headings, code and
 * table text; a softer mix of it for quotes; links in the agent's accent
 * (on my bubble, which is the accent, links take my text colour). Fills
 * (code, quotes, tables) are washes of the text colour so they read on
 * either bubble.
 */
export function zenBubbleShield(side: ZenBubbleSide): Record<string, string> {
  const text = side === 'me' ? 'bubble-me-text' : 'bubble-agent-text'
  return {
    '--color-text': zen(text),
    '--color-text-primary': zen(text),
    '--color-text-secondary': zen(text),
    '--color-text-muted': `color-mix(in srgb, var(--zen-${text}) 72%, transparent)`,
    '--color-code-inline': zen(text),
    '--color-accent': side === 'me' ? zen(text) : zen('accent'),
    '--color-bg': 'transparent',
    '--color-bg-elevated': mix(text, 10),
    '--color-bg-inset': mix(text, 8),
    '--color-bg-stripe': mix(text, 6),
    '--color-border': mix(text, 22),
  }
}

/**
 * Rules a variable can't carry, under the Zen root: placeholders, and the
 * text selection inside bubbles (the app's rule mixes the accent, which on
 * my bubble is the bubble itself).
 */
export const ZEN_SHIELD_CSS = `
[data-zen-root] ::placeholder { color: var(--zen-text-muted); opacity: 1; }
[data-zen-root] [data-zen-bubble="agent"] *::selection,
[data-zen-root] [data-zen-bubble="agent"]::selection { background: color-mix(in srgb, var(--zen-accent) 30%, transparent); color: var(--zen-bubble-agent-text); }
[data-zen-root] [data-zen-bubble="me"] *::selection,
[data-zen-root] [data-zen-bubble="me"]::selection { background: color-mix(in srgb, var(--zen-bubble-me-text) 35%, transparent); color: var(--zen-bubble-me-text); }
`
