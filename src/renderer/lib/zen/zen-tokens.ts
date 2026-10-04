// prd-zen-mode-v1 Z20, Z21, Z26, Z44 — Zen's own design tokens.
//
// Zen is a design system of its own, fully separate from Styles: it never
// maps onto a Style base and never reads `--color-*`, `--radius-*`,
// `--material-*`, `--motion-*` or `--inset-window`. Every token here becomes
// exactly one `--zen-*` custom property on the Zen root. Only the names in
// these tables are ever written: a key the table doesn't know is rejected,
// a value that doesn't parse (a colour that isn't a colour, a size out of
// range) is never passed through as CSS. The daemon's schema
// (`crates/k2-core/src/zen/schema.rs`) is the source of truth for names and
// ranges; `zen-theme-engine.test.ts` checks the two agree with the daemon's
// `default-zen.toml`.

/** Colour tokens, the same keys in `colors.light` and `colors.dark`.
 *  (Decision 9, 2026-10-04: no unread tracking, so `idle`, not `unread`.) */
export const ZEN_COLOR_TOKENS = [
  'canvas',
  'surface',
  'surface-raised',
  'border',
  'text',
  'text-muted',
  'accent',
  'accent-text',
  'bubble-me',
  'bubble-me-text',
  'bubble-agent',
  'bubble-agent-text',
  'working',
  'idle',
  'needs-you',
  'danger',
] as const
export type ZenColorToken = (typeof ZEN_COLOR_TOKENS)[number]

export const ZEN_FAMILIES = ['system', 'rounded', 'serif', 'mono'] as const
export type ZenFamily = (typeof ZEN_FAMILIES)[number]

/** System stacks and fonts K2 already bundles. No remote fonts (Z21). */
export const ZEN_FAMILY_STACKS: Record<ZenFamily, string> = {
  system: '-apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif',
  rounded: 'ui-rounded, "SF Pro Rounded", -apple-system, BlinkMacSystemFont, system-ui, sans-serif',
  serif: 'ui-serif, "New York", Georgia, "Times New Roman", serif',
  mono: '"MesloLGM Nerd Font", "JetBrains Mono", ui-monospace, Menlo, Monaco, monospace',
}

export interface ZenNumToken {
  table: 'type' | 'shape'
  key: string
  /** The `--zen-*` property it writes. */
  cssVar: string
  min: number
  max: number
  unit: 'px' | ''
}

/** Numeric tokens with their inclusive ranges (daemon `NUM_TOKENS`). */
export const ZEN_NUM_TOKENS: readonly ZenNumToken[] = [
  { table: 'type', key: 'size', cssVar: '--zen-font-size', min: 12, max: 20, unit: 'px' },
  { table: 'type', key: 'line-height', cssVar: '--zen-line-height', min: 1.2, max: 1.8, unit: '' },
  { table: 'shape', key: 'radius', cssVar: '--zen-radius', min: 0, max: 28, unit: 'px' },
  { table: 'shape', key: 'bubble-radius', cssVar: '--zen-bubble-radius', min: 0, max: 28, unit: 'px' },
  { table: 'shape', key: 'gap', cssVar: '--zen-gap', min: 4, max: 24, unit: 'px' },
  { table: 'shape', key: 'list-width', cssVar: '--zen-list-width', min: 240, max: 420, unit: 'px' },
]

export const ZEN_FONT_FAMILY_VAR = '--zen-font-family'

export function zenColorVar(token: ZenColorToken): string {
  return `--zen-${token}`
}

/** Every non-motion `--zen-*` property the theme engine may write. */
export const ZEN_THEME_VARS: readonly string[] = [
  ...ZEN_COLOR_TOKENS.map(zenColorVar),
  ZEN_FONT_FAMILY_VAR,
  ...ZEN_NUM_TOKENS.map((t) => t.cssVar),
]

export type ZenSchemeMode = 'auto' | 'light' | 'dark'

/**
 * K2's default Zen template theme (Z44): clean, smooth and simple. Solid
 * surfaces, no blur, no glass, generous spacing, one accent; a warm light
 * scheme and a soft dark one. Byte-for-byte the daemon's builtin
 * `default-zen.toml`, so safe mode and a fresh install look the same.
 */
export const ZEN_DEFAULT_COLORS: Record<'light' | 'dark', Record<ZenColorToken, string>> = {
  light: {
    canvas: '#faf7f2',
    surface: '#ffffff',
    'surface-raised': '#ffffff',
    border: '#e7e1d8',
    text: '#1f1c18',
    'text-muted': '#6f675d',
    accent: '#b4532a',
    'accent-text': '#ffffff',
    'bubble-me': '#b4532a',
    'bubble-me-text': '#ffffff',
    'bubble-agent': '#efe9e0',
    'bubble-agent-text': '#1f1c18',
    working: '#2f855a',
    idle: '#a39b90',
    'needs-you': '#c27c0e',
    danger: '#c53030',
  },
  dark: {
    canvas: '#161514',
    surface: '#1e1d1b',
    'surface-raised': '#262422',
    border: '#34312d',
    text: '#ece7e0',
    'text-muted': '#a39b90',
    accent: '#e08a5f',
    'accent-text': '#1a1410',
    'bubble-me': '#8f4a2c',
    'bubble-me-text': '#fff7f0',
    'bubble-agent': '#262422',
    'bubble-agent-text': '#ece7e0',
    working: '#5fbf8a',
    idle: '#7d766d',
    'needs-you': '#e0a43a',
    danger: '#f07167',
  },
}

