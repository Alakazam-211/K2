// @vitest-environment jsdom
//
// prd-zen-mode-v1 S5 — the Zen theme engine (Z20–Z22, Z26, Z44, Z57).
//
// Asserted (fail loudly):
//   - only known token names become `--zen-*` properties; unknown keys are
//     rejected and never written, and no file text reaches CSS;
//   - a bad value keeps the last good one (else K2's default);
//   - Hyprland curves map to `cubic-bezier(...)` from the numbers (overshoot
//     kept), speed in tenths of a second becomes ms, styles are fixed presets;
//   - reduced motion wins (every duration 0ms, every keyframe none), reduced
//     transparency wins (nothing see-through);
//   - `auto` follows this computer's scheme; safe mode ignores the page;
//   - the renderer's default theme is the daemon's built-in `themes/default.toml`,
//     and its font stacks are the daemon's `FONT_FAMILIES`;
//   - the daemon's real `/cli/zen/get` theme shape reads with nothing rejected.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { buildZenTheme, createZenThemeEngine, installZenThemeEngine } from './zen-theme-engine'
import { zenThemeFor } from './zen-theme'
import {
  ZEN_COLOR_TOKENS,
  ZEN_DEFAULT_COLORS,
  ZEN_DEFAULT_NUMS,
  ZEN_DEFAULT_TERMINAL,
  ZEN_FONT_STACKS,
  ZEN_FONT_TABLE,
  ZEN_TERMINAL_KEYS,
  ZEN_THEME_VARS,
  ZEN_BUNDLE_VARS,
  parseZenColor,
  rgbCss,
  zenTerminalKeyOf,
} from './zen-tokens'
import {
  cubicBezierCss,
  resolveDefaultAnimations,
  zenAnimation,
  ZEN_ANIMATION_NAMES,
  ZEN_DEFAULT_ANIMATIONS,
  ZEN_KEYFRAMES_CSS,
  ZEN_MOTION_VARS,
} from './zen-motion'
import { parseZenChrome, ZEN_DEFAULT_CHROME } from './zen-chrome'
import { BUILTIN_TEXTING_PAGE, type ZenResolvedPage } from './zen-page'

function page(parts: { theme?: unknown; motion?: unknown; chrome?: unknown }): ZenResolvedPage {
  return { ...BUILTIN_TEXTING_PAGE, theme: parts.theme ?? null, motion: parts.motion ?? null, chrome: parts.chrome ?? null }
}

/** A daemon-shaped animation node. */
function node(on: boolean, speed: number, bezier: number[], style: string | null = null, extra: object = {}): object {
  return { on, speed, durationMs: on ? speed * 100 : 0, curve: 'x', bezier, ease: 'ignored', style, from: 'x', ...extra }
}

const ALLOWED = new Set([...ZEN_THEME_VARS, ...ZEN_BUNDLE_VARS, ...ZEN_MOTION_VARS])

let off: (() => void) | null = null
afterEach(() => {
  off?.()
  off = null
})

