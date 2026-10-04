// prd-zen-mode-v1 Z13, Z20–Z22, Z26, Z44, Z57 and the Omarchy additions
// (2026-10-04) — the S5 Zen theme engine.
//
// Turns the `theme` and `motion` blocks of `/cli/zen/get` into `--zen-*`
// custom properties for the Zen root (the `chrome` block goes to the native
// chrome owner, see `zen-chrome.ts`):
//   - a theme is a bundle: `{name, tokens, background?, terminal: {palette},
//     font}`. A daemon that sends the tokens flat (`{scheme, colors, type,
//     shape}`) is read the same way;
//   - only names in `zen-tokens.ts` / `zen-motion.ts` are written; an unknown
//     key is rejected (listed in `rejected`, never written);
//   - every value is parsed (a colour, a number in range, an enum, a bezier
//     of four numbers, a `data:image` URL) and rebuilt; nothing from the
//     file reaches CSS as text;
//   - a value that is present but doesn't parse keeps the last good value
//     for that key (else K2's default), so a bad save never blanks the
//     window; the daemon already serves its last good version and owns the
//     error text (Z13). A key the daemon leaves out is K2's default. While a
//     page loads (null), everything keeps its last good value: no flicker;
//   - `scheme = "auto"` follows this computer, not the Style (Z26);
//   - the `font` token drives the UI and the terminals in Zen;
//   - reduced motion wins: every duration `0ms`, every keyframe `none`;
//   - reduced transparency wins: a colour with alpha is composited onto the
//     canvas (and the canvas onto white or black), and the background image
//     is not drawn, so nothing is see-through.
// Registered once through the S4 plug-in point (`installZenThemeEngine`).

import {
  compositeOver,
  isZenFont,
  parseZenBackgroundSrc,
  parseZenColor,
  rgbCss,
  zenColorVar,
  zenNumInRange,
  zenTerminalFontStack,
  zenTerminalVar,
  ZEN_BACKGROUND_DIM_DEFAULT,
  ZEN_BACKGROUND_DIM_MAX,
  ZEN_BACKGROUND_DIM_MIN,
  ZEN_COLOR_TOKENS,
  ZEN_DEFAULT_COLORS,
  ZEN_DEFAULT_FAMILY,
  ZEN_DEFAULT_NUMS,
  ZEN_DEFAULT_SCHEME_MODE,
  ZEN_DEFAULT_TERMINAL,
  ZEN_FAMILIES,
  ZEN_FONT_FAMILY_VAR,
  ZEN_FONT_STACKS,
  ZEN_NUM_TOKENS,
  ZEN_TERMINAL_ANSI_KEYS,
  ZEN_TERMINAL_FONT_VAR,
  ZEN_TERMINAL_KEYS,
  type ZenColorToken,
  type ZenFamily,
  type ZenFont,
  type ZenRgba,
  type ZenSchemeMode,
  type ZenTerminalKey,
  type ZenTerminalPalette,
} from './zen-tokens'
import {
  animationVars,
  parseAnimationNode,
  resolveDefaultAnimations,
  ZEN_ANIMATION_NAMES,
  type ZenAnimationNode,
} from './zen-motion'
import { registerZenThemeEngine, type ZenThemeEngine, type ZenThemeInput, type ZenThemeResult } from './zen-theme'

const TOKEN_KEYS = new Set(['scheme', 'colors', 'type', 'shape'])
const BUNDLE_KEYS = new Set(['name', 'tokens', 'background', 'terminal', 'font'])
const SCHEME_KEYS = new Set(['light', 'dark'])
const TYPE_KEYS = new Set(['family', ...ZEN_NUM_TOKENS.filter((t) => t.table === 'type').map((t) => t.key)])
const SHAPE_KEYS = new Set(ZEN_NUM_TOKENS.filter((t) => t.table === 'shape').map((t) => t.key))
const MOTION_KEYS = new Set(['beziers', 'animations', 'reducedMotion'])
const BACKGROUND_KEYS = new Set(['url', 'data', 'dim'])
const TERMINAL_KEYS = new Set(['palette'])
const COLOR_SET = new Set<string>(ZEN_COLOR_TOKENS)
const TERMINAL_SET = new Set<string>(ZEN_TERMINAL_KEYS)
const ANIMATION_SET = new Set(ZEN_ANIMATION_NAMES)

const WHITE: ZenRgba = [255, 255, 255, 1]
const BLACK: ZenRgba = [0, 0, 0, 1]

/** Per-key last good values, kept by a stateful engine across reloads. */
export type ZenLastGood = Map<string, unknown>

function obj(v: unknown): Record<string, unknown> | null {
  return v !== null && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : null
}

function present(v: unknown): boolean {
  return v !== undefined && v !== null
}

