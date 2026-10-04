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
// built-in `themes/default.toml` and its `FONT_FAMILIES` table.

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

/**
 * The `font.family` names (daemon `FAMILIES`) with the daemon's CSS stacks
 * (`FONT_FAMILIES` in `schema.rs`, byte for byte) and whether each is
 * fixed-width. System stacks and fonts K2 already bundles; nothing loads from
 * the network (Z21). The renderer writes the stack from this table, never
 * the `stack` text the daemon sends.
 */
export const ZEN_FONT_TABLE = [
  ['system', '-apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif', false],
  ['rounded', 'ui-rounded, "SF Pro Rounded", -apple-system, BlinkMacSystemFont, system-ui, sans-serif', false],
  ['serif', 'ui-serif, "New York", Charter, Georgia, "Times New Roman", serif', false],
  ['mono', 'ui-monospace, "SF Mono", Menlo, Consolas, "DejaVu Sans Mono", monospace', true],
  ['meslo', '"MesloLGM Nerd Font", "MesloLGM Nerd Font Mono", Menlo, Monaco, "Courier New", monospace', true],
  ['jetbrains-mono', '"JetBrains Mono", ui-monospace, Menlo, Consolas, monospace', true],
  ['fira-code', '"Fira Code", ui-monospace, Menlo, Consolas, monospace', true],
  ['lilex', '"Lilex", ui-monospace, Menlo, Consolas, monospace', true],
] as const

export type ZenFont = (typeof ZEN_FONT_TABLE)[number][0]
export const ZEN_FONTS: readonly ZenFont[] = ZEN_FONT_TABLE.map((f) => f[0])
export const ZEN_FONT_STACKS = Object.fromEntries(ZEN_FONT_TABLE.map((f) => [f[0], f[1]])) as Record<ZenFont, string>
const MONO_FONTS: ReadonlySet<ZenFont> = new Set(ZEN_FONT_TABLE.filter((f) => f[2]).map((f) => f[0]))
/** The face terminals use when the font is proportional (daemon `TERMINAL_PARTNER`). */
export const ZEN_TERMINAL_PARTNER: ZenFont = 'meslo'

/** Older spellings a pre-bundle daemon sent, read as today's names. */
const FONT_ALIASES: Record<string, ZenFont> = {
  'MesloLGM Nerd Font': 'meslo',
  'JetBrains Mono': 'jetbrains-mono',
}

/** A known font family name (or an older alias), else null. */
export function parseZenFont(v: unknown): ZenFont | null {
  if (typeof v !== 'string') return null
  if ((ZEN_FONTS as readonly string[]).includes(v)) return v as ZenFont
  return FONT_ALIASES[v] ?? null
}

export function isZenMonoFont(f: ZenFont): boolean {
  return MONO_FONTS.has(f)
}

/** The terminals' face for font `f`: `f` itself when it is fixed-width. */
export function zenTerminalFont(f: ZenFont): ZenFont {
  return MONO_FONTS.has(f) ? f : ZEN_TERMINAL_PARTNER
}

export interface ZenNumToken {
  table: 'font' | 'shape'
  /** The TOML key (`line-height`). */
  key: string
  /** The key in the daemon's resolved JSON (`font.lineHeight`). */
  jsonKey: string
  /** The `--zen-*` property it writes. */
  cssVar: string
  min: number
  max: number
  unit: 'px' | ''
}