export const ZEN_DEFAULT_SCHEME_MODE: ZenSchemeMode = 'auto'
export const ZEN_DEFAULT_FAMILY: ZenFamily = 'system'
/** `type.*` and `shape.*` defaults, keyed `table.key`. */
export const ZEN_DEFAULT_NUMS: Record<string, number> = {
  'type.size': 14,
  'type.line-height': 1.45,
  'shape.radius': 14,
  'shape.bubble-radius': 18,
  'shape.gap': 12,
  'shape.list-width': 300,
}

/** An RGBA colour: r, g, b 0–255, a 0–1. */
export type ZenRgba = [number, number, number, number]

/**
 * Parse a Zen colour the way the daemon does: `#rgb`, `#rgba`, `#rrggbb`,
 * `#rrggbbaa`, `rgb(r, g, b)` or `rgba(r, g, b, a)`. Anything else is null
 * and is never written as CSS.
 */
export function parseZenColor(raw: unknown): ZenRgba | null {
  if (typeof raw !== 'string') return null
  const s = raw.trim()
  if (s.startsWith('#')) {
    const hex = s.slice(1)
    if (!/^[0-9a-fA-F]+$/.test(hex)) return null
    const d = (i: number, n: number): number => Number.parseInt(hex.slice(i, i + n), 16)
    switch (hex.length) {
      case 3:
        return [d(0, 1) * 17, d(1, 1) * 17, d(2, 1) * 17, 1]
      case 4:
        return [d(0, 1) * 17, d(1, 1) * 17, d(2, 1) * 17, (d(3, 1) * 17) / 255]
      case 6:
        return [d(0, 2), d(2, 2), d(4, 2), 1]
      case 8:
        return [d(0, 2), d(2, 2), d(4, 2), d(6, 2) / 255]
      default:
        return null
    }
  }
  const m = /^(rgba?)\(([^()]*)\)$/i.exec(s)
  if (!m) return null
  const want = m[1].toLowerCase() === 'rgba' ? 4 : 3
  const parts = m[2].split(',').map((p) => p.trim())
  if (parts.length !== want) return null
  const out: ZenRgba = [0, 0, 0, 1]
  for (let i = 0; i < want; i++) {
    if (!/^-?\d+(\.\d+)?$/.test(parts[i])) return null
    const v = Number.parseFloat(parts[i])
    if (i < 3 && !(v >= 0 && v <= 255)) return null
    if (i === 3 && !(v >= 0 && v <= 1)) return null
    out[i] = v
  }
  return out
}

/** `rgb(...)` for an opaque colour (alpha composited away). */
export function rgbCss(c: ZenRgba): string {
  return `rgb(${Math.round(c[0])}, ${Math.round(c[1])}, ${Math.round(c[2])})`
}

/** `fg` over an opaque `bg`. */
export function compositeOver(fg: ZenRgba, bg: ZenRgba): ZenRgba {
  const a = fg[3]
  return [fg[0] * a + bg[0] * (1 - a), fg[1] * a + bg[1] * (1 - a), fg[2] * a + bg[2] * (1 - a), 1]
}

/** A numeric token value in range, else null. */
export function zenNumInRange(token: ZenNumToken, raw: unknown): number | null {
  return typeof raw === 'number' && Number.isFinite(raw) && raw >= token.min && raw <= token.max ? raw : null
}

// ── Theme bundles (Omarchy additions 1 and 5, 2026-10-04) ───────────────
// A theme is a bundle: tokens, an optional background image, a terminal
// palette and one font. The font token drives the Zen UI and the terminals
// shown in Zen. A terminal needs a fixed-width face, so a proportional
// choice (`system`, `rounded`, `serif`) keeps K2's mono stack in terminals.

/** The `font` token: a family preset or a font K2 bundles. No remote fonts. */
export const ZEN_FONTS = ['system', 'rounded', 'serif', 'mono', 'MesloLGM Nerd Font', 'JetBrains Mono'] as const
export type ZenFont = (typeof ZEN_FONTS)[number]

export const ZEN_FONT_STACKS: Record<ZenFont, string> = {
  ...ZEN_FAMILY_STACKS,
  'MesloLGM Nerd Font': '"MesloLGM Nerd Font", "JetBrains Mono", ui-monospace, Menlo, Monaco, monospace',
  'JetBrains Mono': '"JetBrains Mono", "MesloLGM Nerd Font", ui-monospace, Menlo, Monaco, monospace',
}