function colorCss(c: ZenRgba): string {
  return c[3] < 1
    ? `rgba(${Math.round(c[0])}, ${Math.round(c[1])}, ${Math.round(c[2])}, ${Math.round(c[3] * 1000) / 1000})`
    : rgbCss(c)
}

/**
 * Build the Zen root's properties and the bundle's parts. Pure apart from
 * `lastGood`, which is read for fallbacks and updated with every value that
 * parsed.
 */
export function buildZenTheme(input: ZenThemeInput, lastGood?: ZenLastGood): ZenThemeResult {
  const rejected: string[] = []
  const loading = input.page === null
  const theme = obj(input.page?.theme) ?? {}
  const tokens = obj(theme.tokens) ?? theme
  const flat = tokens === theme
  const motion = obj(input.page?.motion) ?? {}
  const reducedMotion = input.reducedMotion === true
  const reducedTransparency = input.reducedTransparency === true

  /**
   * The value for `key`: `parsed` when it parsed; the last good one when the
   * raw value was present but bad, or while the page loads; else `fallback`.
   */
  function pick<T>(key: string, raw: unknown, parsed: T | null, fallback: T): T {
    if (parsed !== null) {
      lastGood?.set(key, parsed)
      return parsed
    }
    if ((loading || present(raw)) && lastGood && lastGood.has(key)) return lastGood.get(key) as T
    return fallback
  }

  for (const k of Object.keys(theme)) {
    if (!BUNDLE_KEYS.has(k) && !(flat && TOKEN_KEYS.has(k))) rejected.push(`theme.${k}`)
  }
  if (!flat) for (const k of Object.keys(tokens)) if (!TOKEN_KEYS.has(k)) rejected.push(`theme.tokens.${k}`)
  for (const k of Object.keys(motion)) if (!MOTION_KEYS.has(k)) rejected.push(`motion.${k}`)

  // Scheme (Z26).
  const rawMode = tokens.scheme
  const mode = pick<ZenSchemeMode>(
    'theme.scheme',
    rawMode,
    rawMode === 'auto' || rawMode === 'light' || rawMode === 'dark' ? rawMode : null,
    ZEN_DEFAULT_SCHEME_MODE,
  )
  const scheme = mode === 'auto' ? input.systemScheme : mode

  // Colours: only the active scheme is written; both are checked for names.
  const colors = obj(tokens.colors) ?? {}
  for (const k of Object.keys(colors)) if (!SCHEME_KEYS.has(k)) rejected.push(`colors.${k}`)
  for (const s of ['light', 'dark'] as const) {
    const table = obj(colors[s]) ?? {}
    for (const k of Object.keys(table)) if (!COLOR_SET.has(k)) rejected.push(`colors.${s}.${k}`)
  }
  const active = obj(colors[scheme]) ?? {}
  const rgba = {} as Record<ZenColorToken, ZenRgba>
  for (const tok of ZEN_COLOR_TOKENS) {
    rgba[tok] = pick<ZenRgba>(
      `colors.${scheme}.${tok}`,
      active[tok],
      parseZenColor(active[tok]),
      parseZenColor(ZEN_DEFAULT_COLORS[scheme][tok]) as ZenRgba,
    )
  }
  const canvas = rgba.canvas[3] < 1 ? compositeOver(rgba.canvas, scheme === 'dark' ? BLACK : WHITE) : rgba.canvas
  /** Reduced transparency: nothing see-through, composited on the canvas. */
  const solid = (c: ZenRgba): string => (c[3] < 1 ? rgbCss(compositeOver(c, canvas)) : rgbCss(c))
  const vars: Record<string, string> = {}
  for (const tok of ZEN_COLOR_TOKENS) {
    vars[zenColorVar(tok)] = reducedTransparency
      ? tok === 'canvas'
        ? rgbCss(canvas)
        : solid(rgba[tok])
      : colorCss(rgba[tok])
  }

  // Type and shape.
  const type = obj(tokens.type) ?? {}
  const shape = obj(tokens.shape) ?? {}
  for (const k of Object.keys(type)) if (!TYPE_KEYS.has(k)) rejected.push(`type.${k}`)
  for (const k of Object.keys(shape)) if (!SHAPE_KEYS.has(k)) rejected.push(`shape.${k}`)
  const family = pick<ZenFamily>(
    'type.family',
    type.family,
    (ZEN_FAMILIES as readonly string[]).includes(type.family as string) ? (type.family as ZenFamily) : null,
    ZEN_DEFAULT_FAMILY,
  )
  for (const tok of ZEN_NUM_TOKENS) {
    const table = tok.table === 'type' ? type : shape
    const key = `${tok.table}.${tok.key}`
    const n = pick<number>(key, table[tok.key], zenNumInRange(tok, table[tok.key]), ZEN_DEFAULT_NUMS[key])
    vars[tok.cssVar] = `${n}${tok.unit}`
  }

  // The one font token (Omarchy 5): UI and terminals. Without it, the
  // `type.family` preset.
  const fontObj = obj(theme.font)
  const fontRaw = fontObj ? (fontObj.family ?? fontObj.name) : theme.font
  const font: ZenFont = present(theme.font)
    ? pick<ZenFont>('font', fontRaw, isZenFont(fontRaw) ? fontRaw : null, family)
    : family
  vars[ZEN_FONT_FAMILY_VAR] = ZEN_FONT_STACKS[font]
  vars[ZEN_TERMINAL_FONT_VAR] = zenTerminalFontStack(font)

  // Terminal palette (Omarchy 1): every key, per scheme. Accepts a flat
  // palette, a `{light, dark}` pair, or ANSI 0–15 as an array.
  const terminal = obj(theme.terminal) ?? {}
  for (const k of Object.keys(terminal)) if (!TERMINAL_KEYS.has(k)) rejected.push(`terminal.${k}`)
  let pal: Record<string, unknown> = {}
  const rawPal = terminal.palette
  if (Array.isArray(rawPal)) {
    if (rawPal.length !== ZEN_TERMINAL_ANSI_KEYS.length) rejected.push('terminal.palette')
    else ZEN_TERMINAL_ANSI_KEYS.forEach((k, i) => (pal[k] = rawPal[i]))
  } else {
    const p = obj(rawPal) ?? {}
    const perScheme = obj(p.light) || obj(p.dark)
    pal = perScheme ? (obj(p[scheme]) ?? {}) : p
    for (const k of Object.keys(pal)) if (!TERMINAL_SET.has(k)) rejected.push(`terminal.palette.${k}`)
  }
  const termDefault = ZEN_DEFAULT_TERMINAL[scheme]
  const palette = {} as ZenTerminalPalette
  for (const k of ZEN_TERMINAL_KEYS) {
    const c = pick<ZenRgba>(
      `terminal.${scheme}.${k}`,
      pal[k],
      parseZenColor(pal[k]),
      parseZenColor(termDefault[k as ZenTerminalKey]) as ZenRgba,
    )
    const css = reducedTransparency ? solid(c) : colorCss(c)
    palette[k] = css
    vars[zenTerminalVar(k)] = css
  }

  // Background image (Omarchy 1): the page background under a canvas scrim
  // so text stays readable. Never with reduced transparency.
  const bgObj = obj(theme.background)
  if (bgObj) for (const k of Object.keys(bgObj)) if (!BACKGROUND_KEYS.has(k)) rejected.push(`background.${k}`)
  const bgRaw = bgObj ? (bgObj.data ?? bgObj.url) : theme.background
  const src = pick<string | false>(
    'background.src',
    bgRaw,
    present(bgRaw) ? parseZenBackgroundSrc(bgRaw) : false,
    false,
  )
  const dimRaw = bgObj?.dim
  const dim =
    typeof dimRaw === 'number' && dimRaw >= ZEN_BACKGROUND_DIM_MIN && dimRaw <= ZEN_BACKGROUND_DIM_MAX
      ? dimRaw
      : ZEN_BACKGROUND_DIM_DEFAULT
  const background = src && !reducedTransparency ? { src, dim } : null

  // Motion (Z22, Z57).
  const animations = obj(motion.animations) ?? {}
  for (const k of Object.keys(animations)) if (!ANIMATION_SET.has(k)) rejected.push(`motion.animations.${k}`)
  const defaults = resolveDefaultAnimations()
  for (const name of ZEN_ANIMATION_NAMES) {
    const raw = animations[name]
    const node = pick<ZenAnimationNode>(`animation.${name}`, raw, parseAnimationNode(raw), defaults[name])
    Object.assign(vars, animationVars(name, node, reducedMotion))
  }

  const name = typeof theme.name === 'string' && theme.name ? theme.name : null
  return { scheme, vars, rejected, name, font, terminal: palette, background }
}

/** A theme engine that remembers the last good value of every key. */
export function createZenThemeEngine(): ZenThemeEngine {
  const lastGood: ZenLastGood = new Map()
  let warned = ''
  return (input) => {
    const result = buildZenTheme(input, lastGood)
    const now = (result.rejected ?? []).join(', ')
    if (now && now !== warned) console.warn('[zen] theme keys K2 does not know were ignored:', now)
    warned = now
    return result
  }
}

let unregister: (() => void) | null = null

/** Register the S5 engine through the S4 plug-in point. Idempotent. */
export function installZenThemeEngine(): () => void {
  if (!unregister) {
    const off = registerZenThemeEngine(createZenThemeEngine())
    unregister = () => {
      off()
      unregister = null
    }
  }
  return unregister
}