/** Numeric tokens with their inclusive ranges (daemon `NUM_TOKENS`). */
export const ZEN_NUM_TOKENS: readonly ZenNumToken[] = [
  { table: 'font', key: 'size', jsonKey: 'size', cssVar: '--zen-font-size', min: 12, max: 20, unit: 'px' },
  { table: 'font', key: 'line-height', jsonKey: 'lineHeight', cssVar: '--zen-line-height', min: 1.2, max: 1.8, unit: '' },
  { table: 'shape', key: 'radius', jsonKey: 'radius', cssVar: '--zen-radius', min: 0, max: 28, unit: 'px' },
  { table: 'shape', key: 'bubble-radius', jsonKey: 'bubble-radius', cssVar: '--zen-bubble-radius', min: 0, max: 28, unit: 'px' },
  { table: 'shape', key: 'gap', jsonKey: 'gap', cssVar: '--zen-gap', min: 4, max: 24, unit: 'px' },
  { table: 'shape', key: 'list-width', jsonKey: 'list-width', cssVar: '--zen-list-width', min: 240, max: 420, unit: 'px' },
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
 * scheme and a soft dark one. Byte-for-byte the daemon's built-in
 * `themes/default.toml`, so safe mode and a fresh install look the same.
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
export const ZEN_DEFAULT_FONT: ZenFont = 'system'
/** `font.*` and `shape.*` defaults, keyed `table.key` (TOML names). */
export const ZEN_DEFAULT_NUMS: Record<string, number> = {
  'font.size': 14,
  'font.line-height': 1.45,
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
// palette and one font. The font drives the Zen UI and the terminals shown
// in Zen. A terminal needs a fixed-width face, so a proportional choice
// (`system`, `rounded`, `serif`) pairs with `meslo` in terminals.

export const ZEN_TERMINAL_FONT_VAR = '--zen-terminal-font-family'

/** Terminal palette keys (xterm-style `ITheme` names), ANSI 0–15 in order
 *  after the five UI keys. The daemon spells them in TOML style
 *  (`cursor-text`, `bright-black`); see `zenTerminalKeyOf`. */
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

/** The daemon's `TERMINAL_TOKENS` name → this palette's key:
 *  `cursor-text` → `cursorAccent`, `bright-black` → `brightBlack`. An
 *  xterm-style key is accepted as is. Null for anything else. */
export function zenTerminalKeyOf(raw: string): ZenTerminalKey | null {
  const k = raw === 'cursor-text' ? 'cursorAccent' : raw.replace(/-([a-z])/g, (_, c: string) => c.toUpperCase())
  return (ZEN_TERMINAL_KEYS as readonly string[]).includes(k) ? (k as ZenTerminalKey) : null
}

/** `brightBlack` → `--zen-term-bright-black`. */
export function zenTerminalVar(key: ZenTerminalKey): string {
  return `--zen-term-${key.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}`
}

/** K2's default terminal palettes: the daemon's built-in `themes/default.toml`
 *  `[terminal.light]` / `[terminal.dark]`. */
export const ZEN_DEFAULT_TERMINAL: Record<'light' | 'dark', ZenTerminalPalette> = {
  light: {
    foreground: '#1f1c18',
    background: '#faf7f2',
    cursor: '#b4532a',
    cursorAccent: '#ffffff',
    selection: '#eadfce',
    black: '#2a2622',
    red: '#b8322a',
    green: '#3d7a3a',
    yellow: '#9a6a12',
    blue: '#2f5f9a',
    magenta: '#8a3f86',
    cyan: '#2a7a7a',
    white: '#d8d0c4',
    brightBlack: '#6f675d',
    brightRed: '#d0473e',
    brightGreen: '#4f944b',
    brightYellow: '#b8841f',
    brightBlue: '#3f76b8',
    brightMagenta: '#a3549f',
    brightCyan: '#3a9494',
    brightWhite: '#ffffff',
  },
  dark: {
    foreground: '#ece7e0',
    background: '#161514',
    cursor: '#e08a5f',
    cursorAccent: '#1a1410',
    selection: '#3a3530',
    black: '#262422',
    red: '#f07167',
    green: '#7fbf7a',
    yellow: '#e0b45a',
    blue: '#7aa7d8',
    magenta: '#c792c7',
    cyan: '#6fc1bd',
    white: '#d8d0c4',
    brightBlack: '#7d766d',
    brightRed: '#ff8a80',
    brightGreen: '#9ad694',
    brightYellow: '#f2cc7a',
    brightBlue: '#9cc2ec',
    brightMagenta: '#dcaadc',
    brightCyan: '#8fd6d2',
    brightWhite: '#ffffff',
  },
}

/** `[background] fit` (daemon `BACKGROUND_FITS`). */
export const ZEN_BACKGROUND_FITS = ['cover', 'contain', 'tile', 'center'] as const
export type ZenBackgroundFit = (typeof ZEN_BACKGROUND_FITS)[number]
export const ZEN_BACKGROUND_FIT_DEFAULT: ZenBackgroundFit = 'cover'
/** `[background] opacity`, 0–1; the canvas shows through the rest. */
export const ZEN_BACKGROUND_OPACITY_DEFAULT = 1
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