const MONO_FONTS: ReadonlySet<ZenFont> = new Set(['mono', 'MesloLGM Nerd Font', 'JetBrains Mono'])

export function isZenFont(v: unknown): v is ZenFont {
  return typeof v === 'string' && (ZEN_FONTS as readonly string[]).includes(v)
}

/** The terminals' face for font `f`: `f` itself when it is fixed-width. */
export function zenTerminalFontStack(f: ZenFont): string {
  return MONO_FONTS.has(f) ? ZEN_FONT_STACKS[f] : ZEN_FAMILY_STACKS.mono
}

export const ZEN_TERMINAL_FONT_VAR = '--zen-terminal-font-family'

/** Terminal palette keys (xterm-style), ANSI 0–15 in order after the five UI keys. */
export const ZEN_TERMINAL_UI_KEYS = ['foreground', 'background', 'cursor', 'cursorAccent', 'selection'] as const
export const ZEN_TERMINAL_ANSI_KEYS = [
  'black',
  'red',
  'green',
  'yellow',
  'blue',
  'magenta',
  'cyan',
  'white',
  'brightBlack',
  'brightRed',
  'brightGreen',
  'brightYellow',
  'brightBlue',
  'brightMagenta',
  'brightCyan',
  'brightWhite',
] as const
export const ZEN_TERMINAL_KEYS = [...ZEN_TERMINAL_UI_KEYS, ...ZEN_TERMINAL_ANSI_KEYS] as const
export type ZenTerminalKey = (typeof ZEN_TERMINAL_KEYS)[number]
/** A resolved terminal palette: every key, as CSS colours. */
export type ZenTerminalPalette = Record<ZenTerminalKey, string>

/** `brightBlack` → `--zen-term-bright-black`. */
export function zenTerminalVar(key: ZenTerminalKey): string {
  return `--zen-term-${key.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}`
}

/** K2's default terminal palettes for the default template theme. */
export const ZEN_DEFAULT_TERMINAL: Record<'light' | 'dark', ZenTerminalPalette> = {
  light: {
    foreground: '#1f1c18',
    background: '#ffffff',
    cursor: '#b4532a',
    cursorAccent: '#ffffff',
    selection: '#efe9e0',
    black: '#1f1c18',
    red: '#c53030',
    green: '#2f855a',
    yellow: '#a8670b',
    blue: '#2b6cb0',
    magenta: '#97266d',
    cyan: '#2c7a7b',
    white: '#e7e1d8',
    brightBlack: '#6f675d',
    brightRed: '#e53e3e',
    brightGreen: '#38a169',
    brightYellow: '#c27c0e',
    brightBlue: '#3182ce',
    brightMagenta: '#b83280',
    brightCyan: '#319795',
    brightWhite: '#faf7f2',
  },
  dark: {
    foreground: '#ece7e0',
    background: '#1e1d1b',
    cursor: '#e08a5f',
    cursorAccent: '#1a1410',
    selection: '#34312d',
    black: '#161514',
    red: '#f07167',
    green: '#5fbf8a',
    yellow: '#e0a43a',
    blue: '#7aa7e0',
    magenta: '#d38bc4',
    cyan: '#6cc4c4',
    white: '#ece7e0',
    brightBlack: '#7d766d',
    brightRed: '#ff8a80',
    brightGreen: '#7fd9a4',
    brightYellow: '#f2c46b',
    brightBlue: '#9cc2f0',
    brightMagenta: '#e6a8d8',
    brightCyan: '#8fdede',
    brightWhite: '#ffffff',
  },
}

/** Background scrim: how much canvas lies over a theme's image (readability). */
export const ZEN_BACKGROUND_DIM_MIN = 0.5
export const ZEN_BACKGROUND_DIM_MAX = 0.95
export const ZEN_BACKGROUND_DIM_DEFAULT = 0.8
/** Data URLs above this many characters are refused (about 12 MB of image). */
export const ZEN_BACKGROUND_MAX_CHARS = 16 * 1024 * 1024

const DATA_IMAGE = /^data:image\/(png|jpeg|webp|gif|avif);base64,[A-Za-z0-9+/]+={0,2}$/

/**
 * A theme background image this client will draw: only a base64 `data:`
 * image URL (png, jpeg, webp, gif, avif). The app's CSP allows `data:` and
 * `blob:` images only, and Zen loads nothing remote. Null for anything else.
 */
export function parseZenBackgroundSrc(raw: unknown): string | null {
  if (typeof raw !== 'string') return null
  const s = raw.trim()
  if (s.length > ZEN_BACKGROUND_MAX_CHARS) return null
  return DATA_IMAGE.test(s) ? s : null
}

/** Every `--zen-*` property a theme bundle adds (terminal face and palette). */
export const ZEN_BUNDLE_VARS: readonly string[] = [ZEN_TERMINAL_FONT_VAR, ...ZEN_TERMINAL_KEYS.map(zenTerminalVar)]