describe('tokens: only known names, never raw CSS', () => {
  it('rejects unknown keys at every level and writes only known --zen-* names', () => {
    const r = buildZenTheme({
      systemScheme: 'light',
      page: page({
        theme: {
          scheme: 'light',
          css: 'body { display: none }',
          colors: {
            light: { canvas: '#ffffff', acent: '#ff0000', 'background-image': 'url(https://evil)' },
            dark: { text: '#eeeeee', glow: '#00ff00' },
            purple: { canvas: '#800080' },
          },
          type: { family: 'serif', weight: 900 },
          shape: { radius: 6, padding: 40 },
        },
        motion: { animations: { bounce: node(true, 3, [0, 0, 1, 1]) }, keyframes: '@keyframes x {}' },
      }),
    })
    expect(r.rejected?.sort()).toEqual(
      [
        'theme.css',
        'colors.light.acent',
        'colors.light.background-image',
        'colors.dark.glow',
        'colors.purple',
        'type.weight',
        'shape.padding',
        'motion.animations.bounce',
        'motion.keyframes',
      ].sort(),
    )
    for (const k of Object.keys(r.vars)) expect(ALLOWED.has(k), `unexpected property ${k}`).toBe(true)
    // Every allowed property is written (the root never inherits a stale one).
    expect(Object.keys(r.vars).sort()).toEqual([...ALLOWED].sort())
    const all = Object.values(r.vars).join('\n')
    for (const bad of ['url(', 'display', '@keyframes', '#ff0000', '#800080', '#00ff00']) {
      expect(all.includes(bad), `"${bad}" leaked into CSS`).toBe(false)
    }
    expect(r.vars['--zen-canvas']).toBe('rgb(255, 255, 255)')
    expect(r.vars['--zen-font-family']).toContain('Georgia')
    expect(r.vars['--zen-radius']).toBe('6px')
  })

  it('a value that is not a colour or is out of range is never passed through', () => {
    const r = buildZenTheme({
      systemScheme: 'light',
      page: page({
        theme: {
          colors: { light: { canvas: 'red; background: url(x)', accent: 'rgb(1,2,3); color: red', text: 'expression(alert(1))' } },
          type: { size: 99, family: 'Comic Sans", x' },
          shape: { gap: -4, 'list-width': '300px' },
        },
      }),
    })
    const def = ZEN_DEFAULT_COLORS.light
    expect(r.vars['--zen-canvas']).toBe(rgbCss(parseZenColor(def.canvas)!))
    expect(r.vars['--zen-accent']).toBe(rgbCss(parseZenColor(def.accent)!))
    expect(r.vars['--zen-text']).toBe(rgbCss(parseZenColor(def.text)!))
    expect(r.vars['--zen-font-size']).toBe(`${ZEN_DEFAULT_NUMS['font.size']}px`)
    expect(r.vars['--zen-gap']).toBe(`${ZEN_DEFAULT_NUMS['shape.gap']}px`)
    expect(r.vars['--zen-list-width']).toBe(`${ZEN_DEFAULT_NUMS['shape.list-width']}px`)
    expect(r.vars['--zen-font-family']).not.toContain('Comic')
  })

  it('a bad value keeps the LAST GOOD one, per key', () => {
    const engine = createZenThemeEngine()
    const good = engine({
      systemScheme: 'dark',
      page: page({ theme: { scheme: 'dark', colors: { dark: { accent: '#123456' } }, shape: { radius: 3 } } }),
    })
    expect(good.vars['--zen-accent']).toBe('rgb(18, 52, 86)')
    expect(good.vars['--zen-radius']).toBe('3px')
    const bad = engine({
      systemScheme: 'dark',
      page: page({ theme: { scheme: 'neon', colors: { dark: { accent: 'not a colour' } }, shape: { radius: 400 } } }),
    })
    expect(bad.scheme).toBe('dark')
    expect(bad.vars['--zen-accent']).toBe('rgb(18, 52, 86)')
    expect(bad.vars['--zen-radius']).toBe('3px')
    // While a page reloads (null), the last good look stays: no flicker.
    const loading = engine({ systemScheme: 'dark', page: null })
    expect(loading.vars['--zen-accent']).toBe('rgb(18, 52, 86)')
  })

  it('scheme auto follows this computer; light/dark are fixed', () => {
    const auto = page({ theme: { scheme: 'auto' } })
    expect(buildZenTheme({ systemScheme: 'dark', page: auto }).scheme).toBe('dark')
    expect(buildZenTheme({ systemScheme: 'light', page: auto }).scheme).toBe('light')
    const light = page({ theme: { scheme: 'light' } })
    expect(buildZenTheme({ systemScheme: 'dark', page: light }).scheme).toBe('light')
    expect(buildZenTheme({ systemScheme: 'dark', page: light }).vars['--zen-canvas']).toBe(
      rgbCss(parseZenColor(ZEN_DEFAULT_COLORS.light.canvas)!),
    )
  })

  it('reduced transparency wins: colours with alpha are made opaque against the canvas', () => {
    const p = page({
      theme: { scheme: 'light', colors: { light: { canvas: '#ffffff', accent: 'rgba(0, 0, 0, 0.5)', border: '#00000080' } } },
    })
    const normal = buildZenTheme({ systemScheme: 'light', page: p })
    expect(normal.vars['--zen-accent']).toBe('rgba(0, 0, 0, 0.5)')
    const reduced = buildZenTheme({ systemScheme: 'light', page: p, reducedTransparency: true })
    expect(reduced.vars['--zen-accent']).toBe('rgb(128, 128, 128)')
    expect(reduced.vars['--zen-border']).toBe('rgb(127, 127, 127)')
    for (const tok of ZEN_COLOR_TOKENS) expect(reduced.vars[`--zen-${tok}`]).toMatch(/^rgb\(/)
  })

  it('safe mode ignores the page and draws K2’s default; the S5 engine is what the root gets', () => {
    off = installZenThemeEngine()
    const p = page({ theme: { scheme: 'light', colors: { light: { accent: '#010203' } } } })
    expect(zenThemeFor(p, 'light', false).vars['--zen-accent']).toBe('rgb(1, 2, 3)')
    expect(zenThemeFor(p, 'light', true).vars['--zen-accent']).toBe(
      rgbCss(parseZenColor(ZEN_DEFAULT_COLORS.light.accent)!),
    )
  })
})

describe('motion: Hyprland curves and animation lines', () => {
  it('curves map to cubic-bezier from the numbers, overshoot kept; the daemon’s ease string is not used', () => {
    const r = buildZenTheme({
      systemScheme: 'light',
      page: page({
        motion: {
          beziers: { bouncy: [0.34, 1.56, 0.64, 1] },
          animations: {
            rowIn: node(true, 4, [0.34, 1.56, 0.64, 1], 'slidefade', { ease: 'url(evil)' }),
            messageIn: node(true, 2.5, [0.05, -0.6, 0.2, 2.25], 'popin 85%'),
          },
        },
      }),
    })
    expect(r.vars['--zen-anim-rowIn-ease']).toBe('cubic-bezier(0.34, 1.56, 0.64, 1)')
    expect(r.vars['--zen-anim-rowIn-duration']).toBe('400ms')
    expect(r.vars['--zen-anim-rowIn-keyframes']).toBe('zen-kf-slidefade')
    expect(r.vars['--zen-anim-messageIn-ease']).toBe('cubic-bezier(0.05, -0.6, 0.2, 2.25)')
    expect(r.vars['--zen-anim-messageIn-duration']).toBe('250ms')
    expect(r.vars['--zen-anim-messageIn-keyframes']).toBe('zen-kf-popin')
    expect(r.vars['--zen-anim-messageIn-scale']).toBe('0.85')
    expect(Object.values(r.vars).join(' ')).not.toContain('evil')
    expect(cubicBezierCss([0.22, 1, 0.36, 1])).toBe('cubic-bezier(0.22, 1, 0.36, 1)')
    expect(cubicBezierCss([0, 0, 1, 1])).toBe('cubic-bezier(0, 0, 1, 1)')
  })

  it('a bad node (x outside 0–1, unknown style, speed over 100) keeps the default for that line', () => {
    const def = resolveDefaultAnimations()
    const r = buildZenTheme({
      systemScheme: 'light',
      page: page({
        motion: {
          animations: {
            rowIn: node(true, 3, [1.5, 0, 1, 1], 'slide'),
            zenIn: node(true, 3, [0, 0, 1, 1], 'spin'),
            zenOut: node(true, 500, [0, 0, 1, 1]),
          },
        },
      }),
    })
    expect(r.vars['--zen-anim-rowIn-ease']).toBe(cubicBezierCss(def.rowIn.bezier))
    expect(r.vars['--zen-anim-zenIn-keyframes']).toBe('zen-kf-fade')
    expect(r.vars['--zen-anim-zenOut-duration']).toBe(`${def.zenOut.speed * 100}ms`)
  })

  it('off is instant and has no keyframes; workingPulse loops a pulse', () => {
    const r = buildZenTheme({
      systemScheme: 'light',
      page: page({ motion: { animations: { rowMove: { on: false, speed: 0, bezier: [0, 0, 1, 1], style: null } } } }),
    })
    expect(r.vars['--zen-anim-rowMove-duration']).toBe('0ms')
    expect(r.vars['--zen-anim-rowMove-keyframes']).toBe('none')
    expect(r.vars['--zen-anim-workingPulse-keyframes']).toBe('zen-kf-pulse')
    expect(r.vars['--zen-anim-workingPulse-duration']).toBe('1200ms')
    expect(zenAnimation('workingPulse').animationIterationCount).toBe('infinite')
    expect(zenAnimation('rowIn')).toMatchObject({
      animationName: 'var(--zen-anim-rowIn-keyframes)',
      animationDuration: 'var(--zen-anim-rowIn-duration)',
      animationTimingFunction: 'var(--zen-anim-rowIn-ease)',
      animationIterationCount: '1',
    })
  })

  it('the default tree: a child inherits its parent until it is set', () => {
    const d = resolveDefaultAnimations()
    // zen, rows, messages, conversation take global's line (3, glide, no style).
    for (const n of ['zen', 'rows', 'messages', 'conversation']) {
      expect(d[n].speed, n).toBe(3)
      expect(d[n].bezier, n).toEqual([0.22, 1, 0.36, 1])
      expect(d[n].style, n).toBeNull()
    }
    expect(d.messageIn.style).toBe('popin 92%')
    expect(d.workingPulse.bezier).toEqual([0.45, 0, 0.55, 1])
    expect(d.working.speed).toBe(3)
  })

  it('reduced motion wins: every duration is 0ms and every keyframe none, defaults and user lines alike', () => {
    const p = page({ motion: { animations: { rowIn: node(true, 9, [0.34, 1.56, 0.64, 1], 'slide') } } })
    for (const input of [
      { systemScheme: 'light' as const, page: p, reducedMotion: true },
      { systemScheme: 'light' as const, page: null, reducedMotion: true },
    ]) {
      const r = buildZenTheme(input)
      for (const n of ZEN_ANIMATION_NAMES) {
        expect(r.vars[`--zen-anim-${n}-duration`], n).toBe('0ms')
        expect(r.vars[`--zen-anim-${n}-keyframes`], n).toBe('none')
      }
    }
    expect(zenThemeFor(p, 'light', true, { reducedMotion: true }).vars['--zen-anim-zenIn-duration']).toBe('0ms')
    // The fixed sheet repeats it for anything that hard-codes a duration.
    expect(ZEN_KEYFRAMES_CSS).toMatch(/@media \(prefers-reduced-motion: reduce\)[\s\S]*animation-duration: 0ms !important/)
  })
})

describe('the default template theme is the daemon’s built-in themes/default.toml', () => {
  const toml = readFileSync(resolve(process.cwd(), 'crates/k2-core/src/zen/themes/default.toml'), 'utf8')

  function table(name: string): Record<string, string> {
    const m = new RegExp(`^\\[${name.replace('.', '\\.')}\\]\\n([\\s\\S]*?)(?=^\\[|$(?![\\s\\S]))`, 'm').exec(toml)
    if (!m) throw new Error(`no [${name}] in themes/default.toml`)
    const out: Record<string, string> = {}
    for (const line of m[1].split('\n')) {
      const kv = /^([A-Za-z-]+)\s*=\s*("[^"]*"|\[[^\]]*\]|[^#\s]+)/.exec(line.trim())
      if (kv) out[kv[1]] = kv[2]
    }
    return out
  }
  const unq = (s: string): string => s.replace(/^"|"$/g, '')

  it('colours, font, shape, terminal palettes, chrome and animation lines match', () => {
    for (const scheme of ['light', 'dark'] as const) {
      const t = table(`colors.${scheme}`)
      expect(Object.keys(t).sort()).toEqual([...ZEN_COLOR_TOKENS].sort())
      for (const tok of ZEN_COLOR_TOKENS) expect(unq(t[tok]), `${scheme}.${tok}`).toBe(ZEN_DEFAULT_COLORS[scheme][tok])
      const term = table(`terminal.${scheme}`)
      const keys = Object.keys(term).map((k) => {
        const key = zenTerminalKeyOf(k)
        if (!key) throw new Error(`terminal.${scheme}.${k} has no renderer key`)
        return key
      })
      expect(keys.sort()).toEqual([...ZEN_TERMINAL_KEYS].sort())
      for (const [k, v] of Object.entries(term)) {
        expect(unq(v), `terminal.${scheme}.${k}`).toBe(ZEN_DEFAULT_TERMINAL[scheme][zenTerminalKeyOf(k)!])
      }
    }
    const font = table('font')
    const shape = table('shape')
    expect(unq(font.family)).toBe('system')
    expect(Number(font.size)).toBe(ZEN_DEFAULT_NUMS['font.size'])
    expect(Number(font['line-height'])).toBe(ZEN_DEFAULT_NUMS['font.line-height'])
    for (const k of ['radius', 'bubble-radius', 'gap', 'list-width']) {
      expect(Number(shape[k]), k).toBe(ZEN_DEFAULT_NUMS[`shape.${k}`])
    }
    const chrome = table('chrome')
    expect(parseZenChrome({
      corners: unq(chrome.corners),
      stoplights: unq(chrome.stoplights),
      'stoplight-offset': JSON.parse(chrome['stoplight-offset']),
    })).toEqual(ZEN_DEFAULT_CHROME)
    const anim = table('animation')
    expect(Object.keys(anim).sort()).toEqual(Object.keys(ZEN_DEFAULT_ANIMATIONS).sort())
    for (const [name, line] of Object.entries(ZEN_DEFAULT_ANIMATIONS)) {
      const arr = JSON.parse(anim[name]) as unknown[]
      expect(arr, name).toEqual([line.on ? 1 : 0, line.speed, line.curve, ...(line.style ? [line.style] : [])])
    }
  })

  it('the font table is the daemon’s FONT_FAMILIES: names, stacks and fixed-width flags', () => {
    const rs = readFileSync(resolve(process.cwd(), 'crates/k2-core/src/zen/schema.rs'), 'utf8')
    const block = /pub const FONT_FAMILIES: &\[\(&str, &str, bool\)\] = &\[([\s\S]*?)\n\];/.exec(rs)
    if (!block) throw new Error('no FONT_FAMILIES in schema.rs')
    const rows = [...block[1].matchAll(/\("([a-z-]+)", "((?:[^"\\]|\\.)*)", (true|false)\)/g)].map((m) => [
      m[1],
      m[2].replace(/\\"/g, '"'),
      m[3] === 'true',
    ])
    expect(rows.length).toBeGreaterThan(0)
    expect(rows).toEqual(ZEN_FONT_TABLE.map((f) => [...f]))
    expect(/pub const TERMINAL_PARTNER: &str = "meslo";/.test(rs)).toBe(true)
  })

  it('no page draws exactly the default: the light palette, the system font, meslo terminals', () => {
    const r = buildZenTheme({ systemScheme: 'light', page: null })
    expect(r.font).toBe('system')
    expect(r.vars['--zen-font-family']).toBe(ZEN_FONT_STACKS.system)
    expect(r.vars['--zen-terminal-font-family']).toBe(ZEN_FONT_STACKS.meslo)
    expect(r.terminal?.background).toBe(rgbCss(parseZenColor(ZEN_DEFAULT_TERMINAL.light.background)!))
    expect(r.background).toBeNull()
  })
})

describe('chrome block parsing', () => {
  it('reads only known values; hidden lights, numeric radius and out-of-range offsets keep the last good', () => {
    expect(parseZenChrome({ corners: 'square', stoplights: 'square', 'stoplight-offset': [4, 24] })).toEqual({
      corners: 'square',
      stoplights: 'square',
      offset: [4, 24],
    })
    const last = { corners: 'square' as const, stoplights: 'square' as const, offset: [2, 2] as [number, number] }
    expect(parseZenChrome({ corners: 12, stoplights: 'hidden', 'stoplight-offset': [30, 0] }, last)).toEqual(last)
    expect(parseZenChrome(null)).toEqual(ZEN_DEFAULT_CHROME)
  })
})

describe('theme bundles (Omarchy additions 1 and 5)', () => {
  const PNG =
    'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='

  /** A palette as the daemon sends it: TOML-style keys, every one present. */
  function daemonPalette(scheme: 'light' | 'dark', over: Record<string, string> = {}): Record<string, string> {
    const out: Record<string, string> = {}
    for (const k of ZEN_TERMINAL_KEYS) {
      const toml = k === 'cursorAccent' ? 'cursor-text' : k.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)
      out[toml] = ZEN_DEFAULT_TERMINAL[scheme][k]
    }
    return { ...out, ...over }
  }

  /** `theme` exactly as `GET /cli/zen/get` returns it (store.rs `resolve`). */
  function daemonTheme(extra: Record<string, unknown> = {}): Record<string, unknown> {
    return {
      name: 'tokyo-night',
      builtin: false,
      user: true,
      scope: 'global',
      tokens: {
        scheme: 'dark',
        colors: { light: { ...ZEN_DEFAULT_COLORS.light }, dark: { ...ZEN_DEFAULT_COLORS.dark, accent: '#7aa2f7' } },
        shape: { radius: 10, 'bubble-radius': 18, gap: 12, 'list-width': 300 },
      },
      font: {
        family: 'jetbrains-mono',
        stack: '"JetBrains Mono", ui-monospace, Menlo, Consolas, monospace',
        monospace: true,
        size: 15,
        lineHeight: 1.5,
        terminal: { family: 'jetbrains-mono', stack: '"JetBrains Mono", ui-monospace, Menlo, Consolas, monospace', monospace: true },
      },
      terminal: {
        palette: {
          light: daemonPalette('light'),
          dark: daemonPalette('dark', { background: '#1a1b26', red: '#f7768e', 'bright-white': '#fefefe', 'cursor-text': '#010203' }),
        },
      },
      ...extra,
    }
  }
  const bundle = (extra: Record<string, unknown> = {}): ZenResolvedPage => page({ theme: daemonTheme(extra) })

  it('reads the daemon’s theme: tokens, font block, per-scheme palette, name — nothing rejected', () => {
    const r = buildZenTheme({ systemScheme: 'light', page: bundle() })
    expect(r.rejected).toEqual([])
    expect(r.name).toBe('tokyo-night')
    expect(r.scheme).toBe('dark')
    expect(r.vars['--zen-accent']).toBe('rgb(122, 162, 247)')
    expect(r.vars['--zen-radius']).toBe('10px')
    expect(r.vars['--zen-font-size']).toBe('15px')
    expect(r.vars['--zen-line-height']).toBe('1.5')
    // One font: the UI and the terminals, from K2's own table.
    expect(r.font).toBe('jetbrains-mono')
    expect(r.vars['--zen-font-family']).toBe(ZEN_FONT_STACKS['jetbrains-mono'])
    expect(r.vars['--zen-terminal-font-family']).toBe(r.vars['--zen-font-family'])
    // The dark palette (the active scheme), TOML keys mapped to xterm keys.
    expect(r.terminal?.background).toBe('rgb(26, 27, 38)')
    expect(r.terminal?.red).toBe('rgb(247, 118, 142)')
    expect(r.terminal?.cursorAccent).toBe('rgb(1, 2, 3)')
    expect(r.vars['--zen-term-bright-white']).toBe('rgb(254, 254, 254)')
    expect(r.vars['--zen-term-red']).toBe('rgb(247, 118, 142)')
    expect(r.terminal?.cyan).toBe(rgbCss(parseZenColor(ZEN_DEFAULT_TERMINAL.dark.cyan)!))
    // The light palette follows the scheme.
    const light = buildZenTheme({ systemScheme: 'light', page: bundle({ tokens: { scheme: 'light' } }) })
    expect(light.terminal?.background).toBe(rgbCss(parseZenColor(ZEN_DEFAULT_TERMINAL.light.background)!))
  })

  it('a proportional font drives the UI; terminals take the daemon’s fixed-width partner', () => {
    const r = buildZenTheme({
      systemScheme: 'light',
      page: bundle({
        font: {
          family: 'serif',
          stack: 'ignored',
          monospace: false,
          size: 14,
          lineHeight: 1.45,
          terminal: { family: 'meslo', stack: 'x', monospace: true },
        },
      }),
    })
    expect(r.vars['--zen-font-family']).toBe(ZEN_FONT_STACKS.serif)
    expect(r.vars['--zen-terminal-font-family']).toBe(ZEN_FONT_STACKS.meslo)
    // A proportional or unknown terminal family never reaches a terminal.
    const odd = buildZenTheme({
      systemScheme: 'light',
      page: bundle({ font: { family: 'rounded', terminal: { family: 'serif' } } }),
    })
    expect(odd.vars['--zen-terminal-font-family']).toBe(ZEN_FONT_STACKS.meslo)
  })

  it('the daemon’s stack text never reaches CSS', () => {
    const r = buildZenTheme({
      systemScheme: 'light',
      page: bundle({
        font: { family: 'nope', stack: 'x; background: url(evil)', terminal: { family: 'lilex', stack: 'url(evil)' } },
      }),
    })
    expect(r.vars['--zen-font-family']).toBe(ZEN_FONT_STACKS.system)
    expect(r.vars['--zen-terminal-font-family']).toBe(ZEN_FONT_STACKS.lilex)
    expect(Object.values(r.vars).join(' ')).not.toContain('evil')
  })

  it('older shapes still read: a bare font name, an alias, a flat xterm palette, a bare data URL', () => {
    const r = buildZenTheme({
      systemScheme: 'light',
      page: page({
        theme: {
          tokens: { scheme: 'dark' },
          font: 'JetBrains Mono',
          terminal: { palette: { red: '#aa0000', brightWhite: '#fefefe' } },
          background: PNG,
        },
      }),
    })
    expect(r.rejected).toEqual([])
    expect(r.font).toBe('jetbrains-mono')
    expect(r.terminal?.red).toBe('rgb(170, 0, 0)')
    expect(r.terminal?.brightWhite).toBe('rgb(254, 254, 254)')
    expect(r.background).toEqual({ src: PNG, fit: 'cover', opacity: 1 })
  })

  it('rejects unknown bundle, font, palette and background keys and never writes a remote or non-image background', () => {
    const r = buildZenTheme({
      systemScheme: 'light',
      page: bundle({
        wallpaperBlur: 12,
        font: { family: 'mono', weight: 700, terminal: { family: 'mono', ligatures: true } },
        terminal: { palette: { light: { background: '#000000', sparkle: '#ff00ff' }, sepia: {} }, cursorBlink: true },
        background: { dataUrl: 'https://example.com/a.png', dim: 0.2 },
      }),
    })
    expect(r.rejected?.sort()).toEqual(
      [
        'theme.wallpaperBlur',
        'font.weight',
        'font.terminal.ligatures',
        'terminal.palette.sepia',
        'terminal.palette.light.sparkle',
        'terminal.cursorBlink',
        'background.dim',
      ].sort(),
    )
    expect(r.background).toBeNull()
    for (const bad of ['https://x/a.png', 'data:text/html;base64,PGgxPg==', 'data:image/svg+xml;base64,PHN2Zz4=', 'url(x)']) {
      const b = buildZenTheme({ systemScheme: 'light', page: bundle({ background: { dataUrl: bad, fit: 'cover', opacity: 1 } }) })
      expect(b.background, bad).toBeNull()
    }
  })

  it('background: the daemon’s dataUrl with fit and opacity; bad ones default; reduced transparency drops it', () => {
    const bg = (over: object = {}): object => ({
      dataUrl: PNG,
      mime: 'image/png',
      bytes: 68,
      file: 'background.png',
      fit: 'tile',
      opacity: 0.4,
      lastGood: false,
      ...over,
    })
    const r = buildZenTheme({ systemScheme: 'light', page: bundle({ background: bg() }) })
    expect(r.rejected).toEqual([])
    expect(r.background).toEqual({ src: PNG, fit: 'tile', opacity: 0.4 })
    const lastGood = buildZenTheme({ systemScheme: 'light', page: bundle({ background: bg({ lastGood: true, fit: 'contain' }) }) })
    expect(lastGood.background).toEqual({ src: PNG, fit: 'contain', opacity: 0.4 })
    const odd = buildZenTheme({ systemScheme: 'light', page: bundle({ background: bg({ fit: 'stretch', opacity: 3 }) }) })
    expect(odd.background).toEqual({ src: PNG, fit: 'cover', opacity: 1 })
    const reduced = buildZenTheme({
      systemScheme: 'light',
      page: bundle({ background: bg() }),
      reducedTransparency: true,
    })
    expect(reduced.background).toBeNull()
  })

  it('a bad bundle value keeps the last good one; a theme switch puts left-out keys back to defaults', () => {
    const engine = createZenThemeEngine()
    engine({ systemScheme: 'dark', page: bundle({ background: { dataUrl: PNG, fit: 'cover', opacity: 1 } }) })
    const bad = engine({
      systemScheme: 'dark',
      page: bundle({
        font: { family: 'Papyrus' },
        background: { dataUrl: 'nope', fit: 'cover', opacity: 1 },
        terminal: { palette: { light: daemonPalette('light'), dark: daemonPalette('dark', { red: 'nope' }) } },
      }),
    })
    expect(bad.font).toBe('jetbrains-mono')
    expect(bad.background?.src).toBe(PNG)
    expect(bad.terminal?.red).toBe('rgb(247, 118, 142)')
    const other = engine({ systemScheme: 'dark', page: page({ theme: { name: 'plain', tokens: { scheme: 'dark' } } }) })
    expect(other.name).toBe('plain')
    expect(other.font).toBe('system')
    expect(other.background).toBeNull()
    expect(other.terminal?.red).toBe(rgbCss(parseZenColor(ZEN_DEFAULT_TERMINAL.dark.red)!))
  })

  it('ANSI-array palettes', () => {
    const ansi = Array.from({ length: 16 }, (_, i) => `#0000${i.toString(16).padStart(2, '0')}`)
    const arr = buildZenTheme({ systemScheme: 'light', page: page({ theme: { tokens: {}, terminal: { palette: ansi } } }) })
    expect(arr.terminal?.black).toBe('rgb(0, 0, 0)')
    expect(arr.terminal?.brightWhite).toBe('rgb(0, 0, 15)')
  })
})
